use anyhow::{Context, Result, bail};
use dc_core::Mode;
use dc_knowledge::CodeMap;
use dc_pms::synthetic::SyntheticConnector;
use dc_server::config::Config;
use dc_server::{AppState, router, storage};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env()?;
    if config.mode == Mode::Clinical {
        bail!(
            "clinical mode is not available yet; the Open Dental connector arrives in milestone M2"
        );
    }

    let db = storage::open(&config.data_dir).await?;
    let pms =
        SyntheticConnector::from_path(&config.fixture).context("loading synthetic practice")?;
    let codes = CodeMap::from_path(&config.knowledge_dir.join("codes.toml"))?;

    let state = AppState {
        mode: config.mode,
        db,
        pms: Arc::new(pms),
        codes: Arc::new(codes),
    };
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    tracing::info!(mode = %config.mode, addr = %config.bind, "DentalClarity listening");

    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
