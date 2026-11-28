//! Language and reading level variants (docs/AI_PIPELINE.md, S9; DECISIONS E23).
//!
//! A variant is derived from the approved English text of one section, never
//! from the facts or from unchecked text. One translator call rewrites the
//! section with one output sentence per source sentence. The deterministic
//! checks below run on the result, with one repair round, and a review call
//! compares it with the source. Anything that fails is not shown; the caller
//! falls back to the approved English with a "not available" label.

use crate::facts::FactSheet;
use crate::pipeline::{add_usage, call, is_safety_finding, strip_fences};
use crate::prompt;
use crate::schema::{NextStepsOut, ReviewIssue, ReviewVerdict, Sentence, Severity, Verdict};
use crate::validate::{self, Context, Issue, IssueSeverity};
use dc_ai::{Message, ReasoningEffort, Role, TextModel, Usage};
use dc_core::content::ReadingLevel;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;

pub const TRANSLATOR_PROMPT: &str = include_str!("../prompts/translator.blocks.v1.txt");
pub const TRANSLATION_REVIEWER_PROMPT: &str =
    include_str!("../prompts/reviewer.translation.v1.txt");

pub fn translator_version() -> String {
    prompt::version("translator.blocks@v1", TRANSLATOR_PROMPT)
}

pub fn translation_reviewer_version() -> String {
    prompt::version("reviewer.translation@v1", TRANSLATION_REVIEWER_PROMPT)
}

/// Most characters in one derived sentence. Translations run longer than
/// English; narration refuses sentences over 400 characters.
pub const MAX_SENTENCE_CHARS: usize = 400;
/// Most characters in a derived heading.
pub const MAX_HEADING_CHARS: usize = 80;
/// Block id that carries a section's questions.
pub const QUESTIONS_BLOCK: &str = "questions";

/// One block of patient text: a section field (or the questions) with its
/// heading and sentences. Placeholders stay unfilled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariantBlock {
    pub block_id: String,
    pub heading: String,
    pub sentences: Vec<String>,
}

/// A section's patient text, as sent to the translator and as it answers
/// (`TranslationOut` in docs/AI_PIPELINE.md).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariantText {
    pub blocks: Vec<VariantBlock>,
}

/// What to derive.
#[derive(Debug, Clone, Copy)]
pub struct Target<'a> {
    /// BCP 47 tag, e.g. `es`.
    pub language: &'a str,
    /// English name for the prompt, e.g. `Spanish`.
    pub language_name: &'a str,
    pub level: ReadingLevel,
    /// The level the approved English was written at.
    pub source_level: ReadingLevel,
    pub guardian_audience: bool,
}

impl Target<'_> {
    fn english(&self) -> bool {
        base(self.language) == "en"
    }
}

/// The models for translation and its review.
#[derive(Clone)]
pub struct VariantModels {
    pub translator: Arc<dyn TextModel>,
    pub translator_model: String,
    pub reviewer: Arc<dyn TextModel>,
    pub reviewer_model: String,
    pub reasoning: Option<ReasoningEffort>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Passed the checks and the review; may be shown.
    Ready {
        text: VariantText,
        /// Warnings and minor review notes, for staff.
        notes: Vec<String>,
    },
    /// Failed the checks after a repair, or the review held it. Final for
    /// this source text, target and prompt version.
    Failed { reasons: Vec<String> },
    /// A model could not be reached or answered unreadably. Worth trying
    /// again later.
    Unavailable { reason: String },
}

#[derive(Debug, Clone)]
pub struct Derived {
    pub outcome: Outcome,
    pub usage: Usage,
    pub attempts: u32,
}

/// Cache key for one derived section: SHA-256 over the source text, the
/// target and the prompts and models that produce and review it. Any staff
/// edit, confirmation or prompt change gives a new key.
pub fn cache_hash(source: &VariantText, target: &Target, models: &VariantModels) -> String {
    let canonical = json!({
        "source": source,
        "language": target.language,
        "level": target.level.to_string(),
        "source_level": target.source_level.to_string(),
        "guardian_audience": target.guardian_audience,
        "translator": translator_version(),
        "translator_model": models.translator_model,
        "reviewer": translation_reviewer_version(),
        "reviewer_model": models.reviewer_model,
    });
    hex::encode(Sha256::digest(canonical.to_string().as_bytes()))
}

