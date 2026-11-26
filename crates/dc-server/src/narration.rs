//! Sentence narration for Present mode, with a content-addressed cache.
//!
//! Each sentence is synthesized once per speech model, voice, language and
//! exact text. The clip is stored at `<data_dir>/artifacts/audio/<key>.<ext>`
//! with a row in `audio_clips`; `key` is `dc_core::present::audio_key` over
//! [`speech_model_id`], the language and the text, so an edit gives a new key
//! and stale audio is never played.
//!
//! Only the sentence text goes to the provider. The present endpoint passes
//! approved, placeholder-substituted text; nothing else about the patient or
//! the presentation is sent.
//!
//! Demo mode uses OpenRouter (`DC_SPEECH_MODEL`, default
//! `heygen/heygen-voice-1-turbo`; `DC_SPEECH_VOICE`, default `meredith`).
//! Speech cost counts against the same monthly `ai_usage` budget as text,
//! and narration may spend at most [`crate::generation::SPEECH_SHARE`] of
//! the cap. In demo mode every synthesized character also counts against a
//! per-visitor and a per-address quota ([`Quota`]), at least
//! [`MIN_CHARGED_CHARS`] per call; cached clips are free. One client address
//! may make at most [`UPSTREAM_PER_ADDRESS_PER_MINUTE`] of the server's
//! [`UPSTREAM_PER_MINUTE`] upstream calls.
//!
//! A block plays as one clip ([`Narration::block`]): the sentence clips are
//! fetched or synthesized as above, their PCM samples are joined with
//! [`PAUSE_MS`] of silence between sentences, and the result is one WAV with
//! the exact start of each sentence computed from sample counts. The merged
//! clip is cached too, under [`block_key`] of the ordered sentence keys and
//! the pause length, with its offsets stored after the WAV data in the same
//! file. Quotas and the budget are charged per synthesized sentence only.
//!
//! [`Narration::collect_garbage`] removes clips that have not been played
//! for a while and keeps the cache under [`MAX_CACHE_BYTES`].
//! Clinical mode has no speech provider until M9 and answers 503
//! `narration_unavailable`.

use crate::AppState;
use crate::generation::{Budget, Generation};
use crate::http::Failure;
use crate::limits::{Limits, WindowLimiter};
use anyhow::{Context, Result};
use axum::http::StatusCode;
use dc_ai::openrouter_speech::{OpenRouterSpeech, pcm_to_wav, supported_pcm};
use dc_ai::{AiError, ProviderPolicy, SpeechModel, SpeechRequest, defaults};
use dc_core::Mode;
use dc_core::present::{MAX_NARRATION_SENTENCES, audio_key};
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Longest sentence that may be narrated, in characters.
pub const MAX_TEXT_CHARS: usize = 400;
/// OpenRouter list price for `heygen/heygen-voice-1-turbo` (checked
/// 2026-11-26): $4 per million input characters. The response reports no
/// cost, so calls are recorded at this price.
pub const SPEECH_USD_PER_MILLION_CHARS: f64 = 4.0;
/// Extra attempts after a transport or server failure.
const RETRIES: u32 = 2;
/// Extra attempts after a call that timed out or came back unusable. The
/// provider may have billed such a call, so it is retried only once (D12).
const BILLED_RETRIES: u32 = 1;
/// Upstream speech calls per minute across the whole server.
pub const UPSTREAM_PER_MINUTE: u32 = 120;
/// Upstream speech calls per minute from one client address, a quarter of
/// [`UPSTREAM_PER_MINUTE`], so one network cannot keep narration busy for
/// every other visitor.
pub const UPSTREAM_PER_ADDRESS_PER_MINUTE: u32 = 30;
/// Characters charged to the quota for one synthesized call, however short
/// the sentence. The provider limit is per call, so a run of very short
/// sentences must not cost less quota than the calls it uses.
pub const MIN_CHARGED_CHARS: u32 = 50;
/// Concurrent upstream calls while pre-warming.
const PREWARM_CONCURRENCY: usize = 2;
/// Times pre-warming waits out `narration_busy` for one block before it
/// stops. A long presentation can pass the per-address share of upstream
/// calls, and the cache is better filled a little later than not at all.
const PREWARM_BUSY_WAITS: u32 = 5;
/// Concurrent sentence clips fetched or synthesized for one block request.
const BLOCK_CONCURRENCY: usize = 4;
/// Silence between two sentences of a merged block clip.
pub const PAUSE_MS: u32 = 250;
/// `audio_clips.model` prefix of merged block clips, so they can be told
/// apart from sentence clips.
pub const BLOCK_MODEL_PREFIX: &str = "block:";
/// Marks the sentence offsets stored after a merged clip's WAV data.
const OFFSETS_TAG: &[u8; 4] = b"DCSO";
/// Synthesized characters per demo visitor scope per hour.
pub const DEMO_SCOPE_CHARS_PER_HOUR: u32 = 20_000;
/// Synthesized characters per client address per day.
pub const DEMO_ADDRESS_CHARS_PER_DAY: u32 = 60_000;
/// Demo clips not played for this long are deleted.
pub const DEMO_CLIP_RETENTION_SECS: i64 = 24 * 60 * 60;
/// Clinical clips not played for this long are deleted.
pub const CLINICAL_CLIP_RETENTION_SECS: i64 = 30 * 24 * 60 * 60;
/// Largest total size of the audio cache. Past it, the clips played least
/// recently are deleted first.
pub const MAX_CACHE_BYTES: i64 = 2 * 1024 * 1024 * 1024;
/// File extensions the cache writes. `part` is a clip being written.
const CLIP_EXTENSIONS: [&str; 4] = ["mp3", "wav", "bin", "part"];

/// Who a narration request is for, so synthesized characters can be counted
/// per demo visitor scope and per client address.
#[derive(Clone)]
pub struct Quota {
    pub limits: Arc<Limits>,
    /// The presentation's owner scope (the visitor's session token hash).
    pub scope: String,
    /// `limits::ip_key` of the client address.
    pub address: String,
}

impl Quota {
    /// The scope is resolved on every charge, because the token it names may
    /// have rotated since this quota was made (the pre-warm task outlives
    /// the request that started it).
    fn take(&self, chars: u32) -> Result<(), Failure> {
        let hour = Duration::from_secs(60 * 60);
        let day = Duration::from_secs(24 * 60 * 60);
        let scope = self.limits.current_scope(&self.scope);
        self.limits
            .narration_scope
            .take(&scope, chars, DEMO_SCOPE_CHARS_PER_HOUR, hour)
            .map_err(quota_reached)?;
        if let Err(wait) = self.limits.narration_address.take(
            &self.address,
            chars,
            DEMO_ADDRESS_CHARS_PER_DAY,
            day,
        ) {
            self.limits.narration_scope.refund(&scope, chars);
            return Err(quota_reached(wait));
        }
        Ok(())
    }

    fn refund(&self, chars: u32) {
        let scope = self.limits.current_scope(&self.scope);
        self.limits.narration_scope.refund(&scope, chars);
        self.limits.narration_address.refund(&self.address, chars);
    }
}

/// Quota characters charged for synthesizing `text`: its length, but at
/// least [`MIN_CHARGED_CHARS`].
pub fn charged_chars(text: &str) -> u32 {
    u32::try_from(text.chars().count())
        .unwrap_or(u32::MAX)
        .max(MIN_CHARGED_CHARS)
}

fn quota_reached(wait: u64) -> Failure {
    Failure {
        retry_after: Some(wait),
        ..Failure::new(
            StatusCode::TOO_MANY_REQUESTS,
            "narration_quota",
            "The demo narration limit is used up for now. The text is still shown; try the audio again later.",
        )
    }
}

/// What one cache cleanup removed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanupReport {
    /// Clips not played within the retention period.
    pub stale: u64,
    /// Clips removed to keep the cache under the size cap.
    pub over_cap: u64,
    /// Files in the audio directory with no row.
    pub orphans: u64,
}

/// Cost of narrating `text`.
pub fn speech_cost(text: &str) -> f64 {
    text.chars().count() as f64 * SPEECH_USD_PER_MILLION_CHARS / 1_000_000.0
}

pub struct Narration {
    mode: Mode,
    db: SqlitePool,
    speech: Option<Arc<dyn SpeechModel>>,
    model: String,
    voice: String,
    /// `model#voice`, the identity hashed into every audio key.
    voice_id: String,
    budget: Arc<Budget>,
    audio_dir: PathBuf,
    /// One lock per key being synthesized, so concurrent requests for the
    /// same sentence make one upstream call.
    inflight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    upstream: WindowLimiter,
    /// Upstream calls per client address, so one address gets at most its
    /// share of `upstream`.
    upstream_address: WindowLimiter,
    retry_delay: Duration,
}

impl Narration {
    pub fn new(
        mode: Mode,
        db: SqlitePool,
        audio_dir: PathBuf,
        speech: Option<Arc<dyn SpeechModel>>,
        model: String,
        voice: String,
        budget: Arc<Budget>,
    ) -> Self {
        Self {
            mode,
            db,
            speech,
            voice_id: format!("{model}#{voice}"),
            model,
            voice,
            budget,
            audio_dir,
            inflight: Mutex::default(),
            upstream: WindowLimiter::default(),
            upstream_address: WindowLimiter::default(),
            retry_delay: Duration::from_millis(500),
        }
    }

