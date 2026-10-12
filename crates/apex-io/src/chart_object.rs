//! Charts in documents, each with its own data, as Word keeps them: a
//! small workbook (the data in a sheet, the chart on it) saved as the
//! xlsx that goes with a docx chart. Built from a docx chart part, an odt
//! chart object (its local table), a chart copied from a sheet or the
//! sample data of Insert > Chart.

use std::sync::Arc;

use apex_sheet::{CellRef, Chart, ChartType, Content, DataRef, Range, Value, Workbook};

/// Size of a new chart in sheet pixels (as Excel's).
const SIZE: (f32, f32) = (480.0, 288.0);

/// The workbook of a chart object: the workbook, the sheet holding the
/// chart and the chart's index there.
pub fn open(book: &[u8]) -> Option<(Workbook, usize, usize)> {
    let wb = crate::xlsx::read(book).ok()?;
    let si = wb.sheets.iter().position(|s| !s.charts.is_empty())?;
    Some((wb, si, 0))
}

/// The bytes of a chart object's workbook.
pub fn book(wb: &Workbook) -> Arc<[u8]> {
    Arc::from(crate::xlsx::write(wb))
}

/// Puts `chart` on sheet `sheet` of `wb` as its only chart.
fn put(wb: &mut Workbook, sheet: usize, mut chart: Chart) {
    chart.id = 1;
    if chart.name.is_empty() {
        chart.name = "Chart 1".into();
    }
    chart.sanitize();
    wb.sheets[sheet].charts = vec![chart];
    wb.modified = false;
}

/// A workbook holding a copy of the data `chart` (of sheet `sheet` of
/// `wb`) shows, in a sheet named `name`: the series' names across the
/// first row, a category and a value per series in each row below (the
/// layout of Word's and LibreOffice's chart data), and the chart reading
/// it.
pub fn standalone(wb: &Workbook, sheet: usize, chart: &Chart, name: &str) -> Workbook {
    let data = wb.chart_data(sheet, chart);
    let mut out = Workbook::empty();
    out.sheets.push(apex_sheet::Sheet::new(name));
    let n = data.points().max(1) as u32;
    let xy = chart.kind.is_xy();
    let mut c = chart.clone();
    let cats = Range::new(CellRef::new(1, 0), CellRef::new(n, 0));
    for p in 0..n as usize {
        let v = if xy {
            data.series.first().and_then(|s| s.xs.get(p).copied().flatten()).map_or(Value::Empty, Value::Number)
        } else {
            data.categories.get(p).map_or(Value::Empty, |t| Value::Text(Arc::from(t.as_str())))
        };
        out.load_cell(0, CellRef::new(p as u32 + 1, 0), Content::Value(v), 0);
    }
    let has_cats = !chart.series.iter().all(|s| s.categories.is_empty()) || xy;
    for (i, (s, d)) in c.series.iter_mut().zip(&data.series).enumerate() {
        let col = i as u32 + 1;
        out.load_cell(0, CellRef::new(0, col), Content::Value(Value::Text(Arc::from(d.name.as_str()))), 0);
        for (p, v) in d.values.iter().enumerate().take(n as usize) {
            if let Some(v) = v {
                out.load_cell(0, CellRef::new(p as u32 + 1, col), Content::Value(Value::Number(*v)), 0);
            }
        }
        s.name = DataRef::range(name, Range::single(CellRef::new(0, col)));
        s.values = DataRef::range(name, Range::new(CellRef::new(1, col), CellRef::new(n, col)));
        s.categories = if has_cats { DataRef::range(name, cats) } else { DataRef::default() };
        // Sizes and label texts travel as values of their own.
        s.sizes = DataRef::literal(d.sizes.iter().map(|v| v.map_or(Value::Empty, Value::Number)).collect());
        s.label_range = if d.label_texts.is_empty() { DataRef::default() } else { DataRef::literal(d.label_texts.iter().map(|t| Value::Text(Arc::from(t.as_str()))).collect()) };
    }
    c.series.truncate(data.series.len());
    c.source = Some(DataRef::range(name, Range::new(CellRef::new(0, 0), CellRef::new(n, c.series.len() as u32))));
    c.by_rows = false;
    c.from = CellRef::new(0, c.series.len() as u32 + 2);
    c.dx = 0.0;
    c.dy = 0.0;
    c.anchor = apex_sheet::Anchor::OneCell;
    out.finish_load();
    put(&mut out, 0, c);
    out
}

/// A workbook for a chart read with only the values its references
/// last showed (a docx chart without its workbook, an odt chart's local
/// table): each reference's cells hold those values again.
pub fn from_caches(chart: Chart) -> Workbook {
    let mut wb = Workbook::empty();
    let mut home = None;
    let refs = chart.series.iter().flat_map(|s| [&s.name, &s.categories, &s.values, &s.sizes]);
    for r in refs {
        let Some((sheet, area)) = r.area() else { continue };
        let name = sheet.as_deref().unwrap_or("Sheet1").to_string();
        let si = match wb.sheet_index(&name) {
            Some(i) => i,
            None => {
                wb.sheets.push(apex_sheet::Sheet::new(name.as_str()));
                wb.sheets.len() - 1
            }
        };
        home.get_or_insert(si);
        let cells = (area.start.row..=area.end.row).flat_map(|row| (area.start.col..=area.end.col).map(move |col| CellRef::new(row, col)));
        for (c, v) in cells.zip(&r.cache) {
            if !matches!(v, Value::Empty) {
                wb.load_cell(si, c, Content::Value(v.clone()), 0);
            }
        }
    }
    if wb.sheets.is_empty() {
        wb.sheets.push(apex_sheet::Sheet::new("Sheet1"));
    }
    wb.finish_load();
    let si = home.unwrap_or(0);
    let mut chart = chart;
    let used = wb.sheets[si].extent().map_or(0, |e| e.col + 2);
    chart.from = CellRef::new(0, used);
    put(&mut wb, si, chart);
    wb
}

