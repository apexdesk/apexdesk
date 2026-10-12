//! Footnotes, endnotes, fields and tables of contents in .odt files (a
//! child of `odt`, sharing its reader and writer pieces): `text:note`,
//! the text field elements (`text:page-number`, `text:date`,
//! `text:bookmark-ref`…) and `text:table-of-content`.

use std::collections::HashMap;
use std::fmt::Write as _;

use apex_doc::{Attrs, Document, Field, FieldKind, NOTE_SIZE, Note, NoteFormat, NoteKind, NoteRestart, NotesConfig, OBJECT, ParaStyle, Paragraph, TocMark, TocOpts};

use super::{BodyReader, Ctx, pt, text_props, write_text};
use crate::xml::{Ev, Reader, Tag, esc};

impl BodyReader<'_> {
    /// `text:note` (after its start tag): the note's paragraphs.
    pub(super) fn read_note(&mut self, r: &mut Reader, t: &Tag) -> Note {
        let kind = if t.attr("text:note-class").as_deref() == Some("endnote") { NoteKind::End } else { NoteKind::Foot };
        let mut sub = BodyReader { ctx: self.ctx, out: Vec::new(), lists: Vec::new(), next_table: self.next_table + 1000, copy_left: self.copy_left, spaces_left: self.spaces_left, paras_left: self.paras_left, table_depth: self.table_depth, toc: None, used: Default::default(), notes: Default::default(), track: Default::default(), tracking: false, sects: Vec::new(), sects_opened: 0, marks: Vec::new(), shapes: Vec::new(), pictures: Vec::new(), header: false, biblio: Vec::new(), controls: 0, ruby_seq: 0, ruby: None };
        let mut depth = 1;
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(c) => match c.name {
                    "text:note-body" => sub.read_body(r),
                    _ => r.skip(),
                },
                Ev::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        self.copy_left = sub.copy_left;
        self.spaces_left = sub.spaces_left;
        self.paras_left = sub.paras_left;
        // Notes have no tables: their cells' text stays, a paragraph each.
        let mut paras: Vec<Paragraph> = sub.out;
        for p in &mut paras {
            p.cell = None;
        }
        // Named styles aren't kept in notes.
        for p in &mut paras {
            p.style.custom = 0;
        }
        if paras.is_empty() {
            return Note::new(kind);
        }
        for p in &mut paras {
            p.style = ParaStyle { kind: apex_doc::ParaKind::Normal, toc: None, ..p.style };
            for run in &mut p.runs {
                run.attrs.note = None;
                if run.attrs.size.is_none() {
                    run.attrs.size = Some(NOTE_SIZE);
                }
            }
        }
        Note { kind, paras }
    }

    /// `text:table-of-content` (after its start tag): its entries, marked.
    pub(super) fn read_toc(&mut self, r: &mut Reader) {
        let mut opts = TocOpts::default();
        let mut depth = 1;
        loop {
            let ev = r.next();
            match ev {
                Ev::Eof => break,
                Ev::Start(c) | Ev::Empty(c) => {
                    let is_start = matches!(ev, Ev::Start(_));
                    match c.name {
                        "text:bibliography-source" => {
                            opts = TocOpts { bib: true, links: false, ..TocOpts::default() };
                            if is_start {
                                depth += 1;
                            }
                        }
                        "text:table-of-content-source" | "text:alphabetical-index-source" => {
                            if c.name == "text:alphabetical-index-source" {
                                opts.index = true;
                                opts.links = false;
                                opts.letters = c.attr("text:alphabetical-separators").as_deref() == Some("true");
                            }
                            if let Some(l) = c.attr("text:outline-level").and_then(|v| v.parse::<u8>().ok()) {
                                opts.levels = l.clamp(1, 9);
                            }
                            // Entry templates: page numbers and links.
                            let (mut page, mut link, mut tab) = (false, false, false);
                            if is_start {
                                let mut d = 1;
                                loop {
                                    let e = r.next();
                                    match e {
                                        Ev::Eof => break,
                                        Ev::Start(x) | Ev::Empty(x) => {
                                            match x.name {
                                                "text:index-entry-page-number" => page = true,
                                                "text:index-entry-link-start" => link = true,
                                                "text:index-entry-tab-stop" => tab = true,
                                                _ => {}
                                            }
                                            if matches!(e, Ev::Start(_)) {
                                                d += 1;
                                            }
                                        }
                                        Ev::End(_) => {
                                            d -= 1;
                                            if d == 0 {
                                                break;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                if opts.index {
                                    opts.right_align = tab;
                                } else {
                                    opts.page_numbers = page;
                                    opts.links = link;
                                }
                            }
                        }
                        "text:index-body" if is_start => {
                            self.toc = Some(opts);
                            self.read_body(r);
                            self.toc = None;
                        }
                        _ => {
                            if is_start {
                                depth += 1;
                            }
                        }
                    }
                }
                Ev::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    /// Marks a paragraph read inside a table of contents: "Contents N"
    /// styles give the level; its title stays a plain paragraph.
    pub(super) fn toc_mark(&self, style: &mut ParaStyle, p: &Paragraph, name: Option<&str>) {
        let Some(opts) = self.toc else { return };
        if p.text.trim().is_empty() || matches!(style.kind, apex_doc::ParaKind::Heading(_) | apex_doc::ParaKind::Title) {
            return;
        }
        // The named style (styles missing from the file go by their name).
        let display = name.map(|n| Ctx::chain(&self.ctx.para, n).iter().find(|s| !s.auto).map_or_else(|| n.replace("_20_", " "), |s| s.display.clone()).to_ascii_lowercase()).unwrap_or_default();
        if display.contains("heading") || display.contains("título") || display.contains("titulo") {
            return;
        }
        let level = display.strip_prefix("contents").or_else(|| display.strip_prefix("index")).or_else(|| display.strip_prefix("bibliography")).and_then(|l| l.trim().parse::<u8>().ok()).filter(|l| (1..=9).contains(l)).unwrap_or(0);
        if level > 0 && style.indent_left == 0.0 && style.level == 0 {
            style.indent_left = (level - 1) as f32 * apex_doc::TOC_INDENT_PT;
        }
        style.toc = Some(TocMark { level, opts });
    }
}

const BIB_TYPES: [(apex_doc::SourceKind, &str); 5] = [(apex_doc::SourceKind::Book, "book"), (apex_doc::SourceKind::Article, "article"), (apex_doc::SourceKind::Web, "www"), (apex_doc::SourceKind::Report, "techreport"), (apex_doc::SourceKind::Other, "misc")];

/// The attributes of a `text:bibliography-mark`: ODF keeps each source's
/// data in its citations.
fn biblio_attrs(s: &apex_doc::Source) -> String {
    let ty = BIB_TYPES.iter().find(|t| t.0 == s.kind).map_or("misc", |t| t.1);
    let mut out = format!(r#" text:identifier="{}" text:bibliography-type="{ty}""#, esc(&s.tag));
    // Authors as "Last, First; …" (LibreOffice's own convention).
    for (k, v) in [("author", &s.authors), ("title", &s.title), ("year", &s.year), ("address", &s.city), ("publisher", &s.publisher), ("journal", &s.journal), ("volume", &s.volume), ("pages", &s.pages), ("url", &s.url)] {
        if !v.is_empty() {
            let _ = write!(out, r#" text:{k}="{}""#, esc(v));
        }
    }
    out
}

/// A source from a `text:bibliography-mark`.
pub(super) fn read_biblio_mark(t: &Tag) -> Option<apex_doc::Source> {
    let get = |k: &str| t.attr(k).map(|v| v.into_owned()).unwrap_or_default();
    let tag = get("text:identifier");
    if tag.is_empty() {
        return None;
    }
    let ty = get("text:bibliography-type");
    let kind = BIB_TYPES.iter().find(|x| x.1 == ty).map_or(apex_doc::SourceKind::Other, |x| x.0);
    Some(apex_doc::Source { tag, kind, authors: get("text:author"), title: get("text:title"), year: get("text:year"), city: get("text:address"), publisher: get("text:publisher"), journal: get("text:journal"), volume: get("text:volume"), pages: get("text:pages"), url: get("text:url") })
}

/// A text field element as a field, with its shown text; `None` for other
/// elements.
pub(super) fn read_field(r: &mut Reader, t: &Tag, is_start: bool) -> Option<Field> {
    let kind = match t.name {
        "text:page-number" => FieldKind::Page,
        "text:page-count" => FieldKind::NumPages,
        "text:date" => FieldKind::Date,
        "text:time" => FieldKind::Time,
        "text:author-name" | "text:initial-creator" | "text:creator" => FieldKind::Author,
        "text:title" => FieldKind::Title,
        "text:file-name" => FieldKind::FileName,
        "text:subject" => FieldKind::DocProperty("Subject".into()),
        "text:keywords" => FieldKind::DocProperty("Keywords".into()),
        "text:description" => FieldKind::DocProperty("Comments".into()),
        "text:user-defined" => FieldKind::DocProperty(t.attr("text:name")?.into_owned()),
        "text:sequence" => FieldKind::Seq(t.attr("text:name")?.into_owned()),
        "text:alphabetical-index-mark" => {
            let v = t.attr("text:string-value")?.into_owned();
            let mut keys: Vec<String> = ["text:key1", "text:key2"].iter().filter_map(|k| t.attr(k)).map(|k| k.into_owned()).filter(|k| !k.is_empty()).collect();
            keys.truncate(1);
            keys.push(v);
            FieldKind::IndexEntry(keys.join(":"))
        }
        "text:expression" | "text:table-formula" => {
            let f = t.attr("text:formula")?;
            let f = f.split_once(':').filter(|(ns, _)| !ns.contains(['(', '<'])).map_or(&*f, |(_, e)| e);
            FieldKind::Formula(f.trim_start_matches('=').replace(['<', '>'], ""))
        }
        "text:bookmark-ref" => {
            let name = t.attr("text:ref-name")?.into_owned();
            match t.attr("text:reference-format").as_deref() {
                Some("page") => FieldKind::PageRef(name),
                Some("number" | "number-no-superior" | "number-all-superior") => FieldKind::NoteRef(name),
                _ => FieldKind::Ref(name),
            }
        }
        _ => return None,
    };
    let position = t.name == "text:bookmark-ref" && t.attr("text:reference-format").as_deref() == Some("direction");
    let cached = if is_start { r.text() } else { String::new() };
    let mut f = Field::new(kind);
    f.position = position;
    f.hyperlink = false;
    f.cached = cached;
    Some(f)
}

fn format_of(v: &str) -> NoteFormat {
    match v {
        "i" => NoteFormat::LowerRoman,
        "I" => NoteFormat::UpperRoman,
        "a" => NoteFormat::LowerLetter,
        "A" => NoteFormat::UpperLetter,
        _ => NoteFormat::Arabic,
    }
}

fn format_code(f: NoteFormat) -> &'static str {
    match f {
        NoteFormat::Arabic | NoteFormat::Symbols => "1",
        NoteFormat::LowerRoman => "i",
        NoteFormat::UpperRoman => "I",
        NoteFormat::LowerLetter => "a",
        NoteFormat::UpperLetter => "A",
    }
}

/// `text:linenumbering-configuration` of styles.xml.
pub(super) fn read_line_numbering(styles: &str) -> Option<apex_doc::LineNumbering> {
    let mut r = Reader::new(styles);
    loop {
        match r.next() {
            Ev::Eof => return None,
            Ev::Start(t) | Ev::Empty(t) if t.name == "text:linenumbering-configuration" => {
                if t.attr("text:number-lines").as_deref() != Some("true") {
                    return None;
                }
                let restart = if t.attr("text:restart-on-page").as_deref() == Some("true") { apex_doc::LineRestart::EachPage } else { apex_doc::LineRestart::Continuous };
                let count_by = t.attr("text:increment").and_then(|v| v.parse::<u16>().ok()).unwrap_or(1).clamp(1, 100);
                let distance = t.attr("text:offset").and_then(|v| crate::parse_length_pt(&v)).filter(|&d| d > 0.0).map(|d| (d * 10.0).round() / 10.0);
                return Some(apex_doc::LineNumbering { count_by, start: 1, restart, distance });
            }
            _ => {}
        }
    }
}

/// `text:notes-configuration` of styles.xml.
pub(super) fn read_config(styles: &str) -> NotesConfig {
    let mut cfg = NotesConfig::default();
    let mut r = Reader::new(styles);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.name == "text:notes-configuration" => {
                let s = if t.attr("text:note-class").as_deref() == Some("endnote") { &mut cfg.end } else { &mut cfg.foot };
                // Symbols are written as a custom citation; keep the others.
                if let Some(f) = t.attr("style:num-format") {
                    s.format = format_of(&f);
                }
                if let Some(v) = t.attr("text:start-value").and_then(|v| v.parse::<u32>().ok()) {
                    s.start = (v + 1).min(9999);
                }
                s.restart = match t.attr("text:start-numbering-at").as_deref() {
                    Some("page") => NoteRestart::EachPage,
                    Some("chapter") => NoteRestart::EachSection,
                    _ => NoteRestart::Continuous,
                };
            }
            _ => {}
        }
    }
    cfg
}

// ---------------------------------------------------------------------------
// Writing

/// Numbers notes as they are written, and names the paragraph styles of
/// their spacing, line height and alignment.
#[derive(Default)]
pub(super) struct NoteWriter {
    foot: usize,
    end: usize,
    paras: HashMap<(&'static str, String), String>,
}

/// The paragraph style of a note paragraph: `parent`, or an automatic one
/// over it with the paragraph's own spacing, line height and alignment.
fn note_para_style(st: &ParaStyle, parent: &'static str, nw: &mut NoteWriter, auto: &mut String) -> String {
    let mut props = String::new();
    match st.align {
        apex_doc::Align::Left => {}
        apex_doc::Align::Center => props.push_str(r#" fo:text-align="center""#),
        apex_doc::Align::Right => props.push_str(r#" fo:text-align="end""#),
        apex_doc::Align::Justify => props.push_str(r#" fo:text-align="justify""#),
    }
    let ptx = |v: f32| format!("{}pt", (v * 100.0).round() / 100.0);
    if st.line_spacing > 0.0 {
        let _ = match st.line_rule {
            apex_doc::LineRule::Multiple => write!(props, r#" fo:line-height="{}%""#, (st.line_spacing * 100.0).round() as u32),
            apex_doc::LineRule::Exact => write!(props, r#" fo:line-height="{}""#, ptx(st.line_spacing)),
            apex_doc::LineRule::AtLeast => write!(props, r#" style:line-height-at-least="{}""#, ptx(st.line_spacing)),
        };
    }
    if let Some(b) = st.space_before {
        let _ = write!(props, r#" fo:margin-top="{}""#, ptx(b.max(0.0)));
    }
    if let Some(a) = st.space_after {
        let _ = write!(props, r#" fo:margin-bottom="{}""#, ptx(a.max(0.0)));
    }
    if props.is_empty() {
        return parent.to_string();
    }
    let n = nw.paras.len();
    nw.paras
        .entry((parent, props))
        .or_insert_with_key(|(parent, props)| {
            let name = format!("NoteP{}", n + 1);
            let _ = write!(auto, r#"<style:style style:name="{name}" style:family="paragraph" style:parent-style-name="{parent}"><style:paragraph-properties{props}/></style:style>"#);
            name
        })
        .clone()
}

/// A text style name for `a` (automatic styles are made as needed).
pub(super) fn text_style(a: &Attrs, styles: &mut HashMap<Attrs, String>, auto: &mut String) -> Option<String> {
    let a = Attrs { link: None, link_tip: None, image: None, note: None, field: None, shape: None, math: None, ..a.clone() };
    if a == Attrs::default() {
        return None;
    }
    let n = styles.len();
    Some(
        styles
            .entry(a.clone())
            .or_insert_with(|| {
                let name = format!("T{}", n + 1);
                let _ = write!(auto, r#"<style:style style:name="{}" style:family="text"><style:text-properties{}/></style:style>"#, name, text_props(&a));
                name
            })
            .clone(),
    )
}

/// Writes the note marks or fields of `text` (its `OBJECT` characters).
/// False for other text.
pub(super) fn write_object(body: &mut String, text: &str, a: &Attrs, doc: &Document, nw: &mut NoteWriter, styles: &mut HashMap<Attrs, String>, auto: &mut String) -> bool {
    if a.note.is_none() && a.field.is_none() {
        return false;
    }
    let span = text_style(a, styles, auto);
    for _ in text.matches(OBJECT) {
        if let Some(n) = &a.note {
            let (class, num, prefix) = match n.kind {
                NoteKind::Foot => {
                    nw.foot += 1;
                    ("footnote", nw.foot, "ftn")
                }
                NoteKind::End => {
                    nw.end += 1;
                    ("endnote", nw.end, "edn")
                }
            };
            let s = doc.notes.get(n.kind);
            let label = s.format.label(num + s.start.max(1) as usize - 1);
            // The mark's formatting around it.
            if let Some(name) = &span {
                let _ = write!(body, r#"<text:span text:style-name="{name}">"#);
            }
            let _ = write!(body, r#"<text:note text:id="{prefix}{num}" text:note-class="{class}"><text:note-citation"#);
            if s.format == NoteFormat::Symbols {
                let _ = write!(body, r#" text:label="{}""#, esc(&label));
            }
            let _ = write!(body, ">{}</text:note-citation><text:note-body>", esc(&label));
            let pstyle = if n.kind == NoteKind::Foot { "Footnote" } else { "Endnote" };
            for p in &n.paras {
                let name = note_para_style(&p.style, pstyle, nw, auto);
                let _ = write!(body, r#"<text:p text:style-name="{name}">"#);
                for (s, e, ra) in p.spans() {
                    if s == e || ra.image.is_some() || ra.note.is_some() {
                        continue;
                    }
                    if ra.field.is_some() {
                        let mut inner = NoteWriter::default();
                        write_object(body, &p.text[s..e], ra, doc, &mut inner, styles, auto);
                        continue;
                    }
                    let link = ra.link.as_ref();
                    if let Some(l) = link {
                        let _ = write!(body, r#"<text:a xlink:type="simple" xlink:href="{}" text:style-name="Internet_20_link">"#, esc(l));
                    }
                    match text_style(ra, styles, auto) {
                        Some(name) => {
                            let _ = write!(body, r#"<text:span text:style-name="{name}">"#);
                            write_text(body, &p.text[s..e]);
                            body.push_str("</text:span>");
                        }
                        None => write_text(body, &p.text[s..e]),
                    }
                    if link.is_some() {
                        body.push_str("</text:a>");
                    }
                }
                body.push_str("</text:p>");
            }
            body.push_str("</text:note-body></text:note>");
            if span.is_some() {
                body.push_str("</text:span>");
            }
            continue;
        }
        let Some(f) = &a.field else { continue };
        if let Some(name) = &span {
            let _ = write!(body, r#"<text:span text:style-name="{name}">"#);
        }
        let v = esc(&f.cached);
        let _ = match &f.kind {
            FieldKind::Page => write!(body, r#"<text:page-number text:select-page="current">{v}</text:page-number>"#),
            FieldKind::NumPages => write!(body, "<text:page-count>{v}</text:page-count>"),
            FieldKind::Date => write!(body, "<text:date>{v}</text:date>"),
            FieldKind::Time => write!(body, "<text:time>{v}</text:time>"),
            FieldKind::Author => write!(body, "<text:initial-creator>{v}</text:initial-creator>"),
            FieldKind::Title => write!(body, "<text:title>{v}</text:title>"),
            FieldKind::DocProperty(n) => match n.to_ascii_lowercase().as_str() {
                "title" => write!(body, "<text:title>{v}</text:title>"),
                "subject" => write!(body, "<text:subject>{v}</text:subject>"),
                "author" => write!(body, "<text:initial-creator>{v}</text:initial-creator>"),
                "keywords" => write!(body, "<text:keywords>{v}</text:keywords>"),
                "comments" => write!(body, "<text:description>{v}</text:description>"),
                // Custom properties are meta:user-defined in meta.xml.
                _ => write!(body, r#"<text:user-defined text:name="{}">{v}</text:user-defined>"#, esc(n)),
            },
            FieldKind::FileName => write!(body, r#"<text:file-name text:display="name-and-extension">{v}</text:file-name>"#),
            FieldKind::Seq(n) => write!(body, r#"<text:sequence text:name="{0}" text:formula="ooow:{0}+1" style:num-format="1">{v}</text:sequence>"#, esc(n)),
            FieldKind::IndexEntry(t) => {
                let (m, sub) = apex_doc::index_parts(t);
                match sub {
                    Some(sub) => write!(body, r#"<text:alphabetical-index-mark text:string-value="{}" text:key1="{}"/>"#, esc(&sub), esc(&m)),
                    None => write!(body, r#"<text:alphabetical-index-mark text:string-value="{}"/>"#, esc(&m)),
                }
            }
            FieldKind::Citation(tag) => {
                let src = doc.biblio.source(tag).cloned().unwrap_or_else(|| apex_doc::Source { tag: tag.clone(), ..Default::default() });
                let _ = write!(body, "<text:bibliography-mark{}>", biblio_attrs(&src));
                body.push_str(&v);
                body.push_str("</text:bibliography-mark>");
                Ok(())
            }
            // No tables of authorities in OpenDocument: entries are dropped.
            FieldKind::Authority { .. } => Ok(()),
            FieldKind::IndexPages(_) | FieldKind::AuthorityPages(_) => {
                body.push_str(&v);
                Ok(())
            }
            FieldKind::Formula(e) => write!(body, r#"<text:expression text:formula="ooow:{}" office:value-type="string" office:string-value="{v}">{v}</text:expression>"#, esc(&odf_refs(e))),
            FieldKind::Ref(b) | FieldKind::PageRef(b) | FieldKind::NoteRef(b) => {
                let format = match (&f.kind, f.position) {
                    (_, true) => "direction",
                    (FieldKind::PageRef(_), _) => "page",
                    (FieldKind::NoteRef(_), _) => "number",
                    _ => "text",
                };
                write!(body, r#"<text:bookmark-ref text:reference-format="{format}" text:ref-name="{}">{v}</text:bookmark-ref>"#, esc(b))
            }
        };
        if span.is_some() {
            body.push_str("</text:span>");
        }
    }
    true
}

/// A formula with its cell references in angle brackets ("<A1>"), as
/// OpenDocument writes them.
fn odf_refs(e: &str) -> String {
    let mut out = String::new();
    let cs: Vec<char> = e.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i].is_ascii_alphabetic() && (i == 0 || !cs[i - 1].is_alphanumeric()) {
            let mut j = i;
            while j < cs.len() && cs[j].is_ascii_alphabetic() {
                j += 1;
            }
            let k = cs[j..].iter().take_while(|c| c.is_ascii_digit()).count();
            let end = j + k;
            let word: String = cs[i..end].iter().collect();
            if k > 0 && j - i <= 2 && cs.get(end).is_none_or(|c| !c.is_alphanumeric()) {
                out.push_str(&format!("<{word}>"));
            } else {
                out.push_str(&word);
            }
            i = end;
        } else {
            out.push(cs[i]);
            i += 1;
        }
    }
    out
}

/// Paragraph style of a table of contents entry.
pub(super) fn toc_named(st: &ParaStyle) -> Option<String> {
    let t = st.toc?;
    Some(match t.level {
        0 => "Standard".into(),
        _ if t.opts.toa > 0 => "Standard".into(),
        l if t.opts.index => format!("Index_20_{}", l.min(3)),
        _ if t.opts.bib => "Bibliography_20_1".into(),
        l => format!("Contents_20_{}", l.min(9)),
    })
}

/// Opens `text:table-of-content` before the first entry of a table.
pub(super) fn toc_open(body: &mut String, doc: &Document, idx: usize, count: &mut usize) {
    let Some(t) = doc.paragraphs[idx].style.toc else { return };
    if idx > 0 && doc.paragraphs[idx - 1].style.toc.is_some() {
        return;
    }
    let o = t.opts;
    if o.toa > 0 {
        return;
    }
    *count += 1;
    if o.bib {
        let _ = write!(body, r#"<text:bibliography text:name="Bibliography{count}" text:protected="true"><text:bibliography-source><text:index-title-template/></text:bibliography-source><text:index-body>"#);
        return;
    }
    if o.index {
        let _ = write!(body, r#"<text:alphabetical-index text:name="Alphabetical Index{count}" text:protected="true"><text:alphabetical-index-source text:ignore-case="true" text:alphabetical-separators="{}" text:main-entry-style-name="Standard"><text:index-title-template/>"#, o.letters);
        if o.letters {
            body.push_str(r#"<text:alphabetical-index-entry-template text:outline-level="separator" text:style-name="Standard"><text:index-entry-text/></text:alphabetical-index-entry-template>"#);
        }
        for l in 1..=3u8 {
            let _ = write!(body, r#"<text:alphabetical-index-entry-template text:outline-level="{l}" text:style-name="Index_20_{l}"><text:index-entry-text/>"#);
            if o.page_numbers {
                if o.right_align {
                    body.push_str(r#"<text:index-entry-tab-stop style:type="right"/>"#);
                } else {
                    body.push_str("<text:index-entry-span>, </text:index-entry-span>");
                }
                body.push_str("<text:index-entry-page-number/>");
            }
            body.push_str("</text:alphabetical-index-entry-template>");
        }
        body.push_str("</text:alphabetical-index-source><text:index-body>");
        return;
    }
    let _ = write!(body, r#"<text:table-of-content text:name="Table of Contents{count}" text:protected="true"><text:table-of-content-source text:outline-level="{}" text:use-index-marks="false"><text:index-title-template/>"#, o.levels.clamp(1, 9));
    for l in 1..=10u8 {
        let _ = write!(body, r#"<text:table-of-content-entry-template text:outline-level="{l}" text:style-name="Contents_20_{}">"#, l.min(9));
        if o.links {
            body.push_str("<text:index-entry-link-start/>");
        }
        body.push_str("<text:index-entry-chapter/><text:index-entry-text/>");
        if o.page_numbers {
            let leader = if o.leader { r#" style:leader-char=".""# } else { "" };
            if o.right_align {
                let _ = write!(body, r#"<text:index-entry-tab-stop style:type="right"{leader}/>"#);
            } else {
                body.push_str("<text:index-entry-span> </text:index-entry-span>");
            }
            body.push_str("<text:index-entry-page-number/>");
        }
        if o.links {
            body.push_str("<text:index-entry-link-end/>");
        }
        body.push_str("</text:table-of-content-entry-template>");
    }
    body.push_str("</text:table-of-content-source><text:index-body>");
}

/// Closes it after the last entry.
pub(super) fn toc_close(body: &mut String, doc: &Document, idx: usize) {
    if let Some(t) = doc.paragraphs[idx].style.toc
        && t.opts.toa == 0
        && doc.paragraphs.get(idx + 1).is_none_or(|p| p.style.toc.is_none())
    {
        body.push_str(if t.opts.index {
            "</text:index-body></text:alphabetical-index>"
        } else if t.opts.bib {
            "</text:index-body></text:bibliography>"
        } else {
            "</text:index-body></text:table-of-content>"
        });
    }
}

/// Named styles for notes and tables of contents, and the numbering of notes.
pub(super) fn styles(doc: &Document) -> String {
    let mut s = String::new();
    for (name, display) in [("Footnote", "Footnote"), ("Endnote", "Endnote")] {
        let _ = write!(s, r#"<style:style style:name="{name}" style:display-name="{display}" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"><style:paragraph-properties fo:margin-left="0.2cm" fo:margin-top="0cm" fo:margin-bottom="0cm" fo:text-indent="-0.2cm"/><style:text-properties fo:font-size="10pt"/></style:style>"#);
    }
    for (name, display) in [("Footnote_20_Symbol", "Footnote Symbol"), ("Endnote_20_Symbol", "Endnote Symbol")] {
        let _ = write!(s, r#"<style:style style:name="{name}" style:display-name="{display}" style:family="text"/>"#);
    }
    s.push_str(r#"<style:style style:name="Index" style:family="paragraph" style:parent-style-name="Standard" style:class="index"/>"#);
    let _ = write!(s, r#"<style:style style:name="Contents_20_Heading" style:display-name="Contents Heading" style:family="paragraph" style:parent-style-name="Heading" style:class="index"><style:text-properties fo:font-size="16pt" fo:font-weight="bold"/></style:style>"#);
    let w = doc.page.content_width();
    for n in 1..=9u8 {
        let indent = (n - 1) as f32 * apex_doc::TOC_INDENT_PT;
        let _ = write!(
            s,
            r#"<style:style style:name="Contents_20_{n}" style:display-name="Contents {n}" style:family="paragraph" style:parent-style-name="Index" style:class="index"><style:paragraph-properties fo:margin-left="{}" fo:margin-right="0cm" fo:text-indent="0cm" fo:margin-bottom="0.176cm"><style:tab-stops><style:tab-stop style:position="{}" style:type="right" style:leader-style="dotted" style:leader-text="."/></style:tab-stops></style:paragraph-properties></style:style>"#,
            pt(indent),
            pt(w - indent)
        );
    }
    if let Some(l) = doc.page.line_numbers {
        let offset = l.distance.map_or_else(String::new, |d| format!(r#" text:offset="{}""#, pt(d)));
        let restart = l.restart == apex_doc::LineRestart::EachPage;
        let _ = write!(s, r#"<text:linenumbering-configuration text:number-lines="true" text:increment="{}" text:restart-on-page="{restart}" text:number-position="left" style:num-format="1"{offset}/>"#, l.count_by.max(1));
    }
    s.push_str(r#"<style:style style:name="Bibliography_20_1" style:display-name="Bibliography 1" style:family="paragraph" style:parent-style-name="Index" style:class="index"/>"#);
    let numbered = doc.biblio.style == apex_doc::CiteStyle::Ieee;
    // The style by name too (ours): ODF only tells numbered or not.
    let _ = write!(
        s,
        r#"<text:bibliography-configuration xmlns:apex="{}" apex:cite-style="{}" text:numbered-entries="{numbered}" text:prefix="{}" text:suffix="{}"/>"#,
        crate::xlsx_model::NS,
        doc.biblio.style.name(),
        if numbered { "[" } else { "(" },
        if numbered { "]" } else { ")" }
    );
    for n in 1..=3u8 {
        let indent = (n - 1) as f32 * apex_doc::TOC_INDENT_PT;
        let _ = write!(
            s,
            r#"<style:style style:name="Index_20_{n}" style:display-name="Index {n}" style:family="paragraph" style:parent-style-name="Index" style:class="index"><style:paragraph-properties fo:margin-left="{}" fo:margin-right="0cm" fo:text-indent="0cm"><style:tab-stops><style:tab-stop style:position="{}" style:type="right" style:leader-style="dotted" style:leader-text="."/></style:tab-stops></style:paragraph-properties></style:style>"#,
            pt(indent),
            pt(w - indent)
        );
    }
    for (class, set) in [("footnote", &doc.notes.foot), ("endnote", &doc.notes.end)] {
        let at = match set.restart {
            NoteRestart::Continuous => "document",
            NoteRestart::EachPage => "page",
            NoteRestart::EachSection => "chapter",
        };
        let (pstyle, sym) = if class == "footnote" { ("Footnote", "Footnote_20_Symbol") } else { ("Endnote", "Endnote_20_Symbol") };
        let _ = write!(
            s,
            r#"<text:notes-configuration text:note-class="{class}" text:citation-style-name="{sym}" text:default-style-name="{pstyle}" style:num-format="{}" text:start-value="{}"{}/>"#,
            format_code(set.format),
            set.start.saturating_sub(1),
            if class == "footnote" { format!(r#" text:footnotes-position="page" text:start-numbering-at="{at}""#) } else { String::new() }
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use crate::docx::{check_refs, refs_sample};

    #[test]
    fn notes_fields_toc_roundtrip() {
        let d = refs_sample();
        let back = super::super::read(&super::super::write(&d)).unwrap();
        // Date pictures need data styles in ODF: not kept.
        check_refs(&back, &d, false);
        let again = super::super::read(&super::super::write(&back)).unwrap();
        check_refs(&again, &d, false);
    }

    #[test]
    fn libreoffice_table_of_contents() {
        // As LibreOffice writes it: static page numbers, links to its own
        // heading bookmarks.
        let body = r##"<text:table-of-content text:style-name="Sect1" text:protected="true" text:name="Table of Contents1"><text:table-of-content-source text:outline-level="2"><text:index-title-template text:style-name="Contents_20_Heading">Contents</text:index-title-template><text:table-of-content-entry-template text:outline-level="1" text:style-name="Contents_20_1"><text:index-entry-link-start text:style-name="Index_20_Link"/><text:index-entry-chapter/><text:index-entry-text/><text:index-entry-tab-stop style:type="right" style:leader-char="."/><text:index-entry-page-number/><text:index-entry-link-end/></text:table-of-content-entry-template></text:table-of-content-source><text:index-body><text:index-title text:style-name="Sect1" text:name="Table of Contents1_Head"><text:p text:style-name="Contents_20_Heading">Contents</text:p></text:index-title><text:p text:style-name="Contents_20_1"><text:a xlink:type="simple" xlink:href="#__RefHeading___Toc1_1" text:style-name="Index_20_Link">One<text:tab/>1</text:a></text:p></text:index-body></text:table-of-content><text:h text:outline-level="1"><text:bookmark-start text:name="__RefHeading___Toc1_1"/>One<text:bookmark-end text:name="__RefHeading___Toc1_1"/></text:h><text:p>See <text:bookmark-ref text:reference-format="page" text:ref-name="__RefHeading___Toc1_1">1</text:bookmark-ref> and<text:note text:id="ftn1" text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p text:style-name="Footnote">Note <text:span>text</text:span></text:p></text:note-body></text:note>.</text:p>"##;
        let content = format!(r#"{}<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:xlink="http://www.w3.org/1999/xlink"><office:body><office:text>{body}</office:text></office:body></office:document-content>"#, crate::xml::HEADER);
        let mut z = crate::zip::ZipWriter::new();
        z.add("content.xml", content.as_bytes(), true);
        let doc = super::super::read(&z.finish()).unwrap();
        let t: Vec<&str> = doc.paragraphs.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(t, ["Contents", "One\t1", "One", "See \u{FFFC} and\u{FFFC}."]);
        assert!(doc.paragraphs[0].style.toc.is_none());
        let toc = doc.paragraphs[1].style.toc.unwrap();
        assert_eq!((toc.level, toc.opts.levels, toc.opts.links), (1, 2, true));
        assert_eq!(doc.notes()[0].1.text(), "Note text");
        assert_eq!(doc.fields()[0].1.kind, apex_doc::FieldKind::PageRef("__RefHeading___Toc1_1".into()));
    }
}
