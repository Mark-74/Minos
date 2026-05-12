//! Block-log entry produced by the executor and persisted by the storage layer.

use crate::Direction;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One row in the block log: a record of a single filter decision that either
/// blocked traffic or would have blocked it (dry-run match).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Storage-assigned id; `None` until the row is persisted.
    pub id: Option<u64>,
    /// Unix milliseconds when the decision was made.
    pub ts: i64,
    /// Service name (matches `ServiceConfig::name`).
    pub service: String,
    /// Direction the inspected packet was flowing.
    pub direction: Direction,
    /// `FilterInstance::id` of the filter that fired.
    pub filter_id: Uuid,
    /// `Filter::kind()` of the filter that fired (denormalized for query speed).
    pub rule_kind: String,
    /// Whether this was a dry-run match (true) or a real block (false).
    pub dry_run: bool,
    /// Human-readable reason from the filter.
    pub reason: String,
    /// Truncated bytes of the matched packet (capped by storage layer).
    pub sample: Vec<u8>,
}

/// Predicate for `Storage::query_log`. Empty vectors mean "no constraint on
/// this dimension"; `None` for time fields means "no bound."
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogFilter {
    /// Restrict to these services. Empty = all services.
    pub services: Vec<String>,
    /// Restrict to these directions. Empty = both.
    pub directions: Vec<Direction>,
    /// Restrict to these filter kinds. Empty = all kinds.
    pub kinds: Vec<String>,
    /// Restrict to a specific filter instance.
    pub filter_id: Option<Uuid>,
    /// `Some(true)` shows only dry-run matches; `Some(false)` shows only real
    /// blocks; `None` shows both.
    pub dry_run_only: Option<bool>,
    /// Substring match against `reason` and `sample` (UTF-8 lossy).
    pub search: Option<String>,
    /// Earliest unix millisecond timestamp (inclusive).
    pub since_ms: Option<i64>,
    /// Latest unix millisecond timestamp (exclusive).
    pub until_ms: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> LogEntry {
        LogEntry {
            id: None,
            ts: 1_700_000_000_000,
            service: "web-api".into(),
            direction: Direction::Inbound,
            filter_id: Uuid::nil(),
            rule_kind: "regex".into(),
            dry_run: false,
            reason: "matched evil".into(),
            sample: b"GET /?q=evil".to_vec(),
        }
    }

    #[test]
    fn log_entry_round_trips_through_serde() {
        let e = sample_entry();
        let json = serde_json::to_string(&e).unwrap();
        let back: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn log_filter_defaults_to_unrestricted() {
        let f = LogFilter::default();
        assert!(f.services.is_empty());
        assert!(f.directions.is_empty());
        assert!(f.kinds.is_empty());
        assert!(f.filter_id.is_none());
        assert!(f.dry_run_only.is_none());
        assert!(f.search.is_none());
        assert!(f.since_ms.is_none());
        assert!(f.until_ms.is_none());
    }
}
