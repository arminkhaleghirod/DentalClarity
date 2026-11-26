//! Stale locks, single-section regeneration, release of held sections and
//! job resume.

use super::present::*;
use super::*;
use dc_core::present::{BlockChangeResponse, ExitPresentResponse};
use dc_core::presentation::{RegenerateSectionResponse, SectionView};

/// Generation for Daniel's plan where `procedure:p6` fails the checks on
/// its first run (all three attempts) and passes when written again. Every
/// other section passes and has spare replies for a few rewrites.
fn p6_fails_once() -> impl FnOnce(&SqlitePool) -> Option<Generation> {
    move |db: &SqlitePool| {
        let by_section = DANIEL_SECTIONS
            .iter()
            .map(|key| {
                let good = good_reply(key);
                let replies = if *key == "procedure:p6" {
                    let bad = good.replace(
                        "It can help keep your mouth healthy.",
                        "Success is guaranteed.",
                    );
                    vec![bad.clone(), bad.clone(), bad, good.clone(), good]
                } else {
                    vec![good; 4]
                };
                (key.to_string(), FakeTextModel::with_responses(replies))
            })
            .collect();
        let pass = json!({ "verdict": "pass", "issues": [] }).to_string();
        Some(Generation::new(
            db.clone(),
            Some(25.0),
            Arc::new(SectionRouter {
                by_section,
                gate: None,
            }),
            "fake-writer".into(),
            Arc::new(FakeTextModel::with_responses(vec![pass; 40])),
            "fake-reviewer".into(),
            None,
        ))
    }
}

async fn demo_with(
    generation: impl FnOnce(&SqlitePool) -> Option<Generation>,
) -> (Router, SqlitePool, AppState, Client, String) {
    let (app, db, state) = app_with(Mode::Demo, generation).await;
    let mut client = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    wait_for_job(&mut client, &created.job_id).await;
    (app, db, state, client, created.presentation_id)
}

async fn section_action(client: &mut Client, id: &str, key: &str, action: &str) -> Response {
    client
        .send(
            Method::POST,
            &format!("/api/staff/presentations/{id}/sections/{key}/{action}"),
            None,
        )
        .await
}

fn section<'a>(view: &'a PresentationView, key: &str) -> &'a SectionView {
    view.sections.iter().find(|s| s.key == key).unwrap()
}

async fn calls(db: &SqlitePool) -> i64 {
    let (n,): (i64,) = sqlx::query_as("SELECT calls FROM ai_usage")
        .fetch_one(db)
        .await
        .unwrap();
    n
}

async fn unlock(client: &mut Client, body: serde_json::Value) -> Response {
    client
        .send(Method::POST, "/api/session/unlock", Some(body))
        .await
}

async fn expire_present(db: &SqlitePool) {
    sqlx::query("UPDATE present_sessions SET last_seen_at = last_seen_at - 3600")
        .execute(db)
        .await
        .unwrap();
}

/// The attributes of the session cookie a response sets, without its value.
fn session_cookie_attributes(response: &Response) -> String {
    let set = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|v| v.starts_with("dc_session=") && !v.starts_with("dc_session=;"))
        .expect("the response sets a session cookie");
    set.split_once(';').unwrap().1.to_owned()
}

#[tokio::test]
async fn rotated_session_cookies_keep_the_visit_cookie_lifetime() {
    let (app, db, _) = app_with(Mode::Demo, scripted(2, &[], None)).await;
    let mut client = Client::new(app);
    let visit = client.send(Method::POST, "/api/visit", None).await;
    let visit_attrs = session_cookie_attributes(&visit);
    client.session().await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    wait_for_job(&mut client, &created.job_id).await;
    let id = created.presentation_id;

    // Leaving Present with Done rotates the session token.
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let exited = exit(&mut client, json!({})).await;
    assert_eq!(exited.status(), StatusCode::OK);
    assert_eq!(session_cookie_attributes(&exited), visit_attrs);
    let exited: ExitPresentResponse = json_body(exited).await;
    client.csrf = Some(exited.csrf_token);

    // So does unlocking after the present session expired.
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    expire_present(&db).await;
    client.session().await;
    let unlocked = unlock(&mut client, json!({})).await;
    assert_eq!(unlocked.status(), StatusCode::OK);
    assert_eq!(session_cookie_attributes(&unlocked), visit_attrs);

    // Visitor and staff cookies both end with the browser session today.
    assert!(!visit_attrs.contains("Max-Age"));
    let mut staff = Client::new(app_with_db(Mode::Clinical).await.0);
    staff.session().await;
    let setup = staff
        .send(Method::POST, "/api/setup", Some(json!({ "setup_code": SETUP_CODE, "practice_name": "Maple Grove Family Dental", "password": PASSWORD, "pin": "480913" })))
        .await;
    assert_eq!(setup.status(), StatusCode::OK);
    assert!(!session_cookie_attributes(&setup).contains("Max-Age"));
}

