//! Minos configuration types and persistence flow.
//!
//! This crate defines [`Config`] (the on-disk source of truth), the per-service
//! and per-filter sub-types, and the save/load functions that bridge a
//! [`minos_storage::Storage`] backend with a [`minos_core::FilterRegistry`].

#![deny(missing_docs)]
#![allow(clippy::module_name_repetitions)]

mod bus;
mod error;
mod loader;
mod types;

pub use bus::{new_bus, Bus, LogSink};
pub use error::ConfigError;
pub use loader::{load_active_config, save_config, validate, RuleSet};
pub use types::{Config, FilterInstanceCfg, ServiceConfig};
