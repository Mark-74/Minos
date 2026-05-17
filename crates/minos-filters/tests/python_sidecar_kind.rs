//! `PythonSidecarKind::build` integration test. Requires `uv` on PATH;
//! soft-skips otherwise.

use minos_core::FilterRegistry;
use minos_filters::PythonSidecarKind;
use tempfile::tempdir;

fn uv_available() -> bool {
    std::process::Command::new("uv")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn build_python_sidecar_filter_via_registry() {
    if !uv_available() {
        eprintln!("uv not on PATH, skipping python_sidecar_kind test");
        return;
    }

    // Point env vars at a tempdir so the test doesn't touch /var or /run.
    let tmp = tempdir().unwrap();
    let venv_root = tmp.path().join("venvs");
    let socket_dir = tmp.path().join("sidecars");
    // Integration tests run in separate processes (one per file), so
    // env-var writes are local to this test binary.
    std::env::set_var("MINOS_VENV_ROOT", &venv_root);
    std::env::set_var("MINOS_SOCKET_DIR", &socket_dir);

    let mut r = FilterRegistry::new();
    r.register::<PythonSidecarKind>();

    let cfg = serde_json::json!({
        "service_name": "test-svc",
        "script": "def filter(packet):\n    return None\n",
        "requirements": "",
        "timeout_ms": 100,
        "fail_closed": false,
    });
    let f = r.build("python_sidecar", cfg).expect("build");
    assert_eq!(f.kind(), "python_sidecar");

    // Confirm the staged user script exists.
    assert!(
        socket_dir.join("test-svc.py").exists(),
        "user script not staged"
    );
}
