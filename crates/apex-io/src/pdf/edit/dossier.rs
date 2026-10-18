//! A dossier: several PDFs made one, with a cover, a table of contents
//! whose entries link to each part, a separator page before each part, a
//! bookmark for each and page numbers matching the contents.

use apex_i18n::tr;

use super::appearance::{n, string, width, wrap};
use super::stamp::{Place, Stamp};
use super::{PdfDoc, deflate};
use crate::pdf::obj::{Dict, Obj};

/// One document of the dossier.
pub struct Part {
    pub title: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dossier {
    pub title: String,
    /// Under the title on the cover (a case number, a date…).
    pub subtitle: String,
    pub cover: bool,
    pub index: bool,
    pub separators: bool,
    /// "n / total" at the foot of every page but the cover.
    pub numbers: bool,
}

impl Default for Dossier {
    fn default() -> Dossier {
        Dossier { title: String::new(), subtitle: String::new(), cover: true, index: true, separators: true, numbers: true }
    }
}

/// Entries on each page of the table of contents.
const PER_PAGE: usize = 30;
const MARGIN: f64 = 72.0;
const LINE: f64 = 22.0;

/// An empty A4 page to start from.
fn blank() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595.28 841.89] /Resources << >> >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Makes the dossier; also the parts left out (their index and why).
pub fn build(parts: &[Part], o: &Dossier) -> (Vec<u8>, Vec<(usize, String)>) {
    let mut doc = PdfDoc::open(blank(), None).expect("blank PDF");
    let mut failed = Vec::new();
    let fonts = [("F1", "Helvetica"), ("F2", "Helvetica-Bold")].map(|(k, base)| {
        let f = doc.add(Obj::Dict(Dict::of(vec![
            ("Type", Obj::name_of("Font")),
            ("Subtype", Obj::name_of("Type1")),
            ("BaseFont", Obj::name_of(base)),
            ("Encoding", Obj::name_of("WinAnsiEncoding")),
        ])));
        (k, doc.r(f))
    });
    let res = Dict::of(vec![("Font", Obj::Dict(Dict::of(fonts.to_vec())))]);
    // Each part's title and first page, counted without the contents.
    let mut starts: Vec<(String, usize)> = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        let at = doc.pages().len();
        match doc.insert_from(p.data.clone(), None, &[], at) {
            Ok(0) => failed.push((i, String::new())),
            Ok(_) => {
                if o.separators {
                    doc.insert_blank(at);
                    let body = separator(&doc, doc.pages()[at], starts.len() + 1, &p.title);
                    doc.draw(at, &body, &res, Vec::new());
                }
                starts.push((p.title.clone(), at));
            }
            Err(e) => failed.push((i, e.to_string())),
        }
    }
    let first = usize::from(o.cover);
    if !o.cover {
        doc.delete_pages(&[0]);
        for s in &mut starts {
            s.1 -= 1;
        }
    }
    let index_pages = if o.index && !starts.is_empty() { starts.len().div_ceil(PER_PAGE) } else { 0 };
    for k in 0..index_pages {
        doc.insert_blank(first + k);
    }
    for s in &mut starts {
        s.1 += index_pages;
    }
    let pages = doc.pages();
    if o.cover {
        let body = cover(&doc, pages[0], o);
        doc.draw(0, &body, &res, Vec::new());
    }
    for (k, chunk) in starts.chunks(PER_PAGE).take(index_pages).enumerate() {
        let at = first + k;
        let (body, links) = contents(&doc, pages[at], chunk, k * PER_PAGE, k == 0, &pages);
        doc.draw(at, &body, &res, links);
    }
    if index_pages > 0 {
        doc.add_bookmark(tr("dossier.contents"), first, 0.0, None);
    }
    for (title, at) in &starts {
        doc.add_bookmark(title, *at, 0.0, None);
    }
    if o.numbers {
        let total = pages.len() as u64;
        let which: Vec<usize> = (first..pages.len()).collect();
        let s = Stamp { text: "{n} / {total}".into(), place: Place::BottomCenter, digits: 0, size: 9.0, ..Stamp::default() };
        doc.stamp_pages(&which, &s, first as u64 + 1, Some(total));
    }
    if !o.title.trim().is_empty() {
        let mut info = doc.info();
        info.title = o.title.trim().to_string();
        doc.set_info(&info);
    }
    (doc.to_bytes_whole(), failed)
}

/// Lines of text: (x, y from the top, size, bold, text).
fn text(h: f64, items: &[(f64, f64, f64, bool, String)]) -> String {
    let mut out = String::from("0 g\n");
    for (x, y, size, bold, t) in items {
        out += &format!("BT /{} {} Tf {} {} Td {} Tj ET\n", if *bold { "F2" } else { "F1" }, n(*size), n(*x), n(h - y), string(t));
    }
    out
}

