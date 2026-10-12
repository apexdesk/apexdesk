//! Embedded objects (OLE) and SmartArt diagrams. Both ride on a picture
//! (`Image::object`, `SheetImage::object`): the picture is the preview
//! and gives the place and size, the object keeps what the file stored so
//! it saves back unchanged.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Document, Image, Pos, Shape};

/// An id for a new object, unique in the process (not saved).
pub fn fresh_object_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// What rides on a picture.
#[derive(Clone, Debug)]
pub struct Embedded {
    /// Finds the object again after it was edited in another tab.
    pub id: u64,
    pub kind: EmbedKind,
}

#[derive(Clone, Debug)]
pub enum EmbedKind {
    Ole(OleObject),
    Diagram(Diagram),
    Chart(ChartObject),
}

/// A chart with its own data, as Word keeps them: a small workbook (in
/// xlsx) holding the data and the chart, drawn live and edited in a tab.
#[derive(Clone, Debug)]
pub struct ChartObject {
    pub book: Arc<[u8]>,
}

/// An object embedded by another program (OLE): a workbook, a document,
/// a packaged file...
#[derive(Clone, Debug, Default)]
pub struct OleObject {
    /// "Excel.Sheet.12", "Word.Document.12", "Package"...
    pub prog_id: String,
    /// The data as the file stored it: an Office Open XML or ODF package,
    /// an OLE compound file (a legacy .xls or .doc, a packaged file)...
    pub data: Arc<[u8]>,
    /// The extension and media type of the part it came in (kept to write
    /// it back the same way; empty: chosen from the data).
    pub ext: String,
    pub content_type: String,
    /// Shown as an icon instead of its content.
    pub icon: bool,
}

/// A SmartArt diagram: its parts, kept whole, and the shapes Office drew
/// for it (they can't be edited here).
#[derive(Clone, Debug, Default)]
pub struct Diagram {
    pub parts: Vec<DiagramPart>,
    /// The drawing: shapes placed in a `size` box (points).
    pub shapes: Vec<DiagramShape>,
    pub size: (f32, f32),
    /// The text of its nodes (shown when there is no drawing).
    pub texts: Vec<String>,
}

/// What a diagram part is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagramRole {
    Data,
    Layout,
    Style,
    Colors,
    Drawing,
}

impl DiagramRole {
    pub const ALL: [DiagramRole; 5] = [DiagramRole::Data, DiagramRole::Layout, DiagramRole::Style, DiagramRole::Colors, DiagramRole::Drawing];
}

/// A part of a diagram and the files it points to (pictures of a
/// picture layout): relationship id, type and a file name with its bytes.
#[derive(Clone, Debug)]
pub struct DiagramPart {
    pub role: DiagramRole,
    pub xml: Arc<[u8]>,
    pub rels: Vec<(String, String, String, Arc<[u8]>)>,
}

/// A shape of a diagram's drawing, in points from the diagram's corner.
#[derive(Clone, Debug)]
pub struct DiagramShape {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub shape: Shape,
}

impl Embedded {
    pub fn new(kind: EmbedKind) -> Embedded {
        Embedded { id: fresh_object_id(), kind }
    }

    pub fn ole(&self) -> Option<&OleObject> {
        match &self.kind {
            EmbedKind::Ole(o) => Some(o),
            _ => None,
        }
    }

    pub fn diagram(&self) -> Option<&Diagram> {
        match &self.kind {
            EmbedKind::Diagram(d) => Some(d),
            _ => None,
        }
    }

    pub fn chart(&self) -> Option<&ChartObject> {
        match &self.kind {
            EmbedKind::Chart(c) => Some(c),
            _ => None,
        }
    }

    /// The same object (same id) holding other data.
    pub fn with_data(&self, data: Arc<[u8]>) -> Embedded {
        let mut e = self.clone();
        match &mut e.kind {
            EmbedKind::Ole(o) => o.data = data,
            EmbedKind::Chart(c) => c.book = data,
            EmbedKind::Diagram(_) => {}
        }
        e
    }
}

