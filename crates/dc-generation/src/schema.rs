//! Model output types and the JSON Schemas sent for structured output.
//!
//! Parsing is strict (`deny_unknown_fields`); bounds that JSON Schema cannot
//! express well are checked by the validators.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Exactly one sentence with the ids of what supports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sentence {
    pub text: String,
    pub cites: Vec<String>,
}

pub type Para = Vec<Sentence>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub text: String,
    pub cites: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhyKind {
    /// The linked clinical note states the reason; cites `note:<ref>`.
    Documented,
    /// General reasons from the knowledge base; never framed as this patient's.
    CommonReasons,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Why {
    pub kind: WhyKind,
    pub sentences: Para,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtAGlanceOut {
    pub summary: Para,
    /// Empty unless a linked note documents the treatment objective.
    pub objective: Para,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseOut {
    pub phase_ref: String,
    pub summary: Para,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JourneyOut {
    pub phases: Vec<PhaseOut>,
    /// Empty unless the facts or knowledge base state a dependency.
    pub dependencies: Para,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcedureOut {
    pub procedure_ref: String,
    pub what_it_is: Para,
    pub why: Why,
    pub what_happens: Para,
    pub recovery: Para,
    pub benefits: Para,
    pub if_delayed: Para,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextStepsOut {
    pub outlook: Para,
    pub next_steps: Para,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Revise,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Minor,
    Major,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewIssue {
    pub path: String,
    pub category: String,
    pub severity: Severity,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewVerdict {
    pub verdict: Verdict,
    pub issues: Vec<ReviewIssue>,
}

/// Pattern every citation id must match.
pub const CITE_PATTERN: &str = r"^(fact:(p|ph)[0-9]+|note:p[0-9]+|kb:[a-z0-9_]+#[a-z0-9_]+)$";

fn sentence() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["text", "cites"],
        "properties": {
            "text": {"type": "string"},
            "cites": {"type": "array", "items": {"type": "string", "pattern": CITE_PATTERN}}
        }
    })
}

/// A paragraph of `min..=max` sentences. Bounds are also enforced by the
/// validators, since not every provider applies them during generation.
fn para_n(min: u32, max: u32) -> Value {
    json!({"type": "array", "minItems": min, "maxItems": max, "items": sentence()})
}

fn para() -> Value {
    para_n(1, 6)
}

fn questions() -> Value {
    json!({"type": "array", "minItems": 2, "maxItems": 4, "items": question()})
}

fn question() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["text", "cites"],
        "properties": {
            "text": {"type": "string"},
            "cites": {"type": "array", "items": {"type": "string", "pattern": CITE_PATTERN}}
        }
    })
}

fn object(required: &[&str], properties: Value) -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "required": required, "properties": properties
    })
}

pub fn at_a_glance_schema() -> Value {
    object(
        &["summary", "objective"],
        json!({"summary": para_n(2, 4), "objective": para_n(0, 3)}),
    )
}

pub fn journey_schema() -> Value {
    object(
        &["phases", "dependencies"],
        json!({
            "phases": {"type": "array", "items": object(
                &["phase_ref", "summary"],
                json!({"phase_ref": {"type": "string"}, "summary": para_n(1, 3)}),
            )},
            "dependencies": para_n(0, 3),
        }),
    )
}

pub fn procedure_schema() -> Value {
    object(
        &[
            "procedure_ref",
            "what_it_is",
            "why",
            "what_happens",
            "recovery",
            "benefits",
            "if_delayed",
            "questions",
        ],
        json!({
            "procedure_ref": {"type": "string"},
            "what_it_is": para(),
            "why": object(&["kind", "sentences"], json!({
                "kind": {"type": "string", "enum": ["documented", "common_reasons"]},
                "sentences": para_n(1, 4),
            })),
            "what_happens": para(),
            "recovery": para(),
            "benefits": para(),
            "if_delayed": para(),
            "questions": questions(),
        }),
    )
}

pub fn next_steps_schema() -> Value {
    object(
        &["outlook", "next_steps", "questions"],
        json!({
            "outlook": para_n(1, 4),
            "next_steps": para_n(1, 5),
            "questions": questions(),
        }),
    )
}

pub fn review_schema() -> Value {
    object(
        &["verdict", "issues"],
        json!({
            "verdict": {"type": "string", "enum": ["pass", "revise", "block"]},
            "issues": {"type": "array", "items": object(
                &["path", "category", "severity", "explanation"],
                json!({
                    "path": {"type": "string"},
                    "category": {"type": "string"},
                    "severity": {"type": "string", "enum": ["minor", "major", "critical"]},
                    "explanation": {"type": "string"},
                }),
            )},
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procedure_output_round_trips_and_rejects_extra_fields() {
        let s = |t: &str| Sentence {
            text: t.into(),
            cites: vec!["fact:p1".into()],
        };
        let out = ProcedureOut {
            procedure_ref: "p1".into(),
            what_it_is: vec![s("A crown is a cap.")],
            why: Why {
                kind: WhyKind::CommonReasons,
                sentences: vec![s("Common reasons include a cracked tooth.")],
            },
            what_happens: vec![s("The tooth is shaped.")],
            recovery: vec![s("Many people feel some sensitivity.")],
            benefits: vec![s("A crown protects the tooth.")],
            if_delayed: vec![s("A weak tooth can crack over time.")],
            questions: vec![Question {
                text: "What is it made of?".into(),
                cites: vec![],
            }],
        };
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(
            serde_json::from_value::<ProcedureOut>(json.clone()).unwrap(),
            out
        );
        let mut extra = json;
        extra["note_to_patient"] = "hi".into();
        assert!(serde_json::from_value::<ProcedureOut>(extra).is_err());
    }

    #[test]
    fn schemas_close_every_object() {
        fn check(v: &Value) {
            if v.get("type") == Some(&json!("object")) {
                assert_eq!(v["additionalProperties"], json!(false));
                let props = v["properties"].as_object().unwrap();
                let required: Vec<_> = v["required"].as_array().unwrap().iter().collect();
                assert_eq!(
                    props.len(),
                    required.len(),
                    "every property must be required"
                );
            }
            match v {
                Value::Object(m) => m.values().for_each(check),
                Value::Array(a) => a.iter().for_each(check),
                _ => {}
            }
        }
        for schema in [
            at_a_glance_schema(),
            journey_schema(),
            procedure_schema(),
            next_steps_schema(),
            review_schema(),
        ] {
            check(&schema);
        }
    }
}
