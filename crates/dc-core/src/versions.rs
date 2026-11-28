//! API types for version history and plan sync (docs/AI_PIPELINE.md, Sync).

use serde::{Deserialize, Serialize};

/// Why a version exists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionCause {
    /// The first version, written when the presentation was created.
    #[default]
    Created,
    /// The plan in the practice records changed and the poll picked it up.
    PlanChanged,
    /// Staff wrote a section again after the version had been presented
    /// or exported, which froze it.
    Regenerated,
}

impl VersionCause {
    pub fn as_str(self) -> &'static str {
        match self {
            VersionCause::Created => "created",
            VersionCause::PlanChanged => "plan_changed",
            VersionCause::Regenerated => "regenerated",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "plan_changed" => VersionCause::PlanChanged,
            "regenerated" => VersionCause::Regenerated,
            _ => VersionCause::Created,
        }
    }
}

/// What the last poll of the practice records found for a presentation's plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    /// The plan was read; the presentation matches it or a new version was made.
    #[default]
    Ok,
    /// The plan no longer exists in the practice records. The last version
    /// can still be presented.
    PlanDeleted,
    /// The practice records could not be read at the last poll.
    SourceUnavailable,
}

/// One row of `GET /api/staff/presentations/:id/versions`, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionSummary {
    pub version: u32,
    pub cause: VersionCause,
    /// RFC 3339, UTC.
    pub created_at: String,
    /// Staff-readable changes from the version before, e.g.
    /// "Cost of Crown on tooth 30 changed". Empty for the first version.
    pub changes: Vec<String>,
    /// The version staff edit and present now.
    pub current: bool,
    pub presented: bool,
    pub exported: bool,
    /// First 8 characters of the plan snapshot fingerprint.
    pub short_hash: String,
}
