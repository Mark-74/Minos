//! Integration test for `original_dst`. Linux-only — on other platforms the
//! function unconditionally returns an error and there is nothing
//! interesting to assert.

#![cfg(target_os = "linux")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[tokio::test]
async fn lookup_on_unredirected_connection_returns_local_addr() {
    // Without an iptables redirect, SO_ORIGINAL_DST on an accepted socket
    // returns the local bind address — that's the kernel's "no NAT
    // translation applied" answer.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listen_addr = listener.local_addr().unwrap();
    let saw_match = Arc::new(AtomicBool::new(false));
    let flag = saw_match.clone();
    let handle = tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        let dst = minos_proxy::original_dst(&s).expect("lookup");
        if dst == listen_addr {
            flag.store(true, Ordering::SeqCst);
        }
    });
    let _ = tokio::net::TcpStream::connect(listen_addr).await.unwrap();
    handle.await.unwrap();
    assert!(
        saw_match.load(Ordering::SeqCst),
        "expected SO_ORIGINAL_DST == {listen_addr}"
    );
}
