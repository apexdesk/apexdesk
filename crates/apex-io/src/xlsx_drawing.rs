//! Pictures and charts of a worksheet in .xlsx: `xl/drawings/drawingN.xml`
//! with its anchors, `xl/media/*` and `xl/charts/*` (see [`crate::xlsx_chart`]),
//! and shapes (`xdr:sp`, see [`crate::drawingml`]). Groups are not kept.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_sheet::{Anchor, CellRef, Chart, ImageFormat, MAX_COLS, MAX_ROWS, Sheet, SheetImage, SheetShape, Workbook};

use crate::drawingml::{self, Node};

use crate::xml::{Ev, HEADER, Reader, esc};
use crate::zip::ZipReader;

pub const REL_DRAWING: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing";
pub(crate) const REL_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";

/// EMU per pixel at 96 dpi.
const EMU_PX: f32 = 9525.0;
/// Sheet pixels per Excel pixel: our cells are wider and taller than
/// Excel's, so anchors scale to land on the same place of the same cell
/// while pictures keep their real size.
pub(crate) const SX: f32 = 88.0 / 64.0;
pub(crate) const SY: f32 = 22.0 / 20.0;
/// Pictures read from one sheet at most (hostile files).
const MAX_PICTURES: usize = 10_000;

pub(crate) fn emu(v: &str) -> f32 {
    v.trim().parse::<f64>().ok().filter(|x| x.is_finite()).map_or(0.0, |x| (x / EMU_PX as f64) as f32)
}

fn to_emu(px: f32) -> i64 {
    if px.is_finite() { (px as f64 * EMU_PX as f64).round() as i64 } else { 0 }
}

/// A cell corner of a two-cell anchor, in Excel pixels from the sheet
/// origin.
fn excel_x(sh: &Sheet, col: u32, off: f32) -> f32 {
    sh.col_x(col) / SX + off
}

fn excel_y(sh: &Sheet, row: u32, off: f32) -> f32 {
    sh.row_y(row) / SY + off
}

#[derive(Default)]
struct Marker {
    col: u32,
    row: u32,
    col_off: f32,
    row_off: f32,
}

#[derive(Default)]
struct Raw {
    kind: &'static str,
    edit_as: String,
    from: Option<Marker>,
    to: Option<Marker>,
    pos: Option<(f32, f32)>,
    ext: Option<(f32, f32)>,
    xfrm_ext: Option<(f32, f32)>,
    pic: bool,
    group: bool,
    /// A graphic frame (charts) and the chart relationship in it.
    frame: bool,
    chart_rid: Option<String>,
    chartex: bool,
    /// The chart in the `mc:Fallback` of a chartex frame.
    fallback_rid: Option<String>,
    name: Option<String>,
    descr: String,
    embed: Option<String>,
    crop: [f32; 4],
    rot: f32,
    lock: bool,
    /// A shape (`xdr:sp` or a connector).
    sp: Option<Node>,
    /// The `dgm:relIds` of a SmartArt frame.
    diagram: Option<Node>,
}

/// Excel anchors shapes turned 45..135 degrees (or 225..315) by their box
/// turned a quarter.
fn swapped(rot: f32) -> bool {
    (45.0..135.0).contains(&rot.rem_euclid(180.0))
}

fn read_marker(r: &mut Reader) -> Marker {
    let mut m = Marker::default();
    loop {
        match r.next() {
            Ev::Start(t) => {
                let v = r.text();
                let n = || v.trim().parse::<u32>().unwrap_or(0);
                match t.local() {
                    "col" => m.col = n().min(MAX_COLS - 1),
                    "row" => m.row = n().min(MAX_ROWS - 1),
                    "colOff" => m.col_off = emu(&v),
                    "rowOff" => m.row_off = emu(&v),
                    _ => {}
                }
            }
            Ev::End(_) | Ev::Eof => return m,
            _ => {}
        }
    }
}

