//! Functions a workbook's macros give its cells (VBA `Public Function`s
//! of standard modules, called as `=MyFunc(A1)`). The program running
//! the macros puts them in [`crate::Workbook::udf`]; without it they are
//! unknown names (#NAME?, the saved value kept).

use crate::Workbook;
use crate::addr::{CellRef, Range};
use crate::value::Value;

/// An argument of a call from a cell.
#[derive(Clone, Debug)]
pub enum UdfArg {
    Value(Value),
    /// A reference (the function gets a Range).
    Range { sheet: usize, range: Range },
    /// Left out: `=F(1,,3)`.
    Missing,
}

pub trait Udf: Send + Sync {
    /// Whether cells may call function `name` (any case).
    fn has(&self, name: &str) -> bool;
    /// Whether `name` is recalculated with every change
    /// (`Application.Volatile`).
    fn is_volatile(&self, name: &str) -> bool;
    /// The functions cells may call, as the code writes them, with their
    /// parameters.
    fn names(&self) -> Vec<(String, Vec<String>)>;
    /// The value of `name(args)` called from `cell` of `sheet` (which it
    /// may read, not change).
    fn call(&self, wb: &Workbook, name: &str, args: Vec<UdfArg>, sheet: usize, cell: CellRef) -> Value;
}
