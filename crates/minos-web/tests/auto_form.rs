//! Schema-driven generic filter editor: a registered kind with a schema gets
//! typed form fields, and the POST coerces them back to a typed JSON config.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::Key;
use http_body_util::BodyExt;
use minos_config::{new_bus, Config, FilterInstanceCfg, RuleSet, ServiceConfig};
use minos_core::{
    BuildError, Filter, FilterKind, FilterRegistry, Packet, ProtocolKind, ProxyMode, Verdict,
};
use minos_storage::{InMemoryStorage, Storage};
use minos_web::{routes::router, AppState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;
use uuid::Uuid;

// A third-party-style filter kind with a typed config schema.
#[derive(Serialize, Deserialize, JsonSchema)]
struct CustomCfg {
    enabled: bool,
    threshold: u32,
    label: String,
}

struct Custom;
impl Filter for Custom {
    fn kind(&self) -> &'static str {
        "custom"
    }
    fn accepts(&self, _: &Packet) -> bool {
        true
    }
    fn inspect(&self, _: &Packet) -> Verdict {
        Verdict::Pass
    }
}

struct CustomKind;
impl FilterKind for CustomKind {
    const NAME: &'static str = "custom";
    type Config = CustomCfg;
    fn build(_cfg: CustomCfg) -> Result<Arc<dyn Filter>, BuildError> {
        Ok(Arc::new(Custom))
    }
}

fn state_with_custom_filter(id: Uuid) -> AppState {
    let svc = ServiceConfig {
        name: "svc".into(),
        mode: ProxyMode::Reverse {
            bind: "127.0.0.1:8080".parse().unwrap(),
            upstream: "127.0.0.1:5000".parse().unwrap(),
        },
        protocol: ProtocolKind::Http,
        pipeline: vec![FilterInstanceCfg {
            id,
            display_name: "c".into(),
            kind: "custom".into(),
            config: serde_json::json!({ "enabled": true, "threshold": 3, "label": "hi" }),
            enabled: true,
            dry_run: true,
            on_inbound: true,
            on_outbound: false,
        }],
        block_response_override: None,
        max_body_bytes: 1024,
    };
    let cfg = Config {
        services: vec![svc],
        ..Config::default()
    };
    let (bus, _rx) = new_bus(RuleSet::empty_for(cfg));
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let mut registry = FilterRegistry::new();
    registry.register::<CustomKind>();
    let (broadcast_tx, _sub) = tokio::sync::broadcast::channel(16);
    AppState::new(
        bus,
        storage,
        Arc::new(registry),
        Key::generate(),
        Arc::new(broadcast_tx),
    )
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
async fn generic_editor_renders_schema_form() {
    let id = Uuid::new_v4();
    let app = router(state_with_custom_filter(id));
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/services/svc/filters/{id}"))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let s = std::str::from_utf8(&body).unwrap();
    assert!(s.contains("type=\"checkbox\""), "bool → checkbox");
    assert!(s.contains("name=\"config.enabled\""));
    assert!(s.contains("type=\"number\""), "u32 → number");
    assert!(s.contains("name=\"config.threshold\""));
    assert!(s.contains("name=\"config.label\""));
    // No raw JSON textarea fallback when a schema is present.
    assert!(!s.contains("name=\"config_json\""));
}

#[tokio::test]
async fn generic_editor_post_coerces_typed_config() {
    let id = Uuid::new_v4();
    let state = state_with_custom_filter(id);
    let app = router(state.clone());
    let cookie = login_cookie(&app).await;
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/services/svc/filters/{id}"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "display_name=c&config.enabled=true&config.threshold=9&config.label=hey",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    let draft = state.drafts.get("svc").expect("draft");
    let cfg = &draft.pipeline[0].config;
    assert_eq!(cfg["enabled"], serde_json::json!(true));
    assert_eq!(cfg["threshold"], serde_json::json!(9));
    assert_eq!(cfg["label"], serde_json::json!("hey"));
}
