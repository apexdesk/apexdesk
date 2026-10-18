//! Shared state and helpers of the table, analysis, pivot table and window
//! features: their dialogs (non-modal, so cells can be picked on the grid
//! while one is open), the reference boxes that follow the selection, the
//! custom lists from the options, and the menu entries they add.

use std::sync::Mutex;

use super::analysis_ui::{AnalysisDlg, ToolDlg};
use super::datatools_ui::{AdvDlg, ConsDlg};
use super::names_ui::NamesDlg;
use super::pivot_ui::{PivotDlg, PivotPanel};
use super::tables_ui::TableDlg;
use super::window_ui::Split;
use super::*;
use crate::widgets::DialogResult;
use crate::widgets::FitWindow as _;

/// A dialog of these features (one at a time per view).
pub(super) enum F10Dlg {
    Table(TableDlg),
    Analysis(AnalysisDlg),
    Tool(Box<ToolDlg>),
    Pivot(PivotDlg),
    Names(NamesDlg),
    AdvFilter(AdvDlg),
    Consolidate(ConsDlg),
}

#[derive(Default)]
pub(super) struct F10State {
    pub dlg: Option<F10Dlg>,
    pub pick: RefPick,
    /// Function list of a totals row cell: table, column, where.
    pub total_menu: Option<(apex_sheet::workbook::TableId, usize, Pos2)>,
    pub pivot: PivotPanel,
    pub split: Split,
    /// Active pane of a split window (0 without a split).
    pub pane: u8,
    /// Custom lists generation copied into the workbook.
    pub lists_gen: u64,
}

/// A reference box of a dialog: the last one focused takes the cells the
/// user selects on the grid (like Excel's RefEdit).
#[derive(Default)]
pub(super) struct RefPick {
    /// Key of the box that takes the selection.
    pub field: Option<u32>,
    /// Sheet and selection seen last frame.
    pub seen: Option<(usize, Range)>,
    /// New selection text for the box.
    pub pending: Option<String>,
    /// Sheet the dialog was opened on (other sheets get a prefix).
    pub origin: usize,
    /// Focus the box (text selected) on the next frame.
    pub focus: bool,
}

impl RefPick {
    pub fn new(origin: usize, field: Option<u32>) -> RefPick {
        RefPick { field, seen: None, pending: None, origin, focus: field.is_some() }
    }
}

/// A reference text box tied to `pick` under `key`.
pub(super) fn ref_edit(ui: &mut egui::Ui, text: &mut String, key: u32, pick: &mut RefPick, width: f32) -> egui::Response {
    if pick.field == Some(key) {
        if let Some(t) = pick.pending.take() {
            *text = t;
        }
    }
    let mut out = egui::TextEdit::singleline(text).desired_width(width).font(egui::TextStyle::Monospace).show(ui);
    let r = out.response.clone();
    if pick.focus && pick.field == Some(key) {
        // Like Excel, a dialog opens with its reference selected.
        pick.focus = false;
        r.request_focus();
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()));
        out.state.cursor.set_char_range(Some(all));
        out.state.store(ui.ctx(), r.id);
    }
    if r.has_focus() || r.gained_focus() {
        pick.field = Some(key);
    }
    let active = pick.field == Some(key);
    if active {
        ui.painter().rect_stroke(r.rect.expand(1.0), 2.0, Stroke::new(1.0, theme::SHEET_ACCENT), StrokeKind::Outside);
    }
    r.response
}

// --- custom lists (File > Options) ----------------------------------------------------

static CUSTOM_LISTS: Mutex<(u64, Vec<Vec<String>>)> = Mutex::new((0, Vec::new()));

/// The user's custom lists (settings), for fill series and sorting.
pub fn custom_lists() -> Vec<Vec<String>> {
    CUSTOM_LISTS.lock().map(|g| g.1.clone()).unwrap_or_default()
}

pub fn set_custom_lists(v: Vec<Vec<String>>) {
    if let Ok(mut g) = CUSTOM_LISTS.lock() {
        g.0 += 1;
        g.1 = v;
    }
}

fn lists_generation() -> u64 {
    CUSTOM_LISTS.lock().map(|g| g.0).unwrap_or(0)
}

// --- references -------------------------------------------------------------------------

