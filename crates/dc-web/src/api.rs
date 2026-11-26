//! Calls to the DentalClarity JSON API.

use dc_core::api::{HealthResponse, PatientRow};
use gloo_net::http::Request;
use serde::de::DeserializeOwned;

async fn get<T: DeserializeOwned>(path: &str) -> Result<T, String> {
    let response = Request::get(path).send().await.map_err(|e| e.to_string())?;
    if !response.ok() {
        return Err(format!("The server returned {}.", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

pub async fn health() -> Result<HealthResponse, String> {
    get("/api/health").await
}

pub async fn dashboard() -> Result<Vec<PatientRow>, String> {
    get("/api/staff/dashboard").await
}
