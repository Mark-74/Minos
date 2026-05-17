//! Filter build-time errors.

use thiserror::Error;

/// Errors raised while *building* a filter from its config. Runtime
/// inspection errors do not flow through this type — they degrade to
/// fail-open verdicts inside the filter itself.
#[derive(Debug, Error)]
pub enum FilterError {
    /// A regex pattern was syntactically invalid.
    #[error("invalid regex: {0}")]
    BadRegex(String),
    /// A status-code, method, or other config value was out of range.
    #[error("bad config: {0}")]
    BadConfig(String),
    /// Failed to locate `uv` on `PATH` at build time.
    #[error("uv not found on PATH (required for python_sidecar filters)")]
    UvNotFound,
    /// `uv` exited non-zero while installing requirements.
    #[error("uv install failed: {0}")]
    UvInstallFailed(String),
    /// I/O error during venv creation or script staging.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Saved Python script failed `python -m py_compile` validation.
    #[error("python syntax error: {0}")]
    PythonSyntaxError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FilterError>();
    }

    #[test]
    fn display_for_each_variant_is_nonempty() {
        let cases = [
            FilterError::BadRegex("x".into()),
            FilterError::BadConfig("x".into()),
            FilterError::UvNotFound,
            FilterError::UvInstallFailed("x".into()),
            FilterError::Io(std::io::Error::other("x")),
            FilterError::PythonSyntaxError("x".into()),
        ];
        for c in cases {
            assert!(!format!("{c}").is_empty());
        }
    }
}