/// Reads one anchor element whose start was just returned.
fn read_anchor(r: &mut Reader, a: &mut Raw) {
    let mut depth = 1;
    loop {
        let ev = r.next();
        let empty = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof => return,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                match t.local() {
                    "sp" | "cxnSp" if depth == 1 && !a.pic && !a.frame && !a.group => {
                        a.sp = Some(drawingml::read_node(r, &t, empty));
                        continue;
                    }
                    "Fallback" if !empty => {
                        // Only the chart a newer chart falls back to.
                        let mut d = 1;
                        loop {
                            match r.next() {
                                Ev::Start(_) => d += 1,
                                Ev::Empty(t) if t.local() == "chart" && a.fallback_rid.is_none() => a.fallback_rid = t.attr("r:id").or_else(|| t.attr("id")).map(|v| v.into_owned()),
                                Ev::End(_) => {
                                    d -= 1;
                                    if d == 0 {
                                        break;
                                    }
                                }
                                Ev::Eof => return,
                                _ => {}
                            }
                        }
                        continue;
                    }
                    "graphicFrame" if !a.pic && !a.group => a.frame = true,
                    "graphicData" if a.frame => a.chartex = t.attr("uri").is_some_and(|u| u.contains("chartex")),
                    "relIds" if a.frame && a.diagram.is_none() => {
                        a.diagram = Some(drawingml::read_node(r, &t, empty));
                        continue;
                    }
                    "chart" if a.frame && a.chart_rid.is_none() => a.chart_rid = t.attr("r:id").or_else(|| t.attr("id")).map(|v| v.into_owned()),
                    "from" | "to" if depth == 1 && !empty => {
                        let m = read_marker(r);
                        if t.local() == "from" {
                            a.from = Some(m);
                        } else {
                            a.to = Some(m);
                        }
                        continue;
                    }
                    "pos" if depth == 1 => a.pos = Some((emu(&t.attr("x").unwrap_or_default()), emu(&t.attr("y").unwrap_or_default()))),
                    "ext" if depth == 1 => a.ext = Some((emu(&t.attr("cx").unwrap_or_default()), emu(&t.attr("cy").unwrap_or_default()))),
                    "ext" if a.pic && t.attr("cx").is_some() && a.xfrm_ext.is_none() => {
                        a.xfrm_ext = Some((emu(&t.attr("cx").unwrap_or_default()), emu(&t.attr("cy").unwrap_or_default())));
                    }
                    "pic" if !a.pic => a.pic = true,
                    "grpSp" if depth == 1 => a.group = true,
                    "cNvPr" if (a.pic || a.frame) && a.name.is_none() => {
                        a.name = Some(t.attr("name").unwrap_or_default().into_owned());
                        a.descr = t.attr("descr").unwrap_or_default().into_owned();
                    }
                    "picLocks" => a.lock = t.attr("noChangeAspect").is_some_and(|v| v == "1" || v == "true"),
                    "blip" if a.pic && a.embed.is_none() => a.embed = t.attr("r:embed").or_else(|| t.attr("embed")).map(|v| v.into_owned()),
                    "srcRect" if a.pic => {
                        for (i, k) in ["l", "t", "r", "b"].iter().enumerate() {
                            a.crop[i] = t.attr(k).and_then(|v| v.trim().parse::<f32>().ok()).map_or(0.0, |v| (v / 100_000.0).max(0.0));
                        }
                    }
                    "xfrm" if a.pic => a.rot = t.attr("rot").and_then(|v| v.trim().parse::<f32>().ok()).map_or(0.0, |v| v / 60_000.0),
                    _ => {}
                }
                if !empty {
                    depth += 1;
                }
            }
            _ => {}
        }
    }
}

/// Pictures of the worksheet part at `path` (with its xml), added to `sh`.
/// Cached chart values one sheet keeps, all its charts together.
const SHEET_CHART_POINTS: usize = 4_000_000;

/// Cached chart values a workbook keeps, all its sheets together.
pub const BOOK_CHART_POINTS: usize = 16_000_000;

pub fn read_drawings(z: &ZipReader, path: &str, xml: &str, sh: &mut Sheet, book_points: &mut usize) {
    let tail = xml.find("</sheetData>").map_or(xml, |i| &xml[i..]);
    let Some(i) = tail.find("<drawing ").or_else(|| tail.find(":drawing ")) else { return };
    let mut from = i.saturating_sub(10);
    while !tail.is_char_boundary(from) {
        from -= 1;
    }
    let mut r = Reader::new(&tail[from..]);
    let mut rid = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "drawing" => {
                rid = t.attr("r:id").or_else(|| t.attr("id")).map(|v| v.into_owned());
                break;
            }
            _ => {}
        }
    }
    if let Some(rid) = rid {
        read_drawing_rel(z, path, &rid, sh, book_points);
    }
}

