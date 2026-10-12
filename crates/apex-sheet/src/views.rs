//! Custom views (View > Custom Views): named snapshots of how the workbook
//! is shown, optionally with hidden rows, columns and filters, and with
//! the print settings. Each sheet keeps its part of every view, as xlsx
//! files store them.

use crate::addr::{CellRef, Range};
use crate::data::AutoFilter;
use crate::page::SheetPage;
use crate::workbook::{Op, Workbook};

/// A custom view of the workbook.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomView {
    pub name: String,
    /// Links the view with its part in each sheet (`{8-4-4-4-12}` hex).
    pub guid: String,
    /// The sheet shown, by name.
    pub active: String,
}

/// What a custom view keeps of one sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetView {
    pub guid: String,
    /// Zoom, in percent.
    pub zoom: u16,
    pub top_left: CellRef,
    pub cursor: CellRef,
    pub selection: Range,
    pub frozen: (u32, u32),
    pub hidden_sheet: bool,
    /// Hidden rows and columns and the filter (None: not kept).
    pub hidden: Option<Hidden>,
    /// Print settings (None: not kept).
    pub page: Option<Box<SheetPage>>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Hidden {
    pub rows: Vec<u32>,
    pub cols: Vec<u32>,
    pub filter: Option<AutoFilter>,
}

/// How the active sheet is shown when a view is added.
#[derive(Clone, Copy, Debug)]
pub struct Shown {
    pub sheet: usize,
    pub zoom: f32,
    pub top_left: CellRef,
    pub cursor: CellRef,
    pub selection: Range,
}

/// A new view id: random enough to tell views apart.
fn new_guid(seed: usize) -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut x = (t as u64) ^ ((seed as u64) << 32) ^ 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let (a, b) = (next(), next());
    format!("{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}", a >> 32, (a >> 16) & 0xFFFF, a & 0xFFFF, b >> 48, b & 0xFFFF_FFFF_FFFF)
}

impl Workbook {
    /// Adds (or replaces, by name) a view of the workbook as shown now.
    pub fn add_custom_view(&mut self, name: &str, shown: Shown, hidden: bool, print: bool) {
        let old = self.custom_views.iter().position(|v| v.name.eq_ignore_ascii_case(name));
        let guid = match old {
            Some(i) => self.custom_views[i].guid.clone(),
            None => new_guid(self.custom_views.len()),
        };
        for (i, sh) in self.sheets.iter_mut().enumerate() {
            let here = i == shown.sheet;
            let part = SheetView {
                guid: guid.clone(),
                zoom: if here { (shown.zoom * 100.0).round() as u16 } else { 100 },
                top_left: if here { shown.top_left } else { CellRef::new(0, 0) },
                cursor: if here { shown.cursor } else { CellRef::new(0, 0) },
                selection: if here { shown.selection } else { Range::single(CellRef::new(0, 0)) },
                frozen: (sh.frozen_rows, sh.frozen_cols),
                hidden_sheet: sh.hidden,
                hidden: hidden.then(|| Hidden {
                    rows: sh.row_heights.iter().filter(|(_, h)| **h <= 0.0).map(|(r, _)| *r).collect(),
                    cols: sh.col_widths.iter().filter(|(_, w)| **w <= 0.0).map(|(c, _)| *c).collect(),
                    filter: sh.filter.clone(),
                }),
                page: print.then(|| Box::new(sh.page.clone())),
            };
            sh.custom_views.retain(|v| v.guid != guid);
            sh.custom_views.push(part);
        }
        let view = CustomView { name: name.to_string(), guid, active: self.sheets[shown.sheet].name.clone() };
        match old {
            Some(i) => self.custom_views[i] = view,
            None => self.custom_views.push(view),
        }
        self.modified = true;
    }

    pub fn delete_custom_view(&mut self, i: usize) {
        let v = self.custom_views.remove(i);
        for sh in &mut self.sheets {
            sh.custom_views.retain(|x| x.guid != v.guid);
        }
        self.modified = true;
    }

