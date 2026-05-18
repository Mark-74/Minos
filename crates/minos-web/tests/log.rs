//! /log route tests.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::Key;
use http_body_util::BodyExt;
use minos_config::{new_bus, Config, RuleSet};
use minos_core::{Direction, FilterRegistry, LogEntry};
use minos_storage::{InMemoryStorage, Storage};
use minos_web::{routes::router, AppState};
use tower::ServiceExt;
use uuid::Uuid;

fn state_with_entries() -> AppState {
    let (bus, _rx) = new_bus(RuleSet::empty_for(Config::default()));
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    storage
        .append_log(&entry("svc-a", "regex", false, "matched evil"))
        .unwrap();
    storage
        .append_log(&entry("svc-b", "http", true, "would block"))
        .unwrap();
    let registry = Arc::new(FilterRegistry::new());
    AppState::new(bus, storage, registry, Key::generate())
}

fn entry(service: &str, kind: &str, dry_run: bool, reason: &str) -> LogEntry {
    LogEntry {
        id: None,
        ts: 1_700_000_000_000,
        service: service.into(),
        direction: Direction::Inbound,
        filter_id: Uuid::nil(),
        rule_kind: kind.into(),
        dry_run,
        reason: reason.into(),
        sample: b"GET /x".to_vec(),
    }
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
async fn log_shows_all_entries_by_default() {
    let app = router(state_with_entries());
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/log")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let s = std::str::from_utf8(&body).unwrap();
    assert!(s.contains("svc-a"));
    assert!(s.contains("svc-b"));
    assert!(s.contains("matched evil"));
    assert!(s.contains("would block"));
}

#[tokio::test]
async fn log_filters_by_service() {
    let app = router(state_with_entries());
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/log?service=svc-a")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let s = std::str::from_utf8(&body).unwrap();
    assert!(s.contains("svc-a"));
    assert!(!s.contains("svc-b"), "svc-b should be filtered out");
}
