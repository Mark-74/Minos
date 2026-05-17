//! Per-service sidecar supervisor: owns the Unix socket, spawns the
//! Python wrapper, and round-trips Request/Response with a per-call
//! timeout.
//!
//! Restart-on-exit with backoff is **not** in this phase — Phase 5 wires
//! it up when the binary glues the planes together. For Phase 3, if the
//! child dies, subsequent `call()`s fail-open to `Response::Pass`.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::sidecar::protocol::{read_response, write_request, Request, Response};
use crate::FilterError;

/// Per-service sidecar supervisor.
///
/// Owns the Unix socket, the spawned Python wrapper process, and the
/// connected socket. Clone via [`Arc`] to share across tasks.
pub struct SidecarSupervisor {
    socket_path: PathBuf,
    venv_python: PathBuf,
    wrapper_path: PathBuf,
    user_script_path: PathBuf,
    state: Mutex<Option<SupervisorState>>,
}

/// Internal state present after [`SidecarSupervisor::start`] has run successfully.
pub(crate) struct SupervisorState {
    /// The spawned child process. `None` only in tests that skip spawning.
    ///
    /// Kept alive so that dropping `SupervisorState` kills the child via
    /// `Child`'s `Drop` impl. The field is intentionally never read.
    #[allow(dead_code)]
    pub(crate) child: Option<Child>,
    /// The accepted Unix socket connection to the sidecar.
    pub(crate) conn: UnixStream,
    /// Monotonic counter for assigning request ids.
    pub(crate) next_request_id: u64,
}

impl SidecarSupervisor {
    /// Construct a supervisor.
    ///
    /// Does not bind the socket or spawn the child process — call
    /// [`start`](Self::start) to do that.
    #[must_use]
    pub fn new(
        socket_path: PathBuf,
        venv_python: PathBuf,
        wrapper_path: PathBuf,
        user_script_path: PathBuf,
    ) -> Self {
        Self {
            socket_path,
            venv_python,
            wrapper_path,
            user_script_path,
            state: Mutex::new(None),
        }
    }

    /// Bind the Unix socket, spawn the child sidecar process, and accept
    /// the child's connection-back.
    ///
    /// After this returns `Ok`, the supervisor is ready to handle
    /// [`call`](Self::call)s.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Io`] on socket-bind, process-spawn, or
    /// accept failures.
    pub async fn start(self: &Arc<Self>) -> Result<(), FilterError> {
        let _ = std::fs::remove_file(&self.socket_path);
        let listener = UnixListener::bind(&self.socket_path)?;
        let child = Command::new(&self.venv_python)
            .arg(&self.wrapper_path)
            .arg(&self.user_script_path)
            .arg(&self.socket_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let (conn, _) = listener.accept().await?;
        let mut state = self.state.lock().await;
        *state = Some(SupervisorState {
            child: Some(child),
            conn,
            next_request_id: 1,
        });
        Ok(())
    }

    /// Send `req` to the sidecar and await its response within `timeout`.
    ///
    /// On any error — supervisor not started, socket failure, timeout, or
    /// malformed response — returns `Response::Pass` (fail-open default
    /// per design §7.5).
    ///
    /// TODO(phase 5): on timeout / socket-error, mark the supervisor as
    /// degraded so the restart loop can swap in a fresh child.
    pub async fn call(&self, mut req: Request, timeout: Duration) -> Response {
        let mut guard = self.state.lock().await;
        let Some(state) = guard.as_mut() else {
            return Response::Pass { id: req.id };
        };
        req.id = state.next_request_id;
        state.next_request_id += 1;
        let fut = async {
            write_request(&mut state.conn, &req).await?;
            read_response(&mut state.conn).await
        };
        match tokio::time::timeout(timeout, fut).await {
            Ok(Ok(resp)) => resp,
            _ => Response::Pass { id: req.id },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::protocol::{read_request, write_response, Request, Response};
    use std::sync::Arc;
    use std::time::Duration;

    /// A tokio task that pretends to be the Python sidecar: connects to
    /// `socket_path`, then loops echoing Pass for every Request except
    /// those whose `bytes_b64` field contains "RFJPUA" (base64 of "DROP"),
    /// for which it returns Block.
    async fn fake_sidecar(socket_path: std::path::PathBuf) {
        // The supervisor will call `listener.accept()`; we are the connector.
        let mut conn = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        loop {
            let Ok(req) = read_request(&mut conn).await else {
                return;
            };
            let resp = if req.bytes_b64.contains("RFJPUA") {
                Response::Block {
                    id: req.id,
                    reason: "matched".into(),
                }
            } else {
                Response::Pass { id: req.id }
            };
            if write_response(&mut conn, &resp).await.is_err() {
                return;
            }
        }
    }

    /// A supervisor that skips the real-process spawn and just expects
    /// somebody else to connect to its socket. Useful for the in-process test.
    impl SidecarSupervisor {
        async fn start_without_spawning_for_test(self: &Arc<Self>) -> Result<(), FilterError> {
            let _ = std::fs::remove_file(&self.socket_path);
            let listener = tokio::net::UnixListener::bind(&self.socket_path)?;
            let (conn, _) = listener.accept().await?;
            let mut state = self.state.lock().await;
            *state = Some(SupervisorState {
                child: None,
                conn,
                next_request_id: 1,
            });
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn supervisor_round_trips_pass_and_block() {
        let tmp = tempfile::tempdir().unwrap();
        let sock = tmp.path().join("svc.sock");

        // No real Python: pass placeholder paths.
        let s = Arc::new(SidecarSupervisor::new(
            sock.clone(),
            "/dev/null".into(),
            "/dev/null".into(),
            "/dev/null".into(),
        ));

        // Spawn the fake first — it will connect once the listener binds.
        let s_clone = Arc::clone(&s);
        let server_handle = tokio::spawn(async move {
            s_clone.start_without_spawning_for_test().await.unwrap();
        });
        let sock_for_fake = sock.clone();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let fake = tokio::spawn(async move { fake_sidecar(sock_for_fake).await });
        server_handle.await.unwrap();

        // Clean request → Pass.
        let req = Request {
            id: 0,
            direction: "inbound".into(),
            kind: "raw".into(),
            bytes_b64: "aGVsbG8=".into(), // base64("hello")
            http: None,
        };
        let resp = s.call(req, Duration::from_millis(500)).await;
        assert!(matches!(resp, Response::Pass { .. }), "got {resp:?}");

        // Bad request → Block.
        let req = Request {
            id: 0,
            direction: "inbound".into(),
            kind: "raw".into(),
            bytes_b64: "RFJPUA==".into(), // base64("DROP")
            http: None,
        };
        let resp = s.call(req, Duration::from_millis(500)).await;
        let Response::Block { reason, .. } = resp else {
            panic!("expected Block, got {resp:?}");
        };
        assert_eq!(reason, "matched");

        fake.abort();
    }

    #[tokio::test]
    async fn supervisor_returns_pass_before_start() {
        // call() on an un-started supervisor must return Pass (fail-open).
        let s = Arc::new(SidecarSupervisor::new(
            std::path::PathBuf::from("/tmp/never-exists.sock"),
            "/dev/null".into(),
            "/dev/null".into(),
            "/dev/null".into(),
        ));
        let req = Request {
            id: 7,
            direction: "inbound".into(),
            kind: "raw".into(),
            bytes_b64: String::new(),
            http: None,
        };
        let resp = s.call(req, Duration::from_millis(50)).await;
        assert!(matches!(resp, Response::Pass { id: _ }));
    }
}
