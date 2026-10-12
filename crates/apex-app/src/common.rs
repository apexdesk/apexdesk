//! Toolbar pieces shared by both editors: the file group, undo history,
//! font family and size pickers and the zoom control.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Id, Key, RichText, vec2};

use crate::fonts;
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::widgets::{self, Face};

/// Requests from a view to the application.
#[derive(Clone, Debug)]
pub enum AppCmd {
    NewDoc,
    NewSheet,
    NewSlides,
    Open,
    OpenPath(PathBuf),
    Save,
    Print,
    ExportPdf,
    /// File > Export > PDF: the export dialog with its options (PDF/A).
    ExportPdfDialog,
    /// Opens a new document (made by Compare, mail merge, labels...) in a
    /// new tab, and exports it as PDF when asked.
    OpenDoc(crate::doc_view::DocToOpen, bool),
    /// Opens a sheet of labels in a new tab, named "Labels N".
    OpenLabels(crate::doc_view::DocToOpen),
    /// Shows or hides the formula bar.
    FormulaBar,
    Fullscreen,
    /// Light, dark or (None) the system's.
    Theme(Option<bool>),
    /// View > Dark Workspace: pages, cells and slides dark too.
    DarkCanvas,
    NewWindow,
    /// Compares the workbook with another window (tab id), or stops.
    SideBySide(Option<u64>),
    SyncScroll,
    /// Move or Copy: sheet `sheet` of the active workbook goes to another
    /// one (tab id; None: a new workbook) before sheet `before`.
    SheetToBook { sheet: usize, target: Option<u64>, before: usize, copy: bool },
    /// View > Macros: the macro editor of the active tab.
    Macros,
    /// Run Macro (Alt+F8): the macros of the active workbook.
    RunMacro,
    /// Opens a notebook in a tab of its own, staying where it was asked
    /// (a book shows its pages beside the manuscript).
    OpenNoteBeside(PathBuf),
    /// Goes to page `1` of the notebook whose view is `0`.
    ShowNotePage(u64, i64),
}

/// What the application lends to a view for one frame.
pub struct Shell {
    pub cmd: Option<AppCmd>,
    pub show_formula_bar: bool,
    /// File name of the tab (for header and footer fields).
    pub file_name: String,
    pub fullscreen: bool,
    pub dark: Option<bool>,
    /// Other workbook windows to compare with (tab id, name).
    pub others: Vec<(u64, String)>,
    /// Other open workbooks and their sheets, for Move or Copy (only
    /// filled while its dialog is open).
    pub books: Vec<(u64, String, Vec<String>)>,
    /// Side by side is on, and scrolls in sync.
    pub side: Option<bool>,
}

// --- toolstrip groups both views have ---------------------------------------------------

/// The Zoom group: zoom, 100 % and zoom in/out.
pub fn zoom_group(ui: &mut egui::Ui, zoom: &mut f32, accent: egui::Color32) {
    use crate::toolstrip;
    toolstrip::large_menu(ui, Icon::ZoomIn, tr("toolbar.zoom"), "", accent, |ui| {
        for p in [50.0, 75.0, 100.0, 125.0, 150.0, 200.0, 300.0] {
            if ui.radio((*zoom * 100.0 - p).abs() < 0.5, format!("{p:.0}%")).clicked() {
                *zoom = p / 100.0;
                ui.close();
            }
        }
    });
    if toolstrip::large(ui, Icon::Zoom100, "100%", tr("menu.zoom_reset"), accent, (*zoom - 1.0).abs() < 0.001).clicked() {
        *zoom = 1.0;
    }
    toolstrip::column(ui, |ui| {
        if toolstrip::small(ui, Some(Icon::ZoomIn), tr("menu.zoom_in"), &format!("{} (Ctrl++)", tr("menu.zoom_in")), accent, false).clicked() {
            *zoom = ((*zoom * 10.0).round() / 10.0 + 0.1).min(4.0);
        }
        if toolstrip::small(ui, Some(Icon::ZoomOut), tr("menu.zoom_out"), &format!("{} (Ctrl+-)", tr("menu.zoom_out")), accent, false).clicked() {
            *zoom = ((*zoom * 10.0).round() / 10.0 - 0.1).max(0.25);
        }
    });
}

