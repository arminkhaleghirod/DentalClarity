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

    let words: Vec<&str> = note.split_whitespace().collect();
    let bare = |i: usize| words[i].trim_matches(|c: char| !c.is_alphanumeric() && c != '#');
    let mut drop = vec![false; words.len()];
    for (i, word) in words.iter().enumerate() {
        let bare = bare(i);
        let lower = bare.to_lowercase();
        let digits = bare.chars().filter(char::is_ascii_digit).count();
        // "May" is also a verb, so it counts as a month only in a date such
        // as "May 4" or "4th May".
        let may_date = lower == "may"
            && (words.get(i + 1).is_some_and(|w| is_day_or_year(w))
                || i.checked_sub(1).is_some_and(|p| is_ordinal_day(words[p])));
        let month = MONTHS.contains(&lower.as_str()) || may_date;
        if month {
            // Take the day and year next to the month with it: "March 4, 2024".
            for j in [i.wrapping_sub(1), i + 1, i + 2] {
                if words.get(j).is_some_and(|w| is_day_or_year(w)) {
                    drop[j] = true;
                }
            }
        }
        drop[i] |= name_parts.contains(&lower)
            || month
            || word.contains('@')
            || lower.starts_with("http")
            || lower.starts_with("www.")
            || word.contains('$')
            || word.contains('€')
            || word.contains('£')
            || digits >= 5
            || is_year(bare)
            || looks_like_date_or_time(bare);
    }
    let kept: Vec<&str> = words
        .iter()
        .zip(&drop)
        .filter(|(_, d)| !**d)
        .map(|(w, _)| *w)
        .collect();
    let text = kept.join(" ");
    // Judge how much was removed before truncating, so a long note is not
    // mistaken for one that was mostly identifiers.
    let kept_len = text.chars().filter(|c| !c.is_whitespace()).count();
    if kept_len == 0 || (kept_len as f64) < (original_len as f64) * 0.7 {
        return None;
    }
    Some(text.chars().take(MAX_LEN).collect())
}

/// A year from 1900 to 2099, such as the one in "extraction 2024".
fn is_year(word: &str) -> bool {
    word.len() == 4
        && word
            .parse::<u32>()
            .is_ok_and(|y| (1900..=2099).contains(&y))
}

/// A day of the month or a year next to a month name: `4`, `4th,`, `2024`.
fn is_day_or_year(word: &str) -> bool {
    let bare = word.trim_matches(|c: char| !c.is_alphanumeric());
    let day = ["st", "nd", "rd", "th"]
        .iter()
        .find_map(|s| bare.to_lowercase().strip_suffix(s).map(str::to_owned))
        .unwrap_or_else(|| bare.to_owned());
    is_year(bare) || day.parse::<u32>().is_ok_and(|d| (1..=31).contains(&d))
}

/// A day written with an ordinal suffix, such as `4th`.
fn is_ordinal_day(word: &str) -> bool {
    let lower = word
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    ["st", "nd", "rd", "th"].iter().any(|s| {
        lower
            .strip_suffix(s)
            .is_some_and(|d| d.parse::<u32>().is_ok_and(|d| (1..=31).contains(&d)))
    })
}

/// Dates with two or more separators (`11/18/2026`, `2026-11-18`, `11.18.26`),
/// a month and day such as `11/18`, and times (`10:30`, `10am`). A single
/// hyphen or dot is a range or a measurement (`5-6`, `2.5`), not a date.
fn looks_like_date_or_time(word: &str) -> bool {
    if word.starts_with('#') || !word.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let all_date_chars = word
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '/' | '-' | '.' | ':'));
    let separators = word
        .chars()
        .filter(|c| matches!(c, '/' | '-' | '.'))
        .count();
    let month_day = word.split_once('/').is_some_and(|(m, d)| {
        m.parse::<u32>().is_ok_and(|m| (1..=12).contains(&m))
            && d.parse::<u32>().is_ok_and(|d| (1..=31).contains(&d))
    });
    let lower = word.to_lowercase();
    (all_date_chars && (separators >= 2 || month_day || word.contains(':')))
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
    fn ranges_and_the_verb_may_are_not_dates() {
        let note =
            "Generalized 5-6 mm pockets, 2.5 mm recession on #30. Tooth 30 may need a crown.";
        assert_eq!(sanitize(note, &[]).as_deref(), Some(note));
    }

    #[test]
    fn dates_and_years_are_removed() {
        let note = "Extraction 2024 healed well. Seen May 4, 2024 and on 3rd March for review. \
                    Recheck 2024-03-04 and 3/4/2024, bone fill good on radiograph,                     soft tissue healthy, no mobility, occlusion stable, implant site ready for planning.";
        let clean = sanitize(note, &[]).unwrap();
        for gone in ["2024", "May", "4,", "3rd", "March", "3/4"] {
            assert!(!clean.contains(gone), "{gone} survived: {clean}");
        }
        assert!(clean.contains("Extraction healed well."), "{clean}");
        assert!(clean.contains("bone fill good"), "{clean}");
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
