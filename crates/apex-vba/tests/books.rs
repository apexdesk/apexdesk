//! Other workbooks a macro opens (Workbooks.Open, Add): their objects work
//! on them and never on the macro's own, and the files they reach are the
//! macro's to reach.

mod common;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use apex_sheet::{CellRef, Workbook};
use apex_vba::files::BookFormat;
use apex_vba::{OpenBook, Options, Ui};
use common::*;

/// Opens the workbooks it was given by file name and keeps those saved.
#[derive(Default)]
struct BooksUi {
    files: HashMap<String, Workbook>,
    saved: Vec<(String, Workbook)>,
    printed: Vec<String>,
    answers: Vec<i32>,
}

impl Ui for BooksUi {
    fn msg_box(&mut self, _prompt: &str, _buttons: i32, _title: &str) -> i32 {
        self.answers.pop().unwrap_or(1)
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

    fn save_workbook(&mut self, _wb: &Workbook, path: &str, _format: BookFormat, _sheet: usize) -> Result<(), String> {
        panic!("the macro's own workbook was saved at {path}");
    }

    fn save_opened(&mut self, wb: &Workbook, path: &str, _format: BookFormat, _sheet: usize) -> Result<(), String> {
        fs::write(path, b"saved").map_err(|e| e.to_string())?;
        self.saved.push((path.to_string(), wb.clone()));
        Ok(())
    }

    fn open_workbook(&mut self, path: &str, _password: Option<&str>) -> Result<Workbook, String> {
        let name = Path::new(path).file_name().unwrap().to_string_lossy().to_string();
        self.files.get(&name).cloned().ok_or_else(|| "not a workbook".into())
    }
}

/// A folder with the macro's workbook folder `book` in it, and the
/// workbooks `names` there (each with its name in A1 of its sheet).
fn setup(test: &str, names: &[&str]) -> (PathBuf, BooksUi) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("apex-vba-books").join(test);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("book")).unwrap();
    let mut ui = BooksUi::default();
    for n in names {
        let mut wb = workbook();
        wb.set_input(0, CellRef::parse("A1").unwrap(), n);
        wb.set_input(0, CellRef::parse("B1").unwrap(), "2");
        wb.modified = false;
        fs::write(root.join("book").join(n), b"x").unwrap();
        ui.files.insert(n.to_string(), wb);
    }
    (root, ui)
}

struct Ran {
    main: Workbook,
    error: Option<(i32, String)>,
    books: Vec<OpenBook>,
}

fn run(root: &Path, body: &str, ui: &mut BooksUi) -> Ran {
    let mut main = workbook();
    main.set_input(0, CellRef::parse("A1").unwrap(), "main");
    let p = project(&format!("Sub Main()\n{body}\nEnd Sub\n"));
    let opts = Options { file_path: root.join("book").display().to_string(), file_name: "Main.xlsm".into(), ..Options::default() };
    let out = apex_vba::run_macro(&p, &mut main, ui, &opts, "Main");
    Ran { main, error: out.error.map(|e| (e.number, format!("line {}: {e}", e.line))), books: out.books }
}

fn ok(root: &Path, body: &str, ui: &mut BooksUi) -> Ran {
    let r = run(root, body, ui);
    if let Some((_, e)) = &r.error {
        panic!("{e}");
    }
    r
}

fn cell(wb: &Workbook, a1: &str) -> String {
    text(wb, 0, a1)
}

