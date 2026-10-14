//! Workbooks a macro opens besides its own (`Workbooks.Open`, `Add`). The
//! object model works on one workbook at a time: an object of another one
//! is `Obj::In(book, object)`, and using it swaps that workbook in for the
//! call. What goes in is checked: an object of one workbook never reaches
//! another's members, so none can change the wrong workbook. Their code
//! never runs and no events fire in them.

use std::path::PathBuf;

use apex_sheet::{CellRef, Range, Workbook};

use super::{Args, obj};
use crate::ast::{Arg, Expr};
use crate::error::{Error, R};
use crate::interp::{Book, Interp, Owner};
use crate::object::{Obj, Part, RangeRef};
use crate::value::{Array, Value};

/// Other workbooks open at once.
const MAX_OPEN: usize = 16;
/// Workbooks opened or added in one run.
const MAX_OPENED: usize = 64;

/// What the object model keeps of a workbook while it works on another.
pub(crate) struct BookState<'a> {
    wb: Book<'a>,
    active: usize,
    selection: Range,
    undo: Vec<apex_sheet::Op>,
    owners: Vec<Owner>,
    save: bool,
    file_name: String,
    file_path: String,
    filtered: Vec<(usize, Range)>,
    active_chart: Option<(usize, u32)>,
    split: Option<(u32, u32)>,
    saved_as: Option<(String, usize)>,
    caller: Option<RangeRef>,
    requests: Vec<crate::Request>,
    /// Its file, once it has one.
    book: Option<PathBuf>,
    /// A document's (its workbook is an empty one).
    word: Option<crate::word::WordState<'a>>,
}

/// A workbook a macro opened and left open: the application shows it.
#[derive(Clone)]
pub struct OpenBook {
    pub wb: Workbook,
    pub name: String,
    /// Its file, if it has one.
    pub path: Option<String>,
}

impl std::fmt::Debug for OpenBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenBook").field("name", &self.name).field("path", &self.path).finish()
    }
}

impl BookState<'_> {
    fn new(wb: Workbook, file_name: String, file_path: String, book: Option<PathBuf>, modules: usize) -> BookState<'static> {
        BookState {
            wb: Book::Owned(Box::new(wb)),
            active: 0,
            selection: Range::single(CellRef::new(0, 0)),
            undo: Vec::new(),
            owners: vec![Owner::None; modules],
            save: false,
            file_name,
            file_path,
            filtered: Vec::new(),
            active_chart: None,
            split: None,
            saved_as: None,
            caller: None,
            requests: Vec::new(),
            book,
            word: None,
        }
    }
}

/// A document a macro opened and left open: the application shows it.
#[derive(Clone)]
pub struct OpenDoc {
    pub doc: apex_doc::Document,
    pub name: String,
    /// Its file, if it has one.
    pub path: Option<String>,
}

impl std::fmt::Debug for OpenDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenDoc").field("name", &self.name).field("path", &self.path).finish()
    }
}

/// Whether `o` belongs to a workbook (a sheet, a range...), and so says
/// which one when it is another's.
fn of_book(o: &Obj) -> bool {
    match o {
        Obj::Workbook | Obj::Sheets | Obj::Sheet(_) | Obj::Range(_) | Obj::Font(_) | Obj::Interior(_) | Obj::Border(..) | Obj::Window | Obj::Multi(_) => true,
        Obj::Word(w) => !matches!(**w, crate::word::WObj::Documents),
        Obj::Part(p) => !matches!(**p, Part::FileDialog(_) | Part::DialogFilters(_) | Part::DialogFilter(..) | Part::DialogItems(_) | Part::Fso | Part::TextStream(_) | Part::FsoFile(_) | Part::FsoFolder(_) | Part::FsoFiles(_) | Part::FsoFolders(_)),
        _ => false,
    }
}

