//! The comments pane: every annotation of the file by page, with its
//! author, date and text, its replies, and a box to answer the chosen one.

use std::sync::Arc;

use apex_io::pdf::edit::annots::Annot;
use eframe::egui::{self, RichText, Sense};

use super::PdfView;
use super::annotate::{color32, kind_name};
use crate::i18n::{tr, trf};

#[derive(Default)]
pub struct CommentsUi {
    /// The file's annotations for the revision they are of.
    list: (u64, Arc<Vec<Annot>>),
    pub reply: String,
}

/// "D:20260927101500+02'00'" as "2026-09-27 10:15".
fn short_date(d: &str) -> String {
    let d = d.strip_prefix("D:").unwrap_or(d);
    if d.get(..12).is_some_and(|p| p.bytes().all(|b| b.is_ascii_digit())) {
        format!("{}-{}-{} {}:{}", &d[0..4], &d[4..6], &d[6..8], &d[8..10], &d[10..12])
    } else {
        d.chars().take(16).collect()
    }
}

impl PdfView {
    fn all_comments(&mut self) -> Arc<Vec<Annot>> {
        let rev = self.doc.revision;
        if self.comments.list.1.is_empty() || self.comments.list.0 != rev {
            let list: Vec<Annot> = self.doc.all_annots().into_iter().filter(|a| a.kind.is_comment()).collect();
            self.comments.list = (rev, Arc::new(list));
        }
        self.comments.list.1.clone()
    }

    pub(super) fn comments_ui(&mut self, ui: &mut egui::Ui) {
        let all = self.all_comments();
        let tops: Vec<&Annot> = all.iter().filter(|a| a.reply_to.is_none()).collect();
        if tops.is_empty() {
            ui.label(RichText::new(tr("pdf.no_comments")).weak());
            return;
        }
        ui.label(RichText::new(trf("pdf.comment_count", &[("n", &tops.len())])).weak());
        let mut go = None;
        egui::ScrollArea::vertical().id_salt(("pdf_comments", self.id)).auto_shrink([false, false]).show(ui, |ui| {
            // Nothing in it may be wider than the pane: the pane takes the
            // width of what it holds, and would grow every frame.
            let width = ui.available_width();
            ui.set_max_width(width);
            let mut page = usize::MAX;
            for a in &tops {
                if a.page != page {
                    page = a.page;
                    ui.add_space(4.0);
                    ui.label(RichText::new(trf("pdf.page_n", &[("n", &(page + 1))])).strong());
                }
                let on = self.picked == Some(a.id);
                let frame = egui::Frame::group(ui.style()).fill(if on { ui.visuals().selection.bg_fill.gamma_multiply(0.35) } else { ui.visuals().faint_bg_color });
                let mut head = egui::Rect::NOTHING;
                let inner = (width - frame.total_margin().sum().x).max(40.0);
                let resp = frame.show(ui, |ui| {
                    ui.set_width(inner);
                    head = ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
                        ui.painter().rect_filled(r, 2.0, color32(a.color));
                        ui.add(egui::Label::new(RichText::new(kind_name(a.kind)).strong()).truncate());
                        if !a.author.is_empty() {
                            ui.add(egui::Label::new(RichText::new(&a.author).weak()).truncate());
                        }
                    }).response.rect;
                    if !a.date.is_empty() {
                        ui.label(RichText::new(short_date(&a.date)).weak().small());
                    }
                    if !a.contents.is_empty() {
                        ui.label(&a.contents);
                    }
                    for r in all.iter().filter(|r| r.reply_to == Some(a.id)) {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(10.0);
                            ui.label(RichText::new(format!("↳ {}", if r.author.is_empty() { tr("pdf.reply") } else { r.author.as_str() })).weak());
                            ui.label(&r.contents);
                        });
                    }
                    if on {
                        // The button first (at the right), the box in what is left.
                        ui.horizontal(|ui| ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let send = ui.button(tr("pdf.reply")).clicked();
                            let edit = ui.add(egui::TextEdit::singleline(&mut self.comments.reply).hint_text(tr("pdf.reply_hint")).desired_width(ui.available_width()));
                            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if (send || enter) && !self.comments.reply.trim().is_empty() {
                                let text = std::mem::take(&mut self.comments.reply);
                                self.doc.reply(a.id, text.trim(), &self.author);
                            }
                        }));
                    }
                });
                // The chosen one keeps its reply box free.
                let area = if on { head } else { resp.response.rect };
                let click = ui.interact(area, ui.id().with(("pdf_comment", a.id)), Sense::click());
                if click.clicked() {
                    go = Some((a.id, a.page, a.rect[1]));
                }
                if click.double_clicked() {
                    self.edit_comment(a.id);
                }
                click.context_menu(|ui| {
                    if ui.button(tr("pdf.edit_comment")).clicked() {
                        self.edit_comment(a.id);
                        ui.close();
                    }
                    if ui.button(tr("pdf.delete_annot")).clicked() {
                        self.picked = Some(a.id);
                        self.delete_picked();
                        ui.close();
                    }
                });
            }
        });
        if let Some((id, page, top)) = go {
            self.picked = Some(id);
            self.go_to(page, (top - 40.0).max(0.0));
        }
        if self.comments.list.0 != self.doc.revision {
            self.after_change();
        }
    }
}
