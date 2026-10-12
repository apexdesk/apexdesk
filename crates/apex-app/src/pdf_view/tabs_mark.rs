//! The PDF tab's Comment and Fill & Sign toolstrip tabs.

use apex_io::pdf::edit::annots::Kind;
use eframe::egui::{self, Sense, Stroke, vec2};

use super::annotate::{COLORS, STAMPS, WIDTHS, color32, kind_name};
use super::{Pane, PdfView, Request, Tool};
use crate::i18n::tr;
use crate::icons::Icon;
use crate::toolstrip::{self, Body};

impl PdfView {
    /// The letter size of text boxes and filled-in text: the picked
    /// box's, or the next one's.
    fn font_size_menu(&mut self, ui: &mut egui::Ui, acc: egui::Color32) {
        let now = self.shown_font_size();
        let label = format!("{} {now} pt", tr("pdf.font_size"));
        toolstrip::small_menu(ui, None, &label, tr("pdf.font_size"), acc, |ui| {
            for s in [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 36.0] {
                if ui.selectable_label(now == s, format!("{s} pt")).clicked() {
                    self.set_font_size(s);
                    ui.close();
                }
            }
        });
    }

    pub(super) fn comment_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("pdf.g.markup", Icon::Highlight, |ui| {
            let t = Tool::Markup(Kind::Highlight);
            if toolstrip::large(ui, Icon::Highlight, kind_name(Kind::Highlight), tr("pdf.tool.markup_tip"), acc, self.tool == t).clicked() {
                self.set_tool(t);
            }
            toolstrip::column(ui, |ui| {
                for k in [Kind::Underline, Kind::StrikeOut] {
                    let t = Tool::Markup(k);
                    if toolstrip::small(ui, None, kind_name(k), tr("pdf.tool.markup_tip"), acc, self.tool == t).clicked() {
                        self.set_tool(t);
                    }
                }
            });
        });
        b.group("pdf.g.comment", Icon::Note, |ui| {
            for (k, icon, tip) in [(Kind::Note, Icon::Note, "pdf.tool.note_tip"), (Kind::FreeText, Icon::TextBox, "pdf.tool.text_box_tip")] {
                let t = Tool::Draw(k);
                if toolstrip::large(ui, icon, kind_name(k), tr(tip), acc, self.tool == t).clicked() {
                    self.set_tool(t);
                }
            }
        });
        b.group("pdf.g.draw", Icon::Pencil, |ui| {
            let t = Tool::Draw(Kind::Ink);
            if toolstrip::large(ui, Icon::Pencil, kind_name(Kind::Ink), tr("pdf.tool.ink_tip"), acc, self.tool == t).clicked() {
                self.set_tool(t);
            }
            for pair in [[Kind::Line, Kind::Arrow], [Kind::Square, Kind::Circle]] {
                toolstrip::column(ui, |ui| {
                    for k in pair {
                        let t = Tool::Draw(k);
                        if toolstrip::small(ui, None, kind_name(k), tr("pdf.tool.shape_tip"), acc, self.tool == t).clicked() {
                            self.set_tool(t);
                        }
                    }
                });
            }
            toolstrip::large_menu(ui, Icon::Watermark, kind_name(Kind::Stamp), tr("pdf.tool.stamp_tip"), acc, |ui| {
                for key in STAMPS {
                    let s = tr(key);
                    if ui.selectable_label(self.tool == Tool::Draw(Kind::Stamp) && self.pen.stamp == s, s).clicked() {
                        self.pen.stamp = s.to_string();
                        self.tool = Tool::Draw(Kind::Stamp);
                        ui.close();
                    }
                }
            });
        });
        b.group("pdf.g.style", Icon::Fill, |ui| {
            toolstrip::column(ui, |ui| {
                ui.label(tr("pdf.color"));
                let current = if self.tool == Tool::Markup(Kind::Highlight) { self.pen.marker } else { self.pen.color };
                egui::Grid::new("pdf_colors").spacing(vec2(3.0, 3.0)).show(ui, |ui| {
                    for (i, c) in COLORS.into_iter().enumerate() {
                        let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                        ui.painter().rect_filled(r, 3.0, color32(c));
                        if c == current {
                            ui.painter().rect_stroke(r.expand(1.0), 3.0, Stroke::new(2.0, acc), egui::StrokeKind::Outside);
                        }
                        if resp.on_hover_text(tr("pdf.color")).clicked() {
                            self.recolor(c);
                        }
                        if i % 4 == 3 {
                            ui.end_row();
                        }
                    }
                });
            });
            toolstrip::column(ui, |ui| {
                toolstrip::small_menu(ui, None, tr("pdf.line_width"), tr("pdf.line_width"), acc, |ui| {
                    for w in WIDTHS {
                        if ui.selectable_label(self.pen.width == w, format!("{w} pt")).clicked() {
                            self.pen.width = w;
                            ui.close();
                        }
                    }
                });
                self.font_size_menu(ui, acc);
            });
        });
        b.group("pdf.g.comments", Icon::Comment, |ui| {
            if toolstrip::large(ui, Icon::Comment, tr("pdf.comments"), tr("pdf.comments_tip"), acc, self.pane == Pane::Comments).clicked() {
                self.pane = if self.pane == Pane::Comments { Pane::None } else { Pane::Comments };
            }
            toolstrip::column(ui, |ui| {
                let picked = self.picked;
                ui.add_enabled_ui(picked.is_some(), |ui| {
                    if toolstrip::small(ui, Some(Icon::Note), tr("pdf.edit_comment"), tr("pdf.edit_comment"), acc, false).clicked()
                        && let Some(id) = picked
                    {
                        self.edit_comment(id);
                    }
                    if toolstrip::small(ui, Some(Icon::Reject), tr("pdf.delete_annot"), &format!("{} (Del)", tr("pdf.delete_annot")), acc, false).clicked() {
                        self.delete_picked();
                    }
                });
            });
        });
    }

    pub(super) fn fill_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("pdf.g.fill", Icon::TextBox, |ui| {
            for (t, icon, key, tip) in [
                (Tool::FillText, Icon::TextBox, "pdf.fill_text", "pdf.fill_text_tip"),
                (Tool::FillCheck, Icon::Checkbox, "pdf.fill_check", "pdf.fill_check_tip"),
                (Tool::FillDate, Icon::Calendar, "pdf.fill_date", "pdf.fill_date_tip"),
            ] {
                if toolstrip::large(ui, icon, tr(key), tr(tip), acc, self.tool == t).clicked() {
                    self.set_tool(t);
                }
            }
            toolstrip::column(ui, |ui| self.font_size_menu(ui, acc));
        });
        let fields = self.form_fields();
        if fields.iter().any(|f| matches!(f.kind, apex_io::pdf::edit::forms::FieldKind::Text { .. }) && !f.read_only) {
            b.group("pdf.g.form", Icon::Field, |ui| {
                if toolstrip::large(ui, Icon::Field, tr("pdf.form_first"), tr("pdf.form_first_tip"), acc, false).clicked()
                    && let Some(f) = fields.iter().find(|f| matches!(f.kind, apex_io::pdf::edit::forms::FieldKind::Text { .. }) && !f.read_only)
                {
                    self.set_tool(Tool::Select);
                    self.edit_field(&f.name.clone());
                }
            });
        }
        b.group("pdf.g.sign", Icon::Pen, |ui| {
            if toolstrip::large(ui, Icon::Pen, tr("pdf.sign_visible"), tr("pdf.sign_visible_tip"), acc, self.tool == Tool::PlaceSign).clicked() {
                self.open_sign_box();
            }
            if toolstrip::large(ui, Icon::Protect, tr("pdf.sign_cert"), tr("pdf.sign_cert_tip"), acc, false).clicked() {
                self.requests.push(Request::Sign(None));
            }
            if toolstrip::large(ui, Icon::Accept, tr("pdf.signatures"), tr("pdf.signatures_tip"), acc, false).clicked() {
                self.requests.push(Request::Signatures);
            }
            if crate::license::has("management") && toolstrip::large(ui, Icon::Numbering, tr("pdf.circuit"), tr("pdf.circuit_tip"), acc, false).clicked() {
                self.open_circuit();
            }
        });
    }
}
