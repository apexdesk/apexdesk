//! Named styles survive docx and odt.

use apex_doc::{Align, Attrs, Document, ParaKind, ParaStyle, Paragraph, Pos, Selection, StyleDef};

fn sample() -> Document {
    let h1 = ParaStyle { kind: ParaKind::Heading(1), ..Default::default() };
    let mut d = Document::from_paragraphs(vec![Paragraph::new("Report").with_style(h1), Paragraph::new("A note in its own style."), Paragraph::new("Plain text with strong words.")]);
    let sel = Selection::caret(Pos::default());
    d.set_style(StyleDef { id: "Heading1".into(), name: "heading 1".into(), kind: ParaKind::Heading(1), builtin: true, size: Some(32), color: Some(0x2F5496), align: Some(Align::Center), ..Default::default() }, sel);
    d.set_style(StyleDef { id: "Note".into(), name: "Note".into(), italic: Some(true), color: Some(0x7F7F7F), space_after: Some(14.0), indent_left: Some(18.0), font: Some("Liberation Sans".into()), ..Default::default() }, sel);
    let n = d.styles.custom_number("Note").unwrap();
    d.apply_para_style(1, 1, sel, ParaKind::Normal, n);
    d.set_style(StyleDef { id: "Strong".into(), name: "Strong".into(), char_style: true, bold: Some(true), color: Some(0xC00000), ..Default::default() }, sel);
    d.apply_char_style(Selection { anchor: Pos::new(2, 16), cursor: Pos::new(2, 28) }, Some("Strong".into()));
    d
}

fn check(back: &Document, what: &str) {
    let h = back.char_format(&back.paragraphs[0].style, &Attrs::default());
    assert_eq!((h.size, h.bold, h.color), (32, true, Some(0x2F5496)), "{what}: heading");
    assert_eq!(back.paragraphs[0].style.align, Align::Center, "{what}: heading align");
    let note = back.styles.custom(back.paragraphs[1].style.custom).unwrap_or_else(|| panic!("{what}: note style lost"));
    assert_eq!(&*note.name, "Note", "{what}");
    let f = back.char_format(&back.paragraphs[1].style, &back.paragraphs[1].attrs_at(3));
    assert!(f.italic && f.color == Some(0x7F7F7F) && f.font.as_deref() == Some("Liberation Sans"), "{what}: note format {f:?}");
    assert_eq!(back.paragraphs[1].style.space_after, Some(14.0), "{what}");
    assert_eq!(back.paragraphs[1].style.indent_left, 18.0, "{what}");
    let a = back.paragraphs[2].attrs_at(20);
    assert_eq!(a.char_style.as_deref().and_then(|id| back.styles.by_id(id)).map(|d| &*d.name), Some("Strong"), "{what}: char style");
    let f = back.char_format(&back.paragraphs[2].style, &a);
    assert!(f.bold && f.color == Some(0xC00000), "{what}");
    assert!(back.paragraphs[2].attrs_at(3).char_style.is_none(), "{what}");
    assert_eq!(back.styles.defs.len(), 3, "{what}: {:?}", back.styles.defs.iter().map(|d| &d.id).collect::<Vec<_>>());
}

#[test]
fn docx_styles_round_trip() {
    let d = sample();
    let bytes = apex_io::docx::write(&d);
    if let Some(dir) = std::env::var_os("APEX_STYLES_OUT") {
        std::fs::write(std::path::Path::new(&dir).join("styles.docx"), &bytes).unwrap();
    }
    let back = apex_io::docx::read(&bytes).unwrap();
    check(&back, "docx");
    // Twice: nothing grows.
    let again = apex_io::docx::read(&apex_io::docx::write(&back)).unwrap();
    check(&again, "docx x2");
}

#[test]
fn plain_documents_get_no_styles() {
    let d = Document::from_paragraphs(vec![Paragraph::new("T").with_style(ParaStyle { kind: ParaKind::Title, ..Default::default() }), Paragraph::new("x").with_style(ParaStyle { kind: ParaKind::Heading(2), ..Default::default() }), Paragraph::new("q").with_style(ParaStyle { kind: ParaKind::Quote, ..Default::default() })]);
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert!(back.styles.is_empty(), "{:?}", back.styles);
}

