//! The presentation editor: slide thumbnails on the left, the current
//! slide on the canvas and its speaker notes below, with the toolstrip's
//! tabs. The model and its undo history are `apex_slides`; painting is
//! in [`render`] and [`text`], editing objects in [`objects`],
//! [`canvas`], [`typing`] and [`format`], the other views in [`views`].

use std::sync::Arc;

use apex_slides::{Deck, Notes, Presentation, TextBody, edit};
use eframe::egui::{self, Color32, Id, Key, Modifiers, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};

use crate::app::Notice;
use crate::common::{self, Shell};
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::toolstrip::{self, TabDef};
use crate::shapes::PX_PT;
use crate::theme;

mod brush;
pub mod canvas;
#[cfg_attr(not(feature = "empresa"), path = "slides_view/corp_tpl_free.rs")]
pub mod corp_tpl;
pub mod fill_dlg;
pub mod format;
pub mod handouts;
pub mod media;
mod merge;
pub mod diagram_edit;
pub mod morph;
pub mod objects;
pub mod presenter;
mod select_pane;
pub mod render;
pub mod show;
pub mod show_ink;
mod tabs;
pub mod text;
pub mod typing;
pub mod views;
pub mod warp;


/// Width of the thumbnails pane at first, screen points.
const THUMBS_W: f32 = 190.0;

/// Undo steps typing in the notes joins (per slide).
const NOTES_KEY: u64 = 0x4E0_7E5;

/// Arrow keys move objects this far (points); with Ctrl or Alt, a point.
const NUDGE: f32 = 6.0;

/// What the editor shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Thumbnails, the slide and its notes.
    Normal,
    /// The slides' text as an outline, beside the slide.
    Outline,
    /// All slides in a grid, to order them.
    Sorter,
    /// The masters and their layouts.
    Master,
}

pub struct SlidesView {
    pub pres: Presentation,
    /// The slide shown on the canvas.
    pub cur: usize,
    /// Canvas zoom (1.0: a point is 96/72 pixels).
    pub zoom: f32,
    /// The canvas zoom follows the window.
    pub fit: bool,
    pub notices: Vec<Notice>,
    pub read_only: bool,
    pub show_notes: bool,
    /// The notes being typed: slide id and text.
    notes_draft: Option<(u32, String)>,
    uid: u64,
    /// The selected objects (ids on the canvas's slide, layout or master).
    pub sel: Vec<u32>,
    /// The text being typed in.
    pub typing: Option<typing::Typing>,
    drag: Option<canvas::Drag>,
    /// What a click or drag on the canvas inserts.
    pub tool: Option<canvas::Tool>,
    pub mode: Mode,
    /// In the master view: the master and layout (none: the master).
    pub master_at: (usize, Option<usize>),
    /// The equation dialog: the object edited (none: a new one) and its text.
    /// The equation dialog: the equation edited (none: a new one), its
    /// text and where its buttons insert.
    pub equation: Option<(Option<u32>, String, usize, bool)>,
    /// Insert > Symbol.
    pub symbols: crate::symbols::SymbolDialog,
    /// Insert > Library.
    pub library: crate::library::Dialog,
    /// The text color button's last color.
    pub last_color: Option<u32>,
    sorter_zoom: f32,
    sorter_drag: Option<usize>,
    object_request: Option<crate::objects::Request>,
    /// The show running, if any.
    pub show: Option<show::Show>,
    /// The animation picked in the Animations tab and pane.
    pub anim_sel: Option<usize>,
    /// The animation pane shows.
    pub anim_pane: bool,
    /// Pen marks of the show that ended, waiting for keep or discard.
    ink_keep: Option<std::collections::HashMap<usize, Vec<show_ink::Mark>>>,
    /// The pointer the next show starts with.
    pub pointer: show_ink::Pointer,
    /// Videos and sounds playing, and the trim dialog.
    pub media: media::Playing,
    /// The Diagram text pane.
    pub diagram: diagram_edit::Pane,
    /// The Selection Pane shows, and its state.
    pub pane_open: bool,
    pane: crate::selection_pane::PaneState,
    /// The fill dialog (shapes or background).
    pub fill_dlg: Option<fill_dlg::FillDlg>,
    pub corp_dlg: Option<corp_tpl::CorpDlg>,
    /// The format painter, when on.
    brush: Option<brush::Brush>,
    /// Which shared pick-up `brush` is (see `painter::sync`).
    brush_serial: u64,
    /// Where the canvas showed the slide last.
    canvas_xf: Option<render::Xf>,
}

