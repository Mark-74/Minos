//! `/settings` routes.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use minos_core::BlockResponse;
use serde::Deserialize;

use crate::auth::{current_password_hash, hash_password, set_password_hash, verify_password};
use crate::{AppState, WebError};

const DEFAULT_BLOCK_RESPONSE_KEY: &str = "default_block_response_json";

#[derive(Template)]
#[template(path = "settings.html")]
struct Settings {
    ok: Option<String>,
    error: Option<String>,
    status: u16,
    body: String,
    headers_text: String,
}

/// `GET /settings`.
///
/// # Errors
///
/// Returns [`WebError::Storage`] if the persisted defaults cannot be read.
pub async fn get(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String, std::collections::hash_map::RandomState>>,
) -> Result<Response, WebError> {
    let raw = state.storage.get_setting(DEFAULT_BLOCK_RESPONSE_KEY)?;
    let resp: BlockResponse = match raw {
        Some(s) => serde_json::from_str(&s).unwrap_or_default(),
        None => BlockResponse::default(),
    };
    let (status, body, headers_text) = match resp {
        BlockResponse::HttpStatus {
            status,
            body,
            headers,
        } => {
            let body_str = String::from_utf8_lossy(&body).to_string();
            let headers_text = headers
                .iter()
                .map(|(n, v)| format!("{n}: {v}"))
                .collect::<Vec<_>>()
                .join("\n");
            (status, body_str, headers_text)
        }
        BlockResponse::Close => (403, String::new(), String::new()),
    };
    Ok(render(Settings {
        ok: q.get("ok").cloned(),
        error: q.get("error").cloned(),
        status,
        body,
        headers_text,
    }))
}

/// Form fields for `POST /settings/password`.
#[derive(Deserialize)]
pub struct PasswordForm {
    current: String,
    new: String,
    confirm: String,
}

/// `POST /settings/password`.
///
/// # Errors
///
/// Returns [`WebError::Storage`] or [`WebError::Internal`] on backend
/// failures. Never returns an Err for an invalid form — those redirect to
/// `/settings?error=...`.
pub async fn password(
    State(state): State<AppState>,
    Form(form): Form<PasswordForm>,
) -> Result<Response, WebError> {
    if form.new.is_empty() {
        return Ok(redirect("/settings?error=new+password+required"));
    }
    if form.new != form.confirm {
        return Ok(redirect("/settings?error=new+and+confirm+do+not+match"));
    }
    let stored = current_password_hash(&state)?
        .ok_or_else(|| WebError::Internal("no password set".into()))?;
    if !verify_password(&form.current, &stored)? {
        return Ok(redirect("/settings?error=current+password+wrong"));
    }
    let h = hash_password(&form.new)?;
    set_password_hash(&state, &h)?;
    Ok(redirect("/settings?ok=password"))
}

/// Form fields for `POST /settings/defaults`.
#[derive(Deserialize)]
pub struct DefaultsForm {
    status: u16,
    body: String,
    headers_text: String,
}

/// `POST /settings/defaults`.
///
/// # Errors
///
/// Returns [`WebError::Storage`] on persistence failure.
pub async fn defaults(
    State(state): State<AppState>,
    Form(form): Form<DefaultsForm>,
) -> Result<Response, WebError> {
    let headers: Vec<(String, String)> = form
        .headers_text
        .lines()
        .filter_map(|line| {
            let (n, v) = line.split_once(':')?;
            Some((n.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    let resp = BlockResponse::HttpStatus {
        status: form.status,
        body: form.body.into_bytes(),
        headers,
    };
    let json = serde_json::to_string(&resp)
        .map_err(|e| WebError::Internal(format!("serialize defaults: {e}")))?;
    state
        .storage
        .set_setting(DEFAULT_BLOCK_RESPONSE_KEY, &json)?;
    Ok(redirect("/settings?ok=defaults"))
}

fn redirect(to: &str) -> Response {
    Redirect::to(to).into_response()
}

fn render<T: Template>(t: T) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