/// View Side by Side and Synchronous Scrolling (workbooks).
pub fn side_by_side_buttons(ui: &mut egui::Ui, shell: &mut Shell, accent: egui::Color32) {
    use crate::toolstrip;
    toolstrip::column(ui, |ui| {
        let on = shell.side.is_some();
        // Without another workbook window (or side by side off) the
        // buttons stay: their tip and a click say what they need.
        if on || shell.others.len() <= 1 {
            let hover = if on || !shell.others.is_empty() { "" } else { tr("window.side_by_side_need") };
            if toolstrip::small(ui, Some(Icon::SideBySide), tr("window.side_by_side"), hover, accent, on).clicked() {
                shell.cmd = Some(AppCmd::SideBySide(if on { None } else { shell.others.first().map(|o| o.0) }));
            }
        } else {
            let others = shell.others.clone();
            let mut pick = None;
            toolstrip::small_menu(ui, Some(Icon::SideBySide), tr("window.side_by_side"), "", accent, |ui| {
                ui.label(RichText::new(tr("window.compare_with")).weak());
                for (id, name) in &others {
                    if ui.button(name).clicked() {
                        pick = Some(*id);
                        ui.close();
                    }
                }
            });
            if pick.is_some() {
                shell.cmd = Some(AppCmd::SideBySide(pick));
            }
        }
        let hover = if on { "" } else { tr("window.sync_scroll_need") };
        if toolstrip::small(ui, Some(Icon::Window), tr("window.sync_scroll"), hover, accent, shell.side == Some(true)).clicked() {
            shell.cmd = Some(AppCmd::SyncScroll);
        }
    });
}

/// The Macros group of the View tab (documents and workbooks).
pub fn macros_group(b: &mut crate::toolstrip::Body, shell: &mut Shell) {
    use crate::toolstrip;
    let a = b.accent();
    b.group("macros.title", Icon::Field, |ui| {
        if toolstrip::large(ui, Icon::Field, tr("macros.title"), tr("macros.tip"), a, false).clicked() {
            shell.cmd = Some(AppCmd::Macros);
        }
        if toolstrip::large(ui, Icon::SpeakPlay, tr("macros.run_macro"), tr("macros.run_tip"), a, false).clicked() {
            shell.cmd = Some(AppCmd::RunMacro);
        }
    });
}

/// The Screen group every View tab has: full screen, the interface's
/// theme and, where pages, cells or slides are drawn (`workspace`), the
/// dark workspace.
pub fn screen_group(b: &mut crate::toolstrip::Body, shell: &mut Shell, workspace: bool) {
    use crate::toolstrip;
    let a = b.accent();
    b.group("toolstrip.g.screen", Icon::Fullscreen, |ui| {
        if toolstrip::large(ui, Icon::Fullscreen, tr("view.fullscreen"), &format!("{} (F11)", tr("view.fullscreen")), a, shell.fullscreen).clicked() {
            shell.cmd = Some(AppCmd::Fullscreen);
        }
        let mut pick = None;
        toolstrip::large_menu(ui, Icon::Theme, tr("menu.theme"), "", a, |ui| pick = theme_choices(ui, shell.dark));
        if let Some(v) = pick {
            shell.cmd = Some(AppCmd::Theme(v));
        }
        if workspace && toolstrip::large(ui, Icon::Theme, tr("dw.title"), tr("dw.tip"), a, crate::theme::dark_canvas_option()).clicked() {
            shell.cmd = Some(AppCmd::DarkCanvas);
        }
    });
}

/// The interface's themes as radio buttons (`dark`: the one in use); the
/// one clicked.
pub fn theme_choices(ui: &mut egui::Ui, dark: Option<bool>) -> Option<Option<bool>> {
    let mut pick = None;
    for (v, key) in [(None, "menu.theme_system"), (Some(false), "menu.theme_light"), (Some(true), "menu.theme_dark")] {
        if ui.radio(dark == v, tr(key)).clicked() {
            pick = Some(v);
            ui.close();
        }
    }
    pick
}

