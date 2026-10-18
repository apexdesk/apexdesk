//! PDF/A-2b (ISO 19005-2, long-term archiving): an sRGB output intent
//! with its ICC profile, and XMP metadata matching the document
//! information. Everything else PDF/A asks for (embedded fonts with
//! Unicode maps, a file ID, no encryption or scripts) the writer always
//! does; CMYK JPEGs are converted to RGB (see `Pdf::add_jpeg`).

use std::fmt::Write as _;

/// An ICC v2 sRGB display profile (D50 primaries, the sRGB tone curve as
/// a 1024-entry table shared by the three channels).
pub fn srgb_icc() -> Vec<u8> {
    fn s15(v: f64) -> [u8; 4] {
        ((v * 65536.0).round() as i32).to_be_bytes()
    }
    fn xyz(v: [f64; 3]) -> Vec<u8> {
        let mut t = b"XYZ \0\0\0\0".to_vec();
        for c in v {
            t.extend_from_slice(&s15(c));
        }
        t
    }
    let mut desc = b"desc\0\0\0\0".to_vec();
    let name = b"sRGB IEC61966-2.1\0";
    desc.extend_from_slice(&(name.len() as u32).to_be_bytes());
    desc.extend_from_slice(name);
    // No Unicode or ScriptCode description.
    desc.extend_from_slice(&[0; 8]);
    desc.extend_from_slice(&[0; 3]);
    desc.extend_from_slice(&[0; 67]);
    let mut cprt = b"text\0\0\0\0".to_vec();
    cprt.extend_from_slice(b"No copyright, use freely\0");
    let mut curv = b"curv\0\0\0\0".to_vec();
    let n = 1024u32;
    curv.extend_from_slice(&n.to_be_bytes());
    for i in 0..n {
        let x = i as f64 / (n - 1) as f64;
        let y = if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) };
        curv.extend_from_slice(&((y * 65535.0).round() as u16).to_be_bytes());
    }
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc),
        (b"cprt", cprt),
        (b"wtpt", xyz([0.9642, 1.0, 0.8249])),
        (b"rXYZ", xyz([0.4361, 0.2225, 0.0139])),
        (b"gXYZ", xyz([0.3851, 0.7169, 0.0971])),
        (b"bXYZ", xyz([0.1431, 0.0606, 0.7141])),
        (b"rTRC", curv),
    ];
    // The table: every tag, with gTRC and bTRC pointing at rTRC's data.
    let count = tags.len() + 2;
    let mut data = Vec::new();
    let mut table = Vec::new();
    let mut at = 128 + 4 + count * 12;
    let mut trc = (0, 0);
    for (sig, body) in &tags {
        let len = body.len();
        table.push((**sig, at, len));
        if *sig == b"rTRC" {
            trc = (at, len);
        }
        data.extend_from_slice(body);
        while data.len() % 4 != 0 {
            data.push(0);
        }
        at = 128 + 4 + count * 12 + data.len();
    }
    table.push((*b"gTRC", trc.0, trc.1));
    table.push((*b"bTRC", trc.0, trc.1));
    let size = 128 + 4 + count * 12 + data.len();
    let mut p = Vec::with_capacity(size);
    p.extend_from_slice(&(size as u32).to_be_bytes());
    p.extend_from_slice(&[0; 4]);
    p.extend_from_slice(&[2, 0x10, 0, 0]);
    p.extend_from_slice(b"mntrRGB XYZ ");
    // Date: 2024-01-01 00:00:00.
    for v in [2024u16, 1, 1, 0, 0, 0] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(b"acsp");
    p.extend_from_slice(&[0; 24]);
    p.extend_from_slice(&[0, 0, 0, 0]);
    for c in [0.9642, 1.0, 0.8249] {
        p.extend_from_slice(&s15(c));
    }
    p.extend_from_slice(&[0; 4]);
    p.resize(128, 0);
    p.extend_from_slice(&(count as u32).to_be_bytes());
    for (sig, off, len) in table {
        p.extend_from_slice(&sig);
        p.extend_from_slice(&(off as u32).to_be_bytes());
        p.extend_from_slice(&(len as u32).to_be_bytes());
    }
    p.extend_from_slice(&data);
    p
}

fn xml_esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '&' => o.push_str("&amp;"),
            '"' => o.push_str("&quot;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

/// A PDF date (`D:20260927101500Z`) as an XMP date.
pub(crate) fn xmp_date(pdf: &str) -> String {
    let d = pdf.trim_start_matches("D:");
    let g = |a: usize, b: usize| d.get(a..b).unwrap_or("00");
    format!("{}-{}-{}T{}:{}:{}Z", g(0, 4), g(4, 6), g(6, 8), g(8, 10), g(10, 12), g(12, 14))
}

/// The XMP packet: PDF/A identification plus the document information.
pub(crate) fn xmp(title: &str, author: &str, producer: &str, date: &str, lang: &str) -> String {
    let mut x = String::from("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    x.push_str(r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#);
    x.push_str(r#"<rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:pdf="http://ns.adobe.com/pdf/1.3/">"#);
    x.push_str("<pdfaid:part>2</pdfaid:part><pdfaid:conformance>B</pdfaid:conformance>");
    if !title.is_empty() {
        let _ = write!(x, r#"<dc:title><rdf:Alt><rdf:li xml:lang="x-default">{}</rdf:li></rdf:Alt></dc:title>"#, xml_esc(title));
    }
    if !author.is_empty() {
        let _ = write!(x, "<dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>", xml_esc(author));
    }
    if !lang.is_empty() {
        let _ = write!(x, "<dc:language><rdf:Bag><rdf:li>{}</rdf:li></rdf:Bag></dc:language>", xml_esc(lang));
    }
    let _ = write!(x, "<pdf:Producer>{}</pdf:Producer><xmp:CreateDate>{}</xmp:CreateDate>", xml_esc(producer), xmp_date(date));
    x.push_str("</rdf:Description></rdf:RDF></x:xmpmeta>\n<?xpacket end=\"w\"?>");
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_and_metadata() {
        let p = srgb_icc();
        assert_eq!(u32::from_be_bytes([p[0], p[1], p[2], p[3]]) as usize, p.len());
        assert_eq!(&p[12..24], b"mntrRGB XYZ ");
        assert_eq!(&p[36..40], b"acsp");
        assert_eq!(u32::from_be_bytes([p[128], p[129], p[130], p[131]]), 9);
        // Every tag lies inside the profile, 4-byte aligned.
        for i in 0..9 {
            let r = 132 + i * 12;
            let off = u32::from_be_bytes([p[r + 4], p[r + 5], p[r + 6], p[r + 7]]) as usize;
            let len = u32::from_be_bytes([p[r + 8], p[r + 9], p[r + 10], p[r + 11]]) as usize;
            assert!(off % 4 == 0 && off + len <= p.len());
        }
        assert_eq!(xmp_date("D:20260927101500Z"), "2026-09-27T10:15:00Z");
        let x = xmp("A & B", "Ana", "ApexDesk", "D:20260927101500Z", "es-ES");
        assert!(x.contains("<pdfaid:part>2</pdfaid:part>") && x.contains("A &amp; B") && x.contains("2026-09-27T10:15:00Z"));
    }
}
