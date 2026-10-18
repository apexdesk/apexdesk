//! Notes (legacy comments + VML), hyperlinks and protection in .xlsx.

use std::collections::HashMap;
use std::fmt::Write as _;

use apex_sheet::protect::{EditRange, allow};
use apex_sheet::{CellRef, Link, Note, Password, Protection, Range, Sheet, Workbook};

use crate::xml::{Ev, HEADER, Reader, Tag, esc};
use crate::zip::ZipReader;

const REL_HYPERLINK: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
const REL_COMMENTS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
const REL_VML: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing";

/// Flags whose xlsx attribute defaults to "1" (blocked).
const BLOCKED_BY_DEFAULT: u32 = allow::FORMAT_CELLS
    | allow::FORMAT_COLUMNS
    | allow::FORMAT_ROWS
    | allow::INSERT_COLUMNS
    | allow::INSERT_ROWS
    | allow::INSERT_LINKS
    | allow::DELETE_COLUMNS
    | allow::DELETE_ROWS
    | allow::SORT
    | allow::AUTOFILTER
    | allow::PIVOT_TABLES;

/// A link's address and tip past these are cut (Excel keeps 2083 and 255).
const MAX_LINK_TARGET: usize = 8192;
const MAX_LINK_TIP: usize = 1024;
/// Bytes of link copies one sheet takes from a file.
pub(crate) const LINK_BUDGET: usize = 32 << 20;

/// The cells a link over `area` goes on, with their copies: its first
/// cells of a big area, while `budget` lasts.
pub(crate) fn spread_link(area: &Range, mut link: Link, budget: &mut usize) -> Vec<(CellRef, Link)> {
    fn cut(s: &mut String, n: usize) {
        if s.len() > n {
            let mut i = n;
            while !s.is_char_boundary(i) {
                i -= 1;
            }
            s.truncate(i);
        }
    }
    cut(&mut link.target, MAX_LINK_TARGET);
    cut(&mut link.tip, MAX_LINK_TIP);
    let cost = link.target.len() + link.tip.len() + 64;
    let mut out = Vec::new();
    'rows: for row in area.start.row..=area.end.row.min(area.start.row + 1000) {
        for col in area.start.col..=area.end.col.min(area.start.col + 100) {
            if *budget < cost {
                break 'rows;
            }
            *budget -= cost;
            out.push((CellRef::new(row, col), link.clone()));
        }
    }
    out
}

fn on(v: &str) -> bool {
    v == "1" || v == "true"
}

/// Passwords from `password` (legacy) and `algorithmName`/`hashValue`...
/// attributes, with an optional prefix (`workbook` in workbookProtection).
fn passwords(t: &Tag, prefix: &str) -> Vec<Password> {
    let a = |k: &str| t.attr(&if prefix.is_empty() { k.to_string() } else { format!("{prefix}{}{}", k[..1].to_uppercase(), &k[1..]) });
    let mut out = Vec::new();
    if let Some(h) = a("password").and_then(|v| u16::from_str_radix(v.trim(), 16).ok()).filter(|&h| h != 0) {
        out.push(Password::Legacy(h));
    }
    if let (Some(algorithm), Some(hash), Some(salt)) = (a("algorithmName"), a("hashValue"), a("saltValue")) {
        let spin = a("spinCount").and_then(|v| v.parse().ok()).unwrap_or(0);
        out.push(Password::Salted { algorithm: algorithm.into_owned(), hash: hash.into_owned(), salt: salt.into_owned(), spin });
    }
    out
}

pub fn read_sheet_protection(t: &Tag) -> Option<Protection> {
    if !t.attr("sheet").is_some_and(|v| on(&v)) {
        return None;
    }
    let mut allow = 0;
    for (flag, name) in allow::XLSX {
        let blocked = t.attr(name).map_or(BLOCKED_BY_DEFAULT & flag != 0, |v| on(&v));
        if !blocked {
            allow |= flag;
        }
    }
    Some(Protection { passwords: passwords(t, ""), allow })
}

pub fn read_workbook_protection(t: &Tag) -> Option<Protection> {
    t.attr("lockStructure").is_some_and(|v| on(&v)).then(|| Protection { passwords: passwords(t, "workbook"), allow: 0 })
}

