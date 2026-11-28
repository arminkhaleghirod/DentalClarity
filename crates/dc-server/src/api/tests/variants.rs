//! Language and reading level variants in Present mode (DECISIONS E32).

use super::present::{content, start_present};
use super::*;
use dc_core::content::ReadingLevel;
use dc_core::present::{PatientView, VariantState, audio_key};
use dc_generation::variant::{VariantBlock, VariantText};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A translator that answers in plausible Spanish or simple English, keeping
/// each source sentence's numbers and placeholders. Sections whose source
/// contains `untranslated` are answered with the English unchanged, which
/// the language check rejects.
#[derive(Default)]
struct FakeTranslator {
    calls: AtomicUsize,
    requests: Mutex<Vec<String>>,
    untranslated: Option<&'static str>,
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> &'a str {
    let start = text.find(open).unwrap() + open.len();
    let end = text[start..].find(close).unwrap() + start;
    text[start..end].trim()
}

/// Digits and placeholders of `sentence`, to carry into the answer.
fn kept(sentence: &str) -> String {
    let mut out = String::new();
    for word in sentence.split_whitespace() {
        let digits: String = word.chars().filter(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            out.push(' ');
            out.push_str(&digits);
        }
    }
    let mut rest = sentence;
    while let Some(start) = rest.find("{{") {
        let end = rest[start..].find("}}").unwrap() + start + 2;
        out.push(' ');
        out.push_str(&rest[start..end]);
        rest = &rest[end..];
    }
    out
}

#[async_trait::async_trait]
impl TextModel for FakeTranslator {
    fn provider(&self) -> &'static str {
        "fake"
    }

    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let user = request
            .messages
            .iter()
            .find(|m| m.role == Role::User)
            .unwrap()
            .content
            .clone();
        self.requests.lock().unwrap().push(user.clone());
        let source: VariantText =
            serde_json::from_str(between(&user, "<approved_text>", "</approved_text>")).unwrap();
        let english = user.contains("(en)");
        let answer = if self.untranslated.is_some_and(|u| user.contains(u)) {
            source
        } else {
            VariantText {
                blocks: source
                    .blocks
                    .iter()
                    .map(|b| VariantBlock {
                        block_id: b.block_id.clone(),
                        heading: match (b.heading.is_empty(), english) {
                            (true, _) => String::new(),
                            (false, true) => "About this part".into(),
                            (false, false) => "Sobre esta parte".into(),
                        },
                        sentences: b
                            .sentences
                            .iter()
                            .map(|s| {
                                let extra = kept(s);
                                match (b.block_id == "questions", english) {
                                    (true, true) => format!("What can I ask about this part{extra}?"),
                                    (true, false) => {
                                        format!("¿Qué puedo preguntar sobre esta parte{extra}?")
                                    }
                                    (false, true) => {
                                        format!("This part tells you about the plan in short words{extra}.")
                                    }
                                    (false, false) => {
                                        format!("Esta parte explica el plan con palabras sencillas{extra}.")
                                    }
                                }
                            })
                            .collect(),
                    })
                    .collect(),
            }
        };
        Ok(TextResponse {
            model: request.model,
            route: None,
            content: serde_json::to_string(&answer).unwrap(),
            usage: dc_ai::Usage::default(),
            latency_ms: 0,
        })
    }
}

/// A translation reviewer that always passes.
#[derive(Default)]
struct PassingReviewer {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl TextModel for PassingReviewer {
    fn provider(&self) -> &'static str {
        "fake"
    }

    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(TextResponse {
            model: request.model,
            route: None,
            content: json!({ "verdict": "pass", "issues": [] }).to_string(),
            usage: dc_ai::Usage::default(),
            latency_ms: 0,
        })
    }
}

struct Setup {
    db: SqlitePool,
    state: AppState,
    client: Client,
    translator: Arc<FakeTranslator>,
    reviewer: Arc<PassingReviewer>,
}

/// A demo visitor presenting Daniel's plan, created in `language`, with the
/// fake translator for variants.
async fn presenting(language: &str, translator: FakeTranslator) -> Setup {
    let translator = Arc::new(translator);
    let reviewer = Arc::new(PassingReviewer::default());
    let (t, r) = (translator.clone(), reviewer.clone());
    let (app, db, state) = app_with(Mode::Demo, move |db| {
        scripted(1, &[], None)(db)
            .map(|g| g.with_variant_models(t, "fake-translator".into(), r, "fake-reviewer".into()))
    })
    .await;
    let mut client = visitor_of(&app).await;
    let mut request = daniel_request();
    request["language"] = json!(language);
    let created: CreatePresentationResponse = json_body(
        client
            .send(Method::POST, "/api/staff/presentations", Some(request))
            .await,
    )
    .await;
    wait_for_job(&mut client, &created.job_id).await;
    let id = created.presentation_id;
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    Setup {
        db,
        state,
        client,
        translator,
        reviewer,
    }
}

