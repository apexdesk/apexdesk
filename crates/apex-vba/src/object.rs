//! Objects a macro can hold: those of Excel's object model (a view of the
//! workbook: a sheet by index, a range by address) and VBA's own
//! collections (Collection, Scripting.Dictionary).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use apex_sheet::Range;

use crate::error::{Error, R};
use crate::value::{Value, number_text};

/// How a range counts and iterates: by cells, rows (`Rows`, `EntireRow`)
/// or columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum By {
    Cells,
    Rows,
    Cols,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeRef {
    pub sheet: usize,
    pub range: Range,
    pub by: By,
}

impl RangeRef {
    pub fn new(sheet: usize, range: Range) -> RangeRef {
        RangeRef { sheet, range, by: By::Cells }
    }
}

#[derive(Clone, Debug)]
pub enum Obj {
    Application,
    Workbook,
    /// Worksheets (else Sheets: the same here, there are no chart sheets).
    Sheets,
    Sheet(usize),
    Range(RangeRef),
    Font(RangeRef),
    Interior(RangeRef),
    /// A border of a range (XlBordersIndex; 0: all of them, Borders).
    Border(RangeRef, i32),
    /// ActiveWindow.
    Window,
    WorksheetFunction,
    Err,
    Collection(Rc<RefCell<Collection>>),
    Dictionary(Rc<RefCell<Dict>>),
    /// A module by name (its public procedures and variables).
    Module(usize),
    /// Objects of several areas of a sheet (a Range of several areas, and
    /// their Fonts...).
    Multi(Rc<[Obj]>),
    /// An object of a class module of the project.
    Instance(Rc<Instance>),
    /// Names, a Tab, Hyperlinks, Comments, a Validation...
    Part(Box<Part>),
    /// A control of a UserForm.
    Control(Rc<Instance>, usize),
    /// The Controls of a form (None) or of a Frame or Page in it; with
    /// `true`, the Pages of a MultiPage.
    Controls(Rc<Instance>, Option<usize>, bool),
    /// The Font of a form's control (None: the form's own).
    ControlFont(Rc<Instance>, Option<usize>),
    /// The workbooks open (the macro's own first).
    Workbooks,
    /// An object of another workbook the macro opened (its number).
    In(usize, Box<Obj>),
    /// An object of Word's model (a macro of a document).
    Word(Box<crate::word::WObj>),
    /// An object of Access's model (a macro of a database).
    Access(Box<crate::access::AObj>),
}

/// What a sheet's Sort object holds until Apply: the keys (a cell of the
/// column or row, ascending), the range, Header and Orientation.
#[derive(Clone, Debug, Default)]
pub struct SortState {
    pub keys: Vec<(RangeRef, bool)>,
    pub range: Option<RangeRef>,
    pub header: i64,
    pub orientation: i64,
    pub match_case: bool,
}

