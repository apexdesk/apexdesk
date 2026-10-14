//! Files a macro reads and writes, and how far it reaches: the workbook's
//! folder, what the user picks, and asking before it changes anything.

mod common;

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use apex_vba::files::{BookFormat, FileAccess, FilePick, PickKind};
use apex_vba::{Options, Ui};
use common::*;

/// Answers the questions about files and records them.
#[derive(Default)]
struct FileUi {
    /// The answer to "may the macro change files?".
    allow: bool,
    asked: Vec<String>,
    picks: VecDeque<Option<Vec<String>>>,
    shown: Vec<FilePick>,
    printed: Vec<String>,
    saved: Vec<(String, BookFormat, usize)>,
}

impl Ui for FileUi {
    fn msg_box(&mut self, _prompt: &str, _buttons: i32, _title: &str) -> i32 {
        1
    }

    fn input_box(&mut self, _prompt: &str, _title: &str, _default: &str) -> Option<String> {
        None
    }

    fn debug_print(&mut self, text: &str) {
        self.printed.push(text.to_string());
    }

    fn allow_file_changes(&mut self, folder: &str) -> bool {
        self.asked.push(folder.to_string());
        self.allow
    }

    fn pick_files(&mut self, pick: &FilePick) -> Option<Vec<String>> {
        self.shown.push(pick.clone());
        self.picks.pop_front().flatten()
    }

    fn save_workbook(&mut self, wb: &apex_sheet::Workbook, path: &str, format: BookFormat, sheet: usize) -> Result<(), String> {
        self.saved.push((path.to_string(), format, sheet));
        fs::write(path, wb.sheets[sheet].name.as_bytes()).map_err(|e| e.to_string())
    }
}

/// A clean folder for a test, with the workbook's folder `book` in it.
fn folder(test: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("apex-vba-files").join(test);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("book")).unwrap();
    root
}

fn run(root: &Path, body: &str, ui: &mut FileUi) -> Result<(), (i32, String)> {
    let mut wb = workbook();
    let p = project(&format!("Sub Main()\n{body}\nEnd Sub\n"));
    let opts = Options { file_path: root.join("book").display().to_string(), file_name: "Book1.xlsx".into(), ..Options::default() };
    let out = apex_vba::run_macro(&p, &mut wb, ui, &opts, "Main");
    match out.error {
        Some(e) => Err((e.number, format!("line {}: {e}", e.line))),
        None => Ok(()),
    }
}

fn ok(root: &Path, body: &str, ui: &mut FileUi) {
    if let Err((_, e)) = run(root, body, ui) {
        panic!("{e}");
    }
}

fn number(root: &Path, body: &str, ui: &mut FileUi) -> i32 {
    run(root, body, ui).expect_err("an error").0
}

#[test]
fn write_and_read_text() {
    let root = folder("text");
    let mut ui = FileUi { allow: true, ..Default::default() };
    ok(
        &root,
        r#"Dim f As Integer, s As String, a, b, c
f = FreeFile
Open ThisWorkbook.Path & "\out.txt" For Output As #f
Print #f, "a"; 1; -2
Print #f, "x", "y"
Print #f, "t"; Tab(5); "u"; Spc(2); "v"
Write #f, "q", 3.5, True
Write #f, #2024-01-31#, Null
Close #f
Open ThisWorkbook.Path & "\out.txt" For Append As #f
Print #f, "más €"
Close
Open "out.txt" For Input As #f
Line Input #f, s: Debug.Print s
Line Input #f, s: Debug.Print s & "|"
Line Input #f, s: Debug.Print s
Input #f, a, b, c
Debug.Print a & "|" & b & "|" & c & "|" & TypeName(b) & "|" & EOF(f)
Input #f, a, b
Debug.Print TypeName(a) & "|" & IsNull(b)
Line Input #f, s: Debug.Print s & "|" & EOF(f) & "|" & LOF(f)
Close f"#,
        &mut ui,
    );
    let bytes = fs::read(root.join("book/out.txt")).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text, "a 1 -2 \r\nx             y\r\nt   u  v\r\n\"q\",3.5,#TRUE#\r\n#2024-01-31#,#NULL#\r\nmás €\r\n");
    assert_eq!(ui.printed, vec!["a 1 -2 ", "x             y|", "t   u  v", "q|3.5|True|Double|False", "Date|True", &format!("más €|True|{}", text.len())]);
    // Reading and writing in the workbook's folder: asked once, about it.
    assert_eq!(ui.asked.len(), 1);
    assert!(ui.asked[0].ends_with("book"), "{:?}", ui.asked);
}

