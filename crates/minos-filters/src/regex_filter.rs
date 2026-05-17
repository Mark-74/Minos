//! Raw-byte regex filter. Matches against the raw bytes of `Packet::Raw`
//! and against `method SP path body` for `Packet::Http`.

use std::sync::Arc;

use minos_core::{BuildError, Filter, FilterKind, Packet, Verdict};
use regex::bytes::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::FilterError;

/// A compiled regex matcher. Cheap to share via `Arc<dyn Filter>`.
#[derive(Debug)]
pub struct RegexFilter {
    pattern_source: String,
    re: Regex,
}

impl RegexFilter {
    /// Compile `pattern` into a `RegexFilter`.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::BadRegex`] if `pattern` fails to compile.
    pub fn new(pattern: &str) -> Result<Self, FilterError> {
        let re = Regex::new(pattern).map_err(|e| FilterError::BadRegex(e.to_string()))?;
        Ok(Self {
            pattern_source: pattern.to_string(),
            re,
        })
    }
}

impl Filter for RegexFilter {
    fn kind(&self) -> &'static str {
        "regex"
    }

    fn accepts(&self, _: &Packet) -> bool {
        true
    }

    fn inspect(&self, p: &Packet) -> Verdict {
        let matched = match p {
            Packet::Raw { bytes, .. } => self.re.is_match(bytes),
            Packet::Http { req, .. } => {
                let mut buf =
                    Vec::with_capacity(req.method.len() + 1 + req.path.len() + req.body.len());
                buf.extend_from_slice(req.method.as_bytes());
                buf.push(b' ');
                buf.extend_from_slice(req.path.as_bytes());
                buf.extend_from_slice(&req.body);
                self.re.is_match(&buf)
            }
        };
        if matched {
            Verdict::Block {
                reason: format!("regex matched: {}", self.pattern_source),
            }
        } else {
            Verdict::Pass
        }
    }
}

/// JSON-schema config for [`RegexFilter`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RegexConfig {
    /// Regular expression. Uses the `regex` crate's bytes engine; PCRE
    /// constructs like backreferences are not supported.
    pub pattern: String,
}

/// Registry handle for [`RegexFilter`].
pub struct RegexKind;

impl FilterKind for RegexKind {
    const NAME: &'static str = "regex";
    type Config = RegexConfig;

    fn build(cfg: Self::Config) -> Result<Arc<dyn Filter>, BuildError> {
        let f = RegexFilter::new(&cfg.pattern).map_err(|e| BuildError::Invalid {
            kind: Self::NAME,
            message: e.to_string(),
        })?;
        Ok(Arc::new(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minos_core::{Direction, Packet, ParsedHttp, Verdict};

    fn raw(bytes: &[u8]) -> Packet<'_> {
        Packet::Raw {
            bytes,
            direction: Direction::Inbound,
        }
    }

    fn http(req: &ParsedHttp) -> Packet<'_> {
        Packet::Http {
            req,
            direction: Direction::Inbound,
        }
    }

    #[test]
    fn matches_raw_bytes() {
        let f = RegexFilter::new("DROP TABLE").unwrap();
        assert!(matches!(
            f.inspect(&raw(b"hello DROP TABLE users")),
            Verdict::Block { .. }
        ));
        assert!(matches!(f.inspect(&raw(b"hello users")), Verdict::Pass));
    }

    #[test]
    fn matches_http_request_line_or_body() {
        let f = RegexFilter::new(r"/admin").unwrap();
        let req = ParsedHttp {
            method: "GET".into(),
            path: "/admin/secret".into(),
            headers: vec![],
            body: vec![],
        };
        assert!(matches!(f.inspect(&http(&req)), Verdict::Block { .. }));
    }

    #[test]
    fn invalid_pattern_returns_err() {
        let err = RegexFilter::new("(unbalanced").err().unwrap();
        assert!(matches!(err, crate::FilterError::BadRegex(_)));
    }

    #[test]
    fn accepts_both_raw_and_http() {
        let f = RegexFilter::new("x").unwrap();
        assert!(f.accepts(&raw(b"")));
        let req = ParsedHttp {
            method: "GET".into(),
            path: "/".into(),
            headers: vec![],
            body: vec![],
        };
        assert!(f.accepts(&http(&req)));
    }
}
