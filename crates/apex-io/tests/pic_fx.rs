//! Picture effects in docx and odt.

use std::sync::Arc;

use apex_doc::{Attrs, Document, Image, OBJECT, Paragraph, PicColor, PicFx};

const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn doc(fx: PicFx) -> Document {
    let img = Image { fx: Some(Arc::new(fx)), ..Image::new(Arc::from(PNG), 40.0, 30.0) };
    let mut p = Paragraph::new("");
    p.push(&OBJECT.to_string(), Attrs { image: Some(Arc::new(img)), ..Default::default() });
    Document::from_paragraphs(vec![p])
}

fn fx_of(d: &Document) -> Option<PicFx> {
    d.images().first().and_then(|(_, i)| i.fx.as_deref().cloned())
}

#[test]
fn picture_effects_round_trip() {
    let all = PicFx {
        brightness: 20,
        contrast: -10,
        color: PicColor::Grayscale,
        transparency: 30,
        border: Some((0x123456, 2.0)),
        shadow: true,
        glow: Some((0xFF0000, 6.0)),
        soft_edges: 4.0,
        reflection: true,
    };
    let back = apex_io::docx::read(&apex_io::docx::write(&doc(all.clone()))).unwrap();
    assert_eq!(fx_of(&back), Some(all.clone()));
    // OpenDocument has no reflection.
    let back = apex_io::odt::read(&apex_io::odt::write(&doc(all.clone()))).unwrap();
    assert_eq!(fx_of(&back), Some(PicFx { reflection: false, ..all }));
    for color in [PicColor::BlackWhite, PicColor::Washout, PicColor::Sepia] {
        let fx = PicFx { color, ..Default::default() };
        let back = apex_io::docx::read(&apex_io::docx::write(&doc(fx.clone()))).unwrap();
        assert_eq!(fx_of(&back), Some(fx));
    }
    let back = apex_io::docx::read(&apex_io::docx::write(&doc(PicFx::default()))).unwrap();
    assert_eq!(fx_of(&back), None);
}
