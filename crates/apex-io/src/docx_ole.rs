//! Embedded objects in docx (`w:object` and `w:pict` holding an
//! `o:OLEObject`, drawn by a VML shape) and the SmartArt diagrams' parts.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Align, DocShape, EmbedKind, Embedded, HRel, Image, OleObject, Shape, ShapeWrap, VRel, Wrap};

use crate::drawingml::{Node, read_fragment};
use crate::xml::esc;
use crate::zip::{ZipReader, ZipWriter};

/// The objects a part relates to: embedded data and diagram parts.
#[derive(Default)]
pub(crate) struct Objects {
    /// Embedded data by relationship id: bytes, extension, media type.
    payloads: HashMap<String, (Arc<[u8]>, String, String)>,
    pub diagrams: crate::diagram::Parts,
    /// Charts by relationship id: the chart part and its workbook.
    charts: HashMap<String, (String, Option<Arc<[u8]>>)>,
    /// Chart objects already made, by relationship id: thousands of
    /// drawings showing one chart would each read its workbook again.
    built: std::cell::RefCell<HashMap<String, Option<Arc<[u8]>>>>,
}

/// Media types of a package: by extension and by part name.
pub(crate) fn content_types(z: &ZipReader) -> (HashMap<String, String>, HashMap<String, String>) {
    let (mut by_ext, mut by_part) = (HashMap::new(), HashMap::new());
    let Ok(xml) = z.read_string("[Content_Types].xml") else { return (by_ext, by_part) };
    let root = read_fragment(&xml);
    let Some(types) = root.find("Types") else { return (by_ext, by_part) };
    for k in &types.kids {
        match (k.local(), k.attr("ContentType")) {
            ("Default", Some(ct)) => {
                by_ext.insert(k.attr("Extension").unwrap_or_default().to_ascii_lowercase(), ct.to_string());
            }
            ("Override", Some(ct)) => {
                by_part.insert(k.attr("PartName").unwrap_or_default().trim_start_matches('/').to_string(), ct.to_string());
            }
            _ => {}
        }
    }
    (by_ext, by_part)
}

impl Objects {
    /// The objects of the part at `path`.
    pub(crate) fn load(z: &ZipReader, path: &str) -> Objects {
        let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
        let rels = z.read_string(&format!("{dir}/_rels/{file}.rels")).map(|s| crate::xlsx_notes::read_rels(&s)).unwrap_or_default();
        let mut o = Objects::default();
        let mut types = None;
        for (id, (ty, target, external)) in &rels {
            if !*external && ty.ends_with("/chart") {
                let p = crate::xlsx::resolve_target(dir, target);
                let Ok(xml) = z.read_string(&p) else { continue };
                let (cdir, cfile) = p.rsplit_once('/').unwrap_or(("", &p));
                let crels = z.read_string(&format!("{cdir}/_rels/{cfile}.rels")).map(|s| crate::xlsx_notes::read_rels(&s)).unwrap_or_default();
                let book = crels.values().find(|(t, _, ext)| !*ext && (t.ends_with("/package") || t.ends_with("/oleObject"))).and_then(|(_, t, _)| z.read(&crate::xlsx::resolve_target(cdir, t)).ok()).map(Arc::from);
                o.charts.insert(id.clone(), (xml, book));
                continue;
            }
            if *external || !(ty.ends_with("/oleObject") || ty.ends_with("/package")) {
                continue;
            }
            let p = crate::xlsx::resolve_target(dir, target);
            let Ok(data) = z.read(&p) else { continue };
            let (by_ext, by_part) = types.get_or_insert_with(|| content_types(z));
            let ext = p.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).filter(|e| (1..=8).contains(&e.len()) && e.bytes().all(|b| b.is_ascii_alphanumeric())).unwrap_or_default();
            let ct = by_part.get(&p).or_else(|| by_ext.get(&ext)).cloned().unwrap_or_default();
            o.payloads.insert(id.clone(), (Arc::from(data), ext, ct));
        }
        if rels.values().any(|(ty, _, _)| ty.contains("diagram")) {
            o.diagrams = crate::diagram::Parts::load(z, path);
        }
        o
    }

    /// The embedded object related as `rid`.
    pub(crate) fn ole(&self, rid: &str, prog_id: &str, icon: bool) -> Option<OleObject> {
        let (data, ext, ct) = self.payloads.get(rid)?;
        Some(OleObject { prog_id: prog_id.to_string(), data: data.clone(), ext: ext.clone(), content_type: ct.clone(), icon })
    }
}

