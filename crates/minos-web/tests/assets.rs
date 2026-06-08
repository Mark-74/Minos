//! Asset handler smoke tests.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
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

#[tokio::test]
async fn style_css_is_served_with_correct_mime() {
    let app = router(state());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/assets/style.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers().get("content-type").unwrap(), "text/css");
}

#[tokio::test]
async fn htmx_js_is_served_with_correct_mime() {
    let app = router(state());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/assets/htmx.min.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(
        ct.starts_with("application/javascript") || ct.starts_with("text/javascript"),
        "got content-type {ct}"
    );
}

#[tokio::test]
async fn missing_asset_returns_404() {
    let app = router(state());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/assets/missing.png")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
