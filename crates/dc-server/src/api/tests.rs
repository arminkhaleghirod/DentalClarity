//! End-to-end tests of the API access rules, run against an in-memory database.

use crate::config::HttpConfig;
use crate::generation::Generation;
use crate::jobs::Jobs;
use crate::limits::Limits;
use crate::{AppState, router, storage};
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use dc_ai::fake::FakeTextModel;
use dc_ai::{Role, TextModel, TextRequest, TextResponse};
use dc_core::Mode;
use dc_core::api::{
    ApiError, HealthResponse, PatientDetail, PatientListItem, PatientRow, PracticeSettings,
    PresentationStatus, SessionInfo, SessionState,
};
use dc_core::presentation::{
    BlockSource, CreatePresentationResponse, JobState, JobStatus, PresentationSummary,
    PresentationView, SectionStage, SectionVisibility,
};
use dc_knowledge::KnowledgeBase;
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
    let (app, db, _) = app_with(mode, |_| None).await;
    (app, db)
}

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The app with the repository's knowledge base and, optionally, a
/// generation setup built on the test database.
async fn app_with(
    mode: Mode,
    generation: impl FnOnce(&SqlitePool) -> Option<Generation>,
) -> (Router, SqlitePool, AppState) {
    app_with_narration(mode, generation, |db, _| {
        crate::narration::Narration::unavailable(mode, db.clone())
    })
    .await
}

/// [`app_with`] with narration built by `narration` from the database and
/// the generation setup.
async fn app_with_narration(
    mode: Mode,
    generation: impl FnOnce(&SqlitePool) -> Option<Generation>,
    narration: impl FnOnce(&SqlitePool, Option<&Generation>) -> crate::narration::Narration,
) -> (Router, SqlitePool, AppState) {
    let root = repo_root();
    let pms =
        SyntheticConnector::from_path(&root.join("fixtures/synthetic/practice.toml")).unwrap();
    let db = storage::open_in_memory().await.unwrap();
    let kb = KnowledgeBase::load(&root.join("knowledge")).unwrap();
    let generation = generation(&db);
    let state = AppState {
        mode,
        http: Arc::new(HttpConfig {
            public_origin: ORIGIN.into(),
            trusted_proxy: None,
            client_ip_header: None,
            hsts: false,
        }),
        db: db.clone(),
        default_practice_name: pms.practice_name().to_owned(),
        pms: Arc::new(pms),
        codes: Arc::new(kb.codes.clone()),
        kb: Arc::new(kb),
        narration: Arc::new(narration(&db, generation.as_ref())),
        generation: generation.map(Arc::new),
        jobs: Arc::new(Jobs::default()),
        limits: Arc::new(Limits::default()),
        setup_code: Arc::new(std::sync::Mutex::new(Some(SETUP_CODE.into()))),
    };
    (router(state.clone()), db, state)
}

/// A browser-like client that keeps the session cookie, the present
/// session cookie and the CSRF token.
struct Client {
    app: Router,
    cookie: Option<String>,
    present: Option<String>,
    csrf: Option<String>,
}

