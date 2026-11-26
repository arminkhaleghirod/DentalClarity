//! Present mode, the patient content API and staff edits.

use super::*;
use dc_core::present::{
    BlockChangeResponse, ExitPresentResponse, PatientView, StartPresentResponse, audio_key,
};
use dc_knowledge::entries::Topic;

const PIN: &str = "480913";

/// A demo visitor with a finished presentation of Daniel's plan.
async fn demo_presentation() -> (Router, SqlitePool, AppState, Client, String) {
    let (app, db, state) = app_with(Mode::Demo, scripted(1, &[], None)).await;
    let mut client = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    wait_for_job(&mut client, &created.job_id).await;
    (app, db, state, client, created.presentation_id)
}

async fn start_present(client: &mut Client, id: &str) -> Response {
    client
        .send(
            Method::POST,
            &format!("/api/staff/presentations/{id}/present"),
            None,
        )
        .await
}

async fn content(client: &mut Client) -> Response {
    client.send(Method::GET, "/api/present/content", None).await
}

async fn exit(client: &mut Client, body: serde_json::Value) -> Response {
    client
        .send(Method::POST, "/api/present/exit", Some(body))
        .await
}

async fn view(client: &mut Client, id: &str) -> PresentationView {
    json_body(
        client
            .send(Method::GET, &format!("/api/staff/presentations/{id}"), None)
            .await,
    )
    .await
}

async fn put_block(client: &mut Client, id: &str, body: serde_json::Value) -> Response {
    client
        .send(
            Method::PUT,
            &format!("/api/staff/presentations/{id}/blocks"),
            Some(body),
        )
        .await
}

async fn post_block(
    client: &mut Client,
    id: &str,
    action: &str,
    key: &str,
    field: &str,
) -> Response {
    client
        .send(
            Method::POST,
            &format!("/api/staff/presentations/{id}/{action}"),
            Some(json!({ "section_key": key, "field": field })),
        )
        .await
}

async fn audit_count(db: &SqlitePool, action: &str) -> i64 {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM audit_events WHERE action = ?")
        .bind(action)
        .fetch_one(db)
        .await
        .unwrap();
    n
}

fn root_canal_reasons(state: &AppState) -> String {
    state
        .kb
        .entries()
        .find(|e| e.id == "root_canal")
        .unwrap()
        .passage(Topic::CommonReasons)
        .unwrap()
        .text
        .clone()
}

