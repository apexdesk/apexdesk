//! Diagram on slides: the contextual Diagram Design tab and the text
//! pane. Editing the items, colors and style is [`apex_io::diagram_edit`]'s;
//! layouts it doesn't draw only let their text change.

use std::sync::Arc;

use apex_doc::Diagram;
use apex_io::diagram_edit::{self, Item};
use apex_slides::{ObjKind, Object};
use eframe::egui::{self, Color32};

use super::{SlidesView, objects::Target};
use crate::app::Notice;
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::toolstrip::{self, Body, TabDef};

/// Color of the Diagram Design tab.
pub const DIAGRAM_TAB: Color32 = Color32::from_rgb(0x2E, 0x8B, 0x57);

/// Names of the color schemes and styles offered, in [`diagram_edit::COLORS`]
/// and [`diagram_edit::STYLES`] order.
pub const COLOR_KEYS: [&str; 8] = ["slides.diagram.c.accent1", "slides.diagram.c.accent2", "slides.diagram.c.accent3", "slides.diagram.c.accent4", "slides.diagram.c.accent5", "slides.diagram.c.accent6", "slides.diagram.c.colorful", "slides.diagram.c.outline"];
pub const STYLE_KEYS: [&str; 4] = ["slides.diagram.s.flat", "slides.diagram.s.no_line", "slides.diagram.s.moderate", "slides.diagram.s.intense"];

/// The text pane being edited: the diagram's id and its items, one a
/// line, indented by tabs.
#[derive(Default)]
pub struct Pane {
    pub open: Option<(u32, String)>,
}

impl SlidesView {
    /// The diagram selected on a slide.
    pub fn selected_diagram(&self) -> Option<(u32, &Arc<Diagram>)> {
        if !matches!(self.target(), Target::Slide(_)) {
            return None;
        }
        let o = self.selected_object()?;
        match &o.kind {
            ObjKind::Diagram(d) => Some((o.id, d)),
            _ => None,
        }
    }

    pub fn diagram_info(&self) -> Option<diagram_edit::Info> {
        diagram_edit::info(self.selected_diagram()?.1)
    }

    /// Replaces the selected diagram by what `f` makes of it, as one undo
    /// step. False when there is none or `f` can't.
    pub fn edit_diagram(&mut self, f: impl FnOnce(&Diagram) -> Option<Diagram>) -> bool {
        if self.read_only {
            return false;
        }
        let Some((id, d)) = self.selected_diagram() else { return false };
        let Some(new) = f(d) else { return false };
        let cur = self.cur;
        let deck = self.pres.edit("slides.undo.diagram");
        if let Some(o) = deck.object_mut(cur, id) {
            o.kind = ObjKind::Diagram(Arc::new(new));
            o.touch();
        }
        true
    }

    /// Makes the selected diagram's items `items`; tells why when its
    /// layout only lets text change.
    pub fn set_diagram_items(&mut self, items: &[Item]) -> bool {
        let ok = self.edit_diagram(|d| diagram_edit::set_items(d, items));
        if !ok && self.selected_diagram().is_some() && !self.read_only {
            let why = if self.diagram_info().is_some_and(|i| i.locked) { "slides.diagram.locked" } else { "slides.diagram.text_only" };
            self.notices.push(Notice::info(tr(why)));
        }
        ok
    }

    /// Adds an item at the end: a top one or, with `sub`, one under the
    /// last.
    pub fn diagram_add(&mut self, sub: bool) {
        let Some(info) = self.diagram_info() else { return };
        let mut items = info.items;
        let level = match (sub, items.last()) {
            (true, Some(l)) => l.level + 1,
            _ => 0,
        };
        items.push(Item::new(level, ""));
        self.set_diagram_items(&items);
    }

    /// Removes the last item.
    pub fn diagram_remove(&mut self) {
        let Some(info) = self.diagram_info() else { return };
        let mut items = info.items;
        if items.pop().is_some() {
            self.set_diagram_items(&items);
        }
    }

    pub fn set_diagram_colors(&mut self, id: &str) {
        self.edit_diagram(|d| diagram_edit::set_colors(d, id));
    }

    pub fn set_diagram_style(&mut self, id: &str) {
        self.edit_diagram(|d| diagram_edit::set_style(d, id));
    }

    pub fn open_text_pane(&mut self) {
        let Some((id, d)) = self.selected_diagram() else { return };
        let Some(info) = diagram_edit::info(d) else { return };
        self.diagram.open = Some((id, diagram_edit::outline_text(&info.items)));
    }

