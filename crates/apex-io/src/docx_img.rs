//! Pictures in docx: `w:drawing` (inline and anchored) and VML `w:pict`.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use apex_doc::{Align, Image, PicColor, PicFx, Wrap};

use crate::xml::{Ev, Reader, Tag, esc};
use crate::zip::ZipReader;

/// English Metric Units per point.
const EMU_PT: f32 = 12700.0;

/// Extension and media type of an encoded picture.
pub(crate) fn image_type(data: &[u8]) -> (&'static str, &'static str) {
    if data.starts_with(b"\x89PNG") {
        ("png", "image/png")
    } else if data.starts_with(&[0xFF, 0xD8]) {
        ("jpeg", "image/jpeg")
    } else if data.starts_with(b"GIF8") {
        ("gif", "image/gif")
    } else if data.starts_with(b"BM") {
        ("bmp", "image/bmp")
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        ("tiff", "image/tiff")
    } else if data.starts_with(&[0xD7, 0xCD, 0xC6, 0x9A]) || data.starts_with(&[1, 0, 9, 0]) || data.starts_with(&[2, 0, 9, 0]) {
        ("wmf", "image/x-wmf")
    } else if data.starts_with(&[1, 0, 0, 0]) && data.get(40..44) == Some(b" EMF") {
        ("emf", "image/x-emf")
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        ("webp", "image/webp")
    } else if data.windows(4).take(512).any(|w| w == b"<svg") {
        ("svg", "image/svg+xml")
    } else {
        ("bin", "application/octet-stream")
    }
}

/// Loads the pictures the relationships point to, by relationship id.
pub(crate) fn load_media(z: &ZipReader, rels: &HashMap<String, String>, dir: &str) -> HashMap<String, Arc<[u8]>> {
    let mut out = HashMap::new();
    for (id, target) in rels {
        let lower = target.to_ascii_lowercase();
        // Ink parts ride along with the pictures (see `docx_ink`).
        let ink = lower.contains("ink/") && lower.ends_with(".xml");
        if !ink && !lower.contains("media/") && !["png", "jpeg", "jpg", "gif", "bmp", "tif", "tiff", "emf", "wmf", "svg"].iter().any(|e| lower.ends_with(&format!(".{e}"))) {
            continue;
        }
        let path = resolve(dir, target);
        if let Ok(data) = z.read(&path) {
            out.insert(id.clone(), Arc::from(data));
        }
    }
    out
}

