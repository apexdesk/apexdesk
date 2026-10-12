//! The notebook as a graph: its pages as dots joined by their links,
//! laid out by forces (linked pages pull together, all push apart). The
//! whole notebook, a section or a tag, or only around the open page.

use std::collections::{HashMap, HashSet, VecDeque};

use apex_doc::notebook::Rel;

/// The most pages the graph shows.
const MAX_NODES: usize = 3000;
use eframe::egui::{Pos2, Stroke, Vec2, pos2};

use super::*;

/// Distance linked pages settle at, in graph units.
const SPRING: f32 = 90.0;
/// Below this much movement a step, the layout is still.
const STILL: f32 = 0.3;

/// The color of a link that says `rel`.
pub fn rel_color(rel: Rel) -> Color32 {
    match rel {
        Rel::Link => Color32::from_gray(140),
        Rel::Supports => Color32::from_rgb(0x3F, 0xA6, 0x72),
        Rel::Contradicts => Color32::from_rgb(0xD4, 0x58, 0x4A),
        Rel::Expands => Color32::from_rgb(0x4A, 0x7F, 0xD4),
        Rel::Example => Color32::from_rgb(0xD9, 0xA2, 0x3B),
        Rel::Question => Color32::from_rgb(0x8A, 0x63, 0xC9),
        Rel::Causes => Color32::from_rgb(0x2F, 0xA3, 0xB5),
    }
}

/// Where each page is, moved by forces a step at a time.
#[derive(Default)]
pub struct Layout {
    pub pos: HashMap<i64, Vec2>,
    /// Pages held where they are (being dragged).
    pub pinned: HashSet<i64>,
    heat: f32,
}

impl Layout {
    /// A spot for a page not placed yet: near a placed neighbour, or on a
    /// spiral by its id.
    fn place(&mut self, id: i64, near: Option<Vec2>) {
        let a = id as f32 * 2.399_963;
        let r = 30.0 + 12.0 * (id.unsigned_abs() as f32).sqrt();
        let p = near.map_or(Vec2::new(a.cos() * r, a.sin() * r), |n| n + Vec2::new(a.cos(), a.sin()) * 40.0);
        self.pos.insert(id, p);
        self.heat = self.heat.max(1.0);
    }

    /// Makes sure every node has a place.
    pub fn ensure(&mut self, nodes: &[i64], edges: &[(i64, i64, Rel)]) {
        for &n in nodes {
            if !self.pos.contains_key(&n) {
                let near = edges.iter().find_map(|&(a, b, _)| if a == n { self.pos.get(&b).copied() } else if b == n { self.pos.get(&a).copied() } else { None });
                self.place(n, near);
            }
        }
    }

    /// Heats it up again (something changed).
    pub fn wake(&mut self) {
        self.heat = 1.0;
    }

    /// One step of the forces; how far the pages moved at most.
    pub fn step(&mut self, nodes: &[i64], edges: &[(i64, i64, Rel)]) -> f32 {
        if self.heat < 0.02 {
            return 0.0;
        }
        let k2 = SPRING * SPRING;
        let mut force: HashMap<i64, Vec2> = nodes.iter().map(|&n| (n, Vec2::ZERO)).collect();
        for (i, &a) in nodes.iter().enumerate() {
            let pa = self.pos[&a];
            for &b in &nodes[i + 1..] {
                let mut d = pa - self.pos[&b];
                let mut l = d.length();
                if l < 0.01 {
                    d = Vec2::new((a - b) as f32 % 3.0 + 0.5, 1.0);
                    l = d.length();
                }
                // Far apart, they hardly push.
                if l > SPRING * 6.0 {
                    continue;
                }
                let f = d / l * (k2 / l);
                *force.get_mut(&a).unwrap() += f;
                *force.get_mut(&b).unwrap() -= f;
            }
        }
        for &(a, b, _) in edges {
            let (Some(pa), Some(pb)) = (self.pos.get(&a), self.pos.get(&b)) else { continue };
            let d = *pb - *pa;
            let l = d.length().max(0.01);
            let f = d / l * (l * l / SPRING);
            if let Some(x) = force.get_mut(&a) {
                *x += f;
            }
            if let Some(x) = force.get_mut(&b) {
                *x -= f;
            }
        }
        let max_step = 30.0 * self.heat;
        let mut moved: f32 = 0.0;
        for &n in nodes {
            if self.pinned.contains(&n) {
                continue;
            }
            let p = self.pos.get_mut(&n).unwrap();
            // Towards the middle, so loose groups stay near.
            let f = force[&n] - *p * 0.06;
            let l = f.length();
            if l > 0.0 {
                let mv = f / l * l.min(max_step);
                *p += mv;
                moved = moved.max(mv.length());
            }
        }
        self.heat *= 0.985;
        if moved < STILL {
            self.heat *= 0.8;
        }
        moved
    }
}

