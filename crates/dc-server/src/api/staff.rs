//! Staff workspace endpoints. Every route here runs behind
//! `security::require_session`.

use crate::http::{Failure, JsonBody};
use crate::{AppState, audit, settings};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use dc_core::api::{
    DEFAULT_BRAND_HIGHLIGHT, DEFAULT_BRAND_PRIMARY, PatientDetail, PatientListItem, PatientRow,
    PlanDetail, PracticeSettings, PresentationStatus, ProcedureLine,
};
use dc_core::color;
use dc_core::plan::{PatientKey, PlanSnapshot};
use dc_core::{Mode, ToothRef};
use dc_pms::PmsError;
use serde::Deserialize;

/// Longest search text accepted; anything after this is ignored.
const MAX_QUERY_CHARS: usize = 100;
/// Longest patient key accepted in a path. PMS keys are short numbers.
const MAX_KEY_CHARS: usize = 64;

/// `GET /api/staff/dashboard`: today's recommended patients with a one-line
/// summary of each active plan.
pub async fn dashboard(
    State(state): State<AppState>,
    crate::security::CurrentSession(session): crate::security::CurrentSession,
) -> Result<Json<Vec<PatientRow>>, Failure> {
    // Latest presentation per patient in this caller's scope, newest first.
    let owner = super::presentations::owner_scope(&state, &session);
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT p.patient_key, p.id, j.state
         FROM presentations p JOIN jobs j ON j.presentation_id = p.id
         WHERE p.owner_scope = ?
         ORDER BY p.created_at DESC, p.rowid DESC",
    )
    .bind(&owner)
    .fetch_all(&state.db)
    .await?;
    let mut latest: std::collections::HashMap<String, (String, PresentationStatus)> =
        std::collections::HashMap::new();
    for (patient_key, id, job_state) in rows {
        latest
            .entry(patient_key)
            .or_insert_with(|| (id, status_for_job(&job_state)));
    }

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
            status: latest
                .get(&patient.key.0)
                .map_or(PresentationStatus::NotStarted, |(_, status)| *status),
            patient_estimate: snapshot.total_patient_estimate(),
            presentation_id: latest.get(&patient.key.0).map(|(id, _)| id.clone()),
        });
    }
    Ok(Json(rows))
}

/// Dashboard status for a job state. A failed job needs staff attention, so
/// it counts as needs review rather than disappearing.
fn status_for_job(job_state: &str) -> PresentationStatus {
    match job_state {
        "queued" | "running" => PresentationStatus::Generating,
        "succeeded" => PresentationStatus::Ready,
        _ => PresentationStatus::NeedsReview,
    }
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

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
}

