//! Word processor: paginated WYSIWYG editor with a per-paragraph layout cache.

mod comments_ui;
mod mcp_tools;
mod dialogs;
mod find_ui;
mod images_ui;
mod annotate_ui;
mod shapes_ui;
mod ink_ui;
mod nav;
mod notes;
mod print_ui;
mod hf;
mod char_fx;
mod font_ui;
mod review;
mod mailings;
mod revisions_ui;
pub(crate) mod math_ui;
mod spell_ui;
mod styles_ui;
mod refs;
mod biblio_ui;
pub use biblio_ui::{source_entry, source_fields};
mod toa_ui;
mod line_nums_ui;
mod toolstrip_ui;
mod decor;
mod layout_ui;
mod lists_ui;
mod sections;
pub(crate) mod table_ui;
mod table_tools_ui;
mod protect_ui;
mod book_ui;
mod book_panel;
mod book_scope;
mod book_goto;
mod book_goals;
mod book_cards;
mod book_table;
mod book_write;
mod book_review;
mod book_notebook;
mod book_versions;
mod book_publish;
mod tables;
mod toolbar;
mod views;
mod code_gutter;
mod code_syntax;
mod md_code;
mod md_syntax;
mod outline_ui;
mod controls_ui;
pub mod tasks_ui;
mod forms_ui;
mod thesaurus_ui;
mod asian_layout_ui;
mod lang_ui;
mod pic_fx_ui;
mod art_ui;
pub(crate) mod text_drag;
mod paste_opts_ui;
mod minibar_ui;
mod inspect_ui;
mod reveal_ui;
mod access_ui;
mod mail_db;
mod mail_files;
mod assemble_ui;
mod blocks_ui;
mod contextual;
pub use contextual::SAMPLE_PNG;
mod shapes_arrange;
mod speak_ui;
mod asian_draw;
mod asian_ui;
mod tategaki;
mod rulers_ui;
mod select_pane_ui;
mod brush;
mod cells_in;

pub use views::ViewMode;

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use apex_doc::{Align, Attrs, Document, LINE_BREAK, LineRule, ParaKind, ParaStyle, Paragraph, Pos, Selection};
use eframe::egui::text::{CCursor, LayoutJob, TextFormat};
use eframe::egui::{self, Color32, Event, EventFilter, Galley, Id, Key, Modifiers, Pos2, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};

use crate::app::Notice;
use crate::common::{self, Shell};
use crate::fonts::{self, Face};
use crate::i18n::{tr, trf};
use brush::Brush;
use crate::icons::Icon;
use crate::theme::{self, Paper};
use crate::widgets;
use dialogs::{DocDialog, FindBar, RulerDrag};

pub use comments_ui::{set_user_name, take_user_changed, user_name, user_setting};
pub use revisions_ui::DocToOpen;

/// An equation drawn at `size` pixels (presentations draw them too): its
/// width, ascent, descent and shapes around its baseline's left end.
pub fn render_math(ctx: &egui::Context, eq: &apex_doc::math::Equation, size: f32, display: bool, color: Color32) -> (f32, f32, f32, Vec<egui::Shape>) {
    math_ui::render(ctx, eq, size, display, color)
}

/// A link's extent, target and screen tip.
type LinkHit = (Selection, Arc<str>, Option<Arc<str>>);

/// Screen pixels per typographic point at 100% zoom.
const PT: f32 = 1.333;
const PAGE_GAP: f32 = 24.0;
/// Indentation per level, and the room of a list's markers (as in the
/// files: Word's and LibreOffice's half inch).
const LIST_INDENT_PT: f32 = 36.0;
/// Indent step for normal paragraphs (half an inch, like other editors).
const INDENT_STEP_PT: f32 = 36.0;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct ParaLayout {
    hash: u64,
    galley: Arc<Galley>,
    /// Left edge of the text relative to the content area.
    indent: f32,
    /// Width available to the text.
    width: f32,
    /// Offset added to `indent` when drawing (centered/right rows are laid
    /// out around x = 0).
    align_off: f32,
    space_before: f32,
    space_after: f32,
    /// Pictures, placed relative to the galley.
    images: Vec<images_ui::Placed>,
    /// Shapes anchored here.
    shapes: Vec<shapes_ui::PlacedShape>,
    tab_fills: Vec<decor::TabFill>,
    maths: Vec<math_ui::Placed>,
    /// Phonetic guides, emphasis marks and combined lines over the text.
    overlays: Vec<asian_draw::Overlay>,
    /// Text of a cell turned a quarter: 1 = reads downwards, -1 = upwards.
    rotate: i8,
    /// Artistic text effects painted behind and under the galley.
    fx: crate::text_fx::Layers,
    /// Room its pictures with text beside them take on the left and right,
    /// and how far they reach below the text.
    side: [f32; 3],
    /// `space_after` as laid out, before neighbours took some of it.
    natural_after: f32,
    /// Share of each line's height that is spacing past a single line: it
    /// may reach into the bottom margin, as in Word.
    lead: f32,
}

/// Whether paragraph `i` ends a cell right after a table in that cell.
fn after_nested_table(paras: &[Paragraph], i: usize) -> bool {
    fn path(c: &apex_doc::InCell) -> Vec<(u32, u32, u16)> {
        let mut out = vec![(c.table, c.row, c.col)];
        let mut n = &c.nested;
        while let Some(c) = n {
            out.push((c.table, c.row, c.col));
            n = &c.nested;
        }
        out
    }
    let Some(me) = paras[i].cell.as_ref().map(path) else { return false };
    let prev = i.checked_sub(1).and_then(|k| paras[k].cell.as_ref()).map(path);
    let next = paras.get(i + 1).and_then(|p| p.cell.as_ref()).map(path);
    prev.is_some_and(|p| p.len() > me.len() && p.starts_with(&me)) && !next.is_some_and(|n| n.starts_with(&me))
}

/// A slice of a paragraph's rows placed on one page.
#[derive(Clone, Copy, Debug)]
struct Segment {
    para: usize,
    page: usize,
    /// Top of the slice within the page content area.
    y: f32,
    /// Galley y range shown in this slice.
    gy0: f32,
    gy1: f32,
    /// In a table cell (clipped to the cell's width).
    cell: bool,
    /// A paragraph of a cell covered by a merged cell: not drawn.
    hidden: bool,
    /// Left edge and width of its column (from the page's text area).
    x: f32,
    w: f32,
}

#[derive(Default)]
struct Layout {
    key: (u64, u32, u32, u64, u64),
    paras: Vec<ParaLayout>,
    segs: Vec<Segment>,
    /// Index of the first segment of each paragraph.
    first_seg: Vec<usize>,
    pages: usize,
    /// Size of the largest page.
    page: Vec2,
    /// Text area of the first page.
    content: Vec2,
    geo: Vec<sections::PageGeo>,
    /// Lines between columns.
    rules: Vec<sections::Rule>,
    /// Segment indices in page order (the parts of a table row split across
    /// pages put a paragraph's segments out of it), and where each page's
    /// start in it.
    by_page: Vec<u32>,
    page_start: Vec<usize>,
    /// Repeated table header rows: drawn, but not where the caret goes.
    ghosts: Vec<Segment>,
    frags: Vec<tables::RowFrag>,
    tables: Vec<tables::TableBox>,
    /// The innermost table of each paragraph (`u32::MAX` for none).
    owner: Vec<u32>,
    /// The number each page shows (its section's numbering).
    labels: Vec<String>,
    /// Pages to a row, width of a row and space between pages.
    cols: usize,
    row_w: f32,
    gap: f32,
    /// Field values and notes.
    refs: refs::RefLayout,
    /// Line numbers in the margins, by page.
    line_nums: Vec<decor::LineNum>,
}

impl Layout {
    /// Segments on pages `a..=b`.
    fn page_segs(&self, a: usize, b: usize) -> impl Iterator<Item = &Segment> {
        let s = self.page_start.get(a).copied().unwrap_or(self.by_page.len());
        let e = self.page_start.get(b + 1).copied().unwrap_or(self.by_page.len()).max(s);
        self.by_page[s..e].iter().map(|&k| &self.segs[k as usize])
    }

    fn geo_of(&self, page: usize) -> sections::PageGeo {
        match self.geo.get(page).or(self.geo.last()) {
            Some(g) => *g,
            None => sections::PageGeo { top: PAGE_GAP, size: self.page, content: self.content, ..Default::default() },
        }
    }

    fn total_height(&self) -> f32 {
        self.geo.last().map_or(PAGE_GAP, |g| g.top + g.size.y) + PAGE_GAP
    }

    /// The page at canvas y `y` (the gap around a page counts as its).
    fn page_at_y(&self, y: f32) -> usize {
        self.geo.partition_point(|g| g.top + g.size.y + self.gap / 2.0 <= y).min(self.pages.saturating_sub(1))
    }

    fn segs_of(&self, para: usize) -> &[Segment] {
        // A layout behind the document (the Code view edits it) has none.
        let Some(&a) = self.first_seg.get(para) else { return &[] };
        let b = self.first_seg.get(para + 1).copied().unwrap_or(self.segs.len());
        self.segs.get(a..b.max(a)).unwrap_or(&[])
    }
}

pub struct DocView {
    pub doc: Document,
    /// The embedded font faces this document holds (released on drop).
    fonts_held: Vec<std::path::PathBuf>,
    symbols: crate::symbols::SymbolDialog,
    page_setup: Option<print_ui::SetupDialog>,
    font_dialog: Option<font_ui::FontDialog>,
    /// File name for header and footer fields.
    file_name: String,
    pub zoom: f32,
    /// Zoom to fit requested from the status bar (needs the view size).
    pub fit: Option<common::Fit>,
    pub notices: Vec<Notice>,
    uid: u64,
    sel: Selection,
    /// What the screen reader was last told: caret, paragraph length, text.
    a11y: std::cell::RefCell<(Pos, usize, String)>,
    /// Attributes for the next typed text (toggled with an empty selection).
    pending: Option<Attrs>,
    /// Sticky x position for vertical movement (content coordinates).
    desired_x: Option<f32>,
    layout: Layout,
    /// Page origin (screen) of the canvas from the last frame.
    canvas_origin: Pos2,
    page_x: f32,
    dragging: bool,
    /// Selected text being dragged (see `text_drag`).
    text_drag: Option<text_drag::TextDrag>,
    scroll_to_caret: bool,
    preedit: String,
    clip: Option<(String, Vec<Paragraph>, Arc<apex_doc::Origin>)>,
    paste_expected: bool,
    /// The requested paste drops formatting.
    paste_plain: bool,
    find: Option<FindBar>,
    focus: bool,
    counts: Option<(u64, usize, usize)>,
    viewport_h: f32,
    viewport_w: f32,
    brush: Option<Brush>,
    /// Which shared pick-up `brush` is (see `painter::sync`).
    brush_serial: u64,
    last_color: Option<u32>,
    last_highlight: Option<u32>,
    last_shading: Option<u32>,
    /// Formatting marks (¶, spaces, tabs, line breaks).
    show_marks: bool,
    /// Inside another view (a notebook's page): one sheet down to the
    /// bottom, without page breaks.
    embedded: bool,
    /// Embedded: a link to a page followed, `[[` typed (where it starts),
    /// the caret on screen and the link under the pointer, for the view
    /// around the editor to act on.
    pub note_follow: Option<Arc<str>>,
    /// The selection asked to go to a notebook as a fragment.
    pub fragment_wanted: bool,
    pub link_prompt: Option<Pos>,
    pub caret_screen: Option<Rect>,
    pub hover_link: Option<Arc<str>>,
    /// Laying out for printing (hidden text never shows).
    printing: std::cell::Cell<bool>,
    /// The document grid of the section being laid out.
    grid: std::cell::Cell<apex_doc::DocGrid>,
    /// Laying out vertical text (a cell turned downwards, tbRl).
    vertical: std::cell::Cell<bool>,
    /// Laying out a vertical page (pictures lie on their side).
    tategaki: std::cell::Cell<bool>,
    /// Room on the left and right taken by the pictures of an earlier
    /// paragraph that the one laid out sits beside.
    beside: std::cell::Cell<(f32, f32)>,
    /// A header or footer being painted: the top of its page and of the
    /// page's text, the bottom of the text and of the page (shapes placed
    /// from them).
    story_frame: std::cell::Cell<Option<[f32; 4]>>,
    show_ruler: bool,
    ruler_drag: Option<RulerDrag>,
    /// A margin or table row edge dragged on the vertical ruler.
    vruler_drag: Option<rulers_ui::VDrag>,
    /// The Selection Pane shows, and its state.
    pane_open: bool,
    pane: crate::selection_pane::PaneState,
    dialog: Option<DocDialog>,
    refocus: bool,
    /// The link under the pointer when the context menu was opened.
    menu_link: Option<LinkHit>,
    pics: images_ui::PicState,
    /// The equation editor.
    math_edit: Option<math_ui::MathEdit>,
    shapes: shapes_ui::ShapesState,
    /// The Draw tab's tools, and the ink object drawn last.
    ink: crate::ink::InkState,
    ink_last: Option<Pos>,
    col_drag: Option<table_ui::ColDrag>,
    table_dialog: Option<table_ui::TableDialog>,
    spell: spell_ui::SpellState,
    word_count: Option<review::Counts>,
    mail: mailings::MailState,
    /// Quick Parts: the organizer or Save as Building Block.
    blocks: Option<blocks_ui::BlocksDlg>,
    styles: styles_ui::StylesState,
    /// Review > Check Accessibility.
    access: access_ui::AccessState,
    /// Notes, fields, tables of contents and the navigation pane.
    refs: refs::RefState,
    comments: comments_ui::CommentState,
    markup: revisions_ui::Markup,
    /// A document made by Compare, for the app to open.
    compared: Option<DocToOpen>,
    layout_dialog: Option<layout_ui::LayoutDialog>,
    table_style_dlg: Option<Box<table_ui::TableStyleDlg>>,
    line_num_dlg: Option<line_nums_ui::LineNumDlg>,
    control_dlg: Option<controls_ui::ControlDlg>,
    /// Legacy form fields: their dialog and shading.
    forms: forms_ui::FormState,
    /// Revision whose bound controls and custom XML were last synced.
    bind_rev: u64,
    control_popup: Option<controls_ui::ControlPopup>,
    thes: Option<thesaurus_ui::ThesPane>,
    /// The Clipboard pane is open.
    clip_pane: bool,
    /// The last paste, for the Paste Options button.
    paste_opts: Option<paste_opts_ui::Pasted>,
    minibar: Option<minibar_ui::MiniBar>,
    lang_dlg: Option<lang_ui::LangDlg>,
    ruby_dlg: Option<asian_layout_ui::RubyDlg>,
    asian_dlg: Option<asian_layout_ui::LayoutDlg>,
    lists: lists_ui::ListState,
    tab_ui: layout_ui::TabUi,
    /// Laid out headers and footers.
    hf_cache: std::cell::RefCell<hf::HfCache>,
    /// The header or footer being edited (the body is set aside).
    hf_edit: Option<Box<hf::HfEdit>>,
    /// Page number and count for the fields of a header or footer view.
    story_page: Option<hf::StoryPage>,
    protect: protect_ui::ProtectState,
    book: book_ui::BookState,
    views: views::ViewState,
    /// Opened read-only (someone else has the file): no edit is kept.
    pub read_only: bool,
    /// Read Aloud.
    speak: speak_ui::SpeakState,
    /// The code view (the text of text files, else the Markdown).
    md: md_code::MdState,
}

impl Drop for DocView {
    fn drop(&mut self) {
        crate::fonts::embedded::release(&self.fonts_held);
    }
}

impl DocView {
    /// The tool armed (a shape to draw, the pen, the format painter),
    /// for the QA.
    #[cfg(test)]
    pub fn audit_tool(&self) -> String {
        format!("{:?} {:?} {:?}", self.shapes.armed().map(|t| t.kind), self.ink.mode, self.pending)
    }

    /// What decides what the next action does without showing on screen
    /// (the format for typing, the tool in use), for the button audit: a
    /// click that changes only this has done something.
    #[cfg(test)]
    pub fn audit_modes(&self) -> String {
        let shape = self.shapes.armed().map(|t| t.kind);
        // The text too: an edit may change nothing drawn in the tree.
        let text = (self.doc.revision, self.doc.undo_len(), self.sel);
        format!("{:?} {:?} {:?} {:?} {shape:?} {} {} {:?} {:?} {} {} {} {text:?}", self.pending, self.last_color, self.last_highlight, self.ink.mode, self.forms.no_shading, self.zoom, self.fit, self.views.mode, self.md_source(), self.md.lines, self.md.wrap)
    }

    /// Where the text position `p` shows on the screen (last frame).
    #[cfg(test)]
    pub(crate) fn screen_pos(&self, p: Pos) -> Pos2 {
        self.caret_rect(p).center() + self.canvas_origin.to_vec2()
    }

    #[cfg(test)]
    pub(crate) fn set_selection(&mut self, sel: Selection) {
        self.sel = sel;
    }

    #[cfg(test)]
    pub(crate) fn brush_on(&self) -> Option<bool> {
        self.brush.as_ref().map(|b| b.sticky)
    }

    pub fn new(doc: Document) -> Self {
        let fonts_held = crate::fonts::embedded::register(&doc.fonts, &doc.fonts_used());
        let book = book_ui::BookState::new(doc.book.is_some());
        DocView {
            doc,
            fonts_held,
            zoom: 1.0,
            fit: None,
            notices: Vec::new(),
            uid: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            sel: Selection::default(),
            a11y: std::cell::RefCell::new((Pos::new(usize::MAX, 0), 0, String::new())),
            pending: None,
            desired_x: None,
            layout: Layout::default(),
            canvas_origin: Pos2::ZERO,
            page_x: 0.0,
            dragging: false,
            text_drag: None,
            scroll_to_caret: false,
            preedit: String::new(),
            clip: None,
            paste_expected: false,
            paste_plain: false,
            find: None,
            focus: true,
            counts: None,
            viewport_h: 600.0,
            viewport_w: 800.0,
            brush: None,
            brush_serial: 0,
            last_color: Some(0xC00000),
            last_highlight: Some(0xFFFF00),
            last_shading: Some(0xD9D9D9),
            show_marks: false,
            embedded: false,
            note_follow: None,
            fragment_wanted: false,
            link_prompt: None,
            caret_screen: None,
            hover_link: None,
            printing: Default::default(),
            grid: Default::default(),
            vertical: Default::default(),
            tategaki: Default::default(),
            beside: Default::default(),
            story_frame: Default::default(),
            show_ruler: true,
            ruler_drag: None,
            vruler_drag: None,
            pane_open: false,
            pane: Default::default(),
            dialog: None,
            symbols: Default::default(),
            page_setup: None,
            font_dialog: None,
            file_name: String::new(),
            refocus: false,
            menu_link: None,
            pics: Default::default(),
            math_edit: None,
            shapes: Default::default(),
            ink: Default::default(),
            ink_last: None,
            col_drag: None,
            table_dialog: None,
            protect: Default::default(),
            book,
            spell: Default::default(),
            word_count: None,
            mail: Default::default(),
            blocks: None,
            styles: Default::default(),
            access: Default::default(),
            refs: Default::default(),
            comments: Default::default(),
            markup: Default::default(),
            compared: None,
            layout_dialog: None,
            table_style_dlg: None,
            line_num_dlg: None,
            control_dlg: None,
            forms: Default::default(),
            bind_rev: 0,
            control_popup: None,
            thes: None,
            clip_pane: false,
            paste_opts: None,
            minibar: None,
            lang_dlg: None,
            ruby_dlg: None,
            asian_dlg: None,
            lists: Default::default(),
            tab_ui: Default::default(),
            hf_cache: Default::default(),
            hf_edit: None,
            story_page: None,
            views: Default::default(),
            read_only: false,
            speak: Default::default(),
            md: Default::default(),
        }
    }

    /// An editor inside another view (a notebook's page): the text
    /// without the page around it.
    pub fn embedded(doc: Document) -> Self {
        let mut v = DocView::new(doc);
        v.views.mode = ViewMode::Draft;
        v.embedded = true;
        v.show_ruler = false;
        v
    }

    /// Selects the first place `word` is, without caring for case or
    /// accents.
    pub fn find_first(&mut self, word: &str) {
        let w = apex_doc::notebook::fold(word);
        if w.is_empty() {
            return;
        }
        for (i, p) in self.doc.paragraphs.iter().enumerate() {
            let folded: Vec<(usize, char)> = p.text.char_indices().map(|(at, c)| (at, apex_doc::notebook::fold(&c.to_string()).chars().next().unwrap_or(c))).collect();
            let wc: Vec<char> = w.chars().collect();
            if let Some(k) = folded.windows(wc.len()).position(|win| win.iter().map(|x| x.1).eq(wc.iter().copied())) {
                let end = folded.get(k + wc.len()).map_or(p.text.len(), |x| x.0);
                let s = Selection { anchor: Pos::new(i, folded[k].0), cursor: Pos::new(i, end) };
                self.set_sel(s);
                return;
            }
        }
    }

    pub(crate) fn canvas_id(&self) -> Id {
        Id::new(("doc", self.uid, self.views.drawing))
    }

    fn set_sel(&mut self, s: Selection) {
        self.sel = Selection { anchor: self.doc.clamp(s.anchor), cursor: self.doc.clamp(s.cursor) };
        self.pending = None;
        self.scroll_to_caret = true;
    }

    /// Shows or hides the rulers (a phone has no room for them).
    pub fn set_ruler(&mut self, on: bool) {
        self.show_ruler = on;
    }

    /// The selection a macro starts with (Word's `Selection`), with code
    /// typed in the code view read into the document first.
    pub fn macro_context(&mut self) -> Selection {
        self.md_apply();
        self.sel
    }

    /// Takes the document a macro changed (`None`: it changed nothing) as
    /// one undo step, and selects what it left selected.
    pub fn macro_done(&mut self, doc: Option<Document>, sel: Selection) {
        if let Some(d) = doc {
            let sel_after = Selection { anchor: d.clamp(sel.anchor), cursor: d.clamp(sel.cursor) };
            self.doc.take_changed(d, self.sel, sel_after, apex_doc::EditKind::Replace);
            // A macro edits only while unprotected (it may protect the
            // document after): the protection starts from what it left.
            if !self.read_only && self.views.mode != ViewMode::Read {
                self.protect.guard = self.doc.undo_len();
                self.forms_unprotected();
            }
        }
        self.preedit.clear();
        self.set_sel(sel);
    }

    #[cfg(test)]
    pub fn put_caret(&mut self, p: Pos) {
        self.set_sel(Selection { anchor: p, cursor: p });
    }

    /// Selects the whole document (for tests driving the window).
    #[cfg(test)]
    pub(crate) fn select_all(&mut self) {
        self.set_sel(Selection { anchor: Pos::new(0, 0), cursor: self.doc.end() });
    }

    /// Selects paragraphs `a` to `b`, whole (for the QA tools).
    #[cfg(test)]
    pub(crate) fn select_paras(&mut self, a: usize, b: usize) {
        let end = self.doc.paragraphs.get(b).map_or(0, |p| p.text.len());
        self.set_sel(Selection { anchor: Pos::new(a, 0), cursor: Pos::new(b, end) });
    }

    /// The code view's text while it shows (for the QA tools).
    #[cfg(test)]
    pub(crate) fn code_text(&self) -> Option<&str> {
        self.md.code.as_ref().map(|c| c.text.as_str())
    }

