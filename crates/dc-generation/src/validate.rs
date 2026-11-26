//! Deterministic checks on model output (docs/AI_PIPELINE.md, V1 to V17).
//!
//! These run before the review pass and before anything reaches a patient.
//! They catch the failures that matter most and that a second model might
//! miss: invented costs or teeth, broken citations, timing claims, and
//! language that diagnoses, promises, pressures or advises on medication.

use crate::facts::FactSheet;
use crate::schema::{
    AtAGlanceOut, JourneyOut, NextStepsOut, ProcedureOut, Question, Sentence, WhyKind,
};
use dc_core::ToothRef;
use dc_core::content::ReadingLevel;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueSeverity {
    /// The section cannot be shown until repaired.
    Block,
    /// Worth a repair attempt, but not a reason to hold the section.
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub code: &'static str,
    pub path: String,
    pub severity: IssueSeverity,
    pub detail: String,
}

impl Issue {
    fn block(code: &'static str, path: &str, detail: impl Into<String>) -> Self {
        Issue {
            code,
            path: path.to_owned(),
            severity: IssueSeverity::Block,
            detail: detail.into(),
        }
    }

    fn warn(code: &'static str, path: &str, detail: impl Into<String>) -> Self {
        Issue {
            code,
            path: path.to_owned(),
            severity: IssueSeverity::Warn,
            detail: detail.into(),
        }
    }
}

/// What a section's sentences are allowed to cite and talk about.
pub struct Context<'a> {
    pub sheet: &'a FactSheet,
    /// `kb:<entry>#<passage>` ids sent with this request.
    pub kb_ids: HashSet<String>,
    /// `note:pN` ids sent with this request.
    pub note_ids: HashSet<String>,
    pub level: ReadingLevel,
}

pub fn has_blocking(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.severity == IssueSeverity::Block)
}

// Section entry points ------------------------------------------------------

pub fn check_at_a_glance(out: &AtAGlanceOut, cx: &Context) -> Vec<Issue> {
    let mut issues = Vec::new();
    para(&out.summary, "/summary", 2, 4, cx, &mut issues);
    para(&out.objective, "/objective", 0, 3, cx, &mut issues);
    for (i, s) in out.objective.iter().enumerate() {
        if !s.cites.iter().any(|c| c.starts_with("note:")) {
            issues.push(Issue::block(
                "V4",
                &format!("/objective/{i}"),
                "an objective must cite a clinical note",
            ));
        }
    }
    issues
}

pub fn check_journey(out: &JourneyOut, cx: &Context) -> Vec<Issue> {
    let mut issues = Vec::new();
    let expected: Vec<&str> = cx
        .sheet
        .phases
        .iter()
        .map(|p| p.reference.as_str())
        .collect();
    let got: Vec<&str> = out.phases.iter().map(|p| p.phase_ref.as_str()).collect();
    if expected != got {
        issues.push(Issue::block(
            "V2",
            "/phases",
            format!("phases must be exactly {expected:?} in order, got {got:?}"),
        ));
    }
    for (i, phase) in out.phases.iter().enumerate() {
        para(
            &phase.summary,
            &format!("/phases/{i}/summary"),
            1,
            3,
            cx,
            &mut issues,
        );
    }
    para(&out.dependencies, "/dependencies", 0, 3, cx, &mut issues);
    issues
}

