//! Other documents a Word macro opens (Documents.Open, Add): their objects
//! work on them and never on the macro's own, and the files they reach are
//! the macro's to reach.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use apex_doc::{Document, Pos, Selection};
use apex_sheet::{CellRef, Workbook};
use apex_vba::{DocFormat, ModuleKind, OpenDoc, Options, Project, Source, Ui};

/// Opens the documents it was given by file name and keeps those saved.
#[derive(Default)]
struct DocsUi {
    files: HashMap<String, Document>,
    books: HashMap<String, Workbook>,
    saved: Vec<(String, Document)>,
    printed: Vec<String>,
}

impl Ui for DocsUi {
    fn msg_box(&mut self, _prompt: &str, _buttons: i32, _title: &str) -> i32 {
        1
    }

    fn input_box(&mut self, _prompt: &str, _title: &str, _default: &str) -> Option<String> {
        None
    }

    fn debug_print(&mut self, text: &str) {
        self.printed.push(text.to_string());
    }

    fn allow_file_changes(&mut self, _folder: &str) -> bool {
        true
    }

    fn save_document(&mut self, doc: &Document, path: &str, _format: DocFormat) -> Result<(), String> {
        fs::write(path, b"saved").map_err(|e| e.to_string())?;
        self.saved.push((path.to_string(), doc.clone()));
        Ok(())
    }

    fn open_document(&mut self, path: &str) -> Result<Document, String> {
        let name = Path::new(path).file_name().unwrap().to_string_lossy().to_string();
        self.files.get(&name).cloned().ok_or_else(|| "not a document".into())
    }

    fn open_workbook(&mut self, path: &str, _password: Option<&str>) -> Result<Workbook, String> {
        let name = Path::new(path).file_name().unwrap().to_string_lossy().to_string();
        self.books.get(&name).cloned().ok_or_else(|| "not a workbook".into())
    }
}

/// A folder with the macro's document folder `doc` in it, and the
/// documents `names` there (each with its name as its text).
fn setup(test: &str, names: &[&str]) -> (PathBuf, DocsUi) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("apex-vba-docs").join(test);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("doc")).unwrap();
    let mut ui = DocsUi::default();
    for n in names {
        let mut d = Document::from_plain_text(n);
        d.modified = false;
        fs::write(root.join("doc").join(n), b"x").unwrap();
        ui.files.insert(n.to_string(), d);
    }
    (root, ui)
}

struct Ran {
    main: Document,
    error: Option<(i32, String)>,
    docs: Vec<OpenDoc>,
}

fn run(root: &Path, body: &str, ui: &mut DocsUi) -> Ran {
    run_in(root, Document::from_plain_text("main"), body, ui)
}

fn run_in(root: &Path, mut main: Document, body: &str, ui: &mut DocsUi) -> Ran {
    let p = Project::parse(&[Source { name: "ThisDocument".into(), kind: ModuleKind::Document, code: String::new() }, Source { name: "Module1".into(), kind: ModuleKind::Standard, code: format!("Sub Main()\n{body}\nEnd Sub\n") }]).unwrap_or_else(|e| panic!("line {}: {e}", e.line));
    let opts = Options { file_path: root.join("doc").display().to_string(), file_name: "Main.docm".into(), ..Options::default() };
    let out = apex_vba::run_doc_macro(&p, &mut main, Selection::caret(Pos::new(0, 0)), ui, &opts, "Main");
    Ran { main, error: out.error.map(|e| (e.number, format!("line {}: {e}", e.line))), docs: out.docs }
}

fn ok(root: &Path, body: &str, ui: &mut DocsUi) -> Ran {
    let r = run(root, body, ui);
    if let Some((_, e)) = &r.error {
        panic!("{e}");
    }
    r
}

fn text(d: &Document) -> String {
    d.paragraphs.iter().map(|p| p.text.to_string()).collect::<Vec<_>>().join("|")
}

