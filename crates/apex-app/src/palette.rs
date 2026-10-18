//! Command search (Alt+Q): finds any button of the toolstrip or the File menu
//! by name and runs it.

use eframe::egui::{self, Align2, FontId, Key, Rect, RichText, Sense, pos2, vec2};

use crate::i18n::tr;
use crate::icons;
use crate::toolstrip::Entry;

/// A command and the toolstrip tab it is on ("file": the File menu).
pub struct Command {
    pub tab: &'static str,
    pub entry: Entry,
}

/// What the user chose.
pub struct Chosen {
    pub tab: &'static str,
    pub key: String,
    pub menu: bool,
}

pub struct Palette {
    query: String,
    /// Filled by the app on the frame after opening.
    pub commands: Option<Vec<Command>>,
    hot: usize,
    first: bool,
}

const ROWS: usize = 12;
const ROW_H: f32 = 30.0;

thread_local! {
    /// Most recently run commands first (this session).
    static RECENT: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Palette {
    pub fn new() -> Palette {
        Palette { query: String::new(), commands: None, hot: 0, first: true }
    }

    /// Shows the search; `Some(None)` closes it, `Some(Some(c))` runs `c`.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Option<Chosen>> {
        let screen = ctx.content_rect();
        let width = (screen.width() - 32.0).min(560.0);
        let mut out = None;
        let (up, down, enter, esc) = ctx.input_mut(|i| {
            (i.consume_key(egui::Modifiers::NONE, Key::ArrowUp), i.consume_key(egui::Modifiers::NONE, Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, Key::Enter), i.consume_key(egui::Modifiers::NONE, Key::Escape))
        });
        if esc {
            return Some(None);
        }
        let results = self.results();
        if down && !results.is_empty() {
            self.hot = (self.hot + 1) % results.len();
        }
        if up && !results.is_empty() {
            self.hot = (self.hot + results.len() - 1) % results.len();
        }
        self.hot = self.hot.min(results.len().saturating_sub(1));
        let area = egui::Area::new(egui::Id::new("command_palette")).order(egui::Order::Foreground).fixed_pos(pos2(screen.center().x - width / 2.0, screen.min.y + 56.0));
        let resp = area.show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(8.0).show(ui, |ui| {
                ui.set_width(width);
                let edit = egui::TextEdit::singleline(&mut self.query).hint_text(tr("palette.hint")).desired_width(f32::INFINITY).font(FontId::proportional(15.0)).margin(vec2(8.0, 6.0));
                let r = ui.add(edit);
                if self.first {
                    r.request_focus();
                    self.first = false;
                }
                if r.changed() {
                    self.hot = 0;
                }
                ui.add_space(4.0);
                if self.commands.is_none() {
                    ui.label(RichText::new("…").weak());
                    return;
                }
                if results.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr("palette.none")).weak());
                    ui.add_space(6.0);
                    return;
                }
                if self.query.trim().is_empty() {
                    ui.label(RichText::new(tr("palette.suggested")).small().weak());
                }
                for (row, &i) in results.iter().enumerate() {
                    let c = &self.commands.as_ref().unwrap()[i];
                    // A command that can't run now shows as such (to screen readers too).
                    let (rect, r) = ui
                        .scope(|ui| {
                            if !c.entry.enabled {
                                ui.disable();
                            }
                            ui.allocate_exact_size(vec2(width, ROW_H), Sense::click())
                        })
                        .inner;
                    crate::widgets::name(&r, egui::WidgetType::SelectableLabel, &c.entry.label, Some(row == self.hot));
                    if r.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                        self.hot = row;
                    }
                    paint_row(ui, rect, c, row == self.hot);
                    if r.clicked() && c.entry.enabled {
                        out = Some(Some(chosen(c)));
                    }
                }
            });
        });
        if enter && let Some(&i) = results.get(self.hot) {
            let c = &self.commands.as_ref().unwrap()[i];
            if c.entry.enabled {
                out = Some(Some(chosen(c)));
            }
        }
        if resp.response.clicked_elsewhere() && out.is_none() {
            out = Some(None);
        }
        if let Some(Some(c)) = &out {
            RECENT.with_borrow_mut(|r| {
                r.retain(|k| *k != c.key);
                r.insert(0, c.key.clone());
                r.truncate(20);
            });
        }
        out
    }

    /// Indices of the best matches, best first.
    fn results(&self) -> Vec<usize> {
        let Some(cmds) = &self.commands else { return Vec::new() };
        let q = plain(self.query.trim());
        if q.is_empty() {
            let recent = RECENT.with_borrow(|r| r.clone());
            let mut v: Vec<usize> = recent.iter().filter_map(|k| cmds.iter().position(|c| c.entry.key == *k)).collect();
            for (i, c) in cmds.iter().enumerate() {
                if v.len() >= ROWS {
                    break;
                }
                if c.tab == "home" && c.entry.place.len() == 2 && !v.contains(&i) {
                    v.push(i);
                }
            }
            v.truncate(ROWS);
            return v;
        }
        let recent = RECENT.with_borrow(|r| r.clone());
        let mut scored: Vec<(i32, usize)> = cmds
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let mut s = score(&plain(&c.entry.label), &q)?;
                if let Some(p) = recent.iter().position(|k| *k == c.entry.key) {
                    s += 30 - p as i32;
                }
                if !c.entry.enabled {
                    s -= 40;
                }
                // Deeper in menus: a little less likely.
                s -= c.entry.place.len() as i32;
                Some((s, i))
            })
            .collect();
        // Words of the place ("chart", "table") find their commands too.
        if scored.len() < ROWS {
            for (i, c) in cmds.iter().enumerate() {
                if scored.iter().any(|(_, j)| *j == i) {
                    continue;
                }
                if c.entry.place.iter().any(|p| fold(p).contains(&q)) {
                    scored.push((-100, i));
                }
            }
        }
        scored.sort_by_key(|(s, i)| (std::cmp::Reverse(*s), *i));
        // One of each name and place.
        let mut seen = std::collections::HashSet::new();
        scored.into_iter().filter(|(_, i)| seen.insert((cmds[*i].entry.label.clone(), cmds[*i].entry.place.clone()))).map(|(_, i)| i).take(ROWS).collect()
    }
}

