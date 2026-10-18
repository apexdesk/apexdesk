//! What a macro does, read from its code without running it: the sheets
//! and ranges it names, whether it writes or clears cells, deletes rows
//! or sheets, touches files, saves, prints or asks the user; with what the
//! procedures it calls do. A summary for macros received from others; it
//! can't see what code computes at run time (a sheet's name in a
//! variable), so it says what the code names.

use std::collections::BTreeSet;

use crate::lexer::{Tok, tokenize};
use crate::suspicious::Signal;

/// What a macro does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Explanation {
    /// Sheets named (`Sheets("Data")`), in order, each once.
    pub sheets: Vec<String>,
    /// Ranges named (`Range("A1:B5")`), in order, each once.
    pub ranges: Vec<String>,
    /// It uses the active sheet or the selection (where it acts depends
    /// on what is selected when it runs).
    pub uses_selection: bool,
    pub writes_cells: bool,
    pub formats: bool,
    /// Clears contents (`ClearContents`, `Clear`).
    pub clears: bool,
    /// Deletes rows, columns or cells.
    pub deletes_cells: bool,
    pub adds_sheets: bool,
    pub deletes_sheets: bool,
    pub reads_files: bool,
    pub writes_files: bool,
    pub deletes_files: bool,
    pub saves: bool,
    pub prints: bool,
    /// MsgBox, InputBox, dialogs.
    pub asks_user: bool,
    /// Loops (`For`, `Do`, `While`).
    pub loops: bool,
    /// Programs it creates (`CreateObject("Outlook.Application")`).
    pub objects: Vec<String>,
    /// Procedures of the project it calls.
    pub calls: Vec<String>,
    /// What [`crate::suspicious`] finds in its code and the code it calls.
    pub signals: Vec<Signal>,
    /// The procedure wasn't found.
    pub not_found: bool,
}

/// At most this many sheets and ranges are listed.
const LIST: usize = 12;

/// A procedure's tokens.
struct Proc {
    module: String,
    name: String,
    tokens: Vec<Tok>,
    text: String,
}

/// The procedures of the modules (name, code), as tokens.
fn procs(modules: &[(&str, &str)]) -> Vec<Proc> {
    let mut out = Vec::new();
    for (module, code) in modules {
        let Ok(tokens) = tokenize(code, module) else { continue };
        let lines: Vec<&str> = code.lines().collect();
        let mut cur: Option<(String, Vec<Tok>, u32)> = None;
        let mut start_of_statement = true;
        let mut prev_word = String::new();
        for t in tokens {
            let line = t.line;
            let tok = t.tok;
            if let Some((_, body, _)) = cur.as_mut() {
                body.push(tok.clone());
            }
            match &tok {
                Tok::Word(w) => {
                    let w = w.to_ascii_lowercase();
                    let opens = matches!(w.as_str(), "sub" | "function" | "property") && start_of_statement && !matches!(prev_word.as_str(), "end" | "exit" | "declare");
                    if opens && cur.is_none() {
                        cur = Some((String::new(), Vec::new(), line));
                    } else if let Some((name, _, _)) = cur.as_mut().filter(|c| c.0.is_empty() && !matches!(w.as_str(), "get" | "let" | "set" | "sub" | "function" | "property")) {
                        *name = match &tok {
                            Tok::Word(n) => n.clone(),
                            _ => unreachable!(),
                        };
                    }
                    if matches!(w.as_str(), "sub" | "function" | "property") && prev_word == "end" {
                        if let Some((name, body, from)) = cur.take() {
                            let text = lines.get(from.saturating_sub(1) as usize..(line as usize).min(lines.len())).map(|l| l.join("\n")).unwrap_or_default();
                            out.push(Proc { module: module.to_string(), name, tokens: body, text });
                        }
                    }
                    // Modifiers keep the statement's start.
                    start_of_statement = start_of_statement && matches!(w.as_str(), "public" | "private" | "friend" | "static" | "end" | "exit" | "declare");
                    prev_word = w;
                }
                Tok::Eol | Tok::Colon => {
                    start_of_statement = true;
                    prev_word.clear();
                }
                _ => {
                    start_of_statement = false;
                    prev_word.clear();
                }
            }
        }
    }
    out
}

