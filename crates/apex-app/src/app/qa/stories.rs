//! Stories: tasks done as a person does them (clicks on what shows,
//! drags over the text seen, keys and typing), each followed by a look at
//! the result as that person would: what was made and where (the model),
//! and where it shows (the picture: a selection over the ink of its text,
//! a strike on the line struck, an arrow with a head, the view still
//! where it was). A story fails with what a person would report.

use eframe::egui::{Key, Modifiers, Pos2, Rect, pos2, vec2};

use apex_io::pdf::edit::annots::{Annot, Kind};

use super::super::View;
use super::behave::{self, changed, changed_bands, click_at, content, drag, ink_lines, key, picture};
use super::common::{self, Sink};
use super::model::Qv;
use crate::app::audit::{self, Control, Rig};
use crate::i18n::tr;

type Res = Result<(), String>;

/// A window with a view open, and what a story does in it.
pub(super) struct T {
    pub rig: Rig,
    pub id: u64,
}

/// A control's name: an i18n key ("pdf.tool.arrow") or the name itself.
fn label(name: &'static str) -> String {
    if name.contains('.') && !name.contains(' ') { tr(name).to_string() } else { name.to_string() }
}

impl T {
    fn new(qv: Qv, n: u64) -> T {
        let rig = behave::open(qv, n);
        let id = rig.app.tabs[rig.app.active].id;
        T { rig, id }
    }

    fn find(&self, name: &'static str) -> Option<Control> {
        self.find_in(name, false)
    }

    /// The control named so; of two with that name, the tab (the one
    /// above) or the other.
    fn find_in(&self, name: &'static str, tab: bool) -> Option<Control> {
        let l = label(name);
        let mut all: Vec<&Control> = self.rig.controls.iter().filter(|c| c.key.1 == l).collect();
        // Tabs show above the buttons of the toolstrip.
        all.sort_by(|a, b| if tab { a.rect.min.y.total_cmp(&b.rect.min.y) } else { b.rect.min.y.total_cmp(&a.rect.min.y) });
        all.iter().find(|c| c.enabled).or(all.first()).map(|c| (*c).clone())
    }

    /// Clicks the toolstrip tab named so.
    fn tab(&mut self, name: &'static str) -> Res {
        let c = self.find_in(name, true).ok_or_else(|| format!("there is no tab \"{}\"", label(name)))?;
        self.rig.reach(&c);
        Ok(())
    }

    /// Clicks the control named so (an i18n key or a name).
    fn click(&mut self, name: &'static str) -> Res {
        let c = self.find(name).ok_or_else(|| format!("there is no \"{}\" to click", label(name)))?;
        if !c.enabled {
            return Err(format!("\"{}\" is disabled", label(name)));
        }
        self.rig.reach(&c);
        Ok(())
    }

    fn area(&self) -> Rect {
        content(&self.rig)
    }

    fn view(&mut self) -> &mut View {
        let id = self.id;
        &mut self.rig.app.tabs.iter_mut().find(|t| t.id == id).expect("the tab").view
    }

    fn pdf(&mut self) -> &mut crate::pdf_view::PdfView {
        match self.view() {
            View::Pdf(v) => v,
            _ => panic!("not a PDF"),
        }
    }

    fn doc(&mut self) -> &mut crate::doc_view::DocView {
        match self.view() {
            View::Doc(v) => v,
            _ => panic!("not a document"),
        }
    }

    fn sheet(&mut self) -> &mut crate::sheet_view::SheetView {
        match self.view() {
            View::Sheet(v) => v,
            _ => panic!("not a sheet"),
        }
    }

    fn slides(&mut self) -> &mut crate::slides_view::SlidesView {
        match self.view() {
            View::Slides(v) => v,
            _ => panic!("not slides"),
        }
    }

    /// The objects of the slide shown.
    fn objects(&mut self) -> Vec<apex_slides::object::Object> {
        let v = self.slides();
        v.pres.deck.slides[v.cur].objects.clone()
    }

    fn key(&mut self, k: Key, m: Modifiers) {
        key(&mut self.rig, k, m);
    }

