//! Cell styles: the gallery (Home and Format menu), the Style dialog for
//! new and modified styles, and the theme menus of Page Layout.

use apex_sheet::cellstyles::{self, Group, NamedStyle, part};
use apex_sheet::sheet::{THEME_MAJOR, THEME_MINOR};

use super::dialogs::FormatDlg;
use super::*;
use crate::i18n::lookup;

/// Open style dialogs.
#[derive(Default)]
pub(super) struct CellStylesState {
    dlg: Option<StyleDlg>,
    /// Format Cells opened from the Style dialog.
    format: Option<Box<FormatDlg>>,
    /// A style's right-click menu (shown after the gallery closes).
    menu: Option<(String, Pos2)>,
}

/// The Style dialog: a new style (`orig` None) or the one named `orig`.
struct StyleDlg {
    orig: Option<String>,
    builtin: Option<u16>,
    name: String,
    parts: u8,
    style: CellStyle,
    error: bool,
}

enum Action {
    Apply(String),
    Modify(String),
    Duplicate(String),
    Delete(String),
    New,
    /// Right-click on a style: its menu at a screen position.
    Menu(String, Pos2),
}

const GROUPS: [(Group, &str); 6] = [
    (Group::Custom, "cellstyle.group_custom"),
    (Group::GoodBad, "cellstyle.group_good_bad"),
    (Group::Data, "cellstyle.group_data"),
    (Group::Titles, "cellstyle.group_titles"),
    (Group::Themed, "cellstyle.group_themed"),
    (Group::Number, "cellstyle.group_number"),
];

/// Points picked colors at the theme colors they are (see
/// `cellstyles::link_theme`), with the theme of the active workbook.
pub(super) fn link_theme(s: &mut CellStyle) {
    cellstyles::link_theme(s, &crate::themes_ui::active());
}

/// Sets the font picked in a themed font list: a family name, or the
/// theme's headings or body font.
pub(super) fn pick_font(s: &mut CellStyle, pick: &str) {
    let t = crate::themes_ui::active();
    let (font, theme) = match pick {
        apex_doc::theme::MAJOR_TOKEN => (Some(t.fonts.major.clone()), THEME_MAJOR),
        apex_doc::theme::MINOR_TOKEN => (Some(t.fonts.minor.clone()), THEME_MINOR),
        f if f.eq_ignore_ascii_case(default_font()) => (None, 0),
        f => (Some(Arc::from(f)), 0),
    };
    s.font = font;
    s.theme.font = theme;
}

/// A style's name in the interface language (built-in styles are
/// translated, as in Excel).
pub(super) fn display_name(ns: &NamedStyle) -> String {
    let Some(id) = ns.builtin else { return ns.name.to_string() };
    match id {
        16..=19 => format!("{} {}", tr("cellstyle.heading"), id - 15),
        29..=52 => {
            let n = (id - 29) / 4 + 1;
            match (id - 29) % 4 {
                0 => format!("{}{n}", tr("cellstyle.accent")),
                k => format!("{}% - {}{n}", k * 20, tr("cellstyle.accent")),
            }
        }
        _ => lookup(&format!("cellstyle.b{id}")).map_or_else(|| ns.name.to_string(), str::to_string),
    }
}

/// A gallery entry: the name drawn with the style's font, colors and
/// borders.
fn preview(ui: &mut egui::Ui, ns: &NamedStyle, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(100.0, 28.0), Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &display_name(ns), Some(selected));
    let base = if ns.is_normal() || ns.parts & part::FONT != 0 { ns.style.clone() } else { CellStyle::default() };
    let st = &ns.style;
    let p = ui.painter();
    let cell = rect.shrink(2.0);
    let fill = st.fill.filter(|_| ns.parts & part::FILL != 0 || ns.is_normal()).map_or(Color32::WHITE, theme::rgb);
    p.rect_filled(cell, 0.0, fill);
    if ns.parts & part::BORDER != 0 {
        let b = st.borders;
        let edges = [(b.top, cell.left_top(), cell.right_top()), (b.bottom, cell.left_bottom(), cell.right_bottom()), (b.left, cell.left_top(), cell.left_bottom()), (b.right, cell.right_top(), cell.right_bottom())];
        for (l, a, z) in edges {
            if !l.is_none() {
                paint_line(p, a, z, l, l.color.map_or(Color32::BLACK, theme::rgb), true);
            }
        }
    }
    let pt = style_pt(&base).clamp(9.0, 15.0);
    let (font, _) = fonts::font_for(base.font.as_deref(), base.bold, base.italic, pt * 1.1);
    let color = base.color.filter(|_| ns.parts & part::FONT != 0 || ns.is_normal()).map_or(Color32::BLACK, theme::rgb);
    let name = display_name(ns);
    let clip = p.with_clip_rect(cell);
    clip.text(pos2(cell.min.x + 5.0, cell.center().y), egui::Align2::LEFT_CENTER, &name, font, color);
    let v = ui.visuals();
    if selected {
        p.rect_stroke(rect, 2.0, Stroke::new(2.0, theme::ACCENT), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_stroke(rect, 2.0, Stroke::new(1.5, v.widgets.hovered.bg_stroke.color), StrokeKind::Inside);
    }
    resp.on_hover_text(name)
}

