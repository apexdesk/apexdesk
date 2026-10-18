//! Data tools on the grid: autofilter, data validation (dropdowns, input
//! messages, error alerts, invalid-data circles), remove duplicates, text to
//! columns and typing autocomplete.

use std::cell::RefCell;

use apex_sheet::data::{AutoFilter, CmpOp, ColumnFilter, ErrorStyle, FilterKind, ValKind, Validation, ranges_minus};
use apex_sheet::functions::CritOp;
use apex_sheet::{NumFormat, SplitMode, split_text};

use super::cf_ui::{CfDlg, CfState};
use super::*;

/// Data tools state of one sheet view.
#[derive(Default)]
pub(super) struct DataState {
    pub popup: Option<FilterPopup>,
    pub dialog: Option<DataDlg>,
    /// Circle invalid data is on.
    pub circles: bool,
    pub list: Option<ListPopup>,
    /// Screen rect of the cursor cell in the last frame.
    pub cursor_rect: Option<Rect>,
    pub cf: RefCell<CfState>,
    pub invalid: RefCell<Option<((usize, u64), Vec<CellRef>)>>,
    pub col_texts: RefCell<Option<((usize, CellRef, u64), Vec<String>)>>,
    /// The list of the last in-place advanced filter (Clear shows it again).
    pub advanced: Option<(usize, Range)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ColKind {
    Text,
    Number,
    Date,
}

/// The dropdown of a filter button.
pub(super) struct FilterPopup {
    col: u32,
    at: Pos2,
    search: String,
    items: Vec<(String, bool)>,
    blanks: Option<bool>,
    changed: bool,
    kind: ColKind,
    fills: Vec<Option<u32>>,
    fonts: Vec<Option<u32>>,
    fresh: bool,
}

/// Choices under the cursor cell (validation list or pick from list).
pub(super) struct ListPopup {
    cell: CellRef,
    items: Vec<String>,
    index: usize,
    fresh: bool,
}

/// Comparison offered by the custom filter dialog.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum UiOp {
    None,
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Begins,
    NotBegins,
    Ends,
    NotEnds,
    Contains,
    NotContains,
}

impl UiOp {
    const ALL: [UiOp; 12] = [UiOp::Eq, UiOp::Ne, UiOp::Gt, UiOp::Ge, UiOp::Lt, UiOp::Le, UiOp::Begins, UiOp::NotBegins, UiOp::Ends, UiOp::NotEnds, UiOp::Contains, UiOp::NotContains];

    fn key(self) -> &'static str {
        match self {
            UiOp::None => "filter.op_none",
            UiOp::Eq => "filter.op_eq",
            UiOp::Ne => "filter.op_ne",
            UiOp::Gt => "filter.op_gt",
            UiOp::Ge => "filter.op_ge",
            UiOp::Lt => "filter.op_lt",
            UiOp::Le => "filter.op_le",
            UiOp::Begins => "filter.op_begins",
            UiOp::NotBegins => "filter.op_not_begins",
            UiOp::Ends => "filter.op_ends",
            UiOp::NotEnds => "filter.op_not_ends",
            UiOp::Contains => "filter.op_contains",
            UiOp::NotContains => "filter.op_not_contains",
        }
    }

    /// Date columns speak of before and after.
    fn label(self, kind: ColKind) -> &'static str {
        match (self, kind) {
            (UiOp::Gt, ColKind::Date) => tr("filter.op_after"),
            (UiOp::Lt, ColKind::Date) => tr("filter.op_before"),
            (UiOp::Ge, ColKind::Date) => tr("filter.op_after_eq"),
            (UiOp::Le, ColKind::Date) => tr("filter.op_before_eq"),
            _ => tr(self.key()),
        }
    }

    fn to_crit(self, v: &str) -> Option<(CritOp, String)> {
        Some(match self {
            UiOp::None => return None,
            UiOp::Eq => (CritOp::Eq, v.to_string()),
            UiOp::Ne => (CritOp::Ne, v.to_string()),
            UiOp::Gt => (CritOp::Gt, v.to_string()),
            UiOp::Ge => (CritOp::Ge, v.to_string()),
            UiOp::Lt => (CritOp::Lt, v.to_string()),
            UiOp::Le => (CritOp::Le, v.to_string()),
            UiOp::Begins => (CritOp::Eq, format!("{v}*")),
            UiOp::NotBegins => (CritOp::Ne, format!("{v}*")),
            UiOp::Ends => (CritOp::Eq, format!("*{v}")),
            UiOp::NotEnds => (CritOp::Ne, format!("*{v}")),
            UiOp::Contains => (CritOp::Eq, format!("*{v}*")),
            UiOp::NotContains => (CritOp::Ne, format!("*{v}*")),
        })
    }

    fn from_crit(op: CritOp, s: &str) -> (UiOp, String) {
        let eq = op == CritOp::Eq;
        let (a, b) = (s.starts_with('*'), s.ends_with('*') && s.len() > 1);
        match op {
            CritOp::Eq | CritOp::Ne if a && b => (if eq { UiOp::Contains } else { UiOp::NotContains }, s[1..s.len() - 1].to_string()),
            CritOp::Eq | CritOp::Ne if b => (if eq { UiOp::Begins } else { UiOp::NotBegins }, s[..s.len() - 1].to_string()),
            CritOp::Eq | CritOp::Ne if a => (if eq { UiOp::Ends } else { UiOp::NotEnds }, s[1..].to_string()),
            CritOp::Eq => (UiOp::Eq, s.to_string()),
            CritOp::Ne => (UiOp::Ne, s.to_string()),
            CritOp::Gt => (UiOp::Gt, s.to_string()),
            CritOp::Ge => (UiOp::Ge, s.to_string()),
            CritOp::Lt => (UiOp::Lt, s.to_string()),
            CritOp::Le => (UiOp::Le, s.to_string()),
        }
    }
}

pub(super) struct ValDlg {
    tab: usize,
    v: Validation,
    f1: String,
    f2: String,
}

pub(super) struct TextColsDlg {
    range: Range,
    fixed: bool,
    /// Tab, semicolon, comma, space.
    delims: [bool; 4],
    other: bool,
    other_ch: String,
    merge: bool,
    /// 0 `"`, 1 `'`, 2 none.
    quote: usize,
    breaks: String,
    dest: String,
}

impl TextColsDlg {
    fn mode(&self) -> SplitMode {
        if self.fixed {
            let breaks = self.breaks.split([',', ';', ' ']).filter_map(|s| s.trim().parse().ok()).collect();
            return SplitMode::Fixed(breaks);
        }
        let mut delims = Vec::new();
        for (on, ch) in self.delims.iter().zip(['\t', ';', ',', ' ']) {
            if *on {
                delims.push(ch);
            }
        }
        if self.other {
            delims.extend(self.other_ch.chars().take(1));
        }
        SplitMode::Delimited { delims, merge: self.merge, quote: [Some('"'), Some('\''), None][self.quote.min(2)] }
    }
}

pub(super) enum DataDlg {
    Custom { col: u32, and: bool, a: (UiOp, String), b: (UiOp, String), kind: ColKind },
    Top { col: u32, top: bool, n: f64, percent: bool },
    Validation(Box<ValDlg>),
    ValError { sheet: usize, cell: CellRef, text: String, style: ErrorStyle, title: String, msg: String },
    RemoveDups { range: Range, header: bool, cols: Vec<bool> },
    TextCols(Box<TextColsDlg>),
    Cf(CfDlg),
    Group { ungroup: bool, rows: bool },
    Subtotals(Box<super::outline_ui::SubDlg>),
    BankRec(Box<super::bank_ui::BankDlg>),
}

