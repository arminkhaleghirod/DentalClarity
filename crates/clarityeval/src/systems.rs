//! Systems under test, per-model provider routing and the run config file.
//!
//! A system is one way of producing a presentation: the full DentalClarity
//! pipeline (writer, validators, reviewer, repair) or the plain-prompt
//! baseline (one call, no knowledge passages, no gate). Each runs per model.

use anyhow::{Context as _, Result, bail};
use dc_ai::ReasoningEffort;
use dc_ai::openrouter::default_routing;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SystemKind {
    /// The app's pipeline: grounding, validators, review and repair.
    #[default]
    Pipeline,
    /// One plain-prompt call per presentation with the same facts.
    Baseline,
}

impl SystemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SystemKind::Pipeline => "pipeline",
            SystemKind::Baseline => "baseline",
        }
    }
}

/// Generation parameters, recorded per system and per call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Params {
    pub temperature: f32,
    pub max_tokens: u32,
    pub reasoning: Option<ReasoningEffort>,
    pub max_repairs: usize,
    pub parallel_sections: usize,
    /// Seed sent to the route, when it was sent (see [`SeedMode`]).
    #[serde(default)]
    pub seed: Option<u64>,
    /// Client-side timeout per call.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

impl Params {
    /// The pipeline's production parameters.
    pub fn current(reasoning: Option<ReasoningEffort>) -> Self {
        use dc_generation::pipeline;
        Params {
            temperature: pipeline::TEMPERATURE,
            max_tokens: pipeline::MAX_OUTPUT_TOKENS,
            reasoning,
            max_repairs: pipeline::MAX_REPAIRS,
            parallel_sections: pipeline::PARALLEL_SECTIONS,
            seed: None,
            timeout_s: Some(
                if reasoning.is_some() {
                    dc_ai::openrouter::REASONING_TIMEOUT
                } else {
                    dc_ai::openrouter::TIMEOUT
                }
                .as_secs(),
            ),
        }
    }
}

/// The baseline writes a whole presentation in one call, so it gets longer.
pub const BASELINE_TIMEOUT_S: u64 = 240;
pub const JUDGE_TIMEOUT_S: u64 = 120;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemSpec {
    /// `pipeline:<model>` or `baseline:<model>`.
    pub label: String,
    pub kind: SystemKind,
    pub writer_model: String,
    /// `None` for the baseline, which has no review pass.
    pub reviewer_model: Option<String>,
    pub params: Params,
    pub prompt_versions: Vec<String>,
    /// OpenRouter `provider` object sent for each model this system calls.
    pub routing: BTreeMap<String, Value>,
}

impl SystemSpec {
    pub fn label(kind: SystemKind, model: &str) -> String {
        format!("{}:{model}", kind.as_str())
    }
}

/// Provider routing for a model. The app's default model keeps its benchmarked
/// provider order; every other model is sorted by throughput with fallbacks,
/// so a comparison measures the model rather than a slow or low-bit provider.
pub fn routing_for(model: &str) -> Value {
    if model == dc_ai::defaults::DEMO_TEXT_MODEL {
        default_routing(dc_ai::defaults::DEMO_PROVIDER_ORDER)
    } else {
        default_routing(&[])
    }
}

/// When to send a sampling seed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum SeedMode {
    /// Send it unless a provider pinned first in the routing order does not
    /// accept `seed`. With `require_parameters`, sending it would silently
    /// skip that provider.
    #[default]
    Auto,
    Always,
    Never,
}

/// Snapshot of a model's OpenRouter endpoints at run time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EndpointSnapshot {
    pub fetched_at: String,
    pub error: Option<String>,
    pub endpoints: Vec<Endpoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    pub provider: String,
    pub tag: Option<String>,
    pub quantization: Option<String>,
    pub supports_seed: bool,
    pub supports_reasoning: bool,
    pub supports_response_format: bool,
}

