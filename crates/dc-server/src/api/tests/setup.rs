//! The setup wizard's connection check and branding on first launch.

use super::present::audit_count;
use super::*;
use dc_core::setup::SetupCheckResponse;

#[tokio::test]
async fn connection_check_needs_the_setup_code() {
    let (app, db) = app_with_db(Mode::Clinical).await;
    let mut client = Client::new(app);

    let wrong = client
        .send(
            Method::POST,
            "/api/setup/check",
            Some(json!({ "setup_code": "WRONG-CODE" })),
        )
        .await;
    assert_eq!(error_code(wrong).await, "bad_setup_code");
    assert_eq!(audit_count(&db, "setup_code_rejected").await, 1);

    let right = client
        .send(
            Method::POST,
            "/api/setup/check",
            Some(json!({ "setup_code": SETUP_CODE })),
        )
        .await;
    assert_eq!(right.status(), StatusCode::OK);
    let checked: SetupCheckResponse = json_body(right).await;
    assert!(checked.practice_records.ok);
    // The test app has no text provider.
    assert!(!checked.ai_provider.ok);
    // The check stores nothing and signs no one in.
    assert!(client.cookie.is_none());
    assert_eq!(client.session().await.state, SessionState::SetupRequired);

    set_up(&mut client).await;
    let after = client
        .send(
            Method::POST,
            "/api/setup/check",
            Some(json!({ "setup_code": SETUP_CODE })),
        )
        .await;
    assert_eq!(error_code(after).await, "already_set_up");
}

#[tokio::test]
async fn connection_check_counts_toward_the_backoff() {
    let mut client = Client::new(app(Mode::Clinical).await);
    let mut last = StatusCode::OK;
    for _ in 0..6 {
        last = client
            .send(
                Method::POST,
                "/api/setup/check",
                Some(json!({ "setup_code": "WRONG-CODE" })),
            )
            .await
            .status();
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    // The real code is refused too while the backoff runs.
    let blocked = client
        .send(Method::POST, "/api/setup", Some(json!({ "setup_code": SETUP_CODE, "practice_name": "Maple Grove", "password": PASSWORD, "pin": "480913" })))
        .await;
    assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn demo_has_no_connection_check() {
    let mut client = Client::new(app(Mode::Demo).await);
    let response = client
        .send(
            Method::POST,
            "/api/setup/check",
            Some(json!({ "setup_code": SETUP_CODE })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn setup_stores_brand_colors_and_the_acknowledgment() {
    let (app, db) = app_with_db(Mode::Clinical).await;
    let mut client = Client::new(app);
    // A brand color that fails the contrast check blocks setup.
    let pale = client
        .send(Method::POST, "/api/setup", Some(json!({ "setup_code": SETUP_CODE, "practice_name": "Maple Grove", "password": PASSWORD, "pin": "480913", "brand_primary": "#FFEEAA" })))
        .await;
    assert_eq!(pale.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(client.session().await.state, SessionState::SetupRequired);

    let response = client
        .send(
            Method::POST,
            "/api/setup",
            Some(json!({
                "setup_code": SETUP_CODE, "practice_name": "Maple Grove", "password": PASSWORD,
                "pin": "480913", "brand_primary": "#0b4f6c", "brand_highlight": "#c2410c",
                "acknowledged": true,
            })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let info: SessionInfo = json_body(response).await;
    client.csrf = info.csrf_token;
    let settings: PracticeSettings =
        json_body(client.send(Method::GET, "/api/staff/settings", None).await).await;
    assert_eq!(settings.brand_primary, "#0B4F6C");
    assert_eq!(settings.brand_highlight, "#C2410C");
    let (detail,): (String,) =
        sqlx::query_as("SELECT detail FROM audit_events WHERE action = 'setup_completed'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(detail, "notices acknowledged");
}
