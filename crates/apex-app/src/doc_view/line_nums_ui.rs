//! Line numbers and hyphenation: the Layout tab menus and the Line Numbers
//! dialog.

use apex_doc::{LineNumbering, LineRestart, Scope};

use super::*;
use crate::widgets::DialogResult;

pub(super) struct LineNumDlg {
    on: bool,
    cfg: LineNumbering,
    /// Distance in points; 0 = automatic.
    dist: f32,
}

impl DocView {
    fn caret_line_numbers(&self) -> Option<LineNumbering> {
        self.doc.section_setup(self.sel.cursor.para).0.line_numbers
    }

    fn set_line_numbers(&mut self, v: Option<LineNumbering>) {
        let i = self.doc.section_index(self.sel.cursor.para);
        self.doc.change_sections(Scope::Section(i), self.sel, &|p, _| p.line_numbers = v);
    }

    /// Items of the Line Numbers menu.
    pub(super) fn line_number_items(&mut self, ui: &mut egui::Ui) {
        let cur = self.caret_line_numbers();
        let base = cur.unwrap_or_default();
        let items: [(Option<LineRestart>, &str); 4] = [(None, "linenum.none"), (Some(LineRestart::Continuous), "linenum.continuous"), (Some(LineRestart::EachPage), "linenum.each_page"), (Some(LineRestart::EachSection), "linenum.each_section")];
        for (r, key) in items {
            if ui.radio(cur.map(|c| c.restart) == r, tr(key)).clicked() {
                self.set_line_numbers(r.map(|restart| LineNumbering { restart, ..base }));
                ui.close();
            }
        }
        ui.separator();
        let (a, b) = (self.sel.start().para, self.sel.end().para);
        let off = self.doc.paragraphs[a..=b].iter().all(|p| p.style.no_line_numbers);
        if ui.checkbox(&mut off.clone(), tr("linenum.suppress")).clicked() {
            self.doc.set_para_style(a, b, self.sel, &|st| st.no_line_numbers = !off);
            ui.close();
        }
        ui.separator();
        if ui.button(format!("{}…", tr("linenum.options"))).clicked() {
            let cfg = cur.unwrap_or_default();
            self.line_num_dlg = Some(LineNumDlg { on: cur.is_some(), cfg, dist: cfg.distance.unwrap_or(0.0) });
            ui.close();
        }
    }

    pub(super) fn line_num_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.line_num_dlg.take() else { return };
        let res = widgets::dialog(ctx, "doc_line_numbers", tr("linenum.title"), 360.0, |ui| {
            ui.checkbox(&mut d.on, tr("linenum.add"));
            ui.add_enabled_ui(d.on, |ui| {
                egui::Grid::new("linenum_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label(tr("linenum.start"));
                    ui.add(egui::DragValue::new(&mut d.cfg.start).range(1..=32767));
                    ui.end_row();
                    ui.label(tr("linenum.distance"));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut d.dist).range(0.0..=144.0).speed(0.5).suffix(" pt").custom_formatter(|v, _| if v <= 0.0 { tr("linenum.auto").to_string() } else { format!("{v:.1} pt") }));
                    });
                    ui.end_row();
                    ui.label(tr("linenum.count_by"));
                    ui.add(egui::DragValue::new(&mut d.cfg.count_by).range(1..=100));
                    ui.end_row();
                });
                ui.add_space(4.0);
                ui.label(tr("linenum.numbering"));
                ui.radio_value(&mut d.cfg.restart, LineRestart::EachPage, tr("linenum.each_page"));
                ui.radio_value(&mut d.cfg.restart, LineRestart::EachSection, tr("linenum.each_section"));
                ui.radio_value(&mut d.cfg.restart, LineRestart::Continuous, tr("linenum.continuous"));
            });
        });
        match res {
            DialogResult::None => self.line_num_dlg = Some(d),
            DialogResult::Cancel => self.focus = true,
            DialogResult::Ok => {
                let v = d.on.then_some(LineNumbering { distance: (d.dist > 0.0).then_some(d.dist), ..d.cfg });
                if v != self.caret_line_numbers() {
                    self.set_line_numbers(v);
                }
                self.focus = true;
            }
        }
    }
}

impl DocView {
    /// Items of the Hyphenation menu.
    pub(super) fn hyphenation_items(&mut self, ui: &mut egui::Ui) {
        let auto = self.doc.auto_hyphenation;
        if ui.radio(!auto, tr("hyph.none")).clicked() {
            self.set_hyphenation(false, false);
            ui.close();
        }
        // Again when on: hyphenates the text written since.
        if ui.radio(auto, tr("hyph.auto")).on_hover_text(tr("hyph.auto_hint")).clicked() {
            self.hyphenate_all();
            ui.close();
        }
        ui.separator();
        if ui.button(tr("hyph.optional")).on_hover_text(tr("hyph.optional_hint")).clicked() {
            self.insert("\u{AD}");
            ui.close();
        }
        if ui.button(tr("hyph.nonbreaking")).clicked() {
            self.insert("\u{2011}");
            ui.close();
        }
        ui.separator();
        let any = self.doc.paragraphs.iter().any(|p| p.text.contains(apex_doc::SOFT_HYPHEN));
        if ui.add_enabled(any, egui::Button::new(tr("hyph.remove"))).clicked() {
            self.set_hyphenation(false, true);
            ui.close();
        }
    }

    fn hyphenate_all(&mut self) {
        self.set_hyphenation(true, false);
    }

    /// Automatic hyphenation on (hyphenating what isn't yet) or off
    /// (`strip`: taking the hyphens out), as one undo step.
    fn set_hyphenation(&mut self, on: bool, strip: bool) {
        let (_, after) = self.doc.set_auto_hyphenation(on, strip, self.sel);
        self.sel = after;
    }
}
