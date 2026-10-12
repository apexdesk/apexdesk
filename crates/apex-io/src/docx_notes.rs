//! Footnotes, endnotes, fields and tables of contents in .docx files
//! (a child of `docx`, sharing its reader and writer pieces).
//!
//! Notes live in `word/footnotes.xml` and `word/endnotes.xml`, referenced
//! from the text by `w:footnoteReference`; numbering is in `w:footnotePr`.
//! Fields are kept as fields (`w:fldSimple` and complex fields), and a
//! table of contents is a TOC field whose result paragraphs use the
//! "toc N" styles.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Attrs, Document, Field, FieldKind, LineRule, NOTE_SIZE, Note, NoteFormat, NoteKind, NoteRestart, NoteSeparator, NoteSettings, NotesConfig, OBJECT, ParaStyle, Paragraph, TocMark, TocOpts};

use crate::docx_styles::PlainLooks;
use super::{Ctx, DocReader, TWIP, docx_img, read_ppr, read_rels, read_run, write_rpr, write_run, write_text};
use crate::xml::{Ev, HEADER, Reader, Tag, esc, local};
use crate::zip::ZipReader;

/// Notes of a file by kind and id.
pub(super) type NoteMap = HashMap<(NoteKind, String), Arc<Note>>;

/// Reads `word/footnotes.xml` and `word/endnotes.xml`, and the footnote
/// separator.
pub(super) fn read_notes(z: &ZipReader, ctx: &Ctx) -> (NoteMap, Option<NoteSeparator>) {
    let mut out = NoteMap::new();
    let styles = z.read_string("word/styles.xml").ok();
    let looks = PlainLooks::new(styles.as_deref(), &ctx.theme);
    let mut sep = None;
    for (kind, part, tag) in [(NoteKind::Foot, "footnotes", "footnote"), (NoteKind::End, "endnotes", "endnote")] {
        let Ok(xml) = z.read_string(&format!("word/{part}.xml")) else { continue };
        let rels = z.read_string(&format!("word/_rels/{part}.xml.rels")).map(|s| read_rels(&s)).unwrap_or_default();
        let mut r = Reader::new(&xml);
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) if t.local() == tag => {
                    let id = t.attr("w:id").map(|s| s.into_owned()).unwrap_or_default();
                    // Separators aren't notes.
                    if let Some(ty) = t.attr("w:type").filter(|v| v != "normal") {
                        if kind == NoteKind::Foot && ty == "separator" {
                            sep = Some(read_separator(r.raw_element(), &looks));
                        } else {
                            r.skip();
                        }
                        continue;
                    }
                    let paras = read_note_body(&mut r, ctx, &rels, tag, &looks);
                    out.insert((kind, id), Arc::new(note_from(kind, paras)));
                }
                _ => {}
            }
        }
    }
    (out, sep)
}

/// The footnote separator paragraph: its style and spacing, and the size
/// of its text.
fn read_separator(xml: &str, looks: &PlainLooks) -> NoteSeparator {
    let mut r = Reader::new(xml);
    let (mut style, mut spacing, mut size) = (None, None, None);
    loop {
        match r.next() {
            Ev::Eof => break,
            // The first paragraph only.
            Ev::End(n) if local(n) == "p" => break,
            Ev::Start(t) | Ev::Empty(t) => match t.local() {
                "pStyle" => style = t.attr("w:val").map(|v| v.into_owned()),
                "spacing" if spacing.is_none() && t.attr("w:line").or(t.attr("w:after")).or(t.attr("w:before")).is_some() => {
                    let tw = |k: &str| t.attr(k).and_then(|v| v.parse::<f32>().ok());
                    spacing = Some((tw("w:before"), tw("w:after"), tw("w:line"), t.attr("w:lineRule").map(|v| v.into_owned())));
                }
                "sz" if size.is_none() => size = t.attr("w:val").and_then(|v| v.parse::<u16>().ok()).filter(|&s| s > 0),
                _ => {}
            },
            _ => {}
        }
    }
    let look = looks.of(style.as_deref());
    let (before, after, line, rule) = spacing.unwrap_or_default();
    let (rule, line) = match rule.as_deref() {
        Some("exact") => (LineRule::Exact, line.unwrap_or(240.0)),
        Some("atLeast") => (LineRule::AtLeast, line.unwrap_or(0.0)),
        _ => (LineRule::Multiple, line.map_or(look.line * 100.0, |l| l / 2.4)),
    };
    let tw = |v: f32| v.round().clamp(0.0, u16::MAX as f32) as u16;
    NoteSeparator {
        size: size.or(look.size).unwrap_or(NOTE_SIZE),
        line: tw(line),
        rule,
        before: tw(before.unwrap_or(look.before * 20.0)),
        after: tw(after.unwrap_or(look.after * 20.0)),
    }
}

