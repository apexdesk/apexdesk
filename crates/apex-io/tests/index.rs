//! Alphabetical indexes: entries (XE fields) and the index itself.

use std::sync::Arc;

use apex_doc::{Attrs, Document, Field, FieldKind, OBJECT, Paragraph, Pos, Selection, TocOpts};

fn sample() -> Document {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("Apples and pears."), Paragraph::new("More."), Paragraph::new("")]);
    d.insert_field(Selection::caret(Pos::new(0, 6)), Field::new(FieldKind::IndexEntry("fruit:apple".into())));
    d.insert_field(Selection::caret(Pos::new(1, 0)), Field::new(FieldKind::IndexEntry("Pear".into())));
    d.insert_toc(Selection::caret(Pos::new(2, 0)), TocOpts { index: true, letters: true, ..Default::default() }, "-");
    // Shown pages, as saving stores them.
    for p in &mut d.paragraphs {
        for r in &mut p.runs {
            if let Some(f) = &r.attrs.field
                && matches!(f.kind, FieldKind::IndexPages(_))
            {
                r.attrs = Attrs { field: Some(Arc::new(Field { cached: "1".into(), ..(**f).clone() })), ..r.attrs.clone() };
            }
        }
    }
    d
}

fn check(back: &Document, what: &str) {
    let entries: Vec<String> = back.fields().into_iter().filter_map(|(_, f)| match &f.kind {
        FieldKind::IndexEntry(t) => Some(t.clone()),
        _ => None,
    }).collect();
    assert_eq!(entries, ["fruit:apple", "Pear"], "{what}");
    let blocks = back.toc_blocks();
    assert_eq!(blocks.len(), 1, "{what}");
    let (a, b) = blocks[0];
    let t: Vec<String> = back.paragraphs[a..b].iter().map(|p| p.text.replace(OBJECT, "")).collect();
    assert_eq!(t, ["F", "fruit", "apple\t1", "P", "Pear\t1"], "{what}");
    let o = back.paragraphs[a].style.toc.unwrap();
    assert!(o.opts.index && o.opts.letters && o.opts.right_align, "{what}");
    assert_eq!(back.paragraphs[a + 2].style.toc.unwrap().level, 2, "{what}");
    // Updating rebuilds it from the entries.
    let mut u = back.clone();
    u.update_toc(a, Selection::default(), "-");
    let t: Vec<String> = u.paragraphs[a..b].iter().map(|p| p.text.replace(OBJECT, "#")).collect();
    assert_eq!(t, ["F", "fruit", "apple\t#", "P", "Pear\t#"], "{what}");
}

#[test]
fn docx_index_round_trip() {
    let bytes = apex_io::docx::write(&sample());
    if let Some(dir) = std::env::var_os("APEX_INDEX_OUT") {
        std::fs::write(std::path::Path::new(&dir).join("index.docx"), &bytes).unwrap();
    }
    let back = apex_io::docx::read(&bytes).unwrap();
    check(&back, "docx");
}

#[test]
fn odt_index_round_trip() {
    let back = apex_io::odt::read(&apex_io::odt::write(&sample())).unwrap();
    check(&back, "odt");
}

#[test]
fn docx_table_of_authorities_round_trip() {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("See the case."), Paragraph::new("")]);
    let f = Field::parse(r#" TA \l "Smith v. Jones (1990)" \s "Smith" \c 1 "#, "").unwrap();
    d.insert_field(Selection::caret(Pos::new(0, 12)), f.clone());
    d.insert_toc(Selection::caret(Pos::new(1, 0)), TocOpts { toa: 1, ..Default::default() }, "-");
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    let got: Vec<_> = back.fields().into_iter().filter(|(_, x)| matches!(x.kind, FieldKind::Authority { .. })).map(|(_, x)| x.kind.clone()).collect();
    assert_eq!(got, [f.kind]);
    let (a, _) = back.toc_blocks()[0];
    let t = back.paragraphs[a].style.toc.unwrap();
    assert_eq!((t.opts.toa, t.opts.right_align), (1, true));
    assert!(back.paragraphs[a].text.starts_with("Smith v. Jones (1990)\t"));
    // OpenDocument has no tables of authorities: the text stays.
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert!(back.paragraphs.iter().any(|p| p.text.starts_with("Smith v. Jones (1990)")));
}
