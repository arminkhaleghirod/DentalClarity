//! The generation timing benchmark (docs/AI_PIPELINE.md, measuring the 90
//! second target; docs/DECISIONS.md E38), run by the `dc-bench` binary.
//!
//! Each plan of the workload goes through the server's own job runner on an
//! in-memory database, one job at a time. A job is timed from the moment it
//! enters `running` until every section is in its final state (S0 to S8),
//! which leaves out queue wait, other languages and narration. Per-stage
//! call counts and latencies come from `ai_calls`.

use crate::config::HttpConfig;
use crate::generation::Generation;
use crate::jobs::{self, Jobs};
use crate::limits::Limits;
use crate::{AppState, storage};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use dc_ai::{Role, TextModel, TextRequest, TextResponse, Usage};
use dc_core::Mode;
use dc_core::plan::PlanKey;
use dc_generation::prompt::REVIEWER_PROMPT;
use dc_knowledge::KnowledgeBase;
use dc_pms::PmsConnector;
use dc_pms::synthetic::SyntheticConnector;
use serde::Serialize;
use serde_json::json;
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Benchmark settings.
#[derive(Debug, Clone)]
pub struct Options {
    /// Workload in the synthetic practice format: every plan in it is run.
    pub fixture: PathBuf,
    pub knowledge: PathBuf,
    /// Runs of each plan.
    pub runs: usize,
    /// Scripted model latency per call, drawn evenly from this range.
    pub latency_ms: (u64, u64),
}

/// The models a benchmark uses.
pub enum Models {
    /// A scripted model that writes replies that pass every check.
    Scripted,
    /// A configured generation setup, such as OpenRouter in demo mode.
    Configured(Box<dyn FnOnce(&SqlitePool) -> Generation>),
}

#[derive(Debug, Clone, Serialize)]
pub struct StageStats {
    pub stage: String,
    pub calls: u32,
    pub mean_latency_ms: f64,
    pub max_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    pub plan: String,
    pub run: usize,
    pub state: String,
    pub elapsed_ms: u64,
    pub sections: u32,
    /// Sections that needed at least one repair round.
    pub repaired: u32,
    /// Sections not passed, with the reasons staff would see.
    pub held: Vec<String>,
    pub cost_usd: f64,
    pub stages: Vec<StageStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub runs: Vec<RunResult>,
    pub p50_ms: u64,
    pub p90_ms: u64,
    pub max_ms: u64,
    /// Share of sections that needed a repair.
    pub repair_rate: f64,
    pub mean_cost_usd: f64,
    pub stages: Vec<StageStats>,
}

/// Nearest-rank percentile of sorted values.
pub fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub async fn run(options: &Options, models: Models) -> Result<Report> {
    let pms = SyntheticConnector::from_path(&options.fixture)
        .with_context(|| format!("loading {}", options.fixture.display()))?;
    let kb = KnowledgeBase::load(&options.knowledge).context("loading the knowledge base")?;
    let db = storage::open_in_memory().await?;
    let generation = match models {
        Models::Scripted => Generation::new(
            db.clone(),
            None,
            Arc::new(ScriptedModel::new(options.latency_ms)),
            "scripted-writer".into(),
            Arc::new(ScriptedModel::new(options.latency_ms)),
            "scripted-reviewer".into(),
            None,
        ),
        Models::Configured(build) => build(&db),
    };
    let mut plans = Vec::new();
    for patient in pms.search_patients("").await? {
        for plan in pms.treatment_plans(&patient.key).await? {
            plans.push(plan.key);
        }
    }
    if plans.is_empty() {
        bail!("{} has no plans", options.fixture.display());
    }
    let state = AppState {
        mode: Mode::Demo,
        http: Arc::new(HttpConfig {
            public_origin: "http://127.0.0.1".into(),
            trusted_proxy: None,
            client_ip_header: None,
            hsts: false,
        }),
        db: db.clone(),
        default_practice_name: pms.practice_name().to_owned(),
        pms: Arc::new(pms),
        codes: Arc::new(kb.codes.clone()),
        kb: Arc::new(kb),
        narration: Arc::new(crate::narration::Narration::unavailable(
            Mode::Demo,
            db.clone(),
        )),
        generation: Some(Arc::new(generation)),
        jobs: Arc::new(Jobs::default()),
        limits: Arc::new(Limits::default()),
        setup_code: Arc::new(std::sync::Mutex::new(None)),
    };

    // Run 1 of every plan, then run 2, and so on, so a slow spell of the
    // provider spreads over the plans.
    let mut runs = Vec::new();
    for run in 1..=options.runs {
        for plan in &plans {
            let result = run_once(&state, plan, run).await?;
            tracing::info!(
                plan = %result.plan,
                run,
                elapsed_ms = result.elapsed_ms,
                state = %result.state,
                "benchmark run finished"
            );
            runs.push(result);
        }
    }
    Ok(summarize(runs, stage_stats(&db, None).await?))
}

