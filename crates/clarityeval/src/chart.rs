//! The ClarityEval score (docs/DECISIONS.md E22) and the static bar chart of
//! it that the README shows.
//!
//! The chart is drawn from a run's `results.json`, so it can be regenerated
//! with `clarityeval chart <run>` whenever the report is rewritten. It is
//! plain SVG with no scripts or web fonts, because GitHub shows README images
//! through `<img>`, which runs neither.

use anyhow::{Context, Result};
use serde_json::Value;
use std::fmt::Write as _;

/// Weights of the four parts of the score (E22).
pub const W_SAFE: f64 = 0.4;
pub const W_FLAG: f64 = 0.2;
pub const W_FAITH: f64 = 0.2;
pub const W_SHOWN: f64 = 0.2;

/// The four parts of the score, each a share from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parts {
    /// Presentations with no visible violation.
    pub safe: f64,
    /// Presentations with no critical judge flag.
    pub no_flag: f64,
    /// Judge faithfulness, rescaled from 1..5 to 0..1.
    pub faith: f64,
    /// Requested sections shown to the patient.
    pub shown: f64,
}

impl Parts {
    pub fn score(&self) -> f64 {
        100.0
            * (W_SAFE * self.safe
                + W_FLAG * self.no_flag
                + W_FAITH * self.faith
                + W_SHOWN * self.shown)
    }
}

fn value(v: &Value, path: &[&str]) -> Option<f64> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.get("value")?.as_f64()
}

/// The parts of one system entry of `results.json`. A system without judge
/// scores has no score.
pub fn parts(system: &Value) -> Option<Parts> {
    Some(Parts {
        safe: value(system, &["metrics", "zero_violation_rate"])?,
        no_flag: 1.0 - value(system, &["judge", "critical_per_presentation"])?,
        faith: (value(system, &["judge", "faithfulness"])? - 1.0) / 4.0,
        shown: 1.0 - value(system, &["metrics", "not_shown_rate"])?,
    })
}

/// A system as the chart draws it.
#[derive(Debug, Clone)]
pub struct Bar {
    pub name: String,
    pub lab: &'static str,
    pub pipeline: bool,
    pub score: f64,
}

/// Lab and readable name for a model id. Unknown models keep their id.
fn identify(model: &str) -> (&'static str, String) {
    const NAMES: &[(&str, &str, &str)] = &[
        (
            "deepseek/deepseek-v4.1-flash",
            "DeepSeek",
            "DeepSeek v4.1 Flash",
        ),
        ("openai/gpt-oss-120b", "OpenAI", "GPT-OSS 120b"),
        ("z-ai/glm-5.3-flash", "Z.ai", "GLM 5.3 Flash"),
        ("z-ai/glm-5.3", "Z.ai", "GLM 5.3"),
        ("openai/gpt-6-luna-pro", "OpenAI", "GPT-6 Luna Pro"),
        ("openai/gpt-6-luna", "OpenAI", "GPT-6 Luna"),
        ("tencent/hy4-flash", "Tencent", "Tencent hy4 Flash"),
        (
            "minimax/minimax-m3.1-flash",
            "MiniMax",
            "MiniMax m3.1 Flash",
        ),
        ("xiaomi/mimo-v2.6-flash", "Xiaomi", "MiMo v2.6 Flash"),
        (
            "ricursiveai/ripple-65b",
            "Ricursive AI",
            "Ripple 65b preview",
        ),
        ("anthropic/claude-opus-5.5", "Anthropic", "Claude Opus 5.5"),
        (
            "anthropic/claude-sonnet-5.5",
            "Anthropic",
            "Claude Sonnet 5.5",
        ),
        (
            "anthropic/claude-haiku-5.5",
            "Anthropic",
            "Claude Haiku 5.5",
        ),
    ];
    for (prefix, lab, name) in NAMES {
        if model.starts_with(prefix) {
            return (lab, (*name).to_owned());
        }
    }
    let lab = match model.split('/').next().unwrap_or("") {
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "deepseek" => "DeepSeek",
        "z-ai" => "Z.ai",
        "tencent" => "Tencent",
        "minimax" => "MiniMax",
        "xiaomi" => "Xiaomi",
        "ricursiveai" => "Ricursive AI",
        _ => "Other",
    };
    (lab, model.to_owned())
}

