//! The Object Browser (F2): the classes of VBA and of the object model and
//! the project's modules, their members with parameters and a line of
//! help; F1 opens it at the word under the cursor.

use apex_vba::catalog::{self, Kind};
use eframe::egui::{self, RichText};

use crate::i18n::tr;
use crate::widgets::FitWindow as _;

/// What the browser shows: a class of the catalog or a module of the
/// project, and a member of it.
#[derive(Clone, Default)]
pub(super) struct Browser {
    /// The class (catalog) or module (project) picked.
    pub class: String,
    pub member: Option<String>,
    pub search: String,
    /// Frames left to bring the member picked into view (the list is laid
    /// out first).
    pub scroll: u8,
}

/// A procedure to jump to: (module, row).
pub(super) type Jump = (usize, usize);

/// The class and member a word is (F1): a function or constant, a member
/// of some class, a class, or a procedure of the project.
/// `host_word`: a document's macros (Word's object model).
pub(super) fn lookup(word: &str, modules: &[(String, String)], host_word: bool) -> Option<Browser> {
    let at = |class: &str, member: Option<&str>| Some(Browser { class: class.to_string(), member: member.map(str::to_string), search: String::new(), scroll: 3 });
    if let Some((c, m)) = catalog::global_of(host_word, word) {
        return at(c.name, Some(m.name));
    }
    if let Some(c) = catalog::class_of(host_word, word) {
        return at(c.name, None);
    }
    for c in catalog::classes_of(host_word) {
        if let Some(m) = c.members.iter().find(|m| m.name.eq_ignore_ascii_case(word)) {
            return at(c.name, Some(m.name));
        }
    }
    for (name, code) in modules {
        if let Some((p, _)) = super::vba_edit::proc_rows(code).into_iter().find(|p| p.0.eq_ignore_ascii_case(word)) {
            return at(name, Some(&p));
        }
    }
    None
}

fn kind_icon(k: Kind) -> &'static str {
    match k {
        Kind::Method | Kind::Function => "◆",
        Kind::Property | Kind::ReadOnly => "▪",
        Kind::Event => "⚡",
        Kind::Constant => "≡",
        Kind::Keyword => "·",
    }
}

