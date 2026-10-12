//! The line between `apexdesk mcp` and the open window. The window listens
//! on a Unix socket only the user can reach (elsewhere, on a loopback TCP
//! port whose number and secret are kept in the settings folder, like
//! `instance`). A call is one JSON line, `{"tool": .., "args": ..}` (after
//! the secret on TCP); the answer is one line, `{"text": ..}` or
//! `{"error": ..}`. `APEXDESK_MCP_SOCKET` names another socket (or port
//! file), for tests and for more than one window.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use eframe::egui;
use serde_json::{Value, json};

/// This process created the socket.
static OWNER: AtomicBool = AtomicBool::new(false);

/// How long a call waits for the window.
const WAIT: Duration = Duration::from_secs(30);

pub const NOT_RUNNING: &str = "ApexDesk is not running: open it (with the files to work on) and try again.";

/// A tool call for the window, and where its answer goes.
pub struct Request {
    pub tool: String,
    pub args: Value,
    pub reply: Sender<Result<String, String>>,
}

fn endpoint() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("APEXDESK_MCP_SOCKET").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    default_endpoint()
}

#[cfg(unix)]
fn default_endpoint() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_dir());
    // Without a runtime folder, one of this user's own (see instance.rs).
    let dir = match dir {
        Some(d) => d,
        None => apex_io::safe_path::user_temp_dir("apexdesk").ok()?,
    };
    Some(dir.join("apexdesk-mcp.sock"))
}

#[cfg(not(unix))]
fn default_endpoint() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("mcp"))
}

/// Reads one call from `reader`, hands it to the window and writes the
/// answer. `key`: the secret the call must start with.
fn answer(reader: impl BufRead, mut writer: impl Write, key: Option<&str>, tx: &Sender<Request>, ctx: &egui::Context) {
    // A call never runs to more than this: a line without end can't fill the memory.
    let mut reader = reader.take(64 << 20);
    let mut line = String::new();
    if let Some(k) = key {
        if reader.read_line(&mut line).is_err() || line.trim_end() != k {
            return;
        }
        line.clear();
    }
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let result = match serde_json::from_str::<Value>(&line) {
        Ok(v) => {
            let (rtx, rrx) = std::sync::mpsc::channel();
            let req = Request { tool: v["tool"].as_str().unwrap_or_default().to_string(), args: v.get("args").cloned().unwrap_or_else(|| json!({})), reply: rtx };
            if tx.send(req).is_err() {
                Err(NOT_RUNNING.to_string())
            } else {
                ctx.request_repaint();
                rrx.recv_timeout(WAIT).unwrap_or_else(|_| Err("ApexDesk did not answer in time.".to_string()))
            }
        }
        Err(e) => Err(format!("bad request: {e}")),
    };
    let out = match result {
        Ok(t) => json!({ "text": t }),
        Err(e) => json!({ "error": e }),
    };
    let _ = writeln!(writer, "{out}");
    let _ = writer.flush();
}

/// Reads the window's answer to a call.
fn read_answer(reader: impl BufRead) -> Result<String, String> {
    let line = reader.lines().next().and_then(|l| l.ok()).ok_or_else(|| "ApexDesk closed the connection.".to_string())?;
    let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad answer: {e}"))?;
    match (v["text"].as_str(), v["error"].as_str()) {
        (Some(t), _) => Ok(t.to_string()),
        (_, Some(e)) => Err(e.to_string()),
        _ => Err("bad answer".to_string()),
    }
}

fn call_line(tool: &str, args: &Value) -> String {
    format!("{}\n", json!({ "tool": tool, "args": args }))
}

#[cfg(unix)]
mod sock {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    pub fn listen(path: &Path, ctx: egui::Context) -> Option<Receiver<Request>> {
        use std::os::unix::fs::PermissionsExt;
        let listener = match UnixListener::bind(path) {
            Ok(l) => l,
            Err(_) => {
                // A live window keeps it; otherwise it is stale.
                if UnixStream::connect(path).is_ok() {
                    return None;
                }
                let _ = std::fs::remove_file(path);
                UnixListener::bind(path).ok()?
            }
        };
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("apexdesk-mcp".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let Ok(read) = stream.try_clone() else { continue };
                    let _ = read.set_read_timeout(Some(Duration::from_secs(10)));
                    answer(BufReader::new(read), stream, None, &tx, &ctx);
                }
            })
            .ok()?;
        Some(rx)
    }

    pub fn call(path: &Path, tool: &str, args: &Value) -> Result<String, String> {
        let mut s = UnixStream::connect(path).map_err(|_| NOT_RUNNING.to_string())?;
        s.write_all(call_line(tool, args).as_bytes()).map_err(|e| e.to_string())?;
        let _ = s.set_read_timeout(Some(WAIT + Duration::from_secs(5)));
        read_answer(BufReader::new(s))
    }
}

