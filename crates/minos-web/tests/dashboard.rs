//! Dashboard route tests.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::Key;
use http_body_util::BodyExt;
use minos_config::{new_bus, Config, RuleSet, ServiceConfig};
use minos_core::{FilterRegistry, ProtocolKind, ProxyMode};
use minos_storage::{InMemoryStorage, Storage};
use minos_web::{routes::router, AppState};
use tower::ServiceExt;

fn state_with_one_service() -> AppState {
    let cfg = Config {
        services: vec![ServiceConfig {
            name: "svc".into(),
            mode: ProxyMode::Reverse {
                bind: "127.0.0.1:8080".parse().unwrap(),
                upstream: "127.0.0.1:5000".parse().unwrap(),
            },
            protocol: ProtocolKind::Http,
            pipeline: vec![],
            block_response_override: None,
            max_body_bytes: 1024,
        }],
        ..Config::default()
    };
    let (bus, _rx) = new_bus(RuleSet::empty_for(cfg));
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
    let set = res
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    // strip attributes after the first ';' to get just `name=value`.
    set.split(';').next().unwrap().to_string()
}

#[tokio::test]
async fn dashboard_redirects_when_unauthenticated() {
    let app = router(state_with_one_service());
    let res = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/login");
}

#[tokio::test]
async fn dashboard_renders_after_login() {
    let app = router(state_with_one_service());
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let s = std::str::from_utf8(&body).unwrap();
    assert!(s.contains("svc"));
    assert!(s.contains("Reverse"));
    assert!(s.contains("127.0.0.1:8080"));
}
