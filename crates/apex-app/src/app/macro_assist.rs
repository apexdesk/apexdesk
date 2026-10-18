//! The macro editor's help while typing, as in the VBA editor: Enter keeps
//! the indent and closes blocks, Tab indents lines, known words take their
//! case when the line is left, and the list of members after a dot
//! (Ctrl+Space: of every name) and the parameters of the call being typed.

use eframe::egui::{self, RichText, text::CCursor, text::CCursorRange};

use super::vba_edit::{self, Item, Scope};
use apex_vba::catalog::Kind;

/// What the code being edited can see of its project.
pub(super) struct Info {
    /// A document's macros: Word's object model.
    pub word: bool,
    pub me: Option<&'static str>,
    pub objects: Vec<(String, &'static str)>,
    pub procs: Vec<(String, String)>,
    pub classes: Vec<String>,
    /// Every module's code, for the names the project declares.
    pub sources: Vec<String>,
}

impl Info {
    fn scope<'a>(&self, code: &'a str) -> Scope<'a> {
        Scope { code, word: self.word, me: self.me, objects: self.objects.clone(), procs: self.procs.clone(), classes: self.classes.clone() }
    }
}

#[derive(Default)]
pub(super) struct Assist {
    popup: Option<Popup>,
    /// The row typed in last, to write in case when the cursor leaves it.
    dirty_row: Option<usize>,
    /// A character typed this frame.
    typed: Option<char>,
}

struct Popup {
    start: usize,
    items: Vec<Item>,
    sel: usize,
    forced: bool,
    /// The list moved: scroll to the item picked.
    scroll: bool,
}

fn range(ctx: &egui::Context, id: egui::Id) -> (usize, usize) {
    let state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state.cursor.char_range().map_or((0, 0), |r| (r.primary.index.into(), r.secondary.index.into()))
}

/// The cursor's position in the code `id`.
pub(super) fn cursor(ctx: &egui::Context, id: egui::Id) -> usize {
    range(ctx, id).0
}

/// The cursor and the other end of the selection.
pub(super) fn cursor_range(ctx: &egui::Context, id: egui::Id) -> (usize, usize) {
    range(ctx, id)
}

/// Selects characters `a..b` of the code `id`.
pub(super) fn select(ctx: &egui::Context, id: egui::Id, a: usize, b: usize) {
    set_range(ctx, id, b, a);
}

fn set_range(ctx: &egui::Context, id: egui::Id, primary: usize, secondary: usize) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(secondary), CCursor::new(primary))));
    state.store(ctx, id);
}

fn row_of(code: &str, at: usize) -> usize {
    code.chars().take(at).filter(|c| *c == '\n').count()
}

/// The line `row` with its words in their case; whether it changed.
fn normalize_row(code: &mut String, row: usize, info: &Info) -> bool {
    let Some(line) = code.split('\n').nth(row) else { return false };
    let sources: Vec<&str> = info.sources.iter().map(String::as_str).collect();
    let new = vba_edit::normalize_line(line, &vba_edit::declared(&sources));
    if new == line {
        return false;
    }
    let mut lines: Vec<&str> = code.split('\n').collect();
    lines[row] = &new;
    *code = lines.join("\n");
    true
}

fn accept(ui: &egui::Ui, id: egui::Id, code: &mut String, p: &Popup, at: usize) {
    let Some(item) = p.items.get(p.sel) else { return };
    let a = code.char_indices().nth(p.start).map_or(code.len(), |(i, _)| i);
    let b = code.char_indices().nth(at).map_or(code.len(), |(i, _)| i);
    // The rest of the word under the cursor goes too.
    let end = b + code[b..].find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(code.len() - b);
    code.replace_range(a..end, &item.name);
    let to = p.start + item.name.chars().count();
    set_range(ui.ctx(), id, to, to);
}

