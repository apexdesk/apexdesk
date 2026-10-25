//! The language of the text: the Language dialog (Review > Language) that
//! marks the selection as written in a language or not to be checked.

use std::sync::Arc;

use super::*;
use crate::widgets::DialogResult;

pub(super) struct LangDlg {
    /// "en-US"; `None`: the document's language.
    lang: Option<String>,
    no_proof: bool,
    filter: String,
}

/// Languages offered even without a dictionary.
const COMMON: [&str; 16] = ["es-ES", "es-MX", "es-AR", "en-US", "en-GB", "ca-ES", "gl-ES", "eu-ES", "pt-PT", "pt-BR", "fr-FR", "de-DE", "it-IT", "nl-NL", "sv-SE", "pl-PL"];

fn choices() -> Vec<(String, String, bool)> {
    let installed: Vec<String> = crate::spell::languages().into_iter().map(|l| l.replace('_', "-")).collect();
    let mut all: Vec<String> = COMMON.iter().map(|s| s.to_string()).collect();
    for l in &installed {
        if !all.iter().any(|x| x.eq_ignore_ascii_case(l)) {
            all.push(l.clone());
        }
    }
    let mut out: Vec<(String, String, bool)> = all.into_iter().map(|c| (crate::spell::language_name(&c), c.clone(), installed.iter().any(|i| i.eq_ignore_ascii_case(&c)))).collect();
    out.sort();
    out
}

impl DocView {
    pub(super) fn open_language_dialog(&mut self) {
        let a = self.attrs_at_caret();
        self.lang_dlg = Some(LangDlg { lang: a.lang.as_deref().map(String::from), no_proof: a.no_proof, filter: String::new() });
    }

    pub(super) fn language_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.lang_dlg.take() else { return };
        let res = widgets::dialog(ctx, "doc_text_language", tr("lang.title"), 380.0, |ui| {
            ui.label(tr("lang.mark_as"));
            ui.add(egui::TextEdit::singleline(&mut d.filter).hint_text(tr("lang.filter")).desired_width(f32::INFINITY));
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_size(vec2(ui.available_width(), 220.0));
                egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, false]).show(ui, |ui| {
                    if ui.selectable_label(d.lang.is_none(), tr("lang.document")).clicked() {
                        d.lang = None;
                    }
                    let f = d.filter.to_lowercase();
                    for (name, code, dict) in choices() {
                        if !f.is_empty() && !name.to_lowercase().contains(&f) && !code.to_lowercase().contains(&f) {
                            continue;
                        }
                        let label = if dict { format!("{name}   ({})", tr("lang.has_dict")) } else { name };
                        if ui.selectable_label(d.lang.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(&code)), label).clicked() {
                            d.lang = Some(code);
                        }
                    }
                });
            });
            ui.label(egui::RichText::new(tr("lang.dict_hint")).small().weak());
            ui.add_space(4.0);
            ui.checkbox(&mut d.no_proof, tr("lang.no_proof"));
        });
        match res {
            DialogResult::None => self.lang_dlg = Some(d),
            DialogResult::Cancel => self.focus = true,
            DialogResult::Ok => {
                let lang: Option<Arc<str>> = d.lang.as_deref().map(Arc::from);
                let no_proof = d.no_proof;
                let sel = self.sel_or_word();
                if sel.is_empty() {
                    // Applies to what's typed next.
                    let mut a = self.attrs_at_caret();
                    (a.lang, a.no_proof) = (lang, no_proof);
                    self.pending = Some(a);
                } else {
                    self.doc.format_range(sel, &|a| {
                        a.lang = lang.clone();
                        a.no_proof = no_proof;
                    });
                }
                self.focus = true;
            }
        }
    }
}
