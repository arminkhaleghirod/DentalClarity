//! `GET /api/staff/presentations/{id}/export[?version=N]`: the take-home
//! PDF booklet of one version (PRD 5.14, UI_SPEC section 7).
//!
//! The booklet is built from the same patient view Present mode serves, so
//! held sections, unconfirmed note text and anything else patients may not
//! see never reach it. It is rendered on request and not stored, so no
//! booklet file with patient data stays on disk (DECISIONS E7).

use super::present::patient_view_of;
use super::presentations::{MAX_ID_CHARS, not_found, owner_scope};
use crate::http::Failure;
use crate::security::CurrentSession;
use crate::versions::{self, Marker};
use crate::{AppState, audit, jobs};
use axum::extract::{Path, RawQuery, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use dc_booklet::{Booklet, OutlineEntry, Paper, VersionInfo};
use dc_core::plan::PlanSnapshot;
use dc_generation::facts::FactSheet;
use dc_generation::prompt::SectionKind;
use std::sync::LazyLock;
use tokio::sync::Semaphore;

/// Booklets typeset at the same time across the server. Typesetting is CPU
/// work, so this keeps repeated exports from starving other requests.
pub const MAX_CONCURRENT_RENDERS: usize = 2;
static RENDERS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(MAX_CONCURRENT_RENDERS));

fn bad_version() -> Failure {
    Failure::new(
        StatusCode::BAD_REQUEST,
        "bad_request",
        "Ask for the current version or a version number, such as ?version=2.",
    )
}

/// The `version=N` of the query, if any.
fn requested_version(query: Option<&str>) -> Result<Option<i64>, Failure> {
    let Some(query) = query.filter(|q| !q.is_empty()) else {
        return Ok(None);
    };
    let value = query.strip_prefix("version=").ok_or_else(bad_version)?;
    let version: i64 = value.parse().map_err(|_| bad_version())?;
    if version < 1 {
        return Err(bad_version());
    }
    Ok(Some(version))
}

/// `GET /api/staff/presentations/{id}/export`
pub async fn export(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, Failure> {
    if id.len() > MAX_ID_CHARS {
        return Err(not_found("presentation"));
    }
    let requested = requested_version(query.as_deref())?;
    let owner = owner_scope(&state, &session);
    let (booklet, version) = booklet_for(&state, &owner, &id, requested).await?;

    let _permit = RENDERS.acquire().await.map_err(Failure::internal)?;
    let pdf = tokio::task::spawn_blocking(move || booklet.render())
        .await
        .map_err(Failure::internal)?
        .map_err(Failure::internal)?;
    drop(_permit);

    // An exported version is frozen: a later rewrite makes a new version.
    let mut tx = state.db.begin().await?;
    versions::mark(&mut *tx, &id, version, Marker::Exported).await?;
    audit::record(
        &mut *tx,
        super::present::actor(session.kind),
        "booklet_exported",
        &format!("presentation {id} version {version}"),
    )
    .await?;
    tx.commit().await?;

    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"treatment-plan-v{version}.pdf\""),
            ),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        pdf,
    )
        .into_response())
}

/// The booklet of `requested` (or the current version) of presentation `id`
/// in `owner`'s scope, with the version number.
pub(super) async fn booklet_for(
    state: &AppState,
    owner: &str,
    id: &str,
    requested: Option<i64>,
) -> Result<(Booklet, i64), Failure> {
    let in_scope: Option<(i64,)> =
        sqlx::query_as("SELECT version FROM presentations WHERE id = ? AND owner_scope = ?")
            .bind(id)
            .bind(owner)
            .fetch_optional(&state.db)
            .await?;
    if in_scope.is_none() {
        return Err(not_found("presentation"));
    }
    let data = versions::load(&state.db, id, requested)
        .await?
        .ok_or_else(|| not_found("presentation"))?;
    if data.is_current() {
        let job: Option<(String,)> = sqlx::query_as(
            "SELECT state FROM jobs WHERE presentation_id = ? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
        if !job.is_some_and(|(s,)| jobs::parse_job_state(&s).is_finished()) {
            return Err(Failure::conflict(
                "not_ready",
                "This presentation is still being written.",
            ));
        }
    }

    let snapshot: PlanSnapshot =
        serde_json::from_str(&data.snapshot_json).map_err(Failure::internal)?;
    let sheet = FactSheet::build(&snapshot, &state.kb);
    let outline: Vec<OutlineEntry> = data
        .rows
        .iter()
        .map(|row| OutlineEntry {
            key: row.0.clone(),
            title: SectionKind::from_key(&row.0)
                .map_or_else(|| row.0.clone(), |kind| jobs::section_label(&kind, &sheet)),
        })
        .collect();
    let info = VersionInfo {
        version: u32::try_from(data.version).unwrap_or(1),
        fingerprint: data.fingerprint.clone(),
        plan_as_of: versions::print_date(data.created_at),
        prepared_on: versions::print_date(crate::sessions::now()),
        paper: Paper::UsLetter,
    };
    let version = data.version;
    let view = patient_view_of(state, id, data).await?;
    if view.sections.is_empty() {
        return Err(Failure::conflict(
            "nothing_to_export",
            "No section of this version is ready for patients, so there is nothing to print.",
        ));
    }
    Ok((Booklet::build(&view, &outline, &info), version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_query() {
        assert_eq!(requested_version(None).ok(), Some(None));
        assert_eq!(requested_version(Some("")).ok(), Some(None));
        assert_eq!(requested_version(Some("version=3")).ok(), Some(Some(3)));
        for bad in ["version=0", "version=x", "v=2", "version=2&x=1"] {
            assert!(requested_version(Some(bad)).is_err(), "{bad}");
        }
    }
}