/// Before the code takes the keys: the list's keys, Enter, Tab and
/// Ctrl+Space. Returns whether the code changed.
pub(super) fn before(ui: &mut egui::Ui, id: egui::Id, code: &mut String, a: &mut Assist, info: &Info) -> bool {
    use egui::{Key, Modifiers as M};
    a.typed = ui.input(|i| {
        i.events.iter().rev().find_map(|e| match e {
            egui::Event::Text(t) => t.chars().last(),
            _ => None,
        })
    });
    let (at, other) = range(ui.ctx(), id);
    if let Some(p) = &mut a.popup {
        let n = p.items.len();
        if ui.input_mut(|i| i.consume_key(M::NONE, Key::ArrowDown)) {
            p.sel = (p.sel + 1).min(n.saturating_sub(1));
            p.scroll = true;
        }
        if ui.input_mut(|i| i.consume_key(M::NONE, Key::ArrowUp)) {
            p.sel = p.sel.saturating_sub(1);
            p.scroll = true;
        }
        for (key, step) in [(Key::PageDown, 8i64), (Key::PageUp, -8)] {
            if ui.input_mut(|i| i.consume_key(M::NONE, key)) {
                p.sel = (p.sel as i64 + step).clamp(0, n.saturating_sub(1) as i64) as usize;
                p.scroll = true;
            }
        }
        if ui.input_mut(|i| i.consume_key(M::NONE, Key::Escape)) {
            a.popup = None;
            return false;
        }
        if ui.input_mut(|i| i.consume_key(M::NONE, Key::Tab) || i.consume_key(M::NONE, Key::Enter)) {
            accept(ui, id, code, p, at);
            a.popup = None;
            a.dirty_row = Some(row_of(code, at));
            return true;
        }
        return false;
    }
    if ui.input_mut(|i| i.consume_key(M::COMMAND, Key::Space)) {
        if let Some((start, items)) = vba_edit::complete(&info.scope(code), at, true).filter(|c| !c.1.is_empty()) {
            // The only name that fits goes in at once.
            if items.len() == 1 {
                let p = Popup { start, items, sel: 0, forced: true, scroll: false };
                accept(ui, id, code, &p, at);
                a.dirty_row = Some(row_of(code, at));
                return true;
            }
            a.popup = Some(Popup { start, items, sel: 0, forced: true, scroll: true });
        }
        return false;
    }
    if ui.input_mut(|i| i.consume_key(M::SHIFT, Key::Tab)) {
        let (new, (s, e)) = vba_edit::indent_lines(code, (at, other), true);
        *code = new;
        set_range(ui.ctx(), id, e, s);
        return true;
    }
    if ui.input_mut(|i| i.consume_key(M::NONE, Key::Tab)) {
        if row_of(code, at) != row_of(code, other) {
            let (new, (s, e)) = vba_edit::indent_lines(code, (at, other), false);
            *code = new;
            set_range(ui.ctx(), id, e, s);
        } else {
            let (lo, hi) = (at.min(other), at.max(other));
            let a = code.char_indices().nth(lo).map_or(code.len(), |(i, _)| i);
            let b = code.char_indices().nth(hi).map_or(code.len(), |(i, _)| i);
            code.replace_range(a..b, vba_edit::INDENT);
            let to = lo + vba_edit::INDENT.len();
            set_range(ui.ctx(), id, to, to);
        }
        return true;
    }
    if at == other && ui.input_mut(|i| i.consume_key(M::NONE, Key::Enter)) {
        let row = row_of(code, at);
        normalize_row(code, row, info);
        let (new, to) = vba_edit::enter(code, at);
        *code = new;
        set_range(ui.ctx(), id, to, to);
        a.dirty_row = None;
        return true;
    }
    false
}

