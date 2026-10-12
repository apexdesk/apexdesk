//! Defined names: `Sales` standing for `Sheet1!$B$2:$B$50`, a constant
//! (`=0.21`) or any formula, LAMBDA functions included.

use std::sync::Arc;

use crate::addr::{CellRef, Range};
use crate::formula::{CellAddr, Expr, RangeKind};

#[derive(Clone, Debug, PartialEq)]
pub struct DefinedName {
    pub name: String,
    /// The sheet the name belongs to (`None`: the whole workbook). A local
    /// name hides a workbook name with the same text on its sheet.
    pub scope: Option<String>,
    /// What the name stands for. Relative references count from A1 and
    /// move with the cell that uses the name, as in Excel.
    pub expr: Arc<Expr>,
    pub comment: String,
    pub hidden: bool,
}

impl DefinedName {
    pub fn new(name: &str, scope: Option<String>, expr: Expr) -> DefinedName {
        DefinedName { name: name.to_string(), scope, expr: Arc::new(expr), comment: String::new(), hidden: false }
    }

    /// The cells the name stands for when it is a plain reference, with
    /// the sheet name (`None` for a sheet-less reference).
    pub fn range(&self) -> Option<(Option<Arc<str>>, Range)> {
        match &*self.expr {
            Expr::Ref { sheet, addr } => Some((sheet.clone(), Range::single(CellRef::new(addr.row, addr.col)))),
            Expr::Range { sheet, a, b, .. } => Some((sheet.clone(), Range::new(CellRef::new(a.row, a.col), CellRef::new(b.row, b.col)))),
            Expr::Paren(_) => None,
            _ => None,
        }
    }

    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }
}

/// Why a name can't be used, as an i18n key.
pub fn check_name(s: &str) -> Result<(), &'static str> {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return Err("names.err_empty") };
    if s.chars().count() > 255 {
        return Err("names.err_long");
    }
    if !(first.is_alphabetic() || first == '_' || first == '\\') {
        return Err("names.err_start");
    }
    if !chars.all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '\\') {
        return Err("names.err_chars");
    }
    if looks_like_reference(s) {
        return Err("names.err_ref");
    }
    let up = s.to_uppercase();
    if matches!(up.as_str(), "TRUE" | "FALSE" | "VERDADERO" | "FALSO") {
        return Err("names.err_ref");
    }
    Ok(())
}

/// `A1`, `XFD1048576`, `R`, `C`, `R1C1`, `RC[2]`: text Excel reads as a
/// reference and so can't be a name.
fn looks_like_reference(s: &str) -> bool {
    if crate::formula::parse_addr(s).is_some() {
        return true;
    }
    let up = s.to_ascii_uppercase();
    if up == "R" || up == "C" {
        return true;
    }
    // R1C1 style: R<digits>C<digits>, either number optional.
    let b = up.as_bytes();
    if b.first() == Some(&b'R') {
        let mut i = 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i < b.len() && b[i] == b'C' {
            let rest = &b[i + 1..];
            return rest.iter().all(u8::is_ascii_digit);
        }
        return i == b.len();
    }
    if b.first() == Some(&b'C') {
        return b[1..].iter().all(u8::is_ascii_digit);
    }
    false
}

/// An absolute reference to `range` on `sheet`, the form names use.
pub fn absolute_ref(sheet: &str, range: Range) -> Expr {
    let addr = |c: CellRef| CellAddr { row: c.row, col: c.col, row_abs: true, col_abs: true };
    let sheet = Some(Arc::from(sheet));
    if range.is_single() {
        Expr::Ref { sheet, addr: addr(range.start) }
    } else {
        Expr::Range { sheet, a: addr(range.start), b: addr(range.end), kind: RangeKind::Cells }
    }
}

/// Makes a name out of a header text: spaces become `_`, invalid
/// characters go, and a leading digit gets a `_` in front.
pub fn name_from_label(label: &str) -> Option<String> {
    let mut s: String = label.trim().chars().map(|c| if c.is_whitespace() || c == '-' { '_' } else { c }).filter(|c| c.is_alphanumeric() || *c == '_' || *c == '.').collect();
    if s.is_empty() {
        return None;
    }
    if !s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') || looks_like_reference(&s) {
        s.insert(0, '_');
    }
    check_name(&s).ok().map(|_| s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(check_name("Sales").is_ok());
        assert!(check_name("Tax_2024").is_ok());
        assert!(check_name("my.name").is_ok());
        assert!(check_name("_x").is_ok());
        assert!(check_name("A1").is_err());
        assert!(check_name("XFD1").is_err());
        assert!(check_name("R1C1").is_err());
        assert!(check_name("R").is_err());
        assert!(check_name("1abc").is_err());
        assert!(check_name("a b").is_err());
        assert!(check_name("").is_err());
        assert!(check_name("TRUE").is_err());
        assert_eq!(name_from_label("Total ventas").as_deref(), Some("Total_ventas"));
        assert_eq!(name_from_label("2024").as_deref(), Some("_2024"));
        assert_eq!(name_from_label("A1").as_deref(), Some("_A1"));
    }
}
