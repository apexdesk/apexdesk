//! Protection in .xls files that isn't encryption: the workbook's structure
//! and each sheet, with their 16-bit password hashes and what a protected
//! sheet still allows (`FEATHDR`), and "read-only recommended"
//! (`FILESHARING`).

use apex_sheet::protect::allow;
use apex_sheet::{Password, Protection, Workbook};

use crate::xls::Rec;

fn u16_at(d: &[u8], o: usize) -> u16 {
    d.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

fn passwords(hash: u16) -> Vec<Password> {
    if hash == 0 { Vec::new() } else { vec![Password::Legacy(hash)] }
}

/// `EnhancedProtection` bits, in order, as [`allow`] flags.
const ENHANCED: [u32; 15] = [
    allow::OBJECTS,
    allow::SCENARIOS,
    allow::FORMAT_CELLS,
    allow::FORMAT_COLUMNS,
    allow::FORMAT_ROWS,
    allow::INSERT_COLUMNS,
    allow::INSERT_ROWS,
    allow::INSERT_LINKS,
    allow::DELETE_COLUMNS,
    allow::DELETE_ROWS,
    allow::SELECT_LOCKED,
    allow::SORT,
    allow::AUTOFILTER,
    allow::PIVOT_TABLES,
    allow::SELECT_UNLOCKED,
];

/// The workbook's records up to their `EOF`: structure protection and
/// read-only recommended.
pub(crate) fn globals(recs: &[Rec], wb: &mut Workbook) {
    let (mut locked, mut hash) = (false, 0);
    for r in recs.iter().skip(1) {
        match r.ty {
            0x000A => break,
            0x0012 => locked = u16_at(&r.data, 0) != 0,
            0x0013 => hash = u16_at(&r.data, 0),
            0x005B => wb.meta.read_only_recommended = u16_at(&r.data, 0) != 0,
            _ => {}
        }
    }
    if locked {
        wb.protection = Some(Protection { passwords: passwords(hash), allow: 0 });
    }
}

/// A sheet's protection, from its records (starting at its `BOF`).
pub(crate) fn sheet(recs: &[Rec]) -> Option<Protection> {
    let (mut locked, mut hash, mut flags) = (false, 0, None);
    let mut depth = 0;
    for r in recs {
        match r.ty {
            0x0809 => depth += 1,
            0x000A => {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
            }
            // Embedded charts have protection records of their own.
            _ if depth > 1 => {}
            0x0012 => locked = u16_at(&r.data, 0) != 0,
            0x0013 => hash = u16_at(&r.data, 0),
            // FEATHDR of the protection feature (isf 2) with its flags.
            0x0867 if u16_at(&r.data, 12) == 2 && r.data.len() >= 23 => {
                let bits = u32::from_le_bytes([r.data[19], r.data[20], r.data[21], r.data[22]]);
                flags = Some(ENHANCED.iter().enumerate().filter(|(i, _)| bits & (1 << i) != 0).fold(0, |a, (_, f)| a | f));
            }
            _ => {}
        }
    }
    locked.then(|| Protection { passwords: passwords(hash), allow: flags.unwrap_or(allow::DEFAULT) })
}
