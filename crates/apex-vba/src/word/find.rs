//! Find and Replace of a Range or the Selection: Execute with its
//! arguments, the Find and Replacement properties and their Font.

use apex_doc::{FindFormat, FindMode, FindOpts, Pos};

use super::{Tg, WObj, WordState, clean_input};
use crate::error::{Error, R};
use crate::excel::Args;
use crate::interp::Interp;
use crate::object::Obj;
use crate::value::Value;

/// What a Find looks for and where its last search was.
#[derive(Clone, Debug)]
pub struct FindState {
    pub text: String,
    pub repl: String,
    pub forward: bool,
    /// WdFindWrap: 0 stop, 1 continue, 2 ask (taken as stop).
    pub wrap: i64,
    pub case: bool,
    pub whole: bool,
    pub wildcards: bool,
    /// Whether the formatting set counts.
    pub format: bool,
    pub font: FindFormat,
    pub repl_font: FindFormat,
    pub found: bool,
    /// What the last Execute found: the next one goes on after it.
    pub last: Option<(usize, usize)>,
    /// Where the search started, the part of the text it covers, and
    /// whether it went round to the start already.
    pub origin: usize,
    pub scope: (usize, usize),
    pub wrapped: bool,
}

impl Default for FindState {
    fn default() -> Self {
        FindState { text: String::new(), repl: String::new(), forward: true, wrap: 0, case: false, whole: false, wildcards: false, format: false, font: FindFormat::default(), repl_font: FindFormat::default(), found: false, last: None, origin: 0, scope: (0, 0), wrapped: false }
    }
}

impl FindState {
    pub fn shift(&mut self, map: &impl Fn(usize) -> usize) {
        if let Some((a, b)) = self.last {
            self.last = Some((map(a), map(b)));
        }
        self.origin = map(self.origin);
        self.scope = (map(self.scope.0), map(self.scope.1));
    }

    fn clear_formatting(&mut self, repl: bool) {
        if repl {
            self.repl_font = FindFormat::default();
        } else {
            self.font = FindFormat::default();
        }
    }
}

/// Word's longest Find and Replace text.
const MAX_FIND: usize = 255;

impl WordState<'_> {
    /// Character positions of `ps`, in order.
    fn chars_of(&mut self, ps: &[Pos]) -> Vec<usize> {
        let mut out = Vec::with_capacity(ps.len());
        let (mut para, mut byte, mut n) = (usize::MAX, 0, 0);
        for &p in ps {
            let p = self.doc.clamp(p);
            if p.para != para || p.offset < byte {
                (para, byte, n) = (p.para, 0, 0);
            }
            n += self.doc.paragraphs[p.para].text[byte..p.offset].chars().count();
            byte = p.offset;
            out.push(self.para_start(p.para) + n);
        }
        out
    }

    /// Replaces `hits` (in order) with their texts: the spans of the new
    /// texts.
    fn replace_found(&mut self, hits: &[(Pos, Pos, String)], keep_text: bool, fmt: &FindFormat) -> R<Vec<(usize, usize)>> {
        self.writable()?;
        let flat: Vec<Pos> = hits.iter().flat_map(|h| [h.0, h.1]).collect();
        let at = self.chars_of(&flat);
        let len = self.story_len();
        let added: usize = hits.iter().map(|h| h.2.len()).sum();
        if len.saturating_add(added) > super::MAX_STORY {
            return Err(Error::new(7, "Out of memory: the document is too long"));
        }
        let sel = self.selection();
        let tracked = self.doc.track_changes;
        self.doc.replace_hits(hits, keep_text, fmt, sel);
        self.doc.trim_undo(super::KEEP_UNDO);
        // Each call is a step of its own for Document.Undo.
        self.doc.seal_undo();
        self.dirty();
        let spans: Vec<(usize, usize)> = at.chunks(2).map(|c| (c[0], c[1])).collect();
        if keep_text {
            return Ok(spans);
        }
        let news: Vec<usize> = hits.iter().map(|h| h.2.chars().count()).collect();
        let deltas: Vec<isize> = spans.iter().zip(&news).map(|(&(a, b), &n)| if tracked { n as isize } else { n as isize - (b - a) as isize }).collect();
        for k in (0..spans.len()).rev() {
            let (a, b) = spans[k];
            let e2 = if tracked { b + news[k] } else { a + news[k] };
            self.shift(a, b, e2, deltas[k]);
        }
        let mut out = Vec::with_capacity(spans.len());
        let mut moved = 0isize;
        for k in 0..spans.len() {
            let (a, b) = spans[k];
            let start = (a as isize + moved).max(0) as usize;
            let s = if tracked { start + (b - a) } else { start };
            out.push((s, s + news[k]));
            moved += deltas[k];
        }
        Ok(out)
    }
}

