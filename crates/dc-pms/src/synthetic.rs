//! A fictional practice loaded from a TOML fixture.
//!
//! All names, plans and notes in the fixture are invented. The format mirrors
//! what the Open Dental connector produces so the rest of the app cannot tell
//! the two apart.

use crate::{PatientSummary, PlanSummary, PmsConnector, PmsError, Result};
use async_trait::async_trait;
use dc_core::money::{Amount, Money};
use dc_core::plan::{ClinicalNote, PatientContext, PatientKey, PlanKey, PlannedProcedure};
use dc_core::procedure::{ProcedureCode, ProcedureStatus};
use dc_core::{PlanSnapshot, ToothRef};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct Fixture {
    practice: PracticeFixture,
    patients: Vec<PatientFixture>,
}

#[derive(Debug, Deserialize)]
struct PracticeFixture {
    name: String,
}

#[derive(Debug, Deserialize)]
struct PatientFixture {
    key: String,
    first_name: String,
    last_name: String,
    age: Option<u8>,
    language: Option<String>,
    appointment: Option<String>,
    #[serde(default)]
    guardian: bool,
    #[serde(default)]
    plans: Vec<PlanFixture>,
}

#[derive(Debug, Deserialize)]
struct PlanFixture {
    key: String,
    name: String,
    provider: Option<String>,
    procedures: Vec<ProcedureFixture>,
}

#[derive(Debug, Deserialize)]
struct ProcedureFixture {
    id: String,
    code: String,
    description: String,
    #[serde(default)]
    tooth: String,
    #[serde(default)]
    surfaces: String,
    phase: Option<u8>,
    /// Fees and estimates in cents. Missing values stay unknown.
    fee: Option<i64>,
    insurance_est: Option<i64>,
    patient_est: Option<i64>,
    note: Option<String>,
}

/// In-memory connector over a fixture file.
#[derive(Debug)]
pub struct SyntheticConnector {
    practice_name: String,
    patients: Vec<PatientFixture>,
}

fn amount(cents: Option<i64>) -> Amount {
    cents.map_or(Amount::Unknown, |c| Amount::Known(Money(c)))
}

impl SyntheticConnector {
    pub fn from_toml(source: &str) -> Result<Self> {
        let fixture: Fixture = toml::from_str(source).map_err(|e| PmsError::Load(e.to_string()))?;
        // Validate every plan up front so a bad fixture fails at startup, not mid-demo.
        for patient in &fixture.patients {
            for plan in &patient.plans {
                for p in &plan.procedures {
                    p.tooth.parse::<ToothRef>().map_err(|e| {
                        PmsError::Load(format!("plan {} procedure {}: {e}", plan.key, p.id))
                    })?;
                }
            }
        }
        Ok(Self {
            practice_name: fixture.practice.name,
            patients: fixture.patients,
        })
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| PmsError::Load(format!("{}: {e}", path.display())))?;
        Self::from_toml(&source)
    }

    pub fn practice_name(&self) -> &str {
        &self.practice_name
    }

    fn summary(p: &PatientFixture) -> PatientSummary {
        PatientSummary {
            key: PatientKey(p.key.clone()),
            first_name: p.first_name.clone(),
            last_name: p.last_name.clone(),
            age_years: p.age,
            preferred_language: p.language.clone(),
            appointment_time: p.appointment.clone(),
            is_guardian_presentation: p.guardian,
        }
    }

    fn snapshot(patient: &PatientFixture, plan: &PlanFixture) -> PlanSnapshot {
        let procedures = plan
            .procedures
            .iter()
            .map(|p| PlannedProcedure {
                source_id: p.id.clone(),
                code: ProcedureCode::new(&p.code),
                pms_description: p.description.clone(),
                // Validated in `from_toml`.
                tooth: p.tooth.parse().unwrap_or(ToothRef::None),
                surfaces: p.surfaces.clone(),
                phase: p.phase,
                status: ProcedureStatus::Planned,
                fee: amount(p.fee),
                insurance_estimate: amount(p.insurance_est),
                patient_estimate: amount(p.patient_est),
                note: p.note.as_ref().map(|text| ClinicalNote {
                    source_id: format!("{}-note", p.id),
                    text: text.clone(),
                }),
            })
            .collect();
        PlanSnapshot {
            plan: PlanKey(plan.key.clone()),
            patient: PatientContext {
                key: PatientKey(patient.key.clone()),
                first_name: patient.first_name.clone(),
                last_name: patient.last_name.clone(),
                age_years: patient.age,
                preferred_language: patient.language.clone(),
            },
            plan_name: plan.name.clone(),
            provider_display_name: plan.provider.clone(),
            procedures,
        }
    }
}

