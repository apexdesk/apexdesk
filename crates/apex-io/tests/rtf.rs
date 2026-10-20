//! Rich Text Format.

use std::sync::Arc;

use apex_doc::{Align, Attrs, Document, HfSlot, Image, Mark, Note, NoteKind, OBJECT, ParaKind, ParaStyle, Paragraph, Table};

/// A 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn styled(parts: &[(&str, Attrs)]) -> Paragraph {
    let mut p = Paragraph::new("");
    for (t, a) in parts {
        p.push(t, a.clone());
    }
    p
}

fn sample() -> Document {
    let bold = Attrs { bold: true, ..Default::default() };
    let red = Attrs { color: Some(0xC00000), italic: true, size: Some(28), font: Some(Arc::from("Georgia")), ..Default::default() };
    let link = Attrs { link: Some(Arc::from("https://example.com/a?b=1")), underline: true, ..Default::default() };
    let note = Attrs { note: Some(Arc::new(Note::from_text(NoteKind::Foot, "La nota"))), ..Default::default() };
    let img = Attrs { image: Some(Arc::new(Image::new(Arc::from(PNG), 30.0, 20.0))), ..Default::default() };
    let mut paras = vec![
        Paragraph::new("Título grande").with_style(ParaStyle { kind: ParaKind::Heading(1), ..Default::default() }),
        styled(&[("Normal ", Attrs::default()), ("negrita", bold), (" y ", Attrs::default()), ("roja €ñ😀", red)]),
        Paragraph::new("Centrado {llaves} \\barra\ttab").with_style(ParaStyle { align: Align::Center, space_after: Some(20.0), ..Default::default() }),
        Paragraph::new("uno").with_style(ParaStyle { kind: ParaKind::Bullet, ..Default::default() }),
        Paragraph::new("dos").with_style(ParaStyle { kind: ParaKind::Bullet, level: 1, ..Default::default() }),
        Paragraph::new("primero").with_style(ParaStyle { kind: ParaKind::Numbered, list_format: 1, ..Default::default() }),
        Paragraph::new("segundo").with_style(ParaStyle { kind: ParaKind::Numbered, list_format: 1, ..Default::default() }),
        Paragraph::new("otra lista").with_style(ParaStyle { kind: ParaKind::Numbered, list_format: 1, restart: true, ..Default::default() }),
        styled(&[("Ver ", Attrs::default()), ("enlace", link), (".", Attrs::default()), (&OBJECT.to_string(), note), (" foto ", Attrs::default()), (&OBJECT.to_string(), img)]),
    ];
    let mut t = Table::new(2, 3, 300.0);
    t.rows[0][0].paras = vec![Paragraph::new("A1")];
    t.rows[0][1].paras = vec![Paragraph::new("B1+C1")];
    t.rows[0][1].props.col_span = 2;
    t.rows[0][2].props.covered = true;
    t.rows[1][0].paras = vec![Paragraph::new("A2")];
    t.rows[1][1].paras = vec![Paragraph::new("B2")];
    t.rows[1][2].paras = vec![Paragraph::new("C2"), Paragraph::new("C2 bis")];
    paras.extend(t.into_paras(1));
    let mut last = Paragraph::new("marca aquí");
    last.marks = vec![Mark { name: Arc::from("sitio"), offset: 6, end: false }, Mark { name: Arc::from("sitio"), offset: 11, end: true }];
    paras.push(last);
    let mut d = Document::from_paragraphs(paras);
    d.meta.title = "Prueba RTF".into();
    d.meta.author = "Alguien".into();
    d.page.width = 612.0;
    d.page.height = 792.0;
    d.headers.set(HfSlot::Header, Some(Arc::new(vec![Paragraph::new("Encabezado")])));
    d
}

#[test]
fn rtf_round_trip() {
    let d = sample();
    let bytes = apex_io::rtf::write(&d);
    let back = apex_io::rtf::read(&bytes).unwrap();
    let texts = |d: &Document| d.paragraphs.iter().map(|p| p.text.clone()).collect::<Vec<_>>();
    assert_eq!(texts(&back), texts(&d));
    for (a, b) in d.paragraphs.iter().zip(&back.paragraphs) {
        assert_eq!(a.style.kind, b.style.kind, "{}", a.text);
        assert_eq!(a.style.level, b.style.level, "{}", a.text);
        assert_eq!(a.style.align, b.style.align, "{}", a.text);
        assert_eq!(a.style.restart, b.style.restart, "{}", a.text);
        assert_eq!(a.style.space_after, b.style.space_after, "{}", a.text);
        assert_eq!(a.cell.is_some(), b.cell.is_some(), "{}", a.text);
    }
    // Character formatting.
    let p = &back.paragraphs[1];
    assert_eq!(p.runs.len(), 4);
    assert!(p.runs[1].attrs.bold);
    let r = &p.runs[3].attrs;
    assert_eq!((r.color, r.italic, r.size, r.font.as_deref()), (Some(0xC00000), true, Some(28), Some("Georgia")));
    assert_eq!(back.paragraphs[0].runs[0].attrs, Attrs::default());
    // Link, note and picture.
    let p = &back.paragraphs[8];
    let link = p.runs.iter().find(|r| r.attrs.link.is_some()).unwrap();
    assert_eq!(link.attrs.link.as_deref(), Some("https://example.com/a?b=1"));
    let note = p.runs.iter().find_map(|r| r.attrs.note.clone()).unwrap();
    assert_eq!(note.text(), "La nota");
    let img = p.runs.iter().find_map(|r| r.attrs.image.clone()).unwrap();
    assert_eq!(&*img.data, PNG);
    assert_eq!((img.width, img.height), (30.0, 20.0));
    // Table with a merged cell.
    let t = &back.tables()[0];
    assert_eq!((t.rows, t.cols), (2, 3));
    assert_eq!(t.span_of(0, 1), (1, 2));
    // Bookmark, header, properties and page.
    let last = back.paragraphs.last().unwrap();
    assert_eq!(last.marks.iter().map(|m| (m.offset, m.end)).collect::<Vec<_>>(), vec![(6, false), (11, true)]);
    assert_eq!(back.headers.get(HfSlot::Header).unwrap()[0].text, "Encabezado");
    assert_eq!((back.meta.title.as_str(), back.meta.author.as_str()), ("Prueba RTF", "Alguien"));
    assert_eq!((back.page.width, back.page.height), (612.0, 792.0));
}

