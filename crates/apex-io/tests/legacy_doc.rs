//! Word 97-2003 (.doc) beyond the text: comments, bookmarks, tab stops,
//! borders and shading, table borders, even page headers, a text box and a
//! floating picture, in a file built here structure by structure.

use std::io::{Cursor, Write};
use std::path::PathBuf;

use apex_doc::{Border, HfSlot, LineStyle, ShapeKind, ShapeWrap, TabAlign, TabLeader, VRel, Wrap, comment_id};
use apex_io::Loaded;

/// A 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn ole(streams: &[(&str, &[u8])]) -> Vec<u8> {
    let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    for (name, data) in streams {
        c.create_stream(name).unwrap().write_all(data).unwrap();
    }
    c.flush().unwrap();
    c.into_inner().into_inner()
}

fn u16s(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

/// An OfficeArt record.
fn art(ver: u16, inst: u16, ty: u16, body: &[u8]) -> Vec<u8> {
    [&u16s(&[ver | inst << 4, ty])[..], &(body.len() as u32).to_le_bytes(), body].concat()
}

/// Property tables: (id, value).
fn opt(props: &[(u16, u32)]) -> Vec<u8> {
    art(3, props.len() as u16, 0xF00B, &props.iter().flat_map(|&(id, v)| [&id.to_le_bytes()[..], &v.to_le_bytes()].concat()).collect::<Vec<u8>>())
}

fn sp(spt: u16, spid: u32, props: &[(u16, u32)]) -> Vec<u8> {
    art(0xF, 0, 0xF004, &[art(2, spt, 0xF00A, &u32s(&[spid, 0xA00])), opt(props)].concat())
}

/// A floating shape's anchor: its place in twips and its flags.
fn fspa(spid: u32, rect: [i32; 4], flags: u16) -> Vec<u8> {
    [&spid.to_le_bytes()[..], &rect.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>(), &flags.to_le_bytes(), &0i32.to_le_bytes()].concat()
}

#[test]
fn reads_word_97_comments_bookmarks_and_drawings() {
    // The stories: main text, headers, comments and text boxes.
    let main = "Hola mundo\x05\rA\x07B\x07\x07Caja \x08\x08\r";
    let headers = "Par\rImpar\r\r";
    let comments = "\x05Nota\r";
    let boxes = "Dentro\r";
    let text = [main, headers, comments, boxes].concat();
    let cp = |s: &str| main.find(s).unwrap() as u32;
    let (ccp, cch) = (main.len() as u32, headers.len() as u32);
    let mut lw = [0u32; 22];
    lw[3] = ccp;
    lw[5] = cch;
    lw[7] = comments.len() as u32;
    lw[9] = boxes.len() as u32;
    let mut fclcb = vec![(0u32, 0u32); 136];
    let mut table: Vec<u8> = Vec::new();
    let mut part = |i: usize, data: Vec<u8>| {
        fclcb[i] = (table.len() as u32, data.len() as u32);
        table.extend(data);
    };
    // The piece table: one 8-bit piece at byte 1536.
    let n = text.len() as u32;
    part(33, [&[2][..], &16u32.to_le_bytes(), &u32s(&[0, n]), &u16s(&[0]), &(3072u32 | 0x4000_0000).to_le_bytes(), &u16s(&[0])].concat());
    // Paragraph properties, on page 4.
    part(13, u32s(&[1536, 1536 + n, 4]));
    // Section: a title page.
    part(6, [&u32s(&[0, ccp])[..], &u16s(&[0]), &2560u32.to_le_bytes(), &u16s(&[0]), &u32s(&[u32::MAX])].concat());
    // Headers: six separators, then even and odd headers.
    part(11, u32s(&[0, 0, 0, 0, 0, 0, 0, 4, 10, 10, 10, 10, 10, 11]));
    // Facing pages.
    part(31, vec![1, 0, 0, 0]);
    // A comment by "Ana" on "mundo", tagged 7, dated 2026-09-20 10:30.
    let mut atrd = u16s(&[2, 'A' as u16, 'N' as u16]);
    atrd.resize(20, 0);
    atrd.extend(u16s(&[0, 0, 0]));
    atrd.extend(7i32.to_le_bytes());
    part(4, [u32s(&[cp("\x05"), ccp]), atrd].concat());
    part(5, u32s(&[0, comments.len() as u32]));
    part(36, [u16s(&[3]), utf16("Ana")].concat());
    part(37, [u16s(&[0xFFFF, 1, 10, 0, 0x100]), u32s(&[7, 0])].concat());
    part(42, [u32s(&[cp("mundo"), ccp]), u16s(&[0, 0])].concat());
    part(43, u32s(&[cp("\x05"), ccp]));
    let date = 30 | 10 << 6 | 20 << 11 | 9 << 16 | 126 << 20;
    part(113, [&u32s(&[date])[..], &[0; 14]].concat());
    // A bookmark on "Hola".
    part(21, [u16s(&[0xFFFF, 1, 0, 5]), utf16("Marca")].concat());
    part(22, [u32s(&[0, ccp]), u16s(&[0, 0])].concat());
    part(23, u32s(&[4, ccp]));
    // A text box (shape 1025) and a picture (1026) anchored at the 0x08s.
    let a = cp("\x08");
    part(40, [u32s(&[a, a + 1, ccp]), fspa(1025, [1440, 720, 4320, 2160], 0x72), fspa(1026, [0, 0, 1440, 1440], 0x54)].concat());
    let mut txbx = u32s(&[0, boxes.len() as u32]);
    txbx.extend([0; 14]);
    txbx.extend(1025u32.to_le_bytes());
    txbx.extend([0; 4]);
    part(56, txbx);
    let blip = art(0, 0x6E0, 0xF01E, &[&[0; 17][..], PNG].concat());
    let mut bse = vec![6, 6];
    bse.extend([0; 16]);
    bse.extend(u16s(&[0xFF]));
    bse.extend(u32s(&[blip.len() as u32, 1, 0]));
    bse.extend([0; 4]);
    bse.extend(blip);
    let dgg = art(0xF, 0, 0xF000, &art(0xF, 1, 0xF001, &art(2, 6, 0xF007, &bse)));
    let shapes = [
        art(0xF, 0, 0xF004, &art(2, 0, 0xF00A, &u32s(&[1024, 5]))),
        sp(202, 1025, &[(0x0181, 0x00CC_FFFF), (0x01C0, 0x0000_00FF), (0x0080, 0x10000)]),
        sp(75, 1026, &[(0x4104, 1)]),
    ]
    .concat();
    part(50, [dgg, vec![0], art(0xF, 0, 0xF002, &art(0xF, 0, 0xF003, &shapes))].concat());
    // The FIB.
    let mut wd = [u16s(&[0xA5EC, 0x00C1, 0, 0, 0, 0x0200]), vec![0; 20], u16s(&[14]), vec![0; 28], u16s(&[22])].concat();
    wd.extend(u32s(&lw));
    wd.extend(u16s(&[fclcb.len() as u16]));
    wd.extend(fclcb.iter().flat_map(|&(a, b)| u32s(&[a, b])));
    wd.resize(1536, 0);
    wd.extend(text.bytes());
    wd.resize(2048, 0);
    // The paragraph FKP: runs of file positions and their properties.
    let fc = |s: &str| 1536 + cp(s);
    let runs: [(u32, Vec<u8>); 5] = [
        // Tab stops (right, dots at 1"), a red top border and shading.
        (fc("A"), [&[0x0D, 0xC6, 5, 0, 1, 0xA0, 0x05, 0x0A][..], &[0x4E, 0xC6, 8, 0xFF, 0, 0, 0, 8, 1, 2, 0], &[0x4D, 0xC6, 10, 0, 0, 0, 0xFF, 0xFF, 0xEE, 0xCC, 0, 0, 0]].concat()),
        (fc("B"), vec![0x16, 0x24, 1]),
        (fc("\x07\x07") + 1, vec![0x16, 0x24, 1]),
        // The row: two cells, the first with a red top line and a
        // yellow background, all table borders thin.
        (fc("Caja"), {
            let mut tc = vec![0; 40];
            tc[4..8].copy_from_slice(&[8, 1, 6, 0]);
            let tdef = [&[2][..], &u16s(&[0, 1440, 2880]), &tc].concat();
            let brcs: Vec<u8> = (0..6).flat_map(|_| [0, 0, 0, 0, 4, 1, 0, 0]).collect();
            let shd = [&[0, 0, 0, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0][..], &[0, 0, 0, 0xFF, 0, 0, 0, 0xFF, 0xFF, 0xFF]].concat();
            [&[0x16, 0x24, 1, 0x17, 0x24, 1, 0x08, 0xD6][..], &((tdef.len() + 1) as u16).to_le_bytes(), &tdef, &[0x13, 0xD6, 48], &brcs, &[0x12, 0xD6, 20], &shd].concat()
        }),
        (1536 + n, Vec::new()),
    ];
    let mut fkp = vec![0u8; 512];
    fkp[..4].copy_from_slice(&1536u32.to_le_bytes());
    let mut off = 100;
    for (i, (end, sprms)) in runs.iter().enumerate() {
        fkp[(i + 1) * 4..(i + 2) * 4].copy_from_slice(&end.to_le_bytes());
        let g = [&[0, 0][..], sprms].concat();
        let words = g.len().div_ceil(2);
        fkp[24 + i * 13] = (off / 2) as u8;
        fkp[off] = 0;
        fkp[off + 1] = words as u8;
        fkp[off + 2..off + 2 + g.len()].copy_from_slice(&g);
        off += 2 + words * 2 + 2;
    }
    fkp[511] = runs.len() as u8;
    wd.extend(fkp);
    // The section's properties.
    wd.extend(u16s(&[3, 0x300A]));
    wd.push(1);
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("drawings.doc");
    std::fs::write(&path, ole(&[("WordDocument", &wd), ("1Table", &table)])).unwrap();
    let (loaded, _) = apex_io::open(&path).unwrap();
    let Loaded::Doc(doc) = loaded else { panic!("not a document") };

    // The comment and the bookmark.
    let p0 = &doc.paragraphs[0];
    assert_eq!(p0.text, "Hola mundo");
    assert_eq!(doc.comments.len(), 1);
    let c = &doc.comments[0];
    assert_eq!((c.id, c.author.as_str(), c.text.as_str(), c.date.as_str()), (1, "Ana", "Nota", "2026-09-20T10:30:00Z"));
    let span = doc.comment_spans()[0];
    assert_eq!((span.start.offset, span.end.offset), (5, 10));
    assert!(p0.marks.iter().any(|m| comment_id(&m.name) == Some(1)));
    let b = doc.bookmark("Marca").unwrap();
    assert_eq!((b.start.para, b.start.offset, b.end.offset), (0, 0, 4));
    // Tabs, border and shading.
    let st = &p0.style;
    assert_eq!(st.tabs.len(), 1);
    let tab = st.tabs.as_slice()[0];
    assert_eq!((tab.pos, tab.align, tab.leader), (72.0, TabAlign::Right, TabLeader::Dots));
    let top = st.borders.top.unwrap();
    assert_eq!((top.style, top.width, top.color, top.space), (LineStyle::Single, 1.0, 0xFF0000, 2.0));
    assert!(st.borders.bottom.is_none());
    assert_eq!(st.shading, Some(0xFFEECC));
    // The table: its borders, a cell's own line and background.
    let a1 = doc.paragraphs.iter().find(|p| p.text == "A").unwrap();
    let ic = a1.cell.as_ref().unwrap();
    let tp = ic.table_props.as_ref().unwrap();
    assert_eq!(tp.style, None);
    assert_eq!(tp.borders.inside_v, Some(Border::new(0.5, 0)));
    let cell = ic.cell.as_ref().unwrap();
    assert_eq!(cell.borders.top, Some(Border::new(1.0, 0xFF0000)));
    assert_eq!(cell.shading, Some(0xFFFF00));
    let b1 = doc.paragraphs.iter().find(|p| p.text == "B").unwrap();
    assert_eq!(b1.cell.as_ref().unwrap().cell.as_ref().unwrap().shading, None);
    // Headers for odd and even pages, and a first page of its own.
    assert!(doc.odd_even && doc.headers.different_first);
    assert_eq!(doc.headers.get(HfSlot::EvenHeader).unwrap()[0].text, "Par");
    assert_eq!(doc.headers.get(HfSlot::Header).unwrap()[0].text, "Impar");
    // The text box and the picture the text wraps around.
    let last = doc.paragraphs.last().unwrap();
    assert!(last.text.starts_with("Caja "));
    let objs: Vec<_> = last.runs.iter().map(|r| &r.attrs).filter(|a| a.shape.is_some() || a.image.is_some()).collect();
    assert_eq!(objs.len(), 2);
    let s = objs[0].shape.as_ref().unwrap();
    assert_eq!(s.shape.kind, ShapeKind::TextBox);
    assert_eq!((s.x, s.y, s.width, s.height), (72.0, 36.0, 144.0, 72.0));
    assert_eq!((s.wrap, s.h_rel, s.v_rel), (ShapeWrap::InFront, apex_doc::HRel::Page, VRel::Paragraph));
    assert_eq!((s.shape.fill, s.shape.line), (Some(0xFFFFCC), Some(0xFF0000)));
    assert_eq!(s.shape.plain_text(), "Dentro");
    let img = objs[1].image.as_ref().unwrap();
    assert_eq!(&img.data[..], PNG);
    assert_eq!((img.width, img.height, img.wrap), (72.0, 72.0, Wrap::Square));
}
