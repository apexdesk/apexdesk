//! Rich Text Format (.rtf): text formatting, paragraphs, lists, tables,
//! pictures, links, bookmarks, notes, fields, headers and footers.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{
    Align, Attrs, CharFx, Document, Field, FieldKind, HfSlot, Image, LINE_BREAK, LineRule, Mark, Note, NoteKind, OBJECT, ParaKind, ParaStyle, Paragraph, TabAlign, TabLeader, TabStop, TabStops, Table, TableMap, UnderlineStyle, VAlign,
};

/// Twips per point.
const TW: f32 = 20.0;
const MONO: &str = "Courier New";

// --- writing ---------------------------------------------------------------------------------

struct Writer<'a> {
    doc: &'a Document,
    out: String,
    fonts: Vec<String>,
    /// Color table entries after the automatic one (index 0).
    colors: Vec<u32>,
    /// Lists by (kind, format), numbered from 1.
    lists: Vec<(ParaKind, u8)>,
    /// List overrides (`\lsN`, from 1): the list each one uses.
    overrides: Vec<usize>,
    /// The override of the current run of list paragraphs by list.
    cur_ls: HashMap<usize, usize>,
}

/// Deepest nesting of groups kept (Word writes a few dozen levels).
const MAX_DEPTH: usize = 1000;
/// List definitions kept from `\listtable`.
const MAX_LISTS: usize = 1024;

/// The escaped text of a string (non-ASCII as `\uN` with a code page
/// fallback).
fn escape(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '\\' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            '\t' => out.push_str("\\tab "),
            LINE_BREAK | '\n' => out.push_str("\\line "),
            '\u{AD}' => out.push_str("\\-"),
            '\u{2011}' => out.push_str("\\_"),
            '\u{A0}' => out.push_str("\\~"),
            OBJECT | '\r' => {}
            ' '..='~' => out.push(c),
            _ => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "\\u{}", *u as i16);
                    match to_cp1252(c) {
                        Some(b) if c.len_utf16() == 1 => {
                            let _ = write!(out, "\\'{b:02x}");
                        }
                        _ => out.push('?'),
                    }
                }
            }
        }
    }
}

fn to_cp1252(c: char) -> Option<u8> {
    let u = c as u32;
    if (0xA0..=0xFF).contains(&u) {
        return Some(u as u8);
    }
    (0x80..0xA0u8).find(|&b| crate::cp1252(b) == c)
}

fn tw(pt: f32) -> i32 {
    (pt * TW).round() as i32
}

fn style_index(kind: ParaKind) -> u8 {
    match kind {
        ParaKind::Heading(n) => n.clamp(1, 9),
        ParaKind::Title => 10,
        ParaKind::Quote => 11,
        ParaKind::Code => 12,
        _ => 0,
    }
}

/// Picture data as an RTF `\pict` (PNG and JPEG only).
fn picture(out: &mut String, img: &Image) {
    let d = &img.data;
    let blip = if d.starts_with(b"\x89PNG") {
        "\\pngblip"
    } else if d.starts_with(&[0xFF, 0xD8]) {
        "\\jpegblip"
    } else {
        return;
    };
    let (pw, ph) = crate::pixel_size(d).unwrap_or(((img.width / 0.75) as u32, (img.height / 0.75) as u32));
    let _ = write!(out, "{{\\*\\shppict{{\\pict{blip}\\picw{pw}\\pich{ph}\\picwgoal{}\\pichgoal{}\n", tw(img.width), tw(img.height));
    for (i, b) in d.iter().enumerate() {
        let _ = write!(out, "{b:02x}");
        if i % 64 == 63 {
            out.push('\n');
        }
    }
    out.push_str("}}");
}

