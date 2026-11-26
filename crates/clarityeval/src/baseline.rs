//! The plain-prompt baseline: one call per presentation that asks the model
//! to explain the plan using only the plan facts. Same `FactSheet` facts and
//! notes as the pipeline, same JSON shape (so the same scorers apply), but no
//! knowledge passages, no validators, no review and no repair. Comparing it
//! with the pipeline shows what grounding, validation and review buy.

use crate::score::{SectionRecord, State};
use dc_core::content::ReadingLevel;
use dc_generation::facts::FactSheet;
use dc_generation::prompt::{self, SectionKind};
use dc_generation::schema;
use serde_json::{Value, json};
use std::fmt::Write as _;

pub const BASELINE_PROMPT: &str = include_str!("../prompts/baseline.presentation.v1.txt");
/// Room for reasoning plus a whole presentation in one reply.
pub const BASELINE_MAX_TOKENS: u32 = 32_000;

pub fn version() -> String {
    prompt::version("baseline.presentation@v1", BASELINE_PROMPT)
}

/// The user message: the same `<facts>` block and notes the pipeline uses,
/// without knowledge passages.
pub fn user_message(sheet: &FactSheet, level: ReadingLevel, requested: &[SectionKind]) -> String {
    let mut user = prompt::facts_block(sheet, level);
    for p in &sheet.procedures {
        if let Some(note) = &p.note {
            let _ = writeln!(
                user,
                "<clinical_note ref=\"{r}\" id=\"note:{r}\">{note}</clinical_note>",
                r = p.reference
            );
        }
    }
    let mut parts = Vec::new();
    if requested.contains(&SectionKind::AtAGlance) {
        parts.push("at_a_glance: a short overview of the whole plan".to_owned());
    }
    if requested.contains(&SectionKind::Journey) {
        parts.push(format!(
            "journey: one entry per phase, in this order: {}",
            sheet
                .phases
                .iter()
                .map(|p| p.reference.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let procs: Vec<&str> = requested
        .iter()
        .filter_map(|k| match k {
            SectionKind::Procedure(r) => Some(r.as_str()),
            _ => None,
        })
        .collect();
    if !procs.is_empty() {
        parts.push(format!(
            "procedures: one explanation per procedure, for {} (procedure_ref is the ref)",
            procs.join(", ")
        ));
    }
    if requested.contains(&SectionKind::NextSteps) {
        parts.push("next_steps: what happens next and questions to ask".to_owned());
    }
    let _ = write!(
        user,
        "<task>Explain this treatment plan to the patient. Include: {}.</task>",
        parts.join("; ")
    );
    user
}

/// One object holding the requested sections, each in the pipeline's schema.
pub fn response_schema(requested: &[SectionKind]) -> Value {
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();
    let mut add = |k: &str, v: Value| {
        props.insert(k.into(), v);
        required.push(k.to_owned());
    };
    if requested.contains(&SectionKind::AtAGlance) {
        add("at_a_glance", schema::at_a_glance_schema());
    }
    if requested.contains(&SectionKind::Journey) {
        add("journey", schema::journey_schema());
    }
    if requested
        .iter()
        .any(|k| matches!(k, SectionKind::Procedure(_)))
    {
        add(
            "procedures",
            json!({"type": "array", "items": schema::procedure_schema()}),
        );
    }
    if requested.contains(&SectionKind::NextSteps) {
        add("next_steps", schema::next_steps_schema());
    }
    json!({"type": "object", "additionalProperties": false, "required": required, "properties": props})
}

fn strip_fences(text: &str) -> &str {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

fn record(key: String, output: Value, model: &str) -> SectionRecord {
    SectionRecord {
        key,
        // No gate: everything the call produced reaches the patient.
        state: State::Passed,
        output: Some(output),
        reasons: vec![],
        needs_note_confirmation: false,
        writer_model: model.into(),
        prompt_version: version(),
        attempts: 1,
        attempt_log: vec![],
        usage: Default::default(),
        elapsed_ms: 0,
    }
}

/// Splits one baseline reply into per-section records. A reply that is not
/// JSON is kept whole as `baseline:unparsed`, so its text is still scored.
pub fn split_reply(raw: &str, model: &str) -> Vec<SectionRecord> {
    let text = strip_fences(raw);
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        return vec![record(
            "baseline:unparsed".into(),
            json!({"text": raw, "cites": []}),
            model,
        )];
    };
    let mut out = Vec::new();
    for (k, v) in map {
        match k.as_str() {
            "at_a_glance" | "journey" | "next_steps" => out.push(record(k, v, model)),
            "procedures" => match v {
                Value::Array(items) => {
                    for (i, item) in items.into_iter().enumerate() {
                        let r = item
                            .get("procedure_ref")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        let key = match r {
                            Some(r)
                                if !out
                                    .iter()
                                    .any(|s: &SectionRecord| s.key == format!("procedure:{r}")) =>
                            {
                                format!("procedure:{r}")
                            }
                            _ => format!("baseline:procedures/{i}"),
                        };
                        out.push(record(key, item, model));
                    }
                }
                other => out.push(record("baseline:procedures".into(), other, model)),
            },
            other => out.push(record(format!("baseline:{other}"), v, model)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_reply_into_sections() {
        let raw = r#"```json
{"at_a_glance": {"summary": [], "objective": []},
 "procedures": [{"procedure_ref": "p1"}, {"procedure_ref": "p1"}, {"x": 1}],
 "extra": "hello"}
```"#;
        let recs = split_reply(raw, "m");
        let keys: Vec<&str> = recs.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "at_a_glance",
                "baseline:extra",
                "procedure:p1",
                "baseline:procedures/1",
                "baseline:procedures/2"
            ]
        );
        assert!(recs.iter().all(|r| r.state == State::Passed));
    }

    #[test]
    fn keeps_unparsed_text() {
        let recs = split_reply("Your crown costs $900.", "m");
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].key, "baseline:unparsed");
        assert_eq!(
            recs[0].output.as_ref().unwrap()["text"],
            "Your crown costs $900."
        );
    }

    #[test]
    fn schema_requires_only_requested_parts() {
        let s = response_schema(&[SectionKind::Procedure("p2".into())]);
        assert_eq!(s["required"], json!(["procedures"]));
        assert!(version().starts_with("baseline.presentation@v1+"));
    }
}
