//! Calls to the DentalClarity JSON API.
//!
//! State-changing requests carry the session's CSRF token in `X-DC-CSRF`.
//! When the server answers 401 or 423, the app re-checks the session so the
//! user lands on the sign-in or lock screen.

use dc_core::api::{
    ApiError, HealthResponse, LoginRequest, PatientDetail, PatientListItem, PatientRow,
    PracticeSettings, SessionInfo, SetupRequest,
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
    let message = match response.json::<ApiError>().await {
        Ok(body) => body.message,
        Err(_) => format!("The server returned {status}."),
    };
    Err(Failure { status, message })
}

fn network(e: gloo_net::Error) -> Failure {
    Failure {
        status: 0,
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
