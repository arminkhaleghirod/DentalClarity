//! "Ask a question": grounding, refusals, the fallback, urgent questions,
//! limits and who can read the conversation.

use super::present::{audit_count, exit, start_present};
use super::*;
use dc_core::present::ExitPresentResponse;
use dc_core::qa::{AnswerKind, AskResponse, QaConversation, StaffQuestions};

/// Sends section prompts to the scripted section writer and question
/// prompts to `qa`.
struct QaRouter {
    sections: SectionRouter,
    qa: Arc<FakeTextModel>,
}

#[async_trait::async_trait]
impl TextModel for QaRouter {
    fn provider(&self) -> &'static str {
        "fake"
    }

    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        let is_question = request
            .messages
            .iter()
            .any(|m| m.role == Role::System && m.content.contains("<question> comes from"));
        if is_question {
            self.qa.generate(request).await
        } else {
            self.sections.generate(request).await
        }
    }
}

/// A demo visitor in Present mode on Daniel's plan, with `replies` queued
/// for the question model.
async fn presenting(
    replies: Vec<String>,
) -> (Router, SqlitePool, Client, String, Arc<FakeTextModel>) {
    let qa = Arc::new(FakeTextModel::with_responses(replies));
    let model = qa.clone();
    let (app, db, _) = app_with(Mode::Demo, move |db: &SqlitePool| {
        let by_section = DANIEL_SECTIONS
            .iter()
            .map(|key| {
                (
                    key.to_string(),
                    FakeTextModel::with_responses(vec![good_reply(key); 3]),
                )
            })
            .collect();
        let pass = json!({ "verdict": "pass", "issues": [] }).to_string();
        Some(Generation::new(
            db.clone(),
            Some(25.0),
            Arc::new(QaRouter {
                sections: SectionRouter {
                    by_section,
                    gate: None,
                },
                qa: model,
            }),
            "fake-writer".into(),
            Arc::new(FakeTextModel::with_responses(vec![
                pass;
                DANIEL_SECTIONS.len()
            ])),
            "fake-reviewer".into(),
            None,
        ))
    })
    .await;
    let mut client = visitor_of(&app).await;
    let created: CreatePresentationResponse = json_body(create(&mut client).await).await;
    wait_for_job(&mut client, &created.job_id).await;
    let id = created.presentation_id;
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    (app, db, client, id, qa)
}

fn reply(kind: &str, sentences: &[(&str, &[&str])], suggested: &str) -> String {
    let sentences: Vec<serde_json::Value> = sentences
        .iter()
        .map(|(text, cites)| json!({ "text": text, "cites": cites }))
        .collect();
    json!({
        "kind": kind,
        "sentences": sentences,
        "refer_to_team": kind != "answered",
        "suggested_question": suggested
    })
    .to_string()
}

async fn ask(client: &mut Client, question: &str) -> Response {
    client
        .send(
            Method::POST,
            "/api/present/qa",
            Some(json!({ "question": question })),
        )
        .await
}

async fn staff_questions(client: &mut Client, id: &str) -> Response {
    client
        .send(
            Method::GET,
            &format!("/api/staff/presentations/{id}/questions"),
            None,
        )
        .await
}

