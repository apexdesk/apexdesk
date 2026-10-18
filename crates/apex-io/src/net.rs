//! How requests go out, shared by everything that goes online (Get Data,
//! revocation checks, time stamps, updates): the proxy, the hosts reached
//! without it, the hosts that get the session's sign-in (Kerberos or NTLM
//! through Negotiate), and requests through the system's curl, which
//! speaks HTTPS, proxy authentication and Negotiate on Windows 10+, macOS
//! and Linux alike. The application sets the [`Config`] once at start.

use std::io::Write;
use std::sync::RwLock;

/// Where requests go and how they sign in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    /// The proxy's URL (`http://user:password@host:port`); None: direct.
    pub proxy: Option<String>,
    /// Hosts reached without the proxy: names, suffixes (`.corp.example`)
    /// or `*`. The local machine always is.
    pub bypass: Vec<String>,
    /// Hosts that get the session's sign-in (same forms as `bypass`).
    pub sign_in: Vec<String>,
}

static CONFIG: RwLock<Option<Config>> = RwLock::new(None);

pub fn set_config(c: Config) {
    if let Ok(mut g) = CONFIG.write() {
        *g = Some(c);
    }
}

pub fn config() -> Config {
    CONFIG.read().ok().and_then(|g| g.clone()).unwrap_or_default()
}

