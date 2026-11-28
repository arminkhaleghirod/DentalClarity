//! Model setup for the generation pipeline and the demo budget guard.
//!
//! Demo mode builds OpenRouter models when `OPENROUTER_API_KEY` is set.
//! Clinical mode builds Bedrock models in [`crate::clinical`]; without them,
//! creating a presentation there is refused.
//!
//! Every model call goes through [`BudgetedModel`], which reserves the call's
//! worst-case cost before it runs and records the actual cost afterwards in
//! `ai_usage` for the current UTC month (docs/AI_PIPELINE.md, demo budget guard).
//! Prices come from the dated table in `prices.toml` ([`crate::pricing`]),
//! and every call that reaches the provider writes an `ai_calls` row.

use anyhow::Result;
use async_trait::async_trait;
use dc_ai::openrouter::OpenRouter;
use dc_ai::{AiError, ProviderPolicy, TextModel, TextRequest, TextResponse, Usage, defaults};
use dc_core::Mode;
use dc_generation::Models;
use dc_generation::variant::VariantModels;
use sqlx::SqlitePool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Share of the monthly cap at which new generations stop. Calls already
/// running may continue up to the full cap.
pub const SOFT_STOP_SHARE: f64 = 0.8;
/// Share of the monthly cap that narration may spend. Together with the soft
/// stop this keeps room for generation after narration's share is used up.
pub const SPEECH_SHARE: f64 = 0.2;

/// Cost of a call at the placeholder rates for models the price table does
/// not list ([`crate::pricing::FALLBACK_INPUT_USD_PER_MTOK`]).
pub fn estimate_cost(input_tokens: u32, output_tokens: u32) -> f64 {
    (f64::from(input_tokens) * crate::pricing::FALLBACK_INPUT_USD_PER_MTOK
        + f64::from(output_tokens) * crate::pricing::FALLBACK_OUTPUT_USD_PER_MTOK)
        / 1_000_000.0
}

/// Cost to record for a usage total of `model` from `provider`: the
/// provider's figure when it reports one, otherwise the table price.
pub fn usage_cost(usage: &Usage, provider: &str, model: &str) -> f64 {
    usage.cost_usd.unwrap_or_else(|| {
        crate::pricing::text_price(provider, model).cost(usage.input_tokens, usage.output_tokens)
    })
}

/// Monthly spend tracking and the cap check.
pub struct Budget {
    db: SqlitePool,
    /// `None` means no cap (clinical mode, once it has a provider).
    cap_usd: Option<f64>,
    /// Reservations for calls in flight. Kept in memory: a restart drops them,
    /// which also releases reservations of calls that never finished.
    reserved: Mutex<f64>,
    /// The part of `reserved` held by speech calls.
    speech_reserved: Mutex<f64>,
}

impl Budget {
    pub fn new(db: SqlitePool, cap_usd: Option<f64>) -> Self {
        Self {
            db,
            cap_usd,
            reserved: Mutex::new(0.0),
            speech_reserved: Mutex::new(0.0),
        }
    }

    pub fn cap_usd(&self) -> Option<f64> {
        self.cap_usd
    }

    /// Spend recorded for the current UTC month.
    pub async fn spent(&self) -> Result<f64> {
        let row: Option<(f64,)> =
            sqlx::query_as("SELECT cost_usd FROM ai_usage WHERE month = strftime('%Y-%m', 'now')")
                .fetch_optional(&self.db)
                .await?;
        Ok(row.map_or(0.0, |(c,)| c))
    }

    /// Speech spend recorded for the current UTC month.
    pub async fn speech_spent(&self) -> Result<f64> {
        let row: Option<(f64,)> = sqlx::query_as(
            "SELECT speech_cost_usd FROM ai_usage WHERE month = strftime('%Y-%m', 'now')",
        )
        .fetch_optional(&self.db)
        .await?;
        Ok(row.map_or(0.0, |(c,)| c))
    }

    fn reserved(&self) -> f64 {
        *self.reserved.lock().expect("budget lock")
    }

    /// Whether a new full generation may start: spend plus reservations must
    /// stay under the soft stop.
    pub async fn allows_new_generation(&self) -> Result<bool> {
        let Some(cap) = self.cap_usd else {
            return Ok(true);
        };
        Ok(self.spent().await? + self.reserved() < cap * SOFT_STOP_SHARE)
    }

    /// Reserves `amount` for one call, or refuses when it would pass the cap.
    pub(crate) async fn reserve(&self, amount: f64) -> Result<bool> {
        let Some(cap) = self.cap_usd else {
            return Ok(true);
        };
        let spent = self.spent().await?;
        let mut reserved = self.reserved.lock().expect("budget lock");
        if spent + *reserved + amount > cap {
            return Ok(false);
        }
        *reserved += amount;
        Ok(true)
    }

