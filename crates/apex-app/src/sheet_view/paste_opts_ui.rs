//! Paste Options: the button shown after pasting copied cells, to paste
//! them again another way.

use super::*;
use crate::icons::Icon;
use crate::widgets::Face;

/// The last paste of copied cells, while nothing else changed.
pub(super) struct PasteOpts {
    sheet: usize,
    dst: Range,
    /// Undo steps the paste took.
    steps: usize,
    pick: usize,
    /// Workbook revision and undo depth right after the paste.
    rev: u64,
    undo: usize,
    /// Where the button was drawn last.
    pub button: Option<Rect>,
}

const MODES: [(&str, PasteWhat, bool); 6] = [
    ("edit.paste", PasteWhat::All, false),
    ("paste.values", PasteWhat::Values, false),
    ("paste.formulas", PasteWhat::Formulas, false),
    ("paste.formats", PasteWhat::Formats, false),
    ("paste.no_borders", PasteWhat::AllExceptBorders, false),
    ("paste.transpose", PasteWhat::All, true),
];

impl SheetView {
    /// Offers the options after a paste that took `before`'s undo depth
    /// further.
    pub(super) fn pasted_cells(&mut self, before: usize) {
        let steps = self.wb.undo_len().saturating_sub(before);
        self.paste_opts = (steps > 0).then(|| PasteOpts { sheet: self.sheet, dst: self.sel(), steps, pick: 0, rev: self.wb.revision, undo: self.wb.undo_len(), button: None });
    }

    pub(super) fn paste_live(&self) -> bool {
        self.paste_opts.as_ref().is_some_and(|p| p.rev == self.wb.revision && p.undo == self.wb.undo_len() && p.sheet == self.sheet && self.sel() == p.dst && self.clip.is_some())
    }

    pub(super) fn paste_options_button(&mut self, ui: &mut egui::Ui, g: &Geo) {
        if !self.paste_live() {
            self.paste_opts = None;
            return;
        }
        let Some(dst) = self.paste_opts.as_ref().map(|p| p.dst) else { return };
        let r = g.range_rect(self.sh(), &dst);
        let at = Rect::from_min_size(pos2(r.max.x + 2.0, r.max.y + 2.0), vec2(28.0, 22.0));
        if !g.view.contains(at.min) {
            return;
        }
        let Some(p) = &mut self.paste_opts else { return };
        p.button = Some(at);
        let cur = p.pick;
        let mut pick = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
            ui.painter().rect(at, 3.0, ui.visuals().window_fill, ui.visuals().window_stroke, egui::StrokeKind::Inside);
            let resp = widgets::tool_button(ui, Face::Icon(Icon::Paste), false, tr("paste_opts.title"));
            egui::Popup::menu(&resp).show(|ui| {
                ui.weak(tr("paste_opts.title"));
                for (i, (key, ..)) in MODES.iter().enumerate() {
                    if ui.radio(cur == i, tr(key)).clicked() {
                        pick = Some(i);
                        ui.close();
                    }
                }
            });
        });
        let Some(i) = pick.filter(|i| *i != cur) else { return };
        let Some(p) = self.paste_opts.take() else { return };
        for _ in 0..p.steps {
            self.wb.undo();
        }
        self.anchor = p.dst.start;
        self.cursor = p.dst.start;
        let before = self.wb.undo_len();
        let (_, what, transpose) = MODES[i];
        self.paste_special_now(PasteSpecial { what, transpose, ..Default::default() });
        self.pasted_cells(before);
        if let Some(n) = &mut self.paste_opts {
            n.pick = i;
        }
        self.buf_key = None;
    }
}

#[cfg(test)]
mod tests {
    use apex_sheet::{CellRef, PasteSpecial, PasteWhat, Range, Workbook};

    use super::*;

    fn c(a1: &str) -> CellRef {
        CellRef::parse(a1).unwrap()
    }

