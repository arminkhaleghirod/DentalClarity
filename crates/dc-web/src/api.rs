//! Calls to the DentalClarity JSON API.
//!
//! State-changing requests carry the session's CSRF token in `X-DC-CSRF`.
//! When the server answers 401 or 423, the app re-checks the session so the
//! user lands on the sign-in or lock screen. A staff write refused with
//! `bad_csrf` re-reads the session for a fresh token and is sent once more.

use dc_core::api::{
    ApiError, HealthResponse, LoginRequest, PatientDetail, PatientListItem, PatientRow,
    PracticeSettings, SessionInfo, SetupRequest, UnlockSessionRequest,
};
use dc_core::present::{
    BlockChangeResponse, ConfirmNoteRequest, RevertBlockRequest, StartPresentResponse,
    UpdateBlockRequest,
};
use dc_core::presentation::{
    CreatePresentationRequest, CreatePresentationResponse, JobStatus, PresentationSummary,
    PresentationView, RegenerateSectionResponse,
};
use dc_core::qa::StaffQuestions;
use gloo_net::http::{Request, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::cell::RefCell;

thread_local! {
    static CSRF: RefCell<Option<String>> = const { RefCell::new(None) };
    static ON_SESSION_LOST: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// A failed call, with a message safe to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub status: u16,
    /// The server's `ApiError::code`, or empty when the body had none.
    pub code: String,
    pub message: String,
    /// Seconds from the `Retry-After` header of a 429, when the server sent one.
    pub retry_after: Option<u64>,
}

pub fn set_csrf(token: Option<String>) {
    CSRF.with(|c| *c.borrow_mut() = token);
}

fn current_csrf() -> String {
    CSRF.with(|c| c.borrow().clone()).unwrap_or_default()
}

/// Registers what to do when the server reports the session is gone or locked.
pub fn on_session_lost(handler: impl Fn() + 'static) {
    ON_SESSION_LOST.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
}

/// `watch_session` is true for workspace calls, where a 401 or 423 means the
/// session ended. Sign-in and setup answer 401 for a wrong password, which
/// must not reset the form.
async fn finish<T: DeserializeOwned>(
    response: Response,
    watch_session: bool,
) -> Result<T, Failure> {
    let status = response.status();
    if response.ok() {
        return response.json().await.map_err(|e| Failure {
            status,
            code: String::new(),
            message: e.to_string(),
            retry_after: None,
        });
    }
    Err(refusal(response, watch_session).await)
}

/// Reads the error body of a response outside 200 to 299.
async fn refusal(response: Response, watch_session: bool) -> Failure {
    let status = response.status();
    if watch_session && (status == 401 || status == 423) {
        ON_SESSION_LOST.with(|h| {
            if let Some(handler) = h.borrow().as_ref() {
                handler();
            }
        });
    }
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.trim().parse().ok());
    let (code, message) = match response.json::<ApiError>().await {
        Ok(body) => (body.code, body.message),
        Err(_) => (String::new(), format!("The server returned {status}.")),
    };
    Failure {
        status,
        code,
        message,
        retry_after,
    }
}

fn network(e: gloo_net::Error) -> Failure {
    Failure {
        status: 0,
        code: String::new(),
        message: format!("Could not reach the server. {e}"),
        retry_after: None,
    }
}

async fn get<T: DeserializeOwned>(path: &str) -> Result<T, Failure> {
    let response = Request::get(path).send().await.map_err(network)?;
    finish(response, path.starts_with("/api/staff/")).await
}

#[derive(Clone, Copy)]
enum Write {
    Post,
    Put,
}

async fn send<B: Serialize>(write: Write, path: &str, body: &B) -> Result<Response, Failure> {
    let request = match write {
        Write::Post => Request::post(path),
        Write::Put => Request::put(path),
    };
    request
        .header("X-DC-CSRF", &current_csrf())
        .json(body)
        .map_err(network)?
        .send()
        .await
        .map_err(network)
}

