//! Minos binary wiring: assemble the data plane (proxy listeners) and the
//! control plane (web UI) over the two established seams — the bus's
//! `ArcSwap<RuleSet>` for hot-reload and its log mpsc, now fanned out to a
//! broadcast channel for the live UI feed.
//!
//! [`run`] is the whole program minus argument parsing, kept here (not in
//! `main.rs`) so it is exercisable from an integration test.
#![deny(missing_docs)]

use std::sync::Arc;

use anyhow::Context;
use axum_extra::extract::cookie::Key;
use minos_config::{load_active_config, new_bus, save_config, Config, RuleSet};
use minos_core::FilterRegistry;
use minos_filters::register_builtin_filters;
use minos_proxy::{listen_service, spawn_log_writer};
use minos_storage::{SqliteStorage, Storage};
use minos_web::AppState;

/// Runtime configuration, sourced from the environment with sane defaults.
///
/// The sidecar paths (`MINOS_VENV_ROOT`, `MINOS_SOCKET_DIR`) are read directly
/// by `minos-filters` when a `python_sidecar` filter is built; they are listed
/// here only for documentation.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// `SQLite` database path (`MINOS_DB`, default `minos.db`).
    pub db_path: String,
    /// Web UI bind address (`MINOS_WEB_BIND`, default `0.0.0.0:8080`).
    pub web_bind: String,
    /// Optional path to a JSON config used to **seed services on first run**
    /// (`MINOS_CONFIG`). The web UI can create filters but not services, so
    /// this file is how an operator declares the services to defend. It is
    /// applied only when the database has no active config yet; thereafter the
    /// database (and the UI's edit/rollback history) is authoritative.
    pub config_path: Option<String>,
}

impl AppConfig {
    /// Read configuration from the environment, applying defaults.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            db_path: std::env::var("MINOS_DB").unwrap_or_else(|_| "minos.db".into()),
            web_bind: std::env::var("MINOS_WEB_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            config_path: std::env::var("MINOS_CONFIG").ok().filter(|s| !s.is_empty()),
        }
    }
}

/// Resolve the initial [`RuleSet`]: use the active config from storage if one
/// exists; otherwise seed from `config_path` (a JSON [`Config`]) when given, or
/// fall back to an empty config. The seeded/bootstrapped config is saved as the
/// first version so it appears in history and the UI has something to edit.
///
/// # Errors
///
/// Returns an error if the seed file can't be read or parsed, or if saving /
/// validating the resulting config fails.
fn load_or_seed(
    storage: &dyn Storage,
    registry: &FilterRegistry,
    config_path: Option<&str>,
) -> anyhow::Result<RuleSet> {
    if let Ok(rs) = load_active_config(storage, registry) {
        return Ok(rs);
    }

    let (cfg, note) = if let Some(path) = config_path {
        tracing::info!(path, "seeding initial config from file");
        let bytes = std::fs::read(path).with_context(|| format!("reading seed config {path}"))?;
        let cfg: Config = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing seed config {path} as JSON"))?;
        (cfg, "seed from MINOS_CONFIG")
    } else {
        tracing::warn!("no active config and no MINOS_CONFIG; bootstrapping an empty one");
        (Config::default(), "bootstrap")
    };

    // save_config validates the config (regex compiles, scripts parse, venvs
    // install) before persisting; on success it returns the new version and we
    // re-read it as a built RuleSet.
    save_config(storage, registry, &cfg, Some(note), None).context("saving initial config")?;
    load_active_config(storage, registry).context("loading the freshly saved config")
}