/// Undo or redo with a dropdown of the history (labels are i18n keys, most
/// recent first). Returns how many steps to undo/redo.
pub fn history_button(ui: &mut egui::Ui, redo: bool, labels: &[&'static str]) -> usize {
    let (icon, key, keys) = if redo { (Icon::Redo, "edit.redo", "Ctrl+Y") } else { (Icon::Undo, "edit.undo", "Ctrl+Z") };
    let mut steps = 0;
    let hover = format!("{} ({keys})", tr(key));
    ui.add_enabled_ui(!labels.is_empty(), |ui| {
        let hover_id = ui.id().with(("hist", redo));
        if crate::toolstrip::split(ui, Face::Icon(icon), false, &hover, |ui| {
            let mut hot: Option<usize> = None;
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                ui.set_min_width(200.0);
                let marked: Option<usize> = ui.data(|d| d.get_temp(hover_id));
                for (i, l) in labels.iter().take(100).enumerate() {
                    let on = marked.is_some_and(|m| i <= m);
                    let r = ui.add(egui::Button::selectable(on, tr_label(l)));
                    if r.hovered() {
                        hot = Some(i);
                    }
                    if r.clicked() {
                        steps = i + 1;
                        ui.close();
                    }
                }
            });
            ui.separator();
            let n = hot.map_or(0, |h| h + 1);
            let key = if redo { "toolbar.redo_n" } else { "toolbar.undo_n" };
            ui.label(RichText::new(trf(key, &[("n", &n)])).weak());
            ui.data_mut(|d| d.insert_temp(hover_id, hot));
        }) {
            steps = 1;
        }
    });
    steps
}

