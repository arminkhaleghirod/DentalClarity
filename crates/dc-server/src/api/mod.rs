//! JSON API under `/api`. Route groups and their access rules follow the
//! authorization matrix in docs/SECURITY.md.

mod analytics;
mod auth;
mod edits;
mod present;
mod presentations;
mod qa;
mod review;
mod staff;
#[cfg(test)]
mod tests;

pub use analytics::purge_old as purge_old_engagement;

use crate::{AppState, security};
use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use dc_core::api::HealthResponse;

pub fn routes(state: &AppState) -> Router<AppState> {
    let staff = Router::new()
        .route("/dashboard", get(staff::dashboard))
        .route("/analytics", get(analytics::summary))
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
        .route("/presentations/{id}/present", post(present::start))
        .route("/presentations/{id}/blocks", put(edits::update))
        .route("/presentations/{id}/blocks/revert", post(edits::revert))
        .route(
            "/presentations/{id}/confirm-note",
            post(edits::confirm_note),
        )
        .route(
            "/presentations/{id}/sections/{section_key}/regenerate",
            post(review::regenerate),
        )
        .route(
            "/presentations/{id}/sections/{section_key}/release",
            post(review::release),
        )
        .route("/presentations/{id}/questions", get(qa::staff_questions))
        .route("/jobs/{id}", get(presentations::job))
        .route_layer(from_fn_with_state(state.clone(), security::require_session));

    // Patient screens: only the present session cookie opens these.
    let present = Router::new()
        .route("/content", get(present::content))
        .route("/audio/{key}", get(present::audio))
        .route("/narration", get(present::narration))
        .route("/analytics", post(analytics::record))
        .route("/qa", get(qa::conversation).post(qa::ask))
        .route("/exit", post(present::exit))
        .route_layer(from_fn_with_state(state.clone(), security::require_present));

    Router::new()
        .route("/health", get(health))
        .route("/session", get(auth::session))
        .route("/session/unlock", post(auth::unlock))
        .route("/visit", post(auth::visit))
        .route("/setup", post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .nest("/staff", staff)
        .nest("/present", present)
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
