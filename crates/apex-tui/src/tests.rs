//! The interface drawn on ratatui's test terminal: text snapshots of a
//! sheet and a document, and editing through key events.

use apex_sheet::{CellRef, Value, Workbook};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use crate::{App, View};

/// Sets the interface language for a test; the lock keeps tests that
/// set another one from running meanwhile.
fn lang(code: &str) -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    apex_i18n::set(code);
    g
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| app.draw(f)).unwrap();
    let buf = t.backend().buffer();
    let mut out = String::new();
    for y in 0..h {
        let mut line = String::new();
        for x in 0..w {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn press(app: &mut App, code: KeyCode, m: KeyModifiers) {
    app.handle(Event::Key(KeyEvent::new(code, m)));
}

fn key(app: &mut App, code: KeyCode) {
    press(app, code, KeyModifiers::NONE);
}

fn ctrl(app: &mut App, c: char) {
    press(app, KeyCode::Char(c), KeyModifiers::CONTROL);
}

fn typing(app: &mut App, s: &str) {
    for c in s.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn c(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}

fn book() -> Workbook {
    let mut wb = Workbook::new();
    wb.locale = apex_sheet::Locale::EN;
    for (a, v) in [("A1", "Fruit"), ("B1", "Kilos"), ("A2", "Apples"), ("B2", "12"), ("A3", "Pears"), ("B3", "7.5"), ("A4", "Total"), ("B4", "=SUM(B2:B3)")] {
        wb.set_input(0, c(a), v);
    }
    wb.modified = false;
    wb
}

fn sheet(app: &App) -> &crate::sheet::SheetUi {
    match &app.view {
        View::Sheet(s) => s,
        View::Doc(_) => panic!("not a sheet"),
    }
}

fn value(app: &App, a: &str) -> Value {
    sheet(app).wb.sheets[0].value(c(a)).clone()
}

#[test]
fn sheet_snapshot() {
    let _l = lang("en");
    let mut app = App::from_workbook(book());
    let s = screen(&mut app, 48, 10);
    let expected = "  File  Edit  View  Insert  Format  Data  Sheet
 A1          fx Fruit
        A        B        C        D       E
  1 Fruit    Kilos
  2 Apples         12
  3 Pears         7.5
  4 Total        19.5
  5
 Sheet1   +
 Untitled  F9 menu · F10 commands · F1 help · …
";
    assert_eq!(s, expected, "\n{s}");
}

#[test]
fn edit_cells_and_undo() {
    let _l = lang("en");
    let mut app = App::from_workbook(book());
    screen(&mut app, 60, 12);
    // B2: type over it, Enter moves down.
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Down);
    typing(&mut app, "20");
    key(&mut app, KeyCode::Enter);
    assert_eq!(value(&app, "B2"), Value::Number(20.0));
    assert_eq!(value(&app, "B4"), Value::Number(27.5));
    assert_eq!(sheet(&app).cursor, c("B3"));
    assert!(app.modified());
    // A formula with autocomplete: "=su" + Tab gives "=SUM(".
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    typing(&mut app, "=sum");
    assert_eq!(sheet(&app).edit.as_ref().unwrap().ac.first(), Some(&"SUM"));
    key(&mut app, KeyCode::Tab);
    assert_eq!(sheet(&app).edit.as_ref().unwrap().line.text, "=SUM(");
    let s = screen(&mut app, 60, 12);
    assert!(s.contains("SUM(number1, [number2], …)"), "{s}");
    typing(&mut app, "B2:B4)");
    key(&mut app, KeyCode::Enter);
    assert_eq!(value(&app, "B5"), Value::Number(55.0));
    // Undo twice, redo once.
    ctrl(&mut app, 'z');
    assert_eq!(value(&app, "B5"), Value::Empty);
    ctrl(&mut app, 'z');
    assert_eq!(value(&app, "B2"), Value::Number(12.0));
    ctrl(&mut app, 'y');
    assert_eq!(value(&app, "B2"), Value::Number(20.0));
    // A bad formula stays in the cell being typed.
    typing(&mut app, "=SUM(");
    key(&mut app, KeyCode::Enter);
    assert!(sheet(&app).edit.is_some());
    assert!(app.message.as_ref().is_some_and(|m| m.1));
    key(&mut app, KeyCode::Esc);
    assert!(sheet(&app).edit.is_none());
}

#[test]
fn rows_sort_filter_and_replace() {
    let _l = lang("en");
    let mut app = App::from_workbook(book());
    screen(&mut app, 60, 12);
    // Sort the list by kilos, descending (the header row stays).
    key(&mut app, KeyCode::Right);
    app.run(crate::Cmd::SortDesc);
    assert_eq!(value(&app, "A2"), Value::text("Total"));
    assert_eq!(value(&app, "A3"), Value::text("Apples"));
    ctrl(&mut app, 'z');
    assert_eq!(value(&app, "A2"), Value::text("Apples"));
    // Insert a row above row 3, then delete it.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('r'), KeyModifiers::ALT);
    assert_eq!(value(&app, "A4"), Value::text("Pears"));
    assert!(matches!(value(&app, "B5"), Value::Number(n) if n == 19.5));
    press(&mut app, KeyCode::Char('R'), KeyModifiers::ALT | KeyModifiers::SHIFT);
    assert_eq!(value(&app, "A3"), Value::text("Pears"));
    // Autofilter: uncheck "Pears" in column A.
    press(&mut app, KeyCode::Char('l'), KeyModifiers::ALT);
    assert!(sheet(&app).sh().filter.is_some());
    press(&mut app, KeyCode::Up, KeyModifiers::CONTROL);
    key(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Down, KeyModifiers::ALT);
    typing(&mut app, "pea");
    key(&mut app, KeyCode::Char(' '));
    key(&mut app, KeyCode::Enter);
    assert!(sheet(&app).row_hidden(2), "Pears is filtered out");
    assert!(!sheet(&app).row_hidden(1));
    let s = screen(&mut app, 60, 12);
    assert!(!s.contains("Pears") && s.contains("Apples"), "{s}");
    // Replace in the whole sheet.
    ctrl(&mut app, 'r');
    typing(&mut app, "apples");
    key(&mut app, KeyCode::Enter);
    typing(&mut app, "Plums");
    key(&mut app, KeyCode::Enter);
    assert_eq!(value(&app, "A2"), Value::text("Plums"));
    // Go to, and a new sheet.
    ctrl(&mut app, 'g');
    typing(&mut app, "C10");
    key(&mut app, KeyCode::Enter);
    assert_eq!(sheet(&app).cursor, c("C10"));
    app.run(crate::Cmd::AddSheet);
    assert_eq!(sheet(&app).wb.sheets.len(), 2);
    assert_eq!(sheet(&app).sheet, 1);
    let s = screen(&mut app, 60, 12);
    assert!(s.contains("Sheet1") && s.contains("Sheet2"), "{s}");
}

#[test]
fn formats_and_selection_stats() {
    let _l = lang("en");
    let mut app = App::from_workbook(book());
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down, KeyModifiers::SHIFT);
    let s = screen(&mut app, 70, 12);
    assert!(s.contains("Sum: 19.5") && s.contains("Average: 9.75") && s.contains("Count: 2"), "{s}");
    app.run(crate::Cmd::NumFormat("0.00"));
    ctrl(&mut app, 'b');
    let st = sheet(&app).wb.styles.get(sheet(&app).wb.sheets[0].get(c("B3")).unwrap().style).clone();
    assert!(st.bold && &*st.num_fmt == "0.00");
    let s = screen(&mut app, 70, 12);
    assert!(s.contains("12.00") && s.contains("7.50"), "{s}");
}

fn doc() -> apex_doc::Document {
    apex_io::markdown::read("# Report\n\nSome **bold** and *italic* text that is long enough to wrap in a narrow terminal.\n\n- first\n- second\n\n| Name | Qty |\n|---|---|\n| Apples | 12 |\n\n## Next\n\nEnd.\n")
}

#[test]
fn document_snapshot() {
    let _l = lang("en");
    let mut app = App::from_document(doc());
    let s = screen(&mut app, 50, 24);
    let expected = "  File  Edit  View  Format  Paragraph  Help   Unt…
 Report
 ──────

 Some bold and italic text that is long enough to
 wrap in a narrow terminal.

 • first
 • second

 ┌────────┬─────┐
 │ Name   │ Qty │
 ├────────┼─────┤
 │ Apples │ 12  │
 └────────┴─────┘

 Next

 End.
";
    assert!(s.starts_with(expected), "\n{s}");
}

#[test]
fn edit_document_and_undo() {
    let _l = lang("en");
    let mut app = App::from_document(doc());
    screen(&mut app, 50, 24);
    let text = |app: &App, i: usize| match &app.view {
        View::Doc(d) => d.doc.paragraphs[i].text.clone(),
        _ => unreachable!(),
    };
    // Into the second paragraph, at its start.
    key(&mut app, KeyCode::Down);
    typing(&mut app, "Hi. ");
    assert!(text(&app, 1).starts_with("Hi. Some bold"), "{}", text(&app, 1));
    key(&mut app, KeyCode::Enter);
    assert_eq!(text(&app, 1), "Hi. ");
    // The bold run is kept by the edits.
    let bold = match &app.view {
        View::Doc(d) => d.doc.paragraphs[2].spans().any(|(s, e, a)| a.bold && &d.doc.paragraphs[2].text[s..e] == "bold"),
        _ => false,
    };
    assert!(bold);
    for _ in 0..3 {
        ctrl(&mut app, 'z');
    }
    assert!(text(&app, 1).starts_with("Some bold"), "{}", text(&app, 1));
    // Heading style from the keyboard.
    press(&mut app, KeyCode::Char('2'), KeyModifiers::ALT);
    match &app.view {
        View::Doc(d) => assert_eq!(d.doc.paragraphs[1].style.kind, apex_doc::ParaKind::Heading(2)),
        _ => unreachable!(),
    }
    // Headings panel.
    key(&mut app, KeyCode::F(6));
    let s = screen(&mut app, 80, 24);
    assert!(s.contains("Headings"), "{s}");
}

#[test]
fn quit_asks_when_modified() {
    let _l = lang("en");
    let mut app = App::from_workbook(book());
    ctrl(&mut app, 'q');
    assert!(app.should_quit());
    let mut app = App::from_workbook(book());
    typing(&mut app, "x");
    key(&mut app, KeyCode::Enter);
    ctrl(&mut app, 'q');
    assert!(!app.should_quit());
    let s = screen(&mut app, 70, 12);
    assert!(s.contains("Save the changes before quitting?"), "{s}");
    key(&mut app, KeyCode::Char('n'));
    assert!(app.should_quit());
}

#[test]
fn palette_runs_commands() {
    let _l = lang("en");
    let mut app = App::from_workbook(book());
    key(&mut app, KeyCode::F(10));
    typing(&mut app, "insert col");
    let s = screen(&mut app, 80, 20);
    assert!(s.contains("Insert columns"), "{s}");
    key(&mut app, KeyCode::Enter);
    assert_eq!(value(&app, "B1"), Value::text("Fruit"));
}

/// A book of 100,000 rows draws and moves quickly: only visible cells are
/// looked at.
#[test]
fn big_book_is_fast() {
    let mut wb = Workbook::new();
    wb.locale = apex_sheet::Locale::EN;
    let inputs: Vec<(CellRef, String)> = (0..100_000u32).flat_map(|r| [(CellRef::new(r, 0), format!("Item {r}")), (CellRef::new(r, 1), (r * 3).to_string())]).collect();
    wb.set_inputs(0, inputs);
    let mut app = App::from_workbook(wb);
    let t = std::time::Instant::now();
    screen(&mut app, 120, 40);
    press(&mut app, KeyCode::End, KeyModifiers::CONTROL);
    let s = screen(&mut app, 120, 40);
    for _ in 0..50 {
        key(&mut app, KeyCode::PageUp);
        screen(&mut app, 120, 40);
    }
    let ms = t.elapsed().as_millis();
    assert!(s.contains("100000") && s.contains("299997"), "{s}");
    eprintln!("100k rows: 52 frames in {ms} ms");
    assert!(ms < 5000, "{ms} ms");
}

// --- the menu bar ------------------------------------------------------------------------

fn bar_menu(app: &App) -> Option<usize> {
    match &app.overlay {
        Some(crate::ui::Overlay::Bar(b)) => Some(b.menu),
        _ => None,
    }
}

fn click(app: &mut App, x: u16, y: u16) {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    app.handle(Event::Mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE }));
}

