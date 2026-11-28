//! "Ask a question" in Present mode, and the staff view of the conversations
//! (docs/AI_PIPELINE.md, Q&A assistant; DECISIONS E24).
//!
//! `/api/present/qa` reads the presentation and version only from the
//! present session, like the other present routes. The model sees the plan's
//! locked facts without money, the presentation's patient-visible text and
//! the knowledge base, never names, keys or dates; names in staff text and
//! in the question are removed before the call and placeholders are filled
//! in locally afterwards. Every answer is checked by the deterministic
//! validators, and a failing answer is replaced by a fixed reply.

use super::present::actor;
use super::presentations::{
    MAX_ID_CHARS, load_sections, not_found, owner_scope, parse_level, provider_not_configured,
};
use crate::generation::usage_cost;
use crate::http::{ClientIp, Failure, JsonBody};
use crate::limits::ip_key;
use crate::security::{CurrentPresent, CurrentSession};
use crate::sessions::PresentSession;
use crate::{AppState, audit, settings};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use dc_ai::{AiError, Message, Role, TextRequest, TextResponse};
use dc_core::Mode;
use dc_core::content::ReadingLevel;
use dc_core::plan::PlanSnapshot;
use dc_core::presentation::SectionVisibility;
use dc_core::qa::{
    AnswerKind, AskRequest, AskResponse, DEMO_NOTICE, MAX_QUESTION_CHARS, NOTICE,
    QUESTIONS_PER_SESSION, QaAnswer, QaConversation, QaTurn, StaffQaTurn, StaffQuestions,
};
use dc_generation::facts::FactSheet;
use dc_generation::notes;
use dc_generation::qa::{self as engine, ApprovedBlock, QaKind, QaOut, QaPrompt, Turn};
use std::time::Duration;

/// Shown first when a question mentions possibly urgent symptoms or the
/// answer was marked urgent. Fixed text, never written by a model.
pub const ESCALATION: &str = "This could need prompt care. Please call {{practice_name}} right away. \
If it feels severe, such as trouble breathing or swallowing or swelling that is spreading, \
call emergency services now.";

/// Shown in place of `{{provider_display_name}}` when the plan has no provider.
const NO_PROVIDER: &str = "your dentist";
const PLAN_SOURCE: &str = "Your treatment plan";

fn question_limit(message: &str, retry_after: Option<u64>) -> Failure {
    Failure {
        retry_after,
        ..Failure::new(StatusCode::TOO_MANY_REQUESTS, "question_limit", message)
    }
}

const SESSION_LIMIT_MESSAGE: &str = "You've reached the number of questions for this visit. \
The front desk will be happy to answer more.";

// Patient routes ---------------------------------------------------------------

/// `GET /api/present/qa`: the notice, the limits and this present session's
/// conversation.
pub async fn conversation(
    State(state): State<AppState>,
    CurrentPresent(present): CurrentPresent,
) -> Result<Json<QaConversation>, Failure> {
    let fill = Filler::load(&state, &present.presentation_id).await?;
    let rows = session_rows(&state, &present.token_hash).await?;
    let asked = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    Ok(Json(QaConversation {
        notice: NOTICE.into(),
        demo_notice: (state.mode == Mode::Demo).then(|| DEMO_NOTICE.into()),
        available: state.generation.is_some(),
        max_chars: MAX_QUESTION_CHARS as u32,
        questions_left: QUESTIONS_PER_SESSION.saturating_sub(asked),
        turns: rows
            .into_iter()
            .map(|r| QaTurn {
                question: r.question,
                answer: fill.answer(r.answer),
                asked_at: r.asked_at,
            })
            .collect(),
    }))
}

/// Limits taken for one question, given back when it is not answered.
struct Charges<'a> {
    state: &'a AppState,
    session: String,
    scope: Option<String>,
    address: Option<String>,
}

impl Charges<'_> {
    fn refund(&self) {
        let limits = &self.state.limits;
        limits.qa_session.refund(&self.session, 1);
        if let Some(scope) = &self.scope {
            limits.qa_scope.refund(scope, 1);
        }
        if let Some(address) = &self.address {
            limits.qa_address.refund(address, 1);
        }
    }
}

