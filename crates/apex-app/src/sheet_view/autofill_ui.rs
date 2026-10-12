//! Auto Fill Options: the button shown after dragging the fill handle,
//! to fill the cells again another way.

use apex_sheet::AutoFill;

use super::*;
use crate::icons::Icon;
use crate::widgets::Face;

/// The last dragged fill, while nothing else changed.
pub(super) struct FillOpts {
    sheet: usize,
    src: Range,
    dst: Range,
    mode: AutoFill,
    /// Workbook revision and undo depth right after the fill.
    rev: u64,
    undo: usize,
    /// The source holds dates (offer days, weekdays, months, years).
    dates: bool,
    /// Where the button was drawn last.
    pub button: Option<Rect>,
}

const MODES: [(AutoFill, &str); 5] = [
    (AutoFill::Copy, "autofill.copy"),
    (AutoFill::Series, "autofill.series"),
    (AutoFill::Formats, "autofill.formats"),
    (AutoFill::Values, "autofill.values"),
    (AutoFill::Smart, "autofill.auto"),
];

const DATE_MODES: [(AutoFill, &str); 4] = [(AutoFill::Days, "autofill.days"), (AutoFill::Weekdays, "autofill.weekdays"), (AutoFill::Months, "autofill.months"), (AutoFill::Years, "autofill.years")];

impl SheetView {
    /// Fills by dragging the handle, and offers the options.
    pub(super) fn drag_fill(&mut self, src: Range, dst: Range) {
        let before = self.wb.undo_len();
        self.wb.fill_series(self.sheet, &src, &dst);
        let sh = self.sh();
        let dates = sh.get(src.start).is_some_and(|c| matches!(c.value(), Value::Number(_)) && self.wb.styles.format(c.style).has_date_part());
        self.fill_opts = (self.wb.undo_len() > before).then_some(FillOpts { sheet: self.sheet, src, dst, mode: AutoFill::Smart, rev: self.wb.revision, undo: self.wb.undo_len(), dates, button: None });
    }

    pub(super) fn fill_options_button(&mut self, ui: &mut egui::Ui, g: &Geo) {
        let Some(f) = &self.fill_opts else { return };
        if f.rev != self.wb.revision || f.undo != self.wb.undo_len() || f.sheet != self.sheet || self.sel() != f.dst {
            self.fill_opts = None;
            return;
        }
        let r = g.range_rect(self.sh(), &f.dst);
        let at = Rect::from_min_size(pos2(r.max.x + 2.0, r.max.y + 2.0), vec2(28.0, 22.0));
        if !g.view.contains(at.min) {
            return;
        }
        let (mode, dates) = (f.mode, f.dates);
        if let Some(f) = &mut self.fill_opts {
            f.button = Some(at);
        }
        let mut pick = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
            ui.painter().rect(at, 3.0, ui.visuals().window_fill, ui.visuals().window_stroke, egui::StrokeKind::Inside);
            let resp = widgets::tool_button(ui, Face::Icon(Icon::Fill), false, tr("autofill.title"));
            egui::Popup::menu(&resp).show(|ui| {
                for (m, key) in MODES.iter().chain(if dates { &DATE_MODES[..] } else { &[] }) {
                    if ui.radio(mode == *m, tr(key)).clicked() {
                        pick = Some(*m);
                        ui.close();
                    }
                }
            });
        });
        let Some(m) = pick.filter(|m| *m != mode) else { return };
        let Some(f) = &mut self.fill_opts else { return };
        self.wb.undo();
        self.wb.auto_fill(f.sheet, &f.src, &f.dst, m);
        f.mode = m;
        f.rev = self.wb.revision;
        f.undo = self.wb.undo_len();
        self.buf_key = None;
    }
}