    fn caret(&mut self, p: Pos, extend: bool) {
        let anchor = if extend { self.sel.anchor } else { p };
        self.set_sel(Selection { anchor, cursor: p });
    }

    /// What a screen reader says as the caret moves: the paragraph it
    /// moves to, the letter it steps over or the word it lands in; the
    /// text selected. Typing changes nothing (the reader echoes keys).
    fn a11y_text(&self) -> String {
        let mut last = self.a11y.borrow_mut();
        let cur = self.sel.cursor;
        let Some(p) = self.doc.paragraphs.get(cur.para) else { return last.2.clone() };
        if cur == last.0 && !self.sel.is_empty() == last.2.ends_with(tr("a11y.selected")) {
            return last.2.clone();
        }
        let typed = cur.para == last.0.para && p.text.len() != last.1;
        let text = if !self.sel.is_empty() {
            let t: String = self.doc.text_range(self.sel.anchor, self.sel.cursor).chars().take(400).collect();
            format!("{t} {}", tr("a11y.selected"))
        } else if typed {
            last.2.clone()
        } else if cur.para != last.0.para {
            let t: String = p.text.chars().take(1000).collect();
            if t.trim().is_empty() { tr("a11y.blank_line").to_string() } else { t }
        } else {
            let at = cur.offset.min(p.text.len());
            let (a, b) = (last.0.offset.min(at), last.0.offset.max(at));
            let stepped = p.text.get(a..b).filter(|s| s.chars().count() == 1);
            match stepped {
                Some(ch) => ch.to_string(),
                None => word_at(&p.text, at),
            }
        };
        // The same text twice (two letters alike) must be said again.
        let text = if text == last.2.trim_end_matches('\u{200B}') && !typed { format!("{text}\u{200B}") } else { text };
        *last = (cur, p.text.len(), text.clone());
        text
    }

    fn cur_para(&self) -> &Paragraph {
        &self.doc.paragraphs[self.sel.cursor.para.min(self.doc.paragraphs.len() - 1)]
    }

    // --- layout -----------------------------------------------------------------

    /// Line spacing of a paragraph with the multiple rule: its own, else
    /// its style's (odt keeps it there), else the document defaults'
    /// (Word's 1.08 or 1.15), else single.
    fn multiple(&self, st: &ParaStyle) -> f32 {
        if st.line_spacing > 0.0 {
            return st.line_spacing;
        }
        self.doc.styles.default_line(st)
    }

    fn text_format(&self, st: &ParaStyle, a: &Attrs, z: f32) -> TextFormat {
        let kind = st.kind;
        let cf = self.doc.char_format(st, a);
        // Right-to-left runs take the complex script font and size.
        let rtl = cf.asian.rtl && !a.code;
        let hp = if rtl { cf.asian.cs_size.unwrap_or(cf.size) } else { cf.size } as f32;
        let base = hp / 2.0 * PT * z;
        let mut size = base;
        if a.superscript || a.subscript {
            size *= 0.7;
        }
        let bold = cf.bold;
        let mut font_name = None;
        let (font_id, synth_italic) = if a.code || kind == ParaKind::Code {
            (fonts::font(Face::Mono, bold, size), cf.italic)
        } else {
            let theme = &self.doc.styles.theme;
            let cs = cf.asian.cs_font.as_ref().filter(|_| rtl).and_then(|f| match theme.cs_font(f, cf.asian.cs_lang.as_deref()) {
                Some(t) => Some(t.clone()),
                None => (!f.starts_with('+')).then(|| f.clone()),
            });
            font_name = cs.or_else(|| cf.font.clone()).or_else(|| self.doc.default_font_name());
            match font_name.as_deref() {
                // CJK fonts (MS Mincho...) as the text font: a stand-in.
                Some(f) => match crate::cjk_fonts::stand_in(f) {
                    Some(s) => fonts::font_for(Some(&s), bold, cf.italic, size),
                    None => fonts::font_for(Some(f), bold, cf.italic, size),
                },
                None if matches!(kind, ParaKind::Title | ParaKind::Heading(_)) => (fonts::font(Face::Sans, bold, size), cf.italic),
                None => (fonts::font(Face::Serif, bold, size), cf.italic),
            }
        };
        let mut color = match (cf.color, &a.link) {
            (Some(c), _) => theme::rgb(c),
            (None, Some(_)) => Color32::from_rgb(0x1A, 0x5F, 0xB4),
            _ if kind == ParaKind::Quote => Color32::from_gray(85),
            _ if matches!(kind, ParaKind::Heading(_) | ParaKind::Title) => Color32::from_rgb(0x1F, 0x2A, 0x44),
            _ => Paper::TEXT,
        };
        if a.code && a.color.is_none() {
            color = Color32::from_rgb(0x9A, 0x23, 0x23);
        }
        // A content control's placeholder text.
        if a.control.as_ref().is_some_and(|c| c.placeholder) {
            color = Color32::from_gray(128);
        }
        let stroke = Stroke::new((size / 16.0).max(1.0), color);
        let single = base * fonts::line_factor_for(font_name.as_deref(), &font_id.family);
        let line_height = match st.line_rule {
            // Whole twips, as Word and LibreOffice lay lines out.
            LineRule::Multiple => {
                let twip = PT * z / 20.0;
                (single * self.multiple(st) / twip + 0.01).floor() * twip
            }
            LineRule::Exact => st.line_spacing.max(1.0) * PT * z,
            LineRule::AtLeast => single.max(st.line_spacing * PT * z),
        };
        let mut fmt = TextFormat {
            font_id,
            color,
            background: a.highlight.map(theme::rgb).or_else(|| self.form_shade(a)).unwrap_or(Color32::TRANSPARENT),
            italics: synth_italic,
            underline: if (cf.underline && cf.fx.underline == apex_doc::UnderlineStyle::Single) || a.link.is_some() {
                match cf.fx.underline_color {
                    Some(c) => Stroke::new(stroke.width, theme::rgb(c)),
                    None => stroke,
                }
            } else {
                Stroke::NONE
            },
            strikethrough: if a.strike && !cf.fx.double_strike { stroke } else { Stroke::NONE },
            line_height: Some(line_height),
            valign: if a.superscript {
                egui::Align::TOP
            } else if a.subscript {
                egui::Align::BOTTOM
            } else {
                egui::Align::Center
            },
            ..Default::default()
        };
        let fx = &cf.fx;
        if fx.spacing != 0 {
            fmt.extra_letter_spacing = fx.spacing as f32 / 20.0 * PT * z;
        }
        if fx.hidden && (!self.show_marks || self.printing.get()) {
            // Keeps its characters (so offsets stay) at no width.
            fmt.font_id.size = 0.05;
            fmt.extra_letter_spacing = 0.0;
            fmt.color = Color32::TRANSPARENT;
            fmt.background = Color32::TRANSPARENT;
            fmt.underline = Stroke::NONE;
            fmt.strikethrough = Stroke::NONE;
            return fmt;
        }
        self.markup_format(&mut fmt, a);
        fmt
    }

    /// The Asian font of text `t` with attributes `a` laid out in `fmt`
    /// (None when it has no Asian characters), and whether shared
    /// characters take it (`w:hint`).
    fn asian_font(&self, st: &ParaStyle, a: &Attrs, t: &str, fmt: &TextFormat) -> (Option<egui::FontId>, bool, Option<f32>) {
        let hint = a.asian.as_ref().is_some_and(|x| x.hint) || self.doc.styles.asian.text.as_ref().is_some_and(|x| x.hint);
        if !t.chars().any(|c| apex_doc::uses_asian_font(c, hint)) || fmt.font_id.size < 0.1 {
            return (None, hint, None);
        }
        let cf = self.doc.char_format(st, a);
        let x = &cf.asian;
        let cjk_lang = a.lang.as_deref().filter(|l| ["ja", "zh", "ko"].iter().any(|p| l.starts_with(p)));
        let lang = x.lang.as_deref().or(cjk_lang).map(str::to_string).unwrap_or_else(|| apex_doc::guess_lang(t, crate::cjk_fonts::default_lang()));
        let scale = x.size.map_or(1.0, |s| s as f32 / cf.size.max(1) as f32);
        // Theme fonts (+mn-ea) by the language's script. With no Asian
        // font, a CJK text font (宋体 picked in the font box) draws them.
        let text_font = cf.font.as_deref().filter(|f| crate::cjk_fonts::is_cjk_font(f));
        let theme = &self.doc.styles.theme;
        let name = x.font.as_deref().or(text_font).map(|n| theme.ea_font(n, Some(&lang)).map_or_else(|| theme.font_name(n), |f| &**f));
        let font = crate::cjk_fonts::font_for(name, Some(&lang), cf.bold, cf.italic, fmt.font_id.size * scale).map(|f| f.0);
        // The stand-in's lines are as high as the font's would be.
        let line = name.filter(|n| font.as_ref().is_some_and(|f| !fonts::is_font(&f.family, n))).and_then(fonts::known_line);
        (font, x.hint || hint, line)
    }

    /// The painted effects of text with attributes `a` (see `char_fx`).
    fn text_deco(&self, st: &ParaStyle, a: &Attrs, z: f32) -> Option<char_fx::Deco> {
        let cf = self.doc.char_format(st, a);
        if cf.fx.is_default() || (cf.fx.hidden && (!self.show_marks || self.printing.get())) {
            return None;
        }
        let fmt = self.text_format(st, a, z);
        let width = (fmt.font_id.size / 16.0).max(1.0);
        char_fx::Deco::of(&cf.fx, cf.underline, fmt.color, width, PT * z, self.show_marks && !self.printing.get())
    }

