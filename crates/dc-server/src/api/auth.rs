//! Session bootstrap, first-launch setup, sign-in and sign-out.

use crate::credentials::{check_password, check_pin, hash_secret, verify_secret};
use crate::http::{
    ClientIp, Failure, JsonBody, clear_session_cookie, cookie, session_cookie, session_cookie_name,
};
use crate::limits::ip_key;
use crate::security::{check_csrf, check_new_visitor, expected_kind};
use crate::sessions::{self, Session, SessionKind};
use crate::{AppState, audit, settings};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use dc_core::Mode;
use dc_core::api::{LoginRequest, SessionInfo, SessionState, SetupRequest};
use std::net::IpAddr;
use std::time::Duration;
use subtle::ConstantTimeEq;

fn info(
    state: &AppState,
    practice_name: String,
    session_state: SessionState,
    session: Option<&Session>,
) -> SessionInfo {
    SessionInfo {
        mode: state.mode,
        state: session_state,
        practice_name,
        csrf_token: session.map(|s| s.csrf_token.clone()),
    }
}

/// The caller's session, if it is the kind this mode uses.
async fn current(state: &AppState, headers: &HeaderMap) -> Result<Option<Session>, Failure> {
    let Some(token) = cookie(headers, session_cookie_name(&state.http)) else {
        return Ok(None);
    };
    Ok(sessions::lookup(&state.db, &token)
        .await?
        .filter(|s| s.kind == expected_kind(state.mode)))
}

async fn staff_account_exists(state: &AppState) -> Result<bool, Failure> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM staff_account WHERE id = 1")
        .fetch_optional(&state.db)
        .await?;
    Ok(row.is_some())
}

fn not_in_this_mode() -> Failure {
    Failure::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "Not available in this mode.",
    )
}

fn with_cookie(state: &AppState, token: &str, body: SessionInfo) -> Response {
    (
        [(header::SET_COOKIE, session_cookie(&state.http, token))],
        Json(body),
    )
        .into_response()
}

/// `GET /api/session`: where the browser stands. Read-only.
pub async fn session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SessionInfo>, Failure> {
    let existing = current(&state, &headers).await?;
    let practice = settings::practice_name(&state).await?;
    let body = match (state.mode, existing) {
        (Mode::Demo, Some(s)) => info(&state, practice, SessionState::Visitor, Some(&s)),
        (Mode::Demo, None) => info(&state, practice, SessionState::NewVisitor, None),
        (Mode::Clinical, _) if !staff_account_exists(&state).await? => {
            info(&state, practice, SessionState::SetupRequired, None)
        }
        (Mode::Clinical, Some(s)) if s.locked => {
            info(&state, practice, SessionState::Locked, Some(&s))
        }
        (Mode::Clinical, Some(s)) => info(&state, practice, SessionState::Staff, Some(&s)),
        (Mode::Clinical, None) => info(&state, practice, SessionState::SignedOut, None),
    };
    Ok(Json(body))
}

/// `POST /api/visit`: starts an isolated demo visitor session, so no one has
/// to sign in to try the demo. Returns the existing session if there is one.
pub async fn visit(
    State(state): State<AppState>,
    ip: ClientIp,
    headers: HeaderMap,
) -> Result<Response, Failure> {
    if state.mode != Mode::Demo {
        return Err(not_in_this_mode());
    }
    let practice = settings::practice_name(&state).await?;
    if let Some(s) = current(&state, &headers).await? {
        return Ok(Json(info(&state, practice, SessionState::Visitor, Some(&s))).into_response());
    }
    check_new_visitor(&state, ip)?;
    let issued = sessions::create(&state.db, SessionKind::Visitor).await?;
    let body = info(
        &state,
        practice,
        SessionState::Visitor,
        Some(&issued.session),
    );
    Ok(with_cookie(&state, &issued.token, body))
}

/// Per-address attempt limit and backoff shared by setup and sign-in.
fn check_attempt(state: &AppState, ip: IpAddr) -> Result<(), Failure> {
    if let Some(wait) = state.limits.login_backoff.wait(ip) {
        return Err(Failure::too_many(wait));
    }
    state
        .limits
        .login
        .check(&ip_key(ip), 5, Duration::from_secs(60))
        .map_err(Failure::too_many)
}

/// Records a failed attempt. A surge of failures across all clients writes
/// one audit event per 15 minutes.
async fn record_failure(state: &AppState, ip: IpAddr, action: &str) -> Result<(), Failure> {
    state.limits.login_backoff.record_failure(ip);
    audit::record(&state.db, &ip.to_string(), action, "").await?;
    let quarter_hour = Duration::from_secs(15 * 60);
    if state
        .limits
        .login_failures
        .check("all", 50, quarter_hour)
        .is_err()
        && state
            .limits
            .login_failures
            .check("surge-reported", 1, quarter_hour)
            .is_ok()
    {
        audit::record(
            &state.db,
            "server",
            "login_failure_surge",
            "over 50 failures in 15 minutes",
        )
        .await?;
    }
    Ok(())
}

