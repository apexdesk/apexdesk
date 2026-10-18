//! Spelling (F7) in spreadsheets: goes through the text cells of the
//! sheet from the active cell, row by row.

use std::ops::Range;
use std::sync::Arc;

use apex_sheet::{CellRef, Content, Value};
use eframe::egui;

use super::SheetView;
use crate::spell;

pub(super) struct SpellRun {
    dlg: spell::Dialog,
    at: (CellRef, Range<usize>),
    origin: CellRef,
    wrapped: bool,
}

/// Row-major order, like Excel's check "by rows".
fn key(c: CellRef) -> (u32, u32) {
    (c.row, c.col)
}

/// Typed text that stays text (not a formula or a number) when entered.
fn as_input(text: &str) -> String {
    if text.starts_with(['=', '+', '-', '\'']) || text.trim().parse::<f64>().is_ok() { format!("'{text}") } else { text.to_string() }
}

impl SheetView {
    /// Text constants of the sheet in row-major order.
    fn text_cells(&self) -> Vec<(CellRef, Arc<str>)> {
        let Some(sh) = self.wb.sheets.get(self.sheet) else { return Vec::new() };
        let mut v: Vec<(CellRef, Arc<str>)> = sh
            .iter()
            .filter_map(|(c, cell)| match &cell.content {
                Content::Value(Value::Text(t)) => Some((c, t.clone())),
                _ => None,
            })
            .collect();
        v.sort_by_key(|(c, _)| key(*c));
        v
    }

    /// The first misspelled word from `(cell, byte)` on, before `limit`.
    fn next_misspelling(cells: &[(CellRef, Arc<str>)], from: (CellRef, usize), limit: Option<CellRef>) -> Option<(CellRef, Range<usize>, Arc<str>)> {
        let i = cells.partition_point(|(c, _)| key(*c) < key(from.0));
        for (c, t) in &cells[i..] {
            if limit.is_some_and(|l| key(*c) >= key(l)) {
                break;
            }
            let lo = if *c == from.0 { from.1 } else { 0 };
            if let Some(r) = spell::misspelled(t).into_iter().find(|r| r.start >= lo) {
                return Some((*c, r, t.clone()));
            }
        }
        None
    }

    pub(super) fn start_spelling(&mut self) {
        if !self.settle_edit() {
            return;
        }
        let origin = self.cursor;
        let mut run = SpellRun { dlg: spell::Dialog::default(), at: (origin, 0..0), origin, wrapped: false };
        if !spell::ready() {
            run.dlg.finish();
        }
        self.spell = Some(run);
        if spell::ready() {
            self.spell_advance((origin, 0));
        }
    }

    fn spell_advance(&mut self, from: (CellRef, usize)) {
        let Some(run) = &self.spell else { return };
        let (origin, wrapped) = (run.origin, run.wrapped);
        let cells = self.text_cells();
        let mut hit = None;
        let mut wrap = wrapped;
        if !wrapped {
            hit = Self::next_misspelling(&cells, from, None);
            wrap = hit.is_none();
        }
        if hit.is_none() {
            let from = if wrapped { from } else { (CellRef::new(0, 0), 0) };
            hit = Self::next_misspelling(&cells, from, Some(origin));
        }
        let Some(run) = self.spell.as_mut() else { return };
        run.wrapped = wrap;
        match hit {
            Some((c, r, text)) => {
                run.dlg.found(&text, r.clone());
                run.at = (c, r);
                self.anchor = c;
                self.cursor = c;
                self.ensure_visible = true;
            }
            None => run.dlg.finish(),
        }
    }

    pub(super) fn spelling_dialog(&mut self, ctx: &egui::Context) {
        let Some(run) = self.spell.as_mut() else { return };
        let Some(action) = run.dlg.ui(ctx) else { return };
        let (cell, r) = run.at.clone();
        let word = run.dlg.word.clone();
        let text = match self.wb.cell(self.sheet, cell).map(|c| &c.content) {
            Some(Content::Value(Value::Text(t))) => Some(t.clone()),
            _ => None,
        };
        let still = text.as_deref().and_then(|t| t.get(r.clone())) == Some(word.as_str());
        match action {
            spell::Action::Close => self.spell = None,
            // (Sheets aren't checked for grammar.)
            spell::Action::Recheck | spell::Action::IgnoreRule => self.spell_advance((cell, r.start)),
            spell::Action::IgnoreOnce => self.spell_advance((cell, if still { r.end } else { r.start })),
            spell::Action::IgnoreAll => {
                spell::ignore_all(&word);
                self.spell_advance((cell, r.start));
            }
            spell::Action::Add => {
                spell::add_word(&word);
                self.spell_advance((cell, r.start));
            }
            spell::Action::Change(to) => match text.filter(|_| still) {
                Some(t) if !self.wb.is_protected(self.sheet) => {
                    let new = format!("{}{to}{}", &t[..r.start], &t[r.end..]);
                    self.wb.set_input(self.sheet, cell, &as_input(&new));
                    self.spell_advance((cell, r.start + to.len()));
                }
                _ => self.spell_advance((cell, r.end)),
            },
            spell::Action::ChangeAll(to) => {
                if !self.wb.is_protected(self.sheet) {
                    let inputs: Vec<(CellRef, String)> = self
                        .text_cells()
                        .into_iter()
                        .filter_map(|(c, t)| {
                            let bad = spell::misspelled(&t);
                            let hits: Vec<&Range<usize>> = bad.iter().filter(|b| t[(*b).clone()] == *word).collect();
                            if hits.is_empty() {
                                return None;
                            }
                            let mut out = String::new();
                            let mut last = 0;
                            for h in hits {
                                out.push_str(&t[last..h.start]);
                                out.push_str(&to);
                                last = h.end;
                            }
                            out.push_str(&t[last..]);
                            Some((c, as_input(&out)))
                        })
                        .collect();
                    if !inputs.is_empty() {
                        self.wb.set_inputs(self.sheet, inputs);
                    }
                }
                self.spell_advance((cell, r.start));
            }
        }
    }
}
