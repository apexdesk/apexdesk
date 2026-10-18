//! The requests signing makes: time-stamps, OCSP and CRLs. Plain HTTP is
//! spoken here; HTTPS, and anything through a proxy or with the session's
//! sign-in ([`apex_io::net`]), goes through the system's curl (Windows
//! 10+, macOS and Linux have it). [`Net`] lets the tests answer without a
//! network.

use std::io::{Read, Write};
use web_time::Duration;

use crate::Error;

const TIMEOUT: Duration = Duration::from_secs(20);

/// Where requests go.
pub trait Net: Sync {
    /// POSTs `body` to `url`; the reply's body.
    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<Vec<u8>, Error>;
    /// GETs `url`; the reply's body.
    fn get(&self, url: &str) -> Result<Vec<u8>, Error>;
}

/// The network itself.
pub struct Http;

impl Net for Http {
    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<Vec<u8>, Error> {
        post(url, content_type, body)
    }

    fn get(&self, url: &str) -> Result<Vec<u8>, Error> {
        get(url)
    }
}

/// POSTs `body` to `url` and returns the reply's body.
pub fn post(url: &str, content_type: &str, body: &[u8]) -> Result<Vec<u8>, Error> {
    request(url, Some((content_type, body)), 0, true)
}

/// GETs `url` (following a few redirections).
pub fn get(url: &str) -> Result<Vec<u8>, Error> {
    request(url, None, 0, true)
}

fn request(url: &str, post: Option<(&str, &[u8])>, hops: u32, lan: bool) -> Result<Vec<u8>, Error> {
    let url = url.trim();
    // The addresses come from certificates and replies anyone can make: a
    // line break would add lines to the request (to another service on
    // the host), and none of them belongs in an address.
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::new("the address has spaces or control characters"));
    }
    // A backslash in the host part is read as a separator by some and as
    // part of the name by others: the host checked and the one reached
    // could differ.
    if url.split_once("://").map_or(url, |(_, r)| r).split(['/', '?', '#']).next().is_some_and(|a| a.contains('\\')) {
        return Err(Error::new("the address has a backslash in its host"));
    }
    // A server outside the local network can't send us into it.
    let lan = lan && (hops > 0 || not_private(url).is_err());
    // Where a reply sends us is anyone's choice (addresses taken from
    // documents are checked where they're read; a time-stamp server the
    // user set may well be on this computer).
    if hops > 0 {
        not_local(url)?;
        if !lan {
            not_private(url)?;
        }
    }
    // A name that didn't resolve when it was checked mustn't be looked up
    // again on the way (to an address nobody checked): without a proxy,
    // requests go to the pinned addresses or nowhere.
    if apex_io::net::proxy_for(url).is_none() && host_of(url).1.parse::<std::net::IpAddr>().is_err() && addresses(url).1.is_empty() {
        return Err(Error::new(format!("{}: not found", host_of(url).0)));
    }
    if url.starts_with("http://") && (apex_io::net::proxy_for(url).is_some() || apex_io::net::signs_in(url)) {
        return curl(url, post);
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return match http(rest, post)? {
            Reply::Body(b) => Ok(b),
            Reply::Moved(to) if hops < 4 && post.is_none() => request(&to, post, hops + 1, lan),
            Reply::Moved(_) => Err(Error::new("too many redirections")),
        };
    }
    if url.starts_with("https://") {
        return curl(url, post);
    }
    Err(Error::new(format!("{url}: the address must start with http:// or https://")))
}

/// Refuses an address of this computer (its loopback, link-local or
/// unspecified addresses): where certificates in a document or a server's
/// redirection send requests, the computer's own services would answer.
/// An address that doesn't resolve passes (its request fails anyway).
/// The IPv4 address an IPv6 one carries to an IPv4 network: mapped,
/// compatible, NAT64 (64:ff9b::/96) or 6to4 (2002::/16).
fn carried_v4(v: &std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let s = v.segments();
    let v4 = |hi: u16, lo: u16| std::net::Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
    if s[0] == 0x2002 {
        return Some(v4(s[1], s[2]));
    }
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] || (s[..6] == [0; 6] && (s[6] != 0 || s[7] > 1)) {
        return Some(v4(s[6], s[7]));
    }
    v.to_ipv4_mapped()
}

pub(crate) fn not_local(url: &str) -> Result<(), Error> {
    use std::net::IpAddr;
    let (host_port, ips) = addresses(url);
    let local = |ip: &IpAddr| match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xffc0) == 0xfe80 || carried_v4(v6).is_some_and(|v4| v4.is_loopback() || v4.is_unspecified() || v4.is_link_local()),
    };
    if ips.iter().any(local) {
        return Err(Error::new(format!("{host_port}: an address of this computer")));
    }
    Ok(())
}

