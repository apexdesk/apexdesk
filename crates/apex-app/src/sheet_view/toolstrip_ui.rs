//! The spreadsheet's toolstrip: its tabs and their groups.

use super::toolbar::{currency_code, currency_name, presets};
use super::*;
use crate::common::AppCmd;
use crate::shapes;
use crate::toolstrip::{self, Body, TabDef};
use crate::widgets::Face as WFace;

/// Contextual tab colors.
const TABLE_TAB: Color32 = Color32::from_rgb(0x1E, 0x8E, 0x4E);
const OBJECT_TAB: Color32 = Color32::from_rgb(0xB0, 0x5A, 0xC8);

impl SheetView {
    pub fn toolstrip_tabs(&self) -> Vec<TabDef> {
        let mut v = vec![
            TabDef::new("home", "toolstrip.home"),
            TabDef::new("insert", "menu.insert"),
            TabDef::new("draw", "toolstrip.draw"),
            TabDef::new("page_layout", "menu.page_layout"),
            TabDef::new("formulas", "menu.formulas"),
            TabDef::new("data", "menu.data"),
            TabDef::new("review", "menu.review"),
            TabDef::new("view", "menu.view"),
        ];
        if self.selected_chart().is_some() {
            v.push(TabDef::contextual("chart", "toolstrip.chart_design", OBJECT_TAB));
        } else if self.selected_image().is_some() {
            v.push(TabDef::contextual("picture", "toolstrip.picture_format", OBJECT_TAB));
        } else if self.selected_shape().is_some() {
            v.push(TabDef::contextual("shape", "toolstrip.shape_format", OBJECT_TAB));
        } else {
            if self.cursor_table().is_some() {
                v.push(TabDef::contextual("table", "xtab.design", TABLE_TAB));
            }
            if self.cursor_plan().is_some() {
                v.push(TabDef::contextual("plan", "plan.tab", TABLE_TAB));
            }
            if self.wb.pivot_at(self.sheet, self.cursor).is_some() {
                v.push(TabDef::contextual("pivot", "pivot.analyze", TABLE_TAB));
            }
        }
        v
    }

    /// The body of toolstrip tab `tab`.
    pub fn toolstrip(&mut self, ui: &mut egui::Ui, tab: &str, _shell: &mut Shell) {
        let id = Id::new(("sheet_toolstrip", tab));
        toolstrip::body(ui, id, theme::SHEET_ACCENT, |b| match tab {
            "insert" => self.insert_tab(b),
            "draw" => self.draw_tab(b),
            "page_layout" => self.page_layout_tab(b),
            "formulas" => self.formulas_tab(b),
            "data" => self.data_tab(b),
            "review" => self.review_tab(b),
            "chart" => self.chart_tab(b),
            "picture" => self.picture_tab(b),
            "shape" => self.shape_tab(b),
            "table" => self.table_tab(b),
            "plan" => self.plan_tab(b),
            "pivot" => self.pivot_tab(b),
            _ => self.home_tab(b),
        });
    }

    // --- Home ---------------------------------------------------------------------------

