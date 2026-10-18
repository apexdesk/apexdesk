//! Styles: the Styles pane (a gallery of paragraph and character styles
//! drawn in their own format), Modify Style and New Style dialogs.

use apex_doc::{CharFormat, ParaBorders, StyleDef, Styles, TabStops};

use super::layout_ui::{BordersDlg, TabsDlg, borders_page, tabs_page};

use super::*;
use crate::widgets::DialogResult;
use crate::widgets::FitWindow as _;

/// Editable style settings (sizes in points).
#[derive(Clone, PartialEq)]
struct Vals {
    /// Empty for the document font.
    font: String,
    size: f32,
    bold: bool,
    italic: bool,
    underline: bool,
    color: Option<u32>,
    align: Align,
    before: f32,
    after: f32,
    line: f32,
    left: f32,
    first: f32,
}

impl Vals {
    fn new(cf: &CharFormat, st: &ParaStyle) -> Self {
        let (before, after) = st.spacing();
        Vals {
            font: cf.font.as_deref().unwrap_or_default().to_string(),
            size: cf.size as f32 / 2.0,
            bold: cf.bold,
            italic: cf.italic,
            underline: cf.underline,
            color: cf.color,
            align: st.align,
            before,
            after,
            line: if st.line_rule == LineRule::Multiple && st.line_spacing > 0.0 { st.line_spacing } else { 1.0 },
            left: st.indent_left,
            first: st.first_line,
        }
    }
}

pub(super) struct StyleDlg {
    def: StyleDef,
    name: String,
    orig: Vals,
    cur: Vals,
    /// A new style (else modifying `def`).
    new: bool,
    /// New paragraph style: apply it to the selected paragraphs.
    apply: bool,
    error: bool,
    /// 0 general, 1 borders and shading, 2 tabs.
    page: u8,
    borders: BordersDlg,
    orig_borders: (ParaBorders, Option<u32>),
    tabs: TabsDlg,
    orig_tabs: TabStops,
    /// Choices for the next style: id (`None`: automatic) and name.
    nexts: Vec<(Option<Arc<str>>, String)>,
}

#[derive(Default)]
pub(super) struct StylesState {
    pub(super) pane: bool,
    /// The Reveal Formatting pane is open.
    pub(super) reveal: bool,
    pub(super) dlg: Option<Box<StyleDlg>>,
}

/// One entry of the gallery.
#[derive(Clone)]
enum Entry {
    Kind(ParaKind),
    Custom(u16),
    Char(Arc<str>),
}

/// Built-in styles shown in the gallery (lists have their own buttons).
const GALLERY: [ParaKind; 13] = [
    ParaKind::Normal,
    ParaKind::Title,
    ParaKind::Heading(1),
    ParaKind::Heading(2),
    ParaKind::Heading(3),
    ParaKind::Heading(4),
    ParaKind::Heading(5),
    ParaKind::Heading(6),
    ParaKind::Heading(7),
    ParaKind::Heading(8),
    ParaKind::Heading(9),
    ParaKind::Quote,
    ParaKind::Code,
];

/// A paragraph style of a kind and custom number, with the styles' paragraph
/// formatting applied.
fn styled(styles: &Styles, kind: ParaKind, custom: u16) -> ParaStyle {
    let mut st = ParaStyle { kind, custom, ..Default::default() };
    if let Some(d) = styles.builtin(kind) {
        d.apply_para(&mut st);
    }
    if let Some(d) = styles.custom(custom) {
        d.apply_para(&mut st);
    }
    st
}

impl DocView {
    fn style_dlg(&self, def: StyleDef, name: String, orig: Vals, cur: Vals, new: bool, deco: (&ParaStyle, &ParaStyle)) -> Box<StyleDlg> {
        let (o, c) = deco;
        let mut nexts = vec![(None, tr("style.next_auto").to_string())];
        nexts.extend(GALLERY.iter().map(|k| (Some(Arc::from(apex_doc::kind_id(*k))), kind_label(*k).to_string())));
        nexts.extend(self.doc.styles.custom_para_styles().map(|(_, d)| (Some(d.id.clone()), d.name.to_string())));
        let apply = new && !def.char_style;
        Box::new(StyleDlg { def, name, orig, cur, new, apply, error: false, page: 0, borders: BordersDlg::new(&c.borders, c.shading), orig_borders: (o.borders, o.shading), tabs: TabsDlg::new(&c.tabs), orig_tabs: o.tabs, nexts })
    }

