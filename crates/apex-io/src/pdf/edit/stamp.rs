//! Stamping text on every page: Bates numbers ("foliado"), page numbers
//! and diagonal watermarks, drawn into the pages' content (not as
//! annotations, so they print and can't be moved or hidden).

use super::appearance::{n, rgb, string, width};
use super::{PdfDoc, deflate};
use crate::pdf::obj::{Dict, Obj};

/// Where the text goes on the page as shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
    /// Large, across the page from corner to corner, see-through.
    Diagonal,
}

impl Place {
    pub const ALL: [Place; 7] =
        [Place::TopLeft, Place::TopCenter, Place::TopRight, Place::BottomLeft, Place::BottomCenter, Place::BottomRight, Place::Diagonal];

    pub fn key(self) -> &'static str {
        match self {
            Place::TopLeft => "top_left",
            Place::TopCenter => "top_center",
            Place::TopRight => "top_right",
            Place::BottomLeft => "bottom_left",
            Place::BottomCenter => "bottom_center",
            Place::BottomRight => "bottom_right",
            Place::Diagonal => "diagonal",
        }
    }

    pub fn from_key(s: &str) -> Option<Place> {
        Place::ALL.into_iter().find(|p| p.key() == s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stamp {
    /// The text; `{n}` becomes the number (zero-padded to `digits`) and
    /// `{total}` the last number stamped.
    pub text: String,
    pub place: Place,
    /// Font size in points (a diagonal one is sized to the page).
    pub size: f64,
    pub color: u32,
    /// From the page's edges, in points.
    pub margin: f64,
    pub digits: usize,
}

impl Default for Stamp {
    fn default() -> Stamp {
        Stamp { text: "{n}".into(), place: Place::BottomRight, size: 10.0, color: 0x000000, margin: 24.0, digits: 6 }
    }
}

impl Stamp {
    /// The text on the page numbered `num`, of numbers up to `last`.
    pub fn text_for(&self, num: u64, last: u64) -> String {
        let w = self.digits.min(20);
        self.text.replace("{n}", &format!("{num:0w$}")).replace("{total}", &last.to_string())
    }
}

impl PdfDoc {
    /// Stamps pages `which` (all when empty), numbering them from `first`;
    /// `last` is what `{total}` shows (the count when `None`). One undo
    /// step. Returns the number after the last one used, to go on with in
    /// the next file.
    pub fn stamp_pages(&mut self, which: &[usize], s: &Stamp, first: u64, last: Option<u64>) -> u64 {
        let pages = self.pages();
        let which: Vec<usize> = if which.is_empty() { (0..pages.len()).collect() } else { which.to_vec() };
        let last = last.unwrap_or(first + which.len().max(1) as u64 - 1);
        self.begin("pdf.undo.stamp");
        let font = self.add(Obj::Dict(Dict::of(vec![
            ("Type", Obj::name_of("Font")),
            ("Subtype", Obj::name_of("Type1")),
            ("BaseFont", Obj::name_of("Helvetica")),
            ("Encoding", Obj::name_of("WinAnsiEncoding")),
        ])));
        let gs = (s.place == Place::Diagonal).then(|| {
            self.add(Obj::Dict(Dict::of(vec![("Type", Obj::name_of("ExtGState")), ("ca", Obj::num_of(0.25)), ("CA", Obj::num_of(0.25))])))
        });
        // The page's own drawing is kept inside q ... Q, so whatever state
        // it leaves doesn't move the stamp.
        let open = self.add(deflate(Dict::default(), b"q\n"));
        let mut num = first;
        for &i in &which {
            let Some(&pn) = pages.get(i) else { continue };
            let Some(mut d) = self.dict(pn) else { continue };
            let mut res = self.resources(pn);
            let fname = add_resource(self, &mut res, "Font", "ApxStamp", self.r(font));
            let gname = gs.map(|g| add_resource(self, &mut res, "ExtGState", "ApxStampGs", self.r(g)));
            let body = self.stamp_content(pn, s, &s.text_for(num, last), &fname, gname.as_deref());
            let close = self.add(deflate(Dict::default(), format!("Q\n{body}").as_bytes()));
            let mut parts = match d.get("Contents").cloned() {
                Some(Obj::Array(a)) => a,
                Some(o @ Obj::Ref(..)) => match self.resolve(&o) {
                    Obj::Array(a) => a,
                    _ => vec![o],
                },
                _ => Vec::new(),
            };
            parts.insert(0, self.r(open));
            parts.push(self.r(close));
            d.set("Contents", Obj::Array(parts));
            d.set("Resources", Obj::Dict(res));
            self.set(pn, Obj::Dict(d));
            num += 1;
        }
        self.end();
        num
    }

    fn stamp_content(&self, pn: u32, s: &Stamp, text: &str, font: &str, gs: Option<&str>) -> String {
        let g = self.geom(pn);
        let (w, h) = (g.width, g.height);
        let u = g.upright();
        if s.place == Place::Diagonal {
            // Along the diagonal, as large as fits in 80% of its length.
            let diag = (w * w + h * h).sqrt();
            let size = (diag * 0.8 / width(text, true, 1.0).max(1.0)).min(h * 0.3).max(8.0);
            let tw = width(text, true, size);
            let (c, sn) = (w / diag, h / diag);
            // Rotated about the page's middle, as shown.
            let [cx, cy] = g.to_user(w / 2.0, h / 2.0);
            let rot = mul(&[c, sn, -sn, c, 0.0, 0.0], &u);
            return format!(
                "q /{} gs {} {} {} {} {} {} cm BT /{font} {} Tf {} rg {} {} Td {} Tj ET Q\n",
                gs.unwrap_or(""),
                n(rot[0]),
                n(rot[1]),
                n(rot[2]),
                n(rot[3]),
                n(cx),
                n(cy),
                n(size),
                rgb(s.color),
                n(-tw / 2.0),
                n(-size * 0.35),
                string(text)
            )
            .replace("q / gs ", "q ");
        }
        let tw = width(text, false, s.size);
        let x = match s.place {
            Place::TopLeft | Place::BottomLeft => s.margin,
            Place::TopCenter | Place::BottomCenter => (w - tw) / 2.0,
            _ => w - s.margin - tw,
        };
        let top = matches!(s.place, Place::TopLeft | Place::TopCenter | Place::TopRight);
        // Shown y of the baseline.
        let y = if top { s.margin + s.size * 0.8 } else { h - s.margin };
        let [ux, uy] = g.to_user(x, y);
        format!(
            "q {} {} {} {} {} {} cm BT /{font} {} Tf {} rg 0 0 Td {} Tj ET Q\n",
            n(u[0]),
            n(u[1]),
            n(u[2]),
            n(u[3]),
            n(ux),
            n(uy),
            n(s.size),
            rgb(s.color),
            string(text)
        )
    }
}

/// `a` then `b`, as PDF matrices.
fn mul(a: &[f64; 6], b: &[f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// Puts `obj` in the `kind` subdictionary of `res` under a name starting
/// with `base` that isn't taken; returns the name.
fn add_resource(doc: &PdfDoc, res: &mut Dict, kind: &str, base: &str, obj: Obj) -> String {
    let mut sub = doc.key_dict(res, kind).unwrap_or_default();
    let mut name = base.to_string();
    let mut i = 1;
    while sub.get(&name).is_some() && sub.get(&name) != Some(&obj) {
        i += 1;
        name = format!("{base}{i}");
    }
    sub.set(&name, obj);
    res.set(kind, Obj::Dict(sub));
    name
}
