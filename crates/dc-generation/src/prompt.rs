//! Prompt construction. Plan data enters prompts only through the
//! [`FactSheet`], which carries no names, keys, dates or money.

use crate::facts::{FactSheet, category_label, tooth_label};
use crate::schema;
use dc_core::content::ReadingLevel;
use dc_knowledge::{Entry, KnowledgeBase, Topic};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt::Write;

pub const WRITER_PROMPT: &str = include_str!("../prompts/writer.section.v2.txt");
pub const REVIEWER_PROMPT: &str = include_str!("../prompts/reviewer.section.v1.txt");

/// `writer.section@v1+<first 12 hex of sha256>`, recorded with every output.
pub fn version(name: &str, text: &str) -> String {
    let hash = hex::encode(Sha256::digest(text.as_bytes()));
    format!("{name}+{}", &hash[..12])
}

pub fn writer_version() -> String {
    version("writer.section@v2", WRITER_PROMPT)
}

pub fn reviewer_version() -> String {
    version("reviewer.section@v1", REVIEWER_PROMPT)
}

/// The sections of a presentation that a model writes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SectionKind {
    AtAGlance,
    Journey,
    Procedure(String),
    NextSteps,
}

impl SectionKind {
    /// Stable key, e.g. `procedure:p3`.
    pub fn key(&self) -> String {
        match self {
            SectionKind::AtAGlance => "at_a_glance".into(),
            SectionKind::Journey => "journey".into(),
            SectionKind::Procedure(r) => format!("procedure:{r}"),
            SectionKind::NextSteps => "next_steps".into(),
        }
    }

    pub fn schema(&self) -> Value {
        match self {
            SectionKind::AtAGlance => schema::at_a_glance_schema(),
            SectionKind::Journey => schema::journey_schema(),
            SectionKind::Procedure(_) => schema::procedure_schema(),
            SectionKind::NextSteps => schema::next_steps_schema(),
        }
    }
}

/// Everything a writer call needs, plus the ids its output may cite.
#[derive(Debug, Clone)]
pub struct SectionPrompt {
    pub kind: SectionKind,
    /// The user message: tagged data blocks and the task.
    pub user: String,
    pub kb_ids: HashSet<String>,
    pub note_ids: HashSet<String>,
}

