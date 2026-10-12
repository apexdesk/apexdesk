//! What a save must not lose, as plain strings: a digest of a workbook
//! or document (lists that `diff` compares in order), and the cells of
//! two workbooks compared one by one (too many to keep as strings).

use std::collections::{BTreeMap, HashMap};

use apex_doc::{Document, EmbedKind, OBJECT, Paragraph};
use apex_sheet::{Cell, Content, Locale, Sheet, Value, Workbook};

use super::diff::Tally;
use super::result::Diff;

#[derive(Debug, Default)]
pub struct Digest {
    /// Compared in order: kind → items.
    pub lists: BTreeMap<&'static str, Vec<String>>,
}

impl Digest {
    fn push(&mut self, kind: &'static str, item: String) {
        self.lists.entry(kind).or_default().push(item);
    }
}

/// A number with the noise of float conversions rounded away.
pub fn num(n: f64) -> String {
    let r: f64 = format!("{n:.11e}").parse().unwrap_or(n);
    format!("{r}")
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Empty => String::new(),
        Value::Number(n) => num(*n),
        Value::Text(t) => t.to_string(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Error(e) => format!("#{e:?}"),
        Value::Array(_) => "{array}".into(),
        Value::Lambda(_) => "{lambda}".into(),
    }
}

/// The digest of a workbook, all but its cells (see [`cells`]).
pub fn sheet(wb: &Workbook) -> Digest {
    let mut d = Digest::default();
    let loc = Locale::EN;
    for sh in &wb.sheets {
        d.push("sheets", format!("{}{}", sh.name, if sh.hidden { " (hidden)" } else { "" }));
        for m in &sh.merges {
            d.push("merges", format!("{}!{}", sh.name, m.to_a1()));
        }
        for img in &sh.images {
            d.push("images", format!("{}: {} bytes", sh.name, img.data.len()));
        }
        for c in &sh.charts {
            d.push("charts", format!("{}: {:?}", sh.name, c.kind));
        }
        for _ in &sh.shapes {
            d.push("shapes", sh.name.clone());
        }
        for t in &sh.tables {
            d.push("tables", format!("{}!{} {}", sh.name, t.name, t.range.to_a1()));
        }
        if !sh.validations.is_empty() {
            d.push("validations", format!("{}: {}", sh.name, sh.validations.len()));
        }
        if !sh.cond_formats.is_empty() {
            d.push("conditional formats", format!("{}: {}", sh.name, sh.cond_formats.len()));
        }
    }
    // Merged areas have no order of their own.
    if let Some(m) = d.lists.get_mut("merges") {
        m.sort();
    }
    let mut names: Vec<String> =
        wb.names.iter().map(|n| format!("{}{} = {}", n.scope.as_deref().map(|s| format!("{s}!")).unwrap_or_default(), n.name, n.expr.to_display(&loc))).collect();
    names.sort();
    d.lists.insert("names", names);
    if wb.macros.has_code() {
        d.push("macros", "code".into());
    }
    d
}

/// What is compared of each cell.
const CELL_FACTS: [&str; 5] = ["cells.value", "cells.formula", "styles.cell", "comments", "links"];

/// The facts of a cell in the order of `CELL_FACTS` (value "" when the
/// formula is volatile: NOW, RAND... give another value each time).
fn cell_facts(wb: &Workbook, cell: &Cell, values_only: bool, styles: &mut HashMap<u32, String>) -> [String; 5] {
    let formula = cell.formula().map(|f| f.expr().to_display(&Locale::EN)).unwrap_or_default();
    let value = match &cell.content {
        Content::Spill(_) if !values_only => String::new(),
        _ if is_volatile(&formula) => String::new(),
        _ => value_text(cell.value()),
    };
    if values_only {
        return [value, String::new(), String::new(), String::new(), String::new()];
    }
    let style = styles.entry(cell.style).or_insert_with(|| style_key(wb, cell.style)).clone();
    let note = cell.note().map(|n| n.flat_text()).unwrap_or_default();
    let link = cell.link().map(|l| l.target.clone()).unwrap_or_default();
    [value, formula, style, note, link]
}

