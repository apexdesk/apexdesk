//! Page layout commands: breaks, and the columns, orientation, size and
//! margins of sections; watermark, page color and page borders; paragraph
//! borders, tab stops (dialog and ruler) and drop caps.

use apex_doc::{BreakKind, Columns, DropCap, Line, LineStyle, MAX_COLUMNS, PAPER_SIZES, PageBorders, PageSetup, ParaBorders, Scope, SectionStart, TabAlign, TabLeader, TabStop, TabStops, Watermark};

use super::dialogs::ruler_unit;
use super::*;
use crate::widgets::DialogResult;

/// Which sections a layout change applies to.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ApplyTo {
    #[default]
    Section,
    /// From the caret on (a section break is inserted there).
    Forward,
    Whole,
}

pub enum LayoutDialog {
    Columns(ColumnsDlg),
    Watermark(Box<WatermarkDlg>),
    PageBorders(PageBordersDlg),
    Borders(BordersDlg),
    Tabs(TabsDlg),
    DropCap(DropCapDlg),
    /// Page color > Fill effects: the page's color or gradient.
    PageFill(Option<u32>, Option<apex_doc::Gradient>),
}

pub struct ColumnsDlg {
    n: usize,
    spacing: f32,
    separator: bool,
    equal: bool,
    /// Width and space after of each column, in points.
    widths: Vec<(f32, f32)>,
    /// Width of the text area, in points.
    total: f32,
    apply: ApplyTo,
}

pub struct WatermarkDlg {
    /// 0 none, 1 picture, 2 text.
    kind: u8,
    text: String,
    font: String,
    /// 0 = automatic.
    size: f32,
    color: u32,
    semi: bool,
    diagonal: bool,
    picture: Option<std::sync::Arc<[u8]>>,
    /// 0 = automatic.
    scale: f32,
    washout: bool,
}

pub struct PageBordersDlg {
    sides: [bool; 4],
    line: Line,
    from_edge: bool,
}

pub struct BordersDlg {
    /// Top, bottom, left, right, between.
    sides: [bool; 5],
    line: Line,
    shading: Option<u32>,
    orig_shading: Option<u32>,
}

impl BordersDlg {
    pub(super) fn new(b: &ParaBorders, shading: Option<u32>) -> Self {
        let line = b.top.or(b.left).or(b.bottom).or(b.right).or(b.between).unwrap_or_default();
        let sides = [b.top.is_some(), b.bottom.is_some(), b.left.is_some(), b.right.is_some(), b.between.is_some()];
        BordersDlg { sides, line, shading, orig_shading: shading }
    }

    pub(super) fn borders(&self) -> ParaBorders {
        let l = |on: bool, space: f32| on.then_some(Line { space, ..self.line });
        let side = self.line.space.max(4.0);
        ParaBorders { top: l(self.sides[0], self.line.space), bottom: l(self.sides[1], self.line.space), left: l(self.sides[2], side), right: l(self.sides[3], side), between: l(self.sides[4], self.line.space) }
    }

    pub(super) fn shading(&self) -> Option<u32> {
        self.shading
    }
}

impl TabsDlg {
    pub(super) fn new(tabs: &TabStops) -> Self {
        let stops = tabs.as_slice().to_vec();
        let pos = stops.first().map_or(0.0, |s| s.pos);
        TabsDlg { orig: stops.clone(), stops, pos, align: TabAlign::Left, leader: TabLeader::None }
    }

    pub(super) fn tabs(&self) -> TabStops {
        TabStops::from_slice(&self.stops)
    }

    /// Points the position field at `pos`, with the stop there chosen.
    fn at(mut self, pos: f32) -> Self {
        self.pos = pos;
        if let Some(s) = self.stops.iter().find(|s| (s.pos - pos).abs() < 0.5) {
            (self.align, self.leader) = (s.align, s.leader);
        }
        self
    }
}

pub struct TabsDlg {
    stops: Vec<TabStop>,
    orig: Vec<TabStop>,
    pos: f32,
    align: TabAlign,
    leader: TabLeader,
}

pub struct DropCapDlg {
    /// 0 none, 1 dropped, 2 in the margin.
    mode: u8,
    lines: u8,
    distance: f32,
    font: String,
    orig_font: String,
}

/// The ruler's tab type selector and a tab stop being dragged.
#[derive(Default)]
pub struct TabUi {
    kind: TabAlign,
    /// Original position, current position (points) and whether it's off
    /// the ruler (removed when released).
    drag: Option<(f32, f32, bool)>,
    /// The right-click menu's state while it's open.
    menu: Option<TabMenu>,
}

/// The ruler's right-click menu: where it was opened (points), the stop
/// under the pointer, and the position and type of a stop to add.
#[derive(Clone, Copy)]
struct TabMenu {
    at: f32,
    near: Option<f32>,
    pos: f32,
    kind: TabAlign,
}

/// Paper sizes offered in the Size menu.
const SIZES: [&str; 6] = ["A4", "Letter", "Legal", "A5", "A3", "Executive"];

/// Margin presets: name key and top, bottom, left, right in points.
/// The last one has mirrored margins (inside, outside).
const MARGINS: [(&str, [f32; 4]); 5] = [
    ("layout.margins_normal", [72.0, 72.0, 72.0, 72.0]),
    ("layout.margins_narrow", [36.0, 36.0, 36.0, 36.0]),
    ("layout.margins_moderate", [72.0, 72.0, 54.0, 54.0]),
    ("layout.margins_wide", [72.0, 72.0, 144.0, 144.0]),
    ("layout.margins_mirrored", [72.0, 72.0, 90.0, 72.0]),
];

fn start_key(s: SectionStart) -> &'static str {
    match s {
        SectionStart::NextPage => "layout.break_next",
        SectionStart::Continuous => "layout.break_continuous",
        SectionStart::EvenPage => "layout.break_even",
        SectionStart::OddPage => "layout.break_odd",
    }
}

fn style_key(s: LineStyle) -> &'static str {
    match s {
        LineStyle::Single => "layout.line_single",
        LineStyle::Double => "layout.line_double",
        LineStyle::Dotted => "layout.line_dotted",
        LineStyle::Dashed => "layout.line_dashed",
        LineStyle::DashDot => "layout.line_dash_dot",
        LineStyle::Thick => "layout.line_thick",
        LineStyle::Wave => "layout.line_wave",
    }
}

fn align_key(a: TabAlign) -> &'static str {
    match a {
        TabAlign::Left => "tabs.left",
        TabAlign::Center => "tabs.center",
        TabAlign::Right => "tabs.right",
        TabAlign::Decimal => "tabs.decimal",
    }
}

fn leader_key(l: TabLeader) -> &'static str {
    match l {
        TabLeader::None => "tabs.leader_none",
        TabLeader::Dots => "tabs.leader_dots",
        TabLeader::Dashes => "tabs.leader_dashes",
        TabLeader::Underline => "tabs.leader_underline",
    }
}

/// A length field in the ruler's unit (cm or inches); `v` in points.
fn len_field(ui: &mut egui::Ui, v: &mut f32, min: f32, max: f32) -> bool {
    let (unit, _, suffix) = ruler_unit();
    let mut u = *v / unit;
    let r = ui.add(egui::DragValue::new(&mut u).range(min / unit..=max / unit).speed(0.01).suffix(format!(" {suffix}")).max_decimals(2));
    if r.changed() {
        *v = (u * unit * 100.0).round() / 100.0;
    }
    r.changed()
}

