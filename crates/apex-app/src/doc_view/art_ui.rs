//! Artistic text in documents: the Text Styles gallery of the Insert tab
//! and the text effects under it, for the selected text or the selected
//! shape's text; with nothing selected a style inserts a text box written
//! in it.

use std::sync::Arc;

use apex_doc::{Align, Attrs, DocShape, Pos, Selection, ShapeKind, ShapePara, ShapeRun, TextFx, VAnchor};
use eframe::egui::{self, Color32};

use super::DocView;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::toolstrip;

impl DocView {
    /// Sets (None: clears) the artistic text of the selected shape's text
    /// or the selected characters.
    pub(super) fn set_text_fx(&mut self, fx: Option<TextFx>) {
        let fx = fx.map(Arc::new);
        if self.selected_shape().is_some() {
            self.change_shape_look(move |s| {
                for p in &mut s.text {
                    for r in &mut p.runs {
                        r.fx = fx.clone();
                    }
                }
            });
            return;
        }
        self.format(move |x| x.text_fx = fx.clone());
    }

    /// The artistic text of the selected shape or at the caret.
    fn current_text_fx(&self, a: &Attrs) -> Option<Arc<TextFx>> {
        match self.selected_shape() {
            Some(b) => b.shape.shape.text.iter().flat_map(|p| &p.runs).find_map(|r| r.fx.clone()),
            None => a.text_fx.clone(),
        }
    }

    /// The Text Styles gallery of the Insert tab, with the effects
    /// (fill, outline, shadow…) under it.
    pub(super) fn text_styles_button(&mut self, ui: &mut egui::Ui, accent: Color32) {
        let cur = self.current_text_fx(&self.attrs_at_caret());
        let (mut pick, mut effects) = (None, None);
        toolstrip::large_menu(ui, Icon::TextArt, tr("art.styles"), tr("art.styles_tip"), accent, |ui| {
            pick = crate::text_fx_ui::gallery(ui, cur.as_deref());
            ui.separator();
            effects = crate::text_fx_ui::menu(ui, cur.as_deref(), false);
        });
        if let Some(e) = effects {
            self.set_text_fx(e);
            return;
        }
        let Some(fx) = pick else { return };
        if self.sel.is_empty() && self.selected_shape().is_none() {
            self.insert_styled_text_box(fx);
        } else {
            self.set_text_fx(Some(fx));
        }
    }

    /// A text box without fill or outline at the caret's paragraph, with
    /// sample text in style `fx`.
    fn insert_styled_text_box(&mut self, fx: TextFx) {
        let para = self.sel.cursor.para;
        let mut shape = apex_doc::Shape::new(ShapeKind::TextBox);
        (shape.fill, shape.line) = (None, None);
        shape.valign = VAnchor::Middle;
        let run = ShapeRun { size: Some(36.0), color: fx.main_color(), fx: Some(Arc::new(fx)), ..ShapeRun::new(tr("art.sample")) };
        shape.text = vec![ShapePara { align: Align::Center, runs: vec![run], list: None }];
        let mut s = DocShape::new(shape, 330.0, 72.0);
        s.z = self.doc.shapes().iter().map(|(_, x)| x.z + 1).max().unwrap_or(0);
        s.sanitize();
        let sel = self.doc.insert_shape(Selection::caret(Pos::new(para, 0)), s);
        self.set_sel(sel);
        self.scroll_to_caret = false;
    }
}