/// [`not_local`], and also refuses the private networks' addresses
/// (10/8, 172.16/12, 192.168/16, 100.64/10, fc00::/7): for addresses no
/// authority vouches for yet, such as where to download a missing issuer.
pub(crate) fn not_private(url: &str) -> Result<(), Error> {
    use std::net::IpAddr;
    not_local(url)?;
    let (host_port, ips) = addresses(url);
    let v4 = |v: std::net::Ipv4Addr| v.is_private() || (v.octets()[0] == 100 && v.octets()[1] & 0xC0 == 64);
    let private = |ip: &IpAddr| match ip {
        IpAddr::V4(v) => v4(*v),
        IpAddr::V6(v) => (v.segments()[0] & 0xfe00) == 0xfc00 || carried_v4(v).is_some_and(v4),
    };
    if ips.iter().any(private) {
        return Err(Error::new(format!("{host_port}: an address of the local network")));
    }
    Ok(())
}

/// The addresses each host was checked at, which its requests then go
/// to: a name that resolves elsewhere the next time (DNS rebinding)
/// can't take a checked request to this computer or the local network.
static PINNED: std::sync::Mutex<std::collections::BTreeMap<String, Vec<std::net::IpAddr>>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// An address's `host:port` as written, its host (lowercase, without
/// brackets) and its port.
fn host_of(url: &str) -> (String, String, u16) {
    let url = url.trim();
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, port) = match host_port.strip_prefix('[') {
        Some(r) => {
            let (h, after) = r.split_once(']').unwrap_or((r, ""));
            (h, after.strip_prefix(':'))
        }
        None => match host_port.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (host_port, None),
        },
    };
    let default = if url.get(..6).is_some_and(|s| s.eq_ignore_ascii_case("https:")) { 443 } else { 80 };
    (host_port.to_string(), host.to_ascii_lowercase(), port.and_then(|p| p.parse().ok()).unwrap_or(default))
}

/// The addresses of an address's host: the ones it was checked at, or
/// resolved now and kept.
fn addresses(url: &str) -> (String, Vec<std::net::IpAddr>) {
    use std::net::ToSocketAddrs;
    let (host_port, host, _) = host_of(url);
    let Ok(mut pinned) = PINNED.lock() else { return (host_port, Vec::new()) };
    if let Some(ips) = pinned.get(&host) {
        return (host_port, ips.clone());
    }
    let ips: Vec<std::net::IpAddr> = (host.as_str(), 0).to_socket_addrs().map(|a| a.map(|s| s.ip()).collect()).unwrap_or_default();
    if !ips.is_empty() && host.parse::<std::net::IpAddr>().is_err() {
        pinned.insert(host, ips.clone());
    }
    (host_port, ips)
}

/// Where a request to `url` connects: the checked addresses of its host,
/// if it was checked.
fn pinned(url: &str) -> Option<(String, u16, Vec<std::net::IpAddr>)> {
    let (_, host, port) = host_of(url);
    let ips = PINNED.lock().ok()?.get(&host)?.clone();
    Some((host, port, ips))
}

enum Reply {
    Body(Vec<u8>),
    Moved(String),
}

