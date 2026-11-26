use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use dc_core::api::{HealthResponse, PatientRow, PresentationStatus};
use dc_core::plan::PlanSnapshot;
use std::collections::BTreeMap;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/staff/dashboard", get(dashboard))
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        mode: state.mode,
    })
}

/// Today's recommended patients with a one-line summary of each active plan.
async fn dashboard(State(state): State<AppState>) -> Result<Json<Vec<PatientRow>>, StatusCode> {
    let patients = state.pms.recommended_patients().await.map_err(internal)?;
    let mut rows = Vec::with_capacity(patients.len());
    for patient in patients {
        let plans = state
            .pms
            .treatment_plans(&patient.key)
            .await
            .map_err(internal)?;
        let Some(plan) = plans.first() else { continue };
        let snapshot = state.pms.plan_snapshot(&plan.key).await.map_err(internal)?;
        rows.push(PatientRow {
            patient_key: patient.key.0.clone(),
            display_name: format!("{} {}", patient.first_name, patient.last_name),
            age_years: patient.age_years,
            appointment_time: patient.appointment_time.clone(),
            plan_summary: summarize(&snapshot, &state),
            procedure_count: plan.procedure_count,
            language: patient
                .preferred_language
                .clone()
                .unwrap_or_else(|| "en".into()),
            status: PresentationStatus::NotStarted,
            patient_estimate: snapshot.total_patient_estimate(),
        });
    }
    Ok(Json(rows))
}

/// "Root canal, Crown, 2 tooth-colored fillings", in plan order.
fn summarize(snapshot: &PlanSnapshot, state: &AppState) -> String {
    let mut counts: BTreeMap<usize, (String, u32)> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for p in &snapshot.procedures {
        let name = state
            .codes
            .get(&p.code)
            .map_or_else(|| p.code.to_string(), |i| i.name.clone());
        let idx = order.iter().position(|n| *n == name).unwrap_or_else(|| {
            order.push(name.clone());
            order.len() - 1
        });
        counts.entry(idx).or_insert((name, 0)).1 += 1;
    }
    counts
        .into_values()
        .map(|(name, n)| {
            if n == 1 {
                name
            } else {
                format!("{n}× {name}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn internal(err: impl std::fmt::Display) -> StatusCode {
    tracing::error!(%err, "request failed");
    StatusCode::INTERNAL_SERVER_ERROR
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{router, storage};
    use axum::body::Body;
    use axum::http::Request;
    use dc_core::Mode;
    use dc_knowledge::CodeMap;
    use dc_pms::synthetic::SyntheticConnector;
    use std::path::Path;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn state() -> AppState {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        AppState {
            mode: Mode::Demo,
            db: storage::open_in_memory().await.unwrap(),
            pms: Arc::new(
                SyntheticConnector::from_path(&root.join("fixtures/synthetic/practice.toml"))
                    .unwrap(),
            ),
            codes: Arc::new(CodeMap::from_path(&root.join("knowledge/codes.toml")).unwrap()),
        }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(uri: &str) -> T {
        let response = router(state().await)
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_reports_mode() {
        let body: HealthResponse = get_json("/api/health").await;
        assert_eq!(body.mode, Mode::Demo);
    }

    #[tokio::test]
    async fn dashboard_lists_todays_patients_with_summaries() {
        let rows: Vec<PatientRow> = get_json("/api/staff/dashboard").await;
        let daniel = rows
            .iter()
            .find(|r| r.display_name == "Daniel Okafor")
            .unwrap();
        assert_eq!(
            daniel.plan_summary,
            "Root canal, Build-up, Crown, 2× Tooth-colored filling, Cleaning"
        );
        assert_eq!(daniel.patient_estimate.map(|m| m.cents()), Some(136_900));
        assert!(rows.iter().all(|r| r.display_name != "Grace Holloway"));
    }
}
