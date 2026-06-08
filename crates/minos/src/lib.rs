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
use minos_config::{load_active_config, new_bus, save_config, validate, Config};
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
}

impl AppConfig {
    /// Read configuration from the environment, applying defaults.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            db_path: std::env::var("MINOS_DB").unwrap_or_else(|_| "minos.db".into()),
            web_bind: std::env::var("MINOS_WEB_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into()),
        }
    }
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

    // Load the active config; bootstrap an empty one on first run so the UI
    // has a version to edit.
    let ruleset = match load_active_config(storage.as_ref(), &registry) {
        Ok(rs) => rs,
        Err(e) => {
            tracing::warn!(error = %e, "no active config found; bootstrapping an empty one");
            let empty = Config::default();
            save_config(storage.as_ref(), &registry, &empty, Some("bootstrap"), None)
                .context("saving bootstrap config")?;
            validate(&empty, &registry).context("validating bootstrap config")?
        }
    };

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