/// Counts one question against the per-session limit and, in demo mode, the
/// per-visitor and per-address limits.
async fn charge<'a>(
    state: &'a AppState,
    present: &PresentSession,
    ip: std::net::IpAddr,
) -> Result<Charges<'a>, Failure> {
    let (asked,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM qa_conversations WHERE present_token_hash = ?")
            .bind(&present.token_hash)
            .fetch_one(&state.db)
            .await?;
    if asked >= i64::from(QUESTIONS_PER_SESSION) {
        return Err(question_limit(SESSION_LIMIT_MESSAGE, None));
    }
    let day = Duration::from_secs(24 * 60 * 60);
    // Counted in memory as well, so questions sent at the same moment cannot
    // all pass the stored count.
    state
        .limits
        .qa_session
        .check(&present.token_hash, QUESTIONS_PER_SESSION, day)
        .map_err(|_| question_limit(SESSION_LIMIT_MESSAGE, None))?;
    let mut charges = Charges {
        state,
        session: present.token_hash.clone(),
        scope: None,
        address: None,
    };
    if state.mode == Mode::Demo {
        let busy = "You've asked many questions in a short time. Please wait a little, or ask the front desk.";
        let scope = state.limits.current_scope(&present.staff_token_hash);
        if let Err(wait) = state.limits.qa_scope.check(
            &scope,
            crate::qa::DEMO_SCOPE_PER_HOUR,
            Duration::from_secs(60 * 60),
        ) {
            charges.refund();
            return Err(question_limit(busy, Some(wait)));
        }
        charges.scope = Some(scope);
        let address = ip_key(ip);
        if let Err(wait) =
            state
                .limits
                .qa_address
                .check(&address, crate::qa::DEMO_ADDRESS_PER_DAY, day)
        {
            charges.refund();
            return Err(question_limit(busy, Some(wait)));
        }
        charges.address = Some(address);
    }
    Ok(charges)
}

/// `POST /api/present/qa`: answers one question about the bound version.
pub async fn ask(
    State(state): State<AppState>,
    CurrentPresent(present): CurrentPresent,
    ClientIp(ip): ClientIp,
    JsonBody(request): JsonBody<AskRequest>,
) -> Result<Json<AskResponse>, Failure> {
    let question = request.question.trim().to_owned();
    if question.is_empty() {
        return Err(Failure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "empty_question",
            "Type a question first.",
        ));
    }
    if question.chars().count() > MAX_QUESTION_CHARS {
        return Err(Failure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "question_too_long",
            format!("Please keep questions to {MAX_QUESTION_CHARS} characters."),
        ));
    }
    let charges = charge(&state, &present, ip).await?;
    match answer(&state, &present, &question).await {
        Ok(stored) => {
            let fill = Filler::load(&state, &present.presentation_id).await?;
            let (asked,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM qa_conversations WHERE present_token_hash = ?",
            )
            .bind(&present.token_hash)
            .fetch_one(&state.db)
            .await?;
            let asked = u32::try_from(asked).unwrap_or(u32::MAX);
            Ok(Json(AskResponse {
                turn: QaTurn {
                    question,
                    answer: fill.answer(stored.answer),
                    asked_at: stored.asked_at,
                },
                questions_left: QUESTIONS_PER_SESSION.saturating_sub(asked),
            }))
        }
        Err(failure) => {
            charges.refund();
            Err(failure)
        }
    }
}

