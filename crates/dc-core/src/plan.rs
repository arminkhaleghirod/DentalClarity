use crate::money::{Amount, Money};
use crate::procedure::{ProcedureCode, ProcedureStatus};
use crate::tooth::ToothRef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable identifier of a patient in the source PMS. Local only; never sent to a model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PatientKey(pub String);

/// Stable identifier of a treatment plan in the source PMS.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlanKey(pub String);

/// Patient details used to personalize a presentation on this server.
/// These fields are substituted locally and are not part of model prompts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatientContext {
    pub key: PatientKey,
    pub first_name: String,
    /// Used only to strip the name from clinical notes before they reach a model.
    #[serde(default)]
    pub last_name: String,
    pub age_years: Option<u8>,
    /// BCP 47 tag of the language recorded in the PMS, if any.
    pub preferred_language: Option<String>,
}

/// A clinical note linked to a planned procedure in the PMS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClinicalNote {
    pub source_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedProcedure {
    /// Identifier of the procedure row in the PMS.
    pub source_id: String,
    pub code: ProcedureCode,
    /// The practice's own description from its PMS, shown to staff only.
    pub pms_description: String,
    pub tooth: ToothRef,
    /// Surfaces as recorded, e.g. `"MO"`. Empty when not applicable.
    pub surfaces: String,
    /// Phase or priority number from the plan. `None` when the plan has no phases.
    pub phase: Option<u8>,
    pub status: ProcedureStatus,
    pub fee: Amount,
    pub insurance_estimate: Amount,
    pub patient_estimate: Amount,
    pub note: Option<ClinicalNote>,
}

/// An immutable, normalized copy of one treatment plan at one point in time.
///
/// Every fact a presentation shows about procedures, teeth, phases and costs
/// comes from a snapshot. A new snapshot with a different fingerprint means
/// the source plan changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSnapshot {
    pub plan: PlanKey,
    pub patient: PatientContext,
    pub plan_name: String,
    pub provider_display_name: Option<String>,
    pub procedures: Vec<PlannedProcedure>,
}

impl PlanSnapshot {
    /// Teeth that have at least one planned procedure, in chart order.
    pub fn affected_teeth(&self) -> Vec<ToothRef> {
        let mut teeth: Vec<ToothRef> = self
            .procedures
            .iter()
            .map(|p| p.tooth)
            .filter(|t| t.is_single_tooth())
            .collect();
        teeth.sort_by_key(|t| match t {
            ToothRef::Permanent(n) => (0, *n as u32),
            ToothRef::Primary(c) => (1, *c as u32),
            _ => (2, 0),
        });
        teeth.dedup();
        teeth
    }

    pub fn total_fee(&self) -> Option<Money> {
        self.procedures.iter().map(|p| p.fee.known()).sum()
    }

    pub fn total_patient_estimate(&self) -> Option<Money> {
        self.procedures
            .iter()
            .map(|p| p.patient_estimate.known())
            .sum()
    }

    /// SHA-256 over the canonical JSON form of the snapshot, as lowercase hex.
    ///
    /// `serde_json` writes struct fields in declaration order, so the output is
    /// deterministic for a given snapshot.
    pub fn fingerprint(&self) -> String {
        let json = serde_json::to_vec(self).expect("snapshot serializes");
        hex::encode(Sha256::digest(&json))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> PlanSnapshot {
        let proc = |id: &str, code: &str, tooth: ToothRef, fee: i64, pt: i64| PlannedProcedure {
            source_id: id.into(),
            code: ProcedureCode::new(code),
            pms_description: String::new(),
            tooth,
            surfaces: String::new(),
            phase: Some(1),
            status: ProcedureStatus::Planned,
            fee: Amount::Known(Money(fee)),
            insurance_estimate: Amount::Known(Money(fee - pt)),
            patient_estimate: Amount::Known(Money(pt)),
            note: None,
        };
        PlanSnapshot {
            plan: PlanKey("p1".into()),
            patient: PatientContext {
                key: PatientKey("1".into()),
                first_name: "Test".into(),
                last_name: "Patient".into(),
                age_years: Some(40),
                preferred_language: None,
            },
            plan_name: "Active".into(),
            provider_display_name: None,
            procedures: vec![
                proc("a", "D2740", ToothRef::Permanent(30), 138_000, 69_000),
                proc("b", "D2392", ToothRef::Permanent(14), 26_500, 5_300),
                proc("c", "D1110", ToothRef::None, 13_500, 0),
                proc("d", "D2950", ToothRef::Permanent(30), 31_000, 12_400),
            ],
        }
    }

    #[test]
    fn affected_teeth_are_sorted_and_unique() {
        assert_eq!(
            snapshot().affected_teeth(),
            vec![ToothRef::Permanent(14), ToothRef::Permanent(30)]
        );
    }

    #[test]
    fn totals() {
        let s = snapshot();
        assert_eq!(s.total_fee(), Some(Money(209_000)));
        assert_eq!(s.total_patient_estimate(), Some(Money(86_700)));
    }

    #[test]
    fn unknown_amount_makes_total_unknown() {
        let mut s = snapshot();
        s.procedures[0].patient_estimate = Amount::Unknown;
        assert_eq!(s.total_patient_estimate(), None);
    }

    #[test]
    fn fingerprint_is_stable_and_change_sensitive() {
        let a = snapshot();
        assert_eq!(a.fingerprint(), snapshot().fingerprint());
        let mut b = snapshot();
        b.procedures[1].fee = Amount::Known(Money(27_000));
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
