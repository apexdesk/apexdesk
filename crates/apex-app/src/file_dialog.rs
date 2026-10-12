//! The system's file dialogs (rfd), with the same builder calls. A dialog
//! doesn't block the window: it opens without waiting (rfd's async
//! dialogs: a thread of their own on Linux and Windows, a sheet of the
//! window on macOS), the window keeps painting and answering the
//! desktop meanwhile, and what the code does with the answer runs in a
//! later frame (`poll`). One dialog at a time: asking for another while
//! one is open counts as cancelled.
//!
//! Under `cargo test` no window opens: each request is recorded and a
//! hook set by the test answers it (by default: cancelled), in the next
//! frame like a real answer.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use eframe::egui;

use crate::app::{App, View};

/// Asks for files or a folder.
pub struct FileDialog {
    #[cfg(not(test))]
    inner: rfd::AsyncFileDialog,
    #[cfg(test)]
    title: String,
    #[cfg(test)]
    file_name: String,
    #[cfg(test)]
    exts: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pick,
    PickMany,
    Folder,
    Save,
}

/// What a dialog does with its answer (None: cancelled).
type Then = Box<dyn FnOnce(&mut App, Option<Vec<PathBuf>>)>;

/// A dialog asked for, answered or not yet.
struct Pending {
    answer: mpsc::Receiver<Option<Vec<PathBuf>>>,
    got: Option<Option<Vec<PathBuf>>>,
    then: Then,
}

/// The window dialogs belong to.
#[derive(Clone, Copy)]
struct Parent(raw_window_handle::RawWindowHandle, raw_window_handle::RawDisplayHandle);

thread_local! {
    /// The dialog open now (first), and those refused meanwhile.
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
    static OWNER: Cell<u64> = const { Cell::new(0) };
    static CTX: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
    static PARENT: Cell<Option<Parent>> = const { Cell::new(None) };
}

impl FileDialog {
    pub fn new() -> FileDialog {
        FileDialog {
            #[cfg(not(test))]
            inner: rfd::AsyncFileDialog::new(),
            #[cfg(test)]
            title: String::new(),
            #[cfg(test)]
            file_name: String::new(),
            #[cfg(test)]
            exts: Vec::new(),
        }
    }

    pub fn set_title(mut self, title: impl Into<String>) -> FileDialog {
        let title = title.into();
        #[cfg(test)]
        {
            self.title = title;
        }
        #[cfg(not(test))]
        {
            self.inner = self.inner.set_title(title);
        }
        self
    }

    pub fn add_filter(mut self, name: impl Into<String>, exts: &[impl ToString]) -> FileDialog {
        #[cfg(test)]
        {
            let _ = name.into();
            self.exts.extend(exts.iter().map(|e| e.to_string()));
        }
        #[cfg(not(test))]
        {
            self.inner = self.inner.add_filter(name, exts);
        }
        self
    }

    pub fn set_file_name(mut self, name: impl Into<String>) -> FileDialog {
        let name = name.into();
        #[cfg(test)]
        {
            self.file_name = name;
        }
        #[cfg(not(test))]
        {
            self.inner = self.inner.set_file_name(name);
        }
        self
    }

    pub fn set_directory(#[allow(unused_mut)] mut self, dir: impl AsRef<Path>) -> FileDialog {
        #[cfg(not(test))]
        {
            self.inner = self.inner.set_directory(dir);
        }
        #[cfg(test)]
        let _ = dir;
        self
    }

    /// Asks for a file to open; `then` gets it (nothing happens if the
    /// dialog is cancelled).
    pub fn pick_file(self, then: impl FnOnce(&mut App, PathBuf) + 'static) {
        self.pick_file_or_cancel(move |app, p| {
            if let Some(p) = p {
                then(app, p);
            }
        });
    }

    /// Asks for a file to open; `then` gets it, or None if cancelled.
    pub fn pick_file_or_cancel(self, then: impl FnOnce(&mut App, Option<PathBuf>) + 'static) {
        self.start(Kind::Pick, Box::new(move |app, a| then(app, a.and_then(|v| v.into_iter().next()))));
    }

    pub fn pick_files(self, then: impl FnOnce(&mut App, Vec<PathBuf>) + 'static) {
        self.start(
            Kind::PickMany,
            Box::new(move |app, a| {
                if let Some(v) = a.filter(|v| !v.is_empty()) {
                    then(app, v);
                }
            }),
        );
    }

    pub fn pick_folder(self, then: impl FnOnce(&mut App, PathBuf) + 'static) {
        self.start(
            Kind::Folder,
            Box::new(move |app, a| {
                if let Some(p) = a.and_then(|v| v.into_iter().next()) {
                    then(app, p);
                }
            }),
        );
    }

