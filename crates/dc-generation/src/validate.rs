//! Deterministic checks on generated text.
//!
//! These run on every generated sentence before anything reaches a patient.
//! They catch the failure modes that matter most and that a second model
//! might miss: invented costs, teeth that are not in the plan, and language
//! that diagnoses, promises or pressures.

use dc_core::{PlanSnapshot, ToothRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// Generated text contains a currency amount. Costs are rendered from the snapshot only.
    MoneyInText { sentence: usize, excerpt: String },
    /// Generated text names a tooth number that has no planned procedure.
    UnknownTooth { sentence: usize, tooth: String },
    /// Generated text uses a phrase from the blocked list.
    BlockedPhrase {
        sentence: usize,
        phrase: &'static str,
    },
}

/// Phrases that diagnose, guarantee, pressure or advise beyond the plan.
/// Matched case-insensitively on word boundaries. Kept short on purpose; the
/// review pass handles subtler cases.
const BLOCKED_PHRASES: &[&str] = &[
    "you have cancer",
    "guarantee",
    "guaranteed",
    "100% success",
    "never fail",
    "act now",
    "limited time",
    "before it's too late",
    "you must accept",
    "you should take",
    "stop taking",
    "antibiotics",
    "painless",
    "will definitely",
];

pub fn check_sentences(sentences: &[String], snapshot: &PlanSnapshot) -> Vec<Finding> {
    let planned: Vec<String> = snapshot
        .procedures
        .iter()
        .filter_map(|p| match p.tooth {
            ToothRef::Permanent(_) | ToothRef::Primary(_) => Some(p.tooth.to_string()),
            _ => None,
        })
        .collect();

    let mut findings = Vec::new();
    for (i, sentence) in sentences.iter().enumerate() {
        if let Some(excerpt) = find_money(sentence) {
            findings.push(Finding::MoneyInText {
                sentence: i,
                excerpt,
            });
        }
        for tooth in tooth_mentions(sentence) {
            if !planned.contains(&tooth) {
                findings.push(Finding::UnknownTooth { sentence: i, tooth });
            }
        }
        let lower = sentence.to_lowercase();
        for &phrase in BLOCKED_PHRASES {
            if contains_phrase(&lower, phrase) {
                findings.push(Finding::BlockedPhrase {
                    sentence: i,
                    phrase,
                });
            }
        }
    }
    findings
}

/// Finds a `$` amount or a number followed by "dollars".
fn find_money(text: &str) -> Option<String> {
    if let Some(pos) = text.find('$') {
        let tail: String = text[pos..].chars().take(12).collect();
        if tail.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
            return Some(tail);
        }
    }
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    words.windows(2).find_map(|w| {
        let is_number = w[0]
            .trim_matches(|c: char| c == ',' || c == '.')
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',');
        (is_number && !w[0].is_empty() && w[1].starts_with("dollar"))
            .then(|| format!("{} {}", w[0], w[1]))
    })
}

/// Tooth numbers written as `#30`, `tooth 30` or `teeth 14 and 19`.
fn tooth_mentions(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut after_tooth_word = false;
    for token in tokens {
        let cleaned = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '#');
        let lower = cleaned.to_lowercase();
        if let Some(num) = cleaned.strip_prefix('#') {
            push_tooth(num, &mut found);
            after_tooth_word = false;
        } else if lower == "tooth" || lower == "teeth" {
            after_tooth_word = true;
        } else if after_tooth_word {
            if lower == "and" || lower == "or" {
                continue;
            }
            if !push_tooth(cleaned, &mut found) {
                after_tooth_word = false;
            }
        }
    }
    found
}

fn push_tooth(raw: &str, out: &mut Vec<String>) -> bool {
    let is_tooth = !raw.is_empty()
        && raw.parse::<ToothRef>().is_ok_and(|t| t.is_single_tooth())
        && (raw.chars().all(|c| c.is_ascii_digit())
            || raw.len() == 1 && raw.chars().all(|c| c.is_ascii_uppercase()));
    if is_tooth {
        out.push(
            raw.parse::<ToothRef>()
                .map(|t| t.to_string())
                .unwrap_or_default(),
        );
    }
    is_tooth
}

fn contains_phrase(haystack: &str, phrase: &str) -> bool {
    haystack.match_indices(phrase).any(|(start, _)| {
        let before = haystack[..start].chars().next_back();
        let after = haystack[start + phrase.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_core::money::{Amount, Money};
    use dc_core::plan::{PatientContext, PatientKey, PlanKey, PlannedProcedure};
    use dc_core::procedure::{ProcedureCode, ProcedureStatus};

    fn snapshot() -> PlanSnapshot {
        let proc = |tooth: u8| PlannedProcedure {
            source_id: tooth.to_string(),
            code: ProcedureCode::new("D2391"),
            pms_description: String::new(),
            tooth: ToothRef::Permanent(tooth),
            surfaces: String::new(),
            phase: Some(1),
            status: ProcedureStatus::Planned,
            fee: Amount::Known(Money(21_000)),
            insurance_estimate: Amount::Unknown,
            patient_estimate: Amount::Unknown,
            note: None,
        };
        PlanSnapshot {
            plan: PlanKey("p".into()),
            patient: PatientContext {
                key: PatientKey("1".into()),
                first_name: "A".into(),
                age_years: None,
                preferred_language: None,
            },
            plan_name: "Active".into(),
            provider_display_name: None,
            procedures: vec![proc(14), proc(19), proc(30)],
        }
    }

    fn check(text: &str) -> Vec<Finding> {
        check_sentences(&[text.to_owned()], &snapshot())
    }

    #[test]
    fn clean_sentence_passes() {
        assert!(check("A filling repairs a small cavity in tooth 14.").is_empty());
        assert!(check("Teeth 14 and 19 each need a small filling.").is_empty());
    }

    #[test]
    fn flags_money() {
        assert!(matches!(
            check("This costs about $460.")[..],
            [Finding::MoneyInText { .. }]
        ));
        assert!(matches!(
            check("Expect around 460 dollars.")[..],
            [Finding::MoneyInText { .. }]
        ));
        assert!(check("Most visits take one hour.").is_empty());
    }

    #[test]
    fn flags_teeth_not_in_plan() {
        let findings = check("Tooth #3 also needs work.");
        assert_eq!(
            findings,
            vec![Finding::UnknownTooth {
                sentence: 0,
                tooth: "3".into()
            }]
        );
        let findings = check("Teeth 14 and 20 need fillings.");
        assert_eq!(
            findings,
            vec![Finding::UnknownTooth {
                sentence: 0,
                tooth: "20".into()
            }]
        );
    }

    #[test]
    fn flags_blocked_phrases_on_word_boundaries() {
        assert!(matches!(
            check("Success is guaranteed.")[..],
            [Finding::BlockedPhrase { .. }]
        ));
        assert!(check("Your dentist will discuss options.").is_empty());
        // "painless" inside another word should not match.
        assert!(check("The painlessness scale is a made-up word.").is_empty());
    }
}