    fn type_text(&mut self, s: &str) {
        behave::type_text(&mut self.rig, s);
    }

    /// The annotations of the page shown, of `kind`.
    fn annots(&mut self, kind: Kind) -> Vec<Annot> {
        let v = self.pdf();
        let page = v.cur;
        v.doc.annots(page).into_iter().filter(|a| a.kind == kind && a.reply_to.is_none()).collect()
    }

    /// The lines of text that show on the page, wide ones first found top
    /// down.
    fn text_lines(&self) -> Vec<Rect> {
        let img = picture(&self.rig);
        let page = behave::paper(&img, self.area()).unwrap_or(self.area()).shrink(2.0);
        ink_lines(&img, page).into_iter().filter(|l| l.width() > 250.0 && (5.0..40.0).contains(&l.height())).collect()
    }

    fn line(&self, n: usize) -> Result<Rect, String> {
        self.text_lines().get(n).copied().ok_or_else(|| format!("no line {} of text shows on the page", n + 1))
    }

    /// Drags over line `l`, from its start to `to` of its width.
    fn select_line(&mut self, l: Rect, to: f32) {
        drag(&mut self.rig, pos2(l.min.x + 1.0, l.center().y), pos2(l.min.x + l.width() * to, l.center().y));
    }

    /// Whether the window keeps moving, left alone.
    fn still(&mut self, after: &str) -> Res {
        match behave::drift(&mut self.rig) {
            Some(d) => Err(format!("after {after}, {d}")),
            None => Ok(()),
        }
    }
}

fn show(r: Rect) -> String {
    format!("{:.0},{:.0}–{:.0},{:.0}", r.min.x, r.min.y, r.max.x, r.max.y)
}

// --- PDF -----------------------------------------------------------------------------------

/// Dragged over, line `l` shows selected over its own ink, one band.
fn selection_on_ink(t: &mut T, l: Rect, selected: fn(&mut T) -> bool) -> Res {
    let p0 = picture(&t.rig);
    let keys = behave::control_keys(&t.rig);
    t.select_line(l, 0.6);
    if !selected(t) {
        return Err(format!("dragging over the line of text at {} selects nothing", show(l)));
    }
    let p1 = picture(&t.rig);
    behave::shot(&p1, "selection");
    let around = l.expand2(vec2(30.0, l.height() * 1.5));
    let over = behave::new_controls(&t.rig, &keys);
    let bands: Vec<Rect> = behave::changed_bands_except(&p0, &p1, around, &over).into_iter().filter(|b| b.height() >= 2.0).collect();
    let [b] = bands[..] else {
        return Err(format!("one line selected shows as {} bands ({}), the text at {}", bands.len(), bands.iter().map(|b| show(*b)).collect::<Vec<_>>().join(" "), show(l)));
    };
    if b.min.y > l.min.y + 2.0 || b.max.y < l.max.y - 2.0 || b.height() > l.height() * 2.5 {
        return Err(format!("the selection shows at {} and its text at {}: off the text", show(b), show(l)));
    }
    let slack = (l.height() * 0.6).max(6.0);
    if (b.min.x - l.min.x).abs() > slack {
        return Err(format!("the selection starts at x {:.0} and the text dragged from at x {:.0}", b.min.x, l.min.x));
    }
    Ok(())
}

/// Text dragged over shows selected over its own ink, one band a line.
fn pdf_selection_on_text(t: &mut T) -> Res {
    let l = t.line(1)?;
    selection_on_ink(t, l, |t| t.pdf().sel.is_some())
}