#[test]
fn reading_needs_no_question() {
    let root = folder("read");
    fs::write(root.join("book/in.csv"), b"caf\xE9,1\n").unwrap();
    fs::write(root.join("book/abc.txt"), "abcdef").unwrap();
    let mut ui = FileUi::default();
    ok(&root, "Dim s As String\nOpen \"in.csv\" For Input As 1\nLine Input #1, s\nClose 1\nDebug.Print s", &mut ui);
    assert_eq!(ui.printed, vec!["café,1"]);
    assert!(ui.asked.is_empty());
    // Errors as VBA gives them.
    assert_eq!(number(&root, "Open \"none.txt\" For Input As 1", &mut ui), 53);
    assert_eq!(number(&root, "Dim s\nOpen \"in.csv\" For Input As 1\nLine Input #1, s\nLine Input #1, s", &mut ui), 62);
    assert_eq!(number(&root, "Open \"in.csv\" For Input As 1\nOpen \"in.csv\" For Input As 1", &mut ui), 55);
    assert_eq!(number(&root, "Open \"in.csv\" For Input As 1\nPrint #1, 2", &mut ui), 54);
    assert_eq!(number(&root, "Print #3, 2", &mut ui), 52);
    assert_eq!(number(&root, "Open \"in.csv\" For Input As 600", &mut ui), 52);
    assert_eq!(number(&root, "Debug.Print EOF(1)", &mut ui), 52);
    // Input(n, #f) and Seek.
    ui.printed.clear();
    ok(&root, "Open \"abc.txt\" For Input As #1\nDebug.Print Input(4, #1) & \"|\" & Seek(1)\nSeek #1, 2\nDebug.Print Input(2, 1)\nClose", &mut ui);
    assert_eq!(ui.printed, vec!["abcd|5", "bc"]);
}

#[test]
fn changing_files_is_asked() {
    let root = folder("ask");
    let mut ui = FileUi::default();
    // Refused: nothing is written, every change fails without asking again.
    assert_eq!(number(&root, "On Error Resume Next\nOpen \"a.txt\" For Output As 1\nDebug.Print Err.Number\nErr.Clear\nMkDir \"sub\"\nDebug.Print Err.Number\nOn Error GoTo 0\nKill \"x.txt\"", &mut ui), 70);
    assert_eq!(ui.printed, vec![" 70 ", " 70 "]);
    assert_eq!(ui.asked.len(), 1);
    assert!(!root.join("book/a.txt").exists() && !root.join("book/sub").exists());
    // Allowed by the options: no question.
    let mut wb = workbook();
    let p = project("Sub Main()\nOpen \"a.txt\" For Output As 1\nPrint #1, 1\nClose\nEnd Sub\n");
    let opts = Options { file_path: root.join("book").display().to_string(), files: FileAccess { trusted: false, write: Some(true) }, ..Options::default() };
    let mut ui = FileUi::default();
    assert!(apex_vba::run_macro(&p, &mut wb, &mut ui, &opts, "Main").error.is_none());
    assert!(ui.asked.is_empty() && root.join("book/a.txt").exists());
}

