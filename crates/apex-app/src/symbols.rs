//! The Insert Symbol dialog shared by both editors: a grid of the
//! characters of common Unicode blocks (only those the UI fonts can draw),
//! a list of special characters and the recently used symbols.

// The editors open the dialog from their menus; until they do, parts of
// the API are only used by the tests.

use std::sync::Mutex;

use eframe::egui::{self, Color32, FontId, Id, Key, Modifiers, RichText, Sense, Stroke, vec2};

use crate::common;
use crate::i18n::tr;
use crate::theme;

const MAX_RECENT: usize = 20;
const COLS: usize = 16;
const CELL: f32 = 32.0;
const GRID_ROWS: usize = 8;
const RECENT_CELL: f32 = 25.0;

// --- recently used ---------------------------------------------------------------------

static RECENT: Mutex<Vec<char>> = Mutex::new(Vec::new());

/// Recently inserted symbols, most recent first.
pub fn recent() -> Vec<char> {
    RECENT.lock().map(|v| v.clone()).unwrap_or_default()
}

pub fn set_recent(v: Vec<char>) {
    if let Ok(mut r) = RECENT.lock() {
        *r = v;
        r.truncate(MAX_RECENT);
    }
}

fn add_recent(list: &mut Vec<char>, c: char) {
    list.retain(|&x| x != c);
    list.insert(0, c);
    list.truncate(MAX_RECENT);
}

fn push_recent(c: char) {
    if !visible(c) {
        return;
    }
    if let Ok(mut r) = RECENT.lock() {
        add_recent(&mut r, c);
    }
    common::mark_prefs_dirty();
}

// --- character tables --------------------------------------------------------------------

struct Block {
    /// i18n key of the block name.
    key: &'static str,
    start: u32,
    end: u32,
    names: &'static str,
}

impl Block {
    fn contains(&self, c: char) -> bool {
        (self.start..=self.end).contains(&(c as u32))
    }

    /// Assigned characters of the block.
    fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.names.split('|').zip(self.start..).filter(|(n, _)| !n.is_empty()).filter_map(|(_, c)| char::from_u32(c))
    }
}

fn block_of(c: char) -> Option<usize> {
    BLOCKS.iter().position(|b| b.contains(c))
}

/// False for spaces, format controls and combining marks, which draw
/// nothing on their own (the special characters tab lists the useful ones).
fn visible(c: char) -> bool {
    let u = c as u32;
    !(c.is_whitespace() || c.is_control() || u == 0xAD || (0x483..=0x489).contains(&u) || (0x200B..=0x200F).contains(&u) || (0x202A..=0x202E).contains(&u) || (0x2060..=0x206F).contains(&u) || u == 0xFEFF)
}

/// The English Unicode name of a character of the listed blocks, in title
/// case as Word shows it ("Latin Small Letter E With Acute").
pub fn char_name(c: char) -> Option<String> {
    let b = &BLOCKS[block_of(c)?];
    let raw = b.names.split('|').nth((c as u32 - b.start) as usize).filter(|n| !n.is_empty())?;
    let mut out = String::with_capacity(raw.len() * 2);
    for (i, w) in raw.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let word = match w.as_bytes() {
            [l @ b'a'..=b'z'] => WORDS[(l - b'a') as usize],
            _ => w,
        };
        // Capitalize each part of hyphenated words too ("No-Break").
        let mut up = true;
        for ch in word.chars() {
            out.push(if up { ch.to_ascii_uppercase() } else { ch.to_ascii_lowercase() });
            up = ch == '-';
        }
    }
    Some(out)
}

/// Parses a character code typed as `00E9`, `U+00E9` or `0x00E9`.
pub fn parse_hex(s: &str) -> Option<char> {
    let s = s.trim();
    let s = ["U+", "u+", "0x", "0X", "\\u"].iter().find_map(|p| s.strip_prefix(p)).unwrap_or(s);
    if s.is_empty() || s.len() > 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    char::from_u32(u32::from_str_radix(s, 16).ok()?)
}

pub fn code_text(c: char) -> String {
    format!("U+{:04X}", c as u32)
}

/// A character of the Special characters tab.
pub struct Special {
    pub ch: char,
    /// i18n key of its name.
    pub key: &'static str,
    /// Word's shortcut; "Shift" and "Space" are translated when shown and
    /// `{q}` stands for a double quote.
    pub shortcut: Option<&'static str>,
}

const fn sp(ch: char, key: &'static str, shortcut: Option<&'static str>) -> Special {
    Special { ch, key, shortcut }
}

pub const SPECIALS: &[Special] = &[
    sp('\u{2014}', "symbol.sp.em_dash", Some("Alt+Ctrl+Num -")),
    sp('\u{2013}', "symbol.sp.en_dash", Some("Ctrl+Num -")),
    sp('\u{2011}', "symbol.sp.nb_hyphen", Some("Ctrl+Shift+_")),
    sp('\u{00AD}', "symbol.sp.opt_hyphen", Some("Ctrl+-")),
    sp('\u{2003}', "symbol.sp.em_space", None),
    sp('\u{2002}', "symbol.sp.en_space", None),
    sp('\u{2005}', "symbol.sp.quarter_space", None),
    sp('\u{00A0}', "symbol.sp.nbsp", Some("Ctrl+Shift+Space")),
    sp('\u{202F}', "symbol.sp.narrow_nbsp", None),
    sp('\u{00A9}', "symbol.sp.copyright", Some("Alt+Ctrl+C")),
    sp('\u{00AE}', "symbol.sp.registered", Some("Alt+Ctrl+R")),
    sp('\u{2122}', "symbol.sp.trademark", Some("Alt+Ctrl+T")),
    sp('\u{00A7}', "symbol.sp.section", None),
    sp('\u{00B6}', "symbol.sp.paragraph", None),
    sp('\u{2026}', "symbol.sp.ellipsis", Some("Alt+Ctrl+.")),
    sp('\u{2018}', "symbol.sp.single_open", Some("Ctrl+`,`")),
    sp('\u{2019}', "symbol.sp.single_close", Some("Ctrl+','")),
    sp('\u{201C}', "symbol.sp.double_open", Some("Ctrl+`,{q}")),
    sp('\u{201D}', "symbol.sp.double_close", Some("Ctrl+',{q}")),
    sp('\u{00AB}', "symbol.sp.angle_open", None),
    sp('\u{00BB}', "symbol.sp.angle_close", None),
    sp('\u{00BF}', "symbol.sp.inv_question", None),
    sp('\u{00A1}', "symbol.sp.inv_exclamation", None),
    sp('\u{00AA}', "symbol.sp.ordinal_f", None),
    sp('\u{00BA}', "symbol.sp.ordinal_m", None),
    sp('\u{200B}', "symbol.sp.zw_break", None),
    sp('\u{2060}', "symbol.sp.zw_nobreak", None),
    sp('\u{00B0}', "symbol.sp.degree", None),
    sp('\u{00B1}', "symbol.sp.plus_minus", None),
    sp('\u{00D7}', "symbol.sp.multiplication", None),
    sp('\u{00F7}', "symbol.sp.division", None),
    sp('\u{2030}', "symbol.sp.per_mille", None),
    sp('\u{2022}', "symbol.sp.bullet", None),
    sp('\u{00B7}', "symbol.sp.middle_dot", None),
    sp('\u{20AC}', "symbol.sp.euro", None),
    sp('\u{00A3}', "symbol.sp.pound", None),
    sp('\u{00A5}', "symbol.sp.yen", None),
    sp('\u{00A2}', "symbol.sp.cent", None),
];

fn shortcut_text(s: &str) -> String {
    s.replace("Shift", tr("symbol.key_shift")).replace("Space", tr("symbol.key_space")).replace("{q}", "\u{22}")
}

// --- dialog ------------------------------------------------------------------------------

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Symbols,
    Special,
}

/// State of the Insert Symbol dialog; keep one per editor (or per app).
#[derive(Default)]
pub struct SymbolDialog {
    open: bool,
    tab: Tab,
    block: usize,
    sel: Option<char>,
    special: usize,
    hex: String,
    hex_bad: bool,
    /// Drawable characters of each block, filled on first view.
    avail: Vec<Option<Vec<char>>>,
    scroll_to_sel: bool,
    offset: f32,
    /// Side of a cell of the grid: smaller on a phone, so the row fits.
    cell: f32,
}

impl SymbolDialog {
    /// Opens the dialog (it stays open after inserting, like Word's).
    pub fn open(&mut self) {
        self.open = true;
        self.scroll_to_sel = true;
        if self.sel.is_none() {
            self.sel = recent().first().copied();
            if let Some(b) = self.sel.and_then(block_of) {
                self.block = b;
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn avail(&mut self, ctx: &egui::Context, block: usize) -> &[char] {
        if self.avail.len() < BLOCKS.len() {
            self.avail.resize(BLOCKS.len(), None);
        }
        self.avail[block].get_or_insert_with(|| {
            let font = FontId::proportional(20.0);
            ctx.fonts_mut(|f| BLOCKS[block].chars().filter(|&c| visible(c) && f.has_glyph(&font, c)).collect())
        })
    }

    fn select(&mut self, c: char) {
        self.sel = Some(c);
        if let Some(b) = block_of(c) {
            self.block = b;
        }
        self.scroll_to_sel = true;
    }

    /// Text to insert for the current selection, remembering it as recent.
    fn take_current(&mut self) -> Option<String> {
        let c = match self.tab {
            Tab::Symbols => self.sel?,
            Tab::Special => SPECIALS.get(self.special)?.ch,
        };
        push_recent(c);
        Some(c.to_string())
    }

    /// Draws the dialog when open. Returns the text to insert when the user
    /// presses Insert or Enter or double-clicks a character.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<String> {
        if !self.open {
            return None;
        }
        let hex_id = Id::new("symbol_dialog_hex");
        let typing = ctx.memory(|m| m.focused() == Some(hex_id));
        let mut insert = false;
        if !typing {
            insert |= self.keyboard(ctx);
        }
        let mut close = false;
        self.cell = CELL.min(((crate::widgets::dialog_room(ctx).x - 4.0) / COLS as f32).floor());
        let modal = egui::Modal::new(Id::new(("dialog", "symbols"))).show(ctx, |ui| {
            ui.set_width(COLS as f32 * self.cell + 4.0);
            ui.heading(tr("symbol.title"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (t, key) in [(Tab::Symbols, "symbol.tab_symbols"), (Tab::Special, "symbol.tab_special")] {
                    if ui.add(egui::Button::selectable(self.tab == t, tr(key))).clicked() {
                        self.tab = t;
                    }
                }
            });
            ui.separator();
            match self.tab {
                Tab::Symbols => insert |= self.symbols_tab(ui, hex_id),
                Tab::Special => insert |= self.special_tab(ui),
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("symbol.close")).clicked() {
                    close = true;
                }
                let can = match self.tab {
                    Tab::Symbols => self.sel.is_some(),
                    Tab::Special => true,
                };
                if ui.add_enabled(can, egui::Button::new(RichText::new(tr("symbol.insert")).strong())).clicked() {
                    insert = true;
                }
            });
        });
        if ctx.input(|i| i.key_pressed(Key::Escape)) || modal.should_close() && ctx.input(|i| i.pointer.any_click()) {
            close = true;
        }
        if close {
            self.open = false;
            return None;
        }
        if insert { self.take_current() } else { None }
    }

