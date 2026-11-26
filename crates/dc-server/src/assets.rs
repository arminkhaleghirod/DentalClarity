//! Serves the compiled web app, embedded at build time.
//!
//! Run `trunk build` in `crates/dc-web` before building the server for release.
//! Without a build the server still runs and answers API requests, and page
//! requests return a short notice.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rust_embed::RustEmbed;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

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

/// The Content Security Policy for every response.
///
/// Trunk loads the WebAssembly module from a small inline script. Instead of
/// allowing inline scripts in general, the policy allows exactly the inline
/// scripts in the embedded `index.html`, by hash.
pub fn content_security_policy() -> &'static str {
    static POLICY: OnceLock<String> = OnceLock::new();
    POLICY.get_or_init(|| {
        let hashes: String = WebApp::get("index.html")
            .map(|f| inline_script_hashes(&String::from_utf8_lossy(&f.data)))
            .unwrap_or_default()
            .iter()
            .map(|h| format!(" '{h}'"))
            .collect();
        format!(
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'{hashes}; \
             style-src 'self'; img-src 'self' data:; media-src 'self'; connect-src 'self'; \
             font-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; \
             form-action 'self'"
        )
    })
}

/// `sha256-…` sources for every inline `<script>` element in `html`.
fn inline_script_hashes(html: &str) -> Vec<String> {
    let mut hashes = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<script") {
        rest = &rest[start..];
        let Some(tag_end) = rest.find('>') else { break };
        let tag = &rest[..tag_end];
        let body_start = tag_end + 1;
        let Some(close) = rest[body_start..].find("</script>") else {
            break;
        };
        if !tag.contains("src=") {
            let body = &rest[body_start..body_start + close];
            hashes.push(format!(
                "sha256-{}",
                STANDARD.encode(Sha256::digest(body.as_bytes()))
            ));
        }
        rest = &rest[body_start + close..];
    }
    hashes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_only_inline_scripts() {
        let html = r#"<script src="/a.js"></script><script type="module">init()</script>"#;
        let hashes = inline_script_hashes(html);
        assert_eq!(hashes.len(), 1);
        let expected = STANDARD.encode(Sha256::digest(b"init()"));
        assert_eq!(hashes[0], format!("sha256-{expected}"));
    }

    #[test]
    fn policy_blocks_framing_and_plugins() {
        let policy = content_security_policy();
        assert!(policy.contains("frame-ancestors 'none'"));
        assert!(policy.contains("object-src 'none'"));
        assert!(!policy.contains("unsafe-inline"));
    }
}
