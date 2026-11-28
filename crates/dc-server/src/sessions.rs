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
    /// The stored `kind`, which is also the audit actor for the session.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Staff => "staff",
            SessionKind::Visitor => "visitor",
        }
    }

    /// Reads a stored `kind`. Anything but `staff` is a visitor.
    fn from_db(kind: &str) -> Self {
        if kind == "staff" {
            SessionKind::Staff
        } else {
            SessionKind::Visitor
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
    let kind = SessionKind::from_db(&kind);
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
    // A present session never outlives the session that started it.
    sqlx::query("DELETE FROM present_sessions WHERE staff_token_hash = ?")
        .bind(token_hash)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(db)
        .await?;
    Ok(())
}

/// Ends every session. Used when the server starts in a different mode
/// than last time, so sessions from one mode never carry into the other.
pub async fn delete_all(db: &SqlitePool) -> Result<u64> {
    sqlx::query("DELETE FROM present_sessions")
        .execute(db)
        .await?;
    Ok(sqlx::query("DELETE FROM sessions")
        .execute(db)
        .await?
        .rows_affected())
}

/// Removes expired rows. Called periodically; lookups also expire lazily.
pub async fn purge_expired(db: &SqlitePool) -> Result<u64> {
    sqlx::query(
        "DELETE FROM present_sessions WHERE expires_at <= ?1 OR staff_token_hash IN
             (SELECT token_hash FROM sessions WHERE expires_at <= ?1)",
    )
    .bind(now())
    .execute(db)
    .await?;
    let result = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now())
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}

// Present sessions -------------------------------------------------------

/// Present session idle timeout and absolute lifetime, in seconds.
pub const PRESENT_IDLE_SECS: i64 = 60 * 60;
pub const PRESENT_ABSOLUTE_SECS: i64 = 4 * 60 * 60;
/// PIN tries allowed per lock. After these, only the staff password unlocks.
pub const MAX_PIN_ATTEMPTS: i64 = 5;

/// A live present session, bound to one presentation version and to the
/// locked session that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentSession {
    pub token_hash: String,
    pub presentation_id: String,
    pub version: i64,
    pub staff_token_hash: String,
    pub staff_kind: SessionKind,
    pub pin_attempts: i64,
}