/// A chart's workbook from a docx chart part and the workbook that went
/// with it (if any); `size` is the chart's in points.
pub fn from_docx(xml: &str, embedded: Option<&[u8]>, size: (f32, f32)) -> Option<Arc<[u8]>> {
    let mut chart = crate::xlsx_chart::read_chart(xml)?;
    (chart.width, chart.height) = (size.0 * 4.0 / 3.0, size.1 * 4.0 / 3.0);
    let wb = match embedded.and_then(|d| crate::xlsx::read(d).ok()) {
        Some(mut wb) if !wb.sheets.is_empty() => {
            let named = chart.series.iter().find_map(|s| s.values.area()).and_then(|(n, _)| n);
            let si = named.and_then(|n| wb.sheet_index(&n)).unwrap_or(0);
            chart.from = CellRef::new(0, wb.sheets[si].extent().map_or(0, |e| e.col + 2));
            put(&mut wb, si, chart);
            wb
        }
        _ => from_caches(chart),
    };
    Some(book(&wb))
}

/// The chart of Insert > Chart: Word's sample data, four categories and
/// three series, named `category` 1... and `series` 1...
pub fn sample(kind: ChartType, category: &str, series: &str) -> Workbook {
    let mut wb = Workbook::new();
    let values = [[4.3, 2.4, 2.0], [2.5, 4.4, 2.0], [3.5, 1.8, 3.0], [4.5, 2.8, 5.0]];
    for s in 0..3u32 {
        wb.load_cell(0, CellRef::new(0, s + 1), Content::Value(Value::Text(Arc::from(format!("{series} {}", s + 1)))), 0);
    }
    for (r, row) in values.iter().enumerate() {
        let r = r as u32 + 1;
        wb.load_cell(0, CellRef::new(r, 0), Content::Value(Value::Text(Arc::from(format!("{category} {r}")))), 0);
        for (s, v) in row.iter().enumerate() {
            wb.load_cell(0, CellRef::new(r, s as u32 + 1), Content::Value(Value::Number(*v)), 0);
        }
    }
    if kind == ChartType::Timeline {
        // Dates in the first column, labels in the second.
        for r in 1..=4u32 {
            wb.load_cell(0, CellRef::new(r, 0), Content::Value(Value::Number(45292.0 + f64::from(r) * 45.0)), 0);
            wb.load_cell(0, CellRef::new(r, 1), Content::Value(Value::Text(Arc::from(format!("{category} {r}")))), 0);
        }
    }
    wb.finish_load();
    let mut chart = wb.chart_from_range(0, Range::new(CellRef::new(0, 0), CellRef::new(4, 3)), kind, Some(false));
    (chart.width, chart.height) = SIZE;
    put(&mut wb, 0, chart);
    wb
}

/// A chart object's chart, if the book holds one: the chart and the data
/// it shows now.
pub fn chart_of(book: &[u8]) -> Option<(Chart, apex_sheet::ChartData, apex_sheet::Locale)> {
    let (wb, si, ci) = open(book)?;
    let c = wb.sheets[si].charts.get(ci)?.clone();
    let data = wb.chart_data(si, &c);
    Some((c, data, wb.locale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_standalone_and_caches() {
        let wb = sample(ChartType::Column, "Category", "Series");
        let (c, data, _) = chart_of(&book(&wb)).unwrap();
        assert_eq!(c.series.len(), 3);
        assert_eq!(data.categories, ["Category 1", "Category 2", "Category 3", "Category 4"]);
        assert_eq!(data.series[2].values[3], Some(5.0));
        assert_eq!(data.series[0].name, "Series 1");

        // A chart copied from a sheet takes its data along.
        let mut src = Workbook::new();
        src.sheets[0].name = "Data".into();
        for (r, (k, v)) in [("", "Sales"), ("Jan", "10"), ("Feb", "20")].iter().enumerate() {
            src.set_input(0, CellRef::new(r as u32 + 3, 2), k);
            src.set_input(0, CellRef::new(r as u32 + 3, 3), v);
        }
        let chart = src.chart_from_range(0, Range::new(CellRef::new(3, 2), CellRef::new(5, 3)), ChartType::Line, None);
        let own = standalone(&src, 0, &chart, "Sheet1");
        let d = own.chart_data(0, &own.sheets[0].charts[0]);
        assert_eq!(d.categories, ["Jan", "Feb"]);
        assert_eq!(d.series[0].values, [Some(10.0), Some(20.0)]);
        assert_eq!(d.series[0].name, "Sales");

        // Only the values shown last: they fill their cells again.
        let mut c = own.sheets[0].charts[0].clone();
        for (s, dd) in c.series.iter_mut().zip(&d.series) {
            s.values.cache = dd.values.iter().map(|v| v.map_or(Value::Empty, Value::Number)).collect();
            s.categories.cache = d.categories.iter().map(|t| Value::Text(Arc::from(t.as_str()))).collect();
            s.name.cache = vec![Value::Text(Arc::from(dd.name.as_str()))];
        }
        let back = from_caches(c);
        let d2 = back.chart_data(0, &back.sheets[0].charts[0]);
        assert_eq!(d2.categories, d.categories);
        assert_eq!(d2.series[0].values, d.series[0].values);
        assert_eq!(d2.series[0].name, "Sales");
    }
}
