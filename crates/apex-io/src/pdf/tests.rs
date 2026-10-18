use apex_doc::{Align, FieldKind, HfSlot, ParaKind};

use super::*;

/// A PDF with the standard fonts F1 (Helvetica), F2 (Helvetica-Bold) and
/// F3 (Times-Roman), one page per content stream.
pub(crate) fn pdf(pages: &[String]) -> Vec<u8> {
    let mut objs: Vec<String> = Vec::new();
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 6 + i * 2)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {} /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R >> >> >>", kids.join(" "), pages.len()));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into());
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".into());
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>".into());
    for (i, c) in pages.iter().enumerate() {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>", 7 + i * 2));
        objs.push(format!("<< /Length {} >>\nstream\n{}\nendstream", c.len(), c));
    }
    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, o).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info << /Title (Informe) >> >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Text at (x, y from the top) in font `f` of `size`.
pub(crate) fn t(f: &str, size: f32, x: f32, y: f32, s: &str) -> String {
    format!("BT /{f} {size} Tf 1 0 0 1 {x} {} Tm ({s}) Tj ET\n", 792.0 - y)
}

/// Lines of body text that fill the width (so they read as one
/// paragraph), the last one short.
fn para(y: f32, words: &[&str]) -> String {
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        let x = 72.0;
        out += &t("F1", 11.0, x, y + i as f32 * 14.0, w);
    }
    out
}

const FULL: &str = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna";

#[test]
fn text_headings_and_paragraphs() {
    let mut c = t("F2", 24.0, 180.0, 90.0, "Annual report");
    c += &t("F2", 14.0, 72.0, 140.0, "Introduction");
    c += &para(170.0, &[FULL, FULL, "the end of it."]);
    c += &para(230.0, &[FULL, "second paragraph ends here."]);
    let doc = read(&pdf(&[c]), None).unwrap();
    let texts: Vec<&str> = doc.paragraphs.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts[0], "Annual report");
    assert_eq!(doc.paragraphs[0].style.kind, ParaKind::Title);
    assert_eq!(texts[1], "Introduction");
    assert!(matches!(doc.paragraphs[1].style.kind, ParaKind::Heading(_)));
    assert!(texts[2].starts_with("Lorem ipsum") && texts[2].ends_with("the end of it."), "{texts:?}");
    assert!(texts[3].ends_with("second paragraph ends here."), "{texts:?}");
    assert_eq!(texts.len(), 4, "{texts:?}");
    let a = &doc.paragraphs[2].runs[0].attrs;
    assert_eq!(a.size, Some(22));
    assert_eq!(a.font.as_deref(), Some("Arial"));
    assert!(doc.paragraphs[1].runs[0].attrs.bold);
    assert_eq!(doc.meta.title, "Informe");
    assert!((doc.page.width - 612.0).abs() < 0.5);
}

#[test]
fn ruled_tables() {
    let mut c = t("F1", 11.0, 72.0, 80.0, "Before the table.");
    // A 3 x 2 grid.
    for (i, y) in [100.0, 120.0, 140.0, 160.0].iter().enumerate() {
        let _ = i;
        c += &format!("72 {} m 372 {} l S\n", 792.0 - y, 792.0 - y);
    }
    for x in [72.0, 222.0, 372.0] {
        c += &format!("{x} {} m {x} {} l S\n", 792.0 - 100.0, 792.0 - 160.0);
    }
    for (r, (a, b)) in [("Name", "Value"), ("Alpha", "1"), ("Beta", "2")].iter().enumerate() {
        let y = 114.0 + r as f32 * 20.0;
        c += &t("F2", 10.0, 76.0, y, a);
        c += &t("F1", 10.0, 226.0, y, b);
    }
    c += &t("F1", 11.0, 72.0, 190.0, "After the table.");
    let doc = read(&pdf(&[c]), None).unwrap();
    let tables = doc.tables();
    assert_eq!(tables.len(), 1);
    let cells: Vec<&str> = doc.paragraphs.iter().filter(|p| p.cell.is_some()).map(|p| p.text.as_str()).collect();
    assert_eq!(cells, ["Name", "Value", "Alpha", "1", "Beta", "2"]);
    assert_eq!(doc.paragraphs[0].text, "Before the table.");
    assert_eq!(doc.paragraphs.last().unwrap().text, "After the table.");
    // The same table for Get Data.
    let found = super::tables(&pdf(&[{
        let mut c = String::new();
        for y in [100.0, 120.0, 140.0] {
            c += &format!("72 {} m 372 {} l S\n", 792.0 - y, 792.0 - y);
        }
        for x in [72.0, 222.0, 372.0] {
            c += &format!("{x} {} m {x} {} l S\n", 792.0 - 100.0, 792.0 - 140.0);
        }
        c += &t("F1", 10.0, 76.0, 114.0, "A");
        c += &t("F1", 10.0, 226.0, 114.0, "B");
        c += &t("F1", 10.0, 76.0, 134.0, "C");
        c
    }]), None)
    .unwrap();
    assert_eq!(found, vec![PdfTable { page: 0, rows: vec![vec!["A".into(), "B".into()], vec!["C".into(), String::new()]] }]);
}

