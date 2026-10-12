//! Navigation pane: the outline of headings (click to go, drag to move a
//! whole section) and the find results with their context.

use std::collections::HashSet;

use apex_doc::OBJECT;

use super::*;
use dialogs::FindBar;

#[derive(Default)]
pub(super) struct NavPane {
    /// 0 headings, 1 results.
    tab: usize,
    /// Collapsed headings (paragraph indices; cleared when paragraphs move).
    collapsed: HashSet<usize>,
    /// Headings: paragraph, level and text; for revision `rev`.
    heads: Vec<(usize, u8, String)>,
    rev: Option<u64>,
    focus_query: bool,
}

/// Text shown for a heading or result.
fn clean(s: &str) -> String {
    s.chars().filter(|&c| c != OBJECT).map(|c| if c.is_control() || c == LINE_BREAK { ' ' } else { c }).collect::<String>().trim().to_string()
}

impl DocView {
    pub(super) fn nav_open(&self) -> bool {
        self.refs.nav.is_some()
    }

    pub(super) fn toggle_nav(&mut self) {
        if self.refs.nav.take().is_none() {
            self.refs.nav = Some(NavPane::default());
        } else if self.find.as_ref().is_some_and(|f| f.nav) {
            self.find = None;
        }
    }

    /// The pane, on the left of the page.
    pub(super) fn nav_pane(&mut self, ui: &mut egui::Ui) {
        let Some(mut nav) = self.refs.nav.take() else { return };
        let mut close = false;
        egui::Panel::left(Id::new(("doc_nav", self.uid)))
            .resizable(true)
            .default_size(240.0)
            .min_size(160.0)
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::same(6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(tr("nav.title"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text(tr("dialog.close")).clicked() {
                            close = true;
                        }
                    });
                });
                // Search box: its results are the find bar's.
                let f = self.find.get_or_insert_with(|| FindBar { nav: true, ..Default::default() });
                let q = ui.add(egui::TextEdit::singleline(&mut f.query).hint_text(tr("nav.search_hint")).desired_width(f32::INFINITY));
                if std::mem::take(&mut nav.focus_query) {
                    q.request_focus();
                }
                if q.changed() && !f.query.is_empty() {
                    nav.tab = 1;
                }
                if q.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    self.find_next(ui.input(|i| i.modifiers.shift));
                    nav.tab = 1;
                }
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut nav.tab, 0, tr("nav.headings"));
                    ui.selectable_value(&mut nav.tab, 1, tr("nav.results"));
                });
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if nav.tab == 0 {
                        self.nav_headings(ui, &mut nav);
                    } else {
                        self.nav_results(ui);
                    }
                });
            });
        if close {
            if self.find.as_ref().is_some_and(|f| f.nav) {
                self.find = None;
            }
            self.focus = true;
        } else {
            self.refs.nav = Some(nav);
        }
    }

    fn nav_headings(&mut self, ui: &mut egui::Ui, nav: &mut NavPane) {
        if nav.rev != Some(self.doc.revision) {
            let before = nav.heads.iter().map(|h| h.0).collect::<Vec<_>>();
            nav.heads = self.doc.headings().into_iter().map(|(i, l)| (i, l, clean(&self.doc.paragraphs[i].text))).collect();
            if before != nav.heads.iter().map(|h| h.0).collect::<Vec<_>>() {
                nav.collapsed.clear();
            }
            nav.rev = Some(self.doc.revision);
        }
        if nav.heads.is_empty() {
            ui.label(RichText::new(tr("nav.no_headings")).weak());
            return;
        }
        let cur = self.sel.cursor.para;
        let current = nav.heads.iter().rposition(|h| h.0 <= cur);
        let row_h = 22.0;
        let mut hidden_below: Option<u8> = None;
        let mut goto = None;
        let mut drop: Option<(usize, usize)> = None;
        let dragging = egui::DragAndDrop::has_payload_of_type::<usize>(ui.ctx());
        let n = nav.heads.len();
        for hi in 0..n {
            let (para, level, ref text) = nav.heads[hi];
            if let Some(l) = hidden_below {
                if level > l {
                    continue;
                }
                hidden_below = None;
            }
            let has_children = nav.heads.get(hi + 1).is_some_and(|h| h.1 > level);
            let collapsed = nav.collapsed.contains(&para);
            if collapsed && has_children {
                hidden_below = Some(level);
            }
            // Rows out of view only take their space.
            let next = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), row_h));
            if !ui.is_rect_visible(next) {
                ui.allocate_space(next.size());
                continue;
            }
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
            let indent = 4.0 + (level.saturating_sub(1)) as f32 * 14.0;
            let vis = ui.visuals();
            if Some(hi) == current {
                ui.painter().rect_filled(rect, 3.0, vis.selection.bg_fill.gamma_multiply(0.35));
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 3.0, vis.widgets.hovered.weak_bg_fill);
            }
            // Expand / collapse triangle (drawn).
            let tri = Rect::from_center_size(pos2(rect.min.x + indent + 6.0, rect.center().y), vec2(10.0, 10.0));
            if has_children {
                let c = vis.text_color();
                let pts = if collapsed {
                    vec![tri.left_top() + vec2(2.0, 0.0), tri.right_center(), tri.left_bottom() + vec2(2.0, 0.0)]
                } else {
                    vec![tri.left_top() + vec2(0.0, 2.0), tri.right_top() + vec2(0.0, 2.0), tri.center_bottom()]
                };
                ui.painter().add(egui::Shape::convex_polygon(pts, c.gamma_multiply(0.7), Stroke::NONE));
            }
            let font = egui::FontId::proportional(if level == 1 { 13.5 } else { 13.0 });
            let color = if level == 1 { vis.strong_text_color() } else { vis.text_color() };
            let text_rect = Rect::from_min_max(pos2(tri.max.x + 4.0, rect.min.y), rect.max);
            let g = ui.painter().layout(text.clone(), font, color, text_rect.width().max(10.0));
            let clip = ui.painter().with_clip_rect(text_rect.intersect(ui.clip_rect()));
            clip.galley(pos2(text_rect.min.x, rect.center().y - g.rows.first().map_or(8.0, |r| r.row.size.y / 2.0)), g, color);
            let resp = resp.on_hover_text(text.as_str());
            let on_tri = resp.interact_pointer_pos().is_some_and(|p| p.x < tri.max.x + 2.0);
            if resp.clicked() {
                if has_children && on_tri {
                    if !nav.collapsed.remove(&para) {
                        nav.collapsed.insert(para);
                    }
                } else {
                    goto = Some(para);
                }
            }
            resp.dnd_set_drag_payload(hi);
            // Where a dragged heading goes: before this one, or after it at
            // the end of the list.
            if dragging && let Some(p) = ui.ctx().pointer_interact_pos() && rect.contains(p) {
                let after = hi + 1 == n && p.y > rect.center().y;
                let y = if after { rect.max.y } else { rect.min.y };
                ui.painter().hline(rect.x_range(), y, Stroke::new(2.0, vis.selection.stroke.color));
                if let Some(from) = resp.dnd_release_payload::<usize>() {
                    let to = if after { self.doc.paragraphs.len() } else { para };
                    drop = Some((*from, to));
                }
            }
        }
        if let Some((from, to)) = drop
            && let Some(&(fp, _, _)) = nav.heads.get(from)
        {
            match self.doc.move_section(fp, to, self.sel) {
                Some(s) => self.set_sel(s),
                None => self.notices.push(Notice::info(tr("nav.cant_move"))),
            }
        }
        if let Some(p) = goto {
            self.go_to_para(p);
        }
    }

    fn nav_results(&mut self, ui: &mut egui::Ui) {
        let hits = self.find_hits().to_vec();
        let query_empty = self.find.as_ref().is_none_or(|f| f.query.is_empty());
        if query_empty {
            ui.label(RichText::new(tr("nav.results_hint")).weak());
            return;
        }
        ui.label(RichText::new(match hits.len() {
            1 => tr("find.one_match").to_string(),
            n => trf("find.matches", &[("n", &n)]),
        }).weak());
        let cur = (self.sel.start(), self.sel.end());
        let mut pick = None;
        // Long lists show their first part.
        for (i, &(a, b)) in hits.iter().enumerate().take(500) {
            let text = &self.doc.paragraphs[a.para].text;
            let end = if b.para == a.para { b.offset } else { text.len() };
            let before: String = text[..a.offset].chars().rev().take(28).collect::<Vec<_>>().into_iter().rev().collect();
            let after: String = text[end..].chars().take(40).collect();
            let mut job = egui::text::LayoutJob::default();
            let vis = ui.visuals();
            let plain = TextFormat { font_id: egui::FontId::proportional(12.5), color: vis.text_color(), ..Default::default() };
            let strong = TextFormat { background: Color32::from_rgba_unmultiplied(255, 200, 0, 110), color: vis.strong_text_color(), ..plain.clone() };
            job.append(&format!("{}{}", if before.len() < text[..a.offset].len() { "…" } else { "" }, clean(&before)), 0.0, plain.clone());
            job.append(&clean(&text[a.offset..end]), 0.0, strong);
            job.append(&clean(&after), 0.0, plain);
            job.wrap.max_width = ui.available_width() - 8.0;
            let r = ui.selectable_label(cur == (a, b), job);
            if r.clicked() {
                pick = Some(i);
            }
        }
        if let Some(i) = pick {
            let (a, b) = hits[i];
            self.set_sel(Selection { anchor: a, cursor: b });
        }
    }

    /// Caret at the start of paragraph `p`, scrolled into view.
    pub(crate) fn go_to_para(&mut self, p: usize) {
        self.set_sel(Selection::caret(Pos::new(p.min(self.doc.paragraphs.len() - 1), 0)));
        self.desired_x = None;
        self.focus = true;
    }
}
