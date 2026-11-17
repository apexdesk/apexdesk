//! Files handed over by the Finder on macOS. A double click, Open With or
//! a drop on the Dock icon doesn't start the program with the file as an
//! argument: the system sends the application an "open documents" event,
//! which AppKit gives to the application's delegate. The window library's
//! delegate doesn't take it, and the system then says the file can't be
//! opened. Its delegate is given a subclass that takes it, made when the
//! program starts: the delegate stays the window library's own object.

use std::ffi::{CStr, OsStr, c_char};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender};

use eframe::egui;
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{msg_send, sel};

/// Files that came before the window was there to open them, then where
/// they go.
struct Inbox {
    waiting: Vec<PathBuf>,
    to: Option<(Sender<PathBuf>, egui::Context)>,
}

static INBOX: Mutex<Inbox> = Mutex::new(Inbox { waiting: Vec::new(), to: None });

/// Call before the event loop starts. The delegate exists once the window
/// library has made its event loop, and the files of a double click come
/// before the application has finished launching: in between, the system
/// says it is about to, and that is when the delegate learns to take them.
pub fn install() {
    let Some(center) = AnyClass::get(c"NSNotificationCenter") else { return };
    let Some(string) = AnyClass::get(c"NSString") else { return };
    let block = block2::RcBlock::new(|_note: *mut AnyObject| teach_delegate());
    objc2::rc::autoreleasepool(|_| {
        // SAFETY: messages to Foundation classes that exist, with the
        // arguments they take. The center copies the block and keeps the
        // observer it returns for as long as the program runs.
        unsafe {
            let center: *mut AnyObject = msg_send![center, defaultCenter];
            let name: *mut AnyObject = msg_send![string, stringWithUTF8String: c"NSApplicationWillFinishLaunchingNotification".as_ptr()];
            if center.is_null() || name.is_null() {
                return;
            }
            let nil: *mut AnyObject = std::ptr::null_mut();
            let _observer: *mut AnyObject = msg_send![center, addObserverForName: name, object: nil, queue: nil, usingBlock: &*block];
        }
    });
}

/// Makes the application's delegate one of a subclass of its own class
/// that answers `application:openFiles:`.
fn teach_delegate() {
    let Some(app) = AnyClass::get(c"NSApplication") else { return };
    // SAFETY: on the main thread, where AppKit sends the notification.
    // The new class adds a method and nothing to the object's layout, so
    // the delegate is as valid an instance of it as of its own class, and
    // still one of that class for the window library.
    unsafe {
        let app: *mut AnyObject = msg_send![app, sharedApplication];
        if app.is_null() {
            return;
        }
        let delegate: *mut AnyObject = msg_send![app, delegate];
        let Some(delegate) = delegate.as_ref() else { return };
        let Some(mut class) = ClassBuilder::new(c"ApexDeskApplicationDelegate", delegate.class()) else { return };
        class.add_method(sel!(application:openFiles:), open_files as unsafe extern "C-unwind" fn(_, _, _, _));
        AnyObject::set_class(delegate, class.register());
    }
}

/// `-[NSApplicationDelegate application:openFiles:]`: `files` is an
/// NSArray of NSString paths.
unsafe extern "C-unwind" fn open_files(_this: &AnyObject, _cmd: Sel, app: *mut AnyObject, files: *mut AnyObject) {
    let mut paths = Vec::new();
    // SAFETY: AppKit calls this with its application and an array of
    // strings; each string's bytes are copied while the array is alive.
    unsafe {
        let count: usize = if files.is_null() { 0 } else { msg_send![files, count] };
        for i in 0..count {
            let name: *mut AnyObject = msg_send![files, objectAtIndex: i];
            if name.is_null() {
                continue;
            }
            let bytes: *const c_char = msg_send![name, fileSystemRepresentation];
            if !bytes.is_null() {
                paths.push(PathBuf::from(OsStr::from_bytes(CStr::from_ptr(bytes).to_bytes())));
            }
        }
        // The system waits for the answer (0: the files were taken).
        if !app.is_null() {
            let _: () = msg_send![app, replyToOpenOrPrint: 0usize];
        }
    }
    receive(paths);
}

fn receive(paths: Vec<PathBuf>) {
    let mut inbox = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    match &inbox.to {
        Some((tx, ctx)) => {
            for p in paths {
                let _ = tx.send(p);
            }
            ctx.request_repaint();
        }
        None => inbox.waiting.extend(paths),
    }
}

/// The files to open as tabs: the Finder's along with those other
/// processes send (`instance::listen`). The ones that came before the
/// window are the first in it.
pub fn join(others: Option<Receiver<PathBuf>>, ctx: &egui::Context) -> Option<Receiver<PathBuf>> {
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let mut inbox = INBOX.lock().unwrap_or_else(|e| e.into_inner());
        for p in inbox.waiting.drain(..) {
            let _ = tx.send(p);
        }
        inbox.to = Some((tx.clone(), ctx.clone()));
    }
    if let Some(others) = others {
        let ctx = ctx.clone();
        let _ = apex_fs::thread::Builder::new().name("apexdesk-opened".into()).spawn(move || {
            for p in others {
                if tx.send(p).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
    }
    Some(rx)
}
