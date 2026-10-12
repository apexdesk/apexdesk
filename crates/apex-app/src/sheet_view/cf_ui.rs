//! Conditional formatting: painting, icon sets, the menu of quick rules, the
//! rule editor and the rules manager.

use std::collections::HashMap;

use apex_sheet::data::{CfKind, CfResult, CfRule, CfStyle, Cfvo, CmpOp, ICON_SETS, Period, TextOp, icon_count, ranges_minus};
use apex_sheet::CfCache;

use super::data_ui::{DataDlg, cmp_key};
use super::*;

/// Conditional formatting prepared for painting (per sheet revision).
#[derive(Default)]
pub(super) struct CfState {
    key: Option<(usize, u64)>,
    cache: Option<CfCache>,
    cells: HashMap<CellRef, Option<CfResult>>,
}

pub(super) enum CfDlg {
    Quick(QuickDlg),
    Rule(Box<RuleDlg>),
    Rules(RulesDlg),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum QuickKind {
    Greater,
    Less,
    Between,
    Equal,
    Text,
    Date,
    Duplicates,
    Top { bottom: bool, percent: bool },
    Average { below: bool },
}

pub(super) struct QuickDlg {
    kind: QuickKind,
    a: String,
    b: String,
    period: Period,
    dup: bool,
    n: u32,
    preset: usize,
}

#[derive(Clone)]
pub(super) struct RulesDlg {
    list: Vec<CfRule>,
    selected: Option<usize>,
    /// Only rules touching the selection are listed.
    only_sel: bool,
}

#[derive(Clone, Default)]
struct CfvoEdit {
    /// 0 lowest, 1 highest, 2 number, 3 percent, 4 percentile, 5 formula.
    kind: usize,
    value: String,
}

pub(super) struct RuleDlg {
    /// Position in the manager's list when editing.
    index: Option<usize>,
    back: Option<RulesDlg>,
    ranges: String,
    /// 0 graphic, 1 cells that contain, 2 top/bottom, 3 average, 4 unique or
    /// duplicate, 5 formula.
    tab: usize,
    /// 0 two-color scale, 1 three-color scale, 2 data bar, 3 icon set.
    graphic: usize,
    stops: [(CfvoEdit, u32); 3],
    bar_color: u32,
    bar: [CfvoEdit; 2],
    icon_set: usize,
    icon_reverse: bool,
    icon_show: bool,
    icon_points: Vec<CfvoEdit>,
    /// 0 cell value, 1 specific text, 2 dates, 3 blanks, 4 no blanks,
    /// 5 errors, 6 no errors.
    contains: usize,
    op: CmpOp,
    a: String,
    b: String,
    text_op: TextOp,
    text: String,
    period: Period,
    bottom: bool,
    percent: bool,
    rank: u32,
    /// 0 above, 1 below, 2 equal or above, 3 equal or below.
    average: usize,
    dup: bool,
    formula: String,
    style: CfStyle,
    stop: bool,
    error: Option<String>,
}

const PRESETS: [&str; 6] = ["cf.preset_red", "cf.preset_yellow", "cf.preset_green", "cf.preset_red_fill", "cf.preset_red_text", "cf.preset_bold"];

pub(super) fn preset(i: usize) -> CfStyle {
    let (fill, color, bold) = match i {
        0 => (Some(0xFFC7CE), Some(0x9C0006), None),
        1 => (Some(0xFFEB9C), Some(0x9C5700), None),
        2 => (Some(0xC6EFCE), Some(0x006100), None),
        3 => (Some(0xFFC7CE), None, None),
        4 => (None, Some(0x9C0006), None),
        _ => (None, None, Some(true)),
    };
    CfStyle { fill, color, bold, ..Default::default() }
}

pub(super) const SCALES: [&[u32]; 12] = [
    &[0x63BE7B, 0xFFEB84, 0xF8696B],
    &[0xF8696B, 0xFFEB84, 0x63BE7B],
    &[0x63BE7B, 0xFCFCFF, 0xF8696B],
    &[0xF8696B, 0xFCFCFF, 0x63BE7B],
    &[0x5A8AC6, 0xFCFCFF, 0xF8696B],
    &[0xF8696B, 0xFCFCFF, 0x5A8AC6],
    &[0xFCFCFF, 0xF8696B],
    &[0xF8696B, 0xFCFCFF],
    &[0x63BE7B, 0xFCFCFF],
    &[0xFCFCFF, 0x63BE7B],
    &[0x63BE7B, 0xFFEF9C],
    &[0xFFEF9C, 0x63BE7B],
];

pub(super) const BARS: [u32; 6] = [0x638EC6, 0x63C384, 0xFF555A, 0xFFB628, 0x008AEF, 0xD6007B];

const PERIODS: [(Period, &str); 10] = [
    (Period::Yesterday, "period.yesterday"),
    (Period::Today, "period.today"),
    (Period::Tomorrow, "period.tomorrow"),
    (Period::Last7Days, "period.last7"),
    (Period::LastWeek, "period.last_week"),
    (Period::ThisWeek, "period.this_week"),
    (Period::NextWeek, "period.next_week"),
    (Period::LastMonth, "period.last_month"),
    (Period::ThisMonth, "period.this_month"),
    (Period::NextMonth, "period.next_month"),
];

const TEXT_OPS: [(TextOp, &str); 4] = [(TextOp::Contains, "cf.text_contains"), (TextOp::NotContains, "cf.text_not_contains"), (TextOp::BeginsWith, "cf.text_begins"), (TextOp::EndsWith, "cf.text_ends")];

fn period_key(p: Period) -> &'static str {
    PERIODS.iter().find(|x| x.0 == p).map_or("", |x| x.1)
}

/// Icon thresholds split evenly in percent.
pub(super) fn default_points(n: usize) -> Vec<Cfvo> {
    (1..n).map(|i| Cfvo::Percent((i * 100 / n) as f64)).collect()
}

/// The cell style with conditional formatting on top.
pub(super) fn cf_cell_style(st: &CellStyle, s: &CfStyle) -> CellStyle {
    let mut out = st.clone();
    if let Some(c) = s.color {
        out.color = Some(c);
    }
    if let Some(b) = s.bold {
        out.bold = b;
    }
    if let Some(b) = s.italic {
        out.italic = b;
    }
    if let Some(b) = s.underline {
        out.underline = b;
    }
    if let Some(b) = s.strike {
        out.strike = b;
    }
    out
}

