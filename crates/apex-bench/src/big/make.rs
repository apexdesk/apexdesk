//! Big files written as a stream (never whole in memory), the way other
//! programs write them: a workbook of a million rows with formulas and
//! their cached values, an xlsx of about 200 MB, a Word document of a
//! thousand pages with tables and images, a csv of 500 MB.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use miniz_oxide::deflate::core::{CompressorOxide, TDEFLFlush, TDEFLStatus, compress, create_comp_flags_from_zip_params};

use crate::fuzz::rng::Rng;

/// A zip written part by part; each part is deflated as it comes.
pub struct ZipStream {
    out: BufWriter<File>,
    pos: u64,
    central: Vec<u8>,
    count: u16,
    // The part being written.
    name: String,
    offset: u64,
    crc: u32,
    size: u64,
    csize: u64,
    comp: Box<CompressorOxide>,
    buf: Vec<u8>,
}

impl ZipStream {
    pub fn create(path: &Path) -> std::io::Result<ZipStream> {
        Ok(ZipStream {
            out: BufWriter::with_capacity(1 << 20, File::create(path)?),
            pos: 0,
            central: Vec::new(),
            count: 0,
            name: String::new(),
            offset: 0,
            crc: !0,
            size: 0,
            csize: 0,
            comp: Box::new(CompressorOxide::new(create_comp_flags_from_zip_params(6, -15, 0))),
            buf: vec![0; 1 << 16],
        })
    }

    fn raw(&mut self, d: &[u8]) -> std::io::Result<()> {
        self.out.write_all(d)?;
        self.pos += d.len() as u64;
        Ok(())
    }

    /// Starts a part (sizes and CRC go in a data descriptor after it).
    pub fn start(&mut self, name: &str) -> std::io::Result<()> {
        self.name = name.to_string();
        self.offset = self.pos;
        self.crc = !0;
        self.size = 0;
        self.csize = 0;
        self.comp = Box::new(CompressorOxide::new(create_comp_flags_from_zip_params(6, -15, 0)));
        let mut h = Vec::new();
        h.extend(0x0403_4b50u32.to_le_bytes());
        h.extend(20u16.to_le_bytes());
        h.extend(0x0808u16.to_le_bytes()); // data descriptor, UTF-8
        h.extend(8u16.to_le_bytes());
        h.extend([0, 0, 0x21, 0]);
        h.extend([0; 12]);
        h.extend((name.len() as u16).to_le_bytes());
        h.extend(0u16.to_le_bytes());
        h.extend(name.as_bytes());
        self.raw(&h)
    }

    pub fn write(&mut self, d: &[u8]) -> std::io::Result<()> {
        self.crc = crc_update(self.crc, d);
        self.size += d.len() as u64;
        self.deflate(d, TDEFLFlush::None)
    }

    fn deflate(&mut self, mut d: &[u8], flush: TDEFLFlush) -> std::io::Result<()> {
        loop {
            let mut buf = std::mem::take(&mut self.buf);
            let (status, used, made) = compress(&mut self.comp, d, &mut buf, flush);
            self.csize += made as u64;
            let r = self.raw(&buf[..made]);
            self.buf = buf;
            r?;
            d = &d[used..];
            match status {
                TDEFLStatus::Done => return Ok(()),
                TDEFLStatus::Okay if d.is_empty() && (flush == TDEFLFlush::None) => return Ok(()),
                TDEFLStatus::Okay => {}
                _ => return Err(std::io::Error::other("deflate failed")),
            }
        }
    }

    pub fn finish_part(&mut self) -> std::io::Result<()> {
        self.deflate(&[], TDEFLFlush::Finish)?;
        let crc = !self.crc;
        if self.size > u32::MAX as u64 || self.pos > u32::MAX as u64 {
            return Err(std::io::Error::other("part too big for a zip without zip64"));
        }
        let mut dd = Vec::new();
        dd.extend(0x0807_4b50u32.to_le_bytes());
        dd.extend(crc.to_le_bytes());
        dd.extend((self.csize as u32).to_le_bytes());
        dd.extend((self.size as u32).to_le_bytes());
        self.raw(&dd)?;
        let c = &mut self.central;
        c.extend(0x0201_4b50u32.to_le_bytes());
        c.extend(20u16.to_le_bytes());
        c.extend(20u16.to_le_bytes());
        c.extend(0x0808u16.to_le_bytes());
        c.extend(8u16.to_le_bytes());
        c.extend([0, 0, 0x21, 0]);
        c.extend(crc.to_le_bytes());
        c.extend((self.csize as u32).to_le_bytes());
        c.extend((self.size as u32).to_le_bytes());
        c.extend((self.name.len() as u16).to_le_bytes());
        c.extend([0; 12]);
        c.extend((self.offset as u32).to_le_bytes());
        c.extend(self.name.as_bytes());
        self.count += 1;
        Ok(())
    }

