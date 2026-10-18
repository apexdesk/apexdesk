//! Annotations: reading the ones a page has, adding new ones (highlight,
//! underline, strike out, notes, text boxes, ink, lines, arrows,
//! rectangles, ovals and stamps) with their appearance, changing, moving
//! and deleting them, and replies.

use super::super::obj::{Dict, Obj};
use super::appearance::{self as ap, n};
use super::{Geom, PdfDoc, obj_text, text_obj};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    /// A sticky note (`Text`).
    Note,
    FreeText,
    Ink,
    Line,
    Arrow,
    Square,
    Circle,
    Stamp,
    Link,
    Widget,
    Popup,
    Redact,
    Other,
}

impl Kind {
    fn subtype(self) -> &'static str {
        match self {
            Kind::Highlight => "Highlight",
            Kind::Underline => "Underline",
            Kind::StrikeOut => "StrikeOut",
            Kind::Squiggly => "Squiggly",
            Kind::Note => "Text",
            Kind::FreeText => "FreeText",
            Kind::Ink => "Ink",
            Kind::Line | Kind::Arrow => "Line",
            Kind::Square => "Square",
            Kind::Circle => "Circle",
            Kind::Stamp => "Stamp",
            Kind::Link => "Link",
            Kind::Widget => "Widget",
            Kind::Popup => "Popup",
            Kind::Redact => "Redact",
            Kind::Other => "Unknown",
        }
    }

    /// Text markup: drawn over the text's quadrilaterals.
    pub fn is_markup_text(self) -> bool {
        matches!(self, Kind::Highlight | Kind::Underline | Kind::StrikeOut | Kind::Squiggly | Kind::Redact)
    }

    /// Shown in the comment list.
    pub fn is_comment(self) -> bool {
        !matches!(self, Kind::Link | Kind::Widget | Kind::Popup | Kind::Other)
    }

    /// Appearances this module draws (the others keep theirs).
    pub fn drawn_here(self) -> bool {
        !matches!(self, Kind::Link | Kind::Widget | Kind::Popup | Kind::Other)
    }
}

/// A picture for stamps (visible signatures): RGBA pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// An annotation; positions are points shown (see [`super`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Annot {
    /// Its object number (0: not yet added, or one without its own object).
    pub id: u32,
    pub page: usize,
    pub kind: Kind,
    pub rect: [f32; 4],
    pub color: u32,
    /// Interior colour (rectangles, ovals, text boxes).
    pub fill: Option<u32>,
    /// Line width.
    pub width: f32,
    pub opacity: f32,
    pub contents: String,
    pub author: String,
    pub date: String,
    /// The annotation it answers.
    pub reply_to: Option<u32>,
    /// Text markup areas.
    pub quads: Vec<[f32; 4]>,
    pub ink: Vec<Vec<[f32; 2]>>,
    /// A line's ends: x1, y1, x2, y2.
    pub line: Option<[f32; 4]>,
    pub font_size: f32,
    /// A stamp's text.
    pub stamp: String,
    /// A stamp's picture (only when adding).
    pub picture: Option<Picture>,
    pub hidden: bool,
    pub locked: bool,
}

impl Default for Annot {
    fn default() -> Annot {
        Annot {
            id: 0,
            page: 0,
            kind: Kind::Note,
            rect: [0.0; 4],
            color: 0xFFD400,
            fill: None,
            width: 1.5,
            opacity: 1.0,
            contents: String::new(),
            author: String::new(),
            date: String::new(),
            reply_to: None,
            quads: Vec::new(),
            ink: Vec::new(),
            line: None,
            font_size: 12.0,
            stamp: String::new(),
            picture: None,
            hidden: false,
            locked: false,
        }
    }
}

/// Size of a sticky note's icon.
pub const NOTE_SIZE: f32 = 20.0;

