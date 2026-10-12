//! Word processor toolbar, menus and context menu.

use super::*;
use apex_doc::EditKind;

/// Bullet and numbering styles offered in the list menus (see
/// `apex_doc::BULLETS` and `apex_doc::NUM_FORMATS`); 0 is the default style,
/// which changes with the list level.
const LIST_FORMATS: [u8; 6] = [0, 2, 3, 4, 5, 6];
const LINE_SPACINGS: [f32; 6] = [1.0, 1.15, 1.5, 2.0, 2.5, 3.0];
const CASES: [(Case, &str); 5] = [
    (Case::Sentence, "case.sentence"),
    (Case::Lower, "case.lower"),
    (Case::Upper, "case.upper"),
    (Case::Title, "case.title"),
    (Case::Toggle, "case.toggle"),
];

pub(super) fn undo_label(k: EditKind) -> &'static str {
    match k {
        EditKind::Typing => "undo.typing",
        EditKind::Insert => "undo.insert",
        EditKind::Delete => "undo.delete",
        EditKind::Format => "undo.format",
        EditKind::Paragraph => "undo.paragraph",
        EditKind::Replace => "undo.replace",
        EditKind::Link => "undo.link",
        EditKind::Bookmark => "undo.bookmark",
        EditKind::Image => "undo.image",
        EditKind::Shape => "undo.shape",
        EditKind::Table => "undo.table",
        EditKind::Note => "undo.fnote",
        EditKind::Field => "undo.field",
        EditKind::Toc => "undo.toc",
        EditKind::Outline => "undo.outline",
        EditKind::Comment => "undo.comment",
        EditKind::Revision => "undo.revision",
    }
}

fn spacing_label(v: f32) -> String {
    let s = format!("{v}");
    // Metric East Asian locales keep the decimal point.
    if apex_sheet::format::Locale::from_tag(crate::i18n::current()).decimal == ',' { s.replace('.', ",") } else { s }
}

impl DocView {
    /// Bullet styles drawn as swatches.
    pub(super) fn bullet_menu(&mut self, ui: &mut egui::Ui) -> Option<(ParaKind, u8)> {
        let mut out = None;
        let cur = self.cur_para().style;
        ui.horizontal(|ui| {
            for f in LIST_FORMATS {
                let (rect, resp) = ui.allocate_exact_size(vec2(34.0, 30.0), Sense::click());
                let on = cur.kind == ParaKind::Bullet && cur.list_format == f;
                if on || resp.hovered() {
                    ui.painter().rect_filled(rect, 4.0, if on { ui.visuals().selection.bg_fill.gamma_multiply(0.4) } else { ui.visuals().widgets.hovered.weak_bg_fill });
                }
                paint_bullet(ui.painter(), rect.center(), 3.5, apex_doc::bullet_char(f, 0), ui.visuals().text_color(), 1.0);
                if resp.clicked() {
                    out = Some((ParaKind::Bullet, f));
                    ui.close();
                }
            }
        });
        out
    }

    pub(super) fn numbering_menu(&mut self, ui: &mut egui::Ui) -> Option<(ParaKind, u8)> {
        let mut out = None;
        let cur = self.cur_para().style;
        ui.set_min_width(180.0);
        for f in LIST_FORMATS {
            let label = (1..=3).map(|n| apex_doc::number_label(f, 0, n)).collect::<Vec<_>>().join("   ");
            if ui.add(egui::Button::selectable(cur.kind == ParaKind::Numbered && cur.list_format == f, label)).clicked() {
                out = Some((ParaKind::Numbered, f));
                ui.close();
            }
        }
        ui.separator();
        let numbered = cur.kind == ParaKind::Numbered;
        if ui.add_enabled(numbered, egui::Button::new(tr("doc.restart_numbering"))).clicked() {
            self.para_style(|s| s.restart = true);
            ui.close();
        }
        if ui.add_enabled(numbered, egui::Button::new(tr("doc.continue_numbering"))).clicked() {
            self.para_style(|s| s.restart = false);
            ui.close();
        }
        out
    }

    pub(super) fn spacing_menu(&mut self, ui: &mut egui::Ui) {
        let st = self.cur_para().style;
        let cur = if st.line_rule == LineRule::Multiple { if st.line_spacing > 0.0 { st.line_spacing } else { 1.0 } } else { -1.0 };
        for v in LINE_SPACINGS {
            if ui.radio((cur - v).abs() < 0.01, spacing_label(v)).clicked() {
                self.set_line_spacing(v);
                ui.close();
            }
        }
        ui.separator();
        let (before, after) = st.spacing();
        let key = if before > 0.0 { "doc.space_before_remove" } else { "doc.space_before_add" };
        if ui.button(tr(key)).clicked() {
            self.para_style(move |s| s.space_before = Some(if before > 0.0 { 0.0 } else { 12.0 }));
            ui.close();
        }
        let key = if after > 0.0 { "doc.space_after_remove" } else { "doc.space_after_add" };
        if ui.button(tr(key)).clicked() {
            self.para_style(move |s| s.space_after = Some(if after > 0.0 { 0.0 } else { 12.0 }));
            ui.close();
        }
        ui.separator();
        if ui.button(format!("{}…", tr("doc.paragraph"))).clicked() {
            self.open_paragraph_dialog();
            ui.close();
        }
    }

