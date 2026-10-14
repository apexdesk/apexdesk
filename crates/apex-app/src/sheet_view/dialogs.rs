//! Spreadsheet dialogs and the find & replace bar.

use apex_sheet::functions::Func;

use super::*;
use crate::formula_help;

pub(super) enum Dialog {
    FormatCells(Box<FormatDlg>),
    PasteSpecial(PasteSpecial),
    /// Insert or delete cells: 0 shift right/left, 1 shift down/up,
    /// 2 entire rows, 3 entire columns.
    Cells { delete: bool, choice: u8 },
    /// Row height or column width, in points.
    Size { rows: bool, value: f32 },
    Series { opts: SeriesOpts, step: String, stop: String },
    Sort { keys: Vec<SortKey>, header: bool, case: bool, by_cols: bool, page: usize, sel: usize },
    GoTo(String),
    Special(SpecialDlg),
    Calc(apex_sheet::CalcOptions),
    MergeWarn(u8),
    Functions { query: String, cat: usize, selected: Option<Pick> },
}

/// Go To Special: the option picked (as in `SPECIAL_OPTS`), the value
/// types for constants and formulas, and "same" rules only.
#[derive(Clone, Copy, Debug)]
pub(super) struct SpecialDlg {
    pick: u8,
    types: u8,
    same: bool,
}

impl Default for SpecialDlg {
    fn default() -> Self {
        SpecialDlg { pick: 1, types: apex_sheet::workbook::SPECIAL_ALL_TYPES, same: false }
    }
}

/// Options of Go To Special in two columns.
const SPECIAL_OPTS: [&str; 11] = [
    "goto.notes",
    "goto.constants",
    "goto.formulas",
    "goto.blanks",
    "goto.region",
    "goto.row_diffs",
    "goto.col_diffs",
    "goto.last",
    "goto.visible",
    "goto.cf",
    "goto.validation",
];

impl SpecialDlg {
    fn kind(&self) -> apex_sheet::Special {
        use apex_sheet::Special as S;
        match self.pick {
            0 => S::Notes,
            1 => S::Constants(self.types),
            2 => S::Formulas(self.types),
            3 => S::Blanks,
            4 => S::CurrentRegion,
            5 => S::RowDiffs,
            6 => S::ColDiffs,
            7 => S::LastCell,
            8 => S::Visible,
            9 => S::CondFormats { same: self.same },
            _ => S::Validation { same: self.same },
        }
    }
}

fn calc_ui(ui: &mut egui::Ui, o: &mut apex_sheet::CalcOptions) {
    widgets::section(ui, tr("calc.mode"));
    ui.horizontal(|ui| {
        ui.radio_value(&mut o.manual, false, tr("calc.automatic"));
        ui.radio_value(&mut o.manual, true, tr("calc.manual"));
    });
    ui.add_space(8.0);
    ui.checkbox(&mut o.iterate, tr("calc.iterate"));
    ui.add_enabled_ui(o.iterate, |ui| {
        egui::Grid::new("calc_iter").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(tr("calc.max_iter"));
            ui.add(egui::DragValue::new(&mut o.max_iter).range(1..=32767));
            ui.end_row();
            ui.label(tr("calc.max_change"));
            ui.add(egui::DragValue::new(&mut o.max_change).range(0.0..=1e6).speed(0.001).max_decimals(6));
            ui.end_row();
        });
        ui.label(RichText::new(tr("calc.iterate_hint")).weak());
    });
    ui.add_space(8.0);
    ui.checkbox(&mut o.date1904, tr("calc.date1904")).on_hover_text(tr("calc.date1904_tip"));
    ui.checkbox(&mut o.r1c1, tr("calc.r1c1")).on_hover_text(tr("calc.r1c1_tip"));
}

