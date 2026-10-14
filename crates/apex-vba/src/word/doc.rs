//! The Document, Documents and Word's Application, the names usable
//! without a qualifier, and where a member of each Word object goes.

use super::{Tg, WObj, wo};
use crate::ast::Arg;
use crate::error::{Error, R};
use crate::excel::Args;
use crate::interp::Interp;
use crate::object::Obj;
use crate::value::Value;

/// What the dialogs and windows of Word that macros can't reach are.
const UNSUPPORTED: [&str; 12] = ["wordbasic", "normaltemplate", "activewindow", "windows", "dialogs", "commandbars", "keybindings", "options", "addins", "templates", "tasks", "recentfiles"];

/// What Word's Application shares with Excel's.
const SHARED: [&str; 14] = ["name", "version", "username", "pathseparator", "operatingsystem", "screenupdating", "displayalerts", "statusbar", "run", "filedialog", "wait", "doevents", "visible", "interactive"];

/// Longest password Protect and Unprotect take.
const MAX_PASSWORD: usize = 255;

/// WdUnits conversions: points in one of each.
fn unit_points(name: &str) -> Option<(f64, bool)> {
    Some(match name {
        "centimeterstopoints" => (72.0 / 2.54, true),
        "inchestopoints" => (72.0, true),
        "millimeterstopoints" => (72.0 / 25.4, true),
        "linestopoints" | "picastopoints" => (12.0, true),
        "pointstocentimeters" => (72.0 / 2.54, false),
        "pointstoinches" => (72.0, false),
        "pointstomillimeters" => (72.0 / 25.4, false),
        "pointstolines" | "pointstopicas" => (12.0, false),
        _ => return None,
    })
}

