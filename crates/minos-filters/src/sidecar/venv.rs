//! Hash-deduped Python virtualenvs managed via `uv`.

use std::fmt::Write;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

use crate::FilterError;

/// Compute the SHA-256 hex digest of `requirements`. Identical bytes →
/// identical hash → the same on-disk venv directory.
#[must_use]
pub fn venv_hash(requirements: &[u8]) -> String {
    let digest = Sha256::digest(requirements);
    to_hex(&digest)
}

/// Compose the venv path: `<root>/<hash>`.
#[must_use]
pub fn venv_path(root: impl AsRef<Path>, hash: &str) -> PathBuf {
    root.as_ref().join(hash)
}

/// The root directory under which per-requirements venvs live. Honors
/// `MINOS_VENV_ROOT`, defaulting to `/var/lib/minos/venvs`. Shared by the
/// filter builder and the web UI's save-progress streamer so both agree on
/// where venvs are materialized (and thus share the idempotent cache).
#[must_use]
pub fn default_root() -> PathBuf {
    PathBuf::from(
        std::env::var("MINOS_VENV_ROOT").unwrap_or_else(|_| "/var/lib/minos/venvs".into()),
    )
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").expect("writing to String never fails");
    }
    s
}

/// Result of materializing a venv.
#[derive(Debug)]
pub struct InstallOutcome {
    /// Absolute path to the venv directory.
    pub path: PathBuf,
    /// Combined stdout/stderr from the uv invocations (or `"(cached)"`).
    pub log: String,
}

/// Create a venv under `root` keyed by the hash of `requirements`, and
/// install `requirements` into it. Idempotent: if the venv already exists
/// with the marker file, returns immediately with `log = "(cached)"`.
///
/// This is a thin wrapper over [`install_streaming`] that accumulates every
/// emitted line into the returned [`InstallOutcome::log`].
///
/// # Errors
///
/// Returns [`FilterError::UvNotFound`] if `uv` is missing on PATH,
/// [`FilterError::UvInstallFailed`] if either `uv venv` or `uv pip install`
/// exits non-zero, or [`FilterError::Io`] for filesystem failures.
pub fn install(root: &Path, requirements: &[u8]) -> Result<InstallOutcome, FilterError> {
    install_streaming(root, requirements, &mut |_line| {})
}

/// Like [`install`], but invokes `on_line` for each line of `uv`'s combined
/// stdout/stderr as it arrives, so callers (e.g. the web UI's save flow) can
/// stream progress live. The same lines are also collected into the returned
/// [`InstallOutcome::log`].
///
/// On the cached path, `on_line` is invoked once with `"(cached)"`.
///
/// # Errors
///
/// Same as [`install`].
pub fn install_streaming(
    root: &Path,
    requirements: &[u8],
    on_line: &mut dyn FnMut(&str),
) -> Result<InstallOutcome, FilterError> {
    if which::which("uv").is_err() {
        return Err(FilterError::UvNotFound);
    }

    let hash = venv_hash(requirements);
    let path = venv_path(root, &hash);
    let marker = path.join(".minos-installed");

    if marker.exists() {
        on_line("(cached)");
        return Ok(InstallOutcome {
            path,
            log: "(cached)".into(),
        });
    }

    // Ensure the parent root exists, but let `uv venv` create the venv
    // directory itself — it refuses to populate a pre-existing target.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut log = String::new();

    let path_str = path.to_str().ok_or_else(|| {
        FilterError::BadConfig(format!("venv path is not valid UTF-8: {}", path.display()))
    })?;
    run_streaming(
        Command::new("uv").args(["venv", path_str]),
        &format!("$ uv venv {}", path.display()),
        on_line,
        &mut log,
    )?;

    // The venv exists now; stage requirements.txt next to it.
    let req_file = path.join("requirements.txt");
    std::fs::write(&req_file, requirements)?;

    let py = path.join("bin").join("python");
    let py_str = py.to_str().ok_or_else(|| {
        FilterError::BadConfig(format!("python path is not valid UTF-8: {}", py.display()))
    })?;
    let req_str = req_file.to_str().ok_or_else(|| {
        FilterError::BadConfig(format!(
            "requirements path is not valid UTF-8: {}",
            req_file.display()
        ))
    })?;
    run_streaming(
        Command::new("uv").args(["pip", "install", "--python", py_str, "-r", req_str]),
        &format!("$ uv pip install -r {}", req_file.display()),
        on_line,
        &mut log,
    )?;

    std::fs::write(&marker, b"")?;
    Ok(InstallOutcome { path, log })
}

