//! `clarityeval validate`: schema and lint checks (docs/CLARITYEVAL.md section 4, step 5).

use crate::case::{Case, LoadedCase, Split, TaskKind, parse_tooth};
use dc_core::ProcedureCode;
use dc_knowledge::KnowledgeBase;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub file: String,
    pub message: String,
}

/// Fragments typical of ADA descriptor wording. Our names are short and plain;
/// any of these in a case means descriptor text may have been pasted in (C2).
const DESCRIPTOR_STYLE: &[&str] = &[
    "porcelain fused to",
    "predominantly base metal",
    "resin-based composite",
    "four or more teeth",
    "one to three teeth",
    "per quadrant",
    "per arch",
];

/// Checks one case on its own. `canary` is the repo-wide string from `eval/canary.txt`.
pub fn check_case(loaded: &LoadedCase, kb: &KnowledgeBase, canary: &str) -> Vec<String> {
    let case = &loaded.case;
    let mut errs = Vec::new();
    let mut err = |m: String| errs.push(m);

    if case.schema_version != crate::case::SCHEMA_VERSION {
        err(format!(
            "schema_version must be \"{}\"",
            crate::case::SCHEMA_VERSION
        ));
    }
    let prefix = case.category.prefix();
    let id_ok = case.case_id.len() == 5
        && case.case_id.starts_with(prefix)
        && case.case_id[1..2] == *"-"
        && case.case_id[2..].chars().all(|c| c.is_ascii_digit());
    if !id_ok {
        err(format!(
            "case_id {:?} must look like {prefix}-001 for category {}",
            case.case_id,
            case.category.as_str()
        ));
    }
    if loaded.path.file_stem().and_then(|s| s.to_str()) != Some(case.case_id.as_str()) {
        err(format!("file name must be {}.toml", case.case_id));
    }
    let parent = loaded
        .path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str());
    if parent != Some(case.category.as_str()) {
        err(format!(
            "file must live in a {}/ folder",
            case.category.as_str()
        ));
    }
    if case.canary != canary {
        err("canary does not match eval/canary.txt".into());
    }
    if !is_date(&case.created) {
        err(format!("created {:?} must be YYYY-MM-DD", case.created));
    }
    let author_ok = case
        .author
        .strip_prefix("author-")
        .is_some_and(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_uppercase()));
    if !author_ok {
        err(format!(
            "author {:?} must be a pseudonymous code such as author-A",
            case.author
        ));
    }
    let plan_prefix = format!("plan-{}-", case.split.as_str());
    if !case.plan_ref.starts_with(&plan_prefix) {
        err(format!(
            "plan_ref {:?} must start with {plan_prefix}",
            case.plan_ref
        ));
    }

    let facts = &case.source_facts;
    if case.task.kind.runnable() && facts.language != "en" {
        err("the pipeline writes English only; language must be \"en\"".into());
    }
    if facts.procedures.is_empty() {
        err("source_facts needs at least one procedure".into());
    }
    for (i, p) in facts.procedures.iter().enumerate() {
        let want = format!("p{}", i + 1);
        if p.id != want {
            err(format!(
                "procedure {} must have id {want} (plan order)",
                p.id
            ));
        }
        let code = ProcedureCode::new(&p.code);
        let custom_ok = !p.code.is_empty()
            && p.code.len() <= 10
            && p.code
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        if code.as_str() != p.code || !(code.is_cdt_shaped() || custom_ok) {
            err(format!(
                "{}: code {:?} must be a bare uppercase code",
                p.id, p.code
            ));
        }
        match kb.codes.get(&code) {
            Some(info) if info.name != p.name => err(format!(
                "{}: name must be our own name {:?} from knowledge/codes.toml",
                p.id, info.name
            )),
            None if p.name.chars().count() > 40 => {
                err(format!("{}: name is too long for one of our names", p.id))
            }
            _ => {}
        }
        match parse_tooth(&p.tooth) {
            Ok(t) if !p.surfaces.is_empty() && !t.is_single_tooth() => {
                err(format!("{}: surfaces need a single tooth", p.id))
            }
            Ok(_) => {}
            Err(e) => err(format!("{}: {e}", p.id)),
        }
        if !p.surfaces.chars().all(|c| "MODBLFI".contains(c)) {
            err(format!("{}: surfaces use only M, O, D, B, L, F, I", p.id));
        }
        if p.note.as_deref().is_some_and(|n| n.trim().is_empty()) {
            err(format!("{}: note is empty; leave it out instead", p.id));
        }
    }
    if let Err(e) = case.to_snapshot() {
        err(format!(
            "source facts do not convert to a PlanSnapshot: {e}"
        ));
    }

    // Descriptor-text and PHI lints over every free-text field.
    let planted = case.tags.iter().any(|t| t == "planted_identifiers");
    let mut texts: Vec<(&str, &str)> = facts
        .procedures
        .iter()
        .map(|p| ("name", p.name.as_str()))
        .collect();
    texts.extend(
        facts
            .procedures
            .iter()
            .filter_map(|p| p.note.as_deref())
            .map(|n| ("note", n)),
    );
    texts.push(("notes_for_judge", case.rubric.notes_for_judge.as_str()));
    for (field, text) in &texts {
        let lower = text.to_lowercase();
        if let Some(d) = DESCRIPTOR_STYLE.iter().find(|d| lower.contains(*d)) {
            err(format!("{field} contains descriptor-style wording {d:?}"));
        }
        if !(planted && *field == "note")
            && let Some(hit) = phi_hit(text)
        {
            err(format!(
                "{field} looks like it holds an identifier ({hit}); tag planted_identifiers if intended"
            ));
        }
    }
    if planted && case.expected.forbidden_strings.is_empty() {
        err("planted_identifiers needs expected.forbidden_strings".into());
    }
    let notes: String = facts
        .procedures
        .iter()
        .filter_map(|p| p.note.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    for s in &case.expected.forbidden_strings {
        if !notes.contains(s.as_str()) {
            err(format!("forbidden string {s:?} is not planted in any note"));
        }
    }

    for r in &facts.references {
        match r.id.strip_prefix("kb:") {
            Some(entry) if kb.entries().any(|e| e.id == entry) => {}
            _ => err(format!("reference {:?} is not a knowledge entry", r.id)),
        }
        if r.title.trim().is_empty() {
            err(format!("reference {} needs a title", r.id));
        }
    }

    check_task(case, kb, &mut errs);
    errs
}

