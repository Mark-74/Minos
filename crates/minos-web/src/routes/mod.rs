//! HTTP route table.

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::Router;

use crate::auth::require_auth;
use crate::AppState;

pub mod dashboard;
pub mod filters;
pub mod history;
pub mod log;
pub mod login;
pub mod services;
pub mod settings;

/// Build the complete router.
///
/// Public routes (`/login`, `/logout`, `/assets/…`) are open. Protected
/// routes require a valid signed session cookie; unauthenticated requests are
/// redirected to `/login` by [`crate::auth::require_auth`].
pub fn router(state: AppState) -> Router {
    let public = Router::new()
        .route("/login", get(login::get).post(login::post))
        .route("/logout", post(login::logout))
        .route("/assets/{*path}", get(crate::assets::handler));

    let protected = Router::new()
        .route("/", get(dashboard::get))
        .route("/services/{name}", get(services::get))
        .route(
            "/services/{name}/filters/new",
            get(filters::new_get).post(filters::new_post),
        )
        .route(
            "/services/{name}/filters/{id}",
            get(filters::edit_get).post(filters::edit_post),
        )
        .route("/services/{name}/filters/{id}/up", post(services::up))
        .route("/services/{name}/filters/{id}/down", post(services::down))
        .route(
            "/services/{name}/filters/{id}/delete",
            post(services::delete),
        )
        .route(
            "/services/{name}/filters/{id}/toggle",
            post(services::toggle),
        )
        .route("/services/{name}/save", post(services::save))
        .route("/services/{name}/discard", post(services::discard))
        .route("/history", get(history::get))
        .route("/history/{version}/rollback", post(history::rollback))
        .route("/log", get(log::get))
        .route("/settings", get(settings::get))
        .route("/settings/password", post(settings::password))
        .route("/settings/defaults", post(settings::defaults))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    public.merge(protected).with_state(state)
}
