//! The format painter across windows: the format picked up in one
//! document, workbook or presentation is held here, and another of the
//! same kind takes it when it comes to the front, so it can be painted
//! there. Turned off in one (painted, Escape, the button), it is off in
//! all of them.

use std::sync::Arc;
#[cfg(not(test))]
use std::sync::Mutex;

use apex_doc::{Attrs, Paragraph};

/// A format picked up, in a form that doesn't depend on its file.
pub enum Paint {
    /// A paragraph with the format on its text, and what it needs from
    /// its document to look the same in another (see `Document::adopt`).
    /// `whole`: the paragraph's formatting goes too.
    Doc { sample: Paragraph, origin: Arc<apex_doc::Origin>, whole: bool },
    /// The formats of `rows` × `cols` cells, by rows.
    Cells { styles: Vec<apex_sheet::CellStyle>, rows: u32, cols: u32 },
    /// The look of text on a slide, and its paragraph's formatting and level.
    Slide { attrs: Attrs, para: Option<(apex_slides::ParaProps, u8)> },
}

#[derive(Clone)]
pub struct Held {
    /// Which pick-up this is (never 0).
    pub serial: u64,
    pub paint: Arc<Paint>,
    pub sticky: bool,
}

#[cfg(not(test))]
static HELD: Mutex<(u64, Option<Held>)> = Mutex::new((0, None));

#[cfg(not(test))]
fn with<R>(f: impl FnOnce(&mut (u64, Option<Held>)) -> R) -> R {
    f(&mut HELD.lock().unwrap_or_else(|e| e.into_inner()))
}

// Each test has its own painter (tests run side by side, one a thread).
#[cfg(test)]
thread_local! {
    static HELD: std::cell::RefCell<(u64, Option<Held>)> = const { std::cell::RefCell::new((0, None)) };
}

#[cfg(test)]
fn with<R>(f: impl FnOnce(&mut (u64, Option<Held>)) -> R) -> R {
    HELD.with_borrow_mut(f)
}

/// Holds a format just picked up; gives its serial.
pub fn hold(paint: Paint, sticky: bool) -> u64 {
    with(|(last, held)| {
        *last += 1;
        *held = Some(Held { serial: *last, paint: Arc::new(paint), sticky });
        *last
    })
}

pub fn held() -> Option<Held> {
    with(|(_, held)| held.clone())
}

/// Turns the painter off, if it still holds that pick-up.
pub fn release(serial: u64) {
    with(|(_, held)| {
        if held.as_ref().is_some_and(|h| h.serial == serial) {
            *held = None;
        }
    });
}

/// What a view does each frame with the painter it has (`on`: it has one;
/// `serial`: of the pick-up it has, 0 for none).
pub enum Sync {
    /// Nothing changed.
    Keep,
    /// Its painter was turned off in another window.
    Drop,
    /// Another window picked up a format: take it, if it is of its kind.
    Take(Held),
}

/// Keeps a view's painter and the shared one the same. A view that turned
/// its painter off turns it off for all.
pub fn sync(on: bool, serial: &mut u64) -> Sync {
    if !on && *serial != 0 {
        release(*serial);
        *serial = 0;
    }
    match held() {
        Some(h) if h.serial != *serial => {
            *serial = h.serial;
            Sync::Take(h)
        }
        None if *serial != 0 => {
            *serial = 0;
            Sync::Drop
        }
        _ => Sync::Keep,
    }
}
