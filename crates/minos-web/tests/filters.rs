//! Filter editor routes: new, edit (`regex` / `http` / `python_sidecar` / generic).
//! Tasks 11, 12, and 13.

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

// ---------------------------------------------------------------------------
// Helpers (mirrored from pipeline_edit.rs)
// ---------------------------------------------------------------------------

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

fn filter_instance(kind: &str, display_name: &str, config: serde_json::Value) -> FilterInstanceCfg {
    FilterInstanceCfg {
        id: Uuid::new_v4(),
        display_name: display_name.into(),
        kind: kind.into(),
        config,
        enabled: true,
        dry_run: false,
        on_inbound: true,
        on_outbound: false,
    }
}

fn svc_with_filter(name: &str, f: FilterInstanceCfg) -> ServiceConfig {
    let mut s = svc(name);
    s.pipeline.push(f);
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

async fn body_str(res: axum::response::Response) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ---------------------------------------------------------------------------
// Task 11: new filter + regex + http
// ---------------------------------------------------------------------------

#[tokio::test]
async fn new_get_renders_kind_dropdown() {
    let app = router(state_with(vec![svc("svc")]));
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/services/svc/filters/new")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_str(res).await;
    // At least one built-in kind must appear in the dropdown.
    assert!(body.contains("regex"), "dropdown should contain 'regex'");
}

#[tokio::test]
async fn new_post_adds_filter_to_draft_and_redirects() {
    let state = state_with(vec![svc("svc")]);
    let app = router(state.clone());
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/services/svc/filters/new")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("kind=regex&display_name=block-sqli"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    // Draft must contain the new filter.
    let draft = state.drafts.get("svc").expect("draft should exist");
    assert_eq!(draft.pipeline.len(), 1);
    let f = &draft.pipeline[0];
    assert_eq!(f.kind, "regex");
    assert_eq!(f.display_name, "block-sqli");
    assert!(f.dry_run, "new filters should default to dry_run=true");
    assert!(f.enabled);
    assert!(f.on_inbound);
    assert!(!f.on_outbound);

    // Redirect must point to the new filter's editor.
    let loc = res
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        loc.starts_with("/services/svc/filters/"),
        "redirect should point to the filter editor, got: {loc}"
    );
}

#[tokio::test]
async fn edit_get_regex_renders_form_with_pattern() {
    let f = filter_instance("regex", "sqli", serde_json::json!({"pattern": "SELECT"}));
    let fid = f.id;
    let state = state_with(vec![svc_with_filter("svc", f)]);
    let app = router(state);
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/services/svc/filters/{fid}"))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_str(res).await;
    assert!(
        body.contains("SELECT"),
        "form should contain the current pattern"
    );
    assert!(
        body.contains("sqli"),
        "form should contain the display name"
    );
}

#[tokio::test]
async fn edit_post_regex_updates_pattern_in_draft() {
    let f = filter_instance("regex", "old", serde_json::json!({"pattern": "OLD"}));
    let fid = f.id;
    let state = state_with(vec![svc_with_filter("svc", f)]);
    let app = router(state.clone());
    let cookie = login_cookie(&app).await;
    let body_data = "display_name=new-name&pattern=NEWPAT";
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/services/svc/filters/{fid}"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body_data))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    let draft = state.drafts.get("svc").expect("draft should exist");
    let updated = draft.pipeline.iter().find(|f| f.id == fid).unwrap();
    assert_eq!(updated.display_name, "new-name");
    assert_eq!(
        updated
            .config
            .get("pattern")
            .and_then(serde_json::Value::as_str),
        Some("NEWPAT")
    );
}

#[tokio::test]
async fn edit_get_http_renders_form_with_fields() {
    let f = filter_instance(
        "http",
        "http-filter",
        serde_json::json!({
            "methods": ["GET", "POST"],
            "path_regex": "/api/.*",
            "body_regex": "evil",
            "headers": [{"name": "X-Foo", "value_regex": "bar"}]
        }),
    );
    let fid = f.id;
    let state = state_with(vec![svc_with_filter("svc", f)]);
    let app = router(state);
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/services/svc/filters/{fid}"))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_str(res).await;
    assert!(body.contains("/api/.*"), "should render path_regex");
    assert!(body.contains("evil"), "should render body_regex");
    assert!(body.contains("X-Foo"), "should render header name");
}