impl SheetView {
    /// The Themes, Colors and Fonts menus.
    pub(super) fn theme_menus(&mut self, ui: &mut egui::Ui) {
        let cur = self.wb.cell_styles.theme.clone();
        if let Some(t) = crate::themes_ui::theme_menus(ui, &cur) {
            self.settle_edit();
            self.wb.set_theme(t);
        }
    }

    /// The gallery: styles by group; click applies, right-click offers
    /// Modify, Duplicate and Delete.
    pub(super) fn cell_styles_gallery(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        let cur = self.style_at_cursor().named;
        let list = self.wb.cell_styles.list.clone();
        egui::ScrollArea::vertical().max_height(560.0).show(ui, |ui| {
            for (g, key) in GROUPS {
                let items: Vec<&NamedStyle> = list.iter().filter(|s| cellstyles::group(s.builtin) == g).collect();
                if items.is_empty() {
                    continue;
                }
                ui.label(RichText::new(tr(key)).strong());
                egui::Grid::new(("cell_styles", key)).spacing(vec2(2.0, 2.0)).show(ui, |ui| {
                    for (i, ns) in items.iter().enumerate() {
                        let on = match &cur {
                            Some(n) => n.eq_ignore_ascii_case(&ns.name),
                            None => ns.is_normal(),
                        };
                        let r = preview(ui, ns, on);
                        if r.clicked() {
                            action = Some(Action::Apply(ns.name.to_string()));
                        }
                        if r.secondary_clicked() {
                            let at = r.interact_pointer_pos().unwrap_or(r.rect.center());
                            action = Some(Action::Menu(ns.name.to_string(), at));
                        }
                        if i % 6 == 5 {
                            ui.end_row();
                        }
                    }
                });
                ui.add_space(4.0);
            }
        });
        ui.separator();
        if ui.button(format!("{}…", tr("cellstyle.new"))).clicked() {
            action = Some(Action::New);
        }
        let Some(a) = action else { return };
        ui.close();
        self.settle_edit();
        self.run_style_action(a);
    }

    fn run_style_action(&mut self, a: Action) {
        match a {
            Action::Menu(name, at) => self.cell_styles.menu = Some((name, at)),
            Action::Apply(name) => {
                let r = self.sel();
                self.wb.apply_cell_style(self.sheet, &r, &name);
            }
            Action::Modify(name) | Action::Duplicate(name) if self.wb.cell_styles.get(&name).is_none() => {}
            Action::Modify(name) => {
                let ns = self.wb.cell_styles.get(&name).cloned().unwrap_or_else(|| self.wb.style_from_format(&name, &CellStyle::default()));
                self.cell_styles.dlg = Some(StyleDlg { orig: Some(name), builtin: ns.builtin, name: display_name(&ns), parts: ns.parts, style: ns.style, error: false });
            }
            Action::Duplicate(name) => {
                let ns = self.wb.cell_styles.get(&name).cloned().unwrap_or_else(|| self.wb.style_from_format(&name, &CellStyle::default()));
                let name = self.wb.cell_styles.unique_name(&format!("{} 2", display_name(&ns)));
                self.cell_styles.dlg = Some(StyleDlg { orig: None, builtin: None, name, parts: if ns.is_normal() { part::ALL } else { ns.parts }, style: ns.style, error: false });
            }
            Action::Delete(name) => self.wb.delete_cell_style(&name),
            Action::New => {
                let st = CellStyle { named: None, ..self.style_at_cursor() };
                let name = self.wb.cell_styles.unique_name(tr("cellstyle.default_name"));
                self.cell_styles.dlg = Some(StyleDlg { orig: None, builtin: None, name, parts: part::ALL, style: st, error: false });
            }
        }
    }

