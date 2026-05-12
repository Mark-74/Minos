//! Core domain types and traits for the Minos firewall.
//!
//! This crate is dependency-light by design: it defines the shared vocabulary
//! (`Filter`, `Packet`, `Verdict`, `FilterRegistry`, etc.) that every other
//! Minos crate depends on. Nothing in here performs IO.

#![deny(missing_docs)]
#![allow(clippy::module_name_repetitions)]

mod direction;
mod filter;
mod filter_instance;
mod filter_kind;
mod log_entry;
mod packet;
mod protocol;
mod registry;
mod verdict;

pub use direction::Direction;
pub use filter::Filter;
pub use filter_instance::FilterInstance;
pub use filter_kind::{BuildError, FilterKind};
pub use log_entry::{LogEntry, LogFilter};
pub use packet::{Packet, ParsedHttp};
pub use protocol::{ProtocolKind, ProxyMode};
pub use registry::FilterRegistry;
pub use verdict::{BlockResponse, Verdict};
