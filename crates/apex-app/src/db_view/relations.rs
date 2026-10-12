//! The relations diagram: the tables as boxes with their fields, moved by
//! dragging their title; a field dragged onto a field of another table
//! makes a relation (from the table it points to, usually its key, to the
//! table whose rows point), asking for referential integrity and cascades.
//! Clicking a line selects its relation, which can then be changed or
//! removed.

use apex_db::objects::{Kind, Record};
use apex_db::sqlite::Db;
use apex_db::Relation;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, vec2};

use super::{ConfirmAction, DbView};
use crate::i18n::tr;

const BOX_W: f32 = 170.0;
const ROW_H: f32 = 18.0;

#[derive(Default)]
pub struct Diagram {
    /// Table, place, fields, key fields.
    pub tables: Vec<(String, Pos2, Vec<String>, Vec<String>)>,
    pub rels: Vec<Relation>,
    drag_field: Option<(usize, usize)>,
    drag_table: Option<usize>,
    pub selected: Option<usize>,
    /// A relation being made or changed, and whether it is new.
    pub pending: Option<(Relation, bool)>,
    moved: bool,
}

impl Diagram {
    pub fn load(db: &Db) -> Diagram {
        let mut d = Diagram::default();
        d.reload(db);
        d
    }

    pub fn reload(&mut self, db: &Db) {
        let places: Vec<Record> = db.load_object(Kind::Layout, "relations").map(|t| apex_db::objects::read(&t)).unwrap_or_default();
        let place = |n: &str| places.iter().find(|r| r.section == "place" && r.get("table") == n).map(|r| Pos2::new(r.num("x", 0.0), r.num("y", 0.0)));
        // Tables without a place go under the shortest of four columns.
        let mut bottoms = [20.0f32; 4];
        self.tables = db
            .table_names()
            .into_iter()
            .map(|n| {
                let d = db.design(&n).ok();
                let fields: Vec<String> = d.as_ref().map(|d| d.fields.iter().map(|f| f.name.clone()).collect()).unwrap_or_default();
                let keys = d.as_ref().map(|d| d.fields.iter().filter(|f| f.pk).map(|f| f.name.clone()).collect()).unwrap_or_default();
                let p = place(&n).unwrap_or_else(|| {
                    let c = (0..4).fold(0, |b, c| if bottoms[c] < bottoms[b] { c } else { b });
                    let p = Pos2::new(20.0 + c as f32 * (BOX_W + 70.0), bottoms[c]);
                    bottoms[c] += 28.0 + fields.len() as f32 * ROW_H + 40.0;
                    p
                });
                (n, p, fields, keys)
            })
            .collect();
        self.rels = db.relations();
        self.selected = None;
    }

    /// Keeps the tables' places in the file (with the drawn relations).
    fn save_places(&self, v: &mut DbView) {
        let mut recs: Vec<Record> = v.db.load_object(Kind::Layout, "relations").map(|t| apex_db::objects::read(&t)).unwrap_or_else(|| vec![Record::default()]);
        recs.retain(|r| r.section != "place");
        for (n, p, _, _) in &self.tables {
            let mut r = Record::new("place");
            r.set("table", n).set("x", p.x.round()).set("y", p.y.round());
            recs.push(r);
        }
        // Places aren't worth an undo step.
        if let Err(e) = v.db.save_object(Kind::Layout, "relations", &apex_db::objects::write(&recs)) {
            v.error(e);
        }
    }

    fn field_pos(&self, origin: Pos2, t: usize, f: usize, right: bool) -> Pos2 {
        let (_, p, _, _) = &self.tables[t];
        origin + p.to_vec2() + vec2(if right { BOX_W } else { 0.0 }, 24.0 + f as f32 * ROW_H + ROW_H / 2.0)
    }

