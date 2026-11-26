//! Key-value settings stored in SQLite.

use crate::AppState;
use anyhow::Result;
use sqlx::{Executor, Sqlite};

pub const PRACTICE_NAME: &str = "practice_name";
/// Mode the server last started in.
pub const MODE: &str = "mode";

pub async fn get<'e>(
    db: impl Executor<'e, Database = Sqlite>,
    key: &str,
) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|(v,)| v))
}

pub async fn set<'e>(
    db: impl Executor<'e, Database = Sqlite>,
    key: &str,
    value: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
    )
    .bind(key)
    .bind(value)
    .execute(db)
    .await?;
    Ok(())
}

/// The configured practice name, or the connector's name before setup.
pub async fn practice_name(state: &AppState) -> Result<String> {
    Ok(get(&state.db, PRACTICE_NAME)
        .await?
        .unwrap_or_else(|| state.default_practice_name.clone()))
}