/// The host of `url`, without scheme, user, port or path.
pub fn host(url: &str) -> &str {
    let rest = url.trim().split_once("://").map_or(url.trim(), |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let hp = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(v6) = hp.strip_prefix('[') {
        return v6.split(']').next().unwrap_or(v6);
    }
    hp.split(':').next().unwrap_or(hp)
}

/// Whether `host` is in `list` (names, `.suffix`, `*.suffix` or `*`).
pub fn listed(host: &str, list: &[String]) -> bool {
    let host = host.to_ascii_lowercase();
    list.iter().map(|e| e.trim().to_ascii_lowercase()).filter(|e| !e.is_empty()).any(|e| {
        if e == "*" {
            return true;
        }
        let e = e.trim_start_matches('*');
        match e.strip_prefix('.') {
            Some(suffix) => host == suffix || host.ends_with(e),
            None => host == e || host.ends_with(&format!(".{e}")),
        }
    })
}

/// Splits a list of hosts written with commas, semicolons or spaces.
pub fn split_list(s: &str) -> Vec<String> {
    s.split([',', ';', ' ', '\n']).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect()
}

fn local(host: &str) -> bool {
    matches!(host.to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1")
}

/// The proxy to reach `url` through, if any.
pub fn proxy_for(url: &str) -> Option<String> {
    let c = config();
    let h = host(url);
    c.proxy.filter(|_| !local(h) && !listed(h, &c.bypass))
}

/// Whether `url`'s host gets the session's sign-in.
pub fn signs_in(url: &str) -> bool {
    listed(host(url), &config().sign_in)
}

/// Whether a proxy URL carries its own user.
fn has_user(proxy: &str) -> bool {
    let rest = proxy.split_once("://").map_or(proxy, |(_, r)| r);
    rest.split('/').next().is_some_and(|a| a.contains('@'))
}

/// A proxy URL without its user and password, and those (decoded).
fn split_user(proxy: &str) -> (String, Option<String>) {
    let (scheme, rest) = proxy.split_once("://").map_or(("", proxy), |(s, r)| (s, r));
    let auth_end = rest.find('/').unwrap_or(rest.len());
    let Some(at) = rest[..auth_end].rfind('@') else { return (proxy.to_string(), None) };
    let bare = if scheme.is_empty() { rest[at + 1..].to_string() } else { format!("{scheme}://{}", &rest[at + 1..]) };
    (bare, Some(percent_decode(&rest[..at])))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// curl's arguments for `url` with the current settings (without the URL).
/// A proxy's password is not among them (any user may read a program's
/// arguments): [`curl`] hands it over in a private file.
pub fn curl_args(url: &str) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    match proxy_for(url) {
        Some(p) => {
            a.extend(["--proxy".into(), split_user(&p).0, "--proxy-anyauth".into()]);
            if !has_user(&p) {
                // No user given: the session's (Negotiate or NTLM) if asked.
                a.extend(["--proxy-user".into(), ":".into()]);
            }
        }
        // The configured answer wins over curl's own environment variables.
        None => a.extend(["--noproxy".into(), "*".into()]),
    }
    if signs_in(url) {
        a.extend(["--negotiate".into(), "--user".into(), ":".into()]);
    }
    a
}

/// GETs `url` or POSTs a body to it through curl with the proxy and
/// sign-in settings; the reply's body (at most `limit` bytes).
pub fn curl(url: &str, post: Option<(&str, &[u8])>, max_secs: u32, limit: usize) -> Result<Vec<u8>, String> {
    curl_with(url, post, max_secs, limit, true)
}

/// [`curl`], following redirections only when `follow` (where an address
/// comes from a document, where its server sends us can't be checked).
pub fn curl_with(url: &str, post: Option<(&str, &[u8])>, max_secs: u32, limit: usize, follow: bool) -> Result<Vec<u8>, String> {
    curl_pinned(url, post, max_secs, limit, follow, None)
}

/// [`curl_with`], connecting to the address given as curl's `--resolve`
/// (`host:port:address`) instead of looking the host up.
pub fn curl_pinned(url: &str, post: Option<(&str, &[u8])>, max_secs: u32, limit: usize, follow: bool, resolve: Option<&str>) -> Result<Vec<u8>, String> {
    let mut c = if cfg!(windows) { std::process::Command::new(crate::safe_path::windows_program(r"System32\curl.exe")) } else { std::process::Command::new("curl") };
    c.args(["-s", "-S", "--fail", "--max-time", &max_secs.to_string(), "--max-filesize", &limit.to_string()]);
    // Web addresses only, also where a server redirects to (not file://,
    // ftp://, gopher://, ...).
    // No URL globbing: `{a,b}` or `[1-9]` would fetch hosts nobody checked.
    c.args(["--globoff", "--proto", "=http,https", "--proto-redir", "=http,https"]);
    if follow {
        c.args(["-L", "--max-redirs", "5"]);
    }
    if let Some(r) = resolve {
        c.args(["--resolve", r]);
    }
    c.args(["-A", "ApexDesk"]);
    c.args(curl_args(url));
    // The proxy's user and password go in a file only we can read.
    let secret = proxy_for(url).and_then(|p| split_user(&p).1).map(|user| {
        let dir = crate::safe_path::private_temp_dir("apexdesk-net", crate::safe_path::random_token())?;
        let file = dir.join("proxy.conf");
        // A line break would start another option of curl's: written as
        // the escapes its quoted strings know.
        let quoted = user.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t").replace('\x0B', "\\v");
        let mut o = apex_fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        o.open(&file)?.write_all(format!("proxy-user = \"{quoted}\"\n").as_bytes())?;
        Ok::<_, std::io::Error>(dir)
    });
    let secret = match secret {
        Some(Ok(dir)) => {
            c.arg("--config").arg(dir.join("proxy.conf"));
            Some(dir)
        }
        Some(Err(e)) => return Err(format!("curl: {e}")),
        None => None,
    };
    let _gone = secret.map(RemoveOnDrop);
    if let Some((ct, _)) = post {
        c.args(["-H", &format!("Content-Type: {ct}"), "--data-binary", "@-"]);
    }
    c.arg("--").arg(url);
    c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing up.
        c.creation_flags(0x0800_0000);
    }
    let mut child = c.spawn().map_err(|e| format!("curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take()
        && let Some((_, body)) = post
    {
        stdin.write_all(body).map_err(|e| format!("curl: {e}"))?;
    }
    // Read as it comes, up to the limit: an old curl doesn't hold an
    // answer without a length to --max-filesize.
    use std::io::Read;
    let errs = child.stderr.take().map(|mut e| {
        apex_fs::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = (&mut e).take(64 << 10).read_to_end(&mut s);
            let _ = std::io::copy(&mut e, &mut std::io::sink());
            s
        })
    });
    let mut body = Vec::new();
    let read = child.stdout.take().map(|o| o.take(limit as u64 + 1).read_to_end(&mut body));
    if body.len() > limit {
        let _ = child.kill();
        let _ = child.wait();
        return Err("curl: the answer is too large".into());
    }
    if let Some(Err(e)) = read {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("curl: {e}"));
    }
    let status = child.wait().map_err(|e| format!("curl: {e}"))?;
    let stderr = errs.and_then(|t| t.join().ok()).unwrap_or_default();
    if !status.success() {
        return Err(String::from_utf8_lossy(&stderr).trim().to_string());
    }
    Ok(body)
}

