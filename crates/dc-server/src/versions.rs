//! Presentation versions (docs/AI_PIPELINE.md, Sync; DECISIONS E33).
//!
//! The current version lives in `presentations` and `presentation_sections`.
//! Each version also has a row in `presentation_versions`; when a version is
//! superseded, its section rows are frozen into that row, so a present
//! session bound to it, the history and the booklet can still read it.
//! Staff edits and note confirmations are stored per version already.

use anyhow::{Context, Result};
use dc_core::versions::VersionCause;
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqlitePool, Transaction};

/// A stored section: key, state, output, reasons, needs note confirmation,
/// writer model, prompt version. The same shape as
/// `api::presentations::SectionRow`.
pub type SectionRow = (String, String, Option<String>, String, bool, String, String);

/// One section row as frozen in `presentation_versions.sections_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FrozenSection {
    key: String,
    state: String,
    output_json: Option<String>,
    reasons_json: String,
    needs_note_confirmation: bool,
    writer_model: String,
    prompt_version: String,
}

impl From<SectionRow> for FrozenSection {
    fn from(row: SectionRow) -> Self {
        let (
            key,
            state,
            output_json,
            reasons_json,
            needs_note_confirmation,
            writer_model,
            prompt_version,
        ) = row;
        Self {
            key,
            state,
            output_json,
            reasons_json,
            needs_note_confirmation,
            writer_model,
            prompt_version,
        }
    }
}

impl From<FrozenSection> for SectionRow {
    fn from(s: FrozenSection) -> Self {
        (
            s.key,
            s.state,
            s.output_json,
            s.reasons_json,
            s.needs_note_confirmation,
            s.writer_model,
            s.prompt_version,
        )
    }
}

/// Everything the views need about one version of a presentation.
#[derive(Debug, Clone)]
pub struct VersionData {
    pub version: i64,
    /// The newest version, which staff edit and present.
    pub latest: i64,
    pub reading_level: String,
    pub snapshot_json: String,
    pub fingerprint: String,
    pub cause: VersionCause,
    /// Unix seconds.
    pub created_at: i64,
    pub changes: Vec<String>,
    /// Sections in presentation order.
    pub rows: Vec<SectionRow>,
}

impl VersionData {
    pub fn is_current(&self) -> bool {
        self.version == self.latest
    }
}

type VersionRow = (String, i64, String, String, String, Option<String>);

/// Loads `version` of presentation `id`, or the current version when
/// `version` is `None`. Scope checks are the caller's job. `None` when the
/// presentation or that version does not exist.
pub async fn load(db: &SqlitePool, id: &str, version: Option<i64>) -> Result<Option<VersionData>> {
    let current: Option<(i64, String, String, String, i64)> = sqlx::query_as(
        "SELECT version, reading_level, snapshot_json, fingerprint, created_at
         FROM presentations WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some((latest, level, current_snapshot, current_fingerprint, presentation_created)) =
        current
    else {
        return Ok(None);
    };
    let target = version.unwrap_or(latest);
    let row: Option<VersionRow> = sqlx::query_as(
        "SELECT cause, created_at, snapshot_json, fingerprint, changes_json, sections_json
         FROM presentation_versions WHERE presentation_id = ? AND version = ?",
    )
    .bind(id)
    .bind(target)
    .fetch_optional(db)
    .await?;

    if target == latest {
        let rows: Vec<SectionRow> = sqlx::query_as(
            "SELECT key, state, output_json, reasons_json, needs_note_confirmation,
                 writer_model, prompt_version
             FROM presentation_sections WHERE presentation_id = ? ORDER BY position",
        )
        .bind(id)
        .fetch_all(db)
        .await?;
        let (cause, created_at, changes) = row.map_or(
            (VersionCause::Created, presentation_created, Vec::new()),
            |(cause, created_at, _, _, changes, _)| {
                (
                    VersionCause::parse(&cause),
                    created_at,
                    serde_json::from_str(&changes).unwrap_or_default(),
                )
            },
        );
        return Ok(Some(VersionData {
            version: latest,
            latest,
            reading_level: level,
            snapshot_json: current_snapshot,
            fingerprint: current_fingerprint,
            cause,
            created_at,
            changes,
            rows,
        }));
    }

    let Some((cause, created_at, snapshot_json, fingerprint, changes, Some(sections))) = row else {
        return Ok(None);
    };
    let frozen: Vec<FrozenSection> =
        serde_json::from_str(&sections).context("reading frozen sections")?;
    Ok(Some(VersionData {
        version: target,
        latest,
        reading_level: level,
        snapshot_json,
        fingerprint,
        cause: VersionCause::parse(&cause),
        created_at,
        changes: serde_json::from_str(&changes).unwrap_or_default(),
        rows: frozen.into_iter().map(SectionRow::from).collect(),
    }))
}

/// Records version 1 of a new presentation.
pub async fn record_first(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    snapshot_json: &str,
    fingerprint: &str,
    now: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO presentation_versions (presentation_id, version, cause, created_at,
             snapshot_json, fingerprint)
         VALUES (?, 1, 'created', ?, ?, ?)",
    )
    .bind(id)
    .bind(now)
    .bind(snapshot_json)
    .bind(fingerprint)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The version [`begin_next`] makes.
pub struct NextVersion<'a> {
    pub cause: VersionCause,
    pub snapshot_json: &'a str,
    pub fingerprint: &'a str,
    /// Staff-readable changes from the version before.
    pub changes: &'a [String],
    /// Unix seconds.
    pub now: i64,
}

