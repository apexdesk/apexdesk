//! Access's objects for the macros of a database: DoCmd, CurrentDb and
//! its Recordsets (DAO), Forms and Me (the form whose button ran the
//! macro), and DLookup, DCount, DSum...
//!
//! The rows come from and go to the database at once through
//! [`Database`]; what the macro does to the forms on screen (open,
//! close, move to a record, filter, change a field) is asked of the
//! database tab as [`DbRequest`]s, done when the macro ends.

use std::cell::RefCell;
use std::rc::Rc;

use crate::ast::Arg;
use crate::error::{Error, R};
use crate::excel::Args;
use crate::interp::Interp;
use crate::object::Obj;
use crate::value::Value;

/// The database a macro works on.
pub trait Database {
    /// The rows of a table, a saved query or a SELECT.
    fn rows(&mut self, source: &str) -> Result<Rows, String>;
    /// Runs an action query (its SQL, or a saved query's name); the rows
    /// it changed.
    fn execute(&mut self, sql: &str) -> Result<usize, String>;
    /// Whether `name` is a saved query that changes rows (an action query)
    /// rather than one that shows them; None: no such query.
    fn is_action_query(&mut self, name: &str) -> Option<bool>;
    fn update(&mut self, table: &str, rowid: i64, values: &[(String, Value)]) -> Result<(), String>;
    /// Adds a row; its rowid.
    fn insert(&mut self, table: &str, values: &[(String, Value)]) -> Result<i64, String>;
    fn delete(&mut self, table: &str, rowid: i64) -> Result<(), String>;
}

/// Rows read from the database.
#[derive(Clone, Debug, Default)]
pub struct Rows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// The table they are rows of and each one's rowid, when they can be
    /// changed.
    pub table: Option<(String, Vec<i64>)>,
}

/// An open form, as the macro sees it.
#[derive(Clone, Debug, Default)]
pub struct FormState {
    pub name: String,
    pub caption: String,
    /// Its table or query.
    pub source: String,
    /// The record shown: each field and its value.
    pub fields: Vec<(String, Value)>,
    /// The named controls and the field each shows.
    pub controls: Vec<(String, String)>,
    /// A new record being filled.
    pub new_record: bool,
    /// The record was changed and not saved.
    pub dirty: bool,
    pub filter: String,
}

impl FormState {
    /// The field `name` names: a control's (`Me!txtName`) or its own.
    fn field(&self, name: &str) -> Option<usize> {
        let f = self.controls.iter().find(|(c, _)| c.eq_ignore_ascii_case(name)).map_or(name, |(_, f)| f.as_str());
        self.fields.iter().position(|(n, _)| n.eq_ignore_ascii_case(f))
    }
}

/// Where GoToRecord goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GoTo {
    First,
    Previous,
    Next,
    Last,
    New,
    /// The n-th record (1-based).
    At(usize),
}

/// What a database's macro asks of the database tab (done when it ends,
/// in order).
#[derive(Clone, Debug, PartialEq)]
pub enum DbRequest {
    /// Opens a form, its records narrowed by a condition (SQL).
    OpenForm { name: String, filter: Option<String> },
    OpenReport { name: String, filter: Option<String> },
    OpenTable(String),
    /// Shows a select query's rows.
    OpenQuery(String),
    /// Closes the form or report of that name.
    Close(String),
    /// Puts a value (as typed) in a field of a form's record.
    SetField { form: String, field: String, value: String },
    GoTo { form: String, to: GoTo },
    /// Narrows a form's records (None: all of them).
    Filter { form: String, filter: Option<String> },
    /// Reads a form's records again.
    Requery(String),
    SaveRecord(String),
    DeleteRecord(String),
}

/// The database's state while its macro runs.
pub struct AccessState<'a> {
    pub db: &'a mut dyn Database,
    pub forms: Vec<FormState>,
    /// The form whose button ran the macro (Me).
    pub me: Option<String>,
    pub requests: Vec<DbRequest>,
    /// Rows were changed.
    pub changed: bool,
    /// CurrentDb.RecordsAffected.
    affected: usize,
}

