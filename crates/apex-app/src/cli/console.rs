//! The console of the command line on Windows. Release builds are GUI
//! programs (no console window when opened from Explorer), so they start
//! without standard handles: the command line attaches to the console of
//! the shell that started it, and the terminal interface gets a console of
//! its own when there is none. Output that goes to a file or a pipe keeps
//! its handles.
//!
//! cmd.exe does not wait for GUI programs: its prompt comes back while the
//! command still prints. `start /wait apexdesk ...` (or piping the output,
//! `apexdesk cat x.xlsx | more`) waits and keeps %ERRORLEVEL%.

#[cfg(windows)]
pub fn attach(tui: bool) {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(pid: u32) -> i32;
        fn AllocConsole() -> i32;
        fn GetConsoleWindow() -> *mut c_void;
        fn GetStdHandle(which: u32) -> *mut c_void;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    // SAFETY: plain Win32 calls without pointers passed in.
    unsafe {
        if !GetConsoleWindow().is_null() {
            return;
        }
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        let redirected = !out.is_null() && out as isize != -1;
        if tui {
            // The interface draws on a console: the shell's one would read
            // the same keys, so it gets its own window.
            AllocConsole();
        } else if !redirected {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(not(windows))]
pub fn attach(_tui: bool) {}