    /// Narration with no provider, for clinical mode and tests.
    pub fn unavailable(mode: Mode, db: SqlitePool) -> Self {
        Self::new(
            mode,
            db.clone(),
            std::env::temp_dir().join("dentalclarity-no-audio"),
            None,
            defaults::DEMO_SPEECH_MODEL.into(),
            defaults::DEMO_SPEECH_VOICE.into(),
            Arc::new(Budget::new(db, None)),
        )
    }

    /// Builds narration for `mode` from the environment.
    ///
    /// Demo: OpenRouter speech with `OPENROUTER_API_KEY`, sharing the text
    /// generation budget when generation is enabled. Clinical: no provider.
    /// The key is never logged.
    pub fn from_env(
        mode: Mode,
        db: &SqlitePool,
        data_dir: &Path,
        generation: Option<&Generation>,
        monthly_budget_usd: f64,
    ) -> Result<Self> {
        let var = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let model = var("DC_SPEECH_MODEL").unwrap_or_else(|| defaults::DEMO_SPEECH_MODEL.into());
        let voice = var("DC_SPEECH_VOICE").unwrap_or_else(|| defaults::DEMO_SPEECH_VOICE.into());
        let audio_dir = data_dir.join("artifacts").join("audio");
        let budget = generation.map_or_else(
            || Arc::new(Budget::new(db.clone(), Some(monthly_budget_usd))),
            |g| g.budget.clone(),
        );
        let speech: Option<Arc<dyn SpeechModel>> = match (mode, var("OPENROUTER_API_KEY")) {
            (Mode::Demo, Some(key)) => {
                std::fs::create_dir_all(&audio_dir)
                    .with_context(|| format!("creating {}", audio_dir.display()))?;
                tracing::info!(model = %model, voice = %voice, "narration enabled");
                Some(Arc::new(OpenRouterSpeech::new(
                    ProviderPolicy::for_mode(mode),
                    key,
                )?))
            }
            (Mode::Demo, None) => {
                tracing::warn!("OPENROUTER_API_KEY is not set; narration is disabled");
                None
            }
            (Mode::Clinical, _) => None,
        };
        Ok(Self::new(
            mode,
            db.clone(),
            audio_dir,
            speech,
            model,
            voice,
            budget,
        ))
    }

    /// The identity hashed into audio keys: speech model and voice.
    pub fn voice_id(&self) -> &str {
        &self.voice_id
    }

    /// The audio for one sentence: `(mime type, bytes)`. Served from the
    /// cache when present, otherwise synthesized and stored. In demo mode a
    /// synthesis counts its characters against `quota`; a cache hit does not.
    pub async fn clip(
        &self,
        language: &str,
        text: &str,
        quota: Option<&Quota>,
    ) -> Result<(String, Vec<u8>), Failure> {
        if self.mode != Mode::Demo {
            return Err(unavailable());
        }
        validate(language, text)?;
        let key = audio_key(&self.voice_id, language, text);
        if let Some(hit) = self.cached(&key).await? {
            return Ok(hit);
        }
        let Some(speech) = self.speech.clone() else {
            return Err(unavailable());
        };

        let lock = self.lock_for(&key);
        let result = {
            let _guard = lock.lock().await;
            // Another request may have stored it while this one waited.
            match self.cached(&key).await {
                Ok(Some(hit)) => Ok(hit),
                Ok(None) => {
                    self.synthesize(speech.as_ref(), &key, language, text, quota)
                        .await
                }
                Err(err) => Err(err),
            }
        };
        self.release(&key, &lock);
        result
    }

    /// The lock for work on `key`, so concurrent requests for one clip do
    /// the work once.
    fn lock_for(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut inflight = self.inflight.lock().expect("narration lock");
        inflight.entry(key.to_owned()).or_default().clone()
    }

    fn release(&self, key: &str, lock: &Arc<tokio::sync::Mutex<()>>) {
        let mut inflight = self.inflight.lock().expect("narration lock");
        if inflight.get(key).is_some_and(|l| Arc::ptr_eq(l, lock)) {
            inflight.remove(key);
        }
    }

    /// One clip for the consecutive sentences `texts` of a block: the
    /// sentence clips joined with [`PAUSE_MS`] of silence, and where each
    /// sentence starts. Served from the cache when the same sentences were
    /// merged before; otherwise the sentence clips are fetched or
    /// synthesized four at a time, each charged as [`Narration::clip`] does.
    pub async fn block(
        self: &Arc<Self>,
        language: &str,
        texts: &[String],
        quota: Option<&Quota>,
    ) -> Result<BlockClip, Failure> {
        self.block_with(language, texts, quota, BLOCK_CONCURRENCY)
            .await
    }

    async fn block_with(
        self: &Arc<Self>,
        language: &str,
        texts: &[String],
        quota: Option<&Quota>,
        concurrency: usize,
    ) -> Result<BlockClip, Failure> {
        if self.mode != Mode::Demo {
            return Err(unavailable());
        }
        if texts.is_empty() || texts.len() > MAX_NARRATION_SENTENCES {
            return Err(Failure::new(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "A narration clip covers one to twelve sentences.",
            ));
        }
        for text in texts {
            validate(language, text)?;
        }
        let keys: Vec<String> = texts
            .iter()
            .map(|t| audio_key(&self.voice_id, language, t))
            .collect();
        let key = block_key(&keys, PAUSE_MS);
        if let Some(hit) = self.cached_block(&key).await? {
            return Ok(hit);
        }
        let lock = self.lock_for(&key);
        let result = {
            let _guard = lock.lock().await;
            match self.cached_block(&key).await {
                Ok(Some(hit)) => Ok(hit),
                Ok(None) => self.merge(&key, language, texts, quota, concurrency).await,
                Err(err) => Err(err),
            }
        };
        self.release(&key, &lock);
        result
    }

    async fn cached_block(&self, key: &str) -> Result<Option<BlockClip>, Failure> {
        let Some((_, bytes)) = self.cached(key).await? else {
            return Ok(None);
        };
        let clip = split_offsets(&bytes).and_then(|(wav, offsets_ms)| {
            let duration_ms = parse_wav(wav)?.duration_ms();
            Some(BlockClip {
                wav: wav.to_vec(),
                offsets_ms,
                duration_ms,
            })
        });
        if clip.is_none() {
            tracing::warn!("cached narration block clip is unreadable; merging it again");
        }
        Ok(clip)
    }

    /// Gets every sentence clip, then joins and stores them. Each sentence
    /// runs in its own task, so a client that goes away does not cancel a
    /// synthesis halfway (its quota and budget reservation stay balanced),
    /// and the finished clips stay cached. After a failure no new sentence
    /// is started.
    async fn merge(
        self: &Arc<Self>,
        key: &str,
        language: &str,
        texts: &[String],
        quota: Option<&Quota>,
        concurrency: usize,
    ) -> Result<BlockClip, Failure> {
        let permits = Arc::new(tokio::sync::Semaphore::new(concurrency.max(1)));
        let failed = Arc::new(AtomicBool::new(false));
        let handles: Vec<_> = texts
            .iter()
            .map(|text| {
                let this = self.clone();
                let permits = permits.clone();
                let failed = failed.clone();
                let language = language.to_owned();
                let text = text.clone();
                let quota = quota.cloned();
                tokio::spawn(async move {
                    let _permit = permits.acquire_owned().await.ok()?;
                    if failed.load(Ordering::SeqCst) {
                        return None;
                    }
                    let result = this.clip(&language, &text, quota.as_ref()).await;
                    if result.is_err() {
                        failed.store(true, Ordering::SeqCst);
                    }
                    Some(result)
                })
            })
            .collect();
        let mut clips = Vec::with_capacity(handles.len());
        let mut failure = None;
        for handle in handles {
            match handle.await {
                Ok(Some(Ok(clip))) => clips.push(clip),
                Ok(Some(Err(err))) => {
                    failure.get_or_insert(err);
                }
                Ok(None) => {}
                Err(err) => {
                    failure.get_or_insert(Failure::internal(err));
                }
            }
        }
        if let Some(failure) = failure {
            return Err(failure);
        }
        let wavs: Vec<&[u8]> = clips
            .iter()
            .filter(|(mime, _)| mime == "audio/wav")
            .map(|(_, bytes)| bytes.as_slice())
            .collect();
        let merged = (wavs.len() == clips.len())
            .then(|| merge_wavs(&wavs, PAUSE_MS))
            .flatten()
            .ok_or_else(|| {
                tracing::warn!("narration clips could not be merged");
                Failure::new(
                    StatusCode::BAD_GATEWAY,
                    "narration_failed",
                    "The narration for this part could not be made. Try again.",
                )
            })?;
        let stored = with_offsets(&merged.wav, &merged.offsets_ms);
        let model = format!("{BLOCK_MODEL_PREFIX}{}", self.voice_id);
        self.store(key, language, &model, "audio/wav", &stored)
            .await?;
        Ok(BlockClip {
            wav: merged.wav,
            offsets_ms: merged.offsets_ms,
            duration_ms: merged.duration_ms,
        })
    }