/// A length shown in the ruler's unit.
fn len_text(v: f32) -> String {
    let (unit, _, suffix) = ruler_unit();
    let s = format!("{:.2} {suffix}", v / unit);
    if suffix == "cm" { s.replace('.', ",") } else { s }
}

pub(super) fn apply_to_ui(ui: &mut egui::Ui, a: &mut ApplyTo, sections: bool) {
    ui.horizontal(|ui| {
        ui.label(tr("layout.apply_to"));
        let name = |a: ApplyTo| {
            tr(match a {
                ApplyTo::Section => "layout.apply_section",
                ApplyTo::Forward => "layout.apply_forward",
                ApplyTo::Whole => "layout.apply_whole",
            })
        };
        egui::ComboBox::from_id_salt("layout_apply").width(170.0).selected_text(name(*a)).show_ui(ui, |ui| {
            let mut opts = vec![ApplyTo::Whole, ApplyTo::Forward];
            if sections {
                opts.insert(0, ApplyTo::Section);
            }
            for o in opts {
                ui.selectable_value(a, o, name(o));
            }
        });
    });
}

/// Line style, width and color fields of a border.
/// Style, width and color rows of a border, in a [`widgets::form`].
fn line_rows(ui: &mut egui::Ui, id: &str, l: &mut Line) {
    widgets::form_row(ui, tr("layout.line_style"), |ui| {
        egui::ComboBox::from_id_salt((id, "style")).width(150.0).selected_text(tr(style_key(l.style))).show_ui(ui, |ui| {
            for s in LineStyle::ALL {
                ui.selectable_value(&mut l.style, s, tr(style_key(s)));
            }
        });
    });
    widgets::form_row(ui, tr("table.line_width").trim_end_matches(':'), |ui| {
        egui::ComboBox::from_id_salt((id, "width")).width(150.0).selected_text(format!("{} pt", l.width)).show_ui(ui, |ui| {
            for w in [0.25f32, 0.5, 0.75, 1.0, 1.5, 2.25, 3.0, 4.5, 6.0] {
                ui.selectable_value(&mut l.width, w, format!("{w} pt"));
            }
        });
    });
    widgets::form_row(ui, tr("table.line_color").trim_end_matches(':'), |ui| {
        crate::widgets::menu_button(ui, RichText::new("■").color(theme::rgb(l.color)), |ui| {
            if let Some(c) = widgets::color_menu(ui, tr("color.automatic"), Some(l.color)) {
                l.color = c.unwrap_or(0);
                ui.close();
            }
        });
    });
}

impl DocView {
    /// Index of the section holding the caret.
    pub(super) fn cur_section(&self) -> usize {
        self.doc.section_index(self.sel.cursor.para)
    }

    /// The sections a change applies to; "from here on" first inserts a
    /// section break at the caret (unless a section starts there).
    fn apply_scope(&mut self, to: ApplyTo, start: SectionStart) -> Scope {
        let i = self.cur_section();
        match to {
            ApplyTo::Whole => Scope::Whole,
            ApplyTo::Section => Scope::Section(i),
            ApplyTo::Forward => {
                let c = self.sel.start();
                let first = self.doc.sections().get(i).map_or(0, |s| s.first);
                if c.offset == 0 && c.para == first {
                    return Scope::From(i);
                }
                match self.doc.insert_break(Selection::caret(c), BreakKind::Section(start)) {
                    Some(p) => {
                        self.sel = Selection::caret(p);
                        Scope::From(i + 1)
                    }
                    None => Scope::Section(i),
                }
            }
        }
    }

    /// Applies the page setup dialog: headers and footers are the
    /// document's; the page to the sections chosen.
    pub(super) fn apply_page_setup(&mut self, page: PageSetup, orig: PageSetup, (first, odd_even): (bool, bool), to: ApplyTo) {
        let s = self.cur_section();
        self.doc.set_hf_options(s, first, odd_even, self.sel);
        if page != orig {
            let scope = self.apply_scope(to, SectionStart::NextPage);
            // Mirrored margins and the gutter's side are the whole
            // document's in Word: one undo step with the rest.
            if (page.mirror, page.gutter_top) != (orig.mirror, orig.gutter_top) {
                let chosen = move |i: usize| match scope {
                    Scope::Whole => true,
                    Scope::Section(k) => i == k,
                    Scope::From(k) => i >= k,
                };
                self.doc.change_sections_each(Scope::Whole, self.sel, &move |i, p, _| {
                    if chosen(i) {
                        *p = page;
                    }
                    (p.mirror, p.gutter_top) = (page.mirror, page.gutter_top);
                });
            } else {
                self.doc.change_sections(scope, self.sel, &move |p, _| *p = page);
            }
        }
        self.scroll_to_caret = true;
    }

    /// Changes the page setup of the caret's section.
    pub(super) fn change_page(&mut self, f: impl Fn(&mut PageSetup) + 'static) {
        let i = self.cur_section();
        self.doc.change_sections(Scope::Section(i), self.sel, &|p, _| f(p));
        self.scroll_to_caret = true;
    }

    pub(super) fn set_paper(&mut self, w: f32, h: f32) {
        self.change_page(move |p| {
            let land = p.landscape();
            (p.width, p.height) = (w, h);
            p.set_landscape(land);
        });
    }

    pub(super) fn insert_break(&mut self, kind: BreakKind) {
        self.clear_block();
        match self.doc.insert_break(self.sel, kind) {
            Some(p) => {
                self.sel = Selection::caret(p);
                self.desired_x = None;
                self.scroll_to_caret = true;
            }
            None => self.notices.push(Notice::info(tr("layout.no_break_in_table"))),
        }
    }

