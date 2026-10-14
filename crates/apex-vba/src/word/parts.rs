//! Parts of a Word document: Tables with their Rows, Columns and Cells,
//! Bookmarks, Comments, Hyperlinks, InlineShapes, Styles, Sections with
//! their headers and footers, PageSetup, document properties, Revisions
//! and Fields.

use std::rc::Rc;
use std::sync::Arc;

use apex_doc::{CustomProp, Document, HfSlot, OBJECT, Paragraph, PropValue, Selection, TableMap};

use super::range::{MAX_ITEMS, wbool};
use super::{SpanRef, Tg, WObj, WordState, clean_input, wo};
use crate::error::{Error, R};
use crate::excel::Args;
use crate::interp::Interp;
use crate::object::Obj;
use crate::value::Value;

fn missing() -> Error {
    Error::new(5941, "The requested member of the collection does not exist")
}

fn deleted() -> Error {
    Error::new(5825, "Object has been deleted")
}

/// Most comments a document may have after a macro adds some.
const MAX_COMMENTS: usize = 100_000;
/// Most cells a table a macro adds may have.
const MAX_CELLS: usize = 100_000;

/// A 1-based index a macro gives.
fn index(n: i64, count: usize) -> R<usize> {
    if n >= 1 && (n as u64) <= count as u64 { Ok(n as usize - 1) } else { Err(missing()) }
}

/// Link addresses macros may put in a document or workbook.
pub(crate) fn safe_address(a: &str) -> bool {
    let l = a.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:", "ftp://", "#"].iter().any(|p| l.starts_with(p))
}

impl WordState<'_> {
    /// The tables of `s..e`: those in the cell it is in, else those of the
    /// text that aren't nested.
    fn tables_in(&mut self, s: usize, e: usize) -> Vec<TableMap> {
        let (pa, pb) = self.paras_of(s, e);
        let one_cell = self.doc.container(pa).is_some() && self.doc.container(pa) == self.doc.container(pb);
        let all = if one_cell {
            let depth = self.doc.table_depth(pa).unwrap_or(0);
            match self.table_at(pa, depth) {
                Some(m) => m.nested(&self.doc.paragraphs),
                None => Vec::new(),
            }
        } else {
            self.doc.tables()
        };
        all.into_iter().filter(|m| m.end > pa && m.start <= pb).collect()
    }

    /// Where the table of a Table object is now.
    pub(super) fn table(&mut self, r: &SpanRef, depth: usize) -> R<TableMap> {
        let start = r.borrow().start;
        let p = self.para_of(start);
        let m = self.table_at(p, depth).ok_or_else(deleted)?;
        if self.para_start(m.start) != start {
            return Err(deleted());
        }
        Ok(m)
    }

    /// The hyperlinks of `s..e`: where each one is, its address and tip.
    fn links_in(&mut self, s: usize, e: usize) -> Vec<(usize, usize, Arc<str>, Option<Arc<str>>)> {
        let (pa, pb) = self.paras_of(s, e);
        let mut out: Vec<(usize, usize, Arc<str>, Option<Arc<str>>)> = Vec::new();
        for i in pa..=pb {
            let base = self.para_start(i);
            let p = &self.doc.paragraphs[i];
            let mut chars = 0;
            let mut at = 0;
            for (rs, re, a) in p.spans() {
                chars += p.text[at..rs].chars().count();
                let (cs, n) = (base + chars, p.text[rs..re].chars().count());
                chars += n;
                at = re;
                let Some(link) = &a.link else { continue };
                match out.last_mut() {
                    Some(l) if l.1 == cs && l.2 == *link => l.1 = cs + n,
                    _ => out.push((cs, cs + n, link.clone(), a.link_tip.clone())),
                }
            }
        }
        out.into_iter().filter(|l| l.1 > s && l.0 < e.max(s + 1)).collect()
    }

    /// The pictures of `s..e` (their characters).
    fn shapes_in(&mut self, s: usize, e: usize) -> Vec<usize> {
        let ps: Vec<apex_doc::Pos> = self.doc.images().into_iter().map(|(p, _)| p).collect();
        ps.into_iter().map(|p| self.char_of(p)).filter(|&c| c >= s && c < e.max(s + 1)).collect()
    }

    /// The fields of `s..e` (their characters).
    fn fields_in(&mut self, s: usize, e: usize) -> usize {
        let (a, b) = (self.pos_of(s), self.pos_of(e));
        let mut n = 0;
        for i in a.para..=b.para {
            let p = &self.doc.paragraphs[i];
            let from = if i == a.para { a.offset } else { 0 };
            let to = if i == b.para { b.offset } else { p.text.len() };
            n += p.spans().filter(|(rs, re, at)| *rs >= from && *re <= to && at.field.is_some()).map(|(rs, re, _)| p.text[rs..re].chars().filter(|&c| c == OBJECT).count()).sum::<usize>();
        }
        n
    }

    /// A change of the document beside its text (headers, comments,
    /// settings), as one more step.
    pub fn side_edit<T>(&mut self, f: impl FnOnce(&mut Document, Selection) -> T) -> R<T> {
        self.writable()?;
        let sel = self.selection();
        let out = f(&mut *self.doc, sel);
        self.doc.trim_undo(super::KEEP_UNDO);
        // Each call is a step of its own for Document.Undo.
        self.doc.seal_undo();
        self.touched();
        Ok(out)
    }

    /// The text of a header or footer (`\r` after each paragraph).
    fn hf_text(&self, s: usize, slot: HfSlot) -> String {
        let mut out = String::new();
        if let Some(st) = self.doc.hf_story(s, slot) {
            for p in st.iter() {
                for c in p.text.chars() {
                    out.push(match c {
                        apex_doc::LINE_BREAK => '\u{b}',
                        OBJECT => '\u{1}',
                        c => c,
                    });
                }
                out.push('\r');
            }
        }
        if out.is_empty() {
            out.push('\r');
        }
        out
    }
}

/// Paragraphs of text a macro gives (already cleaned), styled like `like`.
fn paras_of_text(t: &str, like: Option<&Paragraph>) -> Vec<Paragraph> {
    t.split('\n')
        .map(|line| {
            let mut p = Paragraph::new(line);
            if let Some(l) = like {
                p.style = l.style;
                p.runs[0].attrs = l.attrs_at(0);
                p.runs[0].attrs.link = None;
                p.runs[0].attrs.image = None;
                p.runs[0].attrs.field = None;
            }
            p
        })
        .collect()
}

/// The built-in document properties there are, with their
/// WdBuiltInProperty numbers.
const BUILTIN_PROPS: [(i64, &str); 13] = [(1, "Title"), (2, "Subject"), (3, "Author"), (4, "Keywords"), (5, "Comments"), (11, "Creation Date"), (15, "Number of Words"), (16, "Number of Characters"), (18, "Category"), (20, "Manager"), (21, "Company"), (24, "Number of Paragraphs"), (30, "Number of Characters (with spaces)")];