// Stale locks ----------------------------------------------------------------

#[tokio::test]
async fn demo_lock_without_a_present_session_unlocks_in_place() {
    let (app, db, _, mut client, id) = demo_presentation().await;

    // Not locked yet.
    let response = unlock(&mut client, json!({})).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(response).await, "not_locked");

    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    // While the patient is viewing, the lock is an ordinary one.
    let dashboard = client.send(Method::GET, "/api/staff/dashboard", None).await;
    assert_eq!(dashboard.status(), StatusCode::LOCKED);
    assert_eq!(error_code(dashboard).await, "locked");
    let info = client.session().await;
    assert_eq!(info.state, SessionState::Locked);
    assert!(!info.present_ended);
    let response = unlock(&mut client, json!({})).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(response).await, "present_active");

    // The tablet sits idle past the present session's lifetime.
    expire_present(&db).await;
    let dashboard = client.send(Method::GET, "/api/staff/dashboard", None).await;
    assert_eq!(dashboard.status(), StatusCode::LOCKED);
    assert_eq!(error_code(dashboard).await, "locked_present_ended");
    let info = client.session().await;
    assert_eq!(info.state, SessionState::Locked);
    assert!(info.present_ended);

    // Unlocking needs the CSRF token.
    let saved = client.csrf.take();
    let forged = unlock(&mut client, json!({})).await;
    assert_eq!(error_code(forged).await, "bad_csrf");
    client.csrf = saved;

    let old_cookie = client.cookie.clone();
    let response = unlock(&mut client, json!({})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let unlocked: SessionInfo = json_body(response).await;
    assert_eq!(unlocked.state, SessionState::Visitor);
    assert_ne!(client.cookie, old_cookie);
    assert!(client.present.is_none());
    client.csrf = unlocked.csrf_token.clone();
    assert_eq!(audit_count(&db, "session_unlocked").await, 1);

    // The same visitor keeps their presentations; the old token is dead.
    let info = client.session().await;
    assert_eq!(info.state, SessionState::Visitor);
    assert!(!info.present_ended);
    assert_eq!(view(&mut client, &id).await.id, id);
    let mut old = Client::new(app);
    old.cookie = old_cookie;
    assert_eq!(
        old.send(Method::GET, "/api/staff/dashboard", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn clinical_lock_without_a_present_session_needs_the_password() {
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
    expire_present(&db).await;
    assert!(client.session().await.present_ended);

    let response = unlock(&mut client, json!({})).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(response).await, "password_required");
    // The PIN does not unlock a stale lock.
    let response = unlock(&mut client, json!({ "pin": "480913" })).await;
    assert_eq!(error_code(response).await, "password_required");
    let response = unlock(&mut client, json!({ "password": "not the password" })).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(response).await, "invalid_credentials");
    assert_eq!(audit_count(&db, "session_unlock_failed").await, 1);
    assert_eq!(client.session().await.state, SessionState::Locked);

    let response = unlock(&mut client, json!({ "password": PASSWORD })).await;
    assert_eq!(response.status(), StatusCode::OK);
    let unlocked: SessionInfo = json_body(response).await;
    assert_eq!(unlocked.state, SessionState::Staff);
    client.csrf = unlocked.csrf_token;
    assert_eq!(view(&mut client, &id).await.id, id);
    assert_eq!(audit_count(&db, "session_unlocked").await, 1);
}

#[tokio::test]
async fn a_failed_start_leaves_the_session_unlocked() {
    let (_, db, _, mut client, id) = demo_presentation().await;
    // The audit write inside the start transaction fails.
    sqlx::query("DROP TABLE audit_events")
        .execute(&db)
        .await
        .unwrap();
    let response = start_present(&mut client, &id).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(client.present.is_none());
    assert_eq!(client.session().await.state, SessionState::Visitor);
    let (present,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM present_sessions")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(present, 0);
}

// Release ----------------------------------------------------------------------

async fn hold(db: &SqlitePool, key: &str) {
    sqlx::query("UPDATE presentation_sections SET state = 'needs_review' WHERE key = ?")
        .bind(key)
        .execute(db)
        .await
        .unwrap();
}

#[tokio::test]
async fn release_rechecks_staff_text_and_is_audited() {
    let (_, db, _, mut client, id) = demo_presentation().await;
    hold(&db, "procedure:p2").await;
    hold(&db, "procedure:p3").await;

    let before = view(&mut client, &id).await;
    let p2 = section(&before, "procedure:p2");
    assert_eq!(p2.visibility, SectionVisibility::NeedsReview);
    assert!(p2.can_release, "{:?}", p2.release_blockers);
    assert!(p2.can_regenerate);
    // Ready sections are never offered for release.
    assert!(!section(&before, "procedure:p1").can_release);

    // A staff edit that guarantees an outcome blocks the release.
    let response = put_block(
        &mut client,
        &id,
        json!({ "section_key": "procedure:p3", "field": "benefits", "sentences": ["Success is guaranteed."] }),
    )
    .await;
    let changed: BlockChangeResponse = json_body(response).await;
    assert!(!changed.section.can_release);
    assert!(
        changed
            .section
            .release_blockers
            .iter()
            .any(|b| b.contains("V11")),
        "{:?}",
        changed.section.release_blockers
    );
    let response = section_action(&mut client, &id, "procedure:p3", "release").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(response).await, "release_blocked");

    // Fixed, it can be released.
    put_block(
        &mut client,
        &id,
        json!({ "section_key": "procedure:p3", "field": "benefits", "sentences": ["It can help protect the tooth."] }),
    )
    .await;
    let response = section_action(&mut client, &id, "procedure:p3", "release").await;
    assert_eq!(response.status(), StatusCode::OK);
    let released: BlockChangeResponse = json_body(response).await;
    assert_eq!(released.section.visibility, SectionVisibility::Ready);
    assert!(!released.section.can_release);
    assert_eq!(audit_count(&db, "section_released").await, 1);
    let response = section_action(&mut client, &id, "procedure:p3", "release").await;
    assert_eq!(error_code(response).await, "already_ready");

    // The job summary follows: one section is still held.
    let list: Vec<PresentationSummary> = json_body(
        client
            .send(Method::GET, "/api/staff/presentations", None)
            .await,
    )
    .await;
    assert_eq!(list[0].sections_ready, 8);
    assert_eq!(list[0].job_state, JobState::NeedsReview);

    // The released section reaches the patient; the held one does not.
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let patient: dc_core::present::PatientView = json_body(content(&mut client).await).await;
    let keys: Vec<&str> = patient.sections.iter().map(|s| s.key.as_str()).collect();
    assert!(keys.contains(&"procedure:p3"));
    assert!(!keys.contains(&"procedure:p2"));
}

#[tokio::test]
async fn release_never_skips_a_finding_in_generated_text() {
    let (_, db, _, mut client, id) = demo_presentation().await;
    hold(&db, "procedure:p2").await;
    // Stored output that would fail the checks today.
    sqlx::query(
        "UPDATE presentation_sections
         SET output_json = REPLACE(output_json, 'It can help keep your mouth healthy.', 'Success is guaranteed.')
         WHERE key = 'procedure:p2'",
    )
    .execute(&db)
    .await
    .unwrap();
    let p2 = section(&view(&mut client, &id).await, "procedure:p2").clone();
    assert!(!p2.can_release);
    assert!(
        p2.release_blockers
            .iter()
            .any(|b| b.starts_with("Benefits"))
    );
    let response = section_action(&mut client, &id, "procedure:p2", "release").await;
    assert_eq!(error_code(response).await, "release_blocked");

    // Staff text replaces the finding's block, so the finding no longer applies.
    put_block(
        &mut client,
        &id,
        json!({ "section_key": "procedure:p2", "field": "benefits", "sentences": ["It can help support a crown."] }),
    )
    .await;
    let response = section_action(&mut client, &id, "procedure:p2", "release").await;
    assert_eq!(response.status(), StatusCode::OK);

    // A failed section has no text to release.
    sqlx::query(
        "UPDATE presentation_sections SET state = 'failed', output_json = NULL WHERE key = 'procedure:p4'",
    )
    .execute(&db)
    .await
    .unwrap();
    let response = section_action(&mut client, &id, "procedure:p4", "release").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(response).await, "nothing_to_release");
    let response = section_action(&mut client, &id, "procedure:p9", "release").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(response).await, "section_not_found");

    // Another visitor cannot release it.
    hold(&db, "procedure:p5").await;
    let mut other = visitor_of(&client.app).await;
    let response = section_action(&mut other, &id, "procedure:p5", "release").await;
    assert_eq!(error_code(response).await, "presentation_not_found");
}