    pub fn part(&mut self, name: &str, d: &[u8]) -> std::io::Result<()> {
        self.start(name)?;
        self.write(d)?;
        self.finish_part()
    }

    /// Compressed bytes written so far.
    pub fn written(&self) -> u64 {
        self.pos
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        let at = self.pos as u32;
        let central = std::mem::take(&mut self.central);
        self.raw(&central)?;
        let mut e = Vec::new();
        e.extend(0x0605_4b50u32.to_le_bytes());
        e.extend([0; 4]);
        e.extend(self.count.to_le_bytes());
        e.extend(self.count.to_le_bytes());
        e.extend((central.len() as u32).to_le_bytes());
        e.extend(at.to_le_bytes());
        e.extend(0u16.to_le_bytes());
        self.raw(&e)?;
        self.out.flush()
    }
}

fn crc_update(mut c: u32, d: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut x = i as u32;
            for _ in 0..8 {
                x = if x & 1 != 0 { 0xEDB8_8320 ^ (x >> 1) } else { x >> 1 };
            }
            *e = x;
        }
        t
    });
    for &b in d {
        c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c
}

pub fn col_name(mut c: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

const CONTENT_TYPES_XLSX: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#;
const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
const WB_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#;
const WORKBOOK: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets><calcPr calcId="191029"/></workbook>"#;

fn xlsx_head(z: &mut ZipStream, cols: u32, rows: u32) -> std::io::Result<()> {
    z.part("[Content_Types].xml", CONTENT_TYPES_XLSX.as_bytes())?;
    z.part("_rels/.rels", RELS.as_bytes())?;
    z.part("xl/workbook.xml", WORKBOOK.as_bytes())?;
    z.part("xl/_rels/workbook.xml.rels", WB_RELS.as_bytes())?;
    z.start("xl/worksheets/sheet1.xml")?;
    z.write(
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><dimension ref=\"A1:{}{}\"/><sheetData>",
            col_name(cols - 1),
            rows
        )
        .as_bytes(),
    )
}

fn shared_strings(z: &mut ZipStream, strings: impl Iterator<Item = String>, count: usize) -> std::io::Result<()> {
    z.start("xl/sharedStrings.xml")?;
    z.write(format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" count=\"{count}\" uniqueCount=\"{count}\">").as_bytes())?;
    let mut buf = String::new();
    for s in strings {
        buf.push_str("<si><t>");
        buf.push_str(&s);
        buf.push_str("</t></si>");
        if buf.len() > 1 << 20 {
            z.write(buf.as_bytes())?;
            buf.clear();
        }
    }
    buf.push_str("</sst>");
    z.write(buf.as_bytes())?;
    z.finish_part()
}

/// A million rows: a number, a text, and four formulas (arithmetic,
/// ROUND, IF of MOD, a running total that depends on the row above) with
/// the values Excel would have stored.
pub fn sheet_million(path: &Path, rows: u32) -> std::io::Result<()> {
    let mut z = ZipStream::create(path)?;
    xlsx_head(&mut z, 6, rows)?;
    let mut buf = String::with_capacity(2 << 20);
    let mut total = 0f64;
    for r in 1..=rows {
        let a = r as f64;
        let c = a * 2.0 + 1.0;
        let d = (c / 3.0 * 100.0).round() / 100.0;
        total += c;
        let e = if r % 2 == 0 { "even" } else { "odd" };
        let f = if r == 1 { format!("C{r}") } else { format!("F{}+C{r}", r - 1) };
        use std::fmt::Write as _;
        let _ = write!(
            buf,
            "<row r=\"{r}\"><c r=\"A{r}\"><v>{r}</v></c><c r=\"B{r}\" t=\"s\"><v>{}</v></c><c r=\"C{r}\"><f>A{r}*2+1</f><v>{c}</v></c><c r=\"D{r}\"><f>ROUND(C{r}/3,2)</f><v>{d}</v></c><c r=\"E{r}\" t=\"str\"><f>IF(MOD(A{r},2)=0,\"even\",\"odd\")</f><v>{e}</v></c><c r=\"F{r}\"><f>{f}</f><v>{total}</v></c></row>",
            r - 1
        );
        if buf.len() > 1 << 20 {
            z.write(buf.as_bytes())?;
            buf.clear();
        }
    }
    buf.push_str("</sheetData></worksheet>");
    z.write(buf.as_bytes())?;
    z.finish_part()?;
    shared_strings(&mut z, (0..rows).map(|i| format!("item {i}")), rows as usize)?;
    z.finish()
}

