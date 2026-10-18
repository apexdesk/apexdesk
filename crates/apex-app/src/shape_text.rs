//! Typing into a shape's text with its formatting: a caret and a selection
//! over the runs, formatting by parts (Ctrl+B/I/U, the ribbon), bullets and
//! numbers, copy and paste, undo, the mouse and dragging the selection.

use std::sync::Arc;

use apex_doc::{ShapeList, ShapePara, ShapeRun};
use eframe::egui::{self, Color32, Event, Key, Modifiers, Pos2, Rect, Stroke, vec2};
use egui::text::CCursor;

use crate::shapes::ParaBox;

/// A place in the text: a paragraph and a character in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct At {
    pub para: usize,
    pub ch: usize,
}

impl At {
    pub fn new(para: usize, ch: usize) -> At {
        At { para, ch }
    }
}

// --- the text --------------------------------------------------------------------------

fn len(p: &ShapePara) -> usize {
    p.runs.iter().map(|r| r.text.chars().count()).sum()
}

fn byte_of(s: &str, ch: usize) -> usize {
    s.char_indices().nth(ch).map_or(s.len(), |(i, _)| i)
}

/// Splits the runs of `p` at character `ch`; returns the index of the run
/// that starts there (`runs.len()` at the end).
fn split(p: &mut ShapePara, ch: usize) -> usize {
    let mut at = 0;
    for i in 0..p.runs.len() {
        let n = p.runs[i].text.chars().count();
        if ch == at {
            return i;
        }
        if ch < at + n {
            let b = byte_of(&p.runs[i].text, ch - at);
            let tail = p.runs[i].text.split_off(b);
            let r = p.runs[i].with_text(tail);
            p.runs.insert(i + 1, r);
            return i + 1;
        }
        at += n;
    }
    p.runs.len()
}

/// Joins runs formatted alike and drops empty ones (one stays, to keep the
/// format of an empty paragraph).
fn tidy(p: &mut ShapePara) {
    let keep = p.runs.first().map(|r| r.with_text("")).unwrap_or_default();
    let mut out: Vec<ShapeRun> = Vec::with_capacity(p.runs.len());
    for r in p.runs.drain(..).filter(|r| !r.text.is_empty()) {
        match out.last_mut() {
            Some(last) if last.with_text("") == r.with_text("") => last.text.push_str(&r.text),
            _ => out.push(r),
        }
    }
    if out.is_empty() {
        out.push(keep);
    }
    p.runs = out;
}

/// The format of text typed at `at`: that of the character before it (the
/// one after it at the start of a paragraph).
pub fn format_at(paras: &[ShapePara], at: At) -> ShapeRun {
    let Some(p) = paras.get(at.para) else { return paras.last().map(ShapePara::first_run).unwrap_or_default() };
    let mut n = 0;
    for r in p.runs.iter().filter(|r| !r.text.is_empty()) {
        n += r.text.chars().count();
        if n >= at.ch.max(1) {
            return r.with_text("");
        }
    }
    p.runs.last().map(|r| r.with_text("")).unwrap_or_default()
}

/// Deletes from `a` to `b` (a <= b).
fn delete(paras: &mut Vec<ShapePara>, a: At, b: At) {
    if a >= b {
        return;
    }
    let keep = format_at(paras, At::new(a.para, a.ch + 1));
    let tail = {
        let p = &mut paras[b.para];
        let i = split(p, b.ch);
        p.runs.split_off(i)
    };
    let p = &mut paras[a.para];
    let i = split(p, a.ch);
    p.runs.truncate(i);
    if p.runs.is_empty() {
        p.runs.push(keep);
    }
    p.runs.extend(tail);
    tidy(p);
    paras.drain(a.para + 1..=b.para);
}

/// The text from `a` to `b`, as paragraphs.
pub fn slice(paras: &[ShapePara], a: At, b: At) -> Vec<ShapePara> {
    let mut out: Vec<ShapePara> = paras[a.para..=b.para].to_vec();
    if let Some(last) = out.last_mut() {
        let i = split(last, b.ch);
        last.runs.truncate(i);
    }
    let first = &mut out[0];
    let i = split(first, a.ch);
    first.runs.drain(..i);
    for p in &mut out {
        if p.runs.is_empty() {
            p.runs.push(format_at(paras, a));
        }
        tidy(p);
    }
    out
}

/// Inserts `piece` at `at`: its first paragraph joins the one at `at`, its
/// last takes what follows `at`. Returns where it ends.
fn insert_paras(paras: &mut Vec<ShapePara>, at: At, piece: &[ShapePara]) -> At {
    let Some(first) = piece.first() else { return at };
    let p = &mut paras[at.para];
    let i = split(p, at.ch);
    let tail = p.runs.split_off(i);
    p.runs.extend(first.runs.iter().filter(|r| !r.text.is_empty()).cloned());
    let mut end = At::new(at.para, at.ch + len(first));
    if piece.len() == 1 {
        p.runs.extend(tail);
        tidy(p);
        return end;
    }
    tidy(p);
    let mut rest: Vec<ShapePara> = piece[1..].to_vec();
    let last = rest.last_mut().expect("two or more");
    end = At::new(at.para + piece.len() - 1, len(last));
    last.runs.extend(tail);
    tidy(last);
    paras.splice(at.para + 1..at.para + 1, rest);
    end
}

