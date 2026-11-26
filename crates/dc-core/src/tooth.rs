use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Where a planned procedure is located in the mouth.
///
/// Teeth use the Universal Numbering System, which Open Dental uses for US
/// practices: permanent teeth 1 to 32, primary teeth A to T. A procedure with
/// no tooth or area in the source data is [`ToothRef::None`]; the app never
/// guesses a location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ToothRef {
    Permanent(u8),
    Primary(char),
    Quadrant(Quadrant),
    Arch(Arch),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Quadrant {
    UpperRight,
    UpperLeft,
    LowerLeft,
    LowerRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Arch {
    Upper,
    Lower,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid tooth reference {0:?}")]
pub struct ParseToothError(String);

impl ToothRef {
    /// Whether this is a single tooth that can be drawn on the chart.
    pub fn is_single_tooth(self) -> bool {
        matches!(self, ToothRef::Permanent(_) | ToothRef::Primary(_))
    }

    /// The arch a single tooth belongs to.
    pub fn arch(self) -> Option<Arch> {
        match self {
            ToothRef::Permanent(n) if (1..=16).contains(&n) => Some(Arch::Upper),
            ToothRef::Permanent(n) if (17..=32).contains(&n) => Some(Arch::Lower),
            ToothRef::Primary(c) if ('A'..='J').contains(&c) => Some(Arch::Upper),
            ToothRef::Primary(c) if ('K'..='T').contains(&c) => Some(Arch::Lower),
            ToothRef::Arch(a) => Some(a),
            _ => None,
        }
    }
}

impl FromStr for ToothRef {
    type Err = ParseToothError;

    /// Parses a Universal tooth number (`"30"`, `"K"`). An empty string means
    /// no tooth. Quadrants and arches are stored in other PMS fields, so the
    /// connector builds those variants itself; this parser never returns them.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        let err = || ParseToothError(s.to_owned());
        if t.is_empty() {
            return Ok(ToothRef::None);
        }
        if let Ok(n) = t.parse::<u8>() {
            return if (1..=32).contains(&n) {
                Ok(ToothRef::Permanent(n))
            } else {
                Err(err())
            };
        }
        let mut chars = t.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.to_ascii_uppercase().is_ascii_uppercase() => {
                let c = c.to_ascii_uppercase();
                if ('A'..='T').contains(&c) {
                    Ok(ToothRef::Primary(c))
                } else {
                    Err(err())
                }
            }
            _ => Err(err()),
        }
    }
}

impl fmt::Display for ToothRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToothRef::Permanent(n) => write!(f, "{n}"),
            ToothRef::Primary(c) => write!(f, "{c}"),
            ToothRef::Quadrant(q) => f.write_str(match q {
                Quadrant::UpperRight => "upper right",
                Quadrant::UpperLeft => "upper left",
                Quadrant::LowerLeft => "lower left",
                Quadrant::LowerRight => "lower right",
            }),
            ToothRef::Arch(Arch::Upper) => f.write_str("upper arch"),
            ToothRef::Arch(Arch::Lower) => f.write_str("lower arch"),
            ToothRef::None => f.write_str("no specific tooth"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_universal_numbers() {
        assert_eq!("30".parse(), Ok(ToothRef::Permanent(30)));
        assert_eq!("k".parse(), Ok(ToothRef::Primary('K')));
        assert_eq!("".parse(), Ok(ToothRef::None));
        assert_eq!("L".parse(), Ok(ToothRef::Primary('L')));
        assert!("33".parse::<ToothRef>().is_err());
        assert!("0".parse::<ToothRef>().is_err());
        assert!("UR".parse::<ToothRef>().is_err());
        assert!("Z".parse::<ToothRef>().is_err());
    }

    #[test]
    fn arch_of_tooth() {
        assert_eq!(ToothRef::Permanent(14).arch(), Some(Arch::Upper));
        assert_eq!(ToothRef::Permanent(19).arch(), Some(Arch::Lower));
        assert_eq!(ToothRef::Primary('T').arch(), Some(Arch::Lower));
        assert_eq!(ToothRef::None.arch(), None);
    }
}