/// Paints icon `idx` (0 = lowest) of an icon set.
pub(super) fn paint_icon(p: &egui::Painter, r: Rect, set: &str, idx: u8) {
    let n = icon_count(set);
    let i = (idx as usize).min(n - 1);
    let frac = i as f32 / (n - 1).max(1) as f32;
    let red = Color32::from_rgb(0xE0, 0x40, 0x40);
    let yellow = Color32::from_rgb(0xF0, 0xB4, 0x1C);
    let green = Color32::from_rgb(0x3A, 0xA8, 0x5A);
    let gray = Color32::from_gray(128);
    let black = Color32::from_gray(40);
    let c = r.center();
    let s = r.width().min(r.height()) * 0.5;
    let traffic = |i: usize| if i == 0 { red } else if i + 1 == n { green } else { yellow };
    if set.contains("Arrows") {
        let color = if set.contains("Gray") { gray } else { traffic(i) };
        let a = (frac * 2.0 - 1.0) * std::f32::consts::FRAC_PI_2;
        let d = vec2(a.cos(), -a.sin());
        let (from, tip) = (c - d * s * 0.8, c + d * s * 0.8);
        p.line_segment([from, tip - d * s * 0.3], Stroke::new(s * 0.32, color));
        let side = vec2(-d.y, d.x) * s * 0.5;
        p.add(egui::Shape::convex_polygon(vec![tip, tip - d * s * 0.6 + side, tip - d * s * 0.6 - side], color, Stroke::NONE));
    } else if set == "4RedToBlack" {
        let colors = [black, gray, Color32::from_rgb(0xF0, 0x90, 0x90), red];
        p.circle_filled(c, s * 0.75, colors[i]);
    } else if set.contains("TrafficLights") || set == "3Signs" {
        let color = if n == 4 { [black, red, yellow, green][i] } else { traffic(i) };
        if set == "3Signs" && i == 0 {
            p.add(egui::Shape::convex_polygon(vec![c + vec2(0.0, -s * 0.85), c + vec2(s * 0.85, 0.0), c + vec2(0.0, s * 0.85), c + vec2(-s * 0.85, 0.0)], color, Stroke::NONE));
        } else if set == "3Signs" && i == 1 {
            p.add(egui::Shape::convex_polygon(vec![c + vec2(0.0, -s * 0.8), c + vec2(s * 0.85, s * 0.7), c + vec2(-s * 0.85, s * 0.7)], color, Stroke::NONE));
        } else {
            p.circle_filled(c, s * 0.75, color);
        }
    } else if set.starts_with("3Symbols") {
        let color = traffic(i);
        let mark = if set == "3Symbols" {
            p.circle_filled(c, s * 0.85, color);
            Color32::WHITE
        } else {
            color
        };
        let w = Stroke::new(s * 0.22, mark);
        let k = s * 0.4;
        match i {
            0 => {
                p.line_segment([c + vec2(-k, -k), c + vec2(k, k)], w);
                p.line_segment([c + vec2(-k, k), c + vec2(k, -k)], w);
            }
            1 => {
                p.line_segment([c + vec2(0.0, -k * 1.2), c + vec2(0.0, k * 0.3)], w);
                p.circle_filled(c + vec2(0.0, k * 0.95), s * 0.12, mark);
            }
            _ => {
                p.add(egui::Shape::line(vec![c + vec2(-k, 0.0), c + vec2(-k * 0.2, k * 0.8), c + vec2(k, -k * 0.8)], w));
            }
        }
    } else if set == "3Flags" {
        let color = traffic(i);
        let x = c.x - s * 0.55;
        p.line_segment([pos2(x, c.y - s * 0.85), pos2(x, c.y + s * 0.85)], Stroke::new(s * 0.14, black));
        p.add(egui::Shape::convex_polygon(vec![pos2(x, c.y - s * 0.85), pos2(c.x + s * 0.75, c.y - s * 0.4), pos2(x, c.y + s * 0.05)], color, Stroke::NONE));
    } else if set == "3Stars" {
        let gold = Color32::from_rgb(0xF2, 0xB8, 0x1B);
        let pts: Vec<Pos2> = (0..10)
            .map(|k| {
                let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
                let rad = if k % 2 == 0 { s * 0.9 } else { s * 0.38 };
                c + vec2(a.cos(), a.sin()) * rad
            })
            .collect();
        let fill = |p: &egui::Painter| {
            let inner: Vec<Pos2> = pts.iter().skip(1).step_by(2).copied().collect();
            p.add(egui::Shape::convex_polygon(inner.clone(), gold, Stroke::NONE));
            for k in 0..5 {
                p.add(egui::Shape::convex_polygon(vec![pts[k * 2], pts[(k * 2 + 1) % 10], pts[(k * 2 + 9) % 10]], gold, Stroke::NONE));
            }
        };
        match i {
            0 => {}
            1 => fill(&p.with_clip_rect(Rect::from_min_max(r.min, pos2(c.x, r.max.y)).intersect(p.clip_rect()))),
            _ => fill(p),
        }
        p.add(egui::Shape::closed_line(pts, Stroke::new(1.0, gold)));
    } else if set == "3Triangles" {
        match i {
            0 => {
                p.add(egui::Shape::convex_polygon(vec![c + vec2(-s * 0.75, -s * 0.45), c + vec2(s * 0.75, -s * 0.45), c + vec2(0.0, s * 0.65)], red, Stroke::NONE));
            }
            1 => {
                p.rect_filled(Rect::from_center_size(c, vec2(s * 1.3, s * 0.4)), 0.0, yellow);
            }
            _ => {
                p.add(egui::Shape::convex_polygon(vec![c + vec2(0.0, -s * 0.65), c + vec2(s * 0.75, s * 0.45), c + vec2(-s * 0.75, s * 0.45)], green, Stroke::NONE));
            }
        }
    } else {
        // Rating bars.
        let filled = if n == 5 { i } else { i + 1 };
        let bw = s * 0.36;
        for k in 0..4 {
            let h = s * (0.5 + 0.4 * k as f32);
            let x = r.min.x + (r.width() - 4.0 * bw - 3.0 * bw * 0.35) / 2.0 + k as f32 * bw * 1.35;
            let color = if k < filled { Color32::from_rgb(0x30, 0x70, 0xC0) } else { Color32::from_gray(205) };
            p.rect_filled(Rect::from_min_max(pos2(x, c.y + s * 0.85 - h), pos2(x + bw, c.y + s * 0.85)), 0.0, color);
        }
    }
}

/// A horizontal gradient from `color` to near white, like Excel's data bars.
pub(super) fn paint_bar(p: &egui::Painter, r: Rect, color: Color32) {
    let mix = |v: u8| ((v as u16 + 3 * 255) / 4) as u8;
    let light = Color32::from_rgb(mix(color.r()), mix(color.g()), mix(color.b()));
    let mut mesh = egui::Mesh::default();
    let i = mesh.vertices.len() as u32;
    for (pos, c) in [(r.left_top(), color), (r.right_top(), light), (r.right_bottom(), light), (r.left_bottom(), color)] {
        mesh.colored_vertex(pos, c);
    }
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
    p.add(mesh);
    p.rect_stroke(r, 0.0, Stroke::new(1.0, color), StrokeKind::Inside);
}

/// "AaBbCcYyZz" in a conditional format.
/// What a rule's format does, in words (the preview's name for screen
/// readers).
fn style_words(st: &CfStyle) -> String {
    if st.is_empty() {
        return tr("cf.no_format").to_string();
    }
    let mut w: Vec<String> = Vec::new();
    for (key, c) in [("cf.font_color", st.color), ("cf.fill", st.fill)] {
        if let Some(c) = c {
            w.push(format!("{} #{c:06X}", tr(key)));
        }
    }
    for (key, on) in [("format.bold", st.bold), ("format.italic", st.italic), ("format.underline", st.underline), ("format.strike", st.strike)] {
        if let Some(on) = on {
            w.push(if on { tr(key).to_string() } else { format!("-{}", tr(key)) });
        }
    }
    w.join(", ")
}

fn style_preview(ui: &mut egui::Ui, st: &CfStyle, size: Vec2) {
    let (r, resp) = ui.allocate_exact_size(size, Sense::hover());
    widgets::name(&resp, egui::WidgetType::Label, &format!("{} {}", tr("cf.preview"), style_words(st)), None);
    let p = ui.painter();
    p.rect_filled(r, 0.0, st.fill.map_or(Paper::BG, theme::rgb));
    p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_gray(170)), StrokeKind::Inside);
    if st.is_empty() {
        p.text(r.center(), egui::Align2::CENTER_CENTER, tr("cf.no_format"), FontId::proportional(12.0), Color32::from_gray(120));
        return;
    }
    let cs = cf_cell_style(&CellStyle::default(), st);
    let color = st.color.map_or(Paper::TEXT, theme::rgb);
    let g = p.layout_job(cell_job("AaBbCcYyZz", &cs, color, 1.0, None));
    p.galley(r.center() - g.size() / 2.0, g, color);
}