/// What the graph shows and how it is looked at.
pub struct Graph {
    pub layout: Layout,
    /// Graph point at the middle of the view, and pixels per unit.
    pub center: Vec2,
    pub zoom: f32,
    pub section: Option<i64>,
    pub tag: Option<String>,
    /// Only the pages within `depth` links of the open page.
    pub local: bool,
    pub depth: usize,
    /// Pages with no links show (otherwise they are only listed).
    pub loose: bool,
    /// Pages dropped on the graph from the list: they show whatever the
    /// filters say.
    pub added: HashSet<i64>,
    dragging: Option<i64>,
    /// The page whose menu is open.
    menu: Option<i64>,
    /// A link being drawn from this page (Shift+drag).
    linking: Option<i64>,
    /// A link drawn, waiting for its meaning: from, to, where the menu goes.
    pending: Option<(i64, i64, Pos2)>,
    /// The view follows the layout until the user moves it.
    fit: bool,
}

impl Default for Graph {
    fn default() -> Self {
        Graph { layout: Layout::default(), center: Vec2::ZERO, zoom: 1.0, section: None, tag: None, local: false, depth: 2, loose: false, added: HashSet::new(), dragging: None, menu: None, linking: None, pending: None, fit: true }
    }
}

impl NoteView {
    /// The pages and links the graph shows, and the pages with no links.
    pub(super) fn graph_parts(&self, g: &Graph) -> (Vec<i64>, Vec<(i64, i64, Rel)>, Vec<i64>) {
        let tagged: Option<HashSet<i64>> = g.tag.as_ref().map(|t| self.nb.tagged(t).into_iter().collect());
        // Found once for all the pages: this runs every frame.
        let live = self.nb.live_set();
        let mut nodes: Vec<i64> = self
            .nb
            .pages
            .iter()
            .filter(|p| live.contains(&p.id) && (g.added.contains(&p.id) || g.section.is_none_or(|s| p.section == s) && tagged.as_ref().is_none_or(|t| t.contains(&p.id))))
            .map(|p| p.id)
            .collect();
        nodes.sort_unstable();
        nodes.dedup();
        // The layout pulls each page from every other: past this many
        // they aren't all drawn.
        nodes.truncate(MAX_NODES);
        let set: HashSet<i64> = nodes.iter().copied().collect();
        // What each page links to: in its text, then by arrows on live
        // canvases (as `outgoing` says, gathered once).
        let mut arrows: HashMap<i64, Vec<(i64, Rel)>> = HashMap::new();
        for (c, v) in &self.nb.canvas_links {
            if live.contains(c) {
                for &(a, b, rel) in v {
                    arrows.entry(a).or_default().push((b, rel));
                }
            }
        }
        let mut edges: Vec<(i64, i64, Rel)> = Vec::new();
        let mut seen: HashSet<(i64, i64)> = HashSet::new();
        for &a in &nodes {
            let text = self.nb.links.get(&a).into_iter().flatten().copied();
            for (b, rel) in text.chain(arrows.get(&a).into_iter().flatten().copied()) {
                if b != a && set.contains(&b) && seen.insert((a.min(b), a.max(b))) {
                    edges.push((a, b, rel));
                }
            }
        }
        if g.local
            && let Some(start) = self.page
        {
            let mut keep: HashSet<i64> = HashSet::from([start]);
            keep.extend(g.added.iter().copied());
            let mut q = VecDeque::from([(start, 0usize)]);
            while let Some((n, d)) = q.pop_front() {
                if d == g.depth {
                    continue;
                }
                for &(a, b, _) in &edges {
                    let other = if a == n { b } else if b == n { a } else { continue };
                    if keep.insert(other) {
                        q.push_back((other, d + 1));
                    }
                }
            }
            nodes.retain(|n| keep.contains(n));
            edges.retain(|(a, b, _)| keep.contains(a) && keep.contains(b));
        }
        let linked: HashSet<i64> = edges.iter().flat_map(|&(a, b, _)| [a, b]).collect();
        let loose: Vec<i64> = nodes.iter().copied().filter(|n| !linked.contains(n) && !g.added.contains(n) && Some(*n) != self.page.filter(|_| g.local)).collect();
        if !g.loose {
            let loose: HashSet<i64> = loose.iter().copied().collect();
            nodes.retain(|n| !loose.contains(n));
        }
        (nodes, edges, loose)
    }

