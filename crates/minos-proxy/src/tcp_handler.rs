//! Raw TCP protocol handler.
//!
//! Reads one "burst" from the client (until N bytes or M ms idle), runs it
//! through the pipeline as a [`minos_core::Packet::Raw`] inbound packet,
//! then either closes the connection (Block) or connects to the upstream,
//! replays the burst, and bridges traffic in both directions until close.

use std::time::Duration;

use minos_core::{Direction, Packet, Verdict};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::{ProxyError, ServiceContext};

/// Raw TCP handler. `idle_ms` is the per-burst inactivity timeout: once a
/// connection has been idle for that long, the accumulated bytes are
/// considered "one burst" and run through the pipeline.
#[derive(Debug, Clone, Copy)]
pub struct TcpHandler {
    /// Idle timeout in milliseconds. 50 ms is a sensible default for the
    /// CTF use case (services answer fast or not at all).
    pub idle_ms: u64,
}

impl TcpHandler {
    /// Construct a `TcpHandler` with the given idle timeout.
    #[must_use]
    pub fn new(idle_ms: u64) -> Self {
        Self { idle_ms }
    }

    /// Read bytes from `stream` until EOF, the burst cap (`cap`), or an
    /// idle period of `self.idle_ms` ms with no new data.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::Io`] on socket failures.
    async fn read_burst(&self, stream: &mut TcpStream, cap: usize) -> Result<Vec<u8>, ProxyError> {
        let mut buf = Vec::with_capacity(4096);
        loop {
            let mut tmp = [0u8; 4096];
            let fut = stream.read(&mut tmp);
            match tokio::time::timeout(Duration::from_millis(self.idle_ms), fut).await {
                // EOF, or idle timeout — either way, end of burst.
                Ok(Ok(0)) | Err(_) => break,
                Ok(Ok(n)) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.len() >= cap {
                        break;
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
            }
        }
        Ok(buf)
    }
}

#[async_trait::async_trait]
impl crate::ProtocolHandler for TcpHandler {
    async fn handle(
        &self,
        mut client: TcpStream,
        ctx: &ServiceContext,
        bus: &minos_config::Bus,
    ) -> Result<(), ProxyError> {
        let burst = self.read_burst(&mut client, ctx.max_body_bytes).await?;

        let packet = Packet::Raw {
            bytes: &burst,
            direction: Direction::Inbound,
        };
        let pipeline =
            crate::handler::pipeline_snapshot(bus, &ctx.service_name).unwrap_or_default();
        let verdict = crate::execute(&packet, &ctx.service_name, &pipeline, &bus.log);

        if let Verdict::Block { .. } = verdict {
            // For TCP, Block always closes — the BlockResponse::HttpStatus
            // is ignored because there's no HTTP semantics here.
            let _ = client.shutdown().await;
            return Ok(());
        }

        // Connect to upstream, replay the burst, bridge the rest.
        let mut upstream = TcpStream::connect(ctx.upstream)
            .await
            .map_err(|_| ProxyError::UpstreamConnect(ctx.upstream))?;
        upstream.write_all(&burst).await?;
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::ProtocolHandler;
    use minos_config::{new_bus, Config, RuleSet, ServiceConfig};
    use minos_core::{
        Filter, FilterInstance, Packet as MCPacket, ProtocolKind, ProxyMode, Verdict,
    };
    use uuid::Uuid;

    struct BlockOnNeedle {
        needle: Vec<u8>,
    }

    impl Filter for BlockOnNeedle {
        fn kind(&self) -> &'static str {
            "block-on-needle"
        }
        fn accepts(&self, _: &MCPacket) -> bool {
            true
        }
        fn inspect(&self, p: &MCPacket) -> Verdict {
            let bytes: Vec<u8> = match p {
                MCPacket::Raw { bytes, .. } => bytes.to_vec(),
                MCPacket::Http { .. } => return Verdict::Pass,
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

    #[tokio::test(flavor = "multi_thread")]
    async fn tcp_handler_blocks_matching_burst_without_contacting_upstream() {
        // Upstream that would echo, but should NEVER be contacted if blocked.
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_contacted = Arc::new(AtomicBool::new(false));
        let flag = upstream_contacted.clone();
        tokio::spawn(async move {
            if upstream.accept().await.is_ok() {
                flag.store(true, Ordering::SeqCst);
            }
        });

        // Build a bus with one service running the BlockOnNeedle filter.
        let cfg = Config {
            services: vec![ServiceConfig {
                name: "svc".into(),
                mode: ProxyMode::Reverse {
                    bind: "127.0.0.1:0".parse().unwrap(),
                    upstream: upstream_addr,
                },
                protocol: ProtocolKind::Tcp,
                pipeline: vec![],
                block_response_override: None,
                max_body_bytes: 1024,
            }],
            ..Config::default()
        };
        let ruleset = RuleSet {
            source: cfg,
            pipelines: vec![vec![FilterInstance {
                id: Uuid::nil(),
                display_name: "block".into(),
                enabled: true,
                dry_run: false,
                on_inbound: true,
                on_outbound: false,
                filter: Arc::new(BlockOnNeedle {
                    needle: b"DROP".to_vec(),
                }),
            }]],
        };
        let (bus, _rx) = new_bus(ruleset);

        // Spawn the proxy listener.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = listener.local_addr().unwrap();
        let bus2 = bus.clone();
        tokio::spawn(async move {
            let (client, _) = listener.accept().await.unwrap();
            let ctx = ServiceContext {
                service_name: "svc".into(),
                upstream: upstream_addr,
                block_response: minos_core::BlockResponse::Close,
                max_body_bytes: 1024,
            };
            TcpHandler::new(50)
                .handle(client, &ctx, &bus2)
                .await
                .unwrap();
        });

        // Send a bad burst.
        let mut client = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
        client.write_all(b"DROP TABLE users;").await.unwrap();
        drop(client);

        // Give the handler + upstream-accept-stub time to converge.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !upstream_contacted.load(Ordering::SeqCst),
            "upstream must not have been contacted"
        );
    }
}
