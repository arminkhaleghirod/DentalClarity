//! Request and response bodies for the JSON API.

use crate::mode::Mode;
use crate::money::Money;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub mode: Mode,
}

/// Status of a patient's presentation, as shown on the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationStatus {
    NotStarted,
    Generating,
    Ready,
    PlanChanged,
    NeedsReview,
}

/// One row of the dashboard's "Today's patients" table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatientRow {
    pub patient_key: String,
    pub display_name: String,
    pub age_years: Option<u8>,
    /// Appointment time as `HH:MM` in the practice's local time, if any.
    pub appointment_time: Option<String>,
    pub plan_summary: String,
    pub procedure_count: u32,
    pub language: String,
    pub status: PresentationStatus,
    pub patient_estimate: Option<Money>,
}