async fn run_once(state: &AppState, plan: &PlanKey, run: usize) -> Result<RunResult> {
    let snapshot = state.pms.plan_snapshot(plan).await?;
    let presentation_id = jobs::new_id();
    let job_id = jobs::new_id();
    let progress = jobs::initial_progress(&snapshot, &state.kb);
    sqlx::query(
        "INSERT INTO presentations (id, owner_scope, patient_key, patient_display_name, plan_key,
             language, reading_level, created_at, snapshot_json, fingerprint)
         VALUES (?, 'staff', ?, 'Benchmark', ?, 'en', 'standard', ?, ?, ?)",
    )
    .bind(&presentation_id)
    .bind(&snapshot.patient.key.0)
    .bind(&plan.0)
    .bind(crate::sessions::now())
    .bind(serde_json::to_string(&snapshot)?)
    .bind(snapshot.fingerprint())
    .execute(&state.db)
    .await?;
    sqlx::query(
        "INSERT INTO jobs (id, presentation_id, state, created_at, progress_json)
         VALUES (?, ?, 'queued', ?, ?)",
    )
    .bind(&job_id)
    .bind(&presentation_id)
    .bind(jobs::now_ms())
    .bind(serde_json::to_string(&progress)?)
    .execute(&state.db)
    .await?;
    jobs::spawn(state, job_id.clone());

    let (job_state, elapsed_ms) = loop {
        let (s, elapsed): (String, i64) =
            sqlx::query_as("SELECT state, elapsed_ms FROM jobs WHERE id = ?")
                .bind(&job_id)
                .fetch_one(&state.db)
                .await?;
        if jobs::parse_job_state(&s).is_finished() {
            break (s, elapsed);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let (sections, repaired): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(attempts > 1), 0) FROM presentation_sections
         WHERE presentation_id = ?",
    )
    .bind(&presentation_id)
    .fetch_one(&state.db)
    .await?;
    let held: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT key, state, reasons_json FROM presentation_sections
         WHERE presentation_id = ? AND state <> 'passed' ORDER BY position",
    )
    .bind(&presentation_id)
    .fetch_all(&state.db)
    .await?;
    let (cost,): (f64,) =
        sqlx::query_as("SELECT COALESCE(SUM(cost_usd), 0) FROM ai_calls WHERE job_id = ?")
            .bind(&job_id)
            .fetch_one(&state.db)
            .await?;
    Ok(RunResult {
        plan: plan.0.clone(),
        run,
        state: job_state,
        elapsed_ms: u64::try_from(elapsed_ms).unwrap_or(0),
        sections: u32::try_from(sections).unwrap_or(0),
        repaired: u32::try_from(repaired).unwrap_or(0),
        held: held
            .into_iter()
            .map(|(key, state, reasons)| format!("{key} {state}: {reasons}"))
            .collect(),
        cost_usd: cost,
        stages: stage_stats(&state.db, Some(&job_id)).await?,
    })
}