fn size_of(doc: &PdfDoc, pn: u32) -> (f64, f64) {
    let g = doc.geom(pn);
    (g.width, g.height)
}

fn cover(doc: &PdfDoc, pn: u32, o: &Dossier) -> String {
    let (w, h) = size_of(doc, pn);
    let mut items = Vec::new();
    let mut y = h * 0.38;
    for l in wrap(o.title.trim(), w - 2.0 * MARGIN, 28.0, true) {
        items.push(((w - width(&l, true, 28.0)) / 2.0, y, 28.0, true, l));
        y += 36.0;
    }
    // A rule between the title and the subtitle.
    let rule = y - 22.0;
    y += 10.0;
    for l in wrap(o.subtitle.trim(), w - 2.0 * MARGIN, 14.0, false) {
        items.push(((w - width(&l, false, 14.0)) / 2.0, y, 14.0, false, l));
        y += 20.0;
    }
    format!("0.6 g {} {} {} 1 re f\n{}", n(MARGIN * 2.0), n(h - rule), n(w - MARGIN * 4.0), text(h, &items))
}

fn separator(doc: &PdfDoc, pn: u32, num: usize, title: &str) -> String {
    let (w, h) = size_of(doc, pn);
    let label = tr("dossier.part").replace("{n}", &num.to_string());
    let mut items = vec![((w - width(&label, false, 14.0)) / 2.0, h * 0.40, 14.0, false, label)];
    let mut y = h * 0.40 + 40.0;
    for l in wrap(title, w - 2.0 * MARGIN, 24.0, true) {
        items.push(((w - width(&l, true, 24.0)) / 2.0, y, 24.0, true, l));
        y += 30.0;
    }
    text(h, &items)
}

/// A page of the contents: entries `chunk` (numbered from `from`), each
/// with its page number and a link to it.
fn contents(doc: &PdfDoc, pn: u32, chunk: &[(String, usize)], from: usize, heading: bool, pages: &[u32]) -> (String, Vec<([f64; 4], u32)>) {
    let (w, h) = size_of(doc, pn);
    let mut items = Vec::new();
    let mut links = Vec::new();
    let mut y = MARGIN + 20.0;
    if heading {
        items.push((MARGIN, y, 20.0, true, tr("dossier.contents").to_string()));
        y += 40.0;
    }
    let size = 11.0;
    let mut dots = String::new();
    for (k, (title, at)) in chunk.iter().enumerate() {
        let num = (at + 1).to_string();
        let nw = width(&num, false, size);
        let lead = format!("{}.  ", from + k + 1);
        let room = w - 2.0 * MARGIN - nw - width(&lead, false, size) - 16.0;
        let mut t = title.clone();
        if width(&t, false, size) > room {
            while !t.is_empty() && width(&format!("{t}…"), false, size) > room {
                t.pop();
            }
            t.push('…');
        }
        items.push((MARGIN, y, size, false, format!("{lead}{t}")));
        items.push((w - MARGIN - nw, y, size, false, num));
        // Dotted leader between title and number.
        let x0 = MARGIN + width(&format!("{lead}{t}"), false, size) + 6.0;
        let x1 = w - MARGIN - nw - 6.0;
        if x1 > x0 {
            dots += &format!("{} {} m {} {} l\n", n(x0), n(h - y), n(x1), n(h - y));
        }
        links.push(([MARGIN, h - y - 4.0, w - MARGIN, h - y + size], pages[*at]));
        y += LINE;
    }
    (format!("q 0.5 G 0.6 w [1 3] 0 d\n{dots}S Q\n{}", text(h, &items)), links)
}

impl PdfDoc {
    /// Sets page `at`'s drawing and resources, and links (rect, page to go to).
    fn draw(&mut self, at: usize, body: &str, res: &Dict, links: Vec<([f64; 4], u32)>) {
        let Some(&pn) = self.pages().get(at) else { return };
        let Some(mut d) = self.dict(pn) else { return };
        let c = self.add(deflate(Dict::default(), body.as_bytes()));
        d.set("Contents", self.r(c));
        d.set("Resources", Obj::Dict(res.clone()));
        if !links.is_empty() {
            let annots = links
                .into_iter()
                .map(|(r, to)| {
                    let a = self.add(Obj::Dict(Dict::of(vec![
                        ("Type", Obj::name_of("Annot")),
                        ("Subtype", Obj::name_of("Link")),
                        ("Rect", Obj::Array(r.iter().map(|v| Obj::Real(super::appearance::round(*v))).collect())),
                        ("Border", Obj::Array(vec![Obj::Int(0), Obj::Int(0), Obj::Int(0)])),
                        ("Dest", Obj::Array(vec![Obj::Ref(to, 0), Obj::name_of("Fit")])),
                    ])));
                    self.r(a)
                })
                .collect();
            d.set("Annots", Obj::Array(annots));
        }
        self.set(pn, Obj::Dict(d));
    }
}
