//! Request helpers: error bodies, cookies and client addresses.

use crate::AppState;
use crate::config::HttpConfig;
use axum::Json;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use dc_core::api::ApiError;
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// A failed API call. Renders as JSON `ApiError` with the given status.
#[derive(Debug)]
pub struct Failure {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub retry_after: Option<u64>,
}

impl Failure {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn too_many(retry_after: u64) -> Self {
        Self {
            retry_after: Some(retry_after),
            ..Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Too many attempts. Wait a moment and try again.",
            )
        }
    }

    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::error!(%err, "request failed");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Something went wrong on the server.",
        )
    }
}

impl From<anyhow::Error> for Failure {
    fn from(err: anyhow::Error) -> Self {
        Failure::internal(err)
    }
}

impl From<sqlx::Error> for Failure {
    fn from(err: sqlx::Error) -> Self {
        Failure::internal(err)
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let body = Json(ApiError {
            code: self.code.to_owned(),
            message: self.message,
        });
        let mut response = (self.status, body).into_response();
        if let Some(seconds) = self.retry_after {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
    }
}

pub fn session_cookie_name(http: &HttpConfig) -> &'static str {
    if http.secure() {
        "__Host-dc_session"
    } else {
        "dc_session"
    }
}

/// Reads one cookie value from the request headers.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
}

/// `Set-Cookie` value for a session token. No `Max-Age`, so the cookie ends
/// with the browser session; the server enforces the real lifetimes.
pub fn session_cookie(http: &HttpConfig, token: &str) -> HeaderValue {
    set_cookie(http, session_cookie_name(http), token)
}

pub fn clear_session_cookie(http: &HttpConfig) -> HeaderValue {
    clear_cookie(http, session_cookie_name(http))
}

/// Cookie of the present session that the patient screens use. It is never
/// accepted by `/api/staff/*`, and the session cookie is never accepted by
/// `/api/present/*`.
pub fn present_cookie_name(http: &HttpConfig) -> &'static str {
    if http.secure() {
        "__Host-dc_present"
    } else {
        "dc_present"
    }
}

pub fn present_cookie(http: &HttpConfig, token: &str) -> HeaderValue {
    set_cookie(http, present_cookie_name(http), token)
}

pub fn clear_present_cookie(http: &HttpConfig) -> HeaderValue {
    clear_cookie(http, present_cookie_name(http))
}

fn set_cookie(http: &HttpConfig, name: &str, token: &str) -> HeaderValue {
    let secure = if http.secure() { "; Secure" } else { "" };
    let value = format!("{name}={token}; Path=/; HttpOnly; SameSite=Strict{secure}");
    HeaderValue::from_str(&value).expect("cookie value is ASCII")
}

fn clear_cookie(http: &HttpConfig, name: &str) -> HeaderValue {
    let secure = if http.secure() { "; Secure" } else { "" };
    let value = format!("{name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}");
    HeaderValue::from_str(&value).expect("cookie value is ASCII")
}

/// The client address.
///
/// Forwarding headers are read only when the direct peer is inside the
/// trusted proxy ranges. Then the configured client-IP header (such as
/// `fly-client-ip`) wins; otherwise the rightmost `X-Forwarded-For` entry,
/// which the proxy itself appended, is used. Entries to the left come from
/// the client and can be forged. An unknown peer maps to `0.0.0.0`, which is
/// never treated as local.
pub fn client_ip(headers: &HeaderMap, peer: Option<IpAddr>, http: &HttpConfig) -> IpAddr {
    let Some(peer) = peer else {
        return IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    };
    let trusted = http
        .trusted_proxy
        .as_ref()
        .is_some_and(|r| r.contains(peer));
    if !trusted {
        return peer;
    }
    if let Some(name) = &http.client_ip_header
        && let Some(ip) = headers
            .get(name.as_str())
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse().ok())
    {
        return ip;
    }
    if let Some(forwarded) = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .last()
        .and_then(|v| v.trim().parse().ok())
    {
        return forwarded;
    }
    peer
}

/// `Json` that reports a malformed body as an `ApiError` instead of axum's
/// plain-text rejection, without echoing parser details.
pub struct JsonBody<T>(pub T);

