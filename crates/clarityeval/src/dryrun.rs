//! `run --dry-run`: a scripted text model that answers every request
//! offline, so the whole flow (pipeline, baseline, judge, scoring, report)
//! can be exercised in tests and before spending money.
//!
//! `dc_ai::fake::FakeTextModel` replays a fixed queue, which does not fit a
//! pipeline that writes sections in parallel. This model instead reads each
//! request and returns a fitting reply:
//! - writer: a valid section; procedure sections first get a reply with a
//!   planted cost, so the repair loop runs;
//! - reviewer: `pass`;
//! - baseline: the requested parts, with a planted cost and timing sentence
//!   and a note-based reason, which the patient-visible scorer must catch;
//! - judge: 5/5, or 2 with a critical flag when the text shows a cost.

use crate::baseline::BASELINE_PROMPT;
use crate::judge::JUDGE_PROMPT;
use async_trait::async_trait;
use dc_ai::{Role, TextModel, TextRequest, TextResponse, Usage};
use dc_generation::prompt::{REVIEWER_PROMPT, WRITER_PROMPT};
use serde_json::{Value, json};

pub const DRY_RUN_ROUTE: &str = "dry-run";
/// Fake per-call cost, so cost aggregation is exercised.
pub const DRY_RUN_COST: f64 = 0.0001;

#[derive(Debug, Default)]
pub struct ScriptedModel;

fn ids(user: &str, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in user.split('[').skip(1) {
        if let Some((id, _)) = part.split_once(']')
            && id.starts_with(prefix)
            && !out.contains(&id.to_owned())
        {
            out.push(id.to_owned());
        }
    }
    out
}

fn s(text: &str, cites: &[&str]) -> Value {
    json!({"text": text, "cites": cites})
}

fn q(text: &str) -> Value {
    json!({"text": text, "cites": []})
}

fn procedure(reference: &str, kb: &[String], note: bool, baseline: bool) -> Value {
    let fact = format!("fact:{reference}");
    let k = kb.first().cloned().unwrap_or_else(|| fact.clone());
    let why = if baseline && note {
        json!({"kind": "documented", "sentences": [s("Your dentist found a problem with this tooth.", &[&fact, &format!("note:{reference}")])]})
    } else {
        json!({"kind": "common_reasons", "sentences": [s("Common reasons for this procedure include protecting a tooth.", &[&fact, &k])]})
    };
    json!({
        "procedure_ref": reference,
        "what_it_is": [s("This procedure is part of the plan your dental team proposed.", &[&fact, &k])],
        "why": why,
        "what_happens": [s("Your dental team explains each step before it starts.", &[&fact, &k])],
        "recovery": [if baseline {
            s("Recovery takes two days for most people.", &[&fact])
        } else {
            s("Many people feel back to normal soon after.", &[&fact, &k])
        }],
        "benefits": [s("It can help keep the mouth healthy.", &[&fact, &k])],
        "if_delayed": [s("Problems can sometimes grow over time if left alone.", &[&fact, &k])],
        "questions": [q("What should I expect afterwards?"), q("How do I care for the area?")]
    })
}

fn phases(user: &str) -> Vec<String> {
    ids(user, "fact:ph")
        .into_iter()
        .map(|i| i.trim_start_matches("fact:").to_owned())
        .collect()
}

fn procedures(user: &str) -> Vec<String> {
    ids(user, "fact:p")
        .into_iter()
        .filter(|i| !i.starts_with("fact:ph"))
        .map(|i| i.trim_start_matches("fact:").to_owned())
        .collect()
}

fn at_a_glance(user: &str, baseline: bool) -> Value {
    let ph = phases(user);
    let first = format!("fact:{}", ph.first().map_or("ph1", String::as_str));
    let mut summary = vec![
        s(
            "Your plan has a few steps to care for your teeth.",
            &[&first],
        ),
        s("Each step is explained below in plain words.", &[&first]),
    ];
    if baseline {
        summary.push(s("The whole plan costs about $300.", &[&first]));
    }
    json!({"summary": summary, "objective": []})
}

