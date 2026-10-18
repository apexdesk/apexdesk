//! The macro recorder: what the user does to a workbook while it records,
//! written as VBA. Each undo step is read as it is made, from what it
//! undoes (the state before) and the workbook as it is then (after);
//! the selection comes from the view. Steps it can't write become a
//! comment.

use std::collections::HashMap;

use crate::addr::{CellRef, Range};
use crate::format::Locale;
use crate::sheet::{Cell, CellStyle, Content, HAlign, Line, VAlign};
use crate::value::Value;
use crate::workbook::{Op, Workbook};

/// Where the selection is: the sheet's name, the selected range and the
/// active cell.
#[derive(Clone, Debug, PartialEq)]
struct Place {
    sheet: String,
    range: Range,
    active: CellRef,
}

/// Lines written for one step at most (a paste over a large irregular
/// area); the rest is left out with a comment.
const MAX_LINES: usize = 2000;

#[derive(Clone, Debug, Default)]
pub struct Recorder {
    /// Ranges relative to the active cell (`ActiveCell.Offset(1, 0)`)
    /// instead of fixed ones (`Range("B2")`).
    pub relative: bool,
    lines: Vec<String>,
    /// Where the recorded code leaves the selection.
    at: Option<Place>,
    /// Where it is now.
    now: Option<Place>,
    /// What the next step is, when the workbook said (a sort is read
    /// as one, not as the cells it moved).
    hint: Option<Hint>,
}

#[derive(Clone, Debug)]
enum Hint {
    Sort { sheet: usize, range: Range, keys: Vec<(u32, bool)>, case: bool, by_cols: bool },
}

impl Recorder {
    pub fn new(relative: bool) -> Recorder {
        Recorder { relative, ..Recorder::default() }
    }

    /// The view's selection now: on `sheet`, `range` with `active` in it.
    pub fn select(&mut self, sheet: &str, range: Range, active: CellRef) {
        let p = Place { sheet: sheet.to_string(), range, active };
        if self.at.is_none() {
            self.at = Some(p.clone());
        }
        self.now = Some(p);
    }

    /// The code so far, the last selection included.
    pub fn finish(&mut self) -> Vec<String> {
        self.flush_selection();
        self.lines.clone()
    }

