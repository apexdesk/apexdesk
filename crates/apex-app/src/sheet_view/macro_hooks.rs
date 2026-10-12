//! What running a macro needs from the spreadsheet view: the active sheet
//! and selection before it runs, and afterwards the workbook it changed
//! with the sheet and cells it left selected.

use apex_sheet::{Range, Workbook};

use super::SheetView;

impl SheetView {
    /// Commits the cell being edited; returns the active sheet and the
    /// selection (what ActiveSheet and Selection start as).
    pub fn macro_context(&mut self) -> (usize, Range) {
        self.settle_edit();
        (self.sheet, self.sel())
    }

    /// Takes the workbook a macro changed (`None`: it changed nothing) and
    /// shows the sheet and selection it left.
    pub fn macro_done(&mut self, wb: Option<Workbook>, sheet: usize, sel: Range) {
        self.cancel_edit();
        if let Some(wb) = wb {
            self.wb = wb;
        }
        let sheet = sheet.min(self.wb.sheets.len().saturating_sub(1));
        if sheet != self.sheet || self.sheet >= self.wb.sheets.len() {
            self.select_sheet(sheet);
        }
        // The active cell is the top-left one, like Range.Select.
        self.anchor = sel.end;
        self.cursor = sel.start;
        self.extra.clear();
        self.buf_key = None;
        self.ensure_visible = true;
    }
}