#[test]
fn open_read_change_and_close() {
    let (root, mut ui) = setup("open", &["Data.docx"]);
    let r = ok(
        &root,
        r#"Dim d As Document
Set d = Documents.Open(ThisDocument.Path & "/Data.docx")
Debug.Print d.Name & "|" & d.Content.Text & "|" & TypeName(d) & "|" & Documents.Count
Debug.Print ActiveDocument.Name & "|" & ThisDocument.Name & "|" & Documents(1).Name & "|" & Documents("Data").Name
d.Content.InsertAfter " more"
Selection.TypeText "Typed "
ThisDocument.Content.InsertAfter " own"
Debug.Print d.Saved & "|" & (d Is Documents(2)) & "|" & (d Is ThisDocument)
d.Close SaveChanges:=True
Debug.Print Documents.Count & "|" & ActiveDocument.Name"#,
        &mut ui,
    );
    assert_eq!(ui.printed, ["Data.docx|Data.docx\r|Document|2", "Data.docx|Main.docm|Main.docm|Data.docx", "False|True|False", "1|Main.docm"]);
    // The macro's own document only got what was put in it.
    assert_eq!(text(&r.main), "main own");
    let (path, saved) = &ui.saved[0];
    assert!(path.ends_with("Data.docx"), "{path}");
    assert_eq!(text(saved), "Typed Data.docx more");
    assert!(r.docs.is_empty());
}

#[test]
fn objects_of_one_document_stay_in_it() {
    let (root, mut ui) = setup("objects", &["Data.docx"]);
    let r = ok(
        &root,
        r#"Dim d As Document, r As Range, n As Long
Set d = Documents.Open("Data.docx")
Set r = d.Range(0, 4)
ThisDocument.Activate
r.Bold = True
r.InsertAfter "!"
Debug.Print r.Text & "|" & ActiveDocument.Name & "|" & Selection.Start
' The text of another document's range, as a value.
ThisDocument.Content.InsertAfter r
For Each x In Documents
    n = n + 1
Next
Debug.Print n"#,
        &mut ui,
    );
    assert_eq!(ui.printed, ["Data!|Main.docm|0", " 2 "]);
    assert_eq!(text(&r.main), "mainData!");
    // Left open: the application shows it.
    assert_eq!(r.docs.len(), 1);
    assert_eq!(text(&r.docs[0].doc), "Data!.docx");
    assert!(r.docs[0].doc.modified);
    assert!(r.docs[0].path.as_deref().is_some_and(|p| p.ends_with("Data.docx")));
    // An object of the macro's document can't reach another's members.
    let r = run(&root, "Set d = Documents.Open(\"Data.docx\")\nDebug.Print d.Range.InRange(ThisDocument.Content)", &mut ui);
    assert!(r.error.as_ref().is_some_and(|e| e.1.contains("can't be used in another")), "{:?}", r.error);
}

#[test]
fn add_and_templates() {
    let (root, mut ui) = setup("add", &["Letter.dotx"]);
    let mut t = Document::from_plain_text("Dear");
    t.macros.project = Some(std::sync::Arc::new(vec![1, 2, 3]));
    ui.files.insert("Letter.dotx".into(), t);
    let r = ok(
        &root,
        r#"Dim a As Document, b As Document
Set a = Documents.Add
Set b = Documents.Add(Template:="Letter.dotx")
a.Content.Text = "New"
b.Content.InsertAfter " Sir"
Debug.Print Documents.Count & "|" & a.Name & "|" & b.Name & "|" & b.Path & "|" & ActiveDocument.Name
a.Close wdDoNotSaveChanges"#,
        &mut ui,
    );
    assert_eq!(ui.printed, ["3|Document2|Document3||Document3"]);
    assert_eq!(r.docs.len(), 1);
    assert_eq!(text(&r.docs[0].doc), "Dear Sir");
    assert!(r.docs[0].path.is_none());
    // A copy of a template has none of its macros.
    assert!(!r.docs[0].doc.macros.has_code());
}

