//! Downloads over HTTP(S) with ureq (TLS by rustls, so no system library
//! is needed on any platform), through the proxy of [`apex_io::net`].
//! Hosts that get the session's sign-in (Kerberos or NTLM), and proxies
//! that ask for more than a user and password, go through the system's
//! curl, which speaks Negotiate.

use crate::Error;

/// The body of a GET of `url`.
///
/// Where the user said yes to an address on the internet, nothing on the
/// way leads into this computer or the local network: not a redirection
/// (followed here, each checked), nor the name itself resolving there
/// (DNS rebinding: the addresses checked are the ones connected to).
#[cfg(feature = "web")]
pub fn get(url: &str) -> Result<Vec<u8>, Error> {
    use web_time::Duration;
    let url = url.trim();
    // An address the user gave on this computer or network stays usable:
    // one written as such (an address, localhost, a local name), not a name
    // whose DNS, the file maker's own, says so.
    let lan = names_inside(url);
    let curl = |at: &str| apex_io::net::curl_with(at, None, 90, super::MAX_BYTES as usize, lan).map_err(|e| Error::Network(format!("{at}: {e}")));
    if apex_io::net::signs_in(url) {
        return curl(url);
    }
    let proxy = match apex_io::net::proxy_for(url) {
        Some(p) => Some(ureq::Proxy::new(&p).map_err(|e| Error::Network(format!("{p}: {e}")))?),
        None => None,
    };
    let direct = proxy.is_none();
    // Through a proxy, names are looked up here too: one that resolves into
    // the network isn't reached through the proxy either.
    if !direct && !lan && resolves_inside(url) == Some(true) {
        return Err(Error::Network(format!("{url}: an address of this computer or the local network")));
    }
    let config = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(90))).max_redirects(0).user_agent(concat!("ApexDesk/", env!("CARGO_PKG_VERSION"))).proxy(proxy).build();
    // Through a proxy, the proxy looks names up (and is often on the local
    // network itself): redirections are checked here, as resolved here.
    let agent: ureq::Agent = if direct && !lan { ureq::Agent::with_parts(config, ureq::unversioned::transport::DefaultConnector::default(), Outside) } else { config.into() };
    let mut at = url.to_string();
    let mut hops = 0;
    let mut resp = loop {
        let resp = match agent.get(&at).header("Accept", "application/json, text/html, text/csv, application/xml, */*").call() {
            Ok(r) => r,
            // The proxy wants a sign-in ureq can't give (Negotiate or NTLM).
            Err(ureq::Error::StatusCode(407)) if hops == 0 => return curl(url),
            Err(ureq::Error::ConnectProxyFailed(m)) if hops == 0 && m.contains("407") => return curl(url),
            Err(ureq::Error::HostNotFound) if direct && !lan => return Err(Error::Network(format!("{at}: not found, or an address of this computer or the local network"))),
            Err(e) => return Err(Error::Network(format!("{at}: {e}"))),
        };
        if !resp.status().is_redirection() {
            break resp;
        }
        let Some(next) = resp.headers().get("location").and_then(|l| l.to_str().ok()).and_then(|l| join(&at, l)) else {
            return Err(Error::Network(format!("{at}: redirection without a valid address")));
        };
        hops += 1;
        if hops > 10 {
            return Err(Error::Network(format!("{url}: too many redirections")));
        }
        let into = if direct { resolves_inside(&next) != Some(false) } else { names_inside(&next) || resolves_inside(&next) == Some(true) };
        if !lan && into {
            return Err(Error::Network(format!("{url}: redirected into the local network ({next})")));
        }
        at = next;
    };
    resp.body_mut().with_config().limit(super::MAX_BYTES).read_to_vec().map_err(|e| Error::Network(format!("{url}: {e}")))
}

/// Looks names up leaving out this computer's and the local networks'
/// addresses (a name with only those isn't found).
#[cfg(feature = "web")]
#[derive(Debug)]
struct Outside;

#[cfg(feature = "web")]
impl ureq::unversioned::resolver::Resolver for Outside {
    fn resolve(&self, uri: &ureq::http::Uri, config: &ureq::config::Config, timeout: ureq::unversioned::transport::NextTimeout) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        use ureq::unversioned::resolver::DefaultResolver;
        let all = DefaultResolver::default().resolve(uri, config, timeout)?;
        let mut out = self.empty();
        for a in all.iter().filter(|a| !near(a.ip())) {
            out.push(*a);
        }
        if out.is_empty() { Err(ureq::Error::HostNotFound) } else { Ok(out) }
    }
}


/// Where the redirection `loc` from `base` leads: http(s) only.
#[cfg(feature = "web")]
fn join(base: &str, loc: &str) -> Option<String> {
    let loc = loc.trim();
    let lower = loc.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(loc.to_string());
    }
    if loc.contains("://") {
        return None;
    }
    let (scheme, rest) = base.split_once("://")?;
    if let Some(r) = loc.strip_prefix("//") {
        return Some(format!("{scheme}://{r}"));
    }
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let origin = &base[..scheme.len() + 3 + auth_end];
    if loc.starts_with('/') {
        return Some(format!("{origin}{loc}"));
    }
    let path = &rest[auth_end..];
    let path = path.split(['?', '#']).next().unwrap_or("");
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    Some(format!("{origin}{dir}/{loc}"))
}

