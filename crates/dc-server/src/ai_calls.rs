//! The `ai_calls` log: one row per model call (docs/DECISIONS.md E35).
//!
//! Rows carry ids, counts, timings and costs only. Prompt, reply and patient
//! text never reach this table. The job runner sets a [`JobContext`] around
//! each section, and the pipeline sets its own call site
//! (`dc_generation::current_call_site`), so the model wrapper can fill in
//! where a call came from without the request carrying it.

use anyhow::Result;
use sqlx::SqlitePool;
use std::future::Future;

/// Demo `ai_calls` rows are deleted this long after the call.
pub const DEMO_RETENTION_MS: i64 = 90 * 24 * 60 * 60 * 1000;

/// The job a model call belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobContext {
    pub presentation_id: String,
    pub job_id: String,
}

tokio::task_local! {
    static JOB: JobContext;
}

/// Runs `future` with `context` as the job of every model call it makes.
pub async fn in_job<F: Future>(context: JobContext, future: F) -> F::Output {
    JOB.scope(context, future).await
}

/// The job of the model call being made, if a job is making it.
pub fn current_job() -> Option<JobContext> {
    JOB.try_with(Clone::clone).ok()
}

/// One `ai_calls` row.
#[derive(Debug, Clone, PartialEq)]
pub struct CallRecord {
    pub stage: &'static str,
    pub section_key: Option<String>,
    pub round: u32,
    pub attempt: u32,
    pub provider: &'static str,
    pub model_requested: String,
    pub model_returned: Option<String>,
    pub provider_route: Option<String>,
    pub prompt_version: Option<String>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub audio_chars: Option<u32>,
    pub latency_ms: u64,
    /// `ok`, `unusable` (timed out or unreadable, possibly billed) or `failed`.
    pub outcome: &'static str,
    pub cost_usd: f64,
    pub cost_source: &'static str,
    pub reported_cost_usd: Option<f64>,
    pub est_cost_usd: Option<f64>,
}

/// Writes one row, with the current job if there is one.
pub async fn record(db: &SqlitePool, call: &CallRecord) -> Result<()> {
    let job = current_job();
    sqlx::query(
        "INSERT INTO ai_calls (created_at, presentation_id, job_id, section_key, stage, round,
             attempt, provider, model_requested, model_returned, provider_route, prompt_version,
             input_tokens, output_tokens, audio_chars, latency_ms, outcome, cost_usd, cost_source,
             reported_cost_usd, est_cost_usd)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(crate::jobs::now_ms())
    .bind(job.as_ref().map(|j| j.presentation_id.as_str()))
    .bind(job.as_ref().map(|j| j.job_id.as_str()))
    .bind(&call.section_key)
    .bind(call.stage)
    .bind(i64::from(call.round))
    .bind(i64::from(call.attempt))
    .bind(call.provider)
    .bind(&call.model_requested)
    .bind(&call.model_returned)
    .bind(&call.provider_route)
    .bind(&call.prompt_version)
    .bind(i64::from(call.input_tokens))
    .bind(i64::from(call.output_tokens))
    .bind(call.audio_chars.map(i64::from))
    .bind(i64::try_from(call.latency_ms).unwrap_or(i64::MAX))
    .bind(call.outcome)
    .bind(call.cost_usd)
    .bind(call.cost_source)
    .bind(call.reported_cost_usd)
    .bind(call.est_cost_usd)
    .execute(db)
    .await?;
    Ok(())
}

/// Writes `call`, logging instead of failing: a missing row must not fail
/// the model call it describes.
pub async fn record_or_warn(db: &SqlitePool, call: &CallRecord) {
    if let Err(err) = record(db, call).await {
        tracing::warn!(%err, "could not write the ai_calls row");
    }
}
