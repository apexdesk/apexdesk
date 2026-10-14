//! Word's object model over a document: Application, Documents, the
//! Document, Selection and Range, Find and Replace, Font and
//! ParagraphFormat, Paragraphs, Words, Sentences and Characters, Tables,
//! Bookmarks, Comments, Hyperlinks, Styles, Sections with their headers
//! and footers, document properties and revisions.
//!
//! Positions are characters of the main text, each paragraph mark one
//! more, as in Word: `Range(0, 5)` is the first five characters. Ranges
//! follow the edits made through other ranges (text inserted before one
//! moves it). A line is taken as a paragraph (there is no page layout
//! while a macro runs).

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use apex_doc::{Document, LINE_BREAK, OBJECT, Paragraph, Pos, Selection};

use crate::error::{Error, R};
use crate::object::Obj;
use crate::value::Value;

mod controls;
mod doc;
mod find;
mod mailmerge;
mod objects;
mod parts;
pub(crate) use parts::safe_address;
mod range;
mod table_format;

pub(crate) use find::FindState;

/// A Range: its start and end, and its Find.
#[derive(Debug, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub find: Option<Box<FindState>>,
}

pub type SpanRef = Rc<RefCell<Span>>;

/// What a Font, ParagraphFormat, Find or ListFormat belongs to.
#[derive(Clone, Debug)]
pub enum Tg {
    Range(SpanRef),
    Sel,
}

/// A unit of text (WdUnits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Char,
    Word,
    Sentence,
    Para,
    Story,
    Section,
    Cell,
    Row,
    Table,
}

impl Unit {
    pub fn of(n: i64) -> R<Unit> {
        Ok(match n {
            1 => Unit::Char,
            2 => Unit::Word,
            3 => Unit::Sentence,
            // Lines are paragraphs: there is no page layout here.
            4 | 5 => Unit::Para,
            6 => Unit::Story,
            8 => Unit::Section,
            10 => Unit::Row,
            12 => Unit::Cell,
            15 => Unit::Table,
            7 | 9 | 11 => return Err(Error::unsupported("moving by screens, columns or windows")),
            _ => return Err(Error::invalid_call()),
        })
    }
}

/// An object of Word's model. Parts of the text are Ranges, which
/// follow the edits.
#[derive(Clone, Debug)]
pub enum WObj {
    Doc,
    Documents,
    Sel,
    Range(SpanRef),
    /// A Paragraph: its range.
    Para(SpanRef),
    /// Characters, Words, Sentences or Paragraphs of a part of the text.
    Units(SpanRef, Unit),
    Font(Tg),
    ParaFormat(Tg),
    Find(Tg),
    Replacement(Tg),
    /// The Font of a Find (false) or of its Replacement (true).
    FindFont(Tg, bool),
    ListFormat(Tg),
    /// Formatted text (Range.FormattedText of a copy).
    Fragment(Rc<Vec<Paragraph>>),
    /// The tables of a part of the text.
    Tables(SpanRef),
    /// A table: its range and how deep it is nested.
    Table(SpanRef, usize),
    /// Rows (true) or Columns of a table.
    Lines(SpanRef, usize, bool),
    /// A row (true) or column of a table (0-based).
    Line(SpanRef, usize, bool, usize),
    /// A cell of a table (row and column, 0-based).
    Cell(SpanRef, usize, usize, usize),
    /// The cells of a row (Some(true, n)), a column (Some(false, n)) or a
    /// whole table.
    Cells(SpanRef, usize, Option<(bool, usize)>),
    Bookmarks,
    Bookmark(Rc<str>),
    Comments,
    Comment(u32),
    /// The text of a comment.
    CommentText(u32),
    Hyperlinks(SpanRef),
    /// A hyperlink: the range of its text.
    Hyperlink(SpanRef),
    InlineShapes(SpanRef),
    /// A picture: the range of its character.
    InlineShape(SpanRef),
    Styles,
    /// A style by its name.
    Style(Rc<str>),
    /// The Font of a style.
    StyleFont(Rc<str>),
    Sections,
    Section(usize),
    /// A section's Headers (true) or Footers.
    HeadersFooters(usize, bool),
    HeaderFooter(usize, apex_doc::HfSlot),
    /// The text of a header or footer.
    HfRange(usize, apex_doc::HfSlot),
    PageSetup,
    /// BuiltInDocumentProperties (true) or CustomDocumentProperties.
    Props(bool),
    Prop(bool, Rc<str>),
    Revisions(SpanRef),
    Fields(SpanRef),
    /// A field Fields.Add made: its range.
    Field(SpanRef),
    /// The Fields, PageNumbers and ParagraphFormat of a header or footer.
    HfFields(usize, apex_doc::HfSlot),
    HfNumbers(usize, apex_doc::HfSlot),
    HfParaFormat(usize, apex_doc::HfSlot),
    /// TabStops of the paragraphs of a part of the text, and one of them (0-based).
    Tabs(Tg),
    Tab(Tg, usize),
    /// Document.Variables and one by name.
    Vars,
    Var(Rc<str>),
    /// Document.Shapes, a shape (its number in the text's order), its
    /// TextFrame and the TextRange of that.
    Shapes,
    DocShape(usize),
    ShapeFrame(usize),
    ShapeRange(usize),
    /// Document.TablesOfContents and one of them (0-based).
    Tocs,
    Toc(usize),
    /// The Borders of a table.
    TableBorders(SpanRef, usize),
    /// The Shading of a table's cell (row, column).
    CellShading(SpanRef, usize, usize, usize),
    /// The content controls of a part of the text, or the document's with
    /// a title (false) or tag (true).
    Controls(SpanRef, Option<(bool, Rc<str>)>),
    /// A content control by its id.
    Control(u32),
    /// The entries of a list control, and one of them (0-based).
    Entries(u32),
    Entry(u32, usize),
    /// Document.MailMerge, its DataSource and the merge fields.
    MailMerge,
    MergeSource,
    /// DataSource.FieldNames, or DataFields (true: the active record's).
    MergeNames(bool),
    MergeName(bool, usize),
    MergeFields,
}