fn check_task(case: &Case, kb: &KnowledgeBase, errs: &mut Vec<String>) {
    let facts = &case.source_facts;
    let task = &case.task;
    let target = case.target();
    if task.target.is_some() && target.is_none() {
        errs.push(format!("task.target {:?} is not a procedure", task.target));
    }
    if task.kind == TaskKind::ProcedureExplanation && task.target.is_none() {
        errs.push("procedure_explanation needs task.target".into());
    }
    if task.kind == TaskKind::QaAnswer && task.question.is_none() {
        errs.push("qa_answer needs task.question".into());
    }

    let phases: BTreeSet<Option<u8>> = facts.procedures.iter().map(|p| p.phase).collect();
    let ex = &case.expected;
    for f in &ex.must_reference_facts {
        let ok = if let Some(code) = f.strip_prefix("code:") {
            facts.procedures.iter().any(|p| p.code == code)
        } else if let Some(r) = f.strip_prefix("fact:ph") {
            r.parse::<usize>()
                .is_ok_and(|n| n >= 1 && n <= phases.len())
        } else if let Some(r) = f.strip_prefix("fact:") {
            facts.procedures.iter().any(|p| p.id == r)
        } else {
            false
        };
        if !ok {
            errs.push(format!("must_reference_facts entry {f:?} does not resolve"));
        }
    }
    for c in &ex.must_cite_any_of {
        let entry = c
            .strip_prefix("kb:")
            .map(|r| r.split('#').next().unwrap_or(r));
        if !entry.is_some_and(|e| kb.entries().any(|k| k.id == e)) {
            errs.push(format!(
                "must_cite_any_of entry {c:?} is not a knowledge entry"
            ));
        }
    }
    if let Some(v) = &ex.forbidden_citations_outside
        && v != "source_facts.references"
    {
        errs.push("forbidden_citations_outside must be \"source_facts.references\"".into());
    }
    if ex.must_not_state_tooth {
        match target.map(|t| parse_tooth(&t.tooth)) {
            Some(Ok(t)) if !t.is_single_tooth() => {}
            _ => errs.push("must_not_state_tooth needs a target with no single tooth".into()),
        }
    }
    for h in &ex.held_sections {
        let ok = match h.key.as_str() {
            "at_a_glance" | "journey" | "next_steps" => true,
            k => k
                .strip_prefix("procedure:")
                .is_some_and(|r| facts.procedures.iter().any(|p| p.id == r)),
        };
        if !ok {
            errs.push(format!("held_sections key {:?} is not a section", h.key));
        }
    }
}

