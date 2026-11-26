//! Generation jobs: a Tokio task per job, at most [`MAX_CONCURRENT_JOBS`]
//! running at once, with state and live progress persisted in `jobs`.
//!
//! A job writes either the whole presentation or, when `jobs.section_key` is
//! set, one section again with the same facts. Each section result is saved
//! to `job_section_results` as soon as it finishes. A job left `queued` or
//! `running` when the server stopped runs again at startup and skips the
//! sections it already finished, so their model calls are not paid twice.
//! A section that was in flight starts over. When every section is done, the
//! results replace the presentation's rows in one transaction, so readers
//! never see a mix of old and new sections from one job.

use crate::AppState;
use anyhow::{Context, Result, anyhow};
use dc_core::PlanSnapshot;
use dc_core::ToothRef;
use dc_core::content::ReadingLevel;
use dc_core::presentation::{JobState, SectionProgress, SectionStage};
use dc_generation::facts::FactSheet;
use dc_generation::pipeline::{PARALLEL_SECTIONS, section, sections_for};
use dc_generation::prompt::SectionKind;
use dc_generation::{Event, Progress, SectionResult, SectionState};
use dc_knowledge::KnowledgeBase;
use futures::StreamExt;
use sqlx::SqlitePool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, Semaphore};

/// Generation jobs running at the same time across the server (docs/SECURITY.md).
pub const MAX_CONCURRENT_JOBS: usize = 2;
/// Shortest gap between two progress writes for one job.
const PROGRESS_WRITE_INTERVAL: Duration = Duration::from_millis(250);
/// Demo visitor data is purged this long after it was created.
pub const DEMO_RETENTION_SECS: i64 = 24 * 60 * 60;

/// Shared job runner state.
pub struct Jobs {
    slots: Arc<Semaphore>,
    /// Serializes the limit checks and the insert in `create`, so parallel
    /// requests from one visitor cannot pass the limits together.
    pub create_lock: tokio::sync::Mutex<()>,
}

impl Default for Jobs {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT_JOBS)),
            create_lock: tokio::sync::Mutex::new(()),
        }
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Random identifier for presentations and jobs: 128 bits as hex.
pub fn new_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("OS random source is available");
    hex::encode(bytes)
}

pub fn job_state_str(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Succeeded => "succeeded",
        JobState::NeedsReview => "needs_review",
        JobState::Failed => "failed",
    }
}

pub fn parse_job_state(s: &str) -> JobState {
    match s {
        "queued" => JobState::Queued,
        "running" => JobState::Running,
        "succeeded" => JobState::Succeeded,
        "needs_review" => JobState::NeedsReview,
        _ => JobState::Failed,
    }
}

/// The progress stage shown for a stored section state.
pub fn stage_for_state(state: &str) -> SectionStage {
    match state {
        "passed" => SectionStage::Passed,
        "needs_review" => SectionStage::NeedsReview,
        _ => SectionStage::Failed,
    }
}

pub fn section_state_str(state: SectionState) -> &'static str {
    match state {
        SectionState::Passed => "passed",
        SectionState::NeedsReview => "needs_review",
        SectionState::Failed => "failed",
    }
}

/// Staff-facing title of a section, e.g. "Root canal · #30".
pub fn section_label(kind: &SectionKind, sheet: &FactSheet) -> String {
    match kind {
        SectionKind::AtAGlance => "At a glance".into(),
        SectionKind::Journey => "Treatment journey".into(),
        SectionKind::NextSteps => "What happens next".into(),
        SectionKind::Procedure(reference) => match sheet.procedure(reference) {
            Some(p) => match p.tooth {
                ToothRef::None => p.name.clone(),
                ToothRef::Permanent(_) | ToothRef::Primary(_) => {
                    format!("{} · #{}", p.name, p.tooth)
                }
                other => format!("{} · {other}", p.name),
            },
            None => reference.clone(),
        },
    }
}