pub fn wo(o: WObj) -> Value {
    Value::Object(Obj::Word(Box::new(o)))
}

/// `o` of document `b`, one the macro opened.
pub(crate) fn wo_in(b: usize, o: WObj) -> Value {
    Value::Object(Obj::In(b, Box::new(Obj::Word(Box::new(o)))))
}

impl WObj {
    pub fn type_name(&self) -> &'static str {
        match self {
            WObj::Doc => "Document",
            WObj::Documents => "Documents",
            WObj::Sel => "Selection",
            WObj::Range(_) | WObj::Fragment(_) | WObj::CommentText(_) | WObj::HfRange(..) => "Range",
            WObj::Para(_) => "Paragraph",
            WObj::Units(_, Unit::Char) => "Characters",
            WObj::Units(_, Unit::Word) => "Words",
            WObj::Units(_, Unit::Sentence) => "Sentences",
            WObj::Units(..) => "Paragraphs",
            WObj::Font(_) | WObj::FindFont(..) | WObj::StyleFont(_) => "Font",
            WObj::ParaFormat(_) => "ParagraphFormat",
            WObj::Find(_) => "Find",
            WObj::Replacement(_) => "Replacement",
            WObj::ListFormat(_) => "ListFormat",
            WObj::Tables(_) => "Tables",
            WObj::Table(..) => "Table",
            WObj::Lines(_, _, true) => "Rows",
            WObj::Lines(_, _, false) => "Columns",
            WObj::Line(_, _, true, _) => "Row",
            WObj::Line(_, _, false, _) => "Column",
            WObj::Cells(..) => "Cells",
            WObj::Cell(..) => "Cell",
            WObj::Bookmarks => "Bookmarks",
            WObj::Bookmark(_) => "Bookmark",
            WObj::Comments => "Comments",
            WObj::Comment(_) => "Comment",
            WObj::Hyperlinks(_) => "Hyperlinks",
            WObj::Hyperlink(_) => "Hyperlink",
            WObj::InlineShapes(_) => "InlineShapes",
            WObj::InlineShape(_) => "InlineShape",
            WObj::Styles => "Styles",
            WObj::Style(_) => "Style",
            WObj::Sections => "Sections",
            WObj::Section(_) => "Section",
            WObj::HeadersFooters(..) => "HeadersFooters",
            WObj::HeaderFooter(..) => "HeaderFooter",
            WObj::PageSetup => "PageSetup",
            WObj::Props(_) => "DocumentProperties",
            WObj::Prop(..) => "DocumentProperty",
            WObj::Revisions(_) => "Revisions",
            WObj::Fields(_) | WObj::HfFields(..) => "Fields",
            WObj::Field(_) => "Field",
            WObj::HfNumbers(..) => "PageNumbers",
            WObj::HfParaFormat(..) => "ParagraphFormat",
            WObj::Tabs(_) => "TabStops",
            WObj::Tab(..) => "TabStop",
            WObj::Vars => "Variables",
            WObj::Var(_) => "Variable",
            WObj::Shapes => "Shapes",
            WObj::DocShape(_) => "Shape",
            WObj::ShapeFrame(_) => "TextFrame",
            WObj::ShapeRange(_) => "Range",
            WObj::Tocs => "TablesOfContents",
            WObj::Toc(_) => "TableOfContents",
            WObj::TableBorders(..) => "Borders",
            WObj::CellShading(..) => "Shading",
            WObj::Controls(..) => "ContentControls",
            WObj::Control(_) => "ContentControl",
            WObj::Entries(_) => "ContentControlListEntries",
            WObj::Entry(..) => "ContentControlListEntry",
            WObj::MailMerge => "MailMerge",
            WObj::MergeSource => "MailMergeDataSource",
            WObj::MergeNames(false) => "MailMergeFieldNames",
            WObj::MergeNames(true) => "MailMergeDataFields",
            WObj::MergeName(false, _) => "MailMergeFieldName",
            WObj::MergeName(true, _) => "MailMergeDataField",
            WObj::MergeFields => "MailMergeFields",
        }
    }

    /// `Is`: the same object.
    pub fn same(&self, other: &WObj) -> bool {
        match (self, other) {
            (WObj::Range(a), WObj::Range(b)) | (WObj::Para(a), WObj::Para(b)) | (WObj::Hyperlink(a), WObj::Hyperlink(b)) | (WObj::InlineShape(a), WObj::InlineShape(b)) => Rc::ptr_eq(a, b),
            (WObj::Table(a, _), WObj::Table(b, _)) => Rc::ptr_eq(a, b),
            (WObj::Cell(t, _, r, c), WObj::Cell(u, _, s, d)) => Rc::ptr_eq(t, u) && (r, c) == (s, d),
            (WObj::Section(a), WObj::Section(b)) => a == b,
            (WObj::Comment(a), WObj::Comment(b)) | (WObj::Control(a), WObj::Control(b)) => a == b,
            (WObj::Entry(a, i), WObj::Entry(b, j)) => (a, i) == (b, j),
            (WObj::MergeNames(a), WObj::MergeNames(b)) => a == b,
            (WObj::MergeName(a, i), WObj::MergeName(b, j)) => (a, i) == (b, j),
            (WObj::Bookmark(a), WObj::Bookmark(b)) | (WObj::Style(a), WObj::Style(b)) => a.eq_ignore_ascii_case(b),
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b) && matches!(a, WObj::Doc | WObj::Documents | WObj::Sel | WObj::Vars | WObj::Shapes | WObj::Tocs | WObj::Bookmarks | WObj::Comments | WObj::Styles | WObj::Sections | WObj::PageSetup | WObj::MailMerge | WObj::MergeSource | WObj::MergeFields),
        }
    }
}

