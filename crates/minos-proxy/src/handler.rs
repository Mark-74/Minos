//! Per-protocol connection handlers.
//!
//! A handler owns one accepted client connection from accept-to-close. It is
//! responsible for: reading one or more logical units from the client,
//! running them through the pipeline executor, dispatching the upstream
//! request on Pass, and dispatching the configured block response on Block.

use std::net::SocketAddr;

use async_trait::async_trait;
use minos_config::Bus;
use minos_core::{BlockResponse, FilterInstance};
use tokio::net::TcpStream;

use crate::ProxyError;

/// Static per-service context handed to the handler on each accept. The
/// pipeline lives in the bus's `RuleSet`, accessed via `ArcSwap::load` and
/// indexed by service name.
#[derive(Debug, Clone)]
pub struct ServiceContext {
    /// Name of the service this handler is serving — used to locate the
    /// pipeline in the current `RuleSet` and to label log entries.
    pub service_name: String,
    /// Address to forward to after a Pass verdict. In `ProxyMode::Reverse`
    /// this is the configured upstream; in `ProxyMode::Transparent` the
    /// listener has already resolved `SO_ORIGINAL_DST` and put the result
    /// here.
    pub upstream: SocketAddr,
    /// Response to send on Block. Cloned per connection.
    pub block_response: BlockResponse,
    /// Maximum body bytes to inspect (per service).
    pub max_body_bytes: usize,
}

/// Per-protocol connection handler. Implementors handle one accepted client
/// connection end-to-end: read a logical unit, run the pipeline, forward or
/// block, then return.
///
/// Adding a new protocol (TLS termination, websockets, gRPC, ...) means
/// writing one more `impl ProtocolHandler` and dispatching to it from the
/// listener loop. The executor is unaffected.
#[async_trait]
pub trait ProtocolHandler: Send + Sync {
    /// Service one accepted connection. Errors are non-fatal at the listener
    /// level — the loop logs them and keeps accepting.
    async fn handle(
        &self,
        client: TcpStream,
        ctx: &ServiceContext,
        bus: &Bus,
    ) -> Result<(), ProxyError>;
}

/// Resolve the pipeline for `service_name` against the current `RuleSet`
/// snapshot. Returns a cloned `Vec<FilterInstance>` so the handler can call
/// it without holding the `ArcSwap` guard for the duration of the request
/// (which would prevent the next swap from being immediately visible).
///
/// Cloning is cheap: each `FilterInstance::filter` is `Arc<dyn Filter>`, and
/// the surrounding struct holds a few small `bool`s, a `Uuid`, and one
/// `String`. The whole pipeline is typically a handful of items.
pub(crate) fn pipeline_snapshot(bus: &Bus, service_name: &str) -> Option<Vec<FilterInstance>> {
    let guard = bus.rules.load();
    let idx = guard
        .services()
        .iter()
        .position(|s| s.name == service_name)?;
    Some(guard.pipeline_for(idx).to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use minos_config::{new_bus, Config, RuleSet, ServiceConfig};
    use minos_core::{ProtocolKind, ProxyMode};

    fn dummy_service(name: &str) -> ServiceConfig {
        ServiceConfig {
            name: name.into(),
            mode: ProxyMode::Reverse {
                bind: "127.0.0.1:0".parse().unwrap(),
                upstream: "127.0.0.1:1".parse().unwrap(),
            },
            protocol: ProtocolKind::Http,
            pipeline: vec![],
            block_response_override: None,
            max_body_bytes: 1024,
        }
    }

    #[test]
    fn pipeline_snapshot_returns_some_for_known_service() {
        let cfg = Config {
            services: vec![dummy_service("svc")],
            ..Config::default()
        };
        let ruleset = RuleSet::empty_for(cfg);
        let (bus, _rx) = new_bus(ruleset);
        let snap = pipeline_snapshot(&bus, "svc");
        assert!(snap.is_some());
        assert!(snap.unwrap().is_empty());
    }

    #[test]
    fn pipeline_snapshot_returns_none_for_unknown_service() {
        let cfg = Config::default();
        let ruleset = RuleSet::empty_for(cfg);
        let (bus, _rx) = new_bus(ruleset);
        assert!(pipeline_snapshot(&bus, "missing").is_none());
    }

    // Compile-only check that the trait is object-safe.
    #[allow(dead_code)]
    fn assert_dyn_object_safe(_: &dyn ProtocolHandler) {}
}
