//! Alignment, measured on what the whole window draws (the texts it
//! paints) and on the document the view edits:
//! - **ruler**: the horizontal ruler's zero stands where the text of the
//!   caret's paragraph starts, in print layout and in draft, at several
//!   zooms (the numbers it draws, one unit apart, give its zero);
//! - **vruler**: the vertical ruler's zero stands at the top of the text
//!   (the top margin) in print layout;
//! - **line-numbers**: in the code view, each line number is drawn on the
//!   row its line starts at;
//! - **borders**: All borders or No border on an inner cell of a table
//!   reaches its four sides, and the facing sides of the cells around it
//!   (which draw the same lines).
//!
//! The unit tests check the same with the views' own geometry
//! (`rulers_ui`, `md_code`, `apex-doc/tests/tables.rs`); here the window
//! as a whole is looked at.

use apex_doc::{Border, Document, Paragraph, Pos, Selection};
use eframe::egui::{Pos2, Rect};

use super::common::{self, Sink};
use super::model::Qv;
use super::scenes::{self, Text, WIDE};
use crate::app::View;
use crate::app::audit::{self, Rig};
use crate::i18n::tr;

/// How far apart two things that should meet may be drawn, in points.
const NEAR: f32 = 1.5;

fn zooms() -> &'static [f32] {
    if common::full() { &[0.75, 1.0, 1.6] } else { &[1.0, 1.6] }
}

fn plain_doc() -> Document {
    Document::from_paragraphs((0..60).map(|i| Paragraph::new(format!("Paragraph {i} with some words to fill the line"))).collect())
}

fn doc_rig(doc: Document, zoom: f32) -> Rig {
    let mut rig = Rig::open_sized(false, WIDE, |app| app.push_tab(View::Doc(Box::new(crate::doc_view::DocView::new(doc))), None, None));
    if let Some(View::Doc(v)) = rig.app.tabs.last_mut().map(|t| &mut t.view) {
        v.zoom = zoom;
    }
    rig.settle();
    rig.settle();
    rig
}

/// Numbers drawn alone (a ruler's), with their centres.
fn numbers(texts: &[Text]) -> Vec<(i32, Pos2)> {
    texts
        .iter()
        .filter(|t| t.clip.contains(t.rect.center()))
        .filter_map(|t| t.text.trim().parse::<i32>().ok().filter(|_| t.text.trim().len() <= 3).map(|n| (n, t.rect.center())))
        .collect()
}

/// A ruler's zero along one axis (`across`: the other coordinate is the
/// same for all its numbers): from its longest run of numbers 1, 2…
/// a unit apart. None when no ruler is drawn.
fn ruler_zero(nums: &[(i32, Pos2)], horizontal: bool) -> Option<(f32, f32)> {
    let along = |p: &Pos2| if horizontal { p.x } else { p.y };
    let across = |p: &Pos2| if horizontal { p.y } else { p.x };
    let mut best: Option<(usize, f32, f32)> = None;
    // The lines of numbers: those with the same other coordinate.
    let mut lines: Vec<f32> = nums.iter().map(|(_, p)| across(p).round()).collect();
    lines.sort_by(f32::total_cmp);
    lines.dedup();
    for line in lines {
        let mut on: Vec<(i32, f32)> = nums.iter().filter(|(_, p)| (across(p) - line).abs() < 1.0).map(|(n, p)| (*n, along(p))).collect();
        on.sort_by(|a, b| a.1.total_cmp(&b.1));
        // Runs of numbers going up by one.
        let mut start = 0;
        for i in 1..=on.len() {
            if i < on.len() && on[i].0 == on[i - 1].0 + 1 {
                continue;
            }
            let run = &on[start..i];
            if run.len() >= 2 && best.is_none_or(|b| run.len() > b.0) {
                let unit = (run[run.len() - 1].1 - run[0].1) / (run.len() - 1) as f32;
                best = Some((run.len(), run[0].1 - run[0].0 as f32 * unit, unit));
            }
            start = i;
        }
    }
    best.map(|b| (b.1, b.2))
}

/// Where paragraph `text`'s first row starts: its left and its top.
fn text_start(texts: &[Text], text: &str) -> Option<Pos2> {
    texts.iter().find(|t| t.text == text).map(|t| t.rect.min)
}

