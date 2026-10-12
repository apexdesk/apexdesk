//! Data > Forecast Sheet: the dialog with a preview of the forecast.

use apex_sheet::analysis::ets::Completion;
use apex_sheet::chart::ChartType;
use apex_sheet::format::NumFormat;
use apex_sheet::workbook::{AnalysisError, ForecastOptions, ForecastSource};

use super::analysis_ui::{AnalysisDlg, error_text, lang};
use super::f10::{F10Dlg, parse_num, tool_window};
use super::*;
use crate::widgets::DialogResult;

pub(super) struct ForecastDlg {
    src: ForecastSource,
    o: ForecastOptions,
    end: String,
    confidence: String,
    season_auto: bool,
    season: String,
    preview: Option<(ForecastOptions, Vec<(f64, f64, Option<(f64, f64)>)>)>,
}

/// Aggregation choices, by the code of the functions (1..=7).
const AGGREGATIONS: [&str; 7] = ["forecast.agg_average", "forecast.agg_count", "forecast.agg_counta", "forecast.agg_max", "forecast.agg_median", "forecast.agg_min", "forecast.agg_sum"];

const HISTORY: Color32 = Color32::from_rgb(0x44, 0x72, 0xC4);
const FORECAST: Color32 = Color32::from_rgb(0xED, 0x7D, 0x31);

impl ForecastDlg {
    /// The options from the text boxes; None and a message when one is wrong.
    fn options(&self, wb: &Workbook) -> Result<ForecastOptions, &'static str> {
        let mut o = self.o.clone();
        o.end = parse_num(wb, &self.end).filter(|e| *e > self.src.last).ok_or(tr("forecast.err_end"))?;
        if o.confidence.is_some() {
            let c = parse_num(wb, self.confidence.trim_end_matches('%')).filter(|c| *c > 0.0 && *c < 100.0).ok_or(tr("forecast.err_confidence"))?;
            o.confidence = Some(c / 100.0);
        }
        o.season = if self.season_auto {
            None
        } else {
            Some(parse_num(wb, &self.season).filter(|s| *s >= 0.0 && *s <= 8760.0 && s.fract() == 0.0).ok_or(tr("forecast.err_season"))? as u32)
        };
        Ok(o)
    }
}

impl SheetView {
    pub(super) fn open_forecast(&mut self) {
        self.settle_edit();
        let mut r = self.sel();
        if r.is_single() {
            r = self.wb.current_region(self.sheet, r.start);
        }
        match self.wb.forecast_source(self.sheet, &r, lang()) {
            Ok(src) => {
                let o = ForecastOptions { end: src.default_end(), ..Default::default() };
                let end = NumFormat::parse(&src.timeline_format).format_number(o.end, &self.wb.locale).0;
                let season = src.season.to_string();
                let d = ForecastDlg { src, o, end, confidence: "95%".into(), season_auto: true, season, preview: None };
                self.open_f10(F10Dlg::Analysis(AnalysisDlg::Forecast(Box::new(d))), None);
            }
            Err(AnalysisError::BadRange) => self.notices.push(Notice::info(tr("forecast.err_data"))),
            Err(e) => self.notices.push(Notice::refused(error_text(&e))),
        }
    }

    pub(super) fn forecast_dialog(&mut self, ctx: &egui::Context, mut d: Box<ForecastDlg>) -> Option<AnalysisDlg> {
        if let Ok(o) = d.options(&self.wb) {
            if d.preview.as_ref().is_none_or(|p| p.0 != o) {
                let pts = d.src.preview(&o);
                d.preview = Some((o, pts));
            }
        }
        let res = tool_window(ctx, "forecast", tr("forecast.title"), 560.0, false, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr("forecast.chart_kind"));
                ui.selectable_value(&mut d.o.kind, ChartType::Line, tr("chart.t.line"));
                ui.selectable_value(&mut d.o.kind, ChartType::Column, tr("chart.t.column"));
            });
            ui.add_space(4.0);
            preview_plot(ui, &d);
            ui.add_space(6.0);
            egui::Grid::new("forecast_grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                ui.label(tr("forecast.end"));
                ui.add(egui::TextEdit::singleline(&mut d.end).desired_width(140.0));
                ui.end_row();
            });
            egui::CollapsingHeader::new(tr("forecast.options")).id_salt("forecast_opts").show(ui, |ui| {
                egui::Grid::new("forecast_opts_grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                    let mut conf = d.o.confidence.is_some();
                    if ui.checkbox(&mut conf, tr("forecast.confidence")).changed() {
                        d.o.confidence = conf.then_some(0.95);
                    }
                    ui.add_enabled(conf, egui::TextEdit::singleline(&mut d.confidence).desired_width(60.0));
                    ui.end_row();
                    ui.label(tr("forecast.seasonality"));
                    ui.vertical(|ui| {
                        let found = if d.src.season > 1 { trf("forecast.auto_n", &[("n", &d.src.season)]) } else { tr("forecast.auto").to_string() };
                        ui.radio_value(&mut d.season_auto, true, found);
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut d.season_auto, false, tr("forecast.manual"));
                            ui.add_enabled(!d.season_auto, egui::TextEdit::singleline(&mut d.season).desired_width(50.0));
                        });
                    });
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut d.o.statistics, tr("forecast.statistics"));
                    ui.end_row();
                    ui.label(tr("forecast.fill_missing"));
                    egui::ComboBox::from_id_salt("forecast_fill")
                        .selected_text(tr(if d.o.completion == Completion::Interpolate { "forecast.interpolate" } else { "forecast.zeros" }))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.o.completion, Completion::Interpolate, tr("forecast.interpolate"));
                            ui.selectable_value(&mut d.o.completion, Completion::Zero, tr("forecast.zeros"));
                        });
                    ui.end_row();
                    ui.label(tr("forecast.aggregate"));
                    egui::ComboBox::from_id_salt("forecast_agg").selected_text(tr(AGGREGATIONS[(d.o.aggregation.clamp(1, 7) - 1) as usize])).show_ui(ui, |ui| {
                        for (i, k) in AGGREGATIONS.iter().enumerate() {
                            ui.selectable_value(&mut d.o.aggregation, i as u8 + 1, tr(k));
                        }
                    });
                    ui.end_row();
                    ui.label(tr("forecast.source"));
                    ui.label(format!("{} · {} ({})", d.src.captions.0, d.src.captions.1, d.src.timeline.len()));
                    ui.end_row();
                });
            });
        });
        match res {
            DialogResult::None => Some(AnalysisDlg::Forecast(d)),
            DialogResult::Cancel => None,
            DialogResult::Ok => match d.options(&self.wb) {
                Err(msg) => {
                    self.notices.push(Notice::error(msg));
                    Some(AnalysisDlg::Forecast(d))
                }
                Ok(o) => match self.wb.forecast_sheet(&d.src, &o, lang()) {
                    Ok(s) => {
                        self.select_sheet(s);
                        None
                    }
                    Err(e) => {
                        self.notices.push(Notice::refused(error_text(&e)));
                        Some(AnalysisDlg::Forecast(d))
                    }
                },
            },
        }
    }
}

