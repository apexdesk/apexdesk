//! Smithay Clipboard
//!
//! Provides access to the Wayland clipboard for gui applications. The user
//! should have surface around.

#![deny(clippy::all, clippy::if_not_else, clippy::enum_glob_use)]
use std::ffi::c_void;
use std::io::Result;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use sctk::reexports::calloop::channel::{self, Sender};
use sctk::reexports::client::Connection;
use sctk::reexports::client::backend::Backend;

mod mime;
mod state;
mod worker;

/// Access to a Wayland clipboard.
/// What a format request answers: the request's number and the format's
/// name and bytes. (ApexDesk patch.)
pub(crate) type FormatReply = (u64, Result<(String, Vec<u8>)>);

/// The clipboard of the window, for code that has no hold of the window's
/// `Clipboard`: one per process. (ApexDesk patch.)
struct Global {
    sender: Sender<worker::Command>,
    replies: Receiver<FormatReply>,
    next: u64,
}

static GLOBAL: Mutex<Option<Global>> = Mutex::new(None);

/// Whether a window's clipboard is running: the window uses the Wayland
/// backend. (ApexDesk patch.)
pub fn active() -> bool {
    GLOBAL.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Puts the same content on the clipboard in several formats, each under
/// its MIME type; `text/plain` is offered under the other names of text
/// too. `false` when no window's clipboard is running. (ApexDesk patch.)
pub fn store_formats(formats: Vec<(String, Vec<u8>)>) -> bool {
    let global = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
    global.as_ref().is_some_and(|g| g.sender.send(worker::Command::StoreFormats(formats)).is_ok())
}

fn ask(command: impl FnOnce(u64) -> worker::Command, timeout: Duration) -> Result<(String, Vec<u8>)> {
    let mut global = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
    let g = global.as_mut().ok_or_else(|| std::io::Error::other("no clipboard"))?;
    g.next += 1;
    let id = g.next;
    g.sender.send(command(id)).map_err(|_| std::io::Error::other("clipboard is dead."))?;
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match g.replies.recv_timeout(left) {
            // A late answer to an earlier request is dropped.
            Ok((n, reply)) if n == id => return reply,
            Ok(_) => continue,
            Err(_) => return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the clipboard's owner didn't answer")),
        }
    }
}

/// The first of `names` (MIME types; `text/plain` stands for any name of
/// text) the clipboard holds, and its bytes. An owner that doesn't answer
/// within `timeout` gives an error. (ApexDesk patch.)
pub fn load_format(names: &[&str], timeout: Duration) -> Result<(String, Vec<u8>)> {
    let names = names.iter().map(|n| n.to_string()).collect();
    ask(|id| worker::Command::LoadFormat(id, names), timeout)
}

/// The MIME types the clipboard holds. (ApexDesk patch.)
pub fn formats(timeout: Duration) -> Result<Vec<String>> {
    let (_, list) = ask(worker::Command::Formats, timeout)?;
    Ok(String::from_utf8_lossy(&list).lines().map(str::to_string).collect())
}

pub struct Clipboard {
    request_sender: Sender<worker::Command>,
    request_receiver: Receiver<Result<String>>,
    clipboard_thread: Option<std::thread::JoinHandle<()>>,
}

impl Clipboard {
    /// Creates new clipboard which will be running on its own thread with its
    /// own event queue to handle clipboard requests.
    ///
    /// # Safety
    ///
    /// `display` must be a valid `*mut wl_display` pointer, and it must remain
    /// valid for as long as `Clipboard` object is alive.
    pub unsafe fn new(display: *mut c_void) -> Self {
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let connection = Connection::from_backend(backend);

        // Create channel to send data to clipboard thread.
        let (request_sender, rx_chan) = channel::channel();
        // Create channel to get data from the clipboard thread.
        let (clipboard_reply_sender, request_receiver) = mpsc::channel();
        let (format_sender, format_receiver) = mpsc::channel();

        let name = String::from("smithay-clipboard");
        let clipboard_thread =
            worker::spawn(name, connection, rx_chan, clipboard_reply_sender, format_sender);
        *GLOBAL.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Global { sender: request_sender.clone(), replies: format_receiver, next: 0 });

        Self { request_receiver, request_sender, clipboard_thread }
    }

    /// Load clipboard data.
    ///
    /// Loads content from a clipboard on a last observed seat.
    pub fn load(&self) -> Result<String> {
        let _ = self.request_sender.send(worker::Command::Load);

        if let Ok(reply) = self.request_receiver.recv() {
            reply
        } else {
            // The clipboard thread is dead, however we shouldn't crash downstream, so
            // propogating an error.
            Err(std::io::Error::other("clipboard is dead."))
        }
    }

    /// Store to a clipboard.
    ///
    /// Stores to a clipboard on a last observed seat.
    pub fn store<T: Into<String>>(&self, text: T) {
        let request = worker::Command::Store(text.into());
        let _ = self.request_sender.send(request);
    }

    /// Load primary clipboard data.
    ///
    /// Loads content from a  primary clipboard on a last observed seat.
    pub fn load_primary(&self) -> Result<String> {
        let _ = self.request_sender.send(worker::Command::LoadPrimary);

        if let Ok(reply) = self.request_receiver.recv() {
            reply
        } else {
            // The clipboard thread is dead, however we shouldn't crash downstream, so
            // propogating an error.
            Err(std::io::Error::other("clipboard is dead."))
        }
    }

    /// Store to a primary clipboard.
    ///
    /// Stores to a primary clipboard on a last observed seat.
    pub fn store_primary<T: Into<String>>(&self, text: T) {
        let request = worker::Command::StorePrimary(text.into());
        let _ = self.request_sender.send(request);
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        *GLOBAL.lock().unwrap_or_else(|e| e.into_inner()) = None;
        // Shutdown smithay-clipboard.
        let _ = self.request_sender.send(worker::Command::Exit);
        if let Some(clipboard_thread) = self.clipboard_thread.take() {
            let _ = clipboard_thread.join();
        }
    }
}