    fn para_hash(p: &Paragraph) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        p.text.hash(&mut h);
        for r in &p.runs {
            r.len.hash(&mut h);
            r.attrs.hash(&mut h);
        }
        let s = &p.style;
        s.kind.hash(&mut h);
        s.custom.hash(&mut h);
        s.align.hash(&mut h);
        s.level.hash(&mut h);
        s.unnumbered.hash(&mut h);
        s.line_rule.hash(&mut h);
        s.toc.hash(&mut h);
        for v in [s.line_spacing, s.space_before.unwrap_or(-1.0), s.space_after.unwrap_or(-1.0), s.indent_left, s.indent_right, s.first_line] {
            v.to_bits().hash(&mut h);
        }
        for t in s.tabs.as_slice() {
            (t.pos.to_bits(), t.align, t.leader).hash(&mut h);
        }
        s.drop_cap.map(|d| (d.lines, d.margin, d.chars, d.distance.to_bits())).hash(&mut h);
        s.asian.hash(&mut h);
        s.rtl.hash(&mut h);
        h.finish()
    }

    /// Left text edge of a paragraph relative to the content area, in points
    /// (list levels, quotes and code add to the paragraph's own indent).
    fn indent_pt(&self, st: &ParaStyle) -> f32 {
        let mut indent = st.level as f32 * LIST_INDENT_PT;
        if st.kind.is_list() {
            indent += LIST_INDENT_PT;
        }
        match st.kind {
            ParaKind::Quote => indent += 20.0,
            ParaKind::Code => indent += 6.0,
            // Room for the number of numbered headings.
            ParaKind::Heading(h) if self.doc.styles.heading_list > 0 && !st.unnumbered => indent += 18.0 + 12.0 * (h as f32 - 1.0),
            _ => {}
        }
        indent + st.indent_left
    }

    /// Lays out a paragraph `content_w` wide; `look` is the bold and color
    /// that a table style gives the text of its cell.
    fn layout_para(&self, ctx: &egui::Context, p: &Paragraph, z: f32, content_w: f32, look: Option<(bool, Option<u32>)>) -> ParaLayout {
        fonts::note_text(&p.text);
        let st = p.style;
        let styled = |a: &Attrs| match look {
            Some((bold, color)) => Attrs { bold: a.bold || bold, color: a.color.or(color), ..a.clone() },
            None => a.clone(),
        };
        let pt = PT * z;
        let indent = self.indent_pt(&st) * pt;
        let right = (st.indent_right + if st.kind == ParaKind::Code { 6.0 } else { 0.0 }) * pt;
        let mut width = (content_w - indent - right).max(20.0);
        // Pictures with text beside them narrow the paragraph.
        let turned = self.tategaki.get();
        let (side_l, side_r) = images_ui::side_widths(p, z, width, turned);
        let (shape_l, shape_r) = shapes_ui::side_widths(p, z, width);
        let own = (side_l.max(shape_l), side_r.max(shape_r));
        let (side_l, side_r) = (own.0.max(self.beside.get().0), own.1.max(self.beside.get().1));
        width = (width - side_l - side_r).max(20.0);
        let mut indent = indent + side_l;
        // A positive first-line indent; hanging indents move list markers
        // into the margin, and the first line of other paragraphs out of
        // the rest's indent.
        // (A list item's first line under the list's room is where its
        // marker goes, as in Word: its text stays at the indent.)
        let lead = if st.kind.is_list() && st.first_line < LIST_INDENT_PT / 2.0 { 0.0 } else { (st.first_line * pt).max(0.0) };
        let hang = if st.first_line < 0.0 && !st.kind.is_list() && !p.text.is_empty() { -st.first_line * pt } else { 0.0 };
        indent -= hang;
        width += hang;
        // Exact line heights: page breaks mustn't depend on the zoom.
        let mut job = LayoutJob { round_output_to_gui: false, ..Default::default() };
        let display = apex_doc::math::is_display(p);
        let mut maths = Vec::new();
        let mut decos: Vec<char_fx::Deco> = Vec::new();
        let mut fx_runs: Vec<crate::text_fx::Run> = Vec::new();
        let mut spans: Vec<asian_ui::Span> = Vec::new();
        let mut marks: Vec<asian_draw::Mark> = Vec::new();
        if p.text.is_empty() {
            let a = p.runs.first().map(|r| r.attrs.clone()).unwrap_or_default();
            job.append("", lead, self.text_format(&st, &styled(&a), z));
        } else {
            let mut first = true;
            let mut nbsp = None;
            let has_images = p.runs.iter().any(|r| r.attrs.image.is_some());
            for (s, e, a) in p.spans() {
                if e > s {
                    let t = &p.text[s..e];
                    let lead = if first { lead } else { 0.0 };
                    if let Some(f) = a.field.as_ref().filter(|_| t.len() == apex_doc::OBJECT.len_utf8() && t.starts_with(apex_doc::OBJECT)) {
                        let nbsp = *nbsp.get_or_insert_with(|| images_ui::nbsp_ratio(ctx));
                        let mut fmt = self.text_format(&st, &Attrs { field: None, ..styled(a) }, z);
                        if has_images && fmt.valign == egui::Align::Center {
                            fmt.valign = egui::Align::BOTTOM;
                        }
                        job.append("\u{A0}", lead, refs::placeholder(ctx, &fmt, &f.cached, nbsp));
                        first = false;
                        continue;
                    } else if let Some(eq) = &a.math {
                        let nbsp = *nbsp.get_or_insert_with(|| images_ui::nbsp_ratio(ctx));
                        let mut base = self.text_format(&st, &Attrs { math: None, ..styled(a) }, z);
                        if has_images {
                            base.valign = egui::Align::BOTTOM;
                        }
                        let fmt = math_ui::object_format(ctx, &base, eq, display, nbsp, t, s, &mut maths);
                        job.append(&t.replace(apex_doc::OBJECT, "\u{A0}"), lead, fmt);
                        first = false;
                        continue;
                    }
                    if let Some(img) = &a.image {
                        let nbsp = *nbsp.get_or_insert_with(|| images_ui::nbsp_ratio(ctx));
                        let fmt = images_ui::object_format(&self.text_format(&st, &Attrs { image: None, ..styled(a) }, z), img, z, width, nbsp, turned);
                        job.append(&t.replace(apex_doc::OBJECT, "\u{A0}"), lead, fmt);
                        first = false;
                        continue;
                    }
                    if let Some(shape) = &a.shape {
                        let shape = &shapes_ui::in_flow(p, s, shape).unwrap_or_else(|| Arc::new(apex_doc::DocShape { wrap: apex_doc::ShapeWrap::InFront, ..(**shape).clone() }));
                        let nbsp = *nbsp.get_or_insert_with(|| images_ui::nbsp_ratio(ctx));
                        // A floating shape's own size doesn't make its line
                        // taller (Word sizes it by the paragraph mark).
                        let mut sa = apex_doc::without_shape(&styled(a));
                        if !matches!(shape.wrap, apex_doc::ShapeWrap::Inline | apex_doc::ShapeWrap::TopBottom) {
                            sa.size = None;
                        }
                        let fmt = shapes_ui::object_format(&self.text_format(&st, &sa, z), shape, z, width, nbsp);
                        job.append(&t.replace(apex_doc::OBJECT, "\u{A0}"), lead, fmt);
                        first = false;
                        continue;
                    }
                    let sa = styled(a);
                    let mut fmt = self.text_format(&st, &sa, z);
                    // Text sits on the bottom of lines with pictures.
                    if has_images && fmt.valign == egui::Align::Center {
                        fmt.valign = egui::Align::BOTTOM;
                    }
                    let fx = if a.fx.is_some() || !self.doc.styles.is_empty() { self.doc.char_format(&st, &sa).fx } else { Default::default() };
                    if let Some(mut d) = self.text_deco(&st, &sa, z) {
                        (d.from, d.to) = (char_count(&p.text, s), char_count(&p.text, e));
                        decos.push(d);
                    }
                    if let Some(tf) = &a.text_fx {
                        fx_runs.push(crate::text_fx::Run { from: char_count(&p.text, s), to: char_count(&p.text, e), fx: tf.clone() });
                    }
                    // Soft line breaks lay out as newlines (one char each, so
                    // char offsets stay the same).
                    let t = if t.contains(LINE_BREAK) { std::borrow::Cow::Owned(t.replace(LINE_BREAK, "\n")) } else { std::borrow::Cow::Borrowed(t) };
                    // Check boxes are drawn in an em space (fonts may lack ☒).
                    let t = if a.control.as_ref().is_some_and(|c| matches!(c.kind, apex_doc::ControlKind::CheckBox { .. })) { std::borrow::Cow::Owned(t.replace([apex_doc::CHECKED, apex_doc::UNCHECKED], "\u{2003}")) } else { t };
                    let first_sec = job.sections.len();
                    let asian = self.asian_font(&st, &sa, &t, &fmt);
                    let size = fmt.font_id.size;
                    let from = job.text.len();
                    let mark = sa.asian.as_ref().filter(|x| asian_draw::wanted(x)).map(|x| asian_draw::Mark {
                        bytes: 0..0,
                        asian: x.clone(),
                        size,
                        family: asian.0.as_ref().map_or_else(|| fmt.font_id.family.clone(), |f| f.family.clone()),
                        color: fmt.color,
                    });
                    if fx.is_default() {
                        if t.contains(apex_doc::SOFT_HYPHEN) {
                            decor::append_soft_hyphens(&mut job, &t, lead, fmt);
                        } else {
                            job.append(&t, lead, fmt);
                        }
                    } else {
                        char_fx::append(ctx, &mut job, &t.replace(apex_doc::SOFT_HYPHEN, decor::SOFT_BREAK), lead, fmt, &fx);
                    }
                    if let Some(mut m) = mark {
                        m.bytes = from..job.text.len();
                        marks.push(m);
                    }
                    let (font, hint, line) = asian;
                    spans.push(asian_ui::Span { sections: first_sec..job.sections.len(), font, size, hint, line });
                    first = false;
                }
            }
        }
        job.wrap.max_width = width;
        job.wrap.indent_rest = hang;
        let rules = asian_ui::Rules {
            para: self.doc.asian_para(&st),
            grid: self.grid.get(),
            base: self.doc.normal_size_pt(),
            pt,
            multiple: (st.line_rule == LineRule::Multiple).then(|| self.multiple(&st)),
            at_least: (st.line_rule == LineRule::AtLeast).then(|| st.line_spacing * pt),
        };
        asian_ui::apply(&mut job, &spans, &rules);
        if self.vertical.get() {
            asian_ui::vertical(&mut job);
        }
        let prep = asian_draw::prepare(ctx, &mut job, &marks, pt);
        let pitch = (rules.para.snap_to_grid() && rules.grid.kind.lines()).then(|| rules.grid.pitch() * pt);
        if st.toc.is_some_and(|t| t.opts.right_align && t.opts.page_numbers) {
            refs::toc_tab(ctx, &mut job, width);
        }
        // Right-to-left paragraphs start at the right: left and right
        // swap, and justified text ends its last line at the right.
        job.rtl = st.rtl;
        let align = match (display, st.rtl, st.align) {
            // Display equations are centered.
            (true, ..) => Align::Center,
            (false, true, Align::Left) => Align::Right,
            (false, true, Align::Right) => Align::Left,
            (.., a) => a,
        };
        let align_off = match align {
            Align::Center => {
                job.halign = egui::Align::Center;
                width / 2.0
            }
            Align::Right => {
                job.halign = egui::Align::RIGHT;
                width
            }
            Align::Justify => {
                job.justify = true;
                // Word 2013 shrinks spaces by up to a fifth.
                if self.doc.shrink_justified {
                    job.wrap.shrink_spaces = 0.2;
                }
                if st.rtl {
                    job.halign = egui::Align::RIGHT;
                    width
                } else {
                    0.0
                }
            }
            Align::Left => 0.0,
        };
        let (galley, tab_fills) = if job.justify && p.text.contains(apex_doc::SOFT_HYPHEN) {
            decor::layout_justified_hyphens(ctx, job, &st, indent, pt, self.doc.default_tab)
        } else {
            decor::layout_text(ctx, job, &st, indent, pt, self.doc.default_tab)
        };
        let galley = char_fx::apply(galley, &decos);
        let bg = self.doc.deco.color.map_or(Color32::WHITE, crate::theme::rgb);
        let (galley, fx) = crate::text_fx::apply(galley, &fx_runs, PT * z, bg);
        // Vertical lines: characters in the middle of their line.
        let galley = if turned { tategaki::center_rows(galley) } else { galley };
        let (galley, overlays) = asian_draw::place(ctx, galley, &prep, pitch);
        let (before, after) = st.spacing();
        let images = images_ui::place(p, &galley, z, width, align_off, side_l, turned);
        let maths = math_ui::place(p, &galley, maths);
        // Pictures beside the text may be taller than it.
        let shapes = shapes_ui::place(p, &galley, z, width, align_off, side_l);
        let side_h = images.iter().filter(|i| i.img.wrap == apex_doc::Wrap::Square).map(|i| i.rect.max.y).fold(0.0, f32::max).max(shapes_ui::side_height(&shapes));
        let below = (side_h - galley.size().y).max(0.0);
        let after = after * pt + below;
        let m = if st.line_rule == LineRule::Multiple { self.multiple(&st) } else { 1.0 };
        let lead = if m > 1.0 { 1.0 - 1.0 / m } else { 0.0 };
        ParaLayout { hash: Self::para_hash(p), galley, indent, width, align_off, space_before: before * pt, space_after: after, images, shapes, tab_fills, maths, overlays, rotate: 0, fx, side: [own.0, own.1, below], natural_after: after, lead }
    }

    fn update_layout(&mut self, ui: &egui::Ui) {
        self.relayout(ui.ctx());
    }

    /// Lays the document out again if it changed.
    fn relayout(&mut self, ctx: &egui::Context) {
        let z = self.zoom;
        let page = self.doc.page;
        let font_key = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.doc.default_font_token().hash(&mut h);
            self.doc.styles.hash_into(&mut h);
            self.markup.hash(&mut h);
            (self.show_marks, self.printing.get(), self.forms.no_shading).hash(&mut h);
            (self.views.mode, self.page_cols()).hash(&mut h);
            h.finish()
        };
        let key = (self.doc.revision, z.to_bits(), page.content_width().to_bits(), font_key, fonts::generation());
        if self.layout.key == key && self.layout.paras.len() == self.doc.paragraphs.len() {
            return;
        }
        let old = std::mem::take(&mut self.layout);
        self.layout = self.build_layout(ctx, z, key, old);
        if self.fit_edited_footer() {
            let old = std::mem::take(&mut self.layout);
            self.layout = self.build_layout(ctx, z, key, old);
        }
        self.arrange_pages();
    }

    /// Lays out and paginates the document at zoom `z`, reusing the
    /// paragraphs of `old` that didn't change; `vals` are the values of
    /// fields and note marks to show.
    fn build_pass(&self, ctx: &egui::Context, z: f32, key: (u64, u32, u32, u64, u64), mut old_layout: Layout, vals: Option<&std::collections::HashMap<Pos, String>>) -> Layout {
        // A book seen in part lays out the rest empty.
        let hidden = self.scope_hidden();
        let scoped_doc = hidden.as_ref().map(|h| self.scope_layout_doc(h));
        let doc = scoped_doc.as_ref().unwrap_or(&self.doc);
        let page = self.doc.page;
        let pt = PT * z;
        // Vertical pages are laid out turned (the draft and outline views
        // show their text horizontally, like Word's).
        let turn = !matches!(self.views.mode, ViewMode::Draft | ViewMode::Outline);
        let flow = if turn && page.vertical() { page.flow_setup() } else { page };
        let content = vec2(flow.content_width() * pt, flow.content_height() * pt);
        let mut sects = sections::Sect::all(doc, pt, turn);
        self.grid.set(Default::default());
        self.fit_headers(ctx, (key.3, key.4), z, &mut sects);
        let sect_of = sections::sect_of(&sects, doc.paragraphs.len());
        // Reuse layouts of unchanged paragraphs (matched by content hash; the
        // cache is flushed when zoom, width, fonts or the default font change).
        let geometry_changed = (old_layout.key.1, old_layout.key.2, old_layout.key.3, old_layout.key.4) != (key.1, key.2, key.3, key.4);
        let mut old_notes = std::mem::take(&mut old_layout.refs.notes);
        if geometry_changed {
            old_notes = Default::default();
        }
        // Equal hashes mean equal layouts, so one entry serves repeated
        // paragraphs (like the empty cells of a table).
        let mut old: std::collections::HashMap<u64, ParaLayout> =
            if geometry_changed { Default::default() } else { old_layout.paras.into_iter().map(|p| (p.hash, p)).collect() };
        let doc_paras = &doc.paragraphs;
        let col_w = |i: usize| sects.get(sect_of.get(i).copied().unwrap_or(0) as usize).map_or(content.x, |s| s.width());
        let (tables, owner) = tables::TableBox::all(doc.all_tables(), doc_paras, &col_w, pt);
        let mut paras = Vec::with_capacity(doc_paras.len());
        // The column width each paragraph outside tables was laid out in.
        let mut laid_w: Vec<Option<f32>> = vec![None; doc_paras.len()];
        // What a paragraph left out takes: nothing.
        let ghost = hidden.as_ref().map(|_| {
            let mut l = self.layout_para(ctx, &Paragraph::new(""), z, content.x, None);
            let job = egui::text::LayoutJob::single_section(String::new(), TextFormat { font_id: egui::FontId::proportional(0.1), ..Default::default() });
            l.galley = ctx.fonts_mut(|f| f.layout_job(job));
            (l.space_before, l.space_after, l.hash) = (0.0, 0.0, 0x5C0E_D0C5_0000_0001);
            l
        });
        // Paragraphs beside the pictures of an earlier one: (that one, the
        // pictures' room on each side, how far they reach below the last
        // paragraph beside them, that paragraph).
        let mut flow: Option<(usize, f32, f32, f32, usize)> = None;
        // The pictures' room left below the last paragraph beside them
        // goes after it.
        let end_flow = |paras: &mut Vec<ParaLayout>, f: Option<(usize, f32, f32, f32, usize)>| {
            if let Some((owner, _, _, rem, last)) = f
                && last != owner
            {
                paras[last].space_after += rem.max(0.0);
            }
        };
        for (i, p) in doc_paras.iter().enumerate() {
            if let (Some(g), Some(h)) = (&ghost, &hidden)
                && h[i]
            {
                paras.push(g.clone());
                continue;
            }
            // Fields and note marks lay out with the text they show.
            let shown;
            let p = match vals {
                Some(v) if apex_doc::para_has_objects(p) => {
                    shown = refs::shown_para(p, i, v);
                    &shown
                }
                _ => p,
            };
            let mut cell = None;
            if p.cell.is_some() {
                if let Some(tb) = tables.get(owner[i] as usize) {
                    let (x, w) = tb.text_box(doc_paras, i, pt);
                    let m = &tb.map;
                    let (r, c) = m.pos_of(doc_paras, i);
                    let (ar, ac) = m.anchor_of(r, c);
                    let look = m.props(doc_paras).map(|tp| {
                        let l = tp.cell_look(&self.doc.styles, &m.cell_props(doc_paras, ar, ac).cloned().unwrap_or_default(), ar, ac, m.rows, m.cols);
                        (l.bold, l.color)
                    });
                    let rotate = match m.cell_props(doc_paras, ar, ac).map(|c| c.text_dir) {
                        Some(apex_doc::TextDir::Down) => 1i8,
                        Some(apex_doc::TextDir::Up) => -1,
                        _ => 0,
                    };
                    let row_h = m.props(doc_paras).and_then(|tp| tp.row_heights.get(r).copied()).filter(|h| h.is_finite() && *h > 0.0).map(|h| h * pt);
                    cell = Some((x, w, look, rotate, row_h));
                }
            }
            // The section's document grid and direction.
            let sect = sects.get(sect_of.get(i).copied().unwrap_or(0) as usize);
            let grid = sect.map_or(page.grid, |s| s.setup.grid);
            self.grid.set(grid);
            let (tate, upright) = sect.map_or((false, false), |s| (s.vertical, s.upright));
            self.tategaki.set(tate);
            self.vertical.set(upright);
            let h = match cell {
                Some((x, w, look, rotate, _)) => {
                    let mut hs = std::collections::hash_map::DefaultHasher::new();
                    Self::para_hash(p).hash(&mut hs);
                    (x.to_bits(), w.to_bits(), look, rotate).hash(&mut hs);
                    hs.finish()
                }
                None => Self::para_hash(p) ^ col_w(i).to_bits() as u64,
            };
            let h = if grid.kind == apex_doc::GridKind::None { h } else { h ^ asian_ui::grid_hash(&grid).rotate_left(7) };
            // An empty cell end after a table in the cell takes no room, as
            // in Word.
            let after_table = cell.is_some() && p.text.is_empty() && after_nested_table(doc_paras, i);
            let h = if after_table { h ^ 0xA7E2_7AB1_0000_0001 } else { h };
            let h = if tate { h ^ if upright { 0x7A7E_6A4B_0000_0001 } else { 0x7A7E_6A4B_0000_0002 } } else { h };
            // Paragraphs after a picture with text beside it sit beside it
            // too while they fit, as in Word; the first that doesn't goes
            // below it.
            let pending = flow.take();
            if let Some((owner, sl, sr, rem, last)) = pending
                && cell.is_none()
                && !tate
                && !p.style.page_break_before
                && sect_of.get(i) == sect_of.get(owner)
                && !p.runs.iter().any(|r| r.attrs.image.is_some())
            {
                self.beside.set((sl, sr));
                let mut l = self.layout_para(ctx, p, z, col_w(i), None);
                self.beside.set((0.0, 0.0));
                let row = l.galley.rows.first().map_or(0.0, |r| r.row.size.y);
                if l.space_before + l.galley.size().y <= rem + row * 0.5 {
                    if last == owner {
                        let o = &mut paras[owner];
                        o.space_after = (o.space_after - o.side[2]).max(0.0);
                    }
                    let rem = rem - (l.space_before + l.galley.size().y + l.space_after - l.side[2]);
                    l.hash = h ^ 0xB351_DE00_0000_0001;
                    // Its own pictures beside the text: the flow goes on
                    // beside both, as far as the lower reaches.
                    if l.side[2] > 0.0 {
                        let below = l.side[2].max(rem);
                        l.space_after += below - l.side[2];
                        l.side[2] = below;
                        flow = Some((i, sl.max(l.side[0]), sr.max(l.side[1]), below - (l.space_after - below).max(0.0), i));
                        paras.push(l);
                        continue;
                    }
                    paras.push(l);
                    if rem > 0.5 {
                        flow = Some((owner, sl, sr, rem, i));
                    }
                    continue;
                }
            }
            end_flow(&mut paras, pending);
            if cell.is_none() {
                laid_w[i] = Some(col_w(i));
            }
            if let Some(l) = old.get(&h) {
                let mut l = l.clone();
                l.space_after = l.natural_after;
                if l.side[2] > 0.0 && cell.is_none() {
                    flow = Some((i, l.side[0], l.side[1], l.side[2] - (l.space_after - l.side[2]).max(0.0), i));
                }
                paras.push(l);
                continue;
            }
            let l = match cell {
                Some((x, w, look, rotate, row_h)) => {
                    let look = look.filter(|l| l.0 || l.1.is_some());
                    let mut l = if rotate == 0 {
                        self.layout_para(ctx, p, z, w, look)
                    } else {
                        // Turned text runs down the cell: it wraps at the
                        // row's height (or not at all).
                        // Downwards is vertical text: Asian characters upright.
                        self.vertical.set(rotate > 0);
                        let wide = self.layout_para(ctx, p, z, row_h.unwrap_or(4000.0), look);
                        let mut l = if row_h.is_some() { wide } else { self.layout_para(ctx, p, z, wide.galley.size().x.ceil() + 1.0, look) };
                        self.vertical.set(upright);
                        l.rotate = rotate;
                        l
                    };
                    // In cells only explicit paragraph spacing applies, as
                    // table styles usually remove it.
                    l.indent += x;
                    // Vertical text starts at the cell's right edge.
                    if rotate > 0 {
                        l.indent += (w - l.galley.size().y).max(0.0);
                    }
                    l.space_before = p.style.space_before.unwrap_or(0.0) * pt;
                    l.space_after = p.style.space_after.unwrap_or(0.0) * pt;
                    if after_table {
                        let job = egui::text::LayoutJob::single_section(String::new(), TextFormat { font_id: egui::FontId::proportional(0.1), ..Default::default() });
                        l.galley = ctx.fonts_mut(|f| f.layout_job(job));
                        (l.space_before, l.space_after) = (0.0, 0.0);
                    }
                    l.hash = h;
                    l
                }
                None => {
                    let mut l = self.layout_para(ctx, p, z, col_w(i), None);
                    l.hash = h;
                    l
                }
            };
            let mut l = l;
            l.natural_after = l.space_after;
            old.insert(h, l.clone());
            if l.side[2] > 0.0 && cell.is_none() {
                flow = Some((i, l.side[0], l.side[1], l.side[2] - (l.space_after - l.side[2]).max(0.0), i));
            }
            paras.push(l);
        }
        end_flow(&mut paras, flow);
        self.tategaki.set(false);
        self.vertical.set(false);
        let no_vals = std::collections::HashMap::new();
        let mut fp = self.foot_pager(ctx, z, content, vals.unwrap_or(&no_vals), &mut old_notes, {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (key.1, key.2, key.3, key.4).hash(&mut h);
            h.finish()
        });
        contextual_spacing(doc, &mut paras, pt);
        collapse_spacing(doc, &mut paras, pt);
        empty_beside_pictures(doc, &mut paras);
        let mut paged = sections::paginate(doc_paras, &paras, &tables, &sects, pt, &mut fp);
        // Columns of unequal widths: a paragraph is laid out again in the
        // width of the column holding most of it, and the pages made again.
        if sects.iter().any(|s| s.cols.windows(2).any(|c| (c[0].1 - c[1].1).abs() > 0.5)) {
            for _ in 0..3 {
                let mut again = false;
                let mut most: Vec<Option<(f32, f32)>> = vec![None; paras.len()];
                for sg in paged.segs.iter().filter(|sg| !sg.cell) {
                    let h = sg.gy1 - sg.gy0;
                    if most[sg.para].is_none_or(|(_, mh)| h >= mh) {
                        most[sg.para] = Some((sg.w, h));
                    }
                }
                for i in 0..paras.len() {
                    let (Some(w), Some((cw, _))) = (laid_w[i], most[i]) else { continue };
                    if (w - cw).abs() < 0.5 || paras[i].side[2] > 0.0 {
                        continue;
                    }
                    let shown;
                    let p = match vals {
                        Some(v) if apex_doc::para_has_objects(&doc_paras[i]) => {
                            shown = refs::shown_para(&doc_paras[i], i, v);
                            &shown
                        }
                        _ => &doc_paras[i],
                    };
                    let h = paras[i].hash ^ (cw.to_bits() as u64).rotate_left(23);
                    let mut l = match old.get(&h) {
                        Some(l) => l.clone(),
                        None => {
                            let sect = sects.get(sect_of.get(i).copied().unwrap_or(0) as usize);
                            if sect.is_some_and(|s| s.vertical) {
                                continue;
                            }
                            self.grid.set(sect.map_or(page.grid, |s| s.setup.grid));
                            let mut l = self.layout_para(ctx, p, z, cw, None);
                            l.hash = h;
                            old.insert(h, l.clone());
                            l
                        }
                    };
                    (l.space_before, l.space_after, l.natural_after) = (paras[i].space_before, paras[i].space_after, paras[i].natural_after);
                    paras[i] = l;
                    laid_w[i] = Some(cw);
                    again = true;
                }
                if !again {
                    break;
                }
                fp = self.foot_pager(ctx, z, content, vals.unwrap_or(&no_vals), &mut old_notes, {
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    (key.1, key.2, key.3, key.4).hash(&mut h);
                    h.finish()
                });
                paged = sections::paginate(doc_paras, &paras, &tables, &sects, pt, &mut fp);
            }
        }
        let mut segs = std::mem::take(&mut paged.segs);
        if let Some(h) = &hidden {
            for s in &mut segs {
                s.hidden |= h.get(s.para).copied().unwrap_or(false);
            }
        }
        let mut first_seg = vec![0; paras.len()];
        for (k, s) in segs.iter().enumerate().rev() {
            first_seg[s.para] = k;
        }
        let (pages, notes) = fp.finish(paged.end.0, paged.end.1, &segs, &paras, doc);
        let pages = pages.max(paged.geo.len()).max(1);
        // Pages that only have the rest of notes.
        while paged.geo.len() < pages {
            let Some(mut g) = paged.geo.last().copied() else { break };
            g.top += g.size.y + PAGE_GAP;
            g.first = false;
            paged.geo.push(g);
        }
        let mut by_page: Vec<u32> = (0..segs.len() as u32).collect();
        if segs.windows(2).any(|w| w[1].page < w[0].page) {
            by_page.sort_by_key(|&k| segs[k as usize].page);
        }
        let page_start: Vec<usize> = (0..=pages).map(|pg| by_page.partition_point(|&k| segs[k as usize].page < pg)).collect();
        let labels = self.doc.page_labels(paged.geo.iter().map(|g| (g.sect, g.first)));
        Layout {
            labels,
            key,
            paras,
            segs,
            first_seg,
            pages,
            page: paged.max_size,
            content,
            geo: paged.geo,
            rules: paged.rules,
            by_page,
            page_start,
            ghosts: paged.ghosts,
            frags: paged.frags,
            tables,
            owner,
            refs: refs::RefLayout { values: Default::default(), notes },
            line_nums: Vec::new(),
            cols: 1,
            row_w: paged.max_size.x,
            gap: PAGE_GAP,
        }
    }

    // --- coordinates ------------------------------------------------------------------
    //
    // "Canvas" coordinates are relative to the top-left of the scrollable area.

    fn content_origin(&self, page: usize) -> Pos2 {
        let g = self.layout.geo_of(page);
        pos2(self.page_x + g.dx + g.margin.x, g.top + g.margin.y)
    }

    /// Canvas origin of a segment's column.
    fn seg_origin(&self, seg: &Segment) -> Pos2 {
        self.content_origin(seg.page) + vec2(seg.x, 0.0)
    }

    /// Canvas rect of the caret at `p` (on a vertical page, lying across
    /// the line).
    fn caret_rect(&self, p: Pos) -> Rect {
        let (r, page) = self.caret_at(p);
        self.turn_of(page).map_or(r, |t| t.rect(r))
    }

    /// Flow rect of the caret at `p` (see `tategaki`).
    fn caret_rect_flow(&self, p: Pos) -> Rect {
        self.caret_at(p).0
    }

    /// Flow rect of the caret at `p` and its page.
    fn caret_at(&self, p: Pos) -> (Rect, usize) {
        let (Some(pl), Some(para)) = (self.layout.paras.get(p.para), self.doc.paragraphs.get(p.para)) else { return (Rect::NOTHING, 0) };
        let ci = char_count(&para.text, p.offset);
        let r = pl.galley.pos_from_cursor(CCursor::new(ci));
        let segs = self.layout.segs_of(p.para);
        let Some(seg) = segs.iter().rev().find(|s| r.min.y >= s.gy0 - 0.5).or(segs.first()) else { return (Rect::NOTHING, 0) };
        let o = self.seg_origin(seg);
        (r.translate(vec2(o.x + pl.indent + pl.align_off, o.y + seg.y - seg.gy0)), seg.page)
    }

    /// Document position under a canvas point.
    fn pos_at(&self, p: Pos2) -> Pos {
        let (page, q) = self.flow_point(p);
        self.pos_in(page, q)
    }

    /// Document position under flow point `p` of page `page`.
    fn pos_in(&self, page: usize, p: Pos2) -> Pos {
        let l = &self.layout;
        let o = self.content_origin(page);
        let (lx, ly) = (p.x - o.x, p.y - o.y);
        // With columns, the segments of the column under the point.
        let col = sections::column_at(l.page_segs(page, page), lx);
        let visible = |s: &&Segment| !s.hidden && col.is_none_or(|x| s.x == x);
        // In a table row, the cell under the point.
        let in_cell = l.cell_hit(page, lx, ly).and_then(|(a, b)| {
            let mut on = l.page_segs(page, page).filter(visible).filter(|s| (a..b).contains(&s.para)).peekable();
            on.peek()?;
            let mut last = None;
            for s in on {
                if ly < s.y + (s.gy1 - s.gy0) {
                    return Some(*s);
                }
                last = Some(*s);
            }
            last
        });
        let seg = match in_cell {
            Some(s) => s,
            None => {
                let mut last = None;
                let mut found = None;
                for s in l.page_segs(page, page).filter(visible) {
                    if ly < s.y + (s.gy1 - s.gy0) {
                        found = Some(*s);
                        break;
                    }
                    last = Some(*s);
                }
                // An empty page (can't normally happen): the nearest segment.
                match found.or(last) {
                    Some(s) => s,
                    None => l.page_segs(0, page).filter(visible).last().or_else(|| l.segs.iter().find(|s| !s.hidden)).copied().unwrap_or(l.segs[0]),
                }
            }
        };
        let pl = &l.paras[seg.para];
        let gy = (ly - seg.y + seg.gy0).clamp(seg.gy0, seg.gy1 - 0.5);
        let gx = lx - seg.x - pl.indent - pl.align_off;
        let cc = pl.galley.cursor_from_pos(vec2(gx, gy));
        Pos::new(seg.para, char_to_byte(&self.doc.paragraphs[seg.para].text, cc.index.0))
    }

    /// The paragraph that Up or Down moves to from the first or last line of
    /// `para`, for a caret at content x `x`: through table cells by rows.
    fn vertical_neighbor(&self, para: usize, down: bool, x: f32) -> Option<usize> {
        let paras = &self.doc.paragraphs;
        let l = &self.layout;
        // Entering a table from below lands in its last row.
        let from_below = |t: usize| match l.table_at(t) {
            Some(tb) => tb.cell_at(tb.map.rows - 1, x).1.checked_sub(1),
            None => Some(t),
        };
        if let Some(tb) = l.table_at(para) {
            let m = &tb.map;
            let (r, c) = m.pos_of(paras, para);
            let (ar, ac) = m.anchor_of(r, c);
            let (a, b) = m.cell(ar, ac);
            return if down {
                if para + 1 < b {
                    Some(para + 1)
                } else if ar + m.span_of(ar, ac).0 < m.rows {
                    Some(tb.cell_at(ar + m.span_of(ar, ac).0, x).0)
                } else {
                    (m.end < paras.len()).then_some(m.end)
                }
            } else if para > a {
                Some(para - 1)
            } else if ar > 0 {
                tb.cell_at(ar - 1, x).1.checked_sub(1)
            } else {
                from_below(m.start.checked_sub(1)?)
            };
        }
        if down {
            let t = para + 1;
            match l.table_at(t) {
                _ if t >= paras.len() => None,
                Some(tb) if tb.map.start == t => Some(tb.cell_at(0, x).0),
                _ => Some(t),
            }
        } else {
            from_below(para.checked_sub(1)?)
        }
    }

    fn move_vertical(&mut self, down: bool, extend: bool) {
        let cur = self.sel.cursor;
        let cr = self.caret_rect_flow(cur);
        let x = *self.desired_x.get_or_insert(cr.center().x);
        let pl = &self.layout.paras[cur.para];
        let text = &self.doc.paragraphs[cur.para].text;
        let cc = CCursor::new(text[..cur.offset].chars().count());
        let row = pl.galley.layout_from_cursor(cc).row;
        let content_x = x - self.content_origin(0).x;
        let target = if down {
            if row + 1 < pl.galley.rows.len() {
                let r = &pl.galley.rows[row + 1];
                Some((cur.para, r.pos.y + r.row.size.y / 2.0))
            } else {
                self.vertical_neighbor(cur.para, true, content_x).map(|t| (t, 1.0))
            }
        } else if row > 0 {
            let r = &pl.galley.rows[row - 1];
            Some((cur.para, r.pos.y + r.row.size.y / 2.0))
        } else {
            self.vertical_neighbor(cur.para, false, content_x).map(|t| (t, self.layout.paras[t].galley.size().y - 1.0))
        };
        let new = match target {
            Some((pi, gy)) => {
                let pl = &self.layout.paras[pi];
                let segs = self.layout.segs_of(pi);
                let seg = segs.iter().rev().find(|s| gy >= s.gy0).unwrap_or(&segs[0]);
                let o = self.seg_origin(seg);
                let gx = x - o.x - pl.indent - pl.align_off;
                let cc = pl.galley.cursor_from_pos(vec2(gx, gy));
                Pos::new(pi, char_to_byte(&self.doc.paragraphs[pi].text, cc.index.0))
            }
            None if down => self.doc.end(),
            None => Pos::new(0, 0),
        };
        let keep = self.desired_x;
        self.caret(new, extend);
        self.desired_x = keep;
    }

    fn row_edge(&mut self, end: bool, extend: bool) {
        let cur = self.sel.cursor;
        let pl = &self.layout.paras[cur.para];
        let text = &self.doc.paragraphs[cur.para].text;
        let cc = CCursor::new(text[..cur.offset].chars().count());
        let c = if end { pl.galley.cursor_end_of_row(&cc) } else { pl.galley.cursor_begin_of_row(&cc) };
        self.caret(Pos::new(cur.para, char_to_byte(text, c.index.0)), extend);
    }

    fn page_move(&mut self, down: bool, extend: bool) {
        let cr = self.caret_rect(self.sel.cursor);
        let dy = if down { self.viewport_h * 0.9 } else { -self.viewport_h * 0.9 };
        let target = pos2(cr.center().x, (cr.center().y + dy).clamp(0.0, self.layout.total_height()));
        let p = self.pos_at(target);
        self.caret(p, extend);
    }

    /// Moves the caret to the top of page `n` (0-based).
    fn go_to_page(&mut self, n: usize) {
        let n = n.min(self.layout.pages.saturating_sub(1));
        let p = self.pos_in(n, self.content_origin(n) + vec2(0.0, 1.0));
        self.caret(p, false);
    }

    // --- editing ------------------------------------------------------------------------

    fn insert(&mut self, text: &str) {
        // In a legacy text field: within its length, cased, numbers only.
        let fitted = self.form_typed(text);
        let text = fitted.as_deref().unwrap_or(text);
        if text.is_empty() {
            return;
        }
        if self.control_typing(text) {
            return;
        }
        // Typing over cells of a table clears them and types in the first.
        self.clear_block();
        let text = &*self.autocorrect_typed(text).into_owned();
        let attrs = self.pending.take();
        let mut p = self.doc.insert_text(self.sel, text, attrs);
        self.sel = Selection::caret(p);
        self.desired_x = None;
        self.scroll_to_caret = true;
        // AutoCorrect when a word ends.
        if matches!(text, " " | "\t" | "." | "," | ";" | ":" | "!" | "?" | ")" | "\u{2028}") && p.offset >= text.len() {
            self.autocorrect_after(Pos::new(p.para, p.offset - text.len()), text);
            p = self.sel.cursor;
        }
        // AutoFormat as you type: a URL followed by a space becomes a link.
        if matches!(text, " " | "\t" | "\u{2028}") && p.offset >= text.len() {
            self.doc.autolink(Pos::new(p.para, p.offset - text.len()));
        }
    }

    pub fn undo(&mut self) {
        if self.md_source() {
            return self.md_undo(false);
        }
        if let Some(s) = self.doc.undo() {
            self.set_sel(s);
        }
    }

    pub fn redo(&mut self) {
        if self.md_source() {
            return self.md_undo(true);
        }
        if let Some(s) = self.doc.redo() {
            self.set_sel(s);
        }
    }

    fn toggle(&mut self, get: fn(&Attrs) -> bool, set: fn(&mut Attrs, bool)) {
        if let Some(a) = self.shape_text_attrs() {
            let on = !get(&a);
            self.shape_text_format(&move |a| set(a, on));
            return;
        }
        if self.sel.is_empty() {
            let mut a = self.pending.clone().unwrap_or_else(|| self.attrs_at_caret());
            let on = !get(&a);
            set(&mut a, on);
            self.set_pending(a);
        } else {
            self.doc.toggle(self.sel, get, set);
        }
    }

    fn format(&mut self, f: impl Fn(&mut Attrs) + 'static) {
        if self.shape_text_format(&f) {
            return;
        }
        if self.sel.is_empty() {
            let mut a = self.pending.clone().unwrap_or_else(|| self.attrs_at_caret());
            f(&mut a);
            self.set_pending(a);
        } else {
            self.doc.format_range(self.sel, &f);
        }
    }

    /// The format for what is typed next: none kept when it is what the
    /// text at the caret has anyway (Bold clicked twice leaves no trace).
    fn set_pending(&mut self, a: Attrs) {
        self.pending = (a != self.caret_attrs()).then_some(a);
    }

    fn attrs_at_caret(&self) -> Attrs {
        if let Some(a) = self.shape_text_attrs() {
            return a;
        }
        match &self.pending {
            Some(a) => a.clone(),
            None => self.caret_attrs(),
        }
    }

    /// The format of the text at the caret (without a pending one).
    fn caret_attrs(&self) -> Attrs {
        let c = self.sel.start();
        let p = &self.doc.paragraphs[c.para];
        // The character before the caret decides, like other editors.
        let off = if self.sel.is_empty() && c.offset > 0 { prev_boundary(&p.text, c.offset) } else { c.offset };
        let mut a = p.attrs_at(off);
        // Text typed right after a link doesn't continue it.
        if self.sel.is_empty() && self.doc.typing_attrs(c).link.is_none() {
            (a.link, a.link_tip) = (None, None);
        }
        a.clear_object();
        a
    }

    fn para_range(&self) -> (usize, usize) {
        (self.sel.start().para, self.sel.end().para)
    }

    fn para_style(&mut self, f: impl Fn(&mut ParaStyle) + 'static) {
        if self.shape_para_style(&f) {
            return;
        }
        let (a, b) = self.para_range();
        self.doc.set_para_style(a, b, self.sel, &f);
    }

    fn set_kind(&mut self, kind: ParaKind) {
        if self.shape_list(kind) {
            return;
        }
        if !kind.is_list() && self.linked_partial(kind, 0) {
            return self.apply_style(kind, 0);
        }
        let (a, b) = self.para_range();
        let all_same = (a..=b).all(|i| self.doc.paragraphs[i].style.kind == kind);
        // Toggling a list off returns to normal text.
        let kind = if all_same && kind.is_list() { ParaKind::Normal } else { kind };
        let builtin = self.doc.styles.builtin(kind).cloned();
        self.doc.set_para_style(a, b, self.sel, &move |s: &mut ParaStyle| {
            s.kind = kind;
            s.custom = 0;
            s.list = 0;
            s.start_at = 0;
            if !kind.is_list() {
                s.level = 0;
            }
            if let Some(d) = &builtin {
                d.apply_para(s);
            }
        });
        self.doc.clear_linked_text(self.sel);
    }

    /// A bulleted or numbered list with a given style (applied, not toggled).
    fn set_list(&mut self, kind: ParaKind, format: u8) {
        self.para_style(move |s| {
            if !s.kind.is_list() {
                s.level = 0;
            }
            s.kind = kind;
            s.list_format = format;
            s.list = 0;
            s.start_at = 0;
        });
    }

    fn set_align(&mut self, align: Align) {
        self.para_style(move |s| s.align = align);
    }

    /// List items change level; other paragraphs change their left indent.
    fn indent(&mut self, delta: i32) {
        self.para_style(move |s| {
            if s.kind.is_list() {
                s.level = (s.level as i32 + delta).clamp(0, 8) as u8;
            } else {
                s.indent_left = (s.indent_left + delta as f32 * INDENT_STEP_PT).max(0.0);
            }
        });
    }

    fn set_line_spacing(&mut self, v: f32) {
        self.para_style(move |s| {
            s.line_rule = LineRule::Multiple;
            s.line_spacing = if v == 1.0 { 0.0 } else { v };
        });
    }

    fn copy(&mut self, ctx: &egui::Context, cut: bool) {
        if self.sel.is_empty() {
            return;
        }
        // Cells of a table copy as tab-separated text (and as cells).
        if let Some(b) = self.doc.block_selection(self.sel) {
            let text = self.doc.block_text(&b);
            let paras = self.doc.block_slice(&b);
            self.to_system_clipboard(ctx, &text, &paras);
            self.set_clip(text, paras);
            if cut {
                self.clear_block();
            }
            return;
        }
        let (a, b) = (self.sel.start(), self.sel.end());
        // Deleted text (tracked changes) isn't copied.
        let text = self.doc.visible_text_range(a, b);
        let mut paras = self.doc.slice(a, b);
        if paras.iter().any(|p| p.has_deleted()) {
            paras = Document::from_paragraphs(paras).resolved(true).paragraphs;
        }
        self.to_system_clipboard(ctx, &text, &paras);
        self.set_clip(text, paras);
        if cut {
            let (blocked, emptied) = self.control_deleting(true);
            if blocked {
                return;
            }
            let p = self.doc.delete(self.sel);
            self.sel = Selection::caret(p);
            if let Some(c) = emptied {
                self.refill_control(c);
            }
        }
    }

    /// Puts what was copied on the system clipboard: as text, HTML and RTF
    /// for other programs, and as a document for ApexDesk, which keeps
    /// everything.
    fn to_system_clipboard(&self, ctx: &egui::Context, text: &str, paras: &[Paragraph]) {
        use crate::sysclip::{DOC, HTML, RTF, TEXT};
        let text = text.replace(LINE_BREAK, "\n");
        let mut frag = Document::from_paragraphs(paras.to_vec());
        frag.styles = self.doc.styles.clone();
        frag.default_font = self.doc.default_font.clone();
        let html = apex_io::html::fragment_html(&frag);
        let rtf = apex_io::rtf::write(&frag);
        let docx = apex_io::docx::write(&frag);
        if !crate::sysclip::write(&[(TEXT, text.as_bytes()), (HTML, html.as_bytes()), (RTF, &rtf), (DOC, &docx)]) {
            ctx.copy_text(text);
        }
    }

    /// The formatted text another program, or ApexDesk in another
    /// process, put on the system clipboard, as paragraphs of this
    /// document. ApexDesk's own format first, then RTF (word processors),
    /// then HTML (browsers, mail).
    fn system_clip(&mut self, text: &str) -> Option<Vec<Paragraph>> {
        use crate::sysclip::{DOC, HTML, RTF};
        let (name, bytes) = crate::sysclip::read(&[DOC, RTF, HTML])?;
        // Whatever another program put there: a reader's slip is no paste,
        // not the end of the program.
        let doc = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match name.as_str() {
            DOC => apex_io::docx::read(&bytes).ok(),
            RTF => apex_io::rtf::read(&bytes).ok(),
            _ => Some(apex_io::html_in::read(&crate::sysclip::text_of(&bytes))),
        }))
        .ok()??;
        let mut paras = doc.paragraphs.clone();
        // Most programs end RTF and HTML with a paragraph's end the text
        // doesn't have: the paste stays in the paragraph at the caret.
        if !text.ends_with('\n') && paras.len() > 1 && paras.last().is_some_and(|p| p.text.is_empty() && p.cell.is_none()) {
            paras.pop();
        }
        if paras.iter().all(|p| p.text.is_empty()) {
            return None;
        }
        let origin = doc.origin(&paras);
        Some(self.doc.adopt(&paras, &origin))
    }

    /// Keeps what was just copied: here, and for the other windows.
    fn set_clip(&mut self, text: String, paras: Vec<Paragraph>) {
        let origin = Arc::new(self.doc.origin(&paras));
        crate::clip_history::push(crate::clip_history::Entry { text: text.replace(LINE_BREAK, "\n"), rich: crate::clip_history::Rich::Doc(Arc::new(paras.clone()), origin.clone()) });
        self.clip = Some((text, paras, origin));
    }

    /// The formatted text that goes with the clipboard's `text`, as
    /// paragraphs of this document: what was copied here or in another
    /// document of ours. `None`: text of another program.
    fn clip_for(&mut self, text: &str) -> Option<Vec<Paragraph>> {
        use crate::clip_history::Rich;
        let (paras, origin) = match &self.clip {
            Some((t, paras, origin)) if t.replace(LINE_BREAK, "\n") == text => (Arc::new(paras.clone()), origin.clone()),
            _ => match crate::clip_history::find(text)?.rich {
                Rich::Doc(paras, origin) => (paras, origin),
                // Text of a slide says its whole look itself.
                Rich::Runs(paras) => return Some(paras.to_vec()),
                // Cells of a workbook: a table as wide as the text at most.
                Rich::Cells(cells) => return cells_in::cells_table(text, &cells, self.table_room()),
                _ => return None,
            },
        };
        Some(self.doc.adopt(&paras, &origin))
    }

    /// The Clipboard pane.
    fn clipboard_pane(&mut self, ctx: &egui::Context) {
        use crate::clip_history::{Action, Entry, Rich};
        if !self.clip_pane {
            return;
        }
        let mut open = true;
        let action = crate::clip_history::pane(ctx, Id::new(("clip-pane", self.uid)), &mut open, true);
        self.clip_pane = open;
        let paste = |v: &mut Self, e: Entry| {
            v.clip = match e.rich {
                Rich::Doc(p, origin) => Some((e.text.clone(), (*p).clone(), origin)),
                _ => None,
            };
            v.paste(&e.text, false);
        };
        match action {
            Some(Action::Paste(e)) => {
                ctx.copy_text(e.text.clone());
                paste(self, e);
                self.focus = true;
            }
            Some(Action::PasteAll(all)) => {
                for e in all {
                    paste(self, e);
                }
                self.focus = true;
            }
            None => {}
        }
    }

    /// The text selected, as plain text.
    /// The paragraph the selection starts in.
    pub fn selection_para(&self) -> usize {
        self.sel.start().para
    }

    pub fn selected_text(&self) -> String {
        self.doc.text_range(self.sel.start(), self.sel.end())
    }

    fn request_paste(&mut self, ctx: &egui::Context, plain: bool) {
        self.paste_expected = true;
        self.paste_plain = plain;
        self.focus = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        // Nothing comes back when the clipboard holds only a picture.
        if !plain {
            self.paste_clipboard_picture(ctx);
        }
    }

    fn paste(&mut self, text: &str, plain: bool) {
        self.paste_from(text, plain, false);
    }

    /// Pastes `text`; `system`: it came from the system clipboard (Ctrl+V,
    /// the Paste button), whose formatted text comes in with it.
    fn paste_from(&mut self, text: &str, plain: bool, system: bool) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if text.is_empty() {
            return;
        }
        // Copied in another program (what we copied stays as it is).
        crate::clip_history::push(crate::clip_history::Entry { text: text.clone(), rich: crate::clip_history::Rich::Text });
        let clip = if plain { None } else { self.clip_for(&text).or_else(|| if system { self.system_clip(&text) } else { None }) };
        if self.paste_cells(&text, clip.as_deref()) {
            return;
        }
        // The text of a picture copied in another program is the object
        // mark, no text: a picture alone leaves nothing here and comes
        // from the clipboard's picture instead.
        let text = if clip.is_none() && text.contains(apex_doc::OBJECT) { text.replace(apex_doc::OBJECT, "") } else { text };
        if clip.is_none() && text.is_empty() {
            return;
        }
        // Formatted text with its pictures: the clipboard's picture is
        // one of them, not another to insert.
        if system && clip.as_ref().is_some_and(|ps| ps.iter().any(|p| p.runs.iter().any(|r| r.attrs.image.is_some()))) {
            self.picture_pasted();
        }
        // Into a content control: as typed text, on one line.
        if self.caret_control().is_some_and(|c| c.control.is_form() && c.control.kind.typed()) {
            self.insert(&text.replace('\n', " "));
            return;
        }
        if self.caret_control().is_some() && self.control_typing(&text.replace('\n', " ")) {
            return;
        }
        self.clear_block();
        let before = self.sel;
        let p = match &clip {
            Some(paras) => self.doc.insert_paragraphs(self.sel, paras),
            // Unformatted text takes the formatting at the caret.
            None => {
                let attrs = Some(self.attrs_at_caret());
                self.doc.insert_text(self.sel, &text, attrs)
            }
        };
        self.sel = Selection::caret(p);
        self.scroll_to_caret = true;
        self.pasted(before, &text, clip);
    }

    /// The selection, or the word at the caret when nothing is selected.
    fn sel_or_word(&self) -> Selection {
        if self.sel.is_empty() {
            let (a, b) = self.doc.word_at(self.sel.cursor);
            Selection { anchor: a, cursor: b }
        } else {
            self.sel
        }
    }

    /// Clears character formatting (Ctrl+Space); `para` also resets the
    /// paragraph formatting (the toolbar button).
    fn clear_formatting(&mut self, para: bool) {
        let clear = |a: &mut Attrs| {
            let (link, image, note, field, rev, math, shape) = (a.link.take(), a.image.take(), a.note.take(), a.field.take(), a.rev.take(), a.math.take(), a.shape.take());
            *a = Attrs { link, image, note, field, rev, math, shape, ..Attrs::default() };
        };
        if para {
            self.para_style(|s| {
                let list = s.kind.is_list();
                *s = ParaStyle {
                    kind: if list { s.kind } else { ParaKind::Normal },
                    level: if list { s.level } else { 0 },
                    list_format: s.list_format,
                    restart: s.restart,
                    page_break_before: s.page_break_before,
                    ..ParaStyle::default()
                };
            });
        }
        if self.sel.is_empty() {
            self.pending = Some(Attrs::default());
        } else {
            self.doc.format_range(self.sel, &clear);
        }
    }

    /// Font size up or down one step of the size list, per run.
    fn grow_font(&mut self, up: bool) {
        let normal = self.doc.base_size(ParaKind::Normal);
        let step = move |kind: ParaKind, a: &mut Attrs| {
            let cur = a.size.unwrap_or(if kind.inherits_size() { normal } else { kind.base_size() }) as f32 / 2.0;
            a.size = Some((common::step_size(cur, up) * 2.0).round() as u16);
        };
        if self.sel.is_empty() {
            let kind = self.cur_para().style.kind;
            let mut a = self.attrs_at_caret();
            step(kind, &mut a);
            self.pending = Some(a);
        } else {
            self.doc.format_with(self.sel, &step, None);
        }
    }

    fn change_case(&mut self, case: Case) {
        let sel = self.sel_or_word();
        if sel.is_empty() {
            return;
        }
        let text = self.doc.text_range(sel.start(), sel.end());
        let pieces = std::cell::RefCell::new(case_pieces(&text, case).into_iter());
        // `map_text` walks the runs in order; hand each one its converted chars.
        let f = |run: &str| -> String {
            let mut it = pieces.borrow_mut();
            run.chars().map(|_| it.next().unwrap_or_default()).collect()
        };
        let after = self.doc.map_text(sel, &f);
        self.sel = after;
    }

    /// Shift+F3: UPPER → lower → Title → UPPER…
    fn cycle_case(&mut self) {
        let sel = self.sel_or_word();
        let text = self.doc.text_range(sel.start(), sel.end());
        let has_lower = text.chars().any(char::is_lowercase);
        let has_upper = text.chars().any(char::is_uppercase);
        let next = if !has_lower && has_upper {
            Case::Lower
        } else if has_lower && !has_upper {
            Case::Title
        } else {
            Case::Upper
        };
        self.change_case(next);
    }

    fn insert_date_time(&mut self, date: bool, time: bool) {
        let (date_s, time_s) = common::date_time_now();
        let s = match (date, time) {
            (true, true) => format!("{date_s} {time_s}"),
            (true, false) => date_s,
            _ => time_s,
        };
        self.insert(&s);
    }

    // --- links and bookmarks -------------------------------------------------------------

    /// The link on the character under a canvas point (not just near it).
    fn link_under(&self, local: Pos2) -> Option<LinkHit> {
        let ch = self.char_under(local)?;
        self.doc.link_at(ch).filter(|(s, _, _)| s.start() <= ch && ch < s.end())
    }

    /// The character under `local` (a point of the page), if any.
    fn char_under(&self, local: Pos2) -> Option<Pos> {
        if self.layout.segs.is_empty() {
            return None;
        }
        let (page, local) = self.flow_point(local);
        let pos = self.pos_in(page, local);
        let text = &self.doc.paragraphs[pos.para].text;
        let here = self.caret_rect_flow(pos);
        if local.y < here.min.y || local.y > here.max.y {
            return None;
        }
        // The character right of the boundary, or the one left of it.
        let ch = if local.x >= here.min.x {
            let next = self.doc.next_char(pos);
            (next.para == pos.para && next.offset > pos.offset && local.x <= self.caret_rect_flow(next).min.x).then_some(pos)
        } else {
            let prev = self.doc.prev_char(pos);
            (prev.para == pos.para && prev.offset < pos.offset && local.x >= self.caret_rect_flow(prev).min.x).then_some(prev)
        }?;
        (!text[ch.offset..].starts_with(LINE_BREAK)).then_some(ch)
    }

    /// Ctrl+click: jumps to a bookmark ("#name") or opens the target with
    /// the system's handler.
    fn follow_link(&mut self, target: &str) {
        if self.is_page_link(target) {
            self.note_follow = Some(target.into());
        } else if let Some(name) = target.strip_prefix('#') {
            match self.doc.bookmark(name) {
                Some(b) => self.go_to_bookmark(&b),
                None => self.notices.push(Notice::info(trf("link.no_bookmark", &[("name", &name)]))),
            }
        } else if let Err(e) = crate::links::follow(target) {
            self.notices.push(Notice::error(e));
        }
    }

    /// A link to another page of the notebook this editor is in.
    fn is_page_link(&self, target: &str) -> bool {
        self.embedded && (target.starts_with(apex_doc::notebook::links::SCHEME) || target.starts_with(crate::note_view::fragments::ORIGIN) || target.starts_with(crate::note_view::attachments::ATTACH))
    }

    /// Embedded: `[[` just typed asks for a page to link.
    pub(crate) fn check_link_prompt(&mut self, typed: &str) {
        if !self.embedded || !typed.ends_with('[') || !self.sel.is_empty() {
            return;
        }
        let c = self.sel.cursor;
        if c.offset >= 2 && self.doc.paragraphs[c.para].text[..c.offset].ends_with("[[") {
            self.link_prompt = Some(Pos::new(c.para, c.offset - 2));
        }
    }

    /// Embedded: replaces the `[[` typed at `at` with a link to a page.
    pub fn finish_link_prompt(&mut self, at: Pos, title: &str, target: Arc<str>) {
        let end = Pos::new(at.para, at.offset + 2);
        if self.doc.paragraphs.get(at.para).is_none_or(|p| p.text.get(at.offset..end.offset) != Some("[[")) {
            return;
        }
        let link = self.doc.insert_link(Selection { anchor: at, cursor: end }, title, target, None);
        // Typing on after it is no longer the link.
        self.set_sel(Selection { anchor: link.cursor, cursor: link.cursor });
        let mut a = self.doc.typing_attrs(link.cursor);
        (a.link, a.link_tip) = (None, None);
        self.pending = Some(a);
    }

    /// Embedded: `paras` (a page shown in this one) on lines of their own
    /// after the caret's line (or the line where `![[` was typed at `at`,
    /// which goes), and the caret on a line after them.
    pub fn insert_embed(&mut self, at: Option<Pos>, paras: &[Paragraph]) {
        if let Some(at) = at
            && at.offset > 0
            && self.doc.paragraphs.get(at.para).is_some_and(|p| p.text.get(at.offset - 1..at.offset + 2) == Some("![["))
        {
            let start = Pos::new(at.para, at.offset - 1);
            self.doc.insert_text(Selection { anchor: start, cursor: Pos::new(at.para, at.offset + 2) }, "", None);
            self.set_sel(Selection::caret(start));
        }
        let para = self.sel.end().para;
        let mut plain = self.doc.typing_attrs(self.sel.end());
        (plain.link, plain.link_tip, plain.control) = (None, None, None);
        let end = Pos::new(para, self.doc.paragraphs[para].text.len());
        let mut at = end;
        if !self.doc.paragraphs[para].text.is_empty() {
            at = self.doc.insert_text(Selection::caret(end), "\n", Some(plain.clone()));
        }
        let last = self.doc.insert_paragraphs(Selection::caret(at), paras);
        let after = self.doc.insert_text(Selection::caret(last), "\n", Some(plain.clone()));
        // The line after it is the page's own again, not the embed's.
        self.doc.paragraphs[after.para].style = self.doc.paragraphs[para].style.clone();
        self.set_sel(Selection::caret(after));
        self.pending = Some(plain);
        self.focus = true;
    }

    /// Embedded: a link at the caret (to a file kept in the notebook), on
    /// a line of its own.
    /// Hands the keyboard to the text.
    pub fn focus_text(&mut self) {
        self.focus = true;
    }

    pub fn insert_note_link(&mut self, text: &str, target: Arc<str>) {
        // After what is selected (a picture just dropped), not over it.
        self.set_sel(Selection::caret(self.sel.end()));
        // On a line of its own: apart from the text before and after it.
        let plain = |v: &Self, at| {
            let mut a = v.doc.typing_attrs(at);
            (a.link, a.link_tip) = (None, None);
            a
        };
        if self.sel.start().offset > 0 {
            let a = plain(self, self.sel.start());
            let at = self.doc.insert_text(self.sel, "\n", Some(a));
            self.set_sel(Selection::caret(at));
        }
        let rest = !self.doc.paragraphs[self.sel.start().para].text.is_empty();
        let link = self.doc.insert_link(self.sel, text, target, None);
        let a = plain(self, link.cursor);
        if rest {
            self.doc.insert_text(Selection::caret(link.cursor), "\n", Some(a.clone()));
        }
        self.set_sel(Selection::caret(link.cursor));
        self.pending = Some(a);
        self.focus = true;
    }

    fn go_to_bookmark(&mut self, b: &apex_doc::Bookmark) {
        self.set_sel(Selection { anchor: b.start, cursor: b.end });
        self.desired_x = None;
    }

    /// Removes the link at the caret (or on the selection).
    fn remove_link(&mut self, hit: Option<LinkHit>) {
        let range = match hit.or_else(|| self.doc.link_at(self.sel.cursor)) {
            Some((r, _, _)) if self.sel.is_empty() || (r.start() <= self.sel.start() && self.sel.end() <= r.end()) => r,
            _ => self.sel,
        };
        self.doc.set_link(range, None, None);
    }

    // --- find -----------------------------------------------------------------------------

    /// All matches of the find bar query (cached per document revision).
    fn find_hits(&mut self) -> &[(Pos, Pos)] {
        let rev = self.doc.revision;
        let Some(f) = &mut self.find else { return &[] };
        let o = f.opts();
        let key = (rev, f.query.clone(), o.clone());
        if f.hits_key.as_ref() != Some(&key) {
            let r = if f.active() { self.doc.find_ex(&f.query, &o) } else { Ok(Vec::new()) };
            (f.hits, f.error) = match r {
                Ok(h) => (h, None),
                Err(e) => (Vec::new(), Some(e)),
            };
            f.hits_key = Some(key);
        }
        &f.hits
    }

    fn find_next(&mut self, backwards: bool) -> bool {
        let (start, end) = (self.sel.start(), self.sel.end());
        let hits = self.find_hits().to_vec();
        if hits.is_empty() {
            if let Some(f) = self.find.as_ref().filter(|f| f.active()) {
                let msg = match &f.error {
                    Some(e) => format!("{}: {e}", tr("find.bad_pattern")),
                    None => tr("find.not_found").to_string(),
                };
                self.notices.push(Notice::info(msg));
            }
            return false;
        }
        let hit = if backwards {
            hits.iter().rev().find(|(a, _)| *a < start).or(hits.last()).copied()
        } else {
            hits.iter().find(|(a, _)| *a >= end && !(*a == start && start != end)).or(hits.first()).copied()
        };
        let (a, b) = hit.unwrap();
        self.set_sel(Selection { anchor: a, cursor: b });
        true
    }

    /// The query, replacement, options and replacement format.
    fn replace_args(&self) -> Option<(String, String, apex_doc::FindOpts, apex_doc::FindFormat)> {
        let f = self.find.as_ref()?;
        Some((f.query.clone(), f.replace.clone(), f.opts(), if f.more { f.rformat.clone() } else { Default::default() }))
    }

    fn replace_one(&mut self) {
        let Some((q, r, o, rf)) = self.replace_args() else { return };
        let cur = (self.sel.start(), self.sel.end());
        if !self.sel.is_empty() && self.find_hits().contains(&cur) {
            if let Ok((_, end)) = self.doc.replace_ex(&q, &r, &o, &rf, Some(cur), self.sel) {
                self.sel = Selection::caret(end);
            }
        }
        self.find_next(false);
    }

    fn replace_all(&mut self) {
        let Some((q, r, o, rf)) = self.replace_args() else { return };
        if !self.find.as_ref().is_some_and(FindBar::active) {
            return;
        }
        match self.doc.replace_ex(&q, &r, &o, &rf, None, self.sel) {
            Ok((n, _)) => {
                self.sel = Selection::caret(self.doc.clamp(self.sel.cursor));
                self.notices.push(Notice::info(trf("find.replaced", &[("n", &n)])));
            }
            Err(e) => self.notices.push(Notice::info(format!("{}: {e}", tr("find.bad_pattern")))),
        }
    }

    fn open_find(&mut self, replace: bool) {
        if self.md_source() {
            return self.md_open_find(replace);
        }
        let f = self.find.get_or_insert_with(FindBar::default);
        f.nav = false;
        f.show_replace |= replace;
        f.focus = true;
        if !self.sel.is_empty() && self.sel.start().para == self.sel.end().para {
            f.query = self.doc.text_range(self.sel.start(), self.sel.end());
        }
    }

    // --- UI ---------------------------------------------------------------------------------

    pub fn status(&mut self, ui: &mut egui::Ui) {
        if self.counts.is_none_or(|(r, _, _)| r != self.doc.revision) {
            self.counts = Some((self.doc.revision, self.doc.word_count(), self.doc.char_count()));
        }
        let (_, words, chars) = self.counts.unwrap();
        if !self.layout.segs.is_empty() && self.sel.cursor.para < self.layout.paras.len() {
            let page = self.caret_page() + 1;
            let r = ui.add(egui::Label::new(trf("doc.page_of", &[("n", &page), ("total", &self.layout.pages)])).sense(Sense::click()));
            if r.on_hover_text(format!("{} (Ctrl+G)", tr("doc.goto_page"))).clicked() {
                self.open_goto();
            }
            ui.separator();
        }
        if ui.add(egui::Label::new(trf("doc.words", &[("n", &words)])).sense(Sense::click())).on_hover_text(format!("{} (Ctrl+Shift+G)", tr("count.title"))).clicked() {
            self.open_word_count();
        }
        ui.label(trf("doc.chars", &[("n", &chars)]));
        if !self.sel.is_empty() {
            ui.separator();
            let n = apex_doc::count_words(&self.doc.visible_text_range(self.sel.start(), self.sel.end()));
            ui.label(trf("doc.selected_words", &[("n", &n)]));
        }
        self.revision_status(ui);
        if let Some(t) = crate::spell::status() {
            ui.separator();
            ui.weak(t);
        }
        if let Some(b) = &self.brush {
            ui.separator();
            ui.label(RichText::new(tr(if b.sticky { "brush.active_sticky" } else { "brush.active" })).color(theme::SHEET_ACCENT));
        }
    }

    fn caret_page(&self) -> usize {
        let r = self.caret_rect(self.sel.cursor);
        self.layout.page_at(r.center().x - self.page_x, r.center().y)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        self.refs.ctx = Some(ui.ctx().clone());
        self.sync_bindings();
        self.sync_brush();
        crate::themes_ui::set_active(&self.doc.styles.theme);
        // The Code view and the commands change the paragraphs under the
        // selection: it never points past them.
        self.sel = Selection { anchor: self.doc.clamp(self.sel.anchor), cursor: self.doc.clamp(self.sel.cursor) };
        if self.md_source() {
            self.md_source_ui(ui);
            self.file_name.clone_from(&shell.file_name);
            return;
        }
        let area = ui.max_rect();
        let find_bar = self.find.as_ref().is_some_and(|f| !f.nav);
        if find_bar {
            egui::Panel::top(Id::new(("doc_toolbar", self.uid)))
                .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 4)))
                .show(ui, |ui| self.find_bar(ui));
        }
        if self.doc.edit_limit().is_some() {
            egui::Panel::top(Id::new(("doc_protect_bar", self.uid)))
                .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 4)))
                .show(ui, |ui| self.protect_bar(ui));
        }
        if self.views.mode == ViewMode::Read {
            egui::Panel::top(Id::new(("doc_read_bar", self.uid)))
                .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 5)))
                .show(ui, |ui| self.read_bar(ui));
        }
        self.view_keys(ui.ctx());
        if self.hf_edit.is_some() {
            egui::Panel::top(Id::new(("doc_hf_bar", self.uid)))
                .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 4)))
                .show(ui, |ui| self.hf_bar(ui));
        }
        self.quiet_frame(shell);
        let quiet = self.quiet();
        if !quiet {
            self.nav_pane(ui);
            self.book_panel(ui);
        }
        self.clamp_scope();
        if self.scoped() {
            egui::Panel::top(Id::new(("doc_scope_bar", self.uid)))
                .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 5)))
                .show(ui, |ui| self.scope_bar(ui));
        }
        if !quiet {
            self.review_pane(ui);
            self.notebook_pane(ui);
        }
        self.notes_pane(ui);
        self.select_pane(ui);
        // The rulers stand over and beside the pages only; their room is
        // kept now and they are drawn after the pages, with this frame's
        // scroll and zoom.
        let cards = self.is_book() && self.book.view != book_ui::BookView::Text;
        let rulers = if cards { Default::default() } else { self.ruler_panels(ui) };
        if cards && self.book.view == book_ui::BookView::Table {
            egui::CentralPanel::default_margins().frame(egui::Frame::new()).show(ui, |ui| self.book_table(ui));
        } else if cards {
            egui::CentralPanel::default_margins().frame(egui::Frame::new()).show(ui, |ui| self.book_cards(ui));
        } else if self.views.mode == ViewMode::Outline {
            egui::CentralPanel::default_margins().frame(egui::Frame::new()).show(ui, |ui| self.outline_view(ui));
        } else {
            egui::CentralPanel::default_margins().frame(egui::Frame::new()).show(ui, |ui| self.panes(ui));
        }
        self.draw_rulers(ui, rulers);
        if self.views.mode == ViewMode::Focus {
            self.focus_exit(ui.ctx());
        }
        if quiet {
            self.quiet_count(ui.ctx());
        }
        self.file_name.clone_from(&shell.file_name);
        self.dialogs(ui.ctx());
        self.list_dialogs(ui.ctx());
        self.table_dialogs(ui.ctx());
        self.layout_dialogs(ui.ctx());
        self.page_setup_dialog(ui.ctx());
        self.font_dialog(ui.ctx());
        self.picture_dialog(ui.ctx());
        self.annotate_window(ui.ctx());
        self.object_dialog(ui.ctx());
        self.barcode_dialog(ui.ctx());
        self.shape_dialog(ui.ctx());
        self.spelling_dialog(ui.ctx());
        self.word_count_dialog(ui.ctx());
        self.mail_dialogs(ui.ctx());
        self.blocks_dialog(ui.ctx());
        self.merge_files_dialog(ui.ctx());
        self.styles_pane(ui.ctx());
        self.reveal_pane(ui.ctx());
        self.access_window(ui.ctx());
        self.style_dialog(ui.ctx());
        self.table_style_dialog(ui.ctx());
        self.line_num_dialog(ui.ctx());
        self.controls_ui(ui.ctx());
        self.form_props_dialog(ui.ctx());
        self.thesaurus_ui(ui.ctx());
        self.clipboard_pane(ui.ctx());
        self.language_dialog(ui.ctx());
        self.ruby_dialog(ui.ctx());
        self.asian_layout_dialog(ui.ctx());
        self.equation_dialog(ui.ctx());
        if let Some(cmd) = self.mail.out.take() {
            shell.cmd = Some(cmd);
        }
        self.ref_dialogs(ui.ctx());
        self.user_name_dialog(ui.ctx());
        self.restrict_dialog(ui.ctx());
        self.book_dialog(ui.ctx());
        if self.is_book() && ui.input_mut(|i| i.modifiers.matches_exact(Modifiers::COMMAND) && i.consume_key(Modifiers::COMMAND, Key::P)) {
            self.open_go_to();
        }
        self.go_to_ui(ui.ctx());
        self.goals_dialog(ui.ctx());
        self.versions_dialog(ui.ctx());
        self.publish_dialog(ui.ctx());
        self.enforce_protection();
        self.read_aloud_frame(ui.ctx(), area);
        if let Some(d) = self.compared.take() {
            shell.cmd = Some(common::AppCmd::OpenDoc(d, false));
        }
        if let Some((d, pdf)) = self.book.open.take() {
            shell.cmd = Some(common::AppCmd::OpenDoc(d, pdf));
        }
        if let Some(cmd) = self.book.shelf.ask.take() {
            shell.cmd = Some(cmd);
        }
        if let Some(s) = self.symbols.show(ui.ctx()) {
            self.insert(&s);
            self.refocus = true;
        }
        // Toolbar and menu clicks take the focus; give it back to the page,
        // a frame later so that windows can focus their own fields first.
        let unfocused = ui.memory(|m| m.focused().is_none()) && self.dialog.is_none() && self.table_dialog.is_none() && !self.symbols.is_open() && self.page_setup.is_none() && self.layout_dialog.is_none() && !self.pics.dialog_open() && !self.shapes.dialog_open() && !self.shapes.editing() && !self.spell_dialog_open() && self.word_count.is_none() && !self.mail_dialog_open() && self.styles.dlg.is_none() && self.refs.dialog.is_none() && self.comments.user_dialog.is_none() && !self.equation_dialog_open() && self.protect.dlg.is_none() && !self.book.confirm && self.book.go_to.is_none() && self.book.goals.is_none();
        if std::mem::take(&mut self.refocus) && unfocused {
            self.focus = true;
        }
        if ui.input(|i| i.pointer.any_released()) && unfocused {
            self.refocus = true;
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        // In a split window, the pane the caret is in.
        let active = self.views.drawing == self.views.pane;
        if active && let Some(fit) = self.fit.take() {
            self.zoom = self.fit_zoom(fit, ui.available_size());
        }
        self.update_layout(ui);
        self.refresh_comments();
        let bg = theme::workspace_bg(ui.visuals());
        ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
        let avail_w = ui.available_width();
        // Comment balloons go in a margin right of the pages.
        let margin = self.comment_margin();
        let row_w = self.layout.row_w.max(self.layout.page.x);
        let tw = self.typewriter();
        // A typewriter can bring the last line half way up too.
        let room = if tw { self.viewport_h * 0.5 } else { 0.0 };
        // At least the view's height: a short note takes the pointer down to
        // the bottom (shapes are drawn and moved below its text).
        let total = vec2((row_w + 2.0 * PAGE_GAP + margin).max(avail_w), (self.layout.total_height() + room).max(ui.available_height()));
        if active {
            self.viewport_h = ui.available_height();
            self.viewport_w = avail_w;
        }
        let scroll_to_caret = active && std::mem::take(&mut self.scroll_to_caret);
        egui::ScrollArea::both().id_salt(("doc_scroll", self.uid, self.views.drawing)).auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
            let (rect, _) = ui.allocate_exact_size(total, Sense::hover());
            let id = self.canvas_id();
            let resp = ui.interact(rect, id, Sense::click_and_drag());
            if active {
                self.canvas_origin = rect.min;
            }
            self.page_x = ((total.x - row_w - margin) / 2.0).max(PAGE_GAP);
            ui.memory_mut(|m| m.set_focus_lock_filter(id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false }));
            if active && std::mem::take(&mut self.focus) {
                resp.request_focus();
            }
            if active && crate::widgets::a11y_on(ui.ctx()) {
                crate::widgets::a11y_canvas(ui.ctx(), &resp, tr("a11y.document"));
                crate::widgets::announce(ui.ctx(), &self.a11y_text());
            }
            if let Some(h) = resp.hover_pos() {
                let icon = self.shape_cursor(h - rect.min.to_vec2()).or_else(|| self.picture_cursor(h - rect.min.to_vec2()));
                ui.ctx().set_cursor_icon(if self.brush.is_some() { egui::CursorIcon::Copy } else { icon.unwrap_or(egui::CursorIcon::Text) });
            }
            self.mouse(ui, &resp, rect.min);
            self.hover_link = None;
            if let Some(h) = resp.hover_pos()
                && self.brush.is_none()
                && let Some((_, target, tip)) = self.link_under(h - rect.min.to_vec2())
            {
                let page = self.is_page_link(&target);
                if page || ui.input(|i| i.modifiers.command) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if page {
                    self.hover_link = Some(target);
                } else {
                    // The document's tip, then where the link really goes.
                    let what = match tip.as_deref().map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty() && **t != *target) {
                        Some(t) => format!("{t}\n{target}"),
                        None => target.to_string(),
                    };
                    resp.clone().on_hover_text_at_pointer(format!("{what}\n{}", tr("link.follow_hint")));
                }
            } else if let Some(h) = resp.hover_pos().filter(|_| self.brush.is_none()) {
                self.hover_card(ui.ctx(), Some(h - rect.min.to_vec2()), h);
            } else {
                self.hover_card(ui.ctx(), None, Pos2::ZERO);
            }
            self.poll_clipboard_picture();
            // Nothing under an open dialog takes keys.
            let focused = resp.has_focus() && ui.memory(|m| m.allows_interaction(ui.layer_id()));
            if active && ui.memory(|m| m.allows_interaction(ui.layer_id())) {
                self.brush_escape(ui);
                // A pen picked from a menu (its color, its weight) leaves
                // the focus there: Escape still lets it go.
                if !focused && self.ink.active() && !egui::Popup::is_any_open(ui.ctx()) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                    self.ink.set_mode(None);
                }
            }
            if focused || self.paste_expected {
                self.keyboard(ui);
            }
            // Also after an entry of the context menu (which has the focus).
            self.update_layout(ui);
            if active {
                self.views.top_page = self.layout.pages_between(viewport.min.y + viewport.height() * 0.3, viewport.min.y + viewport.height() * 0.3).0;
            }
            // Read mode turns pages: the page goes to the top of the view.
            let to_page = if active { self.views.scroll_to_page.take() } else { None };
            if let Some(pg) = to_page {
                let top = self.layout.geo_of(pg).top - PAGE_GAP;
                ui.scroll_to_rect(Rect::from_min_size(pos2(rect.min.x, rect.min.y + top), vec2(1.0, 1.0)), Some(egui::Align::TOP));
                self.scroll_to_caret = false;
            }
            let caret = self.caret_rect(self.sel.cursor);
            let tw_moved = tw && active && self.book.tw_y != Some(caret.center().y);
            if to_page.is_none() && (scroll_to_caret || active && self.scroll_to_caret || tw_moved) {
                self.scroll_to_caret = false;
                if tw {
                    self.book.tw_y = Some(caret.center().y);
                    ui.scroll_to_rect(caret.translate(rect.min.to_vec2()), Some(egui::Align::Center));
                } else {
                    ui.scroll_to_rect(caret.translate(rect.min.to_vec2()).expand2(vec2(40.0, 30.0)), None);
                }
            }
            self.find_hits();
            self.paint(ui, rect.min, viewport, focused);
            self.shape_editor(ui, rect.min);
            self.paste_options_button(ui, rect.min);
            self.minibar_ui(ui);
            self.comment_balloons(ui, rect.min, viewport);
            self.caret_screen = focused.then(|| caret.translate(rect.min.to_vec2()));
            if focused {
                let screen_caret = caret.translate(rect.min.to_vec2());
                ui.output_mut(|o| {
                    o.ime = Some(egui::output::IMEOutput { purpose: Default::default(), rect: screen_caret, cursor_rect: screen_caret, should_interrupt_composition: false });
                });
            }
        });
    }

    fn mouse(&mut self, ui: &egui::Ui, resp: &egui::Response, origin: Pos2) {
        if self.ink.active() && self.ink_mouse(ui, resp, origin) {
            return;
        }
        // Dragging a table column edge.
        if let Some(d) = &mut self.col_drag {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
            if ui.input(|i| i.pointer.primary_down()) {
                if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                    d.x = p.x - origin.x;
                }
                ui.ctx().request_repaint();
                return;
            }
            if let Some(d) = self.col_drag.take() {
                self.finish_col_drag(d);
            }
            return;
        }
        if let Some(h) = resp.hover_pos()
            && !self.dragging
            && let Some((table, edge)) = self.edge_at(h - origin.to_vec2())
            // Pictures and shapes over the edge (and their handles) win.
            && !self.shape_grab_at(h - origin.to_vec2())
            && !self.picture_grab_at(h - origin.to_vec2())
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
            if ui.input(|i| i.pointer.primary_pressed()) {
                resp.request_focus();
                let x = h.x - origin.x;
                self.col_drag = Some(table_ui::ColDrag { table, edge, start: x, x });
                return;
            }
        }
        if self.text_drag_mouse(ui, origin) {
            return;
        }
        if self.dragging && !ui.input(|i| i.pointer.primary_down()) {
            self.dragging = false;
            // The format painter applies when the mouse is released.
            if self.brush.is_some() {
                self.apply_brush();
            } else if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                self.show_minibar(p, false);
            }
        }
        let Some(p) = resp.interact_pointer_pos().or_else(|| (self.pics_dragging() || self.shapes.busy()).then(|| ui.input(|i| i.pointer.interact_pos())).flatten()) else {
            // The menu has to be shown every frame while it's open.
            self.show_context_menu(resp);
            return;
        };
        let local = p - origin.to_vec2();
        if self.brush.is_none() && self.shape_mouse(ui, resp, local) {
            self.show_context_menu(resp);
            return;
        }
        // Double click on a picture: its properties.
        if resp.double_clicked() && self.picture_at_point(local) && self.picture_props_selected() {
            return;
        }
        if self.brush.is_none() && !resp.secondary_clicked() && self.picture_mouse(ui, resp, local) {
            self.show_context_menu(resp);
            return;
        }
        let shift = ui.input(|i| i.modifiers.shift);
        if resp.triple_clicked() {
            let para = self.pos_at(local).para;
            let len = self.doc.paragraphs[para].len();
            self.set_sel(Selection { anchor: Pos::new(para, 0), cursor: Pos::new(para, len) });
            return;
        }
        if resp.double_clicked() && (self.note_double_click(local) || self.equation_double_click(local)) {
            return;
        }
        // Double click in a page's top or bottom margin: its header or
        // footer; out of the one being edited: back to the body.
        if resp.double_clicked() {
            if self.hf_edit.is_some() {
                if !self.in_edited_hf(local) {
                    self.end_hf_edit();
                    return;
                }
            } else if let Some((page, header)) = self.hf_zone_at(local) {
                self.start_hf_edit(page, header);
                return;
            }
        }
        if resp.double_clicked() {
            let (a, b) = self.doc.word_at(self.pos_at(local));
            self.set_sel(Selection { anchor: a, cursor: b });
            return;
        }
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        if pressed && resp.hovered() && !shift {
            if let Some((_, target, _)) = self.link_under(local).filter(|l| ui.input(|i| i.modifiers.command) || self.is_page_link(&l.1)) {
                resp.request_focus();
                self.follow_link(&target);
                return;
            }
        }
        if pressed && resp.hovered() && !shift && let Some(n) = self.note_at_point(local) {
            self.toggle_notes_pane(Some(n));
            return;
        }
        if pressed && resp.hovered() && !shift && self.control_click(local) {
            resp.request_focus();
            return;
        }
        if pressed && resp.hovered() && !shift && self.brush.is_none() && self.grabs_selection(self.pos_at(local)) {
            resp.request_focus();
            self.start_text_drag(local);
            return;
        }
        if pressed && resp.hovered() {
            resp.request_focus();
            let pos = self.pos_at(local);
            self.caret(pos, shift);
            self.desired_x = None;
            self.dragging = true;
            self.scroll_to_caret = false;
        } else if self.dragging && ui.input(|i| i.pointer.primary_down()) {
            let pos = self.pos_at(local);
            if pos != self.sel.cursor {
                self.sel.cursor = pos;
                self.pending = None;
            }
            // Autoscroll when dragging past the visible area.
            let clip = ui.clip_rect();
            if p.y > clip.max.y || p.y < clip.min.y {
                ui.scroll_to_rect(Rect::from_center_size(p, vec2(1.0, 1.0)), None);
                ui.ctx().request_repaint();
            }
        }
        if resp.secondary_clicked() {
            self.menu_link = self.link_under(local);
            let pos = self.pos_at(local);
            self.spell_menu_at(pos);
            if self.sel.is_empty() || pos < self.sel.start() || pos > self.sel.end() {
                self.caret(pos, false);
            }
            self.show_minibar(p, true);
        }
        self.show_context_menu(resp);
    }

    /// The context menu; it has to be shown every frame while it's open.
    fn show_context_menu(&mut self, resp: &egui::Response) {
        resp.context_menu(|ui| {
            self.context_menu(ui);
            // Where the menu really is: egui moves it to fit on the screen
            // after laying it out.
            if let Some(r) = ui.ctx().memory(|m| m.area_rect(ui.layer_id().id)) {
                self.minibar_menu_rect(r);
            }
        });
    }

    pub(crate) fn open_symbols(&mut self) {
        self.symbols.open();
    }

    fn keyboard(&mut self, ui: &mut egui::Ui) {
        if self.symbols.is_open() || self.page_setup.is_some() || self.pics.dialog_open() || self.refs.dialog.is_some() || self.control_dlg.is_some() || self.forms.dlg.is_some() || self.lang_dlg.is_some() || self.ruby_dlg.is_some() || self.asian_dlg.is_some() || self.equation_dialog_open() || self.shapes.dialog_open() || self.shapes.editing() {
            return;
        }
        if self.ink_keys(ui) || self.shape_keys(ui) {
            return;
        }
        let (mut events, mods) = ui.input(|i| (i.events.clone(), i.modifiers));
        if self.views.mode == ViewMode::Read {
            events.retain(views::read_mode_event);
        }
        for ev in events {
            self.clamp_scope();
            match ev {
                Event::Text(t) => {
                    // Alt+= opens the equation editor rather than typing.
                    if !t.chars().all(char::is_control) && !(mods.alt && !mods.command && t == "=") {
                        self.insert(&t);
                        self.check_link_prompt(&t);
                        self.check_task_prompt(&t);
                    }
                }
                Event::Ime(egui::ImeEvent::Preedit { text, .. }) => self.preedit = text,
                Event::Ime(egui::ImeEvent::Commit(t)) => {
                    self.preedit.clear();
                    if !t.is_empty() {
                        self.insert(&t);
                    }
                }
                Event::Ime(_) => {}
                // Ctrl+Shift+C copies formatting (the format painter).
                Event::Copy if mods.shift => self.start_brush(false),
                Event::Copy => self.copy(ui.ctx(), false),
                Event::Cut => self.copy(ui.ctx(), true),
                // Ctrl+Alt+Shift+V pastes formatting; Ctrl+Shift+V pastes
                // unformatted text.
                Event::Paste(_) if mods.shift && mods.alt => {
                    self.paste_expected = false;
                    self.apply_brush();
                }
                Event::Paste(t) => {
                    let plain = mods.shift || std::mem::take(&mut self.paste_plain);
                    self.paste_expected = false;
                    self.paste_from(&t, plain, true);
                }
                // Ctrl+V with only a picture on the clipboard sends no paste.
                Event::Key { key: Key::V, pressed: false, modifiers: m, .. } if m.command && !m.shift && !m.alt => self.paste_clipboard_picture(ui.ctx()),
                Event::Key { key, physical_key, pressed: true, modifiers, .. } => {
                    self.key(key, physical_key, modifiers);
                    // Keep the layout current for consecutive navigation keys.
                    self.update_layout(ui);
                }
                _ => {}
            }
        }
    }

    fn key(&mut self, key: Key, physical: Option<Key>, m: Modifiers) {
        let cmd = m.command;
        let shift = m.shift;
        let is = |k: Key| key == k || physical == Some(k);
        if self.review_key(key, m) || self.refs_key(key, m) {
            return;
        }
        if cmd && shift && m.alt && is(Key::S) {
            self.styles.pane = !self.styles.pane;
            return;
        }
        // Shift+F1: Reveal Formatting, as in Word.
        if shift && !cmd && !m.alt && key == Key::F1 {
            self.toggle_reveal();
            return;
        }
        if m.alt && !cmd && is(Key::Equals) {
            self.open_equation();
            return;
        }
        // Arrows follow the lines of vertical text.
        let key = if self.caret_vertical() { tategaki::arrow(key) } else { key };
        match key {
            Key::ArrowLeft => {
                if !shift && !self.sel.is_empty() {
                    self.caret(self.sel.start(), false);
                } else {
                    let p = if cmd { self.doc.prev_word(self.sel.cursor) } else { self.doc.prev_char(self.sel.cursor) };
                    self.caret(p, shift);
                }
                self.desired_x = None;
            }
            Key::ArrowRight => {
                if !shift && !self.sel.is_empty() {
                    self.caret(self.sel.end(), false);
                } else {
                    let p = if cmd { self.doc.next_word(self.sel.cursor) } else { self.doc.next_char(self.sel.cursor) };
                    self.caret(p, shift);
                }
                self.desired_x = None;
            }
            Key::ArrowUp if cmd => {
                let para = if self.sel.cursor.offset == 0 { self.sel.cursor.para.saturating_sub(1) } else { self.sel.cursor.para };
                self.caret(Pos::new(para, 0), shift);
            }
            Key::ArrowDown if cmd => {
                let para = (self.sel.cursor.para + 1).min(self.doc.paragraphs.len() - 1);
                let p = if para == self.sel.cursor.para { Pos::new(para, self.doc.paragraphs[para].len()) } else { Pos::new(para, 0) };
                self.caret(p, shift);
            }
            Key::ArrowUp => self.move_vertical(false, shift),
            Key::ArrowDown => self.move_vertical(true, shift),
            Key::Home if cmd => self.caret(Pos::new(0, 0), shift),
            Key::End if cmd => self.caret(self.doc.end(), shift),
            Key::Home => self.row_edge(false, shift),
            Key::End => self.row_edge(true, shift),
            Key::PageUp => self.page_move(false, shift),
            Key::PageDown => self.page_move(true, shift),
            // Shift+Enter: line break inside the paragraph.
            Key::Enter if shift && !cmd => self.insert(&LINE_BREAK.to_string()),
            Key::Enter if cmd && shift => self.insert_break(apex_doc::BreakKind::Column),
            // One undo step, like the Breaks menu.
            Key::Enter if cmd => self.insert_break(apex_doc::BreakKind::Page),
            Key::Enter => {
                self.clear_block();
                let cur = self.sel.cursor.para;
                let st = self.doc.paragraphs[cur].style;
                // Enter on an empty list item leaves the list.
                if self.sel.is_empty() && st.kind.is_list() && self.doc.paragraphs[cur].is_empty() {
                    if st.level > 0 {
                        self.indent(-1);
                    } else {
                        self.set_kind(ParaKind::Normal);
                    }
                } else {
                    if self.sel.is_empty() {
                        self.autocorrect_after(self.sel.cursor, "");
                    }
                    let attrs = self.pending.take();
                    let p = self.doc.new_paragraph(self.sel);
                    self.sel = Selection::caret(p);
                    self.pending = attrs;
                    if p.offset == 0 && p.para > 0 {
                        let prev = p.para - 1;
                        self.doc.autolink(Pos::new(prev, self.doc.paragraphs[prev].len()));
                    }
                    // Headings are followed by normal text.
                    if matches!(st.kind, ParaKind::Heading(_) | ParaKind::Title) && self.doc.paragraphs[p.para].is_empty() {
                        self.doc.set_para_style(p.para, p.para, self.sel, &|s: &mut ParaStyle| s.kind = ParaKind::Normal);
                    }
                }
                self.scroll_to_caret = true;
            }
            Key::Backspace | Key::Delete if self.clear_block() => self.scroll_to_caret = true,
            Key::Backspace | Key::Delete if self.control_deleting(key == Key::Delete).0 => {}
            Key::Backspace => {
                let emptied = self.control_deleting(false).1;
                let cur = self.sel.cursor;
                let st = self.doc.paragraphs[cur.para].style;
                // Backspace at the start of a list item removes the bullet first.
                if self.sel.is_empty() && cur.offset == 0 && (st.kind.is_list() || st.kind == ParaKind::Quote) {
                    self.set_kind(ParaKind::Normal);
                } else {
                    let p = self.doc.backspace(self.sel, cmd);
                    self.sel = Selection::caret(p);
                    if let Some(c) = emptied {
                        self.refill_control(c);
                    }
                }
                self.pending = None;
                self.scroll_to_caret = true;
            }
            Key::Delete => {
                let emptied = self.control_deleting(true).1;
                let p = self.doc.delete_forward(self.sel, cmd);
                self.sel = Selection::caret(p);
                if let Some(c) = emptied {
                    self.refill_control(c);
                }
                self.scroll_to_caret = true;
            }
            // Protected for forms: Tab goes from field to field.
            Key::Tab if !cmd && self.forms_only() && self.tab_to_field(shift) => {}
            // Tab moves between cells; Ctrl+Tab types a tab in one.
            Key::Tab if self.doc.in_table(self.sel.cursor.para) && !cmd && self.doc.container(self.sel.anchor.para).map(|c| c.0) == self.doc.container(self.sel.cursor.para).map(|c| c.0) => {
                self.tab_in_table(shift);
            }
            Key::Tab if cmd && self.doc.in_table(self.sel.cursor.para) => self.insert("\t"),
            Key::Tab => {
                let kind = self.cur_para().style.kind;
                if kind.is_list() || self.sel.start().para != self.sel.end().para {
                    self.indent(if shift { -1 } else { 1 });
                } else if !shift {
                    self.insert("\t");
                }
            }
            Key::Space if cmd => self.clear_formatting(false),
            Key::A if cmd && !shift => self.sel = Selection { anchor: Pos::new(0, 0), cursor: self.doc.end() },
            Key::B if cmd && shift => self.toggle(|a| a.subscript, |a, v| {
                a.subscript = v;
                a.superscript = false;
            }),
            Key::P if cmd && shift => self.toggle(|a| a.superscript, |a, v| {
                a.superscript = v;
                a.subscript = false;
            }),
            Key::B if cmd => self.toggle(|a| a.bold, |a, v| a.bold = v),
            Key::I if cmd => self.toggle(|a| a.italic, |a, v| a.italic = v),
            Key::U if cmd => self.toggle(|a| a.underline, |a, v| a.underline = v),
            Key::D if cmd && shift => self.toggle(|a| a.underline && a.fx().underline == apex_doc::UnderlineStyle::Double, |a, v| {
                a.underline = v;
                a.set_fx(|f| f.underline = if v { apex_doc::UnderlineStyle::Double } else { apex_doc::UnderlineStyle::Single });
            }),
            Key::W if cmd && shift => self.toggle(|a| a.underline && a.fx().underline == apex_doc::UnderlineStyle::Words, |a, v| {
                a.underline = v;
                a.set_fx(|f| f.underline = if v { apex_doc::UnderlineStyle::Words } else { apex_doc::UnderlineStyle::Single });
            }),
            Key::K if cmd && shift => self.toggle(|a| a.fx().small_caps, |a, v| a.set_fx(|f| {
                f.small_caps = v;
                f.all_caps = false;
            })),
            Key::A if cmd && shift => self.toggle(|a| a.fx().all_caps, |a, v| a.set_fx(|f| {
                f.all_caps = v;
                f.small_caps = false;
            })),
            Key::H if cmd && shift => self.toggle(|a| a.fx().hidden, |a, v| a.set_fx(|f| f.hidden = v)),
            Key::D if cmd => self.open_font_dialog(),
            Key::Z if cmd && shift => self.redo(),
            Key::Z if cmd => self.undo(),
            Key::Y if cmd => self.redo(),
            Key::F if cmd => self.open_find(false),
            Key::H if cmd => self.open_find(true),
            Key::G if cmd => self.open_goto(),
            Key::K if cmd => self.open_link_dialog(None),
            Key::F5 if cmd && shift => self.open_bookmark_dialog(),
            Key::F5 if !cmd && !shift => self.open_goto(),
            Key::E if cmd => self.set_align(Align::Center),
            Key::L if cmd && shift => self.set_kind(ParaKind::Bullet),
            Key::L if cmd => self.set_align(Align::Left),
            Key::R if cmd => self.set_align(Align::Right),
            Key::J if cmd => self.set_align(Align::Justify),
            Key::M if cmd => self.indent(if shift { -1 } else { 1 }),
            Key::Num0 if cmd && m.alt => self.set_kind(ParaKind::Normal),
            Key::Num1 if cmd && m.alt => self.set_kind(ParaKind::Heading(1)),
            Key::Num2 if cmd && m.alt => self.set_kind(ParaKind::Heading(2)),
            Key::Num3 if cmd && m.alt => self.set_kind(ParaKind::Heading(3)),
            Key::Num1 if cmd && !shift => self.set_line_spacing(1.0),
            Key::Num2 if cmd && !shift => self.set_line_spacing(2.0),
            Key::Num5 if cmd && !shift => self.set_line_spacing(1.5),
            _ if cmd && shift && is(Key::Num8) => {
                self.show_marks = !self.show_marks;
                self.layout.key.0 = u64::MAX;
            }
            // Ctrl+Shift+> / < (egui has no < or > keys: the physical keys
            // are period and comma), and Ctrl+] / Ctrl+[.
            Key::Period | Key::Colon if cmd && shift => self.grow_font(true),
            Key::Comma | Key::Semicolon if cmd && shift => self.grow_font(false),
            Key::CloseBracket if cmd => self.grow_font(true),
            Key::OpenBracket if cmd => self.grow_font(false),
            Key::F3 if m.alt && !cmd => self.open_save_block(),
            Key::F3 if shift => self.cycle_case(),
            Key::F7 if !cmd && !shift => self.start_spelling(),
            Key::F7 if !cmd && shift => self.open_thesaurus(),
            Key::F3 if !cmd && self.find.is_none() && self.expand_block() => {}
            Key::F3 => {
                self.find_next(false);
            }
            Key::Escape => {
                if self.hf_edit.is_some() {
                    self.end_hf_edit();
                } else if self.brush.is_some() {
                    self.brush = None;
                } else if self.find.is_some() {
                    self.find = None;
                }
            }
            _ => {}
        }
    }

    // --- painting --------------------------------------------------------------------------

    /// Fills the part of a paragraph segment between two char indices
    /// (`to == usize::MAX` runs to the end, past the last row).
    #[allow(clippy::too_many_arguments)]
    fn paint_range(&self, p: &egui::Painter, seg: &Segment, pl: &ParaLayout, from: usize, to: usize, galley_pos: Pos2, color: Color32) {
        let mut start = 0usize;
        for row in &pl.galley.rows {
            let n = row.row.glyphs.len() + usize::from(row.ends_with_newline);
            let end = start + n;
            if row.pos.y + row.row.size.y > seg.gy0 && row.pos.y < seg.gy1 && to >= start && from <= end {
                // Positions come from the row itself: at a row boundary the
                // galley cursor would resolve to the end of the previous row.
                let x_at = |i: usize| row.pos.x + row.row.glyphs.get(i - start).map_or(row.row.size.x, |g| g.pos.x);
                let x0 = x_at(from.max(start));
                let x1 = if to > end || to == usize::MAX { row.pos.x + row.row.size.x + 5.0 * self.zoom } else { x_at(to) };
                let r = Rect::from_min_max(pos2(x0, row.pos.y), pos2(x1.max(x0 + 1.0), row.pos.y + row.row.size.y)).translate(galley_pos.to_vec2());
                p.rect_filled(r, 0.0, color);
            }
            start = end;
        }
    }

    fn paint(&self, ui: &egui::Ui, origin: Pos2, viewport: Rect, focused: bool) {
        let p = ui.painter();
        // The pages go dark with the dark workspace (screen only).
        let dark_from = theme::canvas_mark(p);
        self.paint_pages(ui, origin, viewport, focused);
        theme::darken_since(p, dark_from);
    }

    fn paint_pages(&self, ui: &egui::Ui, origin: Pos2, viewport: Rect, focused: bool) {
        let p = ui.painter();
        let l = &self.layout;
        let off = origin.to_vec2();
        let z = self.zoom;
        let (first_page, last_page) = l.pages_between(viewport.min.y, viewport.max.y);
        let draft = self.views.mode == ViewMode::Draft;
        // A note is in Draft but keeps its shapes, word art and drawings:
        // it has no other view to show them in.
        let floats = !draft || self.embedded;
        let shadow = Color32::from_black_alpha(40);
        let hf_on = self.hf_edit.is_none() && self.doc.has_headers();
        for page in first_page..=last_page {
            let g = l.geo_of(page);
            let size = self.hf_page_size().unwrap_or(g.size);
            let r = Rect::from_min_size(pos2(self.page_x + g.dx, g.top), size).translate(off);
            if draft {
                // Only the text; a dotted line where a page ends.
                if self.embedded {
                    let bottom = if page == last_page { r.max.y.max(viewport.max.y + off.y) } else { r.max.y };
                    p.rect_filled(Rect::from_x_y_ranges(r.x_range(), r.min.y..=bottom), 0.0, Paper::BG);
                    continue;
                }
                p.rect_filled(r, 0.0, Paper::BG);
                if page > 0 {
                    p.extend(egui::Shape::dashed_line(&[r.left_top(), r.right_top()], Stroke::new(1.0, Color32::from_gray(150)), 4.0, 3.0));
                }
                continue;
            }
            p.rect_filled(r.translate(vec2(0.0, 2.0)).expand(1.0), 2.0, shadow);
            p.rect_filled(r, 0.0, Paper::BG);
            self.paint_hf_backdrop(p, ui.ctx(), r);
            self.paint_page_extras(p, l, page, r, z);
            if hf_on {
                self.paint_page_hf(p, ui.ctx(), l, page, r, z, None);
            }
        }
        if floats {
            self.paint_screen_floats(p, first_page, last_page, off, true);
        }
        // Page by page: vertical pages are painted as laid out and turned.
        for page in first_page..=last_page {
            let turn = self.turn_of(page).map(|t| t.offset(off));
            let from = tategaki::mark(p);
            let bp = if turn.is_some() { tategaki::unclipped(p) } else { p.clone() };
            self.paint_body(&bp, page, off);
            if let Some(t) = turn {
                tategaki::turn_painted(p, from, &t, p.clip_rect());
            }
        }
        if floats {
            self.paint_screen_floats(p, first_page, last_page, off, false);
        }
        if let Some(d) = &self.col_drag {
            // The column edge being dragged.
            p.vline(d.x + off.x, viewport.translate(off).y_range(), Stroke::new(1.0, Color32::from_rgb(0x2B, 0x57, 0x9A)));
        }
        if self.show_marks {
            let segs: Vec<Segment> = l.page_segs(first_page, last_page).copied().collect();
            self.paint_bookmarks(p, &segs, off);
        }
        self.paint_range_brackets(p, off);
        self.paint_control_box(p, off);
        self.paint_picture_selection(p, off);
        self.paint_shape_selection(p, off);
        self.paint_ink_live(p, off);
        self.paint_text_drop(p, off);
        // Caret and IME preedit (none while a picture or shape is selected).
        if focused && self.selected_picture().is_none() && self.selected_shape().is_none() {
            let caret = self.caret_rect(self.sel.cursor).translate(off);
            if !self.preedit.is_empty() {
                let font = egui::FontId::proportional(caret.height().max(caret.width()) * 0.8);
                let g = p.layout_no_wrap(self.preedit.clone(), font, Paper::TEXT);
                let r = Rect::from_min_size(caret.min, g.size());
                p.rect_filled(r, 0.0, Paper::BG);
                p.galley(caret.min, g, Paper::TEXT);
                p.hline(r.x_range(), r.max.y, Stroke::new(1.0, Paper::TEXT));
            } else {
                let t = ui.input(|i| i.time);
                let blink = (t * 1.6).fract() < 0.6 || self.dragging;
                if blink {
                    let stroke = Stroke::new(1.5 * z.max(1.0), Paper::TEXT);
                    // Across the line on a vertical page.
                    if caret.width() > caret.height() {
                        p.hline(caret.x_range(), caret.min.y.round() + 0.5, stroke);
                    } else {
                        p.vline(caret.min.x.round() + 0.5, caret.y_range(), stroke);
                    }
                }
                ui.ctx().request_repaint_after(web_time::Duration::from_millis(300));
            }
        }
    }

    /// The text, tables, notes and marks on page `page` (the canvas is at
    /// `off` on the screen), with the selection and the find matches.
    fn paint_body(&self, p: &egui::Painter, page: usize, off: Vec2) {
        let l = &self.layout;
        let z = self.zoom;
        self.paint_notes(p, l, page, self.content_origin(page) + off, z);
        let (sa, sb) = (self.sel.start(), self.sel.end());
        let hits: &[(Pos, Pos)] = self.find.as_ref().map_or(&[], |f| &f.hits);
        let spoken = self.spoken();
        let marks = Color32::from_rgb(0x5A, 0x7F, 0xB0);
        let origin = |page: usize| self.content_origin(page) + off;
        let frags = l.frags_on(page, page);
        let block = self.doc.block_selection(self.sel);
        self.paint_table_back(p, l, frags, &origin, block.as_ref());
        for seg in l.ghosts_on(page, page) {
            let o = self.seg_origin(seg) + off;
            let (_, clip_rect) = seg_geometry(l, seg, &l.paras[seg.para], o);
            let clip = p.with_clip_rect(clip_rect.intersect(p.clip_rect()));
            self.paint_seg_back(&clip, p, l, seg, o, z);
            self.paint_seg_text(&clip, p, l, seg, o, z);
        }
        for seg in l.page_segs(page, page) {
            if seg.hidden {
                continue;
            }
            let pl = &l.paras[seg.para];
            let para = &self.doc.paragraphs[seg.para];
            let o = self.seg_origin(seg) + off;
            let (galley_pos, clip_rect) = seg_geometry(l, seg, pl, o);
            let clip = p.with_clip_rect(clip_rect.intersect(p.clip_rect()));
            self.paint_seg_back(&clip, p, l, seg, o, z);
            // Find matches.
            let hi = hits.partition_point(|(h, _)| h.para < seg.para);
            for (ha, hb) in hits[hi..].iter().take_while(|(h, _)| h.para == seg.para) {
                let (from, to) = (char_count(&para.text, ha.offset), if hb.para == seg.para { char_count(&para.text, hb.offset) } else { usize::MAX });
                self.paint_range(&clip, seg, pl, from, to, galley_pos, Color32::from_rgba_unmultiplied(255, 200, 0, 90));
            }
            // Read Aloud: the paragraph and the word being read.
            if let Some(s) = spoken.as_ref().filter(|s| s.para == seg.para) {
                let ch = |b: usize| char_count(&para.text, b);
                self.paint_range(&clip, seg, pl, ch(s.range.0), ch(s.range.1), galley_pos, Color32::from_rgba_unmultiplied(80, 150, 255, 40));
                if let Some((a, b)) = s.word {
                    self.paint_range(&clip, seg, pl, ch(a), ch(b), galley_pos, Color32::from_rgba_unmultiplied(80, 150, 255, 110));
                }
            }
            self.paint_comment_ranges(&clip, seg, pl, galley_pos);
            self.paint_editable_ranges(&clip, seg, pl, galley_pos);
            self.paint_change_bar(p, seg, o);
            // Selection (whole cells when it spans cells of a table).
            let in_block = block.as_ref().is_some_and(|b| (b.start..b.end).contains(&seg.para));
            if seg.para >= sa.para && seg.para <= sb.para && !self.sel.is_empty() && !in_block {
                let from = if seg.para == sa.para { char_count(&para.text, sa.offset) } else { 0 };
                let to = if seg.para == sb.para { char_count(&para.text, sb.offset) } else { usize::MAX };
                self.paint_range(&clip, seg, pl, from, to, galley_pos, Paper::TEXT_SELECTION);
            }
            self.paint_seg_text(&clip, p, l, seg, o, z);
            self.paint_spelling(&clip, seg, pl, galley_pos);
            if self.show_marks {
                self.paint_marks(&clip, seg, pl, galley_pos, marks);
                self.paint_break_mark(p, seg, pl, galley_pos, o, marks);
            }
        }
        self.paint_table_borders(p, l, frags, &origin, z, true);
    }

    /// Paragraph backgrounds of a segment: shading, code boxes, quote bars.
    fn paint_seg_back(&self, p: &egui::Painter, full: &egui::Painter, l: &Layout, seg: &Segment, o: Pos2, z: f32) {
        let pl = &l.paras[seg.para];
        let para = &self.doc.paragraphs[seg.para];
        let top = o.y + seg.y;
        let h = seg.gy1 - seg.gy0;
        let seg_rect = Rect::from_min_size(pos2(o.x, top), vec2(seg.w, h));
        // Borders reach out of the segment (above and below its rows).
        let boxed = self.paint_para_box(full, l, seg, o, z);
        if let Some(c) = para.style.shading.filter(|_| !boxed) {
            let pad = 2.0 * z;
            p.rect_filled(Rect::from_min_max(pos2(o.x + pl.indent - pad, top), pos2(o.x + pl.indent + pl.width + pad, top + h)), 0.0, theme::rgb(c));
        }
        match para.style.kind {
            ParaKind::Code => {
                p.rect_filled(seg_rect, 0.0, Color32::from_rgb(0xF3, 0xF4, 0xF6));
            }
            ParaKind::Quote => {
                let x = o.x + pl.indent - 12.0 * PT * z;
                p.vline(x, seg_rect.y_range(), Stroke::new(3.0 * z, Color32::from_gray(200)));
            }
            _ => {}
        }
    }

    /// The text of a segment (clipped to it) and its list marker.
    fn paint_seg_text(&self, clip: &egui::Painter, p: &egui::Painter, l: &Layout, seg: &Segment, o: Pos2, z: f32) {
        let pl = &l.paras[seg.para];
        let para = &self.doc.paragraphs[seg.para];
        let (galley_pos, _) = seg_geometry(l, seg, pl, o);
        let top = o.y + seg.y;
        if pl.rotate != 0 {
            // Turned cell text: the galley's top edge goes along the
            // cell's right edge (downwards) or left edge (upwards).
            let g = pl.galley.size();
            let x = o.x + pl.indent;
            let (at, angle) = if pl.rotate > 0 { (pos2(x + g.y, top), std::f32::consts::FRAC_PI_2) } else { (pos2(x, top + g.x), -std::f32::consts::FRAC_PI_2) };
            crate::text_fx::paint_turned(p, at, angle, &pl.fx);
            p.add(egui::epaint::TextShape::new(at, pl.galley.clone(), Paper::TEXT).with_angle(angle));
            // Guides and marks turn with the text.
            let rot = egui::emath::Rot2::from_angle(angle);
            for ov in &pl.overlays {
                p.add(egui::epaint::TextShape::new(at + rot * ov.pos.to_vec2(), ov.galley.clone(), Paper::TEXT).with_angle(angle + ov.angle));
            }
            return;
        }
        if !pl.fx.is_empty() {
            crate::text_fx::paint(clip, galley_pos, &pl.fx);
        }
        clip.galley(galley_pos, pl.galley.clone(), Paper::TEXT);
        for ov in &pl.overlays {
            if ov.row_top >= seg.gy0 - 0.5 && ov.row_top < seg.gy1 - 0.5 {
                clip.add(egui::epaint::TextShape::new(galley_pos + ov.pos.to_vec2(), ov.galley.clone(), Paper::TEXT).with_angle(ov.angle));
            }
        }
        if para.style.toc.is_some() || apex_doc::para_has_objects(para) {
            self.paint_objects(clip, para, Some(seg.para), pl, galley_pos, (seg.gy0, seg.gy1), &l.refs.values, z);
        }
        if !pl.tab_fills.is_empty() {
            self.paint_tab_fills(clip, pl, seg, galley_pos);
        }
        if para.text.contains(apex_doc::SOFT_HYPHEN) {
            decor::paint_soft_hyphens(clip, pl, seg, galley_pos);
        }
        if para.text.contains([apex_doc::CHECKED, apex_doc::UNCHECKED]) {
            self.paint_check_boxes(clip, para, pl, seg, galley_pos);
            self.paint_task_label(p, para, pl, seg, galley_pos);
        }
        self.paint_images(p, pl, seg, galley_pos);
        self.paint_equations(clip, pl, seg, galley_pos);
        if !pl.shapes.is_empty() {
            self.paint_flow_shapes(p, pl, seg, galley_pos, z);
        }
        // List markers on the first row.
        let marker = if seg.gy0 == 0.0 && (para.style.kind.is_list() || matches!(para.style.kind, ParaKind::Heading(_))) { self.marker(seg.para) } else { None };
        if let Some(marker) = marker {
            let st = para.style;
            let a = para.runs.first().map(|r| r.attrs.clone()).unwrap_or_default();
            let plain = Attrs { bold: false, italic: false, underline: false, strike: false, superscript: false, subscript: false, image: None, ..a };
            let fmt = self.text_format(&ParaStyle { line_spacing: 0.0, line_rule: LineRule::Multiple, ..st }, &plain, z);
            let row_h = pl.galley.rows.first().map_or(pl.galley.size().y, |r| r.row.size.y);
            let x = o.x + pl.indent - 6.0 * z;
            let cy = top + row_h / 2.0;
            // A list number starts at the hanging indent, as in Word (and
            // past it when the first line is indented past the list's room).
            let numbered = st.kind == ParaKind::Numbered;
            let hang_x = o.x + pl.indent + (st.first_line - LIST_INDENT_PT / 2.0) * PT * z;
            // Labels sit on the first row's baseline, however tall it is.
            let label_at = |label: String, x: f32, right: bool| {
                let g = p.layout_no_wrap(label, fmt.font_id.clone(), fmt.color);
                // A number too wide for the hanging room ends before the text.
                let right = right || (numbered && x + g.size().x > o.x + pl.indent - 2.0 * z && st.first_line < LIST_INDENT_PT / 2.0);
                let x = if right && numbered { o.x + pl.indent - 6.0 * z } else { x };
                let base = pl.galley.rows.first().and_then(|r| r.row.glyphs.first().map(|q| galley_pos.y + r.pos.y + q.pos.y));
                let own = g.rows.first().and_then(|r| r.row.glyphs.first().map(|q| r.pos.y + q.pos.y));
                let y = match (base, own) {
                    (Some(b), Some(o)) => b - o,
                    _ => cy - g.size().y / 2.0,
                };
                let x = if right { x - g.size().x } else { x };
                p.galley(pos2(x, y), g, fmt.color);
            };
            match marker {
                apex_doc::Marker::Text(label) if numbered => label_at(label, hang_x, false),
                apex_doc::Marker::Text(label) => label_at(label, x, true),
                apex_doc::Marker::Bullet(c) => {
                    let r = fmt.font_id.size * 0.16;
                    // Beside the middle of the lowercase letters, however
                    // tall the line is.
                    let base = pl.galley.rows.first().and_then(|r| r.row.glyphs.first().map(|g| galley_pos.y + r.pos.y + g.pos.y));
                    let cy = base.map_or(cy, |b| b - fmt.font_id.size * 0.28);
                    // Where Word puts it: at the hanging indent.
                    let hang = (LIST_INDENT_PT / 2.0 - st.first_line).clamp(6.0, LIST_INDENT_PT / 2.0) * PT * z;
                    paint_bullet(p, pos2(o.x + pl.indent - hang + r, cy), r, c, fmt.color, z);
                }
            }
        }
    }

    /// Bookmark brackets, shown with the formatting marks like other editors.
    fn paint_bookmarks(&self, p: &egui::Painter, segs: &[Segment], off: Vec2) {
        let color = Color32::from_gray(140);
        let stroke = Stroke::new(1.0 * self.zoom.max(1.0), color);
        let mut last = usize::MAX;
        for seg in segs {
            if seg.para == last {
                continue;
            }
            last = seg.para;
            for m in &self.doc.paragraphs[seg.para].marks {
                if m.name.starts_with('_') || apex_doc::is_comment_mark(&m.name) || apex_doc::is_perm_mark(&m.name) {
                    continue;
                }
                let r = self.caret_rect(self.doc.clamp(Pos::new(seg.para, m.offset))).translate(off);
                let (x, w) = (r.min.x.round() + 0.5, 3.0 * self.zoom.max(1.0));
                let dx = if m.end { -w } else { w };
                p.vline(x, r.y_range(), stroke);
                p.hline(x..=x + dx, r.min.y, stroke);
                p.hline(x..=x + dx, r.max.y, stroke);
            }
        }
    }

    /// Formatting marks: · for spaces, → for tabs, ↵ for line breaks and ¶
    /// at the end of the paragraph (the arrows are drawn: fonts may lack them).
    fn paint_marks(&self, p: &egui::Painter, seg: &Segment, pl: &ParaLayout, galley_pos: Pos2, color: Color32) {
        let z = self.zoom;
        let stroke = Stroke::new(1.0 * z.max(1.0), color);
        let last = pl.galley.rows.len().saturating_sub(1);
        for (ri, row) in pl.galley.rows.iter().enumerate() {
            if row.pos.y + row.row.size.y <= seg.gy0 || row.pos.y >= seg.gy1 {
                continue;
            }
            let rp = galley_pos + row.pos.to_vec2();
            let mid = rp.y + row.row.size.y / 2.0;
            for g in &row.row.glyphs {
                let x = rp.x + g.pos.x;
                match g.chr {
                    ' ' | '\u{a0}' => {
                        p.circle_filled(pos2(x + g.advance_width / 2.0, mid), 1.2 * z, color);
                    }
                    '\t' => {
                        let (a, b) = (pos2(x + 2.0 * z, mid), pos2(x + g.advance_width.max(8.0 * z) - 2.0 * z, mid));
                        p.line_segment([a, b], stroke);
                        p.line_segment([b, b + vec2(-3.0, -3.0) * z], stroke);
                        p.line_segment([b, b + vec2(-3.0, 3.0) * z], stroke);
                    }
                    _ => {}
                }
            }
            let end_x = rp.x + row.row.size.x + 2.0 * z;
            if row.ends_with_newline {
                // ↵ for a soft line break.
                let s = 3.5 * z;
                let (a, b, c) = (pos2(end_x + 2.0 * s, mid - s), pos2(end_x + 2.0 * s, mid), pos2(end_x + 0.3 * s, mid));
                p.line_segment([a, b], stroke);
                p.line_segment([b, c], stroke);
                p.line_segment([c, c + vec2(1.2, -1.2) * s], stroke);
                p.line_segment([c, c + vec2(1.2, 1.2) * s], stroke);
            }
            if ri == last {
                let size = (row.row.size.y * 0.7).max(6.0);
                p.text(pos2(end_x, mid), egui::Align2::LEFT_CENTER, "¶", egui::FontId::proportional(size), color);
            }
        }
    }
}