impl EndpointSnapshot {
    /// Endpoints the default routing may use (8-bit or better, structured output).
    pub fn eligible(&self, reasoning: bool) -> usize {
        self.endpoints
            .iter()
            .filter(|e| {
                matches!(
                    e.quantization.as_deref().unwrap_or("unknown"),
                    "fp8" | "bf16" | "fp16" | "unknown"
                ) && e.supports_response_format
                    && (!reasoning || e.supports_reasoning)
            })
            .count()
    }

    fn pinned_support_seed(&self, order: &[String]) -> bool {
        order.iter().all(|slug| {
            let matching: Vec<&Endpoint> = self
                .endpoints
                .iter()
                .filter(|e| {
                    e.tag
                        .as_deref()
                        .is_some_and(|t| t == slug || t.starts_with(&format!("{slug}/")))
                })
                .collect();
            !matching.is_empty() && matching.iter().all(|e| e.supports_seed)
        })
    }
}

/// Decides whether to send `seed` for a model, and says why.
pub fn seed_decision(
    mode: SeedMode,
    seed: u64,
    routing: &Value,
    snapshot: Option<&EndpointSnapshot>,
) -> (Option<u64>, String) {
    match mode {
        SeedMode::Never => (None, "seed disabled (--seed-mode never)".into()),
        SeedMode::Always => (Some(seed), "seed always sent (--seed-mode always)".into()),
        SeedMode::Auto => {
            let order: Vec<String> = routing
                .get("order")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if order.is_empty() {
                return (
                    Some(seed),
                    "seed sent; require_parameters limits routing to providers that accept it"
                        .into(),
                );
            }
            match snapshot {
                Some(s) if s.error.is_none() && s.pinned_support_seed(&order) => {
                    (Some(seed), format!("seed sent; pinned {order:?} accept it"))
                }
                Some(s) if s.error.is_none() => (
                    None,
                    format!(
                        "seed not sent: pinned provider {order:?} does not accept seed, and \
                         require_parameters would route around it"
                    ),
                ),
                _ => (
                    None,
                    "seed not sent: endpoint list unavailable for a pinned route".into(),
                ),
            }
        }
    }
}

/// Fetches the public endpoint list for a model (no key needed).
pub async fn fetch_endpoints(model: &str) -> EndpointSnapshot {
    let fetched_at = crate::meta::rfc3339(std::time::SystemTime::now());
    let url = format!("https://openrouter.ai/api/v1/models/{model}/endpoints");
    let result: Result<Value> = async {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        let resp = client.get(&url).send().await?;
        if !resp.status().is_success() {
            bail!("HTTP {}", resp.status());
        }
        Ok(resp.json::<Value>().await?)
    }
    .await;
    match result {
        Err(e) => EndpointSnapshot {
            fetched_at,
            error: Some(format!("{e:#}")),
            endpoints: vec![],
        },
        Ok(v) => EndpointSnapshot {
            fetched_at,
            error: None,
            endpoints: parse_endpoints(&v),
        },
    }
}

