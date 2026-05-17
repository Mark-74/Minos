//! Verifies that `Bus::swap` is visible to subsequent accepts without
//! restarting the listener.

use std::net::SocketAddr;
use std::sync::Arc;

use minos_config::{new_bus, Config, RuleSet, ServiceConfig};
use minos_core::{Filter, FilterInstance, Packet, ProtocolKind, ProxyMode, Verdict};
use minos_proxy::listen_service;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

struct AlwaysBlock;

impl Filter for AlwaysBlock {
    fn kind(&self) -> &'static str {
        "always-block"
    }

    fn accepts(&self, _: &Packet) -> bool {
        true
    }

    fn inspect(&self, _: &Packet) -> Verdict {
        Verdict::Block {
            reason: "blocked".into(),
        }
    }
}

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
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
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

#[tokio::test(flavor = "multi_thread")]
async fn swap_takes_effect_on_next_accepted_connection() {
    let upstream = spawn_upstream().await;
    let bind = pick_free_port();

    let cfg = ServiceConfig {
        name: "svc".into(),
        mode: ProxyMode::Reverse { bind, upstream },
        protocol: ProtocolKind::Http,
        pipeline: vec![],
        block_response_override: None,
        max_body_bytes: 1024,
    };
    let initial = RuleSet {
        source: Config {
            services: vec![cfg.clone()],
            ..Config::default()
        },
        pipelines: vec![vec![]], // pass-all
    };
    let (bus, _rx) = new_bus(initial);
    let _handle = listen_service(cfg.clone(), bus.clone());

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    // First request: pass-all pipeline → upstream returns 200.
    {
        let mut c = tokio::net::TcpStream::connect(bind).await.unwrap();
        c.write_all(b"GET / HTTP/1.1\r\nHost: y\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut out = Vec::new();
        let _ = c.read_to_end(&mut out).await;
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("HTTP/1.1 200"), "first request: {s:?}");
    }

    // Swap in a block-all pipeline.
    let next = RuleSet {
        source: Config {
            services: vec![cfg.clone()],
            ..Config::default()
        },
        pipelines: vec![vec![FilterInstance {
            id: Uuid::nil(),
            display_name: "block".into(),
            enabled: true,
            dry_run: false,
            on_inbound: true,
            on_outbound: false,
            filter: Arc::new(AlwaysBlock),
        }]],
    };
    bus.swap(next);

    // Second request: block-all pipeline → 403 without a listener restart.
    {
        let mut c = tokio::net::TcpStream::connect(bind).await.unwrap();
        c.write_all(b"GET / HTTP/1.1\r\nHost: y\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut out = Vec::new();
        let _ = c.read_to_end(&mut out).await;
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("HTTP/1.1 403"), "second request: {s:?}");
    }
}
