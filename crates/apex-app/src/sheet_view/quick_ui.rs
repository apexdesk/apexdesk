//! Quick Analysis: the button shown at the corner of a selected block of
//! data, with formatting, charts, totals, tables and sparklines for it.

use apex_sheet::QuickTotal;
use apex_sheet::chart::{ChartType, DataRef, Grouping, SparkGroup, SparkKind, Sparkline};
use apex_sheet::data::{CfKind, CfRule, Cfvo};

use super::cf_ui::{BARS, QuickKind, SCALES, default_points, paint_bar, paint_icon, preset};
use super::*;
use crate::icons::Icon;
use crate::widgets::Face;

#[derive(Default)]
pub(super) struct QuickState {
    tab: usize,
    /// Where the button was drawn last.
    pub button: Option<Rect>,
    /// Esc hid the button for this selection.
    hidden_for: Option<(usize, Range)>,
    /// Open the panel on the next frame (from the context menu).
    pub open: bool,
    /// The panel was showing last frame.
    shown: bool,
}

const TABS: [&str; 5] = ["quick.formatting", "quick.charts", "quick.totals", "quick.tables", "quick.sparklines"];

#[derive(Clone, Copy)]
enum Action {
    Bars,
    Scale,
    Icons,
    Greater,
    Top10,
    ClearFormat,
    Chart(ChartType, Grouping, u8),
    Total(QuickTotal, bool),
    Table,
    Pivot,
    Spark(SparkKind),
}

/// What a tile shows above its name.
#[derive(Clone, Copy)]
enum Look {
    Icon(Icon),
    Bars,
    Scale,
    Arrows,
    /// Cells with some highlighted: all but the first (`false`) or the
    /// top one (`true`).
    Marked(bool),
}

/// A square button with a picture and its name below.
fn tile(ui: &mut egui::Ui, look: Look, label: &str) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(76.0, 64.0), Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::Button, label, None);
    let v = ui.visuals();
    if resp.hovered() {
        ui.painter().rect_filled(r, 4.0, v.widgets.hovered.weak_bg_fill);
    }
    let ir = Rect::from_center_size(pos2(r.center().x, r.min.y + 20.0), vec2(26.0, 26.0));
    let p = ui.painter();
    let rows = |k: usize| Rect::from_min_size(pos2(ir.min.x, ir.min.y + 1.0 + k as f32 * 8.5), vec2(ir.width(), 7.0));
    match look {
        Look::Icon(icon) => crate::icons::paint_accent(p, ir, icon, v.text_color(), theme::ACCENT),
        Look::Bars => {
            for (k, f) in [0.9, 0.35, 0.6].iter().enumerate() {
                let row = rows(k);
                paint_bar(p, Rect::from_min_size(row.min, vec2(row.width() * f, row.height())), theme::rgb(BARS[0]));
            }
        }
        Look::Scale => {
            for (k, c) in SCALES[0].iter().enumerate() {
                p.rect_filled(rows(k), 1.0, theme::rgb(*c));
            }
        }
        Look::Arrows => {
            for k in 0..3 {
                let row = rows(k);
                paint_icon(p, Rect::from_min_size(row.min, vec2(8.0, 8.0)), "3Arrows", 2 - k as u8);
                p.rect_filled(Rect::from_min_max(pos2(row.min.x + 11.0, row.center().y - 1.0), pos2(row.max.x, row.center().y + 1.0)), 0.0, v.text_color().gamma_multiply(0.5));
            }
        }
        Look::Marked(top) => {
            for k in 0..3 {
                let hit = if top { k == 0 } else { k > 0 };
                let (fill, stroke) = if hit { (theme::rgb(0xFFC7CE), theme::rgb(0x9C0006)) } else { (Color32::TRANSPARENT, v.text_color().gamma_multiply(0.5)) };
                let row = rows(k);
                p.rect(row, 1.0, fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
            }
        }
    }
    let font = egui::FontId::proportional(11.5);
    let galley = p.layout(label.to_string(), font, v.text_color(), r.width() - 4.0);
    p.galley(pos2(r.center().x - galley.size().x / 2.0, r.min.y + 38.0), galley, v.text_color());
    resp.clicked()
}

