//! Errors produced by storage backends.

use thiserror::Error;

/// Errors a storage backend can produce.
#[derive(Debug, Error)]
pub enum StorageError {
    /// The requested version (or other row) was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Backend-specific IO/serialization/etc. failure.
    #[error("backend error: {0}")]
    Backend(String),

    /// The blob could not be decoded by the caller (e.g. invalid JSON).
    #[error("malformed data: {0}")]
    Malformed(String),
}