/// Inserts plain `text` ('\n' starts a paragraph) formatted as `fmt`.
fn insert(paras: &mut Vec<ShapePara>, at: At, text: &str, fmt: &ShapeRun) -> At {
    let piece: Vec<ShapePara> = text.split('\n').map(|l| ShapePara { align: Default::default(), runs: vec![fmt.with_text(l.strip_suffix('\r').unwrap_or(l))], list: None }).collect();
    let n = piece.len();
    let (align, list) = (paras[at.para].align, paras[at.para].list);
    let end = insert_paras(paras, at, &piece);
    // New paragraphs are like the one typed in.
    for p in &mut paras[at.para + 1..at.para + n] {
        p.align = align;
        p.list = list;
    }
    end
}

/// Applies `f` to the runs from `a` to `b`.
fn format_range(paras: &mut [ShapePara], a: At, b: At, f: &dyn Fn(&mut ShapeRun)) {
    for i in a.para..=b.para {
        let p = &mut paras[i];
        let from = if i == a.para { a.ch } else { 0 };
        let to = if i == b.para { b.ch } else { len(p) };
        if len(p) == 0 {
            p.runs.iter_mut().for_each(f);
            continue;
        }
        let s = split(p, from);
        let e = split(p, to);
        p.runs[s..e].iter_mut().for_each(f);
        tidy(p);
    }
}

pub fn plain(paras: &[ShapePara]) -> String {
    paras.iter().map(ShapePara::text).collect::<Vec<_>>().join("\n")
}