fn tr_label(key: &'static str) -> &'static str {
    tr(key)
}

// --- fonts -----------------------------------------------------------------------------

static RECENT_FONTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static PREFS_DIRTY: AtomicBool = AtomicBool::new(false);

pub fn recent_fonts() -> Vec<String> {
    RECENT_FONTS.lock().map(|v| v.clone()).unwrap_or_default()
}

pub fn set_recent_fonts(v: Vec<String>) {
    if let Ok(mut r) = RECENT_FONTS.lock() {
        *r = v;
    }
}

fn push_recent_font(name: &str) {
    if let Ok(mut r) = RECENT_FONTS.lock() {
        r.retain(|x| x != name);
        r.insert(0, name.to_string());
        r.truncate(6);
    }
    PREFS_DIRTY.store(true, Ordering::Relaxed);
}

pub fn mark_prefs_dirty() {
    PREFS_DIRTY.store(true, Ordering::Relaxed);
}

/// True once after recent fonts or colors changed (to save the settings).
pub fn take_prefs_dirty() -> bool {
    PREFS_DIRTY.swap(false, Ordering::Relaxed) | widgets::take_colors_dirty()
}

/// The font family box: typing filters the list; recent fonts come first
/// and each name is drawn in its own font.
pub fn font_picker(ui: &mut egui::Ui, id: Id, current: &str, width: f32) -> Option<String> {
    font_picker_opts(ui, id, current, width, false)
}

/// A font picker listing the theme's headings and body fonts first; those
/// come back as the tokens `+mj-lt` / `+mn-lt`.
pub fn themed_font_picker(ui: &mut egui::Ui, id: Id, current: &str, width: f32) -> Option<String> {
    font_picker_opts(ui, id, current, width, true)
}

fn font_picker_opts(ui: &mut egui::Ui, id: Id, current: &str, width: f32, themed: bool) -> Option<String> {
    let mut out = None;
    let id = crate::toolstrip::own_id(id);
    let text_id = id.with("text");
    let focused = ui.memory(|m| m.has_focus(text_id));
    let mut text: String = if focused { ui.data(|d| d.get_temp(text_id)).unwrap_or_default() } else { current.to_string() };
    let list = BoxList::new(ui, id.with("popup"));
    // A longer name (a translated "Default font") widens the box a little
    // rather than being cut.
    let font = egui::TextStyle::Body.resolve(ui.style());
    let need = ui.painter().layout_no_wrap(current.to_string(), font, egui::Color32::PLACEHOLDER).size().x + 12.0;
    let width = width.max(need.min(width * 1.5));
    let resp = ui.add(egui::TextEdit::singleline(&mut text).id(text_id).desired_width(width).hint_text(tr("toolbar.font")));
    let resp = resp.on_hover_text(tr("toolbar.font"));
    if resp.gained_focus() {
        text.clear();
        list.open(ui);
    }
    if resp.changed() {
        list.open(ui);
    }
    let mut commit = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let filter = text.trim().to_lowercase();
    let fams = fonts::families();
    let recent = recent_fonts();
    let matches: Vec<&String> = fams.iter().filter(|f| filter.is_empty() || f.to_lowercase().contains(&filter)).collect();
    if commit {
        // Enter picks the best match (or keeps an unknown name as typed).
        let pick = matches.iter().find(|f| f.to_lowercase() == filter).or_else(|| matches.iter().find(|f| f.to_lowercase().starts_with(&filter))).map(|s| s.to_string());
        let pick = pick.unwrap_or_else(|| text.trim().to_string());
        if !pick.is_empty() {
            out = Some(pick);
        }
        commit = false;
    }
    let _ = commit;
    list.show(ui, &resp, width.max(220.0), |ui| {
        let row = |ui: &mut egui::Ui, name: &str| {
            let font = fonts::preview_font(name, 15.0);
            let r = ui.add(egui::Button::selectable(name.eq_ignore_ascii_case(current), RichText::new(name).font(font)).min_size(vec2(ui.available_width(), 0.0)));
            r.clicked()
        };
        if themed {
            if let Some(t) = crate::themes_ui::theme_font_rows(ui, current, &filter) {
                out = Some(t.to_string());
            }
        }
        let brand = crate::brand::get().fonts;
        if filter.is_empty() && !brand.is_empty() {
            ui.label(RichText::new(tr("toolbar.brand_fonts")).small().weak());
            for f in &brand {
                if row(ui, f) {
                    out = Some(f.clone());
                }
            }
            ui.separator();
        }
        if filter.is_empty() && !recent.is_empty() {
            ui.label(RichText::new(tr("toolbar.recent_fonts")).small().weak());
            for f in &recent {
                if row(ui, f) {
                    out = Some(f.clone());
                }
            }
            ui.separator();
        }
        if fams.is_empty() {
            ui.label(RichText::new(tr("toolbar.fonts_loading")).weak());
        }
        let h = 24.0;
        egui::ScrollArea::vertical().max_height(360.0).show_rows(ui, h, matches.len(), |ui, range| {
            for f in &matches[range] {
                if row(ui, f) {
                    out = Some((*f).clone());
                }
            }
        });
    });
    list.reopen_after_focus_click(ui, &resp);
    if !focused || resp.changed() || resp.gained_focus() {
        ui.data_mut(|d| d.insert_temp(text_id, text));
    }
    if let Some(f) = &out {
        if !f.starts_with('+') {
            push_recent_font(f);
        }
        list.close(ui);
        ui.memory_mut(|m| m.surrender_focus(text_id));
    }
    out
}

/// The list of a font or size box. On the toolstrip it is a popup of
/// egui's memory; inside a menu (a folded toolstrip group) it keeps its
/// own open state and shows as a submenu of that menu, since egui keeps
/// one memory popup at a time and opening the list would close the menu.
struct BoxList {
    id: Id,
    in_menu: bool,
}

impl BoxList {
    fn new(ui: &egui::Ui, id: Id) -> BoxList {
        BoxList { id, in_menu: egui::containers::menu::is_in_menu(ui) }
    }

    fn open_key(&self) -> Id {
        self.id.with("open")
    }

    fn set(&self, ui: &egui::Ui, open: bool) {
        if self.in_menu {
            ui.data_mut(|d| d.insert_temp(self.open_key(), open));
        } else if open {
            egui::Popup::open_id(ui.ctx(), self.id);
        } else {
            egui::Popup::close_id(ui.ctx(), self.id);
        }
    }

    fn open(&self, ui: &egui::Ui) {
        self.set(ui, true);
    }

    fn close(&self, ui: &egui::Ui) {
        self.set(ui, false);
    }

    fn show(&self, ui: &egui::Ui, resp: &egui::Response, width: f32, content: impl FnOnce(&mut egui::Ui)) {
        let popup = egui::Popup::from_response(resp).id(self.id).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).width(width);
        if !self.in_menu {
            popup.open_memory(None).show(content);
            return;
        }
        let mut open = ui.data(|d| d.get_temp(self.open_key())).unwrap_or(false);
        if open {
            // The menu's open submenu: a click in the list leaves it open.
            egui::containers::menu::MenuState::from_ui(ui, |s, _| s.open_item = Some(self.id));
        }
        popup.open_bool(&mut open).show(content);
        ui.data_mut(|d| d.insert_temp(self.open_key(), open));
    }

    /// The press that focuses a box opens its list, and the release lands
    /// on the box, outside the list, so egui would close it at once: open
    /// again.
    fn reopen_after_focus_click(&self, ui: &egui::Ui, resp: &egui::Response) {
        if resp.clicked() && resp.has_focus() {
            self.open(ui);
        }
    }
}

