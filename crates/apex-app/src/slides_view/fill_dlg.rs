//! Solid and gradient fills of the selected shapes and of the slide's
//! background (Format Shape > Fill and Format Background in Office): the
//! Shape Fill and Background menus' gradient galleries and the dialog
//! behind their "More gradients…".

use apex_slides::{Background, Fill, ObjKind, SColor};
use eframe::egui;

use super::SlidesView;
use super::objects;
use crate::i18n::tr;
use crate::widgets::DialogResult;

/// What the fill dialog changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillTarget {
    Shapes,
    Background,
}

/// The fill dialog's state.
pub struct FillDlg {
    pub target: FillTarget,
    pub fill: Option<u32>,
    pub grad: Option<apex_doc::Gradient>,
    /// Background: every slide.
    pub all: bool,
    fresh: bool,
}

/// A fill as a color and a gradient (with its colors worked out).
pub fn look_of(f: &Fill, cx: &apex_slides::color::ColorCx) -> (Option<u32>, Option<apex_doc::Gradient>) {
    match f {
        Fill::None | Fill::Group => (None, None),
        Fill::Gradient(g) => {
            let g = g.resolve(cx);
            (Some(g.main_color()), Some(g))
        }
        f => (f.main_color(cx).map(|c| c.rgb), None),
    }
}

/// A color and a gradient as a fill.
pub fn fill_of(fill: Option<u32>, grad: Option<&apex_doc::Gradient>) -> Fill {
    match (fill, grad) {
        (None, _) => Fill::None,
        (Some(_), Some(g)) => Fill::Gradient(apex_slides::fill::Gradient::from_doc(g)),
        (Some(c), None) => Fill::Solid(SColor::rgb(c)),
    }
}

impl SlidesView {
    /// The first selected shape's fill, as shown.
    pub fn fill_look(&self) -> (Option<u32>, Option<apex_doc::Gradient>) {
        let Some(ch) = objects::chain(&self.pres.deck, self.target()) else { return (None, None) };
        let Some(o) = self.selected_object() else { return (None, None) };
        ch.fill(o).map_or((None, None), |f| look_of(&f, &ch.cx()))
    }

    /// The current slide's background, as shown.
    pub fn background_look(&self) -> (Option<u32>, Option<apex_doc::Gradient>) {
        let Some(ch) = objects::chain(&self.pres.deck, self.target()) else { return (None, None) };
        look_of(&ch.background(), &ch.cx())
    }

    /// Fills the selected shapes with a color or a gradient (none: no fill).
    pub fn set_fill_look(&mut self, fill: Option<u32>, grad: Option<apex_doc::Gradient>) {
        let f = fill_of(fill, grad.as_ref());
        self.change_selected("slides.undo.fill", &move |o| {
            if matches!(o.kind, ObjKind::Shape { .. } | ObjKind::Picture(_)) {
                o.sp.fill = Some(f.clone());
            }
        });
    }

    /// The background of the current slide (or of all) as a color or a
    /// gradient; no color: the layout's.
    pub fn set_background_look(&mut self, fill: Option<u32>, grad: Option<apex_doc::Gradient>, all: bool) {
        if self.read_only || self.pres.slides.is_empty() {
            return;
        }
        let bg = fill.map(|_| Background::Fill(fill_of(fill, grad.as_ref())));
        let cur = self.cur;
        let deck = self.pres.edit("slides.undo.background");
        let range = if all { 0..deck.slides.len() } else { cur..cur + 1 };
        for i in range {
            deck.slide_mut(i).background = bg.clone();
        }
    }

    pub fn open_fill_dialog(&mut self, target: FillTarget) {
        let (fill, grad) = match target {
            FillTarget::Shapes => self.fill_look(),
            FillTarget::Background => self.background_look(),
        };
        self.fill_dlg = Some(FillDlg { target, fill, grad, all: false, fresh: true });
    }

    pub(super) fn fill_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.fill_dlg else { return };
        let title = match d.target {
            FillTarget::Shapes => tr("slides.format_fill"),
            FillTarget::Background => tr("slides.format_background"),
        };
        let mut res = crate::widgets::dialog(ctx, "slides-fill", title, 420.0, |ui| {
            let none = if d.target == FillTarget::Shapes { "shape.no_fill" } else { "slides.background_reset" };
            crate::gradient_ui::fill_editor(ui, egui::Id::new("slides-fill-grad"), &mut d.fill, &mut d.grad, none);
            if d.target == FillTarget::Background {
                ui.separator();
                ui.checkbox(&mut d.all, tr("slides.background_all"));
            }
        });
        if std::mem::take(&mut d.fresh) && res == DialogResult::Cancel && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            res = DialogResult::None;
        }
        match res {
            DialogResult::None => {}
            DialogResult::Ok => {
                if let Some(d) = self.fill_dlg.take() {
                    match d.target {
                        FillTarget::Shapes => self.set_fill_look(d.fill, d.grad),
                        FillTarget::Background => self.set_background_look(d.fill, d.grad, d.all),
                    }
                }
            }
            DialogResult::Cancel => self.fill_dlg = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_slides::Frame;

    #[test]
    fn gradient_fills_and_backgrounds() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let id = v.insert_text_box(Frame::new(0.0, 0.0, 200.0, 40.0)).unwrap();
        v.stop_typing();
        v.sel = vec![id];
        let g = apex_doc::gradient::variation(0x4472C4, true, 1);
        v.set_fill_look(Some(g.main_color()), Some(g.clone()));
        assert_eq!(v.fill_look().1, Some(g.clone()));
        v.set_background_look(Some(0), Some(g.clone()), true);
        assert_eq!(v.background_look().1, Some(g));
        v.undo();
        assert!(v.background_look().1.is_none());
        v.set_fill_look(None, None);
        assert_eq!(v.fill_look(), (None, None));
    }
}
