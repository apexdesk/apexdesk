//! `apexdesk serve`: ApexDesk in the browser (crates/apex-web) for the
//! users of a server. Each signs in with a name and a password and finds
//! the files kept for them on the server; what they save goes back there.
//! Get Data's databases, which a browser can't reach, are reached by the
//! server for them (only the servers `--db-hosts` names).
//!
//! HTTP/1.1 on std's sockets, a thread per connection, and HTTPS with
//! rustls when given a certificate. Kept in `<dir>`: `users` (one
//! `name:argon2-hash` a line) and `files/<name>/`.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde_json::{Value, json};

use super::args::Parsed;
use super::{Fail, Res, emit, error, say, secret, usage};

pub const SERVE_HELP: &str = "Usage: apexdesk serve [options]
       apexdesk serve --add-user NAME | --remove-user NAME | --list-users

Serves ApexDesk for the browser to the users of this server: each signs
in with a name and a password and finds the files kept for them here.
The browser version is built with crates/apex-web/web/build.sh.

Options:
      --dir DIR          where the users and their files are kept
                         (default: 'serve' in the settings folder)
      --web DIR          ApexDesk for the browser (default: 'web' next to
                         this program)
      --bind ADDR        address to listen on (default 127.0.0.1; 0.0.0.0
                         for every network)
      --port N           port (default 8080, 8443 with --cert)
      --cert FILE        certificate chain (PEM) for HTTPS
      --key FILE         its private key (PEM)
      --db-hosts LIST    database servers that Get Data may reach through
                         this server, comma separated (default: none)
      --add-user NAME    add a user, or change their password (asked for;
                         the first line of the standard input when it is
                         not a terminal)
      --remove-user NAME remove a user (their files are kept)
      --list-users       list the users

Without --cert and --key, passwords and files travel unencrypted: use it
so only on this computer (127.0.0.1) or behind an HTTPS proxy.
";

/// Connections served at once; more wait to be accepted.
const MAX_OPEN: usize = 256;
/// The largest file a user may save.
const MAX_FILE: u64 = 1 << 30;
/// The largest other request (sign-in, database query).
const MAX_BODY: u64 = 1 << 20;
/// Sessions end after this long unused.
const SESSION_IDLE: Duration = Duration::from_secs(12 * 3600);
/// Failed sign-ins from one address before it has to wait.
const MAX_FAILURES: u32 = 5;
const FAILURE_WAIT: Duration = Duration::from_secs(15 * 60);
const COOKIE: &str = "apexdesk";

