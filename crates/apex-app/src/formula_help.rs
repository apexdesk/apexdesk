//! Formula aids: the function catalogue with localized help and
//! autocomplete (apex-i18n), the reading of a formula being typed
//! (apex-sheet), and the colors of its references.

use eframe::egui::Color32;

pub use apex_i18n::functions::*;
pub use apex_sheet::formula_text::*;

/// Colors for the references of a formula, in order of appearance.
pub const REF_COLORS: [Color32; 8] = [
    Color32::from_rgb(0x1F, 0x6F, 0xD0),
    Color32::from_rgb(0xD0, 0x3A, 0x2F),
    Color32::from_rgb(0x2E, 0x9A, 0x4B),
    Color32::from_rgb(0x8E, 0x44, 0xAD),
    Color32::from_rgb(0xD6, 0x7D, 0x00),
    Color32::from_rgb(0x00, 0x8C, 0x9E),
    Color32::from_rgb(0xC2, 0x18, 0x7A),
    Color32::from_rgb(0x6D, 0x5B, 0x2E),
];
