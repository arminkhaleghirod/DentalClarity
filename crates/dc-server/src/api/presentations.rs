//! Presentation endpoints under `/api/staff`: create, list, view, and job
//! status. Every route runs behind `security::require_session`.
//!
//! Scope: a demo visitor sees only presentations created with their own
//! session; clinical staff share one scope. Ids outside the caller's scope
//! answer 404, exactly like unknown ids.

use crate::http::{ClientIp, Failure, JsonBody};
use crate::security::CurrentSession;
use crate::sessions::Session;
use crate::{AppState, jobs};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use dc_core::content::ReadingLevel;
use dc_core::plan::{PatientKey, PlanKey, PlanSnapshot};
use dc_core::presentation::{
    BlockSource, CreatePresentationRequest, CreatePresentationResponse, JobState, JobStatus,
    PresentationSummary, PresentationView, ProcedureFactView, SectionProgress, SectionStage,
    SectionView, SectionVisibility, TextBlock,
};
use dc_core::procedure::ProcedureCategory;
use dc_core::versions::SourceState;
use dc_core::{Mode, ToothRef};
use dc_generation::facts::FactSheet;
use dc_generation::schema::{
    AtAGlanceOut, JourneyOut, NextStepsOut, Para, ProcedureOut, Question, WhyKind,
};
use dc_knowledge::entries::category_id;
use dc_pms::PmsError;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// Demo limits on new generations (docs/SECURITY.md, demo abuse controls).
pub const DEMO_PER_VISITOR_PER_HOUR: i64 = 3;
pub const DEMO_PER_ADDRESS_PER_DAY: i64 = 10;
/// Suggested wait while the visitor's previous job is still running.
const JOB_IN_PROGRESS_RETRY_SECS: u64 = 15;
pub(super) const MAX_ID_CHARS: usize = 64;
const MAX_KEY_CHARS: usize = 64;

pub(super) fn owner_scope(state: &AppState, session: &Session) -> String {
    match state.mode {
        Mode::Demo => session.token_hash.clone(),
        Mode::Clinical => "staff".into(),
    }
}

pub(super) fn not_found(what: &'static str) -> Failure {
    let (code, message) = match what {
        "job" => ("job_not_found", "No generation job with that id was found."),
        _ => (
            "presentation_not_found",
            "No presentation with that id was found.",
        ),
    };
    Failure::new(StatusCode::NOT_FOUND, code, message)
}

