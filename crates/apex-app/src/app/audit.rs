//! Button audit: the whole window driven without a screen. In each kind
//! of tab (and on the start screen, and in the classic interface) every
//! control drawn is clicked, one at a time from a clean window, and every
//! command of the search is run; none may panic or hang the window.
//!
//! Controls are found in the accessibility tree egui builds each frame
//! (role, name, place), and clicked with the mouse where they are drawn;
//! one out of view is scrolled to first, and one the test window cuts
//! (a popup taller than it) is clicked by its id, as a screen reader does.
//! What a click opens (a toolstrip tab, a menu, a dialog, a submenu) is
//! clicked through in turn, down to [`DEPTH`] levels. Context menus are
//! found by right-clicking every control and a grid of points over the
//! larger widgets (cells, pages, slides), and their entries clicked too.
//! File dialogs are answered by a hook (`file_dialog::test`), cancelled
//! by default; nothing reaches the desktop (clipboard, browser, printer).
//!
//! Set `APEXDESK_AUDIT_OUT` to a folder to get a report per state there:
//! every control, the context menus, and the ones whose click changed
//! nothing that shows; `APEXDESK_AUDIT_PROGRESS` to a file to follow a
//! run.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::accesskit::{self, Role};
use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};

use super::{App, View};
use crate::file_dialog::test as dialogs;
use crate::i18n::tr;
use crate::settings::Settings;

// Tall enough for the longest menu (the File menu).
const SCREEN: egui::Vec2 = vec2(1600.0, 1400.0);
/// Frames at most to let a click's effects settle.
const SETTLE: usize = 16;
/// A step taking longer than this is reported as a hang (the process
/// stops: a frame that never returns can't be cut short).
const HANG: Duration = Duration::from_secs(240);
/// Clicks down from the window as it starts: a tab, a button, the dialog
/// or menu it opens, a submenu or a list of the dialog, its entry.
const DEPTH: usize = 5;
/// Choices of a drop-down clicked at most: the first ones and the last.
const COMBO: usize = 4;

// --- the states ------------------------------------------------------------------------

/// What the window shows before each click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    Start,
    Doc,
    DocSelected,
    /// A document with a picture, a shape and a table.
    DocObjects,
    Sheet,
    SheetSelected,
    Slides,
    SlidesSelected,
    Plan,
    Db,
    /// The database's table open in its datasheet.
    DbTable,
    Pdf,
    ClassicDoc,
    ClassicSheet,
    /// A notebook: sections, pages, a canvas, a page type.
    Note,
}

pub(super) fn temp_dir() -> PathBuf {
    let d = crate::settings::config_dir().expect("test folder").join("audit");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// A small PDF to open (written once per process).
pub(super) fn pdf_fixture() -> PathBuf {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        let doc = apex_doc::Document::from_paragraphs(vec![apex_doc::Paragraph::new("Audit PDF page")]);
        let bytes = crate::headless::pdf(apex_tui::Book::Doc(&doc), "audit");
        let p = temp_dir().join("audit-sample.pdf");
        std::fs::write(&p, bytes).unwrap();
        p
    })
    .clone()
}

/// A database with one table of two rows, fresh for each window (clicks
/// change it on disk); its name is always the same (the tab's).
pub(super) fn db_fixture(n: u64) -> PathBuf {
    let dir = temp_dir().join(n.to_string());
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("audit-sample.apexdb");
    let _ = std::fs::remove_file(&p);
    let db = apex_db::sqlite::Db::open(&p).unwrap();
    db.conn().execute_batch("CREATE TABLE People(id INTEGER PRIMARY KEY, name TEXT, born DATE); INSERT INTO People(name, born) VALUES ('Ada', '1815-12-10'), ('Alan', '1912-06-23');").unwrap();
    drop(db);
    p
}

/// A notebook, fresh for each window (clicks change it on disk).
pub(super) fn note_fixture(n: u64) -> PathBuf {
    let dir = temp_dir().join(format!("note{n}"));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("audit-sample.apexnote");
    let _ = std::fs::remove_file(&p);
    crate::note_view::qa_sample(&p);
    p
}

fn sample_doc() -> apex_doc::Document {
    apex_doc::Document::from_paragraphs(vec![apex_doc::Paragraph::new("The first paragraph of the audit."), apex_doc::Paragraph::new("A second one, with more words in it.")])
}

/// Text, a picture, a shape and a table, and text again.
fn objects_doc() -> apex_doc::Document {
    use apex_doc::{Attrs, DocShape, Image, OBJECT, Paragraph, Shape, ShapeKind, Table};
    use std::sync::Arc;
    let object = |attrs: Attrs| {
        let mut p = Paragraph::new("");
        p.push(&OBJECT.to_string(), attrs);
        p
    };
    let png = crate::charts::encode_png(&egui::ColorImage::new([40, 30], vec![egui::Color32::from_rgb(70, 120, 200); 40 * 30])).unwrap();
    let pic = Attrs { image: Some(Arc::new(Image::new(Arc::from(png), 160.0, 120.0))), ..Default::default() };
    let mut shape = DocShape::new(Shape::new(ShapeKind::Rect), 120.0, 60.0);
    shape.wrap = apex_doc::ShapeWrap::Inline;
    let shp = Attrs { shape: Some(Arc::new(shape)), ..Default::default() };
    let mut paras = vec![Paragraph::new("Text before the objects."), object(pic), object(shp)];
    paras.extend(Table::new(3, 3, 360.0).into_paras(1));
    paras.push(Paragraph::new("Text after them."));
    apex_doc::Document::from_paragraphs(paras)
}

fn sample_book() -> apex_sheet::Workbook {
    use apex_sheet::{CellRef, Content, Value};
    let mut wb = apex_sheet::Workbook::empty();
    wb.sheets.push(apex_sheet::Sheet::new("Sheet1"));
    for r in 0..6u32 {
        wb.load_cell(0, CellRef::new(r, 0), Content::Value(Value::Text(format!("Item {r}").into())), 0);
        wb.load_cell(0, CellRef::new(r, 1), Content::Value(Value::Number(f64::from(r) * 2.5)), 0);
    }
    wb.finish_load();
    wb
}

impl App {
    /// Opens what `state` shows (the window being blank).
    fn audit_setup(&mut self, state: State, n: u64) {
        use apex_sheet::CellRef;
        match state {
            State::Start => {}
            State::Doc | State::ClassicDoc => self.push_tab(View::Doc(Box::new(crate::doc_view::DocView::new(sample_doc()))), None, None),
            State::DocSelected => {
                let mut v = crate::doc_view::DocView::new(sample_doc());
                v.select_all();
                self.push_tab(View::Doc(Box::new(v)), None, None);
            }
            State::Sheet | State::ClassicSheet => self.push_tab(View::Sheet(Box::new(crate::sheet_view::SheetView::new(sample_book()))), None, None),
            State::SheetSelected => {
                let mut v = crate::sheet_view::SheetView::new(sample_book());
                v.select_range(CellRef::new(0, 0), CellRef::new(5, 1));
                self.push_tab(View::Sheet(Box::new(v)), None, None);
            }
            State::Slides => self.new_slides(),
            State::SlidesSelected => {
                let mut v = crate::slides_view::SlidesView::new(apex_slides::build::new_deck(0));
                v.select_all();
                self.push_tab(View::Slides(Box::new(v)), None, None);
            }
            State::Plan => {
                let mut wb = sample_book();
                let tasks = [apex_sheet::workbook::NewTask::new("Design", 0, 3.0, ""), apex_sheet::workbook::NewTask::new("Build", 0, 5.0, "1")];
                wb.insert_plan(0, CellRef::new(8, 0), apex_sheet::format::date_to_serial(2026, 10, 5), &tasks).unwrap();
                let mut v = crate::sheet_view::SheetView::new(wb);
                v.select_range(CellRef::new(10, 1), CellRef::new(10, 1));
                self.push_tab(View::Sheet(Box::new(v)), None, None);
            }
            State::DocObjects => self.push_tab(View::Doc(Box::new(crate::doc_view::DocView::new(objects_doc()))), None, None),
            State::Db => self.open_db_tab(&db_fixture(n)),
            State::DbTable => {
                self.open_db_tab(&db_fixture(n));
                if let Some(View::Db(v)) = self.tabs.last_mut().map(|t| &mut t.view) {
                    v.open_table("People");
                }
            }
            State::Pdf => self.open_path(&pdf_fixture()),
            State::Note => self.open_note_tab(&note_fixture(n)),
        }
    }
}

// --- one window without a screen -------------------------------------------------------

thread_local! {
    /// This thread's context, fonts installed (installing them is slow).
    static CTX: std::cell::RefCell<Option<egui::Context>> = const { std::cell::RefCell::new(None) };
    /// The context's clock, which never goes back from one window to the next.
    static CLOCK: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

fn context() -> egui::Context {
    CTX.with_borrow_mut(|c| {
        c.get_or_insert_with(|| {
            let ctx = egui::Context::default();
            let s = test_settings(false);
            super::init(&ctx, &s, Instant::now());
            let start = Instant::now();
            while crate::fonts::families().is_empty() && start.elapsed() < Duration::from_secs(20) {
                std::thread::sleep(Duration::from_millis(10));
            }
            ctx
        })
        .clone()
    })
}

/// Drops this thread's context (after a panic it may be left mid-frame).
pub(super) fn drop_context() {
    CTX.with_borrow_mut(|c| *c = None);
}

fn test_settings(classic: bool) -> Settings {
    // The language in force stays (it is the process's): another test
    // may be reading it.
    Settings { lang: crate::i18n::current().to_string(), classic, ..Settings::default() }
}

/// A control found in the accessibility tree.
#[derive(Clone, Debug)]
pub(super) struct Control {
    /// Role, name and which of the same role and name it is.
    pub(super) key: (String, String, usize),
    pub(super) place: String,
    pub(super) rect: Rect,
    /// Where it is, uncut by the window.
    pub(super) bounds: Rect,
    pub(super) enabled: bool,
    /// Already on (a selected tab, a pressed toggle): clicking it again
    /// may rightly change nothing.
    pub(super) on: bool,
    /// It has an on/off state (a check box, a toggle button).
    pub(super) toggle: bool,
    /// Its node, the id of its widget.
    pub(super) node: u64,
    /// A right-click at the centre of `rect` (which may be a point of a
    /// larger widget), for its context menu.
    secondary: bool,
    /// For a right-click, the key of what it is on (its node changes
    /// from window to window).
    target: Option<(String, String, usize)>,
}

impl Control {
    pub(super) fn describe(&self) -> String {
        let (role, name, n) = &self.key;
        let name = if name.is_empty() { "(unnamed)" } else { name };
        let n = if *n > 0 { format!(" #{n}") } else { String::new() };
        let what = if self.secondary { format!("right-click {name}") } else { format!("{role} \"{name}\"{n}") };
        if self.place.is_empty() { what } else { format!("{what} in {}", self.place) }
    }

    /// What tells it from the other controls of an audit: its place too.
    pub(super) fn id(&self) -> (String, (String, String, usize)) {
        (self.place.clone(), self.key.clone())
    }

    /// A right-click at `at` on `on` (a control or a larger widget).
    fn right_click(on: &Control, at: Pos2, what: &str) -> Control {
        let name = format!("{what} at {:.0},{:.0}", at.x, at.y);
        Control { key: ("Context".into(), name, 0), place: on.place.clone(), rect: Rect::from_center_size(at, vec2(2.0, 2.0)), bounds: Rect::from_center_size(at, vec2(2.0, 2.0)), enabled: true, on: false, toggle: false, node: on.node, secondary: true, target: Some(on.key.clone()) }
    }
}

/// What shows of the window, to tell whether a click did anything.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Print {
    pub(super) tree: BTreeSet<String>,
    pub(super) app: String,
    pub(super) settings: Settings,
    /// File dialogs asked for, and commands to the system (clipboard,
    /// window), since the print before.
    dialogs: usize,
    output: usize,
}

