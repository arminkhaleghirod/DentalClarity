//! The fact sheet: the locked facts of one plan, with opaque references.
//!
//! This is the only route by which plan data reaches a prompt. It holds no
//! names, keys, dates or money, so none can leak into a model request. Money
//! stays in the snapshot and is rendered locally by `costs`.

use crate::notes;
use dc_core::content::AgeBand;
use dc_core::plan::PlanSnapshot;
use dc_core::procedure::{ProcedureCategory, ProcedureStatus};
use dc_core::{ProcedureCode, ToothRef};
use dc_knowledge::KnowledgeBase;
use dc_knowledge::entries::category_id;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcedureFact {
    /// `p1`, `p2`, … in plan order.
    pub reference: String,
    /// Index into `PlanSnapshot::procedures`.
    #[serde(skip)]
    pub index: usize,
    pub code: ProcedureCode,
    pub category: ProcedureCategory,
    /// Our plain-language name, from the code map.
    pub name: String,
    pub tooth: ToothRef,
    pub surfaces: String,
    pub phase_ref: String,
    pub status: ProcedureStatus,
    /// Sanitized linked note, when one exists and survives sanitizing.
    #[serde(skip)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhaseFact {
    /// `ph1`, `ph2`, … in treatment order.
    pub reference: String,
    pub procedure_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FactSheet {
    pub age_band: AgeBand,
    pub guardian_audience: bool,
    pub procedures: Vec<ProcedureFact>,
    pub phases: Vec<PhaseFact>,
}

impl FactSheet {
    pub fn build(snapshot: &PlanSnapshot, kb: &KnowledgeBase) -> FactSheet {
        let age_band = AgeBand::from_age(snapshot.patient.age_years);

        // Numbered phases in order, then procedures without a phase as a final group.
        let mut phase_numbers: Vec<Option<u8>> =
            snapshot.procedures.iter().map(|p| p.phase).collect();
        phase_numbers.sort_by_key(|p| p.map_or(u16::MAX, u16::from));
        phase_numbers.dedup();
        let phase_ref = |phase: Option<u8>| {
            let i = phase_numbers.iter().position(|p| *p == phase).unwrap_or(0);
            format!("ph{}", i + 1)
        };

        let mut names: Vec<&str> = vec![
            snapshot.patient.first_name.as_str(),
            snapshot.patient.last_name.as_str(),
        ];
        if let Some(provider) = &snapshot.provider_display_name {
            names.push(provider);
        }

        let procedures: Vec<ProcedureFact> = snapshot
            .procedures
            .iter()
            .enumerate()
            .map(|(index, p)| {
                let info = kb.codes.get(&p.code);
                ProcedureFact {
                    reference: format!("p{}", index + 1),
                    index,
                    code: p.code.clone(),
                    category: kb.codes.category(&p.code),
                    name: info.map_or_else(|| p.code.to_string(), |i| i.name.clone()),
                    tooth: p.tooth,
                    surfaces: p.surfaces.clone(),
                    phase_ref: phase_ref(p.phase),
                    status: p.status,
                    note: p
                        .note
                        .as_ref()
                        .and_then(|n| notes::sanitize(&n.text, &names)),
                }
            })
            .collect();

        let phases = (0..phase_numbers.len())
            .map(|i| {
                let reference = format!("ph{}", i + 1);
                PhaseFact {
                    procedure_refs: procedures
                        .iter()
                        .filter(|p| p.phase_ref == reference)
                        .map(|p| p.reference.clone())
                        .collect(),
                    reference,
                }
            })
            .collect();

        FactSheet {
            age_band,
            guardian_audience: age_band.guardian_audience(),
            procedures,
            phases,
        }
    }

    pub fn procedure(&self, reference: &str) -> Option<&ProcedureFact> {
        self.procedures.iter().find(|p| p.reference == reference)
    }

    /// Every `fact:` id a sentence may cite.
    pub fn fact_ids(&self) -> Vec<String> {
        self.procedures
            .iter()
            .map(|p| format!("fact:{}", p.reference))
            .chain(self.phases.iter().map(|p| format!("fact:{}", p.reference)))
            .collect()
    }

    /// Distinct knowledge entries this plan needs, in plan order.
    pub fn categories(&self) -> Vec<ProcedureCategory> {
        let mut seen = Vec::new();
        for p in &self.procedures {
            if !seen.contains(&p.category) {
                seen.push(p.category);
            }
        }
        seen
    }
}

/// How a procedure's location reads in a prompt.
pub fn tooth_label(tooth: ToothRef, surfaces: &str) -> String {
    let base = match tooth {
        ToothRef::Permanent(n) => format!("tooth {n}"),
        ToothRef::Primary(c) => format!("baby tooth {c}"),
        ToothRef::None => "no specific tooth".to_owned(),
        other => other.to_string(),
    };
    if surfaces.is_empty() {
        base
    } else {
        format!("{base}, surfaces {surfaces}")
    }
}

pub fn category_label(category: ProcedureCategory) -> String {
    category_id(category)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{daniel, kb};

    #[test]
    fn refs_phases_and_categories() {
        let sheet = FactSheet::build(&daniel(), &kb());
        assert_eq!(sheet.procedures.len(), 6);
        assert_eq!(sheet.procedures[0].reference, "p1");
        assert_eq!(sheet.procedures[0].category, ProcedureCategory::RootCanal);
        assert_eq!(sheet.phases.len(), 2);
        assert_eq!(sheet.phases[0].procedure_refs, vec!["p1", "p2", "p3"]);
        assert_eq!(sheet.phases[1].procedure_refs, vec!["p4", "p5", "p6"]);
        assert!(!sheet.guardian_audience);
    }

    #[test]
    fn linked_note_is_kept_and_sanitized() {
        let sheet = FactSheet::build(&daniel(), &kb());
        let note = sheet.procedures[0].note.as_deref().unwrap();
        assert!(note.contains("deep caries"));
        assert!(sheet.procedures[1].note.is_none());
    }

    #[test]
    fn notes_lose_the_patients_full_name() {
        let mut snapshot = daniel();
        snapshot.procedures[0].note.as_mut().unwrap().text =
            "Daniel Okafor reports pain on #30, deep caries into pulp, symptomatic, plan RCT then buildup and crown.".into();
        let sheet = FactSheet::build(&snapshot, &kb());
        let note = sheet.procedures[0].note.as_deref().unwrap();
        assert!(
            !note.contains("Okafor") && !note.contains("Daniel"),
            "{note}"
        );
    }

    #[test]
    fn serialized_sheet_carries_no_money_or_names() {
        let json = serde_json::to_string(&FactSheet::build(&daniel(), &kb())).unwrap();
        assert!(!json.contains("Daniel"));
        assert!(!json.contains("Patel"));
        assert!(!json.contains("115000"));
        assert!(!json.contains("10482"));
    }
}