/// JSON Schema of the translator's answer.
pub fn schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["blocks"],
        "properties": {"blocks": {"type": "array", "items": {
            "type": "object", "additionalProperties": false,
            "required": ["block_id", "heading", "sentences"],
            "properties": {
                "block_id": {"type": "string"},
                "heading": {"type": "string"},
                "sentences": {"type": "array", "items": {"type": "string"}}
            }
        }}}
    })
}

fn target_block(target: &Target) -> String {
    format!(
        "<target>\nlanguage: {} ({})\nreading_level: {}\nsource_reading_level: {}\naudience: {}\n</target>",
        target.language_name,
        target.language,
        target.level,
        target.source_level,
        if target.guardian_audience {
            "parent or guardian of a child patient"
        } else {
            "the patient"
        }
    )
}

/// The translator's user turn.
pub fn translator_user(source: &VariantText, target: &Target, problems: &[String]) -> String {
    let mut out = format!(
        "{}\n<approved_text>\n{}\n</approved_text>\n",
        target_block(target),
        serde_json::to_string(source).expect("variant text serializes")
    );
    if !problems.is_empty() {
        out.push_str("<previous_problems>\n");
        for p in problems {
            out.push_str("- ");
            out.push_str(p);
            out.push('\n');
        }
        out.push_str("</previous_problems>\n");
    }
    out.push_str("<task>Rewrite every block for the target language and reading level.</task>");
    out
}

fn reviewer_user(source: &VariantText, draft: &VariantText, target: &Target) -> String {
    format!(
        "{}\n<approved_text>\n{}\n</approved_text>\n<draft>\n{}\n</draft>",
        target_block(target),
        serde_json::to_string(source).expect("variant text serializes"),
        serde_json::to_string(draft).expect("variant text serializes"),
    )
}

/// Derives one section: translate, check (one repair), review.
pub async fn derive(
    models: &VariantModels,
    source: &VariantText,
    target: &Target<'_>,
    sheet: &FactSheet,
) -> Derived {
    let mut usage = Usage::default();
    let mut problems: Vec<String> = Vec::new();
    let mut accepted = None;
    let mut attempts = 0;
    for round in 0..2 {
        attempts = round + 1;
        let messages = vec![
            Message {
                role: Role::System,
                content: TRANSLATOR_PROMPT.into(),
            },
            Message {
                role: Role::User,
                content: translator_user(source, target, &problems),
            },
        ];
        let reply = match call(
            &models.translator,
            &models.translator_model,
            messages,
            schema(),
            models.reasoning,
        )
        .await
        {
            Ok(reply) => reply,
            Err(err) => {
                return Derived {
                    outcome: Outcome::Unavailable {
                        reason: format!("The translation model could not be reached ({err})."),
                    },
                    usage,
                    attempts,
                };
            }
        };
        add_usage(&mut usage, &reply.usage);
        let issues = match serde_json::from_str::<VariantText>(strip_fences(&reply.content)) {
            Ok(text) => {
                let issues = check(source, &text, target, sheet);
                if !validate::has_blocking(&issues) {
                    accepted = Some((text, issues));
                    break;
                }
                issues
            }
            Err(err) => vec![Issue {
                code: "V1",
                path: String::new(),
                severity: IssueSeverity::Block,
                detail: format!("the answer did not match the schema ({err})"),
            }],
        };
        problems = issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Block)
            .map(describe)
            .collect();
    }
    let Some((text, warnings)) = accepted else {
        return Derived {
            outcome: Outcome::Failed { reasons: problems },
            usage,
            attempts,
        };
    };

    let messages = vec![
        Message {
            role: Role::System,
            content: TRANSLATION_REVIEWER_PROMPT.into(),
        },
        Message {
            role: Role::User,
            content: reviewer_user(source, &text, target),
        },
    ];
    let reply = match call(
        &models.reviewer,
        &models.reviewer_model,
        messages,
        crate::schema::review_schema(),
        models.reasoning,
    )
    .await
    {
        Ok(reply) => reply,
        Err(err) => {
            return Derived {
                outcome: Outcome::Unavailable {
                    reason: format!("The review model could not be reached ({err})."),
                },
                usage,
                attempts,
            };
        }
    };
    add_usage(&mut usage, &reply.usage);
    let Ok(verdict) = serde_json::from_str::<ReviewVerdict>(strip_fences(&reply.content)) else {
        return Derived {
            outcome: Outcome::Unavailable {
                reason: "The review model returned an unreadable verdict.".into(),
            },
            usage,
            attempts,
        };
    };
    let review_notes: Vec<String> = verdict
        .issues
        .iter()
        .map(|i| {
            format!(
                "{} {:?} at {}: {}",
                i.category, i.severity, i.path, i.explanation
            )
        })
        .collect();
    let outcome = if review_holds(&verdict) {
        Outcome::Failed {
            reasons: review_notes,
        }
    } else {
        Outcome::Ready {
            text,
            notes: warnings.iter().map(describe).chain(review_notes).collect(),
        }
    };
    Derived {
        outcome,
        usage,
        attempts,
    }
}