/// Pictures, shapes and charts of the drawing that the sheet part at
/// `path` relates to as `rid` (also for .xlsb sheets).
pub fn read_drawing_rel(z: &ZipReader, path: &str, rid: &str, sh: &mut Sheet, book_points: &mut usize) {
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    let rels = z.read_string(&format!("{dir}/_rels/{file}.rels")).map(|s| crate::xlsx_notes::read_rels(&s)).unwrap_or_default();
    let Some((_, target, false)) = rels.get(rid) else { return };
    let dpath = crate::xlsx::resolve_target(dir, target);
    let Ok(dxml) = z.read_string(&dpath) else { return };
    let (ddir, dfile) = dpath.rsplit_once('/').unwrap_or(("", &dpath));
    let drels = z.read_string(&format!("{ddir}/_rels/{dfile}.rels")).map(|s| crate::xlsx_notes::read_rels(&s)).unwrap_or_default();
    // Media parts shared by several pictures are read once.
    let mut media: HashMap<String, Option<(Arc<[u8]>, ImageFormat)>> = HashMap::new();
    let mut diagrams = None;
    // Cached chart values the sheet keeps: many anchors on one big chart
    // part would each read it again.
    let mut chart_points = 0usize;
    let mut r = Reader::new(&dxml);
    loop {
        let ev = r.next();
        let (t, empty) = match ev {
            Ev::Eof => break,
            Ev::Start(t) => (t, false),
            Ev::Empty(t) => (t, true),
            _ => continue,
        };
        if t.local() == "AlternateContent" && !empty {
            // Ink: the choice with the content part, not its fallback.
            let mut peek = r.clone();
            let raw = peek.raw_element();
            if raw.contains("contentPart") {
                r = peek;
                if sh.shapes.len() < MAX_PICTURES {
                    if let Some(s) = read_ink(z, ddir, &drels, sh, raw) {
                        sh.shapes.push(s);
                    }
                }
                continue;
            }
        }
        let kind = match t.local() {
            "twoCellAnchor" => "two",
            "oneCellAnchor" => "one",
            "absoluteAnchor" => "abs",
            _ => continue,
        };
        if empty {
            continue;
        }
        let mut a = Raw { kind, edit_as: t.attr("editAs").unwrap_or_default().into_owned(), ..Default::default() };
        read_anchor(&mut r, &mut a);
        if let Some(sp) = a.sp.take() {
            // Excel's hidden stand-ins for embedded objects (read from the
            // sheet's `oleObjects`) are skipped.
            if sh.shapes.len() < MAX_PICTURES && sp.find("compatExt").is_none() {
                if let Some(s) = to_shape(sh, &mut a, &sp) {
                    sh.shapes.push(s);
                }
            }
            continue;
        }
        if let Some(ids) = a.diagram.take().filter(|_| a.frame && !a.group && sh.images.len() < MAX_PICTURES) {
            let parts = diagrams.get_or_insert_with(|| crate::diagram::Parts::load(z, &dpath));
            if let Some(mut img) = to_image(sh, &a, Arc::from(&[][..]), ImageFormat::Png)
                && let Some(d) = parts.diagram(&ids, (img.width * 0.75, img.height * 0.75))
            {
                img.object = Some(Arc::new(apex_doc::Embedded::new(apex_doc::EmbedKind::Diagram(d))));
                sh.images.push(img);
            }
            continue;
        }
        if a.frame && !a.group && !a.pic {
            if sh.charts.len() < MAX_PICTURES && chart_points < SHEET_CHART_POINTS && *book_points > 0 {
                if let Some(c) = read_frame_chart(z, ddir, &drels, sh, &a) {
                    let n = c.series.iter().map(|s| 1 + s.name.cache.len() + s.categories.cache.len() + s.values.cache.len()).sum::<usize>();
                    chart_points += n;
                    *book_points = book_points.saturating_sub(n);
                    sh.charts.push(c);
                }
            }
            continue;
        }
        if !a.pic || a.group || sh.images.len() >= MAX_PICTURES {
            continue;
        }
        let Some((_, target, false)) = a.embed.as_ref().and_then(|e| drels.get(e)).filter(|(ty, _, _)| ty == REL_IMAGE || ty.ends_with("/image")) else { continue };
        let mpath = crate::xlsx::resolve_target(ddir, target);
        let entry = media.entry(mpath.clone()).or_insert_with(|| {
            let data = z.read(&mpath).ok()?;
            let ext = mpath.rsplit_once('.').map_or("", |x| x.1);
            let fmt = ImageFormat::detect(&data, ext)?;
            Some((Arc::from(data), fmt))
        });
        let Some((data, format)) = entry.clone() else { continue };
        if let Some(img) = to_image(sh, &a, data, format) {
            sh.images.push(img);
        }
    }
    let mut id = 0;
    for img in &mut sh.images {
        id += 1;
        img.id = id;
    }
    for (i, s) in sh.shapes.iter_mut().enumerate() {
        s.id = i as u32 + 1;
    }
    for (i, c) in sh.charts.iter_mut().enumerate() {
        c.id = i as u32 + 1;
        if c.name.is_empty() {
            c.name = format!("Chart {}", c.id);
        }
    }
}

/// The chart of a graphic frame: its part (a chartex one, else its
/// fallback), placed as the anchor says.
fn read_frame_chart(z: &ZipReader, ddir: &str, drels: &HashMap<String, (String, String, bool)>, sh: &Sheet, a: &Raw) -> Option<Chart> {
    let part = |rid: &Option<String>| -> Option<String> {
        let (_, target, external) = drels.get(rid.as_ref()?)?;
        if *external {
            return None;
        }
        z.read_string(&crate::xlsx::resolve_target(ddir, target)).ok()
    };
    let mut chart = None;
    if a.chartex {
        chart = part(&a.chart_rid).and_then(|x| crate::xlsx_chart::read_chartex(&x));
    } else if let Some(x) = part(&a.chart_rid) {
        chart = crate::xlsx_chart::read_chart(&x);
    }
    let mut chart = match chart {
        Some(c) => c,
        None => crate::xlsx_chart::read_chart(&part(&a.fallback_rid)?)?,
    };
    // The same geometry as a picture's.
    let place = to_image(sh, a, Arc::from(&[][..]), ImageFormat::Png)?;
    chart.from = place.from;
    chart.dx = place.dx;
    chart.dy = place.dy;
    chart.width = place.width;
    chart.height = place.height;
    chart.anchor = place.anchor;
    chart.name = a.name.clone().unwrap_or_default();
    chart.alt = a.descr.clone();
    chart.sanitize();
    Some(chart)
}

/// Ink (a content part) in the choice of an `mc:AlternateContent` whose
/// inside is `raw`, placed as its anchor says.
fn read_ink(z: &ZipReader, ddir: &str, drels: &HashMap<String, (String, String, bool)>, sh: &Sheet, raw: &str) -> Option<SheetShape> {
    let root = drawingml::read_fragment(raw);
    let choice = root.children("Choice").find(|c| c.find("contentPart").is_some())?;
    let part = choice.find("contentPart")?;
    let (_, target, false) = drels.get(part.attr("r:id").or_else(|| part.attr("id"))?)? else { return None };
    let (xml, _) = crate::ooxml_keep::decode_xml(&z.read(&crate::xlsx::resolve_target(ddir, target)).ok()?);
    // The anchor, read as a picture's: from the choice's source.
    let start = raw.find("Choice")?;
    let mut sub = Reader::new(&raw[start..]);
    let mut a = Raw::default();
    loop {
        match sub.next() {
            Ev::Eof => return None,
            Ev::Start(t) => {
                a.kind = match t.local() {
                    "twoCellAnchor" => "two",
                    "oneCellAnchor" => "one",
                    "absoluteAnchor" => "abs",
                    _ => continue,
                };
                a.edit_as = t.attr("editAs").unwrap_or_default().into_owned();
                read_anchor(&mut sub, &mut a);
                break;
            }
            _ => {}
        }
    }
    let x = part.child("xfrm");
    a.xfrm_ext = x.and_then(|x| x.child("ext")).and_then(|e| Some((e.num("cx")? as f32 / EMU_PX, e.num("cy")? as f32 / EMU_PX)));
    a.rot = x.and_then(|x| x.num("rot")).map_or(0.0, |v| (v / 60_000.0) as f32);
    let place = to_image(sh, &a, Arc::from(&[][..]), ImageFormat::Png)?;
    let mut shape = apex_doc::Shape::ink(crate::inkml::read(&xml));
    shape.rotation = a.rot;
    shape.flip_h = x.and_then(|x| x.attr("flipH")).is_some_and(|v| v == "1" || v == "true");
    shape.flip_v = x.and_then(|x| x.attr("flipV")).is_some_and(|v| v == "1" || v == "true");
    if let Some(c) = part.find("cNvPr") {
        shape.name = c.attr("name").unwrap_or("Ink").to_string();
        shape.alt = c.attr("descr").unwrap_or_default().to_string();
    }
    let mut s = SheetShape::new(shape, place.from, place.width, place.height);
    (s.dx, s.dy, s.anchor) = (place.dx, place.dy, place.anchor);
    s.sanitize();
    Some(s)
}