/// Assemble and run Minos until the web server stops (or, with the signal
/// handling added by `main`, until ctrl-c).
///
/// # Errors
///
/// Returns an error if the database can't be opened, the bootstrap save
/// fails, the web bind address is unavailable, or the web server errors.
pub async fn run(cfg: AppConfig) -> anyhow::Result<()> {
    let storage = Arc::new(
        SqliteStorage::open(&cfg.db_path)
            .with_context(|| format!("opening SQLite database at {}", cfg.db_path))?,
    );

    let mut registry = FilterRegistry::new();
    register_builtin_filters(&mut registry);

    // Load the active config, seeding from MINOS_CONFIG (or empty) on first run.
    let ruleset = load_or_seed(storage.as_ref(), &registry, cfg.config_path.as_deref())?;

    // Services to bind listeners for (captured before the ruleset moves).
    let services = ruleset.source.services.clone();

    let (bus, log_rx) = new_bus(ruleset);

    // Broadcast fan-out for the live UI feed; the log writer is the producer.
    let (log_tx, _) = tokio::sync::broadcast::channel(1024);
    let _writer = spawn_log_writer(log_rx, Arc::clone(&storage), log_tx.clone());

    // Bind one listener per configured service. Listeners are fixed at
    // startup; adding/removing services needs a restart (filter/rule edits
    // hot-reload through the bus). A failed bind is logged inside the task.
    for svc in services {
        tracing::info!(service = %svc.name, "starting listener");
        let _handle = listen_service(svc, bus.clone());
    }

    let dyn_storage: Arc<dyn Storage> = Arc::clone(&storage) as Arc<dyn Storage>;
    let state = AppState::new(
        bus,
        dyn_storage,
        Arc::new(registry),
        Key::generate(),
        Arc::new(log_tx),
    );

    let listener = tokio::net::TcpListener::bind(&cfg.web_bind)
        .await
        .with_context(|| format!("binding web UI to {}", cfg.web_bind))?;
    tracing::info!(bind = %cfg.web_bind, "web UI up");

    // Run until the web server stops or an interrupt arrives. On ctrl-c we
    // return cleanly; spawned listener/writer tasks stop when the process
    // exits.
    tokio::select! {
        res = minos_web::run(state, listener) => res.context("web server error")?,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutdown signal received; stopping");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use minos_core::{ProtocolKind, ProxyMode};
    use minos_storage::InMemoryStorage;

    fn registry() -> FilterRegistry {
        let mut r = FilterRegistry::new();
        register_builtin_filters(&mut r);
        r
    }

    fn one_service_config() -> Config {
        Config {
            services: vec![minos_config::ServiceConfig {
                name: "web".into(),
                mode: ProxyMode::Reverse {
                    bind: "127.0.0.1:9000".parse().unwrap(),
                    upstream: "127.0.0.1:5000".parse().unwrap(),
                },
                protocol: ProtocolKind::Http,
                pipeline: vec![],
                block_response_override: None,
                max_body_bytes: 1024,
            }],
            ..Config::default()
        }
    }

    #[test]
    fn seeds_services_from_file_on_first_run() {
        let storage = InMemoryStorage::new();
        let reg = registry();
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(&file, serde_json::to_vec(&one_service_config()).unwrap()).unwrap();

        let rs = load_or_seed(&storage, &reg, file.path().to_str()).unwrap();
        assert_eq!(rs.source.services.len(), 1);
        assert_eq!(rs.source.services[0].name, "web");
        // Persisted as the first version.
        assert_eq!(storage.active_version().unwrap(), 1);
    }

    #[test]
    fn bootstraps_empty_without_seed_file() {
        let storage = InMemoryStorage::new();
        let reg = registry();
        let rs = load_or_seed(&storage, &reg, None).unwrap();
        assert!(rs.source.services.is_empty());
    }

    #[test]
    fn does_not_reseed_when_active_config_exists() {
        let storage = InMemoryStorage::new();
        let reg = registry();
        // First run seeds one service.
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(&file, serde_json::to_vec(&one_service_config()).unwrap()).unwrap();
        load_or_seed(&storage, &reg, file.path().to_str()).unwrap();

        // Second run with no seed path keeps the existing config.
        let rs = load_or_seed(&storage, &reg, None).unwrap();
        assert_eq!(rs.source.services.len(), 1);
        assert_eq!(
            storage.active_version().unwrap(),
            1,
            "no new version written"
        );
    }

    #[test]
    fn errors_on_missing_seed_file() {
        let storage = InMemoryStorage::new();
        let reg = registry();
        let err = load_or_seed(&storage, &reg, Some("/no/such/file.json")).unwrap_err();
        assert!(err.to_string().contains("reading seed config"));
    }
}
