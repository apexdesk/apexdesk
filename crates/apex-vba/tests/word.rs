//! Macros of documents: Word's objects on an apex-doc document.

mod common;

use apex_doc::{Document, Pos, Selection};
use apex_vba::{DocOutcome, ModuleKind, Options, Project, Source};
use common::TestUi;

fn project(code: &str, doc_code: &str) -> Project {
    Project::parse(&[Source { name: "ThisDocument".into(), kind: ModuleKind::Document, code: doc_code.into() }, Source { name: "Module1".into(), kind: ModuleKind::Standard, code: code.into() }]).unwrap_or_else(|e| panic!("{} line {}: {e}", e.module, e.line))
}

fn caret(d: &Document) -> Selection {
    let _ = d;
    Selection::caret(Pos::new(0, 0))
}

/// Runs `Sub Main` made of `body` on `doc`.
fn run(doc: &mut Document, body: &str) -> (DocOutcome, TestUi) {
    let p = project(&format!("Sub Main()\n{body}\nEnd Sub\n"), "");
    let mut ui = TestUi::default();
    let sel = caret(doc);
    let out = apex_vba::run_doc_macro(&p, doc, sel, &mut ui, &Options::default(), "Main");
    (out, ui)
}

/// Runs `body` and returns its Debug.Print lines; an error fails.
fn ok(doc: &mut Document, body: &str) -> Vec<String> {
    let (out, ui) = run(doc, body);
    if let Some(e) = out.error {
        panic!("line {}: {e}", e.line);
    }
    ui.printed
}

fn err(doc: &mut Document, body: &str) -> apex_vba::Error {
    run(doc, body).0.error.expect("an error")
}

fn doc(text: &str) -> Document {
    Document::from_plain_text(text)
}

fn texts(d: &Document) -> Vec<String> {
    d.paragraphs.iter().map(|p| p.text.to_string()).collect()
}

#[test]
fn text_of_ranges_and_paragraphs() {
    let mut d = doc("Hello world\nSecond line");
    let p = ok(
        &mut d,
        r#"Debug.Print ActiveDocument.Paragraphs.Count
Debug.Print ActiveDocument.Paragraphs(2).Range.Text
Debug.Print ActiveDocument.Range(0, 5).Text
Debug.Print ActiveDocument.Words(2)
Debug.Print Len(ActiveDocument.Content.Text)
Dim r As Range
Set r = ActiveDocument.Range(6, 11)
r.Text = "there"
r.InsertAfter "!"
Debug.Print r.Text
' After the text, before the last paragraph mark.
ActiveDocument.Content.InsertAfter "Third"
Debug.Print ActiveDocument.Paragraphs.Count"#,
    );
    assert_eq!(p, [" 2 ", "Second line\r", "Hello", "world", " 24 ", "there!", " 2 "]);
    assert_eq!(texts(&d), ["Hello there!", "Second lineThird"]);
}

#[test]
fn ranges_follow_edits() {
    let mut d = doc("one two three");
    let p = ok(
        &mut d,
        r#"Dim r As Range
Set r = ActiveDocument.Range(8, 13)
ActiveDocument.Range(0, 0).InsertBefore "zero "
Debug.Print r.Text; r.Start
ActiveDocument.Range(0, 5).Delete
Debug.Print r.Text; r.Start"#,
    );
    assert_eq!(p, ["three 13 ", "three 8 "]);
}

#[test]
fn selection_typing_and_moving() {
    let mut d = doc("abc");
    let p = ok(
        &mut d,
        r#"Selection.EndKey Unit:=wdStory
Selection.TypeText "def"
Selection.TypeParagraph
Selection.TypeText "ghi"
Selection.HomeKey Unit:=wdStory
Selection.MoveRight Unit:=wdCharacter, Count:=2, Extend:=wdExtend
Debug.Print Selection.Text
Selection.Font.Bold = True"#,
    );
    assert_eq!(p, ["ab"]);
    assert_eq!(texts(&d), ["abcdef", "ghi"]);
    assert!(d.paragraphs[0].attrs_at(0).bold);
    assert!(!d.paragraphs[0].attrs_at(3).bold);
}

