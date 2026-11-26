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
    let csrf = CSRF.with(|c| c.borrow().clone()).unwrap_or_default();
    let request = match write {
        Write::Post => Request::post(path),
        Write::Put => Request::put(path),
    };
    request
        .header("X-DC-CSRF", &csrf)
        .json(body)
        .map_err(network)?
        .send()
        .await
        .map_err(network)
}

const UNLOCK_PATH: &str = "/api/session/unlock";

/// Sends a write. A staff write (or an unlock) refused with `bad_csrf` means this tab holds
/// an old token, for example after another tab unlocked the session, so the
/// session is read again for the current token and the write is sent once
/// more. A second refusal is returned as it is.
async fn write<B: Serialize, T: DeserializeOwned>(
    write: Write,
    path: &str,
    body: &B,
) -> Result<T, Failure> {
    let staff = path.starts_with("/api/staff/");
    let retry = staff || path == UNLOCK_PATH;
    let response = send(write, path, body).await?;
    if !(retry && response.status() == 403) {
        return finish(response, staff).await;
    }
    let failure = refusal(response, staff).await;
    if failure.code != "bad_csrf" || !refresh_csrf().await {
        return Err(failure);
    }
    finish(send(write, path, body).await?, staff).await
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

/// Starts a demo visitor session.
pub async fn visit() -> Result<SessionInfo, Failure> {
    let info: SessionInfo = post("/api/visit", &()).await?;
    set_csrf(info.csrf_token.clone());
    Ok(info)
}

pub async fn setup(request: &SetupRequest) -> Result<SessionInfo, Failure> {
    let info: SessionInfo = post("/api/setup", request).await?;
    set_csrf(info.csrf_token.clone());
    Ok(info)
}

pub async fn login(password: String) -> Result<SessionInfo, Failure> {
    let info: SessionInfo = post("/api/auth/login", &LoginRequest { password }).await?;
    set_csrf(info.csrf_token.clone());
    Ok(info)
}

/// Lifts a lock whose present session has ended. Demo mode sends no
/// password; clinical mode sends the staff password. The answer carries a
/// new CSRF token, which replaces the old one.
pub async fn unlock(password: Option<String>) -> Result<SessionInfo, Failure> {
    let info: SessionInfo = post(UNLOCK_PATH, &UnlockSessionRequest { password }).await?;
    set_csrf(info.csrf_token.clone());
    Ok(info)
}

async fn send_logout() -> Result<Response, Failure> {
    let csrf = CSRF.with(|c| c.borrow().clone()).unwrap_or_default();
    Request::post("/api/auth/logout")
        .header("X-DC-CSRF", &csrf)
        .send()
        .await
        .map_err(network)
}

/// Ends the session. The server answers 204 with no body, so this does not
/// go through `write`, but it retries a `bad_csrf` refusal the same way:
/// once, after reading the session for the current token. The token is
/// kept on failure, so a later try can still send it.
pub async fn logout() -> Result<(), Failure> {
    let mut response = send_logout().await?;
    if response.status() == 403 {
        let failure = refusal(response, false).await;
        if failure.code != "bad_csrf" || !refresh_csrf().await {
            return Err(logout_failed(failure.status));
        }
        response = send_logout().await?;
    }
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