    /// Shows view `i`: restores the sheets (one undo step) and returns how
    /// the active sheet is shown.
    pub fn show_custom_view(&mut self, i: usize) -> Option<Shown> {
        let v = self.custom_views.get(i)?.clone();
        let mut ops = Vec::new();
        for (si, sh) in self.sheets.iter().enumerate() {
            let Some(p) = sh.custom_views.iter().find(|x| x.guid == v.guid) else { continue };
            if p.hidden_sheet != sh.hidden && sh.name != v.active {
                ops.push(Op::SheetProps { index: si, hidden: p.hidden_sheet, tab_color: sh.tab_color });
            }
            if p.frozen != (sh.frozen_rows, sh.frozen_cols) {
                ops.push(Op::Freeze { sheet: si, rows: p.frozen.0, cols: p.frozen.1, top: CellRef::new(0, 0) });
            }
            if let Some(h) = &p.hidden {
                let rows: Vec<(u32, Option<f32>)> = sh
                    .row_heights
                    .iter()
                    .filter(|(r, x)| **x <= 0.0 && !h.rows.contains(r))
                    .map(|(r, _)| (*r, None))
                    .chain(h.rows.iter().filter(|r| sh.row_heights.get(r).is_none_or(|x| *x > 0.0)).map(|r| (*r, Some(0.0))))
                    .collect();
                if !rows.is_empty() {
                    ops.push(Op::RowHeights { sheet: si, sizes: rows });
                }
                let cols: Vec<(u32, Option<f32>)> = sh
                    .col_widths
                    .iter()
                    .filter(|(c, x)| **x <= 0.0 && !h.cols.contains(c))
                    .map(|(c, _)| (*c, None))
                    .chain(h.cols.iter().filter(|c| sh.col_widths.get(c).is_none_or(|x| *x > 0.0)).map(|c| (*c, Some(0.0))))
                    .collect();
                if !cols.is_empty() {
                    ops.push(Op::ColWidths { sheet: si, sizes: cols });
                }
                if h.filter != sh.filter {
                    ops.push(Op::Filter { sheet: si, filter: h.filter.clone() });
                }
            }
            if let Some(page) = &p.page {
                if **page != sh.page {
                    ops.push(Op::PageSetup { sheet: si, page: page.clone() });
                }
            }
        }
        if !ops.is_empty() {
            self.execute_as(Op::Batch(ops), "undo.custom_view");
        }
        let sheet = self.sheet_index(&v.active).unwrap_or(0);
        let p = self.sheets[sheet].custom_views.iter().find(|x| x.guid == v.guid)?;
        Some(Shown { sheet, zoom: p.zoom as f32 / 100.0, top_left: p.top_left, cursor: p.cursor, selection: p.selection })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::Sheet;

    #[test]
    fn custom_views_restore() {
        let mut w = Workbook::new();
        w.execute(Op::AddSheet { index: 1, sheet: Box::new(Sheet::new("Two")) });
        w.set_hidden(0, true, 3, 5, true);
        let shown = Shown { sheet: 1, zoom: 1.5, top_left: CellRef::new(10, 2), cursor: CellRef::new(12, 3), selection: Range::parse("D13:E14").unwrap() };
        w.add_custom_view("Mine", shown, true, false);
        assert_eq!(w.custom_views.len(), 1);
        assert_eq!(w.custom_views[0].guid.len(), 38);
        w.set_hidden(0, true, 3, 5, false);
        w.set_hidden(0, true, 20, 20, true);
        let s = w.show_custom_view(0).unwrap();
        assert_eq!(s.sheet, 1);
        assert_eq!(s.zoom, 1.5);
        assert_eq!(s.cursor, CellRef::new(12, 3));
        assert_eq!(w.sheets[0].row_height(4), 0.0);
        assert!(w.sheets[0].row_height(20) > 0.0);
        // Adding it again replaces it.
        w.add_custom_view("mine", shown, false, true);
        assert_eq!(w.custom_views.len(), 1);
        assert_eq!(w.sheets[0].custom_views.len(), 1);
        w.delete_custom_view(0);
        assert!(w.sheets[1].custom_views.is_empty());
    }
}