fn special_ui(ui: &mut egui::Ui, d: &mut SpecialDlg) {
    use apex_sheet::workbook::{SPECIAL_ERRORS, SPECIAL_LOGICAL, SPECIAL_NUMBERS, SPECIAL_TEXT};
    ui.columns(2, |cols| {
        for (i, key) in SPECIAL_OPTS.iter().enumerate() {
            let ui = &mut cols[usize::from(i >= 7)];
            ui.radio_value(&mut d.pick, i as u8, tr(key));
            if i == 2 {
                // The value types go under Formulas and apply to Constants too.
                let on = matches!(d.pick, 1 | 2);
                ui.add_enabled_ui(on, |ui| {
                    for (bit, key) in [(SPECIAL_NUMBERS, "goto.numbers"), (SPECIAL_TEXT, "goto.text"), (SPECIAL_LOGICAL, "goto.logical"), (SPECIAL_ERRORS, "goto.errors")] {
                        ui.horizontal(|ui| {
                            ui.add_space(22.0);
                            let mut v = d.types & bit != 0;
                            if ui.checkbox(&mut v, tr(key)).changed() {
                                d.types ^= bit;
                            }
                        });
                    }
                });
            }
            if i == 10 {
                ui.add_enabled_ui(matches!(d.pick, 9 | 10), |ui| {
                    for (v, key) in [(false, "goto.all"), (true, "goto.same")] {
                        ui.horizontal(|ui| {
                            ui.add_space(22.0);
                            ui.radio_value(&mut d.same, v, tr(key));
                        });
                    }
                });
            }
        }
    });
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum NumCat {
    General,
    Number,
    Currency,
    Accounting,
    Percent,
    Scientific,
    Fraction,
    Date,
    Time,
    Text,
    Custom,
}

const CATS: [(NumCat, &str); 11] = [
    (NumCat::General, "numfmt.general"),
    (NumCat::Number, "numfmt.number"),
    (NumCat::Currency, "numfmt.currency"),
    (NumCat::Accounting, "numfmt.accounting"),
    (NumCat::Percent, "numfmt.percent"),
    (NumCat::Scientific, "numfmt.scientific"),
    (NumCat::Fraction, "numfmt.fraction"),
    (NumCat::Date, "numfmt.date"),
    (NumCat::Time, "numfmt.time"),
    (NumCat::Text, "numfmt.text"),
    (NumCat::Custom, "numfmt.custom"),
];

const SYMBOLS: [(&str, bool); 5] = [("€", true), ("$", false), ("£", false), ("¥", false), ("CHF", true)];

pub(super) struct FormatDlg {
    tab: usize,
    st: CellStyle,
    orig: CellStyle,
    cat: NumCat,
    decimals: usize,
    thousands: bool,
    neg_red: bool,
    symbol: usize,
    code: String,
    /// Border sides to change: top, bottom, left, right, inside horizontal,
    /// inside vertical. `None` = leave as is.
    sides: [Option<Line>; 6],
    line: Line,
    multi: bool,
}

impl FormatDlg {
    pub(super) fn new(st: CellStyle, tab: usize, loc: &apex_sheet::Locale, multi: bool) -> FormatDlg {
        let code = st.num_fmt.to_string();
        let nf = apex_sheet::NumFormat::parse(&code);
        let decimals = code.split(';').next().and_then(|c| c.find('.').map(|d| c[d + 1..].chars().take_while(|&c| c == '0').count())).unwrap_or(if code == "General" { 2 } else { 0 });
        let symbol = SYMBOLS.iter().position(|(s, _)| code.contains(&format!("\"{s}\""))).unwrap_or_else(|| SYMBOLS.iter().position(|(s, _)| *s == loc.currency).unwrap_or(0));
        let cat = if code.eq_ignore_ascii_case("general") {
            NumCat::General
        } else if code == "@" {
            NumCat::Text
        } else if nf.is_date() {
            if code.contains('y') || code.contains('d') { NumCat::Date } else { NumCat::Time }
        } else if nf.has_fill() && code.contains('_') {
            NumCat::Accounting
        } else if code.contains('%') {
            NumCat::Percent
        } else if code.contains("E+") {
            NumCat::Scientific
        } else if code.contains('?') {
            NumCat::Fraction
        } else if code.contains('"') && SYMBOLS.iter().any(|(s, _)| code.contains(s)) {
            NumCat::Currency
        } else if code.chars().all(|c| "#,0.;-[]Red ".contains(c)) {
            NumCat::Number
        } else {
            NumCat::Custom
        };
        FormatDlg {
            tab,
            orig: st.clone(),
            thousands: code.contains(','),
            neg_red: code.contains("[Red]"),
            st,
            cat,
            decimals,
            symbol,
            code,
            sides: [None; 6],
            line: Line::THIN,
            multi,
        }
    }

    /// The whole format as edited (for a cell style's format).
    pub(super) fn result(&self) -> CellStyle {
        let mut s = self.st.clone();
        s.num_fmt = Arc::from(self.code.as_str());
        for (side, flag) in self.sides.iter().zip([apex_sheet::sheet::border::TOP, apex_sheet::sheet::border::BOTTOM, apex_sheet::sheet::border::LEFT, apex_sheet::sheet::border::RIGHT]) {
            if let Some(l) = side {
                s.borders.set(flag, *l);
            }
        }
        cellstyles_ui::link_theme(&mut s);
        s
    }

    /// Rebuilds the code from the category options.
    fn build_code(&mut self, loc: &apex_sheet::Locale) {
        let dec = if self.decimals > 0 { format!(".{}", "0".repeat(self.decimals)) } else { String::new() };
        let with_red = |pos: String| -> String { if self.neg_red { format!("{pos};[Red]-{pos}") } else { pos } };
        self.code = match self.cat {
            NumCat::General => "General".into(),
            NumCat::Number => with_red(format!("{}0{dec}", if self.thousands { "#,##" } else { "" })),
            NumCat::Currency => {
                let (s, after) = SYMBOLS[self.symbol];
                let after = if s == loc.currency { loc.currency_after } else { after };
                with_red(toolbar::currency_code(s, after, self.decimals))
            }
            NumCat::Accounting => {
                let (s, after) = SYMBOLS[self.symbol];
                let after = if s == loc.currency { loc.currency_after } else { after };
                apex_sheet::accounting_code(s, after, self.decimals)
            }
            NumCat::Percent => format!("0{dec}%"),
            NumCat::Scientific => format!("0{dec}E+00"),
            NumCat::Fraction => "# ?/?".into(),
            NumCat::Text => "@".into(),
            NumCat::Date | NumCat::Time | NumCat::Custom => return,
        };
    }
}

pub(super) struct FindState {
    pub text: String,
    pub replace: String,
    pub show_replace: bool,
    pub case: bool,
    pub whole: bool,
    pub formulas: bool,
    pub all_sheets: bool,
    pub results: Vec<(usize, CellRef, String)>,
    pub focus: bool,
}

impl FindState {
    pub fn new(replace: bool) -> FindState {
        FindState { text: String::new(), replace: String::new(), show_replace: replace, case: false, whole: false, formulas: true, all_sheets: false, results: Vec::new(), focus: true }
    }

    fn matches(&self, hay: &str) -> bool {
        if self.text.is_empty() {
            return false;
        }
        if self.case {
            if self.whole { hay == self.text } else { hay.contains(&self.text) }
        } else {
            let (h, n) = (hay.to_lowercase(), self.text.to_lowercase());
            if self.whole { h == n } else { h.contains(&n) }
        }
    }
}

/// Replaces every occurrence of `needle` (optionally ignoring case).
pub(super) fn replace_text(hay: &str, needle: &str, with: &str, case: bool) -> (String, usize) {
    if needle.is_empty() {
        return (hay.to_string(), 0);
    }
    if case {
        let n = hay.matches(needle).count();
        return (hay.replace(needle, with), n);
    }
    let lower = hay.to_lowercase();
    let nl = needle.to_lowercase();
    if hay.chars().any(|c| c.to_lowercase().map(char::len_utf8).sum::<usize>() != c.len_utf8()) {
        // Case folding changes some character's length (the positions
        // found in `lower` aren't those of `hay`): compare char by char.
        let hc: Vec<char> = hay.chars().collect();
        let nc: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
        let mut out = String::new();
        let mut i = 0;
        let mut count = 0;
        while i < hc.len() {
            let m = i + nc.len() <= hc.len() && hc[i..i + nc.len()].iter().zip(&nc).all(|(a, b)| a.to_lowercase().eq(std::iter::once(*b)));
            if m {
                out.push_str(with);
                i += nc.len();
                count += 1;
            } else {
                out.push(hc[i]);
                i += 1;
            }
        }
        return (out, count);
    }
    let mut out = String::new();
    let mut last = 0;
    let mut count = 0;
    for (i, _) in lower.match_indices(&nl) {
        out.push_str(&hay[last..i]);
        out.push_str(with);
        last = i + nl.len();
        count += 1;
    }
    out.push_str(&hay[last..]);
    (out, count)
}

impl SheetView {
    pub(super) fn open_format_dialog(&mut self, tab: usize) {
        self.settle_edit();
        let st = self.style_at_cursor();
        let loc = self.wb.locale;
        self.dialog = Some(Dialog::FormatCells(Box::new(FormatDlg::new(st, tab, &loc, !self.sel().is_single()))));
    }

    pub(super) fn open_size(&mut self, rows: bool) {
        let sh = self.sh();
        let px = if rows { sh.row_height(self.cursor.row) } else { sh.col_width(self.cursor.col) };
        self.dialog = Some(Dialog::Size { rows, value: (px * 0.75 * 10.0).round() / 10.0 });
    }

    pub(super) fn open_series(&mut self) {
        let s = self.sel();
        let opts = SeriesOpts { down: s.rows() >= s.cols(), kind: SeriesKind::Linear, step: 1.0, stop: None };
        self.dialog = Some(Dialog::Series { opts, step: "1".into(), stop: String::new() });
    }

    pub(super) fn open_sort(&mut self) {
        self.dialog = Some(Dialog::Sort { keys: vec![SortKey::new(self.cursor.col, true)], header: self.sort_region(false, false).1, case: false, by_cols: false, page: 0, sel: 0 });
    }

    pub(super) fn open_functions(&mut self) {
        self.dialog = Some(Dialog::Functions { query: String::new(), cat: 0, selected: Some(Pick::Func(Func::Sum)) });
    }

    pub(super) fn open_find(&mut self, replace: bool) {
        match &mut self.find {
            Some(f) => {
                f.show_replace |= replace;
                f.focus = true;
            }
            None => self.find = Some(FindState::new(replace)),
        }
    }

    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.dialog.take() else { return };
        let res = match &mut d {
            Dialog::FormatCells(f) => self.format_dialog(ctx, f),
            Dialog::PasteSpecial(o) => {
                let mut action = 0;
                let res = widgets::dialog(ctx, "paste_special", tr("paste.special"), 380.0, |ui| action = paste_special_ui(ui, o));
                if action != 0 {
                    self.focus_grid = true;
                    if action == 1 { self.paste_link() } else { self.paste_picture() }
                    return;
                }
                res
            }
            Dialog::Cells { delete, choice } => {
                let title = if *delete { tr("cells.delete_cells") } else { tr("cells.insert_cells") };
                let del = *delete;
                widgets::dialog(ctx, "cells", title, 260.0, |ui| {
                    let opts = if del { ["cells.shift_left", "cells.shift_up", "cells.entire_row", "cells.entire_col"] } else { ["cells.shift_right", "cells.shift_down", "cells.entire_row", "cells.entire_col"] };
                    for (i, key) in opts.iter().enumerate() {
                        ui.radio_value(choice, i as u8, tr(key));
                    }
                })
            }
            Dialog::Size { rows, value } => {
                let title = if *rows { tr("size.row_height") } else { tr("size.col_width") };
                widgets::dialog(ctx, "size", title, 240.0, |ui| {
                    egui::Grid::new("size_grid").show(ui, |ui| {
                        let r = ui.label(title);
                        let _ = r;
                        let resp = ui.add(egui::DragValue::new(value).range(0.0..=1000.0).speed(0.5).suffix(" pt").max_decimals(1));
                        if !ui.memory(|m| m.focused().is_some()) {
                            resp.request_focus();
                        }
                        ui.end_row();
                    });
                })
            }
            Dialog::Series { opts, step, stop } => widgets::dialog(ctx, "series", tr("fill.series"), 360.0, |ui| series_ui(ui, opts, step, stop)),
            Dialog::Sort { keys, header, case, by_cols, page, sel } => {
                let region = self.sort_region(*header, *by_cols).0;
                let lines = |r: Range| if *by_cols { r.start.row..=r.end.row } else { r.start.col..=r.end.col };
                let names: Vec<String> = match region {
                    Some(r) => lines(r)
                        .map(|k| {
                            let label = if *by_cols {
                                (*header && r.start.col > 0).then(|| CellRef::new(k, r.start.col - 1))
                            } else {
                                (*header && r.start.row > 0).then(|| CellRef::new(r.start.row - 1, k))
                            };
                            let h = label.map(|c| self.wb.display(self.sheet, c).0).unwrap_or_default();
                            match (h.is_empty(), *by_cols) {
                                (false, _) => h,
                                (true, false) => format!("{} {}", tr("sort.column"), apex_sheet::addr::col_name(k)),
                                (true, true) => format!("{} {}", tr("sort.row"), k + 1),
                            }
                        })
                        .collect(),
                    None => Vec::new(),
                };
                let first = region.map_or(0, |r| *lines(r).start());
                let marks: Vec<_> = keys.iter().map(|k| region.map(|r| self.wb.sort_marks(self.sheet, &r, k.col, *by_cols)).unwrap_or_default()).collect();
                let lists = apex_sheet::analysis::lists::all(&self.wb.analysis.custom_lists);
                let ctx_ = SortCtx { names: &names, first, marks: &marks, lists: &lists };
                let pages = [(Icon::SortAsc, "sort.levels"), (Icon::Options, "sort.options")];
                widgets::nav_dialog(ctx, "sort", tr("sort.custom"), &pages, page, vec2(620.0, 330.0), |ui, page| sort_ui(ui, page, keys, sel, header, case, by_cols, &ctx_))
            }
            Dialog::GoTo(text) => {
                let mut special = false;
                let res = widgets::dialog(ctx, "goto", tr("goto.title"), 280.0, |ui| {
                    ui.label(tr("goto.reference"));
                    let r = ui.add(egui::TextEdit::singleline(text).hint_text("A1, B2:D10, Sheet2!C5"));
                    if !r.has_focus() && !r.lost_focus() {
                        r.request_focus();
                    }
                    ui.add_space(6.0);
                    special = ui.button(tr("goto.special")).clicked();
                });
                if special {
                    self.dialog = Some(Dialog::Special(SpecialDlg::default()));
                    return;
                }
                res
            }
            Dialog::Calc(o) => widgets::dialog(ctx, "calc", tr("calc.title"), 380.0, |ui| calc_ui(ui, o)),
            Dialog::Special(o) => widgets::dialog(ctx, "goto_special", tr("goto.special_title"), 400.0, |ui| special_ui(ui, o)),
            Dialog::MergeWarn(_) => widgets::dialog(ctx, "merge_warn", tr("merge.title"), 340.0, |ui| {
                ui.label(tr("merge.warning"));
            }),
            Dialog::Functions { query, cat, selected } => {
                let udfs = self.wb.udf.as_ref().map(|u| u.names()).unwrap_or_default();
                widgets::dialog(ctx, "functions", tr("fx.title"), 520.0, |ui| functions_ui(ui, query, cat, selected, self.wb.locale.arg_sep, &udfs))
            }
        };
        match res {
            widgets::DialogResult::None => self.dialog = Some(d),
            widgets::DialogResult::Cancel => self.focus_grid = true,
            widgets::DialogResult::Ok => {
                self.focus_grid = true;
                self.dialog_ok(d);
            }
        }
    }

    fn dialog_ok(&mut self, d: Dialog) {
        let s = self.sel();
        match d {
            Dialog::FormatCells(f) => self.apply_format_dialog(*f),
            Dialog::PasteSpecial(o) => self.paste_special_now(o),
            Dialog::Cells { delete, choice } => match (delete, choice) {
                (false, 0) => self.wb.insert_cells(self.sheet, &s, false),
                (false, 1) => self.wb.insert_cells(self.sheet, &s, true),
                (true, 0) => self.wb.delete_cells(self.sheet, &s, false),
                (true, 1) => self.wb.delete_cells(self.sheet, &s, true),
                (false, 2) => self.insert_rows(false),
                (false, _) => self.insert_cols(false),
                (true, 2) => self.delete_rows(),
                (true, _) => self.delete_cols(),
            },
            Dialog::Size { rows, value } => {
                let px = value / 0.75;
                if rows {
                    let sizes = (s.start.row..=s.end.row.min(s.start.row + 100_000)).map(|r| (r, Some(px))).collect();
                    self.wb.execute(Op::RowHeights { sheet: self.sheet, sizes });
                } else {
                    let sizes = (s.start.col..=s.end.col).map(|c| (c, Some(px))).collect();
                    self.wb.execute(Op::ColWidths { sheet: self.sheet, sizes });
                }
            }
            Dialog::Series { mut opts, step, stop } => {
                opts.step = parse_num(&step).unwrap_or(1.0);
                opts.stop = parse_num(&stop);
                let r = if s.is_single() {
                    // A single cell with a stop value: fill as far as needed.
                    match opts.stop {
                        Some(_) if opts.down => Range::new(s.start, CellRef::new((s.start.row + 10_000).min(LAST.row), s.start.col)),
                        Some(_) => Range::new(s.start, CellRef::new(s.start.row, (s.start.col + 1000).min(LAST.col))),
                        None => s,
                    }
                } else {
                    s
                };
                self.wb.fill_series_opts(self.sheet, &r, opts);
            }
            Dialog::Sort { keys, header, case, by_cols, .. } => {
                if let (Some(r), _) = self.sort_region(header, by_cols) {
                    self.wb.sort_range(self.sheet, &r, &keys, case, by_cols);
                }
            }
            Dialog::GoTo(text) => self.goto(&text),
            Dialog::Special(o) => self.goto_special(o.kind()),
            Dialog::Calc(o) => self.set_calc(o),
            Dialog::MergeWarn(k) => self.merge_cmd(k, true),
            Dialog::Functions { selected, .. } => {
                match selected {
                    Some(Pick::Func(f)) => self.insert_function(formula_help::display_name(f)),
                    Some(Pick::Macro(name)) => self.insert_function(&name),
                    None => {}
                }
            }
        }
    }

    /// Changes the calculation options; back to automatic recalculates.
    pub(super) fn set_calc(&mut self, o: apex_sheet::CalcOptions) {
        if o == self.wb.calc {
            return;
        }
        self.settle_edit();
        // One undo step; back to automatic recalculates.
        self.wb.execute(apex_sheet::Op::Calc(o));
    }

    pub(super) fn goto_special(&mut self, kind: apex_sheet::Special) {
        let mut areas: Vec<Range> = self.areas().iter().flat_map(|r| self.wb.special_cells(self.sheet, r, kind, self.cursor)).collect();
        areas.dedup();
        if areas.is_empty() {
            self.notices.push(Notice::info(tr("goto.none")));
            return;
        }
        self.select_areas(&areas);
    }

    pub(super) fn goto(&mut self, text: &str) {
        let t = text.trim();
        if self.goto_table(t) {
            return;
        }
        if let Some((si, r)) = self.wb.name_target(self.sheet, t) {
            if si != self.sheet {
                self.select_sheet(si);
            }
            self.anchor = r.start;
            self.cursor = r.end;
            self.ensure_visible = true;
            return;
        }
        let (sheet, rest) = match t.rsplit_once('!') {
            Some((sh, r)) => (self.wb.sheet_index(sh.trim_matches('\'')), r),
            None => (Some(self.sheet), t),
        };
        let rest = rest.to_ascii_uppercase();
        let r = Range::parse(&rest).or_else(|| CellRef::parse(&rest).map(Range::single));
        match (sheet, r) {
            (Some(si), Some(r)) => {
                if si != self.sheet {
                    self.select_sheet(si);
                }
                self.anchor = r.start;
                self.cursor = r.end;
                self.ensure_visible = true;
            }
            _ => self.notices.push(Notice::error(tr("sheet.bad_reference"))),
        }
    }

    fn insert_function(&mut self, name: &str) {
        if self.edit.is_none() {
            let ctx = self.ctx.clone();
            let cur = self.wb.edit_text(self.sheet, self.cursor);
            let text = if cur.starts_with('=') { format!("{cur}{name}(") } else { format!("={name}(") };
            if let Some(ctx) = ctx {
                self.begin_edit(&ctx, EditOrigin::Bar, false, Some(text));
            }
        } else {
            if !self.buf.starts_with('=') {
                self.buf.insert(0, '=');
            }
            self.buf.push_str(name);
            self.buf.push('(');
            self.refocus = true;
        }
    }

    pub(super) fn format_dialog(&mut self, ctx: &egui::Context, f: &mut FormatDlg) -> widgets::DialogResult {
        let loc = self.wb.locale;
        let value = match self.sh().value(self.cursor) {
            Value::Number(n) => Some(*n),
            _ => None,
        };
        let text_value = self.wb.display(self.sheet, self.cursor).0;
        let pages = [
            (Icon::Currency, "fmtdlg.number"),
            (Icon::AlignLeft, "fmtdlg.alignment"),
            (Icon::GrowFont, "fmtdlg.font"),
            (Icon::Borders, "fmtdlg.border"),
            (Icon::Fill, "fmtdlg.fill"),
            (Icon::Protect, "fmtdlg.protection"),
        ];
        let mut tab = f.tab;
        let res = widgets::nav_dialog(ctx, "format_cells", tr("format.cells"), &pages, &mut tab, vec2(520.0, 390.0), |ui, page| match page {
            0 => number_tab(ui, f, &loc, value, &text_value),
            1 => alignment_tab(ui, f),
            2 => font_tab(ui, f),
            3 => border_tab(ui, f),
            4 => fill_tab(ui, f),
            _ => {
                widgets::section(ui, tr("fmtdlg.protection"));
                let mut locked = !f.st.unlocked;
                ui.checkbox(&mut locked, tr("protect.cell_locked"));
                f.st.unlocked = !locked;
                ui.checkbox(&mut f.st.formula_hidden, tr("protect.cell_hidden"));
                ui.add_space(10.0);
                ui.label(RichText::new(tr("protect.cell_hint")).weak());
            }
        });
        f.tab = tab;
        res
    }

    fn apply_format_dialog(&mut self, f: FormatDlg) {
        let (st, o) = (f.st.clone(), f.orig.clone());
        let code = f.code.clone();
        let code_changed = code != *o.num_fmt;
        // Only the fields changed in the dialog, so a mixed selection keeps
        // what the user did not touch.
        let changed = st != CellStyle { num_fmt: st.num_fmt.clone(), ..o.clone() } || code_changed;
        if changed {
            let code: Arc<str> = Arc::from(code.as_str());
            self.set_style(move |s| {
                macro_rules! upd {
                    ($($f:ident),*) => { $(if st.$f != o.$f { s.$f = st.$f.clone(); })* };
                }
                upd!(bold, italic, underline, strike, font, font_size, color, fill, halign, valign, wrap, indent, rotation, unlocked, formula_hidden);
                if st.theme.font != o.theme.font {
                    s.theme.font = st.theme.font;
                }
                cellstyles_ui::link_theme(s);
                if code_changed {
                    s.num_fmt = code.clone();
                }
            });
        }
        let presets = [BorderPreset::Top, BorderPreset::Bottom, BorderPreset::Left, BorderPreset::Right, BorderPreset::InsideH, BorderPreset::InsideV];
        for (side, p) in f.sides.iter().zip(presets) {
            if let Some(l) = side {
                let r = self.sel();
                self.wb.apply_borders(self.sheet, &r, p, *l);
            }
        }
    }

    // --- find & replace bar ---------------------------------------------------------------

    pub(super) fn find_bar(&mut self, ui: &mut egui::Ui) {
        let Some(mut f) = self.find.take() else { return };
        let mut close = false;
        let mut action = 0u8;
        ui.horizontal(|ui| {
            let id = Id::new(("sfind", self.uid));
            let r = ui.add(egui::TextEdit::singleline(&mut f.text).id(id).hint_text(tr("find.hint")).desired_width(170.0));
            if std::mem::take(&mut f.focus) {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                action = 1;
                r.request_focus();
            }
            if r.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                close = true;
            }
            if f.show_replace {
                ui.add(egui::TextEdit::singleline(&mut f.replace).hint_text(tr("find.replace_hint")).desired_width(150.0));
            }
            ui.toggle_value(&mut f.case, "Aa").on_hover_text(tr("find.case"));
            ui.toggle_value(&mut f.whole, tr("find.whole_cell")).on_hover_text(tr("find.whole_cell_hint"));
            egui::ComboBox::from_id_salt(("look_in", self.uid)).width(90.0).selected_text(if f.formulas { tr("find.in_formulas") } else { tr("find.in_values") }).show_ui(ui, |ui| {
                ui.selectable_value(&mut f.formulas, true, tr("find.in_formulas"));
                ui.selectable_value(&mut f.formulas, false, tr("find.in_values"));
            });
            egui::ComboBox::from_id_salt(("scope", self.uid)).width(90.0).selected_text(if f.all_sheets { tr("find.workbook") } else { tr("find.this_sheet") }).show_ui(ui, |ui| {
                ui.selectable_value(&mut f.all_sheets, false, tr("find.this_sheet"));
                ui.selectable_value(&mut f.all_sheets, true, tr("find.workbook"));
            });
            if ui.button(tr("find.next")).clicked() {
                action = 1;
            }
            if ui.button(tr("find.all")).clicked() {
                action = 2;
            }
            if f.show_replace {
                if ui.button(tr("find.replace")).clicked() {
                    action = 3;
                }
                if ui.button(tr("find.replace_all")).clicked() {
                    action = 4;
                }
            } else if ui.button(tr("find.show_replace")).clicked() {
                f.show_replace = true;
            }
            if ui.button("×").on_hover_text(tr("tab.close")).clicked() {
                close = true;
            }
        });
        match action {
            1 => self.find_next(&f),
            2 => {
                f.results = self.find_all(&f);
                if f.results.is_empty() {
                    self.notices.push(Notice::info(tr("find.not_found")));
                }
            }
            3 => self.replace_one(&f),
            4 => self.replace_all(&f),
            _ => {}
        }
        if !f.results.is_empty() {
            let mut goto = None;
            ui.label(RichText::new(trf("find.found_n", &[("n", &f.results.len())])).weak());
            egui::ScrollArea::vertical().max_height(110.0).id_salt(("find_results", self.uid)).show(ui, |ui| {
                egui::Grid::new(("find_grid", self.uid)).striped(true).num_columns(3).show(ui, |ui| {
                    for (i, (s, c, text)) in f.results.iter().enumerate().take(1000) {
                        if ui.link(format!("{}!{}", self.wb.sheets[*s].name, c.to_a1())).clicked() {
                            goto = Some(i);
                        }
                        ui.label(RichText::new(text.chars().take(80).collect::<String>()).weak());
                        ui.end_row();
                    }
                });
            });
            if let Some(i) = goto {
                let (s, c, _) = f.results[i].clone();
                if s != self.sheet {
                    self.select_sheet(s);
                }
                self.select(c, false);
            }
        }
        if close {
            self.focus_grid = true;
        } else {
            self.find = Some(f);
        }
    }

    fn search_text(&self, sheet: usize, c: CellRef, formulas: bool) -> String {
        if formulas { self.wb.edit_text(sheet, c) } else { self.wb.display(sheet, c).0 }
    }

    pub(super) fn find_all(&self, f: &FindState) -> Vec<(usize, CellRef, String)> {
        let sheets: Vec<usize> = if f.all_sheets { (0..self.wb.sheets.len()).collect() } else { vec![self.sheet] };
        let mut out = Vec::new();
        for s in sheets {
            let mut cells: Vec<CellRef> = self.wb.sheets[s].iter().filter(|(_, c)| !matches!(c.content, Content::Empty)).map(|(c, _)| c).collect();
            cells.sort();
            for c in cells {
                let t = self.search_text(s, c, f.formulas);
                if f.matches(&t) {
                    out.push((s, c, t));
                }
            }
        }
        out
    }

    /// The first match of sheet `s` after `after` (in reading order), or
    /// from the start. Each column stops at its first match, and at the
    /// best one found so far: a search on a million rows does not read
    /// them all when a match is near.
    fn find_after(&self, f: &FindState, s: usize, after: Option<CellRef>) -> Option<CellRef> {
        let sh = &self.wb.sheets[s];
        let mut best: Option<CellRef> = None;
        for col in 0..sh.extent().map_or(0, |e| e.col + 1) {
            let lo = match after {
                Some(a) if col > a.col => a.row,
                Some(a) => a.row + 1,
                None => 0,
            };
            let hi = match best {
                Some(b) if col < b.col => b.row,
                Some(b) if b.row == 0 => continue,
                Some(b) => b.row - 1,
                None => u32::MAX,
            };
            if lo > hi {
                continue;
            }
            for (row, cell) in sh.column_cells(col, lo, hi) {
                let c = CellRef::new(row, col);
                if !matches!(cell.content, Content::Empty) && f.matches(&self.search_text(s, c, f.formulas)) {
                    best = Some(c);
                    break;
                }
            }
        }
        best
    }

    pub(super) fn find_next(&mut self, f: &FindState) {
        // After the cursor, then the next sheets, then around from the top.
        let n = self.wb.sheets.len();
        let order: Vec<(usize, Option<CellRef>)> = if f.all_sheets {
            std::iter::once((self.sheet, Some(self.cursor))).chain((1..=n).map(|i| ((self.sheet + i) % n, None))).collect()
        } else {
            vec![(self.sheet, Some(self.cursor)), (self.sheet, None)]
        };
        let next = order.into_iter().find_map(|(s, after)| self.find_after(f, s, after).map(|c| (s, c)));
        match next {
            Some((s, c)) => {
                if s != self.sheet {
                    self.select_sheet(s);
                }
                self.select(c, false);
            }
            None => self.notices.push(Notice::info(tr("find.not_found"))),
        }
    }

    fn replace_one(&mut self, f: &FindState) {
        let t = self.wb.edit_text(self.sheet, self.cursor);
        if f.matches(&self.search_text(self.sheet, self.cursor, f.formulas)) {
            let new = if f.whole { f.replace.clone() } else { replace_text(&t, &f.text, &f.replace, f.case).0 };
            self.wb.set_input(self.sheet, self.cursor, &new);
        }
        self.find_next(f);
    }

    fn replace_all(&mut self, f: &FindState) {
        let mut total = 0;
        let hits = self.find_all(f);
        let mut by_sheet: std::collections::BTreeMap<usize, Vec<(CellRef, String)>> = Default::default();
        for (s, c, _) in hits {
            let t = self.wb.edit_text(s, c);
            let (new, n) = if f.whole { (f.replace.clone(), 1) } else { replace_text(&t, &f.text, &f.replace, f.case) };
            if n > 0 && new != t {
                total += n;
                by_sheet.entry(s).or_default().push((c, new));
            }
        }
        for (s, inputs) in by_sheet {
            self.wb.set_inputs(s, inputs);
        }
        self.notices.push(Notice::info(trf("find.replaced", &[("n", &total)])));
    }
}

fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() { None } else { t.parse().ok() }
}

/// The Paste Special options; returns 1 for Paste Link, 2 for Picture.
fn paste_special_ui(ui: &mut egui::Ui, o: &mut PasteSpecial) -> u8 {
    ui.columns(2, |cols| {
        cols[0].label(RichText::new(tr("paste.what")).strong());
        for (w, key) in [
            (PasteWhat::All, "paste.all"),
            (PasteWhat::Values, "paste.values"),
            (PasteWhat::Formulas, "paste.formulas"),
            (PasteWhat::Formats, "paste.formats"),
            (PasteWhat::AllExceptBorders, "paste.no_borders"),
            (PasteWhat::Comments, "paste.comments"),
            (PasteWhat::Validation, "paste.validation"),
            (PasteWhat::ColumnWidths, "paste.col_widths"),
        ] {
            cols[0].radio_value(&mut o.what, w, tr(key));
        }
        cols[1].label(RichText::new(tr("paste.operation")).strong());
        for (op, key) in [(PasteOp::None, "paste.op_none"), (PasteOp::Add, "paste.op_add"), (PasteOp::Sub, "paste.op_sub"), (PasteOp::Mul, "paste.op_mul"), (PasteOp::Div, "paste.op_div")] {
            cols[1].radio_value(&mut o.op, op, tr(key));
        }
    });
    ui.add_space(6.0);
    ui.checkbox(&mut o.skip_blanks, tr("paste.skip_blanks"));
    ui.checkbox(&mut o.transpose, tr("paste.transpose"));
    ui.add_space(6.0);
    let mut action = 0;
    ui.horizontal(|ui| {
        if ui.button(tr("paste.link")).clicked() {
            action = 1;
        }
        if ui.button(tr("paste.picture")).clicked() {
            action = 2;
        }
    });
    action
}

