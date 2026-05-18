//! Embedded static assets (CSS, JS).
//!
//! Bundled at compile time via [`rust_embed`] so the binary is single-file.

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "src/assets"]
struct Assets;

/// Serve an embedded asset at `/assets/{*path}`. Returns 404 if not found.
pub async fn handler(Path(path): Path<String>) -> Response {
    if let Some(content) = Assets::get(&path) {
        let mime = mime_guess::from_path(&path).first_or_octet_stream();
        let mut resp = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(header::CACHE_CONTROL, "public, max-age=3600")
            .body(Body::from(content.data.into_owned()))
            .expect("asset response build");
        // Strip null Content-Length added by some build paths.
        let _ = resp.headers_mut().remove("transfer-encoding");
        return resp;
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}
