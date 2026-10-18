//! The objects on a slide, layout or master: shapes and text boxes,
//! pictures (and videos and sounds), groups, tables, charts, diagrams and
//! what we only show (embedded objects, ink). Each keeps the XML it was
//! read from while it is left unchanged, so that saving back to the same
//! format loses nothing.

use std::sync::Arc;

use crate::fill::{Fill, Line, Shadow, StyleRefs};
use crate::table::Table;
use crate::text::TextBody;

/// Place and size in points, rotation (clockwise degrees) and flips.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rot: f32,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl Frame {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Frame {
        Frame { x, y, w, h, ..Default::default() }
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// The box of the frame once turned (for selections and guides).
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        if self.rot.rem_euclid(180.0).abs() < 0.01 {
            return (self.x, self.y, self.w, self.h);
        }
        let (cx, cy) = self.center();
        let a = self.rot.to_radians();
        let (s, c) = (a.sin().abs(), a.cos().abs());
        let (w, h) = (self.w * c + self.h * s, self.w * s + self.h * c);
        (cx - w / 2.0, cy - h / 2.0, w, h)
    }

    /// Keeps values usable whatever a file said.
    pub fn sanitize(&mut self) {
        let fin = |v: f32, d: f32| if v.is_finite() { v.clamp(-100_000.0, 100_000.0) } else { d };
        self.x = fin(self.x, 0.0);
        self.y = fin(self.y, 0.0);
        self.w = fin(self.w, 0.0).max(0.0);
        self.h = fin(self.h, 0.0).max(0.0);
        self.rot = fin(self.rot, 0.0).rem_euclid(360.0);
    }
}

/// Kinds of placeholders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PhType {
    Title,
    CtrTitle,
    SubTitle,
    Body,
    Obj,
    Date,
    Footer,
    SlideNum,
    Header,
    Pic,
    Chart,
    Table,
    Media,
    ClipArt,
    Diagram,
    SlideImage,
}

impl PhType {
    pub const ALL: [(PhType, &'static str); 16] = [
        (PhType::Title, "title"),
        (PhType::CtrTitle, "ctrTitle"),
        (PhType::SubTitle, "subTitle"),
        (PhType::Body, "body"),
        (PhType::Obj, "obj"),
        (PhType::Date, "dt"),
        (PhType::Footer, "ftr"),
        (PhType::SlideNum, "sldNum"),
        (PhType::Header, "hdr"),
        (PhType::Pic, "pic"),
        (PhType::Chart, "chart"),
        (PhType::Table, "tbl"),
        (PhType::Media, "media"),
        (PhType::ClipArt, "clipArt"),
        (PhType::Diagram, "dgm"),
        (PhType::SlideImage, "sldImg"),
    ];

    pub fn parse(s: &str) -> PhType {
        PhType::ALL.iter().find(|(_, n)| *n == s).map_or(PhType::Obj, |p| p.0)
    }

    pub fn name(self) -> &'static str {
        PhType::ALL.iter().find(|(p, _)| *p == self).map_or("obj", |p| p.1)
    }

    pub fn is_title(self) -> bool {
        matches!(self, PhType::Title | PhType::CtrTitle)
    }

    /// Placeholders whose text uses the master's body style.
    pub fn is_body(self) -> bool {
        matches!(self, PhType::Body | PhType::Obj | PhType::SubTitle | PhType::Pic | PhType::Chart | PhType::Table | PhType::Media | PhType::ClipArt | PhType::Diagram)
    }

    /// Header and footer placeholders (date, footer, slide number).
    pub fn is_hf(self) -> bool {
        matches!(self, PhType::Date | PhType::Footer | PhType::SlideNum | PhType::Header)
    }

    /// The master placeholder a layout's placeholder of this kind follows.
    pub fn master_kind(self) -> PhType {
        match self {
            PhType::CtrTitle | PhType::Title => PhType::Title,
            k if k.is_hf() => k,
            _ => PhType::Body,
        }
    }
}

/// A placeholder: its kind and index (a slide's placeholder takes place,
/// size and formatting from the layout's with the same index or kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Placeholder {
    pub kind: PhType,
    pub idx: u32,
    /// Its prompt text on the layout was changed ("Click to add title").
    pub custom_prompt: bool,
    /// Vertical ("vert") text.
    pub vert: bool,
}

impl Placeholder {
    pub fn new(kind: PhType, idx: u32) -> Placeholder {
        Placeholder { kind, idx, custom_prompt: false, vert: false }
    }
}

