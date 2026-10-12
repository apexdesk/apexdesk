//! Office Open XML word processing documents (.docx).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Align, Attrs, ContentControl, Document, Field, LINE_BREAK, OBJECT, LineRule, Mark, NoteKind, ParaKind, ParaStyle, Paragraph, TocOpts, bullet_char, num_format};

use crate::xml::{Ev, HEADER, Reader, Tag, esc, local};
use crate::zip::{ZipReader, ZipWriter};
use crate::{Error, docx_img, docx_layout, highlight_color, is_mono_font, parse_hex_color};

#[path = "docx_notes.rs"]
pub(crate) mod notes;
#[cfg(test)]
pub(crate) use notes::{check_refs, refs_sample};

const TWIP: f32 = 20.0;
const MONO_FONT: &str = "Consolas";

fn on(t: &Tag) -> bool {
    match t.attr("w:val") {
        None => true,
        Some(v) => !matches!(v.as_ref(), "0" | "false" | "off" | "none"),
    }
}

// ---------------------------------------------------------------------------
// Reading

#[derive(Default, Clone)]
struct PStyleDef {
    name: String,
    based_on: Option<String>,
    outline: Option<u8>,
    num: Option<(String, u8)>,
    mono: bool,
}

/// A numbering level: bullet or not, and its `ParaStyle::list_format`.
#[derive(Clone, Copy)]
struct NumLevel {
    bullet: bool,
    format: u8,
}

impl NumLevel {
    const BULLET: NumLevel = NumLevel { bullet: true, format: 0 };
}

#[derive(Clone)]
struct NumDef {
    levels: Vec<NumLevel>,
    /// Start overrides per level.
    starts: [Option<u32>; 9],
    /// The abstract numbering: its id and the whole definition.
    abs: String,
    def: apex_doc::ListDef,
}

#[derive(Default)]
struct Ctx {
    styles: HashMap<String, PStyleDef>,
    numbering: HashMap<String, NumDef>,
    rels: HashMap<String, String>,
    /// Pictures by relationship id.
    media: HashMap<String, Arc<[u8]>>,
    /// Embedded objects and diagrams by relationship id.
    objects: crate::docx_ole::Objects,
    default_sz: Option<u16>,
    /// The language of docDefaults (runs in it keep no language).
    default_lang: Option<String>,
    theme: apex_doc::Theme,
    /// Paragraphs read from all the parts (body, notes, headers): at most
    /// `MAX_PARAGRAPHS`.
    paras: std::cell::Cell<usize>,
}

pub(crate) fn kind_from_name(name: &str) -> Option<ParaKind> {
    let n = name.to_ascii_lowercase();
    if let Some(rest) = n.strip_prefix("heading").or_else(|| n.strip_prefix("título")).or_else(|| n.strip_prefix("titulo")) {
        if let Ok(l) = rest.trim().parse::<u8>() {
            return Some(ParaKind::Heading(l.clamp(1, 9)));
        }
    }
    Some(match n.as_str() {
        "title" | "título" | "titulo" => ParaKind::Title,
        "quote" | "intense quote" | "block text" | "cita" => ParaKind::Quote,
        "html preformatted" | "source code" | "code" | "código" | "plain text" => ParaKind::Code,
        "list bullet" | "list bullet 2" | "list bullet 3" => ParaKind::Bullet,
        "list number" | "list number 2" | "list number 3" => ParaKind::Numbered,
        _ => return None,
    })
}

impl Ctx {
    fn style_chain(&self, id: &str) -> Vec<&PStyleDef> {
        let mut out = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            if out.len() > 12 {
                break;
            }
            let Some(s) = self.styles.get(i) else { break };
            out.push(s);
            cur = s.based_on.as_deref();
        }
        out
    }

    fn level(&self, num_id: &str, lvl: u8) -> Option<NumLevel> {
        let lv = &self.numbering.get(num_id)?.levels;
        lv.get(lvl as usize).or(lv.first()).copied()
    }
}

fn read_styles(xml: &str, ctx: &mut Ctx) {
    let mut r = Reader::new(xml);
    let mut cur: Option<(String, PStyleDef)> = None;
    let mut in_defaults = false;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "docDefaults" => in_defaults = true,
            Ev::End(n) if local(n) == "docDefaults" => in_defaults = false,
            Ev::Start(t) if t.local() == "style" => {
                let id = t.attr("w:styleId").unwrap_or_default().into_owned();
                cur = Some((id, PStyleDef::default()));
            }
            Ev::End(n) if local(n) == "style" => {
                if let Some((id, s)) = cur.take() {
                    ctx.styles.insert(id, s);
                }
            }
            Ev::Start(t) | Ev::Empty(t) => match t.local() {
                "name" => {
                    if let (Some((_, s)), Some(v)) = (&mut cur, t.attr("w:val")) {
                        s.name = v.chars().take(255).collect();
                    }
                }
                "basedOn" => {
                    if let (Some((_, s)), Some(v)) = (&mut cur, t.attr("w:val")) {
                        s.based_on = Some(v.into_owned());
                    }
                }
                "outlineLvl" => {
                    if let (Some((_, s)), Some(v)) = (&mut cur, t.attr("w:val").and_then(|v| v.parse::<u8>().ok())) {
                        if v < 9 {
                            s.outline = Some(v);
                        }
                    }
                }
                "numId" => {
                    if let (Some((_, s)), Some(v)) = (&mut cur, t.attr("w:val")) {
                        let lvl = s.num.as_ref().map_or(0, |n| n.1);
                        s.num = Some((v.into_owned(), lvl));
                    }
                }
                "ilvl" => {
                    if let (Some((_, s)), Some(v)) = (&mut cur, t.attr("w:val").and_then(|v| v.parse::<u8>().ok())) {
                        let id = s.num.as_ref().map(|n| n.0.clone()).unwrap_or_default();
                        s.num = Some((id, v));
                    }
                }
                "rFonts" => {
                    if let (Some((_, s)), Some(f)) = (&mut cur, t.attr("w:ascii")) {
                        s.mono = is_mono_font(&f);
                    }
                }
                "sz" if in_defaults => ctx.default_sz = t.attr("w:val").and_then(|v| v.parse().ok()),
                "lang" if in_defaults => ctx.default_lang = t.attr("w:val").map(|v| v.into_owned()),
                _ => {}
            },
            _ => {}
        }
    }
}

/// `list_format` for a numbering level definition.
fn level_format(fmt: &str, text: &str, lvl: u8) -> NumLevel {
    if fmt == "bullet" || fmt == "none" {
        let c = text.chars().next().unwrap_or('•');
        let f = if c == bullet_char(0, lvl) { 0 } else { (1..apex_doc::BULLETS.len() as u8).find(|&f| bullet_char(f, lvl) == c).unwrap_or(0) };
        return NumLevel { bullet: true, format: f };
    }
    let f = match fmt {
        "lowerLetter" => 3,
        "upperLetter" => 4,
        "lowerRoman" => 5,
        "upperRoman" => 6,
        _ if text.ends_with(')') => 2,
        _ => 1,
    };
    NumLevel { bullet: false, format: if f == num_format(0, lvl) { 0 } else { f } }
}

/// A level of a multilevel list from its `w:lvl`.
fn list_level(fmt: &str, text: &str, start: u32, legal: bool) -> apex_doc::ListLevel {
    use apex_doc::NumStyle as N;
    let style = match fmt {
        "bullet" => N::Bullet,
        "none" => N::None,
        "decimalZero" => N::DecimalZero,
        "lowerLetter" => N::LowerLetter,
        "upperLetter" => N::UpperLetter,
        "lowerRoman" => N::LowerRoman,
        "upperRoman" => N::UpperRoman,
        _ => N::Decimal,
    };
    let text = if style == N::Bullet && text.is_empty() { "•" } else { text };
    apex_doc::ListLevel { style, text: text.to_string(), start, legal }
}

fn read_numbering(xml: &str, ctx: &mut Ctx) {
    let mut r = Reader::new(xml);
    let mut abstracts: HashMap<String, Vec<NumLevel>> = HashMap::new();
    let mut cur_abs: Option<(String, Vec<NumLevel>)> = None;
    let mut defs: HashMap<String, apex_doc::ListDef> = HashMap::new();
    let mut cur_def = apex_doc::ListDef::outline();
    let mut lvl_start = 1u32;
    let mut lvl_legal = false;
    let mut cur_lvl: Option<usize> = None;
    let mut lvl_fmt = (String::new(), String::new());
    let mut nums: Vec<(String, String, [Option<u32>; 9])> = Vec::new();
    let mut cur_num: Option<(String, Option<String>, [Option<u32>; 9])> = None;
    let mut override_lvl = 0usize;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => match t.local() {
                "abstractNum" => {
                    cur_abs = Some((t.attr("w:abstractNumId").unwrap_or_default().into_owned(), vec![NumLevel::BULLET; 9]));
                    cur_def = apex_doc::ListDef::outline();
                }
                "lvl" if cur_abs.is_some() => {
                    cur_lvl = t.attr("w:ilvl").and_then(|v| v.parse().ok());
                    lvl_fmt = ("bullet".into(), String::new());
                    (lvl_start, lvl_legal) = (1, false);
                }
                "start" if cur_lvl.is_some() => lvl_start = t.attr("w:val").and_then(|v| v.parse().ok()).unwrap_or(1),
                "isLgl" if cur_lvl.is_some() => lvl_legal = t.attr("w:val").is_none_or(|v| v != "0" && v != "false"),
                "numFmt" if cur_lvl.is_some() => lvl_fmt.0 = t.attr("w:val").unwrap_or_default().into_owned(),
                // Word takes 255 characters at most.
                "lvlText" if cur_lvl.is_some() => lvl_fmt.1 = t.attr("w:val").unwrap_or_default().chars().take(255).collect(),
                "num" => cur_num = t.attr("w:numId").map(|v| (v.into_owned(), None, [None; 9])),
                "abstractNumId" => {
                    if let (Some(n), Some(a)) = (&mut cur_num, t.attr("w:val")) {
                        n.1 = Some(a.into_owned());
                    }
                }
                "lvlOverride" => override_lvl = t.attr("w:ilvl").and_then(|v| v.parse().ok()).unwrap_or(0),
                "startOverride" => {
                    if let (Some(n), Some(v)) = (&mut cur_num, t.attr("w:val").and_then(|v| v.parse().ok())) {
                        if override_lvl < 9 {
                            n.2[override_lvl] = Some(v);
                        }
                    }
                }
                _ => {}
            },
            Ev::End(n) => match local(n) {
                "abstractNum" => {
                    if let Some((id, lv)) = cur_abs.take() {
                        defs.insert(id.clone(), std::mem::replace(&mut cur_def, apex_doc::ListDef::outline()));
                        abstracts.insert(id, lv);
                    }
                }
                "lvl" => {
                    if let (Some((_, lv)), Some(l)) = (&mut cur_abs, cur_lvl.take()) {
                        if l < 9 {
                            lv[l] = level_format(&lvl_fmt.0, &lvl_fmt.1, l as u8);
                            cur_def.levels[l] = list_level(&lvl_fmt.0, &lvl_fmt.1, lvl_start, lvl_legal);
                        }
                    }
                }
                "num" => {
                    // Each copies its abstract numbering: no more than
                    // any document uses.
                    if let Some((n, Some(a), st)) = cur_num.take().filter(|_| nums.len() < 16_384) {
                        nums.push((n, a, st));
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    for (n, a, starts) in nums {
        if let Some(lv) = abstracts.get(&a) {
            let def = defs.get(&a).cloned().unwrap_or_default();
            ctx.numbering.insert(n, NumDef { levels: lv.clone(), starts, abs: a.clone(), def });
        }
    }
}

/// Longest relationship target kept: part names and links are far
/// shorter, and every reference to one may copy it.
pub(crate) const MAX_REL_TARGET: usize = 8192;

pub(crate) fn read_rels(xml: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "Relationship" => {
                if let (Some(id), Some(target)) = (t.attr("Id"), t.attr("Target").filter(|t| t.len() <= MAX_REL_TARGET)) {
                    m.insert(id.into_owned(), target.into_owned());
                }
            }
            _ => {}
        }
    }
    m
}

struct ParaState {
    para: Paragraph,
    style_id: Option<String>,
    num: Option<(Option<String>, u8)>,
    style: ParaStyle,
    explicit_outline: Option<u8>,
    /// Direct indentation in twips: left, right, first line (negative = hanging).
    ind: Option<(Option<i32>, Option<i32>, Option<i32>)>,
    extra: docx_layout::PprExtra,
    /// The paragraph mark's run properties: an empty paragraph's line is
    /// as tall as its mark.
    mark: Option<Attrs>,
}

impl ParaState {
    fn new() -> Self {
        ParaState { para: Paragraph::new(""), style_id: None, num: None, style: ParaStyle::default(), explicit_outline: None, ind: None, extra: Default::default(), mark: None }
    }
}

/// A link target and screen tip.
type Link = (Arc<str>, Option<Arc<str>>);

/// Hyperlinks, fields and controls open at once that are kept.
const MAX_OPEN: usize = 64;

struct DocReader<'a> {
    ctx: &'a Ctx,
    out: Vec<Paragraph>,
    cur: Option<ParaState>,
    /// Open hyperlinks and fields (`None` for fields that aren't links).
    links: Vec<Option<Link>>,
    /// Link targets read so far, shared: many hyperlinks may name one
    /// long target.
    targets: std::collections::HashSet<Arc<str>>,
    /// Open `w:sdt`s (`None` for block-level ones and kinds kept as plain
    /// content) and how many were read.
    controls: Vec<Option<Arc<apex_doc::ContentControl>>>,
    control_seq: u32,
    /// Open complex fields: instruction text so far, whether the field
    /// pushed a link, and 1 if its result is captured, 2 for a TOC.
    fields: Vec<(String, bool, u8)>,
    /// Bookmark names by id.
    bookmarks: HashMap<String, Arc<str>>,
    /// Editable ranges' mark names by `w:permStart` id.
    perms: HashMap<String, Arc<str>>,
    /// Bookmark starts seen between paragraphs, placed at the next one.
    pending_marks: Vec<Mark>,
    /// numId of the last list paragraph, while the list continues.
    last_num: Option<String>,
    /// Paragraph style ids, kept in `ParaStyle::custom` until the end.
    style_ids: crate::docx_styles::Ids,
    /// Footnotes and endnotes by id, for their references.
    notes: &'a notes::NoteMap,
    /// A field whose result text is being read: the field, the text and
    /// the formatting of its first run.
    capture: Option<(Field, String, Option<Attrs>)>,
    /// Open `w:fldSimple`s: whether each is captured.
    simple: Vec<bool>,
    /// Inside a TOC field (its settings); the last one's; and whether the
    /// current paragraph had one.
    toc: Option<TocOpts>,
    toc_last: TocOpts,
    toc_seen: bool,
    /// Open tracked changes, and the comments whose anchors have started.
    track: crate::docx_revisions::Tracks,
    comments_seen: std::collections::HashSet<u32>,
    /// Paragraph index each section ends before, and the sections.
    sect_ends: Vec<usize>,
    sects: Vec<docx_layout::SectInfo>,
    /// A drop cap's frame paragraph, joined to the next paragraph.
    pending_cap: Option<(Paragraph, apex_doc::DropCap)>,
    /// Reading a header or footer (its watermark shapes are skipped).
    header: bool,
    /// Multilevel lists by abstract numbering id, and the one numbering
    /// the headings.
    lists: Vec<(String, apex_doc::ListDef)>,
    /// Where each abstract numbering is in `lists`.
    list_index: HashMap<String, u16>,
    heading_list: u16,
    /// Numbering instances and levels seen, for their start overrides.
    seen_nums: std::collections::HashSet<(String, u8)>,
    /// A legacy form field being read.
    form: Option<FormRead>,
    /// The phonetic guide of the base text being read, and how many were.
    ruby: Option<Arc<apex_doc::Ruby>>,
    ruby_seq: u32,
    /// Hyperlinks and controls opened past `MAX_OPEN` (not kept).
    links_over: u32,
    controls_over: u32,
}

/// A legacy form field being read: its depth in `fields`, data and the
/// formatting of its start; once past the separator, where its result
/// starts in the paragraph.
struct FormRead {
    depth: usize,
    data: crate::docx_forms::FfData,
    attrs: Attrs,
    result: Option<usize>,
    /// Past the separator of a check box or drop-down: its result is
    /// dropped.
    swallow: bool,
}

impl<'a> DocReader<'a> {
    /// The 1-based id of the multilevel list of abstract numbering `abs`.
    fn list_id(&mut self, abs: &str, def: &apex_doc::ListDef) -> u16 {
        if let Some(n) = self.list_index.get(abs) {
            return *n;
        }
        // Past the numbers there are, new lists share the last one.
        if self.lists.len() >= u16::MAX as usize - 1 {
            return self.lists.len() as u16;
        }
        self.lists.push((abs.to_string(), def.clone()));
        let n = self.lists.len() as u16;
        self.list_index.insert(abs.to_string(), n);
        n
    }

