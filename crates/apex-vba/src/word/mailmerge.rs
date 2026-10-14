//! Mail merge from a macro: Document.MailMerge with a spreadsheet or CSV
//! file as its data source (the first row names the fields), the «Name»
//! merge fields of the document, and Execute, which makes a new document
//! with a copy per record, as Mailings > Finish & Merge does.

use apex_sheet::{CellRef, Workbook};

use super::{WObj, wo};
use crate::error::{Error, R};
use crate::excel::Args;
use crate::interp::Interp;
use crate::object::Obj;
use crate::value::Value;

/// Most fields and records a data source may have.
const MAX_FIELDS: u32 = 1024;
const MAX_RECORDS: u32 = 100_000;

/// The data source of a document's mail merge.
pub(crate) struct Source {
    /// Its file, as given to OpenDataSource.
    name: String,
    names: Vec<String>,
    records: Vec<Vec<String>>,
    /// The record shown (0-based).
    active: usize,
    /// The records merged (1-based, inclusive; `last` may be past the end).
    first: usize,
    last: usize,
}

/// What a document's MailMerge holds.
pub(crate) struct MailState {
    pub source: Option<Source>,
    /// WdMailMergeMainDocType: -1 none, 0 form letters.
    main_type: i64,
}

impl Default for MailState {
    fn default() -> Self {
        MailState { source: None, main_type: -1 }
    }
}

/// The first sheet of `wb` as field names (its first row) and records.
/// None when it has no field names, or more fields or records than a
/// merge takes.
fn records(wb: &Workbook) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let ext = wb.sheets.first()?.extent()?;
    if ext.col >= MAX_FIELDS || ext.row > MAX_RECORDS {
        return None;
    }
    let text = |r: u32, c: u32| wb.display(0, CellRef::new(r, c)).0.trim().to_string();
    let mut names: Vec<String> = (0..=ext.col).map(|c| text(0, c)).collect();
    while names.last().is_some_and(String::is_empty) {
        names.pop();
    }
    if names.is_empty() {
        return None;
    }
    for (i, n) in names.iter_mut().enumerate() {
        if n.is_empty() {
            *n = format!("Column{}", i + 1);
        }
    }
    let records = (1..=ext.row).map(|r| (0..names.len() as u32).map(|c| text(r, c)).collect::<Vec<_>>()).filter(|rec| rec.iter().any(|v| !v.is_empty())).collect();
    Some((names, records))
}

fn no_source() -> Error {
    Error::new(4605, "This method or property is not available because the document has no data source (MailMerge.OpenDataSource)")
}

fn missing() -> Error {
    Error::new(5941, "The requested member of the collection does not exist")
}

