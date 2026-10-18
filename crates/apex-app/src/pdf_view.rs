//! The PDF tab: a PDF's pages as they are (drawn by pdfium when it is
//! installed, see [`engine`], otherwise by [`simple`]), with thumbnails,
//! bookmarks and comments at the side, search, text selection, and the
//! tools to annotate, fill in, sign, redact and organize it. The file is
//! edited in place ([`apex_io::pdf::edit::PdfDoc`]) and saved as an
//! incremental update whenever it can, which keeps earlier signatures.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use apex_io::pdf::edit::PdfDoc;
use apex_io::pdf::edit::annots::Kind;
use eframe::egui::{self, Key, Modifiers, TextureHandle};

use crate::app::Notice;
use crate::common::{Fit, Shell};
use crate::i18n::{tr, trf};

mod annotate;
mod dark;
mod comments;
pub mod compare_ui;
pub mod engine;
mod form_ui;
pub mod organize_ui;
mod pages;
mod print_box;
pub mod redact_ui;
mod show;
mod side;
mod sign_box;
pub mod simple;
pub mod sysfonts;
pub mod stamp_ui;
#[cfg_attr(not(feature = "empresa"), path = "pdf_view/circuit_ui_free.rs")]
pub mod circuit_ui;
mod tabs;
mod tabs_mark;
mod text;
pub mod tools_ui;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_pages;
#[cfg(test)]
mod tests_tools;
// Against pdfium: only with the `pdfium` feature.
#[cfg(all(test, feature = "pdfium"))]
mod render_cmp;

/// What the tab asks the application to do.
pub enum Request {
    /// Open the PDF as an editable document.
    Convert,
    /// A print job ended: the printer's name, or what went wrong.
    Printed(Result<String, String>),
    /// Sign the file with a certificate: in this empty signature field, or
    /// in a new invisible one.
    Sign(Option<String>),
    /// Show the file's signatures.
    Signatures,
    /// Save these pages as a new file.
    Extract(Vec<usize>),
    /// Save each range of pages (with a name, maybe empty) as a file.
    Split(Vec<(String, usize, usize)>),
    /// Add the pages of files the user picks before this page.
    InsertFile(usize),
    /// Compare with another version the user picks.
    Compare,
    /// Save these pages as pictures (at this resolution).
    ExportImages(Vec<usize>, u32),
    /// Save the file under a new name.
    SaveAs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Single,
    Continuous,
    Two,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    None,
    Thumbnails,
    Bookmarks,
    Comments,
}

/// What dragging on a page does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Hand,
    /// Marks the text dragged over (highlight, underline, strike out).
    Markup(Kind),
    /// Draws an annotation of this kind: a note, a text box, ink, a line,
    /// an arrow, a rectangle, an oval or a stamp.
    Draw(Kind),
    /// Fills in anywhere: text, a check mark, today's date.
    FillText,
    FillCheck,
    FillDate,
    /// Places the visible signature being made.
    PlaceSign,
}

/// A drawn page on the GPU.
struct Tex {
    rev: u64,
    width: u32,
    /// Drawn for the dark workspace.
    dark: bool,
    handle: TextureHandle,
}

/// Selected text: page and char indices (inclusive, in drawing order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub page: usize,
    pub a: usize,
    pub b: usize,
}