fn password_attrs(s: &mut String, p: &Protection, prefix: &str) {
    let name = |k: &str| if prefix.is_empty() { k.to_string() } else { format!("{prefix}{}{}", k[..1].to_uppercase(), &k[1..]) };
    for pw in &p.passwords {
        match pw {
            Password::Legacy(h) => {
                let _ = write!(s, r#" {}="{:04X}""#, name("password"), h);
            }
            Password::Salted { algorithm, hash, salt, spin } => {
                let _ = write!(
                    s,
                    r#" {}="{}" {}="{}" {}="{}" {}="{}""#,
                    name("algorithmName"),
                    esc(algorithm),
                    name("hashValue"),
                    esc(hash),
                    name("saltValue"),
                    esc(salt),
                    name("spinCount"),
                    spin
                );
            }
            Password::Digest { .. } => {}
        }
    }
}

pub fn write_sheet_protection(s: &mut String, sh: &Sheet) {
    let Some(p) = &sh.protection else { return };
    s.push_str("<sheetProtection");
    password_attrs(s, p, "");
    s.push_str(r#" sheet="1""#);
    for (flag, name) in allow::XLSX {
        let blocked = !p.allows(flag);
        if blocked != (BLOCKED_BY_DEFAULT & flag != 0) {
            let _ = write!(s, r#" {name}="{}""#, blocked as u8);
        }
    }
    s.push_str("/>");
}

/// A `protectedRange` (Allow Edit Ranges): title, cells and password.
pub fn read_protected_range(t: &Tag) -> Option<EditRange> {
    let ranges = EditRange::parse_refs(&t.attr("sqref")?)?;
    Some(EditRange { title: t.attr("name").unwrap_or_default().into_owned(), ranges, passwords: passwords(t, ""), unlocked: false })
}

pub fn write_protected_ranges(s: &mut String, sh: &Sheet) {
    if sh.edit_ranges.is_empty() {
        return;
    }
    s.push_str("<protectedRanges>");
    for e in &sh.edit_ranges {
        let sqref = e.ranges.iter().map(|r| r.to_a1()).collect::<Vec<_>>().join(" ");
        s.push_str("<protectedRange");
        password_attrs(s, &Protection { passwords: e.passwords.clone(), allow: 0 }, "");
        let _ = write!(s, r#" sqref="{}" name="{}"/>"#, esc(&sqref), esc(&e.title));
    }
    s.push_str("</protectedRanges>");
}

pub fn write_workbook_protection(s: &mut String, wb: &Workbook) {
    let Some(p) = &wb.protection else { return };
    s.push_str("<workbookProtection");
    password_attrs(s, p, "workbook");
    s.push_str(r#" lockStructure="1"/>"#);
}

/// Relationships of a part: id -> (type, target, external).
pub fn read_rels(xml: &str) -> HashMap<String, (String, String, bool)> {
    let mut m = HashMap::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "Relationship" => {
                if let (Some(id), Some(target)) = (t.attr("Id"), t.attr("Target").filter(|t| t.len() <= crate::docx::MAX_REL_TARGET)) {
                    let external = t.attr("TargetMode").is_some_and(|v| v == "External");
                    m.insert(id.into_owned(), (t.attr("Type").unwrap_or_default().into_owned(), target.into_owned(), external));
                }
            }
            _ => {}
        }
    }
    m
}

/// Hyperlinks, notes and protection of a worksheet part at `path`.
pub fn read_sheet_extras(z: &ZipReader, path: &str, xml: &str, sh: &mut Sheet, persons: &HashMap<String, String>) -> Vec<(CellRef, Option<Note>, Option<Link>)> {
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    let rels = z.read_string(&format!("{dir}/_rels/{file}.rels")).map(|s| read_rels(&s)).unwrap_or_default();
    let mut out = Vec::new();
    let mut budget = LINK_BUDGET;
    let tail = xml.find("</sheetData>").map_or(xml, |i| &xml[i..]);
    let mut r = Reader::new(tail);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "sheetProtection" => sh.protection = read_sheet_protection(&t),
            Ev::Start(t) | Ev::Empty(t) if t.local() == "protectedRange" => sh.edit_ranges.extend(read_protected_range(&t)),
            Ev::Start(t) | Ev::Empty(t) if t.local() == "hyperlink" => {
                let Some(area) = t.attr("ref").and_then(|v| Range::parse(&v)) else { continue };
                let external = t.attr("r:id").or_else(|| t.attr("id")).and_then(|id| rels.get(id.as_ref())).map(|(_, target, _)| target.clone());
                let location = t.attr("location").map(|l| format!("#{l}"));
                let target = match (external, location) {
                    (Some(e), Some(l)) => e + &l,
                    (Some(e), None) => e,
                    (None, Some(l)) => l,
                    (None, None) => continue,
                };
                let tip = t.attr("tooltip").unwrap_or_default().into_owned();
                for (c, l) in spread_link(&area, Link { target, tip }, &mut budget) {
                    out.push((c, None, Some(l)));
                }
            }
            _ => {}
        }
    }
    let resolve = |ty: &str| rels.values().find(|(t, _, ext)| t == ty && !ext).map(|(_, target, _)| crate::xlsx::resolve_target(dir, target));
    let visible: std::collections::HashSet<CellRef> = resolve(REL_VML).and_then(|p| z.read_string(&p).ok()).map(|v| visible_notes(&v)).unwrap_or_default().into_iter().collect();
    if let Some(xml) = resolve(REL_COMMENTS).and_then(|p| z.read_string(&p).ok()) {
        for (c, mut note) in read_comments(&xml) {
            note.visible = visible.contains(&c);
            out.push((c, Some(note), None));
        }
    }
    // Threaded comments replace the classic note Excel writes for them.
    if let Some(xml) = resolve(crate::xlsx_threaded::REL_THREADED).and_then(|p| z.read_string(&p).ok()) {
        for (c, mut note) in crate::xlsx_threaded::read_threads(&xml, persons) {
            note.visible = visible.contains(&c);
            out.push((c, Some(note), None));
        }
    }
    out
}

fn read_comments(xml: &str) -> Vec<(CellRef, Note)> {
    let mut authors = Vec::new();
    let mut out = Vec::new();
    let mut r = Reader::new(xml);
    let mut cur: Option<(CellRef, String)> = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            // A name's worth (each comment copies its author's).
            Ev::Start(t) if t.local() == "author" => authors.push(r.text().chars().take(255).collect()),
            // A sheet's worth of comments.
            Ev::Start(t) if t.local() == "comment" && out.len() >= 1 << 18 => break,
            Ev::Start(t) if t.local() == "comment" => {
                cur = t.attr("ref").and_then(|v| CellRef::parse(&v)).map(|c| {
                    let a = t.attr("authorId").and_then(|v| v.parse::<usize>().ok()).and_then(|i| authors.get(i)).cloned().unwrap_or_default();
                    (c, a)
                });
            }
            Ev::Start(t) if t.local() == "text" => {
                let mut text = String::new();
                let mut depth = 1;
                let mut in_rph = false;
                while depth > 0 {
                    match r.next() {
                        Ev::Start(x) if x.local() == "t" && !in_rph => text.push_str(&r.text()),
                        Ev::Start(x) => {
                            in_rph |= x.local() == "rPh";
                            depth += 1;
                        }
                        Ev::End(x) => {
                            if crate::xml::local(x) == "rPh" {
                                in_rph = false;
                            }
                            depth -= 1;
                        }
                        Ev::Eof => break,
                        _ => {}
                    }
                }
                if let Some((c, author)) = cur.take() {
                    let text = without_author(&text, &author);
                    out.push((c, Note { text, author, ..Default::default() }));
                }
            }
            _ => {}
        }
    }
    out
}