/// `$A$1` or `$A$1:$C$9`.
pub(super) fn abs_range(r: &Range) -> String {
    let abs = |c: CellRef| format!("${}${}", apex_sheet::addr::col_name(c.col), c.row + 1);
    if r.is_single() { abs(r.start) } else { format!("{}:{}", abs(r.start), abs(r.end)) }
}

/// A sheet name as written before `!`.
pub(super) fn quoted_sheet(name: &str) -> String {
    if name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit()) {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

/// Parses a reference typed in a dialog: `A1`, `$A$1:$B$4`, `A:C`, `2:5`,
/// `Sheet2!A1:B4`, `'My sheet'!A1` or a table name (its whole range).
pub(super) fn parse_ref(wb: &Workbook, default_sheet: usize, text: &str) -> Option<(usize, Range)> {
    let t = text.trim().trim_start_matches('=').trim();
    if t.is_empty() {
        return None;
    }
    if let Some(id) = wb.find_table(t) {
        return Some((id.0, wb.table(id)?.range));
    }
    let (si, rest) = match t.rsplit_once('!') {
        Some((s, r)) => {
            let s = s.trim();
            let s = s.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')).map_or_else(|| s.to_string(), |x| x.replace("''", "'"));
            (wb.sheet_index(&s)?, r.trim())
        }
        None => (default_sheet, t),
    };
    let up = rest.to_ascii_uppercase().replace('$', "");
    if let Some(r) = Range::parse(&up) {
        return Some((si, r));
    }
    // Whole columns or rows.
    let (a, b) = up.split_once(':').unwrap_or((&up, &up));
    if let (Some(x), Some(y)) = (apex_sheet::addr::parse_col(a), apex_sheet::addr::parse_col(b)) {
        return Some((si, Range::new(CellRef::new(0, x.min(y)), CellRef::new(LAST.row, x.max(y)))));
    }
    if let (Ok(x), Ok(y)) = (a.parse::<u32>(), b.parse::<u32>()) {
        if x >= 1 && y >= 1 && x <= MAX_ROWS && y <= MAX_ROWS {
            return Some((si, Range::new(CellRef::new(x.min(y) - 1, 0), CellRef::new(x.max(y) - 1, LAST.col))));
        }
    }
    None
}

/// A number typed in a dialog, in the workbook's locale or with a dot.
pub(super) fn parse_num(wb: &Workbook, text: &str) -> Option<f64> {
    let t = text.trim();
    if let Value::Number(n) = apex_sheet::format::parse_input(t, &wb.locale).value {
        return Some(n);
    }
    t.replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A number for a dialog box, in the workbook's locale.
pub(super) fn fmt_num(wb: &Workbook, v: f64) -> String {
    let s = format!("{}", (v * 1e12).round() / 1e12);
    if wb.locale.decimal == ',' { s.replace('.', ",") } else { s }
}

// --- windows ----------------------------------------------------------------------------

/// A dialog that leaves the grid usable (to pick cells): a window with OK
/// and Cancel. Escape cancels; Enter confirms (also while picking cells on
/// the grid) unless a cell is being edited or a multiline box has focus.
pub(super) fn tool_window(ctx: &egui::Context, id: &str, title: &str, width: f32, grid_focus: bool, content: impl FnOnce(&mut egui::Ui)) -> DialogResult {
    let mut res = DialogResult::None;
    let screen = ctx.content_rect();
    egui::Window::new(title)
        .id(Id::new(("f10_window", id)))
        .collapsible(false)
        .resizable(false)
        .default_pos(pos2(screen.center().x - width / 2.0, screen.min.y + 150.0))
        .show_fit(ctx, |ui| {
            ui.set_width(width.min(crate::widgets::dialog_room(ctx).x));
            content(ui);
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 24.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("unsaved.cancel")).clicked() {
                    res = DialogResult::Cancel;
                }
                if ui.button(RichText::new(tr("dialog.ok")).strong()).clicked() {
                    res = DialogResult::Ok;
                }
            });
        });
    if res == DialogResult::None {
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            res = DialogResult::Cancel;
        } else if !grid_focus && ctx.input(|i| i.key_pressed(Key::Enter)) && !ctx.memory(|m| m.focused().is_some_and(|f| ctx.data(|d| d.get_temp::<bool>(f.with("multiline")).unwrap_or(false)))) {
            res = DialogResult::Ok;
        }
    }
    res
}