pub const FONT_SIZES: [f32; 16] = [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0, 26.0, 28.0, 36.0, 48.0, 72.0];

/// Next size of the list up or down (Ctrl+Shift+> / <).
pub fn step_size(cur: f32, up: bool) -> f32 {
    if up {
        FONT_SIZES.iter().copied().find(|&s| s > cur + 0.01).unwrap_or((cur + 10.0).min(400.0))
    } else {
        FONT_SIZES.iter().rev().copied().find(|&s| s < cur - 0.01).unwrap_or((cur - 1.0).max(1.0))
    }
}

pub fn fmt_size(v: f32) -> String {
    let s = format!("{:.1}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if crate::i18n::current() == "es" { s.replace('.', ",") } else { s }
}

/// An editable font size box with a list of common sizes.
pub fn size_box(ui: &mut egui::Ui, id: Id, current: f32) -> Option<f32> {
    let mut out = None;
    let id = crate::toolstrip::own_id(id);
    let text_id = id.with("text");
    let focused = ui.memory(|m| m.has_focus(text_id));
    let mut text: String = if focused { ui.data(|d| d.get_temp(text_id)).unwrap_or_default() } else { fmt_size(current) };
    let list = BoxList::new(ui, id.with("popup"));
    let resp = ui.add(egui::TextEdit::singleline(&mut text).id(text_id).desired_width(34.0)).on_hover_text(tr("toolbar.font_size"));
    if resp.gained_focus() {
        list.open(ui);
    }
    if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
        match text.trim().replace(',', ".").parse::<f32>() {
            Ok(v) if (1.0..=400.0).contains(&v) => out = Some((v * 2.0).round() / 2.0),
            _ => {}
        }
    }
    list.show(ui, &resp, 56.0, |ui| {
        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
            for s in FONT_SIZES {
                if ui.add(egui::Button::selectable((s - current).abs() < 0.01, fmt_size(s)).min_size(vec2(50.0, 0.0))).clicked() {
                    out = Some(s);
                }
            }
        });
    });
    list.reopen_after_focus_click(ui, &resp);
    ui.data_mut(|d| d.insert_temp(text_id, text));
    if out.is_some() {
        list.close(ui);
        ui.memory_mut(|m| m.surrender_focus(text_id));
    }
    out
}

// --- zoom --------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    PageWidth,
    WholePage,
}

/// Zoom slider, − and + buttons and the percentage with presets.
pub fn zoom_control(ui: &mut egui::Ui, zoom: &mut f32, doc: bool) -> Option<Fit> {
    let mut fit = None;
    // Right-to-left layout: items appear in reverse order.
    if ui.add(egui::Button::new("+").frame(false)).on_hover_text(tr("menu.zoom_in")).clicked() {
        *zoom = ((*zoom * 10.0).round() / 10.0 + 0.1).min(4.0);
    }
    let mut pct = *zoom * 100.0;
    let slider = egui::Slider::new(&mut pct, 25.0..=400.0).show_value(false).logarithmic(true);
    let r = ui.add_sized(vec2(110.0, 16.0), slider);
    if r.changed() {
        // Snap to 100 % near it.
        *zoom = if (pct - 100.0).abs() < 4.0 { 1.0 } else { pct / 100.0 };
    }
    if ui.add(egui::Button::new("−").frame(false)).on_hover_text(tr("menu.zoom_out")).clicked() {
        *zoom = ((*zoom * 10.0).round() / 10.0 - 0.1).max(0.25);
    }
    let label = ui.add(egui::Button::new(format!("{:.0}%", *zoom * 100.0)).frame(false)).on_hover_text(tr("toolbar.zoom"));
    egui::Popup::menu(&label).show(|ui| {
        for p in [50.0, 75.0, 100.0, 125.0, 150.0, 200.0] {
            if ui.button(format!("{p:.0}%")).clicked() {
                *zoom = p / 100.0;
            }
        }
        if doc {
            ui.separator();
            if ui.button(tr("zoom.page_width")).clicked() {
                fit = Some(Fit::PageWidth);
            }
            if ui.button(tr("zoom.whole_page")).clicked() {
                fit = Some(Fit::WholePage);
            }
        }
    });
    fit
}

