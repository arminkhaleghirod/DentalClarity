//! The generation pipeline (docs/AI_PIPELINE.md, stages S1 to S8).
//!
//! For each section: write, validate deterministically, review with a second
//! model call, and repair at most twice. Sections run in parallel. Nothing
//! here touches the database; the caller persists results and reports
//! progress through [`Progress`]. Runs on the Tokio runtime.

use crate::facts::FactSheet;
use crate::prompt::{self, SectionKind, SectionPrompt};
use crate::schema::{
    AtAGlanceOut, JourneyOut, NextStepsOut, ProcedureOut, ReviewVerdict, Severity, Verdict, WhyKind,
};
use crate::validate::{self, Context, Issue, IssueSeverity};
use dc_ai::{AiError, Message, ReasoningEffort, Role, TextModel, TextRequest, Usage};
use dc_core::PlanSnapshot;
use dc_core::content::ReadingLevel;
use dc_core::procedure::ProcedureCategory;
use dc_knowledge::KnowledgeBase;
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// At most this many repair rounds per section, shared by validator and reviewer findings.
pub const MAX_REPAIRS: usize = 2;
/// Sections generated at the same time for one presentation.
pub const PARALLEL_SECTIONS: usize = 12;
/// Room for hidden reasoning plus the JSON reply. Reasoning tokens count
/// toward this limit, and a cut-off reply is unreadable.
pub const MAX_OUTPUT_TOKENS: u32 = 24_000;
/// Sampling temperature for every writer and reviewer call.
pub const TEMPERATURE: f32 = 0.2;

#[derive(Clone)]
pub struct Models {
    pub writer: Arc<dyn TextModel>,
    pub writer_model: String,
    pub reviewer: Arc<dyn TextModel>,
    pub reviewer_model: String,
    pub reasoning: Option<ReasoningEffort>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionState {
    /// Passed validation and review; may be shown to patients.
    Passed,
    /// Held for staff: review findings, an unsupported procedure, or a failed reviewer.
    NeedsReview,
    /// Still failing deterministic checks after all repairs; never shown.
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct SectionResult {
    pub key: String,
    pub state: SectionState,
    /// The accepted model output, if any.
    pub output: Option<Value>,
    /// Reasons a section is held or failed, readable by staff.
    pub reasons: Vec<String>,
    /// The reason block came from a clinical note and needs one staff confirmation (P11).
    pub needs_note_confirmation: bool,
    pub writer_model: String,
    pub prompt_version: String,
    pub attempts: u32,
    /// Why each repair round happened, for staff and evaluation.
    pub attempt_log: Vec<String>,
    pub usage: Usage,
    pub elapsed_ms: u64,
}

/// Progress events for the job log and the create screen.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Started {
        sections: Vec<String>,
    },
    SectionStage {
        key: String,
        stage: &'static str,
        attempt: u32,
    },
    SectionDone {
        key: String,
        state: SectionState,
    },
}

pub trait Progress: Send + Sync {
    fn report(&self, event: Event);
}

impl<F: Fn(Event) + Send + Sync> Progress for F {
    fn report(&self, event: Event) {
        self(event)
    }
}

pub struct Generated {
    pub sheet: FactSheet,
    pub sections: Vec<SectionResult>,
    pub elapsed_ms: u64,
}

impl Generated {
    pub fn total_cost_usd(&self) -> f64 {
        self.sections.iter().filter_map(|s| s.usage.cost_usd).sum()
    }
}

/// The sections a plan needs, in presentation order.
pub fn sections_for(sheet: &FactSheet) -> Vec<SectionKind> {
    let mut kinds = vec![SectionKind::AtAGlance, SectionKind::Journey];
    kinds.extend(
        sheet
            .procedures
            .iter()
            .map(|p| SectionKind::Procedure(p.reference.clone())),
    );
    kinds.push(SectionKind::NextSteps);
    kinds
}