/// A review holds the variant on a block verdict, any critical finding, or
/// a major finding about safety (R1 to R5) or meaning (R8). Clarity and
/// audience notes (R6, R7) and minor findings do not.
fn review_holds(verdict: &ReviewVerdict) -> bool {
    let meaning = |i: &ReviewIssue| {
        i.category.trim().eq_ignore_ascii_case("r8") && i.severity != Severity::Minor
    };
    verdict.verdict == Verdict::Block
        || verdict
            .issues
            .iter()
            .any(|i| i.severity == Severity::Critical || is_safety_finding(i) || meaning(i))
}

fn describe(issue: &Issue) -> String {
    if issue.path.is_empty() {
        format!("{}: {}", issue.code, issue.detail)
    } else {
        format!("{} at {}: {}", issue.code, issue.path, issue.detail)
    }
}

fn block(code: &'static str, path: &str, detail: impl Into<String>) -> Issue {
    Issue {
        code,
        path: path.to_owned(),
        severity: IssueSeverity::Block,
        detail: detail.into(),
    }
}

// Deterministic checks -------------------------------------------------------

/// Checks a derived section against its source: the same blocks and one
/// sentence per source sentence (V20), the same placeholders (V16), no new
/// digits (V20), no costs (V8), no markup (V15), the target language (V19)
/// and sentence length (V17 for English, V18 otherwise, warnings). English
/// output also goes through the English checks V5 to V15 again, as staff
/// text does, since a rewrite at another level is new English text.
pub fn check(
    source: &VariantText,
    derived: &VariantText,
    target: &Target,
    sheet: &FactSheet,
) -> Vec<Issue> {
    let mut issues = Vec::new();
    let ids = |t: &VariantText| {
        t.blocks
            .iter()
            .map(|b| b.block_id.clone())
            .collect::<Vec<_>>()
    };
    if ids(source) != ids(derived) {
        issues.push(block(
            "V20",
            "",
            format!("blocks must be {:?} in this order", ids(source)),
        ));
        return issues;
    }
    let mut english_sentences = Vec::new();
    for (src, out) in source.blocks.iter().zip(&derived.blocks) {
        let path = format!("/{}", src.block_id);
        heading(&src.heading, &out.heading, &path, target, &mut issues);
        if src.sentences.len() != out.sentences.len() {
            issues.push(block(
                "V20",
                &path,
                format!(
                    "needs {} sentences, one per source sentence, got {}",
                    src.sentences.len(),
                    out.sentences.len()
                ),
            ));
            continue;
        }
        for (i, (s, o)) in src.sentences.iter().zip(&out.sentences).enumerate() {
            let path = format!("{path}/{i}");
            sentence(
                s,
                o,
                &path,
                src.block_id == QUESTIONS_BLOCK,
                target,
                &mut issues,
            );
            english_sentences.push((path, o.clone()));
        }
    }
    if target.english() {
        english_checks(&english_sentences, target.level, sheet, &mut issues);
    }
    issues
}

