//! HTTP/1.1 protocol handler: request reader, block-response writer, and the
//! [`HttpHandler`] implementation of [`crate::ProtocolHandler`].

use minos_core::{BlockResponse, ParsedHttp};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::ProxyError;

/// HTTP/1.1 implementation of [`crate::ProtocolHandler`]. One instance is
/// reusable across many connections; it carries no per-connection state.
#[derive(Debug, Clone, Copy)]
pub struct HttpHandler;

#[async_trait::async_trait]
impl crate::ProtocolHandler for HttpHandler {
    async fn handle(
        &self,
        mut client: tokio::net::TcpStream,
        ctx: &crate::ServiceContext,
        bus: &minos_config::Bus,
    ) -> Result<(), ProxyError> {
        // 1. Parse one inbound request.
        let parsed = match read_request(&mut client, ctx.max_body_bytes).await {
            Ok(p) => p,
            Err(e) => {
                // Tell the client we couldn't parse, then close.
                let resp = minos_core::BlockResponse::HttpStatus {
                    status: 400,
                    body: format!("bad request: {e}").into_bytes(),
                    headers: vec![],
                };
                let _ = write_block_response(&mut client, &resp).await;
                return Err(e);
            }
        };

        // 2. Run the pipeline.
        let packet = minos_core::Packet::Http {
            req: &parsed,
            direction: minos_core::Direction::Inbound,
        };
        let pipeline =
            crate::handler::pipeline_snapshot(bus, &ctx.service_name).unwrap_or_default();
        let verdict = crate::execute(&packet, &ctx.service_name, &pipeline, &bus.log);

        if let minos_core::Verdict::Block { .. } = verdict {
            write_block_response(&mut client, &ctx.block_response).await?;
            return Ok(());
        }

        // 3. Forward to upstream.
        crate::forwarder::forward_http(client, ctx.upstream, &parsed).await
    }
}

pub(crate) const MAX_HEADER_BYTES: usize = 16 * 1024;
pub(crate) const MAX_HEADERS: usize = 64;

/// Read one HTTP/1.1 request from `stream`, returning the parsed form.
///
/// * Headers are read until `\r\n\r\n` or `MAX_HEADER_BYTES` (16 KB).
/// * Bodies are read up to the declared `Content-Length`. A `Content-Length`
///   greater than `max_body_bytes` is rejected with [`ProxyError::BodyTooLarge`]
///   before any body bytes are consumed.
/// * `Transfer-Encoding: chunked` is rejected — chunked decoding is out of
///   scope for v1 and silently sliding past it would let exploit bytes
///   through unfiltered.
///
/// # Errors
///
/// Returns [`ProxyError::HttpParse`] for malformed requests, oversized
/// headers, missing method/path, non-UTF8 header values, non-numeric or
/// missing `Content-Length` when a body is present, or `Transfer-Encoding`
/// other than identity. Returns [`ProxyError::BodyTooLarge`] when the
/// declared body exceeds `max_body_bytes`. Returns [`ProxyError::Io`] for
/// underlying socket failures.
pub(crate) async fn read_request<R>(
    mut stream: R,
    max_body_bytes: usize,
) -> Result<ParsedHttp, ProxyError>
where
    R: AsyncRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let headers_end = loop {
        let mut tmp = [0u8; 4096];
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            if buf.is_empty() {
                return Err(ProxyError::HttpParse("eof before any bytes".into()));
            }
            return Err(ProxyError::HttpParse("eof before headers complete".into()));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > MAX_HEADER_BYTES {
            return Err(ProxyError::HttpParse(format!(
                "headers exceed {MAX_HEADER_BYTES}B"
            )));
        }
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut req = httparse::Request::new(&mut headers);
        match req.parse(&buf) {
            Ok(httparse::Status::Complete(end)) => break end,
            Ok(httparse::Status::Partial) => {}
            Err(e) => return Err(ProxyError::HttpParse(e.to_string())),
        }
    };

    // Re-parse to extract owned method/path/headers.
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut headers);
    req.parse(&buf)
        .map_err(|e| ProxyError::HttpParse(e.to_string()))?;

    let method = req
        .method
        .ok_or_else(|| ProxyError::HttpParse("no method".into()))?
        .to_string();
    let path = req
        .path
        .ok_or_else(|| ProxyError::HttpParse("no path".into()))?
        .to_string();

    let mut owned_headers: Vec<(String, String)> = Vec::with_capacity(req.headers.len());
    let mut content_length: usize = 0;
    let mut content_length_seen = false;
    for h in req.headers.iter() {
        let k = h.name.to_string();
        let v = std::str::from_utf8(h.value)
            .map_err(|_| ProxyError::HttpParse("non-utf8 header".into()))?
            .to_string();
        if k.eq_ignore_ascii_case("transfer-encoding") {
            return Err(ProxyError::HttpParse(format!(
                "unsupported transfer-encoding: {v}"
            )));
        }
        if k.eq_ignore_ascii_case("content-length") {
            content_length = v
                .trim()
                .parse()
                .map_err(|_| ProxyError::HttpParse("non-numeric content-length".into()))?;
            content_length_seen = true;
        }
        owned_headers.push((k, v));
    }

    if content_length_seen && content_length > max_body_bytes {
        return Err(ProxyError::BodyTooLarge {
            limit: max_body_bytes,
            got: content_length,
        });
    }

    let mut body = buf.split_off(headers_end);
    while body.len() < content_length {
        let mut tmp = [0u8; 4096];
        let need = (content_length - body.len()).min(tmp.len());
        let n = stream.read(&mut tmp[..need]).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);

    Ok(ParsedHttp {
        method,
        path,
        headers: owned_headers,
        body,
    })
}

