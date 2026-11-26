//! Resolving held sections: writing one section again, and releasing a held
//! section to patients after staff checked it (docs/AI_PIPELINE.md, Present
//! gating). Both routes run behind `security::require_session`.
//!
//! A release never skips a deterministic finding. The section's stored
//! output is checked again with its citations, and every staff-edited block
//! is checked as staff text; any blocking finding refuses the release.

use super::edits::edit_issues;
use super::present::actor;
use super::presentations::{
    MAX_ID_CHARS, StaffChanges, budget_reached, check_no_running_job, client_key, not_found,
    owner_scope, parse_level, provider_not_configured, section_rows, section_view, stored_output,
};
use crate::http::{ClientIp, Failure};
use crate::security::CurrentSession;
use crate::{AppState, audit, jobs};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use dc_core::Mode;
use dc_core::content::ReadingLevel;
use dc_core::plan::PlanSnapshot;
use dc_core::present::BlockChangeResponse;
use dc_core::presentation::{
    BlockSource, RegenerateSectionResponse, SectionProgress, SectionStage, SectionView,
    SectionVisibility,
};
use dc_generation::facts::FactSheet;
use dc_generation::pipeline::{check_output, has_knowledge, sections_for};
use dc_generation::prompt::SectionKind;
use dc_generation::validate::IssueSeverity;
use serde_json::Value;

/// Single-section regenerations per demo visitor scope per hour, and per
/// client address per day (docs/SECURITY.md, demo abuse controls). They are
/// counted apart from new presentations, because one section costs a
/// fraction of a whole plan.
pub const DEMO_REGENERATIONS_PER_VISITOR_PER_HOUR: usize = 6;
pub const DEMO_REGENERATIONS_PER_ADDRESS_PER_DAY: usize = 20;
const MAX_KEY_CHARS: usize = 64;

fn section_not_found() -> Failure {
    Failure::new(
        StatusCode::NOT_FOUND,
        "section_not_found",
        "That section of the presentation was not found.",
    )
}

fn conflict(code: &'static str, message: impl Into<String>) -> Failure {
    Failure::new(StatusCode::CONFLICT, code, message)
}

/// Fills in what staff can do with a section: write it again, or release
/// it to patients. `section` already has the staff changes applied.
pub(super) fn annotate(
    state: &AppState,
    section: &mut SectionView,
    output: Option<&Value>,
    sheet: &FactSheet,
    level: ReadingLevel,
) {
    let kind = SectionKind::from_key(&section.key);
    section.can_regenerate = kind
        .as_ref()
        .is_some_and(|k| has_knowledge(k, sheet, &state.kb));
    section.release_blockers.clear();
    section.can_release = false;
    if section.visibility != SectionVisibility::NeedsReview {
        return;
    }
    let (Some(kind), Some(output)) = (kind, output) else {
        return;
    };
    section.release_blockers = release_blockers(state, &kind, section, output, sheet, level);
    section.can_release = section.release_blockers.is_empty();
}

/// Blocking findings of the deterministic checks on a section as patients
/// would see it. Generated blocks are checked from the stored output, with
/// the citations the section's prompt allowed. A staff-edited block is
/// checked as staff text instead (no citations), and findings on its
/// generated text no longer apply.
fn release_blockers(
    state: &AppState,
    kind: &SectionKind,
    section: &SectionView,
    output: &Value,
    sheet: &FactSheet,
    level: ReadingLevel,
) -> Vec<String> {
    let under = |path: &str, field: &str| {
        let prefix = format!("/{field}");
        path == prefix || path.starts_with(&format!("{prefix}/"))
    };
    let heading_for = |path: &str| {
        section
            .blocks
            .iter()
            .find(|b| under(path, &b.field))
            .map_or_else(|| path.to_owned(), |b| b.heading.clone())
    };
    let edited: Vec<_> = section
        .blocks
        .iter()
        .filter(|b| b.source == BlockSource::StaffEdit)
        .collect();
    let mut blockers: Vec<String> = check_output(kind, output, sheet, &state.kb, level)
        .into_iter()
        .filter(|i| i.severity == IssueSeverity::Block)
        .filter(|i| !edited.iter().any(|b| under(&i.path, &b.field)))
        .map(|i| format!("{}: {} ({})", heading_for(&i.path), i.detail, i.code))
        .collect();
    for block in edited {
        blockers.extend(
            edit_issues(&block.sentences, sheet, level)
                .into_iter()
                .filter(|(_, i)| i.severity == IssueSeverity::Block)
                .map(|(n, i)| {
                    format!(
                        "{}, sentence {}: {} ({})",
                        block.heading,
                        n + 1,
                        i.detail,
                        i.code
                    )
                }),
        );
    }
    blockers
}

