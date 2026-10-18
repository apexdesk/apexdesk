//! The allocator of apex-bench: the system one, counting the bytes in use.
//! A child process of the fuzzer sets a limit; an allocation past it (or
//! one the system refuses) prints where it came from and ends the process, so a
//! file that makes a reader ask for gigabytes is reported with a place in
//! the code on every platform. `big` reads the peak.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub struct Counting;

static IN_USE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// Limit in bytes (0: none).
static LIMIT: AtomicUsize = AtomicUsize::new(0);
/// Set while the failure is being reported (its own allocations pass).
static REPORTING: AtomicBool = AtomicBool::new(false);

#[global_allocator]
static GLOBAL: Counting = Counting;

pub fn set_limit(bytes: usize) {
    LIMIT.store(bytes, Ordering::Relaxed);
}

pub fn peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}

pub fn reset_peak() {
    PEAK.store(IN_USE.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// Prints the failure with a backtrace and aborts the process.
#[cold]
fn fail(size: usize, refused: bool) -> ! {
    if REPORTING.swap(true, Ordering::SeqCst) {
        std::process::abort();
    }
    let in_use = IN_USE.load(Ordering::Relaxed);
    let why = if refused { "refused by the system" } else { "over the limit" };
    let bt = std::backtrace::Backtrace::force_capture();
    eprintln!("APEX-CRASH memory: allocation of {size} bytes {why} ({in_use} bytes in use)\n{bt}");
    std::process::exit(super::child::EXIT_MEMORY);
}

fn check(size: usize) {
    let limit = LIMIT.load(Ordering::Relaxed);
    if limit != 0 && !REPORTING.load(Ordering::Relaxed) && IN_USE.load(Ordering::Relaxed).saturating_add(size) > limit {
        fail(size, false);
    }
}

fn add(size: usize) {
    let now = IN_USE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        check(layout.size());
        let p = unsafe { System.alloc(layout) };
        if p.is_null() {
            if LIMIT.load(Ordering::Relaxed) != 0 {
                fail(layout.size(), true);
            }
        } else {
            add(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        check(layout.size());
        let p = unsafe { System.alloc_zeroed(layout) };
        if p.is_null() {
            if LIMIT.load(Ordering::Relaxed) != 0 {
                fail(layout.size(), true);
            }
        } else {
            add(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        IN_USE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size > layout.size() {
            check(new_size - layout.size());
        }
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if p.is_null() {
            if LIMIT.load(Ordering::Relaxed) != 0 {
                fail(new_size, true);
            }
        } else if new_size >= layout.size() {
            add(new_size - layout.size());
        } else {
            IN_USE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        p
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_the_peak() {
        let before = super::peak();
        let v = vec![0u8; 8 << 20];
        assert!(super::peak() >= before.min(super::peak()) && super::peak() >= v.len());
        drop(v);
    }
}