impl Writer<'_> {
    fn font(&mut self, name: &str) -> usize {
        match self.fonts.iter().position(|f| f == name) {
            Some(i) => i,
            None => {
                self.fonts.push(name.to_string());
                self.fonts.len() - 1
            }
        }
    }

    fn color(&mut self, c: u32) -> usize {
        let c = c & 0xFF_FFFF;
        1 + match self.colors.iter().position(|&x| x == c) {
            Some(i) => i,
            None => {
                self.colors.push(c);
                self.colors.len() - 1
            }
        }
    }

    /// Character formatting control words for a run.
    fn char_props(&mut self, a: &Attrs, kind: ParaKind) -> String {
        let mut s = String::from("\\plain");
        if a.bold || kind.base_bold() {
            s.push_str("\\b");
        }
        if a.italic {
            s.push_str("\\i");
        }
        let fx = a.fx.as_deref();
        if a.underline {
            s.push_str(match fx.map_or(UnderlineStyle::Single, |f| f.underline) {
                UnderlineStyle::Single => "\\ul",
                UnderlineStyle::Words => "\\ulw",
                UnderlineStyle::Double => "\\uldb",
                UnderlineStyle::Thick => "\\ulth",
                UnderlineStyle::Dotted => "\\uld",
                UnderlineStyle::Dashed => "\\uldash",
                UnderlineStyle::DotDash => "\\uldashd",
                UnderlineStyle::Wave => "\\ulwave",
            });
        }
        if a.strike {
            s.push_str("\\strike");
        }
        match a.rev.as_deref().map(|r| r.kind) {
            Some(apex_doc::RevKind::Delete) => s.push_str("\\deleted"),
            Some(apex_doc::RevKind::Insert) => s.push_str("\\revised"),
            None => {}
        }
        if a.superscript {
            s.push_str("\\super");
        } else if a.subscript {
            s.push_str("\\sub");
        }
        if let Some(fx) = fx {
            if fx.small_caps {
                s.push_str("\\scaps");
            }
            if fx.all_caps {
                s.push_str("\\caps");
            }
            if fx.double_strike {
                s.push_str("\\striked1");
            }
            if fx.hidden {
                s.push_str("\\v");
            }
            if fx.spacing != 0 {
                let _ = write!(s, "\\expndtw{}", fx.spacing);
            }
        }
        let _ = write!(s, "\\fs{}", a.size.unwrap_or(self.doc.base_size(kind)));
        let font = match &a.font {
            Some(f) => {
                let f = self.doc.styles.theme.font_name(f).to_string();
                self.font(&f)
            }
            None if kind == ParaKind::Code || a.code => self.font(MONO),
            None => 0,
        };
        // East Asian and complex script fonts are associated fonts.
        let asian = a.asian.as_deref();
        if let Some(f) = asian.and_then(|x| x.font.as_deref()) {
            let lang = asian.and_then(|x| x.lang.as_deref());
            let f = self.doc.styles.theme.ea_font(f, lang).map_or(f, |x| &**x).to_string();
            if !f.starts_with('+') {
                let _ = write!(s, "\\dbch\\af{}\\loch", self.font(&f));
            }
        }
        if let Some(f) = asian.and_then(|x| x.cs_font.as_deref()) {
            let f = self.doc.styles.theme.font_name(f).to_string();
            let _ = write!(s, "\\rtlch\\af{}\\ltrch", self.font(&f));
        }
        if let Some(l) = asian.and_then(|x| x.lang.as_deref()) {
            if let Some(id) = ASIAN_LANGS.iter().find(|x| x.1.eq_ignore_ascii_case(l) || x.1.split('-').next() == Some(l)).map(|x| x.0) {
                let _ = write!(s, "\\langfe{id}");
            }
        }
        if font != 0 {
            let _ = write!(s, "\\f{font}");
        }
        if let Some(c) = a.color {
            let _ = write!(s, "\\cf{}", self.color(c));
        }
        if let Some(c) = a.highlight {
            let i = self.color(c);
            let _ = write!(s, "\\highlight{i}\\chcbpat{i}");
        }
        if a.no_proof {
            s.push_str("\\noproof");
        }
        s
    }

    /// The list override for a list paragraph.
    fn list_override(&mut self, st: &ParaStyle) -> usize {
        let key = (st.kind, if st.kind == ParaKind::Numbered { st.list_format.max(1) } else { st.list_format });
        let list = match self.lists.iter().position(|l| *l == key) {
            Some(i) => i,
            None => {
                self.lists.push(key);
                self.lists.len() - 1
            }
        };
        match self.cur_ls.get(&list) {
            Some(&ls) if !(st.restart && st.level == 0) => ls,
            _ => {
                self.overrides.push(list);
                let ls = self.overrides.len();
                self.cur_ls.insert(list, ls);
                ls
            }
        }
    }

    fn para_props(&mut self, p: &Paragraph, intbl: bool) {
        let st = &p.style;
        self.out.push_str("\\pard\\plain");
        if intbl {
            self.out.push_str("\\intbl");
        }
        let _ = write!(self.out, "\\s{}", style_index(st.kind));
        if let ParaKind::Heading(n) = st.kind {
            let _ = write!(self.out, "\\outlinelevel{}", n.clamp(1, 9) - 1);
        }
        self.out.push_str(match st.align {
            Align::Left => "\\ql",
            Align::Center => "\\qc",
            Align::Right => "\\qr",
            Align::Justify => "\\qj",
        });
        let list = st.kind.is_list();
        let (li, fi) = if list { (st.indent_left + (st.level as f32 + 1.0) * 18.0, st.first_line - 18.0) } else { (st.indent_left + st.level as f32 * 36.0, st.first_line) };
        if li.abs() > 0.01 {
            let _ = write!(self.out, "\\li{}", tw(li));
        }
        if st.indent_right.abs() > 0.01 {
            let _ = write!(self.out, "\\ri{}", tw(st.indent_right));
        }
        if fi.abs() > 0.01 {
            let _ = write!(self.out, "\\fi{}", tw(fi));
        }
        let (b, a) = st.spacing();
        let _ = write!(self.out, "\\sb{}\\sa{}", tw(b), tw(a));
        if st.line_spacing > 0.0 {
            match st.line_rule {
                LineRule::Multiple => {
                    let _ = write!(self.out, "\\sl{}\\slmult1", (st.line_spacing * 240.0).round() as i32);
                }
                LineRule::Exact => {
                    let _ = write!(self.out, "\\sl-{}\\slmult0", tw(st.line_spacing));
                }
                LineRule::AtLeast => {
                    let _ = write!(self.out, "\\sl{}\\slmult0", tw(st.line_spacing));
                }
            }
        }
        if st.page_break_before {
            self.out.push_str("\\pagebb");
        }
        if st.keep_with_next {
            self.out.push_str("\\keepn");
        }
        if st.keep_lines {
            self.out.push_str("\\keep");
        }
        self.out.push_str(if st.no_widow_control { "\\nowidctlpar" } else { "\\widctlpar" });
        if st.no_line_numbers {
            self.out.push_str("\\noline");
        }
        if let Some(c) = st.shading {
            let i = self.color(c);
            let _ = write!(self.out, "\\cbpat{i}");
        }
        for t in st.tabs.as_slice() {
            self.out.push_str(match t.align {
                TabAlign::Left => "",
                TabAlign::Center => "\\tqc",
                TabAlign::Right => "\\tqr",
                TabAlign::Decimal => "\\tqdec",
            });
            self.out.push_str(match t.leader {
                TabLeader::None => "",
                TabLeader::Dots => "\\tldot",
                TabLeader::Dashes => "\\tlhyph",
                TabLeader::Underline => "\\tlul",
            });
            let _ = write!(self.out, "\\tx{}", tw(t.pos));
        }
        if list {
            let ls = self.list_override(st);
            let _ = write!(self.out, "\\ls{ls}\\ilvl{}", st.level.min(8));
        }
        self.out.push(' ');
    }

    /// A paragraph's content: runs, links, bookmarks and objects.
    fn content(&mut self, p: &Paragraph) {
        let kind = p.style.kind;
        let mut cuts: Vec<usize> = p.marks.iter().map(|m| m.offset).collect();
        let mut pos = 0;
        let mut link: Option<Arc<str>> = None;
        let mut marks: Vec<&Mark> = p.marks.iter().filter(|m| !apex_doc::is_perm_mark(&m.name)).collect();
        marks.sort_by_key(|m| (m.offset, m.end));
        let mut mi = 0;
        cuts.sort_unstable();
        for run in &p.runs {
            let (start, end) = (pos, (pos + run.len).min(p.text.len()));
            pos = end;
            let a = &run.attrs;
            if link != a.link {
                if link.is_some() {
                    self.out.push_str("}}}");
                }
                if let Some(l) = &a.link {
                    let instr = match l.strip_prefix('#') {
                        Some(b) => format!("HYPERLINK \\l \"{}\"", b.replace('"', "")),
                        None => format!("HYPERLINK \"{}\"", l.replace('"', "%22")),
                    };
                    self.out.push_str("{\\field{\\*\\fldinst{");
                    escape(&mut self.out, &instr);
                    self.out.push_str("}}{\\fldrslt{");
                }
                link = a.link.clone();
            }
            // Split the run at bookmarks.
            let mut s = start;
            loop {
                while mi < marks.len() && marks[mi].offset <= s {
                    let m = marks[mi];
                    let _ = write!(self.out, "{{\\*\\{} ", if m.end { "bkmkend" } else { "bkmkstart" });
                    escape(&mut self.out, &m.name);
                    self.out.push('}');
                    mi += 1;
                }
                let e = cuts.iter().copied().find(|&c| c > s && c < end).unwrap_or(end);
                if let Some(text) = p.text.get(s..e) {
                    self.segment(text, a, kind);
                }
                if e >= end {
                    break;
                }
                s = e;
            }
        }
        if link.is_some() {
            self.out.push_str("}}}");
        }
        for m in &marks[mi..] {
            let _ = write!(self.out, "{{\\*\\{} ", if m.end { "bkmkend" } else { "bkmkstart" });
            escape(&mut self.out, &m.name);
            self.out.push('}');
        }
    }

    fn segment(&mut self, text: &str, a: &Attrs, kind: ParaKind) {
        if text.is_empty() {
            return;
        }
        let props = self.char_props(a, kind);
        if !text.contains(OBJECT) {
            let _ = write!(self.out, "{{{props} ");
            escape(&mut self.out, text);
            self.out.push('}');
            return;
        }
        for _ in text.matches(OBJECT) {
            if let Some(img) = &a.image {
                picture(&mut self.out, img);
            } else if let Some(n) = &a.note {
                let alt = if n.kind == NoteKind::End { "\\ftnalt" } else { "" };
                let _ = write!(self.out, "{{{props}\\super\\chftn}}{{\\footnote{alt}");
                let last = n.paras.len().saturating_sub(1);
                for (i, np) in n.paras.iter().enumerate() {
                    self.para_props(np, false);
                    if i == 0 {
                        self.out.push_str("{\\super\\chftn} ");
                    }
                    self.content(np);
                    if i < last {
                        self.out.push_str("\\par ");
                    }
                }
                self.out.push('}');
            } else if let Some(f) = &a.field {
                self.out.push_str("{\\field{\\*\\fldinst{");
                escape(&mut self.out, &f.instr());
                self.out.push_str("}}{\\fldrslt{");
                self.out.push_str(&props);
                self.out.push(' ');
                escape(&mut self.out, &f.cached);
                self.out.push_str("}}}");
            } else if let Some(eq) = &a.math {
                let _ = write!(self.out, "{{{props} ");
                escape(&mut self.out, eq.source());
                self.out.push('}');
            } else if let Some(sh) = &a.shape {
                let t = sh.shape.plain_text();
                if !t.trim().is_empty() {
                    let _ = write!(self.out, "{{{props} ");
                    escape(&mut self.out, &t);
                    self.out.push('}');
                }
            }
        }
        let rest: String = text.chars().filter(|&c| c != OBJECT).collect();
        if !rest.is_empty() {
            let _ = write!(self.out, "{{{props} ");
            escape(&mut self.out, &rest);
            self.out.push('}');
        }
    }

    fn paragraph(&mut self, p: &Paragraph, intbl: bool) {
        self.para_props(p, intbl);
        self.content(p);
    }

    /// Writes table `m` of `paras` (the body's or a header's).
    fn table(&mut self, m: &TableMap, paras: &[Paragraph]) {
        let props = m.props(paras).cloned().unwrap_or_default();
        let bordered = props.style.is_some() || props.borders.top.is_some() || props.borders.inside_h.is_some();
        let widths: Vec<f32> = (0..m.cols).map(|c| props.widths.get(c).copied().unwrap_or(72.0)).collect();
        for r in 0..m.rows {
            self.out.push_str("\\trowd\\trgaph108");
            let _ = write!(self.out, "\\trleft{}", tw(props.indent));
            match props.align {
                Align::Center => self.out.push_str("\\trqc"),
                Align::Right => self.out.push_str("\\trqr"),
                _ => {}
            }
            if (r as u32) < props.header_rows {
                self.out.push_str("\\trhdr");
            }
            // The cells of the row: (first grid column, anchor).
            let mut cells: Vec<(usize, (usize, usize))> = Vec::new();
            let mut right = props.indent;
            let mut c = 0;
            while c < m.cols {
                let anchor = m.anchor[r * m.cols + c];
                let (_, cs) = m.span_of(anchor.0, anchor.1);
                let cs = cs.max(1).min(m.cols - c);
                let cp = m.cell_props(paras, anchor.0, anchor.1).cloned().unwrap_or_default();
                let (rs, _) = m.span_of(anchor.0, anchor.1);
                if anchor.0 < r {
                    self.out.push_str("\\clvmrg");
                } else if rs > 1 {
                    self.out.push_str("\\clvmgf");
                }
                match cp.valign {
                    VAlign::Center => self.out.push_str("\\clvertalc"),
                    VAlign::Bottom => self.out.push_str("\\clvertalb"),
                    VAlign::Top => self.out.push_str("\\clvertalt"),
                }
                if bordered {
                    self.out.push_str("\\clbrdrt\\brdrs\\brdrw10\\clbrdrl\\brdrs\\brdrw10\\clbrdrb\\brdrs\\brdrw10\\clbrdrr\\brdrs\\brdrw10");
                }
                if let Some(s) = cp.shading {
                    let i = self.color(s);
                    let _ = write!(self.out, "\\clcbpat{i}");
                }
                right += widths[c..c + cs].iter().sum::<f32>();
                let _ = write!(self.out, "\\cellx{}", tw(right));
                cells.push((c, anchor));
                c += cs;
            }
            self.out.push('\n');
            for (c, anchor) in cells {
                if anchor != (r, c) && anchor.0 != r {
                    self.out.push_str("\\pard\\plain\\intbl\\cell\n");
                    continue;
                }
                let (a, b) = m.cell(r, c);
                for i in a..b {
                    self.paragraph(&paras[i], true);
                    self.out.push_str(if i + 1 == b { "\\cell\n" } else { "\\par\n" });
                }
                if a >= b {
                    self.out.push_str("\\pard\\plain\\intbl\\cell\n");
                }
            }
            self.out.push_str("\\row\n");
        }
    }

    fn story(&mut self, paras: &[Paragraph]) {
        let tables: HashMap<usize, TableMap> = tables_in(paras).into_iter().map(|m| (m.start, m)).collect();
        let mut i = 0;
        while i < paras.len() {
            if let Some(m) = tables.get(&i) {
                self.table(m, paras);
                i = m.end.max(i + 1);
                continue;
            }
            self.paragraph(&paras[i], false);
            self.out.push_str("\\par\n");
            i += 1;
        }
    }
}

/// Maps of the tables in `paras` that aren't nested.
fn tables_in(paras: &[Paragraph]) -> Vec<TableMap> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < paras.len() {
        match paras[i].cell.is_some().then(|| TableMap::build(paras, i)).flatten() {
            Some(m) => {
                i = m.end.max(i + 1);
                out.push(m);
            }
            None => i += 1,
        }
    }
    out
}