/// Loopback TCP, for systems without Unix sockets: the port and a secret
/// in a file only the user reads, so other users' programs can't call.
#[cfg(any(not(unix), test))]
mod tcp {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};
    use std::path::Path;

    fn read(file: &Path) -> Option<(u16, String)> {
        let text = std::fs::read_to_string(file).ok()?;
        let (port, key) = text.trim().split_once(' ')?;
        Some((port.parse().ok()?, key.to_string()))
    }

    pub fn listen(file: &Path, ctx: egui::Context) -> Option<Receiver<Request>> {
        if let Some((port, key)) = read(file)
            && crate::instance::tcp::connect(port, &key, Duration::from_millis(300)).is_some()
        {
            return None;
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        let key = crate::instance::tcp::secret();
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(file, format!("{port} {key}\n")).ok()?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("apexdesk-mcp".into())
            .spawn(move || {
                let tx = std::sync::Mutex::new(tx);
                crate::instance::tcp::serve(listener, move |stream| {
                    if !crate::instance::tcp::prove(&stream, &key) {
                        return;
                    }
                    let Ok(read) = stream.try_clone() else { return };
                    let tx = tx.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    // The call's 10 seconds are in all, not per read.
                    let within = crate::instance::tcp::Within(&read, std::time::Instant::now() + Duration::from_secs(10));
                    answer(BufReader::new(within), stream, Some(&key), &tx, &ctx);
                });
            })
            .ok()?;
        Some(rx)
    }

    pub fn call(file: &Path, tool: &str, args: &Value) -> Result<String, String> {
        let (port, key) = read(file).ok_or_else(|| NOT_RUNNING.to_string())?;
        // Only to the instance that holds the key (not whatever took its port).
        let mut s = crate::instance::tcp::connect(port, &key, Duration::from_secs(2)).ok_or_else(|| NOT_RUNNING.to_string())?;
        s.write_all(format!("{key}\n{}", call_line(tool, args)).as_bytes()).map_err(|e| e.to_string())?;
        let _ = s.set_read_timeout(Some(WAIT + Duration::from_secs(5)));
        read_answer(BufReader::new(s))
    }
}

/// Starts listening for calls. Scripted runs listen only when told where.
pub fn listen(ctx: egui::Context, scripted: bool) -> Option<Receiver<Request>> {
    if scripted && std::env::var_os("APEXDESK_MCP_SOCKET").is_none() {
        return None;
    }
    let path = endpoint()?;
    #[cfg(unix)]
    let rx = sock::listen(&path, ctx)?;
    #[cfg(not(unix))]
    let rx = tcp::listen(&path, ctx)?;
    OWNER.store(true, Ordering::Relaxed);
    Some(rx)
}

/// Calls `tool` in the running window.
pub fn call(tool: &str, args: &Value) -> Result<String, String> {
    let path = endpoint().ok_or_else(|| NOT_RUNNING.to_string())?;
    #[cfg(unix)]
    return sock::call(&path, tool, args);
    #[cfg(not(unix))]
    return tcp::call(&path, tool, args);
}

/// Removes the socket (or port file) when the listening window exits.
pub fn cleanup() {
    if OWNER.load(Ordering::Relaxed)
        && let Some(p) = endpoint()
    {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers calls like the window would: echoes the tool.
    fn serve(rx: Receiver<Request>) {
        std::thread::spawn(move || {
            for r in rx {
                let res = if r.tool == "fail" { Err("no".to_string()) } else { Ok(format!("{}:{}", r.tool, r.args["x"])) };
                let _ = r.reply.send(res);
            }
        });
    }

    fn dir(name: &str) -> PathBuf {
        let d = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/tmp")).join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn tcp_calls_need_the_secret() {
        let d = dir("mcp-tcp");
        let file = d.join("mcp");
        let rx = tcp::listen(&file, egui::Context::default()).expect("listens");
        assert!(tcp::listen(&file, egui::Context::default()).is_none());
        serve(rx);
        assert_eq!(tcp::call(&file, "read", &json!({"x": 5})), Ok("read:5".to_string()));
        assert_eq!(tcp::call(&file, "fail", &json!({})), Err("no".to_string()));
        // A wrong secret gets no answer.
        let (port, _) = std::fs::read_to_string(&file).unwrap().trim().split_once(' ').map(|(p, k)| (p.parse::<u16>().unwrap(), k.to_string())).unwrap();
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(format!("nonce\nwrong\n{}", call_line("read", &json!({}))).as_bytes()).unwrap();
        assert!(read_answer(BufReader::new(s)).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn socket_calls() {
        // Socket paths are short (about 100 bytes): not under target.
        let path = std::env::temp_dir().join(format!("apexdesk-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert_eq!(sock::call(&path, "read", &json!({})), Err(NOT_RUNNING.to_string()));
        let rx = sock::listen(&path, egui::Context::default()).expect("listens");
        assert!(sock::listen(&path, egui::Context::default()).is_none());
        serve(rx);
        assert_eq!(sock::call(&path, "write", &json!({"x": "a"})), Ok("write:\"a\"".to_string()));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_file(&path);
    }
}
