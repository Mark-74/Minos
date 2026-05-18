//! `GET /services/{name}` — service detail view with pipeline listing,
//! plus `POST` mutation handlers for pipeline edits (toggle, reorder,
//! delete, save, discard).

use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use minos_config::ServiceConfig;
use serde::Deserialize;
use uuid::Uuid;

use crate::{AppState, WebError};

/// One row in the pipeline table.
// Four booleans map directly to four independent UI toggle checkboxes; a
// state machine would obscure the intent without any benefit.
#[allow(clippy::struct_excessive_bools)]
pub struct FilterRow {
    /// Stable UUID of the filter instance.
    pub id: String,
    /// Operator-supplied display name.
    pub display_name: String,
    /// `FilterKind::NAME` string (e.g. `"regex"`, `"http"`, `"python"`).
    pub kind: String,
    /// Whether the instance is currently enabled.
    pub enabled: bool,
    /// Whether the instance is in dry-run mode (log only, no block).
    pub dry_run: bool,
    /// Whether the instance inspects inbound packets.
    pub on_inbound: bool,
    /// Whether the instance inspects outbound packets.
    pub on_outbound: bool,
    /// Zero-based position in the pipeline.
    pub idx: usize,
    /// Index of the last row (used to suppress the ↓ button on the last row).
    pub last_idx: usize,
}

/// Askama template context for the service detail page.
#[derive(Template)]
#[template(path = "service_detail.html")]
struct ServiceDetail {
    /// Service name.
    name: String,
    /// Proxy mode label.
    mode: String,
    /// Bind address as a string.
    bind: String,
    /// Upstream address, or `"—"` for transparent mode.
    upstream: String,
    /// Protocol label.
    protocol: String,
    /// Whether a draft is currently active for this service.
    is_draft: bool,
    /// Pipeline rows.
    rows: Vec<FilterRow>,
}

/// `GET /services/{name}` — render the service detail and pipeline.
///
/// Shows the draft pipeline if one exists for this service; otherwise shows
/// the live pipeline from the active `RuleSet`.
///
/// # Errors
///
/// Returns [`WebError::NotFound`] if no service with `name` exists in the
/// active config.
pub async fn get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Response, WebError> {
    let guard = state.bus.rules.load();
    let svc_idx = guard
        .services()
        .iter()
        .position(|s| s.name == name)
        .ok_or_else(|| WebError::NotFound(format!("service {name}")))?;
    let live = &guard.services()[svc_idx];

    let draft = state.drafts.get(&name);
    let is_draft = draft.is_some();
    let svc = draft.as_ref().unwrap_or(live);

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

    let last_idx = svc.pipeline.len().saturating_sub(1);
    let rows = svc
        .pipeline
        .iter()
        .enumerate()
        .map(|(idx, f)| FilterRow {
            id: f.id.to_string(),
            display_name: f.display_name.clone(),
            kind: f.kind.clone(),
            enabled: f.enabled,
            dry_run: f.dry_run,
            on_inbound: f.on_inbound,
            on_outbound: f.on_outbound,
            idx,
            last_idx,
        })
        .collect();

    let tpl = ServiceDetail {
        name,
        mode,
        bind,
        upstream,
        protocol: format!("{:?}", svc.protocol),
        is_draft,
        rows,
    };
    match tpl.render() {
        Ok(html) => Ok(Html(html).into_response()),
        Err(e) => Ok((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()),
    }
}

// ---------------------------------------------------------------------------
// Helpers shared by the mutation handlers
// ---------------------------------------------------------------------------

/// Load the current draft for `name`, or clone the live service config if no
/// draft exists yet.
///
/// # Errors
///
/// Returns [`WebError::NotFound`] if `name` doesn't match any service in the
/// active [`minos_config::RuleSet`].
fn draft_for(state: &AppState, name: &str) -> Result<ServiceConfig, WebError> {
    if let Some(d) = state.drafts.get(name) {
        return Ok(d);
    }
    let guard = state.bus.rules.load();
    let svc = guard
        .services()
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| WebError::NotFound(format!("service {name}")))?
        .clone();
    Ok(svc)
}

/// Build a 303 redirect to the service detail page.
fn redirect_to_service(name: &str) -> Response {
    Redirect::to(&format!("/services/{name}")).into_response()
}

/// Parse a filter UUID from a route segment.
///
/// # Errors
///
/// Returns [`WebError::BadRequest`] if the string is not a valid UUID.
fn parse_filter_id(id: &str) -> Result<Uuid, WebError> {
    Uuid::parse_str(id).map_err(|_| WebError::BadRequest(format!("invalid filter id {id}")))
}

// ---------------------------------------------------------------------------
// POST /services/{name}/filters/{id}/up
// ---------------------------------------------------------------------------

/// Move a filter one position up (toward the front) in the pipeline draft.
///
/// No-op if the filter is already at position 0. Creates the draft from the
/// live config if none exists.
///
/// # Errors
///
/// Returns [`WebError::NotFound`] if the service or filter is unknown.
pub async fn up(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
) -> Result<Response, WebError> {
    let id = parse_filter_id(&id)?;
    let mut draft = draft_for(&state, &name)?;
    let idx = draft
        .pipeline
        .iter()
        .position(|f| f.id == id)
        .ok_or_else(|| WebError::NotFound(format!("filter {id}")))?;
    if idx > 0 {
        draft.pipeline.swap(idx, idx - 1);
    }
    state.drafts.put(&name, draft);
    Ok(redirect_to_service(&name))
}