fn chosen(c: &Command) -> Chosen {
    Chosen { tab: c.tab, key: c.entry.key.clone(), menu: c.entry.menu }
}

fn paint_row(ui: &egui::Ui, rect: Rect, c: &Command, hot: bool) {
    let v = ui.visuals();
    if hot {
        ui.painter().rect_filled(rect, 4.0, v.selection.bg_fill.gamma_multiply(0.35));
    }
    let color = if c.entry.enabled { v.text_color() } else { v.weak_text_color() };
    if let Some(icon) = c.entry.icon {
        let r = Rect::from_center_size(pos2(rect.min.x + 18.0, rect.center().y), vec2(18.0, 18.0));
        // A mode or toggle that is on shows pressed, as on the toolstrip.
        if c.entry.on {
            ui.painter().rect_filled(r.expand(4.0), 4.0, v.selection.bg_fill.gamma_multiply(0.5));
        }
        icons::paint_accent(ui.painter(), r, icon, color, if c.entry.enabled { crate::theme::ACCENT } else { color });
    }
    let mut label = c.entry.label.clone();
    if c.entry.menu {
        label.push_str(" ›");
    }
    let g = ui.painter().layout_no_wrap(label, FontId::proportional(14.0), color);
    let lw = g.size().x;
    ui.painter().galley(pos2(rect.min.x + 36.0, rect.center().y - g.size().y / 2.0), g, color);
    let mut right = rect.max.x - 8.0;
    if !c.entry.keys.is_empty() {
        let k = ui.painter().text(pos2(right, rect.center().y), Align2::RIGHT_CENTER, &c.entry.keys, FontId::proportional(12.0), v.weak_text_color());
        right = k.min.x - 12.0;
    }
    let place = c.entry.place.join(" › ");
    let x = rect.min.x + 36.0 + lw + 14.0;
    if right > x + 20.0 {
        let p = ui.painter().with_clip_rect(Rect::from_min_max(pos2(x, rect.min.y), pos2(right, rect.max.y)));
        p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, place, FontId::proportional(12.0), v.weak_text_color().gamma_multiply(0.9));
    }
}

/// [`fold`] without "&" and "and" in the interface languages, so "queries
/// and connections" finds "Queries & Connections".
fn plain(s: &str) -> String {
    let f = fold(s);
    let words: Vec<&str> = f.split_whitespace().filter(|w| !["&", "and", "y", "e", "et", "und"].contains(w)).collect();
    if words.is_empty() { f } else { words.join(" ") }
}

/// Lowercase without accents.
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        })
        .filter(|c| *c != '…')
        .collect()
}

/// How well `name` matches `q` (both folded), or None.
pub(crate) fn score(name: &str, q: &str) -> Option<i32> {
    if name == q {
        return Some(1000);
    }
    if name.starts_with(q) {
        return Some(800 - name.len() as i32);
    }
    // Every word of the query starts a word of the name.
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let qw: Vec<&str> = q.split_whitespace().collect();
    if qw.iter().all(|w| words.iter().any(|n| n.starts_with(w))) {
        return Some(600 - name.len() as i32);
    }
    if let Some(p) = name.find(q) {
        return Some(400 - p as i32 - name.len() as i32);
    }
    // The letters in order (initials and such).
    let mut it = name.chars();
    let mut gaps = 0;
    for qc in q.chars().filter(|c| !c.is_whitespace()) {
        let mut found = false;
        for nc in it.by_ref() {
            if nc == qc {
                found = true;
                break;
            }
            gaps += 1;
        }
        if !found {
            return None;
        }
    }
    (q.chars().count() >= 3).then(|| 200 - gaps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching() {
        assert_eq!(fold("Gráfico…"), "grafico");
        assert!(score(&plain("Queries & Connections"), &plain("queries and connections")).is_some());
        assert!(score(&plain("Consultas y conexiones"), &plain("consultas y conex")).is_some());
        assert!(score("insertar grafico", "graf").unwrap() > score("insertar grafico", "igr").unwrap_or(0));
        assert!(score("negrita", "neg").is_some());
        assert!(score("negrita", "xyz").is_none());
        assert!(score("formato condicional", "for con").is_some());
    }
}
