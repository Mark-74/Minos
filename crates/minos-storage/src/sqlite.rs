//! `SQLite` storage backend (production default).
//!
//! # Integer casting
//!
//! `SQLite` stores all integers as `i64`. The `Storage` trait exposes version
//! numbers and row ids as `u64`. The casts (`as u64`, `as i64`) are safe in
//! practice because version numbers and AUTOINCREMENT ids are bounded well
//! below `i64::MAX`. The module-level `#![allow]` below makes this explicit
//! rather than suppressing the lints ad-hoc.

#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::needless_pass_by_value
)]

use crate::{Storage, StorageError, VersionMeta};
use minos_core::{Direction, LogEntry, LogFilter};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS active_version (
    id      INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS config_versions (
    version    INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at INTEGER NOT NULL,
    note       TEXT,
    blob       BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS block_log (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    ts        INTEGER NOT NULL,
    service   TEXT NOT NULL,
    direction TEXT NOT NULL,
    filter_id TEXT NOT NULL,
    rule_kind TEXT NOT NULL,
    dry_run   INTEGER NOT NULL,
    reason    TEXT NOT NULL,
    sample    BLOB NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_block_log_ts      ON block_log (ts DESC);
CREATE INDEX IF NOT EXISTS idx_block_log_service ON block_log (service);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// Production storage backend backed by a single `SQLite` file.
///
/// The database is opened with WAL journal mode and `NORMAL` synchronous
/// setting, which is appropriate for the event-scoped lifecycle of Minos. The
/// schema is applied on open using `CREATE TABLE IF NOT EXISTS`, so the same
/// call works for both fresh databases and existing ones.
pub struct SqliteStorage {
    conn: Mutex<Connection>,
}

impl SqliteStorage {
    /// Open or create a `SQLite` database at the given path. The schema is
    /// applied if not already present (uses `CREATE TABLE IF NOT EXISTS`).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] if the file can't be opened or the
    /// schema can't be applied.
    ///
    /// # Panics
    ///
    /// Methods on this type call `Mutex::lock()` internally. If a thread
    /// panics while holding the lock, subsequent calls will panic with
    /// "lock poisoned". This is treated as an unrecoverable state.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let conn = Connection::open(path).map_err(backend)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(backend)?;
        conn.execute_batch(SCHEMA).map_err(backend)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open an in-memory `SQLite` database. Useful for tests that want to drive
    /// the same code path as production but throw the data away.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] if `SQLite` can't allocate the
    /// in-memory database.
    ///
    /// # Panics
    ///
    /// See [`open`](Self::open) — the same lock-poisoning note applies.
    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory().map_err(backend)?;
        conn.execute_batch(SCHEMA).map_err(backend)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

fn backend(e: rusqlite::Error) -> StorageError {
    StorageError::Backend(e.to_string())
}

fn now_ms() -> i64 {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(dur.as_millis()).unwrap_or(i64::MAX)
}

fn dir_to_str(d: Direction) -> &'static str {
    match d {
        Direction::Inbound => "inbound",
        Direction::Outbound => "outbound",
    }
}

fn dir_from_str(s: &str) -> Result<Direction, StorageError> {
    match s {
        "inbound" => Ok(Direction::Inbound),
        "outbound" => Ok(Direction::Outbound),
        other => Err(StorageError::Malformed(format!(
            "unknown direction {other}"
        ))),
    }
}

/// Read a block-log row into a `(LogEntry, packed_meta)` tuple.
///
/// The packed-meta string has the form `"<direction>|<filter_id>"`.  It is
/// returned separately because direction parsing can fail with
/// [`StorageError::Malformed`], but `rusqlite`'s row-mapping closure must
/// return `rusqlite::Result` — we can't propagate our own error type from
/// inside it. The caller unpacks and validates after the closure returns.
fn row_to_log_entry(row: &Row<'_>) -> rusqlite::Result<(LogEntry, String)> {
    let id: i64 = row.get("id")?;
    let ts: i64 = row.get("ts")?;
    let service: String = row.get("service")?;
    let direction: String = row.get("direction")?;
    let filter_id_str: String = row.get("filter_id")?;
    let rule_kind: String = row.get("rule_kind")?;
    let dry_run: i64 = row.get("dry_run")?;
    let reason: String = row.get("reason")?;
    let sample: Vec<u8> = row.get("sample")?;
    let packed = format!("{direction}|{filter_id_str}");
    Ok((
        LogEntry {
            id: Some(id as u64),
            ts,
            service,
            // direction and filter_id are placeholders fixed by the caller.
            direction: Direction::Inbound,
            filter_id: uuid::Uuid::nil(),
            rule_kind,
            dry_run: dry_run != 0,
            reason,
            sample,
        },
        packed,
    ))
}

impl Storage for SqliteStorage {
    fn save_version(&self, blob: &[u8], note: Option<&str>) -> Result<u64, StorageError> {
        let conn = self.conn.lock().expect("lock poisoned");
        conn.execute(
            "INSERT INTO config_versions (created_at, note, blob) VALUES (?, ?, ?)",
            params![now_ms(), note, blob],
        )
        .map_err(backend)?;
        Ok(conn.last_insert_rowid() as u64)
    }

    fn load_version(&self, version: u64) -> Result<Vec<u8>, StorageError> {
        let conn = self.conn.lock().expect("lock poisoned");
        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT blob FROM config_versions WHERE version = ?",
                params![version as i64],
                |r| r.get(0),
            )
            .optional()
            .map_err(backend)?;
        blob.ok_or_else(|| StorageError::NotFound(format!("version {version}")))
    }

    fn list_versions(&self) -> Result<Vec<VersionMeta>, StorageError> {
        let conn = self.conn.lock().expect("lock poisoned");
        let mut stmt = conn
            .prepare("SELECT version, created_at, note FROM config_versions ORDER BY version DESC")
            .map_err(backend)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(VersionMeta {
                    version: r.get::<_, i64>(0)? as u64,
                    created_at: r.get(1)?,
                    note: r.get(2)?,
                })
            })
            .map_err(backend)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(backend)?);
        }
        Ok(out)
    }

    fn active_version(&self) -> Result<u64, StorageError> {
        let conn = self.conn.lock().expect("lock poisoned");
        let v: Option<i64> = conn
            .query_row("SELECT version FROM active_version WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(backend)?;
        v.map(|n| n as u64)
            .ok_or_else(|| StorageError::NotFound("no active version".into()))
    }

    fn set_active_version(&self, version: u64) -> Result<(), StorageError> {
        let conn = self.conn.lock().expect("lock poisoned");
        // Validate that the version exists — the conformance contract requires
        // this to return NotFound for unknown versions.
        let exists: Option<i64> = conn
            .query_row(
                "SELECT version FROM config_versions WHERE version = ?",
                params![version as i64],
                |r| r.get(0),
            )
            .optional()
            .map_err(backend)?;
        if exists.is_none() {
            return Err(StorageError::NotFound(format!("version {version}")));
        }
        conn.execute(
            "INSERT INTO active_version (id, version) VALUES (1, ?)
             ON CONFLICT(id) DO UPDATE SET version = excluded.version",
            params![version as i64],
        )
        .map_err(backend)?;
        Ok(())
    }

    fn append_log(&self, entry: &LogEntry) -> Result<(), StorageError> {
        let conn = self.conn.lock().expect("lock poisoned");
        conn.execute(
            "INSERT INTO block_log (ts, service, direction, filter_id, rule_kind, dry_run, reason, sample)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                entry.ts,
                entry.service,
                dir_to_str(entry.direction),
                entry.filter_id.to_string(),
                entry.rule_kind,
                i64::from(entry.dry_run),
                entry.reason,
                entry.sample,
            ],
        )
        .map_err(backend)?;
        Ok(())
    }

    fn query_log(&self, filter: &LogFilter, limit: usize) -> Result<Vec<LogEntry>, StorageError> {
        let conn = self.conn.lock().expect("lock poisoned");

        let mut sql = String::from(
            "SELECT id, ts, service, direction, filter_id, rule_kind, dry_run, reason, sample
             FROM block_log",
        );
        let mut clauses: Vec<String> = Vec::new();
        // Bind values accumulated in order of `clauses`.
        let mut bind: Vec<rusqlite::types::Value> = Vec::new();

        if !filter.services.is_empty() {
            let placeholders = std::iter::repeat("?")
                .take(filter.services.len())
                .collect::<Vec<_>>()
                .join(", ");
            clauses.push(format!("service IN ({placeholders})"));
            for s in &filter.services {
                bind.push(s.clone().into());
            }
        }
        if !filter.directions.is_empty() {
            let placeholders = std::iter::repeat("?")
                .take(filter.directions.len())
                .collect::<Vec<_>>()
                .join(", ");
            clauses.push(format!("direction IN ({placeholders})"));
            for d in &filter.directions {
                bind.push(dir_to_str(*d).to_string().into());
            }
        }
        if !filter.kinds.is_empty() {
            let placeholders = std::iter::repeat("?")
                .take(filter.kinds.len())
                .collect::<Vec<_>>()
                .join(", ");
            clauses.push(format!("rule_kind IN ({placeholders})"));
            for k in &filter.kinds {
                bind.push(k.clone().into());
            }
        }
        if let Some(id) = filter.filter_id {
            clauses.push("filter_id = ?".into());
            bind.push(id.to_string().into());
        }
        if let Some(want_dry) = filter.dry_run_only {
            clauses.push("dry_run = ?".into());
            bind.push(i64::from(want_dry).into());
        }
        if let Some(ref needle) = filter.search {
            clauses.push("(LOWER(reason) LIKE ? OR LOWER(CAST(sample AS TEXT)) LIKE ?)".into());
            let like = format!("%{}%", needle.to_lowercase());
            bind.push(like.clone().into());
            bind.push(like.into());
        }
        if let Some(since) = filter.since_ms {
            clauses.push("ts >= ?".into());
            bind.push(since.into());
        }
        if let Some(until) = filter.until_ms {
            clauses.push("ts < ?".into());
            bind.push(until.into());
        }

        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        bind.push((limit as i64).into());

        let mut stmt = conn.prepare(&sql).map_err(backend)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(bind.iter()), |r| {
                row_to_log_entry(r)
            })
            .map_err(backend)?;

        let mut out = Vec::with_capacity(limit);
        for r in rows {
            let (mut entry, packed) = r.map_err(backend)?;
            // Unpack direction + filter_id from the helper's packed string.
            let mut parts = packed.splitn(2, '|');
            let dir_str = parts.next().unwrap_or("");
            let id_str = parts.next().unwrap_or("");
            entry.direction = dir_from_str(dir_str)?;
            entry.filter_id = uuid::Uuid::parse_str(id_str)
                .map_err(|e| StorageError::Malformed(format!("filter_id: {e}")))?;
            out.push(entry);
        }
        Ok(out)
    }

    fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        let conn = self.conn.lock().expect("lock poisoned");
        let v: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(backend)?;
        Ok(v)
    }

    fn set_setting(&self, key: &str, value: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock().expect("lock poisoned");
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(backend)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance::run_conformance_suite;

    #[test]
    fn sqlite_passes_conformance_suite() {
        let s = SqliteStorage::open_in_memory().expect("open in-memory");
        run_conformance_suite(&s);
    }

    #[test]
    fn open_file_creates_schema() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("minos.db");
        {
            let s = SqliteStorage::open(&path).expect("open");
            s.save_version(b"v1", None).expect("save");
        }
        // Re-open: schema already exists, should not error.
        let s = SqliteStorage::open(&path).expect("reopen");
        assert_eq!(s.load_version(1).expect("load"), b"v1".to_vec());
    }
}