impl<'a> AccessState<'a> {
    pub fn new(db: &'a mut dyn Database, forms: Vec<FormState>, me: Option<String>) -> AccessState<'a> {
        AccessState { db, forms, me, requests: Vec::new(), changed: false, affected: 0 }
    }

    fn form(&self, name: &str) -> Option<usize> {
        self.forms.iter().position(|f| f.name.eq_ignore_ascii_case(name))
    }
}

/// A Recordset: the rows read, where it is and the record being edited.
#[derive(Debug)]
pub struct Recordset {
    rows: Rows,
    /// The current record; `rows.len()` past the last (EOF).
    pos: usize,
    /// Before the first (BOF).
    bof: bool,
    edit: Option<Edit>,
    closed: bool,
}

#[derive(Debug)]
struct Edit {
    /// AddNew (else Edit).
    new: bool,
    values: Vec<(usize, Value)>,
}

pub type RsRef = Rc<RefCell<Recordset>>;

/// Objects of Access's model.
#[derive(Clone, Debug)]
pub enum AObj {
    DoCmd,
    /// CurrentDb.
    Db,
    Recordset(RsRef),
    Fields(RsRef),
    Field(RsRef, usize),
    Forms,
    /// An open form, by name.
    Form(Rc<str>),
    /// A control (or field) of an open form.
    Control(Rc<str>, Rc<str>),
}

impl AObj {
    pub fn type_name(&self) -> &'static str {
        match self {
            AObj::DoCmd => "DoCmd",
            AObj::Db => "Database",
            AObj::Recordset(_) => "Recordset",
            AObj::Fields(_) => "Fields",
            AObj::Field(..) => "Field",
            AObj::Forms => "Forms",
            AObj::Form(_) => "Form",
            AObj::Control(..) => "Control",
        }
    }

    pub fn same(&self, other: &AObj) -> bool {
        match (self, other) {
            (AObj::Recordset(a), AObj::Recordset(b)) | (AObj::Fields(a), AObj::Fields(b)) => Rc::ptr_eq(a, b),
            (AObj::Field(a, i), AObj::Field(b, j)) => Rc::ptr_eq(a, b) && i == j,
            (AObj::Form(a), AObj::Form(b)) => a.eq_ignore_ascii_case(b),
            (AObj::Control(a, x), AObj::Control(b, y)) => a.eq_ignore_ascii_case(b) && x.eq_ignore_ascii_case(y),
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
        }
    }
}

fn ao(o: AObj) -> Value {
    Value::Object(Obj::Access(Box::new(o)))
}

/// A table or query name as Access SQL takes it (`[Order Details]`).
fn quoted(name: &str) -> String {
    let n = name.trim();
    let n = n.strip_prefix('[').and_then(|n| n.strip_suffix(']')).unwrap_or(n);
    format!("[{n}]")
}

/// Whether `source` is SQL rather than a name.
fn is_sql(source: &str) -> bool {
    let s = source.trim_start().to_ascii_lowercase();
    s.starts_with("select ") || s.starts_with("select\n") || s.starts_with("with ")
}

fn db_err(e: String) -> Error {
    Error::new(3061, e)
}

fn no_current() -> Error {
    Error::new(3021, "No current record")
}

