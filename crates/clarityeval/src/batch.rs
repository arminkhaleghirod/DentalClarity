//! The OpenRouter batch route (docs/DECISIONS.md E20), for model ids ending
//! in `:batch`.
//!
//! [`BatchModel`] is a [`TextModel`], so the pipeline, baseline and judge call
//! it exactly as they call the live adapter. Requests a model has in flight
//! are gathered until none has arrived for [`QUIET`], then submitted as one
//! batch with the live adapter's request body. Because every section of every
//! presentation waits on its own call, each pipeline round (write, review,
//! repair) arrives together and goes out as one batch.
//!
//! The batch API takes only `only` and `allow_fallbacks` as provider options.
//! A batch the chosen provider refuses outright is sent again to the next
//! cheapest provider, and the provider that worked is kept for later batches.
//! Results carry tokens but no cost, so the batch's charge is shared out by
//! each request's tokens at that provider's prices.

use crate::systems::Endpoint;
use async_trait::async_trait;
use dc_ai::openrouter::{self, OpenRouter};
use dc_ai::{AiError, ProviderPolicy, TextModel, TextRequest, TextResponse};
use dc_core::Mode;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

pub const SUFFIX: &str = ":batch";
const BASE_URL: &str = "https://openrouter.ai/api/v1";
/// A batch is sent once no new request has arrived for this long.
pub const QUIET: Duration = Duration::from_secs(10);
/// Or once it has gathered this long or this many requests.
const MAX_GATHER: Duration = Duration::from_secs(90);
const MAX_REQUESTS: usize = 1000;
const POLL: Duration = Duration::from_secs(15);
/// Past the 24-hour completion window, with room for the provider's delay.
const GIVE_UP: Duration = Duration::from_secs(26 * 60 * 60);

pub fn is_batch_model(model: &str) -> bool {
    model.ends_with(SUFFIX)
}

/// Spend so far across the run, and the cap a batch may not start past.
#[derive(Clone)]
pub struct Budget {
    pub spent: Arc<Mutex<f64>>,
    pub cap: f64,
}

struct Pending {
    request: TextRequest,
    reply: oneshot::Sender<dc_ai::Result<TextResponse>>,
    queued: Instant,
}

pub struct BatchModel {
    tx: mpsc::UnboundedSender<Pending>,
}

struct Shared {
    http: reqwest::Client,
    key: String,
    model: String,
    /// Live adapter used only to build request bodies (seed included).
    builder: OpenRouter,
    /// Providers that can serve the batch variant, cheapest first.
    providers: Vec<Endpoint>,
    /// The provider that last completed a batch, used for the next ones.
    sticky: Mutex<Option<String>>,
    budget: Budget,
}

impl BatchModel {
    /// Must be called inside a Tokio runtime; it starts the gathering task.
    pub fn new(
        key: String,
        model: &str,
        seed: Option<u64>,
        omit_temperature: bool,
        endpoints: &[Endpoint],
        budget: Budget,
    ) -> anyhow::Result<Self> {
        let builder = OpenRouter::new(ProviderPolicy::for_mode(Mode::Demo), key.clone())?
            .with_seed(seed)
            .without_temperature(omit_temperature);
        let mut providers: Vec<Endpoint> = endpoints.to_vec();
        providers.sort_by(|a, b| {
            let price = |e: &Endpoint| e.completion_price.unwrap_or(f64::MAX);
            price(a).total_cmp(&price(b))
        });
        let shared = Arc::new(Shared {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()?,
            key,
            model: model.to_owned(),
            builder,
            providers,
            sticky: Mutex::new(None),
            budget,
        });
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(gather(shared, rx));
        Ok(BatchModel { tx })
    }
}

#[async_trait]
impl TextModel for BatchModel {
    fn provider(&self) -> &'static str {
        "openrouter-batch"
    }

    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Pending {
                request,
                reply,
                queued: Instant::now(),
            })
            .map_err(|_| AiError::Request("batch client stopped".into()))?;
        rx.await
            .unwrap_or_else(|_| Err(AiError::Request("batch dropped the request".into())))
    }
}

async fn gather(shared: Arc<Shared>, mut rx: mpsc::UnboundedReceiver<Pending>) {
    while let Some(first) = rx.recv().await {
        let opened = Instant::now();
        let mut pending = vec![first];
        while pending.len() < MAX_REQUESTS && opened.elapsed() < MAX_GATHER {
            match tokio::time::timeout(QUIET, rx.recv()).await {
                Ok(Some(p)) => pending.push(p),
                Ok(None) | Err(_) => break,
            }
        }
        tokio::spawn(run_batch(shared.clone(), pending));
    }
}