    /// The code written since the last call (a selection made since
    /// stays to be written).
    pub fn take_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.lines)
    }

    /// The code so far.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The selection the code leaves, when it moved since.
    fn flush_selection(&mut self) {
        let (Some(at), Some(now)) = (self.at.clone(), self.now.clone()) else { return };
        if at == now {
            return;
        }
        let other_sheet = at.sheet != now.sheet;
        if other_sheet {
            self.lines.push(format!("Sheets({}).Select", quote(&now.sheet)));
        }
        if other_sheet && now.range.is_single() && now.range.start == CellRef::new(0, 0) && now.active == now.range.start {
            // A sheet selected starts at A1.
        } else if self.relative && !other_sheet {
            self.lines.push(format!("{}.Select", relative(at.active, now.range)));
        } else {
            self.lines.push(format!("Range(\"{}\").Select", now.range.to_a1()));
        }
        if now.active != now.range.start {
            if self.relative {
                self.lines.push(format!("{}.Activate", relative(now.range.start, Range::single(now.active))));
            } else {
                self.lines.push(format!("Range(\"{}\").Activate", now.active.to_a1()));
            }
        }
        self.at = Some(now);
    }

    /// How the code names `range` of sheet `sheet`.
    fn target(&self, wb: &Workbook, sheet: usize, range: Range) -> String {
        let name = wb.sheets.get(sheet).map(|s| s.name.as_str()).unwrap_or_default();
        let Some(at) = self.at.as_ref().filter(|a| a.sheet == name) else {
            return format!("Sheets({}).Range(\"{}\")", quote(name), range.to_a1());
        };
        if range.is_single() && range.start == at.active {
            "ActiveCell".into()
        } else if range == at.range {
            "Selection".into()
        } else if self.relative {
            relative(at.active, range)
        } else {
            format!("Range(\"{}\")", range.to_a1())
        }
    }

    /// The workbook sorts `range` by `keys`; values sorts of up to
    /// three keys are written as Range.Sort.
    pub(crate) fn sorting(&mut self, sheet: usize, range: Range, keys: &[crate::workbook::SortKey], case: bool, by_cols: bool) {
        let plain = keys.len() <= 3 && keys.iter().all(|k| k.on == crate::workbook::SortOn::Values && k.list.is_none());
        self.hint = plain.then(|| Hint::Sort { sheet, range, keys: keys.iter().map(|k| (k.col, k.ascending)).collect(), case, by_cols });
    }

    /// Reads the undo step `inv` the workbook just made (`label`, its
    /// i18n key).
    pub(crate) fn step(&mut self, wb: &Workbook, inv: &Op, label: &str) {
        self.flush_selection();
        let mut out = Vec::new();
        match self.hint.take() {
            Some(Hint::Sort { sheet, range, keys, case, by_cols }) if label == "undo.sort" && matches!(inv, Op::SetCells { .. }) => {
                let mut args = Vec::new();
                for (i, (k, asc)) in keys.iter().enumerate() {
                    let cell = if by_cols { CellRef::new(*k, range.start.col) } else { CellRef::new(range.start.row, *k) };
                    args.push(format!("Key{n}:={}, Order{n}:={}", self.target(wb, sheet, Range::single(cell)), if *asc { "xlAscending" } else { "xlDescending" }, n = i + 1));
                }
                args.push("Header:=xlNo".into());
                if case {
                    args.push("MatchCase:=True".into());
                }
                if by_cols {
                    args.push("Orientation:=xlLeftToRight".into());
                }
                out.push(format!("{}.Sort {}", self.target(wb, sheet, range), args.join(", ")));
            }
            _ => self.op(wb, inv, label, &mut out),
        }
        if out.len() > MAX_LINES {
            out.truncate(MAX_LINES);
            out.push("' ...".into());
        }
        self.lines.extend(out);
    }

    fn op(&mut self, wb: &Workbook, inv: &Op, label: &str, out: &mut Vec<String>) {
        let sheet_name = |i: usize| wb.sheets.get(i).map(|s| s.name.clone()).unwrap_or_default();
        match inv {
            Op::Batch(ops) => {
                // A deletion undoes as an insertion with what the deleted
                // lines held put back: only the insertion says what was done.
                let restores = ops.iter().any(|o| matches!(o, Op::InsertRows { .. } | Op::InsertCols { .. } | Op::InsertRowsAt { .. }));
                // Inverses run last first: the edits were made the other way.
                for op in ops.iter().rev() {
                    if !restores || matches!(op, Op::InsertRows { .. } | Op::InsertCols { .. } | Op::InsertRowsAt { .. }) {
                        self.op(wb, op, label, out);
                    }
                }
            }
            Op::SetCells { sheet, cells } => self.cells(wb, *sheet, cells, out),
            // An insertion undoes as a deletion, and the other way round.
            Op::DeleteRows { sheet, at, count } => out.push(format!("{}.Insert", self.lines_of(wb, *sheet, *at, *count, true))),
            Op::InsertRows { sheet, at, count } => out.push(format!("{}.Delete", self.lines_of(wb, *sheet, *at, *count, true))),
            Op::DeleteCols { sheet, at, count } => out.push(format!("{}.Insert", self.lines_of(wb, *sheet, *at, *count, false))),
            Op::InsertCols { sheet, at, count } => out.push(format!("{}.Delete", self.lines_of(wb, *sheet, *at, *count, false))),
            Op::DeleteRowsAt { sheet, rows } => {
                // Rows inserted one before each of `rows`, as they are now.
                for (k, r) in rows.iter().enumerate().rev() {
                    out.push(format!("{}.Insert", self.lines_of(wb, *sheet, r.saturating_sub(k as u32), 1, true)));
                }
            }
            Op::InsertRowsAt { sheet, at } => {
                for (k, r) in at.iter().enumerate().rev() {
                    out.push(format!("{}.Delete", self.lines_of(wb, *sheet, r + k as u32, 1, true)));
                }
            }
            Op::ColWidths { sheet, sizes } => {
                let sh = &wb.sheets[*sheet];
                let now: Vec<(u32, String)> = sizes.iter().map(|(c, _)| (*c, col_width(sh.col_width(*c)))).collect();
                for (a, b, v) in runs(now) {
                    out.push(format!("{}.{v}", self.lines_of(wb, *sheet, a, b - a + 1, false)));
                }
            }
            Op::RowHeights { sheet, sizes } => {
                let sh = &wb.sheets[*sheet];
                let now: Vec<(u32, String)> = sizes.iter().map(|(r, _)| (*r, row_height(sh.row_height(*r)))).collect();
                for (a, b, v) in runs(now) {
                    out.push(format!("{}.{v}", self.lines_of(wb, *sheet, a, b - a + 1, true)));
                }
            }
            Op::RemoveSheet { index } => {
                // A sheet added at `index`.
                let name = sheet_name(*index);
                let place = if *index > 0 { format!("After:=Sheets({})", quote(&sheet_name(index - 1))) } else { format!("Before:=Sheets({})", quote(&sheet_name(index + 1))) };
                let source = wb.sheets.get(*index).filter(|s| s.extent().is_some()).and_then(|_| name.rsplit_once(" (")).map(|(base, _)| base.to_string()).filter(|base| wb.sheet_index(base).is_some());
                match source {
                    Some(base) => out.push(format!("Sheets({}).Copy {place}", quote(&base))),
                    None => out.push(format!("Sheets.Add {place}")),
                }
                out.push(format!("ActiveSheet.Name = {}", quote(&name)));
                if let Some(at) = &mut self.at {
                    *at = Place { sheet: name, range: Range::single(CellRef::new(0, 0)), active: CellRef::new(0, 0) };
                }
            }
            Op::AddSheet { sheet, .. } => {
                out.push("Application.DisplayAlerts = False".into());
                out.push(format!("Sheets({}).Delete", quote(&sheet.name)));
                out.push("Application.DisplayAlerts = True".into());
            }
            Op::RenameSheet { index, name } => out.push(format!("Sheets({}).Name = {}", quote(name), quote(&sheet_name(*index)))),
            Op::MoveSheet { from, .. } => {
                // The sheet now at `from` was moved there.
                let name = sheet_name(*from);
                if *from + 1 < wb.sheets.len() {
                    out.push(format!("Sheets({}).Move Before:=Sheets({})", quote(&name), quote(&sheet_name(from + 1))));
                } else if *from > 0 {
                    out.push(format!("Sheets({}).Move After:=Sheets({})", quote(&name), quote(&sheet_name(from - 1))));
                }
            }
            Op::SheetProps { index, .. } => {
                let sh = &wb.sheets[*index];
                out.push(format!("Sheets({}).Visible = {}", quote(&sh.name), vb_bool(!sh.hidden)));
            }
            Op::Freeze { sheet, .. } => {
                let sh = &wb.sheets[*sheet];
                if sh.frozen_rows == 0 && sh.frozen_cols == 0 {
                    out.push("ActiveWindow.FreezePanes = False".into());
                } else {
                    out.push("With ActiveWindow".into());
                    out.push(format!("    .SplitColumn = {}", sh.frozen_cols));
                    out.push(format!("    .SplitRow = {}", sh.frozen_rows));
                    out.push("    .FreezePanes = True".into());
                    out.push("End With".into());
                }
            }
            Op::Merges { sheet, merges } => {
                let now = &wb.sheets[*sheet].merges;
                for m in merges.iter().filter(|m| !now.contains(m)) {
                    out.push(format!("{}.UnMerge", self.target(wb, *sheet, *m)));
                }
                for m in now.iter().filter(|m| !merges.contains(m)) {
                    out.push(format!("{}.Merge", self.target(wb, *sheet, *m)));
                }
            }
            Op::Filter { sheet, filter } => {
                let now = &wb.sheets[*sheet].filter;
                match (filter, now) {
                    (Some(_), None) => out.push(format!("Sheets({}).AutoFilterMode = False", quote(&sheet_name(*sheet)))),
                    (None, Some(f)) => out.push(format!("{}.AutoFilter", self.target(wb, *sheet, f.range))),
                    _ => out.push(format!("' {}", not_recorded(label))),
                }
            }
            _ => out.push(format!("' {}", not_recorded(label))),
        }
    }

    /// Rows (or columns) `at..at+count` of `sheet`.
    fn lines_of(&self, wb: &Workbook, sheet: usize, at: u32, count: u32, rows: bool) -> String {
        let name = wb.sheets.get(sheet).map(|s| s.name.as_str()).unwrap_or_default();
        let prefix = if self.at.as_ref().is_some_and(|a| a.sheet == name) { String::new() } else { format!("Sheets({}).", quote(name)) };
        let end = at + count.max(1) - 1;
        if self.relative && prefix.is_empty()
            && let Some(a) = &self.at
        {
            let (r, n) = if rows { (Range::new(CellRef::new(at, a.active.col), CellRef::new(end, a.active.col)), "EntireRow") } else { (Range::new(CellRef::new(a.active.row, at), CellRef::new(a.active.row, end)), "EntireColumn") };
            return format!("{}.{n}", relative(a.active, r));
        }
        if rows {
            format!("{prefix}Rows(\"{}:{}\")", at + 1, end + 1)
        } else {
            let (a, b) = (CellRef::new(0, at).to_a1(), CellRef::new(0, end).to_a1());
            format!("{prefix}Columns(\"{}:{}\")", a.trim_end_matches('1'), b.trim_end_matches('1'))
        }
    }

    /// Cells changed: their content and format, grouped into ranges.
    fn cells(&self, wb: &Workbook, sheet: usize, before: &[(CellRef, Option<Cell>)], out: &mut Vec<String>) {
        // What each cell got, in the order the cells come.
        let mut keys: Vec<String> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut cells: Vec<Vec<CellRef>> = Vec::new();
        let default = CellStyle::default();
        for (c, old) in before {
            let new = wb.cell(sheet, *c);
            let mut changes = Vec::new();
            let (a, b) = (content(old.as_ref(), *c), content(new, *c));
            if a != b && !new.is_some_and(|n| matches!(n.content, Content::Spill(_))) {
                changes.push(b.map_or_else(|| ".ClearContents".to_string(), |b| format!(".{b}")));
            }
            let sa = old.as_ref().map_or(&default, |x| wb.styles.get(x.style));
            let sb = new.map_or(&default, |x| wb.styles.get(x.style));
            if sa != sb {
                style_changes(sa, sb, &mut changes);
            }
            for k in changes {
                let i = *index.entry(k.clone()).or_insert_with(|| {
                    keys.push(k);
                    cells.push(Vec::new());
                    keys.len() - 1
                });
                cells[i].push(*c);
            }
        }
        // The ranges, each with what it got.
        let mut ranges: Vec<(Range, Vec<&str>)> = Vec::new();
        for (k, list) in keys.iter().zip(&cells) {
            for r in rectangles(list) {
                match ranges.iter_mut().find(|(x, _)| *x == r) {
                    Some((_, ks)) => ks.push(k),
                    None => ranges.push((r, vec![k])),
                }
            }
        }
        ranges.sort_by_key(|(r, _)| (r.start.row, r.start.col));
        for (r, ks) in ranges {
            let t = self.target(wb, sheet, r);
            let mut lines: Vec<String> = Vec::new();
            for k in ks {
                for l in if k.starts_with(BORDER) { borders(k, r) } else { vec![k.to_string()] } {
                    if !lines.contains(&l) {
                        lines.push(l);
                    }
                }
            }
            if let [k] = &lines[..] {
                out.push(format!("{t}{k}"));
            } else {
                out.push(format!("With {t}"));
                out.extend(lines.iter().map(|k| format!("    {k}")));
                out.push("End With".into());
            }
        }
    }
}