static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl SlidesView {
    /// The tool armed (what dragging on the slide does), for the QA.
    #[cfg(test)]
    pub fn audit_tool(&self) -> String {
        format!("{:?}", self.tool)
    }

    /// What decides what the next action does without showing in the
    /// accessibility tree (the tool in use, the show's pointer), for the
    /// button audit: a click that changes only this has done something.
    #[cfg(test)]
    pub fn audit_modes(&self) -> String {
        format!("{:?} {:?} {} {} {:?} {:?}", self.tool, self.pointer, self.pres.revision, self.cur, self.sel, self.anim_sel)
    }

    pub fn new(deck: Deck) -> SlidesView {
        let mut deck = deck;
        if deck.slides.is_empty() {
            edit::add_slide(&mut deck, 0, 0, 0);
        }
        SlidesView {
            pres: Presentation::new(deck),
            cur: 0,
            zoom: 1.0,
            fit: true,
            notices: Vec::new(),
            read_only: false,
            show_notes: true,
            notes_draft: None,
            uid: NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            sel: Vec::new(),
            typing: None,
            drag: None,
            tool: None,
            mode: Mode::Normal,
            master_at: (0, None),
            equation: None,
            symbols: Default::default(),
            library: Default::default(),
            last_color: Some(0xC00000),
            sorter_zoom: 1.0,
            sorter_drag: None,
            object_request: None,
            show: None,
            anim_sel: None,
            anim_pane: false,
            ink_keep: None,
            pointer: show_ink::Pointer::Arrow,
            media: media::Playing::default(),
            diagram: diagram_edit::Pane::default(),
            pane_open: false,
            pane: Default::default(),
            fill_dlg: None,
            corp_dlg: None,
            brush: None,
            brush_serial: 0,
            canvas_xf: None,
        }
    }

    pub fn is_modified(&self) -> bool {
        self.pres.modified
    }

    // --- actions -------------------------------------------------------------------------

    fn clamp(&mut self) {
        self.cur = self.cur.min(self.pres.slides.len().saturating_sub(1));
    }

    pub fn go(&mut self, i: usize) {
        self.commit_notes();
        if i != self.cur {
            self.stop_typing();
            self.sel.clear();
            self.drag = None;
        }
        self.cur = i;
        self.clamp();
    }

    /// A slide after the current one with the layout that follows its own.
    pub fn new_slide(&mut self) {
        if self.read_only {
            return;
        }
        self.commit_notes();
        let (m, l) = if self.pres.slides.is_empty() { (0, 0) } else { edit::next_layout(&self.pres, self.cur) };
        self.new_slide_with(m, l);
    }

    pub fn new_slide_with(&mut self, master: usize, layout: usize) {
        if self.read_only {
            return;
        }
        self.commit_notes();
        let at = if self.pres.slides.is_empty() { 0 } else { self.cur + 1 };
        let i = edit::add_slide(self.pres.edit("slides.undo.new_slide"), at, master, layout);
        self.go(i);
    }

    pub fn duplicate(&mut self) {
        if self.read_only || self.pres.slides.is_empty() {
            return;
        }
        self.commit_notes();
        let cur = self.cur;
        let i = edit::duplicate_slides(self.pres.edit("slides.undo.duplicate"), &[cur]);
        self.go(i);
    }

    pub fn delete(&mut self) {
        // A presentation keeps one slide at least.
        if self.read_only || self.pres.slides.len() < 2 {
            return;
        }
        self.notes_draft = None;
        self.stop_typing();
        self.sel.clear();
        let cur = self.cur;
        edit::delete_slides(self.pres.edit("slides.undo.delete_slide"), &[cur]);
        self.clamp();
    }

    pub fn toggle_hidden(&mut self) {
        if self.read_only || self.pres.slides.is_empty() {
            return;
        }
        let cur = self.cur;
        let s = self.pres.edit("slides.undo.hide").slide_mut(cur);
        s.hidden = !s.hidden;
    }

    /// Moves the current slide one place up (`-1`) or down (`1`).
    pub fn move_by(&mut self, d: isize) {
        let n = self.pres.slides.len();
        let to = self.cur as isize + d;
        if self.read_only || to < 0 || to as usize >= n {
            return;
        }
        self.commit_notes();
        let cur = self.cur;
        // `move_slides` takes the place before which they go.
        let before = if d > 0 { to as usize + 1 } else { to as usize };
        self.cur = edit::move_slides(self.pres.edit("slides.undo.move"), &[cur], before);
    }

    pub fn set_layout(&mut self, master: usize, layout: usize) {
        if self.read_only || self.pres.slides.is_empty() {
            return;
        }
        self.stop_typing();
        let cur = self.cur;
        edit::set_layout(self.pres.edit("slides.undo.layout"), cur, master, layout);
        self.clamp_selection();
    }

    /// After undo or redo: the selection and the caret stay on what is there.
    fn after_history(&mut self) {
        self.clamp();
        self.master_at.0 = self.master_at.0.min(self.pres.masters.len().saturating_sub(1));
        if let Some(l) = self.master_at.1
            && l >= self.pres.masters[self.master_at.0].layouts.len()
        {
            self.master_at.1 = None;
        }
        self.clamp_selection();
        let text = self.typing.as_ref().and_then(|t| self.cur_objects().iter().find(|o| o.id == t.id).map(|o| typing::body_of(o, t.cell).map(|b| b.paras.clone())));
        match text {
            Some(paras) => {
                let mut d = apex_doc::Document::new();
                d.paragraphs = paras.unwrap_or_default();
                if d.paragraphs.is_empty() {
                    d.paragraphs.push(apex_doc::Paragraph::default());
                }
                if let Some(t) = &mut self.typing {
                    t.sel = apex_doc::Selection { anchor: d.clamp(t.sel.anchor), cursor: d.clamp(t.sel.cursor) };
                    t.placed = None;
                }
            }
            None => self.typing = None,
        }
    }

    pub fn undo(&mut self) {
        self.commit_notes();
        self.pres.seal();
        self.pres.undo();
        self.after_history();
    }

    /// File > Inspect File: removes (or shows) what the inspector found
    /// of `what`, as one undo step.
    pub fn inspect_clean(&mut self, what: apex_io::inspect::Hidden, show: bool) -> bool {
        self.commit_notes();
        self.pres.seal();
        let done = apex_io::inspect::clean_deck(&mut self.pres, what, show);
        self.after_history();
        done
    }

    pub fn redo(&mut self) {
        self.commit_notes();
        self.pres.seal();
        self.pres.redo();
        self.after_history();
    }

    /// Sets the notes of slide `i` (typing joins one undo step).
    pub fn set_notes(&mut self, i: usize, text: &str) {
        let Some(s) = self.pres.slides.get(i) else { return };
        if s.notes_text() == text || self.read_only {
            return;
        }
        let key = NOTES_KEY ^ ((s.id as u64) << 20);
        let s = self.pres.edit_merge("slides.undo.notes", key).slide_mut(i);
        match &mut s.notes {
            Some(n) => {
                n.text.set_plain(text);
                n.origin.as_mut().map(|o| o.same = false);
            }
            None => s.notes = Some(Notes { text: TextBody::plain(text), origin: None }),
        }
    }

    /// Writes the notes being typed into the slide.
    fn commit_notes(&mut self) {
        if let Some((id, t)) = self.notes_draft.take()
            && let Some(i) = self.pres.index_of(id)
        {
            self.set_notes(i, &t);
            self.pres.seal();
        }
    }

    /// Before saving or printing: what is being typed goes in.
    pub fn commit_edit(&mut self) {
        self.commit_notes();
        self.stop_typing();
    }

    // --- painting ------------------------------------------------------------------------

    /// The whole view.
    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        self.clamp();
        if self.show.is_some() {
            self.show_ui(ui);
            return;
        }
        self.sync_brush();
        self.keys(ui.ctx(), shell.fullscreen);
        if self.show.is_some() {
            return;
        }
        self.ink_dialog(ui.ctx());
        let panel = egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(6, 6));
        match self.mode {
            Mode::Sorter => {
                egui::CentralPanel::default_margins().frame(egui::Frame::new().fill(theme::workspace_bg(ui.visuals()))).show(ui, |ui| self.sorter(ui));
            }
            mode => {
                let left = Id::new((if mode == Mode::Master { "slides_masters" } else if mode == Mode::Outline { "slides_outline" } else { "slides_thumbs" }, self.uid));
                egui::Panel::left(left).resizable(true).default_size(if mode == Mode::Outline { 300.0 } else { THUMBS_W }).frame(panel).show(ui, |ui| match mode {
                    Mode::Master => self.masters_pane(ui),
                    Mode::Outline => self.outline_pane(ui),
                    _ => self.thumbnails(ui),
                });
                if self.pane_open && mode != Mode::Outline {
                    egui::Panel::right(Id::new(("slides_selpane", self.uid))).resizable(true).default_size(220.0).frame(panel).show(ui, |ui| self.select_pane_ui(ui));
                }
                if self.anim_pane && mode == Mode::Normal {
                    egui::Panel::right(Id::new(("slides_anims", self.uid))).resizable(true).default_size(240.0).frame(panel).show(ui, |ui| self.anim_pane_ui(ui));
                }
                if self.show_notes && mode != Mode::Master {
                    egui::Panel::bottom(Id::new(("slides_notes", self.uid)))
                        .resizable(true)
                        .default_size(110.0)
                        .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 6)))
                        .show(ui, |ui| self.notes_pane(ui));
                }
                egui::CentralPanel::default_margins().frame(egui::Frame::new().fill(theme::workspace_bg(ui.visuals()))).show(ui, |ui| self.canvas(ui));
            }
        }
        self.equation_dialog(ui.ctx());
        self.symbols_dialog(ui.ctx());
        if let Some(p) = self.library.ui(ui.ctx()) {
            self.insert_picture_file(&p);
        }
        self.trim_dialog(ui.ctx());
        self.network_media_dialog(ui.ctx());
        self.fill_dialog(ui.ctx());
        self.company_template_dialog(ui.ctx());
        self.text_pane(ui.ctx());
    }

    fn keys(&mut self, ctx: &egui::Context, fullscreen: bool) {
        // The show: F5 from the start, Shift+F5 from here, Alt+F5 with
        // the presenter view.
        let start = ctx.input_mut(|i| {
            if i.consume_key(Modifiers::SHIFT, Key::F5) {
                Some((self.cur, false))
            } else if i.consume_key(Modifiers::ALT, Key::F5) {
                Some((self.cur, true))
            } else if i.consume_key(Modifiers::NONE, Key::F5) {
                Some((0, false))
            } else {
                None
            }
        });
        if let Some((from, presenter)) = start {
            self.start_show(ctx, from, presenter, fullscreen);
            return;
        }
        // A dialog takes the keys (Escape closes it).
        if self.symbols.is_open() || self.equation.is_some() || ctx.memory(|m| m.top_modal_layer().is_some()) {
            return;
        }
        // Escape turns the format painter off (the focus may be on its button).
        if self.brush.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.brush = None;
        }
        // Tab while typing stays in the text (or the table): egui would
        // give the focus to the first widget.
        let last = egui::Id::new("slides_focus_last_frame");
        let before: Option<egui::Id> = ctx.data(|d| d.get_temp(last)).flatten();
        if self.typing.is_some() && ctx.input(|i| i.key_pressed(Key::Tab)) {
            ctx.memory_mut(|m| {
                m.move_focus(egui::FocusDirection::None);
                if let Some(f) = m.focused().filter(|f| before != Some(*f)) {
                    m.surrender_focus(f);
                }
            });
        }
        let now = ctx.memory(|m| m.focused());
        ctx.data_mut(|d| d.insert_temp(last, now));
        // Keys go to the widget with the focus (notes, outline), if any.
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let cmd = Modifiers::COMMAND;
        if self.typing.is_some() {
            self.typing_input(ctx);
        } else if self.mode != Mode::Sorter {
            self.object_keys(ctx);
        }
        let n = self.pres.slides.len();
        let mut go = None;
        let free = self.typing.is_none() && self.sel.is_empty() && self.mode != Mode::Master;
        if free {
            ctx.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::PageDown) || i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                    go = Some((self.cur + 1).min(n.saturating_sub(1)));
                } else if i.consume_key(Modifiers::NONE, Key::PageUp) || i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                    go = Some(self.cur.saturating_sub(1));
                } else if i.consume_key(Modifiers::NONE, Key::Home) {
                    go = Some(0);
                } else if i.consume_key(Modifiers::NONE, Key::End) {
                    go = Some(n.saturating_sub(1));
                }
            });
        }
        if let Some(g) = go {
            self.go(g);
        }
        let (undo, redo, new, dup) = ctx.input_mut(|i| {
            (
                i.consume_key(cmd, Key::Z),
                i.consume_key(cmd, Key::Y) || i.consume_key(cmd | Modifiers::SHIFT, Key::Z),
                i.consume_key(cmd, Key::M),
                i.consume_key(cmd, Key::D),
            )
        });
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        if new {
            self.new_slide();
        }
        if dup {
            if self.sel.is_empty() || self.mode == Mode::Sorter {
                self.duplicate();
            } else {
                self.duplicate_objects();
            }
        }
    }

    /// Keys on the selected objects: delete, nudge, select, group, and the
    /// clipboard.
    fn object_keys(&mut self, ctx: &egui::Context) {
        let cmd = Modifiers::COMMAND;
        let some = !self.sel.is_empty();
        let mut nudge = None;
        let (mut del, mut esc, mut edit, mut tab, mut group, mut ungroup, mut all) = (false, false, false, None, false, false, false);
        ctx.input_mut(|i| {
            if some {
                del = i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace);
                esc = i.consume_key(Modifiers::NONE, Key::Escape);
                edit = i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::F2);
                group = i.consume_key(cmd, Key::G);
                ungroup = i.consume_key(cmd | Modifiers::SHIFT, Key::G);
                for (k, dx, dy) in [(Key::ArrowLeft, -1.0, 0.0), (Key::ArrowRight, 1.0, 0.0), (Key::ArrowUp, 0.0, -1.0), (Key::ArrowDown, 0.0, 1.0)] {
                    if i.consume_key(Modifiers::NONE, k) {
                        nudge = Some((dx * NUDGE, dy * NUDGE));
                    } else if i.consume_key(cmd, k) || i.consume_key(Modifiers::ALT, k) {
                        nudge = Some((dx, dy));
                    }
                }
            }
            if i.consume_key(Modifiers::NONE, Key::Tab) {
                tab = Some(false);
            } else if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                tab = Some(true);
            }
            all = i.consume_key(cmd, Key::A);
        });
        if del {
            self.delete_selected();
        }
        if esc {
            self.sel.clear();
            self.tool = None;
        }
        if edit && let Some(&id) = self.sel.first() {
            self.start_typing(id);
        }
        if let Some((dx, dy)) = nudge {
            self.nudge(dx, dy);
        }
        if let Some(back) = tab {
            self.select_next(back);
        }
        if group {
            self.group_selected();
        }
        if ungroup {
            self.ungroup_selected();
        }
        if all {
            self.select_all();
        }
        // The clipboard: objects between slides, text from elsewhere.
        let events: Vec<egui::Event> = ctx.input(|i| i.events.clone());
        for e in events {
            match e {
                egui::Event::Copy | egui::Event::Cut if some => {
                    if let Some(t) = self.copy_selected(matches!(e, egui::Event::Cut)) {
                        ctx.copy_text(t);
                    }
                }
                // Typing with one object selected types in it.
                egui::Event::Text(t) if self.sel.len() == 1 && !self.read_only => {
                    let id = self.sel[0];
                    self.start_typing(id);
                    if self.typing.is_some() {
                        self.type_text(&t);
                    }
                }
                egui::Event::Paste(s) => {
                    if Self::clip_is_ours(&s) {
                        self.paste_objects();
                    } else if !s.trim().is_empty() && !self.read_only {
                        let f = self.centered(360.0, 40.0);
                        if self.insert_text_box(f).is_some() {
                            self.paste_text(&s);
                            self.stop_typing();
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn thumbnails(&mut self, ui: &mut egui::Ui) {
        let deck = &self.pres.deck;
        let w = (ui.available_width() - 26.0).max(40.0);
        let h = w * deck.height / deck.width.max(1.0);
        let row_h = h + 12.0;
        let n = deck.slides.len();
        let mut clicked = None;
        let mut menu_action: Option<&'static str> = None;
        let cur = self.cur;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, n, |ui, range| {
            for i in range {
                let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                let s = &deck.slides[i];
                let r = Rect::from_min_size(pos2(row.min.x + 22.0, row.min.y + 4.0), vec2(w, h));
                let p = ui.painter();
                let num = RichText::new(format!("{}", i + 1)).size(11.0);
                p.text(pos2(row.min.x + 16.0, r.min.y), egui::Align2::RIGHT_TOP, num.text(), egui::FontId::proportional(11.0), ui.visuals().text_color());
                crate::theme::dark_paint(p, || render::paint_slide(p, deck, s, r, render::Opts::default()));
                if s.hidden {
                    p.rect_filled(r, 0.0, Color32::from_white_alpha(140));
                }
                let (stroke, grow) = if i == cur { (Stroke::new(2.5, theme::SLIDES_ACCENT), 2.0) } else if resp.hovered() { (Stroke::new(1.5, Color32::from_gray(150)), 1.0) } else { (Stroke::new(1.0, Color32::from_gray(190)), 0.0) };
                p.rect_stroke(r.expand(grow), 2.0, stroke, egui::StrokeKind::Outside);
                let n = i + 1;
                crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &trf("slides.slide_n", &[("n", &n)]), Some(i == cur));
                if resp.clicked() {
                    clicked = Some(i);
                }
                resp.context_menu(|ui| {
                    clicked = Some(i);
                    for (key, icon) in [("slides.new_slide", Icon::NewSlide), ("slides.duplicate", Icon::Copy), ("slides.delete", Icon::Eraser), ("slides.hide", Icon::Hide)] {
                        if toolstrip::menu_item(ui, icon, tr(key)).clicked() {
                            menu_action = Some(key);
                            ui.close();
                        }
                    }
                });
            }
        });
        if let Some(i) = clicked {
            self.go(i);
        }
        match menu_action {
            Some("slides.new_slide") => self.new_slide(),
            Some("slides.duplicate") => self.duplicate(),
            Some("slides.delete") => self.delete(),
            Some("slides.hide") => self.toggle_hidden(),
            _ => {}
        }
    }

    fn notes_pane(&mut self, ui: &mut egui::Ui) {
        let Some(s) = self.pres.slides.get(self.cur) else { return };
        let id = s.id;
        let mut text = match &self.notes_draft {
            Some((d, t)) if *d == id => t.clone(),
            _ => s.notes_text(),
        };
        let edit = egui::TextEdit::multiline(&mut text).hint_text(tr("slides.notes_hint")).desired_width(f32::INFINITY).frame(egui::Frame::NONE).interactive(!self.read_only);
        let r = egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| ui.add_sized(ui.available_size(), edit)).inner;
        if r.gained_focus() {
            self.stop_typing();
        }
        if r.changed() {
            self.notes_draft = Some((id, text));
        }
        if r.lost_focus() {
            self.commit_notes();
        }
    }

    /// The slide's size on the screen at the current zoom.
    fn slide_px(&self) -> Vec2 {
        vec2(self.pres.width, self.pres.height) * PX_PT * self.zoom
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_rect_before_wrap();
        if self.fit {
            let k = ((avail.width() - 48.0) / (self.pres.width * PX_PT)).min((avail.height() - 48.0) / (self.pres.height * PX_PT));
            self.zoom = k.clamp(0.05, 4.0);
        }
        let size = self.slide_px();
        if size.x <= avail.width() && size.y <= avail.height() {
            self.canvas_area(ui, size);
        } else {
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| self.canvas_area(ui, size));
        }
    }

    /// The status bar's left side.
    pub fn status(&mut self, ui: &mut egui::Ui) {
        let n = self.pres.slides.len();
        ui.label(trf("slides.slide_of", &[("n", &(self.cur + 1).min(n)), ("total", &n)]));
        ui.separator();
        let notes = ui.add(egui::Button::new(tr("slides.notes")).frame(false).selected(self.show_notes));
        if notes.clicked() {
            self.show_notes = !self.show_notes;
        }
    }

    /// Zoom and "fit to window" at the status bar's right.
    pub fn zoom_status(&mut self, ui: &mut egui::Ui) {
        let before = self.zoom;
        if ui.add(egui::Button::new("⛶").frame(false).selected(self.fit)).on_hover_text(tr("slides.fit")).clicked() {
            self.fit = true;
        }
        common::zoom_control(ui, &mut self.zoom, false);
        if self.zoom != before {
            self.fit = false;
        }
    }

    // --- toolstrip --------------------------------------------------------------------------

    pub fn toolstrip_tabs(&self) -> Vec<TabDef> {
        let mut v = vec![
            TabDef::new("home", "toolstrip.home"),
            TabDef::new("insert", "menu.insert"),
            TabDef::new("design", "menu.design"),
            TabDef::new("transitions", "slides.transitions"),
            TabDef::new("animations", "slides.animations"),
            TabDef::new("slideshow", "slides.slide_show"),
            TabDef::new("view", "menu.view"),
        ];
        if self.mode == Mode::Normal && self.selected_media().is_some() {
            v.push(SlidesView::playback_tab_def());
        }
        if self.mode == Mode::Normal && self.selected_diagram().is_some() {
            v.push(SlidesView::diagram_tab_def());
        }
        v
    }

    pub fn toolstrip(&mut self, ui: &mut egui::Ui, tab: &str, shell: &mut Shell) {
        let id = Id::new(("slides_toolstrip", tab));
        toolstrip::body(ui, id, theme::SLIDES_ACCENT, |b| match tab {
            "insert" => self.insert_tab(b),
            "design" => self.design_tab(b),
            "transitions" => self.transitions_tab(b, shell),
            "animations" => self.animations_tab(b, shell),
            "slideshow" => self.show_tab(b, shell),
            "playback" => self.playback_tab(b),
            "diagram" => self.diagram_tab(b),
            _ => self.home_tab(b),
        });
    }

    /// Contextual tabs not showing (Playback, Diagram Design), with their
    /// commands.
    pub fn contextual_commands(ui: &mut egui::Ui, _shell: &mut Shell, showing: &[TabDef]) -> Vec<(TabDef, Vec<toolstrip::Entry>)> {
        let mut out = Vec::new();
        let t = SlidesView::playback_tab_def();
        if !showing.iter().any(|s| s.id == t.id) {
            out.push((t, SlidesView::media_commands(ui)));
        }
        let t = SlidesView::diagram_tab_def();
        if !showing.iter().any(|s| s.id == t.id) {
            out.push((t, SlidesView::diagram_commands(ui)));
        }
        out
    }

    // --- printing ------------------------------------------------------------------------

    /// One page per slide (hidden ones too, as PowerPoint prints them
    /// unless told), or notes pages or handouts for those scopes.
    pub fn print_job(&mut self, ctx: &egui::Context, file: &str) -> crate::print::Job {
        self.print_job_as(ctx, file, crate::print::Scope::ActiveSheet)
    }

    pub fn print_job_as(&mut self, ctx: &egui::Context, file: &str, what: crate::print::Scope) -> crate::print::Job {
        self.commit_notes();
        let deck = &self.pres.deck;
        let title = if deck.meta.title.is_empty() { file.to_string() } else { deck.meta.title.clone() };
        handouts::job(ctx, deck, title, what)
    }
}