/// `v`, got from workbook `b`, as the macro's own code holds it.
fn wrap(b: usize, v: Value) -> Value {
    match v {
        Value::Object(o) if of_book(&o) => Value::Object(Obj::In(b, Box::new(o))),
        Value::Object(Obj::Collection(c)) => {
            for item in c.borrow_mut().items.iter_mut() {
                let x = std::mem::take(&mut item.1);
                item.1 = wrap(b, x);
            }
            Value::Object(Obj::Collection(c))
        }
        Value::Array(a) if a.data.iter().any(Value::is_object) => {
            let mut a = Array::clone(&a);
            for x in a.data.iter_mut() {
                let v = std::mem::take(x);
                *x = wrap(b, v);
            }
            Value::Array(std::rc::Rc::new(a))
        }
        v => v,
    }
}

fn foreign() -> Error {
    Error::app("An object of one workbook can't be used in another's here: copy its values (dest.Value = src.Value) or use Range.Copy")
}

fn closed() -> Error {
    Error::new(1004, "The workbook was closed")
}

/// Whether `v` holds an object of some workbook (in an array or not).
fn holds_book_object(v: &Value) -> bool {
    match v {
        Value::Object(o) => of_book(o) || matches!(o, Obj::In(..)),
        Value::Array(a) => a.data.iter().any(holds_book_object),
        _ => false,
    }
}

impl<'a> Interp<'a> {
    fn swap_state(&mut self, s: &mut BookState<'a>) {
        use std::mem::swap;
        swap(&mut self.wb, &mut s.wb);
        swap(&mut self.active, &mut s.active);
        swap(&mut self.selection, &mut s.selection);
        swap(&mut self.undo, &mut s.undo);
        swap(&mut self.owners, &mut s.owners);
        swap(&mut self.save, &mut s.save);
        swap(&mut self.file_name, &mut s.file_name);
        swap(&mut self.file_path, &mut s.file_path);
        swap(&mut self.filtered, &mut s.filtered);
        swap(&mut self.active_chart, &mut s.active_chart);
        swap(&mut self.split, &mut s.split);
        swap(&mut self.saved_as, &mut s.saved_as);
        swap(&mut self.caller, &mut s.caller);
        swap(&mut self.requests, &mut s.requests);
        swap(self.files.book_mut(), &mut s.book);
        swap(&mut self.word, &mut s.word);
    }

    /// Runs `f` on workbook `b` (None: the macro's own) in place of the
    /// macro's own.
    pub(crate) fn in_book<T>(&mut self, b: Option<usize>, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        if self.current_book == b {
            return f(self);
        }
        let Some(b) = b else { return Err(foreign()) };
        if self.current_book.is_some() {
            return Err(foreign());
        }
        let mut s = self.books.get_mut(b).and_then(Option::take).ok_or_else(closed)?;
        self.swap_state(&mut s);
        self.current_book = Some(b);
        self.outer_book = s.book.clone();
        let out = f(self);
        self.current_book = None;
        self.outer_book = None;
        self.swap_state(&mut s);
        let stray = std::mem::take(&mut s.requests);
        self.books[b] = Some(s);
        if out.is_ok() && !stray.is_empty() {
            return Err(Error::unsupported("printing a workbook a macro opened (print it once it is open in its own tab)"));
        }
        out
    }

    /// `v` for a member of workbook `b`: its own objects unwrapped; those
    /// of other workbooks refused.
    fn arg_into(&self, b: usize, v: Value) -> R<Value> {
        match v {
            Value::Object(Obj::In(c, o)) if c == b => Ok(Value::Object(*o)),
            v if holds_book_object(&v) => Err(foreign()),
            v => Ok(v),
        }
    }

    /// `args` evaluated here, as values for a member of workbook `b`.
    fn lits(&mut self, b: usize, args: &[Arg], vals: Vec<Value>) -> R<Vec<Arg>> {
        let mut out = Vec::with_capacity(args.len());
        for (a, v) in args.iter().zip(vals) {
            let expr = match v {
                Value::Missing => None,
                v => Some(Expr::Lit(self.arg_into(b, v)?)),
            };
            out.push(Arg { name: a.name.clone(), expr });
        }
        Ok(out)
    }