/// `ActiveCell.Offset(1, 0)`, sized like `range`, from `from`.
fn relative(from: CellRef, range: Range) -> String {
    let (dr, dc) = (range.start.row as i64 - from.row as i64, range.start.col as i64 - from.col as i64);
    let base = if (dr, dc) == (0, 0) { "ActiveCell".to_string() } else { format!("ActiveCell.Offset({dr}, {dc})") };
    if range.is_single() { base } else { format!("{base}.Resize({}, {})", range.rows(), range.cols()) }
}

/// What a cell holds, as a property set (`Value = 5`,
/// `FormulaR1C1 = "=R[-1]C*2"`); None when empty.
fn content(cell: Option<&Cell>, at: CellRef) -> Option<String> {
    let cell = cell?;
    if let Some(f) = cell.formula() {
        return Some(format!("FormulaR1C1 = {}", quote(&f.expr().to_display_r1c1(&Locale::EN, at))));
    }
    Some(match &cell.content {
        Content::Empty | Content::Spill(_) => return None,
        Content::Value(v) => match v {
            Value::Empty => return None,
            Value::Number(n) => format!("Value = {}", number(*n)),
            Value::Text(t) => {
                // Text that would be read as something else (a number, a
                // formula) keeps an apostrophe.
                let looks = t.starts_with('=') || !matches!(crate::format::parse_input(t, &Locale::EN).value, Value::Text(ref x) if **x == **t);
                format!("Value = {}", quote(&if looks { format!("'{t}") } else { t.to_string() }))
            }
            Value::Bool(b) => format!("Value = {}", vb_bool(*b)),
            Value::Error(e) => format!("Formula = {}", quote(e.as_str())),
            _ => return None,
        },
        Content::Formula { .. } => return None,
    })
}

