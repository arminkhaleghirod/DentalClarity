//! In-office engagement analytics (milestone M8): what the patient screens
//! report while Present mode runs, and the staff summary built from it.
//!
//! Only screen names, durations and narration plays are recorded. No text a
//! patient sees, and nothing typed, ever leaves the screen through these
//! types.

use serde::{Deserialize, Serialize};

/// The patient screens of Present mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentScreen {
    AtAGlance,
    Journey,
    Procedures,
    Costs,
    NextSteps,
}

impl PresentScreen {
    pub const ALL: [PresentScreen; 5] = [
        PresentScreen::AtAGlance,
        PresentScreen::Journey,
        PresentScreen::Procedures,
        PresentScreen::Costs,
        PresentScreen::NextSteps,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PresentScreen::AtAGlance => "at_a_glance",
            PresentScreen::Journey => "journey",
            PresentScreen::Procedures => "procedures",
            PresentScreen::Costs => "costs",
            PresentScreen::NextSteps => "next_steps",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            PresentScreen::AtAGlance => "At a glance",
            PresentScreen::Journey => "Journey",
            PresentScreen::Procedures => "Procedures",
            PresentScreen::Costs => "Costs",
            PresentScreen::NextSteps => "Next steps",
        }
    }
}

/// Most seconds one view event may report. The patient screens report every
/// 15 seconds and on each screen change, so a larger value is not genuine.
pub const MAX_VIEW_SECONDS: u32 = 120;

/// Body of `POST /api/present/analytics`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PresentEvent {
    /// The patient spent `seconds` on `screen` since the last report.
    View { screen: PresentScreen, seconds: u32 },
    /// The patient started narration on `screen`.
    Narration { screen: PresentScreen },
}

/// Periods the analytics screen offers, in days.
pub const ANALYTICS_PERIODS: [u32; 3] = [7, 30, 90];

/// `GET /api/staff/analytics?days=N`: activity in the caller's scope over
/// the last `days` days.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsResponse {
    pub days: u32,
    pub generation: GenerationStats,
    pub present: PresentStats,
    /// One entry per screen, in screen order.
    pub screens: Vec<ScreenStats>,
    /// Presentations created in the period, by language and reading level.
    pub languages: Vec<LanguageStats>,
    /// Presentations shown in Present mode, most recently shown first.
    pub presented: Vec<PresentedRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageStats {
    /// BCP 47 tag, such as `en` or `es`.
    pub language: String,
    pub reading_level: crate::content::ReadingLevel,
    pub presentations: u32,
    /// Times presentations in this language and level were shown.
    pub showings: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GenerationStats {
    pub presentations: u32,
    pub succeeded: u32,
    pub needs_review: u32,
    pub failed: u32,
    /// Jobs still queued or running.
    pub in_progress: u32,
    pub sections_passed: u32,
    /// Sections held for staff review or failed, never shown to patients.
    pub sections_held: u32,
    /// Median and 90th percentile time of finished jobs.
    pub p50_seconds: Option<f64>,
    pub p90_seconds: Option<f64>,
    /// Model spend for writing these presentations, in US dollars.
    pub cost_usd: f64,
    pub staff_edits: u32,
    pub notes_confirmed: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PresentStats {
    /// Times Present mode was started.
    pub sessions: u32,
    /// Distinct presentations shown at least once.
    pub presentations: u32,
    /// Time patients spent on the screens.
    pub seconds: u64,
    /// Median time of one showing.
    pub median_seconds: Option<u64>,
    pub narration_plays: u32,
    /// Questions asked in the Ask a question drawer.
    pub questions: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenStats {
    pub screen: PresentScreen,
    pub seconds: u64,
    /// Present sessions that opened this screen.
    pub sessions: u32,
    pub narration_plays: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentedRow {
    pub presentation_id: String,
    pub patient_display_name: String,
    /// RFC 3339, UTC.
    pub last_presented_at: String,
    pub sessions: u32,
    pub seconds: u64,
    /// How many of the five screens the patient opened.
    pub screens_viewed: u32,
    pub narration_plays: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_round_trip_through_their_names() {
        for s in PresentScreen::ALL {
            assert_eq!(PresentScreen::parse(s.as_str()), Some(s));
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(json, format!("\"{}\"", s.as_str()));
        }
        assert_eq!(PresentScreen::parse("settings"), None);
    }

    #[test]
    fn events_are_tagged_by_kind() {
        let e: PresentEvent =
            serde_json::from_str(r#"{"kind":"view","screen":"costs","seconds":15}"#).unwrap();
        assert_eq!(
            e,
            PresentEvent::View {
                screen: PresentScreen::Costs,
                seconds: 15
            }
        );
        assert!(serde_json::from_str::<PresentEvent>(r#"{"kind":"click"}"#).is_err());
    }
}
