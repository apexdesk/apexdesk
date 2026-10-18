//! The data model of a workbook: relationships between its named tables
//! (a key column of the "many" table points at a unique key of the "one"
//! table) and measures written in a subset of DAX. Pivot tables built on
//! the model can group by the columns of related tables and show measures
//! evaluated in the filter context of each cell.
//!
//! The model lives in `analysis::State::model`, so editing it is undoable;
//! the tables themselves are the workbook's tables (read in `data`).

pub mod dax;
pub mod data;
pub mod eval;

pub use dax::{DaxError, DaxErrorKind};
pub use data::ModelData;

/// A one-to-many relationship: every row of `from_table` relates to the row
/// of `to_table` whose `to_column` equals its `from_column`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Relationship {
    /// The "many" side.
    pub from_table: String,
    pub from_column: String,
    /// The "one" side (unique keys).
    pub to_table: String,
    pub to_column: String,
    /// Inactive relationships are kept but not followed.
    pub active: bool,
}

impl Relationship {
    pub fn new(from_table: &str, from_column: &str, to_table: &str, to_column: &str) -> Relationship {
        Relationship { from_table: from_table.into(), from_column: from_column.into(), to_table: to_table.into(), to_column: to_column.into(), active: true }
    }

    /// Links the same two columns as `o`.
    pub fn same_link(&self, o: &Relationship) -> bool {
        let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        eq(&self.from_table, &o.from_table) && eq(&self.from_column, &o.from_column) && eq(&self.to_table, &o.to_table) && eq(&self.to_column, &o.to_column)
    }
}

/// A named DAX formula evaluated in the filter context of a pivot cell.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measure {
    pub name: String,
    /// The table the measure is listed under.
    pub table: String,
    /// Without a leading `=`.
    pub formula: String,
    /// Number format of its values (empty: General).
    pub format: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DataModel {
    pub relationships: Vec<Relationship>,
    pub measures: Vec<Measure>,
}

impl DataModel {
    pub fn is_empty(&self) -> bool {
        self.relationships.is_empty() && self.measures.is_empty()
    }

    pub fn measure(&self, name: &str) -> Option<&Measure> {
        self.measures.iter().find(|m| m.name.eq_ignore_ascii_case(name))
    }

    /// Follows a table rename (relationships and measure homes; formulas
    /// are rewritten by [`dax::rename_table`]).
    pub fn rename_table(&mut self, old: &str, new: &str) {
        let fix = |s: &mut String| {
            if s.eq_ignore_ascii_case(old) {
                *s = new.to_string();
            }
        };
        for r in &mut self.relationships {
            fix(&mut r.from_table);
            fix(&mut r.to_table);
        }
        for m in &mut self.measures {
            fix(&mut m.table);
            m.formula = dax::rename_table(&m.formula, old, new);
        }
    }

    /// Drops the relationships of a table that is gone.
    pub fn remove_table(&mut self, name: &str) {
        self.relationships.retain(|r| !r.from_table.eq_ignore_ascii_case(name) && !r.to_table.eq_ignore_ascii_case(name));
    }
}

/// Relationships the data suggests: a column whose name matches a column
/// of another table where that column's values are unique and cover the
/// first column's values. Existing links are left out.
pub fn detect_relationships(data: &ModelData, existing: &[Relationship]) -> Vec<Relationship> {
    let mut out: Vec<Relationship> = Vec::new();
    for (ti, t) in data.tables.iter().enumerate() {
        for (ci, cname) in t.columns.iter().enumerate() {
            for (ui, u) in data.tables.iter().enumerate() {
                if ui == ti {
                    continue;
                }
                // Same name, or `ProductID` against `ID` of table Products.
                let Some(uc) = u.columns.iter().position(|n| n.eq_ignore_ascii_case(cname)).or_else(|| {
                    let low = cname.to_lowercase();
                    let tn = u.name.to_lowercase();
                    let stem = tn.strip_suffix('s').unwrap_or(&tn);
                    u.columns.iter().position(|n| {
                        let n = n.to_lowercase();
                        (n == "id" || n == "key") && (low == format!("{stem}{n}") || low == format!("{tn}{n}") || low == format!("{stem}_{n}"))
                    })
                }) else {
                    continue;
                };
                if !data.unique(ui, uc) || data.rows(ti) == 0 {
                    continue;
                }
                // Both unique: the bigger table is the "many" side.
                if data.unique(ti, ci) && (data.rows(ti) < data.rows(ui) || (data.rows(ti) == data.rows(ui) && ti > ui)) {
                    continue;
                }
                if !data.covers(ti, ci, ui, uc) {
                    continue;
                }
                let r = Relationship::new(&t.name, cname, &u.name, &u.columns[uc]);
                if existing.iter().chain(&out).any(|e| e.same_link(&r) || (e.from_table.eq_ignore_ascii_case(&r.from_table) && e.to_table.eq_ignore_ascii_case(&r.to_table))) {
                    continue;
                }
                out.push(r);
            }
        }
    }
    out
}

/// Why a relationship can't be added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelError {
    NoTable,
    NoColumn,
    SameTable,
    /// The "one" side has repeated keys.
    NotUnique,
    Duplicate,
}

/// Checks a relationship against the data and the ones already there
/// (`except`: the one being edited).
pub fn check_relationship(data: &ModelData, all: &[Relationship], r: &Relationship, except: Option<usize>) -> Result<(), RelError> {
    let ft = data.table(&r.from_table).ok_or(RelError::NoTable)?;
    let tt = data.table(&r.to_table).ok_or(RelError::NoTable)?;
    if ft == tt {
        return Err(RelError::SameTable);
    }
    data.column(ft, &r.from_column).ok_or(RelError::NoColumn)?;
    let tc = data.column(tt, &r.to_column).ok_or(RelError::NoColumn)?;
    if !data.unique(tt, tc) {
        return Err(RelError::NotUnique);
    }
    if all.iter().enumerate().any(|(i, o)| Some(i) != except && o.same_link(r)) {
        return Err(RelError::Duplicate);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
