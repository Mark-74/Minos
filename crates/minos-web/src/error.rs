//! Error type for web handlers.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use minos_config::ConfigError;
use minos_storage::StorageError;
use thiserror::Error;

/// Errors raised by web handlers. All variants `IntoResponse` directly so
/// handlers can use `?`.
#[derive(Debug, Error)]
pub enum WebError {
    /// Resource (service, filter, version) not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// Caller lacks a valid auth cookie.
    #[error("unauthorized")]
    Unauthorized,
    /// Submitted form failed validation.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Save flow rejected a new config (regex didn't compile, etc.).
    #[error("config: {0}")]
    Config(#[from] ConfigError),
    /// Underlying storage failure.
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    /// Catch-all internal error.
    #[error("internal: {0}")]
    Internal(String),
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let status = match &self {
            WebError::NotFound(_) => StatusCode::NOT_FOUND,
            WebError::Unauthorized => StatusCode::UNAUTHORIZED,
            WebError::BadRequest(_) | WebError::Config(_) => StatusCode::BAD_REQUEST,
            WebError::Storage(_) | WebError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn not_found_renders_404() {
        let r = WebError::NotFound("service xyz".into()).into_response();
        assert_eq!(r.status(), 404);
        let body = r.into_body().collect().await.unwrap().to_bytes();
        let s = std::str::from_utf8(&body).unwrap();
        assert!(s.contains("xyz"));
    }

    #[tokio::test]
    async fn unauthorized_renders_401() {
        let r = WebError::Unauthorized.into_response();
        assert_eq!(r.status(), 401);
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WebError>();
    }
}