async fn stage_stats(db: &SqlitePool, job_id: Option<&str>) -> Result<Vec<StageStats>> {
    let rows: Vec<(String, i64, f64, i64)> = sqlx::query_as(
        "SELECT stage, COUNT(*), AVG(latency_ms), MAX(latency_ms) FROM ai_calls
         WHERE ? IS NULL OR job_id = ?
         GROUP BY stage
         ORDER BY CASE stage WHEN 'write' THEN 0 WHEN 'review' THEN 1 WHEN 'repair' THEN 2 ELSE 3 END",
    )
    .bind(job_id)
    .bind(job_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(stage, calls, mean, max)| StageStats {
            stage,
            calls: u32::try_from(calls).unwrap_or(0),
            mean_latency_ms: mean,
            max_latency_ms: u64::try_from(max).unwrap_or(0),
        })
        .collect())
}

fn summarize(runs: Vec<RunResult>, stages: Vec<StageStats>) -> Report {
    let mut times: Vec<u64> = runs.iter().map(|r| r.elapsed_ms).collect();
    times.sort_unstable();
    let sections: u32 = runs.iter().map(|r| r.sections).sum();
    let repaired: u32 = runs.iter().map(|r| r.repaired).sum();
    let mean_cost = runs.iter().map(|r| r.cost_usd).sum::<f64>() / runs.len().max(1) as f64;
    Report {
        p50_ms: percentile(&times, 50.0),
        p90_ms: percentile(&times, 90.0),
        max_ms: times.last().copied().unwrap_or(0),
        repair_rate: if sections == 0 {
            0.0
        } else {
            f64::from(repaired) / f64::from(sections)
        },
        mean_cost_usd: mean_cost,
        stages,
        runs,
    }
}

/// The report as Markdown, for the terminal and the docs.
pub fn render(report: &Report) -> String {
    let secs = |ms: u64| format!("{:.1} s", ms as f64 / 1000.0);
    let mut out = format!(
        "| Runs | p50 | p90 | Max | Repair rate | Cost per run |\n|---|---|---|---|---|---|\n\
         | {} | {} | {} | {} | {:.1}% | ${:.4} |\n\n",
        report.runs.len(),
        secs(report.p50_ms),
        secs(report.p90_ms),
        secs(report.max_ms),
        report.repair_rate * 100.0,
        report.mean_cost_usd
    );
    out.push_str("| Stage | Calls | Mean latency | Max latency |\n|---|---|---|---|\n");
    for s in &report.stages {
        out.push_str(&format!(
            "| {} | {} | {:.0} ms | {} ms |\n",
            s.stage, s.calls, s.mean_latency_ms, s.max_latency_ms
        ));
    }
    out.push_str(
        "\n| Plan | Run | State | Time | Sections | Repaired |\n|---|---|---|---|---|---|\n",
    );
    for r in &report.runs {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            r.plan,
            r.run,
            r.state,
            secs(r.elapsed_ms),
            r.sections,
            r.repaired
        ));
    }
    out
}

/// A model that answers every section of any plan with a reply that passes
/// the checks, and every review with a pass, after a simulated latency.
pub struct ScriptedModel {
    latency_ms: (u64, u64),
    state: AtomicU64,
}

impl ScriptedModel {
    pub fn new(latency_ms: (u64, u64)) -> Self {
        Self {
            latency_ms,
            state: AtomicU64::new(0x2545_f491_4f6c_dd1d),
        }
    }

    /// A deterministic latency from the configured range (xorshift).
    fn next_latency(&self) -> u64 {
        let (lo, hi) = self.latency_ms;
        if hi <= lo {
            return lo;
        }
        let mut x = self.state.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state.store(x, Ordering::Relaxed);
        lo + x % (hi - lo + 1)
    }
}

fn sentence(text: &str, cite: &str) -> serde_json::Value {
    json!({ "text": text, "cites": [cite] })
}

