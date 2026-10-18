//! Range.Characters(Start, Length): part of a cell's text, its Text and
//! Font, kept as the cell's runs of mixed formatting.

use std::sync::Arc;

use apex_sheet::rich::{self, RunFont, Script};
use apex_sheet::{CellRef, CellStyle, Content, Op, Range};

use super::{Args, Interp, PALETTE, obj, range_obj, swap_rb};
use crate::error::{Error, R};
use crate::object::{By, Obj, Part, RangeRef};
use crate::value::Value;

/// The characters a Characters object covers, in bytes of `text`.
struct Span {
    text: String,
    from: usize,
    to: usize,
    /// A text constant (not a number or a formula's result).
    constant: bool,
}

impl Interp<'_> {
    /// Range.Characters([Start], [Length]).
    pub(super) fn characters(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let start = self.arg_i64(a, 0, "start")?.unwrap_or(1);
        let len = self.arg_i64(a, 1, "length")?;
        if start < 1 || len.is_some_and(|n| n < 0) {
            return Err(Error::invalid_call());
        }
        let start = start.min(i64::from(u32::MAX)) as u32;
        let len = len.map(|n| n.min(i64::from(u32::MAX)) as u32);
        Ok(obj(Obj::Part(Box::new(Part::Characters(r.sheet, r.range.start, start, len)))))
    }

    fn span(&self, s: usize, c: CellRef, start: u32, len: Option<u32>) -> R<Span> {
        self.wb.sheets.get(s).ok_or_else(Error::subscript)?;
        let cell = self.wb.cell(s, c);
        let (text, constant) = match cell.map(|x| &x.content) {
            Some(Content::Value(apex_sheet::value::Value::Text(t))) => (t.to_string(), true),
            _ => (self.wb.display(s, c).0.to_string(), false),
        };
        let byte = |i: usize| text.char_indices().nth(i).map_or(text.len(), |(b, _)| b);
        let first = start as usize - 1;
        let from = byte(first);
        let to = len.map_or(text.len(), |n| byte(first.saturating_add(n as usize)));
        Ok(Span { text, from, to, constant })
    }

    pub(super) fn chars_member(&mut self, s: usize, c: CellRef, start: u32, len: Option<u32>, name: &str, a: &Args) -> R<Value> {
        let sp = self.span(s, c, start, len)?;
        Ok(match name {
            "text" | "caption" => Value::str(&sp.text[sp.from..sp.to]),
            "count" => Value::Long(sp.text[sp.from..sp.to].chars().count() as i32),
            "font" => obj(Obj::Part(Box::new(Part::CharsFont(s, c, start, len)))),
            "insert" => {
                let t = self.arg_str(a, 0, "string")?.ok_or_else(|| Error::new(449, "Argument not optional"))?;
                self.chars_replace(s, c, &sp, &t)?;
                Value::Bool(true)
            }
            "delete" => {
                self.chars_replace(s, c, &sp, "")?;
                Value::Bool(true)
            }
            "parent" => range_obj(s, Range::new(c, c)),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn chars_set(&mut self, s: usize, c: CellRef, start: u32, len: Option<u32>, name: &str, v: Value) -> R<()> {
        match name {
            "text" | "caption" => {
                let sp = self.span(s, c, start, len)?;
                self.chars_replace(s, c, &sp, &v.to_str()?)
            }
            _ => Err(Error::no_member(name)),
        }
    }

    /// Puts `with` in place of the span; the rest keeps its formatting.
    fn chars_replace(&mut self, s: usize, c: CellRef, sp: &Span, with: &str) -> R<()> {
        let old = self.wb.cell(s, c).cloned();
        if old.as_ref().is_some_and(|x| x.formula().is_some()) {
            return Err(Error::app("Unable to set the Text property of the Characters class"));
        }
        let new = format!("{}{with}{}", &sp.text[..sp.from], &sp.text[sp.to..]);
        crate::value::string_room(new.len())?;
        let runs = old.as_ref().and_then(|x| x.rich()).and_then(|r| rich::follow_edit(r, &sp.text, &new)).filter(|r| rich::fits(r, &new));
        let content = if new.is_empty() { Content::Empty } else { Content::Value(apex_sheet::value::Value::text(new.as_str())) };
        let mut cell = apex_sheet::Cell::with_content(old.as_ref(), content);
        if let Some(x) = &mut cell {
            x.set_extra(|e| e.rich = runs.map(Arc::new));
        }
        self.apply(Op::SetCells { sheet: s, cells: vec![(c, cell.filter(|x| !x.is_blank()))] })
    }

    /// Characters.Font: a property is Null when the characters differ.
    pub(super) fn chars_font(&mut self, s: usize, c: CellRef, start: u32, len: Option<u32>, name: &str) -> R<Value> {
        let sp = self.span(s, c, start, len)?;
        let base = self.wb.styles.get(self.wb.cell(s, c).map_or(0, |x| x.style)).clone();
        let runs = if sp.constant { self.wb.cell(s, c).and_then(|x| x.rich()).cloned().unwrap_or_default() } else { Vec::new() };
        let mut fonts: Vec<RunFont> = Vec::new();
        let mut at = 0;
        for r in &runs {
            if at < sp.to && sp.from < at + r.len {
                fonts.push(r.font.clone());
            }
            at += r.len;
        }
        if fonts.is_empty() {
            fonts.push(rich::font_at(&runs, sp.from));
        }
        let mut out: Option<Value> = None;
        for f in &fonts {
            let v = font_prop(&base, f, name)?;
            match &out {
                Some(o) if !same(o, &v) => return Ok(Value::Null),
                Some(_) => {}
                None => out = Some(v),
            }
        }
        Ok(out.unwrap_or(Value::Null))
    }

    pub(super) fn chars_font_set(&mut self, s: usize, c: CellRef, start: u32, len: Option<u32>, name: &str, v: Value) -> R<()> {
        let sp = self.span(s, c, start, len)?;
        if !sp.constant {
            // A number or a formula's result is formatted whole.
            return self.font_set(RangeRef { sheet: s, range: Range::new(c, c), by: By::Cells }, name, v);
        }
        let change: Box<dyn Fn(&mut RunFont)> = match name {
            "bold" => {
                let b = v.to_bool()?;
                Box::new(move |f| f.bold = Some(b))
            }
            "italic" => {
                let b = v.to_bool()?;
                Box::new(move |f| f.italic = Some(b))
            }
            "strikethrough" => {
                let b = v.to_bool()?;
                Box::new(move |f| f.strike = Some(b))
            }
            "underline" => {
                let b = match v {
                    Value::Bool(b) => b,
                    v => !matches!(v.to_i64()?, -4142 | 0),
                };
                Box::new(move |f| f.underline = Some(b))
            }
            "size" => {
                let pt = v.to_f64()?;
                if !(1.0..=409.0).contains(&pt) {
                    return Err(Error::app("Unable to set the Size property of the Font class"));
                }
                Box::new(move |f| f.size = Some(pt as f32))
            }
            "name" => {
                let n: Arc<str> = Arc::from(v.to_str()?);
                Box::new(move |f| f.font = Some(n.clone()))
            }
            "color" => {
                let col = swap_rb(v.to_i64()? as u32 & 0xFF_FFFF);
                Box::new(move |f| f.color = Some(col))
            }
            "colorindex" => {
                let i = v.to_i64()?;
                let col = match i {
                    -4105 | -4142 | 0 => None,
                    1..=56 => Some(PALETTE[i as usize - 1]),
                    _ => return Err(Error::app("Unable to set the ColorIndex property of the Font class")),
                };
                Box::new(move |f| f.color = col)
            }
            "superscript" | "subscript" => {
                let on = v.to_bool()?;
                let want = if name == "superscript" { Script::Super } else { Script::Sub };
                Box::new(move |f| {
                    if on {
                        f.script = want;
                    } else if f.script == want {
                        f.script = Script::Normal;
                    }
                })
            }
            _ => return Err(Error::no_member(name)),
        };
        let Some(mut cell) = self.wb.cell(s, c).cloned() else { return Ok(()) };
        let runs = cell.rich().cloned().unwrap_or_default();
        let runs = rich::apply(&runs, sp.text.len(), sp.from, sp.to, change);
        cell.set_extra(|e| e.rich = runs.map(Arc::new));
        self.apply(Op::SetCells { sheet: s, cells: vec![(c, Some(cell))] })
    }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Long(x), Value::Long(y)) => x == y,
        (Value::Double(x), Value::Double(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        _ => false,
    }
}

/// A Font property of characters with run font `f` in a cell styled `base`.
fn font_prop(base: &CellStyle, f: &RunFont, name: &str) -> R<Value> {
    let color = f.color.or(base.color);
    Ok(match name {
        "bold" => Value::Bool(f.bold.unwrap_or(base.bold)),
        "italic" => Value::Bool(f.italic.unwrap_or(base.italic)),
        "strikethrough" => Value::Bool(f.strike.unwrap_or(base.strike)),
        "underline" => Value::Long(if f.underline.unwrap_or(base.underline) { 2 } else { -4142 }),
        "size" => Value::Double(match f.size {
            Some(pt) => f64::from(pt),
            None if base.font_size == 0 => 11.0,
            None => f64::from(base.font_size) / 10.0,
        }),
        "name" => Value::str(f.font.as_deref().or(base.font.as_deref()).unwrap_or("Calibri")),
        "color" => Value::Long(swap_rb(color.unwrap_or(0)) as i32),
        "colorindex" => Value::Long(match color {
            None => -4105,
            Some(c) => PALETTE.iter().position(|&p| p == c).map_or(-4105, |i| i as i32 + 1),
        }),
        "superscript" => Value::Bool(f.script == Script::Super),
        "subscript" => Value::Bool(f.script == Script::Sub),
        _ => return Err(Error::no_member(name)),
    })
}