    /// `s`, shared with an equal target read before.
    fn target(&mut self, s: &str) -> Arc<str> {
        if let Some(a) = self.targets.get(s) {
            return a.clone();
        }
        let a: Arc<str> = Arc::from(s);
        self.targets.insert(a.clone());
        a
    }

    fn new(ctx: &'a Ctx, notes: &'a notes::NoteMap) -> Self {
        DocReader {
            ctx,
            out: Vec::new(),
            cur: None,
            links: Vec::new(),
            targets: Default::default(),
            controls: Vec::new(),
            control_seq: 0,
            fields: Vec::new(),
            bookmarks: HashMap::new(),
            perms: HashMap::new(),
            pending_marks: Vec::new(),
            last_num: None,
            notes,
            capture: None,
            simple: Vec::new(),
            toc: None,
            toc_last: TocOpts::default(),
            toc_seen: false,
            style_ids: Default::default(),
            track: Default::default(),
            comments_seen: Default::default(),
            sect_ends: Vec::new(),
            sects: Vec::new(),
            pending_cap: None,
            header: false,
            lists: Vec::new(),
            list_index: HashMap::new(),
            heading_list: 0,
            seen_nums: Default::default(),
            form: None,
            ruby: None,
            ruby_seq: 0,
            links_over: 0,
            controls_over: 0,
        }
    }

    fn finish_para(&mut self) {
        let Some(st) = self.cur.take() else { return };
        let ctx = self.ctx;
        let mut style = st.style;
        let chain = st.style_id.as_deref().map(|id| ctx.style_chain(id)).unwrap_or_default();
        let mut kind = None;
        for s in &chain {
            if let Some(k) = kind_from_name(&s.name) {
                kind = Some(k);
                break;
            }
        }
        let outline = st.explicit_outline.or_else(|| chain.iter().find_map(|s| s.outline));
        if kind.is_none() {
            if let Some(o) = outline {
                kind = Some(ParaKind::Heading((o + 1).min(9)));
            }
        }
        if kind.is_none() && chain.iter().any(|s| s.mono) {
            kind = Some(ParaKind::Code);
        }
        // Numbering: direct numPr overrides the style's.
        let style_num = chain.iter().find_map(|s| s.num.clone());
        let num = match st.num {
            Some((Some(id), lvl)) => Some((id, lvl)),
            Some((None, lvl)) => style_num.map(|(id, _)| (id, lvl)),
            None => style_num,
        };
        let mut num_id = None;
        if let Some((id, lvl)) = num {
            if id != "0" && matches!(kind, Some(ParaKind::Heading(_))) {
                // Numbered headings.
                if self.heading_list == 0 {
                    // Only numbers that show (some templates link headings to empty labels).
                    if let Some(n) = ctx.numbering.get(&id).filter(|n| n.def.levels.iter().take(6).any(|l| l.text.contains('%') && l.style != apex_doc::NumStyle::None)) {
                        self.heading_list = self.list_id(&n.abs, &n.def);
                    }
                }
            } else if id != "0" && kind != Some(ParaKind::Title) {
                if let Some(l) = ctx.level(&id, lvl) {
                    kind = Some(if l.bullet { ParaKind::Bullet } else { ParaKind::Numbered });
                    style.level = lvl.min(8);
                    style.list_format = l.format;
                    if let Some(n) = ctx.numbering.get(&id).filter(|n| n.def.is_multilevel()) {
                        style.list = self.list_id(&n.abs, &n.def);
                        style.list_format = 0;
                    }
                    // The first item of a numbering with a start override.
                    if self.seen_nums.insert((id.clone(), lvl)) {
                        if let Some(n) = ctx.numbering.get(&id) {
                            let default = n.def.level(lvl as usize).map_or(1, |l| l.start);
                            if let Some(v) = n.starts[lvl.min(8) as usize].filter(|&v| v != default && v > 1) {
                                style.start_at = v.min(u16::MAX as u32) as u16;
                            }
                        }
                    }
                    num_id = Some(id);
                }
            }
        }
        let is_list = num_id.is_some();
        if let Some((left, right, first)) = st.ind {
            let pt = |v: i32| v as f32 / TWIP;
            if let Some(r) = right {
                style.indent_right = pt(r);
            }
            if is_list {
                // Relative to the numbering's own indentation.
                let lvl = style.level as i32;
                if let Some(l) = left {
                    style.indent_left = pt(l - 720 * (lvl + 1)).max(-36.0 * (lvl + 1) as f32);
                }
                if let Some(f) = first {
                    style.first_line = pt(f + 360);
                }
            } else {
                if let Some(l) = left.filter(|&l| l > 0) {
                    style.level = (l / 720).clamp(0, 8) as u8;
                    style.indent_left = pt(l - style.level as i32 * 720);
                }
                if let Some(f) = first {
                    style.first_line = pt(f);
                }
            }
        }
        // A new list instance right after another list restarts the numbering.
        if let Some(id) = &num_id {
            let lvl = style.level;
            let continues = crate::list_continues(&self.out, lvl);
            let start = ctx.numbering.get(id).and_then(|n| n.starts[lvl as usize]).unwrap_or(1);
            if kind == Some(ParaKind::Numbered) && continues && self.last_num.as_ref() != Some(id) && start <= 1 {
                style.restart = true;
            }
        }
        self.last_num = num_id;
        let extra = st.extra;
        let mark = st.mark;
        let mut p = st.para;
        // A code style whose text is all set in a proportional font isn't code.
        if kind == Some(ParaKind::Code) && p.runs.iter().any(|r| r.len > 0) && p.runs.iter().all(|r| r.len == 0 || (r.attrs.font.is_some() && !r.attrs.code)) {
            kind = None;
        }
        style.kind = kind.unwrap_or(ParaKind::Normal);
        // Code paragraphs don't need per-run code flags.
        if style.kind == ParaKind::Code {
            for r in &mut p.runs {
                r.attrs.code = false;
            }
        }
        self.toc_mark(&mut style, &p, chain.first().map(|s| s.name.as_str()));
        p.style = style;
        if let Some(id) = &st.style_id {
            p.style.custom = self.style_ids.number(id);
        }
        if p.runs.is_empty() {
            p.runs.push(apex_doc::Run { len: p.text.len(), attrs: Attrs::default() });
        }
        if let Some(a) = mark.filter(|_| p.text.is_empty()) {
            p.runs = vec![apex_doc::Run { len: 0, attrs: a }];
        }
        self.push_para(p, extra);
    }

    fn push_para(&mut self, p: Paragraph, extra: docx_layout::PprExtra) {
        if let Some(cap) = extra.cap.filter(|_| !p.text.is_empty() && extra.sect.is_none()) {
            self.flush_cap();
            self.pending_cap = Some((p, cap));
            return;
        }
        let p = match self.pending_cap.take() {
            Some((c, dc)) => docx_layout::merge_cap(c, dc, p),
            None => p,
        };
        if !self.room() {
            return;
        }
        self.out.push(p);
        if let Some(s) = extra.sect {
            self.sect_ends.push(self.out.len());
            self.sects.push(s);
        }
    }

    fn flush_cap(&mut self) {
        if let Some((c, _)) = self.pending_cap.take()
            && self.room()
        {
            self.out.push(c);
        }
    }

    /// Counts the paragraphs added past `before` (tables fill their
    /// empty cells).
    fn count(&self, before: usize) {
        self.ctx.paras.set(self.ctx.paras.get().saturating_add(self.out.len().saturating_sub(before)));
    }

    /// Whether the document takes another paragraph (counted).
    fn room(&self) -> bool {
        let n = self.ctx.paras.get();
        self.ctx.paras.set(n + 1);
        n < apex_doc::MAX_PARAGRAPHS
    }

    /// A column break: the text after it starts a paragraph in the next
    /// column.
    fn column_break(&mut self) {
        match &mut self.cur {
            Some(st) if st.para.text.is_empty() => st.style.column_break_before = true,
            _ => {
                self.break_para(false);
                if let Some(st) = &mut self.cur {
                    st.style.column_break_before = true;
                }
            }
        }
    }

    /// Opens a hyperlink or field (`None`: not a link); past `MAX_OPEN`
    /// open ones are only counted.
    fn push_link(&mut self, l: Option<Link>) {
        if self.links.len() < MAX_OPEN { self.links.push(l) } else { self.links_over += 1 }
    }

    fn pop_link(&mut self) {
        if self.links_over > 0 {
            self.links_over -= 1;
        } else {
            self.links.pop();
        }
    }

    fn push_control(&mut self) {
        if self.controls.len() < MAX_OPEN { self.controls.push(None) } else { self.controls_over += 1 }
    }

    fn pop_control(&mut self) {
        if self.controls_over > 0 {
            self.controls_over -= 1;
        } else {
            self.controls.pop();
        }
    }

    /// Splits the current paragraph at a line or page break.
    fn break_para(&mut self, page: bool) {
        let Some(st) = &self.cur else { return };
        let mut next = ParaState::new();
        next.style_id = st.style_id.clone();
        next.num = st.num.clone();
        next.style = st.style;
        next.style.page_break_before = page;
        next.style.column_break_before = false;
        next.style.drop_cap = None;
        next.explicit_outline = st.explicit_outline;
        next.ind = st.ind;
        self.finish_para();
        self.cur = Some(next);
    }

    fn push_text(&mut self, text: &str, attrs: &Attrs) {
        // A check box or drop-down shows its data, not its result.
        if self.form.as_ref().is_some_and(|f| f.swallow) {
            return;
        }
        // A field's result is its cached value.
        if let Some((_, t, a)) = &mut self.capture {
            t.push_str(text);
            if a.is_none() && !text.is_empty() {
                *a = Some(attrs.clone());
            }
            return;
        }
        if self.cur.is_none() {
            self.cur = Some(ParaState::new());
        }
        let mut a = attrs.clone();
        a.rev = self.track.current();
        if let Some((link, tip)) = self.links.iter().rev().find_map(|l| l.clone()) {
            a.link = Some(link);
            a.link_tip = tip;
        }
        if let Some(c) = self.controls.iter().rev().find_map(|c| c.clone()) {
            a.control = Some(c);
        }
        if let Some(r) = &self.ruby {
            a.set_asian(|x| x.ruby = Some(r.clone()));
        }
        self.cur.as_mut().unwrap().para.push(text, a);
    }

    fn start_para(&mut self) {
        self.finish_para();
        let mut st = ParaState::new();
        st.para.marks = std::mem::take(&mut self.pending_marks);
        self.cur = Some(st);
    }

    /// `w:commentRangeStart` / `End`, or a `w:commentReference` of a
    /// comment without a range (a point comment).
    fn comment_mark(&mut self, t: &Tag, end: Option<bool>) {
        let Some(id) = t.attr("w:id").and_then(|i| crate::docx_comments::our_id(&i)) else { return };
        let name = apex_doc::comment_mark(id);
        match end {
            Some(end) => {
                self.comments_seen.insert(id);
                self.place_mark(name, end);
            }
            None if self.comments_seen.insert(id) => {
                self.place_mark(name.clone(), false);
                self.place_mark(name, true);
            }
            None => {}
        }
    }

    fn bookmark(&mut self, t: &Tag, end: bool) {
        let Some(id) = t.attr("w:id") else { return };
        let name = if end {
            match self.bookmarks.get(id.as_ref()) {
                Some(n) => n.clone(),
                None => return,
            }
        } else {
            let Some(n) = t.attr("w:name").filter(|n| !n.is_empty()) else { return };
            let n: Arc<str> = Arc::from(n.as_ref());
            self.bookmarks.insert(id.into_owned(), n.clone());
            n
        };
        self.place_mark(name, end);
    }

    /// `w:permStart` / `w:permEnd`: an editable range of a protected
    /// document, with who may edit it kept for saving.
    fn perm(&mut self, t: &Tag, end: bool) {
        let Some(id) = t.attr("w:id") else { return };
        let name = if end {
            match self.perms.get(id.as_ref()) {
                Some(n) => n.clone(),
                None => return,
            }
        } else {
            let who = match (t.attr("w:edGrp"), t.attr("w:ed")) {
                (Some(g), _) => format!("edGrp={g}"),
                (None, Some(e)) => format!("ed={e}"),
                (None, None) => String::new(),
            };
            let n = apex_doc::perm_mark(self.perms.len() as u32 + 1, &who);
            self.perms.insert(id.into_owned(), n.clone());
            n
        };
        self.place_mark(name, end);
    }

    fn place_mark(&mut self, name: Arc<str>, end: bool) {
        let target = match &mut self.cur {
            Some(st) => Some(&mut st.para),
            // An end between paragraphs closes at the end of the previous one.
            None if end => self.out.last_mut(),
            None => None,
        };
        match target {
            Some(p) => p.marks.push(Mark { name, offset: p.text.len(), end }),
            None => self.pending_marks.push(Mark { name, offset: 0, end }),
        }
    }

