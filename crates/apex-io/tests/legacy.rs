//! Binary Office files, read only: Excel 97-2003 and 5.0/95 (.xls) and
//! Word 97-2003 (.doc), built here record by record.

use std::io::{Cursor, Write};
use std::path::PathBuf;

use apex_io::{Format, Loaded};
use apex_sheet::{CellRef, Value, Workbook};

fn ole(streams: &[(&str, &[u8])]) -> Vec<u8> {
    let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    for (name, data) in streams {
        c.create_stream(name).unwrap().write_all(data).unwrap();
    }
    c.flush().unwrap();
    c.into_inner().into_inner()
}

fn write(name: &str, data: &[u8]) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, data).unwrap();
    path
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn rec(ty: u16, data: &[u8]) -> Vec<u8> {
    cat(&[&ty.to_le_bytes(), &(data.len() as u16).to_le_bytes(), data])
}

fn u16s(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn open_book(name: &str, data: &[u8]) -> (Workbook, Format) {
    let (loaded, fmt) = apex_io::open(&write(name, data)).unwrap();
    let Loaded::Sheet(wb) = loaded else { panic!("not a workbook") };
    (*wb, fmt)
}

fn formula(wb: &Workbook, sheet: usize, cell: &str) -> String {
    let c = wb.sheets[sheet].get(CellRef::parse(cell).unwrap()).unwrap();
    c.formula().unwrap().expr().to_canonical()
}

fn value(wb: &Workbook, sheet: usize, cell: &str) -> Value {
    wb.sheets[sheet].get(CellRef::parse(cell).unwrap()).map(|c| c.value().clone()).unwrap_or(Value::Empty)
}

/// Joins the globals and sheets, pointing each `BOUNDSHEET` (built by
/// `sheet(name, pos)`) at its sheet.
fn book(head: &[Vec<u8>], names: &[&str], tail: &[Vec<u8>], sheets: &[Vec<u8>], sheet: impl Fn(&str, u32) -> Vec<u8>) -> Vec<u8> {
    let bound_len: usize = names.iter().map(|n| sheet(n, 0).len()).sum();
    let head: Vec<u8> = head.concat();
    let tail: Vec<u8> = tail.concat();
    let mut pos = (head.len() + bound_len + tail.len()) as u32;
    let mut bound = Vec::new();
    for (n, s) in names.iter().zip(sheets) {
        bound.extend(sheet(n, pos));
        pos += s.len() as u32;
    }
    cat(&[&head, &bound, &tail, &sheets.concat()])
}

#[test]
fn reads_excel_97_workbooks() {
    let short = |s: &str| cat(&[&[s.len() as u8, 0], s.as_bytes()]);
    let font = |weight: u16| rec(0x0031, &cat(&[&u16s(&[200, 0, 0x7FFF, weight, 0]), &[0, 0, 0, 0], &short("Arial")]));
    // XF: font, format, protection, alignment, rotation, indent, used,
    // borders (thin bottom), fill (solid yellow).
    let xf = |font: u16, fmt: u16, fill: bool| {
        let b1: u32 = 1 << 12;
        let b2: u32 = if fill { 1 << 26 } else { 0 };
        rec(0x00E0, &cat(&[&u16s(&[font, fmt, 1]), &[0x20, 0, 0, 0], &b1.to_le_bytes(), &b2.to_le_bytes(), &u16s(&[if fill { 13 } else { 64 }])]))
    };
    let head = vec![
        rec(0x0809, &cat(&[&u16s(&[0x0600, 0x0005]), &[0; 12]])),
        font(400),
        font(700),
        rec(0x041E, &cat(&[&u16s(&[164, 5]), &[0], b"0.00%"])),
        xf(0, 0, false),
        xf(1, 164, true),
    ];
    let sst = cat(&[&2u32.to_le_bytes(), &2u32.to_le_bytes(), &u16s(&[4]), &[0], b"Hola", &u16s(&[3]), &[1], &u16s(&[0x41, 0xF1, 0x6F])]);
    let tail = vec![
        rec(0x00FC, &sst),
        rec(0x01AE, &u16s(&[2, 0x0401])),
        rec(0x0017, &u16s(&[1, 0, 1, 1])),
        // "Tasa" = 'Hoja 2'!$A$1.
        rec(0x0018, &cat(&[&[0, 0, 0, 4], &u16s(&[7, 0, 0]), &[0, 0, 0, 0, 0], b"Tasa", &[0x3A], &u16s(&[0, 0, 0])])),
        rec(0x000A, &[]),
    ];
    let bof = rec(0x0809, &cat(&[&u16s(&[0x0600, 0x0010]), &[0; 12]]));
    // A1*2+Tasa, then SUM('Hoja 2'!A1:B2).
    let f1 = cat(&[&[0x44], &u16s(&[0, 0xC000]), &[0x1E], &u16s(&[2]), &[0x05, 0x43], &1u32.to_le_bytes(), &[0x03]]);
    let f2 = cat(&[&[0x3B], &u16s(&[0, 0, 1, 0xC000, 0xC001]), &[0x22, 1], &u16s(&[4])]);
    let fml = |col: u16, v: f64, rgce: &[u8]| rec(0x0006, &cat(&[&u16s(&[0, col, 0]), &v.to_le_bytes(), &[0; 6], &u16s(&[rgce.len() as u16]), rgce]));
    let s1 = [
        bof.clone(),
        rec(0x0203, &cat(&[&u16s(&[0, 0, 1]), &2.5f64.to_le_bytes()])),
        rec(0x00FD, &cat(&[&u16s(&[0, 1, 0]), &1u32.to_le_bytes()])),
        fml(2, 8.0, &f1),
        fml(3, 3.0, &f2),
        rec(0x00E5, &u16s(&[1, 1, 3, 0, 2])),
        rec(0x000A, &[]),
    ]
    .concat();
    let s2 = [bof, rec(0x027E, &cat(&[&u16s(&[0, 0, 0]), &((3u32 << 2) | 2).to_le_bytes()])), rec(0x000A, &[])].concat();
    let stream = book(&head, &["Datos", "Hoja 2"], &tail, &[s1, s2], |n, pos| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0], &short(n)])));
    let (wb, fmt) = open_book("legacy97.xls", &ole(&[("Workbook", &stream)]));
    assert_eq!(fmt, Format::Xls);
    assert!(!fmt.can_save());
    assert_eq!(wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Datos", "Hoja 2"]);
    assert_eq!(value(&wb, 0, "A1"), Value::Number(2.5));
    assert_eq!(value(&wb, 0, "B1"), Value::text("Año"));
    assert_eq!(value(&wb, 1, "A1"), Value::Number(3.0));
    assert_eq!(formula(&wb, 0, "C1"), "A1*2+Tasa");
    assert_eq!(formula(&wb, 0, "D1"), "SUM('Hoja 2'!A1:B2)");
    assert_eq!(value(&wb, 0, "C1"), Value::Number(8.0));
    let st = wb.styles.get(wb.sheets[0].get(CellRef::new(0, 0)).unwrap().style);
    assert!(st.bold);
    assert_eq!(&*st.num_fmt, "0.00%");
    assert_eq!(st.fill, Some(0xFFFF00));
    assert!(wb.sheets[0].merges.iter().any(|m| m.start == CellRef::new(1, 0) && m.end == CellRef::new(3, 2)));
    assert!(wb.names.iter().any(|n| n.name == "Tasa"));
    // WPS Office saves its .et workbooks in the same format.
    let (wb, fmt) = open_book("legacy97.et", &ole(&[("Workbook", &stream)]));
    assert_eq!(fmt, Format::Xls);
    assert_eq!(formula(&wb, 0, "C1"), "A1*2+Tasa");
}