fn series_ui(ui: &mut egui::Ui, o: &mut SeriesOpts, step: &mut String, stop: &mut String) {
    ui.columns(2, |cols| {
        cols[0].label(RichText::new(tr("series.direction")).strong());
        cols[0].radio_value(&mut o.down, false, tr("series.rows"));
        cols[0].radio_value(&mut o.down, true, tr("series.cols"));
        cols[1].label(RichText::new(tr("series.type")).strong());
        let is_date = matches!(o.kind, SeriesKind::Days | SeriesKind::Weekdays | SeriesKind::Months | SeriesKind::Years);
        if cols[1].radio(o.kind == SeriesKind::Linear, tr("series.linear")).clicked() {
            o.kind = SeriesKind::Linear;
        }
        if cols[1].radio(o.kind == SeriesKind::Growth, tr("series.growth")).clicked() {
            o.kind = SeriesKind::Growth;
        }
        if cols[1].radio(is_date, tr("series.date")).clicked() && !is_date {
            o.kind = SeriesKind::Days;
        }
        cols[1].add_enabled_ui(is_date, |ui| {
            ui.indent("units", |ui| {
                for (k, key) in [(SeriesKind::Days, "series.day"), (SeriesKind::Weekdays, "series.weekday"), (SeriesKind::Months, "series.month"), (SeriesKind::Years, "series.year")] {
                    ui.radio_value(&mut o.kind, k, tr(key));
                }
            });
        });
    });
    ui.add_space(6.0);
    egui::Grid::new("series_vals").show(ui, |ui| {
        ui.label(tr("series.step"));
        ui.add(egui::TextEdit::singleline(step).desired_width(90.0));
        ui.end_row();
        ui.label(tr("series.stop"));
        ui.add(egui::TextEdit::singleline(stop).desired_width(90.0));
        ui.end_row();
    });
}