#[async_trait]
impl PmsConnector for SyntheticConnector {
    fn name(&self) -> &'static str {
        "synthetic"
    }

    async fn search_patients(&self, query: &str) -> Result<Vec<PatientSummary>> {
        let q = query.trim().to_lowercase();
        Ok(self
            .patients
            .iter()
            .filter(|p| {
                q.is_empty()
                    || format!("{} {}", p.first_name, p.last_name)
                        .to_lowercase()
                        .contains(&q)
                    || p.key == q
            })
            .map(Self::summary)
            .collect())
    }

    async fn recommended_patients(&self) -> Result<Vec<PatientSummary>> {
        let mut rows: Vec<_> = self
            .patients
            .iter()
            .filter(|p| p.appointment.is_some() && !p.plans.is_empty())
            .map(Self::summary)
            .collect();
        // Fixture times are zero-padded HH:MM, so string order is time order.
        rows.sort_by(|a, b| a.appointment_time.cmp(&b.appointment_time));
        Ok(rows)
    }

    async fn treatment_plans(&self, patient: &PatientKey) -> Result<Vec<PlanSummary>> {
        let p = self
            .patients
            .iter()
            .find(|p| p.key == patient.0)
            .ok_or_else(|| PmsError::PatientNotFound(patient.0.clone()))?;
        Ok(p.plans
            .iter()
            .map(|plan| PlanSummary {
                key: PlanKey(plan.key.clone()),
                name: plan.name.clone(),
                procedure_count: plan.procedures.len() as u32,
            })
            .collect())
    }

    async fn plan_snapshot(&self, plan: &PlanKey) -> Result<PlanSnapshot> {
        self.patients
            .iter()
            .find_map(|patient| {
                patient
                    .plans
                    .iter()
                    .find(|p| p.key == plan.0)
                    .map(|p| Self::snapshot(patient, p))
            })
            .ok_or_else(|| PmsError::PlanNotFound(plan.0.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector() -> SyntheticConnector {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/practice.toml");
        SyntheticConnector::from_path(&path).expect("fixture loads")
    }

    #[tokio::test]
    async fn recommended_patients_are_in_appointment_order() {
        let rows = connector().recommended_patients().await.unwrap();
        assert!(!rows.is_empty());
        let times: Vec<_> = rows
            .iter()
            .map(|r| r.appointment_time.clone().unwrap())
            .collect();
        let mut sorted = times.clone();
        sorted.sort();
        assert_eq!(times, sorted);
    }

    #[tokio::test]
    async fn every_fixture_plan_produces_a_snapshot() {
        let c = connector();
        for patient in c.search_patients("").await.unwrap() {
            for plan in c.treatment_plans(&patient.key).await.unwrap() {
                let snap = c.plan_snapshot(&plan.key).await.unwrap();
                assert_eq!(snap.procedures.len() as u32, plan.procedure_count);
            }
        }
    }

    #[tokio::test]
    async fn search_matches_full_name() {
        let rows = connector().search_patients("daniel oka").await.unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn rejects_invalid_tooth() {
        let bad = r#"
            [practice]
            name = "X"
            [[patients]]
            key = "1"
            first_name = "A"
            last_name = "B"
            [[patients.plans]]
            key = "p"
            name = "Active"
            [[patients.plans.procedures]]
            id = "1"
            code = "D2740"
            description = "Crown"
            tooth = "40"
        "#;
        assert!(SyntheticConnector::from_toml(bad).is_err());
    }
}
