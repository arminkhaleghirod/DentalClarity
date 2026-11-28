//! `dc-bench`: the generation timing benchmark (docs/AI_PIPELINE.md,
//! measuring the 90 second target; docs/DECISIONS.md E38).
//!
//! ```text
//! cargo run -p dc-server --bin dc-bench -- [--runs 4] [--latency-ms 4000-15000]
//!     [--fixture fixtures/perf/typical.toml] [--knowledge knowledge]
//!     [--live] [--max-usd 2] [--json out.json]
//! ```
//!
//! Without `--live` a scripted model answers every call. With `--live` the
//! demo default models are called on OpenRouter with `OPENROUTER_API_KEY`
//! (and `DC_TEXT_MODEL` if set), and `--max-usd` caps the run's spend.

use anyhow::{Context, Result, bail};
use dc_core::Mode;
use dc_server::bench::{self, Models, Options};
use dc_server::generation::Generation;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();

    let mut options = Options {
        fixture: PathBuf::from("fixtures/perf/typical.toml"),
        knowledge: PathBuf::from("knowledge"),
        runs: 4,
        latency_ms: (0, 0),
    };
    let mut live = false;
    let mut max_usd = 2.0;
    let mut json_out: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--runs" => options.runs = value()?.parse().context("--runs")?,
            "--fixture" => options.fixture = value()?.into(),
            "--knowledge" => options.knowledge = value()?.into(),
            "--latency-ms" => {
                let v = value()?;
                let (lo, hi) = v.split_once('-').unwrap_or((&v, &v));
                options.latency_ms = (
                    lo.parse().context("--latency-ms")?,
                    hi.parse().context("--latency-ms")?,
                );
            }
            "--live" => live = true,
            "--max-usd" => max_usd = value()?.parse().context("--max-usd")?,
            "--json" => json_out = Some(value()?.into()),
            other => bail!("unknown argument {other}"),
        }
    }

    let models = if live {
        let text_model = std::env::var("DC_TEXT_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty());
        // Built once here so a missing key fails before any job starts.
        let probe = sqlx::SqlitePool::connect("sqlite::memory:").await?;
        if Generation::from_env(Mode::Demo, &probe, text_model.as_deref(), max_usd)?.is_none() {
            bail!("--live needs OPENROUTER_API_KEY");
        }
        Models::Configured(Box::new(move |db| {
            Generation::from_env(Mode::Demo, db, text_model.as_deref(), max_usd)
                .ok()
                .flatten()
                .expect("checked above")
        }))
    } else {
        Models::Scripted
    };

    let report = bench::run(&options, models).await?;
    println!(
        "{} model, {} runs of each plan in {}\n",
        if live { "Live" } else { "Scripted" },
        options.runs,
        options.fixture.display()
    );
    println!("{}", bench::render(&report));
    if let Some(path) = json_out {
        std::fs::write(&path, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}