    /// `w:fldChar`: complex fields; HYPERLINK fields become links, form
    /// fields (with their `w:ffData` in `ff`) controls.
    fn field_char(&mut self, t: &Tag, ff: Option<crate::docx_forms::FfData>, attrs: &Attrs) {
        match t.attr("w:fldCharType").as_deref() {
            Some("begin") => {
                self.fields.push((String::new(), false, 0));
                // Form fields in a paragraph, not in a control or a result.
                if let Some(data) = ff.filter(|_| self.form.is_none() && self.cur.is_some() && self.capture.is_none() && self.controls.iter().all(Option::is_none)) {
                    self.form = Some(FormRead { depth: self.fields.len(), data, attrs: attrs.clone(), result: None, swallow: false });
                }
            }
            Some("separate") if self.in_form() => {
                let len = self.cur.as_ref().map_or(0, |st| st.para.text.len());
                let word = self.fields.last().and_then(|f| crate::docx_forms::form_word(&f.0));
                if let Some(f) = &mut self.form {
                    f.result = Some(len);
                    f.swallow = word.is_some_and(|w| w != "FORMTEXT");
                }
            }
            Some("end") if self.in_form() => {
                let (instr, _, _) = self.fields.pop().unwrap_or_default();
                self.finish_form(&instr);
            }
            Some("separate") => {
                let Some((instr, pushed, mode)) = self.fields.last_mut() else { return };
                if *pushed || *mode != 0 {
                    return;
                }
                if let Some(l) = parse_hyperlink_field(instr) {
                    *pushed = true;
                    self.push_link(Some(l));
                } else if let Some(o) = notes::toc_opts(instr) {
                    *mode = 2;
                    self.toc = Some(o);
                    self.toc_last = o;
                    self.toc_seen = true;
                } else if self.capture.is_none()
                    && let Some(f) = Field::parse(instr, "")
                {
                    *mode = 1;
                    self.capture = Some((f, String::new(), None));
                } else {
                    // Read once: a field has one separator.
                    *mode = 3;
                }
            }
            Some("end") => match self.fields.pop().inspect(|_| {
                // Settings of a field that wasn't a form field.
                if self.form.as_ref().is_some_and(|f| f.depth > self.fields.len()) {
                    self.form = None;
                }
            }) {
                Some((_, true, _)) => self.pop_link(),
                Some((_, _, 1)) => self.finish_capture(),
                Some((_, _, 2)) => {
                    self.toc = None;
                    self.toc_seen = true;
                }
                // Fields without a result: index entries.
                Some((instr, false, 0)) => {
                    if self.capture.is_none()
                        && let Some(f) = Field::parse(&instr, "").filter(|f| matches!(f.kind, apex_doc::FieldKind::IndexEntry(_) | apex_doc::FieldKind::Authority { .. }))
                    {
                        self.push_field(f, None);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Whether the innermost field is the form field being read.
    fn in_form(&self) -> bool {
        self.form.as_ref().is_some_and(|f| f.depth == self.fields.len()) && self.fields.last().is_some_and(|f| crate::docx_forms::form_word(&f.0).is_some())
    }

    /// The end of a form field: its text becomes a control.
    fn finish_form(&mut self, instr: &str) {
        let Some(f) = self.form.take() else { return };
        let Some(word) = crate::docx_forms::form_word(instr) else { return };
        self.control_seq += 1;
        let (c, text) = crate::docx_forms::control(f.data, word, self.control_seq);
        match text {
            Some(text) => self.push_text(&text, &Attrs { control: Some(Arc::new(c)), ..f.attrs }),
            None => {
                let Some(st) = &mut self.cur else { return };
                let start = f.result.unwrap_or(st.para.text.len()).min(st.para.text.len());
                let end = st.para.text.len();
                let blank = crate::docx_forms::is_blank(st.para.text.get(start..end).unwrap_or(""));
                let c = Arc::new(ContentControl { placeholder: blank, ..c });
                if start == end {
                    self.push_text(apex_doc::FORM_BLANK, &Attrs { control: Some(c), ..f.attrs });
                } else {
                    st.para.map_attrs(start, end, &|a| a.control = Some(c.clone()));
                }
            }
        }
    }
}

/// Splits a field instruction into words, honouring double quotes.
fn field_words(instr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut has = false;
    for c in instr.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

/// `HYPERLINK "url" \l "bookmark" \o "tip"` to a link target and tip.
fn parse_hyperlink_field(instr: &str) -> Option<Link> {
    let w = field_words(instr);
    if !w.first().is_some_and(|k| k.eq_ignore_ascii_case("HYPERLINK")) {
        return None;
    }
    let (mut url, mut anchor, mut tip) = (None, None, None);
    let mut i = 1;
    while i < w.len() {
        match w[i].as_str() {
            "\\l" => {
                anchor = w.get(i + 1).cloned();
                i += 1;
            }
            "\\o" => {
                tip = w.get(i + 1).cloned();
                i += 1;
            }
            "\\t" => i += 1,
            s if s.starts_with('\\') => {}
            s => {
                if url.is_none() {
                    url = Some(s.to_string());
                }
            }
        }
        i += 1;
    }
    let target = match (url, anchor) {
        (Some(u), Some(a)) => format!("{u}#{a}"),
        (Some(u), None) => u,
        (None, Some(a)) => format!("#{a}"),
        (None, None) => return None,
    };
    Some((Arc::from(target.as_str()), tip.filter(|t| !t.is_empty()).map(|t| Arc::from(t.as_str()))))
}

fn read_ppr(r: &mut Reader, st: &mut ParaState, ctx: &Ctx) {
    let mut depth = 1;
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => return,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                let is_start = matches!(ev, Ev::Start(_));
                match t.local() {
                    "pStyle" => st.style_id = t.attr("w:val").map(|v| v.into_owned()),
                    "ilvl" => {
                        let lvl = t.attr("w:val").and_then(|v| v.parse().ok()).unwrap_or(0);
                        let id = st.num.as_ref().and_then(|n| n.0.clone());
                        st.num = Some((id, lvl));
                    }
                    "numId" => {
                        let lvl = st.num.as_ref().map_or(0, |n| n.1);
                        st.num = Some((t.attr("w:val").map(|v| v.into_owned()), lvl));
                    }
                    "jc" => {
                        st.style.align = match t.attr("w:val").as_deref() {
                            Some("center") => Align::Center,
                            Some("right") | Some("end") => Align::Right,
                            Some("both") | Some("distribute") => Align::Justify,
                            _ => Align::Left,
                        }
                    }
                    "spacing" => {
                        let tw = |k: &str| t.attr(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).map(|v| v.clamp(-1e6, 1e6));
                        if let Some(l) = tw("w:line") {
                            match t.attr("w:lineRule").as_deref() {
                                Some("exact") => (st.style.line_rule, st.style.line_spacing) = (LineRule::Exact, (l / TWIP * 10.0).round() / 10.0),
                                Some("atLeast") => (st.style.line_rule, st.style.line_spacing) = (LineRule::AtLeast, (l / TWIP * 10.0).round() / 10.0),
                                _ => {
                                    let ls = l / 240.0;
                                    st.style.line_rule = LineRule::Multiple;
                                    // Single stays 1 (not 0, "the defaults'").
                                    st.style.line_spacing = if (ls - 1.0).abs() > 0.01 { (ls * 100.0).round() / 100.0 } else { 1.0 };
                                }
                            }
                        }
                        if let Some(b) = tw("w:before") {
                            st.style.space_before = Some((b / TWIP * 10.0).round() / 10.0);
                        }
                        if let Some(a) = tw("w:after") {
                            st.style.space_after = Some((a / TWIP * 10.0).round() / 10.0);
                        }
                    }
                    "keepNext" => st.style.keep_with_next = on(&t),
                    "keepLines" => st.style.keep_lines = on(&t),
                    "contextualSpacing" => st.style.contextual_spacing = on(&t),
                    "widowControl" => st.style.no_widow_control = !on(&t),
                    "suppressLineNumbers" => st.style.no_line_numbers = on(&t),
                    "shd" => st.style.shading = t.attr("w:fill").filter(|v| v != "auto").and_then(|v| parse_hex_color(&v)).filter(|&c| c != 0xFFFFFF),
                    "pageBreakBefore" => st.style.page_break_before = on(&t),
                    "outlineLvl" => st.explicit_outline = t.attr("w:val").and_then(|v| v.parse().ok()).filter(|&v: &u8| v < 9),
                    "ind" => {
                        // Within a page's worth of twips (no overflow).
                        let tw = |k: &str| t.attr(k).and_then(|v| v.parse::<i32>().ok()).map(|v| v.clamp(-1_000_000, 1_000_000));
                        let first = tw("w:hanging").map(|h| -h).or_else(|| tw("w:firstLine"));
                        st.ind = Some((tw("w:left").or_else(|| tw("w:start")), tw("w:right").or_else(|| tw("w:end")), first));
                    }
                    "sectPr" if is_start => {
                        let mut s = docx_layout::SectInfo::default();
                        docx_layout::read_sect(r, &mut s);
                        st.extra.sect = Some(s);
                        continue;
                    }
                    "tabs" if is_start => {
                        docx_layout::read_tabs(r, &mut st.style.tabs);
                        continue;
                    }
                    "pBdr" if is_start => {
                        docx_layout::read_pbdr(r, &mut st.style.borders);
                        continue;
                    }
                    "framePr" => st.extra.cap = docx_layout::frame_cap(&t),
                    _ if crate::docx_asian::read_ppr(&t, &mut st.style.asian) => {}
                    // The paragraph mark's run properties (not its changes).
                    "rPr" if is_start => {
                        let mut a = Attrs::default();
                        read_rpr(r, &mut a, ctx);
                        a.rev = None;
                        st.mark = Some(a).filter(|a| *a != Attrs::default());
                        continue;
                    }
                    // Change tracking: skip.
                    "pPrChange" if is_start => {
                        r.skip();
                        continue;
                    }
                    _ => {}
                }
                if is_start {
                    depth += 1;
                }
            }
            _ => {}
        }
    }
}

fn read_rpr(r: &mut Reader, a: &mut Attrs, ctx: &Ctx) {
    let mut depth = 1;
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => return,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    // A complex script size equal to the size isn't kept.
                    if let Some(cs) = a.asian.as_ref().and_then(|x| x.cs_size) {
                        if Some(cs) == a.size.or(ctx.default_sz) {
                            a.set_asian(|x| x.cs_size = None);
                        }
                    }
                    return;
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                let is_start = matches!(ev, Ev::Start(_));
                // Word 2010's text effects.
                if t.name.starts_with("w14:") && matches!(t.local(), "glow" | "shadow" | "reflection" | "textOutline" | "textFill" | "scene3d" | "props3d") {
                    let n = crate::drawingml::read_node(r, &t, !is_start);
                    let mut fx = a.text_fx.as_deref().cloned().unwrap_or_default();
                    crate::text_fx_xml::read(&n, &mut fx);
                    a.text_fx = (!fx.is_empty()).then(|| Arc::new(fx));
                    continue;
                }
                let mut x = a.asian();
                let only = crate::docx_asian::read_rpr(&t, &mut x);
                if a.asian.as_deref() != Some(&x) {
                    a.asian = (!x.is_empty()).then(|| Arc::new(x));
                }
                if only {
                    if is_start {
                        depth += 1;
                    }
                    continue;
                }
                let mut fx = a.fx();
                if crate::docx_fx::read(&t, &mut fx) {
                    if t.local() == "u" {
                        a.underline = t.attr("w:val").is_none_or(|v| v != "none");
                    }
                    a.set_fx(|f| *f = fx);
                    if is_start {
                        depth += 1;
                    }
                    continue;
                }
                match t.local() {
                    "b" => a.bold = on(&t),
                    "i" => a.italic = on(&t),
                    "strike" => a.strike = on(&t),
                    "vertAlign" => match t.attr("w:val").as_deref() {
                        Some("superscript") => a.superscript = true,
                        Some("subscript") => a.subscript = true,
                        _ => {}
                    },
                    "sz" => {
                        let sz = t.attr("w:val").and_then(|v| v.parse::<u16>().ok());
                        a.size = sz.filter(|&s| Some(s) != ctx.default_sz && s > 0);
                    }
                    "color" => a.color = crate::theme_xml::read_word_color(&t, &ctx.theme),
                    "lang" => {
                        a.lang = t.attr("w:val").filter(|v| !v.is_empty() && ctx.default_lang.as_deref().is_none_or(|d| !d.eq_ignore_ascii_case(v))).map(|v| Arc::from(v.as_ref()));
                    }
                    "noProof" => a.no_proof = on(&t),
                    "highlight" => a.highlight = t.attr("w:val").and_then(|v| highlight_color(&v)),
                    "shd" => {
                        if a.highlight.is_none() {
                            a.highlight = t.attr("w:fill").filter(|v| v != "auto").and_then(|v| parse_hex_color(&v)).filter(|&c| c != 0xFFFFFF);
                        }
                    }
                    "rFonts" => {
                        if let Some(f) = crate::theme_xml::read_word_font(&t) {
                            if is_mono_font(&f) {
                                a.code = true;
                            } else {
                                a.font = Some(f);
                            }
                        }
                    }
                    "rStyle" => {
                        if let Some(id) = t.attr("w:val") {
                            let chain = ctx.style_chain(&id);
                            if chain.iter().any(|s| s.mono || s.name.to_ascii_lowercase().contains("code") || s.name.to_ascii_lowercase().contains("verbatim")) {
                                a.code = true;
                            } else {
                                a.char_style = Some(Arc::from(id.as_ref()));
                            }
                        }
                    }
                    "rPrChange" if is_start => {
                        r.skip();
                        continue;
                    }
                    _ => {}
                }
                if is_start {
                    depth += 1;
                }
            }
            _ => {}
        }
    }
}

/// A line end typed inside `<w:t>` is a space in Word (line breaks are
/// `<w:br/>`); XML turns CRLF into one line end first.
fn text_line_ends(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains(['\r', '\n']) {
        return s.into();
    }
    s.replace("\r\n", " ").replace(['\r', '\n'], " ").into()
}

fn read_run(r: &mut Reader, d: &mut DocReader) {
    let mut attrs = Attrs::default();
    let mut depth = 1;
    loop {
        match r.next() {
            Ev::Eof => return,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            Ev::Start(t) => match t.local() {
                "rPr" => read_rpr(r, &mut attrs, d.ctx),
                "t" | "delText" => {
                    let s = r.text();
                    d.push_text(&text_line_ends(&s), &attrs);
                }
                "instrText" | "delInstrText" => {
                    let s = r.text();
                    if let Some((instr, _, _)) = d.fields.last_mut() {
                        instr.push_str(&s);
                    }
                }
                "fldChar" => {
                    let ff = crate::docx_forms::read_fld_char(r);
                    d.field_char(&t, ff, &attrs);
                }
                "ruby" => read_ruby(r, d),
                "drawing" => {
                    // Shapes are read whole; pictures as they stream.
                    let raw = r.raw_element();
                    if raw.contains(crate::diagram::URI) {
                        if let Some((img, node)) = crate::docx_ole::read_diagram(raw, &d.ctx.objects, &d.ctx.media) {
                            match crate::docx_shape::read_floating_picture(&node, img.clone()) {
                                Some(sh) => d.push_text(&OBJECT.to_string(), &Attrs { shape: Some(Arc::new(sh)), ..attrs.clone() }),
                                None => d.push_text(&OBJECT.to_string(), &Attrs { image: Some(Arc::new(img)), ..attrs.clone() }),
                            }
                        }
                    } else if raw.contains(CHART_URI) {
                        if let Some((img, node)) = crate::docx_ole::read_chart(raw, &d.ctx.objects) {
                            match crate::docx_shape::read_floating_picture(&node, img.clone()) {
                                Some(sh) => d.push_text(&OBJECT.to_string(), &Attrs { shape: Some(Arc::new(sh)), ..attrs.clone() }),
                                None => d.push_text(&OBJECT.to_string(), &Attrs { image: Some(Arc::new(img)), ..attrs.clone() }),
                            }
                        }
                    } else if raw.contains("contentPart") {
                        if let Some(sh) = crate::docx_ink::read(&crate::drawingml::read_fragment(raw), &d.ctx.media) {
                            d.push_text(&OBJECT.to_string(), &Attrs { shape: Some(Arc::new(sh)), ..attrs.clone() });
                        }
                    } else if raw.contains("wgp") && raw.contains("anchor") {
                        for sh in crate::docx_shape::read_group(&crate::drawingml::read_fragment(raw), &d.ctx.media) {
                            d.push_text(&OBJECT.to_string(), &Attrs { shape: Some(Arc::new(sh)), ..attrs.clone() });
                        }
                    } else if raw.contains("wsp") {
                        if let Some(sh) = crate::docx_shape::read(&crate::drawingml::read_fragment(raw)) {
                            d.push_text(&OBJECT.to_string(), &Attrs { shape: Some(Arc::new(sh)), ..attrs.clone() });
                        }
                    } else if let Some(img) = docx_img::read_drawing(&mut Reader::new(raw), &d.ctx.media) {
                        let floating = if raw.contains("anchor") { crate::docx_shape::read_floating_picture(&crate::drawingml::read_fragment(raw), img.clone()) } else { None };
                        match floating {
                            Some(sh) => d.push_text(&OBJECT.to_string(), &Attrs { shape: Some(Arc::new(sh)), ..attrs.clone() }),
                            None => d.push_text(&OBJECT.to_string(), &Attrs { image: Some(Arc::new(img)), ..attrs.clone() }),
                        }
                    }
                }
                "AlternateContent" => {
                    // Word's shapes: the choice for Word 2010 shapes.
                    let root = crate::drawingml::read_node(r, &t, false);
                    // Ink (Word 2010 and later).
                    if crate::docx_ink::is_ink(&root) {
                        if let Some(sh) = crate::docx_ink::read(&root, &d.ctx.media) {
                            d.push_text(&OBJECT.to_string(), &Attrs { shape: Some(Arc::new(sh)), ..attrs.clone() });
                            continue;
                        }
                    }
                    let group = root.children("Choice").filter(|c| c.attr("Requires").is_some_and(|q| q.split_whitespace().any(|x| x == "wpg"))).map(|c| crate::docx_shape::read_group(c, &d.ctx.media)).find(|g| !g.is_empty());
                    if let Some(g) = group {
                        for sh in g {
                            d.push_text(&OBJECT.to_string(), &Attrs { shape: Some(Arc::new(sh)), ..attrs.clone() });
                        }
                        continue;
                    }
                    let shape = root.children("Choice").filter(|c| c.attr("Requires").is_some_and(|q| q.split_whitespace().any(|x| x == "wps"))).find_map(crate::docx_shape::read);
                    if let Some(sh) = shape {
                        d.push_text(&OBJECT.to_string(), &Attrs { shape: Some(Arc::new(sh)), ..attrs.clone() });
                    }
                }
                "pict" if d.header => {
                    // Watermarks are the page's, not the header's.
                    let raw = r.raw_element();
                    let img = if raw.to_ascii_lowercase().contains("watermark") { None } else { docx_img::read_pict(&mut Reader::new(raw), &d.ctx.media) };
                    if let Some(img) = img {
                        d.push_text(&OBJECT.to_string(), &Attrs { image: Some(Arc::new(img)), ..attrs.clone() });
                    }
                }
                "pict" | "object" => {
                    let orig = (t.attr("w:dxaOrig").and_then(|v| v.parse().ok()), t.attr("w:dyaOrig").and_then(|v| v.parse().ok()));
                    let raw = r.raw_element();
                    match crate::docx_ole::read(raw, orig, &d.ctx.objects, &d.ctx.media) {
                        Some(crate::docx_ole::Placed::Inline(img)) => d.push_text(&OBJECT.to_string(), &Attrs { image: Some(Arc::new(img)), ..attrs.clone() }),
                        Some(crate::docx_ole::Placed::Float(sh)) => d.push_text(&OBJECT.to_string(), &Attrs { shape: Some(Arc::new(sh)), ..attrs.clone() }),
                        None => {
                            if let Some(img) = docx_img::read_pict(&mut Reader::new(raw), &d.ctx.media) {
                                d.push_text(&OBJECT.to_string(), &Attrs { image: Some(Arc::new(img)), ..attrs.clone() });
                            }
                        }
                    }
                }
                _ => r.skip(),
            },
            Ev::Empty(t) => match t.local() {
                "fldChar" => d.field_char(&t, None, &attrs),
                "footnoteReference" => d.note_ref(&t, NoteKind::Foot, &attrs),
                "endnoteReference" => d.note_ref(&t, NoteKind::End, &attrs),
                "commentReference" => d.comment_mark(&t, None),
                "tab" | "ptab" => d.push_text("\t", &attrs),
                "br" | "cr" => match t.attr("w:type").as_deref() {
                    Some("page") => d.break_para(true),
                    Some("column") => d.column_break(),
                    _ => d.push_text(&LINE_BREAK.to_string(), &attrs),
                },
                "noBreakHyphen" => d.push_text("\u{2011}", &attrs),
                "softHyphen" => d.push_text("\u{AD}", &attrs),
                _ => {}
            },
            _ => {}
        }
    }
}