/// After the code: case on leaving a line, and the list and parameter info
/// at the cursor (`cursor`: its rectangle on screen). Returns whether the
/// code changed.
pub(super) fn after(ui: &mut egui::Ui, id: egui::Id, code: &mut String, a: &mut Assist, info: &Info, edited: bool, cursor: Option<egui::Rect>) -> bool {
    let (at, other) = range(ui.ctx(), id);
    let row = row_of(code, at);
    let mut changed = false;
    if let Some(d) = a.dirty_row
        && d != row
    {
        changed = normalize_row(code, d, info);
        a.dirty_row = None;
    }
    if edited {
        a.dirty_row = Some(row);
        let typed = a.typed.take();
        let word = typed.is_some_and(|c| c.is_alphanumeric() || c == '_');
        match &mut a.popup {
            Some(p) if word || typed.is_none() => match vba_edit::complete(&info.scope(code), at, p.forced) {
                Some((start, items)) if !items.is_empty() && start == p.start => {
                    p.sel = p.sel.min(items.len() - 1);
                    // The first that fits what is typed.
                    p.sel = 0;
                    p.items = items;
                    p.scroll = true;
                }
                _ => a.popup = None,
            },
            _ => {
                a.popup = None;
                if matches!(typed, Some('.' | ' ')) {
                    a.popup = vba_edit::complete(&info.scope(code), at, false).filter(|c| !c.1.is_empty()).map(|(start, items)| Popup { start, items, sel: 0, forced: false, scroll: true });
                }
            }
        }
    }
    // A cursor moved before the word being completed closes the list.
    if a.popup.as_ref().is_some_and(|p| at < p.start || at != other || row_of(code, p.start) != row) {
        a.popup = None;
    }
    let Some(cursor) = cursor else { return changed };
    let mut pick = None;
    if let Some(p) = &mut a.popup {
        let r = egui::Area::new(id.with("complete")).order(egui::Order::Foreground).fixed_pos(cursor.left_bottom() + egui::vec2(0.0, 2.0)).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(260.0);
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for (i, item) in p.items.iter().enumerate() {
                        let icon = match item.kind {
                            Kind::Method | Kind::Function => "◆",
                            Kind::Property | Kind::ReadOnly => "▪",
                            Kind::Event => "⚡",
                            Kind::Constant => "≡",
                            Kind::Keyword => "·",
                        };
                        let r = ui.selectable_label(i == p.sel, RichText::new(format!("{icon} {}", item.name)).monospace());
                        let r = if item.detail.is_empty() { r } else { r.on_hover_text(RichText::new(&item.detail).monospace()) };
                        if i == p.sel && p.scroll {
                            r.scroll_to_me(None);
                        }
                        if r.clicked() {
                            pick = Some(i);
                        }
                    }
                });
            });
        });
        p.scroll = false;
        let _ = r;
    }
    if let Some(i) = pick
        && let Some(mut p) = a.popup.take()
    {
        p.sel = i;
        accept(ui, id, code, &p, at);
        ui.memory_mut(|m| m.request_focus(id));
        a.dirty_row = Some(row);
        return true;
    }
    // Parameter info: the call's parameters, the one being typed in bold.
    if a.popup.is_none()
        && at == other
        && let Some((sig, n)) = vba_edit::signature(&info.scope(code), at)
    {
        let mut job = egui::text::LayoutJob::default();
        let font = egui::FontId::monospace(12.0);
        let color = ui.visuals().text_color();
        let strong = ui.visuals().strong_text_color();
        let (name, rest) = sig.split_at(sig.find('(').unwrap_or(sig.len()));
        job.append(name, 0.0, egui::TextFormat::simple(font.clone(), color));
        let mut part = 0;
        let mut depth = 0;
        let mut cur = String::new();
        let flush = |cur: &mut String, part: usize, job: &mut egui::text::LayoutJob| {
            let mut f = egui::TextFormat::simple(font.clone(), if part == n { strong } else { color });
            if part == n {
                f.underline = egui::Stroke::new(1.0, strong);
            }
            job.append(cur, 0.0, f);
            cur.clear();
        };
        for c in rest.chars() {
            match c {
                '(' => {
                    depth += 1;
                    cur.push(c);
                    if depth == 1 {
                        flush(&mut cur, usize::MAX, &mut job);
                    }
                }
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        flush(&mut cur, part, &mut job);
                    }
                    cur.push(c);
                }
                ',' if depth == 1 => {
                    flush(&mut cur, part, &mut job);
                    part += 1;
                    cur.push(c);
                    flush(&mut cur, usize::MAX, &mut job);
                }
                _ => cur.push(c),
            }
        }
        flush(&mut cur, usize::MAX, &mut job);
        egui::Area::new(id.with("signature")).order(egui::Order::Foreground).interactable(false).fixed_pos(cursor.left_bottom() + egui::vec2(0.0, 2.0)).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(job);
            });
        });
    }
    changed
}