/// A window without the OK/Cancel row (managers with their own buttons).
/// Returns false when closed with its title bar button or Escape.
pub(super) fn plain_window(ctx: &egui::Context, id: &str, title: &str, width: f32, content: impl FnOnce(&mut egui::Ui)) -> bool {
    let mut open = true;
    let screen = ctx.content_rect();
    egui::Window::new(title)
        .id(Id::new(("f10_window", id)))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .default_pos(pos2(screen.center().x - width / 2.0, screen.min.y + 130.0))
        .show_fit(ctx, |ui| {
            ui.set_width(width.min(crate::widgets::dialog_room(ctx).x));
            content(ui);
        });
    open && !ctx.input(|i| i.key_pressed(Key::Escape))
}

impl SheetView {
    /// Whether a cell is being edited (Enter then belongs to the editor).
    pub(super) fn grid_has_focus(&self, ctx: &egui::Context) -> bool {
        self.edit.is_some() || ctx.memory(|m| m.focused().is_some_and(|f| f == self.bar_id() || f == self.cell_edit_id()))
    }

    /// Per-frame work: custom lists from the options and the selection for
    /// the reference box of an open dialog.
    pub(super) fn f10_frame(&mut self) {
        let generation = lists_generation();
        if generation != self.f10.lists_gen {
            self.f10.lists_gen = generation;
            self.wb.analysis.custom_lists = custom_lists();
        }
        if self.f10.dlg.is_none() {
            self.f10.pick.seen = None;
            return;
        }
        let key = (self.sheet, self.sel());
        if self.f10.pick.seen != Some(key) {
            if self.f10.pick.seen.is_some() && self.f10.pick.field.is_some() {
                let mut t = abs_range(&key.1);
                if self.sheet != self.f10.pick.origin {
                    t = format!("{}!{}", quoted_sheet(&self.sh().name), t);
                }
                self.f10.pick.pending = Some(t);
            }
            self.f10.pick.seen = Some(key);
        }
    }

    pub(super) fn open_f10(&mut self, d: F10Dlg, field: Option<u32>) {
        self.settle_edit();
        self.f10.pick = RefPick::new(self.sheet, field);
        self.f10.dlg = Some(d);
    }

    /// Dialogs, popups and panels of these features.
    pub(super) fn f10_ui(&mut self, ctx: &egui::Context) {
        self.totals_popup(ctx);
        let Some(d) = self.f10.dlg.take() else { return };
        let keep = match d {
            F10Dlg::Table(t) => self.table_dialog(ctx, t).map(F10Dlg::Table),
            F10Dlg::Analysis(a) => self.analysis_dialog(ctx, a).map(F10Dlg::Analysis),
            F10Dlg::Tool(t) => self.tool_dialog(ctx, *t).map(|t| F10Dlg::Tool(Box::new(t))),
            F10Dlg::Pivot(p) => self.pivot_dialog(ctx, p).map(F10Dlg::Pivot),
            F10Dlg::Names(n) => self.names_dialog(ctx, n).map(F10Dlg::Names),
            F10Dlg::AdvFilter(d) => self.adv_filter_dialog(ctx, d).map(F10Dlg::AdvFilter),
            F10Dlg::Consolidate(d) => self.consolidate_dialog(ctx, d).map(F10Dlg::Consolidate),
        };
        // A dialog may have opened another one.
        if self.f10.dlg.is_none() {
            self.f10.dlg = keep;
        }
        if self.f10.dlg.is_none() {
            self.focus_grid = true;
        }
    }

    /// Keyboard shortcuts of these features; true if the key was used.
    pub(super) fn f10_key(&mut self, key: Key, m: Modifiers) -> bool {
        if self.f10.dlg.is_some() && key == Key::Enter {
            // It confirms the open dialog.
            return true;
        }
        if !m.command || m.alt {
            return false;
        }
        match key {
            Key::T | Key::L if !m.shift => self.open_create_table(None),
            Key::E if !m.shift => self.flash_fill_cmd(),
            _ => return false,
        }
        true
    }

    // --- menus ------------------------------------------------------------------------------

    /// Entries of the cell context menu.
    pub(super) fn f10_cell_menu(&mut self, ui: &mut egui::Ui) {
        if self.cursor_table().is_some() {
            crate::widgets::menu_button(ui, tr("xtab.table"), |ui| self.table_menu(ui));
        }
        if self.wb.pivot_at(self.sheet, self.cursor).is_some() {
            crate::widgets::menu_button(ui, tr("pivot.pivot"), |ui| self.pivot_menu(ui));
        }
    }

}