// Regeneration -----------------------------------------------------------------

#[tokio::test]
async fn one_section_is_written_again_through_the_job_system() {
    let (_, db, _, mut client, id) = demo_with(p6_fails_once()).await;
    let before = view(&mut client, &id).await;
    assert_eq!(
        section(&before, "procedure:p6").visibility,
        SectionVisibility::Failed
    );
    assert!(section(&before, "procedure:p6").can_regenerate);
    // 9 writer calls, 2 repairs for p6, 8 reviews.
    assert_eq!(calls(&db).await, 19);

    // Staff work on p1: a confirmed note and an edit.
    post_block(&mut client, &id, "confirm-note", "procedure:p1", "why").await;
    put_block(
        &mut client,
        &id,
        json!({ "section_key": "procedure:p1", "field": "what_happens", "sentences": ["The team explains each step."] }),
    )
    .await;

    let response = section_action(&mut client, &id, "procedure:p6", "regenerate").await;
    assert_eq!(response.status(), StatusCode::OK);
    let started: RegenerateSectionResponse = json_body(response).await;
    assert_eq!(started.presentation_id, id);
    assert_eq!(started.section_key, "procedure:p6");
    let status = wait_for_job(&mut client, &started.job_id).await;
    assert_eq!(status.section_key.as_deref(), Some("procedure:p6"));
    assert_eq!(status.state, JobState::Succeeded);
    assert_eq!(status.sections.len(), 9);
    assert!(
        status
            .sections
            .iter()
            .all(|s| s.stage == SectionStage::Passed)
    );
    // One writer call and one review.
    assert_eq!(calls(&db).await, 21);

    let after = view(&mut client, &id).await;
    assert_eq!(after.job_id, started.job_id);
    assert_eq!(after.job_state, JobState::Succeeded);
    assert_eq!(after.sections.len(), 9);
    assert_eq!(
        section(&after, "procedure:p6").visibility,
        SectionVisibility::Ready
    );
    assert_eq!(audit_count(&db, "section_regeneration_started").await, 1);

    // p1 written again: the edit carries forward, the confirmation does not.
    let response = section_action(&mut client, &id, "procedure:p1", "regenerate").await;
    let started: RegenerateSectionResponse = json_body(response).await;
    wait_for_job(&mut client, &started.job_id).await;
    let p1 = section(&view(&mut client, &id).await, "procedure:p1").clone();
    let block = |field: &str| p1.blocks.iter().find(|b| b.field == field).unwrap().clone();
    assert_eq!(block("what_happens").source, BlockSource::StaffEdit);
    assert!(block("why").needs_confirmation);
}

