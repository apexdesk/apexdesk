//! The toolstrip of presentations: Home (clipboard, slides, font, paragraph,
//! drawing), Insert, Design and View, the objects' context menu and the
//! equation dialog. The Design commands (theme, slide size, background)
//! are here too.

use std::sync::Arc;

use apex_doc::Align;
use apex_slides::edit::{AlignTo, Order};
use apex_slides::fill::Background;
use apex_slides::{Fill, Frame, ObjKind, SColor};
use eframe::egui::{self, Color32, Id, Rect, RichText, Sense, Stroke, vec2};

use super::canvas::Tool;
use super::{Mode, SlidesView, render, typing};
use crate::common;
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::toolstrip::{self, Body};
use crate::theme;
use crate::widgets::{self, Face as WFace};

/// Office's slide sizes: 16:9 and 4:3, points.
pub const SIZES: [(&str, (f32, f32)); 2] = [("slides.size_wide", apex_slides::build::WIDE), ("slides.size_standard", apex_slides::build::STANDARD)];

/// Id of the chart object `id` on slide `slide` when its data opens in a tab.
pub fn chart_object_id(slide: u32, id: u32) -> u64 {
    0x5_0000_0000_0000 | ((slide as u64) << 20) | id as u64
}

impl SlidesView {
    // --- design commands -----------------------------------------------------------------

    /// Built-in theme `i` for the current slide's master.
    pub fn set_theme(&mut self, i: usize) {
        if self.read_only {
            return;
        }
        let m = self.pres.slides.get(self.cur).map_or(0, |s| s.master);
        let master = self.pres.edit("slides.undo.theme").master_mut(m);
        master.theme.theme = apex_doc::theme::Theme::builtin(i);
        if let Some(o) = &mut master.theme.origin {
            o.same = false;
        }
    }

    /// A new slide size: everything is scaled to it.
    pub fn set_slide_size(&mut self, w: f32, h: f32) {
        if self.read_only || (w - self.pres.width).abs() < 0.01 && (h - self.pres.height).abs() < 0.01 {
            return;
        }
        let (kx, ky) = (w / self.pres.width.max(1.0), h / self.pres.height.max(1.0));
        let deck = self.pres.edit("slides.undo.size");
        let scale = |v: &mut Vec<apex_slides::Object>| {
            for o in v.iter_mut() {
                if let Some(f) = &mut o.sp.frame {
                    *f = Frame { x: f.x * kx, y: f.y * ky, w: f.w * kx, h: f.h * ky, ..*f };
                    o.raw = None;
                }
            }
        };
        for i in 0..deck.slides.len() {
            scale(&mut deck.slide_mut(i).objects);
        }
        for m in 0..deck.masters.len() {
            scale(&mut deck.master_mut(m).objects);
            for l in 0..deck.masters[m].layouts.len() {
                scale(&mut deck.layout_mut(m, l).objects);
            }
        }
        deck.width = w;
        deck.height = h;
    }

    /// The current slide's background: a color, or the layout's (`None`);
    /// `all`: every slide.
    pub fn set_background(&mut self, c: Option<u32>, all: bool) {
        if self.read_only || self.pres.slides.is_empty() {
            return;
        }
        let bg = c.map(|c| Background::Fill(Fill::Solid(SColor::rgb(c))));
        let cur = self.cur;
        let deck = self.pres.edit("slides.undo.background");
        let range = if all { 0..deck.slides.len() } else { cur..cur + 1 };
        for i in range {
            deck.slide_mut(i).background = bg.clone();
        }
    }

    // --- tabs ----------------------------------------------------------------------------