/// The decimal separator of the user's locale.
pub fn decimal() -> char {
    apex_sheet::Locale::from_env().decimal
}

/// Today's date and the current time in the UI language's short formats.
pub fn date_time_now() -> (String, String) {
    date_time_of(local_now())
}

/// A local time (see [`local_time`]) in the UI language's short formats.
pub fn date_time_of((y, mo, d, h, mi): (i64, u32, u32, u32, u32)) -> (String, String) {
    let date = trf("doc.date_format", &[("d", &d), ("dd", &format!("{d:02}")), ("m", &mo), ("mm", &format!("{mo:02}")), ("y", &y)]);
    let (h12, ampm) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    let time = trf("doc.time_format", &[("H", &h), ("HH", &format!("{h:02}")), ("h12", &h12), ("MM", &format!("{mi:02}")), ("ampm", &ampm)]);
    (date, time)
}

pub fn local_now() -> (i64, u32, u32, u32, u32) {
    local_time(std::time::SystemTime::now())
}

/// The last second of the year 9999: times past it (from a file) are
/// shown as it, and a `SystemTime` can hold it everywhere.
pub(crate) const LAST_SECOND: i64 = 253_402_300_799;

/// Year, month, day, hour and minute of `t` in local time.
pub fn local_time(t: std::time::SystemTime) -> (i64, u32, u32, u32, u32) {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let local = secs.saturating_add(utc_offset(secs));
    let days = local.div_euclid(86_400);
    let sod = local.rem_euclid(86_400);
    let (y, m, d) = apex_sheet::format::serial_to_date((days + 25_569) as f64);
    (y, m, d, (sod / 3600) as u32, (sod % 3600 / 60) as u32)
}

/// Seconds local time is ahead of UTC now.
pub fn local_utc_offset() -> i64 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    utc_offset(secs)
}

/// The local time as `2024-05-01T10:30:00`.
pub fn local_iso() -> String {
    let (y, m, d, h, mi) = local_now();
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:00")
}

#[cfg(unix)]
fn utc_offset(secs: i64) -> i64 {
    let t = secs as libc::time_t;
    // SAFETY: localtime_r only writes into the provided struct.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() { 0 } else { tm.tm_gmtoff as i64 }
    }
}

#[cfg(windows)]
fn utc_offset(secs: i64) -> i64 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FileTimeToLocalFileTime(utc: *const [u32; 2], local: *mut [u32; 2]) -> i32;
    }
    // FILETIME: 100 ns ticks since 1601.
    let ticks = (secs.clamp(-11_644_473_600, LAST_SECOND) + 11_644_473_600) as u64 * 10_000_000;
    let utc = [ticks as u32, (ticks >> 32) as u32];
    let mut local = [0u32; 2];
    // SAFETY: both pointers are valid FILETIMEs.
    if unsafe { FileTimeToLocalFileTime(&utc, &mut local) } == 0 {
        return 0;
    }
    let local = (local[0] as u64 | (local[1] as u64) << 32) as i64;
    (local - ticks as i64) / 10_000_000
}

#[cfg(not(any(unix, windows)))]
fn utc_offset(_secs: i64) -> i64 {
    0
}

#[cfg(test)]
pub mod test {
    /// Programs asked to open something since the count was last taken.
    pub static OPENED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
}