#[tokio::test]
async fn present_locks_the_session_and_shows_only_ready_content() {
    let (app, db, state, mut client, id) = demo_presentation().await;
    // Hold one section for review.
    sqlx::query(
        "UPDATE presentation_sections SET state = 'needs_review' WHERE key = 'procedure:p2'",
    )
    .execute(&db)
    .await
    .unwrap();

    let response = start_present(&mut client, &id).await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cookie.starts_with("dc_present="));
    assert!(cookie.contains("HttpOnly; SameSite=Strict"));
    let started: StartPresentResponse = json_body(response).await;
    assert_eq!(started.present_path, "/present");
    assert!(client.present.is_some());
    assert_eq!(audit_count(&db, "present_started").await, 1);

    // The visitor session is locked on the server.
    let dashboard = client.send(Method::GET, "/api/staff/dashboard", None).await;
    assert_eq!(dashboard.status(), StatusCode::LOCKED);
    assert_eq!(client.session().await.state, SessionState::Locked);
    let again = start_present(&mut client, &id).await;
    assert_eq!(again.status(), StatusCode::LOCKED);

    let response = content(&mut client).await;
    assert_eq!(response.status(), StatusCode::OK);
    let patient: PatientView = json_body(response).await;
    assert!(patient.demo);
    assert_eq!(patient.patient_first_name, "Daniel");
    assert!(!patient.guardian_audience);
    assert_eq!(patient.provider_display_name.as_deref(), Some("Dr. Patel"));
    assert_eq!(patient.brand.name, "Maple Grove Family Dental");
    assert_eq!(patient.brand.primary, "#1B3A5C");
    assert_eq!(patient.language, "en");
    assert_eq!(patient.available_languages, ["en"]);
    assert_eq!(patient.procedures.len(), 6);
    assert!(patient.affected_teeth.contains(&"30".to_owned()));
    assert_eq!(
        patient.total_patient_estimate.map(|m| m.cents()),
        Some(136_900)
    );
    assert_eq!(patient.total_fee.map(|m| m.cents()), Some(345_000));

    let keys: Vec<&str> = patient.sections.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys.len(), 8);
    assert!(!keys.contains(&"procedure:p2"));
    assert_eq!(keys[0], "at_a_glance");

    // The unconfirmed note-derived reason is replaced by common reasons.
    let root_canal = patient
        .sections
        .iter()
        .find(|s| s.key == "procedure:p1")
        .unwrap();
    let why = root_canal.blocks.iter().find(|b| b.field == "why").unwrap();
    assert_eq!(why.heading, "Why this is done");
    assert_eq!(why.source, BlockSource::Generated);
    let joined: Vec<&str> = why.sentences.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(joined.join(" "), root_canal_reasons(&state));
    let json = serde_json::to_string(&patient.sections).unwrap();
    assert!(!json.contains("decay that reached the nerve"));

    // Every sentence carries the key of its exact text.
    let voice = crate::narration::speech_model_id(&state);
    for sentence in patient
        .sections
        .iter()
        .flat_map(|s| &s.blocks)
        .flat_map(|b| &b.sentences)
    {
        assert_eq!(sentence.audio_key, audio_key(voice, "en", &sentence.text));
    }

    // Audio: keys outside this version are unknown; known ones reach the
    // narration service, which has no provider in tests.
    let unknown = client
        .send(
            Method::GET,
            &format!("/api/present/audio/{}", "a".repeat(64)),
            None,
        )
        .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let key = &why.sentences[0].audio_key;
    let known = client
        .send(Method::GET, &format!("/api/present/audio/{key}"), None)
        .await;
    assert_eq!(error_code(known).await, "narration_unavailable");
    drop(app);
}

