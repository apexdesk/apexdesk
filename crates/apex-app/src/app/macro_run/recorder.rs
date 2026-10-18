//! Record Macro: the user's edits of a workbook written as VBA (the
//! workbook's recorder, `apex_sheet::record`), then put as a `Sub` into a
//! module of the workbook or of the personal macro workbook.
//!
//! The recorder is the tab's workbook's, which macros' copies don't get:
//! what a macro does isn't recorded, only that it ran.

use apex_fs::PathExt;
use std::path::Path;

use apex_io::vba::{VbaModule, VbaProject};
use apex_sheet::record::Recorder;
use eframe::egui;

use super::super::{App, Notice, View};
use crate::i18n::{tr, trf};

/// More lines than this aren't kept (the macro would be unusable).
const MAX_LINES: usize = 100_000;

#[derive(Default)]
pub(in crate::app) struct Record {
    dlg: Option<Dlg>,
    on: Option<Recording>,
    /// Use Relative References.
    relative: bool,
}

impl Record {
    /// The tab being recorded.
    pub(in crate::app) fn tab(&self) -> Option<u64> {
        self.on.as_ref().map(|r| r.tab)
    }
}

/// The Record Macro dialog.
struct Dlg {
    tab: u64,
    name: String,
    letter: String,
    personal: bool,
    description: String,
    /// Code without Select/Selection chains.
    clean: bool,
    error: Option<String>,
    focus: bool,
}

struct Recording {
    tab: u64,
    name: String,
    letter: Option<char>,
    personal: bool,
    description: String,
    clean: bool,
    lines: Vec<String>,
}

/// The names of the procedures of a project.
fn procs(project: &VbaProject) -> Vec<String> {
    project.modules.iter().flat_map(|m| super::super::vba_edit::proc_rows(&m.code)).map(|p| p.0).collect()
}

/// The `Sub` the recorder wrote, with the comments Excel puts at its top.
fn macro_code(name: &str, letter: Option<char>, description: &str, lines: &[String]) -> String {
    let mut s = format!("Sub {name}()\n'\n' {name} Macro\n");
    for l in description.lines() {
        s.push_str(&format!("' {l}\n"));
    }
    if let Some(c) = letter {
        let keys = if c.is_ascii_uppercase() { format!("Ctrl+Shift+{c}") } else { format!("Ctrl+{c}") };
        s.push_str(&format!("'\n' Keyboard Shortcut: {keys}\n"));
    }
    s.push_str("'\n");
    for l in lines {
        s.push_str("    ");
        s.push_str(l);
        s.push('\n');
    }
    s.push_str("End Sub\n");
    s
}

/// Puts the macro into `project`: at the end of Module1 (made when there
/// is none), with its shortcut.
fn add_macro(project: &mut VbaProject, name: &str, letter: Option<char>, code: &str) {
    let i = match project.modules.iter().position(|m| m.kind == apex_io::vba::ModuleKind::Standard && m.name.eq_ignore_ascii_case("Module1")) {
        Some(i) => i,
        None => {
            let taken = |n: &str| project.modules.iter().any(|m| m.name.eq_ignore_ascii_case(n));
            let n = (1..).map(|n| format!("Module{n}")).find(|n| !taken(n)).unwrap_or_default();
            project.modules.push(VbaModule::standard(&n));
            project.modules.len() - 1
        }
    };
    let m = &mut project.modules[i];
    let mut text = m.code.trim_end().to_string();
    if !text.is_empty() {
        text.push_str("\n\n");
    }
    text.push_str(code);
    m.code = super::schedule::set_shortcut(&text, name, letter).unwrap_or(text);
}

/// Why the macro can't go into `project`.
fn refusal(project: &VbaProject, signed: bool, name: &str) -> Option<String> {
    if project.protected {
        return Some(tr("macros.locked").to_string());
    }
    if signed {
        return Some(tr("macros.signed").to_string());
    }
    procs(project).iter().any(|p| p.eq_ignore_ascii_case(name)).then(|| trf("macros.record_taken", &[("name", &name)]))
}

/// The personal macro workbook's project as it is in its file.
fn personal_project(path: &Path) -> Result<(apex_sheet::Workbook, VbaProject), String> {
    if !path.fs_exists() {
        super::library::new_personal(path)?;
    }
    let (apex_io::Loaded::Sheet(wb), _) = apex_io::open(path).map_err(|e| e.to_string())? else { return Err(tr("macros.personal").to_string()) };
    let bin = wb.macros.project.clone().ok_or_else(|| tr("macros.personal").to_string())?;
    let project = apex_io::vba::read(&bin).map_err(|e| e.to_string())?;
    Ok((*wb, project))
}

