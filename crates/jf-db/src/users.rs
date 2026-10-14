//! Multi-user management: accounts, per-user policy/config, and sessions.

use crate::{Database, DbError, Result};
use jf_core::dto::{
    SessionInfo, UserConfiguration, UserPolicy, UserRecord,
};
use jf_core::new_item_id;
use sqlx::Row;

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::Argon2;

/// Hash a plaintext password into an Argon2id PHC string.
///
/// `password-hash` 0.6 generates the salt internally, so there is no salt
/// argument here.
pub fn hash_password(plain: &str) -> Result<String> {
    Argon2::default()
        .hash_password(plain.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| DbError::NotFound(format!("password hashing failed: {e}")))
}

/// Verify a plaintext password against a stored PHC hash.
///
/// Returns `false` rather than an error for malformed hashes so that a corrupt
/// row cannot be distinguished from a wrong password by a caller.
pub fn verify_password(plain: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default()
            .verify_password(plain.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Parameters for creating a new user.
pub struct NewUser<'a> {
    pub username: &'a str,
    /// `None` creates a passwordless user (Quick Connect style).
    pub password: Option<&'a str>,
    pub server_id: &'a str,
    pub policy: UserPolicy,
    pub configuration: UserConfiguration,
}

impl Database {
    /// Create a user. Usernames are case-insensitively unique.
    pub async fn create_user(&self, new: NewUser<'_>) -> Result<UserRecord> {
        let username = new.username.trim();
        if username.is_empty() {
            return Err(DbError::NotFound("username must not be empty".into()));
        }

        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM users WHERE username_lower = ?")
                .bind(username.to_lowercase())
                .fetch_optional(self.pool())
                .await?;
        if existing.is_some() {
            return Err(DbError::NotFound(format!("username `{username}` is taken")));
        }

        let id = new_item_id();
        let created_at = now_iso();
        let password_hash = new.password.map(hash_password).transpose()?;

        sqlx::query(
            "INSERT INTO users
                (id, username, username_lower, password_hash, server_id,
                 last_login_date, last_activity_date, created_at, policy_json, config_json)
             VALUES (?, ?, ?, ?, ?, NULL, NULL, ?, ?, ?)",
        )
        .bind(&id)
        .bind(username)
        .bind(username.to_lowercase())
        .bind(&password_hash)
        .bind(new.server_id)
        .bind(&created_at)
        .bind(serde_json::to_string(&new.policy).unwrap_or_else(|_| "{}".into()))
        .bind(serde_json::to_string(&new.configuration).unwrap_or_else(|_| "{}".into()))
        .execute(self.pool())
        .await?;

        self.user(&id).await?.ok_or_else(|| DbError::NotFound("user vanished after insert".into()))
    }

    /// Fetch a user by id.
    pub async fn user(&self, id: &str) -> Result<Option<UserRecord>> {
        let row = sqlx::query("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        row.map(row_to_user).transpose()
    }

    /// Fetch a user by case-insensitive username.
    pub async fn user_by_name(&self, username: &str) -> Result<Option<UserRecord>> {
        let row = sqlx::query("SELECT * FROM users WHERE username_lower = ?")
            .bind(username.trim().to_lowercase())
            .fetch_optional(self.pool())
            .await?;
        row.map(row_to_user).transpose()
    }

    /// List all users, including hidden ones.
    pub async fn all_users(&self) -> Result<Vec<UserRecord>> {
        let rows = sqlx::query("SELECT * FROM users ORDER BY username_lower")
            .fetch_all(self.pool())
            .await?;
        rows.into_iter().map(row_to_user).collect()
    }

    /// List users visible on the login screen.
    pub async fn public_users(&self) -> Result<Vec<UserRecord>> {
        let rows = sqlx::query("SELECT * FROM users WHERE is_hidden = 0 ORDER BY username_lower")
            .fetch_all(self.pool())
            .await?;
        rows.into_iter().map(row_to_user).collect()
    }

    pub async fn user_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(self.pool())
            .await?)
    }

    /// Verify credentials and record the login timestamp.
    ///
    /// Returns `None` for both an unknown user and a bad password so callers
    /// cannot enumerate accounts.
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<UserRecord>> {
        let Some(user) = self.user_by_name(username).await? else {
            return Ok(None);
        };

        if user.policy.is_disabled {
            return Ok(None);
        }

        match user.password_hash.as_deref() {
            Some(phc) if verify_password(password, phc) => {}
            // Passwordless accounts accept an empty password (auto-login).
            None if password.is_empty() => {}
            _ => return Ok(None),
        }

        let last_login = now_iso();
        sqlx::query("UPDATE users SET last_login_date = ?, last_activity_date = ? WHERE id = ?")
            .bind(&last_login)
            .bind(&last_login)
            .bind(&user.id)
            .execute(self.pool())
            .await?;

        Ok(Some(self.user(&user.id).await?.unwrap_or(user)))
    }

    /// Replace a user's policy.
    pub async fn update_policy(&self, user_id: &str, policy: &UserPolicy) -> Result<()> {
        sqlx::query("UPDATE users SET policy_json = ? WHERE id = ?")
            .bind(serde_json::to_string(policy).unwrap_or_else(|_| "{}".into()))
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Replace a user's configuration.
    pub async fn update_configuration(
        &self,
        user_id: &str,
        config: &UserConfiguration,
    ) -> Result<()> {
        sqlx::query("UPDATE users SET config_json = ? WHERE id = ?")
            .bind(serde_json::to_string(config).unwrap_or_else(|_| "{}".into()))
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Set or clear a user's password.
    pub async fn set_password(&self, user_id: &str, password: Option<&str>) -> Result<()> {
        let hash = password.map(hash_password).transpose()?;
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(&hash)
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn delete_user(&self, user_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Record a user's avatar image tag.
    pub async fn set_user_avatar(&self, user_id: &str, tag: &str) -> Result<()> {
        sqlx::query("UPDATE users SET avatar_image_tag = ? WHERE id = ?")
            .bind(tag)
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn touch_activity(&self, user_id: &str) -> Result<()> {
        sqlx::query("UPDATE users SET last_activity_date = ? WHERE id = ?")
            .bind(now_iso())
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Sessions
    // -----------------------------------------------------------------

    /// Issue a session and access token for a logged-in client.
    pub async fn create_session(
        &self,
        user: &UserRecord,
        client: &str,
        device_name: &str,
        device_id: &str,
        app_version: &str,
    ) -> Result<(String, String)> {
        let id = new_item_id();
        let token = crate::generate_access_token();

        sqlx::query(
            "INSERT INTO sessions
                (id, user_id, access_token, client, device_name, device_id,
                 application_version, created_at, last_activity_date)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&user.id)
        .bind(&token)
        .bind(client)
        .bind(device_name)
        .bind(device_id)
        .bind(app_version)
        .bind(now_iso())
        .bind(now_iso())
        .execute(self.pool())
        .await?;

        Ok((id, token))
    }

    /// Resolve an access token to its session and owning user.
    pub async fn session_by_token(&self, token: &str) -> Result<Option<(String, UserRecord)>> {
        let row = sqlx::query("SELECT id, user_id FROM sessions WHERE access_token = ?")
            .bind(token)
            .fetch_optional(self.pool())
            .await?;

        let Some(row) = row else { return Ok(None) };
        let session_id: String = row.get("id");
        let user_id: String = row.get("user_id");

        match self.user(&user_id).await? {
            Some(user) => Ok(Some((session_id, user))),
            None => Ok(None),
        }
    }

    /// Update the last-activity stamp on a session.
    pub async fn touch_session(&self, session_id: &str) -> Result<()> {
        sqlx::query("UPDATE sessions SET last_activity_date = ? WHERE id = ?")
            .bind(now_iso())
            .bind(session_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn end_session(&self, token: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE access_token = ?")
            .bind(token)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Drop every session for a user, e.g. when their password changes.
    pub async fn end_user_sessions(&self, user_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Remove sessions that have exceeded a user's concurrent-session limit.
    /// A limit of zero means unlimited.
    pub async fn enforce_session_limit(&self, user_id: &str, limit: i32) -> Result<()> {
        if limit <= 0 {
            return Ok(());
        }

        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM sessions WHERE user_id = ?
             ORDER BY last_activity_date DESC, created_at DESC",
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;

        for stale in ids.into_iter().skip(limit as usize) {
            sqlx::query("DELETE FROM sessions WHERE id = ?")
                .bind(stale)
                .execute(self.pool())
                .await?;
        }
        Ok(())
    }

    /// List active sessions across all users, for the dashboard.
    pub async fn active_sessions(&self) -> Result<Vec<SessionInfo>> {
        let rows = sqlx::query(
            "SELECT s.*, u.username FROM sessions s
             JOIN users u ON u.id = s.user_id
             ORDER BY s.last_activity_date DESC",
        )
        .fetch_all(self.pool())
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| SessionInfo {
                id: row.get("id"),
                user_id: row.get("user_id"),
                user_name: row.get("username"),
                client: row.get("client"),
                device_name: row.get("device_name"),
                device_id: row.get("device_id"),
                application_version: row.get("application_version"),
                is_active: true,
                supports_media_control: true,
                supports_remote_control: true,
            })
            .collect())
    }
}

fn row_to_user(row: sqlx::sqlite::SqliteRow) -> Result<UserRecord> {
    let policy_json: String = row.get("policy_json");
    let config_json: String = row.get("config_json");

    // Tolerate a malformed policy by falling back to defaults rather than
    // failing the whole login.
    let policy: UserPolicy =
        serde_json::from_str(&policy_json).unwrap_or_else(|_| UserPolicy::default());
    let configuration: UserConfiguration =
        serde_json::from_str(&config_json).unwrap_or_else(|_| UserConfiguration::default());

    Ok(UserRecord {
        id: row.get("id"),
        username: row.get("username"),
        password_hash: row.get("password_hash"),
        server_id: row.get("server_id"),
        last_login_date: row.get("last_login_date"),
        last_activity_date: row.get("last_activity_date"),
        created_at: row.get("created_at"),
        policy,
        configuration,
    })
}

/// Current UTC timestamp in RFC 3339, matching Jellyfin's format.
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}