//! Cells copied in a workbook, pasted in a document: a table with their
//! text as the sheet showed it and their formats (font, colors, fill,
//! alignment, borders, column widths). One cell alone is formatted text.

use std::collections::HashMap;

use apex_doc::{Align, Attrs, Border, Borders, Cell, CellProps, LINE_BREAK, MAX_COLS, Paragraph, Table, TableProps, VAlign};
use apex_sheet::{CellStyle, Clip, HAlign, LineStyle, Value};

/// Cells a pasted table may have (more come as text).
const MAX_CELLS: usize = 20_000;
/// Points per sheet pixel.
const PX: f32 = 0.75;

/// The fields of the clipboard text of cells: rows of tab-separated
/// fields, quoted when they hold tabs, line breaks or quotes.
fn fields(text: &str) -> Vec<Vec<String>> {
    let mut rows = vec![vec![String::new()]];
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        let at_start = rows.last().is_some_and(|r| r.last().is_some_and(String::is_empty));
        let field = |rows: &mut Vec<Vec<String>>, ch: char| {
            if let Some(f) = rows.last_mut().and_then(|r| r.last_mut()) {
                f.push(ch);
            }
        };
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                field(&mut rows, '"');
            }
            '"' if quoted => quoted = false,
            '"' if at_start => quoted = true,
            '\t' if !quoted => {
                if let Some(r) = rows.last_mut() {
                    r.push(String::new());
                }
            }
            '\n' if !quoted => rows.push(vec![String::new()]),
            c => field(&mut rows, c),
        }
    }
    // The text ends with a line break: no row after it.
    if rows.len() > 1 && rows.last().is_some_and(|r| r.len() == 1 && r[0].is_empty()) {
        rows.pop();
    }
    rows
}

fn line(l: apex_sheet::Line) -> Option<Border> {
    let width = match l.style {
        LineStyle::None => return None,
        LineStyle::Hair => 0.25,
        LineStyle::Thin | LineStyle::Dashed | LineStyle::Dotted => 0.5,
        LineStyle::Medium => 1.5,
        LineStyle::Thick | LineStyle::Double => 2.25,
    };
    Some(Border { width, color: l.color.unwrap_or(0) })
}

fn attrs(st: &CellStyle) -> Attrs {
    Attrs {
        bold: st.bold,
        italic: st.italic,
        underline: st.underline,
        strike: st.strike,
        font: st.font.clone(),
        // Tenths of a point to half points.
        size: (st.font_size > 0).then(|| (st.font_size as f32 / 5.0).round() as u16),
        color: st.color,
        ..Attrs::default()
    }
}