    /// The Style dialog and its Format Cells dialog.
    pub(super) fn cell_style_dialogs(&mut self, ctx: &egui::Context) {
        if let Some((name, at)) = self.cell_styles.menu.clone() {
            let normal = self.wb.cell_styles.get(&name).is_some_and(|s| s.is_normal());
            let mut action = None;
            let area = egui::Area::new(Id::new(("cell_style_menu", self.uid))).order(egui::Order::Foreground).fixed_pos(at).show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    ui.set_min_width(140.0);
                    egui::containers::menu::menu_style(ui.style_mut());
                    if ui.button(format!("{}…", tr("cellstyle.modify"))).clicked() {
                        action = Some(Action::Modify(name.clone()));
                    }
                    if ui.button(format!("{}…", tr("cellstyle.duplicate"))).clicked() {
                        action = Some(Action::Duplicate(name.clone()));
                    }
                    if ui.add_enabled(!normal, egui::Button::new(tr("cellstyle.delete"))).clicked() {
                        action = Some(Action::Delete(name.clone()));
                    }
                });
            });
            let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)));
            if action.is_some() || outside || ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.cell_styles.menu = None;
            }
            if let Some(a) = action {
                self.run_style_action(a);
            }
        }
        if let Some(mut f) = self.cell_styles.format.take() {
            match self.format_dialog(ctx, &mut f) {
                widgets::DialogResult::None => self.cell_styles.format = Some(f),
                widgets::DialogResult::Cancel => {}
                widgets::DialogResult::Ok => {
                    if let Some(d) = &mut self.cell_styles.dlg {
                        d.style = CellStyle { named: None, ..f.result() };
                    }
                }
            }
            return;
        }
        let Some(mut d) = self.cell_styles.dlg.take() else { return };
        let mut open_format = false;
        let res = widgets::dialog(ctx, "cell_style", tr("cellstyle.dialog"), 400.0, |ui| style_page(ui, &mut d, &mut open_format));
        if open_format {
            let loc = self.wb.locale;
            self.cell_styles.format = Some(Box::new(FormatDlg::new(d.style.clone(), 0, &loc, false)));
            self.cell_styles.dlg = Some(d);
            return;
        }
        match res {
            widgets::DialogResult::None => self.cell_styles.dlg = Some(d),
            widgets::DialogResult::Cancel => self.focus_grid = true,
            widgets::DialogResult::Ok => {
                let name = d.name.trim().to_string();
                let clash = self.wb.cell_styles.get(&name).is_some_and(|s| d.orig.as_deref().is_none_or(|o| !o.eq_ignore_ascii_case(&s.name)));
                if d.builtin.is_none() && (name.is_empty() || clash) {
                    d.error = true;
                    self.cell_styles.dlg = Some(d);
                    return;
                }
                let name = match (&d.orig, d.builtin) {
                    // Built-in styles keep their (English) file names.
                    (Some(o), Some(_)) => o.clone(),
                    _ => name,
                };
                let def = NamedStyle { name: Arc::from(name.as_str()), builtin: d.builtin, parts: d.parts, style: d.style };
                self.wb.set_cell_style(d.orig.as_deref(), def);
                self.focus_grid = true;
            }
        }
    }
}

/// A summary of what one part of a style sets.
fn part_summary(st: &CellStyle, p: u8) -> String {
    match p {
        part::NUMBER => st.num_fmt.to_string(),
        part::ALIGN => {
            let mut v = Vec::new();
            if st.wrap {
                v.push(tr("sheet.wrap").to_string());
            }
            if st.indent > 0 {
                v.push(format!("{} {}", tr("cellstyle.indent"), st.indent));
            }
            if v.is_empty() { tr("cellstyle.align_default").to_string() } else { v.join(", ") }
        }
        part::FONT => {
            let mut s = format!("{} {}", st.font.as_deref().unwrap_or(default_font()), common::fmt_size(style_pt(st)));
            if st.bold {
                s += &format!(", {}", tr("format.bold"));
            }
            if st.italic {
                s += &format!(", {}", tr("format.italic"));
            }
            if let Some(c) = st.color {
                s += &format!(", #{c:06X}");
            }
            s
        }
        part::BORDER => {
            let b = st.borders;
            if [b.top, b.bottom, b.left, b.right].iter().all(|l| l.is_none()) { tr("cellstyle.no_borders").to_string() } else { tr("cellstyle.borders").to_string() }
        }
        part::FILL => st.fill.map_or_else(|| tr("cellstyle.no_shading").to_string(), |c| format!("#{c:06X}")),
        _ => if st.unlocked { tr("cellstyle.unlocked") } else { tr("cellstyle.locked") }.to_string(),
    }
}

fn style_page(ui: &mut egui::Ui, d: &mut StyleDlg, open_format: &mut bool) {
    widgets::section(ui, tr("cellstyle.sec_style"));
    widgets::form(ui, "cell_style_grid", |ui| {
        widgets::form_row(ui, tr("cellstyle.name").trim_end_matches(':'), |ui| {
            ui.add_enabled(d.builtin.is_none(), egui::TextEdit::singleline(&mut d.name).desired_width(220.0));
        });
    });
    if d.error {
        ui.colored_label(ui.visuals().error_fg_color, tr("cellstyle.bad_name"));
    }
    if ui.button(format!("{}…", tr("cellstyle.format"))).clicked() {
        *open_format = true;
    }
    widgets::section(ui, tr("cellstyle.includes").trim_end_matches(':'));
    let normal = d.builtin == Some(0);
    egui::Grid::new("cell_style_parts").num_columns(2).min_col_width(widgets::FORM_LABEL_W).spacing([12.0, 6.0]).show(ui, |ui| {
        for (p, key) in [(part::NUMBER, "fmtdlg.number"), (part::ALIGN, "fmtdlg.alignment"), (part::FONT, "fmtdlg.font"), (part::BORDER, "fmtdlg.border"), (part::FILL, "fmtdlg.fill"), (part::PROTECT, "fmtdlg.protection")] {
            let mut on = normal || d.parts & p != 0;
            if ui.add_enabled(!normal, egui::Checkbox::new(&mut on, tr(key))).changed() {
                d.parts = if on { d.parts | p } else { d.parts & !p };
            }
            ui.label(RichText::new(part_summary(&d.style, p)).weak());
            ui.end_row();
        }
    });
}
