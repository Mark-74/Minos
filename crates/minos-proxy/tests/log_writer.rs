//! Round-trip: pushing entries through the bus's log channel persists them
//! through the `Storage::append_log` path.

use std::sync::Arc;

use minos_config::{new_bus, Config, RuleSet};
use minos_core::{Direction, LogEntry, LogFilter};
use minos_proxy::spawn_log_writer;
use minos_storage::{InMemoryStorage, Storage};
use uuid::Uuid;

fn entry(service: &str) -> LogEntry {
    LogEntry {
        id: None,
        ts: 0,
        service: service.into(),
        direction: Direction::Inbound,
        filter_id: Uuid::nil(),
        rule_kind: "k".into(),
        dry_run: false,
        reason: "r".into(),
        sample: vec![],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn writer_persists_all_entries_then_exits_on_channel_close() {
    let storage = Arc::new(InMemoryStorage::new());
    let (bus, rx) = new_bus(RuleSet {
        source: Config::default(),
        pipelines: vec![],
    });
    let (broadcast_tx, _sub) = tokio::sync::broadcast::channel(16);
    let handle = spawn_log_writer(rx, storage.clone(), broadcast_tx);

    bus.log.send(entry("a")).unwrap();
    bus.log.send(entry("b")).unwrap();
    bus.log.send(entry("c")).unwrap();
    drop(bus); // close the senders so the writer exits cleanly

    handle.await.unwrap();

    let rows = storage.query_log(&LogFilter::default(), 100).unwrap();
    assert_eq!(rows.len(), 3);
    // Storage ordering is newest-first per the Phase 1 contract.
    let services: Vec<&str> = rows.iter().map(|r| r.service.as_str()).collect();
    assert_eq!(services, vec!["c", "b", "a"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn writer_persists_all_entries_and_broadcasts_each() {
    let storage = Arc::new(InMemoryStorage::new());
    let (bus, rx) = new_bus(RuleSet {
        source: Config::default(),
        pipelines: vec![],
    });
    let (broadcast_tx, mut sub) = tokio::sync::broadcast::channel(16);
    let handle = spawn_log_writer(rx, storage.clone(), broadcast_tx);

    bus.log.send(entry("a")).unwrap();
    bus.log.send(entry("b")).unwrap();

    // Subscriber receives both, in send order, before the channel closes.
    let first = sub.recv().await.unwrap();
    let second = sub.recv().await.unwrap();
    assert_eq!(first.service, "a");
    assert_eq!(second.service, "b");

    drop(bus);
    handle.await.unwrap();

    assert_eq!(
        storage.query_log(&LogFilter::default(), 100).unwrap().len(),
        2
    );
}