/// Every scored system in a `results.json`, highest score first.
pub fn bars(results: &Value) -> Result<Vec<Bar>> {
    let systems = results
        .get("systems")
        .and_then(Value::as_array)
        .context("results.json has no systems")?;
    let mut out = Vec::new();
    for s in systems {
        let label = s.get("label").and_then(Value::as_str).unwrap_or_default();
        let Some((kind, model)) = label.split_once(':') else {
            continue;
        };
        let Some(p) = parts(s) else { continue };
        let (lab, name) = identify(model);
        out.push(Bar {
            name,
            lab,
            pipeline: kind == "pipeline",
            score: p.score(),
        });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.name.cmp(&b.name)));
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

struct Palette {
    bg: &'static str,
    border: &'static str,
    fg: &'static str,
    muted: &'static str,
    grid: &'static str,
}

impl Theme {
    fn palette(self) -> Palette {
        match self {
            Theme::Light => Palette {
                bg: "#ffffff",
                border: "#e3e6eb",
                fg: "#16191f",
                muted: "#6b7280",
                grid: "#dfe3e8",
            },
            Theme::Dark => Palette {
                bg: "#0d1117",
                border: "#30363d",
                fg: "#e6edf3",
                muted: "#8b949e",
                grid: "#262c36",
            },
        }
    }

    fn lab(self, lab: &str) -> (&'static str, &'static str) {
        // (bar color, text color on the bar)
        match (lab, self) {
            ("Anthropic", _) => ("#c8775a", "#ffffff"),
            ("OpenAI", Theme::Light) => ("#1d1f23", "#ffffff"),
            ("OpenAI", Theme::Dark) => ("#d9dce1", "#16191f"),
            ("DeepSeek", Theme::Light) => ("#4d6bfe", "#ffffff"),
            ("DeepSeek", Theme::Dark) => ("#6f88ff", "#ffffff"),
            ("Z.ai", Theme::Light) => ("#5b5f97", "#ffffff"),
            ("Z.ai", Theme::Dark) => ("#8b8fd0", "#ffffff"),
            ("Tencent", _) => ("#3fa9f5", "#ffffff"),
            ("MiniMax", _) => ("#e5442f", "#ffffff"),
            ("Xiaomi", _) => ("#ff6a13", "#ffffff"),
            ("Ricursive AI", _) => ("#2e9e6a", "#ffffff"),
            _ => ("#8b949e", "#ffffff"),
        }
    }
}

const LAB_ORDER: &[&str] = &[
    "Anthropic",
    "OpenAI",
    "DeepSeek",
    "Z.ai",
    "Tencent",
    "MiniMax",
    "Xiaomi",
    "Ricursive AI",
    "Other",
];

