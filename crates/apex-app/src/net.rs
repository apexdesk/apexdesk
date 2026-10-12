//! The network settings put in force: the proxy (the organization's or
//! the user's, else the system's), its exceptions and the sites that get
//! the session's sign-in, handed to [`apex_io::net`] for Get Data,
//! revocation checks and time stamps. Proxy auto-configuration
//! scripts (PAC) are not run: only a fixed proxy is read from the system.

use apex_io::net::{Config, proxy_url, split_list};

use crate::settings::Settings;

pub fn apply(s: &Settings) {
    apex_io::net::set_config(config(s, system));
}

/// The configuration for `s`, asking `system` for the system's proxy when
/// the setting is empty.
fn config(s: &Settings, system: fn() -> (Option<String>, Vec<String>)) -> Config {
    let (proxy, mut bypass) = match s.proxy.trim() {
        "" => system(),
        p if p.eq_ignore_ascii_case("direct") || p.eq_ignore_ascii_case("none") => (None, Vec::new()),
        p => (proxy_url(p), Vec::new()),
    };
    bypass.extend(split_list(&s.proxy_bypass));
    Config { proxy, bypass, sign_in: split_list(&s.sign_in_sites) }
}

/// The system's proxy and the hosts it leaves out: the Windows Internet
/// settings or the macOS network settings, then the usual environment
/// variables (`https_proxy`, `no_proxy`...).
pub fn system() -> (Option<String>, Vec<String>) {
    let os = os_proxy();
    if os.0.is_some() {
        return os;
    }
    from_env(|k| std::env::var(k).ok())
}

fn from_env(get: impl Fn(&str) -> Option<String>) -> (Option<String>, Vec<String>) {
    let first = |names: &[&str]| names.iter().find_map(|n| get(n).filter(|v| !v.trim().is_empty()));
    let proxy = first(&["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"]).and_then(|p| proxy_url(&p));
    let bypass = first(&["no_proxy", "NO_PROXY"]).map(|v| split_list(&v)).unwrap_or_default();
    (proxy, bypass)
}

#[cfg(windows)]
fn os_proxy() -> (Option<String>, Vec<String>) {
    use crate::policy::sources::registry::{HKCU, values};
    let v = values(HKCU, "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");
    let get = |name: &str| v.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).and_then(|(_, t)| t.first().cloned());
    from_windows(get("ProxyEnable").as_deref(), get("ProxyServer").as_deref(), get("ProxyOverride").as_deref())
}

#[cfg(any(windows, test))]
fn from_windows(enable: Option<&str>, server: Option<&str>, overrides: Option<&str>) -> (Option<String>, Vec<String>) {
    if enable.map(str::trim) != Some("1") {
        return (None, Vec::new());
    }
    // "<local>" stands for names without a dot, which `listed` can't say.
    let bypass = overrides.map(split_list).unwrap_or_default().into_iter().filter(|h| h != "<local>").collect();
    (server.and_then(proxy_url), bypass)
}

#[cfg(target_os = "macos")]
fn os_proxy() -> (Option<String>, Vec<String>) {
    match std::process::Command::new("scutil").arg("--proxy").output() {
        Ok(o) if o.status.success() => from_scutil(&String::from_utf8_lossy(&o.stdout)),
        _ => (None, Vec::new()),
    }
}

/// `scutil --proxy`'s dictionary: the HTTPS proxy, else the HTTP one, and
/// the exceptions list.
#[cfg(any(target_os = "macos", test))]
fn from_scutil(text: &str) -> (Option<String>, Vec<String>) {
    let mut keys = std::collections::HashMap::new();
    let mut bypass = Vec::new();
    let mut in_list = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("ExceptionsList") {
            in_list = true;
            continue;
        }
        if in_list && line == "}" {
            in_list = false;
            continue;
        }
        if let Some((k, v)) = line.split_once(" : ") {
            if in_list {
                bypass.push(v.trim().to_string());
            } else {
                keys.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    let proxy = ["HTTPS", "HTTP"].iter().find_map(|p| {
        let on = keys.get(&format!("{p}Enable")).is_some_and(|v| v == "1");
        let host = keys.get(&format!("{p}Proxy")).filter(|h| on && !h.is_empty())?;
        Some(match keys.get(&format!("{p}Port")) {
            Some(port) => format!("http://{host}:{port}"),
            None => format!("http://{host}"),
        })
    });
    (proxy, bypass)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn os_proxy() -> (Option<String>, Vec<String>) {
    (None, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_system() -> (Option<String>, Vec<String>) {
        (Some("http://sys:8080".into()), vec!["sys.local".into()])
    }

    #[test]
    fn settings_over_the_system() {
        let mut s = Settings::default();
        s.sign_in_sites = ".corp.example, intranet".into();
        let c = config(&s, no_system);
        assert_eq!(c.proxy.as_deref(), Some("http://sys:8080"));
        assert_eq!(c.bypass, ["sys.local"]);
        assert_eq!(c.sign_in, [".corp.example", "intranet"]);
        s.proxy = "direct".into();
        s.proxy_bypass = "files".into();
        let c = config(&s, no_system);
        assert_eq!((c.proxy, c.bypass), (None, vec!["files".to_string()]));
        s.proxy = "user:pw@proxy.corp:3128".into();
        assert_eq!(config(&s, no_system).proxy.as_deref(), Some("http://user:pw@proxy.corp:3128"));
    }

    #[test]
    fn system_sources() {
        let env = |k: &str| match k {
            "HTTP_PROXY" => Some("http://p:3128".to_string()),
            "no_proxy" => Some("localhost,.lan".to_string()),
            _ => None,
        };
        assert_eq!(from_env(env), (Some("http://p:3128".into()), vec!["localhost".into(), ".lan".into()]));
        assert_eq!(from_windows(Some("1"), Some("http=a:80;https=b:8443"), Some("*.corp;<local>")), (Some("http://b:8443".into()), vec!["*.corp".into()]));
        assert_eq!(from_windows(Some("0"), Some("a:80"), None).0, None);
        let scutil = "<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n    1 : 169.254/16\n  }\n  HTTPEnable : 1\n  HTTPPort : 3128\n  HTTPProxy : proxy.example.com\n  HTTPSEnable : 0\n}\n";
        assert_eq!(from_scutil(scutil), (Some("http://proxy.example.com:3128".into()), vec!["*.local".into(), "169.254/16".into()]));
    }
}
