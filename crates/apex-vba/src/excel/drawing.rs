//! Worksheet.Shapes and ChartObjects: shapes, text boxes, pictures and
//! charts over the cells; a chart's type, data, title, axes, legend and
//! series. Sizes are points to the macro, sheet pixels (96 dpi) inside.

use std::rc::Rc;
use std::sync::Arc;

use apex_doc::{Shape, ShapeKind};
use apex_sheet::chart::{Axis, Chart, ChartType, DataRef, Grouping, LegendPos, Series, scale_for};
use apex_sheet::shape::SheetShape;
use apex_sheet::{CellRef, Op, Range};

use super::{Args, Interp, obj, range_obj, swap_rb};
use crate::error::{Error, R};
use crate::object::{Obj, Part};
use crate::value::{Array, Value};

/// What a Shape is: one of the sheet's shapes, charts or pictures.
pub const SHAPE: u8 = 0;
pub const CHART: u8 = 1;
pub const PICTURE: u8 = 2;

/// Shapes, charts and pictures a macro may leave on one sheet.
const MAX_OBJECTS: usize = 10_000;

/// Pixels per point.
const PX: f64 = 96.0 / 72.0;

/// MsoAutoShapeType of the shapes there are.
const AUTO_SHAPES: [(i64, ShapeKind); 14] = [
    (1, ShapeKind::Rect),
    (5, ShapeKind::RoundRect),
    (9, ShapeKind::Ellipse),
    (7, ShapeKind::Triangle),
    (8, ShapeKind::RightTriangle),
    (4, ShapeKind::Diamond),
    (12, ShapeKind::Pentagon),
    (10, ShapeKind::Hexagon),
    (33, ShapeKind::RightArrow),
    (34, ShapeKind::LeftArrow),
    (35, ShapeKind::UpArrow),
    (36, ShapeKind::DownArrow),
    (92, ShapeKind::Star5),
    (105, ShapeKind::Callout),
];

/// XlChartType: the type, its grouping, then markers, lines between
/// points, smooth lines, 3-D and filled radar.
type Look = (ChartType, Grouping, bool, bool, bool, bool, bool);

const CHART_TYPES: [(i64, Look); 44] = {
    use ChartType as T;
    use Grouping::{Clustered as C, Percent as P, Stacked as S};
    [
        (51, (T::Column, C, false, false, false, false, false)),
        (52, (T::Column, S, false, false, false, false, false)),
        (53, (T::Column, P, false, false, false, false, false)),
        (54, (T::Column, C, false, false, false, true, false)),
        (55, (T::Column, S, false, false, false, true, false)),
        (56, (T::Column, P, false, false, false, true, false)),
        (-4100, (T::Column, C, false, false, false, true, false)),
        (57, (T::Bar, C, false, false, false, false, false)),
        (58, (T::Bar, S, false, false, false, false, false)),
        (59, (T::Bar, P, false, false, false, false, false)),
        (60, (T::Bar, C, false, false, false, true, false)),
        (61, (T::Bar, S, false, false, false, true, false)),
        (62, (T::Bar, P, false, false, false, true, false)),
        (4, (T::Line, C, false, false, false, false, false)),
        (63, (T::Line, S, false, false, false, false, false)),
        (64, (T::Line, P, false, false, false, false, false)),
        (65, (T::Line, C, true, false, false, false, false)),
        (66, (T::Line, S, true, false, false, false, false)),
        (67, (T::Line, P, true, false, false, false, false)),
        (-4101, (T::Line, C, false, false, false, true, false)),
        (5, (T::Pie, C, false, false, false, false, false)),
        (-4102, (T::Pie, C, false, false, false, true, false)),
        (-4120, (T::Doughnut, C, false, false, false, false, false)),
        (1, (T::Area, C, false, false, false, false, false)),
        (76, (T::Area, S, false, false, false, false, false)),
        (77, (T::Area, P, false, false, false, false, false)),
        (-4098, (T::Area, C, false, false, false, true, false)),
        (-4169, (T::Scatter, C, true, false, false, false, false)),
        (74, (T::Scatter, C, true, true, false, false, false)),
        (75, (T::Scatter, C, false, true, false, false, false)),
        (72, (T::Scatter, C, true, true, true, false, false)),
        (73, (T::Scatter, C, false, true, true, false, false)),
        (-4151, (T::Radar, C, false, false, false, false, false)),
        (81, (T::Radar, C, true, false, false, false, false)),
        (82, (T::Radar, C, false, false, false, false, true)),
        (15, (T::Bubble, C, false, false, false, false, false)),
        (88, (T::Stock, C, false, false, false, false, false)),
        (89, (T::Stock, C, false, false, false, false, false)),
        (83, (T::Surface, C, false, false, false, true, false)),
        (85, (T::Surface, C, false, false, false, false, false)),
        (117, (T::Treemap, C, false, false, false, false, false)),
        (118, (T::Histogram, C, false, false, false, false, false)),
        (119, (T::Waterfall, C, false, false, false, false, false)),
        (120, (T::Sunburst, C, false, false, false, false, false)),
    ]
};

/// The chart types Excel has that are not in the table above.
const MORE_TYPES: [(i64, ChartType); 3] = [(121, ChartType::BoxWhisker), (122, ChartType::Pareto), (123, ChartType::Funnel)];

fn part(p: Part) -> Value {
    obj(Obj::Part(Box::new(p)))
}

fn deleted() -> Error {
    Error::app("The object has been deleted")
}

fn pt(px: f32) -> Value {
    Value::Double((f64::from(px) / PX * 100.0).round() / 100.0)
}

/// Points to pixels, refusing what no sheet holds.
fn px(v: &Value) -> R<f32> {
    let p = v.to_f64()?;
    if !p.is_finite() || p.abs() > 1.0e6 {
        return Err(Error::invalid_call());
    }
    Ok((p * PX) as f32)
}

fn mso_bool(b: bool) -> Value {
    Value::Long(if b { -1 } else { 0 })
}

/// msoTrue, True and other non-zero values are true.
fn mso_true(v: &Value) -> R<bool> {
    match v {
        Value::Bool(b) => Ok(*b),
        v => Ok(v.to_i64()? != 0),
    }
}

/// VBA's line breaks in a shape's text: vbCr, vbLf or vbCrLf.
fn shape_text(t: &str) -> String {
    t.replace("\r\n", "\n").replace('\r', "\n")
}

fn chart_type_code(c: &Chart) -> i64 {
    let look: Look = (c.kind, c.grouping, c.markers, c.lines, c.smooth, c.view3d, c.filled);
    if let Some((code, _)) = CHART_TYPES.iter().find(|(_, l)| *l == look) {
        return *code;
    }
    if let Some((code, _)) = MORE_TYPES.iter().find(|(_, k)| *k == c.kind) {
        return *code;
    }
    CHART_TYPES.iter().find(|(_, l)| l.0 == c.kind && l.1 == c.grouping).or_else(|| CHART_TYPES.iter().find(|(_, l)| l.0 == c.kind)).map_or(51, |x| x.0)
}