impl Print {
    /// Whether `after` (the next print) shows nothing done.
    pub(super) fn unchanged(&self, after: &Print) -> bool {
        self.tree == after.tree && self.app == after.app && self.settings == after.settings && after.dialogs == 0 && after.output == 0
    }
}

/// How a control was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reach {
    Pointer,
    ById,
    /// A right-click whose point is covered.
    Missed,
}

pub(super) struct Rig {
    pub(super) ctx: egui::Context,
    pub(super) app: App,
    frame: eframe::Frame,
    time: f64,
    /// The last frame's controls and tree.
    pub(super) controls: Vec<Control>,
    /// Other widgets that take clicks (a page, the cells), which may have
    /// a context menu.
    pub(super) areas: Vec<Control>,
    tree: BTreeSet<String>,
    /// Commands and clipboard/URL output seen since the last print.
    output: usize,
    /// The window's size: the audit's tall screen, or another (the QA's).
    screen: egui::Vec2,
    /// What the last frame drew.
    pub(super) shapes: Vec<egui::epaint::ClippedShape>,
    /// The pictures loaded as textures (a PDF's pages), but the fonts'.
    pub(super) textures: HashMap<egui::TextureId, Arc<egui::ColorImage>>,
    /// The windows and dialogs shown: name and place.
    pub(super) windows: Vec<(String, Rect)>,
}

fn clickable(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::DefaultButton
            | Role::CheckBox
            | Role::RadioButton
            | Role::Link
            | Role::MenuItem
            | Role::MenuItemCheckBox
            | Role::MenuItemRadio
            | Role::Tab
            | Role::ComboBox
            | Role::Switch
            | Role::ListBoxOption
            | Role::TreeItem
            | Role::Cell
    )
}

fn place_role(role: Role) -> bool {
    matches!(role, Role::Window | Role::Dialog | Role::Menu | Role::MenuBar | Role::Group | Role::TabPanel)
}

impl Rig {
    fn new(state: State, n: u64) -> Rig {
        let classic = matches!(state, State::ClassicDoc | State::ClassicSheet);
        Rig::open(classic, |app| app.audit_setup(state, n))
    }

    /// A fresh window (the classic interface or not) that `setup` fills.
    pub(super) fn open(classic: bool, setup: impl FnOnce(&mut App)) -> Rig {
        Rig::open_sized(classic, SCREEN, setup)
    }

    /// [`Rig::open`] with a window of `screen` points.
    pub(super) fn open_sized(classic: bool, screen: egui::Vec2, setup: impl FnOnce(&mut App)) -> Rig {
        let ctx = context();
        // Fonts set since the last frame take effect in the next one (the
        // memory keeps them until then).
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        // A clean slate: no popup, focus or remembered widget state.
        ctx.memory_mut(|m| {
            let options = m.options.clone();
            *m = egui::Memory::default();
            m.options = options;
        });
        ctx.enable_accesskit();
        crate::toolstrip::test::reset();
        dialogs::set_hook(None);
        dialogs::take_requests();
        // What the process shares between windows.
        crate::autocorrect::close_dialog();
        crate::slides_view::SlidesView::clear_clip();
        crate::spell::test::reset();
        crate::symbols::set_recent(Vec::new());
        let app = App::blank(&ctx, test_settings(classic), None, Instant::now());
        let mut rig = Rig { ctx, app, frame: eframe::Frame::_new_kittest(), time: CLOCK.get(), controls: Vec::new(), areas: Vec::new(), tree: BTreeSet::new(), output: 0, screen, shapes: Vec::new(), textures: HashMap::new(), windows: Vec::new() };
        setup(&mut rig.app);
        rig.settle();
        rig.print();
        rig
    }

    pub(super) fn frame(&mut self, events: Vec<Event>) -> bool {
        self.time += 0.05;
        CLOCK.set(self.time);
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.screen)), time: Some(self.time), focused: true, events, ..Default::default() };
        let (app, frame) = (&mut self.app, &mut self.frame);
        let mut out = self.ctx.run_ui(input, |ui| eframe::App::ui(app, ui, frame));
        for (id, ds) in &out.textures_delta.set {
            // Whole pictures only: the fonts' atlas grows in pieces.
            for d in ds {
                if let (egui::ImageData::Color(img), None) = (&d.image, d.pos)
                    && *id != egui::TextureId::default()
                {
                    self.textures.insert(*id, img.clone());
                }
            }
        }
        for id in &out.textures_delta.free {
            self.textures.remove(id);
        }
        out.textures_delta.clear();
        self.shapes = std::mem::take(&mut out.shapes);
        self.output += out.platform_output.commands.len();
        let vp = out.viewport_output.remove(&egui::ViewportId::ROOT);
        if let Some(vp) = &vp {
            self.output += vp.commands.len();
        }
        if let Some(update) = out.platform_output.accesskit_update.take() {
            self.read_tree(update);
        }
        vp.is_some_and(|v| v.repaint_delay.is_zero())
    }

    pub(super) fn settle(&mut self) {
        for i in 0..SETTLE {
            if !self.frame(Vec::new()) && i >= 1 {
                break;
            }
        }
    }

    fn read_tree(&mut self, update: accesskit::TreeUpdate) {
        let nodes: HashMap<accesskit::NodeId, &accesskit::Node> = update.nodes.iter().map(|(id, n)| (*id, n)).collect();
        let Some(root) = update.tree.as_ref().map(|t| t.root) else { return };
        let (mut controls, mut areas) = (Vec::new(), Vec::new());
        let mut tree = BTreeSet::new();
        let mut seen: HashMap<(String, String), usize> = HashMap::new();
        let mut windows = Vec::new();
        // Depth first, in drawing order, with the places above.
        let mut stack = vec![(root, Vec::<String>::new())];
        let screen = Rect::from_min_size(Pos2::ZERO, self.screen);
        while let Some((id, places)) = stack.pop() {
            let Some(node) = nodes.get(&id) else { continue };
            if node.is_hidden() {
                continue;
            }
            let role = node.role();
            let label = node.label().unwrap_or("").to_string();
            if matches!(role, Role::Window | Role::Dialog)
                && let Some(b) = node.bounds()
            {
                windows.push((label.clone(), Rect::from_min_max(pos2(b.x0 as f32, b.y0 as f32), pos2(b.x1 as f32, b.y1 as f32))));
            }
            let mut sig = format!("{role:?}|{label}|{}", node.value().unwrap_or(""));
            if let Some(t) = node.toggled() {
                sig.push_str(&format!("|t{t:?}"));
            }
            if let Some(s) = node.is_selected() {
                sig.push_str(&format!("|s{s}"));
            }
            if node.is_disabled() {
                sig.push_str("|off");
            }
            let on = node.toggled() == Some(accesskit::Toggled::True) || node.is_selected() == Some(true);
            let n = seen.entry((format!("{role:?}"), label.clone())).or_default();
            tree.insert(format!("{sig}#{n}"));
            if let Some(b) = node.bounds() {
                // Off the screen too (scrolled away): a click scrolls to it.
                let rect = Rect::from_min_max(pos2(b.x0 as f32, b.y0 as f32), pos2(b.x1 as f32, b.y1 as f32));
                let shown = rect.intersect(screen);
                let c = Control { key: (format!("{role:?}"), label.clone(), *n), place: places.join(" > "), rect: shown, bounds: rect, enabled: !node.is_disabled(), on, toggle: node.toggled().is_some(), node: id.0, secondary: false, target: None };
                if clickable(role) {
                    let rect = if shown.width() >= 1.0 && shown.height() >= 1.0 { shown } else { rect };
                    if rect.width() >= 1.0 && rect.height() >= 1.0 {
                        controls.push(Control { rect, ..c });
                    }
                } else if node.supports_action(accesskit::Action::Click) && !node.is_disabled() && shown.width() >= 4.0 && shown.height() >= 4.0 {
                    areas.push(c);
                }
            }
            *n += 1;
            let mut places = places;
            if place_role(role) && !label.is_empty() {
                places.push(label);
            }
            for c in node.children().iter().rev() {
                stack.push((*c, places.clone()));
            }
        }
        self.controls = controls;
        self.areas = areas;
        self.tree = tree;
        self.windows = windows;
    }

    pub(super) fn find(&self, key: &(String, String, usize)) -> Option<Control> {
        self.controls.iter().find(|c| &c.key == key).cloned()
    }

    /// A point of `c` the pointer reaches: its centre, unless the centre is
    /// hidden (a row half scrolled out of its list), then another of its
    /// points egui finds it under. The pointer is left there. None when
    /// no point is found: scrolled out of view.
    pub(super) fn aim(&mut self, c: &Control) -> Option<Pos2> {
        let fractions = [(0.5, 0.5), (0.5, 0.25), (0.5, 0.75), (0.25, 0.5), (0.75, 0.5), (0.5, 0.1), (0.5, 0.9), (0.1, 0.5), (0.9, 0.5)];
        let under = |rig: &Self| rig.ctx.interaction_snapshot(|i| i.contains_pointer.iter().any(|id| id.value() == c.node));
        for (fx, fy) in fractions {
            let at = c.rect.lerp_inside(vec2(fx, fy));
            self.frame(vec![Event::PointerMoved(at)]);
            // Still there a frame later: hovering may move things (a row at
            // the edge of its list, under the scroll bar that shows).
            if under(self) && (self.frame(Vec::new()), under(self)).1 {
                return Some(at);
            }
        }
        self.frame(vec![Event::PointerGone]);
        None
    }

    /// Asks the scroll areas around `c` to show it, as a screen reader
    /// does; `c` as it shows then.
    fn scroll_to(&mut self, c: &Control) -> Option<Control> {
        let request = accesskit::ActionRequest { action: accesskit::Action::ScrollIntoView, target_tree: accesskit::TreeId::ROOT, target_node: accesskit::NodeId(c.node), data: None };
        self.frame(vec![Event::AccessKitActionRequest(request)]);
        self.settle();
        self.find(&c.key)
    }

    /// `c` scrolled into view when the pointer can't reach it where it is
    /// (what a click would do first); the pointer left out of the window.
    pub(super) fn shown(&mut self, c: &Control) -> Control {
        let at = self.aim(c);
        self.frame(vec![Event::PointerGone]);
        let c = if at.is_none() { self.scroll_to(c).unwrap_or_else(|| c.clone()) } else { c.clone() };
        self.settle();
        c
    }

    /// Clicks `c` with the pointer; false when it can't reach it.
    fn click(&mut self, c: &Control) -> bool {
        self.reach(c) == Reach::Pointer
    }

    /// Clicks `c`: with the pointer where it shows, scrolled to first
    /// when it doesn't show, or else as a screen reader does, by its id
    /// (what the test window cuts: a popup taller than it).
    pub(super) fn reach(&mut self, c: &Control) -> Reach {
        // A right-click on what shows here under the same key.
        let here = c.target.as_ref().and_then(|k| self.controls.iter().chain(&self.areas).find(|o| o.key == *k && o.place == c.place));
        let c = &match here {
            Some(o) => Control { node: o.node, ..c.clone() },
            None if c.secondary => return Reach::Missed,
            None => c.clone(),
        };
        let at = match self.aim(c) {
            Some(at) => Some(at),
            None if c.secondary => None,
            None => self.scroll_to(c).and_then(|c| self.aim(&c)),
        };
        let Some(at) = at else {
            if c.secondary {
                return Reach::Missed;
            }
            let request = accesskit::ActionRequest { action: accesskit::Action::Click, target_tree: accesskit::TreeId::ROOT, target_node: accesskit::NodeId(c.node), data: None };
            self.frame(vec![Event::AccessKitActionRequest(request)]);
            self.settle();
            return Reach::ById;
        };
        let button = if c.secondary { PointerButton::Secondary } else { PointerButton::Primary };
        let press = |pressed| Event::PointerButton { pos: at, button, pressed, modifiers: Modifiers::NONE };
        self.frame(vec![press(true)]);
        self.frame(vec![press(false)]);
        self.frame(vec![Event::PointerGone]);
        self.settle();
        Reach::Pointer
    }

    pub(super) fn print(&mut self) -> Print {
        let a = &self.app;
        let tabs: Vec<String> = a.tabs.iter().map(|t| format!("{}:{}:{}", t.name(), t.view.modified(), matches!(t.view, View::Doc(_)))).collect();
        // What changes only what the next action does (a tool, the format
        // for typing) shows nowhere in the tree.
        let modes: Vec<String> = a
            .tabs
            .iter()
            .map(|t| match &t.view {
                View::Doc(v) => v.audit_modes(),
                View::Sheet(v) => v.audit_modes(),
                View::Slides(v) => v.audit_modes(),
                View::Pdf(v) => v.audit_modes(),
                View::Db(v) => format!("{:?}", v.nav_sel),
                View::Note(v) => format!("{:?}/{:?}", v.section(), v.page()),
            })
            .collect();
        let app = format!(
            "{tabs:?} {modes:?} {} {:?} {:?} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
            a.active,
            a.toolstrip_tab,
            a.closing.is_some(),
            a.show_shortcuts,
            a.show_about,
            a.fullscreen.asked(),
            a.print.is_some(),
            a.pdf_task.is_some(),
            a.template_gallery.is_some(),
            a.save_template.is_some(),
            a.side.is_some(),
            a.options.is_some(),
            a.palette.is_some(),
            a.customize.is_some(),
            a.password_prompt.is_some(),
            a.encrypt_dlg.is_some(),
            a.props_dlg.is_some(),
            a.inspector.is_some(),
            a.convert_dlg.is_some(),
            a.encoding_dlg.is_some(),
            a.show_policy,
            a.show_diag,
            a.mail_rx.is_some(),
            a.toasts.len(),
        );
        let dialogs = dialogs::take_requests().len();
        // Messages handed to the mail program.
        let mails = crate::mail::test::COMPOSED.swap(0, std::sync::atomic::Ordering::Relaxed);
        // Files, folders and pages handed to the system to open.
        let mails = mails + crate::common::test::OPENED.swap(0, std::sync::atomic::Ordering::Relaxed);
        Print { tree: self.tree.clone(), app, settings: a.settings.clone(), dialogs, output: std::mem::take(&mut self.output) + mails }
    }
}