/// A short BCP 47 style tag: a 2 or 3 letter language, then up to three
/// subtags of 2 to 8 letters or digits, e.g. `en`, `es-MX`, `zh-Hant`.
pub fn valid_language(tag: &str) -> bool {
    if tag.len() > 20 {
        return false;
    }
    let mut parts = tag.split('-');
    let Some(lang) = parts.next() else {
        return false;
    };
    if !(2..=3).contains(&lang.len()) || !lang.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    let rest: Vec<&str> = parts.collect();
    rest.len() <= 3
        && rest
            .iter()
            .all(|p| (2..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn level_str(level: ReadingLevel) -> String {
    level.to_string()
}

pub(super) fn parse_level(s: &str) -> ReadingLevel {
    match s {
        "simple" => ReadingLevel::Simple,
        "detailed" => ReadingLevel::Detailed,
        _ => ReadingLevel::Standard,
    }
}

/// Seconds until the oldest of `times` (Unix seconds) leaves a window that
/// ends at `now`.
fn retry_after(oldest: i64, window: i64, now: i64) -> u64 {
    u64::try_from(oldest + window - now).unwrap_or(1).max(1)
}

/// `POST /api/staff/presentations`
pub async fn create(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    ClientIp(ip): ClientIp,
    JsonBody(request): JsonBody<CreatePresentationRequest>,
) -> Result<Json<CreatePresentationResponse>, Failure> {
    let Some(generation) = state.generation.clone() else {
        return Err(provider_not_configured(&state));
    };

    let language = request.language.trim();
    if !valid_language(language) {
        return Err(Failure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_language",
            "Choose a language such as en or es-MX.",
        ));
    }
    let patient_key = request.patient_key.trim();
    let plan_key = request.plan_key.trim();
    if patient_key.is_empty()
        || patient_key.len() > MAX_KEY_CHARS
        || plan_key.is_empty()
        || plan_key.len() > MAX_KEY_CHARS
    {
        return Err(patient_not_found());
    }

    // S0: the patient must exist and the plan must be one of theirs.
    let patient_key = PatientKey(patient_key.to_owned());
    let plans = match state.pms.treatment_plans(&patient_key).await {
        Ok(plans) => plans,
        Err(PmsError::PatientNotFound(_)) => return Err(patient_not_found()),
        Err(err) => return Err(source_unavailable(err)),
    };
    if !plans.iter().any(|p| p.key.0 == plan_key) {
        return Err(plan_not_found());
    }
    let snapshot = match state.pms.plan_snapshot(&PlanKey(plan_key.to_owned())).await {
        Ok(snapshot) => snapshot,
        Err(PmsError::PlanNotFound(_)) => return Err(plan_not_found()),
        Err(err) => return Err(source_unavailable(err)),
    };
    let display_name = state
        .pms
        .search_patients(&patient_key.0)
        .await
        .map_err(source_unavailable)?
        .into_iter()
        .find(|p| p.key == patient_key)
        .map(|p| format!("{} {}", p.first_name, p.last_name))
        .unwrap_or_else(|| snapshot.patient.first_name.clone());

    let owner = owner_scope(&state, &session);
    let client_key = client_key(&state, ip);

    let _guard = state.jobs.create_lock.lock().await;
    let now = crate::sessions::now();
    if state.mode == Mode::Demo {
        check_demo_limits(&state, &owner, &client_key, now).await?;
    }
    if !generation.budget.allows_new_generation().await? {
        return Err(budget_reached());
    }

    let presentation_id = jobs::new_id();
    let job_id = jobs::new_id();
    let progress = jobs::initial_progress(&snapshot, &state.kb);
    let snapshot_json = serde_json::to_string(&snapshot).map_err(Failure::internal)?;
    let fingerprint = snapshot.fingerprint();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO presentations (id, owner_scope, client_key, patient_key, patient_display_name,
             plan_key, language, reading_level, version, created_at, snapshot_json, fingerprint)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(&presentation_id)
    .bind(&owner)
    .bind(&client_key)
    .bind(&patient_key.0)
    .bind(&display_name)
    .bind(plan_key)
    .bind(language)
    .bind(level_str(request.reading_level))
    .bind(now)
    .bind(&snapshot_json)
    .bind(&fingerprint)
    .execute(&mut *tx)
    .await?;
    crate::versions::record_first(&mut tx, &presentation_id, &snapshot_json, &fingerprint, now)
        .await
        .map_err(Failure::internal)?;
    sqlx::query(
        "INSERT INTO jobs (id, presentation_id, state, created_at, progress_json, client_key)
         VALUES (?, ?, 'queued', ?, ?, ?)",
    )
    .bind(&job_id)
    .bind(&presentation_id)
    .bind(jobs::now_ms())
    .bind(serde_json::to_string(&progress).map_err(Failure::internal)?)
    .bind(&client_key)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    drop(_guard);

    jobs::spawn(&state, job_id.clone());
    Ok(Json(CreatePresentationResponse {
        presentation_id,
        job_id,
    }))
}

pub(super) fn provider_not_configured(state: &AppState) -> Failure {
    let message = match state.mode {
        Mode::Clinical => {
            "Generating presentations in clinical mode needs the AWS Bedrock connection, \
             which is not available in this version yet."
        }
        Mode::Demo => {
            "The demo AI provider is not configured on this server. Set OPENROUTER_API_KEY and restart."
        }
    };
    Failure::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "provider_not_configured",
        message,
    )
}

pub(super) fn budget_reached() -> Failure {
    Failure::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "budget_reached",
        "Demo AI budget reached for this month. The prepared example cases still work.",
    )
}