/// What macro `name` (`Macro1` or `Sheet1.Macro1`) of the modules
/// (name, code) does, the procedures it calls included.
pub fn explain(modules: &[(&str, &str)], name: &str) -> Explanation {
    let all = procs(modules);
    let (module, proc) = match name.rsplit_once('.') {
        Some((m, p)) => (Some(m), p),
        None => (None, name),
    };
    let find = |m: Option<&str>, p: &str| all.iter().position(|x| x.name.eq_ignore_ascii_case(p) && m.is_none_or(|m| x.module.eq_ignore_ascii_case(m)));
    let mut e = Explanation::default();
    let Some(first) = find(module, proc) else {
        e.not_found = true;
        return e;
    };
    let mut todo = vec![first];
    let mut seen = BTreeSet::from([first]);
    let mut texts = Vec::new();
    while let Some(i) = todo.pop() {
        let p = &all[i];
        texts.push((p.module.as_str(), p.text.as_str()));
        read(&p.tokens, &mut e);
        for t in &p.tokens {
            let Tok::Word(w) = t else { continue };
            // Calls of the project's own procedures (not of a member).
            if let Some(j) = find(None, w)
                && seen.insert(j)
            {
                if e.calls.len() < LIST {
                    e.calls.push(all[j].name.clone());
                }
                todo.push(j);
            }
        }
    }
    e.signals = crate::suspicious::scan(texts).into_iter().filter(|s| s.serious()).collect();
    e
}

fn push(list: &mut Vec<String>, s: &str) {
    if list.len() < LIST && !list.iter().any(|x| x.eq_ignore_ascii_case(s)) {
        list.push(s.to_string());
    }
}

/// What the tokens of one procedure do, into `e`.
fn read(tokens: &[Tok], e: &mut Explanation) {
    let word = |i: usize| match tokens.get(i) {
        Some(Tok::Word(w)) => Some(w.to_ascii_lowercase()),
        _ => None,
    };
    let sym = |i: usize, s: &str| matches!(tokens.get(i), Some(Tok::Sym(x)) if *x == s);
    let string_arg = |i: usize| match (sym(i + 1, "("), tokens.get(i + 2), sym(i + 3, ")")) {
        (true, Some(Tok::Str(s)), true) => Some(s.clone()),
        _ => None,
    };
    for i in 0..tokens.len() {
        let Some(w) = word(i) else { continue };
        let member = i > 0 && sym(i - 1, ".");
        // The member's object, for `Sheets.Add`.
        let owner = if member { i.checked_sub(2).and_then(word) } else { None };
        match w.as_str() {
            "sheets" | "worksheets" if !member || owner.as_deref() == Some("thisworkbook") || owner.as_deref() == Some("activeworkbook") => {
                if let Some(s) = string_arg(i) {
                    push(&mut e.sheets, &s);
                }
            }
            "range" => {
                if let Some(s) = string_arg(i) {
                    push(&mut e.ranges, &s);
                }
            }
            "activesheet" | "selection" | "activecell" => e.uses_selection = true,
            "value" | "value2" | "formula" | "formular1c1" | "formula2" | "text" if member && sym(i + 1, "=") && assigns(tokens, i) => e.writes_cells = true,
            "font" | "interior" | "numberformat" | "borders" | "horizontalalignment" | "columnwidth" | "rowheight" | "autofit" | "style" | "formatconditions" if member => e.formats = true,
            "clearcontents" | "clear" if member => e.clears = true,
            "delete" if member => {
                if matches!(owner.as_deref(), Some("sheets" | "worksheets" | "activesheet")) || is_sheet_call(tokens, i) {
                    e.deletes_sheets = true;
                } else {
                    e.deletes_cells = true;
                }
            }
            "add" if member && matches!(owner.as_deref(), Some("sheets" | "worksheets")) => e.adds_sheets = true,
            "copy" if member && matches!(owner.as_deref(), Some("activesheet")) => e.adds_sheets = true,
            "save" | "saveas" | "savecopyas" if member => e.saves = true,
            "printout" | "printpreview" if member => e.prints = true,
            "msgbox" | "inputbox" | "getopenfilename" | "getsaveasfilename" | "filedialog" => e.asks_user = true,
            "for" | "do" | "while" => e.loops = true,
            "open" if !member => {
                // `Open path For Output|Append|Binary` writes; Input reads.
                let mode = tokens[i..].iter().skip_while(|t| !matches!(t, Tok::Word(w) if w.eq_ignore_ascii_case("for"))).nth(1);
                match mode {
                    Some(Tok::Word(m)) if m.eq_ignore_ascii_case("input") => e.reads_files = true,
                    Some(Tok::Word(_)) => e.writes_files = true,
                    _ => {}
                }
            }
            "open" if matches!(owner.as_deref(), Some("workbooks")) => e.reads_files = true,
            "dir" | "opentextfile" | "readall" => e.reads_files = true,
            "kill" | "rmdir" | "deletefile" | "deletefolder" => e.deletes_files = true,
            "filecopy" | "mkdir" | "copyfile" | "movefile" | "createtextfile" | "createfolder" => e.writes_files = true,
            "name" if !member && tokens[i..].iter().take_while(|t| !matches!(t, Tok::Eol | Tok::Colon)).any(|t| matches!(t, Tok::Word(w) if w.eq_ignore_ascii_case("as"))) => e.writes_files = true,
            "createobject" | "getobject" => {
                if let Some(s) = string_arg(i) {
                    push(&mut e.objects, &s);
                }
            }
            _ => {}
        }
    }
}

