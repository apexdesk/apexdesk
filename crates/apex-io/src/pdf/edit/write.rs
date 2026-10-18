//! Writing the objects back: their syntax, incremental updates (the
//! changed objects after the file, with a cross-reference section of the
//! same kind as the file's last one) and whole files.

use std::sync::Arc;

use super::super::obj::{Dict, Obj};
use super::PdfDoc;

/// A stream compressed with Flate.
pub fn deflate(mut d: Dict, data: &[u8]) -> Obj {
    let z = miniz_oxide::deflate::compress_to_vec_zlib(data, 6);
    d.set("Filter", Obj::name_of("FlateDecode"));
    d.remove("DecodeParms");
    Obj::Stream(d, Arc::from(z))
}

fn real(v: f64, out: &mut Vec<u8>) {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        out.extend_from_slice(format!("{}", v as i64).as_bytes());
        return;
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    out.extend_from_slice(if s == "-0" || s.is_empty() { b"0" } else { s.as_bytes() });
}

fn name(n: &str, out: &mut Vec<u8>) {
    out.push(b'/');
    for &b in n.as_bytes() {
        if b <= b' ' || b >= 0x7F || b == b'#' || super::super::obj::is_delim(b) {
            out.extend_from_slice(format!("#{b:02X}").as_bytes());
        } else {
            out.push(b);
        }
    }
}

fn string(s: &[u8], out: &mut Vec<u8>) {
    let binary = s.iter().filter(|&&b| b < 0x20 && !matches!(b, b'\n' | b'\r' | b'\t') || b >= 0x7F).count();
    if binary * 4 > s.len() {
        out.push(b'<');
        for b in s {
            out.extend_from_slice(format!("{b:02X}").as_bytes());
        }
        out.push(b'>');
        return;
    }
    out.push(b'(');
    for &b in s {
        match b {
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(b);
            }
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b if b < 0x20 || b >= 0x7F => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            b => out.push(b),
        }
    }
    out.push(b')');
}

fn dict(d: &Dict, out: &mut Vec<u8>, skip_length: bool) {
    out.extend_from_slice(b"<<");
    for (k, v) in &d.0 {
        if skip_length && &**k == "Length" {
            continue;
        }
        name(k, out);
        out.push(b' ');
        write_obj(v, out);
    }
    out.extend_from_slice(b">>");
}

/// An object's syntax.
pub fn write_obj(o: &Obj, out: &mut Vec<u8>) {
    match o {
        Obj::Null => out.extend_from_slice(b"null"),
        Obj::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Obj::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Obj::Real(r) => real(*r, out),
        Obj::Name(n) => name(n, out),
        Obj::Str(s) => string(s, out),
        Obj::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_obj(x, out);
            }
            out.push(b']');
        }
        Obj::Dict(d) => dict(d, out, false),
        Obj::Ref(n, g) => out.extend_from_slice(format!("{n} {g} R").as_bytes()),
        Obj::Stream(d, data) => {
            let mut d2 = d.clone();
            d2.set("Length", Obj::Int(data.len() as i64));
            dict(&d2, out, false);
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(data);
            out.extend_from_slice(b"\nendstream");
        }
    }
}

/// Appends object `n` to `out`, encrypted when the file is.
fn object(doc: &PdfDoc, n: u32, g: u16, o: &Obj, out: &mut Vec<u8>) {
    out.extend_from_slice(format!("{n} {g} obj\n").as_bytes());
    match &doc.crypt {
        Some(c) => write_obj(&c.encrypt(n, g, o), out),
        None => write_obj(o, out),
    }
    out.extend_from_slice(b"\nendobj\n");
}

/// The trailer's entries for a new section.
fn trailer_of(doc: &PdfDoc, size: u32) -> Dict {
    let mut t = Dict::default();
    t.set("Size", Obj::Int(size as i64));
    for k in ["Root", "Info", "Encrypt"] {
        if let Some(v) = doc.trailer.get(k) {
            t.set(k, v.clone());
        }
    }
    let id = match doc.trailer.get("ID") {
        Some(Obj::Array(a)) if !a.is_empty() => a[0].clone(),
        _ => Obj::Str(crate::crypto::random(16)),
    };
    // The second half of the ID changes with each revision (the first
    // one keys the encryption).
    t.set("ID", Obj::Array(vec![id, Obj::Str(crate::crypto::random(16))]));
    t
}

