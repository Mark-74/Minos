//! End-to-end Python sidecar integration test. Requires `uv` on PATH;
//! soft-skips otherwise.

use std::sync::Arc;
use std::time::Duration;

use minos_filters::sidecar::protocol::{Request, Response};
use minos_filters::sidecar::supervisor::SidecarSupervisor;
use minos_filters::sidecar::venv;
use minos_filters::sidecar::wrapper::WRAPPER_SOURCE;
use tempfile::tempdir;

fn uv_available() -> bool {
    std::process::Command::new("uv")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[tokio::test(flavor = "multi_thread")]
async fn sidecar_blocks_bad_request_and_passes_clean_one() {
    if !uv_available() {
        eprintln!("uv not on PATH, skipping sidecar_round_trip test");
        return;
    }

    let tmp = tempdir().unwrap();
    let venv_root = tmp.path().join("venvs");
    let outcome = venv::install(&venv_root, b"").expect("venv install");
    let py = outcome.path.join("bin").join("python");

    let wrapper_path = tmp.path().join("wrapper.py");
    std::fs::write(&wrapper_path, WRAPPER_SOURCE).unwrap();

    let user_script_path = tmp.path().join("user.py");
    std::fs::write(
        &user_script_path,
        "def filter(packet):\n    if b\"BAD\" in packet[\"bytes\"]:\n        return {\"verdict\": \"block\", \"reason\": \"saw BAD\"}\n    return None\n",
    )
    .unwrap();

    let socket_path = tmp.path().join("svc.sock");
    let s = Arc::new(SidecarSupervisor::new(
        socket_path,
        py,
        wrapper_path,
        user_script_path,
    ));
    s.start().await.expect("supervisor start");

    // Clean request.
    let clean = Request {
        id: 0,
        direction: "inbound".into(),
        kind: "raw".into(),
        bytes_b64: base64_encode(b"hello world"),
        http: None,
    };
    let v = s.call(clean, Duration::from_secs(2)).await;
    assert!(matches!(v, Response::Pass { .. }), "got {v:?}");

    // Bad request.
    let bad = Request {
        id: 0,
        direction: "inbound".into(),
        kind: "raw".into(),
        bytes_b64: base64_encode(b"hello BAD bytes"),
        http: None,
    };
    let v = s.call(bad, Duration::from_secs(2)).await;
    let Response::Block { reason, .. } = v else {
        panic!("expected Block, got {v:?}");
    };
    assert_eq!(reason, "saw BAD");
}

/// Tiny inline base64 encoder so we don't pull in another dep.
fn base64_encode(b: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for chunk in b.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[((b0 & 0b11) << 4 | b1 >> 4) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((b1 & 0b1111) << 2 | b2 >> 6) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(b2 & 0b11_1111) as usize] as char
        } else {
            '='
        });
    }
    out
}
