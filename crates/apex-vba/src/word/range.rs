//! Range and Selection: their text and where they are, moving and
//! extending them, editing through them, Characters, Words, Sentences and
//! Paragraphs, and their Font, ParagraphFormat and ListFormat.

use std::rc::Rc;
use std::sync::Arc;

use apex_doc::{Align, Attrs, BreakKind, CharFormat, CharFx, LineRule, ParaKind, ParaStyle, Paragraph, SectionStart};

use super::{SpanRef, Tg, Unit, WObj, WordState, clean_input, wo};
use crate::error::{Error, R};
use crate::excel::Args;
use crate::interp::Interp;
use crate::object::Obj;
use crate::value::Value;

/// wdUndefined and wdToggle.
pub const UNDEFINED: i64 = 9_999_999;
pub const TOGGLE: i64 = 9_999_998;
/// wdColorAutomatic.
pub(crate) const AUTO: i64 = -16_777_216;
/// wdForward: as far as there is.
const FORWARD: i64 = 1_073_741_823;
/// Most items For Each goes through.
pub const MAX_ITEMS: usize = 1_000_000;

/// WdColorIndex 1 to 16 (black, blue, turquoise, bright green, pink, red,
/// yellow, white, dark blue, teal, green, violet, dark red, dark yellow,
/// gray 50%, gray 25%), as 0xRRGGBB.
const INDEX: [u32; 16] = [0x000000, 0x0000FF, 0x00FFFF, 0x00FF00, 0xFF00FF, 0xFF0000, 0xFFFF00, 0xFFFFFF, 0x000080, 0x008080, 0x008000, 0x800080, 0x800000, 0x808000, 0x808080, 0xC0C0C0];

