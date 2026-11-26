//! End-to-end tests of the API access rules, run against an in-memory database.

use crate::config::HttpConfig;
use crate::limits::Limits;
use crate::{AppState, router, storage};
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use dc_core::Mode;
use dc_core::api::{
    ApiError, HealthResponse, PatientDetail, PatientListItem, PatientRow, PracticeSettings,
    SessionInfo, SessionState,
};
use dc_knowledge::CodeMap;
use dc_pms::synthetic::SyntheticConnector;
use serde::de::DeserializeOwned;
use serde_json::json;
use sqlx::SqlitePool;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:8080";
const PASSWORD: &str = "tooth fairy on duty";
const SETUP_CODE: &str = "TEST-CODE-0001";

async fn app(mode: Mode) -> Router {
    app_with_db(mode).await.0
}

async fn app_with_db(mode: Mode) -> (Router, SqlitePool) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pms =
        SyntheticConnector::from_path(&root.join("fixtures/synthetic/practice.toml")).unwrap();
    let db = storage::open_in_memory().await.unwrap();
    let app = router(AppState {
        mode,
        http: Arc::new(HttpConfig {
            public_origin: ORIGIN.into(),
            trusted_proxy: None,
            hsts: false,
        }),
        db: db.clone(),
        default_practice_name: pms.practice_name().to_owned(),
        pms: Arc::new(pms),
        codes: Arc::new(CodeMap::from_path(&root.join("knowledge/codes.toml")).unwrap()),
        limits: Arc::new(Limits::default()),
        setup_code: Arc::new(std::sync::Mutex::new(Some(SETUP_CODE.into()))),
    });
    (app, db)
}

/// A browser-like client that keeps the session cookie and CSRF token.
struct Client {
    app: Router,
    cookie: Option<String>,
    csrf: Option<String>,
}

impl Client {
    fn new(app: Router) -> Self {
        Self {
            app,
            cookie: None,
            csrf: None,
        }
    }

