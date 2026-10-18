//! The workbook's named cell styles: `Workbook.Styles`, a `Style` with its
//! Font and Interior, and `Range.Style`. Built-in styles go by their
//! English names, as in Excel's code. Edits go through the workbook's, so
//! the cells that use a style follow it and it is one undo step.

use std::sync::Arc;

use apex_sheet::cellstyles::{NamedStyle, part};
use apex_sheet::{CellStyle, Range};

use super::{Args, Interp, font_edit, font_value, halign_code, halign_of, interior_edit, interior_value, obj};
use crate::error::{Error, R};
use crate::object::{Obj, Part, RangeRef};
use crate::value::Value;

/// Styles a macro may add (Excel's limit is about 64000 formats).
const MAX_STYLES: usize = 4000;

fn style(name: &str) -> Value {
    obj(Obj::Part(Box::new(Part::Style(name.to_string()))))
}

fn not_found(name: &str) -> Error {
    Error::app(format!("There is no style named \"{name}\""))
}

/// The flag of an `Include…` property.
fn include_flag(name: &str) -> Option<u8> {
    Some(match name {
        "includenumber" => part::NUMBER,
        "includealignment" => part::ALIGN,
        "includefont" => part::FONT,
        "includeborder" => part::BORDER,
        "includepatterns" => part::FILL,
        "includeprotection" => part::PROTECT,
        _ => return None,
    })
}