#[tokio::test]
async fn staff_and_present_cookies_never_open_each_others_routes() {
    let (app, _, _, mut client, id) = demo_presentation().await;
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );

    let mut staff_only = Client::new(app.clone());
    staff_only.cookie = client.cookie.clone();
    let response = content(&mut staff_only).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(response).await, "present_ended");
    let narration = format!("/api/present/narration?keys={}", "a".repeat(64));
    let response = staff_only.send(Method::GET, &narration, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = exit(&mut staff_only, json!({})).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let mut present_only = Client::new(app.clone());
    present_only.present = client.present.clone();
    for path in [
        "/api/staff/dashboard",
        &format!("/api/staff/presentations/{id}"),
    ] {
        let response = present_only.send(Method::GET, path, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    assert_eq!(content(&mut present_only).await.status(), StatusCode::OK);

    // Exit from another origin is refused.
    let request = Request::post("/api/present/exit")
        .header(header::ORIGIN, "https://evil.example")
        .header(header::COOKIE, client.present.clone().unwrap())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(error_code(response).await, "bad_origin");
}

#[tokio::test]
async fn demo_exit_needs_no_pin_and_rotates_the_session() {
    let (app, db, _, mut client, id) = demo_presentation().await;
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let old_cookie = client.cookie.clone();
    let old_present = client.present.clone();

    let response = exit(&mut client, json!({})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let exited: ExitPresentResponse = json_body(response).await;
    assert_eq!(exited.staff_path, format!("/presentations/{id}"));
    assert!(client.present.is_none());
    assert_ne!(client.cookie, old_cookie);
    assert_eq!(audit_count(&db, "present_exited").await, 1);

    // The new session is unlocked and still owns the presentation.
    client.csrf = Some(exited.csrf_token.clone());
    let info = client.session().await;
    assert_eq!(info.state, SessionState::Visitor);
    assert_eq!(info.csrf_token.as_deref(), Some(exited.csrf_token.as_str()));
    assert_eq!(view(&mut client, &id).await.id, id);

    // The old session token and the old present cookie stop working.
    let mut old = Client::new(app.clone());
    old.cookie = old_cookie;
    assert_eq!(
        old.send(Method::GET, "/api/staff/dashboard", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let mut old_patient = Client::new(app);
    old_patient.present = old_present;
    assert_eq!(
        content(&mut old_patient).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn an_expired_present_session_leaves_the_session_locked() {
    let (_, db, _, mut client, id) = demo_presentation().await;
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    sqlx::query("UPDATE present_sessions SET last_seen_at = last_seen_at - 3600")
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        content(&mut client).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        exit(&mut client, json!({})).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let info = client.session().await;
    assert_eq!(info.state, SessionState::Locked);
    assert_eq!(
        client
            .send(Method::GET, "/api/staff/dashboard", None)
            .await
            .status(),
        StatusCode::LOCKED
    );
    // Sign-out still works from the lock screen.
    let logout = client.send(Method::POST, "/api/auth/logout", None).await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert_eq!(client.session().await.state, SessionState::NewVisitor);
}

#[tokio::test]
async fn present_needs_a_finished_job_with_a_ready_section() {
    let (app, db, _, mut client, id) = demo_presentation().await;
    sqlx::query("UPDATE presentation_sections SET state = 'needs_review'")
        .execute(&db)
        .await
        .unwrap();
    let response = start_present(&mut client, &id).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(response).await, "nothing_to_present");
    sqlx::query("UPDATE jobs SET state = 'running'")
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        error_code(start_present(&mut client, &id).await).await,
        "not_ready"
    );
    // Nothing was locked.
    assert_eq!(client.session().await.state, SessionState::Visitor);

    // Another visitor cannot present or edit it.
    let mut other = visitor_of(&app).await;
    let response = start_present(&mut other, &id).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = put_block(
        &mut other,
        &id,
        json!({ "section_key": "procedure:p1", "field": "what_it_is", "sentences": ["Hi."] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(response).await, "presentation_not_found");
    let response = post_block(&mut other, &id, "confirm-note", "procedure:p1", "why").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn clinical_exit_needs_the_pin_then_the_password() {
    let (app, db, _) = app_with(Mode::Clinical, scripted(1, &[], None)).await;
    let mut client = Client::new(app);
    set_up(&mut client).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    wait_for_job(&mut client, &created.job_id).await;
    let id = created.presentation_id;

    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let patient: PatientView = json_body(content(&mut client).await).await;
    assert!(!patient.demo);

    let response = exit(&mut client, json!({})).await;
    assert_eq!(error_code(response).await, "pin_required");
    for _ in 0..4 {
        let response = exit(&mut client, json!({ "pin": "111222" })).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(error_code(response).await, "invalid_pin");
    }
    let fifth = exit(&mut client, json!({ "pin": "111222" })).await;
    assert_eq!(fifth.status(), StatusCode::FORBIDDEN);
    assert_eq!(error_code(fifth).await, "pin_disabled");
    // Now even the right PIN is refused.
    let right_pin = exit(&mut client, json!({ "pin": PIN })).await;
    assert_eq!(error_code(right_pin).await, "pin_disabled");
    assert_eq!(client.session().await.state, SessionState::Locked);

    let wrong = exit(&mut client, json!({ "password": "not the password" })).await;
    assert_eq!(error_code(wrong).await, "invalid_credentials");
    let response = exit(&mut client, json!({ "password": PASSWORD })).await;
    assert_eq!(response.status(), StatusCode::OK);
    let exited: ExitPresentResponse = json_body(response).await;
    client.csrf = Some(exited.csrf_token);
    assert_eq!(client.session().await.state, SessionState::Staff);
    assert_eq!(view(&mut client, &id).await.id, id);

    assert_eq!(audit_count(&db, "present_exit_pin_failed").await, 5);
    assert_eq!(audit_count(&db, "present_pin_disabled").await, 1);
    assert_eq!(audit_count(&db, "present_exit_password_failed").await, 1);
    assert_eq!(audit_count(&db, "present_exited").await, 1);

    // A fresh lock gets a fresh PIN allowance, and the right PIN unlocks.
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let response = exit(&mut client, json!({ "pin": PIN })).await;
    assert_eq!(response.status(), StatusCode::OK);
    let exited: ExitPresentResponse = json_body(response).await;
    client.csrf = Some(exited.csrf_token);
    assert_eq!(client.session().await.state, SessionState::Staff);
}

#[tokio::test]
async fn staff_edits_confirmations_and_reverts_reach_the_patient_view() {
    let (_, db, _, mut client, id) = demo_presentation().await;

    // The documented reason waits for confirmation.
    let why = &view(&mut client, &id).await.sections[2].blocks[1];
    assert_eq!(why.source, BlockSource::ClinicalNote);
    assert!(why.needs_confirmation);

    // Writes need the CSRF token.
    let saved = client.csrf.take();
    let edit = json!({
        "section_key": "procedure:p1",
        "field": "what_happens",
        "sentences": [
            "  The team at Maple Grove Family Dental explains each step.  ",
            "Dr. Patel checks the tooth first.",
            "Success is guaranteed."
        ]
    });
    let forged = put_block(&mut client, &id, edit.clone()).await;
    assert_eq!(error_code(forged).await, "bad_csrf");
    client.csrf = saved;

    let response = put_block(&mut client, &id, edit).await;
    assert_eq!(response.status(), StatusCode::OK);
    let changed: BlockChangeResponse = json_body(response).await;
    assert_eq!(changed.warnings.len(), 1, "{:?}", changed.warnings);
    assert!(changed.warnings[0].starts_with("Sentence 3:"));
    assert!(changed.warnings[0].contains("V11"));
    let block = changed
        .section
        .blocks
        .iter()
        .find(|b| b.field == "what_happens")
        .unwrap();
    assert_eq!(block.source, BlockSource::StaffEdit);
    assert_eq!(
        block.sentences[0],
        "The team at Maple Grove Family Dental explains each step."
    );

    let confirmed = post_block(&mut client, &id, "confirm-note", "procedure:p1", "why").await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let confirmed: BlockChangeResponse = json_body(confirmed).await;
    assert!(!confirmed.section.blocks[1].needs_confirmation);
    let staff_view = view(&mut client, &id).await;
    let root_canal = &staff_view.sections[2];
    assert_eq!(root_canal.blocks[1].source, BlockSource::ClinicalNote);
    assert!(!root_canal.blocks[1].needs_confirmation);
    assert_eq!(root_canal.blocks[2].source, BlockSource::StaffEdit);

    // Only unedited note-derived blocks can be confirmed.
    let response = post_block(
        &mut client,
        &id,
        "confirm-note",
        "procedure:p1",
        "what_it_is",
    )
    .await;
    assert_eq!(error_code(response).await, "not_confirmable");
    let response = post_block(&mut client, &id, "confirm-note", "procedure:p2", "why").await;
    assert_eq!(error_code(response).await, "not_confirmable");

    // Patients see the edit with placeholders filled and the confirmed reason.
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let patient: PatientView = json_body(content(&mut client).await).await;
    let section = patient
        .sections
        .iter()
        .find(|s| s.key == "procedure:p1")
        .unwrap();
    let texts = |field: &str| -> Vec<String> {
        section
            .blocks
            .iter()
            .find(|b| b.field == field)
            .unwrap()
            .sentences
            .iter()
            .map(|s| s.text.clone())
            .collect()
    };
    assert_eq!(
        texts("what_happens")[..2],
        [
            "The team at Maple Grove Family Dental explains each step.",
            "Dr. Patel checks the tooth first."
        ]
    );
    let why = section.blocks.iter().find(|b| b.field == "why").unwrap();
    assert_eq!(why.source, BlockSource::ClinicalNote);
    assert_eq!(why.heading, "Why your dentist recommended it");
    assert_eq!(
        texts("why"),
        ["Your dentist found decay that reached the nerve of this tooth."]
    );
    let exited: ExitPresentResponse = json_body(exit(&mut client, json!({})).await).await;
    client.csrf = Some(exited.csrf_token);

    // Revert restores the generated text.
    let reverted = post_block(
        &mut client,
        &id,
        "blocks/revert",
        "procedure:p1",
        "what_happens",
    )
    .await;
    assert_eq!(reverted.status(), StatusCode::OK);
    let block = &view(&mut client, &id).await.sections[2].blocks[2];
    assert_eq!(block.source, BlockSource::Generated);
    assert_eq!(
        block.sentences,
        ["Your dental team explains each step as it happens."]
    );

    // Editing the note-derived reason makes it staff text, which needs no confirmation.
    let response = put_block(
        &mut client,
        &id,
        json!({ "section_key": "at_a_glance", "field": "summary", "sentences": ["A short plan summary."] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = put_block(
        &mut client,
        &id,
        json!({ "section_key": "procedure:p1", "field": "why", "sentences": ["The tooth needs care inside."] }),
    )
    .await;
    let changed: BlockChangeResponse = json_body(response).await;
    assert!(changed.warnings.is_empty(), "{:?}", changed.warnings);
    assert_eq!(changed.section.blocks[1].source, BlockSource::StaffEdit);
    assert!(!changed.section.blocks[1].needs_confirmation);
    let response = post_block(&mut client, &id, "confirm-note", "procedure:p1", "why").await;
    assert_eq!(error_code(response).await, "not_confirmable");

    // The original generated text is kept beside the edit.
    let (original,): (String,) = sqlx::query_as(
        "SELECT original_json FROM staff_edits WHERE section_key = 'procedure:p1' AND field = 'why'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(original.contains("decay that reached the nerve"));

    assert_eq!(audit_count(&db, "block_edited").await, 3);
    assert_eq!(audit_count(&db, "block_reverted").await, 1);
    assert_eq!(audit_count(&db, "note_confirmed").await, 1);
    let (details,): (String,) =
        sqlx::query_as("SELECT group_concat(detail, '|') FROM audit_events")
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(!details.contains("Daniel") && !details.contains("explains"));
}

#[tokio::test]
async fn edits_are_checked_against_the_limits() {
    let (_, _, _, mut client, id) = demo_presentation().await;
    let edit = |sentences: serde_json::Value| json!({ "section_key": "procedure:p1", "field": "recovery", "sentences": sentences });
    for sentences in [
        json!([]),
        json!(vec!["Fine."; 13]),
        json!(["   "]),
        json!(["x".repeat(401)]),
        json!(vec!["x".repeat(400); 6]),
    ] {
        let response = put_block(&mut client, &id, edit(sentences)).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error_code(response).await, "invalid_edit");
    }
    let response = put_block(&mut client, &id, edit(json!(vec!["x".repeat(400); 5]))).await;
    assert_eq!(response.status(), StatusCode::OK);

    for (key, field) in [
        ("procedure:p1", "nope"),
        ("procedure:p9", "why"),
        ("journey", "why"),
    ] {
        let response = put_block(
            &mut client,
            &id,
            json!({ "section_key": key, "field": field, "sentences": ["Hi."] }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(error_code(response).await, "block_not_found");
    }
}

#[tokio::test]
async fn placeholders_in_staff_edits_are_rejected() {
    let (_, _, _, mut client, id) = demo_presentation().await;
    for text in [
        "Ask {{provider_display_name}} at the visit.",
        "Close }} brace.",
    ] {
        let response = put_block(
            &mut client,
            &id,
            json!({ "section_key": "procedure:p1", "field": "recovery", "sentences": [text] }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let error: ApiError = json_body(response).await;
        assert_eq!(error.code, "invalid_edit");
        assert!(error.message.contains("placeholders"));
    }
}

#[tokio::test]
async fn demo_edits_are_limited_per_visitor_even_across_present_mode() {
    let (_, _, _, mut client, id) = demo_presentation().await;
    let edit = |n: u32| json!({ "section_key": "procedure:p1", "field": "recovery", "sentences": [format!("Edit number {n}.")] });
    for n in 0..crate::api::edits::DEMO_EDITS_PER_HOUR {
        let response = put_block(&mut client, &id, edit(n)).await;
        assert_eq!(response.status(), StatusCode::OK, "edit {n}");
    }
    let response = put_block(&mut client, &id, edit(999)).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers().get(header::RETRY_AFTER).is_some());
    assert_eq!(error_code(response).await, "rate_limited");

    // Leaving Present mode rotates the session token but keeps the count.
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let exited: ExitPresentResponse = json_body(exit(&mut client, json!({})).await).await;
    client.csrf = Some(exited.csrf_token);
    let response = put_block(&mut client, &id, edit(1000)).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// A demo visitor with a finished presentation and narration backed by a
/// fake speech provider that shares the generation budget.
async fn demo_with_speech() -> (
    AppState,
    Client,
    String,
    Arc<crate::narration::test_support::FakeSpeech>,
    crate::narration::test_support::TempDir,
) {
    use crate::narration::test_support::{self, FakeSpeech, TempDir};
    let speech = Arc::new(FakeSpeech::default());
    let dir = TempDir::new();
    let (app, _, state) = app_with_narration(Mode::Demo, scripted(1, &[], None), |db, g| {
        test_support::narration(
            Mode::Demo,
            db.clone(),
            &dir,
            speech.clone(),
            g.unwrap().budget.clone(),
        )
    })
    .await;
    let mut client = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    wait_for_job(&mut client, &created.job_id).await;
    (state, client, created.presentation_id, speech, dir)
}

const SENTENCE_ROWS: &str = "SELECT COUNT(*) FROM audio_clips WHERE model NOT LIKE 'block:%'";
const BLOCK_ROWS: &str = "SELECT COUNT(*) FROM audio_clips WHERE model LIKE 'block:%'";

async fn count(db: &SqlitePool, sql: &'static str) -> i64 {
    let (count,): (i64,) = sqlx::query_as(sql).fetch_one(db).await.unwrap();
    count
}

/// Waits until `count(sql)` reaches `n`.
async fn wait_for_rows(db: &SqlitePool, sql: &'static str, n: i64) {
    for _ in 0..400 {
        if count(db, sql).await >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("pre-warm did not finish");
}

#[tokio::test]
async fn prewarm_skips_staff_text_and_quota_counts_only_misses() {
    let (state, mut client, id, speech, _dir) = demo_with_speech().await;
    let edited = "Rest for a day and call us if the pain grows.";
    let response = put_block(
        &mut client,
        &id,
        json!({ "section_key": "procedure:p1", "field": "recovery", "sentences": [edited] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let patient: PatientView = json_body(content(&mut client).await).await;
    let blocks = crate::api::present::prewarm_blocks(&patient);
    let texts: Vec<String> = blocks.iter().flatten().cloned().collect();
    assert!(!texts.iter().any(|t| t == edited));
    let unique: std::collections::HashSet<&String> = texts.iter().collect();
    let unique_blocks: std::collections::HashSet<&Vec<String>> = blocks.iter().collect();
    wait_for_rows(&state.db, SENTENCE_ROWS, unique.len() as i64).await;
    // Every generated block was merged ahead of time too.
    wait_for_rows(&state.db, BLOCK_ROWS, unique_blocks.len() as i64).await;
    // Give a stray staff sentence time to show up if it were pre-warmed.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let calls = speech.calls();
    assert_eq!(calls as usize, unique.len());
    assert!(!speech.texts.lock().unwrap().iter().any(|t| t == edited));

    let sentences: Vec<&dc_core::present::PatientSentence> = patient
        .sections
        .iter()
        .flat_map(|s| &s.blocks)
        .flat_map(|b| &b.sentences)
        .collect();
    let staff = sentences.iter().find(|s| s.text == edited).unwrap();
    let generated = sentences.iter().find(|s| texts.contains(&s.text)).unwrap();

    // The address quota is used up: misses get 429, cached clips still play.
    let used: u32 = unique.iter().map(|t| t.chars().count() as u32).sum();
    state
        .limits
        .narration_address
        .take(
            &crate::limits::ip_key("0.0.0.0".parse().unwrap()),
            crate::narration::DEMO_ADDRESS_CHARS_PER_DAY - used,
            crate::narration::DEMO_ADDRESS_CHARS_PER_DAY,
            std::time::Duration::from_secs(24 * 60 * 60),
        )
        .unwrap();
    let audio = |key: &str| format!("/api/present/audio/{key}");
    let response = client
        .send(Method::GET, &audio(&staff.audio_key), None)
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers().get(header::RETRY_AFTER).is_some());
    assert_eq!(error_code(response).await, "narration_quota");
    let response = client
        .send(Method::GET, &audio(&generated.audio_key), None)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(speech.calls(), calls);
}

#[tokio::test]
async fn narration_budget_share_does_not_stop_generation() {
    let (state, mut client, id, speech, _dir) = demo_with_speech().await;
    // Narration has used its share of the $25 cap; total spend is well
    // under the 80% soft stop.
    sqlx::query("UPDATE ai_usage SET speech_cost_usd = 5.0, cost_usd = cost_usd + 5.0")
        .execute(&state.db)
        .await
        .unwrap();
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let patient: PatientView = json_body(content(&mut client).await).await;
    let key = &patient.sections[0].blocks[0].sentences[0].audio_key;
    let response = client
        .send(Method::GET, &format!("/api/present/audio/{key}"), None)
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(response).await, "budget_reached");
    assert_eq!(speech.calls(), 0);

    let exited: ExitPresentResponse = json_body(exit(&mut client, json!({})).await).await;
    client.csrf = Some(exited.csrf_token);
    let response = create(&mut client).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn narration_plays_one_block_of_the_bound_version_as_one_clip() {
    let (state, mut client, id, speech, _dir) = demo_with_speech().await;
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let patient: PatientView = json_body(content(&mut client).await).await;
    let blocks: Vec<&dc_core::present::PatientBlock> =
        patient.sections.iter().flat_map(|s| &s.blocks).collect();
    let block = blocks.iter().find(|b| b.sentences.len() >= 2).unwrap();
    // Let pre-warming finish, so it does not add calls below.
    let parts = crate::api::present::prewarm_blocks(&patient);
    let unique: std::collections::HashSet<&Vec<String>> = parts.iter().collect();
    wait_for_rows(&state.db, BLOCK_ROWS, unique.len() as i64).await;
    let keys: Vec<&str> = block
        .sentences
        .iter()
        .map(|s| s.audio_key.as_str())
        .collect();
    let url = |keys: &[&str]| format!("/api/present/narration?keys={}", keys.join(","));

    let response = client.send(Method::GET, &url(&keys), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "audio/wav");
    let offsets: Vec<u32> = response.headers()["x-dc-sentence-offsets"]
        .to_str()
        .unwrap()
        .split(',')
        .map(|o| o.parse().unwrap())
        .collect();
    let duration: u32 = response.headers()["x-dc-duration-ms"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    // Fake speech is 10 ms per character, with 250 ms between sentences.
    let mut expected = Vec::new();
    let mut at = 0;
    for sentence in &block.sentences {
        expected.push(at);
        at += u32::try_from(sentence.text.chars().count() * 10).unwrap() + 250;
    }
    assert_eq!(offsets, expected);
    assert_eq!(duration, at - 250);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[0..4], b"RIFF");
    // Exactly the WAV is served, without the stored offsets.
    let riff = u32::from_le_bytes(body[4..8].try_into().unwrap()) as usize;
    assert_eq!(body.len(), riff + 8);

    // A later part of the same block, in order, is fine; a replay is cached.
    let calls = speech.calls();
    let tail = client.send(Method::GET, &url(&keys[1..]), None).await;
    assert_eq!(tail.status(), StatusCode::OK);
    let again = client.send(Method::GET, &url(&keys), None).await;
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(speech.calls(), calls);

    // Keys from two blocks, out of order or not in this version are refused.
    let other = blocks
        .iter()
        .find(|b| b.sentences[0].audio_key != keys[0])
        .unwrap();
    let foreign = audio_key(
        crate::narration::speech_model_id(&state),
        "en",
        "A sentence from another presentation.",
    );
    let reversed: Vec<&str> = keys.iter().rev().copied().collect();
    for bad in [
        vec![keys[keys.len() - 1], other.sentences[0].audio_key.as_str()],
        vec![keys[0], other.sentences[0].audio_key.as_str()],
        reversed,
        vec![keys[0], foreign.as_str()],
        vec![foreign.as_str()],
    ] {
        let response = client.send(Method::GET, &url(&bad), None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(error_code(response).await, "audio_not_found");
    }
    // Malformed requests.
    let thirteen = vec![keys[0]; 13];
    for bad in [
        url(&thirteen),
        url(&[]),
        url(&["abc"]),
        format!("/api/present/narration?keys={},", keys[0]),
        format!("/api/present/narration?other=1&keys={}", keys[0]),
        "/api/present/narration".to_owned(),
    ] {
        let response = client.send(Method::GET, &bad, None).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(speech.calls(), calls);
}

#[tokio::test]
async fn narration_quota_is_charged_per_synthesized_sentence() {
    let (state, mut client, id, speech, _dir) = demo_with_speech().await;
    let edited = [
        "Rest for a day and call us if the pain grows.",
        "Eat soft food on the other side until it heals.",
    ];
    let response = put_block(
        &mut client,
        &id,
        json!({ "section_key": "procedure:p1", "field": "recovery", "sentences": edited }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let patient: PatientView = json_body(content(&mut client).await).await;
    let block = patient
        .sections
        .iter()
        .flat_map(|s| &s.blocks)
        .find(|b| b.sentences.iter().any(|s| s.text == edited[0]))
        .unwrap();
    let keys: Vec<&str> = block
        .sentences
        .iter()
        .map(|s| s.audio_key.as_str())
        .collect();
    // Let pre-warming of generated text finish so it does not overlap.
    let blocks = crate::api::present::prewarm_blocks(&patient);
    let unique: std::collections::HashSet<&Vec<String>> = blocks.iter().collect();
    wait_for_rows(&state.db, BLOCK_ROWS, unique.len() as i64).await;
    let before = speech.calls();

    let url = format!("/api/present/narration?keys={}", keys.join(","));
    assert_eq!(
        client.send(Method::GET, &url, None).await.status(),
        StatusCode::OK
    );
    assert_eq!(speech.calls(), before + 2);
    let response = client.send(Method::GET, &url, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(speech.calls(), before + 2);

    // The address was charged the pre-warmed sentences once each, plus the
    // two edited sentences: nothing for merging or for the replay.
    let prewarmed: std::collections::HashSet<&String> = blocks.iter().flatten().collect();
    let used = prewarmed.iter().map(|t| t.chars().count()).sum::<usize>()
        + edited.iter().map(|t| t.chars().count()).sum::<usize>();
    let day = crate::narration::DEMO_ADDRESS_CHARS_PER_DAY;
    let address = crate::limits::ip_key("0.0.0.0".parse().unwrap());
    let window = std::time::Duration::from_secs(24 * 60 * 60);
    let limiter = &state.limits.narration_address;
    let left = day - u32::try_from(used).unwrap();
    // One more character than is left is refused; exactly what is left is not.
    assert!(limiter.take(&address, left + 1, day, window).is_err());
    assert!(limiter.take(&address, left, day, window).is_ok());
}