pub fn parse_endpoints(v: &Value) -> Vec<Endpoint> {
    v.pointer("/data/endpoints")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|e| {
                    let params: Vec<&str> = e
                        .get("supported_parameters")
                        .and_then(Value::as_array)
                        .map(|p| p.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    Endpoint {
                        provider: e
                            .get("provider_name")
                            .and_then(Value::as_str)
                            .unwrap_or("?")
                            .to_owned(),
                        tag: e.get("tag").and_then(Value::as_str).map(str::to_owned),
                        quantization: e
                            .get("quantization")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        supports_seed: params.contains(&"seed"),
                        supports_reasoning: params.contains(&"reasoning"),
                        supports_response_format: params.contains(&"response_format")
                            || params.contains(&"structured_outputs"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `--config` file: everything a run needs, so a stranger can rerun it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    pub name: Option<String>,
    pub split: Option<String>,
    pub cases_dir: Option<String>,
    #[serde(default)]
    pub cases: Vec<String>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub baseline: bool,
    pub reviewer: Option<String>,
    pub reasoning: Option<String>,
    pub judge: Option<String>,
    pub judge_reasoning: Option<String>,
    pub seed: Option<u64>,
    pub seed_mode: Option<SeedMode>,
    pub concurrency: Option<usize>,
    pub max_cost_usd: Option<f64>,
    pub bootstrap: Option<usize>,
    #[serde(default)]
    pub systems: Vec<ConfigSystem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSystem {
    pub model: String,
    pub reviewer: Option<String>,
    /// `none`, `low`, `medium` or `high`; overrides the run-wide setting.
    pub reasoning: Option<String>,
}

impl RunConfig {
    pub fn load(path: &Path) -> Result<RunConfig> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}

pub fn parse_reasoning(s: &str) -> Result<Option<ReasoningEffort>> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "none" => None,
        "low" => Some(ReasoningEffort::Low),
        "medium" => Some(ReasoningEffort::Medium),
        "high" => Some(ReasoningEffort::High),
        other => bail!("reasoning must be none, low, medium or high, got {other:?}"),
    })
}

pub fn reasoning_str(r: Option<ReasoningEffort>) -> &'static str {
    match r {
        None => "none",
        Some(ReasoningEffort::Low) => "low",
        Some(ReasoningEffort::Medium) => "medium",
        Some(ReasoningEffort::High) => "high",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_model_keeps_its_pinned_route_and_others_sort_by_throughput() {
        let d = routing_for(dc_ai::defaults::DEMO_TEXT_MODEL);
        assert_eq!(d["order"], json!(["primeintellect"]));
        let o = routing_for("some/other-model");
        assert_eq!(o["order"], json!([]));
        assert_eq!(o["sort"], "throughput");
        assert_eq!(o["allow_fallbacks"], true);
        assert_eq!(o["require_parameters"], true);
        assert_eq!(o["data_collection"], "deny");
        assert_eq!(
            o["quantizations"],
            json!(["fp8", "bf16", "fp16", "unknown"])
        );
    }

    fn snapshot(seed_on_pinned: bool) -> EndpointSnapshot {
        let v = json!({"data": {"endpoints": [
            {"provider_name": "PrimeIntellect", "tag": "primeintellect", "quantization": "unknown",
             "supported_parameters": if seed_on_pinned { json!(["seed", "reasoning", "response_format"]) } else { json!(["reasoning", "response_format"]) }},
            {"provider_name": "Other", "tag": "other/fp4", "quantization": "fp4",
             "supported_parameters": ["seed", "reasoning", "response_format"]},
        ]}});
        EndpointSnapshot {
            fetched_at: "t".into(),
            error: None,
            endpoints: parse_endpoints(&v),
        }
    }

    #[test]
    fn auto_seed_respects_pinned_providers() {
        let pinned = routing_for(dc_ai::defaults::DEMO_TEXT_MODEL);
        let (s, why) = seed_decision(SeedMode::Auto, 7, &pinned, Some(&snapshot(false)));
        assert_eq!(s, None, "{why}");
        let (s, _) = seed_decision(SeedMode::Auto, 7, &pinned, Some(&snapshot(true)));
        assert_eq!(s, Some(7));
        let (s, _) = seed_decision(SeedMode::Auto, 7, &routing_for("x/y"), None);
        assert_eq!(s, Some(7));
        assert_eq!(
            seed_decision(SeedMode::Never, 7, &routing_for("x/y"), None).0,
            None
        );
        assert_eq!(snapshot(false).eligible(true), 1);
    }

    #[test]
    fn config_parses_systems() {
        let c: RunConfig = toml::from_str(
            r#"
name = "final"
split = "heldout"
baseline = true
reasoning = "high"
[[systems]]
model = "a/b"
[[systems]]
model = "c/d"
reviewer = "a/b"
reasoning = "none"
"#,
        )
        .unwrap();
        assert_eq!(c.systems.len(), 2);
        assert_eq!(c.systems[1].reviewer.as_deref(), Some("a/b"));
        assert!(toml::from_str::<RunConfig>("surprise = 1").is_err());
        assert_eq!(
            parse_reasoning("High").unwrap(),
            Some(ReasoningEffort::High)
        );
        assert!(parse_reasoning("max").is_err());
    }
}
