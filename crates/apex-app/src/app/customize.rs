//! Customize: the Quick Access Toolbar and keyboard shortcuts, for any
//! command of the toolstrip or the File menu (found like the command search
//! finds them, run like it runs them).

use eframe::egui::{self, Event, Key, KeyboardShortcut, Modifiers, RichText, vec2};

use super::App;
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};
use crate::palette::{self, Chosen};
use crate::settings::Command;
use crate::widgets::{self, DialogResult};

pub(super) struct CustomizeDlg {
    page: usize,
    /// Customizing documents (else spreadsheets).
    doc: bool,
    query: String,
    /// The command picked in the list (its key).
    picked: Option<String>,
    qat: Vec<Command>,
    qat_sel: Option<usize>,
    binds: Vec<(Command, String)>,
    /// Waiting for the keys of a new shortcut.
    pub capturing: bool,
    new_keys: Option<String>,
    note: String,
}

/// A shortcut from its text ("Ctrl+Shift+K").
pub(crate) fn parse_keys(s: &str) -> Option<KeyboardShortcut> {
    let mut m = Modifiers::NONE;
    let mut key = None;
    for p in s.split('+').map(str::trim) {
        match p.to_ascii_lowercase().as_str() {
            "ctrl" | "cmd" => m = m.plus(Modifiers::COMMAND),
            "shift" => m = m.plus(Modifiers::SHIFT),
            "alt" => m = m.plus(Modifiers::ALT),
            _ => key = Key::from_name(p),
        }
    }
    Some(KeyboardShortcut::new(m, key?))
}

pub(crate) fn keys_text(m: Modifiers, key: Key) -> String {
    let mut s = String::new();
    if m.command || m.ctrl {
        s.push_str("Ctrl+");
    }
    if m.shift {
        s.push_str("Shift+");
    }
    if m.alt {
        s.push_str("Alt+");
    }
    s.push_str(key.name());
    s
}

/// Keys that make a shortcut alone (the rest needs Ctrl or Alt).
fn function_key(k: Key) -> bool {
    let n = k.name();
    n.len() > 1 && n.starts_with('F') && n[1..].chars().all(|c| c.is_ascii_digit())
}

pub(crate) fn icon_named(name: &str) -> Option<Icon> {
    icons::ALL.iter().copied().find(|i| format!("{i:?}") == name)
}

fn setting(c: &palette::Command, doc: bool) -> Command {
    Command { doc, tab: c.tab.to_string(), key: c.entry.key.clone(), label: c.entry.label.clone(), icon: c.entry.icon.map(|i| format!("{i:?}")).unwrap_or_default(), menu: c.entry.menu }
}

/// Where a command is, for lists.
fn place(c: &palette::Command) -> String {
    c.entry.place.join(" › ")
}

impl CustomizeDlg {
    /// Keys pressed while capturing.
    pub(super) fn captured(&mut self, key: Key, m: Modifiers) {
        self.capturing = false;
        if key == Key::Escape {
            return;
        }
        if !(m.command || m.ctrl || m.alt || function_key(key)) {
            self.new_keys = None;
            self.note = tr("customize.needs_modifier").to_string();
            return;
        }
        self.new_keys = Some(keys_text(m, key));
        self.note.clear();
    }
}

impl App {
    pub(super) fn open_customize(&mut self, page: usize) {
        let Some(doc) = self.view_is_doc() else { return };
        self.want_commands = true;
        self.customize = Some(CustomizeDlg {
            page,
            doc,
            query: String::new(),
            picked: None,
            qat: self.settings.qat.iter().filter(|c| c.doc == doc).cloned().collect(),
            qat_sel: None,
            binds: self.settings.keybinds.iter().filter(|(c, _)| c.doc == doc).cloned().collect(),
            capturing: false,
            new_keys: None,
            note: String::new(),
        });
    }

    /// Whether the active tab is a document (None without tabs).
    pub(super) fn view_is_doc(&self) -> Option<bool> {
        self.tabs.get(self.active).map(|t| matches!(t.view, super::View::Doc(_)))
    }

    /// Runs a command chosen in the search, the toolbar or by its keys: a
    /// menu opens on the toolstrip, the rest runs on the next frame.
    pub(super) fn choose(&mut self, ctx: &egui::Context, c: Chosen) {
        if c.menu && c.tab == super::toolstrip_bar::FILE {
            // The File menu's submenus can't open from the search, and
            // running one off screen does nothing: their commands are in the
            // search themselves, and Language's choices are on the Options
            // dialog's first page.
            self.open_options();
        } else if c.menu {
            let kind = self.toolstrip_kind();
            self.toolstrip_tab[kind] = self.toolstrip_tabs().into_iter().find(|t| t.id == c.tab).map_or("home", |t| t.id);
            if self.settings.classic {
                self.toolstrip_peek = 1;
            } else {
                self.settings.show_toolbar = true;
            }
            crate::toolstrip::open_menu(c.key);
        } else {
            self.pending_cmd = Some(c);
        }
        ctx.request_repaint();
    }

