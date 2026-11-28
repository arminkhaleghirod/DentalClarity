//! Startup and the serve loop, shared by the console and the Windows service.

use crate::config::Config;
use crate::generation::Generation;
use crate::jobs::{self, Jobs};
use crate::limits::Limits;
use crate::narration::Narration;
use crate::secrets::SecretStore;
use crate::{AppState, clinical, router, sessions, settings, storage, tls};
use anyhow::{Context, Result};
use dc_core::Mode;
use dc_knowledge::KnowledgeBase;
use dc_pms::synthetic::SyntheticConnector;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// How the server was started.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    /// Running as a Windows service, with no console. The first-time setup
    /// code is written to `<data_dir>/setup-code.txt` instead of printed.
    pub service: bool,
}

/// File that holds the setup code when there is no console.
pub const SETUP_CODE_FILE: &str = "setup-code.txt";

/// Runs the server until `shutdown` completes.
pub async fn run(
    config: Config,
    options: RunOptions,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let db = storage::open(&config.data_dir).await?;
    let secrets = SecretStore::new(&config.data_dir);
    // Until the Open Dental connector lands (M2), both modes read the
    // synthetic practice. Clinical mode with synthetic data is a training setup.
    let pms =
        SyntheticConnector::from_path(&config.fixture).context("loading synthetic practice")?;
    let kb = KnowledgeBase::load(&config.knowledge_dir).context("loading the knowledge base")?;
    tracing::info!(entries = kb.entries().count(), "knowledge base loaded");

    let (generation, narration) = match config.mode {
        Mode::Demo => {
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
            (generation, narration)
        }
        Mode::Clinical => match clinical::from_env(&db, &config.data_dir, &secrets).await? {
            Some(p) => (Some(p.generation), p.narration),
            None => (None, Narration::unavailable(Mode::Clinical, db.clone())),
        },
    };

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

    let setup_file = config.data_dir.join(SETUP_CODE_FILE);
    let setup_code = if config.mode == Mode::Clinical && !staff_account_exists(&db).await? {
        let code = new_setup_code();
        if options.service {
            std::fs::write(&setup_file, format!("{code}\n"))
                .with_context(|| format!("writing {}", setup_file.display()))?;
            tracing::warn!(
                path = %setup_file.display(),
                "setup required; the one-time setup code is in this file"
            );
        } else {
            // Printed and logged on this machine only; the log never leaves it.
            println!("\n  First-time setup code: {code}");
            println!("  Enter it on the setup screen to create the staff account.\n");
            tracing::warn!("setup required; the one-time setup code is printed above");
        }
        Some(code)
    } else {
        if setup_file.exists() {
            let _ = std::fs::remove_file(&setup_file);
        }
        None
    };

    let tls_material = match &config.tls {
        Some(source) => Some(tls::load(
            source,
            &config.data_dir,
            &secrets,
            config.bind,
            &config.http.public_origin,
        )?),
        None => None,
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

    let app = router(state).into_make_service_with_connect_info::<SocketAddr>();
    match tls_material {
        Some(material) => {
            let rustls = rustls_config(&material).await?;
            let handle = axum_server::Handle::new();
            let stopper = handle.clone();
            tokio::spawn(async move {
                shutdown.await;
                stopper.graceful_shutdown(Some(Duration::from_secs(10)));
            });
            tracing::info!(
                mode = %config.mode,
                addr = %config.bind,
                origin = %config.http.public_origin,
                "DentalClarity listening (HTTPS)"
            );
            axum_server::bind_rustls(config.bind, rustls)
                .handle(handle)
                .serve(app)
                .await
                .with_context(|| format!("serving HTTPS on {}", config.bind))?;
        }
        None => {
            let listener = tokio::net::TcpListener::bind(config.bind)
                .await
                .with_context(|| format!("binding {}", config.bind))?;
            tracing::info!(
                mode = %config.mode,
                addr = %config.bind,
                origin = %config.http.public_origin,
                "DentalClarity listening"
            );
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown)
                .await?;
        }
    }
    Ok(())
}

/// A rustls server configuration for `material`.
pub async fn rustls_config(
    material: &tls::TlsMaterial,
) -> Result<axum_server::tls_rustls::RustlsConfig> {
    // More than one rustls crypto provider may be compiled in; pick one.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    axum_server::tls_rustls::RustlsConfig::from_pem(
        material.chain_pem.as_bytes().to_vec(),
        material.key_pem.as_bytes().to_vec(),
    )
    .await
    .context("loading the TLS certificate and key")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::{TlsSource, ca_path};

    #[tokio::test]
    async fn serves_https_that_trusts_only_the_local_ca() {
        let dir =
            std::env::temp_dir().join(format!("dc-https-{}", crate::credentials::new_token()));
        let secrets = SecretStore::new(&dir);
        let source = TlsSource::LocalCa { hostnames: vec![] };
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let material = tls::load(&source, &dir, &secrets, bind, "https://localhost").unwrap();
        let config = rustls_config(&material).await.unwrap();

        let app = axum::Router::new().route("/", axum::routing::get(|| async { "ok" }));
        let handle = axum_server::Handle::new();
        let server = tokio::spawn(
            axum_server::bind_rustls(bind, config)
                .handle(handle.clone())
                .serve(app.into_make_service()),
        );
        let addr = handle.listening().await.expect("listening");

        let ca = std::fs::read(ca_path(&dir)).unwrap();
        let trusting = reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(&ca).unwrap())
            .build()
            .unwrap();
        let url = format!("https://localhost:{}/", addr.port());
        let body = trusting
            .get(&url)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "ok");
        let by_ip = format!("https://127.0.0.1:{}/", addr.port());
        assert!(trusting.get(&by_ip).send().await.is_ok());

        // A client without the CA refuses the certificate.
        let plain = reqwest::Client::builder().build().unwrap();
        assert!(plain.get(&url).send().await.is_err());

        handle.shutdown();
        let _ = server.await;
        let _ = std::fs::remove_dir_all(dir);
    }
}