/// The format properties that changed from `a` to `b`.
fn style_changes(a: &CellStyle, b: &CellStyle, out: &mut Vec<String>) {
    if a.font != b.font {
        out.push(format!(".Font.Name = {}", quote(b.font.as_deref().unwrap_or("Calibri"))));
    }
    if a.font_size != b.font_size {
        out.push(format!(".Font.Size = {}", number(if b.font_size == 0 { 11.0 } else { b.font_size as f64 / 10.0 })));
    }
    if a.bold != b.bold {
        out.push(format!(".Font.Bold = {}", vb_bool(b.bold)));
    }
    if a.italic != b.italic {
        out.push(format!(".Font.Italic = {}", vb_bool(b.italic)));
    }
    if a.underline != b.underline {
        out.push(format!(".Font.Underline = {}", if b.underline { "xlUnderlineStyleSingle" } else { "xlUnderlineStyleNone" }));
    }
    if a.strike != b.strike {
        out.push(format!(".Font.Strikethrough = {}", vb_bool(b.strike)));
    }
    if a.color != b.color {
        out.push(match b.color {
            Some(c) => format!(".Font.Color = {}", rgb(c)),
            None => ".Font.ColorIndex = xlAutomatic".into(),
        });
    }
    if a.fill != b.fill {
        out.push(match b.fill {
            Some(c) => format!(".Interior.Color = {}", rgb(c)),
            None => ".Interior.Pattern = xlNone".into(),
        });
    }
    if a.num_fmt != b.num_fmt {
        out.push(format!(".NumberFormat = {}", quote(if b.num_fmt.is_empty() { "General" } else { &b.num_fmt })));
    }
    if a.halign != b.halign {
        let h = match b.halign {
            HAlign::General => "xlGeneral",
            HAlign::Left => "xlLeft",
            HAlign::Center => "xlCenter",
            HAlign::Right => "xlRight",
            HAlign::Justify => "xlJustify",
        };
        out.push(format!(".HorizontalAlignment = {h}"));
    }
    if a.valign != b.valign {
        let v = match b.valign {
            VAlign::Top => "xlTop",
            VAlign::Center => "xlCenter",
            VAlign::Bottom => "xlBottom",
        };
        out.push(format!(".VerticalAlignment = {v}"));
    }
    if a.wrap != b.wrap {
        out.push(format!(".WrapText = {}", vb_bool(b.wrap)));
    }
    if a.indent != b.indent {
        out.push(format!(".IndentLevel = {}", b.indent));
    }
    if a.rotation != b.rotation {
        out.push(format!(".Orientation = {}", if b.rotation == crate::sheet::ROTATION_STACKED { "xlVertical".to_string() } else { b.rotation.to_string() }));
    }
    if a.unlocked != b.unlocked {
        out.push(format!(".Locked = {}", vb_bool(!b.unlocked)));
    }
    if a.formula_hidden != b.formula_hidden {
        out.push(format!(".FormulaHidden = {}", vb_bool(b.formula_hidden)));
    }
    for (side, la, lb) in [('t', a.borders.top, b.borders.top), ('b', a.borders.bottom, b.borders.bottom), ('l', a.borders.left, b.borders.left), ('r', a.borders.right, b.borders.right)] {
        if la != lb {
            out.push(border_mark(side, lb));
        }
    }
}

