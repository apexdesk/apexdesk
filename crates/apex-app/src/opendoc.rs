//! Documents opened from the Finder (macOS): a double-clicked spreadsheet,
//! drawing or presentation reaches the window instead of a dialog saying
//! the format can't be opened.
//!
//! macOS hands the files to the application's delegate
//! (`application:openURLs:`), not as command-line arguments, so they don't
//! come in the way [`crate::instance`]'s files do. winit's delegate, which
//! eframe uses, implements neither `application:openURLs:` nor
//! `application:openFiles:`; with neither, AppKit puts up its own alert
//! ("… no puede abrir archivos en formato …"). Adding those methods to the
//! delegate class before the event arrives both takes the files and keeps
//! the alert away; the event arrives before the run loop, so the patch has
//! to be in place before the window starts ([`install`], from `main`).
//! A patch installed later (while the run loop turns,
//! `applicationDidFinishLaunching:`) is never asked and the alert stands.
//!
//! The delegate is winit's own and exists only once eframe builds its
//! event loop, so [`install`] replaces `-[NSApplication setDelegate:]` and
//! patches the class on the way past, whichever delegate it is.

/// Takes the paths of the documents asked for since the last call, oldest
/// first (a few, once a window starts; the Finder sends one at a time).
#[cfg(target_os = "macos")]
pub(crate) use macos::take;

/// Nothing arrives this way elsewhere.
#[cfg(not(target_os = "macos"))]
pub(crate) fn take() -> Vec<std::path::PathBuf> {
    Vec::new()
}

/// Prepares the application to take documents opened in the file manager.
/// Call before the window starts.
#[cfg(not(target_os = "macos"))]
pub fn install() {}

