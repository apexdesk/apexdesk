//! Chart templates (.crtx): Save as Template, and charts made from or
//! changed to a template.

use std::path::{Path, PathBuf};

use apex_sheet::chart::{ChartType, Grouping};

use super::*;

/// Where Save as Template puts templates and the menus look for them.
fn dir() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("templates").join("charts"))
}

/// The saved templates: (name, path), by name.
fn templates() -> Vec<(String, PathBuf)> {
    let Some(rd) = dir().and_then(|d| std::fs::read_dir(d).ok()) else { return Vec::new() };
    let mut v: Vec<(String, PathBuf)> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("crtx")))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), p)))
        .collect();
    v.sort_by_key(|(n, _)| n.to_lowercase());
    v
}

impl SheetView {
    /// Chart > Save as Picture: a PNG or PDF file of the selected chart.
    pub(super) fn save_chart_picture(&mut self, ctx: &egui::Context) {
        use crate::chart_picture::{Format, picture};
        let Some(c) = self.selected_chart().cloned() else { return };
        let name = if c.name.is_empty() { tr("chart.picture_default").to_string() } else { c.name.clone() };
        let dlg = crate::file_dialog::FileDialog::new()
            .set_title(tr("chart.save_picture"))
            .add_filter(tr("chart.picture_png"), &["png"])
            .add_filter(tr("chart.picture_pdf"), &["pdf"])
            .set_file_name(format!("{name}.png"));
        let ctx = ctx.clone();
        dlg.save_file(crate::file_dialog::in_view(move |v: &mut SheetView, mut path: PathBuf| {
            let format = Format::from_path(&path).unwrap_or_else(|| {
                path.set_extension("png");
                Format::Png
            });
            let (data, _) = v.chart_data(&c, true);
            let result = picture(&ctx, &c, &data, &v.wb.locale, format).ok_or_else(|| String::from("encoding")).and_then(|b| crate::common::write_out(&path, b).map_err(|e| e.to_string()));
            if let Err(e) = result {
                v.notices.push(Notice::error(format!("{}: {e}", tr("chart.picture_error"))));
            }
        }));
    }

    /// Chart > Save as Template.
    pub(super) fn save_chart_template(&mut self) {
        let Some(c) = self.selected_chart().cloned() else { return };
        let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("chart.save_template")).add_filter(tr("chart.template_filter"), &["crtx"]).set_file_name(format!("{}.crtx", tr("chart.template_default")));
        if let Some(d) = dir() {
            let _ = std::fs::create_dir_all(&d);
            dlg = dlg.set_directory(d);
        }
        let sheet = self.sheet;
        dlg.save_file(crate::file_dialog::in_view(move |v: &mut SheetView, mut path: PathBuf| {
            if path.extension().is_none() {
                path.set_extension("crtx");
            }
            let bytes = apex_io::crtx::write(&v.wb, sheet, &c);
            if let Err(e) = crate::common::write_out(&path, bytes) {
                v.notices.push(Notice::error(format!("{}: {e}", tr("chart.template_error"))));
            }
        }));
    }

    /// The saved templates as menu items, and Browse; `change`: apply to the
    /// selected chart rather than insert a new one.
    pub(super) fn chart_template_items(&mut self, ui: &mut egui::Ui, change: bool) {
        let list = templates();
        if list.is_empty() {
            ui.label(RichText::new(tr("chart.no_templates")).weak());
        }
        let mut pick: Option<PathBuf> = None;
        for (name, path) in list {
            if ui.button(name).clicked() {
                pick = Some(path);
                ui.close();
            }
        }
        ui.separator();
        if ui.button(format!("{}…", tr("chart.browse_templates"))).clicked() {
            ui.close();
            let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("chart.templates")).add_filter(tr("chart.template_filter"), &["crtx"]);
            if let Some(d) = dir().filter(|d| d.is_dir()) {
                dlg = dlg.set_directory(d);
            }
            dlg.pick_file(crate::file_dialog::in_view(move |v: &mut SheetView, p: PathBuf| v.use_chart_template(&p, change)));
        }
        if let Some(p) = pick {
            self.use_chart_template(&p, change);
        }
    }

    pub(super) fn use_chart_template(&mut self, path: &Path, change: bool) {
        let t = match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| apex_io::crtx::read(&b).map_err(|e| e.to_string())) {
            Ok(t) => t,
            Err(e) => {
                self.notices.push(Notice::error(format!("{}: {e}", tr("chart.template_error"))));
                return;
            }
        };
        if change {
            if let Some(mut c) = self.selected_chart().cloned() {
                c.apply_template(&t);
                self.wb.set_chart(self.sheet, c);
            }
            return;
        }
        let before = self.images.charts.selected;
        self.insert_chart(ChartType::Column, Grouping::Clustered, 0, false);
        if self.images.charts.selected == before {
            return;
        }
        if let Some(mut c) = self.selected_chart().cloned() {
            c.apply_template(&t);
            self.wb.set_chart(self.sheet, c);
            // One undo step for the new chart.
            self.wb.merge_undo(2, "undo.insert_chart");
        }
    }
}
