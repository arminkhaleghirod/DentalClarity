//! Connectors that read treatment plans from a practice management system.
//!
//! Every connector is read-only. The [`synthetic`] connector serves the
//! fictional practice in `fixtures/synthetic` and is the only connector
//! allowed in demo mode. The [`opendental`] connector (cargo feature
//! `opendental`) reads an Open Dental MySQL database over a read-only user.

#[cfg(feature = "opendental")]
pub mod opendental;
pub mod synthetic;

use async_trait::async_trait;
use dc_core::PlanSnapshot;
use dc_core::plan::{PatientKey, PlanKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum PmsError {
    #[error("patient {0} not found")]
    PatientNotFound(String),
    #[error("treatment plan {0} not found")]
    PlanNotFound(String),
    #[error("could not load PMS data: {0}")]
    Load(String),
    #[error("PMS unavailable: {0}")]
    Unavailable(String),
}

pub type Result<T> = std::result::Result<T, PmsError>;

/// A patient as listed in the directory and on the dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatientSummary {
    pub key: PatientKey,
    pub first_name: String,
    pub last_name: String,
    pub age_years: Option<u8>,
    pub preferred_language: Option<String>,
    /// Today's appointment time as `HH:MM`, if the patient has one.
    pub appointment_time: Option<String>,
    pub is_guardian_presentation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSummary {
    pub key: PlanKey,
    pub name: String,
    pub procedure_count: u32,
}

#[async_trait]
pub trait PmsConnector: Send + Sync {
    /// Short name shown in settings and logs, e.g. `"synthetic"`.
    fn name(&self) -> &'static str;

    async fn search_patients(&self, query: &str) -> Result<Vec<PatientSummary>>;

    /// Patients with an appointment today and an open treatment plan, in appointment order.
    async fn recommended_patients(&self) -> Result<Vec<PatientSummary>>;

    async fn treatment_plans(&self, patient: &PatientKey) -> Result<Vec<PlanSummary>>;

    async fn plan_snapshot(&self, plan: &PlanKey) -> Result<PlanSnapshot>;
}