/// Screen position of the first appearance of a text.
fn find(s: &str, t: &str) -> Option<(u16, u16)> {
    s.lines().enumerate().find_map(|(y, l)| l.find(t).map(|i| (l[..i].chars().count() as u16, y as u16)))
}

/// Draws the app; returns the text and the buffer.
fn frame(app: &mut App, w: u16, h: u16) -> (String, ratatui::buffer::Buffer) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| app.draw(f)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    (out, buf)
}

#[test]
fn menu_bar_keys() {
    let _l = lang("en");
    let mut app = App::from_workbook(book());
    screen(&mut app, 80, 24);
    // F9 pulls down File; ← wraps to Help, → comes back.
    key(&mut app, KeyCode::F(9));
    assert_eq!(bar_menu(&app), Some(0));
    let s = screen(&mut app, 80, 24);
    assert!(s.contains("Save as…") && s.contains("Ctrl+S") && s.contains("Quit"), "{s}");
    key(&mut app, KeyCode::Left);
    assert_eq!(bar_menu(&app), Some(7));
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    assert_eq!(bar_menu(&app), Some(1));
    key(&mut app, KeyCode::Esc);
    assert!(app.overlay.is_none());
    // Alt+T (Insert), Enter runs its first entry: insert rows.
    press(&mut app, KeyCode::Char('t'), KeyModifiers::ALT);
    assert_eq!(bar_menu(&app), Some(3));
    key(&mut app, KeyCode::Enter);
    assert!(app.overlay.is_none());
    assert_eq!(value(&app, "A2"), Value::text("Fruit"));
    // Alt+letters that are shortcuts keep doing their job: Alt+R inserts rows.
    press(&mut app, KeyCode::Char('r'), KeyModifiers::ALT);
    assert!(app.overlay.is_none());
    assert_eq!(value(&app, "A3"), Value::text("Fruit"));
    // A submenu: Format › Number format › 0.00, on B5.
    ctrl(&mut app, 'g');
    typing(&mut app, "B5");
    key(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('m'), KeyModifiers::ALT);
    for _ in 0..4 {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Right);
    let s = screen(&mut app, 80, 24);
    assert!(s.contains("Percentage, 2 decimals") && s.contains("dd/mm/yyyy"), "{s}");
    // Esc closes the submenu only.
    key(&mut app, KeyCode::Esc);
    assert_eq!(bar_menu(&app), Some(4));
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(app.overlay.is_none());
    let st = sheet(&app).wb.styles.get(sheet(&app).wb.sheets[0].get(c("B5")).unwrap().style).clone();
    assert_eq!(&*st.num_fmt, "0.00");
    // The underlined letter of an entry runs it: Edit › Select all.
    press(&mut app, KeyCode::Char('e'), KeyModifiers::ALT);
    let (s, buf) = frame(&mut app, 80, 24);
    let (x, y) = find(&s, "Select all").unwrap();
    let mn = (x..x + 10).find(|&k| buf[(k, y)].modifier.contains(ratatui::style::Modifier::UNDERLINED)).map(|k| buf[(k, y)].symbol().to_lowercase());
    typing(&mut app, &mn.expect("Select all has a mnemonic"));
    assert!(app.overlay.is_none());
    assert_eq!(sheet(&app).sel(), apex_sheet::Range::new(c("A1"), c("B6")));
}