    pub(crate) fn release(&self, amount: f64) {
        if self.cap_usd.is_some() {
            let mut reserved = self.reserved.lock().expect("budget lock");
            *reserved = (*reserved - amount).max(0.0);
        }
    }

    /// Reserves `amount` for one speech call. Refuses when it would pass the
    /// cap or narration's share of it ([`SPEECH_SHARE`]).
    pub(crate) async fn reserve_speech(&self, amount: f64) -> Result<bool> {
        let Some(cap) = self.cap_usd else {
            return Ok(true);
        };
        let speech_spent = self.speech_spent().await?;
        let spent = self.spent().await?;
        let mut reserved = self.reserved.lock().expect("budget lock");
        let mut speech_reserved = self.speech_reserved.lock().expect("budget lock");
        if speech_spent + *speech_reserved + amount > cap * SPEECH_SHARE
            || spent + *reserved + amount > cap
        {
            return Ok(false);
        }
        *reserved += amount;
        *speech_reserved += amount;
        Ok(true)
    }

    pub(crate) fn release_speech(&self, amount: f64) {
        if self.cap_usd.is_some() {
            self.release(amount);
            let mut speech_reserved = self.speech_reserved.lock().expect("budget lock");
            *speech_reserved = (*speech_reserved - amount).max(0.0);
        }
    }

    /// Adds one finished speech call to the current month, in the total and
    /// in the speech counter.
    pub async fn record_speech(&self, cost_usd: f64) -> Result<()> {
        sqlx::query(
            "INSERT INTO ai_usage (month, cost_usd, calls, speech_cost_usd, speech_calls)
             VALUES (strftime('%Y-%m', 'now'), ?, 1, ?, 1)
             ON CONFLICT (month) DO UPDATE SET cost_usd = cost_usd + excluded.cost_usd,
                 calls = calls + 1, speech_cost_usd = speech_cost_usd + excluded.speech_cost_usd,
                 speech_calls = speech_calls + 1",
        )
        .bind(cost_usd)
        .bind(cost_usd)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Adds one finished call to the current month.
    pub async fn record(&self, cost_usd: f64) -> Result<()> {
        sqlx::query(
            "INSERT INTO ai_usage (month, cost_usd, calls) VALUES (strftime('%Y-%m', 'now'), ?, 1)
             ON CONFLICT (month) DO UPDATE SET cost_usd = cost_usd + excluded.cost_usd,
                 calls = calls + 1",
        )
        .bind(cost_usd)
        .execute(&self.db)
        .await?;
        Ok(())
    }
}

/// Cost, its source, the reported cost, the table estimate and the outcome
/// of one finished call. A call that timed out or came back unusable was
/// still sent and may be billed without a usage report, so it is counted at
/// its worst-case reservation. A failed request costs nothing.
fn billed_cost(
    result: &dc_ai::Result<TextResponse>,
    price: &crate::pricing::TextPrice,
    reservation: f64,
) -> (f64, &'static str, Option<f64>, Option<f64>, &'static str) {
    match result {
        Ok(response) => {
            let usage = &response.usage;
            let est = price.cost(usage.input_tokens, usage.output_tokens);
            match usage.cost_usd {
                Some(reported) => (reported, "reported", Some(reported), Some(est), "ok"),
                None => (est, price.source.cost_source(), None, Some(est), "ok"),
            }
        }
        Err(AiError::BadResponse(_)) => (
            reservation,
            "reservation",
            None,
            Some(reservation),
            "unusable",
        ),
        Err(_) => (0.0, "none", None, None, "failed"),
    }
}

/// A text model whose calls are checked against and counted in the budget.
pub struct BudgetedModel {
    inner: Arc<dyn TextModel>,
    budget: Arc<Budget>,
}

#[async_trait]
impl TextModel for BudgetedModel {
    fn provider(&self) -> &'static str {
        self.inner.provider()
    }

    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        let provider = self.inner.provider();
        let price = crate::pricing::text_price(provider, &request.model);
        // About four characters per token.
        let input_chars: usize = request.messages.iter().map(|m| m.content.len()).sum();
        let input_tokens = u32::try_from(input_chars / 4).unwrap_or(u32::MAX);
        let reservation = price.max_cost(input_tokens, request.max_tokens);
        match self.budget.reserve(reservation).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(AiError::Limited(
                    "the monthly demo AI budget is used up".into(),
                ));
            }
            Err(err) => return Err(AiError::Request(format!("budget check failed: {err}"))),
        }
        let model_requested = request.model.clone();
        let started = Instant::now();
        let result = self.inner.generate(request).await;
        let latency_ms = started.elapsed().as_millis() as u64;
        self.budget.release(reservation);
        let (cost, cost_source, reported, est, outcome) = billed_cost(&result, &price, reservation);
        if outcome != "failed"
            && let Err(err) = self.budget.record(cost).await
        {
            tracing::warn!(%err, "could not record model spend");
        }
        if !matches!(result, Err(AiError::NotAllowed { .. })) {
            let site = dc_generation::current_call_site();
            let ok = result.as_ref().ok();
            let call = crate::ai_calls::CallRecord {
                stage: site.as_ref().map_or("other", |s| s.stage.as_str()),
                section_key: site.as_ref().map(|s| s.section.clone()),
                round: site.as_ref().map_or(0, |s| s.round),
                attempt: site.as_ref().map_or(1, |s| s.attempt),
                provider,
                model_requested,
                model_returned: ok.map(|r| r.model.clone()),
                provider_route: ok.and_then(|r| r.route.clone()),
                prompt_version: site.map(|s| s.prompt_version),
                input_tokens: ok.map_or(0, |r| r.usage.input_tokens),
                output_tokens: ok.map_or(0, |r| r.usage.output_tokens),
                audio_chars: None,
                latency_ms,
                outcome,
                cost_usd: cost,
                cost_source,
                reported_cost_usd: reported,
                est_cost_usd: est,
            };
            crate::ai_calls::record_or_warn(&self.budget.db, &call).await;
        }
        result
    }
}