/// A border side changed: a mark the cells are grouped by, turned into
/// code once the range is known (see [`borders`]).
const BORDER: char = '\u{1}';

fn border_mark(side: char, l: Line) -> String {
    let color = l.color.map(rgb).unwrap_or_default();
    format!("{BORDER}{side}{:?}|{color}", l.style)
}

/// The code for a border mark on `range`: Excel's `Borders(xlEdgeTop)`
/// is the range's outer edge, so a side of every cell takes the inside
/// lines too.
fn borders(mark: &str, range: Range) -> Vec<String> {
    let mut it = mark.trim_start_matches(BORDER).chars();
    let side = it.next().unwrap_or('t');
    let rest: String = it.collect();
    let (style, color) = rest.split_once('|').unwrap_or((&rest, ""));
    let mut sides = vec![match side {
        't' => "xlEdgeTop",
        'b' => "xlEdgeBottom",
        'l' => "xlEdgeLeft",
        _ => "xlEdgeRight",
    }];
    if matches!(side, 't' | 'b') && range.rows() > 1 {
        sides.push("xlInsideHorizontal");
    }
    if matches!(side, 'l' | 'r') && range.cols() > 1 {
        sides.push("xlInsideVertical");
    }
    let (line, weight) = match style {
        "None" => ("xlNone", ""),
        "Thin" => ("xlContinuous", "xlThin"),
        "Medium" => ("xlContinuous", "xlMedium"),
        "Thick" => ("xlContinuous", "xlThick"),
        "Dashed" => ("xlDash", "xlThin"),
        "Dotted" => ("xlDot", "xlThin"),
        "Double" => ("xlDouble", "xlThick"),
        _ => ("xlContinuous", "xlHairline"),
    };
    let mut out = Vec::new();
    for s in sides {
        out.push(format!(".Borders({s}).LineStyle = {line}"));
        if !weight.is_empty() {
            out.push(format!(".Borders({s}).Weight = {weight}"));
        }
        if !color.is_empty() && !weight.is_empty() {
            out.push(format!(".Borders({s}).Color = {color}"));
        }
    }
    out
}

