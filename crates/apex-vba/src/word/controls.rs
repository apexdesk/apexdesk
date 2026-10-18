//! Content controls: ContentControls of a part of the text (or those with
//! a title or tag), a ContentControl with its state, and the entries of a
//! list control. Legacy form fields aren't content controls here, as in
//! Word.

use std::rc::Rc;
use std::sync::Arc;

use apex_doc::{CHECKED, ContentControl, ControlAt, ControlKind, ListItem, Pos, UNCHECKED};

use super::{SpanRef, Tg, WObj, clean_input, wo};
use crate::error::{Error, R};
use crate::excel::Args;
use crate::interp::Interp;
use crate::object::Obj;
use crate::value::Value;

/// Entries a list control may have after a macro adds some.
const MAX_ENTRIES: usize = 10_000;
/// Longest title, tag or entry a macro may give.
const MAX_LABEL: usize = 4096;

fn missing() -> Error {
    Error::new(5941, "The requested member of the collection does not exist")
}

fn deleted() -> Error {
    Error::new(5825, "Object has been deleted")
}

fn locked() -> Error {
    Error::new(6124, "You are not allowed to edit this selection because it is protected")
}

fn label(s: String) -> R<String> {
    let s = clean_input(&s).replace(['\n', '\u{2028}'], " ");
    if s.chars().count() > MAX_LABEL { Err(Error::new(5, "Invalid procedure call or argument: the text is too long")) } else { Ok(s) }
}

/// WdContentControlType of a control.
fn kind_type(k: &ControlKind) -> i32 {
    match k {
        ControlKind::RichText => 0,
        ControlKind::PlainText => 1,
        ControlKind::List { editable: true, .. } => 3,
        ControlKind::List { editable: false, .. } => 4,
        ControlKind::Date { .. } => 6,
        ControlKind::CheckBox { .. } => 8,
    }
}

/// The placeholder Word shows in a new control of this kind.
fn placeholder(k: &ControlKind) -> &'static str {
    match k {
        ControlKind::List { .. } => "Choose an item.",
        ControlKind::Date { .. } => "Click or tap to enter a date.",
        _ => "Click or tap here to enter text.",
    }
}

/// A control as the macro sees it: where it is, in characters.
struct Found {
    at: ControlAt,
    start: usize,
    end: usize,
}

