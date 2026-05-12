//! Auxiliary types used by the [`Storage`](crate::Storage) trait.

use serde::{Deserialize, Serialize};

/// Metadata for one entry in the version history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionMeta {
    /// Auto-incrementing version number. Larger = more recent.
    pub version: u64,
    /// Unix milliseconds when the version was saved.
    pub created_at: i64,
    /// Operator-supplied note (e.g. "block ssrf attempts").
    pub note: Option<String>,
}