#[test]
fn folders_and_names() {
    let root = folder("dir");
    for f in ["b.txt", "a.txt", "c.csv", ".hidden.txt"] {
        fs::write(root.join("book").join(f), "12345").unwrap();
    }
    let mut ui = FileUi { allow: true, ..Default::default() };
    ok(
        &root,
        r#"Dim f
f = Dir(ThisWorkbook.Path & "/*.TXT")
Do While f <> ""
    Debug.Print f
    f = Dir()
Loop
Debug.Print Dir("nothing*") = ""
MkDir "sub"
Debug.Print Dir("su*", vbDirectory) & "|" & (GetAttr("sub") And vbDirectory) & "|" & FileLen("a.txt")
FileCopy "a.txt", "sub\copy.txt"
Name "b.txt" As "sub\moved.txt"
ChDir "sub"
Debug.Print Dir("*") & "|" & Dir() & "|" & Dir() & "|" & (CurDir() Like "*sub")
Kill "*.txt"
ChDir ".."
RmDir "sub"
Debug.Print Dir("sub", vbDirectory) = "" And Year(FileDateTime("c.csv")) > 2000"#,
        &mut ui,
    );
    assert_eq!(ui.printed, vec!["a.txt", "b.txt", "True", "sub|16|5", "copy.txt|moved.txt||True", "True"]);
    assert!(!root.join("book/b.txt").exists() && !root.join("book/sub").exists());
    assert_eq!(number(&root, "Kill \"none*.txt\"", &mut ui), 53);
    assert_eq!(number(&root, "Name \"a.txt\" As \"c.csv\"", &mut ui), 58);
    assert_eq!(number(&root, "MkDir \"x\\y\\z\"", &mut ui), 76);
    assert_eq!(number(&root, "Debug.Print Dir()", &mut ui), 5);
}

#[test]
fn the_folder_is_the_limit() {
    let root = folder("limit");
    fs::write(root.join("outside.txt"), "secret").unwrap();
    fs::create_dir_all(root.join("book/.git")).unwrap();
    fs::write(root.join("book/.git/config"), "x").unwrap();
    fs::write(root.join("book/run.sh"), "x").unwrap();
    let mut ui = FileUi { allow: true, ..Default::default() };
    let outside = root.join("outside.txt").display().to_string();
    for code in [
        format!("Open \"{outside}\" For Input As 1"),
        "Open \"..\\outside.txt\" For Input As 1".into(),
        "Open \"../book/../outside.txt\" For Input As 1".into(),
        "Debug.Print FileLen(\"/etc/passwd\")".into(),
        "Debug.Print Dir(\"/etc/*\")".into(),
        "ChDir \"..\"".into(),
        "Open \".git/config\" For Input As 1".into(),
        "Open \".bashrc\" For Append As 1".into(),
        "Open \"run.sh\" For Output As 1".into(),
        "Open \"x.desktop\" For Output As 1".into(),
        "Open \"evil.xlsm\" For Binary As 1".into(),
        "Open \"evil.EXE.\" For Output As 1".into(),
        "Name \"run.sh\" As \"run.txt\"".into(),
        "FileCopy \"run.sh\", \"x.vbs\"".into(),
        "Open \"Book1.xlsx\" For Output As 1".into(),
        "Kill \"../outside.txt\"".into(),
        format!("Name \"run.sh\" As \"{}\"", root.join("moved.sh").display()),
    ] {
        assert_eq!(number(&root, &code, &mut ui), 70, "{code}");
    }
    assert_eq!(fs::read_to_string(root.join("outside.txt")).unwrap(), "secret");
    assert!(root.join("book/run.sh").exists());
    // A link out of the folder leads nowhere.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("outside.txt"), root.join("book/link.txt")).unwrap();
        std::os::unix::fs::symlink(root.join("made.txt"), root.join("book/broken.txt")).unwrap();
        assert_eq!(number(&root, "Open \"link.txt\" For Input As 1", &mut ui), 70);
        assert_eq!(number(&root, "Open \"broken.txt\" For Output As 1", &mut ui), 75);
        assert!(!root.join("made.txt").exists());
        // Nor are programs changed.
        use std::os::unix::fs::PermissionsExt;
        fs::write(root.join("book/tool"), "#!/bin/sh").unwrap();
        fs::set_permissions(root.join("book/tool"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(number(&root, "Open \"tool\" For Append As 1", &mut ui), 70);
        assert_eq!(fs::read_to_string(root.join("book/tool")).unwrap(), "#!/bin/sh");
    }
    // A workbook never saved reaches nothing.
    let mut wb = workbook();
    let p = project("Sub Main()\nOpen \"a.txt\" For Output As 1\nEnd Sub\n");
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &Options::default(), "Main");
    assert_eq!(out.error.map(|e| e.number), Some(70));
}