    pub(super) fn breaks_items(&mut self, ui: &mut egui::Ui) {
        if ui.add(egui::Button::new(tr("doc.page_break")).shortcut_text("Ctrl+Enter")).clicked() {
            self.insert_break(BreakKind::Page);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("layout.column_break")).shortcut_text("Ctrl+Shift+Enter")).clicked() {
            self.insert_break(BreakKind::Column);
            ui.close();
        }
        ui.separator();
        ui.label(RichText::new(tr("layout.section_breaks")).small().weak());
        for s in [SectionStart::NextPage, SectionStart::Continuous, SectionStart::EvenPage, SectionStart::OddPage] {
            if ui.button(tr(start_key(s))).clicked() {
                self.insert_break(BreakKind::Section(s));
                ui.close();
            }
        }
    }

    pub(super) fn columns_items(&mut self, ui: &mut egui::Ui) {
        let cur = self.doc.section_setup(self.sel.cursor.para).1.clone();
        let presets: [(&str, Columns); 5] = [
            ("layout.cols_one", Columns::default()),
            ("layout.cols_two", Columns::equal(2, apex_doc::COLUMN_GAP)),
            ("layout.cols_three", Columns::equal(3, apex_doc::COLUMN_GAP)),
            ("layout.cols_left", Columns { count: 2, custom: vec![(1.0, 0.25), (2.0, 0.0)], ..Default::default() }),
            ("layout.cols_right", Columns { count: 2, custom: vec![(2.0, 0.25), (1.0, 0.0)], ..Default::default() }),
        ];
        for (key, c) in presets {
            let on = cur.n() == c.n() && (c.custom.is_empty() == cur.custom.is_empty()) && (c.custom.is_empty() || cur.custom.first().is_some_and(|f| (f.0 < cur.custom[1].0) == (c.custom[0].0 < c.custom[1].0)));
            if ui.radio(on, tr(key)).clicked() {
                self.set_columns(c, ApplyTo::Section);
                ui.close();
            }
        }
        ui.separator();
        if ui.button(format!("{}…", tr("layout.more_columns"))).clicked() {
            self.open_columns_dialog();
            ui.close();
        }
    }

    pub(super) fn orientation_items(&mut self, ui: &mut egui::Ui) {
        let land = self.doc.section_setup(self.sel.cursor.para).0.landscape();
        for (on, key) in [(false, "page.portrait"), (true, "page.landscape")] {
            if ui.radio(land == on, tr(key)).clicked() {
                self.change_page(move |p| p.set_landscape(on));
                ui.close();
            }
        }
    }

    /// Layout > Text Direction: horizontal or vertical pages (tategaki)
    /// for the caret's section.
    pub(super) fn text_direction_items(&mut self, ui: &mut egui::Ui) {
        let v = self.doc.section_setup(self.sel.cursor.para).0.vertical();
        for (on, key) in [(false, "page.dir_horizontal"), (true, "page.dir_vertical")] {
            if ui.radio(v == on, tr(key)).clicked() {
                self.change_page(move |p| p.set_vertical(on));
                ui.close();
            }
        }
    }

    pub(super) fn size_items(&mut self, ui: &mut egui::Ui) {
        let cur = *self.doc.section_setup(self.sel.cursor.para).0;
        let (w, h) = if cur.landscape() { (cur.height, cur.width) } else { (cur.width, cur.height) };
        for (name, a, b) in PAPER_SIZES.iter().filter(|p| SIZES.contains(&p.0)) {
            if ui.radio((a - w).abs() < 2.0 && (b - h).abs() < 2.0, *name).clicked() {
                self.set_paper(*a, *b);
                ui.close();
            }
        }
        ui.separator();
        if ui.button(format!("{}…", tr("layout.more_sizes"))).clicked() {
            self.open_page_setup(0);
            ui.close();
        }
    }

    pub(super) fn margins_items(&mut self, ui: &mut egui::Ui) {
        let cur = *self.doc.section_setup(self.sel.cursor.para).0;
        for (i, (key, [t, b, l, r])) in MARGINS.into_iter().enumerate() {
            let mirror = i == MARGINS.len() - 1;
            let on = cur.mirror == mirror && [cur.margin_top, cur.margin_bottom, cur.margin_left, cur.margin_right].iter().zip([t, b, l, r]).all(|(a, b)| (a - b).abs() < 1.0);
            let label = format!("{}  ({} / {})", tr(key), len_text(t), len_text(l));
            if ui.radio(on, label).clicked() {
                self.set_margins([t, b, l, r], mirror);
                ui.close();
            }
        }
        ui.separator();
        if ui.button(format!("{}…", tr("layout.custom_margins"))).clicked() {
            self.open_page_setup(1);
            ui.close();
        }
    }

    /// A margins preset: top, bottom, left, right of the caret's section,
    /// and whether margins mirror (the whole document's), as one undo step.
    pub(super) fn set_margins(&mut self, [t, b, l, r]: [f32; 4], mirror: bool) {
        let here = self.cur_section();
        let scope = if self.doc.section_setup(self.sel.cursor.para).0.mirror != mirror { Scope::Whole } else { Scope::Section(here) };
        self.doc.change_sections_each(scope, self.sel, &move |i, p, _| {
            p.mirror = mirror;
            if i == here {
                (p.margin_top, p.margin_bottom, p.margin_left, p.margin_right) = (t, b, l, r);
            }
        });
        self.scroll_to_caret = true;
    }

    pub(super) fn page_color_items(&mut self, ui: &mut egui::Ui) {
        let (cur, cur_grad) = (self.doc.deco.color, self.doc.deco.gradient.clone());
        if let Some(c) = widgets::color_menu(ui, tr("layout.no_color"), cur.filter(|_| cur_grad.is_none())) {
            let deco = apex_doc::PageDeco { color: c, gradient: None, ..self.doc.deco.clone() };
            self.doc.set_page_deco(deco, self.sel);
        }
        ui.separator();
        // Fill effects: a gradient over the page.
        crate::widgets::menu_button(ui, tr("grad.gradient"), |ui| {
            if let Some(g) = crate::gradient_ui::gallery(ui, cur.unwrap_or(0xDEEAF6), cur_grad.as_ref()) {
                let deco = match g {
                    Some(g) => apex_doc::PageDeco { color: Some(g.main_color()), gradient: Some(g), ..self.doc.deco.clone() },
                    None => apex_doc::PageDeco { gradient: None, ..self.doc.deco.clone() },
                };
                self.doc.set_page_deco(deco, self.sel);
                ui.close();
            }
            ui.separator();
            if crate::toolstrip::menu_item(ui, crate::icons::Icon::Fill, &format!("{}…", tr("grad.more"))).clicked() {
                let g = cur_grad.clone().unwrap_or_else(|| apex_doc::gradient::variation(cur.unwrap_or(0xDEEAF6), false, 1));
                self.layout_dialog = Some(LayoutDialog::PageFill(Some(g.main_color()), Some(g)));
                ui.close();
            }
        });
    }

    fn set_columns(&mut self, c: Columns, to: ApplyTo) {
        if self.doc.in_table(self.sel.cursor.para) && to == ApplyTo::Forward {
            return;
        }
        let scope = self.apply_scope(to, SectionStart::Continuous);
        self.doc.change_sections(scope, self.sel, &move |_, cols| *cols = c.clone());
        self.scroll_to_caret = true;
    }

    // --- dialogs -------------------------------------------------------------------------

    fn open_columns_dialog(&mut self) {
        let (page, c) = self.doc.section_setup(self.sel.cursor.para);
        let total = page.content_width().max(10.0);
        let n = c.n();
        let widths = c.layout(total);
        let widths = widths.iter().enumerate().map(|(i, &(x, w))| (w, widths.get(i + 1).map_or(0.0, |nx| nx.0 - x - w))).collect();
        let spacing = if c.custom.len() > 1 { c.custom[0].1 } else { c.spacing };
        let apply = if self.doc.has_sections() { ApplyTo::Section } else { ApplyTo::Whole };
        self.layout_dialog = Some(LayoutDialog::Columns(ColumnsDlg { n, spacing, separator: c.separator, equal: c.custom.len() < 2, widths, total, apply }));
    }

    pub(super) fn open_watermark_dialog(&mut self) {
        let w = self.doc.deco.watermark.clone();
        let d = match w {
            Some(w) => WatermarkDlg {
                kind: if w.picture.is_some() { 1 } else if w.text.is_empty() { 0 } else { 2 },
                text: if w.text.is_empty() { tr("layout.wm_draft").to_string() } else { w.text },
                font: w.font.as_deref().unwrap_or_default().to_string(),
                size: w.size.unwrap_or(0.0),
                color: w.color,
                semi: w.semitransparent,
                diagonal: w.diagonal,
                picture: w.picture,
                scale: w.scale,
                washout: w.washout,
            },
            None => WatermarkDlg { kind: 2, text: tr("layout.wm_draft").to_string(), font: String::new(), size: 0.0, color: 0xC0C0C0, semi: true, diagonal: true, picture: None, scale: 0.0, washout: true },
        };
        self.layout_dialog = Some(LayoutDialog::Watermark(Box::new(d)));
    }

    pub(super) fn open_page_borders_dialog(&mut self) {
        let b = self.doc.deco.borders;
        let line = b.top.or(b.left).or(b.bottom).or(b.right).unwrap_or(Line { space: if b.from_edge { 24.0 } else { 4.0 }, ..Line::default() });
        self.layout_dialog = Some(LayoutDialog::PageBorders(PageBordersDlg { sides: [b.top.is_some(), b.bottom.is_some(), b.left.is_some(), b.right.is_some()], line, from_edge: b.from_edge }));
    }

    pub(super) fn open_para_borders_dialog(&mut self) {
        let st = self.cur_para().style;
        self.layout_dialog = Some(LayoutDialog::Borders(BordersDlg::new(&st.borders, st.shading)));
    }

    pub(super) fn open_tabs_dialog(&mut self) {
        self.layout_dialog = Some(LayoutDialog::Tabs(TabsDlg::new(&self.cur_para().style.tabs)));
    }

    /// The tabs dialog with its position field at `pos` (from the ruler).
    fn open_tabs_dialog_at(&mut self, pos: f32) {
        self.layout_dialog = Some(LayoutDialog::Tabs(TabsDlg::new(&self.cur_para().style.tabs).at(pos)));
    }

    /// The ruler's right-click menu; `max` is the text width in points.
    fn ruler_tab_menu(&mut self, ui: &mut egui::Ui, tabs: &TabStops, max: f32) {
        let Some(mut m) = self.tab_ui.menu else { return };
        if let Some(p) = m.near {
            if ui.button(trf("tabs.menu_remove", &[("v", &len_text(p))])).clicked() {
                self.para_style(move |s| s.tabs.remove(p));
                ui.close();
            }
        } else if ui.button(trf("tabs.menu_add_here", &[("v", &len_text(m.at))])).clicked() {
            let stop = TabStop { pos: m.at, align: m.kind, leader: TabLeader::None };
            self.para_style(move |s| s.tabs.set(stop));
            ui.close();
        }
        ui.separator();
        ui.label(tr("tabs.menu_add_at"));
        ui.horizontal(|ui| {
            len_field(ui, &mut m.pos, 0.0, max);
            if ui.button(tr("tabs.add")).clicked() {
                let stop = TabStop { pos: m.pos, align: m.kind, leader: TabLeader::None };
                self.para_style(move |s| s.tabs.set(stop));
                ui.close();
            }
        });
        for a in TabAlign::ALL {
            ui.radio_value(&mut m.kind, a, tr(align_key(a)));
        }
        if !tabs.as_slice().is_empty() {
            ui.separator();
            if ui.button(tr("tabs.clear_all")).clicked() {
                self.para_style(|s| s.tabs = TabStops::default());
                ui.close();
            }
        }
        ui.separator();
        if ui.button(tr("tabs.menu_settings")).clicked() {
            self.open_tabs_dialog_at(m.near.unwrap_or(m.at));
            ui.close();
        }
        self.tab_ui.menu = Some(m);
    }

    pub(super) fn open_drop_cap_dialog(&mut self) {
        let p = self.cur_para();
        let d = p.style.drop_cap.unwrap_or_default();
        let font = p.runs.first().and_then(|r| r.attrs.font.as_deref()).unwrap_or_default().to_string();
        let mode = match p.style.drop_cap {
            None => 0,
            Some(d) if d.margin => 2,
            Some(_) => 1,
        };
        self.layout_dialog = Some(LayoutDialog::DropCap(DropCapDlg { mode, lines: d.lines, distance: d.distance, orig_font: font.clone(), font }));
    }

    pub(super) fn layout_dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.layout_dialog.take() else { return };
        let sections = self.doc.has_sections();
        let res = match &mut d {
            LayoutDialog::Columns(c) => widgets::dialog(ctx, "doc_columns", tr("layout.columns"), 380.0, |ui| columns_page(ui, c, sections)),
            LayoutDialog::Watermark(w) => widgets::dialog(ctx, "doc_watermark", tr("layout.watermark"), 400.0, |ui| watermark_page(ui, w)),
            LayoutDialog::PageBorders(b) => widgets::dialog(ctx, "doc_page_borders", tr("layout.page_borders"), 360.0, |ui| page_borders_page(ui, b)),
            LayoutDialog::Borders(b) => widgets::dialog(ctx, "doc_para_borders", tr("layout.borders_shading"), 360.0, |ui| borders_page(ui, b)),
            LayoutDialog::Tabs(t) => widgets::dialog(ctx, "doc_tabs", tr("tabs.title"), 380.0, |ui| tabs_page(ui, t)),
            LayoutDialog::DropCap(c) => widgets::dialog(ctx, "doc_drop_cap", tr("dropcap.title"), 320.0, |ui| drop_cap_page(ui, c)),
            LayoutDialog::PageFill(f, g) => widgets::dialog(ctx, "doc_page_fill", tr("layout.fill_effects"), 420.0, |ui| {
                crate::gradient_ui::fill_editor(ui, egui::Id::new("doc-page-grad"), f, g, "layout.no_color");
            }),
        };
        match res {
            DialogResult::None => self.layout_dialog = Some(d),
            DialogResult::Cancel => self.focus = true,
            DialogResult::Ok => {
                self.apply_layout_dialog(d);
                self.focus = true;
            }
        }
    }

    fn apply_layout_dialog(&mut self, d: LayoutDialog) {
        match d {
            LayoutDialog::PageFill(color, gradient) => {
                let deco = apex_doc::PageDeco { color, gradient, ..self.doc.deco.clone() };
                self.doc.set_page_deco(deco, self.sel);
            }
            LayoutDialog::Columns(c) => {
                let custom = if c.equal || c.n < 2 { Vec::new() } else { c.widths.iter().take(c.n).copied().collect() };
                let cols = Columns { count: c.n as u8, spacing: c.spacing, separator: c.separator && c.n > 1, custom };
                self.set_columns(cols, c.apply);
            }
            LayoutDialog::Watermark(w) => {
                let wm = match w.kind {
                    1 if w.picture.is_some() => Some(Watermark { text: String::new(), picture: w.picture, scale: w.scale, washout: w.washout, ..Default::default() }),
                    2 if !w.text.trim().is_empty() => Some(Watermark {
                        text: w.text.trim().to_string(),
                        font: Some(w.font.trim()).filter(|f| !f.is_empty()).map(std::sync::Arc::from),
                        size: (w.size > 0.0).then_some(w.size),
                        color: w.color,
                        semitransparent: w.semi,
                        diagonal: w.diagonal,
                        ..Default::default()
                    }),
                    _ => None,
                };
                let deco = apex_doc::PageDeco { watermark: wm, ..self.doc.deco.clone() };
                self.doc.set_page_deco(deco, self.sel);
            }
            LayoutDialog::PageBorders(b) => {
                let l = |on: bool| on.then_some(b.line);
                let borders = PageBorders { top: l(b.sides[0]), bottom: l(b.sides[1]), left: l(b.sides[2]), right: l(b.sides[3]), from_edge: b.from_edge };
                let deco = apex_doc::PageDeco { borders, ..self.doc.deco.clone() };
                self.doc.set_page_deco(deco, self.sel);
            }
            LayoutDialog::Borders(b) => {
                let borders = b.borders();
                let (shading, orig) = (b.shading, b.orig_shading);
                self.para_style(move |s| {
                    s.borders = borders;
                    if shading != orig {
                        s.shading = shading;
                    }
                });
            }
            LayoutDialog::Tabs(t) => {
                if t.stops != t.orig {
                    let tabs = TabStops::from_slice(&t.stops);
                    self.para_style(move |s| s.tabs = tabs);
                }
            }
            LayoutDialog::DropCap(c) => {
                let cap = match c.mode {
                    0 => None,
                    m => Some(DropCap { lines: c.lines, margin: m == 2, distance: c.distance, chars: 1 }.sane()),
                };
                let (a, b) = self.para_range();
                let _ = b;
                self.doc.set_para_style(a, a, self.sel, &move |s: &mut ParaStyle| s.drop_cap = cap);
                let font = c.font.trim().to_string();
                if cap.is_some() && font != c.orig_font {
                    // The cap letter takes the font chosen.
                    let text = &self.doc.paragraphs[a].text;
                    let end = text.char_indices().nth(1).map_or(text.len(), |(i, _)| i);
                    let f: Option<std::sync::Arc<str>> = (!font.is_empty()).then(|| std::sync::Arc::from(font.as_str()));
                    let sel = Selection { anchor: Pos::new(a, 0), cursor: Pos::new(a, end) };
                    self.doc.format_range(sel, &move |at: &mut Attrs| at.font = f.clone());
                }
            }
        }
        self.scroll_to_caret = true;
    }

    // --- ruler tab stops -------------------------------------------------------------------

    /// The caret's segment.
    pub(super) fn caret_seg(&self) -> Option<Segment> {
        let p = self.sel.cursor;
        let pl = self.layout.paras.get(p.para)?;
        let text = &self.doc.paragraphs[p.para].text;
        let ci = text[..p.offset.min(text.len())].chars().count();
        let r = pl.galley.pos_from_cursor(CCursor::new(ci));
        let segs = self.layout.segs_of(p.para);
        segs.iter().rev().find(|s| r.min.y >= s.gy0 - 0.5).or(segs.first()).copied()
    }

    /// For the ruler: the caret section's page setup, the canvas x of its
    /// page's left edge as the ruler draws it, and the caret's column (left
    /// edge from that page edge, width) with several columns. The page edge
    /// is found from where the text really starts, so that the ruler's zero
    /// stands on the text in every view (a draft page is only the text with
    /// a little padding, not the paper with its margins).
    pub(super) fn ruler_frame(&self) -> (PageSetup, f32, Option<(f32, f32)>) {
        let pt = PT * self.zoom;
        let (page, cols) = self.doc.section_setup(self.sel.cursor.para);
        let seg = self.caret_seg();
        let pg = seg.map_or(0, |s| s.page);
        // The page's own left and right text edges (gutter, mirrored margins).
        let mut shown = *page;
        shown.margin_left = page.text_left(pg);
        shown.margin_right = page.width - shown.margin_left - page.content_width();
        shown.gutter = 0.0;
        let page_l = self.content_origin(pg).x - shown.margin_left * pt;
        let col = seg.filter(|s| !cols.is_single() && !s.cell).map(|s| (shown.margin_left * pt + s.x, s.w));
        (shown, page_l, col)
    }

    /// Tab stops on the ruler: the type selector at its left, the stops of
    /// the caret's paragraph; a click adds one, a drag moves it and a drag
    /// off the ruler removes it. `free` is false over the indent markers.
    /// A right click opens a menu to add a stop there or at a typed
    /// position, remove the one under the pointer or all of them, or open
    /// the tabs dialog.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ruler_tabs(&mut self, ui: &egui::Ui, resp: &egui::Response, p: &egui::Painter, rect: Rect, cl: f32, cr: f32, pt: f32, free: bool) {
        let v = ui.visuals();
        let ink = if v.dark_mode { Color32::from_gray(210) } else { Color32::from_gray(40) };
        let mid = rect.center().y;
        // The selector.
        let sel_rect = Rect::from_min_size(pos2(rect.min.x + 3.0, rect.min.y + 2.0), vec2(16.0, rect.height() - 4.0));
        p.rect_stroke(sel_rect, 2.0, Stroke::new(1.0, v.weak_text_color()), egui::StrokeKind::Inside);
        tab_glyph(p, pos2(sel_rect.center().x, sel_rect.max.y - 4.0), self.tab_ui.kind, ink);
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        if let Some(h) = resp.hover_pos()
            && sel_rect.contains(h)
        {
            resp.clone().on_hover_text_at_pointer(tr(align_key(self.tab_ui.kind)));
        }
        let st = self.cur_para().style;
        let (unit, subdiv, _) = ruler_unit();
        let snap = |x: f32, alt: bool| {
            let raw = ((x - cl) / pt).max(0.0);
            if alt { raw } else { (raw / (unit / subdiv as f32)).round() * (unit / subdiv as f32) }
        };
        let alt = ui.input(|i| i.modifiers.alt);
        let max = ((cr - cl) / pt).max(0.0);
        if resp.secondary_clicked()
            && let Some(h) = resp.interact_pointer_pos()
        {
            let at = snap(h.x, alt).min(max);
            let near = st.tabs.as_slice().iter().find(|s| (cl + s.pos * pt - h.x).abs() <= 5.0).map(|s| s.pos);
            self.tab_ui.menu = Some(TabMenu { at, near, pos: near.unwrap_or(at), kind: self.tab_ui.kind });
        }
        resp.context_menu(|ui| self.ruler_tab_menu(ui, &st.tabs, max));
        if resp.clicked()
            && let Some(h) = resp.interact_pointer_pos()
            && sel_rect.contains(h)
        {
            let all = TabAlign::ALL;
            let i = all.iter().position(|a| *a == self.tab_ui.kind).unwrap_or(0);
            self.tab_ui.kind = all[(i + 1) % all.len()];
            return;
        }
        let drag = self.tab_ui.drag;
        for s in st.tabs.as_slice() {
            let pos = match drag {
                Some((o, c, off)) if (o - s.pos).abs() < 0.01 => {
                    if off {
                        continue;
                    }
                    c
                }
                _ => s.pos,
            };
            tab_glyph(p, pos2(cl + pos * pt, rect.max.y - 3.0), s.align, ink);
        }
        let lower = |h: Pos2| h.y >= mid - 2.0;
        // The stop under the press, not where the pointer is once it has
        // moved far enough to start a drag.
        if resp.drag_started()
            && free
            && let Some(h) = ui.input(|i| i.pointer.press_origin()).or(pointer)
        {
            let near = st.tabs.as_slice().iter().find(|s| (cl + s.pos * pt - h.x).abs() <= 5.0);
            if let Some(s) = near {
                self.tab_ui.drag = Some((s.pos, s.pos, false));
            }
        }
        if let Some((o, _, _)) = self.tab_ui.drag
            && let Some(h) = resp.interact_pointer_pos()
        {
            let off = h.y > rect.max.y + 12.0 || h.y < rect.min.y - 12.0;
            self.tab_ui.drag = Some((o, snap(h.x, alt), off));
            ui.ctx().set_cursor_icon(if off { egui::CursorIcon::NotAllowed } else { egui::CursorIcon::ResizeHorizontal });
            if !off {
                ui.ctx().debug_painter().vline(h.x, rect.max.y..=rect.max.y + self.viewport_h, Stroke::new(1.0, Color32::from_gray(128)));
            }
        }
        if resp.drag_stopped()
            && let Some((o, c, off)) = self.tab_ui.drag.take()
        {
            let Some(old) = st.tabs.as_slice().iter().find(|s| (s.pos - o).abs() < 0.01).copied() else { return };
            let within = c * pt <= cr - cl + 1.0;
            self.para_style(move |s| {
                s.tabs.remove(old.pos);
                if !off && within {
                    s.tabs.set(TabStop { pos: c, ..old });
                }
            });
            return;
        }
        if resp.clicked()
            && free
            && let Some(h) = resp.interact_pointer_pos()
            && lower(h)
            && h.x > cl
            && h.x < cr
        {
            let stop = TabStop { pos: snap(h.x, alt), align: self.tab_ui.kind, leader: TabLeader::None };
            self.para_style(move |s| s.tabs.set(stop));
        }
    }

    /// Commits a margin dragged on the ruler to the caret's section.
    pub(super) fn ruler_margin(&mut self, left: bool, dv: f32) {
        let min_content = 72.0;
        self.change_page(move |pg| {
            let (m, other) = if left { (&mut pg.margin_left, pg.margin_right) } else { (&mut pg.margin_right, pg.margin_left) };
            *m = (*m + dv).clamp(0.0, (pg.width - other - min_content).max(0.0));
        });
    }

    /// The mark of a break ending a paragraph, shown with the formatting
    /// marks: section and column breaks (and page breaks) after the ¶.
    pub(super) fn paint_break_mark(&self, p: &egui::Painter, seg: &Segment, pl: &ParaLayout, galley_pos: Pos2, o: Pos2, color: Color32) {
        if seg.gy1 < pl.galley.size().y - 0.5 {
            return;
        }
        let Some(next) = self.doc.paragraphs.get(seg.para + 1) else { return };
        let key = match next.section_start() {
            Some(s) => match s.start {
                SectionStart::NextPage => "marks.section_next",
                SectionStart::Continuous => "marks.section_continuous",
                SectionStart::EvenPage => "marks.section_even",
                SectionStart::OddPage => "marks.section_odd",
            },
            None if next.style.column_break_before && next.cell.is_none() => "marks.column_break",
            None if next.style.page_break_before && next.cell.is_none() => "marks.page_break",
            None => return,
        };
        let Some(row) = pl.galley.rows.last() else { return };
        let z = self.zoom;
        let y = galley_pos.y + row.pos.y + row.row.size.y / 2.0;
        let x0 = galley_pos.x + row.pos.x + row.row.size.x + 14.0 * z;
        let x1 = o.x + seg.w;
        let font = egui::FontId::proportional(8.0 * z.max(0.8));
        let g = p.layout_no_wrap(tr(key).to_string(), font, color);
        let cx = ((x0 + x1) / 2.0).max(x0 + g.size().x / 2.0 + 4.0);
        let stroke = Stroke::new(1.0, color);
        let double = next.section_start().is_some();
        for (a, b) in [(x0, cx - g.size().x / 2.0 - 3.0), (cx + g.size().x / 2.0 + 3.0, x1)] {
            if b > a {
                for dy in if double { [-1.5, 1.5] } else { [0.0, 0.0] } {
                    p.extend(egui::Shape::dotted_line(&[pos2(a, y + dy), pos2(b, y + dy)], color, 3.0, 0.5));
                }
            }
        }
        let _ = stroke;
        p.galley(pos2(cx - g.size().x / 2.0, y - g.size().y / 2.0), g, color);
    }
}

