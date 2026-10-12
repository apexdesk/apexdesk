//! Helpers of the interpreter's tests: a workbook, a module and a Ui that
//! answers dialogs from a script and records what the macro showed.

#![allow(dead_code)]

use std::collections::VecDeque;

use apex_sheet::{CellRef, Workbook, format::Locale};
use apex_vba::{ModuleKind, Options, Outcome, Project, Source, Ui};

#[derive(Default)]
pub struct TestUi {
    pub answers: VecDeque<i32>,
    pub inputs: VecDeque<Option<String>>,
    /// MsgBox prompts, then InputBox prompts as "? prompt".
    pub shown: Vec<String>,
    pub printed: Vec<String>,
}

impl Ui for TestUi {
    fn msg_box(&mut self, prompt: &str, _buttons: i32, _title: &str) -> i32 {
        self.shown.push(prompt.to_string());
        self.answers.pop_front().unwrap_or(1)
    }

    fn input_box(&mut self, prompt: &str, _title: &str, default: &str) -> Option<String> {
        self.shown.push(format!("? {prompt}"));
        self.inputs.pop_front().unwrap_or(Some(default.to_string()))
    }

    fn debug_print(&mut self, text: &str) {
        self.printed.push(text.to_string());
    }
}

pub fn workbook() -> Workbook {
    let mut wb = Workbook::new();
    wb.locale = Locale::EN;
    wb
}

pub fn project(code: &str) -> Project {
    Project::parse(&[Source { name: "Module1".into(), kind: ModuleKind::Standard, code: code.into() }]).unwrap_or_else(|e| panic!("{} line {}: {e}", e.module, e.line))
}

/// Runs `Main` of `code` on `wb`.
pub fn run_on(wb: &mut Workbook, code: &str, ui: &mut TestUi) -> Outcome {
    let p = project(code);
    apex_vba::run_macro(&p, wb, ui, &Options::default(), "Main")
}

/// Runs `Main` and returns what it printed (Debug.Print), one line each.
pub fn printed(code: &str) -> Vec<String> {
    let mut wb = workbook();
    let mut ui = TestUi::default();
    let out = run_on(&mut wb, code, &mut ui);
    if let Some(e) = out.error {
        panic!("{} line {}: {e}", e.module, e.line);
    }
    ui.printed
}

/// Runs `Sub Main` made of `body` and returns its Debug.Print lines.
pub fn body(body: &str) -> Vec<String> {
    printed(&format!("Sub Main()\n{body}\nEnd Sub\n"))
}

/// The error of running `Main` made of `body`.
pub fn error(body: &str) -> apex_vba::Error {
    let mut wb = workbook();
    let mut ui = TestUi::default();
    run_on(&mut wb, &format!("Sub Main()\n{body}\nEnd Sub\n"), &mut ui).error.expect("an error")
}

pub fn text(wb: &Workbook, sheet: usize, a1: &str) -> String {
    wb.display(sheet, CellRef::parse(a1).unwrap()).0
}