fn info(out: &mut String, key: &str, v: &str) {
    if !v.is_empty() {
        let _ = write!(out, "{{\\{key} ");
        escape(out, v);
        out.push('}');
    }
}

/// Writes the document as RTF.
pub fn write(doc: &Document) -> Vec<u8> {
    let base = doc.default_font_token().map(|f| doc.styles.theme.font_name(&f).to_string()).unwrap_or_else(|| "Calibri".into());
    let mut w = Writer { doc, out: String::new(), fonts: vec![base], colors: Vec::new(), lists: Vec::new(), overrides: Vec::new(), cur_ls: HashMap::new() };
    // Headers and footers go first, then the body.
    let mut hf = String::new();
    let slots: [(HfSlot, &str); 6] = [
        (HfSlot::Header, if doc.odd_even { "headerr" } else { "header" }),
        (HfSlot::Footer, if doc.odd_even { "footerr" } else { "footer" }),
        (HfSlot::EvenHeader, "headerl"),
        (HfSlot::EvenFooter, "footerl"),
        (HfSlot::FirstHeader, "headerf"),
        (HfSlot::FirstFooter, "footerf"),
    ];
    for (slot, key) in slots {
        if let Some(s) = doc.headers.get(slot) {
            w.out.clear();
            w.story(s);
            let _ = write!(hf, "{{\\{key} {}}}\n", w.out);
        }
    }
    w.out.clear();
    w.story(&doc.paragraphs);
    let body = std::mem::take(&mut w.out);

    let mut out = String::from("{\\rtf1\\ansi\\ansicpg1252\\deff0");
    // The default East Asian font.
    if let Some(f) = doc.styles.asian.text.as_deref().and_then(|x| x.font.as_deref()) {
        let f = doc.styles.theme.ea_font(f, None).map_or(f, |x| &**x).to_string();
        if !f.starts_with('+') {
            let _ = write!(out, "\\stshfdbch{}", w.font(&f));
        }
    }
    out.push_str("\\uc1\n{\\fonttbl");
    for (i, f) in w.fonts.iter().enumerate() {
        let family = if f == MONO { "\\fmodern" } else { "\\fnil" };
        let _ = write!(out, "{{\\f{i}{family}\\fcharset0 ");
        escape(&mut out, f);
        out.push_str(";}");
    }
    out.push_str("}\n{\\colortbl;");
    for c in &w.colors {
        let _ = write!(out, "\\red{}\\green{}\\blue{};", c >> 16 & 0xFF, c >> 8 & 0xFF, c & 0xFF);
    }
    out.push_str("}\n{\\stylesheet{\\s0\\fs24 Normal;}");
    for n in 1..=9u8 {
        let k = ParaKind::Heading(n);
        let _ = write!(out, "{{\\s{n}\\outlinelevel{}\\sbasedon0\\snext0\\b\\fs{} heading {n};}}", n - 1, k.base_size());
    }
    let _ = write!(out, "{{\\s10\\sbasedon0\\snext0\\b\\fs{} Title;}}{{\\s11\\sbasedon0\\snext0\\i Quote;}}{{\\s12\\sbasedon0\\fs20 HTML Preformatted;}}}}\n", ParaKind::Title.base_size());
    if !w.lists.is_empty() {
        out.push_str("{\\*\\listtable");
        for (i, (kind, fmt)) in w.lists.iter().enumerate() {
            let _ = write!(out, "{{\\list\\listtemplateid{}\\listhybrid", 1000 + i);
            for lvl in 0..9u8 {
                let ind = (lvl as i32 + 1) * 360;
                if *kind == ParaKind::Numbered {
                    let (nfc, sep) = match fmt {
                        2 => (0, ')'),
                        3 => (4, ')'),
                        4 => (3, '.'),
                        5 => (2, '.'),
                        6 => (1, '.'),
                        _ => (0, '.'),
                    };
                    let _ = write!(out, "{{\\listlevel\\levelnfc{nfc}\\levelnfcn{nfc}\\leveljc0\\levelfollow0\\levelstartat1{{\\leveltext\\'02\\'0{lvl}{sep};}}{{\\levelnumbers\\'01;}}\\fi-360\\li{ind}}}");
                } else {
                    let glyph = apex_doc::BULLETS[(*fmt as usize).min(apex_doc::BULLETS.len() - 1)];
                    let mut g = String::new();
                    escape(&mut g, &glyph.to_string());
                    let _ = write!(out, "{{\\listlevel\\levelnfc23\\levelnfcn23\\leveljc0\\levelfollow0\\levelstartat1{{\\leveltext\\'01{g};}}{{\\levelnumbers;}}\\fi-360\\li{ind}}}");
                }
            }
            let _ = write!(out, "\\listid{}}}", 1 + i);
        }
        out.push_str("}\n{\\*\\listoverridetable");
        for (i, l) in w.overrides.iter().enumerate() {
            let _ = write!(out, "{{\\listoverride\\listid{}\\listoverridecount0\\ls{}}}", l + 1, i + 1);
        }
        out.push_str("}\n");
    }
    let m = &doc.meta;
    let mut inf = String::new();
    info(&mut inf, "title", &m.title);
    info(&mut inf, "subject", &m.subject);
    info(&mut inf, "author", &m.author);
    info(&mut inf, "manager", &m.manager);
    info(&mut inf, "company", &m.company);
    info(&mut inf, "category", &m.category);
    info(&mut inf, "keywords", &m.keywords);
    info(&mut inf, "doccomm", &m.description);
    if !inf.is_empty() {
        let _ = writeln!(out, "{{\\info{inf}}}");
    }
    let pg = &doc.page;
    let _ = writeln!(
        out,
        "\\paperw{}\\paperh{}\\margl{}\\margr{}\\margt{}\\margb{}\\headery{}\\footery{}{}{}{}\\widowctrl\\ftnbj\\aenddoc",
        tw(pg.width),
        tw(pg.height),
        tw(pg.margin_left),
        tw(pg.margin_right),
        tw(pg.margin_top),
        tw(pg.margin_bottom),
        tw(pg.margin_header),
        tw(pg.margin_footer),
        if pg.width > pg.height { "\\landscape" } else { "" },
        if doc.odd_even { "\\facingp" } else { "" },
        if doc.auto_hyphenation { "\\hyphauto1" } else { "" },
    );
    if pg.gutter > 0.0 {
        let _ = write!(out, "\\gutter{}", tw(pg.gutter));
    }
    if pg.mirror {
        out.push_str("\\margmirror");
    }
    if pg.gutter_top {
        out.push_str("\\gutterprl");
    }
    out.push_str("\\sectd");
    if doc.headers.different_first {
        out.push_str("\\titlepg");
    }
    out.push_str(match pg.v_align {
        apex_doc::PageVAlign::Top => "",
        apex_doc::PageVAlign::Center => "\\vertalc",
        apex_doc::PageVAlign::Justify => "\\vertalj",
        apex_doc::PageVAlign::Bottom => "\\vertalb",
    });
    out.push('\n');
    out.push_str(&hf);
    out.push_str(&body);
    out.push_str("}\n");
    out.into_bytes()
}

// --- reading ---------------------------------------------------------------------------------

