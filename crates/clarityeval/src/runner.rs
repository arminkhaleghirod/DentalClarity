//! `clarityeval run`: drives the systems under test on cases and records
//! what happened (docs/CLARITYEVAL.md section 14).
//!
//! Systems are the real `dc-generation` pipeline and, with `--baseline`, a
//! plain-prompt baseline, each per model. Every model call goes through
//! [`Recorder`], which wraps the app's own OpenRouter adapter, so the
//! benchmark uses the app's exact request path and still sees per-call
//! latency, tokens, cost, the model id the route returned and the upstream
//! provider. No prompt or response text is written outside `results.jsonl`.

use crate::baseline;
use crate::case::{Case, LoadedCase, Split, TaskKind};
use crate::judge::{self, Judgment};
use crate::meta;
use crate::score::visible::{self, VisibleScore};
use crate::score::{self, Metric, SectionRecord};
pub use crate::systems::Params;
use crate::systems::{
    self, BASELINE_TIMEOUT_S, EndpointSnapshot, JUDGE_TIMEOUT_S, SeedMode, SystemKind, SystemSpec,
};
use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use dc_ai::openrouter::OpenRouter;
use dc_ai::{
    AiError, Message, ProviderPolicy, ReasoningEffort, Role, TextModel, TextRequest, TextResponse,
};
use dc_core::Mode;
use dc_generation::facts::FactSheet;
use dc_generation::pipeline::{self, Event, Models};
use dc_generation::prompt;
use dc_knowledge::KnowledgeBase;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const ROUTE: &str = "openrouter";
/// Version of the run directory layout written by this runner.
pub const FORMAT: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallRecord {
    /// `writer`, `reviewer` or `baseline`, from the system prompt.
    pub role: String,
    pub model_requested: String,
    pub model_returned: Option<String>,
    /// Upstream provider the route reported for this call.
    #[serde(default)]
    pub provider_route: Option<String>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub reasoning: Option<ReasoningEffort>,
    /// Client-side wall clock around the adapter call.
    pub latency_ms: u64,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: Option<f64>,
    /// sha256 of the request messages, for matching repeated prompts.
    pub prompt_sha256: String,
    pub error: Option<String>,
}

/// Routes each request to the client built for its model.
pub struct Dispatch {
    clients: HashMap<String, Arc<dyn TextModel>>,
    fallback: Option<Arc<dyn TextModel>>,
}

impl Dispatch {
    /// One model answers everything (dry runs and tests).
    pub fn single(model: Arc<dyn TextModel>) -> Self {
        Dispatch {
            clients: HashMap::new(),
            fallback: Some(model),
        }
    }

    pub fn new(clients: HashMap<String, Arc<dyn TextModel>>) -> Self {
        Dispatch {
            clients,
            fallback: None,
        }
    }
}

#[async_trait]
impl TextModel for Dispatch {
    fn provider(&self) -> &'static str {
        "dispatch"
    }

    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        match self.clients.get(&request.model).or(self.fallback.as_ref()) {
            Some(c) => c.generate(request).await,
            None => Err(AiError::Request(format!(
                "no client configured for {}",
                request.model
            ))),
        }
    }
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

fn role_of(request: &TextRequest) -> &'static str {
    match request.messages.first() {
        Some(m) if m.role == Role::System && m.content == prompt::REVIEWER_PROMPT => "reviewer",
        Some(m) if m.role == Role::System && m.content == baseline::BASELINE_PROMPT => "baseline",
        Some(m) if m.role == Role::System && m.content == judge::JUDGE_PROMPT => "judge",
        _ => "writer",
    }
}