#[test]
fn saving_and_the_files_reached() {
    let (root, mut ui) = setup("saving", &["Data.docx"]);
    fs::write(root.join("outside.docx"), b"x").unwrap();
    ui.files.insert("outside.docx".into(), Document::from_plain_text("out"));
    let mut code = Document::from_plain_text("code");
    code.macros.project = Some(std::sync::Arc::new(vec![1, 2, 3]));
    fs::write(root.join("doc").join("Code.docm"), b"x").unwrap();
    ui.files.insert("Code.docm".into(), code);
    let fails = [
        // Outside the document's folder.
        format!(r#"Documents.Open "{}""#, root.join("outside.docx").display()),
        format!(r#"Documents.Add "{}""#, root.join("outside.docx").display()),
        // Not there.
        r#"Documents.Open "Missing.docx""#.to_string(),
        // Over the macro's own document, or over another open one.
        r#"Set d = Documents.Open("Data.docx")
d.SaveAs ThisDocument.FullName"#
            .to_string(),
        r#"Set d = Documents.Add
Set e = Documents.Open("Data.docx")
d.SaveAs ThisDocument.Path & "/Data.docx""#
            .to_string(),
        // Never saved, or read-only.
        r#"Set d = Documents.Add
d.Save"#
            .to_string(),
        r#"Set d = Documents.Open("Data.docx", , True)
d.Save"#
            .to_string(),
        // Macros don't write files that run, nor copy another's macros.
        r#"Set d = Documents.Add
d.SaveAs ThisDocument.Path & "/x.exe""#
            .to_string(),
        r#"Set d = Documents.Open("Code.docm")
d.SaveAs ThisDocument.Path & "/Copy.docm""#
            .to_string(),
    ];
    for (i, body) in fails.iter().enumerate() {
        let r = run(&root, body, &mut ui);
        assert!(r.error.is_some(), "{i}: {body}");
        assert_eq!(text(&r.main), "main", "{i}");
    }
    assert!(ui.saved.is_empty(), "{:?}", ui.saved.iter().map(|s| &s.0).collect::<Vec<_>>());
    // Saved as another file: fine.
    let r = ok(&root, "Set d = Documents.Open(\"Data.docx\", , True)\nd.SaveAs2 ThisDocument.Path & \"/Out.docx\"\nDebug.Print d.Name", &mut ui);
    assert_eq!(ui.printed.last().map(String::as_str), Some("Out.docx"));
    assert!(ui.saved[0].0.ends_with("Out.docx"));
    drop(r);
}

/// A merge document and its data, People.csv, in the macro's folder.
fn merge_setup(test: &str) -> (PathBuf, DocsUi, Document) {
    let (root, mut ui) = setup(test, &["Data.docx"]);
    let mut wb = Workbook::new();
    for (r, row) in [["Name", "City"], ["Ana", "Madrid"], ["Bob", "Lyon"], ["", ""], ["Cy", "Oslo"]].iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            wb.set_input(0, CellRef::new(r as u32, c as u32), v);
        }
    }
    fs::write(root.join("doc").join("People.csv"), b"x").unwrap();
    fs::write(root.join("People.csv"), b"x").unwrap();
    ui.books.insert("People.csv".into(), wb);
    (root, ui, Document::from_plain_text("Dear \u{ab}Name\u{bb}, \u{ab}City\u{bb}"))
}