#[test]
fn open_read_change_and_close() {
    let (root, mut ui) = setup("open", &["Data.xlsx"]);
    let r = ok(
        &root,
        r#"Dim wb As Workbook
Set wb = Workbooks.Open(ThisWorkbook.Path & "/Data.xlsx")
Debug.Print wb.Name & "|" & wb.Sheets(1).Range("A1").Value & "|" & TypeName(wb) & "|" & Workbooks.Count
Debug.Print ActiveWorkbook.Name & "|" & ThisWorkbook.Name & "|" & Range("A1").Value & "|" & ActiveSheet.Range("A1").Value
wb.Sheets(1).Range("C1").Value = wb.Sheets(1).Range("B1").Value * 10
Range("D1").Value = "active"
ThisWorkbook.Sheets(1).Range("B1").Value = wb.Sheets(1).Range("A1")
Debug.Print wb.Saved & "|" & (wb Is Workbooks(2)) & "|" & (wb Is ThisWorkbook) & "|" & Workbooks("Data").Name
wb.Close SaveChanges:=True
Debug.Print Workbooks.Count & "|" & Range("A1").Value"#,
        &mut ui,
    );
    assert_eq!(ui.printed, ["Data.xlsx|Data.xlsx|Workbook|2", "Data.xlsx|Main.xlsm|Data.xlsx|Data.xlsx", "False|True|False|Data.xlsx", "1|main"]);
    // The macro's own workbook only got what was put in it.
    assert_eq!(cell(&r.main, "B1"), "Data.xlsx");
    assert_eq!(cell(&r.main, "C1"), "");
    assert_eq!(cell(&r.main, "D1"), "");
    let (path, saved) = &ui.saved[0];
    assert!(path.ends_with("Data.xlsx"), "{path}");
    assert_eq!(cell(saved, "C1"), "20");
    assert_eq!(cell(saved, "D1"), "active");
    assert!(r.books.is_empty());
}

#[test]
fn objects_of_one_workbook_stay_in_it() {
    let (root, mut ui) = setup("objects", &["Data.xlsx"]);
    let r = ok(
        &root,
        r#"Dim wb As Workbook, ws As Worksheet, c, a, n As Long
Set wb = Workbooks.Open("Data.xlsx")
Set ws = wb.Worksheets(1)
ThisWorkbook.Activate
For Each c In ws.Range("A2:A3")
    n = n + 1
    c.Value = n
Next
For Each a In ws.Range("E1").Areas
    a.Value = "area"
Next
With ws.Range("F1")
    .Value = 7
    .Font.Bold = True
    .Offset(1, 0).Value = 8
End With
ws.Cells(1, 7) = 9
Range("H1").Value = "mine"
Debug.Print TypeName(ws) & "|" & ws.Parent.Name & "|" & ActiveWorkbook.Name & "|" & ws.Range("F1").Font.Bold
wb.Close False"#,
        &mut ui,
    );
    assert_eq!(ui.printed, ["Worksheet|Data.xlsx|Main.xlsm|True"]);
    for a in ["A2", "A3", "E1", "F1", "F2", "G1"] {
        assert_eq!(cell(&r.main, a), "", "{a}");
    }
    assert_eq!(cell(&r.main, "H1"), "mine");
    assert!(ui.saved.is_empty());
}

#[test]
fn objects_dont_go_to_another_workbook() {
    let (root, mut ui) = setup("foreign", &["Data.xlsx", "Other.xlsx"]);
    let cases = [
        // A range of the macro's workbook as an argument of the other's.
        r#"Set wb = Workbooks.Open("Data.xlsx")
wb.Sheets(1).Range(ThisWorkbook.Sheets(1).Range("A1")).Value = 1"#,
        r#"Set wb = Workbooks.Open("Data.xlsx")
Set o = Workbooks.Open("Other.xlsx")
wb.Sheets(1).Range("A1:B2").Sort Key1:=o.Sheets(1).Range("A1")"#,
        r#"Set wb = Workbooks.Open("Data.xlsx")
ThisWorkbook.Sheets(1).Range(wb.Sheets(1).Range("A1"), wb.Sheets(1).Range("A2")).Value = 1"#,
        r#"Set wb = Workbooks.Open("Data.xlsx")
Set ws = ThisWorkbook.Sheets(1)
wb.Sheets(1).Copy After:=ws"#,
        r#"Set wb = Workbooks.Open("Data.xlsx")
Set r = Application.Union(wb.Sheets(1).Range("A1"), Range("A2"))
r.Value = 1"#,
        r#"Set wb = Workbooks.Open("Data.xlsx")
Set r = wb.Sheets(1).Range("A1")
wb.Close False
r.Value = 1"#,
    ];
    for (i, body) in cases.iter().enumerate() {
        let r = run(&root, &format!("Dim wb, o, ws, r\n{body}"), &mut ui);
        assert!(r.error.is_some(), "case {i} ran");
        assert_eq!(cell(&r.main, "A1"), "main", "case {i}");
        assert_eq!(cell(&r.main, "A2"), "", "case {i}");
        for b in &r.books {
            assert_eq!(cell(&b.wb, "A1"), b.name, "case {i}");
        }
    }
    assert!(ui.saved.is_empty());
}