    pub(super) fn text_pane(&mut self, ctx: &egui::Context) {
        let Some((id, mut text)) = self.diagram.open.take() else { return };
        let info = self.diagram_info();
        let res = crate::widgets::dialog(ctx, "slides_diagram_text", tr("slides.diagram.text_pane"), 380.0, |ui| {
            ui.label(tr("slides.diagram.text_pane_help"));
            if let Some(i) = &info {
                let kind = tr(i.kind.key());
                let note = if i.kind.drawn() && !i.locked { kind.to_string() } else { format!("{kind} — {}", tr("slides.diagram.text_only")) };
                ui.weak(note);
            }
            ui.add_space(4.0);
            let r = ui.add(egui::TextEdit::multiline(&mut text).desired_rows(10).desired_width(f32::INFINITY).lock_focus(true).code_editor());
            ui.data_mut(|m| m.insert_temp(r.id.with("multiline"), true));
        });
        match res {
            crate::widgets::DialogResult::Ok => {
                if self.selected_diagram().map(|d| d.0) != Some(id) {
                    self.sel = vec![id];
                }
                if !self.set_diagram_items(&diagram_edit::parse_outline(&text)) && self.selected_diagram().is_some() && !self.read_only {
                    // Kept open to fix what can't be done.
                    self.diagram.open = Some((id, text));
                }
            }
            crate::widgets::DialogResult::Cancel => {}
            crate::widgets::DialogResult::None => self.diagram.open = Some((id, text)),
        }
    }

