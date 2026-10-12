//! Table of authorities: marking citations (TA fields) and inserting the
//! tables.

use apex_doc::{Attrs, Field, FieldKind, TocOpts};

use super::refs::RefDialog;
use super::*;
use crate::widgets::DialogResult;

/// Word's categories, 1..=7.
pub(super) const CATEGORIES: [&str; 7] = ["toa.cat1", "toa.cat2", "toa.cat3", "toa.cat4", "toa.cat5", "toa.cat6", "toa.cat7"];

pub(super) struct MarkCitationDlg {
    long: String,
    short: String,
    cat: u8,
    all: bool,
    /// The selected text (what Mark All looks for).
    found: String,
}

pub(super) struct ToaDlg {
    /// 0: every category with entries.
    cat: u8,
    opts: TocOpts,
}

impl DocView {
    pub(super) fn toa_group(&mut self, b: &mut crate::toolstrip::Body) {
        use crate::toolstrip;
        let a = b.accent();
        b.group("toolstrip.g.toa", Icon::Bookmark, |ui| {
            if toolstrip::large(ui, Icon::Bookmark, tr("toa.mark"), &format!("{} (Alt+Shift+I)", tr("toa.mark")), a, false).clicked() {
                self.open_mark_citation();
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Toc), tr("toa.insert"), "", a, false).clicked() {
                    let cat = self.doc.toc_at(self.sel.cursor.para).and_then(|(s, _)| self.doc.paragraphs[s].style.toc).map_or(0, |t| t.opts.toa);
                    self.refs.dialog = Some(RefDialog::Toa(Box::new(ToaDlg { cat, opts: TocOpts { links: false, ..TocOpts::default() } })));
                }
                let has = self.doc.toc_blocks().iter().any(|&(s, _)| self.doc.paragraphs[s].style.toc.is_some_and(|t| t.opts.toa > 0));
                ui.add_enabled_ui(has, |ui| {
                    if toolstrip::small(ui, Some(Icon::Calculate), tr("toa.update"), "", a, false).on_disabled_hover_text(tr("toa.none")).clicked() {
                        let (changed, sel) = self.update_all_tocs();
                        if changed {
                            self.set_sel(sel);
                        } else {
                            self.notices.push(Notice::info(tr("toc.up_to_date")));
                        }
                    }
                });
            });
        });
    }

    pub(super) fn open_mark_citation(&mut self) {
        let t: String = self.doc.text_range(self.sel.start(), self.sel.end()).chars().filter(|&c| c != apex_doc::OBJECT).map(|c| if c.is_control() || c == apex_doc::LINE_BREAK { ' ' } else { c }).collect();
        let t = t.trim().to_string();
        self.refs.dialog = Some(RefDialog::MarkCitation(Box::new(MarkCitationDlg { long: t.clone(), short: t.clone(), cat: 1, all: false, found: t })));
    }

    /// The Mark Citation and Table of Authorities dialogs.
    pub(super) fn toa_dialog(&mut self, ctx: &egui::Context, d: RefDialog) -> Option<RefDialog> {
        match d {
            RefDialog::MarkCitation(mut m) => {
                let res = widgets::dialog(ctx, "doc_mark_citation", tr("toa.mark_title"), 440.0, |ui| mark_page(ui, &mut m));
                match res {
                    DialogResult::None => return Some(RefDialog::MarkCitation(m)),
                    DialogResult::Cancel => {}
                    DialogResult::Ok => {
                        let long = m.long.trim().to_string();
                        if !long.is_empty() {
                            let kind = FieldKind::Authority { long, short: m.short.trim().to_string(), cat: m.cat };
                            if m.all && !m.found.is_empty() {
                                let n = self.doc.mark_all(&m.found, kind, self.sel);
                                self.notices.push(Notice::info(trf("index.marked", &[("n", &n)])));
                            } else {
                                self.clear_block();
                                let s = self.doc.insert_field(Selection::caret(self.sel.end()), Field::new(kind));
                                self.sel = Selection::caret(s.end());
                            }
                        }
                    }
                }
                self.focus = true;
                None
            }
            RefDialog::Toa(mut t) => {
                let used = {
                    let r = self.doc.refs();
                    let mut v: Vec<u8> = r.authorities.iter().map(|a| a.2).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };
                let res = widgets::dialog(ctx, "doc_toa", tr("toa.title"), 360.0, |ui| toa_page(ui, &mut t));
                match res {
                    DialogResult::None => return Some(RefDialog::Toa(t)),
                    DialogResult::Cancel => {}
                    DialogResult::Ok if t.cat > 0 => {
                        let s = self.doc.insert_toc(self.sel, TocOpts { toa: t.cat, ..t.opts }, tr("toa.no_entries"));
                        self.set_sel(s);
                    }
                    DialogResult::Ok => {
                        // One table per category, each under its name.
                        let cats: Vec<u8> = if used.is_empty() { vec![1] } else { used };
                        // Starting a paragraph of its own.
                        let c = self.doc.clamp(self.sel.start());
                        if c.offset > 0 {
                            let end = self.doc.insert_text(self.sel, "\n", None);
                            self.sel = Selection::caret(end);
                        }
                        for c in cats {
                            let name = tr(CATEGORIES.get(c as usize - 1).copied().unwrap_or("toa.cat3"));
                            let bold = Attrs { bold: true, ..Default::default() };
                            let end = self.doc.insert_text(self.sel, &format!("{name}\n"), Some(bold));
                            self.sel = Selection::caret(end);
                            let s = self.doc.insert_toc(self.sel, TocOpts { toa: c, ..t.opts }, tr("toa.no_entries"));
                            self.sel = s;
                        }
                        self.set_sel(self.sel);
                    }
                }
                self.focus = true;
                None
            }
            other => Some(other),
        }
    }
}

