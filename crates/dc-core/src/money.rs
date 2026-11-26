use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Add;

/// An amount of money in integer cents.
///
/// Amounts are never floats and are never computed by a language model.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Money(pub i64);

impl Money {
    pub const ZERO: Money = Money(0);

    pub fn from_cents(cents: i64) -> Self {
        Money(cents)
    }

    pub fn cents(self) -> i64 {
        self.0
    }
}

impl Add for Money {
    type Output = Money;

    fn add(self, rhs: Money) -> Money {
        Money(self.0 + rhs.0)
    }
}

impl std::iter::Sum for Money {
    fn sum<I: Iterator<Item = Money>>(iter: I) -> Money {
        iter.fold(Money::ZERO, Add::add)
    }
}

/// Formats as US dollars, e.g. `$1,369.00`. Localized formatting happens in the UI.
impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        let dollars = (abs / 100).to_string();
        let mut grouped = String::with_capacity(dollars.len() + dollars.len() / 3);
        for (i, ch) in dollars.chars().enumerate() {
            if i > 0 && (dollars.len() - i).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        write!(f, "{sign}${grouped}.{:02}", abs % 100)
    }
}

/// A cost figure that may be missing from the source system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "cents", rename_all = "snake_case")]
pub enum Amount {
    Known(Money),
    Unknown,
}

impl Amount {
    pub fn known(self) -> Option<Money> {
        match self {
            Amount::Known(m) => Some(m),
            Amount::Unknown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_with_grouping() {
        assert_eq!(Money(0).to_string(), "$0.00");
        assert_eq!(Money(13_500).to_string(), "$135.00");
        assert_eq!(Money(136_900).to_string(), "$1,369.00");
        assert_eq!(Money(123_456_789).to_string(), "$1,234,567.89");
        assert_eq!(Money(-5_005).to_string(), "-$50.05");
    }

    #[test]
    fn sums() {
        let total: Money = [Money(100), Money(250)].into_iter().sum();
        assert_eq!(total, Money(350));
    }
}
