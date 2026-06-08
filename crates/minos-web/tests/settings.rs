//! Settings route tests.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::Key;
use minos_config::{new_bus, Config, RuleSet};
use minos_core::FilterRegistry;
use minos_storage::{InMemoryStorage, Storage};
use minos_web::{routes::router, AppState};
use tower::ServiceExt;

fn state() -> AppState {
    let (bus, _rx) = new_bus(RuleSet::empty_for(Config::default()));
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let registry = Arc::new(FilterRegistry::new());
    AppState::new(
        bus,
        storage,
        registry,
        Key::generate(),
        Arc::new(tokio::sync::broadcast::channel(16).0),
    )
}

async fn login_cookie(app: &axum::Router) -> String {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("password=hunter2"))
                .unwrap(),
        )
        .await
        .unwrap();
    res.headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn password_change_succeeds_with_correct_current() {
    let app = router(state());
    let cookie = login_cookie(&app).await; // also seeds the initial hash

    // Change password.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/password")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, &cookie)
                .body(Body::from("current=hunter2&new=newpass&confirm=newpass"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers().get("location").unwrap(),
        "/settings?ok=password"
    );

    // Confirm new password works for login (after clearing cookie).
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("password=newpass"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/");
}

#[tokio::test]
async fn password_change_rejects_wrong_current() {
    let app = router(state());
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/password")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, cookie)
                .body(Body::from("current=wrong&new=newpass&confirm=newpass"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let loc = res.headers().get("location").unwrap().to_str().unwrap();
    assert!(loc.contains("error="), "got {loc}");
}

#[tokio::test]
async fn defaults_post_persists_block_response() {
    let app = router(state());
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/defaults")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, cookie)
                .body(Body::from(
                    "status=418&body=teapot&headers_text=X-Reason%3A+blocked",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers().get("location").unwrap(),
        "/settings?ok=defaults"
    );
}
