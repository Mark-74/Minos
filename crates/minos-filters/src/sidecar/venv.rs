//! Hash-deduped Python virtualenvs managed via `uv`.

use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

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
/// # Errors
///
/// Returns [`FilterError::UvNotFound`] if `uv` is missing on PATH,
/// [`FilterError::UvInstallFailed`] if either `uv venv` or `uv pip install`
/// exits non-zero, or [`FilterError::Io`] for filesystem failures.
pub fn install(root: &Path, requirements: &[u8]) -> Result<InstallOutcome, FilterError> {
    if which::which("uv").is_err() {
        return Err(FilterError::UvNotFound);
    }

    let hash = venv_hash(requirements);
    let path = venv_path(root, &hash);
    let marker = path.join(".minos-installed");

    if marker.exists() {
        return Ok(InstallOutcome {
            path,
            log: "(cached)".into(),
        });
    }

    std::fs::create_dir_all(&path)?;
    let req_file = path.join("requirements.txt");
    std::fs::write(&req_file, requirements)?;

    let mut log = String::new();

    let path_str = path.to_str().ok_or_else(|| {
        FilterError::BadConfig(format!("venv path is not valid UTF-8: {}", path.display()))
    })?;
    let out = Command::new("uv").args(["venv", path_str]).output()?;
    let _ = write!(
        log,
        "$ uv venv {}\n{}\n{}\n",
        path.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    if !out.status.success() {
        return Err(FilterError::UvInstallFailed(log));
    }

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
    let out = Command::new("uv")
        .args(["pip", "install", "--python", py_str, "-r", req_str])
        .output()?;
    let _ = write!(
        log,
        "$ uv pip install -r {}\n{}\n{}\n",
        req_file.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    if !out.status.success() {
        return Err(FilterError::UvInstallFailed(log));
    }

    std::fs::write(&marker, b"")?;
    Ok(InstallOutcome { path, log })
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
