//! Reusable conformance test suite. Any `Storage` impl can be exercised by
//! calling [`run_conformance_suite`](crate::conformance::run_conformance_suite).

use crate::{Storage, StorageError};
use minos_core::{Direction, LogEntry, LogFilter};
use uuid::Uuid;

fn entry(service: &str, dir: Direction, kind: &str, dry: bool, reason: &str) -> LogEntry {
    LogEntry {
        id: None,
        ts: 0,
        service: service.to_string(),
        direction: dir,
        filter_id: Uuid::nil(),
        rule_kind: kind.to_string(),
        dry_run: dry,
        reason: reason.to_string(),
        sample: b"sample".to_vec(),
    }
}

/// Drive a `Storage` impl through every public method. Panics on the first
/// behavior that doesn't match the contract.
///
/// # Panics
///
/// Panics if any storage method returns a result that violates the `Storage`
/// contract (wrong error variant, wrong value, wrong ordering, etc.).
pub fn run_conformance_suite<S: Storage>(s: &S) {
    // --- Versions ---
    assert!(matches!(s.active_version(), Err(StorageError::NotFound(_))));
    assert!(s.list_versions().expect("list").is_empty());

    let v1 = s.save_version(b"first", Some("one")).expect("save v1");
    let v2 = s.save_version(b"second", None).expect("save v2");
    assert_eq!(v1, 1);
    assert_eq!(v2, 2);
    assert_eq!(s.load_version(1).unwrap(), b"first".to_vec());
    assert_eq!(s.load_version(2).unwrap(), b"second".to_vec());
    let metas = s.list_versions().unwrap();
    assert_eq!(metas.len(), 2);
    assert_eq!(metas[0].version, 2);
    assert_eq!(metas[0].note, None);
    assert_eq!(metas[1].version, 1);
    assert_eq!(metas[1].note, Some("one".into()));

    // load_version on missing
    assert!(matches!(s.load_version(99), Err(StorageError::NotFound(_))));

    // active version
    s.set_active_version(2).expect("set active 2");
    assert_eq!(s.active_version().unwrap(), 2);
    s.set_active_version(1).expect("set active 1");
    assert_eq!(s.active_version().unwrap(), 1);
    assert!(matches!(
        s.set_active_version(99),
        Err(StorageError::NotFound(_))
    ));

    // --- Settings ---
    assert_eq!(s.get_setting("k").unwrap(), None);
    s.set_setting("k", "v").unwrap();
    assert_eq!(s.get_setting("k").unwrap(), Some("v".into()));
    s.set_setting("k", "v2").unwrap();
    assert_eq!(s.get_setting("k").unwrap(), Some("v2".into()));

    // --- Block log ---
    s.append_log(&entry("web-api", Direction::Inbound, "regex", false, "r1"))
        .unwrap();
    s.append_log(&entry("web-api", Direction::Outbound, "http", true, "r2"))
        .unwrap();
    s.append_log(&entry("flag-svc", Direction::Inbound, "regex", false, "r3"))
        .unwrap();

    // No filter, big limit: all rows newest-first, ids assigned.
    let all = s.query_log(&LogFilter::default(), 100).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].reason, "r3");
    assert_eq!(all[2].reason, "r1");
    for e in &all {
        assert!(e.id.is_some());
    }

    // Limit truncates.
    let two = s.query_log(&LogFilter::default(), 2).unwrap();
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].reason, "r3");
    assert_eq!(two[1].reason, "r2");

    // Filter by service.
    let only_web = s
        .query_log(
            &LogFilter {
                services: vec!["web-api".into()],
                ..Default::default()
            },
            100,
        )
        .unwrap();
    assert_eq!(only_web.len(), 2);

    // Filter by direction.
    let only_outbound = s
        .query_log(
            &LogFilter {
                directions: vec![Direction::Outbound],
                ..Default::default()
            },
            100,
        )
        .unwrap();
    assert_eq!(only_outbound.len(), 1);
    assert_eq!(only_outbound[0].reason, "r2");

    // Filter by dry_run.
    let only_dry = s
        .query_log(
            &LogFilter {
                dry_run_only: Some(true),
                ..Default::default()
            },
            100,
        )
        .unwrap();
    assert_eq!(only_dry.len(), 1);
    let only_real = s
        .query_log(
            &LogFilter {
                dry_run_only: Some(false),
                ..Default::default()
            },
            100,
        )
        .unwrap();
    assert_eq!(only_real.len(), 2);

    // Filter by search (case-insensitive on reason).
    let search = s
        .query_log(
            &LogFilter {
                search: Some("R2".into()),
                ..Default::default()
            },
            100,
        )
        .unwrap();
    assert_eq!(search.len(), 1);
    assert_eq!(search[0].reason, "r2");
}
