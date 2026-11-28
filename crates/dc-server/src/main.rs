use anyhow::{Context, Result};
use dc_core::Mode;
use dc_knowledge::KnowledgeBase;
use dc_pms::synthetic::SyntheticConnector;
use dc_server::config::Config;
use dc_server::generation::Generation;
use dc_server::jobs::{self, Jobs};
use dc_server::limits::Limits;
use dc_server::narration::Narration;
use dc_server::{AppState, router, sessions, settings, storage};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Values already set in the environment win over `.env`.
    let dotenv = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    if let Ok(path) = &dotenv {
        tracing::info!(path = %path.display(), "loaded settings file");
    }
    let config = Config::from_env()?;
    let db = storage::open(&config.data_dir).await?;
    // Until the Open Dental connector lands (M2), both modes read the
    // synthetic practice. Clinical mode with synthetic data is a training setup.
    let pms =
        SyntheticConnector::from_path(&config.fixture).context("loading synthetic practice")?;
    let kb = KnowledgeBase::load(&config.knowledge_dir).context("loading the knowledge base")?;
    tracing::info!(entries = kb.entries().count(), "knowledge base loaded");
    let generation = Generation::from_env(
        config.mode,
        &db,
        config.text_model.as_deref(),
        config.demo_monthly_budget_usd,
    )?;
    let narration = Narration::from_env(
        config.mode,
        &db,
        &config.data_dir,
        generation.as_ref(),
        config.demo_monthly_budget_usd,
    )?;

    // Sessions from one mode never carry into the other.
    let mode = config.mode.to_string();
    if settings::get(&db, settings::MODE).await?.as_deref() != Some(mode.as_str()) {
        let ended = sessions::delete_all(&db).await?;
        settings::set(&db, settings::MODE, &mode).await?;
        tracing::info!(
            ended,
            mode,
            "mode changed since last start; all sessions ended"
        );
    }

    let setup_code = if config.mode == Mode::Clinical && !staff_account_exists(&db).await? {
        let code = new_setup_code();
        // Printed and logged on this machine only; the log never leaves it.
        println!(
            "
  First-time setup code: {code}"
        );
        println!(
            "  Enter it on the setup screen to create the staff account.
"
        );
        tracing::warn!("setup required; the one-time setup code is printed above");
        Some(code)
    } else {
        None
    };

    let state = AppState {
        mode: config.mode,
        http: Arc::new(config.http.clone()),
        db: db.clone(),
        default_practice_name: pms.practice_name().to_owned(),
        pms: Arc::new(pms),
        codes: Arc::new(kb.codes.clone()),
        kb: Arc::new(kb),
        generation: generation.map(Arc::new),
        jobs: Arc::new(Jobs::default()),
        limits: Arc::new(Limits::default()),
        setup_code: Arc::new(std::sync::Mutex::new(setup_code)),
        narration: Arc::new(narration),
    };

    // Jobs interrupted by a stop are re-run from the start.
    let resumed = jobs::resume_pending(&state).await?;
    if resumed > 0 {
        tracing::info!(resumed, "re-running interrupted generation jobs");
    }

    let demo = config.mode == Mode::Demo;
    let narration = state.narration.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(600));
        loop {
            tick.tick().await;
            if let Err(err) = sessions::purge_expired(&db).await {
                tracing::warn!(%err, "session purge failed");
            }
            if demo {
                match jobs::purge_demo_data(&db).await {
                    Ok(0) => {}
                    Ok(removed) => tracing::info!(removed, "purged demo presentations"),
                    Err(err) => tracing::warn!(%err, "demo data purge failed"),
                }
            }
            match dc_server::api::purge_old_engagement(&db).await {
                Ok(0) => {}
                Ok(removed) => tracing::info!(removed, "deleted old engagement rows"),
                Err(err) => tracing::warn!(%err, "engagement retention cleanup failed"),
            }
            match narration.collect_garbage().await {
                Ok(report) if report == Default::default() => {}
                Ok(report) => tracing::info!(
                    stale = report.stale,
                    over_cap = report.over_cap,
                    orphans = report.orphans,
                    "cleaned the narration cache"
                ),
                Err(err) => tracing::warn!(%err, "narration cache cleanup failed"),
            }
        }
    });

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    tracing::info!(
        mode = %config.mode,
        addr = %config.bind,
        origin = %config.http.public_origin,
        "DentalClarity listening"
    );

    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}

async fn staff_account_exists(db: &sqlx::SqlitePool) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM staff_account WHERE id = 1")
        .fetch_optional(db)
        .await?;
    Ok(row.is_some())
}

/// Twelve characters from an alphabet without look-alikes, grouped for reading
/// aloud, e.g. `7KQ2-MX9D-4TRB`. About 60 bits, and attempts are rate limited.
fn new_setup_code() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).expect("OS random source is available");
    let chars: Vec<char> = bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect();
    chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}
