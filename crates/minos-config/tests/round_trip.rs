//! End-to-end: save a Config through `save_config`, reopen storage, load it
//! back through `load_active_config`, assert structural equality of the
//! source `Config` and that the rebuilt pipelines match.

use minos_config::{load_active_config, save_config, Config, FilterInstanceCfg, ServiceConfig};
use minos_core::{
    BlockResponse, BuildError, Direction, Filter, FilterKind, FilterRegistry, Packet, ProtocolKind,
    ProxyMode, Verdict,
};
use minos_storage::{SqliteStorage, Storage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tempfile::tempdir;
use uuid::Uuid;

#[derive(Serialize, Deserialize, JsonSchema)]
struct EchoCfg {
    tag: String,
}

struct Echo {
    tag: String,
}
impl Filter for Echo {
    fn kind(&self) -> &'static str {
        "echo"
    }
    fn accepts(&self, _: &Packet<'_>) -> bool {
        true
    }
    fn inspect(&self, _: &Packet<'_>) -> Verdict {
        Verdict::Block {
            reason: format!("echo:{}", self.tag),
        }
    }
}

struct EchoKind;
impl FilterKind for EchoKind {
    const NAME: &'static str = "echo";
    type Config = EchoCfg;
    fn build(cfg: EchoCfg) -> Result<Arc<dyn Filter>, BuildError> {
        Ok(Arc::new(Echo { tag: cfg.tag }))
    }
}

fn sample_config() -> Config {
    let bind: SocketAddr = "0.0.0.0:80".parse().unwrap();
    let upstream: SocketAddr = "127.0.0.1:5000".parse().unwrap();
    Config {
        services: vec![ServiceConfig {
            name: "web-api".into(),
            mode: ProxyMode::Reverse { bind, upstream },
            protocol: ProtocolKind::Http,
            pipeline: vec![FilterInstanceCfg {
                id: Uuid::new_v4(),
                display_name: "echo-block".into(),
                kind: "echo".into(),
                config: serde_json::json!({ "tag": "first" }),
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
fn end_to_end_save_load_round_trip() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("minos.db");

    let cfg = sample_config();

    let mut registry = FilterRegistry::new();
    registry.register::<EchoKind>();

    // Phase 1: save through the full save_config path.
    {
        let storage = SqliteStorage::open(&db).unwrap();
        let (_built, version) = save_config(&storage, &registry, &cfg, Some("first save")).unwrap();
        assert_eq!(version, 1);
        assert_eq!(storage.active_version().unwrap(), 1);
    }

    // Phase 2: reopen, load, assert source equality and runnable pipelines.
    {
        let storage = SqliteStorage::open(&db).unwrap();
        let built = load_active_config(&storage, &registry).unwrap();
        assert_eq!(built.source, cfg);
        assert_eq!(built.pipelines.len(), 1);
        assert_eq!(built.pipelines[0].len(), 1);
        let inst = &built.pipelines[0][0];
        assert_eq!(inst.filter.kind(), "echo");
        let pkt = Packet::Raw {
            bytes: b"x",
            direction: Direction::Inbound,
        };
        let v = inst.filter.inspect(&pkt);
        assert!(matches!(v, Verdict::Block { reason } if reason == "echo:first"));
    }
}

#[test]
fn save_with_unknown_kind_does_not_persist() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("minos.db");

    let cfg = sample_config();
    // Empty registry → "echo" kind is unknown.
    let registry = FilterRegistry::new();

    let storage = SqliteStorage::open(&db).unwrap();
    let err = save_config(&storage, &registry, &cfg, None).err().unwrap();
    assert!(matches!(err, minos_config::ConfigError::FilterBuild { .. }));

    // Active version should still be unset (no row written).
    assert!(storage.active_version().is_err());
}
