//! `GET /` — operator dashboard: live service table.

use askama::Template;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use minos_core::LogFilter;

use crate::{AppState, WebError};

/// One row in the services table.
struct ServiceRow {
    /// Service name.
    name: String,
    /// Proxy mode label: `"Reverse"` or `"Transparent"`.
    mode: String,
    /// Bind address as a string.
    bind: String,
    /// Upstream address, or `"—"` for transparent mode.
    upstream: String,
    /// Protocol label (`"Http"` or `"Tcp"`).
    protocol: String,
    /// Number of filters in the pipeline.
    filter_count: usize,
    /// Number of blocks recorded in the last 5 minutes.
    block_count_5m: usize,
}

/// Askama template context for the dashboard page.
#[derive(Template)]
#[template(path = "dashboard.html")]
struct Dash {
    /// Service rows to render in the table.
    services: Vec<ServiceRow>,
}

/// `GET /` — render the dashboard with the live service table.
///
/// Counts blocks per service from the last five minutes via
/// `Storage::query_log`.
///
/// # Errors
///
/// Returns [`WebError::Storage`] if the block-log query fails.
pub async fn get(State(state): State<AppState>) -> Result<Response, WebError> {
    let guard = state.bus.rules.load();
    let now_ms = now_ms();
    let since_ms = now_ms - 5 * 60 * 1_000;
    let mut services = Vec::with_capacity(guard.services().len());
    for (i, svc) in guard.services().iter().enumerate() {
        let (mode, bind, upstream) = match &svc.mode {
            minos_core::ProxyMode::Reverse { bind, upstream } => (
                "Reverse".to_string(),
                bind.to_string(),
                upstream.to_string(),
            ),
            minos_core::ProxyMode::Transparent { bind } => (
                "Transparent".to_string(),
                bind.to_string(),
                "\u{2014}".to_string(),
            ),
        };
        let filter_count = guard.pipeline_for(i).len();
        let block_count_5m = state
            .storage
            .query_log(
                &LogFilter {
                    services: vec![svc.name.clone()],
                    since_ms: Some(since_ms),
                    ..Default::default()
                },
                1000,
            )?
            .len();
        services.push(ServiceRow {
            name: svc.name.clone(),
            mode,
            bind,
            upstream,
            protocol: format!("{:?}", svc.protocol),
            filter_count,
            block_count_5m,
        });
    }
    let tpl = Dash { services };
    Ok(match tpl.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    })
}

/// Current Unix timestamp in milliseconds.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}
