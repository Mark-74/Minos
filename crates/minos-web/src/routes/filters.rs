//! Filter editor routes: new, edit, and update filter instances in a draft.

use std::collections::HashMap;
use std::hash::BuildHasher;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use minos_config::FilterInstanceCfg;
use serde::Deserialize;
use uuid::Uuid;

use crate::{AppState, WebError};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Load the current draft for `name`, or clone the live service config if no
/// draft exists yet.
///
/// # Errors
///
/// Returns [`WebError::NotFound`] if `name` doesn't match any service in the
/// active [`minos_config::RuleSet`].
fn draft_for(state: &AppState, name: &str) -> Result<minos_config::ServiceConfig, WebError> {
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

/// Build a 303 redirect to the given path.
fn redirect_to(path: &str) -> Response {
    Redirect::to(path).into_response()
}

/// Render an Askama template to an HTML response.
fn render<T: Template>(t: T) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ---------------------------------------------------------------------------
// GET /services/{name}/filters/new
// ---------------------------------------------------------------------------

/// Askama template context for the "new filter" form.
#[derive(Template)]
#[template(path = "filter_new.html")]
struct NewForm {
    /// Service name (URL segment and display).
    name: String,
    /// All registered filter kind names, for the dropdown.
    kinds: Vec<&'static str>,
}

/// Render the new-filter form with a dropdown of registered kinds.
///
/// # Errors
///
/// Returns [`WebError::NotFound`] if the service does not exist.
pub async fn new_get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Response, WebError> {
    // Verify the service exists.
    draft_for(&state, &name)?;
    let mut kinds = state.registry.known_kinds();
    kinds.sort_unstable();
    Ok(render(NewForm { name, kinds }))
}

// ---------------------------------------------------------------------------
// POST /services/{name}/filters/new
// ---------------------------------------------------------------------------

/// Form body for the new-filter POST.
#[derive(Deserialize)]
pub struct NewFilterForm {
    /// Which filter kind to create.
    kind: String,
    /// Operator-supplied display name.
    display_name: String,
}

/// Create a new filter instance in the draft pipeline and redirect to its
/// editor.
///
/// # Errors
///
/// Returns [`WebError::NotFound`] if the service does not exist, or
/// [`WebError::BadRequest`] if `display_name` is blank or `kind` is unknown.
pub async fn new_post(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Form(form): Form<NewFilterForm>,
) -> Result<Response, WebError> {
    if form.display_name.trim().is_empty() {
        return Err(WebError::BadRequest("display_name required".into()));
    }
    if !state.registry.known_kinds().contains(&form.kind.as_str()) {
        return Err(WebError::BadRequest(format!("unknown kind {}", form.kind)));
    }
    let mut draft = draft_for(&state, &name)?;
    let id = Uuid::new_v4();
    draft.pipeline.push(FilterInstanceCfg {
        id,
        display_name: form.display_name,
        kind: form.kind,
        config: serde_json::json!({}),
        enabled: true,
        dry_run: true,
        on_inbound: true,
        on_outbound: false,
    });
    state.drafts.put(&name, draft);
    Ok(redirect_to(&format!("/services/{name}/filters/{id}")))
}

// ---------------------------------------------------------------------------
// Template contexts for the kind-specific edit pages
// ---------------------------------------------------------------------------

/// Askama template context for the regex filter editor.
#[derive(Template)]
#[template(path = "filter_edit_regex.html")]
struct RegexEdit {
    /// Service name.
    name: String,
    /// Filter instance UUID as a string.
    id: String,
    /// Operator-supplied display name.
    display_name: String,
    /// Current regex pattern.
    pattern: String,
}

/// Askama template context for the HTTP filter editor.
#[derive(Template)]
#[template(path = "filter_edit_http.html")]
struct HttpEdit {
    /// Service name.
    name: String,
    /// Filter instance UUID as a string.
    id: String,
    /// Operator-supplied display name.
    display_name: String,
    /// HTTP methods as a comma-separated string.
    methods: String,
    /// Optional path regex.
    path_regex: String,
    /// Optional body regex.
    body_regex: String,
    /// Header rules, one per line (`Name: regex`).
    headers_text: String,
}

/// Askama template context for the `python_sidecar` filter editor.
#[derive(Template)]
#[template(path = "filter_edit_python.html")]
struct PythonEdit {
    /// Service name.
    name: String,
    /// Filter instance UUID as a string.
    id: String,
    /// Operator-supplied display name.
    display_name: String,
    /// Python script source.
    script: String,
    /// pip requirements, one per line.
    requirements: String,
    /// Per-call timeout in milliseconds.
    timeout_ms: u64,
    /// Whether to use fail-closed mode.
    fail_closed: bool,
}

/// Askama template context for the generic JSON-textarea editor.
#[derive(Template)]
#[template(path = "filter_edit_generic.html")]
struct GenericEdit {
    /// Service name.
    name: String,
    /// Filter instance UUID as a string.
    id: String,
    /// Filter kind name.
    kind: String,
    /// Operator-supplied display name.
    display_name: String,
    /// Current config as pretty-printed JSON.
    config_json: String,
}

// ---------------------------------------------------------------------------
// GET /services/{name}/filters/{id}
// ---------------------------------------------------------------------------

/// Render the kind-appropriate editor for an existing filter instance.
///
/// Dispatches on the `kind` field of the instance:
/// - `regex` → `filter_edit_regex.html`
/// - `http` → `filter_edit_http.html`
/// - `python_sidecar` → `filter_edit_python.html`
/// - anything else → `filter_edit_generic.html` (JSON textarea)
///
/// # Errors
///
/// Returns [`WebError::BadRequest`] if `id` is not a valid UUID, or
/// [`WebError::NotFound`] if the service or filter does not exist.
pub async fn edit_get(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
) -> Result<Response, WebError> {
    let id_uuid =
        Uuid::parse_str(&id).map_err(|_| WebError::BadRequest(format!("invalid id {id}")))?;
    let svc = draft_for(&state, &name)?;
    let f = svc
        .pipeline
        .iter()
        .find(|f| f.id == id_uuid)
        .ok_or_else(|| WebError::NotFound(format!("filter {id}")))?;

    Ok(match f.kind.as_str() {
        "regex" => render_regex_edit(name, id, f),
        "http" => render_http_edit(name, id, f),
        "python_sidecar" => render_python_edit(name, id, f),
        other => render_generic_edit(name, id, other, f),
    })
}

fn render_regex_edit(name: String, id: String, f: &minos_config::FilterInstanceCfg) -> Response {
    let pattern = f
        .config
        .get("pattern")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    render(RegexEdit {
        name,
        id,
        display_name: f.display_name.clone(),
        pattern,
    })
}

fn render_http_edit(name: String, id: String, f: &minos_config::FilterInstanceCfg) -> Response {
    let methods = f
        .config
        .get("methods")
        .and_then(serde_json::Value::as_array)
        .map_or_else(String::new, |arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        });
    let path_regex = f
        .config
        .get("path_regex")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let body_regex = f
        .config
        .get("body_regex")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let headers_text = f
        .config
        .get("headers")
        .and_then(serde_json::Value::as_array)
        .map_or_else(String::new, |arr| {
            arr.iter()
                .filter_map(|h| {
                    let n = h.get("name")?.as_str()?;
                    let r = h.get("value_regex")?.as_str()?;
                    Some(format!("{n}: {r}"))
                })
                .collect::<Vec<_>>()
                .join("\n")
        });
    render(HttpEdit {
        name,
        id,
        display_name: f.display_name.clone(),
        methods,
        path_regex,
        body_regex,
        headers_text,
    })
}