/// Sends a request, and resends it once when it is refused with `bad_csrf`.
/// That refusal means this tab holds an old token, for example after another
/// tab unlocked the session, so the session is read again for the current
/// token first. Any other 403 comes back as a failure; the answer to the
/// resent request comes back as it is.
async fn send_with_fresh_csrf<Fut>(send: impl Fn() -> Fut) -> Result<Response, Failure>
where
    Fut: Future<Output = Result<Response, Failure>>,
{
    let response = send().await?;
    if response.status() != 403 {
        return Ok(response);
    }
    // A 403 never reports a lost session, so there is nothing to watch.
    let failure = refusal(response, false).await;
    if failure.code != "bad_csrf" || !refresh_csrf().await {
        return Err(failure);
    }
    send().await
}

const UNLOCK_PATH: &str = "/api/session/unlock";

/// Sends a write. Staff writes and the unlock retry a `bad_csrf` refusal
/// once, through [`send_with_fresh_csrf`].
async fn write<B: Serialize, T: DeserializeOwned>(
    write: Write,
    path: &str,
    body: &B,
) -> Result<T, Failure> {
    let staff = path.starts_with("/api/staff/");
    let send_once = || send(write, path, body);
    let response = if staff || path == UNLOCK_PATH {
        send_with_fresh_csrf(send_once).await?
    } else {
        send_once().await?
    };
    finish(response, staff).await
}

/// Reads the session only for its CSRF token, and reports whether it got
/// one. A locked session still has a token; the retried write then meets
/// the usual 423 handling.
async fn refresh_csrf() -> bool {
    match get::<SessionInfo>("/api/session").await {
        Ok(SessionInfo {
            csrf_token: Some(token),
            ..
        }) => {
            set_csrf(Some(token));
            true
        }
        _ => false,
    }
}

async fn post<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T, Failure> {
    write(Write::Post, path, body).await
}

async fn put<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T, Failure> {
    write(Write::Put, path, body).await
}

/// Percent-encodes a value for a path segment or query string.
pub fn encode(value: &str) -> String {
    String::from(js_sys::encode_uri_component(value))
}

/// Reads the session. The caller stores its CSRF token with [`set_csrf`]
/// once it knows the answer is still current: a read that crossed an unlock
/// carries the token the unlock replaced.
pub async fn session() -> Result<SessionInfo, Failure> {
    get("/api/session").await
}

/// Posts a request answered with a new session, and keeps its CSRF token.
async fn post_session<B: Serialize>(path: &str, body: &B) -> Result<SessionInfo, Failure> {
    let info: SessionInfo = post(path, body).await?;
    set_csrf(info.csrf_token.clone());
    Ok(info)
}

/// Starts a demo visitor session.
pub async fn visit() -> Result<SessionInfo, Failure> {
    post_session("/api/visit", &()).await
}

pub async fn setup(request: &SetupRequest) -> Result<SessionInfo, Failure> {
    post_session("/api/setup", request).await
}

pub async fn login(password: String) -> Result<SessionInfo, Failure> {
    post_session("/api/auth/login", &LoginRequest { password }).await
}

/// Lifts a lock whose present session has ended. Demo mode sends no
/// password; clinical mode sends the staff password. The answer carries a
/// new CSRF token, which replaces the old one.
pub async fn unlock(password: Option<String>) -> Result<SessionInfo, Failure> {
    post_session(UNLOCK_PATH, &UnlockSessionRequest { password }).await
}

async fn send_logout() -> Result<Response, Failure> {
    Request::post("/api/auth/logout")
        .header("X-DC-CSRF", &current_csrf())
        .send()
        .await
        .map_err(network)
}

/// Ends the session. The server answers 204 with no body, so this does not
/// go through `write`, but it retries a `bad_csrf` refusal the same way.
/// The token is kept on failure, so a later try can still send it.
pub async fn logout() -> Result<(), Failure> {
    let response = send_with_fresh_csrf(send_logout)
        .await
        // A network failure (status 0) keeps its own message.
        .map_err(|f| {
            if f.status == 0 {
                f
            } else {
                logout_failed(f.status)
            }
        })?;
    if response.ok() {
        set_csrf(None);
        Ok(())
    } else {
        Err(logout_failed(response.status()))
    }
}