impl SheetView {
    /// Whether the button belongs on the selection now.
    fn quick_wanted(&self, ui: &egui::Ui) -> bool {
        let sel = self.sel();
        if sel.is_single() || self.edit.is_some() || self.drag != Drag::None || self.fill_opts.is_some() || self.paste_live() || self.chart_selected() || self.image_selected() || self.areas().len() > 1 {
            return false;
        }
        if !self.quick.shown && ui.input(|i| i.pointer.primary_down()) && self.quick.button.is_none_or(|b| !ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| b.contains(p))) {
            return false;
        }
        if self.quick.hidden_for == Some((self.sheet, sel)) {
            return false;
        }
        let Some(used) = self.wb.clamp_to_used(self.sheet, &sel) else { return false };
        if used.cell_count() > 2_000_000 {
            return true;
        }
        let mut any = false;
        self.sh().for_each_in(&used, |_, c| any |= !c.value().is_empty());
        any
    }

    pub(super) fn quick_button(&mut self, ui: &mut egui::Ui, g: &Geo) {
        if !self.quick_wanted(ui) {
            self.quick.button = None;
            self.quick.open = false;
            self.quick.shown = false;
            return;
        }
        let sel = self.sel();
        let r = g.range_rect(self.sh(), &sel);
        let size = vec2(28.0, 22.0);
        let min = pos2((r.max.x + 6.0).min(g.view.max.x - size.x - 4.0), (r.max.y + 6.0).min(g.view.max.y - size.y - 4.0));
        let at = Rect::from_min_size(min, size);
        if !g.view.contains(at.min) || !g.view.intersects(r) {
            self.quick.button = None;
            return;
        }
        self.quick.button = Some(at);
        let mut pick = None;
        let mut tab = self.quick.tab;
        let mut popup_open = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
            ui.painter().rect(at, 3.0, ui.visuals().window_fill, ui.visuals().window_stroke, egui::StrokeKind::Inside);
            let resp = widgets::tool_button(ui, Face::Icon(Icon::QuickAnalysis), false, tr("quick.title"));
            let id = egui::Popup::default_response_id(&resp);
            if std::mem::take(&mut self.quick.open) {
                egui::Popup::open_id(ui.ctx(), id);
            }
            popup_open = egui::Popup::is_id_open(ui.ctx(), id);
            egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.horizontal(|ui| {
                    for (i, key) in TABS.iter().enumerate() {
                        if ui.selectable_label(tab == i, tr(key)).clicked() {
                            tab = i;
                        }
                    }
                });
                ui.separator();
                pick = quick_tiles(ui, tab);
                if pick.is_some() {
                    ui.close();
                }
                ui.add_space(2.0);
                ui.label(RichText::new(tr(TIPS[tab])).small().weak());
            });
        });
        self.quick.tab = tab;
        self.quick.shown = popup_open && pick.is_none();
        if !popup_open && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.quick.hidden_for = Some((self.sheet, sel));
        }
        if let Some(a) = pick {
            self.quick_action(a, sel);
        }
    }

    fn quick_action(&mut self, a: Action, sel: Range) {
        self.settle_edit();
        match a {
            Action::Bars => self.graphic_rule(CfKind::DataBar { color: BARS[0], min: Cfvo::Min, max: Cfvo::Max }),
            Action::Scale => {
                let c = SCALES[0];
                self.graphic_rule(CfKind::ColorScale(vec![(Cfvo::Min, c[2]), (Cfvo::Percentile(50.0), c[1]), (Cfvo::Max, c[0])]));
            }
            Action::Icons => self.graphic_rule(CfKind::IconSet { set: "3Arrows".into(), reverse: false, show_value: true, points: default_points(3) }),
            Action::Greater => self.quick(QuickKind::Greater),
            Action::Top10 => self.add_rule(CfRule { ranges: vec![sel], kind: CfKind::Top { bottom: false, percent: true, rank: 10 }, style: preset(0), stop: false }),
            Action::ClearFormat => self.clear_rules(false),
            Action::Chart(kind, grouping, flags) => self.insert_chart(kind, grouping, flags, false),
            Action::Total(kind, below) => {
                let label = match kind {
                    QuickTotal::Sum => tr("quick.sum"),
                    QuickTotal::Average => tr("quick.average"),
                    QuickTotal::Count => tr("quick.count"),
                    QuickTotal::Percent => tr("quick.percent"),
                    QuickTotal::Running => tr("quick.running"),
                };
                let r = self.wb.clamp_to_used(self.sheet, &sel).map_or(sel, |u| Range::new(sel.start, u.end));
                if !self.wb.quick_totals(self.sheet, &r, kind, below, label) {
                    self.notices.push(Notice::info(tr("quick.no_room")));
                }
            }
            Action::Table => {
                let r = self.wb.clamp_to_used(self.sheet, &sel).map_or(sel, |u| Range::new(sel.start, u.end));
                let header = self.guess_header(&r);
                if let Err(e) = self.wb.create_table(self.sheet, r, header, apex_sheet::table::DEFAULT_STYLE) {
                    self.notices.push(Notice::error(super::tables_ui::error_text(e)));
                }
            }
            Action::Pivot => self.open_pivot_create(None),
            Action::Spark(kind) => self.quick_sparklines(kind, sel),
        }
    }

    /// A sparkline of each row with numbers, in the column right of it.
    fn quick_sparklines(&mut self, kind: SparkKind, sel: Range) {
        let r = self.wb.clamp_to_used(self.sheet, &sel).map_or(sel, |u| Range::new(sel.start, u.end));
        if r.end.col + 1 >= apex_sheet::MAX_COLS {
            return;
        }
        let sh = self.sh();
        let name = apex_sheet::chart::quote_sheet(&sh.name);
        let sparklines: Vec<Sparkline> = (r.start.row..=r.end.row)
            .filter(|&row| (r.start.col..=r.end.col).any(|col| matches!(sh.value(CellRef::new(row, col)), Value::Number(_))))
            .map(|row| {
                let data = Range::new(CellRef::new(row, r.start.col), CellRef::new(row, r.end.col));
                Sparkline { cell: CellRef::new(row, r.end.col + 1), data: DataRef::parse(&format!("{name}!{}", data.to_a1())) }
            })
            .collect();
        if sparklines.is_empty() {
            self.notices.push(Notice::info(tr("chart.no_data")));
            return;
        }
        self.wb.add_sparklines(self.sheet, SparkGroup { kind, sparklines, ..Default::default() });
    }
}