impl CfvoEdit {
    fn from(c: &Cfvo, v: &SheetView) -> CfvoEdit {
        let num = |n: f64| v.wb.format_value(0, &Value::Number(n)).0;
        match c {
            Cfvo::Min => CfvoEdit { kind: 0, value: String::new() },
            Cfvo::Max => CfvoEdit { kind: 1, value: String::new() },
            Cfvo::Num(n) => CfvoEdit { kind: 2, value: num(*n) },
            Cfvo::Percent(n) => CfvoEdit { kind: 3, value: num(*n) },
            Cfvo::Percentile(n) => CfvoEdit { kind: 4, value: num(*n) },
            Cfvo::Formula(f) => CfvoEdit { kind: 5, value: format!("={}", v.formula_display(f)) },
        }
    }

    fn to(&self, wb: &Workbook) -> Cfvo {
        let n = || apex_sheet::format::parse_input(self.value.trim(), &wb.locale).value.as_number().unwrap_or(0.0);
        match self.kind {
            0 => Cfvo::Min,
            1 => Cfvo::Max,
            2 => Cfvo::Num(n()),
            3 => Cfvo::Percent(n()),
            4 => Cfvo::Percentile(n()),
            _ => {
                let t = self.value.trim();
                Cfvo::Formula(wb.rule_input(&if t.starts_with('=') { t.to_string() } else { format!("={t}") }))
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, extremes: bool) {
        const KINDS: [&str; 6] = ["cf.lowest", "cf.highest", "cf.number", "cf.percent", "cf.percentile", "cf.formula"];
        egui::ComboBox::from_id_salt(id).width(110.0).selected_text(tr(KINDS[self.kind])).show_ui(ui, |ui| {
            for (k, key) in KINDS.iter().enumerate() {
                if extremes || k >= 2 {
                    ui.selectable_value(&mut self.kind, k, tr(key));
                }
            }
        });
        ui.add_enabled(self.kind >= 2, egui::TextEdit::singleline(&mut self.value).desired_width(90.0));
    }
}

impl RuleDlg {
    fn new(ranges: String) -> RuleDlg {
        RuleDlg {
            index: None,
            back: None,
            ranges,
            tab: 1,
            graphic: 1,
            stops: [
                (CfvoEdit { kind: 0, value: String::new() }, 0xF8696B),
                (CfvoEdit { kind: 4, value: "50".into() }, 0xFFEB84),
                (CfvoEdit { kind: 1, value: String::new() }, 0x63BE7B),
            ],
            bar_color: BARS[0],
            bar: [CfvoEdit { kind: 0, value: String::new() }, CfvoEdit { kind: 1, value: String::new() }],
            icon_set: 0,
            icon_reverse: false,
            icon_show: true,
            icon_points: vec![CfvoEdit { kind: 3, value: "33".into() }, CfvoEdit { kind: 3, value: "67".into() }],
            contains: 0,
            op: CmpOp::Between,
            a: String::new(),
            b: String::new(),
            text_op: TextOp::Contains,
            text: String::new(),
            period: Period::Today,
            bottom: false,
            percent: false,
            rank: 10,
            average: 0,
            dup: true,
            formula: String::new(),
            style: preset(0),
            stop: false,
            error: None,
        }
    }

    fn set_points(&mut self, v: &SheetView) {
        let n = ICON_SETS[self.icon_set].1 as usize;
        if self.icon_points.len() != n - 1 {
            self.icon_points = default_points(n).iter().map(|c| CfvoEdit::from(c, v)).collect();
        }
    }
}

impl SheetView {
    fn ranges_text(&self, ranges: &[Range]) -> String {
        let sep = if self.wb.locale.decimal == ',' { "; " } else { ", " };
        ranges.iter().map(|r| if r.is_single() { r.start.to_a1() } else { r.to_a1() }).collect::<Vec<_>>().join(sep)
    }

    fn parse_ranges(text: &str) -> Option<Vec<Range>> {
        let t = text.trim().trim_start_matches('=');
        let mut out = Vec::new();
        for part in t.split([',', ';', ' ']).map(str::trim).filter(|s| !s.is_empty()) {
            let p = part.replace('$', "");
            out.push(Range::parse(&p).or_else(|| CellRef::parse(&p).map(Range::single))?);
        }
        (!out.is_empty()).then_some(out)
    }

    // --- painting ------------------------------------------------------------------

    fn cf_at(&self, st: &mut CfState, c: CellRef) -> Option<CfResult> {
        if let Some(r) = st.cells.get(&c) {
            return r.clone();
        }
        let r = self.wb.cf_eval(self.sheet, st.cache.as_ref()?, c);
        st.cells.insert(c, r.clone());
        r
    }

    /// Conditional formatting of the cells in `visible`.
    pub(super) fn cf_map(&self, visible: &Range) -> HashMap<CellRef, CfResult> {
        let sh = self.sh();
        let mut out = HashMap::new();
        if sh.cond_formats.is_empty() {
            return out;
        }
        let mut st = self.data.cf.borrow_mut();
        let key = (self.sheet, self.wb.revision);
        if st.key != Some(key) {
            st.key = Some(key);
            st.cache = Some(apex_sheet::functions::shared_budget(1, 1 << 26, || self.wb.cf_prepare(self.sheet)));
            st.cells.clear();
        }
        if st.cells.len() > 100_000 {
            st.cells.clear();
        }
        // The screen's formulas share one budget, as one formula.
        apex_sheet::functions::shared_budget(1, 1 << 26, || {
            for rule in &sh.cond_formats {
                for r in &rule.ranges {
                    let Some(i) = r.intersect(visible) else { continue };
                    if i.cell_count() > 20_000 {
                        continue;
                    }
                    for row in i.start.row..=i.end.row {
                        if sh.row_height(row) == 0.0 {
                            continue;
                        }
                        for col in i.start.col..=i.end.col {
                            let c = CellRef::new(row, col);
                            if out.contains_key(&c) {
                                continue;
                            }
                            if let Some(res) = self.cf_at(&mut st, c) {
                                out.insert(c, res);
                            }
                        }
                    }
                }
            }
        });
        out
    }

    /// Fills, data bars and icons of conditional formats.
    pub(super) fn paint_cf(&self, p: &egui::Painter, g: &Geo, map: &HashMap<CellRef, CfResult>) {
        let sh = self.sh();
        for (c, res) in map {
            let r = g.cell_rect(sh, *c);
            if r.width() < 1.0 {
                continue;
            }
            if let Some(f) = res.style.fill {
                p.rect_filled(r, 0.0, theme::rgb(f));
            }
            if let Some((f, color)) = res.bar {
                let inner = r.shrink2(vec2(2.0, 2.0));
                let w = (inner.width() * f).max(2.0);
                paint_bar(p, Rect::from_min_size(inner.min, vec2(w, inner.height())), theme::rgb(color));
            }
            if let Some((set, i)) = &res.icon {
                let s = (r.height() - 4.0).min(16.0 * g.z).max(6.0f32.min(16.0 * g.z));
                paint_icon(p, Rect::from_min_size(pos2(r.min.x + 2.0, r.center().y - s / 2.0), vec2(s, s)), set, *i);
            }
        }
    }

    // --- menus ---------------------------------------------------------------------

    pub(super) fn quick(&mut self, kind: QuickKind) {
        self.settle_edit();
        let s = self.sel();
        let r = self.wb.clamp_to_used(self.sheet, &s).unwrap_or(s);
        let mut nums = Vec::new();
        if r.cell_count() < 1_000_000 {
            self.sh().for_each_in(&r, |_, cell| {
                if let Value::Number(n) = cell.value() {
                    nums.push(*n);
                }
            });
        }
        nums.sort_by(|a, b| a.total_cmp(b));
        let fmt = |n: f64| self.wb.format_value(0, &Value::Number((n * 100.0).round() / 100.0)).0;
        let (a, b) = match (nums.first(), nums.last()) {
            (Some(lo), Some(hi)) => {
                let avg = nums.iter().sum::<f64>() / nums.len() as f64;
                match kind {
                    QuickKind::Between => (fmt(lo + (hi - lo) * 0.25), fmt(lo + (hi - lo) * 0.75)),
                    _ => (fmt(avg), String::new()),
                }
            }
            _ => (String::new(), String::new()),
        };
        self.data.dialog = Some(DataDlg::Cf(CfDlg::Quick(QuickDlg { kind, a, b, period: Period::Yesterday, dup: true, n: 10, preset: 0 })));
    }

    pub(super) fn add_rule(&mut self, rule: CfRule) {
        let mut list = self.sh().cond_formats.clone();
        list.insert(0, rule);
        self.wb.execute(Op::CondFormats { sheet: self.sheet, list });
    }

    pub(super) fn graphic_rule(&mut self, kind: CfKind) {
        self.settle_edit();
        let ranges = vec![self.sel()];
        self.add_rule(CfRule { ranges, kind, style: CfStyle::default(), stop: false });
    }

    pub(super) fn open_new_rule(&mut self) {
        self.settle_edit();
        let ranges = self.ranges_text(&[self.sel()]);
        self.data.dialog = Some(DataDlg::Cf(CfDlg::Rule(Box::new(RuleDlg::new(ranges)))));
    }

    pub(super) fn open_rules(&mut self) {
        self.settle_edit();
        let list = self.sh().cond_formats.clone();
        let only_sel = list.iter().any(|r| r.ranges.iter().any(|x| x.intersect(&self.sel()).is_some()));
        self.data.dialog = Some(DataDlg::Cf(CfDlg::Rules(RulesDlg { list, selected: None, only_sel })));
    }

    pub(super) fn clear_rules(&mut self, whole_sheet: bool) {
        self.settle_edit();
        let sel = self.sel();
        let list: Vec<CfRule> = if whole_sheet {
            Vec::new()
        } else {
            self.sh()
                .cond_formats
                .iter()
                .cloned()
                .filter_map(|mut r| {
                    r.ranges = ranges_minus(&r.ranges, &sel);
                    (!r.ranges.is_empty()).then_some(r)
                })
                .collect()
        };
        if list != self.sh().cond_formats {
            self.wb.execute(Op::CondFormats { sheet: self.sheet, list });
        }
    }

    pub(super) fn cf_menu(&mut self, ui: &mut egui::Ui) {
        crate::widgets::menu_button(ui, tr("cf.highlight"), |ui| {
            for (k, key) in [
                (QuickKind::Greater, "cf.greater"),
                (QuickKind::Less, "cf.less"),
                (QuickKind::Between, "cf.between"),
                (QuickKind::Equal, "cf.equal"),
                (QuickKind::Text, "cf.text"),
                (QuickKind::Date, "cf.date"),
                (QuickKind::Duplicates, "cf.duplicates"),
            ] {
                if ui.button(format!("{}…", tr(key))).clicked() {
                    self.quick(k);
                    ui.close();
                }
            }
        });
        crate::widgets::menu_button(ui, tr("cf.top_bottom"), |ui| {
            for (k, key) in [
                (QuickKind::Top { bottom: false, percent: false }, "cf.top10"),
                (QuickKind::Top { bottom: false, percent: true }, "cf.top10p"),
                (QuickKind::Top { bottom: true, percent: false }, "cf.bottom10"),
                (QuickKind::Top { bottom: true, percent: true }, "cf.bottom10p"),
                (QuickKind::Average { below: false }, "cf.above_avg"),
                (QuickKind::Average { below: true }, "cf.below_avg"),
            ] {
                if ui.button(format!("{}…", tr(key))).clicked() {
                    self.quick(k);
                    ui.close();
                }
            }
        });
        crate::widgets::menu_button(ui, tr("cf.data_bars"), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(150.0);
                for c in BARS {
                    let (r, resp) = ui.allocate_exact_size(vec2(40.0, 26.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 2.0, ui.visuals().widgets.hovered.bg_fill);
                    }
                    for (k, f) in [0.9, 0.55, 0.3].iter().enumerate() {
                        let y = r.min.y + 3.0 + k as f32 * 7.0;
                        paint_bar(ui.painter(), Rect::from_min_size(pos2(r.min.x + 3.0, y), vec2((r.width() - 6.0) * f, 6.0)), theme::rgb(c));
                    }
                    if resp.clicked() {
                        self.graphic_rule(CfKind::DataBar { color: c, min: Cfvo::Min, max: Cfvo::Max });
                        ui.close();
                    }
                }
            });
        });
        crate::widgets::menu_button(ui, tr("cf.color_scales"), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(170.0);
                for colors in SCALES {
                    let (r, resp) = ui.allocate_exact_size(vec2(36.0, 30.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 2.0, ui.visuals().widgets.hovered.bg_fill);
                    }
                    let inner = r.shrink(4.0);
                    let h = inner.height() / 3.0;
                    for k in 0..3 {
                        let t = k as f32 / 2.0;
                        let col = if colors.len() == 3 { colors[k] } else { lerp(colors[0], colors[1], t) };
                        ui.painter().rect_filled(Rect::from_min_size(pos2(inner.min.x, inner.min.y + h * k as f32), vec2(inner.width(), h)), 0.0, theme::rgb(col));
                    }
                    if resp.clicked() {
                        let stops = if colors.len() == 3 {
                            vec![(Cfvo::Min, colors[2]), (Cfvo::Percentile(50.0), colors[1]), (Cfvo::Max, colors[0])]
                        } else {
                            vec![(Cfvo::Min, colors[1]), (Cfvo::Max, colors[0])]
                        };
                        self.graphic_rule(CfKind::ColorScale(stops));
                        ui.close();
                    }
                }
            });
        });
        crate::widgets::menu_button(ui, tr("cf.icon_sets"), |ui| {
            for (set, n) in ICON_SETS {
                let (r, resp) = ui.allocate_exact_size(vec2(24.0 * n as f32 + 10.0, 22.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 2.0, ui.visuals().widgets.hovered.bg_fill);
                }
                for k in 0..n {
                    paint_icon(ui.painter(), Rect::from_min_size(pos2(r.min.x + 5.0 + 24.0 * k as f32, r.min.y + 3.0), vec2(16.0, 16.0)), set, n - 1 - k);
                }
                if resp.clicked() {
                    self.graphic_rule(CfKind::IconSet { set: set.to_string(), reverse: false, show_value: true, points: default_points(n as usize) });
                    ui.close();
                }
            }
        });
        ui.separator();
        if ui.button(format!("{}…", tr("cf.new_rule"))).clicked() {
            self.open_new_rule();
            ui.close();
        }
        crate::widgets::menu_button(ui, tr("cf.clear"), |ui| {
            if ui.button(tr("cf.clear_sel")).clicked() {
                self.clear_rules(false);
                ui.close();
            }
            if ui.button(tr("cf.clear_sheet")).clicked() {
                self.clear_rules(true);
                ui.close();
            }
        });
        if ui.button(format!("{}…", tr("cf.manage"))).clicked() {
            self.open_rules();
            ui.close();
        }
    }

    // --- dialogs -------------------------------------------------------------------

    /// Shows a conditional formatting dialog; may switch to another one.
    pub(super) fn cf_dialog(&mut self, ctx: &egui::Context, d: &mut CfDlg) -> (widgets::DialogResult, Option<DataDlg>) {
        use widgets::DialogResult as R;
        match d {
            CfDlg::Quick(q) => {
                let res = widgets::dialog(ctx, "cf_quick", tr(quick_title(q.kind)), 460.0, |ui| quick_ui(ui, q));
                if res == R::Ok {
                    let rule = self.quick_rule(q);
                    self.add_rule(rule);
                }
                (res, None)
            }
            CfDlg::Rule(r) => {
                let title = if r.index.is_some() { tr("cf.edit_rule") } else { tr("cf.new_rule") };
                let pages = [
                    (Icon::CondFormat, "cf.page_graphic"),
                    (Icon::Search, "cf.page_contains"),
                    (Icon::SortDesc, "cf.page_top"),
                    (Icon::Sigma, "cf.page_average"),
                    (Icon::RemoveDuplicates, "cf.page_unique"),
                    (Icon::Function, "cf.page_formula"),
                ];
                let mut tab = r.tab;
                let mut res = widgets::nav_dialog(ctx, "cf_rule", title, &pages, &mut tab, vec2(560.0, 400.0), |ui, page| {
                    r.tab = page;
                    self.rule_ui(ui, r)
                });
                r.tab = tab;
                if res == R::Ok {
                    match self.rule_from(r) {
                        Ok(rule) => {
                            if let Some(mut back) = r.back.take() {
                                match r.index {
                                    Some(i) if i < back.list.len() => back.list[i] = rule,
                                    _ => {
                                        back.list.insert(0, rule);
                                        back.selected = Some(0);
                                    }
                                }
                                return (R::None, Some(DataDlg::Cf(CfDlg::Rules(back))));
                            }
                            self.add_rule(rule);
                        }
                        Err(e) => {
                            r.error = Some(e);
                            res = R::None;
                        }
                    }
                } else if res == R::Cancel {
                    if let Some(back) = r.back.take() {
                        return (R::None, Some(DataDlg::Cf(CfDlg::Rules(back))));
                    }
                }
                (res, None)
            }
            CfDlg::Rules(m) => {
                let mut next = None;
                let res = widgets::dialog(ctx, "cf_rules", tr("cf.manager"), 640.0, |ui| next = self.rules_ui(ui, m));
                if let Some(n) = next {
                    return (R::None, Some(n));
                }
                if res == R::Ok && m.list != self.sh().cond_formats {
                    self.wb.execute(Op::CondFormats { sheet: self.sheet, list: m.list.clone() });
                }
                (res, None)
            }
        }
    }

    fn quick_rule(&self, q: &QuickDlg) -> CfRule {
        let kind = match q.kind {
            QuickKind::Greater => CfKind::CellIs { op: CmpOp::Greater, f1: self.wb.rule_input(&q.a), f2: String::new() },
            QuickKind::Less => CfKind::CellIs { op: CmpOp::Less, f1: self.wb.rule_input(&q.a), f2: String::new() },
            QuickKind::Equal => CfKind::CellIs { op: CmpOp::Equal, f1: self.wb.rule_input(&q.a), f2: String::new() },
            QuickKind::Between => CfKind::CellIs { op: CmpOp::Between, f1: self.wb.rule_input(&q.a), f2: self.wb.rule_input(&q.b) },
            QuickKind::Text => CfKind::Text { op: TextOp::Contains, text: q.a.clone() },
            QuickKind::Date => CfKind::Period(q.period),
            QuickKind::Duplicates => CfKind::Duplicates(q.dup),
            QuickKind::Top { bottom, percent } => CfKind::Top { bottom, percent, rank: q.n },
            QuickKind::Average { below } => CfKind::Average { below, equal: false },
        };
        CfRule { ranges: vec![self.sel()], kind, style: preset(q.preset), stop: false }
    }

    fn rule_dlg_from(&self, rule: &CfRule, index: Option<usize>, back: Option<RulesDlg>) -> RuleDlg {
        let mut d = RuleDlg::new(self.ranges_text(&rule.ranges));
        d.index = index;
        d.back = back;
        d.style = rule.style.clone();
        d.stop = rule.stop;
        match &rule.kind {
            CfKind::ColorScale(stops) => {
                d.tab = 0;
                d.graphic = if stops.len() == 3 { 1 } else { 0 };
                let order: Vec<usize> = if stops.len() == 3 { vec![0, 1, 2] } else { vec![0, 2] };
                for (k, (c, col)) in order.into_iter().zip(stops) {
                    d.stops[k] = (CfvoEdit::from(c, self), *col);
                }
            }
            CfKind::DataBar { color, min, max } => {
                d.tab = 0;
                d.graphic = 2;
                d.bar_color = *color;
                d.bar = [CfvoEdit::from(min, self), CfvoEdit::from(max, self)];
            }
            CfKind::IconSet { set, reverse, show_value, points } => {
                d.tab = 0;
                d.graphic = 3;
                d.icon_set = ICON_SETS.iter().position(|x| x.0 == set).unwrap_or(0);
                d.icon_reverse = *reverse;
                d.icon_show = *show_value;
                d.icon_points = points.iter().map(|c| CfvoEdit::from(c, self)).collect();
                d.set_points(self);
            }
            CfKind::CellIs { op, f1, f2 } => {
                d.contains = 0;
                d.op = *op;
                d.a = self.wb.rule_display(f1);
                d.b = self.wb.rule_display(f2);
            }
            CfKind::Text { op, text } => {
                d.contains = 1;
                d.text_op = *op;
                d.text = text.clone();
            }
            CfKind::Period(p) => {
                d.contains = 2;
                d.period = *p;
            }
            CfKind::Blanks(b) => d.contains = if *b { 3 } else { 4 },
            CfKind::Errors(b) => d.contains = if *b { 5 } else { 6 },
            CfKind::Top { bottom, percent, rank } => {
                d.tab = 2;
                d.bottom = *bottom;
                d.percent = *percent;
                d.rank = *rank;
            }
            CfKind::Average { below, equal } => {
                d.tab = 3;
                d.average = match (below, equal) {
                    (false, false) => 0,
                    (true, false) => 1,
                    (false, true) => 2,
                    (true, true) => 3,
                };
            }
            CfKind::Duplicates(b) => {
                d.tab = 4;
                d.dup = *b;
            }
            CfKind::Expression(f) => {
                d.tab = 5;
                d.formula = format!("={}", self.formula_display(f));
            }
        }
        d
    }

    fn rule_from(&self, d: &RuleDlg) -> Result<CfRule, String> {
        let ranges = Self::parse_ranges(&d.ranges).ok_or_else(|| tr("cf.bad_range").to_string())?;
        let kind = match d.tab {
            0 => match d.graphic {
                0 => CfKind::ColorScale(vec![(d.stops[0].0.to(&self.wb), d.stops[0].1), (d.stops[2].0.to(&self.wb), d.stops[2].1)]),
                1 => CfKind::ColorScale(d.stops.iter().map(|(c, col)| (c.to(&self.wb), *col)).collect()),
                2 => CfKind::DataBar { color: d.bar_color, min: d.bar[0].to(&self.wb), max: d.bar[1].to(&self.wb) },
                _ => CfKind::IconSet { set: ICON_SETS[d.icon_set].0.to_string(), reverse: d.icon_reverse, show_value: d.icon_show, points: d.icon_points.iter().map(|c| c.to(&self.wb)).collect() },
            },
            1 => match d.contains {
                0 => {
                    if d.a.trim().is_empty() {
                        return Err(tr("cf.need_value").to_string());
                    }
                    CfKind::CellIs { op: d.op, f1: self.wb.rule_input(&d.a), f2: if d.op.two_args() { self.wb.rule_input(&d.b) } else { String::new() } }
                }
                1 => CfKind::Text { op: d.text_op, text: d.text.clone() },
                2 => CfKind::Period(d.period),
                3 => CfKind::Blanks(true),
                4 => CfKind::Blanks(false),
                5 => CfKind::Errors(true),
                _ => CfKind::Errors(false),
            },
            2 => CfKind::Top { bottom: d.bottom, percent: d.percent, rank: d.rank.max(1) },
            3 => CfKind::Average { below: d.average % 2 == 1, equal: d.average >= 2 },
            4 => CfKind::Duplicates(d.dup),
            _ => {
                let t = d.formula.trim();
                let t = if t.starts_with('=') { t.to_string() } else { format!("={t}") };
                self.wb.check_formula(&t).map_err(|e| trf("sheet.bad_formula", &[("error", &e)]))?;
                CfKind::Expression(self.wb.rule_input(&t))
            }
        };
        let style = if kind.is_graphic() { CfStyle::default() } else { d.style.clone() };
        Ok(CfRule { ranges, kind, style, stop: d.stop })
    }

    fn rule_ui(&self, ui: &mut egui::Ui, d: &mut RuleDlg) {
        let types = ["cf.type_graphic", "cf.type_contains", "cf.type_top", "cf.type_average", "cf.type_unique", "cf.type_formula"];
        widgets::section(ui, tr(types[d.tab.min(5)]));
        {
            match d.tab {
                0 => {
                    ui.horizontal(|ui| {
                        ui.label(tr("cf.format_style"));
                        let names = ["cf.scale2", "cf.scale3", "cf.data_bar", "cf.icon_set"];
                        egui::ComboBox::from_id_salt("cf_graphic").selected_text(tr(names[d.graphic])).show_ui(ui, |ui| {
                            for (i, key) in names.iter().enumerate() {
                                ui.selectable_value(&mut d.graphic, i, tr(key));
                            }
                        });
                    });
                    match d.graphic {
                        0 | 1 => {
                            let idx: &[usize] = if d.graphic == 0 { &[0, 2] } else { &[0, 1, 2] };
                            egui::Grid::new("cf_scale").num_columns(4).show(ui, |ui| {
                                for &k in idx {
                                    ui.label(tr(["cf.minimum", "cf.midpoint", "cf.maximum"][k]));
                                    d.stops[k].0.ui(ui, ("stop", k), k != 1);
                                    color_button(ui, ("stopc", k), &mut d.stops[k].1);
                                    ui.end_row();
                                }
                            });
                        }
                        2 => {
                            egui::Grid::new("cf_bar").num_columns(3).show(ui, |ui| {
                                for k in 0..2 {
                                    ui.label(tr(["cf.minimum", "cf.maximum"][k]));
                                    d.bar[k].ui(ui, ("bar", k), true);
                                    ui.end_row();
                                }
                                ui.label(tr("cf.bar_color"));
                                color_button(ui, "barc", &mut d.bar_color);
                                ui.end_row();
                            });
                        }
                        _ => {
                            ui.horizontal(|ui| {
                                ui.label(tr("cf.icon_style"));
                                let cur = d.icon_set;
                                egui::ComboBox::from_id_salt("cf_icons").width(150.0).selected_text(ICON_SETS[cur].0).show_ui(ui, |ui| {
                                    for (i, (set, n)) in ICON_SETS.iter().enumerate() {
                                        let (r, resp) = ui.allocate_exact_size(vec2(22.0 * *n as f32 + 8.0, 20.0), Sense::click());
                                        if resp.hovered() || i == cur {
                                            ui.painter().rect_filled(r, 2.0, ui.visuals().widgets.hovered.bg_fill);
                                        }
                                        for k in 0..*n {
                                            paint_icon(ui.painter(), Rect::from_min_size(pos2(r.min.x + 4.0 + 22.0 * k as f32, r.min.y + 2.0), vec2(16.0, 16.0)), set, n - 1 - k);
                                        }
                                        if resp.clicked() {
                                            d.icon_set = i;
                                        }
                                    }
                                });
                                ui.checkbox(&mut d.icon_reverse, tr("cf.reverse"));
                                ui.checkbox(&mut d.icon_show, tr("cf.show_value"));
                            });
                            d.set_points(self);
                            let set = ICON_SETS[d.icon_set].0;
                            let n = icon_count(set);
                            egui::Grid::new("cf_points").num_columns(3).show(ui, |ui| {
                                for k in (0..n).rev() {
                                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                                    let icon = if d.icon_reverse { n - 1 - k } else { k };
                                    paint_icon(ui.painter(), r, set, icon as u8);
                                    if k > 0 {
                                        ui.label(tr("cf.when_ge"));
                                        d.icon_points[k - 1].ui(ui, ("pt", k), false);
                                    } else {
                                        ui.label(tr("cf.otherwise"));
                                    }
                                    ui.end_row();
                                }
                            });
                        }
                    }
                }
                1 => {
                    ui.horizontal(|ui| {
                        ui.label(tr("cf.format_only"));
                        let names = ["cf.cell_value", "cf.specific_text", "cf.dates", "cf.blanks", "cf.no_blanks", "cf.errors", "cf.no_errors"];
                        egui::ComboBox::from_id_salt("cf_contains").selected_text(tr(names[d.contains])).show_ui(ui, |ui| {
                            for (i, key) in names.iter().enumerate() {
                                ui.selectable_value(&mut d.contains, i, tr(key));
                            }
                        });
                        match d.contains {
                            0 => {
                                egui::ComboBox::from_id_salt("cf_op").selected_text(tr(cmp_key(d.op))).show_ui(ui, |ui| {
                                    for op in CmpOp::ALL {
                                        ui.selectable_value(&mut d.op, op, tr(cmp_key(op)));
                                    }
                                });
                            }
                            1 => {
                                egui::ComboBox::from_id_salt("cf_textop").selected_text(tr(TEXT_OPS.iter().find(|x| x.0 == d.text_op).unwrap().1)).show_ui(ui, |ui| {
                                    for (op, key) in TEXT_OPS {
                                        ui.selectable_value(&mut d.text_op, op, tr(key));
                                    }
                                });
                            }
                            2 => {
                                egui::ComboBox::from_id_salt("cf_period").selected_text(tr(period_key(d.period))).show_ui(ui, |ui| {
                                    for (p, key) in PERIODS {
                                        ui.selectable_value(&mut d.period, p, tr(key));
                                    }
                                });
                            }
                            _ => {}
                        }
                    });
                    match d.contains {
                        0 => {
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut d.a).desired_width(150.0));
                                if d.op.two_args() {
                                    ui.label(tr("cf.and"));
                                    ui.add(egui::TextEdit::singleline(&mut d.b).desired_width(150.0));
                                }
                            });
                        }
                        1 => {
                            ui.add(egui::TextEdit::singleline(&mut d.text).desired_width(250.0));
                        }
                        _ => {}
                    }
                }
                2 => {
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("cf_top").selected_text(tr(if d.bottom { "filter.bottom" } else { "filter.top" })).show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.bottom, false, tr("filter.top"));
                            ui.selectable_value(&mut d.bottom, true, tr("filter.bottom"));
                        });
                        ui.add(egui::DragValue::new(&mut d.rank).range(1..=1000));
                        ui.checkbox(&mut d.percent, tr("cf.percent_of_range"));
                    });
                }
                3 => {
                    let names = ["cf.avg_above", "cf.avg_below", "cf.avg_above_eq", "cf.avg_below_eq"];
                    ui.horizontal(|ui| {
                        ui.label(tr("cf.format_values"));
                        egui::ComboBox::from_id_salt("cf_avg").selected_text(tr(names[d.average])).show_ui(ui, |ui| {
                            for (i, key) in names.iter().enumerate() {
                                ui.selectable_value(&mut d.average, i, tr(key));
                            }
                        });
                        ui.label(tr("cf.avg_of_range"));
                    });
                }
                4 => {
                    ui.horizontal(|ui| {
                        ui.label(tr("cf.format_all"));
                        egui::ComboBox::from_id_salt("cf_dup").selected_text(tr(if d.dup { "cf.duplicate" } else { "cf.unique" })).show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.dup, true, tr("cf.duplicate"));
                            ui.selectable_value(&mut d.dup, false, tr("cf.unique"));
                        });
                        ui.label(tr("cf.in_range"));
                    });
                }
                _ => {
                    ui.label(tr("cf.formula_help"));
                    ui.add(egui::TextEdit::singleline(&mut d.formula).hint_text("=$B2>100").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                }
            }
        }
        if d.tab != 0 {
            widgets::section(ui, tr("cf.sec_format"));
            style_editor(ui, &mut d.style);
        }
        widgets::section(ui, tr("cf.applies_to"));
        ui.add(egui::TextEdit::singleline(&mut d.ranges).desired_width(260.0));
        if d.tab != 0 {
            ui.checkbox(&mut d.stop, tr("cf.stop"));
        }
        if let Some(e) = &d.error {
            ui.colored_label(Color32::from_rgb(0xD0, 0x30, 0x30), e);
        }
    }

    /// Human description of a rule.
    fn describe(&self, rule: &CfRule) -> String {
        match &rule.kind {
            CfKind::CellIs { op, f1, f2 } => {
                let (a, b) = (self.wb.rule_display(f1), self.wb.rule_display(f2));
                let v = if op.two_args() { trf("cf.desc_two", &[("a", &a), ("b", &b)]) } else { a };
                format!("{}: {} {}", tr("cf.cell_value"), tr(cmp_key(*op)).to_lowercase(), v)
            }
            CfKind::Text { op, text } => format!("{} \"{text}\"", tr(TEXT_OPS.iter().find(|x| x.0 == *op).unwrap().1)),
            CfKind::Period(p) => format!("{}: {}", tr("cf.dates"), tr(period_key(*p))),
            CfKind::Blanks(true) => tr("cf.blanks").into(),
            CfKind::Blanks(false) => tr("cf.no_blanks").into(),
            CfKind::Errors(true) => tr("cf.errors").into(),
            CfKind::Errors(false) => tr("cf.no_errors").into(),
            CfKind::Duplicates(true) => tr("cf.duplicate").into(),
            CfKind::Duplicates(false) => tr("cf.unique").into(),
            CfKind::Top { bottom, percent, rank } => format!("{} {rank}{}", tr(if *bottom { "filter.bottom" } else { "filter.top" }), if *percent { "%" } else { "" }),
            CfKind::Average { below, equal } => tr(match (below, equal) {
                (false, false) => "cf.avg_above",
                (true, false) => "cf.avg_below",
                (false, true) => "cf.avg_above_eq",
                (true, true) => "cf.avg_below_eq",
            })
            .into(),
            CfKind::Expression(f) => format!("{}: ={}", tr("cf.formula"), self.formula_display(f)),
            CfKind::ColorScale(s) => tr(if s.len() == 3 { "cf.scale3" } else { "cf.scale2" }).into(),
            CfKind::DataBar { .. } => tr("cf.data_bar").into(),
            CfKind::IconSet { set, .. } => format!("{} ({set})", tr("cf.icon_set")),
        }
    }

    fn rules_ui(&self, ui: &mut egui::Ui, m: &mut RulesDlg) -> Option<DataDlg> {
        let mut next = None;
        let sel = self.sel();
        widgets::section(ui, tr("cf.show_for").trim_end_matches(':'));
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("cf_scope").width(200.0).selected_text(tr(if m.only_sel { "cf.scope_sel" } else { "cf.scope_sheet" })).show_ui(ui, |ui| {
                ui.selectable_value(&mut m.only_sel, true, tr("cf.scope_sel"));
                ui.selectable_value(&mut m.only_sel, false, tr("cf.scope_sheet"));
            });
        });
        widgets::section(ui, tr("cf.sec_rules"));
        ui.horizontal(|ui| {
            if ui.button(format!("{}…", tr("cf.new_rule"))).clicked() {
                let mut d = RuleDlg::new(self.ranges_text(&[sel]));
                d.back = Some(m.clone());
                next = Some(DataDlg::Cf(CfDlg::Rule(Box::new(d))));
            }
            let has = m.selected.is_some_and(|i| i < m.list.len());
            if ui.add_enabled(has, egui::Button::new(format!("{}…", tr("cf.edit_rule")))).clicked() {
                let i = m.selected.unwrap();
                next = Some(DataDlg::Cf(CfDlg::Rule(Box::new(self.rule_dlg_from(&m.list[i], Some(i), Some(m.clone()))))));
            }
            if ui.add_enabled(has, egui::Button::new(tr("cf.delete_rule"))).clicked() {
                let i = m.selected.unwrap();
                m.list.remove(i);
                m.selected = if m.list.is_empty() { None } else { Some(i.min(m.list.len() - 1)) };
            }
            let up = has && m.selected != Some(0);
            if ui.add_enabled_ui(up, |ui| widgets::tool_button(ui, widgets::Face::Icon(Icon::ChevronUp), false, tr("cf.move_up"))).inner.clicked() {
                let i = m.selected.unwrap();
                m.list.swap(i, i - 1);
                m.selected = Some(i - 1);
            }
            let down = has && m.selected.is_some_and(|i| i + 1 < m.list.len());
            if ui.add_enabled_ui(down, |ui| widgets::tool_button(ui, widgets::Face::Icon(Icon::ChevronDown), false, tr("cf.move_down"))).inner.clicked() {
                let i = m.selected.unwrap();
                m.list.swap(i, i + 1);
                m.selected = Some(i + 1);
            }
        });
        ui.add_space(4.0);
        egui::Frame::new().stroke(Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)).inner_margin(4).show(ui, |ui| {
            ui.set_min_height(200.0);
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                egui::Grid::new("cf_rules_grid").num_columns(4).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
                    ui.strong(tr("cf.col_rule"));
                    ui.strong(tr("cf.col_format"));
                    ui.strong(tr("cf.applies_to"));
                    ui.strong(tr("cf.stop"));
                    ui.end_row();
                    let mut stop_toggle = None;
                    for (i, rule) in m.list.iter().enumerate() {
                        if m.only_sel && !rule.ranges.iter().any(|r| r.intersect(&sel).is_some()) {
                            continue;
                        }
                        let text: String = self.describe(rule).chars().take(60).collect();
                        if ui.add(egui::Button::selectable(m.selected == Some(i), text).min_size(vec2(240.0, 0.0))).clicked() {
                            m.selected = Some(i);
                        }
                        if rule.kind.is_graphic() {
                            let (r, _) = ui.allocate_exact_size(vec2(120.0, 20.0), Sense::hover());
                            match &rule.kind {
                                CfKind::DataBar { color, .. } => paint_bar(ui.painter(), Rect::from_min_size(r.min + vec2(2.0, 3.0), vec2(80.0, 14.0)), theme::rgb(*color)),
                                CfKind::ColorScale(stops) => {
                                    let w = r.width() / stops.len() as f32;
                                    for (k, (_, c)) in stops.iter().enumerate() {
                                        ui.painter().rect_filled(Rect::from_min_size(r.min + vec2(w * k as f32, 2.0), vec2(w, 16.0)), 0.0, theme::rgb(*c));
                                    }
                                }
                                CfKind::IconSet { set, .. } => {
                                    let n = icon_count(set);
                                    for k in 0..n {
                                        paint_icon(ui.painter(), Rect::from_min_size(r.min + vec2(20.0 * k as f32, 2.0), vec2(16.0, 16.0)), set, (n - 1 - k) as u8);
                                    }
                                }
                                _ => {}
                            }
                        } else {
                            style_preview(ui, &rule.style, vec2(120.0, 20.0));
                        }
                        ui.label(self.ranges_text(&rule.ranges));
                        let mut stop = rule.stop;
                        if !rule.kind.is_graphic() && ui.checkbox(&mut stop, "").changed() {
                            stop_toggle = Some((i, stop));
                        } else if rule.kind.is_graphic() {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                    if let Some((i, s)) = stop_toggle {
                        m.list[i].stop = s;
                    }
                });
            });
        });
        ui.label(RichText::new(tr("cf.priority_help")).small().weak());
        next
    }
}

