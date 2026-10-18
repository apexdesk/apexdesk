//! File > Inspect File: everything the file in the active tab carries that
//! doesn't show on screen (macros, links to other files, hidden sheets,
//! rows and slides, comments, tracked changes, properties, signatures,
//! connections, custom XML, embedded objects...), for workbooks, documents
//! and presentations, each kind with a button that removes it for good as
//! one undo step (the finding is in `apex_io::inspect`). PDF files have
//! their own Remove Hidden Information on the PDF tab.

use apex_io::inspect::{self, Found, Hidden};
use eframe::egui::{self, Color32, RichText};

use super::{App, View};
use crate::i18n::{tr, trf};

pub(super) struct InspectorDlg {
    tab: u64,
    found: Vec<Found>,
    /// Kinds removed (or shown, `true`) while the dialog is open.
    done: Vec<(Hidden, bool)>,
}

/// Name and explanation of a kind (translation keys).
fn names(h: Hidden) -> (&'static str, &'static str) {
    match h {
        Hidden::Macros => ("fi.macros", "fi.macros_hint"),
        Hidden::ExternalLinks => ("fi.links", "fi.links_hint"),
        Hidden::HiddenSheets => ("fi.sheets", "fi.sheets_hint"),
        Hidden::HiddenRows => ("fi.rows", "fi.rows_hint"),
        Hidden::HiddenCols => ("fi.cols", "fi.cols_hint"),
        Hidden::HiddenNames => ("fi.names", "fi.names_hint"),
        Hidden::Comments => ("fi.comments", "fi.comments_hint"),
        Hidden::Revisions => ("fi.revisions", "fi.revisions_hint"),
        Hidden::Properties => ("fi.props", "fi.props_hint"),
        Hidden::Signatures => ("fi.sigs", "fi.sigs_hint"),
        Hidden::Connections => ("fi.conns", "fi.conns_hint"),
        Hidden::PivotCaches => ("fi.pivots", "fi.pivots_hint"),
        Hidden::CustomXml => ("fi.xml", "fi.xml_hint"),
        Hidden::Embedded => ("fi.embedded", "fi.embedded_hint"),
        Hidden::HiddenText => ("fi.text", "fi.text_hint"),
        Hidden::Headers => ("fi.headers", "fi.headers_hint"),
        Hidden::HiddenSlides => ("fi.slides", "fi.slides_hint"),
        Hidden::HiddenObjects => ("fi.objects", "fi.objects_hint"),
        Hidden::SpeakerNotes => ("fi.notes", "fi.notes_hint"),
        Hidden::BookNotes => ("fi.book", "fi.book_hint"),
    }
}

impl App {
    /// What the file of tab `idx` carries, signatures included.
    fn inspect_tab(&self, idx: usize) -> Option<Vec<Found>> {
        let t = self.tabs.get(idx)?;
        let mut out = match &t.view {
            View::Sheet(v) => inspect::workbook(&v.wb),
            View::Doc(v) => inspect::document(&v.doc),
            View::Slides(v) => inspect::deck(&v.pres.deck),
            View::Pdf(_) | View::Db(_) | View::Note(_) => return None,
        };
        if let Some(who) = self.signers(t.id) {
            out.push(Found { what: Hidden::Signatures, count: who.len(), items: who.into_iter().filter(|w| !w.is_empty()).collect() });
        }
        Some(out)
    }

    pub(super) fn open_inspector(&mut self) {
        if let Some(View::Pdf(_)) = self.tabs.get(self.active).map(|t| &t.view) {
            return;
        }
        let Some(found) = self.inspect_tab(self.active) else { return };
        self.inspector = Some(InspectorDlg { tab: self.tabs[self.active].id, found, done: Vec::new() });
    }

    /// Removes (or shows) `what` in tab `id`.
    fn inspector_apply(&mut self, id: u64, what: Hidden, show: bool) {
        if what == Hidden::Signatures {
            // The file is written again without them on the next save.
            self.signature_dropped(id);
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                super::macros_ui::mark_modified(&mut t.view, false);
            }
            return;
        }
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) else { return };
        match &mut t.view {
            View::Sheet(v) => {
                v.inspect_clean(what, show);
            }
            View::Doc(v) => {
                v.inspect_clean(what);
            }
            View::Slides(v) => {
                v.inspect_clean(what, show);
            }
            View::Pdf(_) | View::Db(_) | View::Note(_) => {}
        }
    }

    pub(super) fn inspector_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.inspector.take() else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == d.tab) else { return };
        let mut act = None;
        let closed = crate::widgets::info_dialog(ctx, "file_inspector", tr("fi.title"), 560.0, |ui| {
            ui.label(tr("fi.intro"));
            ui.add_space(6.0);
            if d.found.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("✔").strong().color(Color32::from_rgb(0x10, 0x7C, 0x10)));
                    ui.label(tr("fi.nothing"));
                });
            }
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.6).show(ui, |ui| {
                for (i, f) in d.found.iter().enumerate() {
                    let (name, hint) = names(f.what);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("!").strong().color(Color32::from_rgb(0xD8, 0x3B, 0x01)));
                        ui.vertical(|ui| {
                            ui.set_width(380.0);
                            ui.label(RichText::new(format!("{} ({})", tr(name), f.count)).strong());
                            ui.label(RichText::new(tr(hint)).weak());
                            if !f.items.is_empty() {
                                egui::CollapsingHeader::new(tr("fi.details")).id_salt(("fi_items", i)).show(ui, |ui| {
                                    for it in &f.items {
                                        ui.label(it);
                                    }
                                    if f.count > f.items.len() {
                                        ui.label(RichText::new(trf("fi.more", &[("n", &(f.count - f.items.len()))])).weak());
                                    }
                                });
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            if ui.button(tr("fi.remove")).clicked() {
                                act = Some((f.what, false));
                            }
                            if f.what.can_show() && ui.button(tr("fi.show")).clicked() {
                                act = Some((f.what, true));
                            }
                        });
                    });
                    ui.separator();
                }
                for (h, show) in &d.done {
                    if d.found.iter().any(|f| f.what == *h) {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("✔").strong().color(Color32::from_rgb(0x10, 0x7C, 0x10)));
                        ui.label(format!("{}: {}", tr(names(*h).0), tr(if *show { "fi.shown" } else { "fi.removed" })));
                    });
                }
            });
            ui.add_space(4.0);
            ui.label(RichText::new(tr("fi.undo_note")).weak().small());
        });
        if let Some((what, show)) = act {
            self.inspector_apply(d.tab, what, show);
            d.done.retain(|x| x.0 != what);
            d.done.push((what, show));
            d.found = self.inspect_tab(idx).unwrap_or_default();
        }
        if !closed {
            self.inspector = Some(d);
        }
    }
}