    /// A value assigned to a property of workbook `b`: an object of
    /// another one gives its value (`dest = src`, without Set).
    fn value_into(&mut self, b: usize, v: Value) -> R<Value> {
        match v {
            Value::Object(Obj::In(c, o)) if c == b => Ok(Value::Object(*o)),
            v @ Value::Object(_) if holds_book_object(&v) => {
                let v = self.scalar(v)?;
                self.arg_into(b, v)
            }
            v => self.arg_into(b, v),
        }
    }

    /// `f` on workbook `b` with `args`, its result as held here.
    fn call_in(&mut self, b: usize, args: &[Arg], f: impl FnOnce(&mut Self, &[Arg]) -> R<Value>) -> R<Value> {
        let vals = self.arg_raw(args)?;
        let lits = self.lits(b, args, vals)?;
        let v = self.in_book(Some(b), |it| f(it, &lits))?;
        Ok(wrap(b, v))
    }

    /// Member `name` of `o`, an object of workbook `b`.
    pub(crate) fn member_in(&mut self, b: usize, o: Obj, name: &str, args: &[Arg]) -> R<Value> {
        let vals = self.arg_raw(args)?;
        let a = Args(args.iter().zip(vals.iter().cloned()).map(|(a, v)| (a.name.clone(), v)).collect());
        let doc = matches!(&o, Obj::Word(w) if matches!(**w, crate::word::WObj::Doc));
        match (&o, name) {
            (_, "close") if doc => return self.close_doc(b, &a),
            (_, "save") if doc => {
                self.in_book(Some(b), |it| it.save_own_doc())?;
                return Ok(Value::Empty);
            }
            (_, "activate") if doc => {
                self.in_book(Some(b), |_| Ok(()))?;
                self.active_book = Some(b);
                return Ok(Value::Empty);
            }
            (Obj::Workbook, "close") => return self.close_book(b, &a),
            (Obj::Workbook, "save") => {
                self.in_book(Some(b), |it| it.save_own())?;
                return Ok(Value::Empty);
            }
            (Obj::Workbook, "activate") => {
                self.in_book(Some(b), |_| Ok(()))?;
                self.active_book = Some(b);
                return Ok(Value::Empty);
            }
            (Obj::Range(_), "pastespecial") | (Obj::Sheet(_), "paste") if self.clip.is_some() && self.clip_in != Some(b) => {
                let (src, cut) = self.clip.expect("checked");
                if cut {
                    return Err(Error::unsupported("Cut to another workbook (Copy, then ClearContents)"));
                }
                let clip = self.in_book(self.clip_in, |it| it.copy_of(src))?;
                self.carried = Some(clip);
            }
            (Obj::Range(r), "copy") if let Some(dest) = a.get(0, "destination") && !matches!(&dest, Value::Object(Obj::In(c, _)) if *c == b) => {
                self.copy_across(Some(b), *r, dest)?;
                return Ok(Value::Bool(true));
            }
            _ => {}
        }
        let lits = self.lits(b, args, vals);
        let v = lits.and_then(|lits| self.in_book(Some(b), |it| it.member(Value::Object(o), name, &lits)));
        self.carried = None;
        Ok(wrap(b, v?))
    }

    pub(crate) fn set_member_in(&mut self, b: usize, o: Obj, name: &str, args: &[Arg], v: Value) -> R<()> {
        let vals = self.arg_raw(args)?;
        let lits = self.lits(b, args, vals)?;
        let v = self.value_into(b, v)?;
        self.in_book(Some(b), |it| it.set_member(Value::Object(o), name, &lits, v))
    }

    pub(crate) fn set_default_in(&mut self, b: usize, o: Obj, v: Value) -> R<()> {
        let v = self.value_into(b, v)?;
        self.in_book(Some(b), |it| it.set_default(Value::Object(o), v))
    }

