//! Clinical note sanitizing (P11).
//!
//! Strips names, dates, times, contact details, long digit runs and money
//! from a note linked to a planned procedure before it goes into a prompt.
//! This is minimization, not de-identification: free text can still identify
//! someone, which is why clinical notes go only to the practice's own
//! provider account and demo notes are synthetic.

const MAX_LEN: usize = 600;
const MONTHS: &[&str] = &[
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "jan",
    "feb",
    "mar",
    "apr",
    "jun",
    "jul",
    "aug",
    "sep",
    "sept",
    "oct",
    "nov",
    "dec",
];

/// Returns the sanitized note, or `None` when too little survives to use it.
pub fn sanitize(note: &str, names: &[&str]) -> Option<String> {
    let original_len = note.chars().filter(|c| !c.is_whitespace()).count();
    if original_len == 0 {
        return None;
    }
    let name_parts: Vec<String> = names
        .iter()
        .flat_map(|n| n.split_whitespace())
        .map(|n| {
            n.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|n| n.len() >= 2 && n != "dr")
        .collect();

    let mut kept: Vec<&str> = Vec::new();
    for word in note.split_whitespace() {
        let bare = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '#');
        let lower = bare.to_lowercase();
        let digits = bare.chars().filter(char::is_ascii_digit).count();
        let drop = name_parts.contains(&lower)
            || MONTHS.contains(&lower.as_str())
            || word.contains('@')
            || lower.starts_with("http")
            || lower.starts_with("www.")
            || word.contains('$')
            || word.contains('€')
            || word.contains('£')
            || digits >= 5
            || looks_like_date_or_time(bare);
        if !drop {
            kept.push(word);
        }
    }
    let text = kept.join(" ");
    // Judge how much was removed before truncating, so a long note is not
    // mistaken for one that was mostly identifiers.
    let kept_len = text.chars().filter(|c| !c.is_whitespace()).count();
    if kept_len == 0 || (kept_len as f64) < (original_len as f64) * 0.7 {
        return None;
    }
    Some(text.chars().take(MAX_LEN).collect())
}

/// `11/18`, `2026-11-18`, `11.18.26`, `10:30`, `10am`.
fn looks_like_date_or_time(word: &str) -> bool {
    let has_digit = word.chars().any(|c| c.is_ascii_digit());
    if !has_digit {
        return false;
    }
    let separators = word
        .chars()
        .filter(|c| matches!(c, '/' | '-' | '.' | ':'))
        .count();
    let all_date_chars = word
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '/' | '-' | '.' | ':'));
    let lower = word.to_lowercase();
    (all_date_chars && separators >= 1 && !word.starts_with('#'))
        || ((lower.ends_with("am") || lower.ends_with("pm"))
            && lower[..lower.len() - 2]
                .chars()
                .all(|c| c.is_ascii_digit() || c == ':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_clinical_content() {
        let note = "#30 deep caries into pulp, symptomatic. Recommend RCT, buildup, crown.";
        assert_eq!(
            sanitize(note, &["Daniel", "Dr. Patel"]).as_deref(),
            Some(note)
        );
    }

    #[test]
    fn strips_identifiers() {
        let note = "Seen by Dr. Patel 11/18/2026 at 10:30am, call 5551234567 or dan@example.com. \
                    #19 missing since extraction, bone width adequate on CBCT, plan implant then crown after healing. \
                    Adjacent teeth stable, no mobility, gingiva healthy, occlusion acceptable, est $2100.";
        let clean = sanitize(note, &["Daniel", "Dr. Patel"]).unwrap();
        for gone in [
            "Patel",
            "11/18/2026",
            "10:30am",
            "5551234567",
            "dan@example.com",
            "$2100",
        ] {
            assert!(!clean.contains(gone), "{gone} survived: {clean}");
        }
        assert!(clean.contains("#19 missing"));
    }

    #[test]
    fn drops_notes_that_are_mostly_identifiers() {
        assert_eq!(
            sanitize("Daniel Okafor 11/18/2026 5551234567", &["Daniel Okafor"]),
            None
        );
        assert_eq!(sanitize("   ", &[]), None);
    }

    #[test]
    fn truncates_long_notes() {
        let note = "watch occlusion ".repeat(100);
        assert!(sanitize(&note, &[]).unwrap().chars().count() <= MAX_LEN);
    }
}
