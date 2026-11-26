//! Present mode: starting it from the staff workspace, the patient content
//! and narration routes under `/api/present`, and exit with the staff PIN.
//!
//! `/api/present/*` reads the presentation and version only from the present
//! session (docs/SECURITY.md, Present-mode isolation). The present session
//! has no CSRF token; writes rely on the `SameSite=Strict` cookie, the Origin
//! check and a JSON body.

use super::presentations::{
    MAX_ID_CHARS, load_sections, not_found, owner_scope, parse_level, procedure_facts,
};
use super::staff::load_settings;
use crate::credentials::verify_secret;
use crate::http::{
    ClientIp, Failure, JsonBody, clear_present_cookie, present_cookie, session_cookie,
};
use crate::limits::ip_key;
use crate::security::{CurrentPresent, CurrentSession};
use crate::sessions::{self, MAX_PIN_ATTEMPTS, PresentSession, Session, SessionKind};
use crate::{AppState, audit, jobs};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use dc_core::Mode;
use dc_core::plan::PlanSnapshot;
use dc_core::present::{
    ExitPresentRequest, ExitPresentResponse, PatientBlock, PatientSection, PatientSectionKind,
    PatientSentence, PatientView, PracticeBrand, StartPresentResponse, audio_key,
};
use dc_core::presentation::{BlockSource, SectionVisibility, TextBlock};
use dc_generation::facts::FactSheet;
use dc_knowledge::entries::Topic;
use std::net::IpAddr;
use std::time::Duration;

/// Shown in place of `{{provider_display_name}}` when the plan has no provider.
const NO_PROVIDER: &str = "your dentist";

pub(super) fn actor(session_kind: SessionKind) -> &'static str {
    match session_kind {
        SessionKind::Staff => "staff",
        SessionKind::Visitor => "visitor",
    }
}

fn conflict(code: &'static str, message: &'static str) -> Failure {
    Failure::new(StatusCode::CONFLICT, code, message)
}

/// `POST /api/staff/presentations/{id}/present`: locks the caller's session,
/// starts a present session for this presentation version and sets its cookie.
pub async fn start(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    ClientIp(ip): ClientIp,
    Path(id): Path<String>,
) -> Result<Response, Failure> {
    if id.len() > MAX_ID_CHARS {
        return Err(not_found("presentation"));
    }
    let version = presentable_version(&state, &session, &id).await?;
    let token = sessions::start_present(&state.db, &session, &id, version).await?;
    let quota = crate::narration::quota(&state, &owner_scope(&state, &session), ip);
    prewarm_narration(&state, &id, version, quota).await;
    audit::record(
        &state.db,
        actor(session.kind),
        "present_started",
        &format!("presentation {id} version {version}"),
    )
    .await?;
    Ok((
        [(header::SET_COOKIE, present_cookie(&state.http, &token))],
        Json(StartPresentResponse {
            present_path: "/present".into(),
        }),
    )
        .into_response())
}

/// Starts filling the narration cache for the generated sentences patients
/// will see, so the first play is quick. Staff-edited text is narrated only
/// when a patient plays it, so edits cannot be turned into bulk synthesis.
/// Failures only cost a slower first play.
async fn prewarm_narration(
    state: &AppState,
    id: &str,
    version: i64,
    quota: Option<crate::narration::Quota>,
) {
    match patient_view(state, id, version).await {
        Ok(view) => {
            crate::narration::prewarm(state, &view.language, prewarm_texts(&view), quota);
        }
        Err(failure) => tracing::debug!(code = failure.code, "narration pre-warm skipped"),
    }
}

/// The sentences worth narrating ahead of time: those of generated blocks.
pub(super) fn prewarm_texts(view: &PatientView) -> Vec<String> {
    view.sections
        .iter()
        .flat_map(|s| &s.blocks)
        .filter(|b| b.source == BlockSource::Generated)
        .flat_map(|b| &b.sentences)
        .map(|s| s.text.clone())
        .collect()
}

/// The version to present, if the presentation is in scope, its job has
/// finished and at least one section is ready for patients.
async fn presentable_version(
    state: &AppState,
    session: &Session,
    id: &str,
) -> Result<i64, Failure> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT version FROM presentations WHERE id = ? AND owner_scope = ?")
            .bind(id)
            .bind(owner_scope(state, session))
            .fetch_optional(&state.db)
            .await?;
    let Some((version,)) = row else {
        return Err(not_found("presentation"));
    };
    let job: Option<(String,)> = sqlx::query_as(
        "SELECT state FROM jobs WHERE presentation_id = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    if !job.is_some_and(|(s,)| jobs::parse_job_state(&s).is_finished()) {
        return Err(conflict(
            "not_ready",
            "This presentation is still being written.",
        ));
    }
    let (ready,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM presentation_sections WHERE presentation_id = ? AND state = 'passed'",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if ready == 0 {
        return Err(conflict(
            "nothing_to_present",
            "No section of this presentation is ready for patients yet.",
        ));
    }
    Ok(version)
}