/// A shape placed as its anchor says.
fn to_shape(sh: &Sheet, a: &mut Raw, sp: &Node) -> Option<SheetShape> {
    let sp_pr = sp.child("spPr");
    let style = sp.child("style");
    let text_box = sp.find("cNvSpPr").and_then(|n| n.attr("txBox")).is_some_and(|v| v == "1" || v == "true");
    let mut shape = drawingml::read_look(sp_pr, style, text_box);
    if let Some(ink) = crate::inkml::from_sp_pr(sp_pr) {
        shape = apex_doc::Shape::ink(ink);
    }
    if let Some(body) = sp.child("txBody") {
        drawingml::read_tx_body(body, style, &mut shape);
    }
    if let Some(c) = sp.find("cNvPr") {
        shape.name = c.attr("name").unwrap_or_default().to_string();
        shape.alt = c.attr("descr").unwrap_or_default().to_string();
    }
    a.rot = shape.rotation;
    let ext = drawingml::xfrm_ext(sp_pr, EMU_PX);
    a.xfrm_ext = ext;
    let place = to_image(sh, a, Arc::from(&[][..]), ImageFormat::Png)?;
    let (mut w, mut h) = (place.width, place.height);
    // Lines may have no width or height (pictures can't).
    if let (true, Some((ew, eh))) = (shape.kind.is_line(), ext) {
        if ew < 1.0 && w <= 1.0 {
            w = ew;
        }
        if eh < 1.0 && h <= 1.0 {
            h = eh;
        }
    }
    let mut s = SheetShape::new(shape, place.from, w, h);
    (s.dx, s.dy, s.anchor) = (place.dx, place.dy, place.anchor);
    s.sanitize();
    Some(s)
}

fn to_image(sh: &Sheet, a: &Raw, data: Arc<[u8]>, format: ImageFormat) -> Option<SheetImage> {
    let mut img = SheetImage::new(data, format, CellRef::new(0, 0), 1.0, 1.0);
    let (w, h) = match a.kind {
        "abs" => {
            let (x, y) = a.pos.unwrap_or((0.0, 0.0));
            img.set_pos(sh, x * SX, y * SY);
            img.anchor = Anchor::Absolute;
            a.ext.or(a.xfrm_ext)?
        }
        _ => {
            let from = a.from.as_ref()?;
            img.from = CellRef::new(from.row, from.col);
            img.dx = from.col_off * SX;
            img.dy = from.row_off * SY;
            let span = a.to.as_ref().map(|to| {
                let w = excel_x(sh, to.col, to.col_off) - excel_x(sh, from.col, from.col_off);
                let h = excel_y(sh, to.row, to.row_off) - excel_y(sh, from.row, from.row_off);
                (w, h)
            });
            if let (Some((aw, ah)), true, "two") = (span, swapped(a.rot), a.kind) {
                // The anchor holds the box turned a quarter: same center,
                // sides swapped.
                let (w, h) = a.xfrm_ext.unwrap_or((ah, aw));
                let (cx, cy) = (excel_x(sh, from.col, from.col_off) + aw / 2.0, excel_y(sh, from.row, from.row_off) + ah / 2.0);
                img.set_pos(sh, (cx - w / 2.0) * SX, (cy - h / 2.0) * SY);
                img.anchor = if a.edit_as == "oneCell" { Anchor::OneCell } else { Anchor::TwoCell };
                return finish(img, a, w, h);
            }
            img.anchor = match (a.kind, a.edit_as.as_str()) {
                ("one", _) | (_, "oneCell") => Anchor::OneCell,
                (_, "absolute") => Anchor::Absolute,
                _ => Anchor::TwoCell,
            };
            match a.kind {
                "one" => a.ext.or(a.xfrm_ext)?,
                _ => span.filter(|(w, h)| *w > 0.0 && *h > 0.0).or(a.xfrm_ext)?,
            }
        }
    };
    finish(img, a, w, h)
}

/// A cell corner: column, row and offsets in Excel pixels.
pub(crate) type Corner = (u32, u32, f32, f32);

/// A box between two cell corners (`one_cell`: it moves but doesn't size
/// with the cells), placed like a picture.
pub(crate) fn place_box(sh: &Sheet, from: Corner, to: Corner, one_cell: bool) -> Option<SheetImage> {
    let marker = |c: Corner| Marker { col: c.0.min(MAX_COLS - 1), row: c.1.min(MAX_ROWS - 1), col_off: c.2, row_off: c.3 };
    let a = Raw { kind: "two", edit_as: if one_cell { "oneCell".into() } else { String::new() }, from: Some(marker(from)), to: Some(marker(to)), ..Default::default() };
    to_image(sh, &a, Arc::from(&[][..]), ImageFormat::Png)
}