fn http(rest: &str, post: Option<(&str, &[u8])>) -> Result<Reply, Error> {
    let (host_port, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(h, p)| (h, format!("/{p}")));
    let addr = if host_port.contains(':') { host_port.to_string() } else { format!("{host_port}:80") };
    let host = host_port.split(':').next().unwrap_or(host_port);
    let net = |e: std::io::Error| Error::new(format!("{host}: {e}"));
    use std::net::ToSocketAddrs;
    let socks: Vec<std::net::SocketAddr> = match pinned(&format!("http://{rest}")) {
        Some((_, port, ips)) => ips.into_iter().map(|ip| std::net::SocketAddr::new(ip, port)).collect(),
        None => addr.to_socket_addrs().map_err(net)?.collect(),
    };
    let mut s = None;
    let mut last = None;
    for sock in &socks {
        match std::net::TcpStream::connect_timeout(sock, TIMEOUT) {
            Ok(c) => {
                s = Some(c);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let mut s = match (s, last) {
        (Some(s), _) => s,
        (None, Some(e)) => return Err(net(e)),
        (None, None) => return Err(Error::new(format!("{host}: not found"))),
    };
    s.set_read_timeout(Some(TIMEOUT)).map_err(net)?;
    s.set_write_timeout(Some(TIMEOUT)).map_err(net)?;
    let head = match post {
        Some((ct, body)) => format!("POST {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nUser-Agent: ApexDesk\r\nConnection: close\r\n\r\n", body.len()),
        None => format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nUser-Agent: ApexDesk\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(head.as_bytes()).map_err(net)?;
    if let Some((_, body)) = post {
        s.write_all(body).map_err(net)?;
    }
    // As much as curl is let read (CRLs of big authorities run to tens of megabytes).
    const MAX: u64 = 256 << 20;
    let mut reply = Vec::new();
    Read::by_ref(&mut s).take(MAX + 1).read_to_end(&mut reply).map_err(net)?;
    if reply.len() as u64 > MAX {
        return Err(Error::new("the server's reply is too large"));
    }
    parse_http(&reply)
}

/// The body of an HTTP reply (plain or chunked), or where it moved.
fn parse_http(reply: &[u8]) -> Result<Reply, Error> {
    let end = reply.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| Error::new("bad HTTP reply"))?;
    let head = String::from_utf8_lossy(&reply[..end]);
    let status: u32 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let header = |name: &str| head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()));
    if matches!(status, 301 | 302 | 303 | 307 | 308)
        && let Some(to) = header("location")
    {
        return Ok(Reply::Moved(to));
    }
    if status != 200 {
        return Err(Error::new(format!("the server answered HTTP {status}")));
    }
    let body = &reply[end + 4..];
    if !header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
        return Ok(Reply::Body(body.to_vec()));
    }
    let mut out = Vec::new();
    let mut p = 0;
    loop {
        let line_end = body.get(p..).ok_or_else(|| Error::new("bad HTTP reply"))?.windows(2).position(|w| w == b"\r\n").ok_or_else(|| Error::new("bad HTTP reply"))? + p;
        let size_text = String::from_utf8_lossy(&body[p..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16).map_err(|_| Error::new("bad HTTP reply"))?;
        p = line_end + 2;
        if size == 0 {
            return Ok(Reply::Body(out));
        }
        let chunk_end = p.checked_add(size).ok_or_else(|| Error::new("bad HTTP reply"))?;
        out.extend_from_slice(body.get(p..chunk_end).ok_or_else(|| Error::new("bad HTTP reply"))?);
        p = chunk_end + 2;
    }
}

fn curl(url: &str, post: Option<(&str, &[u8])>) -> Result<Vec<u8>, Error> {
    // CRLs of big authorities run to tens of megabytes.
    // Without following redirections: curl's own wouldn't be checked.
    // To the checked address, not one curl would look up again.
    let resolve = pinned(url).and_then(|(host, port, ips)| {
        let ip = ips.first()?;
        Some(if ip.is_ipv6() { format!("{host}:{port}:[{ip}]") } else { format!("{host}:{port}:{ip}") })
    });
    apex_io::net::curl_pinned(url, post, 30, 256 << 20, false, resolve.as_deref()).map_err(Error::new)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_hosts_keep_their_addresses() {
        assert_eq!(host_of("https://u@[::1]:8443/x"), ("[::1]:8443".to_string(), "::1".to_string(), 8443));
        assert_eq!(host_of("HTTPS://Example.COM/x").1, "example.com");
        assert_eq!(host_of("http://example.com/x").2, 80);
        assert!(not_local("http://localhost:8080/ocsp").is_err());
        let (host, port, ips) = pinned("https://localhost:8080/y").expect("pinned");
        assert_eq!((host.as_str(), port), ("localhost", 8080));
        assert!(ips.iter().all(|ip| ip.is_loopback()));
        // An address as written isn't looked up, so not kept.
        assert!(not_local("http://127.0.0.1/x").is_err());
        assert!(pinned("http://127.0.0.1/x").is_none());
    }

    #[test]
    fn addresses_from_documents() {
        for url in ["http://127.0.0.1:6379/x", "http://localhost/ocsp", "http://[::1]/c.crl", "http://169.254.169.254/latest", "http://u@0.0.0.0:80/", "http://[64:ff9b::7f00:1]/", "http://[2002:7f00:1::]/", "http://[::127.0.0.1]/"] {
            assert!(not_local(url).is_err(), "{url}");
        }
        assert!(not_local("http://203.0.113.9/ocsp").is_ok());
        assert!(not_local("http://unresolvable.invalid/ocsp").is_ok());
        for url in ["http://10.0.0.5/ca.crt", "http://192.168.1.1:8080/x", "http://[fd00::1]/x", "http://100.64.1.1/x", "http://127.0.0.1/x", "http://[64:ff9b::a00:5]/"] {
            assert!(not_private(url).is_err(), "{url}");
        }
        assert!(not_private("http://203.0.113.9/ca.crt").is_ok());
        let e = get("http://203.0.113.9/x\r\nFLUSHALL\r\n").unwrap_err();
        assert!(e.to_string().contains("control"), "{e}");
        let e = get("https://example.com\\@127.0.0.1/x").unwrap_err();
        assert!(e.to_string().contains("backslash"), "{e}");
    }

    fn body(r: &[u8]) -> Option<Vec<u8>> {
        match parse_http(r).ok()? {
            Reply::Body(b) => Some(b),
            Reply::Moved(_) => None,
        }
    }

    #[test]
    fn replies() {
        let r = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        assert_eq!(body(r).unwrap(), b"abcde");
        // Chunk sizes past the end, or overflowing, are refused, not a panic.
        assert!(body(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\na").is_none());
        assert!(body(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffff\r\nab").is_none());
        let r = b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nhi";
        assert_eq!(body(r).unwrap(), b"hi");
        assert!(parse_http(b"HTTP/1.1 404 Not Found\r\n\r\n").is_err());
        let r = b"HTTP/1.1 302 Found\r\nLocation: http://x/y.crl\r\n\r\n";
        assert!(matches!(parse_http(r), Ok(Reply::Moved(u)) if u == "http://x/y.crl"));
    }
}
