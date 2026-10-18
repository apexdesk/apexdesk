//! Content controls in .docx and .odt.

use std::sync::Arc;

use apex_doc::{Attrs, ContentControl, ControlKind, Document, ListItem, Paragraph};

fn sample() -> Document {
    let mut p = Paragraph::new("");
    let ctl = |id: u32, kind: ControlKind| ContentControl { title: format!("C{id}"), tag: format!("t{id}"), ..ContentControl::new(id, kind) };
    let mut push = |text: &str, c: Option<ContentControl>| p.push(text, Attrs { control: c.map(Arc::new), ..Default::default() });
    push("Name: ", None);
    push("Write here", Some(ContentControl { placeholder: true, ..ctl(1, ControlKind::PlainText) }));
    push(" ", None);
    push("☒", Some(ctl(2, ControlKind::CheckBox { checked: true })));
    push("Two", Some(ContentControl { lock_content: true, ..ctl(3, ControlKind::List { items: vec![ListItem { text: "One".into(), value: "1".into() }, ListItem { text: "Two".into(), value: "2".into() }], editable: false }) }));
    push("x", Some(ctl(4, ControlKind::List { items: vec![], editable: true })));
    push("05/03/2026", Some(ctl(5, ControlKind::Date { date: Some("2026-03-05".into()), format: "dd/MM/yyyy".into() })));
    push("rich", Some(ContentControl { lock_delete: true, ..ctl(6, ControlKind::RichText) }));
    push(" end", None);
    Document::from_paragraphs(vec![p])
}

/// The controls as (text, control without id).
fn shape(d: &Document) -> Vec<(String, ContentControl)> {
    d.controls().into_iter().map(|c| (d.paragraphs[c.para].text[c.range].to_string(), ContentControl { id: 0, ..(*c.control).clone() })).collect()
}

#[test]
fn docx_controls_round_trip() {
    let d = sample();
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(back.paragraphs[0].text, d.paragraphs[0].text);
    assert_eq!(shape(&back), shape(&d));
}

#[test]
fn odt_controls_round_trip() {
    let d = sample();
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert_eq!(back.paragraphs[0].text, d.paragraphs[0].text);
    assert_eq!(shape(&back), shape(&d));
}

#[test]
fn text_language_round_trip() {
    let mut p = Paragraph::new("");
    p.push("Hola ", Attrs::default());
    p.push("hello world ", Attrs { lang: Some(Arc::from("en-US")), ..Default::default() });
    p.push("xyzzy", Attrs { no_proof: true, ..Default::default() });
    let d = Document::from_paragraphs(vec![p]);
    let runs = |d: &Document| d.paragraphs[0].spans().filter(|(s, e, _)| e > s).map(|(s, e, a)| (d.paragraphs[0].text[s..e].to_string(), a.lang.clone(), a.no_proof)).collect::<Vec<_>>();
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(runs(&back), runs(&d));
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    assert_eq!(runs(&back), runs(&d));
}