#[test]
fn menu_bar_disabled_entries() {
    let _l = lang("en");
    let mut app = App::from_workbook(book());
    let s = screen(&mut app, 80, 24);
    assert!(s.starts_with("  File  Edit"), "{s}");
    // One sheet: Delete sheet, Next and Previous sheet don't apply, and
    // moving down skips them.
    press(&mut app, KeyCode::Char('h'), KeyModifiers::ALT);
    assert_eq!(bar_menu(&app), Some(6));
    let (s, buf) = frame(&mut app, 80, 24);
    let (x, y) = find(&s, "Delete sheet").unwrap();
    let (rx, ry) = find(&s, "Rename sheet").unwrap();
    assert_ne!(buf[(x, y)].fg, buf[(rx, ry)].fg, "disabled entries are gray");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    // That was Delete rows (row 1).
    assert_eq!(value(&app, "A1"), Value::text("Apples"));
    assert_eq!(sheet(&app).wb.sheets.len(), 1);
    // Find next without a search doesn't apply, nor Export as PDF without
    // the window program's hook; switches show their state.
    assert!(!app.cmd_state(&crate::Cmd::FindNext).0);
    assert!(!app.cmd_state(&crate::Cmd::ExportPdf).0);
    assert_eq!(app.cmd_state(&crate::Cmd::ToggleMenuBar), (true, Some(true)));
    // A disabled entry's letter does nothing.
    press(&mut app, KeyCode::Char('f'), KeyModifiers::ALT);
    let (s, buf) = frame(&mut app, 80, 24);
    let (x, y) = find(&s, "Export as PDF").unwrap();
    assert!((x..x + 13).all(|k| !buf[(k, y)].modifier.contains(ratatui::style::Modifier::UNDERLINED)), "{s}");
}