fn category_combo(ui: &mut egui::Ui, id: &str, cat: &mut u8, all: bool) {
    let label = |c: u8| if c == 0 { tr("toa.all") } else { tr(CATEGORIES[(c as usize - 1).min(6)]) };
    egui::ComboBox::from_id_salt(id).width(220.0).selected_text(label(*cat)).show_ui(ui, |ui| {
        if all {
            ui.selectable_value(cat, 0, tr("toa.all"));
        }
        for c in 1..=7u8 {
            ui.selectable_value(cat, c, label(c));
        }
    });
}

fn mark_page(ui: &mut egui::Ui, m: &mut MarkCitationDlg) {
    widgets::section(ui, tr("refs.sec_citation"));
    widgets::form(ui, "toa_mark_grid", |ui| {
        widgets::form_row(ui, tr("toa.long").trim_end_matches(':'), |ui| ui.add(egui::TextEdit::multiline(&mut m.long).desired_rows(2).desired_width(f32::INFINITY)));
        widgets::form_row(ui, tr("toa.short").trim_end_matches(':'), |ui| ui.add(egui::TextEdit::singleline(&mut m.short).desired_width(f32::INFINITY)));
        widgets::form_row(ui, tr("toa.category").trim_end_matches(':'), |ui| category_combo(ui, "toa_mark_cat", &mut m.cat, false));
    });
    widgets::section(ui, tr("refs.sec_options"));
    ui.add_enabled(!m.found.is_empty(), egui::Checkbox::new(&mut m.all, trf("index.mark_all", &[("text", &m.found.as_str())])));
}

fn toa_page(ui: &mut egui::Ui, t: &mut ToaDlg) {
    widgets::section(ui, tr("refs.sec_general"));
    widgets::form(ui, "toa_grid", |ui| {
        widgets::form_row(ui, tr("toa.category").trim_end_matches(':'), |ui| category_combo(ui, "toa_cat", &mut t.cat, true));
    });
    let o = &mut t.opts;
    widgets::section(ui, tr("refs.sec_page_numbers"));
    ui.checkbox(&mut o.page_numbers, tr("toc.page_numbers"));
    ui.add_enabled(o.page_numbers, egui::Checkbox::new(&mut o.right_align, tr("toc.right_align")));
    ui.add_enabled(o.page_numbers && o.right_align, egui::Checkbox::new(&mut o.leader, tr("toc.leader")));
}
