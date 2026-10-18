//! Packages (xlsx, docx, ods, odt, xlsb): the zip is opened into its
//! parts, the parts are changed (XML mutated, parts removed, renamed,
//! doubled, relations pointed nowhere, small compression bombs) and it is
//! written again; sometimes the zip itself is then damaged (truncated,
//! sizes and offsets of its headers changed, compressed data corrupted).

use super::bytes;
use super::rng::Rng;
use super::xml;

#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub data: Vec<u8>,
}

/// The parts of a zip, in order; None if it isn't one.
pub fn parts(data: &[u8]) -> Option<Vec<Part>> {
    let z = apex_io::zip::ZipReader::new(data).ok()?;
    let names: Vec<String> = z.names().map(str::to_string).collect();
    let mut out = Vec::new();
    for name in names {
        let data = z.read(&name).ok()?;
        out.push(Part { name, data });
    }
    Some(out)
}

/// Writes the parts as a zip; `stored` keeps them uncompressed (the
/// `mimetype` of ODF files always is).
pub fn write(parts: &[Part], stored: bool) -> Vec<u8> {
    let mut w = apex_io::zip::ZipWriter::new();
    for p in parts {
        w.add(&p.name, &p.data, !stored && p.name != "mimetype");
    }
    w.finish()
}

fn is_xml(p: &Part) -> bool {
    let n = p.name.to_ascii_lowercase();
    n.ends_with(".xml") || n.ends_with(".rels") || n.ends_with(".vml") || n.ends_with(".rdf") || p.data.starts_with(b"<?xml")
}

/// How much a part is worth mutating: the main parts of the formats most.
fn weight(p: &Part) -> usize {
    let n = p.name.to_ascii_lowercase();
    if n.contains("worksheets/sheet") || n == "word/document.xml" || n == "content.xml" || n == "xl/workbook.xml" {
        12
    } else if n.contains("styles") || n.contains("sharedstrings") || n.contains("numbering") || n.ends_with(".rels") || n.contains("content_types") {
        6
    } else if n.contains("manifest") || n.contains("drawing") || n.contains("chart") || n.contains("comments") || n.contains("footnotes") || n.contains("header") {
        4
    } else if is_xml(p) {
        2
    } else {
        1
    }
}

fn pick_part(rng: &mut Rng, parts: &[Part], xml_only: bool) -> Option<usize> {
    let ws: Vec<usize> = parts.iter().map(|p| if xml_only && !is_xml(p) { 0 } else { weight(p) }).collect();
    let total: usize = ws.iter().sum();
    if total == 0 {
        return None;
    }
    let mut r = rng.below(total);
    for (i, w) in ws.iter().enumerate() {
        if r < *w {
            return Some(i);
        }
        r -= w;
    }
    None
}