    fn entry_name(&self, e: &Entry) -> String {
        match e {
            Entry::Kind(k) => kind_label(*k).to_string(),
            Entry::Custom(n) => self.doc.styles.custom(*n).map(|d| d.name.to_string()).unwrap_or_default(),
            Entry::Char(id) => self.doc.styles.by_id(id).map(|d| d.name.to_string()).unwrap_or_default(),
        }
    }

    fn entry_format(&self, e: &Entry) -> CharFormat {
        match e {
            Entry::Kind(k) => self.doc.char_format(&ParaStyle { kind: *k, ..Default::default() }, &Attrs::default()),
            Entry::Custom(n) => {
                let kind = self.doc.styles.custom(*n).map_or(ParaKind::Normal, |d| d.kind);
                self.doc.char_format(&ParaStyle { kind, custom: *n, ..Default::default() }, &Attrs::default())
            }
            Entry::Char(id) => self.doc.char_format(&ParaStyle::default(), &Attrs { char_style: Some(id.clone()), ..Default::default() }),
        }
    }

    /// Applies a paragraph style (built-in kind or custom) to the selected
    /// paragraphs.
    /// A linked style on part of a paragraph formats only the selected text.
    pub(super) fn apply_style(&mut self, kind: ParaKind, custom: u16) {
        let (a, b) = self.para_range();
        self.doc.apply_style(a, b, self.sel, kind, custom);
    }

    /// Whether applying a style would format only the selected text (a
    /// linked style on part of a paragraph).
    pub(super) fn linked_partial(&self, kind: ParaKind, custom: u16) -> bool {
        let (a, b) = (self.sel.start(), self.sel.end());
        !self.sel.is_empty() && a.para == b.para && (a.offset > 0 || b.offset < self.doc.paragraphs[a.para].text.len()) && self.doc.styles.is_linked(kind, custom)
    }

    fn apply_entry(&mut self, e: &Entry) {
        match e {
            Entry::Kind(k) => self.apply_style(*k, 0),
            Entry::Custom(n) => self.apply_style(ParaKind::Normal, *n),
            Entry::Char(id) => self.apply_char_style(Some(id.clone())),
        }
    }

    fn apply_char_style(&mut self, id: Option<Arc<str>>) {
        if self.sel.is_empty() {
            let mut a = self.attrs_at_caret();
            a.char_style = id;
            self.pending = Some(a);
        } else {
            self.doc.apply_char_style(self.sel, id);
        }
    }

    /// Custom paragraph styles for the style lists (number, name).
    pub(super) fn custom_styles(&self) -> Vec<(u16, String)> {
        self.doc.styles.custom_para_styles().map(|(n, d)| (n, d.name.to_string())).collect()
    }

    /// Opens Modify Style for the style of the caret paragraph.
    pub(super) fn modify_current_style(&mut self) {
        let st = self.cur_para().style;
        let e = if self.doc.styles.custom(st.custom).is_some() { Entry::Custom(st.custom) } else { Entry::Kind(st.kind) };
        self.open_modify(&e);
    }

    fn open_modify(&mut self, e: &Entry) {
        let styles = &self.doc.styles;
        let (def, st) = match e {
            Entry::Kind(k) => {
                let def = styles.builtin(*k).cloned().unwrap_or_else(|| StyleDef { id: apex_doc::kind_id(*k).into(), name: kind_label(*k).into(), kind: *k, builtin: true, ..Default::default() });
                (def, styled(styles, *k, 0))
            }
            Entry::Custom(n) => {
                let Some(def) = styles.custom(*n).cloned() else { return };
                let st = styled(styles, def.kind, *n);
                (def, st)
            }
            Entry::Char(id) => {
                let Some(def) = styles.by_id(id).cloned() else { return };
                (def, ParaStyle::default())
            }
        };
        let v = Vals::new(&self.entry_format(e), &st);
        let name = def.name.to_string();
        self.styles.dlg = Some(self.style_dlg(def, name, v.clone(), v, false, (&st, &st)));
    }