#[tokio::test]
async fn regeneration_respects_the_demo_gates() {
    let (_, db, _, mut client, id) = demo_with(p6_fails_once()).await;

    let response = section_action(&mut client, &id, "procedure:p9", "regenerate").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(response).await, "section_not_found");

    // A job of this presentation is still running.
    sqlx::query("UPDATE jobs SET state = 'running'")
        .execute(&db)
        .await
        .unwrap();
    let response = section_action(&mut client, &id, "procedure:p6", "regenerate").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(response).await, "generation_in_progress");
    sqlx::query("UPDATE jobs SET state = 'needs_review'")
        .execute(&db)
        .await
        .unwrap();

    // The visitor's six rewrites for this hour are used up.
    for _ in 0..crate::api::review::DEMO_REGENERATIONS_PER_VISITOR_PER_HOUR {
        sqlx::query(
            "INSERT INTO regeneration_events (owner_scope, client_key, created_at)
             SELECT owner_scope, 'another network', ? FROM presentations",
        )
        .bind(crate::jobs::now_ms() - 1000)
        .execute(&db)
        .await
        .unwrap();
    }
    let response = section_action(&mut client, &id, "procedure:p6", "regenerate").await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers().get(header::RETRY_AFTER).is_some());
    assert_eq!(error_code(response).await, "rate_limited");
    // The count does not depend on the presentation's jobs, which are
    // purged with the presentation, and it outlives the purge of other
    // demo data.
    sqlx::query("DELETE FROM jobs WHERE section_key IS NOT NULL")
        .execute(&db)
        .await
        .unwrap();
    crate::jobs::purge_demo_data(&db).await.unwrap();
    let response = section_action(&mut client, &id, "procedure:p6", "regenerate").await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    sqlx::query("DELETE FROM regeneration_events")
        .execute(&db)
        .await
        .unwrap();

    // Past the budget's soft stop.
    sqlx::query("UPDATE ai_usage SET cost_usd = 20.0")
        .execute(&db)
        .await
        .unwrap();
    let response = section_action(&mut client, &id, "procedure:p6", "regenerate").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(response).await, "budget_reached");
    sqlx::query("UPDATE ai_usage SET cost_usd = 0")
        .execute(&db)
        .await
        .unwrap();

    // A procedure with no knowledge entry stays held.
    sqlx::query(
        "UPDATE presentations SET snapshot_json = REPLACE(snapshot_json, 'D2740', 'D0001')",
    )
    .execute(&db)
    .await
    .unwrap();
    assert!(!section(&view(&mut client, &id).await, "procedure:p3").can_regenerate);
    let response = section_action(&mut client, &id, "procedure:p3", "regenerate").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(response).await, "no_knowledge_entry");

    // Another visitor cannot rewrite it.
    let mut other = visitor_of(&client.app).await;
    let response = section_action(&mut other, &id, "procedure:p6", "regenerate").await;
    assert_eq!(error_code(response).await, "presentation_not_found");
    assert_eq!(audit_count(&db, "section_regeneration_started").await, 0);
}