    pub(crate) fn default_in(&mut self, b: usize, o: Obj) -> R<Value> {
        let v = self.in_book(Some(b), |it| it.default_value(o))?;
        Ok(wrap(b, v))
    }

    pub(crate) fn items_in(&mut self, b: usize, o: Obj) -> R<Box<dyn Iterator<Item = R<Value>>>> {
        let items = self.in_book(Some(b), |it| it.items_of(Value::Object(o)))?;
        Ok(Box::new(items.map(move |v| v.map(|v| wrap(b, v)))))
    }

    /// Names that mean the active workbook's things (`Range`,
    /// `ActiveSheet`...): another workbook's after Workbooks.Open or
    /// Activate, except in a sheet's own code.
    pub(crate) fn global_elsewhere(&mut self, n: &str, args: &[Arg]) -> Option<R<Value>> {
        let b = self.active_book?;
        if self.current_book.is_some() || matches!(self.owners.get(self.module()), Some(Owner::Sheet(_))) {
            return None;
        }
        let routed = matches!(n, "activeworkbook" | "names" | "worksheets" | "sheets" | "activesheet" | "activewindow" | "activechart" | "activecell" | "selection" | "range" | "cells" | "rows" | "columns" | "usedrange" | "evaluate") || n.starts_with('[');
        if !routed {
            return None;
        }
        Some(self.call_in(b, args, |it, a| it.global(n, a).unwrap_or_else(|| Err(Error::no_member(n)))))
    }

    /// The workbook activated: the macro's own (None) or another one.
    pub(crate) fn activate_book(&mut self) {
        self.active_book = self.current_book;
    }

    fn open_count(&self) -> usize {
        self.books.iter().flatten().count()
    }

    /// The files of the other workbooks open (the macro's own among them
    /// when another one is swapped in), but not the current one's.
    pub(crate) fn other_book_files(&self) -> Vec<PathBuf> {
        self.books.iter().flatten().filter_map(|s| s.book.clone()).chain(self.outer_book.clone()).collect()
    }

    // --- Workbooks ----------------------------------------------------------------------