/// What the model call came to.
enum Reply {
    Passed(QaOut, TextResponse),
    /// A blocking finding; the issue codes, in order, without repeats.
    Blocked(Vec<&'static str>, TextResponse),
    Unreadable(TextResponse),
    Failed,
    BudgetReached,
    /// No provider is configured.
    NoModel,
}

struct Stored {
    answer: QaAnswer,
    asked_at: String,
}

/// Produces, checks and stores the answer to one question.
async fn answer(
    state: &AppState,
    present: &PresentSession,
    question: &str,
) -> Result<Stored, Failure> {
    let urgent = engine::is_urgent(question);
    let plan = Plan::load(state, present).await?;
    let names = plan.names();
    // An urgent question is answered with the escalation text even when no
    // model may be called.
    let Some(model_question) = clean(question, &plan.practice, &names) else {
        if urgent {
            return store(
                state,
                present,
                question,
                urgent_only(),
                true,
                "personal_details",
                None,
            )
            .await;
        }
        return Err(Failure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "personal_details",
            "Please ask again without names, dates, phone numbers or other personal details.",
        ));
    };
    let prompt = if state.generation.is_some() {
        let history = history(state, present, &plan, &names).await?;
        Some(engine::build(
            &plan.sheet,
            &state.kb,
            plan.level,
            &plan.blocks(&names),
            &history,
            &model_question,
        ))
    } else {
        None
    };
    let reply = match &prompt {
        Some(prompt) => call(state, prompt, &plan).await,
        None => Reply::NoModel,
    };

    let fallback = || QaAnswer {
        kind: AnswerKind::Fallback,
        urgent_notice: None,
        sentences: vec![engine::FALLBACK.into()],
        sources: Vec::new(),
        refer_to_team: true,
        suggested_question: None,
    };
    let (mut answer, outcome, response) = match reply {
        Reply::Passed(out, response) => {
            let prompt = prompt.as_ref().expect("a reply needs a prompt");
            (
                shown(&out, prompt, state),
                "passed".to_owned(),
                Some(response),
            )
        }
        Reply::Blocked(codes, response) => (
            fallback(),
            format!("blocked: {}", codes.join(", ")),
            Some(response),
        ),
        Reply::Unreadable(response) => (fallback(), "unreadable".into(), Some(response)),
        Reply::Failed => (fallback(), "model_error".into(), None),
        Reply::BudgetReached if urgent => (urgent_only(), "budget_reached".into(), None),
        Reply::BudgetReached => {
            return Err(Failure::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "budget_reached",
                "The demo has used its AI budget for this month. Your dental team can answer your question.",
            ));
        }
        Reply::NoModel if urgent => (urgent_only(), "no_model".into(), None),
        Reply::NoModel => return Err(provider_not_configured(state)),
    };
    let urgent = urgent || answer.kind == AnswerKind::Urgent;
    if urgent {
        answer.urgent_notice = Some(ESCALATION.into());
        answer.refer_to_team = true;
    }
    store(
        state,
        present,
        question,
        answer,
        urgent,
        &outcome,
        response.as_ref(),
    )
    .await
}

/// The answer to an urgent question when no model answer is available.
fn urgent_only() -> QaAnswer {
    QaAnswer {
        kind: AnswerKind::Urgent,
        urgent_notice: Some(ESCALATION.into()),
        sentences: Vec::new(),
        sources: Vec::new(),
        refer_to_team: true,
        suggested_question: None,
    }
}

async fn store(
    state: &AppState,
    present: &PresentSession,
    question: &str,
    answer: QaAnswer,
    urgent: bool,
    outcome: &str,
    response: Option<&TextResponse>,
) -> Result<Stored, Failure> {
    let kind = serde_json::to_value(answer.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "fallback".into());
    let (model_id, prompt_version) = match (response, &state.generation) {
        (Some(r), _) => (r.model.clone(), engine::qa_version()),
        (None, Some(g)) => (g.models.writer_model.clone(), engine::qa_version()),
        (None, None) => ("none".into(), engine::qa_version()),
    };
    let usage = response.map(|r| r.usage.clone()).unwrap_or_default();
    let now = crate::sessions::now();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO qa_conversations (presentation_id, version, present_token_hash, asked_at,
             question, answer_json, kind, urgent, outcome, prompt_version, model_id,
             input_tokens, output_tokens, cost_usd)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&present.presentation_id)
    .bind(present.version)
    .bind(&present.token_hash)
    .bind(now)
    .bind(question)
    .bind(serde_json::to_string(&answer).map_err(Failure::internal)?)
    .bind(kind)
    .bind(urgent)
    .bind(outcome)
    .bind(prompt_version)
    .bind(&model_id)
    .bind(i64::from(usage.input_tokens))
    .bind(i64::from(usage.output_tokens))
    .bind(if response.is_some() {
        usage_cost(
            &usage,
            state
                .generation
                .as_ref()
                .map_or("none", |g| g.models.writer.provider()),
            &model_id,
        )
    } else {
        0.0
    })
    .execute(&mut *tx)
    .await?;
    if urgent {
        // The staff-visible flag is the urgent column; the audit event
        // records that one was raised, without the question.
        let detail = format!("presentation {}", present.presentation_id);
        audit::record(&mut *tx, actor(present.staff_kind), "qa_urgent", &detail).await?;
    }
    tx.commit().await?;
    let asked_at: (String,) =
        sqlx::query_as("SELECT strftime('%Y-%m-%dT%H:%M:%SZ', ?, 'unixepoch')")
            .bind(now)
            .fetch_one(&state.db)
            .await?;
    Ok(Stored {
        answer,
        asked_at: asked_at.0,
    })
}