const TIPS: [&str; 5] = ["quick.formatting_tip", "quick.charts_tip", "quick.totals_tip", "quick.tables_tip", "quick.sparklines_tip"];

fn quick_tiles(ui: &mut egui::Ui, tab: usize) -> Option<Action> {
    let mut pick = None;
    let mut row = |ui: &mut egui::Ui, items: &[(Look, &'static str, Action)]| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (look, key, a) in items {
                if tile(ui, *look, tr(key)) {
                    pick = Some(*a);
                }
            }
        });
    };
    match tab {
        0 => row(
            ui,
            &[
                (Look::Bars, "cf.data_bars", Action::Bars),
                (Look::Scale, "cf.color_scales", Action::Scale),
                (Look::Arrows, "cf.icon_sets", Action::Icons),
                (Look::Marked(false), "quick.greater", Action::Greater),
                (Look::Marked(true), "quick.top10", Action::Top10),
                (Look::Icon(Icon::Eraser), "quick.clear_format", Action::ClearFormat),
            ],
        ),
        1 => row(
            ui,
            &[
                (Look::Icon(Icon::ChartColumn), "chart.t.column", Action::Chart(ChartType::Column, Grouping::Clustered, 0)),
                (Look::Icon(Icon::ChartBar), "chart.t.bar", Action::Chart(ChartType::Bar, Grouping::Clustered, 0)),
                (Look::Icon(Icon::ChartLine), "chart.t.line", Action::Chart(ChartType::Line, Grouping::Clustered, 0)),
                (Look::Icon(Icon::ChartPie), "chart.t.pie", Action::Chart(ChartType::Pie, Grouping::Clustered, 0)),
                (Look::Icon(Icon::ChartScatter), "chart.t.scatter", Action::Chart(ChartType::Scatter, Grouping::Clustered, 0)),
                (Look::Icon(Icon::ChartColumn), "chart.t.column_stacked", Action::Chart(ChartType::Column, Grouping::Stacked, 0)),
            ],
        ),
        2 => {
            for (below, key) in [(true, "quick.below"), (false, "quick.right")] {
                ui.label(RichText::new(tr(key)).small().strong());
                row(
                    ui,
                    &[
                        (Look::Icon(Icon::Sigma), "quick.sum", Action::Total(QuickTotal::Sum, below)),
                        (Look::Icon(Icon::Sigma), "quick.average", Action::Total(QuickTotal::Average, below)),
                        (Look::Icon(Icon::Sigma), "quick.count", Action::Total(QuickTotal::Count, below)),
                        (Look::Icon(Icon::Percent), "quick.percent", Action::Total(QuickTotal::Percent, below)),
                        (Look::Icon(Icon::Sigma), "quick.running", Action::Total(QuickTotal::Running, below)),
                    ],
                );
            }
        }
        3 => row(ui, &[(Look::Icon(Icon::Table), "quick.table", Action::Table), (Look::Icon(Icon::Pivot), "quick.pivot", Action::Pivot)]),
        _ => row(
            ui,
            &[
                (Look::Icon(Icon::Sparkline), "spark.line", Action::Spark(SparkKind::Line)),
                (Look::Icon(Icon::Sparkline), "spark.column", Action::Spark(SparkKind::Column)),
                (Look::Icon(Icon::Sparkline), "spark.winloss", Action::Spark(SparkKind::WinLoss)),
            ],
        ),
    }
    pick
}