pub fn serve(a: &Parsed) -> Res<i32> {
    if !a.pos.is_empty() {
        return usage(format!("unexpected argument '{}'", a.pos[0]));
    }
    let dir = match a.value("dir") {
        Some(d) => PathBuf::from(d),
        None => crate::settings::config_dir().map(|d| d.join("serve")).ok_or_else(|| Fail::Error("no settings folder: give one with --dir".into()))?,
    };
    if let Some(name) = a.value("add-user") {
        return add_user(&dir, name);
    }
    if let Some(name) = a.value("remove-user") {
        let mut users = read_users(&dir);
        let n = users.len();
        users.retain(|(u, _)| u != name);
        if users.len() == n {
            return error(format!("no user '{name}'"));
        }
        write_users(&dir, &users).map_err(|e| Fail::Error(format!("{}: {e}", users_file(&dir).display())))?;
        say(&format!("Removed '{name}' (their files are kept in {}).", dir.join("files").join(name).display()));
        return Ok(0);
    }
    if a.flag("list-users") {
        let mut out = String::new();
        for (u, _) in read_users(&dir) {
            out.push_str(&u);
            out.push('\n');
        }
        emit(&out);
        return Ok(0);
    }

    let web = match a.value("web") {
        Some(w) => PathBuf::from(w),
        None => std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("web"))).unwrap_or_else(|| PathBuf::from("web")),
    };
    if !web.join("index.html").is_file() || !web.join("apexdesk_bg.wasm").is_file() {
        return error(format!("no ApexDesk for the browser in {}: build it with crates/apex-web/web/build.sh and give its folder with --web", web.display()));
    }
    let web = web.canonicalize().map_err(|e| Fail::Error(format!("{}: {e}", web.display())))?;
    if read_users(&dir).is_empty() {
        return error(format!("no users in {}: add one with 'apexdesk serve --add-user NAME'", dir.display()));
    }
    let tls = match (a.value("cert"), a.value("key")) {
        (Some(c), Some(k)) => Some(tls_config(Path::new(c), Path::new(k)).map_err(Fail::Error)?),
        (None, None) => None,
        _ => return usage("--cert and --key go together"),
    };
    let bind: IpAddr = a.value("bind").unwrap_or("127.0.0.1").parse().map_err(|_| Fail::Usage(format!("not an address: '{}'", a.value("bind").unwrap_or_default())))?;
    let port: u16 = match a.value("port") {
        Some(p) => p.parse().map_err(|_| Fail::Usage(format!("not a port: '{p}'")))?,
        None if tls.is_some() => 8443,
        None => 8080,
    };
    let db_hosts = a.value("db-hosts").map(|l| l.split(',').map(|h| h.trim().to_ascii_lowercase()).filter(|h| !h.is_empty()).collect()).unwrap_or_default();
    std::fs::create_dir_all(dir.join("files")).map_err(|e| Fail::Error(format!("{}: {e}", dir.display())))?;
    let listener = TcpListener::bind((bind, port)).map_err(|e| Fail::Error(format!("{bind} port {port}: {e}")))?;
    let host = if bind.is_unspecified() { "this-server".to_string() } else if bind.is_ipv6() { format!("[{bind}]") } else { bind.to_string() };
    say(&format!("ApexDesk for the browser at {}://{host}:{port}/ (Ctrl+C stops it)", if tls.is_some() { "https" } else { "http" }));
    if tls.is_none() && !bind.is_loopback() {
        say("Warning: without --cert and --key, passwords and files travel unencrypted on the network.");
    }
    let srv = Arc::new(Server { dir, web, db_hosts, secure: tls.is_some(), sessions: Mutex::default(), failures: Mutex::default(), open: AtomicUsize::new(0) });
    for stream in listener.incoming() {
        let Ok(tcp) = stream else { continue };
        if srv.open.load(Ordering::Relaxed) >= MAX_OPEN {
            continue;
        }
        let Ok(peer) = tcp.peer_addr() else { continue };
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = tcp.set_write_timeout(Some(Duration::from_secs(60)));
        let (conn, tls) = (srv.clone(), tls.clone());
        srv.open.fetch_add(1, Ordering::Relaxed);
        let spawned = std::thread::Builder::new().name("serve".into()).spawn(move || {
            connection(&conn, tcp, tls, peer.ip());
            conn.open.fetch_sub(1, Ordering::Relaxed);
        });
        if spawned.is_err() {
            srv.open.fetch_sub(1, Ordering::Relaxed);
        }
    }
    Ok(0)
}

// --- users ---------------------------------------------------------------------------

fn users_file(dir: &Path) -> PathBuf {
    dir.join("users")
}

fn read_users(dir: &Path) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(users_file(dir)).unwrap_or_default();
    text.lines().filter_map(|l| l.split_once(':')).filter(|(u, _)| valid_name(u)).map(|(u, h)| (u.to_string(), h.to_string())).collect()
}

fn write_users(dir: &Path, users: &[(String, String)]) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let text: String = users.iter().map(|(u, h)| format!("{u}:{h}\n")).collect();
    let tmp = dir.join(".users.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    // Only the server's account reads the hashes.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(&tmp)?.write_all(text.as_bytes())?;
    std::fs::rename(&tmp, users_file(dir))
}

/// A user name is also their folder's: letters, digits, `.`, `_`, `-`.
fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && !n.starts_with('.') && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn add_user(dir: &Path, name: &str) -> Res<i32> {
    if !valid_name(name) {
        return usage(format!("'{name}': a user name has letters, digits, '.', '_' and '-'"));
    }
    let read = |p: &str| secret::read(p).map_err(|e| Fail::Error(format!("could not read the password: {e}")));
    let pw = read(&format!("Password for {name}: "))?;
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) && read("Again: ")? != pw {
        return error("the passwords differ");
    }
    if pw.chars().count() < 8 {
        return error("a password has at least 8 characters");
    }
    let hash = hash(&pw).map_err(Fail::Error)?;
    let mut users = read_users(dir);
    match users.iter_mut().find(|(u, _)| u == name) {
        Some(u) => u.1 = hash,
        None => users.push((name.to_string(), hash)),
    }
    write_users(dir, &users).map_err(|e| Fail::Error(format!("{}: {e}", users_file(dir).display())))?;
    let _ = std::fs::create_dir_all(dir.join("files").join(name));
    say(&format!("User '{name}' saved in {}.", users_file(dir).display()));
    Ok(0)
}