/// `GET /api/present/content`
pub async fn content(
    State(state): State<AppState>,
    CurrentPresent(present): CurrentPresent,
) -> Result<Json<PatientView>, Failure> {
    Ok(Json(
        patient_view(&state, &present.presentation_id, present.version).await?,
    ))
}

/// `GET /api/present/audio/{key}`: narration for one sentence of the bound
/// version. Keys that are not in that version answer 404.
pub async fn audio(
    State(state): State<AppState>,
    CurrentPresent(present): CurrentPresent,
    ClientIp(ip): ClientIp,
    Path(key): Path<String>,
) -> Result<Response, Failure> {
    let unknown = || {
        Failure::new(
            StatusCode::NOT_FOUND,
            "audio_not_found",
            "No narration with that key belongs to this presentation.",
        )
    };
    if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(unknown());
    }
    let view = patient_view(&state, &present.presentation_id, present.version).await?;
    let text = view
        .sections
        .iter()
        .flat_map(|s| &s.blocks)
        .flat_map(|b| &b.sentences)
        .find(|s| s.audio_key == key)
        .map(|s| s.text.clone())
        .ok_or_else(unknown)?;
    // Demo presentations are owned by the scope of the session that started
    // Present mode.
    let quota = crate::narration::quota(&state, &present.staff_token_hash, ip);
    let (mime, bytes) =
        crate::narration::clip(&state, &view.language, &text, quota.as_ref()).await?;
    Ok(([(header::CONTENT_TYPE, mime)], bytes).into_response())
}

/// Builds what the patient may see for the present session's version: only
/// ready sections, staff text where it exists, and note-derived blocks only
/// after staff confirmed them.
pub(super) async fn patient_view(
    state: &AppState,
    id: &str,
    bound_version: i64,
) -> Result<PatientView, Failure> {
    let row: Option<(String, i64, String)> = sqlx::query_as(
        "SELECT reading_level, version, snapshot_json FROM presentations WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some((level, version, snapshot)) = row else {
        return Err(not_found("presentation"));
    };
    if version != bound_version {
        return Err(conflict(
            "version_changed",
            "This presentation has changed. Ask your dental team to open it again.",
        ));
    }
    let snapshot: PlanSnapshot = serde_json::from_str(&snapshot).map_err(Failure::internal)?;
    let sheet = FactSheet::build(&snapshot, &state.kb);
    let settings = load_settings(state).await?;
    let brand = PracticeBrand {
        name: settings.practice_name,
        primary: settings.brand_primary,
        highlight: settings.brand_highlight,
    };
    // Content is English until translations land (M7).
    let language = "en".to_owned();
    let fill = Filler {
        practice: &brand.name,
        provider: snapshot.provider_display_name.as_deref(),
        voice: crate::narration::speech_model_id(state),
        language: &language,
    };

    let sections = load_sections(state, id, version, &sheet)
        .await?
        .into_iter()
        .filter(|s| s.visibility == SectionVisibility::Ready)
        .map(|section| {
            let kind = match section.key.as_str() {
                "at_a_glance" => PatientSectionKind::AtAGlance,
                "journey" => PatientSectionKind::Journey,
                "next_steps" => PatientSectionKind::NextSteps,
                _ => PatientSectionKind::Procedure,
            };
            let blocks = section
                .blocks
                .iter()
                .filter_map(|block| {
                    patient_block(
                        state,
                        &sheet,
                        section.procedure_ref.as_deref(),
                        block,
                        &fill,
                    )
                })
                .collect();
            PatientSection {
                kind,
                title: section.title,
                procedure_ref: section.procedure_ref,
                blocks,
                questions: section.questions.iter().map(|q| fill.text(q)).collect(),
                key: section.key,
            }
        })
        .collect();

    Ok(PatientView {
        brand,
        patient_first_name: snapshot.patient.first_name.clone(),
        guardian_audience: sheet.guardian_audience,
        provider_display_name: snapshot.provider_display_name.clone(),
        language,
        reading_level: parse_level(&level),
        available_languages: vec!["en".into()],
        sections,
        procedures: procedure_facts(&snapshot, &sheet),
        affected_teeth: snapshot
            .affected_teeth()
            .iter()
            .map(ToString::to_string)
            .collect(),
        total_fee: snapshot.total_fee(),
        total_insurance_estimate: snapshot
            .procedures
            .iter()
            .map(|p| p.insurance_estimate.known())
            .sum(),
        total_patient_estimate: snapshot.total_patient_estimate(),
        demo: state.mode == Mode::Demo,
    })
}