#[test]
fn reads_excel_95_workbooks() {
    let short = |s: &[u8]| cat(&[&[s.len() as u8], s]);
    let head = vec![
        rec(0x0809, &cat(&[&u16s(&[0x0500, 0x0005]), &[0; 4]])),
        rec(0x0042, &u16s(&[1252])),
        rec(0x0031, &cat(&[&u16s(&[240, 2, 0x7FFF, 700, 0]), &[0, 0, 0, 0], &short(b"Arial")])),
        rec(0x041E, &cat(&[&u16s(&[164]), &short(b"0.0")])),
        // A BIFF5 XF: centred and wrapped, with a solid red fill.
        rec(0x00E0, &cat(&[&u16s(&[0, 164, 1, 0x0A]), &(10u32 | 1 << 16).to_le_bytes(), &0u32.to_le_bytes()])),
    ];
    let tail = vec![rec(0x000A, &[])];
    let bof = rec(0x0809, &cat(&[&u16s(&[0x0500, 0x0010]), &[0; 4]]));
    // 'Hoja 2'!$A$1*2: a BIFF5 3D reference to this workbook's sheet 1.
    let rgce = cat(&[&[0x3A], &u16s(&[0xFFFF]), &[0; 8], &u16s(&[1, 1, 0]), &[0, 0x1E], &u16s(&[2]), &[0x05]]);
    let s1 = [
        bof.clone(),
        rec(0x0203, &cat(&[&u16s(&[0, 0, 0]), &1.25f64.to_le_bytes()])),
        rec(0x0204, &cat(&[&u16s(&[0, 1, 0, 3]), b"A\xF1o"])),
        rec(0x0006, &cat(&[&u16s(&[0, 2, 0]), &6f64.to_le_bytes(), &[0; 6], &u16s(&[rgce.len() as u16]), &rgce])),
        rec(0x001C, &cat(&[&u16s(&[1, 0, 4]), b"Nota"])),
        rec(0x000A, &[]),
    ]
    .concat();
    let s2 = [bof, rec(0x0203, &cat(&[&u16s(&[0, 0, 0]), &3f64.to_le_bytes()])), rec(0x000A, &[])].concat();
    let stream = book(&head, &["Datos", "Hoja 2"], &tail, &[s1, s2], |n, pos| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0], &short(n.as_bytes())])));
    let (wb, fmt) = open_book("legacy95.xls", &ole(&[("Book", &stream)]));
    assert_eq!(fmt, Format::Xls);
    assert_eq!(value(&wb, 0, "A1"), Value::Number(1.25));
    assert_eq!(value(&wb, 0, "B1"), Value::text("Año"));
    assert_eq!(formula(&wb, 0, "C1"), "'Hoja 2'!$A$1*2");
    let st = wb.styles.get(wb.sheets[0].get(CellRef::new(0, 0)).unwrap().style);
    assert_eq!(&*st.num_fmt, "0.0");
    assert!(st.wrap);
    assert_eq!(st.halign, apex_sheet::HAlign::Center);
    assert_eq!(st.fill, Some(0xFF0000));
    let note = wb.sheets[0].get(CellRef::new(1, 0)).and_then(|c| c.note()).unwrap();
    assert_eq!(note.text, "Nota");
}