impl<S, T> axum::extract::FromRequest<S> for JsonBody<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned,
{
    type Rejection = Failure;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(JsonBody(value)),
            Err(rejection) => Err(Failure::new(
                rejection.status(),
                "bad_request",
                "The request could not be read.",
            )),
        }
    }
}

/// Extractor for the client IP address.
#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = ConnectInfo::<SocketAddr>::from_request_parts(parts, state)
            .await
            .ok()
            .map(|ConnectInfo(addr)| addr.ip());
        Ok(ClientIp(client_ip(&parts.headers, peer, &state.http)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(proxy: Option<&str>) -> HttpConfig {
        HttpConfig {
            public_origin: "http://127.0.0.1:8080".into(),
            trusted_proxy: proxy.map(|p| p.parse().unwrap()),
            client_ip_header: None,
            hsts: false,
        }
    }

    #[test]
    fn reads_cookies() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "a=1; dc_session=abc; b=2".parse().unwrap());
        assert_eq!(cookie(&headers, "dc_session").as_deref(), Some("abc"));
        assert_eq!(cookie(&headers, "missing"), None);
    }

    #[test]
    fn forwarded_for_is_trusted_only_from_the_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.9, 10.0.0.1".parse().unwrap());
        let proxy: IpAddr = "10.0.0.1".parse().unwrap();
        let other: IpAddr = "10.0.0.2".parse().unwrap();
        // The rightmost entry is the one the proxy added; the left one is client-supplied.
        assert_eq!(
            client_ip(&headers, Some(proxy), &http(Some("10.0.0.1"))),
            "10.0.0.1".parse::<IpAddr>().unwrap()
        );
        let mut spoofed = HeaderMap::new();
        spoofed.insert(
            "x-forwarded-for",
            "127.0.0.1, 198.51.100.7".parse().unwrap(),
        );
        assert_eq!(
            client_ip(&spoofed, Some(proxy), &http(Some("10.0.0.1"))),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            client_ip(&headers, Some(other), &http(Some("10.0.0.1"))),
            other
        );
        assert_eq!(client_ip(&headers, Some(proxy), &http(None)), proxy);
        assert!(!client_ip(&headers, None, &http(None)).is_loopback());
    }

    #[test]
    fn client_ip_header_is_trusted_only_from_the_proxy_range() {
        let cfg = HttpConfig {
            public_origin: "https://demo.example.org".into(),
            trusted_proxy: Some("fdaa::/16".parse().unwrap()),
            client_ip_header: Some("fly-client-ip".into()),
            hsts: false,
        };
        let mut headers = HeaderMap::new();
        headers.insert("fly-client-ip", "203.0.113.50".parse().unwrap());
        let proxy: IpAddr = "fdaa:0:1:a7b::2".parse().unwrap();
        let outsider: IpAddr = "198.51.100.9".parse().unwrap();
        assert_eq!(
            client_ip(&headers, Some(proxy), &cfg),
            "203.0.113.50".parse::<IpAddr>().unwrap()
        );
        // A client connecting directly cannot choose its address with the header.
        assert_eq!(client_ip(&headers, Some(outsider), &cfg), outsider);
        // Without the header, the proxy falls back to X-Forwarded-For, then to the peer.
        assert_eq!(client_ip(&HeaderMap::new(), Some(proxy), &cfg), proxy);
    }

    #[test]
    fn cookie_flags_follow_the_origin_scheme() {
        let plain = session_cookie(&http(None), "t");
        assert!(plain.to_str().unwrap().starts_with("dc_session=t;"));
        assert!(!plain.to_str().unwrap().contains("Secure"));
        let tls = HttpConfig {
            public_origin: "https://demo.example.org".into(),
            trusted_proxy: None,
            client_ip_header: None,
            hsts: false,
        };
        let secure = session_cookie(&tls, "t");
        assert!(secure.to_str().unwrap().starts_with("__Host-dc_session=t;"));
        assert!(secure.to_str().unwrap().contains("Secure"));
        let present = present_cookie(&tls, "p");
        assert!(
            present
                .to_str()
                .unwrap()
                .starts_with("__Host-dc_present=p;")
        );
        assert!(
            present
                .to_str()
                .unwrap()
                .contains("HttpOnly; SameSite=Strict")
        );
        assert!(
            present_cookie(&http(None), "p")
                .to_str()
                .unwrap()
                .starts_with("dc_present=p;")
        );
    }
}
