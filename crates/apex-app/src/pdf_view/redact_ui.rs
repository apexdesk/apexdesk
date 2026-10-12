//! The PDF tab's Redact toolstrip tab: marking text, areas or what matches a
//! pattern (e-mails, ID numbers, IBANs, card and phone numbers, the search) for
//! redaction, then taking it out of the file for good, and removing
//! hidden information.

use apex_io::pdf::edit::annots::{Annot, Kind};
use super::annotate::line_boxes;
use super::{PdfView, Tool, text};
use crate::app::Notice;
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::pii;
use crate::toolstrip::{self, Body};

/// What "Find and mark" looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    Email,
    /// Spanish DNI and NIE numbers.
    IdNumber,
    Iban,
    Card,
    Phone,
    /// What the find bar matches.
    Search,
}

impl Pattern {
    pub const ALL: [Pattern; 6] = [Pattern::Email, Pattern::IdNumber, Pattern::Iban, Pattern::Card, Pattern::Phone, Pattern::Search];

    fn key(self) -> &'static str {
        match self {
            Pattern::Email => "pdf.redact_emails",
            Pattern::IdNumber => "pdf.redact_ids",
            Pattern::Iban => "pdf.redact_ibans",
            Pattern::Card => "pdf.redact_cards",
            Pattern::Phone => "pdf.redact_phones",
            Pattern::Search => "pdf.redact_search",
        }
    }

    /// The personal data it finds.
    fn kind(self) -> Option<pii::Kind> {
        Some(match self {
            Pattern::Email => pii::Kind::Email,
            Pattern::IdNumber => pii::Kind::IdNumber,
            Pattern::Iban => pii::Kind::Iban,
            Pattern::Card => pii::Kind::Card,
            Pattern::Phone => pii::Kind::Phone,
            Pattern::Search => return None,
        })
    }

    /// The matches on a page: first and last char indices. What looks
    /// like one counts, check digits or not.
    pub fn find(self, p: &apex_io::pdf::Page) -> Vec<(usize, usize)> {
        let Some(k) = self.kind() else { return Vec::new() };
        matches(p, k.regex(), |s| k.accept(s, false))
    }
}

/// The matches of `re` on a page: first and last char indices.
pub fn matches(p: &apex_io::pdf::Page, re: &regex::Regex, accept: impl Fn(&str) -> bool) -> Vec<(usize, usize)> {
    let (chars, map) = text::page_string(p);
    let s: String = chars.iter().collect();
    // Byte offset of each char of `s`.
    let offs: Vec<usize> = s.char_indices().map(|(o, _)| o).collect();
    let at = |byte: usize| offs.partition_point(|&o| o < byte);
    let mut out = Vec::new();
    for m in re.find_iter(&s).filter(|m| accept(m.as_str())) {
        let (a, b) = (at(m.start()), at(m.end()));
        let first = map[a..b].iter().flatten().next();
        let last = map[a..b].iter().flatten().next_back();
        if let (Some(&x), Some(&y)) = (first, last) {
            out.push((x, y));
        }
    }
    out
}

impl PdfView {
    pub(super) fn redact_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("pdf.g.redact_mark", Icon::Eraser, |ui| {
            for (t, icon, key, tip) in [
                (Tool::Markup(Kind::Redact), Icon::Select, "pdf.redact_text", "pdf.redact_text_tip"),
                (Tool::Draw(Kind::Redact), Icon::Lasso, "pdf.redact_area", "pdf.redact_area_tip"),
            ] {
                if toolstrip::large(ui, icon, tr(key), tr(tip), acc, self.tool == t).clicked() {
                    self.set_tool(t);
                }
            }
            toolstrip::large_menu(ui, Icon::Search, tr("pdf.redact_find"), tr("pdf.redact_find_tip"), acc, |ui| {
                for p in Pattern::ALL {
                    if ui.button(tr(p.key())).clicked() {
                        self.mark_pattern(p);
                        ui.close();
                    }
                }
            });
        });
        b.group("pdf.g.redact_apply", Icon::Protect, |ui| {
            let marks = self.doc.redaction_marks().iter().map(|m| m.1.len()).sum::<usize>();
            ui.add_enabled_ui(marks > 0, |ui| {
                if toolstrip::large(ui, Icon::Accept, tr("pdf.redact_apply"), &trf("pdf.redact_apply_tip", &[("n", &marks)]), acc, false).clicked() {
                    self.apply_redactions();
                }
            });
            if toolstrip::large(ui, Icon::ClearFormat, tr("pdf.sanitize"), tr("pdf.sanitize_tip"), acc, false).clicked() {
                self.doc.sanitize();
                self.after_change();
                self.notices.push(Notice::info(tr("pdf.sanitized")));
            }
        });
    }

    /// Marks for redaction every match of `p`; returns how many.
    pub fn mark_pattern(&mut self, p: Pattern) -> usize {
        let Some(pages) = self.text.pages.clone() else {
            self.notices.push(Notice::info(tr("pdf.redact_wait")));
            return 0;
        };
        let hits: Vec<(usize, usize, usize)> = match p {
            Pattern::Search => text::search(&pages, &self.find.query, self.find.case).into_iter().map(|s| (s.page, s.a, s.b)).collect(),
            _ => pages.iter().enumerate().flat_map(|(i, pg)| p.find(pg).into_iter().map(move |(a, b)| (i, a, b))).collect(),
        };
        if hits.is_empty() {
            self.notices.push(Notice::info(tr("pdf.redact_none")));
            return 0;
        }
        self.doc.begin("pdf.undo.redact_mark");
        for &(i, a, b) in &hits {
            let quads = line_boxes(&pages[i], a, b);
            let Some(rect) = quads.iter().copied().reduce(|x, y| [x[0].min(y[0]), x[1].min(y[1]), x[2].max(y[2]), x[3].max(y[3])]) else { continue };
            self.doc.add_annot(&Annot { page: i, kind: Kind::Redact, rect, quads, color: 0xFF0000, author: self.author.clone(), ..Default::default() });
        }
        self.doc.end();
        self.after_change();
        self.notices.push(Notice::info(trf("pdf.redact_marked", &[("n", &hits.len())])));
        hits.len()
    }

    /// Takes what is under the marks out of the file.
    pub fn apply_redactions(&mut self) {
        let signed = self.doc.is_signed();
        let r = self.doc.apply_redactions();
        self.after_change();
        self.picked = None;
        let msg = trf("pdf.redacted", &[("chars", &r.glyphs), ("images", &r.images), ("paths", &r.paths)]);
        self.notices.push(Notice::info(if signed { format!("{msg} {}", tr("pdf.redact_signed")) } else { msg }));
    }
}