    pub(super) fn home_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("toolstrip.g.undo", Icon::Undo, |ui| {
            toolstrip::column(ui, |ui| {
                for _ in 0..common::history_button(ui, false, &self.pres.undo_names()) {
                    self.undo();
                }
                for _ in 0..common::history_button(ui, true, &self.pres.redo_names()) {
                    self.redo();
                }
            });
        });
        b.group("toolstrip.g.clipboard", Icon::Paste, |ui| {
            ui.add_enabled_ui(SlidesView::can_paste(), |ui| {
                if toolstrip::large(ui, Icon::Paste, tr("edit.paste"), &format!("{} (Ctrl+V)", tr("edit.paste")), acc, false).on_disabled_hover_text(tr("slides.clip_empty")).clicked() {
                    self.paste_objects();
                }
            });
            toolstrip::column(ui, |ui| {
                let some = !self.selection().is_empty();
                ui.add_enabled_ui(some, |ui| {
                    if toolstrip::small(ui, Some(Icon::Cut), tr("edit.cut"), &format!("{} (Ctrl+X)", tr("edit.cut")), acc, false).clicked()
                        && let Some(t) = self.copy_selected(true)
                    {
                        ui.ctx().copy_text(t);
                    }
                    if toolstrip::small(ui, Some(Icon::Copy), tr("edit.copy"), &format!("{} (Ctrl+C)", tr("edit.copy")), acc, false).clicked()
                        && let Some(t) = self.copy_selected(false)
                    {
                        ui.ctx().copy_text(t);
                    }
                });
                let text = self.typing.is_some() || !self.selection().is_empty();
                ui.add_enabled_ui(text || self.brush.is_some(), |ui| {
                    let r = toolstrip::small(ui, Some(Icon::Brush), tr("toolstrip.brush"), tr("toolbar.brush_hint"), acc, self.brush.is_some());
                    self.brush_button(r.clicked(), r.double_clicked());
                });
            });
        });
        b.group("slides.g.slides", Icon::NewSlide, |ui| self.slides_group(ui, acc));
        // Text formats need text: an object picked or text being typed.
        let text = self.typing.is_some() || !self.selection().is_empty();
        b.group("toolstrip.g.font", Icon::GrowFont, |ui| {
            ui.add_enabled_ui(text, |ui| self.font_group(ui));
        });
        b.group("toolstrip.g.paragraph", Icon::AlignLeft, |ui| {
            ui.add_enabled_ui(text, |ui| self.paragraph_group(ui));
        });
        b.group("slides.g.drawing", Icon::Shapes, |ui| self.drawing_group(ui, acc));
    }

    fn slides_group(&mut self, ui: &mut egui::Ui, acc: Color32) {
        ui.add_enabled_ui(!self.read_only, |ui| {
            let mut pick = None;
            if toolstrip::large_split(ui, Icon::NewSlide, tr("slides.new_slide"), &format!("{} (Ctrl+M)", tr("slides.new_slide")), acc, |ui| {
                pick = self.layout_menu(ui);
            }) {
                self.new_slide();
            }
            if let Some((m, l)) = pick {
                self.new_slide_with(m, l);
            }
            toolstrip::column(ui, |ui| {
                let mut pick = None;
                toolstrip::small_menu(ui, Some(Icon::Columns), tr("slides.layout"), tr("slides.layout_tip"), acc, |ui| {
                    pick = self.layout_menu(ui);
                });
                if let Some((m, l)) = pick {
                    self.set_layout(m, l);
                }
                if toolstrip::small(ui, Some(Icon::Copy), tr("slides.duplicate"), &format!("{} (Ctrl+D)", tr("slides.duplicate")), acc, false).clicked() {
                    self.duplicate();
                }
                ui.add_enabled_ui(self.pres.slides.len() > 1, |ui| {
                    if toolstrip::small(ui, Some(Icon::Eraser), tr("slides.delete"), tr("slides.delete"), acc, false).clicked() {
                        self.delete();
                    }
                });
            });
            toolstrip::column(ui, |ui| {
                let hidden = self.pres.slides.get(self.cur).is_some_and(|s| s.hidden);
                if toolstrip::small(ui, Some(Icon::Hide), tr("slides.hide"), tr("slides.hide_tip"), acc, hidden).clicked() {
                    self.toggle_hidden();
                }
                let (first, last) = (self.cur == 0, self.cur + 1 >= self.pres.slides.len());
                ui.add_enabled_ui(!first, |ui| {
                    if toolstrip::small(ui, Some(Icon::ChevronUp), tr("slides.move_up"), tr("slides.move_up"), acc, false).on_disabled_hover_text(tr("slides.first_slide")).clicked() {
                        self.move_by(-1);
                    }
                });
                ui.add_enabled_ui(!last, |ui| {
                    if toolstrip::small(ui, Some(Icon::ChevronDown), tr("slides.move_down"), tr("slides.move_down"), acc, false).on_disabled_hover_text(tr("slides.last_slide")).clicked() {
                        self.move_by(1);
                    }
                });
            });
        });
    }

    fn font_group(&mut self, ui: &mut egui::Ui) {
        let cur = self.current_format();
        toolstrip::column(ui, |ui| {
            toolstrip::row(ui, |ui| {
                if let Some(f) = common::themed_font_picker(ui, Id::new(("sfont", self.uid)), &cur.font, 130.0) {
                    self.set_font(Some(Arc::from(f.as_str())));
                }
                if let Some(v) = common::size_box(ui, Id::new(("ssize", self.uid)), cur.size) {
                    self.set_font_size(v);
                }
                if toolstrip::tool(ui, WFace::Icon(Icon::GrowFont), false, &format!("{} (Ctrl+Shift+>)", tr("format.grow"))).clicked() {
                    self.grow_font(true);
                }
                if toolstrip::tool(ui, WFace::Icon(Icon::ShrinkFont), false, &format!("{} (Ctrl+Shift+<)", tr("format.shrink"))).clicked() {
                    self.grow_font(false);
                }
            });
            toolstrip::row(ui, |ui| {
                if toolstrip::tool(ui, WFace::Text(RichText::new("B").strong()), cur.bold, &format!("{} (Ctrl+B)", tr("format.bold"))).clicked() {
                    self.toggle_bold();
                }
                if toolstrip::tool(ui, WFace::Text(RichText::new("I").italics()), cur.italic, &format!("{} (Ctrl+I)", tr("format.italic"))).clicked() {
                    self.toggle_italic();
                }
                if toolstrip::tool(ui, WFace::Text(RichText::new("U").underline()), cur.underline, &format!("{} (Ctrl+U)", tr("format.underline"))).clicked() {
                    self.toggle_underline();
                }
                if toolstrip::tool(ui, WFace::Text(RichText::new("S").strikethrough()), cur.strike, tr("format.strike")).clicked() {
                    self.toggle_strike();
                }
                let last = self.last_color;
                let mut chosen = None;
                let bar = last.map_or(Color32::BLACK, theme::rgb);
                if toolstrip::split(ui, WFace::TextBar(RichText::new("A").strong(), bar), false, tr("format.text_color"), |ui| {
                    chosen = widgets::color_menu(ui, tr("format.auto_color"), cur.color);
                }) {
                    chosen = Some(last);
                }
                if let Some(c) = chosen {
                    self.last_color = c;
                    self.set_text_color(c);
                }
                let mut pick = None;
                let face = WFace::Text(RichText::new("A").strong().color(Color32::from_rgb(0x44, 0x72, 0xC4)));
                toolstrip::tool_menu(ui, face, cur.fx.is_some(), tr("art.effects"), |ui| pick = crate::text_fx_ui::menu(ui, cur.fx.as_deref(), true));
                if let Some(p) = pick {
                    self.set_text_fx(p);
                }
            });
        });
    }

    fn paragraph_group(&mut self, ui: &mut egui::Ui) {
        let cur = self.current_format();
        toolstrip::column(ui, |ui| {
            toolstrip::row(ui, |ui| {
                if toolstrip::tool(ui, WFace::Icon(Icon::Bullets), cur.bullet, tr("doc.bullets")).clicked() {
                    self.toggle_bullets();
                }
                if toolstrip::tool(ui, WFace::Icon(Icon::Numbering), false, tr("doc.numbering")).clicked() {
                    self.numbering();
                }
                if toolstrip::tool(ui, WFace::Icon(Icon::Outdent), false, &format!("{} (Shift+Tab)", tr("doc.outdent"))).clicked() {
                    self.indent(-1);
                }
                if toolstrip::tool(ui, WFace::Icon(Icon::Indent), false, &format!("{} (Tab)", tr("doc.indent"))).clicked() {
                    self.indent(1);
                }
            });
            toolstrip::row(ui, |ui| {
                for (al, icon, key) in [(Align::Left, Icon::AlignLeft, "format.align_left"), (Align::Center, Icon::AlignCenter, "format.align_center"), (Align::Right, Icon::AlignRight, "format.align_right"), (Align::Justify, Icon::Justify, "format.align_justify")] {
                    if toolstrip::tool(ui, WFace::Icon(icon), cur.align == al, tr(key)).clicked() {
                        self.set_align(al);
                    }
                }
            });
        });
    }

    fn drawing_group(&mut self, ui: &mut egui::Ui, acc: Color32) {
        ui.add_enabled_ui(!self.read_only, |ui| {
            toolstrip::large_menu(ui, Icon::Shapes, tr("shape.shapes"), tr("slides.shapes_tip"), acc, |ui| {
                if let Some(t) = crate::shapes::gallery(ui) {
                    self.tool = Some(Tool::of(t));
                }
            });
            toolstrip::large_menu(ui, Icon::Group, tr("slides.arrange"), tr("slides.arrange_tip"), acc, |ui| self.arrange_menu(ui));
            toolstrip::large_menu(ui, Icon::Select, tr("toolstrip.select"), tr("toolstrip.select"), acc, |ui| {
                if toolstrip::menu_item_keys(ui, Icon::Select, tr("edit.select_all"), "Ctrl+A").clicked() {
                    self.select_all();
                    ui.close();
                }
                if toolstrip::menu_item_on(ui, Icon::Select, tr("selpane.title"), "", self.pane_open).clicked() {
                    self.pane_open = !self.pane_open;
                    ui.close();
                }
            });
            toolstrip::column(ui, |ui| {
                let some = !self.selection().is_empty();
                ui.add_enabled_ui(some, |ui| {
                    let mut fill = None;
                    toolstrip::small_menu(ui, Some(Icon::Fill), tr("slides.shape_fill"), tr("slides.shape_fill"), acc, |ui| {
                        if ui.button(tr("slides.no_fill")).clicked() {
                            fill = Some(Some(None));
                            ui.close();
                        }
                        if let Some(c) = widgets::color_menu(ui, tr("slides.style_fill"), None) {
                            fill = Some(c.map(Some));
                        }
                        ui.separator();
                        let (cur, cur_grad) = self.fill_look();
                        crate::widgets::menu_button(ui, tr("grad.gradient"), |ui| {
                            if let Some(g) = crate::gradient_ui::gallery(ui, cur.unwrap_or(apex_doc::SHAPE_FILL), cur_grad.as_ref()) {
                                match g {
                                    Some(g) => self.set_fill_look(Some(g.main_color()), Some(g)),
                                    None => self.set_fill_look(Some(cur_grad.as_ref().map_or(cur.unwrap_or(apex_doc::SHAPE_FILL), |g| g.stops[0].color)), None),
                                }
                                ui.close();
                            }
                            ui.separator();
                            if crate::toolstrip::menu_item(ui, crate::icons::Icon::Fill, &format!("{}…", tr("grad.more"))).clicked() {
                                self.open_fill_dialog(super::fill_dlg::FillTarget::Shapes);
                                ui.close();
                            }
                        });
                    });
                    if let Some(f) = fill {
                        self.set_fill(f);
                    }
                    let mut line = None;
                    toolstrip::small_menu(ui, Some(Icon::Pen), tr("slides.shape_outline"), tr("slides.shape_outline"), acc, |ui| {
                        if ui.button(tr("slides.no_outline")).clicked() {
                            line = Some(Some(None));
                            ui.close();
                        }
                        crate::widgets::menu_button(ui, tr("slides.outline_weight"), |ui| {
                            for w in [0.25f32, 0.5, 0.75, 1.0, 1.5, 2.25, 3.0, 4.5, 6.0] {
                                if ui.button(format!("{w} pt")).clicked() {
                                    self.set_outline_width(w);
                                    ui.close();
                                }
                            }
                        });
                        if let Some(c) = widgets::color_menu(ui, tr("slides.style_outline"), None) {
                            line = Some(c.map(Some));
                        }
                    });
                    if let Some(l) = line {
                        self.set_line_color(l);
                    }
                });
            });
        });
    }

    /// Order, group, align, turn and flip the selection.
    pub(super) fn arrange_menu(&mut self, ui: &mut egui::Ui) {
        let n = self.selection().len();
        if toolstrip::menu_item_on(ui, Icon::Select, tr("selpane.title"), "", self.pane_open).clicked() {
            self.pane_open = !self.pane_open;
            ui.close();
        }
        ui.separator();
        ui.add_enabled_ui(n > 0, |ui| {
            for (how, key) in [(Order::Front, "shape.front"), (Order::Forward, "shape.forward"), (Order::Backward, "shape.backward"), (Order::Back, "shape.back")] {
                if ui.button(tr(key)).clicked() {
                    self.order(how);
                    ui.close();
                }
            }
            ui.separator();
            if ui.add_enabled(n > 1, egui::Button::new(format!("{} (Ctrl+G)", tr("slides.group")))).clicked() {
                self.group_selected();
                ui.close();
            }
            let group = self.selected_object().is_some_and(|o| matches!(o.kind, ObjKind::Group(_)));
            if ui.add_enabled(group, egui::Button::new(format!("{} (Ctrl+Shift+G)", tr("slides.ungroup")))).clicked() {
                self.ungroup_selected();
                ui.close();
            }
            ui.separator();
            crate::widgets::menu_button(ui, tr("slides.align"), |ui| {
                for (how, key) in [
                    (AlignTo::Left, "slides.align_left"),
                    (AlignTo::Center, "slides.align_center"),
                    (AlignTo::Right, "slides.align_right"),
                    (AlignTo::Top, "slides.align_top"),
                    (AlignTo::Middle, "slides.align_middle"),
                    (AlignTo::Bottom, "slides.align_bottom"),
                ] {
                    if ui.button(tr(key)).clicked() {
                        self.align_selected(how);
                        ui.close();
                    }
                }
                ui.separator();
                for (how, key) in [(AlignTo::SpreadH, "slides.distribute_h"), (AlignTo::SpreadV, "slides.distribute_v")] {
                    if ui.add_enabled(n > 2, egui::Button::new(tr(key))).clicked() {
                        self.align_selected(how);
                        ui.close();
                    }
                }
            });
            crate::widgets::menu_button(ui, tr("slides.rotate"), |ui| {
                if ui.button(tr("slides.rotate_right")).clicked() {
                    self.rotate_selected(90.0);
                    ui.close();
                }
                if ui.button(tr("slides.rotate_left")).clicked() {
                    self.rotate_selected(-90.0);
                    ui.close();
                }
                if ui.button(tr("slides.flip_v")).clicked() {
                    self.flip_selected(false);
                    ui.close();
                }
                if ui.button(tr("slides.flip_h")).clicked() {
                    self.flip_selected(true);
                    ui.close();
                }
            });
        });
    }

    /// The objects' context menu on the canvas.
    pub(super) fn object_menu(&mut self, ui: &mut egui::Ui) {
        let some = !self.selection().is_empty();
        if ui.add_enabled(some, egui::Button::new(tr("edit.cut"))).clicked() {
            if let Some(t) = self.copy_selected(true) {
                ui.ctx().copy_text(t);
            }
            ui.close();
        }
        if ui.add_enabled(some, egui::Button::new(tr("edit.copy"))).clicked() {
            if let Some(t) = self.copy_selected(false) {
                ui.ctx().copy_text(t);
            }
            ui.close();
        }
        if ui.add_enabled(SlidesView::can_paste(), egui::Button::new(tr("edit.paste"))).clicked() {
            self.paste_objects();
            ui.close();
        }
        if ui.add_enabled(some, egui::Button::new(tr("slides.delete_objects"))).clicked() {
            self.delete_selected();
            ui.close();
        }
        ui.separator();
        if some {
            self.arrange_menu(ui);
        } else {
            if ui.button(tr("slides.new_slide")).clicked() {
                self.new_slide();
                ui.close();
            }
            crate::widgets::menu_button(ui, tr("slides.layout"), |ui| {
                if let Some((m, l)) = self.layout_menu(ui) {
                    self.set_layout(m, l);
                }
            });
        }
    }

    pub(super) fn insert_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("slides.g.slides", Icon::NewSlide, |ui| {
            ui.add_enabled_ui(!self.read_only, |ui| {
                let mut pick = None;
                if toolstrip::large_split(ui, Icon::NewSlide, tr("slides.new_slide"), &format!("{} (Ctrl+M)", tr("slides.new_slide")), acc, |ui| {
                    pick = self.layout_menu(ui);
                }) {
                    self.new_slide();
                }
                if let Some((m, l)) = pick {
                    self.new_slide_with(m, l);
                }
            });
        });
        b.group("toolstrip.g.tables", Icon::Table, |ui| {
            toolstrip::large_menu(ui, Icon::Table, tr("table.table"), tr("slides.table_tip"), acc, |ui| {
                if let Some((r, c)) = crate::doc_view::table_ui::grid_picker(ui) {
                    self.insert_table(r, c);
                    ui.close();
                }
            });
        });
        b.group("toolstrip.g.illustrations", Icon::Picture, |ui| {
            if toolstrip::large(ui, Icon::Picture, tr("pic.insert"), tr("slides.picture_tip"), acc, false).clicked() {
                self.pick_picture();
            }
            if crate::library::available() && toolstrip::large(ui, Icon::Folder, tr("library.menu"), tr("library.tip"), acc, false).clicked() {
                self.library.show();
            }
            toolstrip::large_menu(ui, Icon::Shapes, tr("shape.shapes"), tr("slides.shapes_tip"), acc, |ui| {
                if let Some(t) = crate::shapes::gallery(ui) {
                    self.tool = Some(Tool::of(t));
                }
            });
            toolstrip::large_menu(ui, Icon::Chart, tr("chart.insert"), tr("slides.chart_tip"), acc, |ui| {
                if let Some(i) = crate::sheet_view::chart_type_menu(ui) {
                    self.insert_chart_type(i);
                }
            });
        });
        b.group("slides.media.g.media", Icon::Video, |ui| {
            ui.add_enabled_ui(!self.read_only, |ui| {
                if toolstrip::large(ui, Icon::Video, tr("slides.media.video"), tr("slides.media.video_tip"), acc, false).clicked() {
                    self.pick_media(true);
                }
                if toolstrip::large(ui, Icon::Audio, tr("slides.media.audio"), tr("slides.media.audio_tip"), acc, false).clicked() {
                    self.pick_media(false);
                }
            });
        });
        b.group("slides.g.text", Icon::TextBox, |ui| {
            // Pressed again, it lets go.
            let on = self.tool == Some(Tool::TextBox);
            if toolstrip::large(ui, Icon::TextBox, tr("slides.text_box"), tr("slides.text_box_tip"), acc, on).clicked() {
                self.tool = if on { None } else { Some(Tool::TextBox) };
            }
            let cur = self.current_format().fx;
            let mut pick = None;
            toolstrip::large_menu(ui, Icon::TextArt, tr("art.styles"), tr("art.styles_tip"), acc, |ui| pick = crate::text_fx_ui::gallery(ui, cur.as_deref()));
            if let Some(fx) = pick {
                self.text_style_picked(fx);
            }
        });
        b.group("slides.g.symbols", Icon::Equation, |ui| {
            if toolstrip::large(ui, Icon::Equation, tr("slides.equation"), tr("slides.equation_tip"), acc, false).clicked() {
                self.equation = Some((None, String::new(), 0, true));
            }
            if toolstrip::large(ui, Icon::Symbol, tr("symbol.menu"), tr("slides.symbol_tip"), acc, false).clicked() {
                self.symbols.open();
            }
        });
    }

    /// Asks for a picture file and puts it on the slide.
    pub fn pick_picture(&mut self) {
        crate::file_dialog::FileDialog::new().set_title(tr("pic.insert")).add_filter(tr("pic.files"), &["png", "jpg", "jpeg", "gif", "bmp", "webp"]).pick_file(crate::file_dialog::in_view(|v: &mut SlidesView, path: std::path::PathBuf| v.insert_picture_file(&path)));
    }

    /// Puts the picture file `path` on the slide (a notice if it isn't one).
    pub(crate) fn insert_picture_file(&mut self, path: &std::path::Path) {
        match std::fs::read(path) {
            Ok(data) => {
                if !self.insert_picture_bytes(data) {
                    self.notices.push(crate::app::Notice::error(trf("pic.bad_file", &[("file", &path.display().to_string())])));
                }
            }
            Err(e) => self.notices.push(crate::app::Notice::error(e.to_string())),
        }
    }

    /// Insert > Chart: a chart of type `CHART_TYPES[i]` with sample data.
    pub fn insert_chart_type(&mut self, i: usize) {
        let (key, _, kind, grouping, flags) = crate::sheet_view::CHART_TYPES[i];
        let mut wb = apex_io::chart_object::sample(kind, tr("chart.sample_category"), tr("chart.sample_series"));
        if let Some(c) = wb.sheets[0].charts.first_mut() {
            crate::sheet_view::apply_chart_type(c, grouping, flags, key == "chart.t.combo");
        }
        let book = apex_io::chart_object::book(&wb);
        if let Some(id) = self.insert_chart(book) {
            self.open_chart_data(id);
        }
    }

    /// Opens the data of chart `id` of the current slide in a tab.
    pub fn open_chart_data(&mut self, id: u32) {
        let Some(s) = self.pres.slides.get(self.cur) else { return };
        if self.mode == Mode::Master {
            return;
        }
        let Some(o) = s.objects.iter().find(|o| o.id == id) else { return };
        if let ObjKind::Chart(c) = &o.kind {
            let e = apex_doc::Embedded { id: chart_object_id(s.id, id), kind: apex_doc::EmbedKind::Chart(c.clone()) };
            self.object_request = Some(crate::objects::Request::Open(Arc::new(e)));
        }
    }

    /// A chart to open, for the app.
    pub fn take_object_request(&mut self) -> Option<crate::objects::Request> {
        self.object_request.take()
    }

    /// Puts back a chart edited in its tab. False if it is gone.
    pub fn replace_chart(&mut self, e: &apex_doc::Embedded) -> bool {
        let apex_doc::EmbedKind::Chart(c) = &e.kind else { return false };
        for i in 0..self.pres.slides.len() {
            let s = &self.pres.slides[i];
            if let Some(o) = s.objects.iter().find(|o| chart_object_id(s.id, o.id) == e.id && matches!(o.kind, ObjKind::Chart(_))) {
                let id = o.id;
                if let Some(o) = self.pres.edit("slides.undo.chart").object_mut(i, id) {
                    o.kind = ObjKind::Chart(c.clone());
                    o.touch();
                }
                return true;
            }
        }
        false
    }

    pub(super) fn design_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("slides.g.themes", Icon::Themes, |ui| {
            ui.add_enabled_ui(!self.read_only, |ui| {
                let now = self.pres.slides.get(self.cur).map(|s| self.pres.masters[s.master].theme.theme.name.to_string()).unwrap_or_default();
                for (i, t) in apex_doc::theme::themes().iter().enumerate().take(6) {
                    if theme_swatch(ui, t, *t.name == *now).clicked() {
                        self.set_theme(i);
                    }
                }
                toolstrip::large_menu(ui, Icon::Themes, tr("slides.more_themes"), tr("slides.themes_tip"), acc, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.set_max_width(420.0);
                        for (i, t) in apex_doc::theme::themes().iter().enumerate() {
                            if theme_swatch(ui, t, *t.name == *now).clicked() {
                                self.set_theme(i);
                                ui.close();
                            }
                        }
                    });
                });
                if crate::license::has("management") && toolstrip::large(ui, Icon::Template, tr("corp_tpl.button"), tr("corp_tpl.tip"), acc, false).clicked() {
                    self.open_company_template();
                }
            });
        });
        b.group("slides.g.customize", Icon::PageSize, |ui| {
            ui.add_enabled_ui(!self.read_only, |ui| {
                toolstrip::large_menu(ui, Icon::PageSize, tr("slides.slide_size"), tr("slides.slide_size_tip"), acc, |ui| {
                    for (key, (w, h)) in SIZES {
                        let on = (self.pres.width - w).abs() < 0.5 && (self.pres.height - h).abs() < 0.5;
                        if ui.add(egui::Button::selectable(on, tr(key))).clicked() {
                            self.set_slide_size(w, h);
                            ui.close();
                        }
                    }
                });
                let mut pick = None;
                toolstrip::large_menu(ui, Icon::PageColor, tr("slides.background"), tr("slides.background_tip"), acc, |ui| {
                    if let Some(c) = widgets::color_menu(ui, tr("slides.background_reset"), None) {
                        pick = Some(c);
                    }
                    ui.separator();
                    let (cur, cur_grad) = self.background_look();
                    crate::widgets::menu_button(ui, tr("grad.gradient"), |ui| {
                        if let Some(Some(g)) = crate::gradient_ui::gallery(ui, cur.unwrap_or(apex_doc::SHAPE_FILL), cur_grad.as_ref()) {
                            self.set_background_look(Some(g.main_color()), Some(g), false);
                            ui.close();
                        }
                    });
                    if toolstrip::menu_item(ui, Icon::PageColor, &format!("{}…", tr("slides.format_background"))).clicked() {
                        self.open_fill_dialog(super::fill_dlg::FillTarget::Background);
                        ui.close();
                    }
                });
                if let Some(c) = pick {
                    self.set_background(c, false);
                }
                if toolstrip::large(ui, Icon::PageColor, tr("slides.background_all"), tr("slides.background_all_tip"), acc, false).clicked() {
                    let bg = self.pres.slides.get(self.cur).and_then(|s| match &s.background {
                        Some(Background::Fill(Fill::Solid(c))) => Some(c.clone()),
                        _ => None,
                    });
                    let cur = self.cur;
                    let deck = self.pres.edit("slides.undo.background");
                    let b = deck.slides[cur].background.clone();
                    for i in 0..deck.slides.len() {
                        deck.slide_mut(i).background = b.clone();
                    }
                    let _ = bg;
                }
            });
        });
    }

    pub(crate) fn view_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("slides.g.views", Icon::Slides, |ui| {
            for (mode, icon, key, tip) in [
                (Mode::Normal, Icon::PrintLayout, "slides.view_normal", "slides.view_normal_tip"),
                (Mode::Outline, Icon::Outline, "slides.view_outline", "slides.view_outline_tip"),
                (Mode::Sorter, Icon::MultiPage, "slides.view_sorter", "slides.view_sorter_tip"),
            ] {
                if toolstrip::large(ui, icon, tr(key), tr(tip), acc, self.mode == mode).clicked() {
                    self.set_mode(mode);
                }
            }
        });
        b.group("slides.g.master", Icon::Template, |ui| {
            if toolstrip::large(ui, Icon::Template, tr("slides.view_master"), tr("slides.view_master_tip"), acc, self.mode == Mode::Master).clicked() {
                self.set_mode(if self.mode == Mode::Master { Mode::Normal } else { Mode::Master });
            }
        });
        b.group("slides.g.show", Icon::Slides, |ui| {
            if toolstrip::large(ui, Icon::Note, tr("slides.notes"), tr("slides.notes_tip"), acc, self.show_notes).clicked() {
                self.show_notes = !self.show_notes;
            }
        });
        b.group("toolstrip.g.zoom", Icon::Zoom100, |ui| {
            let before = self.zoom;
            common::zoom_group(ui, &mut self.zoom, acc);
            if self.zoom != before {
                self.fit = false;
            }
            if toolstrip::large(ui, Icon::Fullscreen, tr("slides.fit"), tr("slides.fit_tip"), acc, self.fit).clicked() {
                self.fit = true;
            }
        });
    }

    /// The layouts of every master, to pick one.
    pub(super) fn layout_menu(&self, ui: &mut egui::Ui) -> Option<(usize, usize)> {
        let mut out = None;
        for (m, master) in self.pres.masters.iter().enumerate() {
            if self.pres.masters.len() > 1 {
                ui.label(RichText::new(if master.name.is_empty() { trf("slides.master_n", &[("n", &(m + 1))]) } else { master.name.clone() }).strong());
            }
            // A grid of layout previews, three to a row.
            let size = vec2(112.0, 112.0 * self.pres.height / self.pres.width.max(1.0));
            for row in master.layouts.chunks(3).enumerate() {
                ui.horizontal(|ui| {
                    for (k, lay) in row.1.iter().enumerate() {
                        let l = row.0 * 3 + k;
                        let name = if lay.name.is_empty() { lay.kind.to_string() } else { lay.name.clone() };
                        let (rect, resp) = ui.allocate_exact_size(size + vec2(0.0, 18.0), Sense::click());
                        crate::widgets::name(&resp, egui::WidgetType::Button, &name, None);
                        let thumb = Rect::from_min_size(rect.min, size);
                        render::paint_layout(ui.painter(), &self.pres.deck, m, Some(l), thumb, render::Opts { edit: true, ..Default::default() });
                        let v = ui.visuals();
                        let stroke = if resp.hovered() { Stroke::new(2.0, v.selection.stroke.color) } else { Stroke::new(1.0, Color32::from_gray(160)) };
                        ui.painter().rect_stroke(thumb, 0.0, stroke, egui::StrokeKind::Outside);
                        let color = v.text_color();
                        let label = ui.painter().layout(name.clone(), egui::FontId::proportional(11.0), color, size.x);
                        ui.painter().galley(egui::pos2(rect.center().x - label.size().x.min(size.x) / 2.0, thumb.max.y + 3.0), label, color);
                        if resp.on_hover_text(&name).clicked() {
                            out = Some((m, l));
                            ui.close();
                        }
                    }
                });
            }
        }
        out
    }

    /// Insert > Symbol: the symbol goes in the text being typed, else at
    /// the end of the selected shape's text, else in a new text box.
    pub(super) fn symbols_dialog(&mut self, ctx: &egui::Context) {
        let Some(s) = self.symbols.show(ctx) else { return };
        if self.read_only {
            return;
        }
        if self.typing.is_none() {
            match self.selection()[..] {
                [id] if self.cur_objects().iter().any(|o| o.id == id && typing::can_type(o)) => self.start_typing(id),
                _ => {
                    let frame = self.centered(72.0, 40.0);
                    self.insert_text_box(frame);
                }
            }
        }
        if self.typing.is_some() {
            self.type_text(&s);
        }
    }

    /// The equation dialog: its linear form, typed, and a preview.
    pub(super) fn equation_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, mut src, mut cursor, mut first)) = self.equation.take() else { return };
        let title = if id.is_some() { tr("eq.edit") } else { tr("eq.insert") };
        let field = egui::Id::new(("slides_eq_text", self.uid));
        let r = widgets::dialog(ctx, "slides_equation", title, 520.0, |ui| {
            crate::doc_view::math_ui::editor(ui, field, &mut src, &mut cursor, &mut first, true);
        });
        if !matches!(r, widgets::DialogResult::None) {
            ctx.memory_mut(|m| m.surrender_focus(field));
        }
        let src = src.trim().to_string();
        match r {
            widgets::DialogResult::Ok if src.is_empty() => {}
            widgets::DialogResult::Ok => match id {
                Some(id) => self.set_equation(id, &src),
                None => {
                    self.insert_equation(&src);
                }
            },
            widgets::DialogResult::Cancel => {}
            widgets::DialogResult::None => self.equation = Some((id, src, cursor, first)),
        }
    }
}