fn hash(pw: &str) -> Result<String, String> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| e.to_string())?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
    argon2::Argon2::default().hash_password(pw.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

fn verify(pw: &str, hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(hash).is_ok_and(|h| argon2::Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
}

/// Checked against when the user doesn't exist, so a wrong name takes as
/// long as a wrong password.
fn dummy_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| hash("not a password").unwrap_or_default())
}

fn tls_config(cert: &Path, key: &Path) -> Result<Arc<rustls::ServerConfig>, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs = CertificateDer::pem_file_iter(cert).and_then(|i| i.collect::<Result<Vec<_>, _>>()).map_err(|e| format!("{}: {e}", cert.display()))?;
    if certs.is_empty() {
        return Err(format!("{}: no certificate", cert.display()));
    }
    let key = PrivateKeyDer::from_pem_file(key).map_err(|e| format!("{}: {e}", key.display()))?;
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("the certificate and the key: {e}"))?;
    Ok(Arc::new(cfg))
}

// --- connections ---------------------------------------------------------------------

struct Server {
    dir: PathBuf,
    web: PathBuf,
    db_hosts: Vec<String>,
    secure: bool,
    /// Signed-in users by their cookie.
    sessions: Mutex<HashMap<String, Session>>,
    /// Failed sign-ins by address: how many, the last one.
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    open: AtomicUsize,
}

struct Session {
    user: String,
    last: Instant,
}

fn connection(srv: &Server, tcp: TcpStream, tls: Option<Arc<rustls::ServerConfig>>, ip: IpAddr) {
    match tls {
        Some(cfg) => {
            let Ok(conn) = rustls::ServerConnection::new(cfg) else { return };
            let mut s = rustls::StreamOwned::new(conn, tcp);
            requests(srv, &mut s, ip);
            s.conn.send_close_notify();
            let _ = s.flush();
        }
        None => {
            let mut tcp = tcp;
            requests(srv, &mut tcp, ip);
        }
    }
}

struct Request {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    length: u64,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// The requests of a connection, kept open between them.
fn requests<S: Read + Write>(srv: &Server, stream: &mut S, ip: IpAddr) {
    let mut r = BufReader::new(stream);
    loop {
        let req = match read_head(&mut r) {
            Ok(Some(req)) => req,
            Ok(None) => return,
            Err(status) => {
                let _ = write_reply(r.get_mut(), Reply::text(status, reason(status)), false);
                return;
            }
        };
        let mut keep = !req.header("connection").is_some_and(|c| c.eq_ignore_ascii_case("close"));
        let mut body = (&mut r).take(req.length);
        let reply = route(srv, &req, &mut body, ip);
        // What the request sent and wasn't read: a little is skipped, more
        // ends the connection.
        let left = body.limit();
        if left > 0 && (left > MAX_BODY || io::copy(&mut body, &mut io::sink()).ok() != Some(left)) {
            keep = false;
        }
        if write_reply(r.get_mut(), reply, keep).is_err() || !keep {
            return;
        }
    }
}

/// The request line and headers (None: the connection ended); an error
/// is the status to answer with.
fn read_head<R: BufRead>(r: &mut R) -> Result<Option<Request>, u16> {
    let mut buf = Vec::new();
    loop {
        // A closed or silent connection just ends.
        match r.by_ref().take(16 * 1024 - buf.len() as u64).read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return Ok(None),
            Ok(_) => {}
        }
        if buf.ends_with(b"\r\n\r\n") || buf.ends_with(b"\n\n") {
            break;
        }
        if buf.len() >= 16 * 1024 {
            return Err(431);
        }
        // Blank lines before a request are skipped.
        if buf == b"\r\n" || buf == b"\n" {
            buf.clear();
        }
    }
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut p = httparse::Request::new(&mut headers);
    match p.parse(&buf) {
        Ok(httparse::Status::Complete(_)) => {}
        _ => return Err(400),
    }
    let (Some(method), Some(target)) = (p.method, p.path) else { return Err(400) };
    let headers: Vec<(String, String)> = p.headers.iter().map(|h| (h.name.to_string(), String::from_utf8_lossy(h.value).into_owned())).collect();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut req = Request { method: method.to_string(), path: path.to_string(), query: query.to_string(), headers, length: 0 };
    if req.header("transfer-encoding").is_some() {
        return Err(411);
    }
    if let Some(l) = req.header("content-length") {
        req.length = l.trim().parse().map_err(|_| 400u16)?;
    }
    Ok(Some(req))
}