/// A presentation in the caller's scope and one of its stored sections.
struct Loaded {
    version: i64,
    sheet: FactSheet,
    level: ReadingLevel,
    /// Stored state: `passed`, `needs_review` or `failed`.
    stored_state: String,
    output: Option<Value>,
    section: SectionView,
}

async fn load(
    state: &AppState,
    session: &crate::sessions::Session,
    id: &str,
    key: &str,
) -> Result<Loaded, Failure> {
    if id.len() > MAX_ID_CHARS {
        return Err(not_found("presentation"));
    }
    let row: Option<(i64, String, String)> = sqlx::query_as(
        "SELECT version, reading_level, snapshot_json FROM presentations
         WHERE id = ? AND owner_scope = ?",
    )
    .bind(id)
    .bind(owner_scope(state, session))
    .fetch_optional(&state.db)
    .await?;
    let Some((version, level, snapshot)) = row else {
        return Err(not_found("presentation"));
    };
    if key.len() > MAX_KEY_CHARS {
        return Err(section_not_found());
    }
    let snapshot: PlanSnapshot = serde_json::from_str(&snapshot).map_err(Failure::internal)?;
    let sheet = FactSheet::build(&snapshot, &state.kb);
    let row = section_rows(state, id)
        .await?
        .into_iter()
        .find(|r| r.0 == key)
        .ok_or_else(section_not_found)?;
    let stored_state = row.1.clone();
    let output = stored_output(&row);
    let mut section = section_view(row, &sheet);
    StaffChanges::load(state, id, version)
        .await?
        .apply(&mut section);
    Ok(Loaded {
        version,
        sheet,
        level: parse_level(&level),
        stored_state,
        output,
        section,
    })
}

/// Refuses while a job of this presentation is queued or running, so a
/// release or a second regeneration cannot race the job's final write.
async fn check_idle(state: &AppState, id: &str) -> Result<(), Failure> {
    let (running,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM jobs WHERE presentation_id = ? AND state IN ('queued', 'running')",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if running > 0 {
        return Err(conflict(
            "generation_in_progress",
            "This presentation is still being written. Wait for it to finish.",
        ));
    }
    Ok(())
}

