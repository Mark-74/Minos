//! Transparent-mode helper: discover the original packet destination via
//! `SO_ORIGINAL_DST` after an iptables REDIRECT.
//!
//! On Linux this wraps [`socket2::SockRef::original_dst`], which itself
//! wraps the `getsockopt(SOL_IP, SO_ORIGINAL_DST)` syscall. On other
//! platforms `original_dst` returns [`ProxyError::TransparentLookup`]
//! unconditionally — transparent mode is Linux-only.

use std::net::SocketAddr;

use tokio::net::TcpStream;

use crate::ProxyError;

/// Look up the pre-NAT destination of `client`. Only meaningful on Linux
/// when iptables `-j REDIRECT` directed the connection here.
///
/// # Errors
///
/// Returns [`ProxyError::TransparentLookup`] if the lookup fails (kernel
/// rejected the option, no IP-family destination, non-Linux platform).
#[cfg(target_os = "linux")]
pub fn original_dst(client: &TcpStream) -> Result<SocketAddr, ProxyError> {
    use socket2::SockRef;
    let r = SockRef::from(client);
    // Try IPv4 first, then IPv6. socket2's original_dst_* return SockAddr,
    // which needs to be converted to std::net::SocketAddr via as_socket_*().
    r.original_dst_v4()
        .and_then(|sa| {
            sa.as_socket_ipv4()
                .ok_or_else(|| std::io::Error::other("SockAddr not IPv4"))
                .map(SocketAddr::V4)
        })
        .or_else(|_| {
            r.original_dst_v6().and_then(|sa| {
                sa.as_socket_ipv6()
                    .ok_or_else(|| std::io::Error::other("SockAddr not IPv6"))
                    .map(SocketAddr::V6)
            })
        })
        .map_err(|e| ProxyError::TransparentLookup(e.to_string()))
}

/// Non-Linux stub. Transparent mode requires Linux's `SO_ORIGINAL_DST`
/// socket option.
#[cfg(not(target_os = "linux"))]
pub fn original_dst(_: &TcpStream) -> Result<SocketAddr, ProxyError> {
    Err(ProxyError::TransparentLookup(
        "transparent mode requires Linux".into(),
    ))
}