fn rulers(sink: &mut Sink) {
    for &zoom in zooms() {
        for draft in [false, true] {
            let mode = if draft { "draft" } else { "print layout" };
            audit::watch(Some(format!("align ruler {mode} {zoom}")));
            let mut rig = doc_rig(plain_doc(), zoom);
            if draft {
                rig.app.toolstrip_tab[rig.app.toolstrip_kind()] = "view";
                rig.settle();
                match scenes::named(&rig, tr("view.draft")) {
                    Some(c) => {
                        rig.reach(&c);
                    }
                    None => {
                        sink.note(format!("no Draft button at zoom {zoom}"));
                        continue;
                    }
                }
                rig.frame(vec![eframe::egui::Event::PointerGone]);
                rig.settle();
            }
            let texts = scenes::texts(&rig);
            let nums = numbers(&texts);
            let first = text_start(&texts, "Paragraph 0 with some words to fill the line");
            let repro = format!("a document of plain paragraphs, {mode}, zoom {zoom}, caret at the start");
            sink.check();
            match (ruler_zero(&nums, true), first) {
                (Some((zero, _)), Some(p)) if (zero - p.x).abs() > NEAR => {
                    sink.fail(common::id(&["ruler", mode, &zoom.to_string()]), format!("[ruler] {mode}, zoom {zoom}: the ruler's zero is at x {zero:.1}, the text starts at x {:.1} ({:+.1} pt). Repro: {repro}", p.x, zero - p.x));
                }
                (None, _) => sink.fail(common::id(&["ruler", mode, &zoom.to_string(), "none"]), format!("[ruler] {mode}, zoom {zoom}: no ruler numbers found. Repro: {repro}")),
                (_, None) => sink.note(format!("ruler {mode} {zoom}: the first paragraph isn't drawn")),
                _ => {}
            }
            if draft {
                continue;
            }
            // The vertical ruler: the numbers along the window's left edge.
            let left: Vec<(i32, Pos2)> = nums.iter().copied().filter(|(_, p)| p.x < 40.0).collect();
            sink.check();
            match (ruler_zero(&left, false), first) {
                (Some((zero, _)), Some(p)) if (zero - p.y).abs() > NEAR => {
                    sink.fail(common::id(&["vruler", &zoom.to_string()]), format!("[vruler] print layout, zoom {zoom}: the vertical ruler's zero is at y {zero:.1}, the text's top at y {:.1} ({:+.1} pt). Repro: {repro}", p.y, zero - p.y));
                }
                (None, _) => sink.fail(common::id(&["vruler", &zoom.to_string(), "none"]), format!("[vruler] print layout, zoom {zoom}: no vertical ruler numbers found. Repro: {repro}")),
                _ => {}
            }
        }
    }
}

fn line_numbers(sink: &mut Sink) {
    audit::watch(Some("align line numbers".into()));
    let rig = Rig::open_sized(false, WIDE, |app| Qv::Code.setup(app, 0));
    // The code: the drawn text with the most lines.
    let mut code: Option<(Pos2, std::sync::Arc<eframe::egui::Galley>)> = None;
    let mut digits: Vec<(usize, f32, f32)> = Vec::new();
    let mut look = |s: &eframe::egui::Shape| {
        if let eframe::egui::Shape::Text(t) = s {
            let text = t.galley.text();
            if !text.trim().is_empty() && text.chars().all(|c| c.is_ascii_digit() || c.is_whitespace()) {
                for row in &t.galley.rows {
                    if let Ok(n) = row.text().trim().parse::<usize>() {
                        digits.push((n, t.pos.x + row.rect().max.x, t.pos.y + row.rect().min.y));
                    }
                }
            } else if code.as_ref().is_none_or(|c| c.1.rows.len() < t.galley.rows.len()) {
                code = Some((t.pos, t.galley.clone()));
            }
        }
    };
    fn each(s: &eframe::egui::Shape, f: &mut dyn FnMut(&eframe::egui::Shape)) {
        if let eframe::egui::Shape::Vec(v) = s {
            v.iter().for_each(|s| each(s, f));
        } else {
            f(s);
        }
    }
    for s in &rig.shapes {
        each(&s.shape, &mut look);
    }
    let repro = "the Code sample (View > Code)";
    let Some((pos, galley)) = code else {
        sink.check();
        sink.fail(common::id(&["line-numbers", "no code"]), format!("[line-numbers] the code isn't drawn. Repro: {repro}"));
        return;
    };
    // The rows each line starts at.
    let mut starts = vec![pos.y + galley.rows.first().map_or(0.0, |r| r.rect().min.y)];
    for (i, r) in galley.rows.iter().enumerate() {
        if r.ends_with_newline && i + 1 < galley.rows.len() {
            starts.push(pos.y + galley.rows[i + 1].rect().min.y);
        }
    }
    let clip: Rect = rig.shapes.iter().map(|s| s.clip_rect).fold(Rect::NOTHING, |a, b| a.union(b));
    // The gutter's numbers: ending just left of the code, from its top down
    // (not the toolstrip's font size or the status bar's).
    let top = starts[0] - 2.0;
    digits.retain(|(_, right, y)| *right <= pos.x + 2.0 && *right >= pos.x - 80.0 && *y >= top);
    let mut bad = 0;
    for (n, _, y) in &digits {
        let Some(want) = starts.get(n.wrapping_sub(1)) else { continue };
        if !clip.contains(Pos2::new(clip.center().x, *y)) {
            continue;
        }
        sink.check();
        if (y - want).abs() > 0.5 {
            bad += 1;
            if bad <= 5 {
                sink.fail(common::id(&["line-numbers", &n.to_string()]), format!("[line-numbers] line {n}: its number is drawn at y {y:.1}, its row at y {want:.1}. Repro: {repro}"));
            }
        }
    }
    if digits.is_empty() {
        sink.check();
        sink.fail(common::id(&["line-numbers", "none"]), format!("[line-numbers] no line numbers drawn beside the code. Repro: {repro}"));
    }
}