    /// Arrow keys move the selection; Enter inserts. Returns true on Enter.
    fn keyboard(&mut self, ctx: &egui::Context) -> bool {
        let mut enter = false;
        let mut step: isize = 0;
        ctx.input_mut(|i| {
            let mut k = |key: Key, d: isize| {
                if i.consume_key(Modifiers::NONE, key) {
                    step = d;
                }
            };
            k(Key::ArrowLeft, -1);
            k(Key::ArrowRight, 1);
            k(Key::ArrowUp, -(COLS as isize));
            k(Key::ArrowDown, COLS as isize);
            k(Key::PageUp, -((COLS * GRID_ROWS) as isize));
            k(Key::PageDown, (COLS * GRID_ROWS) as isize);
            k(Key::Home, isize::MIN);
            k(Key::End, isize::MAX);
            enter = i.consume_key(Modifiers::NONE, Key::Enter);
        });
        if step != 0 {
            match self.tab {
                Tab::Symbols => {
                    let block = self.block;
                    let sel = self.sel;
                    let chars = self.avail(ctx, block);
                    if let Some(i) = step_index(chars.len(), sel.and_then(|s| chars.iter().position(|&c| c == s)), step) {
                        let c = chars[i];
                        self.select(c);
                    }
                }
                Tab::Special => {
                    // Up/down move by one row in the list.
                    let d = if step.unsigned_abs() == COLS { step.signum() } else { step };
                    if let Some(i) = step_index(SPECIALS.len(), Some(self.special), d) {
                        self.special = i;
                        self.scroll_to_sel = true;
                    }
                }
            }
        }
        enter
    }

    fn symbols_tab(&mut self, ui: &mut egui::Ui, hex_id: Id) -> bool {
        let ctx = ui.ctx().clone();
        let mut insert = false;
        ui.horizontal(|ui| {
            ui.label(tr("symbol.subset"));
            let mut block = self.block;
            egui::ComboBox::from_id_salt("symbol_block").width(220.0).height(420.0).selected_text(tr(BLOCKS[block].key)).show_ui(ui, |ui| {
                for (i, b) in BLOCKS.iter().enumerate() {
                    ui.selectable_value(&mut block, i, tr(b.key));
                }
            });
            if block != self.block {
                self.block = block;
                self.offset = 0.0;
                let first = self.avail(&ctx, block).first().copied();
                self.sel = first;
                self.scroll_to_sel = false;
            }
        });
        ui.add_space(4.0);

        // The grid.
        let block = self.block;
        let chars = self.avail(&ctx, block).to_vec();
        if self.sel.is_none() {
            self.sel = chars.first().copied();
        }
        let rows = chars.len().div_ceil(COLS);
        let view_h = GRID_ROWS as f32 * self.cell;
        let mut area = egui::ScrollArea::vertical().id_salt(("symbol_grid", block)).max_height(view_h).min_scrolled_height(view_h).auto_shrink([false, false]);
        if self.scroll_to_sel {
            if let Some(i) = self.sel.and_then(|s| chars.iter().position(|&c| c == s)) {
                let top = (i / COLS) as f32 * self.cell;
                let off = if top < self.offset { top } else if top + self.cell > self.offset + view_h { top + self.cell - view_h } else { self.offset };
                area = area.vertical_scroll_offset(off);
            }
            self.scroll_to_sel = false;
        }
        let font = FontId::proportional(20.0);
        let frame_stroke = Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
        let out = egui::Frame::new().stroke(frame_stroke).show(ui, |ui| {
            if chars.is_empty() {
                ui.set_min_size(vec2(ui.available_width(), view_h));
                ui.label(RichText::new(tr("symbol.none")).weak());
                return None;
            }
            let out = area.show_rows(ui, self.cell, rows, |ui, range| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let mut picked = None;
                for r in range {
                    ui.horizontal(|ui| {
                        for &c in chars.iter().skip(r * COLS).take(COLS) {
                            let resp = glyph_cell(ui, c, self.cell, &font, self.sel == Some(c));
                            if resp.clicked() {
                                picked = Some((c, false));
                            }
                            if resp.double_clicked() {
                                picked = Some((c, true));
                            }
                        }
                    });
                }
                picked
            });
            self.offset = out.state.offset.y;
            out.inner
        });
        if let Some((c, dbl)) = out.inner {
            self.sel = Some(c);
            insert |= dbl;
        }

        // Recently used symbols.
        ui.add_space(6.0);
        ui.label(tr("symbol.recent"));
        let recent: Vec<char> = {
            let f = FontId::proportional(16.0);
            let list = recent();
            ctx.fonts_mut(|fonts| list.into_iter().filter(|&c| fonts.has_glyph(&f, c)).collect())
        };
        // As wide as the grid at most.
        let cell = RECENT_CELL.min((COLS as f32 * self.cell / MAX_RECENT as f32).floor());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let font = FontId::proportional(16.0);
            for i in 0..MAX_RECENT {
                match recent.get(i) {
                    Some(&c) => {
                        // The one chosen shows so here too.
                        let resp = glyph_cell(ui, c, cell, &font, self.sel == Some(c));
                        if resp.clicked() {
                            self.select(c);
                        }
                        if resp.double_clicked() {
                            insert = true;
                        }
                    }
                    None => {
                        let (rect, _) = ui.allocate_exact_size(vec2(cell, cell), Sense::hover());
                        ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(0.5)), egui::StrokeKind::Inside);
                    }
                }
            }
        });

        // Preview, name and code.
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
            let v = ui.visuals();
            ui.painter().rect(rect, 4.0, v.extreme_bg_color, Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color), egui::StrokeKind::Inside);
            if let Some(c) = self.sel {
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, c, FontId::proportional(44.0), v.text_color());
            }
            ui.vertical(|ui| {
                let name = self.sel.and_then(char_name).unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                ui.label(RichText::new(self.sel.map(code_text).unwrap_or_default()).weak());
                ui.horizontal(|ui| {
                    ui.label(tr("symbol.code"));
                    if !ui.memory(|m| m.has_focus(hex_id)) {
                        self.hex = self.sel.map(|c| format!("{:04X}", c as u32)).unwrap_or_default();
                        self.hex_bad = false;
                    }
                    let r = ui.add(egui::TextEdit::singleline(&mut self.hex).id(hex_id).desired_width(64.0).char_limit(8).font(egui::TextStyle::Monospace));
                    if r.changed() {
                        self.jump(&ctx);
                    }
                    ui.label(RichText::new(tr("symbol.from_unicode")).weak());
                });
                if self.hex_bad {
                    ui.label(RichText::new(tr("symbol.not_found")).small().color(ui.visuals().error_fg_color));
                }
            });
        });
        insert
    }

    /// Selects the character typed in the code box when the fonts can draw it.
    fn jump(&mut self, ctx: &egui::Context) {
        let Some(c) = parse_hex(&self.hex) else {
            self.hex_bad = !self.hex.trim().is_empty();
            return;
        };
        let ok = block_of(c).is_some_and(|b| self.avail(ctx, b).contains(&c));
        self.hex_bad = !ok;
        if ok {
            self.select(c);
        }
    }

    fn special_tab(&mut self, ui: &mut egui::Ui) -> bool {
        let mut insert = false;
        let font = FontId::proportional(16.0);
        let row_h = 24.0;
        let char_w = 60.0;
        let key_w = 140.0;
        let v = ui.visuals().clone();
        // Column headers.
        ui.horizontal(|ui| {
            ui.add_sized(vec2(char_w, 18.0), egui::Label::new(RichText::new(tr("symbol.character")).small().weak()));
            ui.label(RichText::new(tr("symbol.name")).small().weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_sized(vec2(key_w, 18.0), egui::Label::new(RichText::new(tr("symbol.shortcut")).small().weak()));
            });
        });
        let drawable: Vec<bool> = ui.ctx().fonts_mut(|f| SPECIALS.iter().map(|s| visible(s.ch) && f.has_glyph(&font, s.ch)).collect());
        let view_h = GRID_ROWS as f32 * self.cell + 80.0;
        let mut area = egui::ScrollArea::vertical().id_salt("symbol_special").max_height(view_h).min_scrolled_height(view_h).auto_shrink([false, false]);
        if self.scroll_to_sel {
            let top = self.special as f32 * row_h;
            let off = if top < self.offset { top } else if top + row_h > self.offset + view_h { top + row_h - view_h } else { self.offset };
            area = area.vertical_scroll_offset(off);
            self.scroll_to_sel = false;
        }
        let frame = egui::Frame::new().stroke(Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color));
        frame.show(ui, |ui| {
            let out = area.show_rows(ui, row_h, SPECIALS.len(), |ui, range| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                for i in range {
                    let s = &SPECIALS[i];
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                    crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, tr(s.key), Some(self.special == i));
                    let on = self.special == i;
                    let fg = if on { Color32::WHITE } else { v.text_color() };
                    if on {
                        ui.painter().rect_filled(rect, 0.0, theme::ACCENT);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 0.0, v.widgets.hovered.weak_bg_fill);
                    }
                    let y = rect.center().y;
                    if drawable[i] {
                        ui.painter().text(egui::pos2(rect.min.x + char_w / 2.0, y), egui::Align2::CENTER_CENTER, s.ch, font.clone(), fg);
                    }
                    ui.painter().text(egui::pos2(rect.min.x + char_w, y), egui::Align2::LEFT_CENTER, tr(s.key), FontId::proportional(14.0), fg);
                    if let Some(k) = s.shortcut {
                        ui.painter().text(egui::pos2(rect.max.x - key_w, y), egui::Align2::LEFT_CENTER, shortcut_text(k), FontId::proportional(13.0), fg);
                    }
                    if resp.clicked() {
                        self.special = i;
                    }
                    if resp.double_clicked() {
                        self.special = i;
                        insert = true;
                    }
                }
            });
            self.offset = out.state.offset.y;
        });
        insert
    }
}

/// One glyph cell of the grids; the character code shows on hover.
fn glyph_cell(ui: &mut egui::Ui, c: char, size: f32, font: &FontId, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &char_name(c).unwrap_or_else(|| code_text(c)), Some(selected));
    let v = ui.visuals();
    let fg = if selected {
        ui.painter().rect_filled(rect, 0.0, theme::ACCENT);
        Color32::WHITE
    } else {
        if resp.hovered() {
            ui.painter().rect_filled(rect, 0.0, v.widgets.hovered.weak_bg_fill);
        }
        v.text_color()
    };
    ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color.gamma_multiply(0.5)), egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, c, font.clone(), fg);
    let tip = match char_name(c) {
        Some(n) => format!("{n}\n{}", code_text(c)),
        None => code_text(c),
    };
    resp.on_hover_text(tip)
}

/// Moves an index by `step` within `0..len` (`isize::MIN`/`MAX` go to the
/// ends); with nothing selected, any key selects the first item.
fn step_index(len: usize, cur: Option<usize>, step: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(cur) = cur else { return Some(0) };
    Some(match step {
        isize::MIN => 0,
        isize::MAX => len - 1,
        s => (cur as isize + s).clamp(0, len as isize - 1) as usize,
    })
}

// --- generated tables --------------------------------------------------------------------

/// Frequent words of the names, abbreviated as `a`..`z` in [`BLOCKS`].
const WORDS: [&str; 26] = [
    "LETTER", "CYRILLIC", "CAPITAL", "SMALL", "WITH", "DRAWINGS", "LATIN", "RIGHTWARDS", "ARROW",
    "HEAVY", "GREEK", "RIGHT", "WHITE", "VERTICAL", "DOUBLE", "HORIZONTAL", "BLACK", "LEFT",
    "LIGHT", "SIGN", "DIAERESIS", "SYMBOL", "LEFTWARDS", "AND", "BOX", "CIRCLED",
];

