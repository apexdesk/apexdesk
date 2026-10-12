//! Spreadsheet toolbar, menus and context menus.

use super::*;

/// Number format presets: (label key, code builder).
pub(super) fn presets(loc: &apex_sheet::Locale) -> Vec<(&'static str, String)> {
    let currency = currency_code(loc.currency, loc.currency_after, 2);
    vec![
        ("numfmt.general", "General".into()),
        ("numfmt.number", "0.00".into()),
        ("numfmt.thousands", "#,##0.00".into()),
        ("numfmt.currency", currency),
        ("numfmt.accounting", apex_sheet::accounting_code(loc.currency, loc.currency_after, 2)),
        ("numfmt.percent", "0%".into()),
        ("numfmt.percent2", "0.00%".into()),
        ("numfmt.scientific", "0.00E+00".into()),
        ("numfmt.fraction", "# ?/?".into()),
        ("numfmt.date", loc.short_date_code().into()),
        ("numfmt.long_date", "d mmmm yyyy".into()),
        ("numfmt.time", "hh:mm:ss".into()),
        ("numfmt.datetime", format!("{} hh:mm", loc.short_date_code())),
        ("numfmt.text", "@".into()),
    ]
}

pub(super) fn currency_code(symbol: &str, after: bool, decimals: usize) -> String {
    let dec = if decimals > 0 { format!(".{}", "0".repeat(decimals)) } else { String::new() };
    if after { format!("#,##0{dec} \"{symbol}\"") } else { format!("\"{symbol}\"#,##0{dec}") }
}

impl SheetView {
    pub(super) fn paste_menu(&mut self, ui: &mut egui::Ui) {
        let items = [
            ("edit.paste", PasteWhat::All, false),
            ("paste.values", PasteWhat::Values, false),
            ("paste.formulas", PasteWhat::Formulas, false),
            ("paste.formats", PasteWhat::Formats, false),
            ("paste.no_borders", PasteWhat::AllExceptBorders, false),
            ("paste.transpose", PasteWhat::All, true),
            ("paste.col_widths", PasteWhat::ColumnWidths, false),
        ];
        for (key, what, transpose) in items {
            if ui.button(tr(key)).clicked() {
                if key == "edit.paste" {
                    self.request_paste(ui.ctx());
                } else {
                    self.paste_special_now(PasteSpecial { what, transpose, ..Default::default() });
                }
                ui.close();
            }
        }
        if ui.button(tr("paste.link")).clicked() {
            self.paste_link();
            ui.close();
        }
        if ui.button(tr("paste.picture")).clicked() {
            self.paste_picture();
            ui.close();
        }
        ui.separator();
        if ui.add(egui::Button::new(format!("{}…", tr("paste.special"))).shortcut_text("Ctrl+Shift+V")).clicked() {
            self.dialog = Some(Dialog::PasteSpecial(PasteSpecial::default()));
            ui.close();
        }
        if ui.button(format!("{}…", tr("clipboard.title"))).clicked() {
            self.clip_pane = true;
            ui.close();
        }
    }

    /// The Clipboard pane.
    pub(super) fn clipboard_pane(&mut self, ctx: &egui::Context) {
        use crate::clip_history::Action;
        if !self.clip_pane {
            return;
        }
        let mut open = true;
        let action = crate::clip_history::pane(ctx, egui::Id::new(("clip-pane", self.uid)), &mut open, false);
        self.clip_pane = open;
        if let Some(Action::Paste(e)) = action {
            ctx.copy_text(e.text.clone());
            match &e.rich {
                crate::clip_history::Rich::Shape(s) => self.paste_shape((**s).clone()),
                _ => self.paste(&e.text),
            }
            self.focus_grid = true;
        }
    }