#[test]
fn reads_word_97_documents() {
    let text = b"Hola mundo\r";
    let n = text.len() as u32;
    // The FIB: base, 14 shorts, 22 longs (ccpText at 3), 93 fc/lcb pairs.
    let mut lw = [0u32; 22];
    lw[0] = 1024 + n;
    lw[3] = n;
    let mut fclcb = [(0u32, 0u32); 93];
    let mut table = Vec::new();
    // The piece table: one compressed piece at byte 1024.
    let clx = cat(&[&[2], &16u32.to_le_bytes(), &0u32.to_le_bytes(), &n.to_le_bytes(), &u16s(&[0]), &(2048u32 | 0x4000_0000).to_le_bytes(), &u16s(&[0])]);
    fclcb[33] = (table.len() as u32, clx.len() as u32);
    table.extend(clx);
    // Bold "Hola" through a character FKP on page 3.
    let bte = cat(&[&1024u32.to_le_bytes(), &(1024 + n).to_le_bytes(), &3u32.to_le_bytes()]);
    fclcb[12] = (table.len() as u32, bte.len() as u32);
    table.extend(bte);
    let mut fkp = vec![0u8; 512];
    for (i, fc) in [1024u32, 1028, 1024 + n].iter().enumerate() {
        fkp[i * 4..i * 4 + 4].copy_from_slice(&fc.to_le_bytes());
    }
    fkp[12] = 250;
    fkp[500..504].copy_from_slice(&[3, 0x35, 0x08, 1]);
    fkp[511] = 2;
    let mut wd = cat(&[&u16s(&[0xA5EC, 0x00C1, 0, 0, 0, 0x0200]), &[0; 20], &u16s(&[14]), &[0; 28], &u16s(&[22])]);
    wd.extend(lw.iter().flat_map(|x| x.to_le_bytes()));
    wd.extend(u16s(&[93]));
    wd.extend(fclcb.iter().flat_map(|(a, b)| cat(&[&a.to_le_bytes(), &b.to_le_bytes()])));
    wd.resize(1024, 0);
    wd.extend(text);
    wd.resize(1536, 0);
    wd.extend(fkp);
    let path = write("legacy.doc", &ole(&[("WordDocument", &wd), ("1Table", &table)]));
    let (loaded, fmt) = apex_io::open(&path).unwrap();
    assert_eq!(fmt, Format::Doc);
    assert!(!fmt.can_save());
    let Loaded::Doc(doc) = loaded else { panic!("not a document") };
    assert_eq!(doc.paragraphs[0].text, "Hola mundo");
    let runs = &doc.paragraphs[0].runs;
    assert_eq!(runs[0].len, 4);
    assert!(runs[0].attrs.bold);
    assert!(!runs.last().unwrap().attrs.bold);
    // A Word 97 file under a package extension opens by its content, as
    // in Office.
    let path = write("legacy.docm", &ole(&[("WordDocument", &wd), ("1Table", &table)]));
    let (loaded, fmt) = apex_io::open(&path).unwrap();
    assert_eq!(fmt, Format::Doc);
    let Loaded::Doc(doc) = loaded else { panic!("not a document") };
    assert_eq!(doc.paragraphs[0].text, "Hola mundo");
    // WPS Office saves its .wps documents in the same format.
    let path = write("legacy.wps", &ole(&[("WordDocument", &wd), ("1Table", &table)]));
    let (loaded, fmt) = apex_io::open(&path).unwrap();
    assert_eq!(fmt, Format::Doc);
    let Loaded::Doc(doc) = loaded else { panic!("not a document") };
    assert_eq!(doc.paragraphs[0].text, "Hola mundo");
}