    /// New Style from the formatting at the caret: a paragraph style (with
    /// the paragraph formatting) or a character style.
    pub(super) fn open_new_style(&mut self, char_style: bool) {
        let st = self.cur_para().style;
        let a = self.attrs_at_caret();
        let kind = if st.kind.is_list() { ParaKind::Normal } else { st.kind };
        // Differences are measured from what the style would be based on.
        let base_st = if char_style { st } else { ParaStyle { kind, ..Default::default() } };
        let base_attrs = Attrs { char_style: None, ..Default::default() };
        let orig = Vals::new(&self.doc.char_format(&base_st, &base_attrs), &base_st);
        let cur_st = if char_style { st } else { ParaStyle { kind, custom: 0, ..st } };
        let cur = Vals::new(&self.doc.char_format(&cur_st, &a), &cur_st);
        let base = if char_style { tr("style.new_char_name") } else { tr("style.new_name") };
        let names: Vec<String> = self.doc.styles.defs.iter().map(|d| d.name.to_string()).collect();
        let name = (1..).map(|n| format!("{base} {n}")).find(|n| !names.contains(n)).unwrap_or_default();
        let def = StyleDef { char_style, kind, ..Default::default() };
        self.styles.dlg = Some(self.style_dlg(def, name, orig, cur, true, (&base_st, &cur_st)));
    }

