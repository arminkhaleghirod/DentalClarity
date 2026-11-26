//! Calls to the DentalClarity JSON API.
//!
//! State-changing requests carry the session's CSRF token in `X-DC-CSRF`.
//! When the server answers 401 or 423, the app re-checks the session so the
//! user lands on the sign-in or lock screen.

use dc_core::api::{
    ApiError, HealthResponse, LoginRequest, PatientDetail, PatientListItem, PatientRow,
    PracticeSettings, SessionInfo, SetupRequest,
};
use dc_core::present::{
    BlockChangeResponse, ConfirmNoteRequest, RevertBlockRequest, StartPresentResponse,
    UpdateBlockRequest,
};
use dc_core::presentation::{
    CreatePresentationRequest, CreatePresentationResponse, JobStatus, PresentationSummary,
    PresentationView,
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
        });
    }
    if watch_session && (status == 401 || status == 423) {
        ON_SESSION_LOST.with(|h| {
            if let Some(handler) = h.borrow().as_ref() {
                handler();
            }
        });
    }
    let (code, message) = match response.json::<ApiError>().await {
        Ok(body) => (body.code, body.message),
        Err(_) => (String::new(), format!("The server returned {status}.")),
    };
    Err(Failure {
        status,
        code,
        message,
    })
}

fn network(e: gloo_net::Error) -> Failure {
    Failure {
        status: 0,
        code: String::new(),
        message: format!("Could not reach the server. {e}"),
    }
}

async fn get<T: DeserializeOwned>(path: &str) -> Result<T, Failure> {
    let response = Request::get(path).send().await.map_err(network)?;
    finish(response, path.starts_with("/api/staff/")).await
}

async fn post<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T, Failure> {
    let csrf = CSRF.with(|c| c.borrow().clone()).unwrap_or_default();
    let response = Request::post(path)
        .header("X-DC-CSRF", &csrf)
        .json(body)
        .map_err(network)?
        .send()
        .await
        .map_err(network)?;
    finish(response, path.starts_with("/api/staff/")).await
}

async fn put<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T, Failure> {
    let csrf = CSRF.with(|c| c.borrow().clone()).unwrap_or_default();
    let response = Request::put(path)
        .header("X-DC-CSRF", &csrf)
        .json(body)
        .map_err(network)?
        .send()
        .await
        .map_err(network)?;
    finish(response, path.starts_with("/api/staff/")).await
}

/// Percent-encodes a value for a path segment or query string.
pub fn encode(value: &str) -> String {
    String::from(js_sys::encode_uri_component(value))
}

pub async fn session() -> Result<SessionInfo, Failure> {
    let info: SessionInfo = get("/api/session").await?;
    set_csrf(info.csrf_token.clone());
    Ok(info)
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

pub async fn logout() -> Result<(), Failure> {
    let csrf = CSRF.with(|c| c.borrow().clone()).unwrap_or_default();
    let response = Request::post("/api/auth/logout")
        .header("X-DC-CSRF", &csrf)
        .send()
        .await
        .map_err(network)?;
    set_csrf(None);
    if response.ok() {
        Ok(())
    } else {
        Err(Failure {
            status: response.status(),
            code: String::new(),
            message: "Sign-out failed. Close the browser to end the session.".into(),
        })
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