/// The paragraphs `clip` (copied with `text` on the clipboard) is in a
/// document: a table at most `max_width` points wide. `None` when it is
/// too big for one, or the text isn't the clip's.
pub(super) fn cells_table(text: &str, clip: &Clip, max_width: f32) -> Option<Vec<Paragraph>> {
    let grid = fields(text);
    let (rows, cols) = (clip.rows as usize, clip.cols as usize);
    if grid.len() != rows || grid.iter().any(|r| r.len() != cols) || cols > MAX_COLS || rows * cols > MAX_CELLS {
        return None;
    }
    let styles: HashMap<u32, &CellStyle> = clip.styles.iter().map(|(id, st)| (*id, st)).collect();
    let cells: HashMap<(u32, u32), &apex_sheet::Cell> = clip.cells.iter().map(|(r, c, cell)| ((*r, *c), cell)).collect();
    let plain = CellStyle::default();
    let mut any_border = false;
    let mut out: Vec<Vec<Cell>> = Vec::with_capacity(rows);
    for (r, row) in grid.iter().enumerate() {
        let mut line_cells = Vec::with_capacity(cols);
        for (c, text) in row.iter().enumerate() {
            let cell = cells.get(&(r as u32, c as u32));
            let st = cell.and_then(|x| styles.get(&x.style)).copied().unwrap_or(&plain);
            let mut p = Paragraph::new("");
            p.runs[0].attrs = attrs(st);
            if !text.is_empty() {
                p.push(&text.replace('\n', &LINE_BREAK.to_string()), attrs(st));
            }
            p.style.align = match st.halign {
                HAlign::General => match cell.map(|x| x.value()) {
                    Some(Value::Number(_)) => Align::Right,
                    Some(Value::Bool(_) | Value::Error(_)) => Align::Center,
                    _ => Align::Left,
                },
                HAlign::Left => Align::Left,
                HAlign::Center => Align::Center,
                HAlign::Right => Align::Right,
                HAlign::Justify => Align::Justify,
            };
            if rows == 1 && cols == 1 {
                return Some(vec![p]);
            }
            let b = st.borders;
            any_border |= !b.is_empty();
            let borders = Borders { top: line(b.top), bottom: line(b.bottom), left: line(b.left), right: line(b.right), ..Borders::default() };
            let valign = match st.valign {
                apex_sheet::VAlign::Top => VAlign::Top,
                apex_sheet::VAlign::Center => VAlign::Center,
                apex_sheet::VAlign::Bottom => VAlign::Bottom,
            };
            line_cells.push(Cell { props: CellProps { shading: st.fill, borders, valign, ..CellProps::default() }, paras: vec![p] });
        }
        out.push(line_cells);
    }
    let mut widths: Vec<f32> = (0..clip.cols).map(|i| clip.col_widths.iter().find(|(c, _)| *c == i).map_or(apex_sheet::sheet::DEFAULT_COL_WIDTH, |(_, w)| *w) * PX).collect();
    let total: f32 = widths.iter().sum();
    if total > max_width && total > 0.0 {
        widths.iter_mut().for_each(|w| *w *= max_width / total);
    }
    // Cells with no borders at all still read as a table: a plain grid.
    let style = (!any_border).then(|| "TableGrid".into());
    Some(Table { props: TableProps { widths, style, ..TableProps::default() }, rows: out }.into_paras(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::{CellRef, Range, Workbook};

    #[test]
    fn quoted_fields() {
        assert_eq!(fields("a\tb\n\"x\ty \"\"z\"\"\nw\"\t\n"), vec![vec!["a".to_string(), "b".into()], vec!["x\ty \"z\"\nw".into(), String::new()]]);
        assert_eq!(fields("5\" tall\n"), vec![vec!["5\" tall".to_string()]]);
    }

    #[test]
    fn cells_are_a_table_with_their_formats() {
        let c = |a: &str| CellRef::parse(a).unwrap();
        let mut wb = Workbook::new();
        wb.set_input(0, c("A1"), "Item");
        wb.set_input(0, c("B1"), "1234.5");
        wb.modify_style(0, &Range::single(c("A1")), |s| {
            s.bold = true;
            s.fill = Some(0xFFEE00);
            s.font_size = 140;
        });
        wb.sheets[0].col_widths.insert(0, 200.0);
        let clip = wb.copy(0, &Range::new(c("A1"), c("B2")));
        let ps = cells_table("Item\t1234.5\n\t\n", &clip, 1000.0).unwrap();
        let t = Table::from_paras(ps);
        assert_eq!((t.nrows(), t.ncols()), (2, 2));
        assert_eq!(t.props.widths, vec![150.0, apex_sheet::sheet::DEFAULT_COL_WIDTH * PX]);
        let a1 = &t.rows[0][0];
        assert_eq!((a1.text(""), a1.props.shading), ("Item".to_string(), Some(0xFFEE00)));
        let at = a1.paras[0].attrs_at(1);
        assert!(at.bold && at.size == Some(28));
        assert_eq!(t.rows[0][1].paras[0].style.align, Align::Right);
        // Too wide for the page: the columns shrink alike.
        let ps = cells_table("Item\t1234.5\n\t\n", &clip, 108.0).unwrap();
        assert_eq!(Table::from_paras(ps).props.widths.iter().sum::<f32>().round(), 108.0);
        // One cell is text, and text that isn't the clip's is left alone.
        let one = wb.copy(0, &Range::single(c("A1")));
        let ps = cells_table("Item\n", &one, 400.0).unwrap();
        assert!(ps.len() == 1 && ps[0].cell.is_none() && ps[0].attrs_at(0).bold);
        assert!(cells_table("other\n", &clip, 400.0).is_none());
    }
}