// --- replies -------------------------------------------------------------------------

enum Body {
    Bytes(Vec<u8>),
    File(std::fs::File, u64),
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Body,
}

impl Reply {
    fn new(status: u16, kind: &str, body: Vec<u8>) -> Reply {
        Reply { status, headers: vec![("Content-Type", kind.to_string())], body: Body::Bytes(body) }
    }

    fn text(status: u16, s: &str) -> Reply {
        Reply::new(status, "text/plain; charset=utf-8", s.as_bytes().to_vec())
    }

    fn json(status: u16, v: Value) -> Reply {
        Reply::new(status, "application/json", v.to_string().into_bytes()).with("Cache-Control", "no-store")
    }

    fn fail(status: u16, msg: &str) -> Reply {
        Reply::json(status, json!({ "error": msg }))
    }

    fn redirect(to: &str) -> Reply {
        Reply::new(303, "text/plain; charset=utf-8", Vec::new()).with("Location", to)
    }

    fn with(mut self, name: &'static str, value: impl Into<String>) -> Reply {
        self.headers.push((name, value.into()));
        self
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        304 => "Not Modified",
        303 => "See Other",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        502 => "Bad Gateway",
        _ => "Internal Server Error",
    }
}

fn write_reply<W: Write>(w: &mut W, reply: Reply, keep: bool) -> io::Result<()> {
    let len = match &reply.body {
        Body::Bytes(b) => b.len() as u64,
        Body::File(_, n) => *n,
    };
    let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
    // The page runs only its own scripts and talks only to this server.
    head.push_str("Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; connect-src 'self'; frame-ancestors 'none'; form-action 'self'; base-uri 'none'\r\n");
    head.push_str("X-Content-Type-Options: nosniff\r\nReferrer-Policy: same-origin\r\nX-Frame-Options: DENY\r\n");
    for (k, v) in &reply.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!("Content-Length: {len}\r\nConnection: {}\r\n\r\n", if keep { "keep-alive" } else { "close" }));
    w.write_all(head.as_bytes())?;
    match reply.body {
        Body::Bytes(b) => w.write_all(&b)?,
        Body::File(f, n) => {
            if io::copy(&mut f.take(n), w)? != n {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
    }
    w.flush()
}

// --- routes --------------------------------------------------------------------------

fn route(srv: &Server, req: &Request, body: &mut dyn Read, ip: IpAddr) -> Reply {
    let path = req.path.as_str();
    let m = req.method.as_str();
    // Changes only from this server's own pages (the cookie is SameSite
    // too).
    if m != "GET" && !same_origin(req) {
        return Reply::fail(403, "the request came from another site");
    }
    match (m, path) {
        ("GET", "/login") => return login_page(req, req.query.contains("failed")),
        ("POST", "/login") => return login(srv, req, body, ip),
        ("POST", "/logout") => {
            if let Some(t) = cookie(req) {
                srv.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(&t);
            }
            return Reply::redirect("/login").with("Set-Cookie", format!("{COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{}", if srv.secure { "; Secure" } else { "" }));
        }
        _ => {}
    }
    let Some(user) = session(srv, req) else {
        return if path.starts_with("/api/") || m != "GET" { Reply::fail(401, "not signed in") } else { Reply::redirect("/login") };
    };
    let root = srv.dir.join("files").join(&user);
    match (m, path) {
        ("GET", "/api/me") => Reply::json(200, json!({ "user": user })),
        ("GET", "/api/files") => {
            let _ = std::fs::create_dir_all(&root);
            let mut out = Vec::new();
            list(&root, "", 0, &mut out);
            Reply::json(200, Value::Array(out))
        }
        (_, p) if p.starts_with("/api/files/") => file(&root, m, &p["/api/files/".len()..], body, req.length),
        ("POST", "/api/db") => db(srv, body, req.length),
        ("GET", p) if !p.starts_with("/api/") => page(srv, p, req.header("if-none-match")),
        _ => Reply::fail(if path.starts_with("/api/") { 404 } else { 405 }, "no such request"),
    }
}

/// Whether the request's `Origin` (when the browser sends one) is this
/// server.
fn same_origin(req: &Request) -> bool {
    let Some(origin) = req.header("origin") else { return true };
    let host = req.header("host").unwrap_or_default();
    origin.split_once("://").is_some_and(|(_, h)| !host.is_empty() && h.eq_ignore_ascii_case(host))
}

fn cookie(req: &Request) -> Option<String> {
    req.headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case("cookie")).flat_map(|(_, v)| v.split(';')).find_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('=').map(str::to_string)).filter(|t| !t.is_empty())
}