fn set_chart_type(c: &mut Chart, code: i64) -> R<()> {
    if let Some((_, (kind, grouping, markers, lines, smooth, view3d, filled))) = CHART_TYPES.iter().find(|(k, _)| *k == code) {
        c.set_kind(*kind);
        c.grouping = *grouping;
        c.markers = *markers;
        c.lines = *lines;
        c.smooth = *smooth;
        c.view3d = *view3d && kind.has_3d();
        c.filled = *filled;
        return Ok(());
    }
    match MORE_TYPES.iter().find(|(k, _)| *k == code) {
        Some((_, kind)) => {
            c.set_kind(*kind);
            Ok(())
        }
        None => Err(Error::unsupported(&format!("chart type {code}"))),
    }
}

impl Interp<'_> {
    // --- finding and changing the objects -----------------------------------------

    fn chart_of(&self, s: usize, id: u32) -> R<Chart> {
        self.wb.sheets.get(s).and_then(|sh| sh.charts.iter().find(|c| c.id == id)).cloned().ok_or_else(deleted)
    }

    fn edit_chart(&mut self, s: usize, id: u32, f: impl FnOnce(&mut Chart) -> R<()>) -> R<()> {
        let mut c = self.chart_of(s, id)?;
        f(&mut c)?;
        self.via(|wb| wb.set_chart(s, c))
    }

    fn shape_of(&self, s: usize, id: u32) -> R<SheetShape> {
        self.wb.sheets.get(s).and_then(|sh| sh.shapes.iter().find(|c| c.id == id)).cloned().ok_or_else(deleted)
    }

    fn edit_shape(&mut self, s: usize, id: u32, f: impl FnOnce(&mut SheetShape) -> R<()>) -> R<()> {
        let mut x = self.shape_of(s, id)?;
        f(&mut x)?;
        self.via(|wb| wb.set_shape(s, x))
    }

    /// Every shape of a sheet, bottom to top: pictures, shapes, charts.
    fn shapes_in(&self, s: usize) -> Vec<(u8, u32)> {
        let Some(sh) = self.wb.sheets.get(s) else { return Vec::new() };
        let pictures = sh.images.iter().map(|x| (PICTURE, x.id));
        let shapes = sh.shapes.iter().map(|x| (SHAPE, x.id));
        pictures.chain(shapes).chain(sh.charts.iter().map(|x| (CHART, x.id))).collect()
    }

    fn shape_name(&self, s: usize, kind: u8, id: u32) -> R<String> {
        let sh = self.wb.sheets.get(s).ok_or_else(deleted)?;
        let name = match kind {
            SHAPE => sh.shapes.iter().find(|x| x.id == id).map(|x| x.shape.name.clone()),
            CHART => sh.charts.iter().find(|x| x.id == id).map(|x| x.name.clone()),
            _ => sh.images.iter().find(|x| x.id == id).map(|x| x.name.clone()),
        };
        name.ok_or_else(deleted)
    }

    /// x, y, width and height in sheet pixels.
    fn shape_rect(&self, s: usize, kind: u8, id: u32) -> R<[f32; 4]> {
        let sh = self.wb.sheets.get(s).ok_or_else(deleted)?;
        let r = match kind {
            SHAPE => sh.shapes.iter().find(|x| x.id == id).map(|x| x.rect(sh)),
            CHART => sh.charts.iter().find(|x| x.id == id).map(|x| x.rect(sh)),
            _ => sh.images.iter().find(|x| x.id == id).map(|x| x.rect(sh)),
        };
        r.ok_or_else(deleted)
    }

    fn set_shape_rect(&mut self, s: usize, kind: u8, id: u32, f: impl FnOnce(&mut [f32; 4])) -> R<()> {
        let mut r = self.shape_rect(s, kind, id)?;
        f(&mut r);
        let [x, y, w, h] = r;
        if w < 0.0 || h < 0.0 {
            return Err(Error::invalid_call());
        }
        match kind {
            SHAPE => {
                let mut v = self.shape_of(s, id)?;
                v.set_pos(&self.wb.sheets[s], x, y);
                (v.width, v.height) = (w, h);
                self.via(|wb| wb.set_shape(s, v))
            }
            CHART => {
                let mut v = self.chart_of(s, id)?;
                v.set_pos(&self.wb.sheets[s], x, y);
                (v.width, v.height) = (w.max(8.0), h.max(8.0));
                self.via(|wb| wb.set_chart(s, v))
            }
            _ => {
                let mut list = self.wb.sheets[s].images.clone();
                let i = list.iter().position(|x| x.id == id).ok_or_else(deleted)?;
                list[i].set_pos(&self.wb.sheets[s], x, y);
                (list[i].width, list[i].height) = (w, h);
                list[i].sanitize();
                self.apply(Op::Images { sheet: s, list })
            }
        }
    }

    fn rename_shape(&mut self, s: usize, kind: u8, id: u32, name: String) -> R<()> {
        if name.is_empty() || name.chars().count() > 255 {
            return Err(Error::app("Unable to set the Name property"));
        }
        match kind {
            SHAPE => self.edit_shape(s, id, |x| {
                x.shape.name = name;
                Ok(())
            }),
            CHART => self.edit_chart(s, id, |x| {
                x.name = name;
                Ok(())
            }),
            _ => {
                let mut list = self.wb.sheets[s].images.clone();
                let i = list.iter().position(|x| x.id == id).ok_or_else(deleted)?;
                list[i].name = name;
                self.apply(Op::Images { sheet: s, list })
            }
        }
    }

    fn delete_shape(&mut self, s: usize, kind: u8, id: u32) -> R<()> {
        self.shape_rect(s, kind, id)?;
        match kind {
            SHAPE => self.via(|wb| wb.remove_shape(s, id)),
            CHART => {
                if self.active_chart == Some((s, id)) {
                    self.active_chart = None;
                }
                self.via(|wb| wb.remove_chart(s, id))
            }
            _ => {
                let mut list = self.wb.sheets[s].images.clone();
                list.retain(|x| x.id != id);
                self.apply(Op::Images { sheet: s, list })
            }
        }
    }

    /// An item of Shapes or ChartObjects by number (1-based) or name.
    fn pick(&mut self, items: Vec<(u8, u32)>, s: usize, a: &Args) -> R<(u8, u32)> {
        let v = a.get(0, "index").ok_or_else(|| Error::new(449, "Argument not optional"))?;
        let v = self.scalar(v)?;
        if let Value::Str(name) = &v {
            for (k, id) in items {
                if self.shape_name(s, k, id)?.eq_ignore_ascii_case(name) {
                    return Ok((k, id));
                }
            }
            return Err(Error::app("The item with the specified name wasn't found"));
        }
        let i = v.to_i64()?;
        if i < 1 || i as usize > items.len() {
            return Err(Error::app("The index into the specified collection is out of bounds"));
        }
        Ok(items[i as usize - 1])
    }

    // --- Shapes ---------------------------------------------------------------------

    pub(super) fn shapes_member(&mut self, s: usize, name: &str, a: &Args) -> R<Value> {
        self.check_sheet(s)?;
        Ok(match name {
            "count" => Value::Long(self.shapes_in(s).len() as i32),
            "item" | "_default" | "range" => {
                let items = self.shapes_in(s);
                let (k, id) = self.pick(items, s, a)?;
                part(Part::Shape(s, k, id))
            }
            "addshape" => {
                let ty = self.arg_i64(a, 0, "type")?.ok_or_else(|| Error::new(449, "Argument not optional"))?;
                let kind = AUTO_SHAPES.iter().find(|(k, _)| *k == ty).map(|x| x.1).ok_or_else(|| Error::unsupported(&format!("AddShape type {ty}")))?;
                let r = self.box_args(a, 1, ["left", "top", "width", "height"])?;
                self.add_shape(s, Shape::new(kind), r)?
            }
            "addtextbox" => {
                let r = self.box_args(a, 1, ["left", "top", "width", "height"])?;
                self.add_shape(s, Shape::new(ShapeKind::TextBox), r)?
            }
            "addlabel" => {
                let r = self.box_args(a, 1, ["left", "top", "width", "height"])?;
                let mut sh = Shape::new(ShapeKind::TextBox);
                (sh.fill, sh.line) = (None, None);
                self.add_shape(s, sh, r)?
            }
            "addline" | "addconnector" => {
                let first = usize::from(name == "addconnector");
                let [x1, y1, x2, y2] = self.box_args(a, first, ["beginx", "beginy", "endx", "endy"])?;
                let mut sh = Shape::new(ShapeKind::Line);
                // The box runs from the top-left; flips pick the diagonal.
                sh.flip_h = x2 < x1;
                sh.flip_v = y2 < y1;
                self.add_shape(s, sh, [x1.min(x2), y1.min(y2), (x2 - x1).abs(), (y2 - y1).abs()])?
            }
            "addchart" | "addchart2" => {
                // AddChart(Type, Left, Top, Width, Height); AddChart2 has
                // Style first.
                let o = usize::from(name == "addchart2");
                let ty = match self.arg_i64(a, o, "xlcharttype")? {
                    Some(t) => Some(t),
                    None => self.arg_i64(a, o, "type")?,
                };
                let at = self.optional_box(a, o + 1)?;
                let id = self.new_chart(s, ty.filter(|t| *t != -1), at, true)?;
                part(Part::Shape(s, CHART, id))
            }
            "addpicture" | "addpicture2" | "addoleobject" | "addformcontrol" | "addsmartart" | "addpolyline" | "addcurve" | "addtexteffect" => return Err(Error::unsupported(&format!("Shapes.{name}"))),
            "selectall" => Value::Empty,
            "parent" => obj(Obj::Sheet(s)),
            _ => return Err(Error::no_member(name)),
        })
    }

    /// Four required numbers in points from `first`, as pixels.
    fn box_args(&mut self, a: &Args, first: usize, names: [&str; 4]) -> R<[f32; 4]> {
        let mut out = [0.0; 4];
        for (i, n) in names.iter().enumerate() {
            let v = a.get(first + i, n).ok_or_else(|| Error::new(449, "Argument not optional"))?;
            out[i] = px(&self.scalar(v)?)?;
        }
        Ok(out)
    }

    /// Left, Top, Width, Height that may be left out.
    fn optional_box(&mut self, a: &Args, first: usize) -> R<[Option<f32>; 4]> {
        let mut out = [None; 4];
        for (i, n) in ["left", "top", "width", "height"].iter().enumerate() {
            if let Some(v) = a.get(first + i, n) {
                out[i] = Some(px(&self.scalar(v)?)?);
            }
        }
        Ok(out)
    }

    fn room_for_one(&self, s: usize) -> R<()> {
        if self.shapes_in(s).len() >= MAX_OBJECTS {
            return Err(Error::app("Too many shapes on the sheet"));
        }
        Ok(())
    }

    fn add_shape(&mut self, s: usize, shape: Shape, [x, y, w, h]: [f32; 4]) -> R<Value> {
        self.room_for_one(s)?;
        let mut v = SheetShape::new(shape, CellRef::new(0, 0), w, h);
        v.set_pos(&self.wb.sheets[s], x, y);
        let id = self.via(|wb| wb.add_shape(s, v))?;
        Ok(part(Part::Shape(s, SHAPE, id)))
    }

    /// A new chart of type `ty` (XlChartType); with `selection` it plots
    /// the selected data, as Excel's Insert Chart does.
    fn new_chart(&mut self, s: usize, ty: Option<i64>, at: [Option<f32>; 4], selection: bool) -> R<u32> {
        self.room_for_one(s)?;
        let sel = self.selection;
        let source = if selection && s == self.active {
            let rg = if sel.is_single() { self.current_region(s, sel) } else { sel };
            let has_data = self.wb.clamp_to_used(s, &rg).is_some_and(|u| !u.is_single() || self.wb.cell(s, u.start).is_some());
            has_data.then_some(rg)
        } else {
            None
        };
        let mut c = match source {
            Some(rg) if rg.cell_count() <= 1_000_000 => self.wb.chart_from_range(s, rg, ChartType::Column, None),
            _ => Chart::new(ChartType::Column, CellRef::new(0, 0), 480.0, 288.0),
        };
        if let Some(t) = ty {
            set_chart_type(&mut c, t)?;
        }
        let sh = &self.wb.sheets[s];
        let (x0, y0) = c.pos(sh);
        let x = at[0].unwrap_or(x0);
        let y = at[1].unwrap_or(y0);
        c.set_pos(sh, x, y);
        c.width = at[2].unwrap_or(c.width);
        c.height = at[3].unwrap_or(c.height);
        let id = self.via(|wb| wb.add_chart(s, c))?;
        self.active_chart = Some((s, id));
        Ok(id)
    }

    // --- Shape ----------------------------------------------------------------------

    pub(super) fn shape_member(&mut self, s: usize, kind: u8, id: u32, name: &str, a: &Args) -> R<Value> {
        let [x, y, w, h] = self.shape_rect(s, kind, id)?;
        let shape = if kind == SHAPE { Some(self.shape_of(s, id)?) } else { None };
        Ok(match name {
            "name" => Value::str(self.shape_name(s, kind, id)?),
            "left" => pt(x),
            "top" => pt(y),
            "width" => pt(w),
            "height" => pt(h),
            "id" => Value::Long(id as i32),
            "type" => Value::Long(match (kind, &shape) {
                (CHART, _) => 3,
                (PICTURE, _) => 13,
                (_, Some(v)) if v.control.is_some() => 8,
                (_, Some(v)) if v.shape.kind == ShapeKind::Line => 9,
                (_, Some(v)) if v.shape.is_text_box() => 17,
                _ => 1,
            }),
            "autoshapetype" => {
                let k = shape.as_ref().map(|v| v.shape.kind);
                Value::Long(AUTO_SHAPES.iter().find(|(_, x)| Some(*x) == k).map_or(-2, |x| x.0 as i32))
            }
            "haschart" => mso_bool(kind == CHART),
            "chart" if kind == CHART => part(Part::Chart(s, id)),
            "visible" => mso_bool(!shape.as_ref().is_some_and(|v| v.shape.hidden)),
            "rotation" => Value::Double(shape.as_ref().map_or(0.0, |v| f64::from(v.shape.rotation))),
            "onaction" => Value::str(shape.as_ref().map_or("", |v| v.macro_name.as_str())),
            "alternativetext" => Value::str(match kind {
                SHAPE => shape.as_ref().map(|v| v.shape.alt.clone()).unwrap_or_default(),
                CHART => self.chart_of(s, id)?.alt,
                _ => self.wb.sheets[s].images.iter().find(|x| x.id == id).map(|x| x.alt.clone()).unwrap_or_default(),
            }),
            "topleftcell" | "bottomrightcell" => {
                let sh = &self.wb.sheets[s];
                let (px_, py) = if name == "topleftcell" { (x, y) } else { ((x + w - 0.5).max(0.0), (y + h - 0.5).max(0.0)) };
                let c = CellRef::new(sh.row_at(py).min(apex_sheet::MAX_ROWS - 1), sh.col_at(px_).min(apex_sheet::MAX_COLS - 1));
                range_obj(s, Range::single(c))
            }
            "textframe" | "textframe2" if kind == SHAPE => part(Part::ShapeText(s, id, false)),
            "fill" if kind == SHAPE => part(Part::ShapeFill(s, id, false)),
            "line" if kind == SHAPE => part(Part::ShapeFill(s, id, true)),
            "delete" | "cut" => {
                self.delete_shape(s, kind, id)?;
                Value::Empty
            }
            "select" | "activate" => {
                if kind == CHART {
                    self.active_chart = Some((s, id));
                }
                Value::Empty
            }
            "incrementleft" | "incrementtop" => {
                let d = px(&self.scalar(a.get(0, "increment").ok_or_else(|| Error::new(449, "Argument not optional"))?)?)?;
                let left = name == "incrementleft";
                self.set_shape_rect(s, kind, id, |r| if left { r[0] = (r[0] + d).max(0.0) } else { r[1] = (r[1] + d).max(0.0) })?;
                Value::Empty
            }
            "incrementrotation" if kind == SHAPE => {
                let d = self.arg_f64(a, 0, "increment")?;
                self.edit_shape(s, id, |v| {
                    v.shape.rotation += d as f32;
                    Ok(())
                })?;
                Value::Empty
            }
            "zorder" if kind == SHAPE => {
                let to = match self.arg_i64(a, 0, "zordercmd")?.ok_or_else(|| Error::new(449, "Argument not optional"))? {
                    0 => apex_sheet::shape::Order::Front,
                    1 => apex_sheet::shape::Order::Back,
                    2 => apex_sheet::shape::Order::Forward,
                    3 => apex_sheet::shape::Order::Backward,
                    _ => return Err(Error::invalid_call()),
                };
                self.via(|wb| wb.reorder_shape(s, id, to))?;
                Value::Empty
            }
            "parent" => obj(Obj::Sheet(s)),
            _ => return Err(Error::no_member(name)),
        })
    }

    fn arg_f64(&mut self, a: &Args, pos: usize, name: &str) -> R<f64> {
        let v = a.get(pos, name).ok_or_else(|| Error::new(449, "Argument not optional"))?;
        let x = self.scalar(v)?.to_f64()?;
        if !x.is_finite() {
            return Err(Error::invalid_call());
        }
        Ok(x)
    }

    pub(super) fn shape_set(&mut self, s: usize, kind: u8, id: u32, name: &str, v: Value) -> R<()> {
        match name {
            "name" => self.rename_shape(s, kind, id, v.to_str()?),
            "left" | "top" | "width" | "height" => {
                let p = px(&v)?;
                let i = ["left", "top", "width", "height"].iter().position(|n| *n == name).unwrap_or(0);
                self.set_shape_rect(s, kind, id, |r| r[i] = if i < 2 { p.max(0.0) } else { p })
            }
            "visible" if kind == SHAPE => {
                let b = mso_true(&v)?;
                self.edit_shape(s, id, |x| {
                    x.shape.hidden = !b;
                    Ok(())
                })
            }
            "rotation" if kind == SHAPE => {
                let r = v.to_f64()?;
                if !r.is_finite() {
                    return Err(Error::invalid_call());
                }
                self.edit_shape(s, id, |x| {
                    x.shape.rotation = r as f32;
                    Ok(())
                })
            }
            "onaction" if kind == SHAPE => {
                let m = v.to_str()?;
                if m.chars().count() > 255 {
                    return Err(Error::invalid_call());
                }
                self.edit_shape(s, id, |x| {
                    x.macro_name = m;
                    Ok(())
                })
            }
            "alternativetext" => {
                let t = v.to_str()?;
                match kind {
                    SHAPE => self.edit_shape(s, id, |x| {
                        x.shape.alt = t;
                        Ok(())
                    }),
                    CHART => self.edit_chart(s, id, |x| {
                        x.alt = t;
                        Ok(())
                    }),
                    _ => {
                        let mut list = self.wb.sheets[s].images.clone();
                        let i = list.iter().position(|x| x.id == id).ok_or_else(deleted)?;
                        list[i].alt = t;
                        self.apply(Op::Images { sheet: s, list })
                    }
                }
            }
            _ => Err(Error::no_member(name)),
        }
    }

    /// TextFrame (or TextFrame2) and its Characters (or TextRange).
    pub(super) fn shape_text_member(&mut self, s: usize, id: u32, chars: bool, name: &str) -> R<Value> {
        let v = self.shape_of(s, id)?;
        Ok(match name {
            "characters" | "textrange" if !chars => part(Part::ShapeText(s, id, true)),
            "text" | "caption" if chars => Value::str(v.shape.plain_text()),
            "hastext" if !chars => mso_bool(v.shape.has_text()),
            "font" if chars => part(Part::ShapeFont(s, id)),
            "count" | "length" if chars => Value::Long(v.shape.plain_text().chars().count() as i32),
            "parent" => part(Part::Shape(s, SHAPE, id)),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn shape_text_set(&mut self, s: usize, id: u32, chars: bool, name: &str, v: Value) -> R<()> {
        match name {
            "text" | "caption" if chars => {
                let t = shape_text(&v.to_str()?);
                crate::value::string_room(t.len())?;
                self.edit_shape(s, id, |x| {
                    if !x.shape.can_have_text() {
                        return Err(Error::app("This shape can't hold text"));
                    }
                    x.shape.set_plain_text(&t);
                    Ok(())
                })
            }
            _ => Err(Error::no_member(name)),
        }
    }

    /// The font of a shape's text (its first run; setting changes all).
    pub(super) fn shape_font(&mut self, s: usize, id: u32, name: &str, v: Option<Value>) -> R<Value> {
        let x = self.shape_of(s, id)?;
        let Some(v) = v else {
            let f = x.shape.text_format();
            return Ok(match name {
                "bold" => Value::Bool(f.bold),
                "italic" => Value::Bool(f.italic),
                "underline" => Value::Long(if f.underline { 2 } else { -4142 }),
                "size" => Value::Double(f.size.map_or(11.0, f64::from)),
                "name" => Value::str(f.font.as_deref().unwrap_or("Calibri")),
                "color" => Value::Long(swap_rb(f.color.unwrap_or(0)) as i32),
                _ => return Err(Error::no_member(name)),
            });
        };
        let change: Box<dyn Fn(&mut apex_doc::ShapeRun)> = match name {
            "bold" => {
                let b = v.to_bool()?;
                Box::new(move |r| r.bold = b)
            }
            "italic" => {
                let b = v.to_bool()?;
                Box::new(move |r| r.italic = b)
            }
            "underline" => {
                let b = match v {
                    Value::Bool(b) => b,
                    v => !matches!(v.to_i64()?, -4142 | 0),
                };
                Box::new(move |r| r.underline = b)
            }
            "size" => {
                let p = v.to_f64()?;
                if !(1.0..=409.0).contains(&p) {
                    return Err(Error::app("Unable to set the Size property of the Font class"));
                }
                Box::new(move |r| r.size = Some(p as f32))
            }
            "name" => {
                let n: Arc<str> = Arc::from(v.to_str()?);
                Box::new(move |r| r.font = Some(n.clone()))
            }
            "color" => {
                let c = swap_rb(v.to_i64()? as u32 & 0xFF_FFFF);
                Box::new(move |r| r.color = Some(c))
            }
            _ => return Err(Error::no_member(name)),
        };
        self.edit_shape(s, id, |x| {
            x.shape.format_text(&*change);
            Ok(())
        })?;
        Ok(Value::Empty)
    }

    /// A shape's Fill or Line (`line`); `color` is its ForeColor.
    pub(super) fn shape_fill(&mut self, s: usize, id: u32, line: bool, color: bool, name: &str, v: Option<Value>) -> R<Value> {
        let x = self.shape_of(s, id)?;
        let cur = if line { x.shape.line } else { x.shape.fill };
        let Some(v) = v else {
            return Ok(match (color, name) {
                (false, "forecolor") => part(Part::ShapeColor(s, id, line)),
                (false, "visible") => mso_bool(cur.is_some()),
                (false, "weight") if line => Value::Double(f64::from(x.shape.line_width)),
                (false, "solid") => Value::Empty,
                (true, "rgb") => Value::Long(swap_rb(cur.unwrap_or(0)) as i32),
                _ => return Err(Error::no_member(name)),
            });
        };
        match (color, name) {
            (false, "visible") => {
                let on = mso_true(&v)?;
                self.edit_shape(s, id, |x| {
                    let slot = if line { &mut x.shape.line } else { &mut x.shape.fill };
                    if !on {
                        *slot = None;
                    } else if slot.is_none() {
                        *slot = Some(if line { 0x000000 } else { 0xFFFFFF });
                    }
                    if !line && !on {
                        x.shape.gradient = None;
                    }
                    Ok(())
                })?;
            }
            (false, "weight") if line => {
                let w = v.to_f64()?;
                if !(0.0..=1584.0).contains(&w) {
                    return Err(Error::invalid_call());
                }
                self.edit_shape(s, id, |x| {
                    x.shape.line_width = w as f32;
                    Ok(())
                })?;
            }
            (true, "rgb") => {
                let c = swap_rb(v.to_i64()? as u32 & 0xFF_FFFF);
                self.edit_shape(s, id, |x| {
                    if line {
                        x.shape.line = Some(c);
                    } else {
                        x.shape.fill = Some(c);
                        x.shape.gradient = None;
                    }
                    Ok(())
                })?;
            }
            _ => return Err(Error::no_member(name)),
        }
        Ok(Value::Empty)
    }

    // --- ChartObjects ---------------------------------------------------------------

    fn charts_in(&self, s: usize) -> Vec<(u8, u32)> {
        self.wb.sheets.get(s).map(|sh| sh.charts.iter().map(|c| (CHART, c.id)).collect()).unwrap_or_default()
    }

    pub(super) fn chart_objects_member(&mut self, s: usize, name: &str, a: &Args) -> R<Value> {
        self.check_sheet(s)?;
        Ok(match name {
            "count" => Value::Long(self.charts_in(s).len() as i32),
            "item" | "_default" => {
                let items = self.charts_in(s);
                let (_, id) = self.pick(items, s, a)?;
                part(Part::ChartObject(s, id))
            }
            "add" => {
                let r = self.box_args(a, 0, ["left", "top", "width", "height"])?;
                let id = self.new_chart(s, None, r.map(Some), false)?;
                part(Part::ChartObject(s, id))
            }
            "delete" => {
                for (_, id) in self.charts_in(s) {
                    self.delete_shape(s, CHART, id)?;
                }
                Value::Empty
            }
            "select" | "selectall" => Value::Empty,
            "parent" => obj(Obj::Sheet(s)),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn chart_object_member(&mut self, s: usize, id: u32, name: &str, a: &Args) -> R<Value> {
        match name {
            "chart" => {
                self.chart_of(s, id)?;
                Ok(part(Part::Chart(s, id)))
            }
            "index" => {
                let i = self.charts_in(s).iter().position(|x| x.1 == id).ok_or_else(deleted)?;
                Ok(Value::Long(i as i32 + 1))
            }
            "shaperange" => Ok(part(Part::Shape(s, CHART, id))),
            _ => self.shape_member(s, CHART, id, name, a),
        }
    }

    // --- Chart ----------------------------------------------------------------------

    pub(super) fn active_chart(&self) -> Value {
        match self.active_chart {
            Some((s, id)) if self.chart_of(s, id).is_ok() => part(Part::Chart(s, id)),
            _ => Value::Nothing,
        }
    }

    pub(super) fn chart_member(&mut self, s: usize, id: u32, name: &str, a: &Args) -> R<Value> {
        let c = self.chart_of(s, id)?;
        Ok(match name {
            "charttype" => Value::Long(chart_type_code(&c) as i32),
            "hastitle" => Value::Bool(c.show_title && (!c.title.is_empty() || c.series.len() == 1)),
            "charttitle" => part(Part::ChartTitle(s, id, 0)),
            "haslegend" => Value::Bool(c.legend != LegendPos::None),
            "legend" => part(Part::Legend(s, id)),
            "axes" => {
                let k = self.arg_i64(a, 0, "type")?.ok_or_else(|| Error::unsupported("the Axes collection"))?;
                if !matches!(k, 1 | 2) {
                    return Err(Error::unsupported(&format!("axis type {k}")));
                }
                if !c.kind.has_axes() {
                    return Err(Error::app("This chart type has no axes"));
                }
                if self.arg_i64(a, 1, "axisgroup")?.unwrap_or(1) != 1 {
                    return Err(Error::unsupported("the secondary axis group"));
                }
                part(Part::ChartAxis(s, id, k as u8))
            }
            "seriescollection" => {
                if a.is_empty() {
                    part(Part::SeriesCollection(s, id))
                } else {
                    return self.series_collection(s, id, "item", a);
                }
            }
            "setsourcedata" => {
                let v = a.get(0, "source").ok_or_else(|| Error::new(449, "Argument not optional"))?;
                // Areas side by side (`"A1:A3,C1:C3"`): the block around
                // them, keeping the series that come from them.
                let areas = match &v {
                    Value::Object(Obj::Multi(_)) => Some(super::areas::ranges_of(&v).ok_or_else(Error::type_mismatch)?),
                    Value::Str(t) if t.contains(',') => Some(super::areas::ranges_of(&self.range_call(s, &Args(vec![(None, v.clone())]))?).ok_or_else(Error::type_mismatch)?),
                    _ => None,
                };
                let r = match &areas {
                    Some(list) => {
                        let first = list[0];
                        if list.iter().any(|x| x.sheet != first.sheet) {
                            return Err(Error::type_mismatch());
                        }
                        let range = list.iter().fold(first.range, |b, x| Range::new(CellRef::new(b.start.row.min(x.range.start.row), b.start.col.min(x.range.start.col)), CellRef::new(b.end.row.max(x.range.end.row), b.end.col.max(x.range.end.col))));
                        crate::object::RangeRef { sheet: first.sheet, range, by: crate::object::By::Cells }
                    }
                    None => self.range_arg(v, s)?,
                };
                let rs = self.check_sheet(r.sheet)?;
                let by_rows = match self.arg_i64(a, 1, "plotby")? {
                    None => None,
                    Some(1) => Some(true),
                    Some(2) => Some(false),
                    Some(_) => return Err(Error::invalid_call()),
                };
                if r.range.cell_count() > 1_000_000 && self.wb.clamp_to_used(rs, &r.range).is_none() {
                    return Err(Error::app("The source data has no values"));
                }
                let mut fresh = self.wb.chart_from_range(rs, r.range, c.kind, by_rows);
                if let Some(list) = &areas {
                    let inside = |d: &DataRef| d.area().is_some_and(|(_, g)| list.iter().any(|x| x.range.contains(g.start)));
                    fresh.series.retain(|x| inside(&x.values));
                }
                self.edit_chart(s, id, |c| {
                    c.series = fresh.series;
                    c.source = fresh.source;
                    c.by_rows = fresh.by_rows;
                    c.legend = fresh.legend;
                    Ok(())
                })?;
                Value::Empty
            }
            "chartstyle" => Value::Long(201 + i32::from(c.style)),
            "clearcontents" | "chartarea" | "plotarea" | "applylayout" | "export" | "copypicture" | "location" | "printout" | "charttitletext" => return Err(Error::unsupported(&format!("Chart.{name}"))),
            "name" => Value::str(format!("{} {}", self.wb.sheets[s].name, c.name)),
            "parent" => part(Part::ChartObject(s, id)),
            "activate" | "select" => {
                self.active_chart = Some((s, id));
                Value::Empty
            }
            "refresh" => Value::Empty,
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn chart_set(&mut self, s: usize, id: u32, name: &str, v: Value) -> R<()> {
        match name {
            "charttype" => {
                let code = v.to_i64()?;
                self.edit_chart(s, id, |c| set_chart_type(c, code))
            }
            "hastitle" => {
                let b = v.to_bool()?;
                self.edit_chart(s, id, |c| {
                    c.show_title = b;
                    Ok(())
                })
            }
            "haslegend" => {
                let b = v.to_bool()?;
                self.edit_chart(s, id, |c| {
                    if !b {
                        c.legend = LegendPos::None;
                    } else if c.legend == LegendPos::None {
                        c.legend = LegendPos::Right;
                    }
                    Ok(())
                })
            }
            "chartstyle" => {
                // Excel's gallery numbers (201...) have no match here.
                v.to_i64()?;
                Ok(())
            }
            _ => Err(Error::no_member(name)),
        }
    }

    /// ChartTitle (0) or an axis title (the axis).
    pub(super) fn chart_title(&mut self, s: usize, id: u32, axis: u8, name: &str, v: Option<Value>) -> R<Value> {
        let c = self.chart_of(s, id)?;
        let text = match axis {
            0 => c.title.clone(),
            1 => c.x_axis.title.clone(),
            _ => c.y_axis.title.clone(),
        };
        match (name, v) {
            ("text" | "caption", None) => Ok(Value::str(text.as_str())),
            ("text" | "caption", Some(v)) => {
                let t = v.to_str()?;
                if t.chars().count() > 255 {
                    return Err(Error::app("The title is too long"));
                }
                self.edit_chart(s, id, |c| {
                    match axis {
                        0 => {
                            c.title = t;
                            c.show_title = true;
                        }
                        1 => c.x_axis.title = t,
                        _ => c.y_axis.title = t,
                    }
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            ("delete", None) => {
                self.edit_chart(s, id, |c| {
                    match axis {
                        0 => c.show_title = false,
                        1 => c.x_axis.title.clear(),
                        _ => c.y_axis.title.clear(),
                    }
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            ("parent", None) => Ok(if axis == 0 { part(Part::Chart(s, id)) } else { part(Part::ChartAxis(s, id, axis)) }),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn legend_member(&mut self, s: usize, id: u32, name: &str, v: Option<Value>) -> R<Value> {
        const POS: [(i32, LegendPos); 4] = [(-4107, LegendPos::Bottom), (-4131, LegendPos::Left), (-4152, LegendPos::Right), (-4160, LegendPos::Top)];
        let c = self.chart_of(s, id)?;
        match (name, v) {
            ("position", None) => Ok(Value::Long(POS.iter().find(|p| p.1 == c.legend).map_or(-4152, |p| p.0))),
            ("position", Some(v)) => {
                let code = v.to_i64()?;
                let pos = POS.iter().find(|p| i64::from(p.0) == code).map(|p| p.1).ok_or_else(|| Error::unsupported(&format!("legend position {code}")))?;
                self.edit_chart(s, id, |c| {
                    c.legend = pos;
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            ("delete", None) => {
                self.edit_chart(s, id, |c| {
                    c.legend = LegendPos::None;
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            ("parent", None) => Ok(part(Part::Chart(s, id))),
            _ => Err(Error::no_member(name)),
        }
    }

    /// The scale the axis shows when it is automatic: from the values.
    fn auto_scale(&self, s: usize, c: &Chart) -> Option<apex_sheet::chart::Scale> {
        let data = self.wb.chart_data(s, c);
        let vals = data.series.iter().flat_map(|x| x.values.iter().flatten().copied()).filter(|v| v.is_finite());
        let (mut lo, mut hi) = vals.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v), b.max(v)));
        if !lo.is_finite() {
            return None;
        }
        if !matches!(c.kind, ChartType::Line | ChartType::Scatter | ChartType::Bubble | ChartType::Stock) || c.y_axis.log_base.is_some() {
            if c.y_axis.log_base.is_none() {
                lo = lo.min(0.0);
                hi = hi.max(0.0);
            }
        }
        Some(scale_for(lo, hi, 5, &c.y_axis))
    }

    pub(super) fn axis_member(&mut self, s: usize, id: u32, k: u8, name: &str, v: Option<Value>) -> R<Value> {
        let c = self.chart_of(s, id)?;
        let ax: &Axis = if k == 1 { &c.x_axis } else { &c.y_axis };
        let Some(v) = v else {
            let scale = |f: fn(&Axis) -> Option<f64>, g: fn(&apex_sheet::chart::Scale) -> f64| -> Value {
                match f(ax) {
                    Some(x) => Value::Double(x),
                    None => self.auto_scale(s, &c).map_or(Value::Double(0.0), |sc| Value::Double(g(&sc))),
                }
            };
            return Ok(match name {
                "hastitle" => Value::Bool(!ax.title.is_empty()),
                "axistitle" => part(Part::ChartTitle(s, id, k)),
                "hasmajorgridlines" => Value::Bool(ax.major_grid),
                "hasminorgridlines" => Value::Bool(ax.minor_grid),
                "reverseplotorder" => Value::Bool(ax.reverse),
                "minimumscale" if k == 2 => scale(|a| a.min, |s| s.min),
                "maximumscale" if k == 2 => scale(|a| a.max, |s| s.max),
                "majorunit" if k == 2 => scale(|a| a.major, |s| s.major),
                "minorunit" if k == 2 => scale(|a| a.minor, |s| s.minor),
                "minimumscaleisauto" => Value::Bool(ax.min.is_none()),
                "maximumscaleisauto" => Value::Bool(ax.max.is_none()),
                "majorunitisauto" => Value::Bool(ax.major.is_none()),
                "minorunitisauto" => Value::Bool(ax.minor.is_none()),
                "scaletype" => Value::Long(if ax.log_base.is_some() { -4133 } else { -4132 }),
                "logbase" => Value::Double(ax.log_base.unwrap_or(10.0)),
                "ticklabels" => part(Part::TickLabels(s, id, k)),
                "type" => Value::Long(i32::from(k)),
                "parent" => part(Part::Chart(s, id)),
                "delete" => {
                    self.edit_chart(s, id, |c| {
                        if k == 1 { c.x_axis.visible = false } else { c.y_axis.visible = false }
                        Ok(())
                    })?;
                    Value::Empty
                }
                _ => return Err(Error::no_member(name)),
            });
        };
        let num = |v: &Value| -> R<f64> {
            let x = v.to_f64()?;
            if x.is_finite() { Ok(x) } else { Err(Error::invalid_call()) }
        };
        let change: Box<dyn FnOnce(&mut Axis) -> R<()>> = match name {
            "hastitle" => {
                let b = v.to_bool()?;
                Box::new(move |a| {
                    if !b {
                        a.title.clear();
                    } else if a.title.is_empty() {
                        a.title = "Axis Title".into();
                    }
                    Ok(())
                })
            }
            "hasmajorgridlines" => {
                let b = v.to_bool()?;
                Box::new(move |a| {
                    a.major_grid = b;
                    Ok(())
                })
            }
            "hasminorgridlines" => {
                let b = v.to_bool()?;
                Box::new(move |a| {
                    a.minor_grid = b;
                    Ok(())
                })
            }
            "reverseplotorder" => {
                let b = v.to_bool()?;
                Box::new(move |a| {
                    a.reverse = b;
                    Ok(())
                })
            }
            "minimumscale" | "maximumscale" | "majorunit" | "minorunit" if k == 2 => {
                let x = num(&v)?;
                if name.ends_with("unit") && x <= 0.0 {
                    return Err(Error::invalid_call());
                }
                let which = name.to_string();
                Box::new(move |a| {
                    match which.as_str() {
                        "minimumscale" => a.min = Some(x),
                        "maximumscale" => a.max = Some(x),
                        "majorunit" => a.major = Some(x),
                        _ => a.minor = Some(x),
                    }
                    Ok(())
                })
            }
            "minimumscaleisauto" | "maximumscaleisauto" | "majorunitisauto" | "minorunitisauto" => {
                let auto = v.to_bool()?;
                let which = name.to_string();
                Box::new(move |a| {
                    if auto {
                        match which.as_str() {
                            "minimumscaleisauto" => a.min = None,
                            "maximumscaleisauto" => a.max = None,
                            "majorunitisauto" => a.major = None,
                            _ => a.minor = None,
                        }
                    }
                    Ok(())
                })
            }
            "scaletype" if k == 2 => {
                let log = match v.to_i64()? {
                    -4133 => true,
                    -4132 => false,
                    _ => return Err(Error::invalid_call()),
                };
                Box::new(move |a| {
                    a.log_base = if log { Some(a.log_base.unwrap_or(10.0)) } else { None };
                    Ok(())
                })
            }
            "logbase" if k == 2 => {
                let b = num(&v)?;
                if !(2.0..=1000.0).contains(&b) {
                    return Err(Error::invalid_call());
                }
                Box::new(move |a| {
                    a.log_base = Some(b);
                    Ok(())
                })
            }
            _ => return Err(Error::no_member(name)),
        };
        self.edit_chart(s, id, |c| change(if k == 1 { &mut c.x_axis } else { &mut c.y_axis }))?;
        Ok(Value::Empty)
    }

    pub(super) fn tick_labels(&mut self, s: usize, id: u32, k: u8, name: &str, v: Option<Value>) -> R<Value> {
        let c = self.chart_of(s, id)?;
        let ax = if k == 1 { &c.x_axis } else { &c.y_axis };
        match (name, v) {
            ("numberformat", None) => Ok(Value::str(if ax.format.is_empty() { "General" } else { &ax.format })),
            ("numberformat", Some(v)) => {
                let f = v.to_str()?;
                if f.len() > 255 {
                    return Err(Error::invalid_call());
                }
                let f = if f.eq_ignore_ascii_case("general") { String::new() } else { f };
                self.edit_chart(s, id, |c| {
                    if k == 1 { c.x_axis.format = f } else { c.y_axis.format = f }
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            _ => Err(Error::no_member(name)),
        }
    }

    // --- Series ---------------------------------------------------------------------

    pub(super) fn series_collection(&mut self, s: usize, id: u32, name: &str, a: &Args) -> R<Value> {
        let c = self.chart_of(s, id)?;
        Ok(match name {
            "count" => Value::Long(c.series.len() as i32),
            "item" | "_default" => {
                let v = self.scalar(a.get(0, "index").ok_or_else(|| Error::new(449, "Argument not optional"))?)?;
                let i = if let Value::Str(n) = &v {
                    let data = self.wb.chart_data(s, &c);
                    data.series.iter().position(|x| x.name.eq_ignore_ascii_case(n)).ok_or_else(|| Error::app("The item with the specified name wasn't found"))?
                } else {
                    let i = v.to_i64()?;
                    if i < 1 || i as usize > c.series.len() {
                        return Err(Error::app("The index into the specified collection is out of bounds"));
                    }
                    i as usize - 1
                };
                part(Part::Series(s, id, i))
            }
            "newseries" => {
                if c.series.len() >= 255 {
                    return Err(Error::app("A chart can have no more than 255 series"));
                }
                let n = c.series.len();
                self.edit_chart(s, id, |c| {
                    c.series.push(Series::default());
                    Ok(())
                })?;
                part(Part::Series(s, id, n))
            }
            "add" => {
                let v = a.get(0, "source").ok_or_else(|| Error::new(449, "Argument not optional"))?;
                let r = self.range_arg(v, s)?;
                let rs = self.check_sheet(r.sheet)?;
                let by_rows = match self.arg_i64(a, 1, "rowcol")? {
                    Some(1) => Some(true),
                    Some(2) | None => Some(false),
                    Some(_) => return Err(Error::invalid_call()),
                };
                let more = self.wb.chart_from_range(rs, r.range, c.kind, by_rows).series;
                if c.series.len() + more.len() > 255 {
                    return Err(Error::app("A chart can have no more than 255 series"));
                }
                self.edit_chart(s, id, |c| {
                    c.series.extend(more);
                    Ok(())
                })?;
                part(Part::SeriesCollection(s, id))
            }
            "parent" => part(Part::Chart(s, id)),
            _ => return Err(Error::no_member(name)),
        })
    }

    /// A data reference from a Range, "=Sheet1!$A$1:$A$9", or an array of
    /// values.
    fn data_ref(&mut self, s: usize, v: Value) -> R<DataRef> {
        match v {
            Value::Object(Obj::Range(r)) => {
                let rs = self.check_sheet(r.sheet)?;
                Ok(DataRef::range(&self.wb.sheets[rs].name, r.range))
            }
            Value::Array(a) => {
                if a.data.len() > 1_000_000 {
                    return Err(Error::invalid_call());
                }
                let vals = a.data.iter().map(|x| match x {
                    Value::Str(t) => apex_sheet::value::Value::text(t.to_string()),
                    Value::Empty | Value::Null => apex_sheet::value::Value::Empty,
                    x => x.to_f64().map_or(apex_sheet::value::Value::Empty, apex_sheet::value::Value::Number),
                });
                Ok(DataRef::literal(vals.collect()))
            }
            v => {
                let t = v.to_str()?;
                if let Some(f) = t.strip_prefix('=') {
                    let r = self.parse_ref(f, s)?;
                    return Ok(DataRef::range(&self.wb.sheets[self.check_sheet(r.sheet)?].name, r.range));
                }
                Err(Error::type_mismatch())
            }
        }
    }

    pub(super) fn series_member(&mut self, s: usize, id: u32, i: usize, name: &str, v: Option<Value>) -> R<Value> {
        let c = self.chart_of(s, id)?;
        if i >= c.series.len() {
            return Err(deleted());
        }
        let Some(v) = v else {
            let data = || self.wb.chart_data(s, &c).series.into_iter().nth(i).unwrap_or_default();
            let list = |vals: Vec<Value>| Value::Array(Rc::new(Array::from_vec(vals, 1)));
            return Ok(match name {
                "name" => Value::str(data().name),
                "values" => list(data().values.into_iter().map(|x| x.map_or(Value::Empty, Value::Double)).collect()),
                "xvalues" => list(data().categories.into_iter().map(|x| Value::str(x)).collect()),
                "plotorder" => Value::Long(i as i32 + 1),
                "formula" => {
                    let r = |d: &DataRef| d.to_canonical().unwrap_or_default();
                    let x = &c.series[i];
                    Value::str(format!("=SERIES({},{},{},{})", r(&x.name), r(&x.categories), r(&x.values), i + 1))
                }
                "delete" => {
                    self.edit_chart(s, id, |c| {
                        c.series.remove(i);
                        Ok(())
                    })?;
                    Value::Empty
                }
                "parent" => part(Part::Chart(s, id)),
                _ => return Err(Error::no_member(name)),
            });
        };
        let d = match name {
            "name" => match &v {
                Value::Str(t) if !t.starts_with('=') => DataRef::text(t),
                _ => self.data_ref(s, v)?,
            },
            "values" | "xvalues" => self.data_ref(s, v)?,
            "charttype" => {
                let code = v.to_i64()?;
                let kind = match CHART_TYPES.iter().find(|(k, _)| *k == code).map(|x| x.1.0) {
                    Some(k @ (ChartType::Column | ChartType::Line | ChartType::Area)) => k,
                    _ => return Err(Error::unsupported(&format!("series chart type {code}"))),
                };
                self.edit_chart(s, id, |c| {
                    c.series[i].kind = Some(kind);
                    Ok(())
                })?;
                return Ok(Value::Empty);
            }
            "axisgroup" => {
                let two = match v.to_i64()? {
                    1 => false,
                    2 => true,
                    _ => return Err(Error::invalid_call()),
                };
                self.edit_chart(s, id, |c| {
                    c.series[i].secondary = two;
                    Ok(())
                })?;
                return Ok(Value::Empty);
            }
            _ => return Err(Error::no_member(name)),
        };
        self.edit_chart(s, id, |c| {
            let x = &mut c.series[i];
            match name {
                "name" => x.name = d,
                "values" => x.values = d,
                _ => x.categories = d,
            }
            Ok(())
        })?;
        Ok(Value::Empty)
    }

    /// For Each over Shapes, ChartObjects and SeriesCollection.
    pub(super) fn drawing_items(&self, p: &Part) -> Option<Vec<Value>> {
        Some(match p {
            Part::Shapes(s) => self.shapes_in(*s).into_iter().map(|(k, id)| part(Part::Shape(*s, k, id))).collect(),
            Part::ChartObjects(s) => self.charts_in(*s).into_iter().map(|(_, id)| part(Part::ChartObject(*s, id))).collect(),
            Part::SeriesCollection(s, id) => {
                let n = self.chart_of(*s, *id).ok()?.series.len();
                (0..n).map(|i| part(Part::Series(*s, *id, i))).collect()
            }
            _ => return None,
        })
    }
}
