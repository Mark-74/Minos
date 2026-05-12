//! Minos configuration types and persistence flow.
//!
//! This crate defines [`Config`] (the on-disk source of truth), the per-service
//! and per-filter sub-types, and the save/load functions that bridge a
//! [`minos_storage::Storage`] backend with a [`minos_core::FilterRegistry`].

#![deny(missing_docs)]
#![allow(clippy::module_name_repetitions)]

mod error;
mod loader;
mod types;

pub use error::ConfigError;
pub use loader::{load_active_config, save_config, validate, BuiltConfig};
pub use types::{Config, FilterInstanceCfg, ServiceConfig};
