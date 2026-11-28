//! Language and reading level variants of the patient screens
//! (docs/DECISIONS.md E23, docs/AI_PIPELINE.md S9).
//!
//! Every variant is derived from the approved English text of a ready
//! section ([`super::present::approved`]), one section per derivation, and
//! cached in `section_variants` per presentation version, language, level and
//! source hash. A section that is not derived yet, or failed its checks, is
//! shown in English with its state, never mixed with the other language.
//!
//! `POST /api/present/variant` starts derivations; `GET /api/present/content`
//! only reads. Both take the presentation and version from the present
//! session alone.

use super::present::{Approved, ApprovedBlock, ApprovedSection, approved, prewarm_blocks, render};
use super::presentations::{client_key, provider_not_configured};
use crate::http::{ClientIp, Failure, JsonBody};
use crate::security::CurrentPresent;
use crate::{AppState, jobs};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use dc_core::Mode;
use dc_core::content::ReadingLevel;
use dc_core::language::{self, LANGUAGES, Language, SOURCE_LANGUAGE};
use dc_core::present::{PatientView, VariantRequest, VariantState};
use dc_generation::variant::{self, Outcome, Target, VariantBlock, VariantText};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;

/// New variants a demo visitor may start per hour.
pub const DEMO_VARIANTS_PER_VISITOR_PER_HOUR: i64 = 10;
/// New variants one network may start per day.
pub const DEMO_VARIANTS_PER_ADDRESS_PER_DAY: i64 = 30;
/// A pending row older than this belongs to a derivation that did not
/// finish (a restart), and may be taken over.
const STALE_PENDING_MS: i64 = 10 * 60 * 1000;
/// Sections of one variant derived at the same time.
const PARALLEL_SECTIONS: usize = 4;

/// A language and reading level to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Choice {
    pub language: &'static Language,
    pub level: ReadingLevel,
}

fn english() -> &'static Language {
    language::find(SOURCE_LANGUAGE).expect("English is listed")
}

/// The presentation's own language and level. A language that is not
/// offered falls back to English.
fn default_choice(approved: &Approved) -> Choice {
    Choice {
        language: language::find(&approved.language).unwrap_or_else(english),
        level: approved.level,
    }
}

/// The approved English itself: nothing to derive.
fn is_source(approved: &Approved, choice: Choice) -> bool {
    choice.language.tag == SOURCE_LANGUAGE && choice.level == approved.level
}

fn unsupported_language() -> Failure {
    Failure::new(
        StatusCode::BAD_REQUEST,
        "unsupported_language",
        "That language is not offered.",
    )
}

fn parse_choice(language: &str, level: ReadingLevel) -> Result<Choice, Failure> {
    let language = LANGUAGES
        .iter()
        .find(|l| l.tag == language)
        .ok_or_else(unsupported_language)?;
    Ok(Choice { language, level })
}

/// `language=<tag>&level=<level>` from a query string. Both are needed when
/// either is given; anything else in the query is refused.
pub(super) fn parse_query(query: Option<&str>) -> Result<Option<Choice>, Failure> {
    let Some(query) = query.filter(|q| !q.is_empty()) else {
        return Ok(None);
    };
    let bad = || {
        Failure::new(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Give both language and level, or neither.",
        )
    };
    let mut language = None;
    let mut level = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("language", v)) if language.is_none() => language = Some(v),
            Some(("level", v)) if level.is_none() => {
                level = Some(match v {
                    "simple" => ReadingLevel::Simple,
                    "standard" => ReadingLevel::Standard,
                    "detailed" => ReadingLevel::Detailed,
                    _ => return Err(bad()),
                });
            }
            _ => return Err(bad()),
        }
    }
    match (language, level) {
        (Some(language), Some(level)) => parse_choice(language, level).map(Some),
        _ => Err(bad()),
    }
}