/// Whether the user was told pages are drawn without pdfium (once).
pub fn warned_simple() -> bool {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    WARNED.swap(true, std::sync::atomic::Ordering::Relaxed)
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub struct PdfView {
    pub doc: PdfDoc,
    pub password: Option<String>,
    /// This view's number (for the drawing thread).
    id: u64,
    pub notices: Vec<Notice>,
    pub read_only: bool,
    pub requests: Vec<Request>,
    pub zoom: f32,
    pub fit: Option<Fit>,
    pub layout: Layout,
    /// Turned by the view, in quarter turns clockwise.
    pub turn: u8,
    /// Seeing the pages in their real colors for now, with the dark
    /// workspace on (not kept).
    pub see_light: bool,
    /// The page at the top of the view.
    pub cur: usize,
    /// Scroll to this page (and point from its top) on the next frame.
    goto: Option<(usize, f32)>,
    /// The page at the top of the view, how far down it (points) and the
    /// zoom then.
    view_top: Option<(usize, f32, f32)>,
    pub pane: Pane,
    pub tool: Tool,
    /// Page sizes as shown, for the revision they are of.
    sizes: (u64, Vec<[f32; 2]>),
    /// The file's bytes as they are now.
    bytes: (u64, Arc<[u8]>),
    tex: HashMap<usize, Tex>,
    /// Thumbnails, drawn smaller.
    thumbs: HashMap<usize, Tex>,
    pending: HashSet<(usize, u32, u64, bool)>,
    tx: Sender<engine::Done>,
    rx: Receiver<engine::Done>,
    text: text::TextLayer,
    pub sel: Option<Selection>,
    /// The selection asked to go to a notebook as a fragment.
    pub fragment_wanted: bool,
    pub find: text::Find,
    show: Option<show::Show>,
    print: Option<print_box::PrintBox>,
    bookmarks: (u64, Vec<apex_io::pdf::edit::outline::Bookmark>),
    /// The view's size last frame, for fitting.
    view_size: egui::Vec2,
    annot_cache: HashMap<usize, (u64, Arc<Vec<apex_io::pdf::edit::annots::Annot>>)>,
    link_cache: HashMap<usize, (u64, Arc<Vec<([f32; 4], apex_io::pdf::edit::outline::Target)>>)>,
    /// Where a text selection started.
    drag_from: Option<usize>,
    /// PDF files have no macros; the macro dialogs see this empty set.
    pub macros: apex_doc::Macros,
    /// Colours and sizes for new annotations.
    pub pen: annotate::Pen,
    /// Who writes comments and replies.
    pub author: String,
    /// An annotation being drawn.
    draft: Option<annotate::Draft>,
    /// The chosen annotation, and one being dragged.
    pub picked: Option<u32>,
    moving: Option<annotate::Move>,
    /// A comment's text being written.
    edit: Option<annotate::EditBox>,
    comments: comments::CommentsUi,
    /// The form's fields for the revision they are of.
    fields: (u64, Arc<Vec<apex_io::pdf::edit::forms::Field>>),
    /// A text field being typed in: its name and text.
    field_edit: Option<form_ui::FieldEdit>,
    sign_box: Option<sign_box::SignBox>,
    /// The visible signature to place, and whether to sign it digitally.
    sig_look: Option<(apex_io::pdf::edit::forms::SigLook, bool)>,
    /// Pages chosen in the thumbnails.
    pub page_sel: std::collections::BTreeSet<usize>,
    /// The thumbnail being dragged to another place.
    thumb_drag: Option<usize>,
    split_box: Option<organize_ui::SplitBox>,
    stamp_box: Option<stamp_ui::StampBox>,
    circuit_box: Option<circuit_ui::CircuitBox>,
    /// The last stamp used, for the next time.
    last_stamp: Option<apex_io::pdf::edit::stamp::Stamp>,
    export_box: Option<tools_ui::ExportBox>,
    props_box: Option<tools_ui::PropsBox>,
    /// Changes found by comparing with another version.
    pub compare: Option<compare_ui::Comparison>,
}

/// Empty document properties for the status bar (a PDF's are in its own
/// properties dialog).
pub fn no_meta() -> &'static apex_doc::Meta {
    static EMPTY: std::sync::OnceLock<apex_doc::Meta> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Default::default)
}