fn heading(src: &str, out: &str, path: &str, target: &Target, issues: &mut Vec<Issue>) {
    let path = format!("{path}/heading");
    let out = out.trim();
    if src.trim().is_empty() {
        if !out.is_empty() {
            issues.push(block("V20", &path, "this block has no heading"));
        }
        return;
    }
    if out.is_empty() || out.chars().count() > MAX_HEADING_CHARS {
        issues.push(block(
            "V1",
            &path,
            format!("a heading must be 1 to {MAX_HEADING_CHARS} characters"),
        ));
    }
    common(src, out, &path, target, issues);
}

fn sentence(
    src: &str,
    out: &str,
    path: &str,
    question: bool,
    target: &Target,
    issues: &mut Vec<Issue>,
) {
    let out = out.trim();
    if out.is_empty() || out.chars().count() > MAX_SENTENCE_CHARS {
        issues.push(block(
            "V1",
            path,
            format!("a sentence must be 1 to {MAX_SENTENCE_CHARS} characters"),
        ));
    }
    if validate::sentence_count(out) > 1 {
        issues.push(block(
            "V20",
            path,
            "write exactly one sentence for each source sentence",
        ));
    }
    if question && !out.ends_with(['?', '？', '؟']) {
        issues.push(block(
            "V1",
            path,
            "a question must end with a question mark",
        ));
    }
    common(src, out, path, target, issues);
    if !target.english() {
        level_length(out, path, target, issues);
    }
}

/// Checks for headings and sentences in every language.
fn common(src: &str, out: &str, path: &str, target: &Target, issues: &mut Vec<Issue>) {
    if out.chars().any(char::is_control) {
        issues.push(block("V1", path, "no line breaks or control characters"));
    }
    let mut want = validate::placeholders(src);
    let mut got = validate::placeholders(out);
    want.sort();
    got.sort();
    if want != got {
        issues.push(block(
            "V16",
            path,
            format!("keep the placeholders exactly as written ({want:?}, got {got:?})"),
        ));
    }
    let src_numbers = numbers(src);
    if let Some(n) = numbers(out).into_iter().find(|n| !src_numbers.contains(n)) {
        issues.push(block(
            "V20",
            path,
            format!("number {n:?} is not in the source sentence"),
        ));
    }
    if out.chars().any(|c| !c.is_ascii_digit() && c.is_numeric()) {
        issues.push(block(
            "V20",
            path,
            "write numbers in the same digits as the source",
        ));
    }
    if let Some(found) = validate::find_money(out) {
        issues.push(block(
            "V8",
            path,
            format!("no costs in text (found {found:?})"),
        ));
    }
    if out
        .chars()
        .any(|c| matches!(c, '$' | '€' | '£' | '¥' | '₩' | '₫' | '₱' | '₽'))
    {
        issues.push(block("V8", path, "no currency symbols in text"));
    }
    let lower = out.to_lowercase();
    if ["<", ">", "](", "http", "www.", "`", "**"]
        .iter()
        .any(|m| lower.contains(m))
        || out.starts_with('#')
        || out.starts_with('*')
    {
        issues.push(block("V15", path, "no links, HTML or formatting marks"));
    }
    if let Some(problem) = language_problem(src, out, target) {
        issues.push(block("V19", path, problem));
    }
}