impl App {
    /// The Record Macro buttons: (recording, relative), in a workbook.
    pub(in crate::app) fn recording_state(&self) -> Option<(bool, bool)> {
        let r = &self.macro_run.record;
        let sheet = matches!(self.tabs.get(self.active).map(|t| &t.view), Some(View::Sheet(_)));
        (sheet || r.on.is_some()).then_some((r.on.is_some(), r.relative))
    }

    /// Record Macro, or Stop Recording.
    pub(in crate::app) fn toggle_record(&mut self) {
        if self.macro_run.record.on.is_some() {
            self.stop_record();
            return;
        }
        let Some(t) = self.tabs.get(self.active) else { return };
        let View::Sheet(v) = &t.view else { return };
        let tab = t.id;
        let taken: Vec<String> = v.wb.macros.project.as_ref().and_then(|b| apex_io::vba::read(b).ok()).map(|p| procs(&p)).unwrap_or_default();
        let name = (1..).map(|n| format!("Macro{n}")).find(|n| !taken.iter().any(|p| p.eq_ignore_ascii_case(n))).unwrap_or_default();
        self.macro_run.record.dlg = Some(Dlg { tab, name, letter: String::new(), personal: false, description: String::new(), clean: true, error: None, focus: true });
    }

    /// Use Relative References, also while recording.
    pub(in crate::app) fn toggle_relative(&mut self) {
        let r = &mut self.macro_run.record;
        r.relative = !r.relative;
        let relative = r.relative;
        let Some(tab) = r.tab() else { return };
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab)
            && let View::Sheet(v) = &mut t.view
            && let Some(rec) = v.wb.recorder.as_mut()
        {
            rec.relative = relative;
        }
    }

    /// The dialog, and what the recorder wrote this frame.
    pub(super) fn record_frame(&mut self, ctx: &egui::Context) {
        self.record_dialog(ctx);
        let relative = self.macro_run.record.relative;
        let Some(rec) = self.macro_run.record.on.as_mut() else { return };
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == rec.tab) else {
            // Closed: what it had is lost with it.
            self.macro_run.record.on = None;
            return;
        };
        let View::Sheet(v) = &mut t.view else { return };
        let (sheet, sel) = v.macro_place();
        let name = v.wb.sheets.get(sheet).map(|s| s.name.clone()).unwrap_or_default();
        // A macro or reading the file again gave the tab another workbook.
        let r = v.wb.recorder.get_or_insert_with(|| Box::new(Recorder::new(relative)));
        r.select(&name, sel, sel.start);
        let lines = r.take_lines();
        if rec.lines.len() < MAX_LINES {
            rec.lines.extend(lines);
            if rec.lines.len() >= MAX_LINES {
                rec.lines.truncate(MAX_LINES);
                rec.lines.push("' ...".into());
            }
        }
    }

    /// A macro the user ran is recorded as run.
    pub(super) fn record_run(&mut self, tab: u64, name: &str) {
        let Some(rec) = self.macro_run.record.on.as_mut().filter(|r| r.tab == tab) else { return };
        // The selection it starts with first: the workbook it leaves
        // has a recorder of its own.
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab)
            && let View::Sheet(v) = &mut t.view
            && let Some(r) = v.wb.recorder.as_mut()
        {
            r.finish();
            rec.lines.extend(r.take_lines());
        }
        if rec.lines.len() < MAX_LINES {
            rec.lines.push(format!("Application.Run \"{}\"", name.replace('"', "\"\"")));
        }
    }

    fn record_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.macro_run.record.dlg else { return };
        let (mut ok, mut cancel) = (false, false);
        let m = egui::Modal::new(egui::Id::new("record_macro")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(tr("macros.record_title"));
            ui.add_space(6.0);
            egui::Grid::new("record_macro_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label(tr("macros.record_name"));
                let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(220.0));
                if std::mem::take(&mut d.focus) {
                    r.request_focus();
                }
                ui.end_row();
                ui.label(tr("macros.shortcut"));
                ui.add(egui::TextEdit::singleline(&mut d.letter).char_limit(1).desired_width(28.0));
                ui.end_row();
                ui.label(tr("macros.record_in"));
                egui::ComboBox::from_id_salt("record_in").selected_text(if d.personal { tr("macros.personal") } else { tr("macros.record_this") }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.personal, false, tr("macros.record_this"));
                    ui.selectable_value(&mut d.personal, true, tr("macros.personal"));
                });
                ui.end_row();
                ui.label(tr("macros.record_description"));
                ui.add(egui::TextEdit::multiline(&mut d.description).desired_rows(2).desired_width(220.0));
                ui.end_row();
            });
            ui.label(egui::RichText::new(tr("macros.shortcut_tip")).weak());
            ui.checkbox(&mut d.clean, tr("macros.record_clean")).on_hover_text(tr("macros.record_clean_tip"));
            if let Some(e) = &d.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                cancel = ui.button(tr("dialog.cancel")).clicked();
                ok = ui.button(tr("dialog.ok")).clicked();
            });
        });
        if m.is_top_modal {
            ok |= ctx.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
            cancel |= ctx.input(|i| i.key_pressed(egui::Key::Escape));
        }
        if cancel {
            self.macro_run.record.dlg = None;
            return;
        }
        if !ok {
            return;
        }
        let name = d.name.trim().to_string();
        let letter = d.letter.trim().chars().next();
        let error = if !apex_io::vba::is_identifier(&name) || name.len() > 255 {
            Some(trf("macros.record_bad_name", &[("name", &name)]))
        } else if letter.is_some_and(|c| !c.is_ascii_alphabetic()) {
            Some(tr("macros.shortcut_tip").to_string())
        } else {
            None
        };
        let (tab, personal, description, clean) = (d.tab, d.personal, d.description.trim().to_string(), d.clean);
        let error = error.or_else(|| self.record_target_refusal(tab, personal, &name));
        if let Some(e) = error {
            if let Some(d) = &mut self.macro_run.record.dlg {
                d.error = Some(e);
            }
            return;
        }
        self.macro_run.record.dlg = None;
        let relative = self.macro_run.record.relative;
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) else { return };
        let View::Sheet(v) = &mut t.view else { return };
        let mut r = Recorder::new(relative);
        let (sheet, sel) = v.macro_place();
        let sheet_name = v.wb.sheets.get(sheet).map(|s| s.name.clone()).unwrap_or_default();
        r.select(&sheet_name, sel, sel.start);
        v.wb.recorder = Some(Box::new(r));
        self.macro_run.record.on = Some(Recording { tab, name, letter, personal, description, clean, lines: Vec::new() });
    }

    /// Why a macro named `name` can't be recorded into the tab's
    /// workbook or the personal one.
    fn record_target_refusal(&mut self, tab: u64, personal: bool, name: &str) -> Option<String> {
        if personal {
            let path = super::personal_path()?;
            if let Some(idx) = self.personal_tab(&path) {
                return self.tab_refusal(idx, name);
            }
            if !path.fs_exists() {
                return None;
            }
            return match personal_project(&path) {
                Ok((wb, p)) => refusal(&p, super::signed_book(&wb, &self.settings.trusted_publishers), name),
                Err(e) => Some(e),
            };
        }
        let idx = self.tabs.iter().position(|t| t.id == tab)?;
        self.tab_refusal(idx, name)
    }

    fn tab_refusal(&mut self, idx: usize, name: &str) -> Option<String> {
        let id = self.tabs[idx].id;
        self.flush_macros(id);
        let View::Sheet(v) = &self.tabs[idx].view else { return None };
        let bin = v.wb.macros.project.as_ref()?;
        match apex_io::vba::read(bin) {
            Ok(p) => refusal(&p, v.wb.macros.signed, name),
            Err(e) => Some(trf("macros.bad_project", &[("error", &e.to_string())])),
        }
    }

    /// The tab that has the personal macro workbook open.
    fn personal_tab(&self, path: &Path) -> Option<usize> {
        let want = path.fs_canonicalize().ok()?;
        self.tabs.iter().position(|t| matches!(t.view, View::Sheet(_)) && t.path.as_ref().and_then(|p| p.fs_canonicalize().ok()).is_some_and(|p| p == want))
    }

    /// Stop Recording: the macro goes into its module.
    fn stop_record(&mut self) {
        let Some(mut rec) = self.macro_run.record.on.take() else { return };
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == rec.tab)
            && let View::Sheet(v) = &mut t.view
            && let Some(mut r) = v.wb.recorder.take()
        {
            rec.lines.extend(r.finish());
        }
        if rec.clean {
            rec.lines = apex_sheet::record::clean(&rec.lines);
        }
        let code = macro_code(&rec.name, rec.letter, &rec.description, &rec.lines);
        let done = if rec.personal {
            self.record_into_personal(&rec, &code)
        } else {
            self.tabs.iter().position(|t| t.id == rec.tab).map_or(Err(String::new()), |idx| self.record_into_tab(idx, &rec, &code))
        };
        match done {
            Ok(()) => self.notify(Notice::info(trf("macros.recorded", &[("name", &rec.name)]))),
            Err(e) if e.is_empty() => {}
            Err(e) => self.notify(Notice::error(e)),
        }
    }

    fn record_into_personal(&mut self, rec: &Recording, code: &str) -> Result<(), String> {
        let path = super::personal_path().ok_or_else(|| tr("macros.personal").to_string())?;
        if let Some(idx) = self.personal_tab(&path) {
            return self.record_into_tab(idx, rec, code);
        }
        let (mut wb, mut project) = personal_project(&path)?;
        if let Some(e) = refusal(&project, super::signed_book(&wb, &self.settings.trusted_publishers), &rec.name) {
            return Err(e);
        }
        add_macro(&mut project, &rec.name, rec.letter, code);
        let bin = project.write().map_err(|e| trf("macros.bad_project", &[("error", &e.to_string())]))?;
        wb.macros.set_project(bin);
        apex_io::save_sheet(&wb, &path, apex_io::Format::Xlsx).map_err(|e| format!("{}: {e}", tr("macros.personal")))
    }

    /// Into the workbook of tab `idx` (a project made when it has none).
    fn record_into_tab(&mut self, idx: usize, rec: &Recording, code: &str) -> Result<(), String> {
        let id = self.tabs[idx].id;
        self.flush_macros(id);
        let View::Sheet(v) = &mut self.tabs[idx].view else { return Err(String::new()) };
        let had = v.wb.macros.project.is_some();
        let mut project = match &v.wb.macros.project {
            Some(bin) => apex_io::vba::read(bin).map_err(|e| trf("macros.bad_project", &[("error", &e.to_string())]))?,
            None => {
                let book = v.wb.code_name.clone().filter(|b| apex_io::vba::is_identifier(b)).unwrap_or_else(|| "ThisWorkbook".into());
                let bin = apex_io::vba::new_workbook_project(&book, &[]).map_err(|e| e.to_string())?;
                apex_io::vba::read(&bin).map_err(|e| e.to_string())?
            }
        };
        if let Some(e) = refusal(&project, v.wb.macros.signed, &rec.name) {
            return Err(e);
        }
        let mut book = v.wb.code_name.clone();
        let mut sheets: Vec<Option<String>> = v.wb.sheets.iter().map(|s| s.code_name.clone()).collect();
        project.match_book(&mut book, &mut sheets);
        add_macro(&mut project, &rec.name, rec.letter, code);
        let bin = project.write().map_err(|e| trf("macros.bad_project", &[("error", &e.to_string())]))?;
        self.hold(idx);
        self.macros.reload(id, &bin);
        let View::Sheet(v) = &mut self.tabs[idx].view else { return Err(String::new()) };
        v.wb.code_name = book;
        for (s, n) in v.wb.sheets.iter_mut().zip(sheets) {
            s.code_name = n;
        }
        v.wb.macros.set_project(bin);
        super::super::macros_ui::mark_modified(&mut self.tabs[idx].view, true);
        // The user's own macros, in a workbook that had none, may run.
        if !had {
            self.enable_macros(id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_macro_goes_into_module1() {
        let bin = apex_io::vba::new_workbook_project("ThisWorkbook", &[]).unwrap();
        let mut p = apex_io::vba::read(&bin).unwrap();
        let code = macro_code("Macro1", Some('K'), "Fills A1", &["Range(\"A1\").Value = 5".into()]);
        assert!(code.contains("' Keyboard Shortcut: Ctrl+Shift+K\n"), "{code}");
        add_macro(&mut p, "Macro1", Some('K'), &code);
        assert!(refusal(&p, false, "macro1").is_some());
        add_macro(&mut p, "Macro2", None, &macro_code("Macro2", None, "", &[]));
        let m = p.modules.iter().find(|m| m.name == "Module1").expect("Module1");
        assert!(m.code.starts_with("Sub Macro1()\nAttribute Macro1.VB_ProcData.VB_Invoke_Func = \"K\\n14\"\n'\n' Macro1 Macro\n' Fills A1\n"), "{}", m.code);
        assert!(m.code.contains("    Range(\"A1\").Value = 5\nEnd Sub\n\nSub Macro2()\n"), "{}", m.code);
        assert_eq!(procs(&p), ["Macro1", "Macro2"]);
        // What it wrote reads back.
        let bin = p.write().unwrap();
        let p = apex_io::vba::read(&bin).unwrap();
        assert!(apex_vba::Project::parse(&[apex_vba::Source { name: "Module1".into(), kind: apex_vba::ModuleKind::Standard, code: p.modules.iter().find(|m| m.name == "Module1").unwrap().code.clone() }]).is_ok());
    }
}
