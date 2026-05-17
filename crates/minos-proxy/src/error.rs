//! Error type for the data plane.

use std::net::SocketAddr;

use thiserror::Error;

/// Non-fatal failures raised by the proxy. The listener catches these per
/// connection; nothing here should propagate up far enough to stop the loop.
#[derive(Debug, Error)]
pub enum ProxyError {
    /// IO failure on the client or upstream socket.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// `httparse` couldn't make sense of the inbound request, or the request
    /// exceeded the per-service `max_body_bytes`.
    #[error("http parse: {0}")]
    HttpParse(String),

    /// Body exceeded `max_body_bytes`.
    #[error("body too large: {got}B > limit {limit}B")]
    BodyTooLarge {
        /// Configured maximum.
        limit: usize,
        /// Actual size (or declared Content-Length).
        got: usize,
    },

    /// Couldn't open a TCP connection to the configured upstream.
    #[error("upstream connect: {0}")]
    UpstreamConnect(SocketAddr),

    /// `SO_ORIGINAL_DST` lookup failed in transparent mode.
    #[error("transparent-mode lookup: {0}")]
    TransparentLookup(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProxyError>();
    }

    #[test]
    fn display_for_each_variant_is_nonempty() {
        let cases = [
            ProxyError::Io(std::io::Error::other("x")),
            ProxyError::HttpParse("malformed".into()),
            ProxyError::BodyTooLarge { limit: 10, got: 11 },
            ProxyError::UpstreamConnect("127.0.0.1:1".parse().unwrap()),
            ProxyError::TransparentLookup("no SO_ORIGINAL_DST".into()),
        ];
        for c in cases {
            assert!(!format!("{c}").is_empty());
        }
    }
}