#[test]
fn menu_bar_mouse_and_hiding() {
    let _l = lang("en");
    let mut app = App::from_workbook(book());
    let s = screen(&mut app, 80, 24);
    // A click on "Edit" opens it, one on an entry runs it.
    let (x, y) = find(&s, "Edit").unwrap();
    click(&mut app, x, y);
    assert_eq!(bar_menu(&app), Some(1));
    let s = screen(&mut app, 80, 24);
    let (x, y) = find(&s, "Select all").unwrap();
    click(&mut app, x, y);
    assert!(app.overlay.is_none());
    assert_eq!(sheet(&app).sel(), apex_sheet::Range::new(c("A1"), c("B4")));
    // A click outside closes it without touching the sheet.
    let cursor = sheet(&app).cursor;
    let (x, y) = find(&s, "View").unwrap();
    click(&mut app, x, y);
    assert_eq!(bar_menu(&app), Some(2));
    click(&mut app, 70, 20);
    assert!(app.overlay.is_none());
    assert_eq!(sheet(&app).cursor, cursor);
    // Hidden, the sheet gets the row back; F9 still opens the bar.
    app.run(crate::Cmd::ToggleMenuBar);
    let s = screen(&mut app, 80, 24);
    assert!(s.starts_with(" A1"), "{s}");
    key(&mut app, KeyCode::F(9));
    let s = screen(&mut app, 80, 24);
    assert!(s.starts_with("  File") && s.contains("Save as…"), "{s}");
    key(&mut app, KeyCode::Esc);
    let s = screen(&mut app, 80, 24);
    assert!(s.starts_with(" A1"), "{s}");
    // Shift+F9 recalculates; F10 is still the command palette.
    press(&mut app, KeyCode::F(9), KeyModifiers::SHIFT);
    assert!(app.overlay.is_none());
    key(&mut app, KeyCode::F(10));
    assert!(matches!(app.overlay, Some(crate::ui::Overlay::Choice(_))));
}