    /// The Styles pane: a window at the right of the view.
    pub(super) fn styles_pane(&mut self, ctx: &egui::Context) {
        if !self.styles.pane {
            return;
        }
        let mut open = true;
        let st = self.cur_para().style;
        let caret_cs = self.attrs_at_caret().char_style;
        let mut entries: Vec<Entry> = GALLERY.iter().map(|k| Entry::Kind(*k)).collect();
        entries.extend(self.doc.styles.custom_para_styles().map(|(n, _)| Entry::Custom(n)));
        let chars: Vec<Entry> = self.doc.styles.char_styles().map(|d| Entry::Char(d.id.clone())).collect();
        let mut action: Option<(u8, Entry)> = None;
        let right = ctx.content_rect().right_top() + vec2(-250.0, 110.0);
        egui::Window::new(tr("style.pane")).id(Id::new(("styles_pane", self.uid))).open(&mut open).default_pos(right).default_width(230.0).resizable(true).collapsible(false).show_fit(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
                let mut row = |ui: &mut egui::Ui, e: &Entry, current: bool| {
                    let cf = self.entry_format(e);
                    let size = (cf.size as f32 / 2.0).clamp(9.0, 18.0);
                    let (font, _) = fonts::font_for(cf.font.clone().or_else(|| self.doc.default_font_name()).as_deref(), cf.bold, cf.italic, size);
                    let mut text = RichText::new(self.entry_name(e)).font(font);
                    if cf.italic {
                        text = text.italics();
                    }
                    if cf.underline {
                        text = text.underline();
                    }
                    if let Some(c) = cf.color {
                        if c != 0 {
                            text = text.color(theme::rgb(c));
                        }
                    }
                    let r = ui.add(egui::Button::selectable(current, text).min_size(vec2(ui.available_width(), 0.0)));
                    if r.clicked() {
                        action = Some((0, e.clone()));
                    }
                    r.context_menu(|ui| {
                        if ui.button(format!("{}…", tr("style.modify"))).clicked() {
                            action = Some((1, e.clone()));
                            ui.close();
                        }
                        if !matches!(e, Entry::Kind(_)) && ui.button(tr("style.delete")).clicked() {
                            action = Some((2, e.clone()));
                            ui.close();
                        }
                    });
                };
                ui.label(RichText::new(tr("style.paragraph_styles")).small().weak());
                for e in &entries {
                    let current = match e {
                        Entry::Kind(k) => st.custom == 0 && st.kind == *k,
                        Entry::Custom(n) => st.custom == *n,
                        Entry::Char(_) => false,
                    };
                    row(ui, e, current);
                }
                if !chars.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(tr("style.char_styles")).small().weak());
                    for e in &chars {
                        let current = matches!(e, Entry::Char(id) if caret_cs.as_ref() == Some(id));
                        row(ui, e, current);
                    }
                }
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button(format!("{}…", tr("style.new"))).on_hover_text(tr("style.new_tip")).clicked() {
                    action = Some((3, Entry::Kind(ParaKind::Normal)));
                }
                if ui.button(format!("{}…", tr("style.new_char"))).clicked() {
                    action = Some((4, Entry::Kind(ParaKind::Normal)));
                }
                if caret_cs.is_some() && ui.button(tr("style.no_char_style")).clicked() {
                    action = Some((5, Entry::Kind(ParaKind::Normal)));
                }
            });
        });
        if !open {
            self.styles.pane = false;
        }
        match action {
            Some((0, e)) => {
                self.apply_entry(&e);
                self.refocus = true;
            }
            Some((1, e)) => self.open_modify(&e),
            Some((2, e)) => {
                let id = match &e {
                    Entry::Custom(n) => self.doc.styles.custom(*n).map(|d| d.id.clone()),
                    Entry::Char(id) => Some(id.clone()),
                    Entry::Kind(_) => None,
                };
                if let Some(id) = id {
                    self.doc.delete_style(&id, self.sel);
                }
            }
            Some((3, _)) => self.open_new_style(false),
            Some((4, _)) => self.open_new_style(true),
            Some((5, _)) => self.apply_char_style(None),
            _ => {}
        }
    }

    /// Modify Style / New Style dialog.
    pub(super) fn style_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.styles.dlg.take() else { return };
        let title = match (d.new, d.def.char_style) {
            (true, false) => tr("style.new_title"),
            (true, true) => tr("style.new_char_title"),
            _ => tr("style.modify_title"),
        };
        let default_font = self.doc.default_font_name();
        let para_pages = [(Icon::Styles, "style.page_general"), (Icon::Borders, "layout.borders_shading"), (Icon::Ruler, "tabs.title")];
        let pages = if d.def.char_style { &para_pages[..1] } else { &para_pages[..] };
        let mut page = d.page as usize;
        let uid = self.uid;
        let res = widgets::nav_dialog(ctx, "style_dlg", title, pages, &mut page, vec2(600.0, 470.0), |ui, page| match page {
            1 => borders_page(ui, &mut d.borders),
            2 => tabs_page(ui, &mut d.tabs),
            _ => style_page(ui, &mut d, default_font.as_deref(), uid),
        });
        d.page = page as u8;
        match res {
            DialogResult::None => self.styles.dlg = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                let name = d.name.trim().to_string();
                let clash = self.doc.styles.defs.iter().any(|s| *s.name == *name && (d.new || s.id != d.def.id));
                let builtin_clash = GALLERY.iter().any(|k| kind_label(*k).eq_ignore_ascii_case(&name) && !(d.def.builtin && d.def.kind == *k));
                if name.is_empty() || clash || builtin_clash {
                    d.error = true;
                    self.styles.dlg = Some(d);
                    return;
                }
                let mut def = d.def.clone();
                if !def.builtin {
                    def.name = name.as_str().into();
                }
                if d.new {
                    def.id = self.doc.styles.new_id(&name);
                }
                let (o, c) = (&d.orig, &d.cur);
                def.font = if c.font != o.font { (!c.font.is_empty()).then(|| Arc::from(c.font.as_str())) } else { def.font };
                def.size = pick(c.size != o.size, (c.size * 2.0).round() as u16, def.size);
                def.bold = pick(c.bold != o.bold, c.bold, def.bold);
                def.italic = pick(c.italic != o.italic, c.italic, def.italic);
                def.underline = pick(c.underline != o.underline, c.underline, def.underline);
                def.color = if c.color != o.color { c.color } else { def.color };
                if !def.char_style {
                    def.align = pick(c.align != o.align, c.align, def.align);
                    def.space_before = pick(c.before != o.before, c.before, def.space_before);
                    def.space_after = pick(c.after != o.after, c.after, def.space_after);
                    def.line_spacing = pick(c.line != o.line, c.line, def.line_spacing);
                    def.indent_left = pick(c.left != o.left, c.left, def.indent_left);
                    def.first_line = pick(c.first != o.first, c.first, def.first_line);
                    let (borders, shading) = (d.borders.borders(), d.borders.shading());
                    if borders != d.orig_borders.0 {
                        def.borders = (!borders.is_empty() || def.builtin).then_some(borders);
                    }
                    if shading != d.orig_borders.1 {
                        def.shading = shading;
                    }
                    let tabs = d.tabs.tabs();
                    if tabs != d.orig_tabs {
                        def.tabs = (!tabs.is_empty() || def.builtin).then_some(tabs);
                    }
                } else {
                    def.next = None;
                    def.linked = false;
                }
                let id = def.id.clone();
                let char_style = def.char_style;
                self.doc.set_style(def, self.sel);
                if d.new && d.apply {
                    if let Some(n) = self.doc.styles.custom_number(&id) {
                        let (a, b) = self.para_range();
                        self.doc.apply_para_style(a, b, self.sel, ParaKind::Normal, n);
                    }
                } else if d.new && char_style && !self.sel.is_empty() {
                    self.doc.apply_char_style(self.sel, Some(id));
                }
                self.refocus = true;
            }
        }
    }
}

