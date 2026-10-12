//! Writing a book: without distractions (the page alone, full screen,
//! with a quiet word count) and as a typewriter (the line written stays
//! half way down the view).

use super::*;

impl DocView {
    pub(super) fn typewriter(&self) -> bool {
        self.book.typewriter && self.is_book()
    }

    pub(super) fn toggle_typewriter(&mut self) {
        self.book.typewriter = !self.book.typewriter;
        self.book.tw_y = None;
        self.scroll_to_caret = true;
    }

    /// The page alone, full screen; Esc comes back.
    pub(super) fn start_quiet(&mut self) {
        self.set_view(ViewMode::Focus);
        self.book.quiet = true;
        self.book.want_fs = Some(true);
        self.focus = true;
    }

    /// Full screen asked for or left, and the end of writing without
    /// distractions when focus was left.
    pub(super) fn quiet_frame(&mut self, shell: &mut Shell) {
        if self.book.quiet && self.views.mode != ViewMode::Focus {
            self.book.quiet = false;
            if std::mem::take(&mut self.book.owns_fs) {
                self.book.want_fs = Some(false);
            }
        }
        if let Some(on) = self.book.want_fs.take()
            && shell.fullscreen != on
            && shell.cmd.is_none()
        {
            shell.cmd = Some(common::AppCmd::Fullscreen);
            self.book.owns_fs = on;
        }
    }

    pub(super) fn quiet(&self) -> bool {
        self.book.quiet && self.views.mode == ViewMode::Focus
    }

    /// The words, bottom right, barely there.
    pub(super) fn quiet_count(&mut self, ctx: &egui::Context) {
        let (words, ..) = self.book_stats();
        let today = self.doc.book.as_ref().map_or(0, |b| b.written_on(&super::book_goals::today()));
        let text = trf("book.quiet_count", &[("n", &words), ("t", &today)]);
        let screen = ctx.content_rect();
        egui::Area::new(Id::new(("book_quiet_count", self.uid))).order(egui::Order::Foreground).anchor(egui::Align2::RIGHT_BOTTOM, vec2(-18.0, -12.0)).interactable(false).show(ctx, |ui| {
            let near = ctx.pointer_hover_pos().is_some_and(|p| p.y > screen.max.y - 60.0);
            ui.set_opacity(if near { 0.9 } else { 0.4 });
            ui.label(RichText::new(text).size(12.0));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writing_without_distractions_goes_full_screen_and_back() {
        let mut v = DocView::new(Document::from_paragraphs(vec![Paragraph::new("x")]));
        v.make_book();
        let mut shell = Shell { cmd: None, show_formula_bar: true, file_name: "libro".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None };
        v.start_quiet();
        assert!(v.quiet());
        v.quiet_frame(&mut shell);
        assert!(matches!(shell.cmd.take(), Some(common::AppCmd::Fullscreen)));
        shell.fullscreen = true;
        v.set_view(ViewMode::Print);
        v.quiet_frame(&mut shell);
        assert!(!v.quiet());
        assert!(matches!(shell.cmd.take(), Some(common::AppCmd::Fullscreen)));
        // Full screen before stays.
        v.start_quiet();
        v.quiet_frame(&mut shell);
        assert!(shell.cmd.is_none());
        v.set_view(ViewMode::Print);
        v.quiet_frame(&mut shell);
        assert!(shell.cmd.is_none());
        v.toggle_typewriter();
        assert!(v.typewriter());
    }
}
