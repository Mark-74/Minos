//! End-to-end test exercising the public registry path: register a kind,
//! serialize a config, build via the registry, run the filter on packets.

use minos_core::{
    BuildError, Direction, Filter, FilterKind, FilterRegistry, Packet, ParsedHttp, Verdict,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize, JsonSchema)]
struct ContainsCfg {
    needle: String,
}

struct Contains {
    needle: Vec<u8>,
}

impl Filter for Contains {
    fn kind(&self) -> &'static str {
        "contains"
    }

    fn accepts(&self, _: &Packet) -> bool {
        true
    }

    fn inspect(&self, p: &Packet) -> Verdict {
        let bytes: Vec<u8> = match p {
            Packet::Raw { bytes, .. } => bytes.to_vec(),
            Packet::Http { req, .. } => req.body.clone(),
        };
        if bytes.windows(self.needle.len()).any(|w| w == self.needle) {
            Verdict::Block {
                reason: format!("contains {:?}", String::from_utf8_lossy(&self.needle)),
            }
        } else {
            Verdict::Pass
        }
    }
}

struct ContainsKind;

impl FilterKind for ContainsKind {
    const NAME: &'static str = "contains";
    type Config = ContainsCfg;

    fn build(cfg: ContainsCfg) -> Result<Arc<dyn Filter>, BuildError> {
        if cfg.needle.is_empty() {
            return Err(BuildError::Invalid {
                kind: "contains",
                message: "needle must not be empty".into(),
            });
        }
        Ok(Arc::new(Contains {
            needle: cfg.needle.into_bytes(),
        }))
    }
}

#[test]
fn full_round_trip_register_build_inspect() {
    let mut registry = FilterRegistry::new();
    registry.register::<ContainsKind>();

    // Serialize a config exactly the way the storage layer would produce.
    let stored = serde_json::json!({ "needle": "evil" });

    let filter = registry.build("contains", stored).unwrap();

    let raw_match = Packet::Raw {
        bytes: b"this is evil indeed",
        direction: Direction::Inbound,
    };
    let raw_clean = Packet::Raw {
        bytes: b"this is fine",
        direction: Direction::Inbound,
    };
    assert!(filter.inspect(&raw_match).is_block());
    assert!(filter.inspect(&raw_clean).is_pass());

    let req = ParsedHttp::new("POST", "/x", vec![], b"...evil...".to_vec());
    let http_match = Packet::Http {
        req: &req,
        direction: Direction::Inbound,
    };
    assert!(filter.inspect(&http_match).is_block());
}

#[test]
fn invalid_config_surfaces_kind_specific_error() {
    let mut registry = FilterRegistry::new();
    registry.register::<ContainsKind>();
    let err = registry
        .build("contains", serde_json::json!({ "needle": "" }))
        .err()
        .unwrap();
    match err {
        BuildError::Invalid { kind, message } => {
            assert_eq!(kind, "contains");
            assert!(message.contains("must not be empty"));
        }
        other @ BuildError::BadConfig { .. } => panic!("expected Invalid, got {other:?}"),
    }
}