/// Every section of a plan, queued, in presentation order.
pub fn initial_progress(snapshot: &PlanSnapshot, kb: &KnowledgeBase) -> Vec<SectionProgress> {
    let sheet = FactSheet::build(snapshot, kb);
    sections_for(&sheet)
        .iter()
        .map(|kind| SectionProgress {
            key: kind.key(),
            label: section_label(kind, &sheet),
            stage: SectionStage::Queued,
        })
        .collect()
}

/// Collects pipeline events and wakes the writer task that persists them.
struct Tracker {
    sections: Mutex<Vec<SectionProgress>>,
    changed: Notify,
    done: AtomicBool,
}

impl Tracker {
    fn snapshot_json(&self) -> String {
        let sections = self.sections.lock().expect("progress lock");
        serde_json::to_string(&*sections).unwrap_or_else(|_| "[]".into())
    }
}

impl Progress for Tracker {
    fn report(&self, event: Event) {
        let (key, stage) = match event {
            Event::Started { .. } => return,
            Event::SectionStage { key, stage, .. } => (
                key,
                match stage {
                    "reviewing" => SectionStage::Reviewing,
                    "repairing" => SectionStage::Repairing,
                    _ => SectionStage::Writing,
                },
            ),
            Event::SectionDone { key, state } => (
                key,
                match state {
                    SectionState::Passed => SectionStage::Passed,
                    SectionState::NeedsReview => SectionStage::NeedsReview,
                    SectionState::Failed => SectionStage::Failed,
                },
            ),
        };
        let mut sections = self.sections.lock().expect("progress lock");
        if let Some(s) = sections.iter_mut().find(|s| s.key == key) {
            s.stage = stage;
        }
        drop(sections);
        self.changed.notify_one();
    }
}

/// Starts a job in the background. It waits for a free slot first.
pub fn spawn(state: &AppState, job_id: String) {
    let state = state.clone();
    tokio::spawn(async move {
        let permit = state.jobs.slots.clone().acquire_owned().await;
        let inner_state = state.clone();
        let inner_id = job_id.clone();
        // A panic inside the pipeline must not leave the job `running` forever.
        let outcome = tokio::spawn(async move { run(&inner_state, &inner_id).await }).await;
        let error = match outcome {
            Ok(Ok(())) => None,
            Ok(Err(err)) => Some(format!("{err:#}")),
            Err(join) => Some(format!("job task stopped: {join}")),
        };
        if let Some(err) = error {
            tracing::error!(job = %job_id, %err, "generation job failed");
            if let Err(err) = fail(
                &state.db,
                &job_id,
                "The presentation could not be generated because of a server error.",
            )
            .await
            {
                tracing::error!(job = %job_id, %err, "could not mark job failed");
            }
        }
        drop(permit);
    });
}

/// Runs again the jobs that were queued or running when the server stopped.
/// Sections they already finished are kept and skipped (see [`run`]).
pub async fn resume_pending(state: &AppState) -> Result<usize> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM jobs WHERE state IN ('queued', 'running') ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await?;
    for (job_id,) in &rows {
        sqlx::query(
            "UPDATE jobs SET state = 'queued', started_at = NULL, error = NULL WHERE id = ?",
        )
        .bind(job_id)
        .execute(&state.db)
        .await?;
        spawn(state, job_id.clone());
    }
    Ok(rows.len())
}

