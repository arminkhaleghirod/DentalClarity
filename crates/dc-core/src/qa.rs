//! API types for "Ask a question" in Present mode and the staff view of the
//! conversations (docs/AI_PIPELINE.md, Q&A grounding and retention).

use serde::{Deserialize, Serialize};

/// Longest question a patient may send, in characters.
pub const MAX_QUESTION_CHARS: usize = 500;
/// Questions one present session may ask (PRD FR-QA-08).
pub const QUESTIONS_PER_SESSION: u32 = 20;

/// Shown above the input before and after the first question (FR-QA-02).
pub const NOTICE: &str = "Answers come from your treatment plan and trusted dental sources. \
Your dental team can read this conversation. Please don't share personal details. \
This chat is not for emergencies.";
/// Added to [`NOTICE`] in demo mode (FR-QA-07).
pub const DEMO_NOTICE: &str =
    "This is a demo with made-up patients. Don't enter personal information.";

/// `POST /api/present/qa`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskRequest {
    pub question: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerKind {
    /// Answered from the plan, the presentation and the knowledge base.
    Answered,
    /// Part of the question could be answered; the rest goes to the team.
    Partial,
    /// Declined: diagnosis, treatment advice, prognosis, medication, prices
    /// or anything else for the dental team.
    OutOfScope,
    /// Possibly urgent symptoms: the escalation text is shown first.
    Urgent,
    /// The answer failed a check or could not be produced; a fixed reply
    /// was shown instead.
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QaAnswer {
    pub kind: AnswerKind,
    /// Fixed escalation text, shown before anything else when the question
    /// mentions urgent symptoms or the answer was marked urgent.
    pub urgent_notice: Option<String>,
    /// The answer, one sentence per item. Text only, never markup.
    pub sentences: Vec<String>,
    /// Patient-readable names of what the answer drew on, such as
    /// "Your treatment plan".
    pub sources: Vec<String>,
    /// The answer sends the patient to the dental team.
    pub refer_to_team: bool,
    /// A question the patient could ask the dental team, for declined answers.
    pub suggested_question: Option<String>,
}

/// One question and its answer as the patient saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QaTurn {
    pub question: String,
    pub answer: QaAnswer,
    /// RFC 3339 UTC.
    pub asked_at: String,
}

/// `GET /api/present/qa`: the notice, the limits and this present session's
/// conversation so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QaConversation {
    pub notice: String,
    /// Present in demo mode only.
    pub demo_notice: Option<String>,
    /// False when no AI provider is configured; the input is then disabled.
    pub available: bool,
    pub max_chars: u32,
    pub questions_left: u32,
    pub turns: Vec<QaTurn>,
}

/// `POST /api/present/qa`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskResponse {
    pub turn: QaTurn,
    pub questions_left: u32,
}

/// One stored question as staff review it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaffQaTurn {
    /// RFC 3339 UTC.
    pub asked_at: String,
    /// The presentation version the patient was viewing.
    pub version: u32,
    /// 1 for the first present session that asked a question, and so on.
    pub session: u32,
    pub question: String,
    pub answer: QaAnswer,
    /// The question matched the urgent-symptom lexicon or the answer was
    /// marked urgent.
    pub urgent: bool,
    /// What the checks decided, e.g. "passed" or "blocked: V8, V14".
    pub outcome: String,
}

/// `GET /api/staff/presentations/:id/questions`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaffQuestions {
    pub presentation_id: String,
    /// Any turn was urgent: the presentation carries a staff-visible flag.
    pub urgent: bool,
    /// Oldest first.
    pub turns: Vec<StaffQaTurn>,
}