    fn find(&self, table: &str, field: &str) -> Option<(usize, usize)> {
        let t = self.tables.iter().position(|x| x.0.eq_ignore_ascii_case(table))?;
        let f = self.tables[t].2.iter().position(|x| x.eq_ignore_ascii_case(field))?;
        Some((t, f))
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, v: &mut DbView) {
        ui.horizontal(|ui| {
            ui.label(tr("db.relations_tip"));
            if let Some(i) = self.selected {
                if ui.button(tr("db.edit_relation")).clicked() {
                    self.pending = Some((self.rels[i].clone(), false));
                }
                if ui.button(tr("db.remove_relation")).clicked() {
                    let r = self.rels[i].clone();
                    v.confirmed(ConfirmAction::Relation(r, false));
                    self.reload(&v.db);
                }
            }
        });
        self.pending_ui(ui.ctx(), v);
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let size = self.tables.iter().fold(vec2(600.0, 400.0), |s, (_, p, f, _)| s.max(vec2(p.x + BOX_W + 40.0, p.y + 30.0 + f.len() as f32 * ROW_H + 40.0)));
            let (resp, painter) = ui.allocate_painter(size, Sense::click());
            let origin = resp.rect.min;
            let vis = ui.visuals().clone();
            // Lines first, under the boxes.
            let mut hit = None;
            let pointer = ui.input(|i| i.pointer.interact_pos());
            for (i, r) in self.rels.iter().enumerate() {
                let (Some(a), Some(b)) = (r.ref_columns.first().and_then(|c| self.find(&r.ref_table, c)), r.columns.first().and_then(|c| self.find(&r.table, c))) else { continue };
                let right = self.tables[a.0].1.x < self.tables[b.0].1.x;
                let pa = self.field_pos(origin, a.0, a.1, right);
                let pb = self.field_pos(origin, b.0, b.1, !right);
                let color = if self.selected == Some(i) { vis.selection.stroke.color } else if r.enforce { vis.strong_text_color() } else { vis.weak_text_color() };
                let width = if self.selected == Some(i) { 3.0 } else { 1.5 };
                painter.line_segment([pa, pb], Stroke::new(width, color));
                // "1" at the table pointed to, "∞" at the rows pointing (enforced).
                if r.enforce {
                    let d = (pb - pa).normalized() * 12.0;
                    painter.text(pa + d + vec2(0.0, -8.0), egui::Align2::CENTER_CENTER, "1", egui::FontId::proportional(11.0), color);
                    painter.text(pb - d + vec2(0.0, -8.0), egui::Align2::CENTER_CENTER, "∞", egui::FontId::proportional(12.0), color);
                }
                if let Some(p) = pointer {
                    if dist_to_segment(p, pa, pb) < 5.0 {
                        hit = Some(i);
                    }
                }
            }
            if resp.clicked() {
                self.selected = hit;
            }
            if resp.double_clicked() {
                if let Some(i) = hit {
                    self.pending = Some((self.rels[i].clone(), false));
                }
            }
            let mut drop_on = None;
            for t in 0..self.tables.len() {
                let (name, p, fields, keys) = self.tables[t].clone();
                let rect = Rect::from_min_size(origin + p.to_vec2(), vec2(BOX_W, 24.0 + fields.len() as f32 * ROW_H + 4.0));
                painter.rect(rect, 4.0, vis.extreme_bg_color, Stroke::new(1.0, vis.widgets.inactive.bg_stroke.color), egui::StrokeKind::Inside);
                let title = Rect::from_min_size(rect.min, vec2(BOX_W, 22.0));
                painter.rect_filled(title, 4.0, vis.selection.bg_fill);
                painter.text(title.left_center() + vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, &name, egui::FontId::proportional(13.0), vis.strong_text_color());
                let tr_ = ui.interact(title, ui.id().with(("db_rel_title", t)), Sense::drag());
                if tr_.drag_started() {
                    self.drag_table = Some(t);
                }
                if tr_.dragged() && self.drag_table == Some(t) {
                    let p = &mut self.tables[t].1;
                    *p = (*p + tr_.drag_delta()).max(Pos2::ZERO);
                    self.moved = true;
                }
                for (f, field) in fields.iter().enumerate() {
                    let fr = Rect::from_min_size(rect.min + vec2(0.0, 24.0 + f as f32 * ROW_H), vec2(BOX_W, ROW_H));
                    let key = keys.contains(field);
                    let label = if key { format!("🔑 {field}") } else { field.clone() };
                    painter.text(fr.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(12.0), vis.text_color());
                    let r = ui.interact(fr, ui.id().with(("db_rel_field", t, f)), Sense::drag());
                    if r.drag_started() {
                        self.drag_field = Some((t, f));
                    }
                    if r.hovered() && self.drag_field.is_some_and(|d| d.0 != t) {
                        painter.rect_stroke(fr, 2.0, Stroke::new(1.5, vis.selection.stroke.color), egui::StrokeKind::Inside);
                    }
                    if self.drag_field.is_some() && ui.input(|i| i.pointer.any_released()) && pointer.is_some_and(|p| fr.contains(p)) {
                        drop_on = Some((t, f));
                    }
                }
            }
            if let (Some((st, sf)), Some(p)) = (self.drag_field, pointer) {
                let a = self.field_pos(origin, st, sf, true);
                painter.line_segment([a, p], Stroke::new(1.5, Color32::GRAY));
            }
            if ui.input(|i| i.pointer.any_released()) {
                if let (Some(from), Some(to)) = (self.drag_field, drop_on) {
                    if from.0 != to.0 {
                        self.propose(from, to);
                    }
                }
                self.drag_field = None;
                if self.drag_table.take().is_some() && self.moved {
                    self.moved = false;
                    self.save_places(v);
                }
            }
        });
    }

    /// A new relation from field `from` (the table pointed to) to `to`.
    pub fn propose(&mut self, from: (usize, usize), to: (usize, usize)) {
        let (pt, pf) = (&self.tables[from.0].0, &self.tables[from.0].2[from.1]);
        let (ct, cf) = (&self.tables[to.0].0, &self.tables[to.0].2[to.1]);
        let r = Relation { name: format!("{pt}_{ct}"), table: ct.clone(), columns: vec![cf.clone()], ref_table: pt.clone(), ref_columns: vec![pf.clone()], enforce: true, ..Default::default() };
        self.pending = Some((r, true));
    }

    fn pending_ui(&mut self, ctx: &egui::Context, v: &mut DbView) {
        let Some((r, new)) = &mut self.pending else { return };
        let mut done = None;
        let mut open = true;
        egui::Window::new(tr("db.relation")).collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            ui.label(format!("{}.{} → {}.{}", r.table, r.columns.join(", "), r.ref_table, r.ref_columns.join(", ")));
            ui.checkbox(&mut r.enforce, tr("db.enforce"));
            ui.add_enabled_ui(r.enforce, |ui| {
                ui.checkbox(&mut r.cascade_update, tr("db.cascade_update"));
                ui.checkbox(&mut r.cascade_delete, tr("db.cascade_delete"));
            });
            if ui.button(tr("dialog.ok")).clicked() {
                done = Some((r.clone(), *new));
            }
        });
        if !open {
            self.pending = None;
        }
        let Some((r, new)) = done else { return };
        self.pending = None;
        if !new {
            // Changed: the old one goes, the new one comes, as one step.
            let old = self.selected.and_then(|i| self.rels.get(i).cloned());
            if let Some(old) = old {
                let r2 = r.clone();
                v.with_snapshot(tr("db.undo.relation"), move |db| {
                    db.remove_relation(&old)?;
                    db.add_relation(&r2)
                });
            }
        } else {
            v.confirmed(ConfirmAction::Relation(r, true));
        }
        self.reload(&v.db);
    }
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}
