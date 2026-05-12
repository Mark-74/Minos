//! Errors produced by the config save/load flow.

use minos_core::BuildError;
use minos_storage::StorageError;
use thiserror::Error;

/// Errors from saving or loading a `Config`.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The config blob couldn't be deserialized.
    #[error("malformed config: {0}")]
    Malformed(#[from] serde_json::Error),

    /// The storage backend rejected the operation.
    #[error("storage: {0}")]
    Storage(#[from] StorageError),

    /// At least one filter in the config could not be built.
    #[error("filter build failed for service {service} filter {filter}: {source}")]
    FilterBuild {
        /// Service name the failing filter belongs to.
        service: String,
        /// Display name of the failing filter instance.
        filter: String,
        /// Underlying registry build error.
        #[source]
        source: BuildError,
    },

    /// The config violates a structural invariant (duplicate service name, etc.).
    #[error("invalid config: {0}")]
    Invalid(String),
}