#[tokio::test]
async fn edit_post_http_persists_all_fields() {
    let f = filter_instance("http", "http-filter", serde_json::json!({}));
    let fid = f.id;
    let state = state_with(vec![svc_with_filter("svc", f)]);
    let app = router(state.clone());
    let cookie = login_cookie(&app).await;
    let body_data =
        "display_name=updated&methods=GET%2C+POST&path_regex=%2Fapi&body_regex=evil&headers_text=X-Foo%3A+bar";
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/services/svc/filters/{fid}"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body_data))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    let draft = state.drafts.get("svc").expect("draft should exist");
    let updated = draft.pipeline.iter().find(|f| f.id == fid).unwrap();
    assert_eq!(updated.display_name, "updated");
    let methods = updated
        .config
        .get("methods")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(methods.iter().any(|m| m.as_str() == Some("GET")));
    assert!(methods.iter().any(|m| m.as_str() == Some("POST")));
    assert_eq!(
        updated.config.get("path_regex").and_then(|v| v.as_str()),
        Some("/api")
    );
    assert_eq!(
        updated.config.get("body_regex").and_then(|v| v.as_str()),
        Some("evil")
    );
    let headers = updated
        .config
        .get("headers")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(
        headers[0].get("name").and_then(|v| v.as_str()),
        Some("X-Foo")
    );
}

// ---------------------------------------------------------------------------
// Task 12: python_sidecar editor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_get_python_renders_form() {
    let f = filter_instance(
        "python_sidecar",
        "py-filter",
        serde_json::json!({
            "service_name": "svc",
            "script": "def run(pkt): return 'pass'",
            "requirements": "requests",
            "timeout_ms": 5000,
            "fail_closed": false
        }),
    );
    let fid = f.id;
    let state = state_with(vec![svc_with_filter("svc", f)]);
    let app = router(state);
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/services/svc/filters/{fid}"))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_str(res).await;
    assert!(body.contains("def run"), "should render the script");
    assert!(body.contains("requests"), "should render requirements");
    assert!(body.contains("5000"), "should render timeout_ms");
}

#[tokio::test]
async fn edit_post_python_persists_config() {
    let f = filter_instance("python_sidecar", "py-filter", serde_json::json!({}));
    let fid = f.id;
    let state = state_with(vec![svc_with_filter("svc", f)]);
    let app = router(state.clone());
    let cookie = login_cookie(&app).await;
    let body_data = "display_name=py-updated&script=def+run(p)%3A+return+'pass'&requirements=requests&timeout_ms=2000&fail_mode=open";
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/services/svc/filters/{fid}"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body_data))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    let draft = state.drafts.get("svc").expect("draft should exist");
    let updated = draft.pipeline.iter().find(|f| f.id == fid).unwrap();
    assert_eq!(updated.display_name, "py-updated");
    assert_eq!(
        updated.config.get("service_name").and_then(|v| v.as_str()),
        Some("svc"),
        "service_name must be auto-injected from the URL path"
    );
    assert_eq!(
        updated
            .config
            .get("timeout_ms")
            .and_then(serde_json::Value::as_u64),
        Some(2000)
    );
    assert_eq!(
        updated
            .config
            .get("fail_closed")
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
}

// ---------------------------------------------------------------------------
// Task 13: generic JSON-textarea editor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_get_generic_renders_json_textarea() {
    let f = filter_instance(
        "unknown_kind",
        "gen-filter",
        serde_json::json!({"foo": "bar"}),
    );
    let fid = f.id;
    let state = state_with(vec![svc_with_filter("svc", f)]);
    let app = router(state);
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/services/svc/filters/{fid}"))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_str(res).await;
    assert!(body.contains("unknown_kind"), "should show the kind name");
    assert!(
        body.contains("config_json"),
        "should have a config_json textarea"
    );
    assert!(
        body.contains("bar"),
        "should render the current config JSON"
    );
}

#[tokio::test]
async fn edit_post_generic_persists_json_config() {
    let f = filter_instance("unknown_kind", "gen-filter", serde_json::json!({"old": 1}));
    let fid = f.id;
    let state = state_with(vec![svc_with_filter("svc", f)]);
    let app = router(state.clone());
    let cookie = login_cookie(&app).await;
    // URL-encode: display_name=gen-updated&config_json={"new":2}
    let body_data = "display_name=gen-updated&config_json=%7B%22new%22%3A2%7D";
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/services/svc/filters/{fid}"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body_data))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    let draft = state.drafts.get("svc").expect("draft should exist");
    let updated = draft.pipeline.iter().find(|f| f.id == fid).unwrap();
    assert_eq!(updated.display_name, "gen-updated");
    assert_eq!(
        updated
            .config
            .get("new")
            .and_then(serde_json::Value::as_i64),
        Some(2)
    );
}

#[tokio::test]
async fn edit_post_generic_rejects_invalid_json() {
    let f = filter_instance("unknown_kind", "gen-filter", serde_json::json!({}));
    let fid = f.id;
    let state = state_with(vec![svc_with_filter("svc", f)]);
    let app = router(state);
    let cookie = login_cookie(&app).await;
    let body_data = "display_name=gen-filter&config_json=not+valid+json";
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/services/svc/filters/{fid}"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body_data))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
