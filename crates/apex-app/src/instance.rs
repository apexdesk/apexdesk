//! Single instance: files opened from the file manager while ApexDesk is
//! running open as tabs in the existing window instead of a new process.
//!
//! The running instance listens on a Unix socket (elsewhere, on a loopback
//! TCP port with a secret kept in the user's settings folder); a new process
//! with files to open sends their absolute paths (one per line) and exits.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use eframe::egui;

/// This process created the socket.
static OWNER: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_dir());
    // Without a runtime folder, one of this user's own in the temporary
    // folder: a socket straight in /tmp could be taken first by another
    // user, to learn the files opened.
    let dir = match dir {
        Some(d) => d,
        None => apex_io::safe_path::user_temp_dir("apexdesk").ok()?,
    };
    Some(dir.join("apexdesk.sock"))
}

/// Set for a window of its own: its files aren't sent to a running one.
pub const NEW_WINDOW: &str = "APEXDESK_NEW_WINDOW";

/// Sends `files` to a running instance. Returns true when it took them.
#[cfg(unix)]
pub fn forward(files: &[PathBuf]) -> bool {
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    if files.is_empty() {
        return false;
    }
    let Some(Ok(mut s)) = socket_path().map(std::os::unix::net::UnixStream::connect) else { return false };
    let mut msg = Vec::new();
    for f in files {
        // Options (`--new-doc`) go as they are; paths become absolute.
        let abs = if f.as_os_str().as_bytes().starts_with(b"--") { f.clone() } else { std::path::absolute(f).unwrap_or_else(|_| f.clone()) };
        msg.extend_from_slice(abs.as_os_str().as_bytes());
        msg.push(b'\n');
    }
    s.write_all(&msg).is_ok()
}

/// Starts listening for files from other processes. Returns `None` if the
/// socket cannot be created (another instance keeps it).
#[cfg(unix)]
pub fn listen(ctx: egui::Context) -> Option<Receiver<PathBuf>> {
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    let path = socket_path()?;
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(_) => {
            // A live instance still answers; otherwise the socket is stale.
            if UnixStream::connect(&path).is_ok() {
                return None;
            }
            let _ = std::fs::remove_file(&path);
            UnixListener::bind(&path).ok()?
        }
    };
    // Only this user, even if the folder were shared.
    let _ = std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600));
    OWNER.store(true, Ordering::Relaxed);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("apexdesk-instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                // A client that never finishes, or sends without end, can't hold the socket.
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let mut buf = Vec::new();
                if stream.take(64 << 10).read_to_end(&mut buf).is_err() {
                    continue;
                }
                for line in buf.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
                    if tx.send(PathBuf::from(std::ffi::OsStr::from_bytes(line))).is_err() {
                        return;
                    }
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                ctx.request_repaint();
            }
        })
        .ok()?;
    Some(rx)
}

/// Removes the socket when the listening instance exits.
#[cfg(unix)]
pub fn cleanup() {
    if OWNER.load(Ordering::Relaxed)
        && let Some(p) = socket_path()
    {
        let _ = std::fs::remove_file(p);
    }
}

/// Loopback TCP, for systems without Unix sockets. The listener's port and
/// a random secret are kept in a file only the user can read; a message
/// starts with the secret, so other users' programs can't send files.
#[cfg(any(not(unix), test))]
pub(crate) mod tcp {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    use eframe::egui;