    #[test]
    fn cells_go_from_one_workbook_to_another() {
        let ctx = egui::Context::default();
        let mut wb = Workbook::new();
        wb.set_input(0, c("A1"), "3");
        wb.set_input(0, c("A2"), "=A1*2");
        wb.modify_style(0, &Range::single(c("A2")), |s| {
            s.bold = true;
            s.fill = Some(0xFFFF00);
        });
        let mut a = SheetView::new(wb);
        a.select_range(c("A1"), c("A2"));
        a.copy(&ctx, false);
        let text = a.clip.as_ref().unwrap().0.clone();

        // Another workbook, with formats of its own.
        let mut wb = Workbook::new();
        wb.set_input(0, c("F1"), "x");
        wb.modify_style(0, &Range::single(c("F1")), |s| s.italic = true);
        let mut b = SheetView::new(wb);
        b.select_range(c("C1"), c("C1"));
        b.paste(&text);
        let style = |v: &SheetView, a1: &str| v.wb.styles.get(v.wb.cell(0, c(a1)).map_or(0, |x| x.style)).clone();
        assert_eq!(b.wb.edit_text(0, c("C2")), "=C1*2");
        assert_eq!(b.wb.display(0, c("C2")).0, "6");
        assert_eq!(style(&b, "C2"), style(&a, "A2"));
        assert!(style(&b, "F1").italic && !style(&b, "F1").bold);
        // The source's sheet isn't here (no marquee), and its workbook was
        // never saved: no file to link to.
        assert!(matches!(b.clip, Some((_, _, None, false))));
        b.select_range(c("E1"), c("E1"));
        b.paste_link();
        assert!(b.wb.cell(0, c("E1")).is_none());
        // Paste special takes the same cells, without pasting first.
        let mut d = SheetView::new(Workbook::new());
        d.select_range(c("B1"), c("B1"));
        d.paste_special_now(PasteSpecial { what: PasteWhat::Values, ..Default::default() });
        assert_eq!(d.wb.edit_text(0, c("B2")), "6");
        assert!(!style(&d, "B2").bold);
    }

    #[test]
    fn cells_reach_other_programs_and_other_apexdesks_formatted() {
        use crate::sysclip::{CELLS, HTML, TEXT};
        let ctx = egui::Context::default();
        let mut wb = Workbook::new();
        wb.set_input(0, c("A1"), "3");
        wb.set_input(0, c("A2"), "=A1*2");
        wb.modify_style(0, &Range::single(c("A2")), |s| s.bold = true);
        let mut a = SheetView::new(wb);
        a.select_range(c("A1"), c("A2"));
        a.copy(&ctx, false);
        assert_eq!(crate::sysclip::formats(), [TEXT, HTML, CELLS]);
        let html = crate::sysclip::text_of(&crate::sysclip::read(&[HTML]).unwrap().1);
        assert!(html.contains("<table") && html.contains("font-weight:bold") && html.contains("<b>6</b></span></td>"), "{html}");
        // ApexDesk in another process gets the cells, formulas and all.
        let mut b = SheetView::new(Workbook::new());
        let clip = b.system_clip("3\n6\n").expect("cells");
        b.clip = Some(("3\n6\n".into(), clip, None, false));
        b.select_range(c("C3"), c("C3"));
        b.paste("3\n6\n");
        assert_eq!(b.wb.edit_text(0, c("C4")), "=C3*2");
        assert!(b.wb.styles.get(b.wb.cell(0, c("C4")).unwrap().style).bold);
    }

    #[test]
    fn tables_of_other_spreadsheets_paste_formatted() {
        use crate::sysclip::{HTML, TEXT};
        let html = "<style>.xl65 {font-weight:700; background:#FFFF00}</style><table><tr><td class=xl65>Item</td><td x:num=\"1234.5\">1,234.50</td></tr></table>";
        crate::sysclip::write(&[(TEXT, b"Item\t1,234.50\n"), (HTML, html.as_bytes())]);
        let mut b = SheetView::new(Workbook::new());
        b.select_range(c("B2"), c("B2"));
        b.paste("Item\t1,234.50\n");
        let style = b.wb.styles.get(b.wb.cell(0, c("B2")).unwrap().style).clone();
        assert!(style.bold && style.fill == Some(0xFFFF00));
        assert_eq!(b.wb.cell(0, c("C2")).unwrap().value(), &apex_sheet::Value::Number(1234.5));
        // A web page whose table is only part of what was copied pastes
        // as text.
        crate::sysclip::write(&[(TEXT, b"Intro\nItem\t2\n"), (HTML, b"<p>Intro</p><table><tr><td>Item</td><td>2</td></tr></table>")]);
        let mut d = SheetView::new(Workbook::new());
        d.select_range(c("A1"), c("A1"));
        d.paste("Intro\nItem\t2\n");
        assert_eq!(d.wb.display(0, c("A1")).0, "Intro");
        assert_eq!(d.wb.display(0, c("B2")).0, "2");
    }

