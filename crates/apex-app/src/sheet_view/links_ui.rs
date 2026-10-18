//! Data > Edit Links: the other workbooks formulas refer to. Update their
//! values from the files, point them to another file, open them or turn
//! the formulas that use them into values.

use apex_fs::PathExt;
use std::path::{Path, PathBuf};

use super::*;
use crate::common::AppCmd;
use crate::widgets::DialogResult;

pub(super) struct LinksDlg {
    selected: usize,
    /// Whether each link's file was found (None: not checked yet).
    status: Vec<Option<bool>>,
    /// Asking before Break Link.
    confirm_break: bool,
}

/// Where a link's file is: its path or URL as saved, next to the workbook,
/// or by name in the workbook's folder.
fn resolve(target: &str, dir: Option<&Path>) -> Option<PathBuf> {
    let t = target.strip_prefix("file://").map(|p| p.replace("%20", " ")).unwrap_or_else(|| target.to_string());
    let t = t.trim_start_matches('/').to_string();
    let t = if target.starts_with("file://") || target.starts_with('/') { format!("/{t}") } else { t };
    let mut tries = vec![PathBuf::from(&t)];
    if let Some(d) = dir {
        tries.push(d.join(&t));
        let name = t.rsplit(['/', '\\']).next().unwrap_or(&t);
        tries.push(d.join(name));
    }
    // Network shares, and files that aren't documents, are not even looked for.
    tries.into_iter().find(|p| crate::links::in_app_refusal(&p.to_string_lossy(), cfg!(windows)).is_none() && p.is_absolute() && p.fs_is_file())
}

/// How a link to `path` is saved: relative to the workbook's folder when
/// it is inside it.
pub(super) fn target_for(path: &Path, dir: Option<&Path>) -> String {
    match dir.and_then(|d| path.strip_prefix(d).ok()) {
        Some(rel) => rel.to_string_lossy().replace('\\', "/"),
        None => path.to_string_lossy().into_owned(),
    }
}

impl SheetView {
    pub(super) fn open_edit_links(&mut self) {
        self.settle_edit();
        if self.wb.externals.is_empty() {
            self.notices.push(Notice::info(tr("links.none")));
            return;
        }
        let dir = self.file_dir.clone();
        let status = self.wb.externals.iter().map(|b| Some(resolve(&b.target, dir.as_deref()).is_some())).collect();
        self.links_dlg = Some(LinksDlg { selected: 0, status, confirm_break: false });
    }

    /// Takes the values of link `i` from its file.
    fn update_link(&mut self, i: usize) -> bool {
        let target = self.wb.externals[i].target.clone();
        let Some(path) = resolve(&target, self.file_dir.as_deref()) else {
            self.notices.push(Notice::error(trf("links.not_found_file", &[("file", &target)])));
            return false;
        };
        match apex_io::open(&path) {
            Ok((apex_io::Loaded::Sheet(src), _)) => {
                let r = self.wb.update_external(i, &src);
                if !r.missing_sheets.is_empty() {
                    self.notices.push(Notice::error(trf("links.missing_sheets", &[("sheets", &r.missing_sheets.join(", "))])));
                }
                true
            }
            Ok(_) => {
                self.notices.push(Notice::error(trf("links.not_a_workbook", &[("file", &path.display().to_string())])));
                false
            }
            Err(e) => {
                self.notices.push(Notice::error(trf("links.open_failed", &[("file", &path.display().to_string()), ("error", &e.to_string())])));
                false
            }
        }
    }

