//! End-to-end: a real HTTP client through Minos (Reverse mode) to a real
//! upstream. Verifies both the Pass and Block code paths against an
//! in-test mock filter.

use std::net::SocketAddr;
use std::sync::Arc;

use minos_config::{new_bus, Config, RuleSet, ServiceConfig};
use minos_core::{Filter, FilterInstance, Packet, ProtocolKind, ProxyMode, Verdict};
use minos_proxy::listen_service;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

struct BlockOnSubstring {
    needle: Vec<u8>,
}

impl Filter for BlockOnSubstring {
    fn kind(&self) -> &'static str {
        "test-substring"
    }

    fn accepts(&self, _: &Packet) -> bool {
        true
    }

    fn inspect(&self, p: &Packet) -> Verdict {
        let bytes: Vec<u8> = match p {
            Packet::Raw { bytes, .. } => bytes.to_vec(),
            Packet::Http { req, .. } => {
                let mut v = req.path.as_bytes().to_vec();
                v.extend_from_slice(&req.body);
                v
            }
        };
        if bytes.windows(self.needle.len()).any(|w| w == self.needle) {
            Verdict::Block {
                reason: "matched".into(),
            }
        } else {
            Verdict::Pass
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
                let _ = s.read(&mut buf).await.unwrap();
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

#[tokio::test(flavor = "multi_thread")]
async fn passes_clean_request_and_blocks_bad_one() {
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
    let ruleset = RuleSet {
        source: Config {
            services: vec![cfg.clone()],
            ..Config::default()
        },
        pipelines: vec![vec![FilterInstance {
            id: Uuid::nil(),
            display_name: "blocker".into(),
            enabled: true,
            dry_run: false,
            on_inbound: true,
            on_outbound: false,
            filter: Arc::new(BlockOnSubstring {
                needle: b"BAD".to_vec(),
            }),
        }]],
    };
    let (bus, _rx) = new_bus(ruleset);
    let _handle = listen_service(cfg, bus);

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    // Clean request: no needle in path → should be forwarded → 200.
    {
        let mut c = tokio::net::TcpStream::connect(bind).await.unwrap();
        c.write_all(b"GET /good HTTP/1.1\r\nHost: y\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut out = Vec::new();
        let _ = c.read_to_end(&mut out).await;
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("HTTP/1.1 200"), "clean request: got: {s:?}");
        assert!(s.ends_with("hello"), "clean request body: got: {s:?}");
    }

    // Bad request: needle in path → blocked → 403.
    {
        let mut c = tokio::net::TcpStream::connect(bind).await.unwrap();
        c.write_all(b"GET /BAD HTTP/1.1\r\nHost: y\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut out = Vec::new();
        let _ = c.read_to_end(&mut out).await;
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("HTTP/1.1 403"), "bad request: got: {s:?}");
    }
}