/// `w:ruby` (the reader is past its start tag): the base runs are text
/// carrying the guide.
fn read_ruby(r: &mut Reader, d: &mut DocReader) {
    // No guide inside a guide's base text (nor the recursion it takes).
    if d.ruby.is_some() {
        r.skip();
        return;
    }
    d.ruby_seq += 1;
    let mut ruby = apex_doc::Ruby { seq: d.ruby_seq, ..Default::default() };
    loop {
        match r.next() {
            Ev::Eof | Ev::End(_) => break,
            Ev::Start(t) => match t.local() {
                "rubyPr" => crate::docx_asian::read_ruby_pr(r, &mut ruby),
                "rt" => crate::docx_asian::read_rt(r, &mut ruby),
                "rubyBase" => {
                    let saved = d.ruby.replace(Arc::new(ruby.clone()));
                    loop {
                        match r.next() {
                            Ev::Eof | Ev::End(_) => break,
                            Ev::Start(t) if t.local() == "r" => read_run(r, d),
                            Ev::Start(_) => r.skip(),
                            _ => {}
                        }
                    }
                    d.ruby = saved;
                }
                _ => r.skip(),
            },
            _ => {}
        }
    }
}

/// Reads the paragraphs and tables of a body (or a header or footer part)
/// into `d`; returns the body's section and page background.
fn read_body(xml: &str, d: &mut DocReader, tstyles: &HashMap<String, crate::docx_table::TblStyleDef>) -> (docx_layout::SectInfo, (Option<u32>, Option<apex_doc::Gradient>)) {
    let ctx = d.ctx;
    let mut tables = crate::docx_table::TblReader::default();
    let mut body_sect = docx_layout::SectInfo::default();
    let mut background = None;
    let mut page_grad = None;
    let mut r = Reader::new(xml);
    loop {
        // Past what a document keeps (`capped` says some were left out):
        // a small file of empty paragraphs would otherwise take gigabytes.
        if d.out.len() > apex_doc::MAX_PARAGRAPHS || d.ctx.paras.get() > apex_doc::MAX_PARAGRAPHS {
            break;
        }
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => match t.local() {
                "p" => d.start_para(),
                "pPr" => {
                    if let Some(st) = &mut d.cur {
                        read_ppr(&mut r, st, ctx);
                    } else {
                        r.skip();
                    }
                }
                "r" => read_run(&mut r, d),
                "oMathPara" | "oMath" => {
                    for (eq, size) in crate::omml::read(&mut r, &t) {
                        d.push_text(&OBJECT.to_string(), &Attrs { math: Some(eq), size, ..Default::default() });
                    }
                }
                "hyperlink" => {
                    let anchor = t.attr("w:anchor").filter(|a| !a.is_empty());
                    let link = match (t.attr("r:id").and_then(|id| ctx.rels.get(id.as_ref()).cloned()), anchor) {
                        (Some(u), Some(a)) => Some(format!("{u}#{a}")),
                        (Some(u), None) => Some(u),
                        (None, Some(a)) => Some(format!("#{a}")),
                        (None, None) => None,
                    };
                    let tip = t.attr("w:tooltip").filter(|v| !v.is_empty()).map(|v| Arc::from(v.as_ref()));
                    let link = link.map(|l| d.target(&l));
                    d.push_link(link.map(|l| (l, tip)));
                }
                "fldSimple" => d.simple_field(&t, true),
                "bookmarkStart" => d.bookmark(&t, false),
                "bookmarkEnd" => d.bookmark(&t, true),
                "permStart" => d.perm(&t, false),
                "permEnd" => d.perm(&t, true),
                "ins" | "moveTo" => d.track.push(apex_doc::RevKind::Insert, &t),
                "del" | "moveFrom" => d.track.push(apex_doc::RevKind::Delete, &t),
                "commentRangeStart" => d.comment_mark(&t, Some(false)),
                "commentRangeEnd" => d.comment_mark(&t, Some(true)),
                "tbl" => {
                    d.finish_para();
                    d.flush_cap();
                    tables.start_table(d.out.len());
                }
                "tblPr" => tables.read_tbl_pr(&mut r),
                "tblGrid" => tables.read_grid(&mut r),
                "tr" => tables.start_row(),
                "trPr" => tables.read_tr_pr(&mut r, &mut d.out),
                "tc" => {
                    d.finish_para();
                    tables.start_cell(d.out.len());
                }
                "tcPr" => tables.read_tc_pr(&mut r),
                "sectPr" => docx_layout::read_sect(&mut r, &mut body_sect),
                "background" => (background, page_grad) = docx_layout::read_background(&mut r, &t),
                "sdt" => d.push_control(),
                "sdtPr" => {
                    // Inline controls only: in a paragraph, and not inside another.
                    let inline = d.cur.is_some() && d.controls.iter().all(Option::is_none) && d.fields.is_empty();
                    d.control_seq += 1;
                    let c = crate::docx_controls::read_pr(&mut r, d.control_seq);
                    if inline && d.controls_over == 0 && let (Some(c), Some(top)) = (c, d.controls.last_mut()) {
                        *top = Some(Arc::new(c));
                    }
                }
                "drawing" | "pict" | "AlternateContent" | "commentReference" | "footnoteReference" | "endnoteReference" => r.skip(),
                _ => {}
            },
            Ev::Empty(t) => match t.local() {
                // `<w:p/>`: an empty paragraph.
                "p" => {
                    d.start_para();
                    d.finish_para();
                }
                "bookmarkStart" => d.bookmark(&t, false),
                "bookmarkEnd" => d.bookmark(&t, true),
                "permStart" => d.perm(&t, false),
                "permEnd" => d.perm(&t, true),
                "fldSimple" => d.simple_field(&t, false),
                "commentRangeStart" => d.comment_mark(&t, Some(false)),
                "commentRangeEnd" => d.comment_mark(&t, Some(true)),
                "background" => background = docx_layout::background(&t),
                _ => {}
            },
            Ev::End(n) => match local(n) {
                "p" => d.finish_para(),
                "tc" => {
                    d.finish_para();
                    let before = d.out.len();
                    tables.end_cell(&mut d.out);
                    d.count(before);
                }
                "tr" => tables.end_row(),
                "tbl" => {
                    d.finish_para();
                    let before = d.out.len();
                    tables.end_table(&mut d.out, tstyles);
                    d.count(before);
                }
                "hyperlink" => d.pop_link(),
                "sdt" => d.pop_control(),
                "fldSimple" => d.end_simple_field(),
                "ins" | "del" | "moveTo" | "moveFrom" => d.track.pop(),
                _ => {}
            },
            _ => {}
        }
    }
    d.finish_para();
    d.flush_cap();
    (body_sect, (background, page_grad))
}

/// Reads a header or footer part (at `path`) with its own relationships
/// and pictures; paragraph styles share `ids` with the body.
fn read_hf_part(z: &ZipReader, ctx: &mut Ctx, path: &str, xml: &str, notes: &notes::NoteMap, tstyles: &HashMap<String, crate::docx_table::TblStyleDef>, ids: &mut crate::docx_styles::Ids) -> Vec<Paragraph> {
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    let rels = z.read_string(&format!("{dir}/_rels/{file}.rels")).map(|s| read_rels(&s)).unwrap_or_default();
    let media = docx_img::load_media(z, &rels, dir);
    let objects = crate::docx_ole::Objects::load(z, path);
    let saved = (std::mem::replace(&mut ctx.rels, rels), std::mem::replace(&mut ctx.media, media), std::mem::replace(&mut ctx.objects, objects));
    let mut d = DocReader::new(ctx, notes);
    d.header = true;
    d.style_ids = std::mem::take(ids);
    read_body(xml, &mut d, tstyles);
    *ids = std::mem::take(&mut d.style_ids);
    let out = std::mem::take(&mut d.out);
    drop(d);
    (ctx.rels, ctx.media, ctx.objects) = saved;
    out
}

pub fn read(data: &[u8]) -> Result<Document, Error> {
    let z = ZipReader::new(data)?;
    let mut ctx = Ctx::default();
    if let Ok(s) = z.read_string("word/_rels/document.xml.rels") {
        ctx.rels = read_rels(&s);
        ctx.media = docx_img::load_media(&z, &ctx.rels, "word");
        ctx.objects = crate::docx_ole::Objects::load(&z, "word/document.xml");
    }
    if let Some(target) = ctx.rels.values().find(|t| t.contains("theme/")) {
        let path = format!("word/{}", target.trim_start_matches("/word/").trim_start_matches('/'));
        if let Ok(s) = z.read_string(&path) {
            ctx.theme = crate::theme_xml::read_theme(&s);
        }
    }
    if let Ok(s) = z.read_string("word/styles.xml") {
        read_styles(&s, &mut ctx);
    }
    if let Ok(s) = z.read_string("word/numbering.xml") {
        read_numbering(&s, &mut ctx);
    }
    let tstyles = z.read_string("word/styles.xml").map(|s| crate::docx_table::read_table_styles(&s)).unwrap_or_default();
    let xml = z.read_string("word/document.xml")?;
    let (note_map, note_sep) = notes::read_notes(&z, &ctx);
    let mut d = DocReader::new(&ctx, &note_map);
    let (body_sect, (background, page_grad)) = read_body(&xml, &mut d, &tstyles);
    let mut default_font = None;
    if let Ok(s) = z.read_string("word/styles.xml") {
        // Document default font from docDefaults.
        let mut r = Reader::new(&s);
        let mut in_def = false;
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) if t.local() == "docDefaults" => in_def = true,
                Ev::End(n) if local(n) == "docDefaults" => break,
                Ev::Start(t) | Ev::Empty(t) if in_def && t.local() == "rFonts" => {
                    default_font = crate::theme_xml::read_word_font(&t);
                }
                _ => {}
            }
        }
    }
    let mut out = std::mem::take(&mut d.out);
    let mut style_ids = std::mem::take(&mut d.style_ids);
    let sect_ends = std::mem::take(&mut d.sect_ends);
    let lists: Vec<apex_doc::ListDef> = std::mem::take(&mut d.lists).into_iter().map(|(_, def)| def).collect();
    let heading_list = d.heading_list;
    let mut sects = std::mem::take(&mut d.sects);
    drop(d);
    sects.push(body_sect);
    // Headers and footers: each part is read once, like the body.
    let mut stories: Vec<Vec<Paragraph>> = Vec::new();
    // By part, not by relationship: many ids naming one part read it once.
    let mut by_path: HashMap<String, usize> = HashMap::new();
    let mut slots: Vec<(usize, apex_doc::HfSlot, usize)> = Vec::new();
    for (k, sect) in sects.iter().enumerate() {
        for (header, ty, id) in &sect.hf_refs {
            let Some(target) = ctx.rels.get(id).cloned() else { continue };
            let path = crate::xlsx::resolve_target("word", &target);
            let n = match by_path.get(&path) {
                Some(&n) => n,
                None => {
                    let Ok(part) = z.read_string(&path) else { continue };
                    stories.push(read_hf_part(&z, &mut ctx, &path, &part, &note_map, &tstyles, &mut style_ids));
                    by_path.insert(path, stories.len() - 1);
                    stories.len() - 1
                }
            };
            slots.push((k, apex_doc::HfSlot::of(*header, *ty), n));
        }
    }
    let body_len = out.len();
    let lens: Vec<usize> = stories.iter().map(Vec::len).collect();
    for st in &mut stories {
        out.append(st);
    }
    let styles_xml = z.read_string("word/styles.xml").ok();
    let styles = crate::docx_styles::import(&mut out, styles_xml.as_deref(), &style_ids, &ctx.theme);
    let mut rest = out.split_off(body_len);
    let mut parts: Vec<apex_doc::Story> = Vec::new();
    for n in lens {
        let tail = rest.split_off(n);
        parts.push(Arc::new(Document::from_paragraphs(std::mem::replace(&mut rest, tail)).paragraphs));
    }
    for (k, slot, n) in slots {
        sects[k].headers.set(slot, Some(parts[n].clone()));
    }
    for sect in &mut sects {
        sect.headers.different_first = sect.title_pg;
    }
    let mut doc = Document::from_paragraphs(out);
    doc.styles = styles;
    if let Some(s) = &styles_xml {
        doc.styles.asian = crate::docx_asian::read_defaults(s);
        // The document defaults (10.5 pt in Japanese documents), which
        // Normal and the other styles start from.
        doc.styles.defaults = crate::docx_defaults::read(s);
    }
    doc.styles.lists = lists;
    doc.styles.heading_list = heading_list;
    doc.styles.tables = crate::docx_table::import_styles(&tstyles, &doc);
    doc.biblio = crate::docx_biblio::read(&z);
    doc.book = crate::book_xml::read_docx(&z);
    doc.kept = crate::ooxml_keep::read_docx(&z);
    if let Some(x) = z.read_string("word/settings.xml").ok().and_then(|s| crate::docx_asian::read_settings(&s)) {
        doc.kept.xml.push((crate::docx_asian::SETTINGS_SLOT.into(), x));
    }
    // Saved as odt, LibreOffice lays the document grid out as Word does.
    doc.kept.xml.push((crate::odt_settings::SLOT.into(), crate::odt_settings::WORD_GRID.into()));
    doc.kept.xml.push((crate::odt_defaults::PAGE_SLOT.into(), crate::odt_defaults::WORD_PAGE.into()));
    doc.notes = notes::read_config(&z, &xml);
    doc.notes.separator = note_sep;
    docx_layout::apply_sections(&mut doc, &sect_ends, &sects);
    doc.deco.color = background;
    doc.deco.gradient = page_grad;
    doc.odd_even = z.read_string("word/settings.xml").is_ok_and(|s| crate::docx_hf::even_and_odd(&s));
    doc.auto_hyphenation = z.read_string("word/settings.xml").is_ok_and(|s| on_flag(&s, "autoHyphenation"));
    if let Ok(s) = z.read_string("word/settings.xml") {
        let (mirror, top) = (on_flag(&s, "mirrorMargins"), on_flag(&s, "gutterAtTop"));
        doc.set_all_pages(|p| (p.mirror, p.gutter_top) = (mirror, top));
    }
    doc.track_changes = z.read_string("word/settings.xml").is_ok_and(|s| s.contains("<w:trackRevisions/>") || s.contains("<w:trackRevisions "));
    doc.protection = z.read_string("word/settings.xml").ok().and_then(|s| read_protection(&s));
    doc.set_comments(crate::docx_comments::read(&z, &ctx.rels));
    let refs = crate::docx_hf::Refs::scan(&xml);
    doc.deco.watermark = docx_layout::read_watermark(&z, &ctx.rels, &refs);
    doc.default_font = default_font.filter(|f| !is_mono_font(f));
    crate::docx_defaults::finish(&mut doc);
    doc.meta = crate::props::read_ooxml(&z);
    doc.meta.read_only_recommended = z.read_string("word/settings.xml").is_ok_and(|s| read_only_recommended(&s));
    doc.macros = crate::macro_parts::read(&z, "word", &z.read_string("word/_rels/document.xml.rels").unwrap_or_default());
    doc.fonts = crate::docx_fonts::read(&z);
    doc.tidy_text_links();
    doc.tidy_shape_groups();
    crate::docx_bind::load(&mut doc);
    Ok(doc)
}

// ---------------------------------------------------------------------------
// Writing

fn style_id(kind: ParaKind) -> Option<String> {
    Some(match kind {
        ParaKind::Normal => return None,
        ParaKind::Heading(n) => format!("Heading{}", n.clamp(1, 9)),
        ParaKind::Title => "Title".into(),
        ParaKind::Quote => "Quote".into(),
        ParaKind::Code => "Code".into(),
        ParaKind::Bullet | ParaKind::Numbered => "ListParagraph".into(),
    })
}