// --- watching for hangs -----------------------------------------------------------------

pub(super) fn watch(what: Option<String>) {
    static STEPS: OnceLock<Mutex<HashMap<std::thread::ThreadId, (Instant, String)>>> = OnceLock::new();
    let steps = STEPS.get_or_init(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let stuck = STEPS.get().and_then(|m| m.lock().ok()?.values().find(|(t, _)| t.elapsed() > HANG).map(|(_, w)| w.clone()));
                if let Some(w) = stuck {
                    eprintln!("button audit: the window hangs at {w}");
                    std::process::exit(101);
                }
            }
        });
        Mutex::default()
    });
    // The steps as they go, to follow a long audit.
    if let (Some(file), Some(w)) = (std::env::var_os("APEXDESK_AUDIT_PROGRESS"), &what) {
        use std::io::Write;
        static START: OnceLock<Instant> = OnceLock::new();
        let secs = START.get_or_init(Instant::now).elapsed().as_secs_f32();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
            let _ = writeln!(f, "{secs:8.1} {w}");
        }
    }
    let mut m = steps.lock().unwrap_or_else(|e| e.into_inner());
    match what {
        Some(w) => m.insert(std::thread::current().id(), (Instant::now(), w)),
        None => m.remove(&std::thread::current().id()),
    };
}

/// Where the last panic of this thread happened (the hook knows it).
pub(super) fn panic_hook() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let prev = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            LAST_PANIC.with_borrow_mut(|p| *p = at);
            if !QUIET.get() {
                prev(info);
            }
        }));
    });
}

