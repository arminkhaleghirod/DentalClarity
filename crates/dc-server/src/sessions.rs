//! Server-side sessions stored in SQLite.
//!
//! The browser holds an opaque token in an HttpOnly cookie. The database keeps
//! only its SHA-256 hash, so a leaked database does not yield usable tokens.

use crate::credentials::{new_token, token_hash};
use anyhow::Result;
use sqlx::SqlitePool;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Staff,
    Visitor,
}

impl SessionKind {
    fn as_str(self) -> &'static str {
        match self {
            SessionKind::Staff => "staff",
            SessionKind::Visitor => "visitor",
        }
    }

    /// (idle timeout, absolute lifetime) in seconds.
    fn lifetimes(self) -> (i64, i64) {
        match self {
            SessionKind::Staff => (30 * 60, 12 * 60 * 60),
            // Demo visitor data is purged after 24 hours.
            SessionKind::Visitor => (24 * 60 * 60, 24 * 60 * 60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub token_hash: String,
    pub kind: SessionKind,
    pub csrf_token: String,
    pub locked: bool,
}

/// A newly issued session. `token` goes into the cookie and is never stored.
pub struct Issued {
    pub token: String,
    pub session: Session,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn create(db: &SqlitePool, kind: SessionKind) -> Result<Issued> {
    let token = new_token();
    let session = Session {
        token_hash: token_hash(&token),
        kind,
        csrf_token: new_token(),
        locked: false,
    };
    let now = now();
    let (_, absolute) = kind.lifetimes();
    sqlx::query(
        "INSERT INTO sessions (token_hash, kind, csrf_token, created_at, last_seen_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&session.token_hash)
    .bind(kind.as_str())
    .bind(&session.csrf_token)
    .bind(now)
    .bind(now)
    .bind(now + absolute)
    .execute(db)
    .await?;
    Ok(Issued { token, session })
}

/// Looks up a live session and refreshes its idle timer. Expired sessions
/// are deleted and reported as missing.
pub async fn lookup(db: &SqlitePool, token: &str) -> Result<Option<Session>> {
    let hash = token_hash(token);
    let row: Option<(String, String, i64, i64, i64)> = sqlx::query_as(
        "SELECT kind, csrf_token, locked, last_seen_at, expires_at FROM sessions WHERE token_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await?;
    let Some((kind, csrf_token, locked, last_seen_at, expires_at)) = row else {
        return Ok(None);
    };
    let kind = match kind.as_str() {
        "staff" => SessionKind::Staff,
        _ => SessionKind::Visitor,
    };
    let now = now();
    let (idle, _) = kind.lifetimes();
    if now >= expires_at || now - last_seen_at >= idle {
        delete(db, &hash).await?;
        return Ok(None);
    }
    sqlx::query("UPDATE sessions SET last_seen_at = ? WHERE token_hash = ?")
        .bind(now)
        .bind(&hash)
        .execute(db)
        .await?;
    Ok(Some(Session {
        token_hash: hash,
        kind,
        csrf_token,
        locked: locked != 0,
    }))
}

pub async fn delete(db: &SqlitePool, token_hash: &str) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(db)
        .await?;
    Ok(())
}

/// Ends every session. Used when the server starts in a different mode
/// than last time, so sessions from one mode never carry into the other.
pub async fn delete_all(db: &SqlitePool) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM sessions")
        .execute(db)
        .await?
        .rows_affected())
}

/// Removes expired rows. Called periodically; lookups also expire lazily.
pub async fn purge_expired(db: &SqlitePool) -> Result<u64> {
    let result = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now())
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::open_in_memory;

    #[tokio::test]
    async fn create_lookup_delete() {
        let db = open_in_memory().await.unwrap();
        let issued = create(&db, SessionKind::Staff).await.unwrap();
        let found = lookup(&db, &issued.token).await.unwrap().unwrap();
        assert_eq!(found, issued.session);
        assert!(lookup(&db, "not-a-token").await.unwrap().is_none());
        delete(&db, &found.token_hash).await.unwrap();
        assert!(lookup(&db, &issued.token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn idle_sessions_expire() {
        let db = open_in_memory().await.unwrap();
        let issued = create(&db, SessionKind::Staff).await.unwrap();
        sqlx::query("UPDATE sessions SET last_seen_at = last_seen_at - 31 * 60")
            .execute(&db)
            .await
            .unwrap();
        assert!(lookup(&db, &issued.token).await.unwrap().is_none());
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(count.0, 0);
    }

    #[tokio::test]
    async fn the_database_never_holds_the_raw_token() {
        let db = open_in_memory().await.unwrap();
        let issued = create(&db, SessionKind::Visitor).await.unwrap();
        let stored: (String,) = sqlx::query_as("SELECT token_hash FROM sessions")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_ne!(stored.0, issued.token);
    }
}