impl Interp<'_> {
    /// Names of Word's object model usable without a qualifier.
    pub(crate) fn word_global(&mut self, n: &str, args: &[Arg]) -> Option<R<Value>> {
        let r = match n {
            "application" | "word" => Ok(Value::Object(Obj::Application)),
            // After Documents.Open or Add, or Activate: that document.
            "activedocument" | "selection" if self.current_book.is_none() && let Some(b) = self.active_book => {
                let o = if n == "selection" { WObj::Sel } else { WObj::Doc };
                Ok(crate::word::wo_in(b, o))
            }
            "activedocument" | "thisdocument" => Ok(wo(WObj::Doc)),
            "selection" => Ok(wo(WObj::Sel)),
            "documents" => {
                if args.is_empty() {
                    Ok(wo(WObj::Documents))
                } else {
                    return Some(self.word_member(WObj::Documents, "item", args));
                }
            }
            _ if unit_points(n).is_some() => {
                let (k, to) = unit_points(n).expect("checked");
                let a = match self.args(args) {
                    Ok(a) => a,
                    Err(e) => return Some(Err(e)),
                };
                let x = match a.get(0, "").map(|v| self.scalar(v).and_then(|v| v.to_f64())) {
                    Some(Ok(x)) => x,
                    Some(Err(e)) => return Some(Err(e)),
                    None => return Some(Err(Error::invalid_call())),
                };
                Ok(Value::Double(if to { x * k } else { x / k }))
            }
            _ if UNSUPPORTED.contains(&n) => Err(Error::unsupported(n)),
            _ => return None,
        };
        Some(r)
    }

    /// Application, in a Word macro.
    pub(crate) fn word_app(&mut self, name: &str, args: &[Arg]) -> R<Value> {
        if let Some(r) = self.word_global(name, args) {
            return r;
        }
        match name {
            "parent" => Ok(Value::Object(Obj::Application)),
            "quit" => Err(Error::unsupported("Application.Quit (macros don't close the program)")),
            "statusbar" | "displayalerts" | "screenupdating" if !args.is_empty() => Err(Error::invalid_call()),
            _ if SHARED.contains(&name) => self.app_member(name, args),
            _ => Err(Error::no_member(name)),
        }
    }

    /// `o.name(args)` of a Word object.
    pub(crate) fn word_member(&mut self, o: WObj, name: &str, args: &[Arg]) -> R<Value> {
        let a = self.args(args)?;
        self.word_call(o, name, &a)
    }

    pub(crate) fn word_call(&mut self, o: WObj, name: &str, a: &Args) -> R<Value> {
        match o {
            WObj::Doc => self.doc_call(name, a),
            WObj::Documents => self.documents_call(name, a),
            WObj::Sel => self.sel_call(name, a),
            WObj::Range(r) => self.text_call(Tg::Range(r), name, a),
            WObj::Para(r) => self.para_call(r, name, a),
            WObj::Units(r, u) => self.units_call(r, u, name, a),
            WObj::Font(tg) => self.wfont_get(tg, name),
            WObj::ParaFormat(tg) => self.pformat_get(tg, name),
            WObj::Find(tg) => self.find_member(tg, false, name, a),
            WObj::Replacement(tg) => self.find_member(tg, true, name, a),
            WObj::FindFont(tg, repl) => self.find_font_get(tg, repl, name),
            WObj::ListFormat(tg) => self.list_call(tg, name, a),
            WObj::Fragment(f) => match name {
                "text" | "_default" => {
                    let mut t = String::new();
                    for (i, p) in f.iter().enumerate() {
                        if i > 0 {
                            t.push('\r');
                        }
                        super::push_text(&mut t, &p.text);
                    }
                    Ok(Value::str(t))
                }
                "application" => Ok(Value::Object(Obj::Application)),
                _ => Err(Error::unsupported(&format!("{name} of FormattedText (put it in a Range first)"))),
            },
            o => self.wpart_call(o, name, a),
        }
    }

    /// `o.name(args) = v` of a Word object.
    pub(crate) fn word_set(&mut self, o: WObj, name: &str, args: &[Arg], v: Value) -> R<()> {
        // `Doc.BuiltInDocumentProperties("Title") = v`: the default
        // property of what it gives.
        if !args.is_empty() {
            let t = self.word_member(o, name, args)?;
            if !t.is_object() {
                return Err(Error::no_member(name));
            }
            return self.set_default(t, v);
        }
        match o {
            WObj::Doc => self.doc_set(name, v),
            WObj::Sel => self.sel_set(name, v),
            WObj::Range(r) => self.text_set(Tg::Range(r), name, v),
            WObj::Para(r) => self.para_set(r, name, v),
            WObj::Font(tg) => self.wfont_set(tg, name, v),
            WObj::ParaFormat(tg) => self.pformat_set(tg, name, v),
            WObj::Find(tg) => self.find_set(tg, false, name, v),
            WObj::Replacement(tg) => self.find_set(tg, true, name, v),
            WObj::FindFont(tg, repl) => self.find_font_set(tg, repl, name, v),
            WObj::ListFormat(tg) => self.list_set(tg, name, v),
            WObj::Units(r, super::Unit::Para) => self.pformat_set(Tg::Range(r), name, v),
            o @ (WObj::Documents | WObj::Units(..) | WObj::Fragment(_)) => Err(Error::no_member(&format!("{name} of {}", o.type_name()))),
            o => self.wpart_set(o, name, v),
        }
    }

    /// The default property of a Word object.
    pub(crate) fn word_default(&mut self, o: WObj) -> R<Value> {
        match o {
            WObj::Sel | WObj::Range(_) | WObj::Para(_) => {
                let tg = match o {
                    WObj::Sel => Tg::Sel,
                    WObj::Range(r) | WObj::Para(r) => Tg::Range(r),
                    _ => unreachable!(),
                };
                self.text_call(tg, "text", &Args::default())
            }
            o @ (WObj::Fragment(_) | WObj::CommentText(_) | WObj::HfRange(..) | WObj::Style(_) | WObj::Var(_) | WObj::ShapeRange(_) | WObj::Bookmark(_) | WObj::Prop(..) | WObj::Hyperlink(_) | WObj::Entry(..) | WObj::MergeName(..)) => self.word_call(o, "_default", &Args::default()),
            WObj::Doc => self.doc_call("name", &Args::default()),
            o => Err(Error::no_member(&format!("default property of {}", o.type_name()))),
        }
    }

    pub(crate) fn word_set_default(&mut self, o: WObj, v: Value) -> R<()> {
        match o {
            o @ (WObj::Sel | WObj::Range(_) | WObj::Para(_) | WObj::CommentText(_) | WObj::HfRange(..)) => self.word_set(o, "text", &[], v),
            o @ (WObj::Prop(..) | WObj::Var(_)) => self.word_set(o, "value", &[], v),
            o @ WObj::ShapeRange(_) => self.word_set(o, "text", &[], v),
            o => Err(Error::no_member(&format!("default property of {}", o.type_name()))),
        }
    }

    /// For Each over a Word collection.
    pub(crate) fn word_items(&mut self, o: WObj) -> R<Vec<Value>> {
        match o {
            WObj::Documents => Ok(self.all_docs()),
            WObj::Units(r, u) => self.units_items(&r, u),
            o => match self.wpart_items(&o)? {
                Some(v) => Ok(v),
                None => Err(Error::new(92, "For Each may only iterate over a collection object or an array")),
            },
        }
    }

    // --- Document ------------------------------------------------------------------------------

    /// The whole text as a Range of its own.
    fn whole(&mut self) -> super::SpanRef {
        let w = self.w();
        let len = w.story_len();
        w.new_span(0, len)
    }

    fn doc_call(&mut self, name: &str, a: &Args) -> R<Value> {
        Ok(match name {
            "name" | "_default" => Value::str(if self.file_name.is_empty() { "Document1".to_string() } else { self.file_name.clone() }),
            "fullname" => {
                let n = if self.file_name.is_empty() { "Document1".to_string() } else { self.file_name.clone() };
                Value::str(if self.file_path.is_empty() { n } else { std::path::Path::new(&self.file_path).join(n).display().to_string() })
            }
            "path" => Value::str(self.file_path.clone()),
            "content" => {
                let r = self.whole();
                wo(WObj::Range(r))
            }
            "range" => {
                let len = self.w().story_len() as i64;
                let s = self.arg_i64(a, 0, "start")?.unwrap_or(0).clamp(0, len) as usize;
                let e = self.arg_i64(a, 1, "end")?.unwrap_or(len).clamp(0, len) as usize;
                self.new_range(s.min(e), s.max(e))
            }
            "paragraphs" | "words" | "sentences" | "characters" | "tables" | "hyperlinks" | "inlineshapes" | "revisions" | "fields" => {
                let r = self.whole();
                return self.text_call(Tg::Range(r), name, a);
            }
            "goto" => {
                let r = self.w().new_span(0, 0);
                return self.text_call(Tg::Range(r), name, a);
            }
            "contentcontrols" => {
                let r = self.whole();
                return self.collection(WObj::Controls(r, None), a);
            }
            "selectcontentcontrolsbytitle" | "selectcontentcontrolsbytag" => {
                let v = self.arg_str(a, 0, "title")?.or(self.arg_str(a, 0, "tag")?).ok_or_else(Error::invalid_call)?;
                let r = self.whole();
                return Ok(wo(WObj::Controls(r, Some((name.ends_with("tag"), std::rc::Rc::from(v.as_str()))))));
            }
            "mailmerge" => wo(WObj::MailMerge),
            "bookmarks" => return self.collection(WObj::Bookmarks, a),
            "comments" => return self.collection(WObj::Comments, a),
            "styles" => return self.collection(WObj::Styles, a),
            "variables" => return self.collection(WObj::Vars, a),
            "shapes" => return self.collection(WObj::Shapes, a),
            "tablesofcontents" => return self.collection(WObj::Tocs, a),
            "sections" => return self.collection(WObj::Sections, a),
            "builtindocumentproperties" => return self.collection(WObj::Props(true), a),
            "customdocumentproperties" => return self.collection(WObj::Props(false), a),
            "pagesetup" => wo(WObj::PageSetup),
            "trackrevisions" => Value::Bool(self.w().doc.track_changes),
            "saved" => Value::Bool(!self.w().doc.modified),
            "readonly" => Value::Bool(self.wb.read_only),
            "type" => Value::Long(0),
            "save" => {
                self.save = true;
                Value::Empty
            }
            "saveas" | "saveas2" => {
                let file = self.arg_str(a, 0, "filename")?.ok_or_else(Error::invalid_call)?;
                let format = self.arg_i64(a, 1, "fileformat")?;
                self.save_doc(&file, format, true)?;
                Value::Empty
            }
            "close" => {
                let save = match self.arg_i64(a, 0, "savechanges")? {
                    None | Some(-2) => None,
                    Some(0) => Some(false),
                    Some(_) => Some(true),
                };
                self.queue_request(crate::Request::Close { save })?;
                // Closing the document ends its macro.
                return Err(Error::new(crate::interp::END, "End").fatal());
            }
            "printout" | "printpreview" => return self.print_request(None),
            "protectiontype" => Value::Long(match self.w().doc.edit_limit() {
                None => -1,
                Some(apex_doc::EditLimit::TrackedChanges) => 0,
                Some(apex_doc::EditLimit::Comments) => 1,
                Some(apex_doc::EditLimit::Forms) => 2,
                Some(apex_doc::EditLimit::ReadOnly) => 3,
            }),
            "computestatistics" => {
                let what = self.arg_i64(a, 0, "statistic")?.ok_or_else(Error::invalid_call)?;
                let len = self.w().story_len();
                return self.statistics(0, len, what);
            }
            "acceptallrevisions" | "rejectallrevisions" => {
                let r = self.whole();
                return self.wpart_call(WObj::Revisions(r), if name == "acceptallrevisions" { "acceptall" } else { "rejectall" }, a);
            }
            "deleteallcomments" => {
                let w = self.w();
                let ids: Vec<u32> = w.doc.comments.iter().map(|c| c.id).collect();
                if !ids.is_empty() {
                    w.side_edit(|d, sel| {
                        d.delete_comments(&ids, sel);
                    })?;
                }
                Value::Empty
            }
            "select" => {
                let w = self.w();
                let len = w.story_len();
                w.anchor = 0;
                w.active = len;
                Value::Empty
            }
            "activate" => {
                self.activate_book();
                Value::Empty
            }
            "repaginate" | "updatestyles" => Value::Empty,
            "application" | "parent" => Value::Object(Obj::Application),
            "protect" => {
                let ty = self.arg_i64(a, 0, "type")?.ok_or_else(Error::invalid_call)?;
                let pw = self.arg_str(a, 2, "password")?.unwrap_or_default();
                self.protect_doc(ty, &pw)?;
                Value::Empty
            }
            "unprotect" => {
                let pw = self.arg_str(a, 0, "password")?.unwrap_or_default();
                self.unprotect_doc(&pw)?;
                Value::Empty
            }
            "undo" | "redo" => {
                let times = self.arg_i64(a, 0, "times")?.unwrap_or(1);
                return self.undo_doc(name == "redo", times);
            }
            "undoclear" => {
                self.w().doc.trim_undo(0);
                Value::Empty
            }
            "vbproject" | "storyranges" | "listparagraphs" | "windows" | "activewindow" | "attachedtemplate" | "exportasfixedformat" | "sendmail" | "followhyperlink" => {
                return Err(Error::unsupported(&format!("Document.{name}")));
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    /// Document.Protect: Restrict Editing with `limit` (WdProtectionType)
    /// and the password that stops it, if any.
    fn protect_doc(&mut self, ty: i64, pw: &str) -> R<()> {
        use apex_doc::{EditLimit, Protection, ProtectionHash};
        let limit = match ty {
            0 => EditLimit::TrackedChanges,
            1 => EditLimit::Comments,
            2 => EditLimit::Forms,
            3 => EditLimit::ReadOnly,
            _ => return Err(Error::invalid_call()),
        };
        if pw.chars().count() > MAX_PASSWORD {
            return Err(Error::new(5, "Invalid procedure call or argument: the password is too long"));
        }
        let w = self.w();
        if w.doc.edit_limit().is_some() {
            return Err(Error::new(4605, "This method or property is not available because the document is already protected"));
        }
        let password = if pw.is_empty() {
            None
        } else {
            match apex_sheet::Password::new(pw).into_iter().next() {
                Some(apex_sheet::Password::Salted { algorithm, hash, salt, spin }) => Some(ProtectionHash { algorithm, hash, salt, spin, legacy: Vec::new() }),
                _ => return Err(Error::new(5, "Invalid procedure call or argument: the password can't be kept")),
            }
        };
        w.doc.protection = Some(Protection { limit, enforced: true, password });
        if limit == EditLimit::TrackedChanges {
            w.doc.track_changes = true;
        }
        w.touched();
        Ok(())
    }

    /// Document.Unprotect: only with the password it was given. A password
    /// this program can't check (older Word files) isn't taken: the
    /// protection stops from Review > Restrict Editing then.
    fn unprotect_doc(&mut self, pw: &str) -> R<()> {
        let w = self.w();
        let Some(p) = w.doc.protection.as_ref().filter(|p| p.enforced) else {
            return Err(Error::new(4605, "This method or property is not available because the document isn't protected"));
        };
        let ok = match &p.password {
            None => true,
            Some(h) if h.algorithm.is_empty() => false,
            Some(h) => {
                let stored = apex_sheet::Password::Salted { algorithm: h.algorithm.clone(), hash: h.hash.clone(), salt: h.salt.clone(), spin: h.spin };
                pw.chars().count() <= MAX_PASSWORD && stored.check(pw) == Some(true)
            }
        };
        if !ok {
            return Err(Error::new(5485, "The password is incorrect"));
        }
        if let Some(p) = &mut w.doc.protection {
            p.enforced = false;
        }
        w.touched();
        Ok(())
    }

    /// Document.Undo and Redo: the macro's own edits (it starts with none
    /// to undo), at most the last few. True when all `times` were done.
    fn undo_doc(&mut self, redo: bool, times: i64) -> R<Value> {
        let w = self.w();
        w.writable()?;
        let mut done = 0;
        for _ in 0..times.clamp(0, 1000) {
            let r = if redo { w.doc.redo() } else { w.doc.undo() };
            if r.is_none() {
                break;
            }
            done += 1;
        }
        if done > 0 {
            w.dirty();
            let last = w.story_len().saturating_sub(1);
            w.anchor = w.anchor.min(last);
            w.active = w.active.min(last);
        }
        Ok(Value::Bool(done as i64 == times.max(0)))
    }

    fn doc_set(&mut self, name: &str, v: Value) -> R<()> {
        let v = self.scalar(v)?;
        match name {
            // Only saving clears it.
            "saved" => {
                v.to_bool()?;
                Ok(())
            }
            "trackrevisions" => {
                let b = v.to_bool()?;
                let w = self.w();
                if w.doc.track_changes != b {
                    w.side_edit(|d, _| d.track_changes = b)?;
                }
                Ok(())
            }
            _ => Err(Error::no_member(name)),
        }
    }

    fn documents_call(&mut self, name: &str, a: &Args) -> R<Value> {
        // From inside another document's member: none of its objects
        // reaches the documents.
        if self.current_book.is_some() && name != "application" && name != "parent" {
            return Err(Error::app("Documents can't be used from a member of another document"));
        }
        match name {
            "count" => Ok(Value::Long(self.docs_count() as i32)),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let k = self.scalar(v)?;
                self.doc_item(k)
            }
            "open" => self.open_doc(a),
            "add" => self.add_doc(a),
            "application" | "parent" => Ok(Value::Object(Obj::Application)),
            _ => Err(Error::no_member(name)),
        }
    }
}
