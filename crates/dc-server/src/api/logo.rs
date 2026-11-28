//! Practice logo routes: upload, removal, and serving it to staff screens
//! and the patient screens (DECISIONS E26).

use super::staff::{load_settings, reject_demo};
use crate::http::Failure;
use crate::{AppState, audit, logo};
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::extract::rejection::BytesRejection;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use dc_core::api::PracticeSettings;

/// Request body limit for the upload route. A little over the logo limit,
/// so a file just over 1 MB gets the friendly message from `logo::check`.
pub const BODY_LIMIT: usize = logo::MAX_BYTES + 1;

/// Logos are drawn through `<img>` only. If one is opened directly, this
/// policy keeps it from running script, loading anything or framing.
const LOGO_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

pub fn staff_url(version: &str) -> String {
    format!("/api/staff/settings/logo?v={version}")
}

pub fn present_url(version: &str) -> String {
    format!("/api/present/logo?v={version}")
}

async fn serve(state: &AppState) -> Result<Response, Failure> {
    let Some(stored) = logo::load(&state.db).await? else {
        return Err(Failure::new(
            StatusCode::NOT_FOUND,
            "logo_not_set",
            "No logo has been uploaded.",
        ));
    };
    let content_type = HeaderValue::from_str(&stored.content_type).map_err(Failure::internal)?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(LOGO_CSP),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("inline"),
            ),
        ],
        stored.bytes,
    )
        .into_response())
}

/// `GET /api/staff/settings/logo`
pub async fn get_staff(State(state): State<AppState>) -> Result<Response, Failure> {
    reject_demo(&state)?;
    serve(&state).await
}

/// `GET /api/present/logo`: the same logo for the patient screens.
pub async fn get_present(State(state): State<AppState>) -> Result<Response, Failure> {
    serve(&state).await
}

/// `PUT /api/staff/settings/logo`: the raw file is the request body.
pub async fn put(
    State(state): State<AppState>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<PracticeSettings>, Failure> {
    reject_demo(&state)?;
    let bytes = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            Failure::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "logo_too_large",
                "The logo must be 1 MB or smaller.",
            )
        } else {
            Failure::new(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "The upload could not be read.",
            )
        }
    })?;
    let kind = logo::check(&bytes).map_err(|message| {
        Failure::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_logo", message)
    })?;
    logo::save(&state.db, kind, &bytes).await?;
    audit::record(&state.db, "staff", "settings_changed", "logo uploaded").await?;
    Ok(Json(load_settings(&state).await?))
}

/// `DELETE /api/staff/settings/logo`: patient screens go back to the initial.
pub async fn delete(State(state): State<AppState>) -> Result<Json<PracticeSettings>, Failure> {
    reject_demo(&state)?;
    if logo::delete(&state.db).await? {
        audit::record(&state.db, "staff", "settings_changed", "logo removed").await?;
    }
    Ok(Json(load_settings(&state).await?))
}