/// The reply for the section a writer request asks for.
pub fn scripted_reply(request: &TextRequest) -> String {
    let user = request
        .messages
        .iter()
        .find(|m| m.role == Role::User)
        .map_or("", |m| m.content.as_str());
    let task = user.rsplit_once("<task>").map_or("", |(_, t)| t);
    let phases: Vec<String> = user
        .lines()
        .filter_map(|l| l.strip_prefix("- [fact:ph"))
        .filter_map(|l| l.split_once(']'))
        .map(|(n, _)| format!("ph{n}"))
        .collect();
    let first_phase = phases.first().cloned().unwrap_or_else(|| "ph1".into());
    let phase_cite = format!("fact:{first_phase}");
    let questions = json!([
        { "text": "What should I expect afterwards?", "cites": [] },
        { "text": "How should I care for this area?", "cites": [] }
    ]);
    if let Some(rest) = task.strip_prefix("Write the explanation for procedure ") {
        let r: String = rest
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        let fact = format!("fact:{r}");
        return json!({
            "procedure_ref": r,
            "what_it_is": [sentence("This treatment cares for the affected area.", &fact)],
            "why": { "kind": "common_reasons", "sentences": [sentence("Common reasons for this treatment include keeping teeth healthy.", &fact)] },
            "what_happens": [sentence("Your dental team explains each step as it happens.", &fact)],
            "recovery": [sentence("Many people feel comfortable afterwards.", &fact)],
            "benefits": [sentence("It can help keep your mouth healthy.", &fact)],
            "if_delayed": [sentence("Problems in a tooth can sometimes grow if they are left alone.", &fact)],
            "questions": questions
        })
        .to_string();
    }
    if task.contains("at a glance") {
        json!({
            "summary": [
                sentence("This plan cares for several of your teeth.", &phase_cite),
                sentence("Each part of the plan is explained in its own section.", &phase_cite)
            ],
            "objective": []
        })
    } else if task.contains("treatment journey") {
        json!({
            "phases": phases.iter().map(|ph| json!({
                "phase_ref": ph,
                "summary": [sentence("This phase covers the treatments listed for it.", &format!("fact:{ph}"))]
            })).collect::<Vec<_>>(),
            "dependencies": []
        })
    } else {
        json!({
            "outlook": [sentence("The plan aims to keep your teeth healthy and comfortable.", &phase_cite)],
            "next_steps": [sentence("You can talk with your dental team about any part of the plan.", &phase_cite)],
            "questions": questions
        })
    }
    .to_string()
}

#[async_trait]
impl TextModel for ScriptedModel {
    fn provider(&self) -> &'static str {
        "scripted"
    }

    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        let latency = self.next_latency();
        tokio::time::sleep(Duration::from_millis(latency)).await;
        let review = request
            .messages
            .iter()
            .any(|m| m.role == Role::System && m.content == REVIEWER_PROMPT);
        let content = if review {
            json!({ "verdict": "pass", "issues": [] }).to_string()
        } else {
            scripted_reply(&request)
        };
        let input_chars: usize = request.messages.iter().map(|m| m.content.len()).sum();
        Ok(TextResponse {
            model: request.model,
            route: None,
            usage: Usage {
                input_tokens: u32::try_from(input_chars / 4).unwrap_or(u32::MAX),
                output_tokens: u32::try_from(content.len() / 4).unwrap_or(u32::MAX),
                cost_usd: None,
            },
            content,
            latency_ms: latency,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let v: Vec<u64> = (1..=20).collect();
        assert_eq!(percentile(&v, 50.0), 10);
        assert_eq!(percentile(&v, 90.0), 18);
        assert_eq!(percentile(&[7], 90.0), 7);
        assert_eq!(percentile(&[], 50.0), 0);
    }

    #[tokio::test]
    async fn the_typical_workload_runs_and_every_section_passes() {
        let options = Options {
            fixture: root().join("fixtures/perf/typical.toml"),
            knowledge: root().join("knowledge"),
            runs: 1,
            latency_ms: (0, 0),
        };
        let report = run(&options, Models::Scripted).await.unwrap();
        assert_eq!(report.runs.len(), 5);
        for r in &report.runs {
            assert_eq!(r.state, "succeeded", "{}: {:?}", r.plan, r.held);
            // 4 to 6 procedures plus at a glance, journey and next steps.
            assert!((7..=9).contains(&r.sections), "{}: {}", r.plan, r.sections);
            assert_eq!(r.repaired, 0);
        }
        let calls: u32 = report.stages.iter().map(|s| s.calls).sum();
        let sections: u32 = report.runs.iter().map(|r| r.sections).sum();
        assert_eq!(calls, 2 * sections);
        assert!(render(&report).contains("| 5 |"));
    }
}