/// The address hash used for demo per-address limits; empty in clinical mode.
pub(super) fn client_key(state: &AppState, ip: std::net::IpAddr) -> String {
    match state.mode {
        Mode::Demo => hex::encode(Sha256::digest(crate::limits::ip_key(ip).as_bytes())),
        Mode::Clinical => String::new(),
    }
}

/// One generation job at a time per demo visitor, whole or single-section.
pub(super) async fn check_no_running_job(state: &AppState, owner: &str) -> Result<(), Failure> {
    let (running,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM jobs j JOIN presentations p ON p.id = j.presentation_id
         WHERE p.owner_scope = ? AND j.state IN ('queued', 'running')",
    )
    .bind(owner)
    .fetch_one(&state.db)
    .await?;
    if running > 0 {
        return Err(Failure {
            retry_after: Some(JOB_IN_PROGRESS_RETRY_SECS),
            ..Failure::new(
                StatusCode::TOO_MANY_REQUESTS,
                "generation_in_progress",
                "Your previous presentation is still being written. Wait for it to finish.",
            )
        });
    }
    Ok(())
}

async fn check_demo_limits(
    state: &AppState,
    owner: &str,
    client_key: &str,
    now: i64,
) -> Result<(), Failure> {
    check_no_running_job(state, owner).await?;

    let hour = 60 * 60;
    let recent: Vec<(i64,)> = sqlx::query_as(
        "SELECT created_at FROM presentations WHERE owner_scope = ? AND created_at > ?
         ORDER BY created_at",
    )
    .bind(owner)
    .bind(now - hour)
    .fetch_all(&state.db)
    .await?;
    if recent.len() as i64 >= DEMO_PER_VISITOR_PER_HOUR {
        return Err(Failure {
            retry_after: Some(retry_after(recent[0].0, hour, now)),
            ..Failure::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "The demo allows 3 new presentations per hour. Try again later.",
            )
        });
    }

    let day = jobs::DEMO_RETENTION_SECS;
    let from_address: Vec<(i64,)> = sqlx::query_as(
        "SELECT created_at FROM presentations WHERE client_key = ? AND created_at > ?
         ORDER BY created_at",
    )
    .bind(client_key)
    .bind(now - day)
    .fetch_all(&state.db)
    .await?;
    if from_address.len() as i64 >= DEMO_PER_ADDRESS_PER_DAY {
        return Err(Failure {
            retry_after: Some(retry_after(from_address[0].0, day, now)),
            ..Failure::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "The demo allows 10 new presentations per day from one network. Try again tomorrow.",
            )
        });
    }
    Ok(())
}

fn patient_not_found() -> Failure {
    Failure::new(
        StatusCode::NOT_FOUND,
        "patient_not_found",
        "No patient with that number was found in the practice records.",
    )
}

fn plan_not_found() -> Failure {
    Failure::new(
        StatusCode::NOT_FOUND,
        "plan_not_found",
        "That treatment plan was not found for this patient.",
    )
}

fn source_unavailable(err: PmsError) -> Failure {
    tracing::warn!(%err, "practice records unavailable");
    Failure::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "source_unavailable",
        "The practice records could not be read. Try again in a moment.",
    )
}

type JobRow = (
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    i64,
    String,
    Option<String>,
);

/// The newest job of a presentation, if it is in scope.
async fn job_row(state: &AppState, owner: &str, job_id: &str) -> Result<Option<JobRow>, Failure> {
    Ok(sqlx::query_as(
        "SELECT j.id, j.presentation_id, j.state, j.error, j.started_at, j.elapsed_ms, j.progress_json,
             j.section_key
         FROM jobs j JOIN presentations p ON p.id = j.presentation_id
         WHERE j.id = ? AND p.owner_scope = ?",
    )
    .bind(job_id)
    .bind(owner)
    .fetch_optional(&state.db)
    .await?)
}