async fn request_variant(client: &mut Client, language: &str, level: &str) -> Response {
    client
        .send(
            Method::POST,
            "/api/present/variant",
            Some(json!({ "language": language, "reading_level": level })),
        )
        .await
}

/// Reads the variant until no section is pending.
async fn settled(client: &mut Client, query: &str) -> PatientView {
    for _ in 0..500 {
        let response = client
            .send(Method::GET, &format!("/api/present/content{query}"), None)
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let view: PatientView = json_body(response).await;
        if !view.pending() {
            return view;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the variant did not settle");
}

async fn variant_rows(db: &SqlitePool, state: &str) -> i64 {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM section_variants WHERE state = ?")
        .bind(state)
        .fetch_one(db)
        .await
        .unwrap();
    n
}

#[tokio::test]
async fn spanish_is_derived_once_cached_and_narrated_in_spanish() {
    let Setup {
        db,
        state,
        mut client,
        translator,
        reviewer,
        ..
    } = presenting("en", FakeTranslator::default()).await;
    let english: PatientView = json_body(content(&mut client).await).await;
    assert_eq!(english.language, "en");
    assert_eq!(english.available_languages.len(), 10);
    assert!(
        english
            .sections
            .iter()
            .all(|s| s.variant == VariantState::Ready)
    );
    assert_eq!(translator.calls.load(Ordering::SeqCst), 0);

    // Reading without asking starts nothing.
    let unasked = settled(&mut client, "?language=es&level=standard").await;
    assert!(
        unasked
            .sections
            .iter()
            .all(|s| s.variant == VariantState::Unavailable)
    );
    assert_eq!(translator.calls.load(Ordering::SeqCst), 0);

    let response = request_variant(&mut client, "es", "standard").await;
    assert_eq!(response.status(), StatusCode::OK);
    let spanish = settled(&mut client, "?language=es&level=standard").await;
    assert_eq!(spanish.language, "es");
    assert_eq!(spanish.sections.len(), english.sections.len());
    let sections = spanish.sections.len();
    assert!(
        spanish
            .sections
            .iter()
            .all(|s| s.variant == VariantState::Ready)
    );
    assert_eq!(translator.calls.load(Ordering::SeqCst), sections);
    assert_eq!(reviewer.calls.load(Ordering::SeqCst), sections);
    assert_eq!(variant_rows(&db, "ready").await, sections as i64);

    let voice = crate::narration::speech_model_id(&state);
    let glance = &spanish.sections[0];
    let sentence = &glance.blocks[0].sentences[0];
    assert!(
        sentence.text.starts_with("Esta parte explica"),
        "{}",
        sentence.text
    );
    assert_eq!(sentence.audio_key, audio_key(voice, "es", &sentence.text));
    assert_ne!(
        sentence.audio_key,
        english.sections[0].blocks[0].sentences[0].audio_key
    );
    // Locked facts are rendered by the app, unchanged.
    assert_eq!(spanish.procedures, english.procedures);
    // Placeholders are filled locally after translation.
    let json = serde_json::to_string(&spanish).unwrap();
    assert!(!json.contains("{{"));
    let next = spanish
        .sections
        .iter()
        .find(|s| s.key == "next_steps")
        .unwrap();
    assert!(next.questions.iter().all(|q| q.starts_with('¿')));

    // The cache answers the second request with no model call.
    let again: PatientView = json_body(request_variant(&mut client, "es", "standard").await).await;
    assert!(!again.pending());
    assert_eq!(again, spanish);
    assert_eq!(translator.calls.load(Ordering::SeqCst), sections);

    // Narration keys of the Spanish text belong to this version.
    let keys = glance.blocks[0]
        .sentences
        .iter()
        .map(|s| s.audio_key.clone())
        .collect::<Vec<_>>()
        .join(",");
    let response = client
        .send(
            Method::GET,
            &format!("/api/present/narration?keys={keys}"),
            None,
        )
        .await;
    assert_ne!(response.status(), StatusCode::NOT_FOUND);
    let response = client
        .send(
            Method::GET,
            &format!("/api/present/audio/{}", sentence.audio_key),
            None,
        )
        .await;
    assert_ne!(response.status(), StatusCode::NOT_FOUND);
    let other = audio_key(voice, "es", "Una frase que no está aquí.");
    let response = client
        .send(Method::GET, &format!("/api/present/audio/{other}"), None)
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn english_at_another_level_is_rewritten_and_failures_fall_back() {
    let Setup {
        db,
        mut client,
        translator,
        ..
    } = presenting(
        "en",
        FakeTranslator {
            untranslated: Some("Each part of the plan"),
            ..FakeTranslator::default()
        },
    )
    .await;
    let english: PatientView = json_body(content(&mut client).await).await;

    // English rewritten at the simple level.
    request_variant(&mut client, "en", "simple").await;
    let simple = settled(&mut client, "?language=en&level=simple").await;
    assert_eq!(simple.reading_level, ReadingLevel::Simple);
    assert!(
        simple
            .sections
            .iter()
            .all(|s| s.variant == VariantState::Ready)
    );
    assert!(
        simple.sections[1].blocks[0].sentences[0]
            .text
            .starts_with("This part tells you")
    );
    let after_english = translator.calls.load(Ordering::SeqCst);

    // At a glance comes back in English, which fails the Spanish language
    // check twice; it is shown in English and marked, never mixed.
    request_variant(&mut client, "es", "standard").await;
    let spanish = settled(&mut client, "?language=es&level=standard").await;
    let glance = &spanish.sections[0];
    assert_eq!(glance.key, "at_a_glance");
    assert_eq!(glance.variant, VariantState::Unavailable);
    assert_eq!(glance.blocks, english.sections[0].blocks);
    assert!(
        spanish.sections[1..]
            .iter()
            .all(|s| s.variant == VariantState::Ready)
    );
    assert_eq!(variant_rows(&db, "failed").await, 1);
    let calls = translator.calls.load(Ordering::SeqCst);
    // One repair for the failing section.
    assert_eq!(calls - after_english, spanish.sections.len() + 1);

    // A failed section is final for its source text: asking again costs nothing.
    request_variant(&mut client, "es", "standard").await;
    assert_eq!(translator.calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn only_approved_text_reaches_the_translator() {
    let Setup {
        db,
        mut client,
        translator,
        ..
    } = presenting("en", FakeTranslator::default()).await;
    sqlx::query(
        "UPDATE presentation_sections SET state = 'needs_review' WHERE key = 'procedure:p2'",
    )
    .execute(&db)
    .await
    .unwrap();
    request_variant(&mut client, "es", "detailed").await;
    let spanish = settled(&mut client, "?language=es&level=detailed").await;
    assert!(spanish.sections.iter().all(|s| s.key != "procedure:p2"));
    let requests = translator.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), spanish.sections.len());
    let all = requests.join("\n");
    // The unconfirmed note-derived reason is replaced by common reasons.
    assert!(!all.contains("decay that reached the nerve"));
    // Names and the practice name never go to the model.
    assert!(!all.contains("Daniel"));
    assert!(!all.contains("Maple Grove"));
}

#[tokio::test]
async fn the_presentation_language_is_prepared_when_present_mode_starts() {
    let Setup {
        mut client,
        translator,
        ..
    } = presenting("es", FakeTranslator::default()).await;
    let view = settled(&mut client, "").await;
    assert_eq!(view.language, "es");
    assert!(
        view.sections
            .iter()
            .all(|s| s.variant == VariantState::Ready)
    );
    assert_eq!(translator.calls.load(Ordering::SeqCst), view.sections.len());
}

#[tokio::test]
async fn without_a_provider_variants_stay_in_english() {
    let Setup {
        state, mut client, ..
    } = presenting("en", FakeTranslator::default()).await;
    // The same database and sessions, served without any provider (clinical
    // mode until M9, or a demo without an API key).
    client.app = router(AppState {
        generation: None,
        ..state
    });
    let response = request_variant(&mut client, "fr", "standard").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(response).await, "provider_not_configured");
    let french = settled(&mut client, "?language=fr&level=standard").await;
    assert!(
        french
            .sections
            .iter()
            .all(|s| s.variant == VariantState::Unavailable)
    );
    // The approved English itself needs no provider.
    let english = settled(&mut client, "?language=en&level=standard").await;
    assert!(
        english
            .sections
            .iter()
            .all(|s| s.variant == VariantState::Ready)
    );
}

#[tokio::test]
async fn requests_are_checked_and_new_variants_are_limited_in_the_demo() {
    let Setup { mut client, .. } = presenting("en", FakeTranslator::default()).await;
    let response = request_variant(&mut client, "xx", "standard").await;
    assert_eq!(error_code(response).await, "unsupported_language");
    let response = client
        .send(Method::GET, "/api/present/content?language=es", None)
        .await;
    assert_eq!(error_code(response).await, "bad_request");

    let combos = ["es", "fr", "pt", "vi"]
        .into_iter()
        .flat_map(|l| ["simple", "standard", "detailed"].map(move |v| (l, v)));
    let mut refused = None;
    for (n, (language, level)) in combos.enumerate() {
        let response = request_variant(&mut client, language, level).await;
        if response.status() != StatusCode::OK {
            refused = Some((n, response));
            break;
        }
        settled(&mut client, &format!("?language={language}&level={level}")).await;
    }
    let (n, response) = refused.expect("the limit applies");
    assert_eq!(
        n as i64,
        super::super::variants::DEMO_VARIANTS_PER_VISITOR_PER_HOUR
    );
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error_code(response).await, "rate_limited");
    // Cached variants stay free.
    let response = request_variant(&mut client, "es", "simple").await;
    assert_eq!(response.status(), StatusCode::OK);
}
