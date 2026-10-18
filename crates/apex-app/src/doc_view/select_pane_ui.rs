//! The Selection Pane of a document: its shapes and pictures front first
//! (in front of the text, in it, behind it), and what the pane asks
//! (select, rename, hide, restack) done each as one undo step.

use std::sync::Arc;

use apex_doc::{DocShape, Image, OBJECT, Pos, Selection};
use eframe::egui::{self, Id};

use crate::i18n::tr;
use crate::selection_pane::{self as pane, Action, Item};

use super::DocView;

/// A shape or a picture of the document.
struct Obj {
    at: Pos,
    shape: Option<Arc<DocShape>>,
    image: Option<Arc<Image>>,
}

impl Obj {
    fn name(&self) -> &str {
        self.shape.as_ref().map_or_else(|| self.image.as_ref().map_or("", |i| i.name.as_str()), |s| s.shape.name.as_str())
    }

    fn hidden(&self) -> bool {
        self.shape.as_ref().map_or_else(|| self.image.as_ref().is_some_and(|i| i.hidden), |s| s.shape.hidden)
    }

    /// Floats: restacked by their z.
    fn floats(&self) -> bool {
        self.shape.as_ref().is_some_and(|s| s.wrap.floats())
    }

    /// Where it stacks: its layer, then its z.
    fn layer(&self) -> (i32, i32) {
        self.shape.as_ref().map_or((1, 0), |s| super::shapes_ui::layer(s))
    }
}

fn key(at: Pos) -> u64 {
    ((at.para as u64) << 32) | at.offset as u64
}

impl DocView {
    /// The objects of the document, front first.
    fn pane_objects(&self) -> Vec<Obj> {
        let mut v: Vec<Obj> = self.doc.shapes().into_iter().map(|(at, s)| Obj { at, shape: Some(s), image: None }).collect();
        v.extend(self.doc.images().into_iter().map(|(at, i)| Obj { at, shape: None, image: Some(i) }));
        // The later in the text, the more in front among equals.
        v.sort_by_key(|o| (o.layer(), o.at));
        v.reverse();
        v
    }

    /// The objects as the pane lists them, front first.
    pub(super) fn pane_items(&self) -> Vec<Item> {
        let (a, b) = (self.sel.start(), self.sel.end());
        let one = a.para == b.para && b.offset == a.offset + OBJECT.len_utf8();
        self.pane_objects()
            .iter()
            .enumerate()
            .map(|(k, o)| Item {
                key: key(o.at),
                name: if o.name().trim().is_empty() { format!("{} {}", tr(if o.image.is_some() { "selpane.picture" } else { "selpane.object" }), k + 1) } else { o.name().to_string() },
                hidden: o.hidden(),
                selected: one && a == o.at,
                movable: o.floats(),
            })
            .collect()
    }

    /// The pane at the right of the pages.
    pub(super) fn select_pane(&mut self, ui: &mut egui::Ui) {
        if !self.pane_open {
            return;
        }
        let frame = egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(6, 6));
        egui::Panel::right(Id::new(("doc_selpane", self.uid))).resizable(true).default_size(220.0).frame(frame).show(ui, |ui| {
            let items = self.pane_items();
            let acts = pane::show(ui, &mut self.pane, &items);
            for a in acts {
                self.pane_action(a);
            }
        });
    }

    /// Does what the pane asked.
    pub(super) fn pane_action(&mut self, a: Action) {
        let objs = self.pane_objects();
        let find = |k: u64| objs.iter().find(|o| key(o.at) == k);
        match a {
            Action::Close => self.pane_open = false,
            Action::Select(k, _) => {
                if let Some(o) = find(k).filter(|o| !o.hidden()) {
                    self.select_shape(o.at);
                    self.scroll_to_caret = true;
                }
            }
            Action::Rename(k, name) => {
                if let Some(o) = find(k) {
                    self.change_pane_objects(&[o], |s| s.name = name.clone(), |i| i.name = name.clone());
                }
            }
            Action::Hide(k, hide) => {
                if let Some(o) = find(k) {
                    self.change_pane_objects(&[o], |s| s.hidden = hide, |i| i.hidden = hide);
                }
            }
            Action::All(hide) => {
                let list: Vec<&Obj> = objs.iter().filter(|o| o.hidden() != hide).collect();
                self.change_pane_objects(&list, |s| s.hidden = hide, |i| i.hidden = hide);
            }
            Action::Move(k, to) => {
                let Some(from) = objs.iter().position(|o| key(o.at) == k) else { return };
                if from == to || !objs[from].floats() {
                    return;
                }
                // The floats numbered back to front in their new order.
                let order: Vec<usize> = pane::moved(&(0..objs.len()).collect::<Vec<_>>(), from, to);
                let mut changes = Vec::new();
                for (z, &i) in order.iter().rev().filter(|&&i| objs[i].floats()).enumerate() {
                    let o = &objs[i];
                    if let Some(s) = &o.shape
                        && s.z != z as i32
                    {
                        changes.push((o.at, DocShape { z: z as i32, ..(**s).clone() }));
                    }
                }
                self.doc.change_shapes(changes, self.sel);
            }
        }
    }

    /// Changes shapes (`fs`) and pictures (`fi`) as one undo step; those
    /// hidden stop being selected.
    fn change_pane_objects(&mut self, objs: &[&Obj], fs: impl Fn(&mut apex_doc::Shape), fi: impl Fn(&mut Image)) {
        let (mut shapes, mut images) = (Vec::new(), Vec::new());
        for o in objs {
            if let Some(s) = &o.shape {
                let mut s = (**s).clone();
                fs(&mut s.shape);
                shapes.push((o.at, s));
            } else if let Some(i) = &o.image {
                let mut i = (**i).clone();
                fi(&mut i);
                images.push((o.at, i));
            }
        }
        let hid: Vec<Pos> = shapes.iter().filter(|(_, s)| s.shape.hidden).map(|(p, _)| *p).chain(images.iter().filter(|(_, i)| i.hidden).map(|(p, _)| *p)).collect();
        self.doc.change_objects(shapes, images, self.sel);
        if hid.contains(&self.sel.start()) && self.sel.end().offset == self.sel.start().offset + OBJECT.len_utf8() {
            self.set_sel(Selection::caret(self.sel.start()));
        }
    }
}

