//! The formula of a cell. Cells filled from one formula (down a column or
//! across a row) share its tree: each keeps how far its copy moved, as a
//! copy and paste moves it. A million filled formulas cost a million small
//! records instead of a million trees.
//!
//! What a cell holds does not depend on where the cell is: sorting or
//! moving it keeps its formula, as before. A copy is shared only when
//! moving the shared tree gives exactly the cell's own formula.

use std::borrow::Cow;
use std::sync::Arc;

use crate::formula::Expr;

#[derive(Clone)]
pub struct CellFormula(Arc<Placed>);

struct Placed {
    base: Arc<Expr>,
    /// How far this copy moved from `base`.
    drow: i32,
    dcol: i32,
}

impl CellFormula {
    pub fn new(expr: Arc<Expr>) -> Self {
        CellFormula(Arc::new(Placed { base: expr, drow: 0, dcol: 0 }))
    }

    fn moved(&self) -> bool {
        self.0.drow != 0 || self.0.dcol != 0
    }

    /// The formula (built from the shared tree for a moved copy).
    pub fn expr(&self) -> Cow<'_, Expr> {
        if self.moved() { Cow::Owned(self.build()) } else { Cow::Borrowed(&self.0.base) }
    }

    /// The shared tree, as written where it was first: for what doesn't
    /// depend on where the copy is (functions, names, other books), as a
    /// quick test before building the copy's own formula.
    pub fn shape(&self) -> &Expr {
        &self.0.base
    }

    /// The formula, to keep.
    pub fn to_arc(&self) -> Arc<Expr> {
        if self.moved() { Arc::new(self.build()) } else { self.0.base.clone() }
    }

    fn build(&self) -> Expr {
        let mut e = (*self.0.base).clone();
        e.offset(self.0.drow as i64, self.0.dcol as i64);
        e
    }

    /// The formula to change: a shared copy becomes the cell's own.
    pub fn make_mut(&mut self) -> &mut Expr {
        if self.moved() || Arc::strong_count(&self.0) > 1 || Arc::strong_count(&self.0.base) > 1 {
            *self = CellFormula::new(Arc::new(self.build_or_clone()));
        }
        let placed = Arc::get_mut(&mut self.0).expect("own record");
        Arc::get_mut(&mut placed.base).expect("own tree")
    }

    fn build_or_clone(&self) -> Expr {
        if self.moved() { self.build() } else { (*self.0.base).clone() }
    }

    /// `expr` as a copy of this formula moved `drow` rows and `dcol`
    /// columns further, sharing its tree, when it is exactly that.
    pub fn follow(&self, drow: i32, dcol: i32, expr: &Expr) -> Option<CellFormula> {
        let (drow, dcol) = (self.0.drow.checked_add(drow)?, self.0.dcol.checked_add(dcol)?);
        let mut e = (*self.0.base).clone();
        e.offset(drow as i64, dcol as i64);
        (e == *expr).then(|| CellFormula(Arc::new(Placed { base: self.0.base.clone(), drow, dcol })))
    }

    /// This formula copied `drow` rows and `dcol` columns away (sharing
    /// the tree when that gives the same as copying it whole).
    pub fn offset(&self, drow: i64, dcol: i64) -> CellFormula {
        let mut e = self.expr().into_owned();
        e.offset(drow, dcol);
        let shared = i32::try_from(drow).ok().zip(i32::try_from(dcol).ok()).and_then(|(r, c)| self.follow(r, c, &e));
        shared.unwrap_or_else(|| e.into())
    }

    /// Whether the two share one tree (for tests and statistics).
    pub fn shares_with(&self, other: &CellFormula) -> bool {
        Arc::ptr_eq(&self.0.base, &other.0.base)
    }
}

impl From<Arc<Expr>> for CellFormula {
    fn from(e: Arc<Expr>) -> Self {
        CellFormula::new(e)
    }
}

impl From<Expr> for CellFormula {
    fn from(e: Expr) -> Self {
        CellFormula::new(Arc::new(e))
    }
}

impl PartialEq for CellFormula {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (&self.0, &other.0);
        if Arc::ptr_eq(&a.base, &b.base) && a.drow == b.drow && a.dcol == b.dcol {
            return true;
        }
        *self.expr() == *other.expr()
    }
}

impl std::fmt::Debug for CellFormula {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.expr().fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::parse_canonical;

    #[test]
    fn copies_share_the_tree_and_read_as_their_own() {
        let a = CellFormula::from(parse_canonical("A2*2+$B$1").unwrap());
        let b = a.follow(1, 0, &parse_canonical("A3*2+$B$1").unwrap()).unwrap();
        let c = b.follow(1, 0, &parse_canonical("A4*2+$B$1").unwrap()).unwrap();
        assert!(a.shares_with(&c));
        assert_eq!(c.expr().to_canonical(), "A4*2+$B$1");
        assert_eq!(*c.to_arc(), parse_canonical("A4*2+$B$1").unwrap());
        // Not the same formula moved: no sharing.
        assert!(b.follow(1, 0, &parse_canonical("A4*2+$B$2").unwrap()).is_none());
        assert!(b.follow(1, 0, &parse_canonical("A5*2+$B$1").unwrap()).is_none());
    }

    #[test]
    fn changing_a_copy_leaves_the_others() {
        let a = CellFormula::from(parse_canonical("SUM(A1:A2)").unwrap());
        let mut b = a.follow(0, 1, &parse_canonical("SUM(B1:B2)").unwrap()).unwrap();
        let keep = b.clone();
        b.make_mut().offset(5, 0);
        assert_eq!(b.expr().to_canonical(), "SUM(B6:B7)");
        assert_eq!(keep.expr().to_canonical(), "SUM(B1:B2)");
        assert_eq!(a.expr().to_canonical(), "SUM(A1:A2)");
        assert!(!a.shares_with(&b));
    }

    #[test]
    fn refs_that_leave_the_sheet_are_not_shared_wrongly() {
        // Moved up from row 1, A1 would be #REF!: only an equal formula shares.
        let a = CellFormula::from(parse_canonical("A2").unwrap());
        assert!(a.follow(-1, 0, &parse_canonical("A1").unwrap()).is_some());
        assert!(a.follow(-2, 0, &parse_canonical("A1").unwrap()).is_none());
    }
}