impl Interp<'_> {
    fn named_style(&self, name: &str) -> R<NamedStyle> {
        self.wb.cell_styles.get(name).cloned().ok_or_else(|| not_found(name))
    }

    /// `Styles` and its members.
    pub(super) fn styles_member(&mut self, name: &str, a: &Args) -> R<Value> {
        let count = self.wb.cell_styles.list.len();
        match name {
            "count" => Ok(Value::Long(count as i32)),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let v = self.scalar(v)?;
                let found = match v {
                    Value::Str(s) => self.wb.cell_styles.get(&s).map(|n| n.name.to_string()),
                    v => {
                        let i = v.to_i64()?;
                        usize::try_from(i - 1).ok().and_then(|i| self.wb.cell_styles.list.get(i)).map(|n| n.name.to_string())
                    }
                };
                found.map(|n| style(&n)).ok_or_else(Error::subscript)
            }
            "add" => {
                let n = self.arg_str(a, 0, "name")?.ok_or_else(Error::invalid_call)?;
                let n = n.trim().to_string();
                if n.is_empty() || n.chars().count() > 255 {
                    return Err(Error::app("The style name isn't valid"));
                }
                if self.wb.cell_styles.get(&n).is_some() {
                    return Err(Error::app(format!("A style named \"{n}\" already exists")));
                }
                if count >= MAX_STYLES {
                    return Err(Error::app("Too many styles"));
                }
                // The format of the BasedOn range's first cell, else Normal's.
                let base = match a.get(1, "basedon") {
                    Some(Value::Object(Obj::Range(r))) => self.style_of(r),
                    Some(_) => return Err(Error::type_mismatch()),
                    None => self.wb.cell_styles.normal().map(|s| s.style.clone()).unwrap_or_default(),
                };
                let mut base = base;
                base.named = None;
                let def = NamedStyle { name: Arc::from(n.as_str()), builtin: None, parts: part::ALL, style: base };
                self.via(|wb| wb.set_cell_style(None, def))?;
                Ok(style(&n))
            }
            "parent" => Ok(obj(Obj::Workbook)),
            "application" => Ok(obj(Obj::Application)),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn styles_items(&self) -> Vec<Value> {
        self.wb.cell_styles.list.iter().map(|s| style(&s.name)).collect()
    }

    /// A `Style`'s properties and Delete.
    pub(super) fn style_member(&mut self, n: &str, name: &str) -> R<Value> {
        let ns = self.named_style(n)?;
        if let Some(flag) = include_flag(name) {
            return Ok(Value::Bool(ns.parts & flag != 0));
        }
        let s = &ns.style;
        Ok(match name {
            "name" | "namelocal" | "value" | "_default" => Value::str(ns.name.to_string()),
            "builtin" => Value::Bool(ns.builtin.is_some()),
            "numberformat" | "numberformatlocal" => Value::str(if s.num_fmt.is_empty() { "General" } else { &s.num_fmt }),
            "horizontalalignment" => Value::Long(halign_code(s.halign)),
            "wraptext" => Value::Bool(s.wrap),
            "indentlevel" => Value::Long(s.indent as i32),
            "locked" => Value::Bool(!s.unlocked),
            "formulahidden" => Value::Bool(s.formula_hidden),
            "font" => obj(Obj::Part(Box::new(Part::StyleFont(ns.name.to_string())))),
            "interior" => obj(Obj::Part(Box::new(Part::StyleInterior(ns.name.to_string())))),
            "delete" => {
                if ns.is_normal() {
                    return Err(Error::app("The Normal style can't be deleted"));
                }
                self.via(|wb| wb.delete_cell_style(n))?;
                Value::Empty
            }
            "parent" => obj(Obj::Workbook),
            "application" => obj(Obj::Application),
            _ => return Err(Error::no_member(name)),
        })
    }

    /// Changes style `n` with `f`, including the part `flag`; the cells
    /// that use it follow.
    fn edit_style(&mut self, n: &str, flag: u8, f: impl Fn(&mut CellStyle)) -> R<()> {
        let mut def = self.named_style(n)?;
        f(&mut def.style);
        def.parts |= flag;
        let old = def.name.to_string();
        self.via(|wb| wb.set_cell_style(Some(&old), def))
    }

    pub(super) fn style_set(&mut self, n: &str, name: &str, v: Value) -> R<()> {
        if let Some(flag) = include_flag(name) {
            let on = v.to_bool()?;
            let mut def = self.named_style(n)?;
            def.parts = if on { def.parts | flag } else { def.parts & !flag };
            let old = def.name.to_string();
            return self.via(|wb| wb.set_cell_style(Some(&old), def));
        }
        match name {
            "numberformat" | "numberformatlocal" => {
                let code: Arc<str> = Arc::from(v.to_str()?);
                self.edit_style(n, part::NUMBER, |s| s.num_fmt = code.clone())
            }
            "horizontalalignment" => {
                let h = halign_of(v.to_i64()?);
                self.edit_style(n, part::ALIGN, |s| s.halign = h)
            }
            "wraptext" => {
                let w = v.to_bool()?;
                self.edit_style(n, part::ALIGN, |s| s.wrap = w)
            }
            "indentlevel" => {
                let i = v.to_i64()?.clamp(0, 15) as u8;
                self.edit_style(n, part::ALIGN, |s| s.indent = i)
            }
            "locked" => {
                let l = v.to_bool()?;
                self.edit_style(n, part::PROTECT, |s| s.unlocked = !l)
            }
            "formulahidden" => {
                let h = v.to_bool()?;
                self.edit_style(n, part::PROTECT, |s| s.formula_hidden = h)
            }
            "name" | "namelocal" | "builtin" => Err(Error::app(format!("The {name} property of a style can't be set"))),
            _ => Err(Error::no_member(name)),
        }
    }

    /// The Font (`interior` false) or Interior of style `n`.
    pub(super) fn style_part_get(&mut self, n: &str, interior: bool, name: &str) -> R<Value> {
        let ns = self.named_style(n)?;
        match (interior, name) {
            (_, "parent") => Ok(style(&ns.name)),
            (false, _) => font_value(&ns.style, name),
            (true, _) => interior_value(&ns.style, name),
        }
    }

    pub(super) fn style_part_set(&mut self, n: &str, interior: bool, name: &str, v: Value) -> R<()> {
        if interior {
            match interior_edit(name, v)? {
                Some(f) => self.edit_style(n, part::FILL, f),
                None => Ok(()),
            }
        } else {
            let f = font_edit(name, v)?;
            self.edit_style(n, part::FONT, f)
        }
    }

    /// `Range.Style`: the style of its first cell.
    pub(super) fn range_style(&self, r: RangeRef) -> Value {
        let named = self.style_of(r).named;
        let n = named.as_deref().filter(|n| self.wb.cell_styles.get(n).is_some()).map_or_else(|| self.wb.cell_styles.normal().map_or("Normal".to_string(), |s| s.name.to_string()), str::to_string);
        style(&n)
    }

    /// `Range.Style = "Good"`: the style's parts go to the cells.
    pub(super) fn set_range_style(&mut self, r: RangeRef, v: Value) -> R<()> {
        let n = v.to_str()?;
        let ns = self.named_style(&n)?;
        self.check_sheet(r.sheet)?;
        let rg = if r.range.cell_count() > 1_000_000 {
            match self.wb.clamp_to_used(r.sheet, &r.range) {
                Some(x) => x,
                None => return Ok(()),
            }
        } else {
            r.range
        };
        Self::check_size(&rg)?;
        let sheet = r.sheet;
        self.via(|wb| wb.apply_cell_style(sheet, &Range::new(rg.start, rg.end), &ns.name))
    }
}