fn monogram(lab: &str) -> &'static str {
    match lab {
        "Anthropic" => "A",
        "OpenAI" => "O",
        "DeepSeek" => "D",
        "Z.ai" => "Z",
        "Tencent" => "T",
        "MiniMax" => "M",
        "Xiaomi" => "Mi",
        "Ricursive AI" => "R",
        _ => "?",
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const SANS: &str = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif";
const SERIF: &str = "Georgia, 'Times New Roman', serif";

/// The chart as a standalone SVG document. `cases` is the number of
/// presentations per system, shown in the subtitle.
pub fn svg(bars: &[Bar], cases: usize, theme: Theme) -> String {
    let p = theme.palette();
    let (w, h) = (1280.0_f64, 640.0_f64);
    let (left, right, top, bottom) = (92.0, 32.0, 168.0, 168.0);
    let plot_h = h - top - bottom;
    let y = |v: f64| top + plot_h - v / 100.0 * plot_h;
    let base = y(0.0);
    let n = bars.len().max(1) as f64;
    let gw = (w - left - right) / n;
    let bw = (gw - 8.0).min(40.0);

    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img" aria-labelledby="t d">"#
    );
    let _ = write!(
        s,
        r#"<title id="t">ClarityEval score</title><desc id="d">ClarityEval score out of 100 for {} systems, highest first: {}.</desc>"#,
        bars.len(),
        esc(&bars
            .iter()
            .map(|b| format!(
                "{} {} {:.0}",
                b.name,
                if b.pipeline {
                    "with DentalClarity"
                } else {
                    "prompt only"
                },
                b.score
            ))
            .collect::<Vec<_>>()
            .join(", "))
    );
    s.push_str("<defs>");
    let mut labs: Vec<&str> = bars.iter().map(|b| b.lab).collect();
    labs.sort_by_key(|l| LAB_ORDER.iter().position(|o| o == l));
    labs.dedup();
    for lab in &labs {
        let (col, _) = theme.lab(lab);
        let id = lab.replace([' ', '.'], "");
        let _ = write!(
            s,
            r#"<pattern id="chk-{id}" width="8" height="8" patternUnits="userSpaceOnUse"><rect width="8" height="8" fill="{col}" fill-opacity="0.5"/><rect width="4" height="4" fill="{col}"/><rect x="4" y="4" width="4" height="4" fill="{col}"/></pattern>"#
        );
    }
    let _ = write!(
        s,
        r#"<pattern id="chk-legend" width="6" height="6" patternUnits="userSpaceOnUse"><rect width="6" height="6" fill="{0}" fill-opacity="0.5"/><rect width="3" height="3" fill="{0}"/><rect x="3" y="3" width="3" height="3" fill="{0}"/></pattern>"#,
        p.muted
    );
    s.push_str("</defs>");
    let _ = write!(
        s,
        r#"<rect x="0.5" y="0.5" width="{}" height="{}" rx="14" fill="{}" stroke="{}"/>"#,
        w - 1.0,
        h - 1.0,
        p.bg,
        p.border
    );
    let _ = write!(
        s,
        r#"<text x="40" y="56" font-family="{SERIF}" font-size="26" font-weight="600" fill="{}">ClarityEval score</text>"#,
        p.fg
    );
    let _ = write!(
        s,
        r#"<text font-family="{SANS}" font-size="13.5" fill="{}"><tspan x="40" y="82">Incorporates 4 measures: presentations with no unsafe or unsupported sentence, presentations with no critical judge flag,</tspan><tspan x="40" y="100">faithfulness to the plan, and sections shown to the patient · {cases} held-out cases per system · Higher is better</tspan></text>"#,
        p.muted
    );

    // Legend.
    let mut lx = 40.0;
    let ly = 132.0;
    let _ = write!(
        s,
        r#"<rect x="{lx}" y="{}" width="13" height="13" rx="2" fill="url(#chk-legend)"/><text x="{}" y="{}" font-family="{SANS}" font-size="13" fill="{}">Same model, single prompt without DentalClarity</text>"#,
        ly - 11.0,
        lx + 20.0,
        ly,
        p.muted
    );
    lx += 20.0 + 318.0;
    for lab in &labs {
        let (col, _) = theme.lab(lab);
        let _ = write!(
            s,
            r#"<rect x="{lx}" y="{}" width="13" height="13" rx="2" fill="{col}"/><text x="{}" y="{}" font-family="{SANS}" font-size="13" fill="{}">{}</text>"#,
            ly - 11.0,
            lx + 20.0,
            ly,
            p.muted,
            esc(lab)
        );
        lx += 20.0 + 9.0 * lab.len() as f64 + 22.0;
    }

    // Grid.
    for t in [0, 25, 50, 75, 100] {
        let yy = y(f64::from(t));
        let dash = if t == 0 {
            ""
        } else {
            r#" stroke-dasharray="3 4""#
        };
        let _ = write!(
            s,
            r#"<line x1="{left}" x2="{}" y1="{yy}" y2="{yy}" stroke="{}"{dash}/><text x="{}" y="{}" text-anchor="end" font-family="{SANS}" font-size="11.5" fill="{}">{t}</text>"#,
            w - right,
            p.grid,
            left - 10.0,
            yy + 4.0,
            p.muted
        );
    }

    for (i, b) in bars.iter().enumerate() {
        let (col, ink) = theme.lab(b.lab);
        let cx = left + gw * i as f64 + gw / 2.0;
        let bx = cx - bw / 2.0;
        let yt = y(b.score.clamp(0.0, 100.0));
        let fill = if b.pipeline {
            col.to_owned()
        } else {
            format!("url(#chk-{})", b.lab.replace([' ', '.'], ""))
        };
        let _ = write!(
            s,
            r#"<rect x="{bx:.1}" y="{yt:.1}" width="{bw:.1}" height="{:.1}" rx="4" fill="{fill}"/>"#,
            base - yt
        );
        let halo = if b.pipeline {
            String::new()
        } else {
            format!(r#" stroke="{col}" stroke-width="3" paint-order="stroke""#)
        };
        let _ = write!(
            s,
            r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" font-family="{SANS}" font-size="12.5" font-weight="700" fill="{ink}"{halo}>{:.0}</text>"#,
            yt + (base - yt) / 2.0 + 4.0,
            b.score
        );
        let mono = monogram(b.lab);
        let _ = write!(
            s,
            r#"<rect x="{:.1}" y="{:.1}" width="18" height="18" rx="4" fill="{col}"/><text x="{cx:.1}" y="{:.1}" text-anchor="middle" font-family="{SANS}" font-size="{}" font-weight="700" fill="{ink}">{mono}</text>"#,
            cx - 9.0,
            base + 10.0,
            base + 22.5,
            if mono.len() > 1 { 7.5 } else { 9.5 }
        );
        let _ = write!(
            s,
            r#"<text transform="translate({:.1} {:.1}) rotate(-50)" text-anchor="end" font-family="{SANS}" font-size="11.5"><tspan x="0" dy="0" fill="{}">{}</tspan><tspan x="0" dy="13" fill="{}">({})</tspan></text>"#,
            cx + 4.0,
            base + 40.0,
            p.fg,
            esc(&b.name),
            p.muted,
            if b.pipeline {
                "DentalClarity"
            } else {
                "prompt only"
            }
        );
    }
    let _ = write!(
        s,
        r#"<text x="{}" y="{}" text-anchor="end" font-family="{SERIF}" font-size="14" fill="{}"><tspan font-weight="600" fill="{}">ClarityEval</tspan> held-out v1</text>"#,
        w - 32.0,
        h - 20.0,
        p.muted,
        p.fg
    );
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn system(label: &str, zero: f64, critical: f64, faith: f64, not_shown: f64) -> Value {
        json!({
            "label": label,
            "presentations": 36,
            "metrics": { "zero_violation_rate": { "value": zero }, "not_shown_rate": { "value": not_shown } },
            "judge": { "critical_per_presentation": { "value": critical }, "faithfulness": { "value": faith } }
        })
    }

    #[test]
    fn score_follows_the_e22_weights() {
        let p = parts(&system("pipeline:x/y", 1.0, 0.0, 5.0, 0.0)).unwrap();
        assert!((p.score() - 100.0).abs() < 1e-9);
        let p = parts(&system("pipeline:x/y", 0.5, 0.5, 3.0, 0.5)).unwrap();
        assert!((p.score() - 50.0).abs() < 1e-9);
        let p = parts(&system("pipeline:x/y", 1.0, 1.0, 1.0, 1.0)).unwrap();
        assert!((p.score() - 40.0).abs() < 1e-9);
    }

    #[test]
    fn bars_are_sorted_and_named() {
        let results = json!({ "systems": [
            system("baseline:anthropic/claude-opus-5.5", 0.5, 0.2, 4.0, 0.0),
            system("pipeline:anthropic/claude-opus-5.5", 1.0, 0.1, 4.5, 0.1),
            json!({ "label": "pipeline:no/judge", "metrics": {} }),
        ]});
        let b = bars(&results).unwrap();
        assert_eq!(b.len(), 2);
        assert!(b[0].pipeline && b[0].score > b[1].score);
        assert_eq!(b[0].name, "Claude Opus 5.5");
        assert_eq!(b[0].lab, "Anthropic");
        let doc = svg(&b, 36, Theme::Dark);
        assert!(doc.starts_with("<svg") && doc.ends_with("</svg>\n"));
        assert!(doc.contains("(prompt only)") && doc.contains("(DentalClarity)"));
        assert!(!doc.contains("<script"));
    }
}