/// The signed-in user of a request, if they still exist.
fn session(srv: &Server, req: &Request) -> Option<String> {
    let token = cookie(req)?;
    let mut sessions = srv.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    sessions.retain(|_, s| now.duration_since(s.last) < SESSION_IDLE);
    let s = sessions.get_mut(&token)?;
    if !read_users(&srv.dir).iter().any(|(u, _)| *u == s.user) {
        sessions.remove(&token);
        return None;
    }
    s.last = now;
    Some(s.user.clone())
}

fn login_page(req: &Request, failed: bool) -> Reply {
    let es = req.header("accept-language").is_some_and(|l| l.trim_start().to_ascii_lowercase().starts_with("es"));
    let t = |en: &'static str, sp: &'static str| if es { sp } else { en };
    let msg = if failed { format!("<p class=\"err\">{}</p>", t("Wrong user or password.", "Usuario o contraseña incorrectos.")) } else { String::new() };
    let html = format!(
        "<!doctype html><html lang=\"{lang}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>ApexDesk</title><link rel=\"icon\" href=\"icon.png\">\
<style>body{{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#f3f3f3;font-family:system-ui,sans-serif;color:#222}}\
@media (prefers-color-scheme: dark){{body{{background:#1e1e1e;color:#ddd}}form{{background:#2a2a2a!important}}input{{background:#1e1e1e;color:#ddd;border-color:#555!important}}}}\
form{{background:#fff;padding:28px;border-radius:8px;box-shadow:0 2px 12px #0002;display:flex;flex-direction:column;gap:10px;width:280px}}\
h1{{margin:0 0 8px;font-size:20px}}input{{padding:8px;font-size:15px;border:1px solid #bbb;border-radius:4px}}button{{padding:9px;font-size:15px;border:0;border-radius:4px;background:#2b6cb0;color:#fff;cursor:pointer}}.err{{color:#c53030;margin:0}}</style></head>\
<body><form method=\"post\" action=\"/login\"><h1>ApexDesk</h1>{msg}<label>{user}<br><input name=\"user\" autocomplete=\"username\" autofocus required style=\"width:100%;box-sizing:border-box\"></label>\
<label>{pass}<br><input name=\"password\" type=\"password\" autocomplete=\"current-password\" required style=\"width:100%;box-sizing:border-box\"></label><button>{go}</button></form></body></html>",
        lang = if es { "es" } else { "en" },
        user = t("User", "Usuario"),
        pass = t("Password", "Contraseña"),
        go = t("Sign in", "Entrar"),
    );
    Reply::new(if failed { 401 } else { 200 }, "text/html; charset=utf-8", html.into_bytes()).with("Cache-Control", "no-store")
}

