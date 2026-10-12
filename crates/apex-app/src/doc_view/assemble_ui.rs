//! Mailings > Assemble: conditional sections (`«IF Field»...«END IF»`
//! around the selection) and Fill In Form, which asks for every field of
//! the document (merge fields, named or bound content controls and the
//! fields of conditions) and opens the assembled copy in a new tab. From a
//! sheet or a database the same assembly runs with each record when
//! merging ([`apex_doc::assemble`]).

use apex_doc::{Pos, Selection, assemble, merge};
use eframe::egui;

use super::DocView;
use crate::common::AppCmd;
use crate::i18n::tr;
use crate::widgets::{self, DialogResult};

/// Fill In Form: a value for each field.
pub(super) struct FormDlg {
    names: Vec<String>,
    values: Vec<String>,
}

impl DocView {
    pub(super) fn assemble_group(&mut self, b: &mut crate::toolstrip::Body) {
        use crate::icons::Icon;
        use crate::toolstrip;
        let a = b.accent();
        b.group("toolstrip.g.assemble", Icon::Split, |ui| {
            let mut fields: Vec<String> = self.mail.recipients.as_ref().map(|r| r.fields.clone()).unwrap_or_default();
            for f in assemble::form_fields(&self.doc) {
                if !fields.iter().any(|x| x.eq_ignore_ascii_case(&f)) {
                    fields.push(f);
                }
            }
            toolstrip::large_menu(ui, Icon::Filter, tr("assemble.section"), tr("assemble.section_tip"), a, |ui| {
                if fields.is_empty() {
                    ui.weak(tr("assemble.no_fields"));
                }
                for f in &fields {
                    if ui.button(f).clicked() {
                        self.wrap_section(f);
                        ui.close();
                    }
                }
            });
            if toolstrip::large(ui, Icon::Checkbox, &format!("{}…", tr("assemble.fill")), tr("assemble.fill_tip"), a, false).clicked() {
                self.open_fill_form();
            }
        });
    }

    /// Puts the selection (or the caret) between `«IF field»` and `«END IF»`.
    fn wrap_section(&mut self, field: &str) {
        let (a, b) = (self.sel.start(), self.sel.end());
        let end = self.doc.insert_text(Selection::caret(b), &merge::field_text("END IF"), None);
        self.doc.insert_text(Selection::caret(a), &merge::field_text(&format!("IF {field}")), None);
        let shift = if a.para == end.para { merge::field_text(&format!("IF {field}")).len() } else { 0 };
        self.sel = Selection::caret(Pos::new(end.para, end.offset + shift));
        self.scroll_to_caret = true;
    }

    fn open_fill_form(&mut self) {
        let names = assemble::form_fields(&self.doc);
        if names.is_empty() {
            self.notices.push(crate::app::Notice::info(tr("assemble.no_fields")));
            return;
        }
        let values = vec![String::new(); names.len()];
        self.mail.form = Some(FormDlg { names, values });
    }

    pub(super) fn fill_form_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.mail.form.take() else { return };
        let res = widgets::dialog(ctx, "assemble_form", tr("assemble.fill"), 440.0, |ui| {
            ui.label(egui::RichText::new(tr("assemble.fill_hint")).weak());
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                widgets::form(ui, "assemble_grid", |ui| {
                    for (n, v) in f.names.iter().zip(f.values.iter_mut()) {
                        widgets::form_row(ui, n, |ui| ui.add(egui::TextEdit::singleline(v).desired_width(240.0)));
                    }
                });
            });
        });
        match res {
            DialogResult::Ok => {
                let out = merge::fill(&self.doc, &f.names, &f.values);
                self.mail.out = Some(AppCmd::OpenDoc(super::DocToOpen(std::sync::Arc::new(out)), false));
            }
            DialogResult::Cancel => {}
            DialogResult::None => self.mail.form = Some(f),
        }
    }
}
