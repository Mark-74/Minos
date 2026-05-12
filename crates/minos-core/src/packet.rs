//! Packet representation passed through the filter pipeline.

use crate::Direction;

/// A parsed HTTP request (or response) the firewall has buffered for inspection.
///
/// Fields are owned because the buffered request typically outlives the borrow
/// scope of the underlying TCP read buffer. Headers are kept as a `Vec` of
/// `(name, value)` pairs (rather than a `HashMap`) because HTTP allows
/// duplicate header names and the order can be semantically meaningful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHttp {
    /// e.g. "GET", "POST".
    pub method: String,
    /// e.g. "/api/users?id=1".
    pub path: String,
    /// Header pairs in the order they appeared on the wire.
    pub headers: Vec<(String, String)>,
    /// Raw request body bytes, capped by the per-service `max_body_bytes`.
    pub body: Vec<u8>,
}

impl ParsedHttp {
    /// Convenience constructor for tests and call sites that already have all
    /// fields ready.
    #[must_use]
    pub fn new(
        method: impl Into<String>,
        path: impl Into<String>,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            headers,
            body,
        }
    }

    /// Look up the first header with the given (case-insensitive) name.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A single inspectable unit handed to the filter pipeline.
#[derive(Debug)]
pub enum Packet<'a> {
    /// A buffered burst of bytes from a raw TCP service.
    Raw {
        /// The buffered bytes.
        bytes: &'a [u8],
        /// Direction relative to the protected service.
        direction: Direction,
    },
    /// A buffered HTTP request or response.
    Http {
        /// The parsed request/response.
        req: &'a ParsedHttp,
        /// Direction relative to the protected service.
        direction: Direction,
    },
}

impl Packet<'_> {
    /// Returns the direction the packet is flowing, regardless of variant.
    #[must_use]
    pub fn direction(&self) -> Direction {
        match self {
            Packet::Raw { direction, .. } | Packet::Http { direction, .. } => *direction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_lookup_is_case_insensitive() {
        let req = ParsedHttp::new(
            "GET",
            "/",
            vec![("Content-Type".into(), "text/plain".into())],
            vec![],
        );
        assert_eq!(req.header("content-type"), Some("text/plain"));
        assert_eq!(req.header("CONTENT-TYPE"), Some("text/plain"));
        assert_eq!(req.header("missing"), None);
    }

    #[test]
    fn header_lookup_returns_first_on_duplicates() {
        let req = ParsedHttp::new(
            "GET",
            "/",
            vec![
                ("X-Foo".into(), "first".into()),
                ("X-Foo".into(), "second".into()),
            ],
            vec![],
        );
        assert_eq!(req.header("x-foo"), Some("first"));
    }

    #[test]
    fn packet_direction_returns_correct_variant() {
        let bytes = b"hello";
        let raw = Packet::Raw {
            bytes,
            direction: Direction::Inbound,
        };
        assert_eq!(raw.direction(), Direction::Inbound);

        let req = ParsedHttp::new("GET", "/", vec![], vec![]);
        let http = Packet::Http {
            req: &req,
            direction: Direction::Outbound,
        };
        assert_eq!(http.direction(), Direction::Outbound);
    }
}
