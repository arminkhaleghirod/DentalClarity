//! The case format (docs/CLARITYEVAL.md section 3) and its loader.
//!
//! One TOML file per case under `eval/cases/<split>/<category>/<case_id>.toml`.
//! Parsing is strict: unknown fields are rejected, so a typo cannot silently
//! drop an expectation. [`Case::to_snapshot`] turns the source facts into the
//! same `PlanSnapshot` the app builds from a practice management system.

use anyhow::{Context as _, Result, bail};
use dc_core::content::{AgeBand, ReadingLevel};
use dc_core::money::{Amount, Money};
use dc_core::plan::{
    ClinicalNote, PatientContext, PatientKey, PlanKey, PlanSnapshot, PlannedProcedure,
};
use dc_core::procedure::ProcedureStatus;
use dc_core::tooth::{Arch, Quadrant};
use dc_core::{ProcedureCode, ToothRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: &str = "1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Split {
    Dev,
    Heldout,
}

impl Split {
    pub fn as_str(self) -> &'static str {
        match self {
            Split::Dev => "dev",
            Split::Heldout => "heldout",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Knowledge,
    TreatmentPlan,
    Education,
    Grounding,
    Safety,
    Multilingual,
    StructuredOutput,
}

impl Category {
    /// The letter every case id in this category starts with (`T-001`).
    pub fn prefix(self) -> char {
        match self {
            Category::Knowledge => 'K',
            Category::TreatmentPlan => 'T',
            Category::Education => 'E',
            Category::Grounding => 'G',
            Category::Safety => 'S',
            Category::Multilingual => 'M',
            Category::StructuredOutput => 'O',
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Knowledge => "knowledge",
            Category::TreatmentPlan => "treatment_plan",
            Category::Education => "education",
            Category::Grounding => "grounding",
            Category::Safety => "safety",
            Category::Multilingual => "multilingual",
            Category::StructuredOutput => "structured_output",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
}

/// Authoring status (section 4, step 4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Draft,
    Checked,
    Frozen,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub schema_version: String,
    pub case_id: String,
    pub split: Split,
    pub category: Category,
    pub difficulty: Difficulty,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Base plan id. All cases from one plan share a split.
    pub plan_ref: String,
    /// Pseudonymous author code, never a name.
    pub author: String,
    pub canary: String,
    pub created: String,
    #[serde(default)]
    pub status: Status,
    pub source_facts: SourceFacts,
    pub task: Task,
    #[serde(default)]
    pub expected: Expected,
    #[serde(default)]
    pub safety: Safety,
    #[serde(default)]
    pub rubric: Rubric,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFacts {
    pub age_band: AgeBand,
    pub language: String,
    pub reading_level: ReadingLevel,
    /// Synthetic display values the app fills in locally. They never reach a
    /// prompt, but the note sanitizer uses them to strip names.
    #[serde(default)]
    pub local: LocalContext,
    pub procedures: Vec<SourceProcedure>,
    #[serde(default)]
    pub references: Vec<Reference>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalContext {
    pub patient_first_name: String,
    #[serde(default)]
    pub patient_last_name: String,
    pub provider_display_name: Option<String>,
}

impl Default for LocalContext {
    fn default() -> Self {
        LocalContext {
            patient_first_name: "Sam".into(),
            patient_last_name: String::new(),
            provider_display_name: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProcedure {
    /// `p1`, `p2`, ... in plan order; must match the pipeline's opaque refs.
    pub id: String,
    /// Bare code only (`D2740`) or a practice custom code.
    pub code: String,
    /// Our own plain-language name. For mapped codes it must equal `knowledge/codes.toml`.
    pub name: String,
    /// `"19"`, `"K"`, `"quadrant:LL"`, `"arch:upper"`, or `""` for none.
    #[serde(default)]
    pub tooth: String,
    #[serde(default)]
    pub surfaces: String,
    pub phase: Option<u8>,
    /// `"unknown"` when the estimate is missing; otherwise use `patient_estimate_cents`.
    pub patient_estimate: Option<String>,
    pub patient_estimate_cents: Option<i64>,
    pub fee_cents: Option<i64>,
    pub insurance_estimate_cents: Option<i64>,
    /// A clinical note linked to this procedure (synthetic).
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    /// `kb:<entry_id>`; covers every passage of that entry.
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// The full pipeline: every section of a presentation.
    Presentation,
    PlanSummary,
    Journey,
    ProcedureExplanation,
    NextSteps,
    KnowledgeMcq,
    KnowledgeShort,
    QaAnswer,
    Translate,
    StructuredExtract,
}

impl TaskKind {
    /// Whether `run` can execute this kind with today's pipeline.
    pub fn runnable(self) -> bool {
        matches!(
            self,
            TaskKind::Presentation
                | TaskKind::PlanSummary
                | TaskKind::Journey
                | TaskKind::ProcedureExplanation
                | TaskKind::NextSteps
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub kind: TaskKind,
    /// Procedure ref for `procedure_explanation`.
    pub target: Option<String>,
    /// Patient question for `qa_answer`.
    pub question: Option<String>,
    /// Informational; the runner records the real prompt versions it used.
    pub prompt_template: Option<String>,
    pub output_schema: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Indication {
    /// No usable note: the reason must use general "common reasons" framing.
    Absent,
    /// A note states the reason: `documented` with a `note:` cite is expected.
    Documented,
    /// Either kind is acceptable as long as citation rules hold.
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    Patient,
    Guardian,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedState {
    Passed,
    NeedsReview,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeldSection {
    /// Section key, e.g. `procedure:p2`.
    pub key: String,
    pub state: ExpectedState,
    /// No model call may be made for it (unsupported procedures).
    #[serde(default)]
    pub no_model_call: bool,
}

/// Property-based expectations. There is no gold text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    /// `fact:p1`, `fact:ph2` or `code:D4341`: must be cited somewhere in the output.
    #[serde(default)]
    pub must_reference_facts: Vec<String>,
    /// The target procedure's text names no tooth at all.
    #[serde(default)]
    pub must_not_state_tooth: bool,
    /// Expected `why` kind for the target procedure (or every procedure when no target).
    pub indication: Option<Indication>,
    #[serde(default)]
    pub must_cite_any_of: Vec<String>,
    /// `"source_facts.references"`: kb cites must stay within the listed references.
    pub forbidden_citations_outside: Option<String>,
    pub reading_level_target: Option<ReadingLevel>,
    pub max_words: Option<usize>,
    pub audience: Option<Audience>,
    /// Sections that must end in a given state.
    #[serde(default)]
    pub held_sections: Vec<HeldSection>,
    /// Case-insensitive phrases that must not appear in generated text.
    #[serde(default)]
    pub must_not_contain: Vec<String>,
    /// Identifiers planted in notes that must never reach generated text.
    #[serde(default)]
    pub forbidden_strings: Vec<String>,
    /// The case plants instructions in source text; score injection resistance.
    #[serde(default)]
    pub injection: bool,
    /// Q&A label (section 3); parsed now, scored once Q&A exists in the pipeline.
    pub response_class: Option<String>,
    #[serde(default)]
    pub must_not: Vec<String>,
    #[serde(default)]
    pub must: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Safety {
    #[serde(default)]
    pub critical_if: Vec<String>,
    #[serde(default)]
    pub required_behaviours: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rubric {
    #[serde(default)]
    pub dimensions: Vec<String>,
    #[serde(default)]
    pub notes_for_judge: String,
}

/// A case plus where it came from.
#[derive(Debug, Clone)]
pub struct LoadedCase {
    pub case: Case,
    pub path: PathBuf,
    /// sha256 of the file bytes, lowercase hex.
    pub sha256: String,
}

impl Case {
    pub fn from_toml(source: &str) -> Result<Case> {
        Ok(toml::from_str(source)?)
    }

    /// Representative age for the band; only the band reaches a prompt.
    fn age_years(&self) -> u8 {
        match self.source_facts.age_band {
            AgeBand::Child => 8,
            AgeBand::Teen => 15,
            AgeBand::Adult => 40,
            AgeBand::OlderAdult => 70,
        }
    }

    pub fn to_snapshot(&self) -> Result<PlanSnapshot> {
        let facts = &self.source_facts;
        let procedures = facts
            .procedures
            .iter()
            .map(|p| {
                let patient_estimate = match (&p.patient_estimate, p.patient_estimate_cents) {
                    (Some(word), None) if word == "unknown" => Amount::Unknown,
                    (None, Some(cents)) => Amount::Known(Money(cents)),
                    (None, None) => Amount::Unknown,
                    _ => bail!(
                        "{}: use patient_estimate = \"unknown\" or patient_estimate_cents, not both",
                        p.id
                    ),
                };
                let known = |c: Option<i64>| c.map_or(Amount::Unknown, |c| Amount::Known(Money(c)));
                Ok(PlannedProcedure {
                    source_id: format!("{}-{}", self.plan_ref, p.id),
                    code: ProcedureCode::new(&p.code),
                    // Staff-only field; our name keeps descriptor text out.
                    pms_description: p.name.clone(),
                    tooth: parse_tooth(&p.tooth).with_context(|| format!("{}: tooth", p.id))?,
                    surfaces: p.surfaces.clone(),
                    phase: p.phase,
                    status: ProcedureStatus::Planned,
                    fee: known(p.fee_cents),
                    insurance_estimate: known(p.insurance_estimate_cents),
                    patient_estimate,
                    note: p.note.as_ref().map(|text| ClinicalNote {
                        source_id: format!("{}-{}-note", self.plan_ref, p.id),
                        text: text.clone(),
                    }),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(PlanSnapshot {
            plan: PlanKey(self.plan_ref.clone()),
            patient: PatientContext {
                key: PatientKey(format!("eval-{}", self.plan_ref)),
                first_name: facts.local.patient_first_name.clone(),
                last_name: facts.local.patient_last_name.clone(),
                age_years: Some(self.age_years()),
                preferred_language: Some(facts.language.clone()),
            },
            plan_name: "Eval plan".into(),
            provider_display_name: facts.local.provider_display_name.clone(),
            procedures,
        })
    }

    /// The procedure this task focuses on, if any.
    pub fn target(&self) -> Option<&SourceProcedure> {
        let target = self.task.target.as_deref()?;
        self.source_facts.procedures.iter().find(|p| p.id == target)
    }
}

pub fn parse_tooth(s: &str) -> Result<ToothRef> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("none") {
        return Ok(ToothRef::None);
    }
    if let Some(q) = t.strip_prefix("quadrant:") {
        return Ok(ToothRef::Quadrant(match q {
            "UR" => Quadrant::UpperRight,
            "UL" => Quadrant::UpperLeft,
            "LL" => Quadrant::LowerLeft,
            "LR" => Quadrant::LowerRight,
            _ => bail!("quadrant must be UR, UL, LL or LR, got {q:?}"),
        }));
    }
    if let Some(a) = t.strip_prefix("arch:") {
        return Ok(ToothRef::Arch(match a {
            "upper" => Arch::Upper,
            "lower" => Arch::Lower,
            _ => bail!("arch must be upper or lower, got {a:?}"),
        }));
    }
    Ok(t.parse::<ToothRef>()?)
}

/// Every `*.toml` file below `dir`, sorted by path.
pub fn case_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).with_context(|| format!("reading {}", d.display()))? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "toml") {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

pub fn load_file(path: &Path) -> Result<LoadedCase> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let source = String::from_utf8(bytes.clone())
        .with_context(|| format!("{} is not UTF-8", path.display()))?;
    let case = Case::from_toml(&source).with_context(|| format!("parsing {}", path.display()))?;
    Ok(LoadedCase {
        case,
        path: path.to_path_buf(),
        sha256: hex::encode(Sha256::digest(&bytes)),
    })
}

/// Where a split's cases live. Held-out cases sit under `eval/heldout/`,
/// which is git-ignored, so they cannot be committed by accident (C3).
pub fn split_dir(repo_root: &Path, split: Split) -> PathBuf {
    match split {
        Split::Dev => repo_root.join("eval/cases/dev"),
        Split::Heldout => repo_root.join("eval/heldout/cases"),
    }
}

/// Loads every case below `dir`. Parse errors are returned per file so
/// `validate` can report all of them at once.
pub fn load_dir(dir: &Path) -> Result<Vec<Result<LoadedCase>>> {
    Ok(case_files(dir)?.iter().map(|p| load_file(p)).collect())
}

/// Simple wildcard match (`*` and `?`) used by `run --cases`.
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi].eq_ignore_ascii_case(&t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
pub(crate) mod tests_support {
    pub const MINIMAL: &str = r#"
schema_version = "1"
case_id = "T-900"
split = "dev"
category = "treatment_plan"
difficulty = "easy"
plan_ref = "plan-test-1"
author = "author-A"
canary = "clarityeval-canary-test"
created = "2026-11-25"

[source_facts]
age_band = "adult"
language = "en"
reading_level = "standard"

  [[source_facts.procedures]]
  id = "p1"
  code = "D3330"
  name = "Root canal"
  tooth = "19"
  phase = 1
  patient_estimate = "unknown"
  note = "Lingering pain to cold on #19."

  [[source_facts.procedures]]
  id = "p2"
  code = "D4341"
  name = "Deep cleaning"
  tooth = "quadrant:LL"
  phase = 2
  patient_estimate_cents = 41500

[[source_facts.references]]
id = "kb:root_canal"
title = "Root canal"

[task]
kind = "presentation"
"#;
}

#[cfg(test)]
mod tests {
    use super::tests_support::MINIMAL;
    use super::*;

    #[test]
    fn parses_and_builds_snapshot() {
        let case = Case::from_toml(MINIMAL).unwrap();
        let snap = case.to_snapshot().unwrap();
        assert_eq!(snap.procedures.len(), 2);
        assert_eq!(snap.procedures[0].tooth, ToothRef::Permanent(19));
        assert_eq!(
            snap.procedures[1].tooth,
            ToothRef::Quadrant(Quadrant::LowerLeft)
        );
        assert_eq!(snap.procedures[0].patient_estimate, Amount::Unknown);
        assert_eq!(
            snap.procedures[1].patient_estimate,
            Amount::Known(Money(41500))
        );
        assert!(snap.procedures[0].note.is_some());
        assert_eq!(AgeBand::from_age(snap.patient.age_years), AgeBand::Adult);
    }

    #[test]
    fn rejects_unknown_fields() {
        let bad = MINIMAL.replace("[task]", "[task]\nsurprise = 1");
        assert!(Case::from_toml(&bad).is_err());
    }

    #[test]
    fn rejects_both_estimate_forms() {
        let bad = MINIMAL.replace(
            "patient_estimate_cents = 41500",
            "patient_estimate_cents = 41500\n  patient_estimate = \"unknown\"",
        );
        assert!(Case::from_toml(&bad).unwrap().to_snapshot().is_err());
    }

    #[test]
    fn tooth_forms() {
        assert_eq!(parse_tooth("").unwrap(), ToothRef::None);
        assert_eq!(parse_tooth("K").unwrap(), ToothRef::Primary('K'));
        assert_eq!(
            parse_tooth("arch:lower").unwrap(),
            ToothRef::Arch(Arch::Lower)
        );
        assert!(parse_tooth("quadrant:XX").is_err());
        assert!(parse_tooth("33").is_err());
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("T-*", "T-001"));
        assert!(wildcard("*001", "G-001"));
        assert!(wildcard("S-00?", "S-003"));
        assert!(!wildcard("T-*", "G-001"));
        assert!(wildcard("t-001", "T-001"));
    }
}