impl Client {
    fn new(app: Router) -> Self {
        Self {
            app,
            cookie: None,
            present: None,
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
        let cookies: Vec<&str> = [&self.cookie, &self.present]
            .into_iter()
            .filter_map(|c| c.as_deref())
            .collect();
        if !cookies.is_empty() {
            req = req.header(header::COOKIE, cookies.join("; "));
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
        for set in response.headers().get_all(header::SET_COOKIE) {
            let pair = set.to_str().unwrap().split(';').next().unwrap().to_owned();
            let value = (!pair.ends_with('=')).then_some(pair.clone());
            if pair.starts_with("dc_present=") {
                self.present = value;
            } else {
                self.cookie = value;
            }
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
async fn full_session_limiters_do_not_lock_out_new_visitors() {
    let (app, _, state) = app_with(Mode::Demo, |_| None).await;
    let minute = std::time::Duration::from_secs(60);
    for i in 0..10_000 {
        let key = format!("filler-{i}");
        state.limits.staff_api.check(&key, 300, minute).unwrap();
        state.limits.present_api.check(&key, 300, minute).unwrap();
        state.limits.edits.check(&key, 60, minute).unwrap();
    }
    let mut client = Client::new(app);
    let response = client.send(Method::POST, "/api/visit", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    client.session().await;
    let response = client.send(Method::GET, "/api/staff/dashboard", None).await;
    assert_eq!(response.status(), StatusCode::OK);
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

// Presentations and generation jobs ------------------------------------------

/// Routes each writer call to a scripted `FakeTextModel` for its section, so
/// replies stay deterministic while sections run in parallel. The section is
/// read from the task in the first user message.
struct SectionRouter {
    by_section: std::collections::HashMap<String, FakeTextModel>,
    /// Calls wait here when set, so a test can watch a job while it runs.
    gate: Option<Arc<tokio::sync::Semaphore>>,
}

fn section_of(request: &TextRequest) -> String {
    let task = request
        .messages
        .iter()
        .find(|m| m.role == Role::User)
        .and_then(|m| m.content.rsplit_once("<task>"))
        .map(|(_, task)| task)
        .unwrap_or_default();
    if let Some(rest) = task.strip_prefix("Write the explanation for procedure ") {
        let reference: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        format!("procedure:{reference}")
    } else if task.contains("at a glance") {
        "at_a_glance".into()
    } else if task.contains("treatment journey") {
        "journey".into()
    } else {
        "next_steps".into()
    }
}

#[async_trait::async_trait]
impl TextModel for SectionRouter {
    fn provider(&self) -> &'static str {
        "fake"
    }

    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        if let Some(gate) = &self.gate {
            gate.acquire().await.unwrap().forget();
        }
        let key = section_of(&request);
        self.by_section
            .get(&key)
            .unwrap_or_else(|| panic!("no script for {key}"))
            .generate(request)
            .await
    }
}

fn sentence(text: &str, cite: &str) -> serde_json::Value {
    json!({ "text": text, "cites": [cite] })
}

fn questions() -> serde_json::Value {
    json!([
        { "text": "What should I expect afterwards?", "cites": [] },
        { "text": "How should I care for this area?", "cites": [] }
    ])
}

/// A reply for one section of Daniel Okafor's plan that passes every check.
fn good_reply(key: &str) -> String {
    match key {
        "at_a_glance" => json!({
            "summary": [
                sentence("This plan cares for several of your teeth.", "fact:ph1"),
                sentence("Each part of the plan is explained in its own section.", "fact:ph2")
            ],
            "objective": []
        }),
        "journey" => json!({
            "phases": [
                { "phase_ref": "ph1", "summary": [sentence("This phase covers the treatments listed for it.", "fact:ph1")] },
                { "phase_ref": "ph2", "summary": [sentence("This phase covers the treatments listed for it.", "fact:ph2")] }
            ],
            "dependencies": []
        }),
        "next_steps" => json!({
            "outlook": [sentence("The plan aims to keep your teeth healthy and comfortable.", "fact:ph1")],
            "next_steps": [sentence("You can talk with your dental team about any part of the plan.", "fact:ph2")],
            "questions": questions()
        }),
        procedure => {
            let r = procedure.trim_start_matches("procedure:");
            let fact = format!("fact:{r}");
            let why = if r == "p1" {
                json!({ "kind": "documented", "sentences": [sentence("Your dentist found decay that reached the nerve of this tooth.", "note:p1")] })
            } else {
                json!({ "kind": "common_reasons", "sentences": [sentence("Common reasons for this treatment include keeping teeth healthy.", &fact)] })
            };
            json!({
                "procedure_ref": r,
                "what_it_is": [sentence("This treatment cares for the affected area.", &fact)],
                "why": why,
                "what_happens": [sentence("Your dental team explains each step as it happens.", &fact)],
                "recovery": [sentence("Many people feel comfortable afterwards.", &fact)],
                "benefits": [sentence("It can help keep your mouth healthy.", &fact)],
                "if_delayed": [sentence("Problems in a tooth can sometimes grow if they are left alone.", &fact)],
                "questions": questions()
            })
        }
    }
    .to_string()
}

const DANIEL_SECTIONS: [&str; 9] = [
    "at_a_glance",
    "journey",
    "procedure:p1",
    "procedure:p2",
    "procedure:p3",
    "procedure:p4",
    "procedure:p5",
    "procedure:p6",
    "next_steps",
];

/// Scripted models for `runs` generations of Daniel's plan. Sections listed
/// in `failing` get a reply that always fails the checks (a guarantee).
fn scripted(
    runs: usize,
    failing: &[&str],
    gate: Option<Arc<tokio::sync::Semaphore>>,
) -> impl FnOnce(&SqlitePool) -> Option<Generation> {
    let failing: Vec<String> = failing.iter().map(|s| s.to_string()).collect();
    move |db: &SqlitePool| {
        let by_section = DANIEL_SECTIONS
            .iter()
            .map(|key| {
                let reply = if failing.iter().any(|f| f == key) {
                    good_reply(key).replace(
                        "It can help keep your mouth healthy.",
                        "Success is guaranteed.",
                    )
                } else {
                    good_reply(key)
                };
                // One reply per attempt: a failing section uses all three.
                (
                    key.to_string(),
                    FakeTextModel::with_responses(vec![reply; runs * 3]),
                )
            })
            .collect();
        let pass = json!({ "verdict": "pass", "issues": [] }).to_string();
        let reviewer = FakeTextModel::with_responses(vec![pass; runs * DANIEL_SECTIONS.len()]);
        Some(Generation::new(
            db.clone(),
            Some(25.0),
            Arc::new(SectionRouter { by_section, gate }),
            "fake-writer".into(),
            Arc::new(reviewer),
            "fake-reviewer".into(),
            None,
        ))
    }
}

async fn visitor_of(app: &Router) -> Client {
    let mut client = Client::new(app.clone());
    client.send(Method::POST, "/api/visit", None).await;
    client.session().await;
    client
}

fn daniel_request() -> serde_json::Value {
    json!({ "patient_key": "10482", "plan_key": "tp-10482-1", "language": "en", "reading_level": "standard" })
}

async fn create(client: &mut Client) -> Response {
    client
        .send(
            Method::POST,
            "/api/staff/presentations",
            Some(daniel_request()),
        )
        .await
}

async fn wait_for_job(client: &mut Client, job_id: &str) -> JobStatus {
    for _ in 0..500 {
        let status: JobStatus = json_body(
            client
                .send(Method::GET, &format!("/api/staff/jobs/{job_id}"), None)
                .await,
        )
        .await;
        if status.state.is_finished() {
            return status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("job {job_id} did not finish");
}

#[tokio::test]
async fn generated_presentation_has_sections_facts_and_states() {
    let (app, db, _) = app_with(Mode::Demo, scripted(1, &["procedure:p6"], None)).await;
    let mut client = visitor_of(&app).await;
    let response = create(&mut client).await;
    assert_eq!(response.status(), StatusCode::OK);
    let created: CreatePresentationResponse = json_body(response).await;

    let status = wait_for_job(&mut client, &created.job_id).await;
    assert_eq!(status.state, JobState::NeedsReview, "{:?}", status.error);
    assert_eq!(status.presentation_id, created.presentation_id);
    let labels: Vec<&str> = status.sections.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels[0], "At a glance");
    assert_eq!(labels[1], "Treatment journey");
    assert_eq!(labels[2], "Root canal · #30");
    assert_eq!(labels[7], "Cleaning");
    assert_eq!(labels[8], "What happens next");
    assert_eq!(status.sections[7].stage, SectionStage::Failed);
    assert!(
        status.sections[..7]
            .iter()
            .all(|s| s.stage == SectionStage::Passed)
    );

    let view: PresentationView = json_body(
        client
            .send(
                Method::GET,
                &format!("/api/staff/presentations/{}", created.presentation_id),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(view.job_state, JobState::NeedsReview);
    assert_eq!(view.patient_display_name, "Daniel Okafor");
    assert_eq!(view.patient_first_name, "Daniel");
    assert_eq!(view.writer_model, "fake-writer");
    assert_eq!(view.prompt_version, dc_generation::prompt::writer_version());
    assert_eq!(view.procedures.len(), 6);
    assert_eq!(view.procedures[0].tooth, "30");
    assert_eq!(view.procedures[3].phase, 2);
    assert_eq!(
        view.total_patient_estimate.map(|m| m.cents()),
        Some(136_900)
    );
    assert_eq!(view.sections.len(), 9);

    let root_canal = &view.sections[2];
    assert_eq!(root_canal.visibility, SectionVisibility::Ready);
    assert_eq!(root_canal.procedure_ref.as_deref(), Some("p1"));
    let fields: Vec<&str> = root_canal.blocks.iter().map(|b| b.field.as_str()).collect();
    assert_eq!(
        fields,
        [
            "what_it_is",
            "why",
            "what_happens",
            "recovery",
            "benefits",
            "if_delayed"
        ]
    );
    let why = &root_canal.blocks[1];
    assert_eq!(why.heading, "Why your dentist recommended it");
    assert_eq!(why.source, BlockSource::ClinicalNote);
    assert!(why.needs_confirmation);
    assert_eq!(root_canal.questions.len(), 2);

    let buildup_why = &view.sections[3].blocks[1];
    assert_eq!(buildup_why.source, BlockSource::Generated);
    assert!(!buildup_why.needs_confirmation);

    let cleaning = &view.sections[7];
    assert_eq!(cleaning.visibility, SectionVisibility::Failed);
    assert!(cleaning.blocks.is_empty());
    assert!(cleaning.reasons.iter().any(|r| r.contains("V11")));

    assert_eq!(view.sections[1].blocks[1].heading, "Phase 2");
    // No model text ever carries money; costs come from the snapshot.
    let json = serde_json::to_string(&view.sections).unwrap();
    assert!(!json.contains('$'));

    let list: Vec<PresentationSummary> = json_body(
        client
            .send(Method::GET, "/api/staff/presentations", None)
            .await,
    )
    .await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].sections_ready, 8);
    assert_eq!(list[0].sections_total, 9);
    assert_eq!(list[0].plan_name, "Active");

    let (calls,): (i64,) = sqlx::query_as("SELECT calls FROM ai_usage")
        .fetch_one(&db)
        .await
        .unwrap();
    // 9 writer calls, 2 repairs for the failing section, 8 reviews.
    assert_eq!(calls, 19);
}

#[tokio::test]
async fn all_sections_passing_means_succeeded() {
    let (app, _, _) = app_with(Mode::Demo, scripted(1, &[], None)).await;
    let mut client = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    let status = wait_for_job(&mut client, &created.job_id).await;
    assert_eq!(status.state, JobState::Succeeded);
}

#[tokio::test]
async fn dashboard_reflects_presentation_state_in_scope() {
    let (app, _db, _) = app_with(Mode::Demo, scripted(1, &[], None)).await;
    let mut client = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    wait_for_job(&mut client, &created.job_id).await;

    let rows: Vec<PatientRow> =
        json_body(client.send(Method::GET, "/api/staff/dashboard", None).await).await;
    let daniel = rows
        .iter()
        .find(|r| r.display_name == "Daniel Okafor")
        .unwrap();
    assert_eq!(daniel.status, PresentationStatus::Ready);
    assert_eq!(
        daniel.presentation_id.as_deref(),
        Some(created.presentation_id.as_str())
    );
    let maria = rows
        .iter()
        .find(|r| r.display_name == "Maria Elena Ruiz")
        .unwrap();
    assert_eq!(maria.status, PresentationStatus::NotStarted);
    assert!(maria.presentation_id.is_none());

    // Another visitor does not see this visitor's presentation on their dashboard.
    let mut other = visitor_of(&app).await;
    let rows: Vec<PatientRow> =
        json_body(other.send(Method::GET, "/api/staff/dashboard", None).await).await;
    let daniel = rows
        .iter()
        .find(|r| r.display_name == "Daniel Okafor")
        .unwrap();
    assert_eq!(daniel.status, PresentationStatus::NotStarted);
}

#[tokio::test]
async fn progress_is_visible_while_the_job_runs() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let (app, _, _) = app_with(Mode::Demo, scripted(1, &[], Some(gate.clone()))).await;
    let mut client = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;

    let mut seen_writing = false;
    for _ in 0..300 {
        let status: JobStatus = json_body(
            client
                .send(
                    Method::GET,
                    &format!("/api/staff/jobs/{}", created.job_id),
                    None,
                )
                .await,
        )
        .await;
        if status.state == JobState::Running
            && status
                .sections
                .iter()
                .any(|s| s.stage == SectionStage::Writing)
        {
            seen_writing = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(seen_writing);

    // One job per visitor at a time.
    let second = create(&mut client).await;
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(second.headers().get(header::RETRY_AFTER).is_some());
    assert_eq!(error_code(second).await, "generation_in_progress");

    gate.add_permits(1000);
    assert_eq!(
        wait_for_job(&mut client, &created.job_id).await.state,
        JobState::Succeeded
    );
}

#[tokio::test]
async fn visitors_only_see_their_own_presentations() {
    let (app, _, _) = app_with(Mode::Demo, scripted(1, &[], None)).await;
    let mut alice = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut alice).await).await;
    wait_for_job(&mut alice, &created.job_id).await;

    let mut bob = visitor_of(&app).await;
    let view = bob
        .send(
            Method::GET,
            &format!("/api/staff/presentations/{}", created.presentation_id),
            None,
        )
        .await;
    assert_eq!(view.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(view).await, "presentation_not_found");
    let job = bob
        .send(
            Method::GET,
            &format!("/api/staff/jobs/{}", created.job_id),
            None,
        )
        .await;
    assert_eq!(job.status(), StatusCode::NOT_FOUND);
    let list: Vec<PresentationSummary> = json_body(
        bob.send(Method::GET, "/api/staff/presentations", None)
            .await,
    )
    .await;
    assert!(list.is_empty());
    let unknown = alice
        .send(Method::GET, "/api/staff/presentations/does-not-exist", None)
        .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn creating_a_presentation_needs_csrf() {
    let (app, _, _) = app_with(Mode::Demo, scripted(1, &[], None)).await;
    let mut client = visitor_of(&app).await;
    client.csrf = None;
    let response = create(&mut client).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(error_code(response).await, "bad_csrf");
}

#[tokio::test]
async fn clinical_mode_has_no_provider_yet() {
    let mut client = Client::new(app(Mode::Clinical).await);
    set_up(&mut client).await;
    let response = create(&mut client).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(response).await, "provider_not_configured");
}

#[tokio::test]
async fn create_validates_the_request() {
    let (app, _, _) = app_with(Mode::Demo, scripted(1, &[], None)).await;
    let mut client = visitor_of(&app).await;
    for (body, status, code) in [
        (
            json!({ "patient_key": "10482", "plan_key": "tp-10482-1", "language": "english!", "reading_level": "standard" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_language",
        ),
        (
            json!({ "patient_key": "99999", "plan_key": "tp-10482-1", "language": "en", "reading_level": "standard" }),
            StatusCode::NOT_FOUND,
            "patient_not_found",
        ),
        (
            // A real plan of another patient.
            json!({ "patient_key": "10482", "plan_key": "tp-10517-1", "language": "en", "reading_level": "standard" }),
            StatusCode::NOT_FOUND,
            "plan_not_found",
        ),
    ] {
        let response = client
            .send(Method::POST, "/api/staff/presentations", Some(body))
            .await;
        assert_eq!(response.status(), status);
        assert_eq!(error_code(response).await, code);
    }
    let list: Vec<PresentationSummary> = json_body(
        client
            .send(Method::GET, "/api/staff/presentations", None)
            .await,
    )
    .await;
    assert!(list.is_empty());
}

#[tokio::test]
async fn visitors_get_three_generations_per_hour() {
    let (app, _, _) = app_with(Mode::Demo, scripted(3, &[], None)).await;
    let mut client = visitor_of(&app).await;
    for _ in 0..3 {
        let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
        wait_for_job(&mut client, &created.job_id).await;
    }
    let fourth = create(&mut client).await;
    assert_eq!(fourth.status(), StatusCode::TOO_MANY_REQUESTS);
    let wait: u64 = fourth.headers()[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=3600).contains(&wait));
    assert_eq!(error_code(fourth).await, "rate_limited");
    let list: Vec<PresentationSummary> = json_body(
        client
            .send(Method::GET, "/api/staff/presentations", None)
            .await,
    )
    .await;
    assert_eq!(list.len(), 3);
}

#[tokio::test]
async fn new_generations_stop_at_80_percent_of_the_budget() {
    let (app, db, _) = app_with(Mode::Demo, scripted(1, &[], None)).await;
    sqlx::query(
        "INSERT INTO ai_usage (month, cost_usd, calls) VALUES (strftime('%Y-%m', 'now'), 20.0, 1)",
    )
    .execute(&db)
    .await
    .unwrap();
    let mut client = visitor_of(&app).await;
    let response = create(&mut client).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(response).await, "budget_reached");
}

#[tokio::test]
async fn interrupted_jobs_run_again_from_the_start() {
    let (app, db, state) = app_with(Mode::Demo, scripted(2, &[], None)).await;
    let mut client = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    wait_for_job(&mut client, &created.job_id).await;

    // Simulate a stop in the middle of the job.
    sqlx::query("UPDATE jobs SET state = 'running'")
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM presentation_sections")
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(crate::jobs::resume_pending(&state).await.unwrap(), 1);
    let status = wait_for_job(&mut client, &created.job_id).await;
    assert_eq!(status.state, JobState::Succeeded);
    let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM presentation_sections")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(rows, 9);
}

#[tokio::test]
async fn demo_data_is_purged_after_a_day() {
    let (app, db, _) = app_with(Mode::Demo, scripted(1, &[], None)).await;
    let mut client = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    wait_for_job(&mut client, &created.job_id).await;
    assert_eq!(crate::jobs::purge_demo_data(&db).await.unwrap(), 0);
    sqlx::query("UPDATE presentations SET created_at = created_at - 86400")
        .execute(&db)
        .await
        .unwrap();
    // A regeneration from a minute ago and one from a day ago.
    let now_ms = crate::jobs::now_ms();
    for at in [now_ms - 60_000, now_ms - 86_401_000] {
        sqlx::query(
            "INSERT INTO regeneration_events (owner_scope, client_key, created_at)
             VALUES ('scope', 'address', ?)",
        )
        .bind(at)
        .execute(&db)
        .await
        .unwrap();
    }
    assert_eq!(crate::jobs::purge_demo_data(&db).await.unwrap(), 1);
    // The recent regeneration still counts after its presentation is gone.
    let (count, oldest): (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), MIN(created_at) FROM regeneration_events")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!((count, oldest), (1, now_ms - 60_000));
    for table in ["presentations", "presentation_sections", "jobs"] {
        let (n,): (i64,) = sqlx::query_as(match table {
            "presentations" => "SELECT COUNT(*) FROM presentations",
            "jobs" => "SELECT COUNT(*) FROM jobs",
            _ => "SELECT COUNT(*) FROM presentation_sections",
        })
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(n, 0, "{table}");
    }
    let (spend_rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ai_usage")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(spend_rows, 1);
}

mod logo;
mod present;
mod review;
mod setup;
