use apex_doc::*;

fn doc(texts: &[&str]) -> Document {
    Document::from_paragraphs(texts.iter().map(|t| Paragraph::new(*t)).collect())
}

fn caret(p: usize, o: usize) -> Selection {
    Selection::caret(Pos::new(p, o))
}

fn range(a: (usize, usize), b: (usize, usize)) -> Selection {
    Selection { anchor: Pos::new(a.0, a.1), cursor: Pos::new(b.0, b.1) }
}

/// The table containing `para` as rows of cell texts (paragraphs joined by
/// '/', covered cells as "#").
fn grid(d: &Document, para: usize) -> Vec<Vec<String>> {
    let m = d.table_map(para).expect("table");
    (0..m.rows)
        .map(|r| {
            (0..m.cols)
                .map(|c| {
                    if !m.is_anchor(r, c) {
                        return "#".to_string();
                    }
                    let (a, b) = m.cell(r, c);
                    (a..b).map(|i| d.paragraphs[i].text.clone()).collect::<Vec<_>>().join("/")
                })
                .collect()
        })
        .collect()
}

fn g(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter().map(|r| r.iter().map(|s| s.to_string()).collect()).collect()
}

/// A document with "before", a table of the given rows and "after".
fn with_table(rows: &[&[&str]]) -> Document {
    let mut t = Table::new(rows.len(), rows[0].len(), 400.0);
    for (r, row) in rows.iter().enumerate() {
        for (c, text) in row.iter().enumerate() {
            t.rows[r][c].paras = vec![Paragraph::new(*text)];
        }
    }
    let mut ps = vec![Paragraph::new("before")];
    ps.extend(t.into_paras(7));
    ps.push(Paragraph::new("after"));
    Document::from_paragraphs(ps)
}

#[test]
fn insert_and_type() {
    let mut d = doc(&["Hello world"]);
    let s = d.insert_table(caret(0, 5), 2, 3, 300.0);
    assert_eq!(d.paragraphs.len(), 2 + 6);
    assert_eq!(d.paragraphs[0].text, "Hello");
    assert_eq!(d.paragraphs[7].text, " world");
    assert_eq!(s.cursor, Pos::new(1, 0));
    let m = d.table_map(1).unwrap();
    assert_eq!((m.rows, m.cols, m.start, m.end), (2, 3, 1, 7));
    let props = m.props(&d.paragraphs).unwrap();
    assert_eq!(props.widths, vec![100.0; 3]);
    // Typing and Enter stay in the cell.
    let p = d.insert_text(s, "one", None);
    let p = d.new_paragraph(Selection::caret(p));
    d.insert_text(Selection::caret(p), "two", None);
    assert_eq!(grid(&d, 1), g(&[&["one/two", "", ""], &["", "", ""]]));
    assert!(d.paragraphs[2].cell.as_ref().unwrap().cell.is_none());
    // Plain text of cells: tabs between cells of a row.
    assert_eq!(d.text_range(Pos::new(1, 0), Pos::new(5, 0)), "one\ntwo\t\t\n");
    // Undo everything back to the original paragraph.
    while d.can_undo() {
        d.undo();
    }
    assert_eq!(d.paragraphs.len(), 1);
    assert_eq!(d.paragraphs[0].text, "Hello world");
    // At the start of a paragraph the table goes before it.
    let mut d = doc(&["x"]);
    d.insert_table(caret(0, 0), 1, 1, 100.0);
    assert!(d.in_table(0));
    assert_eq!(d.paragraphs[1].text, "x");
    // Inside a table, a new table is nested in the cell, before its text.
    let s = d.insert_table(caret(0, 0), 1, 2, 100.0);
    assert_eq!(s.cursor.para, 0);
    assert_eq!(d.tables().len(), 1);
    assert_eq!(d.all_tables().len(), 2);
    assert_eq!((d.table_depth(0), d.table_depth(1), d.table_depth(2)), (Some(1), Some(1), Some(0)));
    let inner = d.table_map(0).unwrap();
    assert_eq!((inner.rows, inner.cols, inner.start, inner.end, inner.depth), (1, 2, 0, 2, 1));
    let outer = d.table_map(2).unwrap();
    assert_eq!((outer.rows, outer.cols, outer.start, outer.end), (1, 1, 0, 3));
    // The outer cell's properties stay on its first paragraph.
    assert!(d.paragraphs[0].cell.as_ref().unwrap().table_props.is_some());
    assert!(d.paragraphs[0].cell.as_ref().unwrap().nested.as_ref().unwrap().table_props.is_some());
    // Editing the nested table leaves the outer one alone.
    d.table_insert_rows(caret(1, 0), true);
    assert_eq!(d.table_map(0).unwrap().rows, 2);
    assert_eq!(d.table_map(4).unwrap().rows, 1);
    assert!(d.paragraphs[0].cell.as_ref().unwrap().table_props.is_some());
    // Deleting it too.
    d.delete_table(caret(0, 0));
    assert_eq!(d.all_tables().len(), 1);
    assert_eq!(d.table_depth(0), Some(0));
    assert!(d.paragraphs[0].cell.as_ref().unwrap().table_props.is_some());
    assert!(d.paragraphs[0].cell.as_ref().unwrap().cell.is_some());
    d.undo();
    assert_eq!(d.all_tables().len(), 2);
}