    pub(crate) fn workbooks_member(&mut self, name: &str, args: &[Arg]) -> R<Value> {
        if self.current_book.is_some() {
            return Err(foreign());
        }
        let a = self.args(args)?;
        match name {
            "count" => Ok(Value::Long(1 + self.open_count() as i32)),
            "item" | "_default" => {
                let k = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let k = self.scalar(k)?;
                self.workbook_item(k)
            }
            "open" => self.open_book(&a),
            "add" => {
                if a.get(0, "template").is_some() {
                    return Err(Error::unsupported("Workbooks.Add with a template (open the template with Workbooks.Open)"));
                }
                self.no_udf()?;
                let name = format!("Book{}", self.books.len() + 2);
                self.push_book(Workbook::new(), name, String::new(), None)
            }
            "close" => Err(Error::unsupported("Workbooks.Close (close each one: wb.Close SaveChanges:=False)")),
            "application" | "parent" => Ok(obj(Obj::Application)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn workbook_item(&mut self, k: Value) -> R<Value> {
        let open: Vec<usize> = (0..self.books.len()).filter(|&b| self.books[b].is_some()).collect();
        if let Value::Str(_) = k {
            let n = k.to_str()?;
            let same = |file: &str| file.eq_ignore_ascii_case(&n) || std::path::Path::new(file).file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&n));
            if same(&self.file_name) {
                return Ok(obj(Obj::Workbook));
            }
            if let Some(&b) = open.iter().find(|&&b| self.books[b].as_ref().is_some_and(|s| same(&s.file_name))) {
                return Ok(obj(Obj::In(b, Box::new(Obj::Workbook))));
            }
            // A workbook never saved may not know its name.
            if self.file_name.is_empty() {
                return Ok(obj(Obj::Workbook));
            }
            return Err(Error::subscript());
        }
        match k.to_i64()? {
            1 => Ok(obj(Obj::Workbook)),
            i if i >= 2 && ((i - 2) as usize) < open.len() => Ok(obj(Obj::In(open[(i - 2) as usize], Box::new(Obj::Workbook)))),
            _ => Err(Error::subscript()),
        }
    }

    /// Workbooks.Open: the file read (where macros may read), as a
    /// workbook of its own; the active one from now on.
    fn open_book(&mut self, a: &Args) -> R<Value> {
        self.no_udf()?;
        let file = self.arg_str(a, 0, "filename")?.ok_or_else(|| Error::new(449, "Argument not optional"))?;
        let read_only = match a.get(2, "readonly") {
            Some(v) => self.scalar(v)?.to_bool()?,
            None => false,
        };
        let password = self.arg_str(a, 4, "password")?.filter(|p| !p.is_empty());
        let p = self.path_to_read(&file)?;
        if self.files.book_mut().as_ref() == Some(&p) {
            self.active_book = None;
            return Ok(obj(Obj::Workbook));
        }
        if let Some(b) = (0..self.books.len()).find(|&b| self.books[b].as_ref().is_some_and(|s| s.book.as_ref() == Some(&p))) {
            self.active_book = Some(b);
            return Ok(obj(Obj::In(b, Box::new(Obj::Workbook))));
        }
        if self.open_count() >= MAX_OPEN || self.books.len() >= MAX_OPENED {
            return Err(Error::app(format!("A macro can have up to {MAX_OPEN} other workbooks open")));
        }
        if !p.is_file() {
            return Err(Error::app(format!("'{file}' could not be found")));
        }
        let path = p.display().to_string();
        let mut wb = self.ui.open_workbook(&path, password.as_deref()).map_err(|e| Error::app(format!("'{file}' couldn't be opened: {e}")))?;
        wb.read_only |= read_only;
        wb.modified = false;
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let folder = p.parent().map(|d| d.display().to_string()).unwrap_or_default();
        self.push_book(wb, name, folder, Some(p))
    }

    fn push_book(&mut self, wb: Workbook, name: String, folder: String, book: Option<PathBuf>) -> R<Value> {
        if self.open_count() >= MAX_OPEN || self.books.len() >= MAX_OPENED {
            return Err(Error::app(format!("A macro can have up to {MAX_OPEN} other workbooks open")));
        }
        let b = self.books.len();
        self.books.push(Some(BookState::new(wb, name, folder, book, self.project.modules.len())));
        self.active_book = Some(b);
        Ok(obj(Obj::In(b, Box::new(Obj::Workbook))))
    }

    /// Workbook.Save of a workbook the macro opened (swapped in).
    fn save_own(&mut self) -> R<()> {
        if self.file_path.is_empty() {
            return Err(Error::app(format!("'{}' has no file yet: use SaveAs", self.file_name)));
        }
        if self.wb.read_only {
            return Err(Error::app(format!("'{}' is read-only: use SaveAs", self.file_name)));
        }
        let full = std::path::Path::new(&self.file_path).join(&self.file_name).display().to_string();
        self.save_book(&full, None, true)
    }

    /// Workbook.Close of workbook `b` (SaveChanges, Filename).
    fn close_book(&mut self, b: usize, a: &Args) -> R<Value> {
        let save = match a.get(0, "savechanges") {
            Some(v) => Some(self.scalar(v)?.to_bool()?),
            None => None,
        };
        let file = self.arg_str(a, 1, "filename")?.filter(|f| !f.is_empty());
        let (modified, name) = self.in_book(Some(b), |it| Ok((it.wb.modified, it.file_name.clone())))?;
        let save = match save {
            Some(s) => s,
            None if !modified || !self.app.display_alerts => false,
            None => match self.paused(|ui| ui.msg_box(&format!("Do you want to save the changes you made to '{name}'?"), 3 + 48, "ApexDesk")) {
                6 => true,
                7 => false,
                _ => return Ok(Value::Empty),
            },
        };
        if save {
            self.in_book(Some(b), |it| match &file {
                Some(f) => it.save_book(f, None, true),
                None => it.save_own(),
            })?;
        }
        self.books[b] = None;
        if self.active_book == Some(b) {
            self.active_book = None;
        }
        Ok(Value::Empty)
    }

    /// The workbooks still open when the macro ends.
    pub(crate) fn take_books(&mut self) -> Vec<OpenBook> {
        let mut out = Vec::new();
        for s in self.books.iter_mut().filter(|s| s.as_ref().is_some_and(|s| s.word.is_none())) {
            let Some(s) = s.take() else { continue };
            if let Book::Owned(wb) = s.wb {
                let path = s.book.map(|p| p.display().to_string());
                out.push(OpenBook { wb: *wb, name: s.file_name, path });
            }
        }
        out
    }

    // --- documents ------------------------------------------------------------------------

    /// The documents a macro opened, with their numbers.
    fn open_docs(&self) -> impl Iterator<Item = (usize, &BookState<'a>)> {
        self.books.iter().enumerate().filter_map(|(b, s)| s.as_ref().filter(|s| s.word.is_some()).map(|s| (b, s)))
    }

    /// Documents.Count.
    pub(crate) fn docs_count(&self) -> usize {
        1 + self.open_docs().count()
    }

    /// Documents(i) or Documents("name"): the macro's own is the first.
    pub(crate) fn doc_item(&mut self, k: Value) -> R<Value> {
        let open: Vec<usize> = self.open_docs().map(|(b, _)| b).collect();
        let doc = |b: usize| crate::word::wo_in(b, crate::word::WObj::Doc);
        if let Value::Str(_) = k {
            let n = k.to_str()?;
            let same = |f: &str| f.eq_ignore_ascii_case(&n) || std::path::Path::new(f).file_stem().is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case(&n));
            if self.file_name.is_empty() && open.is_empty() || same(&self.file_name) || self.file_name.is_empty() && same("Document1") {
                return Ok(crate::word::wo(crate::word::WObj::Doc));
            }
            if let Some(&b) = open.iter().find(|&&b| self.books[b].as_ref().is_some_and(|s| same(&s.file_name))) {
                return Ok(doc(b));
            }
            return Err(Error::new(4160, "Bad file name"));
        }
        match k.to_i64()? {
            1 => Ok(crate::word::wo(crate::word::WObj::Doc)),
            i if i >= 2 && ((i - 2) as usize) < open.len() => Ok(doc(open[(i - 2) as usize])),
            _ => Err(Error::new(5941, "The requested member of the collection does not exist")),
        }
    }

