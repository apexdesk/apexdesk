//! The word processor's toolstrip: its tabs and their groups.

use super::toolbar::{case_menu, undo_label};
use super::*;
use crate::toolstrip::{self, Body, TabDef};
use crate::widgets::Face as WFace;

const OBJECT_TAB: Color32 = Color32::from_rgb(0xB0, 0x5A, 0xC8);
const TABLE_TAB: Color32 = Color32::from_rgb(0x2B, 0x5C, 0xC7);

/// The styles shown in the Styles group, as a gallery.
const QUICK_STYLES: [ParaKind; 7] = [ParaKind::Normal, ParaKind::Title, ParaKind::Heading(1), ParaKind::Heading(2), ParaKind::Heading(3), ParaKind::Quote, ParaKind::Code];

impl DocView {
    pub fn toolstrip_tabs(&self) -> Vec<TabDef> {
        let mut v = vec![
            TabDef::new("home", "toolstrip.home"),
            TabDef::new("insert", "menu.insert"),
            TabDef::new("draw", "toolstrip.draw"),
            TabDef::new("design", "menu.design"),
            TabDef::new("layout", "menu.layout"),
            TabDef::new("references", "menu.references"),
            TabDef::new("mailings", "menu.mailings"),
            TabDef::new("review", "menu.review"),
            TabDef::new("view", "menu.view"),
        ];
        if self.is_book() {
            v.push(TabDef::contextual("book", "book.tab", theme::DOC_ACCENT));
        }
        if self.views.mode == ViewMode::Outline {
            v.insert(0, TabDef::contextual("outline", "outline.tab", theme::DOC_ACCENT));
        }
        if self.selected_picture().is_some() {
            v.push(TabDef::contextual("picture", "toolstrip.picture_format", OBJECT_TAB));
        } else if self.selected_shape().is_some() {
            v.push(TabDef::contextual("shape", "toolstrip.shape_format", OBJECT_TAB));
        } else if self.in_table() {
            v.push(TabDef::contextual("table", "toolstrip.table_layout", TABLE_TAB));
        }
        v
    }

    /// The body of toolstrip tab `tab`.
    pub fn toolstrip(&mut self, ui: &mut egui::Ui, tab: &str, _shell: &mut Shell) {
        let id = Id::new(("doc_toolstrip", tab));
        toolstrip::body(ui, id, theme::DOC_ACCENT, |b| match tab {
            "insert" => self.insert_tab(b),
            "draw" => self.draw_tab(b),
            "design" => self.design_tab(b),
            "layout" => self.layout_tab(b),
            "references" => self.references_tab(b),
            "mailings" => self.mailings_tab(b),
            "review" => self.review_tab(b),
            "picture" => self.picture_tab(b),
            "shape" => self.shape_tab(b),
            "table" => self.table_tab(b),
            "outline" => self.outline_tab(b),
            "book" => self.book_tab(b),
            _ => self.home_tab(b),
        });
    }

    // --- Home ---------------------------------------------------------------------------