#[test]
fn legacy_formats_are_not_saved() {
    let wb = Workbook::new();
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("never.xls");
    assert!(apex_io::save_sheet_with(&wb, &path, Format::Xls, None).is_err());
    assert!(!path.exists());
}

/// An OfficeArt record: version and instance, type and data.
fn art(vi: u16, ty: u16, data: &[u8]) -> Vec<u8> {
    cat(&[&u16s(&[vi, ty]), &(data.len() as u32).to_le_bytes(), data])
}

#[test]
fn reads_excel_97_sheet_tools() {
    use apex_sheet::chart::{ChartType, LegendPos};
    use apex_sheet::data::{CfKind, CmpOp, FilterKind, ValKind};
    use apex_sheet::functions::CritOp;
    use apex_sheet::{ImageFormat, Range};
    let short = |s: &str| cat(&[&[s.len() as u8, 0], s.as_bytes()]);
    let ustr = |s: &str| cat(&[&u16s(&[s.len() as u16]), &[0], s.as_bytes()]);
    let head = vec![
        rec(0x0809, &cat(&[&u16s(&[0x0600, 0x0005]), &[0; 12]])),
        rec(0x0031, &cat(&[&u16s(&[200, 0, 0x7FFF, 400, 0]), &[0, 0, 0, 0], &short("Arial")])),
        rec(0x00E0, &cat(&[&u16s(&[0, 0, 1]), &[0x20, 0, 0, 0], &[0; 8], &u16s(&[64])])),
    ];
    // A PNG in the workbook's BStore.
    let png = cat(&[b"\x89PNG\r\n\x1a\n", &[0; 24]]);
    let blip = art(0x6E00, 0xF01E, &cat(&[&[0; 17], &png]));
    let bse = art(0x0062, 0xF007, &cat(&[&[6, 6], &[0; 16], &u16s(&[0xFF]), &(blip.len() as u32).to_le_bytes(), &1u32.to_le_bytes(), &[0; 8], &blip]));
    let dgg = art(0x000F, 0xF000, &art(0x001F, 0xF001, &bse));
    // _FilterDatabase (hidden, built in) = Datos!$A$1:$B$4.
    let area = cat(&[&[0x3B], &u16s(&[0, 0, 3, 0, 1])]);
    let tail = vec![
        rec(0x01AE, &u16s(&[1, 0x0401])),
        rec(0x0017, &u16s(&[1, 0, 0, 0])),
        rec(0x0018, &cat(&[&u16s(&[0x21]), &[0, 1], &u16s(&[area.len() as u16, 0, 1]), &[0, 0, 0, 0, 0, 0x0D], &area])),
        rec(0x00EB, &dgg),
        rec(0x000A, &[]),
    ];
    let bof = |dt: u16| rec(0x0809, &cat(&[&u16s(&[0x0600, dt]), &[0; 12]]));
    // Cell value > 10 with a solid red fill on A1:A4, then the formula A1>5.
    let dxf_fill = cat(&[&(1u32 << 29 | 1 << 17).to_le_bytes(), &u16s(&[0, 1 << 10, 10 << 7 | 64])]);
    let dxf_none = cat(&[&0u32.to_le_bytes(), &u16s(&[0])]);
    let gt5 = cat(&[&[0x2C], &u16s(&[0, 0xC000]), &[0x1E], &u16s(&[5]), &[0x0D]]);
    // A list of literals (NUL-separated) on B2:B5.
    let list = cat(&[&[0x17, 5, 0], b"a\0b\0c"]);
    let dv_flags: u32 = 3 | 0x80 | 0x100 | 1 << 18 | 1 << 19;
    let obj = |ot: u16, id: u16| rec(0x005D, &cat(&[&u16s(&[0x15, 0x12, ot, id, 0]), &[0; 12], &[0; 4]]));
    let anchor = |a: [u16; 9]| art(0, 0xF010, &u16s(&a));
    let fsp = art(0x0002, 0xF00A, &[0; 8]);
    let pic = art(0x000F, 0xF004, &cat(&[&fsp, &art(0x0013, 0xF00B, &cat(&[&u16s(&[0x4104]), &1u32.to_le_bytes()])), &anchor([2, 1, 0, 1, 0, 3, 0, 5, 0]), &art(0, 0xF011, &[])]));
    let graph = art(0x000F, 0xF004, &cat(&[&fsp, &anchor([0, 4, 0, 1, 0, 8, 0, 10, 0]), &art(0, 0xF011, &[])]));
    let group = art(0x000F, 0xF004, &fsp);
    let dg = art(0x000F, 0xF002, &art(0x000F, 0xF003, &cat(&[&group, &pic, &graph])));
    let values = cat(&[&[0x3B], &u16s(&[0, 0, 3, 1, 1])]);
    let title = cat(&[&u16s(&[0]), &[6, 0], b"Ventas"]);
    let chart = [
        bof(0x0020),
        rec(0x1002, &[0; 16]),
        rec(0x1033, &[]),
        rec(0x1003, &u16s(&[1, 1, 4, 4, 1, 0])),
        rec(0x1033, &[]),
        rec(0x1051, &cat(&[&[1, 2], &u16s(&[0, 0, values.len() as u16]), &values])),
        rec(0x1034, &[]),
        rec(0x1025, &[0; 32]),
        rec(0x1033, &[]),
        rec(0x100D, &title),
        rec(0x1027, &u16s(&[1, 0, 0])),
        rec(0x1034, &[]),
        rec(0x1014, &[0; 20]),
        rec(0x1033, &[]),
        rec(0x1017, &u16s(&[0, 150, 1])),
        rec(0x1015, &cat(&[&[0; 16], &[0, 1], &u16s(&[0])])),
        rec(0x1034, &[]),
        rec(0x1034, &[]),
        rec(0x000A, &[]),
    ]
    .concat();
    let s1 = [
        bof(0x0010),
        rec(0x0203, &cat(&[&u16s(&[0, 0, 0]), &2.5f64.to_le_bytes()])),
        rec(0x00EC, &dg),
        obj(8, 1),
        obj(5, 2),
        chart,
        rec(0x01B0, &u16s(&[2, 1 << 1, 0, 3, 0, 0, 1, 0, 3, 0, 0])),
        rec(0x01B1, &cat(&[&[1, 5], &u16s(&[3, 0]), &dxf_fill, &[0x1E], &u16s(&[10])])),
        rec(0x01B1, &cat(&[&[2, 0], &u16s(&[gt5.len() as u16, 0]), &dxf_none, &gt5])),
        rec(0x01B2, &cat(&[&u16s(&[0]), &[0; 8], &0xFFFF_FFFFu32.to_le_bytes(), &1u32.to_le_bytes()])),
        rec(0x01BE, &cat(&[&dv_flags.to_le_bytes(), &ustr("Elige"), &ustr("Error"), &ustr("Uno de la lista"), &ustr("No vale"), &u16s(&[list.len() as u16, 0]), &list, &u16s(&[0, 0]), &u16s(&[1, 1, 4, 1, 1])])),
        rec(0x009D, &u16s(&[2])),
        // Column B > 3.
        rec(0x009E, &cat(&[&u16s(&[1, 0]), &[4, 4], &3f64.to_le_bytes(), &[0; 10]])),
        rec(0x000A, &[]),
    ]
    .concat();
    let stream = book(&head, &["Datos"], &tail, &[s1], |n, pos| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0], &short(n)])));
    let (wb, _) = open_book("legacy97tools.xls", &ole(&[("Workbook", &stream)]));
    let sh = &wb.sheets[0];
    assert_eq!(value(&wb, 0, "A1"), Value::Number(2.5));
    // Conditional formats.
    assert_eq!(sh.cond_formats.len(), 2);
    let r = &sh.cond_formats[0];
    assert_eq!(r.ranges, vec![Range::parse("A1:A4").unwrap()]);
    assert_eq!(r.kind, CfKind::CellIs { op: CmpOp::Greater, f1: "10".into(), f2: String::new() });
    assert_eq!(r.style.fill, Some(0xFF0000));
    assert_eq!(sh.cond_formats[1].kind, CfKind::Expression("A1>5".into()));
    // Data validation.
    let v = &sh.validations[0];
    assert_eq!(v.ranges, vec![Range::parse("B2:B5").unwrap()]);
    assert_eq!(v.kind, ValKind::List);
    assert!(v.f1.contains("a,b,c"), "{}", v.f1);
    assert_eq!((v.input_title.as_str(), v.error_msg.as_str()), ("Elige", "No vale"));
    assert!(v.show_input && v.show_error && v.allow_blank);
    // Autofilter.
    let f = sh.filter.as_ref().unwrap();
    assert_eq!(f.range, Range::parse("A1:B4").unwrap());
    assert_eq!(f.cols.len(), 1);
    assert_eq!(f.cols[0].col, 1);
    assert_eq!(f.cols[0].kind, FilterKind::Custom { and: true, a: (CritOp::Gt, "3".into()), b: None });
    // The picture and the chart.
    assert_eq!(sh.images.len(), 1);
    assert_eq!(sh.images[0].format, ImageFormat::Png);
    assert_eq!(sh.images[0].from, CellRef::new(1, 1));
    assert_eq!(sh.charts.len(), 1);
    let ch = &sh.charts[0];
    assert_eq!(ch.kind, ChartType::Bar);
    assert_eq!(ch.title, "Ventas");
    assert_eq!(ch.legend, LegendPos::Bottom);
    assert_eq!(ch.from, CellRef::new(1, 4));
    assert_eq!(ch.series.len(), 1);
    assert_eq!(ch.series[0].values.formula.as_ref().unwrap().to_canonical(), "Datos!$B$1:$B$4");
}
