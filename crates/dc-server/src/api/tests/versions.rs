//! Plan sync, version history and the booklet export.

use super::present::{content, demo_presentation, put_block, start_present, view};
use super::*;
use crate::sync::{self, Outcome};
use dc_core::Money;
use dc_core::PlanSnapshot;
use dc_core::ToothRef;
use dc_core::money::Amount;
use dc_core::plan::PlanKey;
use dc_core::present::PatientView;
use dc_core::presentation::{RegenerateSectionResponse, SectionView};
use dc_core::versions::{SourceState, VersionCause, VersionSummary};

async fn live_plan(state: &AppState) -> PlanSnapshot {
    state
        .pms
        .plan_snapshot(&PlanKey("tp-10482-1".into()))
        .await
        .unwrap()
}

async fn versions(client: &mut Client, id: &str) -> Vec<VersionSummary> {
    json_body(
        client
            .send(
                Method::GET,
                &format!("/api/staff/presentations/{id}/versions"),
                None,
            )
            .await,
    )
    .await
}

fn section<'a>(view: &'a PresentationView, key: &str) -> &'a SectionView {
    view.sections
        .iter()
        .find(|s| s.key == key)
        .unwrap_or_else(|| panic!("no section {key}"))
}

async fn export(client: &mut Client, id: &str, query: &str) -> Response {
    client
        .send(
            Method::GET,
            &format!("/api/staff/presentations/{id}/export{query}"),
            None,
        )
        .await
}

async fn ai_calls(db: &SqlitePool) -> i64 {
    let (calls,): (i64,) = sqlx::query_as("SELECT COALESCE(SUM(calls), 0) FROM ai_usage")
        .fetch_one(db)
        .await
        .unwrap();
    calls
}

#[tokio::test]
async fn an_unchanged_plan_makes_no_version_and_no_model_call() {
    let (_, db, state, mut client, id) = demo_presentation().await;
    let calls = ai_calls(&db).await;
    let report = sync::poll_once(&state).await.unwrap();
    assert_eq!(report.checked, 1);
    assert_eq!(report.new_versions, 0);
    assert_eq!(
        sync::apply(&state, &id, &live_plan(&state).await)
            .await
            .unwrap(),
        Outcome::Unchanged
    );
    assert_eq!(ai_calls(&db).await, calls);
    let list = versions(&mut client, &id).await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].version, 1);
    assert_eq!(list[0].cause, VersionCause::Created);
    assert!(list[0].current);
}

#[tokio::test]
async fn a_cost_change_makes_a_new_version_with_every_section_carried() {
    let (_, db, state, mut client, id) = demo_presentation().await;
    put_block(
        &mut client,
        &id,
        json!({ "section_key": "procedure:p3", "field": "recovery", "sentences": ["Rest a little afterwards."] }),
    )
    .await;
    let before = view(&mut client, &id).await;
    let calls = ai_calls(&db).await;

    let mut plan = live_plan(&state).await;
    plan.procedures[1].fee = Amount::Known(Money(99_900));
    assert_eq!(
        sync::apply(&state, &id, &plan).await.unwrap(),
        Outcome::NewVersion(2)
    );
    assert_eq!(ai_calls(&db).await, calls);

    let after = view(&mut client, &id).await;
    assert_eq!(after.version, 2);
    assert_eq!(after.latest_version, 2);
    assert!(!after.read_only);
    assert_eq!(after.version_cause, VersionCause::PlanChanged);
    assert!(after.changes.iter().any(|c| c.starts_with("Cost changed")));
    assert_eq!(after.procedures[1].fee, Some(Money(99_900)));
    for (old, new) in before.sections.iter().zip(&after.sections) {
        assert_eq!(old.key, new.key);
        assert_eq!(old.visibility, new.visibility);
    }
    // The staff edit carried forward.
    let recovery = section(&after, "procedure:p3")
        .blocks
        .iter()
        .find(|b| b.field == "recovery")
        .unwrap();
    assert_eq!(recovery.source, BlockSource::StaffEdit);

    let list = versions(&mut client, &id).await;
    assert_eq!(
        list.iter()
            .map(|v| (v.version, v.current))
            .collect::<Vec<_>>(),
        [(2, true), (1, false)]
    );
    assert_eq!(list[0].cause, VersionCause::PlanChanged);

    // The dashboard row says the plan changed until it is presented.
    let rows: Vec<PatientRow> =
        json_body(client.send(Method::GET, "/api/staff/dashboard", None).await).await;
    let daniel = rows.iter().find(|r| r.patient_key == "10482").unwrap();
    assert_eq!(daniel.status, PresentationStatus::PlanChanged);
    assert_eq!(audit_count(&db, "plan_changed").await, 1);
}

