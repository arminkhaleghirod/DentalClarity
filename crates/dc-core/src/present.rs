//! API types for Present mode, the patient view, staff edits and narration.

use crate::content::ReadingLevel;
use crate::money::Money;
use crate::presentation::{BlockSource, ProcedureFactView};
use serde::{Deserialize, Serialize};

// Staff edits ---------------------------------------------------------------

/// `PUT /api/staff/presentations/:id/blocks`: replaces one block's sentences.
/// The block becomes `BlockSource::StaffEdit`; the original stays stored so
/// it can be restored. Edits are checked by the same validators, as warnings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateBlockRequest {
    pub section_key: String,
    /// The `TextBlock::field` being edited, e.g. `what_happens`.
    pub field: String,
    pub sentences: Vec<String>,
}

/// `POST /api/staff/presentations/:id/blocks/revert`: restores the generated text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevertBlockRequest {
    pub section_key: String,
    pub field: String,
}

/// `POST /api/staff/presentations/:id/confirm-note`: staff confirm that a
/// note-derived block reads correctly, so patients may see it (DECISIONS P11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmNoteRequest {
    pub section_key: String,
    pub field: String,
}

/// Result of an edit, revert or confirm: warnings from the validators
/// (never blocking for staff text) and the updated section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockChangeResponse {
    pub warnings: Vec<String>,
    pub section: crate::presentation::SectionView,
}

// Present mode --------------------------------------------------------------

/// `POST /api/staff/presentations/:id/present`. Locks the caller's staff
/// session on the server and sets the present-session cookie. The browser
/// then does a full page load of `/present`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPresentResponse {
    pub present_path: String,
}

/// `POST /api/present/exit`. Clinical mode needs the staff PIN, or the
/// staff password after five wrong PINs. Demo mode needs neither.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitPresentRequest {
    pub pin: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitPresentResponse {
    /// Where the browser goes next, normally the presentation's editor.
    pub staff_path: String,
    /// Fresh CSRF token for the unlocked staff session.
    pub csrf_token: String,
}

/// Practice branding for the patient screens. No DentalClarity branding is shown there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PracticeBrand {
    pub name: String,
    /// `#RRGGBB`, contrast-checked against white text when saved.
    pub primary: String,
    /// `#RRGGBB`, used for "your teeth" highlights.
    pub highlight: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatientSentence {
    pub text: String,
    /// Key for `GET /api/present/audio/:key`. Derived from the exact text,
    /// language and voice, so any edit changes it.
    pub audio_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatientBlock {
    pub field: String,
    pub heading: String,
    pub sentences: Vec<PatientSentence>,
    pub source: BlockSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatientSectionKind {
    AtAGlance,
    Journey,
    Procedure,
    NextSteps,
}

/// One section as the patient sees it. Only content that passed checks and
/// review, or that staff wrote or confirmed, ever appears here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatientSection {
    pub key: String,
    pub kind: PatientSectionKind,
    pub title: String,
    pub procedure_ref: Option<String>,
    pub blocks: Vec<PatientBlock>,
    pub questions: Vec<String>,
}

/// `GET /api/present/content`: everything the patient screens need, for the
/// version bound to the present session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatientView {
    pub brand: PracticeBrand,
    /// Greeting uses this; it is rendered locally and never sent to a model.
    pub patient_first_name: String,
    pub guardian_audience: bool,
    pub provider_display_name: Option<String>,
    pub language: String,
    pub reading_level: ReadingLevel,
    /// Languages with content for this version. English only until M7.
    pub available_languages: Vec<String>,
    pub sections: Vec<PatientSection>,
    pub procedures: Vec<ProcedureFactView>,
    /// Teeth with planned work, e.g. `["14", "19", "30"]`, for the chart.
    pub affected_teeth: Vec<String>,
    pub total_fee: Option<Money>,
    pub total_insurance_estimate: Option<Money>,
    pub total_patient_estimate: Option<Money>,
    /// True in demo mode: exit needs no PIN, and screens show a demo note.
    pub demo: bool,
}

/// Cache key for one narrated sentence: SHA-256 over the speech model, the
/// language and the exact text. Any edit to the text, or a change of voice or
/// language, gives a new key, so stale audio is never played.
pub fn audio_key(speech_model: &str, language: &str, text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in [speech_model, language, text] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Most sentences in one `GET /api/present/narration` clip. A longer block is
/// played as consecutive parts of this many sentences, and pre-warmed the
/// same way, so the merged clips match.
pub const MAX_NARRATION_SENTENCES: usize = 12;

/// Response header of `GET /api/present/narration`: the start of each
/// sentence in the merged clip, in whole milliseconds from the start,
/// comma-separated, e.g. `0,4120,8590`. Computed from sample counts.
pub const SENTENCE_OFFSETS_HEADER: &str = "x-dc-sentence-offsets";

/// Response header of `GET /api/present/narration`: the clip's length in
/// whole milliseconds.
pub const NARRATION_DURATION_HEADER: &str = "x-dc-duration-ms";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_keys_change_with_any_input() {
        let a = audio_key("voice", "en", "The area is numbed first.");
        assert_eq!(a.len(), 64);
        assert_eq!(a, audio_key("voice", "en", "The area is numbed first."));
        assert_ne!(a, audio_key("voice", "en", "The area is numbed first"));
        assert_ne!(a, audio_key("voice", "es", "The area is numbed first."));
        assert_ne!(a, audio_key("other", "en", "The area is numbed first."));
    }
}