    // --- menus ----------------------------------------------------------------------------

    fn clipboard_items(&mut self, ui: &mut egui::Ui) {
        let has = !self.sel.is_empty();
        if ui.add_enabled(has, egui::Button::new(tr("edit.cut")).shortcut_text("Ctrl+X")).clicked() {
            self.copy(ui.ctx(), true);
            ui.close();
        }
        if ui.add_enabled(has, egui::Button::new(tr("edit.copy")).shortcut_text("Ctrl+C")).clicked() {
            self.copy(ui.ctx(), false);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("edit.paste")).shortcut_text("Ctrl+V")).clicked() {
            self.request_paste(ui.ctx(), false);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("doc.paste_plain")).shortcut_text("Ctrl+Shift+V")).clicked() {
            self.request_paste(ui.ctx(), true);
            ui.close();
        }
        if !self.embedded && ui.add_enabled(has, egui::Button::new(tr("nb.send_fragment"))).on_hover_text(tr("nb.send_fragment_tip")).clicked() {
            self.fragment_wanted = true;
            ui.close();
        }
    }

    pub(super) fn context_menu(&mut self, ui: &mut egui::Ui) {
        if self.shape_menu(ui) {
            return;
        }
        self.spell_context_items(ui);
        self.thesaurus_context_items(ui);
        self.clipboard_items(ui);
        ui.separator();
        self.revision_context_items(ui);
        self.comment_context_items(ui);
        ui.separator();
        self.picture_menu(ui);
        self.equation_menu_item(ui);
        self.task_menu(ui);
        if let Some(hit) = self.menu_link.clone() {
            if let Some((page, now)) = apex_doc::notebook::links::parse(&hit.1).filter(|_| self.embedded) {
                crate::widgets::menu_button(ui, tr("nb.relation"), |ui| {
                    for rel in apex_doc::notebook::Rel::ALL {
                        if ui.radio(rel == now, crate::note_view::rel_name(rel)).clicked() {
                            self.doc.set_link(hit.0, Some(apex_doc::notebook::links::target(page, rel)), None);
                            ui.close();
                        }
                    }
                });
            }
            if ui.button(format!("{}…", tr("link.edit_title"))).clicked() {
                self.open_link_dialog(Some(hit.clone()));
                ui.close();
            }
            if ui.button(tr("link.open")).clicked() {
                self.follow_link(&hit.1);
                ui.close();
            }
            if ui.button(tr("link.copy")).clicked() {
                ui.ctx().copy_text(hit.1.to_string());
                ui.close();
            }
            if ui.button(tr("link.remove_menu")).clicked() {
                self.remove_link(Some(hit));
                ui.close();
            }
        } else if ui.add(egui::Button::new(format!("{}…", tr("doc.link"))).shortcut_text("Ctrl+K")).clicked() {
            self.open_link_dialog(None);
            ui.close();
        }
        ui.separator();
        self.refs_context_items(ui);
        self.table_context_items(ui);
        if ui.button(tr("edit.select_all")).clicked() {
            self.sel = Selection { anchor: Pos::new(0, 0), cursor: self.doc.end() };
            ui.close();
        }
        ui.separator();
        crate::widgets::menu_button(ui, tr("format.change_case"), |ui| {
            if let Some(c) = case_menu(ui) {
                self.change_case(c);
            }
        });
        if ui.add(egui::Button::new(format!("{}…", tr("font.title"))).shortcut_text("Ctrl+D")).clicked() {
            self.open_font_dialog();
            ui.close();
        }
        if ui.button(format!("{}…", tr("doc.paragraph"))).clicked() {
            self.open_paragraph_dialog();
            ui.close();
        }
        crate::widgets::menu_button(ui, tr("doc.bullets"), |ui| {
            if let Some((k, f)) = self.bullet_menu(ui) {
                self.set_list(k, f);
            }
        });
        crate::widgets::menu_button(ui, tr("doc.numbering"), |ui| {
            if let Some((k, f)) = self.numbering_menu(ui) {
                self.set_list(k, f);
            }
        });
        ui.separator();
        if ui.add(egui::Button::new(tr("format.clear")).shortcut_text("Ctrl+Space")).clicked() {
            self.clear_formatting(true);
            ui.close();
        }
    }
}

pub(super) fn case_menu(ui: &mut egui::Ui) -> Option<Case> {
    let mut out = None;
    for (c, key) in CASES {
        if ui.button(tr(key)).clicked() {
            out = Some(c);
            ui.close();
        }
    }
    out
}