/// The new value if the user changed it, else the style's old one.
fn pick<T>(changed: bool, v: T, old: Option<T>) -> Option<T> {
    if changed { Some(v) } else { old }
}

fn style_page(ui: &mut egui::Ui, d: &mut StyleDlg, default_font: Option<&str>, uid: u64) {
    egui::Grid::new("style_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label(tr("style.name"));
        ui.add_enabled(!d.def.builtin, egui::TextEdit::singleline(&mut d.name).desired_width(240.0));
        ui.end_row();
        if !d.def.char_style && !d.def.builtin {
            ui.label(tr("style.based_on"));
            egui::ComboBox::from_id_salt("style_based").width(240.0).selected_text(kind_label(d.def.kind)).show_ui(ui, |ui| {
                for k in GALLERY {
                    ui.selectable_value(&mut d.def.kind, k, kind_label(k));
                }
            });
            ui.end_row();
        }
        ui.label(tr("style.font"));
        ui.horizontal(|ui| {
            let default = tr("font.default");
            let cur = if d.cur.font.is_empty() { default_font.unwrap_or(default).to_string() } else { d.cur.font.clone() };
            if let Some(f) = common::font_picker(ui, Id::new(("style_font", uid)), &cur, 150.0) {
                d.cur.font = if f.eq_ignore_ascii_case(default) { String::new() } else { f };
            }
            if let Some(v) = common::size_box(ui, Id::new(("style_size", uid)), d.cur.size) {
                d.cur.size = v;
            }
        });
        ui.end_row();
        ui.label("");
        ui.horizontal(|ui| {
            ui.toggle_value(&mut d.cur.bold, RichText::new("B").strong()).on_hover_text(tr("format.bold"));
            ui.toggle_value(&mut d.cur.italic, RichText::new("I").italics()).on_hover_text(tr("format.italic"));
            ui.toggle_value(&mut d.cur.underline, RichText::new("U").underline()).on_hover_text(tr("format.underline"));
            let chip = RichText::new("A").strong().color(d.cur.color.map_or(ui.visuals().text_color(), theme::rgb));
            crate::widgets::menu_button(ui, chip, |ui| {
                if let Some(c) = widgets::color_menu(ui, tr("format.auto_color"), d.cur.color) {
                    d.cur.color = c;
                    ui.close();
                }
            })
            .response
            .on_hover_text(tr("format.text_color"));
        });
        ui.end_row();
        if !d.def.char_style {
            ui.label(tr("para.alignment"));
            ui.horizontal(|ui| {
                for (al, key) in [(Align::Left, "format.align_left"), (Align::Center, "format.align_center"), (Align::Right, "format.align_right"), (Align::Justify, "format.align_justify")] {
                    ui.selectable_value(&mut d.cur.align, al, tr(key));
                }
            });
            ui.end_row();
            widgets::number_row(ui, tr("para.before"), &mut d.cur.before, 0.0..=200.0, " pt");
            widgets::number_row(ui, tr("para.after"), &mut d.cur.after, 0.0..=200.0, " pt");
            ui.label(tr("style.line_spacing"));
            ui.add(egui::DragValue::new(&mut d.cur.line).range(0.5..=5.0).speed(0.05).max_decimals(2));
            ui.end_row();
            widgets::number_row(ui, tr("para.left"), &mut d.cur.left, 0.0..=400.0, " pt");
            widgets::number_row(ui, tr("style.first_line"), &mut d.cur.first, -200.0..=200.0, " pt");
            ui.label(tr("style.next"));
            let cur = d.nexts.iter().find(|(id, _)| *id == d.def.next).map_or_else(|| d.def.next.as_deref().unwrap_or_default().to_string(), |(_, n)| n.clone());
            egui::ComboBox::from_id_salt("style_next").width(240.0).selected_text(cur).show_ui(ui, |ui| {
                for (id, n) in &d.nexts {
                    ui.selectable_value(&mut d.def.next, id.clone(), n);
                }
            });
            ui.end_row();
            if !d.def.builtin {
                ui.label("");
                ui.checkbox(&mut d.def.linked, tr("style.linked")).on_hover_text(tr("style.linked_tip"));
                ui.end_row();
            }
        }
    });
    // Preview.
    ui.add_space(6.0);
    let c = &d.cur;
    let size = c.size.clamp(8.0, 28.0);
    let kind = d.def.kind;
    let heading = !d.def.char_style && matches!(kind, ParaKind::Title | ParaKind::Heading(_));
    let font = match Some(c.font.as_str()).filter(|f| !f.is_empty()).or(default_font) {
        Some(f) => fonts::font_for(Some(f), c.bold, c.italic, size).0,
        None => fonts::font(if heading { Face::Sans } else { Face::Serif }, c.bold, size),
    };
    let auto = if heading { Color32::from_rgb(0x1F, 0x2A, 0x44) } else if kind == ParaKind::Quote && !d.def.char_style { Color32::from_gray(85) } else { Paper::TEXT };
    let mut text = RichText::new(tr("style.preview")).font(font).color(c.color.map_or(auto, theme::rgb));
    if c.italic {
        text = text.italics();
    }
    if c.underline {
        text = text.underline();
    }
    let fill = d.borders.shading().map_or(Color32::WHITE, theme::rgb);
    let boxed = !d.borders.borders().is_empty();
    let stroke = if boxed { egui::Stroke::new(1.0, Color32::from_gray(60)) } else { egui::Stroke::NONE };
    egui::Frame::new().fill(fill).stroke(stroke).inner_margin(10.0).corner_radius(3.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let layout = match c.align {
            Align::Center => egui::Layout::top_down(egui::Align::Center),
            Align::Right => egui::Layout::top_down(egui::Align::Max),
            _ => egui::Layout::top_down(egui::Align::Min),
        };
        ui.with_layout(layout, |ui| ui.label(text));
    });
    if d.new && !d.def.char_style {
        ui.checkbox(&mut d.apply, tr("style.apply_now"));
    }
    if d.error {
        ui.colored_label(ui.visuals().error_fg_color, tr("style.bad_name"));
    }
}