    /// The borders menu; returns the preset to apply. Line style and color
    /// are chosen in the same menu and kept for the next borders.
    pub(super) fn borders_menu(&mut self, ui: &mut egui::Ui) -> Option<BorderPreset> {
        let mut out = None;
        let groups: [&[BorderPreset]; 4] = [
            &[BorderPreset::Bottom, BorderPreset::Top, BorderPreset::Left, BorderPreset::Right],
            &[BorderPreset::None, BorderPreset::All, BorderPreset::Outside, BorderPreset::ThickBox],
            &[BorderPreset::Inside, BorderPreset::InsideH, BorderPreset::InsideV],
            &[BorderPreset::DoubleBottom, BorderPreset::TopBottom],
        ];
        for (i, g) in groups.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            for &p in *g {
                // Room on the left for the icon painted over the button.
                let mut job = LayoutJob::default();
                job.append(tr(border_key(p)), 22.0, TextFormat { font_id: egui::TextStyle::Button.resolve(ui.style()), color: ui.visuals().text_color(), ..Default::default() });
                let resp = ui.add(egui::Button::new(job).min_size(vec2(230.0, 0.0)));
                let rect = resp.rect;
                paint_border_icon(ui.painter(), Rect::from_min_size(rect.min + vec2(6.0, (rect.height() - 14.0) / 2.0), vec2(14.0, 14.0)), p, ui.visuals().text_color());
                if resp.clicked() {
                    out = Some(p);
                }
            }
        }
        ui.separator();
        crate::widgets::menu_button(ui, tr("border.line_style"), |ui| {
            for s in [LineStyle::Thin, LineStyle::Medium, LineStyle::Thick, LineStyle::Dashed, LineStyle::Dotted, LineStyle::Double, LineStyle::Hair] {
                let (rect, resp) = ui.allocate_exact_size(vec2(120.0, 18.0), Sense::click());
                if resp.hovered() || self.border_line.style == s {
                    ui.painter().rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let y = rect.center().y;
                paint_line(ui.painter(), pos2(rect.min.x + 8.0, y), pos2(rect.max.x - 8.0, y), Line { style: s, color: None }, ui.visuals().text_color(), true);
                if resp.clicked() {
                    self.border_line.style = s;
                    ui.close();
                }
            }
        });
        crate::widgets::menu_button(ui, tr("border.line_color"), |ui| {
            if let Some(c) = widgets::color_menu(ui, tr("format.auto_color"), self.border_line.color) {
                self.border_line.color = c;
            }
        });
        ui.separator();
        if ui.button(format!("{}…", tr("border.more"))).clicked() {
            self.open_format_dialog(3);
            ui.close();
        }
        if out.is_some() {
            ui.close();
        }
        out
    }

    /// The number format list, each entry previewed with the current value.
    pub(super) fn number_format_list(&mut self, ui: &mut egui::Ui) {
        let loc = self.wb.locale;
        let cur = self.style_at_cursor().num_fmt;
        let value = match self.sh().value(self.cursor) {
            Value::Number(n) => *n,
            _ => 1234.5,
        };
        ui.set_min_width(260.0);
        for (key, code) in presets(&loc) {
            let preview = if code == "@" { value.to_string() } else { apex_sheet::NumFormat::parse(&code).format_number(value, &loc).0 };
            let text = RichText::new(tr(key));
            let r = ui.add(egui::Button::selectable(cur.as_ref() == code, text).right_text(RichText::new(preview).weak()).min_size(vec2(260.0, 0.0)));
            if r.clicked() {
                self.set_num_fmt(&code);
                ui.close();
            }
        }
        ui.separator();
        if ui.add(egui::Button::new(format!("{}…", tr("numfmt.more"))).shortcut_text("Ctrl+1")).clicked() {
            self.open_format_dialog(0);
            ui.close();
        }
    }

    pub(super) fn filter_menu(&mut self, ui: &mut egui::Ui) {
        let f = self.sh().filter.as_ref().map(|f| !f.cols.is_empty());
        if ui.add(egui::Button::selectable(f.is_some(), tr("filter.title")).shortcut_text(tr("key.filter"))).clicked() {
            self.toggle_filter();
            ui.close();
        }
        if ui.add_enabled(f == Some(true), egui::Button::new(tr("filter.clear"))).clicked() {
            self.clear_filters();
            ui.close();
        }
        if ui.add_enabled(f.is_some(), egui::Button::new(tr("filter.reapply"))).clicked() {
            self.reapply_filter();
            ui.close();
        }
    }

