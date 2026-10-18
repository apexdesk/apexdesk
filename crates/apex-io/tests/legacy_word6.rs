//! Word 6.0/95 (.doc) files built here structure by structure: text in
//! the code page of each font, character and paragraph formatting, a
//! heading, a table, a footnote, a picture, the page setup, the header and
//! footer, pieces of text out of order and 16-bit text.

use std::io::{Cursor, Write};

use apex_doc::{Align, HfSlot, LineRule, OBJECT, ParaKind};

fn ole(wd: &[u8]) -> Vec<u8> {
    let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    c.create_stream("WordDocument").unwrap().write_all(wd).unwrap();
    c.flush().unwrap();
    c.into_inner().into_inner()
}

fn u16s(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// A WordDocument stream: the FIB, the text at 1024, formatting pages 4
/// (characters) and 5 (paragraphs), then the structures.
struct Word6 {
    wd: Vec<u8>,
}

impl Word6 {
    fn new(flags: u16, text: &[u8], ccp: [u32; 3]) -> Self {
        let mut wd = vec![0u8; 3072];
        wd[..12].copy_from_slice(&u16s(&[0xA5DC, 0x65, 0, 0x409, 0, flags]));
        wd[0x18..0x20].copy_from_slice(&u32s(&[1024, 1024 + text.len() as u32]));
        wd[0x34..0x40].copy_from_slice(&u32s(&ccp));
        wd[1024..1024 + text.len()].copy_from_slice(text);
        Word6 { wd }
    }

    /// Adds structure `i` of the FIB's list.
    fn part(&mut self, i: usize, data: &[u8]) -> u32 {
        let at = self.wd.len() as u32;
        self.wd[0x58 + i * 8..0x60 + i * 8].copy_from_slice(&u32s(&[at, data.len() as u32]));
        self.wd.extend_from_slice(data);
        at
    }

    /// Adds data at the end, returning where.
    fn raw(&mut self, data: &[u8]) -> u32 {
        self.wd.extend_from_slice(data);
        (self.wd.len() - data.len()) as u32
    }

    /// A formatting page: runs of file positions with their properties.
    fn fkp(&mut self, page: usize, runs: &[(u32, u32, Vec<u8>)], para: bool) {
        let mut p = vec![0u8; 512];
        let n = runs.len();
        for (i, r) in runs.iter().enumerate() {
            p[i * 4..i * 4 + 4].copy_from_slice(&r.0.to_le_bytes());
        }
        p[n * 4..n * 4 + 4].copy_from_slice(&runs[n - 1].1.to_le_bytes());
        let base = 4 * (n + 1);
        let mut off = 510;
        for (i, r) in runs.iter().enumerate() {
            let g = if para { [&[0u8][..], &u16s(&[r.2[0].into()]), &r.2[1..]].concat() } else { [&[r.2.len() as u8][..], &r.2].concat() };
            if para {
                let cw = (g.len() - 1).div_ceil(2);
                off = (off - (1 + cw * 2)) & !1;
                p[off] = cw as u8;
                p[off + 1..off + g.len()].copy_from_slice(&g[1..]);
                p[base + 7 * i] = (off / 2) as u8;
            } else if !r.2.is_empty() {
                off = (off - g.len()) & !1;
                p[off..off + g.len()].copy_from_slice(&g);
                p[base + i] = (off / 2) as u8;
            }
        }
        p[511] = n as u8;
        self.wd[page * 512..page * 512 + 512].copy_from_slice(&p);
    }
}

/// A font: character set and name.
fn ffn(chs: u8, name: &str) -> Vec<u8> {
    let body = [&[0x16, 0x90, 0x01, chs, 0][..], name.as_bytes(), &[0]].concat();
    [&[body.len() as u8][..], &body].concat()
}

/// A paragraph style: name, identifier, base and properties.
fn style(name: &str, sti: u16, base: u16, papx: &[u8], chpx: &[u8]) -> Vec<u8> {
    let mut std = u16s(&[sti, 1 | base << 4, 2, 0]);
    std.push(name.len() as u8);
    std.extend(name.bytes());
    std.push(0);
    if std.len() % 2 == 1 {
        std.push(0);
    }
    std.extend(u16s(&[papx.len() as u16 + 2, 0]));
    std.extend(papx);
    if std.len() % 2 == 1 {
        std.push(0);
    }
    std.extend(u16s(&[chpx.len() as u16]));
    std.extend(chpx);
    [u16s(&[std.len() as u16]), std].concat()
}

#[test]
fn reads_word_6_documents() {
    // The stories: main text, a footnote, then header and footer.
    let mut main: Vec<u8> = b"T\xEDtulo\rHola negrita ".to_vec();
    main.extend(b"\xCF\xF0\xE8\xE2\xE5\xF2 \x02\rA\x07B\x07\x07\x01\rPunto\r");
    let ftn = b"\x02 Nota al pie\r";
    let hdd = b"Cabecera\rPie\r\r";
    let text = [&main[..], ftn, hdd].concat();
    let ccp = [main.len() as u32, ftn.len() as u32, hdd.len() as u32];
    let mut w = Word6::new(0, &text, ccp);
    let fc = |s: &[u8]| 1024 + main.windows(s.len()).position(|x| x == s).unwrap() as u32;
    let end = 1024 + text.len() as u32;

    // A picture: a metafile drawing a 1x1 bitmap, 1" x 0.5".
    let dib = [&u32s(&[40, 1, 1])[..], &u16s(&[1, 24]), &[0; 24], &[0, 0, 255, 0]].concat();
    let stretch = [&u32s(&[(6 + 22 + dib.len() as u32) / 2])[..], &u16s(&[0x0F43]), &[0; 22], &dib].concat();
    let wmf = [&u16s(&[1, 9, 0x300])[..], &[0; 12], &stretch, &u32s(&[3]), &u16s(&[0])].concat();
    let mut pic = u32s(&[58 + wmf.len() as u32]);
    pic.extend(u16s(&[58, 8, 1440, 720, 0]));
    pic.resize(0x1C, 0);
    pic.extend(u16s(&[1440, 720, 1000, 1000]));
    pic.resize(58, 0);
    pic.extend(wmf);
    let pic_at = w.raw(&pic);

    // Character runs: bold, italic, underlined, 14 pt and red; a Cyrillic
    // font; the note reference and the picture.
    let chp = vec![
        (1024, fc(b"negrita"), vec![]),
        (fc(b"negrita"), fc(b"negrita") + 7, vec![85, 1, 86, 1, 94, 1, 99, 28, 0, 98, 6]),
        (fc(b"negrita") + 7, fc(b"\xCF"), vec![]),
        (fc(b"\xCF"), fc(b"\xCF") + 6, vec![93, 1, 0]),
        (fc(b"\xCF") + 6, fc(b"\x02"), vec![]),
        (fc(b"\x02"), fc(b"\x02") + 1, vec![117, 1]),
        (fc(b"\x02") + 1, fc(b"\x01"), vec![]),
        (fc(b"\x01"), fc(b"\x01") + 1, [&[117, 1, 68, 4][..], &pic_at.to_le_bytes()].concat()),
        (fc(b"\x01") + 1, end, vec![]),
    ];
    w.fkp(4, &chp, false);
    // Paragraphs: a heading; centered with indent and spacing; a row of
    // two cells with a bordered first cell; the picture; a bullet.
    let tc = |brc: u16| [&u16s(&[0])[..], &u16s(&[brc, 0, 0, 0])].concat();
    let tdef = [&[2][..], &u16s(&[0, 1440, 2880]), &tc(0x0049), &tc(0)].concat();
    let row = [&[24, 1, 25, 1, 190][..], &u16s(&[tdef.len() as u16 + 1]), &tdef].concat();
    let pap = vec![
        (1024, fc(b"Hola"), vec![1]),
        (fc(b"Hola"), fc(b"A\x07"), vec![0, 5, 1, 17, 0xD0, 0x02, 21, 240, 0, 20, 0x68, 0x01, 1, 0]),
        (fc(b"A\x07"), fc(b"B\x07"), vec![0, 24, 1]),
        (fc(b"B\x07"), fc(b"\x07\x01"), vec![0, 24, 1]),
        (fc(b"\x07\x01"), fc(b"\x01"), [&[0][..], &row].concat()),
        (fc(b"\x01"), fc(b"Punto"), vec![0]),
        // An old style bulleted paragraph.
        (fc(b"Punto"), fc(b"Punto") + 6, vec![0, 13, 11, 12, 3, 0xFF, 1, 0]),
        (fc(b"Punto") + 6, end, vec![0]),
    ];
    w.fkp(5, &pap, true);
    w.part(12, &[&u32s(&[1024, end])[..], &u16s(&[4])].concat());
    w.part(13, &[&u32s(&[1024, end])[..], &u16s(&[5])].concat());
    w.wd[0x18A..0x192].copy_from_slice(&u16s(&[4, 5, 1, 1]));
    // Styles: Normal and Heading 1 (bold 16 pt), fonts, the footnote.
    let mut stsh = u16s(&[14, 2, 8, 1, 0, 0, 0, 0]);
    stsh.extend(style("Normal", 0, 0xFFF, &[], &[99, 24, 0]));
    stsh.extend(style("heading 1", 1, 0, &[22, 120, 0], &[85, 1, 99, 32, 0]));
    w.part(1, &stsh);
    let fonts = [ffn(0, "Times New Roman"), ffn(204, "Arial Cyr")].concat();
    w.part(15, &[&u16s(&[fonts.len() as u16 + 2])[..], &fonts].concat());
    let r = fc(b"\x02") - 1024;
    w.part(2, &[&u32s(&[r, ccp[0]])[..], &u16s(&[1])].concat());
    w.part(3, &u32s(&[0, ftn.len() as u32]));
    // A4 with 2 cm side margins, an odd page header and footer.
    let sepx = [&u16s(&[20])[..], &[164], &u16s(&[11906]), &[165], &u16s(&[16838]), &[166], &u16s(&[1134]), &[167], &u16s(&[1134]), &[168], &u16s(&[1440]), &[169], &u16s(&[1440]), &[153, 0x0A]].concat();
    let sepx_at = w.raw(&sepx);
    w.part(6, &[&u32s(&[0, ccp[0]])[..], &u16s(&[0]), &sepx_at.to_le_bytes(), &u16s(&[0]), &u32s(&[0])].concat());
    w.part(11, &u32s(&[0, 9, 13, 14]));
    w.part(31, &[0, 0, 0, 0]);

    let doc = apex_io::doc::read(&ole(&w.wd)).unwrap();
    let p = &doc.paragraphs;
    assert_eq!(p[0].text, "Título");
    assert_eq!(p[0].style.kind, ParaKind::Heading(1));
    // Normal has no font: the style sheet's standard one (not ours).
    assert_eq!(doc.default_font.as_deref(), Some("Times New Roman"));
    // Text in two code pages, a footnote.
    assert_eq!(p[1].text, format!("Hola negrita Привет {OBJECT}"));
    let st = &p[1].style;
    assert_eq!((st.align, st.indent_left, st.space_before), (Align::Center, 36.0, Some(12.0)));
    assert_eq!((st.line_spacing, st.line_rule), (1.5, LineRule::Multiple));
    let mut runs = Vec::new();
    let mut i = 0;
    for r in &p[1].runs {
        runs.push((&p[1].text[i..i + r.len], &r.attrs));
        i += r.len;
    }
    let (_, a) = runs.iter().find(|r| r.0 == "negrita").unwrap();
    assert!(a.bold && a.italic && a.underline);
    assert_eq!((a.size, a.color), (Some(28), Some(0xFF0000)));
    let (_, a) = runs.iter().find(|r| r.0 == "Привет").unwrap();
    assert_eq!(a.font.as_deref(), Some("Arial Cyr"));
    let note = runs.iter().find_map(|r| r.1.note.clone()).unwrap();
    assert_eq!(note.text(), "Nota al pie");
    // The table.
    let a1 = p.iter().find(|q| q.text == "A").unwrap();
    let cell = a1.cell.as_ref().unwrap();
    assert_eq!((cell.row, cell.col), (0, 0));
    assert_eq!(cell.cell.as_ref().unwrap().borders.top, Some(apex_doc::Border::new(0.75, 0)));
    assert_eq!(p.iter().find(|q| q.text == "B").unwrap().cell.as_ref().unwrap().col, 1);
    // The picture's bitmap.
    let img = p.iter().find_map(|q| q.runs.iter().find_map(|r| r.attrs.image.clone())).unwrap();
    assert_eq!(&img.data[..2], b"BM");
    assert_eq!((img.width, img.height), (72.0, 36.0));
    assert_eq!(p.iter().find(|q| q.text == "Punto").unwrap().style.kind, ParaKind::Bullet);
    // Page setup, header and footer.
    assert_eq!((doc.page.width, doc.page.margin_left, doc.page.margin_top), (595.3, 56.7, 72.0));
    assert_eq!(doc.headers.get(HfSlot::Header).unwrap()[0].text, "Cabecera");
    assert_eq!(doc.headers.get(HfSlot::Footer).unwrap()[0].text, "Pie");
    assert!(doc.headers.get(HfSlot::EvenHeader).is_none());
}

#[test]
fn reads_word_95_pieces_and_far_east_text() {
    // Two pieces in the reverse order of the text: Big5 Chinese, then
    // Latin, in a Chinese font.
    let text = b"\xA4\xA4\xA4\xE5\rHola\r";
    let mut w = Word6::new(0x0004, text, [10, 0, 0]);
    w.wd[2] = 0x68;
    let pcd = |fc: u32| [&u16s(&[0])[..], &fc.to_le_bytes(), &u16s(&[0])].concat();
    let plc = [u32s(&[0, 5, 10]), pcd(1029), pcd(1024)].concat();
    w.part(33, &[&[2][..], &(plc.len() as u32).to_le_bytes(), &plc].concat());
    let fonts = ffn(136, "MingLiU");
    w.part(15, &[&u16s(&[fonts.len() as u16 + 2])[..], &fonts].concat());
    let doc = apex_io::doc::read(&ole(&w.wd)).unwrap();
    let texts: Vec<&str> = doc.paragraphs.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, ["Hola", "中文"]);

    // 16-bit text: each unit one or two bytes of the font's code page.
    let text = u16s(&['H' as u16, 'i' as u16, 0x1092, 0xE9, 13]);
    let w = Word6::new(0x1000, &text, [5, 0, 0]);
    let doc = apex_io::doc::read(&ole(&w.wd)).unwrap();
    assert_eq!(doc.paragraphs[0].text, "Hi’é");
}

#[test]
fn broken_word_2_documents_fail_clearly() {
    // Word 2.0 files are read (see doc/ww2.rs); one with no text gives a
    // clear error.
    let mut data = u16s(&[0xA5DB, 0x2D]);
    data.resize(1024, 0);
    let e = apex_io::doc::read(&data).unwrap_err();
    assert!(e.0.contains("too old"), "{}", e.0);
}