#[cfg(target_os = "macos")]
pub use macos::install;

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{CStr, c_char, c_void};
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::PathBuf;
    use std::ptr;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use std::sync::{Mutex, Once};

    /// An Objective-C object, class, method or selector (all pointers).
    type Id = *mut c_void;
    type Sel = *mut c_void;

    /// The method type of the open-documents delegate methods:
    /// void, and the hidden self and `_cmd`, then the application and the
    /// files.
    const OPEN_TYPES: &CStr = c"v@:@@";

    // The Objective-C runtime, as linked into libobjc (a handful of calls:
    // an `objc2` dependency for them would be out of proportion).
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn class_getInstanceMethod(cls: Id, sel: Sel) -> *mut c_void;
        fn class_addMethod(cls: Id, sel: Sel, imp: *mut c_void, types: *const c_char) -> i8;
        fn method_getImplementation(method: *mut c_void) -> *mut c_void;
        fn method_setImplementation(method: *mut c_void, imp: *mut c_void) -> *mut c_void;
        /// The message sender, sent to with the receiver method's own
        /// signature.
        fn objc_msgSend();
    }

    /// Sends `sel` to `obj`, reading from the message-sending symbol the
    /// shape the method named has.
    ///
    /// # Safety
    /// `sel` must name a method of `obj` that takes no arguments and
    /// returns `R`.
    unsafe fn msg0<R>(obj: Id, sel: Sel) -> R {
        let f: unsafe extern "C" fn(Id, Sel) -> R = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { f(obj, sel) }
    }

    /// As [`msg0`], with one argument.
    ///
    /// # Safety
    /// `sel` must name a method of `obj` that takes `A` and returns `R`.
    unsafe fn msg1<R, A>(obj: Id, sel: Sel, a: A) -> R {
        let f: unsafe extern "C" fn(Id, Sel, A) -> R = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { f(obj, sel, a) }
    }

    /// The blocks the Finder hands over and the window hasn't taken yet.
    static INBOX: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    /// Whether [`INBOX`] holds anything, to skip its lock every frame.
    static WAITING: AtomicBool = AtomicBool::new(false);

    /// The implementations replaced here, to call through to (a delegate
    /// that isn't winit's may have its own).
    static SET_DELEGATE_WAS: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
    static OPEN_URLS_WAS: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
    static OPEN_FILES_WAS: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

    /// The selector `name` (registered runtime-wide on first use).
    fn sel(name: &CStr) -> Sel {
        unsafe { sel_registerName(name.as_ptr()) }
    }

    /// The class `name`, or null.
    fn class(name: &CStr) -> Id {
        unsafe { objc_getClass(name.as_ptr()) }
    }

    /// The path an `NSURL` (or a plain, path-shaped `NSString`) names.
    ///
    /// # Safety
    /// `s` is an `NSURL`/`NSString`, or null.
    unsafe fn path_of(s: Id) -> Option<PathBuf> {
        if s.is_null() {
            return None;
        }
        let path: Id = unsafe { msg0(s, sel(c"path")) };
        let text = if path.is_null() { s } else { path };
        let bytes: *const c_char = unsafe { msg0(text, sel(c"UTF8String")) };
        if bytes.is_null() {
            return None;
        }
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(unsafe { CStr::from_ptr(bytes) }.to_bytes()));
        (!path.as_os_str().is_empty()).then_some(path)
    }

    /// The paths of the `NSURL`s (or of the file names) in `list`.
    ///
    /// # Safety
    /// `list` is an `NSArray` of `NSURL`s (`urls`), or one of strings.
    unsafe fn paths_in(list: Id, urls: bool) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if list.is_null() {
            return paths;
        }
        let count: usize = unsafe { msg0(list, sel(c"count")) };
        for i in 0..count {
            let item: Id = unsafe { msg1(list, sel(c"objectAtIndex:"), i) };
            if item.is_null() {
                continue;
            }
            // A URL that names something on the web isn't a document.
            let file: i8 = unsafe { msg0(item, sel(c"isFileURL")) };
            if urls && file == 0 {
                continue;
            }
            if let Some(p) = unsafe { path_of(item) } {
                paths.push(p);
            }
        }
        paths
    }

    /// Keeps `paths` for the window; not one twice (AppKit may hand a
    /// document over more than once).
    fn keep(paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let mut inbox = INBOX.lock().unwrap_or_else(|e| e.into_inner());
        for p in paths {
            if !inbox.contains(&p) {
                inbox.push(p);
            }
        }
        WAITING.store(true, Ordering::Release);
    }

    /// Takes the kept paths.
    pub(crate) fn take() -> Vec<PathBuf> {
        if !WAITING.swap(false, Ordering::AcqRel) {
            return Vec::new();
        }
        std::mem::take(&mut *INBOX.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Calls the implementation `was` holds, which takes `cmd` and one
    /// argument.
    ///
    /// # Safety
    /// That implementation is the method `cmd` of `this`.
    unsafe fn call_through1(was: &AtomicPtr<c_void>, this: Id, cmd: Sel, a: Id) {
        let imp = was.load(Ordering::Acquire);
        if !imp.is_null() {
            let f: unsafe extern "C" fn(Id, Sel, Id) = unsafe { std::mem::transmute(imp) };
            unsafe { f(this, cmd, a) };
        }
    }

    /// As [`call_through1`], with two arguments.
    ///
    /// # Safety
    /// That implementation is the method `cmd` of `this`.
    unsafe fn call_through2(was: &AtomicPtr<c_void>, this: Id, cmd: Sel, a: Id, b: Id) {
        let imp = was.load(Ordering::Acquire);
        if !imp.is_null() {
            let f: unsafe extern "C" fn(Id, Sel, Id, Id) = unsafe { std::mem::transmute(imp) };
            unsafe { f(this, cmd, a, b) };
        }
    }

    /// `-[NSApplicationDelegate application:openURLs:]` of ours.
    ///
    /// SAFETY: the runtime calls it with the application and an array of
    /// `NSURL`s.
    unsafe extern "C" fn open_urls(this: Id, cmd: Sel, app: Id, urls: Id) {
        keep(unsafe { paths_in(urls, true) });
        unsafe { call_through2(&OPEN_URLS_WAS, this, cmd, app, urls) };
    }

    /// `-[NSApplicationDelegate application:openFiles:]` of ours.
    ///
    /// SAFETY: the runtime calls it with the application and an array of
    /// file names.
    unsafe extern "C" fn open_files(this: Id, cmd: Sel, app: Id, files: Id) {
        keep(unsafe { paths_in(files, false) });
        unsafe { call_through2(&OPEN_FILES_WAS, this, cmd, app, files) };
    }

    /// Gives `cls` `imp` as `selector`: added when neither the class nor
    /// its superclasses have it (the alert comes from that absence), else
    /// replaced, keeping the one it had to call through to.
    ///
    /// # Safety
    /// `imp` takes `selector`'s arguments and returns its type.
    unsafe fn patch(cls: Id, selector: Sel, imp: *mut c_void, was: &AtomicPtr<c_void>) {
        unsafe {
            let method = class_getInstanceMethod(cls, selector);
            if method.is_null() {
                class_addMethod(cls, selector, imp, OPEN_TYPES.as_ptr());
                return;
            }
            let old = method_getImplementation(method);
            if old != imp {
                method_setImplementation(method, imp);
                was.store(old, Ordering::Release);
            }
        }
    }

    /// Adds the open-documents methods to the class of `delegate`.
    fn patch_delegate(delegate: Id) {
        if delegate.is_null() {
            return;
        }
        let cls: Id = unsafe { msg0(delegate, sel(c"class")) };
        if cls.is_null() {
            return;
        }
        let urls = open_urls as unsafe extern "C" fn(Id, Sel, Id, Id) as *mut c_void;
        let files = open_files as unsafe extern "C" fn(Id, Sel, Id, Id) as *mut c_void;
        unsafe {
            patch(cls, sel(c"application:openURLs:"), urls, &OPEN_URLS_WAS);
            patch(cls, sel(c"application:openFiles:"), files, &OPEN_FILES_WAS);
        }
    }

    /// `-[NSApplication setDelegate:]` of ours: the delegate is winit's,
    /// and is patched as it is set, before the run loop where the
    /// documents arrive.
    ///
    /// SAFETY: the runtime calls it with the new delegate.
    unsafe extern "C" fn set_delegate(this: Id, cmd: Sel, delegate: Id) {
        patch_delegate(delegate);
        unsafe { call_through1(&SET_DELEGATE_WAS, this, cmd, delegate) };
    }

    /// Takes over `-[NSApplication setDelegate:]`, once.
    pub fn install() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            let method = class_getInstanceMethod(class(c"NSApplication"), sel(c"setDelegate:"));
            if method.is_null() {
                return;
            }
            let imp = set_delegate as unsafe extern "C" fn(Id, Sel, Id) as *mut c_void;
            SET_DELEGATE_WAS.store(method_getImplementation(method), Ordering::Release);
            method_setImplementation(method, imp);
        });
    }
}
