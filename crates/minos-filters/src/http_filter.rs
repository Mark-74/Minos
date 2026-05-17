//! HTTP-aware filter: method, path regex, header regex, body regex.
//!
//! All non-empty / non-`None` fields are AND-ed. An empty config matches
//! nothing (passes everything) — a useful no-op for testing.

use std::sync::Arc;

use minos_core::{BuildError, Filter, FilterKind, Packet, Verdict};
use regex::bytes::Regex as ByteRegex;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::FilterError;

/// One header-name + value-regex pair.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HeaderMatch {
    /// Header name (case-insensitive comparison).
    pub name: String,
    /// Regex matched against the header value.
    pub value_regex: String,
}

/// Configuration for [`HttpFilter`]. All non-empty / non-`None` fields are
/// AND-ed; an empty config matches nothing (passes everything).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct HttpConfig {
    /// If non-empty, the request method must be in this list (case-insensitive).
    #[serde(default)]
    pub methods: Vec<String>,
    /// If `Some`, the request path must match this regex.
    #[serde(default)]
    pub path_regex: Option<String>,
    /// Each entry must match (name is case-insensitive, value regex).
    #[serde(default)]
    pub headers: Vec<HeaderMatch>,
    /// If `Some`, the request body must match this regex (bytes engine).
    #[serde(default)]
    pub body_regex: Option<String>,
}

/// Compiled HTTP filter.
#[derive(Debug)]
pub struct HttpFilter {
    methods: Vec<String>,
    path_re: Option<Regex>,
    header_matchers: Vec<(String, Regex)>,
    body_re: Option<ByteRegex>,
    /// Whether at least one condition is configured.
    has_conditions: bool,
}

impl HttpFilter {
    /// Compile a config into a runnable filter.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::BadRegex`] if any regex fails to compile.
    pub fn new(cfg: HttpConfig) -> Result<Self, FilterError> {
        let path_re = cfg
            .path_regex
            .map(|s| Regex::new(&s).map_err(|e| FilterError::BadRegex(e.to_string())))
            .transpose()?;
        let body_re = cfg
            .body_regex
            .map(|s| ByteRegex::new(&s).map_err(|e| FilterError::BadRegex(e.to_string())))
            .transpose()?;
        let mut header_matchers = Vec::with_capacity(cfg.headers.len());
        for h in cfg.headers {
            let re =
                Regex::new(&h.value_regex).map_err(|e| FilterError::BadRegex(e.to_string()))?;
            header_matchers.push((h.name, re));
        }
        let has_conditions = !cfg.methods.is_empty()
            || path_re.is_some()
            || !header_matchers.is_empty()
            || body_re.is_some();
        Ok(Self {
            methods: cfg.methods,
            path_re,
            header_matchers,
            body_re,
            has_conditions,
        })
    }
}

impl Filter for HttpFilter {
    fn kind(&self) -> &'static str {
        "http"
    }

    fn accepts(&self, p: &Packet) -> bool {
        matches!(p, Packet::Http { .. })
    }

    fn inspect(&self, p: &Packet) -> Verdict {
        let Packet::Http { req, .. } = p else {
            return Verdict::Pass;
        };
        if !self.has_conditions {
            return Verdict::Pass;
        }
        if !self.methods.is_empty()
            && !self
                .methods
                .iter()
                .any(|m| m.eq_ignore_ascii_case(&req.method))
        {
            return Verdict::Pass;
        }
        if let Some(re) = &self.path_re {
            if !re.is_match(&req.path) {
                return Verdict::Pass;
            }
        }
        for (name, re) in &self.header_matchers {
            let hit = req
                .headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case(name) && re.is_match(v));
            if !hit {
                return Verdict::Pass;
            }
        }
        if let Some(re) = &self.body_re {
            if !re.is_match(&req.body) {
                return Verdict::Pass;
            }
        }
        Verdict::Block {
            reason: "http filter matched".into(),
        }
    }
}

/// Registry handle for [`HttpFilter`].
pub struct HttpKind;

impl FilterKind for HttpKind {
    const NAME: &'static str = "http";
    type Config = HttpConfig;

    fn build(cfg: Self::Config) -> Result<Arc<dyn Filter>, BuildError> {
        let f = HttpFilter::new(cfg).map_err(|e| BuildError::Invalid {
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

    fn http(req: &ParsedHttp) -> Packet<'_> {
        Packet::Http {
            req,
            direction: Direction::Inbound,
        }
    }

    fn req(method: &str, path: &str, body: &[u8]) -> ParsedHttp {
        ParsedHttp {
            method: method.into(),
            path: path.into(),
            headers: vec![("Host".into(), "y".into())],
            body: body.to_vec(),
        }
    }

    #[test]
    fn empty_filter_passes() {
        let f = HttpFilter::new(HttpConfig::default()).unwrap();
        assert!(matches!(
            f.inspect(&http(&req("GET", "/", b""))),
            Verdict::Pass
        ));
    }

    #[test]
    fn method_only_blocks() {
        let cfg = HttpConfig {
            methods: vec!["POST".into()],
            ..Default::default()
        };
        let f = HttpFilter::new(cfg).unwrap();
        assert!(matches!(
            f.inspect(&http(&req("POST", "/", b""))),
            Verdict::Block { .. }
        ));
        assert!(matches!(
            f.inspect(&http(&req("GET", "/", b""))),
            Verdict::Pass
        ));
    }

    #[test]
    fn all_conditions_must_match() {
        let cfg = HttpConfig {
            methods: vec!["POST".into()],
            path_regex: Some("/api/.*".into()),
            body_regex: Some("DROP".into()),
            ..Default::default()
        };
        let f = HttpFilter::new(cfg).unwrap();
        assert!(matches!(
            f.inspect(&http(&req("POST", "/api/x", b"DROP TABLE"))),
            Verdict::Block { .. }
        ));
        assert!(matches!(
            f.inspect(&http(&req("GET", "/api/x", b"DROP TABLE"))),
            Verdict::Pass
        ));
        assert!(matches!(
            f.inspect(&http(&req("POST", "/x", b"DROP TABLE"))),
            Verdict::Pass
        ));
        assert!(matches!(
            f.inspect(&http(&req("POST", "/api/x", b"clean"))),
            Verdict::Pass
        ));
    }

    #[test]
    fn header_match_with_value_regex() {
        let cfg = HttpConfig {
            headers: vec![HeaderMatch {
                name: "User-Agent".into(),
                value_regex: "(?i)sqlmap".into(),
            }],
            ..Default::default()
        };
        let f = HttpFilter::new(cfg).unwrap();
        let mut r = req("GET", "/", b"");
        r.headers.push(("User-Agent".into(), "sqlmap/1.6".into()));
        assert!(matches!(f.inspect(&http(&r)), Verdict::Block { .. }));
        r.headers.last_mut().unwrap().1 = "curl/8".into();
        assert!(matches!(f.inspect(&http(&r)), Verdict::Pass));
    }

    #[test]
    fn raw_packet_is_rejected_by_accepts() {
        let f = HttpFilter::new(HttpConfig::default()).unwrap();
        assert!(!f.accepts(&Packet::Raw {
            bytes: b"",
            direction: Direction::Inbound,
        }));
    }
}