/// A tab stop's marker on the ruler, standing on `at`.
fn tab_glyph(p: &egui::Painter, at: Pos2, a: TabAlign, color: Color32) {
    let s = Stroke::new(1.6, color);
    let (x, y) = (at.x, at.y);
    let h = 6.0;
    match a {
        TabAlign::Left => {
            p.line_segment([pos2(x, y - h), pos2(x, y)], s);
            p.line_segment([pos2(x, y), pos2(x + 5.0, y)], s);
        }
        TabAlign::Right => {
            p.line_segment([pos2(x, y - h), pos2(x, y)], s);
            p.line_segment([pos2(x - 5.0, y), pos2(x, y)], s);
        }
        TabAlign::Center | TabAlign::Decimal => {
            p.line_segment([pos2(x, y - h), pos2(x, y)], s);
            p.line_segment([pos2(x - 4.0, y), pos2(x + 4.0, y)], s);
            if a == TabAlign::Decimal {
                p.circle_filled(pos2(x + 2.8, y - 3.0), 1.1, color);
            }
        }
    }
}

// --- dialog pages ------------------------------------------------------------------------

fn columns_page(ui: &mut egui::Ui, c: &mut ColumnsDlg, sections: bool) {
    ui.label(RichText::new(tr("layout.presets")).strong());
    ui.horizontal(|ui| {
        let presets: [(&str, usize, Option<[f32; 2]>); 5] =
            [("layout.cols_one", 1, None), ("layout.cols_two", 2, None), ("layout.cols_three", 3, None), ("layout.cols_left", 2, Some([1.0, 2.0])), ("layout.cols_right", 2, Some([2.0, 1.0]))];
        // The preset the columns are in shows chosen.
        let narrow_first = c.widths.len() == 2 && c.widths[0].0 < c.widths[1].0;
        for (key, n, ratio) in presets {
            let on = c.n == n && (n == 1 || match ratio {
                None => c.equal,
                Some([a, b]) => !c.equal && narrow_first == (a < b),
            });
            if ui.add(egui::Button::new(tr(key)).selected(on)).clicked() {
                c.n = n;
                c.equal = ratio.is_none();
                if let Some([a, b]) = ratio {
                    let w = c.total - c.spacing;
                    c.widths = vec![(w * a / (a + b), c.spacing), (w * b / (a + b), 0.0)];
                }
            }
        }
    });
    ui.add_space(6.0);
    let before = (c.n, c.equal);
    egui::Grid::new("columns_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label(tr("layout.num_columns"));
        let mut n = c.n as u32;
        ui.add(egui::DragValue::new(&mut n).range(1..=MAX_COLUMNS as u32));
        c.n = n as usize;
        ui.end_row();
        if c.equal || c.n < 2 {
            ui.label(tr("layout.spacing"));
            len_field(ui, &mut c.spacing, 0.0, c.total / c.n as f32 * 0.8);
            ui.end_row();
        }
    });
    ui.checkbox(&mut c.separator, tr("layout.line_between"));
    ui.checkbox(&mut c.equal, tr("layout.equal_width"));
    if (c.n, c.equal) != before || c.widths.len() != c.n {
        let gap = c.spacing;
        let w = (c.total - gap * (c.n.saturating_sub(1)) as f32) / c.n.max(1) as f32;
        c.widths = (0..c.n).map(|i| (w.max(10.0), if i + 1 < c.n { gap } else { 0.0 })).collect();
    }
    if !c.equal && c.n > 1 {
        ui.add_space(4.0);
        egui::Grid::new("columns_widths").num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new(tr("layout.col_number")).weak());
            ui.label(RichText::new(tr("layout.width")).weak());
            ui.label(RichText::new(tr("layout.spacing")).weak());
            ui.end_row();
            let n = c.widths.len();
            for (i, (w, g)) in c.widths.iter_mut().enumerate() {
                ui.label(format!("{}", i + 1));
                len_field(ui, w, 10.0, c.total);
                if i + 1 < n {
                    len_field(ui, g, 0.0, c.total / 2.0);
                }
                ui.end_row();
            }
        });
    }
    ui.add_space(6.0);
    preview_columns(ui, c);
    ui.add_space(6.0);
    apply_to_ui(ui, &mut c.apply, sections);
}