#[test]
fn edges_of_cells() {
    let mut d = with_table(&[&["ab", "cd"], &["ef", "gh"]]);
    // Backspace at the start of a cell and Delete at its end do nothing.
    assert_eq!(d.backspace(caret(2, 0), false), Pos::new(2, 0));
    assert_eq!(d.delete_forward(caret(1, 2), false), Pos::new(1, 2));
    assert_eq!(grid(&d, 1), g(&[&["ab", "cd"], &["ef", "gh"]]));
    // Neither across the table's edges.
    d.backspace(caret(5, 0), false);
    d.delete_forward(caret(0, 6), false);
    assert_eq!(d.paragraphs.len(), 6);
    // The caret moves through the cells.
    assert_eq!(d.next_char(Pos::new(1, 2)), Pos::new(2, 0));
    // A selection across cells clears their text but keeps the grid.
    let p = d.delete(range((1, 1), (3, 1)));
    assert_eq!(p, Pos::new(1, 1));
    assert_eq!(grid(&d, 1), g(&[&["a", ""], &["f", "gh"]]));
    d.undo();
    assert_eq!(grid(&d, 1), g(&[&["ab", "cd"], &["ef", "gh"]]));
    // Typing over it too.
    let p = d.insert_text(range((2, 1), (4, 1)), "X\nY", None);
    assert_eq!(grid(&d, 1), g(&[&["ab", "cX/Y"], &["", "h"]]));
    assert_eq!(p, Pos::new(3, 1));
    d.undo();
    // From text before into the table: text cleared up to there.
    d.delete(range((0, 3), (2, 1)));
    assert_eq!(d.paragraphs[0].text, "bef");
    assert_eq!(grid(&d, 1), g(&[&["", "d"], &["ef", "gh"]]));
    d.undo();
    // Across the whole table: it goes away and the text joins.
    d.delete(range((0, 3), (5, 2)));
    assert_eq!(d.paragraphs.len(), 1);
    assert_eq!(d.paragraphs[0].text, "befter");
    assert!(d.tables().is_empty());
    d.undo();
    assert_eq!(grid(&d, 1), g(&[&["ab", "cd"], &["ef", "gh"]]));
    // Delete in an empty paragraph before a table removes it.
    let mut d = with_table(&[&["a"]]);
    d.delete(range((0, 0), (0, 6)));
    d.delete_forward(caret(0, 0), false);
    assert!(d.in_table(0));
    // Enter at the start of a table at the top opens a paragraph above.
    let p = d.new_paragraph(caret(0, 0));
    assert_eq!(p, Pos::new(0, 0));
    assert!(!d.in_table(0) && d.in_table(1));
}

