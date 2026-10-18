//! Legacy form fields of .doc files: a `FORMTEXT`, `FORMCHECKBOX` or
//! `FORMDROPDOWN` field whose start character points (`sprmCPicLocation`)
//! to its settings (`FFData`) in the Data stream.

use apex_doc::{FormField, FormTextType};

use super::u16_at;
use crate::docx_forms::FfData;

/// An `Xstz` (a length, UTF-16 characters and a terminating zero) at `p`;
/// the text and the position after it.
fn xstz(d: &[u8], p: usize, term: bool) -> Option<(String, usize)> {
    let n = u16_at(d, p)? as usize;
    let units: Vec<u16> = (0..n).map(|i| u16_at(d, p + 2 + 2 * i)).collect::<Option<_>>()?;
    Some((String::from_utf16_lossy(&units), p + 2 + 2 * n + if term { 2 } else { 0 }))
}

/// The `FFData` of the field whose start has its picture location at `at`
/// (after the 68-byte `NilPICFAndBinData` header).
pub(super) fn read(data: &[u8], at: u32) -> Option<FfData> {
    let base = at as usize + 0x44;
    let d = data.get(base..)?;
    if u16_at(d, 0)? != 0xFFFF || u16_at(d, 2)? != 0xFFFF {
        return None;
    }
    let bits = u16_at(d, 4)?;
    let ty = bits & 3;
    let res = (bits >> 2) & 0x1F;
    let mut f = FormField {
        help_auto: bits & 0x80 == 0,
        status_auto: bits & 0x100 == 0,
        enabled: bits & 0x200 == 0,
        text_type: FormTextType::from_doc((bits >> 11) & 7),
        calc_on_exit: bits & 0x4000 != 0,
        max_len: u32::from(u16_at(d, 6)?),
        ..Default::default()
    };
    if bits & 0x400 != 0 {
        f.check_size = Some(u32::from(u16_at(d, 8)?));
    }
    let (name, mut p) = xstz(d, 10, true)?;
    f.name = name;
    if ty == 0 {
        (f.default_text, p) = xstz(d, p, true)?;
    }
    let mut def = 0u16;
    if ty == 1 || ty == 2 {
        def = u16_at(d, p)?;
        p += 2;
    }
    (f.format, p) = xstz(d, p, true)?;
    (f.help, p) = xstz(d, p, true)?;
    (f.status, p) = xstz(d, p, true)?;
    (f.entry_macro, p) = xstz(d, p, true)?;
    (f.exit_macro, p) = xstz(d, p, true)?;
    if f.help.is_empty() {
        f.help_auto = false;
    }
    if f.status.is_empty() {
        f.status_auto = false;
    }
    let mut ff = FfData { field: f, checkbox: None, dropdown: None };
    match ty {
        1 => {
            ff.field.default_checked = def != 0;
            ff.checkbox = Some((res != 25).then_some(res != 0));
        }
        2 => {
            ff.field.default_item = u32::from(def);
            // The list: an extended string table.
            let mut entries = Vec::new();
            if u16_at(d, p) == Some(0xFFFF) {
                let n = u16_at(d, p + 2).unwrap_or(0);
                let extra = u16_at(d, p + 4).unwrap_or(0) as usize;
                let mut q = p + 6;
                // Word lists at most 25; a field reads at most 256.
                for _ in 0..n.min(256) {
                    let Some((s, next)) = xstz(d, q, false) else { break };
                    entries.push(s);
                    q = next + extra;
                }
            }
            ff.dropdown = Some((entries, (res != 25).then_some(res as usize)));
        }
        _ => {}
    }
    Some(ff)
}
