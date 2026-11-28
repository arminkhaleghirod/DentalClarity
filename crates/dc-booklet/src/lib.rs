//! The take-home PDF booklet (docs/UI_SPEC.md section 7), typeset in
//! process with Typst (DECISIONS E7).
//!
//! The booklet is built only from what the patient may see: the
//! [`PatientView`] of one presentation version, plus the titles of the
//! sections held from patients, which print as a neutral card. The template
//! in `templates/booklet.typ` reads one JSON file and places every value as
//! a string, so no text is evaluated as Typst markup.
//!
//! Fonts are bundled in `fonts/` (Noto Sans and Noto Sans Arabic, SIL Open
//! Font License 1.1) and no system font is ever loaded, so a booklet renders
//! the same on every machine.

mod world;

use dc_core::Money;
use dc_core::present::{PatientSectionKind, PatientView};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub use world::plain_text_of;

/// The Typst source of the booklet.
pub const TEMPLATE: &str = include_str!("../templates/booklet.typ");

#[derive(Debug, thiserror::Error)]
pub enum BookletError {
    #[error("the booklet template failed to compile: {0}")]
    Compile(String),
    #[error("the booklet could not be written as PDF: {0}")]
    Pdf(String),
    #[error("the booklet data could not be serialized: {0}")]
    Data(#[from] serde_json::Error),
}

/// Paper size of the booklet. US Letter is the default (UI_SPEC section 7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Paper {
    #[default]
    UsLetter,
    A4,
}

impl Paper {
    fn typst_name(self) -> &'static str {
        match self {
            Paper::UsLetter => "us-letter",
            Paper::A4 => "a4",
        }
    }
}

/// Facts about the version that are not part of the patient view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfo {
    pub version: u32,
    /// The plan snapshot fingerprint of this version; the booklet prints its
    /// first 8 characters.
    pub fingerprint: String,
    /// When the plan was read from the practice records, e.g. `Nov 28, 2026`.
    pub plan_as_of: String,
    /// When the booklet is printed, in the same format.
    pub prepared_on: String,
    pub paper: Paper,
}

/// One section of the version in presentation order, whether patients may
/// see it or not. Sections missing from the patient view print as held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    pub key: String,
    pub title: String,
}

/// The fixed wording of the booklet. English for now; the translation work
/// in M7 can replace these per language without touching the template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Labels {
    pub title: String,
    pub page_of: String,
    pub held: String,
    pub questions_heading: String,
    pub notes_heading: String,
    pub teeth_heading: String,
    pub drawing_caption: String,
    pub costs_title: String,
    pub cost_procedure: String,
    pub cost_phase: String,
    pub cost_fee: String,
    pub cost_insurance: String,
    pub cost_patient: String,
    pub cost_total: String,
    pub not_advice: String,
    pub demo_note: String,
}

impl Default for Labels {
    fn default() -> Self {
        let s = |v: &str| v.to_owned();
        Self {
            title: s("Your treatment plan, explained"),
            page_of: s("of"),
            held: s("Your dental team will go over this part with you."),
            questions_heading: s("Questions you could ask"),
            notes_heading: s("Your notes"),
            teeth_heading: s("Teeth in your plan:"),
            drawing_caption: s("Educational drawing, not your X-ray."),
            costs_title: s("Costs"),
            cost_procedure: s("Treatment"),
            cost_phase: s("Phase"),
            cost_fee: s("Fee"),
            cost_insurance: s("Insurance estimate"),
            cost_patient: s("Your estimated share"),
            cost_total: s("Total"),
            not_advice: s(
                "This booklet explains the plan your dentist proposed. It is not medical advice.",
            ),
            demo_note: s(
                "This booklet comes from a public demo. The patient and the plan are invented.",
            ),
        }
    }
}