/// Most characters a document may grow to in a macro.
const MAX_STORY: usize = 50_000_000;
/// Most paragraphs a document may grow to in a macro (each one costs
/// far more than its characters).
const MAX_PARAS: usize = 1_000_000;
/// Undo steps the macro's copy of the document keeps.
const KEEP_UNDO: usize = 20;

/// The document of a WordState: the macro's own, or one it opened.
pub enum DocRef<'a> {
    Mut(&'a mut Document),
    Owned(Box<Document>),
}

impl std::ops::Deref for DocRef<'_> {
    type Target = Document;
    fn deref(&self) -> &Document {
        match self {
            DocRef::Mut(d) => d,
            DocRef::Owned(d) => d,
        }
    }
}

impl std::ops::DerefMut for DocRef<'_> {
    fn deref_mut(&mut self) -> &mut Document {
        match self {
            DocRef::Mut(d) => d,
            DocRef::Owned(d) => d,
        }
    }
}

/// The document a Word macro works on, and where its Selection and
/// Ranges are.
pub struct WordState<'a> {
    pub doc: DocRef<'a>,
    /// The Selection: where it started and the end that moves.
    pub anchor: usize,
    pub active: usize,
    /// Selection.Find.
    pub find: FindState,
    /// What Copy or Cut put aside.
    pub clip: Option<Rc<Vec<Paragraph>>>,
    /// Formatting set on the Selection with nothing selected: what
    /// TypeText there types with.
    pub typing: Option<(usize, apex_doc::Attrs)>,
    /// Document.MailMerge.
    pub(crate) mail: mailmerge::MailState,
    /// Where each paragraph starts, and the length of the text last.
    starts: Vec<usize>,
    key: Option<(u64, usize)>,
    ranges: Vec<Weak<RefCell<Span>>>,
    prune_at: usize,
}

