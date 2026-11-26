//! The `clarityeval` command line. See docs/CLARITYEVAL.md, "Running the benchmark".

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use clarityeval::case::{self, LoadedCase, Split};
use clarityeval::report::{self, Bootstrap};
use clarityeval::systems::{self, RunConfig, SeedMode};
use clarityeval::{lint, read_canary, repo_root, runner};
use dc_ai::ReasoningEffort;
use dc_knowledge::KnowledgeBase;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "clarityeval", about = "ClarityEval benchmark runner")]
struct Cli {
    /// Repository root. Defaults to the nearest ancestor holding eval/canary.txt.
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum SplitArg {
    Dev,
    Heldout,
}

impl From<SplitArg> for Split {
    fn from(s: SplitArg) -> Split {
        match s {
            SplitArg::Dev => Split::Dev,
            SplitArg::Heldout => Split::Heldout,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Reasoning {
    None,
    Low,
    Medium,
    High,
}

impl Reasoning {
    fn effort(self) -> Option<ReasoningEffort> {
        match self {
            Reasoning::None => None,
            Reasoning::Low => Some(ReasoningEffort::Low),
            Reasoning::Medium => Some(ReasoningEffort::Medium),
            Reasoning::High => Some(ReasoningEffort::High),
        }
    }
}

// Parsed once per process, so the size of the `run` variant does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    /// Load and lint every case (dev, plus held-out when present locally).
    Validate {
        #[arg(long, value_enum)]
        split: Option<SplitArg>,
        /// Lint this directory instead of the split's default.
        #[arg(long)]
        cases_dir: Option<PathBuf>,
    },
    /// Run systems on cases through OpenRouter (or offline with --dry-run).
    Run {
        /// TOML run config (models, baseline, judge, seeds). Command-line flags win.
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, value_enum)]
        split: Option<SplitArg>,
        /// Read cases from here instead of eval/cases/dev or eval/heldout/cases.
        #[arg(long)]
        cases_dir: Option<PathBuf>,
        /// Required for the held-out split: confirms this is a final run.
        #[arg(long)]
        r#final: bool,
        /// Run held-out without eval/commitments/heldout-v1.txt (the report flags it).
        #[arg(long)]
        allow_missing_commitment: bool,
        /// Exact OpenRouter model ids, comma separated. Each runs the full pipeline.
        #[arg(long, alias = "model", value_delimiter = ',')]
        models: Vec<String>,
        /// Reviewer model for every pipeline system (defaults to the writer).
        #[arg(long, alias = "reviewer-model")]
        reviewer: Option<String>,
        /// Also run the plain-prompt baseline for every model.
        #[arg(long)]
        baseline: bool,
        /// LLM judge model: one call per patient-visible section.
        #[arg(long)]
        judge: Option<String>,
        #[arg(long, value_enum)]
        judge_reasoning: Option<Reasoning>,
        #[arg(long, value_enum)]
        reasoning: Option<Reasoning>,
        /// Sampling seed sent where the route supports it.
        #[arg(long)]
        seed: Option<u64>,
        #[arg(long, value_enum)]
        seed_mode: Option<SeedMode>,
        /// Presentations generated at the same time.
        #[arg(long)]
        concurrency: Option<usize>,
        /// Case id or path patterns with * and ?, comma separated.
        #[arg(long, value_delimiter = ',')]
        cases: Vec<String>,
        /// Output directory. Defaults to eval/runs/<run id>/ (git-ignored).
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        limit: Option<usize>,
        /// Stop starting new presentations once this much has been spent.
        #[arg(long)]
        max_cost_usd: Option<f64>,
        /// Bootstrap resamples for the report.
        #[arg(long)]
        bootstrap: Option<usize>,
        /// Run name used in the run id.
        #[arg(long)]
        name: Option<String>,
        /// Offline: a scripted model answers every call. No key, no cost.
        #[arg(long)]
        dry_run: bool,
    },
    /// Re-score a run directory and write report.md and results.json into it.
    Report {
        run_dir: PathBuf,
        /// Case files to re-score against (defaults to the run's cases directory).
        #[arg(long)]
        cases_dir: Option<PathBuf>,
        #[arg(long, default_value_t = clarityeval::stats::BOOTSTRAP_RESAMPLES)]
        bootstrap: usize,
        #[arg(long, default_value_t = clarityeval::stats::BOOTSTRAP_SEED)]
        bootstrap_seed: u64,
    },
}

fn load_from(dir: &Path) -> Result<Vec<LoadedCase>> {
    let mut ok = Vec::new();
    let mut errors = Vec::new();
    for r in case::load_dir(dir)? {
        match r {
            Ok(c) => ok.push(c),
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    if !errors.is_empty() {
        bail!("cases failed to parse:\n{}", errors.join("\n"));
    }
    Ok(ok)
}

fn resolve(root: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.root.clone().unwrap_or_else(repo_root);
    let kb = KnowledgeBase::load(&root.join("knowledge"))?;

    match cli.command {
        Command::Validate { split, cases_dir } => {
            let dirs: Vec<PathBuf> = match (&cases_dir, split) {
                (Some(d), _) => vec![resolve(&root, d)],
                (None, Some(s)) => vec![case::split_dir(&root, Split::from(s))],
                (None, None) => vec![
                    case::split_dir(&root, Split::Dev),
                    case::split_dir(&root, Split::Heldout),
                ],
            };
            let canary = read_canary(&root)?;
            let mut all = Vec::new();
            let mut parse_errors = Vec::new();
            for d in dirs {
                for r in case::load_dir(&d)? {
                    match r {
                        Ok(c) => all.push(c),
                        Err(e) => parse_errors.push(format!("{e:#}")),
                    }
                }
            }
            let mut findings = lint::check_all(&all, &kb, &canary);
            if lint::heldout_in_tree(&root.join("eval/cases")) {
                findings.push(lint::Finding {
                    file: "eval/cases/heldout".into(),
                    message: "held-out cases belong in eval/heldout/cases (git-ignored)".into(),
                });
            }
            for e in &parse_errors {
                println!("error: {e}");
            }
            for f in &findings {
                println!("error: {}: {}", f.file, f.message);
            }
            let mut per_cat = std::collections::BTreeMap::<String, usize>::new();
            for c in &all {
                *per_cat
                    .entry(format!(
                        "{}/{}",
                        c.case.split.as_str(),
                        c.case.category.as_str()
                    ))
                    .or_default() += 1;
            }
            for (k, v) in &per_cat {
                println!("{k}: {v}");
            }
            if parse_errors.is_empty() && findings.is_empty() {
                println!("{} cases valid", all.len());
                Ok(())
            } else {
                bail!(
                    "{} problems in {} cases",
                    parse_errors.len() + findings.len(),
                    all.len() + parse_errors.len()
                )
            }
        }
        Command::Run {
            config,
            split,
            cases_dir,
            r#final,
            allow_missing_commitment,
            models,
            reviewer,
            baseline,
            judge,
            judge_reasoning,
            reasoning,
            seed,
            seed_mode,
            concurrency,
            cases,
            out,
            limit,
            max_cost_usd,
            bootstrap,
            name,
            dry_run,
        } => {
            let cfg = match &config {
                Some(p) => RunConfig::load(&resolve(&root, p))?,
                None => RunConfig::default(),
            };
            let split = match (split, cfg.split.as_deref()) {
                (Some(s), _) => Split::from(s),
                (None, Some("heldout")) => Split::Heldout,
                (None, Some("dev") | None) => Split::Dev,
                (None, Some(other)) => bail!("config split must be dev or heldout, got {other:?}"),
            };
            let reasoning = match (reasoning, &cfg.reasoning) {
                (Some(r), _) => r.effort(),
                (None, Some(r)) => systems::parse_reasoning(r)?,
                (None, None) => Some(ReasoningEffort::High),
            };
            let judge_reasoning = match (judge_reasoning, &cfg.judge_reasoning) {
                (Some(r), _) => r.effort(),
                (None, Some(r)) => systems::parse_reasoning(r)?,
                (None, None) => None,
            };
            let reviewer = reviewer.or(cfg.reviewer.clone());
            let choices: Vec<runner::ModelChoice> = if !models.is_empty() {
                models
                    .iter()
                    .map(|m| runner::ModelChoice {
                        model: m.trim().to_owned(),
                        reviewer: reviewer.clone(),
                        reasoning,
                    })
                    .collect()
            } else {
                cfg.systems
                    .iter()
                    .map(|s| {
                        Ok(runner::ModelChoice {
                            model: s.model.clone(),
                            reviewer: s.reviewer.clone().or(reviewer.clone()),
                            reasoning: match &s.reasoning {
                                Some(r) => systems::parse_reasoning(r)?,
                                None => reasoning,
                            },
                        })
                    })
                    .collect::<Result<_>>()?
            };
            let cases_dir = match (&cases_dir, &cfg.cases_dir) {
                (Some(d), _) => resolve(&root, d),
                (None, Some(d)) => resolve(&root, Path::new(d)),
                (None, None) => case::split_dir(&root, split),
            };
            let loaded = load_from(&cases_dir)?;
            let canary = read_canary(&root)?;
            let findings = lint::check_all(&loaded, &kb, &canary);
            if !findings.is_empty() {
                for f in &findings {
                    eprintln!("error: {}: {}", f.file, f.message);
                }
                bail!("run `clarityeval validate` and fix the cases first");
            }
            let opts = runner::RunOptions {
                name: name.or(cfg.name.clone()),
                split,
                cases_dir,
                final_run: r#final,
                allow_missing_commitment,
                models: choices,
                baseline: baseline || cfg.baseline,
                judge: judge.or(cfg.judge.clone()),
                judge_reasoning,
                seed: seed
                    .or(cfg.seed)
                    .unwrap_or(clarityeval::stats::BOOTSTRAP_SEED),
                seed_mode: seed_mode.or(cfg.seed_mode).unwrap_or_default(),
                concurrency: concurrency.or(cfg.concurrency).unwrap_or(4),
                patterns: if cases.is_empty() {
                    cfg.cases.clone()
                } else {
                    cases
                },
                out,
                limit: limit.or(cfg.limit),
                max_cost_usd: max_cost_usd.or(cfg.max_cost_usd).unwrap_or(1.0),
                dry_run,
                bootstrap: bootstrap
                    .or(cfg.bootstrap)
                    .unwrap_or(clarityeval::stats::BOOTSTRAP_RESAMPLES),
                command: std::env::args().skip(1).collect(),
                config_file: config.map(|p| resolve(&root, &p)),
            };
            let dir = runner::run(&root, &loaded, &kb, &opts).await?;
            let (manifest, results) = report::load_results(&dir)?;
            let b = Bootstrap {
                resamples: opts.bootstrap,
                ..Bootstrap::default()
            };
            let summary = report::summarize(&manifest, &results, &[], b);
            let md = report::write(&dir, &summary, &results)?;
            println!("{md}");
            println!("wrote {}", dir.display());
            Ok(())
        }
        Command::Report {
            run_dir,
            cases_dir,
            bootstrap,
            bootstrap_seed,
        } => {
            let run_dir = resolve(&root, &run_dir);
            let (manifest, mut results) = report::load_results(&run_dir)?;
            let dir = match (&cases_dir, &manifest.cases_dir) {
                (Some(d), _) => resolve(&root, d),
                (None, Some(d)) => resolve(&root, Path::new(d)),
                (None, None) => case::split_dir(&root, manifest.split),
            };
            let cases = load_from(&dir).unwrap_or_default();
            let changed = report::rescore(&mut results, &cases, &kb);
            let b = Bootstrap {
                resamples: bootstrap,
                seed: bootstrap_seed,
            };
            let summary = report::summarize(&manifest, &results, &changed, b);
            let md = report::write(&run_dir, &summary, &results)?;
            println!("{md}");
            Ok(())
        }
    }
}