/// Empty kept parts: a deck keeps what it doesn't model in its package.
pub fn no_kept() -> &'static apex_doc::KeptParts {
    static EMPTY: std::sync::OnceLock<apex_doc::KeptParts> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Default::default)
}

/// The outline as plain text (a title per slide, indented body lines) or
/// Markdown (headings and nested lists).
pub fn outline_text(deck: &Deck, markdown: bool) -> String {
    let mut out = String::new();
    for (i, (title, body)) in edit::outline(deck).into_iter().enumerate() {
        if markdown {
            out.push_str(&format!("## {}\n\n", if title.is_empty() { format!("{}", i + 1) } else { title }));
            for (level, t) in &body {
                out.push_str(&format!("{}- {}\n", "  ".repeat(*level as usize), t));
            }
        } else {
            out.push_str(&format!("{}. {}\n", i + 1, title));
            for (level, t) in &body {
                out.push_str(&format!("{}{}\n", "    ".repeat(*level as usize + 1), t));
            }
        }
        out.push('\n');
    }
    out
}

/// The outline as a standalone HTML page.
pub fn outline_html(deck: &Deck, title: &str) -> String {
    fn esc(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }
    let mut out = format!("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{}</title></head><body>\n", esc(title));
    for (i, (t, body)) in edit::outline(deck).into_iter().enumerate() {
        out.push_str(&format!("<section><h2>{}</h2>\n", if t.is_empty() { format!("{}", i + 1) } else { esc(&t) }));
        let mut depth = 0;
        for (level, text) in &body {
            let want = *level as usize + 1;
            while depth < want {
                out.push_str("<ul>");
                depth += 1;
            }
            while depth > want {
                out.push_str("</ul>");
                depth -= 1;
            }
            out.push_str(&format!("<li>{}</li>\n", esc(text)));
        }
        out.push_str(&"</ul>".repeat(depth));
        out.push_str("</section>\n");
    }
    out.push_str("</body></html>\n");
    out
}

