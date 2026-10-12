//! Tests of frozen and split panes over real frames of the grid.

use super::*;

pub(super) fn view() -> SheetView {
    let mut wb = Workbook::empty();
    wb.sheets.push(Sheet::new("S"));
    for r in 0..200u32 {
        for c in 0..30u32 {
            wb.load_cell(0, CellRef::new(r, c), Content::Value(Value::Number((r * 100 + c) as f64)), 0);
        }
    }
    wb.finish_load();
    SheetView::new(wb)
}

/// Runs a frame of the grid, with `events` as input, and returns its
/// geometry.
pub(super) fn frame(v: &mut SheetView, events: Vec<egui::Event>) -> Geo {
    run(v, events);
    v.geo(v.pane_rect)
}

/// Runs a frame of the grid and returns what it painted, in order.
fn run(v: &mut SheetView, events: Vec<egui::Event>) -> Vec<egui::epaint::ClippedShape> {
    // One context per test (thread): input state isn't shared.
    thread_local! {
        static CTX: egui::Context = {
            let ctx = egui::Context::default();
            crate::fonts::install(&ctx);
            ctx
        };
    }
    let ctx = CTX.with(|c| c.clone());
    let mut shell = Shell { cmd: None, show_formula_bar: true, file_name: "panes".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None };
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), events, ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| {
        v.ui(ui, &mut shell);
    });
    out.textures_delta.clear();
    out.shapes
}

/// Screen top left of cell `c`.
fn at(v: &SheetView, g: &Geo, c: CellRef) -> Pos2 {
    g.cell_rect(v.sh(), c).min
}

/// Turns the wheel over `p` and lets the smooth scrolling end.
pub(super) fn wheel(v: &mut SheetView, p: Pos2, d: Vec2) {
    frame(v, vec![egui::Event::PointerMoved(p), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: d, modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move }]);
    for _ in 0..30 {
        frame(v, Vec::new());
    }
}

/// Freeze Panes at B2, A5 and E1 freezes what is above and left of the
/// cell; the frozen rows and columns stay while the rest scrolls.
#[test]
fn freeze_at_the_active_cell() {
    for (cell, rows, cols) in [("B2", 1, 1), ("A5", 4, 0), ("E1", 0, 4)] {
        let mut v = view();
        frame(&mut v, Vec::new());
        v.select(CellRef::parse(cell).unwrap(), false);
        v.toggle_freeze();
        assert_eq!((v.sh().frozen_rows, v.sh().frozen_cols), (rows, cols), "{cell}");
        assert!(v.frozen());
        let g = frame(&mut v, Vec::new());
        let a1 = at(&v, &g, CellRef::new(0, 0));
        let b2 = at(&v, &g, CellRef::new(1, 1));
        // Scrolling down and right keeps A1 in place.
        for _ in 0..5 {
            wheel(&mut v, pos2(700.0, 500.0), vec2(-200.0, -200.0));
        }
        let g = frame(&mut v, Vec::new());
        let (a1_now, b2_now) = (at(&v, &g, CellRef::new(0, 0)), at(&v, &g, CellRef::new(1, 1)));
        assert!(v.scroll.y > 100.0 && v.scroll.x > 100.0, "{cell}: {:?}", v.scroll);
        assert_eq!((a1_now.x == a1.x, a1_now.y == a1.y), (cols > 0, rows > 0), "{cell}");
        assert_ne!(b2_now, b2, "{cell}");
        // The first scrolled row and column come right after the frozen ones.
        let first = CellRef::new(g.scroll_rows(v.sh()).start().max(&rows).to_owned(), *g.scroll_cols(v.sh()).start());
        if rows > 0 {
            assert!(first.row > rows + 2, "{cell}: {first:?}");
            assert_eq!(g.y(v.sh(), first.row), g.y_end(v.sh(), rows - 1), "{cell}");
        }
        // Freeze Panes again unfreezes.
        v.toggle_freeze();
        assert!(!v.frozen());
    }
}

