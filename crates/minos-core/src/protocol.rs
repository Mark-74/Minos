//! Per-service protocol kind and deployment mode.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// Application-layer protocol Minos should treat the service as speaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolKind {
    /// HTTP/1.1, parsed by Minos before being handed to the filter pipeline.
    Http,
    /// Raw TCP. The filter pipeline sees a buffered burst of bytes.
    Tcp,
}

/// How Minos receives traffic for a given service.
///
/// Selecting a mode changes only the `accept` path; everything downstream
/// (filter pipeline, logging, sidecar supervision) is mode-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProxyMode {
    /// Minos owns the public port. The real service is moved to `upstream`.
    Reverse {
        /// Public address Minos listens on.
        bind: SocketAddr,
        /// Internal address Minos forwards to.
        upstream: SocketAddr,
    },
    /// Minos is reached via `iptables -j REDIRECT --to-port <bind>`. The real
    /// destination port is recovered per-connection via `SO_ORIGINAL_DST`.
    Transparent {
        /// Address Minos listens on after iptables has redirected.
        bind: SocketAddr,
    },
}

impl ProxyMode {
    /// The address Minos's listener binds, regardless of mode.
    #[must_use]
    pub fn bind(&self) -> SocketAddr {
        match self {
            ProxyMode::Reverse { bind, .. } | ProxyMode::Transparent { bind } => *bind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_mode_bind_returns_correct_addr() {
        let bind: SocketAddr = "127.0.0.1:80".parse().unwrap();
        let upstream: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let reverse = ProxyMode::Reverse { bind, upstream };
        let transparent = ProxyMode::Transparent { bind };
        assert_eq!(reverse.bind(), bind);
        assert_eq!(transparent.bind(), bind);
    }

    #[test]
    fn proxy_mode_round_trips_through_serde() {
        let bind: SocketAddr = "0.0.0.0:9999".parse().unwrap();
        let upstream: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        for m in [
            ProxyMode::Reverse { bind, upstream },
            ProxyMode::Transparent { bind },
        ] {
            let json = serde_json::to_string(&m).unwrap();
            let back: ProxyMode = serde_json::from_str(&json).unwrap();
            assert_eq!(m, back);
        }
    }

    #[test]
    fn protocol_kind_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&ProtocolKind::Http).unwrap(),
            "\"http\""
        );
        assert_eq!(
            serde_json::to_string(&ProtocolKind::Tcp).unwrap(),
            "\"tcp\""
        );
    }
}