/// Parts of the workbook reached from its objects.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    /// The names of the workbook, or local to a sheet.
    Names(Option<usize>),
    /// A defined name and the sheet it is local to.
    Name(String, Option<String>),
    Tab(usize),
    Hyperlinks(RangeRef),
    Hyperlink(usize, apex_sheet::CellRef),
    Comments(usize),
    Comment(usize, apex_sheet::CellRef),
    Validation(RangeRef),
    FormatConditions(RangeRef),
    /// A rule of conditional formatting: sheet and index (priority).
    Condition(usize, usize),
    ConditionFont(usize, usize),
    ConditionInterior(usize, usize),
    /// ColorScaleCriteria or IconCriteria of a rule.
    Criteria(usize, usize),
    /// A threshold of a rule (1-based; data bars: 1 MinPoint, 2 MaxPoint).
    Criterion(usize, usize, usize),
    /// A color of a rule: a color scale's stop, or 0 a data bar's.
    CfColor(usize, usize, usize),
    IconSets,
    /// An icon set (XlIconSet).
    IconSet(usize),
    PageSetup(usize),
    /// PageSetup.EvenPage (1) or FirstPage (2).
    HfPage(usize, u8),
    /// A header or footer section of a page: left, center, right header
    /// then footer.
    HfSection(usize, u8, u8),
    /// HPageBreaks (true) or VPageBreaks of a sheet.
    PageBreaks(usize, bool),
    /// A manual break before a row (or column).
    PageBreak(usize, bool, u32),
    ListObjects(usize),
    /// A table, by its id.
    ListObject(u32),
    ListColumns(u32),
    ListColumn(u32, usize),
    ListRows(u32),
    /// A data row of a table (0-based).
    ListRow(u32, usize),
    /// Characters of a cell's text: from (1-based) and how many (to the end).
    Characters(usize, apex_sheet::CellRef, u32, Option<u32>),
    CharsFont(usize, apex_sheet::CellRef, u32, Option<u32>),
    Shapes(usize),
    /// A shape, chart or picture of a sheet (see `excel::drawing`), by id.
    Shape(usize, u8, u32),
    /// A shape's TextFrame, or (true) its Characters.
    ShapeText(usize, u32, bool),
    ShapeFont(usize, u32),
    /// A shape's Fill, or (true) its Line.
    ShapeFill(usize, u32, bool),
    ShapeColor(usize, u32, bool),
    ChartObjects(usize),
    ChartObject(usize, u32),
    Chart(usize, u32),
    /// The chart's title (0) or an axis title (1 category, 2 value).
    ChartTitle(usize, u32, u8),
    ChartAxis(usize, u32, u8),
    TickLabels(usize, u32, u8),
    Legend(usize, u32),
    SeriesCollection(usize, u32),
    Series(usize, u32, usize),
    /// Application.FileDialog(type), its Filters, a filter, SelectedItems.
    FileDialog(u8),
    DialogFilters(u8),
    DialogFilter(u8, usize),
    DialogItems(u8),
    /// A sheet's PivotTables, a pivot table and its PivotCache (by index
    /// among the workbook's).
    PivotTables(usize),
    PivotTable(usize),
    PivotCache(usize),
    /// Workbook.Styles, a named style, and its Font and Interior.
    Styles,
    /// A sheet's Sort object, its SortFields and one of them (0-based).
    SheetSort(usize),
    SortFields(usize),
    SortField(usize, usize),
    Style(String),
    StyleFont(String),
    StyleInterior(String),
    /// Scripting.FileSystemObject.
    Fso,
    /// A TextStream: its number among the open files.
    TextStream(i32),
    /// A File or a Folder of FileSystemObject (the resolved path).
    FsoFile(std::path::PathBuf),
    FsoFolder(std::path::PathBuf),
    /// A folder's Files and SubFolders.
    FsoFiles(std::path::PathBuf),
    FsoFolders(std::path::PathBuf),
}