#[test]
fn odt_styles_round_trip() {
    let d = sample();
    let bytes = apex_io::odt::write(&d);
    if let Some(dir) = std::env::var_os("APEX_STYLES_OUT") {
        std::fs::write(std::path::Path::new(&dir).join("styles.odt"), &bytes).unwrap();
    }
    let back = apex_io::odt::read(&bytes).unwrap();
    check(&back, "odt");
    let again = apex_io::odt::read(&apex_io::odt::write(&back)).unwrap();
    check(&again, "odt x2");
    let plain = Document::from_paragraphs(vec![Paragraph::new("T").with_style(ParaStyle { kind: ParaKind::Title, ..Default::default() }), Paragraph::new("x").with_style(ParaStyle { kind: ParaKind::Heading(2), ..Default::default() }), Paragraph::new("q").with_style(ParaStyle { kind: ParaKind::Quote, ..Default::default() })]);
    let back = apex_io::odt::read(&apex_io::odt::write(&plain)).unwrap();
    assert!(back.styles.is_empty(), "{:?}", back.styles);
}

/// Next style, shading, borders and tabs of styles, and linked styles on
/// part of a paragraph.
#[test]
fn next_linked_and_paragraph_looks_round_trip() {
    use apex_doc::{Line, ParaBorders, TabAlign, TabLeader, TabStop, TabStops};
    let mut d = Document::from_paragraphs(vec![Paragraph::new("Boxed text"), Paragraph::new("Some heading words here"), Paragraph::new("Plain"), Paragraph::new("Third").with_style(ParaStyle { kind: ParaKind::Heading(3), ..Default::default() })]);
    let sel = Selection::caret(Pos::default());
    let bx = ParaBorders::boxed(Line { color: 0x1F4E79, ..Default::default() });
    let tabs = TabStops::from_slice(&[TabStop { pos: 144.0, align: TabAlign::Right, leader: TabLeader::Dots }]);
    d.set_style(StyleDef { id: "Boxed".into(), name: "Boxed".into(), shading: Some(0xDDEEFF), borders: Some(bx), tabs: Some(tabs), next: Some("Heading2".into()), linked: true, bold: Some(true), ..Default::default() }, sel);
    d.set_style(StyleDef { id: "Heading3".into(), name: "heading 3".into(), kind: ParaKind::Heading(3), builtin: true, next: Some("Boxed".into()), shading: Some(0xFFF2CC), ..Default::default() }, sel);
    let n = d.styles.custom_number("Boxed").unwrap();
    d.apply_style(0, 0, sel, ParaKind::Normal, n);
    d.apply_style(1, 1, Selection { anchor: Pos::new(1, 5), cursor: Pos::new(1, 12) }, ParaKind::Heading(1), 0);
    d.apply_style(2, 2, Selection { anchor: Pos::new(2, 0), cursor: Pos::new(2, 2) }, ParaKind::Normal, n);
    for (what, back) in [("docx", apex_io::docx::read(&apex_io::docx::write(&d)).unwrap()), ("odt", apex_io::odt::read(&apex_io::odt::write(&d)).unwrap())] {
        let st = back.paragraphs[0].style;
        let def = back.styles.custom(st.custom).unwrap_or_else(|| panic!("{what}: style lost"));
        assert_eq!((def.shading, def.borders.map(|b| b.top.map(|l| l.color)), def.tabs), (Some(0xDDEEFF), Some(Some(0x1F4E79)), Some(tabs)), "{what}");
        assert_eq!(def.next.as_deref(), Some("Heading2"), "{what}");
        assert_eq!((st.shading, st.tabs, st.borders.left.is_some()), (Some(0xDDEEFF), tabs, true), "{what}");
        let h3 = back.styles.builtin(ParaKind::Heading(3)).unwrap_or_else(|| panic!("{what}: heading 3 lost"));
        assert_eq!((h3.next.as_deref(), h3.shading), (Some("Boxed"), Some(0xFFF2CC)), "{what}");
        // Linked styles: docx keeps them as such; odt as their look.
        let a = back.paragraphs[1].attrs_at(7);
        let f = back.char_format(&back.paragraphs[1].style, &a);
        assert_eq!((f.size, f.bold), (ParaKind::Heading(1).base_size(), true), "{what}");
        assert_eq!(back.paragraphs[1].style.kind, ParaKind::Normal, "{what}");
        assert!(back.char_format(&back.paragraphs[1].style, &back.paragraphs[1].attrs_at(1)).size < 30, "{what}");
        if what == "docx" {
            assert_eq!(a.char_style.as_deref(), Some("Heading1"), "{what}");
            assert!(def.linked, "{what}");
            assert_eq!(back.paragraphs[2].attrs_at(1).char_style.as_deref(), Some("Boxed"), "{what}");
            // Nothing grows on the next round.
            let again = apex_io::docx::read(&apex_io::docx::write(&back)).unwrap();
            assert_eq!(again.styles.defs.len(), back.styles.defs.len());
        }
        assert!(back.char_format(&back.paragraphs[2].style, &back.paragraphs[2].attrs_at(1)).bold, "{what}");
    }
}

