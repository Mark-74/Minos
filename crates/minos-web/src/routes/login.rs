//! `/login`, `/logout` route handlers.

use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::{Cookie, SameSite, SignedCookieJar};
use serde::Deserialize;

use crate::auth::{
    current_password_hash, hash_password, set_password_hash, verify_password, SESSION_COOKIE,
    SESSION_VALUE,
};
use crate::{AppState, WebError};

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: Option<String>,
    first_run: bool,
}

impl IntoResponse for LoginTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => axum::response::Html(html).into_response(),
            Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

/// `GET /login` — render the login form.
///
/// # Errors
///
/// Returns [`WebError::Storage`] if the settings table is unavailable.
pub async fn get(State(state): State<AppState>) -> Result<Response, WebError> {
    let first_run = current_password_hash(&state)?.is_none();
    Ok(LoginTemplate {
        error: None,
        first_run,
    }
    .into_response())
}

/// Form body for `POST /login`.
#[derive(Deserialize)]
pub struct LoginForm {
    password: String,
}

/// `POST /login` — verify password, set session cookie, redirect to `/`.
///
/// On first run (no stored hash) the submitted password becomes the shared
/// password. On subsequent runs the submitted password is verified against
/// the stored Argon2 hash. On success a signed session cookie is set and
/// the browser is redirected to `/`. On failure the login form is
/// re-rendered with an error message.
///
/// # Errors
///
/// Returns [`WebError::Storage`] or [`WebError::Internal`] on backend
/// failures.
pub async fn post(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Form(form): Form<LoginForm>,
) -> Result<Response, WebError> {
    if form.password.is_empty() {
        let first_run = current_password_hash(&state)?.is_none();
        return Ok(LoginTemplate {
            error: Some("password required".into()),
            first_run,
        }
        .into_response());
    }
    let stored = current_password_hash(&state)?;
    let accept = if let Some(ref h) = stored {
        verify_password(&form.password, h)?
    } else {
        let h = hash_password(&form.password)?;
        set_password_hash(&state, &h)?;
        true
    };
    if !accept {
        return Ok(LoginTemplate {
            error: Some("wrong password".into()),
            first_run: false,
        }
        .into_response());
    }
    let cookie = Cookie::build((SESSION_COOKIE, SESSION_VALUE))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .build();
    let jar = jar.add(cookie);
    Ok((jar, Redirect::to("/")).into_response())
}

/// `POST /logout` — remove session cookie, redirect to `/login`.
pub async fn logout(jar: SignedCookieJar) -> Response {
    let jar = jar.remove(Cookie::from(SESSION_COOKIE));
    (jar, Redirect::to("/login")).into_response()
}