#[test]
fn copy_and_paste_between_workbooks() {
    let (root, mut ui) = setup("copy", &["Data.xlsx"]);
    let r = ok(
        &root,
        r#"Dim wb As Workbook
ThisWorkbook.Sheets(1).Range("B5").Value = 3
ThisWorkbook.Sheets(1).Range("C5").Formula = "=B5*2"
ThisWorkbook.Sheets(1).Range("B5").Font.Bold = True
Set wb = Workbooks.Open("Data.xlsx")
wb.Sheets(1).Range("A1:B1").Copy ThisWorkbook.Sheets(1).Range("A3")
ThisWorkbook.Sheets(1).Range("B5:C5").Copy Destination:=wb.Sheets(1).Range("B5")
wb.Sheets(1).Range("A1").Copy
ThisWorkbook.Sheets(1).Range("A7").PasteSpecial xlPasteValues
ThisWorkbook.Sheets(1).Range("B5").Copy
wb.Sheets(1).Range("A9").PasteSpecial
Debug.Print wb.Sheets(1).Range("C5").Value & "|" & wb.Sheets(1).Range("C5").Formula & "|" & wb.Sheets(1).Range("B5").Font.Bold & "|" & wb.Sheets(1).Range("A9").Font.Bold
wb.Close True"#,
        &mut ui,
    );
    assert_eq!(ui.printed, ["6|=B5*2|True|True"]);
    assert_eq!(cell(&r.main, "A3"), "Data.xlsx");
    assert_eq!(cell(&r.main, "B3"), "2");
    assert_eq!(cell(&r.main, "A7"), "Data.xlsx");
    assert_eq!(cell(&r.main, "A1"), "main");
    let saved = &ui.saved[0].1;
    assert_eq!(cell(saved, "B5"), "3");
    assert_eq!(cell(saved, "A9"), "3");
    assert_eq!(cell(saved, "A1"), "Data.xlsx");
}

#[test]
fn the_workbooks_collection() {
    let (root, mut ui) = setup("collection", &["A.xlsx", "B.xlsx"]);
    let r = ok(
        &root,
        r#"Dim w
Workbooks.Open "A.xlsx"
Workbooks.Open "B.xlsx"
Workbooks.Open "A.xlsx"
For Each w In Workbooks
    Debug.Print w.Name & "|" & w.Sheets(1).Range("A1").Value
Next
Debug.Print Workbooks(1).Name & "|" & Workbooks(3).Name & "|" & ActiveWorkbook.Name & "|" & Application.Workbooks.Count
Workbooks("A.xlsx").Activate
Debug.Print ActiveSheet.Range("A1").Value
Set w = Workbooks.Add
w.Sheets(1).Range("A1").Value = "new"
Debug.Print w.Name & "|" & ActiveWorkbook.Name & "|" & Workbooks.Count
Workbooks("B.xlsx").Close False"#,
        &mut ui,
    );
    assert_eq!(
        ui.printed,
        ["Main.xlsm|main", "A.xlsx|A.xlsx", "B.xlsx|B.xlsx", "Main.xlsm|B.xlsx|A.xlsx|3", "A.xlsx", "Book4|Book4|4"]
    );
    // Those left open go to the application.
    let names: Vec<&str> = r.books.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["A.xlsx", "Book4"]);
    assert_eq!(cell(&r.books[1].wb, "A1"), "new");
    assert!(r.books[0].path.as_deref().is_some_and(|p| p.ends_with("A.xlsx")));
    assert_eq!(r.books[1].path, None);
}