/// The listed Unicode blocks with the names of their characters (generated
/// from UnicodeData 16.0.0): one `|`-separated field per code point, empty when
/// unassigned, with frequent words abbreviated (see [`WORDS`]).
const BLOCKS: [Block; 15] = [
    Block {
        key: "symbol.block.latin1",
        start: 0x00A0,
        end: 0x00FF,
        names: "NO-BREAK SPACE|INVERTED EXCLAMATION MARK|CENT t|POUND t|CURRENCY t|YEN t|BROKEN BAR|SECTION t|u|\
            COPYRIGHT t|FEMININE ORDINAL INDICATOR|LEFT-POINTING o ANGLE QUOTATION MARK|NOT t|SOFT HYPHEN|\
            REGISTERED t|MACRON|DEGREE t|PLUS-MINUS t|SUPERSCRIPT TWO|SUPERSCRIPT THREE|ACUTE ACCENT|MICRO t|\
            PILCROW t|MIDDLE DOT|CEDILLA|SUPERSCRIPT ONE|MASCULINE ORDINAL INDICATOR|\
            RIGHT-POINTING o ANGLE QUOTATION MARK|VULGAR FRACTION ONE QUARTER|VULGAR FRACTION ONE HALF|\
            VULGAR FRACTION THREE QUARTERS|INVERTED QUESTION MARK|g c a A e GRAVE|g c a A e ACUTE|\
            g c a A e CIRCUMFLEX|g c a A e TILDE|g c a A e u|g c a A e RING ABOVE|g c a AE|g c a C e CEDILLA|\
            g c a E e GRAVE|g c a E e ACUTE|g c a E e CIRCUMFLEX|g c a E e u|g c a I e GRAVE|g c a I e ACUTE|\
            g c a I e CIRCUMFLEX|g c a I e u|g c a ETH|g c a N e TILDE|g c a O e GRAVE|g c a O e ACUTE|\
            g c a O e CIRCUMFLEX|g c a O e TILDE|g c a O e u|MULTIPLICATION t|g c a O e STROKE|g c a U e GRAVE|\
            g c a U e ACUTE|g c a U e CIRCUMFLEX|g c a U e u|g c a Y e ACUTE|g c a THORN|g d a SHARP S|\
            g d a A e GRAVE|g d a A e ACUTE|g d a A e CIRCUMFLEX|g d a A e TILDE|g d a A e u|\
            g d a A e RING ABOVE|g d a AE|g d a C e CEDILLA|g d a E e GRAVE|g d a E e ACUTE|\
            g d a E e CIRCUMFLEX|g d a E e u|g d a I e GRAVE|g d a I e ACUTE|g d a I e CIRCUMFLEX|g d a I e u|\
            g d a ETH|g d a N e TILDE|g d a O e GRAVE|g d a O e ACUTE|g d a O e CIRCUMFLEX|g d a O e TILDE|\
            g d a O e u|DIVISION t|g d a O e STROKE|g d a U e GRAVE|g d a U e ACUTE|g d a U e CIRCUMFLEX|\
            g d a U e u|g d a Y e ACUTE|g d a THORN|g d a Y e u",
    },
    Block {
        key: "symbol.block.latin_ext_a",
        start: 0x0100,
        end: 0x017F,
        names: "g c a A e MACRON|g d a A e MACRON|g c a A e BREVE|g d a A e BREVE|g c a A e OGONEK|g d a A e OGONEK|\
            g c a C e ACUTE|g d a C e ACUTE|g c a C e CIRCUMFLEX|g d a C e CIRCUMFLEX|g c a C e DOT ABOVE|\
            g d a C e DOT ABOVE|g c a C e CARON|g d a C e CARON|g c a D e CARON|g d a D e CARON|\
            g c a D e STROKE|g d a D e STROKE|g c a E e MACRON|g d a E e MACRON|g c a E e BREVE|g d a E e BREVE|\
            g c a E e DOT ABOVE|g d a E e DOT ABOVE|g c a E e OGONEK|g d a E e OGONEK|g c a E e CARON|\
            g d a E e CARON|g c a G e CIRCUMFLEX|g d a G e CIRCUMFLEX|g c a G e BREVE|g d a G e BREVE|\
            g c a G e DOT ABOVE|g d a G e DOT ABOVE|g c a G e CEDILLA|g d a G e CEDILLA|g c a H e CIRCUMFLEX|\
            g d a H e CIRCUMFLEX|g c a H e STROKE|g d a H e STROKE|g c a I e TILDE|g d a I e TILDE|\
            g c a I e MACRON|g d a I e MACRON|g c a I e BREVE|g d a I e BREVE|g c a I e OGONEK|g d a I e OGONEK|\
            g c a I e DOT ABOVE|g d a DOTLESS I|g c LIGATURE IJ|g d LIGATURE IJ|g c a J e CIRCUMFLEX|\
            g d a J e CIRCUMFLEX|g c a K e CEDILLA|g d a K e CEDILLA|g d a KRA|g c a L e ACUTE|g d a L e ACUTE|\
            g c a L e CEDILLA|g d a L e CEDILLA|g c a L e CARON|g d a L e CARON|g c a L e MIDDLE DOT|\
            g d a L e MIDDLE DOT|g c a L e STROKE|g d a L e STROKE|g c a N e ACUTE|g d a N e ACUTE|\
            g c a N e CEDILLA|g d a N e CEDILLA|g c a N e CARON|g d a N e CARON|g d a N PRECEDED BY APOSTROPHE|\
            g c a ENG|g d a ENG|g c a O e MACRON|g d a O e MACRON|g c a O e BREVE|g d a O e BREVE|\
            g c a O e o ACUTE|g d a O e o ACUTE|g c LIGATURE OE|g d LIGATURE OE|g c a R e ACUTE|g d a R e ACUTE|\
            g c a R e CEDILLA|g d a R e CEDILLA|g c a R e CARON|g d a R e CARON|g c a S e ACUTE|g d a S e ACUTE|\
            g c a S e CIRCUMFLEX|g d a S e CIRCUMFLEX|g c a S e CEDILLA|g d a S e CEDILLA|g c a S e CARON|\
            g d a S e CARON|g c a T e CEDILLA|g d a T e CEDILLA|g c a T e CARON|g d a T e CARON|\
            g c a T e STROKE|g d a T e STROKE|g c a U e TILDE|g d a U e TILDE|g c a U e MACRON|g d a U e MACRON|\
            g c a U e BREVE|g d a U e BREVE|g c a U e RING ABOVE|g d a U e RING ABOVE|g c a U e o ACUTE|\
            g d a U e o ACUTE|g c a U e OGONEK|g d a U e OGONEK|g c a W e CIRCUMFLEX|g d a W e CIRCUMFLEX|\
            g c a Y e CIRCUMFLEX|g d a Y e CIRCUMFLEX|g c a Y e u|g c a Z e ACUTE|g d a Z e ACUTE|\
            g c a Z e DOT ABOVE|g d a Z e DOT ABOVE|g c a Z e CARON|g d a Z e CARON|g d a LONG S",
    },
    Block {
        key: "symbol.block.greek",
        start: 0x0370,
        end: 0x03FF,
        names: "k c a HETA|k d a HETA|k c a ARCHAIC SAMPI|k d a ARCHAIC SAMPI|k NUMERAL t|k LOWER NUMERAL t|\
            k c a PAMPHYLIAN DIGAMMA|k d a PAMPHYLIAN DIGAMMA|||k YPOGEGRAMMENI|k d REVERSED LUNATE SIGMA v|\
            k d DOTTED LUNATE SIGMA v|k d REVERSED DOTTED LUNATE SIGMA v|k QUESTION MARK|k c a YOT|||||k TONOS|\
            k DIALYTIKA TONOS|k c a ALPHA e TONOS|k ANO TELEIA|k c a EPSILON e TONOS|k c a ETA e TONOS|\
            k c a IOTA e TONOS||k c a OMICRON e TONOS||k c a UPSILON e TONOS|k c a OMEGA e TONOS|\
            k d a IOTA e DIALYTIKA x TONOS|k c a ALPHA|k c a BETA|k c a GAMMA|k c a DELTA|k c a EPSILON|\
            k c a ZETA|k c a ETA|k c a THETA|k c a IOTA|k c a KAPPA|k c a LAMDA|k c a MU|k c a NU|k c a XI|\
            k c a OMICRON|k c a PI|k c a RHO||k c a SIGMA|k c a TAU|k c a UPSILON|k c a PHI|k c a CHI|k c a PSI|\
            k c a OMEGA|k c a IOTA e DIALYTIKA|k c a UPSILON e DIALYTIKA|k d a ALPHA e TONOS|\
            k d a EPSILON e TONOS|k d a ETA e TONOS|k d a IOTA e TONOS|k d a UPSILON e DIALYTIKA x TONOS|\
            k d a ALPHA|k d a BETA|k d a GAMMA|k d a DELTA|k d a EPSILON|k d a ZETA|k d a ETA|k d a THETA|\
            k d a IOTA|k d a KAPPA|k d a LAMDA|k d a MU|k d a NU|k d a XI|k d a OMICRON|k d a PI|k d a RHO|\
            k d a FINAL SIGMA|k d a SIGMA|k d a TAU|k d a UPSILON|k d a PHI|k d a CHI|k d a PSI|k d a OMEGA|\
            k d a IOTA e DIALYTIKA|k d a UPSILON e DIALYTIKA|k d a OMICRON e TONOS|k d a UPSILON e TONOS|\
            k d a OMEGA e TONOS|k c KAI v|k BETA v|k THETA v|k UPSILON e HOOK v|k UPSILON e ACUTE x HOOK v|\
            k UPSILON e u x HOOK v|k PHI v|k PI v|k KAI v|k a ARCHAIC KOPPA|k d a ARCHAIC KOPPA|k a STIGMA|\
            k d a STIGMA|k a DIGAMMA|k d a DIGAMMA|k a KOPPA|k d a KOPPA|k a SAMPI|k d a SAMPI|COPTIC c a SHEI|\
            COPTIC d a SHEI|COPTIC c a FEI|COPTIC d a FEI|COPTIC c a KHEI|COPTIC d a KHEI|COPTIC c a HORI|\
            COPTIC d a HORI|COPTIC c a GANGIA|COPTIC d a GANGIA|COPTIC c a SHIMA|COPTIC d a SHIMA|\
            COPTIC c a DEI|COPTIC d a DEI|k KAPPA v|k RHO v|k LUNATE SIGMA v|k a YOT|k c THETA v|\
            k LUNATE EPSILON v|k REVERSED LUNATE EPSILON v|k c a SHO|k d a SHO|k c LUNATE SIGMA v|k c a SAN|\
            k d a SAN|k RHO e STROKE v|k c REVERSED LUNATE SIGMA v|k c DOTTED LUNATE SIGMA v|\
            k c REVERSED DOTTED LUNATE SIGMA v",
    },
    Block {
        key: "symbol.block.cyrillic",
        start: 0x0400,
        end: 0x04FF,
        names: "b c a IE e GRAVE|b c a IO|b c a DJE|b c a GJE|b c a UKRAINIAN IE|b c a DZE|\
            b c a BYELORUSSIAN-UKRAINIAN I|b c a YI|b c a JE|b c a LJE|b c a NJE|b c a TSHE|b c a KJE|\
            b c a I e GRAVE|b c a SHORT U|b c a DZHE|b c a A|b c a BE|b c a VE|b c a GHE|b c a DE|b c a IE|\
            b c a ZHE|b c a ZE|b c a I|b c a SHORT I|b c a KA|b c a EL|b c a EM|b c a EN|b c a O|b c a PE|\
            b c a ER|b c a ES|b c a TE|b c a U|b c a EF|b c a HA|b c a TSE|b c a CHE|b c a SHA|b c a SHCHA|\
            b c a HARD t|b c a YERU|b c a SOFT t|b c a E|b c a YU|b c a YA|b d a A|b d a BE|b d a VE|b d a GHE|\
            b d a DE|b d a IE|b d a ZHE|b d a ZE|b d a I|b d a SHORT I|b d a KA|b d a EL|b d a EM|b d a EN|\
            b d a O|b d a PE|b d a ER|b d a ES|b d a TE|b d a U|b d a EF|b d a HA|b d a TSE|b d a CHE|b d a SHA|\
            b d a SHCHA|b d a HARD t|b d a YERU|b d a SOFT t|b d a E|b d a YU|b d a YA|b d a IE e GRAVE|\
            b d a IO|b d a DJE|b d a GJE|b d a UKRAINIAN IE|b d a DZE|b d a BYELORUSSIAN-UKRAINIAN I|b d a YI|\
            b d a JE|b d a LJE|b d a NJE|b d a TSHE|b d a KJE|b d a I e GRAVE|b d a SHORT U|b d a DZHE|\
            b c a OMEGA|b d a OMEGA|b c a YAT|b d a YAT|b c a IOTIFIED E|b d a IOTIFIED E|b c a LITTLE YUS|\
            b d a LITTLE YUS|b c a IOTIFIED LITTLE YUS|b d a IOTIFIED LITTLE YUS|b c a BIG YUS|b d a BIG YUS|\
            b c a IOTIFIED BIG YUS|b d a IOTIFIED BIG YUS|b c a KSI|b d a KSI|b c a PSI|b d a PSI|b c a FITA|\
            b d a FITA|b c a IZHITSA|b d a IZHITSA|b c a IZHITSA e o GRAVE ACCENT|\
            b d a IZHITSA e o GRAVE ACCENT|b c a UK|b d a UK|b c a ROUND OMEGA|b d a ROUND OMEGA|\
            b c a OMEGA e TITLO|b d a OMEGA e TITLO|b c a OT|b d a OT|b c a KOPPA|b d a KOPPA|b THOUSANDS t|\
            COMBINING b TITLO|COMBINING b PALATALIZATION|COMBINING b DASIA PNEUMATA|COMBINING b PSILI PNEUMATA|\
            COMBINING b POKRYTIE|COMBINING b HUNDRED THOUSANDS t|COMBINING b MILLIONS t|b c a SHORT I e TAIL|\
            b d a SHORT I e TAIL|b c a SEMISOFT t|b d a SEMISOFT t|b c a ER e TICK|b d a ER e TICK|\
            b c a GHE e UPTURN|b d a GHE e UPTURN|b c a GHE e STROKE|b d a GHE e STROKE|b c a GHE e MIDDLE HOOK|\
            b d a GHE e MIDDLE HOOK|b c a ZHE e DESCENDER|b d a ZHE e DESCENDER|b c a ZE e DESCENDER|\
            b d a ZE e DESCENDER|b c a KA e DESCENDER|b d a KA e DESCENDER|b c a KA e n STROKE|\
            b d a KA e n STROKE|b c a KA e STROKE|b d a KA e STROKE|b c a BASHKIR KA|b d a BASHKIR KA|\
            b c a EN e DESCENDER|b d a EN e DESCENDER|b c LIGATURE EN GHE|b d LIGATURE EN GHE|\
            b c a PE e MIDDLE HOOK|b d a PE e MIDDLE HOOK|b c a ABKHASIAN HA|b d a ABKHASIAN HA|\
            b c a ES e DESCENDER|b d a ES e DESCENDER|b c a TE e DESCENDER|b d a TE e DESCENDER|\
            b c a STRAIGHT U|b d a STRAIGHT U|b c a STRAIGHT U e STROKE|b d a STRAIGHT U e STROKE|\
            b c a HA e DESCENDER|b d a HA e DESCENDER|b c LIGATURE TE TSE|b d LIGATURE TE TSE|\
            b c a CHE e DESCENDER|b d a CHE e DESCENDER|b c a CHE e n STROKE|b d a CHE e n STROKE|b c a SHHA|\
            b d a SHHA|b c a ABKHASIAN CHE|b d a ABKHASIAN CHE|b c a ABKHASIAN CHE e DESCENDER|\
            b d a ABKHASIAN CHE e DESCENDER|b a PALOCHKA|b c a ZHE e BREVE|b d a ZHE e BREVE|b c a KA e HOOK|\
            b d a KA e HOOK|b c a EL e TAIL|b d a EL e TAIL|b c a EN e HOOK|b d a EN e HOOK|b c a EN e TAIL|\
            b d a EN e TAIL|b c a KHAKASSIAN CHE|b d a KHAKASSIAN CHE|b c a EM e TAIL|b d a EM e TAIL|\
            b d a PALOCHKA|b c a A e BREVE|b d a A e BREVE|b c a A e u|b d a A e u|b c LIGATURE A IE|\
            b d LIGATURE A IE|b c a IE e BREVE|b d a IE e BREVE|b c a SCHWA|b d a SCHWA|b c a SCHWA e u|\
            b d a SCHWA e u|b c a ZHE e u|b d a ZHE e u|b c a ZE e u|b d a ZE e u|b c a ABKHASIAN DZE|\
            b d a ABKHASIAN DZE|b c a I e MACRON|b d a I e MACRON|b c a I e u|b d a I e u|b c a O e u|\
            b d a O e u|b c a BARRED O|b d a BARRED O|b c a BARRED O e u|b d a BARRED O e u|b c a E e u|\
            b d a E e u|b c a U e MACRON|b d a U e MACRON|b c a U e u|b d a U e u|b c a U e o ACUTE|\
            b d a U e o ACUTE|b c a CHE e u|b d a CHE e u|b c a GHE e DESCENDER|b d a GHE e DESCENDER|\
            b c a YERU e u|b d a YERU e u|b c a GHE e STROKE x HOOK|b d a GHE e STROKE x HOOK|b c a HA e HOOK|\
            b d a HA e HOOK|b c a HA e STROKE|b d a HA e STROKE",
    },
    Block {
        key: "symbol.block.punctuation",
        start: 0x2000,
        end: 0x206F,
        names: "EN QUAD|EM QUAD|EN SPACE|EM SPACE|THREE-PER-EM SPACE|FOUR-PER-EM SPACE|SIX-PER-EM SPACE|\
            FIGURE SPACE|PUNCTUATION SPACE|THIN SPACE|HAIR SPACE|ZERO WIDTH SPACE|ZERO WIDTH NON-JOINER|\
            ZERO WIDTH JOINER|LEFT-TO-RIGHT MARK|RIGHT-TO-LEFT MARK|HYPHEN|NON-BREAKING HYPHEN|FIGURE DASH|\
            EN DASH|EM DASH|p BAR|o n LINE|o LOW LINE|r SINGLE QUOTATION MARK|l SINGLE QUOTATION MARK|\
            SINGLE LOW-9 QUOTATION MARK|SINGLE HIGH-REVERSED-9 QUOTATION MARK|r o QUOTATION MARK|\
            l o QUOTATION MARK|o LOW-9 QUOTATION MARK|o HIGH-REVERSED-9 QUOTATION MARK|DAGGER|o DAGGER|BULLET|\
            TRIANGULAR BULLET|ONE DOT LEADER|TWO DOT LEADER|p ELLIPSIS|HYPHENATION POINT|LINE SEPARATOR|\
            PARAGRAPH SEPARATOR|LEFT-TO-RIGHT EMBEDDING|RIGHT-TO-LEFT EMBEDDING|POP DIRECTIONAL FORMATTING|\
            LEFT-TO-RIGHT OVERRIDE|RIGHT-TO-LEFT OVERRIDE|NARROW NO-BREAK SPACE|PER MILLE t|PER TEN THOUSAND t|\
            PRIME|o PRIME|TRIPLE PRIME|REVERSED PRIME|REVERSED o PRIME|REVERSED TRIPLE PRIME|CARET|\
            SINGLE LEFT-POINTING ANGLE QUOTATION MARK|SINGLE RIGHT-POINTING ANGLE QUOTATION MARK|REFERENCE MARK|\
            o EXCLAMATION MARK|INTERROBANG|OVERLINE|UNDERTIE|CHARACTER TIE|CARET INSERTION POINT|ASTERISM|\
            HYPHEN BULLET|FRACTION SLASH|r SQUARE BRACKET e QUILL|l SQUARE BRACKET e QUILL|o QUESTION MARK|\
            QUESTION EXCLAMATION MARK|EXCLAMATION QUESTION MARK|TIRONIAN t ET|REVERSED PILCROW t|q w BULLET|\
            q h BULLET|LOW ASTERISK|REVERSED SEMICOLON|CLOSE UP|TWO ASTERISKS ALIGNED VERTICALLY|\
            COMMERCIAL MINUS t|SWUNG DASH|INVERTED UNDERTIE|FLOWER PUNCTUATION MARK|THREE DOT PUNCTUATION|\
            QUADRUPLE PRIME|FOUR DOT PUNCTUATION|FIVE DOT PUNCTUATION|TWO DOT PUNCTUATION|FOUR DOT MARK|\
            DOTTED CROSS|TRICOLON|n FOUR DOTS|MEDIUM MATHEMATICAL SPACE|WORD JOINER|FUNCTION APPLICATION|\
            INVISIBLE TIMES|INVISIBLE SEPARATOR|INVISIBLE PLUS||LEFT-TO-RIGHT ISOLATE|RIGHT-TO-LEFT ISOLATE|\
            FIRST STRONG ISOLATE|POP DIRECTIONAL ISOLATE|INHIBIT SYMMETRIC SWAPPING|ACTIVATE SYMMETRIC SWAPPING|\
            INHIBIT ARABIC FORM SHAPING|ACTIVATE ARABIC FORM SHAPING|NATIONAL DIGIT SHAPES|NOMINAL DIGIT SHAPES",
    },
    Block {
        key: "symbol.block.currency",
        start: 0x20A0,
        end: 0x20CF,
        names: "EURO-CURRENCY t|COLON t|CRUZEIRO t|FRENCH FRANC t|LIRA t|MILL t|NAIRA t|PESETA t|RUPEE t|WON t|\
            NEW SHEQEL t|DONG t|EURO t|KIP t|TUGRIK t|DRACHMA t|GERMAN PENNY t|PESO t|GUARANI t|AUSTRAL t|\
            HRYVNIA t|CEDI t|LIVRE TOURNOIS t|SPESMILO t|TENGE t|INDIAN RUPEE t|TURKISH LIRA t|NORDIC MARK t|\
            MANAT t|RUBLE t|LARI t|BITCOIN t|SOM t|||||||||||||||",
    },
    Block {
        key: "symbol.block.letterlike",
        start: 0x2100,
        end: 0x214F,
        names: "ACCOUNT OF|ADDRESSED TO THE SUBJECT|DOUBLE-STRUCK c C|DEGREE CELSIUS|CENTRE LINE v|CARE OF|CADA UNA|\
            EULER CONSTANT|SCRUPLE|DEGREE FAHRENHEIT|SCRIPT d G|SCRIPT c H|BLACK-LETTER c H|DOUBLE-STRUCK c H|\
            PLANCK CONSTANT|PLANCK CONSTANT OVER TWO PI|SCRIPT c I|BLACK-LETTER c I|SCRIPT c L|SCRIPT d L|\
            L B BAR v|DOUBLE-STRUCK c N|NUMERO t|SOUND RECORDING COPYRIGHT|SCRIPT c P|DOUBLE-STRUCK c P|\
            DOUBLE-STRUCK c Q|SCRIPT c R|BLACK-LETTER c R|DOUBLE-STRUCK c R|PRESCRIPTION TAKE|RESPONSE|\
            SERVICE MARK|TELEPHONE t|TRADE MARK t|VERSICLE|DOUBLE-STRUCK c Z|OUNCE t|OHM t|INVERTED OHM t|\
            BLACK-LETTER c Z|TURNED k d a IOTA|KELVIN t|ANGSTROM t|SCRIPT c B|BLACK-LETTER c C|ESTIMATED v|\
            SCRIPT d E|SCRIPT c E|SCRIPT c F|TURNED c F|SCRIPT c M|SCRIPT d O|ALEF v|BET v|GIMEL v|DALET v|\
            INFORMATION SOURCE|ROTATED c Q|FACSIMILE t|DOUBLE-STRUCK d PI|DOUBLE-STRUCK d GAMMA|\
            DOUBLE-STRUCK c GAMMA|DOUBLE-STRUCK c PI|DOUBLE-STRUCK N-ARY SUMMATION|TURNED SANS-SERIF c G|\
            TURNED SANS-SERIF c L|REVERSED SANS-SERIF c L|TURNED SANS-SERIF c Y|DOUBLE-STRUCK ITALIC c D|\
            DOUBLE-STRUCK ITALIC d D|DOUBLE-STRUCK ITALIC d E|DOUBLE-STRUCK ITALIC d I|DOUBLE-STRUCK ITALIC d J|\
            PROPERTY LINE|TURNED AMPERSAND|PER t|AKTIESELSKAB|TURNED d F|v FOR SAMARITAN SOURCE",
    },
    Block {
        key: "symbol.block.number_forms",
        start: 0x2150,
        end: 0x218F,
        names: "VULGAR FRACTION ONE SEVENTH|VULGAR FRACTION ONE NINTH|VULGAR FRACTION ONE TENTH|\
            VULGAR FRACTION ONE THIRD|VULGAR FRACTION TWO THIRDS|VULGAR FRACTION ONE FIFTH|\
            VULGAR FRACTION TWO FIFTHS|VULGAR FRACTION THREE FIFTHS|VULGAR FRACTION FOUR FIFTHS|\
            VULGAR FRACTION ONE SIXTH|VULGAR FRACTION FIVE SIXTHS|VULGAR FRACTION ONE EIGHTH|\
            VULGAR FRACTION THREE EIGHTHS|VULGAR FRACTION FIVE EIGHTHS|VULGAR FRACTION SEVEN EIGHTHS|\
            FRACTION NUMERATOR ONE|ROMAN NUMERAL ONE|ROMAN NUMERAL TWO|ROMAN NUMERAL THREE|ROMAN NUMERAL FOUR|\
            ROMAN NUMERAL FIVE|ROMAN NUMERAL SIX|ROMAN NUMERAL SEVEN|ROMAN NUMERAL EIGHT|ROMAN NUMERAL NINE|\
            ROMAN NUMERAL TEN|ROMAN NUMERAL ELEVEN|ROMAN NUMERAL TWELVE|ROMAN NUMERAL FIFTY|\
            ROMAN NUMERAL ONE HUNDRED|ROMAN NUMERAL FIVE HUNDRED|ROMAN NUMERAL ONE THOUSAND|d ROMAN NUMERAL ONE|\
            d ROMAN NUMERAL TWO|d ROMAN NUMERAL THREE|d ROMAN NUMERAL FOUR|d ROMAN NUMERAL FIVE|\
            d ROMAN NUMERAL SIX|d ROMAN NUMERAL SEVEN|d ROMAN NUMERAL EIGHT|d ROMAN NUMERAL NINE|\
            d ROMAN NUMERAL TEN|d ROMAN NUMERAL ELEVEN|d ROMAN NUMERAL TWELVE|d ROMAN NUMERAL FIFTY|\
            d ROMAN NUMERAL ONE HUNDRED|d ROMAN NUMERAL FIVE HUNDRED|d ROMAN NUMERAL ONE THOUSAND|\
            ROMAN NUMERAL ONE THOUSAND C D|ROMAN NUMERAL FIVE THOUSAND|ROMAN NUMERAL TEN THOUSAND|\
            ROMAN NUMERAL REVERSED ONE HUNDRED|g d a REVERSED C|ROMAN NUMERAL SIX LATE FORM|\
            ROMAN NUMERAL FIFTY EARLY FORM|ROMAN NUMERAL FIFTY THOUSAND|ROMAN NUMERAL ONE HUNDRED THOUSAND|\
            VULGAR FRACTION ZERO THIRDS|TURNED DIGIT TWO|TURNED DIGIT THREE||||",
    },
    Block {
        key: "symbol.block.arrows",
        start: 0x2190,
        end: 0x21FF,
        names: "w i|UPWARDS i|h i|DOWNWARDS i|r l i|UP DOWN i|NORTH WEST i|NORTH EAST i|SOUTH EAST i|SOUTH WEST i|\
            w i e STROKE|h i e STROKE|w WAVE i|h WAVE i|w TWO HEADED i|UPWARDS TWO HEADED i|h TWO HEADED i|\
            DOWNWARDS TWO HEADED i|w i e TAIL|h i e TAIL|w i FROM BAR|UPWARDS i FROM BAR|h i FROM BAR|\
            DOWNWARDS i FROM BAR|UP DOWN i e BASE|w i e HOOK|h i e HOOK|w i e LOOP|h i e LOOP|r l WAVE i|\
            r l i e STROKE|DOWNWARDS ZIGZAG i|UPWARDS i e TIP w|UPWARDS i e TIP h|DOWNWARDS i e TIP w|\
            DOWNWARDS i e TIP h|h i e CORNER DOWNWARDS|DOWNWARDS i e CORNER w|ANTICLOCKWISE TOP SEMICIRCLE i|\
            CLOCKWISE TOP SEMICIRCLE i|NORTH WEST i TO LONG BAR|w i TO BAR OVER h i TO BAR|\
            ANTICLOCKWISE OPEN CIRCLE i|CLOCKWISE OPEN CIRCLE i|w HARPOON e BARB UPWARDS|\
            w HARPOON e BARB DOWNWARDS|UPWARDS HARPOON e BARB h|UPWARDS HARPOON e BARB w|\
            h HARPOON e BARB UPWARDS|h HARPOON e BARB DOWNWARDS|DOWNWARDS HARPOON e BARB h|\
            DOWNWARDS HARPOON e BARB w|h i OVER w i|UPWARDS i w OF DOWNWARDS i|w i OVER h i|w PAIRED ARROWS|\
            UPWARDS PAIRED ARROWS|h PAIRED ARROWS|DOWNWARDS PAIRED ARROWS|w HARPOON OVER h HARPOON|\
            h HARPOON OVER w HARPOON|w o i e STROKE|r l o i e STROKE|h o i e STROKE|w o i|UPWARDS o i|h o i|\
            DOWNWARDS o i|r l o i|UP DOWN o i|NORTH WEST o i|NORTH EAST o i|SOUTH EAST o i|SOUTH WEST o i|\
            w TRIPLE i|h TRIPLE i|w SQUIGGLE i|h SQUIGGLE i|UPWARDS i e o STROKE|DOWNWARDS i e o STROKE|\
            w DASHED i|UPWARDS DASHED i|h DASHED i|DOWNWARDS DASHED i|w i TO BAR|h i TO BAR|w m i|UPWARDS m i|\
            h m i|DOWNWARDS m i|UPWARDS m i FROM BAR|UPWARDS m i ON PEDESTAL|UPWARDS m i ON PEDESTAL e p BAR|\
            UPWARDS m i ON PEDESTAL e n BAR|UPWARDS m o i|UPWARDS m o i ON PEDESTAL|h m i FROM WALL|\
            NORTH WEST i TO CORNER|SOUTH EAST i TO CORNER|UP DOWN m i|l i e d CIRCLE|DOWNWARDS i w OF UPWARDS i|\
            THREE h ARROWS|w i e n STROKE|h i e n STROKE|r l i e n STROKE|w i e o n STROKE|h i e o n STROKE|\
            r l i e o n STROKE|w OPEN-HEADED i|h OPEN-HEADED i|r l OPEN-HEADED i",
    },
    Block {
        key: "symbol.block.math",
        start: 0x2200,
        end: 0x22FF,
        names: "FOR ALL|COMPLEMENT|PARTIAL DIFFERENTIAL|THERE EXISTS|THERE DOES NOT EXIST|EMPTY SET|INCREMENT|NABLA|\
            ELEMENT OF|NOT AN ELEMENT OF|d ELEMENT OF|CONTAINS AS MEMBER|DOES NOT CONTAIN AS MEMBER|\
            d CONTAINS AS MEMBER|END OF PROOF|N-ARY PRODUCT|N-ARY COPRODUCT|N-ARY SUMMATION|MINUS t|\
            MINUS-OR-PLUS t|DOT PLUS|DIVISION SLASH|SET MINUS|ASTERISK OPERATOR|RING OPERATOR|BULLET OPERATOR|\
            SQUARE ROOT|CUBE ROOT|FOURTH ROOT|PROPORTIONAL TO|INFINITY|l ANGLE|ANGLE|MEASURED ANGLE|\
            SPHERICAL ANGLE|DIVIDES|DOES NOT DIVIDE|PARALLEL TO|NOT PARALLEL TO|LOGICAL x|LOGICAL OR|\
            INTERSECTION|UNION|INTEGRAL|o INTEGRAL|TRIPLE INTEGRAL|CONTOUR INTEGRAL|SURFACE INTEGRAL|\
            VOLUME INTEGRAL|CLOCKWISE INTEGRAL|CLOCKWISE CONTOUR INTEGRAL|ANTICLOCKWISE CONTOUR INTEGRAL|\
            THEREFORE|BECAUSE|RATIO|PROPORTION|DOT MINUS|EXCESS|GEOMETRIC PROPORTION|HOMOTHETIC|TILDE OPERATOR|\
            REVERSED TILDE|INVERTED LAZY S|SINE WAVE|WREATH PRODUCT|NOT TILDE|MINUS TILDE|\
            ASYMPTOTICALLY EQUAL TO|NOT ASYMPTOTICALLY EQUAL TO|APPROXIMATELY EQUAL TO|\
            APPROXIMATELY BUT NOT ACTUALLY EQUAL TO|NEITHER APPROXIMATELY NOR ACTUALLY EQUAL TO|ALMOST EQUAL TO|\
            NOT ALMOST EQUAL TO|ALMOST EQUAL OR EQUAL TO|TRIPLE TILDE|ALL EQUAL TO|EQUIVALENT TO|\
            GEOMETRICALLY EQUIVALENT TO|DIFFERENCE BETWEEN|APPROACHES THE LIMIT|GEOMETRICALLY EQUAL TO|\
            APPROXIMATELY EQUAL TO OR THE IMAGE OF|IMAGE OF OR APPROXIMATELY EQUAL TO|COLON EQUALS|EQUALS COLON|\
            RING IN EQUAL TO|RING EQUAL TO|CORRESPONDS TO|ESTIMATES|EQUIANGULAR TO|STAR EQUALS|DELTA EQUAL TO|\
            EQUAL TO BY DEFINITION|MEASURED BY|QUESTIONED EQUAL TO|NOT EQUAL TO|IDENTICAL TO|NOT IDENTICAL TO|\
            STRICTLY EQUIVALENT TO|LESS-THAN OR EQUAL TO|GREATER-THAN OR EQUAL TO|LESS-THAN OVER EQUAL TO|\
            GREATER-THAN OVER EQUAL TO|LESS-THAN BUT NOT EQUAL TO|GREATER-THAN BUT NOT EQUAL TO|MUCH LESS-THAN|\
            MUCH GREATER-THAN|BETWEEN|NOT EQUIVALENT TO|NOT LESS-THAN|NOT GREATER-THAN|\
            NEITHER LESS-THAN NOR EQUAL TO|NEITHER GREATER-THAN NOR EQUAL TO|LESS-THAN OR EQUIVALENT TO|\
            GREATER-THAN OR EQUIVALENT TO|NEITHER LESS-THAN NOR EQUIVALENT TO|\
            NEITHER GREATER-THAN NOR EQUIVALENT TO|LESS-THAN OR GREATER-THAN|GREATER-THAN OR LESS-THAN|\
            NEITHER LESS-THAN NOR GREATER-THAN|NEITHER GREATER-THAN NOR LESS-THAN|PRECEDES|SUCCEEDS|\
            PRECEDES OR EQUAL TO|SUCCEEDS OR EQUAL TO|PRECEDES OR EQUIVALENT TO|SUCCEEDS OR EQUIVALENT TO|\
            DOES NOT PRECEDE|DOES NOT SUCCEED|SUBSET OF|SUPERSET OF|NOT A SUBSET OF|NOT A SUPERSET OF|\
            SUBSET OF OR EQUAL TO|SUPERSET OF OR EQUAL TO|NEITHER A SUBSET OF NOR EQUAL TO|\
            NEITHER A SUPERSET OF NOR EQUAL TO|SUBSET OF e NOT EQUAL TO|SUPERSET OF e NOT EQUAL TO|MULTISET|\
            MULTISET MULTIPLICATION|MULTISET UNION|SQUARE IMAGE OF|SQUARE ORIGINAL OF|\
            SQUARE IMAGE OF OR EQUAL TO|SQUARE ORIGINAL OF OR EQUAL TO|SQUARE CAP|SQUARE CUP|z PLUS|z MINUS|\
            z TIMES|z DIVISION SLASH|z DOT OPERATOR|z RING OPERATOR|z ASTERISK OPERATOR|z EQUALS|z DASH|\
            SQUARED PLUS|SQUARED MINUS|SQUARED TIMES|SQUARED DOT OPERATOR|l TACK|r TACK|DOWN TACK|UP TACK|\
            ASSERTION|MODELS|TRUE|FORCES|TRIPLE n BAR l TURNSTILE|o n BAR o l TURNSTILE|DOES NOT PROVE|NOT TRUE|\
            DOES NOT FORCE|NEGATED o n BAR o l TURNSTILE|PRECEDES UNDER RELATION|SUCCEEDS UNDER RELATION|\
            NORMAL SUBGROUP OF|CONTAINS AS NORMAL SUBGROUP|NORMAL SUBGROUP OF OR EQUAL TO|\
            CONTAINS AS NORMAL SUBGROUP OR EQUAL TO|ORIGINAL OF|IMAGE OF|MULTIMAP|HERMITIAN CONJUGATE MATRIX|\
            INTERCALATE|XOR|NAND|NOR|l ANGLE e ARC|l TRIANGLE|N-ARY LOGICAL x|N-ARY LOGICAL OR|\
            N-ARY INTERSECTION|N-ARY UNION|DIAMOND OPERATOR|DOT OPERATOR|STAR OPERATOR|DIVISION TIMES|BOWTIE|\
            r NORMAL FACTOR SEMIDIRECT PRODUCT|l NORMAL FACTOR SEMIDIRECT PRODUCT|r SEMIDIRECT PRODUCT|\
            l SEMIDIRECT PRODUCT|REVERSED TILDE EQUALS|CURLY LOGICAL OR|CURLY LOGICAL x|o SUBSET|o SUPERSET|\
            o INTERSECTION|o UNION|PITCHFORK|EQUAL x PARALLEL TO|LESS-THAN e DOT|GREATER-THAN e DOT|\
            VERY MUCH LESS-THAN|VERY MUCH GREATER-THAN|LESS-THAN EQUAL TO OR GREATER-THAN|\
            GREATER-THAN EQUAL TO OR LESS-THAN|EQUAL TO OR LESS-THAN|EQUAL TO OR GREATER-THAN|\
            EQUAL TO OR PRECEDES|EQUAL TO OR SUCCEEDS|DOES NOT PRECEDE OR EQUAL|DOES NOT SUCCEED OR EQUAL|\
            NOT SQUARE IMAGE OF OR EQUAL TO|NOT SQUARE ORIGINAL OF OR EQUAL TO|SQUARE IMAGE OF OR NOT EQUAL TO|\
            SQUARE ORIGINAL OF OR NOT EQUAL TO|LESS-THAN BUT NOT EQUIVALENT TO|\
            GREATER-THAN BUT NOT EQUIVALENT TO|PRECEDES BUT NOT EQUIVALENT TO|SUCCEEDS BUT NOT EQUIVALENT TO|\
            NOT NORMAL SUBGROUP OF|DOES NOT CONTAIN AS NORMAL SUBGROUP|NOT NORMAL SUBGROUP OF OR EQUAL TO|\
            DOES NOT CONTAIN AS NORMAL SUBGROUP OR EQUAL|n ELLIPSIS|MIDLINE p ELLIPSIS|UP l DIAGONAL ELLIPSIS|\
            DOWN l DIAGONAL ELLIPSIS|ELEMENT OF e LONG p STROKE|ELEMENT OF e n BAR AT END OF p STROKE|\
            d ELEMENT OF e n BAR AT END OF p STROKE|ELEMENT OF e DOT ABOVE|ELEMENT OF e OVERBAR|\
            d ELEMENT OF e OVERBAR|ELEMENT OF e UNDERBAR|ELEMENT OF e TWO p STROKES|CONTAINS e LONG p STROKE|\
            CONTAINS e n BAR AT END OF p STROKE|d CONTAINS e n BAR AT END OF p STROKE|CONTAINS e OVERBAR|\
            d CONTAINS e OVERBAR|Z NOTATION BAG MEMBERSHIP",
    },
    Block {
        key: "symbol.block.box",
        start: 0x2500,
        end: 0x257F,
        names: "y f s p|y f j p|y f s n|y f j n|y f s TRIPLE DASH p|y f j TRIPLE DASH p|y f s TRIPLE DASH n|\
            y f j TRIPLE DASH n|y f s QUADRUPLE DASH p|y f j QUADRUPLE DASH p|y f s QUADRUPLE DASH n|\
            y f j QUADRUPLE DASH n|y f s DOWN x l|y f DOWN s x l j|y f DOWN j x l s|y f j DOWN x l|\
            y f s DOWN x r|y f DOWN s x r j|y f DOWN j x r s|y f j DOWN x r|y f s UP x l|y f UP s x l j|\
            y f UP j x l s|y f j UP x l|y f s UP x r|y f UP s x r j|y f UP j x r s|y f j UP x r|y f s n x l|\
            y f n s x l j|y f UP j x l DOWN s|y f DOWN j x l UP s|y f n j x l s|y f DOWN s x l UP j|\
            y f UP s x l DOWN j|y f j n x l|y f s n x r|y f n s x r j|y f UP j x r DOWN s|y f DOWN j x r UP s|\
            y f n j x r s|y f DOWN s x r UP j|y f UP s x r DOWN j|y f j n x r|y f s DOWN x p|y f r j x l DOWN s|\
            y f l j x r DOWN s|y f DOWN s x p j|y f DOWN j x p s|y f l s x r DOWN j|y f r s x l DOWN j|\
            y f j DOWN x p|y f s UP x p|y f r j x l UP s|y f l j x r UP s|y f UP s x p j|y f UP j x p s|\
            y f l s x r UP j|y f r s x l UP j|y f j UP x p|y f s n x p|y f r j x l n s|y f l j x r n s|\
            y f n s x p j|y f UP j x DOWN p s|y f DOWN j x UP p s|y f n j x p s|y f r UP j x l DOWN s|\
            y f l UP j x r DOWN s|y f r DOWN j x l UP s|y f l DOWN j x r UP s|y f DOWN s x UP p j|\
            y f UP s x DOWN p j|y f l s x r n j|y f r s x l n j|y f j n x p|y f s o DASH p|y f j o DASH p|\
            y f s o DASH n|y f j o DASH n|y f o p|y f o n|y f DOWN SINGLE x l o|y f DOWN o x l SINGLE|\
            y f o DOWN x l|y f DOWN SINGLE x r o|y f DOWN o x r SINGLE|y f o DOWN x r|y f UP SINGLE x l o|\
            y f UP o x l SINGLE|y f o UP x l|y f UP SINGLE x r o|y f UP o x r SINGLE|y f o UP x r|\
            y f n SINGLE x l o|y f n o x l SINGLE|y f o n x l|y f n SINGLE x r o|y f n o x r SINGLE|y f o n x r|\
            y f DOWN SINGLE x p o|y f DOWN o x p SINGLE|y f o DOWN x p|y f UP SINGLE x p o|y f UP o x p SINGLE|\
            y f o UP x p|y f n SINGLE x p o|y f n o x p SINGLE|y f o n x p|y f s ARC DOWN x l|\
            y f s ARC DOWN x r|y f s ARC UP x r|y f s ARC UP x l|y f s DIAGONAL UPPER l TO LOWER r|\
            y f s DIAGONAL UPPER r TO LOWER l|y f s DIAGONAL CROSS|y f s r|y f s UP|y f s l|y f s DOWN|y f j r|\
            y f j UP|y f j l|y f j DOWN|y f s r x j l|y f s UP x j DOWN|y f j r x s l|y f j UP x s DOWN",
    },
    Block {
        key: "symbol.block.block",
        start: 0x2580,
        end: 0x259F,
        names: "UPPER HALF BLOCK|LOWER ONE EIGHTH BLOCK|LOWER ONE QUARTER BLOCK|LOWER THREE EIGHTHS BLOCK|\
            LOWER HALF BLOCK|LOWER FIVE EIGHTHS BLOCK|LOWER THREE QUARTERS BLOCK|LOWER SEVEN EIGHTHS BLOCK|\
            FULL BLOCK|r SEVEN EIGHTHS BLOCK|r THREE QUARTERS BLOCK|r FIVE EIGHTHS BLOCK|r HALF BLOCK|\
            r THREE EIGHTHS BLOCK|r ONE QUARTER BLOCK|r ONE EIGHTH BLOCK|l HALF BLOCK|s SHADE|MEDIUM SHADE|\
            DARK SHADE|UPPER ONE EIGHTH BLOCK|l ONE EIGHTH BLOCK|QUADRANT LOWER r|QUADRANT LOWER l|\
            QUADRANT UPPER r|QUADRANT UPPER r x LOWER r x LOWER l|QUADRANT UPPER r x LOWER l|\
            QUADRANT UPPER r x UPPER l x LOWER r|QUADRANT UPPER r x UPPER l x LOWER l|QUADRANT UPPER l|\
            QUADRANT UPPER l x LOWER r|QUADRANT UPPER l x LOWER r x LOWER l",
    },
    Block {
        key: "symbol.block.geometric",
        start: 0x25A0,
        end: 0x25FF,
        names: "q SQUARE|m SQUARE|m SQUARE e ROUNDED CORNERS|m SQUARE CONTAINING q d SQUARE|SQUARE e p FILL|\
            SQUARE e n FILL|SQUARE e ORTHOGONAL CROSSHATCH FILL|SQUARE e UPPER r TO LOWER l FILL|\
            SQUARE e UPPER l TO LOWER r FILL|SQUARE e DIAGONAL CROSSHATCH FILL|q d SQUARE|m d SQUARE|\
            q RECTANGLE|m RECTANGLE|q n RECTANGLE|m n RECTANGLE|q PARALLELOGRAM|m PARALLELOGRAM|\
            q UP-POINTING TRIANGLE|m UP-POINTING TRIANGLE|q UP-POINTING d TRIANGLE|m UP-POINTING d TRIANGLE|\
            q RIGHT-POINTING TRIANGLE|m RIGHT-POINTING TRIANGLE|q RIGHT-POINTING d TRIANGLE|\
            m RIGHT-POINTING d TRIANGLE|q RIGHT-POINTING POINTER|m RIGHT-POINTING POINTER|\
            q DOWN-POINTING TRIANGLE|m DOWN-POINTING TRIANGLE|q DOWN-POINTING d TRIANGLE|\
            m DOWN-POINTING d TRIANGLE|q LEFT-POINTING TRIANGLE|m LEFT-POINTING TRIANGLE|\
            q LEFT-POINTING d TRIANGLE|m LEFT-POINTING d TRIANGLE|q LEFT-POINTING POINTER|\
            m LEFT-POINTING POINTER|q DIAMOND|m DIAMOND|m DIAMOND CONTAINING q d DIAMOND|FISHEYE|LOZENGE|\
            m CIRCLE|DOTTED CIRCLE|CIRCLE e n FILL|BULLSEYE|q CIRCLE|CIRCLE e r HALF q|CIRCLE e l HALF q|\
            CIRCLE e LOWER HALF q|CIRCLE e UPPER HALF q|CIRCLE e UPPER l QUADRANT q|\
            CIRCLE e ALL BUT UPPER r QUADRANT q|r HALF q CIRCLE|l HALF q CIRCLE|INVERSE BULLET|INVERSE m CIRCLE|\
            UPPER HALF INVERSE m CIRCLE|LOWER HALF INVERSE m CIRCLE|UPPER r QUADRANT CIRCULAR ARC|\
            UPPER l QUADRANT CIRCULAR ARC|LOWER l QUADRANT CIRCULAR ARC|LOWER r QUADRANT CIRCULAR ARC|\
            UPPER HALF CIRCLE|LOWER HALF CIRCLE|q LOWER l TRIANGLE|q LOWER r TRIANGLE|q UPPER r TRIANGLE|\
            q UPPER l TRIANGLE|m BULLET|SQUARE e r HALF q|SQUARE e l HALF q|SQUARE e UPPER r DIAGONAL HALF q|\
            SQUARE e LOWER l DIAGONAL HALF q|m SQUARE e n BISECTING LINE|m UP-POINTING TRIANGLE e DOT|\
            UP-POINTING TRIANGLE e r HALF q|UP-POINTING TRIANGLE e l HALF q|LARGE CIRCLE|\
            m SQUARE e UPPER r QUADRANT|m SQUARE e LOWER r QUADRANT|m SQUARE e LOWER l QUADRANT|\
            m SQUARE e UPPER l QUADRANT|m CIRCLE e UPPER r QUADRANT|m CIRCLE e LOWER r QUADRANT|\
            m CIRCLE e LOWER l QUADRANT|m CIRCLE e UPPER l QUADRANT|UPPER r TRIANGLE|UPPER l TRIANGLE|\
            LOWER r TRIANGLE|m MEDIUM SQUARE|q MEDIUM SQUARE|m MEDIUM d SQUARE|q MEDIUM d SQUARE|\
            LOWER l TRIANGLE",
    },
    Block {
        key: "symbol.block.misc",
        start: 0x2600,
        end: 0x26FF,
        names: "q SUN e RAYS|CLOUD|UMBRELLA|SNOWMAN|COMET|q STAR|m STAR|LIGHTNING|THUNDERSTORM|SUN|ASCENDING NODE|\
            DESCENDING NODE|CONJUNCTION|OPPOSITION|q TELEPHONE|m TELEPHONE|BALLOT y|BALLOT y e CHECK|\
            BALLOT y e X|SALTIRE|UMBRELLA e RAIN DROPS|HOT BEVERAGE|m SHOGI PIECE|q SHOGI PIECE|SHAMROCK|\
            REVERSED ROTATED FLORAL HEART BULLET|q r POINTING INDEX|q l POINTING INDEX|m r POINTING INDEX|\
            m UP POINTING INDEX|m l POINTING INDEX|m DOWN POINTING INDEX|SKULL x CROSSBONES|CAUTION t|\
            RADIOACTIVE t|BIOHAZARD t|CADUCEUS|ANKH|ORTHODOX CROSS|CHI RHO|CROSS OF LORRAINE|CROSS OF JERUSALEM|\
            STAR x CRESCENT|FARSI v|ADI SHAKTI|HAMMER x SICKLE|PEACE v|YIN YANG|TRIGRAM FOR HEAVEN|\
            TRIGRAM FOR LAKE|TRIGRAM FOR FIRE|TRIGRAM FOR THUNDER|TRIGRAM FOR WIND|TRIGRAM FOR WATER|\
            TRIGRAM FOR MOUNTAIN|TRIGRAM FOR EARTH|WHEEL OF DHARMA|m FROWNING FACE|m SMILING FACE|\
            q SMILING FACE|m SUN e RAYS|FIRST QUARTER MOON|LAST QUARTER MOON|MERCURY|FEMALE t|EARTH|MALE t|\
            JUPITER|SATURN|URANUS|NEPTUNE|PLUTO|ARIES|TAURUS|GEMINI|CANCER|LEO|VIRGO|LIBRA|SCORPIUS|SAGITTARIUS|\
            CAPRICORN|AQUARIUS|PISCES|m CHESS KING|m CHESS QUEEN|m CHESS ROOK|m CHESS BISHOP|m CHESS KNIGHT|\
            m CHESS PAWN|q CHESS KING|q CHESS QUEEN|q CHESS ROOK|q CHESS BISHOP|q CHESS KNIGHT|q CHESS PAWN|\
            q SPADE SUIT|m HEART SUIT|m DIAMOND SUIT|q CLUB SUIT|m SPADE SUIT|q HEART SUIT|q DIAMOND SUIT|\
            m CLUB SUIT|HOT SPRINGS|QUARTER NOTE|EIGHTH NOTE|BEAMED EIGHTH NOTES|BEAMED SIXTEENTH NOTES|\
            MUSIC FLAT t|MUSIC NATURAL t|MUSIC SHARP t|WEST SYRIAC CROSS|EAST SYRIAC CROSS|\
            UNIVERSAL RECYCLING v|RECYCLING v FOR TYPE-1 PLASTICS|RECYCLING v FOR TYPE-2 PLASTICS|\
            RECYCLING v FOR TYPE-3 PLASTICS|RECYCLING v FOR TYPE-4 PLASTICS|RECYCLING v FOR TYPE-5 PLASTICS|\
            RECYCLING v FOR TYPE-6 PLASTICS|RECYCLING v FOR TYPE-7 PLASTICS|RECYCLING v FOR GENERIC MATERIALS|\
            q UNIVERSAL RECYCLING v|RECYCLED PAPER v|PARTIALLY-RECYCLED PAPER v|PERMANENT PAPER t|WHEELCHAIR v|\
            DIE FACE-1|DIE FACE-2|DIE FACE-3|DIE FACE-4|DIE FACE-5|DIE FACE-6|m CIRCLE e DOT l|\
            m CIRCLE e TWO DOTS|q CIRCLE e m DOT l|q CIRCLE e TWO m DOTS|MONOGRAM FOR YANG|MONOGRAM FOR YIN|\
            DIGRAM FOR GREATER YANG|DIGRAM FOR LESSER YIN|DIGRAM FOR LESSER YANG|DIGRAM FOR GREATER YIN|m FLAG|\
            q FLAG|HAMMER x PICK|ANCHOR|CROSSED SWORDS|STAFF OF AESCULAPIUS|SCALES|ALEMBIC|FLOWER|GEAR|\
            STAFF OF HERMES|ATOM v|FLEUR-DE-LIS|OUTLINED m STAR|THREE LINES CONVERGING l|\
            THREE LINES CONVERGING r|WARNING t|HIGH VOLTAGE t|DOUBLED FEMALE t|DOUBLED MALE t|\
            INTERLOCKED FEMALE x MALE t|MALE x FEMALE t|MALE e STROKE t|MALE e STROKE x MALE x FEMALE t|\
            n MALE e STROKE t|p MALE e STROKE t|MEDIUM m CIRCLE|MEDIUM q CIRCLE|MEDIUM d m CIRCLE|MARRIAGE v|\
            DIVORCE v|UNMARRIED PARTNERSHIP v|COFFIN|FUNERAL URN|NEUTER|CERES|PALLAS|JUNO|VESTA|CHIRON|\
            q MOON LILITH|SEXTILE|SEMISEXTILE|QUINCUNX|SESQUIQUADRATE|SOCCER BALL|BASEBALL|SQUARED KEY|\
            m DRAUGHTS MAN|m DRAUGHTS KING|q DRAUGHTS MAN|q DRAUGHTS KING|SNOWMAN WITHOUT SNOW|SUN BEHIND CLOUD|\
            RAIN|q SNOWMAN|THUNDER CLOUD x RAIN|TURNED m SHOGI PIECE|TURNED q SHOGI PIECE|m DIAMOND IN SQUARE|\
            CROSSING LANES|DISABLED CAR|OPHIUCHUS|PICK|CAR SLIDING|HELMET e m CROSS|z CROSSING LANES|CHAINS|\
            NO ENTRY|ALTERNATE ONE-WAY r WAY TRAFFIC|q TWO-WAY r WAY TRAFFIC|m TWO-WAY r WAY TRAFFIC|\
            q r LANE MERGE|m r LANE MERGE|DRIVE SLOW t|j m DOWN-POINTING TRIANGLE|r CLOSED ENTRY|\
            SQUARED SALTIRE|FALLING DIAGONAL IN m CIRCLE IN q SQUARE|q TRUCK|RESTRICTED r ENTRY-1|\
            RESTRICTED r ENTRY-2|ASTRONOMICAL v FOR URANUS|j CIRCLE e STROKE x TWO DOTS ABOVE|PENTAGRAM|\
            RIGHT-HANDED INTERLACED PENTAGRAM|LEFT-HANDED INTERLACED PENTAGRAM|INVERTED PENTAGRAM|\
            q CROSS ON SHIELD|SHINTO SHRINE|CHURCH|CASTLE|HISTORIC SITE|GEAR WITHOUT HUB|GEAR e HANDLES|\
            MAP v FOR LIGHTHOUSE|MOUNTAIN|UMBRELLA ON GROUND|FOUNTAIN|FLAG IN HOLE|FERRY|SAILBOAT|\
            SQUARE FOUR CORNERS|SKIER|ICE SKATE|PERSON e BALL|TENT|JAPANESE BANK v|HEADSTONE GRAVEYARD v|\
            FUEL PUMP|CUP ON q SQUARE|m FLAG e p MIDDLE q STRIPE",
    },
    Block {
        key: "symbol.block.dingbats",
        start: 0x2700,
        end: 0x27BF,
        names: "q SAFETY SCISSORS|UPPER BLADE SCISSORS|q SCISSORS|LOWER BLADE SCISSORS|m SCISSORS|m j CHECK MARK|\
            TELEPHONE LOCATION t|TAPE DRIVE|AIRPLANE|ENVELOPE|RAISED FIST|RAISED HAND|VICTORY HAND|WRITING HAND|\
            LOWER l PENCIL|PENCIL|UPPER l PENCIL|m NIB|q NIB|CHECK MARK|j CHECK MARK|MULTIPLICATION X|\
            j MULTIPLICATION X|BALLOT X|j BALLOT X|OUTLINED k CROSS|j k CROSS|OPEN CENTRE CROSS|\
            j OPEN CENTRE CROSS|g CROSS|SHADOWED m g CROSS|OUTLINED g CROSS|MALTESE CROSS|STAR OF DAVID|\
            FOUR TEARDROP-SPOKED ASTERISK|FOUR BALLOON-SPOKED ASTERISK|j FOUR BALLOON-SPOKED ASTERISK|\
            FOUR CLUB-SPOKED ASTERISK|q FOUR POINTED STAR|m FOUR POINTED STAR|SPARKLES|STRESS OUTLINED m STAR|\
            z m STAR|OPEN CENTRE q STAR|q CENTRE m STAR|OUTLINED q STAR|j OUTLINED q STAR|PINWHEEL STAR|\
            SHADOWED m STAR|j ASTERISK|OPEN CENTRE ASTERISK|EIGHT SPOKED ASTERISK|EIGHT POINTED q STAR|\
            EIGHT POINTED PINWHEEL STAR|SIX POINTED q STAR|EIGHT POINTED RECTILINEAR q STAR|\
            j EIGHT POINTED RECTILINEAR q STAR|TWELVE POINTED q STAR|SIXTEEN POINTED ASTERISK|\
            TEARDROP-SPOKED ASTERISK|OPEN CENTRE TEARDROP-SPOKED ASTERISK|j TEARDROP-SPOKED ASTERISK|\
            SIX PETALLED q x m FLORETTE|q FLORETTE|m FLORETTE|EIGHT PETALLED OUTLINED q FLORETTE|\
            z OPEN CENTRE EIGHT POINTED STAR|j TEARDROP-SPOKED PINWHEEL ASTERISK|SNOWFLAKE|\
            TIGHT TRIFOLIATE SNOWFLAKE|j CHEVRON SNOWFLAKE|SPARKLE|j SPARKLE|BALLOON-SPOKED ASTERISK|\
            EIGHT TEARDROP-SPOKED PROPELLER ASTERISK|j EIGHT TEARDROP-SPOKED PROPELLER ASTERISK|CROSS MARK|\
            SHADOWED m CIRCLE|NEGATIVE SQUARED CROSS MARK|LOWER l DROP-SHADOWED m SQUARE|\
            UPPER l DROP-SHADOWED m SQUARE|LOWER l SHADOWED m SQUARE|UPPER l SHADOWED m SQUARE|\
            q QUESTION MARK ORNAMENT|m QUESTION MARK ORNAMENT|m EXCLAMATION MARK ORNAMENT|q DIAMOND MINUS m X|\
            j EXCLAMATION MARK v|s n BAR|MEDIUM n BAR|j n BAR|j SINGLE TURNED COMMA QUOTATION MARK ORNAMENT|\
            j SINGLE COMMA QUOTATION MARK ORNAMENT|j o TURNED COMMA QUOTATION MARK ORNAMENT|\
            j o COMMA QUOTATION MARK ORNAMENT|j LOW SINGLE COMMA QUOTATION MARK ORNAMENT|\
            j LOW o COMMA QUOTATION MARK ORNAMENT|CURVED STEM PARAGRAPH t ORNAMENT|j EXCLAMATION MARK ORNAMENT|\
            j HEART EXCLAMATION MARK ORNAMENT|j q HEART|ROTATED j q HEART BULLET|FLORAL HEART|\
            ROTATED FLORAL HEART BULLET|MEDIUM r PARENTHESIS ORNAMENT|MEDIUM l PARENTHESIS ORNAMENT|\
            MEDIUM FLATTENED r PARENTHESIS ORNAMENT|MEDIUM FLATTENED l PARENTHESIS ORNAMENT|\
            MEDIUM LEFT-POINTING ANGLE BRACKET ORNAMENT|MEDIUM RIGHT-POINTING ANGLE BRACKET ORNAMENT|\
            j LEFT-POINTING ANGLE QUOTATION MARK ORNAMENT|j RIGHT-POINTING ANGLE QUOTATION MARK ORNAMENT|\
            j LEFT-POINTING ANGLE BRACKET ORNAMENT|j RIGHT-POINTING ANGLE BRACKET ORNAMENT|\
            s r TORTOISE SHELL BRACKET ORNAMENT|s l TORTOISE SHELL BRACKET ORNAMENT|\
            MEDIUM r CURLY BRACKET ORNAMENT|MEDIUM l CURLY BRACKET ORNAMENT|DINGBAT NEGATIVE z DIGIT ONE|\
            DINGBAT NEGATIVE z DIGIT TWO|DINGBAT NEGATIVE z DIGIT THREE|DINGBAT NEGATIVE z DIGIT FOUR|\
            DINGBAT NEGATIVE z DIGIT FIVE|DINGBAT NEGATIVE z DIGIT SIX|DINGBAT NEGATIVE z DIGIT SEVEN|\
            DINGBAT NEGATIVE z DIGIT EIGHT|DINGBAT NEGATIVE z DIGIT NINE|DINGBAT NEGATIVE z NUMBER TEN|\
            DINGBAT z SANS-SERIF DIGIT ONE|DINGBAT z SANS-SERIF DIGIT TWO|DINGBAT z SANS-SERIF DIGIT THREE|\
            DINGBAT z SANS-SERIF DIGIT FOUR|DINGBAT z SANS-SERIF DIGIT FIVE|DINGBAT z SANS-SERIF DIGIT SIX|\
            DINGBAT z SANS-SERIF DIGIT SEVEN|DINGBAT z SANS-SERIF DIGIT EIGHT|DINGBAT z SANS-SERIF DIGIT NINE|\
            DINGBAT z SANS-SERIF NUMBER TEN|DINGBAT NEGATIVE z SANS-SERIF DIGIT ONE|\
            DINGBAT NEGATIVE z SANS-SERIF DIGIT TWO|DINGBAT NEGATIVE z SANS-SERIF DIGIT THREE|\
            DINGBAT NEGATIVE z SANS-SERIF DIGIT FOUR|DINGBAT NEGATIVE z SANS-SERIF DIGIT FIVE|\
            DINGBAT NEGATIVE z SANS-SERIF DIGIT SIX|DINGBAT NEGATIVE z SANS-SERIF DIGIT SEVEN|\
            DINGBAT NEGATIVE z SANS-SERIF DIGIT EIGHT|DINGBAT NEGATIVE z SANS-SERIF DIGIT NINE|\
            DINGBAT NEGATIVE z SANS-SERIF NUMBER TEN|j WIDE-HEADED h i|j PLUS t|j MINUS t|j DIVISION t|\
            j SOUTH EAST i|j h i|j NORTH EAST i|DRAFTING POINT h i|j ROUND-TIPPED h i|TRIANGLE-HEADED h i|\
            j TRIANGLE-HEADED h i|DASHED TRIANGLE-HEADED h i|j DASHED TRIANGLE-HEADED h i|q h i|\
            THREE-D TOP-LIGHTED h ARROWHEAD|THREE-D BOTTOM-LIGHTED h ARROWHEAD|q h ARROWHEAD|\
            j q CURVED DOWNWARDS x h i|j q CURVED UPWARDS x h i|SQUAT q h i|j CONCAVE-POINTED q h i|\
            RIGHT-SHADED m h i|LEFT-SHADED m h i|BACK-TILTED SHADOWED m h i|FRONT-TILTED SHADOWED m h i|\
            j LOWER RIGHT-SHADOWED m h i|j UPPER RIGHT-SHADOWED m h i|NOTCHED LOWER RIGHT-SHADOWED m h i|\
            CURLY LOOP|NOTCHED UPPER RIGHT-SHADOWED m h i|z j m h i|WHITE-FEATHERED h i|\
            BLACK-FEATHERED SOUTH EAST i|BLACK-FEATHERED h i|BLACK-FEATHERED NORTH EAST i|\
            j BLACK-FEATHERED SOUTH EAST i|j BLACK-FEATHERED h i|j BLACK-FEATHERED NORTH EAST i|\
            TEARDROP-BARBED h i|j TEARDROP-SHANKED h i|WEDGE-TAILED h i|j WEDGE-TAILED h i|OPEN-OUTLINED h i|\
            o CURLY LOOP",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_match_their_ranges() {
        let mut prev_end = 0;
        for b in &BLOCKS {
            assert!(b.start > prev_end || prev_end == 0, "{}", b.key);
            assert_eq!(b.names.split('|').count() as u32, b.end - b.start + 1, "{}", b.key);
            assert!(b.chars().count() > 0);
            prev_end = b.end;
        }
        assert_eq!(block_of('é'), Some(0));
        assert_eq!(block_of('A'), None);
        assert_eq!(block_of('\u{27BF}'), Some(BLOCKS.len() - 1));
    }

    #[test]
    fn names() {
        assert_eq!(char_name('é').as_deref(), Some("Latin Small Letter E With Acute"));
        assert_eq!(char_name('\u{00A0}').as_deref(), Some("No-Break Space"));
        assert_eq!(char_name('Ж').as_deref(), Some("Cyrillic Capital Letter Zhe"));
        assert_eq!(char_name('€').as_deref(), Some("Euro Sign"));
        assert_eq!(char_name('→').as_deref(), Some("Rightwards Arrow"));
        assert_eq!(char_name('─').as_deref(), Some("Box Drawings Light Horizontal"));
        assert_eq!(char_name('✓').as_deref(), Some("Check Mark"));
        // Unassigned code points and characters outside the blocks.
        assert_eq!(char_name('\u{0378}'), None);
        assert_eq!(char_name('a'), None);
        // Every assigned character has a name.
        for b in &BLOCKS {
            assert!(b.chars().all(|c| char_name(c).is_some_and(|n| !n.is_empty() && !n.contains("  "))));
        }
    }

    #[test]
    fn hex_codes() {
        assert_eq!(parse_hex("00E9"), Some('é'));
        assert_eq!(parse_hex(" u+20ac "), Some('€'));
        assert_eq!(parse_hex("0x2192"), Some('→'));
        assert_eq!(parse_hex("41"), Some('A'));
        assert_eq!(parse_hex("1F600"), Some('\u{1F600}'));
        for bad in ["", "U+", "xyz", "D800", "110000", "1234567", "-1", "+41"] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
        assert_eq!(code_text('é'), "U+00E9");
        assert_eq!(code_text('\u{1F600}'), "U+1F600");
    }

    #[test]
    fn recents_most_recent_first() {
        let mut v = Vec::new();
        for c in "abcabd".chars() {
            add_recent(&mut v, c);
        }
        assert_eq!(v, vec!['d', 'b', 'a', 'c']);
        for i in 0..40 {
            add_recent(&mut v, char::from_u32(0x2600 + i).unwrap_or('x'));
        }
        assert_eq!(v.len(), MAX_RECENT);
        assert_eq!(v[0], '\u{2627}');
    }

    #[test]
    fn visibility_and_steps() {
        assert!(visible('é') && visible('—'));
        assert!(!visible('\u{00A0}') && !visible('\u{00AD}') && !visible('\u{200B}') && !visible('\u{0483}'));
        assert!(SPECIALS.iter().all(|s| tr(s.key) != s.key));
        assert!(BLOCKS.iter().all(|b| tr(b.key) != b.key));
        assert_eq!(step_index(0, None, 1), None);
        assert_eq!(step_index(10, None, 5), Some(0));
        assert_eq!(step_index(10, Some(3), -16), Some(0));
        assert_eq!(step_index(10, Some(3), isize::MAX), Some(9));
        assert_eq!(step_index(10, Some(3), 1), Some(4));
    }
}
