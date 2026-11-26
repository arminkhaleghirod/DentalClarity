//! Staff workspace endpoints. Every route here runs behind
//! `security::require_session`.

use crate::AppState;
use crate::http::Failure;
use axum::Json;
use axum::extract::State;
use dc_core::api::{PatientRow, PresentationStatus};
use dc_core::plan::PlanSnapshot;

/// `GET /api/staff/dashboard`: today's recommended patients with a one-line
/// summary of each active plan.
pub async fn dashboard(State(state): State<AppState>) -> Result<Json<Vec<PatientRow>>, Failure> {
    let patients = state
        .pms
        .recommended_patients()
        .await
        .map_err(Failure::internal)?;
    let mut rows = Vec::with_capacity(patients.len());
    for patient in patients {
        let plans = state
            .pms
            .treatment_plans(&patient.key)
            .await
            .map_err(Failure::internal)?;
        let Some(plan) = plans.first() else { continue };
        let snapshot = state
            .pms
            .plan_snapshot(&plan.key)
            .await
            .map_err(Failure::internal)?;
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

/// "Root canal, Crown, 2× Tooth-colored filling", in plan order.
pub(crate) fn summarize(snapshot: &PlanSnapshot, state: &AppState) -> String {
    let mut groups: Vec<(String, u32)> = Vec::new();
    for p in &snapshot.procedures {
        let name = state
            .codes
            .get(&p.code)
            .map_or_else(|| p.code.to_string(), |i| i.name.clone());
        match groups.iter_mut().find(|(n, _)| *n == name) {
            Some((_, count)) => *count += 1,
            None => groups.push((name, 1)),
        }
    }
    groups
        .into_iter()
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
