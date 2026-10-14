//! File > Compare with another file in the macro editor: the modules of
//! the project being edited next to those of another version of the
//! file, with the changed lines of each.

use std::path::Path;

use apex_doc::{DiffOp as Op, diff};
use apex_io::vba::VbaProject;
use eframe::egui::{self, RichText};

use crate::i18n::{tr, trf};

/// Lines compared at most (rows × rows); past it a module shows as all
/// removed and added.
const LIMIT: usize = 4_000_000;

pub(super) struct Compare {
    /// The other file's name.
    file: String,
    /// Its modules: (name, code).
    other: Vec<(String, String)>,
    selected: String,
    /// The diff shown: for (module, hash of this side's code).
    shown: Option<(String, u64, Vec<(char, String)>)>,
}

/// The other file's modules, or why they can't be read. A locked project
/// isn't shown: its code is hidden by its author.
pub(super) fn load(path: &Path) -> Result<Compare, String> {
    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (loaded, _) = apex_io::open(path).map_err(|e| trf("error.open", &[("file", &file), ("error", &e.to_string())]))?;
    let bin = match loaded {
        apex_io::Loaded::Sheet(wb) => wb.macros.project.clone(),
        _ => None,
    };
    let Some(bin) = bin else { return Err(trf("macros.compare_none", &[("file", &file)])) };
    let p = apex_io::vba::read(&bin).map_err(|e| trf("error.open", &[("file", &file), ("error", &e.to_string())]))?;
    if p.protected {
        return Err(trf("macros.compare_locked", &[("file", &file)]));
    }
    Ok(Compare { file, other: p.modules.into_iter().map(|m| (m.name, m.code)).collect(), selected: String::new(), shown: None })
}

fn hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// The lines of `a` (the other file) and `b` (this one): ' ' the same,
/// '-' only there, '+' only here.
fn lines(a: &str, b: &str) -> Vec<(char, String)> {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    diff(&a, &b, LIMIT)
        .into_iter()
        .map(|op| match op {
            Op::Same(_, j) => (' ', b[j].to_string()),
            Op::Del(i) => ('-', a[i].to_string()),
            Op::Ins(j) => ('+', b[j].to_string()),
        })
        .collect()
}

/// The window, while `open` holds one.
pub(super) fn show(ctx: &egui::Context, open: &mut Option<Compare>, project: &VbaProject) {
    let Some(c) = open.as_mut() else { return };
    let mut keep = true;
    let mut names: Vec<&str> = project.modules.iter().map(|m| m.name.as_str()).collect();
    for (n, _) in &c.other {
        if !names.iter().any(|x| x.eq_ignore_ascii_case(n)) {
            names.push(n);
        }
    }
    let here = |n: &str| project.modules.iter().find(|m| m.name.eq_ignore_ascii_case(n)).map(|m| m.code.as_str());
    let there = |n: &str| c.other.iter().find(|m| m.0.eq_ignore_ascii_case(n)).map(|m| m.1.as_str());
    if !names.iter().any(|n| n.eq_ignore_ascii_case(&c.selected)) {
        // The first that differs.
        c.selected = names.iter().find(|n| here(n) != there(n)).or(names.first()).map(|n| n.to_string()).unwrap_or_default();
    }
    let title = trf("macros.compare_title", &[("file", &c.file)]);
    let mut selected = c.selected.clone();
    egui::Window::new(title).id(egui::Id::new("macro_compare")).open(&mut keep).default_size([760.0, 480.0]).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).show(ctx, |ui| {
        ui.label(RichText::new(tr("macros.compare_note")).weak());
        ui.separator();
        egui::Panel::left(egui::Id::new("macro_compare_list")).resizable(true).default_size(190.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                for n in &names {
                    let (key, warn) = match (here(n), there(n)) {
                        (Some(a), Some(b)) if a == b => ("macros.compare_same", false),
                        (Some(_), Some(_)) => ("macros.compare_changed", true),
                        (Some(_), None) => ("macros.compare_only_here", true),
                        _ => ("macros.compare_only_there", true),
                    };
                    let state = RichText::new(tr(key)).small();
                    let state = if warn { state.color(ui.visuals().warn_fg_color) } else { state.weak() };
                    ui.horizontal(|ui| {
                        if ui.selectable_label(n.eq_ignore_ascii_case(&selected), *n).clicked() {
                            selected = n.to_string();
                        }
                        ui.label(state);
                    });
                }
            });
        });
        let a = there(&selected).unwrap_or("");
        let b = here(&selected).unwrap_or("");
        let key = hash(b) ^ hash(a).rotate_left(1);
        if !c.shown.as_ref().is_some_and(|(n, h, _)| *n == selected && *h == key) {
            c.shown = Some((selected.clone(), key, lines(a, b)));
        }
        let Some((_, _, rows)) = &c.shown else { return };
        let font = egui::FontId::monospace(12.5);
        let (del, ins) = if ui.visuals().dark_mode {
            (egui::Color32::from_rgb(0x5A, 0x24, 0x24), egui::Color32::from_rgb(0x1E, 0x4A, 0x2A))
        } else {
            (egui::Color32::from_rgb(0xFD, 0xE2, 0xE1), egui::Color32::from_rgb(0xDC, 0xF5, 0xE0))
        };
        let row_h = ui.fonts_mut(|f| f.row_height(&font));
        egui::ScrollArea::both().auto_shrink(false).show_rows(ui, row_h, rows.len(), |ui, range| {
            for (k, text) in &rows[range] {
                let t = RichText::new(format!("{k} {text}")).font(font.clone());
                let t = match k {
                    '-' => t.background_color(del),
                    '+' => t.background_color(ins),
                    _ => t,
                };
                ui.add(egui::Label::new(t).extend());
            }
        });
    });
    c.selected = selected;
    if !keep {
        *open = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_lines() {
        let l = lines("Sub A()\n    x = 1\nEnd Sub\n", "Sub A()\n    x = 2\n    y = 3\nEnd Sub\n");
        let marks: String = l.iter().map(|r| r.0).collect();
        assert_eq!(marks, " -++ ");
        assert_eq!(l[2].1, "    x = 2");
    }

    #[test]
    fn window() {
        let bin = apex_io::vba::new_workbook_project("ThisWorkbook", &[]).unwrap();
        let mut p = apex_io::vba::read(&bin).unwrap();
        p.modules.push(apex_io::vba::VbaModule::standard("Module1"));
        p.modules[1].code = "Sub A()\nEnd Sub\n".into();
        let other = vec![("ThisWorkbook".to_string(), p.modules[0].code.clone()), ("Module1".to_string(), "Sub B()\nEnd Sub\n".to_string()), ("Old".to_string(), String::new())];
        let mut open = Some(Compare { file: "v1.xlsm".into(), other, selected: String::new(), shown: None });
        let ctx = egui::Context::default();
        for _ in 0..2 {
            ctx.run_ui(Default::default(), |ui| show(ui.ctx(), &mut open, &p)).textures_delta.clear();
        }
        let c = open.unwrap();
        // The first module that differs is shown.
        assert_eq!(c.selected, "Module1");
        assert_eq!(c.shown.unwrap().2.iter().map(|r| r.0).collect::<String>(), "-+ ");
    }
}
