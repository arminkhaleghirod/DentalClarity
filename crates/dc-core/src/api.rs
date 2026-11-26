//! Request and response bodies for the JSON API.

use crate::mode::Mode;
use crate::money::Money;
use serde::{Deserialize, Serialize};

/// Error body returned by every failed API call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// Stable machine-readable code, e.g. `"invalid_credentials"`.
    pub code: String,
    /// Message safe to show to staff.
    pub message: String,
}

/// Where the browser stands before it can use the staff workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Clinical mode with no staff account yet.
    SetupRequired,
    /// Clinical mode, no valid session.
    SignedOut,
    /// Signed in to the shared staff account.
    Staff,
    /// Staff session locked by Present mode.
    Locked,
    /// Demo mode, no visitor session yet. The app starts one with
    /// `POST /api/visit`.
    NewVisitor,
    /// Demo mode visitor. No sign-in needed.
    Visitor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub mode: Mode,
    pub state: SessionState,
    pub practice_name: String,
    /// Token to send in the `X-DC-CSRF` header on state-changing requests.
    /// Present only when the browser holds a session.
    pub csrf_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

/// First-launch setup in clinical mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupRequest {
    /// One-time code printed in the server console on first start.
    pub setup_code: String,
    pub practice_name: String,
    pub password: String,
    pub pin: String,
}

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

/// One row of the patient directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatientListItem {
    pub patient_key: String,
    pub display_name: String,
    pub age_years: Option<u8>,
    /// BCP 47 tag recorded in the PMS, if any.
    pub language: Option<String>,
    /// Today's appointment time as `HH:MM`, if any.
    pub appointment_time: Option<String>,
    pub open_plans: u32,
}

/// A patient with every open treatment plan, for the patient page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatientDetail {
    pub patient_key: String,
    pub first_name: String,
    pub last_name: String,
    pub age_years: Option<u8>,
    pub language: Option<String>,
    pub appointment_time: Option<String>,
    pub plans: Vec<PlanDetail>,
}

/// A treatment plan as read from the PMS, with totals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDetail {
    pub plan_key: String,
    pub name: String,
    pub provider_display_name: Option<String>,
    pub procedures: Vec<ProcedureLine>,
    /// `None` when any fee is unknown.
    pub total_fee: Option<Money>,
    /// `None` when any patient estimate is unknown.
    pub total_patient_estimate: Option<Money>,
}

/// One planned procedure in the staff plan table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureLine {
    pub phase: Option<u8>,
    /// Universal tooth number or letter, an area such as "upper right", or
    /// empty when the procedure has no tooth.
    pub tooth: String,
    pub surfaces: String,
    pub code: String,
    /// Our plain-language name from the knowledge base. `None` for codes the
    /// knowledge base does not cover.
    pub name: Option<String>,
    /// The practice's own description from its PMS.
    pub pms_description: String,
    pub fee: Option<Money>,
    pub patient_estimate: Option<Money>,
}

/// Practice name and brand colors used on patient screens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PracticeSettings {
    pub practice_name: String,
    /// `#RRGGBB`. White text on it must reach 4.5:1 contrast.
    pub brand_primary: String,
    /// `#RRGGBB`. Marks the patient's own teeth.
    pub brand_highlight: String,
}

pub const DEFAULT_BRAND_PRIMARY: &str = "#1B3A5C";
pub const DEFAULT_BRAND_HIGHLIGHT: &str = "#E8693F";
