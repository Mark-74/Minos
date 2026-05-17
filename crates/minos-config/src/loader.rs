//! Save, load, and validate `Config` against a `FilterRegistry` and a `Storage`.

use crate::{Config, ConfigError, ServiceConfig};
use minos_core::{Filter, FilterInstance, FilterRegistry};
use minos_storage::Storage;
use std::collections::HashSet;
use std::sync::Arc;

/// The runtime form of a [`Config`]: every `FilterInstance` is built and ready
/// to dispatch. Held in an `Arc<ArcSwap<RuleSet>>` so the data plane sees
/// updates atomically without blocking on the control plane.
#[derive(Debug)]
pub struct RuleSet {
    /// The validated source config.
    pub source: Config,
    /// One built pipeline per service, in the same order as `source.services`.
    pub pipelines: Vec<Vec<FilterInstance>>,
}

impl RuleSet {
    /// Construct a `RuleSet` with no built pipelines, suitable as the initial
    /// value before any config has been loaded. The `source` is retained so
    /// the control plane can render diff views from version 1 onwards.
    #[must_use]
    pub fn empty_for(source: Config) -> Self {
        let pipelines = source.services.iter().map(|_| Vec::new()).collect();
        Self { source, pipelines }
    }

    /// Borrow the list of `ServiceConfig` from the underlying source.
    #[must_use]
    pub fn services(&self) -> &[ServiceConfig] {
        &self.source.services
    }

    /// Borrow the built pipeline for the service at index `service_idx`. The
    /// index must match the order of `services()`. Panics if out of bounds —
    /// the caller is expected to have looked up the index from `services()`.
    #[must_use]
    pub fn pipeline_for(&self, service_idx: usize) -> &[FilterInstance] {
        &self.pipelines[service_idx]
    }
}

/// Validate a `Config` against the registry. Returns a `RuleSet` ready to
/// hand to the data plane, or the first error encountered. Does not perform
/// any IO.
///
/// # Errors
///
/// Returns `ConfigError::Invalid` for structural problems (duplicate service
/// names, zero `max_body_bytes`, etc.) or `ConfigError::FilterBuild` if any
/// filter fails to build.
pub fn validate(cfg: &Config, registry: &FilterRegistry) -> Result<RuleSet, ConfigError> {
    // Structural invariants.
    let mut seen_names: HashSet<&str> = HashSet::new();
    for s in &cfg.services {
        if !seen_names.insert(&s.name) {
            return Err(ConfigError::Invalid(format!(
                "duplicate service name {:?}",
                s.name
            )));
        }
        if s.max_body_bytes == 0 {
            return Err(ConfigError::Invalid(format!(
                "service {:?}: max_body_bytes must be > 0",
                s.name
            )));
        }
    }

    // Build every filter via the registry.
    let mut pipelines = Vec::with_capacity(cfg.services.len());
    for service in &cfg.services {
        pipelines.push(build_service_pipeline(service, registry)?);
    }

    Ok(RuleSet {
        source: cfg.clone(),
        pipelines,
    })
}

fn build_service_pipeline(
    service: &ServiceConfig,
    registry: &FilterRegistry,
) -> Result<Vec<FilterInstance>, ConfigError> {
    let mut out = Vec::with_capacity(service.pipeline.len());
    for cfg in &service.pipeline {
        let filter: Arc<dyn Filter> =
            registry
                .build(&cfg.kind, cfg.config.clone())
                .map_err(|source| ConfigError::FilterBuild {
                    service: service.name.clone(),
                    filter: cfg.display_name.clone(),
                    source,
                })?;
        out.push(FilterInstance {
            id: cfg.id,
            display_name: cfg.display_name.clone(),
            enabled: cfg.enabled,
            dry_run: cfg.dry_run,
            on_inbound: cfg.on_inbound,
            on_outbound: cfg.on_outbound,
            filter,
        });
    }
    Ok(out)
}