#[test]
fn find_in_a_loop() {
    let mut d = doc("cat and cat and dog cat");
    let p = ok(
        &mut d,
        r#"Dim r As Range, n As Long
Set r = ActiveDocument.Range(0, 19)
Do While r.Find.Execute(FindText:="cat")
    n = n + 1
    Debug.Print r.Start
Loop
Debug.Print n
Set r = ActiveDocument.Range(4, 23)
r.Find.Forward = False
Do While r.Find.Execute(FindText:="cat")
    Debug.Print r.Start
Loop
With ActiveDocument.Content.Find
    .Text = "and"
    Do While .Execute
        n = n + 1
    Loop
End With
Debug.Print n"#,
    );
    assert_eq!(p, [" 0 ", " 8 ", " 2 ", " 20 ", " 8 ", " 4 "]);
}

#[test]
fn find_and_replace() {
    let mut d = doc("cat and cat and dog");
    let p = ok(
        &mut d,
        r#"With ActiveDocument.Content.Find
    .Text = "cat"
    .Replacement.Text = "bird"
    .Execute Replace:=wdReplaceAll
End With
Dim r As Range
Set r = ActiveDocument.Content
r.Find.Execute FindText:="dog"
Debug.Print r.Find.Found; r.Start; r.End"#,
    );
    assert_eq!(texts(&d), ["bird and bird and dog"]);
    assert_eq!(p, ["True 18  21 "]);
}

#[test]
fn fonts_and_paragraph_format() {
    let mut d = doc("Title\nBody text");
    ok(
        &mut d,
        r#"With ActiveDocument.Paragraphs(1)
    .Style = wdStyleHeading1
    .Alignment = wdAlignParagraphCenter
End With
With ActiveDocument.Paragraphs(2).Range.Font
    .Italic = True
    .Size = 14
    .Color = wdColorRed
End With"#,
    );
    assert_eq!(d.paragraphs[0].style.kind, apex_doc::ParaKind::Heading(1));
    assert_eq!(d.paragraphs[0].style.align, apex_doc::Align::Center);
    let a = d.paragraphs[1].attrs_at(2);
    assert!(a.italic);
    assert_eq!(a.size, Some(28));
}

#[test]
fn tables() {
    let mut d = doc("Before\nAfter");
    let p = ok(
        &mut d,
        r#"Dim t As Table
Set t = ActiveDocument.Tables.Add(ActiveDocument.Paragraphs(2).Range, 2, 3)
t.Cell(1, 1).Range.Text = "A"
t.Cell(2, 3).Range.Text = "F"
Debug.Print ActiveDocument.Tables.Count; t.Rows.Count; t.Columns.Count
t.Rows.Add
Debug.Print t.Rows.Count
Debug.Print Len(t.Cell(1, 1).Range.Text)
Dim c As Cell, n As Long
For Each c In t.Rows(1).Cells
    n = n + 1
Next
Debug.Print n"#,
    );
    assert_eq!(p, [" 1  2  3 ", " 3 ", " 3 ", " 3 "]);
    assert!(d.paragraphs.iter().any(|q| &*q.text == "A" && q.cell.is_some()));
    assert!(d.paragraphs.iter().any(|q| &*q.text == "F"));
}

