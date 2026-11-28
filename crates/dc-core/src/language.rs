//! Languages patient content can be shown in (docs/DECISIONS.md E32).
//!
//! Content is written and checked in English. Every other language is
//! derived from the approved English text. Only languages with ClarityEval
//! results count as tested; the others are labelled as not reviewed and are
//! never described as validated (DECISIONS P9).

/// The language content is written in before any variant is derived.
pub const SOURCE_LANGUAGE: &str = "en";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    /// BCP 47 tag, e.g. `es`.
    pub tag: &'static str,
    /// The language's own name, e.g. `Español`.
    pub native: &'static str,
    /// English name, used in prompts and staff screens.
    pub english: &'static str,
    /// Has ClarityEval results. Untested languages show "Not reviewed".
    pub tested: bool,
}

impl Language {
    /// "Español · Spanish", or the English name alone when both are the same.
    pub fn label(&self) -> String {
        if self.native == self.english {
            self.english.to_owned()
        } else {
            format!("{} · {}", self.native, self.english)
        }
    }
}

const fn lang(
    tag: &'static str,
    native: &'static str,
    english: &'static str,
    tested: bool,
) -> Language {
    Language {
        tag,
        native,
        english,
        tested,
    }
}

/// Languages offered for presentations and in Present mode, in menu order.
pub const LANGUAGES: [Language; 10] = [
    lang("en", "English", "English", true),
    lang("es", "Español", "Spanish", false),
    lang("vi", "Tiếng Việt", "Vietnamese", false),
    lang("zh", "中文", "Chinese", false),
    lang("tl", "Tagalog", "Tagalog", false),
    lang("ar", "العربية", "Arabic", false),
    lang("fr", "Français", "French", false),
    lang("ko", "한국어", "Korean", false),
    lang("ru", "Русский", "Russian", false),
    lang("pt", "Português", "Portuguese", false),
];

/// The supported language for a tag, matched on its primary subtag and
/// ignoring case, so `es-MX` finds Spanish.
pub fn find(tag: &str) -> Option<&'static Language> {
    let base = tag.split('-').next().unwrap_or(tag).to_ascii_lowercase();
    LANGUAGES.iter().find(|l| l.tag == base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_languages_by_primary_subtag() {
        assert_eq!(find("es-MX").map(|l| l.tag), Some("es"));
        assert_eq!(find("ZH").map(|l| l.english), Some("Chinese"));
        assert!(find("xx").is_none());
        assert_eq!(find(SOURCE_LANGUAGE).map(|l| l.tested), Some(true));
        assert_eq!(LANGUAGES.iter().filter(|l| l.tested).count(), 1);
    }

    #[test]
    fn labels_show_both_names() {
        assert_eq!(find("es").unwrap().label(), "Español · Spanish");
        assert_eq!(find("tl").unwrap().label(), "Tagalog");
    }
}