#[test]
fn columns_of_text_make_tables() {
    let mut c = String::new();
    for (r, row) in [["Date", "Concept", "Amount"], ["02/01", "Rent", "-700.00"], ["05/01", "Salary", "2,100.00"], ["09/01", "Groceries", "-86.40"]].iter().enumerate() {
        let y = 100.0 + r as f32 * 16.0;
        c += &t("F1", 10.0, 72.0, y, row[0]);
        c += &t("F1", 10.0, 160.0, y, row[1]);
        c += &t("F1", 10.0, 400.0, y, row[2]);
    }
    let found = super::tables(&pdf(&[c]), None).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].rows[2], vec!["05/01", "Salary", "2,100.00"]);
}

#[test]
fn headers_footers_and_page_numbers() {
    let pages: Vec<String> = (1..=3)
        .map(|n| {
            let mut c = t("F1", 9.0, 72.0, 40.0, "ACME Corporation");
            c += &para(100.0, &[FULL, &format!("page {n} body.")]);
            c += &t("F1", 9.0, 300.0, 760.0, &n.to_string());
            c
        })
        .collect();
    let doc = read(&pdf(&pages), None).unwrap();
    let h = doc.headers.get(HfSlot::Header).expect("a header");
    assert_eq!(h[0].text, "ACME Corporation");
    let f = doc.headers.get(HfSlot::Footer).expect("a footer");
    assert_eq!(f[0].style.align, Align::Center);
    let field = f[0].runs.iter().find_map(|r| r.attrs.field.clone()).expect("a page field");
    assert_eq!(field.kind, FieldKind::Page);
    assert!(doc.paragraphs.iter().all(|p| !p.text.contains("ACME")));
}

#[test]
fn lists() {
    let mut c = String::new();
    for (i, item) in ["First thing", "Second thing", "Third thing"].iter().enumerate() {
        let y = 100.0 + i as f32 * 14.0;
        c += &t("F1", 11.0, 80.0, y, "\\225");
        c += &t("F1", 11.0, 98.0, y, item);
    }
    for (i, item) in ["One", "Two"].iter().enumerate() {
        let y = 160.0 + i as f32 * 14.0;
        c += &t("F1", 11.0, 80.0, y, &format!("{}.", i + 1));
        c += &t("F1", 11.0, 98.0, y, item);
    }
    let doc = read(&pdf(&[c]), None).unwrap();
    let kinds: Vec<(ParaKind, &str)> = doc.paragraphs.iter().map(|p| (p.style.kind, p.text.as_str())).collect();
    assert_eq!(kinds, [(ParaKind::Bullet, "First thing"), (ParaKind::Bullet, "Second thing"), (ParaKind::Bullet, "Third thing"), (ParaKind::Numbered, "One"), (ParaKind::Numbered, "Two")]);
}

#[test]
fn scanned_pages_need_ocr() {
    // A page with only a picture.
    let img = "q 612 0 0 792 0 0 cm BI /W 1 /H 1 /CS /G /BPC 8 ID \x7f EI Q";
    let e = read(&pdf(&[img.to_string()]), None).unwrap_err();
    assert!(e.pdf_scanned());
}