/// Every picture of a deck's slides, layouts and masters.
pub fn pictures(deck: &Deck) -> Vec<Arc<[u8]>> {
    let mut out: Vec<Arc<[u8]>> = Vec::new();
    let mut add = |o: &apex_slides::Object| {
        if let apex_slides::ObjKind::Picture(p) = &o.kind
            && !out.iter().any(|x| Arc::ptr_eq(x, &p.image.data))
        {
            out.push(p.image.data.clone());
        }
    };
    for s in &deck.slides {
        s.objects.iter().for_each(|o| o.walk(&mut add));
    }
    for m in &deck.masters {
        m.objects.iter().for_each(|o| o.walk(&mut add));
        for l in &m.layouts {
            l.objects.iter().for_each(|o| o.walk(&mut add));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlines() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        v.new_slide();
        let html = outline_html(&v.pres.deck, "<A&B>");
        assert!(html.contains("<title>&lt;A&amp;B&gt;</title>"));
        assert_eq!(html.matches("<section>").count(), 2);
        assert!(outline_text(&v.pres.deck, false).starts_with("1. "));
        assert!(outline_text(&v.pres.deck, true).starts_with("## 1\n"));
        assert!(no_kept().parts.is_empty());
    }

    #[test]
    fn slide_actions_undo() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        assert_eq!(v.pres.slides.len(), 1);
        v.new_slide();
        assert_eq!((v.pres.slides.len(), v.cur), (2, 1));
        v.duplicate();
        assert_eq!((v.pres.slides.len(), v.cur), (3, 2));
        v.move_by(-1);
        assert_eq!(v.cur, 1);
        v.toggle_hidden();
        assert!(v.pres.slides[1].hidden);
        v.delete();
        assert_eq!(v.pres.slides.len(), 2);
        v.set_notes(0, "Hello");
        v.set_notes(0, "Hello there");
        assert_eq!(v.pres.slides[0].notes_text(), "Hello there");
        // Typing the notes is one step.
        v.undo();
        assert_eq!(v.pres.slides[0].notes_text(), "");
        v.undo();
        v.undo();
        v.undo();
        v.undo();
        assert_eq!(v.pres.slides.len(), 2);
        v.undo();
        assert_eq!(v.pres.slides.len(), 1);
        assert!(!v.pres.can_undo());
        v.redo();
        assert_eq!(v.pres.slides.len(), 2);
        // The last slide stays.
        v.go(0);
        v.delete();
        v.delete();
        assert_eq!(v.pres.slides.len(), 1);
    }

    #[test]
    fn read_only_does_not_change() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        v.read_only = true;
        v.new_slide();
        v.set_notes(0, "x");
        assert_eq!(v.pres.slides.len(), 1);
        assert!(!v.is_modified());
    }
}