    /// Page `from` links to page `to` meaning `rel`: a paragraph with the
    /// link at the end of `from`.
    pub(super) fn connect(&mut self, from: i64, to: i64, rel: Rel) {
        if self.nb.outgoing(from).iter().any(|t| t.0 == to) {
            return;
        }
        let title = self.title_of(to);
        self.edit_page(from, |d| {
            if d.paragraphs.last().is_none_or(|p| !p.text.is_empty()) {
                d.paragraphs.push(apex_doc::Paragraph::new(""));
            }
            let end = apex_doc::Selection::caret(d.end());
            d.insert_link(end, &title, apex_doc::notebook::links::target(to, rel), None);
            true
        });
        if self.page == Some(from) {
            self.flush();
        }
    }

    pub fn open_graph(&mut self) {
        self.flush();
        let graph = self.graph.take();
        self.close_graph();
        self.graph = graph;
        if self.graph.is_none() {
            self.graph = Some(Box::default());
        }
    }

    /// Closes the floating windows (trash, history, types): one at a time.
    pub(super) fn close_windows(&mut self) {
        self.trash_open = false;
        self.history = None;
        self.types_win = None;
    }

    /// Back from the graph or the tasks to the pages.
    pub fn close_graph(&mut self) {
        self.graph = None;
        self.tasks = None;
        self.gallery = None;
        self.table = None;
        self.timeline = None;
    }

    /// A view (graph, tasks, gallery, table, timeline) covers the page.
    pub(super) fn view_open(&self) -> bool {
        self.graph.is_some() || self.tasks.is_some() || self.gallery.is_some() || self.table.is_some() || self.timeline.is_some()
    }