/// Starts a present session for `staff`, locks `staff` and writes the
/// `present_started` audit event in one transaction, so a failed write
/// never leaves a locked session without a present cookie. Any earlier
/// present session of the same staff session ends. Returns the raw token
/// for the cookie.
pub async fn start_present(
    db: &SqlitePool,
    staff: &Session,
    presentation_id: &str,
    version: i64,
    actor: &str,
) -> Result<String> {
    let token = new_token();
    let now = now();
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM present_sessions WHERE staff_token_hash = ?")
        .bind(&staff.token_hash)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO present_sessions (token_hash, presentation_id, version, staff_token_hash,
             created_at, last_seen_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(token_hash(&token))
    .bind(presentation_id)
    .bind(version)
    .bind(&staff.token_hash)
    .bind(now)
    .bind(now)
    .bind(now + PRESENT_ABSOLUTE_SECS)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE sessions SET locked = 1 WHERE token_hash = ?")
        .bind(&staff.token_hash)
        .execute(&mut *tx)
        .await?;
    // Counts this showing for the analytics screen, even if the patient
    // screens never report a view.
    sqlx::query(
        "INSERT INTO present_events (presentation_id, version, session, kind, at)
         VALUES (?, ?, ?, 'open', ?)",
    )
    .bind(presentation_id)
    .bind(version)
    .bind(token_hash(&token))
    .bind(now * 1000)
    .execute(&mut *tx)
    .await?;
    // A presented version is frozen: a later rewrite makes a new version.
    crate::versions::mark(
        &mut *tx,
        presentation_id,
        version,
        crate::versions::Marker::Presented,
    )
    .await?;
    crate::audit::record(
        &mut *tx,
        actor,
        "present_started",
        &format!("presentation {presentation_id} version {version}"),
    )
    .await?;
    tx.commit().await?;
    Ok(token)
}

/// Whether `staff_hash` has a present session that has not expired. Expired
/// ones are deleted on the way, as [`lookup_present`] would.
pub async fn live_present(db: &SqlitePool, staff_hash: &str) -> Result<bool> {
    let now = now();
    sqlx::query(
        "DELETE FROM present_sessions
         WHERE staff_token_hash = ? AND (expires_at <= ? OR ? - last_seen_at >= ?)",
    )
    .bind(staff_hash)
    .bind(now)
    .bind(now)
    .bind(PRESENT_IDLE_SECS)
    .execute(db)
    .await?;
    let (live,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM present_sessions WHERE staff_token_hash = ?")
            .bind(staff_hash)
            .fetch_one(db)
            .await?;
    Ok(live > 0)
}

/// Outcome of [`unlock_stale`].
pub enum Unlock {
    /// The session was replaced by this unlocked one.
    Unlocked(Issued),
    /// A present session is still live; leave it through its exit instead.
    PresentActive,
    /// The locked session no longer exists.
    Gone,
}

/// Unlocks a locked session whose present session has ended, by replacing
/// it with a new token and CSRF token exactly as [`end_present`] does. The
/// caller checks the staff password first in clinical mode. Refuses while a
/// present session is live, re-checked inside the transaction. The
/// `session_unlocked` audit event by `actor` commits with the rotation, so
/// the caller has nothing left to write that could fail after it.
pub async fn unlock_stale(
    db: &SqlitePool,
    staff_hash: &str,
    actor: &str,
    detail: &str,
) -> Result<Unlock> {
    let now = now();
    let mut tx = db.begin().await?;
    let (live,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM present_sessions
         WHERE staff_token_hash = ? AND expires_at > ? AND ? - last_seen_at < ?",
    )
    .bind(staff_hash)
    .bind(now)
    .bind(now)
    .bind(PRESENT_IDLE_SECS)
    .fetch_one(&mut *tx)
    .await?;
    if live > 0 {
        return Ok(Unlock::PresentActive);
    }
    let unlocked = rotate(&mut tx, staff_hash).await?;
    if unlocked.is_some() {
        crate::audit::record(&mut *tx, actor, "session_unlocked", detail).await?;
    }
    tx.commit().await?;
    Ok(match unlocked {
        Some(issued) => Unlock::Unlocked(issued),
        None => Unlock::Gone,
    })
}

type PresentRow = (String, i64, String, i64, i64, i64, String, i64);

/// Looks up a live present session and refreshes its idle timer. The locked
/// session behind it is kept from idling out while the patient is viewing,
/// so staff can still unlock afterwards. Expired present sessions are
/// deleted and reported as missing; the locked session stays locked.
///
/// The staff session's absolute expiry is checked here, but deliberately not
/// its idle timeout. While Present mode runs, the present session's own
/// 60 minute idle window stands in for it: the staff session is locked and
/// only the PIN or password (in clinical mode) opens it, and every present
/// request refreshes its `last_seen_at`. Checking the 30 minute staff idle
/// limit as well would end a patient's screen after 30 quiet minutes, for
/// example while the patient talks with the dentist, and send staff back to
/// a full sign-in. Once the present session ends, the staff session's idle
/// timeout applies again from the last present request.
pub async fn lookup_present(db: &SqlitePool, token: &str) -> Result<Option<PresentSession>> {
    let hash = token_hash(token);
    let row: Option<PresentRow> = sqlx::query_as(
        "SELECT p.presentation_id, p.version, p.staff_token_hash, p.pin_attempts,
             p.last_seen_at, p.expires_at, s.kind, s.expires_at
         FROM present_sessions p JOIN sessions s ON s.token_hash = p.staff_token_hash
         WHERE p.token_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await?;
    let Some((presentation_id, version, staff_hash, attempts, seen, expires, kind, staff_expires)) =
        row
    else {
        sqlx::query("DELETE FROM present_sessions WHERE token_hash = ?")
            .bind(&hash)
            .execute(db)
            .await?;
        return Ok(None);
    };
    let now = now();
    if now >= expires || now - seen >= PRESENT_IDLE_SECS || now >= staff_expires {
        sqlx::query("DELETE FROM present_sessions WHERE token_hash = ?")
            .bind(&hash)
            .execute(db)
            .await?;
        return Ok(None);
    }
    sqlx::query("UPDATE present_sessions SET last_seen_at = ? WHERE token_hash = ?")
        .bind(now)
        .bind(&hash)
        .execute(db)
        .await?;
    sqlx::query("UPDATE sessions SET last_seen_at = ? WHERE token_hash = ?")
        .bind(now)
        .bind(&staff_hash)
        .execute(db)
        .await?;
    Ok(Some(PresentSession {
        token_hash: hash,
        presentation_id,
        version,
        staff_token_hash: staff_hash,
        staff_kind: SessionKind::from_db(&kind),
        pin_attempts: attempts,
    }))
}

/// Counts one PIN try before it is checked, so parallel requests cannot
/// exceed the limit. Returns the tries used including this one, or `None`
/// when the PIN is already disabled for this lock.
pub async fn reserve_pin_attempt(db: &SqlitePool, present_hash: &str) -> Result<Option<i64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        "UPDATE present_sessions SET pin_attempts = pin_attempts + 1
         WHERE token_hash = ? AND pin_attempts < ? RETURNING pin_attempts",
    )
    .bind(present_hash)
    .bind(MAX_PIN_ATTEMPTS)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(n,)| n))
}