// ---------------------------------------------------------------------------
// POST /services/{name}/filters/{id}/down
// ---------------------------------------------------------------------------

/// Move a filter one position down (toward the back) in the pipeline draft.
///
/// No-op if the filter is already at the last position. Creates the draft from
/// the live config if none exists.
///
/// # Errors
///
/// Returns [`WebError::NotFound`] if the service or filter is unknown.
pub async fn down(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
) -> Result<Response, WebError> {
    let id = parse_filter_id(&id)?;
    let mut draft = draft_for(&state, &name)?;
    let idx = draft
        .pipeline
        .iter()
        .position(|f| f.id == id)
        .ok_or_else(|| WebError::NotFound(format!("filter {id}")))?;
    if idx + 1 < draft.pipeline.len() {
        draft.pipeline.swap(idx, idx + 1);
    }
    state.drafts.put(&name, draft);
    Ok(redirect_to_service(&name))
}

// ---------------------------------------------------------------------------
// POST /services/{name}/filters/{id}/delete
// ---------------------------------------------------------------------------

/// Remove a filter from the pipeline draft entirely.
///
/// # Errors
///
/// Returns [`WebError::NotFound`] if the service or filter is unknown.
pub async fn delete(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
) -> Result<Response, WebError> {
    let id = parse_filter_id(&id)?;
    let mut draft = draft_for(&state, &name)?;
    let before = draft.pipeline.len();
    draft.pipeline.retain(|f| f.id != id);
    if draft.pipeline.len() == before {
        return Err(WebError::NotFound(format!("filter {id}")));
    }
    state.drafts.put(&name, draft);
    Ok(redirect_to_service(&name))
}

// ---------------------------------------------------------------------------
// POST /services/{name}/filters/{id}/toggle
// ---------------------------------------------------------------------------

/// Form body for the toggle action.
#[derive(Deserialize)]
pub struct ToggleForm {
    /// Which boolean field to flip: `enabled`, `dry_run`, `on_inbound`, or
    /// `on_outbound`.
    pub field: String,
}

/// Flip one boolean toggle on a filter in the pipeline draft.
///
/// # Errors
///
/// Returns [`WebError::BadRequest`] for an unknown field name, or
/// [`WebError::NotFound`] if the service or filter is unknown.
pub async fn toggle(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Form(form): Form<ToggleForm>,
) -> Result<Response, WebError> {
    let id = parse_filter_id(&id)?;
    let mut draft = draft_for(&state, &name)?;
    let f = draft
        .pipeline
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or_else(|| WebError::NotFound(format!("filter {id}")))?;
    match form.field.as_str() {
        "enabled" => f.enabled = !f.enabled,
        "dry_run" => f.dry_run = !f.dry_run,
        "on_inbound" => f.on_inbound = !f.on_inbound,
        "on_outbound" => f.on_outbound = !f.on_outbound,
        other => {
            return Err(WebError::BadRequest(format!(
                "unknown toggle field {other}"
            )))
        }
    }
    state.drafts.put(&name, draft);
    Ok(redirect_to_service(&name))
}

// ---------------------------------------------------------------------------
// POST /services/{name}/save
// ---------------------------------------------------------------------------

/// Validate and persist the current draft for a service.
///
/// Clones the live [`minos_config::Config`], replaces the matching service
/// with the draft, and calls [`minos_config::save_config`] which validates,
/// persists, and atomically swaps the bus. On success the draft is cleared.
///
/// # Errors
///
/// Returns [`WebError::BadRequest`] if there is no draft to save,
/// [`WebError::NotFound`] if the service is not found in the live config, or
/// [`WebError::Config`] if validation fails.
pub async fn save(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Response, WebError> {
    let draft = state
        .drafts
        .get(&name)
        .ok_or_else(|| WebError::BadRequest("no draft to save".into()))?;

    // Build a fresh Config: clone the live source config, replace the
    // matching service entry with the draft.
    let guard = state.bus.rules.load();
    let mut new_cfg = guard.source.clone();
    let idx = new_cfg
        .services
        .iter()
        .position(|s| s.name == name)
        .ok_or_else(|| WebError::NotFound(format!("service {name}")))?;
    new_cfg.services[idx] = draft;
    drop(guard);

    minos_config::save_config(
        state.storage.as_ref(),
        &state.registry,
        &new_cfg,
        Some("from UI"),
        Some(&state.bus),
    )?;
    state.drafts.clear(&name);
    Ok(redirect_to_service(&name))
}

// ---------------------------------------------------------------------------
// POST /services/{name}/discard
// ---------------------------------------------------------------------------

/// Discard the draft for a service and redirect back to the service page.
///
/// No-op if no draft exists.
///
/// # Errors
///
/// This handler is infallible in practice; the return type is
/// `Result<Response, WebError>` for consistency with the router.
pub async fn discard(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Response, WebError> {
    state.drafts.clear(&name);
    Ok(redirect_to_service(&name))
}