#[test]
fn rows_and_columns() {
    let mut d = with_table(&[&["a", "b"], &["c", "d"]]);
    let s = d.table_insert_rows(caret(1, 0), true).unwrap();
    assert_eq!(grid(&d, 1), g(&[&["a", "b"], &["", ""], &["c", "d"]]));
    assert_eq!(s.cursor.para, 3);
    d.table_insert_rows(caret(1, 0), false).unwrap();
    assert_eq!(grid(&d, 1).len(), 4);
    d.undo();
    // Two selected rows insert two rows.
    d.table_insert_rows(range((1, 0), (3, 0)), true).unwrap();
    assert_eq!(grid(&d, 1).len(), 5);
    d.undo();
    d.table_insert_cols(caret(2, 0), true, 1000.0).unwrap();
    assert_eq!(grid(&d, 1), g(&[&["a", "b", ""], &["", "", ""], &["c", "d", ""]]));
    d.table_insert_cols(caret(1, 0), false, 400.0).unwrap();
    assert_eq!(grid(&d, 1)[0], ["", "a", "b", ""]);
    // Kept within the width limit (or the width it had).
    let m = d.table_map(1).unwrap();
    assert!((m.props(&d.paragraphs).unwrap().width() - 600.0).abs() < 0.5);
    d.table_delete_cols(caret(1, 0)).unwrap();
    d.table_delete_rows(range((4, 0), (5, 0))).unwrap();
    assert_eq!(grid(&d, 1), g(&[&["a", "b", ""], &["c", "d", ""]]));
    // Deleting every row deletes the table.
    d.table_delete_rows(range((1, 0), (6, 0))).unwrap();
    assert!(d.tables().is_empty());
    assert_eq!(d.to_plain_text(), "before\nafter");
}

#[test]
fn merge_and_split() {
    let mut d = with_table(&[&["a", "b", "c"], &["d", "e", "f"], &["g", "h", "i"]]);
    d.merge_cells(range((1, 0), (5, 0))).unwrap();
    assert_eq!(grid(&d, 1), g(&[&["a/b/d/e", "#", "c"], &["#", "#", "f"], &["g", "h", "i"]]));
    let m = d.table_map(1).unwrap();
    assert_eq!(m.span_of(0, 0), (2, 2));
    // The caret skips the covered cells.
    let last = m.cell(0, 0).1 - 1;
    let next = d.next_char(Pos::new(last, d.paragraphs[last].len()));
    assert_eq!(d.container(next.para).map(|k| (k.1, k.2)), Some((0, 2)));
    let back = d.prev_char(next);
    assert_eq!(back.para, last);
    // A selection touching a merged cell grows over it.
    let b = d.cell_block(range((m.cell(0, 2).0, 0), (m.cell(1, 1).0, 0))).unwrap();
    assert_eq!((b.r0, b.c0, b.r1, b.c1), (0, 0, 1, 2));
    // Splitting a merged cell unmerges it.
    d.split_cell(caret(1, 0), 2, 2).unwrap();
    assert_eq!(grid(&d, 1), g(&[&["a/b/d/e", "", "c"], &["", "", "f"], &["g", "h", "i"]]));
    // Splitting a single cell adds columns and rows around it.
    let mut d = with_table(&[&["a", "b"], &["c", "d"]]);
    d.split_cell(caret(2, 0), 2, 2).unwrap();
    assert_eq!(grid(&d, 1), g(&[&["a", "b", ""], &["#", "", ""], &["c", "d", "#"]]));
    let m = d.table_map(1).unwrap();
    assert_eq!(m.span_of(0, 0), (2, 1));
    assert_eq!(m.span_of(2, 1), (1, 2));
    let w = &m.props(&d.paragraphs).unwrap().widths;
    assert_eq!(w.len(), 3);
    assert!((w[1] - w[2]).abs() < 0.01 && (w[0] - 2.0 * w[1]).abs() < 0.01);
    // Rows and columns through merged cells keep them merged.
    d.table_insert_rows(caret(2, 0), true).unwrap();
    assert_eq!(d.table_map(1).unwrap().span_of(0, 0), (3, 1));
}

