//! Engagement reports from the patient screens and the staff analytics
//! summary (milestone M8).

use super::present::{demo_presentation, exit, start_present};
use super::*;
use dc_core::analytics::{AnalyticsResponse, PresentScreen};
use dc_core::present::ExitPresentResponse;

async fn report(client: &mut Client, body: serde_json::Value) -> Response {
    client
        .send(Method::POST, "/api/present/analytics", Some(body))
        .await
}

async fn analytics(client: &mut Client, query: &str) -> Response {
    client
        .send(Method::GET, &format!("/api/staff/analytics{query}"), None)
        .await
}

#[tokio::test]
async fn present_reports_add_up_on_the_analytics_screen() {
    let (_app, db, _state, mut client, id) = demo_presentation().await;
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    // The showing started a minute ago.
    sqlx::query("UPDATE present_events SET at = at - 60000 WHERE kind = 'open'")
        .execute(&db)
        .await
        .unwrap();

    let view =
        |screen: &str, seconds: u32| json!({"kind": "view", "screen": screen, "seconds": seconds});
    for body in [
        view("at_a_glance", 15),
        view("costs", 10),
        json!({"kind": "narration", "screen": "costs"}),
        // More than has passed since the showing started: capped.
        view("costs", 100),
    ] {
        assert_eq!(
            report(&mut client, body).await.status(),
            StatusCode::NO_CONTENT
        );
    }
    // Unknown screens and kinds are rejected.
    for body in [
        view("settings", 5),
        json!({"kind": "click", "screen": "costs"}),
    ] {
        assert!(report(&mut client, body).await.status().is_client_error());
    }
    // While Present mode runs, the locked staff session cannot read it.
    assert_eq!(
        analytics(&mut client, "").await.status(),
        StatusCode::LOCKED
    );

    let exited: ExitPresentResponse = json_body(exit(&mut client, json!({})).await).await;
    client.csrf = Some(exited.csrf_token);
    let summary: AnalyticsResponse = json_body(analytics(&mut client, "?days=7").await).await;
    assert_eq!(summary.days, 7);
    assert_eq!(summary.generation.presentations, 1);
    assert_eq!(summary.present.sessions, 1);
    assert_eq!(summary.present.presentations, 1);
    // 15 + 10 + the 40 seconds left of the minute (60 + 5 slack - 25).
    assert_eq!(summary.present.seconds, 65);
    assert_eq!(summary.present.narration_plays, 1);
    assert_eq!(summary.present.median_seconds, Some(65));
    assert_eq!(summary.languages.len(), 1);
    assert_eq!(summary.languages[0].language, "en");
    assert_eq!(
        (
            summary.languages[0].presentations,
            summary.languages[0].showings
        ),
        (1, 1)
    );
    let costs = summary
        .screens
        .iter()
        .find(|s| s.screen == PresentScreen::Costs)
        .unwrap();
    assert_eq!(
        (costs.seconds, costs.sessions, costs.narration_plays),
        (50, 1, 1)
    );
    assert_eq!(summary.screens.len(), PresentScreen::ALL.len());
    assert_eq!(summary.presented.len(), 1);
    let row = &summary.presented[0];
    assert_eq!(row.presentation_id, id);
    assert_eq!((row.sessions, row.screens_viewed), (1, 2));

    assert_eq!(
        analytics(&mut client, "?days=3").await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn analytics_are_scoped_to_the_visitor() {
    let (app, _db, _state, mut client, id) = demo_presentation().await;
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let mut other = demo_visitor_of(&app).await;
    let summary: AnalyticsResponse = json_body(analytics(&mut other, "").await).await;
    assert_eq!(summary.days, 30);
    assert_eq!(summary.generation.presentations, 0);
    assert_eq!(summary.present.sessions, 0);
    assert!(summary.presented.is_empty());
}

async fn demo_visitor_of(app: &Router) -> Client {
    visitor_of(app).await
}

/// FR-ANLY-04: engagement rows hold ids, enums and numbers, never names or
/// free text.
#[tokio::test]
async fn engagement_rows_have_no_free_text_columns() {
    let (_app, db, _state, _client, _id) = demo_presentation().await;
    let columns: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM pragma_table_info('present_events')")
            .fetch_all(&db)
            .await
            .unwrap();
    let names: Vec<&str> = columns.iter().map(|(n,)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "presentation_id",
            "version",
            "session",
            "kind",
            "screen",
            "seconds",
            "at"
        ]
    );
}
