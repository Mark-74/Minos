//! Verdict produced by a filter, and the block-response shape used when a
//! filter or pipeline rejects a packet.

use serde::{Deserialize, Serialize};

/// What a single filter (or the pipeline as a whole) decided about a packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Allow the packet to continue.
    Pass,
    /// Block the packet, including a human-readable reason for logging.
    Block {
        /// Why the filter rejected this packet.
        reason: String,
    },
}

impl Verdict {
    /// True for `Verdict::Pass`.
    #[must_use]
    pub fn is_pass(&self) -> bool {
        matches!(self, Verdict::Pass)
    }

    /// True for `Verdict::Block { .. }`.
    #[must_use]
    pub fn is_block(&self) -> bool {
        matches!(self, Verdict::Block { .. })
    }
}

/// The response Minos sends to the client when a request is blocked.
///
/// For HTTP services this becomes the response body and status. For raw TCP
/// services Minos closes the connection regardless of the variant; the body
/// fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockResponse {
    /// Send a fixed HTTP status with the given body and headers.
    HttpStatus {
        /// HTTP status code, e.g. 403 or 404.
        status: u16,
        /// Response body bytes (may be empty).
        body: Vec<u8>,
        /// Additional headers to send alongside the status.
        headers: Vec<(String, String)>,
    },
    /// Close the connection without sending a response (TCP default).
    Close,
}

impl Default for BlockResponse {
    fn default() -> Self {
        Self::HttpStatus {
            status: 403,
            body: Vec::new(),
            headers: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_helpers() {
        assert!(Verdict::Pass.is_pass());
        assert!(!Verdict::Pass.is_block());
        let blk = Verdict::Block {
            reason: "bad".into(),
        };
        assert!(blk.is_block());
        assert!(!blk.is_pass());
    }

    #[test]
    fn default_block_response_is_403_empty() {
        match BlockResponse::default() {
            BlockResponse::HttpStatus {
                status,
                body,
                headers,
            } => {
                assert_eq!(status, 403);
                assert!(body.is_empty());
                assert!(headers.is_empty());
            }
            other @ BlockResponse::Close => panic!("unexpected default: {other:?}"),
        }
    }

    #[test]
    fn block_response_round_trips_through_serde() {
        let cases = vec![
            BlockResponse::Close,
            BlockResponse::HttpStatus {
                status: 404,
                body: b"not found".to_vec(),
                headers: vec![("X-Trap".into(), "1".into())],
            },
        ];
        for r in cases {
            let json = serde_json::to_string(&r).unwrap();
            let back: BlockResponse = serde_json::from_str(&json).unwrap();
            assert_eq!(r, back);
        }
    }
}
