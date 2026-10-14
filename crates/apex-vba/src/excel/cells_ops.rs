//! Range methods made with the workbook's own edits: SpecialCells,
//! FillDown and the like, AutoFill, RemoveDuplicates, Replace, notes
//! (AddComment), and Copy or Cut without a destination with PasteSpecial
//! and Worksheet.Paste.

use apex_sheet::workbook::{AutoFill, FillDir, PasteOp, PasteSpecial, PasteWhat, SPECIAL_ALL_TYPES, SPECIAL_ERRORS, SPECIAL_LOGICAL, SPECIAL_NUMBERS, SPECIAL_TEXT, Special, Workbook};
use apex_sheet::{CellRef, Content, Note, Range};

use super::{Args, Interp, areas};
use crate::error::{Error, R};
use crate::object::{Obj, Part, RangeRef};
use crate::value::Value;

/// Cells a Replace looks at, at most.
const MAX_REPLACE: u64 = 4_000_000;

fn no_cells() -> Error {
    Error::app("No cells were found.")
}

impl Interp<'_> {
    /// Runs one of the workbook's edits as the macro's: its steps join
    /// the macro's undo step. The workbook checks protection op by op;
    /// what it refused is an error.
    pub(super) fn via<T>(&mut self, f: impl FnOnce(&mut Workbook) -> T) -> R<T> {
        let wb = self.wb_mut()?;
        if wb.read_only {
            return Err(Error::app("The cell or sheet is protected or the workbook is read-only"));
        }
        wb.take_blocked();
        let (t, steps) = wb.steps_of(f);
        let blocked = wb.take_blocked();
        self.undo.extend(steps);
        if blocked {
            return Err(Error::app("The cell or sheet is protected or the workbook is read-only"));
        }
        Ok(t)
    }

    /// Range.SpecialCells(Type, [Value]).
    pub(super) fn special_cells(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let ty = self.arg_i64(a, 0, "type")?.ok_or_else(Error::invalid_call)?;
        let bits = match self.arg_i64(a, 1, "value")? {
            None => SPECIAL_ALL_TYPES,
            Some(v) => {
                let v = v as u8;
                let mut b = 0;
                for (x, y) in [(1, SPECIAL_NUMBERS), (2, SPECIAL_TEXT), (4, SPECIAL_LOGICAL), (16, SPECIAL_ERRORS)] {
                    if v & x != 0 {
                        b |= y;
                    }
                }
                b
            }
        };
        let kind = match ty {
            4 => Special::Blanks,
            2 => Special::Constants(bits),
            -4123 => Special::Formulas(bits),
            -4144 => Special::Notes,
            11 => Special::LastCell,
            12 => Special::Visible,
            -4172 => Special::CondFormats { same: false },
            -4173 => Special::CondFormats { same: true },
            -4174 => Special::Validation { same: false },
            -4175 => Special::Validation { same: true },
            _ => return Err(Error::invalid_call()),
        };
        // One cell: the whole sheet, like Excel.
        let sel = if r.range.is_single() { Range::new(CellRef::new(0, 0), CellRef::new(apex_sheet::MAX_ROWS - 1, apex_sheet::MAX_COLS - 1)) } else { r.range };
        let found = self.wb.special_cells(r.sheet, &sel, kind, r.range.start);
        let found: Vec<Range> = found.into_iter().filter_map(|x| x.intersect(&sel)).collect();
        if found.is_empty() {
            return Err(no_cells());
        }
        areas::areas(r.sheet, found)
    }

    /// FillDown, FillUp, FillRight and FillLeft.
    pub(super) fn fill(&mut self, r: RangeRef, name: &str) -> R<Value> {
        let dir = match name {
            "filldown" => FillDir::Down,
            "fillup" => FillDir::Up,
            "fillright" => FillDir::Right,
            _ => FillDir::Left,
        };
        Self::check_size(&r.range)?;
        self.via(|wb| wb.fill_dir(r.sheet, &r.range, dir))?;
        Ok(Value::Bool(true))
    }

    /// Range.AutoFill Destination, [Type].
    pub(super) fn auto_fill(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let dest = a.get(0, "destination").ok_or_else(Error::invalid_call)?;
        let dest = self.range_arg(dest, r.sheet)?;
        let (s, d) = (r.range, dest.range);
        // The destination holds the source and grows it one way.
        let fits = dest.sheet == r.sheet && d.contains(s.start) && d.contains(s.end) && (d.cols() == s.cols() && d.start.col == s.start.col || d.rows() == s.rows() && d.start.row == s.start.row);
        if !fits {
            return Err(Error::app("AutoFill method of Range class failed"));
        }
        Self::check_size(&d)?;
        let mode = match self.arg_i64(a, 1, "type")?.unwrap_or(0) {
            1 => AutoFill::Copy,
            2 => AutoFill::Series,
            3 => AutoFill::Formats,
            4 => AutoFill::Values,
            5 => AutoFill::Days,
            6 => AutoFill::Weekdays,
            7 => AutoFill::Months,
            8 => AutoFill::Years,
            _ => AutoFill::Smart,
        };
        self.via(|wb| wb.auto_fill(r.sheet, &s, &d, mode))?;
        Ok(Value::Bool(true))
    }

    /// Range.RemoveDuplicates Columns, [Header].
    pub(super) fn remove_duplicates(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let given = a.get(0, "columns").ok_or_else(Error::invalid_call)?;
        let list = match given {
            Value::Array(arr) => arr.data.clone(),
            v => vec![v],
        };
        let mut cols = Vec::with_capacity(list.len());
        for v in list {
            let n = self.scalar(v)?.to_i64()?;
            if n < 1 || n > r.range.cols() as i64 {
                return Err(Error::app("RemoveDuplicates method of Range class failed"));
            }
            cols.push(r.range.start.col + n as u32 - 1);
        }
        let header = self.arg_i64(a, 1, "header")?.unwrap_or(2) == 1;
        self.via(|wb| wb.remove_duplicates(r.sheet, &r.range, &cols, header))?;
        Ok(Value::Empty)
    }

    /// Range.Replace What, Replacement, [LookAt], [SearchOrder],
    /// [MatchCase]: in values and formulas.
    pub(super) fn replace(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let what = self.arg_str(a, 0, "what")?.ok_or_else(Error::invalid_call)?;
        let with = self.arg_str(a, 1, "replacement")?.ok_or_else(Error::invalid_call)?;
        let whole = self.arg_i64(a, 2, "lookat")?.unwrap_or(2) == 1;
        let case = match a.get(4, "matchcase") {
            Some(v) => self.scalar(v)?.to_bool()?,
            None => false,
        };
        if what.is_empty() {
            return Ok(Value::Bool(true));
        }
        let Some(used) = self.wb.clamp_to_used(r.sheet, &r.range) else { return Ok(Value::Bool(true)) };
        if used.rows() as u64 * used.cols() as u64 > MAX_REPLACE {
            return Err(Error::app("The range is too large"));
        }
        let mut edits = Vec::new();
        for row in used.start.row..=used.end.row {
            for col in used.start.col..=used.end.col {
                let c = CellRef::new(row, col);
                let Some(cell) = self.wb.cell(r.sheet, c) else { continue };
                if !matches!(cell.content, Content::Formula { .. } | Content::Value(_)) {
                    continue;
                }
                let text = self.wb.edit_text(r.sheet, c);
                if let Some(new) = replaced(&text, &what, &with, whole, case) {
                    edits.push((c, new));
                }
            }
        }
        if !edits.is_empty() {
            self.via(|wb| wb.set_inputs(r.sheet, edits))?;
        }
        Ok(Value::Bool(true))
    }

    /// Range.AddComment [Text]: a note on its first cell.
    pub(super) fn add_comment(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let c = r.range.start;
        if self.wb.cell(r.sheet, c).is_some_and(|x| x.note().is_some()) {
            return Err(Error::app("Application-defined or object-defined error"));
        }
        let text = self.arg_str(a, 0, "text")?.unwrap_or_default();
        let author = self.user_name.clone();
        self.via(|wb| wb.set_note(r.sheet, c, Some(Note { text, author, ..Note::default() })))?;
        Ok(Value::Object(Obj::Part(Box::new(Part::Comment(r.sheet, c)))))
    }

    /// Range.ClearComments.
    pub(super) fn clear_comments(&mut self, r: RangeRef) -> R<Value> {
        let cells: Vec<CellRef> = self.wb.notes(r.sheet).into_iter().map(|(c, _)| c).filter(|c| r.range.contains(*c)).collect();
        for c in cells {
            self.via(|wb| wb.set_note(r.sheet, c, None))?;
        }
        Ok(Value::Empty)
    }

    /// Range.NoteText [Text]: reads or sets the note of its first cell.
    pub(super) fn note_text(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let c = r.range.start;
        let old = self.wb.cell(r.sheet, c).and_then(|x| x.note().cloned());
        match self.arg_str(a, 0, "text")? {
            None => Ok(Value::str(old.map(|n| n.text).unwrap_or_default())),
            Some(text) => {
                let author = self.user_name.clone();
                let note = Note { text, ..old.unwrap_or(Note { author, ..Note::default() }) };
                self.via(|wb| wb.set_note(r.sheet, c, Some(note)))?;
                Ok(Value::Empty)
            }
        }
    }

    /// Range.PasteSpecial [Paste], [Operation], [SkipBlanks], [Transpose]
    /// of what Copy put aside.
    pub(super) fn paste_special(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let Some((_, cut)) = self.clip else { return Err(Error::app("PasteSpecial method of Range class failed")) };
        let what = match self.arg_i64(a, 0, "paste")?.unwrap_or(-4104) {
            -4104 => PasteWhat::All,
            -4163 | 12 => PasteWhat::Values,
            -4123 | 11 => PasteWhat::Formulas,
            -4122 => PasteWhat::Formats,
            7 => PasteWhat::AllExceptBorders,
            -4144 => PasteWhat::Comments,
            _ => return Err(Error::unsupported("PasteSpecial of that kind")),
        };
        let op = match self.arg_i64(a, 1, "operation")?.unwrap_or(-4142) {
            2 => PasteOp::Add,
            3 => PasteOp::Sub,
            4 => PasteOp::Mul,
            5 => PasteOp::Div,
            _ => PasteOp::None,
        };
        let flag = |it: &mut Self, pos: usize, name: &str| -> R<bool> {
            match a.get(pos, name) {
                Some(v) => it.scalar(v)?.to_bool(),
                None => Ok(false),
            }
        };
        let skip_blanks = flag(self, 2, "skipblanks")?;
        let transpose = flag(self, 3, "transpose")?;
        if cut {
            return Err(Error::app("PasteSpecial method of Range class failed"));
        }
        let clip = self.clip_cells()?;
        let o = PasteSpecial { what, op, skip_blanks, transpose };
        self.via(|wb| wb.paste_special(r.sheet, r.range.start, &clip, o))?;
        Ok(Value::Bool(true))
    }

    /// Worksheet.Paste [Destination]: what Copy or Cut put aside, at the
    /// destination or the selection.
    pub(super) fn sheet_paste(&mut self, s: usize, a: &Args) -> R<Value> {
        let Some((src, cut)) = self.clip else { return Err(Error::app("Paste method of Worksheet class failed")) };
        let to = match a.get(0, "destination") {
            Some(v) => self.range_arg(v, s)?,
            None => RangeRef::new(self.active, self.selection),
        };
        if self.clip_in != self.current_book {
            if cut {
                return Err(Error::unsupported("Cut to another workbook (Copy, then ClearContents)"));
            }
            let clip = self.clip_cells()?;
            return self.paste_clip_adopted(&clip, to).map(|_| Value::Empty);
        }
        self.copy_cells(src, to.sheet, to.range.start)?;
        if cut {
            self.clear_moved(src, to.sheet, to.range.start)?;
            self.clip = None;
        }
        Ok(Value::Empty)
    }

    /// After a Cut: the source cells the destination doesn't cover go.
    pub(super) fn clear_moved(&mut self, src: RangeRef, sheet: usize, to: CellRef) -> R<()> {
        let rg = src.range;
        let dest = Range::new(to, CellRef::new(to.row + rg.rows() - 1, to.col + rg.cols() - 1));
        let mut cells = Vec::new();
        for row in rg.start.row..=rg.end.row {
            for col in rg.start.col..=rg.end.col {
                let c = CellRef::new(row, col);
                if (sheet != src.sheet || !dest.contains(c)) && self.wb.cell(src.sheet, c).is_some() {
                    cells.push((c, None));
                }
            }
        }
        if cells.is_empty() {
            return Ok(());
        }
        self.apply(apex_sheet::Op::SetCells { sheet: src.sheet, cells })
    }
}

