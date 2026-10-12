//! Read Aloud in spreadsheets (Excel's Speak Cells): reads the selected
//! cells row by row, moving the active cell along; the selection comes
//! back when it stops.

use super::*;
use crate::read_aloud::{Item, Reader, Tick};

/// More cells than anyone listens to.
const MAX_CELLS: usize = 5000;

#[derive(Default)]
pub struct SpeakState {
    reader: Option<Reader>,
    cells: Vec<CellRef>,
    /// The selection before reading.
    saved: Option<(usize, CellRef, CellRef)>,
}

impl SheetView {
    pub(super) fn reading_aloud(&self) -> bool {
        self.speak.reader.is_some()
    }

    /// Review > Read Aloud: reads the selected cells, or stops.
    pub(super) fn toggle_read_aloud(&mut self, ctx: &egui::Context) {
        if self.speak.reader.is_some() {
            self.stop_read_aloud();
            return;
        }
        crate::speech::start(ctx);
        let s = &self.wb.sheets[self.sheet];
        let hidden = |sizes: &apex_sheet::sheet::Sizes, i: u32| sizes.get(&i).is_some_and(|w| *w <= 0.0);
        let lang = crate::read_aloud::default_lang();
        let mut cells = Vec::new();
        let mut items = Vec::new();
        for r in self.areas() {
            // Whole columns stop at the last cell used.
            let end = s.extent().map_or(r.end, |e| CellRef::new(r.end.row.min(e.row), r.end.col.min(e.col)));
            for row in r.start.row..=end.row {
                if hidden(&s.row_heights, row) {
                    continue;
                }
                for col in r.start.col..=end.col {
                    if hidden(&s.col_widths, col) || cells.len() >= MAX_CELLS {
                        continue;
                    }
                    let c = CellRef::new(row, col);
                    let text = self.wb.display(self.sheet, c).0;
                    if !text.trim().is_empty() {
                        cells.push(c);
                        items.push(Item { text, lang: lang.clone() });
                    }
                }
            }
        }
        if items.is_empty() {
            self.notices.push(Notice::info(tr("speak.nothing")));
            return;
        }
        self.speak = SpeakState { reader: Some(Reader::new(items)), cells, saved: Some((self.sheet, self.anchor, self.cursor)) };
    }

    fn stop_read_aloud(&mut self) {
        if let Some(mut r) = self.speak.reader.take() {
            r.stop();
        }
        if let Some((sheet, anchor, cursor)) = self.speak.saved.take()
            && sheet == self.sheet
        {
            self.anchor = anchor;
            self.cursor = cursor;
            self.ensure_visible = true;
        }
    }

    /// Follows the reading and shows the player at the top of `area`.
    pub(super) fn read_aloud_frame(&mut self, ctx: &egui::Context, area: egui::Rect) {
        let Some(r) = &mut self.speak.reader else { return };
        match r.tick(ctx) {
            Tick::Nothing => {}
            Tick::Moved => {
                let at = self.speak.cells.get(r.cur).copied();
                if let (Some(c), Some((sheet, ..))) = (at, self.speak.saved) {
                    if sheet == self.sheet {
                        self.anchor = c;
                        self.cursor = c;
                        self.ensure_visible = true;
                    }
                }
            }
            Tick::Finished => self.stop_read_aloud(),
            Tick::Missing(why) => {
                self.stop_read_aloud();
                self.notices.push(Notice::error(trf("speak.no_voice", &[("why", &why)])));
            }
        }
        if let Some(r) = &mut self.speak.reader
            && r.bar(ctx, area)
        {
            self.stop_read_aloud();
        }
    }
}
