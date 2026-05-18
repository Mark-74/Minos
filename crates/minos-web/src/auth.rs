//! Argon2id password hash/verify + auth-cookie middleware.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::WebError;

/// Cookie name used for the signed session cookie.
pub const SESSION_COOKIE: &str = "minos_session";

/// Cookie value indicating the holder is authenticated. Single-tenant —
/// no user id — so the value is just a constant marker. The signature on
/// the cookie provides authenticity.
pub const SESSION_VALUE: &str = "ok";

/// Hash `password` with Argon2id and a random salt. Returns the PHC
/// string suitable for storing in the `settings` table.
///
/// # Errors
///
/// Returns [`WebError::Internal`] if the hasher rejects the input
/// (effectively unreachable for printable passwords).
pub fn hash_password(password: &str) -> Result<String, WebError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| WebError::Internal(format!("argon2 hash: {e}")))
}

/// Verify `password` against `hash`.
///
/// # Errors
///
/// Returns [`WebError::Internal`] if `hash` isn't a valid PHC string.
/// Returns `Ok(false)` on mismatch — that's not an error.
pub fn verify_password(password: &str, hash: &str) -> Result<bool, WebError> {
    let parsed =
        PasswordHash::new(hash).map_err(|e| WebError::Internal(format!("argon2 parse: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::SignedCookieJar;

use crate::AppState;

/// Key in the `settings` table that holds the Argon2 password hash.
pub const PASSWORD_HASH_KEY: &str = "password_hash";

/// Middleware: redirect to `/login` if no valid signed session cookie is
/// present. Pass through otherwise.
pub async fn require_auth(
    State(_state): State<AppState>,
    jar: SignedCookieJar,
    req: Request,
    next: Next,
) -> Response {
    let authed = jar
        .get(SESSION_COOKIE)
        .is_some_and(|c| c.value() == SESSION_VALUE);
    if !authed {
        return Redirect::to("/login").into_response();
    }
    next.run(req).await
}

/// Read the current password hash from storage. Returns `None` on first
/// run (no key yet).
///
/// # Errors
///
/// Returns [`WebError::Storage`] on backend failures.
pub fn current_password_hash(state: &AppState) -> Result<Option<String>, WebError> {
    Ok(state.storage.get_setting(PASSWORD_HASH_KEY)?)
}

/// Set the password hash in storage.
///
/// # Errors
///
/// Returns [`WebError::Storage`] on backend failures.
pub fn set_password_hash(state: &AppState, hash: &str) -> Result<(), WebError> {
    state.storage.set_setting(PASSWORD_HASH_KEY, hash)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_round_trip() {
        let h = hash_password("hunter2").unwrap();
        assert!(verify_password("hunter2", &h).unwrap());
        assert!(!verify_password("wrong", &h).unwrap());
    }

    #[test]
    fn hash_is_unique_per_call_due_to_salt() {
        let a = hash_password("x").unwrap();
        let b = hash_password("x").unwrap();
        assert_ne!(a, b);
        assert!(verify_password("x", &a).unwrap());
        assert!(verify_password("x", &b).unwrap());
    }

    #[test]
    fn verify_rejects_malformed_hash() {
        assert!(verify_password("x", "not a hash").is_err());
    }
}