/// The paragraphs of one note, up to its end tag, with the spacing and
/// size their style gives them.
fn read_note_body(r: &mut Reader, ctx: &Ctx, rels: &HashMap<String, String>, tag: &str, looks: &PlainLooks) -> Vec<Paragraph> {
    let empty = NoteMap::new();
    let mut d = DocReader::new(ctx, &empty);
    loop {
        // Past the document's paragraphs, the rest is left out.
        if ctx.paras.get() > apex_doc::MAX_PARAGRAPHS {
            r.skip();
            break;
        }
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => match t.local() {
                "p" => d.start_para(),
                "pPr" => match &mut d.cur {
                    Some(st) => read_ppr(r, st, ctx),
                    None => r.skip(),
                },
                "r" => read_run(r, &mut d),
                "hyperlink" => {
                    let anchor = t.attr("w:anchor").filter(|a| !a.is_empty()).map(|a| format!("#{a}"));
                    let url = t.attr("r:id").and_then(|id| rels.get(id.as_ref()).cloned());
                    let link = url.or(anchor).map(|l| d.target(&l));
                    d.push_link(link.map(|l| (l, None)));
                }
                "fldSimple" => d.simple_field(&t, true),
                "tbl" | "drawing" | "pict" | "sdtPr" | "del" | "AlternateContent" => r.skip(),
                _ => {}
            },
            Ev::Empty(t) if t.local() == "fldSimple" => d.simple_field(&t, false),
            Ev::End(n) => match local(n) {
                "p" => d.finish_para(),
                "hyperlink" => d.pop_link(),
                "fldSimple" => d.end_simple_field(),
                n if n == tag => break,
                _ => {}
            },
            _ => {}
        }
    }
    d.finish_para();
    // Named styles aren't kept in notes: what they set is.
    for p in &mut d.out {
        let look = looks.of(d.style_ids.id(p.style.custom));
        p.style.custom = 0;
        let st = &mut p.style;
        st.space_before.get_or_insert(look.before);
        st.space_after.get_or_insert(look.after);
        if st.line_rule == LineRule::Multiple && st.line_spacing == 0.0 {
            st.line_spacing = look.line;
        }
        for r in &mut p.runs {
            if r.attrs.size.is_none() {
                r.attrs.size = look.size;
            }
            if r.attrs.font.is_none() {
                r.attrs.font.clone_from(&look.font);
            }
        }
    }
    d.out
}

/// A note from its paragraphs as read: the space (or tab) after its number goes,
/// and text without a size gets the note size.
fn note_from(kind: NoteKind, mut paras: Vec<Paragraph>) -> Note {
    // Notes have no tables: their cells' text stays, a paragraph each.
    for p in &mut paras {
        p.cell = None;
    }
    if paras.is_empty() {
        return Note::new(kind);
    }
    for p in &mut paras {
        p.style = ParaStyle { kind: apex_doc::ParaKind::Normal, toc: None, ..p.style };
        for r in &mut p.runs {
            if r.attrs.size.is_none() {
                r.attrs.size = Some(NOTE_SIZE);
            }
            r.attrs.note = None;
        }
    }
    let mut n = Note { kind, paras };
    let text = n.text();
    if let Some(rest) = text.strip_prefix([' ', '\t']) {
        n.set_text(rest);
    }
    n
}

pub(crate) fn format_of(v: &str) -> Option<NoteFormat> {
    Some(match v {
        "decimal" => NoteFormat::Arabic,
        "lowerRoman" => NoteFormat::LowerRoman,
        "upperRoman" => NoteFormat::UpperRoman,
        "lowerLetter" => NoteFormat::LowerLetter,
        "upperLetter" => NoteFormat::UpperLetter,
        "chicago" => NoteFormat::Symbols,
        _ => return None,
    })
}

pub(crate) fn format_name(f: NoteFormat) -> &'static str {
    match f {
        NoteFormat::Arabic => "decimal",
        NoteFormat::LowerRoman => "lowerRoman",
        NoteFormat::UpperRoman => "upperRoman",
        NoteFormat::LowerLetter => "lowerLetter",
        NoteFormat::UpperLetter => "upperLetter",
        NoteFormat::Symbols => "chicago",
    }
}