async fn audit_count(db: &SqlitePool, action: &str) -> i64 {
    super::present::audit_count(db, action).await
}

#[tokio::test]
async fn an_added_procedure_holds_its_section_and_the_summaries() {
    let (_, _, state, mut client, id) = demo_presentation().await;
    put_block(
        &mut client,
        &id,
        json!({ "section_key": "procedure:p1", "field": "what_it_is", "sentences": ["A root canal cleans the inside of the tooth."] }),
    )
    .await;
    let mut plan = live_plan(&state).await;
    let mut extra = plan.procedures[1].clone();
    extra.source_id = "added-1".into();
    extra.tooth = ToothRef::Permanent(3);
    plan.procedures.insert(0, extra);
    assert_eq!(
        sync::apply(&state, &id, &plan).await.unwrap(),
        Outcome::NewVersion(2)
    );

    let after = view(&mut client, &id).await;
    assert_eq!(after.sections.len(), 10);
    for key in ["at_a_glance", "journey", "procedure:p1", "next_steps"] {
        let held = section(&after, key);
        assert_eq!(held.visibility, SectionVisibility::NeedsReview, "{key}");
        assert!(held.blocks.is_empty(), "{key}");
        assert!(!held.can_release, "{key}");
    }
    assert!(section(&after, "procedure:p1").reasons[0].starts_with("New procedure"));
    assert!(section(&after, "procedure:p1").can_regenerate);
    // The old p1 is now p2, with its text and the staff edit.
    let root_canal = section(&after, "procedure:p2");
    assert_eq!(root_canal.visibility, SectionVisibility::Ready);
    let what = root_canal
        .blocks
        .iter()
        .find(|b| b.field == "what_it_is")
        .unwrap();
    assert_eq!(what.source, BlockSource::StaffEdit);
    assert!(after.changes.iter().any(|c| c.starts_with("Added")));
}

#[tokio::test]
async fn a_present_session_keeps_its_version_through_a_plan_change() {
    let (_, _, state, mut client, id) = demo_presentation().await;
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let before: PatientView = json_body(content(&mut client).await).await;

    let mut plan = live_plan(&state).await;
    plan.procedures[0].patient_estimate = Amount::Known(Money(1));
    plan.procedures[2].tooth = ToothRef::Permanent(2);
    assert_eq!(
        sync::apply(&state, &id, &plan).await.unwrap(),
        Outcome::NewVersion(2)
    );

    let response = content(&mut client).await;
    assert_eq!(response.status(), StatusCode::OK);
    let during: PatientView = json_body(response).await;
    assert_eq!(during, before);
}