#[test]
fn picked_files_are_reachable() {
    let root = folder("pick");
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/in.txt"), "hello").unwrap();
    fs::write(root.join("data/other.txt"), "x").unwrap();
    let picked = root.join("data/in.txt").display().to_string();
    let save = root.join("data/out.txt").display().to_string();
    let mut ui = FileUi::default();
    ui.picks.push_back(Some(vec![picked.clone()]));
    ui.picks.push_back(Some(vec![save.clone()]));
    ui.picks.push_back(None);
    ok(
        &root,
        r#"Dim f, s As String
f = Application.GetOpenFilename("Text (*.txt),*.txt,All (*.*),*.*", 2, "Pick one")
Open f For Input As 1
Line Input #1, s
Close 1
Debug.Print s & "|" & (Dir(f) = "in.txt")
f = Application.GetSaveAsFilename("out.txt", "Text (*.txt),*.txt")
Open f For Output As 1
Print #1, s
Close 1
Debug.Print Application.GetOpenFilename() = False"#,
        &mut ui,
    );
    assert_eq!(ui.printed, vec!["hello|True", "True"]);
    // Picking the file to save in is the permission to write it.
    assert!(ui.asked.is_empty());
    assert_eq!(fs::read_to_string(root.join("data/out.txt")).unwrap(), "hello\r\n");
    assert_eq!(ui.shown[0].kind, PickKind::Open);
    assert_eq!(ui.shown[0].title, "Pick one");
    assert_eq!(ui.shown[0].filters[0].0, "All (*.*)");
    assert_eq!((ui.shown[1].kind, ui.shown[1].initial.as_str()), (PickKind::Save, "out.txt"));
    // What wasn't picked stays out of reach.
    let mut ui = FileUi::default();
    ui.picks.push_back(Some(vec![picked.clone()]));
    let other = root.join("data/other.txt").display().to_string();
    let code = format!("Dim f\nf = Application.GetOpenFilename()\nOpen \"{other}\" For Input As 1");
    assert_eq!(number(&root, &code, &mut ui), 70);
    let code = format!("Dim f\nf = Application.GetOpenFilename()\nOpen f For Output As 1");
    assert_eq!(number(&root, &code, &mut ui), 70);
}