    pub(super) fn validation_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button(format!("{}…", tr("val.title"))).clicked() {
            self.open_validation();
            ui.close();
        }
        if ui.add_enabled(!self.sh().validations.is_empty(), egui::Button::new(tr("val.circle"))).clicked() {
            self.data.circles = true;
            let n = self.wb.invalid_cells(self.sheet).len();
            if n == 0 {
                self.notices.push(Notice::info(tr("val.all_valid")));
            }
            ui.close();
        }
        if ui.add_enabled(self.data.circles, egui::Button::new(tr("val.clear_circles"))).clicked() {
            self.data.circles = false;
            ui.close();
        }
    }

    pub(super) fn fill_menu(&mut self, ui: &mut egui::Ui) {
        for (dir, key, keys) in [(FillDir::Down, "edit.fill_down", "Ctrl+D"), (FillDir::Right, "edit.fill_right", "Ctrl+R"), (FillDir::Up, "fill.up", ""), (FillDir::Left, "fill.left", "")] {
            if ui.add(egui::Button::new(tr(key)).shortcut_text(keys)).clicked() {
                self.fill(dir);
                ui.close();
            }
        }
        ui.separator();
        if ui.button(format!("{}…", tr("fill.series"))).clicked() {
            self.open_series();
            ui.close();
        }
    }

    pub(super) fn clear_menu(&mut self, ui: &mut egui::Ui) {
        for (k, key) in [(ClearKind::All, "clear.all"), (ClearKind::Formats, "clear.formats"), (ClearKind::Contents, "clear.contents"), (ClearKind::Notes, "clear.notes"), (ClearKind::Links, "clear.links")] {
            if ui.button(tr(key)).clicked() {
                self.clear(k);
                ui.close();
            }
        }
    }

    pub(super) fn insert_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button(tr("sheet.insert_rows_above")).clicked() {
            self.insert_rows(false);
            ui.close();
        }
        if ui.button(tr("sheet.insert_rows_below")).clicked() {
            self.insert_rows(true);
            ui.close();
        }
        if ui.button(tr("sheet.insert_cols_left")).clicked() {
            self.insert_cols(false);
            ui.close();
        }
        if ui.button(tr("sheet.insert_cols_right")).clicked() {
            self.insert_cols(true);
            ui.close();
        }
        if ui.add(egui::Button::new(format!("{}…", tr("cells.insert_cells"))).shortcut_text("Ctrl+Shift++")).clicked() {
            self.dialog = Some(Dialog::Cells { delete: false, choice: 1 });
            ui.close();
        }
        ui.separator();
        if ui.button(tr("sheet.add_sheet")).clicked() {
            self.add_sheet();
            ui.close();
        }
    }

    pub(super) fn delete_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button(tr("sheet.delete_rows")).clicked() {
            self.delete_rows();
            ui.close();
        }
        if ui.button(tr("sheet.delete_cols")).clicked() {
            self.delete_cols();
            ui.close();
        }
        if ui.add(egui::Button::new(format!("{}…", tr("cells.delete_cells"))).shortcut_text("Ctrl+-")).clicked() {
            self.dialog = Some(Dialog::Cells { delete: true, choice: 1 });
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(self.visible_sheets() > 1, egui::Button::new(tr("sheet.delete_sheet"))).clicked() {
            self.remove_sheet(self.sheet);
            ui.close();
        }
    }

    pub(super) fn cells_format_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button(format!("{}…", tr("size.row_height"))).clicked() {
            self.open_size(true);
            ui.close();
        }
        if ui.button(tr("size.autofit_rows")).clicked() {
            self.autofit_rows(ui);
            ui.close();
        }
        if ui.button(format!("{}…", tr("size.col_width"))).clicked() {
            self.open_size(false);
            ui.close();
        }
        if ui.button(tr("size.autofit_cols")).clicked() {
            let s = self.sel();
            self.autofit_cols(ui, s.start.col, s.end.col);
            ui.close();
        }
        ui.separator();
        crate::widgets::menu_button(ui, tr("hide.title"), |ui| self.hide_menu(ui));
        crate::widgets::menu_button(ui, tr("freeze.title"), |ui| self.freeze_menu(ui));
        if ui.button(tr("sheet.move_copy")).clicked() {
            self.open_move_copy(self.sheet);
            ui.close();
        }
        ui.separator();
        if ui.add(egui::Button::new(format!("{}…", tr("format.cells"))).shortcut_text("Ctrl+1")).clicked() {
            self.open_format_dialog(0);
            ui.close();
        }
    }

    pub(super) fn hide_menu(&mut self, ui: &mut egui::Ui) {
        let s = self.sel();
        let items = [("hide.rows", true, true), ("hide.show_rows", true, false), ("hide.cols", false, true), ("hide.show_cols", false, false)];
        for (key, rows, hide) in items {
            if ui.button(tr(key)).clicked() {
                self.hide(rows, hide, &s);
                ui.close();
            }
        }
        ui.separator();
        if ui.add_enabled(self.visible_sheets() > 1, egui::Button::new(tr("sheet.hide"))).clicked() {
            self.hide_sheet(self.sheet);
            ui.close();
        }
        let hidden: Vec<(usize, String)> = self.wb.sheets.iter().enumerate().filter(|(_, s)| s.hidden).map(|(i, s)| (i, s.name.clone())).collect();
        ui.add_enabled_ui(!hidden.is_empty(), |ui| {
            crate::widgets::menu_button(ui, tr("sheet.unhide"), |ui| {
                for (i, name) in hidden {
                    if ui.button(name).clicked() {
                        self.unhide_sheet(i);
                        ui.close();
                    }
                }
            });
        });
    }

    pub(super) fn freeze_menu(&mut self, ui: &mut egui::Ui) {
        let frozen = self.frozen();
        if ui.button(if frozen { tr("sheet.unfreeze") } else { tr("sheet.freeze") }).clicked() {
            self.toggle_freeze();
            ui.close();
        }
        for (key, row, col) in [("freeze.top_row", true, false), ("freeze.first_col", false, true), ("freeze.top_row_first_col", true, true)] {
            if ui.button(tr(key)).clicked() {
                self.freeze_top(row, col);
                ui.close();
            }
        }
    }

    // --- context menus ------------------------------------------------------------------

    pub(super) fn context_menu(&mut self, ui: &mut egui::Ui) {
        if self.image_menu_open() {
            self.image_context_menu(ui);
            return;
        }
        if ui.add(egui::Button::new(tr("edit.cut")).shortcut_text("Ctrl+X")).clicked() {
            self.copy(ui.ctx(), true);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("edit.copy")).shortcut_text("Ctrl+C")).clicked() {
            self.copy(ui.ctx(), false);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("edit.paste")).shortcut_text("Ctrl+V")).clicked() {
            self.request_paste(ui.ctx());
            ui.close();
        }
        crate::widgets::menu_button(ui, tr("paste.menu"), |ui| self.paste_menu(ui));
        ui.separator();
        match self.ctx_target {
            Hit::ColHeader(_) => {
                if ui.button(tr("sheet.insert_cols_left")).clicked() {
                    self.insert_cols(false);
                    ui.close();
                }
                if ui.button(tr("sheet.insert_cols_right")).clicked() {
                    self.insert_cols(true);
                    ui.close();
                }
                if ui.button(tr("sheet.delete_cols")).clicked() {
                    self.delete_cols();
                    ui.close();
                }
                ui.separator();
                if ui.button(format!("{}…", tr("size.col_width"))).clicked() {
                    self.open_size(false);
                    ui.close();
                }
                if ui.button(tr("size.autofit_cols")).clicked() {
                    let s = self.sel();
                    self.autofit_cols(ui, s.start.col, s.end.col);
                    ui.close();
                }
                let s = self.sel();
                if ui.button(tr("hide.cols")).clicked() {
                    self.hide(false, true, &s);
                    ui.close();
                }
                if ui.button(tr("hide.show_cols")).clicked() {
                    self.hide(false, false, &s);
                    ui.close();
                }
            }
            Hit::RowHeader(_) => {
                if ui.button(tr("sheet.insert_rows_above")).clicked() {
                    self.insert_rows(false);
                    ui.close();
                }
                if ui.button(tr("sheet.insert_rows_below")).clicked() {
                    self.insert_rows(true);
                    ui.close();
                }
                if ui.button(tr("sheet.delete_rows")).clicked() {
                    self.delete_rows();
                    ui.close();
                }
                ui.separator();
                if ui.button(format!("{}…", tr("size.row_height"))).clicked() {
                    self.open_size(true);
                    ui.close();
                }
                if ui.button(tr("size.autofit_rows")).clicked() {
                    self.autofit_rows(ui);
                    ui.close();
                }
                let s = self.sel();
                if ui.button(tr("hide.rows")).clicked() {
                    self.hide(true, true, &s);
                    ui.close();
                }
                if ui.button(tr("hide.show_rows")).clicked() {
                    self.hide(true, false, &s);
                    ui.close();
                }
            }
            _ => {
                if !self.sel().is_single() && ui.button(tr("quick.title")).clicked() {
                    self.quick.open = true;
                    ui.close();
                }
                crate::widgets::menu_button(ui, tr("cells.insert"), |ui| self.insert_menu(ui));
                crate::widgets::menu_button(ui, tr("cells.delete"), |ui| self.delete_menu(ui));
                crate::widgets::menu_button(ui, tr("clear.title"), |ui| self.clear_menu(ui));
                self.f10_cell_menu(ui);
                ui.separator();
                if ui.button(tr("sheet.sort_asc")).clicked() {
                    self.sort(true);
                    ui.close();
                }
                if ui.button(tr("sheet.sort_desc")).clicked() {
                    self.sort(false);
                    ui.close();
                }
                if ui.button(format!("{}…", tr("sort.custom"))).clicked() {
                    self.open_sort();
                    ui.close();
                }
                if ui.button(tr("filter.by_cell")).clicked() {
                    self.filter_by_cell();
                    ui.close();
                }
                if ui.add(egui::Button::new(format!("{}…", tr("filter.pick"))).shortcut_text(tr("key.alt_down"))).clicked() {
                    self.open_list();
                    ui.close();
                }
                ui.separator();
                let merged = self.sh().merges.iter().any(|m| m.contains(self.cursor));
                if merged {
                    if ui.button(tr("merge.unmerge")).clicked() {
                        self.merge_cmd(3, false);
                        ui.close();
                    }
                } else if !self.sel().is_single() && ui.button(tr("merge.center")).clicked() {
                    self.merge_cmd(0, false);
                    ui.close();
                }
                if ui.add(egui::Button::new(format!("{}…", tr("format.cells"))).shortcut_text("Ctrl+1")).clicked() {
                    self.open_format_dialog(0);
                    ui.close();
                }
                if self.cursor_has_picture() && ui.button(tr("image.place_over_cells")).on_hover_text(tr("image.place_over_cells_tip")).clicked() {
                    self.place_over_cells();
                    ui.close();
                }
                ui.separator();
                let has = self.note_at(self.cursor).is_some();
                if ui.button(if has { tr("note.edit") } else { tr("note.new") }).clicked() {
                    self.edit_note();
                    ui.close();
                }
                if has {
                    if ui.button(tr("note.delete")).clicked() {
                        self.delete_notes();
                        ui.close();
                    }
                    let shown = self.note_at(self.cursor).is_some_and(|n| n.visible);
                    if ui.button(if shown { tr("note.hide") } else { tr("note.show") }).clicked() {
                        self.toggle_note_shown();
                        ui.close();
                    }
                }
                let link = self.link_at(self.cursor).is_some();
                if ui.button(format!("{}…", if link { tr("link.edit_title") } else { tr("doc.link") })).clicked() {
                    self.open_link_dialog();
                    ui.close();
                }
                if link {
                    if ui.button(tr("link.open")).clicked() {
                        let c = self.cursor;
                        self.follow_link(c);
                        ui.close();
                    }
                    if ui.button(tr("link.remove_menu")).clicked() {
                        self.remove_links();
                        ui.close();
                    }
                }
            }
        }
    }

    pub(super) fn tab_menu(&mut self, ui: &mut egui::Ui, i: usize) -> Option<TabAction> {
        let mut out = None;
        if ui.button(tr("sheet.add_sheet")).clicked() {
            out = Some(TabAction::Add);
        }
        if ui.button(tr("sheet.duplicate")).clicked() {
            out = Some(TabAction::Duplicate(i));
        }
        if ui.button(tr("sheet.move_copy")).clicked() {
            out = Some(TabAction::MoveCopy(i));
        }
        if ui.button(tr("sheet.rename")).clicked() {
            out = Some(TabAction::Rename(i));
        }
        let grouped = self.wb.group.len() > 1;
        if ui.button(tr(if grouped { "sheet.ungroup" } else { "sheet.select_all" })).on_hover_text(tr("sheet.group_tip")).clicked() {
            self.group_all(!grouped);
            ui.close();
        }
        if ui.add_enabled(self.visible_sheets() > 1, egui::Button::new(tr("sheet.delete_sheet"))).clicked() {
            out = Some(TabAction::Delete(i));
        }
        ui.separator();
        crate::widgets::menu_button(ui, tr("sheet.tab_color"), |ui| {
            if let Some(c) = widgets::color_menu(ui, tr("sheet.no_color"), self.wb.sheets[i].tab_color) {
                out = Some(TabAction::Color(i, c));
            }
        });
        ui.separator();
        let key = if self.wb.is_protected(i) { "protect.unprotect_sheet" } else { "protect.sheet_title" };
        if ui.button(format!("{}…", tr(key))).clicked() {
            self.protect_cmd(Some(i));
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(self.visible_sheets() > 1, egui::Button::new(tr("sheet.hide"))).clicked() {
            out = Some(TabAction::Hide(i));
        }
        let hidden: Vec<(usize, String)> = self.wb.sheets.iter().enumerate().filter(|(_, s)| s.hidden).map(|(i, s)| (i, s.name.clone())).collect();
        ui.add_enabled_ui(!hidden.is_empty(), |ui| {
            crate::widgets::menu_button(ui, tr("sheet.unhide"), |ui| {
                for (k, name) in hidden {
                    if ui.button(name).clicked() {
                        out = Some(TabAction::Unhide(k));
                    }
                }
            });
        });
        if out.is_some() {
            ui.close();
        }
        out
    }
}

