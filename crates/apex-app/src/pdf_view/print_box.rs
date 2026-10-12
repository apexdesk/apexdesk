//! Printing a PDF tab: the file itself goes to the printer (no drawing of
//! our own), with the printer, copies and sides chosen here.

use std::sync::mpsc::{Receiver, channel};

use eframe::egui;

use super::{PdfView, Request};
use crate::i18n::tr;
use crate::imposition::Sides;
use crate::widgets::{self, DialogResult};

pub struct PrintBox {
    printers: Option<(Vec<String>, Option<String>)>,
    looking: Option<Receiver<(Vec<String>, Option<String>)>>,
    printer: String,
    copies: u32,
    sides: Sides,
    /// Pages to print ("1-3, 5"); all when empty.
    pages: String,
    bad_pages: bool,
}

impl PdfView {
    /// Opens the print dialog (File > Print, Ctrl+P, the toolstrip).
    pub fn open_print(&mut self) {
        let (tx, rx) = channel();
        // Looking for printers can take a moment.
        std::thread::spawn(move || {
            tx.send(crate::print::list_printers()).ok();
        });
        self.print = Some(PrintBox { printers: None, looking: Some(rx), printer: String::new(), copies: 1, sides: Sides::One, pages: String::new(), bad_pages: false });
    }

    pub(super) fn print_ui(&mut self, ctx: &egui::Context) {
        let Some(b) = &mut self.print else { return };
        if let Some(rx) = &b.looking
            && let Ok(found) = rx.try_recv()
        {
            b.printer = found.1.clone().or_else(|| found.0.first().cloned()).unwrap_or_default();
            b.printers = Some(found);
            b.looking = None;
        }
        let res = widgets::dialog(ctx, "pdf_print", tr("print.title"), 380.0, |ui| {
            widgets::form(ui, "pdf_print_form", |ui| {
                widgets::form_row(ui, tr("print.printer"), |ui| {
                    // Room for the message of two lines from the start: the
                    // buttons don't move when the printers are found.
                    ui.set_min_height(2.0 * ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y);
                    match &b.printers {
                        None => {
                            ui.spinner();
                            ui.label(tr("print.looking"));
                        }
                        Some((list, _)) if list.is_empty() => {
                            ui.set_max_width(200.0);
                            ui.add(egui::Label::new(tr("print.no_printers")).wrap());
                        }
                        Some((list, _)) => {
                            egui::ComboBox::from_id_salt("pdf_printer").width(200.0).selected_text(b.printer.as_str()).show_ui(ui, |ui| {
                                for p in list {
                                    ui.selectable_value(&mut b.printer, p.clone(), p);
                                }
                            });
                        }
                    }
                });
                ui.end_row();
                widgets::form_row(ui, tr("print.pages"), |ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut b.pages).hint_text(tr("pdf.print_all_pages")).desired_width(200.0));
                    if r.changed() {
                        b.bad_pages = false;
                    }
                });
                ui.end_row();
                widgets::form_row(ui, tr("print.copies"), |ui| ui.add(egui::DragValue::new(&mut b.copies).range(1..=999)));
                ui.end_row();
                widgets::form_row(ui, tr("print.sides"), |ui| {
                    egui::ComboBox::from_id_salt("pdf_sides").width(200.0).selected_text(tr(b.sides.key())).show_ui(ui, |ui| {
                        // Both sides by hand needs our own pages; the file
                        // goes to the printer as it is.
                        for s in [Sides::One, Sides::LongEdge, Sides::ShortEdge] {
                            ui.selectable_value(&mut b.sides, s, tr(s.key()));
                        }
                    });
                });
                ui.end_row();
            });
            if b.bad_pages {
                ui.colored_label(ui.visuals().error_fg_color, tr("pdf.split_bad"));
            }
            if cfg!(windows) && b.sides != Sides::One {
                ui.label(egui::RichText::new(tr("print.sides_driver")).weak());
            }
        });
        match res {
            DialogResult::Ok if !b.printer.is_empty() => {
                let n = self.sizes.1.len();
                let bytes = if b.pages.trim().is_empty() {
                    self.doc.to_bytes()
                } else if let Some(r) = apex_io::pdf::edit::organize::parse_ranges(&b.pages, n) {
                    self.doc.extract(&r.iter().flat_map(|&(a, z)| a..=z).collect::<Vec<_>>())
                } else {
                    b.bad_pages = true;
                    return;
                };
                // Printing hands the file to another program (on Windows,
                // the system's PDF reader), which would run what this one
                // never does: scripts, launch and open actions, attachments; and it
                // prints what is shown here, not annotations or layers hidden.
                let r = apex_io::pdf::edit::PdfDoc::open(bytes, self.password.as_deref())
                    .map(|mut d| {
                        d.for_print();
                        d.to_bytes_whole()
                    })
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| crate::print::print(bytes, &b.printer, b.copies, b.sides))
                    .map(|_| b.printer.clone());
                self.requests.push(Request::Printed(r));
                self.print = None;
            }
            DialogResult::Ok | DialogResult::Cancel => self.print = None,
            DialogResult::None => {}
        }
    }
}
