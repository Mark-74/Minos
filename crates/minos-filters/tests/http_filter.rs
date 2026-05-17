//! Verifies `HttpFilter` builds correctly through the `FilterRegistry`.

use minos_core::FilterRegistry;
use minos_filters::HttpKind;

#[test]
fn registry_builds_http_filter() {
    let mut r = FilterRegistry::new();
    r.register::<HttpKind>();
    let cfg = serde_json::json!({
        "methods": ["POST"],
        "path_regex": "/api/.*",
        "body_regex": "DROP"
    });
    let f = r.build("http", cfg).expect("build");
    assert_eq!(f.kind(), "http");
}

#[test]
fn registry_rejects_invalid_path_regex() {
    let mut r = FilterRegistry::new();
    r.register::<HttpKind>();
    let cfg = serde_json::json!({"path_regex": "(unbalanced"});
    let err = r.build("http", cfg).err().unwrap();
    assert!(matches!(err, minos_core::BuildError::Invalid { .. }));
}
