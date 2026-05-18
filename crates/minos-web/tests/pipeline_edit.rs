//! Service detail view + edit actions (toggle, reorder, delete, save, discard).
//! Task 9 added the read-only GET assertions; Task 10 adds the mutation tests.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::Key;
use http_body_util::BodyExt;
use minos_config::{new_bus, Config, FilterInstanceCfg, RuleSet, ServiceConfig};
use minos_core::{FilterRegistry, ProtocolKind, ProxyMode};
use minos_filters::register_builtin_filters;
use minos_storage::{InMemoryStorage, Storage};
use minos_web::{routes::router, AppState};
use tower::ServiceExt;
use uuid::Uuid;

/// Build an `AppState` with the given services and the regex filter kind
/// registered so that `save_config` can validate pipelines that contain it.
fn state_with(services: Vec<ServiceConfig>) -> AppState {
    let cfg = Config {
        services,
        ..Config::default()
    };
    let (bus, _rx) = new_bus(RuleSet::empty_for(cfg));
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let mut registry = FilterRegistry::new();
    register_builtin_filters(&mut registry);
    AppState::new(bus, storage, Arc::new(registry), Key::generate())
}

/// Minimal service with an empty pipeline.
fn svc(name: &str) -> ServiceConfig {
    ServiceConfig {
        name: name.into(),
        mode: ProxyMode::Reverse {
            bind: "127.0.0.1:8080".parse().unwrap(),
            upstream: "127.0.0.1:5000".parse().unwrap(),
        },
        protocol: ProtocolKind::Http,
        pipeline: vec![],
        block_response_override: None,
        max_body_bytes: 1024,
    }
}

/// Build a `FilterInstanceCfg` with the `regex` kind and a trivial pattern.
fn regex_filter(display_name: &str) -> FilterInstanceCfg {
    FilterInstanceCfg {
        id: Uuid::new_v4(),
        display_name: display_name.into(),
        kind: "regex".into(),
        config: serde_json::json!({ "pattern": "x" }),
        enabled: true,
        dry_run: false,
        on_inbound: true,
        on_outbound: false,
    }
}

/// A service with a single regex filter in its pipeline.
fn svc_with_one_filter(name: &str) -> ServiceConfig {
    let mut s = svc(name);
    s.pipeline.push(regex_filter("filter-a"));
    s
}

/// A service with two regex filters in its pipeline.
fn svc_with_two_filters(name: &str) -> ServiceConfig {
    let mut s = svc(name);
    s.pipeline.push(regex_filter("filter-a"));
    s.pipeline.push(regex_filter("filter-b"));
    s
}

async fn login_cookie(app: &axum::Router) -> String {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("password=p"))
                .unwrap(),
        )
        .await
        .unwrap();
    res.headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn service_detail_renders_for_known_service() {
    let app = router(state_with(vec![svc("svc")]));
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/services/svc")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let s = std::str::from_utf8(&body).unwrap();
    assert!(s.contains("svc"));
    assert!(s.contains("Pipeline"));
}

#[tokio::test]
async fn service_detail_returns_404_for_unknown_service() {
    let app = router(state_with(vec![svc("svc")]));
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/services/missing")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Task 10: mutation tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn toggle_flips_enabled_in_draft() {
    let service = svc_with_one_filter("svc");
    let filter_id = service.pipeline[0].id;
    let state = state_with(vec![service]);
    let app = router(state.clone());
    let cookie = login_cookie(&app).await;

    let body = "field=enabled".to_string();
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/services/svc/filters/{filter_id}/toggle"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    // Draft must exist and the toggle must have flipped `enabled` to false.
    let draft = state.drafts.get("svc").expect("draft should exist");
    let f = draft.pipeline.iter().find(|f| f.id == filter_id).unwrap();
    assert!(
        !f.enabled,
        "enabled should have been flipped from true to false"
    );
}

#[tokio::test]
async fn reorder_swaps_filters() {
    let service = svc_with_two_filters("svc");
    let first_id = service.pipeline[0].id;
    let second_id = service.pipeline[1].id;
    let state = state_with(vec![service]);
    let app = router(state.clone());
    let cookie = login_cookie(&app).await;

    // Move the first filter down — the two should be swapped.
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/services/svc/filters/{first_id}/down"))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    let draft = state.drafts.get("svc").expect("draft should exist");
    assert_eq!(
        draft.pipeline[0].id, second_id,
        "second filter should now be first"
    );
    assert_eq!(
        draft.pipeline[1].id, first_id,
        "first filter should now be second"
    );
}

#[tokio::test]
async fn delete_removes_filter_from_draft() {
    let service = svc_with_one_filter("svc");
    let filter_id = service.pipeline[0].id;
    let state = state_with(vec![service]);
    let app = router(state.clone());
    let cookie = login_cookie(&app).await;

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/services/svc/filters/{filter_id}/delete"))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    let draft = state
        .drafts
        .get("svc")
        .expect("draft should exist after delete");
    assert!(
        draft.pipeline.is_empty(),
        "pipeline should be empty after deletion"
    );
}

#[tokio::test]
async fn discard_clears_draft() {
    let service = svc_with_one_filter("svc");
    let state = state_with(vec![service.clone()]);
    // Pre-populate a draft so there is something to discard.
    state.drafts.put("svc", service);

    let app = router(state.clone());
    let cookie = login_cookie(&app).await;

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/services/svc/discard")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert!(
        state.drafts.get("svc").is_none(),
        "draft should be cleared after discard"
    );
}

#[tokio::test]
async fn save_persists_draft_and_clears_it() {
    let service = svc_with_one_filter("svc");
    let state = state_with(vec![service.clone()]);
    // Pre-populate a draft (delete the filter so the saved pipeline is empty).
    let mut draft = service.clone();
    draft.pipeline.clear();
    state.drafts.put("svc", draft);

    let app = router(state.clone());
    let cookie = login_cookie(&app).await;

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/services/svc/save")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    // Draft should be gone.
    assert!(
        state.drafts.get("svc").is_none(),
        "draft should be cleared after save"
    );

    // Storage should have exactly one version.
    let versions = state.storage.list_versions().unwrap();
    assert_eq!(versions.len(), 1, "one version should have been written");

    // The live ruleset should reflect the saved (empty) pipeline.
    let live = state.bus.rules.load();
    let pipeline = live.pipeline_for(0);
    assert!(pipeline.is_empty(), "saved pipeline should have no filters");
}