enum Tok<'a> {
    Open,
    Close,
    Word(&'a str, Option<i32>),
    /// A control symbol (`\~`, `\*`...).
    Sym(u8),
    /// A byte of text in the document's code page (`\'hh` or plain).
    Byte(u8),
    Bin(&'a [u8]),
    Eof,
}

struct Lexer<'a> {
    d: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    fn next(&mut self) -> Tok<'a> {
        loop {
            let Some(&b) = self.d.get(self.i) else { return Tok::Eof };
            self.i += 1;
            match b {
                b'{' => return Tok::Open,
                b'}' => return Tok::Close,
                b'\r' | b'\n' | 0 => continue,
                b'\\' => return self.control(),
                _ => return Tok::Byte(b),
            }
        }
    }

    fn control(&mut self) -> Tok<'a> {
        let d = self.d;
        let Some(&c) = d.get(self.i) else { return Tok::Eof };
        if !c.is_ascii_alphabetic() {
            self.i += 1;
            return match c {
                b'\'' => {
                    let h = d.get(self.i..self.i + 2).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                    self.i += 2;
                    h.map_or(Tok::Sym(b'?'), Tok::Byte)
                }
                b'\\' | b'{' | b'}' => Tok::Byte(c),
                b'\r' | b'\n' => Tok::Word("par", None),
                _ => Tok::Sym(c),
            };
        }
        let s = self.i;
        while d.get(self.i).is_some_and(u8::is_ascii_alphabetic) {
            self.i += 1;
        }
        let word = std::str::from_utf8(&d[s..self.i]).unwrap_or("");
        let ns = self.i;
        if d.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        while d.get(self.i).is_some_and(u8::is_ascii_digit) {
            self.i += 1;
        }
        let num = std::str::from_utf8(&d[ns..self.i]).ok().and_then(|n| n.parse::<i64>().ok()).map(|n| n.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
        if num.is_none() {
            self.i = ns;
        }
        if d.get(self.i) == Some(&b' ') {
            self.i += 1;
        }
        if word == "bin" {
            let n = num.unwrap_or(0).max(0) as usize;
            let e = (self.i + n).min(d.len());
            let bytes = &d[self.i..e];
            self.i = e;
            return Tok::Bin(bytes);
        }
        Tok::Word(word, num)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dest {
    Text,
    Skip,
    FontTbl,
    ColorTbl,
    StyleSheet,
    Style,
    InfoGroup,
    Info(u8),
    ListTable,
    List,
    ListLevel,
    LevelText,
    OverrideTable,
    Override,
    Pict,
    FldInst,
    FldRslt,
    Bookmark(bool),
    Footnote,
    Hf(HfSlot),
    Pn,
}

#[derive(Clone)]
struct Chr {
    attrs: Attrs,
    fs: u16,
    font: i32,
    /// Which characters `\af` applies to: 2 after `\dbch` (East Asian),
    /// 3 after `\rtlch` (complex script), else 0.
    region: u8,
    /// The East Asian and complex script fonts (`\af`), -1 when unset.
    ea_font: i32,
    cs_font: i32,
}

#[derive(Clone, Default)]
struct Pp {
    style: ParaStyle,
    li: i32,
    fi: i32,
    s: i32,
    outline: Option<u8>,
    ls: i32,
    ilvl: u8,
    intbl: bool,
    /// Old-style numbering (`\pn`): bullet or numbered.
    pn: Option<ParaKind>,
    sl: i32,
    slmult: bool,
    tab_align: TabAlign,
    tab_leader: TabLeader,
    tabs: Vec<TabStop>,
}

#[derive(Clone)]
struct Group {
    dest: Dest,
    chr: Chr,
    pp: Pp,
    uc: usize,
    /// Text in the group is dropped (shape properties), but pictures in it
    /// are kept.
    hide: bool,
}

#[derive(Clone, Default)]
struct CellDef {
    right: i32,
    hmerge: bool,
    vfirst: bool,
    vmerge: bool,
    shading: Option<u32>,
    valign: VAlign,
    border: bool,
}

#[derive(Default)]
struct RowDef {
    cells: Vec<CellDef>,
    left: i32,
    align: Align,
    header: bool,
}

/// A body, note or header being read.
#[derive(Default)]
struct Story {
    paras: Vec<Paragraph>,
    cur: Paragraph,
    /// The current table: rows of (definition, cells).
    rows: Vec<(Vec<CellDef>, i32, Align, bool, Vec<Vec<Paragraph>>)>,
    cells: Vec<Vec<Paragraph>>,
    cell: Vec<Paragraph>,
    page_break: bool,
    column_break: bool,
    kind: NoteKind,
}

struct FieldState {
    depth: usize,
    instr: String,
    link: Option<Arc<str>>,
    field: Option<Field>,
    result: String,
}

#[derive(Default)]
struct Pict {
    data: Vec<u8>,
    half: Option<u8>,
    kind: u8,
    w: i32,
    h: i32,
    wgoal: i32,
    hgoal: i32,
    sx: i32,
    sy: i32,
}

#[derive(Default)]
struct ListDef {
    id: i32,
    /// Per level: number format and the text after the number.
    levels: Vec<(i32, String)>,
}

struct StyleDef {
    name: String,
    outline: Option<u8>,
}

struct Reader {
    stack: Vec<Group>,
    stories: Vec<Story>,
    fonts: HashMap<i32, String>,
    font_num: i32,
    font_name: String,
    deff: i32,
    /// Default East Asian font (`\stshfdbch`), -1 when unset.
    deff_ea: i32,
    colors: Vec<Option<u32>>,
    color: [Option<u8>; 3],
    styles: HashMap<i32, StyleDef>,
    style_num: i32,
    style_name: String,
    style_outline: Option<u8>,
    lists: Vec<ListDef>,
    level_text: String,
    overrides: HashMap<i32, i32>,
    ovr_list: i32,
    ovr_ls: i32,
    fields: Vec<FieldState>,
    pict: Option<Pict>,
    bookmark: String,
    info: [String; 8],
    row: RowDef,
    cell_def: CellDef,
    /// Cell definitions the rows read so far may still copy.
    defs_left: usize,
    /// Paragraphs and table cells the whole document may still make, in
    /// all its stories (notes have one each).
    paras_left: usize,
    /// `\uN` characters left to skip.
    skip: usize,
    high: Option<u16>,
    doc: Document,
    last_ls: i32,
}

/// Windows language ids of East Asian languages.
const ASIAN_LANGS: [(i32, &str); 6] = [(1041, "ja-JP"), (2052, "zh-CN"), (1028, "zh-TW"), (3076, "zh-HK"), (4100, "zh-SG"), (1042, "ko-KR")];

/// The language tag of an East Asian language id (`\langfe`).
fn asian_lang(id: i32) -> Option<&'static str> {
    ASIAN_LANGS.iter().find(|l| l.0 == id).map(|l| l.1)
}

fn default_chr() -> Chr {
    Chr { attrs: Attrs::default(), fs: 24, font: -1, region: 0, ea_font: -1, cs_font: -1 }
}

fn map_nfc(nfc: i32, sep: &str) -> (ParaKind, u8) {
    match nfc {
        0 if sep.contains(')') => (ParaKind::Numbered, 2),
        0 | 22 => (ParaKind::Numbered, 1),
        4 => (ParaKind::Numbered, 3),
        3 => (ParaKind::Numbered, 4),
        2 => (ParaKind::Numbered, 5),
        1 => (ParaKind::Numbered, 6),
        _ => {
            let g = sep.chars().next().unwrap_or('•');
            let f = apex_doc::BULLETS.iter().skip(1).position(|&b| b == g).map_or(0, |i| i as u8 + 1);
            (ParaKind::Bullet, if matches!(g, 'o' | '◦') { 2 } else if matches!(g, '§' | '▪' | '■') { 3 } else { f })
        }
    }
}

impl Reader {
    fn top(&mut self) -> &mut Group {
        self.stack.last_mut().expect("group")
    }

    fn story(&mut self) -> &mut Story {
        self.stories.last_mut().expect("story")
    }

    fn color_at(&self, i: i32) -> Option<u32> {
        self.colors.get(i.max(0) as usize).copied().flatten()
    }

    fn attrs(&self) -> Attrs {
        let g = self.stack.last().expect("group");
        let mut a = g.chr.attrs.clone();
        a.size = Some(g.chr.fs);
        if g.chr.font >= 0 && g.chr.font != self.deff {
            a.font = self.fonts.get(&g.chr.font).map(|f| Arc::from(f.as_str()));
        }
        // Fonts the text has anyway (Word names them on every run).
        let plain = [self.deff, self.deff_ea, g.chr.font];
        let font = |i: i32| (i >= 0 && !plain.contains(&i)).then(|| self.fonts.get(&i)).flatten().filter(|f| !f.is_empty()).map(|f| Arc::from(f.as_str()));
        let (ea, cs) = (font(g.chr.ea_font), font(g.chr.cs_font));
        if ea.is_some() || cs.is_some() {
            a.set_asian(|x| {
                x.font = ea;
                x.cs_font = cs;
            });
        }
        if let Some(f) = self.fields.last()
            && g.dest == Dest::FldRslt
            && let Some(l) = &f.link
        {
            a.link = Some(l.clone());
        }
        a
    }

    fn text(&mut self, s: &str) {
        let g = self.stack.last().expect("group");
        match g.dest {
            Dest::Text | Dest::Footnote | Dest::Hf(_) | Dest::FldRslt if !g.hide => {}
            Dest::FontTbl => {
                for c in s.chars() {
                    if c == ';' {
                        let name = std::mem::take(&mut self.font_name).trim().to_string();
                        self.fonts.insert(self.font_num, name);
                    } else {
                        self.font_name.push(c);
                    }
                }
                return;
            }
            Dest::ColorTbl => {
                for c in s.chars() {
                    if c == ';' {
                        let [r, g, b] = self.color;
                        self.colors.push(if r.is_none() && g.is_none() && b.is_none() { None } else { Some((r.unwrap_or(0) as u32) << 16 | (g.unwrap_or(0) as u32) << 8 | b.unwrap_or(0) as u32) });
                        self.color = [None; 3];
                    }
                }
                return;
            }
            Dest::Style => {
                self.style_name.push_str(s);
                return;
            }
            Dest::Info(i) => {
                self.info[i as usize].push_str(s);
                return;
            }
            Dest::LevelText => {
                self.level_text.push_str(s);
                return;
            }
            Dest::FldInst => {
                if let Some(f) = self.fields.last_mut() {
                    f.instr.push_str(s);
                }
                return;
            }
            Dest::Bookmark(_) => {
                self.bookmark.push_str(s);
                return;
            }
            Dest::Pict => {
                if let Some(p) = &mut self.pict {
                    for b in s.bytes() {
                        let v = match b {
                            b'0'..=b'9' => b - b'0',
                            b'a'..=b'f' => b - b'a' + 10,
                            b'A'..=b'F' => b - b'A' + 10,
                            _ => continue,
                        };
                        match p.half.take() {
                            Some(h) => p.data.push(h << 4 | v),
                            None => p.half = Some(v),
                        }
                    }
                }
                return;
            }
            _ => return,
        }
        if g.dest == Dest::FldRslt
            && let Some(f) = self.fields.last_mut()
            && f.field.is_some()
        {
            f.result.push_str(s);
            return;
        }
        let a = self.attrs();
        self.story().cur.push(s, a);
    }

    fn put_char(&mut self, c: char) {
        let mut b = [0u8; 4];
        self.text(c.encode_utf8(&mut b));
    }

    fn unicode(&mut self, n: i32) {
        let u = if n < 0 { (n + 65536) as u32 } else { n as u32 };
        self.skip = self.top().uc;
        if (0xD800..0xDC00).contains(&u) {
            self.high = Some(u as u16);
            return;
        }
        let c = match self.high.take() {
            Some(h) if (0xDC00..0xE000).contains(&u) => char::decode_utf16([h, u as u16]).next().and_then(Result::ok),
            _ => char::from_u32(u),
        };
        if let Some(c) = c {
            self.put_char(c);
        }
    }

    fn object(&mut self, a: Attrs) {
        self.story().cur.push(&OBJECT.to_string(), a);
    }

