//! The Selection Pane of a slide: its objects front first, and what the
//! pane asks (select, rename, hide, restack) done each as one undo step.

use eframe::egui;

use crate::i18n::tr;
use crate::selection_pane::{self as pane, Action, Item};

use super::SlidesView;

impl SlidesView {
    /// The objects of the canvas as the pane lists them, front first.
    pub fn pane_items(&self) -> Vec<Item> {
        let sel = self.selection();
        self.cur_objects()
            .iter()
            .rev()
            .map(|o| Item {
                key: o.id as u64,
                name: if o.name.trim().is_empty() { format!("{} {}", tr("selpane.object"), o.id) } else { o.name.clone() },
                hidden: o.hidden,
                selected: sel.contains(&o.id),
                movable: true,
            })
            .collect()
    }

    pub(super) fn select_pane_ui(&mut self, ui: &mut egui::Ui) {
        let items = self.pane_items();
        let mut st = std::mem::take(&mut self.pane);
        let acts = pane::show(ui, &mut st, &items);
        self.pane = st;
        for a in acts {
            self.pane_action(a);
        }
    }

    /// Does what the pane asked.
    pub fn pane_action(&mut self, a: Action) {
        self.pres.seal();
        match a {
            Action::Close => self.pane_open = false,
            Action::Select(key, add) => {
                let id = key as u32;
                if self.cur_objects().iter().any(|o| o.id == id && o.hidden) {
                    return;
                }
                self.typing = None;
                if add {
                    if let Some(k) = self.sel.iter().position(|s| *s == id) {
                        self.sel.remove(k);
                    } else {
                        self.sel.push(id);
                    }
                } else {
                    self.sel = vec![id];
                }
            }
            Action::Rename(key, name) => self.change_object("selpane.undo.rename", key as u32, |o| o.name = name),
            Action::Hide(key, hide) => {
                self.change_object("selpane.undo.hide", key as u32, |o| o.hidden = hide);
                if hide {
                    self.drop_hidden();
                }
            }
            Action::All(hide) => {
                if self.cur_objects().iter().all(|o| o.hidden == hide) {
                    return;
                }
                if let Some(v) = self.edit_objects("selpane.undo.hide") {
                    for o in v.iter_mut().filter(|o| o.hidden != hide) {
                        o.hidden = hide;
                        o.touch();
                    }
                }
                self.drop_hidden();
            }
            Action::Move(key, to) => {
                let front: Vec<u32> = self.cur_objects().iter().rev().map(|o| o.id).collect();
                let Some(from) = front.iter().position(|id| *id == key as u32) else { return };
                if from == to {
                    return;
                }
                let order: Vec<u32> = pane::moved(&front, from, to).into_iter().rev().collect();
                if let Some(v) = self.edit_objects("slides.undo.order") {
                    v.sort_by_key(|o| order.iter().position(|id| *id == o.id).unwrap_or(usize::MAX));
                }
            }
        }
    }

    /// Hidden objects can't stay selected nor be typed in.
    fn drop_hidden(&mut self) {
        let shown: Vec<u32> = self.cur_objects().iter().filter(|o| !o.hidden).map(|o| o.id).collect();
        self.sel.retain(|id| shown.contains(id));
        if self.typing.as_ref().is_some_and(|t| !shown.contains(&t.id)) {
            self.typing = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use apex_slides::Frame;

    use super::*;

    #[test]
    fn the_pane_names_hides_and_restacks() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        v.set_layout(0, 5);
        let a = v.insert_shape("rect", Frame::new(0.0, 0.0, 50.0, 50.0), false).unwrap();
        let b = v.insert_shape("ellipse", Frame::new(60.0, 0.0, 50.0, 50.0), false).unwrap();
        let c = v.insert_shape("rect", Frame::new(120.0, 0.0, 50.0, 50.0), false).unwrap();
        let ids = |v: &SlidesView| v.cur_objects().iter().map(|o| o.id).collect::<Vec<_>>();
        let base = ids(&v);
        assert_eq!(base[base.len() - 3..], [a, b, c]);
        let items = v.pane_items();
        assert_eq!(items[0].key, c as u64, "front first");
        // Rename: one step, undone.
        v.pane_action(Action::Rename(b as u64, "Sun".into()));
        assert_eq!(v.cur_objects().iter().find(|o| o.id == b).unwrap().name, "Sun");
        v.undo();
        assert_ne!(v.cur_objects().iter().find(|o| o.id == b).unwrap().name, "Sun");
        // Hide: drops it from the selection; Show All; undo each.
        v.pane_action(Action::Select(b as u64, false));
        assert_eq!(v.selection(), vec![b]);
        v.pane_action(Action::Hide(b as u64, true));
        assert!(v.selection().is_empty());
        assert!(v.pane_items().iter().find(|i| i.key == b as u64).unwrap().hidden);
        v.pane_action(Action::Select(b as u64, false));
        assert!(v.selection().is_empty(), "hidden objects aren't selected");
        v.pane_action(Action::All(true));
        assert!(v.cur_objects().iter().all(|o| o.hidden));
        v.undo();
        assert_eq!(v.cur_objects().iter().filter(|o| o.hidden).count(), 1);
        v.pane_action(Action::All(false));
        assert!(v.cur_objects().iter().all(|o| !o.hidden));
        v.undo();
        v.undo();
        assert!(v.cur_objects().iter().all(|o| !o.hidden));
        // Restack: the front one to the back of the three, undone.
        let n = v.pane_items().len();
        v.pane_action(Action::Move(c as u64, n - 1));
        assert_eq!(ids(&v)[0], c);
        assert_eq!(v.pane_items()[0].key, b as u64);
        v.undo();
        assert_eq!(ids(&v), base);
    }
}
