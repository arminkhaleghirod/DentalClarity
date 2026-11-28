//! Practice logo upload and serving.

use super::present::*;
use super::*;
use crate::logo::tests::png;
use dc_core::present::PatientView;

impl Client {
    /// Sends raw bytes, as the Settings page uploads a file.
    async fn send_bytes(&mut self, method: Method, uri: &str, bytes: Vec<u8>) -> Response {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::ORIGIN, ORIGIN)
            // The browser's claim is ignored; the server reads the bytes.
            .header(header::CONTENT_TYPE, "image/png");
        if let Some(cookie) = &self.cookie {
            req = req.header(header::COOKIE, cookie);
        }
        if let Some(csrf) = &self.csrf {
            req = req.header("x-dc-csrf", csrf);
        }
        self.app
            .clone()
            .oneshot(req.body(Body::from(bytes)).unwrap())
            .await
            .unwrap()
    }
}

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" fill="#1B3A5C"/></svg>"##;

async fn staff() -> (Client, SqlitePool) {
    let (app, db) = app_with_db(Mode::Clinical).await;
    let mut client = Client::new(app);
    set_up(&mut client).await;
    (client, db)
}

#[tokio::test]
async fn staff_upload_a_logo_and_read_it_back() {
    let (mut client, db) = staff().await;
    let settings: PracticeSettings =
        json_body(client.send(Method::GET, "/api/staff/settings", None).await).await;
    assert_eq!(settings.logo_url, None);
    let none = client
        .send(Method::GET, "/api/staff/settings/logo", None)
        .await;
    assert_eq!(error_code(none).await, "logo_not_set");

    // Larger than the 64 KiB JSON limit, well under the 1 MB logo limit.
    let mut bytes = png(400, 200);
    bytes.resize(200 * 1024, 0);
    let response = client
        .send_bytes(Method::PUT, "/api/staff/settings/logo", bytes.clone())
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let saved: PracticeSettings = json_body(response).await;
    let url = saved.logo_url.expect("logo url");
    assert!(url.starts_with("/api/staff/settings/logo?v="));
    assert_eq!(audit_count(&db, "settings_changed").await, 1);

    let served = client.send(Method::GET, &url, None).await;
    assert_eq!(served.status(), StatusCode::OK);
    let h = served.headers();
    assert_eq!(h[header::CONTENT_TYPE], "image/png");
    assert_eq!(h["x-content-type-options"], "nosniff");
    let csp = h["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("sandbox") && csp.contains("default-src 'none'"));
    let body = axum::body::to_bytes(served.into_body(), 1 << 21)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), bytes.as_slice());

    // An SVG replaces it, and the type comes from the content.
    let response = client
        .send_bytes(Method::PUT, "/api/staff/settings/logo", SVG.into())
        .await;
    let replaced: PracticeSettings = json_body(response).await;
    assert_ne!(replaced.logo_url.as_deref(), Some(url.as_str()));
    let served = client
        .send(Method::GET, "/api/staff/settings/logo", None)
        .await;
    assert_eq!(served.headers()[header::CONTENT_TYPE], "image/svg+xml");

    // Saving the other settings keeps the logo.
    let kept: PracticeSettings = json_body(
        client
            .send(
                Method::PUT,
                "/api/staff/settings",
                Some(json!({ "practice_name": "Maple Grove", "brand_primary": "#1B3A5C", "brand_highlight": "#E8693F" })),
            )
            .await,
    )
    .await;
    assert_eq!(kept.logo_url, replaced.logo_url);

    let removed: PracticeSettings = json_body(
        client
            .send(Method::DELETE, "/api/staff/settings/logo", None)
            .await,
    )
    .await;
    assert_eq!(removed.logo_url, None);
}

#[tokio::test]
async fn unsafe_or_oversized_logos_are_refused() {
    let (mut client, db) = staff().await;
    let script = r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
    for bytes in [script.as_bytes().to_vec(), b"GIF89a".to_vec(), Vec::new()] {
        let response = client
            .send_bytes(Method::PUT, "/api/staff/settings/logo", bytes)
            .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error_code(response).await, "invalid_logo");
    }
    let mut big = png(10, 10);
    big.resize(crate::logo::MAX_BYTES + 10, 0);
    let response = client
        .send_bytes(Method::PUT, "/api/staff/settings/logo", big)
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_code(response).await, "logo_too_large");
    assert!(crate::logo::load(&db).await.unwrap().is_none());

    // Uploads are writes: they need the CSRF token and the session.
    client.csrf = None;
    let forged = client
        .send_bytes(Method::PUT, "/api/staff/settings/logo", png(10, 10))
        .await;
    assert_eq!(error_code(forged).await, "bad_csrf");
    let mut anonymous = Client::new(client.app.clone());
    let response = anonymous
        .send_bytes(Method::PUT, "/api/staff/settings/logo", png(10, 10))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn demo_logo_routes_are_read_only() {
    let mut client = Client::new(app(Mode::Demo).await);
    client.send(Method::POST, "/api/visit", None).await;
    client.session().await;
    let upload = client
        .send_bytes(Method::PUT, "/api/staff/settings/logo", png(10, 10))
        .await;
    assert_eq!(error_code(upload).await, "demo_read_only");
    let read = client
        .send(Method::GET, "/api/staff/settings/logo", None)
        .await;
    assert_eq!(error_code(read).await, "demo_read_only");
    let delete = client
        .send(Method::DELETE, "/api/staff/settings/logo", None)
        .await;
    assert_eq!(error_code(delete).await, "demo_read_only");
}

#[tokio::test]
async fn patient_screens_get_the_logo_through_the_present_session() {
    let (_app, db, _state, mut client, id) = demo_presentation().await;
    // Demo staff cannot upload; store one as a practice install would have.
    crate::logo::save(&db, crate::logo::Kind::Png, &png(64, 64))
        .await
        .unwrap();
    assert_eq!(
        start_present(&mut client, &id).await.status(),
        StatusCode::OK
    );
    let patient: PatientView = json_body(content(&mut client).await).await;
    let url = patient.brand.logo_url.expect("logo url");
    assert!(url.starts_with("/api/present/logo?v="));
    let served = client.send(Method::GET, &url, None).await;
    assert_eq!(served.status(), StatusCode::OK);
    assert_eq!(served.headers()[header::CONTENT_TYPE], "image/png");

    // Without a present session the route is closed.
    let mut stranger = Client::new(client.app.clone());
    let closed = stranger.send(Method::GET, "/api/present/logo", None).await;
    assert_eq!(closed.status(), StatusCode::UNAUTHORIZED);
}
