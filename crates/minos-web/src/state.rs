//! Shared application state extracted by every handler.

use std::sync::Arc;

use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use minos_config::Bus;
use minos_core::FilterRegistry;
use minos_storage::Storage;

use crate::draft::DraftStore;

/// State shared by every web handler. Cheap to clone — every field is
/// reference-counted.
#[derive(Clone)]
pub struct AppState {
    /// Hot-reload bus (atomic `RuleSet` + mpsc log channel).
    pub bus: Bus,
    /// Storage backend (queried for history, settings, log readback).
    pub storage: Arc<dyn Storage>,
    /// Filter registry (used by `save_config` and the auto-form renderer).
    pub registry: Arc<FilterRegistry>,
    /// Per-session draft state (in-memory; cleared on restart).
    pub drafts: Arc<DraftStore>,
    /// Cookie-signing key.
    pub cookie_key: Key,
}

impl AppState {
    /// Construct a fresh state from raw components. The Phase 5 binary
    /// builds it once at startup; tests construct it ad-hoc.
    #[must_use]
    pub fn new(
        bus: Bus,
        storage: Arc<dyn Storage>,
        registry: Arc<FilterRegistry>,
        cookie_key: Key,
    ) -> Self {
        Self {
            bus,
            storage,
            registry,
            drafts: Arc::new(DraftStore::new()),
            cookie_key,
        }
    }
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Key {
        state.cookie_key.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minos_config::{new_bus, Config, RuleSet};
    use minos_storage::InMemoryStorage;

    #[test]
    fn app_state_clones_cheaply() {
        let (bus, _rx) = new_bus(RuleSet::empty_for(Config::default()));
        let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
        let registry = Arc::new(FilterRegistry::new());
        let state = AppState::new(bus, storage, registry, Key::generate());
        let _ = state.clone();
    }
}
