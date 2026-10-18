//! East Asian layout dialogs: the Phonetic Guide (Home > Font) that puts
//! ruby over the selection, and Asian Layout (Home > Paragraph): combine
//! characters, two lines in one, horizontal in vertical and fit text.

use std::sync::Arc;

use apex_doc::{Combine, Ruby};

use super::*;
use crate::widgets::DialogResult;

/// Word's guide alignments, with their labels.
const ALIGNS: [(&str, &str); 5] = [
    ("center", "ruby.align_center"),
    ("distributeLetter", "ruby.align_distribute"),
    ("distributeSpace", "ruby.align_distribute_121"),
    ("left", "ruby.align_left"),
    ("right", "ruby.align_right"),
];

pub(super) struct RubyDlg {
    sel: Selection,
    /// The base text as it is in the document.
    orig_base: String,
    base: String,
    text: String,
    align: &'static str,
    /// Points.
    size: f32,
    /// Distance of the guide over the text (points).
    offset: f32,
    /// Size of the base text (half points).
    base_hp: u16,
    /// The guide it had.
    old: Option<Arc<Ruby>>,
    remove: bool,
}

/// What the Asian Layout dialog does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LayoutKind {
    Combine,
    TwoLines,
    Fit,
    /// Horizontal in vertical (tate-chu-yoko).
    Upright,
}

pub(super) struct LayoutDlg {
    kind: LayoutKind,
    sel: Selection,
    text: String,
    /// Two lines in one: "round", "square", "angle", "curly" or none.
    brackets: Option<&'static str>,
    /// Fit text: the width (points).
    width: f32,
    /// Size of one character (points), for the width in characters.
    char_pt: f32,
    /// Horizontal in vertical: fit in one line (Word's vertCompress).
    compress: bool,
    remove: bool,
}

const BRACKETS: [(&str, &str); 4] = [("round", "()"), ("square", "[]"), ("angle", "<>"), ("curly", "{}")];

/// `w:hpsRaise` for a guide `offset` points over text of `base_hp` half
/// points, and back (Word raises the guide's baseline to about the top of
/// the base text).
fn raise_of(offset: f32, base_hp: u16) -> i32 {
    base_hp as i32 - 2 + (offset * 2.0).round() as i32
}

fn offset_of(raise: i32, base_hp: u16) -> f32 {
    ((raise - base_hp as i32 + 2) as f32 / 2.0).max(0.0)
}

impl DocView {
    /// The text a phonetic guide goes over: the guide at the caret, else
    /// the selection or the word there, within one paragraph.
    pub(super) fn ruby_target(&self) -> Option<Selection> {
        let found = self.doc.ruby_at(self.sel.start()).filter(|(s, _)| self.sel.is_empty() || (s.start() <= self.sel.start() && self.sel.end() <= s.end()));
        let sel = match &found {
            Some((s, _)) => *s,
            None => self.sel_or_word(),
        };
        (sel.start().para == sel.end().para && !sel.is_empty()).then_some(sel)
    }

    pub(super) fn open_ruby_dialog(&mut self) {
        let Some(sel) = self.ruby_target() else { return };
        let found = self.doc.ruby_at(sel.start()).filter(|(s, _)| *s == sel);
        self.sel = sel;
        let p = &self.doc.paragraphs[sel.start().para];
        let base = p.text[sel.start().offset..sel.end().offset].to_string();
        let a = p.attrs_at(sel.start().offset + base.chars().next().map_or(0, char::len_utf8));
        let cf = self.doc.char_format(&p.style, &a);
        let base_hp = cf.asian.size.unwrap_or(cf.size).max(2);
        let old = found.map(|f| f.1);
        let r = old.as_deref().cloned().unwrap_or_default();
        let align = ALIGNS.iter().map(|a| a.0).find(|a| r.align.as_deref() == Some(*a)).unwrap_or("center");
        self.ruby_dlg = Some(RubyDlg {
            sel,
            orig_base: base.clone(),
            base,
            text: r.text.to_string(),
            align,
            size: r.size.unwrap_or(base_hp / 2) as f32 / 2.0,
            offset: r.raise.map_or(0.0, |x| offset_of(x, base_hp)),
            base_hp,
            old,
            remove: false,
        });
    }