/// Cells of `a` and `b` compared one by one, on sheets of the same name.
/// `values_only`: the first sheet's values, all that CSV keeps.
pub fn cells(a: &Workbook, b: &Workbook, values_only: bool) -> Vec<Diff> {
    let mut tally = Tally::default();
    let (mut sa, mut sb) = (HashMap::new(), HashMap::new());
    let n = if values_only { 1 } else { a.sheets.len() };
    let empty: [String; 5] = Default::default();
    for sh in a.sheets.iter().take(n) {
        // A sheet missing from `b` is a difference of "sheets".
        let other: Option<&Sheet> = if values_only { b.sheets.first() } else { b.sheets.iter().find(|s| s.name == sh.name) };
        let Some(other) = other else { continue };
        for (r, cell) in sh.iter() {
            let fa = cell_facts(a, cell, values_only, &mut sa);
            let fb = other.get(r).map(|c| cell_facts(b, c, values_only, &mut sb)).unwrap_or_default();
            for (k, kind) in CELL_FACTS.iter().enumerate() {
                tally.add(kind, || format!("{}!{}", sh.name, r.to_a1()), &fa[k], &fb[k]);
            }
        }
        for (r, cell) in other.iter() {
            if sh.get(r).is_some() {
                continue;
            }
            let fb = cell_facts(b, cell, values_only, &mut sb);
            for (k, kind) in CELL_FACTS.iter().enumerate() {
                tally.add(kind, || format!("{}!{}", sh.name, r.to_a1()), &empty[k], &fb[k]);
            }
        }
    }
    tally.diffs()
}

/// Whether a formula calls a function whose result changes on every
/// calculation.
fn is_volatile(formula: &str) -> bool {
    let f = formula.to_ascii_uppercase();
    ["NOW(", "TODAY(", "RAND(", "RANDBETWEEN(", "RANDARRAY(", "OFFSET(", "INDIRECT(", "CELL(", "INFO("].iter().any(|n| f.contains(n))
}

/// What a cell format looks like, in a few words.
fn style_key(wb: &Workbook, id: u32) -> String {
    let s = wb.styles.get(id);
    let mut k = Vec::new();
    for (on, w) in [(s.bold, "bold"), (s.italic, "italic"), (s.underline, "underline"), (s.strike, "strike"), (s.wrap, "wrap")] {
        if on {
            k.push(w.to_string());
        }
    }
    if let Some(f) = &s.font {
        k.push(format!("font {f}"));
    }
    if s.font_size != 0 {
        k.push(format!("size {}", s.font_size));
    }
    if let Some(c) = s.color {
        k.push(format!("color {c:06x}"));
    }
    if let Some(c) = s.fill {
        k.push(format!("fill {c:06x}"));
    }
    if !s.num_fmt.is_empty() && !s.num_fmt.eq_ignore_ascii_case("general") {
        k.push(format!("format {}", s.num_fmt));
    }
    if s.halign != Default::default() || s.valign != Default::default() {
        k.push(format!("{:?}/{:?}", s.halign, s.valign));
    }
    if !s.borders.is_empty() {
        k.push(format!("borders {:x}", s.borders.flags()));
    }
    k.join(" ")
}

/// The digest of a document.
/// Slides: their text, objects and notes.
pub fn slides(deck: &apex_slides::Deck) -> Digest {
    let mut d = Digest::default();
    for (i, s) in deck.slides.iter().enumerate() {
        d.push("slides", format!("{}{}", i + 1, if s.hidden { " (hidden)" } else { "" }));
        for o in &s.objects {
            o.walk(&mut |x| d.push("objects", format!("{}: {}", i + 1, x.name)));
            for t in o.text_bodies() {
                d.push("text", t.text());
            }
        }
        if let Some(n) = &s.notes {
            d.push("notes", n.text.text());
        }
    }
    d
}