/// One block as the patient sees it, or `None` when it must not be shown.
/// An unconfirmed documented reason is replaced by the knowledge base's
/// common reasons for the procedure's category (P11); an unconfirmed
/// objective is left out.
fn patient_block(
    state: &AppState,
    sheet: &FactSheet,
    procedure_ref: Option<&str>,
    block: &TextBlock,
    fill: &Filler,
) -> Option<PatientBlock> {
    if !block.needs_confirmation {
        return Some(PatientBlock {
            field: block.field.clone(),
            heading: block.heading.clone(),
            sentences: block.sentences.iter().map(|s| fill.sentence(s)).collect(),
            source: block.source,
        });
    }
    if block.field != "why" {
        return None;
    }
    let category = sheet.procedure(procedure_ref?)?.category;
    let passage = state.kb.entry(category)?.passage(Topic::CommonReasons)?;
    let sentences: Vec<PatientSentence> = split_sentences(&passage.text)
        .iter()
        .map(|s| fill.sentence(s))
        .collect();
    (!sentences.is_empty()).then(|| PatientBlock {
        field: block.field.clone(),
        heading: "Why this is done".into(),
        sentences,
        source: BlockSource::Generated,
    })
}

/// Local placeholder substitution and audio keys for patient text.
struct Filler<'a> {
    practice: &'a str,
    provider: Option<&'a str>,
    /// Speech identity hashed into audio keys, from the narration service.
    voice: &'a str,
    language: &'a str,
}

impl Filler<'_> {
    fn text(&self, text: &str) -> String {
        const PROVIDER: &str = "{{provider_display_name}}";
        let mut text = text.replace("{{practice_name}}", self.practice);
        match self.provider {
            Some(name) => text = text.replace(PROVIDER, name),
            None => {
                if let Some(rest) = text.strip_prefix(PROVIDER) {
                    text = format!("Your dentist{rest}");
                }
                text = text.replace(PROVIDER, NO_PROVIDER);
            }
        }
        text
    }

    fn sentence(&self, text: &str) -> PatientSentence {
        let text = self.text(text);
        PatientSentence {
            audio_key: audio_key(self.voice, self.language, &text),
            text,
        }
    }
}

/// Splits knowledge base prose into sentences at `.`, `!` or `?` followed by
/// a space and a capital letter, keeping abbreviations such as "Dr." intact.
pub(super) fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences = Vec::new();
    let mut start = 0;
    for i in 0..chars.len().saturating_sub(2) {
        if matches!(chars[i], '.' | '!' | '?') && chars[i + 1] == ' ' && chars[i + 2].is_uppercase()
        {
            let word_start = chars[..i]
                .iter()
                .rposition(|c| *c == ' ')
                .map_or(0, |p| p + 1);
            let word: String = chars[word_start..i].iter().collect();
            if matches!(word.as_str(), "Dr" | "Mr" | "Mrs" | "Ms" | "e.g" | "i.e") {
                continue;
            }
            sentences.push(chars[start..=i].iter().collect::<String>());
            start = i + 2;
        }
    }
    sentences.push(chars[start..].iter().collect::<String>());
    sentences
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

// Exit -----------------------------------------------------------------------

/// `POST /api/present/exit`: ends the present session and unlocks the
/// session that started it with a new token and CSRF token. Clinical mode
/// needs the staff PIN, or the staff password after five PIN tries.
pub async fn exit(
    State(state): State<AppState>,
    CurrentPresent(present): CurrentPresent,
    ClientIp(ip): ClientIp,
    JsonBody(request): JsonBody<ExitPresentRequest>,
) -> Result<Response, Failure> {
    let minute = Duration::from_secs(60);
    state
        .limits
        .present_exit
        .check(&present.token_hash, 10, minute)
        .map_err(Failure::too_many)?;
    state
        .limits
        .present_exit_ip
        .check(&ip_key(ip), 30, minute)
        .map_err(Failure::too_many)?;

    let method = match state.mode {
        Mode::Demo => "demo",
        Mode::Clinical => check_unlock(&state, &present, ip, request).await?,
    };
    let actor = actor(present.staff_kind);
    let Some(issued) = sessions::end_present(&state.db, &present).await? else {
        // The locked session was signed out from its lock screen meanwhile.
        return Ok((
            StatusCode::UNAUTHORIZED,
            [(header::SET_COOKIE, clear_present_cookie(&state.http))],
            Failure::new(StatusCode::UNAUTHORIZED, "signed_out", "Please sign in."),
        )
            .into_response());
    };
    // The demo scope moved to the new token; its quotas move with it.
    state
        .limits
        .transfer_scope(&present.staff_token_hash, &issued.session.token_hash);
    audit::record(
        &state.db,
        actor,
        "present_exited",
        &format!("presentation {} with {method}", present.presentation_id),
    )
    .await?;
    let mut headers = HeaderMap::new();
    headers.append(header::SET_COOKIE, clear_present_cookie(&state.http));
    headers.append(
        header::SET_COOKIE,
        session_cookie(&state.http, &issued.token),
    );
    Ok((
        headers,
        Json(ExitPresentResponse {
            staff_path: format!("/presentations/{}", present.presentation_id),
            csrf_token: issued.session.csrf_token,
        }),
    )
        .into_response())
}