fn logout_failed(status: u16) -> Failure {
    Failure {
        status,
        code: String::new(),
        message: "Sign-out failed. Try again, or close the browser to end the session.".into(),
        retry_after: None,
    }
}

#[allow(dead_code)]
pub async fn health() -> Result<HealthResponse, Failure> {
    get("/api/health").await
}

pub async fn dashboard() -> Result<Vec<PatientRow>, Failure> {
    get("/api/staff/dashboard").await
}

pub async fn patients(query: String) -> Result<Vec<PatientListItem>, Failure> {
    get(&format!("/api/staff/patients?q={}", encode(&query))).await
}

pub async fn patient(key: String) -> Result<PatientDetail, Failure> {
    get(&format!("/api/staff/patients/{}", encode(&key))).await
}

pub async fn settings() -> Result<PracticeSettings, Failure> {
    get("/api/staff/settings").await
}

pub async fn save_settings(settings: &PracticeSettings) -> Result<PracticeSettings, Failure> {
    put("/api/staff/settings", settings).await
}

pub async fn create_presentation(
    request: &CreatePresentationRequest,
) -> Result<CreatePresentationResponse, Failure> {
    post("/api/staff/presentations", request).await
}

pub async fn job(id: String) -> Result<JobStatus, Failure> {
    get(&format!("/api/staff/jobs/{}", encode(&id))).await
}

pub async fn presentation(id: String) -> Result<PresentationView, Failure> {
    get(&format!("/api/staff/presentations/{}", encode(&id))).await
}

/// Questions patients asked about this presentation in Present mode.
pub async fn presentation_questions(id: String) -> Result<StaffQuestions, Failure> {
    get(&format!(
        "/api/staff/presentations/{}/questions",
        encode(&id)
    ))
    .await
}

pub async fn presentations() -> Result<Vec<PresentationSummary>, Failure> {
    get("/api/staff/presentations").await
}

/// Replaces one block's sentences with staff text. Warnings are advisory.
pub async fn update_block(
    id: String,
    request: &UpdateBlockRequest,
) -> Result<BlockChangeResponse, Failure> {
    put(
        &format!("/api/staff/presentations/{}/blocks", encode(&id)),
        request,
    )
    .await
}

/// Restores a block's generated text.
pub async fn revert_block(
    id: String,
    request: &RevertBlockRequest,
) -> Result<BlockChangeResponse, Failure> {
    post(
        &format!("/api/staff/presentations/{}/blocks/revert", encode(&id)),
        request,
    )
    .await
}

/// Confirms a note-derived block so patients may see it (DECISIONS P11).
pub async fn confirm_note(
    id: String,
    request: &ConfirmNoteRequest,
) -> Result<BlockChangeResponse, Failure> {
    post(
        &format!("/api/staff/presentations/{}/confirm-note", encode(&id)),
        request,
    )
    .await
}

/// Starts Present mode. The server locks this staff session, so the caller
/// must follow up with a full page load of `present_path`.
pub async fn start_present(id: String) -> Result<StartPresentResponse, Failure> {
    post(
        &format!("/api/staff/presentations/{}/present", encode(&id)),
        &(),
    )
    .await
}

fn section_path(id: &str, section_key: &str, action: &str) -> String {
    format!(
        "/api/staff/presentations/{}/sections/{}/{action}",
        encode(id),
        encode(section_key)
    )
}

/// Writes one section again. Poll `job` with the returned job id.
pub async fn regenerate_section(
    id: String,
    section_key: String,
) -> Result<RegenerateSectionResponse, Failure> {
    post(&section_path(&id, &section_key, "regenerate"), &()).await
}

/// Releases a held section to patients after staff have checked it.
pub async fn release_section(
    id: String,
    section_key: String,
) -> Result<BlockChangeResponse, Failure> {
    post(&section_path(&id, &section_key, "release"), &()).await
}