impl PartialEq for Embedded {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
            && match (&self.kind, &o.kind) {
                (EmbedKind::Ole(a), EmbedKind::Ole(b)) => Arc::ptr_eq(&a.data, &b.data) && a.prog_id == b.prog_id && a.icon == b.icon,
                (EmbedKind::Chart(a), EmbedKind::Chart(b)) => Arc::ptr_eq(&a.book, &b.book),
                (EmbedKind::Diagram(a), EmbedKind::Diagram(b)) => a.parts.len() == b.parts.len() && a.parts.iter().zip(&b.parts).all(|(x, y)| Arc::ptr_eq(&x.xml, &y.xml)),
                _ => false,
            }
    }
}

impl Eq for Embedded {}

impl std::hash::Hash for Embedded {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        match &self.kind {
            EmbedKind::Ole(o) => crate::data_key(&o.data).hash(h),
            EmbedKind::Chart(c) => crate::data_key(&c.book).hash(h),
            EmbedKind::Diagram(_) => {}
        }
    }
}

impl Document {
    /// Every embedded object, in the text flow or floating: where it is
    /// and its picture.
    pub fn objects(&self) -> Vec<(Pos, Arc<Image>)> {
        let mut out: Vec<(Pos, Arc<Image>)> = self.images().into_iter().filter(|(_, i)| i.object.is_some()).collect();
        for (at, s) in self.shapes() {
            if let Some(pic) = s.shape.picture.as_ref().filter(|p| p.object.is_some()) {
                out.push((at, pic.clone()));
            }
        }
        out
    }

    /// The object with id `id`.
    pub fn object(&self, id: u64) -> Option<(Pos, Arc<Image>)> {
        self.objects().into_iter().find(|(_, i)| i.object.as_ref().is_some_and(|o| o.id == id))
    }

    /// Puts `object` (and its new preview, if any) in place of the object
    /// with the same id, as one undo step. False when it is gone.
    pub fn replace_object(&mut self, object: Embedded, preview: Option<Arc<[u8]>>) -> bool {
        let Some((at, img)) = self.object(object.id) else { return false };
        let new = Image { data: preview.unwrap_or_else(|| img.data.clone()), object: Some(Arc::new(object)), ..(*img).clone() };
        if self.image_at(at).is_some() {
            self.set_image(at, new);
        } else if let Some((_, ds)) = self.shape_at(at) {
            let mut ds = (*ds).clone();
            ds.shape.picture = Some(Arc::new(new));
            self.set_shape(at, ds);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Paragraph, Selection};

    #[test]
    fn replace_keeps_place_and_undoes() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("ab")]);
        let obj = Embedded::new(EmbedKind::Ole(OleObject { prog_id: "Excel.Sheet.12".into(), data: Arc::from(&b"one"[..]), ..Default::default() }));
        let id = obj.id;
        let img = Image { object: Some(Arc::new(obj.clone())), ..Image::new(Arc::from(&b"emf"[..]), 100.0, 50.0) };
        d.insert_image(Selection::caret(Pos::new(0, 1)), img);
        assert_eq!(d.objects().len(), 1);
        assert!(d.replace_object(obj.with_data(Arc::from(&b"two"[..])), Some(Arc::from(&b"png"[..]))));
        let (_, img) = d.object(id).unwrap();
        assert_eq!(&*img.object.as_ref().unwrap().ole().unwrap().data, b"two");
        assert_eq!(&*img.data, b"png");
        assert_eq!(img.width, 100.0);
        d.undo();
        assert_eq!(&*d.object(id).unwrap().1.object.as_ref().unwrap().ole().unwrap().data, b"one");
        // Floating, it is found and replaced in its shape.
        d.redo();
        assert!(d.float_image(Pos::new(0, 1), false, None));
        assert!(d.replace_object(obj.with_data(Arc::from(&b"three"[..])), None));
        assert_eq!(&*d.object(id).unwrap().1.object.as_ref().unwrap().ole().unwrap().data, b"three");
        assert!(!d.replace_object(Embedded::new(EmbedKind::Diagram(Diagram::default())), None));
    }
}
