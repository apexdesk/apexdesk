//! Links to other workbooks (`xl/externalLinks/externalLinkN.xml`): the
//! linked file, its sheet and name lists and the values formulas used.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use apex_sheet::{CellRef, ErrorKind, ExternalBook, Value, Workbook};

use crate::xml::{Ev, HEADER, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

const REL_LINK: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink";
const REL_PATH: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLinkPath";
pub(crate) const CT_LINK: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml";

/// The linked books in the order of `<externalReferences>` (formulas call
/// the first one `[1]`).
pub(crate) fn read(z: &ZipReader, rels: &HashMap<String, String>, wbxml: &str) -> Vec<ExternalBook> {
    let mut ids = Vec::new();
    let mut r = Reader::new(wbxml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "externalReference" => {
                ids.push(t.attr("r:id").or_else(|| t.attr("id")).unwrap_or_default().into_owned());
            }
            _ => {}
        }
    }
    ids.iter()
        .map(|id| {
            let Some(target) = rels.get(id) else { return ExternalBook::default() };
            let path = crate::xlsx::resolve_target("xl", target);
            let (dir, file) = path.rsplit_once('/').unwrap_or(("", &path));
            let link_rels = z.read_string(&format!("{dir}/_rels/{file}.rels")).map(|s| crate::xlsx::read_rels(&s)).unwrap_or_default();
            z.read_string(&path).map(|xml| read_book(&xml, &link_rels)).unwrap_or_default()
        })
        .collect()
}

fn read_book(xml: &str, rels: &HashMap<String, String>) -> ExternalBook {
    let mut b = ExternalBook::default();
    let mut r = Reader::new(xml);
    let mut sheet = 0usize;
    loop {
        let (t, open) = match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => (t, true),
            Ev::Empty(t) => (t, false),
            _ => continue,
        };
        {
            match t.local() {
                "externalBook" => {
                    if let Some(target) = t.attr("r:id").or_else(|| t.attr("id")).and_then(|id| rels.get(id.as_ref())) {
                        b.target = target.clone();
                    }
                }
                "sheetName" => b.sheets.push(t.attr("val").unwrap_or_default().into_owned()),
                "definedName" => {
                    let local = t.attr("sheetId").and_then(|v| v.trim().parse().ok());
                    b.names.push((t.attr("name").unwrap_or_default().into_owned(), local, t.attr("refersTo").unwrap_or_default().into_owned()));
                }
                "sheetData" => {
                    sheet = t.attr("sheetId").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                }
                "cell" => {
                    let Some(at) = t.attr("r").and_then(|v| CellRef::parse(&v)) else { continue };
                    let ty = t.attr("t").unwrap_or_default().into_owned();
                    let mut v = None;
                    if open {
                        loop {
                            match r.next() {
                                Ev::Start(x) if x.local() == "v" => v = Some(r.text()),
                                // `<v/>`: an empty text ("" from a formula).
                                Ev::Empty(x) if x.local() == "v" => v = Some(String::new()),
                                Ev::End(x) if crate::xml::local(x) == "cell" => break,
                                Ev::Eof => break,
                                _ => {}
                            }
                        }
                    }
                    let Some(v) = v else { continue };
                    let value = match ty.as_str() {
                        "str" | "s" | "inlineStr" => Value::text(v),
                        "b" => Value::Bool(v.trim() == "1" || v.trim() == "true"),
                        "e" => Value::Error(ErrorKind::parse(v.trim()).unwrap_or(ErrorKind::Na)),
                        _ => v.trim().parse().map(Value::Number).unwrap_or(Value::Empty),
                    };
                    // Sheet numbers index the names listed (a few).
                    if sheet > b.sheets.len().max(256) {
                        continue;
                    }
                    if b.cells.len() <= sheet {
                        b.cells.resize(sheet + 1, BTreeMap::new());
                    }
                    b.cells[sheet].insert(at, value);
                }
                _ => {}
            }
        }
    }
    b.cells.resize(b.sheets.len().max(b.cells.len()), BTreeMap::new());
    b
}

