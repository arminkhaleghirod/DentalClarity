use serde::{Deserialize, Serialize};
use std::fmt;

/// A procedure code as stored in the practice management system, e.g. `D2740`.
///
/// Only the bare code is kept. CDT descriptor text is copyrighted by the ADA
/// and is never stored in this repository; the knowledge base supplies its own
/// plain-language names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProcedureCode(String);

impl ProcedureCode {
    pub fn new(code: impl Into<String>) -> Self {
        ProcedureCode(code.into().trim().to_ascii_uppercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the code has the CDT shape (`D` followed by four digits).
    /// Practices also use custom codes, which are valid but need review.
    pub fn is_cdt_shaped(&self) -> bool {
        let b = self.0.as_bytes();
        b.len() == 5 && b[0] == b'D' && b[1..].iter().all(u8::is_ascii_digit)
    }
}

impl fmt::Display for ProcedureCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Our own procedure taxonomy. The knowledge base maps codes to categories,
/// and anything unmapped is [`ProcedureCategory::Unsupported`], which sends
/// the procedure to staff review instead of letting a model improvise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureCategory {
    Exam,
    Radiographs,
    Cleaning,
    PeriodontalScaling,
    Sealant,
    Fluoride,
    FillingComposite,
    FillingAmalgam,
    CoreBuildup,
    PostAndCore,
    Crown,
    RootCanal,
    Pulpotomy,
    Extraction,
    SurgicalExtraction,
    BoneGraft,
    Implant,
    ImplantCrown,
    Bridge,
    DentureComplete,
    DenturePartial,
    NightGuard,
    InlayOnlay,
    Veneer,
    SpaceMaintainer,
    StainlessSteelCrown,
    CrownLengthening,
    Apicoectomy,
    GumGraft,
    Whitening,
    Unsupported,
}

/// Where a procedure sits in the plan's lifecycle, as reported by the PMS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureStatus {
    Planned,
    Scheduled,
    Completed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_codes() {
        let c = ProcedureCode::new(" d2740 ");
        assert_eq!(c.as_str(), "D2740");
        assert!(c.is_cdt_shaped());
        assert!(!ProcedureCode::new("CUST1").is_cdt_shaped());
    }
}