    fn path_for(&self, key: &str, mime: &str) -> PathBuf {
        self.audio_dir.join(format!("{key}.{}", extension(mime)))
    }

    async fn cached(&self, key: &str) -> Result<Option<(String, Vec<u8>)>, Failure> {
        let row: Option<(String,)> = sqlx::query_as("SELECT mime FROM audio_clips WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.db)
            .await?;
        let Some((mime,)) = row else {
            return Ok(None);
        };
        match std::fs::read(self.path_for(key, &mime)) {
            Ok(bytes) => {
                sqlx::query(
                    "UPDATE audio_clips SET last_used_at = CAST(strftime('%s', 'now') AS INTEGER) WHERE key = ?",
                )
                .bind(key)
                .execute(&self.db)
                .await?;
                Ok(Some((mime, bytes)))
            }
            Err(err) => {
                // The file was removed by hand; synthesize it again.
                tracing::warn!(%err, "cached narration clip is missing");
                sqlx::query("DELETE FROM audio_clips WHERE key = ?")
                    .bind(key)
                    .execute(&self.db)
                    .await?;
                Ok(None)
            }
        }
    }

    /// Synthesizes after taking the characters from `quota`, and gives them
    /// back when nothing was synthesized and nothing may have been billed.
    async fn synthesize(
        &self,
        speech: &dyn SpeechModel,
        key: &str,
        language: &str,
        text: &str,
        quota: Option<&Quota>,
    ) -> Result<(String, Vec<u8>), Failure> {
        let chars = charged_chars(text);
        if let Some(quota) = quota {
            quota.take(chars)?;
        }
        let mut billed = 0;
        let address = quota.map(|q| q.address.as_str());
        let result = self
            .call(speech, language, text, address, &mut billed)
            .await;
        let clip = match result {
            Ok(clip) => clip,
            Err(failure) => {
                if billed == 0
                    && let Some(quota) = quota
                {
                    quota.refund(chars);
                }
                return Err(failure);
            }
        };
        tracing::debug!(
            bytes = clip.bytes.len(),
            duration_ms = clip.duration_ms,
            "narration clip synthesized"
        );
        self.store(key, language, &self.voice_id, &clip.mime_type, &clip.bytes)
            .await?;
        Ok((clip.mime_type, clip.bytes))
    }

    /// Counts one upstream call for `address` and for the whole server, or
    /// answers `429 narration_busy`.
    fn take_upstream(&self, address: Option<&str>) -> Result<(), Failure> {
        let minute = Duration::from_secs(60);
        let busy = |wait| Failure {
            retry_after: Some(wait),
            ..Failure::new(
                StatusCode::TOO_MANY_REQUESTS,
                "narration_busy",
                "Narration is busy. Try again in a moment.",
            )
        };
        if let Some(address) = address {
            self.upstream_address
                .check(address, UPSTREAM_PER_ADDRESS_PER_MINUTE, minute)
                .map_err(busy)?;
        }
        if let Err(wait) = self.upstream.check("speech", UPSTREAM_PER_MINUTE, minute) {
            if let Some(address) = address {
                self.upstream_address.refund(address, 1);
            }
            return Err(busy(wait));
        }
        Ok(())
    }

    /// Calls the provider, retrying transport and server failures twice and
    /// a timeout or unusable reply once. Every attempt counts against the
    /// upstream limits and reserves its cost first. A clip, a timeout and an
    /// unusable reply are recorded at [`speech_cost`], since the provider
    /// may have billed them (D12); `billed` counts those attempts.
    async fn call(
        &self,
        speech: &dyn SpeechModel,
        language: &str,
        text: &str,
        address: Option<&str>,
        billed: &mut u32,
    ) -> Result<dc_ai::SpeechClip, Failure> {
        let cost = speech_cost(text);
        let request = SpeechRequest {
            model: self.model.clone(),
            text: text.to_owned(),
            language: language.to_owned(),
            voice: self.voice.clone(),
        };
        let mut transport_retries = 0;
        loop {
            self.take_upstream(address)?;
            if !self.budget.reserve_speech(cost).await? {
                return Err(Failure::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "budget_reached",
                    "Demo narration budget reached for this month. Narration is paused until next month.",
                ));
            }
            let result = speech.synthesize(request.clone()).await;
            self.budget.release_speech(cost);
            if matches!(result, Ok(_) | Err(AiError::BadResponse(_))) {
                *billed += 1;
                if let Err(err) = self.budget.record_speech(cost).await {
                    tracing::warn!(%err, "could not record narration spend");
                }
            }
            match result {
                Ok(clip) => return Ok(clip),
                Err(AiError::Request(err)) if transport_retries < RETRIES => {
                    transport_retries += 1;
                    tracing::debug!(%err, transport_retries, "narration call failed; retrying");
                    tokio::time::sleep(self.retry_delay * transport_retries).await;
                }
                Err(AiError::BadResponse(err)) if *billed <= BILLED_RETRIES => {
                    tracing::debug!(%err, "narration reply was unusable; retrying once");
                    tokio::time::sleep(self.retry_delay).await;
                }
                Err(AiError::Limited(err)) => {
                    tracing::warn!(%err, "narration provider refused the call");
                    return Err(Failure::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "narration_unavailable",
                        "Narration is not available right now.",
                    ));
                }
                Err(AiError::NotAllowed { .. }) => return Err(unavailable()),
                Err(err) => {
                    tracing::warn!(%err, "narration failed");
                    return Err(Failure::new(
                        StatusCode::BAD_GATEWAY,
                        "narration_failed",
                        "The narration for this sentence could not be made. Try again.",
                    ));
                }
            }
        }
    }

    async fn store(
        &self,
        key: &str,
        language: &str,
        model: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<()> {
        std::fs::create_dir_all(&self.audio_dir)
            .with_context(|| format!("creating {}", self.audio_dir.display()))?;
        let path = self.path_for(key, mime);
        let partial = path.with_extension("part");
        std::fs::write(&partial, bytes).context("writing narration clip")?;
        std::fs::rename(&partial, &path).context("storing narration clip")?;
        sqlx::query(
            "INSERT INTO audio_clips (key, language, model, bytes, mime, created_at, last_used_at)
             VALUES (?, ?, ?, ?, ?, CAST(strftime('%s', 'now') AS INTEGER), CAST(strftime('%s', 'now') AS INTEGER))
             ON CONFLICT (key) DO UPDATE SET bytes = excluded.bytes, mime = excluded.mime,
                 last_used_at = excluded.last_used_at",
        )
        .bind(key)
        .bind(language)
        .bind(model)
        .bind(bytes.len() as i64)
        .bind(mime)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Fills the cache for `blocks` in the background, one block at a time
    /// in order with two upstream calls at a time, so each block's merged
    /// clip is ready as early as possible. Each block is the sentence texts
    /// of one clip, at most [`MAX_NARRATION_SENTENCES`]. When narration is
    /// busy it waits as told and tries the block again; other failures are
    /// logged and skipped. Nothing happens without a provider.
    pub fn prewarm(
        self: Arc<Self>,
        language: String,
        blocks: Vec<Vec<String>>,
        quota: Option<Quota>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if self.mode != Mode::Demo || self.speech.is_none() {
                return;
            }
            let mut seen = HashSet::new();
            for texts in blocks {
                if texts.is_empty() || !seen.insert(texts.clone()) {
                    continue;
                }
                let mut waits = 0;
                loop {
                    let result = self
                        .block_with(&language, &texts, quota.as_ref(), PREWARM_CONCURRENCY)
                        .await;
                    let Err(failure) = result else {
                        break;
                    };
                    if failure.code == "narration_busy" && waits < PREWARM_BUSY_WAITS {
                        waits += 1;
                        // Sentences already made stay cached, so the retry
                        // only asks for the rest.
                        let wait = failure.retry_after.unwrap_or(1).clamp(1, 60);
                        tokio::time::sleep(Duration::from_secs(wait)).await;
                        continue;
                    }
                    tracing::debug!(code = failure.code, "narration pre-warm skipped a block");
                    // Out of budget, quota or capacity: the rest would fail
                    // the same way.
                    if matches!(
                        failure.code,
                        "budget_reached" | "narration_busy" | "narration_quota"
                    ) {
                        return;
                    }
                    break;
                }
            }
        })
    }
}

impl Narration {
    /// Deletes clips not played within the retention period of this mode
    /// (24 hours in demo, 30 days in clinical), then the least recently
    /// played clips until the cache is under [`MAX_CACHE_BYTES`], then files
    /// with no row that are older than the retention period.
    pub async fn collect_garbage(&self) -> Result<CleanupReport> {
        self.collect_garbage_at(crate::sessions::now(), MAX_CACHE_BYTES)
            .await
    }

