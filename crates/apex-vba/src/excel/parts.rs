//! Parts of the workbook reached from its objects: defined names (Names,
//! Name), a sheet's Tab, Hyperlinks, notes as Comment objects and a
//! range's data Validation. All edits go through the workbook's own, so
//! they join the macro's undo step and respect protection.

use apex_sheet::data::{CmpOp, ErrorStyle, ValKind, Validation, ranges_minus};
use apex_sheet::names::{absolute_ref, check_name};
use apex_sheet::{CellRef, DefinedName, Link, Note, Op, Range};

use super::{Args, Interp, PALETTE, drawing, obj, range_obj, swap_rb};
use crate::ast::Arg;
use crate::error::{Error, R};
use crate::object::{Obj, Part, RangeRef};
use crate::value::Value;

/// Excel's error when an object is gone or a call doesn't fit.
fn object_error() -> Error {
    Error::app("Application-defined or object-defined error")
}

/// A 1-based index of a collection of `len` items.
fn nth(i: i64, len: usize) -> R<usize> {
    usize::try_from(i - 1).ok().filter(|&i| i < len).ok_or_else(Error::subscript)
}

/// `Sheet1!x` (quoted when the sheet's name needs it) or `x`.
fn qualified(name: &str, scope: Option<&str>) -> String {
    match scope {
        Some(s) if s.chars().all(|c| c.is_alphanumeric() || c == '_') => format!("{s}!{name}"),
        Some(s) => format!("'{}'!{name}", s.replace('\'', "''")),
        None => name.to_string(),
    }
}

/// A validation's formula as VBA gives it: `=$A$1:$A$3`, `10`, `a,b`.
fn formula_out(f: &str) -> String {
    let t = f.trim();
    if let Some(inner) = t.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        return inner.replace("\"\"", "\"");
    }
    if t.is_empty() || t.parse::<f64>().is_ok() { t.to_string() } else { format!("={t}") }
}

