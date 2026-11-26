//! Optional LLM judge (docs/CLARITYEVAL.md sections 7 and 8).
//!
//! One call per patient-visible section rates faithfulness and balance on
//! the anchored 1 to 5 scales, plus a critical flag. The judge sees the same
//! reference data for every system (plan facts, the knowledge passages and
//! note the pipeline would use for that section) and no model identity.
//! Results are automated judge scores, never clinician review.

use crate::score::SectionRecord;
use crate::score::section_kind;
use crate::score::visible::visible_prose;
use dc_ai::{Message, ReasoningEffort, Role, TextModel, TextRequest};
use dc_core::content::ReadingLevel;
use dc_generation::facts::FactSheet;
use dc_generation::prompt;
use dc_knowledge::KnowledgeBase;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::Arc;

pub const JUDGE_PROMPT: &str = include_str!("../prompts/judge.section.v1.txt");
pub const JUDGE_TEMPERATURE: f32 = 0.0;
pub const JUDGE_MAX_TOKENS: u32 = 8_000;

pub fn version() -> String {
    prompt::version("judge.section@v1", JUDGE_PROMPT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriticalType {
    None,
    InventedFact,
    Diagnosis,
    Medication,
    ContradictsPlan,
    MissedUrgent,
    Guarantee,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rated {
    pub score: u8,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgeOut {
    pub faithfulness: Rated,
    pub balance: Rated,
    pub critical: bool,
    pub critical_type: CriticalType,
}

/// One judgment of one visible section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Judgment {
    pub section: String,
    pub judge_model: String,
    pub model_returned: Option<String>,
    pub provider_route: Option<String>,
    pub prompt_version: String,
    pub result: Option<JudgeOut>,
    pub error: Option<String>,
    pub latency_ms: u64,
    pub cost_usd: Option<f64>,
}

pub fn schema() -> Value {
    let rated = json!({
        "type": "object", "additionalProperties": false, "required": ["score", "rationale"],
        "properties": {
            "score": {"type": "integer", "enum": [1, 2, 3, 4, 5]},
            "rationale": {"type": "string"}
        }
    });
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["faithfulness", "balance", "critical", "critical_type"],
        "properties": {
            "faithfulness": rated.clone(),
            "balance": rated,
            "critical": {"type": "boolean"},
            "critical_type": {"type": "string", "enum": [
                "none", "invented_fact", "diagnosis", "medication",
                "contradicts_plan", "missed_urgent", "guarantee", "other"]}
        }
    })
}

/// Reference data plus the visible sentences, one per line.
pub fn user_message(
    section: &SectionRecord,
    sheet: &FactSheet,
    kb: &KnowledgeBase,
    level: ReadingLevel,
) -> String {
    let reference = match section_kind(&section.key) {
        Some(kind) => {
            let built = prompt::build(&kind, sheet, kb, level);
            built
                .user
                .rsplit_once("<task>")
                .map_or(built.user.clone(), |(d, _)| d.to_owned())
        }
        None => prompt::facts_block(sheet, level),
    };
    let mut user = reference;
    user.push_str("<patient_text>\n");
    if let Some(out) = &section.output {
        for p in visible_prose(&section.key, out) {
            let _ = writeln!(user, "{}", p.text.replace('\n', " "));
        }
    }
    user.push_str("</patient_text>");
    user
}

pub fn parse(raw: &str) -> Result<JudgeOut, String> {
    let t = raw.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    let out: JudgeOut = serde_json::from_str(t).map_err(|e| format!("unreadable judgment: {e}"))?;
    for s in [out.faithfulness.score, out.balance.score] {
        if !(1..=5).contains(&s) {
            return Err(format!("score {s} is outside 1 to 5"));
        }
    }
    Ok(out)
}

/// Judges one section; an unreadable judgment is retried once, then recorded as missing.
pub async fn judge_section(
    model: &Arc<dyn TextModel>,
    judge_model: &str,
    reasoning: Option<ReasoningEffort>,
    section: &SectionRecord,
    sheet: &FactSheet,
    kb: &KnowledgeBase,
    level: ReadingLevel,
) -> Judgment {
    let request = TextRequest {
        model: judge_model.into(),
        messages: vec![
            Message {
                role: Role::System,
                content: JUDGE_PROMPT.into(),
            },
            Message {
                role: Role::User,
                content: user_message(section, sheet, kb, level),
            },
        ],
        response_schema: Some(schema()),
        max_tokens: JUDGE_MAX_TOKENS,
        temperature: JUDGE_TEMPERATURE,
        reasoning,
    };
    let mut j = Judgment {
        section: section.key.clone(),
        judge_model: judge_model.into(),
        model_returned: None,
        provider_route: None,
        prompt_version: version(),
        result: None,
        error: None,
        latency_ms: 0,
        cost_usd: None,
    };
    for _ in 0..2 {
        let started = std::time::Instant::now();
        match model.generate(request.clone()).await {
            Err(e) => j.error = Some(e.to_string()),
            Ok(r) => {
                j.model_returned = Some(r.model.clone());
                j.provider_route = r.route.clone();
                j.cost_usd = match (j.cost_usd, r.usage.cost_usd) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
                match parse(&r.content) {
                    Ok(out) => {
                        j.result = Some(out);
                        j.error = None;
                        j.latency_ms += started.elapsed().as_millis() as u64;
                        break;
                    }
                    Err(e) => j.error = Some(e),
                }
            }
        }
        j.latency_ms += started.elapsed().as_millis() as u64;
    }
    j
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_bounds_judgments() {
        let ok = r#"{"faithfulness":{"score":4,"rationale":"r"},"balance":{"score":5,"rationale":"r"},"critical":false,"critical_type":"none"}"#;
        assert_eq!(parse(ok).unwrap().faithfulness.score, 4);
        assert!(parse(&ok.replace("\"score\":4", "\"score\":6")).is_err());
        assert!(parse("not json").is_err());
        assert!(version().starts_with("judge.section@v1+"));
    }
}
