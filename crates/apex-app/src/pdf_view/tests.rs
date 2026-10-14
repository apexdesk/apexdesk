//! The PDF tab driven through egui frames: keys, find, selection, undo.

use apex_io::pdf::edit::annots::{Annot, Kind};
use eframe::egui::{self, Event, Key, Modifiers, Pos2, Rect, vec2};

use super::PdfView;
use crate::common::Shell;

/// A PDF of three pages, each with a line of text.
pub fn sample() -> Vec<u8> {
    let texts = ["Page one", "Page two alpha", "Page three alpha"];
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), "<< /Type /Pages /Kids [3 0 R 5 0 R 7 0 R] /Count 3 >>".to_string()];
    for (i, t) in texts.iter().enumerate() {
        let content = 4 + i * 2;
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << /Font << /F1 9 0 R >> >> /Contents {content} 0 R >>"));
        let c = format!("BT /F1 20 Tf 20 350 Td ({t}) Tj ET");
        objs.push(format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()));
    }
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());
    file_of(&objs)
}

/// A file of these objects (object i + 1 is `objs[i]`, 1 the catalog).
pub(super) fn file_of(objs: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn shell() -> Shell {
    Shell { cmd: None, show_formula_bar: false, file_name: "t.pdf".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None, recording: None }
}

/// Runs a frame with these events.
pub(super) fn frame(ctx: &egui::Context, v: &mut PdfView, events: Vec<Event>) {
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0))), events, ..Default::default() };
    let mut sh = shell();
    let mut out = ctx.run_ui(input, |ui| v.ui(ui, &mut sh));
    out.textures_delta.clear();
}

fn key(k: Key, m: Modifiers) -> Event {
    Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }
}

