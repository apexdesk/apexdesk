//! Data > Forecast Sheet: a new sheet with the history, the FORECAST.ETS
//! predictions and their confidence bounds as formulas, the model
//! statistics and a chart of it all (one undo step).

use super::analysis::AnalysisError;
use super::*;
use crate::analysis::Lang;
use crate::analysis::ets::{Aggregation, Completion, Model, Regular};
use crate::chart::{Chart, ChartType, DataRef, LegendPos};

/// Options of a forecast sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct ForecastOptions {
    /// Last time to forecast (a date serial or a number of the timeline).
    pub end: f64,
    /// Confidence level of the bounds (0.95); None = no bounds.
    pub confidence: Option<f64>,
    /// Season length; None = detected.
    pub season: Option<u32>,
    pub completion: Completion,
    /// 1 average ... 7 sum, as in the functions.
    pub aggregation: u8,
    pub statistics: bool,
    /// Line or column chart.
    pub kind: ChartType,
}

impl Default for ForecastOptions {
    fn default() -> Self {
        ForecastOptions { end: 0.0, confidence: Some(0.95), season: None, completion: Completion::Interpolate, aggregation: 1, statistics: false, kind: ChartType::Line }
    }
}

/// What the dialog shows before creating the sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct ForecastSource {
    pub timeline: Vec<f64>,
    pub values: Vec<Option<f64>>,
    /// Captions of the two columns (from a header row, or defaults).
    pub captions: (String, String),
    pub timeline_format: Arc<str>,
    pub values_format: Arc<str>,
    /// Step of the regular timeline.
    pub step: f64,
    /// The last time of the history.
    pub last: f64,
    /// Season found in the data (0 = none).
    pub season: usize,
    /// The regular timeline (interpolated, averaged).
    pub grid: Regular,
}

impl ForecastSource {
    /// Default end of the forecast: as far again as a third of the history.
    pub fn default_end(&self) -> f64 {
        let len = self.grid.values.len() as f64 - 1.0;
        self.grid.time(len + (len / 3.0).round().max(1.0))
    }

    /// The forecast the sheet will hold, for the dialog's preview: time,
    /// value and bounds, from the last point of the history to `o.end`.
    pub fn preview(&self, o: &ForecastOptions) -> Vec<(f64, f64, Option<(f64, f64)>)> {
        let pts: Vec<(f64, Option<f64>)> = self.timeline.iter().copied().zip(self.values.iter().copied()).collect();
        let agg = Aggregation::from_code(o.aggregation as f64).unwrap_or(Aggregation::Average);
        let Ok(reg) = Regular::new(&pts, o.completion, agg) else { return Vec::new() };
        let Ok(m) = Model::fit(&reg.values, o.season.map(|s| s as usize)) else { return Vec::new() };
        let z = o.confidence.filter(|c| *c > 0.0 && *c < 1.0).map(|c| crate::functions::norm_inv(0.5 + c / 2.0));
        let last = reg.values.last().copied().unwrap_or(0.0);
        let mut out = vec![(reg.end(), last, z.map(|_| (last, last)))];
        let steps = reg.ahead(o.end).ceil().clamp(0.0, 10_000.0) as u32;
        let len = reg.values.len() as f64 - 1.0;
        for k in 1..=steps {
            let t = reg.time(len + k as f64).min(o.end);
            let h = reg.ahead(t);
            let f = m.forecast(h);
            out.push((t, f, z.map(|z| (f - m.interval(h, z), f + m.interval(h, z)))));
        }
        out
    }
}

