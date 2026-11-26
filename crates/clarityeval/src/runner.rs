//! `clarityeval run`: drives the real `dc-generation` pipeline on cases and
//! records what happened (docs/CLARITYEVAL.md section 14).
//!
//! Every model call goes through [`Recorder`], which wraps the app's own
//! OpenRouter adapter, so the benchmark uses the app's exact request path and
//! still sees per-call latency, tokens, cost and the model id the route
//! returned. No prompt or response text is written outside `results.jsonl`.

use crate::case::{Case, LoadedCase, Split, TaskKind};
use crate::meta;
use crate::score::{self, Metric, SectionRecord};
use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use dc_ai::openrouter::OpenRouter;
use dc_ai::{ProviderPolicy, ReasoningEffort, Role, TextModel, TextRequest, TextResponse};
use dc_core::Mode;
use dc_generation::facts::FactSheet;
use dc_generation::pipeline::{self, Event, Models};
use dc_generation::prompt;
use dc_knowledge::KnowledgeBase;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

pub const ROUTE: &str = "openrouter";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallRecord {
    /// `writer` or `reviewer`, from the system prompt.
    pub role: String,
    pub model_requested: String,
    pub model_returned: Option<String>,
    /// Client-side wall clock around the adapter call.
    pub latency_ms: u64,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: Option<f64>,
    /// sha256 of the request messages, for matching repeated prompts.
    pub prompt_sha256: String,
    pub error: Option<String>,
}

/// Wraps a [`TextModel`] and records every call.
pub struct Recorder {
    inner: Arc<dyn TextModel>,
    calls: Mutex<Vec<CallRecord>>,
}

impl Recorder {
    pub fn new(inner: Arc<dyn TextModel>) -> Self {
        Recorder {
            inner,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn take(&self) -> Vec<CallRecord> {
        std::mem::take(&mut *self.calls.lock().expect("recorder lock"))
    }
}

#[async_trait]
impl TextModel for Recorder {
    fn provider(&self) -> &'static str {
        self.inner.provider()
    }

    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        let role = if request
            .messages
            .first()
            .is_some_and(|m| m.role == Role::System && m.content == prompt::REVIEWER_PROMPT)
        {
            "reviewer"
        } else {
            "writer"
        };
        let prompt_sha256 = hex::encode(Sha256::digest(
            serde_json::to_vec(&request.messages).unwrap_or_default(),
        ));
        let model_requested = request.model.clone();
        let started = Instant::now();
        let result = self.inner.generate(request).await;
        let latency_ms = started.elapsed().as_millis() as u64;
        let record = match &result {
            Ok(r) => CallRecord {
                role: role.into(),
                model_requested,
                model_returned: Some(r.model.clone()),
                latency_ms,
                input_tokens: r.usage.input_tokens,
                output_tokens: r.usage.output_tokens,
                cost_usd: r.usage.cost_usd,
                prompt_sha256,
                error: None,
            },
            Err(e) => CallRecord {
                role: role.into(),
                model_requested,
                model_returned: None,
                latency_ms,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: None,
                prompt_sha256,
                error: Some(e.to_string()),
            },
        };
        self.calls.lock().expect("recorder lock").push(record);
        result
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Params {
    pub temperature: f32,
    pub max_tokens: u32,
    pub reasoning: Option<ReasoningEffort>,
    pub max_repairs: usize,
    pub parallel_sections: usize,
}

impl Params {
    pub fn current(reasoning: Option<ReasoningEffort>) -> Self {
        Params {
            temperature: pipeline::TEMPERATURE,
            max_tokens: pipeline::MAX_OUTPUT_TOKENS,
            reasoning,
            max_repairs: pipeline::MAX_REPAIRS,
            parallel_sections: pipeline::PARALLEL_SECTIONS,
        }
    }
}

/// One line of `results.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseResult {
    pub run_id: String,
    pub case_id: String,
    pub case_sha256: String,
    pub split: Split,
    pub category: crate::case::Category,
    pub task_kind: TaskKind,
    pub route: String,
    pub writer_model: String,
    pub reviewer_model: String,
    pub models_returned: Vec<String>,
    /// The upstream provider behind the route. The app's adapter does not
    /// surface it yet, so this stays empty until it does.
    pub upstream_provider: Option<String>,
    pub params: Params,
    pub prompt_versions: Vec<String>,
    pub started_at: String,
    /// Wall clock for the whole case (all sections, repairs and reviews).
    pub elapsed_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
    pub cost_source: String,
    pub calls: Vec<CallRecord>,
    pub sections: Vec<SectionRecord>,
    pub metrics: Vec<Metric>,
    pub passed: bool,
    pub critical_failures: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseEntry {
    pub case_id: String,
    pub sha256: String,
}

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub run_id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub route: String,
    pub writer_model: String,
    pub reviewer_model: String,
    pub models_returned: Vec<String>,
    pub upstream_provider: Option<String>,
    pub params: Params,
    pub prompt_versions: Vec<String>,
    pub scorer_version: String,
    pub split: Split,
    pub case_filter: Vec<String>,
    /// sha256 over the sorted `case_id:sha256` lines of the cases run.
    pub dataset_sha256: String,
    pub cases: Vec<CaseEntry>,
    pub git_commit: Option<String>,
    pub git_dirty: Option<bool>,
    pub rustc: Option<String>,
    pub max_cost_usd: f64,
    pub total_cost_usd: f64,
    pub stopped_early: Option<String>,
}

pub struct RunOptions {
    pub split: Split,
    pub model: String,
    pub reviewer_model: Option<String>,
    pub reasoning: Option<ReasoningEffort>,
    pub patterns: Vec<String>,
    pub out: Option<PathBuf>,
    pub limit: Option<usize>,
    pub max_cost_usd: f64,
}

pub fn select<'a>(cases: &'a [LoadedCase], patterns: &[String]) -> Vec<&'a LoadedCase> {
    cases
        .iter()
        .filter(|c| {
            patterns.is_empty()
                || patterns.iter().any(|p| {
                    crate::case::wildcard(p, &c.case.case_id)
                        || crate::case::wildcard(p, &c.path.to_string_lossy().replace('\\', "/"))
                })
        })
        .collect()
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