/// A line selected and struck out shows struck there; struck again, the
/// strike goes.
fn pdf_strike_and_unstrike(t: &mut T) -> Res {
    let l = t.line(2)?;
    let p0 = picture(&t.rig);
    t.select_line(l, 0.98);
    t.tab("pdf.tab_comment")?;
    t.click("pdf.tool.strike")?;
    let n = t.annots(Kind::StrikeOut).len();
    if n != 1 {
        return Err(format!("striking the selected line makes {n} strike outs"));
    }
    let p1 = picture(&t.rig);
    let marks = changed_bands(&p0, &p1, t.area());
    let off: Vec<String> = marks.iter().filter(|b| !l.expand(4.0).contains_rect(**b)).map(|b| show(*b)).collect();
    if !off.is_empty() || !marks.iter().any(|b| b.width() > l.width() * 0.6) {
        return Err(format!("the line selected is at {}, the strike shows at {}", show(l), marks.iter().map(|b| show(*b)).collect::<Vec<_>>().join(" ")));
    }
    t.select_line(l, 0.98);
    t.click("pdf.tool.strike")?;
    let n = t.annots(Kind::StrikeOut).len();
    if n != 0 {
        return Err(format!("striking the struck line again leaves {n} strike outs: the strike can't be taken off"));
    }
    Ok(())
}

/// The highlighter on, text dragged over gets highlighted; its button
/// again lets go: the next drag only selects.
fn pdf_highlighter_lets_go(t: &mut T) -> Res {
    let (a, b) = (t.line(1)?, t.line(3)?);
    t.tab("pdf.tab_comment")?;
    t.click("pdf.tool.highlight")?;
    t.select_line(a, 0.9);
    let n = t.annots(Kind::Highlight).len();
    if n != 1 {
        return Err(format!("with the highlighter, a drag over a line makes {n} highlights"));
    }
    // Text selected while it is on (Ctrl+A), then its button again.
    key(&mut t.rig, Key::A, Modifiers::COMMAND);
    let before = t.annots(Kind::Highlight).len();
    t.click("pdf.tool.highlight")?;
    let tool = t.pdf().audit_tool();
    if tool != "Select" {
        return Err(format!("with text selected, the highlighter's button clicked again leaves the tool {tool}"));
    }
    if t.annots(Kind::Highlight).len() != before {
        return Err("with text selected, the highlighter's button clicked to let go highlights the text instead".into());
    }
    t.select_line(b, 0.9);
    let n = t.annots(Kind::Highlight).len();
    if n != 1 {
        return Err(format!("the highlighter let go, a drag over another line still highlights ({n} highlights)"));
    }
    Ok(())
}

/// The hand drags the pages: what showed moves with the pointer.
fn pdf_hand_drags(t: &mut T) -> Res {
    let l0 = t.line(4)?;
    t.tab("toolstrip.home")?;
    t.click("pdf.tool_hand")?;
    let at = t.area().center();
    drag(&mut t.rig, at, at - vec2(0.0, 150.0));
    let lines = t.text_lines();
    let moved = lines.iter().filter(|l| (l.width() - l0.width()).abs() < 3.0 && (l.min.x - l0.min.x).abs() < 3.0).map(|l| l0.min.y - l.min.y).min_by(|a, b| (a - 150.0).abs().total_cmp(&(b - 150.0).abs()));
    match moved {
        Some(d) if (110.0..190.0).contains(&d) => Ok(()),
        Some(d) => Err(format!("the hand dragged up 150 points moves the page {d:.0}")),
        None => Err("the hand dragged up 150 points: the line followed isn't found again".into()),
    }
}

/// An arrow drawn has a head at its end.
fn pdf_arrow_has_head(t: &mut T) -> Res {
    t.tab("pdf.tab_comment")?;
    t.click("pdf.tool.arrow")?;
    let a = t.area();
    let (from, to) = (a.lerp_inside(vec2(0.25, 0.8)), a.lerp_inside(vec2(0.6, 0.8)));
    let p0 = picture(&t.rig);
    drag(&mut t.rig, from, to);
    let n = t.annots(Kind::Arrow).len();
    if n != 1 {
        return Err(format!("a drag with the arrow tool makes {n} arrows"));
    }
    let p1 = picture(&t.rig);
    // Tall at its end (the head's two strokes), as thin as a line along it.
    let tall = |p: Pos2| changed_bands(&p0, &p1, Rect::from_center_size(p, vec2(24.0, 60.0))).into_iter().map(|b| b.height()).fold(0.0, f32::max);
    let (end, mid) = (tall(to - vec2(6.0, 0.0)), tall(from.lerp(to, 0.5)));
    if end < (mid * 2.5).max(8.0) {
        return Err(format!("the arrow shows no head: {end:.0} points tall at its end, {mid:.0} along it"));
    }
    Ok(())
}