impl Part {
    pub fn type_name(&self) -> &'static str {
        match self {
            Part::Names(_) => "Names",
            Part::Name(..) => "Name",
            Part::Tab(_) => "Tab",
            Part::Hyperlinks(_) => "Hyperlinks",
            Part::Hyperlink(..) => "Hyperlink",
            Part::Comments(_) => "Comments",
            Part::Comment(..) => "Comment",
            Part::Validation(_) => "Validation",
            Part::FormatConditions(_) => "FormatConditions",
            Part::Condition(..) => "FormatCondition",
            Part::ConditionFont(..) => "Font",
            Part::ConditionInterior(..) => "Interior",
            Part::Criteria(..) => "ColorScaleCriteria",
            Part::Criterion(..) => "ColorScaleCriterion",
            Part::CfColor(..) => "FormatColor",
            Part::IconSets => "IconSets",
            Part::IconSet(_) => "IconSet",
            Part::PageSetup(_) => "PageSetup",
            Part::HfPage(..) => "Page",
            Part::HfSection(..) => "HeaderFooter",
            Part::PageBreaks(_, true) => "HPageBreaks",
            Part::PageBreaks(_, false) => "VPageBreaks",
            Part::PageBreak(_, true, _) => "HPageBreak",
            Part::PageBreak(_, false, _) => "VPageBreak",
            Part::ListObjects(_) => "ListObjects",
            Part::ListObject(_) => "ListObject",
            Part::ListColumns(_) => "ListColumns",
            Part::ListColumn(..) => "ListColumn",
            Part::ListRows(_) => "ListRows",
            Part::ListRow(..) => "ListRow",
            Part::Characters(..) => "Characters",
            Part::CharsFont(..) => "Font",
            Part::FileDialog(_) => "FileDialog",
            Part::DialogFilters(_) => "FileDialogFilters",
            Part::DialogFilter(..) => "FileDialogFilter",
            Part::DialogItems(_) => "FileDialogSelectedItems",
            Part::PivotTables(_) => "PivotTables",
            Part::PivotTable(_) => "PivotTable",
            Part::PivotCache(_) => "PivotCache",
            Part::Styles => "Styles",
            Part::SheetSort(_) => "Sort",
            Part::SortFields(_) => "SortFields",
            Part::SortField(..) => "SortField",
            Part::Style(_) => "Style",
            Part::StyleFont(_) => "Font",
            Part::StyleInterior(_) => "Interior",
            Part::Fso => "FileSystemObject",
            Part::TextStream(_) => "TextStream",
            Part::FsoFile(_) => "File",
            Part::FsoFolder(_) => "Folder",
            Part::FsoFiles(_) => "Files",
            Part::FsoFolders(_) => "Folders",
            Part::Shapes(_) => "Shapes",
            Part::Shape(..) => "Shape",
            Part::ShapeText(_, _, false) => "TextFrame",
            Part::ShapeText(_, _, true) => "Characters",
            Part::ShapeFont(..) => "Font",
            Part::ShapeFill(_, _, false) => "FillFormat",
            Part::ShapeFill(_, _, true) => "LineFormat",
            Part::ShapeColor(..) => "ColorFormat",
            Part::ChartObjects(_) => "ChartObjects",
            Part::ChartObject(..) => "ChartObject",
            Part::Chart(..) => "Chart",
            Part::ChartTitle(_, _, 0) => "ChartTitle",
            Part::ChartTitle(..) => "AxisTitle",
            Part::ChartAxis(..) => "Axis",
            Part::TickLabels(..) => "TickLabels",
            Part::Legend(..) => "Legend",
            Part::SeriesCollection(..) => "SeriesCollection",
            Part::Series(..) => "Series",
        }
    }
}

/// An object of a class module: the module and its own variables.
pub struct Instance {
    pub module: usize,
    pub class: Rc<str>,
    pub vars: RefCell<HashMap<String, crate::interp::Slot>>,
    /// A UserForm's controls and window.
    pub form: Option<RefCell<crate::forms::View>>,
}

impl std::fmt::Debug for Instance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Instance({})", self.class)
    }
}

/// Objects holding objects (a linked list of a million nodes) are let go
/// one by one, not by nested drops that would overflow the stack.
impl Drop for Instance {
    fn drop(&mut self) {
        let mut stack = Vec::new();
        for (_, s) in self.vars.get_mut().drain() {
            if let Ok(var) = Rc::try_unwrap(s) {
                let_go(var.into_inner().val, &mut stack);
            }
        }
        while let Some(i) = stack.pop() {
            if let Ok(mut i) = Rc::try_unwrap(i) {
                for (_, s) in i.vars.get_mut().drain() {
                    if let Ok(var) = Rc::try_unwrap(s) {
                        let_go(var.into_inner().val, &mut stack);
                    }
                }
            }
        }
    }
}

/// The instances held only by `v`, put aside to be let go by the caller.
fn let_go(v: Value, stack: &mut Vec<Rc<Instance>>) {
    match v {
        Value::Object(Obj::Instance(i)) => stack.push(i),
        Value::Object(Obj::Collection(c)) => {
            if let Ok(c) = Rc::try_unwrap(c) {
                for (_, v) in c.into_inner().items {
                    let_go(v, stack);
                }
            }
        }
        Value::Object(Obj::Dictionary(d)) => {
            if let Ok(d) = Rc::try_unwrap(d) {
                let d = d.into_inner();
                for v in d.items.into_iter().chain(d.keys) {
                    let_go(v, stack);
                }
            }
        }
        Value::Array(a) => {
            if let Ok(a) = Rc::try_unwrap(a) {
                for v in a.data {
                    let_go(v, stack);
                }
            }
        }
        Value::Record(r) => {
            if let Ok(r) = Rc::try_unwrap(r) {
                for f in r.fields {
                    let_go(f.val, stack);
                }
            }
        }
        _ => {}
    }
}

