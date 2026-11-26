//! Settings that shape generated content: reading level and audience.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingLevel {
    Simple,
    #[default]
    Standard,
    Detailed,
}

impl fmt::Display for ReadingLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ReadingLevel::Simple => "simple",
            ReadingLevel::Standard => "standard",
            ReadingLevel::Detailed => "detailed",
        })
    }
}

/// Coarse age group. Exact ages and birth dates never leave the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgeBand {
    Child,
    Teen,
    Adult,
    OlderAdult,
}

impl AgeBand {
    pub fn from_age(age: Option<u8>) -> AgeBand {
        match age {
            Some(0..=12) => AgeBand::Child,
            Some(13..=17) => AgeBand::Teen,
            Some(65..) => AgeBand::OlderAdult,
            _ => AgeBand::Adult,
        }
    }

    /// Presentations for children are written for the parent or guardian.
    pub fn guardian_audience(self) -> bool {
        self == AgeBand::Child
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_bands() {
        assert_eq!(AgeBand::from_age(Some(8)), AgeBand::Child);
        assert_eq!(AgeBand::from_age(Some(15)), AgeBand::Teen);
        assert_eq!(AgeBand::from_age(Some(42)), AgeBand::Adult);
        assert_eq!(AgeBand::from_age(Some(81)), AgeBand::OlderAdult);
        assert_eq!(AgeBand::from_age(None), AgeBand::Adult);
        assert!(AgeBand::Child.guardian_audience());
        assert!(!AgeBand::Teen.guardian_audience());
    }
}