    /// A saved command, if its tab is there now.
    fn chosen(&self, c: &Command) -> Option<Chosen> {
        let tab = if c.tab == super::toolstrip_bar::FILE { super::toolstrip_bar::FILE } else { self.toolstrip_tabs().into_iter().find(|t| t.id == c.tab)?.id };
        Some(Chosen { tab, key: c.key.clone(), menu: c.menu })
    }

    /// The user's shortcuts, and the keys of a shortcut being set (true
    /// then: the keys do nothing else).
    pub(super) fn custom_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        if let Some(d) = &mut self.customize
            && d.capturing
        {
            let got = ctx.input_mut(|i| {
                let mut got = None;
                i.events.retain(|e| match e {
                    Event::Key { key, pressed: true, modifiers, .. } if got.is_none() => {
                        got = Some((*key, *modifiers));
                        false
                    }
                    Event::Key { .. } | Event::Text(_) | Event::Copy | Event::Cut | Event::Paste(_) => false,
                    _ => true,
                });
                got
            });
            if let Some((k, m)) = got {
                d.captured(k, m);
            }
            return true;
        }
        let Some(doc) = self.view_is_doc().filter(|_| self.customize.is_none()) else { return false };
        let mut hit = None;
        for (c, keys) in self.settings.keybinds.iter().filter(|(c, _)| c.doc == doc) {
            let Some(sc) = parse_keys(keys) else { continue };
            if ctx.input_mut(|i| i.modifiers.matches_exact(sc.modifiers) && i.consume_shortcut(&sc)) {
                hit = Some(c.clone());
                break;
            }
        }
        if let Some(c) = hit.and_then(|c| self.chosen(&c)) {
            self.choose(ctx, c);
        }
        false
    }

    /// The Quick Access Toolbar, right to left, and its Customize button.
    pub(super) fn qat_bar(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = self.view_is_doc() else { return };
        let r = ui.add(egui::Button::new(RichText::new("⏷").small()).frame(false)).on_hover_text(tr("customize.qat_hint"));
        if r.clicked() {
            self.open_customize(0);
        }
        let items: Vec<Command> = self.settings.qat.iter().filter(|c| c.doc == doc).cloned().collect();
        for c in items.iter().rev() {
            let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 24.0), egui::Sense::click());
            crate::widgets::name(&resp, egui::WidgetType::Button, &c.label, None);
            let v = ui.visuals();
            if resp.hovered() {
                ui.painter().rect_filled(rect, 4.0, v.widgets.hovered.weak_bg_fill);
            }
            let ir = egui::Rect::from_center_size(rect.center(), vec2(16.0, 16.0));
            match icon_named(&c.icon) {
                Some(icon) => icons::paint_accent(ui.painter(), ir, icon, v.text_color(), crate::theme::ACCENT),
                None => {
                    let letter: String = c.label.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, letter, egui::FontId::proportional(13.0), v.text_color());
                }
            }
            let tip = match self.settings.keybinds.iter().find(|(b, _)| b.key == c.key && b.doc == doc) {
                Some((_, k)) => format!("{} ({k})", c.label),
                None => c.label.clone(),
            };
            if resp.on_hover_text(tip).clicked()
                && let Some(ch) = self.chosen(c)
            {
                self.choose(ui.ctx(), ch);
            }
        }
    }

    /// Width the toolbar takes.
    pub(super) fn qat_width(&self) -> f32 {
        let Some(doc) = self.view_is_doc() else { return 0.0 };
        20.0 + 26.0 * self.settings.qat.iter().filter(|c| c.doc == doc).count() as f32
    }

    pub(super) fn customize_dialog(&mut self, ctx: &egui::Context) {
        if self.customize.is_none() {
            return;
        }
        let cmds = self.commands.take();
        let Some(d) = &mut self.customize else { return };
        let pages = [(Icon::Options, "customize.qat"), (Icon::Keyboard, "customize.keys")];
        let title = format!("{} — {}", tr("customize.title"), tr(if d.doc { "customize.for_docs" } else { "customize.for_sheets" }));
        let mut page = d.page;
        let res = widgets::nav_dialog(ctx, "customize", &title, &pages, &mut page, vec2(760.0, 460.0), |ui, p| {
            let Some(cmds) = &cmds else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("customize.loading"));
                });
                return;
            };
            if p == 0 { qat_page(ui, d, cmds) } else { keys_page(ui, d, cmds) }
        });
        d.page = page;
        self.commands = cmds;
        match res {
            DialogResult::None => {}
            DialogResult::Cancel => self.customize = None,
            DialogResult::Ok => {
                if let Some(d) = self.customize.take() {
                    let st = &mut self.settings;
                    st.qat.retain(|c| c.doc != d.doc);
                    st.qat.extend(d.qat);
                    st.keybinds.retain(|(c, _)| c.doc != d.doc);
                    st.keybinds.extend(d.binds);
                    st.save();
                }
            }
        }
    }
}