/// The window; `modules`: the project's (name, code). Returns a procedure
/// to show, picked twice (double click).
pub(super) fn show(ctx: &egui::Context, open: &mut Option<Browser>, modules: &[(String, String)], word: bool) -> Option<Jump> {
    let b = open.as_mut()?;
    let mut keep = true;
    let mut jump = None;
    egui::Window::new(tr("macros.browser")).id(egui::Id::new("macro_browser")).open(&mut keep).default_size([640.0, 440.0]).resizable(true).collapsible(false).show_fit(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut b.search).hint_text(tr("macros.find")).desired_width(240.0));
            if !b.search.is_empty() && ui.button("✕").clicked() {
                b.search.clear();
            }
        });
        ui.separator();
        let search = b.search.to_ascii_lowercase();
        // Fixed: a window grows to what it holds.
        let h = 300.0;
        if !search.is_empty() {
            // Search: every member and class that has it in its name.
            egui::ScrollArea::vertical().id_salt("browser_found").max_height(h).auto_shrink([false, true]).show(ui, |ui| {
                let mut shown = 0;
                for c in catalog::classes_of(word) {
                    for m in c.members.iter().filter(|m| m.name.to_ascii_lowercase().contains(&search)) {
                        if shown < 300 && ui.selectable_label(false, RichText::new(format!("{} {}.{}", kind_icon(m.kind), c.name, m.name)).monospace()).clicked() {
                            b.class = c.name.to_string();
                            b.member = Some(m.name.to_string());
                            b.search.clear();
                            b.scroll = 3;
                        }
                        shown += 1;
                    }
                }
                for (name, code) in modules {
                    for (p, _) in super::vba_edit::proc_rows(code).into_iter().filter(|p| p.0.to_ascii_lowercase().contains(&search)) {
                        if ui.selectable_label(false, RichText::new(format!("◆ {name}.{p}")).monospace()).clicked() {
                            b.class = name.clone();
                            b.member = Some(p);
                            b.search.clear();
                            b.scroll = 3;
                        }
                    }
                }
            });
        } else {
            // Its own height: the separator between the lists would take
            // all there is, and the window grow with it.
            let width = ui.available_width();
            ui.allocate_ui(egui::vec2(width, h), |ui| {
                ui.set_max_height(h);
                ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(200.0);
                    ui.label(RichText::new(tr("macros.classes")).strong());
                    egui::ScrollArea::vertical().id_salt("browser_classes").max_height(h - 24.0).auto_shrink([false, false]).show(ui, |ui| {
                        for c in catalog::classes_of(word) {
                            let r = ui.selectable_label(b.class == c.name, c.name).on_hover_text(c.help);
                            if r.clicked() {
                                b.class = c.name.to_string();
                                b.member = None;
                            }
                        }
                        ui.separator();
                        for (name, _) in modules {
                            if ui.selectable_label(&b.class == name, RichText::new(name).italics()).clicked() {
                                b.class = name.clone();
                                b.member = None;
                            }
                        }
                    });
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.label(RichText::new(tr("macros.members")).strong());
                    egui::ScrollArea::vertical().id_salt("browser_members").max_height(h - 24.0).auto_shrink([false, false]).show(ui, |ui| {
                        if let Some(c) = catalog::class_of(word, &b.class).filter(|c| c.name == b.class) {
                            let mut members: Vec<_> = c.members.iter().collect();
                            members.sort_by_key(|m| m.name.to_ascii_lowercase());
                            for m in members {
                                let on = b.member.as_deref() == Some(m.name);
                                let r = ui.selectable_label(on, RichText::new(format!("{} {}", kind_icon(m.kind), m.name)).monospace());
                                if on && b.scroll > 0 {
                                    r.scroll_to_me(Some(egui::Align::Center));
                                }
                                if r.clicked() {
                                    b.member = Some(m.name.to_string());
                                }
                            }
                        } else if let Some((i, (_, code))) = modules.iter().enumerate().find(|m| m.1.0 == b.class) {
                            for (p, row) in super::vba_edit::proc_rows(code) {
                                let on = b.member.as_deref() == Some(p.as_str());
                                let r = ui.selectable_label(on, RichText::new(format!("◆ {p}")).monospace());
                                if on && b.scroll > 0 {
                                    r.scroll_to_me(Some(egui::Align::Center));
                                }
                                if r.double_clicked() {
                                    jump = Some((i, row));
                                } else if r.clicked() {
                                    b.member = Some(p);
                                }
                            }
                        }
                    });
                });
                });
            });
            b.scroll = b.scroll.saturating_sub(1);
        }
        ui.separator();
        // What is picked: its declaration and help.
        if let Some(c) = catalog::class_of(word, &b.class).filter(|c| c.name == b.class) {
            match b.member.as_deref().and_then(|m| catalog::member_of(word, c.name, m)) {
                Some(m) => {
                    let ty = match m.kind {
                        Kind::Constant => format!(" = {}", m.ty),
                        _ if m.ty.is_empty() => String::new(),
                        _ => format!(": {}", m.ty),
                    };
                    ui.label(RichText::new(format!("{}{}{ty}", m.name, m.params)).monospace().strong());
                    ui.label(format!("{} {}", tr("macros.member_of"), c.name));
                    ui.label(m.help);
                }
                None => {
                    ui.label(RichText::new(c.name).monospace().strong());
                    ui.label(c.help);
                }
            }
        } else if let Some((_, code)) = modules.iter().find(|m| m.0 == b.class) {
            let proc = b.member.as_ref().and_then(|p| super::vba_edit::procedures(code).into_iter().find(|q| &q.0 == p));
            match proc {
                Some((name, params)) => {
                    ui.label(RichText::new(format!("{name}{params}")).monospace().strong());
                    ui.label(format!("{} {}", tr("macros.member_of"), b.class));
                }
                None => {
                    ui.label(RichText::new(&b.class).monospace().strong());
                }
            }
        }
    });
    if !keep {
        *open = None;
    }
    jump
}

#[cfg(test)]
mod tests {
    use super::lookup;

    #[test]
    fn help_on_a_word() {
        let modules = [("Module1".to_string(), "Sub Twice()\nEnd Sub".to_string())];
        let at = |w: &str| lookup(w, &modules, false).map(|b| (b.class, b.member));
        assert_eq!(at("msgbox"), Some(("VBA".into(), Some("MsgBox".into()))));
        assert_eq!(at("xlUp"), Some(("Constants".into(), Some("xlUp".into()))));
        assert_eq!(at("range"), Some(("Global".into(), Some("Range".into()))));
        assert_eq!(at("Offset"), Some(("Range".into(), Some("Offset".into()))));
        assert_eq!(at("Worksheet"), Some(("Worksheet".into(), None)));
        assert_eq!(at("twice"), Some(("Module1".into(), Some("Twice".into()))));
        assert_eq!(at("nothing_like_it"), None);
    }
}