/// A shape's geometry.
#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    /// A preset of the standard ("rect", "roundRect", "rightArrow"...) and
    /// its adjust values.
    Preset(Arc<str>, Vec<(Arc<str>, i64)>),
    /// Drawn paths (`a:custGeom`), kept as written.
    Custom(Arc<str>),
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry::Preset(Arc::from("rect"), Vec::new())
    }
}

impl Geometry {
    pub fn preset(name: &str) -> Geometry {
        Geometry::Preset(Arc::from(name), Vec::new())
    }

    pub fn preset_name(&self) -> Option<&str> {
        match self {
            Geometry::Preset(n, _) => Some(n),
            Geometry::Custom(_) => None,
        }
    }

    pub fn is_line(&self) -> bool {
        matches!(self.preset_name(), Some("line" | "straightConnector1" | "bentConnector2" | "bentConnector3" | "bentConnector4" | "bentConnector5" | "curvedConnector2" | "curvedConnector3" | "curvedConnector4" | "curvedConnector5"))
    }
}

/// A shape's look (`p:spPr`): unset values come from its style references
/// or, for placeholders, from the layout and the master.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeProps {
    pub frame: Option<Frame>,
    pub geom: Option<Geometry>,
    pub fill: Option<Fill>,
    pub line: Option<Line>,
    pub shadow: Option<Shadow>,
    /// Effects, 3D and extensions as written (`a:effectLst`, `a:scene3d`...).
    pub extra: Vec<Arc<str>>,
}

/// A video or sound on a picture.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Media {
    pub video: bool,
    /// The file when it is in the presentation.
    pub data: Option<Arc<[u8]>>,
    /// A linked file (path or URL) otherwise.
    pub link: Option<String>,
    /// Media type ("video/mp4").
    pub mime: String,
    /// Trimmed from the start and the end, seconds.
    pub trim: (f32, f32),
    /// Plays by itself when the slide shows (else on a click). The show
    /// follows the slide's timing (a [`crate::anim::Class::Media`]
    /// animation); this says what was read or last set.
    pub autoplay: bool,
    pub looped: bool,
    /// 0..1.
    pub volume: f32,
    pub mute: bool,
}

impl Media {
    /// A video or sound held in the presentation, playing on a click.
    pub fn embedded(data: Arc<[u8]>, mime: &str, video: bool) -> Media {
        Media { video, data: Some(data), link: None, mime: mime.to_string(), trim: (0.0, 0.0), autoplay: false, looped: false, volume: 1.0, mute: false }
    }
}

/// A picture on a slide.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Picture {
    /// The picture (its size and rotation are the object's frame).
    pub image: apex_doc::Image,
    pub media: Option<Media>,
}

/// A group: its children are placed in the child box `child` (their
/// coordinates), which is stretched onto the group's frame.
#[derive(Clone, Debug, Default)]
pub struct Group {
    pub children: Vec<Object>,
    pub child: Frame,
}

#[derive(Clone, Debug)]
pub enum ObjKind {
    /// A shape, text box or connector, with its text.
    Shape { text: Option<TextBody>, text_box: bool, connector: bool },
    Picture(Box<Picture>),
    Group(Box<Group>),
    Table(Box<Table>),
    /// A chart with its data: a small workbook, as documents keep them.
    Chart(apex_doc::ChartObject),
    /// A diagram: its parts and the drawing Office saved.
    Diagram(Arc<apex_doc::Diagram>),
    /// An equation (OMML, as `apex_doc::math` reads it).
    Math(Arc<apex_doc::math::Equation>),
    /// Something only shown (an embedded object, ink...): its preview
    /// picture if any, and the XML it was read from, written back (with
    /// its new place) even once moved.
    Other { preview: Option<Arc<[u8]>>, what: Arc<str>, xml: Option<Raw> },
}

/// What format a piece of XML kept from a file belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Src {
    Pptx,
    Odp,
}

/// XML as read, written back while the thing it came from is unchanged.
#[derive(Clone, Debug, PartialEq)]
pub struct Raw {
    pub src: Src,
    pub xml: Arc<str>,
}