    /// For Each over Documents.
    pub(crate) fn all_docs(&self) -> Vec<Value> {
        let mut all = vec![crate::word::wo(crate::word::WObj::Doc)];
        all.extend(self.open_docs().map(|(b, _)| crate::word::wo_in(b, crate::word::WObj::Doc)));
        all
    }

    /// Documents.Open: the file read (where macros may read), as a document
    /// of its own; the active one from now on. Its macros never run.
    pub(crate) fn open_doc(&mut self, a: &Args) -> R<Value> {
        let file = self.arg_str(a, 0, "filename")?.ok_or_else(|| Error::new(449, "Argument not optional"))?;
        let read_only = match a.get(2, "readonly") {
            Some(v) => self.scalar(v)?.to_bool()?,
            None => false,
        };
        let p = self.path_to_read(&file)?;
        if self.files.book_mut().as_ref() == Some(&p) {
            self.active_book = None;
            return Ok(crate::word::wo(crate::word::WObj::Doc));
        }
        let found = self.open_docs().find(|(_, s)| s.book.as_ref() == Some(&p)).map(|(b, _)| b);
        if let Some(b) = found {
            self.active_book = Some(b);
            return Ok(crate::word::wo_in(b, crate::word::WObj::Doc));
        }
        self.room_for_doc()?;
        if !p.is_file() {
            return Err(Error::new(5174, format!("'{file}' could not be found")));
        }
        let doc = self.read_doc(&p, &file)?;
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let folder = p.parent().map(|d| d.display().to_string()).unwrap_or_default();
        let b = self.push_doc(doc, name, folder, Some(p), read_only)?;
        Ok(crate::word::wo_in(b, crate::word::WObj::Doc))
    }

