//! Pictures in .ods: `draw:frame` with a `draw:image` in `Pictures/`,
//! anchored to a cell (inside the `table:table-cell`) or to the sheet
//! (inside `table:shapes`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_sheet::{Anchor, CellRef, ImageFormat, MAX_COLS, MAX_ROWS, Sheet, SheetImage};

use crate::parse_length_pt;
use crate::xml::{Ev, Reader, Tag, esc};
use crate::zip::ZipReader;

/// Sheet pixels per LibreOffice pixel (see the column and row scales).
pub(super) const SX: f32 = super::COL_PX_PER_PT * 0.75;
pub(super) const SY: f32 = super::ROW_PX_PER_PT * 0.75;
const MAX_PICTURES: usize = 10_000;

/// A frame as read, resolved once the sheet's geometry is known.
pub(super) struct RawFrame {
    sheet: usize,
    /// Cell holding the frame (None: anchored to the sheet).
    cell: Option<CellRef>,
    name: String,
    desc: String,
    title: String,
    href: String,
    /// Embedded object (a chart): its folder.
    object: String,
    style: String,
    /// Lengths in pixels.
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// Counterclockwise radians and the translation, from draw:transform.
    rotate: f32,
    translate: Option<(f32, f32)>,
    resize: bool,
    /// Stacking order.
    z: u32,
}

fn px(v: Option<std::borrow::Cow<str>>) -> f32 {
    v.and_then(|v| parse_length_pt(&v)).filter(|v| v.is_finite()).map_or(0.0, |pt| pt / 0.75)
}

/// "rotate (-0.52) translate (0.9cm 0.2cm)": the angle and translation.
fn parse_transform(s: &str) -> (f32, Option<(f32, f32)>) {
    let (mut rot, mut tr) = (0.0, None);
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let Some(close) = rest[open..].find(')') else { break };
        let args: Vec<&str> = rest[open + 1..open + close].split([' ', ',']).filter(|a| !a.is_empty()).collect();
        match name {
            "rotate" => rot += args.first().and_then(|a| a.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0),
            "translate" => {
                let len = |i: usize| args.get(i).and_then(|a| parse_length_pt(a)).filter(|v| v.is_finite()).map_or(0.0, |pt| pt / 0.75);
                tr = Some((len(0), len(1)));
            }
            _ => {}
        }
        rest = &rest[open + close + 1..];
    }
    (rot, tr)
}

/// Reads a `draw:frame` whose start was just returned; None unless it
/// holds an embedded picture or chart (other objects are skipped).
pub(super) fn read_frame(r: &mut Reader, t: &Tag, sheet: usize, cell: Option<CellRef>) -> Option<RawFrame> {
    let (rotate, translate) = t.attr("draw:transform").map_or((0.0, None), |v| parse_transform(&v));
    let mut f = RawFrame {
        sheet,
        cell,
        name: t.attr("draw:name").unwrap_or_default().into_owned(),
        desc: String::new(),
        title: String::new(),
        href: String::new(),
        object: String::new(),
        style: t.attr("draw:style-name").unwrap_or_default().into_owned(),
        x: px(t.attr("svg:x")),
        y: px(t.attr("svg:y")),
        w: px(t.attr("svg:width")),
        h: px(t.attr("svg:height")),
        rotate,
        translate,
        resize: t.attr("table:end-cell-address").is_some(),
        z: t.attr("draw:z-index").and_then(|v| v.trim().parse().ok()).unwrap_or(u32::MAX),
    };
    let mut depth = 1;
    loop {
        let ev = r.next();
        let empty = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof => break,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ev::Start(x) | Ev::Empty(x) => {
                match x.name {
                    "draw:image" if f.href.is_empty() => f.href = x.attr("xlink:href").unwrap_or_default().into_owned(),
                    "draw:object" if f.object.is_empty() => f.object = x.attr("xlink:href").unwrap_or_default().into_owned(),
                    "draw:object-ole" if f.object.is_empty() => f.object = x.attr("xlink:href").unwrap_or_default().into_owned(),
                    "draw:plugin" => f.object = "-".into(),
                    "svg:desc" if !empty && depth == 1 => {
                        f.desc = r.text();
                        continue;
                    }
                    "svg:title" if !empty && depth == 1 => {
                        f.title = r.text();
                        continue;
                    }
                    _ => {}
                }
                if !empty {
                    depth += 1;
                }
            }
            _ => {}
        }
    }
    (!f.object.is_empty() && f.object != "-" || f.object.is_empty() && !f.href.is_empty()).then_some(f)
}