const VAL_KINDS: [(ValKind, &str); 8] = [
    (ValKind::Any, "val.any"),
    (ValKind::Whole, "val.whole"),
    (ValKind::Decimal, "val.decimal"),
    (ValKind::List, "val.list"),
    (ValKind::Date, "val.date"),
    (ValKind::Time, "val.time"),
    (ValKind::TextLength, "val.text_length"),
    (ValKind::Custom, "val.custom"),
];

pub(super) fn cmp_key(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Between => "cmp.between",
        CmpOp::NotBetween => "cmp.not_between",
        CmpOp::Equal => "cmp.equal",
        CmpOp::NotEqual => "cmp.not_equal",
        CmpOp::Greater => "cmp.greater",
        CmpOp::Less => "cmp.less",
        CmpOp::GreaterEq => "cmp.greater_eq",
        CmpOp::LessEq => "cmp.less_eq",
    }
}

/// Date number format for the locale (to show dates typed in rules).
fn date_code(loc: &apex_sheet::Locale) -> String {
    let s = loc.date_sep;
    match loc.date_order {
        apex_sheet::format::DateOrder::Dmy => format!("dd{s}mm{s}yyyy"),
        apex_sheet::format::DateOrder::Mdy => format!("mm{s}dd{s}yyyy"),
        apex_sheet::format::DateOrder::Ymd => format!("yyyy{s}mm{s}dd"),
    }
}

/// A small square button with a down arrow (filters and dropdowns).
pub(super) fn paint_drop_button(p: &egui::Painter, r: Rect, filtered: bool, sorted: Option<bool>) {
    p.rect_filled(r, 2.0, Color32::from_gray(246));
    p.rect_stroke(r, 2.0, Stroke::new(1.0, Color32::from_gray(160)), StrokeKind::Inside);
    let c = Color32::from_gray(70);
    if filtered {
        let icon = Rect::from_min_size(r.min + vec2(1.0, 1.0), vec2(r.width() * 0.62, r.height() - 2.0));
        crate::icons::paint(p, icon, Icon::Filter, theme::ACCENT);
        let a = r.right_center() + vec2(-r.width() * 0.2, 1.0);
        let d = r.width() * 0.13;
        p.add(egui::Shape::convex_polygon(vec![a + vec2(-d, -d * 0.6), a + vec2(d, -d * 0.6), a + vec2(0.0, d * 0.8)], c, Stroke::NONE));
        return;
    }
    if let Some(asc) = sorted {
        let x = r.center().x - r.width() * 0.18;
        let (y0, y1) = (r.min.y + r.height() * 0.22, r.max.y - r.height() * 0.22);
        p.line_segment([pos2(x, y0), pos2(x, y1)], Stroke::new(1.2, c));
        let (tip, dir) = if asc { (pos2(x, y0), 1.0) } else { (pos2(x, y1), -1.0) };
        let d = r.width() * 0.14;
        p.line_segment([tip, tip + vec2(-d, d * dir)], Stroke::new(1.2, c));
        p.line_segment([tip, tip + vec2(d, d * dir)], Stroke::new(1.2, c));
        let a = pos2(r.center().x + r.width() * 0.2, r.center().y);
        let d = r.width() * 0.12;
        p.add(egui::Shape::convex_polygon(vec![a + vec2(-d, -d * 0.6), a + vec2(d, -d * 0.6), a + vec2(0.0, d * 0.8)], c, Stroke::NONE));
        return;
    }
    let a = r.center();
    let d = r.width() * 0.2;
    p.add(egui::Shape::convex_polygon(vec![a + vec2(-d, -d * 0.5), a + vec2(d, -d * 0.5), a + vec2(0.0, d * 0.7)], c, Stroke::NONE));
}

impl SheetView {
    fn list_sep(&self) -> char {
        if self.wb.locale.decimal == ',' { ';' } else { self.wb.locale.arg_sep }
    }

    // --- autofilter ------------------------------------------------------------

    /// Turns the filter buttons on (over the data around the cursor) or off.
    pub(super) fn toggle_filter(&mut self) {
        self.settle_edit();
        self.data.popup = None;
        if self.toggle_table_filter() {
            return;
        }
        if self.sh().filter.is_some() {
            self.wb.set_filter(self.sheet, None, "undo.filter");
            return;
        }
        let sel = self.sel();
        let r = if sel.is_single() {
            self.wb.current_region(self.sheet, self.cursor)
        } else {
            match self.wb.clamp_to_used(self.sheet, &sel) {
                Some(u) => Range::new(sel.start, u.end),
                None => sel,
            }
        };
        if r.is_single() && self.sh().value(r.start).is_empty() {
            self.notices.push(Notice::info(tr("filter.no_data")));
            return;
        }
        self.wb.set_filter(self.sheet, Some(AutoFilter::new(r)), "undo.filter");
    }

    /// The sheet's filter with its range grown over data added below.
    fn filter_now(&self) -> Option<AutoFilter> {
        let mut f = self.sh().filter.clone()?;
        if self.is_table_filter(&f) {
            // Tables grow their filter themselves.
            return Some(f);
        }
        let region = self.wb.current_region(self.sheet, f.range.start);
        if region.start.col <= f.range.start.col && region.end.col >= f.range.end.col {
            f.range.end.row = f.range.end.row.max(region.end.row);
        }
        Some(f)
    }

    pub(super) fn set_col_filter(&mut self, col: u32, kind: Option<FilterKind>) {
        self.settle_edit();
        let Some(mut f) = self.filter_now() else { return };
        f.cols.retain(|c| c.col != col);
        if let Some(kind) = kind {
            f.cols.push(ColumnFilter { col, kind });
            f.cols.sort_by_key(|c| c.col);
        }
        self.wb.set_filter(self.sheet, Some(f), "undo.filter");
    }

    pub(super) fn clear_filters(&mut self) {
        self.settle_edit();
        self.clear_advanced_filter();
        if let Some(mut f) = self.filter_now() {
            f.cols.clear();
            f.sort = None;
            self.wb.set_filter(self.sheet, Some(f), "undo.filter");
        }
    }

    pub(super) fn reapply_filter(&mut self) {
        self.settle_edit();
        if let Some(f) = self.filter_now() {
            self.wb.set_filter(self.sheet, Some(f), "undo.filter");
        }
    }

    /// Filters the column of the cursor by the cursor cell's value.
    pub(super) fn filter_by_cell(&mut self) {
        let c = self.cursor;
        if self.sh().filter.is_none() {
            self.toggle_filter();
        }
        let Some(f) = self.sh().filter.clone() else { return };
        if !f.range.contains(c) || c.row == f.range.start.row {
            return;
        }
        let v = self.sh().value(c).clone();
        let kind = if v.is_empty() { FilterKind::Values { items: Vec::new(), blanks: true } } else { FilterKind::Values { items: vec![self.wb.display(self.sheet, c).0], blanks: false } };
        self.set_col_filter(c.col, Some(kind));
    }

    /// Filter buttons on screen: column and rect.
    fn filter_buttons(&self, g: &Geo) -> Vec<(u32, Rect)> {
        let sh = self.sh();
        let Some(f) = &sh.filter else { return Vec::new() };
        let hr = f.range.start.row;
        if sh.row_height(hr) == 0.0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for col in (g.top.col..g.fc).chain(g.scroll_cols(sh)) {
            if col < f.range.start.col || col > f.range.end.col {
                continue;
            }
            let r = g.cell_rect(sh, CellRef::new(hr, col));
            if r.width() < 4.0 || !r.intersects(g.view) {
                continue;
            }
            let s = (r.height() - 3.0).min(17.0 * g.z).max(8.0f32.min(17.0 * g.z)).min(r.width() - 2.0);
            out.push((col, Rect::from_min_max(pos2(r.max.x - s - 2.0, r.max.y - s - 2.0), pos2(r.max.x - 2.0, r.max.y - 2.0))));
        }
        out
    }

