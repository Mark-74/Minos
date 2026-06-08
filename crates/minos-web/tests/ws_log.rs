//! `/log/ws` live-feed tests. These need a real listener (a long-lived
//! WebSocket upgrade can't go through `tower::ServiceExt::oneshot`), so we
//! bind `axum::serve` on `127.0.0.1:0` and drive it with a tungstenite
//! client. Login uses `oneshot` on a clone of the same router — the signed
//! cookie validates across both because they share one `AppState` (and thus
//! one cookie key).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request};
use axum_extra::extract::cookie::Key;
use futures_util::StreamExt;
use minos_config::{new_bus, Config, RuleSet};
use minos_core::{Direction, FilterRegistry, LogEntry};
use minos_proxy::LogBroadcast;
use minos_storage::{InMemoryStorage, Storage};
use minos_web::{routes::router, AppState};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;
use uuid::Uuid;

fn state() -> (AppState, LogBroadcast) {
    let (bus, _rx) = new_bus(RuleSet::empty_for(Config::default()));
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let registry = Arc::new(FilterRegistry::new());
    let (broadcast_tx, _sub) = tokio::sync::broadcast::channel(64);
    let state = AppState::new(
        bus,
        storage,
        registry,
        Key::generate(),
        Arc::new(broadcast_tx.clone()),
    );
    (state, broadcast_tx)
}

fn entry(service: &str) -> LogEntry {
    LogEntry {
        id: None,
        ts: 1_700_000_000_000,
        service: service.into(),
        direction: Direction::Inbound,
        filter_id: Uuid::nil(),
        rule_kind: "regex".into(),
        dry_run: false,
        reason: "matched".into(),
        sample: b"GET /evil".to_vec(),
    }
}

/// Log in via `oneshot` and return the session cookie pair (`name=value`).
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

/// Spawn the router on an OS-chosen port; return the bound address.
async fn spawn_server(state: AppState) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    addr
}

type WsClient =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(addr: std::net::SocketAddr, path: &str, cookie: &str) -> WsClient {
    let url = format!("ws://{addr}{path}");
    let mut req = url.into_client_request().unwrap();
    req.headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    let (ws, _resp) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws
}

async fn next_text(ws: &mut WsClient) -> String {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .expect("frame within timeout")
            .expect("stream open")
            .expect("ws frame");
        if let Message::Text(t) = msg {
            return t.to_string();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn broadcast_entry_reaches_subscriber_as_json() {
    let (state, tx) = state();
    let cookie = login_cookie(&state).await;
    let addr = spawn_server(state).await;

    let mut ws = connect(addr, "/log/ws", &cookie).await;
    // Give the handler a moment to subscribe before broadcasting.
    tokio::time::sleep(Duration::from_millis(100)).await;
    tx.send(entry("svc-a")).unwrap();

    let frame = next_text(&mut ws).await;
    assert!(frame.contains("\"service\":\"svc-a\""), "got: {frame}");
    assert!(frame.contains("\"kind\":\"regex\""), "got: {frame}");
    // sample is pre-rendered, not a raw byte array.
    assert!(frame.contains("GET /evil"), "got: {frame}");
}

#[tokio::test(flavor = "multi_thread")]
async fn service_filter_excludes_non_matching_entries() {
    let (state, tx) = state();
    let cookie = login_cookie(&state).await;
    let addr = spawn_server(state).await;

    let mut ws = connect(addr, "/log/ws?service=foo", &cookie).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    tx.send(entry("bar")).unwrap(); // filtered out
    tx.send(entry("foo")).unwrap(); // kept

    let frame = next_text(&mut ws).await;
    assert!(frame.contains("\"service\":\"foo\""), "got: {frame}");
    assert!(!frame.contains("\"service\":\"bar\""), "got: {frame}");
}