fn render_python_edit(name: String, id: String, f: &minos_config::FilterInstanceCfg) -> Response {
    let script = f
        .config
        .get("script")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let requirements = f
        .config
        .get("requirements")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let timeout_ms = f
        .config
        .get("timeout_ms")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(10);
    let fail_closed = f
        .config
        .get("fail_closed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    render(PythonEdit {
        name,
        id,
        display_name: f.display_name.clone(),
        script,
        requirements,
        timeout_ms,
        fail_closed,
    })
}

fn render_generic_edit(
    name: String,
    id: String,
    kind: &str,
    f: &minos_config::FilterInstanceCfg,
) -> Response {
    let config_json = serde_json::to_string_pretty(&f.config).unwrap_or_else(|_| "{}".into());
    render(GenericEdit {
        name,
        id,
        kind: kind.to_string(),
        display_name: f.display_name.clone(),
        config_json,
    })
}

// ---------------------------------------------------------------------------
// POST /services/{name}/filters/{id}
// ---------------------------------------------------------------------------

/// Single POST handler for all filter kinds. Reads the form as a `HashMap`,
/// looks up the filter's kind from the draft, then dispatches to the
/// kind-specific helper.
///
/// # Errors
///
/// Returns [`WebError::BadRequest`] if `id` is invalid, or
/// [`WebError::NotFound`] if the service or filter is missing.
#[allow(clippy::implicit_hasher)]
pub async fn edit_post(
    State(state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, WebError> {
    let id_uuid =
        Uuid::parse_str(&id).map_err(|_| WebError::BadRequest(format!("invalid id {id}")))?;
    let kind = {
        let svc = draft_for(&state, &name)?;
        svc.pipeline
            .iter()
            .find(|f| f.id == id_uuid)
            .map(|f| f.kind.clone())
            .ok_or_else(|| WebError::NotFound(format!("filter {id}")))?
    };
    match kind.as_str() {
        "regex" => handle_regex_post(&state, &name, id_uuid, &form),
        "http" => handle_http_post(&state, &name, id_uuid, &form),
        "python_sidecar" => handle_python_post(&state, &name, id_uuid, &form),
        _ => handle_generic_post(&state, &name, id_uuid, &form),
    }
}

// ---------------------------------------------------------------------------
// Kind-specific POST helpers
// ---------------------------------------------------------------------------

fn handle_regex_post<S: BuildHasher>(
    state: &AppState,
    name: &str,
    id: Uuid,
    form: &HashMap<String, String, S>,
) -> Result<Response, WebError> {
    let display_name = form.get("display_name").cloned().unwrap_or_default();
    if display_name.trim().is_empty() {
        return Err(WebError::BadRequest("display_name required".into()));
    }
    let pattern = form.get("pattern").cloned().unwrap_or_default();
    let mut draft = draft_for(state, name)?;
    let f = draft
        .pipeline
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or_else(|| WebError::NotFound(format!("filter {id}")))?;
    f.display_name = display_name;
    f.config = serde_json::json!({ "pattern": pattern });
    state.drafts.put(name, draft);
    Ok(redirect_to(&format!("/services/{name}")))
}

fn handle_http_post<S: BuildHasher>(
    state: &AppState,
    name: &str,
    id: Uuid,
    form: &HashMap<String, String, S>,
) -> Result<Response, WebError> {
    let display_name = form.get("display_name").cloned().unwrap_or_default();
    if display_name.trim().is_empty() {
        return Err(WebError::BadRequest("display_name required".into()));
    }
    let methods: Vec<String> = form
        .get("methods")
        .map_or("", String::as_str)
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    let headers: Vec<serde_json::Value> = form
        .get("headers_text")
        .map_or("", String::as_str)
        .lines()
        .filter_map(|line| {
            let (n, r) = line.split_once(':')?;
            let n = n.trim();
            let r = r.trim();
            if n.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "name": n,
                "value_regex": r,
            }))
        })
        .collect();
    let path_regex = form.get("path_regex").cloned().unwrap_or_default();
    let body_regex = form.get("body_regex").cloned().unwrap_or_default();

    let mut cfg = serde_json::Map::new();
    if !methods.is_empty() {
        cfg.insert("methods".into(), serde_json::json!(methods));
    }
    if !path_regex.is_empty() {
        cfg.insert("path_regex".into(), serde_json::json!(path_regex));
    }
    if !body_regex.is_empty() {
        cfg.insert("body_regex".into(), serde_json::json!(body_regex));
    }
    if !headers.is_empty() {
        cfg.insert("headers".into(), serde_json::json!(headers));
    }

    let mut draft = draft_for(state, name)?;
    let f = draft
        .pipeline
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or_else(|| WebError::NotFound(format!("filter {id}")))?;
    f.display_name = display_name;
    f.config = serde_json::Value::Object(cfg);
    state.drafts.put(name, draft);
    Ok(redirect_to(&format!("/services/{name}")))
}