/// Writes the link parts; returns the `<externalReferences>` element for
/// workbook.xml, the workbook relationships and the content types.
pub(crate) fn write(wb: &Workbook, z: &mut ZipWriter, first_rid: usize) -> (String, String, String) {
    let (mut refs, mut rels, mut ct) = (String::new(), String::new(), String::new());
    if wb.externals.is_empty() {
        return (refs, rels, ct);
    }
    refs.push_str("<externalReferences>");
    for (i, b) in wb.externals.iter().enumerate() {
        let k = i + 1;
        let rid = first_rid + i;
        let _ = write!(refs, r#"<externalReference r:id="rId{rid}"/>"#);
        let _ = write!(rels, r#"<Relationship Id="rId{rid}" Type="{REL_LINK}" Target="externalLinks/externalLink{k}.xml"/>"#);
        let _ = write!(ct, r#"<Override PartName="/xl/externalLinks/externalLink{k}.xml" ContentType="{CT_LINK}"/>"#);
        let mut x = String::from(HEADER);
        x.push_str(r#"<externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><externalBook r:id="rId1">"#);
        if !b.sheets.is_empty() {
            x.push_str("<sheetNames>");
            for s in &b.sheets {
                let _ = write!(x, r#"<sheetName val="{}"/>"#, esc(s));
            }
            x.push_str("</sheetNames>");
        }
        if !b.names.is_empty() {
            x.push_str("<definedNames>");
            for (name, local, refers) in &b.names {
                let _ = write!(x, r#"<definedName name="{}" refersTo="{}""#, esc(name), esc(refers));
                if let Some(s) = local {
                    let _ = write!(x, r#" sheetId="{s}""#);
                }
                x.push_str("/>");
            }
            x.push_str("</definedNames>");
        }
        x.push_str("<sheetDataSet>");
        for (si, cells) in b.cells.iter().enumerate() {
            if cells.is_empty() {
                let _ = write!(x, r#"<sheetData sheetId="{si}"/>"#);
                continue;
            }
            let _ = write!(x, r#"<sheetData sheetId="{si}">"#);
            let mut row = None;
            for (c, v) in cells {
                if row != Some(c.row) {
                    if row.is_some() {
                        x.push_str("</row>");
                    }
                    let _ = write!(x, r#"<row r="{}">"#, c.row + 1);
                    row = Some(c.row);
                }
                let (t, text) = match v {
                    Value::Number(n) => ("", n.to_string()),
                    Value::Bool(b) => (r#" t="b""#, if *b { "1" } else { "0" }.to_string()),
                    Value::Error(k) => (r#" t="e""#, k.as_str().to_string()),
                    Value::Text(s) => (r#" t="str""#, s.to_string()),
                    _ => continue,
                };
                let _ = write!(x, r#"<cell r="{}"{t}><v>{}</v></cell>"#, c.to_a1(), esc(&text));
            }
            if row.is_some() {
                x.push_str("</row>");
            }
            x.push_str("</sheetData>");
        }
        x.push_str("</sheetDataSet></externalBook></externalLink>");
        z.add(&format!("xl/externalLinks/externalLink{k}.xml"), x.as_bytes(), true);
        let r = format!(
            r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL_PATH}" Target="{}" TargetMode="External"/></Relationships>"#,
            esc(&b.target)
        );
        z.add(&format!("xl/externalLinks/_rels/externalLink{k}.xml.rels"), r.as_bytes(), true);
    }
    refs.push_str("</externalReferences>");
    (refs, rels, ct)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_texts_are_kept() {
        let xml = r#"<externalLink><externalBook><sheetNames><sheetName val="S"/></sheetNames><sheetDataSet><sheetData sheetId="0"><row r="1"><cell r="A1" t="str"><v/></cell><cell r="B1"><v>2</v></cell></row></sheetData></sheetDataSet></externalBook></externalLink>"#;
        let b = read_book(xml, &HashMap::new());
        assert_eq!(b.cells[0].get(&CellRef::new(0, 0)), Some(&Value::text("")));
        assert_eq!(b.cells[0].get(&CellRef::new(0, 1)), Some(&Value::Number(2.0)));
    }
}