#[test]
fn mail_merge_from_a_macro() {
    let (root, mut ui, main) = merge_setup("merge");
    let r = run_in(
        &root,
        main,
        r#"With ThisDocument.MailMerge
    Debug.Print .State & "|" & .MainDocumentType & "|" & TypeName(.DataSource)
    .OpenDataSource "People.csv"
    Debug.Print .State & "|" & .MainDocumentType & "|" & .DataSource.RecordCount & "|" & .DataSource.FieldNames.Count & "|" & .DataSource.FieldNames(2).Name & "|" & .Fields.Count
    .DataSource.ActiveRecord = wdLastRecord
    Debug.Print .DataSource.ActiveRecord & "|" & .DataSource.DataFields("Name").Value & "|" & .DataSource.DataFields(2)
    Debug.Print .DataSource.FindRecord("ana", "Name") & "|" & .DataSource.ActiveRecord & "|" & .DataSource.FindRecord("nobody")
    .DataSource.FirstRecord = 2
    .Destination = wdSendToNewDocument
    .Execute
End With
Debug.Print (ActiveDocument Is ThisDocument) & "|" & Documents.Count"#,
        &mut ui,
    );
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(ui.printed, ["0|-1|MailMergeDataSource", "2|0|3|2|City|2", "3|Cy|Oslo", "True|1|False", "False|2"]);
    // The document merged is as it was; the letters are a new one.
    assert_eq!(text(&r.main), "Dear \u{ab}Name\u{bb}, \u{ab}City\u{bb}");
    assert_eq!(r.docs.len(), 1);
    let letters = text(&r.docs[0].doc);
    assert!(letters.contains("Dear Bob, Lyon") && letters.contains("Dear Cy, Oslo") && !letters.contains("Ana"), "{letters}");
    assert!(r.docs[0].path.is_none());
    assert!(!r.docs[0].doc.macros.has_code());
}

#[test]
fn mail_merge_fields_and_what_it_refuses() {
    let (root, mut ui, main) = merge_setup("merge-refused");
    let r = run_in(&root, main.clone(), "ThisDocument.MailMerge.Fields.Add ThisDocument.Range(0, 4), \"Greeting\"\nDebug.Print ThisDocument.MailMerge.Fields.Count", &mut ui);
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(text(&r.main), "\u{ab}Greeting\u{bb} \u{ab}Name\u{bb}, \u{ab}City\u{bb}");
    assert_eq!(ui.printed, [" 3 "]);
    let fails = [
        // Outside the document's folder, or not there.
        format!(r#"ThisDocument.MailMerge.OpenDataSource "{}""#, root.join("People.csv").display()),
        r#"ThisDocument.MailMerge.OpenDataSource "Missing.csv""#.to_string(),
        // Not a spreadsheet.
        r#"ThisDocument.MailMerge.OpenDataSource "Data.docx""#.to_string(),
        // No data source yet.
        "ThisDocument.MailMerge.Execute".to_string(),
        "Debug.Print ThisDocument.MailMerge.DataSource.RecordCount".to_string(),
        // Only form letters, to a new document.
        "ThisDocument.MailMerge.MainDocumentType = 1".to_string(),
        "ThisDocument.MailMerge.Destination = wdSendToPrinter".to_string(),
        // No such field or record.
        "ThisDocument.MailMerge.OpenDataSource \"People.csv\"\nDebug.Print ThisDocument.MailMerge.DataSource.FieldNames(9).Name".to_string(),
        "ThisDocument.MailMerge.OpenDataSource \"People.csv\"\nThisDocument.MailMerge.DataSource.ActiveRecord = 4".to_string(),
        "ThisDocument.MailMerge.OpenDataSource \"People.csv\"\nThisDocument.MailMerge.DataSource.FirstRecord = 9\nThisDocument.MailMerge.Execute".to_string(),
        // Merging a document the macro opened.
        "Set d = Documents.Open(\"Data.docx\")\nd.MailMerge.OpenDataSource \"People.csv\"\nd.MailMerge.Execute".to_string(),
    ];
    for (i, body) in fails.iter().enumerate() {
        let r = run_in(&root, main.clone(), body, &mut ui);
        assert!(r.error.is_some(), "{i}: {body}");
        assert!(r.docs.iter().all(|d| !text(&d.doc).contains("Madrid")), "{i}");
    }
}