/// Where a segment's galley is drawn and the rect its text is clipped to,
/// for a page whose content area starts at `o`.
fn seg_geometry(_: &Layout, seg: &Segment, pl: &ParaLayout, o: Pos2) -> (Pos2, Rect) {
    let top = o.y + seg.y;
    let h = seg.gy1 - seg.gy0;
    let galley_pos = pos2(o.x + pl.indent + pl.align_off, top - seg.gy0);
    if seg.cell {
        // Cell text stays in its cell (with room for list markers).
        return (galley_pos, Rect::from_min_max(pos2(o.x + pl.indent - 30.0, top), pos2(o.x + pl.indent + pl.width + 4.0, top + h)));
    }
    // Room for list markers, drop caps in the margin and the ¶ mark.
    (galley_pos, Rect::from_min_max(pos2(o.x - 120.0, top), pos2(o.x + seg.w + 60.0, top + h)))
}

/// Drops the space between neighbour paragraphs of the same style
/// where they ask for it (`ParaStyle::contextual_spacing`).
fn contextual_spacing(doc: &Document, paras: &mut [ParaLayout], pt: f32) {
    let ps = &doc.paragraphs;
    if !ps.iter().any(|p| p.style.contextual_spacing) {
        return;
    }
    // List items all stand for Word's List Paragraph style.
    let key = |p: &Paragraph| {
        let list = p.style.kind.is_list() || doc.styles.custom(p.style.custom).is_some_and(|d| d.id.eq_ignore_ascii_case("ListParagraph") || d.name.eq_ignore_ascii_case("List Paragraph"));
        if list { (true, ParaKind::Normal, 0) } else { (false, p.style.kind, p.style.custom) }
    };
    for i in 1..ps.len().min(paras.len()) {
        let (a, b) = (&ps[i - 1], &ps[i]);
        if !(a.style.contextual_spacing || b.style.contextual_spacing) || !same_place(a.cell.as_ref(), b.cell.as_ref()) || key(a) != key(b) {
            continue;
        }
        // (Cells only take explicit spacing; pictures beside the text
        // keep the room they add below it.)
        let own = |p: &Paragraph| if p.cell.is_some() { (p.style.space_before.unwrap_or(0.0), p.style.space_after.unwrap_or(0.0)) } else { p.style.spacing() };
        if a.style.contextual_spacing {
            paras[i - 1].space_after = (paras[i - 1].space_after - own(a).1 * pt).max(0.0);
        }
        if b.style.contextual_spacing {
            paras[i].space_before = 0.0;
        }
    }
}