impl Obj {
    pub fn type_name(&self) -> &'static str {
        match self {
            Obj::Application => "Application",
            Obj::Workbook => "Workbook",
            Obj::Sheets => "Sheets",
            Obj::Sheet(_) => "Worksheet",
            Obj::Range(_) => "Range",
            Obj::Font(_) => "Font",
            Obj::Interior(_) => "Interior",
            Obj::Border(_, 0) => "Borders",
            Obj::Border(..) => "Border",
            Obj::Window => "Window",
            Obj::WorksheetFunction => "WorksheetFunction",
            Obj::Err => "ErrObject",
            Obj::Collection(_) => "Collection",
            Obj::Dictionary(_) => "Dictionary",
            Obj::Module(_) => "Module",
            Obj::Multi(items) => items.first().map_or("Range", Obj::type_name),
            Obj::Instance(_) => "Object",
            Obj::Part(p) => p.type_name(),
            Obj::Control(f, i) => f.form.as_ref().and_then(|v| v.borrow().items.get(*i).map(|it| it.c.kind.type_name())).unwrap_or("Control"),
            Obj::Controls(_, _, false) => "Controls",
            Obj::Controls(_, _, true) => "Pages",
            Obj::ControlFont(..) => "Font",
            Obj::Workbooks => "Workbooks",
            Obj::In(_, o) => o.type_name(),
            Obj::Word(o) => o.type_name(),
            Obj::Access(o) => o.type_name(),
        }
    }

    /// `Is`: the same object.
    pub fn same(&self, other: &Obj) -> bool {
        match (self, other) {
            (Obj::Collection(a), Obj::Collection(b)) => Rc::ptr_eq(a, b),
            (Obj::Dictionary(a), Obj::Dictionary(b)) => Rc::ptr_eq(a, b),
            (Obj::Sheet(a), Obj::Sheet(b)) | (Obj::Module(a), Obj::Module(b)) => a == b,
            (Obj::Border(a, i), Obj::Border(b, j)) => a.sheet == b.sheet && a.range == b.range && i == j,
            (Obj::Window, Obj::Window) => true,
            (Obj::Instance(a), Obj::Instance(b)) => Rc::ptr_eq(a, b),
            (Obj::Control(a, i), Obj::Control(b, j)) => Rc::ptr_eq(a, b) && i == j,
            (Obj::Part(a), Obj::Part(b)) => a == b,
            (Obj::In(a, x), Obj::In(b, y)) => a == b && x.same(y),
            (Obj::Word(a), Obj::Word(b)) => a.same(b),
            (Obj::Access(a), Obj::Access(b)) => a.same(b),
            (Obj::Multi(a), Obj::Multi(b)) => a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.same(y)),
            (Obj::Range(a), Obj::Range(b)) | (Obj::Font(a), Obj::Font(b)) | (Obj::Interior(a), Obj::Interior(b)) => a.sheet == b.sheet && a.range == b.range,
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
        }
    }
}

/// A key of a Collection or Dictionary: strings (case folded when the
/// comparison ignores it) and numbers.
fn key_text(v: &Value, fold: bool) -> R<String> {
    Ok(match v {
        Value::Str(s) => {
            if fold {
                format!("s:{}", s.to_lowercase())
            } else {
                format!("s:{s}")
            }
        }
        Value::Empty => "e:".into(),
        Value::Bool(b) => format!("n:{}", if *b { -1 } else { 0 }),
        Value::Object(_) | Value::Nothing | Value::Array(_) => return Err(Error::type_mismatch()),
        v => format!("n:{}", number_text(v.to_f64()?)),
    })
}