pub(super) fn border_key(p: BorderPreset) -> &'static str {
    match p {
        BorderPreset::None => "border.none",
        BorderPreset::All => "border.all",
        BorderPreset::Outside => "border.outer",
        BorderPreset::Inside => "border.inside",
        BorderPreset::InsideH => "border.inside_h",
        BorderPreset::InsideV => "border.inside_v",
        BorderPreset::Left => "border.left",
        BorderPreset::Right => "border.right",
        BorderPreset::Top => "border.top",
        BorderPreset::Bottom => "border.bottom",
        BorderPreset::TopBottom => "border.top_bottom",
        BorderPreset::ThickBox => "border.thick_box",
        BorderPreset::DoubleBottom => "border.double_bottom",
    }
}

pub(super) fn currency_name(s: &str) -> &'static str {
    match s {
        "€" => "currency.eur",
        "$" => "currency.usd",
        "£" => "currency.gbp",
        "¥" => "currency.jpy",
        _ => "currency.chf",
    }
}

/// Small picture of a border preset: a 2×2 grid with the preset's lines.
fn paint_border_icon(p: &egui::Painter, r: Rect, preset: BorderPreset, color: Color32) {
    let faint = Stroke::new(1.0, color.gamma_multiply(0.3));
    let (x0, x1, y0, y1) = (r.min.x, r.max.x, r.min.y, r.max.y);
    let (xm, ym) = (r.center().x, r.center().y);
    for x in [x0, xm, x1] {
        p.line_segment([pos2(x, y0), pos2(x, y1)], faint);
    }
    for y in [y0, ym, y1] {
        p.line_segment([pos2(x0, y), pos2(x1, y)], faint);
    }
    let s = Stroke::new(1.5, color);
    let thick = Stroke::new(2.5, color);
    let h = |y: f32, st: Stroke| p.line_segment([pos2(x0, y), pos2(x1, y)], st);
    let v = |x: f32, st: Stroke| p.line_segment([pos2(x, y0), pos2(x, y1)], st);
    match preset {
        BorderPreset::None => {}
        BorderPreset::All => {
            for y in [y0, ym, y1] {
                h(y, s);
            }
            for x in [x0, xm, x1] {
                v(x, s);
            }
        }
        BorderPreset::Outside | BorderPreset::ThickBox => {
            let st = if preset == BorderPreset::ThickBox { thick } else { s };
            h(y0, st);
            h(y1, st);
            v(x0, st);
            v(x1, st);
        }
        BorderPreset::Inside => {
            h(ym, s);
            v(xm, s);
        }
        BorderPreset::InsideH => {
            h(ym, s);
        }
        BorderPreset::InsideV => {
            v(xm, s);
        }
        BorderPreset::Left => {
            v(x0, s);
        }
        BorderPreset::Right => {
            v(x1, s);
        }
        BorderPreset::Top => {
            h(y0, s);
        }
        BorderPreset::Bottom => {
            h(y1, s);
        }
        BorderPreset::TopBottom => {
            h(y0, s);
            h(y1, s);
        }
        BorderPreset::DoubleBottom => {
            h(y1, Stroke::new(1.0, color));
            h(y1 - 3.0, Stroke::new(1.0, color));
        }
    }
}