/// A small page with the columns.
fn preview_columns(ui: &mut egui::Ui, c: &ColumnsDlg) {
    let (rect, _) = ui.allocate_exact_size(vec2(70.0, 90.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, Color32::WHITE);
    p.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
    let inner = rect.shrink(8.0);
    let cols = if c.equal || c.n < 2 { Columns::equal(c.n as u8, c.spacing) } else { Columns { count: c.n as u8, custom: c.widths.clone(), ..Default::default() } };
    let k = inner.width() / c.total.max(1.0);
    let line = Color32::from_gray(150);
    let lay = cols.layout(c.total);
    for (x, w) in &lay {
        let mut y = inner.min.y;
        while y < inner.max.y {
            p.hline((inner.min.x + x * k)..=(inner.min.x + (x + w) * k), y, Stroke::new(1.0, line));
            y += 4.0;
        }
    }
    if c.separator {
        for w in lay.windows(2) {
            let x = inner.min.x + (w[0].0 + w[0].1 + w[1].0) / 2.0 * k;
            p.vline(x, inner.y_range(), Stroke::new(1.0, Color32::DARK_GRAY));
        }
    }
}

fn watermark_page(ui: &mut egui::Ui, w: &mut WatermarkDlg) {
    ui.radio_value(&mut w.kind, 0, tr("layout.wm_none"));
    ui.radio_value(&mut w.kind, 1, tr("layout.wm_picture"));
    ui.add_enabled_ui(w.kind == 1, |ui| {
        ui.indent("wm_pic", |ui| {
            ui.horizontal(|ui| {
                if ui.button(format!("{}…", tr("layout.wm_select_picture"))).clicked() {
                    crate::file_dialog::FileDialog::new().set_title(tr("layout.wm_select_picture")).add_filter(tr("pic.files"), &["png", "jpg", "jpeg", "gif", "bmp", "webp"]).pick_file(crate::file_dialog::in_view(|v: &mut DocView, p: std::path::PathBuf| {
                        if let Some(LayoutDialog::Watermark(w)) = &mut v.layout_dialog
                            && let Some(data) = std::fs::read(p).ok().filter(|d| apex_io::pixel_size(d).is_some())
                        {
                            w.picture = Some(std::sync::Arc::from(data));
                        }
                    }));
                }
                if w.picture.is_some() {
                    ui.label(RichText::new(tr("layout.wm_picture_set")).weak());
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr("layout.wm_scale"));
                let mut pct = (w.scale * 100.0).round() as u32;
                let text = if pct == 0 { tr("page.auto").to_string() } else { format!("{pct}%") };
                egui::ComboBox::from_id_salt("wm_scale").width(90.0).selected_text(text).show_ui(ui, |ui| {
                    for v in [0u32, 50, 100, 150, 200, 500] {
                        ui.selectable_value(&mut pct, v, if v == 0 { tr("page.auto").to_string() } else { format!("{v}%") });
                    }
                });
                w.scale = pct as f32 / 100.0;
                ui.checkbox(&mut w.washout, tr("layout.wm_washout"));
            });
        });
    });
    ui.radio_value(&mut w.kind, 2, tr("layout.wm_text"));
    ui.add_enabled_ui(w.kind == 2, |ui| {
        egui::Grid::new("wm_text").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(tr("layout.wm_text_label"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut w.text).desired_width(180.0));
                crate::widgets::menu_button(ui, "…", |ui| {
                    for key in ["layout.wm_asap", "layout.wm_confidential", "layout.wm_copy", "layout.wm_do_not_copy", "layout.wm_draft", "layout.wm_original", "layout.wm_personal", "layout.wm_sample", "layout.wm_top_secret", "layout.wm_urgent"] {
                        if ui.button(tr(key)).clicked() {
                            w.text = tr(key).to_string();
                            ui.close();
                        }
                    }
                });
            });
            ui.end_row();
            ui.label(tr("layout.font"));
            let shown = if w.font.is_empty() { tr("layout.font_default").to_string() } else { w.font.clone() };
            egui::ComboBox::from_id_salt("wm_font").width(180.0).selected_text(shown).height(300.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut w.font, String::new(), tr("layout.font_default"));
                for f in crate::fonts::families().iter() {
                    ui.selectable_value(&mut w.font, f.clone(), f.as_str());
                }
            });
            ui.end_row();
            ui.label(tr("layout.size"));
            let text = if w.size <= 0.0 { tr("page.auto").to_string() } else { format!("{}", w.size) };
            egui::ComboBox::from_id_salt("wm_size").width(90.0).selected_text(text).show_ui(ui, |ui| {
                for v in [0.0f32, 36.0, 54.0, 72.0, 90.0, 105.0, 120.0, 144.0] {
                    ui.selectable_value(&mut w.size, v, if v == 0.0 { tr("page.auto").to_string() } else { format!("{v}") });
                }
            });
            ui.end_row();
            ui.label(tr("table.line_color").trim_end_matches(':'));
            ui.horizontal(|ui| {
                crate::widgets::menu_button(ui, RichText::new("■").color(theme::rgb(w.color)), |ui| {
                    if let Some(c) = widgets::color_menu(ui, tr("color.automatic"), Some(w.color)) {
                        w.color = c.unwrap_or(0xC0C0C0);
                        ui.close();
                    }
                });
                ui.checkbox(&mut w.semi, tr("layout.wm_semitransparent"));
            });
            ui.end_row();
            ui.label(tr("layout.wm_layout"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut w.diagonal, true, tr("layout.wm_diagonal"));
                ui.radio_value(&mut w.diagonal, false, tr("layout.wm_horizontal"));
            });
            ui.end_row();
        });
    });
}

