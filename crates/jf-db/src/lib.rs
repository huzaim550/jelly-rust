//! SQLite persistence layer.
//!
//! Uses the runtime (non-macro) query API so the crate builds without a
//! `DATABASE_URL` or a pre-generated `.sqlx` cache.

pub mod users;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;

/// Errors surfaced by the persistence layer.
#[derive(Debug, Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("corrupt stored JSON in `{table}.{column}`: {source}")]
    CorruptJson {
        table: &'static str,
        column: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("{0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, DbError>;

/// Handle to the server database.
#[derive(Clone, Debug)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Open (creating if needed) the database at `path`, apply pragmas and run
    /// all migrations.
    pub async fn connect(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // WAL keeps the scanner readable while playback writes progress.
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .foreign_keys(true)
            // The scanner holds a write transaction for long periods.
            .busy_timeout(Duration::from_secs(30));

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(30))
            .connect_with(options)
            .await?;

        let db = Self { pool };
        sqlx::migrate!("./migrations").run(&db.pool).await?;
        Ok(db)
    }

    /// Open an in-memory database. Used by tests.
    pub async fn connect_memory() -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .shared_cache(true)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;

        let db = Self { pool };
        sqlx::migrate!("./migrations").run(&db.pool).await?;
        Ok(db)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Ensure the server row exists and return its id.
    pub async fn ensure_server(&self, name: &str, version: &str) -> Result<String> {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM server_info LIMIT 1")
                .fetch_optional(&self.pool)
                .await?;

        if let Some(id) = existing {
            return Ok(id);
        }

        let id = jf_core::new_item_id();
        sqlx::query("INSERT INTO server_info (id, name, version, startup_completed) VALUES (?, ?, ?, 0)")
            .bind(&id)
            .bind(name)
            .bind(version)
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    /// Mark the first-run wizard as finished.
    pub async fn complete_startup_wizard(&self) -> Result<()> {
        sqlx::query("UPDATE server_info SET startup_completed = 1")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn startup_completed(&self) -> Result<bool> {
        let row = sqlx::query("SELECT startup_completed FROM server_info LIMIT 1")
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| DbError::NotFound("server_info is empty".into()))?;
        Ok(row.get::<i64, _>("startup_completed") != 0)
    }

    pub async fn server_name(&self) -> Result<String> {
        Ok(sqlx::query_scalar("SELECT name FROM server_info LIMIT 1")
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| DbError::NotFound("server_info is empty".into()))?)
    }

    /// Check a database column value parses as JSON, mapping failures to a
    /// descriptive error instead of a bare serde message.
    pub fn json_column<T: serde::de::DeserializeOwned>(
        raw: &str,
        table: &'static str,
        column: &'static str,
    ) -> Result<T> {
        serde_json::from_str(raw).map_err(|source| DbError::CorruptJson { table, column, source })
    }
}

/// Decode a `TEXT` JSON column that may be NULL.
pub fn opt_json_column<T: serde::de::DeserializeOwned>(
    raw: Option<String>,
    table: &'static str,
    column: &'static str,
) -> Result<Option<T>>
where
    T: Default,
{
    match raw {
        None => Ok(None),
        Some(s) => Ok(Some(Database::json_column(&s, table, column)?)),
    }
}

/// Mint a 256-bit random access token, hex encoded.
///
/// Built from two UUIDv4 draws rather than `rand_core` directly: argon2 0.6
/// pins an older `rand_core`, and juggling two incompatible versions of the
/// same crate for one function is not worth it. UUIDv4 draws 122 bits of
/// randomness each, so two give 244 bits of a 256-bit token.
pub fn generate_access_token() -> String {
    let mut token = String::with_capacity(64);
    token.push_str(&uuid::Uuid::new_v4().simple().to_string());
    token.push_str(&uuid::Uuid::new_v4().simple().to_string());
    token
}

/// Helper for reading a required JSON column.
pub fn json_column<T: serde::de::DeserializeOwned>(
    raw: &str,
    table: &'static str,
    column: &'static str,
) -> Result<T> {
    Database::json_column(raw, table, column)
}

/// Read a boolean stored as an INTEGER.
pub fn bool_col(row: &SqliteRow, name: &str) -> bool {
    row.get::<i64, _>(name) != 0
}

/// Build connect options for a file path without connecting.
pub fn connect_options(path: impl AsRef<std::path::Path>) -> SqliteConnectOptions {
    SqliteConnectOptions::new().filename(path.as_ref())
}

/// Parse an ffmpeg-reported bitrate string into bits per second.
pub fn parse_bitrate(raw: &str) -> Option<i64> {
    raw.trim().parse::<i64>().ok()
}

/// Parse an ffprobe duration string (`"1234.567890"`) into runtime ticks.
pub fn parse_duration_ticks(raw: &str) -> Option<i64> {
    let secs: f64 = raw.trim().parse().ok()?;
    Some(jf_core::secs_to_ticks(secs))
}

/// Convenience for parsing options from a string.
pub fn options_from_str(s: &str) -> Option<SqliteConnectOptions> {
    SqliteConnectOptions::from_str(s).ok()
}