    pub(crate) fn secret() -> String {
        let mut b = [0u8; 16];
        if getrandom::fill(&mut b).is_err() {
            use std::hash::{BuildHasher, Hasher};
            for (i, c) in b.chunks_mut(8).enumerate() {
                let mut h = std::collections::hash_map::RandomState::new().build_hasher();
                h.write_usize(i);
                h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
                c.copy_from_slice(&h.finish().to_le_bytes());
            }
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// What only the holder of `key` answers to `nonce`, as the listener
    /// (`side` "instance") or the caller ("caller"): neither's answer
    /// serves as the other's.
    fn proof(side: &str, key: &str, nonce: &str) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(format!("apexdesk-{side}\n{key}\n{nonce}")).iter().map(|x| format!("{x:02x}")).collect()
    }

    /// One line of at most 128 bytes within 2 seconds, read a byte at a
    /// time (nothing past it is taken from the stream).
    fn line(mut s: &TcpStream) -> Option<String> {
        let until = std::time::Instant::now() + Duration::from_secs(2);
        let mut out = Vec::new();
        let mut b = [0u8];
        while out.len() < 128 {
            let left = until.checked_duration_since(std::time::Instant::now()).filter(|d| !d.is_zero())?;
            s.set_read_timeout(Some(left)).ok()?;
            s.read_exact(&mut b).ok()?;
            if b[0] == b'\n' {
                return String::from_utf8(out).ok();
            }
            out.push(b[0]);
        }
        None
    }

    /// The listener on `port` proves it holds `key` (another program may
    /// hold the port of an instance that ended badly): the connection,
    /// ready to send to, or None.
    pub(crate) fn connect(port: u16, key: &str, wait: Duration) -> Option<TcpStream> {
        let mut s = TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, port).into(), wait).ok()?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        let nonce = secret();
        s.write_all(format!("{nonce}\n").as_bytes()).ok()?;
        if line(&s)? != proof("instance", key, &nonce) {
            return None;
        }
        // Then the caller proves it holds the key too.
        let theirs = line(&s)?;
        s.write_all(format!("{}\n", proof("caller", key, &theirs)).as_bytes()).ok()?;
        let _ = s.set_read_timeout(None);
        Some(s)
    }

    /// Runs `f` on each connection to `listener`, each in a thread of its
    /// own (at most 32 at once, the rest waiting to be accepted), so one
    /// slow caller doesn't hold the rest.
    pub(crate) fn serve(listener: TcpListener, f: impl Fn(TcpStream) + Send + Sync + 'static) {
        use std::sync::{Arc, Condvar, Mutex};
        type Busy = Arc<(Mutex<usize>, Condvar)>;
        /// Frees its slot when the connection's thread ends, even by panic.
        struct Slot(Busy);
        impl Drop for Slot {
            fn drop(&mut self) {
                let (n, freed) = &*self.0;
                *n.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
                freed.notify_one();
            }
        }
        let f = Arc::new(f);
        let busy: Busy = Arc::new((Mutex::new(0), Condvar::new()));
        loop {
            {
                let (n, freed) = &*busy;
                let mut n = freed.wait_while(n.lock().unwrap_or_else(|e| e.into_inner()), |n| *n >= 32).unwrap_or_else(|e| e.into_inner());
                *n += 1;
            }
            let slot = Slot(busy.clone());
            let Some(stream) = listener.incoming().next().and_then(Result::ok) else {
                // (Out of descriptors, say: not at once again.)
                std::thread::sleep(Duration::from_millis(50));
                continue;
            };
            let f = f.clone();
            // (Without a thread the slot is freed here.)
            let _ = std::thread::Builder::new().name("apexdesk-ipc".into()).spawn(move || {
                let _slot = slot;
                f(stream);
            });
        }
    }

    /// Reads from a connection until a time in all, not per read: a caller
    /// sending a byte at a time can't hold its slot.
    pub(crate) struct Within<'a>(pub &'a TcpStream, pub std::time::Instant);

    impl Read for Within<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let left = self.1.checked_duration_since(std::time::Instant::now()).filter(|d| !d.is_zero()).ok_or(std::io::ErrorKind::TimedOut)?;
            self.0.set_read_timeout(Some(left))?;
            (&mut &*self.0).read(buf)
        }
    }

    /// The listener's side of [`connect`]: answers the caller's nonce and
    /// has the caller answer one, within seconds (one that can't isn't
    /// read from further).
    pub(crate) fn prove(mut s: &TcpStream, key: &str) -> bool {
        let Some(nonce) = line(s) else { return false };
        let ours = secret();
        if s.write_all(format!("{}\n{ours}\n", proof("instance", key, &nonce)).as_bytes()).is_err() || line(s).as_deref() != Some(proof("caller", key, &ours).as_str()) {
            return false;
        }
        // (What follows has as long per read as before.)
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        true
    }

    /// The port and secret in `file`.
    fn read(file: &Path) -> Option<(u16, String)> {
        let text = std::fs::read_to_string(file).ok()?;
        let (port, key) = text.trim().split_once(' ')?;
        Some((port.parse().ok()?, key.to_string()))
    }

    pub fn forward(file: &Path, files: &[PathBuf]) -> bool {
        let Some((port, key)) = read(file) else { return false };
        let Some(mut s) = connect(port, &key, Duration::from_secs(2)) else { return false };
        let mut msg = format!("{key}\n");
        for f in files {
            let abs = if f.to_string_lossy().starts_with("--") { f.clone() } else { std::path::absolute(f).unwrap_or_else(|_| f.clone()) };
            msg.push_str(&abs.to_string_lossy());
            msg.push('\n');
        }
        s.write_all(msg.as_bytes()).is_ok()
    }

    pub fn listen(file: PathBuf, ctx: egui::Context) -> Option<Receiver<PathBuf>> {
        // A live instance answers on the port in the file.
        if let Some((port, key)) = read(&file)
            && connect(port, &key, Duration::from_millis(300)).is_some()
        {
            return None;
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        let key = secret();
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&file, format!("{port} {key}\n")).ok()?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("apexdesk-instance".into())
            .spawn(move || {
                let tx = std::sync::Mutex::new(tx);
                serve(listener, move |stream| {
                    if !prove(&stream, &key) {
                        return;
                    }
                    let mut buf = String::new();
                    if Within(&stream, std::time::Instant::now() + Duration::from_secs(5)).take(64 << 10).read_to_string(&mut buf).is_err() {
                        return;
                    }
                    let mut lines = buf.lines();
                    if lines.next() != Some(key.as_str()) {
                        return;
                    }
                    let tx = tx.lock().unwrap_or_else(|e| e.into_inner());
                    for line in lines.filter(|l| !l.is_empty()) {
                        if tx.send(PathBuf::from(line)).is_err() {
                            return;
                        }
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.request_repaint();
                });
            })
            .ok()?;
        Some(rx)
    }
}