fn page_borders_page(ui: &mut egui::Ui, b: &mut PageBordersDlg) {
    widgets::section(ui, tr("layout.setting"));
    ui.horizontal(|ui| {
        if ui.button(tr("table.preset_none")).clicked() {
            b.sides = [false; 4];
        }
        if ui.button(tr("table.preset_box")).clicked() {
            b.sides = [true; 4];
        }
    });
    egui::Grid::new("page_border_sides").num_columns(2).show(ui, |ui| {
        let [t, bo, l, r] = &mut b.sides;
        ui.checkbox(t, tr("table.border_top"));
        ui.checkbox(bo, tr("table.border_bottom"));
        ui.end_row();
        ui.checkbox(l, tr("table.border_left"));
        ui.checkbox(r, tr("table.border_right"));
        ui.end_row();
    });
    widgets::section(ui, tr("shape.line"));
    widgets::form(ui, "page_border_form", |ui| {
        line_rows(ui, "page_border_line", &mut b.line);
        widgets::form_row(ui, tr("layout.measure_from"), |ui| {
            if ui.radio_value(&mut b.from_edge, false, tr("layout.from_text")).clicked() {
                b.line.space = 4.0;
            }
            if ui.radio_value(&mut b.from_edge, true, tr("layout.from_edge")).clicked() {
                b.line.space = 24.0;
            }
        });
        widgets::form_row(ui, tr("layout.distance"), |ui| {
            ui.add(egui::DragValue::new(&mut b.line.space).range(0.0..=31.0).speed(0.5).suffix(" pt"));
        });
    });
}