    fn home_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        let a = self.attrs_at_caret();
        let st = self.cur_para().style;
        b.group("toolstrip.g.undo", Icon::Undo, |ui| {
            toolstrip::column(ui, |ui| {
                let labels: Vec<&'static str> = self.doc.undo_history().into_iter().map(undo_label).collect();
                for _ in 0..common::history_button(ui, false, &labels) {
                    self.undo();
                }
                let labels: Vec<&'static str> = self.doc.redo_history().into_iter().map(undo_label).collect();
                for _ in 0..common::history_button(ui, true, &labels) {
                    self.redo();
                }
            });
        });
        b.group("toolstrip.g.clipboard", Icon::Paste, |ui| {
            let mut plain = None;
            if toolstrip::large_split(ui, Icon::Paste, tr("edit.paste"), &format!("{} (Ctrl+V)", tr("edit.paste")), acc, |ui| {
                if toolstrip::menu_item_keys(ui, Icon::Paste, tr("edit.paste"), "Ctrl+V").clicked() {
                    plain = Some(false);
                }
                if toolstrip::menu_item_keys(ui, Icon::Paste, tr("doc.paste_plain"), "Ctrl+Shift+V").clicked() {
                    plain = Some(true);
                }
                ui.separator();
                if toolstrip::menu_item(ui, Icon::Paste, &format!("{}…", tr("clipboard.title"))).clicked() {
                    self.clip_pane = true;
                    ui.close();
                }
            }) {
                plain = Some(false);
            }
            if let Some(p) = plain {
                self.request_paste(ui.ctx(), p);
            }
            toolstrip::column(ui, |ui| {
                let has = !self.sel.is_empty();
                ui.add_enabled_ui(has, |ui| {
                    if toolstrip::small(ui, Some(Icon::Cut), tr("edit.cut"), &format!("{} (Ctrl+X)", tr("edit.cut")), acc, false).clicked() {
                        self.copy(ui.ctx(), true);
                    }
                    if toolstrip::small(ui, Some(Icon::Copy), tr("edit.copy"), &format!("{} (Ctrl+C)", tr("edit.copy")), acc, false).clicked() {
                        self.copy(ui.ctx(), false);
                    }
                });
                let brush = self.brush.as_ref().map(|b| b.sticky);
                let r = toolstrip::small(ui, Some(Icon::Brush), tr("toolstrip.brush"), tr("toolbar.brush_hint"), acc, brush.is_some());
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
                toolstrip::row(ui, |ui| self.font_row(ui, &a, &st));
                toolstrip::row(ui, |ui| self.text_style_row(ui, &a));
            });
        });
        b.group("toolstrip.g.paragraph", Icon::Pilcrow, |ui| {
            toolstrip::column(ui, |ui| {
                toolstrip::row(ui, |ui| self.list_row(ui, &st));
                toolstrip::row(ui, |ui| self.align_row(ui, &st));
            });
        });
        // Text a linked style formats shows that style, not its paragraph's.
        let mut shown = st.clone();
        let at = if self.sel.is_empty() { self.sel.cursor } else { apex_doc::Pos::new(self.sel.start().para, self.sel.start().offset + 1) };
        if let Some((kind, custom)) = self.doc.linked_text_style(at) {
            (shown.kind, shown.custom) = (kind, custom);
        }
        b.group("toolstrip.g.styles", Icon::Styles, |ui| self.quick_styles(ui, &shown));
        b.group("toolstrip.g.editing", Icon::Search, |ui| {
            toolstrip::column(ui, |ui| {
                let on = self.find.as_ref().is_some_and(|f| !f.nav);
                if toolstrip::small(ui, Some(Icon::Search), tr("edit.find"), &format!("{} (Ctrl+F)", tr("edit.find")), acc, on).clicked() {
                    if on {
                        self.find = None;
                    } else {
                        self.open_find(false);
                    }
                }
                if toolstrip::small(ui, Some(Icon::Replace), tr("edit.replace"), &format!("{} (Ctrl+H)", tr("edit.replace")), acc, false).clicked() {
                    self.open_find(true);
                }
                toolstrip::small_menu(ui, Some(Icon::Select), tr("toolstrip.select"), "", acc, |ui| {
                    if toolstrip::menu_item_keys(ui, Icon::Select, tr("edit.select_all"), "Ctrl+A").clicked() {
                        self.sel = Selection { anchor: Pos::new(0, 0), cursor: self.doc.end() };
                        ui.close();
                    }
                    if toolstrip::menu_item_keys(ui, Icon::PageNumber, &format!("{}…", tr("goto.title")), "Ctrl+G").clicked() {
                        self.open_goto();
                        ui.close();
                    }
                    if toolstrip::menu_item_on(ui, Icon::Select, tr("selpane.title"), "", self.pane_open).clicked() {
                        self.pane_open = !self.pane_open;
                        ui.close();
                    }
                });
            });
        });
    }

    fn font_row(&mut self, ui: &mut egui::Ui, a: &Attrs, st: &ParaStyle) {
        let default = tr("font.default");
        let cf = self.doc.char_format(st, a);
        let cur = cf.font.clone().or(self.doc.default_font_name()).unwrap_or_else(|| Arc::from(default));
        if let Some(f) = common::themed_font_picker(ui, Id::new(("dfont", self.uid)), &cur, 130.0) {
            let f: Option<Arc<str>> = if f.eq_ignore_ascii_case(default) { None } else { Some(Arc::from(f.as_str())) };
            self.format(move |x| x.font = f.clone());
        }
        let size = cf.size as f32 / 2.0;
        if let Some(v) = common::size_box(ui, Id::new(("dsize", self.uid)), size) {
            let hp = (v * 2.0).round() as u16;
            self.format(move |x| x.size = Some(hp));
        }
        if toolstrip::tool(ui, WFace::Icon(Icon::GrowFont), false, &format!("{} (Ctrl+Shift+>)", tr("format.grow"))).clicked() {
            self.grow_font(true);
        }
        if toolstrip::tool(ui, WFace::Icon(Icon::ShrinkFont), false, &format!("{} (Ctrl+Shift+<)", tr("format.shrink"))).clicked() {
            self.grow_font(false);
        }
        let mut case = None;
        if toolstrip::split(ui, WFace::Icon(Icon::ChangeCase), false, &format!("{} (Shift+F3)", tr("format.change_case")), |ui| case = case_menu(ui)) {
            self.cycle_case();
        }
        if let Some(c) = case {
            self.change_case(c);
        }
        if toolstrip::tool(ui, WFace::Icon(Icon::ClearFormat), false, &format!("{} (Ctrl+Space)", tr("format.clear"))).clicked() {
            self.clear_formatting(true);
        }
        if toolstrip::tool(ui, WFace::Text(RichText::new("A…").size(11.0)), false, &format!("{}… (Ctrl+D)", tr("font.title"))).clicked() {
            self.open_font_dialog();
        }
        // A guide goes over text of one paragraph.
        let ruby = self.ruby_target().is_some();
        let r = ui.add_enabled_ui(ruby, |ui| toolstrip::tool(ui, WFace::Text(RichText::new("亜").size(13.0)), false, &format!("{}…", tr("ruby.title")))).inner;
        if r.on_disabled_hover_text(tr("ruby.need_text")).clicked() {
            self.open_ruby_dialog();
        }
    }

    fn text_style_row(&mut self, ui: &mut egui::Ui, a: &Attrs) {
        if toolstrip::tool(ui, WFace::Text(RichText::new("B").strong()), a.bold, &format!("{} (Ctrl+B)", tr("format.bold"))).clicked() {
            self.toggle(|a| a.bold, |a, v| a.bold = v);
        }
        if toolstrip::tool(ui, WFace::Text(RichText::new("I").italics()), a.italic, &format!("{} (Ctrl+I)", tr("format.italic"))).clicked() {
            self.toggle(|a| a.italic, |a, v| a.italic = v);
        }
        if toolstrip::tool(ui, WFace::Text(RichText::new("U").underline()), a.underline, &format!("{} (Ctrl+U)", tr("format.underline"))).clicked() {
            self.toggle(|a| a.underline, |a, v| a.underline = v);
        }
        if toolstrip::tool(ui, WFace::Text(RichText::new("S").strikethrough()), a.strike, tr("format.strike")).clicked() {
            self.toggle(|a| a.strike, |a, v| a.strike = v);
        }
        if toolstrip::tool(ui, WFace::Icon(Icon::Subscript), a.subscript, &format!("{} (Ctrl+Shift+B)", tr("format.subscript"))).clicked() {
            self.toggle(|a| a.subscript, |a, v| {
                a.subscript = v;
                a.superscript = false;
            });
        }
        if toolstrip::tool(ui, WFace::Icon(Icon::Superscript), a.superscript, &format!("{} (Ctrl+Shift+P)", tr("format.superscript"))).clicked() {
            self.toggle(|a| a.superscript, |a, v| {
                a.superscript = v;
                a.subscript = false;
            });
        }
        if toolstrip::tool(ui, WFace::Text(RichText::new("</>").monospace().size(11.0)), a.code, tr("format.code")).clicked() {
            self.toggle(|a| a.code, |a, v| a.code = v);
        }
        ui.add_space(4.0);
        let last = self.last_highlight;
        let mut chosen = None;
        let bar = last.map_or(Color32::TRANSPARENT, theme::rgb);
        if toolstrip::split(ui, WFace::IconBar(Icon::Highlight, bar), false, tr("format.highlight"), |ui| {
            chosen = widgets::color_menu(ui, tr("format.no_highlight"), a.highlight);
        }) {
            chosen = Some(last);
        }
        if let Some(c) = chosen {
            self.last_highlight = c;
            self.format(move |x| x.highlight = c);
        }
        let last = self.last_color;
        let mut chosen = None;
        let bar = last.map_or(Paper::TEXT, theme::rgb);
        if toolstrip::split(ui, WFace::TextBar(RichText::new("A").strong(), bar), false, tr("format.text_color"), |ui| {
            chosen = widgets::color_menu(ui, tr("format.auto_color"), a.color);
        }) {
            chosen = Some(last);
        }
        if let Some(c) = chosen {
            self.last_color = c;
            self.format(move |x| x.color = c);
        }
    }

    fn list_row(&mut self, ui: &mut egui::Ui, st: &ParaStyle) {
        let mut list = None;
        if toolstrip::split(ui, WFace::Icon(Icon::Bullets), st.kind == ParaKind::Bullet, &format!("{} (Ctrl+Shift+L)", tr("doc.bullets")), |ui| list = self.bullet_menu(ui)) {
            self.set_kind(ParaKind::Bullet);
        }
        if toolstrip::split(ui, WFace::Icon(Icon::Numbering), st.kind == ParaKind::Numbered, tr("doc.numbering"), |ui| list = self.numbering_menu(ui)) {
            self.set_kind(ParaKind::Numbered);
        }
        if let Some((kind, f)) = list {
            self.set_list(kind, f);
        }
        toolstrip::tool_menu(ui, WFace::Icon(Icon::Multilevel), st.list > 0, tr("lists.multilevel"), |ui| self.multilevel_menu(ui));
        if toolstrip::tool(ui, WFace::Icon(Icon::NoList), false, tr("doc.no_list")).clicked() {
            self.para_style(|s| {
                if s.kind.is_list() {
                    s.kind = ParaKind::Normal;
                    s.level = 0;
                }
            });
        }
        ui.add_space(4.0);
        if toolstrip::tool(ui, WFace::Icon(Icon::Outdent), false, &format!("{} (Ctrl+Shift+M)", tr("doc.outdent"))).clicked() {
            self.indent(-1);
        }
        if toolstrip::tool(ui, WFace::Icon(Icon::Indent), false, &format!("{} (Ctrl+M)", tr("doc.indent"))).clicked() {
            self.indent(1);
        }
        ui.add_space(4.0);
        if toolstrip::tool(ui, WFace::Icon(Icon::Pilcrow), self.show_marks, &format!("{} (Ctrl+Shift+8)", tr("doc.marks"))).clicked() {
            self.show_marks = !self.show_marks;
            self.layout.key.0 = u64::MAX;
        }
    }

    fn align_row(&mut self, ui: &mut egui::Ui, st: &ParaStyle) {
        for (al, icon, key, keys) in [
            (Align::Left, Icon::AlignLeft, "format.align_left", "Ctrl+L"),
            (Align::Center, Icon::AlignCenter, "format.align_center", "Ctrl+E"),
            (Align::Right, Icon::AlignRight, "format.align_right", "Ctrl+R"),
            (Align::Justify, Icon::Justify, "format.align_justify", "Ctrl+J"),
        ] {
            if toolstrip::tool(ui, WFace::Icon(icon), st.align == al, &format!("{} ({keys})", tr(key))).clicked() {
                self.set_align(al);
            }
        }
        ui.add_space(4.0);
        toolstrip::tool_menu(ui, WFace::Icon(Icon::LineSpacing), false, tr("doc.line_spacing"), |ui| self.spacing_menu(ui));
        toolstrip::tool_menu(ui, WFace::Text(RichText::new("文A").size(11.0)), false, tr("asian.layout"), |ui| {
            use super::asian_layout_ui::LayoutKind;
            for (kind, key) in [(LayoutKind::Combine, "asian.combine"), (LayoutKind::TwoLines, "asian.two_lines"), (LayoutKind::Upright, "asian.upright"), (LayoutKind::Fit, "asian.fit_text")] {
                if ui.button(format!("{}…", tr(key))).clicked() {
                    self.open_asian_layout(kind);
                    ui.close();
                }
            }
        });
        let last = self.last_shading;
        let mut chosen = None;
        let bar = last.map_or(Color32::TRANSPARENT, theme::rgb);
        if toolstrip::split(ui, WFace::IconBar(Icon::Shading, bar), false, tr("doc.shading"), |ui| {
            chosen = widgets::color_menu(ui, tr("doc.no_shading"), st.shading);
        }) {
            chosen = Some(last);
        }
        if let Some(c) = chosen {
            self.last_shading = c;
            self.para_style(move |s| s.shading = c);
        }
        if toolstrip::tool(ui, WFace::Icon(Icon::Borders), false, &format!("{}…", tr("layout.borders_shading"))).clicked() {
            self.open_para_borders_dialog();
        }
    }

    /// The Styles gallery: each style drawn as it looks. In the one-row
    /// toolstrip, a button with the current style that opens the gallery.
    fn quick_styles(&mut self, ui: &mut egui::Ui, st: &ParaStyle) {
        let acc = theme::DOC_ACCENT;
        let custom = self.custom_styles();
        if toolstrip::compact() {
            let name = match custom.iter().find(|(n, _)| st.custom != 0 && *n == st.custom) {
                Some((_, name)) => name.clone(),
                None => kind_label(st.kind).to_string(),
            };
            let face = WFace::Text(egui::RichText::new(format!("{name} ⏷")).size(12.5));
            toolstrip::tool_menu(ui, face, false, tr("toolstrip.g.styles"), |ui| {
                toolstrip::with_style(toolstrip::Style::Toolstrip, || {
                    if self.style_grid(ui, st) {
                        ui.close();
                    }
                    ui.separator();
                    crate::widgets::menu_button(ui, tr("toolstrip.more"), |ui| self.more_styles(ui, st, &custom));
                })
            });
        } else {
            self.style_grid(ui, st);
        }
        toolstrip::column(ui, |ui| {
            if !toolstrip::compact() {
                toolstrip::small_menu(ui, None, tr("toolstrip.more"), "", acc, |ui| self.more_styles(ui, st, &custom));
            }
            if toolstrip::small(ui, Some(Icon::Styles), tr("style.pane"), &format!("{} (Ctrl+Shift+Alt+S)", tr("style.pane")), acc, self.styles.pane).clicked() {
                self.styles.pane = !self.styles.pane;
            }
            // Paragraph… is in Layout: three rows, like every group.
            if toolstrip::small(ui, Some(Icon::Search), tr("reveal.title"), &format!("{} (Shift+F1)", tr("reveal.title")), acc, self.styles.reveal).clicked() {
                self.toggle_reveal();
            }
        });
    }

    /// The style chips, four per row; true when one was chosen.
    fn style_grid(&mut self, ui: &mut egui::Ui, st: &ParaStyle) -> bool {
        let mut chosen = false;
        egui::Grid::new(("quick_styles", self.uid)).spacing(vec2(3.0, 3.0)).show(ui, |ui| {
            for (i, k) in QUICK_STYLES.into_iter().enumerate() {
                let on = st.kind == k && st.custom == 0;
                if style_chip(ui, k, on).clicked() | toolstrip::register_only(ui, kind_label(k), Icon::Styles) {
                    self.set_kind(k);
                    chosen = true;
                }
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });
        chosen
    }

    /// Every style, the document's own ones and the style commands.
    fn more_styles(&mut self, ui: &mut egui::Ui, st: &ParaStyle, custom: &[(u16, String)]) {
        for k in KINDS {
            if ui.add(egui::Button::selectable(st.kind == k && st.custom == 0, kind_label(k))).clicked() {
                self.set_kind(k);
                ui.close();
            }
        }
        if !custom.is_empty() {
            ui.separator();
            for (n, name) in custom {
                if ui.add(egui::Button::selectable(st.custom == *n, name.as_str())).clicked() {
                    self.apply_style(ParaKind::Normal, *n);
                    ui.close();
                }
            }
        }
        ui.separator();
        if toolstrip::menu_item(ui, Icon::Styles, &format!("{}…", tr("style.modify_current"))).clicked() {
            self.modify_current_style();
            ui.close();
        }
        if toolstrip::menu_item(ui, Icon::Styles, &format!("{}…", tr("style.new_from_sel"))).clicked() {
            self.open_new_style(false);
            ui.close();
        }
    }

    // --- Insert -------------------------------------------------------------------------

    fn insert_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        // A notebook's page has no pages, headers, fields or forms.
        let doc = !self.embedded;
        if doc {
            b.group("toolstrip.g.pages", Icon::PageBreak, |ui| {
            if toolstrip::large(ui, Icon::PageBreak, tr("doc.page_break"), &format!("{} (Ctrl+Enter)", tr("doc.page_break")), a, false).clicked() {
                self.key(Key::Enter, None, Modifiers::COMMAND);
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Pilcrow), tr("doc.line_break"), &format!("{} (Shift+Enter)", tr("doc.line_break")), a, false).clicked() {
                    self.insert(&LINE_BREAK.to_string());
                }
                toolstrip::small_menu(ui, Some(Icon::PageBreak), tr("layout.breaks"), "", a, |ui| self.breaks_items(ui));
            });
        });
        }
        b.group("toolstrip.g.tables", Icon::Table, |ui| {
            toolstrip::large_menu(ui, Icon::Table, tr("table.table"), "", a, |ui| self.insert_table_items(ui));
        });
        b.group("toolstrip.g.illustrations", Icon::Picture, |ui| {
            if toolstrip::large(ui, Icon::Picture, tr("pic.insert"), "", a, false).clicked() {
                self.pick_picture();
            }
            toolstrip::large_menu(ui, Icon::Shapes, tr("shape.shapes"), "", a, |ui| {
                if let Some(t) = crate::shapes::gallery(ui) {
                    self.start_drawing(t);
                }
            });
            toolstrip::large_menu(ui, Icon::Chart, tr("chart.insert"), tr("chart.insert_doc_tip"), a, |ui| {
                if let Some(i) = crate::sheet_view::chart_type_menu(ui) {
                    self.insert_chart_object(i);
                }
            });
            if toolstrip::large(ui, Icon::QrCode, tr("barcode.menu"), tr("barcode.tip"), a, false).clicked() {
                self.show_barcode();
            }
            if crate::library::available() && toolstrip::large(ui, Icon::Folder, tr("library.menu"), tr("library.tip"), a, false).clicked() {
                self.show_library();
            }
        });
        b.group("toolstrip.g.links", Icon::Link, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Link), &format!("{}…", tr("doc.link")), &format!("{} (Ctrl+K)", tr("doc.link")), a, false).clicked() {
                    self.open_link_dialog(None);
                }
                if toolstrip::small(ui, Some(Icon::Bookmark), &format!("{}…", tr("doc.bookmark")), &format!("{} (Ctrl+Shift+F5)", tr("doc.bookmark")), a, false).clicked() {
                    self.open_bookmark_dialog();
                }
                if toolstrip::small(ui, Some(Icon::CrossRef), &format!("{}…", tr("xref.insert")), "", a, false).clicked() {
                    self.open_xref_dialog();
                }
            });
        });
        b.group("toolstrip.g.comments", Icon::Comment, |ui| {
            if toolstrip::large(ui, Icon::Comment, tr("toolstrip.comment"), "", a, false).clicked() {
                self.new_comment();
            }
        });
        if doc {
        b.group("toolstrip.g.header_footer", Icon::HeaderFooter, |ui| {
            toolstrip::large_menu(ui, Icon::HeaderFooter, tr("toolstrip.header_footer"), "", a, |ui| {
                if ui.button(tr("hf.edit_header")).clicked() {
                    self.edit_hf_at_caret(true);
                    ui.close();
                }
                if ui.button(tr("hf.edit_footer")).clicked() {
                    self.edit_hf_at_caret(false);
                    ui.close();
                }
                ui.separator();
                if ui.button(tr("hf.page_number_top")).clicked() {
                    self.insert_page_number(true);
                    ui.close();
                }
                if ui.button(tr("hf.page_number_bottom")).clicked() {
                    self.insert_page_number(false);
                    ui.close();
                }
                if ui.button(format!("{}…", tr("hf.page_number_format"))).clicked() {
                    self.open_page_setup(2);
                    ui.close();
                }
                ui.separator();
                if ui.button(format!("{}…", tr("page.tab_hf"))).clicked() {
                    self.open_page_setup(2);
                    ui.close();
                }
            });
        });
        }
        b.group("toolstrip.g.text", Icon::TextBox, |ui| {
            let armed = self.shapes.armed().is_some_and(|t| t.kind == apex_doc::ShapeKind::TextBox);
            if toolstrip::large(ui, Icon::TextBox, tr("shape.text_box"), "", a, armed).clicked() {
                self.start_drawing(crate::shapes::Template { kind: apex_doc::ShapeKind::TextBox, arrow: false, preset: None });
            }
            toolstrip::column(ui, |ui| {
                toolstrip::small_menu(ui, Some(Icon::DateTime), tr("doc.date_time"), "", a, |ui| {
                    for (key, date, time) in [("doc.date", true, false), ("doc.time", false, true), ("doc.date_time", true, true)] {
                        if toolstrip::menu_item(ui, Icon::DateTime, tr(key)).clicked() {
                            self.insert_date_time(date, time);
                            ui.close();
                        }
                    }
                });
                if doc && toolstrip::small(ui, Some(Icon::Field), &format!("{}…", tr("field.insert")), "", a, false).clicked() {
                    self.refs.dialog = Some(refs::RefDialog::Field(Box::default()));
                }
                if doc && toolstrip::small(ui, Some(Icon::DropCap), &format!("{}…", tr("dropcap.title")), "", a, false).clicked() {
                    self.open_drop_cap_dialog();
                }
            });
            self.text_styles_button(ui, a);
            if doc {
                self.blocks_menu(ui, a);
            }
            if toolstrip::large(ui, Icon::Object, tr("object.menu"), "", a, false).clicked() {
                self.show_insert_object();
            }
        });
        if doc {
            self.controls_group(b);
        }
        b.group("toolstrip.g.symbols", Icon::Symbol, |ui| {
            if toolstrip::large(ui, Icon::Equation, tr("eq.menu"), &format!("{} (Alt+=)", tr("eq.menu")), a, false).clicked() {
                self.open_equation();
            }
            if toolstrip::large(ui, Icon::Symbol, tr("symbol.menu"), "", a, false).clicked() {
                self.open_symbols();
            }
        });
    }

    // --- Design -------------------------------------------------------------------------

    fn design_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.themes", Icon::Themes, |ui| {
            toolstrip::large_menu(ui, Icon::Themes, tr("theme.themes"), "", a, |ui| {
                let cur = self.doc.styles.theme.clone();
                if let Some(t) = crate::themes_ui::theme_menus(ui, &cur) {
                    self.doc.set_theme(t, self.sel);
                }
            });
        });
        b.group("toolstrip.g.page_background", Icon::PageColor, |ui| {
            if toolstrip::large(ui, Icon::Watermark, tr("layout.watermark"), "", a, false).clicked() {
                self.open_watermark_dialog();
            }
            toolstrip::large_menu(ui, Icon::PageColor, tr("layout.page_color"), "", a, |ui| self.page_color_items(ui));
            if toolstrip::large(ui, Icon::PageBorders, tr("layout.page_borders"), "", a, false).clicked() {
                self.open_page_borders_dialog();
            }
        });
    }

    // --- Layout -------------------------------------------------------------------------

    fn layout_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("page.setup", Icon::PageSize, |ui| {
            toolstrip::large_menu(ui, Icon::Orientation, tr("page.text_dir"), "", a, |ui| self.text_direction_items(ui));
            toolstrip::large_menu(ui, Icon::Margins, tr("layout.margins"), "", a, |ui| self.margins_items(ui));
            toolstrip::large_menu(ui, Icon::PageOrientation, tr("page.orientation"), "", a, |ui| self.orientation_items(ui));
            toolstrip::large_menu(ui, Icon::PageSize, tr("layout.size"), "", a, |ui| self.size_items(ui));
            toolstrip::large_menu(ui, Icon::Columns, tr("layout.columns"), "", a, |ui| self.columns_items(ui));
            toolstrip::column(ui, |ui| {
                toolstrip::small_menu(ui, Some(Icon::PageBreak), tr("layout.breaks"), "", a, |ui| self.breaks_items(ui));
                toolstrip::small_menu(ui, Some(Icon::Numbering), tr("linenum.menu"), "", a, |ui| self.line_number_items(ui));
                toolstrip::small_menu(ui, Some(Icon::Hyphen), tr("hyph.menu"), "", a, |ui| self.hyphenation_items(ui));
            });
            if toolstrip::large(ui, Icon::Options, &format!("{}…", tr("page.setup")), "", a, false).clicked() {
                self.open_page_setup(0);
            }
        });
        b.group("toolstrip.g.paragraph", Icon::Pilcrow, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Pilcrow), &format!("{}…", tr("doc.paragraph")), "", a, false).clicked() {
                    self.open_paragraph_dialog();
                }
                toolstrip::small_menu(ui, Some(Icon::LineSpacing), tr("doc.line_spacing"), "", a, |ui| self.spacing_menu(ui));
                if toolstrip::small(ui, Some(Icon::Ruler), &format!("{}…", tr("tabs.title")), "", a, false).clicked() {
                    self.open_tabs_dialog();
                }
            });
        });
        b.group("toolstrip.g.arrange", Icon::Shapes, |ui| {
            if toolstrip::large(ui, Icon::Borders, tr("layout.borders_shading"), "", a, false).clicked() {
                self.open_para_borders_dialog();
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
            if toolstrip::large(ui, Icon::Thesaurus, tr("thes.title"), &format!("{} (Shift+F7)", tr("thes.title")), a, false).clicked() {
                self.open_thesaurus();
            }
            if toolstrip::large(ui, Icon::Calculate, tr("count.title"), &format!("{} (Ctrl+Shift+G)", tr("count.title")), a, false).clicked() {
                self.open_word_count();
            }
            if toolstrip::large(ui, Icon::ReadAloud, tr("speak.button"), tr("speak.tip"), a, self.reading_aloud()).clicked() {
                self.toggle_read_aloud(ui.ctx());
            }
            toolstrip::column(ui, |ui| {
                let mut text_lang = false;
                crate::spell::review_items(ui, true, Some(&mut text_lang));
                if text_lang {
                    self.open_language_dialog();
                }
            });
        });
        b.group("toolstrip.g.accessibility", Icon::ErrorCheck, |ui| {
            if toolstrip::large(ui, Icon::ErrorCheck, tr("access.button"), tr("access.doc_tip"), a, self.access.open).clicked() {
                self.toggle_accessibility();
            }
        });
        b.group("toolstrip.g.comments", Icon::Comment, |ui| {
            if toolstrip::large(ui, Icon::Comment, tr("toolstrip.comment"), "", a, false).clicked() {
                self.new_comment();
            }
            toolstrip::large_menu(ui, Icon::Comment, tr("toolstrip.comments"), "", a, |ui| self.comment_menu_items(ui));
            // A notebook keeps its own history: the name only signs comments.
            if self.embedded {
                self.user_name_item(ui);
            }
        });
        if self.embedded {
            return;
        }
        b.group("toolstrip.g.tracking", Icon::TrackChanges, |ui| {
            let on = self.doc.track_changes;
            if toolstrip::large(ui, Icon::TrackChanges, tr("toolstrip.track_changes"), "", a, on).clicked() {
                self.toggle_tracking();
            }
            toolstrip::large_menu(ui, Icon::Accept, tr("toolstrip.changes"), "", a, |ui| self.revision_menu_items(ui));
            self.user_name_item(ui);
        });
        b.group("toolstrip.g.protect", Icon::Protect, |ui| {
            let on = self.doc.edit_limit().is_some();
            if toolstrip::large(ui, Icon::Protect, tr("protect.title"), "", a, on).clicked() {
                self.open_restrict_dialog();
            }
        });
    }

    // --- View ---------------------------------------------------------------------------

    /// The View tab's own groups; the application adds the Window and
    /// Screen groups every kind of tab has (`App::view_tab`).
    pub(crate) fn view_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        self.views_group(b);
        self.md_group(b);
        b.group("toolstrip.g.show", Icon::Ruler, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::check(ui, self.show_ruler, Icon::Ruler, tr("doc.ruler")) {
                    self.show_ruler = !self.show_ruler;
                }
                if toolstrip::check(ui, self.show_marks, Icon::Pilcrow, tr("doc.marks")) {
                    self.show_marks = !self.show_marks;
                    self.layout.key.0 = u64::MAX;
                }
                if toolstrip::check(ui, self.nav_open(), Icon::NavPane, tr("nav.title")) {
                    self.toggle_nav();
                }
            });
        });
        b.group("toolstrip.g.zoom", Icon::Zoom100, |ui| {
            common::zoom_group(ui, &mut self.zoom, a);
            self.page_zoom_buttons(ui, a);
        });
    }

    // --- contextual tabs ------------------------------------------------------------------

    fn picture_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("picfx.g_adjust", Icon::Brightness, |ui| self.picture_adjust_group(ui, a));
        b.group("picfx.g_styles", Icon::PicStyles, |ui| self.picture_styles_group(ui, a));
        b.group("toolstrip.g.arrange", Icon::Picture, |ui| {
            if toolstrip::large(ui, Icon::Picture, &format!("{}…", tr("toolstrip.picture_props")), "", a, false).clicked() {
                if let Some((at, _, _)) = self.selected_picture() {
                    self.open_picture_props(at, 0);
                }
            }
            toolstrip::large_menu(ui, Icon::Wrap, tr("pic.wrap"), "", a, |ui| {
                self.picture_wrap_menu(ui);
            });
            toolstrip::large_menu(ui, Icon::Picture, tr("toolstrip.more"), "", a, |ui| {
                self.picture_menu(ui);
            });
            if toolstrip::large(ui, Icon::Select, tr("selpane.title"), "", a, self.pane_open).clicked() {
                self.pane_open = !self.pane_open;
            }
        });
    }

    fn shape_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.insert_shapes", Icon::Shapes, |ui| {
            toolstrip::large_menu(ui, Icon::Shapes, tr("shape.shapes"), "", a, |ui| {
                if let Some(t) = crate::shapes::gallery(ui) {
                    self.start_drawing(t);
                }
            });
        });
        b.group("toolstrip.g.shape_styles", Icon::Fill, |ui| {
            let look = self.selected_shape().map(|b| b.shape.shape.clone());
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
            if toolstrip::large(ui, Icon::Shapes, &format!("{}…", tr("toolstrip.shape_props")), "", a, false).clicked() {
                self.open_shape_dialog(0);
            }
            toolstrip::large_menu(ui, Icon::Shapes, tr("toolstrip.more"), "", a, |ui| {
                self.shape_menu(ui);
            });
            toolstrip::large_menu(ui, Icon::AlignLeft, tr("arrange.align"), "", a, |ui| self.align_menu(ui));
            let (can_group, can_ungroup) = (self.can_group(), self.can_ungroup());
            if ui.add_enabled_ui(can_group, |ui| toolstrip::large(ui, Icon::Group, tr("arrange.group"), &tr("arrange.group_hint"), a, false)).inner.clicked() {
                self.group_picked();
            }
            if ui.add_enabled_ui(can_ungroup, |ui| toolstrip::large(ui, Icon::Ungroup, tr("arrange.ungroup"), "", a, false)).inner.clicked() {
                self.ungroup_selected();
            }
            if toolstrip::large(ui, Icon::Select, tr("selpane.title"), "", a, self.pane_open).clicked() {
                self.pane_open = !self.pane_open;
            }
        });
    }

    fn table_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.rows_cols", Icon::InsertRows, |ui| self.table_toolstrip_edit(ui, a));
        b.group("toolstrip.g.cell_size", Icon::Columns, |ui| self.table_tools_group(ui, a));
        b.group("toolstrip.g.table_styles", Icon::FormatAsTable, |ui| {
            toolstrip::large_menu(ui, Icon::FormatAsTable, tr("table.styles"), "", a, |ui| self.style_menu(ui));
            toolstrip::large_menu(ui, Icon::Borders, tr("table.borders"), "", a, |ui| self.borders_menu(ui));
            toolstrip::column(ui, |ui| {
                let header = self.table_props().is_some_and(|p| p.header_rows > 0);
                if toolstrip::check(ui, header, Icon::Table, tr("table.repeat_header")) {
                    self.toggle_header();
                }
            });
        });
        b.group("toolstrip.g.data", Icon::Table, |ui| {
            toolstrip::large_menu(ui, Icon::Table, tr("table.table"), "", a, |ui| self.table_menu(ui));
            if toolstrip::large(ui, Icon::Options, &format!("{}…", tr("table.properties")), "", a, false).clicked() {
                self.open_props_dialog();
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::SortAsc), &format!("{}…", tr("table.sort")), "", a, false).clicked() {
                    self.open_sort_dialog();
                }
                if toolstrip::small(ui, Some(Icon::TextToColumns), &format!("{}…", tr("table.table_to_text")), "", a, false).clicked() {
                    self.open_to_text_dialog();
                }
            });
        });
    }
}