/// Leaves Present mode, so the visitor's workspace opens again.
async fn leave(client: &mut Client) {
    let response = exit(client, json!({})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let exited: ExitPresentResponse = json_body(response).await;
    client.csrf = Some(exited.csrf_token);
}

#[tokio::test]
async fn grounded_answers_cite_their_sources_and_never_carry_names() {
    let answer = reply(
        "answered",
        &[
            (
                "During a root canal, {{provider_display_name}} cleans the inside of the tooth.",
                &["text:1", "kb:root_canal#what_happens"],
            ),
            ("Your dental team explains each step.", &["fact:p1"]),
        ],
        "",
    );
    let (_, _, mut client, _, qa) = presenting(vec![answer]).await;

    let before: QaConversation =
        json_body(client.send(Method::GET, "/api/present/qa", None).await).await;
    assert!(
        before
            .notice
            .contains("Your dental team can read this conversation")
    );
    assert!(before.demo_notice.is_some());
    assert!(before.available);
    assert_eq!((before.questions_left, before.turns.len()), (20, 0));

    let response = ask(
        &mut client,
        "Hi, this is Daniel Okafor. What happens during the root canal?",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let asked: AskResponse = json_body(response).await;
    let answer = &asked.turn.answer;
    assert_eq!(answer.kind, AnswerKind::Answered);
    assert_eq!(
        answer.sentences[0],
        "During a root canal, Dr. Patel cleans the inside of the tooth."
    );
    assert_eq!(answer.sources[0], "Your treatment plan");
    assert!(
        answer.sources[1].starts_with("Root canal treatment ("),
        "{:?}",
        answer.sources
    );
    assert!(answer.urgent_notice.is_none());
    assert_eq!(asked.questions_left, 19);

    // What the model saw: the approved text, facts and knowledge base, and
    // the question without the patient's name. No money, no names.
    let request = &qa.requests()[0];
    let user = &request.messages[1].content;
    assert!(user.contains("<approved_text>"));
    assert!(user.contains("[text:1]"));
    assert!(user.contains("[kb:root_canal#what_happens]"));
    assert!(user.contains("What happens during the root canal?</question>"));
    for forbidden in [
        "Daniel",
        "Okafor",
        "Patel",
        "Maple Grove",
        "$",
        "1150",
        "10482",
    ] {
        assert!(!user.contains(forbidden), "{forbidden} reached the model");
    }

    let after: QaConversation =
        json_body(client.send(Method::GET, "/api/present/qa", None).await).await;
    assert_eq!(after.turns.len(), 1);
    assert_eq!(after.turns[0].answer, asked.turn.answer);
    assert_eq!(after.questions_left, 19);
}

#[tokio::test]
async fn out_of_scope_questions_are_declined_with_a_question_for_the_team() {
    let declined = reply(
        "out_of_scope",
        &[(
            "Your dental team is the best place to ask about medicine.",
            &[],
        )],
        "Which medicine is right for me after the root canal?",
    );
    let (_, _, mut client, _, _) = presenting(vec![declined]).await;
    let asked: AskResponse =
        json_body(ask(&mut client, "Which painkiller should I take?").await).await;
    let answer = asked.turn.answer;
    assert_eq!(answer.kind, AnswerKind::OutOfScope);
    assert!(answer.refer_to_team);
    assert!(answer.sources.is_empty());
    assert_eq!(
        answer.suggested_question.as_deref(),
        Some("Which medicine is right for me after the root canal?")
    );
}

#[tokio::test]
async fn failing_answers_are_replaced_by_the_fallback() {
    let replies = vec![
        reply("answered", &[("The crown costs $1,150.", &["fact:p3"])], ""),
        reply(
            "answered",
            &[("A crown covers the tooth.", &["text:99"])],
            "",
        ),
        "this is not json".to_owned(),
    ];
    let (_, _, mut client, id, _) = presenting(replies).await;
    for question in [
        "How much is the crown?",
        "What is a crown?",
        "What is a filling?",
    ] {
        let asked: AskResponse = json_body(ask(&mut client, question).await).await;
        let answer = asked.turn.answer;
        assert_eq!(answer.kind, AnswerKind::Fallback, "{question}");
        assert_eq!(answer.sentences, [dc_generation::qa::FALLBACK]);
        assert!(answer.sources.is_empty());
    }

    leave(&mut client).await;
    let staff: StaffQuestions = json_body(staff_questions(&mut client, &id).await).await;
    let outcomes: Vec<&str> = staff.turns.iter().map(|t| t.outcome.as_str()).collect();
    assert_eq!(outcomes, ["blocked: V8", "blocked: V3", "unreadable"]);
    assert_eq!(staff.turns[0].question, "How much is the crown?");
    // The blocked text itself is never stored or shown.
    let json = serde_json::to_string(&staff).unwrap();
    assert!(!json.contains("1,150"));
}

#[tokio::test]
async fn urgent_questions_get_the_escalation_text_and_flag_the_presentation() {
    let answer = reply(
        "urgent",
        &[("Please contact {{practice_name}} right away.", &[])],
        "",
    );
    let (_, db, mut client, id, _) = presenting(vec![answer]).await;
    let asked: AskResponse =
        json_body(ask(&mut client, "My face is swollen since yesterday").await).await;
    let answer = asked.turn.answer;
    assert_eq!(answer.kind, AnswerKind::Urgent);
    let notice = answer.urgent_notice.unwrap();
    assert!(notice.contains("Please call Maple Grove Family Dental right away"));
    assert!(notice.contains("emergency services"));
    assert_eq!(
        answer.sentences,
        ["Please contact Maple Grove Family Dental right away."]
    );
    assert_eq!(audit_count(&db, "qa_urgent").await, 1);

    // With the budget used up, an ordinary question is refused but an urgent
    // one still gets the escalation text, without a model call.
    sqlx::query(
        "INSERT INTO ai_usage (month, cost_usd, calls) VALUES (strftime('%Y-%m', 'now'), 25.0, 1)
         ON CONFLICT (month) DO UPDATE SET cost_usd = 25.0",
    )
    .execute(&db)
    .await
    .unwrap();
    let refused = ask(&mut client, "What is a crown?").await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(refused).await, "budget_reached");
    let urgent: AskResponse = json_body(ask(&mut client, "I can't stop the bleeding").await).await;
    assert_eq!(urgent.turn.answer.kind, AnswerKind::Urgent);
    assert!(urgent.turn.answer.urgent_notice.is_some());
    assert!(urgent.turn.answer.sentences.is_empty());
    // The refused question was not counted.
    assert_eq!(urgent.questions_left, 18);

    leave(&mut client).await;
    let staff: StaffQuestions = json_body(staff_questions(&mut client, &id).await).await;
    assert!(staff.urgent);
    assert_eq!(staff.turns.len(), 2);
    assert!(staff.turns.iter().all(|t| t.urgent));
    assert_eq!(staff.turns[1].outcome, "budget_reached");
}

#[tokio::test]
async fn questions_are_checked_against_the_limits() {
    let (_, db, mut client, _, _) = presenting(Vec::new()).await;
    let empty = ask(&mut client, "   ").await;
    assert_eq!(empty.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(empty).await, "empty_question");
    let long = ask(&mut client, &"a".repeat(501)).await;
    assert_eq!(long.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(long).await, "question_too_long");
    let personal = ask(&mut client, "Daniel Okafor 555-123-4567").await;
    assert_eq!(error_code(personal).await, "personal_details");

    // Twenty questions already asked in this present session.
    let (token_hash, presentation_id): (String, String) =
        sqlx::query_as("SELECT token_hash, presentation_id FROM present_sessions")
            .fetch_one(&db)
            .await
            .unwrap();
    for _ in 0..20 {
        sqlx::query(
            "INSERT INTO qa_conversations (presentation_id, version, present_token_hash,
                 asked_at, question, answer_json, kind, outcome, prompt_version, model_id)
             VALUES (?, 1, ?, 0, 'q', '{}', 'fallback', 'passed', 'v', 'm')",
        )
        .bind(&presentation_id)
        .bind(&token_hash)
        .execute(&db)
        .await
        .unwrap();
    }
    let limited = ask(&mut client, "What is a crown?").await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error_code(limited).await, "question_limit");
    // Rows older than the retention period are deleted.
    assert_eq!(crate::qa::purge_expired(&db).await.unwrap(), 20);
}

