//! Atomicity contract: an orphaned version row (saved but never made active)
//! must not affect what `active_version` returns when the store is reopened.

use minos_storage::{SqliteStorage, Storage, StorageError};
use tempfile::tempdir;

#[test]
fn orphaned_version_does_not_affect_active_version() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("minos.db");

    {
        let s = SqliteStorage::open(&path).unwrap();
        let v1 = s.save_version(b"good", None).unwrap();
        s.set_active_version(v1).unwrap();
        // Save a second version but never mark it active (simulating a crash
        // between write_version and set_active_version).
        let _v2 = s.save_version(b"orphan", None).unwrap();
    }

    // Reopen
    let s = SqliteStorage::open(&path).unwrap();
    assert_eq!(s.active_version().unwrap(), 1);
    assert_eq!(s.load_version(1).unwrap(), b"good".to_vec());
    // The orphaned row is still queryable for rollback purposes.
    assert_eq!(s.load_version(2).unwrap(), b"orphan".to_vec());
}

#[test]
fn fresh_db_has_no_active_version() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("minos.db");
    let s = SqliteStorage::open(&path).unwrap();
    assert!(matches!(s.active_version(), Err(StorageError::NotFound(_))));
}