/// One model call, with one retry for a request that never reached the
/// model. The call goes through the budget guard.
async fn call(state: &AppState, prompt: &QaPrompt, plan: &Plan) -> Reply {
    let Some(generation) = &state.generation else {
        return Reply::NoModel;
    };
    let models = &generation.models;
    let request = TextRequest {
        model: models.writer_model.clone(),
        messages: vec![
            Message {
                role: Role::System,
                content: prompt.system.clone(),
            },
            Message {
                role: Role::User,
                content: prompt.user.clone(),
            },
        ],
        response_schema: Some(engine::schema()),
        max_tokens: engine::MAX_OUTPUT_TOKENS,
        temperature: engine::TEMPERATURE,
        reasoning: models.reasoning,
    };
    let mut result = models.writer.generate(request.clone()).await;
    if matches!(result, Err(AiError::Request(_))) {
        result = models.writer.generate(request).await;
    }
    let response = match result {
        Ok(response) => response,
        Err(AiError::Limited(_)) => return Reply::BudgetReached,
        Err(err) => {
            tracing::warn!(%err, "question answer failed");
            return Reply::Failed;
        }
    };
    let out = match engine::parse(&response.content) {
        Ok(out) => out,
        Err(_) => return Reply::Unreadable(response),
    };
    let issues = engine::check(&out, prompt, &plan.sheet, plan.level);
    if issues.is_empty() {
        return Reply::Passed(out, response);
    }
    let mut codes: Vec<&'static str> = Vec::new();
    for issue in &issues {
        if !codes.contains(&issue.code) {
            codes.push(issue.code);
        }
    }
    tracing::info!(codes = ?codes, "question answer blocked; showing the fallback");
    Reply::Blocked(codes, response)
}

/// The patient-facing answer for a reply that passed the checks.
fn shown(out: &QaOut, prompt: &QaPrompt, state: &AppState) -> QaAnswer {
    let kind = match out.kind {
        QaKind::Answered => AnswerKind::Answered,
        QaKind::Partial => AnswerKind::Partial,
        QaKind::OutOfScope => AnswerKind::OutOfScope,
        QaKind::Urgent => AnswerKind::Urgent,
    };
    let mut sources: Vec<String> = Vec::new();
    let mut add = |label: String| {
        if !sources.contains(&label) {
            sources.push(label);
        }
    };
    let cites = out.sentences.iter().flat_map(|s| &s.cites);
    if cites.clone().any(|c| !c.starts_with("kb:")) {
        add(PLAN_SOURCE.into());
    }
    for cite in cites.filter_map(|c| c.strip_prefix("kb:")) {
        if prompt.kb_ids.contains(&format!("kb:{cite}"))
            && let Some(label) = kb_label(state, cite)
        {
            add(label);
        }
    }
    let suggested = out.suggested_question.trim();
    QaAnswer {
        kind,
        urgent_notice: None,
        sentences: out
            .sentences
            .iter()
            .map(|s| s.text.trim().to_owned())
            .collect(),
        sources,
        refer_to_team: out.refer_to_team || matches!(out.kind, QaKind::OutOfScope | QaKind::Urgent),
        suggested_question: (!suggested.is_empty()).then(|| suggested.to_owned()),
    }
}

