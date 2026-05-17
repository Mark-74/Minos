//! Confirms `register_builtin_filters` registers all three kinds and they
//! build through the registry on real JSON configs.

use minos_core::FilterRegistry;
use minos_filters::register_builtin_filters;

#[test]
fn registry_builds_regex_and_http_kinds() {
    let mut r = FilterRegistry::new();
    register_builtin_filters(&mut r);

    let regex_cfg = serde_json::json!({"pattern": "evil"});
    let f = r.build("regex", regex_cfg).expect("regex builds");
    assert_eq!(f.kind(), "regex");

    let http_cfg = serde_json::json!({"methods": ["POST"]});
    let f = r.build("http", http_cfg).expect("http builds");
    assert_eq!(f.kind(), "http");
    // python_sidecar is exercised by its own uv-gated test
    // (tests/python_sidecar_kind.rs).
}