#[test]
fn rtf_through_open_and_save() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("rtf");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.rtf");
    apex_io::save_doc(&sample(), &path, apex_io::Format::Rtf).unwrap();
    let (loaded, fmt) = apex_io::open(&path).unwrap();
    assert_eq!(fmt, apex_io::Format::Rtf);
    assert!(matches!(loaded, apex_io::Loaded::Doc(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reads_word_and_wordpad_rtf() {
    let word = br#"{\rtf1\adeflang1025\ansi\ansicpg1252\uc1\deff31507\deflang3082{\fonttbl{\f0\fbidi \froman\fcharset0\fprq2{\*\panose 02020603050405020304}Times New Roman;}{\f31507\fbidi \fswiss\fcharset0\fprq2{\*\panose 020f0502020204030204}Calibri;}}
{\colortbl;\red0\green0\blue0;\red255\green0\blue0;}{\stylesheet{\ql \li0\ri0\sa160\sl259\slmult1 \fs22 \snext0 Normal;}{\s1\ql \sb240\outlinelevel0 \fs32 \sbasedon0 \snext0 heading 1;}}
{\*\listtable{\list\listtemplateid1{\listlevel\levelnfc23\levelnfcn23\leveljc0\levelfollow0\levelstartat1{\leveltext\leveltemplateid2\'01\u-3913 ?;}{\levelnumbers;}\f3\fi-360\li720}\listid5}}{\*\listoverridetable{\listoverride\listid5\listoverridecount0\ls1}}
{\info{\title Informe}{\author Ana}{\creatim\yr2024\mo1\dy2}}\paperw11906\paperh16838\margl1701\margr1701
\pard\plain \s1\ql\sb240\outlinelevel0 \fs32 {\rtlch \ltrch\loch Cap\'edtulo}\par
\pard\plain \ql\sa160 \fs22 Hola {\b negrita} y {\cf2 rojo} \u8364\'80 fin.\par
{\listtext\pard\plain \f3 \'b7\tab}\pard\plain \ls1\ilvl0\fi-360\li720 \fs22 punto\par
\trowd\cellx2000\cellx4000\pard\intbl a\cell b\cell\row
\pard x{\field{\*\fldinst {HYPERLINK "http://a.b/"}}{\fldrslt {web}}} {\field{\*\fldinst { PAGE }}{\fldrslt {1}}}\par}"#;
    let d = apex_io::rtf::read(word).unwrap();
    let t: Vec<&str> = d.paragraphs.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(t[0], "Capítulo");
    assert_eq!(d.paragraphs[0].style.kind, ParaKind::Heading(1));
    assert_eq!(t[1], "Hola negrita y rojo € fin.");
    assert!(d.paragraphs[1].runs.iter().any(|r| r.attrs.bold));
    assert!(d.paragraphs[1].runs.iter().any(|r| r.attrs.color == Some(0xFF0000)));
    assert_eq!((t[2], d.paragraphs[2].style.kind), ("punto", ParaKind::Bullet));
    assert_eq!(d.tables().len(), 1);
    assert_eq!(&t[3..5], &["a", "b"]);
    let p = d.paragraphs.last().unwrap();
    assert!(p.runs.iter().any(|r| r.attrs.link.as_deref() == Some("http://a.b/")));
    assert!(p.runs.iter().any(|r| r.attrs.field.is_some()));
    assert_eq!(d.meta.title, "Informe");
    assert_eq!(d.default_font.as_deref(), Some("Calibri"));

    let wordpad = br"{\rtf1\ansi\deff0{\fonttbl{\f0\fnil\fcharset0 Calibri;}{\f1\fnil\fcharset2 Symbol;}}
\viewkind4\uc1\pard\sa200\sl276\slmult1\f0\fs22\lang10 Primera l\'ednea\par
{\pntext\f1\'b7\tab}{\*\pn\pnlvlblt\pnf1\pnindent0{\pntxtb\'b7}}\fi-360\li720 elemento\par
\pard\qc centrado\par}";
    let d = apex_io::rtf::read(wordpad).unwrap();
    let t: Vec<&str> = d.paragraphs.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(t, ["Primera línea", "elemento", "centrado"]);
    assert_eq!(d.paragraphs[1].style.kind, ParaKind::Bullet);
    assert_eq!(d.paragraphs[2].style.align, Align::Center);
    assert!(apex_io::rtf::read(b"hello").is_err());
}

#[test]
fn tracked_changes_stay_tracked() {
    let src = br"{\rtf1\ansi Price {\deleted 100}{\revised 200} euros.\par}";
    let d = apex_io::rtf::read(src).unwrap();
    // Deleted text isn't taken for text that stays: the change shows as one.
    assert_eq!(d.changes().len(), 2);
    assert_eq!(d.resolved(true).paragraphs[0].text, "Price 200 euros.");
    let again = apex_io::rtf::read(&apex_io::rtf::write(&d)).unwrap();
    assert_eq!(again.changes().len(), 2);
    assert_eq!(again.resolved(true).paragraphs[0].text, "Price 200 euros.");
}

/// A WMF without the placeable header, as RTF holds them: a window
/// 2000×1000 units, then the end.
fn wmf() -> Vec<u8> {
    let words: [u16; 22] = [1, 9, 0x300, 22, 0, 0, 5, 0, 0, 5, 0, 0x020B, 0, 0, 5, 0, 0x020C, 1000, 2000, 3, 0, 0];
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn pictures(d: &Document) -> Vec<Arc<Image>> {
    d.paragraphs.iter().flat_map(|p| p.runs.iter().filter_map(|r| r.attrs.image.clone())).collect()
}

#[test]
fn pictures_word_puts_on_the_clipboard() {
    // Word 365's inline picture: PNG for readers of shapes, a WMF for
    // the rest.
    let rtf = format!(
        r"{{\rtf1\ansi\ansicpg1252\deff0{{\fonttbl{{\f0\froman Times New Roman;}}}}\pard\plain\f0\fs24 Before {{\*\shppict{{\pict{{\*\picprop\shplid1025{{\sp{{\sn shapeType}}{{\sv 75}}}}{{\sp{{\sn fLayoutInCell}}{{\sv 1}}}}}}\picscalex100\picscaley100\piccropl0\piccropr0\piccropt0\piccropb0\picw26\pich26\picwgoal300\pichgoal300\pngblip\bliptag255{{\*\blipuid 0123456789abcdef0123456789abcdef}}{png}}}}}{{\nonshppict{{\pict\picscalex100\picscaley100\piccropl0\picw26\pich26\picwgoal300\pichgoal300\wmetafile8\bliptag255\blipupw15\blipuph15 {wmf}}}}} after\par}}",
        png = hex(PNG),
        wmf = hex(&wmf())
    );
    let d = apex_io::rtf::read(rtf.as_bytes()).unwrap();
    assert_eq!(d.paragraphs[0].text, format!("Before {OBJECT} after"));
    let pics = pictures(&d);
    assert_eq!(pics.len(), 1);
    assert_eq!((&pics[0].data[..], pics[0].width), (PNG, 15.0));

    // An embedded object: its picture, not its data.
    let rtf = format!(
        r"{{\rtf1\ansi{{\fonttbl{{\f0 Calibri;}}}}\pard\f0 {{\object\objemb\objw2000\objh1000{{\*\objclass Excel.Sheet.12}}{{\*\objdata 01050000020000000b000000457863656c2e53686565742e313200}}{{\result{{\pict\picscalex100\picscaley100\picw3528\pich1764\picwgoal2000\pichgoal1000\wmetafile8 {wmf}}}}}}}\par}}",
        wmf = hex(&wmf())
    );
    let d = apex_io::rtf::read(rtf.as_bytes()).unwrap();
    assert_eq!(d.paragraphs[0].text, OBJECT.to_string());
    let pics = pictures(&d);
    assert_eq!(pics.len(), 1);
    assert_eq!((pics[0].width, pics[0].height), (100.0, 50.0));
    // With the header that gives a metafile its size.
    assert!(pics[0].data.starts_with(&[0xD7, 0xCD, 0xC6, 0x9A]));
    let (w, h) = apex_io::metafile::size(&pics[0].data).unwrap();
    assert!((w - 100.0).abs() < 0.01 && (h - 50.0).abs() < 0.01, "{w}×{h}");

    // EMF, sized by \picw (hundredths of a millimetre) without a goal.
    let rtf = r"{\rtf1\ansi{\pict\emfblip\picw2540\pich1270 0100000064000000}\par}";
    let d = apex_io::rtf::read(rtf.as_bytes()).unwrap();
    let pics = pictures(&d);
    assert_eq!(pics.len(), 1);
    assert_eq!((&pics[0].data[..], pics[0].width, pics[0].height), (&[1, 0, 0, 0, 0x64, 0, 0, 0][..], 72.0, 36.0));
}