/// The approved text of a section as one translator request.
fn source_text(section: &ApprovedSection) -> VariantText {
    let mut blocks: Vec<VariantBlock> = section
        .blocks
        .iter()
        .map(|b| VariantBlock {
            block_id: b.field.clone(),
            heading: b.heading.clone(),
            sentences: b.sentences.clone(),
        })
        .collect();
    if !section.questions.is_empty() {
        blocks.push(VariantBlock {
            block_id: variant::QUESTIONS_BLOCK.into(),
            heading: String::new(),
            sentences: section.questions.clone(),
        });
    }
    VariantText { blocks }
}

/// `section` with its text replaced by the derived `text`, or `None` when the
/// stored text does not fit the section (it was checked against this source,
/// so that means a corrupt row).
fn apply(section: &ApprovedSection, text: &VariantText) -> Option<ApprovedSection> {
    let source = source_text(section);
    if text.blocks.len() != source.blocks.len() {
        return None;
    }
    let mut out = section.clone();
    out.blocks = section
        .blocks
        .iter()
        .zip(&text.blocks)
        .map(|(b, t)| {
            (t.block_id == b.field && t.sentences.len() == b.sentences.len()).then(|| {
                ApprovedBlock {
                    heading: t.heading.clone(),
                    sentences: t.sentences.clone(),
                    ..b.clone()
                }
            })
        })
        .collect::<Option<_>>()?;
    if !section.questions.is_empty() {
        let questions = text.blocks.last()?;
        if questions.block_id != variant::QUESTIONS_BLOCK {
            return None;
        }
        out.questions = questions.sentences.clone();
    }
    Some(out)
}

fn target<'a>(approved: &Approved, choice: Choice) -> Target<'a> {
    Target {
        language: choice.language.tag,
        language_name: choice.language.english,
        level: choice.level,
        source_level: approved.level,
        guardian_audience: approved.sheet.guardian_audience,
    }
}

type Row = (String, String, String, Option<String>, i64);

/// Cached rows of one variant: (section key, source hash, state, output, updated at).
async fn rows(
    state: &AppState,
    id: &str,
    version: i64,
    choice: Choice,
) -> Result<Vec<Row>, Failure> {
    Ok(sqlx::query_as(
        "SELECT section_key, source_hash, state, output_json, updated_at FROM section_variants
         WHERE presentation_id = ? AND version = ? AND language = ? AND reading_level = ?",
    )
    .bind(id)
    .bind(version)
    .bind(choice.language.tag)
    .bind(choice.level.to_string())
    .fetch_all(&state.db)
    .await?)
}

/// The view of `approved` in `choice`, from the cache only.
async fn build(
    state: &AppState,
    id: &str,
    version: i64,
    approved: &Approved,
    choice: Choice,
) -> Result<PatientView, Failure> {
    let render_as = |sections| render(state, approved, choice.language.tag, choice.level, sections);
    if is_source(approved, choice) {
        let sections = approved
            .sections
            .iter()
            .map(|s| (s.clone(), VariantState::Ready))
            .collect();
        return Ok(render_as(sections));
    }
    let Some(generation) = &state.generation else {
        let sections = approved
            .sections
            .iter()
            .map(|s| (s.clone(), VariantState::Unavailable))
            .collect();
        return Ok(render_as(sections));
    };
    let target = target(approved, choice);
    let cached: HashMap<(String, String), (String, Option<String>, i64)> =
        rows(state, id, version, choice)
            .await?
            .into_iter()
            .map(|(key, hash, st, out, at)| ((key, hash), (st, out, at)))
            .collect();
    let now = jobs::now_ms();
    let sections = approved
        .sections
        .iter()
        .map(|section| {
            let hash = variant::cache_hash(&source_text(section), &target, &generation.variants);
            match cached.get(&(section.key.clone(), hash)) {
                Some((st, Some(json), _)) if st == "ready" => {
                    match serde_json::from_str(json)
                        .ok()
                        .and_then(|t| apply(section, &t))
                    {
                        Some(derived) => (derived, VariantState::Ready),
                        None => (section.clone(), VariantState::Unavailable),
                    }
                }
                Some((st, _, at)) if st == "pending" && now - at < STALE_PENDING_MS => {
                    (section.clone(), VariantState::Pending)
                }
                _ => (section.clone(), VariantState::Unavailable),
            }
        })
        .collect();
    Ok(render_as(sections))
}

