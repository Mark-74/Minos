//! In-memory drafts of pipeline edits.
//!
//! When the operator opens a service's pipeline editor, the current
//! `ServiceConfig` is cloned into a draft. Edits accumulate against the
//! draft. On Save the draft replaces the corresponding service in a fresh
//! `Config` and is passed through `save_config`. On Discard the draft is
//! deleted (the next GET re-clones from the active `RuleSet`).
//!
//! Drafts live in memory; restart loses them. Single-user model means one
//! draft per service across the whole process is sufficient.

use std::collections::HashMap;
use std::sync::Mutex;

use minos_config::ServiceConfig;

/// Per-service draft store.
#[derive(Debug, Default)]
pub struct DraftStore {
    drafts: Mutex<HashMap<String, ServiceConfig>>,
}

impl DraftStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the draft for `service_name`, if any.
    #[must_use]
    pub fn get(&self, service_name: &str) -> Option<ServiceConfig> {
        let guard = self.drafts.lock().expect("draft lock poisoned");
        guard.get(service_name).cloned()
    }

    /// Insert or replace the draft for `service_name`.
    pub fn put(&self, service_name: &str, draft: ServiceConfig) {
        let mut guard = self.drafts.lock().expect("draft lock poisoned");
        guard.insert(service_name.to_string(), draft);
    }

    /// Drop the draft for `service_name`. No-op if absent.
    pub fn clear(&self, service_name: &str) {
        let mut guard = self.drafts.lock().expect("draft lock poisoned");
        guard.remove(service_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minos_config::ServiceConfig;
    use minos_core::{ProtocolKind, ProxyMode};

    fn svc(name: &str) -> ServiceConfig {
        ServiceConfig {
            name: name.into(),
            mode: ProxyMode::Reverse {
                bind: "127.0.0.1:0".parse().unwrap(),
                upstream: "127.0.0.1:1".parse().unwrap(),
            },
            protocol: ProtocolKind::Http,
            pipeline: vec![],
            block_response_override: None,
            max_body_bytes: 1024,
        }
    }

    #[test]
    fn empty_store_returns_none() {
        let s = DraftStore::new();
        assert!(s.get("svc").is_none());
    }

    #[test]
    fn put_then_get_returns_draft() {
        let s = DraftStore::new();
        s.put("svc", svc("svc"));
        let got = s.get("svc").unwrap();
        assert_eq!(got.name, "svc");
    }

    #[test]
    fn clear_removes_draft() {
        let s = DraftStore::new();
        s.put("svc", svc("svc"));
        s.clear("svc");
        assert!(s.get("svc").is_none());
    }

    #[test]
    fn put_replaces_existing_draft() {
        let s = DraftStore::new();
        s.put("svc", svc("svc"));
        let mut updated = svc("svc");
        updated.max_body_bytes = 9999;
        s.put("svc", updated);
        assert_eq!(s.get("svc").unwrap().max_body_bytes, 9999);
    }

    #[test]
    fn drafts_are_independent_per_service() {
        let s = DraftStore::new();
        s.put("a", svc("a"));
        s.put("b", svc("b"));
        assert_eq!(s.get("a").unwrap().name, "a");
        assert_eq!(s.get("b").unwrap().name, "b");
        s.clear("a");
        assert!(s.get("a").is_none());
        assert!(s.get("b").is_some());
    }
}