/// One change to the parts; `other` holds the parts of another file of
/// the same format.
pub fn mutate_parts(rng: &mut Rng, parts: &mut Vec<Part>, other: Option<&[Part]>) {
    if parts.is_empty() {
        parts.push(Part { name: "[Content_Types].xml".into(), data: b"<Types/>".to_vec() });
        return;
    }
    match rng.below(20) {
        0..=10 => {
            let Some(i) = pick_part(rng, parts, true) else { return bytes::binary(rng, &mut parts[0].data) };
            let text = String::from_utf8_lossy(&parts[i].data).into_owned();
            let same = other.and_then(|o| o.iter().find(|p| p.name == parts[i].name)).map(|p| String::from_utf8_lossy(&p.data).into_owned());
            for _ in 0..8 {
                if let Some(m) = xml::mutate(rng, &text, same.as_deref()) {
                    parts[i].data = m.into_bytes();
                    return;
                }
            }
        }
        11 => {
            // A part missing (often a necessary one).
            let i = if rng.one_in(2) { pick_part(rng, parts, false).unwrap_or(0) } else { rng.below(parts.len()) };
            parts.remove(i);
        }
        12 => {
            // A relation to a part that doesn't exist, or to an odd one.
            let rels: Vec<usize> = (0..parts.len()).filter(|&i| parts[i].name.ends_with(".rels") || parts[i].name.ends_with("manifest.xml")).collect();
            if rels.is_empty() {
                return;
            }
            let i = *rng.pick(&rels);
            let text = String::from_utf8_lossy(&parts[i].data).into_owned();
            let attr = if text.contains("Target=\"") { "Target=\"" } else { "manifest:full-path=\"" };
            let hits: Vec<usize> = text.match_indices(attr).map(|(p, _)| p + attr.len()).collect();
            if hits.is_empty() {
                return;
            }
            let s = *rng.pick(&hits);
            let e = s + text[s..].find('"').unwrap_or(0);
            let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
            let target = match rng.below(6) {
                0 => "missing/part.xml".to_string(),
                1 => String::new(),
                2 => format!("/{}", rng.pick(&names)),
                3 => "../../../../../../x.xml".to_string(),
                4 => text[s..e].to_string() + "#frag",
                _ => rng.pick(&names).to_string(),
            };
            let mut t = text.clone();
            t.replace_range(s..e, &target);
            parts[i].data = t.into_bytes();
        }
        13 => {
            // A part twice.
            let i = rng.below(parts.len());
            let copy = parts[i].clone();
            parts.insert(rng.below(parts.len() + 1), copy);
        }
        14 => {
            // A part renamed (case, slashes, a path out of the package).
            let i = rng.below(parts.len());
            let n = parts[i].name.clone();
            parts[i].name = match rng.below(5) {
                0 => n.to_uppercase(),
                1 => format!("/{n}"),
                2 => n.replace('/', "\\"),
                3 => format!("../{n}"),
                _ => format!("{n}x"),
            };
        }
        15 => {
            // A small compression bomb: a part with many megabytes of
            // blanks or repeated elements, a few kilobytes compressed.
            let Some(i) = pick_part(rng, parts, true) else { return };
            let text = String::from_utf8_lossy(&parts[i].data).into_owned();
            let at = text.rfind("</").unwrap_or(text.len());
            let mb = if cfg!(test) { 1 } else { *rng.pick(&[8usize, 32, 100]) };
            let filler = if rng.one_in(2) { " ".repeat(mb << 20) } else { "<x/>".repeat((mb << 20) / 4) };
            let mut t = text;
            t.insert_str(at, &filler);
            parts[i].data = t.into_bytes();
        }
        16 => {
            // A part's data replaced: empty, junk, or another part's.
            let i = rng.below(parts.len());
            parts[i].data = match rng.below(3) {
                0 => Vec::new(),
                1 => (0..rng.small_len(4096)).map(|_| rng.next() as u8).collect(),
                _ => parts[rng.below(parts.len())].data.clone(),
            };
        }
        17 => {
            // Binary parts (images, embedded objects, vbaProject.bin, xlsb
            // records): bytes changed.
            let bins: Vec<usize> = (0..parts.len()).filter(|&i| !is_xml(&parts[i])).collect();
            let i = if bins.is_empty() { rng.below(parts.len()) } else { *rng.pick(&bins) };
            if parts[i].data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
                parts[i].data = super::ole::mutate(rng, &parts[i].data);
            } else {
                for _ in 0..rng.small_len(4) {
                    bytes::binary(rng, &mut parts[i].data);
                }
            }
        }
        18 => {
            // A part of another file of the same kind.
            let Some(o) = other.filter(|o| !o.is_empty()) else { return };
            let p = rng.pick(o).clone();
            match parts.iter().position(|q| q.name == p.name) {
                Some(i) => parts[i] = p,
                None => parts.push(p),
            }
        }
        _ => {
            // Many parts.
            let n = *rng.pick(&[100usize, 5000]);
            let mut base = parts[rng.below(parts.len())].clone();
            if base.data.len() * n > 64 << 20 {
                base.data.truncate(64);
            }
            for k in 0..n {
                parts.push(Part { name: format!("{}{k}", base.name), data: base.data.clone() });
            }
        }
    }
}

fn u16_at(d: &[u8], i: usize) -> Option<usize> {
    Some(u16::from_le_bytes(d.get(i..i + 2)?.try_into().ok()?) as usize)
}