impl Workbook {
    /// Reads the data of a forecast: `r` is two columns (or two rows),
    /// timeline then values, with an optional header.
    pub fn forecast_source(&self, sheet: usize, r: &Range, lang: Lang) -> Result<ForecastSource, AnalysisError> {
        let sh = self.sheets.get(sheet).ok_or(AnalysisError::NotFound)?;
        let r = if r.cell_count() > 1_000_000 { self.clamp_to_used(sheet, r).ok_or(AnalysisError::NoData)? } else { *r };
        let by_rows = r.cols() != 2 && r.rows() == 2;
        if !by_rows && r.cols() != 2 {
            return Err(AnalysisError::BadRange);
        }
        let len = if by_rows { r.cols() } else { r.rows() };
        let at = |line: u32, i: u32| if by_rows { CellRef::new(r.start.row + line, r.start.col + i) } else { CellRef::new(r.start.row + i, r.start.col + line) };
        let header = !matches!(sh.value(at(1, 0)), Value::Number(_) | Value::Empty);
        let first = header as u32;
        let caption = |line: u32, key: &str| {
            let s = if header { self.display(sheet, at(line, 0)).0 } else { String::new() };
            if s.is_empty() { lang.text(key) } else { s }
        };
        let mut pts = Vec::new();
        for i in first..len {
            let t = match sh.value(at(0, i)) {
                Value::Number(n) if n.is_finite() => *n,
                Value::Empty => continue,
                _ => return Err(AnalysisError::BadRange),
            };
            let v = match sh.value(at(1, i)) {
                Value::Number(n) if n.is_finite() => Some(*n),
                _ => None,
            };
            pts.push((t, v));
        }
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let reg = Regular::new(&pts, Completion::Interpolate, Aggregation::Average).map_err(AnalysisError::Forecast)?;
        let fmt = |line: u32| sh.get(at(line, first)).map_or_else(|| Arc::from("General"), |c| self.styles.get(c.style).num_fmt.clone());
        Ok(ForecastSource {
            captions: (caption(0, "fc_timeline"), caption(1, "values")),
            timeline_format: fmt(0),
            values_format: fmt(1),
            step: reg.step,
            last: reg.end(),
            season: Model::fit(&reg.values, None).map_or(0, |m| if m.period > 1 { m.period } else { 0 }),
            timeline: pts.iter().map(|p| p.0).collect(),
            grid: reg,
            values: pts.iter().map(|p| p.1).collect(),
        })
    }