    /// Dropdown button of a list validation next to the cursor cell.
    fn list_button(&self) -> Option<Rect> {
        let r = self.data.cursor_rect?;
        let v = self.wb.validation_at(self.sheet, self.cursor)?;
        if v.kind != ValKind::List || !v.dropdown || !self.sel().is_single() {
            return None;
        }
        let s = r.height().clamp(12.0, 20.0);
        Some(Rect::from_min_size(pos2(r.max.x + 1.0, r.max.y - s), vec2(s, s)))
    }

    /// Filter buttons, the list dropdown button and invalid-data circles.
    pub(super) fn paint_data_overlays(&self, painter: &egui::Painter, g: &Geo) {
        let p = painter.with_clip_rect(g.view);
        if let Some(f) = &self.sh().filter {
            for (col, r) in self.filter_buttons(g) {
                let sorted = f.sort.filter(|s| s.0 == col).map(|s| s.1);
                paint_drop_button(&p, r, f.column(col).is_some(), sorted);
            }
        }
        if self.data.circles {
            let sh = self.sh();
            for c in self.invalid_now() {
                let r = g.cell_rect(sh, c);
                if r.width() > 1.0 && r.intersects(g.view) {
                    p.add(egui::Shape::ellipse_stroke(r.center(), r.size() * 0.5 + vec2(5.0, 3.0), Stroke::new(1.6, Color32::from_rgb(0xE0, 0x10, 0x10))));
                }
            }
        }
        if self.edit.is_none() {
            if let Some(r) = self.list_button() {
                paint_drop_button(painter, r, false, None);
            }
        }
    }

    fn invalid_now(&self) -> Vec<CellRef> {
        let key = (self.sheet, self.wb.revision);
        let mut cache = self.data.invalid.borrow_mut();
        if cache.as_ref().is_none_or(|(k, _)| *k != key) {
            *cache = Some((key, self.wb.invalid_cells(self.sheet)));
        }
        cache.as_ref().unwrap().1.clone()
    }

    /// Handles a click on a filter or dropdown button; true if it took it.
    pub(super) fn data_click(&mut self, g: &Geo, p: Pos2) -> bool {
        if self.table_click(g, p) {
            return true;
        }
        if let Some((col, r)) = self.filter_buttons(g).into_iter().find(|(_, r)| r.expand(1.0).contains(p)) {
            if self.data.popup.as_ref().is_some_and(|x| x.col == col) {
                self.data.popup = None;
            } else {
                self.open_filter_popup(col, r.left_bottom() + vec2(0.0, 2.0));
            }
            return true;
        }
        if self.list_button().is_some_and(|r| r.contains(p)) {
            if self.data.list.is_some() {
                self.data.list = None;
            } else {
                self.open_list();
            }
            return true;
        }
        false
    }

    fn col_kind(&self, f: &AutoFilter, col: u32) -> ColKind {
        let sh = self.sh();
        let Some(d) = f.data_rows() else { return ColKind::Text };
        let (mut nums, mut texts, mut dates) = (0, 0, 0);
        for row in d.start.row..=d.end.row.min(d.start.row + 2000) {
            let c = CellRef::new(row, col);
            match sh.value(c) {
                Value::Number(_) => {
                    nums += 1;
                    let st = sh.get(c).map_or(0, |x| x.style);
                    if NumFormat::parse(&self.wb.styles.get(st).num_fmt).is_date() {
                        dates += 1;
                    }
                }
                Value::Text(_) => texts += 1,
                _ => {}
            }
        }
        if nums > texts {
            if dates * 2 > nums { ColKind::Date } else { ColKind::Number }
        } else {
            ColKind::Text
        }
    }

    pub(super) fn open_filter_popup(&mut self, col: u32, at: Pos2) {
        self.settle_edit();
        let Some(f) = self.filter_now() else { return };
        let (vals, blanks) = self.wb.filter_values(self.sheet, &f, col);
        let (checked, blank_on): (Box<dyn Fn(&str) -> bool>, bool) = match f.column(col).map(|c| &c.kind) {
            Some(FilterKind::Values { items, blanks }) => {
                let set: std::collections::HashSet<String> = items.iter().map(|s| s.to_lowercase()).collect();
                (Box::new(move |s: &str| set.contains(&s.to_lowercase())), *blanks)
            }
            _ => (Box::new(|_: &str| true), true),
        };
        let items = vals.into_iter().map(|v| (v.clone(), checked(&v))).collect();
        let kind = self.col_kind(&f, col);
        let (mut fills, mut fonts) = (Vec::new(), Vec::new());
        if let Some(d) = f.data_rows() {
            let sh = self.sh();
            for row in d.start.row..=d.end.row.min(d.start.row + 5000) {
                let st = sh.get(CellRef::new(row, col)).map(|x| self.wb.styles.get(x.style));
                let (fill, font) = (st.and_then(|s| s.fill), st.and_then(|s| s.color));
                if !fills.contains(&fill) && fills.len() < 24 {
                    fills.push(fill);
                }
                if !fonts.contains(&font) && fonts.len() < 24 {
                    fonts.push(font);
                }
            }
        }
        self.data.list = None;
        self.data.popup = Some(FilterPopup { col, at, search: String::new(), items, blanks: blanks.then_some(blank_on), changed: false, kind, fills, fonts, fresh: true });
    }

    /// The filter popup, the list dropdown and the input message.
    pub(super) fn data_popups(&mut self, ui: &mut egui::Ui) {
        self.filter_popup(ui.ctx());
        self.list_popup(ui.ctx());
        self.input_message(ui.ctx());
    }