/// Whether two paragraphs sit in the same cell, or both outside tables.
fn same_place(a: Option<&apex_doc::InCell>, b: Option<&apex_doc::InCell>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => (x.table, x.row, x.col) == (y.table, y.row, y.col) && same_place(x.nested.as_deref(), y.nested.as_deref()),
        _ => false,
    }
}

/// Where the file asks for it (`Document::collapse_spacing`), the space
/// between two paragraphs is the larger of the first's after and the
/// second's before: the second's before gives up what the first's after
/// already leaves.
fn collapse_spacing(doc: &Document, paras: &mut [ParaLayout], pt: f32) {
    if !doc.collapse_spacing {
        return;
    }
    let ps = &doc.paragraphs;
    for i in 1..ps.len().min(paras.len()) {
        let (a, b) = (&ps[i - 1], &ps[i]);
        if !same_place(a.cell.as_ref(), b.cell.as_ref()) {
            continue;
        }
        let after = if a.cell.is_some() { a.style.space_after.unwrap_or(0.0) } else { a.style.spacing().1 };
        let after = paras[i - 1].space_after.min(after * pt);
        paras[i].space_before = (paras[i].space_before - after).max(0.0);
    }
}

/// Empty paragraphs after a picture with text beside it sit beside the
/// picture, as in Word, instead of below it: the room the picture adds
/// below its paragraph shrinks by their height.
fn empty_beside_pictures(doc: &Document, paras: &mut [ParaLayout]) {
    let ps = &doc.paragraphs;
    let n = ps.len().min(paras.len());
    for i in 0..n {
        let l = &paras[i];
        let side = l.images.iter().filter(|p| p.img.wrap == apex_doc::Wrap::Square).map(|p| p.rect.max.y).fold(0.0, f32::max).max(shapes_ui::side_height(&l.shapes));
        let mut over = (side - l.galley.size().y).min(l.space_after);
        if over <= 0.0 {
            continue;
        }
        let cell = |p: &Paragraph| p.cell.as_ref().map(|c| c.key());
        // (Outside tables, `relayout` puts them beside.)
        if cell(&ps[i]).is_none() {
            continue;
        }
        let mut taken = 0.0;
        for j in i + 1..n {
            let q = &ps[j];
            let blank = q.text.is_empty() && !q.style.kind.is_list() && q.style.shading.is_none() && q.style.borders.is_empty() && !q.style.page_break_before && cell(q) == cell(&ps[i]);
            if over <= 0.0 || !blank {
                break;
            }
            let h = (paras[j].space_before + paras[j].galley.size().y + paras[j].space_after).min(over);
            over -= h;
            taken += h;
        }
        paras[i].space_after -= taken;
    }
}

