//! Security audit events. Never record patient information here.

use anyhow::Result;
use sqlx::{Executor, Sqlite};

/// Writes one event. `db` is the pool, or a transaction when the event must
/// commit together with the change it records.
pub async fn record<'e, E>(db: E, actor: &str, action: &str, detail: &str) -> Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query("INSERT INTO audit_events (actor, action, detail) VALUES (?, ?, ?)")
        .bind(actor)
        .bind(action)
        .bind(detail)
        .execute(db)
        .await?;
    Ok(())
}