#[async_trait]
impl TextModel for Recorder {
    fn provider(&self) -> &'static str {
        self.inner.provider()
    }

    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        let role = role_of(&request);
        let prompt_sha256 = hex::encode(Sha256::digest(
            serde_json::to_vec(&request.messages).unwrap_or_default(),
        ));
        let model_requested = request.model.clone();
        let (temperature, reasoning) = (Some(request.temperature), request.reasoning);
        let started = Instant::now();
        let result = self.inner.generate(request).await;
        let latency_ms = started.elapsed().as_millis() as u64;
        let record = match &result {
            Ok(r) => CallRecord {
                role: role.into(),
                model_requested,
                model_returned: Some(r.model.clone()),
                provider_route: r.route.clone(),
                temperature,
                reasoning,
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
                provider_route: None,
                temperature,
                reasoning,
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

/// One line of `results.jsonl`: one system on one case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseResult {
    pub run_id: String,
    /// `pipeline:<model>` or `baseline:<model>`. Empty in runs before format 2.
    #[serde(default)]
    pub system: String,
    #[serde(default)]
    pub system_kind: SystemKind,
    pub case_id: String,
    pub case_sha256: String,
    #[serde(default)]
    pub plan_ref: String,
    pub split: Split,
    pub category: crate::case::Category,
    pub task_kind: TaskKind,
    pub route: String,
    pub writer_model: String,
    pub reviewer_model: String,
    pub models_returned: Vec<String>,
    /// Distinct upstream providers the route reported, comma separated.
    pub upstream_provider: Option<String>,
    pub params: Params,
    pub prompt_versions: Vec<String>,
    pub started_at: String,
    /// Wall clock for the whole presentation (all sections, repairs and reviews).
    pub elapsed_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
    pub cost_source: String,
    pub calls: Vec<CallRecord>,
    pub sections: Vec<SectionRecord>,
    pub metrics: Vec<Metric>,
    /// What a patient would see, scored (see `score::visible`).
    #[serde(default)]
    pub visible: Option<VisibleScore>,
    #[serde(default)]
    pub judgments: Vec<Judgment>,
    pub passed: bool,
    pub critical_failures: usize,
    pub error: Option<String>,
}

impl CaseResult {
    /// System label, also for runs written before systems existed.
    pub fn system_label(&self) -> String {
        if self.system.is_empty() {
            SystemSpec::label(self.system_kind, &self.writer_model)
        } else {
            self.system.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseEntry {
    pub case_id: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Commitment {
    pub path: String,
    pub file_sha256: String,
    /// The root hash found in the file (64 hex characters), if any.
    pub root: Option<String>,
    /// Whether the cases run match the commitment (needs the local salts file).
    #[serde(default)]
    pub verified: Option<bool>,
    #[serde(default)]
    pub verification: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgeSpec {
    pub model: String,
    pub reasoning: Option<ReasoningEffort>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub prompt_version: String,
    pub routing: Value,
    pub seed: Option<u64>,
}

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "legacy_format")]
    pub format: u32,
    pub run_id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub route: String,
    #[serde(default)]
    pub systems: Vec<SystemSpec>,
    #[serde(default)]
    pub judge: Option<JudgeSpec>,
    /// Format 1 only: the single writer and reviewer.
    #[serde(default)]
    pub writer_model: String,
    #[serde(default)]
    pub reviewer_model: String,
    pub models_returned: Vec<String>,
    #[serde(default)]
    pub provider_routes: Vec<String>,
    #[serde(default)]
    pub upstream_provider: Option<String>,
    /// Format 1 only.
    #[serde(default)]
    pub params: Option<Params>,
    #[serde(default)]
    pub prompt_versions: Vec<String>,
    pub scorer_version: String,
    #[serde(default)]
    pub visible_scorer_version: String,
    pub split: Split,
    #[serde(default)]
    pub cases_dir: Option<String>,
    #[serde(default)]
    pub final_run: bool,
    #[serde(default)]
    pub commitment: Option<Commitment>,
    pub case_filter: Vec<String>,
    /// sha256 over the sorted `case_id:sha256` lines of the cases run.
    pub dataset_sha256: String,
    pub cases: Vec<CaseEntry>,
    #[serde(default)]
    pub case_counts: BTreeMap<String, usize>,
    pub git_commit: Option<String>,
    pub git_dirty: Option<bool>,
    pub rustc: Option<String>,
    pub max_cost_usd: f64,
    pub total_cost_usd: f64,
    #[serde(default)]
    pub judge_cost_usd: f64,
    pub stopped_early: Option<String>,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub seed_mode: Option<SeedMode>,
    /// Per model: whether the seed was sent and why.
    #[serde(default)]
    pub seed_notes: BTreeMap<String, String>,
    #[serde(default)]
    pub concurrency: usize,
    #[serde(default)]
    pub endpoints: BTreeMap<String, EndpointSnapshot>,
    #[serde(default)]
    pub dry_run: bool,
    /// The command line, for "how to rerun".
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub config_file: Option<String>,
    #[serde(default)]
    pub config_sha256: Option<String>,
}

fn legacy_format() -> u32 {
    1
}

#[derive(Debug, Clone)]
pub struct ModelChoice {
    pub model: String,
    pub reviewer: Option<String>,
    pub reasoning: Option<ReasoningEffort>,
}

pub struct RunOptions {
    pub name: Option<String>,
    pub split: Split,
    /// Where the cases came from, recorded relative to the repo root.
    pub cases_dir: PathBuf,
    pub final_run: bool,
    pub allow_missing_commitment: bool,
    pub models: Vec<ModelChoice>,
    pub baseline: bool,
    pub judge: Option<String>,
    pub judge_reasoning: Option<ReasoningEffort>,
    pub seed: u64,
    pub seed_mode: SeedMode,
    pub concurrency: usize,
    pub patterns: Vec<String>,
    pub out: Option<PathBuf>,
    pub limit: Option<usize>,
    pub max_cost_usd: f64,
    pub dry_run: bool,
    pub bootstrap: usize,
    pub command: Vec<String>,
    pub config_file: Option<PathBuf>,
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

pub const COMMITMENT_FILE: &str = "eval/commitments/heldout-v1.txt";

/// Reads the held-out commitment, if present: the file hash and the root
/// hash it states (the first 64-hex token, preferring a line naming "root").
pub fn read_commitment(root: &Path) -> Option<Commitment> {
    let path = root.join(COMMITMENT_FILE);
    let bytes = std::fs::read(&path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let hex64 = |line: &str| {
        line.split(|c: char| !c.is_ascii_hexdigit())
            .find(|t| t.len() == 64)
            .map(str::to_lowercase)
    };
    let lines: Vec<&str> = text.lines().collect();
    // "Root hash: <hex>" on one line, or a "Root hash" heading over the hex.
    let root_hash = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.to_lowercase().contains("root"))
        .find_map(|(i, l)| {
            hex64(l).or_else(|| {
                lines[i + 1..]
                    .iter()
                    .find(|n| !n.trim().is_empty())
                    .filter(|n| n.trim().len() == 64)
                    .and_then(|n| hex64(n))
            })
        })
        .or_else(|| lines.iter().find_map(|l| hex64(l)));
    Some(Commitment {
        path: COMMITMENT_FILE.into(),
        file_sha256: hex::encode(Sha256::digest(&bytes)),
        root: root_hash,
        verified: None,
        verification: vec![],
    })
}

/// Per-case hashes listed in a commitment: `<case_id> <path> <hex64>` lines.
fn committed_hashes(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            (t.len() == 3 && t[2].len() == 64 && t[2].bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| (t[0].to_owned(), t[2].to_lowercase()))
        })
        .collect()
}

/// Checks the cases against the commitment with the local salts file
/// (`<case_id> <salt hex>` per line): per-case `sha256(salt_hex || file bytes)`
/// and the root over the sorted per-case hashes. Fills `verified`.
pub fn verify_commitment(
    root: &Path,
    commitment: &mut Commitment,
    cases: &[&LoadedCase],
    salts: &Path,
) {
    let mut problems = Vec::new();
    let text = std::fs::read_to_string(root.join(&commitment.path)).unwrap_or_default();
    let committed = committed_hashes(&text);
    let Ok(salt_text) = std::fs::read_to_string(salts) else {
        commitment.verification = vec![format!("no salts file at {}", salts.display())];
        return;
    };
    let salt: BTreeMap<&str, &str> = salt_text
        .lines()
        .filter_map(|l| l.split_once(char::is_whitespace))
        .map(|(id, s)| (id.trim(), s.trim()))
        .collect();
    let mut all: Vec<String> = Vec::new();
    for (id, s) in &salt {
        let Some(c) = cases.iter().find(|c| c.case.case_id == *id) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&c.path) else {
            problems.push(format!("{id}: cannot read {}", c.path.display()));
            continue;
        };
        let mut h = Sha256::new();
        h.update(s.as_bytes());
        h.update(&bytes);
        let got = hex::encode(h.finalize());
        match committed.get(*id) {
            Some(want) if *want == got => {}
            Some(_) => problems.push(format!("{id}: file differs from its committed hash")),
            None => problems.push(format!("{id}: not in the commitment")),
        }
        all.push(got);
    }
    for c in cases {
        if !salt.contains_key(c.case.case_id.as_str()) {
            problems.push(format!(
                "{}: no salt, so it is not committed",
                c.case.case_id
            ));
        }
    }
    // The root covers every committed case, so check it only on a full run.
    if all.len() == committed.len() {
        all.sort();
        let got = hex::encode(Sha256::digest(all.concat().as_bytes()));
        if commitment.root.as_deref() != Some(got.as_str()) {
            problems.push("recomputed root differs from the committed root".into());
        }
    } else {
        problems.push(format!(
            "root not rechecked: {} of {} committed cases in this run",
            all.len(),
            committed.len()
        ));
    }
    let hard = problems
        .iter()
        .any(|p| !p.starts_with("root not rechecked"));
    commitment.verified = Some(!hard);
    commitment.verification = problems;
}

/// The systems a run compares, in report order.
pub fn plan_systems(opts: &RunOptions) -> Vec<(SystemSpec, ModelChoice)> {
    let mut out = Vec::new();
    for m in &opts.models {
        let reviewer = m.reviewer.clone().unwrap_or_else(|| m.model.clone());
        let mut routing = BTreeMap::new();
        routing.insert(m.model.clone(), systems::routing_for(&m.model));
        routing.insert(reviewer.clone(), systems::routing_for(&reviewer));
        out.push((
            SystemSpec {
                label: SystemSpec::label(SystemKind::Pipeline, &m.model),
                kind: SystemKind::Pipeline,
                writer_model: m.model.clone(),
                reviewer_model: Some(reviewer),
                params: Params::current(m.reasoning),
                prompt_versions: prompt_versions(),
                routing,
            },
            m.clone(),
        ));
    }
    if opts.baseline {
        for m in &opts.models {
            let mut routing = BTreeMap::new();
            routing.insert(m.model.clone(), systems::routing_for(&m.model));
            out.push((
                SystemSpec {
                    label: SystemSpec::label(SystemKind::Baseline, &m.model),
                    kind: SystemKind::Baseline,
                    writer_model: m.model.clone(),
                    reviewer_model: None,
                    params: Params {
                        temperature: pipeline::TEMPERATURE,
                        max_tokens: baseline::BASELINE_MAX_TOKENS,
                        reasoning: m.reasoning,
                        max_repairs: 0,
                        parallel_sections: 1,
                        seed: None,
                        timeout_s: Some(BASELINE_TIMEOUT_S),
                    },
                    prompt_versions: vec![baseline::version()],
                    routing,
                },
                m.clone(),
            ));
        }
    }
    out
}

struct Clients {
    pipeline: Arc<dyn TextModel>,
    baseline: Arc<dyn TextModel>,
    judge: Arc<dyn TextModel>,
}

fn build_clients(
    root: &Path,
    models: &[String],
    judge_model: Option<&str>,
    seeds: &BTreeMap<String, Option<u64>>,
) -> Result<Clients> {
    let _ = dotenvy::from_path(root.join(".env"));
    let key = std::env::var("OPENROUTER_API_KEY")
        .context("OPENROUTER_API_KEY is not set (environment or .env)")?;
    let make = |m: &str, timeout: Option<u64>| -> Result<Arc<dyn TextModel>> {
        let mut c = OpenRouter::new(ProviderPolicy::for_mode(Mode::Demo), key.clone())?
            .with_routing(systems::routing_for(m))
            .with_seed(seeds.get(m).copied().flatten());
        if let Some(t) = timeout {
            c = c.with_timeout(Duration::from_secs(t))?;
        }
        Ok(Arc::new(c))
    };
    let mut pipeline = HashMap::new();
    let mut baseline = HashMap::new();
    for m in models {
        // The pipeline keeps the app adapter's own timeout.
        pipeline.insert(m.clone(), make(m, None)?);
        baseline.insert(m.clone(), make(m, Some(BASELINE_TIMEOUT_S))?);
    }
    let mut judge = HashMap::new();
    if let Some(j) = judge_model {
        judge.insert(j.to_owned(), make(j, Some(JUDGE_TIMEOUT_S))?);
    }
    Ok(Clients {
        pipeline: Arc::new(Dispatch::new(pipeline)),
        baseline: Arc::new(Dispatch::new(baseline)),
        judge: Arc::new(Dispatch::new(judge)),
    })
}

pub async fn run(
    root: &Path,
    cases: &[LoadedCase],
    kb: &KnowledgeBase,
    opts: &RunOptions,
) -> Result<PathBuf> {
    if opts.split == Split::Heldout && !opts.final_run {
        bail!(
            "the held-out split is run once per model per freeze; pass --final to confirm this is a final run"
        );
    }
    let mut commitment = read_commitment(root);
    if opts.split == Split::Heldout && commitment.is_none() && !opts.allow_missing_commitment {
        bail!(
            "no held-out commitment at {COMMITMENT_FILE}; publish it before final runs \
             (or pass --allow-missing-commitment, which the report flags)"
        );
    }
    if opts.models.is_empty() {
        bail!("no models given (--models a,b or [[systems]] in --config)");
    }
    let selected: Vec<&LoadedCase> = select(cases, &opts.patterns)
        .into_iter()
        .filter(|c| c.case.split == opts.split)
        .take(opts.limit.unwrap_or(usize::MAX))
        .collect();
    if selected.is_empty() {
        bail!("no cases match the filter");
    }
    if opts.split == Split::Heldout
        && let Some(c) = commitment.as_mut()
    {
        let salts = opts
            .cases_dir
            .parent()
            .map_or_else(|| root.join("eval/heldout"), Path::to_path_buf)
            .join("salts.txt");
        verify_commitment(root, c, &selected, &salts);
        for v in &c.verification {
            eprintln!("commitment: {v}");
        }
        if c.verified == Some(false) && !opts.allow_missing_commitment {
            bail!(
                "the held-out cases do not match {COMMITMENT_FILE}; regenerate the commitment \
                 (v2) or restore the committed files"
            );
        }
    }
    if opts.max_cost_usd <= 0.0 {
        bail!("--max-cost-usd must be above zero");
    }

    let plans = plan_systems(opts);
    let mut models: Vec<String> = Vec::new();
    for (s, _) in &plans {
        for m in s.routing.keys() {
            if !models.contains(m) {
                models.push(m.clone());
            }
        }
    }
    let mut all_models = models.clone();
    if let Some(j) = &opts.judge
        && !all_models.contains(j)
    {
        all_models.push(j.clone());
    }

    // Endpoint snapshot and seed decision per model.
    let mut endpoints = BTreeMap::new();
    let mut seeds: BTreeMap<String, Option<u64>> = BTreeMap::new();
    let mut seed_notes = BTreeMap::new();
    for m in &all_models {
        let snap = if opts.dry_run {
            None
        } else {
            let s = systems::fetch_endpoints(m).await;
            if let Some(e) = &s.error {
                eprintln!("warning: endpoint list for {m} unavailable: {e}");
            } else {
                let reasoning = plans
                    .iter()
                    .any(|(s, _)| s.writer_model == *m && s.params.reasoning.is_some());
                if s.eligible(reasoning) == 0 {
                    bail!(
                        "{m}: no OpenRouter endpoint meets the routing (8-bit or better, structured output{}); pick another model or reasoning setting",
                        if reasoning { ", reasoning" } else { "" }
                    );
                }
            }
            Some(s)
        };
        let (seed, note) = if opts.dry_run {
            (Some(opts.seed), "dry run: no provider".to_owned())
        } else {
            systems::seed_decision(
                opts.seed_mode,
                opts.seed,
                &systems::routing_for(m),
                snap.as_ref(),
            )
        };
        seeds.insert(m.clone(), seed);
        seed_notes.insert(m.clone(), note);
        if let Some(s) = snap {
            endpoints.insert(m.clone(), s);
        }
    }
    let plans: Vec<(SystemSpec, ModelChoice)> = plans
        .into_iter()
        .map(|(mut s, c)| {
            s.params.seed = seeds.get(&s.writer_model).copied().flatten();
            (s, c)
        })
        .collect();

    let clients = if opts.dry_run {
        let fake: Arc<dyn TextModel> = Arc::new(crate::dryrun::ScriptedModel);
        Clients {
            pipeline: fake.clone(),
            baseline: fake.clone(),
            judge: fake,
        }
    } else {
        build_clients(root, &models, opts.judge.as_deref(), &seeds)?
    };

    let started = SystemTime::now();
    let (git_commit, git_dirty) = meta::git_state(root);
    let short = git_commit
        .as_deref()
        .map_or("nogit".to_owned(), |s| s[..s.len().min(7)].to_owned());
    let label = opts.name.clone().unwrap_or_else(|| {
        if opts.models.len() == 1 {
            slug(&opts.models[0].model)
        } else {
            format!("{}-models", opts.models.len())
        }
    });
    let run_id = format!(
        "{}-{}{}-{short}",
        meta::compact(started),
        if opts.dry_run { "dryrun-" } else { "" },
        slug(&label)
    );
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
    let mut case_counts = BTreeMap::new();
    for c in &selected {
        *case_counts
            .entry(c.case.category.as_str().to_owned())
            .or_insert(0) += 1;
    }
    let rel = |p: &Path| {
        p.strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let judge_spec = opts.judge.as_ref().map(|j| JudgeSpec {
        model: j.clone(),
        reasoning: opts.judge_reasoning,
        temperature: judge::JUDGE_TEMPERATURE,
        max_tokens: judge::JUDGE_MAX_TOKENS,
        prompt_version: judge::version(),
        routing: systems::routing_for(j),
        seed: seeds.get(j).copied().flatten(),
    });
    let mut manifest = Manifest {
        format: FORMAT,
        run_id: run_id.clone(),
        name: opts.name.clone(),
        started_at: meta::rfc3339(started),
        finished_at: None,
        route: if opts.dry_run {
            crate::dryrun::DRY_RUN_ROUTE.into()
        } else {
            ROUTE.into()
        },
        systems: plans.iter().map(|(s, _)| s.clone()).collect(),
        judge: judge_spec,
        writer_model: String::new(),
        reviewer_model: String::new(),
        models_returned: vec![],
        provider_routes: vec![],
        upstream_provider: None,
        params: None,
        prompt_versions: {
            let mut v = prompt_versions();
            if opts.baseline {
                v.push(baseline::version());
            }
            if opts.judge.is_some() {
                v.push(judge::version());
            }
            v
        },
        scorer_version: score::SCORER_VERSION.into(),
        visible_scorer_version: visible::VISIBLE_SCORER_VERSION.into(),
        split: opts.split,
        cases_dir: Some(rel(&opts.cases_dir)),
        final_run: opts.final_run,
        commitment,
        case_filter: opts.patterns.clone(),
        dataset_sha256: hex::encode(Sha256::digest(dataset.as_bytes())),
        cases: entries,
        case_counts,
        git_commit,
        git_dirty,
        rustc: meta::rustc_version(root),
        max_cost_usd: opts.max_cost_usd,
        total_cost_usd: 0.0,
        judge_cost_usd: 0.0,
        stopped_early: None,
        seed: Some(opts.seed),
        seed_mode: Some(opts.seed_mode),
        seed_notes,
        concurrency: opts.concurrency.max(1),
        endpoints,
        dry_run: opts.dry_run,
        command: opts.command.clone(),
        config_file: opts.config_file.as_deref().map(rel),
        config_sha256: opts
            .config_file
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| hex::encode(Sha256::digest(b))),
    };
    write_manifest(&dir, &manifest)?;

    // Interleave systems per case so both see the same provider conditions.
    let jobs: Vec<(usize, &LoadedCase, &SystemSpec, &ModelChoice)> = selected
        .iter()
        .flat_map(|c| plans.iter().map(move |(s, m)| (*c, s, m)))
        .enumerate()
        .map(|(i, (c, s, m))| (i, c, s, m))
        .collect();
    let total_jobs = jobs.len();
    let spent = Arc::new(Mutex::new(0.0f64));
    let skipped = Arc::new(Mutex::new(0usize));
    let ctx = JobContext {
        run_id: &run_id,
        kb,
        clients: &clients,
        judge: opts.judge.as_deref(),
        judge_reasoning: opts.judge_reasoning,
    };
    let mut stream = futures::stream::iter(jobs)
        .map(|(i, loaded, system, choice)| {
            let spent = spent.clone();
            let skipped = skipped.clone();
            let ctx = &ctx;
            async move {
                if *spent.lock().expect("spent lock") >= opts.max_cost_usd {
                    *skipped.lock().expect("skipped lock") += 1;
                    return None;
                }
                eprintln!(
                    "[{}/{total_jobs}] {} {} ({:?})",
                    i + 1,
                    system.label,
                    loaded.case.case_id,
                    loaded.case.task.kind
                );
                let result = run_job(ctx, loaded, system, choice).await;
                let cost = result.cost_usd.unwrap_or(0.0)
                    + result
                        .judgments
                        .iter()
                        .filter_map(|j| j.cost_usd)
                        .sum::<f64>();
                *spent.lock().expect("spent lock") += cost;
                Some(result)
            }
        })
        .buffer_unordered(opts.concurrency.max(1));

    let mut results = std::fs::File::create(dir.join("results.jsonl"))?;
    let mut returned: Vec<String> = Vec::new();
    let mut routes: Vec<String> = Vec::new();
    while let Some(next) = stream.next().await {
        let Some(result) = next else { continue };
        manifest.total_cost_usd += result.cost_usd.unwrap_or(0.0);
        manifest.judge_cost_usd += result
            .judgments
            .iter()
            .filter_map(|j| j.cost_usd)
            .sum::<f64>();
        for c in &result.calls {
            if let Some(m) = &c.model_returned
                && !returned.contains(m)
            {
                returned.push(m.clone());
            }
            if let Some(r) = &c.provider_route
                && !routes.contains(r)
            {
                routes.push(r.clone());
            }
        }
        let v = result.visible.clone().unwrap_or_default();
        eprintln!(
            "    {} {}: {} visible violations, {}/{} sections withheld, {:.1}s, ${:.4}{}",
            result.system,
            result.case_id,
            v.violation_count(),
            v.sections_withheld,
            v.sections_requested,
            result.elapsed_ms as f64 / 1000.0,
            result.cost_usd.unwrap_or(0.0),
            result
                .error
                .as_deref()
                .map(|e| format!(", error: {e}"))
                .unwrap_or_default()
        );
        serde_json::to_writer(&mut results, &result)?;
        results.write_all(b"\n")?;
        results.flush()?;
    }
    drop(stream);
    let skipped = *skipped.lock().expect("skipped lock");
    if skipped > 0 {
        manifest.stopped_early = Some(format!(
            "cost cap ${:.4} reached; {skipped} of {total_jobs} presentations not started",
            opts.max_cost_usd
        ));
    }
    returned.sort();
    routes.sort();
    manifest.models_returned = returned;
    manifest.provider_routes = routes;
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

struct JobContext<'a> {
    run_id: &'a str,
    kb: &'a KnowledgeBase,
    clients: &'a Clients,
    judge: Option<&'a str>,
    judge_reasoning: Option<ReasoningEffort>,
}

/// The baseline's single call, with the pipeline's three transport retries.
async fn baseline_call(
    model: &Arc<dyn TextModel>,
    request: TextRequest,
) -> Result<TextResponse, AiError> {
    let mut last = None;
    for attempt in 0..3u32 {
        match model.generate(request.clone()).await {
            Ok(r) => return Ok(r),
            Err(e @ AiError::NotAllowed { .. }) => return Err(e),
            Err(e) => {
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
            }
        }
    }
    Err(last.expect("at least one attempt"))
}

async fn run_job(
    ctx: &JobContext<'_>,
    loaded: &LoadedCase,
    system: &SystemSpec,
    choice: &ModelChoice,
) -> CaseResult {
    let case: &Case = &loaded.case;
    let kb = ctx.kb;
    let started = SystemTime::now();
    let clock = Instant::now();
    let progress = |_: Event| {};
    let mut error = None;
    let mut sections: Vec<SectionRecord> = Vec::new();
    let reviewer_model = system.reviewer_model.clone().unwrap_or_default();
    let recorder = Arc::new(Recorder::new(match system.kind {
        SystemKind::Pipeline => ctx.clients.pipeline.clone(),
        SystemKind::Baseline => ctx.clients.baseline.clone(),
    }));

    let snapshot = if !case.task.kind.runnable() {
        error = Some(format!(
            "task kind {:?} is not runnable yet",
            case.task.kind
        ));
        None
    } else {
        match case.to_snapshot() {
            Err(e) => {
                error = Some(e.to_string());
                None
            }
            Ok(s) => Some(s),
        }
    };
    let level = case.source_facts.reading_level;
    let sheet = snapshot.as_ref().map(|s| FactSheet::build(s, kb));
    if let (Some(snapshot), Some(sheet)) = (&snapshot, &sheet) {
        match system.kind {
            SystemKind::Pipeline => {
                let models = Models {
                    writer: recorder.clone(),
                    writer_model: system.writer_model.clone(),
                    reviewer: recorder.clone(),
                    reviewer_model: reviewer_model.clone(),
                    reasoning: choice.reasoning,
                };
                let results = if case.task.kind == TaskKind::Presentation {
                    pipeline::generate(snapshot, kb, &models, level, &progress)
                        .await
                        .sections
                } else {
                    let mut out = Vec::new();
                    for kind in score::requested_sections(case, sheet) {
                        out.push(
                            pipeline::section(&kind, sheet, kb, &models, level, &progress).await,
                        );
                    }
                    out
                };
                sections = results.iter().map(SectionRecord::from_result).collect();
            }
            SystemKind::Baseline => {
                let requested = score::requested_sections(case, sheet);
                let request = TextRequest {
                    model: system.writer_model.clone(),
                    messages: vec![
                        Message {
                            role: Role::System,
                            content: baseline::BASELINE_PROMPT.into(),
                        },
                        Message {
                            role: Role::User,
                            content: baseline::user_message(sheet, level, &requested),
                        },
                    ],
                    response_schema: Some(baseline::response_schema(&requested)),
                    max_tokens: system.params.max_tokens,
                    temperature: system.params.temperature,
                    reasoning: choice.reasoning,
                };
                let rec: Arc<dyn TextModel> = recorder.clone();
                match baseline_call(&rec, request).await {
                    Ok(reply) => {
                        sections = baseline::split_reply(&reply.content, &system.writer_model);
                    }
                    Err(e) => error = Some(format!("baseline call failed: {e}")),
                }
            }
        }
    }
    let elapsed_ms = clock.elapsed().as_millis() as u64;

    let (metrics, visible) = if error.is_none() {
        (
            score::score(case, kb, &sections),
            Some(visible::score_visible(case, kb, system.kind, &sections)),
        )
    } else {
        (Vec::new(), None)
    };

    // Judge every patient-visible requested section, one call each.
    let mut judgments = Vec::new();
    if let (Some(judge_model), Some(sheet), None) = (ctx.judge, &sheet, &error) {
        let shown: Vec<&SectionRecord> = sections
            .iter()
            .filter(|s| {
                s.output.is_some()
                    && (system.kind == SystemKind::Baseline || s.state == score::State::Passed)
            })
            .collect();
        judgments = futures::future::join_all(shown.into_iter().map(|s| {
            judge::judge_section(
                &ctx.clients.judge,
                judge_model,
                ctx.judge_reasoning,
                s,
                sheet,
                kb,
                level,
            )
        }))
        .await;
    }

    let calls = recorder.take();
    let mut models_returned: Vec<String> = Vec::new();
    let mut routes: Vec<String> = Vec::new();
    for c in &calls {
        if let Some(m) = &c.model_returned
            && !models_returned.contains(m)
        {
            models_returned.push(m.clone());
        }
        if let Some(r) = &c.provider_route
            && !routes.contains(r)
        {
            routes.push(r.clone());
        }
    }
    let reported: Vec<f64> = calls.iter().filter_map(|c| c.cost_usd).collect();
    let all_reported = calls.iter().filter(|c| c.error.is_none()).count() == reported.len();
    CaseResult {
        run_id: ctx.run_id.into(),
        system: system.label.clone(),
        system_kind: system.kind,
        case_id: case.case_id.clone(),
        case_sha256: loaded.sha256.clone(),
        plan_ref: case.plan_ref.clone(),
        split: case.split,
        category: case.category,
        task_kind: case.task.kind,
        route: ROUTE.into(),
        writer_model: system.writer_model.clone(),
        reviewer_model,
        models_returned,
        upstream_provider: (!routes.is_empty()).then(|| routes.join(",")),
        params: system.params.clone(),
        prompt_versions: system.prompt_versions.clone(),
        started_at: meta::rfc3339(started),
        elapsed_ms,
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
        sections,
        metrics,
        visible,
        judgments,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_commitment_root() {
        let dir = std::env::temp_dir().join(format!("ce-commit-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("eval/commitments")).unwrap();
        assert!(read_commitment(&dir).is_none());
        let root = "ab".repeat(32);
        std::fs::write(
            dir.join(COMMITMENT_FILE),
            format!("case T-1 {}\nroot: {root}\n", "cd".repeat(32)),
        )
        .unwrap();
        let c = read_commitment(&dir).unwrap();
        assert_eq!(c.root.as_deref(), Some(root.as_str()));
        assert_eq!(c.file_sha256.len(), 64);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verifies_cases_against_salted_hashes() {
        let dir = std::env::temp_dir().join(format!("ce-verify-{}", std::process::id()));
        let cases_dir = dir.join("eval/heldout/cases/treatment_plan");
        std::fs::create_dir_all(&cases_dir).unwrap();
        std::fs::create_dir_all(dir.join("eval/commitments")).unwrap();
        let file = cases_dir.join("T-900.toml");
        std::fs::write(&file, crate::case::tests_support::MINIMAL).unwrap();
        let loaded = crate::case::load_file(&file).unwrap();
        let salt = "11".repeat(32);
        let mut h = Sha256::new();
        h.update(salt.as_bytes());
        h.update(crate::case::tests_support::MINIMAL.as_bytes());
        let case_hash = hex::encode(h.finalize());
        let root_hash = hex::encode(Sha256::digest(case_hash.as_bytes()));
        std::fs::write(
            dir.join("eval/heldout/salts.txt"),
            format!("T-900 {salt}\n"),
        )
        .unwrap();
        let commitment = format!(
            "Root hash\n{root_hash}\n\nPer-case hashes\nT-900 cases/treatment_plan/T-900.toml {case_hash}\n"
        );
        std::fs::write(dir.join(COMMITMENT_FILE), &commitment).unwrap();
        let mut c = read_commitment(&dir).unwrap();
        assert_eq!(c.root.as_deref(), Some(root_hash.as_str()));
        let salts = dir.join("eval/heldout/salts.txt");
        verify_commitment(&dir, &mut c, &[&loaded], &salts);
        assert_eq!(c.verified, Some(true), "{:?}", c.verification);

        std::fs::write(
            &file,
            format!("{}\n# edited", crate::case::tests_support::MINIMAL),
        )
        .unwrap();
        let mut c = read_commitment(&dir).unwrap();
        verify_commitment(&dir, &mut c, &[&loaded], &salts);
        assert_eq!(c.verified, Some(false));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn legacy_results_get_a_system_label() {
        let line = include_str!("../tests/fixtures/legacy_result.json");
        let r: CaseResult = serde_json::from_str(line).unwrap();
        assert_eq!(r.system_label(), "pipeline:test/model");
        assert!(r.visible.is_none());
    }
}