/// The view of the bound version in `choice`, or in the presentation's own
/// language and level, from the cache only.
pub(super) async fn view(
    state: &AppState,
    id: &str,
    version: i64,
    choice: Option<Choice>,
) -> Result<PatientView, Failure> {
    let approved = approved(state, id, version).await?;
    let choice = choice.unwrap_or_else(|| default_choice(&approved));
    build(state, id, version, &approved, choice).await
}

/// Every view of the bound version a patient may have been shown: the
/// default one and each variant with cached sections. Narration keys are
/// looked up here, so only text of this version can be narrated.
pub(super) async fn all_views(
    state: &AppState,
    id: &str,
    version: i64,
) -> Result<Vec<PatientView>, Failure> {
    let approved = approved(state, id, version).await?;
    let mut choices = vec![default_choice(&approved)];
    let source = Choice {
        language: english(),
        level: approved.level,
    };
    if !choices.contains(&source) {
        choices.push(source);
    }
    let cached: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT language, reading_level FROM section_variants
         WHERE presentation_id = ? AND version = ? AND state = 'ready'",
    )
    .bind(id)
    .bind(version)
    .fetch_all(&state.db)
    .await?;
    for (language, level) in cached {
        if let Ok(choice) = parse_choice(&language, super::presentations::parse_level(&level))
            && !choices.contains(&choice)
        {
            choices.push(choice);
        }
    }
    let mut views = Vec::with_capacity(choices.len());
    for choice in choices {
        views.push(build(state, id, version, &approved, choice).await?);
    }
    Ok(views)
}

/// Who asked for a variant, for the demo limits and the narration pre-warm.
pub(super) struct Requester {
    pub owner_scope: String,
    pub client_key: String,
    /// Pre-warm narration for the variant once it is derived.
    pub prewarm: Option<Option<crate::narration::Quota>>,
}