/// Validate, persist, mark active, and (optionally) publish to a [`crate::Bus`].
///
/// On success returns the new version number. If a `bus` is provided, the
/// built [`RuleSet`] is also swapped into it atomically — that's how the
/// running daemon picks up new configs. Callers that don't need a bus
/// (CLI, batch tools) pass `None`.
///
/// # Errors
///
/// Returns the same errors as [`validate`], or `ConfigError::Storage` for IO
/// failures during the save/set-active steps. If validation fails, no version
/// row is written.
pub fn save_config<S: Storage>(
    storage: &S,
    registry: &FilterRegistry,
    cfg: &Config,
    note: Option<&str>,
    bus: Option<&crate::Bus>,
) -> Result<u64, ConfigError> {
    let built = validate(cfg, registry)?;
    let blob = serde_json::to_vec(cfg)?;
    let version = storage.save_version(&blob, note)?;
    storage.set_active_version(version)?;
    if let Some(bus) = bus {
        bus.swap(built);
    }
    Ok(version)
}

/// Load the currently active config from storage and rebuild it.
///
/// # Errors
///
/// Returns `ConfigError::Storage` (e.g. `NotFound` if no active version is
/// set), `ConfigError::Malformed` if the stored blob can't be deserialized,
/// or any error from [`validate`].
pub fn load_active_config<S: Storage>(
    storage: &S,
    registry: &FilterRegistry,
) -> Result<RuleSet, ConfigError> {
    let version = storage.active_version()?;
    let blob = storage.load_version(version)?;
    let cfg: Config = serde_json::from_slice(&blob)?;
    validate(&cfg, registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use minos_core::{BuildError, FilterKind, Packet, ProtocolKind, ProxyMode, Verdict};
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use std::net::SocketAddr;
    use uuid::Uuid;

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct PassCfg {}

    struct PassF;
    impl Filter for PassF {
        fn kind(&self) -> &'static str {
            "pass"
        }
        fn accepts(&self, _: &Packet) -> bool {
            true
        }
        fn inspect(&self, _: &Packet) -> Verdict {
            Verdict::Pass
        }
    }

    struct PassKind;
    impl FilterKind for PassKind {
        const NAME: &'static str = "pass";
        type Config = PassCfg;
        fn build(_: PassCfg) -> Result<Arc<dyn Filter>, BuildError> {
            Ok(Arc::new(PassF))
        }
    }

    fn sample_config() -> Config {
        let bind: SocketAddr = "0.0.0.0:80".parse().unwrap();
        let upstream: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        Config {
            services: vec![ServiceConfig {
                name: "web".into(),
                mode: ProxyMode::Reverse { bind, upstream },
                protocol: ProtocolKind::Http,
                pipeline: vec![crate::FilterInstanceCfg {
                    id: Uuid::new_v4(),
                    display_name: "passes".into(),
                    kind: "pass".into(),
                    config: serde_json::json!({}),
                    enabled: true,
                    dry_run: false,
                    on_inbound: true,
                    on_outbound: false,
                }],
                block_response_override: None,
                max_body_bytes: 65536,
            }],
            default_block_response: minos_core::BlockResponse::default(),
        }
    }

    #[test]
    fn validate_ok_for_well_formed_config() {
        let mut r = FilterRegistry::new();
        r.register::<PassKind>();
        let built = validate(&sample_config(), &r).unwrap();
        assert_eq!(built.pipelines.len(), 1);
        assert_eq!(built.pipelines[0].len(), 1);
        assert_eq!(built.pipelines[0][0].filter.kind(), "pass");
    }

    #[test]
    fn validate_rejects_unknown_filter_kind() {
        let r = FilterRegistry::new();
        let err = validate(&sample_config(), &r).err().unwrap();
        match err {
            ConfigError::FilterBuild {
                service, filter, ..
            } => {
                assert_eq!(service, "web");
                assert_eq!(filter, "passes");
            }
            other @ (ConfigError::Malformed(_)
            | ConfigError::Storage(_)
            | ConfigError::Invalid(_)) => {
                panic!("expected FilterBuild, got {other:?}");
            }
        }
    }

    #[test]
    fn validate_rejects_duplicate_service_names() {
        let mut r = FilterRegistry::new();
        r.register::<PassKind>();
        let mut cfg = sample_config();
        cfg.services.push(cfg.services[0].clone());
        let err = validate(&cfg, &r).err().unwrap();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn validate_rejects_zero_body_cap() {
        let mut r = FilterRegistry::new();
        r.register::<PassKind>();
        let mut cfg = sample_config();
        cfg.services[0].max_body_bytes = 0;
        let err = validate(&cfg, &r).err().unwrap();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }
}
