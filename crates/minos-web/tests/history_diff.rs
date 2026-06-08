//! `/history/diff` side-by-side comparison route.

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

fn svc(name: &str) -> ServiceConfig {
    ServiceConfig {
        name: name.into(),
        mode: ProxyMode::Reverse {
            bind: "127.0.0.1:8080".parse().unwrap(),
            upstream: "127.0.0.1:5000".parse().unwrap(),
        },
        protocol: ProtocolKind::Http,
        pipeline: vec![],
        block_response_override: None,
        max_body_bytes: 1024,
    }
}

/// State with two saved versions: v1 has service "alpha", v2 adds "beta".
fn state_with_two_versions() -> AppState {
    let v1 = Config {
        services: vec![svc("alpha")],
        ..Config::default()
    };
    let (bus, _rx) = new_bus(RuleSet::empty_for(v1.clone()));
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let mut registry = FilterRegistry::new();
    register_builtin_filters(&mut registry);
    let registry = Arc::new(registry);
    save_config(storage.as_ref(), &registry, &v1, Some("v1"), Some(&bus)).unwrap();

    let v2 = Config {
        services: vec![svc("alpha"), svc("beta")],
        ..Config::default()
    };
    save_config(storage.as_ref(), &registry, &v2, Some("v2"), Some(&bus)).unwrap();

    let (broadcast_tx, _sub) = tokio::sync::broadcast::channel(16);
    AppState::new(
        bus,
        storage,
        registry,
        Key::generate(),
        Arc::new(broadcast_tx),
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
async fn diff_renders_two_versions_side_by_side() {
    let app = router(state_with_two_versions());
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/history/diff?from=1&to=2")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let s = std::str::from_utf8(&body).unwrap();
    // Both versions' service names appear; only v2 has "beta".
    assert!(s.contains("alpha"), "v1/v2 both contain alpha");
    assert!(s.contains("beta"), "v2 contains beta");
    assert!(s.contains("v1") && s.contains("v2"), "headers present");
}

#[tokio::test]
async fn diff_rejects_missing_params() {
    let app = router(state_with_two_versions());
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/history/diff?from=1")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