pub fn build(
    kind: &SectionKind,
    sheet: &FactSheet,
    kb: &KnowledgeBase,
    level: ReadingLevel,
) -> SectionPrompt {
    let mut user = facts_block(sheet, level);
    let mut kb_ids = HashSet::new();
    let mut note_ids = HashSet::new();

    let (topics, task): (&[Topic], String) = match kind {
        SectionKind::AtAGlance => (
            &[Topic::WhatItIs, Topic::Benefits],
            "Write the \"at a glance\" overview of the whole plan. summary: 2 to 4 sentences on \
             what is planned and what it is meant to achieve, citing the procedures or phases. \
             objective: 0 to 2 sentences, only if a clinical note states the overall goal; \
             each must cite that note. Otherwise return an empty array."
                .into(),
        ),
        SectionKind::Journey => (
            &[Topic::WhatItIs, Topic::WhatHappens],
            format!(
                "Write the treatment journey. phases: exactly one entry per phase, in this order: {}. \
                 Each summary is 1 to 3 sentences on what that phase includes, citing the phase \
                 and its procedures. dependencies: 0 to 2 sentences, only for an order of steps \
                 that the facts or kb passages state (for example a build-up before a crown); \
                 otherwise an empty array. Never mention visit counts, dates or durations.",
                sheet
                    .phases
                    .iter()
                    .map(|p| p.reference.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        SectionKind::Procedure(reference) => (
            &Topic::ALL,
            format!(
                "Write the explanation for procedure {reference}. procedure_ref must be \
                 \"{reference}\". what_it_is, what_happens, recovery, benefits and if_delayed: \
                 1 to 4 sentences each. why: 1 to 3 sentences (see the rules for documented \
                 versus common_reasons). questions: 2 to 4 questions the patient could ask \
                 their dental team."
            ),
        ),
        SectionKind::NextSteps => (
            &[Topic::Benefits, Topic::IfDelayed, Topic::Questions],
            "Write \"what happens next\". outlook: 1 to 3 balanced sentences on what the plan \
             as a whole is meant to achieve, without promises. next_steps: 1 to 4 sentences on \
             practical next steps, such as talking with the dental team, asking questions and \
             deciding together; never pressure. questions: 2 to 4 questions about the plan."
                .into(),
        ),
    };

    // Knowledge passages for the procedures this section covers.
    let entries: Vec<&Entry> = match kind {
        SectionKind::Procedure(reference) => sheet
            .procedure(reference)
            .and_then(|p| kb.entry(p.category))
            .into_iter()
            .collect(),
        _ => sheet
            .categories()
            .into_iter()
            .filter_map(|c| kb.entry(c))
            .collect(),
    };
    for entry in entries {
        let _ = writeln!(user, "<kb entry=\"{}\">", entry.id);
        for topic in topics {
            if let Some(passage) = entry.passage(*topic) {
                let id = entry.source_id(passage);
                let _ = writeln!(user, "[{id}] {}", passage.text.trim());
                kb_ids.insert(id);
            }
        }
        user.push_str("</kb>\n");
    }

    // Linked clinical notes: the procedure's own note, or all notes for the overview.
    let note_refs: Vec<&str> = match kind {
        SectionKind::Procedure(reference) => vec![reference.as_str()],
        SectionKind::AtAGlance => sheet
            .procedures
            .iter()
            .map(|p| p.reference.as_str())
            .collect(),
        _ => vec![],
    };
    for reference in note_refs {
        if let Some(note) = sheet.procedure(reference).and_then(|p| p.note.as_deref()) {
            let _ = writeln!(
                user,
                "<clinical_note ref=\"{reference}\" id=\"note:{reference}\">{note}</clinical_note>"
            );
            note_ids.insert(format!("note:{reference}"));
        }
    }

    let _ = write!(user, "<task>{task}</task>");
    SectionPrompt {
        kind: kind.clone(),
        user,
        kb_ids,
        note_ids,
    }
}

fn facts_block(sheet: &FactSheet, level: ReadingLevel) -> String {
    let mut out = String::from("<facts>\n");
    let audience = serde_json::to_value(sheet.age_band).ok();
    let _ = writeln!(
        out,
        "audience: {} (guardian_audience: {})",
        audience.as_ref().and_then(Value::as_str).unwrap_or("adult"),
        sheet.guardian_audience
    );
    let _ = writeln!(out, "reading_level: {level}");
    out.push_str("procedures:\n");
    for p in &sheet.procedures {
        let _ = writeln!(
            out,
            "- [fact:{r}] {name} (category {cat}, code {code}), {tooth}, phase {phase}",
            r = p.reference,
            name = p.name,
            cat = category_label(p.category),
            code = p.code,
            tooth = tooth_label(p.tooth, &p.surfaces),
            phase = p.phase_ref,
        );
    }
    out.push_str("phases, in treatment order:\n");
    for ph in &sheet.phases {
        let _ = writeln!(
            out,
            "- [fact:{}] includes {}",
            ph.reference,
            ph.procedure_refs.join(", ")
        );
    }
    out.push_str("</facts>\n");
    out
}

/// The reviewer's user message: the same data the writer saw, plus the draft.
pub fn review_user(section: &SectionPrompt, draft: &str) -> String {
    let data = section
        .user
        .rsplit_once("<task>")
        .map_or(section.user.as_str(), |(data, _)| data);
    format!("{data}<draft>{draft}</draft>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{daniel, kb_with_entry};

    #[test]
    fn procedure_prompt_has_kb_and_its_own_note_only() {
        let kb = kb_with_entry();
        let sheet = FactSheet::build(&daniel(), &kb);
        let prompt = build(
            &SectionKind::Procedure("p1".into()),
            &sheet,
            &kb,
            ReadingLevel::Standard,
        );
        assert!(prompt.user.contains("[kb:root_canal#what_it_is]"));
        assert!(prompt.user.contains("<clinical_note ref=\"p1\""));
        assert!(prompt.note_ids.contains("note:p1"));
        assert_eq!(prompt.kb_ids.len(), 7);
    }

    #[test]
    fn prompts_never_carry_names_keys_or_money() {
        let kb = kb_with_entry();
        let snapshot = daniel();
        let sheet = FactSheet::build(&snapshot, &kb);
        for kind in [
            SectionKind::AtAGlance,
            SectionKind::Journey,
            SectionKind::Procedure("p1".into()),
            SectionKind::NextSteps,
        ] {
            let user = build(&kind, &sheet, &kb, ReadingLevel::Simple).user;
            for forbidden in [
                "Daniel", "Okafor", "Patel", "10482", "tp-10482", "pl-10482", "$", "1150", "460",
            ] {
                assert!(
                    !user.contains(forbidden),
                    "{forbidden} leaked into {kind:?}: {user}"
                );
            }
        }
    }

    #[test]
    fn prompt_versions_are_stable() {
        assert!(writer_version().starts_with("writer.section@v2+"));
        assert_eq!(writer_version(), writer_version());
    }
}
