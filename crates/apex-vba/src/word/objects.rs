//! More of Word's object model over the document: Fields.Add and the fields
//! and page numbers of headers and footers, tab stops, document Variables,
//! the Shapes of the document (text boxes), TablesOfContents and the
//! Borders of a Table.

use std::rc::Rc;
use std::sync::Arc;

use apex_doc::{Align, Border, Borders, Field, HfSlot, OBJECT, Paragraph, Shape, ShapeKind, TabAlign, TabLeader, TabStop, TabStops, TocOpts};

use super::range::wbool;
use super::{SpanRef, Tg, WObj, WordState, wo};
use crate::error::{Error, R};
use crate::excel::Args;
use crate::interp::Interp;
use crate::object::Obj;
use crate::value::Value;

fn missing() -> Error {
    Error::new(5941, "The requested member of the collection does not exist")
}

fn deleted() -> Error {
    Error::new(5825, "Object has been deleted")
}

fn out_of_range() -> Error {
    Error::new(4608, "Value out of range")
}

/// A 1-based index a macro gives.
fn index(n: i64, count: usize) -> R<usize> {
    if n >= 1 && (n as u64) <= count as u64 { Ok(n as usize - 1) } else { Err(missing()) }
}

/// Most variables a document may hold, and the longest value of one.
const MAX_VARS: usize = 10_000;
const MAX_VAR_LEN: usize = 65_000;
/// Most shapes a macro may add to a document.
const MAX_SHAPES: usize = 10_000;

/// A field of a WdFieldType (and the text after its name, switches included).
fn field_of(ty: i64, text: &str) -> R<Field> {
    let name = match ty {
        -1 => "",
        33 => "PAGE",
        26 => "NUMPAGES",
        31 => "DATE",
        32 => "TIME",
        17 => "AUTHOR",
        15 => "TITLE",
        29 => "FILENAME",
        12 => "SEQ",
        3 => "REF",
        37 => "PAGEREF",
        72 => "NOTEREF",
        85 => "DOCPROPERTY",
        _ => return Err(Error::unsupported("this WdFieldType")),
    };
    if text.len() > 4096 || text.chars().any(char::is_control) {
        return Err(Error::invalid_call());
    }
    Field::parse(&format!("{name} {text}"), "").ok_or_else(Error::invalid_call)
}

fn align_of(n: i64) -> R<Align> {
    Ok(match n {
        0 => Align::Left,
        1 => Align::Center,
        2 => Align::Right,
        3 => Align::Justify,
        _ => return Err(out_of_range()),
    })
}

impl WordState<'_> {
    /// The paragraphs of a header or footer (one empty one when it has none).
    fn hf_paras(&self, i: usize, slot: HfSlot) -> Vec<Paragraph> {
        self.doc.hf_story(i, slot).map(|s| s.to_vec()).unwrap_or_else(|| vec![Paragraph::new("")])
    }

    /// The properties of the table that `m` maps.
    pub(super) fn props_of(&self, m: &apex_doc::TableMap) -> Option<apex_doc::TableProps> {
        let mut c = self.doc.paragraphs.get(m.start)?.cell.as_ref()?;
        for _ in 0..m.depth {
            c = c.nested.as_deref()?;
        }
        c.table_props.as_deref().cloned()
    }
}