pub fn check_procedure(out: &ProcedureOut, requested_ref: &str, cx: &Context) -> Vec<Issue> {
    let mut issues = Vec::new();
    if out.procedure_ref != requested_ref {
        issues.push(Issue::block(
            "V2",
            "/procedure_ref",
            format!("must be {requested_ref}"),
        ));
    }
    for (path, p, min) in [
        ("/what_it_is", &out.what_it_is, 1),
        ("/what_happens", &out.what_happens, 1),
        ("/recovery", &out.recovery, 1),
        ("/benefits", &out.benefits, 1),
        ("/if_delayed", &out.if_delayed, 1),
    ] {
        para(p, path, min, 6, cx, &mut issues);
    }
    para(&out.why.sentences, "/why/sentences", 1, 4, cx, &mut issues);
    let note_id = format!("note:{requested_ref}");
    let cites_note = out.why.sentences.iter().any(|s| s.cites.contains(&note_id));
    match out.why.kind {
        WhyKind::Documented if !cites_note => issues.push(Issue::block(
            "V4",
            "/why",
            format!("a documented reason must cite {note_id}"),
        )),
        WhyKind::Documented if !cx.note_ids.contains(&note_id) => issues.push(Issue::block(
            "V4",
            "/why",
            "no clinical note was provided, so the reason cannot be documented",
        )),
        WhyKind::CommonReasons
            if out
                .why
                .sentences
                .iter()
                .flat_map(|s| &s.cites)
                .any(|c| c.starts_with("note:")) =>
        {
            issues.push(Issue::block(
                "V4",
                "/why",
                "common reasons must not cite a clinical note",
            ));
        }
        _ => {}
    }
    // Every other section of a procedure must stay general.
    for (path, p) in [
        ("/what_it_is", &out.what_it_is),
        ("/what_happens", &out.what_happens),
        ("/recovery", &out.recovery),
        ("/benefits", &out.benefits),
        ("/if_delayed", &out.if_delayed),
    ] {
        for (i, s) in p.iter().enumerate() {
            if s.cites.iter().any(|c| c.starts_with("note:")) {
                issues.push(Issue::block(
                    "V4",
                    &format!("{path}/{i}"),
                    "only the reason may cite the clinical note",
                ));
            }
        }
    }
    questions(&out.questions, "/questions", cx, &mut issues);
    issues
}

pub fn check_next_steps(out: &NextStepsOut, cx: &Context) -> Vec<Issue> {
    let mut issues = Vec::new();
    para(&out.outlook, "/outlook", 1, 4, cx, &mut issues);
    para(&out.next_steps, "/next_steps", 1, 5, cx, &mut issues);
    questions(&out.questions, "/questions", cx, &mut issues);
    issues
}

// Shared checks -------------------------------------------------------------

fn para(p: &[Sentence], path: &str, min: usize, max: usize, cx: &Context, issues: &mut Vec<Issue>) {
    if p.len() < min || p.len() > max {
        issues.push(Issue::block(
            "V1",
            path,
            format!("needs {min} to {max} sentences, got {}", p.len()),
        ));
    }
    for (i, s) in p.iter().enumerate() {
        let path = format!("{path}/{i}");
        if s.cites.is_empty() || s.cites.len() > 4 {
            issues.push(Issue::block(
                "V3",
                &path,
                "each sentence needs 1 to 4 citations",
            ));
        }
        cites(&s.cites, &path, cx, issues);
        text(&s.text, &s.cites, &path, cx, issues);
    }
}

fn questions(qs: &[Question], path: &str, cx: &Context, issues: &mut Vec<Issue>) {
    if qs.len() < 2 || qs.len() > 4 {
        issues.push(Issue::block(
            "V1",
            path,
            format!("needs 2 to 4 questions, got {}", qs.len()),
        ));
    }
    for (i, q) in qs.iter().enumerate() {
        let path = format!("{path}/{i}");
        if !q.text.trim_end().ends_with('?') {
            issues.push(Issue::block(
                "V1",
                &path,
                "a question must end with a question mark",
            ));
        }
        cites(&q.cites, &path, cx, issues);
        text(&q.text, &q.cites, &path, cx, issues);
    }
}

fn cites(cites: &[String], path: &str, cx: &Context, issues: &mut Vec<Issue>) {
    let facts = cx.sheet.fact_ids();
    for c in cites {
        let known = facts.contains(c) || cx.kb_ids.contains(c) || cx.note_ids.contains(c);
        if !known {
            issues.push(Issue::block("V3", path, format!("unknown citation {c}")));
        }
    }
}