    /// Ends the paragraph being read (in a cell when `cell`).
    fn end_para(&mut self, cell: bool) {
        let pp = self.top().pp.clone();
        let para = self.finish_para(&pp);
        let intbl = pp.intbl || cell;
        let Some(left) = self.paras_left.checked_sub(1) else { return };
        self.paras_left = left;
        let st = self.story();
        // Past what a document keeps (`capped` says some were left out):
        // a small file of `\par`s would take gigabytes.
        if st.paras.len() + st.cells.len() + st.cell.len() > apex_doc::MAX_PARAGRAPHS {
            return;
        }
        if intbl {
            st.cell.push(para);
            if cell {
                let c = std::mem::take(&mut st.cell);
                st.cells.push(c);
            }
        } else {
            self.flush_table();
            self.story().paras.push(para);
        }
    }

    fn finish_para(&mut self, pp: &Pp) -> Paragraph {
        let mut st = pp.style.clone();
        let story = self.stories.last_mut().expect("story");
        let mut p = std::mem::replace(&mut story.cur, Paragraph::new(""));
        if std::mem::take(&mut story.page_break) {
            st.page_break_before = true;
        }
        if std::mem::take(&mut story.column_break) {
            st.column_break_before = true;
        }
        // The kind from the style or outline level.
        let def = self.styles.get(&pp.s);
        let name = def.map(|d| d.name.to_lowercase()).unwrap_or_default();
        let outline = pp.outline.or(def.and_then(|d| d.outline));
        st.kind = if let Some(n) = name.strip_prefix("heading ").or_else(|| name.strip_prefix("título ")).and_then(|n| n.trim().parse::<u8>().ok()) {
            ParaKind::Heading(n.clamp(1, 9))
        } else if name == "title" || name == "título" {
            ParaKind::Title
        } else if name.contains("quote") || name == "cita" {
            ParaKind::Quote
        } else if name.contains("preformatted") || name.contains("code") || name == "código" {
            ParaKind::Code
        } else if let Some(o) = outline.filter(|&o| o < 9) {
            ParaKind::Heading(o + 1)
        } else {
            ParaKind::Normal
        };
        // Lists.
        let mut list = pp.pn.map(|k| (k, if k == ParaKind::Numbered { 1 } else { 0 }));
        if pp.ls > 0
            && let Some(id) = self.overrides.get(&pp.ls)
            && let Some(l) = self.lists.iter().find(|l| l.id == *id)
        {
            let (nfc, sep) = l.levels.get(pp.ilvl as usize).or(l.levels.first()).cloned().unwrap_or((23, "•".into()));
            list = Some(map_nfc(nfc, &sep));
        }
        let level = pp.ilvl.min(8);
        if let Some((k, f)) = list {
            st.kind = k;
            st.list_format = f;
            st.level = level;
            st.indent_left = (pp.li as f32 / TW - (level as f32 + 1.0) * 18.0).max(0.0);
            st.first_line = pp.fi as f32 / TW + 18.0;
            if st.first_line.abs() < 0.5 {
                st.first_line = 0.0;
            }
            if st.indent_left < 0.5 {
                st.indent_left = 0.0;
            }
            if k == ParaKind::Numbered {
                if pp.ls != self.last_ls && level == 0 {
                    st.restart = crate::list_continues(&story.paras, level);
                }
                self.last_ls = pp.ls;
            }
        } else {
            st.indent_left = pp.li as f32 / TW;
            st.first_line = pp.fi as f32 / TW;
        }
        // Spacing equal to the kind's default stays the default.
        let (db, da) = st.default_spacing();
        st.space_before = st.space_before.filter(|b| (b - db).abs() > 0.05);
        st.space_after = st.space_after.filter(|a| (a - da).abs() > 0.05);
        if pp.sl != 0 {
            if pp.slmult {
                st.line_spacing = pp.sl.abs() as f32 / 240.0;
                st.line_rule = LineRule::Multiple;
                if (st.line_spacing - 1.0).abs() < 0.01 {
                    st.line_spacing = 0.0;
                }
            } else {
                st.line_spacing = pp.sl.abs() as f32 / TW;
                st.line_rule = if pp.sl < 0 { LineRule::Exact } else { LineRule::AtLeast };
            }
        }
        st.tabs = TabStops::from_slice(&pp.tabs);
        // Character formatting that the kind gives anyway.
        let kind = st.kind;
        for r in &mut p.runs {
            if r.attrs.size == Some(kind.base_size()) || r.attrs.size == Some(24) && !matches!(kind, ParaKind::Heading(_) | ParaKind::Title | ParaKind::Code) {
                r.attrs.size = None;
            }
            if kind.base_bold() {
                r.attrs.bold = false;
            }
            if kind == ParaKind::Code && r.attrs.font.as_deref().is_some_and(crate::is_mono_font) {
                r.attrs.font = None;
            }
        }
        p.runs.dedup_by(|b, a| {
            if a.attrs == b.attrs {
                a.len += b.len;
                true
            } else {
                false
            }
        });
        p.style = st;
        p
    }

    /// Turns the rows read into a table.
    fn flush_table(&mut self) {
        let story = self.stories.last_mut().expect("story");
        if !story.cells.is_empty() || !story.cell.is_empty() {
            // A row without `\row`.
            let mut cells = std::mem::take(&mut story.cells);
            if !story.cell.is_empty() {
                cells.push(std::mem::take(&mut story.cell));
            }
            story.rows.push((self.row.cells.clone(), self.row.left, self.row.align, self.row.header, cells));
        }
        if story.rows.is_empty() {
            return;
        }
        let mut rows = std::mem::take(&mut story.rows);
        rows.truncate(32767);
        // Column edges from all the rows.
        let mut edges: Vec<i32> = Vec::new();
        for (defs, left, ..) in &rows {
            edges.push(*left);
            edges.extend(defs.iter().map(|d| d.right));
        }
        edges.sort_unstable();
        edges.dedup_by(|a, b| (*a - *b).abs() < 30);
        if edges.len() < 2 {
            let n = rows.iter().map(|r| r.4.len()).max().unwrap_or(1).max(1) as i32;
            edges = (0..=n).map(|i| i * 1440).collect();
        }
        let ncols = (edges.len() - 1).min(apex_doc::MAX_COLS);
        // Short rows are filled to the widest: the cells added count (those
        // read were counted as their paragraphs ended).
        let mut keep = 0;
        for r in &rows {
            let pad = ncols.saturating_sub(r.4.len());
            if pad > self.paras_left {
                break;
            }
            self.paras_left -= pad;
            keep += 1;
        }
        if keep == 0 {
            return;
        }
        rows.truncate(keep);
        // The nearest edge (they are sorted).
        let col_of = |x: i32| {
            let i = edges.partition_point(|e| *e < x);
            if i > 0 && (i == edges.len() || x - edges[i - 1] <= edges[i] - x) { i - 1 } else { i.min(edges.len() - 1) }
        };
        let mut table = Table::new(rows.len(), ncols, 0.0);
        table.props.widths = edges.windows(2).map(|w| ((w[1] - w[0]) as f32 / TW).max(apex_doc::MIN_COL_WIDTH)).collect();
        table.props.indent = rows[0].1 as f32 / TW;
        table.props.align = rows[0].2;
        table.props.header_rows = rows.iter().take_while(|r| r.3).count() as u32;
        let bordered = rows.iter().any(|r| r.0.iter().any(|d| d.border));
        table.props.style = bordered.then(|| Arc::from("TableGrid"));
        // Each column's last row whose cell isn't covered: a merge's anchor
        // (rows may be many).
        let mut uncovered: Vec<Option<usize>> = vec![None; ncols];
        for (r, (defs, left, _, _, cells)) in rows.into_iter().enumerate() {
            let mut cells = cells.into_iter();
            let mut x = left;
            let mut prev_anchor: Option<usize> = None;
            for (i, d) in defs.iter().enumerate() {
                let (c0, c1) = (col_of(x), col_of(d.right).max(col_of(x) + 1).min(ncols));
                x = d.right;
                let paras = cells.next().unwrap_or_default();
                if c0 >= ncols {
                    continue;
                }
                if d.hmerge && i > 0 {
                    if let Some(a) = prev_anchor {
                        let span = &mut table.rows[r][a].props.col_span;
                        *span = span.saturating_add((c1 - c0) as u16);
                    }
                    for c in c0..c1 {
                        table.rows[r][c].props.covered = true;
                    }
                    continue;
                }
                if d.vmerge && r > 0 {
                    for c in c0..c1 {
                        table.rows[r][c].props.covered = true;
                    }
                    // The anchor above grows.
                    if let Some(ar) = uncovered[c0] {
                        table.rows[ar][c0].props.row_span = (r - ar + 1) as u32;
                    }
                    prev_anchor = None;
                    continue;
                }
                let cell = &mut table.rows[r][c0];
                cell.props.col_span = (c1 - c0) as u16;
                cell.props.row_span = 1;
                cell.props.shading = d.shading;
                cell.props.valign = d.valign;
                if !paras.is_empty() {
                    cell.paras = paras;
                }
                for c in c0 + 1..c1 {
                    table.rows[r][c].props.covered = true;
                }
                prev_anchor = Some(c0);
            }
            for (c, u) in uncovered.iter_mut().enumerate() {
                if !table.rows[r][c].props.covered {
                    *u = Some(r);
                }
            }
        }
        let id = story.paras.iter().filter_map(|p| p.cell.as_ref().map(|c| c.table)).max().unwrap_or(0) + 1;
        story.paras.extend(table.into_paras(id));
    }