#[test]
fn saving_and_the_files_reached() {
    let (root, mut ui) = setup("saving", &["Data.xlsx"]);
    fs::write(root.join("outside.xlsx"), b"x").unwrap();
    ui.files.insert("outside.xlsx".into(), workbook());
    let fails = [
        // Outside the workbook's folder.
        format!(r#"Workbooks.Open "{}""#, root.join("outside.xlsx").display()),
        // Not there.
        r#"Workbooks.Open "Missing.xlsx""#.to_string(),
        // Over the macro's own workbook, or over another open one.
        r#"Set wb = Workbooks.Open("Data.xlsx")
wb.SaveAs ThisWorkbook.FullName"#
            .to_string(),
        r#"Set wb = Workbooks.Add
Set d = Workbooks.Open("Data.xlsx")
wb.SaveAs ThisWorkbook.Path & "/Data.xlsx""#
            .to_string(),
        // Never saved.
        r#"Set wb = Workbooks.Add
wb.Save"#
            .to_string(),
        // Macros don't write files that run.
        r#"Set wb = Workbooks.Add
wb.SaveAs ThisWorkbook.Path & "/x.exe""#
            .to_string(),
    ];
    let mut macros = workbook();
    macros.macros.project = Some(std::sync::Arc::new(vec![1, 2, 3]));
    assert!(macros.macros.has_code());
    fs::write(root.join("book").join("Code.xlsm"), b"x").unwrap();
    ui.files.insert("Code.xlsm".into(), macros);
    let r = run(&root, "Set wb = Workbooks.Open(\"Code.xlsm\")\nwb.SaveAs ThisWorkbook.Path & \"/Copy.xlsm\"", &mut ui);
    assert!(r.error.as_ref().is_some_and(|e| e.1.contains("another workbook's macros")), "{:?}", r.error);
    for (i, body) in fails.iter().enumerate() {
        let r = run(&root, &format!("Dim wb, d\n{body}"), &mut ui);
        assert!(r.error.is_some(), "case {i} ran");
    }
    assert!(ui.saved.is_empty());
    let r = ok(
        &root,
        r#"Dim wb
Set wb = Workbooks.Add
wb.Sheets(1).Range("A1").Value = "made"
wb.SaveAs ThisWorkbook.Path & "/Made.xlsx"
Debug.Print wb.Name & "|" & wb.Saved
wb.Sheets(1).Range("A2").Value = 1
wb.Save
wb.Close"#,
        &mut ui,
    );
    assert_eq!(ui.printed, ["Made.xlsx|True"]);
    assert_eq!(ui.saved.len(), 2);
    assert!(ui.saved[1].0.ends_with("Made.xlsx"));
    assert_eq!(cell(&ui.saved[1].1, "A2"), "1");
    assert!(r.books.is_empty());
}

#[test]
fn closing_unsaved_asks() {
    let (root, mut ui) = setup("ask", &["Data.xlsx"]);
    // No: closed without saving.
    ui.answers = vec![7];
    ok(&root, "Set wb = Workbooks.Open(\"Data.xlsx\")\nwb.Sheets(1).Range(\"A5\") = 1\nwb.Close\nDebug.Print Workbooks.Count", &mut ui);
    // Cancel: left open.
    ui.answers = vec![2];
    let r = ok(&root, "Set wb = Workbooks.Open(\"Data.xlsx\")\nwb.Sheets(1).Range(\"A5\") = 1\nwb.Close\nDebug.Print Workbooks.Count", &mut ui);
    assert_eq!(ui.printed, [" 1 ", " 2 "]);
    assert!(ui.saved.is_empty());
    assert_eq!(r.books.len(), 1);
    assert_eq!(cell(&r.books[0].wb, "A5"), "1");
}