fn lerp(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xFF) as f32, ((b >> s) & 0xFF) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

fn quick_title(k: QuickKind) -> &'static str {
    match k {
        QuickKind::Greater => "cf.greater",
        QuickKind::Less => "cf.less",
        QuickKind::Between => "cf.between",
        QuickKind::Equal => "cf.equal",
        QuickKind::Text => "cf.text",
        QuickKind::Date => "cf.date",
        QuickKind::Duplicates => "cf.duplicates",
        QuickKind::Top { bottom: false, percent: false } => "cf.top10",
        QuickKind::Top { bottom: false, percent: true } => "cf.top10p",
        QuickKind::Top { bottom: true, percent: false } => "cf.bottom10",
        QuickKind::Top { bottom: true, percent: true } => "cf.bottom10p",
        QuickKind::Average { below: false } => "cf.above_avg",
        QuickKind::Average { below: true } => "cf.below_avg",
    }
}

fn quick_ui(ui: &mut egui::Ui, q: &mut QuickDlg) {
    let help = match q.kind {
        QuickKind::Greater => "cf.q_greater",
        QuickKind::Less => "cf.q_less",
        QuickKind::Between => "cf.q_between",
        QuickKind::Equal => "cf.q_equal",
        QuickKind::Text => "cf.q_text",
        QuickKind::Date => "cf.q_date",
        QuickKind::Duplicates => "cf.q_dups",
        QuickKind::Top { bottom: false, .. } => "cf.q_top",
        QuickKind::Top { bottom: true, .. } => "cf.q_bottom",
        QuickKind::Average { below: false } => "cf.q_above",
        QuickKind::Average { below: true } => "cf.q_below",
    };
    widgets::section(ui, tr(help).trim_end_matches(':'));
    ui.horizontal(|ui| {
        match q.kind {
            QuickKind::Between => {
                ui.add(egui::TextEdit::singleline(&mut q.a).desired_width(90.0));
                ui.label(tr("cf.and"));
                ui.add(egui::TextEdit::singleline(&mut q.b).desired_width(90.0));
            }
            QuickKind::Greater | QuickKind::Less | QuickKind::Equal | QuickKind::Text => {
                let r = ui.add(egui::TextEdit::singleline(&mut q.a).desired_width(150.0));
                if !ui.memory(|m| m.focused().is_some()) {
                    r.request_focus();
                }
            }
            QuickKind::Date => {
                egui::ComboBox::from_id_salt("q_period").selected_text(tr(period_key(q.period))).show_ui(ui, |ui| {
                    for (p, key) in PERIODS {
                        ui.selectable_value(&mut q.period, p, tr(key));
                    }
                });
            }
            QuickKind::Duplicates => {
                egui::ComboBox::from_id_salt("q_dup").selected_text(tr(if q.dup { "cf.duplicate" } else { "cf.unique" })).show_ui(ui, |ui| {
                    ui.selectable_value(&mut q.dup, true, tr("cf.duplicate"));
                    ui.selectable_value(&mut q.dup, false, tr("cf.unique"));
                });
            }
            QuickKind::Top { percent, .. } => {
                ui.add(egui::DragValue::new(&mut q.n).range(1..=1000).suffix(if percent { "%" } else { "" }));
            }
            QuickKind::Average { .. } => {
                ui.label(RichText::new(tr("cf.avg_of_range")).weak());
            }
        }
    });
    widgets::section(ui, tr("cf.sec_format"));
    widgets::form(ui, "q_format", |ui| {
        widgets::form_row(ui, tr("cf.format_with"), |ui| {
            egui::ComboBox::from_id_salt("q_preset").width(240.0).selected_text(tr(PRESETS[q.preset])).show_ui(ui, |ui| {
                for (i, key) in PRESETS.iter().enumerate() {
                    ui.selectable_value(&mut q.preset, i, tr(key));
                }
            });
        });
        widgets::form_row(ui, tr("cf.preview").trim_end_matches(':'), |ui| style_preview(ui, &preset(q.preset), vec2(240.0, 26.0)));
    });
}