/// Run `cmd` with stdout+stderr piped, forwarding each line to `on_line` and
/// appending it to `log`. The `banner` (e.g. the command being run) is
/// emitted first. Reads both streams concurrently on threads so a chatty
/// stream can't deadlock against a full pipe buffer.
///
/// Returns [`FilterError::UvInstallFailed`] (carrying the accumulated log) if
/// the command exits non-zero.
fn run_streaming(
    cmd: &mut Command,
    banner: &str,
    on_line: &mut dyn FnMut(&str),
    log: &mut String,
) -> Result<(), FilterError> {
    on_line(banner);
    let _ = writeln!(log, "{banner}");

    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx_err = tx.clone();
    let h_out = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let h_err = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx_err.send(line).is_err() {
                break;
            }
        }
    });

    // Both senders drop when the threads finish, closing the channel.
    for line in rx {
        on_line(&line);
        log.push_str(&line);
        log.push('\n');
    }
    let _ = h_out.join();
    let _ = h_err.join();

    let status = child.wait()?;
    if !status.success() {
        return Err(FilterError::UvInstallFailed(std::mem::take(log)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_requirements_hash_identically() {
        let a = venv_hash(b"requests==2.31\nhttpx==0.27\n");
        let b = venv_hash(b"requests==2.31\nhttpx==0.27\n");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64); // sha256 hex
    }

    #[test]
    fn whitespace_differences_change_hash() {
        let a = venv_hash(b"requests==2.31\n");
        let b = venv_hash(b"requests==2.31");
        assert_ne!(a, b);
    }

    #[test]
    fn venv_path_is_root_plus_hash() {
        let p = venv_path("/var/lib/minos/venvs", "abcd1234");
        assert_eq!(p, std::path::PathBuf::from("/var/lib/minos/venvs/abcd1234"));
    }
}

#[cfg(test)]
mod install_tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn uv_available() -> bool {
        Command::new("uv")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn install_creates_venv_and_runs_python() {
        if !uv_available() {
            eprintln!("uv not on PATH, skipping");
            return;
        }
        let root = tempdir().unwrap();
        let requirements = b"requests==2.31.0\n";
        let outcome = install(root.path(), requirements).expect("install");
        let py = outcome.path.join("bin").join("python");
        assert!(py.exists(), "expected python at {py:?}");
        assert!(
            outcome.path.join(".minos-installed").exists(),
            "marker missing"
        );
    }

    #[test]
    fn install_streaming_invokes_callback_per_line() {
        if !uv_available() {
            eprintln!("uv not on PATH, skipping");
            return;
        }
        let root = tempdir().unwrap();
        let mut lines: Vec<String> = Vec::new();
        let outcome = install_streaming(root.path(), b"", &mut |l| lines.push(l.to_string()))
            .expect("install");
        assert!(
            !lines.is_empty(),
            "expected at least one streamed line (the command banner)"
        );
        // The banner for the venv command must have been streamed.
        assert!(
            lines.iter().any(|l| l.contains("uv venv")),
            "banner missing from streamed lines: {lines:?}"
        );
        // Streamed lines and the collected log agree on the banner.
        assert!(outcome.log.contains("uv venv"));
    }

    #[test]
    fn install_is_idempotent_on_second_call() {
        if !uv_available() {
            eprintln!("uv not on PATH, skipping");
            return;
        }
        let root = tempdir().unwrap();
        let requirements = b"";
        let first = install(root.path(), requirements).expect("first");
        let second = install(root.path(), requirements).expect("second");
        assert_eq!(first.path, second.path);
        assert!(
            second.log.contains("(cached)"),
            "second call should hit cache"
        );
    }
}
