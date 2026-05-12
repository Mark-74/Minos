//! On-disk configuration types. Together they form the source of truth that
//! gets serialized into a single blob per version.

use minos_core::{BlockResponse, ProtocolKind, ProxyMode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Top-level Minos config. One instance is in effect at any moment; the
/// version history holds previous instances.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// All defended services, in the order they appear in the UI.
    pub services: Vec<ServiceConfig>,
    /// Default block response when a service doesn't override.
    #[serde(default)]
    pub default_block_response: BlockResponse,
}

/// One defended service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// Operator-supplied display name (used in the UI and in log entries).
    /// Must be unique within the `Config`.
    pub name: String,
    /// How Minos receives traffic for this service.
    pub mode: ProxyMode,
    /// Application-layer protocol Minos should treat the service as speaking.
    pub protocol: ProtocolKind,
    /// Filter pipeline, run in order; first `Block` wins.
    pub pipeline: Vec<FilterInstanceCfg>,
    /// Per-service override of the block response.
    #[serde(default)]
    pub block_response_override: Option<BlockResponse>,
    /// Cap on the inspected payload size per logical unit.
    pub max_body_bytes: usize,
}

/// Per-instance filter configuration. The `kind` field selects which
/// `FilterKind` will deserialize `config` and build the runnable filter.
// Four independent boolean toggles (enabled, dry_run, on_inbound, on_outbound)
// are the right representation here: each is orthogonal and maps directly to a
// UI checkbox. A state-machine refactor would obscure intent without benefit.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterInstanceCfg {
    /// Stable id for this instance.
    pub id: Uuid,
    /// Operator-supplied display name.
    pub display_name: String,
    /// `FilterKind::NAME` of the kind that built this filter.
    pub kind: String,
    /// Opaque per-kind config blob.
    pub config: serde_json::Value,
    /// Whether the executor should run this instance at all.
    pub enabled: bool,
    /// Whether matches should be logged-only instead of blocking.
    pub dry_run: bool,
    /// Inspect inbound packets.
    pub on_inbound: bool,
    /// Inspect outbound packets.
    pub on_outbound: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn sample() -> Config {
        let bind: SocketAddr = "0.0.0.0:80".parse().unwrap();
        let upstream: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        Config {
            services: vec![ServiceConfig {
                name: "web-api".into(),
                mode: ProxyMode::Reverse { bind, upstream },
                protocol: ProtocolKind::Http,
                pipeline: vec![FilterInstanceCfg {
                    id: Uuid::nil(),
                    display_name: "block evil".into(),
                    kind: "regex".into(),
                    config: serde_json::json!({ "pattern": "evil" }),
                    enabled: true,
                    dry_run: false,
                    on_inbound: true,
                    on_outbound: false,
                }],
                block_response_override: None,
                max_body_bytes: 65536,
            }],
            default_block_response: BlockResponse::default(),
        }
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = sample();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn default_config_is_empty() {
        let cfg = Config::default();
        assert!(cfg.services.is_empty());
        assert_eq!(cfg.default_block_response, BlockResponse::default());
    }
}