/// Text for a macro: paragraph marks as `\r` (with `\a` after a cell's
/// last one), line breaks as Chr(11), pictures and other objects as Chr(1).
pub(crate) fn push_text(out: &mut String, t: &str) {
    for c in t.chars() {
        out.push(match c {
            LINE_BREAK => '\u{b}',
            OBJECT => '\u{1}',
            c => c,
        });
    }
}

/// Text a macro gives, for the document: `\r`, `\r\n`, `\n` and Chr(12)
/// start paragraphs, Chr(11) is a line break; other control characters
/// (and the object character) are left out.
pub fn clean_input(s: &str) -> String {
    let s = s.replace("\r\n", "\n");
    s.chars()
        .filter_map(|c| match c {
            '\r' | '\u{c}' | '\u{2029}' => Some('\n'),
            '\u{b}' => Some(LINE_BREAK),
            '\n' | '\t' => Some(c),
            OBJECT => None,
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

fn protected() -> Error {
    Error::new(4605, "This method or property is not available because the document is protected")
}

impl<'a> WordState<'a> {
    pub fn new(doc: &'a mut Document, sel: Selection) -> WordState<'a> {
        Self::of(DocRef::Mut(doc), sel)
    }

    pub fn of(doc: DocRef<'a>, sel: Selection) -> WordState<'a> {
        let mut w = WordState { doc, anchor: 0, active: 0, find: FindState::default(), clip: None, typing: None, mail: Default::default(), starts: Vec::new(), key: None, ranges: Vec::new(), prune_at: 1024 };
        w.anchor = w.char_of(sel.anchor);
        w.active = w.char_of(sel.cursor);
        w
    }

    /// The Selection as the document has it.
    pub fn selection(&mut self) -> Selection {
        Selection { anchor: self.pos_of(self.anchor), cursor: self.pos_of(self.active) }
    }

    pub fn sel(&self) -> (usize, usize) {
        (self.anchor.min(self.active), self.anchor.max(self.active))
    }

    fn sync(&mut self) {
        let key = (self.doc.revision, self.doc.paragraphs.len());
        if self.key == Some(key) && self.starts.len() == self.doc.paragraphs.len() + 1 {
            return;
        }
        self.starts.clear();
        let mut c = 0;
        for p in &self.doc.paragraphs {
            self.starts.push(c);
            c += p.text.chars().count() + 1;
        }
        self.starts.push(c);
        self.key = Some(key);
    }

    /// The document changed.
    pub fn dirty(&mut self) {
        self.key = None;
    }

    /// Positions in the main text (the last one is after the last mark).
    pub fn story_len(&mut self) -> usize {
        self.sync();
        self.starts[self.starts.len() - 1]
    }

    pub fn para_start(&mut self, p: usize) -> usize {
        self.sync();
        self.starts[p.min(self.starts.len() - 1)]
    }

    /// The paragraph holding position `c` (the last one for the end).
    pub fn para_of(&mut self, c: usize) -> usize {
        self.sync();
        let n = self.doc.paragraphs.len();
        self.starts.partition_point(|&s| s <= c).saturating_sub(1).min(n - 1)
    }

    /// The document position of `c`: past the last paragraph's text, its
    /// end (the last mark can't be gone past).
    pub fn pos_of(&mut self, c: usize) -> Pos {
        let len = self.story_len();
        if c + 1 >= len {
            return self.doc.end();
        }
        let p = self.para_of(c);
        let k = c - self.starts[p];
        let t = &self.doc.paragraphs[p].text;
        Pos::new(p, t.char_indices().nth(k).map_or(t.len(), |(i, _)| i))
    }

    pub fn char_of(&mut self, p: Pos) -> usize {
        self.sync();
        let p = self.doc.clamp(p);
        self.starts[p.para] + self.doc.paragraphs[p.para].text[..p.offset].chars().count()
    }

    /// Whether paragraph `i` is the last of a table cell.
    fn cell_end(&self, i: usize) -> bool {
        self.doc.container(i).is_some() && self.doc.container(i + 1) != self.doc.container(i)
    }

    /// The text of `s..e`.
    pub fn text(&mut self, s: usize, e: usize) -> R<String> {
        let len = self.story_len();
        let (s, e) = (s.min(len), e.min(len));
        if s >= e {
            return Ok(String::new());
        }
        crate::value::string_room(e - s)?;
        let (a, b) = (self.pos_of(s), self.pos_of(e));
        let mut out = String::new();
        for i in a.para..=b.para {
            let t = &self.doc.paragraphs[i].text;
            let from = if i == a.para { a.offset } else { 0 };
            let to = if i == b.para { b.offset } else { t.len() };
            push_text(&mut out, &t[from..to]);
            if i < b.para {
                // A manual page break (paragraph break-before) is Chr(12) in
                // place of the paragraph mark before it: positions stay.
                let brk = self.doc.paragraphs.get(i + 1).is_some_and(|n| n.style.page_break_before) && self.doc.container(i).is_none();
                out.push(if brk { '\u{c}' } else { '\r' });
                if self.cell_end(i) {
                    out.push('\u{7}');
                }
            }
        }
        if e == len {
            out.push('\r');
        }
        Ok(out)
    }

    /// The selection of the document for `s..e`.
    pub fn doc_sel(&mut self, s: usize, e: usize) -> Selection {
        Selection { anchor: self.pos_of(s), cursor: self.pos_of(e) }
    }

    /// A Range over `s..e` that follows the edits.
    pub fn new_span(&mut self, s: usize, e: usize) -> SpanRef {
        let len = self.story_len();
        let (s, e) = (s.min(len), e.min(len));
        let r = Rc::new(RefCell::new(Span { start: s.min(e), end: e.max(s), find: None }));
        if self.ranges.len() >= self.prune_at {
            self.ranges.retain(|w| w.strong_count() > 0);
            self.prune_at = (self.ranges.len() * 2).max(1024);
        }
        self.ranges.push(Rc::downgrade(&r));
        r
    }

    pub fn writable(&self) -> R<()> {
        if self.doc.edit_limit().is_some() { Err(protected()) } else { Ok(()) }
    }

    /// Changes the text at `s..e` with `f` (given the document's
    /// selection, it returns where the change ends): the start and end of
    /// what is there now. Ranges after it move with the text.
    pub fn mutate(&mut self, s: usize, e: usize, f: impl FnOnce(&mut Document, Selection) -> Option<Pos>) -> R<(usize, usize)> {
        self.writable()?;
        let len = self.story_len();
        let sel = self.doc_sel(s, e.max(s));
        // Where the change really is (the last mark stays).
        let (at, to) = (self.char_of(sel.anchor), self.char_of(sel.cursor));
        let Some(end) = f(&mut *self.doc, sel) else { return Err(Error::new(4605, "This command isn't available here (in a table?)")) };
        self.doc.trim_undo(KEEP_UNDO);
        // Each call is a step of its own for Document.Undo.
        self.doc.seal_undo();
        self.dirty();
        let new_len = self.story_len();
        let end = self.char_of(end).max(at);
        self.shift(at, to, end, new_len as isize - len as isize);
        if new_len > MAX_STORY || self.doc.paragraphs.len() > MAX_PARAS {
            return Err(Error::new(7, "Out of memory: the document is too long"));
        }
        Ok((at, end))
    }

    /// Changes the part `s..e` of the text with `f`, given `sel` (a table
    /// edit: the whole table changes). Where `f` puts the caret.
    pub fn mutate_in(&mut self, s: usize, e: usize, sel: Selection, f: impl FnOnce(&mut Document, Selection) -> Option<Pos>) -> R<usize> {
        self.writable()?;
        let len = self.story_len();
        let Some(caret) = f(&mut *self.doc, sel) else { return Err(Error::new(4605, "This command isn't available here")) };
        self.doc.trim_undo(KEEP_UNDO);
        // Each call is a step of its own for Document.Undo.
        self.doc.seal_undo();
        self.dirty();
        let new_len = self.story_len();
        let delta = new_len as isize - len as isize;
        let e2 = (e as isize + delta).max(s as isize) as usize;
        self.shift(s, e, e2, delta);
        if new_len > MAX_STORY || self.doc.paragraphs.len() > MAX_PARAS {
            return Err(Error::new(7, "Out of memory: the document is too long"));
        }
        Ok(self.char_of(caret))
    }

    /// The table nested `depth` deep that holds paragraph `p`.
    pub fn table_at(&self, p: usize, depth: usize) -> Option<apex_doc::TableMap> {
        let level = |i: usize| {
            let mut c = self.doc.paragraphs.get(i)?.cell.as_ref()?;
            for _ in 0..depth {
                c = c.nested.as_deref()?;
            }
            Some(c.table)
        };
        let id = level(p)?;
        let mut a = p;
        while a > 0 && level(a - 1) == Some(id) {
            a -= 1;
        }
        apex_doc::TableMap::build_at(&self.doc.paragraphs, a, depth)
    }

    /// Replaces `s..e` with `text` (already cleaned); a cell's end mark and
    /// the last paragraph mark stay. The span of the new text.
    pub fn replace(&mut self, s: usize, e: usize, text: &str) -> R<(usize, usize)> {
        self.replace_as(s, e, text, None)
    }

    /// `replace`, the new text with `attrs` (else as typed there).
    pub fn replace_as(&mut self, s: usize, e: usize, text: &str, attrs: Option<apex_doc::Attrs>) -> R<(usize, usize)> {
        let len = self.story_len();
        if len.saturating_add(text.len()) > MAX_STORY {
            return Err(Error::new(7, "Out of memory: the document is too long"));
        }
        let mut e = self.keep_marks(s, e);
        let mut s = s;
        let mut attrs = attrs;
        // Text put in a content control stays in it (a placeholder goes
        // whole); a control whose contents are locked keeps them.
        let sel = self.doc_sel(s, e);
        // A caret at either end of one is beside it.
        let inside = |c: &apex_doc::ControlAt| !sel.is_empty() || (c.range.start < sel.anchor.offset && sel.anchor.offset < c.range.end);
        if let Some(at) = self.doc.control_of(sel).filter(|c| !c.control.is_form() && inside(c)) {
            if at.control.lock_content {
                return Err(Error::new(6124, "You are not allowed to edit this selection because it is protected"));
            }
            if at.control.placeholder || attrs.is_none() {
                let para = &self.doc.paragraphs[at.para];
                let mut a = attrs.unwrap_or_else(|| para.attrs_at(at.range.start + para.text[at.range.start..].chars().next().map_or(0, char::len_utf8)));
                a.control = Some(at.control.clone());
                a.filled_control();
                attrs = Some(a);
            }
            if at.control.placeholder {
                s = self.char_of(Pos::new(at.para, at.range.start));
                e = self.char_of(Pos::new(at.para, at.range.end));
            }
        } else if sel.is_empty() && attrs.is_none() && self.doc.control_at(sel.anchor).is_some() {
            let mut a = self.doc.typing_attrs(sel.anchor);
            a.control = None;
            attrs = Some(a);
        }
        self.mutate(s, e, |d, sel| {
            Some(if !text.is_empty() {
                d.insert_text(sel, text, attrs)
            } else if sel.is_empty() {
                sel.anchor
            } else {
                d.delete(sel)
            })
        })
    }

    /// `e`, not past the end mark of a cell `s..e` ends at.
    pub fn keep_marks(&mut self, s: usize, e: usize) -> usize {
        let len = self.story_len();
        if e <= s || e >= len {
            return e;
        }
        let b = self.pos_of(e);
        if b.offset == 0 && b.para > 0 && self.cell_end(b.para - 1) { e - 1 } else { e }
    }

    /// Positions after an edit that turned `s..e` into `s..e2` and the
    /// text `delta` characters longer.
    pub fn shift(&mut self, s: usize, e: usize, e2: usize, delta: isize) {
        let len = self.story_len();
        let map = |p: usize| -> usize {
            let q = if p <= s {
                p
            } else if p >= e {
                (p as isize + delta).max(s as isize) as usize
            } else {
                p.min(e2)
            };
            q.min(len)
        };
        self.ranges.retain(|w| w.strong_count() > 0);
        for w in &self.ranges {
            if let Some(r) = w.upgrade() {
                let mut r = r.borrow_mut();
                r.start = map(r.start);
                r.end = map(r.end).max(r.start);
                if let Some(f) = r.find.as_deref_mut() {
                    f.shift(&map);
                }
            }
        }
        self.anchor = map(self.anchor);
        self.active = map(self.active);
        self.find.shift(&map);
    }

    /// Something other than the text changed (properties, page setup).
    pub fn touched(&mut self) {
        self.doc.revision += 1;
        self.doc.modified = true;
        self.dirty();
    }

    // --- units ---------------------------------------------------------------

    fn chars(&self, p: usize) -> Vec<char> {
        self.doc.paragraphs[p].text.chars().collect()
    }

    /// Where the words or sentences of paragraph `p` start, and its end
    /// (after its mark).
    fn bounds(&mut self, p: usize, unit: Unit) -> Vec<usize> {
        let base = self.para_start(p);
        let cs = self.chars(p);
        let rel = match unit {
            Unit::Word => word_bounds(&cs),
            Unit::Sentence => sentence_bounds(&cs),
            _ => vec![0, cs.len() + 1],
        };
        rel.into_iter().map(|x| base + x).collect()
    }

    /// The paragraphs of the table, row or cell holding paragraph `p`.
    fn cell_paras(&self, p: usize, unit: Unit) -> R<(usize, usize)> {
        let not = || Error::new(4605, "This method or property is not available because the object refers to a place that isn't in a table");
        let m = self.doc.table_map(p).ok_or_else(not)?;
        if unit == Unit::Table {
            return Ok((m.start, m.end));
        }
        let k = m.cells.iter().position(|&(a, b)| a <= p && p < b).ok_or_else(not)?;
        if unit == Unit::Cell {
            return Ok(m.cells[k]);
        }
        let row = k / m.cols.max(1);
        let cells = &m.cells[row * m.cols..(row + 1) * m.cols];
        let a = cells.iter().map(|c| c.0).min().unwrap_or(p);
        let b = cells.iter().map(|c| c.1).max().unwrap_or(p + 1);
        Ok((a, b))
    }

    /// The unit holding position `c`.
    pub fn unit_at(&mut self, c: usize, unit: Unit) -> R<(usize, usize)> {
        let len = self.story_len();
        let c = c.min(len.saturating_sub(1));
        Ok(match unit {
            Unit::Char => (c, c + 1),
            Unit::Story => (0, len),
            Unit::Word | Unit::Sentence | Unit::Para => {
                let p = self.para_of(c);
                let b = self.bounds(p, unit);
                let i = b.partition_point(|&x| x <= c).max(1);
                (b[i - 1], b[i.min(b.len() - 1)])
            }
            Unit::Section => {
                let p = self.para_of(c);
                let secs = self.doc.sections();
                let s = secs.iter().find(|s| s.first <= p && p < s.end).map_or((0, self.doc.paragraphs.len()), |s| (s.first, s.end));
                (self.para_start(s.0), self.para_start(s.1))
            }
            Unit::Cell | Unit::Row | Unit::Table => {
                let p = self.para_of(c);
                let (a, b) = self.cell_paras(p, unit)?;
                (self.para_start(a), self.para_start(b))
            }
        })
    }

    /// The units in `s..e` (the one holding `s` when it's empty), at most `max`.
    pub fn units_in(&mut self, s: usize, e: usize, unit: Unit, max: usize) -> R<Vec<(usize, usize)>> {
        let len = self.story_len();
        let (s, e) = (s.min(len), e.min(len));
        if s >= e {
            return Ok(vec![self.unit_at(s, unit)?]);
        }
        let mut out = Vec::new();
        match unit {
            Unit::Char => out.extend((s..e.min(s.saturating_add(max))).map(|c| (c, c + 1))),
            Unit::Story => out.push((0, len)),
            Unit::Word | Unit::Sentence | Unit::Para => {
                for p in self.para_of(s)..=self.para_of(e - 1) {
                    let b = self.bounds(p, unit);
                    for w in b.windows(2) {
                        if w[1] > s && w[0] < e {
                            if out.len() >= max {
                                return Ok(out);
                            }
                            out.push((w[0], w[1]));
                        }
                    }
                }
            }
            Unit::Section => {
                let (pa, pb) = (self.para_of(s), self.para_of(e - 1));
                let secs: Vec<(usize, usize)> = self.doc.sections().iter().map(|x| (x.first, x.end)).collect();
                for (a, b) in secs {
                    if b > pa && a <= pb {
                        out.push((self.para_start(a), self.para_start(b)));
                    }
                }
            }
            Unit::Cell | Unit::Row | Unit::Table => return Err(Error::invalid_call()),
        }
        Ok(out)
    }

    /// How many units `s..e` has.
    pub fn count_in(&mut self, s: usize, e: usize, unit: Unit) -> R<usize> {
        let len = self.story_len();
        let (s, e) = (s.min(len), e.min(len));
        if s >= e {
            return Ok(1);
        }
        Ok(match unit {
            Unit::Char => e - s,
            Unit::Para => self.para_of(e - 1) + 1 - self.para_of(s),
            Unit::Word | Unit::Sentence => {
                let mut n = 0;
                for p in self.para_of(s)..=self.para_of(e - 1) {
                    n += self.bounds(p, unit).windows(2).filter(|w| w[1] > s && w[0] < e).count();
                }
                n
            }
            _ => self.units_in(s, e, unit, usize::MAX)?.len(),
        })
    }

    /// The next (or previous) unit boundary from `c`.
    pub fn step(&mut self, c: usize, unit: Unit, forward: bool) -> R<Option<usize>> {
        let len = self.story_len();
        Ok(match (unit, forward) {
            (Unit::Char, true) => (c < len).then_some(c + 1),
            (Unit::Char, false) => c.checked_sub(1),
            (Unit::Story, true) => (c < len).then_some(len),
            (Unit::Story, false) => (c > 0).then_some(0),
            (Unit::Word | Unit::Sentence | Unit::Para, true) => {
                if c >= len {
                    None
                } else {
                    let p = self.para_of(c);
                    self.bounds(p, unit).into_iter().find(|&x| x > c)
                }
            }
            (Unit::Word | Unit::Sentence | Unit::Para, false) => {
                if c == 0 {
                    None
                } else {
                    let p = self.para_of(c - 1);
                    self.bounds(p, unit).into_iter().rev().find(|&x| x < c)
                }
            }
            (Unit::Section | Unit::Cell | Unit::Row | Unit::Table, _) => {
                let (a, b) = self.unit_at(if forward { c } else { c.saturating_sub(1) }, unit)?;
                if forward { (b > c).then_some(b) } else { (a < c).then_some(a) }
            }
        })
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{3000}')
}

/// Where the words of a paragraph's characters start (Word's words: a run
/// of letters and digits or one other character, with the spaces after
/// it; the paragraph mark is one), relative, ending after the mark.
fn word_bounds(cs: &[char]) -> Vec<usize> {
    let n = cs.len();
    let mut b = vec![0];
    let mut i = 0;
    while i < n {
        if is_word_char(cs[i]) {
            while i < n && (is_word_char(cs[i]) || matches!(cs[i], '\'' | '\u{2019}') && i + 1 < n && is_word_char(cs[i + 1])) {
                i += 1;
            }
        } else if !is_space(cs[i]) {
            i += 1;
        }
        while i < n && is_space(cs[i]) {
            i += 1;
        }
        b.push(i);
    }
    b.push(n + 1);
    b
}

/// Where the sentences of a paragraph's characters start: after `.`, `!`
/// or `?` (and closing quotes) and the spaces after them; the last one
/// takes the paragraph mark.
fn sentence_bounds(cs: &[char]) -> Vec<usize> {
    let n = cs.len();
    let mut b = vec![0];
    let mut i = 0;
    while i < n {
        let c = cs[i];
        let ideo = matches!(c, '\u{3002}' | '\u{FF01}' | '\u{FF1F}');
        if matches!(c, '.' | '!' | '?') || ideo {
            let mut j = i + 1;
            while j < n && matches!(cs[j], '.' | '!' | '?' | '"' | '\'' | ')' | '\u{201D}' | '\u{2019}' | '\u{BB}') {
                j += 1;
            }
            if j < n && (ideo || is_space(cs[j]) || cs[j] == '\t') {
                while j < n && (is_space(cs[j]) || cs[j] == '\t') {
                    j += 1;
                }
                if j < n && j > *b.last().unwrap_or(&0) {
                    b.push(j);
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    b.push(n + 1);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn words_and_sentences() {
        assert_eq!(word_bounds(&chars("Hello, world  it's")), [0, 5, 7, 14, 18, 19]);
        assert_eq!(word_bounds(&chars("")), [0, 1]);
        assert_eq!(word_bounds(&chars("  a")), [0, 2, 3, 4]);
        assert_eq!(sentence_bounds(&chars("One. Two!  Three")), [0, 5, 11, 17]);
        assert_eq!(sentence_bounds(&chars("3.5 is a number.")), [0, 17]);
    }

    #[test]
    fn input_text() {
        assert_eq!(clean_input("a\r\nb\rc\u{b}d\u{7}\u{1}e\u{fffc}"), "a\nb\nc\u{2028}de");
    }
}