/// Validates a set of cases together: per-case checks plus unique ids and
/// plan-level split integrity.
pub fn check_all(cases: &[LoadedCase], kb: &KnowledgeBase, canary: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let file = |c: &LoadedCase| c.path.display().to_string();
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
    let mut plan_splits: BTreeMap<&str, BTreeSet<Split>> = BTreeMap::new();
    for c in cases {
        for message in check_case(c, kb, canary) {
            findings.push(Finding {
                file: file(c),
                message,
            });
        }
        *ids.entry(c.case.case_id.as_str()).or_default() += 1;
        plan_splits
            .entry(c.case.plan_ref.as_str())
            .or_default()
            .insert(c.case.split);
    }
    for c in cases {
        if ids[c.case.case_id.as_str()] > 1 {
            findings.push(Finding {
                file: file(c),
                message: format!("case_id {} is used more than once", c.case.case_id),
            });
        }
        if plan_splits[c.case.plan_ref.as_str()].len() > 1 {
            findings.push(Finding {
                file: file(c),
                message: format!("plan {} appears in more than one split", c.case.plan_ref),
            });
        }
    }
    findings
}

/// A held-out case inside the committed tree is a contamination risk (C3).
pub fn heldout_in_tree(cases_root: &Path) -> bool {
    cases_root.join("heldout").exists()
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Emails, phone numbers, long digit runs, dates of birth and member ids.
pub fn phi_hit(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for word in text.split_whitespace() {
        let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '@' && c != '.');
        if w.contains('@') && w.contains('.') {
            return Some("email".into());
        }
        let digits = w.chars().filter(char::is_ascii_digit).count();
        if digits >= 7 {
            return Some("long number".into());
        }
        if digits >= 4 && w.chars().filter(|c| *c == '/' || *c == '-').count() >= 2 {
            return Some("date or phone".into());
        }
    }
    for marker in [
        "dob",
        "date of birth",
        "ssn",
        "member id",
        "subscriber id",
        "mrn",
    ] {
        if crate::score::contains_word(&lower, marker) {
            return Some(marker.into());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::{Case, LoadedCase};
    use std::path::PathBuf;

    fn kb() -> KnowledgeBase {
        KnowledgeBase::load(&crate::repo_root().join("knowledge")).unwrap()
    }

    fn loaded(src: &str) -> LoadedCase {
        LoadedCase {
            case: Case::from_toml(src).unwrap(),
            path: PathBuf::from("eval/cases/dev/treatment_plan/T-900.toml"),
            sha256: String::new(),
        }
    }

    const CANARY: &str = "clarityeval-canary-test";

    fn base() -> String {
        crate::case::tests_support::MINIMAL.replace("plan-test-1", "plan-dev-900")
    }

    #[test]
    fn clean_case_passes() {
        let errs = check_case(&loaded(&base()), &kb(), CANARY);
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn catches_bad_ids_canary_and_names() {
        let src = base()
            .replace("case_id = \"T-900\"", "case_id = \"G-900\"")
            .replace(
                "name = \"Root canal\"",
                "name = \"Root canal with resin-based composite seal\"",
            );
        let errs = check_case(&loaded(&src), &kb(), "other");
        let all = errs.join("\n");
        assert!(all.contains("case_id"), "{all}");
        assert!(all.contains("canary"), "{all}");
        assert!(all.contains("our own name"), "{all}");
        assert!(all.contains("descriptor"), "{all}");
    }

    #[test]
    fn catches_phi_unless_planted() {
        let src = base().replace(
            "Lingering pain to cold on #19.",
            "Call 555-201-3344 about #19.",
        );
        let errs = check_case(&loaded(&src), &kb(), CANARY);
        assert!(errs.iter().any(|e| e.contains("identifier")), "{errs:?}");

        let planted = src
            .replace("plan_ref", "tags = [\"planted_identifiers\"]\nplan_ref")
            .replace(
                "[task]",
                "[expected]\nforbidden_strings = [\"555-201-3344\"]\n\n[task]",
            );
        let errs = check_case(&loaded(&planted), &kb(), CANARY);
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn catches_unresolved_expectations() {
        let src = base().replace(
            "[task]\nkind = \"presentation\"",
            "[task]\nkind = \"procedure_explanation\"\ntarget = \"p9\"\n\n[expected]\nmust_reference_facts = [\"code:D9999\", \"fact:ph7\"]\nmust_cite_any_of = [\"kb:nope\"]",
        );
        let errs = check_case(&loaded(&src), &kb(), CANARY).join("\n");
        assert!(errs.contains("task.target"), "{errs}");
        assert!(errs.contains("code:D9999"), "{errs}");
        assert!(errs.contains("fact:ph7"), "{errs}");
        assert!(errs.contains("kb:nope"), "{errs}");
    }

    #[test]
    fn split_integrity_and_duplicates() {
        let a = loaded(&base());
        let mut b = loaded(&base().replace("split = \"dev\"", "split = \"heldout\""));
        b.path = PathBuf::from("eval/heldout/cases/treatment_plan/T-900.toml");
        let findings = check_all(&[a, b], &kb(), CANARY);
        let all: Vec<_> = findings.iter().map(|f| f.message.as_str()).collect();
        assert!(all.iter().any(|m| m.contains("more than once")));
        assert!(all.iter().any(|m| m.contains("more than one split")));
    }
}
