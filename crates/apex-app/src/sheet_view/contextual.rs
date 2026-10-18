//! The commands of contextual tabs that aren't showing, for the command
//! search: each tab is drawn off screen on a small sample workbook with a
//! table, a pivot table, a chart, a picture and a shape.

use apex_sheet::analysis::pivot::{Field, PivotSpec, ValueField};
use apex_sheet::chart::{Chart, ChartType};
use apex_sheet::shape::SheetShape;
use apex_sheet::{ImageFormat, SheetImage};

use super::*;
use crate::toolstrip::{self, Entry, TabDef};

impl SheetView {
    /// Contextual tabs other than `showing`, with their commands.
    pub fn contextual_commands(ui: &mut egui::Ui, shell: &mut Shell, showing: &[TabDef]) -> Vec<(TabDef, Vec<Entry>)> {
        let mut wb = Workbook::new();
        let cell = |r, c| CellRef::new(r, c);
        wb.set_inputs(0, vec![(cell(0, 0), "A".into()), (cell(0, 1), "B".into()), (cell(1, 0), "x".into()), (cell(1, 1), "1".into()), (cell(2, 0), "y".into()), (cell(2, 1), "2".into())]);
        let _ = wb.create_table(0, Range::new(cell(0, 0), cell(2, 1)), true, "TableStyleMedium2");
        let spec = PivotSpec { source_sheet: wb.sheets[0].name.clone(), source: Range::new(cell(0, 0), cell(2, 1)), rows: vec![Field { col: 0, ..Default::default() }], values: vec![ValueField { col: 1, ..Default::default() }], ..Default::default() };
        let _ = wb.create_pivot(spec, Some((0, cell(0, 5))));
        let chart = wb.add_chart(0, Chart::new(ChartType::Column, cell(10, 0), 200.0, 120.0));
        let image = wb.add_image(0, SheetImage::new(Arc::from(crate::doc_view::SAMPLE_PNG), ImageFormat::Png, cell(10, 5), 20.0, 20.0));
        let shape = wb.add_shape(0, SheetShape::new(apex_doc::Shape::new(apex_doc::ShapeKind::Rect), cell(20, 0), 40.0, 30.0));
        let _ = wb.insert_plan(0, cell(30, 0), 46000.0, &[apex_sheet::workbook::NewTask::new("T", 0, 1.0, "")]);
        let mut v = SheetView::new(wb);
        let mut out = Vec::new();
        for id in ["table", "plan", "pivot", "chart", "picture", "shape"] {
            if showing.iter().any(|t| t.id == id) {
                continue;
            }
            (v.images.selected, v.images.charts.selected, v.images.shapes.selected) = (None, None, None);
            v.cursor = cell(1, 0);
            match id {
                "pivot" => v.cursor = cell(1, 5),
                "plan" => v.cursor = cell(31, 1),
                "chart" => v.images.charts.selected = Some((0, chart)),
                "picture" => v.images.selected = Some((0, image)),
                "shape" => v.images.shapes.selected = Some((0, shape)),
                _ => {}
            }
            v.anchor = v.cursor;
            let Some(t) = v.toolstrip_tabs().into_iter().find(|t| t.id == id) else { continue };
            let entries = toolstrip::collect(|| toolstrip::within(tr(t.label), || v.toolstrip(ui, id, shell)));
            out.push((t, entries));
        }
        out
    }
}