/// VBA colors are 0xBBGGRR; the document keeps 0xRRGGBB.
pub fn to_bgr(c: u32) -> i32 {
    (((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)) as i32
}

/// A color a macro gives (None: automatic).
pub fn from_bgr(v: i64) -> R<Option<u32>> {
    if v == AUTO {
        return Ok(None);
    }
    if !(0..=0xFF_FFFF).contains(&v) {
        return Err(Error::new(5843, "One of the values passed to this method or property is out of range"));
    }
    let c = v as u32;
    Ok(Some(((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)))
}

/// Points as half points (Word's sizes: 1 to 1638).
pub fn half_points(pt: f64) -> R<u16> {
    if !(1.0..=1638.0).contains(&pt) {
        return Err(Error::new(4608, "Value out of range"));
    }
    Ok((pt * 2.0).round() as u16)
}

fn out_of_range() -> Error {
    Error::new(4608, "Value out of range")
}

/// A WdColorIndex (0: none).
pub(super) fn index_color(n: i64) -> R<Option<u32>> {
    match n {
        0 => Ok(None),
        1..=16 => Ok(Some(INDEX[n as usize - 1])),
        _ => Err(Error::new(5843, "One of the values passed to this method or property is out of range")),
    }
}

/// The WdColorIndex nearest a color.
pub(super) fn color_index(c: u32) -> i32 {
    let d = |a: u32, b: u32| (0..3).map(|k| (((a >> (8 * k)) & 0xFF) as i32 - ((b >> (8 * k)) & 0xFF) as i32).pow(2)).sum::<i32>();
    INDEX.iter().enumerate().min_by_key(|(_, x)| d(**x, c)).map_or(0, |(i, _)| i as i32 + 1)
}

/// Word's True and False (Long).
pub fn wbool(b: bool) -> Value {
    Value::Long(if b { -1 } else { 0 })
}

fn eqv(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Long(x), Value::Long(y)) => x == y,
        (Value::Double(x), Value::Double(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        _ => false,
    }
}

/// The effects of `a`, changed by `f`.
fn with_fx(a: &mut Attrs, f: impl FnOnce(&mut CharFx)) {
    let mut fx = a.fx.as_deref().cloned().unwrap_or_default();
    f(&mut fx);
    a.fx = (!fx.is_default()).then(|| Arc::new(fx));
}

/// Built-in style names, in English and in the languages Word is used
/// in most here (Spanish, German, French, Italian, Portuguese, Dutch).
fn builtin_style(name: &str) -> Option<ParaKind> {
    let n = name.trim().to_lowercase();
    let n = n.as_str();
    let heading = ["heading ", "título ", "titulo ", "überschrift ", "titre ", "titolo ", "kop "].iter().find_map(|p| n.strip_prefix(p)?.trim().parse::<u8>().ok()).filter(|k| (1..=9).contains(k));
    if let Some(k) = heading {
        return Some(ParaKind::Heading(k));
    }
    Some(match n {
        "normal" | "standard" | "standaard" | "normale" => ParaKind::Normal,
        "title" | "título" | "titulo" | "titel" | "titre" | "titolo" => ParaKind::Title,
        "quote" | "cita" | "zitat" | "citation" | "citazione" | "citação" | "citacao" | "citaat" => ParaKind::Quote,
        "code" => ParaKind::Code,
        "list bullet" => ParaKind::Bullet,
        "list number" => ParaKind::Numbered,
        _ => return None,
    })
}

impl WordState<'_> {
    /// The name of a paragraph style.
    pub fn style_name(&self, kind: ParaKind, custom: u16) -> String {
        if let Some(d) = self.doc.styles.custom(custom) {
            return d.name.to_string();
        }
        match kind {
            ParaKind::Normal => "Normal".into(),
            ParaKind::Heading(n) => format!("Heading {n}"),
            ParaKind::Title => "Title".into(),
            ParaKind::Quote => "Quote".into(),
            ParaKind::Code => "Code".into(),
            ParaKind::Bullet => "List Bullet".into(),
            ParaKind::Numbered => "List Number".into(),
        }
    }

    /// The style a macro names (a name, a WdBuiltinStyle number or a
    /// Style): a paragraph style (kind, custom number, false) or a
    /// character style (its number in the styles, true).
    pub fn style_of(&self, v: &Value) -> R<Option<(ParaKind, u16, bool)>> {
        let name = match v {
            Value::Object(Obj::Word(o)) => match &**o {
                WObj::Style(n) => n.to_string(),
                _ => return Err(Error::type_mismatch()),
            },
            Value::Long(_) | Value::Int(_) | Value::Double(_) => {
                let k = match v.to_i64()? {
                    -1 => ParaKind::Normal,
                    n @ -10..=-2 => ParaKind::Heading((-n - 1) as u8),
                    -63 => ParaKind::Title,
                    -181 => ParaKind::Quote,
                    -49 => ParaKind::Bullet,
                    -50 => ParaKind::Numbered,
                    _ => return Ok(None),
                };
                return Ok(Some((k, 0, false)));
            }
            v => v.to_str()?,
        };
        let styles = &self.doc.styles;
        if let Some(i) = styles.defs.iter().position(|d| d.name.eq_ignore_ascii_case(name.trim()) || d.id.eq_ignore_ascii_case(name.trim())) {
            let d = &styles.defs[i];
            if d.char_style {
                return Ok(Some((ParaKind::Normal, i as u16 + 1, true)));
            }
            if !d.builtin {
                return Ok(Some((d.kind, i as u16 + 1, false)));
            }
        }
        if let Some(k) = builtin_style(&name) {
            return Ok(Some((k, 0, false)));
        }
        Ok(styles.para_style_of(name.trim()).map(|(k, c)| (k, c, false)))
    }

    /// Calls `f` with the formatting of each piece of `s..e` (of the
    /// caret for an empty one).
    fn each_run(&mut self, s: usize, e: usize, f: &mut dyn FnMut(&CharFormat, &Attrs)) {
        let (a, b) = (self.pos_of(s), self.pos_of(e));
        let doc = &*self.doc;
        if s >= e {
            let p = &doc.paragraphs[a.para];
            let at = match &self.typing {
                Some((c, at)) if *c == s => at.clone(),
                _ => p.attrs_at(a.offset),
            };
            f(&doc.char_format(&p.style, &at), &at);
            return;
        }
        for i in a.para..=b.para {
            let p = &doc.paragraphs[i];
            let from = if i == a.para { a.offset } else { 0 };
            let to = if i == b.para { b.offset } else { p.text.len() };
            let mut any = false;
            for (rs, re, at) in p.spans() {
                if re > from && rs < to {
                    f(&doc.char_format(&p.style, at), at);
                    any = true;
                }
            }
            if !any && i < b.para {
                let at = p.attrs_at(p.text.len());
                f(&doc.char_format(&p.style, &at), &at);
            }
        }
    }

    /// The paragraphs `s..e` is in.
    pub fn paras_of(&mut self, s: usize, e: usize) -> (usize, usize) {
        let from = self.para_of(s);
        let to = if e > s { self.para_of(e - 1) } else { from };
        (from, to.max(from))
    }

    /// The first character from `c` (going back from it, the one before)
    /// for which `stop` holds, at most `max` characters away: where it is.
    fn scan(&mut self, c: usize, forward: bool, max: usize, stop: &dyn Fn(char) -> bool) -> Option<usize> {
        let len = self.story_len();
        if forward {
            let mut p = self.para_of(c.min(len.saturating_sub(1)));
            let mut at = c;
            while at < len && at - c < max {
                let base = self.para_start(p);
                let mut cs: Vec<char> = self.doc.paragraphs[p].text.chars().collect();
                cs.push('\r');
                for (k, &ch) in cs.iter().enumerate().skip(at - base) {
                    if at - c >= max {
                        return None;
                    }
                    if stop(ch) {
                        return Some(base + k);
                    }
                    at += 1;
                }
                p += 1;
                if p >= self.doc.paragraphs.len() {
                    break;
                }
            }
            None
        } else {
            if c == 0 {
                return None;
            }
            let mut p = self.para_of(c - 1);
            let mut at = c;
            loop {
                let base = self.para_start(p);
                let mut cs: Vec<char> = self.doc.paragraphs[p].text.chars().collect();
                cs.push('\r');
                while at > base {
                    if c - at >= max {
                        return None;
                    }
                    if stop(cs[at - 1 - base]) {
                        return Some(at - 1);
                    }
                    at -= 1;
                }
                if p == 0 {
                    return None;
                }
                p -= 1;
            }
        }
    }
}

/// What a Delete, Move... is given: the unit and how many.
fn unit_arg(it: &mut Interp, a: &Args, pos: usize, default: i64) -> R<Unit> {
    Unit::of(it.arg_i64(a, pos, "unit")?.unwrap_or(default))
}

impl<'a> Interp<'a> {
    pub(crate) fn w(&mut self) -> &mut WordState<'a> {
        self.word.as_mut().expect("Word's objects are only made with a document")
    }

    /// Where a Range or the Selection is.
    pub(crate) fn wspan(&mut self, tg: &Tg) -> (usize, usize) {
        let len = self.w().story_len();
        let (s, e) = match tg {
            Tg::Sel => self.w().sel(),
            Tg::Range(r) => {
                let r = r.borrow();
                (r.start, r.end)
            }
        };
        let s = s.min(len);
        (s, e.min(len).max(s))
    }

    pub(crate) fn set_span(&mut self, tg: &Tg, s: usize, e: usize) {
        let len = self.w().story_len();
        let (s, e) = (s.min(len), e.min(len));
        let (s, e) = (s.min(e), s.max(e));
        match tg {
            Tg::Sel => {
                let w = self.w();
                w.anchor = s;
                w.active = e;
            }
            Tg::Range(r) => {
                let mut r = r.borrow_mut();
                r.start = s;
                r.end = e;
            }
        }
    }

    pub(crate) fn new_range(&mut self, s: usize, e: usize) -> Value {
        let r = self.w().new_span(s, e);
        wo(WObj::Range(r))
    }

    /// The last place a caret can be (before the last paragraph mark).
    fn last(&mut self) -> usize {
        self.w().story_len().saturating_sub(1)
    }

    /// A Range (or the Selection, a Paragraph) a macro gives.
    pub(crate) fn span_of(&mut self, v: &Value) -> R<(usize, usize)> {
        match v {
            Value::Object(Obj::Word(o)) => match &**o {
                WObj::Range(r) | WObj::Para(r) => Ok(self.wspan(&Tg::Range(r.clone()))),
                WObj::Sel => Ok(self.wspan(&Tg::Sel)),
                _ => Err(Error::type_mismatch()),
            },
            Value::Nothing => Err(Error::not_set()),
            _ => Err(Error::type_mismatch()),
        }
    }

    /// Text with formatting a macro gives (FormattedText).
    fn fragment_of(&mut self, v: &Value) -> R<Rc<Vec<Paragraph>>> {
        if let Value::Object(Obj::Word(o)) = v
            && let WObj::Fragment(f) = &**o
        {
            return Ok(f.clone());
        }
        let (s, e) = self.span_of(v)?;
        Ok(Rc::new(self.wcopy_of(s, e)))
    }

    fn wcopy_of(&mut self, s: usize, e: usize) -> Vec<Paragraph> {
        let w = self.w();
        let (a, b) = (w.pos_of(s), w.pos_of(e));
        if a == b { Vec::new() } else { w.doc.slice(a, b) }
    }

    /// Steps `count` units from `c` (back for a negative one): where it
    /// gets and how many it went.
    fn steps(&mut self, mut c: usize, unit: Unit, count: i64) -> R<(usize, i64)> {
        let mut moved = 0;
        for _ in 0..count.unsigned_abs() {
            match self.w().step(c, unit, count > 0)? {
                Some(x) => {
                    c = x;
                    moved += 1;
                }
                None => break,
            }
        }
        Ok((c, if count < 0 { -moved } else { moved }))
    }

    // --- Range and Selection ---------------------------------------------------------------

    /// What Ranges and the Selection have.
    pub(crate) fn text_call(&mut self, tg: Tg, name: &str, a: &Args) -> R<Value> {
        let (s, e) = self.wspan(&tg);
        let len = self.w().story_len();
        let int = |it: &mut Self, pos: usize, n: &str, d: i64| -> R<i64> { Ok(it.arg_i64(a, pos, n)?.unwrap_or(d)) };
        Ok(match name {
            "text" => Value::str(self.w().text(s, e)?),
            "start" => Value::Long(s as i32),
            "end" => Value::Long(e as i32),
            "storylength" => Value::Long(len as i32),
            "storytype" => Value::Long(1),
            "duplicate" | "range" => self.new_range(s, e),
            "formattedtext" => wo(WObj::Fragment(Rc::new(self.wcopy_of(s, e)))),
            "font" => wo(WObj::Font(tg)),
            "paragraphformat" | "format" => wo(WObj::ParaFormat(tg)),
            "find" => wo(WObj::Find(tg)),
            "listformat" => wo(WObj::ListFormat(tg)),
            "characters" | "words" | "sentences" | "paragraphs" => {
                let unit = match name {
                    "characters" => Unit::Char,
                    "words" => Unit::Word,
                    "sentences" => Unit::Sentence,
                    _ => Unit::Para,
                };
                let r = self.w().new_span(s, e);
                return self.collection(WObj::Units(r, unit), a);
            }
            "tables" | "hyperlinks" | "inlineshapes" | "revisions" | "fields" => {
                let r = self.w().new_span(s, e);
                let o = match name {
                    "tables" => WObj::Tables(r),
                    "hyperlinks" => WObj::Hyperlinks(r),
                    "inlineshapes" => WObj::InlineShapes(r),
                    "revisions" => WObj::Revisions(r),
                    _ => WObj::Fields(r),
                };
                return self.collection(o, a);
            }
            "contentcontrols" => {
                let r = self.w().new_span(s, e);
                return self.collection(WObj::Controls(r, None), a);
            }
            "sections" => {
                let Some(n) = int(self, 0, "index", 0).ok().filter(|&n| n >= 1) else {
                    return Err(Error::unsupported("Sections of a Range without an index (ActiveDocument.Sections has them all)"));
                };
                let w = self.w();
                let (pa, pb) = w.paras_of(s, e);
                let (sa, sb) = (w.doc.section_index(pa), w.doc.section_index(pb));
                let k = sa + n as usize - 1;
                if k > sb {
                    return Err(Error::new(5941, "The requested member of the collection does not exist"));
                }
                wo(WObj::Section(k))
            }
            "cells" | "rows" | "columns" => return Err(Error::unsupported(&format!("{name} of a Range or the Selection (use Tables(1).{name})"))),
            "bold" | "italic" | "underline" => self.wfont_get(tg, name)?,
            "highlightcolorindex" => {
                let w = self.w();
                let mut out: Option<Value> = None;
                let mut mixed = false;
                w.each_run(s, e, &mut |_, at| {
                    let v = Value::Long(at.highlight.map_or(0, color_index));
                    match &out {
                        None => out = Some(v),
                        Some(o) if !eqv(o, &v) => mixed = true,
                        _ => {}
                    }
                });
                if mixed { Value::Long(UNDEFINED as i32) } else { out.unwrap_or(Value::Long(0)) }
            }
            "style" => {
                let w = self.w();
                let (pa, pb) = w.paras_of(s, e);
                let first = w.doc.paragraphs[pa].style;
                if (pa..=pb).all(|i| w.doc.paragraphs[i].style.kind == first.kind && w.doc.paragraphs[i].style.custom == first.custom) {
                    wo(WObj::Style(w.style_name(first.kind, first.custom).into()))
                } else {
                    Value::Nothing
                }
            }
            "case" => {
                let t = self.w().text(s, e)?;
                let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
                Value::Long(if letters.iter().all(|c| !c.is_lowercase()) && !letters.is_empty() {
                    1
                } else if letters.iter().all(|c| !c.is_uppercase()) {
                    0
                } else {
                    UNDEFINED as i32
                })
            }
            "information" => {
                let what = self.arg_i64(a, 0, "type")?.ok_or_else(Error::invalid_call)?;
                return self.information(s, e, what);
            }
            "computestatistics" => {
                let what = self.arg_i64(a, 0, "statistic")?.ok_or_else(Error::invalid_call)?;
                return self.statistics(s, e, what);
            }
            "insertafter" | "insertbefore" => {
                let t = clean_input(&self.arg_str(a, 0, "text")?.ok_or_else(Error::invalid_call)?);
                self.insert_at(&tg, s, e, &t, name == "insertafter")?;
                Value::Empty
            }
            "insertparagraphafter" => {
                self.insert_at(&tg, s, e, "\n", true)?;
                Value::Empty
            }
            "insertparagraphbefore" => {
                self.insert_at(&tg, s, e, "\n", false)?;
                Value::Empty
            }
            "insertparagraph" => {
                let (at, end) = self.w().replace(s, e, "\n")?;
                self.set_span(&tg, at, end);
                Value::Empty
            }
            "insertsymbol" => {
                let n = self.arg_i64(a, 0, "characternumber")?.ok_or_else(Error::invalid_call)?;
                let c = u32::try_from(n & 0xFFFF_FFFF).ok().and_then(char::from_u32).filter(|c| !c.is_control()).ok_or_else(Error::invalid_call)?;
                let (at, end) = self.w().replace(s, e, &c.to_string())?;
                self.set_span(&tg, at, end);
                Value::Empty
            }
            "insertbreak" => {
                let kind = int(self, 0, "type", 7)?;
                self.insert_break(&tg, s, e, kind)?;
                Value::Empty
            }
            "insertfile" => return Err(Error::unsupported("InsertFile")),
            "delete" => {
                let unit = unit_arg(self, a, 0, 1)?;
                let count = int(self, 1, "count", 1)?;
                return self.delete(&tg, s, e, unit, count);
            }
            "collapse" => {
                let c = if int(self, 0, "direction", 1)? == 0 { e.min(len - 1).max(s) } else { s };
                self.set_span(&tg, c, c);
                Value::Empty
            }
            "expand" => {
                let unit = unit_arg(self, a, 0, 2)?;
                let w = self.w();
                let (ns, _) = w.unit_at(s, unit)?;
                let (_, ne) = w.unit_at(if e > s { e - 1 } else { s }, unit)?;
                self.set_span(&tg, ns, ne);
                Value::Long(((ne as i64 - ns as i64) - (e - s) as i64) as i32)
            }
            "move" => {
                let unit = unit_arg(self, a, 0, 1)?;
                let count = int(self, 1, "count", 1)?;
                if count == 0 {
                    return Ok(Value::Long(0));
                }
                let (from, extra) = if s < e { (if count > 0 { e } else { s }, 1) } else { (s, 0) };
                let rest = if extra > 0 { count - count.signum() } else { count };
                let (c, moved) = self.steps(from, unit, rest)?;
                let c = c.min(self.last());
                self.set_span(&tg, c, c);
                Value::Long((moved + extra * count.signum()) as i32)
            }
            "movestart" | "moveend" => {
                let unit = unit_arg(self, a, 0, 1)?;
                let count = int(self, 1, "count", 1)?;
                let start = name == "movestart";
                let (c, moved) = self.steps(if start { s } else { e }, unit, count)?;
                let (ns, ne) = if start { (c, e.max(c)) } else { (s.min(c), c) };
                self.set_span(&tg, ns, ne);
                Value::Long(moved as i32)
            }
            "startof" | "endof" => {
                let unit = unit_arg(self, a, 0, 2)?;
                let extend = int(self, 1, "extend", 0)? == 1;
                let w = self.w();
                if name == "startof" {
                    let (u, _) = w.unit_at(s, unit)?;
                    self.set_span(&tg, u, if extend { e } else { u });
                    Value::Long(u as i32 - s as i32)
                } else {
                    let (_, u) = w.unit_at(if e > s { e - 1 } else { e }, unit)?;
                    let u = if extend { u } else { u.min(len - 1) };
                    self.set_span(&tg, if extend { s } else { u }, u);
                    Value::Long(u as i32 - e as i32)
                }
            }
            "setrange" => {
                let ns = self.arg_i64(a, 0, "start")?.ok_or_else(Error::invalid_call)?.clamp(0, len as i64) as usize;
                let ne = self.arg_i64(a, 1, "end")?.ok_or_else(Error::invalid_call)?.clamp(0, len as i64) as usize;
                self.set_span(&tg, ns, ne);
                Value::Empty
            }
            "moveuntil" | "movestartuntil" | "moveenduntil" | "movewhile" | "movestartwhile" | "moveendwhile" => {
                let cset: Vec<char> = self.arg_str(a, 0, "cset")?.ok_or_else(Error::invalid_call)?.chars().collect();
                let count = int(self, 1, "count", FORWARD)?;
                return self.move_chars(&tg, s, e, name, &cset, count);
            }
            "next" | "previous" => {
                let unit = unit_arg(self, a, 0, 2)?;
                let count = int(self, 1, "count", 1)?.max(1);
                let w = self.w();
                let mut c = if name == "next" { e } else { s };
                for _ in 1..count {
                    match w.step(c, unit, name == "next")? {
                        Some(x) => c = x,
                        None => return Ok(Value::Nothing),
                    }
                }
                if name == "next" {
                    if c >= len {
                        return Ok(Value::Nothing);
                    }
                    let (us, ue) = w.unit_at(c, unit)?;
                    self.new_range(us, ue)
                } else {
                    if c == 0 {
                        return Ok(Value::Nothing);
                    }
                    let (us, ue) = w.unit_at(c - 1, unit)?;
                    self.new_range(us, ue)
                }
            }
            "select" => {
                let w = self.w();
                w.anchor = s;
                w.active = e;
                Value::Empty
            }
            "isequal" | "inrange" => {
                let (os, oe) = self.span_of(&a.get(0, "range").ok_or_else(Error::invalid_call)?)?;
                Value::Bool(if name == "isequal" { (s, e) == (os, oe) } else { os <= s && e <= oe })
            }
            "instory" => Value::Bool(true),
            "copy" | "cut" => {
                if s < e {
                    let c = self.wcopy_of(s, e);
                    self.w().clip = Some(Rc::new(c));
                    if name == "cut" {
                        let (at, _) = self.w().replace(s, e, "")?;
                        self.set_span(&tg, at, at);
                    }
                }
                Value::Empty
            }
            "paste" | "pasteandformat" => {
                let Some(clip) = self.w().clip.clone() else {
                    return Err(Error::new(4605, "There is nothing to paste: macros paste only what they copied"));
                };
                let (at, end) = self.put_fragment(s, e, &clip)?;
                if matches!(tg, Tg::Sel) {
                    self.set_span(&tg, end, end);
                } else {
                    self.set_span(&tg, at, end);
                }
                Value::Empty
            }
            "pastespecial" => return Err(Error::unsupported("PasteSpecial")),
            "goto" => {
                let (gs, ge) = self.goto(s, a)?;
                self.new_range(gs, ge)
            }
            "application" => Value::Object(Obj::Application),
            "parent" | "document" => wo(WObj::Doc),
            "isendofrowmark" => Value::Bool(false),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(crate) fn text_set(&mut self, tg: Tg, name: &str, v: Value) -> R<()> {
        let (s, e) = self.wspan(&tg);
        let len = self.w().story_len();
        match name {
            "text" => {
                let t = clean_input(&self.scalar(v)?.to_str()?);
                let (at, end) = self.w().replace(s, e, &t)?;
                self.set_span(&tg, at, end);
            }
            "start" | "end" => {
                let n = self.scalar(v)?.to_i64()?.clamp(0, len as i64) as usize;
                if name == "start" {
                    self.set_span(&tg, n, e.max(n));
                } else {
                    self.set_span(&tg, s.min(n), n);
                }
            }
            "bold" | "italic" | "underline" => self.wfont_set(tg, name, v)?,
            "highlightcolorindex" => {
                let c = index_color(self.scalar(v)?.to_i64()?)?;
                self.format_chars(&tg, &move |a| a.highlight = c)?;
            }
            "style" => {
                let v = match v {
                    v @ Value::Object(_) => v,
                    v => self.scalar(v)?,
                };
                self.set_style(&tg, &v)?;
            }
            "case" => {
                let n = self.scalar(v)?.to_i64()?;
                let f: fn(&str) -> String = match n {
                    0 => |t| t.to_lowercase(),
                    1 => |t| t.to_uppercase(),
                    2 => title_case,
                    4 => sentence_case,
                    5 => |t| t.chars().flat_map(|c| if c.is_uppercase() { c.to_lowercase().collect::<Vec<_>>() } else { c.to_uppercase().collect() }).collect(),
                    _ => return Err(Error::unsupported("this WdCharacterCase")),
                };
                if s < e {
                    self.w().mutate(s, e, |d, sel| Some(d.map_text(sel, &f).end()))?;
                }
            }
            "formattedtext" => {
                let frag = self.fragment_of(&v)?;
                let (at, end) = self.put_fragment(s, e, &frag)?;
                self.set_span(&tg, at, end);
            }
            _ => return Err(Error::no_member(name)),
        }
        Ok(())
    }

    /// Puts copied text in place of `s..e`: where it is.
    fn put_fragment(&mut self, s: usize, e: usize, frag: &[Paragraph]) -> R<(usize, usize)> {
        if frag.iter().map(|p| p.text.len() + 1).sum::<usize>().saturating_add(self.w().story_len()) > super::MAX_STORY {
            return Err(Error::new(7, "Out of memory: the document is too long"));
        }
        let e = self.w().keep_marks(s, e);
        self.w().mutate(s, e, |d, sel| Some(if frag.is_empty() { d.delete(sel) } else { d.insert_paragraphs(sel, frag) }))
    }

    /// InsertAfter (`after`) or InsertBefore: the Range grows to take
    /// the text.
    fn insert_at(&mut self, tg: &Tg, s: usize, e: usize, t: &str, after: bool) -> R<()> {
        let at = if after { e } else { s };
        let attrs = match (tg, &self.w().typing) {
            (Tg::Sel, Some((c, a))) if *c == at && s == e => Some(a.clone()),
            _ => None,
        };
        let (_, end) = self.w().replace_as(at, at, t, attrs)?;
        let (ns, ne) = self.wspan(tg);
        if after { self.set_span(tg, ns.min(s), end.max(ne)) } else { self.set_span(tg, s, ne) }
        Ok(())
    }

    /// InsertBreak: WdBreakType 7 page, 8 column, 6 and 11 a line break,
    /// 2 to 5 sections.
    fn insert_break(&mut self, tg: &Tg, s: usize, e: usize, kind: i64) -> R<()> {
        let k = match kind {
            7 => BreakKind::Page,
            8 => BreakKind::Column,
            2 => BreakKind::Section(SectionStart::NextPage),
            3 => BreakKind::Section(SectionStart::Continuous),
            4 => BreakKind::Section(SectionStart::EvenPage),
            5 => BreakKind::Section(SectionStart::OddPage),
            6 | 11 => {
                let (_, end) = self.w().replace(s, e, &apex_doc::LINE_BREAK.to_string())?;
                self.set_span(tg, end, end);
                return Ok(());
            }
            _ => return Err(Error::unsupported("this WdBreakType")),
        };
        let at = if s < e { self.w().replace(s, e, "")?.0 } else { s };
        let (_, end) = self.w().mutate(at, at, |d, sel| d.insert_break(sel, k))?;
        self.set_span(tg, end, end);
        Ok(())
    }

    /// Delete: the Range, else `count` units from it.
    fn delete(&mut self, tg: &Tg, s: usize, e: usize, unit: Unit, count: i64) -> R<Value> {
        if s < e {
            let (at, _) = self.w().replace(s, e, "")?;
            self.set_span(tg, at, at);
            return Ok(Value::Long(1));
        }
        if count == 0 {
            return Ok(Value::Long(0));
        }
        let (c, moved) = self.steps(s, unit, count)?;
        let (a, b) = (s.min(c), s.max(c));
        if a == b {
            return Ok(Value::Long(0));
        }
        let (at, _) = self.w().replace(a, b, "")?;
        self.set_span(tg, at, at);
        Ok(Value::Long(moved.abs() as i32))
    }

    /// MoveUntil, MoveWhile and their Start and End forms.
    fn move_chars(&mut self, tg: &Tg, s: usize, e: usize, name: &str, cset: &[char], count: i64) -> R<Value> {
        let until = name.ends_with("until");
        let stop = |c: char| cset.contains(&c) == until;
        let forward = count > 0;
        let max = count.unsigned_abs() as usize;
        let which = if name.starts_with("movestart") {
            0
        } else if name.starts_with("moveend") {
            1
        } else {
            2
        };
        let from = match which {
            0 => s,
            1 => e,
            _ if forward => e,
            _ => s,
        };
        let len = self.w().story_len();
        let found = self.w().scan(from, forward, max, &stop);
        let to = match found {
            // Going back, the range ends after the character found.
            Some(k) => if forward { k } else { k + 1 },
            // MoveUntil doesn't move; MoveWhile goes as far as it may.
            None if until => return Ok(Value::Long(0)),
            None => if forward { from.saturating_add(max).min(len) } else { from.saturating_sub(max) },
        };
        match which {
            0 => self.set_span(tg, to, e.max(to)),
            1 => self.set_span(tg, s.min(to), to),
            _ => self.set_span(tg, to, to),
        }
        Ok(Value::Long(to as i32 - from as i32))
    }

    /// Information: whether it's in a table and where (no page layout here).
    fn information(&mut self, s: usize, e: usize, what: i64) -> R<Value> {
        let w = self.w();
        let (pa, pb) = w.paras_of(s, e);
        let cell = |w: &mut WordState, p: usize| -> Option<(usize, usize, usize, usize)> {
            let m = w.doc.table_map(p)?;
            let k = m.cells.iter().position(|&(a, b)| a <= p && p < b)?;
            Some((k / m.cols.max(1) + 1, k % m.cols.max(1) + 1, m.rows, m.cols))
        };
        Ok(match what {
            12 => Value::Bool(w.doc.in_table(pa)),
            13 | 16 | 15 | 18 => match cell(w, pa) {
                Some((r, c, rows, cols)) => Value::Long(match what {
                    13 => r,
                    16 => c,
                    15 => rows,
                    _ => cols,
                } as i32),
                None => Value::Long(-1),
            },
            14 | 17 => match cell(w, pb) {
                Some((r, c, ..)) => Value::Long(if what == 14 { r } else { c } as i32),
                None => Value::Long(-1),
            },
            _ => return Err(Error::unsupported("this Information type (there are no pages or lines while a macro runs)")),
        })
    }

    /// ComputeStatistics: words, characters, paragraphs.
    pub(crate) fn statistics(&mut self, s: usize, e: usize, what: i64) -> R<Value> {
        let w = self.w();
        let t = w.text(s, e)?;
        let n = match what {
            0 => t.split(|c: char| c.is_whitespace() || c == '\u{7}').filter(|x| x.chars().any(|c| c.is_alphanumeric())).count(),
            3 => t.chars().filter(|c| !c.is_whitespace() && !c.is_control()).count(),
            5 => t.chars().filter(|c| !matches!(c, '\r' | '\u{7}')).count(),
            4 => t.split('\r').filter(|p| !p.trim_matches(['\u{7}', ' ']).is_empty()).count(),
            _ => return Err(Error::unsupported("counting lines or pages (there is no page layout while a macro runs)")),
        };
        Ok(Value::Long(n.min(i32::MAX as usize) as i32))
    }

    /// GoTo(What, Which, Count, Name): bookmarks (-1), sections (0),
    /// tables (2), comments (6) and headings (11).
    fn goto(&mut self, from: usize, a: &Args) -> R<(usize, usize)> {
        let what = self.arg_i64(a, 0, "what")?.ok_or_else(Error::invalid_call)?;
        let which = self.arg_i64(a, 1, "which")?.unwrap_or(1);
        let count = self.arg_i64(a, 2, "count")?.unwrap_or(1);
        let name = self.arg_str(a, 3, "name")?;
        let w = self.w();
        if what == -1 {
            let n = name.ok_or_else(Error::invalid_call)?;
            let b = w.doc.bookmark(&n).ok_or_else(|| Error::new(5101, "This bookmark does not exist"))?;
            let (bs, be) = (w.char_of(b.start), w.char_of(b.end));
            return Ok((bs, be));
        }
        let places: Vec<usize> = match what {
            0 => {
                let firsts: Vec<usize> = w.doc.sections().iter().map(|x| x.first).collect();
                firsts.into_iter().map(|p| w.para_start(p)).collect()
            }
            2 => {
                let starts: Vec<usize> = w.doc.tables().iter().map(|m| m.start).collect();
                starts.into_iter().map(|p| w.para_start(p)).collect()
            }
            6 => {
                let mut v: Vec<apex_doc::Pos> = w.doc.comment_spans().iter().map(|c| c.start).collect();
                v.sort();
                v.into_iter().map(|p| w.char_of(p)).collect()
            }
            11 => {
                let hs: Vec<usize> = (0..w.doc.paragraphs.len()).filter(|&i| matches!(w.doc.paragraphs[i].style.kind, ParaKind::Heading(_))).collect();
                hs.into_iter().map(|p| w.para_start(p)).collect()
            }
            1 | 3 => return Err(Error::unsupported("going to a page or line (there is no page layout while a macro runs)")),
            _ => return Err(Error::unsupported("going to this kind of item")),
        };
        let pick = match which {
            1 if count >= 1 => places.get(count as usize - 1).copied(),
            -1 => places.last().copied(),
            2 => places.iter().copied().filter(|&p| p > from).nth(count.max(1) as usize - 1),
            3 => places.iter().rev().copied().filter(|&p| p < from).nth(count.max(1) as usize - 1),
            _ => None,
        };
        let c = pick.or(if which == 1 { places.last().copied() } else { None }).ok_or_else(|| Error::new(5941, "The requested member of the collection does not exist"))?;
        Ok((c, c))
    }

    /// Range.Style = ...: a paragraph style for its paragraphs (part of
    /// one: a linked style's look for the text), a character style for
    /// the text.
    fn set_style(&mut self, tg: &Tg, v: &Value) -> R<()> {
        let (s, e) = self.wspan(tg);
        let st = self.w().style_of(v)?.ok_or_else(|| Error::new(5834, "Item with specified name does not exist"))?;
        let w = self.w();
        let (from, to) = w.paras_of(s, e);
        match st {
            (_, n, true) => {
                let id = w.doc.styles.defs[n as usize - 1].id.clone();
                if s < e {
                    w.mutate(s, e, |d, sel| {
                        d.apply_char_style(sel, Some(id));
                        Some(sel.cursor)
                    })?;
                }
            }
            (kind, custom, false) => {
                w.mutate(s, e, |d, sel| {
                    d.apply_style(from, to, sel, kind, custom);
                    Some(sel.cursor)
                })?;
            }
        }
        Ok(())
    }

    /// Changes the characters of a Range or the Selection; with nothing
    /// selected, what the Selection types next.
    fn format_chars(&mut self, tg: &Tg, f: &dyn Fn(&mut Attrs)) -> R<()> {
        let (s, e) = self.wspan(tg);
        let w = self.w();
        w.writable()?;
        if s >= e {
            if matches!(tg, Tg::Sel) {
                let mut at = match w.typing.take() {
                    Some((c, at)) if c == s => at,
                    _ => {
                        let p = w.pos_of(s);
                        w.doc.typing_attrs(p)
                    }
                };
                f(&mut at);
                w.typing = Some((s, at));
            }
            return Ok(());
        }
        w.mutate(s, e, |d, sel| {
            d.format_range(sel, f);
            Some(sel.cursor)
        })?;
        Ok(())
    }

    // --- Selection ---------------------------------------------------------------------------

    pub(crate) fn sel_call(&mut self, name: &str, a: &Args) -> R<Value> {
        let w = self.w();
        let (s, e) = w.sel();
        let len = w.story_len();
        let last = len - 1;
        let int = |it: &mut Self, pos: usize, n: &str, d: i64| -> R<i64> { Ok(it.arg_i64(a, pos, n)?.unwrap_or(d)) };
        Ok(match name {
            "typetext" | "typeparagraph" => {
                let t = if name == "typetext" { clean_input(&self.arg_str(a, 0, "text")?.ok_or_else(Error::invalid_call)?) } else { "\n".into() };
                let w = self.w();
                let attrs = match w.typing.take() {
                    Some((c, at)) if c == s && s == e => Some(at),
                    _ => None,
                };
                let (_, end) = w.replace_as(s, e, &t, attrs)?;
                w.anchor = end;
                w.active = end;
                Value::Empty
            }
            "typebackspace" => {
                let w = self.w();
                let (a0, b0) = if s < e { (s, e) } else { (s.saturating_sub(1), s) };
                if a0 < b0 {
                    let (at, _) = w.replace(a0, b0, "")?;
                    w.anchor = at;
                    w.active = at;
                }
                Value::Empty
            }
            "homekey" | "endkey" => {
                let unit = Unit::of(int(self, 0, "unit", 5)?)?;
                let extend = int(self, 1, "extend", 0)? == 1;
                let w = self.w();
                let from = w.active;
                let to = match (unit, name) {
                    (Unit::Story, "homekey") => 0,
                    (Unit::Story, _) => last,
                    (u, "homekey") => w.unit_at(from, u)?.0,
                    (Unit::Para, _) => {
                        let p = w.para_of(from);
                        w.para_start(p + 1).saturating_sub(1)
                    }
                    (u, _) => w.unit_at(from, u)?.1.saturating_sub(1),
                };
                let to = to.min(last);
                if !extend {
                    w.anchor = to;
                }
                w.active = to;
                Value::Long(to as i32 - from as i32)
            }
            "moveleft" | "moveright" | "moveup" | "movedown" => {
                let vertical = matches!(name, "moveup" | "movedown");
                let unit = Unit::of(int(self, 0, "unit", if vertical { 5 } else { 1 })?)?;
                let count = int(self, 1, "count", 1)?;
                let extend = int(self, 2, "extend", 0)? == 1;
                let back = matches!(name, "moveleft" | "moveup");
                let count = if back { -count } else { count };
                if count == 0 {
                    return Ok(Value::Long(0));
                }
                let w = self.w();
                if !extend && s < e && !vertical {
                    // A selection collapses first: that is one move.
                    let c = if back { s } else { e.min(last) };
                    w.anchor = c;
                    w.active = c;
                    let (c2, moved) = self.steps(c, unit, count - count.signum())?;
                    let c2 = c2.min(last);
                    let w = self.w();
                    w.anchor = c2;
                    w.active = c2;
                    return Ok(Value::Long((moved + count.signum()) as i32));
                }
                let from = w.active;
                let (to, moved) = if vertical && unit == Unit::Para {
                    // Lines are paragraphs: the same place in the one above or below.
                    let p = w.para_of(from);
                    let off = from - w.para_start(p);
                    let n = w.doc.paragraphs.len() as i64;
                    let q = (p as i64 + count).clamp(0, n - 1) as usize;
                    let room = w.para_start(q + 1) - w.para_start(q) - 1;
                    (w.para_start(q) + off.min(room), q as i64 - p as i64)
                } else {
                    self.steps(from, unit, count)?
                };
                let to = to.min(if extend { len } else { last });
                let w = self.w();
                if !extend {
                    w.anchor = to;
                }
                w.active = to;
                Value::Long(moved.abs() as i32)
            }
            "wholestory" => {
                let w = self.w();
                w.anchor = 0;
                w.active = len;
                Value::Empty
            }
            "type" => Value::Long(if s < e { 2 } else { 1 }),
            "startisactive" => Value::Bool(w.active < w.anchor),
            "range" => self.new_range(s, e),
            "escapekey" => Value::Empty,
            "selectrow" | "selectcell" => {
                let w = self.w();
                let (rs, re) = w.unit_at(s, if name == "selectrow" { Unit::Row } else { Unit::Cell })?;
                w.anchor = rs;
                w.active = re;
                Value::Empty
            }
            "goto" => {
                let (gs, ge) = self.goto(s, a)?;
                let w = self.w();
                w.anchor = gs;
                w.active = ge;
                self.new_range(gs, ge)
            }
            "insertrowsabove" | "insertrowsbelow" | "insertcolumns" | "insertcolumnsright" | "insertrows" => {
                let n = int(self, 0, "numrows", 1)?.clamp(1, 100) as usize;
                for _ in 0..n {
                    self.table_insert(s, e, name)?;
                }
                Value::Empty
            }
            "extend" | "shrink" | "copyformat" | "pasteformat" => return Err(Error::unsupported(&format!("Selection.{name}"))),
            _ => return self.text_call(Tg::Sel, name, a),
        })
    }

    pub(crate) fn sel_set(&mut self, name: &str, v: Value) -> R<()> {
        match name {
            "start" | "end" => {
                let len = self.w().story_len();
                let n = self.scalar(v)?.to_i64()?.clamp(0, len as i64) as usize;
                let w = self.w();
                let (s, e) = w.sel();
                let (s, e) = if name == "start" { (n, e.max(n)) } else { (s.min(n), n) };
                w.anchor = s;
                w.active = e;
                Ok(())
            }
            _ => self.text_set(Tg::Sel, name, v),
        }
    }

    /// Selection.InsertRowsAbove... on the table the Selection is in.
    fn table_insert(&mut self, s: usize, e: usize, name: &str) -> R<()> {
        let w = self.w();
        let p = w.para_of(s);
        let (ta, tb) = w.doc.table_bounds(p).ok_or_else(|| Error::new(4605, "This method or property is not available because the object refers to a place that isn't in a table"))?;
        let (rs, re) = (w.para_start(ta), w.para_start(tb));
        let sel = w.doc_sel(s, e);
        let width = w.doc.page.content_width();
        w.mutate_in(rs, re, sel, |d, sel| {
            match name {
                "insertrowsabove" => d.table_insert_rows(sel, false),
                "insertrowsbelow" | "insertrows" => d.table_insert_rows(sel, true),
                "insertcolumns" => d.table_insert_cols(sel, false, width),
                _ => d.table_insert_cols(sel, true, width),
            }
            .map(|s| s.cursor)
        })?;
        Ok(())
    }

    // --- Paragraphs, Words... ------------------------------------------------------------

    /// A collection, or its item when there is an index.
    pub(crate) fn collection(&mut self, o: WObj, a: &Args) -> R<Value> {
        if a.is_empty() {
            Ok(wo(o))
        } else {
            self.word_call(o, "item", a)
        }
    }

    pub(crate) fn units_call(&mut self, r: SpanRef, unit: Unit, name: &str, a: &Args) -> R<Value> {
        let (s, e) = self.wspan(&Tg::Range(r.clone()));
        let missing = || Error::new(5941, "The requested member of the collection does not exist");
        match name {
            "count" => Ok(Value::Long(self.w().count_in(s, e, unit)?.min(i32::MAX as usize) as i32)),
            "item" | "_default" | "first" | "last" => {
                let w = self.w();
                let n = if name == "first" {
                    1
                } else if name == "last" {
                    w.count_in(s, e, unit)? as i64
                } else {
                    self.arg_i64(a, 0, "index")?.ok_or_else(Error::invalid_call)?
                };
                if n < 1 {
                    return Err(missing());
                }
                let w = self.w();
                let (us, ue) = if unit == Unit::Para {
                    let (pa, pb) = w.paras_of(s, e);
                    let p = pa + n as usize - 1;
                    if p > pb {
                        return Err(missing());
                    }
                    (w.para_start(p), w.para_start(p + 1))
                } else {
                    *w.units_in(s, e, unit, n as usize)?.get(n as usize - 1).ok_or_else(missing)?
                };
                let sp = self.w().new_span(us, ue);
                Ok(wo(if unit == Unit::Para { WObj::Para(sp) } else { WObj::Range(sp) }))
            }
            "add" if unit == Unit::Para => {
                let at = match a.get(0, "range") {
                    Some(v) => self.span_of(&v)?.0,
                    None => self.last(),
                };
                let (_, end) = self.w().replace(at, at, "\n")?;
                let w = self.w();
                let p = w.para_of(end);
                let (ps, pe) = (w.para_start(p), w.para_start(p + 1));
                let sp = w.new_span(ps, pe);
                Ok(wo(WObj::Para(sp)))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ if unit == Unit::Para => self.pformat_get(Tg::Range(r), name),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(crate) fn units_items(&mut self, r: &SpanRef, unit: Unit) -> R<Vec<Value>> {
        let (s, e) = self.wspan(&Tg::Range(r.clone()));
        let w = self.w();
        let spans = w.units_in(s, e, unit, MAX_ITEMS + 1)?;
        if spans.len() > MAX_ITEMS {
            return Err(Error::new(7, "Out of memory: too many items"));
        }
        Ok(spans
            .into_iter()
            .map(|(a, b)| {
                let sp = w.new_span(a, b);
                wo(if unit == Unit::Para { WObj::Para(sp) } else { WObj::Range(sp) })
            })
            .collect())
    }

    pub(crate) fn para_call(&mut self, r: SpanRef, name: &str, a: &Args) -> R<Value> {
        let (s, e) = self.wspan(&Tg::Range(r.clone()));
        match name {
            "range" => Ok(self.new_range(s, e)),
            "format" | "paragraphformat" => Ok(wo(WObj::ParaFormat(Tg::Range(r)))),
            "next" | "previous" => {
                let n = self.arg_i64(a, 0, "count")?.unwrap_or(1).max(1) as usize;
                let w = self.w();
                let p = w.para_of(s);
                let q = if name == "next" { p.checked_add(n).filter(|&q| q < w.doc.paragraphs.len()) } else { p.checked_sub(n) };
                let Some(q) = q else { return Ok(Value::Nothing) };
                let (qs, qe) = (w.para_start(q), w.para_start(q + 1));
                let sp = w.new_span(qs, qe);
                Ok(wo(WObj::Para(sp)))
            }
            "style" => self.text_call(Tg::Range(r), name, a),
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            "isstyleseparator" => Ok(Value::Bool(false)),
            "listformat" | "font" => self.text_call(Tg::Range(r), name, a),
            _ => self.pformat_get(Tg::Range(r), name),
        }
    }

    pub(crate) fn para_set(&mut self, r: SpanRef, name: &str, v: Value) -> R<()> {
        match name {
            "style" => self.text_set(Tg::Range(r), name, v),
            _ => self.pformat_set(Tg::Range(r), name, v),
        }
    }

    // --- Font ---------------------------------------------------------------------------------

    pub(crate) fn wfont_get(&mut self, tg: Tg, name: &str) -> R<Value> {
        let (s, e) = self.wspan(&tg);
        if name == "reset" {
            self.format_chars(&tg, &|a| {
                a.bold = false;
                a.italic = false;
                a.underline = false;
                a.strike = false;
                a.superscript = false;
                a.subscript = false;
                a.size = None;
                a.color = None;
                a.font = None;
                a.char_style = None;
                a.fx = None;
            })?;
            return Ok(Value::Empty);
        }
        if matches!(name, "application" | "parent") {
            return Ok(if name == "application" { Value::Object(Obj::Application) } else { wo(WObj::Range(self.w().new_span(s, e))) });
        }
        let default_font = self.w().doc.default_font_name();
        let get: Box<dyn Fn(&CharFormat, &Attrs) -> Value> = match name {
            "bold" => Box::new(|f, _| wbool(f.bold)),
            "italic" => Box::new(|f, _| wbool(f.italic)),
            "underline" => Box::new(|f, _| Value::Long(if !f.underline {
                0
            } else {
                match f.fx.underline {
                    apex_doc::UnderlineStyle::Single => 1,
                    apex_doc::UnderlineStyle::Words => 2,
                    apex_doc::UnderlineStyle::Double => 3,
                    apex_doc::UnderlineStyle::Dotted => 4,
                    apex_doc::UnderlineStyle::Thick => 6,
                    apex_doc::UnderlineStyle::Dashed => 7,
                    apex_doc::UnderlineStyle::DotDash => 9,
                    apex_doc::UnderlineStyle::Wave => 11,
                }
            })),
            "strikethrough" => Box::new(|_, a| wbool(a.strike)),
            "doublestrikethrough" => Box::new(|f, _| wbool(f.fx.double_strike)),
            "superscript" => Box::new(|_, a| wbool(a.superscript)),
            "subscript" => Box::new(|_, a| wbool(a.subscript)),
            "allcaps" => Box::new(|f, _| wbool(f.fx.all_caps)),
            "smallcaps" => Box::new(|f, _| wbool(f.fx.small_caps)),
            "hidden" => Box::new(|f, _| wbool(f.fx.hidden)),
            "size" => Box::new(|f, _| Value::Double(f.size as f64 / 2.0)),
            "name" => Box::new(move |f, _| Value::str(f.font.clone().or_else(|| default_font.clone()).as_deref().unwrap_or("Calibri"))),
            "color" | "textcolor" => Box::new(|f, _| Value::Long(f.color.map_or(AUTO as i32, to_bgr))),
            "colorindex" => Box::new(|f, _| Value::Long(f.color.map_or(0, color_index))),
            "spacing" => Box::new(|f, _| Value::Double(f.fx.spacing as f64 / 20.0)),
            "position" => Box::new(|f, _| Value::Double(f.fx.position as f64 / 2.0)),
            _ => return Err(Error::no_member(name)),
        };
        let mut out: Option<Value> = None;
        let mut mixed = false;
        self.w().each_run(s, e, &mut |f, a| {
            if mixed {
                return;
            }
            let v = get(f, a);
            match &out {
                None => out = Some(v),
                Some(o) if !eqv(o, &v) => mixed = true,
                _ => {}
            }
        });
        Ok(if mixed {
            if name == "name" { Value::str("") } else { Value::Long(UNDEFINED as i32) }
        } else {
            out.unwrap_or(Value::Long(UNDEFINED as i32))
        })
    }

    pub(crate) fn wfont_set(&mut self, tg: Tg, name: &str, v: Value) -> R<()> {
        let v = self.scalar(v)?;
        // wdToggle: the opposite of what the first character has.
        let flag = |it: &mut Self, v: &Value| -> R<bool> {
            if matches!(v, Value::Long(_) | Value::Int(_) | Value::Double(_)) && v.to_i64()? == TOGGLE {
                let now = it.wfont_get(tg.clone(), name)?;
                return Ok(now.to_i64().unwrap_or(0) == 0);
            }
            v.to_bool()
        };
        match name {
            "bold" | "italic" | "strikethrough" | "doublestrikethrough" | "superscript" | "subscript" | "allcaps" | "smallcaps" | "hidden" => {
                let b = flag(self, &v)?;
                let f: Box<dyn Fn(&mut Attrs)> = match name {
                    "bold" => Box::new(move |a| a.bold = b),
                    "italic" => Box::new(move |a| a.italic = b),
                    "strikethrough" => Box::new(move |a| a.strike = b),
                    "doublestrikethrough" => Box::new(move |a| with_fx(a, |x| x.double_strike = b)),
                    "superscript" => Box::new(move |a| {
                        a.superscript = b;
                        if b {
                            a.subscript = false;
                        }
                    }),
                    "subscript" => Box::new(move |a| {
                        a.subscript = b;
                        if b {
                            a.superscript = false;
                        }
                    }),
                    "allcaps" => Box::new(move |a| with_fx(a, |x| x.all_caps = b)),
                    "smallcaps" => Box::new(move |a| with_fx(a, |x| x.small_caps = b)),
                    _ => Box::new(move |a| with_fx(a, |x| x.hidden = b)),
                };
                self.format_chars(&tg, &*f)
            }
            "underline" => {
                let n = if matches!(v, Value::Bool(_)) { if v.to_bool()? { 1 } else { 0 } } else { v.to_i64()? };
                let n = if n == TOGGLE { if self.wfont_get(tg.clone(), name)?.to_i64().unwrap_or(0) == 0 { 1 } else { 0 } } else { n };
                use apex_doc::UnderlineStyle as U;
                let style = match n {
                    0 | 1 => U::Single,
                    2 => U::Words,
                    3 => U::Double,
                    4 => U::Dotted,
                    6 => U::Thick,
                    7 => U::Dashed,
                    9 => U::DotDash,
                    11 => U::Wave,
                    _ => return Err(Error::unsupported("this WdUnderline")),
                };
                self.format_chars(&tg, &move |a| {
                    a.underline = n != 0;
                    with_fx(a, |x| x.underline = style);
                })
            }
            "size" => {
                let h = half_points(v.to_f64()?)?;
                self.format_chars(&tg, &move |a| a.size = Some(h))
            }
            "name" => {
                let n = v.to_str()?;
                let n = n.trim();
                if n.is_empty() || n.chars().count() > 31 || n.chars().any(char::is_control) {
                    return Err(Error::invalid_call());
                }
                let f: Arc<str> = Arc::from(n);
                self.format_chars(&tg, &move |a| a.font = Some(f.clone()))
            }
            "color" | "textcolor" => {
                let c = from_bgr(v.to_i64()?)?;
                self.format_chars(&tg, &move |a| a.color = c)
            }
            "colorindex" => {
                let c = index_color(v.to_i64()?)?;
                self.format_chars(&tg, &move |a| a.color = c)
            }
            "spacing" => {
                let x = v.to_f64()?;
                if !(-1584.0..=1584.0).contains(&x) {
                    return Err(out_of_range());
                }
                let t = (x * 20.0).round() as i16;
                self.format_chars(&tg, &move |a| with_fx(a, |f| f.spacing = t))
            }
            "position" => {
                let x = v.to_f64()?;
                if !(-1584.0..=1584.0).contains(&x) {
                    return Err(out_of_range());
                }
                let h = (x * 2.0).round() as i16;
                self.format_chars(&tg, &move |a| with_fx(a, |f| f.position = h))
            }
            _ => Err(Error::no_member(name)),
        }
    }

    // --- ParagraphFormat --------------------------------------------------------------------

    pub(crate) fn pformat_get(&mut self, tg: Tg, name: &str) -> R<Value> {
        if name == "tabstops" {
            return Ok(wo(WObj::Tabs(tg)));
        }
        let (s, e) = self.wspan(&tg);
        let w = self.w();
        let (from, to) = w.paras_of(s, e);
        let get: Box<dyn Fn(&ParaStyle) -> Value> = match name {
            "alignment" => Box::new(|st| {
                Value::Long(match st.align {
                    Align::Left => 0,
                    Align::Center => 1,
                    Align::Right => 2,
                    Align::Justify => 3,
                })
            }),
            "leftindent" => Box::new(|st| Value::Double(st.indent_left as f64)),
            "rightindent" => Box::new(|st| Value::Double(st.indent_right as f64)),
            "firstlineindent" => Box::new(|st| Value::Double(st.first_line as f64)),
            "spacebefore" => Box::new(|st| Value::Double(st.spacing().0 as f64)),
            "spaceafter" => Box::new(|st| Value::Double(st.spacing().1 as f64)),
            "linespacingrule" => Box::new(|st| {
                Value::Long(match st.line_rule {
                    LineRule::AtLeast => 3,
                    LineRule::Exact => 4,
                    LineRule::Multiple if st.line_spacing <= 0.0 || st.line_spacing == 1.0 => 0,
                    LineRule::Multiple if st.line_spacing == 1.5 => 1,
                    LineRule::Multiple if st.line_spacing == 2.0 => 2,
                    LineRule::Multiple => 5,
                })
            }),
            "linespacing" => Box::new(|st| {
                Value::Double(match st.line_rule {
                    LineRule::Multiple => 12.0 * if st.line_spacing > 0.0 { st.line_spacing as f64 } else { 1.0 },
                    _ => st.line_spacing as f64,
                })
            }),
            "keepwithnext" => Box::new(|st| wbool(st.keep_with_next)),
            "keeptogether" => Box::new(|st| wbool(st.keep_lines)),
            "pagebreakbefore" => Box::new(|st| wbool(st.page_break_before)),
            "widowcontrol" => Box::new(|st| wbool(!st.no_widow_control)),
            "outlinelevel" => Box::new(|st| {
                Value::Long(match st.kind {
                    ParaKind::Heading(n) => n as i32,
                    _ => 10,
                })
            }),
            "style" => {
                let st = w.doc.paragraphs[from].style;
                return Ok(wo(WObj::Style(w.style_name(st.kind, st.custom).into())));
            }
            "application" => return Ok(Value::Object(Obj::Application)),
            _ => return Err(Error::no_member(name)),
        };
        let first = get(&w.doc.paragraphs[from].style);
        for i in from + 1..=to {
            if !eqv(&first, &get(&w.doc.paragraphs[i].style)) {
                return Ok(Value::Long(UNDEFINED as i32));
            }
        }
        Ok(first)
    }

    pub(crate) fn pformat_set(&mut self, tg: Tg, name: &str, v: Value) -> R<()> {
        if name == "style" {
            return self.text_set(tg, name, v);
        }
        let v = self.scalar(v)?;
        let pts = |v: &Value, lo: f64, hi: f64| -> R<f32> {
            let x = v.to_f64()?;
            if !(lo..=hi).contains(&x) {
                return Err(out_of_range());
            }
            Ok(x as f32)
        };
        let f: Box<dyn Fn(&mut ParaStyle)> = match name {
            "alignment" => {
                let al = match v.to_i64()? {
                    0 => Align::Left,
                    1 => Align::Center,
                    2 => Align::Right,
                    3 | 4 | 5 | 7 | 8 | 9 => Align::Justify,
                    _ => return Err(out_of_range()),
                };
                Box::new(move |st| st.align = al)
            }
            "leftindent" => {
                let x = pts(&v, -1584.0, 1584.0)?;
                Box::new(move |st| st.indent_left = x)
            }
            "rightindent" => {
                let x = pts(&v, -1584.0, 1584.0)?;
                Box::new(move |st| st.indent_right = x)
            }
            "firstlineindent" => {
                let x = pts(&v, -1584.0, 1584.0)?;
                Box::new(move |st| st.first_line = x)
            }
            "spacebefore" => {
                let x = pts(&v, 0.0, 1584.0)?;
                Box::new(move |st| st.space_before = Some(x))
            }
            "spaceafter" => {
                let x = pts(&v, 0.0, 1584.0)?;
                Box::new(move |st| st.space_after = Some(x))
            }
            "linespacingrule" => {
                let n = v.to_i64()?;
                if !(0..=5).contains(&n) {
                    return Err(out_of_range());
                }
                Box::new(move |st| match n {
                    0 => (st.line_rule, st.line_spacing) = (LineRule::Multiple, 1.0),
                    1 => (st.line_rule, st.line_spacing) = (LineRule::Multiple, 1.5),
                    2 => (st.line_rule, st.line_spacing) = (LineRule::Multiple, 2.0),
                    5 => {
                        if st.line_rule != LineRule::Multiple {
                            st.line_spacing = 1.0;
                        }
                        st.line_rule = LineRule::Multiple;
                    }
                    _ => {
                        if st.line_rule == LineRule::Multiple {
                            st.line_spacing = 12.0 * if st.line_spacing > 0.0 { st.line_spacing } else { 1.0 };
                        }
                        st.line_rule = if n == 3 { LineRule::AtLeast } else { LineRule::Exact };
                    }
                })
            }
            "linespacing" => {
                let x = pts(&v, 0.06, 1584.0)?;
                Box::new(move |st| st.line_spacing = if st.line_rule == LineRule::Multiple { x / 12.0 } else { x })
            }
            "keepwithnext" | "keeptogether" | "pagebreakbefore" | "widowcontrol" => {
                let b = v.to_bool()?;
                match name {
                    "keepwithnext" => Box::new(move |st| st.keep_with_next = b),
                    "keeptogether" => Box::new(move |st| st.keep_lines = b),
                    "pagebreakbefore" => Box::new(move |st| st.page_break_before = b),
                    _ => Box::new(move |st| st.no_widow_control = !b),
                }
            }
            "outlinelevel" => return Err(Error::unsupported("setting OutlineLevel (give the paragraph a heading style)")),
            _ => return Err(Error::no_member(name)),
        };
        let (s, e) = self.wspan(&tg);
        let w = self.w();
        let (from, to) = w.paras_of(s, e);
        w.mutate(s, e, |d, sel| {
            d.set_para_style(from, to, sel, &*f);
            Some(sel.cursor)
        })?;
        Ok(())
    }

    // --- ListFormat -------------------------------------------------------------------------

    pub(crate) fn list_call(&mut self, tg: Tg, name: &str, _a: &Args) -> R<Value> {
        let (s, e) = self.wspan(&tg);
        let w = self.w();
        let (from, to) = w.paras_of(s, e);
        let st = w.doc.paragraphs[from].style;
        match name {
            "listtype" => Ok(Value::Long(match st.kind {
                ParaKind::Bullet => 2,
                ParaKind::Numbered => 3,
                _ => 0,
            })),
            "listlevelnumber" => Ok(Value::Long(st.level as i32 + 1)),
            "listvalue" => Ok(Value::Long(if st.kind == ParaKind::Numbered { w.doc.list_number(from) as i32 } else { 0 })),
            "liststring" => Ok(Value::str(match st.kind {
                ParaKind::Bullet => apex_doc::bullet_char(st.list_format, st.level).to_string(),
                ParaKind::Numbered => apex_doc::number_label(st.list_format, st.level, w.doc.list_number(from)),
                _ => String::new(),
            })),
            "applybulletdefault" | "applynumberdefault" | "removenumbers" => {
                let target = match name {
                    "applybulletdefault" => Some(ParaKind::Bullet),
                    "applynumberdefault" => Some(ParaKind::Numbered),
                    _ => None,
                };
                w.mutate(s, e, |d, sel| {
                    let styles = d.styles.clone();
                    d.set_para_style(from, to, sel, &|st| match target {
                        Some(k) if st.kind != k => styles.restyle(st, k, 0),
                        None if st.kind.is_list() => styles.restyle(st, ParaKind::Normal, 0),
                        _ => {}
                    });
                    Some(sel.cursor)
                })?;
                Ok(Value::Empty)
            }
            "listindent" | "listoutdent" => {
                let up = name == "listoutdent";
                w.mutate(s, e, |d, sel| {
                    d.set_para_style(from, to, sel, &|st| {
                        if st.kind.is_list() {
                            st.level = if up { st.level.saturating_sub(1) } else { (st.level + 1).min(8) };
                        }
                    });
                    Some(sel.cursor)
                })?;
                Ok(Value::Empty)
            }
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(crate) fn list_set(&mut self, tg: Tg, name: &str, v: Value) -> R<()> {
        match name {
            "listlevelnumber" => {
                let n = self.scalar(v)?.to_i64()?;
                if !(1..=9).contains(&n) {
                    return Err(out_of_range());
                }
                let (s, e) = self.wspan(&tg);
                let w = self.w();
                let (from, to) = w.paras_of(s, e);
                w.mutate(s, e, |d, sel| {
                    d.set_para_style(from, to, sel, &|st| {
                        if st.kind.is_list() {
                            st.level = n as u8 - 1;
                        }
                    });
                    Some(sel.cursor)
                })?;
                Ok(())
            }
            _ => Err(Error::no_member(name)),
        }
    }
}

/// Title Case: the first letter of each word up.
fn title_case(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut start = true;
    for c in t.chars() {
        if start && c.is_alphabetic() {
            out.extend(c.to_uppercase());
        } else {
            out.extend(c.to_lowercase());
        }
        start = !c.is_alphanumeric();
    }
    out
}

/// Sentence case: the first letter of each sentence up, the rest down.
fn sentence_case(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut start = true;
    for c in t.chars() {
        if start && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            start = false;
        } else {
            out.extend(c.to_lowercase());
        }
        if matches!(c, '.' | '!' | '?' | '\n') {
            start = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_and_cases() {
        assert_eq!(to_bgr(0xFF0000), 0x0000FF);
        assert_eq!(from_bgr(0x0000FF).unwrap(), Some(0xFF0000));
        assert_eq!(from_bgr(AUTO).unwrap(), None);
        assert!(from_bgr(-5).is_err());
        assert_eq!(color_index(0xFFFF00), 7);
        assert_eq!(half_points(10.5).unwrap(), 21);
        assert!(half_points(0.0).is_err());
        assert_eq!(title_case("hello wORLD"), "Hello World");
        assert_eq!(sentence_case("one. two"), "One. Two");
        assert_eq!(builtin_style("Título 2"), Some(ParaKind::Heading(2)));
        assert_eq!(builtin_style("Überschrift 1"), Some(ParaKind::Heading(1)));
        assert_eq!(builtin_style("Heading 10"), None);
    }
}