fn login(srv: &Server, req: &Request, body: &mut dyn Read, ip: IpAddr) -> Reply {
    {
        let mut f = srv.failures.lock().unwrap_or_else(|e| e.into_inner());
        f.retain(|_, (_, last)| last.elapsed() < FAILURE_WAIT);
        if f.get(&ip).is_some_and(|(n, _)| *n >= MAX_FAILURES) {
            return Reply::text(429, "Too many failed sign-ins: try again in 15 minutes.");
        }
    }
    let Some(form) = read_body(body, req.length, 4096) else { return Reply::text(413, reason(413)) };
    let form = String::from_utf8_lossy(&form);
    let field = |name: &str| form.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == name).and_then(|(_, v)| decode(v, true));
    let (user, pw) = (field("user").unwrap_or_default(), field("password").unwrap_or_default());
    let users = read_users(&srv.dir);
    let hash = users.iter().find(|(u, _)| *u == user).map(|(_, h)| h.as_str());
    let ok = verify(&pw, hash.unwrap_or(dummy_hash())) && hash.is_some();
    if !ok {
        let mut f = srv.failures.lock().unwrap_or_else(|e| e.into_inner());
        let e = f.entry(ip).or_insert((0, Instant::now()));
        *e = (e.0 + 1, Instant::now());
        say(&format!("{ip}: failed sign-in"));
        return Reply::redirect("/login?failed");
    }
    srv.failures.lock().unwrap_or_else(|e| e.into_inner()).remove(&ip);
    let mut t = [0u8; 32];
    if getrandom::fill(&mut t).is_err() {
        return Reply::fail(500, "no random numbers");
    }
    let token: String = t.iter().map(|b| format!("{b:02x}")).collect();
    srv.sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(token.clone(), Session { user: user.clone(), last: Instant::now() });
    say(&format!("{ip}: {user} signed in"));
    Reply::redirect("/").with("Set-Cookie", format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict{}", if srv.secure { "; Secure" } else { "" }))
}

/// The body of a request when it isn't longer than `max`.
fn read_body(body: &mut dyn Read, length: u64, max: u64) -> Option<Vec<u8>> {
    if length > max {
        return None;
    }
    let mut b = Vec::with_capacity(length as usize);
    body.take(max).read_to_end(&mut b).ok()?;
    (b.len() as u64 == length).then_some(b)
}

/// Undoes `%XX` (and `+` for a space in forms); None when it isn't UTF-8.
fn decode(s: &str, plus: bool) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(h, 16).ok()?);
                i += 3;
                continue;
            }
            b'+' if plus => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// A path of the request (`/`-separated, each part encoded) under `root`:
/// plain names only, none hidden.
fn under(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut p = root.to_path_buf();
    for part in rel.split('/') {
        let name = decode(part, false)?;
        if name.is_empty() || name.starts_with('.') || name.len() > 255 || name.contains(['/', '\\', '\0', ':']) {
            return None;
        }
        p.push(name);
    }
    (p != root).then_some(p)
}

/// Whether `p`, symbolic links followed, is still under `root`.
fn inside(root: &Path, p: &Path) -> bool {
    match (root.canonicalize(), p.canonicalize()) {
        (Ok(r), Ok(p)) => p.starts_with(r),
        _ => false,
    }
}

/// The user's files and folders, paths `/`-separated from their folder.
fn list(dir: &Path, rel: &str, depth: usize, out: &mut Vec<Value>) {
    if depth > 32 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if out.len() >= 20_000 {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if meta.is_dir() {
            out.push(json!({ "path": path, "dir": true }));
            list(&e.path(), &path, depth + 1, out);
        } else if meta.is_file() {
            let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            out.push(json!({ "path": path, "size": meta.len(), "modified": modified }));
        }
    }
}

fn file(root: &Path, method: &str, rel: &str, body: &mut dyn Read, length: u64) -> Reply {
    let Some(path) = under(root, rel) else { return Reply::fail(400, "not a file name") };
    match method {
        "GET" => {
            if !inside(root, &path) || !path.is_file() {
                return Reply::fail(404, "no such file");
            }
            match std::fs::File::open(&path).and_then(|f| Ok((f.metadata()?.len(), f))) {
                Ok((n, f)) => Reply { status: 200, headers: vec![("Content-Type", "application/octet-stream".into()), ("Content-Disposition", "attachment".into()), ("Cache-Control", "no-store".into())], body: Body::File(f, n) },
                Err(_) => Reply::fail(404, "no such file"),
            }
        }
        "PUT" => {
            if length > MAX_FILE {
                return Reply::fail(413, "the file is too big");
            }
            let parent = path.parent().unwrap_or(root);
            if std::fs::create_dir_all(parent).is_err() || !inside(root, parent) || path.is_dir() || path.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
                return Reply::fail(409, "the file can't be saved there");
            }
            match save(parent, &path, body, length) {
                Ok(()) => Reply::json(200, json!({ "saved": true })),
                Err(e) => Reply::fail(500, &format!("not saved: {e}")),
            }
        }
        "DELETE" => {
            if !inside(root, &path) {
                return Reply::fail(404, "no such file");
            }
            let r = if path.is_dir() { std::fs::remove_dir(&path) } else { std::fs::remove_file(&path) };
            match r {
                Ok(()) => Reply::json(200, json!({ "deleted": true })),
                Err(e) => Reply::fail(409, &e.to_string()),
            }
        }
        _ => Reply::fail(405, "no such request"),
    }
}