pub async fn generate(
    snapshot: &PlanSnapshot,
    kb: &KnowledgeBase,
    models: &Models,
    level: ReadingLevel,
    progress: &dyn Progress,
) -> Generated {
    let started = Instant::now();
    let sheet = FactSheet::build(snapshot, kb);
    let kinds = sections_for(&sheet);
    progress.report(Event::Started {
        sections: kinds.iter().map(SectionKind::key).collect(),
    });

    let mut results: Vec<(usize, SectionResult)> =
        futures::stream::iter(kinds.into_iter().enumerate())
            .map(|(i, kind)| {
                let sheet = &sheet;
                async move { (i, section(&kind, sheet, kb, models, level, progress).await) }
            })
            .buffer_unordered(PARALLEL_SECTIONS)
            .collect()
            .await;
    results.sort_by_key(|(i, _)| *i);

    Generated {
        sections: results.into_iter().map(|(_, r)| r).collect(),
        sheet,
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

/// Runs one section through write, validate, review and repair. [`generate`]
/// calls this for every section; ClarityEval calls it for single-section tasks.
pub async fn section(
    kind: &SectionKind,
    sheet: &FactSheet,
    kb: &KnowledgeBase,
    models: &Models,
    level: ReadingLevel,
    progress: &dyn Progress,
) -> SectionResult {
    let started = Instant::now();
    let key = kind.key();
    let mut result = SectionResult {
        key: key.clone(),
        state: SectionState::Failed,
        output: None,
        reasons: Vec::new(),
        needs_note_confirmation: false,
        writer_model: models.writer_model.clone(),
        prompt_version: prompt::writer_version(),
        attempts: 0,
        attempt_log: Vec::new(),
        usage: Usage::default(),
        elapsed_ms: 0,
    };

    // S1: a procedure with no knowledge entry is never improvised.
    if !has_knowledge(kind, sheet, kb) {
        result.state = SectionState::NeedsReview;
        result
            .reasons
            .push("No reviewed educational content exists for this procedure yet.".into());
        progress.report(Event::SectionDone {
            key,
            state: result.state,
        });
        return result;
    }

    let section_prompt = prompt::build(kind, sheet, kb, level);
    let cx = Context {
        sheet,
        kb_ids: section_prompt.kb_ids.clone(),
        note_ids: section_prompt.note_ids.clone(),
        level,
    };
    let mut messages = vec![
        Message {
            role: Role::System,
            content: prompt::WRITER_PROMPT.into(),
        },
        Message {
            role: Role::User,
            content: section_prompt.user.clone(),
        },
    ];

    // The last draft that passed validation, with whether its reviewer findings
    // were safety findings and the reasons to show staff. A repair that cannot
    // reach the writer falls back to it.
    let mut validated: Option<(Value, bool, Vec<String>)> = None;

    for attempt in 0..=MAX_REPAIRS {
        result.attempts = attempt as u32 + 1;
        progress.report(Event::SectionStage {
            key: key.clone(),
            stage: if attempt == 0 { "writing" } else { "repairing" },
            attempt: result.attempts,
        });

        let reply = match call(
            &models.writer,
            &models.writer_model,
            messages.clone(),
            kind.schema(),
            models.reasoning,
        )
        .await
        {
            Ok(reply) => reply,
            Err(err) => {
                let reason = format!("The writing model could not be reached ({err}).");
                result
                    .attempt_log
                    .push(format!("attempt {}: {reason}", result.attempts));
                match validated.take() {
                    Some((value, holds, reasons)) => {
                        result.state = if holds {
                            SectionState::NeedsReview
                        } else {
                            SectionState::Passed
                        };
                        result.reasons = if holds { reasons } else { Vec::new() };
                        result.output = Some(value);
                    }
                    None => {
                        result.reasons = vec![reason];
                        result.state = SectionState::Failed;
                    }
                }
                break;
            }
        };
        add_usage(&mut result.usage, &reply.usage);
        let raw = strip_fences(&reply.content);

        // S5: deterministic validation.
        let parsed = parse_and_validate(kind, raw, &cx);
        let issues = match &parsed {
            Ok((_, issues)) => issues.clone(),
            Err(issue) => vec![issue.clone()],
        };
        if validate::has_blocking(&issues) || parsed.is_err() {
            result.state = SectionState::Failed;
            result.reasons = issues
                .iter()
                .filter(|i| i.severity == IssueSeverity::Block)
                .map(describe)
                .collect();
            result.attempt_log.push(format!(
                "attempt {}: checks failed: {}",
                result.attempts,
                result.reasons.join("; ")
            ));
            if attempt < MAX_REPAIRS {
                messages.push(Message {
                    role: Role::Assistant,
                    content: raw.to_owned(),
                });
                messages.push(Message {
                    role: Role::User,
                    content: repair_request(&issues),
                });
                continue;
            }
            break;
        }
        let (value, issues) = parsed.expect("checked above");

        // S6: independent review pass.
        progress.report(Event::SectionStage {
            key: key.clone(),
            stage: "reviewing",
            attempt: result.attempts,
        });
        let verdict = review(models, &section_prompt, raw, &mut result.usage).await;
        let warnings: Vec<&Issue> = issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Warn)
            .collect();
        match verdict {
            Ok(v) if v.verdict == Verdict::Pass => {
                result.state = SectionState::Passed;
                result.reasons.clear();
                result.output = Some(value);
                break;
            }
            Ok(v) => {
                let critical = v.verdict == Verdict::Block
                    || v.issues.iter().any(|i| i.severity == Severity::Critical);
                // Only safety findings may hold a section from patients:
                // faithfulness, patient-specific claims, scope, balance and
                // certainty (R1 to R5) at major or critical severity. Clarity
                // and audience notes (R6, R7) and minor findings get one repair
                // attempt, then the section passes with the note recorded.
                let holds = critical || v.issues.iter().any(is_safety_finding);
                if !holds && attempt >= 1 {
                    result.attempt_log.push(format!(
                        "attempt {}: reviewer notes accepted after a repair: {}",
                        result.attempts,
                        v.issues
                            .iter()
                            .map(|i| format!("{} {}", i.category, i.path))
                            .collect::<Vec<_>>()
                            .join("; ")
                    ));
                    result.state = SectionState::Passed;
                    result.reasons.clear();
                    result.output = Some(value);
                    break;
                }
                result.reasons = v
                    .issues
                    .iter()
                    .map(|i| format!("{} {}: {}", i.category, i.path, i.explanation))
                    .collect();
                result.attempt_log.push(format!(
                    "attempt {}: reviewer {:?}: {}",
                    result.attempts,
                    v.verdict,
                    result.reasons.join("; ")
                ));
                if attempt < MAX_REPAIRS {
                    messages.push(Message {
                        role: Role::Assistant,
                        content: raw.to_owned(),
                    });
                    let mut request = String::from(
                        "A reviewer found these problems. Fix them and return the full corrected JSON.\n",
                    );
                    for i in &v.issues {
                        request.push_str(&format!(
                            "- {} ({}, {:?}): {}\n",
                            i.path, i.category, i.severity, i.explanation
                        ));
                    }
                    for w in &warnings {
                        request.push_str(&format!("- {}: {}\n", w.path, w.detail));
                    }
                    messages.push(Message {
                        role: Role::User,
                        content: request,
                    });
                    // A block verdict gets one repair, not two.
                    if critical && attempt >= 1 {
                        result.state = SectionState::NeedsReview;
                        result.output = Some(value);
                        break;
                    }
                    validated = Some((value, holds, result.reasons.clone()));
                    continue;
                }
                result.state = SectionState::NeedsReview;
                result.output = Some(value);
                break;
            }
            Err(reason) => {
                result
                    .attempt_log
                    .push(format!("attempt {}: {reason}", result.attempts));
                result.state = SectionState::NeedsReview;
                result.reasons = vec![reason];
                result.output = Some(value);
                break;
            }
        }
    }

    // P11: a reason taken from a clinical note waits for one staff confirmation.
    if let Some(output) = &result.output
        && output.pointer("/why/kind").and_then(Value::as_str) == Some("documented")
    {
        result.needs_note_confirmation = true;
    }
    result.elapsed_ms = started.elapsed().as_millis() as u64;
    progress.report(Event::SectionDone {
        key,
        state: result.state,
    });
    result
}

/// Whether a section can be written at all. A procedure whose code maps to
/// no knowledge entry is never improvised (S1); every other kind can.
pub fn has_knowledge(kind: &SectionKind, sheet: &FactSheet, kb: &KnowledgeBase) -> bool {
    let SectionKind::Procedure(reference) = kind else {
        return true;
    };
    let category = sheet.procedure(reference).map(|p| p.category);
    category != Some(ProcedureCategory::Unsupported) && category.and_then(|c| kb.entry(c)).is_some()
}

/// Runs the deterministic validators (S5) again over a stored section
/// output, with the citations the section's prompt allows. Used before staff
/// release a held section, so a release never skips a blocking issue.
pub fn check_output(
    kind: &SectionKind,
    output: &Value,
    sheet: &FactSheet,
    kb: &KnowledgeBase,
    level: ReadingLevel,
) -> Vec<Issue> {
    let section_prompt = prompt::build(kind, sheet, kb, level);
    let cx = Context {
        sheet,
        kb_ids: section_prompt.kb_ids,
        note_ids: section_prompt.note_ids,
        level,
    };
    match validate_value(kind, output.clone(), &cx) {
        Ok((_, issues)) => issues,
        Err(issue) => vec![issue],
    }
}

fn parse_and_validate(
    kind: &SectionKind,
    raw: &str,
    cx: &Context,
) -> Result<(Value, Vec<Issue>), Issue> {
    let value: Value = serde_json::from_str(raw).map_err(schema_issue)?;
    validate_value(kind, value, cx)
}

fn schema_issue(e: serde_json::Error) -> Issue {
    Issue {
        code: "V1",
        path: "/".into(),
        severity: IssueSeverity::Block,
        detail: format!("the reply is not valid JSON for this schema: {e}"),
    }
}

fn validate_value(
    kind: &SectionKind,
    value: Value,
    cx: &Context,
) -> Result<(Value, Vec<Issue>), Issue> {
    let bad = schema_issue;
    let issues = match kind {
        SectionKind::AtAGlance => validate::check_at_a_glance(
            &serde_json::from_value::<AtAGlanceOut>(value.clone()).map_err(bad)?,
            cx,
        ),
        SectionKind::Journey => validate::check_journey(
            &serde_json::from_value::<JourneyOut>(value.clone()).map_err(bad)?,
            cx,
        ),
        SectionKind::Procedure(reference) => {
            let out: ProcedureOut = serde_json::from_value(value.clone()).map_err(bad)?;
            let mut issues = validate::check_procedure(&out, reference, cx);
            if out.why.kind == WhyKind::Documented
                && !cx.note_ids.contains(&format!("note:{reference}"))
            {
                issues.push(Issue {
                    code: "V4",
                    path: "/why".into(),
                    severity: IssueSeverity::Block,
                    detail: "no note was provided".into(),
                });
            }
            issues
        }
        SectionKind::NextSteps => validate::check_next_steps(
            &serde_json::from_value::<NextStepsOut>(value.clone()).map_err(bad)?,
            cx,
        ),
    };
    Ok((value, issues))
}

async fn review(
    models: &Models,
    section: &SectionPrompt,
    draft: &str,
    usage: &mut Usage,
) -> Result<ReviewVerdict, String> {
    let messages = vec![
        Message {
            role: Role::System,
            content: prompt::REVIEWER_PROMPT.into(),
        },
        Message {
            role: Role::User,
            content: prompt::review_user(section, draft),
        },
    ];
    let reply = call(
        &models.reviewer,
        &models.reviewer_model,
        messages,
        crate::schema::review_schema(),
        models.reasoning,
    )
    .await
    .map_err(|e| format!("The review model could not be reached ({e})."))?;
    add_usage(usage, &reply.usage);
    let verdict: ReviewVerdict = serde_json::from_str(strip_fences(&reply.content))
        .map_err(|_| "The review model returned an unreadable verdict.".to_owned())?;
    if verdict.verdict == Verdict::Pass && !verdict.issues.is_empty() {
        // Treat a pass with findings as a revise.
        return Ok(ReviewVerdict {
            verdict: Verdict::Revise,
            issues: verdict.issues,
        });
    }
    Ok(verdict)
}

/// One model call with jittered backoff: up to three attempts for transport
/// errors, but only two when the provider timed out or replied with something
/// unusable, because those calls may be billed.
async fn call(
    model: &Arc<dyn TextModel>,
    model_id: &str,
    messages: Vec<Message>,
    schema: Value,
    reasoning: Option<ReasoningEffort>,
) -> Result<dc_ai::TextResponse, AiError> {
    let request = TextRequest {
        model: model_id.to_owned(),
        messages,
        response_schema: Some(schema),
        max_tokens: MAX_OUTPUT_TOKENS,
        temperature: TEMPERATURE,
        reasoning,
    };
    let mut last = None;
    let mut billed_failures = 0;
    for attempt in 0..3u32 {
        match model.generate(request.clone()).await {
            Ok(reply) => return Ok(reply),
            Err(err @ AiError::NotAllowed { .. }) => return Err(err),
            Err(err @ AiError::BadResponse(_)) if billed_failures >= 1 => return Err(err),
            Err(err) => {
                if matches!(err, AiError::BadResponse(_)) {
                    billed_failures += 1;
                }
                tracing::warn!(%err, attempt, "model call failed; retrying");
                last = Some(err);
                let jitter = u64::from(std::process::id() % 250);
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt) + jitter)).await;
            }
        }
    }
    Err(last.expect("at least one attempt"))
}