    fn word(&mut self, w: &str, n: Option<i32>, star: bool) {
        let v = n.unwrap_or(1);
        let on = n != Some(0);
        let dest = self.top().dest;
        if dest == Dest::Skip {
            return;
        }
        // Destinations.
        let new_dest = match w {
            "fonttbl" => Some(Dest::FontTbl),
            "colortbl" => Some(Dest::ColorTbl),
            "stylesheet" => Some(Dest::StyleSheet),
            "info" => Some(Dest::InfoGroup),
            "title" | "subject" | "author" | "manager" | "company" | "category" | "keywords" | "doccomm" if dest == Dest::InfoGroup => {
                let i = ["title", "subject", "author", "manager", "company", "category", "keywords", "doccomm"].iter().position(|k| *k == w).unwrap_or(0);
                Some(Dest::Info(i as u8))
            }
            "listtable" => Some(Dest::ListTable),
            // Real documents define a handful; each paragraph looks its list up.
            "list" if dest == Dest::ListTable && self.lists.len() >= MAX_LISTS => Some(Dest::Skip),
            "list" if dest == Dest::ListTable => {
                self.lists.push(ListDef::default());
                Some(Dest::List)
            }
            "listlevel" if dest == Dest::List => {
                if let Some(l) = self.lists.last_mut() {
                    l.levels.push((0, String::new()));
                }
                Some(Dest::ListLevel)
            }
            "leveltext" if dest == Dest::ListLevel => {
                self.level_text.clear();
                Some(Dest::LevelText)
            }
            "listoverridetable" => Some(Dest::OverrideTable),
            "listoverride" if dest == Dest::OverrideTable => {
                self.ovr_list = 0;
                self.ovr_ls = 0;
                Some(Dest::Override)
            }
            "pict" => {
                self.pict = Some(Pict { sx: 100, sy: 100, ..Default::default() });
                Some(Dest::Pict)
            }
            "shppict" | "shp" | "shpinst" | "shptxt" => Some(dest),
            "sv" => {
                self.top().hide = true;
                None
            }
            "nonshppict" | "shprslt" | "sn" | "listtext" | "pntext" | "fldtype" | "datafield" | "formfield" | "themedata" | "colorschememapping" | "latentstyles" | "datastore" | "xmlnstbl" | "rsidtbl" | "generator" | "mmathPr" | "pgdsctbl" | "listpicture" | "revtbl" | "annotation" | "atnid" | "atnauthor" | "xe" | "tc" | "object" | "objdata" | "nesttableprops" | "userprops" | "docvar" | "wgrffmtfilter" | "filetbl" | "pnseclvl" | "template" => Some(Dest::Skip),
            "field" => {
                let depth = self.stack.len();
                self.fields.push(FieldState { depth, instr: String::new(), link: None, field: None, result: String::new() });
                None
            }
            "fldinst" => Some(Dest::FldInst),
            "fldrslt" => Some(Dest::FldRslt),
            "bkmkstart" | "bkmkend" => {
                self.bookmark.clear();
                Some(Dest::Bookmark(w == "bkmkend"))
            }
            "footnote" => {
                self.stories.push(Story { cur: Paragraph::new(""), ..Default::default() });
                Some(Dest::Footnote)
            }
            "header" | "headerr" | "footer" | "footerr" | "headerl" | "footerl" | "headerf" | "footerf" => {
                let slot = match w {
                    "header" | "headerr" => HfSlot::Header,
                    "footer" | "footerr" => HfSlot::Footer,
                    "headerl" => HfSlot::EvenHeader,
                    "footerl" => HfSlot::EvenFooter,
                    "headerf" => HfSlot::FirstHeader,
                    _ => HfSlot::FirstFooter,
                };
                self.stories.push(Story { cur: Paragraph::new(""), ..Default::default() });
                Some(Dest::Hf(slot))
            }
            "pn" => Some(Dest::Pn),
            _ => None,
        };
        if let Some(d) = new_dest {
            let g = self.top();
            g.dest = d;
            if matches!(d, Dest::Footnote | Dest::Hf(_)) {
                g.pp = Pp::default();
                g.chr = default_chr();
            }
            return;
        }
        if star {
            // An unknown optional destination.
            self.top().dest = Dest::Skip;
            return;
        }
        match dest {
            Dest::Skip | Dest::InfoGroup => return,
            Dest::FontTbl => {
                if w == "f" {
                    self.font_num = v;
                    self.font_name.clear();
                }
                return;
            }
            Dest::ColorTbl => {
                let i = match w {
                    "red" => 0,
                    "green" => 1,
                    "blue" => 2,
                    _ => return,
                };
                self.color[i] = Some(v.clamp(0, 255) as u8);
                return;
            }
            Dest::StyleSheet | Dest::Style => {
                match w {
                    "s" => {
                        self.top().dest = Dest::Style;
                        self.style_num = v;
                    }
                    "cs" | "ds" | "ts" => self.top().dest = Dest::Skip,
                    "outlinelevel" => self.style_outline = Some(v.clamp(0, 9) as u8),
                    _ => {}
                }
                return;
            }
            Dest::List => {
                if w == "listid"
                    && let Some(l) = self.lists.last_mut()
                {
                    l.id = v;
                }
                return;
            }
            Dest::ListLevel => {
                if w == "levelnfc"
                    && let Some(l) = self.lists.last_mut()
                    && let Some(lv) = l.levels.last_mut()
                {
                    lv.0 = v;
                }
                return;
            }
            Dest::Override => {
                match w {
                    "listid" => self.ovr_list = v,
                    "ls" => self.ovr_ls = v,
                    _ => {}
                }
                return;
            }
            Dest::Pict => {
                if let Some(p) = &mut self.pict {
                    match w {
                        "pngblip" => p.kind = 1,
                        "jpegblip" => p.kind = 2,
                        "picw" => p.w = v,
                        "pich" => p.h = v,
                        "picwgoal" => p.wgoal = v,
                        "pichgoal" => p.hgoal = v,
                        "picscalex" => p.sx = v,
                        "picscaley" => p.sy = v,
                        _ => {}
                    }
                }
                return;
            }
            Dest::Pn => {
                let g = self.top();
                match w {
                    "pnlvlblt" => g.pp.pn = Some(ParaKind::Bullet),
                    "pnlvlbody" | "pndec" | "pnlcltr" | "pnucltr" | "pnlcrm" | "pnucrm" => g.pp.pn = Some(ParaKind::Numbered),
                    "pnlvl" => g.pp.ilvl = (v - 1).clamp(0, 8) as u8,
                    _ => {}
                }
                return;
            }
            Dest::Text | Dest::Footnote | Dest::Hf(_) | Dest::FldRslt => {}
            _ => {
                if w == "u" {
                    self.unicode(v);
                }
                return;
            }
        }
        match w {
            // Document.
            "deff" => self.deff = v,
            "stshfdbch" => self.deff_ea = v,
            "paperw" => self.doc.page.width = v as f32 / TW,
            "paperh" => self.doc.page.height = v as f32 / TW,
            "margl" | "marglsxn" => self.doc.page.margin_left = v as f32 / TW,
            "margr" | "margrsxn" => self.doc.page.margin_right = v as f32 / TW,
            "margt" | "margtsxn" => self.doc.page.margin_top = v as f32 / TW,
            "margb" | "margbsxn" => self.doc.page.margin_bottom = v as f32 / TW,
            "headery" => self.doc.page.margin_header = v as f32 / TW,
            "footery" => self.doc.page.margin_footer = v as f32 / TW,
            "pgwsxn" => self.doc.page.width = v as f32 / TW,
            "pghsxn" => self.doc.page.height = v as f32 / TW,
            "titlepg" => self.doc.headers.different_first = true,
            "gutter" | "guttersxn" => self.doc.page.gutter = v as f32 / TW,
            "margmirror" => self.doc.page.mirror = true,
            "gutterprl" => self.doc.page.gutter_top = true,
            "vertalc" => self.doc.page.v_align = apex_doc::PageVAlign::Center,
            "vertalj" => self.doc.page.v_align = apex_doc::PageVAlign::Justify,
            "vertalb" => self.doc.page.v_align = apex_doc::PageVAlign::Bottom,
            "vertalt" => self.doc.page.v_align = apex_doc::PageVAlign::Top,
            "facingp" => self.doc.odd_even = true,
            "hyphauto" => self.doc.auto_hyphenation = on,
            "uc" => self.top().uc = v.clamp(0, 10) as usize,
            "u" => self.unicode(v),
            // Special characters.
            "par" => self.end_para(false),
            "sect" => {
                if !self.story().cur.text.is_empty() {
                    self.end_para(false);
                }
            }
            "line" => self.put_char(LINE_BREAK),
            "tab" => self.put_char('\t'),
            "page" => {
                if self.story().cur.text.is_empty() {
                    self.story().page_break = true;
                } else {
                    self.end_para(false);
                    self.story().page_break = true;
                }
            }
            "column" => {
                self.end_para(false);
                self.story().column_break = true;
            }
            "emdash" => self.put_char('—'),
            "endash" => self.put_char('–'),
            "emspace" => self.put_char('\u{2003}'),
            "enspace" => self.put_char('\u{2002}'),
            "qmspace" => self.put_char('\u{2005}'),
            "bullet" => self.put_char('•'),
            "lquote" => self.put_char('‘'),
            "rquote" => self.put_char('’'),
            "ldblquote" => self.put_char('“'),
            "rdblquote" => self.put_char('”'),
            "chpgn" => {
                let mut a = self.attrs();
                a.field = Some(Arc::new(Field::new(FieldKind::Page)));
                self.object(a);
            }
            "chdate" => {
                let mut a = self.attrs();
                a.field = Some(Arc::new(Field::new(FieldKind::Date)));
                self.object(a);
            }
            "ftnalt" => self.story().kind = NoteKind::End,
            // Tables.
            "trowd" => {
                self.row = RowDef::default();
                self.cell_def = CellDef::default();
            }
            "trleft" => self.row.left = v,
            "trqc" => self.row.align = Align::Center,
            "trqr" => self.row.align = Align::Right,
            "trql" => self.row.align = Align::Left,
            "trhdr" => self.row.header = true,
            "clmgf" => {}
            "clmrg" => self.cell_def.hmerge = true,
            "clvmgf" => self.cell_def.vfirst = true,
            "clvmrg" => self.cell_def.vmerge = true,
            "clcbpat" => self.cell_def.shading = self.color_at(v),
            "clvertalc" => self.cell_def.valign = VAlign::Center,
            "clvertalb" => self.cell_def.valign = VAlign::Bottom,
            "clvertalt" => self.cell_def.valign = VAlign::Top,
            "clbrdrt" | "clbrdrb" | "clbrdrl" | "clbrdrr" => self.cell_def.border = true,
            "cellx" => {
                let mut d = std::mem::take(&mut self.cell_def);
                d.right = v;
                // Word's tables have 63 columns at most.
                if self.row.cells.len() < 4096 {
                    self.row.cells.push(d);
                }
            }
            "cell" | "nestcell" => self.end_para(true),
            "row" => {
                let st = self.stories.last_mut().expect("story");
                if !st.cell.is_empty() {
                    let c = std::mem::take(&mut st.cell);
                    st.cells.push(c);
                }
                let cells = std::mem::take(&mut st.cells);
                // Rows past what a table may hold are left out, and so is
                // a run of rows copying more definitions than any table has.
                let cost = self.row.cells.len().max(1);
                if st.rows.len() < 32767 && cost <= self.defs_left {
                    self.defs_left -= cost;
                    st.rows.push((self.row.cells.clone(), self.row.left, self.row.align, self.row.header, cells));
                }
            }
            "intbl" => self.top().pp.intbl = true,
            // Paragraphs.
            "pard" => {
                let g = self.top();
                g.pp = Pp::default();
            }
            "s" => self.top().pp.s = v,
            "outlinelevel" => self.top().pp.outline = Some(v.clamp(0, 9) as u8),
            "ql" => self.top().pp.style.align = Align::Left,
            "qc" => self.top().pp.style.align = Align::Center,
            "qr" => self.top().pp.style.align = Align::Right,
            "qj" | "qd" => self.top().pp.style.align = Align::Justify,
            "li" | "lin" => self.top().pp.li = v,
            "ri" | "rin" => self.top().pp.style.indent_right = v as f32 / TW,
            "fi" => self.top().pp.fi = v,
            "sb" => self.top().pp.style.space_before = Some(v as f32 / TW),
            "sa" => self.top().pp.style.space_after = Some(v as f32 / TW),
            "sl" => self.top().pp.sl = n.unwrap_or(0),
            "slmult" => self.top().pp.slmult = on,
            "keepn" => self.top().pp.style.keep_with_next = on,
            "keep" => self.top().pp.style.keep_lines = on,
            "pagebb" => self.top().pp.style.page_break_before = on,
            "nowidctlpar" => self.top().pp.style.no_widow_control = true,
            "widctlpar" => self.top().pp.style.no_widow_control = false,
            "noline" => self.top().pp.style.no_line_numbers = true,
            "cbpat" => {
                let c = self.color_at(v);
                self.top().pp.style.shading = c;
            }
            "ls" => self.top().pp.ls = v,
            "ilvl" => self.top().pp.ilvl = v.clamp(0, 8) as u8,
            "tqc" => self.top().pp.tab_align = TabAlign::Center,
            "tqr" => self.top().pp.tab_align = TabAlign::Right,
            "tqdec" => self.top().pp.tab_align = TabAlign::Decimal,
            "tldot" | "tlmdot" => self.top().pp.tab_leader = TabLeader::Dots,
            "tlhyph" => self.top().pp.tab_leader = TabLeader::Dashes,
            "tlul" | "tlth" => self.top().pp.tab_leader = TabLeader::Underline,
            "tx" | "tb" => {
                let pp = &mut self.top().pp;
                let stop = TabStop { pos: v as f32 / TW, align: std::mem::take(&mut pp.tab_align), leader: std::mem::take(&mut pp.tab_leader) };
                // Paragraphs keep MAX_TABS; more would only be copied
                // into every group.
                if pp.tabs.len() < 4 * apex_doc::MAX_TABS {
                    pp.tabs.push(stop);
                }
            }
            // Characters.
            "plain" => {
                let g = self.top();
                g.chr = default_chr();
            }
            "b" => self.top().chr.attrs.bold = on,
            "i" => self.top().chr.attrs.italic = on,
            "ul" | "ulw" | "uldb" | "ulth" | "uld" | "uldash" | "uldashd" | "ulwave" | "ulthd" | "ulthdash" | "uldashdd" | "ulhwave" | "ululdbwave" => {
                let style = match w {
                    "ulw" => UnderlineStyle::Words,
                    "uldb" => UnderlineStyle::Double,
                    "ulth" => UnderlineStyle::Thick,
                    "uld" | "ulthd" => UnderlineStyle::Dotted,
                    "uldash" | "ulthdash" => UnderlineStyle::Dashed,
                    "uldashd" | "uldashdd" => UnderlineStyle::DotDash,
                    "ulwave" | "ulhwave" | "ululdbwave" => UnderlineStyle::Wave,
                    _ => UnderlineStyle::Single,
                };
                let a = &mut self.top().chr.attrs;
                a.underline = on;
                if on && style != UnderlineStyle::Single {
                    Arc::make_mut(a.fx.get_or_insert_with(Default::default)).underline = style;
                }
            }
            "ulnone" => self.top().chr.attrs.underline = false,
            "strike" => self.top().chr.attrs.strike = on,
            "striked" => self.fx(|f| f.double_strike = on),
            "super" => {
                let a = &mut self.top().chr.attrs;
                a.superscript = on;
                a.subscript = false;
            }
            "sub" => {
                let a = &mut self.top().chr.attrs;
                a.subscript = on;
                a.superscript = false;
            }
            "nosupersub" => {
                let a = &mut self.top().chr.attrs;
                a.subscript = false;
                a.superscript = false;
            }
            "caps" => self.fx(|f| f.all_caps = on),
            "scaps" => self.fx(|f| f.small_caps = on),
            "v" => self.fx(|f| f.hidden = on),
            // Tracked changes: text deleted or inserted, not yet accepted
            // (who and when aren't read).
            "deleted" | "revised" => {
                let kind = if w == "deleted" { apex_doc::RevKind::Delete } else { apex_doc::RevKind::Insert };
                let a = &mut self.top().chr.attrs;
                if on {
                    a.rev = Some(apex_doc::Revision::new(kind, "", ""));
                } else if a.rev.as_ref().is_some_and(|r| r.kind == kind) {
                    a.rev = None;
                }
            }
            "expndtw" => self.fx(|f| f.spacing = v.clamp(-2000, 2000) as i16),
            "fs" => self.top().chr.fs = v.clamp(2, 3276) as u16,
            "f" => self.top().chr.font = v,
            "loch" | "hich" | "ltrch" => self.top().chr.region = 0,
            "dbch" => self.top().chr.region = 2,
            "rtlch" => self.top().chr.region = 3,
            "af" => {
                let c = &mut self.top().chr;
                match c.region {
                    2 => c.ea_font = v,
                    3 => c.cs_font = v,
                    _ => {}
                }
            }
            "langfe" | "langfenp" => {
                let l = asian_lang(v);
                self.top().chr.attrs.set_asian(|x| x.lang = l.map(Arc::from));
            }
            "cf" => {
                let c = self.color_at(v);
                self.top().chr.attrs.color = c;
            }
            "highlight" | "chcbpat" | "cb" => {
                let c = self.color_at(v);
                if c.is_some() || w == "highlight" {
                    self.top().chr.attrs.highlight = c;
                }
            }
            "noproof" => self.top().chr.attrs.no_proof = on,
            _ => {}
        }
    }