impl Interp<'_> {
    pub(crate) fn wpart_call(&mut self, o: WObj, name: &str, a: &Args) -> R<Value> {
        match o {
            WObj::Tables(r) => self.tables_call(r, name, a),
            WObj::Table(r, d) => self.table_call(r, d, name, a),
            WObj::Lines(r, d, rows) => self.wlines_call(r, d, rows, name, a),
            WObj::Line(r, d, rows, i) => self.line_call(r, d, rows, i, name, a),
            WObj::Cells(r, d, which) => self.wcells_call(r, d, which, name, a),
            WObj::Cell(r, d, row, col) => self.cell_call(r, d, row, col, name),
            WObj::Bookmarks => self.bookmarks_call(name, a),
            WObj::Bookmark(n) => self.bookmark_call(n, name),
            WObj::Comments => self.comments_call(name, a),
            WObj::Comment(id) => self.comment_call(id, name),
            WObj::CommentText(id) => self.comment_text_call(id, name, a),
            WObj::Hyperlinks(r) => self.links_call(r, name, a),
            WObj::Hyperlink(r) => self.link_call(r, name),
            WObj::InlineShapes(r) => self.shapes_call(r, name, a),
            WObj::InlineShape(r) => self.shape_call(r, name),
            WObj::Styles => self.styles_call(name, a),
            WObj::Style(n) => self.style_call(n, name),
            WObj::StyleFont(n) => self.style_font_get(n, name),
            WObj::Field(r) => self.field_call(r, name),
            WObj::HfFields(i, slot) => self.hf_fields_call(i, slot, name, a),
            WObj::HfNumbers(i, slot) => self.hf_numbers_call(i, slot, name, a),
            WObj::HfParaFormat(i, slot) => self.hf_pformat_get(i, slot, name),
            WObj::Tabs(tg) => self.tabs_call(tg, name, a),
            WObj::Tab(tg, i) => self.tab_call(tg, i, name),
            WObj::Vars => self.vars_call(name, a),
            WObj::Var(n) => self.var_call(n, name),
            WObj::Shapes => self.doc_shapes_call(name, a),
            WObj::DocShape(k) => self.shape_get(k, name),
            WObj::ShapeFrame(k) => self.shape_text_call(k, false, name),
            WObj::ShapeRange(k) => self.shape_text_call(k, true, name),
            WObj::Tocs => self.tocs_call(name, a),
            WObj::Toc(k) => self.toc_call(k, name),
            WObj::TableBorders(r, d) => self.table_borders_call(r, d, name),
            WObj::CellShading(r, d, row, col) => self.cell_shading_get(r, d, row, col, name),
            WObj::Sections => self.sections_call(name, a),
            WObj::Section(i) => self.section_call(i, name, a),
            WObj::HeadersFooters(i, h) => self.hfs_call(i, h, name, a),
            WObj::HeaderFooter(i, slot) => self.hf_call(i, slot, name),
            WObj::HfRange(i, slot) => self.hf_range_call(i, slot, name, a),
            WObj::PageSetup => self.wpage_get(name),
            WObj::Props(b) => self.props_call(b, name, a),
            WObj::Prop(b, n) => self.prop_call(b, n, name),
            WObj::Revisions(r) => self.revisions_call(r, name),
            WObj::Fields(r) => self.fields_call(r, name, a),
            WObj::Controls(r, by) => self.controls_call(r, by, name, a),
            WObj::Control(id) => self.control_call(id, name, a),
            WObj::Entries(id) => self.entries_call(id, name, a),
            WObj::Entry(id, k) => self.entry_call(id, k, name),
            o @ (WObj::MailMerge | WObj::MergeSource | WObj::MergeNames(_) | WObj::MergeName(..) | WObj::MergeFields) => self.mail_call(o, name, a),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(crate) fn wpart_set(&mut self, o: WObj, name: &str, v: Value) -> R<()> {
        let v = match v {
            v @ Value::Object(_) => v,
            v => self.scalar(v)?,
        };
        match (o, name) {
            (WObj::Bookmark(_), _) => Err(Error::no_member(name)),
            (WObj::Comment(id), "author" | "initial") => {
                let t = v.to_str()?;
                self.w().side_edit(|d, _| {
                    if let Some(c) = d.comments.iter_mut().find(|c| c.id == id) {
                        c.author = t;
                    }
                })
            }
            (WObj::Comment(id), "done") => {
                let b = v.to_bool()?;
                self.w().side_edit(|d, sel| {
                    d.set_comment_resolved(id, b, sel);
                })
            }
            (WObj::CommentText(id), "text") => {
                let t = clean_input(&v.to_str()?);
                self.set_comment(id, t)
            }
            (WObj::Hyperlink(r), "address" | "subaddress" | "screentip") => {
                let t = v.to_str()?;
                self.set_link(&r, name, t)
            }
            (WObj::Hyperlink(r), "texttodisplay") => {
                let t = clean_input(&v.to_str()?).replace('\n', " ");
                let (s, e) = self.wspan(&Tg::Range(r.clone()));
                let link = self.w().links_in(s, e).into_iter().next().ok_or_else(deleted)?;
                let (at, end) = self.w().replace(link.0, link.1, &t)?;
                let w = self.w();
                w.mutate(at, end, |d, sel| {
                    d.set_link(sel, Some(link.2.clone()), link.3.clone());
                    Some(sel.cursor)
                })?;
                Ok(())
            }
            (WObj::InlineShape(r), "width" | "height") => {
                let x = v.to_f64()?;
                if !(1.0..=1584.0).contains(&x) {
                    return Err(Error::new(4608, "Value out of range"));
                }
                let start = r.borrow().start;
                let w = self.w();
                let p = w.pos_of(start);
                let (_, img) = w.doc.image_at(p).ok_or_else(deleted)?;
                let mut img = (*img).clone();
                if name == "width" {
                    img.width = x as f32;
                } else {
                    img.height = x as f32;
                }
                w.side_edit(|d, _| d.set_image(p, img))
            }
            (WObj::HeaderFooter(i, slot), "linktoprevious") => {
                let link = v.to_bool()?;
                if i == 0 {
                    return Ok(());
                }
                let w = self.w();
                let story = if link { None } else { Some(w.doc.hf_story(i, slot).map(|s| s.to_vec()).unwrap_or_else(|| vec![Paragraph::new("")])) };
                w.side_edit(|d, sel| d.set_hf_story(i, slot, story, sel))
            }
            (WObj::HfRange(i, slot), "text") => {
                let t = clean_input(&v.to_str()?);
                self.set_hf_text(i, slot, &t)
            }
            (WObj::StyleFont(n), _) => self.style_font_set(n, name, v),
            (WObj::HfParaFormat(i, slot), _) => self.hf_pformat_set(i, slot, name, v),
            (WObj::Var(n), _) => self.var_set(n, name, v),
            (WObj::DocShape(k), _) => self.shape_put(k, name, v),
            (WObj::ShapeRange(k), _) => self.shape_text_put(k, name, v),
            (WObj::TableBorders(r, d), _) => self.table_borders_set(r, d, name, v),
            (WObj::CellShading(r, d, row, col), _) => self.cell_shading_set(r, d, row, col, name, v),
            (WObj::Line(r, d, rows, i), _) => self.line_format_set(r, d, rows, Some(i), name, v),
            (WObj::Lines(r, d, rows), _) => self.line_format_set(r, d, rows, None, name, v),
            (WObj::Cell(r, d, row, col), _) => self.cell_format_set(r, d, row, col, name, v),
            (WObj::Table(r, d), "style") => self.table_style_set(r, d, v),
            (WObj::PageSetup, _) => self.wpage_set(name, v),
            (WObj::Prop(b, n), "value") => self.prop_set(b, &n, v),
            (WObj::Control(id), _) => self.cc_set(id, name, v),
            (WObj::Entry(id, k), _) => self.entry_set(id, k, name, v),
            (o @ (WObj::MailMerge | WObj::MergeSource), _) => self.mail_set(o, name, v),
            (o, _) => Err(Error::no_member(&format!("{name} of {}", o.type_name()))),
        }
    }

    pub(crate) fn wpart_items(&mut self, o: &WObj) -> R<Option<Vec<Value>>> {
        let n = match o {
            WObj::Tables(_) | WObj::Lines(..) | WObj::Cells(..) | WObj::Bookmarks | WObj::Comments | WObj::Hyperlinks(_) | WObj::InlineShapes(_) | WObj::Sections | WObj::Props(_) | WObj::Styles | WObj::Vars | WObj::Shapes | WObj::Tocs | WObj::HeadersFooters(..) | WObj::Controls(..) | WObj::Entries(_) | WObj::MergeNames(_) => {
                self.wpart_call(o.clone(), "count", &Args::default())?.to_i64()?
            }
            _ => return Ok(None),
        };
        if let WObj::Props(true) = o {
            return Ok(Some(BUILTIN_PROPS.iter().map(|p| wo(WObj::Prop(true, Rc::from(p.1)))).collect()));
        }
        if n as usize > MAX_ITEMS {
            return Err(Error::new(7, "Out of memory: too many items"));
        }
        let mut out = Vec::with_capacity(n.max(0) as usize);
        for i in 1..=n {
            out.push(self.wpart_call(o.clone(), "item", &Args::one(Value::Long(i as i32)))?);
        }
        Ok(Some(out))
    }

    /// The name or number a collection's Item is given.
    fn item_arg(&mut self, a: &Args) -> R<Value> {
        let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
        self.scalar(v)
    }

    // --- Tables ------------------------------------------------------------------------------

    fn tables_call(&mut self, r: SpanRef, name: &str, a: &Args) -> R<Value> {
        let (s, e) = self.wspan(&Tg::Range(r.clone()));
        match name {
            "count" => Ok(Value::Long(self.w().tables_in(s, e).len() as i32)),
            "item" | "_default" => {
                let n = self.item_arg(a)?.to_i64()?;
                let w = self.w();
                let ts = w.tables_in(s, e);
                let m = &ts[index(n, ts.len())?];
                Ok(self.table_obj(m))
            }
            "add" => {
                let at = self.span_of(&a.get(0, "range").ok_or_else(Error::invalid_call)?)?;
                let rows = self.arg_i64(a, 1, "numrows")?.ok_or_else(Error::invalid_call)?;
                let cols = self.arg_i64(a, 2, "numcolumns")?.ok_or_else(Error::invalid_call)?;
                if !(1..=32_767).contains(&rows) || !(1..=63).contains(&cols) || (rows * cols) as usize > MAX_CELLS {
                    return Err(Error::new(4608, "Value out of range"));
                }
                let pos = if at.0 < at.1 { self.w().replace(at.0, at.1, "")?.0 } else { at.0 };
                let w = self.w();
                let width = w.doc.page.content_width();
                let (_, caret) = w.mutate(pos, pos, |d, sel| Some(d.insert_table(sel, rows as usize, cols as usize, width).cursor))?;
                let p = w.para_of(caret);
                let depth = w.doc.table_depth(p).unwrap_or(0);
                let m = w.table_at(p, depth).ok_or_else(deleted)?;
                Ok(self.table_obj(&m))
            }
            "nestinglevel" => Ok(Value::Long(1)),
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn table_obj(&mut self, m: &TableMap) -> Value {
        let w = self.w();
        let (s, e) = (w.para_start(m.start), w.para_start(m.end));
        let r = w.new_span(s, e);
        wo(WObj::Table(r, m.depth))
    }

    fn table_call(&mut self, r: SpanRef, depth: usize, name: &str, a: &Args) -> R<Value> {
        let m = self.w().table(&r, depth)?;
        match name {
            "rows" | "columns" => {
                let o = WObj::Lines(r, depth, name == "rows");
                self.collection_of(o, a)
            }
            "cell" => {
                let row = self.arg_i64(a, 0, "row")?.ok_or_else(Error::invalid_call)?;
                let col = self.arg_i64(a, 1, "column")?.ok_or_else(Error::invalid_call)?;
                let (row, col) = (index(row, m.rows)?, index(col, m.cols)?);
                self.check_cell(&m, row, col)?;
                Ok(wo(WObj::Cell(r, depth, row, col)))
            }
            "range" => {
                let w = self.w();
                let (s, e) = (w.para_start(m.start), w.para_start(m.end));
                Ok(self.new_range(s, e))
            }
            "delete" => {
                let w = self.w();
                let (s, e) = (w.para_start(m.start), w.para_start(m.end));
                let sel = w.doc_sel(s, s);
                w.mutate_in(s, e, sel, |d, sel| d.delete_table(sel).map(|x| x.cursor))?;
                Ok(Value::Empty)
            }
            "select" => {
                let w = self.w();
                let (s, e) = (w.para_start(m.start), w.para_start(m.end));
                w.anchor = s;
                w.active = e;
                Ok(Value::Empty)
            }
            "nestinglevel" => Ok(Value::Long(depth as i32 + 1)),
            "borders" => Ok(wo(WObj::TableBorders(r, depth))),
            "style" => self.table_style_get(r, depth),
            "uniform" => Ok(Value::Bool(m.span.iter().all(|&s| s == (1, 1)))),
            "tables" => Err(Error::unsupported("Table.Tables (use Cell(r, c).Tables)")),
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::unsupported(&format!("Table.{name}"))),
        }
    }

    /// A collection, or its item with an index.
    fn collection_of(&mut self, o: WObj, a: &Args) -> R<Value> {
        if a.is_empty() { Ok(wo(o)) } else { self.wpart_call(o, "item", a) }
    }

    fn check_cell(&self, m: &TableMap, row: usize, col: usize) -> R<()> {
        if m.anchor.get(row * m.cols + col) != Some(&(row, col)) {
            return Err(Error::new(5991, "Cannot access individual cells because this table has merged cells"));
        }
        Ok(())
    }

    fn wlines_call(&mut self, r: SpanRef, depth: usize, rows: bool, name: &str, a: &Args) -> R<Value> {
        let m = self.w().table(&r, depth)?;
        let count = if rows { m.rows } else { m.cols };
        match name {
            "count" => Ok(Value::Long(count as i32)),
            "item" | "_default" | "first" | "last" => {
                let i = match name {
                    "first" => 0,
                    "last" => count - 1,
                    _ => index(self.item_arg(a)?.to_i64()?, count)?,
                };
                Ok(wo(WObj::Line(r, depth, rows, i)))
            }
            "add" => {
                let before = match a.get(0, if rows { "beforerow" } else { "beforecolumn" }) {
                    Some(Value::Object(Obj::Word(o))) => match *o {
                        WObj::Line(_, _, _, i) => Some(i),
                        _ => return Err(Error::type_mismatch()),
                    },
                    Some(_) => return Err(Error::type_mismatch()),
                    None => None,
                };
                let (row, col, after) = match (rows, before) {
                    (true, Some(i)) => (i, 0, false),
                    (true, None) => (m.rows - 1, 0, true),
                    (false, Some(i)) => (0, i, false),
                    (false, None) => (0, m.cols - 1, true),
                };
                let (row, col) = (m.anchor[row * m.cols + col].0, m.anchor[row * m.cols + col].1);
                let k = row * m.cols + col;
                let w = self.w();
                let (s, e) = (w.para_start(m.start), w.para_start(m.end));
                let c = w.para_start(m.cells[k].0);
                let sel = w.doc_sel(c, c);
                let width = w.doc.page.content_width();
                w.mutate_in(s, e, sel, |d, sel| if rows { d.table_insert_rows(sel, after) } else { d.table_insert_cols(sel, after, width) }.map(|x| x.cursor))?;
                let n = if after { if rows { m.rows } else { m.cols } } else { before.unwrap_or(0) };
                Ok(wo(WObj::Line(r, depth, rows, n)))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Table(r, depth))),
            _ => self.line_format_get(r, depth, rows, None, name),
        }
    }

    fn line_call(&mut self, r: SpanRef, depth: usize, rows: bool, i: usize, name: &str, a: &Args) -> R<Value> {
        let m = self.w().table(&r, depth)?;
        if i >= if rows { m.rows } else { m.cols } {
            return Err(deleted());
        }
        match name {
            "cells" => self.collection_of(WObj::Cells(r, depth, Some((rows, i))), a),
            "index" => Ok(Value::Long(i as i32 + 1)),
            "isfirst" => Ok(Value::Bool(i == 0)),
            "islast" => Ok(Value::Bool(i + 1 == if rows { m.rows } else { m.cols })),
            "range" if rows => {
                let cells = &m.cells[i * m.cols..(i + 1) * m.cols];
                let a0 = cells.iter().map(|c| c.0).min().unwrap_or(m.start);
                let b0 = cells.iter().map(|c| c.1).max().unwrap_or(m.end);
                let w = self.w();
                let (s, e) = (w.para_start(a0), w.para_start(b0));
                Ok(self.new_range(s, e))
            }
            "select" if rows => {
                let cells = &m.cells[i * m.cols..(i + 1) * m.cols];
                let a0 = cells.iter().map(|c| c.0).min().unwrap_or(m.start);
                let b0 = cells.iter().map(|c| c.1).max().unwrap_or(m.end);
                let w = self.w();
                let (s, e) = (w.para_start(a0), w.para_start(b0));
                w.anchor = s;
                w.active = e;
                Ok(Value::Empty)
            }
            "delete" => {
                let k = if rows { i * m.cols } else { i };
                let w = self.w();
                let (s, e) = (w.para_start(m.start), w.para_start(m.end));
                let c = w.para_start(m.cells[k].0);
                let sel = w.doc_sel(c, c);
                w.mutate_in(s, e, sel, |d, sel| if rows { d.table_delete_rows(sel) } else { d.table_delete_cols(sel) }.map(|x| x.cursor))?;
                Ok(Value::Empty)
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Table(r, depth))),
            _ => self.line_format_get(r, depth, rows, Some(i), name),
        }
    }

    fn wcells_call(&mut self, r: SpanRef, depth: usize, which: Option<(bool, usize)>, name: &str, a: &Args) -> R<Value> {
        let m = self.w().table(&r, depth)?;
        let count = match which {
            Some((true, _)) => m.cols,
            Some((false, _)) => m.rows,
            None => m.rows * m.cols,
        };
        match name {
            "count" => Ok(Value::Long(count as i32)),
            "item" | "_default" => {
                let k = index(self.item_arg(a)?.to_i64()?, count)?;
                let (row, col) = match which {
                    Some((true, i)) => (i, k),
                    Some((false, i)) => (k, i),
                    None => (k / m.cols.max(1), k % m.cols.max(1)),
                };
                self.check_cell(&m, row, col)?;
                Ok(wo(WObj::Cell(r, depth, row, col)))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::unsupported(&format!("Cells.{name}"))),
        }
    }

    fn cell_call(&mut self, r: SpanRef, depth: usize, row: usize, col: usize, name: &str) -> R<Value> {
        let m = self.w().table(&r, depth)?;
        if row >= m.rows || col >= m.cols {
            return Err(deleted());
        }
        let (a0, b0) = m.cells[row * m.cols + col];
        let w = self.w();
        let (s, e) = (w.para_start(a0), w.para_start(b0));
        match name {
            "range" => Ok(self.new_range(s, e)),
            "rowindex" => Ok(Value::Long(row as i32 + 1)),
            "columnindex" => Ok(Value::Long(col as i32 + 1)),
            "select" => {
                w.anchor = s;
                w.active = e;
                Ok(Value::Empty)
            }
            "tables" => {
                let sp = w.new_span(s, e);
                Ok(wo(WObj::Tables(sp)))
            }
            "next" | "previous" => {
                let k = row * m.cols + col;
                let k2 = if name == "next" { k.checked_add(1).filter(|&x| x < m.cells.len()) } else { k.checked_sub(1) };
                Ok(match k2 {
                    Some(k2) => wo(WObj::Cell(r, depth, k2 / m.cols, k2 % m.cols)),
                    None => Value::Nothing,
                })
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Table(r, depth))),
            _ => self.cell_format_get(r, depth, row, col, name),
        }
    }

    // --- Bookmarks ---------------------------------------------------------------------------

    fn bookmarks_call(&mut self, name: &str, a: &Args) -> R<Value> {
        let list: Vec<Rc<str>> = self.w().doc.bookmarks().into_iter().filter(|b| !b.is_hidden()).map(|b| Rc::from(&*b.name)).collect();
        match name {
            "count" => Ok(Value::Long(list.len() as i32)),
            "item" | "_default" => {
                let v = self.item_arg(a)?;
                let n = match v {
                    Value::Str(s) => list.iter().find(|n| n.eq_ignore_ascii_case(&s)).cloned().or_else(|| self.w().doc.bookmark(&s).map(|b| Rc::from(&*b.name))).ok_or_else(missing)?,
                    v => list[index(v.to_i64()?, list.len())?].clone(),
                };
                Ok(wo(WObj::Bookmark(n)))
            }
            "exists" => {
                let n = self.arg_str(a, 0, "name")?.ok_or_else(Error::invalid_call)?;
                Ok(Value::Bool(self.w().doc.bookmark(&n).is_some()))
            }
            "add" => {
                let n = self.arg_str(a, 0, "name")?.ok_or_else(Error::invalid_call)?;
                if !apex_doc::valid_bookmark_name(&n) {
                    return Err(Error::new(5828, "Bookmark name is not valid"));
                }
                let (s, e) = match a.get(1, "range") {
                    Some(v) => self.span_of(&v)?,
                    None => self.w().sel(),
                };
                let w = self.w();
                w.mutate(s, e, |d, sel| {
                    d.add_bookmark(&n, sel);
                    Some(sel.cursor)
                })?;
                Ok(wo(WObj::Bookmark(Rc::from(n.as_str()))))
            }
            "showhidden" => Ok(Value::Bool(false)),
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn bookmark_call(&mut self, n: Rc<str>, name: &str) -> R<Value> {
        let w = self.w();
        let b = w.doc.bookmark(&n).ok_or_else(deleted)?;
        let (s, e) = (w.char_of(b.start), w.char_of(b.end));
        match name {
            "name" | "_default" => Ok(Value::str(&*b.name)),
            "range" => Ok(self.new_range(s, e)),
            "start" => Ok(Value::Long(s as i32)),
            "end" => Ok(Value::Long(e as i32)),
            "empty" => Ok(Value::Bool(s == e)),
            "select" => {
                w.anchor = s;
                w.active = e;
                Ok(Value::Empty)
            }
            "delete" => {
                w.mutate(s, s, |d, sel| {
                    d.remove_bookmark(&n, sel);
                    Some(sel.cursor)
                })?;
                Ok(Value::Empty)
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    // --- Comments ----------------------------------------------------------------------------

    /// The comments' ids in the order of their places in the text.
    fn comment_ids(&mut self) -> Vec<u32> {
        let doc = &self.w().doc;
        let mut spans = doc.comment_spans();
        spans.sort_by_key(|s| s.start);
        let mut ids: Vec<u32> = spans.iter().map(|s| s.id).filter(|id| doc.comments.iter().any(|c| c.id == *id)).collect();
        ids.extend(doc.comments.iter().map(|c| c.id).filter(|id| !spans.iter().any(|s| s.id == *id)));
        ids
    }

    fn comments_call(&mut self, name: &str, a: &Args) -> R<Value> {
        match name {
            "count" => Ok(Value::Long(self.comment_ids().len() as i32)),
            "item" | "_default" => {
                let ids = self.comment_ids();
                let k = index(self.item_arg(a)?.to_i64()?, ids.len())?;
                Ok(wo(WObj::Comment(ids[k])))
            }
            "add" => {
                let (s, e) = self.span_of(&a.get(0, "range").ok_or_else(Error::invalid_call)?)?;
                let t = clean_input(&self.arg_str(a, 1, "text")?.unwrap_or_default()).replace('\n', "\r");
                if t.len() > 1 << 20 {
                    return Err(Error::new(7, "Out of memory: the comment is too long"));
                }
                let w = self.w();
                if w.doc.comments.len() >= MAX_COMMENTS {
                    return Err(Error::new(7, "Out of memory: too many comments"));
                }
                let mut id = 0;
                w.mutate(s, e, |d, sel| {
                    id = d.add_comment(sel, &t.replace('\r', "\n"));
                    Some(sel.cursor)
                })?;
                Ok(wo(WObj::Comment(id)))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn comment_call(&mut self, id: u32, name: &str) -> R<Value> {
        let w = self.w();
        let c = w.doc.comments.iter().find(|c| c.id == id).cloned().ok_or_else(deleted)?;
        match name {
            "range" => Ok(wo(WObj::CommentText(id))),
            "scope" => {
                let sp = w.doc.comment_spans().into_iter().find(|s| s.id == id);
                let (s, e) = match sp {
                    Some(sp) => (w.char_of(sp.start), w.char_of(sp.end)),
                    None => (0, 0),
                };
                Ok(self.new_range(s, e))
            }
            "author" => Ok(Value::str(c.author)),
            "initial" => Ok(Value::str(c.author.split_whitespace().filter_map(|x| x.chars().next()).collect::<String>())),
            "date" => Ok(Value::str(c.date)),
            "done" => Ok(Value::Bool(c.resolved)),
            "index" => Ok(Value::Long(self.comment_ids().iter().position(|&x| x == id).map_or(0, |k| k as i32 + 1))),
            "delete" => {
                w.side_edit(|d, sel| {
                    d.delete_comments(&[id], sel);
                })?;
                Ok(Value::Empty)
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::unsupported(&format!("Comment.{name}"))),
        }
    }

    fn set_comment(&mut self, id: u32, t: String) -> R<()> {
        if t.len() > 1 << 20 {
            return Err(Error::new(7, "Out of memory: the comment is too long"));
        }
        let w = self.w();
        if !w.doc.comments.iter().any(|c| c.id == id) {
            return Err(deleted());
        }
        w.side_edit(|d, sel| {
            d.set_comment_text(id, &t, sel);
        })
    }

    fn comment_text_call(&mut self, id: u32, name: &str, a: &Args) -> R<Value> {
        let w = self.w();
        let c = w.doc.comments.iter().find(|c| c.id == id).cloned().ok_or_else(deleted)?;
        match name {
            "text" | "_default" => Ok(Value::str(c.text.replace('\n', "\r"))),
            "insertafter" | "insertbefore" => {
                let t = clean_input(&self.arg_str(a, 0, "text")?.ok_or_else(Error::invalid_call)?);
                let new = if name == "insertafter" { format!("{}{t}", c.text) } else { format!("{t}{}", c.text) };
                self.set_comment(id, new)?;
                Ok(Value::Empty)
            }
            "delete" => {
                self.set_comment(id, String::new())?;
                Ok(Value::Empty)
            }
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::unsupported(&format!("the {name} of a comment's Range"))),
        }
    }

    // --- Hyperlinks and pictures ----------------------------------------------------------------

    fn links_call(&mut self, r: SpanRef, name: &str, a: &Args) -> R<Value> {
        let (s, e) = self.wspan(&Tg::Range(r));
        match name {
            "count" => Ok(Value::Long(self.w().links_in(s, e).len() as i32)),
            "item" | "_default" => {
                let k = self.item_arg(a)?.to_i64()?;
                let w = self.w();
                let ls = w.links_in(s, e);
                let l = &ls[index(k, ls.len())?];
                let sp = w.new_span(l.0, l.1);
                Ok(wo(WObj::Hyperlink(sp)))
            }
            "add" => {
                let (ls, le) = self.span_of(&a.get(0, "anchor").ok_or_else(Error::invalid_call)?)?;
                let address = self.arg_str(a, 1, "address")?.unwrap_or_default();
                let sub = self.arg_str(a, 2, "subaddress")?.unwrap_or_default();
                let tip = self.arg_str(a, 3, "screentip")?.filter(|t| !t.is_empty());
                let shown = self.arg_str(a, 4, "texttodisplay")?;
                let target = if sub.is_empty() { address.trim().to_string() } else { format!("{}#{}", address.trim(), sub.trim()) };
                if target.is_empty() || !safe_address(&target) || target.len() > 2083 || target.chars().any(char::is_control) {
                    return Err(Error::new(4198, "Command failed: macros may add web, e-mail and bookmark links only"));
                }
                let link: Arc<str> = Arc::from(target.as_str());
                let tip: Option<Arc<str>> = tip.map(|t| Arc::from(t.as_str()));
                let text = match shown {
                    Some(t) if !t.is_empty() => clean_input(&t).replace('\n', " "),
                    _ if ls < le => String::new(),
                    _ => address.clone(),
                };
                let (s2, e2) = if text.is_empty() { (ls, le) } else { self.w().replace(ls, le, &text)? };
                let w = self.w();
                w.mutate(s2, e2, |d, sel| {
                    d.set_link(sel, Some(link.clone()), tip.clone());
                    Some(sel.cursor)
                })?;
                let sp = w.new_span(s2, e2);
                Ok(wo(WObj::Hyperlink(sp)))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn link_call(&mut self, r: SpanRef, name: &str) -> R<Value> {
        let (s, e) = self.wspan(&Tg::Range(r.clone()));
        let w = self.w();
        let l = w.links_in(s, e).into_iter().next().ok_or_else(deleted)?;
        let (addr, sub) = match l.2.split_once('#') {
            Some((a0, b0)) => (a0.to_string(), b0.to_string()),
            None => (l.2.to_string(), String::new()),
        };
        match name {
            "address" => Ok(Value::str(addr)),
            "subaddress" => Ok(Value::str(sub)),
            "name" | "_default" => Ok(Value::str(&*l.2)),
            "screentip" => Ok(Value::str(l.3.as_deref().unwrap_or(""))),
            "texttodisplay" => Ok(Value::str(w.text(l.0, l.1)?)),
            "range" => Ok(self.new_range(l.0, l.1)),
            "delete" => {
                w.mutate(l.0, l.1, |d, sel| {
                    d.set_link(sel, None, None);
                    Some(sel.cursor)
                })?;
                Ok(Value::Empty)
            }
            "follow" => Err(Error::unsupported("Hyperlink.Follow (macros don't open links)")),
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn set_link(&mut self, r: &SpanRef, name: &str, t: String) -> R<()> {
        let (s, e) = self.wspan(&Tg::Range(r.clone()));
        let w = self.w();
        let l = w.links_in(s, e).into_iter().next().ok_or_else(deleted)?;
        let (addr, sub) = match l.2.split_once('#') {
            Some((a0, b0)) => (a0.to_string(), b0.to_string()),
            None => (l.2.to_string(), String::new()),
        };
        let (target, tip) = match name {
            "address" => (if sub.is_empty() { t.trim().to_string() } else { format!("{}#{sub}", t.trim()) }, l.3.clone()),
            "subaddress" => (if t.trim().is_empty() { addr } else { format!("{addr}#{}", t.trim()) }, l.3.clone()),
            _ => (l.2.to_string(), (!t.is_empty()).then(|| Arc::from(t.as_str()))),
        };
        if target.is_empty() || !safe_address(&target) || target.len() > 2083 || target.chars().any(char::is_control) {
            return Err(Error::new(4198, "Command failed: macros may add web, e-mail and bookmark links only"));
        }
        let link: Arc<str> = Arc::from(target.as_str());
        w.mutate(l.0, l.1, |d, sel| {
            d.set_link(sel, Some(link), tip);
            Some(sel.cursor)
        })?;
        Ok(())
    }

    fn shapes_call(&mut self, r: SpanRef, name: &str, a: &Args) -> R<Value> {
        let (s, e) = self.wspan(&Tg::Range(r));
        match name {
            "count" => Ok(Value::Long(self.w().shapes_in(s, e).len() as i32)),
            "item" | "_default" => {
                let k = self.item_arg(a)?.to_i64()?;
                let w = self.w();
                let cs = w.shapes_in(s, e);
                let c = cs[index(k, cs.len())?];
                let sp = w.new_span(c, c + 1);
                Ok(wo(WObj::InlineShape(sp)))
            }
            "addpicture" => Err(Error::unsupported("InlineShapes.AddPicture (macros don't read pictures from files)")),
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn shape_call(&mut self, r: SpanRef, name: &str) -> R<Value> {
        let start = r.borrow().start;
        let w = self.w();
        let p = w.pos_of(start);
        let (_, img) = w.doc.image_at(p).ok_or_else(deleted)?;
        match name {
            "width" => Ok(Value::Double(img.width as f64)),
            "height" => Ok(Value::Double(img.height as f64)),
            "type" => Ok(Value::Long(3)),
            "range" => Ok(self.new_range(start, start + 1)),
            "delete" => {
                w.replace(start, start + 1, "")?;
                Ok(Value::Empty)
            }
            "select" => {
                w.anchor = start;
                w.active = start + 1;
                Ok(Value::Empty)
            }
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::unsupported(&format!("InlineShape.{name}"))),
        }
    }

    // --- Styles ---------------------------------------------------------------------------------

    /// The styles there are: the built-in ones, then the document's.
    fn style_names(&mut self) -> Vec<String> {
        let w = self.w();
        let mut out: Vec<String> = ["Normal", "Heading 1", "Heading 2", "Heading 3", "Heading 4", "Heading 5", "Heading 6", "Heading 7", "Heading 8", "Heading 9", "Title", "Quote", "Code", "List Bullet", "List Number"].iter().map(|s| s.to_string()).collect();
        out.extend(w.doc.styles.defs.iter().filter(|d| !d.builtin).map(|d| d.name.to_string()));
        out
    }

    fn styles_call(&mut self, name: &str, a: &Args) -> R<Value> {
        match name {
            "count" => Ok(Value::Long(self.style_names().len() as i32)),
            "item" | "_default" => {
                let v = self.item_arg(a)?;
                let positive = !matches!(v, Value::Str(_)) && v.to_i64()? > 0;
                let n = match &v {
                    v if positive => {
                        let names = self.style_names();
                        names[index(v.to_i64()?, names.len())?].clone()
                    }
                    v => {
                        let w = self.w();
                        match w.style_of(v)?.ok_or_else(|| Error::new(5834, "Item with specified name does not exist"))? {
                            (_, n, true) => w.doc.styles.defs[n as usize - 1].name.to_string(),
                            (k, c, false) => w.style_name(k, c),
                        }
                    }
                };
                Ok(wo(WObj::Style(Rc::from(n.as_str()))))
            }
            "add" => {
                let n = self.arg_str(a, 0, "name")?.ok_or_else(Error::invalid_call)?;
                let n = n.trim();
                if n.is_empty() || n.chars().count() > 255 || n.chars().any(char::is_control) {
                    return Err(Error::invalid_call());
                }
                let char_style = match self.arg_i64(a, 1, "type")?.unwrap_or(1) {
                    1 => false,
                    2 => true,
                    _ => return Err(Error::unsupported("this WdStyleType (only paragraph and character styles)")),
                };
                let w = self.w();
                if w.style_of(&Value::str(n))?.is_some() {
                    return Err(Error::new(5170, "The style name already exists"));
                }
                let def = apex_doc::StyleDef { id: w.doc.styles.new_id(n), name: Arc::from(n), char_style, ..Default::default() };
                w.side_edit(|d, sel| d.set_style(def, sel))?;
                Ok(wo(WObj::Style(Rc::from(n))))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn style_call(&mut self, n: Rc<str>, name: &str) -> R<Value> {
        let w = self.w();
        let st = w.style_of(&Value::str(&*n))?.ok_or_else(deleted)?;
        match name {
            "namelocal" | "name" | "_default" => Ok(Value::str(&*n)),
            "type" => Ok(Value::Long(if st.2 { 2 } else { 1 })),
            "builtin" => Ok(Value::Bool(st.1 == 0)),
            "inuse" => Ok(Value::Bool(true)),
            "font" => Ok(wo(WObj::StyleFont(n))),
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::unsupported(&format!("Style.{name} (change styles from the Styles panel)"))),
        }
    }

    /// The definition of a style (a change to a built-in one, or a new one
    /// made to be filled), with the formatting text of the style has.
    fn style_def(&mut self, n: &str) -> R<(apex_doc::StyleDef, apex_doc::ParaStyle, apex_doc::Attrs)> {
        let w = self.w();
        let (kind, custom, is_char) = w.style_of(&Value::str(n))?.ok_or_else(deleted)?;
        let name = w.style_name(kind, custom);
        if is_char {
            let def = w.doc.styles.defs[custom as usize - 1].clone();
            let attrs = apex_doc::Attrs { char_style: Some(def.id.clone()), ..Default::default() };
            return Ok((def, apex_doc::ParaStyle::default(), attrs));
        }
        let st = apex_doc::ParaStyle { kind, custom, ..Default::default() };
        let def = match (w.doc.styles.custom(custom), w.doc.styles.builtin(kind)) {
            (Some(d), _) => d.clone(),
            (None, Some(d)) => d.clone(),
            _ => apex_doc::StyleDef { id: Arc::from(apex_doc::kind_id(kind)), name: Arc::from(name.as_str()), kind, builtin: true, ..Default::default() },
        };
        Ok((def, st, apex_doc::Attrs::default()))
    }

    fn style_font_get(&mut self, n: Rc<str>, name: &str) -> R<Value> {
        let (_, st, attrs) = self.style_def(&n)?;
        let default_font = self.w().doc.default_font_name();
        let f = self.w().doc.char_format(&st, &attrs);
        Ok(match name {
            "bold" => wbool(f.bold),
            "italic" => wbool(f.italic),
            "underline" => Value::Long(f.underline as i32),
            "size" => Value::Double(f.size as f64 / 2.0),
            "name" => Value::str(f.font.or(default_font).as_deref().unwrap_or("Calibri")),
            "color" | "textcolor" => Value::Long(f.color.map_or(super::range::AUTO as i32, super::range::to_bgr)),
            "application" => Value::Object(Obj::Application),
            "parent" => wo(WObj::Style(n)),
            _ => return Err(Error::no_member(name)),
        })
    }

    fn style_font_set(&mut self, n: Rc<str>, name: &str, v: Value) -> R<()> {
        let (mut def, ..) = self.style_def(&n)?;
        match name {
            "bold" => def.bold = Some(v.to_bool()?),
            "italic" => def.italic = Some(v.to_bool()?),
            "underline" => def.underline = Some(if matches!(v, Value::Bool(_)) { v.to_bool()? } else { v.to_i64()? != 0 }),
            "size" => def.size = Some(super::range::half_points(v.to_f64()?)?),
            "name" => {
                let t = v.to_str()?;
                let t = t.trim();
                if t.is_empty() || t.chars().count() > 31 || t.chars().any(char::is_control) {
                    return Err(Error::invalid_call());
                }
                def.font = Some(Arc::from(t));
            }
            "color" | "textcolor" => def.color = super::range::from_bgr(v.to_i64()?)?,
            _ => return Err(Error::no_member(name)),
        }
        self.w().side_edit(|d, sel| d.set_style(def, sel))
    }

    // --- Sections, headers and footers ----------------------------------------------------------

    fn sections_call(&mut self, name: &str, a: &Args) -> R<Value> {
        let n = self.w().doc.sections().len();
        match name {
            "count" => Ok(Value::Long(n as i32)),
            "item" | "_default" => Ok(wo(WObj::Section(index(self.item_arg(a)?.to_i64()?, n)?))),
            "first" => Ok(wo(WObj::Section(0))),
            "last" => Ok(wo(WObj::Section(n - 1))),
            "add" => {
                let at = match a.get(0, "range") {
                    Some(v) => self.span_of(&v)?.0,
                    None => self.w().story_len() - 1,
                };
                let kind = match self.arg_i64(a, 1, "start")?.unwrap_or(2) {
                    0 => apex_doc::SectionStart::Continuous,
                    2 => apex_doc::SectionStart::NextPage,
                    3 => apex_doc::SectionStart::EvenPage,
                    4 => apex_doc::SectionStart::OddPage,
                    _ => return Err(Error::unsupported("this WdSectionStart")),
                };
                let w = self.w();
                let (_, end) = w.mutate(at, at, |d, sel| d.insert_break(sel, apex_doc::BreakKind::Section(kind)))?;
                let p = w.para_of(end);
                Ok(wo(WObj::Section(w.doc.section_index(p))))
            }
            "pagesetup" => Ok(wo(WObj::PageSetup)),
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn section_call(&mut self, i: usize, name: &str, a: &Args) -> R<Value> {
        let w = self.w();
        let secs: Vec<(usize, usize)> = w.doc.sections().iter().map(|s| (s.first, s.end)).collect();
        let &(first, end) = secs.get(i).ok_or_else(deleted)?;
        match name {
            "range" => {
                let (s, e) = (w.para_start(first), w.para_start(end));
                Ok(self.new_range(s, e))
            }
            "headers" | "footers" => self.collection_of(WObj::HeadersFooters(i, name == "headers"), a),
            "pagesetup" => Ok(wo(WObj::PageSetup)),
            "index" => Ok(Value::Long(i as i32 + 1)),
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::unsupported(&format!("Section.{name}"))),
        }
    }

    fn hfs_call(&mut self, i: usize, header: bool, name: &str, a: &Args) -> R<Value> {
        match name {
            "count" => Ok(Value::Long(3)),
            "item" | "_default" => {
                let k = self.item_arg(a)?.to_i64()?;
                let kind = match k {
                    1 => 0,
                    2 => 1,
                    3 => 2,
                    _ => return Err(missing()),
                };
                Ok(wo(WObj::HeaderFooter(i, HfSlot::of(header, kind))))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn hf_call(&mut self, i: usize, slot: HfSlot, name: &str) -> R<Value> {
        let w = self.w();
        match name {
            "range" => Ok(wo(WObj::HfRange(i, slot))),
            "exists" => Ok(Value::Bool(slot.kind() == 0 || w.doc.hf_story(i, slot).is_some())),
            "linktoprevious" => Ok(Value::Bool(w.doc.hf_linked(i, slot))),
            "pagenumbers" => Ok(wo(WObj::HfNumbers(i, slot))),
            "index" => Ok(Value::Long(slot.kind() as i32 + 1)),
            "isheader" => Ok(Value::Bool(slot.is_header())),
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::unsupported(&format!("HeaderFooter.{name}"))),
        }
    }

    fn set_hf_text(&mut self, i: usize, slot: HfSlot, t: &str) -> R<()> {
        if t.len() > 1 << 20 {
            return Err(Error::new(7, "Out of memory: the header is too long"));
        }
        let w = self.w();
        let like = w.doc.hf_story(i, slot).and_then(|s| s.first().cloned());
        let paras = paras_of_text(t, like.as_ref());
        w.side_edit(|d, sel| d.set_hf_story(i, slot, Some(paras), sel))
    }

    fn hf_range_call(&mut self, i: usize, slot: HfSlot, name: &str, a: &Args) -> R<Value> {
        let text = self.w().hf_text(i, slot);
        match name {
            "text" | "_default" => Ok(Value::str(text)),
            "insertafter" | "insertbefore" => {
                let t = clean_input(&self.arg_str(a, 0, "text")?.ok_or_else(Error::invalid_call)?);
                let old = clean_input(text.strip_suffix('\r').unwrap_or(&text));
                let new = if name == "insertafter" { format!("{old}{t}") } else { format!("{t}{old}") };
                self.set_hf_text(i, slot, &new)?;
                Ok(Value::Empty)
            }
            "delete" => {
                self.set_hf_text(i, slot, "")?;
                Ok(Value::Empty)
            }
            // The Range stands for the whole header or footer, and what is
            // added goes at its end: collapsing it changes nothing.
            "collapse" => Ok(Value::Empty),
            "paragraphformat" | "format" => Ok(wo(WObj::HfParaFormat(i, slot))),
            "fields" => Ok(wo(WObj::HfFields(i, slot))),
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::unsupported(&format!("the {name} of a header's or footer's Range"))),
        }
    }

    // --- PageSetup ---------------------------------------------------------------------------

    fn wpage_get(&mut self, name: &str) -> R<Value> {
        let w = self.w();
        let p = &w.doc.page;
        Ok(match name {
            "topmargin" => Value::Double(p.margin_top as f64),
            "bottommargin" => Value::Double(p.margin_bottom as f64),
            "leftmargin" => Value::Double(p.margin_left as f64),
            "rightmargin" => Value::Double(p.margin_right as f64),
            "headerdistance" => Value::Double(p.margin_header as f64),
            "footerdistance" => Value::Double(p.margin_footer as f64),
            "gutter" => Value::Double(p.gutter as f64),
            "pagewidth" => Value::Double(p.width as f64),
            "pageheight" => Value::Double(p.height as f64),
            "orientation" => Value::Long(if p.landscape() { 1 } else { 0 }),
            "differentfirstpageheaderfooter" => wbool(w.doc.section_headers(0).different_first),
            "oddandevenpagesheaderfooter" => wbool(w.doc.odd_even),
            "application" => Value::Object(Obj::Application),
            _ => return Err(Error::unsupported(&format!("PageSetup.{name}"))),
        })
    }

    fn wpage_set(&mut self, name: &str, v: Value) -> R<()> {
        let w = self.w();
        if matches!(name, "differentfirstpageheaderfooter" | "oddandevenpagesheaderfooter") {
            let b = v.to_bool()?;
            let n = w.doc.sections().len();
            return w.side_edit(|d, sel| {
                for s in 0..n {
                    let first = if name == "differentfirstpageheaderfooter" { b } else { d.section_headers(s).different_first };
                    let odd = if name == "oddandevenpagesheaderfooter" { b } else { d.odd_even };
                    d.set_hf_options(s, first, odd, sel);
                }
            });
        }
        if name == "orientation" {
            let land = match v.to_i64()? {
                0 => false,
                1 => true,
                _ => return Err(Error::new(4608, "Value out of range")),
            };
            return w.side_edit(|d, _| d.set_all_pages(|p| p.set_landscape(land)));
        }
        let x = v.to_f64()?;
        let (lo, hi) = if matches!(name, "pagewidth" | "pageheight") { (36.0, 1584.0) } else { (0.0, 1584.0) };
        if !(lo..=hi).contains(&x) {
            return Err(Error::new(4608, "Value out of range"));
        }
        let x = x as f32;
        let f: Box<dyn Fn(&mut apex_doc::PageSetup)> = match name {
            "topmargin" => Box::new(move |p| p.margin_top = x),
            "bottommargin" => Box::new(move |p| p.margin_bottom = x),
            "leftmargin" => Box::new(move |p| p.margin_left = x),
            "rightmargin" => Box::new(move |p| p.margin_right = x),
            "headerdistance" => Box::new(move |p| p.margin_header = x),
            "footerdistance" => Box::new(move |p| p.margin_footer = x),
            "gutter" => Box::new(move |p| p.gutter = x),
            "pagewidth" => Box::new(move |p| p.width = x),
            "pageheight" => Box::new(move |p| p.height = x),
            _ => return Err(Error::unsupported(&format!("PageSetup.{name}"))),
        };
        // The text keeps some room: margins can't meet.
        let mut test = w.doc.page;
        f(&mut test);
        if test.content_width() < 18.0 || test.content_height() < 18.0 {
            return Err(Error::new(4608, "Value out of range: the margins leave no room for the text"));
        }
        w.side_edit(|d, _| d.set_all_pages(|p| f(p)))
    }

    // --- Document properties ------------------------------------------------------------------

    fn props_call(&mut self, builtin: bool, name: &str, a: &Args) -> R<Value> {
        let names: Vec<String> = if builtin { BUILTIN_PROPS.iter().map(|s| s.1.to_string()).collect() } else { self.w().doc.meta.custom.iter().map(|c| c.name.clone()).collect() };
        match name {
            "count" => Ok(Value::Long(names.len() as i32)),
            "item" | "_default" => {
                let v = self.item_arg(a)?;
                let n = match v {
                    Value::Str(s) => names.iter().find(|n| n.eq_ignore_ascii_case(s.trim())).cloned().ok_or_else(|| Error::new(5, "Invalid procedure call or argument: there is no such property"))?,
                    v if builtin => {
                        let k = v.to_i64()?;
                        let (_, n) = BUILTIN_PROPS.iter().find(|p| p.0 == k).ok_or_else(|| Error::new(5, "Invalid procedure call or argument: there is no such property"))?;
                        n.to_string()
                    }
                    v => names[index(v.to_i64()?, names.len())?].clone(),
                };
                Ok(wo(WObj::Prop(builtin, Rc::from(n.as_str()))))
            }
            "add" if !builtin => {
                let n = self.arg_str(a, 0, "name")?.ok_or_else(Error::invalid_call)?;
                let n = n.trim().to_string();
                if n.is_empty() || n.chars().count() > 255 || n.chars().any(char::is_control) {
                    return Err(Error::invalid_call());
                }
                if names.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                    return Err(Error::new(5, "Invalid procedure call or argument: the property exists already"));
                }
                if names.len() >= 1000 {
                    return Err(Error::new(7, "Out of memory: too many properties"));
                }
                let ty = self.arg_i64(a, 2, "type")?.unwrap_or(4);
                let v = a.get(3, "value").ok_or_else(Error::invalid_call)?;
                let v = self.scalar(v)?;
                let value = prop_value(ty, &v)?;
                self.w().side_edit(|d, _| d.meta.custom.push(CustomProp { name: n.clone(), value }))?;
                Ok(wo(WObj::Prop(false, Rc::from(n.as_str()))))
            }
            "application" => Ok(Value::Object(Obj::Application)),
            "parent" => Ok(wo(WObj::Doc)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn prop_call(&mut self, builtin: bool, n: Rc<str>, name: &str) -> R<Value> {
        match name {
            "name" => return Ok(Value::str(&*n)),
            "application" => return Ok(Value::Object(Obj::Application)),
            "delete" if !builtin => {
                let w = self.w();
                if !w.doc.meta.custom.iter().any(|c| c.name == *n) {
                    return Err(deleted());
                }
                w.side_edit(|d, _| d.meta.custom.retain(|c| c.name != *n))?;
                return Ok(Value::Empty);
            }
            "value" | "_default" | "type" => {}
            _ => return Err(Error::no_member(name)),
        }
        let w = self.w();
        let (v, ty) = if builtin {
            let m = &w.doc.meta;
            let text = |s: &str| (Value::str(s), 4);
            match &*n {
                "Title" => text(&m.title),
                "Subject" => text(&m.subject),
                "Author" => text(&m.author),
                "Keywords" => text(&m.keywords),
                "Comments" => text(&m.description),
                "Category" => text(&m.category),
                "Company" => text(&m.company),
                "Manager" => text(&m.manager),
                "Creation Date" => text(&m.created),
                _ => {
                    let len = w.story_len();
                    let what = match &*n {
                        "Number of Words" => 0,
                        "Number of Characters" => 3,
                        "Number of Paragraphs" => 4,
                        _ => 5,
                    };
                    (self.statistics(0, len, what)?, 1)
                }
            }
        } else {
            let c = w.doc.meta.custom.iter().find(|c| c.name == *n).ok_or_else(deleted)?;
            match &c.value {
                PropValue::Text(t) => (Value::str(t.as_str()), 4),
                PropValue::Number(x) => (Value::Double(*x), 1),
                PropValue::Bool(b) => (Value::Bool(*b), 2),
                PropValue::Date(d) => (Value::str(d.as_str()), 3),
            }
        };
        Ok(if name == "type" { Value::Long(ty) } else { v })
    }

    fn prop_set(&mut self, builtin: bool, n: &str, v: Value) -> R<()> {
        let w = self.w();
        if builtin {
            let t = v.to_str()?;
            if t.len() > 1 << 16 {
                return Err(Error::new(7, "Out of memory: the property is too long"));
            }
            let t = t.replace(['\r', '\n'], " ");
            return w.side_edit(|d, _| {
                let m = &mut d.meta;
                let slot = match n {
                    "Title" => &mut m.title,
                    "Subject" => &mut m.subject,
                    "Author" => &mut m.author,
                    "Keywords" => &mut m.keywords,
                    "Comments" => &mut m.description,
                    "Category" => &mut m.category,
                    "Company" => &mut m.company,
                    "Manager" => &mut m.manager,
                    _ => return Err(Error::new(5, "Invalid procedure call or argument: this property can't be changed")),
                };
                *slot = t;
                Ok(())
            })?;
        }
        let k = w.doc.meta.custom.iter().position(|c| c.name == n).ok_or_else(deleted)?;
        let ty = match w.doc.meta.custom[k].value {
            PropValue::Number(_) => 1,
            PropValue::Bool(_) => 2,
            PropValue::Date(_) => 3,
            PropValue::Text(_) => 4,
        };
        let value = prop_value(ty, &v)?;
        w.side_edit(|d, _| d.meta.custom[k].value = value)
    }

    // --- Revisions and fields -------------------------------------------------------------------

    fn revisions_call(&mut self, r: SpanRef, name: &str) -> R<Value> {
        let (s, e) = self.wspan(&Tg::Range(r));
        let w = self.w();
        let len = w.story_len();
        let whole = s == 0 && e >= len.saturating_sub(1);
        let (a, b) = (w.pos_of(s), w.pos_of(e));
        match name {
            "count" => Ok(Value::Long(w.doc.changes().iter().filter(|c| whole || (c.end > a && c.start < b)).count() as i32)),
            "acceptall" | "rejectall" => {
                let accept = name == "acceptall";
                if whole {
                    let sel = w.doc_sel(0, 0);
                    w.mutate_in(0, len, sel, |d, sel| {
                        d.resolve_all(accept, sel);
                        Some(sel.cursor)
                    })?;
                } else {
                    let sel = w.doc_sel(s, s);
                    w.mutate_in(s, e, sel, |d, sel| {
                        d.resolve_changes(a, b, accept, sel);
                        Some(sel.cursor)
                    })?;
                }
                Ok(Value::Empty)
            }
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::unsupported(&format!("Revisions.{name}"))),
        }
    }

    fn fields_call(&mut self, r: SpanRef, name: &str, a: &Args) -> R<Value> {
        let (s, e) = self.wspan(&Tg::Range(r));
        match name {
            "count" => Ok(Value::Long(self.w().fields_in(s, e) as i32)),
            // Fields are worked out as the document is shown and printed.
            "update" => Ok(Value::Long(0)),
            "add" => self.fields_add(a),
            "application" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::unsupported(&format!("Fields.{name}"))),
        }
    }
}

/// A custom property's value of type `ty` (MsoDocProperties: 1 number,
/// 2 Boolean, 3 date, 4 text).
fn prop_value(ty: i64, v: &Value) -> R<PropValue> {
    Ok(match ty {
        1 | 5 => PropValue::Number(v.to_f64()?),
        2 => PropValue::Bool(v.to_bool()?),
        3 => PropValue::Date(v.to_str()?),
        4 => {
            let t = v.to_str()?;
            if t.len() > 1 << 16 {
                return Err(Error::new(7, "Out of memory: the property is too long"));
            }
            PropValue::Text(t)
        }
        _ => return Err(Error::invalid_call()),
    })
}