/// The cell corners of a box placed like a picture.
pub(crate) fn box_corners(sh: &Sheet, from: CellRef, dx: f32, dy: f32, w: f32, h: f32) -> [Corner; 2] {
    let (ex, ey) = ((sh.col_x(from.col) + dx) / SX + w, (sh.row_y(from.row) + dy) / SY + h);
    let col = sh.col_at((ex * SX).max(0.0)).min(MAX_COLS - 1);
    let row = sh.row_at((ey * SY).max(0.0)).min(MAX_ROWS - 1);
    [(from.col, from.row, dx / SX, dy / SY), (col, row, (ex - excel_x(sh, col, 0.0)).max(0.0), (ey - excel_y(sh, row, 0.0)).max(0.0))]
}

/// A cell corner as `xdr:col`, `xdr:colOff`, `xdr:row` and `xdr:rowOff`.
pub(crate) fn corner_xml(c: Corner) -> String {
    format!("<xdr:col>{}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>{}</xdr:rowOff>", c.0, to_emu(c.2), c.1, to_emu(c.3))
}

fn finish(mut img: SheetImage, a: &Raw, w: f32, h: f32) -> Option<SheetImage> {
    img.width = w;
    img.height = h;
    img.name = a.name.clone().unwrap_or_default();
    img.alt = a.descr.clone();
    img.crop = a.crop;
    img.rotation = a.rot;
    img.lock_aspect = a.lock;
    img.sanitize();
    Some(img)
}

/// Media parts of the workbook being written, shared between sheets.
#[derive(Default)]
pub struct Media {
    /// Picture bytes -> part name ("imageN.ext").
    by_data: HashMap<Arc<[u8]>, String>,
    pub files: Vec<(String, Arc<[u8]>)>,
    /// Extensions used, with their content type.
    pub types: Vec<(&'static str, &'static str)>,
    /// Other parts (charts): path in the package, content and content type.
    pub parts: Vec<(String, String, &'static str)>,
    /// Ranges of newer charts, saved as `_xlchart.v1.N` names.
    pub chart_names: Vec<String>,
    /// Embedded objects and diagrams.
    pub objects: crate::xlsx_ole::Parts,
}

impl Media {
    pub(crate) fn add(&mut self, img: &SheetImage) -> String {
        self.add_data(&img.data, img.format)
    }

    /// Adds picture bytes as `xl/media/imageN.ext` (once); returns the
    /// file name.
    pub fn add_data(&mut self, data: &Arc<[u8]>, format: ImageFormat) -> String {
        if let Some(n) = self.by_data.get(data) {
            return n.clone();
        }
        let ext = format.ext();
        let name = format!("image{}.{ext}", self.files.len() + 1);
        if !self.types.iter().any(|(e, _)| *e == ext) {
            self.types.push((ext, format.mime()));
        }
        self.files.push((name.clone(), data.clone()));
        self.by_data.insert(data.clone(), name.clone());
        name
    }