/// A stamp placed where clicked, with its words, and the view stays.
fn pdf_stamp_in_place(t: &mut T) -> Res {
    t.tab("pdf.tab_comment")?;
    t.click("pdf.tool.stamp")?;
    t.click("pdf.stamp.not_approved")?;
    let at = t.area().center();
    let p0 = picture(&t.rig);
    click_at(&mut t.rig, at, 1);
    let s = t.annots(Kind::Stamp);
    let [st] = &s[..] else { return Err(format!("a click with the stamp makes {} stamps", s.len())) };
    let want = tr("pdf.stamp.not_approved");
    if st.stamp != want {
        return Err(format!("the stamp chosen is \"{want}\", the one placed says \"{}\"", st.stamp));
    }
    let p1 = picture(&t.rig);
    let far = changed(&p0, &p1, t.area(), Some(Rect::from_center_size(at, vec2(500.0, 240.0))));
    if far > 0.2 {
        return Err(format!("placing the stamp changes {:.0}% of the view away from it: the view jumped", far * 100.0));
    }
    t.still("placing a stamp")?;
    let bytes = t.pdf().doc.to_bytes_whole();
    let page = t.pdf().cur;
    let back = apex_io::pdf::edit::PdfDoc::open(bytes, None).map_err(|e| format!("saved with a stamp, it can't be read: {e}"))?;
    let words: Vec<String> = back.annots(page).into_iter().filter(|a| a.kind == Kind::Stamp).map(|a| a.stamp).collect();
    if words != [want.to_string()] {
        return Err(format!("saved and read back, the stamp says {words:?}, not \"{want}\""));
    }
    Ok(())
}

/// The pen draws where dragged, and the view stays.
fn pdf_pen_in_place(t: &mut T) -> Res {
    t.pdf().doc.add_annot(&Annot { page: 0, kind: Kind::Note, rect: [60.0, 60.0, 80.0, 80.0], contents: LONG.repeat(3), author: "Ana".into(), ..Default::default() });
    t.click("pdf.comments")?;
    t.tab("pdf.tab_comment")?;
    t.click("pdf.tool.ink")?;
    let a = t.area();
    let (from, to) = (a.lerp_inside(vec2(0.3, 0.5)), a.lerp_inside(vec2(0.5, 0.6)));
    let p0 = picture(&t.rig);
    drag(&mut t.rig, from, to);
    let n = t.annots(Kind::Ink).len();
    if n != 1 {
        return Err(format!("a drag with the pen makes {n} drawings"));
    }
    let p1 = picture(&t.rig);
    let far = changed(&p0, &p1, a, Some(Rect::from_two_pos(from, to).expand(40.0)));
    if far > 0.2 {
        return Err(format!("drawing with the pen changes {:.0}% of the view away from it: the view jumped", far * 100.0));
    }
    t.still("drawing with the pen")
}

const LONG: &str = "A comment long enough to fill a few lines of the pane, and then some more words. ";

/// The comments pane, with long comments and replies, keeps its width.
fn pdf_comments_keep_width(t: &mut T) -> Res {
    let page = t.pdf().cur;
    let long = LONG.repeat(4);
    let id = t.pdf().doc.add_annot(&Annot { page, kind: Kind::Note, rect: [60.0, 60.0, 80.0, 80.0], contents: long.clone(), author: "Ana".into(), ..Default::default() });
    t.pdf().doc.reply(id, &long, "Bob");
    // More than the pane holds: its scroll bar shows.
    for i in 0..12 {
        t.pdf().doc.add_annot(&Annot { page, kind: Kind::Note, rect: [60.0, 100.0 + 30.0 * i as f32, 80.0, 120.0 + 30.0 * i as f32], contents: LONG.into(), author: "Carla Fernández de la Peña".into(), ..Default::default() });
    }
    t.click("pdf.comments")?;
    // Picked, as when clicked in the list: its reply box shows.
    t.pdf().picked = Some(id);
    let a0 = t.area();
    t.still("opening the comments")?;
    let a1 = t.area();
    if a1.width() < a0.width() - 2.0 || a1.min.x > behave::SCREEN.x * 0.55 {
        return Err(format!("with the comments open, the page goes from {} to {}", show(a0), show(a1)));
    }
    Ok(())
}


