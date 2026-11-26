//! Model setup for the generation pipeline and the demo budget guard.
//!
//! Demo mode builds OpenRouter models when `OPENROUTER_API_KEY` is set.
//! Clinical mode has no provider until the Bedrock path lands (M9), so
//! creating a presentation there is refused.
//!
//! Every model call goes through [`BudgetedModel`], which reserves the call's
//! worst-case cost before it runs and records the actual cost afterwards in
//! `ai_usage` for the current UTC month (docs/AI_PIPELINE.md, demo budget guard).

use anyhow::Result;
use async_trait::async_trait;
use dc_ai::openrouter::OpenRouter;
use dc_ai::{AiError, ProviderPolicy, TextModel, TextRequest, TextResponse, Usage, defaults};
use dc_core::Mode;
use dc_generation::Models;
use sqlx::SqlitePool;
use std::sync::{Arc, Mutex};

/// Share of the monthly cap at which new generations stop. Calls already
/// running may continue up to the full cap.
pub const SOFT_STOP_SHARE: f64 = 0.8;
/// Share of the monthly cap that narration may spend. Together with the soft
/// stop this keeps room for generation after narration's share is used up.
pub const SPEECH_SHARE: f64 = 0.2;

/// Placeholder prices for estimating a call's cost when the provider does
/// not report one, in US dollars per million tokens. Deliberately above the
/// demo model's list price so the estimate errs high. Replace with the dated
/// price table once prices are verified.
const EST_INPUT_USD_PER_MTOK: f64 = 1.0;
const EST_OUTPUT_USD_PER_MTOK: f64 = 4.0;

/// Estimated cost of a call from its token counts.
pub fn estimate_cost(input_tokens: u32, output_tokens: u32) -> f64 {
    (f64::from(input_tokens) * EST_INPUT_USD_PER_MTOK
        + f64::from(output_tokens) * EST_OUTPUT_USD_PER_MTOK)
        / 1_000_000.0
}

/// Cost to record for a usage total: the provider's figure when it reports
/// one, otherwise the estimate.
pub fn usage_cost(usage: &Usage) -> f64 {
    usage
        .cost_usd
        .unwrap_or_else(|| estimate_cost(usage.input_tokens, usage.output_tokens))
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

/// Spend to record for one finished call. A call that timed out or came back
/// unusable was still sent and may be billed without a usage report, so it is
/// counted at its worst-case reservation. Refused or failed requests cost nothing.
fn billed_cost(result: &dc_ai::Result<TextResponse>, reservation: f64) -> Option<f64> {
    match result {
        Ok(response) => Some(usage_cost(&response.usage)),
        Err(AiError::BadResponse(_)) => Some(reservation),
        Err(_) => None,
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
        // About four characters per token.
        let input_chars: usize = request.messages.iter().map(|m| m.content.len()).sum();
        let input_tokens = u32::try_from(input_chars / 4).unwrap_or(u32::MAX);
        let reservation = estimate_cost(input_tokens, request.max_tokens);
        match self.budget.reserve(reservation).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(AiError::Limited(
                    "the monthly demo AI budget is used up".into(),
                ));
            }
            Err(err) => return Err(AiError::Request(format!("budget check failed: {err}"))),
        }
        let result = self.inner.generate(request).await;
        self.budget.release(reservation);
        if let Some(cost) = billed_cost(&result, reservation)
            && let Err(err) = self.budget.record(cost).await
        {
            tracing::warn!(%err, "could not record model spend");
        }
        result
    }
}

/// Everything the job runner needs to generate a presentation.
pub struct Generation {
    pub models: Models,
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
        Self {
            models: Models {
                writer: wrap(writer),
                writer_model,
                reviewer: wrap(reviewer),
                reviewer_model,
                reasoning,
            },
            budget,
        }
    }

    /// Builds the provider for `mode` from the environment, if one is available.
    ///
    /// Demo: OpenRouter with `OPENROUTER_API_KEY`, the app-managed default text
    /// model (or `text_model` from `DC_TEXT_MODEL`) for writer and reviewer, and
    /// the monthly cap. Clinical: none until M9. The key is never logged.
    pub fn from_env(
        mode: Mode,
        db: &SqlitePool,
        text_model: Option<&str>,
        monthly_budget_usd: f64,
    ) -> Result<Option<Self>> {
        if mode != Mode::Demo {
            tracing::info!("no AI provider in clinical mode until M9; generation is disabled");
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
    }

    #[test]
    fn reported_cost_wins_over_the_estimate() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cost_usd: Some(0.01),
        };
        assert_eq!(usage_cost(&usage), 0.01);
        assert_eq!(
            usage_cost(&Usage {
                cost_usd: None,
                ..usage
            }),
            1.0
        );
    }
}