/// Ends a present session and unlocks the session behind it by replacing it
/// with a new token and CSRF token. The old token stops working. The new
/// session keeps the old absolute expiry, and demo presentations move to the
/// new visitor scope. The `present_exited` audit event by `actor` commits
/// with the rotation. Returns `None` when the locked session no longer exists.
pub async fn end_present(
    db: &SqlitePool,
    present: &PresentSession,
    actor: &str,
    detail: &str,
) -> Result<Option<Issued>> {
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM present_sessions WHERE token_hash = ?")
        .bind(&present.token_hash)
        .execute(&mut *tx)
        .await?;
    let issued = rotate(&mut tx, &present.staff_token_hash).await?;
    if issued.is_some() {
        crate::audit::record(&mut *tx, actor, "present_exited", detail).await?;
    }
    tx.commit().await?;
    Ok(issued)
}

/// Replaces the session `old_hash` with a new, unlocked session that keeps
/// its kind and absolute expiry, moves demo presentations to the new scope,
/// and ends the old session with its present sessions. `None` when the old
/// session no longer exists.
async fn rotate(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    old_hash: &str,
) -> Result<Option<Issued>> {
    let old: Option<(String, i64)> =
        sqlx::query_as("SELECT kind, expires_at FROM sessions WHERE token_hash = ?")
            .bind(old_hash)
            .fetch_optional(&mut **tx)
            .await?;
    let Some((kind, expires_at)) = old else {
        return Ok(None);
    };
    let kind = SessionKind::from_db(&kind);
    let token = new_token();
    let session = Session {
        token_hash: token_hash(&token),
        kind,
        csrf_token: new_token(),
        locked: false,
    };
    let now = now();
    sqlx::query(
        "INSERT INTO sessions (token_hash, kind, csrf_token, created_at, last_seen_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&session.token_hash)
    .bind(kind.as_str())
    .bind(&session.csrf_token)
    .bind(now)
    .bind(now)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    // Demo presentations are scoped to the visitor's token hash. Clinical
    // rows use the literal scope 'staff', which never matches a hash.
    sqlx::query("UPDATE presentations SET owner_scope = ? WHERE owner_scope = ?")
        .bind(&session.token_hash)
        .bind(old_hash)
        .execute(&mut **tx)
        .await?;
    // The visitor's regeneration count follows the scope.
    sqlx::query("UPDATE regeneration_events SET owner_scope = ? WHERE owner_scope = ?")
        .bind(&session.token_hash)
        .bind(old_hash)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM present_sessions WHERE staff_token_hash = ?")
        .bind(old_hash)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(old_hash)
        .execute(&mut **tx)
        .await?;
    Ok(Some(Issued { token, session }))
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
    async fn rotation_and_its_audit_event_commit_together() {
        let db = open_in_memory().await.unwrap();
        let staff = create(&db, SessionKind::Staff).await.unwrap();
        sqlx::query(
            "INSERT INTO presentations (id, owner_scope, patient_key, patient_display_name,
                 plan_key, language, reading_level, created_at, snapshot_json, fingerprint)
             VALUES ('p1', 'staff', 'k', 'Test Patient', 'plan', 'en', 'standard', 0, '{}', 'f')",
        )
        .execute(&db)
        .await
        .unwrap();
        let present_token = start_present(&db, &staff.session, "p1", 1, "staff")
            .await
            .unwrap();
        let present = lookup_present(&db, &present_token).await.unwrap().unwrap();
        sqlx::query(
            "CREATE TRIGGER no_audit BEFORE INSERT ON audit_events
             BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END",
        )
        .execute(&db)
        .await
        .unwrap();
        // A failed audit write leaves the old token working, so the browser
        // is never left holding a token that no longer exists.
        assert!(end_present(&db, &present, "staff", "d").await.is_err());
        assert!(lookup(&db, &staff.token).await.unwrap().is_some());
        assert!(lookup_present(&db, &present_token).await.unwrap().is_some());
        sqlx::query("DELETE FROM present_sessions")
            .execute(&db)
            .await
            .unwrap();
        assert!(
            unlock_stale(&db, &staff.session.token_hash, "staff", "d")
                .await
                .is_err()
        );
        assert!(lookup(&db, &staff.token).await.unwrap().unwrap().locked);

        sqlx::query("DROP TRIGGER no_audit")
            .execute(&db)
            .await
            .unwrap();
        let Unlock::Unlocked(issued) = unlock_stale(&db, &staff.session.token_hash, "staff", "d")
            .await
            .unwrap()
        else {
            panic!("expected an unlocked session");
        };
        assert!(lookup(&db, &staff.token).await.unwrap().is_none());
        assert!(lookup(&db, &issued.token).await.unwrap().is_some());
        let (events,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM audit_events WHERE action = 'session_unlocked'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(events, 1);
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