/// `GET /api/staff/patients?q=`: patients whose name or patient number
/// matches, sorted by last name. An empty query lists everyone.
pub async fn patients(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<PatientListItem>>, Failure> {
    let q: String = query.q.trim().chars().take(MAX_QUERY_CHARS).collect();
    let mut found = state
        .pms
        .search_patients(&q)
        .await
        .map_err(Failure::internal)?;
    found.sort_by(|a, b| {
        (a.last_name.to_lowercase(), a.first_name.to_lowercase())
            .cmp(&(b.last_name.to_lowercase(), b.first_name.to_lowercase()))
    });
    let mut rows = Vec::with_capacity(found.len());
    for patient in found {
        let plans = state
            .pms
            .treatment_plans(&patient.key)
            .await
            .map_err(Failure::internal)?;
        rows.push(PatientListItem {
            display_name: format!("{} {}", patient.first_name, patient.last_name),
            patient_key: patient.key.0,
            age_years: patient.age_years,
            language: patient.preferred_language,
            appointment_time: patient.appointment_time,
            open_plans: plans.len() as u32,
        });
    }
    Ok(Json(rows))
}

/// `GET /api/staff/patients/{key}`: one patient with each open plan as read
/// from the PMS.
pub async fn patient(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<PatientDetail>, Failure> {
    if key.is_empty() || key.chars().count() > MAX_KEY_CHARS {
        return Err(patient_not_found());
    }
    let key = PatientKey(key);
    let plans = match state.pms.treatment_plans(&key).await {
        Ok(plans) => plans,
        Err(PmsError::PatientNotFound(_)) => return Err(patient_not_found()),
        Err(e) => return Err(Failure::internal(e)),
    };
    let summary = state
        .pms
        .search_patients(&key.0)
        .await
        .map_err(Failure::internal)?
        .into_iter()
        .find(|p| p.key == key)
        .ok_or_else(patient_not_found)?;

    let mut details = Vec::with_capacity(plans.len());
    for plan in plans {
        let snapshot = state
            .pms
            .plan_snapshot(&plan.key)
            .await
            .map_err(Failure::internal)?;
        details.push(plan_detail(&snapshot, &state));
    }
    Ok(Json(PatientDetail {
        patient_key: summary.key.0,
        first_name: summary.first_name,
        last_name: summary.last_name,
        age_years: summary.age_years,
        language: summary.preferred_language,
        appointment_time: summary.appointment_time,
        plans: details,
    }))
}

fn plan_detail(snapshot: &PlanSnapshot, state: &AppState) -> PlanDetail {
    let procedures = snapshot
        .procedures
        .iter()
        .map(|p| ProcedureLine {
            phase: p.phase,
            tooth: match p.tooth {
                ToothRef::None => String::new(),
                other => other.to_string(),
            },
            surfaces: p.surfaces.clone(),
            code: p.code.to_string(),
            name: state.codes.get(&p.code).map(|i| i.name.clone()),
            pms_description: p.pms_description.clone(),
            fee: p.fee.known(),
            patient_estimate: p.patient_estimate.known(),
        })
        .collect();
    PlanDetail {
        plan_key: snapshot.plan.0.clone(),
        name: snapshot.plan_name.clone(),
        provider_display_name: snapshot.provider_display_name.clone(),
        procedures,
        total_fee: snapshot.total_fee(),
        total_patient_estimate: snapshot.total_patient_estimate(),
    }
}

fn patient_not_found() -> Failure {
    Failure::new(
        StatusCode::NOT_FOUND,
        "patient_not_found",
        "No patient with that number was found in the practice records.",
    )
}

/// Demo visitors cannot read or change settings (docs/SECURITY.md). The demo
/// web app shows the defaults instead.
fn reject_demo(state: &AppState) -> Result<(), Failure> {
    if state.mode == Mode::Demo {
        return Err(Failure::new(
            StatusCode::FORBIDDEN,
            "demo_read_only",
            "Settings are read-only in the demo.",
        ));
    }
    Ok(())
}

pub(super) async fn load_settings(state: &AppState) -> Result<PracticeSettings, Failure> {
    Ok(PracticeSettings {
        practice_name: settings::practice_name(state).await?,
        brand_primary: settings::get(&state.db, settings::BRAND_PRIMARY)
            .await?
            .unwrap_or_else(|| DEFAULT_BRAND_PRIMARY.into()),
        brand_highlight: settings::get(&state.db, settings::BRAND_HIGHLIGHT)
            .await?
            .unwrap_or_else(|| DEFAULT_BRAND_HIGHLIGHT.into()),
    })
}

/// `GET /api/staff/settings`: practice name and brand colors.
pub async fn get_settings(
    State(state): State<AppState>,
) -> Result<Json<PracticeSettings>, Failure> {
    reject_demo(&state)?;
    Ok(Json(load_settings(&state).await?))
}

/// `PUT /api/staff/settings`: validates and stores practice name and brand colors.
pub async fn put_settings(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<PracticeSettings>,
) -> Result<Json<PracticeSettings>, Failure> {
    reject_demo(&state)?;
    let valid = validate_settings(&request)
        .map_err(|message| Failure::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid", message))?;
    let mut tx = state.db.begin().await?;
    settings::set(&mut *tx, settings::PRACTICE_NAME, &valid.practice_name).await?;
    settings::set(&mut *tx, settings::BRAND_PRIMARY, &valid.brand_primary).await?;
    settings::set(&mut *tx, settings::BRAND_HIGHLIGHT, &valid.brand_highlight).await?;
    tx.commit().await?;
    audit::record(&state.db, "staff", "settings_changed", "").await?;
    Ok(Json(valid))
}

/// Trims the name, upper-cases the colors, and checks every rule.
fn validate_settings(request: &PracticeSettings) -> Result<PracticeSettings, String> {
    let practice_name = request.practice_name.trim();
    if practice_name.is_empty() || practice_name.chars().count() > 100 {
        return Err("Enter the practice name (up to 100 characters).".into());
    }
    let primary = request.brand_primary.trim();
    let Some(rgb) = color::parse_hex(primary) else {
        return Err("Enter the brand color as #RRGGBB, for example #1B3A5C.".into());
    };
    let highlight = request.brand_highlight.trim();
    if color::parse_hex(highlight).is_none() {
        return Err("Enter the highlight color as #RRGGBB, for example #E8693F.".into());
    }
    let ratio = color::contrast_with_white(rgb);
    if ratio < color::MIN_CONTRAST_ON_PRIMARY {
        return Err(format!(
            "White text on {primary} has a contrast of {ratio:.2}:1. Patients need at least 4.5:1 to read it. Choose a darker brand color."
        ));
    }
    Ok(PracticeSettings {
        practice_name: practice_name.to_owned(),
        brand_primary: primary.to_ascii_uppercase(),
        brand_highlight: highlight.to_ascii_uppercase(),
    })
}
