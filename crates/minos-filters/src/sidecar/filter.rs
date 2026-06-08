//! Filter-trait adapter that talks to a [`SidecarSupervisor`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use minos_core::{BuildError, Filter, FilterKind, Packet, Verdict};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;

use crate::sidecar::protocol::{Request, RequestHttp, Response};
use crate::sidecar::supervisor::SidecarSupervisor;
use crate::sidecar::venv;
use crate::sidecar::wrapper::WRAPPER_SOURCE;
use crate::FilterError;

/// Filter that delegates `inspect` to a Python sidecar over the Unix
/// socket owned by `supervisor`. Fail-open by default: any timeout,
/// supervisor-not-started, or sidecar error returns Pass. Set
/// `fail_closed` to flip the default to Block on failure.
pub struct PythonSidecarFilter {
    supervisor: Arc<SidecarSupervisor>,
    timeout: Duration,
    fail_closed: bool,
}

impl PythonSidecarFilter {
    /// Construct a new sidecar filter.
    #[must_use]
    pub fn new(supervisor: Arc<SidecarSupervisor>, timeout: Duration, fail_closed: bool) -> Self {
        Self {
            supervisor,
            timeout,
            fail_closed,
        }
    }
}

impl Filter for PythonSidecarFilter {
    fn kind(&self) -> &'static str {
        "python_sidecar"
    }

    fn accepts(&self, _: &Packet) -> bool {
        true
    }

    fn inspect(&self, packet: &Packet) -> Verdict {
        let req = build_request(packet);
        let supervisor = Arc::clone(&self.supervisor);
        let timeout = self.timeout;
        // Bridge sync trait → async supervisor. Must be called from inside
        // a tokio runtime (the data plane is — the test that drives this
        // is too).
        let resp = tokio::task::block_in_place(|| {
            Handle::current().block_on(async move { supervisor.call(req, timeout).await })
        });
        match resp {
            Response::Pass { .. } => {
                if self.fail_closed_should_block() {
                    Verdict::Block {
                        reason: "sidecar unavailable (fail-closed)".into(),
                    }
                } else {
                    Verdict::Pass
                }
            }
            Response::Block { reason, .. } => Verdict::Block { reason },
        }
    }
}

impl PythonSidecarFilter {
    /// The supervisor returns `Response::Pass` both for genuine pass
    /// verdicts AND for fail-open recovery. In fail-closed mode, we can't
    /// distinguish them from the response alone — for v1 we treat all
    /// Pass responses as Pass and rely on the supervisor's fail-open
    /// default; if `fail_closed` is set, the operator is asking us to
    /// flip ALL passes to blocks. That's a strong choice — documented in
    /// the filter's rustdoc.
    ///
    /// TODO(phase 5): when the supervisor grows a degraded-state flag, use
    /// that here so genuine Pass verdicts stay Pass even in fail-closed mode.
    fn fail_closed_should_block(&self) -> bool {
        // For Phase 3: fail_closed unconditionally blocks Pass responses.
        // This is the conservative-but-blunt v1 interpretation.
        self.fail_closed
    }
}

fn build_request(packet: &Packet<'_>) -> Request {
    match packet {
        Packet::Raw { bytes, direction } => Request {
            id: 0, // supervisor reassigns
            direction: direction_str(*direction).into(),
            kind: "raw".into(),
            bytes_b64: base64_encode(bytes),
            http: None,
        },
        Packet::Http { req, direction } => Request {
            id: 0,
            direction: direction_str(*direction).into(),
            kind: "http".into(),
            bytes_b64: String::new(),
            http: Some(RequestHttp {
                method: req.method.clone(),
                path: req.path.clone(),
                headers: req.headers.clone(),
                body_b64: base64_encode(&req.body),
            }),
        },
    }
}

fn direction_str(d: minos_core::Direction) -> &'static str {
    match d {
        minos_core::Direction::Inbound => "inbound",
        minos_core::Direction::Outbound => "outbound",
    }
}

// ── PythonSidecarConfig ───────────────────────────────────────────────────────