// Resume -----------------------------------------------------------------------

#[tokio::test]
async fn a_resumed_job_skips_the_sections_it_already_finished() {
    let (app, db, state) = app_with(Mode::Demo, scripted(2, &[], None)).await;
    let mut client = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    wait_for_job(&mut client, &created.job_id).await;
    // 9 writer calls and 9 reviews.
    assert_eq!(calls(&db).await, 18);

    // Simulate a stop after eight sections were saved.
    sqlx::query(
        "INSERT INTO job_section_results (job_id, key, position, state, output_json,
             reasons_json, needs_note_confirmation, writer_model, prompt_version)
         SELECT ?, key, position, state, output_json, reasons_json, needs_note_confirmation,
             writer_model, prompt_version
         FROM presentation_sections WHERE key <> 'next_steps'",
    )
    .bind(&created.job_id)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query("DELETE FROM presentation_sections")
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state = 'running'")
        .execute(&db)
        .await
        .unwrap();

    assert_eq!(crate::jobs::resume_pending(&state).await.unwrap(), 1);
    let status = wait_for_job(&mut client, &created.job_id).await;
    assert_eq!(status.state, JobState::Succeeded);
    // Only next_steps ran again.
    assert_eq!(calls(&db).await, 20);
    let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM presentation_sections")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(rows, 9);
    let (left,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM job_section_results")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
