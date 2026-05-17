//! Helper for constructing `LogEntry::sample` from an inspected packet.

use minos_core::Packet;

const DEFAULT_SAMPLE_CAP: usize = 1024;

/// Produce a UTF-8-or-bytes sample suitable for `LogEntry::sample`. Always
/// caps the result at `cap` bytes; never panics.
pub fn sample_from_packet(packet: &Packet<'_>, cap: usize) -> Vec<u8> {
    match packet {
        Packet::Raw { bytes, .. } => bytes.iter().take(cap).copied().collect(),
        Packet::Http { req, .. } => {
            let mut buf = Vec::with_capacity(cap);
            buf.extend_from_slice(req.method.as_bytes());
            buf.push(b' ');
            buf.extend_from_slice(req.path.as_bytes());
            buf.push(b'\n');
            for (k, v) in &req.headers {
                buf.extend_from_slice(k.as_bytes());
                buf.extend_from_slice(b": ");
                buf.extend_from_slice(v.as_bytes());
                buf.push(b'\n');
                if buf.len() >= cap {
                    buf.truncate(cap);
                    return buf;
                }
            }
            buf.push(b'\n');
            let remaining = cap.saturating_sub(buf.len());
            buf.extend_from_slice(&req.body[..req.body.len().min(remaining)]);
            buf
        }
    }
}

/// Default sample cap (1024 bytes). Public so the executor can reference it
/// without duplicating the constant.
#[must_use]
pub fn default_sample_cap() -> usize {
    DEFAULT_SAMPLE_CAP
}

#[cfg(test)]
mod tests {
    use super::*;
    use minos_core::{Direction, Packet, ParsedHttp};

    #[test]
    fn raw_sample_is_capped() {
        let bytes = vec![b'a'; 100];
        let p = Packet::Raw {
            bytes: &bytes,
            direction: Direction::Inbound,
        };
        let s = sample_from_packet(&p, 16);
        assert_eq!(s.len(), 16);
        assert!(s.iter().all(|b| *b == b'a'));
    }

    #[test]
    fn http_sample_is_request_line_plus_body_truncated() {
        let req = ParsedHttp {
            method: "GET".into(),
            path: "/x".into(),
            headers: vec![("Host".into(), "y".into())],
            body: b"payload".to_vec(),
        };
        let p = Packet::Http {
            req: &req,
            direction: Direction::Inbound,
        };
        let s = sample_from_packet(&p, 1024);
        let s = std::str::from_utf8(&s).unwrap();
        assert!(s.contains("GET /x"));
        assert!(s.contains("payload"));
    }
}
