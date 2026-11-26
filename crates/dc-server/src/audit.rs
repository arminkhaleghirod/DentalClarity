//! Security audit events. Never record patient information here.

use anyhow::Result;
use sqlx::SqlitePool;

pub async fn record(db: &SqlitePool, actor: &str, action: &str, detail: &str) -> Result<()> {
    sqlx::query("INSERT INTO audit_events (actor, action, detail) VALUES (?, ?, ?)")
        .bind(actor)
        .bind(action)
        .bind(detail)
        .execute(db)
        .await?;
    Ok(())
}