    /// Adds `xl/charts/<prefix>N.xml`, N unique in the workbook; returns
    /// its file name.
    fn add_part(&mut self, prefix: &str, xml: String, ty: &'static str) -> String {
        let n = self.parts.iter().filter(|(_, _, t)| *t == ty).count() + 1;
        let name = format!("{prefix}{n}.xml");
        self.parts.push((format!("xl/charts/{name}"), xml, ty));
        name
    }
}

/// Opening and closing tags of the anchor of a box placed like a picture.
#[allow(clippy::too_many_arguments)]
fn anchor_tags(sh: &Sheet, from: CellRef, dx: f32, dy: f32, w: f32, h: f32, anchor: Anchor, rotation: f32) -> (String, &'static str) {
    let (px, py) = (sh.col_x(from.col) + dx, sh.row_y(from.row) + dy);
    // Position in Excel pixels.
    let (ex, ey) = (px / SX, py / SY);
    let marker = |tag: &str, x: f32, y: f32| -> String {
        // `x`, `y` in Excel pixels; the cell is found in our geometry.
        let col = sh.col_at((x * SX).max(0.0)).min(MAX_COLS - 1);
        let row = sh.row_at((y * SY).max(0.0)).min(MAX_ROWS - 1);
        let (co, ro) = ((x - excel_x(sh, col, 0.0)).max(0.0), (y - excel_y(sh, row, 0.0)).max(0.0));
        format!("<xdr:{tag}><xdr:col>{col}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{row}</xdr:row><xdr:rowOff>{}</xdr:rowOff></xdr:{tag}>", to_emu(co), to_emu(ro))
    };
    match anchor {
        Anchor::Absolute => (format!(r#"<xdr:absoluteAnchor><xdr:pos x="{}" y="{}"/><xdr:ext cx="{}" cy="{}"/>"#, to_emu(ex), to_emu(ey), to_emu(w), to_emu(h)), "</xdr:absoluteAnchor>"),
        a => {
            let edit = if a == Anchor::OneCell { r#" editAs="oneCell""# } else { "" };
            let (from, to) = if swapped(rotation) {
                let (cx, cy) = (ex + w / 2.0, ey + h / 2.0);
                let (hw, hh) = (h / 2.0, w / 2.0);
                (marker("from", cx - hw, cy - hh), marker("to", cx + hw, cy + hh))
            } else {
                let from = format!(
                    "<xdr:from><xdr:col>{}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>{}</xdr:rowOff></xdr:from>",
                    from.col,
                    to_emu(dx / SX),
                    from.row,
                    to_emu(dy / SY)
                );
                (from, marker("to", ex + w, ey + h))
            };
            (format!("<xdr:twoCellAnchor{edit}>{from}{to}"), "</xdr:twoCellAnchor>")
        }
    }
}

const REL_CHART: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";
const REL_CHARTEX: &str = "http://schemas.microsoft.com/office/2014/relationships/chartEx";
const CT_CHART: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
const CT_CHARTEX: &str = "application/vnd.ms-office.chartex+xml";

/// The drawing part of a sheet and its relationships, or None without
/// pictures or charts. Chart parts go into `media`.
pub fn write_drawing(wb: &Workbook, sheet: usize, media: &mut Media) -> Option<(String, String)> {
    let sh = &wb.sheets[sheet];
    if sh.images.is_empty() && sh.charts.is_empty() && sh.shapes.iter().all(|s| s.control.is_some()) {
        return None;
    }
    let mut x = String::from(HEADER);
    x.push_str(r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#);
    let mut rels = String::from(HEADER);
    rels.push_str(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    let mut rids: HashMap<String, usize> = HashMap::new();
    for (i, img) in sh.images.iter().enumerate() {
        if img.object.is_some() {
            let anchor = if img.anchor == Anchor::OneCell { Anchor::OneCell } else { Anchor::TwoCell };
            let (open, close) = anchor_tags(sh, img.from, img.dx, img.dy, img.width, img.height, anchor, 0.0);
            crate::xlsx_ole::drawing_object(&mut x, &mut rels, (&open, close), sh, i, media);
            continue;
        }
        let part = media.add(img);
        let n = rids.len() + 1;
        let rid = *rids.entry(part.clone()).or_insert_with(|| {
            let _ = write!(rels, r#"<Relationship Id="rId{n}" Type="{REL_IMAGE}" Target="../media/{part}"/>"#);
            n
        });
        let (px, py) = img.pos(sh);
        let (ex, ey) = (px / SX, py / SY);
        let (cx, cy) = (to_emu(img.width), to_emu(img.height));
        let (open, close) = anchor_tags(sh, img.from, img.dx, img.dy, img.width, img.height, img.anchor, img.rotation);
        x.push_str(&open);
        let _ = write!(x, r#"<xdr:pic><xdr:nvPicPr><xdr:cNvPr id="{}" name="{}""#, i + 2, esc(&img.name));
        if !img.alt.is_empty() {
            let _ = write!(x, r#" descr="{}""#, esc(&img.alt));
        }
        x.push_str("/><xdr:cNvPicPr>");
        if img.lock_aspect {
            x.push_str(r#"<a:picLocks noChangeAspect="1"/>"#);
        }
        let _ = write!(x, r#"</xdr:cNvPicPr></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="rId{rid}"/>"#);
        if img.crop.iter().any(|c| *c > 0.0) {
            x.push_str("<a:srcRect");
            for (k, v) in ["l", "t", "r", "b"].iter().zip(img.crop) {
                if v > 0.0 {
                    let _ = write!(x, r#" {k}="{}""#, (v * 100_000.0).round() as i64);
                }
            }
            x.push_str("/>");
        }
        x.push_str("<a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr><a:xfrm");
        let rot = (img.rotation * 60_000.0).round() as i64;
        if rot != 0 {
            let _ = write!(x, r#" rot="{rot}""#);
        }
        let _ = write!(
            x,
            r#"><a:off x="{}" y="{}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic><xdr:clientData/>"#,
            to_emu(ex),
            to_emu(ey)
        );
        x.push_str(close);
    }
    let mut n = rids.len();
    for (i, chart) in sh.charts.iter().enumerate() {
        let id = sh.images.len() + i + 2;
        let (open, close) = anchor_tags(sh, chart.from, chart.dx, chart.dy, chart.width, chart.height, chart.anchor, 0.0);
        x.push_str(&open);
        let frame = |rid: usize, uri: &str, tag: &str| {
            format!(
                r#"<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{id}" name="{}"{}/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="{uri}">{tag} r:id="rId{rid}"/></a:graphicData></a:graphic></xdr:graphicFrame>"#,
                esc(&chart.name),
                if chart.alt.is_empty() { String::new() } else { format!(r#" descr="{}""#, esc(&chart.alt)) }
            )
        };
        const URI_C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
        const TAG_C: &str = r#"<c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart""#;
        // Plain charts, and what newer ones fall back to.
        let plain = crate::xlsx_chart::write_chart(wb, sheet, chart);
        let part = media.add_part("chart", plain, CT_CHART);
        n += 1;
        let _ = write!(rels, r#"<Relationship Id="rId{n}" Type="{REL_CHART}" Target="../charts/{part}"/>"#);
        match crate::xlsx_chart::write_chartex(wb, sheet, chart, &mut media.chart_names) {
            Some(ex) => {
                let part = media.add_part("chartEx", ex, CT_CHARTEX);
                let _ = write!(rels, r#"<Relationship Id="rId{}" Type="{REL_CHARTEX}" Target="../charts/{part}"/>"#, n + 1);
                let (p, ns) = crate::xlsx_chart::CHARTEX_REQUIRES;
                let _ = write!(
                    x,
                    r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:{p}="{ns}" Requires="{p}">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
                    frame(n + 1, "http://schemas.microsoft.com/office/drawing/2014/chartex", r#"<cx:chart xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex""#),
                    frame(n, URI_C, TAG_C)
                );
                n += 1;
            }
            None => x.push_str(&frame(n, URI_C, TAG_C)),
        }
        x.push_str("<xdr:clientData/>");
        x.push_str(close);
    }
    for (i, s) in sh.shapes.iter().filter(|s| s.control.is_none()).enumerate() {
        let id = sh.images.len() + sh.charts.len() + i + 2;
        let (open, close) = anchor_tags(sh, s.from, s.dx, s.dy, s.width, s.height, s.anchor, s.shape.rotation);
        if let Some(ink) = &s.shape.ink {
            n += 1;
            let k = media.parts.iter().filter(|(_, _, t)| *t == crate::inkml::CONTENT_TYPE).count() + 1;
            media.parts.push((format!("xl/ink/ink{k}.xml"), crate::inkml::write(ink, s.width * 0.75, s.height * 0.75), crate::inkml::CONTENT_TYPE));
            let _ = write!(rels, r#"<Relationship Id="rId{n}" Type="{}" Target="../ink/ink{k}.xml"/>"#, crate::inkml::REL_TYPE);
            write_ink(&mut x, sh, s, ink, id, n, (&open, close));
            continue;
        }
        x.push_str(&open);
        let _ = write!(x, r#"<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="{id}" name="{}""#, esc(&s.shape.name));
        if !s.shape.alt.is_empty() {
            let _ = write!(x, r#" descr="{}""#, esc(&s.shape.alt));
        }
        let _ = write!(x, "/><xdr:cNvSpPr{}/></xdr:nvSpPr><xdr:spPr>", if s.shape.is_text_box() { r#" txBox="1""# } else { "" });
        let (px, py) = s.pos(sh);
        drawingml::write_sp_pr(&mut x, &s.shape, (to_emu(px / SX), to_emu(py / SY)), (to_emu(s.width), to_emu(s.height)));
        x.push_str("</xdr:spPr>");
        drawingml::write_style(&mut x, "xdr:style", &s.shape);
        if s.shape.can_have_text() {
            let _ = write!(
                x,
                r#"<xdr:txBody><a:bodyPr vertOverflow="clip" horzOverflow="clip" wrap="square" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0" anchor="{}"/><a:lstStyle/>"#,
                drawingml::anchor_value(s.shape.valign)
            );
            drawingml::write_paras(&mut x, &s.shape);
            x.push_str("</xdr:txBody>");
        }
        x.push_str("</xdr:sp><xdr:clientData/>");
        x.push_str(close);
    }
    x.push_str("</xdr:wsDr>");
    rels.push_str("</Relationships>");
    Some((x, rels))
}

/// Ink as Excel writes it: the content part in an anchor for Excel 2010
/// and later, freeform shapes in the same anchor for other programs.
fn write_ink(x: &mut String, sh: &Sheet, s: &SheetShape, ink: &apex_doc::Ink, id: usize, rid: usize, (open, close): (&str, &str)) {
    let name = if s.shape.name.is_empty() || s.shape.name == "Ink" { format!("Ink {id}") } else { s.shape.name.clone() };
    let descr = if s.shape.alt.is_empty() { String::new() } else { format!(r#" descr="{}""#, esc(&s.shape.alt)) };
    let (px, py) = s.pos(sh);
    let (off, ext) = ((to_emu(px / SX), to_emu(py / SY)), (to_emu(s.width), to_emu(s.height)));
    let rot = (s.shape.rotation * 60_000.0).round() as i64;
    let mut xf = if rot != 0 { format!(r#" rot="{rot}""#) } else { String::new() };
    xf.push_str(if s.shape.flip_h { r#" flipH="1""# } else { "" });
    xf.push_str(if s.shape.flip_v { r#" flipV="1""# } else { "" });
    let _ = write!(
        x,
        r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:xdr14="http://schemas.microsoft.com/office/excel/2010/spreadsheetDrawing" Requires="xdr14">{open}<xdr14:contentPart r:id="rId{rid}"><xdr14:nvContentPartPr><xdr14:cNvPr id="{id}" name="{}"{descr}/><xdr14:cNvContentPartPr/></xdr14:nvContentPartPr><xdr14:nvPr macro=""/><xdr14:xfrm{xf}><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></xdr14:xfrm></xdr14:contentPart><xdr:clientData/>{close}</mc:Choice><mc:Fallback>{open}"#,
        esc(&name),
        off.0,
        off.1,
        ext.0,
        ext.1
    );
    let looks = crate::inkml::looks(ink);
    let sp = |x: &mut String, strokes: &[&apex_doc::InkStroke], id: usize, off: (i64, i64)| {
        let _ = write!(x, r#"<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="{id}" name="{}"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr>"#, esc(&name));
        let mut p = String::new();
        crate::inkml::write_sp_pr(&mut p, strokes, off, ext);
        if !xf.is_empty() {
            p = p.replacen("<a:xfrm>", &format!("<a:xfrm{xf}>"), 1);
        }
        x.push_str(&p);
        x.push_str("</xdr:spPr></xdr:sp>");
    };
    if looks.len() > 1 {
        let _ = write!(
            x,
            r#"<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="{id}" name="{}"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/><a:chOff x="{}" y="{}"/><a:chExt cx="{}" cy="{}"/></a:xfrm></xdr:grpSpPr>"#,
            esc(&name),
            off.0,
            off.1,
            ext.0,
            ext.1,
            off.0,
            off.1,
            ext.0,
            ext.1
        );
        for g in &looks {
            sp(x, g, id, off);
        }
        x.push_str("</xdr:grpSp>");
    } else {
        sp(x, looks.first().map_or(&[][..], |g| &g[..]), id, off);
    }
    let _ = write!(x, "<xdr:clientData/>{close}</mc:Fallback></mc:AlternateContent>");
}

#[cfg(test)]
pub(crate) mod tests {
    use apex_sheet::{Anchor, CellRef, ImageFormat, SheetImage, Workbook};
    use std::sync::Arc;

    pub(crate) fn png() -> Arc<[u8]> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x04\0\0\0\x03\x08\x06\0\0\0".to_vec();
        v.extend_from_slice(&[7; 40]);
        Arc::from(v)
    }

    pub(crate) fn sample() -> Workbook {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "x");
        wb.sheets[0].col_widths.insert(2, 137.5);
        let data = png();
        let mut a = SheetImage::new(data.clone(), ImageFormat::Png, CellRef::new(1, 1), 120.0, 80.0);
        a.dx = 11.0;
        a.dy = 5.5;
        a.alt = "A \"red\" <box>".into();
        a.name = "Logo".into();
        a.rotation = 30.0;
        a.crop = [0.1, 0.0, 0.25, 0.05];
        wb.add_image(0, a);
        let mut b = SheetImage::new(data, ImageFormat::Png, CellRef::new(10, 3), 64.0, 32.0);
        b.anchor = Anchor::TwoCell;
        b.lock_aspect = false;
        b.rotation = 90.0;
        b.dx = 3.0;
        wb.add_image(0, b);
        let mut c = SheetImage::new(Arc::from(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3][..]), ImageFormat::Jpeg, CellRef::new(0, 0), 30.0, 30.0);
        c.anchor = Anchor::Absolute;
        c.dx = 40.0;
        c.dy = 12.0;
        wb.add_image(0, c);
        wb
    }

    pub(crate) fn check(back: &Workbook, exact_anchor: bool) {
        let (a, b) = (&wb_images(back)[0], &wb_images(back)[1]);
        let orig = sample();
        let (oa, ob) = (&orig.sheets[0].images[0], &orig.sheets[0].images[1]);
        assert_eq!(a.data, oa.data, "bytes kept");
        assert_eq!(a.format, ImageFormat::Png);
        assert_eq!((a.from, a.anchor), (oa.from, Anchor::OneCell));
        let close = |x: f32, y: f32, tol: f32| assert!((x - y).abs() <= tol, "{x} vs {y}");
        let tol = if exact_anchor { 0.01 } else { 0.6 };
        close(a.dx, oa.dx, tol);
        close(a.dy, oa.dy, tol);
        close(a.width, oa.width, tol);
        close(a.height, oa.height, tol);
        close(a.rotation, 30.0, 0.01);
        for i in 0..4 {
            close(a.crop[i], oa.crop[i], if exact_anchor { 0.001 } else { 0.01 });
        }
        assert_eq!((a.name.as_str(), a.alt.as_str()), ("Logo", "A \"red\" <box>"));
        // ODF has no "lock aspect ratio".
        assert!(!exact_anchor || a.lock_aspect && !b.lock_aspect);
        assert_eq!((b.from, b.anchor), (ob.from, Anchor::TwoCell));
        close(b.width, 64.0, tol);
        close(b.height, 32.0, tol);
        close(b.rotation, 90.0, 0.01);
        let (bp, obp) = (b.pos(&back.sheets[0]), ob.pos(&orig.sheets[0]));
        close(bp.0, obp.0, tol);
        close(bp.1, obp.1, tol);
        let c = &wb_images(back)[2];
        assert_eq!((c.format, c.anchor), (ImageFormat::Jpeg, Anchor::Absolute));
        let (x, y) = c.pos(&back.sheets[0]);
        close(x, 40.0, tol);
        close(y, 12.0, tol);
        assert_eq!(c.data.len(), 7);
    }

    fn wb_images(wb: &Workbook) -> &[SheetImage] {
        &wb.sheets[0].images
    }

    #[test]
    fn xlsx_roundtrip() {
        let bytes = crate::xlsx::write(&sample());
        let back = crate::xlsx::read(&bytes).unwrap();
        check(&back, true);
        // One media part for the shared picture.
        let z = crate::zip::ZipReader::new(&bytes).unwrap();
        let media: Vec<&str> = z.names().filter(|n| n.starts_with("xl/media/")).collect();
        assert_eq!(media.len(), 2, "{media:?}");
        let ct = z.read_string("[Content_Types].xml").unwrap();
        assert!(ct.contains(r#"Extension="png""#) && ct.contains("drawing+xml"));
        let sheet = z.read_string("xl/worksheets/sheet1.xml").unwrap();
        assert!(sheet.contains("<drawing r:id="));
        // Saved again: same pictures.
        let again = crate::xlsx::read(&crate::xlsx::write(&back)).unwrap();
        check(&again, true);
    }

    #[test]
    fn hostile_drawings() {
        // Broken markers, missing media and junk values don't panic.
        let mut wb = sample();
        wb.sheets[0].images[0].width = f32::NAN;
        let bytes = crate::xlsx::write(&wb);
        let back = crate::xlsx::read(&bytes).unwrap();
        assert!(back.sheets[0].images[0].width >= 1.0);
        let z = crate::zip::ZipReader::new(&bytes).unwrap();
        let mut w = crate::zip::ZipWriter::new();
        for n in z.names() {
            let mut data = z.read(n).unwrap();
            if n.ends_with("drawing1.xml") {
                data = String::from_utf8(data).unwrap().replace("<xdr:col>1</xdr:col>", "<xdr:col>-9</xdr:col>").replace("rot=", "rot=\"x\" a=").into_bytes();
            }
            if n.starts_with("xl/media/image2") {
                continue;
            }
            w.add(n, &data, true);
        }
        let back = crate::xlsx::read(&w.finish()).unwrap();
        assert_eq!(back.sheets[0].images.len(), 2);
    }
}
