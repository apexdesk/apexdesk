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
    WorksheetFunction,
    Err,
    Collection(Rc<RefCell<Collection>>),
    Dictionary(Rc<RefCell<Dict>>),
    /// A module by name (its public procedures and variables).
    Module(usize),
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
            Obj::WorksheetFunction => "WorksheetFunction",
            Obj::Err => "ErrObject",
            Obj::Collection(_) => "Collection",
            Obj::Dictionary(_) => "Dictionary",
            Obj::Module(_) => "Module",
        }
    }

    /// `Is`: the same object.
    pub fn same(&self, other: &Obj) -> bool {
        match (self, other) {
            (Obj::Collection(a), Obj::Collection(b)) => Rc::ptr_eq(a, b),
            (Obj::Dictionary(a), Obj::Dictionary(b)) => Rc::ptr_eq(a, b),
            (Obj::Sheet(a), Obj::Sheet(b)) | (Obj::Module(a), Obj::Module(b)) => a == b,
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
