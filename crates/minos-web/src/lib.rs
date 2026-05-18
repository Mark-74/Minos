//! Minos operator web UI: axum + Askama + htmx, signed-cookie auth.
//!
//! ## Architecture
//!
//! Construct an [`AppState`] from the shared [`minos_config::Bus`], a
//! `Storage` impl, a [`minos_core::FilterRegistry`], and a cookie-signing
//! `Key`; then hand it to [`run`] alongside a bound TCP listener. The
//! router is also exposed via [`routes::router`] for tests that drive
//! handlers directly through `tower::ServiceExt::oneshot`.
//!
//! ## Authentication
//!
//! Single-tenant, single shared password (design §9.9). The first POST to
//! `/login` with any non-empty password sets the Argon2id hash; subsequent
//! logins verify against it. The session is a signed cookie; unauthenticated
//! access to a protected route 303-redirects to `/login`.
//!
//! ## Editing model
//!
//! Pipeline edits accumulate in an in-memory draft store. Save produces a
//! fresh [`minos_config::Config`] and runs [`minos_config::save_config`],
//! which validates, persists, and swaps the `Bus`'s `RuleSet`. Discard
//! drops the draft without touching anything.
#![deny(missing_docs)]

mod assets;
pub mod auth;
mod draft;
mod error;
pub mod routes;
mod state;

pub use auth::{hash_password, verify_password, SESSION_COOKIE, SESSION_VALUE};
pub use error::WebError;
pub use state::AppState;

/// Run the web server on `listener` until the listener exits.
///
/// # Errors
///
/// Returns [`WebError::Internal`] if the underlying `axum::serve` loop
/// errors (typically only on shutdown).
pub async fn run(state: AppState, listener: tokio::net::TcpListener) -> Result<(), WebError> {
    let app = routes::router(state);
    axum::serve(listener, app)
        .await
        .map_err(|e| WebError::Internal(e.to_string()))
}