fn styles_xml(doc: &Document) -> String {
    let mut s = String::from(HEADER);
    let _ = write!(s, r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" mc:Ignorable="w14 w15">{}"#, crate::docx_defaults::write(doc));
    if !crate::docx_styles::overridden(doc, ParaKind::Normal) {
        s.push_str(r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>"#);
    }
    s.push_str(r#"<w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/><w:semiHidden/></w:style>"#);
    if !crate::docx_styles::overridden(doc, ParaKind::Title) {
        s.push_str(r#"<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:after="240"/></w:pPr><w:rPr><w:b/><w:sz w:val="52"/><w:szCs w:val="52"/></w:rPr></w:style>"#);
    }
    for n in (1..=9u8).filter(|&n| !crate::docx_styles::overridden(doc, ParaKind::Heading(n))) {
        let sz = ParaKind::Heading(n).base_size();
        let _ = write!(
            s,
            r#"<w:style w:type="paragraph" w:styleId="Heading{n}"><w:name w:val="heading {n}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="80"/><w:outlineLvl w:val="{l}"/></w:pPr><w:rPr><w:b/><w:sz w:val="{sz}"/><w:szCs w:val="{sz}"/></w:rPr></w:style>"#,
            n = n,
            l = n - 1,
            sz = sz
        );
    }
    if !crate::docx_styles::overridden(doc, ParaKind::Quote) {
        s.push_str(r#"<w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:ind w:left="720" w:right="720"/></w:pPr><w:rPr><w:i/><w:color w:val="555555"/></w:rPr></w:style>"#);
    }
    s.push_str(&crate::docx_styles::styles_xml(doc));
    // The document's own styles may already use these ids (a Word file's
    // "ListParagraph", say); style ids must be unique.
    let add = |s: &mut String, id: &str, xml: &str| {
        if !s.contains(&format!("w:styleId=\"{id}\"")) {
            s.push_str(xml);
        }
    };
    add(&mut s, "Code", &format!(r#"<w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Source Code"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:shd w:val="clear" w:color="auto" w:fill="F3F3F3"/><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:rFonts w:ascii="{m}" w:hAnsi="{m}" w:cs="{m}"/><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>"#, m = MONO_FONT));
    add(&mut s, "ListParagraph", r#"<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:after="40"/></w:pPr></w:style>"#);
    add(&mut s, "Hyperlink", r#"<w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:basedOn w:val="DefaultParagraphFont"/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style>"#);
    s.push_str(&crate::docx_table::styles_xml(doc));
    s.push_str(&notes::styles());
    add(&mut s, "VerbatimChar", &format!(r#"<w:style w:type="character" w:styleId="VerbatimChar"><w:name w:val="Verbatim Char"/><w:basedOn w:val="DefaultParagraphFont"/><w:rPr><w:rFonts w:ascii="{m}" w:hAnsi="{m}" w:cs="{m}"/></w:rPr></w:style>"#, m = MONO_FONT));
    s.push_str("</w:styles>");
    s
}

/// Numbering definitions used while writing: abstract formats (bullet,
/// list_format) and concrete nums (abstract index, level start override).
#[derive(Default)]
struct Numbering {
    abstracts: Vec<(bool, u8)>,
    /// Multilevel lists, written after the simple ones.
    defs: Vec<apex_doc::ListDef>,
    nums: Vec<(usize, Option<(u8, usize)>)>,
}

/// Abstract numbering ids of multilevel lists start here.
const DEF_ABS: usize = 1000;

impl Numbering {
    fn abstract_id(&mut self, bullet: bool, format: u8) -> usize {
        match self.abstracts.iter().position(|&a| a == (bullet, format)) {
            Some(i) => i,
            None => {
                self.abstracts.push((bullet, format));
                self.abstracts.len() - 1
            }
        }
    }

    fn def_abstract(&mut self, def: &apex_doc::ListDef) -> usize {
        match self.defs.iter().position(|d| d == def) {
            Some(i) => DEF_ABS + i,
            None => {
                self.defs.push(def.clone());
                DEF_ABS + self.defs.len() - 1
            }
        }
    }

    /// Adds a num and returns its numId.
    fn add(&mut self, abs: usize, start: Option<(u8, usize)>) -> usize {
        self.nums.push((abs, start));
        self.nums.len()
    }

    fn xml(&self) -> String {
        let mut s = String::from(HEADER);
        s.push_str(r#"<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#);
        for (i, &(bullet, format)) in self.abstracts.iter().enumerate() {
            let _ = write!(s, r#"<w:abstractNum w:abstractNumId="{}"><w:multiLevelType w:val="hybridMultilevel"/>"#, i);
            for l in 0..9u8 {
                let (fmt, text) = if bullet {
                    ("bullet", bullet_char(format, l).to_string())
                } else {
                    let f = num_format(format, l);
                    let fmt = ["decimal", "decimal", "decimal", "lowerLetter", "upperLetter", "lowerRoman", "upperRoman"][f as usize];
                    (fmt, format!("%{}{}", l + 1, if matches!(f, 2 | 3) { ")" } else { "." }))
                };
                let _ = write!(s, r#"<w:lvl w:ilvl="{}"><w:start w:val="1"/><w:numFmt w:val="{}"/><w:lvlText w:val="{}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{}" w:hanging="360"/></w:pPr></w:lvl>"#, l, fmt, esc(&text), 720 * (l as u32 + 1));
            }
            s.push_str("</w:abstractNum>");
        }
        for (i, def) in self.defs.iter().enumerate() {
            use apex_doc::NumStyle as N;
            let _ = write!(s, r#"<w:abstractNum w:abstractNumId="{}"><w:multiLevelType w:val="multilevel"/>"#, DEF_ABS + i);
            for (l, lv) in def.levels.iter().enumerate().take(9) {
                let fmt = match lv.style {
                    N::Decimal => "decimal",
                    N::DecimalZero => "decimalZero",
                    N::LowerLetter => "lowerLetter",
                    N::UpperLetter => "upperLetter",
                    N::LowerRoman => "lowerRoman",
                    N::UpperRoman => "upperRoman",
                    N::Bullet => "bullet",
                    N::None => "none",
                };
                let legal = if lv.legal { "<w:isLgl/>" } else { "" };
                let _ = write!(s, r#"<w:lvl w:ilvl="{}"><w:start w:val="{}"/><w:numFmt w:val="{}"/>{}<w:lvlText w:val="{}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{}" w:hanging="360"/></w:pPr></w:lvl>"#, l, lv.start, fmt, legal, esc(&lv.text), 720 * (l as u32 + 1));
            }
            s.push_str("</w:abstractNum>");
        }
        for (i, &(abs, start)) in self.nums.iter().enumerate() {
            let _ = write!(s, r#"<w:num w:numId="{}"><w:abstractNumId w:val="{}"/>"#, i + 1, abs);
            if let Some((l, n)) = start {
                let _ = write!(s, r#"<w:lvlOverride w:ilvl="{}"><w:startOverride w:val="{}"/></w:lvlOverride>"#, l, n);
            }
            s.push_str("</w:num>");
        }
        s.push_str("</w:numbering>");
        s
    }
}

pub(crate) fn write_text(out: &mut String, text: &str) {
    let mut rest = text;
    while !rest.is_empty() {
        let (part, sep) = match rest.find(['\t', LINE_BREAK, '\u{AD}', '\u{2011}']) {
            Some(i) => {
                let c = rest[i..].chars().next().unwrap();
                (&rest[..i], Some((c, i + c.len_utf8())))
            }
            None => (rest, None),
        };
        if !part.is_empty() {
            let _ = write!(out, r#"<w:t xml:space="preserve">{}</w:t>"#, esc(part));
        }
        match sep {
            Some((c, next)) => {
                out.push_str(match c {
                    '\t' => "<w:tab/>",
                    '\u{AD}' => "<w:softHyphen/>",
                    '\u{2011}' => "<w:noBreakHyphen/>",
                    _ => "<w:br/>",
                });
                rest = &rest[next..];
            }
            None => break,
        }
    }
}

fn write_rpr(out: &mut String, a: &Attrs, link: bool) {
    let mut p = String::new();
    if link {
        p.push_str(r#"<w:rStyle w:val="Hyperlink"/>"#);
    } else if a.code {
        p.push_str(r#"<w:rStyle w:val="VerbatimChar"/>"#);
    } else if let Some(id) = crate::docx_styles::run_style(a) {
        let _ = write!(p, r#"<w:rStyle w:val="{}"/>"#, esc(id));
    }
    let asian = a.asian.as_deref();
    crate::docx_asian::write_rfonts(&mut p, a.font.as_deref().filter(|_| !a.code), asian);
    if a.bold {
        p.push_str("<w:b/><w:bCs/>");
    }
    if a.italic {
        p.push_str("<w:i/><w:iCs/>");
    }
    let fx = a.fx();
    crate::docx_fx::write_early(&mut p, &fx, a.strike, a.no_proof);
    // Artistic text's main color stands in where its effects are unknown.
    if let Some(c) = a.color.or_else(|| a.text_fx.as_ref().and_then(|f| f.main_color())) {
        let _ = write!(p, "<w:color {}/>", crate::theme_xml::word_color_attrs(c));
    }
    crate::docx_fx::write_late(&mut p, &fx);
    let cs_size = asian.and_then(|x| x.cs_size);
    if let Some(sz) = a.size {
        let _ = write!(p, r#"<w:sz w:val="{0}"/><w:szCs w:val="{1}"/>"#, sz, cs_size.unwrap_or(sz));
    } else if let Some(cs) = cs_size {
        let _ = write!(p, r#"<w:szCs w:val="{cs}"/>"#);
    }
    if let Some(h) = a.highlight {
        let _ = write!(p, r#"<w:shd w:val="clear" w:color="auto" w:fill="{:06X}"/>"#, h & 0xFF_FFFF);
    }
    if a.underline {
        p.push_str(&crate::docx_fx::underline_xml(&fx));
    }
    if let Some(x) = asian {
        crate::docx_asian::write_fit(&mut p, x);
    }
    if a.superscript {
        p.push_str(r#"<w:vertAlign w:val="superscript"/>"#);
    } else if a.subscript {
        p.push_str(r#"<w:vertAlign w:val="subscript"/>"#);
    }
    if let Some(x) = asian {
        crate::docx_asian::write_em(&mut p, x);
    }
    let asian_lang = asian.map(crate::docx_asian::lang_attrs).unwrap_or_default();
    if a.lang.is_some() || !asian_lang.is_empty() {
        p.push_str("<w:lang");
        if let Some(l) = &a.lang {
            let _ = write!(p, r#" w:val="{}""#, esc(l));
        }
        let _ = write!(p, "{asian_lang}/>");
    }
    if let Some(x) = asian {
        crate::docx_asian::write_layout(&mut p, x);
    }
    if let Some(fx) = &a.text_fx {
        crate::text_fx_xml::write_w14(&mut p, fx);
    }
    if !p.is_empty() {
        let _ = write!(out, "<w:rPr>{}</w:rPr>", p);
    }
}

/// Writes a run, wrapped in a hyperlink when it has one (`#name` targets
/// are bookmarks in the document; others get an external relationship).
fn write_run(body: &mut String, text: &str, a: &Attrs, links: &mut Vec<Arc<str>>, media: &mut docx_img::Media, nw: &mut notes::NoteWriter) {
    if (a.note.is_some() || a.field.is_some()) && notes::write_object(body, text, a, nw) {
        return;
    }
    if let Some(img) = &a.image {
        for _ in text.matches(OBJECT) {
            body.push_str("<w:r>");
            write_rpr(body, a, false);
            media.drawing(body, img);
            body.push_str("</w:r>");
        }
        return;
    }
    if let Some(sh) = &a.shape {
        for _ in text.matches(OBJECT) {
            body.push_str("<w:r>");
            write_rpr(body, a, false);
            crate::docx_shape::write(body, sh, media);
            body.push_str("</w:r>");
        }
        return;
    }
    match &a.link {
        Some(l) => {
            body.push_str("<w:hyperlink");
            match l.strip_prefix('#') {
                Some(anchor) => {
                    let _ = write!(body, r#" w:anchor="{}""#, esc(anchor));
                }
                None => {
                    let id = match links.iter().position(|x| x == l) {
                        Some(i) => i,
                        None => {
                            links.push(l.clone());
                            links.len() - 1
                        }
                    };
                    let _ = write!(body, r#" r:id="rIdL{}""#, id + 1);
                }
            }
            if let Some(tip) = &a.link_tip {
                let _ = write!(body, r#" w:tooltip="{}""#, esc(tip));
            }
            body.push_str(r#" w:history="1"><w:r>"#);
            write_rpr(body, a, true);
            write_text(body, text);
            body.push_str("</w:r></w:hyperlink>");
        }
        None => {
            body.push_str("<w:r>");
            write_rpr(body, a, false);
            write_text(body, text);
            body.push_str("</w:r>");
        }
    }
}

/// The header and footer parts of every section and each section's
/// references to them. A watermark goes in every header (as in Word).
fn hf_parts(doc: &Document, w: &mut Shared) -> (Vec<crate::docx_hf::Part>, Vec<String>) {
    use apex_doc::HfSlot;
    let wm = doc.deco.watermark.as_ref().filter(|m| !m.is_empty());
    let pic = wm.and_then(|m| m.picture.as_ref()).and_then(|data| {
        let size = crate::odt_img::pixel_size(data)?;
        let n = w.media.add(data);
        Some((w.media.file_name(n), size))
    });
    let wm_xml = wm.map(|m| docx_layout::watermark_xml(m, &doc.page, pic.as_ref().map(|(_, size)| ("rIdW1", *size)))).unwrap_or_default();
    let wm_rel = pic.map(|(file, _)| format!(r#"<Relationship Id="rIdW1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/{file}"/>"#));
    let mut parts = Vec::new();
    let mut refs = Vec::new();
    let empty = Arc::new(vec![Paragraph::new("")]);
    // A story sections share is written once (sections may be many).
    let mut written: HashMap<(usize, bool), usize> = HashMap::new();
    for s in 0..doc.sections().len() {
        let h = doc.section_headers(s);
        let mut r = String::new();
        for slot in HfSlot::ALL {
            let used = match slot.kind() {
                1 => h.different_first,
                2 => doc.odd_even,
                _ => true,
            };
            // The first section's headers carry the watermark to every page.
            let story = match h.get(slot) {
                Some(st) => st,
                None if s == 0 && used && slot.is_header() && wm.is_some() => &empty,
                None => continue,
            };
            let header = slot.is_header();
            let ty = ["default", "first", "even"][slot.kind() as usize];
            let kind = if header { "header" } else { "footer" };
            if let Some(n) = written.get(&(Arc::as_ptr(story) as usize, header)) {
                let _ = write!(r, r#"<w:{kind}Reference w:type="{ty}" r:id="rIdH{n}"/>"#);
                continue;
            }
            let tag = if header { "hdr" } else { "ftr" };
            let mut xml = String::from(HEADER);
            let vml = if header && wm.is_some() { docx_layout::VML_NS } else { "" };
            let _ = write!(xml, r#"<w:{tag} xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"{vml}>"#);
            if header {
                xml.push_str(&wm_xml);
            }
            let mut mini = Document::from_paragraphs(story.to_vec());
            mini.styles = doc.styles.clone();
            write_paras(&mini, &mut xml, w, &|_| String::new());
            // A part ends with a paragraph.
            if mini.paragraphs.last().is_some_and(|p| p.cell.is_some()) {
                xml.push_str("<w:p/>");
            }
            let _ = write!(xml, "</w:{tag}>");
            let n = parts.len() + 1;
            written.insert((Arc::as_ptr(story) as usize, header), n);
            let _ = write!(r, r#"<w:{kind}Reference w:type="{ty}" r:id="rIdH{n}"/>"#);
            let rels = if header { wm_rel.clone() } else { None };
            parts.push(crate::docx_hf::Part { header, file: format!("{}{n}.xml", if header { "header" } else { "footer" }), xml, rels });
        }
        refs.push(r);
    }
    (parts, refs)
}

/// State shared by the body and the header and footer parts.
#[derive(Default)]
struct Shared {
    links: Vec<Arc<str>>,
    media: docx_img::Media,
    nw: notes::NoteWriter,
    bookmark_ids: HashMap<String, usize>,
    numbering: Numbering,
    rev_ids: crate::docx_revisions::Ids,
}

/// Writes the paragraphs and tables of `doc` (the body, or a header or
/// footer); `sect_of(k)` is section k's `w:sectPr`.
fn write_paras(doc: &Document, body: &mut String, w: &mut Shared, sect_of: &dyn Fn(usize) -> String) {
    let sections = doc.sections();
    let mut bullet_nums: HashMap<u8, usize> = HashMap::new();
    // numIds of multilevel lists by list id, and of the headings.
    let mut list_nums: HashMap<u16, usize> = HashMap::new();
    let heading_num = doc.styles.list(doc.styles.heading_list).map(|def| {
        let a = w.numbering.def_abstract(def);
        w.numbering.add(a, None)
    });
    // Current numbered list: (numId, list_format).
    let mut cur_num: Option<(usize, u8)> = None;
    let mut numbers: Option<Vec<usize>> = None;
    let mut tables = crate::docx_table::TblWriter::new(doc);
    let comment_ids = crate::docx_comments::Ids::new(doc);
    let mut sect_k = 0;
    for (idx, p) in doc.paragraphs.iter().enumerate() {
        if !tables.before(body, idx) {
            continue;
        }
        // A section ending with a table ends in a paragraph after it.
        if idx > 0 && p.section_start().is_some() && doc.paragraphs[idx - 1].cell.is_some() {
            let _ = write!(body, "<w:p><w:pPr>{}</w:pPr></w:p>", sect_of(sect_k));
        }
        if sections.get(sect_k + 1).is_some_and(|s| s.first <= idx) {
            sect_k += 1;
        }
        let split = docx_layout::split_cap(p);
        if let Some((cap, _)) = &split {
            let (ppr, sz) = docx_layout::cap_ppr(&p.style, cap.runs.first().and_then(|r| r.attrs.size).unwrap_or(doc.base_size(p.style.kind)));
            let _ = write!(body, "<w:p><w:pPr>{ppr}</w:pPr>");
            for (s, e, a) in cap.spans() {
                write_run(body, &cap.text[s..e], &Attrs { size: Some(sz.min(3276) as u16), ..a.clone() }, &mut w.links, &mut w.media, &mut w.nw);
            }
            body.push_str("</w:p>");
        }
        let p = split.as_ref().map_or(p, |s| &s.1);
        body.push_str("<w:p>");
        let st = &p.style;
        let mut ppr = String::new();
        if let Some(id) = crate::docx_styles::custom_id(doc, st).or_else(|| style_id(st.kind)).or_else(|| notes::toc_style(st)) {
            let _ = write!(ppr, r#"<w:pStyle w:val="{}"/>"#, esc(&id));
        }
        if st.keep_with_next && !matches!(st.kind, ParaKind::Heading(_)) {
            ppr.push_str("<w:keepNext/>");
        }
        if st.keep_lines {
            ppr.push_str("<w:keepLines/>");
        }
        if st.page_break_before {
            ppr.push_str("<w:pageBreakBefore/>");
        }
        if st.no_widow_control {
            ppr.push_str(r#"<w:widowControl w:val="0"/>"#);
        }
        match st.kind {
            ParaKind::Heading(h) if heading_num.is_some() => {
                let _ = write!(ppr, r#"<w:numPr><w:ilvl w:val="{}"/><w:numId w:val="{}"/></w:numPr>"#, h - 1, heading_num.unwrap_or(0));
            }
            ParaKind::Bullet | ParaKind::Numbered if doc.styles.list(st.list).is_some() => {
                let def = doc.styles.list(st.list).cloned().unwrap_or_default();
                let fresh = st.restart || st.start_at > 0;
                let id = match list_nums.get(&st.list) {
                    Some(&id) if !fresh => id,
                    _ => {
                        let a = w.numbering.def_abstract(&def);
                        let start = fresh.then(|| (st.level, if st.start_at > 0 { st.start_at as usize } else { def.level(st.level as usize).map_or(1, |l| l.start as usize) }));
                        let id = w.numbering.add(a, start);
                        list_nums.insert(st.list, id);
                        id
                    }
                };
                let _ = write!(ppr, r#"<w:numPr><w:ilvl w:val="{}"/><w:numId w:val="{}"/></w:numPr>"#, st.level, id);
            }
            ParaKind::Bullet => {
                let id = match bullet_nums.get(&st.list_format) {
                    Some(&id) => id,
                    None => {
                        let a = w.numbering.abstract_id(true, st.list_format);
                        let id = w.numbering.add(a, None);
                        bullet_nums.insert(st.list_format, id);
                        id
                    }
                };
                let _ = write!(ppr, r#"<w:numPr><w:ilvl w:val="{}"/><w:numId w:val="{}"/></w:numPr>"#, st.level, id);
            }
            ParaKind::Numbered => {
                let n = numbers.get_or_insert_with(|| doc.list_numbers())[idx];
                let fresh = st.restart || (st.level == 0 && n == 1);
                let id = match cur_num {
                    Some((id, f)) if !fresh && f == st.list_format => id,
                    _ => {
                        let a = w.numbering.abstract_id(false, st.list_format);
                        let id = w.numbering.add(a, Some((st.level, n)));
                        cur_num = Some((id, st.list_format));
                        id
                    }
                };
                let _ = write!(ppr, r#"<w:numPr><w:ilvl w:val="{}"/><w:numId w:val="{}"/></w:numPr>"#, st.level, id);
            }
            _ => {}
        }
        if st.no_line_numbers {
            ppr.push_str("<w:suppressLineNumbers/>");
        }
        docx_layout::write_pbdr(&mut ppr, st);
        if let Some(c) = st.shading {
            let _ = write!(ppr, r#"<w:shd w:val="clear" w:color="auto" w:fill="{:06X}"/>"#, c & 0xFF_FFFF);
        }
        let before = ppr.len();
        notes::toc_tabs(&mut ppr, st, doc);
        if ppr.len() == before {
            docx_layout::write_tabs(&mut ppr, st);
        }
        crate::docx_asian::write_ppr(&mut ppr, &st.asian);
        let mut sp = String::new();
        if let Some(b) = st.space_before {
            let _ = write!(sp, r#" w:before="{}""#, ((b * TWIP).round() as i32).max(0));
        }
        if let Some(a) = st.space_after {
            let _ = write!(sp, r#" w:after="{}""#, ((a * TWIP).round() as i32).max(0));
        }
        // (At least 0 is a single line of the text's own height.)
        if st.line_spacing > 0.0 || st.line_rule == LineRule::AtLeast {
            match st.line_rule {
                LineRule::Multiple => {
                    let _ = write!(sp, r#" w:line="{}" w:lineRule="auto""#, (st.line_spacing * 240.0).round() as i32);
                }
                LineRule::Exact => {
                    let _ = write!(sp, r#" w:line="{}" w:lineRule="exact""#, (st.line_spacing * TWIP).round() as i32);
                }
                LineRule::AtLeast => {
                    let _ = write!(sp, r#" w:line="{}" w:lineRule="atLeast""#, (st.line_spacing * TWIP).round() as i32);
                }
            }
        }
        if !sp.is_empty() {
            let _ = write!(ppr, "<w:spacing{}/>", sp);
        }
        let tw = |v: f32| (v * TWIP).round() as i32;
        let mut ind = String::new();
        if st.kind.is_list() {
            if st.indent_left != 0.0 || st.first_line != 0.0 {
                // w:hanging can't be negative: a first line past the text
                // start is a w:firstLine.
                let hang = 360 - tw(st.first_line);
                let (k, v) = if hang < 0 { ("firstLine", -hang) } else { ("hanging", hang) };
                let _ = write!(ind, r#" w:left="{}" w:{k}="{v}""#, 720 * (st.level as i32 + 1) + tw(st.indent_left));
            }
        } else {
            if st.level > 0 || st.indent_left != 0.0 {
                let _ = write!(ind, r#" w:left="{}""#, st.level as i32 * 720 + tw(st.indent_left));
            }
            if st.first_line > 0.0 {
                let _ = write!(ind, r#" w:firstLine="{}""#, tw(st.first_line));
            } else if st.first_line < 0.0 {
                let _ = write!(ind, r#" w:hanging="{}""#, -tw(st.first_line));
            }
        }
        if st.indent_right != 0.0 {
            let _ = write!(ind, r#" w:right="{}""#, tw(st.indent_right));
        }
        if !ind.is_empty() {
            let _ = write!(ppr, "<w:ind{}/>", ind);
        }
        if st.contextual_spacing {
            ppr.push_str("<w:contextualSpacing/>");
        }
        match st.align {
            Align::Left => {}
            Align::Center => ppr.push_str(r#"<w:jc w:val="center"/>"#),
            Align::Right => ppr.push_str(r#"<w:jc w:val="right"/>"#),
            Align::Justify => ppr.push_str(r#"<w:jc w:val="both"/>"#),
        }
        crate::docx_asian::write_font_align(&mut ppr, &st.asian);
        let mark = crate::docx_revisions::para_mark(p, &mut w.rev_ids);
        match p.runs.first().filter(|_| p.text.is_empty() && mark.is_empty()) {
            // An empty paragraph's look is its mark's.
            Some(r) => write_rpr(&mut ppr, &r.attrs, false),
            None => ppr.push_str(&mark),
        }
        // The last paragraph of a section (but the last) holds its setup.
        if p.cell.is_none() && doc.paragraphs.get(idx + 1).is_some_and(|n| n.section_start().is_some()) {
            ppr.push_str(&sect_of(sect_k));
        }
        if !ppr.is_empty() {
            let _ = write!(body, "<w:pPr>{}</w:pPr>", ppr);
        }
        if st.column_break_before && idx > 0 && p.cell.is_none() {
            body.push_str(r#"<w:r><w:br w:type="column"/></w:r>"#);
        }
        notes::toc_begin(body, doc, idx);
        // Runs, split where bookmarks start or end (starts first at the
        // same offset, so point bookmarks stay well formed).
        let mut marks: Vec<&Mark> = p.marks.iter().collect();
        marks.sort_by_key(|m| (m.offset, m.end));
        let mut mi = 0;
        let mut emit_marks = |body: &mut String, upto: usize, mi: &mut usize| {
            while let Some(m) = marks.get(*mi).filter(|m| m.offset <= upto) {
                *mi += 1;
                if comment_ids.mark(body, m) {
                    continue;
                }
                if let Some((id, who)) = apex_doc::perm_parts(&m.name) {
                    match (m.end, who.split_once('=')) {
                        (true, _) => _ = write!(body, r#"<w:permEnd w:id="{id}"/>"#),
                        (false, Some((k, v))) => _ = write!(body, r#"<w:permStart w:id="{id}" w:{k}="{}"/>"#, esc(v)),
                        (false, None) => _ = write!(body, r#"<w:permStart w:id="{id}"/>"#),
                    }
                    continue;
                }
                let n = w.bookmark_ids.len();
                let id = *w.bookmark_ids.entry(m.name.to_lowercase()).or_insert(n);
                if m.end {
                    let _ = write!(body, r#"<w:bookmarkEnd w:id="{id}"/>"#);
                } else {
                    let _ = write!(body, r#"<w:bookmarkStart w:id="{id}" w:name="{}"/>"#, esc(&m.name));
                }
            }
        };
        // The open control; for a form field, its run properties and the
        // bookmark written for it.
        let mut open_ctl: Option<(Arc<ContentControl>, String, Option<usize>)> = None;
        let close_ctl = |body: &mut String, open: &Option<(Arc<ContentControl>, String, Option<usize>)>| match open {
            Some((c, rpr, bm)) if c.legacy.is_some() => {
                crate::docx_forms::close(body, c, rpr);
                if let Some(id) = bm {
                    let _ = write!(body, r#"<w:bookmarkEnd w:id="{id}"/>"#);
                }
            }
            Some(_) => body.push_str(crate::docx_controls::CLOSE),
            None => {}
        };
        // End of the phonetic guide written last.
        let mut ruby_end = 0;
        for (s, e, a) in p.spans() {
            if s == e || s < ruby_end {
                continue;
            }
            let id = a.control.as_ref().map(|c| c.id);
            if id != open_ctl.as_ref().map(|o| o.0.id) {
                close_ctl(body, &open_ctl);
                open_ctl = None;
                if let Some(c) = &a.control {
                    match &c.legacy {
                        Some(f) => {
                            // Bookmarks starting here go before the field; one
                            // with its name if the document has none.
                            emit_marks(body, s, &mut mi);
                            let mut bm = None;
                            if !f.name.is_empty() && !p.marks.iter().any(|m| *m.name == *f.name) && doc.bookmark(&f.name).is_none() {
                                // Ids apart from the document's bookmarks.
                                let id = 1_000_000 + c.id as usize;
                                let _ = write!(body, r#"<w:bookmarkStart w:id="{id}" w:name="{}"/>"#, esc(&f.name));
                                bm = Some(id);
                            }
                            let mut rpr = String::new();
                            write_rpr(&mut rpr, &Attrs { control: None, link: None, ..a.clone() }, false);
                            let text = &p.text[s..e];
                            crate::docx_forms::open(body, c, f, text, &rpr);
                            open_ctl = Some((c.clone(), rpr, bm));
                        }
                        None => {
                            crate::docx_controls::open(body, c);
                            open_ctl = Some((c.clone(), String::new(), None));
                        }
                    }
                }
            }
            // Check boxes and drop-downs have no result to write.
            if a.control.as_ref().is_some_and(|c| c.legacy.is_some() && !matches!(c.kind, apex_doc::ControlKind::PlainText | apex_doc::ControlKind::RichText)) {
                continue;
            }
            let mut a = a.clone();
            if st.kind == ParaKind::Code {
                a.code = false;
            }
            // A phonetic guide over all the runs that share it.
            if let Some(ruby) = a.asian.as_ref().and_then(|x| x.ruby.clone()) {
                emit_marks(body, s, &mut mi);
                let group: Vec<(usize, usize, Attrs)> = p.spans().filter(|(gs, ge, _)| *gs >= s && ge > gs).take_while(|(_, _, b)| b.asian.as_ref().and_then(|x| x.ruby.as_ref()) == Some(&ruby)).map(|(gs, ge, b)| (gs, ge, b.clone())).collect();
                ruby_end = group.last().map_or(e, |g| g.1);
                crate::docx_asian::write_ruby_start(body, &ruby, a.size.unwrap_or(doc.base_size(st.kind)));
                for (gs, ge, mut ga) in group {
                    ga.set_asian(|x| x.ruby = None);
                    ga.code &= st.kind != ParaKind::Code;
                    body.push_str("<w:r>");
                    write_rpr(body, &ga, false);
                    write_text(body, &p.text[gs..ge]);
                    body.push_str("</w:r>");
                }
                body.push_str(crate::docx_asian::RUBY_END);
                continue;
            }
            let mut pos = s;
            while pos < e {
                emit_marks(body, pos, &mut mi);
                let next = marks.get(mi).map_or(e, |m| m.offset.min(e));
                if a.math.is_some() {
                    crate::omml::write(body, &p.text[pos..next], &a, apex_doc::math::is_display(p));
                    pos = next;
                    continue;
                }
                let at = body.len();
                write_run(body, &p.text[pos..next], &a, &mut w.links, &mut w.media, &mut w.nw);
                crate::docx_revisions::wrap(body, at, a.rev.as_ref(), &mut w.rev_ids);
                pos = next;
            }
        }
        close_ctl(body, &open_ctl);
        emit_marks(body, usize::MAX, &mut mi);
        notes::toc_end(body, doc, idx);
        body.push_str("</w:p>");
    }
    tables.close(body);
}

/// Writes a docx package (a docm one if the document has a VBA project).
pub fn write(doc: &Document) -> Vec<u8> {
    write_as(doc, crate::Flavor::for_macros(&doc.macros))
}

/// Writes the package as docx, docm, dotx or dotm.
pub fn write_as(doc: &Document, flavor: crate::Flavor) -> Vec<u8> {
    // Sequence fields (captions) are written with their numbers.
    let fresh = doc.with_seq_results();
    let doc = fresh.as_ref().unwrap_or(doc);
    // Text in a linked paragraph style gets its character style twin.
    let linked = crate::docx_styles::linked_as_char(doc);
    let doc = linked.as_ref().unwrap_or(doc);
    // Custom XML takes the values of the controls bound to it.
    let bound = crate::docx_bind::has_bindings(doc).then(|| {
        let mut d = doc.clone();
        crate::docx_bind::sync(&mut d, &|_| String::new());
        d
    });
    let doc = bound.as_ref().unwrap_or(doc);
    crate::theme_xml::set_writing_theme(&doc.styles.theme);
    let mut w = Shared::default();
    w.media.groups = crate::docx_shape::group_plan(doc);
    let mut body = String::with_capacity(doc.paragraphs.iter().map(|p| p.text.len() + 120).sum());
    let sections = doc.sections();
    let (hf_parts, sect_refs) = hf_parts(doc, &mut w);
    let sect_of = |k: usize| {
        let s = &sections[k];
        docx_layout::sect_xml(&format!("{}{}", sect_refs[k], notes::sect_pr(doc)), doc.section_headers(k), s.page, s.columns, s.start, &doc.deco.borders)
    };
    write_paras(doc, &mut body, &mut w, &sect_of);
    body.push_str(&sect_of(sections.len() - 1));
    let mut document = String::with_capacity(body.len() + 512);
    document.push_str(HEADER);
    document.push_str(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" mc:Ignorable="w14""#);
    document.push_str(docx_layout::VML_NS);
    document.push('>');
    if let Some(c) = doc.deco.color {
        docx_layout::write_background(&mut document, c, doc.deco.gradient.as_ref());
    }
    document.push_str("<w:body>");
    document.push_str(&body);
    document.push_str("</w:body></w:document>");

    let comment_parts = crate::docx_comments::parts(doc);
    let comment_types = if comment_parts.is_some() { crate::docx_comments::CONTENT_TYPES } else { "" };
    let note_parts = w.nw.parts(doc.notes.separator);
    let note_types: String = note_parts.iter().map(|p| format!(r#"<Override PartName="/word/{}" ContentType="{}"/>"#, p.0, p.2)).collect();
    let mut z = ZipWriter::new();
    let (kept_rels, kept_pkg, kept_types) = crate::ooxml_keep::write_common(&mut z, &doc.kept, "word", None);
    let kept = crate::macro_parts::write(&mut z, &doc.macros, "word", flavor);
    z.add("[Content_Types].xml", format!("{}{}{}{}{}{}{}{}{}{}{}{}{}", HEADER, r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>"#, w.media.content_types(), format!(r#"<Override PartName="/word/document.xml" ContentType="{}"/>{}"#, flavor.document_type(), kept.types), r#"<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#, crate::props::content_types(&doc.meta), r#"<Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/word/theme/theme1.xml" ContentType=""#, crate::theme_xml::CONTENT_TYPE, r#""/>"#, crate::docx_hf::content_types(&hf_parts), format!("{note_types}{comment_types}{}{}", if doc.biblio.sources.is_empty() { "" } else { crate::docx_biblio::CONTENT_TYPE }, if doc.fonts.faces.is_empty() { "" } else { crate::docx_fonts::CONTENT_TYPES }) + if doc.book.is_some() { crate::book_xml::DOCX_CONTENT_TYPE } else { "" }, kept_types, "</Types>").as_bytes(), true);
    z.add("_rels/.rels", format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>{}{}{kept_pkg}</Relationships>"#, crate::props::rels(&doc.meta), kept.root_rels).as_bytes(), true);
    let mut rels = String::from(HEADER);
    rels.push_str(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>"#);
    for (i, l) in w.links.iter().enumerate() {
        let _ = write!(rels, r#"<Relationship Id="rIdL{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="{}" TargetMode="External"/>"#, i + 1, esc(l));
    }
    let _ = write!(rels, r#"<Relationship Id="rIdT" Type="{}" Target="theme/theme1.xml"/>"#, crate::theme_xml::REL_TYPE);
    z.add("word/theme/theme1.xml", crate::theme_xml::write_theme(&doc.styles.theme).as_bytes(), true);
    rels.push_str(r#"<Relationship Id="rIdS" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/>"#);
    rels.push_str(&crate::docx_hf::rels(&hf_parts));
    if crate::docx_biblio::write(&mut z, &doc.biblio) {
        rels.push_str(crate::docx_biblio::REL);
    }
    if crate::docx_fonts::write(&mut z, &doc.fonts) {
        rels.push_str(crate::docx_fonts::REL);
    }
    if let Some(b) = &doc.book {
        crate::book_xml::write_docx(&mut z, b);
        rels.push_str(crate::book_xml::DOCX_REL);
    }
    rels.push_str(&kept_rels);
    for (i, p) in note_parts.iter().enumerate() {
        let _ = write!(rels, r#"<Relationship Id="rIdN{}" Type="{}" Target="{}"/>"#, i + 1, p.3, p.0);
    }
    if let Some((comments, extended)) = &comment_parts {
        rels.push_str(crate::docx_comments::RELS);
        z.add("word/comments.xml", comments.as_bytes(), true);
        z.add("word/commentsExtended.xml", extended.as_bytes(), true);
    }
    rels.push_str(&w.media.rels());
    rels.push_str(&kept.main_rels);
    for (i, f) in w.media.files.iter().enumerate() {
        z.add(&format!("word/media/{}", w.media.file_name(i + 1)), f, false);
    }
    w.media.objects.write_files(&mut z);
    for (i, x) in w.media.inks.iter().enumerate() {
        z.add(&format!("word/ink/ink{}.xml", i + 1), x.as_bytes(), true);
    }
    rels.push_str("</Relationships>");
    z.add("word/_rels/document.xml.rels", rels.as_bytes(), true);
    z.add("word/document.xml", document.as_bytes(), true);
    z.add("word/styles.xml", styles_xml(doc).as_bytes(), true);
    z.add("word/numbering.xml", w.numbering.xml().as_bytes(), true);
    let even = if doc.odd_even { "<w:evenAndOddHeaders/>" } else { "" };
    let hyph = if doc.auto_hyphenation { "<w:autoHyphenation/>" } else { "" };
    let track = if doc.track_changes { "<w:trackRevisions/>" } else { "" };
    let note_settings = notes::settings(doc, &w.nw);
    let bg = if doc.deco.color.is_some() { "<w:displayBackgroundShape/>" } else { "" };
    let bg = format!("{bg}{}", crate::docx_fonts::settings(&doc.fonts));
    let protect = doc.protection.as_ref().map(protection_xml).unwrap_or_default();
    let mirror = if doc.page.mirror { "<w:mirrorMargins/>" } else { "" };
    let gutter_top = if doc.page.gutter_top { "<w:gutterAtTop/>" } else { "" };
    let write_protect = if doc.meta.read_only_recommended { r#"<w:writeProtection w:recommended="1"/>"# } else { "" };
    let asian = doc.kept.xml.iter().find(|(slot, _)| slot == crate::docx_asian::SETTINGS_SLOT).map_or("", |(_, x)| x.as_str());
    z.add("word/settings.xml", format!(r#"{HEADER}<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{write_protect}{bg}{mirror}{gutter_top}{track}{protect}{hyph}{even}{asian}{note_settings}</w:settings>"#).as_bytes(), true);
    for (file, xml, _, _, prels) in &note_parts {
        z.add(&format!("word/{file}"), xml.as_bytes(), true);
        if let Some(r) = prels {
            z.add(&format!("word/_rels/{file}.rels"), r.as_bytes(), true);
        }
    }
    // Header and footer parts may use any picture or link.
    let mut part_rels = String::new();
    for (i, l) in w.links.iter().enumerate() {
        let _ = write!(part_rels, r#"<Relationship Id="rIdL{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="{}" TargetMode="External"/>"#, i + 1, esc(l));
    }
    part_rels.push_str(&w.media.rels());
    for p in &hf_parts {
        z.add(&format!("word/{}", p.file), p.xml.as_bytes(), true);
        let extra = p.rels.as_deref().unwrap_or("");
        z.add(&format!("word/_rels/{}.rels", p.file), format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{part_rels}{extra}</Relationships>"#).as_bytes(), true);
    }
    crate::props::write_ooxml(&mut z, &doc.meta);
    z.finish()
}

/// "Always open read-only" in settings.xml (`w:writeProtection`).
fn read_only_recommended(settings: &str) -> bool {
    let mut r = Reader::new(settings);
    loop {
        match r.next() {
            Ev::Eof => return false,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "writeProtection" => return t.attr("w:recommended").is_some_and(|v| matches!(v.as_ref(), "1" | "true" | "on")),
            _ => {}
        }
    }
}

/// Restrict Editing from settings.xml (`w:documentProtection`).
fn read_protection(settings: &str) -> Option<apex_doc::Protection> {
    let mut r = Reader::new(settings);
    loop {
        match r.next() {
            Ev::Eof => return None,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "documentProtection" => {
                let limit = t.attr("w:edit").and_then(|v| apex_doc::EditLimit::from_docx(&v))?;
                let enforced = t.attr("w:enforcement").is_some_and(|v| matches!(v.as_ref(), "1" | "true" | "on"));
                let password = match (t.attr("w:algorithmName"), t.attr("w:hashValue"), t.attr("w:saltValue")) {
                    (Some(a), Some(h), Some(s)) => Some(apex_doc::ProtectionHash {
                        algorithm: a.into_owned(),
                        hash: h.into_owned(),
                        salt: s.into_owned(),
                        spin: t.attr("w:spinCount").and_then(|v| v.parse().ok()).unwrap_or(0),
                        legacy: Vec::new(),
                    }),
                    _ if t.attr("w:hash").is_some() => {
                        let keep = ["w:cryptProviderType", "w:cryptAlgorithmClass", "w:cryptAlgorithmType", "w:cryptAlgorithmSid", "w:cryptSpinCount", "w:hash", "w:salt"];
                        let legacy = keep.iter().filter_map(|k| t.attr(k).map(|v| (k.to_string(), v.into_owned()))).collect();
                        Some(apex_doc::ProtectionHash { algorithm: String::new(), hash: String::new(), salt: String::new(), spin: 0, legacy })
                    }
                    _ => None,
                };
                return Some(apex_doc::Protection { limit, enforced, password });
            }
            _ => {}
        }
    }
}

fn protection_xml(p: &apex_doc::Protection) -> String {
    let mut s = format!(r#"<w:documentProtection w:edit="{}" w:enforcement="{}""#, p.limit.docx(), u8::from(p.enforced));
    match &p.password {
        Some(h) if !h.algorithm.is_empty() => {
            let _ = write!(s, r#" w:algorithmName="{}" w:hashValue="{}" w:saltValue="{}" w:spinCount="{}""#, esc(&h.algorithm), esc(&h.hash), esc(&h.salt), h.spin);
        }
        Some(h) => {
            for (k, v) in &h.legacy {
                let _ = write!(s, r#" {k}="{}""#, esc(v));
            }
        }
        None => {}
    }
    s.push_str("/>");
    s
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use apex_doc::{PageSetup, Pos, Selection};

    pub(crate) fn sample() -> Document {
        let mut ps = Vec::new();
        ps.push(Paragraph::new("Informe").with_style(ParaStyle { kind: ParaKind::Title, ..Default::default() }));
        ps.push(Paragraph::new("Introducción").with_style(ParaStyle { kind: ParaKind::Heading(1), ..Default::default() }));
        let mut p = Paragraph::new("");
        p.push("Texto ", Attrs::default());
        p.push("negrita", Attrs { bold: true, ..Default::default() });
        p.push(" y ", Attrs::default());
        p.push("enlace", Attrs { link: Some("https://example.com".into()), ..Default::default() });
        p.push("\tcon tab, ", Attrs::default());
        p.push("rojo", Attrs { color: Some(0xFF0000), size: Some(32), ..Default::default() });
        p.push(" y ", Attrs::default());
        p.push("code", Attrs { code: true, ..Default::default() });
        p.style.align = Align::Justify;
        p.style.line_spacing = 1.5;
        ps.push(p);
        ps.push(Paragraph::new("uno").with_style(ParaStyle { kind: ParaKind::Bullet, ..Default::default() }));
        ps.push(Paragraph::new("anidado").with_style(ParaStyle { kind: ParaKind::Bullet, level: 1, ..Default::default() }));
        ps.push(Paragraph::new("primero").with_style(ParaStyle { kind: ParaKind::Numbered, ..Default::default() }));
        ps.push(Paragraph::new("segundo").with_style(ParaStyle { kind: ParaKind::Numbered, ..Default::default() }));
        ps.push(Paragraph::new("Una cita").with_style(ParaStyle { kind: ParaKind::Quote, ..Default::default() }));
        ps.push(Paragraph::new("fn main() {}").with_style(ParaStyle { kind: ParaKind::Code, ..Default::default() }));
        ps.push(Paragraph::new("Nueva página").with_style(ParaStyle { page_break_before: true, align: Align::Center, ..Default::default() }));
        ps.push(Paragraph::new("Con sangría\u{2028}y salto").with_style(ParaStyle { indent_left: 18.0, indent_right: 12.0, first_line: 24.0, space_before: Some(6.0), space_after: Some(0.0), shading: Some(0xFFEECC), keep_with_next: true, ..Default::default() }));
        ps.push(Paragraph::new("Francesa").with_style(ParaStyle { level: 1, first_line: -18.0, line_rule: LineRule::Exact, line_spacing: 20.0, ..Default::default() }));
        ps.push(Paragraph::new("Mínimo").with_style(ParaStyle { line_rule: LineRule::AtLeast, line_spacing: 14.0, ..Default::default() }));
        ps.push(Paragraph::new("check").with_style(ParaStyle { kind: ParaKind::Bullet, list_format: 5, ..Default::default() }));
        ps.push(Paragraph::new("a)").with_style(ParaStyle { kind: ParaKind::Numbered, list_format: 3, ..Default::default() }));
        ps.push(Paragraph::new("b)").with_style(ParaStyle { kind: ParaKind::Numbered, list_format: 3, ..Default::default() }));
        ps.push(Paragraph::new("reinicia").with_style(ParaStyle { kind: ParaKind::Numbered, list_format: 3, restart: true, ..Default::default() }));
        let mut p = Paragraph::new("");
        p.push("x", Attrs::default());
        p.push("2", Attrs { superscript: true, ..Default::default() });
        p.push(" H", Attrs { italic: true, underline: true, ..Default::default() });
        p.push("2", Attrs { subscript: true, ..Default::default() });
        p.push("O ", Attrs::default());
        p.push("tachado", Attrs { strike: true, ..Default::default() });
        p.push(" resaltado", Attrs { highlight: Some(0xFFFF00), font: Some("Georgia".into()), ..Default::default() });
        p.style.shading = Some(0xD9D9D9);
        ps.push(p);
        let mut p = Paragraph::new("");
        let fx = |f: apex_doc::CharFx| Some(Arc::new(f));
        use apex_doc::{CharFx, UnderlineStyle as U};
        p.push("versalitas ", Attrs { fx: fx(CharFx { small_caps: true, ..Default::default() }), ..Default::default() });
        p.push("mayús ", Attrs { fx: fx(CharFx { all_caps: true, spacing: 40, ..Default::default() }), ..Default::default() });
        p.push("doble ", Attrs { fx: fx(CharFx { double_strike: true, scale: 150, ..Default::default() }), ..Default::default() });
        p.push("oculto ", Attrs { fx: fx(CharFx { hidden: true, position: 6, ..Default::default() }), ..Default::default() });
        p.push("onda ", Attrs { underline: true, fx: fx(CharFx { underline: U::Wave, underline_color: Some(0xFF0000), ..Default::default() }), ..Default::default() });
        p.push("doble", Attrs { underline: true, fx: fx(CharFx { underline: U::Double, spacing: -20, ..Default::default() }), ..Default::default() });
        ps.push(p);
        ps.push(Paragraph::new("romano").with_style(ParaStyle { kind: ParaKind::Numbered, list_format: 6, indent_left: 10.0, ..Default::default() }));
        let mut d = Document::from_paragraphs(ps);
        d.meta.title = "Prueba".into();
        d.meta.author = "Ana".into();
        d.page = PageSetup { margin_left: 90.0, margin_right: 54.0, ..PageSetup::LETTER };
        d
    }

    pub(crate) fn check(back: &Document, orig: &Document) {
        assert_eq!(back.paragraphs.len(), orig.paragraphs.len());
        for (a, b) in back.paragraphs.iter().zip(&orig.paragraphs) {
            assert_eq!(a.text, b.text);
            assert_eq!(a.style, b.style, "{}", a.text);
            let sa: Vec<_> = a.spans().map(|(s, e, x)| (s, e, x.clone())).collect();
            let sb: Vec<_> = b.spans().map(|(s, e, x)| (s, e, x.clone())).collect();
            assert_eq!(sa, sb, "{}", a.text);
        }
        assert_eq!(apex_doc::Meta { created: String::new(), ..back.meta.clone() }, orig.meta);
        assert!((back.page.width - orig.page.width).abs() < 0.1);
        assert!((back.page.margin_left - orig.page.margin_left).abs() < 0.1);
        assert!((back.page.margin_right - orig.page.margin_right).abs() < 0.1);
    }

    #[test]
    fn roundtrip() {
        let d = sample();
        let bytes = write(&d);
        if let Some(dir) = std::env::var_os("APEX_DUMP") {
            std::fs::write(std::path::Path::new(&dir).join("sample.docx"), &bytes).unwrap();
        }
        let back = read(&bytes).unwrap();
        check(&back, &d);
    }

    #[test]
    fn captions_and_table_of_figures() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("a"), Paragraph::new("b"), Paragraph::new("c")]);
        d.insert_caption(Selection::caret(Pos::new(1, 0)), "Ilustración", ": Uno", false, "Descripción");
        d.insert_caption(Selection::caret(Pos::new(3, 0)), "Ilustración", ": Dos", false, "Descripción");
        d.insert_toc(Selection::caret(Pos::new(0, 0)), apex_doc::TocOpts { caption: 1, ..Default::default() }, "-");
        let bytes = write(&d);
        if let Some(dir) = std::env::var_os("APEX_DUMP") {
            std::fs::write(std::path::Path::new(&dir).join("figures.docx"), &bytes).unwrap();
        }
        let back = read(&bytes).unwrap();
        assert_eq!(back.captions(1).len(), 2);
        let refs = back.refs();
        assert_eq!(back.caption_text(back.captions(1)[1], &refs), "Ilustración 2: Dos");
        let toc: Vec<_> = back.paragraphs.iter().filter_map(|p| p.style.toc).collect();
        assert_eq!(toc.len(), 2);
        assert!(toc.iter().all(|t| t.opts.caption == 1 && t.level == 1));
        assert!(back.styles.by_id(apex_doc::CAPTION_STYLE).is_some_and(|s| s.italic == Some(true)));
    }

    #[test]
    fn multilevel_lists() {
        let mut d = Document::from_paragraphs(["Uno", "a", "b", "c", "Dos", "d", "e"].iter().map(|t| Paragraph::new(*t)).collect());
        let sel = Selection::caret(Pos::new(0, 0));
        d.paragraphs[0].style.kind = ParaKind::Heading(1);
        d.paragraphs[4].style.kind = ParaKind::Heading(2);
        d.set_heading_numbering(Some(&apex_doc::ListDef::headings()), sel);
        d.apply_list(1, 3, sel, &apex_doc::ListDef::outline());
        d.paragraphs[2].style.level = 1;
        d.apply_list(5, 6, sel, &apex_doc::ListDef::outline());
        d.set_list_value(5, sel, Some(5));
        let labels = |d: &Document| d.list_markers().into_iter().map(|m| match m {
            Some(apex_doc::Marker::Text(t)) => t,
            _ => String::new(),
        }).collect::<Vec<_>>();
        let want = labels(&d);
        assert_eq!(want, ["1", "1.", "1.1.", "2.", "1.1", "5.", "6."]);
        let bytes = write(&d);
        if let Some(dir) = std::env::var_os("APEX_DUMP") {
            std::fs::write(std::path::Path::new(&dir).join("lists.docx"), &bytes).unwrap();
        }
        let back = read(&bytes).unwrap();
        assert_eq!(labels(&back), want);
    }

    /// A table with a table nested in its second cell (after some text) and
    /// another in the first cell of the nested one; shared with odt.
    pub(crate) fn nested() -> Document {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("antes"), Paragraph::new("después")]);
        d.insert_table(Selection::caret(Pos::new(1, 0)), 2, 2, 300.0);
        // Cells: 1..5; type into the first two.
        let p = d.insert_text(Selection::caret(Pos::new(1, 0)), "A1", None);
        let _ = p;
        let s = d.insert_text(Selection::caret(Pos::new(2, 0)), "B1", None);
        // Nested 2x3 table at the end of B1's text.
        let s = d.insert_table(Selection::caret(s), 2, 3, 120.0);
        d.insert_text(s, "x", None);
        let inner = d.table_map(s.cursor.para).unwrap();
        let (c, _) = inner.cell(1, 2);
        d.insert_text(Selection::caret(Pos::new(c, 0)), "fin", None);
        // Two levels in.
        let s = d.insert_table(Selection::caret(Pos::new(inner.cell(0, 1).0, 0)), 1, 1, 40.0);
        d.insert_text(s, "hondo", None);
        d.set_cell_props(Selection::caret(s.cursor), &|p, _| p.shading = Some(0xFFEE00));
        d
    }

    /// Texts and nesting levels of the paragraphs, and the shape of every table.
    pub(crate) fn nest_shape(d: &Document) -> (Vec<(String, usize)>, Vec<(usize, usize, usize)>) {
        let paras = d.paragraphs.iter().map(|p| (p.text.clone(), p.cell.as_ref().map_or(0, |c| c.depth()))).collect();
        let tables = d.all_tables().iter().map(|m| (m.rows, m.cols, m.depth)).collect();
        (paras, tables)
    }

    #[test]
    fn nested_tables() {
        let d = nested();
        let (paras, tables) = nest_shape(&d);
        assert_eq!(tables, vec![(2, 2, 0), (2, 3, 1), (1, 1, 2)]);
        assert!(paras.iter().any(|(t, n)| t == "hondo" && *n == 3));
        let bytes = write(&d);
        if let Some(dir) = std::env::var_os("APEX_DUMP") {
            std::fs::write(std::path::Path::new(&dir).join("nested.docx"), &bytes).unwrap();
        }
        let back = read(&bytes).unwrap();
        assert_eq!(nest_shape(&back), (paras, tables));
        let deep = back.all_tables()[2].start;
        assert_eq!(back.table_map(deep).unwrap().cell_props(&back.paragraphs, 0, 0).and_then(|p| p.shading), Some(0xFFEE00));
    }

    /// A document with pictures of every kind, shared with the odt tests.
    pub(crate) fn with_images() -> Document {
        use apex_doc::{Image, Wrap};
        let png: Arc<[u8]> = Arc::from(&b"\x89PNG\r\n\x1a\nfake"[..]);
        let jpg: Arc<[u8]> = Arc::from(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3][..]);
        let mut d = Document::from_paragraphs(vec![Paragraph::new("a"), Paragraph::new("b"), Paragraph::new("")]);
        let inline = Image { crop: [0.1, 0.0, 0.25, 0.05], rotation: 90.0, name: "Pic".into(), alt: "A <cat> & dog".into(), ..Image::new(png.clone(), 144.0, 72.5) };
        d.insert_image(Selection::caret(Pos::new(0, 1)), inline);
        let square = Image { wrap: Wrap::Square, align: Some(Align::Right), ..Image::new(jpg, 50.0, 60.0) };
        d.insert_image(Selection::caret(Pos::new(1, 0)), square);
        let tb = Image { wrap: Wrap::TopBottom, offset_x: 36.0, ..Image::new(png, 100.0, 20.0) };
        d.insert_image(Selection::caret(Pos::new(2, 0)), tb);
        d
    }

    pub(crate) fn check_images(back: &Document, orig: &Document) {
        let (a, b) = (back.images(), orig.images());
        assert_eq!(a.len(), b.len());
        for ((pa, ia), (pb, ib)) in a.iter().zip(&b) {
            assert_eq!(pa, pb);
            assert_eq!(ia.data, ib.data);
            for (x, y) in [(ia.width, ib.width), (ia.height, ib.height), (ia.rotation, ib.rotation), (ia.offset_x, ib.offset_x)].into_iter().chain(ia.crop.into_iter().zip(ib.crop)) {
                assert!((x - y).abs() < 0.01, "{ia:?} vs {ib:?}");
            }
            assert_eq!((ia.wrap, ia.align, &ia.alt), (ib.wrap, ib.align, &ib.alt));
        }
        assert_eq!(back.paragraphs.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), orig.paragraphs.iter().map(|p| p.text.as_str()).collect::<Vec<_>>());
    }

    #[test]
    fn images() {
        let d = with_images();
        let bytes = write(&d);
        let z = ZipReader::new(&bytes).unwrap();
        // The PNG is stored once.
        assert_eq!(z.names().filter(|n| n.starts_with("word/media/")).count(), 2);
        check_images(&read(&bytes).unwrap(), &d);
        // APEX_DUMP=dir APEX_PNG=file.png: files to check in other programs.
        if let (Some(dir), Some(png)) = (std::env::var_os("APEX_DUMP"), std::env::var_os("APEX_PNG")) {
            let png: Arc<[u8]> = Arc::from(std::fs::read(png).unwrap());
            let mut d = with_images();
            for (pos, img) in d.images() {
                d.set_image(pos, apex_doc::Image { data: png.clone(), ..(*img).clone() });
            }
            d.paragraphs[1].text.insert_str(OBJECT.len_utf8(), &"Text beside a square picture. ".repeat(12));
            d.paragraphs[1].runs.push(apex_doc::Run { len: 30 * 12, attrs: Attrs::default() });
            let dir = std::path::Path::new(&dir);
            std::fs::write(dir.join("images.docx"), write(&d)).unwrap();
            std::fs::write(dir.join("images.odt"), crate::odt::write(&d)).unwrap();
        }
    }

    /// A landscape document with two sections and every kind of header
    /// and footer (formatted text, a field and a table), shared with the
    /// odt tests.
    pub(crate) fn with_headers() -> Document {
        use apex_doc::{Field, FieldKind, HfSlot, SectionStart, Table};
        let mut two = Paragraph::new("Page two");
        two.style.page_break_before = true;
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Body"), two, Paragraph::new("Second")]);
        d.page = PageSetup { margin_top: 90.0, margin_bottom: 80.0, margin_header: 30.0, margin_footer: 25.0, ..PageSetup::LETTER };
        d.page.set_landscape(true);
        let mut h = Paragraph::new("Report ");
        h.push("Q3", Attrs { bold: true, ..Default::default() });
        h.push("\t", Attrs::default());
        h.push(&OBJECT.to_string(), Attrs { field: Some(Arc::new(Field::new(FieldKind::Page))), ..Default::default() });
        h.style.align = Align::Center;
        let mut t = Table::new(1, 2, 400.0);
        t.rows[0][0].paras = vec![Paragraph::new("Left cell")];
        t.rows[0][1].paras = vec![Paragraph::new("Right cell")];
        let mut footer = t.into_paras(1);
        footer.push(Paragraph::new("end"));
        let set = |d: &mut Document, s: usize, slot: HfSlot, v: Vec<Paragraph>| d.set_hf_story(s, slot, Some(v), Selection::default());
        set(&mut d, 0, HfSlot::Header, vec![h]);
        set(&mut d, 0, HfSlot::Footer, footer);
        set(&mut d, 0, HfSlot::FirstHeader, vec![Paragraph::new("Cover")]);
        set(&mut d, 0, HfSlot::FirstFooter, vec![Paragraph::new("")]);
        let page_field = |text: &str| {
            let mut p = Paragraph::new(text);
            p.push(&OBJECT.to_string(), Attrs { field: Some(Arc::new(Field::new(FieldKind::Page))), ..Default::default() });
            p
        };
        set(&mut d, 0, HfSlot::EvenHeader, vec![page_field("Even ")]);
        set(&mut d, 0, HfSlot::EvenFooter, vec![Paragraph::new("Even foot")]);
        d.set_hf_options(0, true, true, Selection::default());
        d.insert_break(Selection::caret(Pos { para: 2, offset: 0 }), apex_doc::BreakKind::Section(SectionStart::NextPage));
        set(&mut d, 1, HfSlot::Header, vec![page_field("Chapter 2 ")]);
        // Roman numbers, then from 1 again.
        d.set_page_numbering(0, apex_doc::PageNumbering { format: apex_doc::NoteFormat::LowerRoman, start: None }, Selection::default());
        d.set_page_numbering(1, apex_doc::PageNumbering { format: apex_doc::NoteFormat::Arabic, start: Some(1) }, Selection::default());
        d
    }

    /// Text of a header or footer (cells separated by '|').
    fn story_text(st: Option<&apex_doc::Story>) -> String {
        st.map(|ps| ps.iter().map(|p| p.text.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("|")).unwrap_or_default()
    }

    pub(crate) fn check_headers(back: &Document, orig: &Document) {
        use apex_doc::HfSlot;
        let (a, b) = (&back.page, &orig.page);
        for (x, y) in [(a.width, b.width), (a.height, b.height), (a.margin_top, b.margin_top), (a.margin_bottom, b.margin_bottom), (a.margin_header, b.margin_header), (a.margin_footer, b.margin_footer)] {
            assert!((x - y).abs() < 0.1, "{a:?} vs {b:?}");
        }
        assert_eq!(back.odd_even, orig.odd_even);
        assert_eq!(back.sections().len(), orig.sections().len());
        for s in 0..orig.sections().len() {
            assert_eq!(back.section_headers(s).different_first, orig.section_headers(s).different_first, "section {s}");
            assert_eq!(back.section_headers(s).numbering, orig.section_headers(s).numbering, "section {s}");
            let used = |slot: HfSlot| match slot.kind() {
                1 => orig.section_headers(s).different_first,
                2 => orig.odd_even,
                _ => true,
            };
            for slot in HfSlot::ALL.into_iter().filter(|&x| used(x)) {
                assert_eq!(story_text(back.hf_story(s, slot)), story_text(orig.hf_story(s, slot)), "section {s} {slot:?}");
            }
        }
        if !orig.has_headers() {
            return;
        }
        let h = &back.hf_story(0, HfSlot::Header).unwrap()[0];
        assert_eq!(h.style.align, Align::Center);
        assert!(h.spans().any(|(s, e, a)| a.bold && &h.text[s..e] == "Q3"));
        assert!(h.spans().any(|(_, _, a)| a.field.as_ref().is_some_and(|f| f.kind == apex_doc::FieldKind::Page)));
        let f = back.hf_story(0, HfSlot::Footer).unwrap();
        assert!(f[0].cell.is_some() && f[1].cell.is_some() && f.last().unwrap().cell.is_none());
    }

    #[test]
    fn headers() {
        let d = with_headers();
        let bytes = write(&d);
        if let Some(dir) = std::env::var_os("APEX_DUMP") {
            std::fs::write(std::path::Path::new(&dir).join("headers.docx"), &bytes).unwrap();
        }
        let back = read(&bytes).unwrap();
        check_headers(&back, &d);
        // Without headers, no parts are written.
        let plain = sample();
        let z = ZipReader::new(&write(&plain)).map(|z| z.names().any(|n| n.contains("header"))).unwrap();
        assert!(!z);
    }

    /// Links of every kind and bookmarks: ranges, points, hidden, one
    /// spanning paragraphs and one ending inside a link.
    pub(crate) fn linked() -> Document {
        use apex_doc::{Pos, Selection};
        let link = |l: &str| Attrs { link: Some(l.into()), ..Default::default() };
        let mut p = Paragraph::new("");
        p.push("Ir a ", Attrs::default());
        p.push("sección", link("#Seccion_2"));
        p.push(" o ", Attrs::default());
        p.push("web", Attrs { bold: true, link_tip: Some("Ejemplo & más".into()), ..link("https://example.com/a?b=1&c=2") });
        p.push(" y ", Attrs::default());
        p.push("correo", link("mailto:ana@example.com"));
        let h = Paragraph::new("Sección 2").with_style(ParaStyle { kind: ParaKind::Heading(1), ..Default::default() });
        let mut d = Document::from_paragraphs(vec![p, h, Paragraph::new("fin del  texto")]);
        let sel = |a: (usize, usize), b: (usize, usize)| Selection { anchor: Pos::new(a.0, a.1), cursor: Pos::new(b.0, b.1) };
        d.add_bookmark("Seccion_2", sel((1, 0), (1, 10)));
        d.add_bookmark("Punto", sel((2, 4), (2, 4)));
        d.add_bookmark("_GoBack", sel((2, 0), (2, 0)));
        d.add_bookmark("Cruza", sel((0, 2), (2, 3)));
        d.add_bookmark("EnEnlace", sel((0, 17), (0, 22)));
        d.add_bookmark("Final", sel((2, 15), (2, 15)));
        d
    }

    pub(crate) fn check_links(back: &Document, orig: &Document) {
        check(back, orig);
        assert_eq!(back.bookmarks(), orig.bookmarks());
        assert_eq!(orig.bookmarks().len(), 6);
    }

    #[test]
    fn links_and_bookmarks() {
        let d = linked();
        let bytes = write(&d);
        let back = read(&bytes).unwrap();
        check_links(&back, &d);
        let z = ZipReader::new(&bytes).unwrap();
        let xml = z.read_string("word/document.xml").unwrap();
        assert!(xml.contains(r#"<w:hyperlink w:anchor="Seccion_2""#));
        assert!(xml.contains(r#"w:tooltip="Ejemplo &amp; más""#));
        let rels = z.read_string("word/_rels/document.xml.rels").unwrap();
        assert!(rels.contains(r#"Target="https://example.com/a?b=1&amp;c=2" TargetMode="External""#));
        assert!(!rels.contains("Seccion_2"));
    }

    #[test]
    fn editable_ranges() {
        let body = r#"<w:p><w:r><w:t xml:space="preserve">Nombre: </w:t></w:r><w:permStart w:id="7" w:edGrp="everyone"/><w:r><w:t>Ana</w:t></w:r><w:permEnd w:id="7"/></w:p><w:tbl><w:tr><w:tc><w:p><w:permStart w:id="9" w:ed="ana@x.org"/><w:r><w:t>celda</w:t></w:r><w:permEnd w:id="9"/></w:p></w:tc></w:tr></w:tbl><w:p/>"#;
        let xml = format!(r#"{HEADER}<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#);
        let mut z = ZipWriter::new();
        z.add("word/document.xml", xml.as_bytes(), true);
        let d = read(&z.finish()).unwrap();
        let want = vec![(Pos::new(0, 8), Pos::new(0, 11)), (Pos::new(1, 0), Pos::new(1, 5))];
        assert_eq!(d.editable_ranges(), want);
        assert!(d.bookmarks().is_empty());
        let bytes = write(&d);
        let xml = ZipReader::new(&bytes).unwrap().read_string("word/document.xml").unwrap();
        assert!(xml.contains(r#"w:edGrp="everyone""#) && xml.contains(r#"w:ed="ana@x.org""#) && !xml.contains("bookmarkStart"));
        assert_eq!(read(&bytes).unwrap().editable_ranges(), want);
    }

    #[test]
    fn hyperlink_fields() {
        let body = r#"<w:p><w:r><w:t xml:space="preserve">a </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> HYPERLINK "http://x.org/" \o "Tip" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>uno</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> b </w:t></w:r><w:fldSimple w:instr=" HYPERLINK \l &quot;Marca&quot; "><w:r><w:t>dos</w:t></w:r></w:fldSimple><w:fldSimple w:instr="PAGE"><w:r><w:t>3</w:t></w:r></w:fldSimple></w:p><w:bookmarkStart w:id="0" w:name="Marca"/><w:p><w:r><w:t>x</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#;
        let xml = format!(r#"{HEADER}<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#);
        let mut z = ZipWriter::new();
        z.add("word/document.xml", xml.as_bytes(), true);
        let d = read(&z.finish()).unwrap();
        let p = &d.paragraphs[0];
        // PAGE stays a field showing "3".
        assert_eq!(p.text, format!("a uno b dos{OBJECT}"));
        assert_eq!(d.fields()[0].1.cached, "3");
        let links: Vec<_> = p.spans().map(|(s, e, a)| (&p.text[s..e], a.link.as_deref(), a.link_tip.as_deref())).collect();
        assert_eq!(links, [("a ", None, None), ("uno", Some("http://x.org/"), Some("Tip")), (" b ", None, None), ("dos", Some("#Marca"), None), ("\u{FFFC}", None, None)]);
        let b = d.bookmark("Marca").unwrap();
        assert_eq!((b.start, b.end), (apex_doc::Pos::new(1, 0), apex_doc::Pos::new(1, 1)));
    }
}

/// A settings.xml on/off element: present and not turned off.
pub(crate) fn on_flag(settings: &str, name: &str) -> bool {
    let mut r = Reader::new(settings);
    loop {
        match r.next() {
            Ev::Eof => return false,
            Ev::Start(t) | Ev::Empty(t) if t.local() == name => return t.attr("w:val").is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off")),
            _ => {}
        }
    }
}

/// The `a:graphicData` of a chart.
pub(crate) const CHART_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
