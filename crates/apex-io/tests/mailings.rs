//! Labels, envelopes and merged documents survive docx and odt.
//! With `APEX_MAIL_OUT=dir` the files are also written there (for checking
//! them in LibreOffice).

use apex_doc::merge::{self, ENVELOPES, LABELS};
use apex_doc::{Document, Paragraph};

fn samples() -> Vec<(&'static str, Document)> {
    let people = ["Ana García\nCalle Mayor 5\n28013 Madrid", "John Smith\n12 High Street\nLondon", "Marie Curie\n3 rue des Écoles\n75005 Paris"];
    let texts: Vec<String> = people.iter().map(|s| s.to_string()).collect();
    let letter = Document::from_paragraphs(vec![Paragraph::new("Dear «Name»,"), Paragraph::new("You owe «Amount» EUR.")]);
    let names = vec!["Name".to_string(), "Amount".to_string()];
    let records = vec![vec!["Ana".to_string(), "10".to_string()], vec!["John".to_string(), "20".to_string()]];
    vec![
        ("labels", merge::labels(&LABELS[0], &texts)),
        ("envelopes", merge::envelopes(&ENVELOPES[0], "Apex Ltd.\n1 Main St", &texts)),
        ("merged", merge::merge(&letter, &names, &records)),
    ]
}

fn text(d: &Document) -> String {
    d.text_range(apex_doc::Pos::default(), d.end())
}

#[test]
fn mail_documents_round_trip() {
    let out = std::env::var_os("APEX_MAIL_OUT").map(std::path::PathBuf::from);
    for (name, doc) in samples() {
        for (ext, bytes) in [("docx", apex_io::docx::write(&doc)), ("odt", apex_io::odt::write(&doc))] {
            let back = if ext == "docx" { apex_io::docx::read(&bytes) } else { apex_io::odt::read(&bytes) }.unwrap();
            assert_eq!(text(&back), text(&doc), "{name}.{ext}");
            assert!((back.page.width - doc.page.width).abs() < 1.0 && (back.page.height - doc.page.height).abs() < 1.0, "{name}.{ext} page size");
            if let Some(dir) = &out {
                std::fs::write(dir.join(format!("{name}.{ext}")), &bytes).unwrap();
            }
        }
    }
}