    /// Asks where to save; `then` gets the path.
    pub fn save_file(self, then: impl FnOnce(&mut App, PathBuf) + 'static) {
        self.save_file_or_cancel(move |app, p| {
            if let Some(p) = p {
                then(app, p);
            }
        });
    }

    /// Asks where to save; `then` gets the path, or None if cancelled.
    pub fn save_file_or_cancel(self, then: impl FnOnce(&mut App, Option<PathBuf>) + 'static) {
        self.start(Kind::Save, Box::new(move |app, a| then(app, a.and_then(|v| v.into_iter().next()))));
    }

    fn start(self, kind: Kind, then: Then) {
        if busy() {
            // Cancelled, in the next frame like any answer.
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(None);
            PENDING.with_borrow_mut(|p| p.push(Pending { answer: rx, got: None, then }));
            return;
        }
        let (tx, rx) = mpsc::channel();
        #[cfg(test)]
        {
            let req = test::Request { kind, title: self.title, file_name: self.file_name, exts: self.exts };
            test::reply(tx, test::answer(req));
        }
        #[cfg(not(test))]
        if std::env::var_os("APEXDESK_SCRIPT").is_some() {
            // A scripted run never opens a dialog: the answer is the path
            // in APEXDESK_DIALOG, else cancelled.
            let _ = (kind, self.inner);
            let _ = tx.send(std::env::var_os("APEXDESK_DIALOG").map(|p| vec![PathBuf::from(p)]));
        } else {
            let mut dlg = self.inner;
            if let Some(p) = PARENT.get() {
                dlg = dlg.set_parent(&p);
            }
            // Made here, on the main thread (macOS shows it as a sheet of
            // the window now); another thread waits for the answer.
            let answer: std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<PathBuf>>> + Send>> = match kind {
                Kind::Pick => Box::pin(one(dlg.pick_file())),
                Kind::PickMany => {
                    let f = dlg.pick_files();
                    Box::pin(async move { f.await.map(|v| v.iter().map(|h| h.path().to_path_buf()).collect()) })
                }
                Kind::Folder => Box::pin(one(dlg.pick_folder())),
                Kind::Save => Box::pin(one(dlg.save_file())),
            };
            let ctx = CTX.with_borrow(Clone::clone);
            let spawned = std::thread::Builder::new().name("file dialog".into()).spawn(move || {
                let _ = tx.send(block_on(answer));
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
            if let Err(e) = spawned {
                eprintln!("file dialog: {e}");
            }
        }
        PENDING.with_borrow_mut(|p| p.push(Pending { answer: rx, got: None, then }));
    }
}

/// A single file's answer as a list.
#[cfg(not(test))]
async fn one(f: impl std::future::Future<Output = Option<rfd::FileHandle>>) -> Option<Vec<PathBuf>> {
    f.await.map(|h| vec![h.path().to_path_buf()])
}

/// Runs a future to its end on this thread.
#[cfg(not(test))]
fn block_on<T>(mut f: std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>) -> T {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}

impl raw_window_handle::HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // The app's one window lives as long as the app.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.0) })
    }
}

impl raw_window_handle::HasDisplayHandle for Parent {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(self.1) })
    }
}

/// A new frame of the window: notes what dialogs need of it.
pub fn frame(ctx: &egui::Context, frame: &eframe::Frame) {
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
    CTX.with_borrow_mut(|c| {
        if c.is_none() {
            *c = Some(ctx.clone());
        }
    });
    if let (Ok(w), Ok(d)) = (frame.window_handle(), frame.display_handle()) {
        PARENT.set(Some(Parent(w.as_raw(), d.as_raw())));
    }
}

/// Whether a dialog is open.
pub fn busy() -> bool {
    PENDING.with_borrow(|p| !p.is_empty())
}

/// Does what the dialogs were for, once they're answered.
pub fn poll(app: &mut App) {
    loop {
        let ready = PENDING.with_borrow_mut(|p| {
            for d in p.iter_mut().filter(|d| d.got.is_none()) {
                d.got = match d.answer.try_recv() {
                    Ok(a) => Some(a),
                    // A thread that died without answering: cancelled.
                    Err(mpsc::TryRecvError::Disconnected) => Some(None),
                    Err(mpsc::TryRecvError::Empty) => None,
                };
            }
            let i = p.iter().position(|d| d.got.is_some())?;
            Some(p.remove(i))
        });
        let Some(d) = ready else { return };
        (d.then)(app, d.got.flatten());
    }
}

/// The tab whose view is drawn (or acted on) now: a view's dialog
/// answers that tab.
pub fn set_owner(tab: u64) {
    OWNER.set(tab);
}

/// A view of some kind.
pub trait ViewOf {
    fn of(view: &mut View) -> Option<&mut Self>;
}