fn fail_all(pending: Vec<Pending>, err: impl Fn() -> AiError) {
    for p in pending {
        let _ = p.reply.send(Err(err()));
    }
}

/// The provider slug `only` takes: the endpoint tag's first segment.
fn slug(e: &Endpoint) -> Option<String> {
    e.tag
        .as_deref()
        .map(|t| t.split('/').next().unwrap_or(t).to_owned())
}

async fn run_batch(shared: Arc<Shared>, pending: Vec<Pending>) {
    let over = {
        let spent = *shared.budget.spent.lock().expect("budget lock");
        spent >= shared.budget.cap
    };
    if over {
        let cap = shared.budget.cap;
        return fail_all(pending, || {
            AiError::Limited(format!("cost cap ${cap:.2} reached"))
        });
    }
    let bodies: Vec<Value> = pending
        .iter()
        .map(|p| {
            let mut b = shared.builder.request_body(&p.request);
            if let Some(o) = b.as_object_mut() {
                o.remove("provider");
            }
            b
        })
        .collect();

    // Provider order: the one that worked last, else the router's own pick,
    // then each listed provider by price.
    let mut tries: Vec<Option<String>> = Vec::new();
    if let Some(s) = shared.sticky.lock().expect("sticky lock").clone() {
        tries.push(Some(s));
    } else {
        tries.push(None);
    }
    for e in &shared.providers {
        let s = slug(e);
        if s.is_some() && !tries.contains(&s) {
            tries.push(s);
        }
    }

    let mut last_error = String::from("no provider accepted the batch");
    let mut refused: Vec<String> = Vec::new();
    for only in tries {
        if only.as_ref().is_some_and(|o| refused.contains(o)) {
            continue;
        }
        match submit_and_wait(&shared, &bodies, only.as_deref()).await {
            Outcome::Done {
                results,
                cost,
                provider,
            } => {
                if let Some(p) = &provider {
                    *shared.sticky.lock().expect("sticky lock") = Some(provider_slug(p));
                }
                deliver(&shared, pending, results, cost, provider.as_deref());
                return;
            }
            Outcome::Refused(why, by) => {
                refused.extend(only.clone());
                refused.extend(by.as_deref().map(provider_slug));
                eprintln!(
                    "batch for {} refused by {}: {why}; trying the next provider",
                    shared.model,
                    only.as_deref().unwrap_or("the default route")
                );
                last_error = why;
            }
            Outcome::Fatal(err) => {
                let msg = err.to_string();
                let limited = matches!(err, AiError::Limited(_));
                return fail_all(pending, || {
                    if limited {
                        AiError::Limited(msg.clone())
                    } else {
                        AiError::Request(msg.clone())
                    }
                });
            }
        }
    }
    fail_all(pending, || {
        AiError::Request(format!("batch failed: {last_error}"))
    });
}

enum Outcome {
    Done {
        results: HashMap<String, Value>,
        cost: Option<f64>,
        provider: Option<String>,
    },
    /// The provider would not take the batch; another may.
    Refused(String, Option<String>),
    Fatal(AiError),
}

/// `Sail Research` as `only` names it: `sail-research`.
fn provider_slug(name: &str) -> String {
    name.to_lowercase().replace(' ', "-")
}

