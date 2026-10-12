//! Questions and hypotheses: a page with a status (open, supported,
//! refuted) that counts the pages linking to it as support or against.

use apex_doc::notebook::{Claim, ClaimKind, Status};

use super::*;

pub fn status_name(s: Status) -> String {
    tr(match s {
        Status::Open => "nb.status_open",
        Status::Supported => "nb.status_supported",
        Status::Refuted => "nb.status_refuted",
    })
    .to_string()
}

pub fn status_color(s: Status) -> Color32 {
    match s {
        Status::Open => Color32::from_rgb(0xE0, 0xA0, 0x30),
        Status::Supported => Color32::from_rgb(0x4C, 0xAF, 0x50),
        Status::Refuted => Color32::from_rgb(0xE0, 0x55, 0x55),
    }
}

pub(super) fn kind_name(k: ClaimKind) -> String {
    tr(match k {
        ClaimKind::Question => "nb.question",
        ClaimKind::Hypothesis => "nb.hypothesis",
    })
    .to_string()
}

impl NoteView {
    /// A new question after the open page.
    pub fn new_question(&mut self) {
        let s = match self.section {
            Some(s) => s,
            None => self.here(),
        };
        let after = self.page.and_then(|p| self.nb.page(p)).map(|p| (p.parent, p.id));
        let mut id = 0;
        let title = tr("nb.untitled_question").to_string();
        self.change(|nb| {
            id = nb.add_page(s, after.and_then(|a| a.0), after.map(|a| a.1), &title, now());
            if let Some(p) = nb.page_mut(id) {
                p.claim = Some(Claim::new(ClaimKind::Question));
            }
        });
        self.close_graph();
        self.open_page(id);
        self.name_new = true;
        self.grid = false;
    }

    /// Makes page `id` a question, a hypothesis or a plain page.
    pub(super) fn set_claim(&mut self, id: i64, claim: Option<Claim>) {
        if self.nb.page(id).is_none_or(|p| p.claim == claim) {
            return;
        }
        self.change(|nb| {
            if let Some(p) = nb.page_mut(id) {
                p.claim = claim;
            }
        });
    }

    /// The open page's status and evidence, under its title.
    pub(super) fn claim_ui(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.page else { return };
        let Some(claim) = self.nb.page(id).and_then(|p| p.claim) else { return };
        let ev = self.nb.evidence(id);
        let mut set = None;
        let mut open = None;
        ui.add_space(4.0);
        let color = status_color(claim.status);
        egui::Frame::new().fill(color.gamma_multiply(0.12)).stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.6))).corner_radius(4.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for k in [ClaimKind::Question, ClaimKind::Hypothesis] {
                    if ui.selectable_label(claim.kind == k, RichText::new(kind_name(k)).strong()).clicked() {
                        set = Some(Claim { kind: k, ..claim });
                    }
                }
                ui.separator();
                for s in Status::ALL {
                    let text = RichText::new(status_name(s));
                    let text = if claim.status == s { text.strong() } else { text };
                    if ui.selectable_label(claim.status == s, text).clicked() {
                        set = Some(Claim { status: s, ..claim });
                    }
                }
                ui.separator();
                ui.label(RichText::new(trf("nb.evidence_count", &[("for", &ev.supports.len()), ("against", &ev.contradicts.len())])).weak());
            });
            for (key, pages) in [("nb.evidence_for", &ev.supports), ("nb.evidence_against", &ev.contradicts)] {
                if pages.is_empty() {
                    continue;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(tr(key)).small().strong());
                    for p in pages {
                        let title = self.nb.page(*p).map(|p| p.title.clone()).unwrap_or_default();
                        if ui.link(RichText::new(title).small()).clicked() {
                            open = Some(*p);
                        }
                    }
                });
            }
            if ev.supports.is_empty() && ev.contradicts.is_empty() {
                ui.label(RichText::new(tr("nb.evidence_tip")).weak().small());
            }
        });
        if let Some(c) = set {
            self.set_claim(id, Some(c));
        }
        if let Some(p) = open {
            self.open_page(p);
        }
    }
}