/// One change to the zip structure itself.
pub fn damage(rng: &mut Rng, d: &mut Vec<u8>) {
    let find = |sig: &[u8; 4], d: &[u8]| -> Vec<usize> { d.windows(4).enumerate().filter(|(_, w)| w == sig).map(|(i, _)| i).collect() };
    match rng.below(7) {
        0 => {
            let at = rng.below(d.len());
            d.truncate(at);
        }
        1 | 2 => {
            // A field of a central directory entry.
            let cds = find(b"PK\x01\x02", d);
            if cds.is_empty() {
                return;
            }
            let p = *rng.pick(&cds);
            // method, crc, sizes, name length, extra length, offset.
            let (off, wide) = *rng.pick(&[(10usize, false), (16, true), (20, true), (24, true), (28, false), (30, false), (42, true)]);
            if wide {
                bytes::put(d, p + off, &rng.pick(bytes::U32S).to_le_bytes());
            } else {
                bytes::put(d, p + off, &rng.pick(bytes::U16S).to_le_bytes());
            }
        }
        3 => {
            // A field of a local header.
            let lhs = find(b"PK\x03\x04", d);
            if lhs.is_empty() {
                return;
            }
            let p = *rng.pick(&lhs);
            let (off, wide) = *rng.pick(&[(8usize, false), (18, true), (22, true), (26, false), (28, false)]);
            if wide {
                bytes::put(d, p + off, &rng.pick(bytes::U32S).to_le_bytes());
            } else {
                bytes::put(d, p + off, &rng.pick(bytes::U16S).to_le_bytes());
            }
        }
        4 => {
            // The end record: count, size and offset of the directory.
            let Some(&p) = find(b"PK\x05\x06", d).last() else { return };
            let (off, wide) = *rng.pick(&[(8usize, false), (10, false), (12, true), (16, true), (20, false)]);
            if wide {
                bytes::put(d, p + off, &rng.pick(bytes::U32S).to_le_bytes());
            } else {
                bytes::put(d, p + off, &rng.pick(bytes::U16S).to_le_bytes());
            }
        }
        5 => {
            // Compressed data corrupted inside an entry.
            let lhs = find(b"PK\x03\x04", d);
            if lhs.is_empty() {
                return;
            }
            let p = *rng.pick(&lhs);
            let (Some(n), Some(x)) = (u16_at(d, p + 26), u16_at(d, p + 28)) else { return };
            let start = p + 30 + n + x;
            if start + 4 < d.len() {
                let at = start + rng.below((d.len() - start).min(256));
                for i in 0..rng.small_len(8) {
                    if let Some(b) = d.get_mut(at + i) {
                        *b = rng.next() as u8;
                    }
                }
            }
        }
        _ => bytes::binary(rng, d),
    }
}

/// A mutated package: parts changed, written again, maybe damaged.
pub fn mutate(rng: &mut Rng, data: &[u8], other: Option<&[Part]>) -> Vec<u8> {
    let Some(mut ps) = parts(data) else {
        let mut d = data.to_vec();
        bytes::binary(rng, &mut d);
        return d;
    };
    let raw_only = rng.one_in(10);
    if !raw_only {
        for _ in 0..1 + rng.below(3) {
            mutate_parts(rng, &mut ps, other);
        }
    }
    let mut out = write(&ps, rng.one_in(4));
    if raw_only || rng.one_in(8) {
        damage(rng, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let ps = vec![
            Part { name: "[Content_Types].xml".into(), data: b"<Types><Override PartName=\"/a.xml\"/></Types>".to_vec() },
            Part { name: "_rels/.rels".into(), data: b"<Relationships><Relationship Id=\"rId1\" Target=\"a.xml\"/></Relationships>".to_vec() },
            Part { name: "a.xml".into(), data: b"<a><b c=\"1\">x</b><b/></a>".to_vec() },
        ];
        write(&ps, false)
    }

    #[test]
    fn parts_roundtrip() {
        let ps = parts(&sample()).unwrap();
        assert_eq!(ps.len(), 3);
        assert_eq!(ps[2].data, b"<a><b c=\"1\">x</b><b/></a>");
    }

    #[test]
    fn mutations_run() {
        let mut rng = Rng::new(5);
        let base = sample();
        let other = parts(&base).unwrap();
        let mut differ = 0;
        for _ in 0..300 {
            let m = mutate(&mut rng, &base, Some(&other));
            differ += (m != base) as usize;
        }
        assert!(differ > 200);
    }
}