/// `POST /api/staff/presentations/{id}/sections/{section_key}/release`:
/// staff release a held section to patients. Allowed only for a section in
/// `needs_review` with stored output whose current text, staff edits
/// included, has no blocking finding. Writes a `section_released` audit
/// event in the same transaction as the state change.
pub async fn release(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Path((id, key)): Path<(String, String)>,
) -> Result<Json<BlockChangeResponse>, Failure> {
    let mut loaded = load(&state, &session, &id, &key).await?;
    check_idle(&state, &id).await?;
    match loaded.stored_state.as_str() {
        "needs_review" => {}
        "passed" => {
            return Err(conflict(
                "already_ready",
                "This section is already ready for patients.",
            ));
        }
        _ => {
            return Err(conflict(
                "nothing_to_release",
                "This section has no text that passed the checks. Write it again instead.",
            ));
        }
    }
    let Some(kind) = SectionKind::from_key(&key) else {
        return Err(section_not_found());
    };
    let Some(output) = loaded.output.as_ref() else {
        return Err(conflict(
            "nothing_to_release",
            "This section has no text to release. Write it again instead.",
        ));
    };
    let blockers = release_blockers(
        &state,
        &kind,
        &loaded.section,
        output,
        &loaded.sheet,
        loaded.level,
    );
    if !blockers.is_empty() {
        return Err(conflict(
            "release_blocked",
            format!(
                "Fix these before releasing the section: {}",
                blockers.join("; ")
            ),
        ));
    }

    let edited = loaded
        .section
        .blocks
        .iter()
        .filter(|b| b.source == BlockSource::StaffEdit)
        .count();
    let mut tx = state.db.begin().await?;
    let changed = sqlx::query(
        "UPDATE presentation_sections SET state = 'passed'
         WHERE presentation_id = ? AND key = ? AND state = 'needs_review'",
    )
    .bind(&id)
    .bind(&key)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(conflict(
            "already_ready",
            "This section is already ready for patients.",
        ));
    }
    jobs::refresh_latest_job(&mut tx, &id).await?;
    audit::record(
        &mut *tx,
        actor(session.kind),
        "section_released",
        &format!(
            "presentation {id} version {} section {key} with {edited} staff-edited blocks",
            loaded.version
        ),
    )
    .await?;
    tx.commit().await?;

    loaded.section.visibility = SectionVisibility::Ready;
    annotate(
        &state,
        &mut loaded.section,
        loaded.output.as_ref(),
        &loaded.sheet,
        loaded.level,
    );
    Ok(Json(BlockChangeResponse {
        warnings: Vec::new(),
        section: loaded.section,
    }))
}

/// `POST /api/staff/presentations/{id}/sections/{section_key}/regenerate`:
/// writes one section again from the same facts, as a job. Poll
/// `GET /api/staff/jobs/{job_id}`. The same gates as a new presentation
/// apply: the provider, one running job per demo visitor, the demo
/// regeneration limits and the budget's soft stop.
pub async fn regenerate(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    ClientIp(ip): ClientIp,
    Path((id, key)): Path<(String, String)>,
) -> Result<Json<RegenerateSectionResponse>, Failure> {
    let Some(generation) = state.generation.clone() else {
        return Err(provider_not_configured(&state));
    };
    if id.len() > MAX_ID_CHARS {
        return Err(not_found("presentation"));
    }
    let owner = owner_scope(&state, &session);
    let row: Option<(String,)> =
        sqlx::query_as("SELECT snapshot_json FROM presentations WHERE id = ? AND owner_scope = ?")
            .bind(&id)
            .bind(&owner)
            .fetch_optional(&state.db)
            .await?;
    let Some((snapshot,)) = row else {
        return Err(not_found("presentation"));
    };
    if key.len() > MAX_KEY_CHARS {
        return Err(section_not_found());
    }
    let snapshot: PlanSnapshot = serde_json::from_str(&snapshot).map_err(Failure::internal)?;
    let sheet = FactSheet::build(&snapshot, &state.kb);
    let kinds = sections_for(&sheet);
    let Some(kind) = kinds.iter().find(|k| k.key() == key) else {
        return Err(section_not_found());
    };
    if !has_knowledge(kind, &sheet, &state.kb) {
        return Err(conflict(
            "no_knowledge_entry",
            "No reviewed educational content exists for this procedure yet, so it cannot be written. It stays held.",
        ));
    }
    let client_key = client_key(&state, ip);

    let _guard = state.jobs.create_lock.lock().await;
    check_idle(&state, &id).await?;
    if state.mode == Mode::Demo {
        check_no_running_job(&state, &owner).await?;
        check_regeneration_limits(&state, &owner, &client_key).await?;
    }
    if !generation.budget.allows_new_generation().await? {
        return Err(budget_reached());
    }

    // Every section stays listed, with the stages of the others as they are.
    let stored: Vec<(String, String)> =
        sqlx::query_as("SELECT key, state FROM presentation_sections WHERE presentation_id = ?")
            .bind(&id)
            .fetch_all(&state.db)
            .await?;
    let progress: Vec<SectionProgress> = kinds
        .iter()
        .map(|k| {
            let k_key = k.key();
            let stage = if k_key == key {
                SectionStage::Queued
            } else {
                stored
                    .iter()
                    .find(|(s, _)| *s == k_key)
                    .map_or(SectionStage::Failed, |(_, st)| jobs::stage_for_state(st))
            };
            SectionProgress {
                label: jobs::section_label(k, &sheet),
                key: k_key,
                stage,
            }
        })
        .collect();
    let job_id = jobs::new_id();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO jobs (id, presentation_id, state, created_at, progress_json, section_key,
             client_key)
         VALUES (?, ?, 'queued', ?, ?, ?, ?)",
    )
    .bind(&job_id)
    .bind(&id)
    .bind(jobs::now_ms())
    .bind(serde_json::to_string(&progress).map_err(Failure::internal)?)
    .bind(&key)
    .bind(&client_key)
    .execute(&mut *tx)
    .await?;
    if state.mode == Mode::Demo {
        sqlx::query(
            "INSERT INTO regeneration_events (owner_scope, client_key, created_at) VALUES (?, ?, ?)",
        )
        .bind(&owner)
        .bind(&client_key)
        .bind(jobs::now_ms())
        .execute(&mut *tx)
        .await?;
    }
    audit::record(
        &mut *tx,
        actor(session.kind),
        "section_regeneration_started",
        &format!("presentation {id} section {key} job {job_id}"),
    )
    .await?;
    tx.commit().await?;
    drop(_guard);

    jobs::spawn(&state, job_id.clone());
    Ok(Json(RegenerateSectionResponse {
        presentation_id: id,
        job_id,
        section_key: key,
    }))
}