/// The `=` after token `at` assigns (`Range("A1").Value = 1`, also
/// after `Then`), it doesn't compare (`If Range("A1").Value = 1 Then`).
fn assigns(tokens: &[Tok], at: usize) -> bool {
    for t in tokens[..at].iter().rev() {
        match t {
            Tok::Eol | Tok::Colon => return true,
            Tok::Word(w) if w.eq_ignore_ascii_case("then") || w.eq_ignore_ascii_case("else") => return true,
            Tok::Word(w) if ["if", "elseif", "while", "until", "do", "loop", "case", "for", "to", "and", "or", "not", "print"].iter().any(|k| w.eq_ignore_ascii_case(k)) => return false,
            Tok::Sym(s) if !matches!(*s, "." | "(" | ")" | "," | "!") => return false,
            _ => {}
        }
    }
    true
}

/// `Sheets("x").Delete`: the call before `.Delete` is on a sheet.
fn is_sheet_call(tokens: &[Tok], delete: usize) -> bool {
    // Back over `( ... )` to the word before it.
    let mut i = delete.saturating_sub(1);
    if i == 0 || !matches!(tokens.get(i - 1), Some(Tok::Sym(")"))) {
        return false;
    }
    i -= 1;
    let mut depth = 0;
    loop {
        match tokens.get(i) {
            Some(Tok::Sym(")")) => depth += 1,
            Some(Tok::Sym("(")) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            None => return false,
            _ => {}
        }
        if i == 0 {
            return false;
        }
        i -= 1;
    }
    matches!(i.checked_sub(1).and_then(|j| tokens.get(j)), Some(Tok::Word(w)) if w.eq_ignore_ascii_case("sheets") || w.eq_ignore_ascii_case("worksheets"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_it_does() {
        let m1 = "Sub Tidy()\n    Sheets(\"Data\").Range(\"A2:C100\").ClearContents\n    Range(\"B1\").Value = \"Total\"\n    Range(\"B1\").Font.Bold = True\n    Helper\n    Sheets(\"Old\").Delete\nEnd Sub\n\nPrivate Sub Helper()\n    Dim i As Long\n    For i = 1 To 10\n        Rows(i).Delete\n    Next\n    Open \"C:\\out.txt\" For Output As #1\n    Close #1\n    ActiveWorkbook.Save\n    MsgBox \"Done\"\nEnd Sub\n";
        let m2 = "Sub Other()\n    Kill \"x\"\nEnd Sub\n";
        let e = explain(&[("Module1", m1), ("Module2", m2)], "Tidy");
        assert_eq!(e.sheets, ["Data", "Old"]);
        assert_eq!(e.ranges, ["A2:C100", "B1"]);
        assert_eq!(e.calls, ["Helper"]);
        assert!(e.clears && e.writes_cells && e.formats && e.deletes_sheets && e.deletes_cells && e.loops && e.writes_files && e.saves && e.asks_user);
        // Other's code isn't called.
        assert!(!e.deletes_files && !e.reads_files && !e.uses_selection && e.signals.is_empty());
        assert!(explain(&[("Module1", m1)], "Nope").not_found);
    }

    #[test]
    fn named_by_module_and_signals() {
        let doc = "Private Sub Workbook_Open()\nEnd Sub\nPublic Sub Go()\n    Selection.Value = 1\n    Set o = CreateObject(\"Outlook.Application\")\n    Shell \"calc\"\nEnd Sub\n";
        let e = explain(&[("ThisWorkbook", doc)], "ThisWorkbook.Go");
        assert!(e.uses_selection && e.writes_cells);
        assert_eq!(e.objects, ["Outlook.Application"]);
        assert_eq!(e.signals, [Signal::RunsPrograms]);
        // Recursion is followed once.
        let e = explain(&[("M", "Sub A()\n    B\nEnd Sub\nSub B()\n    A\n    Workbooks.Open \"f.xlsx\"\nEnd Sub\n")], "A");
        assert_eq!(e.calls, ["B"]);
        assert!(!e.writes_cells && e.reads_files);
        // Comparing isn't writing; after Then it is.
        let e = explain(&[("M", "Sub A()\n    If Range(\"A1\").Value = 1 Then MsgBox 1\nEnd Sub\n")], "A");
        assert!(!e.writes_cells);
        let e = explain(&[("M", "Sub A()\n    If x Then Range(\"A1\").Value = 1\nEnd Sub\n")], "A");
        assert!(e.writes_cells);
    }
}