    pub(super) fn edit_links_dialog(&mut self, ctx: &egui::Context, shell: &mut Shell) {
        let Some(mut d) = self.links_dlg.take() else { return };
        let n = self.wb.externals.len();
        if n == 0 {
            self.focus_grid = true;
            return;
        }
        d.selected = d.selected.min(n - 1);
        d.status.resize(n, None);
        if d.confirm_break {
            let name = self.wb.externals[d.selected].file_name().to_string();
            let res = widgets::dialog(ctx, "break_link", tr("links.break"), 400.0, |ui| {
                ui.label(trf("links.break_confirm", &[("file", &name)]));
            });
            match res {
                DialogResult::None => {}
                DialogResult::Cancel => d.confirm_break = false,
                DialogResult::Ok => {
                    d.confirm_break = false;
                    self.wb.break_external(d.selected);
                    d.status.remove(d.selected);
                }
            }
            self.links_dlg = Some(d);
            return;
        }
        let uses = self.wb.external_use_counts();
        let dir = self.file_dir.clone();
        let mut action = None;
        let res = widgets::dialog(ctx, "edit_links", tr("links.title"), 620.0, |ui| {
            widgets::section(ui, tr("links.sec_links"));
            ui.horizontal_top(|ui| {
                egui::Frame::new().stroke(ui.visuals().widgets.noninteractive.bg_stroke).inner_margin(4.0).show(ui, |ui| {
                    ui.set_width(440.0);
                    egui::ScrollArea::vertical().min_scrolled_height(150.0).max_height(180.0).auto_shrink([false, false]).show(ui, |ui| {
                        egui::Grid::new("links_grid").num_columns(3).spacing([16.0, 4.0]).min_col_width(60.0).show(ui, |ui| {
                            for key in ["links.source", "links.used_by", "links.status"] {
                                ui.label(egui::RichText::new(tr(key)).weak());
                            }
                            ui.end_row();
                            for (i, b) in self.wb.externals.iter().enumerate() {
                                if ui.selectable_label(d.selected == i, b.file_name()).clicked() {
                                    d.selected = i;
                                }
                                ui.label(trf("links.cells", &[("n", &uses.get(i).copied().unwrap_or(0).to_string())]));
                                match d.status[i] {
                                    Some(true) => ui.label(tr("links.ok")),
                                    Some(false) => ui.colored_label(ui.visuals().error_fg_color, tr("links.not_found")),
                                    None => ui.label(tr("links.unknown")),
                                };
                                ui.end_row();
                            }
                        });
                    });
                });
                ui.vertical(|ui| {
                    let w = ui.available_width();
                    let found = d.status[d.selected] == Some(true);
                    for (k, key, on) in [(0, "links.update", found), (1, "links.change", true), (2, "links.open", found), (3, "links.break", true), (4, "links.check", true)] {
                        if ui.add_enabled(on, egui::Button::new(tr(key)).min_size(vec2(w, 0.0))).clicked() {
                            action = Some(k);
                        }
                    }
                });
            });
            widgets::form(ui, "links_info", |ui| {
                let b = &self.wb.externals[d.selected];
                widgets::form_row(ui, tr("links.location"), |ui| {
                    let shown = resolve(&b.target, dir.as_deref()).map_or_else(|| b.target.clone(), |p| p.display().to_string());
                    ui.add(egui::Label::new(shown).truncate())
                });
                widgets::form_row(ui, tr("links.sheets"), |ui| ui.add(egui::Label::new(b.sheets.join(", ")).truncate()));
            });
            ui.label(egui::RichText::new(tr("links.hint")).small().weak());
        });
        match action {
            Some(0) => {
                if self.update_link(d.selected) {
                    self.notices.push(Notice::info(tr("links.updated")));
                }
            }
            Some(1) => {
                let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("links.change_title")).add_filter(tr("dialog.filter_sheets"), apex_io::SHEET_EXTS);
                if let Some(dir) = &dir {
                    dlg = dlg.set_directory(dir);
                }
                let selected = d.selected;
                dlg.pick_file(crate::file_dialog::in_view(move |v: &mut SheetView, p: std::path::PathBuf| {
                    if selected >= v.wb.externals.len() {
                        return;
                    }
                    v.wb.change_external_source(selected, &target_for(&p, dir.as_deref()));
                    if let Some(s) = v.links_dlg.as_mut().and_then(|d| d.status.get_mut(selected)) {
                        *s = Some(true);
                    }
                    v.update_link(selected);
                }));
            }
            Some(2) => {
                if let Some(p) = resolve(&self.wb.externals[d.selected].target, dir.as_deref()) {
                    shell.cmd = Some(AppCmd::OpenPath(p));
                    self.links_dlg = None;
                    return;
                }
            }
            Some(3) => d.confirm_break = true,
            Some(4) => {
                for (i, b) in self.wb.externals.iter().enumerate() {
                    d.status[i] = Some(resolve(&b.target, dir.as_deref()).is_some());
                }
            }
            _ => {}
        }
        if res != DialogResult::None {
            self.focus_grid = true;
            return;
        }
        self.links_dlg = Some(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_links_next_to_the_workbook() {
        let dir = apex_fs::temp_dir().join(format!("apex_links_{}", apex_fs::process_id()));
        apex_fs::create_dir_all(dir.join("sub")).unwrap();
        let f = dir.join("sub").join("Budget 1.xlsx");
        apex_fs::write(&f, b"x").unwrap();
        assert_eq!(resolve("sub/Budget 1.xlsx", Some(&dir)), Some(f.clone()));
        assert_eq!(resolve(&format!("file://{}", f.display()).replace(' ', "%20"), None), Some(f.clone()));
        assert_eq!(resolve("C:\\Users\\u\\Budget 1.xlsx", Some(&dir.join("sub"))), Some(f.clone()));
        assert_eq!(resolve("Missing.xlsx", Some(&dir)), None);
        assert_eq!(target_for(&f, Some(&dir)), "sub/Budget 1.xlsx");
        apex_fs::remove_dir_all(&dir).unwrap();
    }
}
