//! `FilterKind`: the trait third-party crates implement to plug a new filter
//! type into the registry.

use crate::Filter;
use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;
use thiserror::Error;

/// Errors a filter kind can produce while building from its config blob.
#[derive(Debug, Error)]
pub enum BuildError {
    /// The config could not be deserialized into the kind's `Config` type.
    #[error("invalid config for filter kind {kind}: {source}")]
    BadConfig {
        /// The filter kind name (`FilterKind::NAME`) that rejected the config.
        kind: &'static str,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
    /// Domain-specific build failure (e.g. regex pattern won't compile).
    #[error("filter kind {kind} rejected config: {message}")]
    Invalid {
        /// The filter kind name.
        kind: &'static str,
        /// Human-readable message suitable for the UI.
        message: String,
    },
}

/// A pluggable filter type. Implementors define the config schema and how to
/// turn a deserialized config into a runnable [`Filter`].
///
/// The same trait is the registration boundary for third-party filter crates:
/// such a crate depends only on `minos-core`, defines a `FilterKind` impl,
/// and the host binary calls `FilterRegistry::register::<MyKind>()`.
pub trait FilterKind: 'static {
    /// Stable identifier persisted in configs. Must match the value returned
    /// by `Filter::kind()` of the filter this kind builds.
    const NAME: &'static str;

    /// The deserializable config type for this kind. Must be JSON-serializable
    /// (for storage) and JSON-Schema-derivable (for auto-rendered UI forms).
    type Config: Serialize + DeserializeOwned + JsonSchema;

    /// Construct the runnable filter from a validated config.
    ///
    /// # Errors
    ///
    /// Returns [`BuildError::BadConfig`] if `cfg` cannot be validated, or
    /// [`BuildError::Invalid`] if the kind rejects the config for domain-
    /// specific reasons (e.g. a regex pattern that won't compile).
    fn build(cfg: Self::Config) -> Result<Arc<dyn Filter>, BuildError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Packet, Verdict};
    use schemars::schema_for;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct DummyCfg {
        verdict_pass: bool,
    }

    struct Dummy {
        verdict_pass: bool,
    }
    impl Filter for Dummy {
        fn kind(&self) -> &'static str {
            "dummy"
        }
        fn accepts(&self, _: &Packet) -> bool {
            true
        }
        fn inspect(&self, _: &Packet) -> Verdict {
            if self.verdict_pass {
                Verdict::Pass
            } else {
                Verdict::Block {
                    reason: "dummy".into(),
                }
            }
        }
    }

    struct DummyKind;
    impl FilterKind for DummyKind {
        const NAME: &'static str = "dummy";
        type Config = DummyCfg;
        fn build(cfg: DummyCfg) -> Result<Arc<dyn Filter>, BuildError> {
            Ok(Arc::new(Dummy {
                verdict_pass: cfg.verdict_pass,
            }))
        }
    }

    #[test]
    fn build_returns_runnable_filter() {
        let f = DummyKind::build(DummyCfg { verdict_pass: true }).unwrap();
        assert_eq!(f.kind(), "dummy");
        let pkt = Packet::Raw {
            bytes: b"x",
            direction: crate::Direction::Inbound,
        };
        assert_eq!(f.inspect(&pkt), Verdict::Pass);
    }

    #[test]
    fn json_schema_derives() {
        let _schema = schema_for!(DummyCfg);
        // We just want to confirm the bound holds at compile + runtime; schema
        // shape is exercised in registry tests.
    }

    #[test]
    fn build_error_messages_include_kind() {
        let err = BuildError::Invalid {
            kind: "dummy",
            message: "bad".into(),
        };
        assert!(err.to_string().contains("dummy"));
        assert!(err.to_string().contains("bad"));
    }
}