/// A theme's colors and name, to pick it.
fn theme_swatch(ui: &mut egui::Ui, t: &apex_doc::theme::Theme, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(64.0, 58.0), Sense::click());
    widgets::name(&resp, egui::WidgetType::Button, &t.name, Some(on));
    let p = ui.painter();
    let inner = rect.shrink(3.0);
    // Light background, dark text, then the six accents.
    p.rect_filled(inner, 2.0, theme::rgb(t.color(apex_doc::theme::slot::LT1)));
    p.text(inner.min + vec2(5.0, 3.0), egui::Align2::LEFT_TOP, "Aa", egui::FontId::proportional(16.0), theme::rgb(t.color(apex_doc::theme::slot::DK1)));
    let w = (inner.width() - 8.0) / 6.0;
    for k in 0..6 {
        let r = Rect::from_min_size(egui::pos2(inner.min.x + 4.0 + k as f32 * w, inner.max.y - 12.0), vec2(w - 1.0, 8.0));
        p.rect_filled(r, 0.0, theme::rgb(t.color(4 + k as u8)));
    }
    let stroke = if on { Stroke::new(2.5, theme::SLIDES_ACCENT) } else if resp.hovered() { Stroke::new(1.5, Color32::from_gray(120)) } else { Stroke::new(1.0, Color32::from_gray(180)) };
    p.rect_stroke(inner, 2.0, stroke, egui::StrokeKind::Outside);
    resp.on_hover_text(t.name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_commands() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        v.set_theme(4);
        assert_eq!(v.pres.masters[0].theme.theme.name, apex_doc::theme::Theme::builtin(4).name);
        let id = v.insert_shape("rect", Frame::new(96.0, 54.0, 96.0, 54.0), false).unwrap();
        v.set_slide_size(720.0, 540.0);
        assert_eq!((v.pres.width, v.pres.height), (720.0, 540.0));
        let f = v.frames_of(&[id])[0].1;
        assert_eq!((f.x, f.w, f.y), (72.0, 72.0, 54.0));
        v.new_slide();
        v.go(0);
        v.set_background(Some(0x112233), false);
        assert!(v.pres.slides[0].background.is_some() && v.pres.slides[1].background.is_none());
        v.set_background(None, true);
        assert!(v.pres.slides.iter().all(|s| s.background.is_none()));
        v.undo();
        v.undo();
        v.undo();
        v.undo();
        assert_eq!(v.pres.width, 960.0);
    }

    #[test]
    fn charts_open_and_come_back() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let (_, _, kind, ..) = crate::sheet_view::CHART_TYPES[0];
        let wb = apex_io::chart_object::sample(kind, "C", "S");
        let id = v.insert_chart(apex_io::chart_object::book(&wb)).unwrap();
        v.open_chart_data(id);
        let Some(crate::objects::Request::Open(e)) = v.take_object_request() else { panic!("no request") };
        assert!(v.replace_chart(&e));
        let other = apex_doc::Embedded { id: 1, kind: e.kind.clone() };
        assert!(!v.replace_chart(&other));
    }
}
