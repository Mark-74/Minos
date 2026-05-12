//! Type-erased registry mapping filter-kind names to their builders and schemas.

use crate::{BuildError, Filter, FilterKind};
use schemars::{schema_for, Schema};
use std::collections::HashMap;
use std::sync::Arc;

type Builder = Box<dyn Fn(serde_json::Value) -> Result<Arc<dyn Filter>, BuildError> + Send + Sync>;

struct Entry {
    builder: Builder,
    schema: Schema,
}

/// Registry of all known filter kinds. The control plane consults it to
/// validate, build, and render UI forms for filter configs.
#[derive(Default)]
pub struct FilterRegistry {
    entries: HashMap<&'static str, Entry>,
}

impl FilterRegistry {
    /// Create an empty registry. Use [`Self::register`] to add kinds.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a filter kind by type. Re-registering the same name overwrites
    /// the previous entry (this is intentional so tests can swap impls).
    pub fn register<K: FilterKind>(&mut self) {
        let builder: Builder = Box::new(|value| {
            let cfg: K::Config =
                serde_json::from_value(value).map_err(|source| BuildError::BadConfig {
                    kind: K::NAME,
                    source,
                })?;
            K::build(cfg)
        });
        let schema = schema_for!(K::Config);
        self.entries.insert(K::NAME, Entry { builder, schema });
    }

    /// Names of all registered kinds, in unspecified order.
    #[must_use]
    pub fn known_kinds(&self) -> Vec<&'static str> {
        self.entries.keys().copied().collect()
    }

    /// Build a filter from a kind name and a serialized config blob.
    ///
    /// # Errors
    ///
    /// Returns `BuildError::Invalid` if `kind` is not registered, or whatever
    /// the kind's `build` method returned (typically `BuildError::BadConfig`
    /// for deserialization failures or `BuildError::Invalid` for domain errors).
    pub fn build(
        &self,
        kind: &str,
        config: serde_json::Value,
    ) -> Result<Arc<dyn Filter>, BuildError> {
        let entry = self.entries.get(kind).ok_or_else(|| BuildError::Invalid {
            // The kind name is not in our registered set, so we can't return it
            // as the `kind: &'static str` field. Use a fixed sentinel and put
            // the user-supplied name in the message instead.
            kind: "unknown",
            message: format!("no filter kind registered with name {kind}"),
        })?;
        (entry.builder)(config)
    }

    /// JSON Schema for the given kind's config, suitable for auto-rendering
    /// an HTML form in the UI.
    #[must_use]
    pub fn schema(&self, kind: &str) -> Option<&Schema> {
        self.entries.get(kind).map(|e| &e.schema)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Packet, Verdict};
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct EchoCfg {
        verdict: String,
    }

    struct Echo {
        verdict: String,
    }
    impl Filter for Echo {
        fn kind(&self) -> &'static str {
            "echo"
        }
        fn accepts(&self, _: &Packet) -> bool {
            true
        }
        fn inspect(&self, _: &Packet) -> Verdict {
            if self.verdict == "pass" {
                Verdict::Pass
            } else {
                Verdict::Block {
                    reason: self.verdict.clone(),
                }
            }
        }
    }

    struct EchoKind;
    impl FilterKind for EchoKind {
        const NAME: &'static str = "echo";
        type Config = EchoCfg;
        fn build(cfg: EchoCfg) -> Result<Arc<dyn Filter>, BuildError> {
            Ok(Arc::new(Echo {
                verdict: cfg.verdict,
            }))
        }
    }

    #[test]
    fn known_kinds_lists_registered() {
        let mut r = FilterRegistry::new();
        r.register::<EchoKind>();
        let kinds = r.known_kinds();
        assert_eq!(kinds, vec!["echo"]);
    }

    #[test]
    fn build_constructs_runnable_filter() {
        let mut r = FilterRegistry::new();
        r.register::<EchoKind>();
        let f = r
            .build("echo", serde_json::json!({ "verdict": "pass" }))
            .unwrap();
        let pkt = Packet::Raw {
            bytes: b"x",
            direction: crate::Direction::Inbound,
        };
        assert_eq!(f.inspect(&pkt), Verdict::Pass);
    }

    #[test]
    fn build_unknown_kind_errors() {
        let r = FilterRegistry::new();
        let err = r.build("nope", serde_json::json!({})).err().unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("nope"),
            "error should mention the missing kind, got: {msg}"
        );
    }

    #[test]
    fn build_with_bad_config_errors_with_kind() {
        let mut r = FilterRegistry::new();
        r.register::<EchoKind>();
        // EchoCfg requires `verdict` (a string); pass an int instead.
        let err = r
            .build("echo", serde_json::json!({ "verdict": 42 }))
            .err()
            .unwrap();
        match err {
            BuildError::BadConfig { kind, .. } => assert_eq!(kind, "echo"),
            other @ BuildError::Invalid { .. } => panic!("expected BadConfig, got {other:?}"),
        }
    }

    #[test]
    fn schema_returns_some_for_known_kind() {
        let mut r = FilterRegistry::new();
        r.register::<EchoKind>();
        assert!(r.schema("echo").is_some());
        assert!(r.schema("missing").is_none());
    }
}