// --- documents --------------------------------------------------------------------------------

fn doc_selection_on_text(t: &mut T) -> Res {
    let l = t.line(1)?;
    selection_on_ink(t, l, |t| !t.doc().selected_text().is_empty())
}

/// Typed at the end, the words are there, and shown.
fn doc_type_at_end(t: &mut T) -> Res {
    t.key(Key::End, Modifiers::COMMAND);
    let p0 = picture(&t.rig);
    t.type_text(" Fin del texto.");
    let last = t.doc().doc.paragraphs.last().map(|p| p.text.clone()).unwrap_or_default();
    if !last.ends_with(" Fin del texto.") {
        return Err(format!("typed at the end (Ctrl+End), the last paragraph reads \"{last}\""));
    }
    if changed(&p0, &picture(&t.rig), t.area(), None) < 0.0005 {
        return Err("typed at the end, nothing new shows".into());
    }
    t.key(Key::Z, Modifiers::COMMAND);
    let last = t.doc().doc.paragraphs.last().map(|p| p.text.clone()).unwrap_or_default();
    if last.contains("Fin del texto") {
        return Err(format!("undone (Ctrl+Z), the typing stays: \"{last}\""));
    }
    Ok(())
}

/// A line selected and made bold (Ctrl+B) is bold, and shows bolder.
fn doc_bold_selection(t: &mut T) -> Res {
    let l = t.line(1)?;
    t.select_line(l, 0.9);
    let text = t.doc().selected_text();
    if text.is_empty() {
        return Err("dragging over a line selects nothing".into());
    }
    let p0 = picture(&t.rig);
    t.key(Key::B, Modifiers::COMMAND);
    let bold = t.doc().doc.paragraphs.iter().any(|p| {
        let mut at = 0;
        p.runs.iter().any(|r| {
            let piece = p.text.get(at..at + r.len).unwrap_or("");
            at += r.len;
            r.attrs.bold && piece.len() > 3 && text.contains(piece.trim())
        })
    });
    if !bold {
        return Err(format!("\"{}\" selected and made bold (Ctrl+B) isn't bold", text.trim()));
    }
    if changed(&p0, &picture(&t.rig), l.expand(4.0), None) < 0.01 {
        return Err("made bold, the line shows as before".into());
    }
    Ok(())
}

/// Typing over a selection replaces it.
fn doc_type_over_selection(t: &mut T) -> Res {
    let l = t.line(1)?;
    t.select_line(l, 0.5);
    let sel = t.doc().selected_text();
    if sel.trim().len() < 4 {
        return Err(format!("dragging over half a line selects \"{sel}\""));
    }
    let before: usize = t.doc().doc.paragraphs.iter().map(|p| p.text.len()).sum();
    t.type_text("X");
    let after: usize = t.doc().doc.paragraphs.iter().map(|p| p.text.len()).sum();
    if after + sel.len() != before + 1 {
        return Err(format!("typing over \"{sel}\" leaves {after} characters, {before} before"));
    }
    Ok(())
}

// --- sheets ------------------------------------------------------------------------------------

fn cell(t: &mut T, a1: &str) -> String {
    let c = apex_sheet::CellRef::parse(a1).expect("a cell");
    t.sheet().wb.display(0, c).0
}