/// Everything the job runner needs to generate a presentation.
pub struct Generation {
    pub models: Models,
    /// Translator and translation reviewer for language and reading level
    /// variants. By default the writer and reviewer models (the demo
    /// translator is the same model as the writer, docs/AI_PIPELINE.md).
    pub variants: VariantModels,
    pub budget: Arc<Budget>,
}

impl Generation {
    /// Wraps the given models with the budget guard.
    pub fn new(
        db: SqlitePool,
        cap_usd: Option<f64>,
        writer: Arc<dyn TextModel>,
        writer_model: String,
        reviewer: Arc<dyn TextModel>,
        reviewer_model: String,
        reasoning: Option<dc_ai::ReasoningEffort>,
    ) -> Self {
        let budget = Arc::new(Budget::new(db, cap_usd));
        let wrap = |inner: Arc<dyn TextModel>| -> Arc<dyn TextModel> {
            Arc::new(BudgetedModel {
                inner,
                budget: budget.clone(),
            })
        };
        let writer = wrap(writer);
        let reviewer = wrap(reviewer);
        Self {
            variants: VariantModels {
                translator: writer.clone(),
                translator_model: writer_model.clone(),
                reviewer: reviewer.clone(),
                reviewer_model: reviewer_model.clone(),
                reasoning,
            },
            models: Models {
                writer,
                writer_model,
                reviewer,
                reviewer_model,
                reasoning,
            },
            budget,
        }
    }

    /// Uses other models for variants, wrapped with the same budget guard.
    pub fn with_variant_models(
        mut self,
        translator: Arc<dyn TextModel>,
        translator_model: String,
        reviewer: Arc<dyn TextModel>,
        reviewer_model: String,
    ) -> Self {
        let wrap = |inner: Arc<dyn TextModel>| -> Arc<dyn TextModel> {
            Arc::new(BudgetedModel {
                inner,
                budget: self.budget.clone(),
            })
        };
        self.variants = VariantModels {
            translator: wrap(translator),
            translator_model,
            reviewer: wrap(reviewer),
            reviewer_model,
            reasoning: self.models.reasoning,
        };
        self
    }

