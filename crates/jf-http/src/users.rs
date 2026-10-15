//! User administration: accounts, roles and per-user access.
//!
//! The dashboard and library screens answer "what is on this server"; this
//! module answers "who may use it and what may they reach".
//!
//! Every handler here is administrator-only except a user changing their own
//! password, and each one re-checks the policy: the routes are not hidden from
//! a determined caller, so the check is the security boundary.
//!
//! Requests and responses follow Jellyfin's shapes (`POST /Users/New`,
//! `DELETE /Users/{id}`, `POST /Users/{id}/Password`) so a stock client could
//! drive the same endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use jf_core::dto::{UserConfiguration, UserDto, UserPolicy, UserRecord};
use jf_db::users::NewUser;
use serde::Deserialize;
use std::sync::Arc;

use crate::auth::RequireAuth;
use crate::routes::ApiError;
use crate::state::AppState;

type Shared = Arc<AppState>;

/// Longest username worth storing; Jellyfin's own limit is generous, but an
/// unbounded name is a cheap way to fill the database.
const MAX_USERNAME_LEN: usize = 64;

/// `GET /Users` — every account on the server.
///
/// Administrator-only: this is the full list including disabled accounts,
/// unlike `/Users/Public`, which is the login screen's subset.
pub async fn list_users(
    State(app): State<Shared>,
    auth: RequireAuth,
) -> Result<Json<Vec<UserDto>>, ApiError> {
    require_admin(&auth)?;

    let users = app.db.all_users().await.map_err(ApiError::from)?;
    Ok(Json(users.iter().map(UserRecord::to_dto).collect()))
}

/// Body of `POST /Users/New`.
#[derive(Debug, Deserialize)]
pub struct CreateUserDto {
    #[serde(rename = "Name")]
    pub name: Option<String>,
    #[serde(rename = "Password")]
    pub password: Option<String>,
    /// Convenience over Jellyfin's shape.
    ///
    /// Real Jellyfin creates the account and then patches the policy in a
    /// second request. Accepting the flag here lets our client pick the role
    /// in one step without a window where the account exists with the wrong
    /// one; a stock client that omits it still gets a standard member.
    #[serde(rename = "IsAdministrator", default)]
    pub is_administrator: bool,
}

/// `POST /Users/New` — create an account.
pub async fn create_user(
    State(app): State<Shared>,
    auth: RequireAuth,
    Json(body): Json<CreateUserDto>,
) -> Result<(StatusCode, Json<UserDto>), ApiError> {
    require_admin(&auth)?;

    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ApiError::bad_request("A username is required."))?;

    if name.chars().count() > MAX_USERNAME_LEN {
        return Err(ApiError::bad_request(format!(
            "A username may be at most {MAX_USERNAME_LEN} characters."
        )));
    }

    // Reject a duplicate up front so the caller gets a clear message rather
    // than the database's internal one.
    if app
        .db
        .user_by_name(name)
        .await
        .map_err(ApiError::from)?
        .is_some()
    {
        return Err(ApiError::bad_request(format!(
            "The username `{name}` is already taken."
        )));
    }

    let policy = if body.is_administrator {
        UserPolicy::administrator()
    } else {
        UserPolicy::default()
    };

    let user = app
        .db
        .create_user(NewUser {
            username: name,
            password: body.password.as_deref().filter(|p| !p.is_empty()),
            server_id: &app.server_id,
            policy,
            configuration: UserConfiguration::default(),
        })
        .await
        .map_err(ApiError::from)?;

    app.emit("usercreated", serde_json::json!({ "user_id": user.id }));

    Ok((StatusCode::OK, Json(user.to_dto())))
}

/// `DELETE /Users/{userId}` — remove an account.
pub async fn delete_user(
    State(app): State<Shared>,
    Path(user_id): Path<String>,
    auth: RequireAuth,
) -> Result<StatusCode, ApiError> {
    require_admin(&auth)?;

    if auth.0.user.id == user_id {
        return Err(ApiError::bad_request("You cannot delete your own account."));
    }

    let target = app
        .db
        .user(&user_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("No such user."))?;

    if is_active_admin(&target.policy) {
        ensure_admin_remains(&app, &user_id).await?;
    }

    app.db.delete_user(&user_id).await.map_err(ApiError::from)?;
    // The account is gone, so its tokens must stop working immediately rather
    // than the next time each session is touched.
    app.db
        .end_user_sessions(&user_id)
        .await
        .map_err(ApiError::from)?;

    app.emit("userdeleted", serde_json::json!({ "user_id": user_id }));

    Ok(StatusCode::NO_CONTENT)
}

/// Body of `POST /Users/{userId}/Password`.
#[derive(Debug, Deserialize)]
pub struct UpdatePasswordDto {
    /// Required when a non-administrator changes their own password.
    #[serde(rename = "CurrentPw", default)]
    pub current: Option<String>,
    #[serde(rename = "NewPw", default)]
    pub new: Option<String>,
    /// Alias used by some clients.
    #[serde(rename = "Password", default)]
    pub password: Option<String>,
    /// Clears the password instead of setting one (passwordless account).
    #[serde(rename = "ResetPassword", default)]
    pub reset: bool,
}

