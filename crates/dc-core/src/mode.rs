use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Deployment mode, fixed at startup.
///
/// The mode decides which PMS connectors and AI providers may be constructed.
/// See `docs/DECISIONS.md` (D1, D2) for the reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Public demo: synthetic data only, OpenRouter for all AI features.
    Demo,
    /// Practice install: Open Dental data, AWS Bedrock and Polly only.
    Clinical,
}

impl Mode {
    /// Whether leaving Present mode requires the staff PIN.
    pub fn present_exit_requires_pin(self) -> bool {
        matches!(self, Mode::Clinical)
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::Demo => "demo",
            Mode::Clinical => "clinical",
        })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown mode {0:?}; expected \"demo\" or \"clinical\"")]
pub struct ParseModeError(String);

impl FromStr for Mode {
    type Err = ParseModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "demo" => Ok(Mode::Demo),
            "clinical" => Ok(Mode::Clinical),
            other => Err(ParseModeError(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes_case_insensitively() {
        assert_eq!("Demo".parse::<Mode>().unwrap(), Mode::Demo);
        assert_eq!(" clinical ".parse::<Mode>().unwrap(), Mode::Clinical);
        assert!("prod".parse::<Mode>().is_err());
    }

    #[test]
    fn only_clinical_requires_pin() {
        assert!(Mode::Clinical.present_exit_requires_pin());
        assert!(!Mode::Demo.present_exit_requires_pin());
    }
}