#[test]
fn cell_props_and_look() {
    let mut d = with_table(&[&["h1", "h2"], &["a", "b"], &["c", "d"]]);
    let m = d.table_map(1).unwrap();
    let props = m.props(&d.paragraphs).unwrap().clone();
    assert_eq!(props.style.as_deref(), Some("TableGrid"));
    let look = props.cell_look(&Default::default(), &CellProps::default(), 1, 1, 3, 2);
    assert_eq!(look.top, Border::new(0.5, 0));
    // Shading a block and an outside border.
    d.set_cell_props(range((3, 0), (6, 0)), &|p, e| {
        p.shading = Some(0xFF0000);
        if e.right {
            p.borders.right = Some(Border::new(2.0, 0x00FF00));
        }
    });
    let m = d.table_map(1).unwrap();
    assert_eq!(m.cell_props(&d.paragraphs, 1, 0).unwrap().shading, Some(0xFF0000));
    assert_eq!(m.cell_props(&d.paragraphs, 0, 0).unwrap().shading, None);
    assert!(m.cell_props(&d.paragraphs, 2, 0).unwrap().borders.right.is_none());
    assert_eq!(m.cell_props(&d.paragraphs, 2, 1).unwrap().borders.right, Some(Border::new(2.0, 0x00FF00)));
    // A style with a header row and bands.
    d.set_table_props(caret(1, 0), &|p| p.style = Some("GridTable4-Accent1".into()));
    let props = d.table_map(1).unwrap().props(&d.paragraphs).unwrap().clone();
    let h = props.cell_look(&Default::default(), &CellProps::default(), 0, 1, 3, 2);
    assert_eq!((h.fill, h.color, h.bold), (Some(0x4472C4), Some(0xFFFFFF), true));
    assert_eq!(props.cell_look(&Default::default(), &CellProps::default(), 1, 1, 3, 2).fill, Some(0xD9E2F3));
    assert_eq!(props.cell_look(&Default::default(), &CellProps::default(), 2, 1, 3, 2).fill, None);
    // Resizing a column border moves width between the columns.
    let before = props.widths.clone();
    assert!(d.resize_column(1, 1, 30.0, caret(1, 0)));
    let w = d.table_map(1).unwrap().props(&d.paragraphs).unwrap().widths.clone();
    assert_eq!(w, vec![before[0] + 30.0, before[1] - 30.0]);
}

#[test]
fn text_and_tables() {
    let mut d = doc(&["intro", "Name\tAge", "Ana\t30", "Luis"]);
    let s = d.text_to_table(range((1, 0), (3, 0)), TextSep::Tab, 300.0).unwrap();
    assert_eq!(grid(&d, 1), g(&[&["Name", "Age"], &["Ana", "30"], &["Luis", ""]]));
    // A table at the end gets a paragraph after it.
    assert!(!d.in_table(d.paragraphs.len() - 1));
    assert_eq!(s.start(), Pos::new(1, 0));
    d.table_to_text(caret(1, 0), TextSep::Comma).unwrap();
    assert_eq!(d.to_plain_text(), "intro\nName,Age\nAna,30\nLuis,\n");
    d.undo();
    d.table_to_text(caret(2, 0), TextSep::Paragraph).unwrap();
    assert_eq!(d.to_plain_text(), "intro\nName\nAge\nAna\n30\nLuis\n\n");
    // Formatting survives the split.
    let mut p = Paragraph::new("");
    p.push("bold", Attrs { bold: true, ..Default::default() });
    p.push(";plain", Attrs::default());
    let mut d = Document::from_paragraphs(vec![p]);
    d.text_to_table(caret(0, 0), TextSep::Semicolon, 200.0).unwrap();
    assert!(d.paragraphs[0].runs[0].attrs.bold);
    assert_eq!(d.paragraphs[1].text, "plain");
    assert!(!d.paragraphs[1].runs[0].attrs.bold);
}