/// A table style of the document's own survives docx; odt keeps its look.
#[test]
fn own_table_style_round_trip() {
    use apex_doc::{Border, Borders, TableStyle};
    let mut d = Document::from_paragraphs(vec![Paragraph::new("")]);
    let sel = d.insert_table(Selection::caret(Pos::default()), 3, 2, 300.0);
    let line = Border::new(1.0, 0x7030A0);
    let st = TableStyle { id: "Purple".into(), name: "Purple rows".into(), borders: Borders { top: Some(line), bottom: Some(line), inside_h: Some(line), ..Default::default() }, header_fill: Some(0x7030A0), header_color: Some(0xFFFFFF), header_bold: true, band_fill: Some(0xE4D7F0), ..apex_doc::PLAIN };
    d.set_table_style(st.clone(), sel);
    d.set_table_props(sel, &|p| p.style = Some("Purple".into()));
    let look = |doc: &Document, r: usize| {
        let m = doc.all_tables().into_iter().next().unwrap();
        let cell = m.cell_props(&doc.paragraphs, r, 0).cloned().unwrap_or_default();
        m.props(&doc.paragraphs).unwrap().cell_look(&doc.styles, &cell, r, 0, 3, 2)
    };
    let h = look(&d, 0);
    assert_eq!((h.fill, h.color, h.bold), (Some(0x7030A0), Some(0xFFFFFF), true));
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(back.styles.tables, vec![st], "docx");
    assert_eq!(look(&back, 1).fill, Some(0xE4D7F0));
    let odt = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    let (h, b) = (look(&odt, 0), look(&odt, 1));
    assert_eq!((h.fill, b.fill, b.top), (Some(0x7030A0), Some(0xE4D7F0), line), "odt");
    // Undo takes a new style back.
    d.undo();
    d.undo();
    assert!(d.styles.tables.is_empty());
}

/// A built-in heading set not bold (over the bold heading style it is
/// based on in odt) stays so, and keeps its style.
#[test]
fn odt_heading_not_bold_is_kept() {
    let h1 = ParaStyle { kind: ParaKind::Heading(1), ..Default::default() };
    let mut d = Document::from_paragraphs(vec![Paragraph::new("Title").with_style(h1), Paragraph::new("Text")]);
    d.set_style(StyleDef { id: "Heading1".into(), name: "Heading 1".into(), kind: ParaKind::Heading(1), builtin: true, bold: Some(false), ..Default::default() }, Selection::caret(Pos::default()));
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert_eq!(back.styles.builtin(ParaKind::Heading(1)).and_then(|s| s.bold), Some(false));
}

/// Pictures and note marks keep the formatting of their text in odt.
#[test]
fn odt_objects_keep_their_text_formatting() {
    use apex_doc::{Note, NoteKind, OBJECT, Run};
    use std::sync::Arc;
    let png: Arc<[u8]> = Arc::from(&b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x10\0\0\0\x10\x08\x06\0\0\0"[..]);
    let fmt = Attrs { underline: true, size: Some(30), ..Default::default() };
    let mut p = Paragraph::new(&format!("ab{OBJECT}{OBJECT}"));
    let note = Note { kind: NoteKind::Foot, paras: vec![Paragraph::new("n")] };
    p.runs = vec![
        Run { len: 2, attrs: fmt.clone() },
        Run { len: 3, attrs: Attrs { image: Some(Arc::new(apex_doc::Image::new(png, 20.0, 20.0))), ..fmt.clone() } },
        Run { len: 3, attrs: Attrs { note: Some(Arc::new(note)), ..fmt.clone() } },
    ];
    let d = Document::from_paragraphs(vec![p]);
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    let runs = &back.paragraphs[0].runs;
    assert!(runs.iter().any(|r| r.attrs.image.is_some()) && runs.iter().any(|r| r.attrs.note.is_some()));
    for r in runs.iter().filter(|r| r.len > 0) {
        assert!(r.attrs.underline && r.attrs.size == Some(30), "{:?}", r.attrs);
    }
}