    /// Documents.Add([Template]): a new document, empty or with the
    /// template's content (where macros may read).
    pub(crate) fn add_doc(&mut self, a: &Args) -> R<Value> {
        self.room_for_doc()?;
        let doc = match self.arg_str(a, 0, "template")?.filter(|t| !t.is_empty()) {
            Some(t) => {
                let p = self.path_to_read(&t)?;
                if !p.is_file() {
                    return Err(Error::new(5151, format!("'{t}' could not be found")));
                }
                let mut d = self.read_doc(&p, &t)?;
                // A copy of the template: no file, and none of its macros.
                d.macros = Default::default();
                d.modified = false;
                d
            }
            None => apex_doc::Document::new(),
        };
        let name = format!("Document{}", self.books.len() + 2);
        let b = self.push_doc(doc, name, String::new(), None, false)?;
        Ok(crate::word::wo_in(b, crate::word::WObj::Doc))
    }

    fn room_for_doc(&self) -> R<()> {
        if self.open_count() >= MAX_OPEN || self.books.len() >= MAX_OPENED {
            return Err(Error::app(format!("A macro can have up to {MAX_OPEN} other documents open")));
        }
        Ok(())
    }

    fn read_doc(&mut self, p: &std::path::Path, file: &str) -> R<apex_doc::Document> {
        let path = p.display().to_string();
        let t = std::time::Instant::now();
        let res = self.ui.open_document(&path);
        self.paused += t.elapsed();
        let mut doc = res.map_err(|e| Error::new(5174, format!("'{file}' couldn't be opened: {e}")))?;
        doc.modified = false;
        Ok(doc)
    }

    pub(crate) fn push_doc(&mut self, doc: apex_doc::Document, name: String, folder: String, book: Option<PathBuf>, read_only: bool) -> R<usize> {
        self.room_for_doc()?;
        let b = self.books.len();
        // Its workbook only says whether it is read-only.
        let mut wb = Workbook::new();
        wb.read_only = read_only;
        let mut s = BookState::new(wb, name, folder, book, self.project.modules.len());
        s.word = Some(crate::word::WordState::of(crate::word::DocRef::Owned(Box::new(doc)), apex_doc::Selection::default()));
        self.books.push(Some(s));
        self.active_book = Some(b);
        Ok(b)
    }

    /// Document.Save of a document the macro opened (swapped in).
    fn save_own_doc(&mut self) -> R<()> {
        if self.file_path.is_empty() {
            return Err(Error::app(format!("'{}' has no file yet: use SaveAs", self.file_name)));
        }
        if self.wb.read_only {
            return Err(Error::app(format!("'{}' is read-only: use SaveAs", self.file_name)));
        }
        let full = std::path::Path::new(&self.file_path).join(&self.file_name).display().to_string();
        self.save_doc(&full, None, true)
    }