    fn filter_popup(&mut self, ctx: &egui::Context) {
        let Some(mut pop) = self.data.popup.take() else { return };
        let header = self.sh().filter.as_ref().map(|f| self.wb.display(self.sheet, CellRef::new(f.range.start.row, pop.col)).0).unwrap_or_default();
        let current = self.sh().filter.as_ref().and_then(|f| f.column(pop.col).cloned());
        enum Act {
            Sort(bool),
            Set(Option<FilterKind>),
            Dialog(DataDlg),
            Close,
        }
        let mut act = None;
        let col = pop.col;
        let kind = pop.kind;
        let area = egui::Area::new(Id::new(("filter_popup", self.uid))).order(egui::Order::Foreground).fixed_pos(pop.at).constrain(true).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(250.0);
                let (asc, desc) = match kind {
                    ColKind::Text => ("filter.sort_az", "filter.sort_za"),
                    ColKind::Number => ("filter.sort_small", "filter.sort_large"),
                    ColKind::Date => ("filter.sort_old", "filter.sort_new"),
                };
                if ui.add(egui::Button::new(tr(asc)).frame(false)).clicked() {
                    act = Some(Act::Sort(true));
                }
                if ui.add(egui::Button::new(tr(desc)).frame(false)).clicked() {
                    act = Some(Act::Sort(false));
                }
                ui.separator();
                let label = trf("filter.clear_from", &[("col", &if header.is_empty() { apex_sheet::addr::col_name(col) } else { header.clone() })]);
                if ui.add_enabled(current.is_some(), egui::Button::new(label).frame(false)).clicked() {
                    act = Some(Act::Set(None));
                }
                if pop.fills.len() > 1 || pop.fonts.len() > 1 {
                    crate::widgets::menu_button(ui, tr("filter.by_color"), |ui| {
                        for (font, list, key) in [(false, &pop.fills, "filter.fill_color"), (true, &pop.fonts, "filter.font_color")] {
                            if list.len() < 2 {
                                continue;
                            }
                            ui.label(RichText::new(tr(key)).weak());
                            ui.horizontal_wrapped(|ui| {
                                for c in list {
                                    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 16.0), Sense::click());
                                    match c {
                                        Some(c) => {
                                            ui.painter().rect_filled(r, 2.0, theme::rgb(*c));
                                        }
                                        None => {
                                            ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, Color32::GRAY), StrokeKind::Inside);
                                        }
                                    }
                                    let tip = match c {
                                        Some(c) => format!("#{c:06X}"),
                                        None => tr(if font { "color.automatic" } else { "color.none" }).to_string(),
                                    };
                                    if resp.on_hover_text(tip).clicked() {
                                        act = Some(Act::Set(Some(FilterKind::Color { font, color: *c })));
                                        ui.close();
                                    }
                                }
                            });
                        }
                    });
                }
                let menu = match kind {
                    ColKind::Text => "filter.text_filters",
                    ColKind::Number => "filter.number_filters",
                    ColKind::Date => "filter.date_filters",
                };
                crate::widgets::menu_button(ui, tr(menu), |ui| {
                    let ops: &[UiOp] = match kind {
                        ColKind::Text => &[UiOp::Eq, UiOp::Ne, UiOp::Begins, UiOp::Ends, UiOp::Contains, UiOp::NotContains],
                        ColKind::Number => &[UiOp::Eq, UiOp::Ne, UiOp::Gt, UiOp::Ge, UiOp::Lt, UiOp::Le],
                        ColKind::Date => &[UiOp::Eq, UiOp::Lt, UiOp::Gt],
                    };
                    let dlg = |a: UiOp, b: UiOp| DataDlg::Custom { col, and: true, a: (a, String::new()), b: (b, String::new()), kind };
                    for &op in ops {
                        if ui.button(format!("{}…", op.label(kind))).clicked() {
                            act = Some(Act::Dialog(dlg(op, UiOp::None)));
                            ui.close();
                        }
                    }
                    if kind != ColKind::Text && ui.button(format!("{}…", tr("filter.between"))).clicked() {
                        act = Some(Act::Dialog(dlg(UiOp::Ge, UiOp::Le)));
                        ui.close();
                    }
                    if kind == ColKind::Number {
                        ui.separator();
                        if ui.button(format!("{}…", tr("filter.top10"))).clicked() {
                            act = Some(Act::Dialog(DataDlg::Top { col, top: true, n: 10.0, percent: false }));
                            ui.close();
                        }
                        if ui.button(tr("filter.above_avg")).clicked() {
                            act = Some(Act::Set(Some(FilterKind::Average { above: true })));
                            ui.close();
                        }
                        if ui.button(tr("filter.below_avg")).clicked() {
                            act = Some(Act::Set(Some(FilterKind::Average { above: false })));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(format!("{}…", tr("filter.custom"))).clicked() {
                        let d = match current.as_ref().map(|c| &c.kind) {
                            Some(FilterKind::Custom { and, a, b }) => DataDlg::Custom {
                                col,
                                and: *and,
                                a: { let (o, s) = UiOp::from_crit(a.0, &a.1); (o, self.operand_out(&s, kind)) },
                                b: b.as_ref().map_or((UiOp::None, String::new()), |b| { let (o, s) = UiOp::from_crit(b.0, &b.1); (o, self.operand_out(&s, kind)) }),
                                kind,
                            },
                            _ => dlg(UiOp::Eq, UiOp::None),
                        };
                        act = Some(Act::Dialog(d));
                        ui.close();
                    }
                });
                ui.add_space(4.0);
                let search = ui.add(egui::TextEdit::singleline(&mut pop.search).hint_text(tr("filter.search")).desired_width(f32::INFINITY));
                if pop.fresh {
                    search.request_focus();
                }
                let q = pop.search.trim().to_lowercase();
                let shown: Vec<usize> = (0..pop.items.len()).filter(|&i| q.is_empty() || pop.items[i].0.to_lowercase().contains(&q)).collect();
                egui::Frame::new().stroke(Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)).inner_margin(4).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                        let all = shown.iter().all(|&i| pop.items[i].1) && (!q.is_empty() || pop.blanks != Some(false));
                        let mut all_now = all;
                        let key = if q.is_empty() { "filter.select_all" } else { "filter.select_results" };
                        if ui.checkbox(&mut all_now, tr(key)).changed() {
                            for &i in &shown {
                                pop.items[i].1 = all_now;
                            }
                            if q.is_empty() {
                                if let Some(b) = &mut pop.blanks {
                                    *b = all_now;
                                }
                            }
                            pop.changed = true;
                        }
                        for &i in shown.iter().take(10_000) {
                            let (text, on) = &mut pop.items[i];
                            let label: String = text.chars().take(80).collect();
                            if ui.checkbox(on, label).changed() {
                                pop.changed = true;
                            }
                        }
                        if q.is_empty() {
                            if let Some(b) = &mut pop.blanks {
                                if ui.checkbox(b, tr("filter.blanks")).changed() {
                                    pop.changed = true;
                                }
                            }
                        }
                    });
                });
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("unsaved.cancel")).clicked() {
                        act = Some(Act::Close);
                    }
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    if ui.button(RichText::new(tr("dialog.ok")).strong()).clicked() || enter {
                        if !pop.changed && q.is_empty() {
                            act = Some(Act::Close);
                        } else {
                            let items: Vec<String> = pop.items.iter().filter(|(t, on)| *on && (q.is_empty() || t.to_lowercase().contains(&q))).map(|(t, _)| t.clone()).collect();
                            let blanks = q.is_empty() && pop.blanks == Some(true);
                            let everything = q.is_empty() && items.len() == pop.items.len() && pop.blanks != Some(false);
                            act = Some(Act::Set(if everything { None } else { Some(FilterKind::Values { items, blanks }) }));
                        }
                    }
                });
            });
        });
        let was_fresh = std::mem::take(&mut pop.fresh);
        let clicked_out = !was_fresh
            && ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)))
            && !ctx.pointer_interact_pos().is_some_and(|p| ctx.layer_id_at(p).is_some_and(|l| l.order == egui::Order::Foreground && l.id != area.response.layer_id.id));
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        match act {
            Some(Act::Sort(asc)) => {
                self.settle_edit();
                if let Some(f) = self.filter_now() {
                    if f != *self.sh().filter.as_ref().unwrap() {
                        self.wb.set_filter(self.sheet, Some(f), "undo.filter");
                    }
                }
                self.wb.filter_sort(self.sheet, col, asc);
                self.focus_grid = true;
            }
            Some(Act::Set(k)) => {
                self.set_col_filter(col, k);
                self.focus_grid = true;
            }
            Some(Act::Dialog(d)) => self.data.dialog = Some(d),
            Some(Act::Close) => self.focus_grid = true,
            None if escape || clicked_out => self.focus_grid = true,
            None => self.data.popup = Some(pop),
        }
    }

    /// Filter operand typed by the user, stored for matching.
    fn operand_in(&self, text: &str) -> String {
        let t = text.trim();
        if t.contains(['*', '?']) {
            return t.to_string();
        }
        match apex_sheet::format::parse_input(t, &self.wb.locale).value {
            Value::Number(n) => format!("{n}"),
            _ => t.to_string(),
        }
    }

    fn operand_out(&self, stored: &str, kind: ColKind) -> String {
        match stored.parse::<f64>() {
            Ok(n) if kind == ColKind::Date => NumFormat::parse(&date_code(&self.wb.locale)).format(&Value::Number(n), &self.wb.locale).0,
            Ok(n) => self.wb.format_value(0, &Value::Number(n)).0,
            Err(_) => stored.to_string(),
        }
    }

    // --- lists -------------------------------------------------------------------

    /// Opens the validation list, or the column's values (pick from list).
    pub(super) fn open_list(&mut self) {
        self.settle_edit();
        let c = self.cursor;
        let items = match self.wb.validation_at(self.sheet, c) {
            Some(v) if v.kind == ValKind::List => self.wb.validation_items(self.sheet, c),
            _ => {
                let mut v = self.wb.column_texts(self.sheet, c);
                v.sort_by_key(|s| s.to_lowercase());
                v
            }
        };
        if items.is_empty() {
            return;
        }
        let cur = self.wb.display(self.sheet, c).0;
        let index = items.iter().position(|i| *i == cur).unwrap_or(0);
        self.data.popup = None;
        self.data.list = Some(ListPopup { cell: c, items, index, fresh: true });
    }

    /// Keys for the open list; true if one was used.
    pub(super) fn list_keys(&mut self, ui: &egui::Ui) -> bool {
        let Some(l) = &mut self.data.list else { return false };
        let consume = |k: Key| ui.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        let n = l.items.len();
        if consume(Key::ArrowDown) {
            l.index = (l.index + 1).min(n - 1);
        } else if consume(Key::ArrowUp) {
            l.index = l.index.saturating_sub(1);
        } else if consume(Key::Enter) || consume(Key::Tab) {
            let (c, v) = (l.cell, l.items[l.index].clone());
            self.pick(c, v);
        } else if consume(Key::Escape) || ui.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowUp)) {
            self.data.list = None;
        } else {
            return false;
        }
        true
    }

    fn pick(&mut self, c: CellRef, v: String) {
        self.data.list = None;
        if self.wb.edit_text(self.sheet, c) != v {
            self.wb.set_input(self.sheet, c, &v);
        }
        self.focus_grid = true;
    }

    fn list_popup(&mut self, ctx: &egui::Context) {
        let Some(l) = &mut self.data.list else { return };
        if l.cell != self.cursor || self.edit.is_some() {
            self.data.list = None;
            return;
        }
        let Some(r) = self.data.cursor_rect else { return };
        let mut chosen = None;
        let fresh = std::mem::take(&mut l.fresh);
        let area = egui::Area::new(Id::new(("val_list", self.uid))).order(egui::Order::Foreground).fixed_pos(r.left_bottom() + vec2(0.0, 1.0)).constrain(true).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(2).show(ui, |ui| {
                let w = r.width().max(120.0);
                ui.set_width(w);
                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    for (i, item) in l.items.iter().enumerate() {
                        let resp = ui.add(egui::Button::selectable(i == l.index, item.as_str()).min_size(vec2(w - 8.0, 0.0)).truncate());
                        if i == l.index && fresh {
                            resp.scroll_to_me(None);
                        }
                        if resp.clicked() {
                            chosen = Some(item.clone());
                        }
                    }
                });
            });
        });
        let cell = l.cell;
        if let Some(v) = chosen {
            self.pick(cell, v);
        } else if !fresh && ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p))) {
            self.data.list = None;
        }
    }

    fn input_message(&self, ctx: &egui::Context) {
        if self.data.popup.is_some() || self.data.list.is_some() || self.data.dialog.is_some() || self.dialog.is_some() {
            return;
        }
        let Some(r) = self.data.cursor_rect else { return };
        let Some(v) = self.wb.validation_at(self.sheet, self.cursor) else { return };
        if !v.show_input || (v.input_title.is_empty() && v.input_msg.is_empty()) {
            return;
        }
        egui::Area::new(Id::new(("val_input", self.uid))).order(egui::Order::Middle).fixed_pos(pos2(r.min.x + r.width().min(60.0), r.max.y + 6.0)).interactable(false).show(ctx, |ui| {
            egui::Frame::new().fill(Color32::from_rgb(0xFF, 0xFD, 0xE7)).stroke(Stroke::new(1.0, Color32::from_gray(150))).inner_margin(6).show(ui, |ui| {
                ui.set_max_width(260.0);
                if !v.input_title.is_empty() {
                    ui.label(RichText::new(&v.input_title).strong().color(Paper::TEXT));
                }
                if !v.input_msg.is_empty() {
                    ui.label(RichText::new(&v.input_msg).color(Paper::TEXT));
                }
            });
        });
    }

    // --- validation ------------------------------------------------------------------

    pub(super) fn open_validation(&mut self) {
        self.settle_edit();
        let v = self.wb.validation_at(self.sheet, self.cursor).cloned().unwrap_or_else(|| Validation::new(Vec::new()));
        let (f1, f2) = match v.kind {
            ValKind::List => match v.literal_items() {
                Some(items) => (items.join(&self.list_sep().to_string()), String::new()),
                None => (self.wb.rule_display(&format!("={}", v.f1)).trim_start_matches("==").to_string(), String::new()),
            },
            ValKind::Custom => (format!("={}", self.formula_display(&v.f1)), String::new()),
            _ => (self.val_display(&v.f1, v.kind), self.val_display(&v.f2, v.kind)),
        };
        self.data.dialog = Some(DataDlg::Validation(Box::new(ValDlg { tab: 0, v, f1, f2 })));
    }

    /// Formula text (without `=`) for display in the user's locale.
    pub(super) fn formula_display(&self, canonical: &str) -> String {
        match apex_sheet::formula::parse_canonical(canonical) {
            Ok(e) => e.to_display(&self.wb.locale).trim_start_matches('=').to_string(),
            Err(_) => canonical.to_string(),
        }
    }

    fn val_display(&self, stored: &str, kind: ValKind) -> String {
        let code = match kind {
            ValKind::Date => Some(date_code(&self.wb.locale)),
            ValKind::Time => Some("hh:mm".to_string()),
            _ => None,
        };
        match (code, apex_sheet::formula::parse_canonical(stored)) {
            (Some(code), Ok(apex_sheet::formula::Expr::Number(n))) => NumFormat::parse(&code).format(&Value::Number(n), &self.wb.locale).0,
            _ => self.wb.rule_display(stored),
        }
    }

    fn apply_validation(&mut self, d: ValDlg) {
        let sel = self.sel();
        let mut v = d.v;
        v.ranges = vec![sel];
        match v.kind {
            ValKind::List => {
                let src = d.f1.trim();
                if src.starts_with('=') {
                    v.f1 = self.wb.rule_input(src);
                } else {
                    let items: Vec<&str> = src.split(self.list_sep()).map(str::trim).filter(|s| !s.is_empty()).collect();
                    v.f1 = format!("\"{}\"", items.join(",").replace('"', "\"\""));
                }
                v.f2.clear();
            }
            ValKind::Custom => {
                let t = d.f1.trim();
                let t = if t.starts_with('=') { t.to_string() } else { format!("={t}") };
                v.f1 = self.wb.rule_input(&t);
                v.f2.clear();
            }
            ValKind::Any => {
                v.f1.clear();
                v.f2.clear();
            }
            _ => {
                v.f1 = self.wb.rule_input(&d.f1);
                v.f2 = if v.op.two_args() { self.wb.rule_input(&d.f2) } else { String::new() };
            }
        }
        let mut list: Vec<Validation> = self
            .sh()
            .validations
            .iter()
            .cloned()
            .filter_map(|mut x| {
                x.ranges = ranges_minus(&x.ranges, &sel);
                (!x.ranges.is_empty()).then_some(x)
            })
            .collect();
        let has_msg = v.show_input && (!v.input_title.is_empty() || !v.input_msg.is_empty());
        if v.kind != ValKind::Any || has_msg {
            list.push(v);
        }
        if list != self.sh().validations {
            self.wb.execute(Op::Validations { sheet: self.sheet, list });
        }
    }

    /// Checks a value just typed into `c`: shows the error alert if needed.
    pub(super) fn check_validation(&mut self, c: CellRef, text: String) {
        let Some(v) = self.wb.validation_at(self.sheet, c) else { return };
        if !v.show_error || self.wb.is_valid(self.sheet, c) {
            return;
        }
        self.data.dialog = Some(DataDlg::ValError { sheet: self.sheet, cell: c, text, style: v.error_style, title: v.error_title.clone(), msg: v.error_msg.clone() });
    }

    // --- remove duplicates and text to columns ------------------------------------

    pub(super) fn data_range(&self) -> Range {
        let sel = self.sel();
        if sel.is_single() {
            self.wb.current_region(self.sheet, self.cursor)
        } else {
            self.wb.clamp_to_used(self.sheet, &sel).map_or(sel, |u| Range::new(sel.start, u.end))
        }
    }

    pub(super) fn open_remove_dups(&mut self) {
        self.settle_edit();
        let range = self.data_range();
        let sh = self.sh();
        // A header row: all text over some other kind of value below.
        let header = range.rows() > 1
            && (range.start.col..=range.end.col).all(|c| matches!(sh.value(CellRef::new(range.start.row, c)), Value::Text(_)))
            && (range.start.col..=range.end.col).any(|c| !matches!(sh.value(CellRef::new(range.start.row + 1, c)), Value::Text(_)));
        self.anchor = range.start;
        self.cursor = range.end;
        self.data.dialog = Some(DataDlg::RemoveDups { range, header, cols: vec![true; range.cols().min(500) as usize] });
    }

    pub(super) fn open_text_cols(&mut self) {
        self.settle_edit();
        let sel = self.sel();
        let range = match self.wb.clamp_to_used(self.sheet, &sel) {
            Some(u) => Range::new(sel.start, CellRef::new(u.end.row, sel.start.col)),
            None => {
                self.notices.push(Notice::info(tr("textcols.no_data")));
                return;
            }
        };
        // Guess the delimiter from the first cells.
        let mut sample = String::new();
        for row in range.start.row..=range.end.row.min(range.start.row + 20) {
            sample.push_str(&self.wb.display(self.sheet, CellRef::new(row, range.start.col)).0);
        }
        let count = |ch: char| sample.matches(ch).count();
        let delims = [count('\t') > 0, count(';') > 0 && count(';') >= count(','), count(',') > 0 && count(',') > count(';'), false];
        let delims = if delims.iter().any(|x| *x) { delims } else { [true, false, false, count(' ') > 0] };
        self.data.dialog = Some(DataDlg::TextCols(Box::new(TextColsDlg {
            range,
            fixed: false,
            delims,
            other: false,
            other_ch: String::new(),
            merge: false,
            quote: 0,
            breaks: String::new(),
            dest: range.start.to_a1(),
        })));
    }

    // --- typing autocomplete ---------------------------------------------------------

    /// The column value that completes what is being typed in the cell.
    pub(super) fn value_completion(&self) -> Option<String> {
        let e = self.edit?;
        if e.origin != EditOrigin::Cell || self.buf.is_empty() || self.buf.starts_with('=') || self.buf.contains('\n') || self.ac_hidden.as_deref() == Some(self.buf.as_str()) {
            return None;
        }
        if apex_sheet::format::parse_input(&self.buf, &self.wb.locale).value.as_number().is_some() {
            return None;
        }
        let key = (self.sheet, self.cursor, self.wb.revision);
        let mut cache = self.data.col_texts.borrow_mut();
        if cache.as_ref().is_none_or(|(k, _)| *k != key) {
            *cache = Some((key, self.wb.column_texts(self.sheet, self.cursor)));
        }
        let texts = &cache.as_ref().unwrap().1;
        let low = self.buf.to_lowercase();
        let mut found = texts.iter().filter(|t| t.len() > self.buf.len() && t.to_lowercase().starts_with(&low));
        let first = found.next()?;
        if found.any(|t| t.to_lowercase() != first.to_lowercase()) {
            return None;
        }
        // Keep what was typed and add the rest.
        let rest: String = first.chars().skip(self.buf.chars().count()).collect();
        Some(format!("{}{rest}", self.buf))
    }

    // --- dialogs -------------------------------------------------------------------

    pub(super) fn data_dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.data.dialog.take() else { return };
        let res = match &mut d {
            DataDlg::Custom { col, and, a, b, kind } => {
                let items = self.sh().filter.clone().map(|f| self.wb.filter_values(self.sheet, &f, *col).0).unwrap_or_default();
                let header = self.sh().filter.as_ref().map(|f| self.wb.display(self.sheet, CellRef::new(f.range.start.row, *col)).0).unwrap_or_default();
                let kind = *kind;
                widgets::dialog(ctx, "custom_filter", tr("filter.custom_title"), 440.0, |ui| {
                    ui.label(trf("filter.show_rows", &[("col", &header)]));
                    ui.add_space(4.0);
                    cond_row(ui, "cf_a", a, kind, &items, false);
                    ui.horizontal(|ui| {
                        ui.radio_value(and, true, tr("filter.and"));
                        ui.radio_value(and, false, tr("filter.or"));
                    });
                    cond_row(ui, "cf_b", b, kind, &items, true);
                    ui.add_space(4.0);
                    ui.label(RichText::new(tr("filter.wildcards")).small().weak());
                })
            }
            DataDlg::Top { top, n, percent, .. } => widgets::dialog(ctx, "top10", tr("filter.top10_title"), 360.0, |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("top_dir").selected_text(tr(if *top { "filter.top" } else { "filter.bottom" })).show_ui(ui, |ui| {
                        ui.selectable_value(top, true, tr("filter.top"));
                        ui.selectable_value(top, false, tr("filter.bottom"));
                    });
                    ui.add(egui::DragValue::new(n).range(1.0..=500.0).speed(0.2).max_decimals(0));
                    egui::ComboBox::from_id_salt("top_unit").selected_text(tr(if *percent { "filter.percent" } else { "filter.items" })).show_ui(ui, |ui| {
                        ui.selectable_value(percent, false, tr("filter.items"));
                        ui.selectable_value(percent, true, tr("filter.percent"));
                    });
                });
            }),
            DataDlg::Validation(v) => {
                let clear = std::cell::Cell::new(false);
                let pages = [(Icon::Validation, "val.settings"), (Icon::Comment, "val.input_tab"), (Icon::ErrorCheck, "val.error_tab")];
                let mut tab = v.tab;
                let r = widgets::nav_dialog(ctx, "validation", tr("val.title"), &pages, &mut tab, vec2(440.0, 330.0), |ui, page| {
                    validation_ui(ui, v, page);
                    if page == 0 {
                        ui.add_space(14.0);
                        if ui.button(tr("val.clear_all")).clicked() {
                            clear.set(true);
                        }
                    }
                });
                v.tab = tab;
                if clear.get() {
                    v.v = Validation::new(Vec::new());
                    v.v.kind = ValKind::Any;
                    v.f1.clear();
                    v.f2.clear();
                }
                r
            }
            DataDlg::ValError { .. } => {
                // Its own buttons.
                self.val_error_dialog(ctx, d);
                return;
            }
            DataDlg::RemoveDups { range, header, cols } => {
                let names: Vec<String> = (0..cols.len() as u32)
                    .map(|i| {
                        let c = range.start.col + i;
                        let h = if *header { self.wb.display(self.sheet, CellRef::new(range.start.row, c)).0 } else { String::new() };
                        if h.is_empty() { format!("{} {}", tr("sort.column"), apex_sheet::addr::col_name(c)) } else { h }
                    })
                    .collect();
                widgets::dialog(ctx, "remove_dups", tr("dups.title"), 360.0, |ui| {
                    ui.label(tr("dups.help"));
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr("dups.select_all")).clicked() {
                            cols.iter_mut().for_each(|c| *c = true);
                        }
                        if ui.button(tr("dups.unselect_all")).clicked() {
                            cols.iter_mut().for_each(|c| *c = false);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.checkbox(header, tr("dups.header")));
                    });
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        for (on, name) in cols.iter_mut().zip(&names) {
                            ui.checkbox(on, name);
                        }
                    });
                })
            }
            DataDlg::BankRec(b) => self.bank_dialog(ctx, b),
            DataDlg::TextCols(t) => {
                let rows: Vec<String> = (t.range.start.row..=t.range.end.row.min(t.range.start.row + 7)).map(|r| self.wb.display(self.sheet, CellRef::new(r, t.range.start.col)).0).collect();
                widgets::dialog(ctx, "text_cols", tr("textcols.title"), 520.0, |ui| text_cols_ui(ui, t, &rows))
            }
            DataDlg::Group { .. } | DataDlg::Subtotals(_) => self.outline_dialog(ctx, &mut d).unwrap_or(widgets::DialogResult::Cancel),
            DataDlg::Cf(c) => {
                let r = self.cf_dialog(ctx, c);
                if let Some(next) = r.1 {
                    self.data.dialog = Some(next);
                    return;
                }
                r.0
            }
        };
        match res {
            widgets::DialogResult::None => self.data.dialog = Some(d),
            widgets::DialogResult::Cancel => self.focus_grid = true,
            widgets::DialogResult::Ok => {
                self.focus_grid = true;
                self.data_dialog_ok(d);
            }
        }
    }

    fn data_dialog_ok(&mut self, d: DataDlg) {
        match d {
            DataDlg::Custom { col, and, a, b, kind: _ } => {
                let a_val = self.operand_in(&a.1);
                let b_val = self.operand_in(&b.1);
                let Some(a) = a.0.to_crit(&a_val) else { return };
                let b = if b.1.trim().is_empty() && !matches!(b.0, UiOp::Eq | UiOp::Ne) { None } else { b.0.to_crit(&b_val) };
                self.set_col_filter(col, Some(FilterKind::Custom { and, a, b }));
            }
            DataDlg::Top { col, top, n, percent } => self.set_col_filter(col, Some(FilterKind::Top { top, percent, n })),
            DataDlg::Validation(v) => self.apply_validation(*v),
            DataDlg::RemoveDups { range, header, cols } => {
                let keys: Vec<u32> = cols.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| range.start.col + i as u32).collect();
                if keys.is_empty() {
                    return;
                }
                let (removed, kept) = self.wb.remove_duplicates(self.sheet, &range, &keys, header);
                let msg = if removed == 0 { trf("dups.none", &[("kept", &kept.to_string())]) } else { trf("dups.done", &[("removed", &removed.to_string()), ("kept", &kept.to_string())]) };
                self.notices.push(Notice::info(msg));
            }
            DataDlg::TextCols(t) => {
                let dest = CellRef::parse(t.dest.trim().trim_start_matches('=').replace('$', "").as_str()).unwrap_or(t.range.start);
                let mode = t.mode();
                self.wb.text_to_columns(self.sheet, &t.range, &mode, dest, &[]);
            }
            DataDlg::BankRec(b) => self.bank_dialog_ok(b),
            DataDlg::Group { .. } | DataDlg::Subtotals(_) => self.outline_dialog_ok(d),
            DataDlg::ValError { .. } | DataDlg::Cf(_) => {}
        }
    }

    fn val_error_dialog(&mut self, ctx: &egui::Context, d: DataDlg) {
        let DataDlg::ValError { sheet, cell, text, style, title, msg } = &d else { return };
        #[derive(PartialEq)]
        enum Btn {
            Keep,
            Retry,
            Undo,
        }
        let mut btn = None;
        let title = if title.is_empty() { "ApexDesk" } else { title.as_str() };
        let msg = if msg.is_empty() { tr("val.default_error") } else { msg.as_str() };
        egui::Modal::new(Id::new(("val_error", self.uid))).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(title);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                paint_alert(ui.painter(), r, *style);
                ui.vertical(|ui| {
                    ui.label(msg);
                    if *style == ErrorStyle::Warning {
                        ui.add_space(4.0);
                        ui.label(tr("val.continue"));
                    }
                });
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match style {
                ErrorStyle::Stop => {
                    if ui.button(tr("unsaved.cancel")).clicked() {
                        btn = Some(Btn::Undo);
                    }
                    if ui.button(RichText::new(tr("val.retry")).strong()).clicked() {
                        btn = Some(Btn::Retry);
                    }
                }
                ErrorStyle::Warning => {
                    if ui.button(tr("unsaved.cancel")).clicked() {
                        btn = Some(Btn::Undo);
                    }
                    if ui.button(tr("val.no")).clicked() {
                        btn = Some(Btn::Retry);
                    }
                    if ui.button(RichText::new(tr("val.yes")).strong()).clicked() {
                        btn = Some(Btn::Keep);
                    }
                }
                ErrorStyle::Info => {
                    if ui.button(tr("unsaved.cancel")).clicked() {
                        btn = Some(Btn::Undo);
                    }
                    if ui.button(RichText::new(tr("dialog.ok")).strong()).clicked() {
                        btn = Some(Btn::Keep);
                    }
                }
            });
        });
        if btn.is_none() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                btn = Some(Btn::Undo);
            } else if ctx.input(|i| i.key_pressed(Key::Enter)) {
                btn = Some(if *style == ErrorStyle::Stop { Btn::Retry } else { Btn::Keep });
            }
        }
        let Some(btn) = btn else {
            self.data.dialog = Some(d);
            return;
        };
        if btn != Btn::Keep {
            self.wb.undo();
            self.wb.discard_redo();
        }
        if *sheet == self.sheet {
            self.anchor = *cell;
            self.cursor = *cell;
            self.ensure_visible = true;
        }
        if btn == Btn::Retry {
            self.begin_edit(ctx, EditOrigin::Cell, false, Some(text.clone()));
        } else {
            self.focus_grid = true;
        }
    }
}