impl Interp<'_> {
    /// The document's content controls (not form fields), in order, with
    /// where each is.
    fn all_controls(&mut self) -> Vec<Found> {
        let w = self.w();
        let list: Vec<ControlAt> = w.doc.controls().into_iter().filter(|c| !c.control.is_form()).collect();
        list.into_iter()
            .map(|at| {
                let start = w.char_of(Pos::new(at.para, at.range.start));
                let end = w.char_of(Pos::new(at.para, at.range.end));
                Found { at, start, end }
            })
            .collect()
    }

    /// The controls of `r` (those in it, or the one it is in), or those of
    /// the document with a title (false) or tag (true).
    fn cc_of(&mut self, r: &SpanRef, by: &Option<(bool, Rc<str>)>) -> Vec<Found> {
        let (s, e) = self.wspan(&Tg::Range(r.clone()));
        self.all_controls()
            .into_iter()
            .filter(|f| match by {
                Some((tag, v)) => (if *tag { &f.at.control.tag } else { &f.at.control.title }) == &**v,
                None => (f.start >= s && f.end <= e && f.start < f.end) || (f.start <= s && e <= f.end),
            })
            .collect()
    }

    fn control(&mut self, id: u32) -> R<Found> {
        self.all_controls().into_iter().find(|f| f.at.control.id == id).ok_or_else(deleted)
    }

    /// Gives control `id` a new state and text (`None`: the same text).
    fn set_control(&mut self, id: u32, f: impl FnOnce(&mut ContentControl), text: Option<String>) -> R<()> {
        let found = self.control(id)?;
        let mut c = (*found.at.control).clone();
        f(&mut c);
        let at = found.at.clone();
        let text = text.unwrap_or_else(|| self.w().doc.paragraphs[at.para].text[at.range.clone()].to_string());
        self.w().mutate(found.start, found.end, move |d, sel| Some(d.set_control(&at, c, &text, sel)))?;
        Ok(())
    }

    pub(crate) fn controls_call(&mut self, r: SpanRef, by: Option<(bool, Rc<str>)>, name: &str, a: &Args) -> R<Value> {
        match name {
            "count" => Ok(Value::Long(self.cc_of(&r, &by).len() as i32)),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let list = self.cc_of(&r, &by);
                let f = match self.scalar(v)? {
                    // Word's ID is a string of digits.
                    Value::Str(s) => list.iter().find(|f| f.at.control.id.to_string() == *s).ok_or_else(missing)?,
                    v => {
                        let n = v.to_i64()?;
                        if n < 1 || n as u64 > list.len() as u64 {
                            return Err(missing());
                        }
                        &list[n as usize - 1]
                    }
                };
                Ok(wo(WObj::Control(f.at.control.id)))
            }
            "add" if by.is_none() => {
                let ty = self.arg_i64(a, 0, "type")?.unwrap_or(0);
                let kind = match ty {
                    0 => ControlKind::RichText,
                    1 => ControlKind::PlainText,
                    3 => ControlKind::List { items: Vec::new(), editable: true },
                    4 => ControlKind::List { items: Vec::new(), editable: false },
                    6 => ControlKind::Date { date: None, format: String::new() },
                    8 => ControlKind::CheckBox { checked: false },
                    2 | 5 | 7 | 9 => return Err(Error::unsupported("picture, building block, group and repeating section content controls")),
                    _ => return Err(Error::invalid_call()),
                };
                let (s, e) = match a.get(1, "range") {
                    Some(v) if !matches!(v, Value::Missing) => self.span_of(&v)?,
                    _ => self.w().sel(),
                };
                // Not in another one, nor over a paragraph mark.
                if self.all_controls().iter().any(|f| f.start < e.max(s + 1) && s < f.end) {
                    return Err(Error::new(6124, "You can't put a content control inside another one here"));
                }
                let w = self.w();
                let (a0, b0) = (w.pos_of(s), w.pos_of(e));
                if a0.para != b0.para {
                    return Err(Error::new(4605, "A content control can't span paragraphs here"));
                }
                let id = w.doc.next_control_id();
                let (text, ph) = match &kind {
                    ControlKind::CheckBox { .. } => (UNCHECKED.to_string(), false),
                    k => (placeholder(k).to_string(), true),
                };
                // Text controls wrap what the range has.
                let wraps = s < e && matches!(kind, ControlKind::RichText | ControlKind::PlainText);
                let c = ContentControl { placeholder: ph && !wraps, ..ContentControl::new(id, kind) };
                w.mutate(s, e, |d, sel| Some(d.insert_control(sel, c, &text).cursor))?;
                Ok(wo(WObj::Control(id)))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(crate) fn control_call(&mut self, id: u32, name: &str, a: &Args) -> R<Value> {
        let f = self.control(id)?;
        let c = f.at.control.clone();
        Ok(match name {
            "id" => Value::str(c.id.to_string()),
            "title" => Value::str(c.title.as_str()),
            "tag" => Value::str(c.tag.as_str()),
            "type" => Value::Long(kind_type(&c.kind)),
            "range" => self.new_range(f.start, f.end),
            "showingplaceholdertext" => Value::Bool(c.placeholder),
            "lockcontents" => Value::Bool(c.lock_content),
            "lockcontentcontrol" => Value::Bool(c.lock_delete),
            "temporary" => Value::Bool(false),
            "checked" => match c.kind {
                ControlKind::CheckBox { checked } => Value::Bool(checked),
                _ => return Err(Error::new(4605, "This content control isn't a check box")),
            },
            "dropdownlistentries" => match c.kind {
                ControlKind::List { .. } => return self.collection(WObj::Entries(id), a),
                _ => return Err(Error::new(4605, "This content control isn't a list")),
            },
            "datedisplayformat" => match &c.kind {
                ControlKind::Date { format, .. } => Value::str(if format.is_empty() { "dd/MM/yyyy" } else { format }),
                _ => return Err(Error::new(4605, "This content control isn't a date")),
            },
            "setplaceholdertext" => {
                let t = match a.get(2, "text") {
                    Some(v) => self.scalar(v)?.to_str()?,
                    None => return Err(Error::unsupported("SetPlaceholderText without Text")),
                };
                let t = label(t)?;
                // Only shown while it has no text of its own.
                if c.placeholder && !t.is_empty() {
                    self.set_control(id, |_| {}, Some(t))?;
                }
                Value::Empty
            }
            "delete" => {
                let contents = match a.get(0, "deletecontents") {
                    Some(v) => self.scalar(v)?.to_bool()?,
                    None => false,
                };
                if c.lock_delete || (contents && c.lock_content) {
                    return Err(locked());
                }
                let at = f.at.clone();
                let w = self.w();
                if contents || c.placeholder {
                    w.mutate(f.start, f.end, |d, sel| Some(d.delete(sel)))?;
                } else {
                    w.mutate(f.start, f.end, |d, sel| Some(d.remove_control(&at, sel)))?;
                }
                Value::Empty
            }
            "select" => {
                let w = self.w();
                w.anchor = f.start;
                w.active = f.end;
                Value::Empty
            }
            "application" => Value::Object(Obj::Application),
            "parent" => wo(WObj::Doc),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(crate) fn cc_set(&mut self, id: u32, name: &str, v: Value) -> R<()> {
        let v = self.scalar(v)?;
        let c = self.control(id)?.at.control;
        match name {
            "title" => {
                let t = label(v.to_str()?)?;
                self.set_control(id, |c| c.title = t, None)
            }
            "tag" => {
                let t = label(v.to_str()?)?;
                self.set_control(id, |c| c.tag = t, None)
            }
            "lockcontents" => {
                let b = v.to_bool()?;
                self.set_control(id, |c| c.lock_content = b, None)
            }
            "lockcontentcontrol" => {
                let b = v.to_bool()?;
                self.set_control(id, |c| c.lock_delete = b, None)
            }
            "checked" => {
                let b = v.to_bool()?;
                if !matches!(c.kind, ControlKind::CheckBox { .. }) {
                    return Err(Error::new(4605, "This content control isn't a check box"));
                }
                if c.lock_content {
                    return Err(locked());
                }
                self.set_control(id, |c| c.kind = ControlKind::CheckBox { checked: b }, Some((if b { CHECKED } else { UNCHECKED }).to_string()))
            }
            "datedisplayformat" => {
                let t = label(v.to_str()?)?;
                let ControlKind::Date { date, .. } = &c.kind else { return Err(Error::new(4605, "This content control isn't a date")) };
                let date = date.clone();
                self.set_control(id, |c| c.kind = ControlKind::Date { date, format: t }, None)
            }
            _ => Err(Error::no_member(name)),
        }
    }

    /// The entries of list control `id`.
    fn entries(&mut self, id: u32) -> R<(Arc<ContentControl>, Vec<ListItem>)> {
        let c = self.control(id)?.at.control;
        match &c.kind {
            ControlKind::List { items, .. } => {
                let items = items.clone();
                Ok((c, items))
            }
            _ => Err(deleted()),
        }
    }

    /// Changes the entries of list control `id`.
    fn set_entries(&mut self, id: u32, f: impl FnOnce(&mut Vec<ListItem>) -> R<()>) -> R<()> {
        let (c, mut items) = self.entries(id)?;
        f(&mut items)?;
        let editable = matches!(c.kind, ControlKind::List { editable: true, .. });
        self.set_control(id, |c| c.kind = ControlKind::List { items, editable }, None)
    }

    pub(crate) fn entries_call(&mut self, id: u32, name: &str, a: &Args) -> R<Value> {
        let (_, items) = self.entries(id)?;
        match name {
            "count" => Ok(Value::Long(items.len() as i32)),
            "item" | "_default" => {
                let n = self.arg_i64(a, 0, "index")?.ok_or_else(Error::invalid_call)?;
                if n < 1 || n as u64 > items.len() as u64 {
                    return Err(missing());
                }
                Ok(wo(WObj::Entry(id, n as usize - 1)))
            }
            "add" => {
                let text = label(self.arg_str(a, 0, "text")?.ok_or_else(Error::invalid_call)?)?;
                let value = match self.arg_str(a, 1, "value")? {
                    Some(v) => label(v)?,
                    None => text.clone(),
                };
                let at = self.arg_i64(a, 2, "index")?;
                if text.is_empty() || items.iter().any(|i| i.text == text) {
                    return Err(Error::new(6137, "The entry's text is empty or already in the list"));
                }
                if items.len() >= MAX_ENTRIES {
                    return Err(Error::new(7, "Out of memory: too many entries"));
                }
                let k = match at {
                    Some(n) if n >= 1 => (n as usize - 1).min(items.len()),
                    Some(_) => return Err(Error::invalid_call()),
                    None => items.len(),
                };
                self.set_entries(id, |v| {
                    v.insert(k, ListItem { text, value });
                    Ok(())
                })?;
                Ok(wo(WObj::Entry(id, k)))
            }
            "clear" => {
                self.set_entries(id, |v| {
                    v.clear();
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Control(id))),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(crate) fn entry_call(&mut self, id: u32, k: usize, name: &str) -> R<Value> {
        let (c, items) = self.entries(id)?;
        let item = items.get(k).cloned().ok_or_else(deleted)?;
        Ok(match name {
            "text" | "_default" => Value::str(item.text),
            "value" => Value::str(item.value),
            "index" => Value::Long(k as i32 + 1),
            "delete" => {
                self.set_entries(id, |v| {
                    v.remove(k);
                    Ok(())
                })?;
                Value::Empty
            }
            "select" => {
                if c.lock_content {
                    return Err(locked());
                }
                self.set_control(id, |c| c.placeholder = false, Some(item.text))?;
                Value::Empty
            }
            "moveup" | "movedown" => {
                let to = if name == "moveup" { k.checked_sub(1) } else { Some(k + 1).filter(|&j| j < items.len()) };
                if let Some(j) = to {
                    self.set_entries(id, |v| {
                        v.swap(k, j);
                        Ok(())
                    })?;
                }
                Value::Empty
            }
            "application" => Value::Object(Obj::Application),
            "parent" => wo(WObj::Control(id)),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(crate) fn entry_set(&mut self, id: u32, k: usize, name: &str, v: Value) -> R<()> {
        let t = label(self.scalar(v)?.to_str()?)?;
        let (_, items) = self.entries(id)?;
        if k >= items.len() {
            return Err(deleted());
        }
        match name {
            "text" => {
                if t.is_empty() || items.iter().enumerate().any(|(i, x)| i != k && x.text == t) {
                    return Err(Error::new(6137, "The entry's text is empty or already in the list"));
                }
                self.set_entries(id, |v| {
                    v[k].text = t;
                    Ok(())
                })
            }
            "value" => self.set_entries(id, |v| {
                v[k].value = t;
                Ok(())
            }),
            _ => Err(Error::no_member(name)),
        }
    }
}