/// RGBA pixels made small enough that neither side passes `side` (as
/// they are when they fit).
fn fit_side(size: [usize; 2], rgba: Vec<u8>, side: usize) -> ([usize; 2], Vec<u8>) {
    let [w, h] = size;
    if w <= side && h <= side {
        return (size, rgba);
    }
    let k = side as f64 / w.max(h) as f64;
    let (nw, nh) = (((w as f64 * k) as usize).clamp(1, side), ((h as f64 * k) as usize).clamp(1, side));
    let mut out = Vec::with_capacity(nw * nh * 4);
    for y in 0..nh {
        let sy = y * h / nh;
        for x in 0..nw {
            let i = (sy * w + x * w / nw) * 4;
            out.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    ([nw, nh], out)
}

impl PdfView {
    /// The tool armed (what dragging on a page does), for the QA.
    #[cfg(test)]
    pub fn audit_tool(&self) -> String {
        format!("{:?}", self.tool)
    }

    /// Whether the text of every page has been read, for the QA.
    #[cfg(test)]
    pub fn audit_text_ready(&self) -> bool {
        (0..self.page_count()).all(|i| self.text.page(i).is_some())
    }

    /// The tool, layout and fit, for the button audit: with a short file
    /// switching them may not change what shows.
    #[cfg(test)]
    pub fn audit_modes(&self) -> String {
        format!("{:?} {:?} {:?} {} {:?} {} {}", self.tool, self.layout, self.fit, self.zoom, self.pane, self.turn, self.doc.revision)
    }

    /// Opens `data` (encrypted files with `password`).
    pub fn open(data: Vec<u8>, password: Option<String>) -> Result<PdfView, apex_io::Error> {
        let doc = PdfDoc::open(data, password.as_deref())?;
        Ok(PdfView::new(doc, password))
    }

    pub fn new(doc: PdfDoc, password: Option<String>) -> PdfView {
        let (tx, rx) = channel();
        let bytes: Arc<[u8]> = Arc::from(doc.base());
        let rev = doc.revision;
        let mut v = PdfView {
            doc,
            password,
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            notices: Vec::new(),
            read_only: false,
            requests: Vec::new(),
            zoom: 1.0,
            fit: Some(Fit::PageWidth),
            layout: Layout::Continuous,
            turn: 0,
            see_light: false,
            cur: 0,
            goto: None,
            view_top: None,
            pane: Pane::Thumbnails,
            tool: Tool::Select,
            sizes: (0, Vec::new()),
            bytes: (rev, bytes),
            tex: HashMap::new(),
            thumbs: HashMap::new(),
            pending: HashSet::new(),
            tx,
            rx,
            text: text::TextLayer::default(),
            sel: None,
            fragment_wanted: false,
            find: text::Find::default(),
            show: None,
            print: None,
            bookmarks: (0, Vec::new()),
            view_size: egui::Vec2::ZERO,
            annot_cache: HashMap::new(),
            link_cache: HashMap::new(),
            drag_from: None,
            macros: Default::default(),
            pen: annotate::Pen::default(),
            author: annotate::user_name(),
            draft: None,
            picked: None,
            moving: None,
            edit: None,
            comments: Default::default(),
            fields: (u64::MAX, Arc::new(Vec::new())),
            field_edit: None,
            sign_box: None,
            sig_look: None,
            page_sel: Default::default(),
            thumb_drag: None,
            split_box: None,
            stamp_box: None,
            circuit_box: None,
            last_stamp: None,
            export_box: None,
            props_box: None,
            compare: None,
        };
        v.apply_view_prefs();
        v
    }

    /// The layout and pane the file asks to open with.
    fn apply_view_prefs(&mut self) {
        let Some((_, cat)) = self.doc.catalog() else { return };
        match self.doc.key(&cat, "PageLayout").name() {
            Some("SinglePage") => self.layout = Layout::Single,
            Some("TwoColumnLeft" | "TwoColumnRight" | "TwoPageLeft" | "TwoPageRight") => self.layout = Layout::Two,
            _ => {}
        }
        match self.doc.key(&cat, "PageMode").name() {
            Some("UseOutlines") => self.pane = Pane::Bookmarks,
            Some("UseNone") => self.pane = Pane::None,
            Some("UseThumbs") => self.pane = Pane::Thumbnails,
            _ => {}
        }
    }

    pub fn is_modified(&self) -> bool {
        self.doc.modified()
    }

    pub fn page_count(&self) -> usize {
        self.sizes.1.len()
    }

    /// The bytes of the file as it is now.
    pub fn bytes(&mut self) -> Arc<[u8]> {
        if self.bytes.0 != self.doc.revision {
            self.bytes = (self.doc.revision, Arc::from(self.doc.to_bytes()));
        }
        self.bytes.1.clone()
    }

    /// Page sizes as shown (before the view's turn).
    fn refresh(&mut self) {
        if self.sizes.0 != self.doc.revision {
            let sizes = self.doc.pages().into_iter().map(|p| {
                let g = self.doc.geom(p);
                [g.width as f32, g.height as f32]
            }).collect();
            self.sizes = (self.doc.revision, sizes);
            self.cur = self.cur.min(self.sizes.1.len().saturating_sub(1));
        }
    }

    /// A page's size on screen at zoom 1, turned.
    fn shown_size(&self, i: usize) -> [f32; 2] {
        let s = self.sizes.1.get(i).copied().unwrap_or([612.0, 792.0]);
        if self.turn % 2 == 1 { [s[1], s[0]] } else { s }
    }

    pub fn undo(&mut self) {
        if self.doc.undo() {
            self.after_change();
        }
    }

    pub fn redo(&mut self) {
        if self.doc.redo() {
            self.after_change();
        }
    }

    /// After a change to the file: the selection may no longer be there.
    pub fn after_change(&mut self) {
        self.sel = None;
        self.field_edit = None;
        if self.picked.is_some_and(|id| self.doc.page_of_annot(id).is_none()) {
            self.picked = None;
        }
        self.refresh();
    }

    /// Takes the file as it is now on disk (signed elsewhere): what is
    /// shown is drawn again.
    pub fn reload(&mut self, data: Vec<u8>) -> Result<(), apex_io::Error> {
        let doc = PdfDoc::open(data, self.password.as_deref())?;
        self.bytes = (doc.revision, Arc::from(doc.base()));
        self.doc = doc;
        self.id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.tex.clear();
        self.thumbs.clear();
        self.pending.clear();
        self.text = text::TextLayer::default();
        self.annot_cache.clear();
        self.link_cache.clear();
        self.sizes = (u64::MAX, Vec::new());
        self.fields = (u64::MAX, Arc::new(Vec::new()));
        self.bookmarks = (u64::MAX, Vec::new());
        self.picked = None;
        self.after_change();
        Ok(())
    }

    pub fn go_to(&mut self, page: usize, top: f32) {
        let n = self.page_count();
        if n == 0 {
            return;
        }
        self.cur = page.min(n - 1);
        self.goto = Some((self.cur, top));
    }

    /// Whether the pages are drawn dark now: the dark workspace is on and
    /// the real colors are not asked for.
    pub(crate) fn dark(&self) -> bool {
        crate::theme::dark_canvas() && !self.see_light
    }

    /// The color of the paper under a page (what shows while it is drawn).
    pub(crate) fn paper(&self) -> egui::Color32 {
        if self.dark() { crate::theme::dark_color(egui::Color32::WHITE) } else { egui::Color32::WHITE }
    }

    /// Takes the pages the drawing thread finished.
    fn receive(&mut self, ctx: &egui::Context) {
        // The drawing thread skips pages of outdated revisions without an
        // answer: not waited for (more than a few would stop the asking).
        let rev = self.doc.revision;
        self.pending.retain(|p| p.2 == rev);
        // Tests wait for the pages asked for, so that what they look at is
        // drawn.
        if cfg!(test) {
            let until = web_time::Instant::now() + web_time::Duration::from_secs(5);
            while !self.pending.is_empty() && web_time::Instant::now() < until {
                if let Ok(d) = self.rx.recv_timeout(web_time::Duration::from_millis(50)) {
                    self.take(ctx, d);
                }
            }
        }
        while let Ok(d) = self.rx.try_recv() {
            self.take(ctx, d);
        }
    }

    /// Takes one page the drawing thread finished.
    fn take(&mut self, ctx: &egui::Context, d: engine::Done) {
        let thumb = d.width <= pages::THUMB_PX;
        self.pending.remove(&(d.page, d.width, d.rev, d.dark));
        if d.width == 0 || d.rgba.is_empty() {
            return;
        }
        // Larger than a texture can be here (zoomed far in): made to fit.
        let side = ctx.input(|i| i.max_texture_side).max(64);
        let (size, rgba) = fit_side(d.size, d.rgba, side);
        let img = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
        let handle = ctx.load_texture(format!("pdf-{}-{}-{}", self.id, d.page, thumb), img, egui::TextureOptions::LINEAR);
        let t = Tex { rev: d.rev, width: d.width, dark: d.dark, handle };
        if thumb {
            self.thumbs.insert(d.page, t);
        } else {
            self.tex.insert(d.page, t);
        }
    }

    /// Asks the drawing thread for page `i` at `width` pixels (unless it
    /// is on its way or there).
    fn want(&mut self, ctx: &egui::Context, i: usize, width: u32) -> bool {
        let rev = self.doc.revision;
        let have = if width <= pages::THUMB_PX { self.thumbs.get(&i) } else { self.tex.get(&i) };
        let dark = self.dark();
        if have.is_some_and(|t| t.rev == rev && t.width == width && t.dark == dark) || self.pending.contains(&(i, width, rev, dark)) {
            return true;
        }
        // Not too many at once: the rest next frame.
        if self.pending.len() > 6 {
            return true;
        }
        let bytes = self.bytes();
        let job = engine::Job { doc: self.id, rev, bytes, password: self.password.clone(), page: i, width, annots: true, dark, reply: self.tx.clone(), ctx: Some(ctx.clone()) };
        if engine::request(job) {
            self.pending.insert((i, width, rev, dark));
            // Tests take it on the next frame (see `receive`).
            if cfg!(test) {
                ctx.request_repaint();
            }
            true
        } else {
            false
        }
    }

    // --- the view ----------------------------------------------------------------------

    pub fn showing(&self) -> bool {
        self.show.is_some()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let _ = shell;
        self.refresh();
        self.receive(ui.ctx());
        self.print_ui(ui.ctx());
        self.edit_ui(ui.ctx());
        self.sign_box_ui(ui.ctx());
        self.split_ui(ui.ctx());
        self.stamp_ui(ui.ctx());
        self.circuit_ui(ui.ctx());
        self.export_ui(ui.ctx());
        self.props_ui(ui.ctx());
        let bytes = self.bytes();
        self.text.poll(&bytes, self.password.as_deref(), self.doc.revision, !engine::available());
        if self.show.is_some() {
            self.show_ui(ui);
            return;
        }
        self.keys(ui);
        if self.page_count() == 0 {
            ui.centered_and_justified(|ui| ui.label(tr("pdf.no_pages")));
            return;
        }
        if self.pane != Pane::None {
            egui::Panel::left(egui::Id::new(("pdf_pane", self.id))).resizable(true).default_size(190.0).min_size(120.0).max_size((ui.available_width() * 0.45).max(200.0)).show(ui, |ui| self.side_ui(ui));
        }
        if self.compare.is_some() {
            egui::Panel::right(egui::Id::new(("pdf_compare", self.id))).resizable(true).default_size(260.0).min_size(160.0).show(ui, |ui| self.compare_pane(ui));
        }
        if self.find.open {
            egui::Panel::top(egui::Id::new(("pdf_find", self.id))).show(ui, |ui| self.find_bar(ui));
        }
        self.pages_ui(ui);
    }

    fn keys(&mut self, ui: &mut egui::Ui) {
        // The tab's own dialogs, and any other on top, have the keys.
        if ui.ctx().egui_wants_keyboard_input() || ui.ctx().memory(|m| m.top_modal_layer().is_some()) || self.edit.is_some() || self.sign_box.is_some() || self.print.is_some() || self.split_box.is_some() || self.stamp_box.is_some() || self.circuit_box.is_some() || self.export_box.is_some() || self.props_box.is_some() {
            return;
        }
        let n = self.page_count();
        if self.annot_keys(ui) {
            return;
        }
        let (next, prev, home, end, copy, find, all, esc, f5) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::PageDown) && self.layout == Layout::Single || i.consume_key(Modifiers::CTRL, Key::PageDown),
                i.consume_key(Modifiers::NONE, Key::PageUp) && self.layout == Layout::Single || i.consume_key(Modifiers::CTRL, Key::PageUp),
                i.consume_key(Modifiers::CTRL, Key::Home),
                i.consume_key(Modifiers::CTRL, Key::End),
                i.consume_key(Modifiers::COMMAND, Key::C) || i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                i.consume_key(Modifiers::COMMAND, Key::F),
                i.consume_key(Modifiers::COMMAND, Key::A),
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::NONE, Key::F5),
            )
        });
        let step = if self.layout == Layout::Two { 2 } else { 1 };
        if next {
            self.go_to((self.cur + step).min(n.saturating_sub(1)), 0.0);
        }
        if prev {
            self.go_to(self.cur.saturating_sub(step), 0.0);
        }
        if home {
            self.go_to(0, 0.0);
        }
        if end {
            self.go_to(n.saturating_sub(1), 0.0);
        }
        if copy && let Some(t) = self.selected_text() {
            ui.ctx().copy_text(t);
        }
        if find {
            self.find.open = true;
            self.find.focus = true;
        }
        if all {
            self.select_page(self.cur);
        }
        if esc {
            self.sel = None;
        }
        if f5 {
            self.start_show(ui.ctx(), self.cur);
        }
    }

    /// The status bar's left side.
    pub fn status(&mut self, ui: &mut egui::Ui) {
        let n = self.page_count();
        ui.label(trf("pdf.page_of", &[("n", &(self.cur + 1).min(n)), ("total", &n)]));
        if !engine::available() {
            ui.separator();
            ui.label(egui::RichText::new(tr("pdf.simple_drawing")).weak()).on_hover_text(tr("pdf.simple_drawing_tip"));
        }
        if self.doc.is_signed() {
            ui.separator();
            ui.label(tr("pdf.signed_file"));
        }
    }

    /// Zoom at the status bar's right.
    pub fn zoom_status(&mut self, ui: &mut egui::Ui) {
        let before = self.zoom;
        if let Some(f) = crate::common::zoom_control(ui, &mut self.zoom, true) {
            self.fit = Some(f);
        } else if self.zoom != before {
            self.fit = None;
        }
    }
}