/// Opens a URL, e-mail or file with the system's default handler, without
/// waiting for it.
pub fn open_external(target: &str) -> bool {
    use std::process::{Command, Stdio};
    let target = target.trim();
    // Nor what `open` or `xdg-open` would take for an option.
    if target.is_empty() || target.starts_with('-') {
        return false;
    }
    // Tests start no program; they count what was asked for.
    if cfg!(test) {
        #[cfg(test)]
        test::OPENED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return true;
    }
    let mut cmd = if cfg!(target_os = "windows") {
        // Not `cmd /C start`: the shell would interpret the URL.
        let mut c = Command::new(apex_io::safe_path::windows_program(r"System32\rundll32.exe"));
        c.arg("url.dll,FileProtocolHandler");
        c
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    cmd.arg(target).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    match cmd.spawn() {
        Ok(mut child) => {
            // Reap the process so it doesn't linger as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

/// The user's name for notes and comments: the full name from the system
/// account, or the login name.
pub fn user_name() -> String {
    let login = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    #[cfg(unix)]
    if let Ok(passwd) = std::fs::read_to_string("/etc/passwd") {
        for line in passwd.lines() {
            let f: Vec<&str> = line.split(':').collect();
            if f.len() > 4 && f[0] == login {
                let full = f[4].split(',').next().unwrap_or("").trim();
                if !full.is_empty() {
                    return full.to_string();
                }
            }
        }
    }
    login
}

#[cfg(test)]
mod picker_tests {
    use super::*;

    /// Clicks the box (press in one frame, release in the next) and says
    /// whether its list is still open two frames later.
    fn list_open_after_click(draw: impl Fn(&mut egui::Ui) -> egui::Rect, popup_id: Id) -> bool {
        clicked(draw, |ctx| egui::Popup::is_id_open(ctx, popup_id))
    }

    /// Clicks the box `draw` returns and says what `open` sees two frames
    /// later.
    fn clicked(draw: impl Fn(&mut egui::Ui) -> egui::Rect, open: impl Fn(&egui::Context) -> bool) -> bool {
        let ctx = egui::Context::default();
        // The lists show names in their own faces once another test has
        // scanned the system's fonts: this context needs them too.
        crate::fonts::install(&ctx);
        let rect = std::cell::Cell::new(egui::Rect::NOTHING);
        let frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0))), ..Default::default() };
            ctx.run_ui(input, |ui| rect.set(draw(ui))).textures_delta.clear();
        };
        frame(vec![]);
        frame(vec![]);
        let p = rect.get().center();
        let button = |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(vec![egui::Event::PointerMoved(p), button(true)]);
        frame(vec![button(false)]);
        frame(vec![]);
        frame(vec![]);
        open(&ctx)
    }

    #[test]
    fn in_a_folded_group_the_lists_open_and_the_group_stays() {
        for size in [false, true] {
            let id = Id::new(("folded", size));
            let group = std::cell::Cell::new(Id::NULL);
            let open = clicked(
                |ui| {
                    let g = ui.button("Font");
                    let menu = egui::Popup::default_response_id(&g);
                    group.set(menu);
                    if !egui::Popup::is_id_open(ui.ctx(), menu) && ui.ctx().cumulative_pass_nr() == 0 {
                        egui::Popup::open_id(ui.ctx(), menu);
                    }
                    let mut r = egui::Rect::NOTHING;
                    egui::Popup::menu(&g).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                        if size {
                            size_box(ui, id, 11.0);
                        } else {
                            font_picker(ui, id, "Calibri", 120.0);
                        }
                        r = ui.min_rect();
                    });
                    r
                },
                |ctx| egui::Popup::is_id_open(ctx, group.get()) && ctx.data(|d| d.get_temp::<bool>(id.with("popup").with("open"))) == Some(true),
            );
            assert!(open, "size box: {size}");
        }
    }

    #[test]
    fn a_click_on_the_font_and_size_boxes_leaves_their_lists_open() {
        let font = Id::new("font");
        assert!(list_open_after_click(
            |ui| {
                font_picker(ui, font, "Calibri", 120.0);
                ui.min_rect()
            },
            font.with("popup")
        ));
        let size = Id::new("size");
        assert!(list_open_after_click(
            |ui| {
                size_box(ui, size, 11.0);
                ui.min_rect()
            },
            size.with("popup")
        ));
    }
}

/// Writes a file the user chose (an export, a copy): through a temporary
/// and a rename, never through a link someone else put in its place.
pub(crate) fn write_out(path: impl AsRef<std::path::Path>, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    apex_io::write_atomic(path.as_ref(), bytes.as_ref()).map_err(|e| std::io::Error::other(e.to_string()))
}