/// `POST /Users/{userId}/Password` — set or clear a password.
///
/// An administrator may reset anyone's password; everybody else may only
/// change their own, and must prove the current one first.
pub async fn update_password(
    State(app): State<Shared>,
    Path(user_id): Path<String>,
    auth: RequireAuth,
    Json(body): Json<UpdatePasswordDto>,
) -> Result<StatusCode, ApiError> {
    let is_self = auth.0.user.id == user_id;
    if !is_self && !auth.0.is_admin() {
        return Err(ApiError::forbidden("You may only change your own password."));
    }

    let target = app
        .db
        .user(&user_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("No such user."))?;

    let new_password = body.new.or(body.password);

    // Changing your own password requires the current one. An administrator
    // resetting somebody else's does not: they are recovering a locked-out
    // account and cannot know the old secret. An administrator changing their
    // own still has to prove it, or a stolen token becomes a permanent takeover.
    if is_self {
        if let Some(phc) = target.password_hash.as_deref() {
            let supplied = body.current.as_deref().unwrap_or_default();
            if !jf_db::users::verify_password(supplied, phc) {
                return Err(ApiError::forbidden("The current password is incorrect."));
            }
        }
    }

    if body.reset {
        app.db
            .set_password(&user_id, None)
            .await
            .map_err(ApiError::from)?;
    } else {
        let password = new_password
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| ApiError::bad_request("A new password is required."))?;

        if password.chars().count() > 512 {
            return Err(ApiError::bad_request("That password is too long."));
        }

        app.db
            .set_password(&user_id, Some(password))
            .await
            .map_err(ApiError::from)?;
    }

    // Resetting somebody else's password invalidates their sessions, since the
    // point is usually that they lost access. Changing your own keeps the
    // session you are using, so you are not signed out of the page you are on.
    if !is_self {
        app.db
            .end_user_sessions(&user_id)
            .await
            .map_err(ApiError::from)?;
    }

    Ok(StatusCode::NO_CONTENT)
}

fn require_admin(auth: &RequireAuth) -> Result<(), ApiError> {
    if auth.0.is_admin() {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "Managing users requires administrator access.",
        ))
    }
}

/// An administrator who can actually sign in.
///
/// A disabled administrator cannot log in, so counting them would let the last
/// usable admin be demoted while the server looked fine.
fn is_active_admin(policy: &UserPolicy) -> bool {
    policy.is_administrator && !policy.is_disabled
}

/// Refuse a change that would leave nobody able to administer the server.
///
/// Demoting or deleting the final administrator cannot be undone from inside
/// the application — the only way back is editing the database by hand — so it
/// is rejected rather than merely warned about.
pub(crate) async fn ensure_admin_remains(app: &Shared, excluding: &str) -> Result<(), ApiError> {
    let users = app.db.all_users().await.map_err(ApiError::from)?;
    if count_other_active_admins(&users, excluding) == 0 {
        return Err(ApiError::bad_request(
            "This would leave the server without an administrator. \
             Grant another account administrator access first.",
        ));
    }
    Ok(())
}

/// How many accounts besides `excluding` can still administer the server.
///
/// Split out as a pure function so the rule can be tested without a database.
fn count_other_active_admins(users: &[UserRecord], excluding: &str) -> usize {
    users
        .iter()
        .filter(|u| u.id != excluding && is_active_admin(&u.policy))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str, admin: bool, disabled: bool) -> UserRecord {
        UserRecord {
            id: id.into(),
            username: id.into(),
            password_hash: None,
            server_id: "server".into(),
            last_login_date: None,
            last_activity_date: None,
            created_at: "2024-01-01T00:00:00Z".into(),
            policy: UserPolicy {
                is_administrator: admin,
                is_disabled: disabled,
                ..Default::default()
            },
            configuration: UserConfiguration::default(),
        }
    }

    #[test]
    fn a_lone_administrator_counts_as_the_last_one() {
        let users = vec![user("admin", true, false), user("viewer", false, false)];
        assert_eq!(count_other_active_admins(&users, "admin"), 0);
        assert_eq!(count_other_active_admins(&users, "viewer"), 1);
    }

    #[test]
    fn a_second_administrator_makes_a_demotion_safe() {
        let users = vec![
            user("admin", true, false),
            user("ops", true, false),
            user("viewer", false, false),
        ];
        assert_eq!(count_other_active_admins(&users, "admin"), 1);
    }

    #[test]
    fn a_disabled_administrator_cannot_be_the_safety_net() {
        // The trap: two accounts carry the administrator flag, but one cannot
        // sign in, so demoting the other would still brick the server.
        let users = vec![user("admin", true, false), user("suspended", true, true)];
        assert_eq!(
            count_other_active_admins(&users, "admin"),
            0,
            "a disabled admin must not count as cover"
        );
    }

    #[test]
    fn a_disabled_administrator_is_itself_protected_by_the_others() {
        let users = vec![user("admin", true, false), user("suspended", true, true)];
        // `suspended` is not an active admin, so it is not "the last one" and
        // this returns 1 (the healthy `admin`).
        assert_eq!(count_other_active_admins(&users, "suspended"), 1);
    }
}
