//! Formula aids while typing: function autocomplete, the argument hint and
//! colored references.

use super::*;
use crate::formula_help::{self, REF_COLORS};

impl SheetView {
    /// Matches for the function name being typed (empty when none).
    /// Functions and table names completing the word being typed (None
    /// for a table).
    fn completions(&self) -> Vec<(String, Option<apex_sheet::functions::Func>)> {
        if self.edit.is_none() || self.ac_hidden.as_deref() == Some(self.buf.as_str()) {
            return Vec::new();
        }
        let Some(w) = formula_help::word_at_end(&self.buf) else { return Vec::new() };
        let mut v: Vec<(String, Option<apex_sheet::functions::Func>)> = if self.buf.starts_with('=') { self.table_completions(w).into_iter().chain(self.name_completions(w)).map(|n| (n, None)).collect() } else { Vec::new() };
        v.extend(formula_help::complete(w).into_iter().map(|(n, f)| (n.to_string(), Some(f))));
        // Nothing to suggest when the word is already a complete name.
        if v.len() == 1 && v[0].0.eq_ignore_ascii_case(w) {
            v.clear();
        }
        v.truncate(12);
        v
    }

    /// Up/Down/Tab/Enter/Escape for the autocomplete list, before the
    /// editors see them.
    pub(super) fn autocomplete_keys(&mut self, ui: &egui::Ui) {
        let focused = ui.memory(|m| m.has_focus(self.bar_id()) || m.has_focus(self.cell_edit_id()));
        if !focused {
            return;
        }
        let list = self.completions();
        if list.is_empty() {
            self.ac_index = 0;
            return;
        }
        let consume = |k: Key| ui.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        if consume(Key::ArrowDown) {
            self.ac_index = (self.ac_index + 1) % list.len();
        }
        if consume(Key::ArrowUp) {
            self.ac_index = (self.ac_index + list.len() - 1) % list.len();
        }
        if consume(Key::Escape) {
            self.ac_hidden = Some(self.buf.clone());
            return;
        }
        if consume(Key::Tab) || consume(Key::Enter) {
            let (name, f) = list[self.ac_index.min(list.len() - 1)].clone();
            self.accept_completion(&name, f.is_some());
        }
    }

    fn accept_completion(&mut self, name: &str, function: bool) {
        if let Some(w) = formula_help::word_at_end(&self.buf) {
            let keep = self.buf.len() - w.len();
            self.buf.truncate(keep);
            self.buf.push_str(name);
            self.buf.push(if function { '(' } else { '[' });
            self.caret_to_end = true;
            self.ac_index = 0;
            self.refocus = true;
        }
    }

    /// The autocomplete list and the argument hint under the editor.
    pub(super) fn formula_popups(&mut self, ui: &mut egui::Ui) {
        let Some(anchor) = self.editor_rect else { return };
        if self.edit.is_none() || !self.buf.starts_with('=') {
            return;
        }
        let sep = self.wb.locale.arg_sep;
        let list = self.completions();
        let mut y = anchor.max.y + 2.0;
        if let Some((f, n)) = formula_help::call_at_end(&self.buf, sep).filter(|_| list.is_empty()) {
            let (args, desc) = formula_help::help(f, sep);
            let mut job = LayoutJob::default();
            let base = TextFormat { font_id: FontId::proportional(13.0), color: ui.visuals().text_color(), ..Default::default() };
            let bold = TextFormat { font_id: fonts::font(Face::Sans, true, 13.0), ..base.clone() };
            job.append(&format!("{}(", formula_help::display_name(f)), 0.0, base.clone());
            let parts: Vec<&str> = args.split(sep).collect();
            // Repeating arguments ("…") keep the last one highlighted.
            let cur = if parts.last().is_some_and(|p| p.trim() == "…") && n >= parts.len() - 1 { parts.len().saturating_sub(2) } else { n };
            for (i, p) in parts.iter().enumerate() {
                if i > 0 {
                    job.append(&sep.to_string(), 0.0, base.clone());
                }
                job.append(p.trim_start(), if i > 0 { 4.0 } else { 0.0 }, if i == cur { bold.clone() } else { base.clone() });
            }
            job.append(")", 0.0, base.clone());
            egui::Area::new(Id::new(("arg_hint", self.uid))).order(egui::Order::Tooltip).fixed_pos(pos2(anchor.min.x, y)).interactable(false).show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(460.0);
                    ui.label(job);
                    ui.label(RichText::new(desc).small().weak());
                });
            });
            y += 48.0;
        }
        if list.is_empty() {
            return;
        }
        self.ac_index = self.ac_index.min(list.len() - 1);
        let mut chosen = None;
        egui::Area::new(Id::new(("autocomplete", self.uid))).order(egui::Order::Foreground).fixed_pos(pos2(anchor.min.x, y)).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(380.0);
                for (i, (name, f)) in list.iter().enumerate() {
                    let desc = match f {
                        Some(f) => formula_help::help(*f, sep).1,
                        None => tr("xtab.completion"),
                    };
                    let r = ui.add(egui::Button::selectable(i == self.ac_index, RichText::new(name.as_str()).monospace().strong()).right_text(RichText::new(desc).small().weak()).min_size(vec2(370.0, 0.0)).truncate());
                    if r.clicked() {
                        chosen = Some((name.clone(), f.is_some()));
                    }
                }
                ui.label(RichText::new(tr("fx.autocomplete_hint")).small().weak());
            });
        });
        if let Some((n, f)) = chosen {
            self.accept_completion(&n, f);
        }
    }

    /// References of the formula being edited on this sheet, with colors.
    pub(super) fn edited_refs(&self) -> Vec<(Range, Color32)> {
        if self.edit.is_none() || !self.buf.starts_with('=') {
            return Vec::new();
        }
        let name = &self.sh().name;
        formula_help::references(&self.buf)
            .into_iter()
            .enumerate()
            .filter(|(_, r)| r.sheet.as_ref().is_none_or(|s| s.eq_ignore_ascii_case(name)))
            .map(|(i, r)| (r.range, REF_COLORS[i % REF_COLORS.len()]))
            .collect()
    }
}

/// Lays out formula text with each reference in its color.
pub(super) fn formula_job(text: &str, font: FontId, color: Color32, wrap: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    let fmt = |c: Color32| TextFormat { font_id: font.clone(), color: c, ..Default::default() };
    let mut at = 0;
    if text.starts_with('=') {
        for (i, r) in formula_help::references(text).into_iter().enumerate() {
            if r.span.start < at || r.span.end > text.len() {
                continue;
            }
            job.append(&text[at..r.span.start], 0.0, fmt(color));
            job.append(&text[r.span.clone()], 0.0, fmt(REF_COLORS[i % REF_COLORS.len()]));
            at = r.span.end;
        }
    }
    job.append(&text[at..], 0.0, fmt(color));
    job
}
