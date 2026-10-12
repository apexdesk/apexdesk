//! Cell margins, text direction and table formulas through docx and odt.

use apex_doc::{Document, Field, FieldKind, Paragraph, Pos, Selection, TextDir};

fn sample() -> Document {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("")]);
    let s = d.insert_table(Selection::caret(Pos::new(0, 0)), 3, 2, 300.0);
    let m = d.table_map(s.cursor.para).unwrap();
    let (a10, a00) = (m.cell(1, 0).0, m.cell(0, 0).0);
    d.insert_text(Selection::caret(Pos::new(a10, 0)), "12", None);
    d.insert_text(Selection::caret(Pos::new(a00, 0)), "3", None);
    let m = d.table_map(s.cursor.para).unwrap();
    let a20 = m.cell(2, 0).0;
    d.insert_field(Selection::caret(Pos::new(a20, 0)), Field::parse("=SUM(ABOVE) \\# \"0.0\"", "").unwrap());
    let at = Selection::caret(Pos::new(m.cell(0, 1).0, 0));
    d.set_table_props(at, &|p| p.cell_margins = Some([2.0, 7.0, 3.0, 9.0]));
    d.set_cell_props(at, &|c, _| c.text_dir = TextDir::Up);
    let at = Selection::caret(Pos::new(m.cell(1, 1).0, 0));
    d.set_cell_props(at, &|c, _| c.text_dir = TextDir::Down);
    d
}

fn check(back: &Document, format: &str) {
    let m = back.table_map(back.paragraphs.iter().position(|p| p.cell.is_some()).unwrap()).unwrap();
    let props = m.props(&back.paragraphs).unwrap();
    let mar = props.margins();
    for (a, b) in mar.iter().zip([2.0, 7.0, 3.0, 9.0]) {
        assert!((a - b).abs() < 0.1, "{format}: margins {mar:?}");
    }
    assert_eq!(m.cell_props(&back.paragraphs, 0, 1).map(|c| c.text_dir), Some(TextDir::Up), "{format}");
    assert_eq!(m.cell_props(&back.paragraphs, 1, 1).map(|c| c.text_dir), Some(TextDir::Down), "{format}");
    let (_, f) = back.field_at(Pos::new(m.cell(2, 0).0, 0)).unwrap_or_else(|| panic!("{format}: no field"));
    assert_eq!(f.kind, FieldKind::Formula("SUM(ABOVE)".into()), "{format}");
    assert_eq!(f.cached, "15.0", "{format}");
}

#[test]
fn docx_table_tools() {
    let back = crate::docx::read(&crate::docx::write(&sample())).expect("docx");
    check(&back, "docx");
}

#[test]
fn odt_table_tools() {
    let back = crate::odt::read(&crate::odt::write(&sample())).expect("odt");
    check(&back, "odt");
}