/// The stamps PDF names (ISO 32000, 12.5.6.12).
const STANDARD_STAMPS: [&str; 14] = ["Approved", "Experimental", "NotApproved", "AsIs", "Expired", "NotForPublicRelease", "Confidential", "Final", "Sold", "Departmental", "ForComment", "TopSecret", "Draft", "ForPublicRelease"];

/// A stamp's words from its name: "NotApproved" is "Not Approved"; a
/// name with spaces is as it is.
fn stamp_text(name: &str) -> String {
    if name.contains(' ') {
        return name.to_string();
    }
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_uppercase() && !out.ends_with(' ') {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn union(rs: impl Iterator<Item = [f32; 4]>) -> Option<[f32; 4]> {
    rs.reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
}

impl Annot {
    /// The box its appearance covers: the rectangle, or around its
    /// quadrilaterals, strokes or line.
    pub fn bounds(&self) -> [f32; 4] {
        let pad = self.width.max(1.0) * 2.0 + 2.0;
        let grow = |r: [f32; 4], p: f32| [r[0] - p, r[1] - p, r[2] + p, r[3] + p];
        match self.kind {
            k if k.is_markup_text() && !self.quads.is_empty() => union(self.quads.iter().copied()).map_or(self.rect, |r| grow(r, 1.0)),
            Kind::Ink if !self.ink.is_empty() => union(self.ink.iter().flatten().map(|p| [p[0], p[1], p[0], p[1]])).map_or(self.rect, |r| grow(r, pad)),
            Kind::Line | Kind::Arrow => match self.line {
                Some(l) => grow([l[0].min(l[2]), l[1].min(l[3]), l[0].max(l[2]), l[1].max(l[3])], pad + if self.kind == Kind::Arrow { self.width * 3.0 + 4.0 } else { 0.0 }),
                None => self.rect,
            },
            _ => self.rect,
        }
    }

    /// The same annotation moved by (dx, dy).
    pub fn moved(&self, dx: f32, dy: f32) -> Annot {
        let mut a = self.clone();
        let sh = |r: [f32; 4]| [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy];
        a.rect = sh(a.rect);
        a.quads = a.quads.iter().map(|q| sh(*q)).collect();
        a.ink = a.ink.iter().map(|s| s.iter().map(|p| [p[0] + dx, p[1] + dy]).collect()).collect();
        a.line = a.line.map(sh);
        a
    }
}

/// The dictionary's flags.
const HIDDEN: i64 = 2;
const PRINT: i64 = 4;
const LOCKED: i64 = 128;

impl PdfDoc {
    /// The page's annotations list: its own array object (if it has one)
    /// and the entries.
    pub(super) fn annots_of(&self, page: u32) -> (Option<u32>, Vec<Obj>) {
        let Some(d) = self.dict(page) else { return (None, Vec::new()) };
        match d.get("Annots") {
            Some(Obj::Ref(n, _)) => (Some(*n), self.get(*n).and_then(Obj::array).map(<[Obj]>::to_vec).unwrap_or_default()),
            Some(Obj::Array(a)) => (None, a.clone()),
            _ => (None, Vec::new()),
        }
    }

    pub(super) fn set_annots_of(&mut self, page: u32, list: Vec<Obj>) {
        let (own, _) = self.annots_of(page);
        match own {
            Some(n) => self.set(n, Obj::Array(list)),
            None => {
                if let Some(mut d) = self.dict(page) {
                    if list.is_empty() {
                        d.remove("Annots");
                    } else {
                        d.set("Annots", Obj::Array(list));
                    }
                    self.set(page, Obj::Dict(d));
                }
            }
        }
    }

    /// The annotations of page `i`.
    pub fn annots(&self, i: usize) -> Vec<Annot> {
        let pages = self.pages();
        let Some(&pn) = pages.get(i) else { return Vec::new() };
        let g = self.geom(pn);
        let (_, list) = self.annots_of(pn);
        list.iter().take(10_000).filter_map(|o| {
            let id = match o {
                Obj::Ref(n, _) => *n,
                _ => 0,
            };
            let d = self.resolve(o).dict().cloned()?;
            Some(self.read_annot(id, i, &g, &d))
        }).collect()
    }

    /// Every page's annotations.
    pub fn all_annots(&self) -> Vec<Annot> {
        (0..self.pages().len()).flat_map(|i| self.annots(i)).collect()
    }

    fn read_annot(&self, id: u32, page: usize, g: &Geom, d: &Dict) -> Annot {
        let sub = self.key(d, "Subtype");
        let le_arrow = matches!(self.key(d, "LE"), Obj::Array(a) if a.iter().any(|x| x.name().is_some_and(|n| n != "None")));
        let kind = match sub.name().unwrap_or("") {
            "Highlight" => Kind::Highlight,
            "Underline" => Kind::Underline,
            "StrikeOut" => Kind::StrikeOut,
            "Squiggly" => Kind::Squiggly,
            "Text" => Kind::Note,
            "FreeText" => Kind::FreeText,
            "Ink" => Kind::Ink,
            "Line" if le_arrow => Kind::Arrow,
            "Line" => Kind::Line,
            "Square" => Kind::Square,
            "Circle" => Kind::Circle,
            "Stamp" => Kind::Stamp,
            "Link" => Kind::Link,
            "Widget" => Kind::Widget,
            "Popup" => Kind::Popup,
            "Redact" => Kind::Redact,
            _ => Kind::Other,
        };
        let r = self.nums(d.get("Rect").unwrap_or(&Obj::Null));
        let rect = if r.len() == 4 { g.rect_to_view([r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]) } else { [0.0; 4] };
        let color = |k: &str| ap::color_of(&self.nums(d.get(k).unwrap_or(&Obj::Null)));
        let width = match self.key_dict(d, "BS").and_then(|bs| self.key(&bs, "W").num()) {
            Some(w) => w,
            None => match self.key(d, "Border") {
                Obj::Array(a) if a.len() >= 3 => a[2].num().unwrap_or(1.0),
                _ => 1.0,
            },
        };
        let flags = self.key(d, "F").int().unwrap_or(0);
        let pts = |o: &Obj| -> Vec<[f32; 2]> {
            self.nums(o).chunks_exact(2).map(|p| {
                let v = g.to_view(p[0], p[1]);
                [v[0] as f32, v[1] as f32]
            }).collect()
        };
        let quads: Vec<[f32; 4]> = pts(d.get("QuadPoints").unwrap_or(&Obj::Null)).chunks_exact(4).filter_map(|q| union(q.iter().map(|p| [p[0], p[1], p[0], p[1]]))).collect();
        let ink = match self.key(d, "InkList") {
            Obj::Array(a) => a.iter().map(|s| pts(s)).collect(),
            _ => Vec::new(),
        };
        let line = {
            let l = pts(d.get("L").unwrap_or(&Obj::Null));
            (l.len() == 2).then(|| [l[0][0], l[0][1], l[1][0], l[1][1]])
        };
        let font_size = obj_text(&self.key(d, "DA")).split_whitespace().collect::<Vec<_>>().windows(2).find(|w| w[1] == "Tf").and_then(|w| w[0].parse::<f32>().ok()).unwrap_or(12.0);
        let reply_to = match d.get("IRT") {
            Some(Obj::Ref(n, _)) => Some(*n),
            _ => None,
        };
        let default_color = match kind {
            Kind::Highlight => 0xFFFF00,
            Kind::Note => 0xFFD400,
            _ => 0,
        };
        Annot {
            id,
            page,
            kind,
            rect,
            color: color("C").unwrap_or(default_color),
            fill: color("IC"),
            width: width as f32,
            opacity: self.key(d, "CA").num().unwrap_or(1.0) as f32,
            contents: obj_text(&self.key(d, "Contents")),
            author: obj_text(&self.key(d, "T")),
            date: obj_text(&self.key(d, "M")),
            reply_to,
            quads,
            ink,
            line,
            font_size: if font_size > 0.0 { font_size } else { 12.0 },
            stamp: self.key(d, "Name").name().map(stamp_text).unwrap_or_default(),
            picture: None,
            hidden: flags & HIDDEN != 0,
            locked: flags & LOCKED != 0,
        }
    }

    /// Adds `a` to its page (one undo step); returns its object number.
    pub fn add_annot(&mut self, a: &Annot) -> u32 {
        let pages = self.pages();
        let Some(&pn) = pages.get(a.page) else { return 0 };
        self.begin("pdf.undo.annotate");
        let n = self.add(Obj::Null);
        let d = self.annot_dict(a, pn, None);
        self.set(n, Obj::Dict(d));
        self.set_appearance(n, a, pn);
        let (_, mut list) = self.annots_of(pn);
        list.push(Obj::Ref(n, 0));
        self.set_annots_of(pn, list);
        self.end();
        n
    }

    /// Writes `a` over the annotation it is (same object).
    pub fn update_annot(&mut self, a: &Annot) {
        let pages = self.pages();
        let Some(&pn) = pages.get(a.page) else { return };
        let Some(old) = self.dict(a.id) else { return };
        self.begin("pdf.undo.annot_change");
        let g = self.geom(pn);
        let before = self.read_annot(a.id, a.page, &g, &old);
        if a.kind.drawn_here() && !(a.kind == Kind::Stamp && self.has_picture(&old)) {
            let d = self.annot_dict(a, pn, Some(&old));
            self.set(a.id, Obj::Dict(d));
            self.set_appearance(a.id, a, pn);
        } else {
            // Kept appearances follow the rectangle when it moves.
            let mut d = old.clone();
            let (dx, dy) = (a.rect[0] - before.rect[0], a.rect[1] - before.rect[1]);
            if dx != 0.0 || dy != 0.0 {
                let moved = before.moved(dx, dy);
                d.set("Rect", rect_obj(g.rect_to_user(moved.rect)));
            }
            d.set("Contents", text_obj(&a.contents));
            d.set("M", text_obj(&super::pdf_date_now()));
            self.set(a.id, Obj::Dict(d));
        }
        self.end();
    }

    fn has_picture(&self, d: &Dict) -> bool {
        self.key_dict(d, "AP").and_then(|ap| self.key_dict(&ap, "N")).and_then(|n| self.key_dict(&n, "Resources")).and_then(|r| self.key_dict(&r, "XObject")).is_some()
    }

    /// Deletes an annotation, its pop-up and its replies.
    pub fn delete_annot(&mut self, id: u32) {
        let pages = self.pages();
        self.begin("pdf.undo.annot_delete");
        self.removing();
        let mut gone = vec![id];
        // Replies, and replies to them.
        let mut i = 0;
        while i < gone.len() {
            let target = gone[i];
            for &pn in &pages {
                for o in self.annots_of(pn).1 {
                    if let Obj::Ref(m, _) = o
                        && let Some(d) = self.dict(m)
                        && matches!(d.get("IRT"), Some(Obj::Ref(t, _)) if *t == target)
                        && !gone.contains(&m)
                    {
                        gone.push(m);
                    }
                }
            }
            i += 1;
        }
        let popups: Vec<u32> = gone.iter().filter_map(|&g| match self.dict(g)?.get("Popup") {
            Some(Obj::Ref(p, _)) => Some(*p),
            _ => None,
        }).collect();
        gone.extend(popups);
        for &pn in &pages {
            let (_, list) = self.annots_of(pn);
            let kept: Vec<Obj> = list.iter().filter(|o| !matches!(o, Obj::Ref(m, _) if gone.contains(m))).cloned().collect();
            if kept.len() != list.len() {
                self.set_annots_of(pn, kept);
            }
        }
        // A form field left without its widgets goes with its value, and
        // nothing else (structure tree, other fields) still leads to them.
        let gone: std::collections::HashSet<u32> = gone.into_iter().collect();
        if let Some((_, cat)) = self.catalog()
            && let Some(mut form) = self.key_dict(&cat, "AcroForm")
            && let Obj::Array(fields) = self.key(&form, "Fields")
            && let Some(kept) = self.prune_fields(&fields, &gone, 0)
        {
            form.set("Fields", Obj::Array(kept));
            self.set_catalog("AcroForm", Some(Obj::Dict(form)));
        }
        self.forget(&gone);
        self.end();
    }

    /// Answers annotation `id`; returns the reply's number.
    pub fn reply(&mut self, id: u32, text: &str, author: &str) -> u32 {
        let Some(page) = self.page_of_annot(id) else { return 0 };
        let pn = self.pages()[page];
        let g = self.geom(pn);
        let parent = self.dict(id).map(|d| self.read_annot(id, page, &g, &d)).unwrap_or_default();
        let r = Annot { page, kind: Kind::Note, rect: [parent.rect[0], parent.rect[1], parent.rect[0] + NOTE_SIZE, parent.rect[1] + NOTE_SIZE], contents: text.to_string(), author: author.to_string(), reply_to: Some(id), ..Default::default() };
        self.begin("pdf.undo.reply");
        let n = self.add(Obj::Null);
        let mut d = self.annot_dict(&r, pn, None);
        d.set("IRT", Obj::Ref(id, 0));
        d.set("RT", Obj::name_of("R"));
        // Replies show in the comment list, not on the page.
        d.set("AP", Obj::Dict(Dict::of(vec![("N", Obj::Ref(self.add(ap::form(1.0, 1.0, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], b"", Dict::default())), 0))])));
        self.set(n, Obj::Dict(d));
        let (_, mut list) = self.annots_of(pn);
        list.push(Obj::Ref(n, 0));
        self.set_annots_of(pn, list);
        self.end();
        n
    }

    /// The page an annotation is on.
    pub fn page_of_annot(&self, id: u32) -> Option<usize> {
        self.pages().iter().position(|&pn| self.annots_of(pn).1.iter().any(|o| matches!(o, Obj::Ref(m, _) if *m == id)))
    }

    /// The annotation's dictionary (keeping what `old` had that isn't
    /// set here).
    fn annot_dict(&self, a: &Annot, page: u32, old: Option<&Dict>) -> Dict {
        let g = self.geom(page);
        let mut d = old.cloned().unwrap_or_default();
        let now = super::pdf_date_now();
        d.set("Type", Obj::name_of("Annot"));
        d.set("Subtype", Obj::name_of(a.kind.subtype()));
        d.set("Rect", rect_obj(g.rect_to_user(a.bounds())));
        d.set("P", Obj::Ref(page, 0));
        if old.is_none() {
            d.set("NM", text_obj(&format!("apex-{}", hex(&crate::crypto::random(8)))));
            d.set("CreationDate", text_obj(&now));
        }
        d.set("M", text_obj(&now));
        let mut flags = old.and_then(|o| o.get("F")).and_then(Obj::int).unwrap_or(PRINT);
        flags = if a.hidden { flags | HIDDEN } else { flags & !HIDDEN };
        flags = if a.locked { flags | LOCKED } else { flags & !LOCKED };
        d.set("F", Obj::Int(flags));
        d.set("C", ap::color_obj(a.color));
        if (a.opacity - 1.0).abs() > 0.001 {
            d.set("CA", Obj::Real(ap::round(a.opacity as f64)));
        } else {
            d.remove("CA");
        }
        if !a.author.is_empty() {
            d.set("T", text_obj(&a.author));
        }
        d.set("Contents", text_obj(&a.contents));
        let pts = |ps: &[[f32; 2]]| Obj::Array(ps.iter().flat_map(|p| {
            let u = g.to_user(p[0] as f64, p[1] as f64);
            [Obj::Real(ap::round(u[0])), Obj::Real(ap::round(u[1]))]
        }).collect());
        match a.kind {
            k if k.is_markup_text() => {
                let q: Vec<[f32; 2]> = a.quads.iter().flat_map(|r| [[r[0], r[1]], [r[2], r[1]], [r[0], r[3]], [r[2], r[3]]]).collect();
                d.set("QuadPoints", pts(&q));
                if k == Kind::Redact {
                    d.set("IC", ap::color_obj(0));
                }
            }
            Kind::Ink => {
                d.set("InkList", Obj::Array(a.ink.iter().map(|s| pts(s)).collect()));
                d.set("BS", Obj::Dict(Dict::of(vec![("W", Obj::Real(ap::round(a.width as f64)))])));
            }
            Kind::Line | Kind::Arrow => {
                let l = a.line.unwrap_or(a.rect);
                d.set("L", pts(&[[l[0], l[1]], [l[2], l[3]]]));
                d.set("BS", Obj::Dict(Dict::of(vec![("W", Obj::Real(ap::round(a.width as f64)))])));
                if a.kind == Kind::Arrow {
                    d.set("LE", Obj::Array(vec![Obj::name_of("None"), Obj::name_of("OpenArrow")]));
                } else {
                    d.remove("LE");
                }
            }
            Kind::Square | Kind::Circle => {
                d.set("BS", Obj::Dict(Dict::of(vec![("W", Obj::Real(ap::round(a.width as f64)))])));
                match a.fill {
                    Some(f) => d.set("IC", ap::color_obj(f)),
                    None => {
                        d.remove("IC");
                    }
                }
            }
            Kind::FreeText => {
                d.set("DA", text_obj(&format!("/{} {} Tf {} rg", ap::HELV, n(a.font_size as f64), ap::rgb(a.color))));
                d.set("BS", Obj::Dict(Dict::of(vec![("W", Obj::Real(ap::round(a.width as f64)))])));
                match a.fill {
                    Some(f) => d.set("IC", ap::color_obj(f)),
                    None => {
                        d.remove("IC");
                    }
                }
            }
            Kind::Note => {
                d.set("Name", Obj::name_of("Comment"));
                d.set("Open", Obj::Bool(false));
            }
            Kind::Stamp => {
                // A standard stamp by its standard name ("NotApproved"),
                // another by its words (spaces kept, see `stamp_text`).
                let bare: String = a.stamp.chars().filter(|c| c.is_alphanumeric()).collect();
                let name = STANDARD_STAMPS.iter().find(|s| s.eq_ignore_ascii_case(&bare)).map_or_else(|| a.stamp.trim().to_string(), |s| s.to_string());
                d.set("Name", Obj::name_of(if name.is_empty() { "Draft" } else { &name }));
            }
            _ => {}
        }
        d
    }

    /// Draws the annotation's normal appearance.
    fn set_appearance(&mut self, id: u32, a: &Annot, page: u32) {
        let g = self.geom(page);
        let b = a.bounds();
        let (w, h) = ((b[2] - b[0]) as f64, (b[3] - b[1]) as f64);
        // Local coordinates: from the box's bottom left, y up, as shown.
        let lx = |x: f32| (x - b[0]) as f64;
        let ly = |y: f32| (b[3] - y) as f64;
        let mut s = String::new();
        let mut blend = None;
        let alpha = ((a.opacity - 1.0).abs() > 0.001).then_some(a.opacity as f64);
        let lw = a.width.max(0.1) as f64;
        let mut xobjects: Vec<(String, u32)> = Vec::new();
        match a.kind {
            Kind::Highlight => {
                blend = Some("Multiply");
                s.push_str(&format!("{} rg\n", ap::rgb(a.color)));
                for q in &a.quads {
                    s.push_str(&format!("{} {} {} {} re f\n", n(lx(q[0])), n(ly(q[3])), n((q[2] - q[0]) as f64), n((q[3] - q[1]) as f64)));
                }
            }
            Kind::Underline | Kind::StrikeOut | Kind::Squiggly => {
                s.push_str(&format!("{} RG\n", ap::rgb(a.color)));
                for q in &a.quads {
                    let qh = (q[3] - q[1]) as f64;
                    let t = (qh / 14.0).max(0.5);
                    let y = match a.kind {
                        Kind::StrikeOut => ly(q[3]) + qh * 0.45,
                        _ => ly(q[3]) + t,
                    };
                    if a.kind == Kind::Squiggly {
                        let step = (qh / 6.0).max(1.5);
                        let mut x = lx(q[0]);
                        let mut up = true;
                        s.push_str(&format!("{} w {} {} m\n", n(t), n(x), n(y)));
                        while x < lx(q[2]) {
                            x += step;
                            s.push_str(&format!("{} {} l\n", n(x), n(if up { y + step } else { y })));
                            up = !up;
                        }
                        s.push_str("S\n");
                    } else {
                        s.push_str(&format!("{} w {} {} m {} {} l S\n", n(t), n(lx(q[0])), n(y), n(lx(q[2])), n(y)));
                    }
                }
            }
            Kind::Redact => {
                s.push_str(&format!("{} RG 1 w\n", ap::rgb(0xFF0000)));
                for q in &a.quads {
                    s.push_str(&format!("{} {} {} {} re S\n", n(lx(q[0])), n(ly(q[3])), n((q[2] - q[0]) as f64), n((q[3] - q[1]) as f64)));
                }
            }
            Kind::Note => {
                // A folded note with lines.
                let (x1, y1) = (w - 1.0, h - 1.0);
                s.push_str(&format!("{} rg 0.25 G 0.8 w\n1 1 m {} 1 l {} {} l {} {} l 1 {} l h B\n", ap::rgb(a.color), n(x1), n(x1), n(y1 - 5.0), n(x1 - 5.0), n(y1), n(y1)));
                s.push_str(&format!("{} {} m {} {} l {} {} l S\n", n(x1 - 5.0), n(y1), n(x1 - 5.0), n(y1 - 5.0), n(x1), n(y1 - 5.0)));
                for i in 0..3 {
                    let y = h * 0.62 - i as f64 * h * 0.18;
                    s.push_str(&format!("{} {} m {} {} l S\n", n(4.0), n(y), n(w - 7.0), n(y)));
                }
            }
            Kind::FreeText => {
                if let Some(f) = a.fill {
                    s.push_str(&format!("{} rg 0 0 {} {} re f\n", ap::rgb(f), n(w), n(h)));
                }
                if a.width > 0.0 {
                    s.push_str(&format!("{} RG {} w {} {} {} {} re S\n", ap::rgb(a.color), n(lw), n(lw / 2.0), n(lw / 2.0), n(w - lw), n(h - lw)));
                }
                let pad = 2.0 + lw;
                let lines = ap::wrap(&a.contents, w - 2.0 * pad, a.font_size as f64, false);
                s.push_str(&format!("q 0 0 {} {} re W n\n", n(w), n(h)));
                s.push_str(&ap::text_block(&lines, w, h, a.font_size as f64, a.color, pad, 0));
                s.push_str("Q\n");
            }
            Kind::Ink => {
                s.push_str(&format!("{} RG {} w 1 J 1 j\n", ap::rgb(a.color), n(lw)));
                for stroke in &a.ink {
                    for (i, p) in stroke.iter().enumerate() {
                        s.push_str(&format!("{} {} {}\n", n(lx(p[0])), n(ly(p[1])), if i == 0 { "m" } else { "l" }));
                    }
                    if stroke.len() == 1 {
                        s.push_str(&format!("{} {} l\n", n(lx(stroke[0][0]) + 0.1), n(ly(stroke[0][1]))));
                    }
                    s.push_str("S\n");
                }
            }
            Kind::Line | Kind::Arrow => {
                let l = a.line.unwrap_or(a.rect);
                let (x1, y1, x2, y2) = (lx(l[0]), ly(l[1]), lx(l[2]), ly(l[3]));
                s.push_str(&format!("{} RG {} w 1 J 1 j {} {} m {} {} l S\n", ap::rgb(a.color), n(lw), n(x1), n(y1), n(x2), n(y2)));
                if a.kind == Kind::Arrow {
                    let ang = (y2 - y1).atan2(x2 - x1);
                    let len = lw * 3.0 + 6.0;
                    let p = |d: f64| (x2 - len * (ang + d).cos(), y2 - len * (ang + d).sin());
                    let (ax, ay) = p(0.45);
                    let (bx, by) = p(-0.45);
                    s.push_str(&format!("{} {} m {} {} l {} {} l S\n", n(ax), n(ay), n(x2), n(y2), n(bx), n(by)));
                }
            }
            Kind::Square | Kind::Circle => {
                let (x0, y0, x1, y1) = (lw / 2.0, lw / 2.0, w - lw / 2.0, h - lw / 2.0);
                let path = if a.kind == Kind::Square { format!("{} {} {} {} re\n", n(x0), n(y0), n(x1 - x0), n(y1 - y0)) } else { ap::ellipse(x0, y0, x1, y1) };
                if let Some(f) = a.fill {
                    s.push_str(&format!("{} rg\n", ap::rgb(f)));
                }
                s.push_str(&format!("{} RG {} w\n{}{}\n", ap::rgb(a.color), n(lw), path, if a.fill.is_some() { "B" } else { "S" }));
            }
            Kind::Stamp => {
                if let Some(p) = &a.picture {
                    let (img, mask) = ap::image(p.w, p.h, &p.rgba);
                    let img = match mask {
                        Some(m) => {
                            let mn = self.add(m);
                            match img {
                                Obj::Stream(mut d, data) => {
                                    d.set("SMask", Obj::Ref(mn, 0));
                                    Obj::Stream(d, data)
                                }
                                o => o,
                            }
                        }
                        None => img,
                    };
                    let inum = self.add(img);
                    xobjects.push(("Im0".into(), inum));
                    s.push_str(&format!("q {} 0 0 {} 0 0 cm /Im0 Do Q\n", n(w), n(h)));
                } else {
                    let text = if a.stamp.is_empty() { "DRAFT".to_string() } else { a.stamp.to_uppercase() };
                    let bw = 2.5_f64.max(h * 0.06);
                    s.push_str(&format!("{} RG {} w\n{}S\n", ap::rgb(a.color), n(bw), ap::rounded(bw, bw, w - bw, h - bw, h * 0.18)));
                    let mut size = (h * 0.55).max(4.0);
                    let tw = ap::width(&text, true, size);
                    if tw > w - 4.0 * bw {
                        size *= (w - 4.0 * bw) / tw;
                    }
                    let tw = ap::width(&text, true, size);
                    s.push_str(&format!("BT /{} {} Tf {} rg 1 0 0 1 {} {} Tm {} Tj ET\n", ap::HELV_BOLD, n(size), ap::rgb(a.color), n((w - tw) / 2.0), n((h - size * 0.72) / 2.0), ap::string(&text)));
                }
            }
            _ => return,
        }
        let mut res = ap::resources(alpha, blend);
        if !xobjects.is_empty() {
            res.set("XObject", Obj::Dict(Dict(xobjects.iter().map(|(k, v)| (std::sync::Arc::from(k.as_str()), Obj::Ref(*v, 0))).collect())));
        }
        if alpha.is_some() || blend.is_some() {
            s.insert_str(0, "/GS0 gs\n");
        }
        let form = ap::form(w, h, g.upright(), s.as_bytes(), res);
        let f = self.add(form);
        if let Some(mut d) = self.dict(id) {
            d.set("AP", Obj::Dict(Dict::of(vec![("N", Obj::Ref(f, 0))])));
            self.set(id, Obj::Dict(d));
        }
    }
}

fn rect_obj(r: [f64; 4]) -> Obj {
    Obj::Array(r.iter().map(|v| Obj::Real(ap::round(*v))).collect())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