#[tokio::test]
async fn an_older_version_opens_read_only() {
    let (_, _, state, mut client, id) = demo_presentation().await;
    let mut plan = live_plan(&state).await;
    plan.procedures[2].tooth = ToothRef::Permanent(2);
    sync::apply(&state, &id, &plan).await.unwrap();

    let old: PresentationView = json_body(
        client
            .send(
                Method::GET,
                &format!("/api/staff/presentations/{id}/versions/1"),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(old.version, 1);
    assert_eq!(old.latest_version, 2);
    assert!(old.read_only);
    assert_eq!(
        old.procedures[2].tooth,
        live_plan(&state).await.procedures[2].tooth.to_string()
    );
    assert_ne!(old.procedures[2].tooth, "2");
    assert_eq!(
        section(&old, "procedure:p3").visibility,
        SectionVisibility::Ready
    );
    assert!(
        old.sections
            .iter()
            .all(|s| !s.can_regenerate && !s.can_release)
    );

    let current = view(&mut client, &id).await;
    assert_eq!(
        section(&current, "procedure:p3").visibility,
        SectionVisibility::NeedsReview
    );

    for missing in ["9", "0", "x"] {
        let response = client
            .send(
                Method::GET,
                &format!("/api/staff/presentations/{id}/versions/{missing}"),
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{missing}");
    }
    let mut other = visitor_of(&client.app).await;
    let response = other
        .send(
            Method::GET,
            &format!("/api/staff/presentations/{id}/versions"),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_deleted_plan_is_reported_and_nothing_changes() {
    let (_, db, state, mut client, id) = demo_presentation().await;
    sqlx::query("UPDATE presentations SET plan_key = 'tp-gone' WHERE id = ?")
        .bind(&id)
        .execute(&db)
        .await
        .unwrap();
    sync::poll_once(&state).await.unwrap();
    let after = view(&mut client, &id).await;
    assert_eq!(after.source_state, SourceState::PlanDeleted);
    assert_eq!(after.version, 1);
}

#[tokio::test]
async fn the_booklet_is_a_pdf_without_held_sections() {
    let (_, db, state, mut client, id) = demo_presentation().await;
    sqlx::query(
        "UPDATE presentation_sections SET state = 'needs_review' WHERE key = 'procedure:p2'",
    )
    .execute(&db)
    .await
    .unwrap();

    let response = export(&mut client, &id, "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/pdf");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 24)
        .await
        .unwrap();
    assert!(bytes.starts_with(b"%PDF"));
    assert_eq!(audit_count(&db, "booklet_exported").await, 1);
    assert!(versions(&mut client, &id).await[0].exported);

    // What the booklet prints: the held section shows only its title and
    // the held card, never its text.
    let (booklet, version) =
        crate::api::booklet::booklet_for(&state, &owner(&db, &id).await, &id, None)
            .await
            .unwrap();
    assert_eq!(version, 1);
    let held = booklet
        .sections
        .iter()
        .find(|s| s.key == "procedure:p2")
        .unwrap();
    assert!(held.held && held.blocks.is_empty() && held.questions.is_empty());
    let text = booklet.plain_text().unwrap();
    assert!(text.contains("Your dental team will go over this part with you."));
    assert!(text.contains("Maple Grove Family Dental"));
    assert!(!text.contains("DentalClarity"));
    // The unconfirmed note-derived reason never prints.
    assert!(!text.contains("decay that reached the nerve"));
    assert_eq!(
        booklet.sections.iter().filter(|s| !s.held).count(),
        8,
        "every other section prints"
    );

    // Other visitors cannot export it; bad version queries are refused.
    let mut other = visitor_of(&client.app).await;
    assert_eq!(
        export(&mut other, &id, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        export(&mut client, &id, "?version=7").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        export(&mut client, &id, "?page=1").await.status(),
        StatusCode::BAD_REQUEST
    );
}

async fn owner(db: &SqlitePool, id: &str) -> String {
    let (owner,): (String,) = sqlx::query_as("SELECT owner_scope FROM presentations WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap();
    owner
}

#[tokio::test]
async fn rewriting_an_exported_version_makes_a_new_version() {
    let (app, db, _) = app_with(Mode::Demo, scripted(2, &[], None)).await;
    let mut client = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    wait_for_job(&mut client, &created.job_id).await;
    let id = created.presentation_id;
    put_block(
        &mut client,
        &id,
        json!({ "section_key": "procedure:p4", "field": "recovery", "sentences": ["Rest a little afterwards."] }),
    )
    .await;
    assert_eq!(export(&mut client, &id, "").await.status(), StatusCode::OK);

    let response = client
        .send(
            Method::POST,
            &format!("/api/staff/presentations/{id}/sections/procedure:p2/regenerate"),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let started: RegenerateSectionResponse = json_body(response).await;
    wait_for_job(&mut client, &started.job_id).await;

    let list = versions(&mut client, &id).await;
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].cause, VersionCause::Regenerated);
    assert!(list[0].changes[0].starts_with("Section written again"));
    assert!(list[1].exported && !list[0].exported);
    let current = view(&mut client, &id).await;
    assert_eq!(current.version, 2);
    let recovery = section(&current, "procedure:p4")
        .blocks
        .iter()
        .find(|b| b.field == "recovery")
        .unwrap();
    assert_eq!(recovery.source, BlockSource::StaffEdit);
    drop(db);
}