/// What an object reads as: in the text flow, or floating.
pub(crate) enum Placed {
    Inline(Image),
    Float(DocShape),
}

/// Base of VML z-indexes in Word (negative: behind the text).
const Z_BASE: i64 = 251_658_240;

/// A length in a VML style in points.
fn vml_len(v: &str) -> Option<f32> {
    let v = v.trim();
    let end = v.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(v.len());
    let n: f32 = v[..end].parse().ok()?;
    let pt = match &v[end..] {
        "pt" => n,
        "in" => n * 72.0,
        "cm" => n * 72.0 / 2.54,
        "mm" => n * 72.0 / 25.4,
        "px" | "" => n * 0.75,
        _ => return None,
    };
    pt.is_finite().then_some(pt)
}

fn style_map(style: &str) -> HashMap<String, String> {
    style.split(';').filter_map(|d| d.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect()
}

/// Reads a `w:object` or a `w:pict` (its inside, from `raw_element`)
/// holding an embedded object. `orig`: the object's own size in twips
/// (`w:dxaOrig`, `w:dyaOrig`). `None` when it holds none (or only a link).
pub(crate) fn read(raw: &str, orig: (Option<f32>, Option<f32>), objs: &Objects, media: &HashMap<String, Arc<[u8]>>) -> Option<Placed> {
    let root = read_fragment(raw);
    let ole = root.find("OLEObject")?;
    if ole.attr("Type") == Some("Link") {
        return None;
    }
    let obj = objs.ole(ole.attr("r:id")?, ole.attr("ProgID").unwrap_or_default(), ole.attr("DrawAspect") == Some("Icon"))?;
    let shape = root.find("shape").or_else(|| root.find("rect"));
    let data = shape.and_then(|s| s.find("imagedata")).and_then(|i| i.attr("r:id")).and_then(|id| media.get(id)).cloned().unwrap_or_else(|| Arc::from(&[][..]));
    let st = style_map(shape.and_then(|s| s.attr("style")).unwrap_or_default());
    let len = |k: &str| st.get(k).and_then(|v| vml_len(v));
    let w = len("width").or(orig.0.map(|v| v / 20.0)).unwrap_or(72.0).max(1.0);
    let h = len("height").or(orig.1.map(|v| v / 20.0)).unwrap_or(72.0).max(1.0);
    // Our own stand-in for an object without a preview reads back as none.
    let data = if *data == *blank_png() { Arc::from(&[][..]) } else { data };
    let mut img = Image::new(data, w, h);
    img.alt = shape.and_then(|s| s.attr("alt")).unwrap_or_default().to_string();
    img.object = Some(Arc::new(Embedded::new(EmbedKind::Ole(obj))));
    Some(place_vml(img, &st, shape))
}

/// Reads a `w:pict` (its inside, from `raw_element`) holding a picture,
/// in the line or floating as its VML style says.
pub(crate) fn read_picture(raw: &str, media: &HashMap<String, Arc<[u8]>>) -> Option<Placed> {
    let img = crate::docx_img::read_pict(&mut crate::xml::Reader::new(raw), media)?;
    let root = read_fragment(raw);
    let shape = root.find("shape").or_else(|| root.find("rect"));
    let st = style_map(shape.and_then(|s| s.attr("style")).unwrap_or_default());
    Some(place_vml(img, &st, shape))
}

/// A VML picture placed by its style (`st`): in the line, or floating.
fn place_vml(mut img: Image, st: &HashMap<String, String>, shape: Option<&Node>) -> Placed {
    let (w, h) = (img.width, img.height);
    let len = |k: &str| st.get(k).and_then(|v| vml_len(v));
    if st.get("position").map(String::as_str) != Some("absolute") {
        return Placed::Inline(img);
    }
    let x = len("margin-left").or(len("left")).unwrap_or(0.0);
    let y = len("margin-top").or(len("top")).unwrap_or(0.0);
    let z = st.get("z-index").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let wrap = shape.and_then(|s| s.find("wrap")).and_then(|w| w.attr("type"));
    match wrap {
        Some("square" | "tight" | "through" | "topAndBottom") => {
            img.wrap = if wrap == Some("topAndBottom") { Wrap::TopBottom } else { Wrap::Square };
            img.offset_x = x.max(0.0);
            Placed::Inline(img)
        }
        _ => {
            let mut d = DocShape::new(Shape::picture(img), w, h);
            d.wrap = if z < 0 { ShapeWrap::Behind } else { ShapeWrap::InFront };
            (d.x, d.y) = (x, y);
            d.h_rel = match st.get("mso-position-horizontal-relative").map(String::as_str) {
                Some("page") => HRel::Page,
                _ => HRel::Margin,
            };
            d.v_rel = match st.get("mso-position-vertical-relative").map(String::as_str) {
                Some("page") => VRel::Page,
                Some("margin" | "top-margin-area") => VRel::Margin,
                _ => VRel::Paragraph,
            };
            d.h_align = match st.get("mso-position-horizontal").map(String::as_str) {
                Some("center") => Some(Align::Center),
                Some("right") => Some(Align::Right),
                Some("left") => Some(Align::Left),
                _ => None,
            };
            // Word counts from a large base, above or below the text.
            let z = if z >= Z_BASE / 2 { z - Z_BASE } else if z <= -Z_BASE / 2 { z + Z_BASE } else { z };
            d.z = z.clamp(-1_000_000, 1_000_000) as i32;
            d.sanitize();
            Placed::Float(d)
        }
    }
}

/// Reads a `w:drawing` holding a SmartArt diagram (its inside); the
/// picture is the fallback Office may store, else empty.
pub(crate) fn read_diagram(raw: &str, objs: &Objects, media: &HashMap<String, Arc<[u8]>>) -> Option<(Image, Node)> {
    let root = read_fragment(raw);
    let ext = root.find("extent")?;
    let size = (ext.num("cx")? as f32 / crate::drawingml::EMU_PT, ext.num("cy")? as f32 / crate::drawingml::EMU_PT);
    let d = objs.diagrams.diagram(&root, size)?;
    let data = root.find("blip").and_then(|b| b.attr("r:embed")).and_then(|id| media.get(id)).cloned().unwrap_or_else(|| Arc::from(&[][..]));
    let img = placed(&root, size, data, EmbedKind::Diagram(d));
    Some((img, root))
}

/// Reads a `w:drawing` holding a chart (its inside): the chart and its
/// data become a chart object.
pub(crate) fn read_chart(raw: &str, objs: &Objects) -> Option<(Image, Node)> {
    let root = read_fragment(raw);
    let ext = root.find("extent")?;
    let size = (ext.num("cx")? as f32 / crate::drawingml::EMU_PT, ext.num("cy")? as f32 / crate::drawingml::EMU_PT);
    let rid = root.find("graphicData")?.find("chart")?.attr("r:id")?;
    let (xml, book) = objs.charts.get(rid)?;
    let made = objs.built.borrow().get(rid).cloned();
    let book = match made {
        Some(b) => b?,
        None => {
            let b = crate::chart_object::from_docx(xml, book.as_deref(), size);
            objs.built.borrow_mut().insert(rid.to_string(), b.clone());
            b?
        }
    };
    let img = placed(&root, size, Arc::from(&[][..]), EmbedKind::Chart(apex_doc::ChartObject { book }));
    Some((img, root))
}

/// The picture holding an object read from a `w:drawing`: its size,
/// name, description and place.
fn placed(root: &Node, size: (f32, f32), data: Arc<[u8]>, kind: EmbedKind) -> Image {
    let mut img = Image::new(data, size.0.max(1.0), size.1.max(1.0));
    if let Some(pr) = root.find("docPr") {
        img.name = pr.attr("name").unwrap_or_default().to_string();
        img.alt = pr.attr("descr").or_else(|| pr.attr("title")).unwrap_or_default().to_string();
    }
    if root.find("anchor").is_some() {
        img.wrap = if root.find("wrapTopAndBottom").is_some() || root.find("wrapNone").is_some() { Wrap::TopBottom } else { Wrap::Square };
        if let Some(h) = root.find("positionH") {
            match h.child("align").map(|a| a.text.trim().to_string()) {
                Some(a) => img.align = Some(if a == "center" { Align::Center } else if a == "right" { Align::Right } else { Align::Left }),
                None => img.offset_x = h.child("posOffset").and_then(|o| o.text.trim().parse::<f32>().ok()).map_or(0.0, |v| (v / crate::drawingml::EMU_PT).max(0.0)),
            }
        }
    }
    img.object = Some(Arc::new(Embedded::new(kind)));
    img
}

// --- writing ----------------------------------------------------------------------

/// File name of embedding number `k` with extension `ext`, as Office names
/// them.
pub(crate) fn embedding_name(k: usize, ext: &str) -> String {
    match ext {
        "xlsx" | "xlsm" => format!("Microsoft_Excel_Worksheet{k}.{ext}"),
        "docx" | "docm" => format!("Microsoft_Word_Document{k}.{ext}"),
        "pptx" | "pptm" => format!("Microsoft_PowerPoint_Presentation{k}.{ext}"),
        "" => format!("oleObject{k}.bin"),
        e => format!("oleObject{k}.{e}"),
    }
}

/// Embedded data and diagrams written to a docx package.
#[derive(Default)]
pub(crate) struct Parts {
    /// Data, extension, media type, relationship type.
    files: Vec<(Arc<[u8]>, String, String, &'static str)>,
    shapetype: bool,
    shapes: usize,
    diagrams: Vec<crate::diagram::Written>,
    /// Chart parts and the workbooks with their data.
    charts: Vec<(String, Arc<[u8]>)>,
}

const CHART_TYPE: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
const XLSX_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

impl Parts {
    fn name(&self, k: usize) -> String {
        embedding_name(k, &self.files[k - 1].1)
    }

    fn add(&mut self, o: &OleObject) -> usize {
        let part = crate::embed::ooxml_part(o);
        match self.files.iter().position(|f| Arc::ptr_eq(&f.0, &part.0)) {
            Some(i) => i + 1,
            None => {
                self.files.push(part);
                self.files.len()
            }
        }
    }

    /// Adds a diagram; returns its `dgm:relIds`.
    pub(crate) fn add_diagram(&mut self, d: &apex_doc::Diagram) -> String {
        let w = crate::diagram::write(d, self.diagrams.len() + 1, "word", "");
        let ids = w.rel_ids.clone();
        self.diagrams.push(w);
        ids
    }

    /// Adds a chart object; returns its relationship id (None: its
    /// workbook holds no chart).
    pub(crate) fn add_chart(&mut self, c: &apex_doc::ChartObject) -> Option<String> {
        let (wb, si, ci) = crate::chart_object::open(&c.book)?;
        let xml = crate::xlsx_chart::write_chart(&wb, si, &wb.sheets[si].charts[ci]);
        // Its workbook, where Word edits the data.
        let link = r#"<c:externalData r:id="rId1"><c:autoUpdate val="0"/></c:externalData>"#;
        let at = xml.rfind("<c:extLst>").filter(|&i| !xml[i..].contains("</c:chart>")).or_else(|| xml.rfind("</c:chartSpace>"))?;
        let xml = format!("{}{link}{}", &xml[..at], &xml[at..]);
        self.charts.push((xml, c.book.clone()));
        Some(format!("rIdC{}", self.charts.len()))
    }

    /// Relationships (targets relative to `word/`).
    pub(crate) fn rels(&self) -> String {
        let mut s = String::new();
        for (k, f) in self.files.iter().enumerate() {
            let _ = write!(s, r#"<Relationship Id="rIdO{}" Type="{}" Target="embeddings/{}"/>"#, k + 1, f.3, self.name(k + 1));
        }
        for d in &self.diagrams {
            s.push_str(&d.rels);
        }
        for k in 1..=self.charts.len() {
            let _ = write!(s, r#"<Relationship Id="rIdC{k}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="charts/chart{k}.xml"/>"#);
        }
        s
    }

    pub(crate) fn content_types(&self) -> String {
        let mut s = String::new();
        for (k, f) in self.files.iter().enumerate() {
            let _ = write!(s, r#"<Override PartName="/word/embeddings/{}" ContentType="{}"/>"#, self.name(k + 1), esc(&f.2));
        }
        for d in &self.diagrams {
            s.push_str(&d.types);
        }
        for k in 1..=self.charts.len() {
            let _ = write!(s, r#"<Override PartName="/word/charts/chart{k}.xml" ContentType="{CHART_TYPE}"/><Override PartName="/word/embeddings/Microsoft_Excel_Chart_Data{k}.xlsx" ContentType="{XLSX_TYPE}"/>"#);
        }
        s
    }

    pub(crate) fn write_files(&self, z: &mut ZipWriter) {
        for (k, f) in self.files.iter().enumerate() {
            z.add(&format!("word/embeddings/{}", self.name(k + 1)), &f.0, !f.1.starts_with("xls") && !f.1.starts_with("doc") && !f.1.starts_with("ppt"));
        }
        for d in &self.diagrams {
            for (path, data) in &d.files {
                z.add(path, data, true);
            }
        }
        for (k, (xml, book)) in self.charts.iter().enumerate() {
            let k = k + 1;
            z.add(&format!("word/charts/chart{k}.xml"), xml.as_bytes(), true);
            let rels = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/package" Target="../embeddings/Microsoft_Excel_Chart_Data{k}.xlsx"/></Relationships>"#);
            z.add(&format!("word/charts/_rels/chart{k}.xml.rels"), rels.as_bytes(), true);
            z.add(&format!("word/embeddings/Microsoft_Excel_Chart_Data{k}.xlsx"), book, false);
        }
    }
}

pub(crate) const SHAPETYPE: &str = r#"<v:shapetype id="_x0000_t75" coordsize="21600,21600" o:spt="75" o:preferrelative="t" path="m@4@5l@4@11@9@11@9@5xe" filled="f" stroked="f"><v:stroke joinstyle="miter"/><v:formulas><v:f eqn="if lineDrawn pixelLineWidth 0"/><v:f eqn="sum @0 1 0"/><v:f eqn="sum 0 0 @1"/><v:f eqn="prod @2 1 2"/><v:f eqn="prod @3 21600 pixelWidth"/><v:f eqn="prod @3 21600 pixelHeight"/><v:f eqn="sum @0 0 1"/><v:f eqn="prod @6 1 2"/><v:f eqn="prod @7 21600 pixelWidth"/><v:f eqn="sum @8 21600 0"/><v:f eqn="prod @7 21600 pixelHeight"/><v:f eqn="sum @10 21600 0"/></v:formulas><v:path o:extrusionok="f" gradientshapeok="t" o:connecttype="rect"/><o:lock v:ext="edit" aspectratio="t"/></v:shapetype>"#;

/// A preview for objects that have none: a small white picture.
pub(crate) fn blank_png() -> Arc<[u8]> {
    let mut raw = Vec::new();
    for _ in 0..8 {
        raw.push(0);
        raw.extend_from_slice(&[0xFF; 8 * 3]);
    }
    let chunk = |out: &mut Vec<u8>, ty: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = ty.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crate::zip::crc32(&c).to_be_bytes());
    };
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &[0, 0, 0, 8, 0, 0, 0, 8, 8, 2, 0, 0, 0]);
    chunk(&mut png, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
    chunk(&mut png, b"IEND", &[]);
    Arc::from(png)
}

/// Writes an embedded object as the inside of a `w:r`: `ds` places a
/// floating one.
pub(crate) fn write(out: &mut String, img: &Image, ds: Option<&DocShape>, media: &mut crate::docx_img::Media) {
    let Some(o) = img.object.as_ref().and_then(|o| o.ole()) else { return };
    let converted = crate::embed::for_family(o, false);
    let o = converted.as_ref().unwrap_or(o);
    // (A picture Office doesn't know, like LibreOffice's metafiles, is
    // kept as it is, as for pictures without an object.)
    let preview = if img.data.is_empty() { blank_png() } else { img.data.clone() };
    let pic = media.add(&preview);
    let k = media.objects.add(o);
    let (w, h) = ds.map_or((img.width, img.height), |d| (d.width, d.height));
    let _ = write!(out, r#"<w:object w:dxaOrig="{}" w:dyaOrig="{}">"#, (w * 20.0).round() as i64, (h * 20.0).round() as i64);
    if !media.objects.shapetype {
        media.objects.shapetype = true;
        out.push_str(SHAPETYPE);
    }
    media.objects.shapes += 1;
    let n = media.objects.shapes;
    let mut style = String::new();
    let mut wrap = "";
    let floating = ds.is_some() || img.wrap != Wrap::Inline;
    let id = if floating { format!("_x0000_s{}", 2048 + n) } else { format!("_x0000_i{}", 1024 + n) };
    match (ds, img.wrap) {
        (Some(d), _) => {
            let _ = write!(style, "position:absolute;margin-left:{:.2}pt;margin-top:{:.2}pt;", d.x, d.y);
            let z = if d.wrap == ShapeWrap::Behind { -Z_BASE + d.z as i64 } else { Z_BASE + d.z as i64 };
            let hr = if d.h_rel == HRel::Page { "page" } else { "margin" };
            let vr = match d.v_rel {
                VRel::Page => "page",
                VRel::Margin => "margin",
                VRel::Paragraph => "text",
            };
            let _ = write!(style, "z-index:{z};mso-position-horizontal-relative:{hr};mso-position-vertical-relative:{vr};");
            if let Some(a) = d.h_align {
                let _ = write!(style, "mso-position-horizontal:{};", match a {
                    Align::Center => "center",
                    Align::Right => "right",
                    _ => "left",
                });
            }
            match d.wrap {
                ShapeWrap::Square => wrap = r#"<w10:wrap type="square"/>"#,
                ShapeWrap::TopBottom => wrap = r#"<w10:wrap type="topAndBottom"/>"#,
                _ => {}
            }
        }
        (None, Wrap::Inline) => {}
        (None, w) => {
            let _ = write!(style, "position:absolute;margin-left:{:.2}pt;margin-top:0;z-index:{};mso-position-horizontal-relative:text;mso-position-vertical-relative:text;", img.offset_x, Z_BASE + n as i64);
            if let Some(a) = img.align {
                let _ = write!(style, "mso-position-horizontal:{};", match a {
                    Align::Center => "center",
                    Align::Right => "right",
                    _ => "left",
                });
            }
            wrap = if w == Wrap::TopBottom { r#"<w10:wrap type="topAndBottom"/>"# } else { r#"<w10:wrap type="square"/>"# };
        }
    }
    let _ = write!(style, "width:{w:.3}pt;height:{h:.3}pt");
    let alt = if img.alt.is_empty() { String::new() } else { format!(r#" alt="{}""#, esc(&img.alt)) };
    let _ = write!(out, r##"<v:shape id="{id}" type="#_x0000_t75" style="{style}"{alt} o:ole=""><v:imagedata r:id="rIdM{pic}" o:title=""/>{wrap}</v:shape>"##);
    let aspect = if o.icon { "Icon" } else { "Content" };
    let prog = if o.prog_id.is_empty() { "Package" } else { o.prog_id.as_str() };
    let _ = write!(out, r#"<o:OLEObject Type="Embed" ProgID="{}" ShapeID="{id}" DrawAspect="{aspect}" ObjectID="_{}" r:id="rIdO{k}"/>"#, esc(prog), 1_700_000_000 + n as u64);
    out.push_str("</w:object>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::{Document, Paragraph, Pos, Selection};

    fn object_doc() -> (Document, Vec<u8>) {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Before"), Paragraph::new("After")]);
        let book = crate::xlsx::write(&apex_sheet::Workbook::new());
        let obj = crate::embed::from_file("Book.xlsx", book.clone());
        let img = Image { object: Some(Arc::new(Embedded::new(EmbedKind::Ole(obj)))), alt: "Sales".into(), ..Image::new(blank_png(), 200.0, 80.0) };
        d.insert_image(Selection::caret(Pos::new(0, 6)), img);
        let pkg = crate::embed::from_file("notes.txt", b"hello".to_vec());
        let img = Image { object: Some(Arc::new(Embedded::new(EmbedKind::Ole(pkg)))), ..Image::new(Arc::from(&[][..]), 50.0, 40.0) };
        let s = d.insert_image(Selection::caret(Pos::new(1, 0)), img);
        d.float_image(s.start(), true, Some((30.0, 12.0)));
        (d, book)
    }

    #[test]
    fn docx_round_trip() {
        let (d, book) = object_doc();
        let bytes = crate::docx::write(&d);
        let z = ZipReader::new(&bytes).unwrap();
        assert!(z.contains("word/embeddings/Microsoft_Excel_Worksheet1.xlsx"));
        assert!(z.contains("word/embeddings/oleObject2.bin"));
        let ct = z.read_string("[Content_Types].xml").unwrap();
        assert!(ct.contains("/word/embeddings/oleObject2.bin\" ContentType=\"application/vnd.openxmlformats-officedocument.oleObject"));
        let back = crate::docx::read(&bytes).unwrap();
        let objs = back.objects();
        assert_eq!(objs.len(), 2);
        let (_, a) = back.images().into_iter().find(|(_, i)| i.object.is_some()).unwrap();
        let o = a.object.as_ref().unwrap().ole().unwrap();
        assert_eq!((o.prog_id.as_str(), &o.data[..]), ("Excel.Sheet.12", &book[..]));
        assert_eq!((a.width, a.height, a.alt.as_str()), (200.0, 80.0, "Sales"));
        let (_, ds) = back.shapes().into_iter().next().unwrap();
        assert_eq!((ds.wrap, ds.x, ds.y), (ShapeWrap::Behind, 30.0, 12.0));
        let pkg = ds.shape.picture.as_ref().unwrap().object.as_ref().unwrap().ole().unwrap().clone();
        assert_eq!(pkg.prog_id, "Package");
        // Without a preview it keeps none (the stand-in isn't read as one).
        assert!(ds.shape.picture.as_ref().unwrap().data.is_empty());
        assert_eq!(crate::embed::contents(&pkg), ("notes.txt".to_string(), b"hello".to_vec()));
        // Saved again, nothing changes.
        let again = crate::docx::read(&crate::docx::write(&back)).unwrap();
        assert_eq!(again.objects().len(), 2);
    }

    /// Sizes in inches (odt) keep their value, not one rounded up a point.
    #[test]
    fn object_size_is_precise() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("a")]);
        let obj = crate::embed::from_file("notes.txt", b"hello".to_vec());
        let img = Image { object: Some(Arc::new(Embedded::new(EmbedKind::Ole(obj)))), ..Image::new(Arc::from(&[][..]), 400.9, 225.497) };
        d.insert_image(Selection::caret(Pos::new(0, 0)), img);
        let back = crate::docx::read(&crate::docx::write(&d)).unwrap();
        let (_, a) = back.images().into_iter().find(|(_, i)| i.object.is_some()).unwrap();
        assert!((a.height - 225.497).abs() < 0.002, "{}", a.height);
    }

    #[test]
    fn diagrams_and_odt() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Plan")]);
        let img = Image { object: Some(Arc::new(Embedded::new(EmbedKind::Diagram(crate::diagram::tests::sample())))), ..Image::new(Arc::from(&[][..]), 300.0, 100.0) };
        d.insert_image(Selection::caret(Pos::new(0, 4)), img);
        let bytes = crate::docx::write(&d);
        let z = ZipReader::new(&bytes).unwrap();
        assert!(z.contains("word/diagrams/data1.xml") && z.contains("word/diagrams/drawing1.xml"));
        let back = crate::docx::read(&bytes).unwrap();
        let objs = back.objects();
        let g = objs[0].1.object.as_ref().unwrap().diagram().unwrap();
        assert_eq!((g.parts.len(), g.shapes.len()), (5, 2));
        assert_eq!(g.texts, ["Plan", "Do"]);
        // ODF has no diagrams: a group of the drawn shapes.
        let odt = crate::odt::write(&back);
        let content = ZipReader::new(&odt).unwrap().read_string("content.xml").unwrap();
        assert!(content.contains("<draw:g ") && content.matches("<draw:custom-shape").count() == 2, "{content}");
        // Objects in odt: sub-documents and OLE objects.
        let (d, book) = object_doc();
        let back = crate::odt::read(&crate::odt::write(&d)).unwrap();
        let objs = back.objects();
        assert_eq!(objs.len(), 2);
        let first = objs[0].1.object.as_ref().unwrap().ole().unwrap().clone();
        // The workbook became a spreadsheet sub-document, and back.
        assert_eq!(first.prog_id, "opendocument.CalcDocument.1");
        let docx = crate::docx::read(&crate::docx::write(&back)).unwrap();
        let o = docx.objects()[0].1.object.as_ref().unwrap().ole().unwrap().clone();
        assert_eq!(o.prog_id, "Excel.Sheet.12");
        assert!(matches!(crate::embed::openable(&o), Some((_, crate::Format::Xlsx))));
        let _ = book;
        let pkg = objs[1].1.object.as_ref().unwrap().ole().unwrap().clone();
        assert_eq!(crate::embed::contents(&pkg), ("notes.txt".to_string(), b"hello".to_vec()));
    }

    #[test]
    fn charts_round_trip_with_their_data() {
        use apex_sheet::ChartType;
        let book = crate::chart_object::book(&crate::chart_object::sample(ChartType::Line, "Cat", "Ser"));
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Sales")]);
        let img = Image { object: Some(Arc::new(Embedded::new(EmbedKind::Chart(apex_doc::ChartObject { book })))), ..Image::new(Arc::from(&[][..]), 360.0, 216.0) };
        d.insert_image(Selection::caret(Pos::new(0, 5)), img);
        let bytes = crate::docx::write(&d);
        let z = ZipReader::new(&bytes).unwrap();
        let chart = z.read_string("word/charts/chart1.xml").unwrap();
        assert!(chart.contains("<c:lineChart>") && chart.contains(r#"<c:externalData r:id="rId1">"#), "{chart}");
        assert!(z.contains("word/embeddings/Microsoft_Excel_Chart_Data1.xlsx"));
        let check = |doc: &Document, what: &str| {
            let objs = doc.objects();
            assert_eq!(objs.len(), 1, "{what}");
            let (img, c) = (&objs[0].1, objs[0].1.object.as_ref().unwrap().chart().unwrap_or_else(|| panic!("{what}")));
            assert_eq!(((img.width) as i32, img.height as i32), (360, 216), "{what}");
            let (chart, data, _) = crate::chart_object::chart_of(&c.book).unwrap();
            assert_eq!(chart.kind, ChartType::Line, "{what}");
            assert_eq!(data.categories, ["Cat 1", "Cat 2", "Cat 3", "Cat 4"], "{what}");
            assert_eq!(data.series[1].name, "Ser 2", "{what}");
            assert_eq!(data.series[2].values, [Some(2.0), Some(2.0), Some(3.0), Some(5.0)], "{what}");
        };
        let back = crate::docx::read(&bytes).unwrap();
        check(&back, "docx");
        // Without its workbook: from the values the chart last showed.
        let mut w = crate::zip::ZipWriter::new();
        for n in z.names() {
            if !n.contains("embeddings/") && !n.ends_with('/') {
                w.add(n, &z.read(n).unwrap(), true);
            }
        }
        check(&crate::docx::read(&w.finish()).unwrap(), "docx without workbook");
        // odt: a chart object with its local table.
        let odt = crate::odt::write(&back);
        let oz = ZipReader::new(&odt).unwrap();
        let content = oz.read_string("Object 1/content.xml").unwrap();
        assert!(content.contains(r#"table:name="local-table""#) && content.contains("local-table.$B$2:"), "{content}");
        assert!(!content.contains(r#"xlink:href="..""#));
        check(&crate::odt::read(&odt).unwrap(), "odt");
    }

    #[test]
    fn vml_objects() {
        let mut objs = Objects::default();
        objs.payloads.insert("rId5".into(), (Arc::from(&b"xls"[..]), "xls".into(), "application/vnd.ms-excel".into()));
        let media: HashMap<String, Arc<[u8]>> = [("rId4".to_string(), Arc::from(&b"emf"[..]))].into();
        let raw = r##"<v:shape id="_x0000_s1032" type="#_x0000_t75" style="position:absolute;margin-left:31.05pt;margin-top:5.2pt;width:180.9pt;height:180.5pt;z-index:251655680"><v:imagedata r:id="rId4" o:title=""/></v:shape><o:OLEObject Type="Embed" ProgID="Excel.Sheet.8" ShapeID="_x0000_s1032" DrawAspect="Content" ObjectID="_1" r:id="rId5"/></w:pict>"##;
        let Some(Placed::Float(d)) = read(raw, (None, None), &objs, &media) else { panic!() };
        assert_eq!((d.x, d.y, d.width, d.wrap), (31.05, 5.2, 180.9, ShapeWrap::InFront));
        let o = d.shape.picture.as_ref().unwrap().object.as_ref().unwrap().ole().unwrap().clone();
        assert_eq!((o.prog_id.as_str(), o.ext.as_str()), ("Excel.Sheet.8", "xls"));
        let inline = r#"<v:shape style="width:207.75pt;height:15.75pt" o:ole=""><v:imagedata r:id="rId4"/></v:shape><o:OLEObject Type="Embed" ProgID="Excel.Sheet.8" DrawAspect="Icon" r:id="rId5"/></w:object>"#;
        let Some(Placed::Inline(i)) = read(inline, (None, None), &objs, &media) else { panic!() };
        assert_eq!((i.width, i.height), (207.75, 15.75));
        assert!(i.object.as_ref().unwrap().ole().unwrap().icon);
        assert!(read(r#"<o:OLEObject Type="Link" r:id="rId5"/>"#, (None, None), &objs, &media).is_none());
        assert!(read(r#"<o:OLEObject Type="Embed" r:id="rId9"/>"#, (None, None), &objs, &media).is_none());
    }
}
