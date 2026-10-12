//! Memory a cell costs in the model (heap bytes counted by the allocator):
//! numbers, texts and formulas of a big column, as a file loads them.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use apex_sheet::{CellRef, Content, Value, Workbook};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LIVE.fetch_add(l.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        LIVE.fetch_add(new as isize - l.size() as isize, Ordering::Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static A: Counting = Counting;

const N: u32 = 100_000;

/// Heap bytes per cell of a workbook made by `fill` (one sheet of N rows).
fn per_cell(fill: impl Fn(&mut Workbook, u32)) -> f64 {
    let mut wb = Workbook::empty();
    wb.sheets.push(apex_sheet::Sheet::new("S"));
    let before = LIVE.load(Ordering::Relaxed);
    for r in 0..N {
        fill(&mut wb, r);
    }
    wb.finish_load();
    let bytes = LIVE.load(Ordering::Relaxed) - before;
    let cells = wb.sheets[0].cell_count() as f64;
    let per = bytes as f64 / cells;
    drop(wb);
    per
}

// One test, so that others running at the same time don't count.
#[test]
fn bytes_per_cell() {
    let num = per_cell(|wb, r| wb.load_cell(0, CellRef::new(r, 0), Content::Value(Value::Number(r as f64)), 0));
    let text = per_cell(|wb, r| wb.load_cell(0, CellRef::new(r, 0), Content::Value(Value::text(format!("item {}", r % 100))), 0));
    // As the readers load repeated texts: one copy shared by the cells.
    let texts = std::cell::RefCell::new(apex_sheet::Interner::default());
    let shared = per_cell(|wb, r| wb.load_cell(0, CellRef::new(r, 0), Content::Value(texts.borrow_mut().text(&format!("item {}", r % 100))), 0));
    let formula = per_cell(|wb, r| {
        wb.load_cell(0, CellRef::new(r, 0), Content::Value(Value::Number(r as f64)), 0);
        let e = apex_sheet::formula::parse_canonical(&format!("A{}*2+1", r + 1)).unwrap();
        wb.load_cell(0, CellRef::new(r, 1), Content::Formula { expr: (e).into(), value: Value::Empty }, 0);
    }) * 2.0 - num;
    eprintln!("bytes per cell: number {num:.1}, text {text:.1}, shared text {shared:.1}, formula {formula:.1}");
    // They were 100, 124 and 457 with a map entry per cell.
    assert!(num < 60.0, "number cell: {num:.1} bytes");
    assert!(text < 90.0, "text cell: {text:.1} bytes");
    assert!(shared < 60.0, "shared text cell: {shared:.1} bytes");
    assert!(formula < 400.0, "formula cell: {formula:.1} bytes");
}