/// "Root canal (American Association of Endodontists)" for
/// `root_canal#what_it_is`.
fn kb_label(state: &AppState, cite: &str) -> Option<String> {
    let (entry_id, passage_id) = cite.split_once('#')?;
    let entry = state.kb.entries().find(|e| e.id == entry_id)?;
    let passage = entry.passages.iter().find(|p| p.id == passage_id)?;
    let mut publishers: Vec<&str> = Vec::new();
    for id in &passage.sources {
        if let Some(source) = entry.sources.iter().find(|s| &s.id == id)
            && !publishers.contains(&source.publisher.as_str())
        {
            publishers.push(&source.publisher);
        }
    }
    Some(if publishers.is_empty() {
        entry.title.clone()
    } else {
        format!("{} ({})", entry.title, publishers.join(", "))
    })
}

/// The presentation version a present session is bound to.
struct Plan {
    snapshot: PlanSnapshot,
    sheet: FactSheet,
    level: ReadingLevel,
    sections: Vec<dc_core::presentation::SectionView>,
    practice: String,
}

impl Plan {
    async fn load(state: &AppState, present: &PresentSession) -> Result<Plan, Failure> {
        // Read through the version history, so a plan change during Present
        // mode leaves the questions answered from the version shown.
        let data =
            crate::versions::load(&state.db, &present.presentation_id, Some(present.version))
                .await?
                .ok_or_else(|| not_found("presentation"))?;
        let level = data.reading_level.clone();
        let version = data.version;
        let snapshot: PlanSnapshot =
            serde_json::from_str(&data.snapshot_json).map_err(Failure::internal)?;
        let sheet = FactSheet::build(&snapshot, &state.kb);
        let sections =
            load_sections(state, &present.presentation_id, version, data.rows, &sheet).await?;
        Ok(Plan {
            practice: settings::practice_name(state).await?,
            level: parse_level(&level),
            snapshot,
            sheet,
            sections,
        })
    }

    /// Names that must not reach the model: patient and provider.
    fn names(&self) -> Vec<String> {
        let mut names = vec![
            self.snapshot.patient.first_name.clone(),
            self.snapshot.patient.last_name.clone(),
        ];
        names.extend(self.snapshot.provider_display_name.clone());
        names.retain(|n| !n.trim().is_empty());
        names
    }

    /// The text patients can see: ready sections, without note-derived
    /// blocks that staff have not confirmed. Staff text is stripped of names.
    fn blocks(&self, names: &[String]) -> Vec<ApprovedBlock> {
        self.sections
            .iter()
            .filter(|s| s.visibility == SectionVisibility::Ready)
            .flat_map(|s| {
                s.blocks
                    .iter()
                    .filter(|b| !b.needs_confirmation && !b.sentences.is_empty())
                    .filter_map(|b| {
                        let text = clean(&b.sentences.join(" "), &self.practice, names)?;
                        Some(ApprovedBlock {
                            section_title: s.title.clone(),
                            heading: b.heading.clone(),
                            sentences: vec![text],
                        })
                    })
            })
            .collect()
    }
}

/// Text with the practice name replaced by its placeholder and names, dates,
/// contact details and long numbers removed. `None` when too little is left
/// (the note sanitizer's rule).
fn clean(text: &str, practice: &str, names: &[String]) -> Option<String> {
    let text = if practice.trim().is_empty() {
        text.to_owned()
    } else {
        text.replace(practice, "{{practice_name}}")
    };
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    notes::sanitize(&text, &names)
}

/// Earlier turns of this present session for the prompt, oldest first.
async fn history(
    state: &AppState,
    present: &PresentSession,
    plan: &Plan,
    names: &[String],
) -> Result<Vec<Turn>, Failure> {
    let rows = session_rows(state, &present.token_hash).await?;
    Ok(rows
        .into_iter()
        .rev()
        .take(engine::HISTORY_TURNS)
        .rev()
        .filter_map(|r| {
            let question = clean(&r.question, &plan.practice, names)?;
            Some(Turn {
                question,
                answer: r.answer.sentences.join(" "),
            })
        })
        .collect())
}

struct Row {
    question: String,
    answer: QaAnswer,
    asked_at: String,
}