macro_rules! view_of {
    ($t:ty, $v:ident) => {
        impl ViewOf for $t {
            fn of(view: &mut View) -> Option<&mut Self> {
                match view {
                    View::$v(v) => Some(v),
                    _ => None,
                }
            }
        }
    };
}

view_of!(crate::sheet_view::SheetView, Sheet);
// A notebook's open page is a document too: what its editor asked for
// (a picture, a file to link) goes to it.
impl ViewOf for crate::doc_view::DocView {
    fn of(view: &mut View) -> Option<&mut Self> {
        match view {
            View::Doc(v) => Some(v),
            View::Note(n) => n.editor.as_deref_mut(),
            _ => None,
        }
    }
}
view_of!(crate::slides_view::SlidesView, Slides);
view_of!(crate::pdf_view::PdfView, Pdf);
view_of!(crate::db_view::DbView, Db);
view_of!(crate::note_view::NoteView, Note);

/// An answer for the view that asks now (dropped if its tab has closed
/// meanwhile).
pub fn in_view<V: ViewOf + 'static, T: 'static>(f: impl FnOnce(&mut V, T) + 'static) -> impl FnOnce(&mut App, T) + 'static {
    let tab = OWNER.get();
    move |app, x| {
        if let Some(v) = app.tab_view_mut(tab).and_then(V::of) {
            f(v, x);
        }
    }
}

/// An answer kept for the part of the window that asked, which takes it
/// when it's next drawn (dropped with it).
pub struct Slot<T>(Rc<RefCell<Option<T>>>);

impl<T> Default for Slot<T> {
    fn default() -> Slot<T> {
        Slot(Rc::new(RefCell::new(None)))
    }
}

impl<T: 'static> Slot<T> {
    /// What fills the slot, to give a dialog.
    pub fn fill(&self) -> impl FnOnce(&mut App, T) + 'static {
        let s = self.0.clone();
        move |_, x| *s.borrow_mut() = Some(x)
    }

    /// The answer, once.
    pub fn take(&self) -> Option<T> {
        self.0.borrow_mut().take()
    }
}

#[cfg(test)]
pub mod test {
    //! What the tests see of the file dialogs.

    use std::cell::RefCell;
    use std::path::PathBuf;

    pub use super::Kind;

    /// A file dialog the code asked for.
    #[derive(Clone, Debug)]
    pub struct Request {
        pub kind: Kind,
        pub title: String,
        pub file_name: String,
        pub exts: Vec<String>,
    }

    type Hook = Box<dyn FnMut(&Request) -> Option<Vec<PathBuf>>>;

    type Reply = (std::sync::mpsc::Sender<Option<Vec<PathBuf>>>, Option<Vec<PathBuf>>);

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = RefCell::new(None);
        static LOG: RefCell<Vec<Request>> = const { RefCell::new(Vec::new()) };
        /// Answers held back, as if the user took their time.
        static HELD: RefCell<Option<Vec<Reply>>> = const { RefCell::new(None) };
    }

    pub(super) fn reply(tx: std::sync::mpsc::Sender<Option<Vec<PathBuf>>>, answer: Option<Vec<PathBuf>>) {
        let held = HELD.with_borrow_mut(|h| match h {
            Some(h) => {
                h.push((tx.clone(), answer.clone()));
                true
            }
            None => false,
        });
        if !held {
            let _ = tx.send(answer);
        }
    }

    /// Holds the dialogs' answers back (the dialogs stay open) until
    /// `release`.
    pub fn hold() {
        HELD.with_borrow_mut(|h| *h = Some(Vec::new()));
    }

    /// Gives the answers held back.
    pub fn release() {
        for (tx, answer) in HELD.with_borrow_mut(Option::take).unwrap_or_default() {
            let _ = tx.send(answer);
        }
    }

    pub(super) fn answer(req: Request) -> Option<Vec<PathBuf>> {
        LOG.with_borrow_mut(|l| l.push(req.clone()));
        let hook = HOOK.with_borrow_mut(Option::take);
        let Some(mut hook) = hook else { return None };
        let out = hook(&req);
        HOOK.with_borrow_mut(|h| {
            if h.is_none() {
                *h = Some(hook);
            }
        });
        out
    }

    /// Answers this thread's dialogs with `hook` (None: cancel them all).
    /// A dialog still waiting for its answer is dropped.
    pub fn set_hook(hook: Option<Hook>) {
        HOOK.with_borrow_mut(|h| *h = hook);
        HELD.with_borrow_mut(|h| *h = None);
        super::PENDING.with_borrow_mut(Vec::clear);
    }

    /// The dialogs asked for on this thread since the last call.
    pub fn take_requests() -> Vec<Request> {
        LOG.with_borrow_mut(std::mem::take)
    }
}