impl Interp<'_> {
    fn source(&mut self) -> R<&mut Source> {
        self.w().mail.source.as_mut().ok_or_else(no_source)
    }

    pub(crate) fn mail_call(&mut self, o: WObj, name: &str, a: &Args) -> R<Value> {
        match o {
            WObj::MailMerge => self.merge_call(name, a),
            WObj::MergeSource => self.merge_source_call(name, a),
            WObj::MergeNames(data) => {
                let count = self.source()?.names.len();
                match name {
                    "count" => Ok(Value::Long(count as i32)),
                    "item" | "_default" => {
                        let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                        let i = match self.scalar(v)? {
                            Value::Str(s) => self.source()?.names.iter().position(|n| n.eq_ignore_ascii_case(&s)).ok_or_else(missing)?,
                            v => match v.to_i64()? {
                                n if n >= 1 && (n as u64) <= count as u64 => n as usize - 1,
                                _ => return Err(missing()),
                            },
                        };
                        Ok(wo(WObj::MergeName(data, i)))
                    }
                    "parent" => Ok(wo(WObj::MergeSource)),
                    "application" => Ok(Value::Object(Obj::Application)),
                    _ => Err(Error::no_member(name)),
                }
            }
            WObj::MergeName(data, i) => {
                let s = self.source()?;
                let field = s.names.get(i).cloned().ok_or_else(missing)?;
                match name {
                    "name" => Ok(Value::str(field)),
                    "_default" if !data => Ok(Value::str(field)),
                    "value" | "_default" if data => Ok(Value::str(s.records.get(s.active).and_then(|r| r.get(i)).cloned().unwrap_or_default())),
                    "index" => Ok(Value::Long(i as i32 + 1)),
                    "parent" => Ok(wo(WObj::MergeSource)),
                    "application" => Ok(Value::Object(Obj::Application)),
                    _ => Err(Error::no_member(name)),
                }
            }
            WObj::MergeFields => match name {
                "count" => Ok(Value::Long(apex_doc::merge::fields_used(&self.w().doc).len() as i32)),
                // Fields.Add(Range, Name): «Name» in place of the range.
                "add" => {
                    let field = self.arg_str(a, 1, "name")?.ok_or_else(Error::invalid_call)?;
                    let field = super::clean_input(&field).replace(['\n', '«', '»'], "");
                    if field.is_empty() || field.chars().count() > 255 {
                        return Err(Error::invalid_call());
                    }
                    let (s, e) = match a.get(0, "range") {
                        Some(v) => self.span_of(&v)?,
                        None => self.w().sel(),
                    };
                    let (at, end) = self.w().replace(s, e, &apex_doc::merge::field_text(&field))?;
                    let r = self.w().new_span(at, end);
                    Ok(wo(WObj::Range(r)))
                }
                "parent" => Ok(wo(WObj::MailMerge)),
                "application" => Ok(Value::Object(Obj::Application)),
                _ => Err(Error::no_member(name)),
            },
            _ => Err(Error::no_member(name)),
        }
    }

    fn merge_call(&mut self, name: &str, a: &Args) -> R<Value> {
        let w = self.w();
        Ok(match name {
            "opendatasource" => {
                let file = self.arg_str(a, 0, "name")?.ok_or_else(|| Error::new(449, "Argument not optional"))?;
                let p = self.path_to_read(&file)?;
                if !p.is_file() {
                    return Err(Error::new(5174, format!("'{file}' could not be found")));
                }
                let path = p.display().to_string();
                let t = std::time::Instant::now();
                let res = self.ui.open_workbook(&path, None);
                self.paused += t.elapsed();
                let wb = res.map_err(|e| Error::new(5631, format!("'{file}' couldn't be opened as a data source: {e}")))?;
                let (names, records) = records(&wb).ok_or_else(|| Error::new(5631, format!("'{file}' has no field names in its first row, or more than {MAX_FIELDS} fields or {MAX_RECORDS} records")))?;
                let m = &mut self.w().mail;
                m.source = Some(Source { name: path, names, records, active: 0, first: 1, last: usize::MAX });
                if m.main_type == -1 {
                    m.main_type = 0;
                }
                Value::Empty
            }
            "datasource" => wo(WObj::MergeSource),
            "fields" => {
                if a.is_empty() {
                    wo(WObj::MergeFields)
                } else {
                    return Err(Error::unsupported("MailMerge.Fields(i) (the fields are «Name» text: use Fields.Count and Fields.Add)"));
                }
            }
            "maindocumenttype" => Value::Long(w.mail.main_type as i32),
            // WdMailMergeState: normal, main document only, with its data.
            "state" => Value::Long(if w.mail.source.is_some() {
                2
            } else if w.mail.main_type != -1 {
                1
            } else {
                0
            }),
            // Only to a new document.
            "destination" => Value::Long(0),
            "execute" => return self.merge_execute(),
            "parent" => wo(WObj::Doc),
            "application" => Value::Object(Obj::Application),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(crate) fn mail_set(&mut self, o: WObj, name: &str, v: Value) -> R<()> {
        let v = self.scalar(v)?;
        match (o, name) {
            (WObj::MailMerge, "maindocumenttype") => match v.to_i64()? {
                -1 => {
                    let m = &mut self.w().mail;
                    m.main_type = -1;
                    m.source = None;
                    Ok(())
                }
                0 => {
                    self.w().mail.main_type = 0;
                    Ok(())
                }
                _ => Err(Error::unsupported("merging as labels, envelopes, a catalog or e-mail from a macro (use form letters, wdFormLetters)")),
            },
            (WObj::MailMerge, "destination") => match v.to_i64()? {
                0 => Ok(()),
                _ => Err(Error::unsupported("merging to the printer, e-mail or fax from a macro (merge to a new document, wdSendToNewDocument)")),
            },
            (WObj::MergeSource, "activerecord") => {
                let s = self.source()?;
                let n = s.records.len();
                if n == 0 {
                    return Ok(());
                }
                s.active = match v.to_i64()? {
                    -2 => (s.active + 1).min(n - 1),
                    -3 => s.active.saturating_sub(1),
                    -4 => 0,
                    -5 => n - 1,
                    i if i >= 1 && (i as u64) <= n as u64 => i as usize - 1,
                    _ => return Err(Error::new(5852, "Requested object is not available")),
                };
                Ok(())
            }
            (WObj::MergeSource, "firstrecord" | "lastrecord") => {
                let i = v.to_i64()?;
                let s = self.source()?;
                let i = match i {
                    -16 if name == "lastrecord" => usize::MAX,
                    i if i >= 1 => i as usize,
                    _ => return Err(Error::new(5852, "Requested object is not available")),
                };
                if name == "firstrecord" {
                    s.first = i;
                } else {
                    s.last = i;
                }
                Ok(())
            }
            (o, _) => Err(Error::no_member(&format!("{name} of {}", o.type_name()))),
        }
    }

    fn merge_source_call(&mut self, name: &str, a: &Args) -> R<Value> {
        match name {
            "fieldnames" | "datafields" => {
                let o = WObj::MergeNames(name == "datafields");
                if a.is_empty() { Ok(wo(o)) } else { self.mail_call(o, "item", a) }
            }
            "findrecord" => {
                let find = self.arg_str(a, 0, "findtext")?.ok_or_else(Error::invalid_call)?.to_lowercase();
                let field = self.arg_str(a, 1, "field")?;
                let s = self.source()?;
                let col = match &field {
                    Some(f) => Some(s.names.iter().position(|n| n.eq_ignore_ascii_case(f)).ok_or_else(missing)?),
                    None => None,
                };
                let n = s.records.len();
                // From the record after the active one, round to it.
                let found = (1..=n).map(|k| (s.active + k) % n.max(1)).find(|&r| s.records[r].iter().enumerate().any(|(c, v)| col.is_none_or(|x| x == c) && v.to_lowercase().contains(&find)));
                if let Some(r) = found {
                    s.active = r;
                }
                Ok(Value::Bool(found.is_some()))
            }
            "close" => {
                self.w().mail.source = None;
                Ok(Value::Empty)
            }
            "parent" => Ok(wo(WObj::MailMerge)),
            "application" => Ok(Value::Object(Obj::Application)),
            _ => {
                let s = self.source()?;
                Ok(match name {
                    "name" => Value::str(s.name.clone()),
                    "recordcount" => Value::Long(s.records.len() as i32),
                    "activerecord" => Value::Long(if s.records.is_empty() { -1 } else { s.active as i32 + 1 }),
                    "firstrecord" => Value::Long(s.first.min(i32::MAX as usize) as i32),
                    "lastrecord" => Value::Long(s.last.min(s.records.len()) as i32),
                    _ => return Err(Error::no_member(name)),
                })
            }
        }
    }

    /// MailMerge.Execute: a new document, one copy of this one per record
    /// (FirstRecord to LastRecord), the active document from now on.
    fn merge_execute(&mut self) -> R<Value> {
        if self.current_book.is_some() {
            return Err(Error::unsupported("MailMerge.Execute of a document the macro opened (merge in the macro's own document)"));
        }
        let (names, records) = {
            let s = self.source()?;
            let first = s.first.max(1) - 1;
            let last = s.last.min(s.records.len());
            if first >= last {
                return Err(Error::new(5852, "Requested object is not available: no records to merge"));
            }
            (s.names.clone(), s.records[first..last].to_vec())
        };
        let w = self.w();
        let len = w.story_len();
        let values: usize = records.iter().flatten().map(String::len).sum();
        if len.saturating_mul(records.len()).saturating_add(values) > super::MAX_STORY || w.doc.paragraphs.len().saturating_mul(records.len()) > super::MAX_PARAS {
            return Err(Error::new(7, "Out of memory: the merged document would be too long"));
        }
        let mut merged = apex_doc::merge::merge(&w.doc, &names, &records);
        // The letters, not the macros that made them.
        merged.macros = Default::default();
        merged.modified = true;
        let name = format!("Letters{}", self.books.len() + 1);
        let b = self.push_doc(merged, name, String::new(), None, false)?;
        Ok(super::wo_in(b, WObj::Doc))
    }
}
