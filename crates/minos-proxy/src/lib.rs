//! Minos data plane: tokio listeners, protocol handlers, pipeline executor.
//!
//! ## Architecture
//!
//! One [`listen_service`] task per `ServiceConfig` accepts client
//! connections, dispatches to a [`ProtocolHandler`] ([`HttpHandler`] or
//! [`TcpHandler`]), runs the inspected packet through [`execute`] against
//! the live `RuleSet` (read lock-free through `minos_config::Bus`), and
//! either forwards to the upstream or sends the configured block response.
//!
//! The single producer/consumer seam to the control plane is the bus's
//! mpsc log channel. Drain it into a `minos_storage::Storage` impl with
//! [`spawn_log_writer`].
//!
//! ## Transparent mode
//!
//! `ProxyMode::Transparent` resolves the upstream per connection via
//! [`original_dst`] (`SO_ORIGINAL_DST`, Linux only).
//!
//! ## Errors
//!
//! Per-connection failures surface as [`ProxyError`]; the accept loop logs
//! and continues. Nothing inside this crate is fatal to the listener.
#![deny(missing_docs)]

mod error;
pub use error::ProxyError;

mod handler;
pub use handler::{ProtocolHandler, ServiceContext};

mod sample;

mod executor;
pub use executor::execute;

mod http_handler;
pub use http_handler::HttpHandler;

mod tcp_handler;
pub use tcp_handler::TcpHandler;

mod transparent;
pub use transparent::original_dst;

mod forwarder;

mod listener;
pub use listener::listen_service;

mod log_writer;
pub use log_writer::spawn_log_writer;

/// Broadcast sender used to fan log entries out to live subscribers (the web
/// UI's WebSocket handlers). The log-writer task is the producer; each
/// subscriber gets its own receiver via
/// [`tokio::sync::broadcast::Sender::subscribe`].
pub type LogBroadcast = tokio::sync::broadcast::Sender<minos_core::LogEntry>;