/// The cells of `list` as few rectangles: runs along each row, joined
/// downwards while the rows below have the same run.
fn rectangles(list: &[CellRef]) -> Vec<Range> {
    let mut set: Vec<CellRef> = list.to_vec();
    set.sort_by_key(|c| (c.row, c.col));
    set.dedup();
    let has = |c: CellRef, set: &[CellRef]| set.binary_search_by_key(&(c.row, c.col), |x| (x.row, x.col)).is_ok();
    let mut used = vec![false; set.len()];
    let mut out = Vec::new();
    for i in 0..set.len() {
        if used[i] {
            continue;
        }
        let start = set[i];
        let mut end_col = start.col;
        while has(CellRef::new(start.row, end_col + 1), &set) && !used_at(&set, &used, CellRef::new(start.row, end_col + 1)) {
            end_col += 1;
        }
        let mut end_row = start.row;
        'down: loop {
            let r = end_row + 1;
            for c in start.col..=end_col {
                let x = CellRef::new(r, c);
                if !has(x, &set) || used_at(&set, &used, x) {
                    break 'down;
                }
            }
            // A row below whose run is wider is left for its own rectangle.
            end_row = r;
        }
        for r in start.row..=end_row {
            for c in start.col..=end_col {
                if let Ok(k) = set.binary_search_by_key(&(r, c), |x| (x.row, x.col)) {
                    used[k] = true;
                }
            }
        }
        out.push(Range::new(start, CellRef::new(end_row, end_col)));
    }
    out
}