fn handle_python_post<S: BuildHasher>(
    state: &AppState,
    name: &str,
    id: Uuid,
    form: &HashMap<String, String, S>,
) -> Result<Response, WebError> {
    let display_name = form.get("display_name").cloned().unwrap_or_default();
    if display_name.trim().is_empty() {
        return Err(WebError::BadRequest("display_name required".into()));
    }
    let script = form.get("script").cloned().unwrap_or_default();
    let requirements = form.get("requirements").cloned().unwrap_or_default();
    let timeout_ms: u64 = form
        .get("timeout_ms")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let fail_closed = form.get("fail_mode").is_some_and(|s| s == "closed");

    let mut draft = draft_for(state, name)?;
    let f = draft
        .pipeline
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or_else(|| WebError::NotFound(format!("filter {id}")))?;
    f.display_name = display_name;
    f.config = serde_json::json!({
        "service_name": name,
        "script": script,
        "requirements": requirements,
        "timeout_ms": timeout_ms,
        "fail_closed": fail_closed,
    });
    state.drafts.put(name, draft);
    Ok(redirect_to(&format!("/services/{name}")))
}

fn handle_generic_post<S: BuildHasher>(
    state: &AppState,
    name: &str,
    id: Uuid,
    form: &HashMap<String, String, S>,
) -> Result<Response, WebError> {
    let display_name = form.get("display_name").cloned().unwrap_or_default();
    if display_name.trim().is_empty() {
        return Err(WebError::BadRequest("display_name required".into()));
    }
    let config_json = form
        .get("config_json")
        .cloned()
        .unwrap_or_else(|| "{}".into());
    let config: serde_json::Value = serde_json::from_str(&config_json)
        .map_err(|e| WebError::BadRequest(format!("invalid JSON config: {e}")))?;

    let mut draft = draft_for(state, name)?;
    let f = draft
        .pipeline
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or_else(|| WebError::NotFound(format!("filter {id}")))?;
    f.display_name = display_name;
    f.config = config;
    state.drafts.put(name, draft);
    Ok(redirect_to(&format!("/services/{name}")))
}
