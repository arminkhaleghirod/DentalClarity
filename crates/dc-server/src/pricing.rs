//! The dated price table in `prices.toml` (docs/DECISIONS.md E36).
//!
//! Prices are used where the provider does not report a cost: to size the
//! budget reservation before a call, and to cost a finished call whose reply
//! carried no cost. A call uses the newest entry for its provider and model
//! dated on or before the call's UTC date.

use serde::Deserialize;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const PRICES_TOML: &str = include_str!("../prices.toml");

/// Placeholder rates for a text model the table does not list, in US dollars
/// per million input and output tokens. They are the rates the budget used
/// before the table existed and stay as the fallback.
pub const FALLBACK_INPUT_USD_PER_MTOK: f64 = 1.0;
pub const FALLBACK_OUTPUT_USD_PER_MTOK: f64 = 4.0;
/// Placeholder speech rate for a model the table does not list, per million
/// characters: the demo voice's price, which every speech call used before.
pub const FALLBACK_SPEECH_USD_PER_MCHAR: f64 = 4.0;

#[derive(Debug, Deserialize)]
struct Table {
    #[serde(default)]
    text: Vec<TextEntry>,
    #[serde(default)]
    speech: Vec<SpeechEntry>,
}

#[derive(Debug, Deserialize)]
struct TextEntry {
    provider: String,
    model: String,
    effective: String,
    input_usd_per_mtok: f64,
    output_usd_per_mtok: f64,
    max_input_usd_per_mtok: f64,
    max_output_usd_per_mtok: f64,
    #[allow(dead_code)]
    source: String,
}

#[derive(Debug, Deserialize)]
struct SpeechEntry {
    provider: String,
    model: String,
    effective: String,
    usd_per_mchar: f64,
    #[allow(dead_code)]
    source: String,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| toml::from_str(PRICES_TOML).expect("prices.toml is valid"))
}

/// Whether a price came from the table or the placeholder rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceSource {
    Table,
    Fallback,
}

impl PriceSource {
    /// The `ai_calls.cost_source` value for a cost computed from this price.
    pub fn cost_source(self) -> &'static str {
        match self {
            PriceSource::Table => "price_table",
            PriceSource::Fallback => "fallback_rate",
        }
    }
}

/// Text prices in US dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextPrice {
    pub input: f64,
    pub output: f64,
    pub max_input: f64,
    pub max_output: f64,
    pub source: PriceSource,
}

impl TextPrice {
    /// Cost of a finished call on the preferred route.
    pub fn cost(&self, input_tokens: u32, output_tokens: u32) -> f64 {
        (f64::from(input_tokens) * self.input + f64::from(output_tokens) * self.output) / 1e6
    }

    /// Cost of a call on the most expensive route it may take.
    pub fn max_cost(&self, input_tokens: u32, output_tokens: u32) -> f64 {
        (f64::from(input_tokens) * self.max_input + f64::from(output_tokens) * self.max_output)
            / 1e6
    }
}

/// The price of `model` from `provider` on `date` (`YYYY-MM-DD`, UTC).
pub fn text_price_on(provider: &str, model: &str, date: &str) -> TextPrice {
    table()
        .text
        .iter()
        .filter(|e| e.provider == provider && e.model == model && e.effective.as_str() <= date)
        .max_by(|a, b| a.effective.cmp(&b.effective))
        .map_or(
            TextPrice {
                input: FALLBACK_INPUT_USD_PER_MTOK,
                output: FALLBACK_OUTPUT_USD_PER_MTOK,
                max_input: FALLBACK_INPUT_USD_PER_MTOK,
                max_output: FALLBACK_OUTPUT_USD_PER_MTOK,
                source: PriceSource::Fallback,
            },
            |e| TextPrice {
                input: e.input_usd_per_mtok,
                output: e.output_usd_per_mtok,
                max_input: e.max_input_usd_per_mtok,
                max_output: e.max_output_usd_per_mtok,
                source: PriceSource::Table,
            },
        )
}

/// [`text_price_on`] for today.
pub fn text_price(provider: &str, model: &str) -> TextPrice {
    text_price_on(provider, model, &today_utc())
}

/// Speech price in US dollars per million characters on `date`.
pub fn speech_price_on(provider: &str, model: &str, date: &str) -> (f64, PriceSource) {
    table()
        .speech
        .iter()
        .filter(|e| e.provider == provider && e.model == model && e.effective.as_str() <= date)
        .max_by(|a, b| a.effective.cmp(&b.effective))
        .map_or(
            (FALLBACK_SPEECH_USD_PER_MCHAR, PriceSource::Fallback),
            |e| (e.usd_per_mchar, PriceSource::Table),
        )
}

/// [`speech_price_on`] for today.
pub fn speech_price(provider: &str, model: &str) -> (f64, PriceSource) {
    speech_price_on(provider, model, &today_utc())
}

/// Today's UTC date as `YYYY-MM-DD`.
pub fn today_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    date_of(secs)
}

/// The UTC date of a Unix time, as `YYYY-MM-DD` (civil-from-days).
fn date_of(unix_secs: u64) -> String {
    let z = (unix_secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_date(s: &str) -> bool {
        s.len() == 10 && s.as_bytes()[4] == b'-' && s.as_bytes()[7] == b'-'
    }

    #[test]
    fn the_table_parses_and_every_entry_is_dated_and_bounded() {
        let t = table();
        assert!(!t.text.is_empty() && !t.speech.is_empty());
        for e in &t.text {
            assert!(is_date(&e.effective), "{}", e.effective);
            assert!(
                e.max_input_usd_per_mtok >= e.input_usd_per_mtok,
                "{}",
                e.model
            );
            assert!(
                e.max_output_usd_per_mtok >= e.output_usd_per_mtok,
                "{}",
                e.model
            );
            assert!(!e.source.is_empty());
        }
        for e in &t.speech {
            assert!(is_date(&e.effective) && e.usd_per_mchar > 0.0);
        }
        // One entry per provider, model and date.
        let mut keys: Vec<_> = t
            .text
            .iter()
            .map(|e| (&e.provider, &e.model, &e.effective))
            .collect();
        let n = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), n);
    }

    #[test]
    fn prices_apply_from_their_date() {
        let model = dc_ai::defaults::DEMO_TEXT_MODEL;
        let before = text_price_on("openrouter", model, "2026-11-27");
        assert_eq!(before.source, PriceSource::Fallback);
        let on = text_price_on("openrouter", model, "2026-11-28");
        assert_eq!(on.source, PriceSource::Table);
        assert_eq!(on.input, 0.194);
        assert_eq!(text_price_on("openrouter", model, "2027-06-01"), on);
        assert!((on.cost(1_000_000, 1_000_000) - 0.894).abs() < 1e-9);
        assert!((on.max_cost(0, 1_000_000) - 6.0).abs() < 1e-9);
        assert_eq!(
            text_price_on("fake", model, "2027-01-01").source,
            PriceSource::Fallback
        );
        let (speech, source) = speech_price_on(
            "openrouter",
            dc_ai::defaults::DEMO_SPEECH_MODEL,
            "2026-11-28",
        );
        assert_eq!((speech, source), (4.0, PriceSource::Table));
    }

    #[test]
    fn dates_are_utc_calendar_dates() {
        assert_eq!(date_of(0), "1970-01-01");
        assert_eq!(date_of(951_782_400), "2000-02-29");
        // 2026-11-28T23:59:59Z
        assert_eq!(date_of(1_795_910_399), "2026-11-28");
        assert_eq!(date_of(1_795_910_400), "2026-11-29");
        assert!(is_date(&today_utc()));
    }
}