    pub(super) fn ruby_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.ruby_dlg.take() else { return };
        let res = widgets::dialog(ctx, "doc_phonetic_guide", tr("ruby.title"), 420.0, |ui| {
            egui::Grid::new("ruby_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(tr("ruby.base"));
                ui.add(egui::TextEdit::singleline(&mut d.base).desired_width(260.0));
                ui.end_row();
                ui.label(tr("ruby.text"));
                ui.add(egui::TextEdit::singleline(&mut d.text).desired_width(260.0));
                ui.end_row();
                ui.label(tr("ruby.align"));
                let cur = ALIGNS.iter().find(|a| a.0 == d.align).map_or("", |a| tr(a.1));
                egui::ComboBox::from_id_salt("ruby_align").width(200.0).selected_text(cur).show_ui(ui, |ui| {
                    for (v, k) in ALIGNS {
                        ui.selectable_value(&mut d.align, v, tr(k));
                    }
                });
                ui.end_row();
                ui.label(tr("ruby.offset"));
                ui.add(egui::DragValue::new(&mut d.offset).range(0.0..=100.0).speed(0.5).suffix(" pt").max_decimals(1));
                ui.end_row();
                ui.label(tr("font.size"));
                ui.add(egui::DragValue::new(&mut d.size).range(1.0..=100.0).speed(0.5).suffix(" pt").max_decimals(1));
                ui.end_row();
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(d.old.is_some(), egui::Button::new(tr("ruby.clear"))).clicked() {
                    d.remove = true;
                }
            });
            ui.add_space(6.0);
            ruby_preview(ui, &d);
        });
        match res {
            DialogResult::None if !d.remove => self.ruby_dlg = Some(d),
            DialogResult::Cancel => self.focus = true,
            _ => {
                self.apply_ruby(d);
                self.focus = true;
            }
        }
    }

    fn apply_ruby(&mut self, d: RubyDlg) {
        let mut sel = d.sel;
        if d.remove || d.text.trim().is_empty() {
            if d.old.is_some() {
                self.doc.set_ruby(sel, None);
            }
            return;
        }
        let base = d.base.replace(['\n', '\r'], "");
        if base.is_empty() {
            return;
        }
        if base != d.orig_base {
            let attrs = self.doc.paragraphs[sel.start().para].attrs_at(sel.start().offset + d.orig_base.chars().next().map_or(0, char::len_utf8));
            let end = self.doc.insert_text(sel, &base, Some(attrs));
            sel = Selection { anchor: Pos::new(end.para, end.offset - base.len()), cursor: end };
        }
        let old = d.old.as_deref().cloned().unwrap_or_default();
        let size = (d.size * 2.0).round().clamp(2.0, 400.0) as u16;
        let offset_changed = d.old.as_ref().is_none_or(|o| o.raise.map_or(0.0, |x| offset_of(x, d.base_hp)) != d.offset);
        let ruby = Ruby {
            text: Arc::from(d.text.as_str()),
            align: Some(Arc::from(d.align)),
            size: Some(size),
            base_size: Some(d.base_hp),
            raise: if offset_changed { Some(raise_of(d.offset, d.base_hp)) } else { old.raise },
            lang: old.lang.clone().or_else(|| Some(Arc::from("ja-JP"))),
            // The run properties written with the guide keep its size.
            docx_rpr: if old.size == Some(size) { old.docx_rpr.clone() } else { None },
            ..old
        };
        self.doc.set_ruby(sel, Some(ruby));
        self.sel = sel;
    }

    pub(super) fn open_asian_layout(&mut self, kind: LayoutKind) {
        let sel = self.sel_or_word();
        if sel.is_empty() || sel.start().para != sel.end().para {
            return;
        }
        self.sel = sel;
        let p = &self.doc.paragraphs[sel.start().para];
        let text = p.text[sel.start().offset..sel.end().offset].to_string();
        let a = p.attrs_at(sel.start().offset + text.chars().next().map_or(0, char::len_utf8));
        let cf = self.doc.char_format(&p.style, &a);
        let x = a.asian();
        let char_pt = cf.asian.size.unwrap_or(cf.size) as f32 / 2.0;
        let brackets = x.combine.as_ref().and_then(|c| c.brackets.as_deref()).and_then(|b| BRACKETS.iter().map(|x| x.0).find(|x| *x == b));
        let width = x.fit_text.map_or(char_pt * text.chars().count() as f32, |f| f.0 as f32 / 20.0);
        let compress = x.combine.as_ref().is_none_or(|c| !c.vert || c.vert_compress);
        self.asian_dlg = Some(LayoutDlg { kind, sel, text, brackets, width, char_pt, compress, remove: false });
    }

    pub(super) fn asian_layout_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.asian_dlg.take() else { return };
        let title = match d.kind {
            LayoutKind::Combine => tr("asian.combine"),
            LayoutKind::TwoLines => tr("asian.two_lines"),
            LayoutKind::Fit => tr("asian.fit_text"),
            LayoutKind::Upright => tr("asian.upright"),
        };
        let has = {
            let a = self.doc.paragraphs[d.sel.start().para].attrs_at(d.sel.start().offset + d.text.chars().next().map_or(0, char::len_utf8)).asian();
            match d.kind {
                LayoutKind::Fit => a.fit_text.is_some(),
                LayoutKind::Upright => a.combine.as_ref().is_some_and(|c| c.vert),
                _ => a.combine.as_ref().is_some_and(|c| c.lines || c.letters),
            }
        };
        let res = widgets::dialog(ctx, "doc_asian_layout", title, 380.0, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr("asian.text"));
                ui.strong(&d.text);
            });
            ui.add_space(6.0);
            match d.kind {
                LayoutKind::Combine => {
                    if d.text.chars().count() > 6 {
                        ui.label(egui::RichText::new(tr("asian.combine_hint")).weak());
                    }
                }
                LayoutKind::TwoLines => {
                    ui.horizontal(|ui| {
                        ui.label(tr("asian.brackets"));
                        let cur = d.brackets.and_then(|b| BRACKETS.iter().find(|x| x.0 == b)).map_or(tr("asian.no_brackets"), |x| x.1);
                        egui::ComboBox::from_id_salt("asian_brackets").width(120.0).selected_text(cur).show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.brackets, None, tr("asian.no_brackets"));
                            for (v, l) in BRACKETS {
                                ui.selectable_value(&mut d.brackets, Some(v), l);
                            }
                        });
                    });
                }
                LayoutKind::Upright => {
                    ui.checkbox(&mut d.compress, tr("asian.upright_fit"));
                }
                LayoutKind::Fit => {
                    ui.horizontal(|ui| {
                        ui.label(tr("asian.fit_width"));
                        let mut chars = d.width / d.char_pt.max(0.5);
                        if ui.add(egui::DragValue::new(&mut chars).range(0.5..=200.0).speed(0.1).max_decimals(1).suffix(format!(" {}", tr("asian.chars")))).changed() {
                            d.width = chars * d.char_pt;
                        }
                        ui.add(egui::DragValue::new(&mut d.width).range(1.0..=1500.0).speed(0.5).max_decimals(1).suffix(" pt"));
                    });
                }
            }
            ui.add_space(6.0);
            if ui.add_enabled(has, egui::Button::new(tr("asian.remove"))).clicked() {
                d.remove = true;
            }
        });
        match res {
            DialogResult::None if !d.remove => self.asian_dlg = Some(d),
            DialogResult::Cancel => self.focus = true,
            _ => {
                self.apply_asian_layout(d);
                self.focus = true;
            }
        }
    }

    fn apply_asian_layout(&mut self, d: LayoutDlg) {
        let id = self.doc.next_layout_id();
        match d.kind {
            LayoutKind::Fit => {
                let fit = (!d.remove).then(|| ((d.width * 20.0).round().max(20.0) as u32, id));
                self.doc.format_range(d.sel, &move |a| a.set_asian(|x| x.fit_text = fit));
            }
            _ => {
                let c = (!d.remove).then(|| Combine {
                    id: Some(id),
                    lines: d.kind == LayoutKind::TwoLines,
                    letters: d.kind == LayoutKind::Combine,
                    brackets: d.brackets.filter(|_| d.kind == LayoutKind::TwoLines).map(Arc::from),
                    vert: d.kind == LayoutKind::Upright,
                    vert_compress: d.kind == LayoutKind::Upright && d.compress,
                    ..Default::default()
                });
                self.doc.format_range(d.sel, &move |a| a.set_asian(|x| x.combine = c.clone()));
            }
        }
    }
}