fn used_at(set: &[CellRef], used: &[bool], c: CellRef) -> bool {
    set.binary_search_by_key(&(c.row, c.col), |x| (x.row, x.col)).is_ok_and(|k| used[k])
}

/// Runs of the same value over consecutive indexes: (first, last, value).
fn runs(mut v: Vec<(u32, String)>) -> Vec<(u32, u32, String)> {
    v.sort_by_key(|x| x.0);
    let mut out: Vec<(u32, u32, String)> = Vec::new();
    for (i, s) in v {
        match out.last_mut() {
            Some((_, b, t)) if *b + 1 == i && *t == s => *b = i,
            _ => out.push((i, i, s)),
        }
    }
    out
}

/// The width VBA sets (the pixels of the sheet, as characters).
fn col_width(px: f32) -> String {
    if px <= 0.0 {
        return "Hidden = True".into();
    }
    let w = ((px / COL_SCALE - 5.0) / 7.0 * 100.0).round() / 100.0;
    format!("ColumnWidth = {}", number(w.max(0.0) as f64))
}

fn row_height(px: f32) -> String {
    if px <= 0.0 {
        return "Hidden = True".into();
    }
    format!("RowHeight = {}", number(((px / ROW_SCALE * 4.0).round() / 4.0) as f64))
}

/// Sheet pixels per Excel unit, as in the VBA engine.
const COL_SCALE: f32 = 88.0 / 64.0;
const ROW_SCALE: f32 = 22.0 / 15.0;

fn not_recorded(label: &str) -> String {
    format!("Not recorded: {}", label.strip_prefix("undo.").unwrap_or(label).replace('_', " "))
}

fn quote(s: &str) -> String {
    let s = s.replace('"', "\"\"");
    // Line breaks can't be in a VBA string literal.
    let s = s.replace("\r\n", "\" & vbLf & \"").replace(['\n', '\r'], "\" & vbLf & \"");
    format!("\"{s}\"")
}

fn vb_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

fn rgb(c: u32) -> String {
    format!("RGB({}, {}, {})", c >> 16 & 0xFF, c >> 8 & 0xFF, c & 0xFF)
}