/// Freezes the current version's sections and makes `from + 1` the current
/// version with the new snapshot. The caller then writes the new version's
/// sections and copies the staff changes it keeps. Returns the new number.
pub async fn begin_next(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    from: i64,
    next_version: NextVersion<'_>,
) -> Result<i64> {
    let NextVersion {
        cause,
        snapshot_json,
        fingerprint,
        changes,
        now,
    } = next_version;
    let rows: Vec<SectionRow> = sqlx::query_as(
        "SELECT key, state, output_json, reasons_json, needs_note_confirmation,
             writer_model, prompt_version
         FROM presentation_sections WHERE presentation_id = ? ORDER BY position",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let frozen: Vec<FrozenSection> = rows.into_iter().map(FrozenSection::from).collect();
    let (old_snapshot, old_fingerprint, created_at): (String, String, i64) = sqlx::query_as(
        "SELECT snapshot_json, fingerprint, created_at FROM presentations WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    // A presentation from before version history has no row yet.
    sqlx::query(
        "INSERT INTO presentation_versions (presentation_id, version, cause, created_at,
             snapshot_json, fingerprint)
         VALUES (?, ?, 'created', ?, ?, ?)
         ON CONFLICT (presentation_id, version) DO NOTHING",
    )
    .bind(id)
    .bind(from)
    .bind(created_at)
    .bind(&old_snapshot)
    .bind(&old_fingerprint)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE presentation_versions SET sections_json = ?
         WHERE presentation_id = ? AND version = ?",
    )
    .bind(serde_json::to_string(&frozen)?)
    .bind(id)
    .bind(from)
    .execute(&mut **tx)
    .await?;

    let next = from + 1;
    sqlx::query(
        "INSERT INTO presentation_versions (presentation_id, version, cause, created_at,
             snapshot_json, fingerprint, changes_json)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(next)
    .bind(cause.as_str())
    .bind(now)
    .bind(snapshot_json)
    .bind(fingerprint)
    .bind(serde_json::to_string(changes)?)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE presentations SET version = ?, snapshot_json = ?, fingerprint = ? WHERE id = ?",
    )
    .bind(next)
    .bind(snapshot_json)
    .bind(fingerprint)
    .bind(id)
    .execute(&mut **tx)
    .await?;
    Ok(next)
}