/// Removes a folder when dropped.
struct RemoveOnDrop(std::path::PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = apex_fs::remove_dir_all(&self.0);
    }
}

/// A proxy written as the systems do (`host:port`, `http=host:port;https=...`,
/// or a URL) as a URL for `https` requests.
pub fn proxy_url(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let one = if s.contains('=') {
        let parts: Vec<(&str, &str)> = s.split(';').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.trim(), v.trim())).collect();
        parts.iter().find(|(k, _)| k.eq_ignore_ascii_case("https")).or_else(|| parts.iter().find(|(k, _)| k.eq_ignore_ascii_case("http")))?.1
    } else {
        s
    };
    if one.is_empty() {
        return None;
    }
    Some(if one.contains("://") { one.to_string() } else { format!("http://{one}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_lists() {
        assert_eq!(host("https://user:pw@intranet.corp.example:8443/a?b"), "intranet.corp.example");
        assert_eq!(host("http://[::1]:80/"), "::1");
        assert_eq!(host("example.com"), "example.com");
        let l = split_list(".corp.example; files, *.lan");
        assert!(listed("intranet.corp.example", &l));
        assert!(listed("corp.example", &l));
        assert!(listed("FILES", &l));
        assert!(listed("a.files", &l));
        assert!(listed("nas.lan", &l));
        assert!(!listed("myfiles", &l));
        assert!(!listed("example.com", &l));
        assert!(listed("anything", &["*".to_string()]));
    }

    #[test]
    fn system_forms() {
        assert_eq!(proxy_url("proxy:3128").as_deref(), Some("http://proxy:3128"));
        assert_eq!(proxy_url("http=a:80;https=b:443").as_deref(), Some("http://b:443"));
        assert_eq!(proxy_url("ftp=f:21;http=a:80").as_deref(), Some("http://a:80"));
        assert_eq!(proxy_url("socks5://s:1080").as_deref(), Some("socks5://s:1080"));
        assert_eq!(proxy_url(" "), None);
    }

    #[test]
    fn curl_follows_the_settings() {
        set_config(Config { proxy: Some("http://proxy:3128".into()), bypass: vec![".corp".into()], sign_in: vec!["intranet.corp".into()] });
        let a = curl_args("https://example.com/x");
        assert!(a.windows(2).any(|w| w[0] == "--proxy" && w[1] == "http://proxy:3128"));
        assert!(a.windows(2).any(|w| w[0] == "--proxy-user" && w[1] == ":"));
        assert!(!a.contains(&"--negotiate".to_string()));
        let a = curl_args("https://intranet.corp/x");
        assert!(a.windows(2).any(|w| w[0] == "--noproxy"));
        assert!(a.contains(&"--negotiate".to_string()));
        assert_eq!(proxy_for("http://localhost:8080/"), None);
        set_config(Config { proxy: Some("http://u:p@proxy:3128".into()), ..Config::default() });
        let a = curl_args("https://example.com");
        assert!(!a.contains(&"--proxy-user".to_string()));
        // The password never shows among the arguments.
        assert!(a.windows(2).any(|w| w[0] == "--proxy" && w[1] == "http://proxy:3128"));
        assert_eq!(split_user("http://u%40x:p%3Aw@proxy:3128"), ("http://proxy:3128".to_string(), Some("u@x:p:w".to_string())));
        set_config(Config::default());
    }
}