/// The base text with its guide over it.
fn ruby_preview(ui: &mut egui::Ui, d: &RubyDlg) {
    let (w, h) = (ui.available_width(), 64.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, crate::theme::Paper::BG);
    p.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, Color32::from_gray(180)), egui::StrokeKind::Inside);
    let base_size = (d.base_hp as f32 / 2.0 * 1.6).clamp(10.0, 32.0);
    let ruby_size = (d.size * 1.6).clamp(5.0, 20.0);
    let family = egui::FontFamily::Proportional;
    let base = p.layout_no_wrap(d.base.clone(), egui::FontId::new(base_size, family.clone()), crate::theme::Paper::TEXT);
    let ruby = p.layout_no_wrap(d.text.clone(), egui::FontId::new(ruby_size, family), crate::theme::Paper::TEXT);
    let bw = base.size().x.max(ruby.size().x);
    let x = rect.center().x - bw / 2.0;
    let base_y = rect.bottom() - 8.0 - base.size().y;
    let rx = match d.align {
        "left" => x,
        "right" => x + bw - ruby.size().x,
        _ => x + (bw - ruby.size().x) / 2.0,
    };
    p.galley(pos2(rx, base_y - ruby.size().y + base_size * 0.1 - d.offset), ruby, crate::theme::Paper::TEXT);
    p.galley(pos2(x + (bw - base.size().x) / 2.0, base_y), base, crate::theme::Paper::TEXT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_and_raise_round_trip() {
        assert_eq!(raise_of(0.0, 21), 19);
        assert_eq!(offset_of(raise_of(1.5, 21), 21), 1.5);
        assert_eq!(offset_of(10, 21), 0.0);
    }
}