/// Typed in a cell and entered: the value is there, and the cursor goes
/// down; a formula shows its result.
fn sheet_type_and_formula(t: &mut T) -> Res {
    t.key(Key::Home, Modifiers::COMMAND);
    for _ in 0..6 {
        t.key(Key::ArrowRight, Modifiers::NONE);
    }
    t.key(Key::ArrowDown, Modifiers::NONE);
    t.type_text("42");
    t.key(Key::Enter, Modifiers::NONE);
    let (v, at) = (cell(t, "G2"), t.sheet().audit_cursor());
    if v != "42" {
        return Err(format!("42 typed in G2 and entered: G2 shows \"{v}\""));
    }
    if at != apex_sheet::CellRef::parse("G3").unwrap() {
        return Err(format!("after Enter the cursor is at {at:?}, not G3"));
    }
    t.type_text("=SUM(B2:B4)");
    t.key(Key::Enter, Modifiers::NONE);
    let v = cell(t, "G3");
    if v != "18" {
        return Err(format!("=SUM(B2:B4) (3 + 6 + 9) shows \"{v}\""));
    }
    t.key(Key::Z, Modifiers::COMMAND);
    let v = cell(t, "G3");
    if !v.is_empty() {
        return Err(format!("undone (Ctrl+Z), G3 still shows \"{v}\""));
    }
    Ok(())
}

/// Cells dragged over show selected from where the drag began to where
/// it ended.
fn sheet_drag_selects(t: &mut T) -> Res {
    let a = t.area();
    let (from, to) = (a.lerp_inside(vec2(0.3, 0.3)), a.lerp_inside(vec2(0.45, 0.5)));
    let p0 = picture(&t.rig);
    drag(&mut t.rig, from, to);
    let p1 = picture(&t.rig);
    // Near the drag: the headers of its rows and columns light up too.
    let dragged = Rect::from_two_pos(from, to);
    let bands: Vec<Rect> = changed_bands(&p0, &p1, dragged.expand(150.0).intersect(a)).into_iter().filter(|b| b.intersects(dragged)).collect();
    let Some(b) = bands.iter().copied().reduce(|x, y| x.union(y)) else { return Err("dragging over cells shows nothing selected".into()) };
    if !b.expand(2.0).contains(from) || !b.expand(2.0).contains(to) {
        return Err(format!("dragged from {:.0},{:.0} to {:.0},{:.0}, the selection shows at {}", from.x, from.y, to.x, to.y, show(b)));
    }
    if b.width() > (to.x - from.x) + 260.0 || b.height() > (to.y - from.y) + 120.0 {
        return Err(format!("dragged over {:.0}×{:.0} points, the selection shows over {}", to.x - from.x, to.y - from.y, show(b)));
    }
    Ok(())
}

// --- slides ------------------------------------------------------------------------------------

/// A text box placed on the slide takes what is typed, and shows it.
fn slides_text_box(t: &mut T) -> Res {
    let n = t.objects().len();
    t.tab("menu.insert")?;
    t.click("slides.text_box")?;
    let at = t.area().lerp_inside(vec2(0.5, 0.8));
    let p0 = picture(&t.rig);
    click_at(&mut t.rig, at, 1);
    t.type_text("Hola mundo");
    t.key(Key::Escape, Modifiers::NONE);
    let objs = t.objects();
    if objs.len() != n + 1 {
        return Err(format!("a click with the text box tool makes {} objects", objs.len() as i64 - n as i64));
    }
    if !objs.iter().any(|o| format!("{:?}", o.kind).contains("Hola mundo")) {
        return Err("typed in a new text box, the words aren't in it".into());
    }
    let near = Rect::from_center_size(at, vec2(400.0, 120.0));
    if changed(&p0, &picture(&t.rig), near, None) < 0.002 {
        return Err("typed in a new text box, nothing shows where it was placed".into());
    }
    Ok(())
}