#[test]
fn menu_bar_in_documents_in_spanish() {
    let _l = lang("es");
    let mut app = App::from_document(doc());
    let s = screen(&mut app, 100, 24);
    assert!(s.starts_with("  Archivo  Editar  Ver  Formato  Párrafo  Ayuda"), "{s}");
    // The title stays on the same row, right of the menus.
    assert!(s.lines().next().unwrap().contains("Report"), "{s}");
    // Alt+P: Párrafo; Enter opens "Estilo de párrafo"; Título 1 is its
    // third entry.
    press(&mut app, KeyCode::Char('p'), KeyModifiers::ALT);
    assert_eq!(bar_menu(&app), Some(4));
    key(&mut app, KeyCode::Enter);
    let s = screen(&mut app, 100, 24);
    assert!(s.contains("Estilo de párrafo") && s.contains("Título del documento"), "{s}");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    match &app.view {
        View::Doc(d) => assert_eq!(d.doc.paragraphs[0].style.kind, apex_doc::ParaKind::Heading(1)),
        _ => unreachable!(),
    }
    // Alt+B stays "bulleted list" (a shortcut), Alt+H is Archivo.
    press(&mut app, KeyCode::Char('b'), KeyModifiers::ALT);
    assert!(app.overlay.is_none());
    press(&mut app, KeyCode::Char('h'), KeyModifiers::ALT);
    assert_eq!(bar_menu(&app), Some(0));
    // Nothing selected: Cut and Copy don't apply.
    assert!(!app.cmd_state(&crate::Cmd::Copy).0);
    key(&mut app, KeyCode::Esc);
    // In sheets, Alt+A stays the alignment menu.
    let mut app = App::from_workbook(book());
    press(&mut app, KeyCode::Char('a'), KeyModifiers::ALT);
    assert!(matches!(app.overlay, Some(crate::ui::Overlay::Choice(_))));
}