/// `fo:clip` of the graphic styles in a styles part: name -> clip.
pub(super) fn graphic_clips(xml: &str, out: &mut HashMap<String, String>) {
    let mut r = Reader::new(xml);
    let mut cur: Option<String> = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.name == "style:style" => cur = (t.attr("style:family").is_some_and(|f| f == "graphic")).then(|| t.attr("style:name").unwrap_or_default().into_owned()),
            Ev::Start(t) | Ev::Empty(t) if t.name == "style:graphic-properties" => {
                if let (Some(n), Some(c)) = (&cur, t.attr("fo:clip")) {
                    out.insert(n.clone(), c.into_owned());
                }
            }
            Ev::End("style:style") => cur = None,
            _ => {}
        }
    }
}

/// Crop fractions (l, t, r, b) from "rect(top, right, bottom, left)",
/// whose lengths are on the picture's natural size.
fn parse_clip(clip: &str, data: &[u8]) -> [f32; 4] {
    let Some((w, h)) = apex_sheet::image::natural_size(data) else { return [0.0; 4] };
    let inner = clip.trim().strip_prefix("rect(").and_then(|s| s.strip_suffix(')')).unwrap_or("");
    let v: Vec<f32> = inner.split([',', ' ']).filter(|s| !s.is_empty()).map(|s| parse_length_pt(s).map_or(0.0, |pt| pt / 0.75)).collect();
    if v.len() != 4 {
        return [0.0; 4];
    }
    [v[3] / w, v[0] / h, v[1] / w, v[2] / h]
}