/// A view whose text has been read.
pub(super) fn ready() -> (egui::Context, PdfView) {
    // Our own context (tests run in parallel), its fonts ready from the
    // second frame on.
    let ctx = egui::Context::default();
    crate::fonts::install(&ctx);
    ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    let mut v = PdfView::open(sample(), None).unwrap();
    for _ in 0..400 {
        frame(&ctx, &mut v, Vec::new());
        if v.text.page(2).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(v.text.page(2).is_some(), "text layer not read");
    (ctx, v)
}

#[test]
fn keys_move_between_pages() {
    let (ctx, mut v) = ready();
    assert_eq!(v.page_count(), 3);
    frame(&ctx, &mut v, vec![key(Key::End, Modifiers::CTRL)]);
    assert_eq!(v.cur, 2);
    frame(&ctx, &mut v, vec![key(Key::Home, Modifiers::CTRL)]);
    assert_eq!(v.cur, 0);
    frame(&ctx, &mut v, vec![key(Key::PageDown, Modifiers::CTRL)]);
    assert_eq!(v.cur, 1);
    frame(&ctx, &mut v, vec![key(Key::PageUp, Modifiers::CTRL)]);
    assert_eq!(v.cur, 0);
}

#[test]
fn select_all_copy_and_escape() {
    let (ctx, mut v) = ready();
    frame(&ctx, &mut v, vec![key(Key::A, Modifiers::COMMAND)]);
    assert_eq!(v.selected_text().as_deref(), Some("Page one"));
    frame(&ctx, &mut v, vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(v.sel.is_none());
}

#[test]
fn find_bar_counts_matches() {
    let (ctx, mut v) = ready();
    frame(&ctx, &mut v, vec![key(Key::F, Modifiers::COMMAND)]);
    assert!(v.find.open);
    v.find.query = "ALPHA".into();
    frame(&ctx, &mut v, Vec::new());
    assert_eq!(v.find.hits.len(), 2);
    v.find.case = true;
    frame(&ctx, &mut v, Vec::new());
    assert!(v.find.hits.is_empty());
}

#[test]
fn annotations_undo_and_redo() {
    let (ctx, mut v) = ready();
    let a = Annot { page: 1, kind: Kind::Square, rect: [20.0, 20.0, 120.0, 80.0], color: 0xFF0000, width: 2.0, ..Default::default() };
    v.doc.add_annot(&a);
    v.after_change();
    assert!(v.is_modified());
    assert_eq!(v.doc.annots(1).len(), 1);
    v.undo();
    assert!(v.doc.annots(1).is_empty());
    v.redo();
    assert_eq!(v.doc.annots(1).len(), 1);
    frame(&ctx, &mut v, Vec::new());
    // What saving writes opens again with the annotation.
    let back = PdfView::open(v.doc.save(), None).unwrap();
    assert_eq!(back.doc.annots(1).len(), 1);
}

/// A mouse click at `p` (pressed in one frame, released in the next).
fn click(ctx: &egui::Context, v: &mut PdfView, p: Pos2) {
    let b = |pressed| Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    frame(ctx, v, vec![Event::PointerMoved(p), b(true)]);
    frame(ctx, v, vec![b(false)]);
}

/// A point on the first page (it fills the view's width).
const ON_PAGE: Pos2 = Pos2::new(600.0, 300.0);

#[test]
fn markup_tool_marks_the_selection() {
    let (ctx, mut v) = ready();
    frame(&ctx, &mut v, vec![key(Key::A, Modifiers::COMMAND)]);
    v.set_tool(super::Tool::Markup(Kind::Highlight));
    let a = v.doc.annots(0);
    assert_eq!(a.len(), 1);
    assert_eq!((a[0].kind, a[0].quads.len(), a[0].color), (Kind::Highlight, 1, v.pen.marker));
    assert!(v.sel.is_none());
    v.undo();
    assert!(v.doc.annots(0).is_empty());
}

#[test]
fn note_tool_asks_for_its_text() {
    let (ctx, mut v) = ready();
    v.set_tool(super::Tool::Draw(Kind::Note));
    click(&ctx, &mut v, ON_PAGE);
    let e = v.edit.as_mut().expect("comment box");
    e.text = "Check this".into();
    v.commit_edit();
    let a = v.doc.annots(0);
    assert_eq!((a.len(), a[0].kind, a[0].contents.as_str()), (1, Kind::Note, "Check this"));
    assert_eq!(v.picked, Some(a[0].id));
    // Still the note tool: Escape leaves the pick, then the tool.
    frame(&ctx, &mut v, vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(v.picked.is_none());
    frame(&ctx, &mut v, vec![key(Key::Escape, Modifiers::NONE)]);
    assert_eq!(v.tool, super::Tool::Select);
}

#[test]
fn delete_key_removes_the_picked_annotation() {
    let (ctx, mut v) = ready();
    let id = v.doc.add_annot(&Annot { page: 0, kind: Kind::Square, rect: [20.0, 20.0, 120.0, 80.0], ..Default::default() });
    let reply = v.doc.reply(id, "ok", "Bob");
    assert!(reply > 0);
    v.after_change();
    v.picked = Some(id);
    frame(&ctx, &mut v, vec![key(Key::Delete, Modifiers::NONE)]);
    assert!(v.doc.annots(0).is_empty());
    assert!(v.picked.is_none());
    v.undo();
    assert_eq!(v.doc.annots(0).len(), 2);
    // The comments pane shows it with its reply.
    v.pane = super::Pane::Comments;
    frame(&ctx, &mut v, Vec::new());
}

#[test]
fn fill_tools_write_on_the_page() {
    let (ctx, mut v) = ready();
    v.set_tool(super::Tool::FillDate);
    click(&ctx, &mut v, ON_PAGE);
    v.set_tool(super::Tool::FillCheck);
    click(&ctx, &mut v, ON_PAGE + vec2(0.0, 40.0));
    let a = v.doc.annots(0);
    assert_eq!(a.len(), 2);
    assert_eq!((a[0].kind, a[0].contents.clone()), (Kind::FreeText, super::annotate::today()));
    assert_eq!((a[1].kind, a[1].ink.len()), (Kind::Ink, 1));
}

#[test]
fn filled_in_text_takes_its_size_and_resizes_by_its_handles() {
    let (ctx, mut v) = ready();
    v.set_tool(super::Tool::FillText);
    v.set_font_size(20.0);
    // Left of the middle of the narrow sample page: the box fits.
    let at = Pos2::new(450.0, 300.0);
    click(&ctx, &mut v, at);
    v.edit.as_mut().expect("text box").text = "Hello".into();
    v.commit_edit();
    let a = v.doc.annots(0)[0].clone();
    let (w, h) = (a.rect[2] - a.rect[0], a.rect[3] - a.rect[1]);
    // As big as its text: one line of 20 pt, centred on the click.
    assert_eq!(a.font_size, 20.0);
    assert!((h - 27.0).abs() < 0.5, "{h}");
    let text_w = apex_io::pdf::edit::appearance::width("Hello", false, 20.0) as f32;
    assert!(w > text_w && w < text_w + 10.0, "{w} for {text_w}");
    // Its bottom right handle drags it larger, with any tool.
    let z = v.zoom;
    let corner = at + vec2(w * z, h / 2.0 * z);
    let b = |p, pressed| Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    let to = corner + vec2(40.0, 30.0);
    frame(&ctx, &mut v, vec![Event::PointerMoved(corner)]);
    frame(&ctx, &mut v, vec![b(corner, true)]);
    for k in 1..=4 {
        frame(&ctx, &mut v, vec![Event::PointerMoved(corner + (to - corner) * k as f32 / 4.0)]);
    }
    frame(&ctx, &mut v, vec![b(to, false)]);
    let r = v.doc.annots(0)[0].rect;
    assert_eq!(v.doc.annots(0).len(), 1, "no new box");
    assert!(((r[2] - r[0]) - (w + 40.0 / z)).abs() < 1.0 && ((r[3] - r[1]) - (h + 30.0 / z)).abs() < 1.0, "{r:?}");
    // The size menu changes the picked box, which grows to hold its text.
    v.set_font_size(36.0);
    let a = v.doc.annots(0)[0].clone();
    assert_eq!(a.font_size, 36.0);
    assert!(a.rect[3] - a.rect[1] >= 4.0 + 1.15 * 36.0 - 0.5);
}

/// A one-page form with two text fields near the top.
fn form() -> Vec<u8> {
    file_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] /DA (/Helv 0 Tf 0 g) >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Annots [4 0 R 5 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (a) /Rect [20 360 150 380] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (b) /Rect [20 330 150 350] >>".into(),
    ])
}

#[test]
fn typing_in_form_fields_and_tab() {
    let ctx = egui::Context::default();
    crate::fonts::install(&ctx);
    ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    let mut v = PdfView::open(form(), None).unwrap();
    frame(&ctx, &mut v, Vec::new());
    v.edit_field("a");
    frame(&ctx, &mut v, Vec::new());
    frame(&ctx, &mut v, vec![Event::Text("42".into())]);
    frame(&ctx, &mut v, vec![key(Key::Tab, Modifiers::NONE)]);
    let value = |v: &PdfView, n: &str| v.doc.fields().into_iter().find(|f| f.name == n).unwrap().value;
    assert_eq!(value(&v, "a"), "42");
    assert_eq!(v.field_edit.as_ref().map(|e| e.name.as_str()), Some("b"));
    frame(&ctx, &mut v, Vec::new());
    frame(&ctx, &mut v, vec![Event::Text("x".into())]);
    // Escape drops what was typed.
    frame(&ctx, &mut v, vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(v.field_edit.is_none());
    assert_eq!(value(&v, "b"), "");
    v.undo();
    assert_eq!(value(&v, "a"), "");
}

#[test]
fn placing_a_visible_signature() {
    let (ctx, mut v) = ready();
    v.open_sign_box();
    let b = v.sign_box.take().unwrap();
    let b = super::sign_box::SignBox { name: "Ana".into(), ..b };
    let look = b.look().unwrap();
    assert_eq!(look.lines[0], "Ana");
    v.sig_look = Some((look, true));
    v.tool = super::Tool::PlaceSign;
    click(&ctx, &mut v, ON_PAGE);
    let f = v.doc.fields();
    assert_eq!((f.len(), f[0].name.as_str()), (1, "Signature1"));
    assert!(matches!(v.requests.last(), Some(super::Request::Sign(Some(n))) if n == "Signature1"));
    assert_eq!(v.tool, super::Tool::Select);
}

#[test]
fn marking_marked_text_again_unmarks_it() {
    let (ctx, mut v) = ready();
    frame(&ctx, &mut v, vec![key(Key::A, Modifiers::COMMAND)]);
    let sel = v.sel;
    assert!(v.mark_selection(Kind::StrikeOut));
    assert_eq!(v.doc.annots(0).len(), 1);
    // The same text struck again: the strike goes (how a strike is undone).
    v.sel = sel;
    assert!(v.mark_selection(Kind::StrikeOut));
    assert!(v.doc.annots(0).is_empty());
}

#[test]
fn markup_button_pressed_again_lets_go() {
    let (ctx, mut v) = ready();
    v.set_tool(super::Tool::Markup(Kind::Highlight));
    assert_eq!(v.tool, super::Tool::Markup(Kind::Highlight));
    // Pressed again, even with text selected: back to selecting, nothing marked.
    frame(&ctx, &mut v, vec![key(Key::A, Modifiers::COMMAND)]);
    v.set_tool(super::Tool::Markup(Kind::Highlight));
    assert_eq!(v.tool, super::Tool::Select);
    assert!(v.doc.annots(0).is_empty());
}

#[test]
fn a_line_of_text_is_one_box() {
    let (_ctx, v) = ready();
    let p = v.text.page(1).unwrap();
    let boxes = super::annotate::line_boxes(p, 0, p.chars.len() - 1);
    assert_eq!(boxes.len(), 1);
    let b = boxes[0];
    assert!(b[2] > b[0] && b[3] > b[1]);
}

#[test]
fn comments_pane_keeps_its_width() {
    let (ctx, mut v) = ready();
    let id = v.doc.add_annot(&Annot { page: 0, kind: Kind::Note, rect: [20.0, 20.0, 40.0, 40.0], contents: "A rather long comment ".repeat(20), ..Default::default() });
    v.doc.reply(id, &"long reply ".repeat(30), "Bob");
    v.pane = super::Pane::Comments;
    let mut widths = Vec::new();
    for _ in 0..12 {
        frame(&ctx, &mut v, Vec::new());
        widths.push(v.zoom);
    }
    // The pane growing would shrink the page's fit zoom frame after frame.
    assert!(widths.windows(2).skip(3).all(|w| (w[0] - w[1]).abs() < 1e-3), "{widths:?}");
}