/// Stop, warning or information symbol.
fn paint_alert(p: &egui::Painter, r: Rect, style: ErrorStyle) {
    let c = r.center();
    let s = r.width() * 0.45;
    let white = Stroke::new(s * 0.2, Color32::WHITE);
    match style {
        ErrorStyle::Stop => {
            p.circle_filled(c, s, Color32::from_rgb(0xD0, 0x30, 0x30));
            let k = s * 0.42;
            p.line_segment([c + vec2(-k, -k), c + vec2(k, k)], white);
            p.line_segment([c + vec2(-k, k), c + vec2(k, -k)], white);
        }
        ErrorStyle::Warning => {
            p.add(egui::Shape::convex_polygon(vec![c + vec2(0.0, -s), c + vec2(s, s * 0.8), c + vec2(-s, s * 0.8)], Color32::from_rgb(0xE0, 0x9A, 0x10), Stroke::NONE));
            p.line_segment([c + vec2(0.0, -s * 0.35), c + vec2(0.0, s * 0.3)], white);
            p.circle_filled(c + vec2(0.0, s * 0.56), s * 0.1, Color32::WHITE);
        }
        ErrorStyle::Info => {
            p.circle_filled(c, s, theme::ACCENT);
            p.line_segment([c + vec2(0.0, -s * 0.1), c + vec2(0.0, s * 0.55)], white);
            p.circle_filled(c + vec2(0.0, -s * 0.45), s * 0.12, Color32::WHITE);
        }
    }
}

