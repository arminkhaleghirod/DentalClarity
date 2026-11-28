//! Storage limits and retention for "Ask a question" conversations. The
//! routes are in `api::qa`.

use anyhow::Result;
use sqlx::SqlitePool;

/// Questions per hour for one demo visitor (docs/SECURITY.md).
pub const DEMO_SCOPE_PER_HOUR: u32 = 20;
/// Questions per day from one client address (IPv6 by /64) in demo mode.
pub const DEMO_ADDRESS_PER_DAY: u32 = 60;
/// Conversations are hard-deleted this long after the question. Demo rows go
/// sooner, with their presentation after 24 hours.
pub const RETENTION_SECS: i64 = 30 * 24 * 60 * 60;

/// Deletes conversations past the retention period. Returns the rows removed.
pub async fn purge_expired(db: &SqlitePool) -> Result<u64> {
    let cutoff = crate::sessions::now() - RETENTION_SECS;
    Ok(
        sqlx::query("DELETE FROM qa_conversations WHERE asked_at <= ?")
            .bind(cutoff)
            .execute(db)
            .await?
            .rows_affected(),
    )
}