/// A rectangle drawn shows where dragged; dragged, it moves as far.
fn slides_draw_and_move(t: &mut T) -> Res {
    let n = t.objects().len();
    t.tab("menu.insert")?;
    t.click("shape.shapes")?;
    t.click("shape.k.rect")?;
    let a = t.area();
    let (from, to) = (a.lerp_inside(vec2(0.2, 0.55)), a.lerp_inside(vec2(0.4, 0.8)));
    let p0 = picture(&t.rig);
    drag(&mut t.rig, from, to);
    if t.objects().len() != n + 1 {
        return Err(format!("a drag with the rectangle makes {} objects", t.objects().len() as i64 - n as i64));
    }
    t.key(Key::Escape, Modifiers::NONE);
    let p1 = picture(&t.rig);
    let drawn = changed_bands(&p0, &p1, a).into_iter().reduce(|x, y| x.union(y)).ok_or("the rectangle drawn doesn't show")?;
    let want = Rect::from_two_pos(from, to);
    if (drawn.min - want.min).length() > 14.0 || (drawn.max - want.max).length() > 14.0 {
        return Err(format!("dragged over {}, the rectangle shows at {}", show(want), show(drawn)));
    }
    // Moved: from its middle, 90 right and 50 down.
    let objs0 = format!("{:?}", t.objects().last().map(|o| &o.sp));
    drag(&mut t.rig, want.center(), want.center() + vec2(90.0, 50.0));
    t.key(Key::Escape, Modifiers::NONE);
    if format!("{:?}", t.objects().last().map(|o| &o.sp)) == objs0 {
        return Err("the rectangle dragged doesn't move".into());
    }
    let p2 = picture(&t.rig);
    let moved = changed_bands(&p0, &p2, a).into_iter().reduce(|x, y| x.union(y)).ok_or("the rectangle moved doesn't show")?;
    let d = moved.min - drawn.min;
    if (d.x - 90.0).abs() > 10.0 || (d.y - 50.0).abs() > 10.0 {
        return Err(format!("dragged 90, 50 points, the rectangle moves {:.0}, {:.0}", d.x, d.y));
    }
    // Deleted, and back with undo.
    click_at(&mut t.rig, moved.center(), 1);
    t.key(Key::Delete, Modifiers::NONE);
    if t.objects().len() != n {
        return Err("the rectangle selected and deleted (Delete) stays".into());
    }
    t.key(Key::Z, Modifiers::COMMAND);
    if t.objects().len() != n + 1 {
        return Err("the rectangle deleted doesn't come back with undo (Ctrl+Z)".into());
    }
    Ok(())
}

/// A new slide comes after the one shown, and shows.
fn slides_new_slide(t: &mut T) -> Res {
    let (n, cur) = (t.slides().pres.deck.slides.len(), t.slides().cur);
    t.tab("toolstrip.home")?;
    // The icon above its name (the name opens the layouts).
    let c = t.find("slides.new_slide").ok_or("there is no \"New slide\"")?;
    click_at(&mut t.rig, pos2(c.rect.center().x, c.rect.min.y - 14.0), 1);
    let (m, now) = (t.slides().pres.deck.slides.len(), t.slides().cur);
    if m != n + 1 || now != cur + 1 {
        return Err(format!("New slide: {m} slides (were {n}), showing slide {} (was {})", now + 1, cur + 1));
    }
    Ok(())
}

/// A transition picked goes on the slide shown.
fn slides_transition(t: &mut T) -> Res {
    t.tab("slides.transitions")?;
    t.click("slides.tr.push")?;
    let v = t.slides();
    let tr = format!("{:?}", v.pres.deck.slides[v.cur].transition);
    if !tr.contains("Push") {
        return Err(format!("Push picked, the slide's transition is {tr}"));
    }
    Ok(())
}

// --- the run ---------------------------------------------------------------------------------

