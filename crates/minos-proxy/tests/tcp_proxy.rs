//! End-to-end: a real TCP client through Minos (Reverse mode) to a real
//! upstream. Verifies both the Pass and Block code paths for raw TCP against
//! an in-test mock filter.

use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

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
        "test-substring-tcp"
    }

    fn accepts(&self, _: &Packet) -> bool {
        true
    }

    fn inspect(&self, p: &Packet) -> Verdict {
        let bytes: Vec<u8> = match p {
            Packet::Raw { bytes, .. } => bytes.to_vec(),
            Packet::Http { .. } => return Verdict::Pass,
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

async fn spawn_upstream(flag: Arc<AtomicBool>) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            flag.store(true, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                if let Ok(n) = s.read(&mut buf).await {
                    let _ = s.write_all(&buf[..n]).await;
                }
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

fn make_cfg(bind: SocketAddr, upstream: SocketAddr) -> ServiceConfig {
    ServiceConfig {
        name: "svc-tcp".into(),
        mode: ProxyMode::Reverse { bind, upstream },
        protocol: ProtocolKind::Tcp,
        pipeline: vec![],
        block_response_override: None,
        max_body_bytes: 1024,
    }
}

fn make_ruleset(cfg: ServiceConfig, needle: &'static [u8]) -> RuleSet {
    RuleSet {
        source: Config {
            services: vec![cfg],
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
                needle: needle.to_vec(),
            }),
        }]],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tcp_passes_clean_burst_to_upstream() {
    let flag = Arc::new(AtomicBool::new(false));
    let upstream = spawn_upstream(flag.clone()).await;
    let bind = pick_free_port();

    let cfg = make_cfg(bind, upstream);
    let ruleset = make_ruleset(cfg.clone(), b"DROP");
    let (bus, _rx) = new_bus(ruleset);
    let _handle = listen_service(cfg, bus);

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    // Send a clean burst (no needle) — should be echoed back by upstream.
    let mut c = tokio::net::TcpStream::connect(bind).await.unwrap();
    c.write_all(b"hello\n").await.unwrap();
    // Give the burst-timeout (50 ms idle) plus slack for the handler to open
    // the upstream connection and relay the echo.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let mut out = vec![0u8; 64];
    let n = c.read(&mut out).await.unwrap();
    let echoed = &out[..n];
    assert_eq!(echoed, b"hello\n", "expected echo, got: {echoed:?}");
    assert!(
        flag.load(Ordering::SeqCst),
        "upstream should have been contacted"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tcp_blocks_matching_burst_without_contacting_upstream() {
    let flag = Arc::new(AtomicBool::new(false));
    let upstream = spawn_upstream(flag.clone()).await;
    let bind = pick_free_port();

    let cfg = make_cfg(bind, upstream);
    let ruleset = make_ruleset(cfg.clone(), b"DROP");
    let (bus, _rx) = new_bus(ruleset);
    let _handle = listen_service(cfg, bus);

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    // Send a bad burst — handler should close the connection without touching
    // upstream.
    let mut c = tokio::net::TcpStream::connect(bind).await.unwrap();
    c.write_all(b"DROP TABLE users;").await.unwrap();
    drop(c);

    // Give the handler + upstream-accept-stub time to converge.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(
        !flag.load(Ordering::SeqCst),
        "upstream must not have been contacted"
    );
}