    /// Document.Close of document `b` (SaveChanges, , RouteDocument).
    fn close_doc(&mut self, b: usize, a: &Args) -> R<Value> {
        let save = match a.get(0, "savechanges") {
            Some(v) => match self.scalar(v)?.to_i64()? {
                -2 => None,
                0 => Some(false),
                _ => Some(true),
            },
            None => None,
        };
        let (modified, name) = self.in_book(Some(b), |it| Ok((it.w().doc.modified, it.file_name.clone())))?;
        let save = match save {
            Some(s) => s,
            None if !modified || !self.app.display_alerts => false,
            None => match self.paused(|ui| ui.msg_box(&format!("Do you want to save the changes you made to '{name}'?"), 3 + 48, "ApexDesk")) {
                6 => true,
                7 => false,
                _ => return Ok(Value::Empty),
            },
        };
        if save {
            self.in_book(Some(b), |it| it.save_own_doc())?;
        }
        self.books[b] = None;
        if self.active_book == Some(b) {
            self.active_book = None;
        }
        Ok(Value::Empty)
    }

    /// The documents still open when the macro ends.
    pub(crate) fn take_docs(&mut self) -> Vec<OpenDoc> {
        let mut out = Vec::new();
        for s in self.books.drain(..).flatten() {
            let path = s.book.map(|p| p.display().to_string());
            if let Some(w) = s.word
                && let crate::word::DocRef::Owned(doc) = w.doc
            {
                out.push(OpenDoc { doc: *doc, name: s.file_name, path });
            }
        }
        out
    }

    // --- copying between workbooks --------------------------------------------------------

    /// Range.Copy from `src` of workbook `from` (None: the macro's own) to
    /// `dest`, a range of another, as a paste there (formats taken over,
    /// references to other sheets by name).
    pub(crate) fn copy_across(&mut self, from: Option<usize>, src: RangeRef, dest: Value) -> R<()> {
        let (to, dest) = match dest {
            Value::Object(Obj::In(b, o)) => (Some(b), *o),
            Value::Object(o) if self.current_book.is_none() => (None, o),
            Value::Nothing => return Err(Error::not_set()),
            _ => return Err(Error::object_required()),
        };
        let Obj::Range(dest) = dest else { return Err(Error::new(13, "Type mismatch: the destination must be a range")) };
        let clip = self.in_book(from, |it| it.copy_of(src))?;
        self.in_book(to, |it| it.paste_clip(&clip, dest))
    }

    /// The cells of `src`, to paste in another workbook.
    fn copy_of(&self, src: RangeRef) -> R<Clip> {
        Self::check_size(&src.range)?;
        if src.sheet >= self.wb.sheets.len() {
            return Err(Error::subscript());
        }
        Ok(self.wb.copy(src.sheet, &src.range))
    }

    /// `clip`, copied in another workbook, pasted at `dest`.
    fn paste_clip(&mut self, clip: &Clip, dest: RangeRef) -> R<()> {
        let clip = self.wb_mut()?.adopt_clip(clip);
        self.paste_clip_adopted(&clip, dest)
    }

    /// `clip`, as cells of this workbook, pasted at `dest`.
    pub(crate) fn paste_clip_adopted(&mut self, clip: &Clip, dest: RangeRef) -> R<()> {
        if dest.sheet >= self.wb.sheets.len() {
            return Err(Error::subscript());
        }
        let (r0, c0) = (dest.range.start.row as i64, dest.range.start.col as i64);
        super::rng(r0, c0, r0 + clip.rows as i64 - 1, c0 + clip.cols as i64 - 1)?;
        self.via(|wb| wb.paste(dest.sheet, &Range::single(dest.range.start), clip))
    }

    /// What Copy put aside, from whichever workbook it is in, as cells of
    /// the current one (formats taken over when it is another's).
    pub(crate) fn clip_cells(&mut self) -> R<Clip> {
        let Some((src, _)) = self.clip else { return Err(Error::app("There is nothing copied to paste")) };
        if self.clip_in == self.current_book {
            return self.copy_of(src);
        }
        let clip = match self.carried.take() {
            Some(c) => c,
            None if self.current_book.is_none() => self.in_book(self.clip_in, |it| it.copy_of(src))?,
            None => return Err(foreign()),
        };
        Ok(self.wb_mut()?.adopt_clip(&clip))
    }
}

/// Cells copied, as the workbook keeps them.
type Clip = apex_sheet::workbook::Clip;