impl<'a> Interp<'a> {
    fn acc(&mut self) -> &mut AccessState<'a> {
        // Access's objects are only made in a database's macro.
        self.access.as_mut().expect("a database's macro")
    }

    /// Names of Access's model usable without a qualifier.
    pub(crate) fn access_global(&mut self, n: &str, args: &[Arg]) -> Option<R<Value>> {
        let r = match n {
            "docmd" => Ok(ao(AObj::DoCmd)),
            "currentdb" | "codedb" => Ok(ao(AObj::Db)),
            "forms" if args.is_empty() => Ok(ao(AObj::Forms)),
            "forms" => return Some(self.access_member(AObj::Forms, "item", args)),
            "application" | "access" => Ok(Value::Object(Obj::Application)),
            "screen" => Ok(Value::Object(Obj::Application)),
            "nz" => (|| {
                let a = self.args(args)?;
                let v = a.get(0, "value").ok_or_else(Error::invalid_call)?;
                let v = self.scalar(v)?;
                Ok(match v {
                    Value::Null | Value::Empty => a.get(1, "valueifnull").unwrap_or(Value::Empty),
                    v => v,
                })
            })(),
            "dlookup" | "dcount" | "dsum" | "davg" | "dmin" | "dmax" | "dfirst" | "dlast" => self.acc_domain(n, args),
            "workbooks" | "worksheets" | "sheets" | "activesheet" | "activeworkbook" | "thisworkbook" | "range" | "cells" | "selection" | "activecell" => Err(Error::unsupported(&format!("{n} (a database's macro has no workbook)"))),
            _ => return None,
        };
        Some(r)
    }

    /// DLookup("expr", "domain", "criteria") and its kin.
    fn acc_domain(&mut self, f: &str, args: &[Arg]) -> R<Value> {
        let a = self.args(args)?;
        let text = |it: &mut Self, i: usize, name: &str| -> R<Option<String>> {
            match a.get(i, name) {
                Some(v) => Ok(Some(it.scalar(v)?.to_str()?)),
                None => Ok(None),
            }
        };
        let expr = text(self, 0, "expr")?.ok_or_else(Error::invalid_call)?;
        let dom = text(self, 1, "domain")?.ok_or_else(Error::invalid_call)?;
        let crit = text(self, 2, "criteria")?.filter(|c| !c.trim().is_empty());
        let agg = match f {
            "dcount" => format!("COUNT({expr})"),
            "dsum" => format!("SUM({expr})"),
            "davg" => format!("AVG({expr})"),
            "dmin" => format!("MIN({expr})"),
            "dmax" => format!("MAX({expr})"),
            _ => expr,
        };
        let mut sql = format!("SELECT {agg} FROM {}", quoted(&dom));
        if let Some(c) = crit {
            sql.push_str(&format!(" WHERE {c}"));
        }
        match f {
            "dlast" => sql.push_str(" ORDER BY rowid DESC LIMIT 1"),
            "dlookup" | "dfirst" => sql.push_str(" LIMIT 1"),
            _ => {}
        }
        let rows = self.acc().db.rows(&sql).map_err(db_err)?;
        Ok(rows.rows.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or(Value::Null))
    }

    /// Application in a database's macro.
    pub(crate) fn access_app(&mut self, name: &str, args: &[Arg]) -> R<Value> {
        match name {
            "activeform" => self.access_me(),
            "name" => Ok(Value::str("ApexDesk")),
            _ => match self.access_global(name, args) {
                Some(r) => r,
                None => Err(Error::no_member(name)),
            },
        }
    }

    /// Me: the form whose button ran the macro.
    pub(crate) fn access_me(&mut self) -> R<Value> {
        match self.acc().me.clone() {
            Some(f) => Ok(ao(AObj::Form(f.into()))),
            None => Err(Error::new(2475, "No form is active: the macro wasn't run from a form")),
        }
    }

    pub(crate) fn access_member(&mut self, o: AObj, name: &str, args: &[Arg]) -> R<Value> {
        let a = self.args(args)?;
        match o {
            AObj::DoCmd => self.acc_docmd(name, &a),
            AObj::Db => self.acc_db(name, &a),
            AObj::Recordset(rs) => self.acc_recordset(rs, name, &a),
            AObj::Fields(rs) => match name {
                "count" => Ok(Value::Long(rs.borrow().rows.columns.len() as i32)),
                "item" | "_default" => {
                    let i = self.acc_field(&rs, a.get(0, "index"))?;
                    Ok(ao(AObj::Field(rs, i)))
                }
                _ => Err(Error::no_member(name)),
            },
            AObj::Field(rs, i) => match name {
                "name" => Ok(Value::str(rs.borrow().rows.columns[i].as_str())),
                "value" | "_default" => field_value(&rs, i),
                _ => Err(Error::no_member(name)),
            },
            AObj::Forms => match name {
                "count" => Ok(Value::Long(self.acc().forms.len() as i32)),
                "item" | "_default" => {
                    let k = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                    let k = self.scalar(k)?;
                    let st = self.acc();
                    let f = match k {
                        Value::Str(s) => st.form(&s),
                        v => usize::try_from(v.to_i64()?).ok().filter(|&i| i < st.forms.len()),
                    };
                    match f {
                        Some(i) => Ok(ao(AObj::Form(st.forms[i].name.as_str().into()))),
                        None => Err(Error::new(2450, "The form isn't open")),
                    }
                }
                _ => Err(Error::no_member(name)),
            },
            AObj::Form(f) => self.acc_form(f, name, &a),
            AObj::Control(f, c) => match name {
                "value" | "_default" | "text" => {
                    let st = self.acc();
                    let fi = st.form(&f).ok_or_else(|| Error::new(2450, "The form isn't open"))?;
                    let k = st.forms[fi].field(&c).ok_or_else(|| Error::new(2465, format!("No control or field named {c}")))?;
                    Ok(st.forms[fi].fields[k].1.clone())
                }
                "name" => Ok(Value::str(&*c)),
                "controlsource" => {
                    let st = self.acc();
                    let fi = st.form(&f).ok_or_else(|| Error::new(2450, "The form isn't open"))?;
                    let k = st.forms[fi].field(&c).ok_or_else(|| Error::new(2465, format!("No control or field named {c}")))?;
                    Ok(Value::str(st.forms[fi].fields[k].0.as_str()))
                }
                "setfocus" | "requery" | "undo" => Ok(Value::Empty),
                _ => Err(Error::no_member(name)),
            },
        }
    }

    fn acc_form(&mut self, f: Rc<str>, name: &str, a: &Args) -> R<Value> {
        let st = self.acc();
        let fi = st.form(&f).ok_or_else(|| Error::new(2450, "The form isn't open"))?;
        let form = &st.forms[fi];
        let fname = form.name.clone();
        Ok(match name {
            "item" | "_default" | "controls" if a.get(0, "index").is_some() => {
                let n = a.get(0, "index").expect("checked");
                let n = self.scalar(n)?.to_str()?;
                let st = self.acc();
                if st.forms[fi].field(&n).is_none() {
                    return Err(Error::new(2465, format!("No control or field named {n}")));
                }
                ao(AObj::Control(f, n.into()))
            }
            "name" => Value::str(fname),
            "caption" => Value::str(form.caption.as_str()),
            "recordsource" => Value::str(form.source.as_str()),
            "filter" => Value::str(form.filter.as_str()),
            "filteron" => Value::Bool(!form.filter.is_empty()),
            "newrecord" => Value::Bool(form.new_record),
            "dirty" => Value::Bool(form.dirty),
            "requery" | "refresh" | "recalc" => {
                st.requests.push(DbRequest::Requery(fname));
                Value::Empty
            }
            "setfocus" | "repaint" => Value::Empty,
            "undo" => {
                st.forms[fi].dirty = false;
                st.requests.retain(|r| !matches!(r, DbRequest::SetField { form, .. } if *form == fname));
                Value::Empty
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(crate) fn access_set(&mut self, o: AObj, name: &str, args: &[Arg], v: Value) -> R<()> {
        let v = match v {
            v @ Value::Object(_) => self.scalar(v)?,
            v => v,
        };
        match o {
            AObj::Recordset(rs) if matches!(name, "item" | "_default" | "fields") => {
                let a = self.args(args)?;
                let i = self.acc_field(&rs, a.get(0, "index"))?;
                set_field(&rs, i, v)
            }
            AObj::Fields(rs) if matches!(name, "item" | "_default") => {
                let a = self.args(args)?;
                let i = self.acc_field(&rs, a.get(0, "index"))?;
                set_field(&rs, i, v)
            }
            AObj::Field(rs, i) if matches!(name, "value" | "_default") => set_field(&rs, i, v),
            AObj::Form(f) if matches!(name, "item" | "_default" | "controls") => {
                let a = self.args(args)?;
                let n = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let n = self.scalar(n)?.to_str()?;
                self.acc_set_control(&f, &n, v)
            }
            AObj::Form(f) if args.is_empty() => {
                let st = self.acc();
                let fi = st.form(&f).ok_or_else(|| Error::new(2450, "The form isn't open"))?;
                let fname = st.forms[fi].name.clone();
                match name {
                    "filter" => st.forms[fi].filter = v.to_str()?,
                    "filteron" => {
                        let on = v.to_bool()?;
                        let filter = st.forms[fi].filter.clone();
                        st.requests.push(DbRequest::Filter { form: fname, filter: (on && !filter.is_empty()).then_some(filter) });
                    }
                    "dirty" if !v.to_bool()? => {
                        st.forms[fi].dirty = false;
                        st.requests.push(DbRequest::SaveRecord(fname));
                    }
                    "caption" => st.forms[fi].caption = v.to_str()?,
                    _ => return Err(Error::no_member(name)),
                }
                Ok(())
            }
            AObj::Control(f, c) if matches!(name, "value" | "_default" | "text") => self.acc_set_control(&f, &c, v),
            o => Err(Error::no_member(&format!("{name} of {}", o.type_name()))),
        }
    }

    fn acc_set_control(&mut self, f: &str, c: &str, v: Value) -> R<()> {
        let text = match &v {
            Value::Null | Value::Empty => String::new(),
            v => v.to_str()?,
        };
        let st = self.acc();
        let fi = st.form(f).ok_or_else(|| Error::new(2450, "The form isn't open"))?;
        let k = st.forms[fi].field(c).ok_or_else(|| Error::new(2465, format!("No control or field named {c}")))?;
        let form = &mut st.forms[fi];
        form.fields[k].1 = v;
        form.dirty = true;
        let (form, field) = (form.name.clone(), form.fields[k].0.clone());
        st.requests.push(DbRequest::SetField { form, field, value: text });
        Ok(())
    }

    pub(crate) fn access_default(&mut self, o: AObj) -> R<Value> {
        match o {
            AObj::Field(rs, i) => field_value(&rs, i),
            AObj::Control(..) => self.access_member(o, "value", &[]),
            o => Err(Error::no_member(&format!("default property of {}", o.type_name()))),
        }
    }

    pub(crate) fn access_items(&mut self, o: AObj) -> R<Vec<Value>> {
        match o {
            AObj::Fields(rs) | AObj::Recordset(rs) => {
                let n = rs.borrow().rows.columns.len();
                Ok((0..n).map(|i| ao(AObj::Field(rs.clone(), i))).collect())
            }
            AObj::Forms => Ok(self.acc().forms.iter().map(|f| ao(AObj::Form(f.name.as_str().into()))).collect()),
            _ => Err(Error::new(92, "For Each may only iterate over a collection object or an array")),
        }
    }

    // --- DoCmd -------------------------------------------------------------------------

    fn acc_text(&mut self, a: &Args, i: usize, name: &str) -> R<Option<String>> {
        match a.get(i, name) {
            Some(v) => Ok(Some(self.scalar(v)?.to_str()?).filter(|s| !s.is_empty())),
            None => Ok(None),
        }
    }

    fn acc_num(&mut self, a: &Args, i: usize, name: &str) -> R<Option<i64>> {
        match a.get(i, name) {
            Some(v) => Ok(Some(self.scalar(v)?.to_i64()?)),
            None => Ok(None),
        }
    }

    /// The form a DoCmd works on: the one named, else Me.
    fn acc_target(&mut self, name: Option<String>) -> R<String> {
        let st = self.acc();
        match name {
            Some(n) => st.form(&n).map(|i| st.forms[i].name.clone()).ok_or_else(|| Error::new(2489, format!("The form {n} isn't open"))),
            None => st.me.clone().ok_or_else(|| Error::new(2046, "No form is active")),
        }
    }

    fn acc_docmd(&mut self, name: &str, a: &Args) -> R<Value> {
        match name {
            "openform" => {
                let form = self.acc_text(a, 0, "formname")?.ok_or_else(Error::invalid_call)?;
                let filter = self.acc_text(a, 3, "wherecondition")?;
                self.acc().requests.push(DbRequest::OpenForm { name: form, filter });
            }
            "openreport" => {
                let report = self.acc_text(a, 0, "reportname")?.ok_or_else(Error::invalid_call)?;
                let filter = self.acc_text(a, 3, "wherecondition")?;
                self.acc().requests.push(DbRequest::OpenReport { name: report, filter });
            }
            "opentable" => {
                let t = self.acc_text(a, 0, "tablename")?.ok_or_else(Error::invalid_call)?;
                self.acc().requests.push(DbRequest::OpenTable(t));
            }
            "openquery" => {
                let q = self.acc_text(a, 0, "queryname")?.ok_or_else(Error::invalid_call)?;
                let st = self.acc();
                match st.db.is_action_query(&q) {
                    Some(true) => {
                        st.affected = st.db.execute(&q).map_err(db_err)?;
                        st.changed = true;
                    }
                    Some(false) => st.requests.push(DbRequest::OpenQuery(q)),
                    None => return Err(Error::new(7874, format!("The query {q} doesn't exist"))),
                }
            }
            "runsql" => {
                let sql = self.acc_text(a, 0, "sqlstatement")?.ok_or_else(Error::invalid_call)?;
                let st = self.acc();
                st.affected = st.db.execute(&sql).map_err(db_err)?;
                st.changed = true;
            }
            "close" => {
                let kind = self.acc_num(a, 0, "objecttype")?;
                let target = self.acc_text(a, 1, "objectname")?;
                let target = match (kind, target) {
                    (_, Some(t)) => t,
                    (None | Some(-1 | 2), None) => self.acc_target(None)?,
                    _ => return Ok(Value::Empty),
                };
                let st = self.acc();
                st.requests.push(DbRequest::Close(target.clone()));
                st.forms.retain(|f| !f.name.eq_ignore_ascii_case(&target));
            }
            "gotorecord" => {
                let form = self.acc_text(a, 1, "objectname")?;
                let form = self.acc_target(form)?;
                let to = match self.acc_num(a, 2, "record")?.unwrap_or(1) {
                    0 => GoTo::Previous,
                    2 => GoTo::First,
                    3 => GoTo::Last,
                    4 => GoTo::At(self.acc_num(a, 3, "offset")?.unwrap_or(1).max(1) as usize),
                    5 => GoTo::New,
                    _ => GoTo::Next,
                };
                self.acc().requests.push(DbRequest::GoTo { form, to });
            }
            "applyfilter" | "showallrecords" | "setfilter" => {
                let form = self.acc_target(None)?;
                let filter = if name == "showallrecords" { None } else { self.acc_text(a, 1, "wherecondition")?.or(self.acc_text(a, 0, "filtername")?) };
                let st = self.acc();
                if let Some(i) = st.form(&form) {
                    st.forms[i].filter = filter.clone().unwrap_or_default();
                }
                st.requests.push(DbRequest::Filter { form, filter });
            }
            "requery" => {
                let form = self.acc_target(None)?;
                self.acc().requests.push(DbRequest::Requery(form));
            }
            "runcommand" => {
                let form = self.acc_target(None)?;
                let r = match self.acc_num(a, 0, "command")? {
                    Some(97) => DbRequest::SaveRecord(form),
                    Some(223) => DbRequest::DeleteRecord(form),
                    Some(18) => DbRequest::Requery(form),
                    _ => return Err(Error::unsupported("DoCmd.RunCommand with that command")),
                };
                self.acc().requests.push(r);
            }
            "save" => {
                let form = self.acc_target(None)?;
                self.acc().requests.push(DbRequest::SaveRecord(form));
            }
            "setwarnings" | "hourglass" | "echo" | "beep" | "maximize" | "minimize" | "restore" | "repaintobject" | "gotocontrol" => {}
            "quit" => return Err(Error::unsupported("DoCmd.Quit (macros don't close the program)")),
            _ => return Err(Error::unsupported(&format!("DoCmd.{name}"))),
        }
        Ok(Value::Empty)
    }

    // --- CurrentDb and Recordsets --------------------------------------------------------

    fn acc_db(&mut self, name: &str, a: &Args) -> R<Value> {
        match name {
            "execute" => {
                let sql = self.acc_text(a, 0, "query")?.ok_or_else(Error::invalid_call)?;
                let st = self.acc();
                st.affected = st.db.execute(&sql).map_err(db_err)?;
                st.changed = true;
                Ok(Value::Empty)
            }
            "openrecordset" => {
                let src = self.acc_text(a, 0, "name")?.ok_or_else(Error::invalid_call)?;
                let src = if is_sql(&src) { src } else { src.trim().trim_start_matches('[').trim_end_matches(']').to_string() };
                let rows = self.acc().db.rows(&src).map_err(db_err)?;
                let bof = rows.rows.is_empty();
                Ok(ao(AObj::Recordset(Rc::new(RefCell::new(Recordset { rows, pos: 0, bof, edit: None, closed: false })))))
            }
            "recordsaffected" => Ok(Value::Long(self.acc().affected as i32)),
            "name" => Ok(Value::str("CurrentDb")),
            "close" => Ok(Value::Empty),
            _ => Err(Error::no_member(name)),
        }
    }

    fn acc_field(&mut self, rs: &RsRef, k: Option<Value>) -> R<usize> {
        let k = self.scalar(k.ok_or_else(Error::invalid_call)?)?;
        let r = rs.borrow();
        match k {
            Value::Str(s) => r.rows.columns.iter().position(|c| c.eq_ignore_ascii_case(s.trim_start_matches('[').trim_end_matches(']'))).ok_or_else(|| Error::new(3265, format!("Item not found in this collection: {s}"))),
            v => usize::try_from(v.to_i64()?).ok().filter(|&i| i < r.rows.columns.len()).ok_or_else(|| Error::new(3265, "Item not found in this collection")),
        }
    }

    fn acc_recordset(&mut self, rs: RsRef, name: &str, a: &Args) -> R<Value> {
        if rs.borrow().closed && name != "close" {
            return Err(Error::new(3420, "Object invalid or no longer set (the Recordset is closed)"));
        }
        let len = rs.borrow().rows.rows.len();
        let step = |rs: &RsRef, to: isize| {
            let mut r = rs.borrow_mut();
            r.edit = None;
            if to < 0 {
                r.pos = 0;
                r.bof = true;
            } else {
                r.pos = (to as usize).min(len);
                r.bof = len == 0;
            }
        };
        match name {
            "eof" => Ok(Value::Bool(rs.borrow().pos >= len)),
            "bof" => Ok(Value::Bool(rs.borrow().bof)),
            "recordcount" => Ok(Value::Long(len as i32)),
            "absoluteposition" => Ok(Value::Long(if rs.borrow().pos >= len { -1 } else { rs.borrow().pos as i32 })),
            "nomatch" => Ok(Value::Bool(false)),
            "movefirst" => {
                step(&rs, 0);
                Ok(Value::Empty)
            }
            "movelast" => {
                step(&rs, len as isize - 1);
                Ok(Value::Empty)
            }
            "movenext" => {
                let p = rs.borrow().pos;
                if p >= len {
                    return Err(no_current());
                }
                let bof = rs.borrow().bof;
                step(&rs, if bof { 0 } else { p as isize + 1 });
                Ok(Value::Empty)
            }
            "moveprevious" => {
                let p = rs.borrow().pos;
                if rs.borrow().bof {
                    return Err(no_current());
                }
                step(&rs, p as isize - 1);
                Ok(Value::Empty)
            }
            "move" => {
                let n = self.acc_num(a, 0, "rows")?.ok_or_else(Error::invalid_call)?;
                let p = rs.borrow().pos as i64 + n;
                step(&rs, p.clamp(-1, len as i64) as isize);
                Ok(Value::Empty)
            }
            "fields" if a.is_empty() => Ok(ao(AObj::Fields(rs))),
            "fields" | "item" | "_default" | "collect" => {
                let i = self.acc_field(&rs, a.get(0, "index"))?;
                if name == "fields" {
                    Ok(ao(AObj::Field(rs, i)))
                } else {
                    field_value(&rs, i)
                }
            }
            "addnew" | "edit" => {
                let new = name == "addnew";
                let mut r = rs.borrow_mut();
                if r.rows.table.is_none() {
                    return Err(Error::new(3027, "Cannot update. Database or object is read-only (rows of a query)"));
                }
                if !new && r.pos >= len {
                    return Err(no_current());
                }
                r.edit = Some(Edit { new, values: Vec::new() });
                Ok(Value::Empty)
            }
            "cancelupdate" => {
                rs.borrow_mut().edit = None;
                Ok(Value::Empty)
            }
            "update" => {
                let (edit, table, rowid, cols) = {
                    let mut r = rs.borrow_mut();
                    let edit = r.edit.take().ok_or_else(|| Error::new(3020, "Update or CancelUpdate without AddNew or Edit"))?;
                    let (table, ids) = r.rows.table.clone().expect("checked by AddNew and Edit");
                    let rowid = ids.get(r.pos).copied();
                    (edit, table, rowid, r.rows.columns.clone())
                };
                let values: Vec<(String, Value)> = edit.values.iter().map(|(i, v)| (cols[*i].clone(), v.clone())).collect();
                let st = self.acc();
                if edit.new {
                    let id = st.db.insert(&table, &values).map_err(db_err)?;
                    let mut r = rs.borrow_mut();
                    let mut row = vec![Value::Null; cols.len()];
                    for (i, v) in edit.values {
                        row[i] = v;
                    }
                    r.rows.rows.push(row);
                    if let Some((_, ids)) = &mut r.rows.table {
                        ids.push(id);
                    }
                } else {
                    let rowid = rowid.ok_or_else(no_current)?;
                    if !values.is_empty() {
                        st.db.update(&table, rowid, &values).map_err(db_err)?;
                    }
                    let mut r = rs.borrow_mut();
                    let p = r.pos;
                    for (i, v) in edit.values {
                        r.rows.rows[p][i] = v;
                    }
                }
                self.acc().changed = true;
                Ok(Value::Empty)
            }
            "delete" => {
                let (table, rowid) = {
                    let r = rs.borrow();
                    let (table, ids) = r.rows.table.as_ref().ok_or_else(|| Error::new(3027, "Cannot update. Database or object is read-only (rows of a query)"))?;
                    (table.clone(), *ids.get(r.pos).ok_or_else(no_current)?)
                };
                let st = self.acc();
                st.db.delete(&table, rowid).map_err(db_err)?;
                st.changed = true;
                let mut r = rs.borrow_mut();
                let p = r.pos;
                r.rows.rows.remove(p);
                if let Some((_, ids)) = &mut r.rows.table {
                    ids.remove(p);
                }
                Ok(Value::Empty)
            }
            "close" => {
                rs.borrow_mut().closed = true;
                Ok(Value::Empty)
            }
            "getrows" => {
                let n = self.acc_num(a, 0, "numrows")?.unwrap_or(i64::MAX).max(0) as usize;
                let mut r = rs.borrow_mut();
                let from = r.pos.min(len);
                let to = from.saturating_add(n).min(len);
                let cols = r.rows.columns.len();
                // Fields down, records across, as Access gives them.
                let mut arr = crate::value::Array::new(vec![(0, cols as i64 - 1), (0, (to - from) as i64 - 1)], crate::value::VType::Variant)?;
                for (j, row) in r.rows.rows[from..to].iter().enumerate() {
                    for (i, v) in row.iter().enumerate() {
                        let at = arr.offset(&[i as i64, j as i64])?;
                        arr.data[at] = v.clone();
                    }
                }
                r.pos = to;
                Ok(Value::Array(Rc::new(arr)))
            }
            "requery" => Ok(Value::Empty),
            _ => Err(Error::no_member(name)),
        }
    }
}

fn field_value(rs: &RsRef, i: usize) -> R<Value> {
    let r = rs.borrow();
    if let Some(e) = &r.edit
        && let Some((_, v)) = e.values.iter().find(|(k, _)| *k == i)
    {
        return Ok(v.clone());
    }
    if r.edit.as_ref().is_some_and(|e| e.new) {
        return Ok(Value::Null);
    }
    match r.rows.rows.get(r.pos) {
        Some(row) if !r.bof => Ok(row[i].clone()),
        _ => Err(no_current()),
    }
}

fn set_field(rs: &RsRef, i: usize, v: Value) -> R<()> {
    let mut r = rs.borrow_mut();
    let Some(e) = &mut r.edit else { return Err(Error::new(3020, "Update or CancelUpdate without AddNew or Edit")) };
    match e.values.iter_mut().find(|(k, _)| *k == i) {
        Some(slot) => slot.1 = v,
        None => e.values.push((i, v)),
    }
    Ok(())
}
