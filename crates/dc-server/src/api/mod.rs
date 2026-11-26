//! JSON API under `/api`. Route groups and their access rules follow the
//! authorization matrix in docs/SECURITY.md.

mod auth;
mod presentations;
mod staff;
#[cfg(test)]
mod tests;

use crate::{AppState, security};
use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::{Json, Router};
use dc_core::api::HealthResponse;

pub fn routes(state: &AppState) -> Router<AppState> {
    let staff = Router::new()
        .route("/dashboard", get(staff::dashboard))
        .route("/patients", get(staff::patients))
        .route("/patients/{key}", get(staff::patient))
        .route(
            "/settings",
            get(staff::get_settings).put(staff::put_settings),
        )
        .route(
            "/presentations",
            get(presentations::list).post(presentations::create),
        )
        .route("/presentations/{id}", get(presentations::view))
        .route("/jobs/{id}", get(presentations::job))
        .route_layer(from_fn_with_state(state.clone(), security::require_session));

    Router::new()
        .route("/health", get(health))
        .route("/session", get(auth::session))
        .route("/visit", post(auth::visit))
        .route("/setup", post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .nest("/staff", staff)
        .layer(from_fn_with_state(state.clone(), security::same_origin))
        .layer(DefaultBodyLimit::max(64 * 1024))
}

async fn health(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        mode: state.mode,
    })
}