    pub(super) fn graph_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut g) = self.graph.take() else { return };
        let (nodes, edges, loose) = self.graph_parts(&g);
        let mut open = None;
        let mut close = false;
        // The filters.
        egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(tr("nb.graph")).strong().size(16.0));
                ui.add_space(12.0);
                let name = g.section.and_then(|s| self.nb.section(s)).map_or_else(|| tr("nb.all_sections").to_string(), |s| s.name.clone());
                egui::ComboBox::from_id_salt(("graph_section", self.uid)).selected_text(name).show_ui(ui, |ui| {
                    if ui.selectable_label(g.section.is_none(), tr("nb.all_sections")).clicked() {
                        g.section = None;
                        g.layout.wake();
                    }
                    for s in self.nb.live_sections() {
                        if ui.selectable_label(g.section == Some(s.id), &s.name).clicked() {
                            g.section = Some(s.id);
                            g.layout.wake();
                        }
                    }
                });
                let tags = self.nb.tags();
                if !tags.is_empty() {
                    let name = g.tag.as_ref().map_or_else(|| tr("nb.all_tags").to_string(), |t| format!("#{t}"));
                    egui::ComboBox::from_id_salt(("graph_tag", self.uid)).selected_text(name).show_ui(ui, |ui| {
                        if ui.selectable_label(g.tag.is_none(), tr("nb.all_tags")).clicked() {
                            g.tag = None;
                            g.layout.wake();
                        }
                        for t in tags.keys() {
                            if ui.selectable_label(g.tag.as_ref() == Some(t), format!("#{t}")).clicked() {
                                g.tag = Some(t.clone());
                                g.layout.wake();
                            }
                        }
                    });
                }
                if ui.add_enabled(self.page.is_some(), egui::Checkbox::new(&mut g.local, tr("nb.graph_local"))).changed() {
                    g.layout.wake();
                }
                if g.local {
                    ui.add(egui::Slider::new(&mut g.depth, 1..=4).text(tr("nb.graph_steps")));
                }
                if ui.checkbox(&mut g.loose, trf("nb.graph_loose", &[("n", &loose.len())])).changed() {
                    g.layout.wake();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("nb.graph_close")).clicked() {
                        close = true;
                    }
                    if ui.button(tr("nb.graph_fit")).clicked() {
                        g.fit = true;
                    }
                });
            });
        });
        // Pages with no links, listed aside when they don't show.
        if !g.loose && !loose.is_empty() {
            egui::Panel::right(Id::new(("graph_loose", self.uid))).resizable(true).default_size(180.0).show(ui, |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new(trf("nb.graph_loose", &[("n", &loose.len())])).strong());
                ui.label(RichText::new(tr("nb.graph_loose_tip")).weak().small());
                egui::ScrollArea::vertical().id_salt("graph_loose_scroll").show(ui, |ui| {
                    for id in &loose {
                        if ui.link(self.title_of(*id)).clicked() {
                            open = Some(*id);
                        }
                    }
                });
            });
        }
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        g.layout.ensure(&nodes, &edges);
        // The pages put by hand stay put and push no one away; their links
        // still pull.
        let moving: Vec<i64> = nodes.iter().copied().filter(|n| !g.added.contains(n)).collect();
        for _ in 0..4 {
            g.layout.step(&moving, &edges);
        }
        if g.layout.heat > 0.02 {
            ui.ctx().request_repaint();
        }
        let mid = rect.center();
        // Everything in view.
        if g.fit && !nodes.is_empty() {
            let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for n in &nodes {
                let p = g.layout.pos[n];
                lo = lo.min(p);
                hi = hi.max(p);
            }
            g.center = (lo + hi) / 2.0;
            let span = (hi - lo).max(Vec2::splat(1.0));
            g.zoom = ((rect.width() - 160.0) / span.x).min((rect.height() - 100.0) / span.y).clamp(0.15, 1.5);
        }
        let to_screen = |p: Vec2, g: &Graph| mid + (p - g.center) * g.zoom;
        // Zoom about the pointer.
        if let Some(h) = resp.hover_pos() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let before = g.center + (h - mid) / g.zoom;
                g.zoom = (g.zoom * (scroll / 300.0).exp()).clamp(0.15, 4.0);
                g.fit = false;
                g.center = before - (h - mid) / g.zoom;
            }
        }
        let degree = |n: i64| edges.iter().filter(|e| e.0 == n || e.1 == n).count();
        let radius = |n: i64| (5.0 + (degree(n) as f32).sqrt() * 2.5) * g.zoom.sqrt();
        let hovered = resp.hover_pos().and_then(|h| nodes.iter().copied().filter(|&n| (to_screen(g.layout.pos[&n], &g) - h).length() <= radius(n) + 3.0).last());
        // Where the press was: a quick hand is already past the page when
        // the drag starts.
        if resp.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
        {
            let hit = nodes.iter().copied().filter(|&n| (to_screen(g.layout.pos[&n], &g) - p).length() <= radius(n) + 3.0).last();
            if hit.is_some() && ui.input(|i| i.modifiers.shift) {
                g.linking = hit;
            } else {
                g.dragging = hit;
            }
            if let Some(n) = g.dragging {
                g.layout.pinned.insert(n);
                g.fit = false;
            }
        }
        if resp.dragged() && g.linking.is_none() {
            let d = resp.drag_delta() / g.zoom;
            match g.dragging {
                Some(n) => {
                    if let Some(p) = g.layout.pos.get_mut(&n) {
                        *p += d;
                    }
                    g.layout.wake();
                }
                None => {
                    g.center -= d;
                    g.fit = false;
                }
            }
        }
        if resp.drag_stopped()
            && let Some(from) = g.linking.take()
            && let Some(to) = hovered.filter(|&t| t != from)
            && let Some(at) = ui.ctx().pointer_latest_pos()
        {
            g.pending = Some((from, to, at));
        }
        if resp.drag_stopped() {
            if let Some(n) = g.dragging.take() {
                g.layout.pinned.remove(&n);
            }
        }
        if resp.clicked()
            && let Some(n) = hovered
        {
            open = Some(n);
        }
        // A page dragged from the list joins the graph where it is let go.
        if let Some(Naming::Page(p)) = self.drag
            && ui.rect_contains_pointer(rect)
        {
            painter.rect_stroke(rect.shrink(1.0), 0.0, ui.visuals().selection.stroke, egui::StrokeKind::Inside);
            if ui.input(|i| i.pointer.any_released())
                && let Some(at) = ui.ctx().pointer_latest_pos()
            {
                self.drag = None;
                g.added.insert(p);
                g.layout.pos.insert(p, g.center + (at - mid) / g.zoom);
                g.layout.wake();
                g.fit = false;
                ui.ctx().request_repaint();
            }
        }
        if resp.secondary_clicked() {
            g.menu = hovered;
        }
        if let Some(n) = g.menu
            && g.added.contains(&n)
        {
            resp.context_menu(|ui| {
                if ui.button(tr("nb.graph_remove")).clicked() {
                    g.added.remove(&n);
                    g.layout.wake();
                    g.menu = None;
                    ui.close();
                }
            });
        }
        let near: HashSet<i64> = hovered.map(|h| edges.iter().filter_map(|&(a, b, _)| if a == h { Some(b) } else if b == h { Some(a) } else { None }).chain([h]).collect()).unwrap_or_default();
        // Links, with an arrow at the page linked to.
        for &(a, b, rel) in &edges {
            let (pa, pb) = (to_screen(g.layout.pos[&a], &g), to_screen(g.layout.pos[&b], &g));
            let lit = hovered.is_some_and(|h| h == a || h == b);
            let mut c = rel_color(rel);
            if hovered.is_some() && !lit {
                c = c.gamma_multiply(0.25);
            }
            let w = if rel == Rel::Link { 1.2 } else { 2.0 };
            let dir = (pb - pa).normalized();
            let tip = pb - dir * (radius(b) + 2.0);
            painter.line_segment([pa, tip], Stroke::new(w, c));
            let side = Vec2::new(-dir.y, dir.x) * 4.0;
            painter.add(egui::Shape::convex_polygon(vec![tip, tip - dir * 8.0 + side, tip - dir * 8.0 - side], c, Stroke::NONE));
        }
        // Pages.
        let text = ui.visuals().text_color();
        for &n in &nodes {
            let p = to_screen(g.layout.pos[&n], &g);
            if !rect.expand(40.0).contains(p) {
                continue;
            }
            let color = self.nb.page(n).and_then(|p| self.nb.section(p.section)).map_or(Color32::GRAY, |s| section_color(s.color));
            let dim = hovered.is_some() && !near.contains(&n);
            let r = radius(n);
            painter.circle_filled(p, r, if dim { color.gamma_multiply(0.3) } else { color });
            if Some(n) == self.page {
                painter.circle_stroke(p, r + 3.0, Stroke::new(2.0, theme::NOTE_ACCENT));
            }
            if g.zoom >= 0.7 || near.contains(&n) || Some(n) == self.page {
                let c = if dim { text.gamma_multiply(0.3) } else { text };
                let size = if Some(n) == hovered { 14.0 } else { 12.0 };
                painter.text(p + Vec2::new(0.0, r + 3.0), egui::Align2::CENTER_TOP, self.title_of(n), egui::FontId::proportional(size), c);
            }
        }
        if hovered.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if nodes.is_empty() {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, tr("nb.graph_empty"), egui::FontId::proportional(14.0), ui.visuals().weak_text_color());
        }
        // The link being drawn.
        if let Some(from) = g.linking
            && let Some(at) = ui.ctx().pointer_latest_pos()
        {
            let a = to_screen(g.layout.pos[&from], &g);
            let end = hovered.filter(|&t| t != from).map_or(at, |t| to_screen(g.layout.pos[&t], &g));
            painter.line_segment([a, end], Stroke::new(2.0, theme::NOTE_ACCENT));
        }
        // Its meaning, once dropped on a page.
        if let Some((from, to, at)) = g.pending {
            let mut chosen = None;
            let area = egui::Area::new(Id::new(("graph_rel", self.uid))).order(egui::Order::Foreground).fixed_pos(at + Vec2::new(8.0, 8.0)).show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(format!("{} > {}", self.title_of(from), self.title_of(to))).strong());
                    for rel in Rel::ALL {
                        if ui.button(rel_name(rel)).clicked() {
                            chosen = Some(rel);
                        }
                    }
                });
            });
            let away = ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)) || i.key_pressed(egui::Key::Escape));
            if let Some(rel) = chosen {
                g.pending = None;
                self.connect(from, to, rel);
                g.layout.wake();
            } else if away {
                g.pending = None;
            }
        }
        if g.linking.is_none() && g.pending.is_none() {
            painter.text(pos2(rect.max.x - 12.0, rect.max.y - 12.0), egui::Align2::RIGHT_BOTTOM, tr("nb.graph_link_tip"), egui::FontId::proportional(12.0), ui.visuals().weak_text_color());
        }
        // Which color is which link.
        let used: Vec<Rel> = Rel::ALL.into_iter().filter(|r| edges.iter().any(|e| e.2 == *r)).collect();
        let mut y = rect.max.y - 12.0 - used.len() as f32 * 18.0;
        for rel in used {
            let at = pos2(rect.min.x + 16.0, y);
            painter.line_segment([at, at + Vec2::new(18.0, 0.0)], Stroke::new(2.0, rel_color(rel)));
            painter.text(at + Vec2::new(26.0, 0.0), egui::Align2::LEFT_CENTER, rel_name(rel), egui::FontId::proportional(12.0), ui.visuals().weak_text_color());
            y += 18.0;
        }
        if close {
            return;
        }
        self.graph = Some(g);
        if let Some(n) = open {
            self.graph = None;
            self.open_page(n);
            self.grid = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_pages_end_up_closer_than_the_rest() {
        let nodes = [1, 2, 3, 4];
        let edges = [(1, 2, Rel::Link), (3, 4, Rel::Supports)];
        let mut l = Layout::default();
        l.ensure(&nodes, &edges);
        for _ in 0..600 {
            l.step(&nodes, &edges);
        }
        let d = |a: i64, b: i64| (l.pos[&a] - l.pos[&b]).length();
        assert!(d(1, 2) < d(1, 3) && d(3, 4) < d(2, 4), "{:?}", l.pos);
        // It comes to rest.
        assert!(l.step(&nodes, &edges) < 1.0);
    }
}
