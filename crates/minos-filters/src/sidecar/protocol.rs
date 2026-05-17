//! Length-prefixed JSON wire format spoken by the Python sidecar.
//!
//! Frame: `[u32 big-endian length][JSON object]`.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Optional HTTP block on a [`Request`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestHttp {
    /// HTTP method.
    pub method: String,
    /// Request path.
    pub path: String,
    /// Headers as `[name, value]` pairs.
    pub headers: Vec<(String, String)>,
    /// Base64-encoded body bytes.
    pub body_b64: String,
}

/// Wire-format request from the data plane to the sidecar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Monotonic per-process request id; echoed in the response.
    pub id: u64,
    /// `"inbound"` or `"outbound"`.
    pub direction: String,
    /// `"raw"` or `"http"`.
    pub kind: String,
    /// Base64-encoded raw bytes (always present; for HTTP this is the
    /// re-serialized request, may be empty when the `http` field carries
    /// the full payload).
    pub bytes_b64: String,
    /// HTTP fields when `kind == "http"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<RequestHttp>,
}

/// Wire-format response. The `verdict` field discriminates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "lowercase")]
pub enum Response {
    /// Pass verdict.
    Pass {
        /// Echoes the request id.
        id: u64,
    },
    /// Block verdict.
    Block {
        /// Echoes the request id.
        id: u64,
        /// Human-readable reason from the user's script.
        reason: String,
    },
}

impl Response {
    /// Request id this response refers to.
    #[must_use]
    pub fn id(&self) -> u64 {
        match self {
            Response::Pass { id } | Response::Block { id, .. } => *id,
        }
    }
}

/// Maximum frame size accepted by the codec (8 MiB).
const MAX_FRAME_BYTES: u32 = 8 * 1024 * 1024;

/// Errors raised by the framing codec.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// Underlying socket failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Length prefix announced a frame larger than `MAX_FRAME_BYTES`.
    #[error("oversized frame: {0} bytes")]
    Oversized(u32),
    /// JSON in the frame body failed to deserialize.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Read one length-prefixed JSON [`Request`] from `r`.
///
/// # Errors
///
/// Returns [`CodecError`] for IO failure, oversized frames, or malformed JSON.
pub async fn read_request<R: AsyncRead + Unpin>(r: &mut R) -> Result<Request, CodecError> {
    read_frame(r).await
}

/// Read one length-prefixed JSON [`Response`] from `r`.
///
/// # Errors
///
/// Returns [`CodecError`] for IO failure, oversized frames, or malformed JSON.
pub async fn read_response<R: AsyncRead + Unpin>(r: &mut R) -> Result<Response, CodecError> {
    read_frame(r).await
}

/// Write `req` as a length-prefixed JSON frame to `w`.
///
/// # Errors
///
/// Returns [`CodecError`] for IO failure or oversized payload.
pub async fn write_request<W: AsyncWrite + Unpin>(
    w: &mut W,
    req: &Request,
) -> Result<(), CodecError> {
    write_frame(w, req).await
}

/// Write `resp` as a length-prefixed JSON frame to `w`.
///
/// # Errors
///
/// Returns [`CodecError`] for IO failure or oversized payload.
pub async fn write_response<W: AsyncWrite + Unpin>(
    w: &mut W,
    resp: &Response,
) -> Result<(), CodecError> {
    write_frame(w, resp).await
}

async fn read_frame<R, T>(r: &mut R) -> Result<T, CodecError>
where
    R: AsyncRead + Unpin,
    T: for<'de> serde::Deserialize<'de>,
{
    let mut lenbuf = [0u8; 4];
    r.read_exact(&mut lenbuf).await?;
    let len = u32::from_be_bytes(lenbuf);
    if len > MAX_FRAME_BYTES {
        return Err(CodecError::Oversized(len));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

async fn write_frame<W, T>(w: &mut W, value: &T) -> Result<(), CodecError>
where
    W: AsyncWrite + Unpin,
    T: serde::Serialize,
{
    let body = serde_json::to_vec(value)?;
    let len = u32::try_from(body.len()).map_err(|_| CodecError::Oversized(u32::MAX))?;
    if len > MAX_FRAME_BYTES {
        return Err(CodecError::Oversized(len));
    }
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&body).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_json() {
        let r = Request {
            id: 1,
            direction: "inbound".into(),
            kind: "raw".into(),
            bytes_b64: "aGVsbG8=".into(),
            http: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: Request = serde_json::from_str(&s).unwrap();
        assert_eq!(r.id, back.id);
    }

    #[test]
    fn response_block_serializes_with_reason() {
        let r = Response::Block {
            id: 1,
            reason: "nope".into(),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"verdict\":\"block\""));
        assert!(s.contains("\"reason\":\"nope\""));
    }

    #[test]
    fn response_pass_id_accessor_works() {
        let r = Response::Pass { id: 7 };
        assert_eq!(r.id(), 7);
        let r = Response::Block {
            id: 8,
            reason: "x".into(),
        };
        assert_eq!(r.id(), 8);
    }

    #[tokio::test]
    async fn write_then_read_round_trips() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let req = Request {
            id: 42,
            direction: "inbound".into(),
            kind: "raw".into(),
            bytes_b64: "aGk=".into(),
            http: None,
        };
        write_request(&mut a, &req).await.unwrap();
        let back = read_request(&mut b).await.unwrap();
        assert_eq!(back.id, 42);
    }

    #[tokio::test]
    async fn read_rejects_oversized_frame() {
        use tokio::io::AsyncWriteExt;
        let (mut a, mut b) = tokio::io::duplex(4096);
        // 50 MB length prefix exceeds the 8 MiB cap.
        a.write_all(&50_000_000u32.to_be_bytes()).await.unwrap();
        let err = read_request(&mut b).await.err().unwrap();
        assert!(matches!(err, CodecError::Oversized(_)), "got {err:?}");
    }
}