async fn session_rows(state: &AppState, token_hash: &str) -> Result<Vec<Row>, Failure> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT question, answer_json, strftime('%Y-%m-%dT%H:%M:%SZ', asked_at, 'unixepoch')
         FROM qa_conversations WHERE present_token_hash = ? ORDER BY id",
    )
    .bind(token_hash)
    .fetch_all(&state.db)
    .await?;
    rows.into_iter()
        .map(|(question, answer, asked_at)| {
            Ok(Row {
                question,
                answer: serde_json::from_str(&answer).map_err(Failure::internal)?,
                asked_at,
            })
        })
        .collect()
}

/// Local placeholder substitution for stored answers.
struct Filler {
    practice: String,
    provider: Option<String>,
}

impl Filler {
    async fn load(state: &AppState, presentation_id: &str) -> Result<Filler, Failure> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT snapshot_json FROM presentations WHERE id = ?")
                .bind(presentation_id)
                .fetch_optional(&state.db)
                .await?;
        let Some((snapshot,)) = row else {
            return Err(not_found("presentation"));
        };
        let snapshot: PlanSnapshot = serde_json::from_str(&snapshot).map_err(Failure::internal)?;
        Ok(Filler {
            practice: settings::practice_name(state).await?,
            provider: snapshot.provider_display_name,
        })
    }

    fn text(&self, text: &str) -> String {
        const PROVIDER: &str = "{{provider_display_name}}";
        let mut text = text.replace("{{practice_name}}", &self.practice);
        match &self.provider {
            Some(name) => text = text.replace(PROVIDER, name),
            None => {
                if let Some(rest) = text.strip_prefix(PROVIDER) {
                    text = format!("Your dentist{rest}");
                }
                text = text.replace(PROVIDER, NO_PROVIDER);
            }
        }
        text
    }

    fn answer(&self, answer: QaAnswer) -> QaAnswer {
        QaAnswer {
            urgent_notice: answer.urgent_notice.map(|t| self.text(&t)),
            sentences: answer.sentences.iter().map(|s| self.text(s)).collect(),
            suggested_question: answer.suggested_question.map(|q| self.text(&q)),
            ..answer
        }
    }
}

// Staff route ------------------------------------------------------------------

/// `GET /api/staff/presentations/{id}/questions`: every question asked about
/// this presentation, oldest first, for staff review (PRD FR-QA-06).
pub async fn staff_questions(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Path(id): Path<String>,
) -> Result<Json<StaffQuestions>, Failure> {
    if id.len() > MAX_ID_CHARS {
        return Err(not_found("presentation"));
    }
    let owned: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM presentations WHERE id = ? AND owner_scope = ?")
            .bind(&id)
            .bind(owner_scope(&state, &session))
            .fetch_optional(&state.db)
            .await?;
    if owned.is_none() {
        return Err(not_found("presentation"));
    }
    let fill = Filler::load(&state, &id).await?;
    let rows: Vec<(i64, String, String, String, bool, String, String)> = sqlx::query_as(
        "SELECT version, strftime('%Y-%m-%dT%H:%M:%SZ', asked_at, 'unixepoch'), question,
             answer_json, urgent, outcome, present_token_hash
         FROM qa_conversations WHERE presentation_id = ? ORDER BY id",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    let mut sessions: Vec<String> = Vec::new();
    let mut turns = Vec::with_capacity(rows.len());
    for (version, asked_at, question, answer, urgent, outcome, token_hash) in rows {
        let session = match sessions.iter().position(|s| *s == token_hash) {
            Some(i) => i + 1,
            None => {
                sessions.push(token_hash);
                sessions.len()
            }
        };
        let answer: QaAnswer = serde_json::from_str(&answer).map_err(Failure::internal)?;
        turns.push(StaffQaTurn {
            asked_at,
            version: u32::try_from(version).unwrap_or_default(),
            session: u32::try_from(session).unwrap_or(u32::MAX),
            question,
            answer: fill.answer(answer),
            urgent,
            outcome,
        });
    }
    Ok(Json(StaffQuestions {
        presentation_id: id,
        urgent: turns.iter().any(|t| t.urgent),
        turns,
    }))
}