/// Write an HTTP block response to `stream` and shut it down.
///
/// `BlockResponse::Close` for HTTP is treated as a fallback to the default
/// 403; raw TCP handlers interpret `Close` differently (they just shut the
/// socket without writing anything — see `tcp_handler`).
///
/// # Errors
///
/// Returns [`ProxyError::Io`] if the write or shutdown fails.
pub(crate) async fn write_block_response<W>(
    mut stream: W,
    response: &BlockResponse,
) -> Result<(), ProxyError>
where
    W: AsyncWrite + Unpin,
{
    let (status, body, extra_headers): (u16, &[u8], &[(String, String)]) = match response {
        BlockResponse::HttpStatus {
            status,
            body,
            headers,
        } => (*status, body.as_slice(), headers.as_slice()),
        BlockResponse::Close => {
            // Inline the default to avoid a recursive async call.
            (403u16, b"".as_slice(), [].as_slice())
        }
    };
    let reason = http_reason(status);
    let mut out = Vec::with_capacity(128 + body.len());
    out.extend_from_slice(format!("HTTP/1.1 {status} {reason}\r\n").as_bytes());
    out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    out.extend_from_slice(b"Connection: close\r\n");
    for (k, v) in extra_headers {
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(v.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    stream.write_all(&out).await?;
    stream.shutdown().await?;
    Ok(())
}

fn http_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        418 => "I'm a teapot",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Forbidden",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_simple_get() {
        use tokio::io::AsyncWriteExt;
        let (mut a, b) = tokio::io::duplex(4096);
        a.write_all(b"GET /x HTTP/1.1\r\nHost: y\r\n\r\n")
            .await
            .unwrap();
        drop(a);
        let parsed = read_request(b, 1024).await.unwrap();
        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.path, "/x");
        assert_eq!(
            parsed
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("host"))
                .unwrap()
                .1,
            "y"
        );
        assert!(parsed.body.is_empty());
    }

    #[tokio::test]
    async fn reads_post_with_content_length() {
        use tokio::io::AsyncWriteExt;
        let (mut a, b) = tokio::io::duplex(4096);
        a.write_all(b"POST / HTTP/1.1\r\nHost: y\r\nContent-Length: 5\r\n\r\nhello")
            .await
            .unwrap();
        drop(a);
        let parsed = read_request(b, 1024).await.unwrap();
        assert_eq!(parsed.body, b"hello");
    }

    #[tokio::test]
    async fn rejects_body_over_limit() {
        use tokio::io::AsyncWriteExt;
        let (mut a, b) = tokio::io::duplex(4096);
        a.write_all(b"POST / HTTP/1.1\r\nHost: y\r\nContent-Length: 100\r\n\r\n")
            .await
            .unwrap();
        drop(a);
        let err = read_request(b, 10).await.unwrap_err();
        assert!(matches!(
            err,
            crate::ProxyError::BodyTooLarge {
                limit: 10,
                got: 100
            }
        ));
    }

    #[tokio::test]
    async fn rejects_chunked() {
        use tokio::io::AsyncWriteExt;
        let (mut a, b) = tokio::io::duplex(4096);
        a.write_all(b"POST / HTTP/1.1\r\nHost: y\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .unwrap();
        drop(a);
        let err = read_request(b, 1024).await.unwrap_err();
        assert!(matches!(err, crate::ProxyError::HttpParse(_)));
    }

    #[tokio::test]
    async fn writes_403_default() {
        use tokio::io::AsyncReadExt;
        let (a, mut b) = tokio::io::duplex(4096);
        write_block_response(a, &BlockResponse::default())
            .await
            .unwrap();
        let mut out = Vec::new();
        b.read_to_end(&mut out).await.unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 403"));
        assert!(s.contains("Content-Length: 0"));
        assert!(s.contains("Connection: close"));
    }

    #[tokio::test]
    async fn writes_custom_status_and_body() {
        use tokio::io::AsyncReadExt;
        let (a, mut b) = tokio::io::duplex(4096);
        let resp = BlockResponse::HttpStatus {
            status: 418,
            body: b"teapot".to_vec(),
            headers: vec![("X-Reason".into(), "blocked".into())],
        };
        write_block_response(a, &resp).await.unwrap();
        let mut out = Vec::new();
        b.read_to_end(&mut out).await.unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 418"));
        assert!(s.contains("X-Reason: blocked"));
        assert!(s.contains("Content-Length: 6"));
        assert!(s.ends_with("teapot"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn handler_passes_request_to_upstream_when_pipeline_is_empty() {
        use crate::ProtocolHandler as _;
        use tokio::io::AsyncWriteExt;

        // Spawn fixture upstream that responds with HTTP/1.1 200 OK / body=ok.
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = upstream.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut s, &mut buf)
                .await
                .unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
            let _ = s.shutdown().await;
        });

        // Spawn proxy.
        let (bus, _rx) =
            minos_config::new_bus(minos_config::RuleSet::empty_for(minos_config::Config {
                services: vec![minos_config::ServiceConfig {
                    name: "svc".into(),
                    mode: minos_core::ProxyMode::Reverse {
                        bind: "127.0.0.1:0".parse().unwrap(),
                        upstream: upstream_addr,
                    },
                    protocol: minos_core::ProtocolKind::Http,
                    pipeline: vec![],
                    block_response_override: None,
                    max_body_bytes: 1024,
                }],
                ..minos_config::Config::default()
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = listener.local_addr().unwrap();
        let bus2 = bus.clone();
        tokio::spawn(async move {
            let (client, _) = listener.accept().await.unwrap();
            let ctx = crate::ServiceContext {
                service_name: "svc".into(),
                upstream: upstream_addr,
                block_response: minos_core::BlockResponse::default(),
                max_body_bytes: 1024,
            };
            HttpHandler.handle(client, &ctx, &bus2).await.unwrap();
        });

        // Drive the client.
        let mut client = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: y\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut out = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut client, &mut out).await;
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 200"), "got: {s:?}");
        assert!(s.ends_with("ok"), "got: {s:?}");
    }
}