#[cfg(not(unix))]
fn instance_file() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("instance"))
}

#[cfg(not(unix))]
pub fn forward(files: &[PathBuf]) -> bool {
    !files.is_empty() && instance_file().is_some_and(|f| tcp::forward(&f, files))
}

#[cfg(not(unix))]
pub fn listen(ctx: egui::Context) -> Option<Receiver<PathBuf>> {
    let rx = tcp::listen(instance_file()?, ctx)?;
    OWNER.store(true, Ordering::Relaxed);
    Some(rx)
}

#[cfg(not(unix))]
pub fn cleanup() {
    if OWNER.load(Ordering::Relaxed)
        && let Some(f) = instance_file()
    {
        let _ = std::fs::remove_file(f);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn tcp_forwarding() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/tmp/instance-tcp"));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("instance");
        let ctx = eframe::egui::Context::default();
        let rx = super::tcp::listen(file.clone(), ctx.clone()).expect("listens");
        // A second instance sees the first one.
        assert!(super::tcp::listen(file.clone(), ctx).is_none());
        assert!(super::tcp::forward(&file, &[PathBuf::from("/a/b.docx"), PathBuf::from("--new-doc")]));
        let got: Vec<PathBuf> = (0..2).map(|_| rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap()).collect();
        assert_eq!(got, [PathBuf::from("/a/b.docx"), PathBuf::from("--new-doc")]);
        // Without the secret nothing gets through.
        let (port, _) = std::fs::read_to_string(&file).unwrap().trim().split_once(' ').map(|(p, k)| (p.parse::<u16>().unwrap(), k.to_string())).unwrap();
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::io::Write::write_all(&mut s, b"nonce\nwrong\n/evil\n").unwrap();
        drop(s);
        assert!(rx.recv_timeout(std::time::Duration::from_millis(500)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
