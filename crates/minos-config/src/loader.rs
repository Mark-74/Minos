//! Save, load, and validate `Config` against a `FilterRegistry` and a `Storage`.

use crate::{Config, ConfigError, ServiceConfig};
use minos_core::{Filter, FilterInstance, FilterRegistry};
use minos_storage::Storage;
use std::collections::HashSet;
use std::sync::Arc;

/// A `Config` whose filters have been built into runnable instances.
///
/// Phase 2 will add a higher-level `RuleSet` that owns a `BuiltConfig`
/// alongside per-service execution state. For Phase 1 this is the bridge
/// between the on-disk types and the runtime.
#[derive(Debug)]
pub struct BuiltConfig {
    /// The validated source config.
    pub source: Config,
    /// One built pipeline per service, in the same order as `source.services`.
    pub pipelines: Vec<Vec<FilterInstance>>,
}

/// Validate a `Config` against the registry. Returns a `BuiltConfig` ready to
/// hand to the data plane, or the first error encountered. Does not perform
/// any IO.
///
/// # Errors
///
/// Returns `ConfigError::Invalid` for structural problems (duplicate service
/// names, zero `max_body_bytes`, etc.) or `ConfigError::FilterBuild` if any
/// filter fails to build.
pub fn validate(cfg: &Config, registry: &FilterRegistry) -> Result<BuiltConfig, ConfigError> {
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

    Ok(BuiltConfig {
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

/// Validate, then persist, then mark active. The save flow from spec §6.3
/// abridged to what Phase 1 exposes: returns the new version number.
///
/// Note: the actual `ArcSwap<RuleSet>` swap happens in Phase 2 once the data
/// plane exists. For now, callers get back the `BuiltConfig` and the new
/// version, and decide whether to use either.
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
) -> Result<(BuiltConfig, u64), ConfigError> {
    let built = validate(cfg, registry)?;
    let blob = serde_json::to_vec(cfg)?;
    let version = storage.save_version(&blob, note)?;
    storage.set_active_version(version)?;
    Ok((built, version))
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
) -> Result<BuiltConfig, ConfigError> {
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
