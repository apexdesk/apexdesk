//! The question before saving in a format that can't keep what the file
//! has (a workbook with several sheets, formulas or formatting as CSV, a
//! formatted document as plain text), asked before writing as Excel and
//! LibreOffice do: keep that format, or save in the program's own.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use apex_io::{Format, Kind};
use eframe::egui::{self, RichText};

use super::{App, SaveStep, View};
use crate::i18n::{tr, trf};

struct Prompt {
    tab: u64,
    path: PathBuf,
    fmt: Format,
    /// What would be lost (keys of their descriptions).
    what: Vec<&'static str>,
}

#[derive(Default)]
pub(super) struct Lossy {
    prompt: Option<Prompt>,
    /// Tabs whose user chose to keep such a format: asked once a session.
    kept: HashSet<u64>,
}

/// What saving `view` in `fmt` would lose (keys of their descriptions);
/// empty when it keeps everything.
pub(super) fn losses(view: &View, fmt: Format) -> Vec<&'static str> {
    match view {
        View::Sheet(v) if matches!(fmt, Format::Csv | Format::Tsv | Format::Text) => sheet_losses(&v.wb),
        // Code saved as it is loses nothing.
        View::Doc(v) if fmt == Format::Text && !v.saves_as_typed(fmt) => {
            if doc_formatted(&v.backup_doc()) { vec!["lossy.doc_formatting"] } else { Vec::new() }
        }
        _ => Vec::new(),
    }
}

/// CSV keeps the values of the first sheet, as shown.
fn sheet_losses(wb: &apex_sheet::Workbook) -> Vec<&'static str> {
    let mut out = Vec::new();
    if wb.sheets.len() > 1 {
        out.push("lossy.sheets");
    }
    let Some(sh) = wb.sheets.first() else { return out };
    // A number format is kept: values are written as shown.
    let bare = |id: u32| {
        let mut s = wb.styles.get(id).clone();
        s.num_fmt = wb.styles.get(0).num_fmt.clone();
        s
    };
    let normal = bare(0);
    let mut plain: HashMap<u32, bool> = HashMap::new();
    let (mut formulas, mut formatted) = (false, false);
    for (_, c) in sh.iter() {
        formulas |= matches!(c.content, apex_sheet::Content::Formula { .. });
        if !formatted {
            formatted = c.extra.is_some() || (c.style != 0 && !*plain.entry(c.style).or_insert_with(|| bare(c.style) == normal));
        }
        if formulas && formatted {
            break;
        }
    }
    formatted |= !sh.merges.is_empty() || !sh.images.is_empty() || !sh.charts.is_empty() || !sh.shapes.is_empty() || !sh.cond_formats.is_empty();
    if formulas {
        out.push("lossy.formulas");
    }
    if formatted {
        out.push("lossy.formatting");
    }
    out
}

/// Whether the document has anything plain text can't hold.
fn doc_formatted(doc: &apex_doc::Document) -> bool {
    let plain = apex_doc::ParaStyle::default();
    doc.paragraphs.iter().any(|p| p.style != plain || p.cell.is_some() || p.section.is_some() || !p.marks.is_empty() || p.runs.iter().any(|r| r.attrs != apex_doc::Attrs::default()))
        || doc.has_headers()
        || !doc.comments.is_empty()
}

impl App {
    /// Whether tab `idx` can be saved at `path` in `fmt` without losing
    /// anything (or its user already chose that format); else asks first.
    pub(super) fn lossy_fits(&mut self, idx: usize, path: &Path, fmt: Format) -> bool {
        let Some(t) = self.tabs.get(idx) else { return true };
        if fmt.is_rich() || self.lossy.kept.contains(&t.id) {
            return true;
        }
        let what = losses(&t.view, fmt);
        if what.is_empty() {
            return true;
        }
        self.lossy.prompt = Some(Prompt { tab: t.id, path: path.to_path_buf(), fmt, what });
        false
    }

    /// The program's own format for tab `idx`'s kind (the settings' one).
    fn native_ext(&self, idx: usize) -> &'static str {
        let (own, fallback) = match self.tabs[idx].view.kind() {
            Kind::Sheet => (self.settings.save_sheet_format.as_str(), "xlsx"),
            Kind::Doc => (self.settings.save_doc_format.as_str(), "docx"),
            Kind::Slides => ("", "pptx"),
        };
        ["xlsx", "ods", "fods", "docx", "odt", "fodt"].into_iter().find(|e| *e == own && Format::from_path(Path::new(&format!("x.{e}"))).is_some_and(|f| f.kind() == self.tabs[idx].view.kind())).unwrap_or(fallback)
    }

    /// "Keep this format?"
    pub(super) fn lossy_dialog(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.lossy.prompt else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == p.tab) else {
            self.lossy.prompt = None;
            return;
        };
        let format = p.fmt.extension().to_uppercase();
        let native = self.native_ext(idx);
        let mut choice = None;
        let m = egui::Modal::new(egui::Id::new("lossy_save")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(tr("lossy.title"));
            ui.add_space(8.0);
            ui.label(trf("lossy.body", &[("format", &format)]));
            ui.add_space(4.0);
            for w in &p.what {
                ui.label(format!("• {}", tr(w)));
            }
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("dialog.cancel")).clicked() {
                    choice = Some(0);
                }
                if ui.button(trf("lossy.keep", &[("format", &format)])).clicked() {
                    choice = Some(1);
                }
                if ui.button(RichText::new(trf("lossy.native", &[("format", &native.to_uppercase())])).strong()).clicked() {
                    choice = Some(2);
                }
            });
        });
        if choice.is_none() && m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            choice = Some(0);
        }
        let Some(choice) = choice else { return };
        let Some(p) = self.lossy.prompt.take() else { return };
        self.answer_lossy(idx, p.path, p.fmt, choice, native);
    }

    /// 1: keep the format (and don't ask again for this tab); 2: ask where
    /// to save it in `native`; else nothing.
    fn answer_lossy(&mut self, idx: usize, path: PathBuf, fmt: Format, choice: u8, native: &'static str) {
        match choice {
            1 => {
                self.lossy.kept.insert(self.tabs[idx].id);
                self.save_from(idx, path, fmt, SaveStep::Signature);
            }
            2 => self.ask_save_path_as(idx, Some(native), |app, target| {
                if let Some((idx, path, fmt)) = target {
                    app.save_checked(idx, path, fmt);
                }
            }),
            _ => {}
        }
    }

    /// Answers the question being asked, as the dialog's buttons do.
    #[cfg(test)]
    pub(super) fn answer_lossy_for_test(&mut self, choice: u8) -> bool {
        let Some(p) = self.lossy.prompt.take() else { return false };
        let Some(idx) = self.tabs.iter().position(|t| t.id == p.tab) else { return false };
        let native = self.native_ext(idx);
        self.answer_lossy(idx, p.path, p.fmt, choice, native);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_plain_formats_lose() {
        let plain = apex_doc::Document::from_plain_text("One\nTwo");
        assert!(!doc_formatted(&plain));
        let mut bold = plain.clone();
        bold.paragraphs[0].runs[0].attrs.bold = true;
        assert!(doc_formatted(&bold));
        let mut wb = apex_sheet::Workbook::new();
        assert!(sheet_losses(&wb).is_empty());
        wb.sheets.push(wb.sheets[0].clone());
        assert_eq!(sheet_losses(&wb), ["lossy.sheets"]);
    }
}
