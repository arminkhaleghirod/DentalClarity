//! Version history under `/api/staff/presentations/{id}/versions`
//! (PRD FR-VER-02). Both routes run behind `security::require_session` and
//! answer 404 for presentations outside the caller's scope.

use super::presentations::{MAX_ID_CHARS, build_view, not_found, owner_scope};
use crate::AppState;
use crate::http::Failure;
use crate::security::CurrentSession;
use axum::Json;
use axum::extract::{Path, State};
use dc_core::presentation::PresentationView;
use dc_core::versions::{VersionCause, VersionSummary};

type VersionRow = (i64, String, i64, String, String, Option<i64>, Option<i64>);

/// `GET /api/staff/presentations/{id}/versions`: every version, newest first.
pub async fn list(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Path(id): Path<String>,
) -> Result<Json<Vec<VersionSummary>>, Failure> {
    if id.len() > MAX_ID_CHARS {
        return Err(not_found("presentation"));
    }
    let current: Option<(i64,)> =
        sqlx::query_as("SELECT version FROM presentations WHERE id = ? AND owner_scope = ?")
            .bind(&id)
            .bind(owner_scope(&state, &session))
            .fetch_optional(&state.db)
            .await?;
    let Some((current,)) = current else {
        return Err(not_found("presentation"));
    };
    let rows: Vec<VersionRow> = sqlx::query_as(
        "SELECT version, cause, created_at, fingerprint, changes_json, presented_at, exported_at
         FROM presentation_versions WHERE presentation_id = ? ORDER BY version DESC",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(version, cause, created_at, fingerprint, changes, presented, exported)| {
                    VersionSummary {
                        version: u32::try_from(version).unwrap_or(1),
                        cause: VersionCause::parse(&cause),
                        created_at: crate::versions::rfc3339(created_at),
                        changes: serde_json::from_str(&changes).unwrap_or_default(),
                        current: version == current,
                        presented: presented.is_some(),
                        exported: exported.is_some(),
                        short_hash: fingerprint.chars().take(8).collect(),
                    }
                },
            )
            .collect(),
    ))
}

/// `GET /api/staff/presentations/{id}/versions/{version}`: one version as
/// the editor shows it. Older versions are read-only.
pub async fn view(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<PresentationView>, Failure> {
    let version: i64 = version.parse().map_err(|_| not_found("presentation"))?;
    if id.len() > MAX_ID_CHARS || version < 1 {
        return Err(not_found("presentation"));
    }
    let owner = owner_scope(&state, &session);
    Ok(Json(build_view(&state, &owner, &id, Some(version)).await?))
}