    /// Builds the provider for `mode` from the environment, if one is available.
    ///
    /// Demo: OpenRouter with `OPENROUTER_API_KEY`, the app-managed default text
    /// model (or `text_model` from `DC_TEXT_MODEL`) for writer and reviewer, and
    /// the monthly cap. Clinical: always `None`; see [`crate::clinical`]. The
    /// key is never logged.
    pub fn from_env(
        mode: Mode,
        db: &SqlitePool,
        text_model: Option<&str>,
        monthly_budget_usd: f64,
    ) -> Result<Option<Self>> {
        if mode != Mode::Demo {
            return Ok(None);
        }
        let Some(key) = std::env::var("OPENROUTER_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
        else {
            tracing::warn!("OPENROUTER_API_KEY is not set; generation is disabled");
            return Ok(None);
        };
        let client: Arc<dyn TextModel> = Arc::new(OpenRouter::new(
            ProviderPolicy::for_mode(mode),
            key.trim().to_owned(),
        )?);
        let model = text_model.unwrap_or(defaults::DEMO_TEXT_MODEL).to_owned();
        tracing::info!(model = %model, budget_usd = monthly_budget_usd, "generation enabled");
        Ok(Some(Self::new(
            db.clone(),
            Some(monthly_budget_usd),
            client.clone(),
            model.clone(),
            client,
            model,
            Some(defaults::DEMO_TEXT_REASONING),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::open_in_memory;
    use dc_ai::fake::FakeTextModel;
    use dc_ai::{Message, Role};

    fn request() -> TextRequest {
        TextRequest {
            model: "m".into(),
            messages: vec![Message {
                role: Role::User,
                content: "hello".into(),
            }],
            response_schema: None,
            max_tokens: 1000,
            temperature: 0.0,
            reasoning: None,
        }
    }

    #[tokio::test]
    async fn calls_are_counted_and_refused_past_the_cap() {
        let db = open_in_memory().await.unwrap();
        let budget = Arc::new(Budget::new(db.clone(), Some(1.0)));
        let model = BudgetedModel {
            inner: Arc::new(FakeTextModel::with_responses(["ok", "ok"])),
            budget: budget.clone(),
        };
        model.generate(request()).await.unwrap();
        let (calls,): (i64,) = sqlx::query_as("SELECT calls FROM ai_usage")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(calls, 1);
        assert!(budget.allows_new_generation().await.unwrap());

        sqlx::query("UPDATE ai_usage SET cost_usd = 0.85")
            .execute(&db)
            .await
            .unwrap();
        assert!(!budget.allows_new_generation().await.unwrap());
        // Still under the hard cap: one more small call is allowed.
        model.generate(request()).await.unwrap();

        sqlx::query("UPDATE ai_usage SET cost_usd = 1.0")
            .execute(&db)
            .await
            .unwrap();
        assert!(matches!(
            model.generate(request()).await,
            Err(AiError::Limited(_))
        ));
    }

    struct Failing(fn() -> AiError);

    #[async_trait]
    impl TextModel for Failing {
        fn provider(&self) -> &'static str {
            "failing"
        }
        async fn generate(&self, _: TextRequest) -> dc_ai::Result<TextResponse> {
            Err((self.0)())
        }
    }

    #[tokio::test]
    async fn billed_failures_are_counted_at_the_reservation() {
        let db = open_in_memory().await.unwrap();
        let budget = Arc::new(Budget::new(db.clone(), Some(1.0)));
        let timed_out = BudgetedModel {
            inner: Arc::new(Failing(|| {
                AiError::BadResponse("no reply within 120 s".into())
            })),
            budget: budget.clone(),
        };
        assert!(timed_out.generate(request()).await.is_err());
        let (cost, calls): (f64, i64) = sqlx::query_as("SELECT cost_usd, calls FROM ai_usage")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(calls, 1);
        // "hello" is one token by the estimate, plus the 1000-token output cap.
        assert!((cost - estimate_cost(1, 1000)).abs() < 1e-12, "{cost}");

        let refused = BudgetedModel {
            inner: Arc::new(Failing(|| AiError::Request("HTTP 503".into()))),
            budget: budget.clone(),
        };
        assert!(refused.generate(request()).await.is_err());
        let (calls,): (i64,) = sqlx::query_as("SELECT calls FROM ai_usage")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(budget.reserved(), 0.0);

        // Both calls reached the provider, so both are logged; only the
        // timed-out one is charged.
        let rows: Vec<(String, String, f64, String)> = sqlx::query_as(
            "SELECT provider, outcome, cost_usd, cost_source FROM ai_calls ORDER BY id",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].0.as_str(), rows[0].1.as_str(), rows[0].3.as_str()),
            ("failing", "unusable", "reservation")
        );
        assert!((rows[0].2 - cost).abs() < 1e-12);
        assert_eq!(
            (rows[1].1.as_str(), rows[1].2, rows[1].3.as_str()),
            ("failed", 0.0, "none")
        );
    }

    #[tokio::test]
    async fn refused_calls_write_no_row() {
        let db = open_in_memory().await.unwrap();
        let budget = Arc::new(Budget::new(db.clone(), Some(0.0)));
        let model = BudgetedModel {
            inner: Arc::new(FakeTextModel::default()),
            budget,
        };
        assert!(matches!(
            model.generate(request()).await,
            Err(AiError::Limited(_))
        ));
        let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ai_calls")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn reported_cost_wins_over_the_estimate() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cost_usd: Some(0.01),
        };
        assert_eq!(usage_cost(&usage, "fake", "m"), 0.01);
        let unreported = Usage {
            cost_usd: None,
            ..usage
        };
        assert_eq!(usage_cost(&unreported, "fake", "m"), 1.0);
        let listed = crate::pricing::text_price("openrouter", defaults::DEMO_TEXT_MODEL);
        assert_eq!(
            usage_cost(&unreported, "openrouter", defaults::DEMO_TEXT_MODEL),
            listed.cost(1_000_000, 0)
        );
    }
}