impl Interp<'_> {
    pub(super) fn part_member(&mut self, p: Part, name: &str, args: &[Arg]) -> R<Value> {
        let a = self.args(args)?;
        self.part_call(p, name, &a)
    }

    /// A part as a member gives it: `ws.Hyperlinks` the collection,
    /// `ws.Hyperlinks(1)` an item of it.
    pub(super) fn part_get(&mut self, p: Part, a: &Args) -> R<Value> {
        if a.is_empty() { Ok(obj(Obj::Part(Box::new(p)))) } else { self.part_call(p, "item", a) }
    }

    fn part_call(&mut self, p: Part, name: &str, a: &Args) -> R<Value> {
        match p {
            Part::Names(scope) => self.names_member(scope, name, a),
            Part::Name(n, scope) => self.name_member(&n, scope.as_deref(), name),
            Part::Tab(s) => {
                let color = self.wb.sheets.get(s).ok_or_else(Error::subscript)?.tab_color;
                Ok(match name {
                    "color" => color.map_or(Value::Bool(false), |c| Value::Long(swap_rb(c) as i32)),
                    "colorindex" => Value::Long(color.map_or(-4142, |c| PALETTE.iter().position(|&p| p == c).map_or(-4142, |i| i as i32 + 1))),
                    "parent" => obj(Obj::Sheet(s)),
                    _ => return Err(Error::no_member(name)),
                })
            }
            Part::Hyperlinks(r) => self.hyperlinks_member(r, name, a),
            Part::Hyperlink(s, c) => self.hyperlink_member(s, c, name),
            Part::Comments(s) => {
                let notes: Vec<CellRef> = self.wb.notes(s).into_iter().map(|(c, _)| c).collect();
                match name {
                    "count" => Ok(Value::Long(notes.len() as i32)),
                    "item" | "_default" => {
                        let i = self.arg_i64(a, 0, "index")?.ok_or_else(Error::invalid_call)?;
                        Ok(obj(Obj::Part(Box::new(Part::Comment(s, notes[nth(i, notes.len())?])))))
                    }
                    "parent" => Ok(obj(Obj::Sheet(s))),
                    _ => Err(Error::no_member(name)),
                }
            }
            Part::Comment(s, c) => self.comment_member(s, c, name, a),
            Part::Validation(r) => self.validation_member(r, name, a),
            Part::FormatConditions(r) => self.conditions_member(r, name, a),
            Part::Condition(s, i) => self.condition_member(s, i, name, a),
            Part::ConditionFont(s, i) => self.condition_font(s, i, name, None),
            Part::ConditionInterior(s, i) => self.condition_interior(s, i, name, None),
            Part::Criteria(s, i) => self.criteria_member(s, i, name, a),
            Part::Criterion(s, i, k) => self.criterion_member(s, i, k, name, a),
            Part::CfColor(s, i, k) => self.cf_color(s, i, k, name, None),
            Part::IconSets => match name {
                "item" | "_default" => self.icon_sets(a),
                "count" => Ok(Value::Long(20)),
                _ => Err(Error::no_member(name)),
            },
            Part::PageSetup(s) => self.page_member(s, name),
            Part::HfPage(s, k) => self.hf_page_member(s, k, name),
            Part::HfSection(s, k, i) => self.hf_section(s, k, i, name, None),
            Part::PageBreaks(s, rows) => self.breaks_member(s, rows, name, a),
            Part::PageBreak(s, rows, at) => self.break_member(s, rows, at, name),
            Part::ListObjects(s) => self.tables_member(s, name, a),
            Part::ListObject(id) => self.table_member(id, name, a),
            Part::ListColumns(id) => self.columns_member(id, name, a),
            Part::ListColumn(id, k) => self.column_member(id, k, name),
            Part::ListRows(id) => self.rows_member(id, name, a),
            Part::ListRow(id, k) => self.row_member(id, k, name),
            Part::Characters(s, c, f, n) => self.chars_member(s, c, f, n, name, a),
            Part::CharsFont(s, c, f, n) => self.chars_font(s, c, f, n, name),
            Part::Shapes(s) => self.shapes_member(s, name, a),
            Part::Shape(s, k, id) => self.shape_member(s, k, id, name, a),
            Part::ShapeText(s, id, ch) => self.shape_text_member(s, id, ch, name),
            Part::ShapeFont(s, id) => self.shape_font(s, id, name, None),
            Part::ShapeFill(s, id, line) => self.shape_fill(s, id, line, false, name, None),
            Part::ShapeColor(s, id, line) => self.shape_fill(s, id, line, true, name, None),
            Part::ChartObjects(s) => self.chart_objects_member(s, name, a),
            Part::ChartObject(s, id) => self.chart_object_member(s, id, name, a),
            Part::Chart(s, id) => self.chart_member(s, id, name, a),
            Part::ChartTitle(s, id, k) => self.chart_title(s, id, k, name, None),
            Part::ChartAxis(s, id, k) => self.axis_member(s, id, k, name, None),
            Part::TickLabels(s, id, k) => self.tick_labels(s, id, k, name, None),
            Part::Legend(s, id) => self.legend_member(s, id, name, None),
            Part::SeriesCollection(s, id) => self.series_collection(s, id, name, a),
            Part::Series(s, id, i) => {
                let v = self.series_member(s, id, i, name, None)?;
                // `.Values(3)`: an element of the array.
                match (&v, a.get(0, "index")) {
                    (Value::Array(arr), Some(k)) if matches!(name, "values" | "xvalues") => {
                        let k = self.scalar(k)?.to_i64()?;
                        Ok(arr.data[arr.offset(&[k])?].clone())
                    }
                    _ => Ok(v),
                }
            }
            Part::FileDialog(k) => self.dialog_member(k, name, a),
            Part::DialogFilters(k) => self.dialog_filters(k, name, a),
            Part::DialogFilter(k, n) => self.dialog_filter(k, n, name),
            Part::DialogItems(k) => self.dialog_items(k, name, a),
            Part::Fso => self.fso_member(name, a),
            Part::TextStream(n) => self.stream_member(n, name, a),
            Part::FsoFile(p) | Part::FsoFolder(p) => self.fso_file_member(p, name, a),
            Part::FsoFiles(p) => self.fso_list_member(p, false, name, a),
            Part::FsoFolders(p) => self.fso_list_member(p, true, name, a),
            Part::Styles => self.styles_member(name, a),
            Part::PivotTables(s) => self.pivot_tables(s, name, a),
            Part::PivotTable(i) => self.pivot_member(i, name),
            Part::PivotCache(i) => self.pivot_cache_member(i, name),
            Part::SheetSort(s) => self.sort_member(s, name, a),
            Part::SortFields(s) => self.sort_fields_member(s, name, a),
            Part::SortField(s, i) => self.sort_field_member(s, i, name),
            Part::Style(n) => self.style_member(&n, name),
            Part::StyleFont(n) => self.style_part_get(&n, false, name),
            Part::StyleInterior(n) => self.style_part_get(&n, true, name),
            Part::IconSet(n) => match name {
                "id" => Ok(Value::Long(n as i32)),
                _ => Err(Error::no_member(name)),
            },
        }
    }

    pub(super) fn part_set(&mut self, p: Part, name: &str, v: Value) -> R<()> {
        let v = self.scalar(v)?;
        match p {
            Part::Name(n, scope) => self.name_set(&n, scope.as_deref(), name, v),
            Part::Tab(s) => {
                let sh = self.wb.sheets.get(s).ok_or_else(Error::subscript)?;
                let hidden = sh.hidden;
                let tab_color = match (name, &v) {
                    ("color" | "colorindex", Value::Bool(false)) => None,
                    ("color", v) => match v.to_i64()? {
                        -4142 => None,
                        c => Some(swap_rb(c as u32 & 0xFF_FFFF)),
                    },
                    ("colorindex", v) => match v.to_i64()? {
                        -4142 | -4105 | 0 => None,
                        i @ 1..=56 => Some(PALETTE[i as usize - 1]),
                        _ => return Err(Error::app("Unable to set the ColorIndex property of the Tab class")),
                    },
                    _ => return Err(Error::no_member(name)),
                };
                self.apply(Op::SheetProps { index: s, hidden, tab_color })
            }
            Part::Hyperlink(s, c) => self.hyperlink_set(s, c, name, v),
            Part::Comment(s, c) => {
                let mut note = self.note(s, c)?;
                match name {
                    "visible" => note.visible = v.to_bool()?,
                    "author" => note.author = v.to_str()?,
                    _ => return Err(Error::no_member(name)),
                }
                self.via(|wb| wb.set_note(s, c, Some(note)))
            }
            Part::Validation(r) => self.validation_set(r, name, v),
            Part::Condition(s, i) => self.condition_set(s, i, name, v),
            Part::ConditionFont(s, i) => self.condition_font(s, i, name, Some(v)).map(|_| ()),
            Part::ConditionInterior(s, i) => self.condition_interior(s, i, name, Some(v)).map(|_| ()),
            Part::Criterion(s, i, k) => self.criterion_set(s, i, k, name, v),
            Part::CfColor(s, i, k) => self.cf_color(s, i, k, name, Some(v)).map(|_| ()),
            Part::PageSetup(s) => self.page_set(s, name, v),
            Part::SheetSort(s) => self.sort_set(s, name, v),
            Part::Style(n) => self.style_set(&n, name, v),
            Part::StyleFont(n) => self.style_part_set(&n, false, name, v),
            Part::StyleInterior(n) => self.style_part_set(&n, true, name, v),
            Part::ListObject(id) => self.table_set(id, name, v),
            Part::ListColumn(id, k) => self.column_set(id, k, name, v),
            Part::HfSection(s, k, i) => self.hf_section(s, k, i, name, Some(v)).map(|_| ()),
            Part::Characters(s, c, f, n) => self.chars_set(s, c, f, n, name, v),
            Part::CharsFont(s, c, f, n) => self.chars_font_set(s, c, f, n, name, v),
            Part::Shape(s, k, id) => self.shape_set(s, k, id, name, v),
            Part::ChartObject(s, id) => self.shape_set(s, drawing::CHART, id, name, v),
            Part::ShapeText(s, id, ch) => self.shape_text_set(s, id, ch, name, v),
            Part::ShapeFont(s, id) => self.shape_font(s, id, name, Some(v)).map(|_| ()),
            Part::ShapeFill(s, id, line) => self.shape_fill(s, id, line, false, name, Some(v)).map(|_| ()),
            Part::ShapeColor(s, id, line) => self.shape_fill(s, id, line, true, name, Some(v)).map(|_| ()),
            Part::Chart(s, id) => self.chart_set(s, id, name, v),
            Part::ChartTitle(s, id, k) => self.chart_title(s, id, k, name, Some(v)).map(|_| ()),
            Part::ChartAxis(s, id, k) => self.axis_member(s, id, k, name, Some(v)).map(|_| ()),
            Part::TickLabels(s, id, k) => self.tick_labels(s, id, k, name, Some(v)).map(|_| ()),
            Part::Legend(s, id) => self.legend_member(s, id, name, Some(v)).map(|_| ()),
            Part::Series(s, id, i) => self.series_member(s, id, i, name, Some(v)).map(|_| ()),
            Part::FileDialog(k) => self.dialog_set(k, name, v),
            Part::FsoFile(p) | Part::FsoFolder(p) => self.fso_file_set(p, name, v),
            Part::Names(_) | Part::Hyperlinks(_) | Part::Comments(_) | Part::FormatConditions(_) | Part::Criteria(..) | Part::IconSets | Part::IconSet(_) | Part::HfPage(..) | Part::PageBreaks(..) | Part::PageBreak(..) | Part::ListObjects(_) | Part::ListColumns(_) | Part::ListRows(_) | Part::ListRow(..) | Part::Shapes(_) | Part::ChartObjects(_) | Part::SeriesCollection(..) | Part::DialogFilters(_) | Part::DialogFilter(..) | Part::DialogItems(_) | Part::Styles | Part::SortFields(_) | Part::SortField(..) | Part::PivotTables(_) | Part::PivotTable(_) | Part::PivotCache(_) | Part::Fso | Part::TextStream(_) | Part::FsoFiles(_) | Part::FsoFolders(_) => Err(Error::no_member(name)),
        }
    }

    /// The objects of a collection part, for For Each.
    pub(crate) fn part_items(&self, p: &Part) -> Option<Vec<Value>> {
        Some(match p {
            Part::Names(scope) => self.names_in(*scope).into_iter().map(|d| obj(Obj::Part(Box::new(Part::Name(d.name.clone(), d.scope.clone()))))).collect(),
            Part::Hyperlinks(r) => self.links_in(*r).into_iter().map(|c| obj(Obj::Part(Box::new(Part::Hyperlink(r.sheet, c))))).collect(),
            Part::Styles => self.styles_items(),
            Part::PivotTables(s) => self.pivot_items(*s),
            Part::Comments(s) => self.wb.notes(*s).into_iter().map(|(c, _)| obj(Obj::Part(Box::new(Part::Comment(*s, c))))).collect(),
            Part::ListObjects(s) => self.wb.sheets.get(*s)?.tables.iter().map(|t| obj(Obj::Part(Box::new(Part::ListObject(t.id))))).collect(),
            Part::ListColumns(id) => {
                let n = self.wb.tables().find(|(_, t)| t.id == *id)?.1.columns.len();
                (0..n).map(|k| obj(Obj::Part(Box::new(Part::ListColumn(*id, k))))).collect()
            }
            Part::ListRows(id) => {
                let (a, b) = self.wb.tables().find(|(_, t)| t.id == *id)?.1.data_rows();
                (0..(b + 1).saturating_sub(a)).map(|k| obj(Obj::Part(Box::new(Part::ListRow(*id, k as usize))))).collect()
            }
            Part::DialogItems(k) => self.dialog_items_list(*k),
            Part::FsoFiles(d) => return self.fso_items(d, false),
            Part::FsoFolders(d) => return self.fso_items(d, true),
            p => return self.drawing_items(p),
        })
    }

    // --- names -----------------------------------------------------------------

    /// The names of the workbook (all of them) or local to a sheet.
    fn names_in(&self, scope: Option<usize>) -> Vec<&DefinedName> {
        let sheet = scope.and_then(|s| self.wb.sheets.get(s)).map(|s| s.name.as_str());
        let mut list: Vec<&DefinedName> = self.wb.names.iter().filter(|d| scope.is_none() || d.scope.as_deref().is_some_and(|x| Some(x) == sheet)).collect();
        // In Excel's order: by name.
        list.sort_by_cached_key(|d| (d.name.to_lowercase(), d.scope.as_deref().map(str::to_lowercase)));
        list
    }

    fn find_name(&self, n: &str, scope: Option<&str>) -> R<DefinedName> {
        self.wb.names.iter().find(|d| d.is(n) && d.scope.as_deref().map(str::to_lowercase) == scope.map(str::to_lowercase)).cloned().ok_or_else(object_error)
    }

    /// What a name stands for, from RefersTo: a Range, a formula `=…` or
    /// a constant.
    fn refers_to(&mut self, v: Value) -> R<apex_sheet::Expr> {
        if let Value::Object(Obj::Range(r)) = &v {
            let sheet = self.wb.sheets.get(r.sheet).ok_or_else(Error::subscript)?.name.clone();
            return Ok(absolute_ref(&sheet, r.range));
        }
        let v = self.scalar(v)?;
        let text = match &v {
            Value::Str(s) => s.to_string(),
            v => return apex_sheet::formula::parse_canonical(&v.to_str()?).map_err(|_| object_error()),
        };
        let src = match text.trim().strip_prefix('=') {
            Some(f) => f.to_string(),
            None if text.trim().parse::<f64>().is_ok() => text.trim().to_string(),
            None => format!("\"{}\"", text.replace('"', "\"\"")),
        };
        apex_sheet::formula::parse_canonical(&src).map_err(|e| Error::app(format!("The formula contains an error: {text} ({e})")))
    }

    /// Range.Name = "x": defines a workbook name that refers to the range.
    pub(super) fn range_set_name(&mut self, r: RangeRef, v: Value) -> R<()> {
        let text = v.to_str()?;
        let (sheet, n) = match text.rsplit_once('!') {
            Some((s, n)) => {
                let s = s.trim_matches('\'').replace("''", "'");
                let i = self.wb.sheet_index(&s).ok_or_else(object_error)?;
                (Some(self.wb.sheets[i].name.clone()), n.to_string())
            }
            None => (None, text.clone()),
        };
        if check_name(&n).is_err() {
            return Err(Error::app(format!("The name that you entered is not valid: {n}")));
        }
        let expr = self.refers_to(obj(Obj::Range(r)))?;
        self.via(|wb| wb.define_name(DefinedName::new(&n, sheet, expr)))
    }

    /// Range.Name: the name that refers to exactly this range.
    pub(super) fn range_get_name(&mut self, r: RangeRef) -> R<Value> {
        let sheet = self.wb.sheets.get(r.sheet).ok_or_else(Error::subscript)?.name.clone();
        let found = self.wb.names.iter().find(|d| d.range().is_some_and(|(s, rg)| rg == r.range && s.as_deref().unwrap_or(d.scope.as_deref().unwrap_or(&sheet)).eq_ignore_ascii_case(&sheet)));
        match found {
            Some(d) => Ok(obj(Obj::Part(Box::new(Part::Name(d.name.clone(), d.scope.clone()))))),
            None => Err(object_error()),
        }
    }

    fn names_member(&mut self, scope: Option<usize>, name: &str, a: &Args) -> R<Value> {
        match name {
            "count" => Ok(Value::Long(self.names_in(scope).len() as i32)),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let v = self.scalar(v)?;
                let list = self.names_in(scope);
                let d = match &v {
                    Value::Str(t) => {
                        let (sheet, n) = match t.rsplit_once('!') {
                            Some((s, n)) => (Some(s.trim_matches('\'').replace("''", "'")), n),
                            None => (None, &**t),
                        };
                        list.iter().find(|d| d.is(n) && (sheet.is_none() || d.scope.as_deref().is_some_and(|x| sheet.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(x)))))
                    }
                    v => list.get(nth(v.to_i64()?, list.len())?),
                };
                let d = d.ok_or_else(Error::subscript)?;
                Ok(obj(Obj::Part(Box::new(Part::Name(d.name.clone(), d.scope.clone())))))
            }
            "add" => {
                let text = match self.arg_str(a, 0, "name")? {
                    Some(t) => t,
                    None => self.arg_str(a, 6, "namelocal")?.ok_or_else(Error::invalid_call)?,
                };
                let refers = a.get(1, "refersto").or_else(|| a.get(7, "referstolocal")).ok_or_else(Error::invalid_call)?;
                let (mut sheet, n) = match text.rsplit_once('!') {
                    Some((s, n)) => {
                        let s = s.trim_matches('\'').replace("''", "'");
                        let i = self.wb.sheet_index(&s).ok_or_else(object_error)?;
                        (Some(self.wb.sheets[i].name.clone()), n.to_string())
                    }
                    None => (None, text.clone()),
                };
                if let Some(s) = scope {
                    sheet = Some(self.wb.sheets.get(s).ok_or_else(Error::subscript)?.name.clone());
                }
                if check_name(&n).is_err() {
                    return Err(Error::app(format!("The name that you entered is not valid: {n}")));
                }
                let expr = self.refers_to(refers)?;
                let mut d = DefinedName::new(&n, sheet.clone(), expr);
                if let Some(v) = a.get(2, "visible") {
                    d.hidden = !self.scalar(v)?.to_bool()?;
                }
                self.via(|wb| wb.define_name(d))?;
                Ok(obj(Obj::Part(Box::new(Part::Name(n, sheet)))))
            }
            "parent" => Ok(match scope {
                Some(s) => obj(Obj::Sheet(s)),
                None => obj(Obj::Workbook),
            }),
            "application" => Ok(obj(Obj::Application)),
            _ => Err(Error::no_member(name)),
        }
    }

    fn name_member(&mut self, n: &str, scope: Option<&str>, name: &str) -> R<Value> {
        let d = self.find_name(n, scope)?;
        Ok(match name {
            "name" | "namelocal" => Value::str(qualified(&d.name, d.scope.as_deref())),
            "refersto" | "referstolocal" | "value" | "_default" => Value::str(format!("={}", d.expr.to_canonical())),
            "referstorange" => {
                let (s, r) = d.range().ok_or_else(object_error)?;
                let sheet = match s.as_deref().or(d.scope.as_deref()) {
                    Some(s) => self.wb.sheet_index(s).ok_or_else(object_error)?,
                    None => self.active,
                };
                range_obj(sheet, r)
            }
            "visible" => Value::Bool(!d.hidden),
            "comment" => Value::str(d.comment),
            "index" => Value::Long(self.names_in(None).iter().position(|x| **x == d).map_or(0, |i| i as i32 + 1)),
            "delete" => {
                let mut list = self.wb.names.clone();
                list.retain(|x| x != &d);
                self.via(|wb| wb.set_names(list))?;
                Value::Empty
            }
            "parent" => obj(Obj::Workbook),
            _ => return Err(Error::no_member(name)),
        })
    }

    fn name_set(&mut self, n: &str, scope: Option<&str>, name: &str, v: Value) -> R<()> {
        let d = self.find_name(n, scope)?;
        let mut new = d.clone();
        let mut renames = Vec::new();
        match name {
            "refersto" | "referstolocal" | "value" => new.expr = std::sync::Arc::new(self.refers_to(v)?),
            "visible" => new.hidden = !v.to_bool()?,
            "comment" => new.comment = v.to_str()?,
            "name" => {
                let t = v.to_str()?;
                if check_name(&t).is_err() || self.wb.names.iter().any(|x| x != &d && x.is(&t) && x.scope == d.scope) {
                    return Err(Error::app(format!("The name that you entered is not valid: {t}")));
                }
                renames.push((d.name.clone(), t.clone()));
                new.name = t;
            }
            _ => return Err(Error::no_member(name)),
        }
        let list: Vec<DefinedName> = self.wb.names.iter().map(|x| if x == &d { new.clone() } else { x.clone() }).collect();
        self.via(|wb| wb.replace_names(list, &renames))
    }

    // --- hyperlinks ------------------------------------------------------------

    /// The cells of `r` with a hyperlink, row by row.
    fn links_in(&self, r: RangeRef) -> Vec<CellRef> {
        let Some(sh) = self.wb.sheets.get(r.sheet) else { return Vec::new() };
        let mut out: Vec<CellRef> = sh.iter().filter(|(c, cell)| r.range.contains(*c) && cell.link().is_some()).map(|(c, _)| c).collect();
        out.sort_by_key(|c| (c.row, c.col));
        out
    }

    fn hyperlinks_member(&mut self, r: RangeRef, name: &str, a: &Args) -> R<Value> {
        match name {
            "count" => Ok(Value::Long(self.links_in(r).len() as i32)),
            "item" | "_default" => {
                let i = self.arg_i64(a, 0, "index")?.ok_or_else(Error::invalid_call)?;
                let list = self.links_in(r);
                Ok(obj(Obj::Part(Box::new(Part::Hyperlink(r.sheet, list[nth(i, list.len())?])))))
            }
            "add" => {
                let anchor = match a.get(0, "anchor") {
                    Some(Value::Object(Obj::Range(x))) => x,
                    Some(_) => return Err(Error::unsupported("hyperlinks on shapes")),
                    None => return Err(Error::invalid_call()),
                };
                let address = self.arg_str(a, 1, "address")?.unwrap_or_default();
                let sub = self.arg_str(a, 2, "subaddress")?.unwrap_or_default();
                let tip = self.arg_str(a, 3, "screentip")?.unwrap_or_default();
                let shown = self.arg_str(a, 4, "texttodisplay")?;
                if address.is_empty() && sub.is_empty() {
                    return Err(object_error());
                }
                let target = match (address.is_empty(), sub.is_empty()) {
                    (false, true) => address.clone(),
                    (true, false) => format!("#{sub}"),
                    _ => format!("{address}#{sub}"),
                };
                check_link(&target)?;
                let (s, c) = (anchor.sheet, anchor.range.start);
                let cell = range_obj(s, Range::single(c));
                match shown {
                    Some(t) => self.set_default(cell, Value::str(t))?,
                    None if self.wb.cell(s, c).is_none_or(|x| x.value().is_empty()) => self.set_default(cell, Value::str(if address.is_empty() { sub } else { address }))?,
                    None => {}
                }
                self.via(|wb| wb.set_link(s, c, Some(Link { target, tip })))?;
                Ok(obj(Obj::Part(Box::new(Part::Hyperlink(s, c)))))
            }
            "delete" => {
                self.via(|wb| wb.clear_extras(r.sheet, &r.range, true))?;
                Ok(Value::Empty)
            }
            "parent" => Ok(obj(Obj::Range(r))),
            _ => Err(Error::no_member(name)),
        }
    }

    fn link(&self, s: usize, c: CellRef) -> R<Link> {
        self.wb.cell(s, c).and_then(|x| x.link().cloned()).ok_or_else(object_error)
    }

    fn hyperlink_member(&mut self, s: usize, c: CellRef, name: &str) -> R<Value> {
        let l = self.link(s, c)?;
        let (address, sub) = match l.target.split_once('#') {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (l.target.clone(), String::new()),
        };
        Ok(match name {
            "address" => Value::str(address),
            "subaddress" => Value::str(sub),
            "screentip" => Value::str(l.tip),
            "texttodisplay" | "name" => Value::str(self.wb.display(s, c).0),
            "range" | "parent" => range_obj(s, Range::single(c)),
            "type" => Value::Long(0),
            "delete" => {
                self.via(|wb| wb.set_link(s, c, None))?;
                Value::Empty
            }
            // Opening a link is the user's to do (a browser, a file).
            "follow" => return Err(Error::unsupported("Hyperlink.Follow (open the link by clicking it)")),
            _ => return Err(Error::no_member(name)),
        })
    }

    fn hyperlink_set(&mut self, s: usize, c: CellRef, name: &str, v: Value) -> R<()> {
        let mut l = self.link(s, c)?;
        let (mut address, mut sub) = match l.target.split_once('#') {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (l.target.clone(), String::new()),
        };
        match name {
            "address" => address = v.to_str()?,
            "subaddress" => sub = v.to_str()?,
            "screentip" => l.tip = v.to_str()?,
            "texttodisplay" => return self.set_default(range_obj(s, Range::single(c)), Value::str(v.to_str()?)),
            _ => return Err(Error::no_member(name)),
        }
        l.target = if sub.is_empty() { address } else { format!("{address}#{sub}") };
        // A file's own link keeps working when only its tip changes.
        if name != "screentip" {
            check_link(&l.target)?;
        }
        self.via(|wb| wb.set_link(s, c, Some(l)))
    }

    // --- comments --------------------------------------------------------------

    fn note(&self, s: usize, c: CellRef) -> R<Note> {
        self.wb.cell(s, c).and_then(|x| x.note().cloned()).ok_or_else(object_error)
    }

    fn comment_member(&mut self, s: usize, c: CellRef, name: &str, a: &Args) -> R<Value> {
        let note = self.note(s, c)?;
        Ok(match name {
            "text" => {
                let Some(t) = self.arg_str(a, 0, "text")? else { return Ok(Value::str(note.text)) };
                let start = self.arg_i64(a, 1, "start")?;
                let overwrite = match a.get(2, "overwrite") {
                    Some(v) => self.scalar(v)?.to_bool()?,
                    None => false,
                };
                let text = match start {
                    None => t,
                    Some(k) => {
                        let mut chars: Vec<char> = note.text.chars().collect();
                        let at = usize::try_from(k - 1).map_err(|_| Error::invalid_call())?.min(chars.len());
                        let new: Vec<char> = t.chars().collect();
                        if overwrite {
                            let end = (at + new.len()).min(chars.len());
                            chars.splice(at..end, new);
                        } else {
                            chars.splice(at..at, new);
                        }
                        chars.into_iter().collect()
                    }
                };
                let out = text.clone();
                self.via(|wb| wb.set_note(s, c, Some(Note { text, ..note })))?;
                Value::str(out)
            }
            "author" => Value::str(note.author),
            "visible" => Value::Bool(note.visible),
            "delete" => {
                self.via(|wb| wb.set_note(s, c, None))?;
                Value::Empty
            }
            "parent" => range_obj(s, Range::single(c)),
            "next" | "previous" => {
                let all: Vec<CellRef> = self.wb.notes(s).into_iter().map(|(x, _)| x).collect();
                let i = all.iter().position(|x| *x == c).ok_or_else(object_error)?;
                let j = if name == "next" { i.checked_add(1) } else { i.checked_sub(1) };
                match j.and_then(|j| all.get(j)) {
                    Some(x) => obj(Obj::Part(Box::new(Part::Comment(s, *x)))),
                    None => Value::Nothing,
                }
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    // --- validation ------------------------------------------------------------

    fn validation(&self, r: RangeRef) -> R<(usize, Validation)> {
        let list = &self.wb.sheets.get(r.sheet).ok_or_else(Error::subscript)?.validations;
        list.iter().enumerate().find(|(_, v)| v.contains(r.range.start)).map(|(i, v)| (i, v.clone())).ok_or_else(object_error)
    }

    /// A formula of Validation.Add as the workbook keeps it.
    fn formula_in(&mut self, v: Value, kind: ValKind, origin: CellRef) -> R<String> {
        let v = self.scalar(v)?;
        let text = match &v {
            Value::Str(s) => s.trim().to_string(),
            Value::Date(d) => return Ok(d.to_string()),
            v => return Ok(v.to_f64()?.to_string()),
        };
        if text.starts_with('=') {
            return self.rule_formula(Value::str(text), origin);
        }
        if kind == ValKind::List {
            return Ok(format!("\"{}\"", text.replace('"', "\"\"")));
        }
        if text.is_empty() || text.parse::<f64>().is_ok() {
            return Ok(text);
        }
        // A date or time typed as text: its serial number.
        match crate::value::Value::str(text.clone()).to_date() {
            Ok(d) => Ok(d.to_string()),
            Err(_) => Err(Error::app(format!("The formula contains an error: {text}"))),
        }
    }

    /// The rule Validation.Add or Modify describe.
    fn new_validation(&mut self, r: RangeRef, a: &Args) -> R<Validation> {
        let ty = self.arg_i64(a, 0, "type")?.ok_or_else(Error::invalid_call)?;
        let kind = *usize::try_from(ty).ok().and_then(|i| ValKind::ALL.get(i)).ok_or_else(Error::invalid_call)?;
        let mut v = Validation::new(vec![r.range]);
        v.kind = kind;
        v.error_style = match self.arg_i64(a, 1, "alertstyle")?.unwrap_or(1) {
            1 => ErrorStyle::Stop,
            2 => ErrorStyle::Warning,
            3 => ErrorStyle::Info,
            _ => return Err(Error::invalid_call()),
        };
        v.op = cmp_op(self.arg_i64(a, 2, "operator")?.unwrap_or(1))?;
        if let Some(f) = a.get(3, "formula1") {
            v.f1 = self.formula_in(f, kind, r.range.start)?;
        }
        if let Some(f) = a.get(4, "formula2") {
            v.f2 = self.formula_in(f, kind, r.range.start)?;
        }
        if kind != ValKind::Any && v.f1.is_empty() {
            return Err(object_error());
        }
        Ok(v)
    }

    /// The sheet's rules with `r` left out (each keeps its other cells).
    fn validations_without(&self, r: RangeRef) -> Vec<Validation> {
        self.wb.sheets[r.sheet]
            .validations
            .iter()
            .cloned()
            .filter_map(|mut x| {
                x.ranges = ranges_minus(&x.ranges, &r.range);
                (!x.ranges.is_empty()).then_some(x)
            })
            .collect()
    }

    fn validation_member(&mut self, r: RangeRef, name: &str, a: &Args) -> R<Value> {
        self.wb.sheets.get(r.sheet).ok_or_else(Error::subscript)?;
        match name {
            "add" | "modify" => {
                // Excel refuses Add where a rule is (macros Delete first).
                if name == "add" && self.wb.sheets[r.sheet].validations.iter().any(|v| v.ranges.iter().any(|x| x.intersect(&r.range).is_some())) {
                    return Err(object_error());
                }
                let mut v = self.new_validation(r, a)?;
                if name == "modify"
                    && let Ok((_, old)) = self.validation(r)
                {
                    v = Validation { ranges: v.ranges, kind: v.kind, op: v.op, f1: v.f1, f2: v.f2, error_style: v.error_style, ..old };
                }
                let mut list = self.validations_without(r);
                list.push(v);
                self.apply(Op::Validations { sheet: r.sheet, list })?;
                return Ok(Value::Empty);
            }
            "delete" => {
                let list = self.validations_without(r);
                if list != self.wb.sheets[r.sheet].validations {
                    self.apply(Op::Validations { sheet: r.sheet, list })?;
                }
                return Ok(Value::Empty);
            }
            "parent" => return Ok(obj(Obj::Range(r))),
            "value" => {
                // The cells there are, then whether a rule that refuses
                // blanks covers a cell that isn't there.
                let sh = &self.wb.sheets[r.sheet];
                let cells: Vec<CellRef> = sh.iter().map(|(c, _)| c).filter(|c| r.range.contains(*c)).collect();
                if !cells.iter().all(|c| self.wb.is_valid(r.sheet, *c)) {
                    return Ok(Value::Bool(false));
                }
                for v in sh.validations.iter().filter(|v| v.kind != ValKind::Any && !v.allow_blank) {
                    for x in v.ranges.iter().filter_map(|x| x.intersect(&r.range)) {
                        if x.cell_count() > cells.iter().filter(|c| x.contains(**c)).count() as u64 {
                            return Ok(Value::Bool(false));
                        }
                    }
                }
                return Ok(Value::Bool(true));
            }
            _ => {}
        }
        let (_, v) = self.validation(r)?;
        Ok(match name {
            "type" => Value::Long(ValKind::ALL.iter().position(|k| *k == v.kind).unwrap_or(0) as i32),
            "operator" => Value::Long(CMP_OPS.iter().position(|o| *o == v.op).map_or(1, |i| i as i32 + 1)),
            "alertstyle" => Value::Long(match v.error_style {
                ErrorStyle::Stop => 1,
                ErrorStyle::Warning => 2,
                ErrorStyle::Info => 3,
            }),
            "formula1" => Value::str(formula_out(&v.f1)),
            "formula2" => Value::str(formula_out(&v.f2)),
            "ignoreblank" => Value::Bool(v.allow_blank),
            "incelldropdown" => Value::Bool(v.dropdown),
            "showinput" => Value::Bool(v.show_input),
            "showerror" => Value::Bool(v.show_error),
            "inputtitle" => Value::str(v.input_title),
            "inputmessage" => Value::str(v.input_msg),
            "errortitle" => Value::str(v.error_title),
            "errormessage" => Value::str(v.error_msg),
            _ => return Err(Error::no_member(name)),
        })
    }

    fn validation_set(&mut self, r: RangeRef, name: &str, v: Value) -> R<()> {
        let (i, mut rule) = self.validation(r)?;
        match name {
            "ignoreblank" => rule.allow_blank = v.to_bool()?,
            "incelldropdown" => rule.dropdown = v.to_bool()?,
            "showinput" => rule.show_input = v.to_bool()?,
            "showerror" => rule.show_error = v.to_bool()?,
            "inputtitle" => rule.input_title = v.to_str()?,
            "inputmessage" => rule.input_msg = v.to_str()?,
            "errortitle" => rule.error_title = v.to_str()?,
            "errormessage" => rule.error_msg = v.to_str()?,
            _ => return Err(Error::no_member(name)),
        }
        let mut list = self.wb.sheets[r.sheet].validations.clone();
        list[i] = rule;
        self.apply(Op::Validations { sheet: r.sheet, list })
    }
}

/// XlFormatConditionOperator order.
pub(super) const CMP_OPS: [CmpOp; 8] = [CmpOp::Between, CmpOp::NotBetween, CmpOp::Equal, CmpOp::NotEqual, CmpOp::Greater, CmpOp::Less, CmpOp::GreaterEq, CmpOp::LessEq];

pub(super) fn cmp_op(i: i64) -> R<CmpOp> {
    usize::try_from(i - 1).ok().and_then(|i| CMP_OPS.get(i)).copied().ok_or_else(Error::invalid_call)
}

/// As in documents: macros add web, e-mail and in-workbook links only (no
/// files or programs a click would open).
fn check_link(target: &str) -> R<()> {
    if !crate::word::safe_address(target) || target.len() > 2083 || target.chars().any(char::is_control) {
        return Err(Error::new(1004, "Macros may add web, e-mail and in-workbook links only"));
    }
    Ok(())
}