struct SortCtx<'a> {
    names: &'a [String],
    first: u32,
    /// Fills, font colors and icons of each level's column.
    marks: &'a [(Vec<Option<u32>>, Vec<Option<u32>>, Vec<u8>)],
    lists: &'a [Vec<String>],
}

fn color_text(c: Option<u32>) -> egui::RichText {
    match c {
        Some(c) => egui::RichText::new(format!("■■■  #{c:06X}")).color(Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)),
        None => egui::RichText::new(tr("sort.no_color")),
    }
}

fn list_text(l: &[String]) -> String {
    let t = l.join(", ");
    if t.chars().count() > 40 { format!("{}…", t.chars().take(40).collect::<String>()) } else { t }
}

#[allow(clippy::too_many_arguments)]
fn sort_ui(ui: &mut egui::Ui, page: usize, keys: &mut Vec<SortKey>, sel: &mut usize, header: &mut bool, case: &mut bool, by_cols: &mut bool, cx: &SortCtx) {
    use apex_sheet::SortOn;
    if page == 1 {
        widgets::section(ui, tr("sort.options"));
        ui.checkbox(case, tr("find.case"));
        widgets::section(ui, tr("sort.orientation"));
        let before = *by_cols;
        ui.radio_value(by_cols, false, tr("sort.top_bottom"));
        ui.radio_value(by_cols, true, tr("sort.left_right"));
        if *by_cols != before {
            // Levels name the other kind of line now.
            keys.truncate(1);
            keys[0] = SortKey::new(cx.first, true);
            *sel = 0;
        }
        return;
    }
    widgets::section(ui, tr("sort.data"));
    ui.checkbox(header, if *by_cols { tr("sort.header_row") } else { tr("sort.header") });
    widgets::section(ui, tr("sort.levels"));
    *sel = (*sel).min(keys.len() - 1);
    ui.horizontal(|ui| {
        if ui.add_enabled(keys.len() < 64, egui::Button::new(format!("+ {}", tr("sort.add_level")))).clicked() {
            keys.insert(*sel + 1, SortKey::new(cx.first, true));
            *sel += 1;
        }
        if ui.add_enabled(keys.len() > 1, egui::Button::new(tr("sort.delete_level"))).clicked() {
            keys.remove(*sel);
            *sel = (*sel).min(keys.len() - 1);
        }
        if ui.add_enabled(keys.len() < 64, egui::Button::new(tr("sort.copy_level"))).clicked() {
            let k = keys[*sel].clone();
            keys.insert(*sel + 1, k);
            *sel += 1;
        }
        if ui.add_enabled_ui(*sel > 0, |ui| widgets::tool_button(ui, widgets::Face::Icon(Icon::ChevronUp), false, tr("sort.move_up"))).inner.clicked() {
            keys.swap(*sel, *sel - 1);
            *sel -= 1;
        }
        if ui.add_enabled_ui(*sel + 1 < keys.len(), |ui| widgets::tool_button(ui, widgets::Face::Icon(Icon::ChevronDown), false, tr("sort.move_down"))).inner.clicked() {
            keys.swap(*sel, *sel + 1);
            *sel += 1;
        }
    });
    ui.add_space(6.0);
    egui::Grid::new("sort_keys").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
        let head = |ui: &mut egui::Ui, t: &str| ui.label(egui::RichText::new(t).small().weak());
        head(ui, "");
        head(ui, if *by_cols { tr("sort.row") } else { tr("sort.column") });
        head(ui, tr("sort.sort_on"));
        head(ui, tr("sort.order"));
        ui.end_row();
        for (i, k) in keys.iter_mut().enumerate() {
            if ui.selectable_label(*sel == i, if i == 0 { tr("sort.by") } else { tr("sort.then_by") }).clicked() {
                *sel = i;
            }
            let default = if *by_cols { format!("{} {}", tr("sort.row"), k.col + 1) } else { apex_sheet::addr::col_name(k.col) };
            let name = cx.names.get(k.col.saturating_sub(cx.first) as usize).cloned().unwrap_or(default);
            egui::ComboBox::from_id_salt(("sort_col", i)).width(140.0).selected_text(name).show_ui(ui, |ui| {
                for (j, n) in cx.names.iter().enumerate() {
                    ui.selectable_value(&mut k.col, cx.first + j as u32, n);
                }
            });
            let (fills, fonts, icons) = cx.marks.get(i).cloned().unwrap_or_default();
            let on_name = |on: &SortOn| match on {
                SortOn::Values => tr("sort.on_values"),
                SortOn::CellColor(_) => tr("sort.on_fill"),
                SortOn::FontColor(_) => tr("sort.on_font"),
                SortOn::Icon(_) => tr("sort.on_icon"),
            };
            egui::ComboBox::from_id_salt(("sort_on", i)).width(120.0).selected_text(on_name(&k.on)).show_ui(ui, |ui| {
                let opts = [
                    SortOn::Values,
                    // The first real color is the likely pick.
                    SortOn::CellColor(fills.iter().copied().flatten().next()),
                    SortOn::FontColor(fonts.iter().copied().flatten().next()),
                    SortOn::Icon(icons.first().copied().unwrap_or(0)),
                ];
                for o in opts {
                    if ui.selectable_label(std::mem::discriminant(&k.on) == std::mem::discriminant(&o), on_name(&o)).clicked() {
                        k.on = o;
                        k.list = None;
                    }
                }
            });
            let is_fill = matches!(k.on, SortOn::CellColor(_));
            match &mut k.on {
                SortOn::Values => {
                    let text = match &k.list {
                        Some(l) => list_text(l),
                        None if k.ascending => tr("sort.ascending").to_string(),
                        None => tr("sort.descending").to_string(),
                    };
                    egui::ComboBox::from_id_salt(("sort_dir", i)).width(200.0).selected_text(text).show_ui(ui, |ui| {
                        if ui.selectable_label(k.list.is_none() && k.ascending, tr("sort.ascending")).clicked() {
                            (k.list, k.ascending) = (None, true);
                        }
                        if ui.selectable_label(k.list.is_none() && !k.ascending, tr("sort.descending")).clicked() {
                            (k.list, k.ascending) = (None, false);
                        }
                        ui.separator();
                        ui.label(egui::RichText::new(tr("sort.list")).weak());
                        for l in cx.lists {
                            if ui.selectable_label(k.list.as_ref() == Some(l), list_text(l)).clicked() {
                                (k.list, k.ascending) = (Some(l.clone()), true);
                            }
                        }
                    });
                }
                SortOn::CellColor(c) | SortOn::FontColor(c) => {
                    let all = if is_fill { &fills } else { &fonts };
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt(("sort_color", i)).width(110.0).selected_text(color_text(*c)).show_ui(ui, |ui| {
                            for x in all {
                                ui.selectable_value(c, *x, color_text(*x));
                            }
                        });
                        place_combo(ui, i, &mut k.ascending, *by_cols);
                    });
                }
                SortOn::Icon(n) => {
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt(("sort_icon", i)).width(110.0).selected_text(format!("{} {}", tr("sort.icon"), *n + 1)).show_ui(ui, |ui| {
                            for x in &icons {
                                ui.selectable_value(n, *x, format!("{} {}", tr("sort.icon"), x + 1));
                            }
                        });
                        place_combo(ui, i, &mut k.ascending, *by_cols);
                    });
                }
            }
            ui.end_row();
        }
    });
}

