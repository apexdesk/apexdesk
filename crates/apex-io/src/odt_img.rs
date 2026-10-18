//! Pictures in odt: `draw:frame` with a `draw:image`.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use apex_doc::{Align, DocShape, HRel, Image, PicColor, PicFx, Shape, ShapeWrap, VRel, Wrap};

use crate::docx_img::image_type;
use crate::parse_length_pt;
use crate::xml::{Ev, Reader, Tag, esc};
use crate::zip::ZipReader;

/// Position and wrapping of a graphic style.
#[derive(Clone, Debug, Default)]
pub(crate) struct Graphic {
    parent: Option<String>,
    wrap: Option<String>,
    hpos: Option<String>,
    clip: Option<String>,
    run_through: Option<String>,
    hrel: Option<String>,
    vrel: Option<String>,
    /// Picture effects: attribute -> value.
    fx: Vec<(&'static str, String)>,
}

/// Graphic properties of picture effects.
const FX_ATTRS: [&str; 10] = ["draw:luminance", "draw:contrast", "draw:color-mode", "draw:image-opacity", "fo:border", "style:shadow", "loext:glow-radius", "loext:glow-color", "loext:softedge-radius", "draw:shadow"];

fn percent(v: &str) -> Option<f32> {
    v.trim().trim_end_matches('%').trim().parse::<f32>().ok()
}

/// Picture effects from graphic properties.
fn read_fx(props: &[(&str, String)]) -> Option<PicFx> {
    let get = |k: &str| props.iter().find(|(a, _)| *a == k).map(|(_, v)| v.as_str());
    let mut fx = PicFx::default();
    fx.brightness = get("draw:luminance").and_then(percent).map_or(0, |v| v.clamp(-100.0, 100.0) as i8);
    fx.contrast = get("draw:contrast").and_then(percent).map_or(0, |v| v.clamp(-100.0, 100.0) as i8);
    fx.color = match get("draw:color-mode") {
        Some("greyscale") => PicColor::Grayscale,
        Some("mono") => PicColor::BlackWhite,
        Some("watermark") => PicColor::Washout,
        _ => PicColor::Normal,
    };
    fx.transparency = get("draw:image-opacity").and_then(percent).map_or(0, |v| (100.0 - v).clamp(0.0, 100.0) as u8);
    if let Some(b) = get("fo:border").filter(|b| !b.contains("none"))
        && let Some(w) = b.split_whitespace().next().and_then(parse_length_pt)
    {
        let c = b.split('#').nth(1).and_then(|c| u32::from_str_radix(c.get(..6)?, 16).ok()).unwrap_or(0);
        fx.border = Some((c, w));
    }
    fx.shadow = get("style:shadow").is_some_and(|v| !v.contains("none")) || get("draw:shadow") == Some("visible");
    if let Some(r) = get("loext:glow-radius").and_then(parse_length_pt).filter(|r| *r > 0.0) {
        let c = get("loext:glow-color").and_then(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok()).unwrap_or(0xFFC000);
        fx.glow = Some((c, r));
    }
    fx.soft_edges = get("loext:softedge-radius").and_then(parse_length_pt).unwrap_or(0.0);
    (!fx.is_default()).then_some(fx)
}

/// Graphic properties of a picture's effects.
fn write_fx(props: &mut String, fx: &PicFx) {
    if fx.brightness != 0 {
        let _ = write!(props, r#" draw:luminance="{}%""#, fx.brightness);
    }
    if fx.contrast != 0 {
        let _ = write!(props, r#" draw:contrast="{}%""#, fx.contrast);
    }
    let mode = match fx.color {
        PicColor::Grayscale | PicColor::Sepia => Some("greyscale"),
        PicColor::BlackWhite => Some("mono"),
        PicColor::Washout => Some("watermark"),
        PicColor::Normal => None,
    };
    if let Some(m) = mode {
        let _ = write!(props, r#" draw:color-mode="{m}""#);
    }
    if fx.transparency > 0 {
        let _ = write!(props, r#" draw:image-opacity="{}%""#, 100 - fx.transparency.min(100) as i32);
    }
    if let Some((c, w)) = fx.border {
        let _ = write!(props, r##" fo:border="{w:.2}pt solid #{c:06x}" fo:padding="0pt""##);
    }
    if fx.shadow {
        props.push_str(r##" style:shadow="#808080 0.1cm 0.1cm""##);
    }
    if let Some((c, r)) = fx.glow {
        let _ = write!(props, r##" loext:glow-radius="{r:.2}pt" loext:glow-color="#{c:06x}" loext:glow-transparency="60%""##);
    }
    if fx.soft_edges > 0.0 {
        let _ = write!(props, r#" loext:softedge-radius="{:.2}pt""#, fx.soft_edges);
    }
}

/// What a picture frame is: a picture in the text flow, or one floating
/// in front of or behind the text.
pub(crate) enum Framed {
    Image(Image),
    Float(DocShape),
}

/// Graphic styles by name (`style:family="graphic"`).
pub(crate) fn read_graphics(xml: &str, out: &mut HashMap<String, Graphic>) {
    let mut r = Reader::new(xml);
    let mut cur: Option<String> = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.name == "style:style" => {
                cur = None;
                if t.attr("style:family").as_deref() == Some("graphic") {
                    let name = t.attr("style:name").unwrap_or_default().into_owned();
                    out.insert(name.clone(), Graphic { parent: t.attr("style:parent-style-name").map(|v| v.into_owned()), ..Default::default() });
                    cur = Some(name);
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.name == "style:graphic-properties" => {
                if let Some(g) = cur.as_ref().and_then(|n| out.get_mut(n)) {
                    g.wrap = t.attr("style:wrap").map(|v| v.into_owned()).or(g.wrap.take());
                    g.hpos = t.attr("style:horizontal-pos").map(|v| v.into_owned()).or(g.hpos.take());
                    g.clip = t.attr("fo:clip").map(|v| v.into_owned()).or(g.clip.take());
                    g.run_through = t.attr("style:run-through").map(|v| v.into_owned()).or(g.run_through.take());
                    g.hrel = t.attr("style:horizontal-rel").map(|v| v.into_owned()).or(g.hrel.take());
                    g.vrel = t.attr("style:vertical-rel").map(|v| v.into_owned()).or(g.vrel.take());
                    for k in FX_ATTRS {
                        if let Some(v) = t.attr(k) {
                            g.fx.retain(|(a, _)| *a != k);
                            g.fx.push((k, v.into_owned()));
                        }
                    }
                }
            }
            Ev::End("style:style") => cur = None,
            _ => {}
        }
    }
}

/// Every file under `Pictures/`, and the content of embedded objects.
pub(crate) fn load_pictures(z: &ZipReader) -> HashMap<String, Arc<[u8]>> {
    let names: Vec<String> = z.names().filter(|n| n.starts_with("Pictures/")).map(str::to_string).collect();
    let mut out: HashMap<String, Arc<[u8]>> = names.into_iter().filter_map(|n| z.read(&n).ok().map(|d| (n, Arc::from(d)))).collect();
    crate::mathml::load_objects(z, &mut out);
    crate::odf_ole::load(z, &mut out);
    out
}

/// Width and height in pixels of a PNG, JPEG, GIF, BMP or WebP file.
pub fn pixel_size(data: &[u8]) -> Option<(u32, u32)> {
    let be32 = |i: usize| data.get(i..i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let le16 = |i: usize| data.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as u32);
    let le32 = |i: usize| data.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if data.starts_with(b"\x89PNG") {
        return Some((be32(16)?, be32(20)?));
    }
    if data.starts_with(b"GIF8") {
        return Some((le16(6)?, le16(8)?));
    }
    if data.starts_with(b"BM") {
        return Some((le32(18)?, (le32(22)? as i32).unsigned_abs()));
    }
    if data.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = data[i + 1];
            let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            // Start of frame markers (not DHT, JPG or DAC).
            if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((w, h));
            }
            if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
                i += 2;
            } else {
                i += 2 + len;
            }
        }
    }
    // WebP and the rest.
    apex_sheet::image::pixel_size(data)
}

/// The picture's own size in points (pixels at 96 dpi), for crop lengths.
fn natural_pt(data: &[u8], shown: (f32, f32)) -> (f32, f32) {
    pixel_size(data).filter(|&(w, h)| w > 0 && h > 0).map_or(shown, |(w, h)| (w as f32 * 0.75, h as f32 * 0.75))
}

/// Reads a `draw:frame` (`t`, already read; `start` if it has content).
/// `None` for frames without a known picture or object (text boxes).
pub(crate) fn read_frame(r: &mut Reader, t: &Tag, start: bool, graphics: &HashMap<String, Graphic>, pictures: &HashMap<String, Arc<[u8]>>) -> Option<Framed> {
    let mut img = Image { name: t.attr("draw:name").unwrap_or_default().into_owned(), ..Default::default() };
    img.hidden = t.attr("drawooo:display").or_else(|| t.attr("draw:display")).is_some_and(|v| v == "none");
    img.width = t.attr("svg:width").and_then(|v| parse_length_pt(&v)).unwrap_or(0.0);
    img.height = t.attr("svg:height").and_then(|v| parse_length_pt(&v)).unwrap_or(0.0);
    if let Some(tr) = t.attr("draw:transform") {
        if let Some(rad) = tr.split("rotate").nth(1).and_then(|s| s.trim_start().trim_start_matches('(').split(')').next()).and_then(|v| v.trim().parse::<f32>().ok()) {
            // ODF turns counterclockwise.
            img.rotation = (-rad.to_degrees()).rem_euclid(360.0);
            img.rotation = (img.rotation * 100.0).round() / 100.0;
        }
    }
    let anchor = t.attr("text:anchor-type").unwrap_or_default().into_owned();
    let x = t.attr("svg:x").and_then(|v| parse_length_pt(&v));
    let y = t.attr("svg:y").and_then(|v| parse_length_pt(&v));
    let z = t.attr("draw:z-index").and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(0);
    let style = t.attr("draw:style-name").map(|v| v.into_owned());
    let mut data = None;
    let mut object = None;
    let mut title = String::new();
    if start {
        let mut depth = 1;
        let href = |c: &Tag| c.attr("xlink:href").and_then(|h| pictures.get(h.trim_start_matches("./")).cloned());
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Empty(c) if c.name == "draw:image" && data.is_none() => data = href(&c),
                Ev::Empty(c) if c.name == "draw:object" || c.name == "draw:object-ole" => object = object.or_else(|| c.attr("xlink:href").and_then(|h| crate::odf_ole::doc_object(pictures, &h))),
                Ev::Start(c) if c.name == "svg:desc" || c.name == "svg:title" => {
                    let s = r.text();
                    if c.name == "svg:desc" {
                        img.alt = s;
                    } else {
                        title = s;
                    }
                }
                Ev::Start(c) => {
                    if c.name == "draw:image" && data.is_none() {
                        data = href(&c);
                    } else if c.name == "draw:object" || c.name == "draw:object-ole" {
                        object = object.or_else(|| c.attr("xlink:href").and_then(|h| crate::odf_ole::doc_object(pictures, &h)));
                    }
                    depth += 1;
                }
                Ev::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    if img.alt.is_empty() {
        img.alt = title;
    }
    if object.is_some() {
        // An embedded object shows its replacement picture, if any.
        data = data.or_else(|| Some(Arc::from(&[][..])));
        img.object = object.map(Arc::new);
    }
    img.data = data?;
    if img.width <= 0.0 || img.height <= 0.0 {
        let (w, h) = natural_pt(&img.data, (72.0, 72.0));
        (img.width, img.height) = (w, h);
    }
    // Style chain: wrap, horizontal position and crop.
    let mut g = Graphic::default();
    let mut name = style;
    for _ in 0..8 {
        let Some(s) = name.as_ref().and_then(|n| graphics.get(n)) else { break };
        g.wrap = g.wrap.or(s.wrap.clone());
        g.hpos = g.hpos.or(s.hpos.clone());
        g.clip = g.clip.or(s.clip.clone());
        g.run_through = g.run_through.or(s.run_through.clone());
        g.hrel = g.hrel.or(s.hrel.clone());
        g.vrel = g.vrel.or(s.vrel.clone());
        for (k, v) in &s.fx {
            if !g.fx.iter().any(|(a, _)| a == k) {
                g.fx.push((k, v.clone()));
            }
        }
        name = s.parent.clone();
    }
    img.fx = read_fx(&g.fx).map(Arc::new);
    if let Some(clip) = g.clip.as_deref().and_then(|c| c.trim().strip_prefix("rect(")).and_then(|c| c.strip_suffix(')')) {
        let v: Vec<f32> = clip.split([',', ' ']).filter(|s| !s.is_empty()).map(|s| parse_length_pt(s).unwrap_or(0.0)).collect();
        if let [top, right, bottom, left] = v[..] {
            let (nw, nh) = natural_pt(&img.data, (img.width, img.height));
            img.crop = [left / nw, top / nh, right / nw, bottom / nh].map(|c| (c * 1000.0).round().clamp(0.0, 990.0) / 1000.0);
        }
    }
    if anchor != "as-char" && g.wrap.as_deref() == Some("run-through") {
        // In front of or behind the text: a floating shape holding it.
        let (w, h) = (img.width, img.height);
        let mut d = DocShape::new(Shape::picture(img), w, h);
        d.wrap = if g.run_through.as_deref() == Some("background") { ShapeWrap::Behind } else { ShapeWrap::InFront };
        let page = anchor == "page";
        d.h_rel = match g.hrel.as_deref() {
            Some("page" | "page-start-margin" | "page-end-margin") => HRel::Page,
            None if page => HRel::Page,
            _ => HRel::Margin,
        };
        d.v_rel = match g.vrel.as_deref() {
            Some("page") => VRel::Page,
            Some("page-content") => VRel::Margin,
            _ if page => VRel::Page,
            _ => VRel::Paragraph,
        };
        d.h_align = match g.hpos.as_deref() {
            Some("center") => Some(Align::Center),
            Some("right" | "outside") => Some(Align::Right),
            Some("left" | "inside") => Some(Align::Left),
            _ => None,
        };
        (d.x, d.y, d.z) = (x.unwrap_or(0.0), y.unwrap_or(0.0), z);
        d.sanitize();
        return Some(Framed::Float(d));
    }
    if anchor != "as-char" {
        img.wrap = match g.wrap.as_deref() {
            Some("none") => Wrap::TopBottom,
            _ => Wrap::Square,
        };
        match g.hpos.as_deref() {
            Some("center") => img.align = Some(Align::Center),
            Some("right" | "outside") => img.align = Some(Align::Right),
            Some("left" | "inside") => img.align = Some(Align::Left),
            _ => img.offset_x = x.unwrap_or(0.0).max(0.0),
        }
    }
    Some(Framed::Image(img))
}

/// Pictures written to an odt package.
#[derive(Default)]
pub(crate) struct Pictures {
    files: Vec<Arc<[u8]>>,
    /// Indexes of the data added, by address (the data kept so its
    /// address isn't reused) and by a hash of the bytes.
    seen: std::collections::HashMap<usize, (Arc<[u8]>, usize)>,
    by_hash: std::collections::HashMap<u64, Vec<usize>>,
    styles: Vec<String>,
    count: usize,
    /// Equations, as formula objects.
    pub formulas: crate::mathml::Formulas,
    /// Files of embedded objects: path, bytes (none for folders), type.
    objects: Vec<(String, Option<Arc<[u8]>>, String)>,
    object_count: usize,
}

impl Pictures {
    pub(crate) fn file(&mut self, data: &Arc<[u8]>) -> String {
        let n = self.index(data);
        Self::path(n, data)
    }

    /// Index of `data` in `files` (1-based), adding it if new: found by
    /// hash, as a document may show many pictures many times.
    fn index(&mut self, data: &Arc<[u8]>) -> usize {
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

    fn path(n: usize, data: &[u8]) -> String {
        format!("Pictures/image{}.{}", n, image_type(data).0)
    }

    /// A `draw:frame` for a picture floating in front of or behind the
    /// text; `z_min` is the document's lowest z.
    pub(crate) fn floating_frame(&mut self, out: &mut String, d: &DocShape, z_min: i32) {
        let Some(pic) = &d.shape.picture else { return };
        if !d.wrap.floats() {
            let wrap = match d.wrap {
                ShapeWrap::Square => Wrap::Square,
                ShapeWrap::TopBottom => Wrap::TopBottom,
                _ => Wrap::Inline,
            };
            let img = Image { width: d.width, height: d.height, rotation: d.shape.rotation, wrap, offset_x: d.x.max(0.0), align: d.h_align, name: d.shape.name.clone(), alt: d.shape.alt.clone(), hidden: d.shape.hidden, ..(**pic).clone() };
            return self.frame(out, &img);
        }
        let img = Image { width: d.width, height: d.height, rotation: d.shape.rotation, wrap: Wrap::Inline, name: d.shape.name.clone(), alt: d.shape.alt.clone(), hidden: d.shape.hidden, ..(**pic).clone() };
        let through = if d.wrap == ShapeWrap::Behind { "background" } else { "foreground" };
        let h_pos = match d.h_align {
            Some(Align::Center) => "center",
            Some(Align::Right) => "right",
            Some(_) => "left",
            None => "from-left",
        };
        let h_rel = if d.h_rel == HRel::Page { "page" } else { "page-content" };
        let v_rel = match d.v_rel {
            VRel::Paragraph => "paragraph",
            VRel::Margin => "page-content",
            VRel::Page => "page",
        };
        let props = format!(r#" style:wrap="run-through" style:run-through="{through}" style:horizontal-pos="{h_pos}" style:horizontal-rel="{h_rel}" style:vertical-pos="from-top" style:vertical-rel="{v_rel}""#);
        let z = (d.z as i64 - z_min as i64).max(0);
        self.frame_with(out, &img, props, "paragraph", Some((d.x, d.y)), Some(z));
    }

    /// A `draw:frame` for the picture; its style goes to the automatic styles.
    pub fn frame(&mut self, out: &mut String, img: &Image) {
        self.frame_with(out, img, String::new(), "", None, None);
    }

    /// A frame with graphic properties `props` (and anchor, position and
    /// z-index) given, or found from the picture's wrapping.
    fn frame_with(&mut self, out: &mut String, img: &Image, mut props: String, anchor: &str, at: Option<(f32, f32)>, z: Option<i64>) {
        let ole = img.object.as_ref().and_then(|o| o.ole());
        let chart = img.object.as_ref().and_then(|o| o.chart());
        let href = if ole.is_some() || chart.is_some() { String::new() } else { self.file(&img.data) };
        self.count += 1;
        let n = self.count;
        if img.crop.iter().any(|&c| c > 0.0) {
            let (nw, nh) = natural_pt(&img.data, (img.width, img.height));
            let c = img.crop;
            let _ = write!(props, r#" fo:clip="rect({:.3}pt, {:.3}pt, {:.3}pt, {:.3}pt)""#, c[1] * nh, c[2] * nw, c[3] * nh, c[0] * nw);
        }
        if let Some(fx) = &img.fx {
            write_fx(&mut props, fx);
        }
        let anchor = match img.wrap {
            _ if !anchor.is_empty() => anchor,
            Wrap::Inline => {
                props.push_str(r#" style:vertical-pos="top" style:vertical-rel="baseline""#);
                "as-char"
            }
            wrap => {
                let _ = write!(props, r#" style:wrap="{}" style:number-wrapped-paragraphs="no-limit" style:vertical-pos="top" style:vertical-rel="paragraph" style:horizontal-rel="paragraph""#, if wrap == Wrap::Square { "parallel" } else { "none" });
                let pos = match img.align {
                    Some(Align::Center) => "center",
                    Some(Align::Right) => "right",
                    Some(_) => "left",
                    None => "from-left",
                };
                let _ = write!(props, r#" style:horizontal-pos="{pos}""#);
                "paragraph"
            }
        };
        let style = format!("fr{n}");
        self.styles.push(format!(r#"<style:style style:name="{style}" style:family="graphic"><style:graphic-properties{props}/></style:style>"#));
        let name = if img.name.is_empty() { format!("Image{n}") } else { img.name.clone() };
        let _ = write!(out, r#"<draw:frame draw:style-name="{style}" draw:name="{}" text:anchor-type="{anchor}""#, esc(&name));
        if img.hidden {
            out.push_str(crate::odf_shape::HIDDEN);
        }
        if let Some((x, y)) = at {
            let _ = write!(out, r#" svg:x="{x:.3}pt" svg:y="{y:.3}pt""#);
        } else if img.wrap != Wrap::Inline && img.align.is_none() {
            let _ = write!(out, r#" svg:x="{:.3}pt" svg:y="0pt""#, img.offset_x);
        }
        let _ = write!(out, r#" svg:width="{:.3}pt" svg:height="{:.3}pt" draw:z-index="{}""#, img.width, img.height, z.unwrap_or(n as i64 - 1));
        if img.rotation != 0.0 {
            // ODF rotates counterclockwise around the top-left corner, then
            // the frame is moved back so it turns about its center.
            let a = -img.rotation.to_radians();
            let (w, h) = (img.width, img.height);
            let (cx, cy) = (w / 2.0, h / 2.0);
            let (s, c) = a.sin_cos();
            let (tx, ty) = (cx - (cx * c + cy * s), cy - (-cx * s + cy * c));
            let _ = write!(out, r#" draw:transform="rotate ({:.6}) translate ({:.3}pt {:.3}pt)""#, a, tx, ty);
        }
        if let Some(c) = chart {
            self.object_count += 1;
            out.push('>');
            out.push_str(&crate::odf_ole::write_chart(c, self.object_count, &mut self.objects).unwrap_or_default());
        } else if let Some(o) = ole {
            self.object_count += 1;
            out.push('>');
            out.push_str(&crate::odf_ole::write(o, &img.data, self.object_count, &mut self.objects));
        } else {
            let _ = write!(out, r#"><draw:image xlink:href="{href}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad" draw:mime-type="{}"/>"#, image_type(&img.data).1);
        }
        if !img.alt.is_empty() {
            let _ = write!(out, "<svg:desc>{}</svg:desc>", esc(&img.alt));
        }
        out.push_str("</draw:frame>");
    }

    /// Automatic styles of the frames.
    pub fn styles(&self) -> String {
        self.styles.concat() + self.formulas.styles()
    }

    /// Manifest entries of the picture files.
    pub fn manifest(&self) -> String {
        let mut s = String::new();
        for (i, f) in self.files.iter().enumerate() {
            let _ = write!(s, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="{}"/>"#, Self::path(i + 1, f), image_type(f).1);
        }
        s + &self.formulas.manifest() + &crate::odf_ole::manifest_entries(&self.objects)
    }

    /// The picture files: path and bytes.
    pub fn files(&self) -> impl Iterator<Item = (String, &Arc<[u8]>)> {
        let objects = self.objects.iter().filter_map(|(p, d, _)| Some((p.clone(), d.as_ref()?)));
        self.files.iter().enumerate().map(|(i, f)| (Self::path(i + 1, f), f)).chain(self.formulas.files()).chain(objects)
    }
}