/// History and forecast (with its bounds) as lines or columns.
fn preview_plot(ui: &mut egui::Ui, d: &ForecastDlg) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 200.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    p.rect_stroke(rect, 2.0, ui.visuals().widgets.noninteractive.bg_stroke, StrokeKind::Inside);
    let hist: Vec<(f64, f64)> = d.src.timeline.iter().zip(&d.src.values).filter_map(|(t, v)| v.map(|v| (*t, v))).collect();
    let fc: &[(f64, f64, Option<(f64, f64)>)] = d.preview.as_ref().map_or(&[], |p| &p.1);
    if hist.is_empty() {
        return;
    }
    let (mut x0, mut x1, mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    let mut see = |x: f64, y: f64| {
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    };
    for (t, v) in &hist {
        see(*t, *v);
    }
    for (t, v, b) in fc {
        see(*t, *v);
        if let Some((lo, hi)) = b {
            see(*t, *lo);
            see(*t, *hi);
        }
    }
    let columns = d.o.kind == ChartType::Column;
    if columns {
        y0 = y0.min(0.0);
        y1 = y1.max(0.0);
    }
    let pad = ((y1 - y0) * 0.08).max(1e-9);
    let (y0, y1) = (y0 - pad, y1 + pad);
    let x1 = if x1 > x0 { x1 } else { x0 + 1.0 };
    let inner = rect.shrink2(vec2(14.0, 12.0));
    let at = |x: f64, y: f64| pos2(inner.left() + ((x - x0) / (x1 - x0)) as f32 * inner.width(), inner.bottom() - ((y - y0) / (y1 - y0)) as f32 * inner.height());
    // The end of the history.
    let split = at(d.src.last, y0).x;
    p.line_segment([pos2(split, inner.top()), pos2(split, inner.bottom())], Stroke::new(1.0, ui.visuals().weak_text_color()));
    if columns {
        let n = (hist.len() + fc.len().saturating_sub(1)).max(1) as f32;
        let w = (inner.width() / n * 0.7).max(1.0);
        let base = at(x0, 0.0).y;
        for (t, v) in &hist {
            let q = at(*t, *v);
            p.rect_filled(Rect::from_x_y_ranges(q.x - w / 2.0..=q.x + w / 2.0, q.y.min(base)..=q.y.max(base)), 0.0, HISTORY);
        }
        for (t, v, _) in fc.iter().skip(1) {
            let q = at(*t, *v);
            p.rect_filled(Rect::from_x_y_ranges(q.x - w / 2.0..=q.x + w / 2.0, q.y.min(base)..=q.y.max(base)), 0.0, FORECAST);
        }
    } else {
        p.add(egui::Shape::line(hist.iter().map(|(t, v)| at(*t, *v)).collect(), Stroke::new(2.0, HISTORY)));
        p.add(egui::Shape::line(fc.iter().map(|(t, v, _)| at(*t, *v)).collect(), Stroke::new(2.0, FORECAST)));
    }
    for side in 0..2 {
        let pts: Vec<Pos2> = fc.iter().filter_map(|(t, _, b)| b.map(|(lo, hi)| at(*t, if side == 0 { lo } else { hi }))).collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.0, FORECAST.gamma_multiply(0.8))));
    }
}