fn text(t: &str, cites: &[String], path: &str, cx: &Context, issues: &mut Vec<Issue>) {
    let trimmed = t.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 300 {
        issues.push(Issue::block("V1", path, "text must be 1 to 300 characters"));
    }
    if sentence_count(trimmed) > 1 {
        issues.push(Issue::block(
            "V9",
            path,
            "write exactly one sentence per item",
        ));
    }
    let lower = trimmed.to_lowercase();

    // V5: teeth must belong to the cited procedures (or the plan, if none is cited).
    let cited_refs: Vec<&str> = cites
        .iter()
        .filter_map(|c| c.strip_prefix("fact:").or_else(|| c.strip_prefix("note:")))
        .filter(|r| r.starts_with('p') && !r.starts_with("ph"))
        .collect();
    let allowed_teeth: Vec<ToothRef> = cx
        .sheet
        .procedures
        .iter()
        .filter(|p| cited_refs.is_empty() || cited_refs.contains(&p.reference.as_str()))
        .map(|p| p.tooth)
        .collect();
    for tooth in tooth_mentions(trimmed) {
        if !allowed_teeth.contains(&tooth) {
            issues.push(Issue::block(
                "V5",
                path,
                format!("tooth {tooth} is not part of the cited procedure"),
            ));
        }
    }

    // V6: procedure codes must be the cited ones.
    let allowed_codes: Vec<&str> = cx
        .sheet
        .procedures
        .iter()
        .filter(|p| cited_refs.is_empty() || cited_refs.contains(&p.reference.as_str()))
        .map(|p| p.code.as_str())
        .collect();
    for code in cdt_codes(trimmed) {
        if !allowed_codes.contains(&code.as_str()) {
            issues.push(Issue::block(
                "V6",
                path,
                format!("code {code} is not the cited procedure's code"),
            ));
        }
    }

    if let Some(found) = find_timing(&lower) {
        issues.push(Issue::block(
            "V7",
            path,
            format!("no visit counts, dates or durations (found {found:?})"),
        ));
    }
    if let Some(found) = find_money(trimmed) {
        issues.push(Issue::block(
            "V8",
            path,
            format!("no costs in text (found {found:?})"),
        ));
    }
    for (code, list, what) in [
        ("V10", DIAGNOSIS, "diagnosis"),
        ("V11", GUARANTEES, "guarantee"),
        ("V12", PROGNOSIS, "individual prognosis"),
        ("V13", PRESSURE, "pressure or urgency"),
        ("V14", MEDICATION, "medication advice"),
    ] {
        if let Some(phrase) = list.iter().find(|p| contains_phrase(&lower, p)) {
            issues.push(Issue::block(
                code,
                path,
                format!("{what} language: {phrase:?}"),
            ));
        }
    }
    if ["<", ">", "](", "http", "www.", "`", "**"]
        .iter()
        .any(|m| lower.contains(m))
        || trimmed.starts_with('#')
        || trimmed.starts_with('*')
    {
        issues.push(Issue::block(
            "V15",
            path,
            "no links, HTML or formatting marks",
        ));
    }
    for placeholder in placeholders(trimmed) {
        if placeholder != "practice_name" && placeholder != "provider_display_name" {
            issues.push(Issue::block(
                "V16",
                path,
                format!("placeholder {{{{{placeholder}}}}} is not allowed"),
            ));
        }
    }

    // V17: sentence length by reading level (a warning only).
    let words = trimmed.split_whitespace().count();
    let max_words = match cx.level {
        ReadingLevel::Simple => 20,
        ReadingLevel::Standard => 25,
        ReadingLevel::Detailed => 30,
    };
    if words > max_words {
        issues.push(Issue::warn(
            "V17",
            path,
            format!("{words} words; aim for {max_words} or fewer"),
        ));
    }
}

// Lexicons (English) --------------------------------------------------------