/// On top / on bottom (or left / right) for color and icon levels.
fn place_combo(ui: &mut egui::Ui, i: usize, top: &mut bool, by_cols: bool) {
    let (a, b) = if by_cols { (tr("sort.on_left"), tr("sort.on_right")) } else { (tr("sort.on_top"), tr("sort.on_bottom")) };
    egui::ComboBox::from_id_salt(("sort_place", i)).width(140.0).selected_text(if *top { a } else { b }).show_ui(ui, |ui| {
        ui.selectable_value(top, true, a);
        ui.selectable_value(top, false, b);
    });
}

/// A function of the Insert Function dialog.
#[derive(Clone, PartialEq)]
pub(super) enum Pick {
    Func(Func),
    /// Of the workbook's macros.
    Macro(String),
}

fn functions_ui(ui: &mut egui::Ui, query: &mut String, cat: &mut usize, selected: &mut Option<Pick>, sep: char, udfs: &[(String, Vec<String>)]) {
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(query).hint_text(tr("fx.search")).desired_width(200.0));
        if !r.has_focus() && query.is_empty() && ui.memory(|m| m.focused().is_none()) {
            r.request_focus();
        }
        let user = (!udfs.is_empty()).then_some("fncat.user");
        let cats: Vec<&str> = std::iter::once("fncat.all").chain(formula_help::CATEGORIES).chain(user).collect();
        *cat = (*cat).min(cats.len() - 1);
        let r = egui::ComboBox::from_id_salt("fx_cat").width(170.0).selected_text(tr_dyn(cats[*cat])).show_ui(ui, |ui| {
            for (i, c) in cats.iter().enumerate() {
                ui.selectable_value(cat, i, tr_dyn(c));
            }
        });
        // Named for screen readers (it shows only the category chosen).
        widgets::name(&r.response, egui::WidgetType::ComboBox, tr("fmtdlg.category"), None);
    });
    let q = query.to_lowercase();
    let user_cat = formula_help::CATEGORIES.len() + 1;
    let mut list: Vec<(String, Pick)> = Func::ALL
        .iter()
        .copied()
        .filter(|&f| *cat == 0 || formula_help::CATEGORIES.get(*cat - 1).is_some_and(|c| formula_help::category(f) == *c))
        .filter(|&f| {
            q.is_empty() || formula_help::display_name(f).to_lowercase().contains(&q) || f.name().to_lowercase().contains(&q) || formula_help::help(f, sep).1.to_lowercase().contains(&q)
        })
        .map(|f| (formula_help::display_name(f).to_string(), Pick::Func(f)))
        .collect();
    if *cat == 0 || *cat == user_cat {
        list.extend(udfs.iter().filter(|(n, _)| q.is_empty() || n.to_lowercase().contains(&q)).map(|(n, _)| (n.clone(), Pick::Macro(n.clone()))));
    }
    list.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    if selected.as_ref().is_none_or(|s| !list.iter().any(|(_, p)| p == s)) {
        *selected = list.first().map(|(_, p)| p.clone());
    }
    ui.add_space(4.0);
    ui.horizontal_top(|ui| {
        egui::ScrollArea::vertical().id_salt("fx_list").max_height(240.0).min_scrolled_height(240.0).show(ui, |ui| {
            ui.set_width(170.0);
            // One under the other (the row around would lay them side by side).
            ui.vertical(|ui| {
                for (name, p) in &list {
                    if ui.selectable_label(selected.as_ref() == Some(p), name).clicked() {
                        *selected = Some(p.clone());
                    }
                }
            });
        });
        // As tall as the list: one filling the height would grow the
        // dialog a little every frame.
        ui.add_sized([6.0, 240.0], egui::Separator::default().vertical());
        ui.vertical(|ui| {
            match selected {
                Some(Pick::Func(f)) => {
                    let (args, desc) = formula_help::help(*f, sep);
                    ui.label(RichText::new(format!("{}({args})", formula_help::display_name(*f))).monospace().strong());
                    ui.add_space(6.0);
                    ui.label(desc);
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr_dyn(formula_help::category(*f))).weak());
                }
                Some(Pick::Macro(name)) => {
                    let params = udfs.iter().find(|(n, _)| n == name).map(|(_, p)| p.join(&format!("{sep} "))).unwrap_or_default();
                    ui.label(RichText::new(format!("{name}({params})")).monospace().strong());
                    ui.add_space(6.0);
                    ui.label(tr("fx.user_defined"));
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr("fncat.user")).weak());
                }
                None => {}
            }
        });
    });
}