/// Copies staff edits and note confirmations from version `from` to `to`,
/// renaming section keys with `key_for`. A section `key_for` maps to `None`
/// keeps its changes in the old version only.
pub async fn copy_staff_changes(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    from: i64,
    to: i64,
    key_for: impl Fn(&str) -> Option<String>,
) -> Result<()> {
    let edits: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT section_key, field, sentences_json, original_json, updated_at FROM staff_edits
         WHERE presentation_id = ? AND version = ?",
    )
    .bind(id)
    .bind(from)
    .fetch_all(&mut **tx)
    .await?;
    for (key, field, sentences, original, updated_at) in edits {
        let Some(new_key) = key_for(&key) else {
            continue;
        };
        sqlx::query(
            "INSERT INTO staff_edits (presentation_id, version, section_key, field,
                 sentences_json, original_json, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(to)
        .bind(new_key)
        .bind(field)
        .bind(sentences)
        .bind(original)
        .bind(updated_at)
        .execute(&mut **tx)
        .await?;
    }
    let confirmed: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT section_key, field, confirmed_at FROM note_confirmations
         WHERE presentation_id = ? AND version = ?",
    )
    .bind(id)
    .bind(from)
    .fetch_all(&mut **tx)
    .await?;
    for (key, field, confirmed_at) in confirmed {
        let Some(new_key) = key_for(&key) else {
            continue;
        };
        sqlx::query(
            "INSERT INTO note_confirmations (presentation_id, version, section_key, field,
                 confirmed_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(to)
        .bind(new_key)
        .bind(field)
        .bind(confirmed_at)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Before a section of the current version is written again: when that
/// version was presented or exported, it is frozen and the rewrite goes
/// into a new version with the same plan and every staff change carried
/// (PRD FR-VER-01). Returns the version the rewrite lands in.
pub async fn fork_if_frozen(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    change: &str,
) -> Result<i64> {
    let (version, snapshot, fingerprint): (i64, String, String) = sqlx::query_as(
        "SELECT version, snapshot_json, fingerprint FROM presentations WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    let marked: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT presented_at, exported_at FROM presentation_versions
         WHERE presentation_id = ? AND version = ?",
    )
    .bind(id)
    .bind(version)
    .fetch_optional(&mut **tx)
    .await?;
    let frozen =
        marked.is_some_and(|(presented, exported)| presented.is_some() || exported.is_some());
    if !frozen {
        return Ok(version);
    }
    let changes = [change.to_owned()];
    let next = begin_next(
        tx,
        id,
        version,
        NextVersion {
            cause: VersionCause::Regenerated,
            snapshot_json: &snapshot,
            fingerprint: &fingerprint,
            changes: &changes,
            now: crate::sessions::now(),
        },
    )
    .await?;
    copy_staff_changes(tx, id, version, next, |key| Some(key.to_owned())).await?;
    Ok(next)
}

/// Marks a version as presented or exported, keeping the first time.
pub async fn mark<'e, E>(db: E, id: &str, version: i64, marker: Marker) -> Result<()>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let sql = match marker {
        Marker::Presented => {
            "UPDATE presentation_versions SET presented_at = COALESCE(presented_at, ?)
             WHERE presentation_id = ? AND version = ?"
        }
        Marker::Exported => {
            "UPDATE presentation_versions SET exported_at = COALESCE(exported_at, ?)
             WHERE presentation_id = ? AND version = ?"
        }
    };
    sqlx::query(sql)
        .bind(crate::sessions::now())
        .bind(id)
        .bind(version)
        .execute(db)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Presented,
    Exported,
}

/// Year, month (1 to 12) and day of Unix seconds, in UTC.
fn civil_date(secs: i64) -> (i64, u32, u32) {
    // Howard Hinnant's days-to-civil algorithm.
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Unix seconds as RFC 3339 in UTC, e.g. `2026-11-28T08:21:00Z`.
pub fn rfc3339(secs: i64) -> String {
    let (year, month, day) = civil_date(secs);
    let rem = secs.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// Unix seconds as a date for print, e.g. `Nov 28, 2026` (UTC).
pub fn print_date(secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (year, month, day) = civil_date(secs);
    format!("{} {day}, {year}", MONTHS[(month - 1) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_795_852_860), "2026-11-28T08:01:00Z");
        assert_eq!(print_date(1_795_852_860), "Nov 28, 2026");
        assert_eq!(print_date(951_782_400), "Feb 29, 2000");
    }
}