#[test]
fn table_sizes_and_cell_format() {
    let mut d = doc("Before\nAfter");
    let p = ok(
        &mut d,
        r#"Dim t As Table
Set t = ActiveDocument.Tables.Add(ActiveDocument.Paragraphs(2).Range, 2, 3)
t.Columns.Width = 100
t.Columns(2).Width = 150
Debug.Print t.Columns(1).Width; t.Columns(2).Width; t.Columns.Width
t.Rows(1).Height = 30
t.Rows(1).HeightRule = wdRowHeightExactly
Debug.Print t.Rows(1).Height; t.Rows(1).HeightRule; t.Rows(2).HeightRule; t.Rows.HeightRule
t.Rows(1).HeadingFormat = True
t.Rows(2).AllowBreakAcrossPages = False
Debug.Print t.Rows(1).HeadingFormat; t.Rows(2).HeadingFormat; t.Rows(2).AllowBreakAcrossPages
t.Rows.Alignment = wdAlignRowCenter
t.Rows.LeftIndent = 12
Debug.Print t.Rows.Alignment; t.Rows.LeftIndent
t.Cell(2, 3).Width = 80
t.Cell(2, 2).VerticalAlignment = wdCellAlignVerticalCenter
t.Cell(1, 1).Shading.BackgroundPatternColor = RGB(255, 0, 0)
Debug.Print t.Cell(2, 3).Width; t.Cell(2, 2).VerticalAlignment; t.Cell(1, 1).Shading.BackgroundPatternColor = vbRed
Debug.Print t.Cell(1, 2).Shading.BackgroundPatternColor = wdColorAutomatic; t.Cell(1, 1).Height"#,
    );
    assert_eq!(p, [" 100  150  9999999 ", " 30  2  0  9999999 ", "-1  0  0 ", " 1  12 ", " 80  1 True", "True 30 "]);
    let m = &d.tables()[0];
    let props = d.paragraphs[m.start].cell.as_ref().and_then(|c| c.table_props.as_deref()).expect("table props");
    assert_eq!(props.widths, [100.0, 150.0, 80.0]);
    assert_eq!(props.header_rows, 1);
    assert!(props.row_rules[0].exact && props.row_rules[1].cant_split);
    assert_eq!(props.align, apex_doc::Align::Center);
    assert_eq!(m.cell_props(&d.paragraphs, 1, 1).map(|c| c.valign), Some(apex_doc::VAlign::Center));
    assert_eq!(m.cell_props(&d.paragraphs, 0, 0).and_then(|c| c.shading), Some(0xFF0000));
    let mut d = doc("Before\nAfter");
    let e = err(&mut d, "ActiveDocument.Tables.Add(ActiveDocument.Paragraphs(2).Range, 1, 1).Columns(1).Width = 5000");
    assert_eq!(e.number, 4608);
}

#[test]
fn bookmarks_comments_and_links() {
    let mut d = doc("Some text here");
    let p = ok(
        &mut d,
        r#"ActiveDocument.Bookmarks.Add "Mark", ActiveDocument.Range(5, 9)
Debug.Print ActiveDocument.Bookmarks.Exists("Mark"); ActiveDocument.Bookmarks("mark").Range.Text
ActiveDocument.Comments.Add ActiveDocument.Range(0, 4), "Check this"
Debug.Print ActiveDocument.Comments.Count; ActiveDocument.Comments(1).Range.Text
ActiveDocument.Hyperlinks.Add ActiveDocument.Range(10, 14), "https://example.com"
Debug.Print ActiveDocument.Hyperlinks.Count; ActiveDocument.Hyperlinks(1).Address"#,
    );
    assert_eq!(p, ["Truetext", " 1 Check this", " 1 https://example.com"]);
    assert_eq!(d.comments.len(), 1);
    assert!(d.bookmark("Mark").is_some());
}