/// Freezing a scrolled sheet freezes from the top left cell in view, as
/// Excel does: the frozen rows fit and the others still scroll.
#[test]
fn freeze_when_scrolled() {
    let mut v = view();
    frame(&mut v, Vec::new());
    for _ in 0..3 {
        wheel(&mut v, pos2(700.0, 500.0), vec2(0.0, -200.0));
    }
    let g = frame(&mut v, Vec::new());
    let top = g.row_at(v.sh(), g.origin.y + 1.0);
    assert!(top > 10, "{top}");
    v.select(CellRef::new(top + 3, 2), false);
    let g = frame(&mut v, Vec::new());
    let before = at(&v, &g, CellRef::new(top + 3, 2));
    v.toggle_freeze();
    assert_eq!((v.sh().frozen_rows, v.sh().frozen_cols), (top + 3, 2));
    let g = frame(&mut v, Vec::new());
    // The view didn't move (but for the top row, now shown whole).
    let now = at(&v, &g, CellRef::new(top + 3, 2));
    assert!(now.x == before.x && (now.y - before.y).abs() < v.sh().row_height(top), "{now:?} {before:?}");
    assert_eq!(at(&v, &g, CellRef::new(top, 0)).y, g.origin.y);
    for _ in 0..3 {
        wheel(&mut v, pos2(700.0, 500.0), vec2(0.0, -200.0));
    }
    let g = frame(&mut v, Vec::new());
    assert_eq!(at(&v, &g, CellRef::new(top, 0)).y, g.origin.y, "frozen rows stay");
    assert!(g.row_at(v.sh(), g.origin.y + g.fy * g.z + 1.0) > top + 10, "the others scroll");
    // Scrolling back up stops at the first unfrozen row.
    for _ in 0..10 {
        wheel(&mut v, pos2(700.0, 500.0), vec2(0.0, 400.0));
    }
    let g = frame(&mut v, Vec::new());
    assert_eq!(g.row_at(v.sh(), g.origin.y + g.fy * g.z + 1.0), top + 3);
    // With the zoom too.
    v.zoom = 2.0;
    let g = frame(&mut v, Vec::new());
    assert_eq!(at(&v, &g, CellRef::new(top, 0)).y, g.origin.y);
    assert_eq!(at(&v, &g, CellRef::new(top + 3, 0)).y, g.origin.y + g.fy * 2.0);
    // Unfreezing shows the same cells.
    v.toggle_freeze();
    let g = frame(&mut v, Vec::new());
    assert_eq!(g.row_at(v.sh(), g.origin.y + 1.0), top);
}

/// Freeze Top Row, First Column and both freeze the top row and first
/// column in view.
#[test]
fn freeze_top_row_and_first_column() {
    for (row, col) in [(true, false), (false, true), (true, true)] {
        let mut v = view();
        frame(&mut v, Vec::new());
        v.select(CellRef::new(8, 6), false);
        v.freeze_top(row, col);
        assert_eq!((v.sh().frozen_rows, v.sh().frozen_cols), (row as u32, col as u32));
        wheel(&mut v, pos2(700.0, 500.0), vec2(-300.0, -300.0));
        let g = frame(&mut v, Vec::new());
        assert_eq!(at(&v, &g, CellRef::new(0, 0)) == g.origin, row && col);
        if row {
            assert_eq!(g.row_at(v.sh(), g.origin.y + 1.0), 0);
            assert!(g.row_at(v.sh(), g.origin.y + g.fy + 1.0) > 3);
        }
        if col {
            assert_eq!(g.col_at(v.sh(), g.origin.x + 1.0), 0);
        }
    }
}

fn press(p: Pos2, down: bool) -> egui::Event {
    egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: down, modifiers: egui::Modifiers::NONE }
}

/// Drags the pointer from `a` to `b`.
fn drag(v: &mut SheetView, a: Pos2, b: Pos2) {
    frame(v, vec![egui::Event::PointerMoved(a)]);
    frame(v, vec![press(a, true)]);
    for k in 1..=4 {
        frame(v, vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 4.0)]);
    }
    frame(v, vec![press(b, false)]);
    frame(v, Vec::new());
}

fn double_click(v: &mut SheetView, p: Pos2) {
    frame(v, vec![egui::Event::PointerMoved(p)]);
    frame(v, vec![press(p, true), press(p, false), press(p, true), press(p, false)]);
    frame(v, Vec::new());
}

/// Split at `cell` (unscrolled).
fn split_at(cell: &str) -> SheetView {
    let mut v = view();
    frame(&mut v, Vec::new());
    v.select(CellRef::parse(cell).unwrap(), false);
    v.toggle_split();
    frame(&mut v, Vec::new());
    v
}