/// Turns the frames read into pictures of their sheets.
pub(super) fn resolve(z: &ZipReader, mut frames: Vec<RawFrame>, clips: &HashMap<String, String>, sheets: &mut [Sheet]) {
    frames.sort_by_key(|f| (f.sheet, f.z));
    let mut media: HashMap<String, Option<(Arc<[u8]>, ImageFormat)>> = HashMap::new();
    let mut objects = None;
    for f in frames {
        let Some(sh) = sheets.get_mut(f.sheet) else { continue };
        let mut object = None;
        if !f.object.is_empty() {
            if let Some(mut c) = super::chart::read_object(z, &f.object, &f.desc) {
                // Our settings take the description, so ours keep the
                // alternative text in the title; other programs' in either.
                c.alt = if !f.title.is_empty() || super::chart::is_ours(&f.desc) { f.title } else { f.desc };
                super::chart::place(sh, c, f.name, f.cell, f.resize, f.x, f.y, f.w, f.h);
                continue;
            }
            // Another embedded object, shown by its replacement picture.
            let files = objects.get_or_insert_with(|| {
                let mut m = HashMap::new();
                crate::odf_ole::load(z, &mut m);
                m
            });
            let Some(o) = crate::odf_ole::object(files, &f.object) else { continue };
            object = Some(Arc::new(o));
        }
        if sh.images.len() >= MAX_PICTURES {
            continue;
        }
        let path = f.href.trim_start_matches("./").to_string();
        if path.contains("://") || path.starts_with('/') || path.starts_with("..") {
            continue;
        }
        let entry = media.entry(path.clone()).or_insert_with(|| {
            let data = z.read(&path).ok()?;
            let fmt = ImageFormat::detect(&data, path.rsplit_once('.').map_or("", |x| x.1))?;
            Some((Arc::from(data), fmt))
        });
        let Some((data, format)) = entry.clone().or_else(|| object.is_some().then(|| (Arc::from(&[][..]), ImageFormat::Png))) else { continue };
        let crop = clips.get(&f.style).map_or([0.0; 4], |c| parse_clip(c, &data));
        let mut img = SheetImage::new(data, format, CellRef::new(0, 0), f.w, f.h);
        img.name = f.name;
        img.alt = f.desc;
        img.object = object;
        img.crop = crop;
        img.rotation = -f.rotate.to_degrees();
        img.anchor = match (f.cell, f.resize) {
            (None, _) => Anchor::Absolute,
            (Some(_), true) => Anchor::TwoCell,
            (Some(_), false) => Anchor::OneCell,
        };
        // Origin of the frame's coordinates, in LibreOffice pixels.
        let (ox, oy) = f.cell.map_or((0.0, 0.0), |c| (sh.col_x(c.col) / SX, sh.row_y(c.row) / SY));
        let (x, y) = match f.translate {
            Some((tx, ty)) => {
                // The translation places the turned top-left corner.
                let (s, c) = (-f.rotate).sin_cos();
                let (hw, hh) = (f.w / 2.0, f.h / 2.0);
                let (cx, cy) = (ox + tx + hw * c - hh * s, oy + ty + hw * s + hh * c);
                (cx - hw, cy - hh)
            }
            None => (ox + f.x, oy + f.y),
        };
        img.set_pos(sh, x * SX, y * SY);
        if f.cell.is_some() && f.translate.is_none() && img.rotation == 0.0 {
            // Keep the cell LibreOffice chose (it may hold an offset larger
            // than the cell).
            if let Some(c) = f.cell {
                img.from = c;
                img.dx = f.x * SX;
                img.dy = f.y * SY;
            }
        }
        img.sanitize();
        sh.images.push(img);
    }
    for sh in sheets.iter_mut() {
        for (i, img) in sh.images.iter_mut().enumerate() {
            img.id = i as u32 + 1;
        }
    }
}

/// Pictures of the document being written, shared between sheets.
#[derive(Default)]
pub(super) struct Pictures {
    by_data: HashMap<Arc<[u8]>, String>,
    pub files: Vec<(String, Arc<[u8]>, &'static str)>,
    /// Graphic styles of the frames.
    pub styles: String,
    clips: HashMap<String, String>,
    /// Chart objects written ("Object N").
    pub objects: usize,
    /// Shapes (their styles).
    pub shapes: crate::odf_shape::Writer,
    /// Files of other embedded objects: path, bytes (none for folders), type.
    pub embedded: Vec<(String, Option<Arc<[u8]>>, String)>,
}

impl Pictures {
    fn path(&mut self, img: &SheetImage) -> String {
        self.add_data(&img.data, img.format)
    }

    /// Adds picture bytes as `Pictures/imageN.ext` (once); returns the path.
    pub(super) fn add_data(&mut self, data: &Arc<[u8]>, format: apex_sheet::ImageFormat) -> String {
        if let Some(p) = self.by_data.get(data) {
            return p.clone();
        }
        let p = format!("Pictures/image{}.{}", self.files.len() + 1, format.ext());
        self.files.push((p.clone(), data.clone(), format.mime()));
        self.by_data.insert(data.clone(), p.clone());
        p
    }