/// A document with a 3×3 table; the paragraph of its middle cell.
fn table_doc() -> (Document, usize, usize) {
    let mut doc = Document::from_paragraphs(vec![Paragraph::new("Before the table")]);
    doc.insert_table(Selection::caret(Pos::new(0, 16)), 3, 3, 360.0);
    let first = (0..doc.paragraphs.len()).find(|&i| doc.table_map(i).is_some()).unwrap_or(0);
    let middle = doc.table_map(first).map_or(first, |m| m.cell(1, 1).0);
    (doc, first, middle)
}

fn borders(sink: &mut Sink) {
    let presets = [("All", "table.border_all", true), ("None", "table.border_none", false)];
    // Each preset on a fresh table, and No border after All borders.
    let runs: [&[usize]; 3] = [&[0], &[1], &[0, 1]];
    for run in runs {
        let names: Vec<&str> = run.iter().map(|i| presets[*i].0).collect();
        let what = names.join(" then ");
        audit::watch(Some(format!("align borders {what}")));
        let (doc, first, middle) = table_doc();
        // A window wide enough that the Borders menu isn't folded into a
        // group's drop-down (whose popup a submenu closes).
        let mut rig = Rig::open_sized(false, eframe::egui::vec2(2400.0, 900.0), |app| {
            let mut v = crate::doc_view::DocView::new(doc);
            v.select_paras(middle, middle);
            app.push_tab(View::Doc(Box::new(v)), None, None);
        });
        let repro = format!("a 3×3 table, the middle cell selected, Table Design > Borders > {what}");
        let mut missed = None;
        for i in run {
            rig.app.toolstrip_tab[rig.app.toolstrip_kind()] = "table";
            rig.settle();
            let r = scenes::click(&mut rig, tr("table.borders"), "toolstrip.g.table_styles").and_then(|()| scenes::click(&mut rig, tr(presets[*i].1), ""));
            rig.frame(vec![eframe::egui::Event::PointerGone]);
            rig.settle();
            if let Err(e) = r {
                missed = Some(e);
            }
        }
        if let Some(e) = missed {
            sink.note(format!("borders {what}: not reached ({e})"));
            continue;
        }
        let Some(View::Doc(v)) = rig.app.tabs.last().map(|t| &t.view) else { continue };
        let Some(m) = v.doc.table_map(first) else { continue };
        let on = presets[*run.last().unwrap_or(&0)].2;
        let side = |r: usize, c: usize, s: usize| {
            let b = m.cell_props(&v.doc.paragraphs, r, c).map(|p| p.borders).unwrap_or_default();
            [b.top, b.bottom, b.left, b.right][s]
        };
        let drawn = |b: Option<Border>| b.is_some_and(|b| b != Border::NONE);
        // The cell's side, and the neighbor's facing one.
        let sides = [("top", (1, 1, 0), (0, 1, 1)), ("bottom", (1, 1, 1), (2, 1, 0)), ("left", (1, 1, 2), (1, 0, 3)), ("right", (1, 1, 3), (1, 2, 2))];
        for (name, own, facing) in sides {
            sink.check();
            let (a, b) = (side(own.0, own.1, own.2), side(facing.0, facing.1, facing.2));
            let ok = if on { drawn(a) && b != Some(Border::NONE) } else { !drawn(a) && !drawn(b) };
            if !ok {
                sink.fail(common::id(&["borders", &what, name]), format!("[borders] {what} on the middle cell: its {name} line {} (cell: {a:?}, the cell beyond: {b:?}). Repro: {repro}", if on { "isn't drawn" } else { "is still drawn" }));
            }
        }
    }
}

fn run_job(job: &str, sink: &mut Sink) {
    audit::panic_hook();
    let r = audit::quietly(|| match job {
        "rulers" => rulers(sink),
        "code" => line_numbers(sink),
        "borders" => borders(sink),
        _ => {}
    });
    if let Err(e) = r {
        sink.check();
        sink.fail(common::id(&[job, "panic"]), format!("[panic] {job}: {}", audit::panic_text(&e)));
    }
    audit::watch(None);
}

#[test]
#[ignore]
fn qa_align() {
    let jobs = ["rulers", "code", "borders"].map(String::from);
    common::run("align", "app::qa::align::qa_align", &jobs, run_job);
}