fn pin_disabled() -> Failure {
    Failure::new(
        StatusCode::FORBIDDEN,
        "pin_disabled",
        "Too many wrong PINs. Enter the staff password to unlock.",
    )
}

/// Checks the PIN or password for a clinical exit. Returns how it unlocked.
async fn check_unlock(
    state: &AppState,
    present: &PresentSession,
    ip: IpAddr,
    request: ExitPresentRequest,
) -> Result<&'static str, Failure> {
    let stored: Option<(String, String)> =
        sqlx::query_as("SELECT password_hash, pin_hash FROM staff_account WHERE id = 1")
            .fetch_optional(&state.db)
            .await?;
    let Some((password_hash, pin_hash)) = stored else {
        return Err(conflict(
            "setup_required",
            "DentalClarity has not been set up yet.",
        ));
    };
    let detail = format!("presentation {}", present.presentation_id);

    // The password always works; it is the only way once the PIN is disabled.
    if let Some(password) = request.password.filter(|p| !p.is_empty()) {
        if let Some(wait) = state.limits.login_backoff.wait(ip) {
            return Err(Failure::too_many(wait));
        }
        if !verify(password, password_hash).await? {
            state.limits.login_backoff.record_failure(ip);
            audit::record(&state.db, "staff", "present_exit_password_failed", &detail).await?;
            return Err(Failure::new(
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "That password is not correct.",
            ));
        }
        state.limits.login_backoff.record_success(ip);
        return Ok("password");
    }

    let Some(pin) = request.pin.filter(|p| !p.is_empty()) else {
        if present.pin_attempts >= MAX_PIN_ATTEMPTS {
            return Err(pin_disabled());
        }
        return Err(Failure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "pin_required",
            "Enter the staff PIN to leave the presentation.",
        ));
    };
    let Some(used) = sessions::reserve_pin_attempt(&state.db, &present.token_hash).await? else {
        return Err(pin_disabled());
    };
    if verify(pin, pin_hash).await? {
        return Ok("pin");
    }
    audit::record(&state.db, "staff", "present_exit_pin_failed", &detail).await?;
    if used >= MAX_PIN_ATTEMPTS {
        audit::record(&state.db, "staff", "present_pin_disabled", &detail).await?;
        return Err(pin_disabled());
    }
    let left = MAX_PIN_ATTEMPTS - used;
    Err(Failure::new(
        StatusCode::UNAUTHORIZED,
        "invalid_pin",
        format!(
            "That PIN is not correct. {left} {} left before the staff password is needed.",
            if left == 1 { "try" } else { "tries" }
        ),
    ))
}

/// Argon2 verification off the async runtime. The comparison inside the
/// `argon2` crate is constant time.
async fn verify(secret: String, hash: String) -> Result<bool, Failure> {
    tokio::task::spawn_blocking(move || verify_secret(&secret, &hash))
        .await
        .map_err(Failure::internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_knowledge_prose_into_sentences() {
        assert_eq!(
            split_sentences("One thing. Dr. Lee says two! Three? four. Five"),
            ["One thing.", "Dr. Lee says two!", "Three? four.", "Five"]
        );
        assert!(split_sentences("  ").is_empty());
    }

    #[test]
    fn placeholders_are_filled_locally() {
        let with = Filler {
            practice: "Maple Grove",
            provider: Some("Dr. Rivera"),
            voice: "model#voice",
            language: "en",
        };
        assert_eq!(
            with.text("{{provider_display_name}} at {{practice_name}} explains it."),
            "Dr. Rivera at Maple Grove explains it."
        );
        let without = Filler {
            provider: None,
            ..with
        };
        assert_eq!(
            without.text(
                "{{provider_display_name}} explains it, and {{provider_display_name}} listens."
            ),
            "Your dentist explains it, and your dentist listens."
        );
        let s = without.sentence("Hello {{practice_name}}.");
        assert_eq!(s.text, "Hello Maple Grove.");
        assert_eq!(
            s.audio_key,
            audio_key("model#voice", "en", "Hello Maple Grove.")
        );
    }
}