/// `text` with `what` replaced (None: not found).
fn replaced(text: &str, what: &str, with: &str, whole: bool, case: bool) -> Option<String> {
    let eq = |a: &str, b: &str| if case { a == b } else { a.to_lowercase() == b.to_lowercase() };
    if whole {
        return eq(text, what).then(|| with.to_string());
    }
    let (hay, needle) = if case { (text.to_string(), what.to_string()) } else { (text.to_lowercase(), what.to_lowercase()) };
    // Lower case may change lengths: then only exact case.
    if hay.len() != text.len() || needle.len() != what.len() {
        return text.contains(what).then(|| text.replace(what, with));
    }
    if !hay.contains(&needle) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(p) = hay[i..].find(&needle) {
        out.push_str(&text[i..i + p]);
        out.push_str(with);
        i += p + needle.len();
    }
    out.push_str(&text[i..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::replaced;

    #[test]
    fn replacing() {
        assert_eq!(replaced("Hello hello", "hello", "bye", false, false).as_deref(), Some("bye bye"));
        assert_eq!(replaced("Hello hello", "hello", "bye", false, true).as_deref(), Some("Hello bye"));
        assert_eq!(replaced("Hello", "hell", "x", true, false), None);
        assert_eq!(replaced("=A1+1", "A1", "B2", false, false).as_deref(), Some("=B2+1"));
    }
}
