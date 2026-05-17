//! Per-service listener loop. One `tokio::task::JoinHandle` per service;
//! dropping the handle cancels the accept loop.

use std::net::SocketAddr;
use std::sync::Arc;

use minos_config::{Bus, ServiceConfig};
use minos_core::{ProtocolKind, ProxyMode};
use tokio::net::{TcpListener, TcpStream};

use crate::{HttpHandler, ProtocolHandler, ProxyError, ServiceContext, TcpHandler};

/// Default per-burst idle timeout (in ms) for [`TcpHandler`]. The CTF use
/// case is fine with a short window — services answer fast or not at all.
const DEFAULT_TCP_IDLE_MS: u64 = 50;

/// Start the data-plane listener for one service. Returns the join handle
/// for the background accept loop; dropping the handle cancels the loop.
///
/// The bind address is whichever `cfg.mode.bind()` reports; the upstream is
/// either the configured static upstream (Reverse mode) or resolved per
/// connection via `SO_ORIGINAL_DST` (Transparent mode). Per-connection
/// failures are logged via `tracing` and never break the accept loop.
#[must_use]
pub fn listen_service(cfg: ServiceConfig, bus: Bus) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let bind = cfg.mode.bind();
        let listener = match TcpListener::bind(bind).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(service = %cfg.name, addr = %bind, error = %e, "bind failed");
                return;
            }
        };
        tracing::info!(service = %cfg.name, addr = %bind, "listener up");

        let handler: Arc<dyn ProtocolHandler> = match cfg.protocol {
            ProtocolKind::Http => Arc::new(HttpHandler),
            ProtocolKind::Tcp => Arc::new(TcpHandler::new(DEFAULT_TCP_IDLE_MS)),
        };

        loop {
            let (client, _peer) = match listener.accept().await {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!(service = %cfg.name, error = %e, "accept failed");
                    continue;
                }
            };
            let upstream = match resolve_upstream(&cfg.mode, &client) {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(service = %cfg.name, error = ?e, "upstream resolve failed");
                    continue;
                }
            };
            let ctx = ServiceContext {
                service_name: cfg.name.clone(),
                upstream,
                block_response: cfg.block_response_override.clone().unwrap_or_default(),
                max_body_bytes: cfg.max_body_bytes,
            };
            let bus = bus.clone();
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                if let Err(e) = handler.handle(client, &ctx, &bus).await {
                    tracing::debug!(error = ?e, "connection ended with error");
                }
            });
        }
    })
}

/// Resolve the upstream address for a new connection. In `Reverse` mode the
/// upstream is a static configured address; in `Transparent` mode the kernel
/// tells us the original destination via `SO_ORIGINAL_DST`.
fn resolve_upstream(mode: &ProxyMode, client: &TcpStream) -> Result<SocketAddr, ProxyError> {
    match mode {
        ProxyMode::Reverse { upstream, .. } => Ok(*upstream),
        ProxyMode::Transparent { .. } => crate::original_dst(client),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minos_config::{new_bus, Config, RuleSet};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn pick_free_port() -> SocketAddr {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        addr
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn listen_service_serves_one_request_end_to_end() {
        // Fixture upstream.
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = upstream.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = s.read(&mut buf).await.unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
            let _ = s.shutdown().await;
        });

        let bind = pick_free_port();
        let cfg = ServiceConfig {
            name: "svc".into(),
            mode: ProxyMode::Reverse {
                bind,
                upstream: upstream_addr,
            },
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
            pipelines: vec![vec![]],
        };
        let (bus, _rx) = new_bus(ruleset);
        let _handle = listen_service(cfg, bus);

        // Give the listener a moment to bind.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut client = tokio::net::TcpStream::connect(bind).await.unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: y\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut out = Vec::new();
        let _ = client.read_to_end(&mut out).await;
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 200"), "got: {s:?}");
        assert!(s.ends_with("ok"), "got: {s:?}");
    }
}
