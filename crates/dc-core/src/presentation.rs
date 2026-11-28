//! API types for creating, tracking and viewing presentations.

use crate::content::ReadingLevel;
use crate::money::Money;
use serde::{Deserialize, Serialize};

/// `POST /api/staff/presentations`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePresentationRequest {
    pub patient_key: String,
    pub plan_key: String,
    /// BCP 47 tag the patient will see. Content is written in English first;
    /// other languages are produced from it in milestone M7.
    pub language: String,
    pub reading_level: ReadingLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePresentationResponse {
    pub presentation_id: String,
    pub job_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    /// Every section passed.
    Succeeded,
    /// Finished, but some sections are held for staff.
    NeedsReview,
    Failed,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            JobState::Succeeded | JobState::NeedsReview | JobState::Failed
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionStage {
    Queued,
    Writing,
    Reviewing,
    Repairing,
    Passed,
    NeedsReview,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionProgress {
    /// `at_a_glance`, `journey`, `procedure:p3`, `next_steps`.
    pub key: String,
    /// Staff-facing label, e.g. "Root canal · #30".
    pub label: String,
    pub stage: SectionStage,
}

/// `GET /api/staff/jobs/:id`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStatus {
    pub job_id: String,
    pub presentation_id: String,
    pub state: JobState,
    pub sections: Vec<SectionProgress>,
    pub elapsed_ms: u64,
    /// Staff-readable reason when the whole job failed.
    pub error: Option<String>,
    /// The one section a regeneration job rewrites; `None` for a job that
    /// writes the whole presentation. `sections` always lists every section,
    /// with the stages of the sections this job does not touch as they are.
    #[serde(default)]
    pub section_key: Option<String>,
    /// While the job is queued: how many jobs on the server are running or
    /// were queued before it. `None` once it runs.
    #[serde(default)]
    pub queue_ahead: Option<u32>,
}

/// `POST /api/staff/presentations/:id/sections/:section_key/regenerate`.
/// Poll `GET /api/staff/jobs/:job_id` for progress, as after creating a
/// presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegenerateSectionResponse {
    pub presentation_id: String,
    pub job_id: String,
    pub section_key: String,
}

/// Where a block of content came from. Mirrors `provenance::Provenance` for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockSource {
    /// Locked fact from the practice's records.
    Source,
    /// Written from a clinical note linked to the procedure.
    ClinicalNote,
    /// Written by the model and passed the checks.
    Generated,
    /// Edited by staff.
    StaffEdit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextBlock {
    /// `what_it_is`, `why`, `summary`, `phases/0/summary`, …
    pub field: String,
    pub heading: String,
    pub sentences: Vec<String>,
    pub source: BlockSource,
    /// The reason block came from a clinical note and waits for one staff
    /// confirmation before patients see it (DECISIONS P11).
    pub needs_confirmation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionVisibility {
    /// Passed checks and review; patients may see it.
    Ready,
    /// Held for staff; patients see nothing from this section.
    NeedsReview,
    /// Failed checks; never shown.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionView {
    pub key: String,
    pub title: String,
    pub visibility: SectionVisibility,
    /// Why the section is held or failed, readable by staff.
    pub reasons: Vec<String>,
    pub blocks: Vec<TextBlock>,
    /// Questions the patient could ask, for procedure and next-steps sections.
    pub questions: Vec<String>,
    /// Procedure ref for procedure sections, e.g. `p3`.
    pub procedure_ref: Option<String>,
    /// The section can be written again with
    /// `POST .../sections/:key/regenerate`. False for a procedure with no
    /// knowledge entry, which is never improvised.
    #[serde(default)]
    pub can_regenerate: bool,
    /// A held section (`NeedsReview`) that staff may release to patients
    /// with `POST .../sections/:key/release`: it has output, and every
    /// block, with staff edits applied, passes the deterministic checks.
    #[serde(default)]
    pub can_release: bool,
    /// For a held section with output, the blocking findings of the
    /// deterministic checks on the current text. Release stays refused
    /// until this is empty.
    #[serde(default)]
    pub release_blockers: Vec<String>,
}

/// A locked fact row, rendered from the plan snapshot, never from a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureFactView {
    pub procedure_ref: String,
    pub name: String,
    pub code: String,
    /// "30", "K", or empty for procedures without a tooth.
    pub tooth: String,
    pub surfaces: String,
    /// 1-based phase number in treatment order.
    pub phase: u32,
    pub fee: Option<Money>,
    pub insurance_estimate: Option<Money>,
    pub patient_estimate: Option<Money>,
    /// Knowledge base category id, e.g. `root_canal`. `None` for unsupported codes.
    #[serde(default)]
    pub category: Option<String>,
}

/// `GET /api/staff/presentations/:id`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationView {
    pub id: String,
    pub version: u32,
    pub patient_key: String,
    pub patient_first_name: String,
    pub patient_display_name: String,
    pub plan_name: String,
    pub provider_display_name: Option<String>,
    pub language: String,
    pub reading_level: ReadingLevel,
    pub created_at: String,
    pub job_id: String,
    pub job_state: JobState,
    pub procedures: Vec<ProcedureFactView>,
    pub total_fee: Option<Money>,
    pub total_patient_estimate: Option<Money>,
    /// In presentation order. Empty until the job finishes.
    pub sections: Vec<SectionView>,
    pub writer_model: String,
    pub prompt_version: String,
    pub generation_ms: u64,
}

/// One row of `GET /api/staff/presentations`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationSummary {
    pub id: String,
    pub patient_key: String,
    pub patient_display_name: String,
    pub plan_name: String,
    pub version: u32,
    pub created_at: String,
    pub job_state: JobState,
    pub sections_ready: u32,
    pub sections_total: u32,
}
