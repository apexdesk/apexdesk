//! Chart templates (.crtx): a chart part alone in a package, as Excel
//! saves them with Save as Template.

use apex_sheet::{Chart, Workbook};

use crate::Error;
use crate::zip::{ZipReader, ZipWriter};

const HEADER: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;

/// The template of `chart` (its data references are kept, as Excel does).
pub fn write(wb: &Workbook, sheet: usize, chart: &Chart) -> Vec<u8> {
    let mut z = ZipWriter::new();
    let ct = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/chart/chart.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/></Types>"#;
    z.add("[Content_Types].xml", format!("{HEADER}{ct}").as_bytes(), true);
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="chart/chart.xml"/></Relationships>"#;
    z.add("_rels/.rels", format!("{HEADER}{rels}").as_bytes(), true);
    z.add("chart/chart.xml", crate::xlsx_chart::write_chart(wb, sheet, chart).as_bytes(), true);
    z.finish()
}

/// The chart of a template.
pub fn read(bytes: &[u8]) -> Result<Chart, Error> {
    let z = ZipReader::new(bytes)?;
    // The chart part is named by the package relationships.
    let target = z
        .read_string("_rels/.rels")
        .ok()
        .and_then(|r| crate::xlsx_notes::read_rels(&r).into_values().find(|(t, _, _)| t.ends_with("/chart")).map(|(_, target, _)| target.trim_start_matches('/').to_string()))
        .unwrap_or_else(|| "chart/chart.xml".into());
    let xml = z.read_string(&target)?;
    crate::xlsx_chart::read_chart(&xml).ok_or_else(|| Error::new("not a chart template"))
}

#[cfg(test)]
mod tests {
    use apex_sheet::chart::{ChartType, LegendPos, Series};
    use apex_sheet::{CellRef, Range};

    use super::*;

    #[test]
    fn template_roundtrip() {
        let mut wb = Workbook::new();
        wb.set_inputs(0, vec![(CellRef::new(0, 1), "Sales".into()), (CellRef::new(1, 0), "A".into()), (CellRef::new(1, 1), "3".into()), (CellRef::new(2, 0), "B".into()), (CellRef::new(2, 1), "5".into())]);
        let mut c = wb.chart_from_range(0, Range::parse("A1:B3").unwrap(), ChartType::Line, None);
        c.markers = true;
        c.legend = LegendPos::Top;
        c.series[0].color = Some(0x00AA00);
        let bytes = write(&wb, 0, &c);
        if let Some(out) = std::env::var_os("APEX_CRTX_OUT") {
            apex_fs::write(out, &bytes).unwrap();
        }
        let t = read(&bytes).unwrap();
        assert_eq!((t.kind, t.markers, t.legend), (ChartType::Line, true, LegendPos::Top));
        assert_eq!(t.series[0].color, Some(0x00AA00));
        let mut other = Chart::new(ChartType::Column, CellRef::new(0, 4), 300.0, 200.0);
        other.series.push(Series::default());
        other.apply_template(&t);
        assert_eq!(other.kind, ChartType::Line);
        assert!(read(b"not a zip").is_err());
    }
}