/// Comment Block / Uncomment Block on the lines selected.
pub(super) fn comment(ctx: &egui::Context, id: egui::Id, code: &mut String, on: bool) -> bool {
    let (at, other) = range(ctx, id);
    let (new, (s, e)) = vba_edit::comment_lines(code, (at, other), on);
    if new == *code {
        return false;
    }
    *code = new;
    set_range(ctx, id, e, s);
    true
}

/// The editor's own undo and redo, per module: every change to the code,
/// typing merged while it goes on (a pause or anything else starts a new
/// step), so Enter's indent or a Replace All undo as one step.
#[derive(Default)]
pub(super) struct History {
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    /// When the last step was typing (one character more or less).
    typing: Option<f64>,
}

const LEVELS: usize = 500;

impl History {
    /// The code was `old` (cursor at `at`) before a change at `now`
    /// (seconds) that left `new`.
    pub(super) fn record(&mut self, old: String, at: usize, new: &str, now: f64) {
        let typed = old.chars().count().abs_diff(new.chars().count()) == 1;
        let merge = typed && self.typing.is_some_and(|t| now - t < 1.0);
        self.typing = typed.then_some(now);
        self.redo.clear();
        if merge {
            return;
        }
        self.undo.push((old, at));
        if self.undo.len() > LEVELS {
            self.undo.remove(0);
        }
    }

    /// Back one step: the code and cursor to show.
    pub(super) fn undo(&mut self, current: &str, at: usize) -> Option<(String, usize)> {
        let step = self.undo.pop()?;
        self.redo.push((current.to_string(), at));
        self.typing = None;
        Some(step)
    }

    pub(super) fn redo(&mut self, current: &str, at: usize) -> Option<(String, usize)> {
        let step = self.redo.pop()?;
        self.undo.push((current.to_string(), at));
        self.typing = None;
        Some(step)
    }
}

/// Ctrl+Z and Ctrl+Y (or Ctrl+Mayús+Z) in the code `id`: the code goes
/// back or forward a step. Returns whether it changed.
pub(super) fn undo_keys(ui: &mut egui::Ui, id: egui::Id, code: &mut String, h: &mut History) -> bool {
    use egui::{Key, Modifiers as M};
    let redo = ui.input_mut(|i| i.consume_key(M::COMMAND | M::SHIFT, Key::Z) || i.consume_key(M::COMMAND, Key::Y));
    let undo = !redo && ui.input_mut(|i| i.consume_key(M::COMMAND, Key::Z));
    if !undo && !redo {
        return false;
    }
    let at = range(ui.ctx(), id).0;
    let step = if undo { h.undo(code, at) } else { h.redo(code, at) };
    let Some((new, at)) = step else { return false };
    *code = new;
    let at = at.min(code.chars().count());
    set_range(ui.ctx(), id, at, at);
    true
}

#[cfg(test)]
mod tests {
    use super::History;

    #[test]
    fn undo_steps() {
        let mut h = History::default();
        // Typing "ab" quickly is one step; Enter's indent another.
        h.record("".into(), 0, "a", 0.0);
        h.record("a".into(), 1, "ab", 0.3);
        h.record("ab".into(), 2, "ab\n    ", 0.5);
        assert_eq!(h.undo("ab\n    ", 7), Some(("ab".into(), 2)));
        assert_eq!(h.undo("ab", 2), Some(("".into(), 0)));
        assert_eq!(h.undo("", 0), None);
        assert_eq!(h.redo("", 0), Some(("ab".into(), 2)));
        assert_eq!(h.redo("ab", 2), Some(("ab\n    ".into(), 7)));
        // A pause starts a new step; a change clears what could be redone.
        h.record("ab\n    ".into(), 7, "ab\n    x", 5.0);
        h.record("ab\n    x".into(), 8, "ab\n    xy", 7.0);
        assert_eq!(h.undo("ab\n    xy", 9), Some(("ab\n    x".into(), 8)));
        h.record("ab\n    x".into(), 8, "ab\n    xz", 8.0);
        assert_eq!(h.redo("ab\n    xz", 9), None);
    }
}