pub fn doc(d0: &Document) -> Digest {
    let mut d = Digest::default();
    let mut tables: BTreeMap<u32, (u32, u16, usize)> = BTreeMap::new();
    for (i, p) in d0.paragraphs.iter().enumerate() {
        d.push("text", p.text.clone());
        d.push("paragraph format", para_key(p));
        d.push("character format", runs_key(p, &d0.styles.theme));
        if let Some(c) = &p.cell {
            let n = tables.len();
            let t = tables.entry(c.table).or_insert((0, 0, n));
            t.0 = t.0.max(c.row + 1);
            t.1 = t.1.max(c.col + 1);
        }
        for (s, e, a) in p.spans() {
            let objects = p.text[s..e].chars().filter(|c| *c == OBJECT).count();
            if let Some(img) = &a.image {
                for _ in 0..objects {
                    d.push("images", format!("¶{i}: {:.0}x{:.0}pt {} bytes", img.width, img.height, img.data.len()));
                    match img.object.as_deref().map(|o| &o.kind) {
                        Some(EmbedKind::Ole(o)) => d.push("objects", format!("¶{i}: {}", o.prog_id)),
                        Some(EmbedKind::Diagram(_)) => d.push("objects", format!("¶{i}: diagram")),
                        Some(EmbedKind::Chart(_)) => d.push("objects", format!("¶{i}: chart")),
                        None => {}
                    }
                }
            }
            if let Some(n) = &a.note {
                d.push("notes", n.paras.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join("\n"));
            }
            if let Some(f) = &a.field {
                let k = format!("{:?}", f.kind);
                d.push("fields", k.chars().take(60).collect());
            }
            if a.math.is_some() {
                d.push("equations", format!("¶{i}"));
            }
            if let Some(sh) = &a.shape {
                d.push("shapes", format!("¶{i}: {}", sh.shape.text.iter().map(|p| p.text()).collect::<Vec<_>>().join(" / ")));
            }
            if let Some(l) = &a.link {
                d.push("links", l.to_string());
            }
        }
        for m in &p.marks {
            if !m.end {
                d.push("bookmarks", m.name.to_string());
            }
        }
    }
    // Bookmarks starting at the same place come in any order.
    if let Some(b) = d.lists.get_mut("bookmarks") {
        b.sort();
    }
    d.lists.get_mut("links").map(|l| l.dedup());
    d.lists.get_mut("fields").map(|l| l.dedup());
    d.lists.get_mut("notes").map(|l| l.dedup());
    d.lists.get_mut("shapes").map(|l| l.dedup());
    let mut t: Vec<_> = tables.values().collect();
    t.sort_by_key(|t| t.2);
    d.lists.insert("tables", t.iter().map(|(r, c, _)| format!("{r}x{c}")).collect());
    for c in &d0.comments {
        d.push("comments", c.text.clone());
    }
    // Comments are kept in file order (Word's is when they were added,
    // odt's where they are anchored): their order means nothing.
    if let Some(c) = d.lists.get_mut("comments") {
        c.sort();
    }
    // The headers and footers the pages show (links to earlier sections
    // followed; first and even page ones only when in use).
    for s in 0..d0.sections().len() {
        for slot in apex_doc::HfSlot::ALL {
            let used = match slot.kind() {
                1 => d0.section_headers(s).different_first,
                2 => d0.odd_even,
                _ => true,
            };
            let Some(story) = d0.hf_story(s, slot).filter(|_| used) else { continue };
            let text: Vec<&str> = story.iter().map(|p| p.text.as_str()).collect();
            // An empty header written or not is the same page.
            if text.iter().all(|t| t.trim().is_empty()) {
                continue;
            }
            d.push("headers and footers", format!("§{s} {slot:?}: {}", text.join(" / ")));
        }
    }
    // Word's built-in names don't care about case ("heading 1").
    let mut styles: Vec<String> = d0.styles.defs.iter().map(|s| s.name.to_lowercase()).collect();
    styles.sort();
    d.lists.insert("styles", styles);
    if d0.macros.has_code() {
        d.push("macros", "code".into());
    }
    d
}

fn para_key(p: &Paragraph) -> String {
    let s = &p.style;
    let mut k = format!("{:?} {:?}", s.kind, s.align);
    if s.list != 0 {
        k.push_str(&format!(" list {} level {}", s.list, s.level));
    }
    if s.page_break_before {
        k.push_str(" page break");
    }
    // In steps of 2 pt: twips, 1/100 mm and inches round differently.
    let indent = (s.indent_left / 2.0).round() * 2.0;
    if indent != 0.0 {
        k.push_str(&format!(" indent {indent:.0}"));
    }
    k
}