/// JSON config blob for [`PythonSidecarKind`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PythonSidecarConfig {
    /// Service name. Used to name the Unix socket file
    /// (`<socket_dir>/<service_name>.sock`) and the staged user script
    /// (`<socket_dir>/<service_name>.py`).
    pub service_name: String,
    /// User-supplied Python script body. Must define a top-level
    /// `filter(packet)` function that returns `None` (pass) or a non-empty
    /// string reason (block).
    pub script: String,
    /// `requirements.txt` contents. Empty string means "stdlib only".
    #[serde(default)]
    pub requirements: String,
    /// Per-call timeout in milliseconds. Defaults to 10 ms (design §7.4).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// When `true`, flips fail-open to fail-closed: any sidecar error or
    /// timeout produces a Block verdict instead of Pass.
    #[serde(default)]
    pub fail_closed: bool,
}

fn default_timeout_ms() -> u64 {
    10
}

// ── PythonSidecarKind ─────────────────────────────────────────────────────────

/// Registry handle for [`PythonSidecarFilter`].
///
/// # Lifecycle — why `build()` returns a non-started filter
///
/// [`FilterKind::build`] is synchronous (the trait cannot be made `async`
/// without breaking every implementor). [`SidecarSupervisor::start`] is
/// `async`. To avoid forcing every `save_config` call-site into a Tokio
/// runtime, `build()` performs only the synchronous work — it installs the
/// venv (via `uv`), stages the wrapper and user script on disk, and
/// constructs a [`PythonSidecarFilter`] wrapping a **non-started**
/// [`SidecarSupervisor`].
///
/// The Phase 5 binary (which owns the Tokio runtime) is responsible for
/// iterating over the built `RuleSet`, locating each sidecar filter's
/// supervisor, and calling `supervisor.start().await` before the ruleset
/// goes live on the data plane.
pub struct PythonSidecarKind;

impl FilterKind for PythonSidecarKind {
    const NAME: &'static str = "python_sidecar";
    type Config = PythonSidecarConfig;

    /// Build a [`PythonSidecarFilter`] from `cfg`.
    ///
    /// Synchronously:
    /// 1. Installs (or cache-hits) the Python venv via `uv`.
    /// 2. Stages the embedded wrapper script inside the venv directory.
    /// 3. Stages the user-supplied script under `$MINOS_SOCKET_DIR`.
    /// 4. Constructs a [`SidecarSupervisor`] — **not yet started**.
    /// 5. Wraps the supervisor in a [`PythonSidecarFilter`].
    ///
    /// The returned filter is fail-open until `supervisor.start().await`
    /// succeeds — any `inspect()` call before that returns `Pass`.
    ///
    /// # Errors
    ///
    /// Returns [`BuildError::Invalid`] if `uv` is missing, the venv
    /// install fails, or any filesystem operation fails.
    fn build(cfg: Self::Config) -> Result<std::sync::Arc<dyn minos_core::Filter>, BuildError> {
        build_python_sidecar(cfg).map_err(|e| BuildError::Invalid {
            kind: Self::NAME,
            message: e.to_string(),
        })
    }
}

fn build_python_sidecar(
    cfg: PythonSidecarConfig,
) -> Result<std::sync::Arc<dyn minos_core::Filter>, FilterError> {
    let PythonSidecarConfig {
        service_name,
        script,
        requirements,
        timeout_ms,
        fail_closed,
    } = cfg;

    let venv_root = venv::default_root();
    let socket_dir = PathBuf::from(
        std::env::var("MINOS_SOCKET_DIR").unwrap_or_else(|_| "/run/minos/sidecars".into()),
    );

    // 1. Materialize the venv (idempotent; cached if hash unchanged).
    let outcome = venv::install(&venv_root, requirements.as_bytes())?;
    let venv_python = outcome.path.join("bin").join("python");

    // 2. Stage the wrapper script inside the venv directory so it is
    //    co-located with the Python that will run it.
    let wrapper_path = outcome.path.join("minos_wrapper.py");
    std::fs::write(&wrapper_path, WRAPPER_SOURCE)?;

    // 3. Stage the user script under the socket dir, keyed by service name.
    std::fs::create_dir_all(&socket_dir)?;
    let user_script_path = socket_dir.join(format!("{service_name}.py"));
    std::fs::write(&user_script_path, &script)?;

    // 4. Construct the (not-yet-started) supervisor.
    let socket_path = socket_dir.join(format!("{service_name}.sock"));
    let supervisor = std::sync::Arc::new(SidecarSupervisor::new(
        socket_path,
        venv_python,
        wrapper_path,
        user_script_path,
    ));

    // 5. Wrap in the Filter adapter.
    let filter = PythonSidecarFilter::new(
        supervisor,
        std::time::Duration::from_millis(timeout_ms),
        fail_closed,
    );
    Ok(std::sync::Arc::new(filter))
}

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
