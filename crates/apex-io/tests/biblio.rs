//! Citations, sources and bibliographies.

use apex_doc::{CiteStyle, Document, Field, FieldKind, OBJECT, Paragraph, Pos, Selection, Source, SourceKind, TocOpts};

fn sample(style: CiteStyle) -> Document {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("As said ."), Paragraph::new("")]);
    d.biblio.style = style;
    d.biblio.sources = vec![
        Source { tag: "Doe20".into(), kind: SourceKind::Book, authors: "Doe, John; Roe, Ann".into(), title: "Rust & Things".into(), year: "2020".into(), city: "Madrid".into(), publisher: "Pub".into(), ..Default::default() },
        Source { tag: "Web1".into(), kind: SourceKind::Web, authors: "Acme".into(), title: "Site".into(), year: "2024".into(), journal: "Acme site".into(), url: "https://x.test/a?b=1".into(), ..Default::default() },
        Source { tag: "Art9".into(), kind: SourceKind::Article, authors: "Smith, Al".into(), title: "On X".into(), year: "1999".into(), journal: "J. Stuff".into(), volume: "4".into(), pages: "1-9".into(), ..Default::default() },
    ];
    d.insert_field(Selection::caret(Pos::new(0, 8)), Field { cached: "(Doe & Roe, 2020)".into(), ..Field::new(FieldKind::Citation("Doe20".into())) });
    d.insert_field(Selection::caret(Pos::new(0, 7)), Field { cached: "(Smith, 1999)".into(), ..Field::new(FieldKind::Citation("Art9".into())) });
    d.insert_toc(Selection::caret(Pos::new(1, 0)), TocOpts { bib: true, ..Default::default() }, "-");
    d
}

fn check(back: &Document, orig: &Document, what: &str) {
    let mut a = back.biblio.sources.clone();
    let mut b = orig.biblio.sources.clone();
    a.sort_by(|x, y| x.tag.cmp(&y.tag));
    b.sort_by(|x, y| x.tag.cmp(&y.tag));
    if what.starts_with("odt") {
        // Only cited sources travel in ODF.
        b.retain(|s| s.tag != "Web1");
    }
    assert_eq!(a, b, "{what}");
    let cites: Vec<String> = back.fields().into_iter().filter_map(|(_, f)| match &f.kind {
        FieldKind::Citation(t) => Some(format!("{t}={}", f.cached)),
        _ => None,
    }).collect();
    assert_eq!(cites, ["Art9=(Smith, 1999)", "Doe20=(Doe & Roe, 2020)"], "{what}");
    let (s, e) = back.toc_blocks()[0];
    assert!(back.paragraphs[s].style.toc.unwrap().opts.bib, "{what}");
    let t: Vec<String> = back.paragraphs[s..e].iter().map(|p| p.text.replace(OBJECT, "")).collect();
    let o: Vec<String> = orig.toc_blocks().first().map(|&(s, e)| orig.paragraphs[s..e].iter().map(|p| p.text.clone()).collect()).unwrap();
    assert_eq!(t, o, "{what}");
    // The title is italic.
    let p = back.paragraphs[s..e].iter().find(|p| p.text.contains("Rust")).unwrap();
    let at = p.text.find("Rust").unwrap();
    assert!(p.attrs_at(at + 1).italic, "{what}");
}

#[test]
fn docx_biblio_round_trip() {
    for st in CiteStyle::ALL {
        let d = sample(st);
        let bytes = apex_io::docx::write(&d);
        if let Some(dir) = std::env::var_os("APEX_BIB_OUT") {
            std::fs::write(std::path::Path::new(&dir).join("biblio.docx"), &bytes).unwrap();
        }
        let back = apex_io::docx::read(&bytes).unwrap();
        assert_eq!(back.biblio.style, st);
        check(&back, &d, "docx");
    }
}

#[test]
fn docx_keeps_the_style_without_sources() {
    for st in CiteStyle::ALL {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("x")]);
        d.biblio.style = st;
        let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
        assert_eq!(back.biblio.style, st);
        assert!(back.biblio.sources.is_empty());
    }
}

#[test]
fn odt_biblio_round_trip() {
    let d = sample(CiteStyle::Apa);
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    check(&back, &d, "odt");
    for st in CiteStyle::ALL {
        let back = apex_io::odt::read(&apex_io::odt::write(&sample(st))).unwrap();
        assert_eq!(back.biblio.style, st);
    }
}