/// The base file with the changed objects appended; also where the new
/// cross-reference section starts.
pub fn incremental(doc: &PdfDoc) -> (Vec<u8>, usize) {
    let (prev, stream) = doc.base_xref.unwrap_or((0, false));
    let mut out = doc.base.to_vec();
    if out.last() != Some(&b'\n') {
        out.push(b'\n');
    }
    let mut entries: Vec<(u32, u16, usize)> = Vec::new();
    let mut freed: Vec<u32> = Vec::new();
    for &n in &doc.changed {
        match doc.objs.get(&n) {
            Some((g, o)) => {
                entries.push((n, *g, out.len()));
                object(doc, n, *g, o, &mut out);
            }
            None => freed.push(n),
        }
    }
    let size = doc.max_n().max(freed.iter().copied().max().unwrap_or(0)) + 1;
    let mut t = trailer_of(doc, size);
    t.set("Prev", Obj::Int(prev as i64));
    if stream {
        // A cross-reference stream: type 1 rows (and type 0 for freed).
        let xn = size;
        let at = out.len();
        entries.push((xn, 0, at));
        let mut rows: Vec<(u32, u8, u32, u16)> = entries.iter().map(|&(n, g, off)| (n, 1u8, off as u32, g)).collect();
        rows.extend(freed.iter().map(|&n| (n, 0u8, 0u32, 1u16)));
        rows.sort_by_key(|r| r.0);
        let mut index = Vec::new();
        let mut data = Vec::new();
        let mut i = 0;
        while i < rows.len() {
            let mut j = i;
            while j + 1 < rows.len() && rows[j + 1].0 == rows[j].0 + 1 {
                j += 1;
            }
            index.push(Obj::Int(rows[i].0 as i64));
            index.push(Obj::Int((j - i + 1) as i64));
            for r in &rows[i..=j] {
                data.push(r.1);
                data.extend_from_slice(&r.2.to_be_bytes());
                data.extend_from_slice(&r.3.to_be_bytes());
            }
            i = j + 1;
        }
        t.set("Size", Obj::Int(xn as i64 + 1));
        t.set("Type", Obj::name_of("XRef"));
        t.set("W", Obj::Array(vec![Obj::Int(1), Obj::Int(4), Obj::Int(2)]));
        t.set("Index", Obj::Array(index));
        out.extend_from_slice(format!("{xn} 0 obj\n").as_bytes());
        write_obj(&Obj::Stream(t, Arc::from(data)), &mut out);
        out.extend_from_slice(b"\nendobj\n");
        out.extend_from_slice(format!("startxref\n{at}\n%%EOF\n").as_bytes());
        return (out, at);
    }
    let at = out.len();
    let mut rows: Vec<(u32, String)> = entries.iter().map(|&(n, g, off)| (n, format!("{off:010} {g:05} n \n"))).collect();
    rows.extend(freed.iter().map(|&n| (n, "0000000000 00001 f \n".to_string())));
    rows.sort_by_key(|r| r.0);
    out.extend_from_slice(b"xref\n");
    let mut i = 0;
    while i < rows.len() {
        let mut j = i;
        while j + 1 < rows.len() && rows[j + 1].0 == rows[j].0 + 1 {
            j += 1;
        }
        out.extend_from_slice(format!("{} {}\n", rows[i].0, j - i + 1).as_bytes());
        for r in &rows[i..=j] {
            out.extend_from_slice(r.1.as_bytes());
        }
        i = j + 1;
    }
    out.extend_from_slice(b"trailer\n");
    write_obj(&Obj::Dict(t), &mut out);
    out.extend_from_slice(format!("\nstartxref\n{at}\n%%EOF\n").as_bytes());
    (out, at)
}

/// A whole new file with the objects reachable from the trailer and a
/// classic cross-reference table.
pub fn whole(doc: &PdfDoc) -> Vec<u8> {
    let keep = doc.reachable();
    let version = doc.base.get(5..8).filter(|v| v[0].is_ascii_digit() && v[1] == b'.' && v[2].is_ascii_digit()).map_or("1.7".to_string(), |v| String::from_utf8_lossy(v).into_owned());
    let version = if version.as_str() < "1.4" { "1.4".to_string() } else { version };
    let mut out = format!("%PDF-{version}\n%\u{E2}\u{E3}\u{CF}\u{D3}\n").into_bytes();
    let mut offs: Vec<(u32, u16, usize)> = Vec::new();
    for &n in &keep {
        let Some((g, o)) = doc.objs.get(&n) else { continue };
        offs.push((n, *g, out.len()));
        object(doc, n, *g, o, &mut out);
    }
    let size = offs.last().map_or(1, |o| o.0 + 1);
    let at = out.len();
    out.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
    let present: std::collections::HashSet<u32> = offs.iter().map(|o| o.0).collect();
    // Free entries link to the next free one (0 ends the list).
    let free: Vec<u32> = (0..size).filter(|n| *n == 0 || !present.contains(n)).collect();
    let mut next_free = free.iter().skip(1);
    let mut next = offs.iter().peekable();
    for n in 0..size {
        if let Some(&&(m, g, off)) = next.peek()
            && m == n
        {
            out.extend_from_slice(format!("{off:010} {g:05} n \n").as_bytes());
            next.next();
        } else {
            let link = next_free.next().copied().unwrap_or(0);
            out.extend_from_slice(format!("{link:010} {:05} f \n", if n == 0 { 65535 } else { 1 }).as_bytes());
        }
    }
    let t = trailer_of(doc, size);
    out.extend_from_slice(b"trailer\n");
    write_obj(&Obj::Dict(t), &mut out);
    out.extend_from_slice(format!("\nstartxref\n{at}\n%%EOF\n").as_bytes());
    out
}
