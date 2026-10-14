//! Sizes and formatting of a Word table's rows, columns and cells: Width,
//! Height, HeightRule, AllowBreakAcrossPages, HeadingFormat, Alignment and
//! LeftIndent of rows and columns, and the VerticalAlignment and Shading
//! of cells.

use apex_doc::{Align, TableMap, TableProps, VAlign};

use super::range::{AUTO, color_index, from_bgr, index_color, to_bgr, wbool};
use super::{SpanRef, WObj, wo};
use crate::error::{Error, R};
use crate::interp::Interp;
use crate::object::Obj;
use crate::value::Value;

/// wdUndefined: rows or columns that differ.
const UNDEFINED: i32 = 9_999_999;
/// Widest column, highest row and deepest indent a macro may set, in points.
const MAX_POINTS: f64 = 1584.0;

fn out_of_range() -> Error {
    Error::new(4608, "Value out of range")
}

fn deleted() -> Error {
    Error::new(5825, "Object has been deleted")
}

fn points(v: &Value) -> R<f32> {
    let x = v.to_f64()?;
    if !(0.0..=MAX_POINTS).contains(&x) {
        return Err(out_of_range());
    }
    Ok(x as f32)
}

/// A property of row (`rows`) or column `i` as a number, or None if it
/// isn't one of those we know.
fn line_value(p: &TableProps, rows: bool, i: usize, name: &str) -> Option<f64> {
    let height = p.row_heights.get(i).copied().unwrap_or(0.0);
    let rule = p.row_rules.get(i).copied().unwrap_or_default();
    Some(match (rows, name) {
        (false, "width") => p.widths.get(i).copied().unwrap_or(0.0) as f64,
        (true, "height") => height as f64,
        // wdRowHeightAuto, wdRowHeightAtLeast, wdRowHeightExactly.
        (true, "heightrule") => {
            if rule.exact {
                2.0
            } else if height > 0.0 {
                1.0
            } else {
                0.0
            }
        }
        (true, "allowbreakacrosspages") => (!rule.cant_split) as u8 as f64,
        (true, "headingformat") => (i < p.header_rows as usize) as u8 as f64,
        _ => return None,
    })
}

fn line_out(name: &str, x: f64) -> Value {
    match name {
        "width" | "height" => Value::Double(x),
        "allowbreakacrosspages" | "headingformat" => wbool(x != 0.0),
        _ => Value::Long(x as i32),
    }
}

/// Row height lists as long as the table.
fn grow_rows(p: &mut TableProps, n: usize) {
    if p.row_heights.len() < n {
        p.row_heights.resize(n, 0.0);
    }
    if p.row_rules.len() < n {
        p.row_rules.resize(n, Default::default());
    }
}