    pub(super) fn diagram_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        let Some(info) = self.diagram_info() else { return };
        let free = info.kind.drawn() && !info.locked && !self.read_only;
        b.group("slides.diagram.g.graphic", Icon::Outline, |ui| {
            if toolstrip::large(ui, Icon::Outline, tr("slides.diagram.text_pane"), tr("slides.diagram.text_pane_tip"), acc, false).clicked() {
                self.open_text_pane();
            }
            ui.add_enabled_ui(free, |ui| {
                ui.vertical(|ui| {
                    if toolstrip::small(ui, Some(Icon::InsertRows), tr("slides.diagram.add"), tr("slides.diagram.add_tip"), acc, false).clicked() {
                        self.diagram_add(false);
                    }
                    if toolstrip::small(ui, Some(Icon::Indent), tr("slides.diagram.add_sub"), tr("slides.diagram.add_sub_tip"), acc, false).clicked() {
                        self.diagram_add(true);
                    }
                    let can_remove = !info.items.is_empty();
                    ui.add_enabled_ui(can_remove, |ui| {
                        if toolstrip::small(ui, Some(Icon::DeleteRows), tr("slides.diagram.remove"), tr("slides.diagram.remove_tip"), acc, false).clicked() {
                            self.diagram_remove();
                        }
                    });
                });
            });
        });
        b.group("slides.diagram.g.styles", Icon::Styles, |ui| {
            ui.add_enabled_ui(free, |ui| {
                let mut pick = None;
                toolstrip::large_menu(ui, Icon::Themes, tr("slides.diagram.colors"), tr("slides.diagram.colors_tip"), acc, |ui| {
                    for (id, key) in diagram_edit::COLORS.iter().zip(COLOR_KEYS) {
                        if toolstrip::small(ui, None, tr(key), "", acc, info.colors == *id).clicked() {
                            pick = Some(*id);
                            ui.close();
                        }
                    }
                });
                if let Some(id) = pick {
                    self.set_diagram_colors(id);
                }
                let mut pick = None;
                toolstrip::large_menu(ui, Icon::Styles, tr("slides.diagram.style"), tr("slides.diagram.style_tip"), acc, |ui| {
                    for (id, key) in diagram_edit::STYLES.iter().zip(STYLE_KEYS) {
                        if toolstrip::small(ui, None, tr(key), "", acc, info.style == *id).clicked() {
                            pick = Some(*id);
                            ui.close();
                        }
                    }
                });
                if let Some(id) = pick {
                    self.set_diagram_style(id);
                }
            });
        });
        b.group("slides.diagram.g.layout", Icon::Shapes, |ui| {
            ui.vertical(|ui| {
                ui.label(trf("slides.diagram.layout", &[("name", &tr(info.kind.key()))]));
                if !free {
                    ui.weak(tr(if info.locked { "slides.diagram.locked" } else { "slides.diagram.text_only" }));
                }
            });
        });
    }

    /// The Diagram Design tab's commands for the search, on a sample slide.
    pub fn diagram_commands(ui: &mut egui::Ui) -> Vec<toolstrip::Entry> {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let d = diagram_edit::new_diagram(diagram_edit::Kind::Process, &[Item::new(0, "A"), Item::new(0, "B")], (200.0, 60.0));
        let mut o = Object::new(0, "Diagram", ObjKind::Diagram(Arc::new(d)));
        o.sp.frame = Some(apex_slides::Frame::new(10.0, 10.0, 200.0, 60.0));
        v.insert_object("slides.undo.diagram", o);
        toolstrip::collect(|| toolstrip::within(tr("slides.diagram.design"), || toolstrip::body(ui, egui::Id::new("slides_diagram_cmds"), DIAGRAM_TAB, |b| v.diagram_tab(b))))
    }

    pub fn diagram_tab_def() -> TabDef {
        TabDef::contextual("diagram", "slides.diagram.design", DIAGRAM_TAB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> SlidesView {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let d = diagram_edit::new_diagram(diagram_edit::Kind::Process, &[Item::new(0, "Plan"), Item::new(0, "Do")], (300.0, 100.0));
        let mut o = Object::new(0, "Diagram", ObjKind::Diagram(Arc::new(d)));
        o.sp.frame = Some(apex_slides::Frame::new(10.0, 10.0, 300.0, 100.0));
        v.insert_object("slides.undo.diagram", o);
        v
    }

    fn items(v: &SlidesView) -> Vec<(usize, String)> {
        v.diagram_info().unwrap().items.into_iter().map(|i| (i.level, i.text)).collect()
    }

    #[test]
    fn edit_items_colors_and_undo() {
        let mut v = view();
        assert_eq!(items(&v), [(0, "Plan".into()), (0, "Do".into())]);
        v.diagram_add(false);
        v.diagram_add(true);
        assert_eq!(items(&v).iter().map(|i| i.0).collect::<Vec<_>>(), [0, 0, 0, 1]);
        v.diagram_remove();
        assert_eq!(items(&v).len(), 3);
        assert!(v.set_diagram_items(&diagram_edit::parse_outline("Plan\nDo\n\tTest\nShip")));
        assert_eq!(items(&v)[2], (1, "Test".into()));
        v.set_diagram_colors("colorful1");
        v.set_diagram_style("simple3");
        let i = v.diagram_info().unwrap();
        assert_eq!((i.colors.as_str(), i.style.as_str()), ("colorful1", "simple3"));
        v.pres.undo();
        v.pres.undo();
        v.pres.undo();
        assert_eq!(items(&v).len(), 3);
        v.pres.redo();
        assert_eq!(items(&v)[3], (0, "Ship".into()));
        // The text pane opens with the items and writes them back.
        v.open_text_pane();
        let (_, text) = v.diagram.open.clone().unwrap();
        assert_eq!(text, "Plan\nDo\n\tTest\nShip");
    }

    #[test]
    fn text_only_layouts_say_so() {
        let mut v = view();
        // A layout not drawn here: its items can't be added.
        let d = v.selected_diagram().unwrap().1.clone();
        let mut d = (*d).clone();
        for p in &mut d.parts {
            let x = String::from_utf8_lossy(&p.xml).replace("layout/process1", "layout/gear1");
            p.xml = Arc::from(x.into_bytes());
        }
        assert!(v.edit_diagram(|_| Some(d)));
        let before = v.notices.len();
        v.diagram_add(false);
        assert_eq!(items(&v).len(), 2);
        assert_eq!(v.notices.len(), before + 1);
        // Text still changes.
        assert!(v.set_diagram_items(&[Item::new(0, "One"), Item::new(0, "Two")]));
        assert_eq!(items(&v)[1], (0, "Two".into()));
    }

    #[test]
    fn diagram_commands_are_found() {
        let ctx = egui::Context::default();
        let mut found = Vec::new();
        ctx.run_ui(Default::default(), |ui| found = SlidesView::diagram_commands(ui)).textures_delta.clear();
        let labels: Vec<String> = found.iter().map(|e| e.label.clone()).collect();
        for key in ["slides.diagram.text_pane", "slides.diagram.add", "slides.diagram.colors", "slides.diagram.style"] {
            assert!(labels.iter().any(|l| l.contains(tr(key))), "{key}: {labels:?}");
        }
    }
}