pub(super) fn borders_page(ui: &mut egui::Ui, b: &mut BordersDlg) {
    widgets::section(ui, tr("table.borders"));
    // The preset the sides make shows chosen.
    ui.horizontal(|ui| {
        const NONE: [bool; 5] = [false; 5];
        const BOX: [bool; 5] = [true, true, true, true, false];
        if ui.selectable_label(b.sides == NONE, tr("table.preset_none")).clicked() {
            b.sides = NONE;
        }
        if ui.selectable_label(b.sides == BOX, tr("table.preset_box")).clicked() {
            b.sides = BOX;
        }
    });
    egui::Grid::new("para_border_sides").num_columns(2).show(ui, |ui| {
        let [t, bo, l, r, between] = &mut b.sides;
        ui.checkbox(t, tr("table.border_top"));
        ui.checkbox(bo, tr("table.border_bottom"));
        ui.end_row();
        ui.checkbox(l, tr("table.border_left"));
        ui.checkbox(r, tr("table.border_right"));
        ui.end_row();
        ui.checkbox(between, tr("layout.border_between"));
        ui.end_row();
    });
    widgets::section(ui, tr("shape.line"));
    widgets::form(ui, "para_border_form", |ui| {
        line_rows(ui, "para_border_line", &mut b.line);
        widgets::form_row(ui, tr("layout.padding"), |ui| {
            ui.add(egui::DragValue::new(&mut b.line.space).range(0.0..=31.0).speed(0.5).suffix(" pt"));
        });
    });
    widgets::section(ui, tr("table.shading"));
    widgets::form(ui, "para_shading_form", |ui| {
        widgets::form_row(ui, tr("shape.fill"), |ui| {
            let chip = match b.shading {
                Some(c) => RichText::new("■").color(theme::rgb(c)),
                None => RichText::new(tr("table.no_color")),
            };
            crate::widgets::menu_button(ui, chip, |ui| {
                if let Some(c) = widgets::color_menu(ui, tr("table.no_color"), b.shading) {
                    b.shading = c;
                    ui.close();
                }
            });
        });
    });
}

