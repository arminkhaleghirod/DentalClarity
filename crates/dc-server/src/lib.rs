//! HTTP server: JSON API under `/api`, and the embedded web app for every other path.

pub mod api;
pub mod assets;
pub mod config;
pub mod storage;

use axum::Router;
use axum::http::{HeaderName, HeaderValue};
use dc_core::Mode;
use dc_knowledge::CodeMap;
use dc_pms::PmsConnector;
use sqlx::SqlitePool;
use std::sync::Arc;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub mode: Mode,
    pub db: SqlitePool,
    pub pms: Arc<dyn PmsConnector>,
    pub codes: Arc<CodeMap>,
}

pub fn router(state: AppState) -> Router {
    let header = |name: &'static str, value: &'static str| {
        SetResponseHeaderLayer::overriding(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        )
    };
    Router::new()
        .nest("/api", api::routes())
        .fallback(assets::serve)
        .with_state(state)
        .layer(header("x-content-type-options", "nosniff"))
        .layer(header("x-frame-options", "DENY"))
        .layer(header("referrer-policy", "no-referrer"))
        .layer(TraceLayer::new_for_http())
}