/// Note numbering from `w:footnotePr` / `w:endnotePr` (settings first, then
/// the section's, which win).
pub(super) fn read_config(z: &ZipReader, document: &str) -> NotesConfig {
    let mut cfg = NotesConfig::default();
    for xml in [z.read_string("word/settings.xml").unwrap_or_default().as_str(), document] {
        let mut r = Reader::new(xml);
        let mut cur: Option<NoteKind> = None;
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) => match t.local() {
                    "footnotePr" => cur = Some(NoteKind::Foot),
                    "endnotePr" => cur = Some(NoteKind::End),
                    _ => {}
                },
                Ev::End(n) if matches!(local(n), "footnotePr" | "endnotePr") => cur = None,
                Ev::Empty(t) => {
                    let Some(k) = cur else { continue };
                    let s = match k {
                        NoteKind::Foot => &mut cfg.foot,
                        NoteKind::End => &mut cfg.end,
                    };
                    let v = t.attr("w:val");
                    match t.local() {
                        "numFmt" => {
                            if let Some(f) = v.as_deref().and_then(format_of) {
                                s.format = f;
                            }
                        }
                        "numStart" => {
                            if let Some(n) = v.and_then(|v| v.parse::<u32>().ok()) {
                                s.start = n.clamp(1, 9999);
                            }
                        }
                        "numRestart" => {
                            s.restart = match v.as_deref() {
                                Some("eachPage") => NoteRestart::EachPage,
                                Some("eachSect") => NoteRestart::EachSection,
                                _ => NoteRestart::Continuous,
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    cfg
}

/// Settings of a TOC field instruction (`TOC \o "1-3" \h \z \u`).
pub(super) fn toc_opts(instr: &str) -> Option<TocOpts> {
    let w = apex_doc::words(instr);
    if w.first().is_some_and(|k| k.eq_ignore_ascii_case("INDEX")) {
        // `INDEX \e "<tab>" \h "A"`: the separator before the pages, letter headings.
        let mut o = TocOpts { index: true, links: false, right_align: false, ..TocOpts::default() };
        for (i, x) in w.iter().enumerate() {
            match x.as_str() {
                "\\e" => o.right_align = w.get(i + 1).is_some_and(|s| s.contains('\t')),
                "\\h" => o.letters = true,
                _ => {}
            }
        }
        return Some(o);
    }
    if w.first().is_some_and(|k| k.eq_ignore_ascii_case("TOA")) {
        // `TOA \h \c "1" \p`: one category; `\e` sets the separator.
        let mut o = TocOpts { toa: 1, links: false, ..TocOpts::default() };
        for (i, x) in w.iter().enumerate() {
            match x.as_str() {
                "\\c" => o.toa = w.get(i + 1).and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(1).clamp(1, 16),
                "\\e" => o.right_align = w.get(i + 1).is_some_and(|s| s.contains('\t')),
                _ => {}
            }
        }
        return Some(o);
    }
    if w.first().is_some_and(|k| k.eq_ignore_ascii_case("BIBLIOGRAPHY")) {
        return Some(TocOpts { bib: true, links: false, ..TocOpts::default() });
    }
    if !w.first().is_some_and(|k| k.eq_ignore_ascii_case("TOC")) {
        return None;
    }
    let mut o = TocOpts { links: false, ..TocOpts::default() };
    let mut i = 1;
    while i < w.len() {
        match w[i].as_str() {
            "\\o" => {
                if let Some(range) = w.get(i + 1).filter(|a| !a.starts_with('\\')) {
                    if let Some(hi) = range.split('-').next_back().and_then(|x| x.trim().parse::<u8>().ok()) {
                        o.levels = hi.clamp(1, 9);
                    }
                    i += 1;
                }
            }
            "\\h" => o.links = true,
            "\\c" => {
                if let Some(label) = w.get(i + 1).filter(|a| !a.starts_with('\\')) {
                    o.caption = apex_doc::caption_kind(label).max(1);
                    i += 1;
                }
            }
            "\\n" => o.page_numbers = false,
            "\\p" => {
                o.right_align = false;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    Some(o)
}

/// Level of a "toc N" paragraph style.
pub(super) fn toc_level(style_name: &str) -> Option<u8> {
    let n = style_name.to_lowercase();
    if n == "table of figures" || n == "tabla de ilustraciones" || n == "bibliography" || n == "bibliografía" || n == "table of authorities" || n == "tabla de autoridades" {
        return Some(1);
    }
    let rest = n.strip_prefix("toc").or_else(|| n.strip_prefix("tdc")).or_else(|| n.strip_prefix("index").filter(|r| !r.trim().starts_with('h')))?;
    rest.trim().parse::<u8>().ok().filter(|l| (1..=9).contains(l))
}

impl DocReader<'_> {
    /// `w:fldSimple`: links (see the caller) and fields; `open` when it has
    /// a result inside.
    pub(super) fn simple_field(&mut self, t: &Tag, open: bool) {
        let instr = t.attr("w:instr").map(|s| s.into_owned()).unwrap_or_default();
        let link = super::parse_hyperlink_field(&instr);
        let field = if link.is_none() && self.capture.is_none() { Field::parse(&instr, "") } else { None };
        if !open {
            if let Some(f) = field {
                self.push_field(f, None);
            }
            return;
        }
        self.push_link(link);
        let capturing = field.is_some();
        if let Some(f) = field {
            self.capture = Some((f, String::new(), None));
        }
        self.simple.push(capturing);
    }

    pub(super) fn end_simple_field(&mut self) {
        self.pop_link();
        if self.simple.pop() == Some(true) {
            self.finish_capture();
        }
    }

    /// The field whose result was being read goes in as one character.
    pub(super) fn finish_capture(&mut self) {
        if let Some((mut f, text, attrs)) = self.capture.take() {
            f.cached = text;
            self.push_field(f, attrs);
        }
    }

    pub(super) fn push_field(&mut self, f: Field, attrs: Option<Attrs>) {
        let mut a = attrs.unwrap_or_default();
        a.clear_object();
        a.field = Some(Arc::new(f));
        self.push_text(&OBJECT.to_string(), &a);
    }

    /// `w:footnoteReference` / `w:endnoteReference`.
    pub(super) fn note_ref(&mut self, t: &Tag, kind: NoteKind, attrs: &Attrs) {
        let Some(id) = t.attr("w:id") else { return };
        let Some(n) = self.notes.get(&(kind, id.into_owned())).cloned() else { return };
        let mut a = attrs.clone();
        a.clear_object();
        (a.superscript, a.subscript) = (false, false);
        a.note = Some(n);
        self.push_text(&OBJECT.to_string(), &a);
    }

    /// Marks a paragraph read inside a TOC field as an entry.
    pub(super) fn toc_mark(&mut self, style: &mut ParaStyle, p: &Paragraph, style_name: Option<&str>) {
        let in_toc = self.toc.is_some() || std::mem::take(&mut self.toc_seen);
        if self.toc.is_some() {
            self.toc_seen = false;
        }
        if !in_toc || p.text.trim().is_empty() || matches!(style.kind, apex_doc::ParaKind::Heading(_) | apex_doc::ParaKind::Title) {
            return;
        }
        let level = style_name.and_then(toc_level).unwrap_or(0);
        if level > 0 && style.indent_left == 0.0 && style.level == 0 {
            style.indent_left = (level - 1) as f32 * apex_doc::TOC_INDENT_PT;
        }
        style.toc = Some(TocMark { level, opts: self.toc_last });
    }
}

// ---------------------------------------------------------------------------
// Writing

/// Notes written so far, as XML, and their links.
#[derive(Default)]
pub(super) struct NoteWriter {
    foot: Vec<String>,
    end: Vec<String>,
    foot_links: Vec<Arc<str>>,
    end_links: Vec<Arc<str>>,
    /// Each note written, by its address: references sharing one share
    /// its id (a note can have many).
    ids: HashMap<usize, usize>,
}

/// The spacing of a note paragraph where it isn't our note style's (none
/// around it, single lines).
fn note_spacing(s: &mut String, st: &ParaStyle) {
    let mut sp = String::new();
    for (k, v) in [("before", st.space_before), ("after", st.space_after)] {
        if let Some(v) = v.filter(|&v| v != 0.0) {
            let _ = write!(sp, r#" w:{k}="{}""#, (v * TWIP).round() as i32);
        }
    }
    match st.line_rule {
        LineRule::Multiple if st.line_spacing > 0.0 && (st.line_spacing - 1.0).abs() > 0.005 => {
            let _ = write!(sp, r#" w:line="{}" w:lineRule="auto""#, (st.line_spacing * 240.0).round() as i32);
        }
        LineRule::Exact | LineRule::AtLeast if st.line_spacing > 0.0 => {
            let rule = if st.line_rule == LineRule::Exact { "exact" } else { "atLeast" };
            let _ = write!(sp, r#" w:line="{}" w:lineRule="{rule}""#, (st.line_spacing * TWIP).round() as i32);
        }
        _ => {}
    }
    if !sp.is_empty() {
        let _ = write!(s, "<w:spacing{sp}/>");
    }
}

/// Paragraph and run properties of the note separators: as the file had
/// them, else a 12 pt line (what ApexDesk leaves for it).
fn separator_xml(sep: Option<NoteSeparator>) -> (String, String) {
    let Some(s) = sep else { return (r#"<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="exact"/></w:pPr>"#.into(), String::new()) };
    let rule = match s.rule {
        LineRule::Multiple => "auto",
        LineRule::Exact => "exact",
        LineRule::AtLeast => "atLeast",
    };
    let line = if s.rule == LineRule::Multiple { (s.line as f32 * 2.4).round() as u32 } else { s.line as u32 };
    (
        format!(r#"<w:pPr><w:spacing w:before="{}" w:after="{}" w:line="{line}" w:lineRule="{rule}"/></w:pPr>"#, s.before, s.after),
        format!(r#"<w:rPr><w:sz w:val="{0}"/><w:szCs w:val="{0}"/></w:rPr>"#, s.size),
    )
}

/// Writes a note mark or field (`text` holds its `OBJECT` characters).
/// False for other text.
pub(super) fn write_object(body: &mut String, text: &str, a: &Attrs, nw: &mut NoteWriter) -> bool {
    if let Some(n) = &a.note {
        for _ in text.matches(OBJECT) {
            let (tag, style) = match n.kind {
                NoteKind::Foot => ("footnoteReference", "FootnoteReference"),
                NoteKind::End => ("endnoteReference", "EndnoteReference"),
            };
            let id = nw.add(n);
            let _ = write!(body, r#"<w:r><w:rPr><w:rStyle w:val="{style}"/><w:vertAlign w:val="superscript"/></w:rPr><w:{tag} w:id="{id}"/></w:r>"#);
        }
        return true;
    }
    let Some(f) = &a.field else { return false };
    let plain = Attrs { image: None, note: None, field: None, link: None, link_tip: None, ..a.clone() };
    let mut rpr = String::new();
    write_rpr(&mut rpr, &plain, false);
    for _ in text.matches(OBJECT) {
        match &f.kind {
            // Index pages are text of the INDEX field's result.
            FieldKind::IndexPages(_) | FieldKind::AuthorityPages(_) => {
                let _ = write!(body, "<w:r>{rpr}");
                write_text(body, &f.cached);
                body.push_str("</w:r>");
                continue;
            }
            // XE fields have no result.
            FieldKind::IndexEntry(_) | FieldKind::Authority { .. } => {
                let _ = write!(body, r#"<w:r>{rpr}<w:fldChar w:fldCharType="begin"/></w:r><w:r>{rpr}<w:instrText xml:space="preserve">{}</w:instrText></w:r><w:r>{rpr}<w:fldChar w:fldCharType="end"/></w:r>"#, esc(&f.instr()));
                continue;
            }
            _ => {}
        }
        let _ = write!(
            body,
            r#"<w:r>{rpr}<w:fldChar w:fldCharType="begin"/></w:r><w:r>{rpr}<w:instrText xml:space="preserve">{}</w:instrText></w:r><w:r>{rpr}<w:fldChar w:fldCharType="separate"/></w:r><w:r>{rpr}"#,
            esc(&f.instr())
        );
        write_text(body, &f.cached);
        let _ = write!(body, r#"</w:r><w:r>{rpr}<w:fldChar w:fldCharType="end"/></w:r>"#);
    }
    true
}

impl NoteWriter {
    /// Adds a note and returns its id (1-based per kind).
    fn add(&mut self, n: &Arc<Note>) -> usize {
        if let Some(&id) = self.ids.get(&(Arc::as_ptr(n) as usize)) {
            return id;
        }
        let id = self.add_new(n);
        self.ids.insert(Arc::as_ptr(n) as usize, id);
        id
    }

    fn add_new(&mut self, n: &Note) -> usize {
        let (kind, style, reference, tag) = match n.kind {
            NoteKind::Foot => ("footnote", "FootnoteText", "FootnoteReference", "footnoteRef"),
            NoteKind::End => ("endnote", "EndnoteText", "EndnoteReference", "endnoteRef"),
        };
        let mut links = std::mem::take(if n.kind == NoteKind::Foot { &mut self.foot_links } else { &mut self.end_links });
        let id = match n.kind {
            NoteKind::Foot => self.foot.len() + 1,
            NoteKind::End => self.end.len() + 1,
        };
        let mut s = String::new();
        let _ = write!(s, r#"<w:{kind} w:id="{id}">"#);
        // Pictures in notes aren't written (their relationships belong to
        // the document part).
        let mut media = docx_img::Media::default();
        let mut inner = NoteWriter::default();
        for (i, p) in n.paras.iter().enumerate() {
            let _ = write!(s, r#"<w:p><w:pPr><w:pStyle w:val="{style}"/>"#);
            note_spacing(&mut s, &p.style);
            match p.style.align {
                apex_doc::Align::Center => s.push_str(r#"<w:jc w:val="center"/>"#),
                apex_doc::Align::Right => s.push_str(r#"<w:jc w:val="right"/>"#),
                apex_doc::Align::Justify => s.push_str(r#"<w:jc w:val="both"/>"#),
                apex_doc::Align::Left => {}
            }
            s.push_str("</w:pPr>");
            if i == 0 {
                let _ = write!(s, r#"<w:r><w:rPr><w:rStyle w:val="{reference}"/><w:vertAlign w:val="superscript"/></w:rPr><w:{tag}/></w:r><w:r><w:t xml:space="preserve"> </w:t></w:r>"#);
            }
            for (a, b, attrs) in p.spans() {
                if a == b || attrs.image.is_some() || attrs.note.is_some() {
                    continue;
                }
                write_run(&mut s, &p.text[a..b], attrs, &mut links, &mut media, &mut inner);
            }
            s.push_str("</w:p>");
        }
        let _ = write!(s, "</w:{kind}>");
        match n.kind {
            NoteKind::Foot => {
                self.foot.push(s);
                self.foot_links = links;
            }
            NoteKind::End => {
                self.end.push(s);
                self.end_links = links;
            }
        }
        id
    }

    /// The parts to add: path, XML, content type, relationship type and the
    /// part's own relationships.
    pub(super) fn parts(&self, sep: Option<NoteSeparator>) -> Vec<(String, String, &'static str, &'static str, Option<String>)> {
        let mut out = Vec::new();
        let (sep_ppr, sep_rpr) = separator_xml(sep);
        for (notes, links, kind) in [(&self.foot, &self.foot_links, "footnote"), (&self.end, &self.end_links, "endnote")] {
            if notes.is_empty() {
                continue;
            }
            let mut s = String::from(HEADER);
            let _ = write!(
                s,
                r#"<w:{kind}s xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:{kind} w:type="separator" w:id="-1"><w:p>{sep_ppr}<w:r>{sep_rpr}<w:separator/></w:r></w:p></w:{kind}><w:{kind} w:type="continuationSeparator" w:id="0"><w:p>{sep_ppr}<w:r>{sep_rpr}<w:continuationSeparator/></w:r></w:p></w:{kind}>"#
            );
            for n in notes {
                s.push_str(n);
            }
            let _ = write!(s, "</w:{kind}s>");
            let rels = (!links.is_empty()).then(|| {
                let mut r = String::from(HEADER);
                r.push_str(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
                for (i, l) in links.iter().enumerate() {
                    let _ = write!(r, r#"<Relationship Id="rIdL{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="{}" TargetMode="External"/>"#, i + 1, esc(l));
                }
                r.push_str("</Relationships>");
                r
            });
            let (ct, rel) = if kind == "footnote" {
                ("application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml", "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes")
            } else {
                ("application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml", "http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes")
            };
            out.push((format!("{kind}s.xml"), s, ct, rel, rels));
        }
        out
    }

    pub(super) fn is_empty(&self) -> bool {
        self.foot.is_empty() && self.end.is_empty()
    }
}

fn note_pr(s: &NoteSettings, def: &NoteSettings) -> String {
    let mut out = String::new();
    if s.format != def.format {
        let _ = write!(out, r#"<w:numFmt w:val="{}"/>"#, format_name(s.format));
    }
    if s.start != def.start {
        let _ = write!(out, r#"<w:numStart w:val="{}"/>"#, s.start);
    }
    match s.restart {
        NoteRestart::Continuous => {}
        NoteRestart::EachPage => out.push_str(r#"<w:numRestart w:val="eachPage"/>"#),
        NoteRestart::EachSection => out.push_str(r#"<w:numRestart w:val="eachSect"/>"#),
    }
    out
}

/// `w:footnotePr` and `w:endnotePr` of the section.
pub(super) fn sect_pr(doc: &Document) -> String {
    let def = NotesConfig::default();
    let (f, e) = (note_pr(&doc.notes.foot, &def.foot), note_pr(&doc.notes.end, &def.end));
    let mut s = String::new();
    if !f.is_empty() {
        let _ = write!(s, "<w:footnotePr>{f}</w:footnotePr>");
    }
    if !e.is_empty() {
        let _ = write!(s, "<w:endnotePr>{e}</w:endnotePr>");
    }
    s
}

/// The settings' note properties, naming the separator notes.
pub(super) fn settings(doc: &Document, nw: &NoteWriter) -> String {
    if nw.is_empty() {
        return String::new();
    }
    let def = NotesConfig::default();
    // The separators are named only for the kinds written (their part).
    let mut s = String::new();
    if !nw.foot.is_empty() {
        let _ = write!(s, r#"<w:footnotePr>{}<w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr>"#, note_pr(&doc.notes.foot, &def.foot));
    }
    if !nw.end.is_empty() {
        let _ = write!(s, r#"<w:endnotePr>{}<w:endnote w:id="-1"/><w:endnote w:id="0"/></w:endnotePr>"#, note_pr(&doc.notes.end, &def.end));
    }
    s
}

/// Paragraph style of a table of contents entry.
pub(super) fn toc_style(st: &ParaStyle) -> Option<String> {
    let t = st.toc?;
    if t.opts.index {
        return (t.level > 0).then(|| format!("Index{}", t.level.min(9)));
    }
    if t.opts.bib {
        return (t.level > 0).then(|| "Bibliography".to_string());
    }
    if t.opts.toa > 0 {
        return (t.level > 0).then(|| "TableofAuthorities".to_string());
    }
    if t.opts.caption > 0 {
        return (t.level > 0).then(|| "TableofFigures".to_string());
    }
    (t.level > 0).then(|| format!("TOC{}", t.level.min(9)))
}

/// The right tab of a table of contents entry.
pub(super) fn toc_tabs(ppr: &mut String, st: &ParaStyle, doc: &Document) {
    let Some(t) = st.toc.filter(|t| t.level > 0 && t.opts.page_numbers && t.opts.right_align) else { return };
    let pos = (doc.page.content_width() * TWIP).round() as i32;
    let _ = write!(ppr, r#"<w:tabs><w:tab w:val="right" w:leader="{}" w:pos="{pos}"/></w:tabs>"#, if t.opts.leader { "dot" } else { "none" });
}

/// Opens the TOC field at the first paragraph of a table of contents.
pub(super) fn toc_begin(body: &mut String, doc: &Document, idx: usize) {
    let Some(t) = doc.paragraphs[idx].style.toc else { return };
    if idx > 0 && doc.paragraphs[idx - 1].style.toc.is_some() {
        return;
    }
    let o = t.opts;
    if o.toa > 0 {
        let sep = if o.right_align { "" } else { r#" \e ", ""# };
        let _ = write!(body, r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#, esc(&format!(" TOA \\c \"{}\"{sep} ", o.toa)));
        return;
    }
    if o.bib {
        body.push_str(r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> BIBLIOGRAPHY </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#);
        return;
    }
    if o.index {
        let mut instr = String::from(" INDEX");
        if o.right_align {
            instr.push_str(" \\e \"\t\"");
        }
        if o.letters {
            instr.push_str(" \\h \"A\"");
        }
        instr.push(' ');
        let _ = write!(body, r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#, esc(&instr));
        return;
    }
    let mut instr = if o.caption > 0 {
        let label = doc.caption_label(o.caption).unwrap_or_else(|| ["Figure", "Table", "Equation"][(o.caption as usize - 1).min(2)].to_string());
        format!(r#" TOC \c "{}""#, label.replace('"', ""))
    } else {
        format!(r#" TOC \o "1-{}""#, o.levels.clamp(1, 9))
    };
    if o.links {
        instr.push_str(" \\h");
    }
    if !o.page_numbers {
        instr.push_str(" \\n");
    }
    instr.push_str(" \\z \\u ");
    let _ = write!(body, r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#, esc(&instr));
}

/// Closes the TOC field at the last paragraph of a table of contents.
pub(super) fn toc_end(body: &mut String, doc: &Document, idx: usize) {
    if doc.paragraphs[idx].style.toc.is_some() && doc.paragraphs.get(idx + 1).is_none_or(|p| p.style.toc.is_none()) {
        body.push_str(r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#);
    }
}

/// Styles for notes and tables of contents.
pub(super) fn styles() -> String {
    let mut s = String::new();
    for (id, name) in [("FootnoteText", "footnote text"), ("EndnoteText", "endnote text")] {
        let _ = write!(s, r#"<w:style w:type="paragraph" w:styleId="{id}"><w:name w:val="{name}"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>"#);
    }
    for (id, name) in [("FootnoteReference", "footnote reference"), ("EndnoteReference", "endnote reference")] {
        let _ = write!(s, r#"<w:style w:type="character" w:styleId="{id}"><w:name w:val="{name}"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>"#);
    }
    s.push_str(r#"<w:style w:type="paragraph" w:styleId="TableofAuthorities"><w:name w:val="table of authorities"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:pPr><w:ind w:left="240" w:hanging="240"/></w:pPr></w:style>"#);
    s.push_str(r#"<w:style w:type="paragraph" w:styleId="Bibliography"><w:name w:val="Bibliography"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="37"/><w:unhideWhenUsed/></w:style>"#);
    s.push_str(r#"<w:style w:type="paragraph" w:styleId="TableofFigures"><w:name w:val="table of figures"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/></w:style>"#);
    for n in 1..=9 {
        let _ = write!(s, r#"<w:style w:type="paragraph" w:styleId="Index{n}"><w:name w:val="index {n}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:ind w:left="{}" w:hanging="220"/></w:pPr></w:style>"#, 220 * n);
    }
    for n in 1..=9 {
        let _ = write!(s, r#"<w:style w:type="paragraph" w:styleId="TOC{n}"><w:name w:val="toc {n}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:pPr><w:spacing w:after="100"/></w:pPr></w:style>"#);
    }
    s
}

/// A document with notes, fields and a table of contents (for tests).
#[cfg(test)]
pub(crate) fn refs_sample() -> Document {
    use apex_doc::{ParaKind, Pos, Selection};
    let h = |t: &str, l: u8| Paragraph::new(t).with_style(ParaStyle { kind: ParaKind::Heading(l), ..Default::default() });
    let mut d = Document::from_paragraphs(vec![h("Intro", 1), Paragraph::new("Some text here."), h("Details", 2), Paragraph::new("Page  of "), h("End", 1), Paragraph::new("Last.")]);
    // A footnote after "Some", an endnote after "Last".
    let at = Pos::new(1, 4);
    d.insert_note(Selection::caret(at), NoteKind::Foot);
    let mut n = (*d.note_at(at).unwrap().1).clone();
    n.set_text("A footnote, with ");
    n.paras[0].push("bold", Attrs { bold: true, size: Some(NOTE_SIZE), ..Default::default() });
    n.set_text("A footnote, with bold\nSecond paragraph");
    d.set_note(at, n);
    let at = Pos::new(5, 4);
    d.insert_note(Selection::caret(at), NoteKind::End);
    let mut n = (*d.note_at(at).unwrap().1).clone();
    n.set_text("An endnote");
    d.set_note(at, n);
    // "{DATE}Page {PAGE} of {NUMPAGES}".
    d.insert_field(Selection::caret(Pos::new(3, 9)), Field { cached: "2".into(), ..Field::new(apex_doc::FieldKind::NumPages) });
    d.insert_field(Selection::caret(Pos::new(3, 5)), Field { cached: "1".into(), ..Field::new(apex_doc::FieldKind::Page) });
    d.insert_field(Selection::caret(Pos::new(3, 0)), Field { cached: "01/02/2026".into(), format: Some("dd/MM/yyyy".into()), ..Field::new(apex_doc::FieldKind::Date) });
    d.insert_toc(Selection::caret(Pos::new(0, 0)), TocOpts::default(), "No entries");
    d.notes.foot.format = NoteFormat::LowerLetter;
    d.notes.foot.restart = NoteRestart::EachPage;
    d.notes.end.start = 3;
    d
}

/// Checks that what `refs_sample` has survived a round trip.
#[cfg(test)]
pub(crate) fn check_refs(back: &Document, orig: &Document, formats: bool) {
    let texts = |d: &Document| d.paragraphs.iter().map(|p| p.text.clone()).collect::<Vec<_>>();
    assert_eq!(texts(back), texts(orig));
    let notes: Vec<(NoteKind, String)> = back.notes().iter().map(|(_, n)| (n.kind, n.text())).collect();
    assert_eq!(notes, vec![(NoteKind::Foot, "A footnote, with bold\nSecond paragraph".to_string()), (NoteKind::End, "An endnote".to_string())]);
    let n = &back.notes()[0].1;
    assert!(n.paras[0].attrs_at(n.paras[0].text.find("bold").unwrap() + 1).bold);
    let kinds = |d: &Document| d.fields().iter().map(|(_, f)| (f.kind.clone(), f.cached.clone(), if formats { f.format.clone() } else { None })).collect::<Vec<_>>();
    assert_eq!(kinds(back), kinds(orig));
    assert_eq!(back.toc_blocks(), orig.toc_blocks());
    for (a, b) in back.paragraphs.iter().zip(&orig.paragraphs) {
        assert_eq!(a.style.toc.map(|t| (t.level, t.opts.levels, t.opts.links, t.opts.page_numbers)), b.style.toc.map(|t| (t.level, t.opts.levels, t.opts.links, t.opts.page_numbers)), "{}", a.text);
    }
    assert_eq!(back.notes.foot.format, NoteFormat::LowerLetter);
    assert_eq!(back.notes.foot.restart, NoteRestart::EachPage);
    assert_eq!(back.notes.end.start, 3);
    // The entries still link to their headings.
    let link = back.paragraphs[0].runs[0].attrs.link.clone().unwrap();
    assert!(back.bookmark(&link[1..]).is_some());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip::ZipWriter;

    #[test]
    fn notes_fields_toc_roundtrip() {
        let d = refs_sample();
        let back = super::super::read(&super::super::write(&d)).unwrap();
        check_refs(&back, &d, true);
        // Twice gives the same.
        let again = super::super::read(&super::super::write(&back)).unwrap();
        check_refs(&again, &d, true);
    }

    #[test]
    fn word_fields() {
        // A field without a result, a simple one, a date picture, and a
        // field that isn't handled (its result stays as text).
        let body = r#"<w:p><w:fldSimple w:instr=" NUMPAGES "/><w:fldSimple w:instr=" PAGE \* MERGEFORMAT "><w:r><w:t>4</w:t></w:r></w:fldSimple><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> DATE \@ "d MMMM yyyy" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>1 May 2026</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> x</w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD name </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>kept</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
        let xml = format!(r#"{HEADER}<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#);
        let mut z = ZipWriter::new();
        z.add("word/document.xml", xml.as_bytes(), true);
        let d = super::super::read(&z.finish()).unwrap();
        let f: Vec<(apex_doc::FieldKind, String)> = d.fields().iter().map(|(_, f)| (f.kind.clone(), f.cached.clone())).collect();
        assert_eq!(f, vec![(apex_doc::FieldKind::NumPages, String::new()), (apex_doc::FieldKind::Page, "4".into()), (apex_doc::FieldKind::Date, "1 May 2026".into())]);
        assert_eq!(d.paragraphs[0].text, format!("{OBJECT}{OBJECT}{OBJECT} xkept"));
        let (p, f) = &d.fields()[2];
        assert_eq!(f.format.as_deref(), Some("d MMMM yyyy"));
        assert!(d.paragraphs[0].attrs_at(p.offset + OBJECT.len_utf8()).bold);
    }

    #[test]
    fn a_table_in_a_note_keeps_its_text() {
        let cell = |t: &str, col| {
            let mut p = Paragraph::new(t);
            p.cell = Some(apex_doc::InCell { table: 1, row: 0, col, cell: None, table_props: None, nested: None });
            p
        };
        let n = note_from(NoteKind::Foot, vec![Paragraph::new(" See:"), cell("a", 0), cell("b", 1)]);
        assert_eq!(n.paras.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["See:", "a", "b"]);
        assert!(n.paras.iter().all(|p| p.cell.is_none()));
    }
}