#[test]
fn file_dialog() {
    let root = folder("dialog");
    fs::create_dir_all(root.join("data")).unwrap();
    for f in ["a.txt", "b.txt"] {
        fs::write(root.join("data").join(f), f).unwrap();
    }
    let data = root.join("data").display().to_string();
    let mut ui = FileUi::default();
    ui.picks.push_back(Some(vec![data.clone()]));
    ui.picks.push_back(None);
    ok(
        &root,
        r#"Dim f, d, n
With Application.FileDialog(msoFileDialogFolderPicker)
    .Title = "Where"
    If .Show = -1 Then d = .SelectedItems(1)
    Debug.Print .SelectedItems.Count & "|" & .Title
End With
f = Dir(d & "\*.txt")
Do While f <> ""
    n = n + FileLen(d & "\" & f)
    f = Dir
Loop
Debug.Print n
With Application.FileDialog(msoFileDialogFilePicker)
    .Filters.Clear
    .Filters.Add "Text", "*.txt; *.csv"
    .AllowMultiSelect = True
    Debug.Print .Filters.Count & "|" & .Filters(1).Extensions & "|" & .Show
    For Each f In .SelectedItems
        Debug.Print f
    Next
End With"#,
        &mut ui,
    );
    assert_eq!(ui.printed, vec!["1|Where", " 10 ", "1|*.txt; *.csv|0"]);
    assert_eq!(ui.shown[0].kind, PickKind::Folder);
    assert!(ui.shown[1].multi);
    assert_eq!(ui.shown[1].filters, vec![("Text".to_string(), vec!["*.txt".to_string(), "*.csv".to_string()])]);
}

#[test]
fn binary_files() {
    let root = folder("binary");
    let mut ui = FileUi { allow: true, ..Default::default() };
    ok(
        &root,
        r#"Dim n As Long, x As Double, s As String, b() As Byte, k As Integer
Open "data.bin" For Binary As #1
n = 258: x = 1.5: s = "hey"
Put #1, , n
Put #1, , x
Put #1, , s
Debug.Print Loc(1) & "|" & LOF(1)
n = 0: x = 0: s = "   "
Get #1, 1, n
Get #1, , x
Get #1, , s
Debug.Print n & "|" & x & "|" & s & "|" & EOF(1)
Close #1
Open "data.bin" For Binary Access Read As #1
ReDim b(0 To LOF(1) - 1)
Get #1, , b
Close #1
Debug.Print b(0) & "|" & b(1) & "|" & b(12) & "|" & UBound(b)
Open "rec.dat" For Random As #2 Len = 8
k = 7: Put #2, 3, k
k = 0: Get #2, 3, k
Debug.Print k & "|" & LOF(2) & "|" & Seek(2)
Close"#,
        &mut ui,
    );
    assert_eq!(ui.printed, vec!["15|15", "258|1.5|hey|True", "2|1|104|14", "7|18|4"]);
    assert_eq!(&fs::read(root.join("book/data.bin")).unwrap()[..4], &258i32.to_le_bytes());
}

#[test]
fn file_system_object() {
    let root = folder("fso");
    let book = root.join("book");
    let mut ui = FileUi { allow: true, ..Default::default() };
    ok(
        &root,
        r#"Dim fso As Object, ts As Object, f, n As Long, s As String
Set fso = CreateObject("Scripting.FileSystemObject")
Debug.Print fso.GetFileName("C:\a\b.txt"); "|"; fso.GetBaseName("b.tar.gz"); "|"; fso.GetExtensionName("b.tar.gz"); "|"; fso.GetParentFolderName("C:\a\b.txt"); "|"; fso.BuildPath("C:\a", "b")
Set ts = fso.CreateTextFile(ThisWorkbook.Path & "\t.txt", True)
ts.WriteLine "one"
ts.Write "tw"
ts.Write "o"
Debug.Print ts.Line; ts.Column
ts.WriteBlankLines 1
ts.Close
Set ts = fso.OpenTextFile(ThisWorkbook.Path & "\t.txt", ForAppending)
ts.WriteLine "three"
ts.Close
Set ts = fso.OpenTextFile(ThisWorkbook.Path & "\t.txt", ForReading)
Do Until ts.AtEndOfStream
    s = s & ts.Line & "=" & ts.ReadLine & ";"
Loop
ts.Close
Debug.Print s
Debug.Print fso.FileExists(ThisWorkbook.Path & "\t.txt"); fso.FileExists(ThisWorkbook.Path & "\no.txt"); fso.FolderExists(ThisWorkbook.Path)
fso.CreateFolder ThisWorkbook.Path & "\sub"
fso.CopyFile ThisWorkbook.Path & "\*.txt", ThisWorkbook.Path & "\sub\"
fso.CopyFile ThisWorkbook.Path & "\t.txt", ThisWorkbook.Path & "\sub\u.txt"
For Each f In fso.GetFolder(ThisWorkbook.Path & "\sub").Files
    Debug.Print f.Name; f.Size
Next
Debug.Print fso.GetFolder(ThisWorkbook.Path).SubFolders.Count; fso.GetFolder(ThisWorkbook.Path).SubFolders("sub").Name
fso.CopyFolder ThisWorkbook.Path & "\sub", ThisWorkbook.Path & "\copy"
fso.MoveFile ThisWorkbook.Path & "\copy\u.txt", ThisWorkbook.Path & "\copy\v.txt"
fso.GetFile(ThisWorkbook.Path & "\copy\v.txt").Name = "w.txt"
Set ts = fso.GetFile(ThisWorkbook.Path & "\copy\w.txt").OpenAsTextStream(ForReading)
Debug.Print fso.FileExists(ThisWorkbook.Path & "\copy\w.txt"); ts.ReadAll = "one" & vbCrLf & "two" & vbCrLf & "three" & vbCrLf
ts.Close
fso.DeleteFile ThisWorkbook.Path & "\sub\*.txt"
fso.DeleteFolder ThisWorkbook.Path & "\sub"
fso.GetFolder(ThisWorkbook.Path & "\copy").Delete
Debug.Print fso.FolderExists(ThisWorkbook.Path & "\sub"); fso.FolderExists(ThisWorkbook.Path & "\copy")
On Error Resume Next
Set ts = fso.OpenTextFile(ThisWorkbook.Path & "\t.txt", ForReading)
ts.ReadAll
ts.ReadLine
Debug.Print Err.Number
Err.Clear
fso.CreateTextFile ThisWorkbook.Path & "\t.txt", False
Debug.Print Err.Number"#,
        &mut ui,
    );
    assert_eq!(
        ui.printed,
        ["b.txt|b.tar|gz|C:\\a|C:\\a\\b", " 2  4 ", "1=one;2=two;3=three;", "TrueFalseTrue", "t.txt 17 ", "u.txt 17 ", " 1 sub", "TrueTrue", "FalseFalse", " 62 ", " 58 "]
    );
    assert_eq!(ui.asked.len(), 1);
    assert_eq!(fs::read_to_string(book.join("t.txt")).unwrap(), "one\r\ntwo\r\nthree\r\n");
}

#[test]
fn file_system_object_keeps_to_the_folder() {
    let root = folder("fso-limit");
    let book = root.join("book");
    fs::write(root.join("outside.txt"), "x").unwrap();
    fs::create_dir_all(book.join("d/.git")).unwrap();
    fs::write(book.join("d/.git/config"), "x").unwrap();
    fs::write(book.join("Book1.xlsx"), "x").unwrap();
    let mut ui = FileUi { allow: true, ..Default::default() };
    let fso = "Dim fso As Object\nSet fso = CreateObject(\"Scripting.FileSystemObject\")\n";
    let up = root.join("outside.txt").display().to_string();
    // Outside the folder it doesn't exist and can't be reached.
    ok(&root, &format!("{fso}Debug.Print fso.FileExists(\"{up}\"); fso.FileExists(\"..\\outside.txt\")"), &mut ui);
    assert_eq!(ui.printed, ["FalseFalse"]);
    assert_eq!(number(&root, &format!("{fso}fso.GetFile \"{up}\""), &mut ui), 70);
    assert_eq!(number(&root, &format!("{fso}fso.DeleteFile \"{up}\""), &mut ui), 70);
    assert_eq!(number(&root, &format!("{fso}fso.CopyFile \"{up}\", ThisWorkbook.Path & \"\\\""), &mut ui), 70);
    assert_eq!(number(&root, &format!("{fso}fso.CopyFile ThisWorkbook.FullName, \"{}\"", root.join("c.txt").display()), &mut ui), 70);
    // Hidden files inside: the folder isn't deleted, nor moved, nor partly.
    assert_eq!(number(&root, &format!("{fso}fso.DeleteFolder ThisWorkbook.Path & \"\\d\""), &mut ui), 70);
    assert_eq!(number(&root, &format!("{fso}fso.MoveFolder ThisWorkbook.Path & \"\\d\", ThisWorkbook.Path & \"\\e\""), &mut ui), 70);
    assert_eq!(number(&root, "Name ThisWorkbook.Path & \"\\d\" As ThisWorkbook.Path & \"\\e\"", &mut ui), 70);
    assert!(book.join("d/.git/config").exists());
    // Not the workbook's folder, nor the workbook, nor scripts.
    assert_eq!(number(&root, &format!("{fso}fso.DeleteFolder ThisWorkbook.Path"), &mut ui), 75);
    assert_eq!(number(&root, &format!("{fso}fso.DeleteFile ThisWorkbook.FullName"), &mut ui), 70);
    assert_eq!(number(&root, &format!("{fso}fso.CreateTextFile ThisWorkbook.Path & \"\\run.sh\""), &mut ui), 70);
    assert_eq!(number(&root, &format!("{fso}fso.MoveFile ThisWorkbook.FullName, ThisWorkbook.Path & \"\\x.txt\""), &mut ui), 70);
    assert!(book.join("Book1.xlsx").exists() && root.join("outside.txt").exists());
    // Refused, nothing changes.
    let mut no = FileUi::default();
    assert_eq!(number(&root, &format!("{fso}fso.CreateFolder ThisWorkbook.Path & \"\\n\""), &mut no), 70);
    assert!(!book.join("n").exists());
}

#[test]
fn save_as_and_close() {
    let root = folder("saveas");
    let book = root.join("book");
    let mut ui = FileUi { allow: true, ..Default::default() };
    let mut wb = workbook();
    let p = project(
        r#"Sub Main()
ThisWorkbook.SaveCopyAs ThisWorkbook.Path & "\copy.xlsx"
Debug.Print ThisWorkbook.Name
Range("A1").Value = 1
ThisWorkbook.SaveAs ThisWorkbook.Path & "\new", xlOpenXMLWorkbookMacroEnabled
Debug.Print ThisWorkbook.Name; "|"; ThisWorkbook.Saved
On Error Resume Next
ThisWorkbook.SaveAs ThisWorkbook.Path & "\x.xlsx", 52
Debug.Print Err.Number
Err.Clear
ThisWorkbook.SaveCopyAs ThisWorkbook.Path & "\x.docm"
Debug.Print Err.Number
Err.Clear
ThisWorkbook.SaveCopyAs ThisWorkbook.Path & "\..\out.xlsx"
Debug.Print Err.Number
Err.Clear
ThisWorkbook.SaveAs ThisWorkbook.Path & "\list.csv"
Debug.Print Err.Number; ThisWorkbook.Name
On Error GoTo 0
ThisWorkbook.Close False
Debug.Print "not reached"
End Sub
"#,
    );
    let opts = Options { file_path: book.display().to_string(), file_name: "Book1.xlsm".into(), ..Options::default() };
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &opts, "Main");
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(ui.printed, ["Book1.xlsm", "new.xlsm|True", " 1004 ", " 1004 ", " 70 ", " 0 list.csv"]);
    let at = |n: &str| book.join(n).display().to_string();
    assert_eq!(ui.saved, [(at("copy.xlsx"), BookFormat::Xlsx, 0), (at("new.xlsm"), BookFormat::Xlsx, 0), (at("list.csv"), BookFormat::Csv, 0)]);
    assert_eq!(out.requests, [apex_vba::Request::SavedAs { path: at("list.csv"), clean: true }, apex_vba::Request::Close { save: Some(false) }]);
    assert!(!root.join("out.xlsx").exists());
    // Changes after it: not clean; refused: nothing written.
    let p = project("Sub Main()\nThisWorkbook.SaveAs ThisWorkbook.Path & \"\\b.xlsx\"\nRange(\"A1\").Value = 2\nEnd Sub\n");
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &opts, "Main");
    assert_eq!(out.requests, [apex_vba::Request::SavedAs { path: at("b.xlsx"), clean: false }]);
    let mut no = FileUi::default();
    let p = project("Sub Main()\nThisWorkbook.SaveCopyAs ThisWorkbook.Path & \"\\c.xlsx\"\nEnd Sub\n");
    let out = apex_vba::run_macro(&p, &mut wb, &mut no, &opts, "Main");
    assert_eq!(out.error.map(|e| e.number), Some(70));
    assert!(no.saved.is_empty() && !book.join("c.xlsx").exists());
}