/// The searchable list of commands; returns the picked one.
fn command_list<'a>(ui: &mut egui::Ui, d: &mut CustomizeDlg, cmds: &'a [palette::Command], width: f32) -> Option<&'a palette::Command> {
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.add(egui::TextEdit::singleline(&mut d.query).hint_text(tr("customize.search")).desired_width(width));
        let q = palette::fold(d.query.trim());
        let shown: Vec<&palette::Command> = cmds.iter().filter(|c| q.is_empty() || palette::fold(&c.entry.label).contains(&q) || palette::fold(&place(c)).contains(&q)).collect();
        egui::ScrollArea::vertical().id_salt("cmds").max_height(ui.available_height() - 4.0).auto_shrink([false, false]).show_rows(ui, 34.0, shown.len(), |ui, rows| {
            for c in &shown[rows] {
                let on = d.picked.as_deref() == Some(c.entry.key.as_str());
                let (rect, r) = ui.allocate_exact_size(vec2(width - 14.0, 34.0), egui::Sense::click());
                crate::widgets::name(&r, egui::WidgetType::SelectableLabel, &c.entry.label, Some(on));
                let v = ui.visuals();
                if on {
                    ui.painter().rect_filled(rect, 3.0, v.selection.bg_fill.gamma_multiply(0.45));
                } else if r.hovered() {
                    ui.painter().rect_filled(rect, 3.0, v.widgets.hovered.weak_bg_fill);
                }
                if let Some(icon) = c.entry.icon {
                    let ir = egui::Rect::from_center_size(egui::pos2(rect.min.x + 13.0, rect.center().y), vec2(16.0, 16.0));
                    icons::paint_accent(ui.painter(), ir, icon, v.text_color(), crate::theme::ACCENT);
                }
                let mut label = c.entry.label.clone();
                if c.entry.menu {
                    label.push_str(" ›");
                }
                let p = ui.painter().with_clip_rect(rect);
                p.text(egui::pos2(rect.min.x + 28.0, rect.min.y + 10.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(13.0), v.text_color());
                p.text(egui::pos2(rect.min.x + 28.0, rect.min.y + 25.0), egui::Align2::LEFT_CENTER, place(c), egui::FontId::proportional(10.5), v.weak_text_color());
                if r.clicked() {
                    d.picked = Some(c.entry.key.clone());
                    d.new_keys = None;
                    d.note.clear();
                }
            }
        });
    });
    d.picked.as_ref().and_then(|k| cmds.iter().find(|c| c.entry.key == *k))
}

fn qat_page(ui: &mut egui::Ui, d: &mut CustomizeDlg, cmds: &[palette::Command]) {
    ui.horizontal_top(|ui| {
        let h = ui.available_height();
        let picked = command_list(ui, d, cmds, 330.0);
        ui.vertical(|ui| {
            ui.add_space(h / 2.0 - 40.0);
            let can_add = picked.is_some_and(|c| !d.qat.iter().any(|q| q.key == c.entry.key));
            if ui.add_enabled(can_add, egui::Button::new(format!("{} »", tr("customize.add")))).clicked()
                && let Some(c) = picked
            {
                d.qat.push(setting(c, d.doc));
                d.qat_sel = Some(d.qat.len() - 1);
            }
            if ui.add_enabled(d.qat_sel.is_some(), egui::Button::new(format!("« {}", tr("customize.remove")))).clicked()
                && let Some(i) = d.qat_sel.take()
                && i < d.qat.len()
            {
                d.qat.remove(i);
            }
        });
        ui.vertical(|ui| {
            ui.label(RichText::new(tr("customize.qat_items")).strong());
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_size(vec2(220.0, h - 90.0));
                if d.qat.is_empty() {
                    ui.weak(tr("customize.qat_empty"));
                }
                for (i, c) in d.qat.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
                        if let Some(icon) = icon_named(&c.icon) {
                            icons::paint_accent(ui.painter(), r, icon, ui.visuals().text_color(), crate::theme::ACCENT);
                        }
                        if ui.selectable_label(d.qat_sel == Some(i), &c.label).clicked() {
                            d.qat_sel = Some(i);
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                let sel = d.qat_sel.filter(|&i| i < d.qat.len());
                if ui.add_enabled(sel.is_some_and(|i| i > 0), egui::Button::new("⏶")).on_hover_text(tr("customize.up")).clicked()
                    && let Some(i) = sel
                {
                    d.qat.swap(i, i - 1);
                    d.qat_sel = Some(i - 1);
                }
                if ui.add_enabled(sel.is_some_and(|i| i + 1 < d.qat.len()), egui::Button::new("⏷")).on_hover_text(tr("customize.down")).clicked()
                    && let Some(i) = sel
                {
                    d.qat.swap(i, i + 1);
                    d.qat_sel = Some(i + 1);
                }
                if ui.add_enabled(!d.qat.is_empty(), egui::Button::new(tr("customize.reset"))).clicked() {
                    d.qat.clear();
                    d.qat_sel = None;
                }
            });
        });
    });
}