fn number(n: f64) -> String {
    if n.is_finite() && n == n.trunc() && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

/// Recorded code (absolute references) without the selecting: a range
/// selected and then used (`Range("B2:B10").Select` / `Selection.Font.Bold
/// = True`) becomes `Range("B2:B10").Font.Bold = True`, and a selection
/// nothing uses is left out. Lines that depend on the selection some other
/// way (pasting, the window, running a macro) keep it selected first.
/// Sheets are still selected: unqualified ranges depend on them.
pub fn clean(lines: &[String]) -> Vec<String> {
    /// The range selected and its active cell, not yet written.
    struct Pending {
        range: String,
        active: String,
        written: bool,
    }
    fn needs_selection(line: &str) -> bool {
        let words = ["Selection", "ActiveCell", "ActiveWindow", "ActiveSheet.Paste", "Application.Run", ".Paste", "SendKeys"];
        !line.trim_start().starts_with('\'') && words.iter().any(|w| line.contains(w))
    }
    fn write_selection(p: &mut Pending, out: &mut Vec<String>) {
        out.push(format!("{}.Select", p.range));
        if p.active != first_cell(&p.range) {
            out.push(format!("{}.Activate", p.active));
        }
        p.written = true;
    }
    let mut out = Vec::new();
    let mut at: Option<Pending> = None;
    for line in lines {
        let l = line.as_str();
        if let Some(r) = l.strip_suffix(".Select").filter(|r| is_range(r)) {
            at = Some(Pending { range: r.to_string(), active: first_cell(r), written: false });
            continue;
        }
        if let (Some(r), Some(p)) = (l.strip_suffix(".Activate").filter(|r| is_range(r)), at.as_mut()) {
            p.active = r.to_string();
            p.written = false;
            continue;
        }
        let Some(p) = at.as_mut() else {
            out.push(line.clone());
            continue;
        };
        if l.ends_with(".Select") {
            // Another sheet, or a selection relative to this one: what is
            // selected after isn't known here.
            if needs_selection(l) && !p.written {
                write_selection(p, &mut out);
            }
            at = None;
            out.push(line.clone());
            continue;
        }
        let rewritten = if let Some(rest) = l.strip_prefix("Selection.").filter(|r| !needs_selection(r)) {
            Some(format!("{}.{rest}", p.range))
        } else if let Some(rest) = l.strip_prefix("ActiveCell.").filter(|r| !needs_selection(r)) {
            Some(format!("{}.{rest}", p.active))
        } else if l == "With Selection" {
            Some(format!("With {}", p.range))
        } else if l == "With ActiveCell" {
            Some(format!("With {}", p.active))
        } else {
            None
        };
        match rewritten {
            Some(r) => out.push(r),
            None => {
                if needs_selection(l) && !p.written {
                    write_selection(p, &mut out);
                }
                out.push(line.clone());
            }
        }
    }
    out
}

/// `Range("…")` as the recorder writes it.
fn is_range(s: &str) -> bool {
    s.strip_prefix("Range(\"").and_then(|r| r.strip_suffix("\")")).is_some_and(|a| !a.contains('"'))
}

/// `Range("B2")` for `Range("B2:C5,E1")`.
fn first_cell(range: &str) -> String {
    let a = &range[7..range.len() - 2];
    let first = a.split([':', ',']).next().unwrap_or(a);
    format!("Range(\"{first}\")")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_cover_the_cells() {
        let c = |s: &str| CellRef::parse(s).unwrap();
        let list = [c("A1"), c("B1"), c("A2"), c("B2"), c("D5"), c("A3")];
        let r: Vec<String> = rectangles(&list).iter().map(|r| r.to_a1()).collect();
        assert_eq!(r, ["A1:B2", "A3", "D5"]);
    }

    #[test]
    fn literals() {
        assert_eq!(quote("a\"b\nc"), "\"a\"\"b\" & vbLf & \"c\"");
        assert_eq!(number(2.0), "2");
        assert_eq!(number(2.5), "2.5");
        assert_eq!(rgb(0xFF0080), "RGB(255, 0, 128)");
        assert_eq!(relative(CellRef::new(1, 1), Range::new(CellRef::new(2, 1), CellRef::new(3, 2))), "ActiveCell.Offset(1, 0).Resize(2, 2)");
    }

    #[test]
    fn clean_code() {
        let lines: Vec<String> = [
            "Range(\"A1\").Select",
            "Range(\"B2:B10\").Select",
            "Selection.Font.Bold = True",
            "Range(\"C3\").Activate",
            "ActiveCell.FormulaR1C1 = \"x\"",
            "With Selection",
            "    .HorizontalAlignment = xlCenter",
            "End With",
            "Range(\"D1\").Select",
            "ActiveSheet.Paste",
            "Selection.Copy",
            "Sheets(\"Data\").Select",
            "Selection.ClearContents",
            "Range(\"E5\").Select",
            "Range(\"F6\").Select",
            "ActiveCell.Offset(1, 0).Range(\"A1\").Select",
            "Selection.Value = 1",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            clean(&lines),
            [
                "Range(\"B2:B10\").Font.Bold = True",
                "Range(\"C3\").FormulaR1C1 = \"x\"",
                "With Range(\"B2:B10\")",
                "    .HorizontalAlignment = xlCenter",
                "End With",
                "Range(\"D1\").Select",
                "ActiveSheet.Paste",
                "Range(\"D1\").Copy",
                "Sheets(\"Data\").Select",
                "Selection.ClearContents",
                "Range(\"F6\").Select",
                "ActiveCell.Offset(1, 0).Range(\"A1\").Select",
                "Selection.Value = 1",
            ]
        );
        assert_eq!(first_cell("Range(\"B2:C5,E1\")"), "Range(\"B2\")");
    }
}
