//! `/services/{name}/save-progress` WebSocket save flow. Uses a regex draft
//! (no `python_sidecar`) so the test needs no `uv` on PATH; it asserts the WS
//! reports success and the config is persisted + the draft cleared.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request};
use axum_extra::extract::cookie::Key;
use futures_util::StreamExt;
use minos_config::{new_bus, Config, RuleSet, ServiceConfig};
use minos_core::{FilterRegistry, ProtocolKind, ProxyMode};
use minos_filters::register_builtin_filters;
use minos_storage::{InMemoryStorage, Storage};
use minos_web::{routes::router, AppState};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
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

fn state() -> AppState {
    let cfg = Config {
        services: vec![svc("svc")],
        ..Config::default()
    };
    let (bus, _rx) = new_bus(RuleSet::empty_for(cfg.clone()));
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    // Seed an initial active version so save produces v2.
    let mut registry = FilterRegistry::new();
    register_builtin_filters(&mut registry);
    let registry = Arc::new(registry);
    minos_config::save_config(storage.as_ref(), &registry, &cfg, Some("seed"), None).unwrap();
    let (broadcast_tx, _sub) = tokio::sync::broadcast::channel(16);
    AppState::new(
        bus,
        storage,
        registry,
        Key::generate(),
        Arc::new(broadcast_tx),
    )
}

async fn login_cookie(state: &AppState) -> String {
    let res = router(state.clone())
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

/// Create a draft regex filter via the "rule from match" POST.
async fn seed_draft(state: &AppState, cookie: &str) {
    router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/services/svc/filters/new")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("kind=regex&display_name=block&pattern=evil"))
                .unwrap(),
        )
        .await
        .unwrap();
}

async fn spawn_server(state: AppState) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn save_progress_reports_ok_and_persists() {
    let state = state();
    let cookie = login_cookie(&state).await;
    seed_draft(&state, &cookie).await;
    assert!(state.drafts.get("svc").is_some(), "draft should exist");

    let addr = spawn_server(state.clone()).await;
    let url = format!("ws://{addr}/services/svc/save-progress");
    let mut req = url.into_client_request().unwrap();
    req.headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    let (mut ws, _resp) = tokio_tungstenite::connect_async(req).await.unwrap();

    // Read frames until a terminal {"status": ...} arrives.
    let mut status = None;
    while let Ok(Some(msg)) = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap_or(None)
        .transpose()
    {
        if let Message::Text(t) = msg {
            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
            if v.get("status").is_some() {
                status = Some(v);
                break;
            }
        }
    }

    let status = status.expect("a terminal status frame");
    assert_eq!(status["status"], "ok", "got: {status}");

    // Draft cleared, and a new version is active.
    assert!(state.drafts.get("svc").is_none(), "draft should be cleared");
    let active = state.storage.active_version().unwrap();
    assert!(active >= 2, "expected a new version, got v{active}");
}