fn job_status(row: JobRow) -> JobStatus {
    let (job_id, presentation_id, state, error, started_at, elapsed_ms, progress, section_key) =
        row;
    let state = jobs::parse_job_state(&state);
    let elapsed_ms = match (state, started_at) {
        (JobState::Running, Some(started)) => jobs::now_ms() - started,
        _ => elapsed_ms,
    };
    JobStatus {
        job_id,
        presentation_id,
        state,
        sections: serde_json::from_str(&progress).unwrap_or_default(),
        elapsed_ms: u64::try_from(elapsed_ms).unwrap_or(0),
        error,
        section_key,
    }
}

/// `GET /api/staff/jobs/{id}`
pub async fn job(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Path(id): Path<String>,
) -> Result<Json<JobStatus>, Failure> {
    if id.len() > MAX_ID_CHARS {
        return Err(not_found("job"));
    }
    let owner = owner_scope(&state, &session);
    let row = job_row(&state, &owner, &id)
        .await?
        .ok_or_else(|| not_found("job"))?;
    Ok(Json(job_status(row)))
}

/// id, patient key, display name, snapshot, version, created_at, job state, progress.
type SummaryRow = (String, String, String, String, i64, String, String, String);

/// `GET /api/staff/presentations`: the caller's presentations, newest first.
pub async fn list(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
) -> Result<Json<Vec<PresentationSummary>>, Failure> {
    let owner = owner_scope(&state, &session);
    let rows: Vec<SummaryRow> = sqlx::query_as(
        "SELECT p.id, p.patient_key, p.patient_display_name, p.snapshot_json, p.version,
                 strftime('%Y-%m-%dT%H:%M:%SZ', p.created_at, 'unixepoch'),
                 COALESCE(j.state, 'failed'), COALESCE(j.progress_json, '[]')
             FROM presentations p
             LEFT JOIN jobs j ON j.id = (SELECT id FROM jobs WHERE presentation_id = p.id
                                         ORDER BY created_at DESC LIMIT 1)
             WHERE p.owner_scope = ?
             ORDER BY p.created_at DESC, p.rowid DESC",
    )
    .bind(&owner)
    .fetch_all(&state.db)
    .await?;
    let summaries = rows
        .into_iter()
        .map(
            |(
                id,
                patient_key,
                display_name,
                snapshot,
                version,
                created_at,
                job_state,
                progress,
            )| {
                let progress: Vec<SectionProgress> =
                    serde_json::from_str(&progress).unwrap_or_default();
                let plan_name = serde_json::from_str::<PlanSnapshot>(&snapshot)
                    .map(|s| s.plan_name)
                    .unwrap_or_default();
                PresentationSummary {
                    id,
                    patient_key,
                    patient_display_name: display_name,
                    plan_name,
                    version: u32::try_from(version).unwrap_or(1),
                    created_at,
                    job_state: jobs::parse_job_state(&job_state),
                    sections_ready: progress
                        .iter()
                        .filter(|s| s.stage == SectionStage::Passed)
                        .count() as u32,
                    sections_total: progress.len() as u32,
                }
            },
        )
        .collect();
    Ok(Json(summaries))
}

pub(super) type SectionRow = crate::versions::SectionRow;