/// Demo limits on single-section regenerations, counted from
/// `regeneration_events` so they survive a restart and the purge of the
/// presentation they were made on.
async fn check_regeneration_limits(
    state: &AppState,
    owner: &str,
    client_key: &str,
) -> Result<(), Failure> {
    let now = jobs::now_ms();
    let hour_ms = 60 * 60 * 1000;
    let day_ms = jobs::DEMO_RETENTION_SECS * 1000;
    let limited = |oldest: i64, window: i64, message: String| Failure {
        retry_after: Some(
            u64::try_from((oldest + window - now) / 1000)
                .unwrap_or(1)
                .max(1),
        ),
        ..Failure::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message)
    };
    let by_visitor: Vec<(i64,)> = sqlx::query_as(
        "SELECT created_at FROM regeneration_events
         WHERE owner_scope = ? AND created_at > ?
         ORDER BY created_at",
    )
    .bind(owner)
    .bind(now - hour_ms)
    .fetch_all(&state.db)
    .await?;
    if by_visitor.len() >= DEMO_REGENERATIONS_PER_VISITOR_PER_HOUR {
        return Err(limited(
            by_visitor[0].0,
            hour_ms,
            format!(
                "The demo allows {DEMO_REGENERATIONS_PER_VISITOR_PER_HOUR} section rewrites per hour. Try again later."
            ),
        ));
    }
    let by_address: Vec<(i64,)> = sqlx::query_as(
        "SELECT created_at FROM regeneration_events
         WHERE client_key = ? AND created_at > ?
         ORDER BY created_at",
    )
    .bind(client_key)
    .bind(now - day_ms)
    .fetch_all(&state.db)
    .await?;
    if by_address.len() >= DEMO_REGENERATIONS_PER_ADDRESS_PER_DAY {
        return Err(limited(
            by_address[0].0,
            day_ms,
            format!(
                "The demo allows {DEMO_REGENERATIONS_PER_ADDRESS_PER_DAY} section rewrites per day from one network. Try again tomorrow."
            ),
        ));
    }
    Ok(())
}