    fn home_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        let st = self.style_at_cursor();
        b.group("toolstrip.g.undo", Icon::Undo, |ui| {
            toolstrip::column(ui, |ui| {
                let n = common::history_button(ui, false, &self.wb.undo_history());
                for _ in 0..n {
                    self.undo();
                }
                let n = common::history_button(ui, true, &self.wb.redo_history());
                for _ in 0..n {
                    self.redo();
                }
            });
        });
        b.group("toolstrip.g.clipboard", Icon::Paste, |ui| {
            if toolstrip::large_split(ui, Icon::Paste, tr("edit.paste"), &format!("{} (Ctrl+V)", tr("edit.paste")), a, |ui| self.paste_menu(ui)) {
                self.request_paste(ui.ctx());
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Cut), tr("edit.cut"), &format!("{} (Ctrl+X)", tr("edit.cut")), a, false).clicked() {
                    self.copy(ui.ctx(), true);
                }
                if toolstrip::small(ui, Some(Icon::Copy), tr("edit.copy"), &format!("{} (Ctrl+C)", tr("edit.copy")), a, false).clicked() {
                    self.copy(ui.ctx(), false);
                }
                let brush = self.brush.as_ref().map(|b| b.sticky);
                let r = toolstrip::small(ui, Some(Icon::Brush), tr("toolstrip.brush"), tr("toolbar.brush_hint"), a, brush.is_some());
                if r.double_clicked() {
                    self.start_brush(true);
                } else if r.clicked() {
                    if brush.is_some() {
                        self.brush = None;
                    } else {
                        self.start_brush(false);
                    }
                }
            });
        });
        b.group("toolstrip.g.font", Icon::GrowFont, |ui| {
            toolstrip::column(ui, |ui| {
                toolstrip::row(ui, |ui| self.font_row(ui, &st));
                toolstrip::row(ui, |ui| self.text_style_row(ui, &st));
            });
        });
        b.group("toolstrip.g.alignment", Icon::AlignCenter, |ui| self.align_group(ui, &st));
        b.group("toolstrip.g.number", Icon::Currency, |ui| self.number_group(ui, &st));
        b.group("toolstrip.g.styles", Icon::Styles, |ui| {
            toolstrip::large_menu(ui, Icon::CondFormat, tr("toolstrip.cond_format"), tr("cf.title"), a, |ui| self.cf_menu(ui));
            toolstrip::large_menu(ui, Icon::FormatAsTable, tr("xtab.format_as"), "", a, |ui| {
                let current = self.cursor_table().and_then(|id| self.wb.table(id).cloned());
                if let Some(name) = self.style_gallery(ui, current.as_ref()) {
                    self.open_create_table(Some(&name));
                    ui.close();
                }
            });
            toolstrip::large_menu(ui, Icon::CellStyles, tr("cellstyle.menu"), "", a, |ui| self.cell_styles_gallery(ui));
        });
        b.group("toolstrip.g.cells", Icon::InsertRows, |ui| {
            toolstrip::large_menu(ui, Icon::InsertRows, tr("cells.insert"), tr("cells.insert_hint"), a, |ui| self.insert_menu(ui));
            toolstrip::large_menu(ui, Icon::DeleteRows, tr("cells.delete"), tr("cells.delete_hint"), a, |ui| self.delete_menu(ui));
            toolstrip::large_menu(ui, Icon::CellFormat, tr("cells.format"), tr("cells.format_hint"), a, |ui| self.cells_format_menu(ui));
        });
        b.group("toolstrip.g.editing", Icon::Search, |ui| {
            toolstrip::column(ui, |ui| {
                let mut chosen = None;
                let last = self.last_autosum;
                toolstrip::row(ui, |ui| {
                    if toolstrip::split(ui, WFace::Icon(Icon::Sigma), false, &format!("{} (Alt+=)", tr("sheet.autosum")), |ui| {
                        for (f, key) in [("SUM", "autosum.sum"), ("AVERAGE", "autosum.average"), ("COUNT", "autosum.count"), ("MAX", "autosum.max"), ("MIN", "autosum.min")] {
                            if ui.button(tr(key)).clicked() {
                                chosen = Some(f);
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.button(format!("{}…", tr("fx.more"))).clicked() {
                            self.open_functions();
                            ui.close();
                        }
                    }) {
                        chosen = Some(last);
                    }
                    ui.label(RichText::new(tr("sheet.autosum")).size(12.5));
                });
                if let Some(f) = chosen {
                    self.last_autosum = f;
                    self.auto_sum(f);
                }
                toolstrip::small_menu(ui, Some(Icon::FillDown), tr("fill.title"), "", a, |ui| self.fill_menu(ui));
                toolstrip::small_menu(ui, Some(Icon::Eraser), tr("clear.title"), "", a, |ui| self.clear_menu(ui));
            });
            toolstrip::large_menu(ui, Icon::SortAsc, tr("toolstrip.sort_filter"), "", a, |ui| self.sort_filter_menu(ui));
            toolstrip::large_menu(ui, Icon::Search, tr("toolstrip.find_select"), "", a, |ui| self.find_select_menu(ui));
        });
    }

    fn font_row(&mut self, ui: &mut egui::Ui, st: &CellStyle) {
        let cur = st.font.as_deref().unwrap_or(default_font());
        if let Some(f) = common::themed_font_picker(ui, Id::new(("sfont", self.uid)), cur, 120.0) {
            self.set_style(move |s| cellstyles_ui::pick_font(s, &f));
        }
        let size = style_pt(st);
        if let Some(v) = common::size_box(ui, Id::new(("ssize", self.uid)), size) {
            self.set_size(v);
        }
        if toolstrip::tool(ui, WFace::Icon(Icon::GrowFont), false, &format!("{} (Ctrl+Shift+>)", tr("format.grow"))).clicked() {
            self.grow_font(true);
        }
        if toolstrip::tool(ui, WFace::Icon(Icon::ShrinkFont), false, &format!("{} (Ctrl+Shift+<)", tr("format.shrink"))).clicked() {
            self.grow_font(false);
        }
    }

    fn text_style_row(&mut self, ui: &mut egui::Ui, st: &CellStyle) {
        if toolstrip::tool(ui, WFace::Text(RichText::new("B").strong()), st.bold, &format!("{} (Ctrl+B)", tr("format.bold"))).clicked() {
            self.toggle(|s| s.bold, |s, v| s.bold = v);
        }
        if toolstrip::tool(ui, WFace::Text(RichText::new("I").italics()), st.italic, &format!("{} (Ctrl+I)", tr("format.italic"))).clicked() {
            self.toggle(|s| s.italic, |s, v| s.italic = v);
        }
        if toolstrip::tool(ui, WFace::Text(RichText::new("U").underline()), st.underline, &format!("{} (Ctrl+U)", tr("format.underline"))).clicked() {
            self.toggle(|s| s.underline, |s, v| s.underline = v);
        }
        if toolstrip::tool(ui, WFace::Text(RichText::new("S").strikethrough()), st.strike, &format!("{} (Ctrl+5)", tr("format.strike"))).clicked() {
            self.toggle(|s| s.strike, |s, v| s.strike = v);
        }
        ui.add_space(4.0);
        let last = self.last_border;
        let mut chosen = None;
        if toolstrip::split(ui, WFace::Icon(Icon::Borders), false, tr("format.borders"), |ui| {
            chosen = self.borders_menu(ui);
        }) {
            chosen = Some(last);
        }
        if let Some(p) = chosen {
            self.last_border = p;
            self.apply_borders(p);
        }
        let last = self.last_fill;
        let mut chosen = None;
        let bar = last.map_or(Color32::TRANSPARENT, theme::rgb);
        if toolstrip::split(ui, WFace::IconBar(Icon::Fill, bar), false, tr("format.fill_color"), |ui| {
            chosen = widgets::color_menu(ui, tr("format.no_fill"), st.fill);
        }) {
            chosen = Some(last);
        }
        if let Some(c) = chosen {
            self.last_fill = c;
            self.set_style(move |s| {
                s.fill = c;
                cellstyles_ui::link_theme(s);
            });
        }
        let last = self.last_color;
        let mut chosen = None;
        let bar = last.map_or(Paper::TEXT, theme::rgb);
        if toolstrip::split(ui, WFace::TextBar(RichText::new("A").strong(), bar), false, tr("format.text_color"), |ui| {
            chosen = widgets::color_menu(ui, tr("format.auto_color"), st.color);
        }) {
            chosen = Some(last);
        }
        if let Some(c) = chosen {
            self.last_color = c;
            self.set_style(move |s| {
                s.color = c;
                cellstyles_ui::link_theme(s);
            });
        }
    }

    fn align_group(&mut self, ui: &mut egui::Ui, st: &CellStyle) {
        let a = crate::theme::SHEET_ACCENT;
        toolstrip::column(ui, |ui| {
            toolstrip::row(ui, |ui| {
                for (v, icon, key) in [(VAlign::Top, Icon::AlignTop, "format.valign_top"), (VAlign::Center, Icon::AlignMiddle, "format.valign_middle"), (VAlign::Bottom, Icon::AlignBottom, "format.valign_bottom")] {
                    if toolstrip::tool(ui, WFace::Icon(icon), st.valign == v, tr(key)).clicked() {
                        self.set_style(move |s| s.valign = v);
                    }
                }
                ui.add_space(4.0);
                toolstrip::tool_menu(ui, WFace::Icon(Icon::Orientation), st.rotation != 0, tr("rotate.title"), |ui| {
                    for (r, key) in [(45i16, "rotate.ccw"), (-45, "rotate.cw"), (ROTATION_STACKED, "rotate.stacked"), (90, "rotate.up"), (-90, "rotate.down"), (0, "rotate.none")] {
                        if ui.radio(st.rotation == r, tr(key)).clicked() {
                            self.set_style(move |s| s.rotation = r);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(format!("{}…", tr("format.cells"))).clicked() {
                        self.open_format_dialog(1);
                        ui.close();
                    }
                });
            });
            toolstrip::row(ui, |ui| {
                for (h, icon, key) in [(HAlign::Left, Icon::AlignLeft, "format.align_left"), (HAlign::Center, Icon::AlignCenter, "format.align_center"), (HAlign::Right, Icon::AlignRight, "format.align_right")] {
                    if toolstrip::tool(ui, WFace::Icon(icon), st.halign == h, tr(key)).clicked() {
                        let target = if st.halign == h { HAlign::General } else { h };
                        self.set_style(move |s| s.halign = target);
                    }
                }
                ui.add_space(4.0);
                if toolstrip::tool(ui, WFace::Icon(Icon::CellOutdent), false, tr("sheet.outdent")).clicked() {
                    self.set_style(|s| s.indent = s.indent.saturating_sub(1));
                }
                if toolstrip::tool(ui, WFace::Icon(Icon::CellIndent), false, tr("sheet.indent")).clicked() {
                    self.set_style(|s| {
                        s.indent = (s.indent + 1).min(15);
                        if matches!(s.halign, HAlign::General | HAlign::Center | HAlign::Justify) {
                            s.halign = HAlign::Left;
                        }
                    });
                }
            });
        });
        toolstrip::column(ui, |ui| {
            if toolstrip::small(ui, Some(Icon::Wrap), tr("sheet.wrap"), "", a, st.wrap).clicked() {
                self.toggle(|s| s.wrap, |s, v| s.wrap = v);
            }
            let merged = self.sh().merges.iter().any(|m| m.contains(self.cursor));
            toolstrip::small_menu(ui, Some(Icon::Merge), tr("merge.title"), "", a, |ui| {
                for (k, key) in [(0u8, "merge.center"), (2, "merge.across"), (1, "merge.cells"), (3, "merge.unmerge")] {
                    if ui.add_enabled(k != 3 || merged, egui::Button::new(tr(key))).clicked() {
                        self.merge_cmd(k, false);
                        ui.close();
                    }
                }
            });
            if toolstrip::small(ui, Some(Icon::Justify), tr("format.align_justify"), "", a, st.halign == HAlign::Justify).clicked() {
                let target = if st.halign == HAlign::Justify { HAlign::General } else { HAlign::Justify };
                self.set_style(move |s| s.halign = target);
            }
        });
    }

    fn number_group(&mut self, ui: &mut egui::Ui, st: &CellStyle) {
        let loc = self.wb.locale;
        // The format of the cell names the button, as in Excel's list.
        let list = presets(&loc);
        let current = list.iter().find(|(_, c)| *c == *st.num_fmt).map_or(tr("numfmt.custom"), |(k, _)| tr(k));
        toolstrip::large_menu(ui, Icon::CellFormat, current, tr("sheet.number_format"), crate::theme::SHEET_ACCENT, |ui| self.number_format_list(ui));
        toolstrip::column(ui, |ui| {
            toolstrip::row(ui, |ui| {
                let mut chosen = None;
                let sym = loc.currency;
                if toolstrip::split(ui, WFace::Icon(Icon::Currency), false, tr("numfmt.currency"), |ui| {
                    for (s, after) in [("€", true), ("$", false), ("£", false), ("¥", false), ("CHF", true)] {
                        if ui.button(format!("{s}  {}", tr(currency_name(s)))).clicked() {
                            chosen = Some(currency_code(s, after, 2));
                            ui.close();
                        }
                    }
                    ui.separator();
                    for (s, after) in [("€", true), ("$", false)] {
                        if ui.button(format!("{}  {s}", tr("numfmt.accounting"))).clicked() {
                            chosen = Some(apex_sheet::accounting_code(s, after, 2));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(format!("{}…", tr("numfmt.more"))).clicked() {
                        self.open_format_dialog(0);
                        ui.close();
                    }
                }) {
                    chosen = Some(currency_code(sym, loc.currency_after, 2));
                }
                if let Some(code) = chosen {
                    self.set_num_fmt(&code);
                }
                if toolstrip::tool(ui, WFace::Icon(Icon::Percent), false, &format!("{} (Ctrl+Shift+%)", tr("numfmt.percent"))).clicked() {
                    self.set_num_fmt("0%");
                }
                if toolstrip::tool(ui, WFace::Icon(Icon::Thousands), false, tr("numfmt.thousands")).clicked() {
                    self.set_num_fmt("#,##0.00");
                }
            });
            toolstrip::row(ui, |ui| {
                if toolstrip::tool(ui, WFace::Icon(Icon::Decimals), false, tr("sheet.more_decimals")).clicked() {
                    self.set_style(|s| s.num_fmt = Arc::from(change_decimals(&s.num_fmt, 1)));
                }
                if toolstrip::tool(ui, WFace::Icon(Icon::FewerDecimals), false, tr("sheet.fewer_decimals")).clicked() {
                    self.set_style(|s| s.num_fmt = Arc::from(change_decimals(&s.num_fmt, -1)));
                }
                if toolstrip::tool(ui, WFace::Icon(Icon::Calendar), false, tr("numfmt.date")).clicked() {
                    let code = loc.short_date_code().to_string();
                    self.set_num_fmt(&code);
                }
            });
            if toolstrip::small(ui, Some(Icon::Options), &format!("{}…", tr("numfmt.more")), "", crate::theme::SHEET_ACCENT, false).clicked() {
                self.open_format_dialog(0);
            }
        });
    }

    fn sort_filter_menu(&mut self, ui: &mut egui::Ui) {
        if toolstrip::menu_item(ui, Icon::SortAsc, tr("sheet.sort_asc")).clicked() {
            self.sort(true);
            ui.close();
        }
        if toolstrip::menu_item(ui, Icon::SortDesc, tr("sheet.sort_desc")).clicked() {
            self.sort(false);
            ui.close();
        }
        if toolstrip::menu_item(ui, Icon::SortAsc, &format!("{}…", tr("sort.custom"))).clicked() {
            self.open_sort();
            ui.close();
        }
        ui.separator();
        self.filter_menu(ui);
    }

    fn find_select_menu(&mut self, ui: &mut egui::Ui) {
        if toolstrip::menu_item_keys(ui, Icon::Search, tr("edit.find"), "Ctrl+F").clicked() {
            self.open_find(false);
            ui.close();
        }
        if toolstrip::menu_item_keys(ui, Icon::Replace, tr("edit.replace"), "Ctrl+H").clicked() {
            self.open_find(true);
            ui.close();
        }
        if toolstrip::menu_item_keys(ui, Icon::Select, &format!("{}…", tr("goto.title")), "Ctrl+G").clicked() {
            self.dialog = Some(Dialog::GoTo(String::new()));
            ui.close();
        }
        if toolstrip::menu_item(ui, Icon::Select, &format!("{}…", tr("goto.special_title"))).clicked() {
            self.dialog = Some(Dialog::Special(Default::default()));
            ui.close();
        }
        ui.separator();
        use apex_sheet::Special as S;
        let quick = [
            ("goto.formulas", S::Formulas(apex_sheet::workbook::SPECIAL_ALL_TYPES)),
            ("goto.notes", S::Notes),
            ("goto.cf", S::CondFormats { same: false }),
            ("goto.validation", S::Validation { same: false }),
        ];
        for (key, kind) in quick {
            if toolstrip::menu_item(ui, Icon::Select, tr(key)).clicked() {
                self.settle_edit();
                self.goto_special(kind);
                ui.close();
            }
        }
        ui.separator();
        if toolstrip::menu_item_keys(ui, Icon::Select, tr("edit.select_all"), "Ctrl+A").clicked() {
            self.anchor = CellRef::new(0, 0);
            self.cursor = LAST;
            ui.close();
        }
    }

    // --- Insert -------------------------------------------------------------------------

    fn insert_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.tables", Icon::Table, |ui| {
            if toolstrip::large(ui, Icon::Pivot, tr("pivot.insert"), "", a, false).clicked() {
                self.open_pivot_create(None);
            }
            // A table can't go inside another one.
            let free = self.cursor_table().is_none();
            let r = ui.add_enabled_ui(free, |ui| toolstrip::large(ui, Icon::Table, tr("xtab.insert"), &format!("{} (Ctrl+T)", tr("xtab.insert")), a, false)).inner;
            if r.on_disabled_hover_text(tr("xtab.in_table")).clicked() {
                self.open_create_table(None);
            }
        });
        self.plan_insert_group(b);
        b.group("toolstrip.g.illustrations", Icon::Picture, |ui| {
            if toolstrip::large(ui, Icon::Picture, tr("image.insert"), "", a, false).clicked() {
                self.insert_image_dialog();
            }
            if toolstrip::large(ui, Icon::PictureInCell, tr("image.in_cell_file"), tr("image.in_cell_file_tip"), a, false).clicked() {
                self.insert_cell_picture_dialog();
            }
            if toolstrip::large(ui, Icon::Picture, tr("image.place_over_cells"), tr("image.place_over_cells_tip"), a, false).clicked() {
                self.place_over_cells();
            }
            if toolstrip::large(ui, Icon::Object, tr("object.menu"), tr("object.insert"), a, false).clicked() {
                self.show_insert_object();
            }
            if toolstrip::large(ui, Icon::QrCode, tr("barcode.menu"), tr("barcode.tip"), a, false).clicked() {
                self.show_barcode();
            }
            if crate::library::available() && toolstrip::large(ui, Icon::Folder, tr("library.menu"), tr("library.tip"), a, false).clicked() {
                self.show_library();
            }
            toolstrip::large_menu(ui, Icon::Shapes, tr("shape.shapes"), "", a, |ui| {
                if let Some(t) = shapes::gallery(ui) {
                    self.start_drawing(t);
                }
            });
            let mut pick = None;
            toolstrip::large_menu(ui, Icon::Outline, tr("diagram.menu"), tr("diagram.tip"), a, |ui| pick = crate::diagram_ui::gallery(ui, None));
            if let Some(k) = pick {
                self.insert_diagram(k);
            }
        });
        b.group("toolstrip.g.charts", Icon::Chart, |ui| {
            toolstrip::large_menu(ui, Icon::Chart, tr("chart.insert"), "", a, |ui| self.chart_type_items(ui));
            // The common kinds, one click each.
            toolstrip::column(ui, |ui| {
                for row in [[0usize, 4, 14, 20], [8, 11, 17, 26], [33, 31, 28, 29]] {
                    toolstrip::row(ui, |ui| {
                        for i in row {
                            let (key, icon, kind, grouping, flag) = charts_ui::CHART_TYPES[i];
                            if toolstrip::tool(ui, WFace::Icon(icon), false, tr(key)).clicked() {
                                self.insert_chart(kind, grouping, flag, key == "chart.t.combo");
                            }
                        }
                    });
                }
            });
        });
        b.group("toolstrip.g.cell_controls", Icon::Checkbox, |ui| {
            if toolstrip::large(ui, Icon::Checkbox, tr("checkbox.insert"), tr("checkbox.insert_tip"), a, false).clicked() {
                self.insert_checkboxes();
            }
            toolstrip::large_menu(ui, Icon::Control, tr("control.form"), tr("control.form_tip"), a, |ui| self.form_controls_menu(ui));
        });
        b.group("spark.insert", Icon::Sparkline, |ui| {
            self.sparkline_buttons(ui, a);
        });
        b.group("toolstrip.g.links", Icon::Link, |ui| {
            if toolstrip::large(ui, Icon::Link, tr("doc.link"), &format!("{} (Ctrl+K)", tr("doc.link")), a, false).clicked() {
                self.open_link_dialog();
            }
        });
        b.group("toolstrip.g.notes", Icon::Note, |ui| {
            if toolstrip::large(ui, Icon::Note, tr("note.new"), &format!("{} (Shift+F2)", tr("note.new")), a, false).clicked() {
                self.edit_note();
            }
        });
        b.group("toolstrip.g.text", Icon::TextBox, |ui| {
            let armed = self.images.shapes.armed().is_some_and(|t| t.kind == apex_doc::ShapeKind::TextBox);
            if toolstrip::large(ui, Icon::TextBox, tr("shape.text_box"), "", a, armed).clicked() {
                self.start_drawing(shapes::Template { kind: apex_doc::ShapeKind::TextBox, arrow: false, preset: None });
            }
            if toolstrip::large(ui, Icon::HeaderFooter, tr("toolstrip.header_footer"), "", a, false).clicked() {
                self.open_page_setup(2);
            }
        });
        b.group("toolstrip.g.symbols", Icon::Symbol, |ui| {
            if toolstrip::large(ui, Icon::Function, tr("fx.title"), &format!("{} (Shift+F3)", tr("fx.title")), a, false).clicked() {
                self.open_functions();
            }
            if toolstrip::large(ui, Icon::Symbol, tr("symbol.menu"), "", a, false).clicked() {
                self.open_symbols();
            }
        });
    }

    // --- Page Layout --------------------------------------------------------------------

    fn page_layout_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.themes", Icon::Themes, |ui| {
            toolstrip::large_menu(ui, Icon::Themes, tr("theme.themes"), "", a, |ui| self.theme_menus(ui));
        });
        b.group("page.setup", Icon::PageSize, |ui| {
            if toolstrip::large(ui, Icon::Margins, tr("page.tab_margins"), "", a, false).clicked() {
                self.open_page_setup(1);
            }
            toolstrip::large_menu(ui, Icon::PageOrientation, tr("page.orientation"), "", a, |ui| self.orientation_items(ui));
            toolstrip::large_menu(ui, Icon::PageSize, tr("toolstrip.size"), tr("page.paper"), a, |ui| self.paper_items(ui));
            toolstrip::large_menu(ui, Icon::PrintArea, tr("page.print_area"), "", a, |ui| self.print_area_items(ui));
            toolstrip::large_menu(ui, Icon::PageBreak, tr("page.breaks"), "", a, |ui| self.breaks_items(ui));
            if self.sh().page.background.is_some() {
                if toolstrip::large(ui, Icon::Picture, tr("page.delete_background"), tr("page.delete_background_tip"), a, false).clicked() {
                    self.delete_background();
                }
            } else if toolstrip::large(ui, Icon::Picture, tr("page.background"), tr("page.background_tip"), a, false).clicked() {
                self.background_dialog();
            }
            if toolstrip::large(ui, Icon::PrintTitles, tr("page.print_titles"), "", a, false).clicked() {
                self.open_page_setup(3);
            }
        });
        b.group("toolstrip.g.sheet_options", Icon::Gridlines, |ui| {
            toolstrip::column(ui, |ui| self.sheet_print_options(ui));
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::PageSize), &format!("{}…", tr("page.setup")), "", a, false).clicked() {
                    self.open_page_setup(0);
                }
            });
        });
    }

    // --- Formulas -----------------------------------------------------------------------

    fn formulas_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.function_library", Icon::Function, |ui| {
            if toolstrip::large(ui, Icon::Function, tr("fx.title"), &format!("{} (Shift+F3)", tr("fx.title")), a, false).clicked() {
                self.open_functions();
            }
            let mut chosen = None;
            toolstrip::large_menu(ui, Icon::Sigma, tr("sheet.autosum"), "", a, |ui| {
                for (f, key) in [("SUM", "autosum.sum"), ("AVERAGE", "autosum.average"), ("COUNT", "autosum.count"), ("MAX", "autosum.max"), ("MIN", "autosum.min")] {
                    if toolstrip::menu_item(ui, Icon::Sigma, tr(key)).clicked() {
                        chosen = Some(f);
                        ui.close();
                    }
                }
            });
            if let Some(f) = chosen {
                self.last_autosum = f;
                self.auto_sum(f);
            }
        });
        b.group("toolstrip.g.defined_names", Icon::NameManager, |ui| {
            if toolstrip::large(ui, Icon::NameManager, tr("names.manager"), &format!("{} (Ctrl+F3)", tr("names.manager")), a, false).clicked() {
                self.open_name_manager();
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::NameManager), &format!("{}…", tr("names.define")), &tr("names.define_tip"), a, false).clicked() {
                    self.open_define_name();
                }
                toolstrip::small_menu(ui, Some(Icon::Function), tr("names.use_in_formula"), &format!("{} (F3)", tr("names.use_in_formula")), a, |ui| self.use_in_formula_menu(ui));
                if toolstrip::small(ui, Some(Icon::Table), &format!("{}…", tr("names.create_title")), &format!("{} (Ctrl+Shift+F3)", tr("names.create_title")), a, false).clicked() {
                    self.open_create_names();
                }
            });
        });
        b.group("toolstrip.g.auditing", Icon::TracePrecedents, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::TracePrecedents), tr("audit.trace_prec"), "", a, false).clicked() {
                    self.trace(false);
                }
                if toolstrip::small(ui, Some(Icon::TraceDependents), tr("audit.trace_dep"), "", a, false).clicked() {
                    self.trace(true);
                }
                let has = self.has_arrows();
                ui.add_enabled_ui(has, |ui| {
                    if toolstrip::small(ui, Some(Icon::RemoveArrows), tr("audit.remove_arrows"), "", a, false).on_disabled_hover_text(tr("audit.no_arrows")).clicked() {
                        self.remove_arrows();
                    }
                });
            });
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::ShowFormulas), tr("view.show_formulas"), &format!("{} (Ctrl+`)", tr("view.show_formulas")), a, self.show_formulas).clicked() {
                    self.show_formulas = !self.show_formulas;
                }
                if toolstrip::small(ui, Some(Icon::ErrorCheck), &format!("{}…", tr("audit.checker")), "", a, false).clicked() {
                    self.open_checker();
                }
                if toolstrip::small(ui, Some(Icon::Function), &format!("{}…", tr("evalf.title")), tr("evalf.tip"), a, false).clicked() {
                    self.open_evaluate();
                }
            });
            if toolstrip::large(ui, Icon::Checkbox, &format!("{}…", tr("tests.title")), tr("tests.tip"), a, self.audit.tests.open).clicked() {
                self.open_tests();
            }
            if toolstrip::large(ui, Icon::Window, tr("watch.title"), tr("watch.tip"), a, self.audit.watch.open).clicked() {
                self.toggle_watch_window();
            }
            if toolstrip::large(ui, Icon::TracePrecedents, tr("org.button"), tr("org.tip"), a, self.audit.origin.open).clicked() {
                self.open_origin();
            }
        });
        b.group("toolstrip.g.calculation", Icon::Calculate, |ui| {
            if toolstrip::large(ui, Icon::Calculate, tr("audit.calc_now"), &format!("{} (F9)", tr("audit.calc_now")), a, false).clicked() {
                self.calculate_now();
            }
            let manual = self.wb.calc.manual;
            toolstrip::large_menu(ui, Icon::Options, tr("calc.options"), tr("calc.options"), a, |ui| {
                for (m, key) in [(false, "calc.automatic"), (true, "calc.manual")] {
                    if ui.radio(manual == m, tr(key)).clicked() {
                        self.set_calc(apex_sheet::CalcOptions { manual: m, ..self.wb.calc });
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button(tr("calc.iteration")).clicked() {
                    self.dialog = Some(Dialog::Calc(self.wb.calc));
                    ui.close();
                }
            });
        });
    }

    // --- Data ---------------------------------------------------------------------------

    fn data_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.get_data", Icon::Table, |ui| {
            toolstrip::large_menu(ui, Icon::Table, tr("getdata.menu"), tr("getdata.tip"), a, |ui| {
                for (i, (key, icon)) in super::getdata_ui::SOURCES.iter().enumerate() {
                    // Files, the web, databases, then this workbook.
                    if i == 7 || i == 10 {
                        ui.separator();
                    }
                    if toolstrip::menu_item(ui, *icon, &format!("{}…", tr(key))).clicked() {
                        self.new_query(key);
                        ui.close();
                    }
                }
            });
            let any = self.wb.queries.iter().any(|q| q.table.is_some());
            let why = tr(if any { "getdata.busy" } else { "getdata.no_tables" });
            ui.add_enabled_ui(any && !self.getdata_busy(), |ui| {
                if toolstrip::large(ui, Icon::Calculate, tr("getdata.refresh_all"), tr("getdata.refresh_all_tip"), a, false).on_disabled_hover_text(why).clicked() {
                    self.refresh_all_queries();
                }
                toolstrip::column(ui, |ui| {
                    if toolstrip::small(ui, Some(Icon::Calculate), tr("getdata.refresh"), tr("getdata.refresh_tip"), a, false).on_disabled_hover_text(why).clicked() {
                        self.refresh_query_here();
                    }
                });
            });
        });
        b.group("toolstrip.g.sort_filter", Icon::Filter, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::tool(ui, WFace::Icon(Icon::SortAsc), false, tr("sheet.sort_asc")).clicked() {
                    self.sort(true);
                }
                if toolstrip::tool(ui, WFace::Icon(Icon::SortDesc), false, tr("sheet.sort_desc")).clicked() {
                    self.sort(false);
                }
            });
            if toolstrip::large(ui, Icon::SortAsc, tr("toolstrip.sort"), &format!("{}…", tr("sort.custom")), a, false).clicked() {
                self.open_sort();
            }
            let on = self.sh().filter.is_some();
            if toolstrip::large(ui, Icon::Filter, tr("filter.title"), &format!("{} ({})", tr("filter.title"), tr("key.filter")), a, on).clicked() {
                self.toggle_filter();
            }
            toolstrip::column(ui, |ui| {
                let f = self.sh().filter.as_ref().map(|f| !f.cols.is_empty());
                ui.add_enabled_ui(f == Some(true) || self.data.advanced.is_some(), |ui| {
                    if toolstrip::small(ui, Some(Icon::Eraser), tr("filter.clear"), "", a, false).on_disabled_hover_text(tr("filter.nothing_filtered")).clicked() {
                        self.clear_filters();
                    }
                });
                ui.add_enabled_ui(f.is_some(), |ui| {
                    if toolstrip::small(ui, Some(Icon::Filter), tr("filter.reapply"), "", a, false).on_disabled_hover_text(tr("filter.no_filter")).clicked() {
                        self.reapply_filter();
                    }
                });
                if toolstrip::small(ui, Some(Icon::Filter), &format!("{}…", tr("adv.short")), tr("adv.title"), a, false).clicked() {
                    self.open_advanced_filter();
                }
            });
        });
        b.group("toolstrip.g.data_tools", Icon::TextToColumns, |ui| {
            if toolstrip::large(ui, Icon::TextToColumns, tr("textcols.title"), "", a, false).clicked() {
                self.open_text_cols();
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::FillDown), tr("flash.title"), &format!("{} (Ctrl+E)", tr("flash.title")), a, false).clicked() {
                    self.flash_fill_cmd();
                }
                if toolstrip::small(ui, Some(Icon::RemoveDuplicates), tr("dups.title"), "", a, false).clicked() {
                    self.open_remove_dups();
                }
                toolstrip::small_menu(ui, Some(Icon::Validation), tr("val.menu"), "", a, |ui| self.validation_menu(ui));
            });
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Subtotal), &format!("{}…", tr("cons.title")), "", a, false).clicked() {
                    self.open_consolidate();
                }
                if toolstrip::small(ui, Some(Icon::CellFormat), &format!("{}…", tr("form.button")), tr("form.tip"), a, false).clicked() {
                    self.open_data_form();
                }
                if crate::license::has("management") && toolstrip::small(ui, Some(Icon::Accept), &format!("{}…", tr("bank.title")), tr("bank.tip"), a, false).clicked() {
                    self.open_bank_rec();
                }
            });
        });
        b.group("toolstrip.g.forecast", Icon::Scenario, |ui| {
            toolstrip::large_menu(ui, Icon::Scenario, tr("whatif.menu"), "", a, |ui| {
                if toolstrip::menu_item(ui, Icon::Scenario, &format!("{}…", tr("scenario.manager"))).clicked() {
                    self.open_scenarios();
                    ui.close();
                }
                if toolstrip::menu_item(ui, Icon::GoalSeek, &format!("{}…", tr("goal.title"))).clicked() {
                    self.open_goal_seek();
                    ui.close();
                }
                if toolstrip::menu_item(ui, Icon::Table, &format!("{}…", tr("dtable.title"))).clicked() {
                    self.open_data_table();
                    ui.close();
                }
            });
            if toolstrip::large(ui, Icon::Solver, tr("solver.title"), "", a, false).clicked() {
                self.open_solver();
            }
            if toolstrip::large(ui, Icon::ChartHistogram, tr("tools.title"), "", a, false).clicked() {
                self.open_data_analysis();
            }
            if toolstrip::large(ui, Icon::ChartLine, tr("forecast.button"), tr("forecast.tip"), a, false).clicked() {
                self.open_forecast();
            }
        });
        b.group("toolstrip.g.outline", Icon::Group, |ui| {
            if toolstrip::large(ui, Icon::Group, tr("group.group"), &format!("{} ({})", tr("group.group"), tr("key.group")), a, false).clicked() {
                self.group_cmd(false);
            }
            if toolstrip::large(ui, Icon::Ungroup, tr("group.ungroup"), &format!("{} ({})", tr("group.ungroup"), tr("key.ungroup")), a, false).clicked() {
                self.group_cmd(true);
            }
            if toolstrip::large(ui, Icon::Subtotal, tr("subtotal.title"), "", a, false).clicked() {
                self.open_subtotals();
            }
            toolstrip::column(ui, |ui| {
                toolstrip::small_menu(ui, Some(Icon::Outline), tr("group.menu"), "", a, |ui| self.outline_menu(ui));
            });
        });
        b.group("toolstrip.g.queries", Icon::Pivot, |ui| {
            if toolstrip::large(ui, Icon::Table, tr("queries.button"), tr("queries.tip"), a, self.queries_dlg.is_some()).clicked() {
                self.open_queries();
            }
            let any = !self.wb.analysis.pivots.is_empty();
            ui.add_enabled_ui(any, |ui| {
                if toolstrip::large(ui, Icon::Calculate, tr("pivot.refresh_all"), "", a, false).on_disabled_hover_text(tr("pivot.none")).clicked() {
                    self.wb.refresh_all_pivots();
                }
            });
            ui.add_enabled_ui(!self.wb.externals.is_empty(), |ui| {
                if toolstrip::large(ui, Icon::Link, tr("links.button"), tr("links.tip"), a, false).clicked() {
                    self.open_edit_links();
                }
            });
        });
        b.group("toolstrip.g.model", Icon::Pivot, |ui| {
            let page = self.model_dlg.as_ref().map(|d| d.page());
            if toolstrip::large(ui, Icon::Link, tr("model.relationships"), tr("model.rel_tip"), a, page == Some(0)).clicked() {
                self.open_data_model(0);
            }
            if toolstrip::large(ui, Icon::Function, tr("model.measures"), tr("model.measures_tip"), a, page == Some(1)).clicked() {
                self.open_data_model(1);
            }
        });
    }

    // --- Review -------------------------------------------------------------------------

    fn review_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.proofing", Icon::Spelling, |ui| {
            if toolstrip::large(ui, Icon::Spelling, tr("spell.title"), &format!("{} (F7)", tr("spell.title")), a, false).clicked() {
                self.start_spelling();
            }
            toolstrip::column(ui, |ui| crate::spell::review_items(ui, false, None));
            if toolstrip::large(ui, Icon::Table, tr("stats.button"), tr("stats.tip"), a, false).clicked() {
                self.open_statistics();
            }
            if toolstrip::large(ui, Icon::ReadAloud, tr("speak.button"), tr("speak.tip_cells"), a, self.reading_aloud()).clicked() {
                self.toggle_read_aloud(ui.ctx());
            }
        });
        b.group("toolstrip.g.accessibility", Icon::ErrorCheck, |ui| {
            if toolstrip::large(ui, Icon::ErrorCheck, tr("access.button"), tr("access.tip"), a, self.review.access_open).clicked() {
                self.toggle_accessibility();
            }
        });
        b.group("toolstrip.g.comments", Icon::Comment, |ui| {
            let thread = self.note_at(self.cursor).is_some_and(|n| n.threaded);
            let key = if thread { "comment.reply" } else { "comment.new" };
            if toolstrip::large(ui, Icon::Comment, tr(key), &format!("{} (Ctrl+Alt+M)", tr(key)), a, false).clicked() {
                self.new_comment();
            }
        });
        b.group("toolstrip.g.notes", Icon::Note, |ui| {
            let has = self.note_at(self.cursor).is_some();
            let key = if has { "note.edit" } else { "note.new" };
            if toolstrip::large(ui, Icon::Note, tr(key), &format!("{} (Shift+F2)", tr(key)), a, false).clicked() {
                self.edit_note();
            }
            toolstrip::column(ui, |ui| {
                ui.add_enabled_ui(has, |ui| {
                    if toolstrip::small(ui, Some(Icon::Eraser), tr("note.delete"), "", a, false).on_disabled_hover_text(tr("note.none_here")).clicked() {
                        self.delete_notes();
                    }
                });
                let any = !self.wb.notes(self.sheet).is_empty();
                let why = tr("note.none");
                ui.add_enabled_ui(any, |ui| {
                    toolstrip::row(ui, |ui| {
                        if toolstrip::small(ui, Some(Icon::ChevronUp), tr("note.previous"), "", a, false).on_disabled_hover_text(why).clicked() {
                            self.step_note(true);
                        }
                        if toolstrip::small(ui, Some(Icon::ChevronDown), tr("note.next"), "", a, false).on_disabled_hover_text(why).clicked() {
                            self.step_note(false);
                        }
                    });
                    let key = if self.any_note_hidden() { "note.show_all" } else { "note.hide_all" };
                    if toolstrip::small(ui, Some(Icon::Note), tr(key), "", a, false).on_disabled_hover_text(why).clicked() {
                        self.show_all_notes();
                    }
                });
            });
        });
        b.group("toolstrip.g.protect", Icon::Protect, |ui| {
            let sheet_key = if self.wb.is_protected(self.sheet) { "protect.unprotect_sheet" } else { "protect.sheet_title" };
            if toolstrip::large(ui, Icon::Protect, tr(sheet_key), "", a, false).clicked() {
                self.protect_cmd(Some(self.sheet));
            }
            let book_key = if self.wb.structure_protected() { "protect.unprotect_workbook" } else { "protect.workbook_title" };
            if toolstrip::large(ui, Icon::Protect, tr(book_key), "", a, false).clicked() {
                self.protect_cmd(None);
            }
            if toolstrip::large(ui, Icon::Select, tr("ranges.button"), tr("ranges.tip"), a, false).clicked() {
                self.open_edit_ranges();
            }
        });
    }

    // --- View ---------------------------------------------------------------------------

    /// The View tab's own groups; the application adds the Window and
    /// Screen groups every kind of tab has (`App::view_tab`).
    /// The Controls group of the Developer tab: form controls to insert,
    /// and the selected one's settings (its macro among them).
    pub(crate) fn developer_controls(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("developer.controls", Icon::Control, |ui| {
            toolstrip::large_menu(ui, Icon::Control, tr("developer.insert"), tr("control.form_tip"), a, |ui| self.form_controls_menu(ui));
            let control = self.selected_shape().is_some_and(|s| s.control.is_some());
            ui.add_enabled_ui(control, |ui| {
                if toolstrip::large(ui, Icon::Options, tr("developer.properties"), tr("developer.properties_tip"), a, false).on_disabled_hover_text(tr("developer.no_control")).clicked() {
                    self.open_control_dialog();
                }
            });
        });
    }

    pub(crate) fn view_tab(&mut self, b: &mut Body, shell: &mut Shell) {
        let a = b.accent();
        b.group("toolstrip.g.workbook_views", Icon::PageBreak, |ui| {
            let view = self.sh().view;
            if toolstrip::large(ui, Icon::Sheet, tr("toolstrip.normal"), tr("view.normal_tip"), a, view == ViewMode::Normal).clicked() {
                self.set_view_mode(ViewMode::Normal);
            }
            if toolstrip::large(ui, Icon::PageBreak, tr("view.page_breaks"), tr("view.page_breaks_tip"), a, view == ViewMode::PageBreakPreview).clicked() {
                self.set_view_mode(if view == ViewMode::PageBreakPreview { ViewMode::Normal } else { ViewMode::PageBreakPreview });
            }
            if toolstrip::large(ui, Icon::Margins, tr("view.page_layout"), tr("view.page_layout_tip"), a, view == ViewMode::PageLayout).clicked() {
                self.set_view_mode(if view == ViewMode::PageLayout { ViewMode::Normal } else { ViewMode::PageLayout });
            }
            if toolstrip::large(ui, Icon::Window, tr("views.title"), tr("views.tip"), a, false).clicked() {
                self.open_custom_views();
            }
        });
        b.group("toolstrip.g.show", Icon::Gridlines, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::check(ui, shell.show_formula_bar, Icon::Function, tr("view.formula_bar")) {
                    shell.cmd = Some(AppCmd::FormulaBar);
                }
                if toolstrip::check(ui, self.show_formulas, Icon::ShowFormulas, tr("view.show_formulas")) {
                    self.show_formulas = !self.show_formulas;
                }
                if toolstrip::check(ui, self.focus_cell, Icon::Focus, tr("view.focus_cell")) {
                    self.focus_cell = !self.focus_cell;
                }
            });
        });
        b.group("toolstrip.g.zoom", Icon::Zoom100, |ui| {
            common::zoom_group(ui, &mut self.zoom, a);
            if toolstrip::large(ui, Icon::ZoomIn, tr("view.zoom_selection"), tr("view.zoom_selection_tip"), a, false).clicked() {
                self.zoom_to_selection();
            }
        });
    }

    /// The workbook's buttons of the View tab's Window group.
    pub(crate) fn window_buttons(&mut self, ui: &mut egui::Ui, shell: &mut Shell, a: Color32) {
        if toolstrip::large(ui, Icon::Window, tr("window.new"), "", a, false).clicked() {
            shell.cmd = Some(AppCmd::NewWindow);
        }
        toolstrip::large_menu(ui, Icon::Freeze, tr("freeze.title"), "", a, |ui| self.freeze_menu(ui));
        if toolstrip::large(ui, Icon::Split, tr("window.split"), "", a, self.split_on()).clicked() {
            self.toggle_split();
        }
        common::side_by_side_buttons(ui, shell, a);
    }

    // --- contextual tabs ------------------------------------------------------------------

    fn picture_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.arrange", Icon::Picture, |ui| {
            if toolstrip::large(ui, Icon::ChevronUp, tr("image.front"), "", a, false).clicked() {
                self.reorder_selected_image(true);
            }
            if toolstrip::large(ui, Icon::ChevronDown, tr("image.back"), "", a, false).clicked() {
                self.reorder_selected_image(false);
            }
            if toolstrip::large(ui, Icon::PictureInCell, tr("image.place_in_cell"), tr("image.place_in_cell_tip"), a, false).clicked() {
                self.place_in_cell();
            }
            toolstrip::large_menu(ui, Icon::Picture, tr("toolstrip.more"), "", a, |ui| self.image_context_menu(ui));
        });
    }

    fn shape_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.insert_shapes", Icon::Shapes, |ui| {
            toolstrip::large_menu(ui, Icon::Shapes, tr("shape.shapes"), "", a, |ui| {
                if let Some(t) = shapes::gallery(ui) {
                    self.start_drawing(t);
                }
            });
        });
        b.group("toolstrip.g.shape_styles", Icon::Fill, |ui| {
            let look = self.selected_shape().map(|s| s.shape.clone());
            ui.add_enabled_ui(look.as_ref().is_some_and(|s| !s.kind.is_line()), |ui| {
                toolstrip::large_menu(ui, Icon::Fill, tr("shape.shape_fill"), tr("shape.shape_fill_tip"), a, |ui| {
                    let Some(s) = &look else { return };
                    match crate::gradient_ui::fill_menu(ui, s.fill, s.gradient.as_ref(), "shape.no_fill") {
                        Some(crate::gradient_ui::FillPick::Set(f, g)) => self.change_shape_look(move |s| (s.fill, s.gradient) = (f, g.clone())),
                        Some(crate::gradient_ui::FillPick::More) => self.open_shape_dialog(0),
                        None => {}
                    }
                });
            });
        });
        b.group("toolstrip.g.arrange", Icon::Shapes, |ui| {
            toolstrip::large_menu(ui, Icon::Shapes, tr("toolstrip.more"), "", a, |ui| self.shape_context_menu(ui));
        });
    }

    fn table_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.properties", Icon::Table, |ui| {
            toolstrip::large_menu(ui, Icon::Table, tr("xtab.design"), "", a, |ui| self.table_menu(ui));
        });
        b.group("toolstrip.g.table_styles", Icon::FormatAsTable, |ui| {
            toolstrip::large_menu(ui, Icon::FormatAsTable, tr("toolstrip.quick_styles"), "", a, |ui| {
                let current = self.cursor_table().and_then(|id| self.wb.table(id).cloned());
                if let Some(name) = self.style_gallery(ui, current.as_ref()) {
                    self.open_create_table(Some(&name));
                    ui.close();
                }
            });
        });
    }

    fn pivot_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("pivot.pivot", Icon::Pivot, |ui| {
            toolstrip::large_menu(ui, Icon::Pivot, tr("pivot.analyze"), "", a, |ui| self.pivot_menu(ui));
            if toolstrip::large(ui, Icon::Calculate, tr("pivot.refresh_all"), "", a, false).clicked() {
                self.wb.refresh_all_pivots();
            }
        });
        b.group("pivot.tools", Icon::Chart, |ui| {
            if toolstrip::large(ui, Icon::ChartColumn, tr("pivot.chart"), tr("pivot.chart_tip"), a, false).clicked() {
                self.insert_pivot_chart();
            }
            if toolstrip::large(ui, Icon::Filter, tr("slicer.insert"), tr("slicer.insert_tip"), a, false).clicked() {
                self.open_slicers();
            }
            ui.vertical(|ui| {
                if toolstrip::small(ui, Some(Icon::Function), &format!("{}…", tr("pivot.calc_field")), tr("pivot.calc_tip"), a, false).clicked() {
                    let index = self.wb.pivot_at(self.sheet, self.cursor);
                    if let Some(i) = index {
                        self.open_calc_field(i);
                    }
                }
                if toolstrip::small(ui, Some(Icon::Table), tr("pivot.show_details"), tr("pivot.details_tip"), a, false).clicked() && !self.show_pivot_details() {
                    self.notices.push(Notice::info(tr("pivot.no_details")));
                }
            });
        });
    }
}