/// A reviewer finding that is about safety (R1 to R5) rather than style,
/// at major or critical severity.
fn is_safety_finding(issue: &crate::schema::ReviewIssue) -> bool {
    let rubric: u32 = issue
        .category
        .trim()
        .trim_start_matches(['R', 'r'])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    // An unrecognized category is treated as a safety finding, to stay on the safe side.
    let safety_category = rubric == 0 || (1..=5).contains(&rubric);
    safety_category && issue.severity != Severity::Minor
}

fn add_usage(total: &mut Usage, more: &Usage) {
    total.input_tokens += more.input_tokens;
    total.output_tokens += more.output_tokens;
    total.cost_usd = match (total.cost_usd, more.cost_usd) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    };
}

fn strip_fences(text: &str) -> &str {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

fn describe(issue: &Issue) -> String {
    format!("{} at {}: {}", issue.code, issue.path, issue.detail)
}

fn repair_request(issues: &[Issue]) -> String {
    let mut out = String::from(
        "Your JSON did not pass the automatic checks. Fix every problem below and return the \
         full corrected JSON object only.\n",
    );
    for issue in issues {
        out.push_str(&format!(
            "- {}: {} ({})\n",
            issue.path, issue.detail, issue.code
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{daniel, kb_with_entry};
    use dc_ai::fake::FakeTextModel;
    use serde_json::json;
    use std::sync::Mutex;

    fn s(text: &str, cite: &str) -> Value {
        json!({"text": text, "cites": [cite]})
    }

    fn good_procedure() -> String {
        let k = "kb:root_canal#what_it_is";
        json!({
            "procedure_ref": "p1",
            "what_it_is": [s("A root canal treats the soft inside of a tooth.", k)],
            "why": {"kind": "documented", "sentences": [s("Your dentist found decay that reached the nerve of this tooth.", "note:p1")]},
            "what_happens": [s("The area is numbed before treatment starts.", "kb:root_canal#what_happens")],
            "recovery": [s("Many people feel tender for a short while.", "kb:root_canal#recovery")],
            "benefits": [s("It can let you keep your natural tooth.", "kb:root_canal#benefits")],
            "if_delayed": [s("An infection inside a tooth can spread over time.", "kb:root_canal#if_delayed")],
            "questions": [{"text": "Will I need a crown afterwards?", "cites": []}, {"text": "How should I care for the tooth?", "cites": []}]
        })
        .to_string()
    }

    fn models(writer: FakeTextModel, reviewer: FakeTextModel) -> Models {
        Models {
            writer: Arc::new(writer),
            writer_model: "writer".into(),
            reviewer: Arc::new(reviewer),
            reviewer_model: "reviewer".into(),
            reasoning: None,
        }
    }

    fn pass() -> String {
        json!({"verdict": "pass", "issues": []}).to_string()
    }

    async fn run_procedure(writer: Vec<String>, reviewer: Vec<String>) -> SectionResult {
        let kb = kb_with_entry();
        let sheet = FactSheet::build(&daniel(), &kb);
        let events = Mutex::new(Vec::new());
        let progress = |e: Event| events.lock().unwrap().push(e);
        let m = models(
            FakeTextModel::with_responses(writer),
            FakeTextModel::with_responses(reviewer),
        );
        section(
            &SectionKind::Procedure("p1".into()),
            &sheet,
            &kb,
            &m,
            ReadingLevel::Standard,
            &progress,
        )
        .await
    }

    #[tokio::test]
    async fn clean_output_passes_and_flags_note_confirmation() {
        let result = run_procedure(vec![good_procedure()], vec![pass()]).await;
        assert_eq!(result.state, SectionState::Passed, "{:?}", result.reasons);
        assert!(result.needs_note_confirmation);
        assert_eq!(result.attempts, 1);
    }

    #[tokio::test]
    async fn validator_failure_is_repaired() {
        let bad = good_procedure().replace(
            "Many people feel tender for a short while.",
            "Recovery takes two days and costs $40.",
        );
        let result = run_procedure(vec![bad, good_procedure()], vec![pass()]).await;
        assert_eq!(result.state, SectionState::Passed);
        assert_eq!(result.attempts, 2);
    }

    #[tokio::test]
    async fn persistent_failure_never_passes() {
        let bad = good_procedure().replace(
            "It can let you keep your natural tooth.",
            "Success is guaranteed.",
        );
        let result = run_procedure(vec![bad.clone(), bad.clone(), bad], vec![]).await;
        assert_eq!(result.state, SectionState::Failed);
        assert!(result.output.is_none());
        assert!(result.reasons.iter().any(|r| r.contains("V11")));
    }

    #[tokio::test]
    async fn clarity_notes_pass_after_one_repair() {
        let style = json!({"verdict": "revise", "issues": [{"path": "/what_it_is/0", "category": "R6", "severity": "major", "explanation": "Unexplained abbreviation."}]}).to_string();
        let result = run_procedure(
            vec![good_procedure(), good_procedure()],
            vec![style.clone(), style],
        )
        .await;
        assert_eq!(
            result.state,
            SectionState::Passed,
            "{:?}",
            result.attempt_log
        );
        assert_eq!(result.attempts, 2);
    }

    #[tokio::test]
    async fn reviewer_findings_lead_to_needs_review_when_unresolved() {
        let revise = json!({"verdict": "revise", "issues": [{"path": "/benefits/0", "category": "R5", "severity": "major", "explanation": "Overstates the outcome."}]}).to_string();
        let result = run_procedure(
            vec![good_procedure(), good_procedure(), good_procedure()],
            vec![revise.clone(), revise.clone(), revise],
        )
        .await;
        assert_eq!(result.state, SectionState::NeedsReview);
        assert!(result.reasons[0].contains("R5"));
    }

    /// Replies in order; an exhausted script fails as not allowed, which the
    /// transport loop does not retry.
    struct Scripted(Mutex<std::collections::VecDeque<String>>);

    // This crate does not depend on async-trait, so the test models spell out
    // the signature the macro generates.
    type Reply<'a> = futures::future::BoxFuture<'a, dc_ai::Result<dc_ai::TextResponse>>;

    impl TextModel for Scripted {
        fn provider(&self) -> &'static str {
            "scripted"
        }
        fn generate<'life0, 'async_trait>(&'life0 self, request: TextRequest) -> Reply<'async_trait>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            let next = self.0.lock().unwrap().pop_front();
            Box::pin(async move {
                match next {
                    Some(content) => Ok(dc_ai::TextResponse {
                        model: request.model,
                        route: None,
                        content,
                        usage: Usage::default(),
                        latency_ms: 0,
                    }),
                    None => Err(AiError::NotAllowed {
                        provider: "scripted",
                        mode: dc_core::Mode::Clinical,
                    }),
                }
            })
        }
    }

    async fn run_with_failing_repair(reviewer: String) -> SectionResult {
        let kb = kb_with_entry();
        let sheet = FactSheet::build(&daniel(), &kb);
        let m = Models {
            writer: Arc::new(Scripted(Mutex::new([good_procedure()].into()))),
            writer_model: "writer".into(),
            reviewer: Arc::new(FakeTextModel::with_responses([reviewer])),
            reviewer_model: "reviewer".into(),
            reasoning: None,
        };
        section(
            &SectionKind::Procedure("p1".into()),
            &sheet,
            &kb,
            &m,
            ReadingLevel::Standard,
            &|_: Event| {},
        )
        .await
    }

    #[tokio::test]
    async fn writer_error_during_repair_keeps_the_validated_draft() {
        let safety = json!({"verdict": "revise", "issues": [{"path": "/benefits/0", "category": "R5", "severity": "major", "explanation": "Overstates the outcome."}]}).to_string();
        let result = run_with_failing_repair(safety).await;
        assert_eq!(result.state, SectionState::NeedsReview);
        assert!(result.output.is_some());
        assert!(result.reasons[0].contains("R5"), "{:?}", result.reasons);

        let style = json!({"verdict": "revise", "issues": [{"path": "/what_it_is/0", "category": "R6", "severity": "major", "explanation": "Unexplained abbreviation."}]}).to_string();
        let result = run_with_failing_repair(style).await;
        assert_eq!(result.state, SectionState::Passed);
        assert!(result.output.is_some());
        assert!(result.reasons.is_empty());
    }

    /// Fails with a billed error every time and counts the calls.
    struct TimingOut(std::sync::atomic::AtomicU32);

    impl TextModel for TimingOut {
        fn provider(&self) -> &'static str {
            "timing-out"
        }
        fn generate<'life0, 'async_trait>(&'life0 self, _: TextRequest) -> Reply<'async_trait>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Err(AiError::BadResponse("no reply within 120 s".into())) })
        }
    }

    #[tokio::test]
    async fn a_timeout_is_retried_once() {
        let counter = Arc::new(TimingOut(Default::default()));
        let model: Arc<dyn TextModel> = counter.clone();
        let result = call(&model, "m", vec![], json!({}), None).await;
        assert!(matches!(result, Err(AiError::BadResponse(_))));
        assert_eq!(counter.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn procedures_without_knowledge_are_held_without_a_model_call() {
        let kb = kb_with_entry();
        let sheet = FactSheet::build(&daniel(), &kb);
        let m = models(FakeTextModel::default(), FakeTextModel::default());
        let progress = |_: Event| {};
        // p3 is a crown; the test knowledge base only has the root canal entry.
        let result = section(
            &SectionKind::Procedure("p3".into()),
            &sheet,
            &kb,
            &m,
            ReadingLevel::Standard,
            &progress,
        )
        .await;
        assert_eq!(result.state, SectionState::NeedsReview);
        assert_eq!(result.attempts, 0);
    }

    #[test]
    fn stored_output_is_checked_again_with_its_citations() {
        let kb = kb_with_entry();
        let sheet = FactSheet::build(&daniel(), &kb);
        let kind = SectionKind::Procedure("p1".into());
        let good: Value = serde_json::from_str(&good_procedure()).unwrap();
        let issues = check_output(&kind, &good, &sheet, &kb, ReadingLevel::Standard);
        assert!(!validate::has_blocking(&issues), "{issues:?}");

        let bad: Value = serde_json::from_str(&good_procedure().replace(
            "It can let you keep your natural tooth.",
            "Success is guaranteed.",
        ))
        .unwrap();
        let issues = check_output(&kind, &bad, &sheet, &kb, ReadingLevel::Standard);
        assert!(
            issues
                .iter()
                .any(|i| i.code == "V11" && i.path == "/benefits/0")
        );
        let unreadable = check_output(&kind, &json!({}), &sheet, &kb, ReadingLevel::Standard);
        assert!(validate::has_blocking(&unreadable));
    }

    #[test]
    fn knowledge_decides_whether_a_section_can_be_written() {
        let kb = kb_with_entry();
        let sheet = FactSheet::build(&daniel(), &kb);
        assert!(has_knowledge(
            &SectionKind::Procedure("p1".into()),
            &sheet,
            &kb
        ));
        // p3 is a crown; the test knowledge base only has the root canal entry.
        assert!(!has_knowledge(
            &SectionKind::Procedure("p3".into()),
            &sheet,
            &kb
        ));
        assert!(has_knowledge(&SectionKind::Journey, &sheet, &kb));
    }

    #[test]
    fn section_keys_round_trip() {
        for kind in [
            SectionKind::AtAGlance,
            SectionKind::Journey,
            SectionKind::Procedure("p3".into()),
            SectionKind::NextSteps,
        ] {
            assert_eq!(SectionKind::from_key(&kind.key()), Some(kind));
        }
        assert_eq!(SectionKind::from_key("procedure:"), None);
        assert_eq!(SectionKind::from_key("costs"), None);
    }

    #[test]
    fn strips_code_fences() {
        assert_eq!(strip_fences("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_fences(" {\"a\":1} "), "{\"a\":1}");
    }
}