/// Writes the body to a hidden file next to `path`, then puts it in its
/// place: a broken upload leaves the old file.
fn save(parent: &Path, path: &Path, body: &mut dyn Read, length: u64) -> io::Result<()> {
    let mut t = [0u8; 8];
    let _ = getrandom::fill(&mut t);
    let tmp = parent.join(format!(".apexdesk-{}.tmp", t.iter().map(|b| format!("{b:02x}")).collect::<String>()));
    let res = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        if io::copy(&mut body.take(length), &mut f)? != length {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the upload was cut"));
        }
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// A Get Data query on a database server, for the page.
fn db(srv: &Server, body: &mut dyn Read, length: u64) -> Reply {
    if srv.db_hosts.is_empty() {
        return Reply::fail(403, "this server reaches no databases (see apexdesk serve --db-hosts)");
    }
    if apex_query::disabled() {
        return Reply::fail(403, "Get Data is turned off on this server");
    }
    let Some(b) = read_body(body, length, MAX_BODY).filter(|b| !b.is_empty()) else { return Reply::fail(400, "no query") };
    let Some((src, pw)) = crate::db_wire::parse_request(&b) else { return Reply::fail(400, "not a database query") };
    let apex_sheet::query::Source::Database { host, .. } = &src else { return Reply::fail(400, "not a database query") };
    if !srv.db_hosts.contains(&host.trim().to_ascii_lowercase()) {
        return Reply::fail(403, &format!("this server doesn't reach the database server '{host}'"));
    }
    match apex_query::sources::db::server(&src, &pw) {
        Ok(t) => Reply::new(200, "application/json", crate::db_wire::table(&t).into_bytes()).with("Cache-Control", "no-store"),
        Err(e) => Reply::fail(502, &e.to_string()),
    }
}

/// A file of ApexDesk for the browser; the browser keeps them and asks
/// whether they changed (its tag, from the file's size and time).
fn page(srv: &Server, path: &str, seen: Option<&str>) -> Reply {
    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let Some(p) = under(&srv.web, rel).filter(|p| inside(&srv.web, p) && p.is_file()) else { return Reply::text(404, reason(404)) };
    let kind = match p.extension().and_then(|e| e.to_str()).unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" => "text/plain; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        _ => "application/octet-stream",
    };
    let Ok((meta, f)) = std::fs::File::open(&p).and_then(|f| Ok((f.metadata()?, f))) else { return Reply::text(404, reason(404)) };
    let time = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    let tag = format!("\"{:x}-{time:x}\"", meta.len());
    if seen.is_some_and(|s| s.split(',').any(|t| t.trim() == tag)) {
        return Reply::new(304, kind, Vec::new()).with("ETag", tag).with("Cache-Control", "no-cache");
    }
    Reply { status: 200, headers: vec![("Content-Type", kind.into()), ("Cache-Control", "no-cache".into()), ("ETag", tag)], body: Body::File(f, meta.len()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths() {
        assert!(valid_name("ana.perez-2") && !valid_name("../x") && !valid_name(".hidden") && !valid_name(""));
        let root = Path::new("/srv/files/ana");
        assert_eq!(under(root, "a/b%20c.xlsx"), Some(root.join("a").join("b c.xlsx")));
        for bad in ["", "..", "a/../b", "a//b", ".x", "a%2Fb", "a%5Cb", "a%00", "c:x", "%C0%AF"] {
            assert_eq!(under(root, bad), None, "{bad}");
        }
        assert_eq!(decode("a+b%21", true).as_deref(), Some("a b!"));
        assert_eq!(decode("%zz", false), None);
    }

    #[test]
    fn passwords() {
        let h = hash("correct horse").unwrap();
        assert!(verify("correct horse", &h) && !verify("wrong", &h) && !verify("x", "not a hash"));
    }
}