#[test]
fn sorting() {
    let mut d = with_table(&[&["Name", "N"], &["pear", "10"], &["Apple", "9"], &["fig", ""], &["kiwi", "1,5"]]);
    assert!(d.sort_table(caret(1, 0), 1, true, true));
    let col = |d: &Document| grid(d, 1).into_iter().map(|r| r[0].clone()).collect::<Vec<_>>();
    assert_eq!(col(&d), ["Name", "kiwi", "Apple", "pear", "fig"]);
    assert!(d.sort_table(caret(1, 0), 0, false, true));
    assert_eq!(col(&d), ["Name", "pear", "kiwi", "fig", "Apple"]);
    assert!(d.sort_table(caret(1, 0), 0, true, false));
    assert_eq!(col(&d)[0], "Apple");
    // Not with merged rows.
    let m = d.table_map(1).unwrap();
    let (a, b) = (m.cell(1, 0).0, m.cell(2, 0).0);
    d.merge_cells(range((a, 0), (b, 0))).unwrap();
    assert!(!d.sort_table(caret(1, 0), 0, true, false));
}

#[test]
fn normalize_repairs() {
    // Missing cells, stray props and a table ending the document.
    let tag = |r: u32, c: u16| {
        let mut p = Paragraph::new(format!("{r}{c}"));
        p.cell = Some(InCell::new(3, r, c));
        p
    };
    let mut first = tag(0, 0);
    first.cell.as_mut().unwrap().cell = Some(Box::new(CellProps { col_span: 5, ..Default::default() }));
    let d = Document::from_paragraphs(vec![first, tag(1, 0), tag(1, 2), tag(4, 1)]);
    let m = d.table_map(0).unwrap();
    assert_eq!((m.rows, m.cols), (3, 3));
    assert_eq!(m.span_of(0, 0), (1, 3));
    assert_eq!(grid(&d, 0), g(&[&["00", "#", "#"], &["10", "", "12"], &["", "41", ""]]));
    assert!(!d.in_table(d.paragraphs.len() - 1));
    let props = m.props(&d.paragraphs).unwrap();
    assert_eq!(props.widths.len(), 3);
    // Two tables with the same id get different ones.
    let mut ps = vec![tag(0, 0), Paragraph::new("x"), tag(0, 0)];
    ps[2].text = "other".into();
    ps[2].runs[0].len = 5;
    let d = Document::from_paragraphs(ps);
    let t = d.tables();
    assert_eq!(t.len(), 2);
    assert_ne!(d.container(0).unwrap().0, d.container(2).unwrap().0);
}

#[test]
fn clipboard_and_navigation() {
    let mut d = with_table(&[&["a", "b"], &["c", "d"]]);
    // Tab moves by cells; past the last there is nothing.
    assert_eq!(d.next_cell(1, false).unwrap(), range((2, 0), (2, 1)));
    assert_eq!(d.next_cell(3, true).unwrap().cursor, Pos::new(2, 1));
    assert!(d.next_cell(4, false).is_none());
    // A block of cells as text and as a table.
    let b = d.block_selection(range((1, 0), (4, 1))).unwrap();
    assert_eq!(d.block_text(&b), "a\tb\nc\td");
    let clip = d.block_slice(&b);
    assert_eq!(clip.len(), 4);
    // Pasting cells after the text makes a new table.
    let end = d.end();
    d.insert_paragraphs(Selection::caret(end), &clip);
    assert_eq!(d.tables().len(), 2);
    assert!(!d.in_table(d.paragraphs.len() - 1));
    let ids: Vec<u32> = d.tables().iter().map(|m| d.container(m.start).unwrap().0).collect();
    assert_ne!(ids[0], ids[1]);
    // Pasting cells into a cell nests them there.
    let n = d.paragraphs.len();
    d.insert_paragraphs(caret(1, 1), &clip);
    assert_eq!(d.paragraphs.len(), n + 5);
    assert_eq!(d.tables().len(), 2);
    assert_eq!(d.all_tables().len(), 3);
    assert_eq!(d.table_depth(2), Some(1));
    assert_eq!(d.table_map(2).unwrap().rows, 2);
    // Removing the nested table puts things back as they were.
    d.undo();
    assert_eq!(d.paragraphs.len(), n);
    // Copying inside one cell is plain text.
    assert!(d.slice(Pos::new(1, 0), Pos::new(1, 1)).iter().all(|p| p.cell.is_none()));
    // Select row / column / table.
    let s = d.select_table_part(caret(3, 0), TablePart::Column).unwrap();
    let b = d.cell_block(s).unwrap();
    assert_eq!((b.r0, b.c0, b.r1, b.c1), (0, 0, 1, 0));
    let s = d.select_table_part(caret(1, 0), TablePart::Table).unwrap();
    let b = d.cell_block(s).unwrap();
    assert_eq!((b.r1, b.c1), (1, 1));
    // Clearing a block.
    let p = d.clear_cells(range((1, 0), (d.table_map(1).unwrap().cell(1, 0).0, 0))).unwrap();
    assert_eq!(p.para, 1);
    assert_eq!(grid(&d, 1), g(&[&["", "b"], &["", "d"]]));
}