/// Cells whose note VML shape is visible.
fn visible_notes(vml: &str) -> Vec<CellRef> {
    let mut out = Vec::new();
    let mut r = Reader::new(vml);
    let (mut visible, mut row, mut col) = (false, None, None);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "ClientData" => (visible, row, col) = (false, None, None),
            Ev::Start(t) | Ev::Empty(t) if t.local() == "Visible" => visible = true,
            Ev::Start(t) if t.local() == "Row" => row = r.text().trim().parse::<u32>().ok(),
            Ev::Start(t) if t.local() == "Column" => col = r.text().trim().parse::<u32>().ok(),
            Ev::End(n) if crate::xml::local(n) == "ClientData" => {
                if let (true, Some(row), Some(col)) = (visible, row, col) {
                    out.push(CellRef::new(row, col));
                }
            }
            _ => {}
        }
    }
    out
}

/// What a worksheet needs besides its own part.
#[derive(Default)]
pub struct SheetParts {
    /// `<hyperlinks>` (goes before pageMargins).
    pub hyperlinks: String,
    /// `<drawing>` (pictures; goes after the page setup).
    pub drawing: String,
    /// `<legacyDrawing>` (goes after pageMargins).
    pub legacy_drawing: String,
    /// `<oleObjects>` (goes after the picture).
    pub ole_objects: String,
    /// The worksheet's .rels part, if it has relationships.
    pub rels: Option<String>,
    pub comments: Option<String>,
    pub vml: Option<String>,
    pub threads: Option<String>,
}

