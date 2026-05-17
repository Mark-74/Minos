//! Built-in filter kinds for Minos: `regex`, `http`, and `python_sidecar`.
//!
//! ## Quick start
//!
//! ```no_run
//! use minos_core::FilterRegistry;
//! use minos_filters::register_builtin_filters;
//!
//! let mut registry = FilterRegistry::new();
//! register_builtin_filters(&mut registry);
//! ```
//!
//! After registration, saved configs (handled by `minos-config`) can
//! reference any of the three kinds by name. The Minos binary calls
//! [`register_builtin_filters`] once at startup.
//!
//! ## The three kinds
//!
//! * [`RegexKind`] — raw-byte regex matched against the packet contents.
//!   For HTTP packets, matched against `method + " " + path + body`.
//! * [`HttpKind`] — method / path-regex / header-regex / body-regex
//!   matchers AND-ed together. Applies only to HTTP packets.
//! * [`PythonSidecarKind`] — defers to a user-written `filter(packet)`
//!   function in a per-service Python sidecar. The sidecar runs inside a
//!   hash-deduped venv materialised at build time via `uv`. Default
//!   failure mode is fail-open (Pass on timeout, crash, or missing
//!   sidecar) per design §7.5.
//!
//! ## Adding a new kind
//!
//! See `docs/developer/filters.md`.
#![deny(missing_docs)]

mod error;
pub use error::FilterError;

mod regex_filter;
pub use regex_filter::{RegexConfig, RegexFilter, RegexKind};

mod http_filter;
pub use http_filter::{HeaderMatch, HttpConfig, HttpFilter, HttpKind};

pub mod sidecar;
pub use sidecar::filter::{PythonSidecarConfig, PythonSidecarFilter, PythonSidecarKind};

/// Register the three built-in filter kinds — `regex`, `http`, and
/// `python_sidecar` — on `registry`. The Minos binary calls this once at
/// startup so saved configs can reference any of them. Tests can call it
/// ad-hoc.
pub fn register_builtin_filters(registry: &mut minos_core::FilterRegistry) {
    registry.register::<RegexKind>();
    registry.register::<HttpKind>();
    registry.register::<PythonSidecarKind>();
}