#[test]
fn damaged_cross_references_are_rebuilt() {
    let mut data = pdf(&[t("F1", 11.0, 72.0, 100.0, "Still here")]);
    // Wrong offsets everywhere.
    let x = memchr::memmem::find(&data, b"\nxref\n").unwrap();
    for b in &mut data[x + 31..x + 110] {
        if b.is_ascii_digit() {
            *b = b'9';
        }
    }
    let doc = read(&data, None).unwrap();
    assert_eq!(doc.paragraphs[0].text, "Still here");
}

#[test]
fn two_columns_read_in_order() {
    let mut c = String::new();
    for i in 0..6 {
        let y = 100.0 + i as f32 * 14.0;
        c += &t("F3", 10.0, 72.0, y, &format!("Left column line {i} with words"));
        c += &t("F3", 10.0, 320.0, y, &format!("Right column line {i} with words"));
    }
    let doc = read(&pdf(&[c]), None).unwrap();
    let all = doc.to_plain_text();
    let l5 = all.find("Left column line 5").unwrap();
    let r0 = all.find("Right column line 0").unwrap();
    assert!(l5 < r0, "{all}");
    let doc_font = doc.paragraphs[0].runs[0].attrs.font.clone();
    assert_eq!(doc_font.as_deref(), Some("Times New Roman"));
}

#[test]
fn close_columns_with_padded_rows() {
    // Rows 30 points apart; "02/01/2024" ends 7 points before the concept.
    let mut c = String::new();
    for (i, x) in [72.0, 130.0, 300.0].iter().enumerate() {
        c += &t("F1", 10.0, *x, 100.0, ["Date", "Concept", "Amount"][i]);
    }
    for (r, (d, what, n)) in [("02/01/2024", "Rent", "-700.00"), ("05/01/2024", "Salary", "2,100.00"), ("09/01/2024", "Food", "-86.40")].iter().enumerate() {
        let y = 130.0 + r as f32 * 30.0;
        c += &t("F1", 10.0, 72.0, y, d);
        c += &t("F1", 10.0, 130.0, y, what);
        c += &t("F1", 10.0, 300.0, y, n);
    }
    let found = super::tables(&pdf(&[c]), None).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].rows[0], ["Date", "Concept", "Amount"]);
    assert_eq!(found[0].rows[1], ["02/01/2024", "Rent", "-700.00"]);
}

#[test]
fn text_strings() {
    assert_eq!(text_string(b"\xFE\xFF\x00A\x00\xE9"), "Aé");
    assert_eq!(text_string(b"caf\xE9 \x84"), "café —");
}

#[test]
fn text_in_layers_off_is_not_read() {
    // A layer off as shown (7 0 R) and one on (8 0 R); a form in the one off.
    let content = format!("{}/OC /Off BDC {}EMC /OC /On BDC {}EMC /Fm Do\n", t("F1", 11.0, 72.0, 72.0, "Shown"), t("F1", 11.0, 72.0, 100.0, "Secret"), t("F1", 11.0, 72.0, 130.0, "Also"));
    let form = t("F1", 11.0, 72.0, 160.0, "Hidden");
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [7 0 R 8 0 R] /D << /OFF [7 0 R] >> >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 612 792] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> /Properties << /Off 7 0 R /On 8 0 R >> /XObject << /Fm 6 0 R >> >> /Annots [<< /Type /Annot /Subtype /Link /Rect [0 0 50 50] /F 2 /A << /S /URI /URI (http://a.example/) >> >> << /Type /Annot /Subtype /Link /Rect [0 0 50 50] /OC 7 0 R /A << /S /URI /URI (http://b.example/) >> >> << /Type /Annot /Subtype /Link /Rect [0 0 50 50] /A << /S /URI /URI (http://c.example/) >> >>] >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 612 792] /OC 7 0 R /Resources << /Font << /F1 3 0 R >> >> /Length {} >>\nstream\n{form}\nendstream", form.len()),
        "<< /Type /OCG /Name (Off) >>".into(),
        "<< /Type /OCG /Name (On) >>".into(),
    ];
    let mut out = b"%PDF-1.5\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let pages = text_pages(&out, None).unwrap();
    let text: String = pages[0].chars.iter().map(|c| c.ch).collect();
    assert_eq!(text, "ShownAlso");
    let links: Vec<&str> = pages[0].links.iter().map(|l| &*l.1).collect();
    assert_eq!(links, ["http://c.example/"]);
}
