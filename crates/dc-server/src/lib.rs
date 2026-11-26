//! HTTP server: JSON API under `/api`, and the embedded web app for every other path.

pub mod api;
pub mod assets;
pub mod audit;
pub mod config;
pub mod credentials;
pub mod generation;
pub mod http;
pub mod jobs;
pub mod limits;
pub mod security;
pub mod sessions;
pub mod settings;
pub mod storage;

use axum::Router;
use axum::middleware::from_fn_with_state;
use config::HttpConfig;
use dc_core::Mode;
use dc_knowledge::{CodeMap, KnowledgeBase};
use dc_pms::PmsConnector;
use generation::Generation;
use jobs::Jobs;
use limits::Limits;
use sqlx::SqlitePool;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub mode: Mode,
    pub http: Arc<HttpConfig>,
    pub db: SqlitePool,
    pub pms: Arc<dyn PmsConnector>,
    /// Code map for names and categories. The same map as `kb.codes`.
    pub codes: Arc<CodeMap>,
    /// Code map plus the educational entries used by the generation pipeline.
    pub kb: Arc<KnowledgeBase>,
    /// Models for the generation pipeline. `None` when no provider is
    /// configured: clinical mode until M9, or demo mode without an API key.
    pub generation: Option<Arc<Generation>>,
    /// Generation job slots and the create lock.
    pub jobs: Arc<Jobs>,
    pub limits: Arc<Limits>,
    /// Shown before a practice name is configured in settings.
    pub default_practice_name: String,
    /// One-time code required to create the staff account on a fresh
    /// clinical install. Printed to the server console at startup.
    pub setup_code: Arc<std::sync::Mutex<Option<String>>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/api", api::routes(&state))
        .fallback(assets::serve)
        .layer(from_fn_with_state(state.clone(), security::headers))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