    pub(crate) async fn collect_garbage_at(
        &self,
        now: i64,
        max_bytes: i64,
    ) -> Result<CleanupReport> {
        let retention = match self.mode {
            Mode::Demo => DEMO_CLIP_RETENTION_SECS,
            Mode::Clinical => CLINICAL_CLIP_RETENTION_SECS,
        };
        let cutoff = now - retention;
        let mut report = CleanupReport::default();

        let stale: Vec<(String, String)> =
            sqlx::query_as("SELECT key, mime FROM audio_clips WHERE last_used_at <= ?")
                .bind(cutoff)
                .fetch_all(&self.db)
                .await?;
        for (key, mime) in stale {
            // Skips a clip that was played since the select.
            let removed =
                sqlx::query("DELETE FROM audio_clips WHERE key = ? AND last_used_at <= ?")
                    .bind(&key)
                    .bind(cutoff)
                    .execute(&self.db)
                    .await?
                    .rows_affected();
            if removed > 0 {
                self.remove_clip_file(&key, &mime);
                report.stale += 1;
            }
        }

        let (total,): (i64,) = sqlx::query_as("SELECT COALESCE(SUM(bytes), 0) FROM audio_clips")
            .fetch_one(&self.db)
            .await?;
        if total > max_bytes {
            let mut total = total;
            let by_age: Vec<(String, String, i64)> = sqlx::query_as(
                "SELECT key, mime, bytes FROM audio_clips ORDER BY last_used_at, created_at",
            )
            .fetch_all(&self.db)
            .await?;
            for (key, mime, bytes) in by_age {
                if total <= max_bytes {
                    break;
                }
                sqlx::query("DELETE FROM audio_clips WHERE key = ?")
                    .bind(&key)
                    .execute(&self.db)
                    .await?;
                self.remove_clip_file(&key, &mime);
                total -= bytes;
                report.over_cap += 1;
            }
        }

        report.orphans = self.remove_orphan_files(now, retention).await?;
        Ok(report)
    }

    /// Removes the file of one clip. Only a 64-character hex key with a known
    /// extension inside the audio directory is ever deleted.
    fn remove_clip_file(&self, key: &str, mime: &str) {
        if !is_clip_key(key) {
            tracing::warn!("audio clip row with an unexpected key; file left in place");
            return;
        }
        match std::fs::remove_file(self.path_for(key, mime)) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!(%err, "could not delete a narration clip"),
        }
    }

    /// Deletes clip files that have no row and were last written before the
    /// retention period, such as leftovers of an interrupted write.
    async fn remove_orphan_files(&self, now: i64, retention: i64) -> Result<u64> {
        let entries = match std::fs::read_dir(&self.audio_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(err) => return Err(err).context("reading the audio directory"),
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(key) = name.to_str().and_then(clip_file_key) else {
                continue;
            };
            // `DirEntry::metadata` does not follow symbolic links.
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(now, |d| i64::try_from(d.as_secs()).unwrap_or(now));
            if modified > now - retention {
                continue;
            }
            let known: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM audio_clips WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.db)
                .await?;
            if known.is_some() {
                continue;
            }
            match std::fs::remove_file(self.audio_dir.join(&name)) {
                Ok(()) => removed += 1,
                Err(err) => tracing::warn!(%err, "could not delete an orphan narration file"),
            }
        }
        Ok(removed)
    }
}

fn is_clip_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The key of a cache file name `<64 hex>.<known extension>`, if it is one.
fn clip_file_key(name: &str) -> Option<&str> {
    let (key, ext) = name.split_once('.')?;
    (is_clip_key(key) && CLIP_EXTENSIONS.contains(&ext)).then_some(key)
}

/// A merged block clip: one WAV and where each sentence starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockClip {
    pub wav: Vec<u8>,
    /// Start of each sentence in whole milliseconds, from sample counts.
    pub offsets_ms: Vec<u32>,
    /// Length of the clip in whole milliseconds.
    pub duration_ms: u32,
}

/// Cache key of the merged clip for the ordered sentence keys `sentence_keys`
/// joined with `pause_ms` of silence. The sentence keys already cover the
/// voice, language and text.
pub fn block_key(sentence_keys: &[String], pause_ms: u32) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"dc-narration-block-v1");
    hasher.update(pause_ms.to_le_bytes());
    for key in sentence_keys {
        hasher.update((key.len() as u64).to_le_bytes());
        hasher.update(key.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// 16-bit PCM samples inside a WAV file.
struct Wav<'a> {
    rate: u32,
    channels: u16,
    data: &'a [u8],
}

impl Wav<'_> {
    fn frame_bytes(&self) -> usize {
        usize::from(self.channels) * 2
    }

    fn frames(&self) -> u64 {
        (self.data.len() / self.frame_bytes()) as u64
    }

    fn duration_ms(&self) -> u32 {
        frames_to_ms(self.frames(), self.rate)
    }
}

fn frames_to_ms(frames: u64, rate: u32) -> u32 {
    frames
        .checked_mul(1000)
        .and_then(|ms| ms.checked_div(u64::from(rate)))
        .and_then(|ms| u32::try_from(ms).ok())
        .unwrap_or(u32::MAX)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The samples of a 16-bit PCM WAV file, cut to whole frames. Other formats,
/// and rates or channel counts outside `supported_pcm` (8 to 48 kHz, mono or
/// stereo), give `None`.
fn parse_wav(bytes: &[u8]) -> Option<Wav<'_>> {
    if bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut format = None;
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = usize::try_from(u32_at(bytes, pos + 4)?).ok()?;
        let start = pos.checked_add(8)?;
        let body = &bytes[start..start.saturating_add(size).min(bytes.len())];
        match id {
            b"fmt " => {
                let (tag, channels) = (u16_at(body, 0)?, u16_at(body, 2)?);
                let (rate, bits) = (u32_at(body, 4)?, u16_at(body, 14)?);
                if tag != 1 || bits != 16 || !supported_pcm(rate, channels) {
                    return None;
                }
                format = Some((rate, channels));
            }
            b"data" => {
                let (rate, channels) = format?;
                let frame = usize::from(channels) * 2;
                let whole = body.len() / frame * frame;
                return Some(Wav {
                    rate,
                    channels,
                    data: &body[..whole],
                });
            }
            _ => {}
        }
        pos = start.checked_add(size)?.checked_add(size & 1)?;
    }
    None
}

/// Joins WAV clips of one format with `pause_ms` of silence between them.
/// `None` when a clip is not 16-bit PCM WAV or the formats differ.
fn merge_wavs(clips: &[&[u8]], pause_ms: u32) -> Option<BlockClip> {
    let parsed: Vec<Wav<'_>> = clips.iter().map(|c| parse_wav(c)).collect::<Option<_>>()?;
    let first = parsed.first()?;
    let (rate, channels) = (first.rate, first.channels);
    if parsed
        .iter()
        .any(|w| w.rate != rate || w.channels != channels)
    {
        return None;
    }
    let frame = first.frame_bytes();
    let pause_frames =
        usize::try_from(u64::from(rate).checked_mul(u64::from(pause_ms))? / 1000).ok()?;
    let pause = vec![0u8; pause_frames.checked_mul(frame)?];
    let total = parsed
        .iter()
        .try_fold(0usize, |sum, w| sum.checked_add(w.data.len()))?
        .checked_add(pause.len().checked_mul(parsed.len() - 1)?)?;
    let mut pcm = Vec::with_capacity(total);
    let mut offsets_ms = Vec::with_capacity(parsed.len());
    for (i, wav) in parsed.iter().enumerate() {
        if i > 0 {
            pcm.extend_from_slice(&pause);
        }
        offsets_ms.push(frames_to_ms((pcm.len() / frame) as u64, rate));
        pcm.extend_from_slice(wav.data);
    }
    let duration_ms = frames_to_ms((pcm.len() / frame) as u64, rate);
    Some(BlockClip {
        wav: pcm_to_wav(&pcm, rate, channels)?,
        offsets_ms,
        duration_ms,
    })
}

/// A merged clip as stored: the WAV, then `DCSO`, the sentence count and
/// each offset as little-endian `u32`. The RIFF size covers only the WAV, so
/// [`split_offsets`] can cut the stored offsets off again.
fn with_offsets(wav: &[u8], offsets_ms: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(wav.len() + 8 + 4 * offsets_ms.len());
    out.extend_from_slice(wav);
    out.extend_from_slice(OFFSETS_TAG);
    out.extend_from_slice(&u32::try_from(offsets_ms.len()).unwrap_or(0).to_le_bytes());
    for offset in offsets_ms {
        out.extend_from_slice(&offset.to_le_bytes());
    }
    out
}

/// The WAV and offsets of a stored merged clip.
fn split_offsets(stored: &[u8]) -> Option<(&[u8], Vec<u32>)> {
    let wav_len = usize::try_from(u32_at(stored, 4)?).ok()?.checked_add(8)?;
    let wav = stored.get(..wav_len)?;
    let tail = stored.get(wav_len..)?;
    if tail.get(0..4)? != OFFSETS_TAG {
        return None;
    }
    let count = usize::try_from(u32_at(tail, 4)?).ok()?;
    if count == 0 || tail.len() != 8 + 4 * count {
        return None;
    }
    let offsets = (0..count)
        .map(|i| u32_at(tail, 8 + 4 * i))
        .collect::<Option<_>>()?;
    Some((wav, offsets))
}