impl Interp<'_> {
    /// Runs `f` on the Find of `tg`.
    pub(crate) fn with_find<T>(&mut self, tg: &Tg, f: impl FnOnce(&mut FindState) -> T) -> T {
        match tg {
            Tg::Sel => f(&mut self.w().find),
            Tg::Range(r) => f(r.borrow_mut().find.get_or_insert_with(Default::default)),
        }
    }

    fn find_bool(&mut self, a: &Args, pos: usize, name: &str) -> R<Option<bool>> {
        match a.get(pos, name) {
            Some(v) => Ok(Some(self.scalar(v)?.to_bool()?)),
            None => Ok(None),
        }
    }

    pub(crate) fn find_member(&mut self, tg: Tg, repl: bool, name: &str, a: &Args) -> R<Value> {
        let st = self.with_find(&tg, |f| f.clone());
        Ok(match (repl, name) {
            (_, "text") => Value::str(if repl { st.repl } else { st.text }),
            (false, "replacement") => super::wo(WObj::Replacement(tg)),
            (_, "font") => super::wo(WObj::FindFont(tg, repl)),
            (_, "clearformatting") => {
                self.with_find(&tg, |f| f.clear_formatting(repl));
                Value::Empty
            }
            (_, "style") => {
                let f = if repl { st.repl_font } else { st.font };
                match f.para_style {
                    Some((k, c)) => Value::str(self.w().style_name(k, c)),
                    None => Value::str(""),
                }
            }
            (_, "highlight") => Value::Long(match (if repl { st.repl_font } else { st.font }).highlight {
                Some(true) => 1,
                Some(false) => 0,
                None => 9_999_999,
            }),
            (false, "forward") => Value::Bool(st.forward),
            (false, "wrap") => Value::Long(st.wrap as i32),
            (false, "matchcase") => Value::Bool(st.case),
            (false, "matchwholeword") => Value::Bool(st.whole),
            (false, "matchwildcards") => Value::Bool(st.wildcards),
            (false, "format") => Value::Bool(st.format),
            (false, "found") => Value::Bool(st.found),
            (false, "matchsoundslike" | "matchallwordforms" | "matchprefix" | "matchsuffix" | "matchbyte" | "ignorepunct" | "ignorespace" | "matchkashida" | "matchdiacritics" | "matchalefhamza" | "matchcontrol") => Value::Bool(false),
            (false, "execute") => return self.find_execute(tg, a),
            (_, "parent") => super::wo(match tg {
                Tg::Sel => WObj::Sel,
                Tg::Range(r) => WObj::Range(r),
            }),
            (_, "application") => Value::Object(Obj::Application),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(crate) fn find_set(&mut self, tg: Tg, repl: bool, name: &str, v: Value) -> R<()> {
        let v = self.scalar(v)?;
        match (repl, name) {
            (_, "text") => {
                let t = v.to_str()?;
                if t.chars().count() > MAX_FIND {
                    return Err(Error::new(5854, "The string parameter is too long"));
                }
                self.with_find(&tg, |f| if repl { f.repl = t } else { f.text = t });
            }
            (_, "style") => {
                let st = self.w().style_of(&v)?;
                let Some((kind, custom, false)) = st else { return Err(Error::new(5834, "Item with specified name does not exist")) };
                self.with_find(&tg, |f| if repl { f.repl_font.para_style = Some((kind, custom)) } else { f.font.para_style = Some((kind, custom)) });
            }
            (_, "highlight") => {
                let h = match v.to_i64()? {
                    9_999_999 => None,
                    0 => Some(false),
                    _ => Some(true),
                };
                self.with_find(&tg, |f| if repl { f.repl_font.highlight = h } else { f.font.highlight = h });
            }
            (false, "forward") => {
                let b = v.to_bool()?;
                self.with_find(&tg, |f| f.forward = b);
            }
            (false, "wrap") => {
                let n = v.to_i64()?;
                self.with_find(&tg, |f| f.wrap = n);
            }
            (false, "matchcase" | "matchwholeword" | "matchwildcards" | "format") => {
                let b = v.to_bool()?;
                self.with_find(&tg, |f| match name {
                    "matchcase" => f.case = b,
                    "matchwholeword" => f.whole = b,
                    "matchwildcards" => f.wildcards = b,
                    _ => f.format = b,
                });
            }
            (false, "matchsoundslike" | "matchallwordforms" | "matchprefix" | "matchsuffix" | "matchbyte" | "ignorepunct" | "ignorespace" | "matchkashida" | "matchdiacritics" | "matchalefhamza" | "matchcontrol") => {
                if v.to_bool()? {
                    return Err(Error::unsupported(&format!("Find.{name}")));
                }
            }
            _ => return Err(Error::no_member(name)),
        }
        Ok(())
    }

    /// The Font of a Find or Replacement: what it looks for or gives.
    pub(crate) fn find_font_get(&mut self, tg: Tg, repl: bool, name: &str) -> R<Value> {
        let f = self.with_find(&tg, |f| if repl { f.repl_font.clone() } else { f.font.clone() });
        let b = |x: Option<bool>| match x {
            Some(true) => Value::Bool(true),
            Some(false) => Value::Bool(false),
            None => Value::Long(9_999_999),
        };
        Ok(match name {
            "bold" => b(f.bold),
            "italic" => b(f.italic),
            "underline" => b(f.underline),
            "strikethrough" => b(f.strike),
            "name" => Value::str(f.font.as_deref().unwrap_or("")),
            "size" => f.size.map_or(Value::Long(9_999_999), |s| Value::Double(s as f64 / 2.0)),
            "color" => f.color.map_or(Value::Long(9_999_999), |c| Value::Long(super::range::to_bgr(c))),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(crate) fn find_font_set(&mut self, tg: Tg, repl: bool, name: &str, v: Value) -> R<()> {
        let v = self.scalar(v)?;
        let undefined = matches!(v, Value::Long(9_999_999) | Value::Double(9_999_999.0));
        let flag = |v: &Value| -> R<Option<bool>> { if undefined { Ok(None) } else { Ok(Some(v.to_bool()?)) } };
        let change: Box<dyn Fn(&mut FindFormat)> = match name {
            "bold" => {
                let x = flag(&v)?;
                Box::new(move |f| f.bold = x)
            }
            "italic" => {
                let x = flag(&v)?;
                Box::new(move |f| f.italic = x)
            }
            "underline" => {
                let x = if undefined { None } else { Some(v.to_i64()? != 0) };
                Box::new(move |f| f.underline = x)
            }
            "strikethrough" => {
                let x = flag(&v)?;
                Box::new(move |f| f.strike = x)
            }
            "name" => {
                let n = v.to_str()?;
                let x = (!n.is_empty()).then(|| std::sync::Arc::<str>::from(n.as_str()));
                Box::new(move |f| f.font = x.clone())
            }
            "size" => {
                let x = if undefined { None } else { Some(super::range::half_points(v.to_f64()?)?) };
                Box::new(move |f| f.size = x)
            }
            "color" => {
                let x = if undefined { None } else { super::range::from_bgr(v.to_i64()?)? };
                Box::new(move |f| f.color = x)
            }
            _ => return Err(Error::no_member(name)),
        };
        self.with_find(&tg, |f| change(if repl { &mut f.repl_font } else { &mut f.font }));
        Ok(())
    }

    /// Find.Execute: searches (and replaces) as set, with the arguments
    /// changing the settings first.
    fn find_execute(&mut self, tg: Tg, a: &Args) -> R<Value> {
        let text = self.arg_str(a, 0, "findtext")?;
        let case = self.find_bool(a, 1, "matchcase")?;
        let whole = self.find_bool(a, 2, "matchwholeword")?;
        let wild = self.find_bool(a, 3, "matchwildcards")?;
        for (i, n) in [(4, "matchsoundslike"), (5, "matchallwordforms")] {
            if self.find_bool(a, i, n)? == Some(true) {
                return Err(Error::unsupported(&format!("Find.{n}")));
            }
        }
        let forward = self.find_bool(a, 6, "forward")?;
        let wrap = self.arg_i64(a, 7, "wrap")?;
        let format = self.find_bool(a, 8, "format")?;
        let with = self.arg_str(a, 9, "replacewith")?;
        let replace = self.arg_i64(a, 10, "replace")?.unwrap_or(0);
        if text.as_ref().or(with.as_ref()).is_some_and(|t| t.chars().count() > MAX_FIND) {
            return Err(Error::new(5854, "The string parameter is too long"));
        }
        let st = self.with_find(&tg, |f| {
            if let Some(t) = text {
                f.text = t;
            }
            if let Some(t) = with {
                f.repl = t;
            }
            f.case = case.unwrap_or(f.case);
            f.whole = whole.unwrap_or(f.whole);
            f.wildcards = wild.unwrap_or(f.wildcards);
            f.forward = forward.unwrap_or(f.forward);
            f.wrap = wrap.unwrap_or(f.wrap);
            f.format = format.unwrap_or(f.format);
            f.clone()
        });
        let fmt_find = if st.format { st.font.clone() } else { FindFormat::default() };
        let fmt_repl = if st.format { st.repl_font.clone() } else { FindFormat::default() };
        let query = clean_input(&st.text);
        if query.is_empty() && fmt_find.is_empty() {
            self.with_find(&tg, |f| f.found = false);
            return Ok(Value::Bool(false));
        }
        let opts = FindOpts { case: st.case, whole: st.whole, mode: if st.wildcards { FindMode::Wildcards } else { FindMode::Plain }, format: fmt_find };
        let with = clean_input(&st.repl);
        let keep_text = with.is_empty() && !fmt_repl.is_empty();
        let (s, e) = self.wspan(&tg);
        let w = self.w();
        let len = w.story_len();
        let hits = w.doc.find_hits(&query, &opts, &with).map_err(|m| Error::new(5560, format!("The Find What text isn't a valid pattern: {m}")))?;
        let continuing = s < e && st.last == Some((s, e));
        let wraps = st.wrap == 1;
        if replace == 2 {
            let (lo, hi) = if continuing {
                (e, st.scope.1)
            } else if s < e {
                (s, e)
            } else if wraps {
                (0, len)
            } else if st.forward {
                (s, len)
            } else {
                (0, s)
            };
            let (pl, ph) = (w.pos_of(lo), w.pos_of(hi));
            let mine: Vec<(Pos, Pos, String)> = hits.into_iter().filter(|h| h.0 >= pl && h.1 <= ph && (h.0 < h.1 || keep_text)).collect();
            if mine.is_empty() {
                self.with_find(&tg, |f| f.found = false);
                return Ok(Value::Bool(false));
            }
            w.replace_found(&mine, keep_text, &fmt_repl)?;
            self.with_find(&tg, |f| {
                f.found = true;
                f.last = None;
            });
            return Ok(Value::Bool(true));
        }
        // The parts of the text to look in, in order.
        let (scope, origin, wrapped, regions): ((usize, usize), usize, bool, Vec<(usize, usize)>) = if continuing {
            let (scope, origin, wrapped) = (st.scope, st.origin, st.wrapped);
            let mut r = Vec::new();
            if st.forward {
                r.push((e, if wrapped { origin } else { scope.1 }));
                if !wrapped && wraps {
                    r.push((0, origin));
                }
            } else {
                r.push((if wrapped { origin } else { scope.0 }, s));
                if !wrapped && wraps {
                    r.push((origin, len));
                }
            }
            (scope, origin, wrapped, r)
        } else if s < e {
            // Within the range only: the next search goes on from the hit to
            // its end (forward) or its start (backward), never round.
            ((s, e), if st.forward { e } else { s }, true, vec![(s, e)])
        } else if st.forward {
            ((s, len), s, false, if wraps { vec![(s, len), (0, s)] } else { vec![(s, len)] })
        } else {
            ((0, s), s, false, if wraps { vec![(0, s), (s, len)] } else { vec![(0, s)] })
        };
        let mut pick = None;
        for (i, &(lo, hi)) in regions.iter().enumerate() {
            if lo > hi {
                continue;
            }
            let (pl, ph) = (w.pos_of(lo), w.pos_of(hi));
            let ok = |h: &&(Pos, Pos, String)| h.0 >= pl && h.1 <= ph && h.0 < h.1;
            let found = if st.forward { hits.iter().find(ok) } else { hits.iter().rev().find(ok) };
            if let Some(h) = found {
                pick = Some((h.clone(), i > 0));
                break;
            }
        }
        let Some((hit, went_round)) = pick else {
            self.with_find(&tg, |f| {
                f.found = false;
                f.last = None;
            });
            return Ok(Value::Bool(false));
        };
        let wrapped = wrapped || went_round;
        // Kept before replacing, which moves them with the text.
        self.with_find(&tg, |f| {
            f.scope = scope;
            f.origin = origin;
            f.wrapped = wrapped;
        });
        let w = self.w();
        let (hs, he) = if replace == 1 {
            w.replace_found(std::slice::from_ref(&hit), keep_text, &fmt_repl)?[0]
        } else {
            let at = w.chars_of(&[hit.0, hit.1]);
            (at[0], at[1])
        };
        self.set_span(&tg, hs, he);
        self.with_find(&tg, |f| {
            f.found = true;
            f.last = Some((hs, he));
        });
        Ok(Value::Bool(true))
    }
}