/// One condition row of the custom filter dialog.
fn cond_row(ui: &mut egui::Ui, id: &str, c: &mut (UiOp, String), kind: ColKind, items: &[String], optional: bool) {
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt((id, "op")).width(170.0).selected_text(c.0.label(kind)).show_ui(ui, |ui| {
            if optional {
                ui.selectable_value(&mut c.0, UiOp::None, tr(UiOp::None.key()));
            }
            for op in UiOp::ALL {
                ui.selectable_value(&mut c.0, op, op.label(kind));
            }
        });
        let enabled = c.0 != UiOp::None;
        ui.add_enabled(enabled, egui::TextEdit::singleline(&mut c.1).desired_width(170.0));
        ui.add_enabled_ui(enabled && !items.is_empty(), |ui| {
            egui::ComboBox::from_id_salt((id, "vals")).width(24.0).selected_text("").show_ui(ui, |ui| {
                for it in items.iter().take(500) {
                    if ui.selectable_label(false, it.as_str()).clicked() {
                        c.1 = it.clone();
                    }
                }
            });
        });
    });
}

fn validation_ui(ui: &mut egui::Ui, d: &mut ValDlg, page: usize) {
    let v = &mut d.v;
    match page {
        0 => {
            widgets::section(ui, tr("val.criteria"));
            widgets::form(ui, "val_grid", |ui| {
                ui.label(tr("val.allow"));
                let name = VAL_KINDS.iter().find(|k| k.0 == v.kind).map_or("", |k| tr(k.1));
                egui::ComboBox::from_id_salt("val_kind").width(200.0).selected_text(name).show_ui(ui, |ui| {
                    for (k, key) in VAL_KINDS {
                        ui.selectable_value(&mut v.kind, k, tr(key));
                    }
                });
                ui.end_row();
                if v.kind.has_op() {
                    ui.label(tr("val.data"));
                    egui::ComboBox::from_id_salt("val_op").width(200.0).selected_text(tr(cmp_key(v.op))).show_ui(ui, |ui| {
                        for op in CmpOp::ALL {
                            ui.selectable_value(&mut v.op, op, tr(cmp_key(op)));
                        }
                    });
                    ui.end_row();
                    let hint = match v.kind {
                        ValKind::Date => "2026-01-31",
                        ValKind::Time => "9:00",
                        _ => "",
                    };
                    if v.op.two_args() {
                        ui.label(tr("val.minimum"));
                        ui.add(egui::TextEdit::singleline(&mut d.f1).hint_text(hint).desired_width(200.0));
                        ui.end_row();
                        ui.label(tr("val.maximum"));
                        ui.add(egui::TextEdit::singleline(&mut d.f2).hint_text(hint).desired_width(200.0));
                        ui.end_row();
                    } else {
                        ui.label(tr("val.value"));
                        ui.add(egui::TextEdit::singleline(&mut d.f1).hint_text(hint).desired_width(200.0));
                        ui.end_row();
                    }
                }
                match v.kind {
                    ValKind::List => {
                        ui.label(tr("val.source"));
                        ui.add(egui::TextEdit::singleline(&mut d.f1).hint_text(tr("val.source_hint")).desired_width(200.0));
                        ui.end_row();
                    }
                    ValKind::Custom => {
                        ui.label(tr("val.formula"));
                        ui.add(egui::TextEdit::singleline(&mut d.f1).hint_text("=A1>0").desired_width(200.0));
                        ui.end_row();
                    }
                    _ => {}
                }
            });
            ui.add_space(8.0);
            if v.kind != ValKind::Any {
                ui.checkbox(&mut v.allow_blank, tr("val.ignore_blank"));
            }
            if v.kind == ValKind::List {
                ui.checkbox(&mut v.dropdown, tr("val.dropdown"));
            }
        }
        1 => {
            widgets::section(ui, tr("val.input_tab"));
            ui.checkbox(&mut v.show_input, tr("val.show_input"));
            ui.add_space(8.0);
            ui.add_enabled_ui(v.show_input, |ui| {
                ui.label(tr("val.msg_title"));
                ui.add(egui::TextEdit::singleline(&mut v.input_title).desired_width(f32::INFINITY));
                ui.add_space(4.0);
                ui.label(tr("val.input_msg"));
                ui.add(egui::TextEdit::multiline(&mut v.input_msg).desired_rows(5).desired_width(f32::INFINITY));
            });
        }
        _ => {
            widgets::section(ui, tr("val.error_tab"));
            ui.checkbox(&mut v.show_error, tr("val.show_error"));
            ui.add_space(8.0);
            ui.add_enabled_ui(v.show_error, |ui| {
            widgets::form(ui, "val_err", |ui| {
                ui.label(tr("val.style"));
                let key = |s: ErrorStyle| match s {
                    ErrorStyle::Stop => "val.stop",
                    ErrorStyle::Warning => "val.warning",
                    ErrorStyle::Info => "val.info",
                };
                egui::ComboBox::from_id_salt("val_style").width(160.0).selected_text(tr(key(v.error_style))).show_ui(ui, |ui| {
                    for s in [ErrorStyle::Stop, ErrorStyle::Warning, ErrorStyle::Info] {
                        ui.selectable_value(&mut v.error_style, s, tr(key(s)));
                    }
                });
                ui.end_row();
                ui.label(tr("val.msg_title"));
                ui.add(egui::TextEdit::singleline(&mut v.error_title).desired_width(260.0));
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.label(tr("val.error_msg"));
            ui.add(egui::TextEdit::multiline(&mut v.error_msg).desired_rows(5).desired_width(f32::INFINITY));
            });
        }
    }
}

fn text_cols_ui(ui: &mut egui::Ui, t: &mut TextColsDlg, rows: &[String]) {
    ui.horizontal(|ui| {
        ui.radio_value(&mut t.fixed, false, tr("textcols.delimited"));
        ui.radio_value(&mut t.fixed, true, tr("textcols.fixed"));
    });
    ui.add_space(4.0);
    if t.fixed {
        ui.label(tr("textcols.breaks_help"));
        ui.add(egui::TextEdit::singleline(&mut t.breaks).hint_text("10, 25").desired_width(200.0));
    } else {
        ui.horizontal_wrapped(|ui| {
            for (on, key) in t.delims.iter_mut().zip(["textcols.tab", "textcols.semicolon", "textcols.comma", "textcols.space"]) {
                ui.checkbox(on, tr(key));
            }
            ui.checkbox(&mut t.other, tr("textcols.other"));
            ui.add_enabled(t.other, egui::TextEdit::singleline(&mut t.other_ch).char_limit(1).desired_width(24.0));
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut t.merge, tr("textcols.merge"));
            ui.label(tr("textcols.qualifier"));
            let names = ["\"", "'", tr("textcols.none")];
            egui::ComboBox::from_id_salt("tc_quote").width(60.0).selected_text(names[t.quote]).show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut t.quote, i, *n);
                }
            });
        });
    }
    ui.horizontal(|ui| {
        ui.label(tr("textcols.dest"));
        ui.add(egui::TextEdit::singleline(&mut t.dest).desired_width(90.0));
    });
    ui.add_space(6.0);
    ui.label(RichText::new(tr("textcols.preview")).strong());
    let mode = t.mode();
    egui::Frame::new().stroke(Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)).inner_margin(4).show(ui, |ui| {
        egui::ScrollArea::horizontal().show(ui, |ui| {
            egui::Grid::new("tc_preview").striped(true).show(ui, |ui| {
                for r in rows {
                    for part in split_text(r, &mode).iter().take(30) {
                        ui.monospace(part.chars().take(24).collect::<String>());
                    }
                    ui.end_row();
                }
            });
        });
    });
}
