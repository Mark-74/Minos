use minos_storage::{conformance::run_conformance_suite, SqliteStorage};
use tempfile::tempdir;

#[test]
fn sqlite_in_memory_passes_conformance_suite() {
    let s = SqliteStorage::open_in_memory().unwrap();
    run_conformance_suite(&s);
}

#[test]
fn sqlite_on_disk_passes_conformance_suite() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("minos.db");
    let s = SqliteStorage::open(&path).unwrap();
    run_conformance_suite(&s);
}
