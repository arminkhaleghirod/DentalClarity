//! Calls made by the patient screens. Only `/api/present/*` endpoints are
//! used here; the present-session cookie is the only credential.

use dc_core::api::ApiError;
use dc_core::present::{ExitPresentResponse, PatientView};
use gloo_net::http::{Request, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// A failed call, with a message safe to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub status: u16,
    /// The server's `ApiError::code`, or empty when the body had none.
    pub code: String,
    pub message: String,
}

impl Failure {
    /// The present session is missing or expired.
    pub fn session_ended(&self) -> bool {
        self.status == 401
    }
}

/// Body of `POST /api/present/exit`. Absent fields are left out, so demo
/// mode sends `{}`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExitBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

fn network(e: gloo_net::Error) -> Failure {
    Failure {
        status: 0,
        code: String::new(),
        message: format!("Could not reach the server. {e}"),
    }
}

async fn error_of(response: Response) -> Failure {
    let status = response.status();
    let (code, message) = match response.json::<ApiError>().await {
        Ok(body) => (body.code, body.message),
        Err(_) => (String::new(), format!("The server returned {status}.")),
    };
    Failure {
        status,
        code,
        message,
    }
}

async fn finish<T: DeserializeOwned>(response: Response) -> Result<T, Failure> {
    if response.ok() {
        let status = response.status();
        return response.json().await.map_err(|e| Failure {
            status,
            code: String::new(),
            message: e.to_string(),
        });
    }
    Err(error_of(response).await)
}

pub async fn content() -> Result<PatientView, Failure> {
    let response = Request::get("/api/present/content")
        .send()
        .await
        .map_err(network)?;
    finish(response).await
}

pub async fn exit(body: &ExitBody) -> Result<ExitPresentResponse, Failure> {
    let response = Request::post("/api/present/exit")
        .json(body)
        .map_err(network)?
        .send()
        .await
        .map_err(network)?;
    finish(response).await
}

/// URL of one narrated sentence, used as an audio element's `src`.
pub fn audio_url(audio_key: &str) -> String {
    format!(
        "/api/present/audio/{}",
        String::from(js_sys::encode_uri_component(audio_key))
    )
}

/// Asks the server why a clip failed to play. The audio element only
/// reports that loading failed, not the status or `ApiError` code.
pub async fn audio_failure(audio_key: &str) -> Failure {
    match Request::get(&audio_url(audio_key)).send().await {
        Ok(response) if response.ok() => Failure {
            status: response.status(),
            code: String::new(),
            message: "The browser could not play this clip.".into(),
        },
        Ok(response) => error_of(response).await,
        Err(e) => network(e),
    }
}