/// An xlsx of about `target` bytes: 20 columns of numbers, dates and
/// texts, rows until the file is that big (or the sheet is full).
pub fn sheet_bytes(path: &Path, target: u64) -> std::io::Result<u32> {
    let cols = 20u32;
    let mut z = ZipStream::create(path)?;
    // The dimension is not known before; Excel files have one, readers
    // must not trust it anyway.
    xlsx_head(&mut z, cols, 1_048_576)?;
    let mut rng = Rng::new(42);
    let mut buf = String::with_capacity(2 << 20);
    let mut rows = 0u32;
    let strings = 5000usize;
    while rows < 1_048_576 && z.written() < target {
        rows += 1;
        let r = rows;
        use std::fmt::Write as _;
        let _ = write!(buf, "<row r=\"{r}\">");
        for c in 0..cols {
            let name = col_name(c);
            match c % 5 {
                0 => {
                    let _ = write!(buf, "<c r=\"{name}{r}\"><v>{}</v></c>", rng.below(1_000_000));
                }
                1 | 2 => {
                    let _ = write!(buf, "<c r=\"{name}{r}\"><v>{}</v></c>", rng.next() as f64 / u64::MAX as f64 * 10_000.0);
                }
                3 => {
                    let _ = write!(buf, "<c r=\"{name}{r}\" s=\"0\"><v>{}</v></c>", 40_000 + rng.below(10_000));
                }
                _ => {
                    let _ = write!(buf, "<c r=\"{name}{r}\" t=\"s\"><v>{}</v></c>", rng.below(strings));
                }
            }
        }
        buf.push_str("</row>");
        if buf.len() > 1 << 20 {
            z.write(buf.as_bytes())?;
            buf.clear();
        }
    }
    buf.push_str("</sheetData></worksheet>");
    z.write(buf.as_bytes())?;
    z.finish_part()?;
    shared_strings(&mut z, (0..strings).map(|i| format!("text value number {i} with some words")), strings)?;
    z.finish()?;
    Ok(rows)
}

/// A csv of about `target` bytes, of the usual kinds of columns.
pub fn csv(path: &Path, target: u64) -> std::io::Result<u64> {
    let mut out = BufWriter::with_capacity(1 << 20, File::create(path)?);
    let mut rng = Rng::new(7);
    out.write_all(b"id,name,amount,quantity,date,comment,flag,ratio,code,city\n")?;
    let mut written = 0u64;
    let mut rows = 0u64;
    let cities = ["Madrid", "Lugo", "Vigo", "M\u{e1}laga", "Bilbao", "Zaragoza"];
    let mut line = String::new();
    while written < target {
        rows += 1;
        line.clear();
        use std::fmt::Write as _;
        let _ = writeln!(
            line,
            "{rows},Name {},{:.2},{},2024-{:02}-{:02},\"text, with a comma {}\",{},{:.4},C{:06},{}",
            rng.below(100_000),
            rng.next() as f64 / u64::MAX as f64 * 100_000.0,
            rng.below(1000),
            1 + rng.below(12),
            1 + rng.below(28),
            rng.below(1000),
            if rng.one_in(2) { "TRUE" } else { "FALSE" },
            rng.next() as f64 / u64::MAX as f64,
            rng.below(1_000_000),
            rng.pick(&cities)
        );
        out.write_all(line.as_bytes())?;
        written += line.len() as u64;
    }
    out.flush()?;
    Ok(rows)
}