/// A relationship target relative to the part's folder (`word`).
fn resolve(dir: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn emu(t: &Tag, key: &str) -> Option<f32> {
    t.attr(key)?.trim().parse::<f64>().ok().map(|v| v as f32 / EMU_PT)
}

/// Reads a `w:drawing` (after its start tag). `None` for shapes, charts and
/// pictures whose data is missing.
pub(crate) fn read_drawing(r: &mut Reader, media: &HashMap<String, Arc<[u8]>>) -> Option<Image> {
    let mut img = Image::default();
    let mut data = None;
    let mut size = None;
    let mut xfrm_size = None;
    let mut anchored = false;
    let mut wrap = Wrap::Square;
    let mut in_pos_h = false;
    let mut fx = PicFx::default();
    // A line being read: (color, width, no fill); and the glow.
    let mut ln: Option<(u32, f32, bool)> = None;
    let mut in_glow = false;
    let mut depth = 1;
    loop {
        let t = match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => {
                depth += 1;
                match t.local() {
                    "ln" => ln = Some((0, emu(&t, "w").unwrap_or(0.75), false)),
                    "glow" => in_glow = true,
                    _ => {}
                }
                t
            }
            Ev::Empty(t) => t,
            Ev::End(n) => {
                depth -= 1;
                match crate::xml::local(n) {
                    "positionH" => in_pos_h = false,
                    "ln" => {
                        if let Some((c, w, none)) = ln.take()
                            && !none
                        {
                            fx.border = Some((c, w));
                        }
                    }
                    "glow" => in_glow = false,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                continue;
            }
            _ => continue,
        };
        match t.local() {
            "anchor" => anchored = true,
            "extent" => {
                if let (Some(w), Some(h)) = (emu(&t, "cx"), emu(&t, "cy")) {
                    size = Some((w, h));
                }
            }
            "ext" => {
                if let (Some(w), Some(h)) = (emu(&t, "cx"), emu(&t, "cy")) {
                    xfrm_size.get_or_insert((w, h));
                }
            }
            "docPr" => {
                img.name = t.attr("name").unwrap_or_default().into_owned();
                img.alt = t.attr("descr").or_else(|| t.attr("title")).unwrap_or_default().into_owned();
                img.hidden = t.attr("hidden").is_some_and(|v| v == "1" || v == "true");
            }
            "blip" => {
                if let Some(id) = t.attr("r:embed") {
                    data = media.get(id.as_ref()).cloned();
                }
            }
            "srcRect" => {
                let f = |k: &str| t.attr(k).and_then(|v| v.parse::<f32>().ok()).map_or(0.0, |v| (v / 100_000.0).clamp(0.0, 0.99));
                img.crop = [f("l"), f("t"), f("r"), f("b")];
            }
            "xfrm" => {
                if let Some(rot) = t.attr("rot").and_then(|v| v.parse::<f32>().ok()) {
                    img.rotation = rot / 60000.0;
                }
            }
            "noFill" => {
                if let Some(l) = &mut ln {
                    l.2 = true;
                }
            }
            "srgbClr" => {
                let c = t.attr("val").and_then(|v| u32::from_str_radix(&v, 16).ok()).unwrap_or(0);
                if let Some(l) = &mut ln {
                    l.0 = c;
                } else if in_glow && let Some(g) = &mut fx.glow {
                    g.0 = c;
                }
            }
            "grayscl" => fx.color = PicColor::Grayscale,
            "biLevel" => fx.color = PicColor::BlackWhite,
            "duotone" => fx.color = PicColor::Sepia,
            "lum" => {
                let v = |k: &str| t.attr(k).and_then(|v| v.parse::<i32>().ok()).map_or(0, |v| (v / 1000).clamp(-100, 100) as i8);
                let (b, c) = (v("bright"), v("contrast"));
                if (b, c) == (70, -70) {
                    fx.color = PicColor::Washout;
                } else {
                    (fx.brightness, fx.contrast) = (b, c);
                }
            }
            "alphaModFix" => {
                let amt = t.attr("amt").and_then(|v| v.parse::<i32>().ok()).unwrap_or(100_000);
                fx.transparency = (100 - amt / 1000).clamp(0, 100) as u8;
            }
            "outerShdw" | "prstShdw" => fx.shadow = true,
            "glow" => fx.glow = Some((0xFFC000, emu(&t, "rad").unwrap_or(5.0))),
            "softEdge" => fx.soft_edges = emu(&t, "rad").unwrap_or(0.0),
            "reflection" => fx.reflection = true,
            "wrapSquare" | "wrapTight" | "wrapThrough" => wrap = Wrap::Square,
            "wrapTopAndBottom" | "wrapNone" => wrap = Wrap::TopBottom,
            "positionH" => in_pos_h = true,
            "posOffset" if in_pos_h => {
                img.offset_x = r.text().trim().parse::<f64>().map_or(0.0, |v| (v as f32 / EMU_PT).max(0.0));
                depth -= 1;
            }
            "align" if in_pos_h => {
                img.align = match r.text().trim() {
                    "center" => Some(Align::Center),
                    "right" | "outside" => Some(Align::Right),
                    _ => Some(Align::Left),
                };
                depth -= 1;
            }
            _ => {}
        }
    }
    let (w, h) = size.or(xfrm_size)?;
    img.data = data?;
    if !fx.is_default() {
        img.fx = Some(Arc::new(fx));
    }
    img.width = w.max(1.0);
    img.height = h.max(1.0);
    if anchored {
        img.wrap = wrap;
    } else {
        (img.offset_x, img.align) = (0.0, None);
    }
    Some(img)
}

/// A picture's effects: the `a:blip` children and what follows the
/// geometry in `pic:spPr`.
fn fx_xml(fx: &PicFx) -> (String, String) {
    let mut blip = String::new();
    match fx.color {
        PicColor::Grayscale => blip.push_str("<a:grayscl/>"),
        PicColor::BlackWhite => blip.push_str(r#"<a:biLevel thresh="50000"/>"#),
        PicColor::Sepia => blip.push_str(r#"<a:duotone><a:prstClr val="black"/><a:srgbClr val="D9C3A5"><a:tint val="50000"/><a:satMod val="180000"/></a:srgbClr></a:duotone>"#),
        _ => {}
    }
    let (b, c) = if fx.color == PicColor::Washout { (70, -70) } else { (fx.brightness as i32, fx.contrast as i32) };
    if b != 0 || c != 0 {
        let _ = write!(blip, r#"<a:lum bright="{}" contrast="{}"/>"#, b * 1000, c * 1000);
    }
    if fx.transparency > 0 {
        let _ = write!(blip, r#"<a:alphaModFix amt="{}"/>"#, (100 - fx.transparency.min(100) as i32) * 1000);
    }
    let mut sp = String::new();
    if let Some((color, w)) = fx.border {
        let _ = write!(sp, r#"<a:ln w="{}"><a:solidFill><a:srgbClr val="{color:06X}"/></a:solidFill></a:ln>"#, (w * EMU_PT) as i64);
    }
    let mut eff = String::new();
    if let Some((color, rad)) = fx.glow {
        let _ = write!(eff, r#"<a:glow rad="{}"><a:srgbClr val="{color:06X}"><a:alpha val="40000"/></a:srgbClr></a:glow>"#, (rad * EMU_PT) as i64);
    }
    if fx.shadow {
        eff.push_str(r#"<a:outerShdw blurRad="50800" dist="38100" dir="2700000" algn="tl" rotWithShape="0"><a:prstClr val="black"><a:alpha val="40000"/></a:prstClr></a:outerShdw>"#);
    }
    if fx.reflection {
        eff.push_str(r#"<a:reflection blurRad="6350" stA="50000" endA="300" endPos="55500" dist="12700" dir="5400000" sy="-100000" algn="bl" rotWithShape="0"/>"#);
    }
    if fx.soft_edges > 0.0 {
        let _ = write!(eff, r#"<a:softEdge rad="{}"/>"#, (fx.soft_edges * EMU_PT) as i64);
    }
    if !eff.is_empty() {
        let _ = write!(sp, "<a:effectLst>{eff}</a:effectLst>");
    }
    (blip, sp)
}

/// Reads a VML `w:pict` (after its start tag): the first `v:imagedata`.
pub(crate) fn read_pict(r: &mut Reader, media: &HashMap<String, Arc<[u8]>>) -> Option<Image> {
    let mut img = None;
    let mut size = None;
    let mut depth = 1;
    loop {
        let (t, start) = match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => {
                depth += 1;
                (t, true)
            }
            Ev::Empty(t) => (t, false),
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                continue;
            }
            _ => continue,
        };
        let _ = start;
        match t.local() {
            "shape" | "rect" if size.is_none() => size = t.attr("style").and_then(|s| vml_size(&s)),
            "imagedata" if img.is_none() => {
                if let Some(data) = t.attr("r:id").and_then(|id| media.get(id.as_ref()).cloned()) {
                    let mut i = Image { data, ..Default::default() };
                    i.name = t.attr("o:title").unwrap_or_default().into_owned();
                    img = Some(i);
                }
            }
            _ => {}
        }
    }
    let mut img = img?;
    let (w, h) = size?;
    (img.width, img.height) = (w.max(1.0), h.max(1.0));
    Some(img)
}

/// `width:120pt;height:3in` from a VML style, in points.
fn vml_size(style: &str) -> Option<(f32, f32)> {
    let mut w = None;
    let mut h = None;
    for decl in style.split(';') {
        let Some((k, v)) = decl.split_once(':') else { continue };
        let v = v.trim();
        let num_end = v.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(v.len());
        let Ok(n) = v[..num_end].parse::<f32>() else { continue };
        let pt = match &v[num_end..] {
            "pt" => n,
            "in" => n * 72.0,
            "cm" => n * 72.0 / 2.54,
            "mm" => n * 72.0 / 25.4,
            "px" | "" => n * 0.75,
            _ => continue,
        };
        match k.trim() {
            "width" => w = Some(pt),
            "height" => h = Some(pt),
            _ => {}
        }
    }
    Some((w?, h?))
}

/// Pictures written to a package: shared files are stored once.
#[derive(Default)]
pub(crate) struct Media {
    pub files: Vec<Arc<[u8]>>,
    next_id: usize,
    /// Groups of shapes written as one (see `docx_shape::group_plan`).
    pub groups: HashMap<u32, Vec<Arc<apex_doc::DocShape>>>,
    /// Embedded objects and diagrams.
    pub objects: crate::docx_ole::Parts,
    /// InkML parts (`ink/inkN.xml`).
    pub inks: Vec<String>,
    /// Indexes of the data added, by address (the data kept so its
    /// address isn't reused) and by a hash of the bytes: a document may
    /// show one picture many times.
    seen: HashMap<usize, (Arc<[u8]>, usize)>,
    by_hash: HashMap<u64, Vec<usize>>,
}

impl Media {
    /// Index of `data` in `files` (1-based), adding it if new.
    pub(crate) fn add(&mut self, data: &Arc<[u8]>) -> usize {
        use std::hash::{Hash, Hasher};
        if let Some((_, n)) = self.seen.get(&(Arc::as_ptr(data) as *const u8 as usize)) {
            return *n;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        data.hash(&mut h);
        let same = self.by_hash.entry(h.finish()).or_default();
        let n = match same.iter().find(|&&n| *self.files[n - 1] == **data) {
            Some(&n) => n,
            None => {
                self.files.push(data.clone());
                same.push(self.files.len());
                self.files.len()
            }
        };
        self.seen.insert(Arc::as_ptr(data) as *const u8 as usize, (data.clone(), n));
        n
    }

    /// A new `wp:docPr` id.
    pub(crate) fn next_doc_id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }

    /// Name of the picture file `n` (1-based) in the package.
    pub fn file_name(&self, n: usize) -> String {
        format!("image{}.{}", n, image_type(&self.files[n - 1]).0)
    }

    /// Relationships to the pictures (targets relative to `word/`).
    pub fn rels(&self) -> String {
        let mut s = String::new();
        for i in 1..=self.files.len() {
            let _ = write!(s, r#"<Relationship Id="rIdM{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/{}"/>"#, self.file_name(i));
        }
        s.push_str(&self.objects.rels());
        for i in 1..=self.inks.len() {
            let _ = write!(s, r#"<Relationship Id="rIdInk{i}" Type="{}" Target="ink/ink{i}.xml"/>"#, crate::inkml::REL_TYPE);
        }
        s
    }

    /// `Default` content types of the picture extensions used.
    pub fn content_types(&self) -> String {
        let mut seen = Vec::new();
        let mut s = String::new();
        for f in &self.files {
            let (ext, mime) = image_type(f);
            if !seen.contains(&ext) {
                seen.push(ext);
                let _ = write!(s, r#"<Default Extension="{ext}" ContentType="{mime}"/>"#);
            }
        }
        s.push_str(&self.objects.content_types());
        for i in 1..=self.inks.len() {
            let _ = write!(s, r#"<Override PartName="/word/ink/ink{i}.xml" ContentType="{}"/>"#, crate::inkml::CONTENT_TYPE);
        }
        s
    }

    /// The `wp:docPr` and `a:graphic` of a picture shown `cx` by `cy` EMU,
    /// turned `rotation` degrees; and its id.
    pub(crate) fn pic_graphic(&mut self, img: &Image, cx: i64, cy: i64, rotation: f32) -> (String, String, usize) {
        if let Some(d) = img.object.as_ref().and_then(|o| o.diagram()) {
            // A SmartArt diagram: its parts go with the package.
            self.next_id += 1;
            let id = self.next_id;
            let name = if img.name.is_empty() { format!("Diagram {id}") } else { img.name.clone() };
            let doc_pr = format!(r#"<wp:docPr id="{id}" name="{}" descr="{}"{}/>"#, esc(&name), esc(&img.alt), if img.hidden { r#" hidden="1""# } else { "" });
            let ids = self.objects.add_diagram(d);
            return (doc_pr, crate::diagram::graphic(&ids), id);
        }
        if let Some(rid) = img.object.as_ref().and_then(|o| o.chart()).and_then(|c| self.objects.add_chart(c)) {
            // A chart: its part and workbook go with the package.
            self.next_id += 1;
            let id = self.next_id;
            let name = if img.name.is_empty() { format!("Chart {id}") } else { img.name.clone() };
            let doc_pr = format!(r#"<wp:docPr id="{id}" name="{}" descr="{}"{}/>"#, esc(&name), esc(&img.alt), if img.hidden { r#" hidden="1""# } else { "" });
            let graphic = format!(r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="{uri}"><c:chart xmlns:c="{uri}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="{rid}"/></a:graphicData></a:graphic>"#, uri = crate::docx::CHART_URI);
            return (doc_pr, graphic, id);
        }
        let n = self.add(&img.data);
        self.next_id += 1;
        let id = self.next_id;
        let name = if img.name.is_empty() { format!("Picture {id}") } else { img.name.clone() };
        let doc_pr = format!(r#"<wp:docPr id="{id}" name="{}" descr="{}"{}/>"#, esc(&name), esc(&img.alt), if img.hidden { r#" hidden="1""# } else { "" });
        let rot = if rotation != 0.0 { format!(r#" rot="{}""#, (rotation * 60000.0).round() as i64) } else { String::new() };
        let crop = if img.crop.iter().any(|&c| c > 0.0) {
            let c = img.crop.map(|v| (v * 100_000.0).round() as i64);
            format!(r#"<a:srcRect l="{}" t="{}" r="{}" b="{}"/>"#, c[0], c[1], c[2], c[3])
        } else {
            String::new()
        };
        let (blip_fx, sp_fx) = img.fx.as_deref().map(fx_xml).unwrap_or_default();
        let blip = if blip_fx.is_empty() { format!(r#"<a:blip r:embed="rIdM{n}"/>"#) } else { format!(r#"<a:blip r:embed="rIdM{n}">{blip_fx}</a:blip>"#) };
        let graphic = format!(
            r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{id}" name="{}" descr="{}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill>{blip}{crop}<a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm{rot}><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>{sp_fx}</pic:spPr></pic:pic></a:graphicData></a:graphic>"#,
            esc(&name),
            esc(&img.alt)
        );
        (doc_pr, graphic, id)
    }

    /// A `w:drawing` for the picture.
    pub fn drawing(&mut self, out: &mut String, img: &Image) {
        if img.object.as_ref().is_some_and(|o| o.ole().is_some()) {
            return crate::docx_ole::write(out, img, None, self);
        }
        let (cx, cy) = ((img.width * EMU_PT) as i64, (img.height * EMU_PT) as i64);
        let (doc_pr, graphic, id) = self.pic_graphic(img, cx, cy, img.rotation);
        let [el, et, er, eb] = img.fx.as_deref().map_or([0.0; 4], |fx| fx.extent(img.height)).map(|v| (v * EMU_PT) as i64);
        let effect = format!(r#"<wp:effectExtent l="{el}" t="{et}" r="{er}" b="{eb}"/>"#);
        out.push_str("<w:drawing>");
        match img.wrap {
            Wrap::Inline => {
                let _ = write!(out, r#"<wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/>{effect}{doc_pr}<wp:cNvGraphicFramePr/>{graphic}</wp:inline>"#);
            }
            wrap => {
                let pos_h = match img.align {
                    Some(Align::Center) => "<wp:align>center</wp:align>".to_string(),
                    Some(Align::Right) => "<wp:align>right</wp:align>".to_string(),
                    Some(_) => "<wp:align>left</wp:align>".to_string(),
                    None => format!("<wp:posOffset>{}</wp:posOffset>", (img.offset_x * EMU_PT) as i64),
                };
                let wrap_xml = if wrap == Wrap::Square { r#"<wp:wrapSquare wrapText="bothSides"/>"# } else { "<wp:wrapTopAndBottom/>" };
                let _ = write!(
                    out,
                    r#"<wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" relativeHeight="{id}" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column">{pos_h}</wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="{cx}" cy="{cy}"/>{effect}{wrap_xml}{doc_pr}<wp:cNvGraphicFramePr/>{graphic}</wp:anchor>"#
                );
            }
        }
        out.push_str("</w:drawing>");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_sizes() {
        assert_eq!(resolve("word", "media/a.png"), "word/media/a.png");
        assert_eq!(resolve("word", "../media/a.png"), "media/a.png");
        assert_eq!(resolve("word", "/word/media/a.png"), "word/media/a.png");
        assert_eq!(vml_size("position:absolute;width:1in;height:36pt"), Some((72.0, 36.0)));
        assert_eq!(image_type(b"\x89PNG\r\n").0, "png");
    }
}
