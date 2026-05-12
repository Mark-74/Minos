//! Storage trait and built-in backends for Minos.
//!
//! All persistence in Minos goes through the [`Storage`] trait. Two backends
//! ship in this crate: [`memory::InMemoryStorage`] (intended for tests) and
//! [`sqlite::SqliteStorage`] (the production default). Future backends can be
//! added as separate crates implementing [`Storage`] without changes here.

#![deny(missing_docs)]
#![allow(clippy::module_name_repetitions)]

mod error;
mod memory;
mod sqlite;
mod storage;
mod types;

pub use error::StorageError;
pub use memory::InMemoryStorage;
pub use sqlite::SqliteStorage;
pub use storage::Storage;
pub use types::VersionMeta;

/// Reusable conformance test suite for `Storage` implementations.
/// Re-exported under the `conformance` module so downstream backends in
/// separate crates can import and run it.
pub mod conformance;
