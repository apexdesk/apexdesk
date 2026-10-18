use apex_doc::*;

fn caret(p: usize, o: usize) -> Selection {
    Selection::caret(Pos::new(p, o))
}

#[test]
fn typing_and_undo() {
    let mut d = Document::new();
    let mut pos = Pos::default();
    for ch in "Hola".chars() {
        pos = d.insert_text(Selection::caret(pos), &ch.to_string(), None);
    }
    assert_eq!(d.to_plain_text(), "Hola");
    assert_eq!(pos, Pos::new(0, 4));
    // Typing merges into one undo step.
    d.undo();
    assert_eq!(d.to_plain_text(), "");
    d.redo();
    assert_eq!(d.to_plain_text(), "Hola");
}

#[test]
fn split_and_join() {
    let mut d = Document::from_plain_text("abcdef");
    let p = d.new_paragraph(caret(0, 3));
    assert_eq!(p, Pos::new(1, 0));
    assert_eq!(d.to_plain_text(), "abc\ndef");
    let p = d.backspace(Selection::caret(p), false);
    assert_eq!(p, Pos::new(0, 3));
    assert_eq!(d.to_plain_text(), "abcdef");
    d.undo();
    assert_eq!(d.to_plain_text(), "abc\ndef");
    d.undo();
    assert_eq!(d.to_plain_text(), "abcdef");
    d.redo();
    d.redo();
    assert_eq!(d.to_plain_text(), "abcdef");
}

#[test]
fn multi_paragraph_insert_and_delete() {
    let mut d = Document::from_plain_text("uno\ndos\ntres");
    let p = d.insert_text(Selection { anchor: Pos::new(0, 1), cursor: Pos::new(2, 2) }, "X\nY", None);
    assert_eq!(d.to_plain_text(), "uX\nYes");
    assert_eq!(p, Pos::new(1, 1));
    d.undo();
    assert_eq!(d.to_plain_text(), "uno\ndos\ntres");
}

#[test]
fn formatting_runs() {
    let mut d = Document::from_plain_text("hola mundo");
    let sel = Selection { anchor: Pos::new(0, 5), cursor: Pos::new(0, 10) };
    d.toggle(sel, |a| a.bold, |a, v| a.bold = v);
    let p = &d.paragraphs[0];
    assert_eq!(p.runs.len(), 2);
    assert!(p.runs[1].attrs.bold);
    assert!(d.range_all(sel, |a| a.bold));
    d.toggle(sel, |a| a.bold, |a, v| a.bold = v);
    assert_eq!(d.paragraphs[0].runs.len(), 1);
    // Typing at end of bold text inherits bold.
    d.toggle(sel, |a| a.bold, |a, v| a.bold = v);
    d.insert_text(caret(0, 10), "!", None);
    assert_eq!(d.paragraphs[0].runs[1].len, 6);
}

#[test]
fn utf8_navigation() {
    let d = Document::from_plain_text("año ñu");
    assert_eq!(d.next_char(Pos::new(0, 1)), Pos::new(0, 3));
    assert_eq!(d.prev_char(Pos::new(0, 3)), Pos::new(0, 1));
    assert_eq!(d.next_word(Pos::new(0, 0)), Pos::new(0, 5));
    assert_eq!(d.prev_word(Pos::new(0, 8)), Pos::new(0, 5));
    assert_eq!(d.word_at(Pos::new(0, 1)), (Pos::new(0, 0), Pos::new(0, 4)));
}

#[test]
fn lists_and_search() {
    let mut d = Document::from_plain_text("a\nb\nc");
    d.set_para_style(0, 2, caret(0, 0), &|s| s.kind = ParaKind::Numbered);
    assert_eq!(d.list_number(2), 3);
    // Enter on empty list item ends the list.
    let p = d.new_paragraph(caret(2, 1));
    assert_eq!(d.paragraphs[p.para].style.kind, ParaKind::Numbered);
    d.new_paragraph(Selection::caret(p));
    assert_eq!(d.paragraphs[3].style.kind, ParaKind::Normal);
    let mut d = Document::from_plain_text("Casa casa CASA");
    assert_eq!(d.find_all("casa", false).len(), 3);
    assert_eq!(d.find("casa", Pos::new(0, 1), true), Some((Pos::new(0, 5), Pos::new(0, 9))));
    assert_eq!(d.replace_all("casa", "piso", false), 3);
    assert_eq!(d.to_plain_text(), "piso piso piso");
    d.undo();
    assert_eq!(d.to_plain_text(), "Casa casa CASA");
}

#[test]
fn paste_paragraphs() {
    let mut d = Document::from_plain_text("inicio fin");
    let src = Document::from_plain_text("A\nB");
    let clip = src.slice(Pos::new(0, 0), src.end());
    let p = d.insert_paragraphs(caret(0, 7), &clip);
    assert_eq!(d.to_plain_text(), "inicio A\nBfin");
    assert_eq!(p, Pos::new(1, 1));
    d.undo();
    assert_eq!(d.to_plain_text(), "inicio fin");
}

#[test]
fn change_case_keeps_formatting() {
    let mut p = Paragraph::new("");
    p.push("hola ", Attrs::default());
    p.push("mundo", Attrs { bold: true, ..Default::default() });
    let mut d = Document::from_paragraphs(vec![p]);
    let sel = Selection { anchor: Pos::new(0, 2), cursor: Pos::new(0, 8) };
    let after = d.map_text(sel, &|t| t.to_uppercase());
    assert_eq!(d.to_plain_text(), "hoLA MUNdo");
    assert_eq!(after.cursor, Pos::new(0, 8));
    assert!(d.paragraphs[0].attrs_at(6).bold);
    assert!(!d.paragraphs[0].attrs_at(3).bold);
    assert_eq!(d.undo_history(), vec![EditKind::Format]);
    d.undo();
    assert_eq!(d.to_plain_text(), "hola mundo");
}

#[test]
fn whole_word_find() {
    let d = Document::from_plain_text("cat catalog cat.");
    assert_eq!(d.find_all_opts("cat", false, false).len(), 3);
    assert_eq!(d.find_all_opts("cat", false, true).len(), 2);
}