/// Generated sections of a presentation in order, as stored.
pub(super) async fn section_rows(state: &AppState, id: &str) -> Result<Vec<SectionRow>, Failure> {
    Ok(sqlx::query_as(
        "SELECT key, state, output_json, reasons_json, needs_note_confirmation,
             writer_model, prompt_version
         FROM presentation_sections WHERE presentation_id = ? ORDER BY position",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?)
}

/// Staff edits and note confirmations of one presentation version, keyed by
/// (section key, block field).
#[derive(Debug, Default)]
pub(super) struct StaffChanges {
    edits: HashMap<(String, String), Vec<String>>,
    confirmed: HashSet<(String, String)>,
}

impl StaffChanges {
    pub(super) async fn load(state: &AppState, id: &str, version: i64) -> Result<Self, Failure> {
        let edits: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT section_key, field, sentences_json FROM staff_edits
             WHERE presentation_id = ? AND version = ?",
        )
        .bind(id)
        .bind(version)
        .fetch_all(&state.db)
        .await?;
        let confirmed: Vec<(String, String)> = sqlx::query_as(
            "SELECT section_key, field FROM note_confirmations
             WHERE presentation_id = ? AND version = ?",
        )
        .bind(id)
        .bind(version)
        .fetch_all(&state.db)
        .await?;
        Ok(Self {
            edits: edits
                .into_iter()
                .filter_map(|(key, field, json)| {
                    serde_json::from_str(&json).ok().map(|s| ((key, field), s))
                })
                .collect(),
            confirmed: confirmed.into_iter().collect(),
        })
    }

    /// Puts staff text in place of edited blocks. A note-derived block that
    /// was not edited needs confirmation until staff confirm it (P11).
    pub(super) fn apply(&self, section: &mut SectionView) {
        for block in &mut section.blocks {
            let key = (section.key.clone(), block.field.clone());
            if let Some(sentences) = self.edits.get(&key) {
                block.sentences = sentences.clone();
                block.source = BlockSource::StaffEdit;
                block.needs_confirmation = false;
            } else if block.source == BlockSource::ClinicalNote {
                block.needs_confirmation = !self.confirmed.contains(&key);
            }
        }
    }
}

/// Sections of a presentation version with staff edits and confirmations applied.
pub(super) async fn load_sections(
    state: &AppState,
    id: &str,
    version: i64,
    rows: Vec<SectionRow>,
    sheet: &FactSheet,
) -> Result<Vec<SectionView>, Failure> {
    let changes = StaffChanges::load(state, id, version).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut section = section_view(row, sheet);
            changes.apply(&mut section);
            section
        })
        .collect())
}

/// Locked fact rows from the snapshot, in plan order.
pub(super) fn procedure_facts(
    snapshot: &PlanSnapshot,
    sheet: &FactSheet,
) -> Vec<ProcedureFactView> {
    snapshot
        .procedures
        .iter()
        .zip(&sheet.procedures)
        .map(|(p, fact)| ProcedureFactView {
            procedure_ref: fact.reference.clone(),
            name: fact.name.clone(),
            code: p.code.to_string(),
            tooth: match p.tooth {
                ToothRef::None => String::new(),
                other => other.to_string(),
            },
            surfaces: p.surfaces.clone(),
            phase: fact.phase_ref.trim_start_matches("ph").parse().unwrap_or(1),
            fee: p.fee.known(),
            insurance_estimate: p.insurance_estimate.known(),
            patient_estimate: p.patient_estimate.known(),
            category: match fact.category {
                ProcedureCategory::Unsupported => None,
                other => Some(category_id(other)).filter(|id| !id.is_empty()),
            },
        })
        .collect()
}

/// `GET /api/staff/presentations/{id}`
pub async fn view(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Path(id): Path<String>,
) -> Result<Json<PresentationView>, Failure> {
    if id.len() > MAX_ID_CHARS {
        return Err(not_found("presentation"));
    }
    let owner = owner_scope(&state, &session);
    Ok(Json(build_view(&state, &owner, &id, None).await?))
}