/// `tr` for keys held in tables (always valid keys).
fn tr_dyn(key: &str) -> &'static str {
    crate::i18n::lookup(key).unwrap_or("")
}

fn number_tab(ui: &mut egui::Ui, f: &mut FormatDlg, loc: &apex_sheet::Locale, value: Option<f64>, text_value: &str) {
    // A sample of the value first, as it will look.
    widgets::section(ui, tr("fmtdlg.preview"));
    let preview = match value {
        Some(n) => apex_sheet::NumFormat::parse(&f.code).format_number(n, loc),
        None => (text_value.to_string(), None),
    };
    let color = preview.1.map_or(Paper::TEXT, theme::rgb);
    egui::Frame::new().fill(Paper::BG).inner_margin(8).corner_radius(3).show(ui, |ui| {
        ui.set_width(ui.available_width() - 16.0);
        ui.label(RichText::new(if preview.0.is_empty() { " ".into() } else { preview.0 }).color(color).size(15.0));
    });
    ui.add_space(4.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(150.0);
            widgets::section(ui, tr("fmtdlg.category"));
            ui.spacing_mut().item_spacing.y = 1.0;
            for (c, key) in CATS {
                if ui.add(egui::Button::selectable(f.cat == c, tr_dyn(key)).min_size(vec2(150.0, 22.0))).clicked() && f.cat != c {
                    f.cat = c;
                    match c {
                        NumCat::Date => f.code = loc.short_date_code().to_string(),
                        NumCat::Time => f.code = "hh:mm:ss".into(),
                        _ => f.build_code(loc),
                    }
                }
            }
        });
        ui.add_space(16.0);
        ui.vertical(|ui| {
            widgets::section(ui, tr("fmtdlg.options"));
            let before = (f.decimals, f.thousands, f.neg_red, f.symbol);
            match f.cat {
                NumCat::Number | NumCat::Currency | NumCat::Accounting | NumCat::Percent | NumCat::Scientific => {
                    egui::Grid::new("num_opts").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label(tr("fmtdlg.decimals"));
                        ui.add(egui::DragValue::new(&mut f.decimals).range(0..=10));
                        ui.end_row();
                        if matches!(f.cat, NumCat::Currency | NumCat::Accounting) {
                            ui.label(tr("fmtdlg.symbol"));
                            egui::ComboBox::from_id_salt("num_symbol").width(120.0).selected_text(SYMBOLS[f.symbol].0).show_ui(ui, |ui| {
                                for (i, (s, _)) in SYMBOLS.iter().enumerate() {
                                    ui.selectable_value(&mut f.symbol, i, *s);
                                }
                            });
                            ui.end_row();
                        }
                    });
                    if f.cat == NumCat::Number {
                        ui.checkbox(&mut f.thousands, tr("fmtdlg.thousands"));
                    }
                    if matches!(f.cat, NumCat::Number | NumCat::Currency) {
                        ui.checkbox(&mut f.neg_red, tr("fmtdlg.neg_red"));
                    }
                }
                NumCat::Date | NumCat::Time => {
                    let codes: Vec<String> = if f.cat == NumCat::Date {
                        vec![loc.short_date_code().into(), "d mmmm yyyy".into(), "dddd, d mmmm yyyy".into(), "mmmm yyyy".into(), "d-mmm-yy".into(), "yyyy-mm-dd".into(), format!("{} hh:mm", loc.short_date_code())]
                    } else {
                        vec!["hh:mm".into(), "hh:mm:ss".into(), "mm:ss".into()]
                    };
                    let v = value.unwrap_or(45000.5);
                    ui.label(tr("fmtdlg.type"));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for c in codes {
                        let sample = apex_sheet::NumFormat::parse(&c).format_number(v, loc).0;
                        if ui.add(egui::Button::selectable(f.code == c, sample).min_size(vec2(240.0, 22.0))).clicked() {
                            f.code = c;
                        }
                    }
                }
                NumCat::General => {
                    ui.label(RichText::new(tr("fmtdlg.general_hint")).weak());
                }
                _ => {}
            }
            if before != (f.decimals, f.thousands, f.neg_red, f.symbol) {
                f.build_code(loc);
            }
            ui.add_space(10.0);
            ui.label(tr("fmtdlg.code"));
            if ui.add(egui::TextEdit::singleline(&mut f.code).desired_width(ui.available_width()).font(egui::TextStyle::Monospace)).changed() {
                f.cat = NumCat::Custom;
            }
        });
    });
}

fn alignment_tab(ui: &mut egui::Ui, f: &mut FormatDlg) {
    let st = &mut f.st;
    widgets::section(ui, tr("fmtdlg.text_alignment"));
    widgets::form(ui, "align_grid", |ui| {
        ui.label(tr("fmtdlg.horizontal"));
        let hname = |h: HAlign| match h {
            HAlign::General => tr("fmtdlg.h_general"),
            HAlign::Left => tr("format.align_left"),
            HAlign::Center => tr("format.align_center"),
            HAlign::Right => tr("format.align_right"),
            HAlign::Justify => tr("format.align_justify"),
        };
        egui::ComboBox::from_id_salt("halign").width(180.0).selected_text(hname(st.halign)).show_ui(ui, |ui| {
            for h in [HAlign::General, HAlign::Left, HAlign::Center, HAlign::Right, HAlign::Justify] {
                ui.selectable_value(&mut st.halign, h, hname(h));
            }
        });
        ui.end_row();
        ui.label(tr("fmtdlg.indent"));
        ui.add(egui::DragValue::new(&mut st.indent).range(0..=15));
        ui.end_row();
        ui.label(tr("fmtdlg.vertical"));
        let vname = |v: VAlign| match v {
            VAlign::Top => tr("format.valign_top"),
            VAlign::Center => tr("format.valign_middle"),
            VAlign::Bottom => tr("format.valign_bottom"),
        };
        egui::ComboBox::from_id_salt("valign").width(180.0).selected_text(vname(st.valign)).show_ui(ui, |ui| {
            for v in [VAlign::Top, VAlign::Center, VAlign::Bottom] {
                ui.selectable_value(&mut st.valign, v, vname(v));
            }
        });
        ui.end_row();
    });
    widgets::section(ui, tr("fmtdlg.orientation"));
    widgets::form(ui, "orient_grid", |ui| {
        ui.label(tr("rotate.title"));
        ui.horizontal(|ui| {
            let mut stacked = st.rotation == ROTATION_STACKED;
            ui.add_enabled_ui(!stacked, |ui| {
                let mut deg = if stacked { 0 } else { st.rotation };
                if ui.add(egui::DragValue::new(&mut deg).range(-90..=90).suffix("°")).changed() {
                    st.rotation = deg;
                }
            });
            if ui.checkbox(&mut stacked, tr("rotate.stacked")).changed() {
                st.rotation = if stacked { ROTATION_STACKED } else { 0 };
            }
        });
        ui.end_row();
    });
    widgets::section(ui, tr("fmtdlg.text_control"));
    ui.checkbox(&mut st.wrap, tr("sheet.wrap"));
}