/// A color swatch that opens the color palette.
fn color_button(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, c: &mut u32) {
    let (r, resp) = ui.allocate_exact_size(vec2(36.0, 18.0), Sense::click());
    ui.painter().rect_filled(r, 2.0, theme::rgb(*c));
    ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, Color32::GRAY), StrokeKind::Inside);
    egui::Popup::menu(&resp).id(Id::new(("cf_color", id))).show(|ui| {
        if let Some(Some(v)) = widgets::color_menu(ui, tr("color.automatic"), Some(*c)) {
            *c = v;
            ui.close();
        }
    });
}

/// Font color, fill and font style of a rule.
fn style_editor(ui: &mut egui::Ui, st: &mut CfStyle) {
    widgets::form(ui, "cf_style", |ui| {
        widgets::form_row(ui, tr("cf.preview").trim_end_matches(':'), |ui| {
            style_preview(ui, st, vec2(200.0, 26.0));
            // Off with no format to clear.
            if ui.add_enabled(!st.is_empty(), egui::Button::new(tr("cf.clear_format")).small()).clicked() {
                *st = CfStyle::default();
            }
        });
        widgets::form_row(ui, tr("cf.colors"), |ui| {
            crate::widgets::menu_button(ui, tr("cf.font_color"), |ui| {
                if let Some(c) = widgets::color_menu(ui, tr("color.automatic"), st.color) {
                    st.color = c;
                    ui.close();
                }
            });
            crate::widgets::menu_button(ui, tr("cf.fill"), |ui| {
                if let Some(c) = widgets::color_menu(ui, tr("color.none"), st.fill) {
                    st.fill = c;
                    ui.close();
                }
            });
        });
        widgets::form_row(ui, tr("cf.font_style"), |ui| {
            for (v, key) in [(&mut st.bold, "format.bold"), (&mut st.italic, "format.italic"), (&mut st.underline, "format.underline"), (&mut st.strike, "format.strike")] {
                let mut on = *v == Some(true);
                if ui.checkbox(&mut on, tr(key)).changed() {
                    *v = on.then_some(true);
                }
            }
        });
    });
}