async fn submit_and_wait(shared: &Shared, bodies: &[Value], only: Option<&str>) -> Outcome {
    let provider = match only {
        Some(p) => json!({ "only": [p], "allow_fallbacks": false }),
        None => json!({ "allow_fallbacks": true }),
    };
    // Field order matters to the API: everything before `requests`.
    let mut submit = serde_json::Map::new();
    submit.insert("endpoint".into(), json!("/v1/chat/completions"));
    submit.insert("model".into(), json!(shared.model));
    submit.insert("provider".into(), provider);
    submit.insert("completion_window".into(), json!("24h"));
    submit.insert(
        "requests".into(),
        Value::Array(
            bodies
                .iter()
                .enumerate()
                .map(|(i, b)| json!({ "custom_id": i.to_string(), "body": b }))
                .collect(),
        ),
    );

    let mut id = None;
    let mut assigned: Option<String> = None;
    for attempt in 0..20u32 {
        let resp = shared
            .http
            .post(format!("{BASE_URL}/batches"))
            .bearer_auth(&shared.key)
            .json(&Value::Object(submit.clone()))
            .send()
            .await;
        match resp {
            Err(e) => {
                if attempt >= 4 {
                    return Outcome::Fatal(AiError::Request(e.without_url().to_string()));
                }
            }
            Ok(r) => {
                let status = r.status().as_u16();
                let text = r.text().await.unwrap_or_default();
                match status {
                    200..=299 => {
                        let v = serde_json::from_str::<Value>(&text).ok();
                        let field = |k: &str| {
                            v.as_ref()
                                .and_then(|v| v.get(k))
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        };
                        id = field("id");
                        assigned = field("provider");
                        break;
                    }
                    402 => return Outcome::Fatal(AiError::Limited(format!("HTTP 402: {text}"))),
                    429 => {}
                    _ => {
                        return Outcome::Refused(format!("HTTP {status}: {}", short(&text)), None);
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(30 * u64::from(attempt.min(4) + 1))).await;
    }
    let Some(id) = id else {
        return Outcome::Fatal(AiError::Request("batch submit kept failing".into()));
    };
    eprintln!(
        "batch {id}: {} requests for {}{}",
        bodies.len(),
        shared.model,
        only.map(|p| format!(" on {p}")).unwrap_or_default()
    );

    let started = Instant::now();
    let batch = loop {
        tokio::time::sleep(POLL).await;
        if started.elapsed() > GIVE_UP {
            return Outcome::Fatal(AiError::Request(format!("batch {id} never finished")));
        }
        let Ok(r) = shared
            .http
            .get(format!("{BASE_URL}/batches/{id}"))
            .bearer_auth(&shared.key)
            .send()
            .await
        else {
            continue;
        };
        let Ok(v) = r.json::<Value>().await else {
            continue;
        };
        match v.get("status").and_then(Value::as_str) {
            Some("completed") => break v,
            Some("failed") => {
                let completed = v
                    .pointer("/request_counts/completed")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let why = v
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("failed")
                    .to_owned();
                if completed == 0 {
                    return Outcome::Refused(why, assigned);
                }
                break v;
            }
            Some("expired") | Some("cancelled") => {
                return Outcome::Fatal(AiError::Request(format!("batch {id} {}", v["status"])));
            }
            _ => {}
        }
    };

    let mut results = HashMap::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut url = format!("{BASE_URL}/batches/{id}/results?limit=1000");
        if let Some(c) = &cursor {
            url.push_str(&format!("&cursor={c}"));
        }
        let page = match shared.http.get(&url).bearer_auth(&shared.key).send().await {
            Ok(r) => r.json::<Value>().await.ok(),
            Err(_) => None,
        };
        let Some(page) = page else {
            tokio::time::sleep(POLL).await;
            continue;
        };
        for item in page
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(cid) = item.get("custom_id").and_then(Value::as_str) {
                results.insert(cid.to_owned(), item.clone());
            }
        }
        cursor = page
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if page.get("has_more").and_then(Value::as_bool) != Some(true) || cursor.is_none() {
            break;
        }
    }
    Outcome::Done {
        results,
        cost: batch.pointer("/usage/cost").and_then(Value::as_f64),
        provider: batch
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn short(s: &str) -> String {
    s.chars().take(300).collect()
}

/// Weight of one request's tokens at `provider`'s prices (tokens alone when
/// the price is unknown), for sharing out the batch's charge.
fn weight(shared: &Shared, provider: Option<&str>, body: &Value) -> f64 {
    let usage = body.get("usage");
    let n = |k: &str| {
        usage
            .and_then(|u| u.get(k))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    let (pin, pout) = provider
        .and_then(|p| {
            shared
                .providers
                .iter()
                .find(|e| e.provider.eq_ignore_ascii_case(p))
        })
        .and_then(|e| Some((e.prompt_price?, e.completion_price?)))
        .unwrap_or((1.0, 1.0));
    n("prompt_tokens") * pin + n("completion_tokens") * pout
}

fn deliver(
    shared: &Shared,
    pending: Vec<Pending>,
    mut results: HashMap<String, Value>,
    cost: Option<f64>,
    provider: Option<&str>,
) {
    let ok_body = |item: &Value| -> Option<Value> {
        let resp = item.get("response")?;
        (resp.get("status_code").and_then(Value::as_u64) == Some(200))
            .then(|| resp.get("body").cloned())
            .flatten()
    };
    let total: f64 = results
        .values()
        .filter_map(ok_body)
        .map(|b| weight(shared, provider, &b))
        .sum();
    for (i, p) in pending.into_iter().enumerate() {
        let latency = p.queued.elapsed().as_millis() as u64;
        let reply = match results.remove(&i.to_string()) {
            None => Err(AiError::Request(
                "request missing from batch results".into(),
            )),
            Some(item) => match ok_body(&item) {
                Some(body) => {
                    let share = match (cost, total > 0.0) {
                        (Some(c), true) => Some(c * weight(shared, provider, &body) / total),
                        _ => None,
                    };
                    openrouter::parse_reply(body, &p.request.model, latency).map(|mut r| {
                        r.usage.cost_usd = share;
                        if r.route.is_none() {
                            r.route = provider.map(str::to_owned);
                        }
                        r
                    })
                }
                None => {
                    let detail = item
                        .pointer("/error/message")
                        .or_else(|| item.pointer("/response/body/error/message"))
                        .and_then(Value::as_str)
                        .unwrap_or("request failed in the batch");
                    Err(AiError::Request(short(detail)))
                }
            },
        };
        let _ = p.reply.send(reply);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(provider: &str, tag: &str, pin: f64, pout: f64) -> Endpoint {
        Endpoint {
            provider: provider.into(),
            tag: Some(tag.into()),
            quantization: None,
            supports_seed: false,
            supports_reasoning: true,
            supports_response_format: true,
            supports_temperature: true,
            prompt_price: Some(pin),
            completion_price: Some(pout),
        }
    }

    #[test]
    fn slugs_come_from_tags() {
        assert_eq!(
            slug(&endpoint("Morph", "morph/fp8", 0.0, 0.0)).as_deref(),
            Some("morph")
        );
        assert_eq!(provider_slug("Sail Research"), "sail-research");
        assert_eq!(
            slug(&endpoint("Together", "together", 0.0, 0.0)).as_deref(),
            Some("together")
        );
    }

    #[tokio::test]
    async fn shares_the_batch_cost_by_priced_tokens() {
        let model = BatchModel::new(
            "k".into(),
            "a/b:batch",
            None,
            false,
            &[endpoint("Morph", "morph/fp8", 1.0, 3.0)],
            Budget {
                spent: Arc::new(Mutex::new(0.0)),
                cap: 1.0,
            },
        )
        .unwrap();
        drop(model);
        let shared = Shared {
            http: reqwest::Client::new(),
            key: "k".into(),
            model: "a/b:batch".into(),
            builder: OpenRouter::new(ProviderPolicy::for_mode(Mode::Demo), "k".into()).unwrap(),
            providers: vec![endpoint("Morph", "morph/fp8", 1.0, 3.0)],
            sticky: Mutex::new(None),
            budget: Budget {
                spent: Arc::new(Mutex::new(0.0)),
                cap: 1.0,
            },
        };
        let body = |i: u32, o: u32| {
            json!({"model": "a/b:batch", "choices": [{"message": {"content": "{}"}}],
                   "usage": {"prompt_tokens": i, "completion_tokens": o}, "provider": "Morph"})
        };
        let item = |b: Value| json!({"response": {"status_code": 200, "body": b}});
        let mut results = HashMap::new();
        results.insert("0".to_owned(), item(body(10, 0)));
        results.insert("1".to_owned(), item(body(0, 10)));
        results.insert(
            "2".to_owned(),
            json!({"response": {"status_code": 400, "body": {"error": {"message": "bad"}}}}),
        );
        let mut rxs = Vec::new();
        let mut pending = Vec::new();
        for _ in 0..4 {
            let (tx, rx) = oneshot::channel();
            rxs.push(rx);
            pending.push(Pending {
                request: TextRequest {
                    model: "a/b:batch".into(),
                    messages: vec![],
                    response_schema: None,
                    max_tokens: 1,
                    temperature: 0.0,
                    reasoning: None,
                },
                reply: tx,
                queued: Instant::now(),
            });
        }
        deliver(&shared, pending, results, Some(0.4), Some("Morph"));
        let mut got = Vec::new();
        for rx in rxs {
            got.push(rx.await.unwrap());
        }
        let c0 = got[0].as_ref().unwrap().usage.cost_usd.unwrap();
        let c1 = got[1].as_ref().unwrap().usage.cost_usd.unwrap();
        assert!(
            (c0 - 0.1).abs() < 1e-9 && (c1 - 0.3).abs() < 1e-9,
            "{c0} {c1}"
        );
        assert_eq!(got[1].as_ref().unwrap().route.as_deref(), Some("Morph"));
        assert!(matches!(got[2], Err(AiError::Request(ref m)) if m == "bad"));
        assert!(matches!(got[3], Err(AiError::Request(_))));
    }
}