/// Starts deriving the sections of `choice` that have no usable row. Returns
/// whether any derivation started. Refuses without a provider, and past the
/// demo limits.
pub(super) async fn start(
    state: &AppState,
    id: &str,
    version: i64,
    choice: Option<Choice>,
    requester: Requester,
) -> Result<bool, Failure> {
    let approved = approved(state, id, version).await?;
    let choice = choice.unwrap_or_else(|| default_choice(&approved));
    if is_source(&approved, choice) || approved.sections.is_empty() {
        return Ok(false);
    }
    let generation = state
        .generation
        .clone()
        .ok_or_else(|| provider_not_configured(state))?;
    let goal = target(&approved, choice);
    let cached: HashMap<(String, String), (String, i64)> = rows(state, id, version, choice)
        .await?
        .into_iter()
        .map(|(key, hash, st, _, at)| ((key, hash), (st, at)))
        .collect();
    let now = jobs::now_ms();
    let wanted: Vec<(ApprovedSection, VariantText, String)> = approved
        .sections
        .iter()
        .filter_map(|section| {
            let text = source_text(section);
            let hash = variant::cache_hash(&text, &goal, &generation.variants);
            let usable = match cached.get(&(section.key.clone(), hash.clone())) {
                Some((st, at)) if st == "pending" => now - at < STALE_PENDING_MS,
                Some(_) => true,
                None => false,
            };
            (!usable).then(|| (section.clone(), text, hash))
        })
        .collect();
    if wanted.is_empty() {
        return Ok(false);
    }
    if state.mode == Mode::Demo {
        check_demo_limits(state, &requester.owner_scope, &requester.client_key, now).await?;
    }

    // Claim each section, so two requests never derive the same one.
    let mut claimed = Vec::new();
    for (section, text, hash) in wanted {
        let result = sqlx::query(
            "INSERT INTO section_variants
                 (presentation_id, version, section_key, language, reading_level, source_hash,
                  state, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, 'pending', ?)
             ON CONFLICT (presentation_id, version, section_key, language, reading_level,
                          source_hash)
             DO UPDATE SET updated_at = excluded.updated_at
             WHERE section_variants.state = 'pending' AND section_variants.updated_at < ?",
        )
        .bind(id)
        .bind(version)
        .bind(&section.key)
        .bind(choice.language.tag)
        .bind(choice.level.to_string())
        .bind(&hash)
        .bind(now)
        .bind(now - STALE_PENDING_MS)
        .execute(&state.db)
        .await?;
        if result.rows_affected() == 1 {
            claimed.push((section, text, hash));
        }
    }
    if claimed.is_empty() {
        return Ok(false);
    }
    if state.mode == Mode::Demo {
        sqlx::query(
            "INSERT INTO variant_events (owner_scope, client_key, created_at) VALUES (?, ?, ?)",
        )
        .bind(&requester.owner_scope)
        .bind(&requester.client_key)
        .bind(now)
        .execute(&state.db)
        .await?;
    }

    let state = state.clone();
    let id = id.to_owned();
    tokio::spawn(async move {
        let approved = Arc::new(approved);
        futures::stream::iter(claimed)
            .for_each_concurrent(PARALLEL_SECTIONS, |(section, text, hash)| {
                let state = &state;
                let approved = approved.clone();
                let generation = generation.clone();
                let id = &id;
                async move {
                    let target = target(&approved, choice);
                    let derived =
                        variant::derive(&generation.variants, &text, &target, &approved.sheet)
                            .await;
                    if let Err(err) =
                        store(state, id, version, choice, &section.key, &hash, derived).await
                    {
                        tracing::warn!(%err, "could not store a derived section");
                    }
                }
            })
            .await;
        if let Some(quota) = requester.prewarm {
            match build(&state, &id, version, &approved, choice).await {
                Ok(view) => {
                    crate::narration::prewarm(&state, &view.language, prewarm_blocks(&view), quota);
                }
                Err(failure) => tracing::debug!(code = failure.code, "narration pre-warm skipped"),
            }
        }
    });
    Ok(true)
}