fn journey(user: &str) -> Value {
    let phases: Vec<Value> = phases(user)
        .iter()
        .map(|p| json!({"phase_ref": p, "summary": [s("This phase covers the steps listed for it.", &[&format!("fact:{p}")])]}))
        .collect();
    json!({"phases": phases, "dependencies": []})
}

fn next_steps(user: &str) -> Value {
    let ph = phases(user);
    let first = format!("fact:{}", ph.first().map_or("ph1", String::as_str));
    json!({
        "outlook": [s("The plan is meant to help keep your mouth healthy.", &[&first])],
        "next_steps": [s("You can talk with your dental team and ask any questions.", &[&first])],
        "questions": [q("Which step comes first?"), q("What can I do at home?")]
    })
}

fn writer(user: &str, first_attempt: bool) -> Value {
    let task = user.rsplit_once("<task>").map_or("", |(_, t)| t);
    if let Some(rest) = task.strip_prefix("Write the explanation for procedure ") {
        let reference = rest
            .split_whitespace()
            .next()
            .unwrap_or("p1")
            .trim_end_matches('.');
        let mut v = procedure(reference, &ids(user, "kb:"), false, false);
        if first_attempt {
            v["recovery"] = json!([s("It costs $120.", &[&format!("fact:{reference}")])]);
        }
        v
    } else if task.contains("\"at a glance\"") {
        at_a_glance(user, false)
    } else if task.contains("treatment journey") {
        journey(user)
    } else {
        next_steps(user)
    }
}

fn baseline(user: &str) -> Value {
    let task = user.rsplit_once("<task>").map_or("", |(_, t)| t);
    let mut out = serde_json::Map::new();
    if task.contains("at_a_glance:") {
        out.insert("at_a_glance".into(), at_a_glance(user, true));
    }
    if task.contains("journey:") {
        out.insert("journey".into(), journey(user));
    }
    if let Some(rest) = task
        .split("procedures: one explanation per procedure, for ")
        .nth(1)
    {
        let list = rest.split(" (").next().unwrap_or("");
        let refs: Vec<String> = list.split(", ").map(str::to_owned).collect();
        let all = procedures(user);
        let items: Vec<Value> = refs
            .iter()
            .filter(|r| all.contains(r))
            .map(|r| {
                let note = user.contains(&format!("id=\"note:{r}\""));
                procedure(r, &[], note, true)
            })
            .collect();
        out.insert("procedures".into(), Value::Array(items));
    }
    if task.contains("next_steps:") {
        out.insert("next_steps".into(), next_steps(user));
    }
    Value::Object(out)
}

fn judge(user: &str) -> Value {
    let text = user.rsplit_once("<patient_text>").map_or("", |(_, t)| t);
    let cost = text.contains('$');
    json!({
        "faithfulness": {"score": if cost { 2 } else { 5 }, "rationale": "Scripted dry-run rating."},
        "balance": {"score": 5, "rationale": "Scripted dry-run rating."},
        "critical": cost,
        "critical_type": if cost { "invented_fact" } else { "none" }
    })
}

#[async_trait]
impl TextModel for ScriptedModel {
    fn provider(&self) -> &'static str {
        "dry-run"
    }

    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        let system = request
            .messages
            .first()
            .filter(|m| m.role == Role::System)
            .map_or("", |m| m.content.as_str());
        let user = request
            .messages
            .iter()
            .find(|m| m.role == Role::User)
            .map_or("", |m| m.content.as_str());
        let content = if system == REVIEWER_PROMPT {
            json!({"verdict": "pass", "issues": []})
        } else if system == BASELINE_PROMPT {
            baseline(user)
        } else if system == JUDGE_PROMPT {
            judge(user)
        } else if system == WRITER_PROMPT {
            writer(user, request.messages.len() <= 2)
        } else {
            return Err(dc_ai::AiError::BadResponse(
                "dry-run model got an unknown prompt".into(),
            ));
        };
        Ok(TextResponse {
            model: format!("{}-dryrun", request.model),
            route: Some(DRY_RUN_ROUTE.into()),
            content: content.to_string(),
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: Some(DRY_RUN_COST),
            },
            latency_ms: 0,
        })
    }
}