    async fn send(
        &mut self,
        method: Method,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> Response {
        let mut req = Request::builder().method(method.clone()).uri(uri);
        if let Some(cookie) = &self.cookie {
            req = req.header(header::COOKIE, cookie);
        }
        if method != Method::GET {
            req = req.header(header::ORIGIN, ORIGIN);
            if let Some(csrf) = &self.csrf {
                req = req.header("x-dc-csrf", csrf);
            }
        }
        let req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let response = self.app.clone().oneshot(req).await.unwrap();
        if let Some(set) = response.headers().get(header::SET_COOKIE) {
            let pair = set.to_str().unwrap().split(';').next().unwrap().to_owned();
            self.cookie = (!pair.ends_with('=')).then_some(pair);
        }
        response
    }

    async fn session(&mut self) -> SessionInfo {
        let info: SessionInfo = json_body(self.send(Method::GET, "/api/session", None).await).await;
        self.csrf = info.csrf_token.clone();
        info
    }
}

async fn json_body<T: DeserializeOwned>(response: Response) -> T {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn error_code(response: Response) -> String {
    json_body::<ApiError>(response).await.code
}

async fn set_up(client: &mut Client) {
    let response = client
        .send(
            Method::POST,
            "/api/setup",
            Some(json!({ "setup_code": SETUP_CODE, "practice_name": "Maple Grove Family Dental", "password": PASSWORD, "pin": "480913" })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let info: SessionInfo = json_body(response).await;
    client.csrf = info.csrf_token;
}

#[tokio::test]
async fn health_reports_mode() {
    let mut client = Client::new(app(Mode::Demo).await);
    let body: HealthResponse = json_body(client.send(Method::GET, "/api/health", None).await).await;
    assert_eq!(body.mode, Mode::Demo);
}

#[tokio::test]
async fn responses_carry_security_headers() {
    let mut client = Client::new(app(Mode::Demo).await);
    let response = client.send(Method::GET, "/api/health", None).await;
    let h = response.headers();
    assert!(h.get("content-security-policy").is_some());
    assert_eq!(h.get("x-frame-options").unwrap(), "DENY");
    assert_eq!(h.get(header::CACHE_CONTROL).unwrap(), "no-store");
}

#[tokio::test]
async fn demo_visitor_gets_a_session_and_the_dashboard() {
    let mut client = Client::new(app(Mode::Demo).await);
    let unauthenticated = client.send(Method::GET, "/api/staff/dashboard", None).await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    // Reading the session never creates one.
    let info = client.session().await;
    assert_eq!(info.state, SessionState::NewVisitor);
    assert!(client.cookie.is_none());

    let started = client.send(Method::POST, "/api/visit", None).await;
    assert_eq!(started.status(), StatusCode::OK);
    let info = client.session().await;
    assert_eq!(info.state, SessionState::Visitor);
    assert_eq!(info.practice_name, "Maple Grove Family Dental");
    assert!(info.csrf_token.is_some());

    let rows: Vec<PatientRow> =
        json_body(client.send(Method::GET, "/api/staff/dashboard", None).await).await;
    let daniel = rows
        .iter()
        .find(|r| r.display_name == "Daniel Okafor")
        .unwrap();
    assert_eq!(
        daniel.plan_summary,
        "Root canal, Build-up, Crown, 2× Tooth-colored filling, Cleaning"
    );
    assert_eq!(daniel.patient_estimate.map(|m| m.cents()), Some(136_900));
    assert!(rows.iter().all(|r| r.display_name != "Grace Holloway"));

    // The same visitor keeps the same session.
    let before = client.cookie.clone();
    client.send(Method::POST, "/api/visit", None).await;
    assert_eq!(client.cookie, before);
}

#[tokio::test]
async fn demo_has_no_sign_in_or_setup() {
    let mut client = Client::new(app(Mode::Demo).await);
    client.session().await;
    let login = client
        .send(
            Method::POST,
            "/api/auth/login",
            Some(json!({ "password": PASSWORD })),
        )
        .await;
    assert_eq!(login.status(), StatusCode::NOT_FOUND);
    let setup = client
        .send(Method::POST, "/api/setup", Some(json!({ "setup_code": SETUP_CODE, "practice_name": "X", "password": PASSWORD, "pin": "480913" })))
        .await;
    assert_eq!(setup.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn writes_from_another_origin_are_rejected() {
    let app = app(Mode::Clinical).await;
    let request = Request::post("/api/auth/login")
        .header(header::ORIGIN, "https://evil.example")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "password": PASSWORD }).to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(error_code(response).await, "bad_origin");
}

#[tokio::test]
async fn clinical_setup_then_sign_in_and_out() {
    let mut client = Client::new(app(Mode::Clinical).await);
    assert_eq!(client.session().await.state, SessionState::SetupRequired);

    set_up(&mut client).await;
    assert_eq!(client.session().await.state, SessionState::Staff);
    let dashboard = client.send(Method::GET, "/api/staff/dashboard", None).await;
    assert_eq!(dashboard.status(), StatusCode::OK);

    // Setup cannot run twice.
    let again = client
        .send(Method::POST, "/api/setup", Some(json!({ "setup_code": SETUP_CODE, "practice_name": "Other", "password": PASSWORD, "pin": "480913" })))
        .await;
    assert_eq!(error_code(again).await, "already_set_up");

    // Logout needs the CSRF token.
    let saved_csrf = client.csrf.take();
    let forged = client.send(Method::POST, "/api/auth/logout", None).await;
    assert_eq!(error_code(forged).await, "bad_csrf");
    client.csrf = saved_csrf;
    let logout = client.send(Method::POST, "/api/auth/logout", None).await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert_eq!(client.session().await.state, SessionState::SignedOut);
    assert_eq!(
        client
            .send(Method::GET, "/api/staff/dashboard", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // Wrong then right password.
    let wrong = client
        .send(
            Method::POST,
            "/api/auth/login",
            Some(json!({ "password": "not the password" })),
        )
        .await;
    assert_eq!(error_code(wrong).await, "invalid_credentials");
    let right = client
        .send(
            Method::POST,
            "/api/auth/login",
            Some(json!({ "password": PASSWORD })),
        )
        .await;
    assert_eq!(right.status(), StatusCode::OK);
    let info: SessionInfo = json_body(right).await;
    assert_eq!(info.state, SessionState::Staff);
    assert_eq!(info.practice_name, "Maple Grove Family Dental");
}

#[tokio::test]
async fn setup_needs_the_one_time_code() {
    // Even a request that appears to come from the server machine needs the code.
    let app = app(Mode::Clinical)
        .await
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
    let mut client = Client::new(app);
    let response = client
        .send(Method::POST, "/api/setup", Some(json!({ "setup_code": "WRONG-CODE", "practice_name": "Maple Grove", "password": PASSWORD, "pin": "480913" })))
        .await;
    assert_eq!(error_code(response).await, "bad_setup_code");
    assert_eq!(client.session().await.state, SessionState::SetupRequired);
}

#[tokio::test]
async fn visitor_sessions_do_not_open_the_clinical_workspace() {
    let (app, db) = app_with_db(Mode::Clinical).await;
    let mut client = Client::new(app);
    set_up(&mut client).await;
    // Simulate a visitor session left over from a demo-mode start.
    let issued = crate::sessions::create(&db, crate::sessions::SessionKind::Visitor)
        .await
        .unwrap();
    client.cookie = Some(format!("dc_session={}", issued.token));
    let dashboard = client.send(Method::GET, "/api/staff/dashboard", None).await;
    assert_eq!(dashboard.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(client.session().await.state, SessionState::SignedOut);
}

#[tokio::test]
async fn malformed_bodies_get_a_json_error() {
    let mut client = Client::new(app(Mode::Clinical).await);
    let request = Request::post("/api/auth/login")
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let response = client.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error_code(response).await, "bad_request");
    let _ = &mut client;
}

#[tokio::test]
async fn weak_credentials_are_rejected_at_setup() {
    let mut client = Client::new(app(Mode::Clinical).await);
    let response = client
        .send(Method::POST, "/api/setup", Some(json!({ "setup_code": SETUP_CODE, "practice_name": "Maple Grove", "password": "short", "pin": "480913" })))
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let response = client
        .send(Method::POST, "/api/setup", Some(json!({ "setup_code": SETUP_CODE, "practice_name": "Maple Grove", "password": PASSWORD, "pin": "123456" })))
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn repeated_failures_trigger_backoff() {
    let mut client = Client::new(app(Mode::Clinical).await);
    set_up(&mut client).await;
    let mut last = StatusCode::OK;
    for _ in 0..6 {
        last = client
            .send(
                Method::POST,
                "/api/auth/login",
                Some(json!({ "password": "wrong password!" })),
            )
            .await
            .status();
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn locked_sessions_get_423_on_staff_routes() {
    let (app, db) = app_with_db(Mode::Clinical).await;
    let mut client = Client::new(app);
    set_up(&mut client).await;
    // Present mode (M5) sets this flag; simulate it directly.
    sqlx::query("UPDATE sessions SET locked = 1")
        .execute(&db)
        .await
        .unwrap();
    let dashboard = client.send(Method::GET, "/api/staff/dashboard", None).await;
    assert_eq!(dashboard.status(), StatusCode::LOCKED);
    assert_eq!(client.session().await.state, SessionState::Locked);
}

async fn demo_visitor() -> Client {
    let mut client = Client::new(app(Mode::Demo).await);
    client.send(Method::POST, "/api/visit", None).await;
    client.session().await;
    client
}

#[tokio::test]
async fn patient_search_sorts_by_last_name_and_filters() {
    let mut client = demo_visitor().await;
    let all: Vec<PatientListItem> = json_body(
        client
            .send(Method::GET, "/api/staff/patients?q=", None)
            .await,
    )
    .await;
    let names: Vec<_> = all.iter().map(|p| p.display_name.as_str()).collect();
    assert!(names.contains(&"Grace Holloway"));
    let last = |n: &&str| n.rsplit(' ').next().unwrap().to_owned();
    let mut sorted = names.clone();
    sorted.sort_by_key(last);
    assert_eq!(names, sorted);

    let found: Vec<PatientListItem> = json_body(
        client
            .send(Method::GET, "/api/staff/patients?q=okafor", None)
            .await,
    )
    .await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].patient_key, "10482");
    assert_eq!(found[0].open_plans, 1);
    assert_eq!(found[0].appointment_time.as_deref(), Some("08:30"));

    // Overlong queries are cut to 100 characters instead of failing.
    let long = "a".repeat(500);
    let response = client
        .send(Method::GET, &format!("/api/staff/patients?q={long}"), None)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let none: Vec<PatientListItem> = json_body(response).await;
    assert!(none.is_empty());
}

#[tokio::test]
async fn patient_detail_has_plan_rows_and_totals() {
    let mut client = demo_visitor().await;
    let daniel: PatientDetail = json_body(
        client
            .send(Method::GET, "/api/staff/patients/10482", None)
            .await,
    )
    .await;
    assert_eq!(daniel.last_name, "Okafor");
    let plan = &daniel.plans[0];
    assert_eq!(plan.procedures.len(), 6);
    assert_eq!(
        plan.total_patient_estimate.map(|m| m.cents()),
        Some(136_900)
    );
    assert_eq!(plan.total_fee.map(|m| m.cents()), Some(345_000));
    let first = &plan.procedures[0];
    assert_eq!(first.tooth, "30");
    assert_eq!(first.name.as_deref(), Some("Root canal"));
    assert!(!first.pms_description.is_empty());
    assert!(plan.procedures.iter().any(|p| p.tooth.is_empty()));

    let priya: PatientDetail = json_body(
        client
            .send(Method::GET, "/api/staff/patients/10655", None)
            .await,
    )
    .await;
    let plan = &priya.plans[0];
    assert_eq!(plan.total_patient_estimate, None);
    assert_eq!(plan.procedures[2].patient_estimate, None);
    assert_eq!(plan.procedures[2].fee.map(|m| m.cents()), Some(16_500));
}

#[tokio::test]
async fn unknown_patient_is_404_json() {
    let mut client = demo_visitor().await;
    let response = client
        .send(Method::GET, "/api/staff/patients/99999", None)
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(response).await, "patient_not_found");
}

#[tokio::test]
async fn demo_visitors_cannot_read_or_change_settings() {
    let mut client = demo_visitor().await;
    let read = client.send(Method::GET, "/api/staff/settings", None).await;
    assert_eq!(read.status(), StatusCode::FORBIDDEN);
    assert_eq!(error_code(read).await, "demo_read_only");
    let write = client
        .send(
            Method::PUT,
            "/api/staff/settings",
            Some(json!({ "practice_name": "X", "brand_primary": "#000000", "brand_highlight": "#E8693F" })),
        )
        .await;
    assert_eq!(write.status(), StatusCode::FORBIDDEN);
    assert_eq!(error_code(write).await, "demo_read_only");
}

#[tokio::test]
async fn clinical_staff_update_settings() {
    let (app, db) = app_with_db(Mode::Clinical).await;
    let mut client = Client::new(app);
    set_up(&mut client).await;

    let defaults: PracticeSettings =
        json_body(client.send(Method::GET, "/api/staff/settings", None).await).await;
    assert_eq!(defaults.practice_name, "Maple Grove Family Dental");
    assert_eq!(defaults.brand_primary, "#1B3A5C");
    assert_eq!(defaults.brand_highlight, "#E8693F");

    let response = client
        .send(
            Method::PUT,
            "/api/staff/settings",
            Some(json!({ "practice_name": "  Cedar Park Dental ", "brand_primary": "#2a4d14", "brand_highlight": "#e8693f" })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let saved: PracticeSettings = json_body(response).await;
    assert_eq!(saved.practice_name, "Cedar Park Dental");
    assert_eq!(saved.brand_primary, "#2A4D14");
    let reread: PracticeSettings =
        json_body(client.send(Method::GET, "/api/staff/settings", None).await).await;
    assert_eq!(reread, saved);
    assert_eq!(client.session().await.practice_name, "Cedar Park Dental");
    let (audited,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM audit_events WHERE action = 'settings_changed'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(audited, 1);

    // A light brand color fails the contrast check with white text.
    let pale = client
        .send(
            Method::PUT,
            "/api/staff/settings",
            Some(json!({ "practice_name": "Cedar Park Dental", "brand_primary": "#FFD966", "brand_highlight": "#E8693F" })),
        )
        .await;
    assert_eq!(pale.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let error: ApiError = json_body(pale).await;
    assert!(error.message.contains("4.5:1"), "{}", error.message);

    for bad in [
        json!({ "practice_name": "", "brand_primary": "#1B3A5C", "brand_highlight": "#E8693F" }),
        json!({ "practice_name": "x".repeat(101), "brand_primary": "#1B3A5C", "brand_highlight": "#E8693F" }),
        json!({ "practice_name": "X", "brand_primary": "navy", "brand_highlight": "#E8693F" }),
        json!({ "practice_name": "X", "brand_primary": "#1B3A5C", "brand_highlight": "#E869" }),
    ] {
        let response = client
            .send(Method::PUT, "/api/staff/settings", Some(bad))
            .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    let unchanged: PracticeSettings =
        json_body(client.send(Method::GET, "/api/staff/settings", None).await).await;
    assert_eq!(unchanged, saved);
}

#[tokio::test]
async fn settings_update_needs_csrf() {
    let mut client = Client::new(app(Mode::Clinical).await);
    set_up(&mut client).await;
    client.csrf = None;
    let response = client
        .send(
            Method::PUT,
            "/api/staff/settings",
            Some(json!({ "practice_name": "X", "brand_primary": "#000000", "brand_highlight": "#E8693F" })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(error_code(response).await, "bad_csrf");
}
