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
    if let SectionKind::Procedure(reference) = kind {
        let category = sheet.procedure(reference).map(|p| p.category);
        if category == Some(ProcedureCategory::Unsupported)
            || category.and_then(|c| kb.entry(c)).is_none()
        {
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
                result.reasons = vec![format!("The writing model could not be reached ({err}).")];
                result.state = SectionState::Failed;
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

fn parse_and_validate(
    kind: &SectionKind,
    raw: &str,
    cx: &Context,
) -> Result<(Value, Vec<Issue>), Issue> {
    let bad = |e: serde_json::Error| Issue {
        code: "V1",
        path: "/".into(),
        severity: IssueSeverity::Block,
        detail: format!("the reply is not valid JSON for this schema: {e}"),
    };
    let value: Value = serde_json::from_str(raw).map_err(bad)?;
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

/// One model call with up to three transport retries and jittered backoff.
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
    for attempt in 0..3u32 {
        match model.generate(request.clone()).await {
            Ok(reply) => return Ok(reply),
            Err(err @ AiError::NotAllowed { .. }) => return Err(err),
            Err(err) => {
                tracing::warn!(%err, attempt, "model call failed; retrying");
                last = Some(err);
                let jitter = u64::from(std::process::id() % 250);
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt) + jitter)).await;
            }
        }
    }
    Err(last.expect("at least one attempt"))
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
    fn strips_code_fences() {
        assert_eq!(strip_fences("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_fences(" {\"a\":1} "), "{\"a\":1}");
    }
}