/// A PNG of `w`×`h` pixels, a gradient of the given hue.
pub fn png(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let mut raw = Vec::with_capacity(((w * 3 + 1) * h) as usize);
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            raw.extend([(x * 255 / w) as u8, (y * 255 / h) as u8, (seed * 37 % 256) as u8]);
        }
    }
    let chunk = |out: &mut Vec<u8>, kind: &[u8], data: &[u8]| {
        out.extend((data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend(data);
        out.extend(&c);
        out.extend((!crc_update(!0, &c)).to_be_bytes());
    };
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
    chunk(&mut out, b"IEND", &[]);
    out
}

const LOREM: &[&str] = &[
    "lorem", "ipsum", "dolor", "sit", "amet", "consectetur", "adipiscing", "elit", "sed", "do", "eiusmod", "tempor", "incididunt", "ut", "labore", "et",
    "dolore", "magna", "aliqua", "enim", "ad", "minim", "veniam", "quis", "nostrud", "exercitation", "ullamco", "laboris", "nisi", "aliquip", "ex", "ea",
    "commodo", "consequat", "duis", "aute", "irure", "in", "reprehenderit", "voluptate", "velit", "esse", "cillum", "fugiat", "nulla", "pariatur",
];

/// A Word document of `pages` pages: each has a heading, paragraphs,
/// every other one a table, every third one an image, and a page break.
pub fn document(path: &Path, pages: u32) -> std::io::Result<()> {
    let images = 20u32;
    let mut z = ZipStream::create(path)?;
    let mut types = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>"#,
    );
    types.push_str("</Types>");
    z.part("[Content_Types].xml", types.as_bytes())?;
    z.part("_rels/.rels", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#)?;
    let mut rels = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#);
    for i in 0..images {
        rels.push_str(&format!(r#"<Relationship Id="rImg{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image{i}.png"/>"#));
    }
    rels.push_str("</Relationships>");
    z.part("word/_rels/document.xml.rels", rels.as_bytes())?;
    z.part("word/styles.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Liberation Serif" w:hAnsi="Liberation Serif"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4"/><w:left w:val="single" w:sz="4"/><w:bottom w:val="single" w:sz="4"/><w:right w:val="single" w:sz="4"/><w:insideH w:val="single" w:sz="4"/><w:insideV w:val="single" w:sz="4"/></w:tblBorders></w:tblPr></w:style></w:styles>"#)?;
    for i in 0..images {
        z.part(&format!("word/media/image{i}.png"), &png(400, 300, i))?;
    }
    z.start("word/document.xml")?;
    z.write(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>"#)?;
    let mut rng = Rng::new(3);
    let mut buf = String::new();
    let words = |rng: &mut Rng, n: usize| -> String { (0..n).map(|_| *rng.pick(LOREM)).collect::<Vec<_>>().join(" ") };
    for p in 1..=pages {
        use std::fmt::Write as _;
        let _ = write!(buf, r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Chapter {p}: {}</w:t></w:r></w:p>"#, words(&mut rng, 4));
        let paras = if p % 3 == 0 { 2 } else { 4 };
        for _ in 0..paras {
            let _ = write!(buf, r#"<w:p><w:r><w:t xml:space="preserve">{} </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">{} </w:t></w:r><w:r><w:t>{}.</w:t></w:r></w:p>"#, words(&mut rng, 40), words(&mut rng, 3), words(&mut rng, 40));
        }
        if p % 2 == 0 {
            buf.push_str(r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="5000" w:type="pct"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid>"#);
            for r in 0..6 {
                buf.push_str("<w:tr>");
                for c in 0..4 {
                    let _ = write!(buf, r#"<w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr><w:p><w:r><w:t>{} {}</w:t></w:r></w:p></w:tc>"#, words(&mut rng, 2), r * 4 + c);
                }
                buf.push_str("</w:tr>");
            }
            buf.push_str("</w:tbl>");
        }
        if p % 3 == 0 {
            let i = p % images;
            let _ = write!(buf, r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="3048000" cy="2286000"/><wp:docPr id="{p}" name="Picture {p}"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{p}" name="image{i}.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rImg{i}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3048000" cy="2286000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#);
        }
        if p < pages {
            buf.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
        }
        if buf.len() > 1 << 20 {
            z.write(buf.as_bytes())?;
            buf.clear();
        }
    }
    buf.push_str(r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1417" w:right="1417" w:bottom="1417" w:left="1417" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#);
    z.write(buf.as_bytes())?;
    z.finish_part()?;
    z.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_files_open() {
        let dir = std::env::temp_dir().join(format!("apex-bench-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("m.xlsx");
        sheet_million(&p, 50).unwrap();
        let (apex_io::Loaded::Sheet(mut wb), _) = apex_io::open(&p).unwrap() else { panic!() };
        let f50 = wb.sheets[0].value(apex_sheet::CellRef::new(49, 5)).clone();
        wb.recalc_all();
        assert_eq!(wb.sheets[0].value(apex_sheet::CellRef::new(49, 5)), &f50);
        let p = dir.join("b.xlsx");
        let rows = sheet_bytes(&p, 20_000).unwrap();
        assert!(rows > 10);
        assert!(apex_io::open(&p).is_ok());
        let p = dir.join("d.docx");
        document(&p, 4).unwrap();
        let (apex_io::Loaded::Doc(doc), _) = apex_io::open(&p).unwrap() else { panic!() };
        assert!(doc.images().len() >= 1);
        assert!(doc.paragraphs.iter().any(|p| p.cell.is_some()));
        let p = dir.join("c.csv");
        csv(&p, 10_000).unwrap();
        assert!(apex_io::open(&p).is_ok());
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(26), "AA");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
