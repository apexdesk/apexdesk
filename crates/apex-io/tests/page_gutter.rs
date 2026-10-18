//! Gutter, mirrored margins and vertical alignment through the formats.

use apex_doc::{Document, PageSetup, PageVAlign, Paragraph};
use apex_io::Format;

fn sample() -> Document {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("Portada")]);
    d.page = PageSetup { gutter: 36.0, mirror: true, v_align: PageVAlign::Center, margin_left: 80.0, margin_right: 50.0, ..PageSetup::A4 };
    d
}

fn round(d: &Document, fmt: Format, ext: &str) -> Document {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("page_gutter");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("a.{ext}"));
    apex_io::save_doc(d, &path, fmt).unwrap();
    match apex_io::open(&path).unwrap().0 {
        apex_io::Loaded::Doc(d) => *d,
        _ => panic!("not a document"),
    }
}

#[test]
fn page_layout_round_trip() {
    let d = sample();
    for (fmt, ext, valign) in [(Format::Docx, "docx", true), (Format::Rtf, "rtf", true), (Format::Odt, "odt", false)] {
        let p = round(&d, fmt, ext).page;
        assert!((p.gutter - 36.0).abs() < 0.2, "{ext}: {}", p.gutter);
        assert!(p.mirror, "{ext}");
        assert!((p.margin_left - 80.0).abs() < 0.2, "{ext}");
        if valign {
            assert_eq!(p.v_align, PageVAlign::Center, "{ext}");
        }
    }
    // Gutter at the top.
    let mut d = sample();
    d.page.mirror = false;
    d.page.gutter_top = true;
    for (fmt, ext) in [(Format::Docx, "docx"), (Format::Rtf, "rtf")] {
        let p = round(&d, fmt, ext).page;
        assert!(p.gutter_top && !p.mirror, "{ext}");
    }
}

#[test]
fn text_area() {
    let p = PageSetup { gutter: 20.0, mirror: true, margin_left: 70.0, margin_right: 50.0, ..PageSetup::A4 };
    assert_eq!(p.content_width(), p.width - 140.0);
    // Right-hand pages: gutter inside (left); left-hand pages: outside left.
    assert_eq!(p.text_left(0), 90.0);
    assert_eq!(p.text_left(1), 50.0);
    let top = PageSetup { gutter: 20.0, gutter_top: true, ..PageSetup::A4 };
    assert_eq!((top.text_top(), top.text_left(1)), (92.0, 72.0));
    assert_eq!(top.content_height(), top.height - 164.0);
}