/// The staff view of one version of a presentation in `owner`'s scope, or
/// of the current version when `version` is `None`. An older version comes
/// back read-only, with no staff actions.
pub(super) async fn build_view(
    state: &AppState,
    owner: &str,
    id: &str,
    version: Option<i64>,
) -> Result<PresentationView, Failure> {
    let row: Option<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT patient_key, patient_display_name, language,
             strftime('%Y-%m-%dT%H:%M:%SZ', created_at, 'unixepoch'), source_state
         FROM presentations WHERE id = ? AND owner_scope = ?",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&state.db)
    .await?;
    let Some((patient_key, display_name, language, created_at, source_state)) = row else {
        return Err(not_found("presentation"));
    };
    let data = crate::versions::load(&state.db, id, version)
        .await?
        .ok_or_else(|| not_found("presentation"))?;
    let snapshot: PlanSnapshot =
        serde_json::from_str(&data.snapshot_json).map_err(Failure::internal)?;
    // The newest job, and the newest whole-presentation job, whose time is
    // the generation time shown to staff.
    let job: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, state, section_key FROM jobs WHERE presentation_id = ?
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (generation_ms,): (i64,) = sqlx::query_as(
        "SELECT COALESCE((SELECT elapsed_ms FROM jobs WHERE presentation_id = ?
             AND section_key IS NULL ORDER BY created_at DESC LIMIT 1), 0)",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    let (job_id, job_state, section_job) = job
        .map(|(id, s, key)| (id, jobs::parse_job_state(&s), key.is_some()))
        .unwrap_or((String::new(), JobState::Failed, false));

    let sheet = FactSheet::build(&snapshot, &state.kb);
    let level = parse_level(&data.reading_level);
    let read_only = !data.is_current();
    // While one section is written again, the others stay readable. An
    // older version is frozen, so a running job does not touch it.
    let rows = if read_only || job_state.is_finished() || section_job {
        data.rows.clone()
    } else {
        Vec::new()
    };
    let writer_model = rows.first().map(|r| r.5.clone()).unwrap_or_default();
    let prompt_version = rows.first().map(|r| r.6.clone()).unwrap_or_default();
    let changes = StaffChanges::load(state, id, data.version).await?;
    let sections = rows
        .into_iter()
        .map(|r| {
            let output = stored_output(&r);
            let mut section = section_view(r, &sheet);
            changes.apply(&mut section);
            super::review::annotate(state, &mut section, output.as_ref(), &sheet, level);
            if read_only {
                section.can_regenerate = false;
                section.can_release = false;
            }
            section
        })
        .collect();
    let procedures = procedure_facts(&snapshot, &sheet);

    Ok(PresentationView {
        id: id.to_owned(),
        version: u32::try_from(data.version).unwrap_or(1),
        patient_key,
        patient_first_name: snapshot.patient.first_name.clone(),
        patient_display_name: display_name,
        plan_name: snapshot.plan_name.clone(),
        provider_display_name: snapshot.provider_display_name.clone(),
        language,
        reading_level: level,
        created_at,
        job_id,
        job_state,
        total_fee: snapshot.total_fee(),
        total_patient_estimate: snapshot.total_patient_estimate(),
        procedures,
        sections,
        writer_model,
        prompt_version,
        generation_ms: u64::try_from(generation_ms).unwrap_or(0),
        latest_version: u32::try_from(data.latest).unwrap_or(1),
        read_only,
        version_cause: data.cause,
        version_created_at: crate::versions::rfc3339(data.created_at),
        changes: data.changes.clone(),
        source_state: match source_state.as_str() {
            "plan_deleted" => SourceState::PlanDeleted,
            "source_unavailable" => SourceState::SourceUnavailable,
            _ => SourceState::Ok,
        },
    })
}

/// The stored model output of a section row, citations included.
pub(super) fn stored_output(row: &SectionRow) -> Option<Value> {
    row.2.as_deref().and_then(|o| serde_json::from_str(o).ok())
}

/// One stored section as generated, before staff edits are applied. The
/// staff actions (`can_regenerate`, `can_release`) are filled in by
/// `review::annotate`.
pub(super) fn section_view(row: SectionRow, sheet: &FactSheet) -> SectionView {
    let (key, state, output, reasons, needs_confirmation, _, _) = row;
    let kind = match key.as_str() {
        "at_a_glance" => dc_generation::prompt::SectionKind::AtAGlance,
        "journey" => dc_generation::prompt::SectionKind::Journey,
        "next_steps" => dc_generation::prompt::SectionKind::NextSteps,
        other => dc_generation::prompt::SectionKind::Procedure(
            other.trim_start_matches("procedure:").to_owned(),
        ),
    };
    let procedure_ref = match &kind {
        dc_generation::prompt::SectionKind::Procedure(r) => Some(r.clone()),
        _ => None,
    };
    let visibility = match state.as_str() {
        "passed" => SectionVisibility::Ready,
        "needs_review" => SectionVisibility::NeedsReview,
        _ => SectionVisibility::Failed,
    };
    let output: Option<Value> = output.and_then(|o| serde_json::from_str(&o).ok());
    let (blocks, questions) = output
        .map(|o| blocks_for(&kind, o, needs_confirmation))
        .unwrap_or_default();
    SectionView {
        title: jobs::section_label(&kind, sheet),
        key,
        visibility,
        reasons: serde_json::from_str(&reasons).unwrap_or_default(),
        blocks,
        questions,
        procedure_ref,
        can_regenerate: false,
        can_release: false,
        release_blockers: Vec::new(),
    }
}