    fn fx(&mut self, f: impl FnOnce(&mut CharFx)) {
        let a = &mut self.top().chr.attrs;
        f(Arc::make_mut(a.fx.get_or_insert_with(Default::default)));
    }

    fn sym(&mut self, c: u8) {
        match c {
            b'~' => self.put_char('\u{A0}'),
            b'-' => self.put_char('\u{AD}'),
            b'_' => self.put_char('\u{2011}'),
            b'|' | b':' => {}
            _ => {}
        }
    }

    /// Closes a group, finishing what its destination was reading.
    fn close(&mut self) {
        let Some(g) = self.stack.pop() else { return };
        let parent = self.stack.last().map(|p| p.dest);
        match g.dest {
            Dest::Style if parent != Some(Dest::Style) => {
                let name = std::mem::take(&mut self.style_name).trim().trim_end_matches(';').trim().to_string();
                self.styles.insert(self.style_num, StyleDef { name, outline: self.style_outline.take() });
                self.style_num = 0;
            }
            Dest::FontTbl if parent == Some(Dest::FontTbl) => {
                let name = std::mem::take(&mut self.font_name).trim().trim_end_matches(';').to_string();
                if !name.is_empty() {
                    self.fonts.insert(self.font_num, name);
                }
            }
            Dest::LevelText if parent != Some(Dest::LevelText) => {
                // The first character is the length; numbers are \'00…\'08.
                let t: String = std::mem::take(&mut self.level_text).chars().skip(1).filter(|&c| c as u32 > 8 && c != ';').collect();
                if let Some(l) = self.lists.last_mut()
                    && let Some(lv) = l.levels.last_mut()
                {
                    lv.1 = t;
                }
            }
            Dest::Override if parent != Some(Dest::Override) => {
                self.overrides.insert(self.ovr_ls, self.ovr_list);
            }
            Dest::Pict if parent != Some(Dest::Pict) => {
                if let Some(p) = self.pict.take()
                    && p.kind != 0
                    && !p.data.is_empty()
                {
                    let (pw, ph) = crate::pixel_size(&p.data).map_or((p.w, p.h), |(w, h)| (w as i32, h as i32));
                    let w = if p.wgoal > 0 { p.wgoal as f32 / TW } else { pw as f32 * 0.75 } * p.sx.max(1) as f32 / 100.0;
                    let h = if p.hgoal > 0 { p.hgoal as f32 / TW } else { ph as f32 * 0.75 } * p.sy.max(1) as f32 / 100.0;
                    let mut a = self.attrs();
                    a.link = None;
                    a.image = Some(Arc::new(Image::new(Arc::from(p.data), w.max(1.0), h.max(1.0))));
                    self.object(a);
                }
            }
            Dest::FldInst if parent != Some(Dest::FldInst) => {
                if let Some(f) = self.fields.last_mut() {
                    let instr = f.instr.trim().to_string();
                    let words = apex_doc::words(&instr);
                    if words.first().is_some_and(|w| w.eq_ignore_ascii_case("HYPERLINK")) {
                        let mut target = String::new();
                        let mut i = 1;
                        while i < words.len() {
                            match words[i].as_str() {
                                "\\l" => {
                                    target.push('#');
                                    target.push_str(words.get(i + 1).map_or("", |s| s.as_str()));
                                    i += 1;
                                }
                                s if s.starts_with('\\') => i += 1,
                                s if target.is_empty() => target = s.to_string(),
                                _ => {}
                            }
                            i += 1;
                        }
                        f.link = (!target.is_empty()).then(|| Arc::from(target.as_str()));
                    } else {
                        f.field = Field::parse(&instr, "");
                    }
                }
            }
            Dest::Bookmark(end) if parent.is_some_and(|p| !matches!(p, Dest::Bookmark(_))) => {
                let name = std::mem::take(&mut self.bookmark).trim().to_string();
                if !name.is_empty() {
                    let cur = &mut self.story().cur;
                    let offset = cur.text.len();
                    cur.marks.push(Mark { name: Arc::from(name.as_str()), offset, end });
                }
            }
            Dest::Footnote if parent != Some(Dest::Footnote) => {
                self.stack.push(g.clone());
                if !self.story().cur.text.is_empty() || self.story().paras.is_empty() {
                    self.end_para(false);
                }
                self.flush_table();
                self.stack.pop();
                if let Some(st) = self.stories.pop() {
                    let mut paras = st.paras;
                    // The note mark at the start of the first paragraph.
                    if let Some(p) = paras.first_mut() {
                        let t = p.text.trim_start_matches([' ', '\t']).len();
                        let cut = p.text.len() - t;
                        if cut > 0 {
                            let rest = Paragraph { text: String::new(), runs: Vec::new(), ..p.clone() };
                            let mut q = rest;
                            let mut pos = 0;
                            for r in &p.runs {
                                let (s, e) = (pos, pos + r.len);
                                pos = e;
                                let s2 = s.max(cut);
                                if e > s2 {
                                    q.push(&p.text[s2..e], r.attrs.clone());
                                }
                            }
                            if q.runs.is_empty() {
                                q.runs.push(apex_doc::Run { len: 0, attrs: p.runs.first().map(|r| r.attrs.clone()).unwrap_or_default() });
                            }
                            q.marks = p.marks.iter().map(|m| Mark { offset: m.offset.saturating_sub(cut), ..m.clone() }).collect();
                            *p = q;
                        }
                    }
                    if paras.is_empty() {
                        paras.push(Paragraph::new(""));
                    }
                    let note = Note { kind: st.kind, paras };
                    // A note that was the whole file (`{\rtf\footnote }`) has
                    // no group around it: its own formatting then.
                    let outer = self.stack.is_empty();
                    if outer {
                        self.stack.push(g.clone());
                    }
                    let mut a = self.attrs();
                    if outer {
                        self.stack.pop();
                    }
                    a.superscript = false;
                    a.link = None;
                    a.note = Some(Arc::new(note));
                    self.object(a);
                }
            }
            Dest::Hf(slot) if parent != Some(Dest::Hf(slot)) => {
                self.stack.push(g.clone());
                if !self.story().cur.text.is_empty() {
                    self.end_para(false);
                }
                self.flush_table();
                self.stack.pop();
                if let Some(st) = self.stories.pop() {
                    let mut paras = st.paras;
                    if paras.is_empty() {
                        paras.push(Paragraph::new(""));
                    }
                    self.doc.headers.set(slot, Some(Arc::new(paras)));
                }
            }
            Dest::Pn => {
                // The numbering belongs to the paragraph.
                if let Some(p) = self.stack.last_mut() {
                    p.pp.pn = g.pp.pn;
                    if g.pp.pn.is_some() {
                        p.pp.ilvl = g.pp.ilvl;
                    }
                }
            }
            _ => {}
        }
        // The end of a field group.
        if self.fields.last().is_some_and(|f| f.depth == self.stack.len() + 1) {
            let f = self.fields.pop().expect("field");
            if let Some(mut field) = f.field {
                field.cached = f.result;
                let mut a = self.attrs();
                a.field = Some(Arc::new(field));
                self.object(a);
            }
        }
    }
}