#[test]
fn unsafe_links_are_refused() {
    for a in ["javascript:alert(1)", "file:///etc/passwd", "vbscript:x", "data:text/html,x", "C:\\x.exe"] {
        let mut d = doc("Click");
        let e = err(&mut d, &format!("ActiveDocument.Hyperlinks.Add ActiveDocument.Range(0, 5), \"{a}\""));
        assert_eq!(e.number, 4198, "{a}");
        assert!(d.paragraphs[0].attrs_at(0).link.is_none());
    }
    let mut d = doc("Click");
    ok(&mut d, r#"ActiveDocument.Hyperlinks.Add ActiveDocument.Range(0, 5), "https://example.com""#);
    let e = err(&mut d, r#"ActiveDocument.Hyperlinks(1).Address = "javascript:alert(1)""#);
    assert_eq!(e.number, 4198);
    let e = err(&mut d, r#"ActiveDocument.Hyperlinks(1).Follow"#);
    assert_eq!(e.number, 445);
}

#[test]
fn headers_properties_and_page_setup() {
    let mut d = doc("Body");
    let p = ok(
        &mut d,
        r#"ActiveDocument.Sections(1).Headers(wdHeaderFooterPrimary).Range.Text = "My header"
Debug.Print ActiveDocument.Sections(1).Headers(1).Range.Text
ActiveDocument.BuiltInDocumentProperties("Title") = "Report"
ActiveDocument.BuiltInDocumentProperties(wdPropertyAuthor).Value = "Ann"
ActiveDocument.CustomDocumentProperties.Add Name:="Client", LinkToContent:=False, Type:=msoPropertyTypeString, Value:="ACME"
Debug.Print ActiveDocument.CustomDocumentProperties("client").Value
ActiveDocument.PageSetup.TopMargin = CentimetersToPoints(3)
ActiveDocument.PageSetup.Orientation = wdOrientLandscape
Debug.Print ActiveDocument.PageSetup.Orientation"#,
    );
    assert_eq!(p, ["My header\r", "ACME", " 1 "]);
    assert_eq!(d.meta.title, "Report");
    assert_eq!(d.meta.author, "Ann");
    assert!((d.page.margin_top - 85.04).abs() < 0.1);
    assert!(d.page.width > d.page.height);
}

#[test]
fn excel_names_are_not_there() {
    let mut d = doc("x");
    for body in ["Range(\"A1\").Value = 1", "Worksheets(1).Name = \"x\"", "ActiveSheet.Name = \"y\"", "x = ActiveCell.Value", "Application.ActiveSheet.Name = \"z\""] {
        let e = err(&mut d, body);
        assert!(e.number != 0, "{body}");
    }
    assert_eq!(texts(&d), ["x"]);
}

#[test]
fn the_document_module() {
    let p = project("Sub Main()\nThisDocument.Hello\nDebug.Print TypeName(ThisDocument); ThisDocument.Counter\nEnd Sub\n", "Public Counter As Long\nSub Hello()\nCounter = Counter + 1\nDebug.Print Me.Name; TypeName(Me)\nEnd Sub\n");
    let mut d = doc("x");
    let mut ui = TestUi::default();
    let sel = caret(&d);
    let mut opts = Options::default();
    opts.file_name = "Letter.docm".into();
    let out = apex_vba::run_doc_macro(&p, &mut d, sel, &mut ui, &opts, "Main");
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(ui.printed, ["Letter.docmDocument", "Document 1 "]);
    assert!(!out.changed);
}

#[test]
fn open_event() {
    let p = project("", "Private Sub Document_Open()\nActiveDocument.Content.InsertAfter \" opened\"\nEnd Sub\n");
    let mut d = doc("Doc");
    let mut ui = TestUi::default();
    let sel = caret(&d);
    let out = apex_vba::doc_event(&p, &mut d, sel, &mut ui, &Options::default(), "Open").expect("a handler");
    assert!(out.error.is_none());
    assert!(out.changed);
    assert_eq!(texts(&d), ["Doc opened"]);
    assert!(apex_vba::doc_handler(&p, "Close").is_none());
    let p = project("Sub AutoOpen()\nEnd Sub\n", "");
    assert!(apex_vba::doc_handler(&p, "Open").is_some());
}

#[test]
fn saving_needs_permission() {
    let mut d = doc("x");
    let e = err(&mut d, r#"ActiveDocument.SaveAs2 FileName:="/tmp/never-written.docx""#);
    assert_eq!(e.number, 70);
    let e = err(&mut d, r#"ActiveDocument.SaveAs2 FileName:="/tmp/never-written.exe""#);
    assert_ne!(e.number, 0);
    let (out, _) = run(&mut d, "ActiveDocument.Save");
    assert!(out.save);
}

#[test]
fn protected_documents_are_not_changed() {
    let mut d = doc("Locked");
    d.protection = Some(apex_doc::Protection { limit: apex_doc::EditLimit::ReadOnly, enforced: true, password: None });
    let e = err(&mut d, r#"ActiveDocument.Content.InsertAfter "x""#);
    assert_eq!(e.number, 4605);
    let e = err(&mut d, r#"ActiveDocument.BuiltInDocumentProperties("Title") = "x""#);
    assert_eq!(e.number, 4605);
    let p = ok(&mut d, "Debug.Print ActiveDocument.ProtectionType = wdAllowOnlyReading");
    assert_eq!(p, ["True"]);
    assert_eq!(texts(&d), ["Locked"]);
}

#[test]
fn for_each_and_statistics() {
    let mut d = doc("one two\nthree\nfour five six");
    let p = ok(
        &mut d,
        r#"Dim p As Paragraph, n As Long
For Each p In ActiveDocument.Paragraphs
    ' Each paragraph mark is a "word" too.
    n = n + p.Range.Words.Count
Next
Debug.Print n
Debug.Print ActiveDocument.ComputeStatistics(wdStatisticWords)
Debug.Print ActiveDocument.Paragraphs.Last.Range.Words.First"#,
    );
    assert_eq!(p, [" 9 ", " 6 ", "four "]);
}

#[test]
fn errors_are_word_errors() {
    let mut d = doc("x");
    assert_eq!(err(&mut d, "x = ActiveDocument.Tables(1).Rows.Count").number, 5941);
    assert_eq!(err(&mut d, "x = ActiveDocument.Bookmarks(\"none\").Range.Text").number, 5941);
    assert_eq!(err(&mut d, "ActiveDocument.Bookmarks.Add \"1bad\"").number, 5828);
    assert_eq!(err(&mut d, "ActiveDocument.Tables.Add ActiveDocument.Range(0, 0), 1, 64").number, 4608);
    assert_eq!(err(&mut d, "Debug.Print Documents(2).Name").number, 5941);
}

#[test]
fn protect_and_unprotect() {
    let mut d = doc("Text");
    ok(&mut d, r#"ActiveDocument.Protect wdAllowOnlyReading, Password:="secret"
Debug.Print ActiveDocument.ProtectionType"#);
    let p = d.protection.clone().expect("protected");
    assert!(p.enforced && p.password.is_some() && p.limit == apex_doc::EditLimit::ReadOnly);
    // Nothing changes while it is protected, a wrong password doesn't stop it.
    assert_eq!(err(&mut d, "ActiveDocument.Content.Text = \"x\"").number, 4605);
    assert_eq!(err(&mut d, "ActiveDocument.Unprotect \"wrong\"").number, 5485);
    assert_eq!(err(&mut d, "ActiveDocument.Unprotect").number, 5485);
    assert_eq!(err(&mut d, "ActiveDocument.Protect 1").number, 4605);
    assert_eq!(err(&mut d, "ActiveDocument.Undo").number, 4605);
    assert!(d.edit_limit().is_some());
    let p = ok(&mut d, r#"ActiveDocument.Unprotect "secret"
ActiveDocument.Content.Text = "Changed"
Debug.Print ActiveDocument.ProtectionType
ActiveDocument.Protect wdAllowOnlyComments"#);
    assert_eq!(p, ["-1 "]);
    assert_eq!(texts(&d), ["Changed"]);
    assert_eq!(d.edit_limit(), Some(apex_doc::EditLimit::Comments));
    // Without a password any macro stops it; not protected, nothing to stop.
    ok(&mut d, "ActiveDocument.Unprotect");
    assert_eq!(err(&mut d, "ActiveDocument.Unprotect").number, 4605);
    // A password this program can't check isn't taken.
    let mut d = doc("Old");
    d.protection = Some(apex_doc::Protection { limit: apex_doc::EditLimit::ReadOnly, enforced: true, password: Some(apex_doc::ProtectionHash { algorithm: String::new(), hash: String::new(), salt: String::new(), spin: 0, legacy: vec![("hash".into(), "x".into())] }) });
    assert_eq!(err(&mut d, "ActiveDocument.Unprotect \"\"").number, 5485);
    assert!(d.edit_limit().is_some());
}

#[test]
fn undo_and_redo_of_the_macros_edits() {
    let mut d = doc("one");
    let p = ok(&mut d, r#"ActiveDocument.Content.InsertAfter " two"
ActiveDocument.Content.InsertAfter " three"
Debug.Print ActiveDocument.Undo
Debug.Print ActiveDocument.Paragraphs(1).Range.Text
Debug.Print ActiveDocument.Redo
Debug.Print ActiveDocument.Undo(5)
Debug.Print ActiveDocument.Paragraphs(1).Range.Text
ActiveDocument.Content.InsertAfter " four"
ActiveDocument.UndoClear
Debug.Print ActiveDocument.Undo"#);
    assert_eq!(p, ["True", "one two\r", "True", "False", "one\r", "False"]);
    assert_eq!(texts(&d), ["one four"]);
}

#[test]
fn content_controls() {
    let mut d = doc("Name: \nDone: ");
    let p = ok(&mut d, r#"Dim c As ContentControl
Set c = ActiveDocument.ContentControls.Add(wdContentControlText, ActiveDocument.Range(6, 6))
c.Title = "Name"
c.Tag = "name"
Debug.Print c.ShowingPlaceholderText; c.Type
Dim b As ContentControl
Set b = ActiveDocument.ContentControls.Add(wdContentControlCheckBox, ActiveDocument.Paragraphs(2).Range.Characters.Last)
b.Checked = True
Debug.Print b.Checked; ActiveDocument.ContentControls.Count
Debug.Print ActiveDocument.SelectContentControlsByTag("name")(1).Title
Debug.Print ActiveDocument.SelectContentControlsByTitle("None").Count
Dim l As ContentControl
Set l = ActiveDocument.ContentControls.Add(wdContentControlDropdownList, ActiveDocument.Range(0, 0))
l.DropdownListEntries.Add "Red", "R"
l.DropdownListEntries.Add "Blue", "B"
l.DropdownListEntries(2).Select
Debug.Print l.Range.Text; l.DropdownListEntries.Count; l.DropdownListEntries(1).Value
Dim x
For Each x In ActiveDocument.ContentControls
    Debug.Print x.Type;
Next
Debug.Print
l.Delete True
Debug.Print ActiveDocument.ContentControls.Count; c.Title
c.LockContentControl = True"#);
    assert_eq!(p, ["True 1 ", "True 2 ", "Name", " 0 ", "Blue 2 R", " 4 ", " 1 ", " 8 ", "", " 2 Name"]);
    assert_eq!(err(&mut d, "ActiveDocument.SelectContentControlsByTag(\"name\")(1).Delete").number, 6124);
    assert_eq!(err(&mut d, "ActiveDocument.ContentControls.Add 2").number, 445);
    let c = d.controls();
    assert_eq!(c.len(), 2);
    assert!(c[0].control.lock_delete && c[0].control.tag == "name");
    assert!(matches!(c[1].control.kind, apex_doc::ControlKind::CheckBox { checked: true }));
    assert!(texts(&d)[1].contains(apex_doc::CHECKED));
}

#[test]
fn writing_in_a_content_control() {
    let mut d = doc("Name: ");
    let p = ok(&mut d, r#"Dim c As ContentControl
Set c = ActiveDocument.ContentControls.Add(wdContentControlText, ActiveDocument.Range(6, 6))
c.Range.Text = "Ana"
Debug.Print ActiveDocument.ContentControls.Count; c.ShowingPlaceholderText; c.Range.Text"#);
    assert_eq!(p, [" 1 FalseAna"]);
    assert_eq!(texts(&d), ["Name: Ana"]);
    // Text added beside it isn't in it; locked contents stay.
    let p = ok(&mut d, r#"ActiveDocument.Content.InsertAfter "!"
ActiveDocument.ContentControls(1).LockContents = True
Debug.Print ActiveDocument.ContentControls(1).Range.Text"#);
    assert_eq!(p, ["Ana"]);
    assert_eq!(err(&mut d, "ActiveDocument.ContentControls(1).Range.Text = \"Eva\"").number, 6124);
    assert_eq!(texts(&d), ["Name: Ana!"]);
}