fn unavailable() -> Failure {
    Failure::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "narration_unavailable",
        "Narration is not available on this server.",
    )
}

fn validate(language: &str, text: &str) -> Result<(), Failure> {
    let language_ok = !language.is_empty()
        && language.len() <= 35
        && language
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if !language_ok {
        return Err(Failure::new(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Unknown narration language.",
        ));
    }
    if text.trim().is_empty() {
        return Err(Failure::new(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "There is no text to narrate.",
        ));
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(Failure::new(
            StatusCode::BAD_REQUEST,
            "narration_text_too_long",
            "This sentence is too long to narrate.",
        ));
    }
    Ok(())
}

fn extension(mime: &str) -> &'static str {
    match mime {
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        _ => "bin",
    }
}

/// The quota for a request from `address` about presentations of `scope`.
/// `None` outside demo mode.
pub fn quota(state: &AppState, scope: &str, address: std::net::IpAddr) -> Option<Quota> {
    (state.mode == Mode::Demo).then(|| Quota {
        limits: state.limits.clone(),
        scope: scope.to_owned(),
        address: crate::limits::ip_key(address),
    })
}

/// The audio for one sentence: `(mime type, bytes)`. Called by
/// `GET /api/present/audio/{key}` with the sentence text that key belongs to.
pub async fn clip(
    state: &AppState,
    language: &str,
    text: &str,
    quota: Option<&Quota>,
) -> Result<(String, Vec<u8>), Failure> {
    state.narration.clip(language, text, quota).await
}

/// The speech identity to pass to `dc_core::present::audio_key`, so keys
/// handed to the browser match the cache: `<model>#<voice>`, by default
/// `heygen/heygen-voice-1-turbo#meredith`.
pub fn speech_model_id(state: &AppState) -> &str {
    state.narration.voice_id()
}

/// One clip for consecutive sentences of a block. Called by
/// `GET /api/present/narration` with the texts the requested keys belong to.
pub async fn block(
    state: &AppState,
    language: &str,
    texts: &[String],
    quota: Option<&Quota>,
) -> Result<BlockClip, Failure> {
    state.narration.block(language, texts, quota).await
}

/// Fills the cache for `blocks` (the sentence texts of each clip) in the
/// background, so the first play is fast. Returns at once; awaiting the
/// handle is optional.
pub fn prewarm(
    state: &AppState,
    language: &str,
    blocks: Vec<Vec<String>>,
    quota: Option<Quota>,
) -> tokio::task::JoinHandle<()> {
    state
        .narration
        .clone()
        .prewarm(language.to_owned(), blocks, quota)
}