impl Interp<'_> {
    // --- Fields -------------------------------------------------------------------------------

    /// Fields.Add(Range, Type, Text, PreserveFormatting) of a part of the text.
    pub(super) fn fields_add(&mut self, a: &Args) -> R<Value> {
        let (s, e) = match a.get(0, "range") {
            Some(v) => self.span_of(&v)?,
            None => self.wspan(&Tg::Sel),
        };
        let ty = self.arg_i64(a, 1, "type")?.unwrap_or(-1);
        let text = self.arg_str(a, 2, "text")?.unwrap_or_default();
        let field = field_of(ty, &text)?;
        let w = self.w();
        let (at, end) = w.mutate(s, e, |d, sel| Some(d.insert_field(sel, field).cursor))?;
        let r = w.new_span(at, end);
        Ok(wo(WObj::Field(r)))
    }

    pub(super) fn field_call(&mut self, r: SpanRef, name: &str) -> R<Value> {
        let (s, e) = self.wspan(&Tg::Range(r));
        match name {
            // Fields are worked out as the document is shown and printed.
            "update" => Ok(Value::Bool(true)),
            "result" | "code" => Ok(self.new_range(s, e)),
            "delete" => {
                self.w().replace(s, e, "")?;
                Ok(Value::Empty)
            }
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::unsupported(&format!("Field.{name}"))),
        }
    }

    /// The fields of a header or footer.
    pub(super) fn hf_fields_call(&mut self, i: usize, slot: HfSlot, name: &str, a: &Args) -> R<Value> {
        match name {
            "count" => {
                let n = self.w().hf_paras(i, slot).iter().map(|p| p.spans().filter(|(_, _, at)| at.field.is_some()).map(|(s, e, _)| p.text[s..e].chars().filter(|&c| c == OBJECT).count()).sum::<usize>()).sum::<usize>();
                Ok(Value::Long(n as i32))
            }
            "update" => Ok(Value::Long(0)),
            // The field goes at the end of the header or footer: its Range
            // has no place of its own to collapse to.
            "add" => {
                let ty = self.arg_i64(a, 1, "type")?.unwrap_or(-1);
                let text = self.arg_str(a, 2, "text")?.unwrap_or_default();
                let field = field_of(ty, &text)?;
                self.hf_push_field(i, slot, field, None)?;
                Ok(Value::Empty)
            }
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::unsupported(&format!("Fields.{name}"))),
        }
    }

    /// PageNumbers of a header or footer: Add([PageNumberAlignment], [FirstPage]).
    pub(super) fn hf_numbers_call(&mut self, i: usize, slot: HfSlot, name: &str, a: &Args) -> R<Value> {
        match name {
            "add" => {
                let al = align_of(self.arg_i64(a, 0, "pagenumberalignment")?.unwrap_or(2))?;
                self.hf_push_field(i, slot, Field::new(apex_doc::FieldKind::Page), Some(al))?;
                Ok(Value::Empty)
            }
            "count" => {
                let n = self.w().hf_paras(i, slot).iter().map(|p| p.spans().filter(|(_, _, at)| at.field.as_ref().is_some_and(|f| f.kind == apex_doc::FieldKind::Page)).count()).sum::<usize>();
                Ok(Value::Long(n as i32))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::unsupported(&format!("PageNumbers.{name}"))),
        }
    }

    /// Puts a field at the end of a header or footer: after its text, or
    /// (with an alignment, a page number) in a paragraph of its own.
    fn hf_push_field(&mut self, i: usize, slot: HfSlot, field: Field, align: Option<Align>) -> R<()> {
        let w = self.w();
        let mut paras = w.hf_paras(i, slot);
        let attrs = apex_doc::Attrs { field: Some(Arc::new(field)), ..Default::default() };
        let empty = paras.last().is_some_and(|p| p.text.is_empty());
        if align.is_some() && !empty {
            let like = paras.last().map(|p| p.style).unwrap_or_default();
            paras.push(Paragraph::new("").with_style(apex_doc::ParaStyle { align: Align::Left, ..like }));
        }
        let last = paras.last_mut().ok_or_else(Error::invalid_call)?;
        if let Some(al) = align {
            last.style.align = al;
        }
        last.push(&OBJECT.to_string(), attrs);
        w.side_edit(|d, sel| d.set_hf_story(i, slot, Some(paras), sel))
    }

    /// ParagraphFormat of a header or footer (its alignment).
    pub(super) fn hf_pformat_get(&mut self, i: usize, slot: HfSlot, name: &str) -> R<Value> {
        match name {
            "alignment" => {
                let paras = self.w().hf_paras(i, slot);
                let al = |p: &Paragraph| match p.style.align {
                    Align::Left => 0,
                    Align::Center => 1,
                    Align::Right => 2,
                    Align::Justify => 3,
                };
                let first = paras.first().map_or(0, al);
                Ok(Value::Long(if paras.iter().all(|p| al(p) == first) { first } else { super::range::UNDEFINED as i32 }))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::unsupported(&format!("ParagraphFormat.{name} of a header or footer"))),
        }
    }

    pub(super) fn hf_pformat_set(&mut self, i: usize, slot: HfSlot, name: &str, v: Value) -> R<()> {
        if name != "alignment" {
            return Err(Error::unsupported(&format!("ParagraphFormat.{name} of a header or footer")));
        }
        let al = align_of(v.to_i64()?)?;
        let w = self.w();
        let mut paras = w.hf_paras(i, slot);
        for p in &mut paras {
            p.style.align = al;
        }
        w.side_edit(|d, sel| d.set_hf_story(i, slot, Some(paras), sel))
    }

    // --- Tab stops -----------------------------------------------------------------------------

    pub(super) fn tabs_call(&mut self, tg: Tg, name: &str, a: &Args) -> R<Value> {
        let (s, e) = self.wspan(&tg);
        let w = self.w();
        let (from, to) = w.paras_of(s, e);
        let stops = w.doc.paragraphs[from].style.tabs;
        match name {
            "count" => Ok(Value::Long(stops.len() as i32)),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let n = self.scalar(v)?.to_i64()?;
                Ok(wo(WObj::Tab(tg, index(n, stops.len())?)))
            }
            "clearall" => {
                let w = self.w();
                w.mutate(s, e, |d, sel| {
                    d.set_para_style(from, to, sel, &|st| st.tabs = TabStops::default());
                    Some(sel.cursor)
                })?;
                Ok(Value::Empty)
            }
            "add" => {
                let pos = a.get(0, "position").ok_or_else(Error::invalid_call)?;
                let pos = self.scalar(pos)?.to_f64()?;
                if !(0.0..=1584.0).contains(&pos) {
                    return Err(out_of_range());
                }
                let align = match self.arg_i64(a, 1, "alignment")?.unwrap_or(0) {
                    0 => TabAlign::Left,
                    1 => TabAlign::Center,
                    2 => TabAlign::Right,
                    3 => TabAlign::Decimal,
                    4 => return Err(Error::unsupported("bar tab stops")),
                    _ => return Err(out_of_range()),
                };
                let leader = match self.arg_i64(a, 2, "leader")?.unwrap_or(0) {
                    0 => TabLeader::None,
                    1 | 5 => TabLeader::Dots,
                    2 => TabLeader::Dashes,
                    3 | 4 => TabLeader::Underline,
                    _ => return Err(out_of_range()),
                };
                let stop = TabStop { pos: pos as f32, align, leader };
                let w = self.w();
                w.mutate(s, e, |d, sel| {
                    d.set_para_style(from, to, sel, &|st| st.tabs.set(stop));
                    Some(sel.cursor)
                })?;
                let k = self.w().doc.paragraphs[from].style.tabs.as_slice().iter().position(|t| (t.pos - stop.pos).abs() < 0.5).ok_or_else(Error::invalid_call)?;
                Ok(wo(WObj::Tab(tg, k)))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn tab_call(&mut self, tg: Tg, i: usize, name: &str) -> R<Value> {
        let (s, e) = self.wspan(&tg);
        let w = self.w();
        let (from, to) = w.paras_of(s, e);
        let stop = *w.doc.paragraphs[from].style.tabs.as_slice().get(i).ok_or_else(deleted)?;
        Ok(match name {
            "position" => Value::Double(stop.pos as f64),
            "alignment" => Value::Long(match stop.align {
                TabAlign::Left => 0,
                TabAlign::Center => 1,
                TabAlign::Right => 2,
                TabAlign::Decimal => 3,
            }),
            "leader" => Value::Long(match stop.leader {
                TabLeader::None => 0,
                TabLeader::Dots => 1,
                TabLeader::Dashes => 2,
                TabLeader::Underline => 3,
            }),
            "customtab" => Value::Bool(true),
            "clear" => {
                w.mutate(s, e, |d, sel| {
                    d.set_para_style(from, to, sel, &|st| st.tabs.remove(stop.pos));
                    Some(sel.cursor)
                })?;
                Value::Empty
            }
            "application" => Value::Object(Obj::Application),
            _ => return Err(Error::no_member(name)),
        })
    }

    // --- Variables -----------------------------------------------------------------------------

    pub(super) fn vars_call(&mut self, name: &str, a: &Args) -> R<Value> {
        match name {
            "count" => Ok(Value::Long(self.w().doc.meta.vars.len() as i32)),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let v = self.scalar(v)?;
                let w = self.w();
                let k = match &v {
                    Value::Str(t) => w.doc.meta.vars.iter().position(|(n, _)| n.eq_ignore_ascii_case(t.trim())).ok_or_else(|| Error::new(5, "Invalid procedure call or argument: there is no such variable"))?,
                    v => index(v.to_i64()?, w.doc.meta.vars.len())?,
                };
                Ok(wo(WObj::Var(Rc::from(w.doc.meta.vars[k].0.as_str()))))
            }
            "add" => {
                let n = self.arg_str(a, 0, "name")?.ok_or_else(Error::invalid_call)?;
                let n = n.trim().to_string();
                let val = match a.get(1, "value") {
                    Some(v) => self.scalar(v)?.to_str()?,
                    None => String::new(),
                };
                if n.is_empty() || n.chars().count() > 255 || n.chars().any(char::is_control) || val.len() > MAX_VAR_LEN {
                    return Err(Error::invalid_call());
                }
                let w = self.w();
                if w.doc.meta.vars.iter().any(|(x, _)| x.eq_ignore_ascii_case(&n)) {
                    return Err(Error::app("A variable with this name already exists"));
                }
                if w.doc.meta.vars.len() >= MAX_VARS {
                    return Err(Error::new(7, "Out of memory: too many variables"));
                }
                w.side_edit(|d, _| d.meta.vars.push((n.clone(), val)))?;
                Ok(wo(WObj::Var(Rc::from(n.as_str()))))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn var_call(&mut self, n: Rc<str>, name: &str) -> R<Value> {
        let w = self.w();
        let k = w.doc.meta.vars.iter().position(|(x, _)| x.eq_ignore_ascii_case(&n)).ok_or_else(deleted)?;
        Ok(match name {
            "value" | "_default" => Value::str(w.doc.meta.vars[k].1.as_str()),
            "name" => Value::str(w.doc.meta.vars[k].0.as_str()),
            "index" => Value::Long(k as i32 + 1),
            "delete" => {
                w.side_edit(|d, _| d.meta.vars.remove(k))?;
                Value::Empty
            }
            "application" => Value::Object(Obj::Application),
            "parent" => wo(WObj::Doc),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn var_set(&mut self, n: Rc<str>, name: &str, v: Value) -> R<()> {
        if name != "value" {
            return Err(Error::no_member(name));
        }
        let val = v.to_str()?;
        if val.len() > MAX_VAR_LEN {
            return Err(Error::invalid_call());
        }
        let w = self.w();
        let k = w.doc.meta.vars.iter().position(|(x, _)| x.eq_ignore_ascii_case(&n)).ok_or_else(deleted)?;
        w.side_edit(|d, _| d.meta.vars[k].1 = val)?;
        Ok(())
    }

    // --- Shapes ---------------------------------------------------------------------------------

    pub(super) fn doc_shapes_call(&mut self, name: &str, a: &Args) -> R<Value> {
        let all = self.w().doc.shapes();
        match name {
            "count" => Ok(Value::Long(all.len() as i32)),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let v = self.scalar(v)?;
                let k = match &v {
                    Value::Str(t) => all.iter().position(|(_, s)| s.shape.name.eq_ignore_ascii_case(t.trim())).ok_or_else(missing)?,
                    v => index(v.to_i64()?, all.len())?,
                };
                Ok(wo(WObj::DocShape(k)))
            }
            "addtextbox" => {
                let [left, top, width, height] = [(1, "left"), (2, "top"), (3, "width"), (4, "height")].map(|(p, n)| a.get(p, n));
                let mut num = |v: Option<Value>| -> R<f32> {
                    let v = v.ok_or_else(Error::invalid_call)?;
                    let x = self.scalar(v)?.to_f64()?;
                    if !(-20_000.0..=20_000.0).contains(&x) {
                        return Err(out_of_range());
                    }
                    Ok(x as f32)
                };
                let (left, top, width, height) = (num(left)?, num(top)?, num(width)?, num(height)?);
                if width < 1.0 || height < 1.0 {
                    return Err(out_of_range());
                }
                if all.len() >= MAX_SHAPES {
                    return Err(Error::new(7, "Out of memory: too many shapes"));
                }
                let at = match a.get(5, "anchor") {
                    Some(v) => self.span_of(&v)?.0,
                    None => 0,
                };
                let mut shape = Shape::new(ShapeKind::TextBox);
                shape.name = format!("Text Box {}", all.len() + 1);
                let mut ds = apex_doc::DocShape::new(shape, width, height);
                (ds.x, ds.y, ds.h_rel, ds.v_rel) = (left, top, apex_doc::HRel::Page, apex_doc::VRel::Page);
                ds.z = all.iter().map(|(_, s)| s.z).max().map_or(0, |z| z + 1);
                ds.sanitize();
                let w = self.w();
                w.mutate(at, at, |d, sel| Some(d.insert_shape(sel, ds).cursor))?;
                // Shapes are in the order of the text.
                let p = w.pos_of(at);
                let k = w.doc.shapes().iter().position(|(q, _)| *q >= p).ok_or_else(deleted)?;
                Ok(wo(WObj::DocShape(k)))
            }
            "addshape" | "addpicture" | "addlabel" | "addline" | "addcanvas" | "addcallout" | "addcurve" | "addpolyline" | "addtexteffect" => Err(Error::unsupported(&format!("Shapes.{name} (only AddTextbox)"))),
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn shape_get(&mut self, k: usize, name: &str) -> R<Value> {
        let (_, ds) = self.w().doc.shapes().into_iter().nth(k).ok_or_else(deleted)?;
        Ok(match name {
            "name" => Value::str(ds.shape.name.as_str()),
            "left" => Value::Double(ds.x as f64),
            "top" => Value::Double(ds.y as f64),
            "width" => Value::Double(ds.width as f64),
            "height" => Value::Double(ds.height as f64),
            "alternativetext" => Value::str(ds.shape.alt.as_str()),
            "type" => Value::Long(if ds.shape.is_text_box() { 17 } else if ds.shape.picture.is_some() { 13 } else { 1 }),
            "visible" => wbool(!ds.shape.hidden),
            "textframe" => wo(WObj::ShapeFrame(k)),
            "delete" => {
                let (p, _) = self.w().doc.shapes().into_iter().nth(k).ok_or_else(deleted)?;
                let w = self.w();
                let c = w.char_of(p);
                w.replace(c, c + 1, "")?;
                Value::Empty
            }
            "application" => Value::Object(Obj::Application),
            "parent" => wo(WObj::Doc),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn shape_put(&mut self, k: usize, name: &str, v: Value) -> R<()> {
        let (p, ds) = self.w().doc.shapes().into_iter().nth(k).ok_or_else(deleted)?;
        let mut ds = (*ds).clone();
        let num = |v: &Value| -> R<f32> {
            let x = v.to_f64()?;
            if !(-20_000.0..=20_000.0).contains(&x) {
                return Err(out_of_range());
            }
            Ok(x as f32)
        };
        match name {
            "left" => ds.x = num(&v)?,
            "top" => ds.y = num(&v)?,
            "width" => ds.width = num(&v)?,
            "height" => ds.height = num(&v)?,
            "name" => {
                let t = v.to_str()?;
                if t.trim().is_empty() || t.chars().count() > 255 || t.chars().any(char::is_control) {
                    return Err(Error::invalid_call());
                }
                ds.shape.name = t.trim().to_string();
            }
            "alternativetext" => ds.shape.alt = v.to_str()?,
            "visible" => ds.shape.hidden = !v.to_bool()?,
            _ => return Err(Error::no_member(name)),
        }
        ds.sanitize();
        self.w().side_edit(|d, _| d.set_shape(p, ds))
    }

    pub(super) fn shape_text_call(&mut self, k: usize, range: bool, name: &str) -> R<Value> {
        let (_, ds) = self.w().doc.shapes().into_iter().nth(k).ok_or_else(deleted)?;
        Ok(match (range, name) {
            (false, "textrange") => wo(WObj::ShapeRange(k)),
            (false, "hastext") => wbool(ds.shape.has_text()),
            (true, "text" | "_default") => Value::str(ds.shape.plain_text().replace('\n', "\r")),
            (_, "application") => Value::Object(Obj::Application),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn shape_text_put(&mut self, k: usize, name: &str, v: Value) -> R<()> {
        if name != "text" {
            return Err(Error::no_member(name));
        }
        let (p, ds) = self.w().doc.shapes().into_iter().nth(k).ok_or_else(deleted)?;
        if !ds.shape.can_have_text() {
            return Err(Error::app("This shape can't hold text"));
        }
        let t = super::clean_input(&v.to_str()?);
        if t.len() > 1 << 20 {
            return Err(Error::new(7, "Out of memory: the text is too long"));
        }
        let mut ds = (*ds).clone();
        ds.shape.set_plain_text(&t);
        self.w().side_edit(|d, _| d.set_shape(p, ds))
    }

    // --- Tables of contents --------------------------------------------------------------------

    pub(super) fn tocs_call(&mut self, name: &str, a: &Args) -> R<Value> {
        let blocks = self.w().doc.toc_blocks();
        match name {
            "count" => Ok(Value::Long(blocks.len() as i32)),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let n = self.scalar(v)?.to_i64()?;
                Ok(wo(WObj::Toc(index(n, blocks.len())?)))
            }
            "add" => {
                let at = match a.get(0, "range") {
                    Some(v) => self.span_of(&v)?.0,
                    None => self.wspan(&Tg::Sel).0,
                };
                let flag = |it: &mut Self, pos: usize, n: &str, d: bool| -> R<bool> {
                    match a.get(pos, n) {
                        Some(v) => it.scalar(v)?.to_bool(),
                        None => Ok(d),
                    }
                };
                if !flag(self, 1, "useheadingstyles", true)? {
                    return Err(Error::unsupported("TablesOfContents.Add without UseHeadingStyles"));
                }
                let upper = self.arg_i64(a, 2, "upperheadinglevel")?.unwrap_or(1);
                let lower = self.arg_i64(a, 3, "lowerheadinglevel")?.unwrap_or(9);
                if !(1..=9).contains(&upper) || !(1..=9).contains(&lower) || upper > lower {
                    return Err(out_of_range());
                }
                if upper != 1 {
                    return Err(Error::unsupported("a table of contents that starts below Heading 1"));
                }
                let right = flag(self, 6, "rightalignpagenumbers", true)?;
                let opts = TocOpts { levels: lower as u8, page_numbers: flag(self, 7, "includepagenumbers", true)?, right_align: right, leader: right, links: flag(self, 9, "usehyperlinks", true)?, ..TocOpts::default() };
                let w = self.w();
                w.mutate(at, at, |d, sel| Some(d.insert_toc(sel, opts, "No table of contents entries found.").cursor))?;
                let p = w.pos_of(at).para;
                let k = w.doc.toc_blocks().iter().position(|b| b.0 >= p).ok_or_else(|| Error::new(4605, "This command isn't available here (in a table?)"))?;
                Ok(wo(WObj::Toc(k)))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn toc_call(&mut self, k: usize, name: &str) -> R<Value> {
        let w = self.w();
        let &(a, b) = w.doc.toc_blocks().get(k).ok_or_else(deleted)?;
        Ok(match name {
            "update" | "updatepagenumbers" => {
                w.side_edit(|d, sel| d.update_toc(a, sel, "No table of contents entries found."))?;
                Value::Empty
            }
            "range" => {
                let (s, e) = (w.para_start(a), w.para_start(b));
                self.new_range(s, e)
            }
            "uppergadinglevel" | "upperheadinglevel" => Value::Long(1),
            "application" => Value::Object(Obj::Application),
            "parent" => wo(WObj::Doc),
            _ => return Err(Error::unsupported(&format!("TableOfContents.{name}"))),
        })
    }

    // --- Table borders and style ---------------------------------------------------------------

    pub(super) fn table_borders_call(&mut self, r: SpanRef, depth: usize, name: &str) -> R<Value> {
        let w = self.w();
        let start = r.borrow().start;
        let p = w.para_of(start);
        let m = w.table_at(p, depth).ok_or_else(deleted)?;
        let props = w.props_of(&m).ok_or_else(deleted)?;
        let mut b = props.style.as_deref().and_then(|id| apex_doc::table_style(id).map(|s| s.borders)).unwrap_or_default();
        b.overlay(&props.borders);
        let line = |x: &[Option<Border>]| Value::Long(if x.iter().all(|s| s.is_some_and(|s| !s.is_none())) { 1 } else { 0 });
        Ok(match name {
            "enable" => wbool([b.top, b.bottom, b.left, b.right, b.inside_h, b.inside_v].iter().all(|s| s.is_some_and(|s| !s.is_none()))),
            "outsidelinestyle" => line(&[b.top, b.bottom, b.left, b.right]),
            "insidelinestyle" => line(&[b.inside_h, b.inside_v]),
            "application" => Value::Object(Obj::Application),
            "parent" => wo(WObj::Table(r, depth)),
            _ => return Err(Error::unsupported(&format!("Borders.{name} of a table"))),
        })
    }

    pub(super) fn table_borders_set(&mut self, r: SpanRef, depth: usize, name: &str, v: Value) -> R<()> {
        // wdLineStyleNone is 0; any other line is drawn as a single one.
        let line = match name {
            "enable" => v.to_bool()?,
            "outsidelinestyle" | "insidelinestyle" => v.to_i64()? != 0,
            _ => return Err(Error::unsupported(&format!("Borders.{name} of a table"))),
        };
        let border = Some(if line { Border::THIN } else { Border::NONE });
        self.set_table_props(&r, depth, &move |p| match name {
            "enable" => p.borders = Borders::all(border.unwrap_or(Border::NONE)),
            "outsidelinestyle" => (p.borders.top, p.borders.bottom, p.borders.left, p.borders.right) = (border, border, border, border),
            _ => (p.borders.inside_h, p.borders.inside_v) = (border, border),
        })
    }

    /// Table.Style: the name of the style of a table.
    pub(super) fn table_style_get(&mut self, r: SpanRef, depth: usize) -> R<Value> {
        let w = self.w();
        let start = r.borrow().start;
        let p = w.para_of(start);
        let m = w.table_at(p, depth).ok_or_else(deleted)?;
        let props = w.props_of(&m).ok_or_else(deleted)?;
        let id = props.style.as_deref().unwrap_or("");
        let name = apex_doc::table_style(id).map(|s| s.name.to_string()).or_else(|| w.doc.styles.tables.iter().find(|s| s.id == id).map(|s| s.name.to_string()));
        Ok(Value::str(name.unwrap_or_else(|| "Table Normal".into())))
    }

    pub(super) fn table_style_set(&mut self, r: SpanRef, depth: usize, v: Value) -> R<()> {
        let n = v.to_str()?;
        let n = n.trim();
        let w = self.w();
        let found = apex_doc::TABLE_STYLES.iter().find(|s| s.name.eq_ignore_ascii_case(n) || s.id.eq_ignore_ascii_case(n)).map(|s| s.id.to_string()).or_else(|| w.doc.styles.tables.iter().find(|s| s.name.eq_ignore_ascii_case(n) || s.id.eq_ignore_ascii_case(n)).map(|s| s.id.to_string()));
        let id: Option<Arc<str>> = match found {
            Some(id) => Some(Arc::from(id.as_str())),
            None if n.eq_ignore_ascii_case("Table Normal") || n.is_empty() => None,
            None => return Err(Error::new(5834, "Item with specified name does not exist")),
        };
        self.set_table_props(&r, depth, &move |p| p.style = id.clone())
    }

    /// Changes the properties of a table as one more undo step.
    pub(super) fn set_table_props(&mut self, r: &SpanRef, depth: usize, f: &dyn Fn(&mut apex_doc::TableProps)) -> R<()> {
        let w = self.w();
        let start = r.borrow().start;
        let p = w.para_of(start);
        let m = w.table_at(p, depth).ok_or_else(deleted)?;
        let c = w.para_start(m.start);
        let at = w.doc_sel(c, c);
        w.side_edit(|d, _| d.set_table_props(at, f))?;
        Ok(())
    }
}