fn font_tab(ui: &mut egui::Ui, f: &mut FormatDlg) {
    let st = &mut f.st;
    widgets::section(ui, tr("fmtdlg.font"));
    widgets::form(ui, "font_grid", |ui| {
        ui.label(tr("toolbar.font"));
        let cur = st.font.as_deref().unwrap_or(default_font()).to_string();
        if let Some(n) = common::themed_font_picker(ui, Id::new("dlg_font"), &cur, 200.0) {
            cellstyles_ui::pick_font(st, &n);
        }
        ui.end_row();
        ui.label(tr("toolbar.font_size"));
        let size = style_pt(st);
        if let Some(v) = common::size_box(ui, Id::new("dlg_size"), size) {
            st.font_size = if (v - DEFAULT_PT).abs() < 0.01 { 0 } else { (v * 10.0).round() as u16 };
        }
        ui.end_row();
        ui.label(tr("fmtdlg.style"));
        ui.horizontal(|ui| {
            ui.checkbox(&mut st.bold, tr("format.bold"));
            ui.checkbox(&mut st.italic, tr("format.italic"));
        });
        ui.end_row();
        ui.label(tr("fmtdlg.effects"));
        ui.horizontal(|ui| {
            ui.checkbox(&mut st.underline, tr("format.underline"));
            ui.checkbox(&mut st.strike, tr("format.strike"));
        });
        ui.end_row();
        ui.label(tr("format.text_color"));
        let r = ui.button(st.color.map_or(tr("format.auto_color").to_string(), |c| format!("■ #{c:06X}")));
        egui::Popup::menu(&r).show(|ui| {
            if let Some(c) = widgets::color_menu(ui, tr("format.auto_color"), st.color) {
                st.color = c;
            }
        });
        ui.end_row();
    });
    widgets::section(ui, tr("fmtdlg.preview"));
    let px = style_pt(st) * PT;
    let (font, synth) = fonts::font_for(st.font.as_deref(), st.bold, st.italic, px.min(40.0));
    let fmt = TextFormat { font_id: font, color: st.color.map_or(Paper::TEXT, theme::rgb), italics: synth, ..Default::default() };
    egui::Frame::new().fill(Paper::BG).inner_margin(8).corner_radius(3).show(ui, |ui| {
        ui.set_width(ui.available_width() - 16.0);
        ui.set_min_height(56.0);
        ui.label(LayoutJob::single_section("AaBbCcYyZz 123".into(), fmt));
    });
}

fn fill_tab(ui: &mut egui::Ui, f: &mut FormatDlg) {
    widgets::section(ui, tr("format.fill_color"));
    let mut c = f.st.fill;
    ui.push_id("fill_colors", |ui| {
        if let Some(n) = widgets::color_menu(ui, tr("format.no_fill"), c) {
            c = n;
        }
    });
    f.st.fill = c;
    widgets::section(ui, tr("fmtdlg.preview"));
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width() - 16.0, 48.0), Sense::hover());
    ui.painter().rect_filled(rect, 3.0, c.map_or(Paper::BG, theme::rgb));
    ui.painter().rect_stroke(rect, 3.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, c.map_or(tr("format.no_fill").to_string(), |c| format!("#{c:06X}")), FontId::proportional(13.0), Paper::TEXT);
}

fn border_tab(ui: &mut egui::Ui, f: &mut FormatDlg) {
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(150.0);
            widgets::section(ui, tr("border.line_style"));
            for s in [LineStyle::Thin, LineStyle::Medium, LineStyle::Thick, LineStyle::Dashed, LineStyle::Dotted, LineStyle::Double, LineStyle::Hair] {
                let (rect, resp) = ui.allocate_exact_size(vec2(130.0, 18.0), Sense::click());
                if f.line.style == s {
                    ui.painter().rect_filled(rect, 3.0, ui.visuals().selection.bg_fill.gamma_multiply(0.3));
                }
                let y = rect.center().y;
                paint_line(ui.painter(), pos2(rect.min.x + 6.0, y), pos2(rect.max.x - 6.0, y), Line { style: s, color: None }, ui.visuals().text_color(), true);
                if resp.clicked() {
                    f.line.style = s;
                }
            }
            widgets::section(ui, tr("border.line_color"));
            let r = ui.button(f.line.color.map_or(tr("format.auto_color").to_string(), |c| format!("■ #{c:06X}")));
            egui::Popup::menu(&r).show(|ui| {
                if let Some(c) = widgets::color_menu(ui, tr("format.auto_color"), f.line.color) {
                    f.line.color = c;
                }
            });
        });
        ui.add_space(16.0);
        ui.vertical(|ui| {
            widgets::section(ui, tr("border.presets"));
            ui.horizontal(|ui| {
                if ui.button(tr("border.none")).clicked() {
                    f.sides = [Some(Line::default()); 6];
                }
                if ui.button(tr("border.outer")).clicked() {
                    for s in &mut f.sides[..4] {
                        *s = Some(f.line);
                    }
                }
                if f.multi && ui.button(tr("border.inside")).clicked() {
                    f.sides[4] = Some(f.line);
                    f.sides[5] = Some(f.line);
                }
            });
            widgets::section(ui, tr("fmtdlg.border"));
            ui.label(RichText::new(tr("border.click_sides")).weak().small());
            // Preview: click a side to toggle it.
            let (rect, resp) = ui.allocate_exact_size(vec2(200.0, 130.0), Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, 0.0, Paper::BG);
            let inner = rect.shrink(16.0);
            let o = &f.orig.borders;
            let current = [o.top, o.bottom, o.left, o.right, Line::default(), Line::default()];
            let side = |i: usize| f.sides[i].unwrap_or(current[i]);
            let text = Paper::TEXT;
            p.text(inner.center(), egui::Align2::CENTER_CENTER, tr("border.sample"), FontId::proportional(13.0), Color32::GRAY);
            let (a, b, c, d) = (inner.left_top(), inner.right_top(), inner.left_bottom(), inner.right_bottom());
            let lines = [(a, b), (c, d), (a, c), (b, d), (pos2(a.x, inner.center().y), pos2(b.x, inner.center().y)), (pos2(inner.center().x, a.y), pos2(inner.center().x, c.y))];
            for (i, (s, e)) in lines.iter().enumerate() {
                if i >= 4 && !f.multi {
                    continue;
                }
                let l = side(i);
                if l.is_none() {
                    p.line_segment([*s, *e], Stroke::new(1.0, Color32::from_gray(225)));
                } else {
                    paint_line(p, *s, *e, l, l.color.map_or(text, theme::rgb), false);
                }
            }
            if resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let dist = |s: Pos2, e: Pos2| {
                        let t = ((pos - s).dot(e - s) / (e - s).length_sq()).clamp(0.0, 1.0);
                        (s + (e - s) * t - pos).length()
                    };
                    let best = lines.iter().enumerate().filter(|(i, _)| *i < 4 || f.multi).min_by(|x, y| dist(x.1.0, x.1.1).total_cmp(&dist(y.1.0, y.1.1)));
                    if let Some((i, _)) = best {
                        f.sides[i] = Some(if side(i).is_none() { f.line } else { Line::default() });
                    }
                }
            }
        });
    });
}
