//! Middleware: response headers, Origin checks, and the staff and present
//! session gates.

use crate::http::{ClientIp, Failure, cookie, present_cookie_name, session_cookie_name};
use crate::sessions::{self, PresentSession, Session};
use crate::{AppState, assets};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::time::Duration;
use subtle::ConstantTimeEq;

/// Header carrying the session's CSRF token on state-changing requests.
pub const CSRF_HEADER: &str = "x-dc-csrf";

/// `423` code for a locked session that has no live present session left.
pub const LOCKED_PRESENT_ENDED: &str = "locked_present_ended";

fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Adds security headers to every response.
pub async fn headers(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    let set = |h: &mut axum::http::HeaderMap, name: &'static str, value: &str| {
        if let Ok(v) = HeaderValue::from_str(value) {
            h.insert(name, v);
        }
    };
    // A response may bring a stricter policy of its own (the practice logo).
    if !h.contains_key("content-security-policy") {
        set(
            h,
            "content-security-policy",
            assets::content_security_policy(),
        );
    }
    set(h, "x-content-type-options", "nosniff");
    set(h, "x-frame-options", "DENY");
    set(h, "referrer-policy", "no-referrer");
    set(
        h,
        "permissions-policy",
        "camera=(), microphone=(), geolocation=(), payment=()",
    );
    set(h, "cross-origin-opener-policy", "same-origin");
    if state.http.secure() && state.http.hsts {
        set(h, "strict-transport-security", "max-age=31536000");
    }
    if is_api {
        set(h, header::CACHE_CONTROL.as_str(), "no-store");
    }
    response
}

/// Rejects state-changing API requests that do not come from our own origin.
pub async fn same_origin(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if is_safe(request.method()) {
        return next.run(request).await;
    }
    let expected = state.http.public_origin.as_str();
    let headers = request.headers();
    let origin_ok = match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(origin) => origin == expected,
        None => headers
            .get(header::REFERER)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|r| r.starts_with(&format!("{expected}/"))),
    };
    if !origin_ok {
        return Failure::new(
            StatusCode::FORBIDDEN,
            "bad_origin",
            "This request did not come from DentalClarity.",
        )
        .into_response();
    }
    next.run(request).await
}

/// Requires a valid, unlocked session for `/api/staff/*`, enforces the
/// per-session rate limit and the CSRF token, and stores the session in the
/// request extensions for handlers.
pub async fn require_session(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers().clone();
    let method = request.method().clone();
    match check_session(&state, &headers, &method).await {
        Ok(session) => {
            let mut request = request;
            request.extensions_mut().insert(session);
            next.run(request).await
        }
        Err(failure) => failure.into_response(),
    }
}

async fn check_session(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    method: &Method,
) -> Result<Session, Failure> {
    let token = cookie(headers, session_cookie_name(&state.http)).ok_or_else(signed_out)?;
    let session = sessions::lookup(&state.db, &token)
        .await?
        .ok_or_else(signed_out)?;
    // A visitor session must never open the clinical workspace, and a staff
    // session has no meaning in the demo.
    if session.kind != expected_kind(state.mode) {
        return Err(signed_out());
    }
    if session.locked {
        // A lock whose present session has ended can only be lifted with
        // `POST /api/session/unlock`, so the app needs to tell the two apart.
        return Err(
            if sessions::live_present(&state.db, &session.token_hash).await? {
                Failure::new(
                    StatusCode::LOCKED,
                    "locked",
                    "This session is locked while a patient is viewing a presentation.",
                )
            } else {
                Failure::new(
                    StatusCode::LOCKED,
                    LOCKED_PRESENT_ENDED,
                    "This session is still locked, but the presentation that locked it has ended. Unlock it to continue.",
                )
            },
        );
    }
    state
        .limits
        .staff_api
        .check(&session.token_hash, 300, Duration::from_secs(60))
        .map_err(Failure::too_many)?;
    if !is_safe(method) {
        check_csrf(headers, &session)?;
    }
    Ok(session)
}

pub fn check_csrf(headers: &axum::http::HeaderMap, session: &Session) -> Result<(), Failure> {
    let sent = headers
        .get(CSRF_HEADER)
        .map(|v| v.as_bytes())
        .unwrap_or_default();
    if bool::from(sent.ct_eq(session.csrf_token.as_bytes())) {
        Ok(())
    } else {
        Err(Failure::new(
            StatusCode::FORBIDDEN,
            "bad_csrf",
            "Your session expired. Reload the page and try again.",
        ))
    }
}

pub fn expected_kind(mode: dc_core::Mode) -> sessions::SessionKind {
    match mode {
        dc_core::Mode::Demo => sessions::SessionKind::Visitor,
        dc_core::Mode::Clinical => sessions::SessionKind::Staff,
    }
}

fn signed_out() -> Failure {
    Failure::new(StatusCode::UNAUTHORIZED, "signed_out", "Please sign in.")
}

/// Extractor for the session stored by [`require_session`].
pub struct CurrentSession(pub Session);

impl FromRequestParts<AppState> for CurrentSession {
    type Rejection = Failure;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Session>()
            .cloned()
            .map(CurrentSession)
            .ok_or_else(signed_out)
    }
}

/// Requires a live present session for `/api/present/*`. Only the present
/// cookie is read; a staff session cookie never opens these routes. The
/// present session has no CSRF token: its cookie is `SameSite=Strict`, every
/// write passes the Origin check in [`same_origin`], and bodies must be JSON,
/// which a cross-site form cannot send.
pub async fn require_present(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    match check_present(&state, request.headers()).await {
        Ok(present) => {
            let mut request = request;
            request.extensions_mut().insert(present);
            next.run(request).await
        }
        Err(failure) => failure.into_response(),
    }
}

async fn check_present(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<PresentSession, Failure> {
    let token = cookie(headers, present_cookie_name(&state.http)).ok_or_else(present_ended)?;
    let present = sessions::lookup_present(&state.db, &token)
        .await?
        .ok_or_else(present_ended)?;
    if present.staff_kind != expected_kind(state.mode) {
        return Err(present_ended());
    }
    state
        .limits
        .present_api
        .check(&present.token_hash, 300, Duration::from_secs(60))
        .map_err(Failure::too_many)?;
    Ok(present)
}

fn present_ended() -> Failure {
    Failure::new(
        StatusCode::UNAUTHORIZED,
        "present_ended",
        "This presentation has ended. Ask your dental team to open it again.",
    )
}

/// Extractor for the present session stored by [`require_present`].
pub struct CurrentPresent(pub PresentSession);

impl FromRequestParts<AppState> for CurrentPresent {
    type Rejection = Failure;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<PresentSession>()
            .cloned()
            .map(CurrentPresent)
            .ok_or_else(present_ended)
    }
}

/// Global limit on new demo visitor sessions per client address.
pub fn check_new_visitor(state: &AppState, ip: ClientIp) -> Result<(), Failure> {
    state
        .limits
        .visitor
        .check(&crate::limits::ip_key(ip.0), 30, Duration::from_secs(60))
        .map_err(Failure::too_many)
}