/// View > Split divides at the active cell as Excel does: in four,
/// between columns only in the top row, between rows only in the first
/// column, in the middle at A1; the panes right and below go on where
/// the others end.
#[test]
fn split_at_the_active_cell() {
    let v = split_at("E8");
    let s = &v.f10.split;
    assert!(s.x.is_some() && s.y.is_some());
    let sh = v.sh();
    assert_eq!((s.xs[0], s.xs[1]), (0.0, sh.col_x(4)));
    assert_eq!((s.ys[0], s.ys[1]), (0.0, sh.row_y(7)));
    let v = split_at("E1");
    assert!(v.f10.split.x.is_some() && v.f10.split.y.is_none());
    assert_eq!(v.f10.split.xs[1], v.sh().col_x(4));
    let v = split_at("A8");
    assert!(v.f10.split.x.is_none() && v.f10.split.y.is_some());
    let v = split_at("A1");
    let (s, a) = (&v.f10.split, v.f10.split.area.unwrap());
    let (x, y) = (s.x.unwrap(), s.y.unwrap());
    assert!((x - a.width() / 2.0).abs() < 100.0 && (y - a.height() / 2.0).abs() < 40.0, "{x} {y} {a:?}");
}

/// The bars drag; onto the edge one goes away (four panes to two), a
/// double click removes one, and the panes of a row share the vertical
/// scroll and those of a column the horizontal one.
#[test]
fn split_bars_drag_and_go() {
    let mut v = split_at("E8");
    let a = v.f10.split.area.unwrap();
    let (x, y) = (v.f10.split.x.unwrap(), v.f10.split.y.unwrap());
    // Linked scrolling: the wheel over the bottom left pane.
    wheel(&mut v, a.min + vec2(x / 2.0, a.height() - 100.0), vec2(0.0, -300.0));
    let s = &v.f10.split;
    assert!(s.ys[1] > v.sh().row_y(12), "{:?}", s.ys);
    assert_eq!(s.ys[0], 0.0);
    // Moving a bar.
    drag(&mut v, a.min + vec2(x, y + 150.0), a.min + vec2(x + 100.0, y + 150.0));
    assert!((v.f10.split.x.unwrap() - (x + 100.0)).abs() < 2.0, "{:?}", v.f10.split.x);
    // The horizontal bar onto the column headers: two panes side by side.
    let x = v.f10.split.x.unwrap();
    drag(&mut v, a.min + vec2(x + 200.0, y), a.min + vec2(x + 200.0, 2.0));
    assert!(v.f10.split.y.is_none() && v.f10.split.x.is_some(), "{:?}", (v.f10.split.x, v.f10.split.y));
    assert!(v.split_on());
    // A double click on the other removes the split.
    double_click(&mut v, a.min + vec2(x, 300.0));
    assert!(!v.split_on());
    // The button removes it too.
    let mut v = split_at("E8");
    v.toggle_split();
    assert!(!v.split_on());
}

/// The marks of the cells (error triangle, note corner) go under a
/// picture over them; the fill handle of the selection goes over it.
#[test]
fn cell_marks_under_pictures() {
    let mut v = view();
    v.wb.set_input(0, CellRef::new(1, 1), "=1/0");
    v.wb.set_note(0, CellRef::new(1, 2), Some(apex_sheet::Note { text: "n".into(), ..Default::default() }));
    v.wb.sheets[0].images.push(apex_sheet::SheetImage::new(Arc::from(&b"x"[..]), apex_sheet::ImageFormat::Png, CellRef::new(1, 1), 300.0, 100.0));
    v.select(CellRef::new(2, 3), false);
    frame(&mut v, Vec::new());
    let shapes = run(&mut v, Vec::new());
    let fill = |color: Color32| {
        shapes.iter().position(|s| match &s.shape {
            egui::Shape::Path(p) => p.fill == color,
            egui::Shape::Rect(r) => r.fill == color && r.rect.width() < 10.0,
            _ => false,
        })
    };
    // The picture (a placeholder: its data isn't an image).
    let picture = fill(Color32::from_gray(222)).expect("picture");
    let flag = fill(Color32::from_rgb(0x10, 0x8A, 0x3C)).expect("error triangle");
    let note = fill(Color32::from_rgb(0xD0, 0x10, 0x10)).expect("note mark");
    let handle = fill(theme::SHEET_ACCENT).expect("fill handle");
    assert!(flag < picture && note < picture && handle > picture, "{flag} {note} {picture} {handle}");
}

/// The Page Break Preview of a sheet used down to a far row paints only
/// what shows: dashing each break line down the whole area made millions
/// of shapes and took gigabytes.
#[test]
fn page_breaks_of_a_far_cell_paint_only_what_shows() {
    let mut v = view();
    v.wb.set_input(0, CellRef::new(500_000, 300), "x");
    v.set_view_mode(apex_sheet::ViewMode::PageBreakPreview);
    let shapes = run(&mut v, Vec::new());
    let shapes = run(&mut v, Vec::new()).len().max(shapes.len());
    assert!(shapes < 20_000, "{shapes}");
}