/// An address of this computer or of a local network.
#[cfg(feature = "web")]
fn near(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    let v4 = |v: std::net::Ipv4Addr| {
        let o = v.octets();
        v.is_private() || v.is_loopback() || v.is_unspecified() || v.is_link_local() || (o[0] == 100 && o[1] & 0xC0 == 64) || (o[0] == 192 && o[1] == 0 && o[2] == 0) || (o[0] == 198 && o[1] & 0xFE == 18)
    };
    match ip {
        IpAddr::V4(v) => v4(v),
        IpAddr::V6(v) => {
            let s = v.segments();
            // NAT64 (64:ff9b::/96) and 6to4 (2002::/16) carry an IPv4
            // address a gateway may take into the local network.
            let embedded = |hi: u16, lo: u16| v4(std::net::Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8));
            v.is_loopback() || v.is_unspecified() || (s[0] & 0xffc0) == 0xfe80 || (s[0] & 0xfe00) == 0xfc00 || v.to_ipv4_mapped().is_some_and(v4) || (s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] && embedded(s[6], s[7])) || (s[0] == 0x2002 && embedded(s[1], s[2]))
        }
    }
}

/// Whether `url` names this computer or the local network as written:
/// an address of theirs, localhost, or a name without dots or under
/// .local, .lan, .internal, .intranet, .corp or .home.arpa.
#[cfg(feature = "web")]
fn names_inside(url: &str) -> bool {
    let host = host_of(url).0.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return near(ip);
    }
    host == "localhost" || host.ends_with(".localhost") || !host.contains('.') || [".local", ".lan", ".internal", ".intranet", ".corp", ".home.arpa"].iter().any(|s| host.ends_with(s))
}

/// `url`'s host (without brackets) and port.
#[cfg(feature = "web")]
fn host_of(url: &str) -> (String, u16) {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, port) = match host_port.strip_prefix('[') {
        Some(r) => {
            let (h, after) = r.split_once(']').unwrap_or((r, ""));
            (h, after.strip_prefix(':').and_then(|p| p.parse().ok()))
        }
        None => match host_port.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().ok()),
            None => (host_port, None),
        },
    };
    (host.to_string(), port.unwrap_or(if url.to_ascii_lowercase().starts_with("https") { 443 } else { 80 }))
}

/// Whether `url`'s host resolves to this computer or a local network
/// (any of its addresses); None when it doesn't resolve.
#[cfg(feature = "web")]
fn resolves_inside(url: &str) -> Option<bool> {
    use std::net::ToSocketAddrs;
    let (host, port) = host_of(url);
    let ips: Vec<_> = (host.as_str(), port).to_socket_addrs().ok()?.map(|a| a.ip()).collect();
    if ips.is_empty() { None } else { Some(ips.into_iter().any(near)) }
}

#[cfg(all(test, feature = "web"))]
mod tests {
    use super::*;

    #[test]
    fn redirections_resolve_and_local_hosts_are_inside() {
        assert_eq!(join("https://a.org/x/y.csv?q=1", "z.csv").as_deref(), Some("https://a.org/x/z.csv"));
        assert_eq!(join("https://a.org/x/y.csv", "/z").as_deref(), Some("https://a.org/z"));
        assert_eq!(join("https://a.org", "//b.org/z").as_deref(), Some("https://b.org/z"));
        assert_eq!(join("https://a.org", "file:///etc/passwd"), None);
        assert_eq!(resolves_inside("http://127.0.0.1:631/x"), Some(true));
        assert_eq!(resolves_inside("http://192.168.1.1/config"), Some(true));
        assert_eq!(resolves_inside("http://[::1]/"), Some(true));
        assert_eq!(resolves_inside("http://8.8.8.8/"), Some(false));
        assert_eq!(resolves_inside("http://no-such-host.invalid/"), None);
        // Local as written, not as some DNS says.
        assert!(names_inside("http://192.168.1.10/x") && names_inside("http://intranet/x") && names_inside("http://nas.local/") && names_inside("http://[::1]:8080/"));
        assert!(!names_inside("http://datos.example.com/x.csv") && !names_inside("http://8.8.8.8/"));
        // Addresses that reach a local network through a translation.
        for inside in ["http://[64:ff9b::c0a8:101]/", "http://[2002:a00:1::]/", "http://198.18.0.1/", "http://192.0.0.8/"] {
            assert!(names_inside(inside), "{inside}");
        }
        assert!(!names_inside("http://[64:ff9b::808:808]/") && !names_inside("http://[2002:808:808::]/"));
    }

    #[test]
    fn local_names_are_not_found_from_outside() {
        use ureq::unversioned::resolver::Resolver;
        let config = ureq::config::Config::default();
        let uri: ureq::http::Uri = "http://127.0.0.1:631/".parse().unwrap();
        assert!(Outside.resolve(&uri, &config, ureq::unversioned::transport::NextTimeout { after: ureq::unversioned::transport::time::Duration::NotHappening, reason: ureq::Timeout::Global }).is_err());
    }
}


#[cfg(not(feature = "web"))]
pub fn get(_url: &str) -> Result<Vec<u8>, Error> {
    Err(Error::NotBuilt("web"))
}