    fn style(&mut self, img: &SheetImage) -> String {
        let clip = match (img.crop.iter().any(|c| *c > 0.0), apex_sheet::image::natural_size(&img.data)) {
            (true, Some((w, h))) => {
                let [l, t, r, b] = img.crop;
                format!("rect({}, {}, {}, {})", len(t * h), len(r * w), len(b * h), len(l * w))
            }
            _ => String::new(),
        };
        let n = self.clips.len();
        self.clips
            .entry(clip.clone())
            .or_insert_with(|| {
                let name = format!("gr{}", n + 1);
                let _ = write!(self.styles, r#"<style:style style:name="{name}" style:family="graphic"><style:graphic-properties draw:stroke="none" draw:fill="none""#);
                if !clip.is_empty() {
                    let _ = write!(self.styles, r#" fo:clip="{clip}""#);
                }
                self.styles.push_str("/></style:style>");
                name
            })
            .clone()
    }
}

/// A length in pixels as ODF inches.
pub(super) fn len(px: f32) -> String {
    super::fmt_len(px * 0.75)
}

pub(super) fn cell_address(sheet: &str, c: CellRef) -> String {
    let plain = sheet.chars().all(|ch| ch.is_alphanumeric() || ch == '_');
    let name = if plain { sheet.to_string() } else { format!("'{}'", sheet.replace('\'', "''")) };
    format!("{name}.{}", c.to_a1())
}

/// The last row a picture or chart reaches down to. The table must have it:
/// LibreOffice sizes a box that ends in a cell from that cell, and rows past
/// the table's end have its own default height, not ours.
pub(super) fn bottom_row(sh: &Sheet) -> u32 {
    let images = sh.images.iter().map(|i| i.pos(sh).1 + i.height.max(i.width) * SY);
    let charts = sh.charts.iter().map(|c| c.pos(sh).1 + c.height * SY);
    images.chain(charts).filter(|y| y.is_finite()).map(|y| sh.row_at(y.max(0.0))).max().unwrap_or(0).max(super::shapes::bottom_row(sh))
}

/// Frames of a sheet: the ones anchored to the sheet (for `table:shapes`)
/// and, by cell, the ones anchored to cells.
pub(super) fn frames(wb: &apex_sheet::Workbook, si: usize, pics: &mut Pictures) -> (String, HashMap<(u32, u32), String>) {
    let sh = &wb.sheets[si];
    let mut shapes = String::new();
    let mut cells: HashMap<(u32, u32), String> = HashMap::new();
    for (z, img) in sh.images.iter().enumerate() {
        let object = img.object.as_deref();
        if let Some(d) = object.and_then(|o| o.diagram()) {
            // ODF has no diagrams: their drawing, as a group of shapes.
            let (x, y) = img.pos(sh);
            crate::odf_ole::write_diagram(&mut pics.shapes, &mut shapes, d, (x * 0.75, y * 0.75), (img.width * 0.75, img.height * 0.75), "", z as i64);
            continue;
        }
        let ole = object.and_then(|o| o.ole());
        let href = if ole.is_some() { String::new() } else { pics.path(img) };
        let style = pics.style(img);
        let (px_, py_) = img.pos(sh);
        // Top-left in LibreOffice pixels.
        let (lx, ly) = (px_ / SX, py_ / SY);
        let (w, h) = (img.width, img.height);
        let turned = img.rotation != 0.0;
        // The point the frame's coordinates place: the top-left corner, or
        // the turned one.
        let (tx, ty) = if turned {
            let (s, c) = img.rotation.to_radians().sin_cos();
            let (hw, hh) = (w / 2.0, h / 2.0);
            (lx + hw - (hw * c - hh * s), ly + hh - (hw * s + hh * c))
        } else {
            (lx, ly)
        };
        let cell = match img.anchor {
            Anchor::Absolute => None,
            _ if !turned => Some(img.from),
            _ => Some(CellRef::new(sh.row_at((ty * SY).max(0.0)).min(MAX_ROWS - 1), sh.col_at((tx * SX).max(0.0)).min(MAX_COLS - 1))),
        };
        let (ox, oy) = cell.map_or((0.0, 0.0), |c| (sh.col_x(c.col) / SX, sh.row_y(c.row) / SY));
        let mut x = String::from("<draw:frame");
        if let (Some(_), Anchor::TwoCell) = (cell, img.anchor) {
            // LibreOffice ends the box at the placed point plus the size.
            let (ex, ey) = ((tx + w) * SX, (ty + h) * SY);
            let end = CellRef::new(sh.row_at(ey.max(0.0)).min(MAX_ROWS - 1), sh.col_at(ex.max(0.0)).min(MAX_COLS - 1));
            let _ = write!(
                x,
                r#" table:end-cell-address="{}" table:end-x="{}" table:end-y="{}""#,
                esc(&cell_address(&sh.name, end)),
                len((ex - sh.col_x(end.col)).max(0.0) / SX),
                len((ey - sh.row_y(end.row)).max(0.0) / SY)
            );
        }
        let _ = write!(x, r#" draw:z-index="{z}" draw:name="{}" draw:style-name="{style}" svg:width="{}" svg:height="{}""#, esc(&img.name), len(w), len(h));
        if turned {
            let _ = write!(x, r#" draw:transform="rotate ({}) translate ({} {})""#, -img.rotation.to_radians(), len(tx - ox), len(ty - oy));
        } else if cell == Some(img.from) {
            let _ = write!(x, r#" svg:x="{}" svg:y="{}""#, len(img.dx / SX), len(img.dy / SY));
        } else {
            let _ = write!(x, r#" svg:x="{}" svg:y="{}""#, len(tx - ox), len(ty - oy));
        }
        match ole {
            Some(o) => {
                let n = pics.embedded.iter().filter(|e| e.0.starts_with("ObjectReplacements/")).count() + 1;
                x.push('>');
                x.push_str(&crate::odf_ole::write(o, &img.data, n, &mut pics.embedded));
            }
            None => {
                let _ = write!(x, r#"><draw:image xlink:href="{}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad" draw:mime-type="{}"><text:p/></draw:image>"#, esc(&href), img.format.mime());
            }
        }
        if !img.alt.is_empty() {
            let _ = write!(x, "<svg:desc>{}</svg:desc>", esc(&img.alt));
        }
        x.push_str("</draw:frame>");
        match cell {
            Some(c) => cells.entry((c.row, c.col)).or_default().push_str(&x),
            None => shapes.push_str(&x),
        }
    }
    for (z, c) in sh.charts.iter().enumerate() {
        let (cell, x) = super::chart::frame(wb, si, c, sh.images.len() + z, pics);
        match cell {
            Some(c) => cells.entry((c.row, c.col)).or_default().push_str(&x),
            None => shapes.push_str(&x),
        }
    }
    super::shapes::frames(sh, si, &mut pics.shapes, sh.images.len() + sh.charts.len(), &mut shapes, &mut cells);
    if !shapes.is_empty() {
        shapes = format!("<table:shapes>{shapes}</table:shapes>");
    }
    (shapes, cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms() {
        let (r, t) = parse_transform("rotate (-0.5235987755983) translate (0.928cm 0.241cm)");
        assert!((r + 0.5236).abs() < 1e-4);
        let (x, y) = t.unwrap();
        assert!((x - 0.928 / 2.54 * 96.0).abs() < 0.01 && (y - 0.241 / 2.54 * 96.0).abs() < 0.01);
        assert_eq!(parse_transform("junk(").0, 0.0);
        let (x, y) = parse_transform("rotate (x) translate (1cm)").1.unwrap();
        assert!((x - 1.0 / 2.54 * 96.0).abs() < 1e-3 && y == 0.0);
    }

    #[test]
    fn ods_roundtrip() {
        let wb = crate::xlsx_drawing::tests::sample();
        let bytes = crate::ods::write(&wb);
        let back = crate::ods::read(&bytes).unwrap();
        crate::xlsx_drawing::tests::check(&back, false);
        let z = ZipReader::new(&bytes).unwrap();
        let manifest = z.read_string("META-INF/manifest.xml").unwrap();
        assert!(manifest.contains("Pictures/image1.png"));
        let again = crate::ods::read(&crate::ods::write(&back)).unwrap();
        crate::xlsx_drawing::tests::check(&again, false);
    }
}