/// `POST /api/setup`: creates the shared staff account on first launch.
/// Requires the one-time setup code printed in the server console, so a
/// device on the network cannot claim a fresh install.
pub async fn setup(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    JsonBody(request): JsonBody<SetupRequest>,
) -> Result<Response, Failure> {
    if state.mode == Mode::Demo {
        return Err(not_in_this_mode());
    }
    if staff_account_exists(&state).await? {
        return Err(Failure::new(
            StatusCode::FORBIDDEN,
            "already_set_up",
            "DentalClarity is already set up.",
        ));
    }
    check_attempt(&state, ip)?;
    let expected = state.setup_code.lock().expect("setup code lock").clone();
    let code_ok = expected.is_some_and(|code| {
        bool::from(code.as_bytes().ct_eq(request.setup_code.trim().as_bytes()))
    });
    if !code_ok {
        record_failure(&state, ip, "setup_code_rejected").await?;
        return Err(Failure::new(
            StatusCode::FORBIDDEN,
            "bad_setup_code",
            "That setup code is not correct. It is printed in the server console when DentalClarity starts.",
        ));
    }

    let practice = request.practice_name.trim();
    if practice.is_empty() || practice.chars().count() > 100 {
        return Err(invalid("Enter the practice name (up to 100 characters)."));
    }
    check_password(&request.password, practice).map_err(invalid)?;
    check_pin(&request.pin).map_err(invalid)?;

    let (password, pin) = (request.password, request.pin);
    let (password_hash, pin_hash) = tokio::task::spawn_blocking(move || {
        Ok::<_, anyhow::Error>((hash_secret(&password)?, hash_secret(&pin)?))
    })
    .await
    .map_err(Failure::internal)??;

    let mut tx = state.db.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO staff_account (id, password_hash, pin_hash) VALUES (1, ?, ?)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(&password_hash)
    .bind(&pin_hash)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Err(Failure::new(
            StatusCode::FORBIDDEN,
            "already_set_up",
            "DentalClarity is already set up.",
        ));
    }
    settings::set(&mut *tx, settings::PRACTICE_NAME, practice).await?;
    tx.commit().await?;
    *state.setup_code.lock().expect("setup code lock") = None;
    state.limits.login_backoff.record_success(ip);
    audit::record(&state.db, &ip.to_string(), "setup_completed", "").await?;

    signed_in(&state, practice.to_owned()).await
}

/// `POST /api/auth/login`: signs in to the shared staff account.
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    JsonBody(request): JsonBody<LoginRequest>,
) -> Result<Response, Failure> {
    if state.mode == Mode::Demo {
        return Err(not_in_this_mode());
    }
    check_attempt(&state, ip)?;

    let stored: Option<(String,)> =
        sqlx::query_as("SELECT password_hash FROM staff_account WHERE id = 1")
            .fetch_optional(&state.db)
            .await?;
    let Some((hash,)) = stored else {
        return Err(Failure::new(
            StatusCode::CONFLICT,
            "setup_required",
            "DentalClarity has not been set up yet.",
        ));
    };
    let password = request.password;
    let ok = tokio::task::spawn_blocking(move || verify_secret(&password, &hash))
        .await
        .map_err(Failure::internal)?;
    if !ok {
        record_failure(&state, ip, "login_failed").await?;
        return Err(Failure::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "That password is not correct.",
        ));
    }
    state.limits.login_backoff.record_success(ip);
    audit::record(&state.db, &ip.to_string(), "login", "").await?;
    let practice = settings::practice_name(&state).await?;
    signed_in(&state, practice).await
}

/// `POST /api/auth/logout`: ends the current session. Allowed while locked.
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Failure> {
    if let Some(session) = current(&state, &headers).await? {
        check_csrf(&headers, &session)?;
        sessions::delete(&state.db, &session.token_hash).await?;
        if session.kind == SessionKind::Staff {
            audit::record(&state.db, "staff", "logout", "").await?;
        }
    }
    Ok((
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, clear_session_cookie(&state.http))],
    )
        .into_response())
}

async fn signed_in(state: &AppState, practice: String) -> Result<Response, Failure> {
    let issued = sessions::create(&state.db, SessionKind::Staff).await?;
    let body = info(state, practice, SessionState::Staff, Some(&issued.session));
    Ok(with_cookie(state, &issued.token, body))
}

fn invalid(message: &'static str) -> Failure {
    Failure::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid", message)
}