impl Interp<'_> {
    fn table_now(&mut self, r: &SpanRef, depth: usize) -> R<(TableMap, TableProps)> {
        let w = self.w();
        let m = w.table(r, depth)?;
        let p = w.props_of(&m).ok_or_else(deleted)?;
        Ok((m, p))
    }

    /// A size or format of row or column `i` (`None`: of all of them, the
    /// Rows or Columns collection).
    pub(super) fn line_format_get(&mut self, r: SpanRef, depth: usize, rows: bool, i: Option<usize>, name: &str) -> R<Value> {
        let (m, p) = self.table_now(&r, depth)?;
        if rows && i.is_none() {
            match name {
                // wdAlignRowLeft, wdAlignRowCenter, wdAlignRowRight.
                "alignment" => return Ok(Value::Long(match p.align {
                    Align::Center => 1,
                    Align::Right => 2,
                    _ => 0,
                })),
                "leftindent" => return Ok(Value::Double(p.indent as f64)),
                _ => {}
            }
        }
        let n = if rows { m.rows } else { m.cols };
        let mut seen: Option<f64> = None;
        for k in i.map_or(0..n, |i| i..i + 1) {
            let Some(x) = line_value(&p, rows, k, name) else {
                return Err(Error::unsupported(&format!("{}.{name}", line_type(rows, i))));
            };
            match seen {
                Some(y) if y != x => return Ok(Value::Long(UNDEFINED)),
                _ => seen = Some(x),
            }
        }
        Ok(seen.map_or(Value::Long(UNDEFINED), |x| line_out(name, x)))
    }

    /// Sets a size or format of row or column `i` (`None`: of all of them).
    pub(super) fn line_format_set(&mut self, r: SpanRef, depth: usize, rows: bool, i: Option<usize>, name: &str, v: Value) -> R<()> {
        let (m, _) = self.table_now(&r, depth)?;
        let n = if rows { m.rows } else { m.cols };
        let (from, to) = i.map_or((0, n), |i| (i, i + 1));
        let all = i.is_none();
        let f: Box<dyn Fn(&mut TableProps)> = match (rows, name) {
            (false, "width") => {
                let x = points(&v)?;
                Box::new(move |p| {
                    for w in p.widths.iter_mut().take(to).skip(from) {
                        *w = x;
                    }
                })
            }
            (true, "height") => {
                let x = points(&v)?;
                Box::new(move |p| {
                    grow_rows(p, n);
                    p.row_heights[from..to].fill(x);
                })
            }
            (true, "heightrule") => {
                let rule = v.to_i64()?;
                if !(0..=2).contains(&rule) {
                    return Err(out_of_range());
                }
                Box::new(move |p| {
                    grow_rows(p, n);
                    for k in from..to {
                        p.row_rules[k].exact = rule == 2;
                        if rule == 0 {
                            p.row_heights[k] = 0.0;
                        }
                    }
                })
            }
            (true, "allowbreakacrosspages") => {
                let b = v.to_bool()?;
                Box::new(move |p| {
                    grow_rows(p, n);
                    for k in from..to {
                        p.row_rules[k].cant_split = !b;
                    }
                })
            }
            // Heading rows are the first ones of the table: turning one on
            // turns on those above it, turning one off those below it.
            (true, "headingformat") => {
                let b = v.to_bool()?;
                Box::new(move |p| {
                    let h = p.header_rows as usize;
                    p.header_rows = match (all, b) {
                        (true, true) => n,
                        (true, false) => 0,
                        (false, true) => h.max(to),
                        (false, false) => h.min(from),
                    } as u32;
                })
            }
            (true, "alignment") if all => {
                let a = match v.to_i64()? {
                    0 => Align::Left,
                    1 => Align::Center,
                    2 => Align::Right,
                    _ => return Err(out_of_range()),
                };
                Box::new(move |p| p.align = a)
            }
            (true, "leftindent") if all => {
                let x = v.to_f64()?;
                if !(-MAX_POINTS..=MAX_POINTS).contains(&x) {
                    return Err(out_of_range());
                }
                Box::new(move |p| p.indent = x as f32)
            }
            _ => return Err(Error::no_member(&format!("{name} of {}", line_type(rows, i)))),
        };
        self.set_table_props(&r, depth, &*f)
    }

    /// Width, Height, HeightRule, VerticalAlignment and Shading of a cell.
    pub(super) fn cell_format_get(&mut self, r: SpanRef, depth: usize, row: usize, col: usize, name: &str) -> R<Value> {
        let (m, p) = self.table_now(&r, depth)?;
        let span = m.span.get(row * m.cols + col).copied().unwrap_or((1, 1));
        match name {
            "width" => Ok(Value::Double(p.widths.iter().skip(col).take(span.1.max(1)).sum::<f32>() as f64)),
            "height" | "heightrule" => self.line_format_get(r, depth, true, Some(row), name),
            "verticalalignment" => {
                let w = self.w();
                let v = m.cell_props(&w.doc.paragraphs, row, col).map(|c| c.valign).unwrap_or_default();
                // wdCellAlignVerticalTop, Center and Bottom.
                Ok(Value::Long(match v {
                    VAlign::Top => 0,
                    VAlign::Center => 1,
                    VAlign::Bottom => 3,
                }))
            }
            "shading" => Ok(wo(WObj::CellShading(r, depth, row, col))),
            _ => Err(Error::unsupported(&format!("Cell.{name}"))),
        }
    }

    pub(super) fn cell_format_set(&mut self, r: SpanRef, depth: usize, row: usize, col: usize, name: &str, v: Value) -> R<()> {
        let (m, p) = self.table_now(&r, depth)?;
        let span = m.span.get(row * m.cols + col).copied().unwrap_or((1, 1)).1.max(1);
        match name {
            // A merged cell's columns keep their proportions.
            "width" => {
                let x = points(&v)?;
                let old: f32 = p.widths.iter().skip(col).take(span).sum();
                self.set_table_props(&r, depth, &move |p| {
                    let n = p.widths.len().min(col + span);
                    for w in &mut p.widths[col.min(n)..n] {
                        *w = if old > 0.0 { *w * x / old } else { x / span as f32 };
                    }
                })
            }
            "height" | "heightrule" => self.line_format_set(r, depth, true, Some(row), name, v),
            "verticalalignment" => {
                let a = match v.to_i64()? {
                    0 => VAlign::Top,
                    1 => VAlign::Center,
                    3 => VAlign::Bottom,
                    _ => return Err(out_of_range()),
                };
                self.set_cell(&m, row, col, &move |c| c.valign = a)
            }
            _ => Err(Error::no_member(&format!("{name} of Cell"))),
        }
    }

    /// Cell.Shading.
    pub(super) fn cell_shading_get(&mut self, r: SpanRef, depth: usize, row: usize, col: usize, name: &str) -> R<Value> {
        let m = self.w().table(&r, depth)?;
        let w = self.w();
        let fill = m.cell_props(&w.doc.paragraphs, row, col).and_then(|c| c.shading);
        Ok(match name {
            "backgroundpatterncolor" => Value::Long(fill.map_or(AUTO as i32, to_bgr)),
            "backgroundpatterncolorindex" => Value::Long(fill.map_or(0, color_index)),
            "foregroundpatterncolor" => Value::Long(AUTO as i32),
            "foregroundpatterncolorindex" => Value::Long(0),
            // wdTextureNone: the fill is solid.
            "texture" => Value::Long(0),
            "application" => Value::Object(Obj::Application),
            "parent" => wo(WObj::Cell(r, depth, row, col)),
            _ => return Err(Error::unsupported(&format!("Shading.{name} of a cell"))),
        })
    }

    pub(super) fn cell_shading_set(&mut self, r: SpanRef, depth: usize, row: usize, col: usize, name: &str, v: Value) -> R<()> {
        let fill = match name {
            "backgroundpatterncolor" => from_bgr(v.to_i64()?)?,
            "backgroundpatterncolorindex" => index_color(v.to_i64()?)?,
            "texture" if v.to_i64()? == 0 => return Ok(()),
            _ => return Err(Error::unsupported(&format!("Shading.{name} of a cell"))),
        };
        let m = self.w().table(&r, depth)?;
        self.set_cell(&m, row, col, &move |c| c.shading = fill)
    }

    /// Changes one cell's properties as one more undo step.
    fn set_cell(&mut self, m: &TableMap, row: usize, col: usize, f: &dyn Fn(&mut apex_doc::CellProps)) -> R<()> {
        let k = row * m.cols + col;
        let (a, _) = *m.cells.get(k).ok_or_else(deleted)?;
        let w = self.w();
        let c = w.para_start(a);
        let at = w.doc_sel(c, c);
        w.side_edit(|d, _| d.set_cell_props(at, &|p, _| f(p)))?;
        Ok(())
    }
}

fn line_type(rows: bool, i: Option<usize>) -> &'static str {
    match (rows, i) {
        (true, Some(_)) => "Row",
        (false, Some(_)) => "Column",
        (true, None) => "Rows",
        (false, None) => "Columns",
    }
}
