//! Hot-reload seam between the control plane (rule writer) and the data plane
//! (filter executor + log producer).
//!
//! The two concurrency primitives here — `ArcSwap<RuleSet>` and the mpsc log
//! channel — are the only seams between the control plane and the data plane.
//! Centralising them here lets the rest of the workspace treat the seam as a
//! single owned handle.

use std::sync::Arc;

use arc_swap::ArcSwap;
use minos_core::LogEntry;
use tokio::sync::mpsc;

use crate::RuleSet;

/// One end of the mpsc log channel. Cheap to clone; every clone produces
/// another producer handle. The data plane holds clones; the receiver half
/// is owned by the log-writer task.
pub type LogSink = mpsc::UnboundedSender<LogEntry>;

/// The hot-reload bus. Holds the two seams between the control plane and the
/// data plane:
///
/// * `rules` — an `Arc<ArcSwap<RuleSet>>`. Readers call `.load()` to get a
///   guard that holds a strong reference; the `ArcSwap` itself is a single
///   atomic pointer. Writes go through [`Bus::swap`].
/// * `log` — a `LogSink` that the data plane uses to publish [`LogEntry`]
///   values. The control plane owns the matching receiver and drains it via
///   the log-writer task in `minos-proxy`.
///
/// `Bus` is `Clone`; both halves of the seam are reference-counted, so a
/// clone is cheap. Hand a clone to every accept task.
#[derive(Clone)]
pub struct Bus {
    /// Lock-free atomic pointer to the current `RuleSet`. Cloning the `Arc`
    /// is cheap; `.load()` returns a guard that keeps the inner alive.
    pub rules: Arc<ArcSwap<RuleSet>>,
    /// Producer half of the log mpsc channel.
    pub log: LogSink,
}

impl Bus {
    /// Atomically install a new `RuleSet`. In-flight `load()` guards continue
    /// to hold the old value; subsequent `.load()` calls return the new one.
    pub fn swap(&self, next: RuleSet) {
        self.rules.store(Arc::new(next));
    }
}

/// Construct a new [`Bus`] plus the matching log receiver. The receiver is not
/// part of `Bus` because there is exactly one consumer (the log-writer task)
/// and giving it `Clone` would lie about its semantics.
#[must_use]
pub fn new_bus(initial: RuleSet) -> (Bus, mpsc::UnboundedReceiver<LogEntry>) {
    let rules = Arc::new(ArcSwap::from(Arc::new(initial)));
    let (tx, rx) = mpsc::unbounded_channel();
    (Bus { rules, log: tx }, rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use crate::RuleSet;

    #[test]
    fn new_bus_starts_with_initial_ruleset() {
        let cfg = Config::default();
        let initial = RuleSet::empty_for(cfg);
        let (bus, _rx) = new_bus(initial);
        let guard = bus.rules.load();
        assert!(guard.services().is_empty());
    }

    #[test]
    fn swap_updates_visible_ruleset() {
        let cfg = Config::default();
        let initial = RuleSet::empty_for(cfg.clone());
        let (bus, _rx) = new_bus(initial);

        let next = RuleSet::empty_for(cfg);
        bus.swap(next);

        let guard = bus.rules.load();
        assert!(guard.services().is_empty());
    }

    #[tokio::test]
    async fn log_sink_delivers_entries_in_order() {
        let cfg = Config::default();
        let initial = RuleSet::empty_for(cfg);
        let (bus, mut rx) = new_bus(initial);

        bus.log.send(sample_log_entry("a")).expect("send a");
        bus.log.send(sample_log_entry("b")).expect("send b");

        let first = rx.recv().await.expect("a");
        let second = rx.recv().await.expect("b");
        assert_eq!(first.service, "a");
        assert_eq!(second.service, "b");
    }

    fn sample_log_entry(service: &str) -> minos_core::LogEntry {
        minos_core::LogEntry {
            id: None,
            ts: 0,
            service: service.into(),
            direction: minos_core::Direction::Inbound,
            filter_id: uuid::Uuid::nil(),
            rule_kind: "test".into(),
            dry_run: false,
            reason: "r".into(),
            sample: vec![],
        }
    }
}