/// Runs of ASCII digits in `text`.
fn numbers(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// V18: a sentence much longer than the level's English limit (warning).
fn level_length(out: &str, path: &str, target: &Target, issues: &mut Vec<Issue>) {
    if expected_script(target.language) == Script::Han {
        return;
    }
    let words = out.split_whitespace().count();
    let limit = match target.level {
        ReadingLevel::Simple => 30,
        ReadingLevel::Standard => 38,
        ReadingLevel::Detailed => 45,
    };
    if words > limit {
        issues.push(Issue {
            code: "V18",
            path: path.to_owned(),
            severity: IssueSeverity::Warn,
            detail: format!("{words} words; aim for {limit} or fewer"),
        });
    }
}

/// The English checks staff text gets (V5 to V17 without citations), for
/// English output at another reading level.
fn english_checks(
    sentences: &[(String, String)],
    level: ReadingLevel,
    sheet: &FactSheet,
    issues: &mut Vec<Issue>,
) {
    let out = NextStepsOut {
        outlook: sentences
            .iter()
            .map(|(_, text)| Sentence {
                text: text.clone(),
                cites: Vec::new(),
            })
            .collect(),
        next_steps: Vec::new(),
        questions: Vec::new(),
    };
    let cx = Context {
        sheet,
        kb_ids: HashSet::new(),
        note_ids: HashSet::new(),
        level,
    };
    for issue in validate::check_next_steps(&out, &cx) {
        if matches!(issue.code, "V3" | "V4") {
            continue;
        }
        let Some(index) = issue
            .path
            .strip_prefix("/outlook/")
            .and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        // Length is checked above with the variant limit.
        if issue.code == "V1" && issue.detail.contains("characters") {
            continue;
        }
        issues.push(Issue {
            path: sentences[index].0.clone(),
            ..issue
        });
    }
}

// Language detection (V19) ---------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    Latin,
    Cyrillic,
    Han,
    Hangul,
    Arabic,
}

fn base(tag: &str) -> String {
    tag.split('-').next().unwrap_or(tag).to_ascii_lowercase()
}

fn expected_script(tag: &str) -> Script {
    match base(tag).as_str() {
        "zh" | "ja" => Script::Han,
        "ko" => Script::Hangul,
        "ar" | "fa" | "ur" => Script::Arabic,
        "ru" | "uk" | "bg" | "sr" | "mk" | "be" => Script::Cyrillic,
        _ => Script::Latin,
    }
}

fn script_of(c: char) -> Option<Script> {
    match u32::from(c) {
        0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x24F | 0x1E00..=0x1EFF => Some(Script::Latin),
        0x400..=0x4FF => Some(Script::Cyrillic),
        0x3400..=0x4DBF | 0x4E00..=0x9FFF => Some(Script::Han),
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Some(Script::Hangul),
        0x600..=0x6FF | 0x750..=0x77F => Some(Script::Arabic),
        _ => None,
    }
}

/// Words that are common in English and rare in the other Latin-script
/// languages offered, used to spot text left in English.
const ENGLISH_MARKERS: &[&str] = &[
    "the", "and", "you", "your", "is", "are", "this", "that", "with", "will", "for", "of", "to",
    "it", "be", "can", "what", "have", "has", "they", "after", "before", "about", "which",
    "should", "not", "how", "when", "from", "there", "these", "their", "them",
];

