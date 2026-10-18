//! Writes presentations as OpenDocument (.odp). A deck read from an .odp
//! goes back patched: its package files, styles and master pages stay
//! as they were, unchanged slides and objects are copied as read, and
//! only what changed is written again (with automatic styles named
//! "apx..." beside the file's own). Other decks are written whole, their
//! masters as master pages and everything resolved (sizes, colors,
//! fonts), so that the file looks the same without our masters.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Shape, ShapeKind, ShapePara, ShapeRun, VAnchor};
use apex_slides::fill::{Fill, Line};
use apex_slides::object::{Frame, Geometry, ObjKind, Object, PhType, Src};
use apex_slides::{Chain, Deck, Slide};

use crate::odf_shape::{self, Place, Styles};
use crate::odp_text::{self, AutoStyles};
use crate::xml::{HEADER, esc};
use crate::zip::ZipWriter;

pub(crate) const MIME: &str = "application/vnd.oasis.opendocument.presentation";

const NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:drawooo="http://openoffice.org/2010/draw" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" xmlns:number="urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:smil="urn:oasis:names:tc:opendocument:xmlns:smil-compatible:1.0" xmlns:anim="urn:oasis:names:tc:opendocument:xmlns:animation:1.0" xmlns:loext="urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0" xmlns:apx="urn:apexdesk:xmlns:presentation:1.0" office:version="1.3""#;

fn pt(v: f32) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    format!("{}pt", (v * 100.0).round() / 100.0)
}

fn hex(c: u32) -> String {
    format!("#{:06x}", c & 0xFFFFFF)
}

/// The writer's state: automatic styles, named fills for `office:styles`
/// and the pictures to add to the package.
struct W<'a> {
    deck: &'a Deck,
    auto: AutoStyles,
    /// Gradients, fill images, dashes and markers for `office:styles`.
    named: String,
    named_keys: HashMap<String, String>,
    common: bool,
    /// Pictures added: path and data.
    pics: Vec<(String, Arc<[u8]>)>,
    pic_names: HashMap<usize, String>,
    /// Videos and sounds added: path, data and media type.
    media: Vec<(String, Arc<[u8]>, String)>,
    /// Chart objects' files: path, data (none for their folder) and media
    /// type.
    objects: Vec<(String, Option<Arc<[u8]>>, String)>,
    shapes: odf_shape::Writer,
    /// The page whose objects get ids (`xml:id`), none on masters.
    page_ix: Option<usize>,
    /// Ids in the file of the page's objects written so far.
    targets: HashMap<u32, String>,
    /// The page's animated objects.
    animated: HashSet<u32>,
}

fn ext_of(data: &[u8]) -> (&'static str, &'static str) {
    if data.starts_with(b"\x89PNG") {
        ("png", "image/png")
    } else if data.starts_with(&[0xFF, 0xD8]) {
        ("jpg", "image/jpeg")
    } else if data.starts_with(b"GIF8") {
        ("gif", "image/gif")
    } else if data.starts_with(b"BM") {
        ("bmp", "image/bmp")
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        ("webp", "image/webp")
    } else if data.windows(4).take(512).any(|w| w == b"<svg") {
        ("svg", "image/svg+xml")
    } else {
        ("png", "image/png")
    }
}

