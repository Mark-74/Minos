//! The Python sidecar wrapper script, embedded at compile time.
//!
//! The supervisor writes [`WRAPPER_SOURCE`] to disk before spawning the
//! sidecar process. Bundling the script in-binary keeps Minos a single
//! deployable artifact.

/// Full source of the Python wrapper script. See `wrapper.py` in this
/// directory for the human-edited version.
pub const WRAPPER_SOURCE: &str = include_str!("wrapper.py");