#[cfg(test)]
mod tests {
    use apex_doc::{Attrs, Document, Paragraph, Shape, ShapeKind, ShapeWrap};

    use super::*;

    fn doc() -> Document {
        let mut p = Paragraph::new("Text ");
        for (name, wrap, z) in [("Back", ShapeWrap::Behind, 0), ("Low", ShapeWrap::InFront, 1), ("High", ShapeWrap::InFront, 2)] {
            let mut d = DocShape::new(Shape::new(ShapeKind::Rect), 40.0, 30.0);
            (d.shape.name, d.wrap, d.z) = (name.into(), wrap, z);
            p.push(&OBJECT.to_string(), Attrs { shape: Some(Arc::new(d)), ..Default::default() });
        }
        // Not drawn here: any bytes do.
        let data: Arc<[u8]> = Arc::from(&b"picture"[..]);
        p.push(&OBJECT.to_string(), Attrs { image: Some(Arc::new(Image::new(data, 20.0, 20.0))), ..Default::default() });
        Document::from_paragraphs(vec![p])
    }

    fn names(v: &DocView) -> Vec<String> {
        v.pane_items().into_iter().map(|i| i.name).collect()
    }

    #[test]
    fn the_pane_lists_hides_and_restacks() {
        let mut v = DocView::new(doc());
        assert_eq!(names(&v), ["High", "Low", "Picture 3", "Back"].map(String::from));
        let items = v.pane_items();
        assert!(items[0].movable && !items[2].movable);
        // Select, then hide: no longer selected; undone in one step.
        v.pane_action(Action::Select(items[1].key, false));
        assert!(v.pane_items()[1].selected);
        let drawn = DocView::para_hash(&v.doc.paragraphs[0]);
        v.pane_action(Action::Hide(items[1].key, true));
        assert!(v.pane_items()[1].hidden && !v.pane_items()[1].selected);
        assert_ne!(DocView::para_hash(&v.doc.paragraphs[0]), drawn, "laid out again");
        v.undo();
        assert!(!v.pane_items()[1].hidden);
        // Hide all (the picture too) and rename the picture, each undone.
        v.pane_action(Action::All(true));
        assert!(v.pane_items().iter().all(|i| i.hidden));
        assert!(v.doc.images()[0].1.hidden);
        v.undo();
        assert!(v.pane_items().iter().all(|i| !i.hidden));
        v.pane_action(Action::Rename(items[2].key, "Logo".into()));
        assert_eq!(v.doc.images()[0].1.name, "Logo");
        v.undo();
        assert_eq!(v.doc.images()[0].1.name, "");
        // "Low" to the front, one step.
        v.pane_action(Action::Move(items[1].key, 0));
        assert_eq!(names(&v)[..2], ["Low".to_string(), "High".to_string()]);
        v.undo();
        assert_eq!(names(&v)[..2], ["High".to_string(), "Low".to_string()]);
    }
}
