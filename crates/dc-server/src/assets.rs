//! Serves the compiled web app, embedded at build time.
//!
//! Run `trunk build` in `crates/dc-web` before building the server for release.
//! Without a build the server still runs and answers API requests, and page
//! requests return a short notice.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../dc-web/dist"]
#[allow_missing = true]
struct WebApp;

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some(response) = file(path) {
        return response;
    }
    // Client-side routes fall back to the app shell. Missing asset files do not.
    if path.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    file("index.html").unwrap_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "The web app has not been built. Run `trunk build` in crates/dc-web.",
        )
            .into_response()
    })
}

fn file(path: &str) -> Option<Response> {
    let asset = WebApp::get(path)?;
    let mime = asset.metadata.mimetype().to_owned();
    let cache = if path == "index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    Some(
        (
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, cache.to_owned()),
            ],
            asset.data.into_owned(),
        )
            .into_response(),
    )
}
