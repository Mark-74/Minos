//! `GET /history` — version list; `GET /history/diff` — side-by-side compare;
//! `POST /history/{version}/rollback`.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};

use crate::{AppState, WebError};

/// One row in the history table.
struct Row {
    /// Version number.
    version: u64,
    /// Unix epoch seconds (milliseconds divided by 1000).
    created_at_secs: i64,
    /// Operator note, or empty string.
    note: String,
    /// Whether this is the currently active version.
    is_active: bool,
}

/// Askama template context for the history page.
#[derive(Template)]
#[template(path = "history.html")]
struct History {
    /// Version rows, newest first.
    rows: Vec<Row>,
    /// Currently active version, or 0 if none (used for "diff vs active").
    active_version: u64,
}

/// `GET /history` — render version list with active version highlighted.
///
/// # Errors
///
/// Returns [`WebError::Storage`] if versions or the active pointer cannot be
/// read from storage.
pub async fn get(State(state): State<AppState>) -> Result<Response, WebError> {
    let versions = state.storage.list_versions()?;
    let active = state.storage.active_version().ok();
    let rows: Vec<Row> = versions
        .into_iter()
        .map(|v| Row {
            version: v.version,
            created_at_secs: v.created_at / 1000,
            note: v.note.unwrap_or_default(),
            is_active: active == Some(v.version),
        })
        .collect();
    Ok(render(History {
        rows,
        active_version: active.unwrap_or(0),
    }))
}

/// `POST /history/{version}/rollback` — re-save an old config blob as a new
/// version and redirect to `/history`.
///
/// The new version's blob is identical to the rolled-back version's blob; this
/// keeps history as a stack of intentional saves.
///
/// # Errors
///
/// Returns [`WebError::Storage`] if the blob cannot be loaded,
/// [`WebError::Internal`] if it cannot be deserialized, or
/// [`WebError::Config`] if the config fails re-validation.
pub async fn rollback(
    State(state): State<AppState>,
    Path(version): Path<u64>,
) -> Result<Response, WebError> {
    let blob = state.storage.load_version(version)?;
    let cfg: minos_config::Config = serde_json::from_slice(&blob)
        .map_err(|e| WebError::Internal(format!("malformed config blob v{version}: {e}")))?;
    minos_config::save_config(
        state.storage.as_ref(),
        &state.registry,
        &cfg,
        Some(&format!("rollback to v{version}")),
        Some(&state.bus),
    )?;
    Ok(Redirect::to("/history").into_response())
}

/// Askama template context for the side-by-side diff page.
#[derive(Template)]
#[template(path = "history_diff.html")]
struct HistoryDiff {
    /// "From" version number.
    from: u64,
    /// "To" version number.
    to: u64,
    /// Pretty-printed JSON of the "from" config blob.
    a: String,
    /// Pretty-printed JSON of the "to" config blob.
    b: String,
}

/// `GET /history/diff?from={a}&to={b}` — render two config blobs as
/// pretty-printed JSON, side by side. No semantic diffing for v1; the
/// operator's eye does the comparison.
///
/// # Errors
///
/// Returns [`WebError::BadRequest`] if `from`/`to` are missing or unparseable,
/// [`WebError::Storage`] if a version blob can't be read, or
/// [`WebError::Internal`] if a blob isn't valid JSON.
#[allow(clippy::implicit_hasher)]
pub async fn diff(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, WebError> {
    let from = parse_version(&q, "from")?;
    let to = parse_version(&q, "to")?;
    let a = pretty(&state.storage.load_version(from)?)?;
    let b = pretty(&state.storage.load_version(to)?)?;
    Ok(render(HistoryDiff { from, to, a, b }))
}

fn parse_version(q: &HashMap<String, String>, key: &str) -> Result<u64, WebError> {
    q.get(key)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| WebError::BadRequest(format!("{key} must be a version number")))
}

fn pretty(blob: &[u8]) -> Result<String, WebError> {
    let v: serde_json::Value = serde_json::from_slice(blob)
        .map_err(|e| WebError::Internal(format!("malformed config blob: {e}")))?;
    serde_json::to_string_pretty(&v).map_err(|e| WebError::Internal(format!("pretty-print: {e}")))
}

fn render<T: Template>(t: T) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
