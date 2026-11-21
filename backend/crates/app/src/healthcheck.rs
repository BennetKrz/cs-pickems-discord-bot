//! `app healthcheck`: asks the running app's `GET /healthz` and succeeds only
//! on 200. Used by the Docker `HEALTHCHECK`, so the image needs no curl (T16).
//!
//! It needs no configuration besides the listen address, so a broken or
//! incomplete environment never makes the probe itself fail.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// The whole probe (connect, send, read) must finish within this.
pub const TIMEOUT: Duration = Duration::from_secs(3);

/// Default of `BIND_ADDR`, as in [`crate::config`].
const DEFAULT_BIND: &str = "0.0.0.0:8080";

/// The address to probe: `addr` if given, else `BIND_ADDR`, else the default.
/// A wildcard bind address (`0.0.0.0`, `[::]`) is probed on loopback.
pub fn target(addr: Option<SocketAddr>, bind_addr: Option<&str>) -> Result<SocketAddr, String> {
    let addr = match addr {
        Some(a) => a,
        None => {
            let raw = bind_addr
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_BIND);
            raw.parse::<SocketAddr>()
                .map_err(|e| format!("BIND_ADDR: expected host:port, got `{raw}` ({e})"))?
        }
    };
    let ip = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    Ok(SocketAddr::new(ip, addr.port()))
}

/// `GET /healthz` on `addr`. `Ok` with the status line on 200, else an
/// error describing what went wrong.
pub fn check(addr: SocketAddr, timeout: Duration) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("cannot connect to {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|e| format!("socket setup: {e}"))?;
    write!(
        stream,
        "GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| format!("cannot send the request: {e}"))?;

    // The status line is all we need; the body is small but read a bounded
    // amount anyway.
    let mut buf = Vec::with_capacity(512);
    if let Err(e) = stream.take(4096).read_to_end(&mut buf) {
        // A reset after the status line arrived still tells us the answer.
        if !buf.contains(&b'\n') {
            return Err(format!("cannot read the response: {e}"));
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let status_line = text.lines().next().unwrap_or_default().trim().to_owned();
    match status_line.split_whitespace().nth(1) {
        Some("200") => Ok(status_line),
        Some(_) => Err(format!("unhealthy: {status_line}")),
        None => Err("no HTTP response".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn target_resolution() {
        let t = |addr: Option<&str>, bind: Option<&str>| {
            target(addr.map(|a| a.parse().unwrap()), bind).map(|a| a.to_string())
        };
        assert_eq!(t(None, None).unwrap(), "127.0.0.1:8080");
        assert_eq!(t(None, Some(" ")).unwrap(), "127.0.0.1:8080");
        assert_eq!(t(None, Some("0.0.0.0:9000")).unwrap(), "127.0.0.1:9000");
        assert_eq!(t(None, Some("[::]:9000")).unwrap(), "[::1]:9000");
        assert_eq!(t(None, Some("10.1.2.3:80")).unwrap(), "10.1.2.3:80");
        assert_eq!(
            t(Some("127.0.0.1:1234"), Some("0.0.0.0:9000")).unwrap(),
            "127.0.0.1:1234"
        );
        assert!(
            t(None, Some("localhost"))
                .unwrap_err()
                .contains("BIND_ADDR")
        );
    }

    /// Answers one connection with `response`.
    fn one_shot(response: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            // Read the whole request first, so closing doesn't reset.
            let mut req = Vec::new();
            let mut chunk = [0u8; 256];
            while !req.ends_with(b"\r\n\r\n") {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&chunk[..n]),
                }
            }
            let _ = s.write_all(response.as_bytes());
        });
        addr
    }

    #[test]
    fn ok_only_on_200() {
        let ok = one_shot("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}");
        assert_eq!(check(ok, TIMEOUT).unwrap(), "HTTP/1.1 200 OK");

        let down = one_shot("HTTP/1.1 503 Service Unavailable\r\n\r\n");
        assert!(check(down, TIMEOUT).unwrap_err().contains("503"));

        let junk = one_shot("");
        assert_eq!(check(junk, TIMEOUT).unwrap_err(), "no HTTP response");
    }

    #[test]
    fn nothing_listening_is_an_error() {
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(check(addr, TIMEOUT).unwrap_err().contains("cannot connect"));
    }
}