fn block(field: &str, heading: &str, para: &Para) -> TextBlock {
    TextBlock {
        field: field.into(),
        heading: heading.into(),
        sentences: para.iter().map(|s| s.text.clone()).collect(),
        source: BlockSource::Generated,
        needs_confirmation: false,
    }
}

fn question_texts(questions: &[Question]) -> Vec<String> {
    questions.iter().map(|q| q.text.clone()).collect()
}

/// Turns a section's stored output into display blocks and questions.
/// Citations stay in the stored JSON and are not sent to the browser yet.
fn blocks_for(
    kind: &dc_generation::prompt::SectionKind,
    output: Value,
    needs_note_confirmation: bool,
) -> (Vec<TextBlock>, Vec<String>) {
    use dc_generation::prompt::SectionKind;
    match kind {
        SectionKind::AtAGlance => {
            let Ok(out) = serde_json::from_value::<AtAGlanceOut>(output) else {
                return Default::default();
            };
            let mut blocks = vec![block("summary", "Your plan at a glance", &out.summary)];
            if !out.objective.is_empty() {
                // The objective always cites a clinical note, so it waits for
                // staff confirmation like the documented reason (P11).
                blocks.push(TextBlock {
                    source: BlockSource::ClinicalNote,
                    needs_confirmation: true,
                    ..block("objective", "What the treatment aims to do", &out.objective)
                });
            }
            (blocks, Vec::new())
        }
        SectionKind::Journey => {
            let Ok(out) = serde_json::from_value::<JourneyOut>(output) else {
                return Default::default();
            };
            let mut blocks: Vec<TextBlock> = out
                .phases
                .iter()
                .enumerate()
                .map(|(i, phase)| {
                    block(
                        &format!("phases/{i}/summary"),
                        &format!("Phase {}", i + 1),
                        &phase.summary,
                    )
                })
                .collect();
            if !out.dependencies.is_empty() {
                blocks.push(block(
                    "dependencies",
                    "How the steps fit together",
                    &out.dependencies,
                ));
            }
            (blocks, Vec::new())
        }
        SectionKind::Procedure(_) => {
            let Ok(out) = serde_json::from_value::<ProcedureOut>(output) else {
                return Default::default();
            };
            let documented = out.why.kind == WhyKind::Documented;
            let blocks = vec![
                block("what_it_is", "What it is", &out.what_it_is),
                TextBlock {
                    source: if documented {
                        BlockSource::ClinicalNote
                    } else {
                        BlockSource::Generated
                    },
                    needs_confirmation: documented && needs_note_confirmation,
                    ..block("why", "Why your dentist recommended it", &out.why.sentences)
                },
                block(
                    "what_happens",
                    "What happens during the visit",
                    &out.what_happens,
                ),
                block("recovery", "Recovery", &out.recovery),
                block("benefits", "Benefits", &out.benefits),
                block("if_delayed", "If you wait", &out.if_delayed),
            ];
            (blocks, question_texts(&out.questions))
        }
        SectionKind::NextSteps => {
            let Ok(out) = serde_json::from_value::<NextStepsOut>(output) else {
                return Default::default();
            };
            (
                vec![
                    block("outlook", "What to expect", &out.outlook),
                    block("next_steps", "Next steps", &out.next_steps),
                ],
                question_texts(&out.questions),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags() {
        for ok in ["en", "es-MX", "zh-Hant", "vi", "pt-BR", "sr-Latn-RS"] {
            assert!(valid_language(ok), "{ok}");
        }
        for bad in [
            "",
            "e",
            "english",
            "en_US",
            "en-",
            "en-x",
            "12",
            "en-US-a-b-c-d",
        ] {
            assert!(!valid_language(bad), "{bad}");
        }
    }
}