/// A style of the Styles gallery, drawn in its look.
fn style_chip(ui: &mut egui::Ui, k: ParaKind, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(76.0, 34.0), Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, kind_label(k), Some(on));
    let v = ui.visuals();
    let fill = if on { v.selection.bg_fill.gamma_multiply(0.35) } else if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { v.extreme_bg_color };
    ui.painter().rect(rect, 3.0, fill, Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color), egui::StrokeKind::Inside);
    let text = v.text_color();
    let (mut font, color) = match k {
        ParaKind::Title => (egui::FontId::proportional(17.0), text),
        ParaKind::Heading(1) => (egui::FontId::proportional(14.5), theme::DOC_ACCENT),
        ParaKind::Heading(2) => (egui::FontId::proportional(13.0), theme::DOC_ACCENT),
        ParaKind::Heading(_) => (egui::FontId::proportional(12.0), theme::DOC_ACCENT.gamma_multiply(0.8)),
        ParaKind::Code => (egui::FontId::monospace(11.5), text),
        ParaKind::Quote => (egui::FontId::proportional(12.0), v.weak_text_color()),
        _ => (egui::FontId::proportional(12.0), text),
    };
    let caption = ui.painter().layout_no_wrap(kind_label(k).to_string(), egui::FontId::proportional(10.0), v.weak_text_color());
    // A big sample (Title) shrinks to stay above its name.
    let room = rect.height() - 5.0 - caption.size().y;
    let mut sample = ui.painter().layout_no_wrap("AaBbCc".into(), font.clone(), color);
    if sample.size().y > room {
        font.size *= room / sample.size().y;
        sample = ui.painter().layout_no_wrap("AaBbCc".into(), font, color);
    }
    let p = ui.painter().with_clip_rect(rect.shrink(2.0));
    p.galley(pos2(rect.min.x + 5.0, rect.min.y + 2.0), sample, color);
    p.galley(pos2(rect.min.x + 5.0, rect.max.y - 3.0 - caption.size().y), caption, v.weak_text_color());
    resp.on_hover_text(kind_label(k))
}