#[derive(Clone, Debug)]
pub struct Object {
    /// Unique on its slide (DrawingML `cNvPr id`).
    pub id: u32,
    pub name: String,
    /// Alternative text.
    pub descr: String,
    pub hidden: bool,
    pub ph: Option<Placeholder>,
    pub sp: ShapeProps,
    pub style: Option<StyleRefs>,
    pub kind: ObjKind,
    /// Hyperlink when clicked: a URL, or "#slide:N" for a slide (1-based)
    /// and "#next", "#prev", "#first", "#last", "#end".
    pub click: Option<Arc<str>>,
    /// The XML it was read from; dropped when it changes.
    pub raw: Option<Raw>,
}

impl Object {
    pub fn new(id: u32, name: impl Into<String>, kind: ObjKind) -> Object {
        Object { id, name: name.into(), descr: String::new(), hidden: false, ph: None, sp: ShapeProps::default(), style: None, kind, click: None, raw: None }
    }

    /// A text box.
    pub fn text_box(id: u32, frame: Frame, text: TextBody) -> Object {
        let mut o = Object::new(id, format!("TextBox {}", id.saturating_sub(1)), ObjKind::Shape { text: Some(text), text_box: true, connector: false });
        o.sp.frame = Some(frame);
        o.sp.geom = Some(Geometry::preset("rect"));
        o.sp.fill = Some(Fill::None);
        o
    }

    pub fn text(&self) -> Option<&TextBody> {
        match &self.kind {
            ObjKind::Shape { text, .. } => text.as_ref(),
            _ => None,
        }
    }

    pub fn text_mut(&mut self) -> Option<&mut TextBody> {
        match &mut self.kind {
            ObjKind::Shape { text, .. } => text.as_mut(),
            _ => None,
        }
    }

    /// The text, made empty first if the shape has none (lines have none).
    pub fn text_or_new(&mut self) -> Option<&mut TextBody> {
        let line = self.sp.geom.as_ref().is_some_and(Geometry::is_line);
        match &mut self.kind {
            ObjKind::Shape { text, connector: false, .. } if !line => Some(text.get_or_insert_with(|| TextBody::plain(""))),
            _ => None,
        }
    }

    pub fn is_placeholder(&self, kind: PhType) -> bool {
        self.ph.is_some_and(|p| p.kind == kind)
    }

    pub fn is_title(&self) -> bool {
        self.ph.is_some_and(|p| p.kind.is_title())
    }

    /// Visits this object and those in it, depth first.
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a Object)) {
        f(self);
        if let ObjKind::Group(g) = &self.kind {
            for c in &g.children {
                c.walk(f);
            }
        }
    }

    pub fn walk_mut(&mut self, f: &mut dyn FnMut(&mut Object)) {
        f(self);
        if let ObjKind::Group(g) = &mut self.kind {
            for c in &mut g.children {
                c.walk_mut(f);
            }
        }
    }

    /// The largest id in it.
    pub fn max_id(&self) -> u32 {
        let mut m = 0;
        self.walk(&mut |o| m = m.max(o.id));
        m
    }

    /// Drops what was kept from the file (it changed), here and in the
    /// groups it is in are the caller's to drop.
    pub fn touch(&mut self) {
        self.raw = None;
    }

    /// Drops everything kept from the file, for a copy put on another
    /// slide (the XML kept points at the relationships of its part).
    pub fn detach(&mut self) {
        self.walk_mut(&mut |o| {
            o.raw = None;
            if let ObjKind::Other { xml, .. } = &mut o.kind {
                *xml = None;
            }
        });
    }

    /// The tables' and shapes' text bodies, for finding and spelling.
    pub fn text_bodies(&self) -> Vec<&TextBody> {
        let mut out = Vec::new();
        self.walk(&mut |o| match &o.kind {
            ObjKind::Shape { text: Some(t), .. } => out.push(t),
            ObjKind::Table(t) => {
                for r in &t.rows {
                    for c in &r.cells {
                        out.push(&c.text);
                    }
                }
            }
            _ => {}
        });
        out
    }

    /// Every text body in it, to change (finding and replacing).
    pub fn text_bodies_mut(&mut self, f: &mut dyn FnMut(&mut TextBody) -> bool) -> bool {
        let mut changed = false;
        self.walk_mut(&mut |o| {
            let mut here = false;
            match &mut o.kind {
                ObjKind::Shape { text: Some(t), .. } => here |= f(t),
                ObjKind::Table(t) => {
                    for r in &mut t.rows {
                        for c in &mut r.cells {
                            here |= f(&mut c.text);
                        }
                    }
                }
                _ => {}
            }
            if here {
                o.raw = None;
                changed = true;
            }
        });
        if changed {
            self.raw = None;
        }
        changed
    }
}