/// Reads an RTF document.
pub fn read(data: &[u8]) -> Result<Document, crate::Error> {
    if !data.starts_with(b"{\\rtf") {
        return Err(crate::Error::new("not an RTF file"));
    }
    let mut r = Reader {
        stack: Vec::new(),
        stories: vec![Story { cur: Paragraph::new(""), ..Default::default() }],
        fonts: HashMap::new(),
        font_num: 0,
        font_name: String::new(),
        deff: 0,
        deff_ea: -1,
        colors: Vec::new(),
        color: [None; 3],
        styles: HashMap::new(),
        style_num: 0,
        style_name: String::new(),
        style_outline: None,
        lists: Vec::new(),
        level_text: String::new(),
        overrides: HashMap::new(),
        ovr_list: 0,
        ovr_ls: 0,
        fields: Vec::new(),
        pict: None,
        bookmark: String::new(),
        info: Default::default(),
        row: RowDef::default(),
        cell_def: CellDef::default(),
        defs_left: 4_000_000,
        paras_left: apex_doc::MAX_PARAGRAPHS,
        skip: 0,
        high: None,
        doc: Document::new(),
        last_ls: 0,
    };
    let mut lx = Lexer { d: data, i: 0 };
    let mut star = false;
    let mut text = Vec::<u8>::new();
    // Groups opened past MAX_DEPTH, not pushed.
    let mut deep = 0usize;
    loop {
        let t = lx.next();
        // Runs of plain bytes are handled together.
        if let Tok::Byte(b) = t {
            if r.skip > 0 {
                r.skip -= 1;
                continue;
            }
            text.push(b);
            continue;
        }
        if !text.is_empty() {
            let bytes = std::mem::take(&mut text);
            if r.stack.last().is_some_and(|g| g.dest == Dest::Pict) {
                r.text(&String::from_utf8_lossy(&bytes));
            } else {
                let s: String = bytes.iter().map(|&b| if b < 0x80 { b as char } else { crate::cp1252(b) }).collect();
                r.text(&s);
            }
        }
        match t {
            Tok::Eof => break,
            // Groups nested deeper than any real file keep their parent's
            // formatting (each level copies it: a file of braces would
            // take all the memory).
            Tok::Open if r.stack.len() >= MAX_DEPTH => {
                deep += 1;
                star = false;
                r.skip = 0;
            }
            Tok::Close if deep > 0 => {
                deep -= 1;
                star = false;
                r.skip = 0;
            }
            Tok::Open => {
                let g = r.stack.last().cloned().unwrap_or(Group { dest: Dest::Text, chr: default_chr(), pp: Pp::default(), uc: 1, hide: false });
                r.stack.push(g);
                star = false;
                r.skip = 0;
            }
            Tok::Close => {
                r.close();
                star = false;
                r.skip = 0;
                if r.stack.is_empty() {
                    break;
                }
            }
            Tok::Word(w, n) => {
                if r.skip > 0 {
                    r.skip -= 1;
                    continue;
                }
                r.word(w, n, star);
                star = false;
            }
            Tok::Sym(b'*') => star = true,
            Tok::Sym(c) => {
                if r.skip > 0 {
                    r.skip -= 1;
                    continue;
                }
                r.sym(c);
            }
            Tok::Bin(bytes) => {
                if let Some(p) = &mut r.pict {
                    p.data.extend_from_slice(bytes);
                }
            }
            Tok::Byte(_) => {}
        }
    }
    // What's left of the body.
    if r.stack.is_empty() {
        r.stack.push(Group { dest: Dest::Text, chr: default_chr(), pp: Pp::default(), uc: 1, hide: false });
    }
    while r.stories.len() > 1 {
        r.stories.pop();
    }
    if !r.story().cur.text.is_empty() {
        r.end_para(false);
    }
    r.flush_table();
    let story = r.stories.pop().unwrap_or_default();
    let mut doc = std::mem::replace(&mut r.doc, Document::new());
    let page = doc.page.clone();
    let headers = std::mem::take(&mut doc.headers);
    let mut out = Document::from_paragraphs(story.paras);
    out.page = page;
    out.headers = headers;
    out.odd_even = doc.odd_even;
    out.auto_hyphenation = doc.auto_hyphenation;
    if let Some(f) = r.fonts.get(&r.deff).filter(|f| !f.is_empty() && !crate::is_mono_font(f)) {
        out.default_font = Some(Arc::from(f.as_str()));
    }
    if let Some(f) = r.fonts.get(&r.deff_ea).filter(|f| !f.is_empty()) {
        out.styles.asian.text = Some(Arc::new(apex_doc::AsianAttrs { font: Some(Arc::from(f.as_str())), ..Default::default() }));
    }
    let [title, subject, author, manager, company, category, keywords, desc] = r.info.map(|s| s.trim().to_string());
    let m = &mut out.meta;
    (m.title, m.subject, m.author, m.manager, m.company, m.category, m.keywords, m.description) = (title, subject, author, manager, company, category, keywords, desc);
    out.modified = false;
    Ok(out)
}