const DIAGNOSIS: &[&str] = &[
    "you have cancer",
    "you have an infection",
    "you have gum disease",
    "you have periodontitis",
    "you have a cavity",
    "you have decay",
    "you are diagnosed",
    "you've been diagnosed",
    "this means you have",
    "your diagnosis",
];
const GUARANTEES: &[&str] = &[
    "guarantee",
    "guaranteed",
    "will definitely",
    "100%",
    "permanently fix",
    "permanent fix",
    "never fail",
    "pain-free",
    "painless",
    "completely safe",
    "no risk",
    "risk-free",
    "lasts forever",
    "last forever",
    "last a lifetime",
];
const PROGNOSIS: &[&str] = &[
    "your tooth will last",
    "your tooth will fail",
    "you will lose",
    "your tooth will die",
    "will need to be pulled",
    "you will need dentures",
    "you will need an implant",
];
const PRESSURE: &[&str] = &[
    "act now",
    "don't wait",
    "do not wait",
    "immediately",
    "as soon as possible",
    "asap",
    "before it's too late",
    "limited time",
    "you must",
    "you have to",
    "don't delay",
    "do not delay",
];
const MEDICATION: &[&str] = &[
    "ibuprofen",
    "acetaminophen",
    "paracetamol",
    "aspirin",
    "amoxicillin",
    "antibiotic",
    "antibiotics",
    "penicillin",
    "opioid",
    "codeine",
    "hydrocodone",
    "naproxen",
    "advil",
    "tylenol",
    "motrin",
    "aleve",
    "painkiller",
    "take pain medicine",
    "take medication",
];