/// Draws a list bullet (shapes, so they never depend on font glyphs).
fn paint_bullet(p: &egui::Painter, c: Pos2, r: f32, ch: char, color: Color32, z: f32) {
    let stroke = Stroke::new((1.0 * z).max(1.0), color);
    match ch {
        '◦' => {
            p.circle_stroke(c, r * 0.9, stroke);
        }
        '▪' | '■' => {
            p.rect_filled(Rect::from_center_size(c, vec2(r * 1.7, r * 1.7)), 0.0, color);
        }
        '–' => {
            p.line_segment([c - vec2(r * 1.4, 0.0), c + vec2(r * 1.4, 0.0)], Stroke::new(r * 0.5, color));
        }
        '✓' => {
            let s = r * 1.3;
            p.line_segment([c + vec2(-s, 0.0), c + vec2(-s * 0.3, s * 0.8)], Stroke::new(r * 0.45, color));
            p.line_segment([c + vec2(-s * 0.3, s * 0.8), c + vec2(s * 1.1, -s)], Stroke::new(r * 0.45, color));
        }
        '➢' => {
            let s = r * 1.4;
            p.add(egui::Shape::convex_polygon(vec![c + vec2(-s, -s), c + vec2(s, 0.0), c + vec2(-s, s), c + vec2(-s * 0.4, 0.0)], color, Stroke::NONE));
        }
        _ => {
            p.circle_filled(c, r, color);
        }
    }
}