/// Placeholders removed, so their Latin names do not count as letters.
fn without_placeholders(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        match rest[start..].find("}}") {
            Some(end) => rest = &rest[start + end + 2..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Why `out` does not read as the target language, if it does not. Text
/// with fewer than 12 letters is too short to judge and passes.
fn language_problem(src: &str, out: &str, target: &Target) -> Option<String> {
    let text = without_placeholders(out);
    let scripts: Vec<Script> = text.chars().filter_map(script_of).collect();
    if scripts.len() < 12 {
        return None;
    }
    let expected = expected_script(target.language);
    let share = scripts.iter().filter(|s| **s == expected).count() as f64 / scripts.len() as f64;
    if share < 0.6 {
        return Some(format!("not written in {}", target.language_name));
    }
    if expected != Script::Latin {
        return None;
    }
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphabetic() && c != '\'')
        .filter(|w| !w.is_empty())
        .collect();
    let english = words.iter().filter(|w| ENGLISH_MARKERS.contains(w)).count();
    if target.english() {
        return (words.len() >= 6 && english == 0).then(|| "does not read as English".into());
    }
    if words.len() >= 5 && english * 100 >= words.len() * 35 {
        return Some(format!("reads as English, not {}", target.language_name));
    }
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    if words.len() >= 4 && same(src, out) {
        return Some(format!(
            "left in English instead of {}",
            target.language_name
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{daniel, kb};
    use dc_ai::fake::FakeTextModel;

    fn spanish() -> Target<'static> {
        Target {
            language: "es",
            language_name: "Spanish",
            level: ReadingLevel::Standard,
            source_level: ReadingLevel::Standard,
            guardian_audience: false,
        }
    }

    fn simple_english() -> Target<'static> {
        Target {
            language: "en",
            language_name: "English",
            level: ReadingLevel::Simple,
            ..spanish()
        }
    }

    fn text(blocks: &[(&str, &str, &[&str])]) -> VariantText {
        VariantText {
            blocks: blocks
                .iter()
                .map(|(id, heading, sentences)| VariantBlock {
                    block_id: (*id).into(),
                    heading: (*heading).into(),
                    sentences: sentences.iter().map(|s| (*s).to_owned()).collect(),
                })
                .collect(),
        }
    }

    fn source() -> VariantText {
        text(&[
            (
                "what_happens",
                "What happens",
                &[
                    "The team at {{practice_name}} numbs tooth 30 first.",
                    "You can rest afterwards.",
                ],
            ),
            ("questions", "", &["What should I expect afterwards?"]),
        ])
    }

    fn good_spanish() -> VariantText {
        text(&[
            (
                "what_happens",
                "Qué sucede",
                &[
                    "El equipo de {{practice_name}} adormece primero el diente 30.",
                    "Después puede descansar un poco.",
                ],
            ),
            (
                "questions",
                "",
                &["¿Qué debo esperar después del tratamiento?"],
            ),
        ])
    }

    fn codes(issues: &[Issue]) -> Vec<&'static str> {
        issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Block)
            .map(|i| i.code)
            .collect()
    }

    fn check_es(derived: &VariantText) -> Vec<&'static str> {
        let sheet = FactSheet::build(&daniel(), &kb());
        codes(&check(&source(), derived, &spanish(), &sheet))
    }

    #[test]
    fn a_faithful_translation_passes() {
        assert!(check_es(&good_spanish()).is_empty());
    }

    #[test]
    fn structure_and_sentence_parity_are_enforced() {
        let mut fewer = good_spanish();
        fewer.blocks[0].sentences.pop();
        assert_eq!(check_es(&fewer), ["V20"]);
        let mut renamed = good_spanish();
        renamed.blocks[0].block_id = "why".into();
        assert_eq!(check_es(&renamed), ["V20"]);
        let mut joined = good_spanish();
        joined.blocks[0].sentences[1] = "Después puede descansar. Luego vuelve a casa.".into();
        assert!(check_es(&joined).contains(&"V20"));
    }

    #[test]
    fn placeholders_numbers_and_costs_must_not_change() {
        let mut dropped = good_spanish();
        dropped.blocks[0].sentences[0] =
            "El equipo adormece primero el diente 30 con cuidado.".into();
        assert_eq!(check_es(&dropped), ["V16"]);
        let mut wrong_tooth = good_spanish();
        wrong_tooth.blocks[0].sentences[0] =
            "El equipo de {{practice_name}} adormece primero el diente 31.".into();
        assert_eq!(check_es(&wrong_tooth), ["V20"]);
        let mut cost = good_spanish();
        cost.blocks[0].sentences[1] = "Después puede descansar, y no cuesta €.".into();
        assert!(check_es(&cost).contains(&"V8"));
        let mut question = good_spanish();
        question.blocks[1].sentences[0] =
            "Puede preguntar qué esperar después del tratamiento.".into();
        assert_eq!(check_es(&question), ["V1"]);
    }

    #[test]
    fn text_left_in_english_or_in_another_script_fails() {
        let mut english = good_spanish();
        english.blocks[0].sentences[1] = "You can rest afterwards.".into();
        assert_eq!(check_es(&english), ["V19"]);
        let mut half = good_spanish();
        half.blocks[0].sentences[1] = "Después you can rest and relax at home.".into();
        assert_eq!(check_es(&half), ["V19"]);

        let sheet = FactSheet::build(&daniel(), &kb());
        let russian = Target {
            language: "ru",
            language_name: "Russian",
            ..spanish()
        };
        let issues = check(&source(), &good_spanish(), &russian, &sheet);
        assert!(codes(&issues).iter().all(|c| *c == "V19"));
        assert!(!issues.is_empty());
    }

    #[test]
    fn english_rewrites_get_the_english_checks() {
        let sheet = FactSheet::build(&daniel(), &kb());
        let mut simple = source();
        simple.blocks[0].heading = "What happens".into();
        simple.blocks[0].sentences[1] = "You can rest after it is done.".into();
        assert!(codes(&check(&source(), &simple, &simple_english(), &sheet)).is_empty());
        simple.blocks[0].sentences[1] = "Success is guaranteed for you.".into();
        assert_eq!(
            codes(&check(&source(), &simple, &simple_english(), &sheet)),
            ["V11"]
        );
    }

    fn models(translator: FakeTextModel, reviewer: FakeTextModel) -> VariantModels {
        VariantModels {
            translator: Arc::new(translator),
            translator_model: "t".into(),
            reviewer: Arc::new(reviewer),
            reviewer_model: "r".into(),
            reasoning: None,
        }
    }

    fn pass() -> String {
        json!({ "verdict": "pass", "issues": [] }).to_string()
    }

    #[tokio::test]
    async fn derive_repairs_once_then_reviews() {
        let sheet = FactSheet::build(&daniel(), &kb());
        let mut bad = good_spanish();
        bad.blocks[0].sentences[1] = "You can rest afterwards.".into();
        let m = models(
            FakeTextModel::with_responses([
                serde_json::to_string(&bad).unwrap(),
                serde_json::to_string(&good_spanish()).unwrap(),
            ]),
            FakeTextModel::with_responses([pass()]),
        );
        let derived = derive(&m, &source(), &spanish(), &sheet).await;
        assert_eq!(derived.attempts, 2);
        match derived.outcome {
            Outcome::Ready { text, .. } => assert_eq!(text, good_spanish()),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn derive_fails_closed() {
        let sheet = FactSheet::build(&daniel(), &kb());
        let m = models(
            FakeTextModel::with_responses(["not json", "still not json"]),
            FakeTextModel::default(),
        );
        let derived = derive(&m, &source(), &spanish(), &sheet).await;
        assert!(matches!(derived.outcome, Outcome::Failed { .. }));

        let meaning = json!({ "verdict": "revise", "issues": [{
            "path": "/blocks/0/sentences/0", "category": "R8", "severity": "major",
            "explanation": "The meaning changed."
        }]})
        .to_string();
        let m = models(
            FakeTextModel::with_responses([serde_json::to_string(&good_spanish()).unwrap()]),
            FakeTextModel::with_responses([meaning]),
        );
        let derived = derive(&m, &source(), &spanish(), &sheet).await;
        assert!(matches!(derived.outcome, Outcome::Failed { .. }));

        let clarity = json!({ "verdict": "revise", "issues": [{
            "path": "/blocks/0/sentences/0", "category": "R6", "severity": "major",
            "explanation": "A little stiff."
        }]})
        .to_string();
        let m = models(
            FakeTextModel::with_responses([serde_json::to_string(&good_spanish()).unwrap()]),
            FakeTextModel::with_responses([clarity]),
        );
        let derived = derive(&m, &source(), &spanish(), &sheet).await;
        assert!(matches!(derived.outcome, Outcome::Ready { .. }));
    }

    #[test]
    fn cache_hash_follows_source_and_target() {
        let m = models(FakeTextModel::default(), FakeTextModel::default());
        let a = cache_hash(&source(), &spanish(), &m);
        assert_eq!(a, cache_hash(&source(), &spanish(), &m));
        assert_ne!(a, cache_hash(&source(), &simple_english(), &m));
        let mut edited = source();
        edited.blocks[0].sentences[1] = "You may rest afterwards.".into();
        assert_ne!(a, cache_hash(&edited, &spanish(), &m));
    }

    #[test]
    fn prompts_carry_the_target_and_mark_data() {
        let user = translator_user(&source(), &spanish(), &["V19 at /x: wrong".into()]);
        assert!(user.contains("language: Spanish (es)"));
        assert!(user.contains("<approved_text>"));
        assert!(user.contains("<previous_problems>"));
        assert!(translator_version().starts_with("translator.blocks@v1+"));
        assert!(TRANSLATOR_PROMPT.contains("data"));
    }
}