/// Writes one derivation's result. A model that could not be reached leaves
/// no row, so a later request tries again.
async fn store(
    state: &AppState,
    id: &str,
    version: i64,
    choice: Choice,
    section_key: &str,
    hash: &str,
    derived: variant::Derived,
) -> anyhow::Result<()> {
    let level = choice.level.to_string();
    let (state_text, output, notes) = match derived.outcome {
        Outcome::Ready { text, notes } => ("ready", Some(serde_json::to_string(&text)?), notes),
        Outcome::Failed { reasons } => {
            tracing::info!(
                section_key,
                language = choice.language.tag,
                "variant failed its checks"
            );
            ("failed", None, reasons)
        }
        Outcome::Unavailable { reason } => {
            tracing::warn!(section_key, %reason, "variant not derived");
            sqlx::query(
                "DELETE FROM section_variants
                 WHERE presentation_id = ? AND version = ? AND section_key = ?
                     AND language = ? AND reading_level = ? AND source_hash = ?",
            )
            .bind(id)
            .bind(version)
            .bind(section_key)
            .bind(choice.language.tag)
            .bind(&level)
            .bind(hash)
            .execute(&state.db)
            .await?;
            return Ok(());
        }
    };
    let generation = state.generation.as_ref();
    let model = generation
        .map(|g| g.variants.translator_model.clone())
        .unwrap_or_default();
    sqlx::query(
        "UPDATE section_variants SET state = ?, output_json = ?, notes_json = ?,
             translator_model = ?, prompt_version = ?, attempts = ?, input_tokens = ?,
             output_tokens = ?, cost_usd = ?, updated_at = ?
         WHERE presentation_id = ? AND version = ? AND section_key = ?
             AND language = ? AND reading_level = ? AND source_hash = ?",
    )
    .bind(state_text)
    .bind(output)
    .bind(serde_json::to_string(&notes)?)
    .bind(model)
    .bind(variant::translator_version())
    .bind(i64::from(derived.attempts))
    .bind(i64::from(derived.usage.input_tokens))
    .bind(i64::from(derived.usage.output_tokens))
    .bind(crate::generation::usage_cost(&derived.usage))
    .bind(jobs::now_ms())
    .bind(id)
    .bind(version)
    .bind(section_key)
    .bind(choice.language.tag)
    .bind(&level)
    .bind(hash)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Demo limits on new variants, counted from `variant_events` so they
/// survive a restart. Variants already cached are free.
async fn check_demo_limits(
    state: &AppState,
    owner: &str,
    client_key: &str,
    now: i64,
) -> Result<(), Failure> {
    let hour = 60 * 60 * 1000;
    let day = jobs::DEMO_RETENTION_SECS * 1000;
    let limited = |oldest: Option<i64>, window: i64, message: &str| Failure {
        retry_after: Some(
            u64::try_from((oldest.unwrap_or(now) + window - now) / 1000)
                .unwrap_or(1)
                .max(1),
        ),
        ..Failure::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message)
    };
    let (n, oldest): (i64, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(*), MIN(created_at) FROM variant_events
         WHERE owner_scope = ? AND created_at > ?",
    )
    .bind(owner)
    .bind(now - hour)
    .fetch_one(&state.db)
    .await?;
    if n >= DEMO_VARIANTS_PER_VISITOR_PER_HOUR {
        return Err(limited(
            oldest,
            hour,
            "The demo prepares up to 10 new languages or reading levels per hour. Try again later.",
        ));
    }
    let (n, oldest): (i64, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(*), MIN(created_at) FROM variant_events
         WHERE client_key = ? AND created_at > ?",
    )
    .bind(client_key)
    .bind(now - day)
    .fetch_one(&state.db)
    .await?;
    if n >= DEMO_VARIANTS_PER_ADDRESS_PER_DAY {
        return Err(limited(
            oldest,
            day,
            "This network has prepared the most languages and reading levels the demo allows today.",
        ));
    }
    Ok(())
}

/// `POST /api/present/variant`: starts preparing the requested language and
/// reading level for the present session's version and returns the view as
/// it stands. Sections still being prepared are `pending`.
pub async fn request(
    State(state): State<AppState>,
    CurrentPresent(present): CurrentPresent,
    ClientIp(ip): ClientIp,
    JsonBody(request): JsonBody<VariantRequest>,
) -> Result<Json<PatientView>, Failure> {
    let choice = parse_choice(&request.language, request.reading_level)?;
    let requester = Requester {
        // Demo presentations are owned by the scope of the session that
        // started Present mode.
        owner_scope: present.staff_token_hash.clone(),
        client_key: client_key(&state, ip),
        prewarm: None,
    };
    start(
        &state,
        &present.presentation_id,
        present.version,
        Some(choice),
        requester,
    )
    .await?;
    Ok(Json(
        view(
            &state,
            &present.presentation_id,
            present.version,
            Some(choice),
        )
        .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_name_both_language_and_level() {
        assert_eq!(parse_query(None).unwrap(), None);
        assert_eq!(parse_query(Some("")).unwrap(), None);
        let choice = parse_query(Some("language=es&level=simple"))
            .unwrap()
            .unwrap();
        assert_eq!(choice.language.tag, "es");
        assert_eq!(choice.level, ReadingLevel::Simple);
        for bad in [
            "language=es",
            "level=simple",
            "language=es&level=easy",
            "language=es&level=simple&id=other",
            "language=es&language=fr&level=simple",
        ] {
            assert_eq!(
                parse_query(Some(bad)).unwrap_err().code,
                "bad_request",
                "{bad}"
            );
        }
        assert_eq!(
            parse_query(Some("language=xx&level=simple"))
                .unwrap_err()
                .code,
            "unsupported_language"
        );
    }
}