fn char_at(paras: &[ShapePara], at: At) -> Option<char> {
    paras.get(at.para)?.text().chars().nth(at.ch)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The start of the word before `at` (or of the one after, `forward`).
fn word_step(paras: &[ShapePara], at: At, forward: bool) -> At {
    let chars: Vec<char> = paras[at.para].text().chars().collect();
    let mut i = at.ch;
    if forward {
        while i < chars.len() && !is_word(chars[i]) {
            i += 1;
        }
        while i < chars.len() && is_word(chars[i]) {
            i += 1;
        }
    } else {
        while i > 0 && !is_word(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(chars[i - 1]) {
            i -= 1;
        }
    }
    At::new(at.para, i)
}

/// A tab is laid out as four characters (see `shapes::layout_para`).
fn shown(p: &ShapePara, ch: usize) -> usize {
    p.runs.iter().flat_map(|r| r.text.chars()).take(ch).map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

fn from_shown(p: &ShapePara, i: usize) -> usize {
    let mut n = 0;
    for (k, c) in p.runs.iter().flat_map(|r| r.text.chars()).enumerate() {
        let w = if c == '\t' { 4 } else { 1 };
        if n + w > i {
            return if i - n > w / 2 { k + 1 } else { k };
        }
        n += w;
    }
    len(p)
}

/// A run's format as a document's (for the ribbon and its buttons).
pub fn run_attrs(r: &ShapeRun) -> apex_doc::Attrs {
    apex_doc::Attrs {
        bold: r.bold,
        italic: r.italic,
        underline: r.underline,
        size: r.size.map(|pt| (pt * 2.0).round().clamp(1.0, 3200.0) as u16),
        color: r.color,
        font: r.font.clone(),
        text_fx: r.fx.clone(),
        ..Default::default()
    }
}

/// Takes the format of `a` into a run (what a shape's text keeps).
pub fn set_run_attrs(r: &mut ShapeRun, a: &apex_doc::Attrs) {
    r.bold = a.bold;
    r.italic = a.italic;
    r.underline = a.underline;
    r.size = a.size.map(|hp| hp as f32 / 2.0);
    r.color = a.color;
    r.font = a.font.clone();
    r.fx = a.text_fx.clone();
}

// --- the editor ------------------------------------------------------------------------

#[derive(Clone)]
struct Snap {
    text: Vec<ShapePara>,
    anchor: At,
    cursor: At,
}

/// Dragging the selection with the mouse.
#[derive(Clone, Copy)]
struct Drag {
    from: Pos2,
    moving: bool,
    to: Option<At>,
}

/// How far the pointer moves before a press in the selection drags it.
const DRAG_START: f32 = 4.0;

/// Typing into a shape: where the caret and selection are, and what to
/// undo.
#[derive(Default)]
pub struct RichEdit {
    pub anchor: At,
    pub cursor: At,
    /// Text typed to start editing, typed on the first frame.
    typed: Option<String>,
    /// First frame: takes the focus.
    fresh: bool,
    /// The format of what is typed next (Bold with no selection).
    pending: Option<ShapeRun>,
    /// Where Up and Down aim, on screen.
    desired_x: Option<f32>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    /// The last change was typing: more typing joins its undo step.
    typing: bool,
    /// The mouse is selecting.
    selecting: bool,
    drag: Option<Drag>,
    /// The text as last laid out, for the mouse, and its scale.
    boxes: Vec<ParaBox>,
    scale: f32,
}

/// What `paste` takes back as formatted text.
#[derive(Clone)]
struct Copied {
    plain: String,
    paras: Arc<Vec<ShapePara>>,
}

fn copied_id() -> egui::Id {
    egui::Id::new("shape-text-copied")
}

impl RichEdit {
    /// Starts with the caret at the end; `typed`: text typed to start.
    pub fn new(paras: &[ShapePara], typed: Option<&str>) -> RichEdit {
        let end = At::new(paras.len().saturating_sub(1), paras.last().map_or(0, len));
        RichEdit { anchor: end, cursor: end, typed: typed.map(str::to_string), fresh: true, ..Default::default() }
    }

    pub fn sorted(&self) -> (At, At) {
        if self.anchor <= self.cursor { (self.anchor, self.cursor) } else { (self.cursor, self.anchor) }
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.cursor
    }

    fn caret(&mut self, at: At, extend: bool) {
        self.cursor = at;
        if !extend {
            self.anchor = at;
        }
        self.pending = None;
        self.typing = false;
    }

    /// Keeps the caret and selection within `paras`.
    fn clamp(&mut self, paras: &[ShapePara]) {
        let fit = |a: At| {
            let para = a.para.min(paras.len().saturating_sub(1));
            At::new(para, a.ch.min(paras.get(para).map_or(0, len)))
        };
        self.anchor = fit(self.anchor);
        self.cursor = fit(self.cursor);
    }

    fn snap(&mut self, paras: &[ShapePara], typing: bool) {
        if typing && self.typing {
            return;
        }
        self.undo.push(Snap { text: paras.to_vec(), anchor: self.anchor, cursor: self.cursor });
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.typing = typing;
    }

    fn step(&mut self, paras: &mut Vec<ShapePara>, redo: bool) {
        let (from, to) = if redo { (&mut self.redo, &mut self.undo) } else { (&mut self.undo, &mut self.redo) };
        let Some(s) = from.pop() else { return };
        to.push(Snap { text: std::mem::replace(paras, s.text), anchor: self.anchor, cursor: self.cursor });
        (self.anchor, self.cursor) = (s.anchor, s.cursor);
        self.typing = false;
        self.pending = None;
    }

    /// The format of what is typed next.
    pub fn format_now(&self, paras: &[ShapePara]) -> ShapeRun {
        if let Some(p) = &self.pending {
            return p.clone();
        }
        let (a, b) = self.sorted();
        // A selection shows the format of its first character.
        if a != b { format_at(paras, At::new(a.para, a.ch + 1)) } else { format_at(paras, a) }
    }

    /// Types `text` over the selection.
    pub fn type_text(&mut self, paras: &mut Vec<ShapePara>, text: &str) {
        if paras.is_empty() {
            paras.push(ShapePara { runs: vec![ShapeRun::default()], ..Default::default() });
        }
        self.snap(paras, !text.contains('\n') && !self.has_selection());
        let fmt = self.format_now(paras);
        let (a, b) = self.sorted();
        delete(paras, a, b);
        let end = insert(paras, a, text, &fmt);
        let typing = self.typing;
        self.caret(end, false);
        self.typing = typing;
        self.desired_x = None;
    }

    fn insert_rich(&mut self, paras: &mut Vec<ShapePara>, piece: &[ShapePara]) {
        self.snap(paras, false);
        let (a, b) = self.sorted();
        delete(paras, a, b);
        let end = insert_paras(paras, a, piece);
        self.caret(end, false);
    }

    fn delete_selection(&mut self, paras: &mut Vec<ShapePara>) {
        self.snap(paras, false);
        let (a, b) = self.sorted();
        delete(paras, a, b);
        self.caret(a, false);
    }

    /// Applies `f` to the selected text, or to what is typed next.
    pub fn format(&mut self, paras: &mut Vec<ShapePara>, f: &dyn Fn(&mut ShapeRun)) {
        if !self.has_selection() {
            let mut p = self.format_now(paras);
            f(&mut p);
            let typing = self.typing;
            self.pending = (p != format_at(paras, self.cursor)).then_some(p);
            self.typing = typing;
            return;
        }
        self.snap(paras, false);
        let (a, b) = self.sorted();
        format_range(paras, a, b, f);
    }

    /// Turns a format on for the selection, or off when its first
    /// character has it.
    pub fn toggle(&mut self, paras: &mut Vec<ShapePara>, get: fn(&ShapeRun) -> bool, set: fn(&mut ShapeRun, bool)) {
        let on = !get(&self.format_now(paras));
        self.format(paras, &move |r| set(r, on));
    }

    /// The paragraphs the selection touches.
    fn paras_range(&self) -> std::ops::RangeInclusive<usize> {
        let (a, b) = self.sorted();
        a.para..=b.para
    }

    /// Bullets or numbers for the selected paragraphs; off when the first
    /// already is such a list.
    pub fn toggle_list(&mut self, paras: &mut Vec<ShapePara>, numbered: bool) {
        if paras.is_empty() {
            return;
        }
        self.snap(paras, false);
        let r = self.paras_range();
        let off = paras[*r.start()].list.is_some_and(|l| l.numbered == numbered);
        for p in &mut paras[r] {
            p.list = if off { None } else { Some(ShapeList { numbered, level: p.list.map_or(0, |l| l.level) }) };
        }
    }

    /// The list of the paragraph at the caret.
    pub fn list_now(&self, paras: &[ShapePara]) -> Option<ShapeList> {
        paras.get(self.sorted().0.para).and_then(|p| p.list)
    }

    /// One level deeper (or back) for the selected list paragraphs.
    pub fn indent(&mut self, paras: &mut Vec<ShapePara>, deeper: bool) {
        self.snap(paras, false);
        for p in &mut paras[self.paras_range()] {
            if let Some(l) = &mut p.list {
                l.level = if deeper { (l.level + 1).min(ShapeList::MAX_LEVEL) } else { l.level.saturating_sub(1) };
            }
        }
    }

    /// Changes the paragraphs the selection touches.
    pub fn each_para(&mut self, paras: &mut Vec<ShapePara>, f: &dyn Fn(&mut ShapePara)) {
        if paras.is_empty() {
            return;
        }
        self.clamp(paras);
        self.snap(paras, false);
        paras[self.paras_range()].iter_mut().for_each(f);
    }

    /// Applies a document's format change to the selection.
    pub fn format_attrs(&mut self, paras: &mut Vec<ShapePara>, f: &dyn Fn(&mut apex_doc::Attrs)) {
        self.format(paras, &|r| {
            let mut a = run_attrs(r);
            f(&mut a);
            set_run_attrs(r, &a);
        });
    }

    pub fn select_all(&mut self, paras: &[ShapePara]) {
        self.anchor = At::default();
        self.cursor = At::new(paras.len().saturating_sub(1), paras.last().map_or(0, len));
    }

    fn selected(&self, paras: &[ShapePara]) -> Option<(String, Vec<ShapePara>)> {
        let (a, b) = self.sorted();
        if a == b {
            return None;
        }
        let piece = slice(paras, a, b);
        Some((plain(&piece), piece))
    }

    fn copy(&self, ctx: &egui::Context, paras: &[ShapePara]) {
        if let Some((text, piece)) = self.selected(paras) {
            ctx.copy_text(text.clone());
            ctx.data_mut(|d| d.insert_temp(copied_id(), Copied { plain: text, paras: Arc::new(piece) }));
        }
    }

    fn paste(&mut self, ctx: &egui::Context, paras: &mut Vec<ShapePara>, text: &str) {
        let text = text.replace("\r\n", "\n");
        match ctx.data(|d| d.get_temp::<Copied>(copied_id())).filter(|c| c.plain == text) {
            Some(c) => self.insert_rich(paras, &c.paras),
            None => {
                // A step of its own, not joined to typing.
                self.typing = false;
                self.type_text(paras, &text);
                self.typing = false;
            }
        }
    }

    fn backspace(&mut self, paras: &mut Vec<ShapePara>, word: bool) {
        if self.has_selection() {
            return self.delete_selection(paras);
        }
        let c = self.cursor;
        if c.ch == 0 {
            // At the start of a list item: the bullet goes first.
            if paras[c.para].list.is_some() {
                self.snap(paras, false);
                paras[c.para].list = None;
                return;
            }
            if c.para == 0 {
                return;
            }
        }
        let from = if c.ch == 0 { At::new(c.para - 1, len(&paras[c.para - 1])) } else if word { word_step(paras, c, false) } else { At::new(c.para, c.ch - 1) };
        self.snap(paras, !word && c.ch > 0);
        let typing = self.typing;
        delete(paras, from, c);
        self.caret(from, false);
        self.typing = typing;
    }

    fn delete_forward(&mut self, paras: &mut Vec<ShapePara>, word: bool) {
        if self.has_selection() {
            return self.delete_selection(paras);
        }
        let c = self.cursor;
        let n = len(&paras[c.para]);
        let to = if c.ch == n {
            if c.para + 1 >= paras.len() {
                return;
            }
            At::new(c.para + 1, 0)
        } else if word {
            word_step(paras, c, true)
        } else {
            At::new(c.para, c.ch + 1)
        };
        self.snap(paras, false);
        delete(paras, c, to);
        self.caret(c, false);
    }

    fn enter(&mut self, paras: &mut Vec<ShapePara>) {
        let c = self.cursor;
        // Enter in an empty list item ends the list.
        if !self.has_selection() && paras[c.para].list.is_some() && len(&paras[c.para]) == 0 {
            self.snap(paras, false);
            paras[c.para].list = None;
            return;
        }
        self.type_text(paras, "\n");
    }

    fn left_right(&mut self, paras: &[ShapePara], right: bool, m: Modifiers) {
        let (a, b) = self.sorted();
        if self.has_selection() && !m.shift {
            return self.caret(if right { b } else { a }, false);
        }
        let c = self.cursor;
        let to = if right {
            if c.ch < len(&paras[c.para]) {
                if m.command || m.alt { word_step(paras, c, true) } else { At::new(c.para, c.ch + 1) }
            } else if c.para + 1 < paras.len() {
                At::new(c.para + 1, 0)
            } else {
                c
            }
        } else if c.ch > 0 {
            if m.command || m.alt { word_step(paras, c, false) } else { At::new(c.para, c.ch - 1) }
        } else if c.para > 0 {
            At::new(c.para - 1, len(&paras[c.para - 1]))
        } else {
            c
        };
        self.caret(to, m.shift);
        self.desired_x = None;
    }

    /// The caret's rectangle on screen.
    fn caret_rect(&self, paras: &[ShapePara], at: At) -> Option<Rect> {
        let b = self.boxes.get(at.para)?;
        let r = b.galley.pos_from_cursor(CCursor::new(shown(paras.get(at.para)?, at.ch)));
        Some(r.translate(b.pos.to_vec2()))
    }

    /// The character under screen point `q`.
    fn at_pos(&self, paras: &[ShapePara], q: Pos2) -> At {
        let n = self.boxes.len().min(paras.len());
        if n == 0 {
            return At::default();
        }
        let i = (0..n).find(|&i| q.y < self.boxes[i].pos.y + self.boxes[i].galley.rect.max.y).unwrap_or(n - 1);
        let b = &self.boxes[i];
        let c = b.galley.cursor_from_pos(q - b.pos);
        At::new(i, from_shown(&paras[i], c.index.0))
    }

    fn up_down(&mut self, paras: &[ShapePara], down: bool, shift: bool) {
        let c = self.cursor;
        let (Some(b), Some(cr)) = (self.boxes.get(c.para), self.caret_rect(paras, c)) else { return };
        let x = *self.desired_x.get_or_insert(cr.center().x);
        let cur = CCursor::new(shown(&paras[c.para], c.ch));
        let (moved, _) = if down { b.galley.cursor_down_one_row(&cur, Some(x - b.pos.x)) } else { b.galley.cursor_up_one_row(&cur, Some(x - b.pos.x)) };
        let rows = b.galley.rows.len();
        let row = b.galley.rows.iter().position(|r| r.rect().translate(b.pos.to_vec2()).bottom() > cr.center().y).unwrap_or(rows.saturating_sub(1));
        let to = if (down && row + 1 >= rows) || (!down && row == 0) {
            // Into the next or previous paragraph, at the same x.
            let next = if down { c.para + 1 } else { c.para.wrapping_sub(1) };
            match self.boxes.get(next) {
                Some(nb) if next < paras.len() => {
                    let y = if down { nb.galley.rows.first() } else { nb.galley.rows.last() }.map_or(0.0, |r| r.rect().center().y);
                    At::new(next, from_shown(&paras[next], nb.galley.cursor_from_pos(vec2(x - nb.pos.x, y)).index.0))
                }
                _ if down => At::new(c.para, len(&paras[c.para])),
                _ => At::new(c.para, 0),
            }
        } else {
            At::new(c.para, from_shown(&paras[c.para], moved.index.0))
        };
        let keep = self.desired_x;
        self.caret(to, shift);
        self.desired_x = keep;
    }

    fn home_end(&mut self, paras: &[ShapePara], end: bool, m: Modifiers) {
        let c = self.cursor;
        let to = if m.command {
            if end { At::new(paras.len() - 1, len(&paras[paras.len() - 1])) } else { At::default() }
        } else {
            match self.boxes.get(c.para) {
                Some(b) => {
                    let cur = CCursor::new(shown(&paras[c.para], c.ch));
                    let k = if end { b.galley.cursor_end_of_row(&cur) } else { b.galley.cursor_begin_of_row(&cur) };
                    At::new(c.para, from_shown(&paras[c.para], k.index.0))
                }
                None => At::new(c.para, if end { len(&paras[c.para]) } else { 0 }),
            }
        };
        self.caret(to, m.shift);
        self.desired_x = None;
    }

    /// One key; false when it isn't the editor's. Escape is the caller's.
    pub fn key(&mut self, ctx: &egui::Context, paras: &mut Vec<ShapePara>, key: Key, m: Modifiers) -> bool {
        if paras.is_empty() {
            paras.push(ShapePara { runs: vec![ShapeRun::default()], ..Default::default() });
        }
        self.clamp(paras);
        match key {
            Key::Backspace => self.backspace(paras, m.command || m.alt),
            Key::Delete => self.delete_forward(paras, m.command || m.alt),
            Key::Enter => self.enter(paras),
            Key::Tab if self.list_now(paras).is_some() => self.indent(paras, !m.shift),
            Key::Tab if !m.shift => self.type_text(paras, "\t"),
            Key::ArrowLeft | Key::ArrowRight => self.left_right(paras, key == Key::ArrowRight, m),
            Key::ArrowUp | Key::ArrowDown => self.up_down(paras, key == Key::ArrowDown, m.shift),
            Key::Home | Key::End => self.home_end(paras, key == Key::End, m),
            Key::A if m.command => self.select_all(paras),
            Key::B if m.command => self.toggle(paras, |r| r.bold, |r, v| r.bold = v),
            Key::I if m.command => self.toggle(paras, |r| r.italic, |r, v| r.italic = v),
            Key::U if m.command => self.toggle(paras, |r| r.underline, |r, v| r.underline = v),
            Key::Z if m.command => self.step(paras, m.shift),
            Key::Y if m.command => self.step(paras, true),
            _ => {
                let _ = ctx;
                return false;
            }
        }
        true
    }

    /// The editor's input of a frame: typing, keys, clipboard and IME.
    /// Returns whether Escape was pressed.
    fn input(&mut self, ctx: &egui::Context, paras: &mut Vec<ShapePara>) -> bool {
        let events = ctx.input(|i| i.filtered_events(&egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        let mut escape = false;
        for ev in events {
            match ev {
                Event::Text(t) if !t.is_empty() && t != "\n" && t != "\r" => self.type_text(paras, &t),
                Event::Ime(egui::ImeEvent::Commit(t)) if !t.is_empty() => self.type_text(paras, &t),
                Event::Paste(t) => self.paste(ctx, paras, &t),
                Event::Copy => self.copy(ctx, paras),
                Event::Cut => {
                    self.copy(ctx, paras);
                    if self.has_selection() {
                        self.delete_selection(paras);
                    }
                }
                Event::Key { key: Key::Escape, pressed: true, .. } => escape = true,
                Event::Key { key, pressed: true, modifiers, .. } => {
                    self.key(ctx, paras, key, modifiers);
                }
                _ => {}
            }
        }
        escape
    }

    /// Mouse on the text: placing the caret, selecting, double click for a
    /// word, dragging the selection.
    fn mouse(&mut self, ui: &egui::Ui, resp: &egui::Response, paras: &mut Vec<ShapePara>) {
        let (pressed, down, at, clicks, shift, copy) = ui.input(|i| {
            let p = &i.pointer;
            (p.primary_pressed(), p.primary_down(), p.interact_pos(), if p.button_triple_clicked(egui::PointerButton::Primary) { 3 } else if p.button_double_clicked(egui::PointerButton::Primary) { 2 } else { 1 }, i.modifiers.shift, crate::doc_view::text_drag::copy_modifier(&i.modifiers))
        });
        let Some(q) = at else { return };
        if pressed && resp.hovered() {
            let hit = self.at_pos(paras, q);
            let (a, b) = self.sorted();
            if clicks == 1 && !shift && a != b && hit >= a && hit < b {
                self.drag = Some(Drag { from: q, moving: false, to: None });
                return;
            }
            match clicks {
                2 => {
                    self.anchor = word_step(paras, At::new(hit.para, hit.ch + usize::from(char_at(paras, hit).is_some_and(is_word))), false);
                    self.cursor = word_step(paras, hit, true);
                }
                3 => {
                    self.anchor = At::new(hit.para, 0);
                    self.cursor = At::new(hit.para, len(&paras[hit.para]));
                }
                _ => self.caret(hit, shift),
            }
            self.selecting = clicks == 1;
            self.desired_x = None;
            return;
        }
        if let Some(d) = &mut self.drag {
            if d.moving && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.drag = None;
                return;
            }
            if down {
                d.moving |= (q - d.from).length() > DRAG_START;
                if d.moving {
                    ui.ctx().set_cursor_icon(if copy { egui::CursorIcon::Copy } else { egui::CursorIcon::Grabbing });
                    let to = self.at_pos(paras, q);
                    if let Some(d) = &mut self.drag {
                        d.to = Some(to);
                    }
                    ui.ctx().request_repaint();
                }
                return;
            }
            let d = self.drag.take().expect("dragging");
            match d.to.filter(|_| d.moving) {
                Some(to) => self.drop_text(paras, to, copy),
                None => {
                    let hit = self.at_pos(paras, d.from);
                    self.caret(hit, false);
                }
            }
            return;
        }
        if self.selecting {
            if down {
                let hit = self.at_pos(paras, q);
                self.cursor = hit;
                ui.ctx().request_repaint();
            } else {
                self.selecting = false;
            }
        }
    }

    /// Moves (or copies) the selection to `to`, as one undo step.
    pub fn drop_text(&mut self, paras: &mut Vec<ShapePara>, to: At, copy: bool) {
        let (a, b) = self.sorted();
        if (to > a && to < b) || (!copy && (to == a || to == b)) {
            return;
        }
        self.snap(paras, false);
        let piece = slice(paras, a, b);
        let n = piece.len();
        let last_len = piece.last().map_or(0, len);
        if to >= b {
            // Inserted after the selection: what follows the cut keeps its
            // distance to the end of its paragraph.
            let end = insert_paras(paras, to, &piece);
            if copy {
                (self.anchor, self.cursor) = (to, end);
                return;
            }
            let from_end = len(&paras[end.para]) - end.ch;
            delete(paras, a, b);
            let shift = b.para - a.para;
            let end_para = end.para - shift;
            let end = At::new(end_para, len(&paras[end_para]) - from_end);
            let start = if n == 1 { At::new(end.para, end.ch - last_len) } else { At::new(end.para + 1 - n, len(&paras[end.para + 1 - n]) - len(&piece[0])) };
            (self.anchor, self.cursor) = (start, end);
        } else {
            if !copy {
                delete(paras, a, b);
            }
            let end = insert_paras(paras, to, &piece);
            (self.anchor, self.cursor) = (to, end);
        }
        self.typing = false;
    }
}

// --- on screen -------------------------------------------------------------------------

/// The editor over a shape drawn in `rect` (screen, unturned), its text
/// laid out as `shapes::place_text` does. Returns whether editing is
/// done (Escape or a click elsewhere).
pub fn editor(ui: &mut egui::Ui, id: egui::Id, rect: Rect, shape: &mut apex_doc::Shape, e: &mut RichEdit, scale: f32) -> bool {
    let ctx = ui.ctx().clone();
    if shape.text.is_empty() {
        let fmt = shape.text_format();
        shape.text.push(ShapePara { align: shape.text_align(), runs: vec![fmt], list: None });
    }
    e.clamp(&shape.text);
    e.scale = scale;
    if let Some(t) = e.typed.take() {
        e.type_text(&mut shape.text, &t);
    }
    // Until the editor has it (an area takes no focus on its first frame),
    // and back after a button of the ribbon took it: only a field typed in
    // (the font size) keeps it.
    let now = ctx.memory(|m| m.focused());
    if e.fresh || (now != Some(id) && now.is_none_or(|f| egui::TextEdit::load_state(&ctx, f).is_none())) {
        ctx.memory_mut(|m| m.request_focus(id));
    }
    let focused = ctx.memory(|m| m.has_focus(id));
    let mut escape = false;
    if focused {
        ctx.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        escape = e.input(&ctx, &mut shape.text);
    }
    let clip = ui.clip_rect();
    let area = crate::shapes::text_rect(shape, rect, scale);
    let out = egui::Area::new(id.with("area")).fixed_pos(rect.min).order(egui::Order::Foreground).show(&ctx, |ui| {
        ui.set_clip_rect(clip);
        let resp = ui.interact(rect.union(area), id, egui::Sense::click_and_drag());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        e.boxes = crate::shapes::place_text(ui.painter(), shape, area, scale);
        e.mouse(ui, &resp, &mut shape.text);
        if resp.clicked() || resp.drag_started() {
            resp.request_focus();
        }
        resp.context_menu(|ui| menu(ui, &mut shape.text, e));
        // Whatever the mouse changed is drawn as it is now.
        e.clamp(&shape.text);
        e.boxes = crate::shapes::place_text(ui.painter(), shape, area, scale);
        paint(ui, shape, e);
        resp
    });
    let resp = out.inner;
    e.fresh &= !resp.has_focus();
    // A click elsewhere on the page (not on the ribbon or a menu) is done.
    let pressed = ui.input(|i| i.pointer.any_pressed().then(|| i.pointer.interact_pos()).flatten());
    let clicked_away = pressed.is_some_and(|q| clip.contains(q) && !resp.contains_pointer() && ctx.layer_id_at(q) == Some(ui.layer_id()));
    escape || clicked_away
}

/// The context menu of the text being typed.
fn menu(ui: &mut egui::Ui, paras: &mut Vec<ShapePara>, e: &mut RichEdit) {
    use crate::i18n::tr;
    let ctx = ui.ctx().clone();
    let has = e.has_selection();
    if ui.add_enabled(has, egui::Button::new(tr("edit.cut")).shortcut_text("Ctrl+X")).clicked() {
        e.copy(&ctx, paras);
        e.delete_selection(paras);
        ui.close();
    }
    if ui.add_enabled(has, egui::Button::new(tr("edit.copy")).shortcut_text("Ctrl+C")).clicked() {
        e.copy(&ctx, paras);
        ui.close();
    }
    if ui.add(egui::Button::new(tr("edit.paste")).shortcut_text("Ctrl+V")).clicked() {
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        ui.close();
    }
    if ui.add(egui::Button::new(tr("edit.select_all")).shortcut_text("Ctrl+A")).clicked() {
        e.select_all(paras);
        ui.close();
    }
    ui.separator();
    let f = e.format_now(paras);
    type Toggle = (&'static str, &'static str, bool, fn(&ShapeRun) -> bool, fn(&mut ShapeRun, bool));
    let toggles: [Toggle; 3] = [
        ("format.bold", "Ctrl+B", f.bold, |r| r.bold, |r, v| r.bold = v),
        ("format.italic", "Ctrl+I", f.italic, |r| r.italic, |r, v| r.italic = v),
        ("format.underline", "Ctrl+U", f.underline, |r| r.underline, |r, v| r.underline = v),
    ];
    for (key, keys, on, get, set) in toggles {
        if ui.add(egui::Button::selectable(on, tr(key)).shortcut_text(keys)).clicked() {
            e.toggle(paras, get, set);
            ui.close();
        }
    }
    ui.separator();
    let list = e.list_now(paras);
    for (key, numbered) in [("doc.bullets", false), ("doc.numbering", true)] {
        if ui.add(egui::Button::selectable(list.is_some_and(|l| l.numbered == numbered), tr(key))).clicked() {
            e.toggle_list(paras, numbered);
            ui.close();
        }
    }
    if list.is_some() {
        if ui.add(egui::Button::new(tr("doc.indent")).shortcut_text("Tab")).clicked() {
            e.indent(paras, true);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("doc.outdent")).shortcut_text("Shift+Tab")).clicked() {
            e.indent(paras, false);
            ui.close();
        }
    }
}

/// The selection, the text and the caret.
fn paint(ui: &egui::Ui, shape: &apex_doc::Shape, e: &RichEdit) {
    let p = ui.painter();
    let paras = &shape.text;
    let (a, b) = e.sorted();
    let sel_color = ui.visuals().selection.bg_fill.gamma_multiply(0.6);
    if a != b {
        for (i, bx) in e.boxes.iter().enumerate().take(paras.len()).skip(a.para).take(b.para - a.para + 1) {
            let from = if i == a.para { shown(&paras[i], a.ch) } else { 0 };
            let to = if i == b.para { shown(&paras[i], b.ch) } else { usize::MAX };
            let mut start = 0;
            for row in &bx.galley.rows {
                let n = row.row.glyphs.len();
                let (s, t) = (from.max(start), to.min(start + n));
                let x = |j: usize| row.pos.x + row.row.glyphs.get(j - start).map_or_else(|| row.row.glyphs.last().map_or(0.0, |g| g.max_x()), |g| g.pos.x);
                if s < t || (s == t && to > start + n && s == start + n) {
                    let mut r = Rect::from_x_y_ranges(x(s)..=x(t), row.rect().y_range()).translate(bx.pos.to_vec2());
                    if to > start + n {
                        // The paragraph's end is selected too.
                        r.max.x += 4.0;
                    }
                    p.rect_filled(r, 0.0, sel_color);
                }
                start += n;
            }
        }
    }
    crate::shapes::paint_boxes(p, shape, &e.boxes, e.scale, None);
    // Where dragged text would land.
    if let Some(to) = e.drag.filter(|d| d.moving).and_then(|d| d.to)
        && !(to > a && to < b)
        && let Some(r) = e.caret_rect(paras, to)
    {
        p.vline(r.min.x.round() + 0.5, r.y_range(), Stroke::new(2.0, ui.visuals().text_color()));
    }
    let time = ui.input(|i| i.time);
    let blink = (time * 2.0).fract() < 0.6;
    if !e.has_selection()
        && let Some(r) = e.caret_rect(paras, e.cursor)
    {
        if blink {
            let color = crate::theme::rgb(e.format_now(paras).color.unwrap_or_else(|| shape.auto_text_color()));
            p.vline(r.min.x.round() + 0.5, r.y_range(), Stroke::new(1.5, if color == Color32::WHITE { Color32::WHITE } else { color }));
        }
        ui.ctx().output_mut(|o| o.ime = Some(egui::output::IMEOutput { purpose: Default::default(), rect: r, cursor_rect: r, should_interrupt_composition: false }));
        ui.ctx().request_repaint_after(web_time::Duration::from_millis(400));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paras(s: &str) -> Vec<ShapePara> {
        s.split('\n').map(|l| ShapePara { runs: vec![ShapeRun::new(l)], ..Default::default() }).collect()
    }

    fn runs(p: &ShapePara) -> Vec<(String, bool)> {
        p.runs.iter().map(|r| (r.text.clone(), r.bold)).collect()
    }

    #[test]
    fn formats_parts_and_types_with_them() {
        let mut t = paras("one two three");
        let mut e = RichEdit::new(&t, None);
        (e.anchor, e.cursor) = (At::new(0, 4), At::new(0, 7));
        e.toggle(&mut t, |r| r.bold, |r, v| r.bold = v);
        assert_eq!(runs(&t[0]), vec![("one ".into(), false), ("two".into(), true), (" three".into(), false)]);
        // Typed after the bold word: bold.
        e.caret(At::new(0, 7), false);
        e.type_text(&mut t, "s");
        assert_eq!(runs(&t[0])[1], ("twos".into(), true));
        // Bold off with no selection: what comes next isn't.
        e.toggle(&mut t, |r| r.bold, |r, v| r.bold = v);
        e.type_text(&mut t, "!");
        assert_eq!(plain(&t), "one twos! three");
        assert!(!t[0].runs[2].bold);
        // Typing is one undo step; the format another.
        e.step(&mut t, false);
        assert_eq!(plain(&t), "one two three");
        e.step(&mut t, false);
        assert_eq!(runs(&t[0]), vec![("one two three".into(), false)]);
        e.step(&mut t, true);
        assert!(t[0].runs[1].bold);
    }

    #[test]
    fn paragraphs_break_join_and_keep_their_lists() {
        let mut t = paras("first");
        t[0].list = Some(ShapeList::bullet(0));
        let mut e = RichEdit::new(&t, None);
        let ctx = egui::Context::default();
        e.key(&ctx, &mut t, Key::Enter, Modifiers::NONE);
        e.type_text(&mut t, "second");
        assert_eq!(t.len(), 2);
        assert_eq!(t[1].list, Some(ShapeList::bullet(0)));
        e.key(&ctx, &mut t, Key::Tab, Modifiers::NONE);
        assert_eq!(t[1].list, Some(ShapeList::bullet(1)));
        e.key(&ctx, &mut t, Key::Tab, Modifiers::SHIFT);
        // Enter on an empty item ends the list.
        e.key(&ctx, &mut t, Key::Enter, Modifiers::NONE);
        e.key(&ctx, &mut t, Key::Enter, Modifiers::NONE);
        assert_eq!((t.len(), t[2].list), (3, None));
        // Backspace at the start: joins with the paragraph before.
        e.key(&ctx, &mut t, Key::Backspace, Modifiers::NONE);
        assert_eq!(plain(&t), "first\nsecond");
        assert_eq!(e.cursor, At::new(1, 6));
        // Numbers for both, then off again.
        e.select_all(&t);
        e.toggle_list(&mut t, true);
        assert!(t.iter().all(|p| p.list == Some(ShapeList::number(0))));
        e.toggle_list(&mut t, true);
        assert!(t.iter().all(|p| p.list.is_none()));
    }

    #[test]
    fn deletes_and_pastes_across_paragraphs() {
        let mut t = paras("alpha\nbeta\ngamma");
        t[1].runs[0].bold = true;
        let piece = slice(&t, At::new(0, 3), At::new(1, 2));
        assert_eq!(plain(&piece), "ha\nbe");
        assert!(piece[1].runs[0].bold);
        delete(&mut t, At::new(0, 3), At::new(1, 2));
        assert_eq!(plain(&t), "alpta\ngamma");
        assert_eq!(runs(&t[0]), vec![("alp".into(), false), ("ta".into(), true)]);
        let end = insert_paras(&mut t, At::new(1, 2), &piece);
        assert_eq!(plain(&t), "alpta\ngaha\nbemma");
        assert_eq!(end, At::new(2, 2));
    }

    #[test]
    fn drags_text_between_paragraphs() {
        let mut t = paras("one two\nthree");
        let mut e = RichEdit::new(&t, None);
        (e.anchor, e.cursor) = (At::new(0, 0), At::new(0, 3));
        e.drop_text(&mut t, At::new(1, 5), false);
        assert_eq!(plain(&t), " two\nthreeone");
        assert_eq!(slice(&t, e.anchor, e.cursor)[0].text(), "one");
        (e.anchor, e.cursor) = (At::new(1, 5), At::new(1, 8));
        e.drop_text(&mut t, At::new(0, 0), true);
        assert_eq!(plain(&t), "one two\nthreeone");
    }
}