/// A fake speech provider and a scratch audio directory, shared with the
/// API tests.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use async_trait::async_trait;
    use dc_ai::SpeechClip;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Counts calls, waits a little so concurrent requests overlap, and can
    /// fail the first few calls with a transport error (`fail_first`) or, after
    /// those, with an unusable reply such as a timeout (`unusable_first`).
    #[derive(Default)]
    pub(crate) struct FakeSpeech {
        pub calls: AtomicU32,
        pub fail_first: u32,
        pub unusable_first: u32,
        pub texts: Mutex<Vec<String>>,
    }

    impl FakeSpeech {
        pub fn calls(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl SpeechModel for FakeSpeech {
        fn provider(&self) -> &'static str {
            "fake"
        }

        async fn synthesize(&self, request: SpeechRequest) -> dc_ai::Result<SpeechClip> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            self.texts.lock().unwrap().push(request.text.clone());
            tokio::time::sleep(Duration::from_millis(30)).await;
            if n < self.fail_first {
                return Err(AiError::Request("connection reset".into()));
            }
            if n < self.fail_first + self.unusable_first {
                return Err(AiError::BadResponse("no reply within 30 s".into()));
            }
            Ok(fake_clip(&request.text))
        }
    }

    /// Frames per character of fake speech: 10 ms at 44.1 kHz.
    pub(crate) const FAKE_FRAMES_PER_CHAR: usize = 441;

    /// 16-bit mono 44.1 kHz WAV, [`FAKE_FRAMES_PER_CHAR`] frames per
    /// character, with no silent samples.
    pub(crate) fn fake_clip(text: &str) -> SpeechClip {
        let frames = text.chars().count() * FAKE_FRAMES_PER_CHAR;
        let pcm: Vec<u8> = std::iter::repeat_n([0x01, 0x00], frames)
            .flatten()
            .collect();
        SpeechClip {
            mime_type: "audio/wav".into(),
            bytes: dc_ai::openrouter_speech::pcm_to_wav(&pcm, 44_100, 1).unwrap(),
            duration_ms: Some(u32::try_from(frames * 1000 / 44_100).unwrap_or(0)),
        }
    }

    /// A per-test directory under the system temp dir, removed on drop.
    pub(crate) struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new() -> Self {
            let mut id = [0u8; 8];
            getrandom::fill(&mut id).unwrap();
            Self(std::env::temp_dir().join(format!("dc-narration-{}", hex::encode(id))))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Narration for `mode` with `speech`, storing clips under `dir`.
    pub(crate) fn narration(
        mode: Mode,
        db: SqlitePool,
        dir: &TempDir,
        speech: Arc<FakeSpeech>,
        budget: Arc<Budget>,
    ) -> Narration {
        let mut narration = Narration::new(
            mode,
            db,
            dir.0.clone(),
            Some(speech),
            "test/voice-model".into(),
            "calm".into(),
            budget,
        );
        narration.retry_delay = Duration::from_millis(1);
        narration
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{self, FakeSpeech, TempDir};
    use super::*;
    use crate::storage::open_in_memory;
    use std::sync::atomic::Ordering;

    struct Setup {
        narration: Arc<Narration>,
        speech: Arc<FakeSpeech>,
        db: SqlitePool,
        _dir: TempDir,
    }

    async fn setup(mode: Mode, fake: FakeSpeech, cap: Option<f64>) -> Setup {
        let db = open_in_memory().await.unwrap();
        let dir = TempDir::new();
        let speech = Arc::new(fake);
        let narration = test_support::narration(
            mode,
            db.clone(),
            &dir,
            speech.clone(),
            Arc::new(Budget::new(db.clone(), cap)),
        );
        Setup {
            narration: Arc::new(narration),
            speech,
            db,
            _dir: dir,
        }
    }

    fn quota(limits: &Arc<Limits>, scope: &str, address: &str) -> Quota {
        Quota {
            limits: limits.clone(),
            scope: scope.into(),
            address: address.into(),
        }
    }

    const SENTENCE: &str = "A crown covers and protects a weak tooth.";

    #[tokio::test]
    async fn cache_hit_returns_the_same_bytes_without_a_call() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let first = s.narration.clip("en", SENTENCE, None).await.unwrap();
        let second = s.narration.clip("en", SENTENCE, None).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(first.0, "audio/wav");
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 1);

        let key = audio_key(s.narration.voice_id(), "en", SENTENCE);
        let (bytes, model): (i64, String) =
            sqlx::query_as("SELECT bytes, model FROM audio_clips WHERE key = ?")
                .bind(&key)
                .fetch_one(&s.db)
                .await
                .unwrap();
        assert_eq!(bytes as usize, first.1.len());
        assert_eq!(model, "test/voice-model#calm");
        assert!(s._dir.0.join(format!("{key}.wav")).exists());

        // Another language or text is a different clip.
        s.narration.clip("es", SENTENCE, None).await.unwrap();
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 2);
        // Only the sentence text reaches the provider.
        assert_eq!(s.speech.texts.lock().unwrap()[0], SENTENCE);
    }

    #[tokio::test]
    async fn concurrent_requests_make_one_upstream_call() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let calls = (0..8).map(|_| {
            let n = s.narration.clone();
            tokio::spawn(async move { n.clip("en", SENTENCE, None).await.unwrap() })
        });
        let results: Vec<_> = futures_join(calls).await;
        assert!(results.windows(2).all(|w| w[0] == w[1]));
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 1);
        assert!(s.narration.inflight.lock().unwrap().is_empty());
    }

    async fn futures_join<T: Send + 'static>(
        handles: impl Iterator<Item = tokio::task::JoinHandle<T>>,
    ) -> Vec<T> {
        let mut out = Vec::new();
        for h in handles.collect::<Vec<_>>() {
            out.push(h.await.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn long_or_empty_text_is_refused() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let long = "a".repeat(MAX_TEXT_CHARS + 1);
        let err = s.narration.clip("en", &long, None).await.unwrap_err();
        assert_eq!(err.code, "narration_text_too_long");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(
            s.narration
                .clip("en", &"a".repeat(MAX_TEXT_CHARS), None)
                .await
                .is_ok()
        );
        assert_eq!(
            s.narration.clip("en", "  ", None).await.unwrap_err().code,
            "bad_request"
        );
        assert_eq!(
            s.narration
                .clip("e n", SENTENCE, None)
                .await
                .unwrap_err()
                .code,
            "bad_request"
        );
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn clinical_mode_is_unavailable() {
        let s = setup(Mode::Clinical, FakeSpeech::default(), None).await;
        let err = s.narration.clip("en", SENTENCE, None).await.unwrap_err();
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.code, "narration_unavailable");
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 0);

        let db = open_in_memory().await.unwrap();
        let none = Narration::unavailable(Mode::Demo, db);
        assert_eq!(
            none.clip("en", SENTENCE, None).await.unwrap_err().code,
            "narration_unavailable"
        );
    }

    #[tokio::test]
    async fn spend_is_recorded_and_refused_past_the_cap() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(1.0)).await;
        s.narration.clip("en", SENTENCE, None).await.unwrap();
        let (cost, calls): (f64, i64) = sqlx::query_as("SELECT cost_usd, calls FROM ai_usage")
            .fetch_one(&s.db)
            .await
            .unwrap();
        assert_eq!(calls, 1);
        assert!((cost - speech_cost(SENTENCE)).abs() < 1e-12);

        sqlx::query("UPDATE ai_usage SET cost_usd = 1.0")
            .execute(&s.db)
            .await
            .unwrap();
        let err = s
            .narration
            .clip("en", "Another sentence.", None)
            .await
            .unwrap_err();
        assert_eq!(err.code, "budget_reached");
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        // Cached clips still play.
        assert!(s.narration.clip("en", SENTENCE, None).await.is_ok());
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn transport_failures_are_retried_twice() {
        let fake = FakeSpeech {
            fail_first: 2,
            ..FakeSpeech::default()
        };
        let s = setup(Mode::Demo, fake, Some(25.0)).await;
        assert!(s.narration.clip("en", SENTENCE, None).await.is_ok());
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 3);

        let fake = FakeSpeech {
            fail_first: 3,
            ..FakeSpeech::default()
        };
        let s = setup(Mode::Demo, fake, Some(25.0)).await;
        let err = s.narration.clip("en", SENTENCE, None).await.unwrap_err();
        assert_eq!(err.code, "narration_failed");
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 3);
        // Nothing is charged or cached for a failed sentence.
        let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM audio_clips")
            .fetch_one(&s.db)
            .await
            .unwrap();
        assert_eq!(rows, 0);
    }

    fn texts(list: &[&str]) -> Vec<String> {
        list.iter().map(|t| (*t).to_owned()).collect()
    }

    async fn block_rows(db: &SqlitePool) -> i64 {
        let (n,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM audio_clips WHERE model LIKE 'block:%'")
                .fetch_one(db)
                .await
                .unwrap();
        n
    }

    #[tokio::test]
    async fn prewarm_fills_the_cache_once_per_sentence_and_block() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let blocks = vec![
            texts(&["One.", "Two."]),
            texts(&["One.", "Three."]),
            texts(&["One.", "Two."]),
        ];
        s.narration
            .clone()
            .prewarm("en".into(), blocks, None)
            .await
            .unwrap();
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 3);
        assert_eq!(block_rows(&s.db).await, 2);
        s.narration.clip("en", "Two.", None).await.unwrap();
        s.narration
            .block("en", &texts(&["One.", "Three."]), None)
            .await
            .unwrap();
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 3);
    }

    /// A 16-bit WAV of `frames` frames, every sample `value`.
    fn wav(frames: usize, rate: u32, channels: u16, value: i16) -> Vec<u8> {
        let samples = frames * usize::from(channels);
        let pcm: Vec<u8> = std::iter::repeat_n(value.to_le_bytes(), samples)
            .flatten()
            .collect();
        pcm_to_wav(&pcm, rate, channels).unwrap()
    }

    #[test]
    fn merged_offsets_follow_sample_counts() {
        let (a, b, c) = (
            wav(1000, 44_100, 1, 7),
            wav(2205, 44_100, 1, 8),
            wav(7, 44_100, 1, 9),
        );
        let merged = merge_wavs(&[&a, &b, &c], 250).unwrap();
        // 250 ms at 44.1 kHz is 11,025 frames of silence.
        let pause = 11_025;
        let starts = [0, 1000 + pause, 1000 + pause + 2205 + pause];
        let total = starts[2] + 7;
        assert_eq!(
            merged.offsets_ms,
            starts.map(|f| u32::try_from(f * 1000 / 44_100).unwrap())
        );
        assert_eq!(merged.offsets_ms, [0, 272, 572]);
        assert_eq!(merged.duration_ms, 572);
        let parsed = parse_wav(&merged.wav).unwrap();
        assert_eq!((parsed.rate, parsed.channels), (44_100, 1));
        assert_eq!(parsed.frames(), total as u64);
        let sample =
            |frame: usize| i16::from_le_bytes([parsed.data[2 * frame], parsed.data[2 * frame + 1]]);
        assert_eq!(sample(0), 7);
        assert_eq!(sample(999), 7);
        assert!((1000..1000 + pause).all(|f| sample(f) == 0));
        assert_eq!(sample(starts[1]), 8);
        assert_eq!(sample(starts[2] - 1), 0);
        assert_eq!(sample(starts[2]), 9);
        assert_eq!(sample(total - 1), 9);

        // Stereo counts frames, not samples.
        let stereo = merge_wavs(&[&wav(4410, 44_100, 2, 1), &wav(10, 44_100, 2, 1)], 250).unwrap();
        assert_eq!(stereo.offsets_ms, [0, 350]);
        // A single clip has no pause.
        assert_eq!(merge_wavs(&[&a], 250).unwrap().duration_ms, 22);
        // Mixed formats and non-WAV clips cannot be merged.
        assert!(merge_wavs(&[&a, &wav(10, 22_050, 1, 1)], 250).is_none());
        assert!(merge_wavs(&[&a, &wav(10, 44_100, 2, 1)], 250).is_none());
        assert!(merge_wavs(&[&a, b"ID3 not a wav"], 250).is_none());
        assert!(merge_wavs(&[], 250).is_none());
    }

    #[test]
    fn wav_parsing_skips_other_chunks_and_partial_frames() {
        let plain = wav(3, 8_000, 1, 5);
        let mut bytes = b"RIFF    WAVE".to_vec();
        // An odd-sized chunk before `fmt `, padded to an even length.
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"abc ");
        bytes.extend_from_slice(&plain[12..36]);
        // Three frames and half of a fourth.
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&7u32.to_le_bytes());
        bytes.extend_from_slice(&[5, 0, 5, 0, 5, 0, 1]);
        let parsed = parse_wav(&bytes).unwrap();
        assert_eq!(parsed.frames(), 3);
        assert_eq!(parsed.rate, 8_000);
        // 8-bit or compressed WAV is refused.
        let mut eight_bit = plain.clone();
        eight_bit[34] = 8;
        assert!(parse_wav(&eight_bit).is_none());
        let mut compressed = plain;
        compressed[20] = 3;
        assert!(parse_wav(&compressed).is_none());
    }

    #[test]
    fn stored_offsets_round_trip() {
        let merged = merge_wavs(&[&wav(100, 44_100, 1, 1), &wav(100, 44_100, 1, 1)], 250).unwrap();
        let stored = with_offsets(&merged.wav, &merged.offsets_ms);
        let (wav_part, offsets) = split_offsets(&stored).unwrap();
        assert_eq!(wav_part, merged.wav.as_slice());
        assert_eq!(offsets, merged.offsets_ms);
        assert!(split_offsets(&merged.wav).is_none());
        assert!(split_offsets(&stored[..stored.len() - 1]).is_none());
        assert!(split_offsets(b"RIFF").is_none());
    }

    #[test]
    fn block_keys_depend_on_order_and_pause() {
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        let ab = block_key(&[a.clone(), b.clone()], 250);
        assert_eq!(ab.len(), 64);
        assert_eq!(ab, block_key(&[a.clone(), b.clone()], 250));
        assert_ne!(ab, block_key(&[b.clone(), a.clone()], 250));
        assert_ne!(ab, block_key(&[a.clone(), b], 300));
        assert_ne!(ab, block_key(&[a], 250));
    }

    #[tokio::test]
    async fn block_is_merged_cached_and_charged_per_synthesized_sentence() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let limits = Arc::new(Limits::default());
        let visitor = quota(&limits, "scope-a", "10.0.0.3");
        let other = "The tooth is cleaned and shaped.";
        // One sentence is cached already, so only the other is synthesized.
        s.narration
            .clip("en", SENTENCE, Some(&visitor))
            .await
            .unwrap();
        let block = texts(&[SENTENCE, other]);
        let clip = s
            .narration
            .block("en", &block, Some(&visitor))
            .await
            .unwrap();
        assert_eq!(s.speech.calls(), 2);
        // Fake speech is 10 ms per character; then 250 ms of silence.
        let first_ms = u32::try_from(SENTENCE.chars().count() * 10).unwrap();
        let second_ms = u32::try_from(other.chars().count() * 10).unwrap();
        assert_eq!(clip.offsets_ms, [0, first_ms + PAUSE_MS]);
        assert_eq!(clip.duration_ms, first_ms + PAUSE_MS + second_ms);
        assert!(parse_wav(&clip.wav).is_some());

        // Replays are merged-cache hits: no call and no charge.
        for _ in 0..3 {
            let again = s
                .narration
                .block("en", &block, Some(&visitor))
                .await
                .unwrap();
            assert_eq!(again, clip);
        }
        assert_eq!(s.speech.calls(), 2);
        assert_eq!(block_rows(&s.db).await, 1);
        let used = charged_chars(SENTENCE) + charged_chars(other);
        let hour = Duration::from_secs(3600);
        assert!(
            limits
                .narration_scope
                .take(
                    "scope-a",
                    DEMO_SCOPE_CHARS_PER_HOUR - used,
                    DEMO_SCOPE_CHARS_PER_HOUR,
                    hour
                )
                .is_ok()
        );
        let (speech_calls,): (i64,) = sqlx::query_as("SELECT speech_calls FROM ai_usage")
            .fetch_one(&s.db)
            .await
            .unwrap();
        assert_eq!(speech_calls, 2);

        // The merged clip still plays when its sentence clips were removed.
        sqlx::query("DELETE FROM audio_clips WHERE model NOT LIKE 'block:%'")
            .execute(&s.db)
            .await
            .unwrap();
        assert_eq!(s.narration.block("en", &block, None).await.unwrap(), clip);
        assert_eq!(s.speech.calls(), 2);

        // With the quota used up, a new block is refused and nothing merged.
        let err = s
            .narration
            .block(
                "en",
                &texts(&[SENTENCE, "A sentence nobody heard yet."]),
                Some(&visitor),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "narration_quota");
        assert_eq!(block_rows(&s.db).await, 1);
    }

    #[tokio::test]
    async fn concurrent_block_requests_merge_once() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let block = texts(&["One.", "Two.", "Three.", "Four.", "Five."]);
        let calls = (0..4).map(|_| {
            let n = s.narration.clone();
            let block = block.clone();
            tokio::spawn(async move { n.block("en", &block, None).await.unwrap() })
        });
        let results = futures_join(calls).await;
        assert!(results.windows(2).all(|w| w[0] == w[1]));
        assert_eq!(results[0].offsets_ms.len(), 5);
        assert_eq!(s.speech.calls(), 5);
        assert_eq!(block_rows(&s.db).await, 1);
        assert!(s.narration.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn block_requests_are_checked() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let too_many: Vec<String> = (0..=MAX_NARRATION_SENTENCES)
            .map(|i| format!("Sentence {i}."))
            .collect();
        for bad in [Vec::new(), too_many, texts(&["Fine.", " "])] {
            let err = s.narration.block("en", &bad, None).await.unwrap_err();
            assert_eq!(err.status, StatusCode::BAD_REQUEST);
        }
        assert_eq!(s.speech.calls(), 0);
        let clinical = setup(Mode::Clinical, FakeSpeech::default(), None).await;
        let err = clinical
            .narration
            .block("en", &texts(&["Fine."]), None)
            .await
            .unwrap_err();
        assert_eq!(err.code, "narration_unavailable");
    }

    #[tokio::test]
    async fn quota_counts_only_synthesized_characters() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let limits = Arc::new(Limits::default());
        let visitor = quota(&limits, "scope-a", "10.0.0.1");
        s.narration
            .clip("en", SENTENCE, Some(&visitor))
            .await
            .unwrap();
        // Cache hits are free, so replaying never uses the quota up.
        for _ in 0..5 {
            s.narration
                .clip("en", SENTENCE, Some(&visitor))
                .await
                .unwrap();
        }
        let used = charged_chars(SENTENCE);
        let hour = Duration::from_secs(3600);
        // Exactly the one synthesized sentence was counted.
        let left = DEMO_SCOPE_CHARS_PER_HOUR - used;
        assert!(
            limits
                .narration_scope
                .take("scope-a", left, DEMO_SCOPE_CHARS_PER_HOUR, hour)
                .is_ok()
        );

        let err = s
            .narration
            .clip("en", "A new sentence to narrate.", Some(&visitor))
            .await
            .unwrap_err();
        assert_eq!(err.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(err.code, "narration_quota");
        assert!(err.retry_after.is_some_and(|w| (1..=3600).contains(&w)));
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 1);
        // The cached clip still plays with the quota used up.
        assert!(
            s.narration
                .clip("en", SENTENCE, Some(&visitor))
                .await
                .is_ok()
        );

        // Another visitor scope on the same network is held by the address
        // quota, and a refused scope charge is given back.
        let neighbour = quota(&limits, "scope-b", "10.0.0.1");
        limits
            .narration_address
            .take(
                "10.0.0.1",
                DEMO_ADDRESS_CHARS_PER_DAY - used,
                DEMO_ADDRESS_CHARS_PER_DAY,
                hour,
            )
            .unwrap();
        let err = s
            .narration
            .clip("en", "Yet another sentence for the demo.", Some(&neighbour))
            .await
            .unwrap_err();
        assert_eq!(err.code, "narration_quota");
        assert!(
            limits
                .narration_scope
                .take(
                    "scope-b",
                    DEMO_SCOPE_CHARS_PER_HOUR,
                    DEMO_SCOPE_CHARS_PER_HOUR,
                    hour
                )
                .is_ok()
        );
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_synthesis_gives_the_quota_back() {
        let fake = FakeSpeech {
            fail_first: 3,
            ..FakeSpeech::default()
        };
        let s = setup(Mode::Demo, fake, Some(25.0)).await;
        let limits = Arc::new(Limits::default());
        let visitor = quota(&limits, "scope", "10.0.0.2");
        assert!(
            s.narration
                .clip("en", SENTENCE, Some(&visitor))
                .await
                .is_err()
        );
        let hour = Duration::from_secs(3600);
        assert!(
            limits
                .narration_scope
                .take(
                    "scope",
                    DEMO_SCOPE_CHARS_PER_HOUR,
                    DEMO_SCOPE_CHARS_PER_HOUR,
                    hour
                )
                .is_ok()
        );
        assert!(
            limits
                .narration_address
                .take(
                    "10.0.0.2",
                    DEMO_ADDRESS_CHARS_PER_DAY,
                    DEMO_ADDRESS_CHARS_PER_DAY,
                    hour
                )
                .is_ok()
        );
    }

    #[tokio::test]
    async fn timeouts_are_billed_and_retried_once() {
        let fake = FakeSpeech {
            unusable_first: 1,
            ..FakeSpeech::default()
        };
        let s = setup(Mode::Demo, fake, Some(25.0)).await;
        let limits = Arc::new(Limits::default());
        let visitor = quota(&limits, "scope", "10.0.0.7");
        assert!(
            s.narration
                .clip("en", SENTENCE, Some(&visitor))
                .await
                .is_ok()
        );
        assert_eq!(s.speech.calls(), 2);
        // The timed-out call and the good one are both counted.
        let (cost, speech_calls): (f64, i64) =
            sqlx::query_as("SELECT speech_cost_usd, speech_calls FROM ai_usage")
                .fetch_one(&s.db)
                .await
                .unwrap();
        assert_eq!(speech_calls, 2);
        assert!((cost - 2.0 * speech_cost(SENTENCE)).abs() < 1e-12);

        // Two unusable replies: one retry, then failure. Both are counted,
        // and the quota is kept, since the provider may have billed them.
        let fake = FakeSpeech {
            unusable_first: 5,
            ..FakeSpeech::default()
        };
        let s = setup(Mode::Demo, fake, Some(25.0)).await;
        let err = s
            .narration
            .clip("en", SENTENCE, Some(&visitor))
            .await
            .unwrap_err();
        assert_eq!(err.code, "narration_failed");
        assert_eq!(s.speech.calls(), 2);
        let (speech_calls,): (i64,) = sqlx::query_as("SELECT speech_calls FROM ai_usage")
            .fetch_one(&s.db)
            .await
            .unwrap();
        assert_eq!(speech_calls, 2);
        let hour = Duration::from_secs(3600);
        let left = DEMO_SCOPE_CHARS_PER_HOUR - 2 * charged_chars(SENTENCE);
        assert!(
            limits
                .narration_scope
                .take("scope", left + 1, DEMO_SCOPE_CHARS_PER_HOUR, hour)
                .is_err()
        );
    }

    #[tokio::test]
    async fn short_sentences_are_charged_a_minimum() {
        assert_eq!(charged_chars("Yes."), MIN_CHARGED_CHARS);
        assert_eq!(charged_chars(&"a".repeat(80)), 80);
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let limits = Arc::new(Limits::default());
        let visitor = quota(&limits, "scope", "10.0.0.8");
        s.narration
            .clip("en", "Yes.", Some(&visitor))
            .await
            .unwrap();
        let hour = Duration::from_secs(3600);
        let left = DEMO_SCOPE_CHARS_PER_HOUR - MIN_CHARGED_CHARS;
        assert!(
            limits
                .narration_scope
                .take("scope", left + 1, DEMO_SCOPE_CHARS_PER_HOUR, hour)
                .is_err()
        );
        assert!(
            limits
                .narration_scope
                .take("scope", left, DEMO_SCOPE_CHARS_PER_HOUR, hour)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn one_address_gets_only_its_share_of_upstream_calls() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let limits = Arc::new(Limits::default());
        let greedy = quota(&limits, "scope-a", "10.0.0.9");
        for i in 0..UPSTREAM_PER_ADDRESS_PER_MINUTE {
            s.narration
                .clip("en", &format!("Sentence number {i}."), Some(&greedy))
                .await
                .unwrap();
        }
        let err = s
            .narration
            .clip("en", "One sentence too many.", Some(&greedy))
            .await
            .unwrap_err();
        assert_eq!(err.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(err.code, "narration_busy");
        assert!(err.retry_after.is_some());
        // The refused call is not charged to the quota.
        let hour = Duration::from_secs(3600);
        let used = UPSTREAM_PER_ADDRESS_PER_MINUTE * MIN_CHARGED_CHARS;
        assert!(
            limits
                .narration_scope
                .take(
                    "scope-a",
                    DEMO_SCOPE_CHARS_PER_HOUR - used,
                    DEMO_SCOPE_CHARS_PER_HOUR,
                    hour
                )
                .is_ok()
        );
        // Another network still gets through.
        let other = quota(&limits, "scope-b", "10.0.0.10");
        assert!(
            s.narration
                .clip("en", "One sentence too many.", Some(&other))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn quota_charges_follow_a_rotated_scope() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let limits = Arc::new(Limits::default());
        // A quota made before the token rotated, as the pre-warm task holds.
        let stale = quota(&limits, "old-token", "10.0.0.11");
        limits.transfer_scope("old-token", "new-token");
        s.narration
            .clip("en", SENTENCE, Some(&stale))
            .await
            .unwrap();
        let hour = Duration::from_secs(3600);
        let left = DEMO_SCOPE_CHARS_PER_HOUR - charged_chars(SENTENCE);
        assert!(
            limits
                .narration_scope
                .take("new-token", left + 1, DEMO_SCOPE_CHARS_PER_HOUR, hour)
                .is_err()
        );
        assert!(
            limits
                .narration_scope
                .take(
                    "old-token",
                    DEMO_SCOPE_CHARS_PER_HOUR,
                    DEMO_SCOPE_CHARS_PER_HOUR,
                    hour
                )
                .is_ok()
        );
    }

    #[test]
    fn wav_with_an_unsupported_rate_or_channel_count_is_refused() {
        let good = wav(10, 44_100, 1, 1);
        assert!(parse_wav(&good).is_some());
        let with = |rate: u32, channels: u16| {
            let mut bytes = good.clone();
            bytes[22..24].copy_from_slice(&channels.to_le_bytes());
            bytes[24..28].copy_from_slice(&rate.to_le_bytes());
            bytes
        };
        for (rate, channels) in [
            (0, 1),
            (7_999, 1),
            (48_001, 1),
            (u32::MAX, 1),
            (44_100, 0),
            (44_100, 3),
            (44_100, u16::MAX),
        ] {
            assert!(
                parse_wav(&with(rate, channels)).is_none(),
                "{rate} {channels}"
            );
        }
        assert!(parse_wav(&with(48_000, 2)).is_some());
    }

    #[tokio::test]
    async fn narration_stops_at_its_budget_share_and_generation_goes_on() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        s.narration.clip("en", SENTENCE, None).await.unwrap();
        let (cost, speech, speech_calls): (f64, f64, i64) =
            sqlx::query_as("SELECT cost_usd, speech_cost_usd, speech_calls FROM ai_usage")
                .fetch_one(&s.db)
                .await
                .unwrap();
        assert_eq!(speech_calls, 1);
        assert!((cost - speech_cost(SENTENCE)).abs() < 1e-12);
        assert!((speech - cost).abs() < 1e-12);

        // Narration has spent its 20% share of the $25 cap ($5); text has spent $1.
        sqlx::query("UPDATE ai_usage SET speech_cost_usd = 5.0, cost_usd = 6.0")
            .execute(&s.db)
            .await
            .unwrap();
        let err = s
            .narration
            .clip("en", "Another sentence.", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.code, "budget_reached");
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 1);
        // Cached clips still play, and generation is still allowed.
        assert!(s.narration.clip("en", SENTENCE, None).await.is_ok());
        assert!(s.narration.budget.allows_new_generation().await.unwrap());
        assert!(s.narration.budget.reserve(1.0).await.unwrap());
    }

    fn set_last_used(db: &SqlitePool, key: &str, at: i64) -> impl Future<Output = ()> {
        let db = db.clone();
        let key = key.to_owned();
        async move {
            sqlx::query("UPDATE audio_clips SET last_used_at = ? WHERE key = ?")
                .bind(at)
                .bind(&key)
                .execute(&db)
                .await
                .unwrap();
        }
    }

    async fn clip_rows(db: &SqlitePool) -> Vec<String> {
        sqlx::query_as::<_, (String,)>("SELECT key FROM audio_clips ORDER BY key")
            .fetch_all(db)
            .await
            .unwrap()
            .into_iter()
            .map(|(k,)| k)
            .collect()
    }

    #[tokio::test]
    async fn cleanup_deletes_stale_clips_orphans_and_keeps_the_size_cap() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        let texts = ["Old one.", "Recent one.", "Newest one."];
        let keys: Vec<String> = texts
            .iter()
            .map(|t| audio_key(s.narration.voice_id(), "en", t))
            .collect();
        for text in texts {
            s.narration.clip("en", text, None).await.unwrap();
        }
        let now = crate::sessions::now();
        set_last_used(&s.db, &keys[0], now - DEMO_CLIP_RETENTION_SECS - 1).await;
        set_last_used(&s.db, &keys[1], now - 60).await;
        set_last_used(&s.db, &keys[2], now).await;
        let file = |key: &str| s._dir.0.join(format!("{key}.wav"));

        // Files that are not cache clips are never touched; an old clip file
        // with no row is removed.
        let orphan = s._dir.0.join(format!("{}.mp3", "ab".repeat(32)));
        let other = s._dir.0.join("notes.txt");
        let odd = s._dir.0.join(format!("{}.wav", "zz".repeat(32)));
        for path in [&orphan, &other, &odd] {
            std::fs::write(path, b"x").unwrap();
            let old = std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }

        let report = s
            .narration
            .collect_garbage_at(now, MAX_CACHE_BYTES)
            .await
            .unwrap();
        assert_eq!(
            report,
            CleanupReport {
                stale: 1,
                over_cap: 0,
                orphans: 1,
            }
        );
        assert_eq!(clip_rows(&s.db).await, {
            let mut k = vec![keys[1].clone(), keys[2].clone()];
            k.sort();
            k
        });
        assert!(!file(&keys[0]).exists());
        assert!(file(&keys[1]).exists());
        assert!(!orphan.exists());
        assert!(other.exists() && odd.exists());

        // Over the size cap, the least recently played clip goes first.
        let newest_bytes = std::fs::metadata(file(&keys[2])).unwrap().len();
        let report = s
            .narration
            .collect_garbage_at(now, i64::try_from(newest_bytes).unwrap())
            .await
            .unwrap();
        assert_eq!(report.over_cap, 1);
        assert_eq!(clip_rows(&s.db).await, vec![keys[2].clone()]);
        assert!(!file(&keys[1]).exists());
        assert!(file(&keys[2]).exists());

        // A removed clip is synthesized again on the next play.
        s.narration.clip("en", texts[0], None).await.unwrap();
        assert_eq!(s.speech.calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn clinical_clips_are_kept_for_30_days() {
        let s = setup(Mode::Demo, FakeSpeech::default(), Some(25.0)).await;
        s.narration.clip("en", SENTENCE, None).await.unwrap();
        let clinical = Narration::new(
            Mode::Clinical,
            s.db.clone(),
            s._dir.0.clone(),
            None,
            "test/voice-model".into(),
            "calm".into(),
            Arc::new(Budget::new(s.db.clone(), None)),
        );
        let now = crate::sessions::now();
        let two_days = now + 2 * 24 * 60 * 60;
        assert_eq!(
            clinical
                .collect_garbage_at(two_days, MAX_CACHE_BYTES)
                .await
                .unwrap(),
            CleanupReport::default()
        );
        let later = now + CLINICAL_CLIP_RETENTION_SECS + 1;
        let report = clinical
            .collect_garbage_at(later, MAX_CACHE_BYTES)
            .await
            .unwrap();
        assert_eq!(report.stale, 1);
        assert!(clip_rows(&s.db).await.is_empty());
    }

    #[test]
    fn only_cache_file_names_are_recognized() {
        let key = "0f".repeat(32);
        assert_eq!(clip_file_key(&format!("{key}.wav")), Some(key.as_str()));
        assert_eq!(clip_file_key(&format!("{key}.part")), Some(key.as_str()));
        assert_eq!(clip_file_key(&format!("{key}.txt")), None);
        assert_eq!(clip_file_key(&format!("{key}.wav.bak")), None);
        assert_eq!(clip_file_key(&format!("../{key}.wav")), None);
        assert_eq!(clip_file_key("dentalclarity.db"), None);
    }
}