pub fn prompt_versions() -> Vec<String> {
    vec![prompt::writer_version(), prompt::reviewer_version()]
}

pub async fn run(
    root: &Path,
    cases: &[LoadedCase],
    kb: &KnowledgeBase,
    opts: &RunOptions,
) -> Result<PathBuf> {
    let selected: Vec<&LoadedCase> = select(cases, &opts.patterns)
        .into_iter()
        .filter(|c| c.case.split == opts.split)
        .take(opts.limit.unwrap_or(usize::MAX))
        .collect();
    if selected.is_empty() {
        bail!("no cases match the filter");
    }
    if opts.max_cost_usd <= 0.0 {
        bail!("--max-cost-usd must be above zero");
    }

    let _ = dotenvy::from_path(root.join(".env"));
    let key = std::env::var("OPENROUTER_API_KEY")
        .context("OPENROUTER_API_KEY is not set (environment or .env)")?;
    let client: Arc<dyn TextModel> =
        Arc::new(OpenRouter::new(ProviderPolicy::for_mode(Mode::Demo), key)?);
    let recorder = Arc::new(Recorder::new(client));
    let reviewer_model = opts.reviewer_model.clone().unwrap_or(opts.model.clone());
    let models = Models {
        writer: recorder.clone(),
        writer_model: opts.model.clone(),
        reviewer: recorder.clone(),
        reviewer_model: reviewer_model.clone(),
        reasoning: opts.reasoning,
    };

    let started = SystemTime::now();
    let (git_commit, git_dirty) = meta::git_state(root);
    let short = git_commit
        .as_deref()
        .map_or("nogit".to_owned(), |s| s[..s.len().min(7)].to_owned());
    let run_id = format!("{}-{}-{short}", meta::compact(started), slug(&opts.model));
    let dir = opts
        .out
        .clone()
        .unwrap_or_else(|| root.join("eval/runs").join(&run_id));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let mut entries: Vec<CaseEntry> = selected
        .iter()
        .map(|c| CaseEntry {
            case_id: c.case.case_id.clone(),
            sha256: c.sha256.clone(),
        })
        .collect();
    entries.sort_by(|a, b| a.case_id.cmp(&b.case_id));
    let dataset = entries
        .iter()
        .map(|e| format!("{}:{}\n", e.case_id, e.sha256))
        .collect::<String>();
    let mut manifest = Manifest {
        run_id: run_id.clone(),
        started_at: meta::rfc3339(started),
        finished_at: None,
        route: ROUTE.into(),
        writer_model: opts.model.clone(),
        reviewer_model: reviewer_model.clone(),
        models_returned: vec![],
        upstream_provider: None,
        params: Params::current(opts.reasoning),
        prompt_versions: prompt_versions(),
        scorer_version: score::SCORER_VERSION.into(),
        split: opts.split,
        case_filter: opts.patterns.clone(),
        dataset_sha256: hex::encode(Sha256::digest(dataset.as_bytes())),
        cases: entries,
        git_commit,
        git_dirty,
        rustc: meta::rustc_version(root),
        max_cost_usd: opts.max_cost_usd,
        total_cost_usd: 0.0,
        stopped_early: None,
    };
    write_manifest(&dir, &manifest)?;

    let mut results = std::fs::File::create(dir.join("results.jsonl"))?;
    let mut returned: Vec<String> = Vec::new();
    for (i, loaded) in selected.iter().enumerate() {
        if manifest.total_cost_usd >= opts.max_cost_usd {
            manifest.stopped_early = Some(format!(
                "cost cap ${:.4} reached after {i} cases",
                opts.max_cost_usd
            ));
            break;
        }
        eprintln!(
            "[{}/{}] {} ({:?})",
            i + 1,
            selected.len(),
            loaded.case.case_id,
            loaded.case.task.kind
        );
        let result = run_case(&run_id, loaded, kb, &models, &recorder, opts).await;
        manifest.total_cost_usd += result.cost_usd.unwrap_or(0.0);
        for m in &result.models_returned {
            if !returned.contains(m) {
                returned.push(m.clone());
            }
        }
        eprintln!(
            "    {} in {:.1}s, ${:.4}, critical failures {}",
            if result.passed { "pass" } else { "fail" },
            result.elapsed_ms as f64 / 1000.0,
            result.cost_usd.unwrap_or(0.0),
            result.critical_failures
        );
        serde_json::to_writer(&mut results, &result)?;
        results.write_all(b"\n")?;
        results.flush()?;
    }
    manifest.models_returned = returned;
    manifest.finished_at = Some(meta::rfc3339(SystemTime::now()));
    write_manifest(&dir, &manifest)?;
    Ok(dir)
}

fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<()> {
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(manifest)?,
    )?;
    Ok(())
}

async fn run_case(
    run_id: &str,
    loaded: &LoadedCase,
    kb: &KnowledgeBase,
    models: &Models,
    recorder: &Recorder,
    opts: &RunOptions,
) -> CaseResult {
    let case: &Case = &loaded.case;
    let started = SystemTime::now();
    let clock = Instant::now();
    let progress = |e: Event| {
        if let Event::SectionDone { key, state } = e {
            eprintln!("    {key}: {state:?}");
        }
    };
    let mut error = None;
    let mut sections = Vec::new();
    if !case.task.kind.runnable() {
        error = Some(format!(
            "task kind {:?} is not runnable yet",
            case.task.kind
        ));
    } else {
        match case.to_snapshot() {
            Err(e) => error = Some(e.to_string()),
            Ok(snapshot) => {
                let level = case.source_facts.reading_level;
                if case.task.kind == TaskKind::Presentation {
                    let generated =
                        pipeline::generate(&snapshot, kb, models, level, &progress).await;
                    sections = generated.sections;
                } else {
                    let sheet = FactSheet::build(&snapshot, kb);
                    for kind in score::requested_sections(case, &sheet) {
                        sections.push(
                            pipeline::section(&kind, &sheet, kb, models, level, &progress).await,
                        );
                    }
                }
            }
        }
    }
    let records: Vec<SectionRecord> = sections.iter().map(SectionRecord::from_result).collect();
    let metrics = if error.is_none() {
        score::score(case, kb, &records)
    } else {
        Vec::new()
    };
    let calls = recorder.take();
    let mut models_returned: Vec<String> = Vec::new();
    for c in &calls {
        if let Some(m) = &c.model_returned
            && !models_returned.contains(m)
        {
            models_returned.push(m.clone());
        }
    }
    let reported: Vec<f64> = calls.iter().filter_map(|c| c.cost_usd).collect();
    let all_reported = calls.iter().filter(|c| c.error.is_none()).count() == reported.len();
    CaseResult {
        run_id: run_id.into(),
        case_id: case.case_id.clone(),
        case_sha256: loaded.sha256.clone(),
        split: case.split,
        category: case.category,
        task_kind: case.task.kind,
        route: ROUTE.into(),
        writer_model: models.writer_model.clone(),
        reviewer_model: models.reviewer_model.clone(),
        models_returned,
        upstream_provider: None,
        params: Params::current(opts.reasoning),
        prompt_versions: prompt_versions(),
        started_at: meta::rfc3339(started),
        elapsed_ms: clock.elapsed().as_millis() as u64,
        input_tokens: calls.iter().map(|c| u64::from(c.input_tokens)).sum(),
        output_tokens: calls.iter().map(|c| u64::from(c.output_tokens)).sum(),
        cost_usd: (!reported.is_empty()).then(|| reported.iter().sum()),
        cost_source: if calls.is_empty() {
            "none".into()
        } else if all_reported {
            "provider".into()
        } else {
            "provider_partial".into()
        },
        passed: error.is_none() && score::case_passed(&metrics),
        critical_failures: score::critical_failures(&metrics),
        calls,
        sections: records,
        metrics,
        error,
    }
}
