//! "What it does" in Run Macro: a macro's code read without running it
//! (`apex_vba::explain`), as sentences.

use apex_vba::explain::Explanation;
use apex_vba::suspicious::Signal;
use eframe::egui::{self, RichText};

use crate::i18n::{tr, trf};

/// The sentences, each with whether it is worth a warning color.
pub(super) fn lines(e: &Explanation) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    if e.not_found {
        return out;
    }
    let list = |v: &[String]| v.join(", ");
    if !e.signals.is_empty() {
        let why: Vec<&str> = e
            .signals
            .iter()
            .map(|s| match s {
                Signal::AutoRun => tr("macros.sig_autorun"),
                Signal::RunsPrograms => tr("macros.sig_programs"),
                Signal::Downloads => tr("macros.sig_downloads"),
                Signal::WindowsApi => tr("macros.sig_api"),
                Signal::Obfuscated => tr("macros.sig_hidden"),
            })
            .collect();
        out.push((trf("macros.explain_signals", &[("why", &why.join(", "))]), true));
    }
    if !e.sheets.is_empty() {
        out.push((trf("macros.explain_sheets", &[("list", &list(&e.sheets))]), false));
    }
    if !e.ranges.is_empty() {
        out.push((trf("macros.explain_ranges", &[("list", &list(&e.ranges))]), false));
    }
    let flags = [
        (e.uses_selection, "macros.explain_selection", false),
        (e.writes_cells, "macros.explain_writes", false),
        (e.formats, "macros.explain_formats", false),
        (e.clears, "macros.explain_clears", true),
        (e.deletes_cells, "macros.explain_deletes_cells", true),
        (e.adds_sheets, "macros.explain_adds_sheets", false),
        (e.deletes_sheets, "macros.explain_deletes_sheets", true),
        (e.reads_files, "macros.explain_reads_files", false),
        (e.writes_files, "macros.explain_writes_files", true),
        (e.deletes_files, "macros.explain_deletes_files", true),
        (e.saves, "macros.explain_saves", false),
        (e.prints, "macros.explain_prints", false),
        (e.asks_user, "macros.explain_asks", false),
        (e.loops, "macros.explain_loops", false),
    ];
    for (on, key, warn) in flags {
        if on {
            out.push((tr(key).to_string(), warn));
        }
    }
    if !e.objects.is_empty() {
        out.push((trf("macros.explain_objects", &[("list", &list(&e.objects))]), true));
    }
    if !e.calls.is_empty() {
        out.push((trf("macros.explain_calls", &[("list", &list(&e.calls))]), false));
    }
    if out.is_empty() {
        out.push((tr("macros.explain_nothing").to_string(), false));
    }
    out
}

/// The sentences under a title, with the note on what can't be seen.
pub(super) fn show(ui: &mut egui::Ui, lines: &[(String, bool)]) {
    ui.label(RichText::new(tr("macros.explain_title")).strong()).on_hover_text(tr("macros.explain_note"));
    egui::ScrollArea::vertical().id_salt("macro_explain").max_height(130.0).show(ui, |ui| {
        for (text, warn) in lines {
            let t = RichText::new(format!("• {text}"));
            ui.label(if *warn { t.color(ui.visuals().warn_fg_color) } else { t });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences() {
        let e = apex_vba::explain::explain(&[("M", "Sub A()\n    Sheets(\"Data\").Range(\"A1\").ClearContents\nEnd Sub\n")], "A");
        let l = lines(&e);
        assert_eq!(l.len(), 3);
        assert!(l[2].1, "clearing is a warning");
        assert_eq!(lines(&Explanation::default()), [(tr("macros.explain_nothing").to_string(), false)]);
    }
}