pub(super) fn tabs_page(ui: &mut egui::Ui, t: &mut TabsDlg) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(tr("tabs.position"));
            len_field(ui, &mut t.pos, 0.0, 1500.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(140.0);
                egui::ScrollArea::vertical().max_height(130.0).min_scrolled_height(130.0).auto_shrink([false, false]).show(ui, |ui| {
                    for s in &t.stops {
                        let on = (s.pos - t.pos).abs() < 0.5;
                        if ui.selectable_label(on, len_text(s.pos)).clicked() {
                            (t.pos, t.align, t.leader) = (s.pos, s.align, s.leader);
                        }
                    }
                });
            });
        });
        ui.vertical(|ui| {
            ui.label(RichText::new(tr("tabs.alignment")).strong());
            for a in TabAlign::ALL {
                ui.radio_value(&mut t.align, a, tr(align_key(a)));
            }
            ui.add_space(4.0);
            ui.label(RichText::new(tr("tabs.leader")).strong());
            for l in TabLeader::ALL {
                ui.radio_value(&mut t.leader, l, tr(leader_key(l)));
            }
        });
    });
    ui.label(RichText::new(trf("tabs.default", &[("v", &len_text(apex_doc::DEFAULT_TAB))])).small().weak());
    ui.horizontal(|ui| {
        if ui.button(tr("tabs.set")).clicked() {
            let mut ts = TabStops::from_slice(&t.stops);
            ts.set(TabStop { pos: t.pos, align: t.align, leader: t.leader });
            t.stops = ts.as_slice().to_vec();
        }
        // Clearing asks for a stop to clear.
        let at_pos = t.stops.iter().any(|s| (s.pos - t.pos).abs() < 0.5);
        if ui.add_enabled(at_pos, egui::Button::new(tr("tabs.clear"))).clicked() {
            t.stops.retain(|s| (s.pos - t.pos).abs() >= 0.5);
        }
        if ui.add_enabled(!t.stops.is_empty(), egui::Button::new(tr("tabs.clear_all"))).clicked() {
            t.stops.clear();
        }
    });
}

fn drop_cap_page(ui: &mut egui::Ui, c: &mut DropCapDlg) {
    ui.label(RichText::new(tr("dropcap.position")).strong());
    ui.horizontal(|ui| {
        ui.radio_value(&mut c.mode, 0, tr("dropcap.none"));
        ui.radio_value(&mut c.mode, 1, tr("dropcap.dropped"));
        ui.radio_value(&mut c.mode, 2, tr("dropcap.in_margin"));
    });
    ui.add_space(6.0);
    ui.add_enabled_ui(c.mode != 0, |ui| {
        egui::Grid::new("drop_cap").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(tr("layout.font"));
            let shown = if c.font.is_empty() { tr("layout.font_default").to_string() } else { c.font.clone() };
            egui::ComboBox::from_id_salt("cap_font").width(170.0).selected_text(shown).height(300.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut c.font, String::new(), tr("layout.font_default"));
                for f in crate::fonts::families().iter() {
                    ui.selectable_value(&mut c.font, f.clone(), f.as_str());
                }
            });
            ui.end_row();
            ui.label(tr("dropcap.lines"));
            ui.add(egui::DragValue::new(&mut c.lines).range(1..=10));
            ui.end_row();
            ui.label(tr("dropcap.distance"));
            len_field(ui, &mut c.distance, 0.0, 144.0);
            ui.end_row();
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn margins(v: &DocView) -> (bool, [f32; 4]) {
        let p = v.doc.page;
        (p.mirror, [p.margin_top, p.margin_bottom, p.margin_left, p.margin_right])
    }

    #[test]
    fn mirrored_margins_undo_in_one_step() {
        let mut v = DocView::new(Document::from_plain_text("one\ntwo"));
        let before = margins(&v);
        let (_, m) = MARGINS[MARGINS.len() - 1];
        v.set_margins(m, true);
        assert_eq!(margins(&v), (true, m));
        v.undo();
        assert_eq!(margins(&v), before);
        v.redo();
        assert_eq!(margins(&v), (true, m));
    }

    #[test]
    fn page_setup_with_mirroring_undoes_in_one_step() {
        let mut v = DocView::new(Document::from_plain_text("one\ntwo"));
        let orig = v.doc.page;
        let page = PageSetup { mirror: true, margin_left: 100.0, ..orig };
        v.apply_page_setup(page, orig, (false, false), ApplyTo::Section);
        assert!(v.doc.page.mirror && v.doc.page.margin_left == 100.0);
        v.undo();
        assert_eq!(v.doc.page, orig);
    }
}