/// The character formatting of a paragraph: spans of equal attributes.
fn runs_key(p: &Paragraph, theme: &apex_doc::Theme) -> String {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (s, e, a) in p.spans() {
        if e == s {
            continue;
        }
        let mut k = String::new();
        for (on, w) in [(a.bold, "b"), (a.italic, "i"), (a.underline, "u"), (a.strike, "s"), (a.superscript, "sup"), (a.subscript, "sub")] {
            if on {
                k.push_str(w);
                k.push(' ');
            }
        }
        if let Some(sz) = a.size {
            k.push_str(&format!("{sz}pt "));
        }
        if let Some(f) = &a.font {
            // Theme fonts (+mj-lt) by the name they stand for.
            k.push_str(&format!("{} ", theme.font_name(f)));
        }
        if let Some(c) = a.color {
            k.push_str(&format!("#{c:06x} "));
        }
        let n = p.text[s..e].chars().count();
        match out.last_mut() {
            Some((len, last)) if *last == k => *len += n,
            _ => out.push((n, k)),
        }
    }
    out.iter().map(|(n, k)| format!("{n}:{}", k.trim_end())).collect::<Vec<_>>().join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_lose_float_noise() {
        assert_eq!(num(0.1 + 0.2), "0.3");
        assert_eq!(num(12.0), "12");
        assert_eq!(num(1e-20), "0.00000000000000000001");
    }

    #[test]
    fn document_digest() {
        let document = Document::from_plain_text("one\ntwo");
        let d = doc(&document);
        assert_eq!(d.lists["text"], vec!["one".to_string(), "two".to_string()]);
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("apex-bench-model-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn workbook_round_trip_through_xlsx_and_ods() {
        use apex_sheet::CellRef;
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::parse("A1").unwrap(), "12.5");
        wb.set_input(0, CellRef::parse("A2").unwrap(), "hello");
        wb.set_input(0, CellRef::parse("A3").unwrap(), "=A1*2");
        let orig = sheet(&wb);
        for ext in ["xlsx", "ods"] {
            let p = scratch(&format!("book.{ext}"));
            apex_io::save_sheet(&wb, &p, apex_io::Format::from_path(&p).unwrap()).unwrap();
            let (apex_io::Loaded::Sheet(back), _) = apex_io::open(&p).unwrap() else { panic!("not a workbook") };
            assert!(crate::run::diff::diff(&orig, &sheet(&back)).is_empty(), "{ext}");
            assert!(cells(&wb, &back, false).is_empty(), "{ext}: {:?}", cells(&wb, &back, false));
            let mut changed = *back;
            changed.set_input(0, CellRef::parse("A2").unwrap(), "bye");
            changed.set_input(0, CellRef::parse("A3").unwrap(), "=A1*3");
            let d = cells(&wb, &changed, false);
            assert_eq!(d.len(), 2, "{d:?}");
            assert_eq!(d[0].kind, "cells.formula");
            assert_eq!(d[0].detail, "Sheet1!A3: =A1*2 → =A1*3");
            assert_eq!(d[1].detail, "Sheet1!A2: hello → bye; Sheet1!A3: 25 → 37.5");
        }
    }

    #[test]
    fn document_round_trip_through_docx_and_odt() {
        let document = Document::from_plain_text("first\nsecond");
        let orig = doc(&document);
        for ext in ["docx", "odt"] {
            let p = scratch(&format!("text.{ext}"));
            apex_io::save_doc(&document, &p, apex_io::Format::from_path(&p).unwrap()).unwrap();
            let (apex_io::Loaded::Doc(back), _) = apex_io::open(&p).unwrap() else { panic!("not a document") };
            let d = crate::run::diff::diff(&orig, &doc(&back));
            assert!(!d.iter().any(|d| d.kind == "text"), "{ext}: {d:?}");
        }
    }
}
