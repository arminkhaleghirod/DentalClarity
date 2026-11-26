//! Calls made by the patient screens. Only `/api/present/*` endpoints are
//! used here; the present-session cookie is the only credential.

use dc_core::api::ApiError;
use dc_core::present::{ExitPresentResponse, PatientView, SENTENCE_OFFSETS_HEADER};
use gloo_net::http::{Request, Response};
use leptos::wasm_bindgen::JsCast;
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
    /// The present session is missing or expired (`present_ended`), or the
    /// staff session behind it was signed out (`signed_out`). Other 401s,
    /// such as a wrong PIN or password, leave the presentation running.
    pub fn session_ended(&self) -> bool {
        matches!(self.code.as_str(), "present_ended" | "signed_out")
    }

    /// Too many wrong PINs: only the staff password unlocks now.
    pub fn pin_disabled(&self) -> bool {
        self.code == "pin_disabled"
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

/// One part of a block's narration, ready to play.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockAudio {
    /// Object URL of the clip, for an audio element's `src`. Give it back
    /// with [`revoke`] when it is no longer needed.
    pub url: String,
    /// Start of each sentence in milliseconds, from the server's sample
    /// counts.
    pub offsets_ms: Vec<f64>,
}

/// Fetches one clip for consecutive sentences of a block
/// (`GET /api/present/narration`). A cold cache can take a few seconds.
pub async fn narration(keys: &[String]) -> Result<BlockAudio, Failure> {
    let response = Request::get(&format!("/api/present/narration?keys={}", keys.join(",")))
        .send()
        .await
        .map_err(network)?;
    if !response.ok() {
        return Err(error_of(response).await);
    }
    let unplayable = |message: &str| Failure {
        status: response.status(),
        code: String::new(),
        message: message.to_owned(),
    };
    let offsets_ms: Vec<f64> = response
        .headers()
        .get(SENTENCE_OFFSETS_HEADER)
        .unwrap_or_default()
        .split(',')
        .filter_map(|o| o.trim().parse().ok())
        .collect();
    if offsets_ms.len() != keys.len() {
        return Err(unplayable("The narration timing was missing."));
    }
    let bytes = response.binary().await.map_err(network)?;
    let url = object_url(&bytes, "audio/wav")
        .ok_or_else(|| unplayable("The browser could not prepare this clip."))?;
    Ok(BlockAudio { url, offsets_ms })
}

/// `URL.createObjectURL(new Blob([bytes], { type }))`.
fn object_url(bytes: &[u8], mime: &str) -> Option<String> {
    let global = js_sys::global();
    let blob_class = js_sys::Reflect::get(&global, &"Blob".into())
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?;
    let options = js_sys::Object::new();
    js_sys::Reflect::set(&options, &"type".into(), &mime.into()).ok()?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let blob =
        js_sys::Reflect::construct(&blob_class, &js_sys::Array::of2(&parts, &options)).ok()?;
    let url_class = js_sys::Reflect::get(&global, &"URL".into()).ok()?;
    js_sys::Reflect::get(&url_class, &"createObjectURL".into())
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?
        .call1(&url_class, &blob)
        .ok()?
        .as_string()
}

/// Releases an object URL made by [`narration`].
pub fn revoke(url: &str) {
    let global = js_sys::global();
    let Ok(url_class) = js_sys::Reflect::get(&global, &"URL".into()) else {
        return;
    };
    if let Ok(f) = js_sys::Reflect::get(&url_class, &"revokeObjectURL".into())
        && let Ok(f) = f.dyn_into::<js_sys::Function>()
    {
        let _ = f.call1(&url_class, &url.into());
    }
}
