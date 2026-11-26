use serde::{Deserialize, Serialize};

/// Where a block of presentation content came from. Shown to staff as a label
/// on every block and used to decide what must be regenerated after a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Provenance {
    /// A fact copied from the plan snapshot. Read-only in the editor.
    Source,
    /// Plain-language wording of a clinical note linked to a planned procedure.
    ClinicalNote { note_source_id: String },
    /// Written by a model and checked by the validators.
    Generated {
        model: String,
        prompt_version: String,
    },
    /// Edited by staff. `previous` keeps the text it replaced so the edit can be reverted.
    StaffEdit { edited_at: String, previous: String },
}

impl Provenance {
    /// The label the editor shows next to a block.
    pub fn label(&self) -> &'static str {
        match self {
            Provenance::Source => "Locked · from Open Dental",
            Provenance::ClinicalNote { .. } => "From clinical note",
            Provenance::Generated { .. } => "AI-written · checked",
            Provenance::StaffEdit { .. } => "Edited by staff",
        }
    }
}

/// Result of validating one block or section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reasons", rename_all = "snake_case")]
pub enum ValidationState {
    Passed,
    /// Content is held back from patients until staff resolve the listed reasons.
    NeedsReview(Vec<String>),
    Failed(Vec<String>),
}

impl ValidationState {
    pub fn is_patient_visible(&self) -> bool {
        matches!(self, ValidationState::Passed)
    }
}