type Story = (Qv, &'static str, fn(&mut T) -> Res);

// --- every view ---------------------------------------------------------------------------------

/// File > Options opens the dialog, and Escape closes it (as every dialog).
fn options_close_with_escape(t: &mut T) -> Res {
    t.tab("menu.file")?;
    t.click("options.title")?;
    if t.rig.app.options.is_none() {
        return Err("File > Options doesn't open the dialog".into());
    }
    t.key(Key::Escape, Modifiers::NONE);
    if t.rig.app.options.is_some() {
        return Err("File > Options, then Escape: the dialog stays open".into());
    }
    Ok(())
}

// --- notes -------------------------------------------------------------------------------------

/// Clicked in the page and typed at its end, the words are in the page,
/// and shown.
fn note_type_in_page(t: &mut T) -> Res {
    let c = t.area().center();
    click_at(&mut t.rig, c, 1);
    t.key(Key::End, Modifiers::COMMAND);
    let p0 = picture(&t.rig);
    t.type_text(" Nota de prueba.");
    let View::Note(v) = t.view() else { return Err("not a notebook".into()) };
    let Some(ed) = &v.editor else { return Err("clicked in the page, no page is open to type in".into()) };
    let last = ed.doc.paragraphs.last().map(|p| p.text.clone()).unwrap_or_default();
    if !last.ends_with(" Nota de prueba.") {
        return Err(format!("clicked in the page and typed at its end (Ctrl+End), the last paragraph reads \"{last}\""));
    }
    if changed(&p0, &picture(&t.rig), t.area(), None) < 0.0005 {
        return Err("typed in the page, nothing new shows".into());
    }
    Ok(())
}

const STORIES: &[Story] = &[
    (Qv::Pdf, "selection on its text", pdf_selection_on_text),
    (Qv::Pdf, "strike out and take it off", pdf_strike_and_unstrike),
    (Qv::Pdf, "the highlighter lets go", pdf_highlighter_lets_go),
    (Qv::Pdf, "the hand drags the pages", pdf_hand_drags),
    (Qv::Pdf, "an arrow has a head", pdf_arrow_has_head),
    (Qv::Pdf, "a stamp in place, with its words", pdf_stamp_in_place),
    (Qv::Pdf, "the pen draws in place", pdf_pen_in_place),
    (Qv::Pdf, "the comments keep their width", pdf_comments_keep_width),
    (Qv::Doc, "selection on its text", doc_selection_on_text),
    (Qv::Doc, "typing at the end, and undo", doc_type_at_end),
    (Qv::Doc, "bold on a selection", doc_bold_selection),
    (Qv::Doc, "typing over a selection", doc_type_over_selection),
    (Qv::Sheet, "a value, a formula, undo", sheet_type_and_formula),
    (Qv::Sheet, "a drag selects the cells", sheet_drag_selects),
    (Qv::Slides, "a text box takes typing", slides_text_box),
    (Qv::Slides, "a rectangle drawn, moved, deleted", slides_draw_and_move),
    (Qv::Slides, "a new slide", slides_new_slide),
    (Qv::Slides, "a transition", slides_transition),
    (Qv::Note, "typing in a page", note_type_in_page),
    (Qv::Doc, "Options close with Escape", options_close_with_escape),
    (Qv::Sheet, "Options close with Escape", options_close_with_escape),
    (Qv::Slides, "Options close with Escape", options_close_with_escape),
    (Qv::Pdf, "Options close with Escape", options_close_with_escape),
    (Qv::Db, "Options close with Escape", options_close_with_escape),
    (Qv::Note, "Options close with Escape", options_close_with_escape),
];

fn run_view(job: &str, sink: &mut Sink) {
    audit::panic_hook();
    let Some(qv) = behave::job_view(job) else { return };
    let mut n = 0;
    for (i, (_, name, story)) in STORIES.iter().enumerate().filter(|(_, s)| s.0 == qv) {
        audit::watch(Some(format!("{job}: {name}")));
        sink.check();
        n += 1;
        let r = audit::quietly(|| {
            let mut t = T::new(qv, 300 + i as u64);
            common::take_errors();
            let r = story(&mut t);
            r.and_then(|()| match common::take_errors().first() {
                Some(e) => Err(format!("it reports an error: {e}")),
                None => Ok(()),
            })
        });
        let fail = match r {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(e) => {
                audit::drop_context();
                crate::toolstrip::test::reset();
                Some(format!("panics: {}", audit::panic_text(&e)))
            }
        };
        if let Some(e) = fail {
            sink.fail(common::id(&[job, "story", name]), format!("[story] {job}: {name} — {e}. Repro: app::qa::stories, story \"{name}\""));
        }
    }
    audit::watch(None);
    sink.note(format!("{n} stories"));
}

#[test]
#[ignore]
fn qa_stories() {
    let mut jobs: Vec<String> = Vec::new();
    for (qv, _, _) in STORIES {
        for lang in behave::LANGS {
            let job = format!("{}@{lang}", qv.name());
            if !jobs.contains(&job) {
                jobs.push(job);
            }
        }
    }
    common::run("stories", "app::qa::stories::qa_stories", &jobs, run_view);
}