impl W<'_> {
    /// The package path of a picture, added once.
    fn picture(&mut self, data: &Arc<[u8]>) -> String {
        let key = data.as_ptr() as usize;
        if let Some(p) = self.pic_names.get(&key) {
            return p.clone();
        }
        let path = format!("Pictures/apx{}.{}", self.pics.len() + 1, ext_of(data).0);
        self.pics.push((path.clone(), data.clone()));
        self.pic_names.insert(key, path.clone());
        path
    }

    /// The package path of a video or sound, added once.
    fn media_file(&mut self, data: &Arc<[u8]>, mime: &str) -> String {
        if let Some(m) = self.media.iter().find(|m| Arc::ptr_eq(&m.1, data)) {
            return m.0.clone();
        }
        let ext = crate::pptx_shape::ext_of_mime(mime);
        let path = format!("Media/apx{}.{ext}", self.media.len() + 1);
        self.media.push((path.clone(), data.clone(), mime.to_string()));
        path
    }

    /// A video or sound: LibreOffice's media player with the picture
    /// shown before it plays.
    fn media(&mut self, out: &mut String, o: &Object, p: &apex_slides::object::Picture, m: &apex_slides::object::Media, f: &Frame) {
        let href = match (&m.data, &m.link) {
            (Some(d), _) => self.media_file(d, &m.mime),
            (None, Some(l)) => l.clone(),
            _ => return,
        };
        let g = self.auto.get("graphic", r#"<style:graphic-properties draw:fill="none" draw:stroke="none"/>"#);
        let _ = write!(out, r#"<draw:frame draw:style-name="{g}"{} draw:layer="layout" {}><draw:plugin xlink:href="{}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad" draw:mime-type="application/vnd.sun.star.media">"#, self.name_attr(o), Self::place(f), esc(&href));
        let db = if m.volume > 0.0 { (20.0 * m.volume.log10()).round().clamp(-40.0, 0.0) as i32 } else { -40 };
        let params = [
            ("Loop", m.looped.to_string()),
            ("Mute", m.mute.to_string()),
            ("VolumeDB", db.to_string()),
            ("Zoom", "fit".to_string()),
            ("MimeType", m.mime.clone()),
            ("TrimStart", format!("{}", m.trim.0)),
            ("TrimEnd", format!("{}", m.trim.1)),
            ("AutoPlay", m.autoplay.to_string()),
        ];
        for (k, v) in params {
            let _ = write!(out, r#"<draw:param draw:name="{k}" draw:value="{}"/>"#, esc(&v));
        }
        out.push_str("</draw:plugin>");
        if !p.image.data.is_empty() {
            let path = self.picture(&p.image.data);
            let _ = write!(out, r#"<draw:image xlink:href="{path}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"><text:p/></draw:image>"#);
        }
        let _ = write!(out, "{}</draw:frame>", Self::desc(o));
    }

    /// A named element of `office:styles`, added once.
    fn named(&mut self, prefix: &str, body: impl Fn(&str) -> String, key: String) -> String {
        if let Some(n) = self.named_keys.get(&key) {
            return n.clone();
        }
        let name = format!("apx{prefix}{}", self.named_keys.len() + 1);
        self.named.push_str(&body(&name));
        self.named_keys.insert(key, name.clone());
        name
    }

    fn need_common(&mut self) {
        if !self.common {
            self.common = true;
            self.named.push_str(&odf_shape::common_styles());
        }
    }

    /// Graphic (or drawing page) fill properties.
    fn fill_props(&mut self, f: Option<&Fill>, ch: &Chain) -> String {
        match f {
            None | Some(Fill::None) | Some(Fill::Group) => r#" draw:fill="none""#.to_string(),
            Some(Fill::Solid(c)) => {
                let c = ch.color(c);
                let mut s = format!(r#" draw:fill="solid" draw:fill-color="{}""#, hex(c.rgb));
                if c.alpha < 255 {
                    let _ = write!(s, r#" draw:opacity="{}%""#, (c.alpha as f32 / 2.55).round());
                }
                s
            }
            Some(Fill::Gradient(g)) => {
                let g = g.resolve(&ch.cx());
                let name = self.named(
                    "G",
                    |n| {
                        let mut x = crate::odf_grad::gradient_xml(n, &g);
                        x.push_str(&crate::odf_grad::opacity_xml(&format!("{n}_T"), &g).unwrap_or_default());
                        x
                    },
                    format!("g{}", crate::odf_grad::gradient_xml("", &g)) + &crate::odf_grad::opacity_xml("", &g).unwrap_or_default(),
                );
                let mut s = format!(r#" draw:fill="gradient" draw:fill-gradient-name="{name}" draw:fill-color="{}""#, hex(g.main_color()));
                if !g.opaque() {
                    let _ = write!(s, r#" draw:opacity-name="{name}_T""#);
                }
                s
            }
            Some(Fill::Picture(p)) => {
                let path = self.picture(&p.data);
                let name = self.named("I", |n| format!(r#"<draw:fill-image draw:name="{n}" xlink:href="{path}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/>"#), format!("i{path}"));
                format!(r#" draw:fill="bitmap" draw:fill-image-name="{name}" style:repeat="{}""#, if p.tile { "repeat" } else { "stretch" })
            }
            Some(f @ Fill::Pattern { .. }) => {
                let c = f.main_color(&ch.cx()).map_or(0xFFFFFF, |c| c.rgb);
                format!(r#" draw:fill="solid" draw:fill-color="{}""#, hex(c))
            }
        }
    }

    /// Outline properties.
    fn line_props(&mut self, l: Option<&Line>, ch: &Chain) -> String {
        let Some(l) = l else { return r#" draw:stroke="none""#.to_string() };
        let color = match &l.fill {
            None | Some(Fill::None) => return r#" draw:stroke="none""#.to_string(),
            Some(f) => f.main_color(&ch.cx()).map_or(0, |c| c.rgb),
        };
        let mut s = String::new();
        let dash = l.dash.map(|d| d.to_doc()).filter(|d| *d != apex_doc::Dash::Solid);
        match dash {
            Some(d) => {
                self.need_common();
                let _ = write!(s, r#" draw:stroke="dash" draw:stroke-dash="{}""#, odf_shape::dash_name(d));
            }
            None => s.push_str(r#" draw:stroke="solid""#),
        }
        let w = l.width.unwrap_or(0.75);
        let _ = write!(s, r#" svg:stroke-color="{}" svg:stroke-width="{}""#, hex(color), pt(w));
        let head = pt((w * 4.0).max(6.0));
        if l.head.is_some_and(|a| a != apex_slides::fill::Arrow::None) {
            self.need_common();
            let _ = write!(s, r#" draw:marker-start="Arrow" draw:marker-start-width="{head}""#);
        }
        if l.tail.is_some_and(|a| a != apex_slides::fill::Arrow::None) {
            self.need_common();
            let _ = write!(s, r#" draw:marker-end="Arrow" draw:marker-end-width="{head}""#);
        }
        s
    }

    /// Position attributes of a frame (turned about its centre).
    fn place(f: &Frame) -> String {
        let (w, h) = (f.w.max(0.0), f.h.max(0.0));
        let pos = if f.rot.abs() > 0.001 {
            let (cx, cy) = (f.x + w / 2.0, f.y + h / 2.0);
            let (sn, c) = f.rot.to_radians().sin_cos();
            let (tx, ty) = (cx + (f.x - cx) * c - (f.y - cy) * sn, cy + (f.x - cx) * sn + (f.y - cy) * c);
            format!(r#"svg:width="{}" svg:height="{}" draw:transform="rotate ({}) translate ({} {})""#, pt(w), pt(h), -f.rot.to_radians(), pt(tx), pt(ty))
        } else {
            format!(r#"svg:width="{}" svg:height="{}" svg:x="{}" svg:y="{}""#, pt(w), pt(h), pt(f.x), pt(f.y))
        };
        // ODF frames don't flip: kept for us.
        match (f.flip_h, f.flip_v) {
            (false, false) => pos,
            (h, v) => format!(r#"{pos} apx:flip="{}{}""#, if h { "h" } else { "" }, if v { "v" } else { "" }),
        }
    }

    /// The id of an object of a slide (`xml:id` and `draw:id`), noted
    /// for its animations.
    fn xid(&mut self, o: &Object) -> String {
        let Some(p) = self.page_ix else { return String::new() };
        let x = format!("apx{p}_{}", o.id);
        self.targets.insert(o.id, x.clone());
        format!(r#" xml:id="{x}" draw:id="{x}""#)
    }

    /// An object kept as read: its id noted, or given one when animated.
    fn kept(&mut self, o: &Object, xml: &str) -> String {
        let Some(p) = self.page_ix else { return xml.to_string() };
        let head = &xml[..xml.find('>').unwrap_or(0)];
        for k in [" xml:id=\"", " draw:id=\""] {
            if let Some(at) = head.find(k).map(|i| i + k.len()) {
                if let Some(e) = head[at..].find('"') {
                    self.targets.insert(o.id, head[at..at + e].to_string());
                    return xml.to_string();
                }
            }
        }
        if !self.animated.contains(&o.id) || !xml.starts_with('<') {
            return xml.to_string();
        }
        let end = xml[1..].find(|c: char| c.is_whitespace() || c == '/' || c == '>').map_or(xml.len(), |e| e + 1);
        let x = format!("apx{p}_{}", o.id);
        self.targets.insert(o.id, x.clone());
        format!(r#"{} xml:id="{x}" draw:id="{x}"{}"#, &xml[..end], &xml[end..])
    }

    /// Whether objects inside group `o` are animated.
    fn animated_inside(&self, o: &Object) -> bool {
        match &o.kind {
            ObjKind::Group(g) => g.children.iter().any(|c| self.animated.contains(&c.id) || self.animated_inside(c)),
            _ => false,
        }
    }

    /// What ODF can't tell of an object's own fill and outline: that it
    /// has none of its own (it follows its placeholder or style, which
    /// show none), and which outline settings are its own.
    fn own_attrs(o: &Object, ch: &Chain) -> String {
        let mut s = String::new();
        if o.sp.fill.is_none() && ch.fill(o).is_none() {
            s.push_str(r#" apx:fill="inherit""#);
        }
        let shown = ch.line(o);
        match &o.sp.line {
            None if shown.is_none() => s.push_str(r#" apx:line="inherit""#),
            None => {}
            Some(l) => {
                let r = shown.unwrap_or_default();
                let set: Vec<&str> = [("width", l.width.is_some() || r.width.is_some()), ("fill", l.fill.is_some() || r.fill.is_some()), ("dash", l.dash.is_some() || r.dash.is_some()), ("head", l.head.is_some() || r.head.is_some()), ("tail", l.tail.is_some() || r.tail.is_some())]
                    .into_iter()
                    .filter_map(|(k, on)| on.then_some(k))
                    .collect();
                let _ = write!(s, r#" apx:line="{}""#, set.join(" "));
            }
        }
        s
    }

    /// The outline width of an object that shows no outline (it comes
    /// back with the outline settings).
    fn hidden_width(o: &Object, ch: &Chain) -> String {
        let drawn = ch.line(o).is_some_and(|l| l.fill.as_ref().is_some_and(|f| !matches!(f, Fill::None)));
        match o.sp.line.as_ref().and_then(|l| l.width).filter(|_| !drawn) {
            Some(w) => format!(r#" svg:stroke-width="{}""#, pt(w)),
            None => String::new(),
        }
    }

    fn name_attr(&mut self, o: &Object) -> String {
        let mut s = self.xid(o);
        if !o.name.is_empty() {
            let _ = write!(s, r#" draw:name="{}""#, esc(&o.name));
        }
        if o.hidden {
            s.push_str(crate::odf_shape::HIDDEN);
        }
        s
    }

    fn desc(o: &Object) -> String {
        if o.descr.is_empty() { String::new() } else { format!("<svg:desc>{}</svg:desc>", esc(&o.descr)) }
    }

    /// Writes an object (copied as read when it came unchanged from an .odp).
    fn object(&mut self, out: &mut String, o: &Object, ch: &Chain, keep: bool) {
        if keep && !self.animated_inside(o) {
            if let Some(r) = o.raw.as_ref().filter(|r| r.src == Src::Odp) {
                let xml = self.kept(o, &r.xml);
                out.push_str(&xml);
                return;
            }
        }
        let f = ch.frame(o);
        match &o.kind {
            ObjKind::Shape { text, text_box, connector } => {
                let geom = ch.geometry(o);
                let rect = geom.preset_name() == Some("rect");
                if o.ph.is_some() || (*text_box && rect) {
                    self.text_frame(out, o, text.as_ref(), &f, ch);
                } else if let Geometry::Custom(xml) = &geom {
                    self.path(out, o, xml, &f, ch);
                } else {
                    self.shape(out, o, text.as_ref(), &f, &geom, *connector, ch);
                }
            }
            ObjKind::Picture(p) if p.media.is_some() => self.media(out, o, p, p.media.as_ref().unwrap(), &f),
            ObjKind::Picture(p) => {
                let path = self.picture(&p.image.data);
                let g = self.auto.get("graphic", r#"<style:graphic-properties draw:fill="none" draw:stroke="none"/>"#);
                let _ = write!(out, r#"<draw:frame draw:style-name="{g}"{} draw:layer="layout" {}><draw:image xlink:href="{path}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"><text:p/></draw:image>{}</draw:frame>"#, self.name_attr(o), Self::place(&f), Self::desc(o));
            }
            ObjKind::Other { preview: Some(data), .. } => {
                let path = self.picture(data);
                let g = self.auto.get("graphic", r#"<style:graphic-properties draw:fill="none" draw:stroke="none"/>"#);
                let _ = write!(out, r#"<draw:frame draw:style-name="{g}"{} draw:layer="layout" {}><draw:image xlink:href="{path}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"><text:p/></draw:image></draw:frame>"#, self.name_attr(o), Self::place(&f));
            }
            ObjKind::Group(g) => {
                let _ = write!(out, "<draw:g{}>", self.name_attr(o));
                let (sx, sy) = (if g.child.w > 0.0 { f.w / g.child.w } else { 1.0 }, if g.child.h > 0.0 { f.h / g.child.h } else { 1.0 });
                for c in &g.children {
                    let mut c = c.clone();
                    let cf = ch.frame(&c);
                    let mut nf = cf;
                    nf.x = f.x + (cf.x - g.child.x) * sx;
                    nf.y = f.y + (cf.y - g.child.y) * sy;
                    nf.w = cf.w * sx;
                    nf.h = cf.h * sy;
                    c.sp.frame = Some(nf);
                    c.raw = None;
                    self.object(out, &c, ch, false);
                }
                out.push_str("</draw:g>");
            }
            ObjKind::Table(t) => self.table(out, o, t, &f, ch),
            // OpenDocument has no SmartArt: its drawing as a group of
            // shapes, or its text when it has none.
            ObjKind::Diagram(d) => {
                self.need_common();
                let _ = write!(out, "<draw:g{}>", self.name_attr(o));
                let (sx, sy) = (if d.size.0 > 0.0 { f.w / d.size.0 } else { 1.0 }, if d.size.1 > 0.0 { f.h / d.size.1 } else { 1.0 });
                for s in &d.shapes {
                    let place = Place { x: f.x + s.x * sx, y: f.y + s.y * sy, w: s.w * sx, h: s.h * sy, attrs: r#"draw:layer="layout""#, graphic: "", z: 0 };
                    self.shapes.write(out, &s.shape, &place);
                }
                if d.shapes.is_empty() && !d.texts.is_empty() {
                    let mut s = Shape::new(ShapeKind::TextBox);
                    s.text = d.texts.iter().map(|t| ShapePara { runs: vec![ShapeRun::new(t.clone())], ..Default::default() }).collect();
                    let place = Place { x: f.x, y: f.y, w: f.w, h: f.h, attrs: r#"draw:layer="layout""#, graphic: "", z: 0 };
                    self.shapes.write(out, &s, &place);
                }
                out.push_str("</draw:g>");
            }
            // A chart as a chart object with its data in its own table,
            // under a name the file doesn't use yet.
            ObjKind::Chart(c) => {
                let used = |n: usize| {
                    let dir = format!("Object {n}/");
                    self.objects.iter().any(|(p, _, _)| p.starts_with(&dir)) || self.deck.package.parts.iter().any(|p| p.path.starts_with(&dir))
                };
                let n = (1..).find(|n| !used(*n)).unwrap_or(1);
                if let Some(obj) = crate::odf_ole::write_chart(c, n, &mut self.objects) {
                    let _ = write!(out, r#"<draw:frame{} draw:layer="layout" {}>{obj}{}</draw:frame>"#, self.name_attr(o), Self::place(&f), Self::desc(o));
                }
            }
            // Equations and objects without a picture have no
            // OpenDocument form here.
            _ => {}
        }
    }

    /// Graphic properties of a text frame.
    fn text_props(&mut self, o: &Object, t: Option<&apex_slides::TextBody>, ch: &Chain) -> String {
        let mut g = String::new();
        if let Some(t) = t {
            let b = ch.body(o, t);
            let ins = b.insets_or_default();
            let v = match b.anchor.unwrap_or(VAnchor::Top) {
                VAnchor::Top => "top",
                VAnchor::Middle => "middle",
                VAnchor::Bottom => "bottom",
            };
            let grow_h = matches!(b.autofit, Some(apex_slides::text::Autofit::Resize));
            let grow_w = b.wrap == Some(false);
            let _ = write!(g, r#" draw:textarea-vertical-align="{v}" draw:auto-grow-height="{grow_h}" draw:auto-grow-width="{grow_w}" fo:padding-left="{}" fo:padding-top="{}" fo:padding-right="{}" fo:padding-bottom="{}""#, pt(ins[0]), pt(ins[1]), pt(ins[2]), pt(ins[3]));
            if matches!(b.autofit, Some(apex_slides::text::Autofit::Shrink { .. })) {
                g.push_str(r#" draw:fit-to-size="shrink-to-fit""#);
            }
            if b.wrap == Some(false) {
                g.push_str(r#" fo:wrap-option="no-wrap""#);
            }
        }
        g
    }

    fn text_frame(&mut self, out: &mut String, o: &Object, t: Option<&apex_slides::TextBody>, f: &Frame, ch: &Chain) {
        let fill = ch.fill(o);
        let line = ch.line(o);
        let mut g = self.fill_props(fill.as_ref(), ch);
        g.push_str(&self.line_props(line.as_ref(), ch));
        g.push_str(&Self::hidden_width(o, ch));
        g.push_str(&self.text_props(o, t, ch));
        let own = Self::own_attrs(o, ch);
        let class = o.ph.map(|p| match p.kind {
            PhType::Title | PhType::CtrTitle => "title",
            PhType::SubTitle => "subtitle",
            PhType::Date => "date-time",
            PhType::Footer => "footer",
            PhType::SlideNum => "page-number",
            PhType::Header => "header",
            _ => "outline",
        });
        let mut body = String::new();
        let empty = t.is_none_or(|t| t.is_empty());
        if let Some(t) = t.filter(|_| !empty || o.ph.is_none()) {
            odp_text::write_body(&mut body, t, o, ch, &mut self.auto);
        }
        let head = match class {
            Some(c) => {
                let s = self.auto.get("presentation", &format!("<style:graphic-properties{g}/>"));
                let ph = if empty { r#" presentation:placeholder="true""# } else { "" };
                // Moved or sized from where its layout has it.
                let moved = if o.sp.frame.is_some() { r#" presentation:user-transformed="true""# } else { "" };
                format!(r#"presentation:style-name="{s}" presentation:class="{c}"{moved}{ph}{own}"#)
            }
            None => {
                let s = self.auto.get("graphic", &format!("<style:graphic-properties{g}/>"));
                format!(r#"draw:style-name="{s}"{own}"#)
            }
        };
        if body.is_empty() {
            let _ = write!(out, r#"<draw:frame {head}{} draw:layer="layout" {}><draw:text-box/>{}</draw:frame>"#, self.name_attr(o), Self::place(f), Self::desc(o));
        } else {
            let _ = write!(out, r#"<draw:frame {head}{} draw:layer="layout" {}><draw:text-box>{body}</draw:text-box>{}</draw:frame>"#, self.name_attr(o), Self::place(f), Self::desc(o));
        }
    }

    /// A preset shape, through the document shapes' writer.
    #[allow(clippy::too_many_arguments)]
    fn shape(&mut self, out: &mut String, o: &Object, t: Option<&apex_slides::TextBody>, f: &Frame, geom: &Geometry, connector: bool, ch: &Chain) {
        let name = geom.preset_name().unwrap_or("rect");
        let (kind, exact) = if connector || geom.is_line() { (ShapeKind::Line, true) } else { ShapeKind::from_preset(name) };
        let mut s = Shape::new(kind);
        if !exact || (kind == ShapeKind::Line && name != "line") {
            s.preset = Some(Arc::from(name));
        }
        if let Geometry::Preset(_, adj) = geom {
            s.adj = adj.clone();
        }
        let fill = ch.fill(o);
        s.fill = if kind.is_line() { None } else { fill.as_ref().and_then(|f| f.main_color(&ch.cx())).map(|c| c.rgb) };
        let line = ch.line(o);
        match line.as_ref().filter(|l| !matches!(l.fill, None | Some(Fill::None))) {
            Some(l) => {
                s.line = l.fill.as_ref().and_then(|f| f.main_color(&ch.cx())).map(|c| c.rgb);
                s.line_width = l.width.unwrap_or(0.75);
                s.dash = l.dash.map_or(apex_doc::Dash::Solid, |d| d.to_doc());
                s.arrow_start = l.head.is_some_and(|a| a != apex_slides::fill::Arrow::None);
                s.arrow_end = l.tail.is_some_and(|a| a != apex_slides::fill::Arrow::None);
            }
            None => s.line = None,
        }
        s.rotation = f.rot;
        s.flip_h = f.flip_h;
        s.flip_v = f.flip_v;
        s.name = o.name.clone();
        s.alt = o.descr.clone();
        if let Some(t) = t.filter(|t| !t.is_empty()) {
            s.valign = ch.body(o, t).anchor.unwrap_or(VAnchor::Middle);
            for i in 0..t.paras.len() {
                let (pp, _) = ch.para_run(o, t, i);
                let p = &t.paras[i];
                let mut para = ShapePara { align: pp.align.unwrap_or(apex_doc::Align::Center), runs: Vec::new(), list: None };
                let mut at = 0;
                for r in &p.runs {
                    let end = (at + r.len).min(p.text.len());
                    let text = p.text.get(at..end).unwrap_or("").replace(apex_doc::OBJECT, "");
                    at = end;
                    let rf = ch.run(o, &pp, &r.attrs);
                    let mut sr = ShapeRun::new(text);
                    sr.bold = rf.bold;
                    sr.italic = rf.italic;
                    sr.underline = rf.underline;
                    sr.size = Some(rf.size);
                    sr.color = Some(rf.color.rgb);
                    sr.font = Some(rf.font.clone());
                    para.runs.push(sr);
                }
                s.text.push(para);
            }
        }
        let attrs = format!(r#"draw:layer="layout"{}{}"#, self.xid(o), Self::own_attrs(o, ch));
        let place = Place { x: f.x, y: f.y, w: f.w, h: f.h, attrs: &attrs, graphic: "", z: 0 };
        self.need_common();
        let fancy = !kind.is_line() && matches!(fill, Some(Fill::Gradient(_) | Fill::Picture(_)));
        if fancy {
            s.fill = None;
        }
        let at = self.shapes.styles.len();
        self.shapes.write(out, &s, &place);
        // Gradients and pictures: the shape's style gets them in place of
        // the solid color the shapes' writer knows.
        if fancy {
            let fp = self.fill_props(fill.as_ref(), ch);
            let none = r#" draw:fill="none""#;
            if let Some(i) = self.shapes.styles[at..].find(none) {
                self.shapes.styles.replace_range(at + i..at + i + none.len(), &fp);
            }
        }
        let width = Self::hidden_width(o, ch);
        if !width.is_empty() {
            let none = r#" draw:stroke="none""#;
            if let Some(i) = self.shapes.styles[at..].find(none) {
                self.shapes.styles.insert_str(at + i + none.len(), &width);
            }
        }
    }

    /// Drawn paths, flattened.
    fn path(&mut self, out: &mut String, o: &Object, xml: &str, f: &Frame, ch: &Chain) {
        let paths = apex_slides::geom::custom_paths(xml, f.w.max(1.0), f.h.max(1.0));
        let fill = ch.fill(o);
        let line = ch.line(o);
        let open = paths.iter().all(|p| p.fill == apex_doc::PresetFill::None);
        let mut g = if open { r#" draw:fill="none""#.to_string() } else { self.fill_props(fill.as_ref(), ch) };
        g.push_str(&self.line_props(line.as_ref(), ch));
        g.push_str(&Self::hidden_width(o, ch));
        let style = self.auto.get("graphic", &format!("<style:graphic-properties{g}/>"));
        let own = Self::own_attrs(o, ch);
        let mut d = String::new();
        for p in &paths {
            for (pts, closed) in &p.subpaths {
                for (i, (x, y)) in pts.iter().enumerate() {
                    let _ = write!(d, "{}{} {} ", if i == 0 { "M" } else { "L" }, (x * 100.0).round() as i64, (y * 100.0).round() as i64);
                }
                if *closed {
                    d.push_str("Z ");
                }
            }
        }
        let vb = format!("0 0 {} {}", (f.w.max(1.0) * 100.0).round() as i64, (f.h.max(1.0) * 100.0).round() as i64);
        let _ = write!(out, r#"<draw:path draw:style-name="{style}"{}{own} draw:layer="layout" {} svg:viewBox="{vb}" svg:d="{}">{}</draw:path>"#, self.name_attr(o), Self::place(f), d.trim_end(), Self::desc(o));
    }

    fn table(&mut self, out: &mut String, o: &Object, t: &apex_slides::Table, f: &Frame, ch: &Chain) {
        let g = self.auto.get("graphic", r#"<style:graphic-properties draw:fill="none" draw:stroke="none"/>"#);
        let _ = write!(out, r#"<draw:frame draw:style-name="{g}"{} draw:layer="layout" {}><table:table>"#, self.name_attr(o), Self::place(f));
        for w in &t.cols {
            let s = self.auto.get("table-column", &format!(r#"<style:table-column-properties style:column-width="{}"/>"#, pt(*w)));
            let _ = write!(out, r#"<table:table-column table:style-name="{s}"/>"#);
        }
        for (r, row) in t.rows.iter().enumerate() {
            let s = self.auto.get("table-row", &format!(r#"<style:table-row-properties style:row-height="{}"/>"#, pt(row.height)));
            let _ = write!(out, r#"<table:table-row table:style-name="{s}">"#);
            for (c, cell) in row.cells.iter().enumerate() {
                if cell.covered() {
                    out.push_str("<table:covered-table-cell/>");
                    continue;
                }
                let look = t.cell_look(r, c);
                let fill = cell.fill.clone().or(look.fill.clone());
                let mut gp = self.fill_props(fill.as_ref(), ch);
                let mut borders = String::new();
                for (k, side) in ["left", "right", "top", "bottom"].into_iter().enumerate() {
                    let l = cell.borders[k].clone().or(look.border.clone());
                    let v = match l.as_ref().filter(|l| !matches!(l.fill, None | Some(Fill::None))) {
                        Some(l) => format!("{} solid {}", pt(l.width.unwrap_or(0.75)), hex(l.fill.as_ref().and_then(|f| f.main_color(&ch.cx())).map_or(0, |c| c.rgb))),
                        None => "none".into(),
                    };
                    let _ = write!(borders, r#" fo:border-{side}="{v}""#);
                }
                let v = match cell.anchor.unwrap_or(VAnchor::Top) {
                    VAnchor::Top => "top",
                    VAnchor::Middle => "middle",
                    VAnchor::Bottom => "bottom",
                };
                let _ = write!(gp, r#" draw:textarea-vertical-align="{v}""#);
                let cs = self.auto.get("table-cell", &format!(r#"<style:graphic-properties{gp}/><style:table-cell-properties{borders}/>"#));
                let mut span = String::new();
                if cell.span > 1 {
                    let _ = write!(span, r#" table:number-columns-spanned="{}""#, cell.span);
                }
                if cell.rspan > 1 {
                    let _ = write!(span, r#" table:number-rows-spanned="{}""#, cell.rspan);
                }
                let _ = write!(out, r#"<table:table-cell table:style-name="{cs}"{span}>"#);
                // Cell text in the style's color and weight.
                let mut text = cell.text.clone();
                if look.text.is_some() || look.bold {
                    let color = look.text.as_ref().map(|c| ch.color(c).rgb);
                    for p in &mut text.paras {
                        for run in &mut p.runs {
                            if run.attrs.color.is_none() {
                                run.attrs.color = color;
                            }
                            run.attrs.bold |= look.bold;
                        }
                    }
                }
                odp_text::write_body(out, &text, o, ch, &mut self.auto);
                out.push_str("</table:table-cell>");
            }
            out.push_str("</table:table-row>");
        }
        out.push_str("</table:table></draw:frame>");
    }

    /// A drawing page style: `extra` properties (the original page's)
    /// with the fill and visibility of `s` over them.
    /// In a file of our own (`inherit` false) the background is always
    /// written, as its layouts are not; otherwise only the slide's own.
    fn page_style(&mut self, s: &Slide, extra: Option<&HashMap<String, String>>, ch: &Chain, inherit: bool) -> String {
        let mut props: Vec<(String, String)> = extra.map(|m| m.iter().filter(|(k, _)| !k.starts_with("draw:fill") && !k.starts_with("draw:opacity") && !matches!(k.as_str(), "style:repeat" | "presentation:visibility" | "presentation:background-visible" | "presentation:background-objects-visible") && !crate::odp_anim::is_transition_prop(k)).map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
        props.sort();
        let mut p = String::new();
        for (k, v) in props {
            let _ = write!(p, r#" {k}="{}""#, esc(&v));
        }
        if !inherit || s.background.is_some() {
            let bg = ch.background();
            p.push_str(&self.fill_props(Some(&bg), ch));
        }
        p.push_str(r#" presentation:background-visible="true""#);
        let _ = write!(p, r#" presentation:background-objects-visible="{}""#, s.show_master);
        if s.hidden {
            p.push_str(r#" presentation:visibility="hidden""#);
        }
        p.push_str(&crate::odp_anim::transition_props(&s.transition));
        self.auto.get("drawing-page", &format!("<style:drawing-page-properties{p}/>"))
    }

    /// A slide written again.
    fn page(&mut self, out: &mut String, i: usize, s: &Slide, st: Option<&Styles>, masters: &[String]) {
        let deck = self.deck;
        let ch = Chain::for_slide(deck, s);
        // The original page: its style's other settings, animations and forms.
        let orig = s.origin.as_ref().filter(|o| o.src == Src::Odp).and_then(|o| o.xml.clone());
        let orig_node = orig.as_deref().map(crate::drawingml::read_fragment);
        let page_node = orig_node.as_ref().and_then(|n| n.kids.first());
        let extra = page_node.and_then(|p| p.attr("draw:style-name")).and_then(|n| st.and_then(|st| st.own(n)));
        let dp = self.page_style(s, extra, &ch, st.is_some());
        let name = if s.name.is_empty() { format!("page{}", i + 1) } else { s.name.clone() };
        let master = masters.get(s.master).cloned().unwrap_or_default();
        let _ = write!(out, r#"<draw:page draw:name="{}" draw:style-name="{dp}" draw:master-page-name="{}""#, esc(&name), esc(&master));
        // The footer, header and date the master's frames show.
        for a in ["presentation:use-footer-name", "presentation:use-header-name", "presentation:use-date-time-name"] {
            if let Some(v) = page_node.and_then(|p| p.attr(a)) {
                let _ = write!(out, r#" {a}="{}""#, esc(v));
            }
        }
        out.push('>');
        if let Some(p) = page_node {
            for k in p.kids.iter().filter(|k| k.local() == "forms") {
                crate::pptx_shape::node_xml(out, k);
            }
        }
        let keep = st.is_some();
        self.targets.clear();
        self.animated = s.timing.anims.iter().map(|a| a.shape).collect();
        // A new file has no layouts: their decoration goes on the slide.
        if !keep {
            if let Some(l) = deck.layout_of(s) {
                let lch = Chain::for_layout(deck, s.master, s.layout);
                for o in l.objects.iter().filter(|o| o.ph.is_none()) {
                    self.object(out, o, &lch, false);
                }
            }
        }
        self.page_ix = Some(s.id as usize);
        for o in &s.objects {
            self.object(out, o, &ch, keep);
        }
        self.page_ix = None;
        // Animations: ours written again, the page's other sequences kept.
        let mut kept = Vec::new();
        if let Some(root) = page_node.and_then(crate::odp_anim::timing_root) {
            for k in root.kids.iter().filter(|k| !crate::odp_anim::is_main(k)) {
                let mut x = String::new();
                crate::pptx_shape::node_xml(&mut x, k);
                kept.push(x);
            }
        }
        let targets = std::mem::take(&mut self.targets);
        crate::odp_anim::write_timing(out, &s.timing, &|id| targets.get(&id).cloned(), &kept);
        // Notes: a picture of the slide above its text.
        let (nw, nh) = if deck.notes_size.0 > 1.0 { deck.notes_size } else { (540.0, 720.0) };
        out.push_str("<presentation:notes>");
        let tw = nw * 0.75;
        let th = tw * deck.height / deck.width.max(1.0);
        let _ = write!(out, r#"<draw:page-thumbnail draw:layer="layout" svg:width="{}" svg:height="{}" svg:x="{}" svg:y="{}" draw:page-number="{}" presentation:class="page"/>"#, pt(tw), pt(th), pt(nw * 0.125), pt(nh * 0.075), i + 1);
        let text = s.notes.as_ref().map(|n| &n.text).filter(|t| !t.is_empty());
        let nf = Frame::new(nw * 0.1, nh * 0.475, nw * 0.8, nh * 0.45);
        let ns = self.auto.get("presentation", r#"<style:graphic-properties draw:fill="none" draw:stroke="none"/>"#);
        match text {
            Some(t) => {
                let mut notes_obj = Object::new(1, "Notes", ObjKind::Shape { text: None, text_box: true, connector: false });
                notes_obj.sp.frame = Some(nf);
                let mut body = String::new();
                odp_text::write_body(&mut body, t, &notes_obj, &ch, &mut self.auto);
                let _ = write!(out, r#"<draw:frame presentation:style-name="{ns}" draw:layer="layout" {} presentation:class="notes"><draw:text-box>{body}</draw:text-box></draw:frame>"#, Self::place(&nf));
            }
            None => {
                let _ = write!(out, r#"<draw:frame presentation:style-name="{ns}" draw:layer="layout" {} presentation:class="notes" presentation:placeholder="true"><draw:text-box/></draw:frame>"#, Self::place(&nf));
            }
        }
        out.push_str("</presentation:notes></draw:page>");
    }
}

/// The master page names of a deck read from an .odp, in order.
fn odp_masters(deck: &Deck) -> Vec<String> {
    deck.masters.iter().map(|m| m.origin.as_ref().map_or_else(|| m.name.clone(), |o| o.part.clone())).collect()
}

fn part<'a>(deck: &'a Deck, path: &str) -> Option<&'a [u8]> {
    deck.package.part(path).map(|p| &*p.data)
}

/// Writes an .odp package.
pub fn write(deck: &Deck) -> Vec<u8> {
    let orig = deck.package.src == Some(Src::Odp) && part(deck, "content.xml").is_some();
    let mut w = W { deck, auto: AutoStyles::new("apx"), named: String::new(), named_keys: HashMap::new(), common: false, pics: Vec::new(), pic_names: HashMap::new(), media: Vec::new(), objects: Vec::new(), shapes: odf_shape::Writer::default(), page_ix: None, targets: HashMap::new(), animated: HashSet::new() };
    let (content, styles) = if orig { patched(deck, &mut w) } else { whole(deck, &mut w) };
    let mut z = ZipWriter::new();
    z.add("mimetype", MIME.as_bytes(), false);
    let mut manifest_extra = String::new();
    for (path, data) in &w.pics {
        let _ = write!(manifest_extra, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="{}"/>"#, esc(path), ext_of(data).1);
        z.add(path, data, false);
    }
    for (path, data, mime) in &w.media {
        let _ = write!(manifest_extra, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="{}"/>"#, esc(path), esc(mime));
        z.add(path, data, false);
    }
    for (path, data, mime) in &w.objects {
        let _ = write!(manifest_extra, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="{}"/>"#, esc(path), esc(mime));
        if let Some(d) = data {
            z.add(path, d, true);
        }
    }
    // Parts of the file as read that nothing points at any more (a picture,
    // an embedded document or its replacement image removed, say by the
    // document inspector) stay out, and so does the old thumbnail.
    let dropped: Vec<&str> = if orig { deck.package.parts.iter().map(|p| p.path.as_str()).filter(|p| unused_part(p, &content, &styles)).collect() } else { Vec::new() };
    let manifest = match part(deck, "META-INF/manifest.xml").filter(|_| orig).map(String::from_utf8_lossy) {
        Some(m) if m.contains("</manifest:manifest>") => without_entries(&m, &dropped).replacen("</manifest:manifest>", &format!("{manifest_extra}</manifest:manifest>"), 1),
        _ => format!(
            r#"{HEADER}<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="{MIME}"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/>{manifest_extra}</manifest:manifest>"#
        ),
    };
    z.add("META-INF/manifest.xml", manifest.as_bytes(), true);
    if orig {
        for p in &deck.package.parts {
            if matches!(p.path.as_str(), "mimetype" | "content.xml" | "styles.xml" | "meta.xml" | "META-INF/manifest.xml") || dropped.contains(&p.path.as_str()) {
                continue;
            }
            z.add(&p.path, &p.data, !p.path.starts_with("Pictures/"));
        }
    }
    z.add("meta.xml", crate::props::odf_xml(&deck.meta).as_bytes(), true);
    z.add("styles.xml", styles.as_bytes(), true);
    z.add("content.xml", content.as_bytes(), true);
    z.finish()
}

/// Whether a part of the file as read is left out: the thumbnail, and
/// pictures, media and embedded objects the content and styles written no
/// longer name.
fn unused_part(path: &str, content: &str, styles: &str) -> bool {
    let top = path.split('/').next().unwrap_or("");
    let name = match top {
        "Thumbnails" => return true,
        "Pictures" | "Media" | "ObjectReplacements" => path,
        _ if top.starts_with("Object ") && path.contains('/') => top,
        _ => return false,
    };
    let named = |x: &str| x.contains(name) || x.contains(&esc(name).to_string());
    !named(content) && !named(styles)
}

/// A manifest without the entries of `paths` (and of the directories of
/// embedded objects left out).
fn without_entries(m: &str, paths: &[&str]) -> String {
    let mut out = m.to_string();
    let mut gone: Vec<String> = paths.iter().map(|p| esc(p).to_string()).collect();
    gone.extend(paths.iter().filter_map(|p| p.split_once('/').map(|(d, _)| format!("{}/", esc(d)))).filter(|d| d.starts_with("Object ")));
    for g in gone {
        let key = format!("manifest:full-path=\"{g}\"");
        while let Some(at) = out.find(&key) {
            let Some(start) = out[..at].rfind('<') else { break };
            let Some(end) = out[at..].find("/>").map(|e| at + e + 2) else { break };
            out.replace_range(start..end, "");
        }
    }
    out
}

/// Puts `add` at the end of element `tag` of `xml` (or makes the element
/// before `before` when it is missing or empty).
fn append_in(xml: &str, tag: &str, add: &str, before: &str) -> String {
    let close = format!("</{tag}>");
    if let Some(i) = xml.find(&close) {
        return format!("{}{add}{}", &xml[..i], &xml[i..]);
    }
    let empty = format!("<{tag}/>");
    if let Some(i) = xml.find(&empty) {
        return format!("{}<{tag}>{add}</{tag}>{}", &xml[..i], &xml[i + empty.len()..]);
    }
    match xml.find(before) {
        Some(i) => format!("{}<{tag}>{add}</{tag}>{}", &xml[..i], &xml[i..]),
        None => xml.to_string(),
    }
}

/// A deck read from an .odp: its content patched, its styles kept.
fn patched(deck: &Deck, w: &mut W) -> (String, String) {
    let content = String::from_utf8_lossy(part(deck, "content.xml").unwrap_or_default()).into_owned();
    let styles = String::from_utf8_lossy(part(deck, "styles.xml").unwrap_or_default()).into_owned();
    let mut st = Styles::default();
    st.read(&styles);
    st.read(&content);
    let masters = odp_masters(deck);
    let mut pages = String::new();
    for (i, s) in deck.slides.iter().enumerate() {
        match s.origin.as_ref().filter(|o| o.src == Src::Odp && o.same).and_then(|o| o.xml.as_deref()) {
            Some(xml) => pages.push_str(xml),
            None => w.page(&mut pages, i, s, Some(&st), &masters),
        }
    }
    // The pages replace the old ones; declarations before them and the
    // show settings after them stay.
    // (The end tag can't be inside an attribute, as an earlier copy can.)
    let (start, end) = match (content.find("<office:presentation"), content.rfind("</office:presentation>")) {
        (Some(a), Some(b)) if a < b => (content[a..].find('>').map_or(a, |k| a + k + 1).min(b), b),
        _ => (content.len(), content.len()),
    };
    let inner = &content[start..end];
    let first = inner.find("<draw:page ").or_else(|| inner.find("<draw:page>")).unwrap_or(inner.len());
    let last = inner.rfind("</draw:page>").map_or(first, |k| k + "</draw:page>".len()).max(first);
    let tail = if deck.show.changed() { crate::odp_anim::set_show(&inner[last..], &deck.show) } else { inner[last..].to_string() };
    let new_content = format!("{}{}{pages}{tail}{}", &content[..start], &inner[..first], &content[end..]);
    let new_content = if w.auto.xml.is_empty() { new_content } else { append_in(&new_content, "office:automatic-styles", &w.auto.xml, "<office:body") };
    let shape_styles = std::mem::take(&mut w.shapes.styles);
    let new_content = if shape_styles.is_empty() { new_content } else { append_in(&new_content, "office:automatic-styles", &shape_styles, "<office:body") };
    let mut styles = if w.named.is_empty() { styles } else { append_in(&styles, "office:styles", &w.named, "<office:automatic-styles") };
    for (m, name) in deck.masters.iter().zip(&masters) {
        if m.origin.as_ref().is_some_and(|o| o.src == Src::Odp && !o.same) {
            styles = crate::odp_anim::set_master_theme(&styles, name, &m.theme.theme);
        }
    }
    use crate::odp_anim::{NS_APX, NS_LOEXT};
    let ns = [("apx", NS_APX), ("loext", NS_LOEXT), ("smil", "urn:oasis:names:tc:opendocument:xmlns:smil-compatible:1.0"), ("anim", "urn:oasis:names:tc:opendocument:xmlns:animation:1.0"), ("presentation", "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0")];
    (crate::odp_anim::ensure_ns(&new_content, &ns), crate::odp_anim::ensure_ns(&styles, &ns))
}

/// Any other deck: everything written.
fn whole(deck: &Deck, w: &mut W) -> (String, String) {
    let masters: Vec<String> = (0..deck.masters.len()).map(|i| format!("M{}", i + 1)).collect();
    // Master pages with their decoration and background.
    let mut sauto = AutoStyles::new("apxm");
    std::mem::swap(&mut w.auto, &mut sauto);
    let mut mp = String::new();
    for (i, m) in deck.masters.iter().enumerate() {
        let ch = Chain::for_master(deck, i);
        let bg = ch.background();
        let fill = w.fill_props(Some(&bg), &ch);
        let dp = w.auto.get("drawing-page", &format!("<style:drawing-page-properties{fill}/>"));
        let _ = write!(mp, r#"<style:master-page style:name="{}" style:display-name="{}" style:page-layout-name="PM1" draw:style-name="{dp}">"#, masters[i], esc(&m.name));
        mp.push_str(&crate::odp_anim::theme_xml(&m.theme.theme));
        for o in m.objects.iter().filter(|o| o.ph.is_none()) {
            w.object(&mut mp, o, &ch, false);
        }
        mp.push_str("</style:master-page>");
    }
    std::mem::swap(&mut w.auto, &mut sauto);
    let master_shapes = std::mem::take(&mut w.shapes.styles);
    let mut body = String::new();
    for (i, s) in deck.slides.iter().enumerate() {
        w.page(&mut body, i, s, None, &masters);
    }
    if crate::odp_anim::show_set(&deck.show) {
        let _ = write!(body, "<presentation:settings{}/>", crate::odp_anim::show_attrs(&deck.show));
    }
    let shape_styles = std::mem::take(&mut w.shapes.styles);
    let (nw, nh) = if deck.notes_size.0 > 1.0 { deck.notes_size } else { (540.0, 720.0) };
    let styles = format!(
        r##"{HEADER}<office:document-styles {NS}><office:styles><style:default-style style:family="graphic"><style:graphic-properties svg:stroke-color="#3465a4" draw:fill-color="#729fcf"/><style:text-properties fo:font-size="18pt"/></style:default-style>{named}</office:styles><office:automatic-styles><style:page-layout style:name="PM1"><style:page-layout-properties fo:margin-top="0pt" fo:margin-bottom="0pt" fo:margin-left="0pt" fo:margin-right="0pt" fo:page-width="{}" fo:page-height="{}" style:print-orientation="{}"/></style:page-layout><style:page-layout style:name="PM2"><style:page-layout-properties fo:page-width="{}" fo:page-height="{}" style:print-orientation="portrait"/></style:page-layout>{}{master_shapes}</office:automatic-styles><office:master-styles>{mp}</office:master-styles></office:document-styles>"##,
        pt(deck.width),
        pt(deck.height),
        if deck.width >= deck.height { "landscape" } else { "portrait" },
        pt(nw),
        pt(nh),
        sauto.xml,
        named = w.named,
    );
    let content = format!(
        r#"{HEADER}<office:document-content {NS}><office:automatic-styles>{}{shape_styles}</office:automatic-styles><office:body><office:presentation>{body}</office:presentation></office:body></office:document-content>"#,
        w.auto.xml
    );
    (content, styles)
}

#[cfg(test)]
mod tests {
    use apex_slides::edit;

    fn deck() -> apex_slides::Deck {
        let mut d = apex_slides::build::new_deck(0);
        let n = d.slides.len();
        edit::add_slide(&mut d, n, 0, 1);
        let s = d.slide_mut(0);
        if let Some(t) = s.objects.iter_mut().find(|o| o.is_title()).and_then(|o| o.text_or_new()) {
            t.set_plain("Hello ODP");
        }
        let s = d.slide_mut(1);
        if let Some(t) = s.objects.iter_mut().find(|o| o.ph.is_some_and(|p| p.kind.is_body())).and_then(|o| o.text_or_new()) {
            t.set_plain("One\nTwo");
        }
        s.notes = Some(apex_slides::Notes { text: apex_slides::TextBody::plain("Say hello"), origin: None });
        d
    }

    #[test]
    fn new_file_reads_back() {
        let d = deck();
        let bytes = super::write(&d);
        let back = crate::odp::read(&bytes).unwrap();
        assert_eq!(back.slides.len(), 2);
        assert!((back.width - d.width).abs() < 0.1 && (back.height - d.height).abs() < 0.1);
        assert_eq!(back.slides[0].title(), "Hello ODP");
        let body = back.slides[1].objects.iter().filter_map(|o| o.text()).map(|t| t.text()).collect::<Vec<_>>().join("|");
        assert!(body.contains("One\nTwo"), "{body}");
        assert_eq!(back.slides[1].notes_text(), "Say hello");
    }

    /// Objects hidden and renamed in the Selection Pane keep both.
    #[test]
    fn hidden_objects_are_kept() {
        let mut d = deck();
        let tb = hidden_box();
        d.slide_mut(1).objects.push(tb);
        for (fmt, back) in [("odp", crate::odp::read(&super::write(&d)).unwrap()), ("pptx", crate::pptx::read(&crate::pptx::write(&d)).unwrap())] {
            let o = back.slides[1].objects.iter().find(|o| o.name == "Secret note").expect(fmt);
            assert!(o.hidden, "{fmt}");
            assert_eq!(o.text().map(|t| t.text()).as_deref(), Some("Hidden"), "{fmt}");
            assert!(back.slides[1].objects.iter().filter(|o| o.name != "Secret note").all(|o| !o.hidden), "{fmt}");
        }
    }

    fn hidden_box() -> apex_slides::Object {
        use apex_slides::object::Frame;
        let mut o = apex_slides::Object::text_box(77, Frame { x: 40.0, y: 50.0, w: 200.0, h: 40.0, ..Default::default() }, apex_slides::TextBody::plain("Hidden"));
        o.name = "Secret note".into();
        o.hidden = true;
        o
    }

    #[test]
    fn diagrams_become_groups() {
        use apex_slides::object::{Frame, ObjKind, Object};
        use std::sync::Arc;
        let mut d = deck();
        let dg = crate::diagram::tests::sample();
        let n = dg.shapes.len();
        let mut o = Object::new(90, "Diagram", ObjKind::Diagram(Arc::new(dg)));
        o.sp.frame = Some(Frame { x: 20.0, y: 30.0, w: 480.0, h: 120.0, ..Default::default() });
        d.slide_mut(1).objects.push(o);
        let back = crate::odp::read(&super::write(&d)).unwrap();
        let g = back.slides[1].objects.iter().find(|o| matches!(o.kind, ObjKind::Group(_))).expect("a group");
        let ObjKind::Group(g) = &g.kind else { unreachable!() };
        assert_eq!(g.children.len(), n);
        let text: String = g.children.iter().filter_map(|c| c.text()).map(|t| t.text()).collect();
        assert!(text.contains("Plan"), "{text}");
    }

    #[test]
    fn charts_become_chart_objects() {
        use apex_slides::object::{Frame, ObjKind, Object};
        let mut d = deck();
        let wb = crate::chart_object::sample(apex_sheet::ChartType::Column, "Category", "Series");
        let chart = apex_doc::ChartObject { book: crate::chart_object::book(&wb) };
        let mut o = Object::new(91, "Chart", ObjKind::Chart(chart));
        o.sp.frame = Some(Frame { x: 20.0, y: 30.0, w: 400.0, h: 240.0, ..Default::default() });
        d.slide_mut(1).objects.push(o);
        let bytes = super::write(&d);
        let z = crate::zip::ZipReader::new(&bytes).unwrap();
        let content = z.read_string("content.xml").unwrap();
        assert!(content.contains(r#"<draw:object xlink:href="./Object 1""#), "{content}");
        assert!(z.read_string("Object 1/content.xml").unwrap().contains("<chart:chart"));
        assert!(z.read_string("META-INF/manifest.xml").unwrap().contains(r#"manifest:full-path="Object 1/""#));
        // Read back as a chart, with its series.
        let back = crate::odp::read(&bytes).unwrap();
        let c = back.slides[1].objects.iter().find_map(|o| match &o.kind {
            ObjKind::Chart(c) => Some(c.clone()),
            _ => None,
        });
        let (wb, si, ci) = crate::chart_object::open(&c.expect("a chart").book).unwrap();
        assert_eq!(wb.sheets[si].charts[ci].series.len(), 3);
    }

    #[test]
    fn unchanged_parts_go_back_as_read() {
        let bytes = super::write(&deck());
        let mut d = crate::odp::read(&bytes).unwrap();
        // Unchanged: the same pages.
        let again = super::write(&d);
        let content = |b: &[u8]| crate::zip::ZipReader::new(b).unwrap().read_string("content.xml").unwrap();
        let (a, b) = (content(&bytes), content(&again));
        let pages = |s: &str| s[s.find("<draw:page ").unwrap()..s.rfind("</draw:page>").unwrap()].to_string();
        assert_eq!(pages(&a), pages(&b));
        // A changed slide is written again, the other kept.
        let first = d.slides[0].objects.iter().find(|o| o.is_title()).map(|o| o.id).unwrap();
        if let Some(o) = d.object_mut(0, first) {
            o.touch();
            if let Some(t) = o.text_or_new() {
                t.set_plain("Changed");
            }
        }
        let changed = super::write(&d);
        let back = crate::odp::read(&changed).unwrap();
        assert_eq!(back.slides[0].title(), "Changed");
        assert_eq!(back.slides[1].notes_text(), "Say hello");
        let c = content(&changed);
        let second = &a[a.find("<draw:page ").unwrap()..];
        let second = &second[second.find("</draw:page>").unwrap() + 12..];
        let second = &second[..second.find("</draw:page>").unwrap()];
        assert!(c.contains(second));
    }

    /// Reads, writes and reads again every .odp under $ODP_CORPUS.
    #[test]
    #[ignore]
    fn corpus_round_trip() {
        let Ok(dir) = std::env::var("ODP_CORPUS") else { return };
        let mut files = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            for e in apex_fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "odp") {
                    files.push(p);
                }
            }
        }
        files.sort();
        let (mut ok, mut bad) = (0, 0);
        for f in &files {
            let data = apex_fs::read(f).unwrap();
            let r = std::panic::catch_unwind(|| {
                let d = crate::odp::read(&data).map_err(|e| format!("read: {e}"))?;
                let text = |d: &apex_slides::Deck| d.slides.iter().map(|s| s.objects.iter().filter_map(|o| o.text()).map(|t| t.text()).collect::<String>()).collect::<Vec<_>>();
                let mut changed = d.clone();
                for i in 0..changed.slides.len() {
                    changed.slide_mut(i);
                }
                for deck in [&d, &changed] {
                    let back = crate::odp::read(&super::write(deck)).map_err(|e| format!("reread: {e}"))?;
                    if back.slides.len() != d.slides.len() {
                        return Err(format!("slides {} -> {}", d.slides.len(), back.slides.len()));
                    }
                    if text(&back) != text(&d) {
                        return Err("text differs".to_string());
                    }
                }
                Ok(())
            });
            match r {
                Ok(Ok(())) => ok += 1,
                Ok(Err(e)) => {
                    bad += 1;
                    eprintln!("{}: {e}", f.display());
                }
                Err(_) => {
                    bad += 1;
                    eprintln!("{}: PANIC", f.display());
                }
            }
        }
        eprintln!("odp corpus: {ok} ok, {bad} bad");
    }

    /// Writes $ODP_IN again as $ODP_OUT with every slide and object
    /// marked changed (to compare both in LibreOffice).
    #[test]
    #[ignore]
    fn rewrite_all() {
        let (Ok(src), Ok(dst)) = (std::env::var("ODP_IN"), std::env::var("ODP_OUT")) else { return };
        let mut d = crate::odp::read(&apex_fs::read(src).unwrap()).unwrap();
        for i in 0..d.slides.len() {
            for o in &mut d.slide_mut(i).objects {
                o.detach();
            }
        }
        apex_fs::write(dst, super::write(&d)).unwrap();
    }
}