thread_local! {
    static LAST_PANIC: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static QUIET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f`, a panic in it caught without its message printed.
pub(super) fn quietly<R>(f: impl FnOnce() -> R) -> std::thread::Result<R> {
    QUIET.set(true);
    let r = panic::catch_unwind(AssertUnwindSafe(f));
    QUIET.set(false);
    r
}

pub(super) fn panic_text(e: &Box<dyn std::any::Any + Send>) -> String {
    let msg = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default();
    format!("{msg} at {}", LAST_PANIC.with_borrow(Clone::clone))
}

// --- the audit -------------------------------------------------------------------------

#[derive(Default)]
pub(super) struct Report {
    pub clicked: usize,
    pub panics: Vec<String>,
    pub no_effect: Vec<String>,
    /// Without visible effect, rightly: what is already on, clicked again,
    /// and the controls of [`EXPECTED`].
    pub expected: Vec<String>,
    /// Clicked by id, as a screen reader does: out of the pointer's reach
    /// in the test window (a popup taller than it).
    pub by_id: Vec<String>,
    /// Drop-downs with more choices than [`COMBO`], only some clicked.
    pub sampled: Vec<String>,
    pub gone: Vec<String>,
    /// The context menus found: where they open, and their entries.
    pub menus: Vec<String>,
    /// The most clicks down from the window as it starts.
    pub depth: usize,
    pub all: Vec<String>,
}

impl Report {
    fn write(&self, name: &str) {
        let Some(dir) = std::env::var_os("APEXDESK_AUDIT_OUT") else { return };
        let dir = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        let text = format!(
            "clicked {}, {} levels down, {} context menus\n\n== panics\n{}\n\n== no visible effect\n{}\n\n== no effect, as expected\n{}\n\n== clicked by id: out of the pointer's reach in the test window\n{}\n\n== drop-downs with only some choices clicked\n{}\n\n== not found on replay\n{}\n\n== context menus\n{}\n\n== all\n{}\n",
            self.clicked,
            self.depth,
            self.menus.len(),
            self.panics.join("\n"),
            self.no_effect.join("\n"),
            self.expected.join("\n"),
            self.by_id.join("\n"),
            self.sampled.join("\n"),
            self.gone.join("\n"),
            self.menus.join("\n"),
            self.all.join("\n")
        );
        let _ = crate::common::write_out(dir.join(format!("{name}.txt")), text);
    }
}

/// Controls that rightly change nothing in the audit's windows, with why
/// (they stay enabled there, as in Office).
const EXPECTED: &[(&str, &str)] = &[
    ("sheet.outdent", "the cells have no indent to take away"),
    ("plan.outdent", "the task is at the top level already"),
    ("audit.calc_now", "nothing volatile: calculating again gives the same values"),
    ("edit.select_all", "everything is selected already (DocSelected)"),
    ("format.change_case", "a new document has no text to change"),
    ("diag.open_log", "tests keep no log"),
    ("diag.open_folder", "tests keep no log"),
    ("sign.refresh", "reads again what hasn't changed (the certificates, the data)"),
    ("getdata.reload", "no server to read from in tests"),
    ("solver.reset", "nothing set yet to reset"),
    ("ac.reset", "the list is the default one already"),
];

/// Whether a control of this name is one of [`EXPECTED`].
fn expected(name: &str) -> bool {
    // A submenu opens as the pointer comes to it: clicking it then adds
    // nothing.
    name.ends_with('⏵') || EXPECTED.iter().any(|(key, _)| tr(key) == name.trim_end_matches('…'))
}

/// Labels never clicked: they change the process for every test (the
/// interface language).
pub(super) fn skipped(c: &Control) -> bool {
    crate::i18n::available().any(|l| l.name == c.key.1)
}

/// What a replay found.
enum Replayed {
    /// A step wasn't there, or a right-click's point was covered.
    Gone,
    /// The print before and after the last click, the controls after it,
    /// how it was clicked, and whether the window still shows the tab it
    /// started with (not another one, opened or left for).
    Clicked(Print, Print, Vec<Control>, Reach, bool),
}

/// Clicks through `path` from a fresh window in `state`.
fn replay(state: State, path: &[Control], n: u64) -> Result<Replayed, String> {
    let r = quietly(|| {
        let mut rig = Rig::new(state, n);
        let last = path.len() - 1;
        let mut before = None;
        let mut reach = Reach::Pointer;
        let tab = (rig.app.tabs.len(), rig.app.active);
        for (i, step) in path.iter().enumerate() {
            // A right-click is at a point, not on a control found by name.
            let c = if step.secondary { step.clone() } else if let Some(c) = rig.find(&step.key) { c } else { return Replayed::Gone };
            if i == last {
                before = Some(rig.print());
            }
            reach = rig.reach(&c);
            if reach == Reach::Missed {
                return Replayed::Gone;
            }
        }
        let after = rig.print();
        match before {
            Some(b) => Replayed::Clicked(b, after, rig.controls.clone(), reach, tab == (rig.app.tabs.len(), rig.app.active)),
            None => Replayed::Gone,
        }
    });
    r.map_err(|e| {
        drop_context();
        crate::toolstrip::test::reset();
        panic_text(&e)
    })
}

fn describe_path(path: &[Control]) -> String {
    path.iter().map(Control::describe).collect::<Vec<_>>().join(" → ")
}

/// Points of `rect` a grid `step` apart, and along its edges (headers, a
/// ruler): at most `max`.
fn grid(rect: Rect, step: f32, max: usize) -> Vec<Pos2> {
    let axis = |a: f32, b: f32| {
        let mut v = vec![a + 5.0];
        let mut x = a + step / 2.0;
        while x < b - 5.0 {
            v.push(x);
            x += step;
        }
        v.push(b - 5.0);
        v
    };
    let (xs, ys) = (axis(rect.min.x, rect.max.x), axis(rect.min.y, rect.max.y));
    ys.iter().flat_map(|y| xs.iter().map(move |x| pos2(*x, *y))).take(max).collect()
}

/// Where right-clicks open a context menu in `state`: on each control
/// and over the larger widgets (a page, the cells, a slide); one place
/// for each different menu.
fn context_menus(state: State, base: &[Control], report: &mut Report) -> Vec<Control> {
    watch(Some(format!("{state:?}: looking for context menus")));
    let r = quietly(|| {
        let mut rig = Rig::new(state, 0);
        let old: HashSet<_> = base.iter().map(Control::id).collect();
        let mut points: Vec<Control> = rig.controls.iter().filter(|c| c.enabled && !skipped(c)).map(|c| Control::right_click(c, c.rect.center(), &c.describe())).collect();
        for a in &rig.areas {
            let what = if a.key.1.is_empty() { format!("{:?}", a.key.0) } else { a.key.1.clone() };
            points.extend(grid(a.rect, 40.0, 900).into_iter().map(|p| Control::right_click(a, p, &what)));
        }
        let mut found = Vec::new();
        let mut menus: HashSet<BTreeSet<String>> = HashSet::new();
        for p in points {
            watch(Some(format!("{state:?}: looking for context menus: {}", p.describe())));
            if rig.reach(&p) == Reach::Missed {
                continue;
            }
            let menu = |rig: &Rig| {
                let mut new: Vec<String> = rig.controls.iter().filter(|c| !old.contains(&c.id())).map(|c| c.key.1.clone()).collect();
                // Spelling suggestions (above Ignore All) differ from word
                // to word, in the same menu.
                if let Some(i) = new.iter().position(|l| *l == tr("spell.ignore_all")) {
                    new.drain(..i);
                }
                let new: BTreeSet<String> = new.into_iter().collect();
                (egui::Popup::is_any_open(&rig.ctx) && !new.is_empty()).then_some(new)
            };
            let open = egui::Popup::is_any_open(&rig.ctx);
            if let Some(new) = menu(&rig).filter(|m| !menus.contains(m)) {
                // The same menu, right-clicked there in a clean window (as
                // the audit will).
                let mut clean = Rig::new(state, 0);
                // (Its entries may differ there: a new one, then.)
                let again = if clean.reach(&p) == Reach::Pointer { menu(&clean).filter(|m| !menus.contains(m)) } else { None };
                menus.insert(new);
                if let Some(again) = again {
                    report.menus.push(format!("{}: {}", p.describe(), again.iter().cloned().collect::<Vec<_>>().join(", ")));
                    menus.insert(again);
                    found.push(p.clone());
                }
                // The windows share the context: this one's is gone.
                drop(clean);
                rig = Rig::new(state, 0);
                continue;
            }
            // The next point with the menu closed, or from a clean window
            // when the right-click changed more.
            if open {
                rig.frame(vec![Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
                rig.settle();
            }
            if egui::Popup::is_any_open(&rig.ctx) || rig.controls.len() != base.len() || rig.controls.iter().any(|c| !old.contains(&c.id())) {
                rig = Rig::new(state, 0);
            }
        }
        found
    });
    r.unwrap_or_else(|e| {
        drop_context();
        crate::toolstrip::test::reset();
        report.panics.push(format!("{state:?}: looking for context menus: {}", panic_text(&e)));
        Vec::new()
    })
}

/// Clicks every control of `state`, what they open, and every entry of
/// its context menus, one at a time.
pub(super) fn crawl(state: State) -> Report {
    panic_hook();
    let mut report = Report::default();
    let base = {
        watch(Some(format!("{state:?}: start")));
        let rig = Rig::new(state, 0);
        rig.controls.clone()
    };
    let menus = context_menus(state, &base, &mut report);
    let old: HashSet<_> = base.iter().map(Control::id).collect();
    // What was clicked, and in which context menu: the entries of each
    // menu are its own, even under the name of another menu's.
    let mut seen: HashSet<(String, (String, (String, String, usize)))> = HashSet::new();
    let mut queue: Vec<Vec<Control>> = base.iter().filter(|c| c.enabled && !skipped(c)).map(|c| vec![c.clone()]).collect();
    queue.extend(menus.into_iter().map(|m| vec![m]));
    let mut n = 0;
    while !queue.is_empty() {
        let mut next = Vec::new();
        for path in queue {
            n += 1;
            let what = format!("{state:?}: {}", describe_path(&path));
            watch(Some(what.clone()));
            report.clicked += 1;
            report.all.push(what.clone());
            match replay(state, &path, n) {
                Err(e) => report.panics.push(format!("{what}: {e}")),
                Ok(Replayed::Gone) => report.gone.push(what),
                Ok(Replayed::Clicked(before, after, controls, reach, same_tab)) => {
                    report.depth = report.depth.max(path.len());
                    let last = &path[path.len() - 1];
                    if reach == Reach::ById {
                        report.by_id.push(what.clone());
                    }
                    if before.unchanged(&after) {
                        if last.on || expected(&last.key.1) { report.expected.push(what.clone()) } else { report.no_effect.push(what.clone()) }
                    }
                    // Another tab (a new file, or the start screen) is
                    // another state's audit.
                    if path.len() < DEPTH && same_tab {
                        let menu = if path.len() == 1 && last.secondary { last.key.1.clone() } else { String::new() };
                        let mut fresh: Vec<Control> = controls.into_iter().filter(|c| c.enabled && !old.contains(&c.id()) && !skipped(c) && seen.insert((menu.clone(), c.id()))).collect();
                        // A drop-down's choices are alike (fonts, voices):
                        // the first ones and the last.
                        if last.key.0 == "ComboBox" && fresh.len() > COMBO {
                            report.sampled.push(format!("{what}: {COMBO} of {} choices", fresh.len()));
                            fresh.drain(COMBO - 1..fresh.len() - 1);
                        }
                        next.extend(fresh.into_iter().map(|c| {
                            let mut p = path.clone();
                            p.push(c);
                            p
                        }));
                    }
                }
            }
        }
        queue = next;
    }
    watch(None);
    report
}


/// Runs every command of the search in `state`, one at a time.
pub(super) fn commands(state: State) -> Report {
    panic_hook();
    let mut report = Report::default();
    watch(Some(format!("{state:?}: collecting commands")));
    let list = {
        let mut rig = Rig::new(state, 0);
        rig.app.want_commands = true;
        rig.settle();
        rig.app.commands.take().unwrap_or_default()
    };
    assert!(state == State::Start || list.len() > 20, "{state:?}: {} commands", list.len());
    for (i, c) in list.iter().enumerate() {
        if !c.entry.enabled {
            continue;
        }
        let what = format!("{state:?}: command {} ({})", c.entry.key, if c.entry.menu { "menu" } else { "button" });
        watch(Some(what.clone()));
        report.clicked += 1;
        report.all.push(what.clone());
        let r = quietly(|| {
            let mut rig = Rig::new(state, 1000 + i as u64);
            let before = rig.print();
            let ctx = rig.ctx.clone();
            rig.app.choose(&ctx, crate::palette::Chosen { tab: c.tab, key: c.entry.key.clone(), menu: c.entry.menu });
            rig.frame(Vec::new());
            let found = c.entry.menu || crate::toolstrip::test::take_last_run() == Some(true);
            rig.settle();
            let open = egui::Popup::is_any_open(&rig.ctx);
            (found, before.unchanged(&rig.print()) && !open)
        });
        match r {
            Err(e) => {
                drop_context();
                crate::toolstrip::test::reset();
                report.panics.push(format!("{what}: {}", panic_text(&e)));
            }
            Ok((false, _)) => report.gone.push(what),
            Ok((true, true)) if c.entry.on || expected(&c.entry.label) => report.expected.push(what),
            Ok((true, true)) => report.no_effect.push(what),
            Ok((true, false)) => {}
        }
    }
    watch(None);
    report
}

fn check(name: &str, r: &Report) {
    r.write(name);
    eprintln!("{name}: {} clicked, {} panics, {} without visible effect, {} not found again", r.clicked, r.panics.len(), r.no_effect.len(), r.gone.len());
    assert!(r.panics.is_empty(), "{name}: controls that panic:\n{}", r.panics.join("\n"));
}

/// Runs `f` in a process of its own: the fonts and the language are the
/// process's, and a window must not share them with other tests' ones.
fn isolated(name: &str, f: impl FnOnce()) {
    isolated_at(&format!("{}::{name}", module_path!().split_once("::").map_or("", |p| p.1)), f);
}

/// [`isolated`] for the test at `path` ("app::qa::a11y::name").
pub(super) fn isolated_at(path: &str, f: impl FnOnce()) {
    const CHILD: &str = "APEXDESK_AUDIT_CHILD";
    if std::env::var(CHILD).is_ok_and(|v| v == path) {
        f();
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe).args([path, "--exact", "--nocapture", "--test-threads=1"]).env(CHILD, &path).output().unwrap();
    let text = String::from_utf8_lossy(&out.stderr);
    eprint!("{text}");
    assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("1 passed"), "{path} failed:\n{text}");
}

macro_rules! audit {
    ($($crawl:ident, $cmds:ident, $state:ident;)*) => {
        $(
            #[test]
            fn $crawl() {
                isolated(stringify!($crawl), || check(stringify!($crawl), &crawl(State::$state)));
            }

            #[test]
            fn $cmds() {
                isolated(stringify!($cmds), || {
                    let r = commands(State::$state);
                    check(stringify!($cmds), &r);
                    assert!(r.gone.is_empty(), "{:?}: commands of the search not found when run:\n{}", State::$state, r.gone.join("\n"));
                });
            }
        )*
    };
}

audit! {
    start_screen_controls, start_screen_commands, Start;
    doc_controls, doc_commands, Doc;
    doc_selected_controls, doc_selected_commands, DocSelected;
    doc_objects_controls, doc_objects_commands, DocObjects;
    sheet_controls, sheet_commands, Sheet;
    sheet_selected_controls, sheet_selected_commands, SheetSelected;
    slides_controls, slides_commands, Slides;
    slides_selected_controls, slides_selected_commands, SlidesSelected;
    plan_controls, plan_commands, Plan;
    db_controls, db_commands, Db;
    db_table_controls, db_table_commands, DbTable;
    pdf_controls, pdf_commands, Pdf;
    classic_doc_controls, classic_doc_commands, ClassicDoc;
    classic_sheet_controls, classic_sheet_commands, ClassicSheet;
    note_controls, note_commands, Note;
}

/// A check box low in Options, shown first, clicked twice leaves the
/// window as it was: scrolling to it is not the check box's doing.
#[test]
fn shown_toggle_clicked_twice_is_back() {
    isolated("shown_toggle_clicked_twice_is_back", || {
        let mut rig = Rig::new(State::Doc, 0);
        rig.app.open_options();
        rig.settle();
        let c = rig.controls.iter().find(|c| c.key.1 == tr("options.lock_files")).cloned().expect("options: lock files");
        let c = rig.shown(&c);
        let before = rig.print();
        assert!(rig.click(&c) && rig.click(&rig.find(&c.key).expect("found again")), "out of view");
        let after = rig.print();
        assert!(before.tree == after.tree && before.settings == after.settings, "{:?}", before.tree.symmetric_difference(&after.tree).take(4).collect::<Vec<_>>());
    });
}

/// The close cross sits under the tab's own click area; clicking it must
/// still close the tab (or ask to save it) instead of just selecting it.
#[test]
fn tab_cross_closes() {
    isolated("tab_cross_closes", || {
        let mut rig = Rig::new(State::Doc, 0);
        let before = rig.app.tabs.len();
        let c = rig.controls.iter().find(|c| c.key.1 == "×").cloned().expect("tab cross");
        assert!(rig.click(&c), "out of view");
        assert!(rig.app.tabs.len() < before || rig.app.closing.is_some(), "the tab cross did nothing");
    });
}

/// The start screen and the tab bar's "+" menu create a database, like
/// they create the other kinds of files.
#[test]
fn start_screen_creates_database() {
    isolated("start_screen_creates_database", || {
        let mut rig = Rig::new(State::Start, 0);
        let dir = std::env::temp_dir().join(format!("apexdesk-audit-newdb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let into = dir.join("new.apexdb");
        dialogs::set_hook(Some(Box::new(move |r| (r.kind == dialogs::Kind::Save).then(|| vec![into.clone()]))));
        let c = rig.controls.iter().find(|c| c.key.1 == tr("start.new_database")).cloned().expect("start screen: new database");
        assert!(rig.click(&c), "out of view");
        assert!(matches!(rig.app.tabs.first().map(|t| &t.view), Some(View::Db(_))), "no database tab");
        let plus = rig.controls.iter().find(|c| c.key.1 == "+").cloned().expect("tab bar: +");
        assert!(rig.click(&plus), "out of view");
        let item = format!("{}…", tr("menu.new_database"));
        assert!(rig.controls.iter().any(|c| c.key.1 == item), "+ menu: new database");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// "Save As" asks for a titled dialog with a name and a filter, and
/// writes the file the dialog answers.
#[test]
fn save_as_writes_file() {
    isolated("save_as_writes_file", || {
        for (i, state) in [State::Doc, State::Sheet, State::Slides].into_iter().enumerate() {
            let mut rig = Rig::new(state, 500 + i as u64);
            let dir = std::env::temp_dir().join(format!("apexdesk-audit-save-{}-{i}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let into = dir.clone();
            dialogs::set_hook(Some(Box::new(move |r| {
                let stem = std::path::Path::new(&r.file_name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let ext = r.exts.first().cloned().unwrap_or_default();
                (r.kind == dialogs::Kind::Save).then(|| vec![into.join(format!("{stem}.{ext}"))])
            })));
            // The dialog answers in the next frame.
            let saved = std::rc::Rc::new(std::cell::Cell::new(false));
            let s = saved.clone();
            rig.app.save_then(0, true, move |_, ok| s.set(ok));
            crate::file_dialog::poll(&mut rig.app);
            assert!(saved.get(), "{state:?}: save as failed");
            let asked = dialogs::take_requests();
            assert!(asked.len() == 1 && !asked[0].title.is_empty() && !asked[0].file_name.is_empty(), "{state:?}: {asked:?}");
            let written: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.metadata().is_ok_and(|m| m.len() > 0)).collect();
            assert_eq!(written.len(), 1, "{state:?}: nothing written");
            let _ = std::fs::remove_dir_all(&dir);
        }
    });
}

/// "Save As" offers PDF, as Office does: the file is exported and the tab
/// stays the file it was, still to be saved.
#[test]
fn save_as_pdf_exports() {
    isolated("save_as_pdf_exports", || {
        for (i, state) in [State::Doc, State::Sheet, State::Slides].into_iter().enumerate() {
            let mut rig = Rig::new(state, 530 + i as u64);
            let dir = std::env::temp_dir().join(format!("apexdesk-audit-savepdf-{}-{i}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let into = dir.join("out.pdf");
            let to = into.clone();
            dialogs::set_hook(Some(Box::new(move |r| (r.kind == dialogs::Kind::Save && r.exts.iter().any(|e| e == "pdf")).then(|| vec![to.clone()]))));
            rig.app.save_then(0, true, |_, _| {});
            crate::file_dialog::poll(&mut rig.app);
            for _ in 0..400 {
                if rig.app.pdf_task.is_none() && into.exists() {
                    break;
                }
                rig.frame(Vec::new());
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let pdf = std::fs::read(&into).unwrap_or_default();
            assert!(pdf.starts_with(b"%PDF"), "{state:?}: no PDF written");
            let t = &rig.app.tabs[0];
            assert!(t.path.is_none(), "{state:?}: the tab became the PDF");
            let _ = std::fs::remove_dir_all(&dir);
        }
    });
}

/// A file dialog doesn't stop the window: frames go on while it's open,
/// its answer is used once it comes, and dropped if the tab it was for
/// has closed meanwhile.
#[test]
fn file_dialog_waits_without_blocking() {
    isolated("file_dialog_waits_without_blocking", || {
        use std::cell::Cell;
        use std::rc::Rc;
        let mut rig = Rig::new(State::Doc, 520);
        let dir = std::env::temp_dir().join(format!("apexdesk-audit-dlg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let into = dir.join("later.docx");
        let to = into.clone();
        dialogs::set_hook(Some(Box::new(move |r| (r.kind == dialogs::Kind::Save).then(|| vec![to.clone()]))));
        dialogs::hold();
        let saved = Rc::new(Cell::new(None));
        let s = saved.clone();
        rig.app.save_then(0, true, move |_, ok| s.set(Some(ok)));
        let before = rig.ctx.cumulative_pass_nr();
        for _ in 0..5 {
            rig.frame(Vec::new());
        }
        assert_eq!(rig.ctx.cumulative_pass_nr(), before + 5, "frames stopped");
        assert!(crate::file_dialog::busy() && saved.get().is_none() && !into.exists(), "answered too soon");
        // Another dialog meanwhile counts as cancelled.
        let other = Rc::new(Cell::new(false));
        let o = other.clone();
        crate::file_dialog::FileDialog::new().pick_file_or_cancel(move |_, p| o.set(p.is_none()));
        dialogs::release();
        rig.frame(Vec::new());
        assert_eq!(saved.get(), Some(true), "the answer wasn't used");
        assert!(into.exists() && other.get() && !crate::file_dialog::busy());

        // The tab closes while its dialog is open: the answer is dropped.
        let into = dir.join("dropped.docx");
        let to = into.clone();
        dialogs::set_hook(Some(Box::new(move |_| Some(vec![to.clone()]))));
        dialogs::hold();
        let saved = Rc::new(Cell::new(None));
        let s = saved.clone();
        rig.app.save_then(0, true, move |_, ok| s.set(Some(ok)));
        let used = Rc::new(Cell::new(false));
        let u = used.clone();
        crate::file_dialog::set_owner(rig.app.tabs[0].id);
        let then = crate::file_dialog::in_view(move |_: &mut crate::doc_view::DocView, _: std::path::PathBuf| u.set(true));
        rig.frame(Vec::new());
        rig.app.remove_tab(0);
        rig.frame(Vec::new());
        dialogs::release();
        rig.frame(Vec::new());
        assert_eq!(saved.get(), Some(false), "saved a closed tab");
        assert!(!into.exists());
        crate::file_dialog::FileDialog::new().pick_file(then);
        rig.frame(Vec::new());
        assert!(!used.get(), "a closed tab's view got the answer");
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// Clicks the control named `name` (the first of them), which must be there.
fn click_named(rig: &mut Rig, name: &str) {
    let c = rig.controls.iter().find(|c| c.key.1 == name).cloned().unwrap_or_else(|| panic!("no control \"{name}\""));
    assert!(c.enabled, "\"{name}\" is disabled");
    assert!(rig.click(&c), "out of view");
}

fn enabled(rig: &Rig, name: &str) -> Option<bool> {
    rig.controls.iter().find(|c| c.key.1 == name).map(|c| c.enabled)
}

/// The database's Data and Design views act on the open object, or on
/// the one chosen in the navigation pane; with neither they are disabled.
#[test]
fn db_views_switch() {
    isolated("db_views_switch", || {
        let mut rig = Rig::new(State::Db, 0);
        let (data, design) = (tr("db.view_data"), tr("db.design"));
        assert_eq!(enabled(&rig, data), Some(false), "Data without an object");
        assert_eq!(enabled(&rig, design), Some(false), "Design without an object");
        click_named(&mut rig, "People");
        click_named(&mut rig, data);
        let pane = |rig: &Rig| match &rig.app.tabs[0].view {
            View::Db(v) => v.panes.get(v.cur).map(|p| match p {
                crate::db_view::Pane::Sheet(_) => "sheet",
                crate::db_view::Pane::Design(_) => "design",
                _ => "other",
            }),
            _ => None,
        };
        assert_eq!(pane(&rig), Some("sheet"));
        click_named(&mut rig, design);
        assert_eq!(pane(&rig), Some("design"));
        click_named(&mut rig, data);
        assert_eq!(pane(&rig), Some("sheet"));
    });
}

/// The PDF's tools, page modes and fits switch back and forth.
#[test]
fn pdf_view_modes_switch() {
    isolated("pdf_view_modes_switch", || {
        use crate::pdf_view::{Layout, Tool};
        let mut rig = Rig::new(State::Pdf, 0);
        let pdf = |rig: &Rig| match &rig.app.tabs[0].view {
            View::Pdf(v) => (v.tool, v.layout, v.fit),
            _ => unreachable!(),
        };
        click_named(&mut rig, tr("pdf.tool_hand"));
        assert_eq!(pdf(&rig).0, Tool::Hand);
        click_named(&mut rig, tr("pdf.tool_select"));
        assert_eq!(pdf(&rig).0, Tool::Select);
        click_named(&mut rig, tr("menu.view"));
        click_named(&mut rig, tr("pdf.layout_single"));
        assert_eq!(pdf(&rig).1, Layout::Single);
        click_named(&mut rig, tr("pdf.layout_continuous"));
        assert_eq!(pdf(&rig).1, Layout::Continuous);
        click_named(&mut rig, tr("zoom.whole_page"));
        assert_eq!(pdf(&rig).2, Some(crate::common::Fit::WholePage));
        click_named(&mut rig, tr("zoom.page_width"));
        assert_eq!(pdf(&rig).2, Some(crate::common::Fit::PageWidth));
    });
}

/// Database and PDF tabs show their own icon, not a document's.
#[test]
fn tab_icons_by_kind() {
    isolated("tab_icons_by_kind", || {
        use crate::icons::Icon;
        for (state, icon) in [(State::Db, Icon::Table), (State::Pdf, Icon::Pdf), (State::Doc, Icon::Doc)] {
            let rig = Rig::new(state, 700);
            assert_eq!(rig.app.tabs[0].view.badge().0, icon, "{state:?}");
        }
    });
}

/// What Send by E-mail attaches from a database and a PDF: the file, or
/// a real PDF, named for what it is.
#[test]
fn mail_attachments() {
    isolated("mail_attachments", || {
        let pdf_bytes = |p: &std::path::Path| std::fs::read(p).unwrap().starts_with(b"%PDF");
        let mut rig = Rig::new(State::Db, 800);
        let db = rig.app.mail_file(0, false).unwrap();
        assert_eq!(db.extension().unwrap(), "apexdb");
        let as_pdf = rig.app.mail_file(0, true).unwrap();
        assert!(as_pdf.extension().unwrap() == "pdf" && pdf_bytes(&as_pdf), "database as PDF: {as_pdf:?}");
        let mut rig = Rig::new(State::Pdf, 801);
        // Changed since saved: a copy is attached.
        if let View::Pdf(v) = &mut rig.app.tabs[0].view {
            v.doc.rotate_pages(&[0], 1);
        }
        assert!(rig.app.tabs[0].view.modified());
        for pdf in [false, true] {
            let f = rig.app.mail_file(0, pdf).unwrap();
            assert!(f.extension().unwrap() == "pdf" && pdf_bytes(&f), "PDF (as PDF: {pdf}): {f:?}");
        }
    });
}

/// The Insert function dialog keeps its size, its list shows one
/// function a row, and picking one shows it.
#[test]
fn function_list_rows() {
    isolated("function_list_rows", || {
        let mut rig = Rig::new(State::Sheet, 0);
        click_named(&mut rig, tr("fx.title"));
        let ok = |rig: &Rig| rig.controls.iter().find(|c| c.key.1 == tr("dialog.ok")).map(|c| c.rect);
        let at = ok(&rig);
        for _ in 0..3 {
            rig.frame(Vec::new());
        }
        rig.print();
        assert_eq!(at, ok(&rig), "the dialog grows");
        let row = |rig: &Rig, name: &str| rig.controls.iter().find(|c| c.key.1 == name).cloned().unwrap();
        let (abs, acos) = (row(&rig, "ABS"), row(&rig, "ACOS"));
        assert!(abs.rect.min.x == acos.rect.min.x && acos.rect.min.y > abs.rect.max.y, "side by side: {:?} {:?}", abs.rect, acos.rect);
        let before = rig.print();
        assert!(rig.click(&acos));
        assert!(!before.unchanged(&rig.print()));
    });
}

/// What has nothing to act on is disabled rather than doing nothing, and
/// a preset already in place shows chosen.
#[test]
fn disabled_without_target() {
    isolated("disabled_without_target", || {
        let is = |rig: &Rig, name: &str, want: bool| assert_eq!(enabled(rig, name), Some(want), "{name}");
        // Slides: no object picked, the first slide, nothing copied.
        let rig = Rig::new(State::Slides, 0);
        for name in ["Bold", "Center", "Move slide up", "Paste"] {
            is(&rig, name, false);
        }
        let rig = Rig::new(State::SlidesSelected, 0);
        is(&rig, "Bold", true);
        // A document without ink; a selection over several paragraphs.
        let mut rig = Rig::new(State::DocSelected, 0);
        is(&rig, "Phonetic Guide…", false);
        click_named(&mut rig, "Draw");
        is(&rig, tr("ink.select_all"), false);
        // Find with nothing typed.
        let mut rig = Rig::new(State::ClassicDoc, 0);
        click_named(&mut rig, "Find…");
        is(&rig, "Previous", false);
        is(&rig, "Next", false);
        let mut rig = Rig::new(State::ClassicDoc, 0);
        click_named(&mut rig, "Borders and shading…");
        assert!(rig.controls.iter().any(|c| c.key.1 == tr("table.preset_none") && c.on));
        // A plan with one task picked: nothing to link.
        let mut rig = Rig::new(State::Plan, 0);
        click_named(&mut rig, "Planning");
        is(&rig, tr("plan.link"), false);
    });
}

/// Clicks through `path` ("name" or "name #n", the n-th of that name).
fn click_path(rig: &mut Rig, state: State, path: &[&str]) {
    for step in path {
        let c = nth_named(rig, step).unwrap_or_else(|| {
            let word = step.split_whitespace().next().unwrap_or(step);
            let near: Vec<&str> = rig.controls.iter().map(|c| c.key.1.as_str()).filter(|n| n.contains(word)).collect();
            panic!("{state:?}: no \"{step}\" (near: {near:?})")
        });
        assert!(rig.click(&c), "{state:?}: {step}");
    }
}

/// The control called `step`: "name", or "name #n" for the n-th of them.
fn nth_named(rig: &Rig, step: &str) -> Option<Control> {
    let (name, n) = step.split_once(" #").map_or((step, 0), |(a, n)| (a, n.parse().unwrap()));
    rig.controls.iter().filter(|c| c.key.1 == name).nth(n).cloned()
}

/// Buttons with nothing to do where they show are disabled: stepping
/// past the ends, clearing what is empty, adding nothing.
#[test]
fn disabled_with_nothing_to_do() {
    isolated("disabled_with_nothing_to_do", || {
        let plan = ["Insert", "Project Plan", "Blank Plan", "Planning"];
        let chart = ["Insert", "Clustered Column", "Chart Design"];
        let (select_data, change_type) = ([chart[0], chart[1], chart[2], "Select Data"], [chart[0], chart[1], chart[2], "Change Chart Type"]);
        let cases: &[(State, &[&str], &[&str])] = &[
            // Outline: at the top, all selected, no headings to fold.
            (State::Doc, &["View", "Outline"], &["Move Up", " + ", " − "]),
            (State::DocSelected, &["View", "Outline"], &["Move Up", "Move Down"]),
            // A new table's columns are all of one width.
            (State::Doc, &["Insert", "Table", "Insert Table", "Table Layout"], &["Distribute Columns"]),
            (State::DocObjects, &["Review", "Check Accessibility", "Add alt text", "Effects"], &["Reset effects"]),
            (State::Doc, &["Find", "More >>", "Format…"], &["No formatting"]),
            (State::Doc, &["Review", "AutoCorrect Options", "(c)"], &["Replace"]),
            (State::Sheet, &select_data, &["Apply"]),
            // One series: nothing to combine.
            (State::Sheet, &change_type, &["Combo"]),
            (State::Sheet, &["Data", "Form", "Criteria"], &["Clear", "Restore"]),
            // A blank plan on an empty sheet: no tasks yet.
            (State::SheetSelected, &plan, &["Delete Task", "Unlink Tasks", "Scroll to Task"]),
            (State::Plan, &["Planning", "Project Settings"], &["Add #1"]),
            (State::Doc, &["View", "Read Mode"], &["◀", "▶"]),
            (State::Sheet, &["Formulas", "Error checking"], &["Previous", "Next"]),
            (State::Sheet, &["Data", "Consolidate"], &["Add"]),
            (State::Doc, &["Layout", "Tabs"], &["Clear", "Clear all"]),
            (State::Slides, &["Animations", "Animation Pane"], &["▲", "▼", "Remove"]),
            (State::Db, &["+ #3"], &["OK"]),
            (State::Pdf, &["Fill & Sign", "Signature", "Draw"], &["Clear"]),
            (State::Doc, &["Review", "Thesaurus"], &["Search"]),
        ];
        for (state, path, off) in cases {
            let mut rig = Rig::new(*state, 0);
            click_path(&mut rig, *state, path);
            for name in *off {
                assert_eq!(nth_named(&rig, name).map(|c| c.enabled), Some(false), "{state:?} {path:?}: {name}");
            }
        }
    });
}

/// What these do shows: in the window, or in what the audit reads of
/// the views (the cell being edited).
#[test]
fn effects_seen() {
    isolated("effects_seen", || {
        let cases: &[(State, &[&str])] = &[
            (State::Plan, &["Formulas", "Error checking", "Edit in formula bar"]),
            // In the first row: an empty paragraph above the table.
            (State::Doc, &["Insert", "Table", "Insert Table", "Table Layout", "Split Table"]),
            // The note's number goes to its mark, selected.
            (State::Doc, &["References", "Insert Footnote", "1"]),
            (State::Doc, &["References", "Insert Endnote", "i"]),
            // The last task of a plan goes too, leaving one empty row.
            (State::Sheet, &["Insert", "Project Plan", "Blank Plan", "Planning", "Delete Task"]),
        ];
        for (state, path) in cases {
            let mut rig = Rig::new(*state, 0);
            let (last, before) = path.split_last().unwrap();
            click_path(&mut rig, *state, before);
            let c = nth_named(&rig, last).unwrap_or_else(|| panic!("{state:?}: no \"{last}\""));
            let was = rig.print();
            assert!(rig.click(&c), "{state:?}: {last}");
            assert!(!was.unchanged(&rig.print()), "{state:?} {path:?}: nothing shows");
        }
    });
}

/// What is chosen already shows chosen: the columns preset of the text,
/// the symbol picked among the recent ones.
#[test]
fn chosen_shows() {
    isolated("chosen_shows", || {
        let mut rig = Rig::new(State::Doc, 0);
        click_path(&mut rig, State::Doc, &["Layout", "Columns", "More columns…"]);
        assert_eq!(nth_named(&rig, "One").map(|c| c.on), Some(true), "One column");
        assert_eq!(nth_named(&rig, "Two").map(|c| c.on), Some(false), "Two columns");
        let mut rig = Rig::new(State::Doc, 0);
        click_path(&mut rig, State::Doc, &["Insert", "Symbol", "Insert #1"]);
        // The grid's first symbol (¡), inserted: chosen in the grid and
        // among the recent ones.
        let on = rig.controls.iter().filter(|c| c.key.1 == "Inverted Exclamation Mark" && c.on).count();
        assert_eq!(on, 2, "¡ chosen in the grid and the recent ones");
    });
}


/// Read Aloud always shows something: the player, or why it can't read
/// (in tests a silent engine reads, done at once).
#[test]
fn read_aloud_shows() {
    isolated("read_aloud_shows", || {
        crate::speech::start(&egui::Context::default());
        for _ in 0..100 {
            if !matches!(crate::speech::status(), crate::speech::Status::Loading) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        for state in [State::Doc, State::DocSelected] {
            let mut rig = Rig::new(state, 0);
            click_named(&mut rig, "Review");
            let before = rig.print();
            click_named(&mut rig, tr("speak.button"));
            assert!(!before.unchanged(&rig.print()), "{state:?}");
        }
    });
}

/// A context menu stays open when the pointer leaves the window (the
/// cells' closed at once).
#[test]
fn context_menu_stays_open() {
    isolated("context_menu_stays_open", || {
        for state in [State::Sheet, State::Doc] {
            let mut rig = Rig::new(state, 0);
            let view = rig.areas.iter().max_by_key(|a| a.rect.area() as i64).cloned().expect("the view");
            // The click ends with the pointer gone.
            assert_eq!(rig.reach(&Control::right_click(&view, view.rect.center(), "the view")), Reach::Pointer, "{state:?}");
            assert!(egui::Popup::is_any_open(&rig.ctx), "{state:?}: the context menu closed");
        }
    });
}

/// What the page's context menu cuts or deletes is gone from the page
/// drawn in the same frame (it was drawn from the old layout, and
/// panicked).
#[test]
fn page_context_menu_edits() {
    isolated("page_context_menu_edits", || {
        for (state, at, path) in [(State::DocSelected, (0.0, 0.0), &["Cut"][..]), (State::DocObjects, (0.0, 415.0), &["Delete ⏵", "Delete Table"][..])] {
            let mut rig = Rig::new(state, 0);
            let page = rig.areas.iter().find(|a| a.key.1 == "Document text").cloned().expect("the page");
            let p = pos2(page.rect.min.x + 5.0 + at.0 * page.rect.width(), page.rect.min.y + 5.0 + at.1);
            assert_eq!(rig.reach(&Control::right_click(&page, p, "the page")), Reach::Pointer);
            for entry in path {
                let c = rig.controls.iter().find(|c| c.key.1.starts_with(entry)).cloned().unwrap_or_else(|| panic!("{state:?}: no {entry}"));
                assert!(rig.click(&c), "{state:?}: {entry}");
            }
        }
    });
}

/// The audit finds the page's context menus, found again in a new
/// window (where the widgets have other ids).
#[test]
fn finds_context_menus() {
    isolated("finds_context_menus", || {
        let base = Rig::new(State::Doc, 0).controls.clone();
        let mut report = Report::default();
        let found = context_menus(State::Doc, &base, &mut report);
        assert!(report.panics.is_empty(), "{:?}", report.panics);
        assert!(found.iter().any(|p| p.key.1.starts_with("Document text")), "{:?}", report.menus);
    });
}

/// The document's view in `rig`'s first tab.
fn doc_of(rig: &mut Rig) -> &mut crate::doc_view::DocView {
    match &mut rig.app.tabs[0].view {
        View::Doc(v) => v,
        _ => panic!("not a document"),
    }
}

/// A press at `from`, the pointer moved to `to`, and the release there.
fn drag(rig: &mut Rig, from: Pos2, to: Pos2) {
    let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    rig.frame(vec![Event::PointerMoved(from)]);
    rig.frame(vec![button(from, true)]);
    rig.frame(vec![Event::PointerMoved(to)]);
    rig.frame(vec![button(to, false)]);
    rig.frame(vec![Event::PointerGone]);
    rig.settle();
}

/// Clicks `name` `n` times in a row (twice: a double click).
fn click_times(rig: &mut Rig, name: &str, n: usize) {
    let c = rig.controls.iter().find(|c| c.key.1 == name).cloned().unwrap_or_else(|| panic!("no control \"{name}\""));
    let at = rig.aim(&c).expect("out of view");
    let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    for _ in 0..n {
        rig.frame(vec![press(true)]);
        rig.frame(vec![press(false)]);
    }
    rig.frame(vec![Event::PointerGone]);
    rig.settle();
}

/// A quick click at `at`: press and release in the same frame.
fn click_at(rig: &mut Rig, at: Pos2) {
    let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    rig.frame(vec![Event::PointerMoved(at)]);
    rig.frame(vec![button(true), button(false)]);
    rig.frame(vec![Event::PointerGone]);
    rig.settle();
}

/// The document's format painter, used as the mouse does: its button,
/// then a click on a word or a drag over text.
#[test]
fn doc_format_painter() {
    isolated("doc_format_painter", || {
        use apex_doc::{ParaKind, Pos, Selection};
        let src = |a: &mut apex_doc::Attrs| (a.bold, a.color, a.size) = (true, Some(0xC00000), Some(32));
        let painted = |a: &apex_doc::Attrs| a.bold && a.color == Some(0xC00000) && a.size == Some(32);
        let caret = Selection::caret;
        for sticky in [false, true] {
            let mut rig = Rig::new(State::Doc, 0);
            let v = doc_of(&mut rig);
            // "first", in the first paragraph, has the formatting to copy.
            v.doc.format_range(Selection { anchor: Pos::new(0, 4), cursor: Pos::new(0, 9) }, &src);
            v.set_selection(caret(Pos::new(0, 6)));
            rig.settle();
            click_times(&mut rig, tr("toolstrip.brush"), if sticky { 2 } else { 1 });
            assert_eq!(doc_of(&mut rig).brush_on(), Some(sticky), "the button starts the painter");
            let undo = doc_of(&mut rig).doc.undo_len();
            // A click in "more": the whole word.
            let at = doc_of(&mut rig).screen_pos(Pos::new(1, 21));
            click_at(&mut rig, at);
            let p = &doc_of(&mut rig).doc.paragraphs[1];
            assert!(painted(&p.attrs_at(20)) && painted(&p.attrs_at(23)), "a click paints the word (sticky {sticky})");
            assert!(!painted(&p.attrs_at(19)) && !painted(&p.attrs_at(24)), "only the word");
            assert_eq!(doc_of(&mut rig).doc.undo_len(), undo + 1, "one step to undo");
            assert_eq!(doc_of(&mut rig).brush_on(), sticky.then_some(true), "a single use ends");
            if !sticky {
                continue;
            }
            // Sticky: a drag over "second one" too.
            let (a, b) = (doc_of(&mut rig).screen_pos(Pos::new(1, 2)), doc_of(&mut rig).screen_pos(Pos::new(1, 12)));
            drag(&mut rig, a, b);
            let p = &doc_of(&mut rig).doc.paragraphs[1];
            assert!((3..=12).all(|i| painted(&p.attrs_at(i))), "a drag paints the range");
            assert!(!painted(&p.attrs_at(2)) && !painted(&p.attrs_at(13)), "only the range");
            assert_eq!(doc_of(&mut rig).brush_on(), Some(true), "still on");
            // Escape turns it off.
            rig.frame(vec![Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
            rig.settle();
            assert_eq!(doc_of(&mut rig).brush_on(), None, "Escape ends it");
        }
        // Formatting that comes from a paragraph style (a heading's).
        let heading = ParaKind::Heading(1);
        for whole in [false, true] {
            let mut rig = Rig::new(State::Doc, 0);
            let v = doc_of(&mut rig);
            v.doc.apply_para_style(0, 0, caret(Pos::new(0, 0)), heading, 0);
            // Part of the heading's text: its look; the caret: its paragraph too.
            v.set_selection(if whole { caret(Pos::new(0, 6)) } else { Selection { anchor: Pos::new(0, 4), cursor: Pos::new(0, 9) } });
            rig.settle();
            click_named(&mut rig, tr("toolstrip.brush"));
            let at = doc_of(&mut rig).screen_pos(Pos::new(1, 21));
            click_at(&mut rig, at);
            let d = &doc_of(&mut rig).doc;
            let (h, p) = (&d.paragraphs[0], &d.paragraphs[1]);
            let looks = d.char_format(&h.style, &h.attrs_at(6));
            assert_eq!(d.char_format(&p.style, &p.attrs_at(21)), looks, "the word looks like the heading (whole {whole})");
            assert_eq!(p.style.kind == heading, whole, "the paragraph's style goes only with the caret");
            assert_eq!(d.char_format(&p.style, &p.attrs_at(19)) == looks, whole, "the rest of the paragraph");
        }
    });
}

/// The slides' view in `rig`'s first tab.
fn slides_of(rig: &mut Rig) -> &mut crate::slides_view::SlidesView {
    match &mut rig.app.tabs[0].view {
        View::Slides(v) => v,
        _ => panic!("not a presentation"),
    }
}

/// The presentation's format painter: the title's look on the subtitle,
/// all of it with a click on it, only the word clicked in text being typed.
#[test]
fn slides_format_painter() {
    isolated("slides_format_painter", || {
        use apex_doc::{Pos, Selection};
        // The look at offset `off` of object `id`'s text.
        let look = |rig: &mut Rig, id: u32, off: usize| {
            let v = slides_of(rig);
            v.stop_typing();
            v.start_typing(id);
            v.typing.as_mut().unwrap().sel = Selection::caret(Pos::new(0, off));
            let f = v.current_format();
            v.stop_typing();
            (f.size, f.bold, f.color, f.font)
        };
        for (sticky, typed) in [(false, false), (true, false), (false, true)] {
            let mut rig = Rig::new(State::Slides, 0);
            let v = slides_of(&mut rig);
            let ids: Vec<u32> = v.cur_objects().iter().map(|o| o.id).collect();
            let [title, sub] = ids[..] else { panic!("a title slide: {ids:?}") };
            for (id, text) in [(title, "Big title"), (sub, "Alpha beta gamma")] {
                v.change_object("x", id, |o| *o.text_or_new().unwrap() = apex_slides::TextBody::plain(text));
            }
            v.sel = vec![title];
            rig.settle();
            let (big, small) = (look(&mut rig, title, 1), look(&mut rig, sub, 7));
            assert_ne!(big, small, "the title looks different");
            slides_of(&mut rig).sel = vec![title];
            rig.settle();
            click_times(&mut rig, tr("toolstrip.brush"), if sticky { 2 } else { 1 });
            assert_eq!(slides_of(&mut rig).brush_on(), Some(sticky), "the button starts the painter");
            if typed {
                slides_of(&mut rig).start_typing(sub);
                rig.settle();
            }
            let undo = slides_of(&mut rig).pres.undo_names().len();
            let at = slides_of(&mut rig).screen_rect(sub).expect("the subtitle shows").center();
            click_at(&mut rig, at);
            assert_eq!(slides_of(&mut rig).pres.undo_names().len(), undo + 1, "one step to undo");
            assert_eq!(slides_of(&mut rig).brush_on(), sticky.then_some(true), "a single use ends");
            // The words' looks: all the title's, or in typed text one word's.
            let words = [0, 7, 12].map(|off| look(&mut rig, sub, off) == big);
            let n = words.iter().filter(|w| **w).count();
            assert_eq!(n, if typed { 1 } else { 3 }, "words painted (typed {typed}): {words:?}");
            if sticky {
                rig.frame(vec![Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
                rig.settle();
                assert_eq!(slides_of(&mut rig).brush_on(), None, "Escape ends it");
            }
        }
    });
}

// --- controls every view has -----------------------------------------------------------

/// The controls that don't depend on the kind of file are in every view,
/// under the same name, in the same place and with the same icon: in the
/// File menu (also on the start screen), and in the View tab (toolstrip, the
/// classic interface's View menu and the command search). The dark
/// workspace only where pages, cells or slides are drawn.
#[test]
fn global_controls_in_every_view() {
    isolated("global_controls_in_every_view", || {
        let dots = |k: &'static str| format!("{}…", tr(k));
        let customize = dots("customize.title");
        let file = [dots("options.title"), tr("menu.theme").to_string(), customize.clone(), tr("menu.shortcuts").to_string(), tr("menu.about").to_string(), dots("diag.title"), dots("mcp.title"), dots("policy.title")];
        let view = [(tr("toolstrip.g.screen"), tr("view.fullscreen")), (tr("toolstrip.g.screen"), tr("menu.theme"))];
        let mut seen: HashMap<String, (Vec<String>, Option<crate::icons::Icon>, State)> = HashMap::new();
        let mut missing = Vec::new();
        let mut differ = Vec::new();
        let states = [State::Start, State::Doc, State::Sheet, State::Slides, State::Plan, State::Db, State::Pdf, State::ClassicDoc, State::ClassicSheet];
        for (i, state) in states.into_iter().enumerate() {
            let mut rig = Rig::new(state, 700 + i as u64);
            rig.app.want_commands = true;
            rig.settle();
            let list = rig.app.commands.take().unwrap_or_default();
            let mut wanted: Vec<(&str, &str, String)> = Vec::new();
            // The start screen has the File menu only (customizing needs a tab).
            wanted.extend(file.iter().filter(|l| state != State::Start || **l != customize).map(|l| (super::toolstrip_bar::FILE, "", l.clone())));
            if state != State::Start {
                wanted.extend(view.iter().map(|(g, l)| (super::view_tab::VIEW, *g, l.to_string())));
                // The tab itself shows: a toolstrip tab, or a menu of the menu bar.
                if !rig.controls.iter().any(|c| c.key.1 == tr("menu.view")) {
                    missing.push(format!("{state:?}: the View tab"));
                }
            }
            for (tab, group, label) in wanted {
                let Some(c) = list.iter().find(|c| c.tab == tab && c.entry.label == label && (group.is_empty() || c.entry.place.get(1).is_some_and(|p| p == group))) else {
                    missing.push(format!("{state:?}: {tab} > {group} > {label}"));
                    continue;
                };
                let k = format!("{tab}/{label}");
                match seen.get(&k) {
                    Some((place, icon, first)) if (place, icon) != (&c.entry.place, &c.entry.icon) => differ.push(format!("{label}: {first:?} {place:?} {icon:?}, {state:?} {:?} {:?}", c.entry.place, c.entry.icon)),
                    Some(_) => {}
                    None => {
                        seen.insert(k, (c.entry.place.clone(), c.entry.icon, state));
                    }
                }
            }
            let dark_ws = list.iter().any(|c| c.tab == super::view_tab::VIEW && c.entry.label == tr("dw.title"));
            let workspace = !matches!(state, State::Start | State::Db | State::Pdf);
            if dark_ws != workspace {
                differ.push(format!("{state:?}: the dark workspace {}", if dark_ws { "shows" } else { "is missing" }));
            }
        }
        assert!(missing.is_empty(), "controls every view has, missing:\n{}", missing.join("\n"));
        assert!(differ.is_empty(), "controls every view has, not the same everywhere:\n{}", differ.join("\n"));
    });
}

/// The File menu in groups: the less used commands in submenus (found by
/// the search under them), only those that apply to the active tab, and
/// no separate PDF/A, Open PDF or Open database (the PDF export's dialog
/// has the PDF/A choice, Open opens PDF files and databases).
#[test]
fn file_menu_groups() {
    isolated("file_menu_groups", || {
        let dots = |k: &'static str| format!("{}…", tr(k));
        let (new, export, share, protect, info, tools) = (tr("menu.new"), tr("file.export"), tr("file.share"), tr("file.protect"), tr("file.info"), tr("file.tools"));
        // Label, submenu ("": none) and the states it shows in.
        type Shows = fn(State) -> bool;
        let entries: Vec<(String, &str, Shows)> = vec![
            (tr("menu.save").to_string(), "", |s| s != State::Start),
            (tr("menu.open").to_string(), "", |_| true),
            (tr("menu.new_doc").to_string(), new, |_| true),
            (tr("menu.new_slides").to_string(), new, |_| true),
            (dots("db.new_database"), new, |_| true),
            (dots("convert.title"), tools, |_| true),
            // Beside Export, as Office has it.
            (dots("print.export_pdf"), "", |s| s != State::Start),
            (dots("export.html"), export, |s| s != State::Start),
            (dots("export.epub"), export, |s| matches!(s, State::Doc | State::ClassicDoc)),
            (dots("export.all_csv"), export, |s| matches!(s, State::Sheet | State::ClassicSheet)),
            (dots("slides.export_png"), export, |s| s == State::Slides),
            (dots("page.setup"), "", |s| matches!(s, State::Doc | State::ClassicDoc | State::Sheet | State::ClassicSheet)),
            (tr("mail.send_pdf").to_string(), share, |s| s != State::Start),
            (dots("sign.doc"), protect, |s| s != State::Start),
            (dots("password.encrypt_title"), protect, |s| s != State::Start),
            (dots("props.title"), info, |s| !matches!(s, State::Start | State::Db)),
            (dots("fi.title"), info, |s| !matches!(s, State::Start | State::Db | State::Pdf)),
            (tr("fonts.embed").to_string(), info, |s| matches!(s, State::Doc | State::ClassicDoc)),
            (dots("customize.title"), tools, |s| s != State::Start),
            (dots("enc.open"), tools, |_| true),
            (dots("diag.title"), tools, |_| true),
            (tr("menu.close_tab").to_string(), "", |s| s != State::Start),
        ];
        let gone = [dots("pdf.open_title"), dots("db.open_database"), "PDF/A".to_string(), tr("menu.language").to_string()];
        let mut wrong = Vec::new();
        for (i, state) in [State::Start, State::Doc, State::Sheet, State::Slides, State::Db, State::Pdf, State::ClassicDoc, State::ClassicSheet].into_iter().enumerate() {
            let mut rig = Rig::new(state, 900 + i as u64);
            rig.app.want_commands = true;
            rig.settle();
            let list: Vec<_> = rig.app.commands.take().unwrap_or_default().into_iter().filter(|c| c.tab == super::toolstrip_bar::FILE).collect();
            for (label, sub, shows) in &entries {
                let found = list.iter().find(|c| &c.entry.label == label);
                if found.is_some() != shows(state) {
                    wrong.push(format!("{state:?}: {label} {}", if found.is_some() { "shows" } else { "is missing" }));
                }
                if let Some(c) = found {
                    let place: Vec<&str> = std::iter::once(tr("menu.file")).chain((!sub.is_empty()).then_some(*sub)).collect();
                    if c.entry.place != place {
                        wrong.push(format!("{state:?}: {label} in {:?}, not {place:?}", c.entry.place));
                    }
                }
            }
            for label in &gone {
                if list.iter().any(|c| c.entry.label.contains(label.as_str())) {
                    wrong.push(format!("{state:?}: {label} is still there"));
                }
            }
            // In order: Open, New, the saves, Export to PDF, Export.
            let at = |l: &str| list.iter().position(|c| c.entry.label == l);
            let order = [tr("menu.open").to_string(), tr("menu.new_doc").to_string(), tr("menu.save").to_string(), dots("print.export_pdf"), dots("export.html")];
            let seen: Vec<usize> = order.iter().filter_map(|l| at(l)).collect();
            if !seen.is_sorted() {
                wrong.push(format!("{state:?}: out of order: {order:?} at {seen:?}"));
            }
            // A submenu with nothing that applies isn't there.
            if state == State::Db && list.iter().any(|c| c.entry.label == info) {
                wrong.push("Db: an empty Info submenu".into());
            }
        }
        assert!(wrong.is_empty(), "the File menu:\n{}", wrong.join("\n"));

        // A command of a submenu runs from the search.
        let mut rig = Rig::new(State::Start, 950);
        let ctx = rig.ctx.clone();
        let key = format!("{}/{tools}/{}", tr("menu.file"), dots("diag.title"));
        rig.app.choose(&ctx, crate::palette::Chosen { tab: super::toolstrip_bar::FILE, key, menu: false });
        rig.settle();
        assert!(rig.app.show_diag, "Tools > Diagnostics from the search");
        // Export to PDF shows the export's dialog, with the PDF/A choice.
        let mut rig = Rig::new(State::Doc, 951);
        let ctx = rig.ctx.clone();
        let key = format!("{}/{}", tr("menu.file"), dots("print.export_pdf"));
        rig.app.choose(&ctx, crate::palette::Chosen { tab: super::toolstrip_bar::FILE, key, menu: false });
        rig.settle();
        assert!(rig.app.print.as_ref().is_some_and(|d| d.export), "Export to PDF opens the export dialog");
        assert!(rig.controls.iter().any(|c| c.key.1 == tr("pdf.pdfa")), "the export dialog shows PDF/A");
        // Open takes Access and dBase files into a database, as Open
        // database did; the rest (PDF too) opens as before.
        for (p, db) in [("a.MDB", true), ("b.accdb", true), ("c.dbf", true), ("d.pdf", false), ("e.apexdb", false), ("f.xlsx", false)] {
            assert_eq!(App::imported_db(std::path::Path::new(p)), db, "{p}");
        }
    });
}

/// Opened and left alone, a dialog stays where it is.
#[test]
fn dialogs_stay_put() {
    isolated("dialogs_stay_put", || {
        let cases: &[(State, &[&str], &str)] = &[(State::Doc, &["File", "About ApexDesk", "Third-party licenses"], "Close"), (State::Doc, &["File", "Print"], ">"), (State::Doc, &["File", "Print", "Booklet (fold the stack in half)"], ">"), (State::Doc, &["File", "Print", "Tagged PDF (accessible)"], ">"), (State::Doc, &["File", "Print", "Booklet (fold the stack in half)"], "Cancel"), (State::Pdf, &["Print"], "OK")];
        for (state, path, name) in cases {
            let mut rig = Rig::new(*state, 0);
            click_path(&mut rig, *state, path);
            let at = |rig: &Rig| rig.controls.iter().filter(|c| c.key.1 == *name).map(|c| c.bounds).collect::<Vec<_>>();
            let a = at(&rig);
            for i in 0..30 {
                rig.frame(Vec::new());
                if at(&rig) != a {
                    panic!("{path:?}: {name} moved at frame {i}: {a:?} → {:?}", at(&rig));
                }
            }
        }
    });
}

/// The database's small windows close with Escape.
#[test]
fn escape_closes_db_windows() {
    isolated("escape_closes_db_windows", || {
        for path in [&["+ #3"][..], &["+ #6"], &["External Data", "Link to Server"]] {
            let mut rig = Rig::new(State::Db, 0);
            click_path(&mut rig, State::Db, path);
            let before = rig.windows.clone();
            for pressed in [true, false] {
                rig.frame(vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE }]);
            }
            rig.settle();
            assert!(rig.windows.len() < before.len(), "{path:?}: {before:?} → {:?}", rig.windows);
        }
    });
}
