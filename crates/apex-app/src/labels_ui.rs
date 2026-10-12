//! Sensitivity labels (Microsoft Purview) of the open file: shown in the
//! status bar and in File > Properties. They can't be applied or changed
//! here: the organization manages them.

use apex_doc::{KeptParts, Meta};
use eframe::egui::{self, RichText};

use crate::i18n::tr;
use crate::icons::{self, Icon};

/// The names of the file's labels ("Sensitivity label" for one without a
/// name), or `None` without labels.
pub fn label_names(meta: &Meta, kept: &KeptParts) -> Option<String> {
    let labels = kept.sensitivity_labels(meta);
    if labels.is_empty() {
        return None;
    }
    let names: Vec<&str> = labels.iter().map(|l| if l.name.is_empty() { tr("props.label") } else { l.name.as_str() }).collect();
    Some(names.join(", "))
}

/// A padlock and the label's name, for a status bar laid out right to left.
pub fn status(ui: &mut egui::Ui, meta: &Meta, kept: &KeptParts) {
    let Some(names) = label_names(meta, kept) else { return };
    ui.separator();
    let r = ui.label(RichText::new(&names).weak());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    icons::paint(ui.painter(), rect, Icon::Protect, ui.visuals().weak_text_color());
    r.on_hover_text(format!("{}: {names}\n{}", tr("props.label"), tr("label.tip")));
}