/// VBA's Collection: items in order, some with a (case-insensitive) key.
#[derive(Debug, Default)]
pub struct Collection {
    pub items: Vec<(Option<String>, Value)>,
}

impl Collection {
    /// Position of the item at a 1-based index or with a key.
    pub fn find(&self, at: &Value) -> R<usize> {
        match at {
            Value::Str(k) => {
                let k = k.to_lowercase();
                self.items.iter().position(|(key, _)| key.as_deref() == Some(k.as_str())).ok_or_else(Error::invalid_call)
            }
            v => {
                let i = v.to_i64()?;
                if i < 1 || i as usize > self.items.len() {
                    return Err(Error::subscript());
                }
                Ok(i as usize - 1)
            }
        }
    }

    pub fn add(&mut self, item: Value, key: Option<String>, before: Option<usize>, after: Option<usize>) -> R<()> {
        let key = key.map(|k| k.to_lowercase());
        if let Some(k) = &key {
            if self.items.iter().any(|(x, _)| x.as_deref() == Some(k.as_str())) {
                return Err(Error::new(457, "This key is already associated with an element of this collection"));
            }
        }
        let at = match (before, after) {
            (Some(b), _) => b,
            (None, Some(a)) => a + 1,
            _ => self.items.len(),
        };
        self.items.insert(at.min(self.items.len()), (key, item));
        Ok(())
    }
}

/// Scripting.Dictionary: pairs in the order they were added.
#[derive(Debug, Default)]
pub struct Dict {
    pub keys: Vec<Value>,
    pub items: Vec<Value>,
    index: HashMap<String, usize>,
    /// CompareMode = vbTextCompare.
    pub text: bool,
}

impl Dict {
    pub fn find(&self, key: &Value) -> R<Option<usize>> {
        Ok(self.index.get(&key_text(key, self.text)?).copied())
    }

    pub fn insert(&mut self, key: Value, item: Value) -> R<()> {
        let k = key_text(&key, self.text)?;
        match self.index.get(&k) {
            Some(&i) => self.items[i] = item,
            None => {
                self.index.insert(k, self.keys.len());
                self.keys.push(key);
                self.items.push(item);
            }
        }
        Ok(())
    }

    pub fn add(&mut self, key: Value, item: Value) -> R<()> {
        if self.find(&key)?.is_some() {
            return Err(Error::new(457, "This key is already associated with an element of this collection"));
        }
        self.insert(key, item)
    }

    pub fn remove(&mut self, key: &Value) -> R<()> {
        let i = self.find(key)?.ok_or_else(|| Error::new(32811, "Element not found"))?;
        self.keys.remove(i);
        self.items.remove(i);
        self.reindex();
        Ok(())
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.items.clear();
        self.index.clear();
    }

    pub fn reindex(&mut self) {
        self.index.clear();
        for (i, k) in self.keys.iter().enumerate() {
            if let Ok(t) = key_text(k, self.text) {
                self.index.insert(t, i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_keys() {
        let mut d = Dict::default();
        d.add(Value::str("a"), Value::Long(1)).unwrap();
        d.add(Value::Long(2), Value::Long(2)).unwrap();
        assert!(d.add(Value::str("a"), Value::Empty).is_err());
        assert_eq!(d.find(&Value::Double(2.0)).unwrap(), Some(1));
        assert_eq!(d.find(&Value::str("A")).unwrap(), None);
        d.text = true;
        d.reindex();
        assert_eq!(d.find(&Value::str("A")).unwrap(), Some(0));
        d.remove(&Value::str("a")).unwrap();
        assert_eq!(d.keys.len(), 1);
    }

    #[test]
    fn collection_items() {
        let mut c = Collection::default();
        c.add(Value::Long(1), Some("One".into()), None, None).unwrap();
        c.add(Value::Long(0), None, Some(0), None).unwrap();
        assert_eq!(c.find(&Value::str("one")).unwrap(), 1);
        assert_eq!(c.find(&Value::Long(1)).unwrap(), 0);
        assert_eq!(c.find(&Value::Long(3)).unwrap_err().number, 9);
    }
}
