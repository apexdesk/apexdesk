//! Calculation order known only while calculating. The dependency graph
//! comes from the references written in formulas, which is too much for
//! some (`OFFSET(A12,-1,0)` in A12 and `ROW(A1:A9)` in A5 don't read
//! themselves) and too little for others (what OFFSET or INDIRECT reach).
//! As Excel does, a formula that reads a formula not calculated yet waits
//! for it, and only a formula that ends up reading itself is circular.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

/// Formulas still to calculate, and the first of them read by the formula
/// being calculated (when watching).
#[derive(Default)]
pub(crate) struct Pending {
    on: AtomicBool,
    state: Mutex<(HashSet<CellKey>, Option<CellKey>)>,
}

impl Clone for Pending {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl Pending {
    pub(crate) fn set(&self, cells: HashSet<CellKey>) {
        if let Ok(mut s) = self.state.lock() {
            *s = (cells, None);
        }
    }

    pub(crate) fn done(&self, k: CellKey) {
        if let Ok(mut s) = self.state.lock() {
            s.0.remove(&k);
        }
    }

    pub(crate) fn clear(&self) {
        self.on.store(false, Ordering::Relaxed);
        self.set(HashSet::default());
    }

    /// A cell was read: noted when it is a formula still to calculate.
    #[inline]
    pub(crate) fn read(&self, k: CellKey) {
        if self.on.load(Ordering::Relaxed)
            && let Ok(mut s) = self.state.lock()
            && s.1.is_none()
            && s.0.contains(&k)
        {
            s.1 = Some(k);
        }
    }

    pub(crate) fn watching(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    fn watch(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.1 = None;
        }
        self.on.store(true, Ordering::Relaxed);
    }

    /// Stops watching: the pending formula read, if any.
    fn unwatch(&self) -> Option<CellKey> {
        self.on.store(false, Ordering::Relaxed);
        self.state.lock().ok().and_then(|mut s| s.1.take())
    }
}

impl Workbook {
    /// Calculates `k` noting whether it read a formula still to calculate
    /// (other than itself): Err with that formula, and the value is
    /// thrown away.
    pub(super) fn eval_watching(&self, k: CellKey) -> Result<Value, CellKey> {
        self.pending_cells.watch();
        let v = self.eval_cell(k);
        match self.pending_cells.unwatch() {
            Some(p) => Err(p),
            None => Ok(v),
        }
    }

    /// The formulas of what looked like a cycle, calculated in the order
    /// their reads ask for; the ones that really read themselves (through
    /// others or not) are circular and get #CIRC!.
    pub(super) fn resolve_cycles(&mut self, rest: &[CellKey], spilled: &mut Vec<CellKey>) {
        let mut left: HashSet<CellKey> = rest.iter().copied().collect();
        self.pending_cells.set(left.clone());
        // First cells on top: the sheet order is the usual order of reads.
        let mut stack: Vec<CellKey> = rest.iter().rev().copied().collect();
        let mut waiting: HashSet<CellKey> = HashSet::default();
        let mut budget = rest.len().saturating_mul(50).saturating_add(10_000);
        while let Some(&k) = stack.last() {
            if !left.contains(&k) {
                stack.pop();
                waiting.remove(&k);
                continue;
            }
            if budget == 0 {
                break;
            }
            budget -= 1;
            let v = match self.eval_watching(k) {
                // It needs `p` first.
                Err(p) if p != k && !waiting.contains(&p) => {
                    waiting.insert(k);
                    stack.push(p);
                    continue;
                }
                // It reads itself.
                Err(_) => Value::Error(ErrorKind::Circular),
                Ok(v) => v,
            };
            let v = self.place(k, v, spilled);
            if let Some(Content::Formula { value, .. }) = self.sheets[k.sheet as usize].get_mut(k.cell()).map(|c| &mut c.content) {
                *value = v;
            }
            left.remove(&k);
            self.pending_cells.done(k);
            stack.pop();
            waiting.remove(&k);
        }
        // Too long to sort out: what is left counts as circular.
        for k in left {
            if let Some(Content::Formula { value, .. }) = self.sheets[k.sheet as usize].get_mut(k.cell()).map(|c| &mut c.content) {
                *value = Value::Error(ErrorKind::Circular);
            }
        }
        self.pending_cells.clear();
    }
}