async fn fail(db: &SqlitePool, job_id: &str, message: &str) -> Result<()> {
    let now = now_ms();
    sqlx::query(
        "UPDATE jobs SET state = 'failed', error = ?, finished_at = ?,
             elapsed_ms = CASE WHEN started_at IS NULL THEN 0 ELSE ? - started_at END
         WHERE id = ?",
    )
    .bind(message)
    .bind(now)
    .bind(now)
    .bind(job_id)
    .execute(db)
    .await?;
    sqlx::query("DELETE FROM job_section_results WHERE job_id = ?")
        .bind(job_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Saves one finished section of a job, so a restart does not write it again.
async fn save_result(
    db: &SqlitePool,
    job_id: &str,
    position: usize,
    s: &SectionResult,
) -> Result<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO job_section_results (job_id, key, position, state, output_json,
             reasons_json, needs_note_confirmation, writer_model, prompt_version, attempts,
             attempt_log_json, input_tokens, output_tokens, cost_usd, elapsed_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(job_id)
    .bind(&s.key)
    .bind(position as i64)
    .bind(section_state_str(s.state))
    .bind(s.output.as_ref().map(|v| v.to_string()))
    .bind(serde_json::to_string(&s.reasons)?)
    .bind(s.needs_note_confirmation)
    .bind(&s.writer_model)
    .bind(&s.prompt_version)
    .bind(i64::from(s.attempts))
    .bind(serde_json::to_string(&s.attempt_log)?)
    .bind(i64::from(s.usage.input_tokens))
    .bind(i64::from(s.usage.output_tokens))
    .bind(crate::generation::usage_cost(&s.usage))
    .bind(s.elapsed_ms as i64)
    .execute(db)
    .await?;
    Ok(())
}

async fn run(state: &AppState, job_id: &str) -> Result<()> {
    let row: Option<(String, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT p.id, p.snapshot_json, p.reading_level, j.progress_json, j.section_key
         FROM jobs j JOIN presentations p ON p.id = j.presentation_id WHERE j.id = ?",
    )
    .bind(job_id)
    .fetch_optional(&state.db)
    .await?;
    // The presentation was purged while the job waited.
    let Some((presentation_id, snapshot_json, level, progress_json, section_key)) = row else {
        return Ok(());
    };

    let started = now_ms();
    sqlx::query("UPDATE jobs SET state = 'running', started_at = ?, error = NULL WHERE id = ?")
        .bind(started)
        .bind(job_id)
        .execute(&state.db)
        .await?;

    let Some(generation) = state.generation.clone() else {
        fail(
            &state.db,
            job_id,
            "No AI provider is configured on this server.",
        )
        .await?;
        return Ok(());
    };
    let snapshot: PlanSnapshot =
        serde_json::from_str(&snapshot_json).context("reading the stored snapshot")?;
    let level: ReadingLevel = serde_json::from_value(serde_json::Value::String(level))
        .map_err(|e| anyhow!("stored reading level: {e}"))?;
    let sheet = FactSheet::build(&snapshot, &state.kb);

    // The sections this job writes, with their position in the presentation.
    let targets: Vec<(usize, SectionKind)> = sections_for(&sheet)
        .into_iter()
        .enumerate()
        .filter(|(_, kind)| section_key.as_deref().is_none_or(|key| kind.key() == key))
        .collect();
    if targets.is_empty() {
        fail(
            &state.db,
            job_id,
            "That section is no longer part of this plan.",
        )
        .await?;
        return Ok(());
    }
    let finished: Vec<(String, String)> =
        sqlx::query_as("SELECT key, state FROM job_section_results WHERE job_id = ?")
            .bind(job_id)
            .fetch_all(&state.db)
            .await?;

    let mut sections: Vec<SectionProgress> = serde_json::from_str(&progress_json)
        .ok()
        .filter(|s: &Vec<SectionProgress>| !s.is_empty())
        .unwrap_or_else(|| initial_progress(&snapshot, &state.kb));
    for s in &mut sections {
        if let Some((_, stored)) = finished.iter().find(|(key, _)| *key == s.key) {
            s.stage = stage_for_state(stored);
        } else if targets.iter().any(|(_, kind)| kind.key() == s.key) {
            s.stage = SectionStage::Queued;
        }
    }
    let remaining: Vec<(usize, SectionKind)> = targets
        .into_iter()
        .filter(|(_, kind)| !finished.iter().any(|(key, _)| *key == kind.key()))
        .collect();
    if !finished.is_empty() {
        tracing::info!(
            job = %job_id,
            skipped = finished.len(),
            remaining = remaining.len(),
            "resuming generation job"
        );
    }

    let tracker = Arc::new(Tracker {
        sections: Mutex::new(sections),
        changed: Notify::new(),
        done: AtomicBool::new(false),
    });
    let writer = {
        let tracker = tracker.clone();
        let db = state.db.clone();
        let job_id = job_id.to_owned();
        tokio::spawn(async move {
            loop {
                tracker.changed.notified().await;
                if tracker.done.load(Ordering::Acquire) {
                    break;
                }
                if let Err(err) = sqlx::query("UPDATE jobs SET progress_json = ? WHERE id = ?")
                    .bind(tracker.snapshot_json())
                    .bind(&job_id)
                    .execute(&db)
                    .await
                {
                    tracing::warn!(job = %job_id, %err, "could not save job progress");
                }
                tokio::time::sleep(PROGRESS_WRITE_INTERVAL).await;
            }
        })
    };

    // Sections run in parallel, as in `dc_generation::generate`, and each is
    // saved the moment it finishes.
    let saved: Vec<Result<()>> = futures::stream::iter(remaining)
        .map(|(position, kind)| {
            let sheet = &sheet;
            let models = &generation.models;
            let progress: &dyn Progress = tracker.as_ref();
            async move {
                let result = section(&kind, sheet, &state.kb, models, level, progress).await;
                save_result(&state.db, job_id, position, &result).await
            }
        })
        .buffer_unordered(PARALLEL_SECTIONS)
        .collect()
        .await;

    tracker.done.store(true, Ordering::Release);
    tracker.changed.notify_one();
    let _ = writer.await;
    for result in saved {
        result.context("saving a section result")?;
    }

    let finished_at = now_ms();
    let mut tx = state.db.begin().await?;
    match &section_key {
        None => {
            sqlx::query("DELETE FROM presentation_sections WHERE presentation_id = ?")
                .bind(&presentation_id)
                .execute(&mut *tx)
                .await?;
        }
        Some(key) => {
            sqlx::query("DELETE FROM presentation_sections WHERE presentation_id = ? AND key = ?")
                .bind(&presentation_id)
                .bind(key)
                .execute(&mut *tx)
                .await?;
            // A confirmation covers the note-derived text staff read, not
            // whatever the section says now. Staff edits stay: the facts are
            // the same, so they carry forward (AI_PIPELINE, staff-edit conflicts).
            sqlx::query(
                "DELETE FROM note_confirmations WHERE presentation_id = ? AND section_key = ?",
            )
            .bind(&presentation_id)
            .bind(key)
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query(
        "INSERT INTO presentation_sections (presentation_id, key, position, state, output_json,
             reasons_json, needs_note_confirmation, writer_model, prompt_version, attempts,
             attempt_log_json, input_tokens, output_tokens, cost_usd, elapsed_ms)
         SELECT ?, key, position, state, output_json, reasons_json, needs_note_confirmation,
             writer_model, prompt_version, attempts, attempt_log_json, input_tokens,
             output_tokens, cost_usd, elapsed_ms
         FROM job_section_results WHERE job_id = ?",
    )
    .bind(&presentation_id)
    .bind(job_id)
    .execute(&mut *tx)
    .await?;
    let (job_cost,): (f64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(cost_usd), 0) FROM job_section_results WHERE job_id = ?",
    )
    .bind(job_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM job_section_results WHERE job_id = ?")
        .bind(job_id)
        .execute(&mut *tx)
        .await?;

    // The job's state describes the whole presentation, also after a
    // single-section job, because lists and the editor read it that way.
    let (total, passed): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(state = 'passed'), 0) FROM presentation_sections
         WHERE presentation_id = ?",
    )
    .bind(&presentation_id)
    .fetch_one(&mut *tx)
    .await?;
    let (final_state, error) = if passed == total {
        (JobState::Succeeded, None)
    } else if passed > 0 {
        (JobState::NeedsReview, None)
    } else {
        (
            JobState::Failed,
            Some("No section passed the checks. Try again later, or review the plan details."),
        )
    };
    sqlx::query(
        "UPDATE jobs SET state = ?, error = ?, finished_at = ?, elapsed_ms = ?, progress_json = ?
         WHERE id = ?",
    )
    .bind(job_state_str(final_state))
    .bind(error)
    .bind(finished_at)
    .bind(finished_at - started)
    .bind(tracker.snapshot_json())
    .bind(job_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(
        job = %job_id,
        state = job_state_str(final_state),
        elapsed_ms = finished_at - started,
        cost_usd = job_cost,
        "generation job finished"
    );
    Ok(())
}

/// Brings the newest job of a presentation in line with its stored sections
/// after staff changed a section's state (a release): each section's stage
/// in `progress_json`, and the job state, which lists and the editor read as
/// the presentation's state. A job still running is left alone.
pub async fn refresh_latest_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    presentation_id: &str,
) -> Result<()> {
    let job: Option<(String, String, String)> = sqlx::query_as(
        "SELECT id, state, progress_json FROM jobs WHERE presentation_id = ?
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(presentation_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((job_id, job_state, progress_json)) = job else {
        return Ok(());
    };
    if !parse_job_state(&job_state).is_finished() {
        return Ok(());
    }
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, state FROM presentation_sections WHERE presentation_id = ?")
            .bind(presentation_id)
            .fetch_all(&mut **tx)
            .await?;
    let mut progress: Vec<SectionProgress> =
        serde_json::from_str(&progress_json).unwrap_or_default();
    for s in &mut progress {
        if let Some((_, stored)) = rows.iter().find(|(key, _)| *key == s.key) {
            s.stage = stage_for_state(stored);
        }
    }
    let passed = rows.iter().filter(|(_, state)| state == "passed").count();
    let state = if passed == rows.len() {
        JobState::Succeeded
    } else if passed > 0 {
        JobState::NeedsReview
    } else {
        JobState::Failed
    };
    sqlx::query("UPDATE jobs SET state = ?, progress_json = ? WHERE id = ?")
        .bind(job_state_str(state))
        .bind(serde_json::to_string(&progress)?)
        .bind(&job_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Deletes demo presentations (with their sections and jobs) older than the
/// retention period. Spend in `ai_usage` is kept.
pub async fn purge_demo_data(db: &SqlitePool) -> Result<u64> {
    let cutoff = crate::sessions::now() - DEMO_RETENTION_SECS;
    let mut tx = db.begin().await?;
    for statement in [
        "DELETE FROM present_sessions WHERE presentation_id IN
             (SELECT id FROM presentations WHERE owner_scope <> 'staff' AND created_at <= ?)",
        "DELETE FROM staff_edits WHERE presentation_id IN
             (SELECT id FROM presentations WHERE owner_scope <> 'staff' AND created_at <= ?)",
        "DELETE FROM note_confirmations WHERE presentation_id IN
             (SELECT id FROM presentations WHERE owner_scope <> 'staff' AND created_at <= ?)",
        "DELETE FROM job_section_results WHERE job_id IN
             (SELECT j.id FROM jobs j JOIN presentations p ON p.id = j.presentation_id
              WHERE p.owner_scope <> 'staff' AND p.created_at <= ?)",
    ] {
        sqlx::query(statement)
            .bind(cutoff)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "DELETE FROM presentation_sections WHERE presentation_id IN
             (SELECT id FROM presentations WHERE owner_scope <> 'staff' AND created_at <= ?)",
    )
    .bind(cutoff)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM jobs WHERE presentation_id IN
             (SELECT id FROM presentations WHERE owner_scope <> 'staff' AND created_at <= ?)",
    )
    .bind(cutoff)
    .execute(&mut *tx)
    .await?;
    let removed =
        sqlx::query("DELETE FROM presentations WHERE owner_scope <> 'staff' AND created_at <= ?")
            .bind(cutoff)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    tx.commit().await?;
    Ok(removed)
}
