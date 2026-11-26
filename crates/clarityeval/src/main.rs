//! The `clarityeval` command line. See docs/CLARITYEVAL.md, "Running the benchmark".

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use clarityeval::case::{self, LoadedCase, Split};
use clarityeval::{lint, read_canary, repo_root, report, runner};
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

#[derive(Subcommand)]
enum Command {
    /// Load and lint every case (dev, plus held-out when present locally).
    Validate {
        #[arg(long, value_enum)]
        split: Option<SplitArg>,
    },
    /// Run the real pipeline on cases through OpenRouter. Costs money.
    Run {
        #[arg(long, value_enum, default_value = "dev")]
        split: SplitArg,
        /// Exact OpenRouter model id for the writer (and reviewer unless set).
        #[arg(long)]
        model: String,
        #[arg(long)]
        reviewer_model: Option<String>,
        #[arg(long, value_enum, default_value = "high")]
        reasoning: Reasoning,
        /// Case id or path patterns with * and ?, comma separated.
        #[arg(long, value_delimiter = ',')]
        cases: Vec<String>,
        /// Output directory. Defaults to eval/runs/<run id>/ (git-ignored).
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        limit: Option<usize>,
        /// Stop starting new cases once this much has been spent.
        #[arg(long, default_value_t = 1.0)]
        max_cost_usd: f64,
    },
    /// Summarize a run directory and write report.md into it.
    Report { run_dir: PathBuf },
}

fn load(root: &Path, split: Split) -> Result<Vec<LoadedCase>> {
    let mut ok = Vec::new();
    let mut errors = Vec::new();
    for r in case::load_dir(&case::split_dir(root, split))? {
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.root.unwrap_or_else(repo_root);
    let kb = KnowledgeBase::load(&root.join("knowledge"))?;

    match cli.command {
        Command::Validate { split } => {
            let splits = match split {
                Some(s) => vec![Split::from(s)],
                None => vec![Split::Dev, Split::Heldout],
            };
            let canary = read_canary(&root)?;
            let mut all = Vec::new();
            let mut parse_errors = Vec::new();
            for s in splits {
                for r in case::load_dir(&case::split_dir(&root, s))? {
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
            split,
            model,
            reviewer_model,
            reasoning,
            cases,
            out,
            limit,
            max_cost_usd,
        } => {
            let split = Split::from(split);
            let loaded = load(&root, split)?;
            let canary = read_canary(&root)?;
            let findings = lint::check_all(&loaded, &kb, &canary);
            if !findings.is_empty() {
                for f in &findings {
                    eprintln!("error: {}: {}", f.file, f.message);
                }
                bail!("run `clarityeval validate` and fix the cases first");
            }
            let opts = runner::RunOptions {
                split,
                model,
                reviewer_model,
                reasoning: match reasoning {
                    Reasoning::None => None,
                    Reasoning::Low => Some(ReasoningEffort::Low),
                    Reasoning::Medium => Some(ReasoningEffort::Medium),
                    Reasoning::High => Some(ReasoningEffort::High),
                },
                patterns: cases,
                out,
                limit,
                max_cost_usd,
            };
            let dir = runner::run(&root, &loaded, &kb, &opts).await?;
            println!("wrote {}", dir.display());
            Ok(())
        }
        Command::Report { run_dir } => {
            let (manifest, mut results) = report::load_results(&run_dir)?;
            let cases = load(&root, manifest.split).unwrap_or_default();
            let changed = report::rescore(&mut results, &cases, &kb);
            let md = report::render(&manifest, &results, &changed);
            std::fs::write(run_dir.join("report.md"), &md)?;
            println!("{md}");
            Ok(())
        }
    }
}
