//! Reading a password in the terminal without showing it (termios on
//! Linux and macOS, the console mode on Windows). When the standard input
//! is not a terminal, its first line is the password.

use std::io::{BufRead, IsTerminal, Write};

pub fn read(prompt: &str) -> std::io::Result<String> {
    let stdin = std::io::stdin();
    let mut line = String::new();
    if !stdin.is_terminal() {
        stdin.lock().read_line(&mut line)?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_string());
    }
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let echo = NoEcho::new();
    let res = stdin.lock().read_line(&mut line);
    drop(echo);
    eprintln!();
    res?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// Turns the terminal's echo off while it lives.
#[cfg(unix)]
struct NoEcho(Option<libc::termios>);

#[cfg(unix)]
impl NoEcho {
    fn new() -> Self {
        // SAFETY: termios calls on the standard input with a zeroed struct
        // they fill in.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut t) != 0 {
                return NoEcho(None);
            }
            let old = t;
            t.c_lflag &= !libc::ECHO;
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
            NoEcho(Some(old))
        }
    }
}

#[cfg(unix)]
impl Drop for NoEcho {
    fn drop(&mut self) {
        if let Some(old) = &self.0 {
            // SAFETY: restores the settings read in `new`.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, old);
            }
        }
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetStdHandle(which: u32) -> *mut c_void;
        pub fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        pub fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
    }
    pub const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    pub const ENABLE_ECHO_INPUT: u32 = 0x0004;
}

#[cfg(windows)]
struct NoEcho(Option<u32>);

#[cfg(windows)]
impl NoEcho {
    fn new() -> Self {
        // SAFETY: console calls on the standard input handle.
        unsafe {
            let h = win::GetStdHandle(win::STD_INPUT_HANDLE);
            let mut mode = 0u32;
            if win::GetConsoleMode(h, &mut mode) == 0 {
                return NoEcho(None);
            }
            win::SetConsoleMode(h, mode & !win::ENABLE_ECHO_INPUT);
            NoEcho(Some(mode))
        }
    }
}

#[cfg(windows)]
impl Drop for NoEcho {
    fn drop(&mut self) {
        if let Some(mode) = self.0 {
            // SAFETY: restores the mode read in `new`.
            unsafe {
                win::SetConsoleMode(win::GetStdHandle(win::STD_INPUT_HANDLE), mode);
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct NoEcho;

#[cfg(not(any(unix, windows)))]
impl NoEcho {
    fn new() -> Self {
        NoEcho
    }
}
