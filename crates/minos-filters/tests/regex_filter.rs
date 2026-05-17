//! Verifies `RegexFilter` builds correctly through the `FilterRegistry`.

use minos_core::FilterRegistry;
use minos_filters::RegexKind;

#[test]
fn registry_builds_regex_filter() {
    let mut r = FilterRegistry::new();
    r.register::<RegexKind>();
    let cfg = serde_json::json!({ "pattern": "evil" });
    let f = r.build("regex", cfg).expect("build");
    assert_eq!(f.kind(), "regex");
}

#[test]
fn registry_rejects_invalid_regex() {
    let mut r = FilterRegistry::new();
    r.register::<RegexKind>();
    let cfg = serde_json::json!({ "pattern": "(unbalanced" });
    let err = r.build("regex", cfg).err().unwrap();
    assert!(matches!(err, minos_core::BuildError::Invalid { .. }));
}
