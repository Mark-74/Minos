//! History route: list versions and rollback.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::Key;
use http_body_util::BodyExt;
use minos_config::{new_bus, save_config, Config, RuleSet, ServiceConfig};
use minos_core::{FilterRegistry, ProtocolKind, ProxyMode};
use minos_filters::register_builtin_filters;
use minos_storage::{InMemoryStorage, Storage};
use minos_web::{routes::router, AppState};
use tower::ServiceExt;

fn state_with_versions() -> AppState {
    let (bus, _rx) = new_bus(RuleSet::empty_for(Config::default()));
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let mut registry = FilterRegistry::new();
    register_builtin_filters(&mut registry);
    let registry = Arc::new(registry);
    let state = AppState::new(
        bus.clone(),
        storage.clone(),
        registry.clone(),
        Key::generate(),
    );

    let svc = ServiceConfig {
        name: "svc".into(),
        mode: ProxyMode::Reverse {
            bind: "127.0.0.1:8080".parse().unwrap(),
            upstream: "127.0.0.1:5000".parse().unwrap(),
        },
        protocol: ProtocolKind::Http,
        pipeline: vec![],
        block_response_override: None,
        max_body_bytes: 1024,
    };
    let cfg = Config {
        services: vec![svc],
        ..Config::default()
    };
    save_config(
        storage.as_ref(),
        &registry,
        &cfg,
        Some("initial"),
        Some(&bus),
    )
    .unwrap();
    state
}

async fn login_cookie(app: &axum::Router) -> String {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("password=p"))
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
async fn history_lists_versions() {
    let state = state_with_versions();
    let app = router(state);
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/history")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let s = std::str::from_utf8(&body).unwrap();
    assert!(s.contains("v1"), "expected 'v1' in body");
    assert!(s.contains("initial"), "expected note 'initial' in body");
}

#[tokio::test]
async fn rollback_creates_new_version_pointing_to_same_blob() {
    let state = state_with_versions();
    let storage_handle = state.storage.clone();
    let app = router(state);
    let cookie = login_cookie(&app).await;

    // Initially: 1 version.
    assert_eq!(storage_handle.list_versions().unwrap().len(), 1);

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/history/1/rollback")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    // After rollback: 2 versions (rollback created a new one).
    let versions = storage_handle.list_versions().unwrap();
    assert_eq!(versions.len(), 2);
    assert!(
        versions[0]
            .note
            .as_deref()
            .unwrap_or("")
            .contains("rollback to v1"),
        "expected rollback note on newest version"
    );
}