#[test]
fn text_and_a_table_paste_together() {
    // What a word processor copies: a heading, a table and a line after.
    let src = with_table(&[&["a", "b"], &["c", "d"]]);
    let clip = src.slice(Pos::new(0, 0), Pos::new(5, 5));
    assert!(clip.iter().any(|p| p.cell.is_some()) && clip.iter().any(|p| p.cell.is_none()));
    let mut d = doc(&[""]);
    d.insert_paragraphs(caret(0, 0), &clip);
    let texts: Vec<&str> = d.paragraphs.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, ["before", "a", "b", "c", "d", "after", ""]);
    assert_eq!(grid(&d, 1), g(&[&["a", "b"], &["c", "d"]]));
    assert!(!d.in_table(0) && !d.in_table(5));
    // Into a cell, the text goes in the cell around the nested table.
    let mut d = with_table(&[&["x", "y"]]);
    d.insert_paragraphs(caret(1, 1), &clip);
    assert_eq!(d.table_depth(3), Some(1));
    assert_eq!(d.table_map(1).unwrap().cols, 2);
    let before = d.paragraphs.iter().rposition(|p| p.text == "before").unwrap();
    assert_eq!(d.table_depth(before), Some(0));
}

#[test]
fn paste_into_cells() {
    let mut d = with_table(&[&["a", "b"], &["c", "d"]]);
    let cell = |t: &str| vec![Paragraph::new(t)];
    let rows = vec![vec![cell("x"), cell("y"), cell("dropped")], vec![cell("z")], vec![cell("w")]];
    // From cell (0, 1): the third column is dropped and a row is added.
    let b = d.table_map(1).unwrap().cell(0, 1).0;
    let s = d.fill_cells(caret(b, 0), &rows).unwrap();
    assert_eq!(grid(&d, 1), g(&[&["a", "x"], &["c", "z"], &["", "w"]]));
    assert_eq!(s.cursor.para, b);
    d.undo();
    assert_eq!(grid(&d, 1), g(&[&["a", "b"], &["c", "d"]]));
    assert!(d.fill_cells(caret(0, 0), &rows).is_none());
}

#[test]
fn no_border_on_a_cell_clears_its_shared_lines() {
    let mut d = with_table(&[&["a", "b", "c"], &["d", "e", "f"], &["g", "h", "i"]]);
    let p = d.table_map(1).unwrap().cell(1, 1).0;
    d.set_cell_props(caret(p, 0), &|c, _| c.borders = Borders { top: Some(Border::NONE), bottom: Some(Border::NONE), left: Some(Border::NONE), right: Some(Border::NONE), ..Default::default() });
    let m = d.table_map(1).unwrap();
    let side = |r, c| m.cell_props(&d.paragraphs, r, c).unwrap().borders;
    // The neighbors draw the same lines: their facing sides go too.
    assert_eq!(side(0, 1).bottom, Some(Border::NONE));
    assert_eq!(side(2, 1).top, Some(Border::NONE));
    assert_eq!(side(1, 0).right, Some(Border::NONE));
    assert_eq!(side(1, 2).left, Some(Border::NONE));
    // Cells not touching it keep theirs.
    assert_eq!(side(0, 0).bottom, None);
    assert_eq!(side(2, 2).left, None);
}