    #[test]
    fn the_format_painter_goes_to_another_workbook() {
        let mut wb = Workbook::new();
        wb.set_input(0, c("A1"), "3");
        wb.modify_style(0, &Range::single(c("A1")), |s| {
            s.bold = true;
            s.fill = Some(0x00FF00);
        });
        let mut a = SheetView::new(wb);
        a.select_range(c("A1"), c("A1"));
        a.start_brush(false);
        a.sync_brush();
        assert!(a.brush.is_some());

        // The other workbook has formats of its own: numbers differ.
        let mut wb = Workbook::new();
        wb.set_input(0, c("B2"), "x");
        wb.modify_style(0, &Range::single(c("F1")), |s| s.italic = true);
        let mut b = SheetView::new(wb);
        b.sync_brush();
        b.select_range(c("B2"), c("B2"));
        b.apply_brush();
        let st = b.wb.styles.get(b.wb.cell(0, c("B2")).unwrap().style).clone();
        assert!(st.bold && st.fill == Some(0x00FF00) && !st.italic);
        // Painted once: off there, and off where it was picked up.
        b.sync_brush();
        a.sync_brush();
        assert!(a.brush.is_none() && b.brush.is_none());
        // Escape where it was picked up turns it off in the other too.
        a.start_brush(true);
        b.sync_brush();
        assert!(b.brush.as_ref().is_some_and(|x| x.sticky));
        a.brush = None;
        a.sync_brush();
        b.sync_brush();
        assert!(b.brush.is_none());
    }

    #[test]
    fn widths_rules_links_and_pictures_come_from_another_workbook() {
        let ctx = egui::Context::default();
        let mut wb = Workbook::new();
        wb.set_input(0, c("A1"), "3");
        wb.set_input(0, c("B1"), "=A1*2");
        wb.sheets[0].col_widths.insert(1, 140.0);
        let mut v = apex_sheet::data::Validation::new(vec![Range::single(c("A1"))]);
        v.kind = apex_sheet::data::ValKind::Whole;
        v.f1 = "1".into();
        v.f2 = "9".into();
        wb.execute(apex_sheet::Op::Validations { sheet: 0, list: vec![v] });
        let mut a = SheetView::new(wb);
        a.set_file_dir(Some(std::path::Path::new("/books")));
        a.set_file_name("Budget.xlsx".into());
        a.select_range(c("A1"), c("B1"));
        a.copy(&ctx, false);

        let mut b = SheetView::new(Workbook::new());
        b.set_file_dir(Some(std::path::Path::new("/books")));
        b.ctx = Some(ctx.clone());
        b.select_range(c("D4"), c("D4"));
        b.paste_special_now(PasteSpecial { what: PasteWhat::ColumnWidths, ..Default::default() });
        assert_eq!(b.wb.sheets[0].col_widths.get(&4), Some(&140.0));
        b.paste_special_now(PasteSpecial { what: PasteWhat::Validation, ..Default::default() });
        assert_eq!(b.wb.sheets[0].validations[0].ranges, vec![Range::single(c("D4"))]);
        assert!(b.wb.cell(0, c("D4")).is_none());
        b.paste_link();
        assert_eq!(b.wb.edit_text(0, c("E4")), "=[Budget.xlsx]Sheet1!B1");
        assert_eq!(b.wb.display(0, c("E4")).0, "6");
        assert_eq!(b.wb.externals[0].target, "Budget.xlsx");
        crate::fonts::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        b.paste_picture();
        let img = &b.wb.sheets[0].images[0];
        assert_eq!(img.width, a.wb.sheets[0].col_width(0) + 140.0);
    }
}
