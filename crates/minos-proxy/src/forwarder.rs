//! Upstream forwarding for HTTP-protocol services.

use std::net::SocketAddr;

use minos_core::ParsedHttp;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::ProxyError;

/// Build the on-wire representation of a parsed HTTP/1.1 request.
///
/// Preserves the original request line and all headers. Appends
/// `Connection: close` if the client did not already send one (keep-alive
/// is not supported in v1).
fn serialize_request(request: &ParsedHttp) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256 + request.body.len());
    buf.extend_from_slice(request.method.as_bytes());
    buf.push(b' ');
    buf.extend_from_slice(request.path.as_bytes());
    buf.extend_from_slice(b" HTTP/1.1\r\n");
    let mut saw_connection = false;
    for (k, v) in &request.headers {
        if k.eq_ignore_ascii_case("connection") {
            saw_connection = true;
        }
        buf.extend_from_slice(k.as_bytes());
        buf.extend_from_slice(b": ");
        buf.extend_from_slice(v.as_bytes());
        buf.extend_from_slice(b"\r\n");
    }
    if !saw_connection {
        buf.extend_from_slice(b"Connection: close\r\n");
    }
    buf.extend_from_slice(b"\r\n");
    buf.extend_from_slice(&request.body);
    buf
}

/// Re-serialize the parsed request, send it to `upstream`, and bridge bytes
/// in both directions until either side closes the connection.
///
/// The serialized request preserves the original request line and headers,
/// adding `Connection: close` if the client did not send one (we don't
/// support keep-alive in v1).
///
/// # Errors
///
/// Returns [`ProxyError::UpstreamConnect`] if the TCP connect to `upstream`
/// fails, or [`ProxyError::Io`] if the initial header write fails. Bridging
/// errors (the inevitable disconnect when either side closes) are swallowed
/// — they are not actionable.
pub(crate) async fn forward_http(
    mut client: TcpStream,
    upstream: SocketAddr,
    request: &ParsedHttp,
) -> Result<(), ProxyError> {
    let mut up = TcpStream::connect(upstream)
        .await
        .map_err(|_| ProxyError::UpstreamConnect(upstream))?;

    let buf = serialize_request(request);
    up.write_all(&buf).await?;

    // Bridge until either side closes. Errors here are inevitable
    // disconnects; not actionable.
    let _ = tokio::io::copy_bidirectional(&mut client, &mut up).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> ParsedHttp {
        ParsedHttp {
            method: "POST".into(),
            path: "/api/x".into(),
            headers: vec![
                ("Host".into(), "y".into()),
                ("Content-Length".into(), "5".into()),
            ],
            body: b"hello".to_vec(),
        }
    }

    #[test]
    fn serialize_includes_request_line_headers_and_body() {
        let buf = serialize_request(&req());
        let s = std::str::from_utf8(&buf).unwrap();
        assert!(s.starts_with("POST /api/x HTTP/1.1\r\n"), "actual: {s:?}");
        assert!(s.contains("Host: y\r\n"));
        assert!(s.contains("Content-Length: 5\r\n"));
        assert!(s.ends_with("\r\n\r\nhello"));
    }

    #[test]
    fn serialize_adds_connection_close_when_client_did_not() {
        let buf = serialize_request(&req());
        let s = std::str::from_utf8(&buf).unwrap();
        assert!(s.contains("Connection: close\r\n"));
    }

    #[test]
    fn serialize_preserves_client_connection_header() {
        let mut r = req();
        r.headers.push(("Connection".into(), "keep-alive".into()));
        let buf = serialize_request(&r);
        let s = std::str::from_utf8(&buf).unwrap();
        assert!(s.contains("Connection: keep-alive"));
        // No injected Connection: close
        assert_eq!(s.matches("Connection:").count(), 1);
    }
}
