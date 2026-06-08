//! End-to-end smoke test of the assembled data plane.
//!
//! Wires the same components `minos::run` does — `SqliteStorage`, the builtin
//! `FilterRegistry`, `save_config`/`load_active_config`, the bus, the
//! log-writer-with-broadcast, and a real `listen_service` — then drives HTTP
//! through the proxy. Asserts a clean request is forwarded, an `EVIL` request
//! is blocked, and the block lands in storage via the log writer.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use minos_config::{
    load_active_config, new_bus, save_config, Config, FilterInstanceCfg, ServiceConfig,
};
use minos_core::{FilterRegistry, LogFilter, ProtocolKind, ProxyMode};
use minos_filters::register_builtin_filters;
use minos_proxy::{listen_service, spawn_log_writer};
use minos_storage::{SqliteStorage, Storage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

async fn spawn_upstream() -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                    )
                    .await;
                let _ = s.shutdown().await;
            });
        }
    });
    addr
}

fn pick_free_port() -> SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    addr
}

async fn http_roundtrip(bind: SocketAddr, path: &str) -> String {
    let mut c = tokio::net::TcpStream::connect(bind).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: y\r\nConnection: close\r\n\r\n");
    c.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = c.read_to_end(&mut out).await;
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn assembled_data_plane_passes_clean_and_blocks_evil() {
    let upstream = spawn_upstream().await;
    let bind = pick_free_port();

    // Storage + registry, exactly as the binary builds them.
    let storage = Arc::new(SqliteStorage::open_in_memory().unwrap());
    let mut registry = FilterRegistry::new();
    register_builtin_filters(&mut registry);

    // One HTTP reverse-proxy service with a real regex filter (live, not dry).
    let svc = ServiceConfig {
        name: "svc".into(),
        mode: ProxyMode::Reverse { bind, upstream },
        protocol: ProtocolKind::Http,
        pipeline: vec![FilterInstanceCfg {
            id: Uuid::new_v4(),
            display_name: "block-evil".into(),
            kind: "regex".into(),
            config: serde_json::json!({ "pattern": "EVIL" }),
            enabled: true,
            dry_run: false,
            on_inbound: true,
            on_outbound: false,
        }],
        block_response_override: None,
        max_body_bytes: 4096,
    };
    let cfg = Config {
        services: vec![svc],
        ..Config::default()
    };

    // Persist + load through the real config path, then build the bus.
    save_config(storage.as_ref(), &registry, &cfg, Some("smoke"), None).unwrap();
    let ruleset = load_active_config(storage.as_ref(), &registry).unwrap();
    let services = ruleset.source.services.clone();
    let (bus, log_rx) = new_bus(ruleset);

    let (log_tx, _sub) = tokio::sync::broadcast::channel(64);
    let _writer = spawn_log_writer(log_rx, Arc::clone(&storage), log_tx);

    let _handles: Vec<_> = services
        .into_iter()
        .map(|s| listen_service(s, bus.clone()))
        .collect();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Clean request → forwarded → upstream's 200/hello.
    let clean = http_roundtrip(bind, "/good").await;
    assert!(clean.contains("HTTP/1.1 200"), "clean: {clean:?}");
    assert!(clean.ends_with("hello"), "clean body: {clean:?}");

    // EVIL request → blocked → 403, never reaches upstream.
    let bad = http_roundtrip(bind, "/EVIL").await;
    assert!(bad.contains("HTTP/1.1 403"), "blocked: {bad:?}");

    // The block was logged to storage via the writer task.
    let mut logged = 0;
    for _ in 0..20 {
        logged = storage.query_log(&LogFilter::default(), 100).unwrap().len();
        if logged > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(logged >= 1, "expected a block log entry to be persisted");
}