/// Matches on word boundaries.
pub fn contains_phrase(haystack: &str, phrase: &str) -> bool {
    haystack.match_indices(phrase).any(|(start, _)| {
        let before = haystack[..start].chars().next_back();
        let after = haystack[start + phrase.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

fn sentence_count(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut count = 1;
    for i in 0..chars.len().saturating_sub(2) {
        if matches!(chars[i], '.' | '!' | '?') && chars[i + 1] == ' ' && chars[i + 2].is_uppercase()
        {
            // Ignore abbreviations such as "Dr." before a name.
            let word_start = chars[..i]
                .iter()
                .rposition(|c| *c == ' ')
                .map_or(0, |p| p + 1);
            let word: String = chars[word_start..i].iter().collect();
            if !matches!(word.as_str(), "Dr" | "Mr" | "Mrs" | "Ms" | "e.g" | "i.e") {
                count += 1;
            }
        }
    }
    count
}

/// Teeth written as `#30`, `tooth 30`, `teeth 14 and 19` or `tooth K`.
fn tooth_mentions(text: &str) -> Vec<ToothRef> {
    let mut found = Vec::new();
    let mut after_tooth_word = false;
    for token in text.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '#');
        let lower = cleaned.to_lowercase();
        if let Some(num) = cleaned.strip_prefix('#') {
            if let Ok(t) = num.parse::<ToothRef>()
                && t.is_single_tooth()
            {
                found.push(t);
            }
            after_tooth_word = false;
        } else if lower == "tooth" || lower == "teeth" {
            after_tooth_word = true;
        } else if after_tooth_word {
            if lower == "and" || lower == "or" {
                continue;
            }
            let is_number = cleaned.chars().all(|c| c.is_ascii_digit()) && !cleaned.is_empty();
            let is_letter = cleaned.len() == 1 && cleaned.chars().all(|c| c.is_ascii_uppercase());
            match cleaned.parse::<ToothRef>() {
                Ok(t) if t.is_single_tooth() && (is_number || is_letter) => found.push(t),
                _ => after_tooth_word = false,
            }
        }
    }
    found
}

fn cdt_codes(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| {
            w.len() == 5 && w.starts_with('D') && w[1..].chars().all(|c| c.is_ascii_digit())
        })
        .map(str::to_owned)
        .collect()
}

const NUMBER_WORDS: &[&str] = &[
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "a few",
    "several",
    "a couple of",
    "single",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "10",
];
const TIME_UNITS: &[&str] = &[
    "visit",
    "visits",
    "appointment",
    "appointments",
    "minute",
    "minutes",
    "hour",
    "hours",
    "day",
    "days",
    "week",
    "weeks",
    "month",
    "months",
    "year",
    "years",
    "session",
    "sessions",
];
const CALENDAR: &[&str] = &[
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "tomorrow",
    "next week",
    "next month",
    "this week",
    "january",
    "february",
    "march",
    "april",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

fn find_timing(lower: &str) -> Option<String> {
    for word in CALENDAR {
        if contains_phrase(lower, word) {
            return Some((*word).to_owned());
        }
    }
    let words: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || c == ',' || c == '.')
        .filter(|w| !w.is_empty())
        .collect();
    for i in 0..words.len() {
        for n in NUMBER_WORDS {
            let n_words: Vec<&str> = n.split(' ').collect();
            let end = i + n_words.len();
            if end < words.len() && words[i..end] == n_words[..] {
                let unit = words[end].trim_end_matches(|c: char| !c.is_alphanumeric());
                if TIME_UNITS.contains(&unit) {
                    return Some(format!("{n} {unit}"));
                }
            }
        }
        // "30-minute", "2-3 weeks"
        let w = words[i];
        if w.chars().next().is_some_and(|c| c.is_ascii_digit())
            && let Some(unit) = w.split('-').next_back()
            && TIME_UNITS.contains(&unit)
        {
            return Some(w.to_owned());
        }
    }
    None
}

fn find_money(text: &str) -> Option<String> {
    if let Some(pos) = text.find(['$', '€', '£']) {
        return Some(text[pos..].chars().take(10).collect());
    }
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    words.windows(2).find_map(|w| {
        let number = w[0].trim_matches(|c: char| c == ',' || c == '.');
        let is_number =
            !number.is_empty() && number.chars().all(|c| c.is_ascii_digit() || c == ',');
        let money_word = ["dollar", "dollars", "usd", "cents", "euro", "euros"]
            .contains(&w[1].trim_matches(|c: char| !c.is_alphanumeric()));
        (is_number && money_word).then(|| format!("{} {}", w[0], w[1]))
    })
}

fn placeholders(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                out.push(after[..end].trim().to_owned());
                rest = &after[end + 2..];
            }
            None => {
                out.push(after.to_owned());
                break;
            }
        }
    }
    if rest.contains("}}") {
        out.push("}}".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Question, Why};
    use crate::test_support::{daniel, kb};

    fn cx(sheet: &FactSheet) -> Context<'_> {
        Context {
            sheet,
            kb_ids: ["kb:root_canal#what_it_is".to_owned()].into(),
            note_ids: ["note:p1".to_owned()].into(),
            level: ReadingLevel::Standard,
        }
    }

    fn s(text: &str, cites: &[&str]) -> Sentence {
        Sentence {
            text: text.into(),
            cites: cites.iter().map(|c| (*c).to_owned()).collect(),
        }
    }

    fn procedure(why_kind: WhyKind, why_cite: &str) -> ProcedureOut {
        let k = &["kb:root_canal#what_it_is"];
        ProcedureOut {
            procedure_ref: "p1".into(),
            what_it_is: vec![s(
                "A root canal treats the soft inside of tooth 30.",
                &["fact:p1", "kb:root_canal#what_it_is"],
            )],
            why: Why {
                kind: why_kind,
                sentences: vec![s(
                    "The decay in this tooth has reached the nerve.",
                    &[why_cite],
                )],
            },
            what_happens: vec![s("The area is numbed first.", k)],
            recovery: vec![s("Many people feel tender for a short while.", k)],
            benefits: vec![s("It lets you keep your natural tooth.", k)],
            if_delayed: vec![s("An infection can spread over time.", k)],
            questions: vec![
                Question {
                    text: "Will I need a crown afterwards?".into(),
                    cites: vec![],
                },
                Question {
                    text: "What should I eat after the visit?".into(),
                    cites: vec![],
                },
            ],
        }
    }

    fn codes(issues: &[Issue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.code).collect()
    }

    #[test]
    fn clean_procedure_passes() {
        let sheet = FactSheet::build(&daniel(), &kb());
        let issues = check_procedure(
            &procedure(WhyKind::Documented, "note:p1"),
            "p1",
            &cx(&sheet),
        );
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn documented_reason_needs_the_note() {
        let sheet = FactSheet::build(&daniel(), &kb());
        let issues = check_procedure(
            &procedure(WhyKind::Documented, "kb:root_canal#what_it_is"),
            "p1",
            &cx(&sheet),
        );
        assert!(codes(&issues).contains(&"V4"));
        let issues = check_procedure(
            &procedure(WhyKind::CommonReasons, "note:p1"),
            "p1",
            &cx(&sheet),
        );
        assert!(codes(&issues).contains(&"V4"));
    }

    #[test]
    fn wrong_ref_and_unknown_citation_block() {
        let sheet = FactSheet::build(&daniel(), &kb());
        let mut out = procedure(WhyKind::Documented, "note:p1");
        out.procedure_ref = "p2".into();
        out.benefits[0].cites = vec!["kb:made_up#x".into()];
        let found = codes(&check_procedure(&out, "p1", &cx(&sheet)));
        assert!(found.contains(&"V2") && found.contains(&"V3"));
    }

    fn one(text: &str, cites: &[&str]) -> Vec<&'static str> {
        let sheet = FactSheet::build(&daniel(), &kb());
        let mut issues = Vec::new();
        para(&[s(text, cites)], "/x", 1, 6, &cx(&sheet), &mut issues);
        codes(&issues)
    }

    #[test]
    fn tooth_must_belong_to_cited_procedure() {
        assert!(one("Tooth 30 is treated first.", &["fact:p1"]).is_empty());
        assert_eq!(one("Tooth 14 is treated first.", &["fact:p1"]), vec!["V5"]);
        assert_eq!(one("Tooth #3 also needs work.", &["fact:ph1"]), vec!["V5"]);
    }

    #[test]
    fn blocks_timing_money_and_risky_language() {
        assert_eq!(one("This takes two visits.", &["fact:p1"]), vec!["V7"]);
        assert_eq!(one("Recovery takes 3 days.", &["fact:p1"]), vec!["V7"]);
        assert_eq!(one("It costs $460.", &["fact:p1"]), vec!["V8"]);
        assert_eq!(
            one("You have decay in this tooth.", &["fact:p1"]),
            vec!["V10"]
        );
        assert_eq!(one("Results are guaranteed.", &["fact:p1"]), vec!["V11"]);
        assert_eq!(
            one("Book this as soon as possible.", &["fact:p1"]),
            vec!["V13"]
        );
        assert!(
            one(
                "If bleeding does not stop, call the practice right away.",
                &["fact:p1"]
            )
            .is_empty()
        );
        assert_eq!(
            one("Take ibuprofen for soreness.", &["fact:p1"]),
            vec!["V14"]
        );
        assert_eq!(
            one("See https://example.org for more.", &["fact:p1"]),
            vec!["V15"]
        );
        assert_eq!(
            one("Ask {{patient_name}} to call.", &["fact:p1"]),
            vec!["V16"]
        );
        assert!(one("Ask {{provider_display_name}} any question.", &["fact:p1"]).is_empty());
    }

    #[test]
    fn one_sentence_per_item() {
        assert_eq!(
            one("The tooth is numbed. Then it is cleaned.", &["fact:p1"]),
            vec!["V9"]
        );
        assert!(one("Dr. Lee numbs the tooth first.", &["fact:p1"]).is_empty());
    }

    #[test]
    fn journey_phases_must_match() {
        let sheet = FactSheet::build(&daniel(), &kb());
        let out = JourneyOut {
            phases: vec![crate::schema::PhaseOut {
                phase_ref: "ph1".into(),
                summary: vec![s("First the tooth is saved.", &["fact:ph1"])],
            }],
            dependencies: vec![],
        };
        assert!(codes(&check_journey(&out, &cx(&sheet))).contains(&"V2"));
    }
}
