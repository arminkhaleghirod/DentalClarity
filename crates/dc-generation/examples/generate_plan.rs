//! Runs the pipeline once on a synthetic plan against OpenRouter.
//!
//! ```sh
//! cargo run -p dc-generation --example generate_plan -- tp-10482-1 standard out.json
//! ```
//!
//! Reads `OPENROUTER_API_KEY` from the environment or `.env`. Synthetic data only.

use dc_ai::openrouter::OpenRouter;
use dc_ai::{ProviderPolicy, defaults};
use dc_core::Mode;
use dc_core::content::ReadingLevel;
use dc_core::plan::PlanKey;
use dc_generation::{Event, Models, generate};
use dc_knowledge::KnowledgeBase;
use dc_pms::PmsConnector;
use dc_pms::synthetic::SyntheticConnector;
use std::path::Path;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let plan = args.first().cloned().unwrap_or_else(|| "tp-10482-1".into());
    let level = match args.get(1).map(String::as_str) {
        Some("simple") => ReadingLevel::Simple,
        Some("detailed") => ReadingLevel::Detailed,
        _ => ReadingLevel::Standard,
    };
    let out = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "generated.json".into());

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pms = SyntheticConnector::from_path(&root.join("fixtures/synthetic/practice.toml"))?;
    let kb = KnowledgeBase::load(&root.join("knowledge"))?;
    let snapshot = pms.plan_snapshot(&PlanKey(plan)).await?;

    let key = std::env::var("OPENROUTER_API_KEY")?;
    let client = Arc::new(OpenRouter::new(ProviderPolicy::for_mode(Mode::Demo), key)?);
    let models = Models {
        writer: client.clone(),
        writer_model: defaults::DEMO_TEXT_MODEL.into(),
        reviewer: client,
        reviewer_model: defaults::DEMO_TEXT_MODEL.into(),
        reasoning: Some(defaults::DEMO_TEXT_REASONING),
    };
    let progress = |e: Event| eprintln!("{}", serde_json::to_string(&e).unwrap_or_default());
    let result = generate(&snapshot, &kb, &models, level, &progress).await;

    println!(
        "elapsed {:.1}s, cost ${:.4}",
        result.elapsed_ms as f64 / 1000.0,
        result.total_cost_usd()
    );
    for s in &result.sections {
        println!(
            "{:<16} {:?} attempts={} {:.1}s {}",
            s.key,
            s.state,
            s.attempts,
            s.elapsed_ms as f64 / 1000.0,
            s.reasons.join(" | ")
        );
        for line in &s.attempt_log {
            println!("    {line}");
        }
    }
    std::fs::write(&out, serde_json::to_string_pretty(&result.sections)?)?;
    println!("wrote {out}");
    Ok(())
}