#[tokio::test]
async fn staff_read_the_conversation_and_other_visitors_cannot() {
    let answer = reply(
        "answered",
        &[("A crown covers the whole tooth.", &["fact:p3"])],
        "",
    );
    let (app, db, mut client, id, _) = presenting(vec![answer]).await;
    assert_eq!(
        ask(&mut client, "What is a crown?").await.status(),
        StatusCode::OK
    );

    // The locked session cannot read it while the patient is presenting,
    // and the present cookie never opens staff routes.
    assert_eq!(
        staff_questions(&mut client, &id).await.status(),
        StatusCode::LOCKED
    );
    let mut patient = Client::new(app.clone());
    patient.present = client.present.clone();
    assert_eq!(
        staff_questions(&mut patient, &id).await.status(),
        StatusCode::UNAUTHORIZED
    );

    leave(&mut client).await;
    let staff: StaffQuestions = json_body(staff_questions(&mut client, &id).await).await;
    assert_eq!(staff.presentation_id, id);
    assert!(!staff.urgent);
    assert_eq!(staff.turns.len(), 1);
    let turn = &staff.turns[0];
    assert_eq!(turn.question, "What is a crown?");
    assert_eq!(turn.answer.sentences, ["A crown covers the whole tooth."]);
    assert_eq!((turn.session, turn.version), (1, 1));
    assert_eq!(turn.outcome, "passed");
    // An ended present session can no longer ask.
    assert_eq!(
        ask(&mut patient, "What is a crown?").await.status(),
        StatusCode::UNAUTHORIZED
    );

    let mut other = visitor_of(&app).await;
    let response = staff_questions(&mut other, &id).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(response).await, "presentation_not_found");

    // The conversation goes with its presentation in the demo purge.
    sqlx::query("UPDATE presentations SET created_at = created_at - 86400")
        .execute(&db)
        .await
        .unwrap();
    crate::jobs::purge_demo_data(&db).await.unwrap();
    let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM qa_conversations")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}