/// `n` is the 1-based sheet number used to name the parts.
/// A legacy note's text without the "Author:" line Excel starts it with
/// (in .xlsx, .xlsb and .xls alike).
pub(crate) fn without_author(text: &str, author: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    match text.strip_prefix(author).and_then(|t| t.strip_prefix(':')) {
        Some(rest) if !author.is_empty() => rest.strip_prefix('\n').unwrap_or(rest).to_string(),
        _ => text,
    }
}

/// `authors` gathers the persons of threaded comments across sheets.
pub fn sheet_parts(sh: &Sheet, n: usize, authors: &mut Vec<String>) -> SheetParts {
    let mut p = SheetParts::default();
    let mut rels = String::new();
    let mut rid = 0;
    let mut notes: Vec<(CellRef, &Note)> = Vec::new();
    let mut links: Vec<(CellRef, &Link)> = Vec::new();
    for (c, cell) in sh.iter() {
        if let Some(x) = &cell.extra {
            if let Some(nt) = &x.note {
                notes.push((c, nt));
            }
            if let Some(l) = &x.link {
                links.push((c, l));
            }
        }
    }
    links.sort_by_key(|(c, _)| (c.row, c.col));
    notes.sort_by_key(|(c, _)| (c.row, c.col));
    if !links.is_empty() {
        p.hyperlinks.push_str("<hyperlinks>");
        for (c, l) in &links {
            let _ = write!(p.hyperlinks, r#"<hyperlink ref="{}""#, c.to_a1());
            let (url, place) = match l.target.split_once('#') {
                Some((u, pl)) => (u, Some(pl)),
                None => (l.target.as_str(), None),
            };
            if !url.is_empty() {
                rid += 1;
                let _ = write!(rels, r#"<Relationship Id="rId{rid}" Type="{REL_HYPERLINK}" Target="{}" TargetMode="External"/>"#, esc(url));
                let _ = write!(p.hyperlinks, r#" r:id="rId{rid}""#);
            }
            if let Some(pl) = place {
                let _ = write!(p.hyperlinks, r#" location="{}""#, esc(pl));
            }
            if !l.tip.is_empty() {
                let _ = write!(p.hyperlinks, r#" tooltip="{}""#, esc(&l.tip));
            }
            p.hyperlinks.push_str("/>");
        }
        p.hyperlinks.push_str("</hyperlinks>");
    }
    if !notes.is_empty() {
        let persons = authors;
        let mut list = String::new();
        let mut vml = String::from(
            r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout><v:shapetype id="_x0000_t202" coordsize="21600,21600" o:spt="202" path="m,l,21600r21600,l21600,xe"><v:stroke joinstyle="miter"/><v:path gradientshapeok="t" o:connecttype="rect"/></v:shapetype>"#,
        );
        let threaded: Vec<(CellRef, &Note)> = notes.iter().filter(|(_, nt)| nt.threaded).copied().collect();
        let mut legacy = Vec::new().into_iter();
        if !threaded.is_empty() {
            let (x, l) = crate::xlsx_threaded::write_threads(n, &threaded, persons);
            p.threads = Some(x);
            legacy = l.into_iter();
        }
        let mut names: Vec<String> = Vec::new();
        for (i, (c, nt)) in notes.iter().enumerate() {
            // A thread's note carries its id as author ("tc=") and a text
            // for older versions.
            let (author, text) = match nt.threaded.then(|| legacy.next()).flatten() {
                Some((id, shown)) => (format!("tc={id}"), shown),
                None => (nt.author.clone(), nt.text.clone()),
            };
            let a = match names.iter().position(|x| *x == author) {
                Some(a) => a,
                None => {
                    names.push(author);
                    names.len() - 1
                }
            };
            let _ = write!(list, r#"<comment ref="{}" authorId="{a}"><text><r><t xml:space="preserve">{}</t></r></text></comment>"#, c.to_a1(), esc(&text));
            let _ = write!(
                vml,
                r##"<v:shape id="_x0000_s{}" type="#_x0000_t202" style="position:absolute;margin-left:80pt;margin-top:2pt;width:108pt;height:60pt;z-index:{};visibility:{}" fillcolor="#ffffe1" o:insetmode="auto"><v:fill color2="#ffffe1"/><v:shadow on="t" color="black" obscured="t"/><v:path o:connecttype="none"/><v:textbox style="mso-direction-alt:auto"><div style="text-align:left"></div></v:textbox><x:ClientData ObjectType="Note"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor>{}, 15, {}, 2, {}, 15, {}, 16</x:Anchor><x:AutoFill>False</x:AutoFill><x:Row>{}</x:Row><x:Column>{}</x:Column>{}</x:ClientData></v:shape>"##,
                1025 + i,
                i + 1,
                if nt.visible { "visible" } else { "hidden" },
                c.col + 1,
                c.row.saturating_sub(1),
                c.col + 3,
                c.row + 3,
                c.row,
                c.col,
                if nt.visible { "<x:Visible/>" } else { "" }
            );
        }
        vml.push_str("</xml>");
        let mut cx = String::from(HEADER);
        cx.push_str(r#"<comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><authors>"#);
        for a in &names {
            let _ = write!(cx, "<author>{}</author>", esc(a));
        }
        let _ = write!(cx, "</authors><commentList>{list}</commentList></comments>");
        let _ = write!(rels, r#"<Relationship Id="rId{}" Type="{REL_COMMENTS}" Target="../comments{n}.xml"/>"#, rid + 1);
        let _ = write!(rels, r#"<Relationship Id="rId{}" Type="{REL_VML}" Target="../drawings/vmlDrawing{n}.vml"/>"#, rid + 2);
        p.legacy_drawing = format!(r#"<legacyDrawing r:id="rId{}"/>"#, rid + 2);
        if p.threads.is_some() {
            let _ = write!(rels, r#"<Relationship Id="rIdTC{n}" Type="{}" Target="../threadedComments/threadedComment{n}.xml"/>"#, crate::xlsx_threaded::REL_THREADED);
        }
        p.comments = Some(cx);
        p.vml = Some(vml);
    }
    if !sh.images.is_empty() || !sh.charts.is_empty() || sh.shapes.iter().any(|s| s.control.is_none()) {
        let id = rid + if p.comments.is_some() { 3 } else { 1 };
        let _ = write!(rels, r#"<Relationship Id="rId{id}" Type="{}" Target="../drawings/drawing{n}.xml"/>"#, crate::xlsx_drawing::REL_DRAWING);
        p.drawing = format!(r#"<drawing r:id="rId{id}"/>"#);
    }
    if !rels.is_empty() {
        p.rels = Some(format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#));
    }
    p
}

#[cfg(test)]
mod tests {
    use super::without_author;
    use apex_sheet::protect::{EditRange, allow};
    use apex_sheet::{CellRef, Link, Note, Protection, Workbook};

    fn sample() -> Workbook {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "Web");
        wb.set_input(0, CellRef::new(1, 0), "Go");
        wb.set_note(0, CellRef::new(0, 0), Some(Note { text: "First line\nsecond & <third>".into(), author: "Ana".into(), ..Default::default() }));
        wb.set_note(0, CellRef::new(3, 2), Some(Note { text: "Shown".into(), author: "Bo".into(), visible: true, ..Default::default() }));
        wb.set_link(0, CellRef::new(0, 0), Some(Link { target: "https://example.com/?a=1&b=2".into(), tip: "Example".into() }));
        wb.set_link(0, CellRef::new(1, 0), Some(Link { target: "#Sheet2!B3".into(), tip: String::new() }));
        wb.sheets.push(apex_sheet::Sheet::new("Sheet2"));
        let id = wb.styles.intern(apex_sheet::CellStyle { unlocked: true, ..Default::default() });
        wb.load_cell(1, CellRef::new(0, 0), apex_sheet::Content::Empty, id);
        wb.set_input(1, CellRef::new(1, 0), "=1+1");
        wb.modify_style(1, &apex_sheet::Range::single(CellRef::new(1, 0)), |s| s.formula_hidden = true);
        let r = |a: &str| apex_sheet::Range::parse(a).unwrap();
        wb.set_edit_ranges(1, vec![EditRange::new("Prices & <tax>", vec![r("C3:D9"), r("F2")], "secret"), EditRange::new("Open", vec![r("H1:H4")], "")]);
        wb.set_protection(Some(1), Some(Protection::with_password("pw", allow::DEFAULT | allow::SORT)));
        wb.set_protection(None, Some(Protection::default()));
        wb
    }

    fn check(back: &Workbook, xlsx: bool) {
        let a1 = back.sheets[0].get(CellRef::new(0, 0)).unwrap();
        assert_eq!(a1.note().unwrap().text, "First line\nsecond & <third>");
        assert_eq!(a1.note().unwrap().author, "Ana");
        assert!(!a1.note().unwrap().visible);
        assert_eq!(a1.link().unwrap().target, "https://example.com/?a=1&b=2");
        assert_eq!(a1.link().unwrap().tip, "Example");
        assert_eq!(back.sheets[0].get(CellRef::new(1, 0)).unwrap().link().unwrap().place(), Some("Sheet2!B3"));
        let c4 = back.sheets[0].get(CellRef::new(3, 2)).unwrap().note().unwrap();
        assert!(c4.visible && c4.text == "Shown");
        let p = back.sheets[1].protection.as_ref().unwrap();
        assert!(p.unlocks("pw") && !p.unlocks("nope"));
        if xlsx {
            assert!(p.allows(allow::SORT));
            // Allow Edit Ranges (ods has nothing like them).
            let e = &back.sheets[1].edit_ranges;
            assert_eq!(e.len(), 2);
            assert_eq!((e[0].title.as_str(), e[0].refers_to().as_str()), ("Prices & <tax>", "C3:D9,F2"));
            assert!(e[0].has_password() && e[0].unlocks("secret") && !e[0].unlocks("x"));
            assert!(!e[1].has_password());
            assert!(!back.cell_locked(1, CellRef::new(2, 7)));
            assert_eq!(back.edit_range_asking(1, CellRef::new(4, 2)), Some(0));
        }
        assert!(p.allows(allow::SELECT_LOCKED) && !p.allows(allow::DELETE_ROWS));
        assert!(back.protection.is_some());
        assert!(!back.cell_locked(1, CellRef::new(0, 0)));
        assert!(back.cell_locked(1, CellRef::new(5, 5)));
        assert!(back.formula_hidden(1, CellRef::new(1, 0)));
        assert!(back.sheets[0].protection.is_none());
    }

    #[test]
    fn author_line() {
        // .xls notes (TXO text) and .xlsx ones start with the author.
        assert_eq!(without_author("Yegor Kozlov:\rfirst cell", "Yegor Kozlov"), "first cell");
        assert_eq!(without_author("Ana:\r\nx\r\ny", "Ana"), "x\ny");
        assert_eq!(without_author("Ana: x", "Ana"), " x");
        assert_eq!(without_author("Other:\nx", "Ana"), "Other:\nx");
        assert_eq!(without_author(":\nx", ""), ":\nx");
    }

    #[test]
    fn notes_links_protection_roundtrip() {
        let wb = sample();
        // APEX_DUMP=dir keeps the files, to open them in other programs.
        if let Ok(dir) = std::env::var("APEX_DUMP") {
            let _ = apex_fs::write(format!("{dir}/notes.xlsx"), crate::xlsx::write(&wb));
            let _ = apex_fs::write(format!("{dir}/notes.ods"), crate::ods::write(&wb));
        }
        check(&crate::xlsx::read(&crate::xlsx::write(&wb)).unwrap(), true);
        check(&crate::ods::read(&crate::ods::write(&wb)).unwrap(), false);
    }

    #[test]
    fn threaded_comments_roundtrip() {
        let mut wb = Workbook::new();
        let thread = Note {
            text: "Is this right?".into(),
            author: "Ana".into(),
            threaded: true,
            resolved: true,
            date: "2024-05-01T10:30:00".into(),
            replies: vec![apex_sheet::Reply { text: "Yes & no".into(), author: "Bo".into(), date: "2024-05-02T09:00:00".into() }],
            ..Default::default()
        };
        wb.set_note(0, CellRef::new(1, 1), Some(thread.clone()));
        wb.set_note(0, CellRef::new(2, 1), Some(Note { text: "Classic".into(), author: "Cy".into(), ..Default::default() }));
        let data = crate::xlsx::write(&wb);
        let back = crate::xlsx::read(&data).unwrap();
        assert_eq!(back.sheets[0].get(CellRef::new(1, 1)).unwrap().note().unwrap(), &thread);
        let classic = back.sheets[0].get(CellRef::new(2, 1)).unwrap().note().unwrap();
        assert!(!classic.threaded && classic.text == "Classic" && classic.author == "Cy");
        // Readers without threads see the text in the classic note.
        let z = crate::zip::ZipReader::new(&data).unwrap();
        let legacy = z.read_string("xl/comments1.xml").unwrap();
        assert!(legacy.contains("Is this right?") && legacy.contains("Yes &amp; no"));
        let ods = crate::ods::read(&crate::ods::write(&wb)).unwrap();
        let n = ods.sheets[0].get(CellRef::new(1, 1)).unwrap().note().unwrap().clone();
        assert_eq!(n.text, "Is this right?\n\nBo: Yes & no");
        assert_eq!(n.date, "2024-05-01T10:30:00");
    }

    #[test]
    fn excel_protected_ranges() {
        let xml = r#"<protectedRanges><protectedRange password="CBEB" sqref="A1:B2 $D$4" name="Legacy"/><protectedRange algorithmName="SHA-512" hashValue="h" saltValue="s" spinCount="100000" sqref="E1" name="New"/><protectedRange sqref="bad!" name="Broken"/></protectedRanges>"#;
        let mut r = crate::xml::Reader::new(xml);
        let mut got = Vec::new();
        loop {
            match r.next() {
                crate::xml::Ev::Eof => break,
                crate::xml::Ev::Empty(t) => got.extend(super::read_protected_range(&t)),
                _ => {}
            }
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].refers_to(), "A1:B2,D4");
        assert!(got[0].unlocks("test") && !got[0].unlocks("tost"));
        assert!(matches!(&got[1].passwords[0], apex_sheet::Password::Salted { spin: 100000, .. }));
        let mut sh = apex_sheet::Sheet::new("S");
        sh.edit_ranges = got;
        let mut out = String::new();
        super::write_protected_ranges(&mut out, &sh);
        assert!(out.starts_with(r#"<protectedRanges><protectedRange password="CBEB" sqref="A1:B2 D4" name="Legacy"/>"#), "{out}");
        assert!(out.contains(r#"algorithmName="SHA-512" hashValue="h" saltValue="s" spinCount="100000" sqref="E1""#));
    }

    #[test]
    fn excel_protection_attrs() {
        let t = r#"<sheetProtection password="CBEB" sheet="1" objects="1" scenarios="1" sort="0" selectLockedCells="1"/>"#;
        let mut r = crate::xml::Reader::new(t);
        let crate::xml::Ev::Empty(tag) = r.next() else { panic!() };
        let p = super::read_sheet_protection(&tag).unwrap();
        assert!(p.unlocks("test") && !p.unlocks("x"));
        assert!(p.allows(allow::SORT) && !p.allows(allow::SELECT_LOCKED) && p.allows(allow::SELECT_UNLOCKED) && !p.allows(allow::OBJECTS));
    }
}