/// Change case modes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Upper,
    Lower,
    Sentence,
    Title,
    Toggle,
}

/// Converts `text` (paragraphs joined by '\n') and returns one string per
/// char, skipping the paragraph separators, so runs keep their formatting.
fn case_pieces(text: &str, case: Case) -> Vec<String> {
    let mut out = Vec::with_capacity(text.len());
    let mut word_start = true;
    let mut sentence_start = true;
    for c in text.chars() {
        if c == '\n' {
            word_start = true;
            sentence_start = true;
            continue;
        }
        let s: String = match case {
            Case::Upper => c.to_uppercase().collect(),
            Case::Lower => c.to_lowercase().collect(),
            Case::Toggle if c.is_uppercase() => c.to_lowercase().collect(),
            Case::Toggle => c.to_uppercase().collect(),
            Case::Title if word_start => c.to_uppercase().collect(),
            Case::Title => c.to_lowercase().collect(),
            Case::Sentence if sentence_start && c.is_alphabetic() => c.to_uppercase().collect(),
            Case::Sentence => c.to_lowercase().collect(),
        };
        out.push(s);
        if c.is_alphanumeric() {
            sentence_start = false;
        }
        if matches!(c, '.' | '!' | '?' | LINE_BREAK) {
            sentence_start = true;
        }
        word_start = !c.is_alphanumeric() && c != '\'' && c != '’';
    }
    out
}

const KINDS: [ParaKind; 15] = [
    ParaKind::Normal,
    ParaKind::Title,
    ParaKind::Heading(1),
    ParaKind::Heading(2),
    ParaKind::Heading(3),
    ParaKind::Heading(4),
    ParaKind::Heading(5),
    ParaKind::Heading(6),
    ParaKind::Heading(7),
    ParaKind::Heading(8),
    ParaKind::Heading(9),
    ParaKind::Quote,
    ParaKind::Code,
    ParaKind::Bullet,
    ParaKind::Numbered,
];

/// The word around byte `at` of `text`, or the letter there.
fn word_at(text: &str, at: usize) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '\'' || c == '’';
    let start = text[..at].char_indices().rev().take_while(|&(_, c)| is_word(c)).last().map_or(at, |(i, _)| i);
    let end = text[at..].char_indices().find(|&(_, c)| !is_word(c)).map_or(text.len(), |(i, _)| at + i);
    if start < end { text[start..end].to_string() } else { text[at..].chars().next().map(String::from).unwrap_or_default() }
}

fn kind_label(k: ParaKind) -> &'static str {
    match k {
        ParaKind::Normal => tr("kind.normal"),
        ParaKind::Title => tr("kind.title"),
        ParaKind::Heading(1) => tr("kind.h1"),
        ParaKind::Heading(2) => tr("kind.h2"),
        ParaKind::Heading(3) => tr("kind.h3"),
        ParaKind::Heading(4) => tr("kind.h4"),
        ParaKind::Heading(5) => tr("kind.h5"),
        ParaKind::Heading(6) => tr("kind.h6"),
        ParaKind::Heading(7) => tr("kind.h7"),
        ParaKind::Heading(8) => tr("kind.h8"),
        ParaKind::Heading(_) => tr("kind.h9"),
        ParaKind::Quote => tr("kind.quote"),
        ParaKind::Code => tr("kind.code"),
        ParaKind::Bullet => tr("kind.bullet"),
        ParaKind::Numbered => tr("kind.numbered"),
    }
}

fn char_count(s: &str, byte: usize) -> usize {
    s[..s.floor_char_boundary(byte)].chars().count()
}

fn char_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

