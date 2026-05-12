//! In-memory storage backend for tests.

use crate::{Storage, StorageError, VersionMeta};
use minos_core::{LogEntry, LogFilter};
use std::cmp::Reverse;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct Inner {
    versions: Vec<VersionRow>,
    active: Option<u64>,
    log: Vec<LogEntry>,
    next_log_id: u64,
    settings: std::collections::HashMap<String, String>,
}

struct VersionRow {
    meta: VersionMeta,
    blob: Vec<u8>,
}

/// In-memory `Storage` impl, used in unit tests and end-to-end tests.
/// Not for production use: state is lost when the process exits.
pub struct InMemoryStorage {
    inner: Mutex<Inner>,
}

impl InMemoryStorage {
    /// Create a fresh, empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> i64 {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(dur.as_millis()).unwrap_or(i64::MAX)
}

impl Storage for InMemoryStorage {
    fn save_version(&self, blob: &[u8], note: Option<&str>) -> Result<u64, StorageError> {
        let mut g = self.inner.lock().expect("lock poisoned");
        let next = g.versions.last().map_or(0, |r| r.meta.version) + 1;
        g.versions.push(VersionRow {
            meta: VersionMeta {
                version: next,
                created_at: now_ms(),
                note: note.map(str::to_owned),
            },
            blob: blob.to_vec(),
        });
        Ok(next)
    }

    fn load_version(&self, version: u64) -> Result<Vec<u8>, StorageError> {
        let g = self.inner.lock().expect("lock poisoned");
        g.versions
            .iter()
            .find(|r| r.meta.version == version)
            .map(|r| r.blob.clone())
            .ok_or_else(|| StorageError::NotFound(format!("version {version}")))
    }

    fn list_versions(&self) -> Result<Vec<VersionMeta>, StorageError> {
        let g = self.inner.lock().expect("lock poisoned");
        let mut metas: Vec<_> = g.versions.iter().map(|r| r.meta.clone()).collect();
        metas.sort_by_key(|m| Reverse(m.version));
        Ok(metas)
    }

    fn active_version(&self) -> Result<u64, StorageError> {
        let g = self.inner.lock().expect("lock poisoned");
        g.active
            .ok_or_else(|| StorageError::NotFound("no active version".into()))
    }

    fn set_active_version(&self, version: u64) -> Result<(), StorageError> {
        let mut g = self.inner.lock().expect("lock poisoned");
        if !g.versions.iter().any(|r| r.meta.version == version) {
            return Err(StorageError::NotFound(format!("version {version}")));
        }
        g.active = Some(version);
        Ok(())
    }

    fn append_log(&self, entry: &LogEntry) -> Result<(), StorageError> {
        let mut g = self.inner.lock().expect("lock poisoned");
        g.next_log_id += 1;
        let mut owned = entry.clone();
        owned.id = Some(g.next_log_id);
        g.log.push(owned);
        Ok(())
    }

    fn query_log(&self, filter: &LogFilter, limit: usize) -> Result<Vec<LogEntry>, StorageError> {
        let g = self.inner.lock().expect("lock poisoned");
        let mut rows: Vec<LogEntry> = g
            .log
            .iter()
            .rev()
            .filter(|e| matches_filter(e, filter))
            .cloned()
            .collect();
        rows.truncate(limit);
        Ok(rows)
    }

    fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        let g = self.inner.lock().expect("lock poisoned");
        Ok(g.settings.get(key).cloned())
    }

    fn set_setting(&self, key: &str, value: &str) -> Result<(), StorageError> {
        let mut g = self.inner.lock().expect("lock poisoned");
        g.settings.insert(key.to_owned(), value.to_owned());
        Ok(())
    }
}

fn matches_filter(e: &LogEntry, f: &LogFilter) -> bool {
    if !f.services.is_empty() && !f.services.iter().any(|s| s == &e.service) {
        return false;
    }
    if !f.directions.is_empty() && !f.directions.contains(&e.direction) {
        return false;
    }
    if !f.kinds.is_empty() && !f.kinds.iter().any(|k| k == &e.rule_kind) {
        return false;
    }
    if let Some(id) = f.filter_id {
        if id != e.filter_id {
            return false;
        }
    }
    if let Some(want_dry) = f.dry_run_only {
        if want_dry != e.dry_run {
            return false;
        }
    }
    if let Some(ref needle) = f.search {
        let needle_l = needle.to_lowercase();
        let in_reason = e.reason.to_lowercase().contains(&needle_l);
        let in_sample = String::from_utf8_lossy(&e.sample)
            .to_lowercase()
            .contains(&needle_l);
        if !in_reason && !in_sample {
            return false;
        }
    }
    if let Some(since) = f.since_ms {
        if e.ts < since {
            return false;
        }
    }
    if let Some(until) = f.until_ms {
        if e.ts >= until {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_version() {
        let s = InMemoryStorage::new();
        let v = s.save_version(b"hello", Some("first")).unwrap();
        assert_eq!(v, 1);
        assert_eq!(s.load_version(1).unwrap(), b"hello".to_vec());
    }

    #[test]
    fn versions_increment() {
        let s = InMemoryStorage::new();
        assert_eq!(s.save_version(b"a", None).unwrap(), 1);
        assert_eq!(s.save_version(b"b", None).unwrap(), 2);
        assert_eq!(s.save_version(b"c", None).unwrap(), 3);
    }

    #[test]
    fn list_versions_returns_newest_first() {
        let s = InMemoryStorage::new();
        s.save_version(b"a", None).unwrap();
        s.save_version(b"b", None).unwrap();
        s.save_version(b"c", None).unwrap();
        let metas = s.list_versions().unwrap();
        assert_eq!(
            metas.iter().map(|m| m.version).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn active_version_round_trips() {
        let s = InMemoryStorage::new();
        assert!(matches!(s.active_version(), Err(StorageError::NotFound(_))));
        s.save_version(b"x", None).unwrap();
        s.set_active_version(1).unwrap();
        assert_eq!(s.active_version().unwrap(), 1);
    }

    #[test]
    fn set_active_to_unknown_errors() {
        let s = InMemoryStorage::new();
        assert!(matches!(
            s.set_active_version(99),
            Err(StorageError::NotFound(_))
        ));
    }

    #[test]
    fn settings_round_trip() {
        let s = InMemoryStorage::new();
        assert_eq!(s.get_setting("k").unwrap(), None);
        s.set_setting("k", "v").unwrap();
        assert_eq!(s.get_setting("k").unwrap(), Some("v".into()));
    }
}