/// Everything the template prints, already formatted as strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Booklet {
    pub labels: Labels,
    pub practice_name: String,
    /// `#RRGGBB`.
    pub primary: String,
    /// `#RRGGBB`.
    pub highlight: String,
    /// Primary language subtag for hyphenation and text direction, e.g. `es`.
    pub lang: String,
    pub paper: &'static str,
    pub prepared_line: String,
    pub language_line: String,
    pub provider_line: Option<String>,
    pub plan_line: String,
    pub version_line: String,
    pub teeth: Vec<String>,
    pub sections: Vec<BookletSection>,
    pub costs: Costs,
    pub cost_caveat: String,
    pub demo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BookletSection {
    pub key: String,
    /// `at_a_glance`, `journey`, `procedure` or `next_steps`.
    pub kind: &'static str,
    pub title: String,
    /// Held from patients: only the title and the held card print.
    pub held: bool,
    /// Tooth and surfaces of a procedure section, e.g. `Tooth 30`.
    pub location: Option<String>,
    /// Virtual path of the procedure's drawing, e.g. `/illustrations/crown.svg`.
    pub illustration: Option<String>,
    pub blocks: Vec<BookletBlock>,
    pub questions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BookletBlock {
    pub heading: String,
    pub sentences: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Costs {
    pub rows: Vec<CostRow>,
    pub fee: String,
    pub insurance: String,
    pub patient: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CostRow {
    pub name: String,
    pub location: String,
    pub phase: String,
    pub fee: String,
    pub insurance: String,
    pub patient: String,
}

const NOT_AVAILABLE: &str = "Not available";

fn money(value: Option<Money>) -> String {
    value.map_or_else(|| NOT_AVAILABLE.to_owned(), |m| m.to_string())
}

fn kind_of(key: &str) -> &'static str {
    match key {
        "at_a_glance" => "at_a_glance",
        "journey" => "journey",
        "next_steps" => "next_steps",
        _ => "procedure",
    }
}

fn location(tooth: &str, surfaces: &str) -> String {
    let tooth = tooth.trim();
    if tooth.is_empty() {
        return String::new();
    }
    let base = if tooth.len() <= 2 && tooth.chars().all(|c| c.is_ascii_alphanumeric()) {
        format!("Tooth {tooth}")
    } else {
        tooth.to_owned()
    };
    match surfaces.trim() {
        "" => base,
        s => format!("{base} ({s})"),
    }
}

/// The colors the template uses must be `#RRGGBB`; anything else falls back
/// to the default patient brand, so a bad setting cannot break the export.
fn hex_color(value: &str, fallback: &str) -> String {
    let ok = value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|b| b.is_ascii_hexdigit());
    if ok {
        value.to_owned()
    } else {
        fallback.to_owned()
    }
}

impl Booklet {
    /// Builds the booklet of one version from its patient view. `outline`
    /// lists every section of the version in order; a section that is not
    /// in `view` is held and prints only its title and the held card, never
    /// its text (FR-BOOK-03).
    pub fn build(view: &PatientView, outline: &[OutlineEntry], info: &VersionInfo) -> Booklet {
        let labels = Labels::default();
        let fact = |reference: &str| {
            view.procedures
                .iter()
                .find(|p| p.procedure_ref == reference)
        };
        let sections = outline
            .iter()
            .map(|entry| {
                let shown = view.sections.iter().find(|s| s.key == entry.key);
                let kind = shown.map_or_else(
                    || kind_of(&entry.key),
                    |s| match s.kind {
                        PatientSectionKind::AtAGlance => "at_a_glance",
                        PatientSectionKind::Journey => "journey",
                        PatientSectionKind::Procedure => "procedure",
                        PatientSectionKind::NextSteps => "next_steps",
                    },
                );
                let procedure = entry.key.strip_prefix("procedure:").and_then(&fact);
                let location = procedure
                    .map(|p| location(&p.tooth, &p.surfaces))
                    .filter(|l| !l.is_empty());
                match shown {
                    Some(section) => BookletSection {
                        key: entry.key.clone(),
                        kind,
                        title: section.title.clone(),
                        held: false,
                        location,
                        illustration: procedure
                            .map(|p| world::illustration_path(p.category.as_deref().unwrap_or(""))),
                        blocks: section
                            .blocks
                            .iter()
                            .filter(|b| !b.sentences.is_empty())
                            .map(|b| BookletBlock {
                                heading: b.heading.clone(),
                                sentences: b.sentences.iter().map(|s| s.text.clone()).collect(),
                            })
                            .collect(),
                        questions: section.questions.clone(),
                    },
                    None => BookletSection {
                        key: entry.key.clone(),
                        kind,
                        title: entry.title.clone(),
                        held: true,
                        location,
                        illustration: None,
                        blocks: Vec::new(),
                        questions: Vec::new(),
                    },
                }
            })
            .collect();

        let rows = view
            .procedures
            .iter()
            .map(|p| CostRow {
                name: p.name.clone(),
                location: location(&p.tooth, &p.surfaces),
                phase: p.phase.to_string(),
                fee: money(p.fee),
                insurance: money(p.insurance_estimate),
                patient: money(p.patient_estimate),
            })
            .collect();
        let short_hash: String = info.fingerprint.chars().take(8).collect();
        let lang = view
            .language
            .split('-')
            .next()
            .filter(|l| (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_alphabetic()))
            .unwrap_or("en")
            .to_ascii_lowercase();

        Booklet {
            practice_name: view.brand.name.clone(),
            primary: hex_color(&view.brand.primary, "#1B3A5C"),
            highlight: hex_color(&view.brand.highlight, "#E0735A"),
            paper: info.paper.typst_name(),
            prepared_line: if view.guardian_audience {
                format!(
                    "Prepared for the family of {} on {}",
                    view.patient_first_name, info.prepared_on
                )
            } else {
                format!(
                    "Prepared for {} on {}",
                    view.patient_first_name, info.prepared_on
                )
            },
            language_line: format!("Language: {}", view.language),
            provider_line: view
                .provider_display_name
                .as_ref()
                .map(|p| format!("Treatment plan from {p}")),
            plan_line: format!("Plan as of {}", info.plan_as_of),
            version_line: format!("Version {} · {short_hash}", info.version),
            teeth: view.affected_teeth.clone(),
            sections,
            costs: Costs {
                rows,
                fee: money(view.total_fee),
                insurance: money(view.total_insurance_estimate),
                patient: money(view.total_patient_estimate),
            },
            cost_caveat: format!(
                "These are estimates from your plan on {}. Your insurance decides the final amount, which can change.",
                info.plan_as_of
            ),
            demo: view.demo,
            lang,
            labels,
        }
    }

    /// Typesets the booklet as PDF bytes. CPU-bound; run it off the async
    /// runtime.
    pub fn render(&self) -> Result<Vec<u8>, BookletError> {
        let document = world::compile(self)?;
        let options = typst_pdf::PdfOptions {
            // No creator string, so the booklet names no software.
            creator: typst::foundations::Smart::Custom(None),
            ..Default::default()
        };
        typst_pdf::pdf(&document, &options).map_err(|errors| {
            BookletError::Pdf(
                errors
                    .iter()
                    .map(|e| e.message.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })
    }

    /// The text the booklet prints, page by page, for tests and checks.
    pub fn plain_text(&self) -> Result<String, BookletError> {
        Ok(plain_text_of(&world::compile(self)?))
    }
}

/// SHA-256 over the template and the bundled fonts' names, as hex. Part of
/// a booklet cache key, so a template change gives new booklets.
pub fn template_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(TEMPLATE.as_bytes());
    for name in world::FONT_FILES {
        hasher.update(name.as_bytes());
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests;