    /// Creates the forecast sheet after the source sheet. Returns its index.
    pub fn forecast_sheet(&mut self, src: &ForecastSource, o: &ForecastOptions, lang: Lang) -> Result<usize, AnalysisError> {
        let n = src.timeline.len() as u32;
        if n < 3 {
            return Err(AnalysisError::NoData);
        }
        let steps = src.grid.ahead(o.end).ceil();
        if !(steps >= 1.0) {
            return Err(AnalysisError::BadRange);
        }
        let steps = steps.min((MAX_ROWS - n - 2) as f64).min(100_000.0) as u32;
        let name = self.unique_sheet_name(&lang.text("sheet_forecast"));
        let mut sh = Sheet::new(name.clone());
        let tfmt = self.style_with_format(0, &src.timeline_format);
        let vfmt = self.style_with_format(0, &src.values_format);
        let head = self.styles.intern(CellStyle { bold: true, ..CellStyle::default() });
        let put = |sh: &mut Sheet, row: u32, col: u32, content: Content, style: u32| sh.put(CellRef::new(row, col), Some(Cell::new(content, style)));
        let formula = |s: String| Content::Formula { expr: (crate::formula::parse(&s, &crate::format::Locale::EN).expect("forecast formula")).into(), value: Value::Empty };
        let vals = &src.captions.1;
        let bounds = o.confidence.is_some();
        let mut caps = vec![src.captions.0.clone(), vals.clone(), lang.text("fc_forecast").replace("{0}", vals)];
        if bounds {
            caps.push(lang.text("fc_lower").replace("{0}", vals));
            caps.push(lang.text("fc_upper").replace("{0}", vals));
        }
        for (c, t) in caps.iter().enumerate() {
            put(&mut sh, 0, c as u32, Content::Value(Value::text(t.as_str())), head);
        }
        for (i, (t, v)) in src.timeline.iter().zip(&src.values).enumerate() {
            let row = i as u32 + 1;
            put(&mut sh, row, 0, Content::Value(Value::Number(*t)), tfmt);
            if let Some(v) = v {
                put(&mut sh, row, 1, Content::Value(Value::Number(*v)), vfmt);
            }
        }
        // The forecast starts at the last point so that the lines join.
        let last = n + 1;
        for c in 2..caps.len() as u32 {
            put(&mut sh, n, c, formula(format!("=B{last}")), vfmt);
        }
        let hist = format!("$B$2:$B${last},$A$2:$A${last}");
        let season = o.season.map_or(1, |s| s);
        let tail = format!("{season},{},{}", (o.completion == Completion::Interpolate) as u8, o.aggregation);
        for k in 1..=steps {
            let row = n + k;
            let r1 = row + 1;
            let t = src.grid.time((src.grid.values.len() - 1) as f64 + k as f64).min(o.end);
            put(&mut sh, row, 0, Content::Value(Value::Number(t)), tfmt);
            put(&mut sh, row, 2, formula(format!("=FORECAST.ETS(A{r1},{hist},{tail})")), vfmt);
            if let Some(conf) = o.confidence {
                let ci = format!("FORECAST.ETS.CONFINT(A{r1},{hist},{conf},{tail})");
                put(&mut sh, row, 3, formula(format!("=C{r1}-{ci}")), vfmt);
                put(&mut sh, row, 4, formula(format!("=C{r1}+{ci}")), vfmt);
            }
        }
        let width = crate::sheet::DEFAULT_COL_WIDTH;
        sh.col_widths.insert(0, width * 1.4);
        // Wide enough for the captions (the default width holds about 9).
        for (c, cap) in caps.iter().enumerate().skip(1) {
            sh.col_widths.insert(c as u32, width * (cap.chars().count() as f32 / 8.0).max(1.4));
        }
        let stats_col = caps.len() as u32 + 1;
        if o.statistics {
            put(&mut sh, 0, stats_col, Content::Value(Value::text(lang.text("fc_statistic"))), head);
            put(&mut sh, 0, stats_col + 1, Content::Value(Value::text(lang.text("fc_value"))), head);
            let three = self.style_with_format(0, "0.000");
            for (i, label) in ["Alpha", "Beta", "Gamma", "MASE", "SMAPE", "MAE", "RMSE"].iter().enumerate() {
                let row = i as u32 + 1;
                put(&mut sh, row, stats_col, Content::Value(Value::text(*label)), 0);
                put(&mut sh, row, stats_col + 1, formula(format!("=FORECAST.ETS.STAT({hist},{},{tail})", i + 1)), three);
            }
            sh.col_widths.insert(stats_col, width * 1.2);
        }
        let index = self.sheets.len();
        let before = self.undo.len();
        self.execute_as(Op::AddSheet { index, sheet: Box::new(sh) }, "undo.forecast");
        // The chart: history, forecast and bounds over the timeline.
        let q = crate::chart::quote_sheet(&name);
        let end = n + steps + 1;
        let col_ref = |c: u32, from: u32| DataRef::parse(&format!("{q}!${}${from}:${}${end}", crate::addr::col_name(c), crate::addr::col_name(c)));
        let chart_col = if o.statistics { stats_col + 3 } else { caps.len() as u32 + 1 };
        let mut chart = Chart::new(o.kind, CellRef::new(1, chart_col), 560.0, 320.0);
        for c in 1..caps.len() as u32 {
            let mut s = crate::chart::Series { name: DataRef::parse(&format!("{q}!${}$1", crate::addr::col_name(c))), categories: col_ref(0, 2), values: col_ref(c, 2), ..Default::default() };
            if c >= 3 {
                s.kind = Some(ChartType::Line);
                s.line_width = Some(1.0);
            }
            chart.series.push(s);
        }
        chart.legend = LegendPos::Bottom;
        self.add_chart(index, chart);
        let steps = self.undo.len().saturating_sub(before);
        self.merge_undo(steps, "undo.forecast");
        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Workbook {
        let mut w = Workbook::new();
        w.set_input(0, CellRef::new(0, 0), "Month");
        w.set_input(0, CellRef::new(0, 1), "Sales");
        for i in 0..24u32 {
            let season = [0.0, 4.0, 8.0, 3.0][i as usize % 4];
            let v = 100.0 + 2.0 * i as f64 + season + if i % 3 == 0 { 0.5 } else { -0.3 };
            w.set_input(0, CellRef::new(i + 1, 0), &(i + 1).to_string());
            w.set_input(0, CellRef::new(i + 1, 1), &v.to_string());
        }
        w
    }

    #[test]
    fn source_reads_columns() {
        let w = book();
        let r = Range::new(CellRef::new(0, 0), CellRef::new(24, 1));
        let s = w.forecast_source(0, &r, Lang::En).unwrap();
        assert_eq!(s.captions, ("Month".to_string(), "Sales".to_string()));
        assert_eq!(s.timeline.len(), 24);
        assert_eq!((s.step, s.last, s.season), (1.0, 24.0, 4));
        assert_eq!(s.default_end(), 32.0);
        let bad = Range::new(CellRef::new(0, 0), CellRef::new(24, 0));
        assert_eq!(w.forecast_source(0, &bad, Lang::En), Err(AnalysisError::BadRange));
    }

    #[test]
    fn sheet_has_formulas_and_chart() {
        let mut w = book();
        let r = Range::new(CellRef::new(0, 0), CellRef::new(24, 1));
        let s = w.forecast_source(0, &r, Lang::En).unwrap();
        let o = ForecastOptions { end: 30.0, statistics: true, ..Default::default() };
        let i = w.forecast_sheet(&s, &o, Lang::En).unwrap();
        let sh = &w.sheets[i];
        assert_eq!(sh.name, "Forecast1");
        assert_eq!(w.display(i, CellRef::new(0, 2)).0, "Forecast(Sales)");
        // The last point joins the lines.
        assert_eq!(sh.value(CellRef::new(24, 2)), sh.value(CellRef::new(24, 1)));
        assert_eq!(sh.value(CellRef::new(30, 0)), &Value::Number(30.0));
        let Value::Number(f) = *sh.value(CellRef::new(25, 2)) else { panic!("{:?}", sh.value(CellRef::new(25, 2))) };
        // Month 25 is in the first season slot: about 100 + 2*24 = 148.
        assert!((f - 148.0).abs() < 3.0, "{f}");
        let (Value::Number(lo), Value::Number(hi)) = (sh.value(CellRef::new(25, 3)).clone(), sh.value(CellRef::new(25, 4)).clone()) else { panic!() };
        assert!(lo < f && f < hi);
        assert!(matches!(sh.value(CellRef::new(1, 7)), Value::Number(a) if (0.0..=1.0).contains(a)));
        assert_eq!(sh.charts.len(), 1);
        assert_eq!(sh.charts[0].series.len(), 4);
        // One undo step removes it all.
        w.undo();
        assert_eq!(w.sheets.len(), 1);
    }

    #[test]
    fn monthly_dates() {
        let mut w = Workbook::new();
        w.locale = crate::format::Locale::EN;
        for i in 0..30i64 {
            let d = crate::format::date_to_serial(2024, i + 1, 31).min(crate::format::date_to_serial(2024, i + 2, 1) - 1.0);
            w.set_input(0, CellRef::new(i as u32, 0), &d.to_string());
            w.set_input(0, CellRef::new(i as u32, 1), &(100.0 + i as f64 * 2.0 + [0.0, 3.0, -2.0][i as usize % 3]).to_string());
        }
        let r = Range::new(CellRef::new(0, 0), CellRef::new(29, 1));
        let s = w.forecast_source(0, &r, Lang::En).unwrap();
        assert_eq!(s.grid.months, Some(0));
        // Ten months ahead of June 2026: the last day of April 2027.
        assert_eq!(s.default_end(), crate::format::date_to_serial(2027, 5, 1) - 1.0);
        let i = w.forecast_sheet(&s, &ForecastOptions { end: s.default_end(), ..Default::default() }, Lang::En).unwrap();
        assert_eq!(w.sheets[i].value(CellRef::new(31, 0)), &Value::Number(crate::format::date_to_serial(2026, 8, 1) - 1.0));
        let pv = s.preview(&ForecastOptions { end: s.default_end(), ..Default::default() });
        assert_eq!(pv.len(), 11);
        assert!(pv.iter().skip(1).all(|p| matches!(p.2, Some((lo, hi)) if lo < p.1 && p.1 < hi)), "{pv:?}");
        assert!(matches!(w.sheets[i].value(CellRef::new(31, 2)), Value::Number(v) if (v - 162.0).abs() < 4.0), "{:?}", w.sheets[i].value(CellRef::new(31, 2)));
    }

    #[test]
    fn end_before_last_is_rejected() {
        let mut w = book();
        let r = Range::new(CellRef::new(0, 0), CellRef::new(24, 1));
        let s = w.forecast_source(0, &r, Lang::En).unwrap();
        let o = ForecastOptions { end: 20.0, ..Default::default() };
        assert_eq!(w.forecast_sheet(&s, &o, Lang::En), Err(AnalysisError::BadRange));
    }
}