/// In every language, the menus of each bar have distinct mnemonics that
/// no Alt shortcut uses, entries of a menu too, and the bar holds every
/// command of the palette.
#[test]
fn menu_bar_mnemonics_in_every_language() {
    use crate::menubar;
    use apex_io::Kind;
    let _l = lang("en");
    for l in apex_i18n::langs() {
        apex_i18n::set(l.code);
        for kind in [Kind::Sheet, Kind::Doc] {
            let taken = crate::cmd::alt_taken(kind);
            let mut seen = Vec::new();
            for (i, m) in menubar::mnemonics(kind).into_iter().enumerate() {
                let m = m.unwrap_or_else(|| panic!("{}: menu {i} has no mnemonic", l.code));
                assert!(m.is_ascii_alphanumeric(), "{}: '{m}' is not on every keyboard", l.code);
                assert!(!taken.contains(m), "{}: Alt+{m} is a shortcut ({kind:?})", l.code);
                assert!(!seen.contains(&m), "{}: '{m}' twice in the {kind:?} bar", l.code);
                seen.push(m);
            }
            let open = menubar::Open::new(kind, 0, &|_| (true, None));
            for list in menubar::entry_mnemonics(&open) {
                let mut u = list.clone();
                u.sort();
                u.dedup();
                assert_eq!(u.len(), list.len(), "{}: {list:?}", l.code);
            }
        }
    }
    apex_i18n::set("en");
    for kind in [Kind::Sheet, Kind::Doc] {
        let all = menubar::all_cmds(kind);
        for e in crate::cmd::palette(kind) {
            assert!(all.contains(&e.cmd) || e.cmd == crate::Cmd::MenuBar, "{:?} is not in the {kind:?} bar", e.cmd);
        }
    }
}