fn keys_page(ui: &mut egui::Ui, d: &mut CustomizeDlg, cmds: &[palette::Command]) {
    ui.horizontal_top(|ui| {
        let picked = command_list(ui, d, cmds, 330.0);
        ui.vertical(|ui| {
            let Some(c) = picked else {
                ui.weak(tr("customize.pick_command"));
                custom_list(ui, d);
                return;
            };
            ui.label(RichText::new(&c.entry.label).strong());
            ui.small(place(c));
            ui.add_space(6.0);
            let default = if c.entry.keys.is_empty() { tr("customize.none").to_string() } else { c.entry.keys.clone() };
            ui.label(trf("customize.default_keys", &[("keys", &default)]));
            let mine: Vec<String> = d.binds.iter().filter(|(b, _)| b.key == c.entry.key).map(|(_, k)| k.clone()).collect();
            if !mine.is_empty() {
                ui.label(trf("customize.own_keys", &[("keys", &mine.join(", "))]));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let text = if d.capturing {
                    tr("customize.press_keys").to_string()
                } else {
                    d.new_keys.clone().unwrap_or_else(|| tr("customize.click_to_set").to_string())
                };
                if ui.add(egui::Button::new(text).min_size(vec2(200.0, 26.0)).selected(d.capturing)).clicked() {
                    d.capturing = !d.capturing;
                    d.note.clear();
                }
                if ui.add_enabled(d.new_keys.is_some() && !d.capturing, egui::Button::new(tr("customize.assign"))).clicked()
                    && let Some(k) = d.new_keys.take()
                {
                    // A shortcut does one thing.
                    d.binds.retain(|(_, x)| *x != k);
                    d.binds.push((setting(c, d.doc), k));
                }
            });
            if let Some(k) = &d.new_keys {
                let mine = d.binds.iter().find(|(b, x)| x == k && b.key != c.entry.key).map(|(b, _)| b.label.clone());
                let builtin = cmds.iter().find(|x| x.entry.keys.eq_ignore_ascii_case(k) && x.entry.key != c.entry.key).map(|x| x.entry.label.clone());
                if let Some(other) = mine.or(builtin) {
                    ui.colored_label(ui.visuals().warn_fg_color, trf("customize.taken", &[("cmd", &other)]));
                }
            }
            if !d.note.is_empty() {
                ui.colored_label(ui.visuals().warn_fg_color, &d.note);
            }
            ui.add_space(8.0);
            custom_list(ui, d);
        });
    });
}

/// The user's shortcuts, to remove them.
fn custom_list(ui: &mut egui::Ui, d: &mut CustomizeDlg) {
    ui.separator();
    ui.label(RichText::new(tr("customize.own_list")).strong());
    if d.binds.is_empty() {
        ui.weak(tr("customize.none"));
    }
    let mut remove = None;
    egui::ScrollArea::vertical().id_salt("binds").max_height(160.0).show(ui, |ui| {
        for (i, (c, k)) in d.binds.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button("×").on_hover_text(tr("customize.remove")).clicked() {
                    remove = Some(i);
                }
                ui.label(RichText::new(k).monospace());
                ui.label(&c.label);
            });
        }
    });
    if let Some(i) = remove {
        d.binds.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_text() {
        let k = parse_keys("Ctrl+Shift+K").unwrap();
        assert_eq!(k.logical_key, Key::K);
        assert!(k.modifiers.command && k.modifiers.shift && !k.modifiers.alt);
        assert_eq!(keys_text(k.modifiers, k.logical_key), "Ctrl+Shift+K");
        assert_eq!(parse_keys("F5").map(|k| k.logical_key), Some(Key::F5));
        assert!(parse_keys("Ctrl+").is_none());
        assert!(function_key(Key::F12) && !function_key(Key::A));
    }
}
