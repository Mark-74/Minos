//! The [`Storage`] trait every persistence backend implements.

use crate::{StorageError, VersionMeta};
use minos_core::{LogEntry, LogFilter};

/// Persistent storage for Minos: versioned config blobs, the active-version
/// pointer, the rolling block log, and key-value settings.
///
/// Implementations must be `Send + Sync` so they can be shared by the data
/// plane (which writes log entries) and the control plane (which reads/writes
/// everything else). The trait is sync; backends needing async IO wrap their
/// blocking calls in `tokio::task::block_in_place`.
///
/// **Atomicity contract.** [`save_version`](Self::save_version) must be
/// atomic (the row exists or it doesn't). [`set_active_version`](Self::set_active_version)
/// must be atomic. The save flow elsewhere relies on these being the only two
/// writes that affect "what's running."
pub trait Storage: Send + Sync {
    // ---- Versioned config ----

    /// Persist a new config blob with optional human note. Returns the
    /// assigned version number. Atomic.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::Backend` for IO/SQL failures.
    fn save_version(&self, blob: &[u8], note: Option<&str>) -> Result<u64, StorageError>;

    /// Retrieve a previously saved config blob.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::NotFound` if the version doesn't exist.
    fn load_version(&self, version: u64) -> Result<Vec<u8>, StorageError>;

    /// List all saved versions, newest first.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::Backend` for IO/SQL failures.
    fn list_versions(&self) -> Result<Vec<VersionMeta>, StorageError>;

    /// Get the version pointed to by the active-version pointer.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::NotFound` if no active version is set.
    fn active_version(&self) -> Result<u64, StorageError>;

    /// Atomically set the active-version pointer.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::NotFound` if `version` doesn't exist.
    fn set_active_version(&self, version: u64) -> Result<(), StorageError>;

    // ---- Block log ----

    /// Append a log entry. The storage layer assigns the entry's `id`.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::Backend` for IO/SQL failures.
    fn append_log(&self, entry: &LogEntry) -> Result<(), StorageError>;

    /// Query the block log with the given filter, returning at most `limit`
    /// rows in reverse-chronological order (newest first).
    ///
    /// # Errors
    ///
    /// Returns `StorageError::Backend` for IO/SQL failures.
    fn query_log(&self, filter: &LogFilter, limit: usize) -> Result<Vec<LogEntry>, StorageError>;

    // ---- Settings ----

    /// Get a setting value by key, or `None` if unset.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::Backend` for IO/SQL failures.
    fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError>;

    /// Set a setting value by key (overwriting any existing value).
    ///
    /// # Errors
    ///
    /// Returns `StorageError::Backend` for IO/SQL failures.
    fn set_setting(&self, key: &str, value: &str) -> Result<(), StorageError>;
}