fn prev_boundary(s: &str, off: usize) -> usize {
    s[..s.floor_char_boundary(off)].char_indices().next_back().map_or(0, |(b, _)| b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A laid out document with fonts installed.
    fn laid_out(doc: Document) -> (DocView, egui::Context) {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        let frame = || ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        frame();
        frame();
        let mut v = DocView::new(doc);
        v.layout = v.build_layout(&ctx, 1.0, (1, 0, 0, 0, 0), Layout::default());
        (v, ctx)
    }

    #[test]
    fn shapes_merge_in_one_step() {
        use apex_doc::{DocShape, Shape, ShapeKind};
        let mut doc = Document::from_paragraphs(vec![Paragraph::new("Text")]);
        for x in [0.0, 50.0] {
            let s = DocShape { x, y: 0.0, ..DocShape::new(Shape::new(ShapeKind::Rect), 100.0, 100.0) };
            doc.insert_shape(Selection::caret(Pos::new(0, 0)), s);
        }
        let (mut v, ctx) = laid_out(doc);
        let at: Vec<Pos> = v.doc.shapes().iter().map(|s| s.0).collect();
        v.select_shape(at[0]);
        v.toggle_picked(at[1]);
        assert!(v.can_merge());
        v.merge_picked(apex_slides::combine::Merge::Union);
        let shapes = v.doc.shapes();
        assert_eq!(shapes.len(), 1);
        let s = &shapes[0].1;
        assert!(s.shape.custom.is_some());
        assert!((s.width - 150.0).abs() < 0.5 && (s.height - 100.0).abs() < 0.5, "{} {}", s.width, s.height);
        // Laid out again, the drawn shape stands where the two were.
        v.layout = v.build_layout(&ctx, 1.0, (1, 0, 0, 0, 0), Layout::default());
        let b = v.all_boxes();
        assert!((b[0].rect.width() - 150.0 * PT).abs() < 1.0, "{:?}", b[0].rect);
        // One undo brings both back.
        v.doc.undo();
        assert_eq!(v.doc.shapes().len(), 2);
    }

    #[test]
    fn guides_marks_and_fitted_text() {
        let text = "漢字です。重要な氏名";
        let (plain, _ctx) = laid_out(Document::from_paragraphs(vec![Paragraph::new(text)]));
        let mut doc = Document::from_paragraphs(vec![Paragraph::new(text)]);
        let sel = |a: usize, b: usize| Selection { anchor: Pos::new(0, a * 3), cursor: Pos::new(0, b * 3) };
        // A guide raised well over the text.
        doc.set_ruby(sel(0, 2), Some(apex_doc::Ruby { text: "かんじ".into(), size: Some(10), raise: Some(40), ..Default::default() }));
        doc.format_range(sel(5, 7), &|a| a.set_asian(|x| x.emphasis = Some(apex_doc::Emphasis::Dot)));
        doc.format_range(sel(8, 10), &|a| a.set_asian(|x| x.fit_text = Some((1260, 1))));
        let (v, _ctx) = laid_out(doc);
        let (g0, g1) = (&plain.layout.paras[0].galley, &v.layout.paras[0].galley);
        assert!(g1.size().y > g0.size().y + 5.0, "{} {}", g0.size().y, g1.size().y);
        // The guide and a mark per character.
        assert_eq!(v.layout.paras[0].overlays.len(), 3);
        // The fitted text is 63 points wide: first to last glyph.
        let x = |c: usize| g1.pos_from_cursor(egui::text::CCursor::new(c)).min.x;
        let w = x(10) - x(8);
        assert!((w - 63.0 * PT).abs() < 1.0, "{w}");
    }

    // Bold clicked twice at the caret leaves nothing pending, bold text
    // or not.
    #[test]
    fn a_format_toggled_twice_leaves_nothing_pending() {
        let mut p = Paragraph::new("plain ");
        p.push("bold", Attrs { bold: true, ..Default::default() });
        let mut v = DocView::new(Document::from_paragraphs(vec![p]));
        for at in [3, 10] {
            v.sel = Selection::caret(Pos::new(0, at));
            v.toggle(|a| a.bold, |a, on| a.bold = on);
            assert!(v.pending.is_some());
            v.toggle(|a| a.bold, |a, on| a.bold = on);
            assert_eq!(v.pending, None, "at {at}");
        }
    }

    // Ctrl+Enter (and Insert > Page break) is one undo step.
    #[test]
    fn a_page_break_undoes_in_one_step() {
        let paras = vec![Paragraph::new("one"), Paragraph::new("two three"), Paragraph::new("four")];
        let mut v = DocView::new(Document::from_paragraphs(paras.clone()));
        v.sel = Selection::caret(Pos::new(1, 3));
        v.key(Key::Enter, None, Modifiers::COMMAND);
        assert_eq!(v.doc.paragraphs.len(), 4);
        assert!(v.doc.paragraphs[2].style.page_break_before);
        v.undo();
        let show = |ps: &[Paragraph]| ps.iter().map(|p| (p.text.clone(), p.style.page_break_before)).collect::<Vec<_>>();
        assert_eq!(show(&v.doc.paragraphs), show(&paras));
        // In the code view (nothing laid out) too, like the other breaks.
        v.toggle_md_source();
        v.key(Key::Enter, None, Modifiers::COMMAND);
        assert!(v.doc.paragraphs[2].style.page_break_before);
    }

    // Commands in the code view (nothing laid out) and positions inside
    // a letter or past a changed text don't stop the program.
    #[test]
    fn stale_layout_and_positions_are_safe() {
        let (mut v, _ctx) = laid_out(Document::from_paragraphs(vec![Paragraph::new("ñandú Ω")]));
        v.toggle_md_source();
        v.sel = Selection::caret(Pos::new(0, 1));
        v.insert_date_time(true, true);
        v.doc.replace_body(vec![Paragraph::new("a"), Paragraph::new("b"), Paragraph::new("\u{FFFC}c")]);
        assert_eq!(v.caret_at(Pos::new(2, 1)).0, Rect::NOTHING);
        assert!(v.layout.segs_of(7).is_empty());
        assert_eq!((char_count("ñ", 1), prev_boundary("ñ", 9)), (0, 0));
    }

    #[test]
    fn charts_go_in_with_their_data() {
        let mut v = DocView::new(Document::from_paragraphs(vec![Paragraph::new("Sales")]));
        v.sel = Selection::caret(Pos::new(0, 5));
        let pie = crate::sheet_view::CHART_TYPES.iter().position(|t| t.0 == "chart.t.pie").unwrap();
        v.insert_chart_object(pie);
        let objs = v.doc.objects();
        assert_eq!(objs.len(), 1);
        let obj = objs[0].1.object.clone().unwrap();
        let (chart, data, _) = apex_io::chart_object::chart_of(&obj.chart().unwrap().book).unwrap();
        assert_eq!(chart.kind, apex_sheet::ChartType::Pie);
        assert_eq!(data.series[0].values.len(), 4);
        // Its data opens in a tab to be typed over.
        assert!(matches!(v.take_object_request(), Some(crate::objects::Request::Open(o)) if o.id == obj.id));
        // One undo step.
        v.doc.undo();
        assert!(v.doc.objects().is_empty());
        // A chart copied from a sheet comes back as a chart with its data.
        let book = apex_io::chart_object::book(&apex_io::chart_object::sample(apex_sheet::ChartType::Bar, "C", "S"));
        let img = eframe::egui::ColorImage::new([4, 3], vec![eframe::egui::Color32::RED; 12]);
        let png = crate::charts::encode_png(&img).unwrap();
        let rgba: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
        crate::charts::set_chart_clip(Some((crate::images::pixel_hash(&rgba), book.clone(), (300.0, 180.0))));
        let (got, size) = crate::charts::chart_clip_for(&png).unwrap();
        assert!(Arc::ptr_eq(&got, &book) && size == (300.0, 180.0));
        crate::charts::set_chart_clip(None);
    }

    #[test]
    fn screen_readers_hear_the_caret() {
        let mut v = DocView::new(Document::from_paragraphs(vec![Paragraph::new("Hello big world"), Paragraph::new(""), Paragraph::new("Second")]));
        let mut at = |para: usize, offset: usize| {
            v.sel = Selection::caret(Pos::new(para, offset));
            v.a11y_text()
        };
        // A new paragraph is read whole; a blank one says so.
        assert_eq!(at(0, 0), "Hello big world");
        assert_eq!(at(1, 0), tr("a11y.blank_line"));
        assert_eq!(at(2, 0), "Second");
        // A step over a letter says it, even twice the same; a jump the word.
        assert_eq!(at(2, 1), "S");
        assert_eq!(at(0, 7), "Hello big world");
        assert_eq!(at(0, 8), "i");
        assert_eq!(at(0, 11), "world");
        v.doc.paragraphs[0].text = "Hello big wworld".into();
        assert_eq!(v.a11y_text(), "world", "typing says nothing new");
        v.sel = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 5) };
        assert_eq!(v.a11y_text(), format!("Hello {}", tr("a11y.selected")));
        assert_eq!(word_at("a  b", 2), " ");
    }

    #[test]
    fn guide_without_raise_sits_as_in_word() {
        // odt has no raise: the guide goes where Word puts it by default
        // (raised to 1 pt below the 12 pt base's size), and the line is
        // as tall.
        let height = |raise: Option<i32>| {
            let mut doc = Document::from_paragraphs(vec![Paragraph::new("漢字です")]);
            let sel = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 6) };
            doc.set_ruby(sel, Some(apex_doc::Ruby { text: "かんじ".into(), size: Some(10), raise, ..Default::default() }));
            let (v, _ctx) = laid_out(doc);
            v.layout.paras[0].galley.size().y
        };
        // The first contexts of the process load the Japanese fallback
        // font while they lay out, and tests running alongside may still
        // be loading it: compare once the heights settle.
        let mut h = (0.0, 0.0);
        for _ in 0..10 {
            h = (height(None), height(Some(22)));
            if (h.0 - h.1).abs() < 0.01 {
                break;
            }
        }
        assert!((h.0 - h.1).abs() < 0.01, "{} {}", h.0, h.1);
    }

    #[test]
    fn table_layout() {
        let rows: Vec<String> = (0..80).map(|i| format!("| a{i} | {} | c |", if i % 9 == 0 { "long text ".repeat(12) } else { "b".into() })).collect();
        let md = format!("Intro\n\n| H1 | H2 | H3 |\n|---|---|---|\n{}\n\nEnd\n", rows.join("\n"));
        let (v, _ctx) = laid_out(apex_io::markdown::read(&md));
        let l = &v.layout;
        let n = v.doc.paragraphs.len();
        assert!(l.pages >= 2);
        // Every paragraph has segments, contiguous and in order.
        for i in 0..n {
            let s = l.segs_of(i);
            assert!(!s.is_empty() && s.iter().all(|s| s.para == i), "para {i}");
        }
        assert_eq!(l.page_start.len(), l.pages + 1);
        assert_eq!(*l.page_start.last().unwrap(), l.segs.len());
        for p in 0..l.pages {
            assert!(l.page_segs(p, p).all(|s| s.page == p));
            assert!(l.page_segs(p, p).count() > 0, "page {p} empty");
        }
        assert!(l.frags.windows(2).all(|w| w[0].page <= w[1].page));
        // The header row repeats on the next pages.
        assert!(l.frags.iter().any(|f| f.ghost && f.page == 1 && f.row == 0));
        assert!(l.ghosts.iter().all(|g| g.page >= 1));
        // Rows are inside the page.
        assert!(l.frags.iter().all(|f| f.y >= 0.0 && f.y + f.h <= l.content.y + 0.5));
        // A click on a cell lands in it; Down goes to the cell below.
        let tb = &l.tables[0];
        let (a, _) = tb.map.cell(3, 1);
        let r = v.caret_rect(Pos::new(a, 0));
        assert_eq!(v.pos_at(r.center()).para, a);
        let x = r.center().x - v.content_origin(0).x;
        assert_eq!(v.vertical_neighbor(a, true, x), Some(tb.map.cell(4, 1).0));
        assert_eq!(v.vertical_neighbor(a, false, x), Some(tb.map.cell(2, 1).0));
        assert_eq!(v.vertical_neighbor(tb.map.start - 1, true, x), Some(tb.map.cell(0, 1).0));
        assert_eq!(v.vertical_neighbor(tb.map.end, false, x), Some(tb.map.cell(tb.map.rows - 1, 1).1 - 1));
    }

    #[test]
    fn widows_and_keep_lines() {
        let long = "palabra ".repeat(120);
        let mut split = 0;
        for k in 20..60 {
            for keep in [false, true] {
                let mut paras: Vec<Paragraph> = (0..k).map(|i| Paragraph::new(format!("línea {i}"))).collect();
                let mut p = Paragraph::new(long.clone());
                p.style.keep_lines = keep;
                paras.push(p);
                let (v, _ctx) = laid_out(Document::from_paragraphs(paras));
                let segs = v.layout.segs_of(k);
                let g = &v.layout.paras[k].galley;
                let rows = |a: f32, b: f32| g.rows.iter().filter(|r| r.pos.y >= a - 0.1 && r.pos.y < b - 0.1).count();
                if keep {
                    assert_eq!(segs.len(), 1, "k {k}");
                } else if segs.len() > 1 {
                    split += 1;
                    for s in segs {
                        assert!(rows(s.gy0, s.gy1) >= 2, "k {k}: {} lines", rows(s.gy0, s.gy1));
                    }
                }
            }
        }
        assert!(split > 5);
    }

    #[test]
    fn floating_picture_leaves_the_text() {
        let png: Arc<[u8]> = Arc::from(&b"png"[..]);
        let mut p = Paragraph::new("");
        p.push(&apex_doc::OBJECT.to_string(), Attrs { image: Some(Arc::new(apex_doc::Image::new(png, 150.0, 100.0))), ..Default::default() });
        p.push(" after", Attrs::default());
        let (mut v, ctx) = laid_out(Document::from_paragraphs(vec![p]));
        let x0 = v.layout.paras[0].galley.pos_from_cursor(egui::text::CCursor::new(2)).min.x;
        assert!(x0 > 100.0, "{x0}");
        assert!(v.doc.float_image(Pos::new(0, 0), true, None));
        let old = std::mem::take(&mut v.layout);
        v.layout = v.build_layout(&ctx, 1.0, (2, 0, 0, 0, 0), old);
        let x1 = v.layout.paras[0].galley.pos_from_cursor(egui::text::CCursor::new(2)).min.x;
        assert!(x1 < 20.0, "{x1}");
    }

    #[test]
    fn short_paragraphs_sit_beside_a_floating_picture() {
        let png: Arc<[u8]> = Arc::from(&b"png"[..]);
        let mut img = apex_doc::Image::new(png, 150.0, 200.0);
        img.wrap = apex_doc::Wrap::Square;
        let mut p = Paragraph::new("");
        p.push(&apex_doc::OBJECT.to_string(), Attrs { image: Some(Arc::new(img)), ..Default::default() });
        p.push("Where:", Attrs::default());
        let long = "word ".repeat(400);
        let (mut v, ctx) = laid_out(Document::from_paragraphs(vec![p, Paragraph::new("Beside"), Paragraph::new(&long)]));
        for _ in 0..2 {
            let l = &v.layout;
            let (s1, s2) = (l.segs_of(1)[0], l.segs_of(2)[0]);
            assert!(l.paras[1].indent > 100.0 && s1.y < 100.0, "{} {}", l.paras[1].indent, s1.y);
            assert!(l.paras[2].indent < 20.0 && s2.y >= 200.0, "{} {}", l.paras[2].indent, s2.y);
            // Laid out again from the cache, the same.
            let old = std::mem::take(&mut v.layout);
            v.layout = v.build_layout(&ctx, 1.0, old.key, old);
        }
    }

    #[test]
    fn tall_row_splits() {
        let tall = "word ".repeat(3000);
        let md = format!("| A | B |\n|---|---|\n| {tall} | x |\n| y | z |\n");
        let (v, _ctx) = laid_out(apex_io::markdown::read(&md));
        let l = &v.layout;
        assert!(l.pages >= 2);
        let row1: Vec<_> = l.frags.iter().filter(|f| f.row == 1 && !f.ghost).collect();
        assert!(row1.len() >= 2);
        assert!(row1[0].top && !row1[0].bottom && row1.last().unwrap().bottom);
        for i in 0..v.doc.paragraphs.len() {
            assert!(!l.segs_of(i).is_empty());
        }
        assert!(l.segs.iter().all(|s| s.y + (s.gy1 - s.gy0) <= l.content.y + 1.0));
    }

    #[test]
    fn header_rows_go_with_the_first_row() {
        // Whatever text comes before, the repeated header row never ends a
        // page alone.
        for n in (30..60).step_by(3) {
            let md = format!("{}| A | B |\n|---|---|\n| {} | x |\n| y | z |\n", "line\n\n".repeat(n), "word ".repeat(20));
            let mut doc = apex_io::markdown::read(&md);
            let at = doc.paragraphs.iter().position(|p| p.cell.is_some()).unwrap();
            doc.set_table_props(Selection::caret(Pos::new(at, 0)), &|t| t.header_rows = 1);
            let (v, _ctx) = laid_out(doc);
            let page = |r: u32| v.layout.frags.iter().find(|f| f.row == r && !f.ghost).unwrap().page;
            assert_eq!(page(0), page(1), "{n}");
        }
    }

    #[test]
    fn floating_shapes_leave_their_line_as_tall_as_an_empty_one() {
        let mut p = Paragraph::new("");
        let mut shape = apex_doc::DocShape::new(apex_doc::Shape::new(apex_doc::ShapeKind::TextBox), 100.0, 80.0);
        shape.wrap = apex_doc::ShapeWrap::InFront;
        p.push(&apex_doc::OBJECT.to_string(), Attrs { size: Some(50), shape: Some(Arc::new(shape)), ..Default::default() });
        let (v, _ctx) = laid_out(Document::from_paragraphs(vec![p, Paragraph::new("")]));
        let h = |i: usize| v.layout.paras[i].galley.rect.height();
        assert!((h(0) - h(1)).abs() < 0.5, "{} vs {}", h(0), h(1));
    }

    #[test]
    fn field_is_one_char() {
        let mut doc = Document::from_paragraphs(vec![Paragraph::new("Page  of")]);
        doc.insert_field(Selection::caret(Pos::new(0, 5)), apex_doc::Field::new(apex_doc::FieldKind::Page));
        let (v, _ctx) = laid_out(doc);
        let g = &v.layout.paras[0].galley;
        assert_eq!(g.job.text.chars().count(), v.doc.paragraphs[0].text.chars().count());
    }

    #[test]
    fn protection_takes_back_edits() {
        let (mut v, _ctx) = laid_out(Document::from_paragraphs(vec![Paragraph::new("Hola")]));
        v.insert("antes ");
        v.enforce_protection();
        v.doc.protection = Some(apex_doc::Protection { limit: apex_doc::EditLimit::ReadOnly, enforced: true, password: None });
        v.enforce_protection();
        v.insert("x");
        v.enforce_protection();
        assert_eq!(v.doc.paragraphs[0].text, "antes Hola");
        // Undoing what came before is a change too.
        v.doc.undo();
        v.enforce_protection();
        assert_eq!(v.doc.paragraphs[0].text, "antes Hola");
        // Comments only: text edits go, comments stay.
        v.doc.protection = Some(apex_doc::Protection { limit: apex_doc::EditLimit::Comments, enforced: true, password: None });
        v.insert("y");
        v.enforce_protection();
        assert_eq!(v.doc.paragraphs[0].text, "antes Hola");
        // Tracked changes: edits stay, tracked.
        v.doc.protection = Some(apex_doc::Protection { limit: apex_doc::EditLimit::TrackedChanges, enforced: true, password: None });
        v.enforce_protection();
        assert!(v.doc.track_changes);
        v.insert("z");
        v.enforce_protection();
        assert!(v.doc.paragraphs[0].text.contains('z'));
    }

    #[test]
    fn protection_keeps_editable_ranges() {
        let (mut v, _ctx) = laid_out(Document::from_paragraphs(vec![Paragraph::new("Nombre: Ana")]));
        v.doc.add_editable_range(Selection { anchor: Pos::new(0, 8), cursor: Pos::new(0, 11) });
        v.doc.protection = Some(apex_doc::Protection { limit: apex_doc::EditLimit::ReadOnly, enforced: true, password: None });
        v.enforce_protection();
        assert_eq!(v.protect.ranges.len(), 1);
        // Typing at the end of the range goes on inside it.
        v.sel = Selection::caret(Pos::new(0, 11));
        v.insert(" Pérez");
        v.enforce_protection();
        assert_eq!(v.doc.paragraphs[0].text, "Nombre: Ana Pérez");
        v.sel = Selection::caret(Pos::new(0, 0));
        v.insert("x");
        v.enforce_protection();
        assert_eq!(v.doc.paragraphs[0].text, "Nombre: Ana Pérez");
        assert_eq!(v.protect.ranges, vec![(Pos::new(0, 8), Pos::new(0, 18))]);
    }

    #[test]
    fn turned_cells_and_margins() {
        let mut doc = apex_io::markdown::read("| Una cabecera larga | B |\n|---|---|\n| x | y |\n");
        let a = doc.paragraphs.iter().position(|p| p.text == "Una cabecera larga").unwrap();
        let (v, _ctx) = laid_out(doc.clone());
        let row0 = |v: &DocView| v.layout.frags.iter().find(|f| f.row == 0 && !f.ghost).unwrap().h;
        let flat = row0(&v);
        let x0 = v.layout.tables[0].text_box(&v.doc.paragraphs, a, PT).0;
        doc.set_cell_props(Selection::caret(Pos::new(a, 0)), &|c, _| c.text_dir = apex_doc::TextDir::Up);
        doc.set_table_props(Selection::caret(Pos::new(a, 0)), &|p| p.cell_margins = Some([10.0, 20.0, 10.0, 20.0]));
        let (v, _ctx) = laid_out(doc);
        // The row is as tall as the turned text is long, plus the margins.
        let g = v.layout.paras[a].galley.size();
        assert_eq!(v.layout.paras[a].rotate, -1);
        assert!(row0(&v) >= g.x + 20.0 * PT - 0.5 && row0(&v) > flat * 2.0);
        assert!(v.layout.tables[0].text_box(&v.doc.paragraphs, a, PT).0 > x0 + 10.0 * PT);
    }

    #[test]
    fn nested_table_layout() {
        let mut doc = apex_io::markdown::read("| A | B |\n|---|---|\n| x | y |\n\nEnd\n");
        let y = doc.paragraphs.iter().position(|p| p.text == "y").unwrap();
        doc.insert_table(Selection::caret(Pos::new(y, 0)), 60, 2, 200.0);
        let (v, _ctx) = laid_out(doc);
        let l = &v.layout;
        assert_eq!(l.tables.len(), 2);
        assert_eq!(l.tables[0].kids, vec![1]);
        let (outer, inner) = (&l.tables[0], &l.tables[1]);
        // The inner table sits in the outer one's cell.
        let (cx, cw) = outer.text_box(&v.doc.paragraphs, inner.map.start, PT);
        assert!(inner.x[0] >= cx - 0.5 && inner.x[inner.x.len() - 1] <= cx + cw + 0.5);
        // Its rows go to the next page, below the outer row's fill.
        assert!(l.pages >= 2);
        for f in l.frags.iter().filter(|f| f.table == 1) {
            let parent = l.frags.iter().position(|o| o.table == 0 && o.page == f.page && !o.ghost).unwrap();
            let me = l.frags.iter().position(|o| std::ptr::eq(o, f)).unwrap();
            assert!(parent < me);
        }
        assert_eq!(l.frags.iter().filter(|f| f.table == 1).count(), 60);
        for i in 0..v.doc.paragraphs.len() {
            assert!(!l.segs_of(i).is_empty(), "para {i}");
        }
        assert!(l.segs.iter().all(|s| s.y + (s.gy1 - s.gy0) <= l.content.y + 1.0));
        // Clicks land in the inner cells; Down moves through its rows.
        let (a, _) = inner.map.cell(3, 1);
        assert_eq!(l.table_at(a).map(|t| t.map.depth), Some(1));
        let r = v.caret_rect(Pos::new(a, 0));
        assert_eq!(v.pos_at(r.center()).para, a);
        let x = r.center().x - v.content_origin(0).x;
        assert_eq!(v.vertical_neighbor(a, true, x), Some(inner.map.cell(4, 1).0));
    }

    #[test]
    fn case_conversion() {
        let conv = |t: &str, c| case_pieces(t, c).concat();
        assert_eq!(conv("hello world. bye", Case::Upper), "HELLO WORLD. BYE");
        assert_eq!(conv("HELLO World", Case::Lower), "hello world");
        assert_eq!(conv("hello wORLD. bye now", Case::Sentence), "Hello world. Bye now");
        assert_eq!(conv("el niño's día", Case::Title), "El Niño's Día");
        assert_eq!(conv("Hello", Case::Toggle), "hELLO");
        // Paragraph separators are skipped: one piece per char.
        assert_eq!(case_pieces("ab\ncd", Case::Title).len(), 4);
    }
}
