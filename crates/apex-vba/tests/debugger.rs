//! The debugger: breakpoints, stepping, watches, the Immediate window and
//! pausing at errors.

mod common;

use std::collections::VecDeque;

use apex_vba::debug::{Breakpoint, DebugCmd, Debugger, Pause, Reason, Watch, WatchKind};
use apex_vba::{Options, Ui};
use common::*;

/// Answers pauses from a script (then Continue) and records them.
#[derive(Default)]
struct DebugUi {
    cmds: VecDeque<DebugCmd>,
    pauses: Vec<Pause>,
    printed: Vec<String>,
}

impl Ui for DebugUi {
    fn msg_box(&mut self, _prompt: &str, _buttons: i32, _title: &str) -> i32 {
        1
    }

    fn input_box(&mut self, _prompt: &str, _title: &str, default: &str) -> Option<String> {
        Some(default.to_string())
    }

    fn debug_print(&mut self, text: &str) {
        self.printed.push(text.to_string());
    }

    fn pause(&mut self, at: &Pause, _wb: &apex_sheet::Workbook) -> DebugCmd {
        self.pauses.push(at.clone());
        self.cmds.pop_front().unwrap_or(DebugCmd::Continue)
    }
}

fn bp(line: u32) -> Breakpoint {
    Breakpoint { module: "Module1".into(), line, condition: None }
}

fn debug(code: &str, d: Debugger, cmds: Vec<DebugCmd>) -> (DebugUi, apex_vba::Outcome) {
    let p = project(code);
    let mut wb = workbook();
    let mut ui = DebugUi { cmds: cmds.into(), ..Default::default() };
    let opts = Options { debug: Some(d), ..Options::default() };
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &opts, "Main");
    (ui, out)
}

fn lines(ui: &DebugUi) -> Vec<u32> {
    ui.pauses.iter().map(|p| p.line).collect()
}

const CODE: &str = "Sub Main()\n    Dim i As Long\n    For i = 1 To 3\n        Twice i\n    Next\n    Debug.Print \"done\"\nEnd Sub\n\nSub Twice(n)\n    Dim x\n    x = n * 2\n    Debug.Print x\nEnd Sub\n";

#[test]
fn breakpoints_stop_each_time() {
    let (ui, out) = debug(CODE, Debugger { breakpoints: vec![bp(11)], ..Default::default() }, vec![]);
    assert!(out.error.is_none());
    assert_eq!(lines(&ui), [11, 11, 11]);
    assert_eq!(ui.printed, [" 2 ", " 4 ", " 6 ", "done"]);
    let p = &ui.pauses[1];
    assert_eq!(p.reason, Reason::Breakpoint);
    let names: Vec<_> = p.locals.iter().map(|v| (v.name.as_str(), v.value.as_str())).collect();
    assert_eq!(names, [("n", "2"), ("x", "Empty")]);
    let stack: Vec<_> = p.stack.iter().map(|s| (s.proc.as_str(), s.line)).collect();
    assert_eq!(stack, [("Twice", 11), ("Main", 4)]);
}

#[test]
fn conditional_breakpoints() {
    let b = Breakpoint { condition: Some("n = 3".into()), ..bp(11) };
    let (ui, _) = debug(CODE, Debugger { breakpoints: vec![b], ..Default::default() }, vec![]);
    assert_eq!(lines(&ui), [11]);
    assert_eq!(ui.pauses[0].locals[0].value, "3");
}

#[test]
fn stepping() {
    let d = || Debugger { step: true, ..Default::default() };
    // Into: every statement, in the calls too (not Dim).
    let (ui, _) = debug(CODE, d(), vec![DebugCmd::StepInto; 6]);
    assert_eq!(lines(&ui), [3, 4, 11, 12, 4, 11, 12]);
    // Over: the calls run whole.
    let (ui, _) = debug(CODE, d(), vec![DebugCmd::StepOver; 5]);
    assert_eq!(lines(&ui), [3, 4, 4, 4, 6]);
    // Out: back to the caller.
    let (ui, _) = debug(CODE, d(), vec![DebugCmd::StepInto, DebugCmd::StepInto, DebugCmd::StepOut, DebugCmd::Continue]);
    assert_eq!(lines(&ui), [3, 4, 11, 4]);
    // To a line.
    let (ui, _) = debug(CODE, d(), vec![DebugCmd::RunTo { module: "module1".into(), line: 6 }]);
    assert_eq!(lines(&ui), [3, 6]);
    // End stops it there, without an error.
    let (ui, out) = debug(CODE, d(), vec![DebugCmd::End]);
    assert!(out.error.is_none());
    assert!(ui.printed.is_empty());
}

#[test]
fn immediate_window() {
    let cmds = vec![DebugCmd::Eval("?n * 10".into()), DebugCmd::Eval("n = 100".into()), DebugCmd::Eval("?nope(".into()), DebugCmd::Eval("? 1 / 0".into())];
    let (ui, out) = debug(CODE, Debugger { breakpoints: vec![bp(11)], ..Default::default() }, cmds);
    assert!(out.error.is_none());
    assert_eq!(ui.printed[0], " 10 ");
    // The variable changed: the macro goes on with it.
    assert_eq!(ui.printed[3], " 200 ");
    assert!(ui.printed[1].starts_with("Expected") || ui.printed[1].contains("rror"), "{:?}", ui.printed);
    assert_eq!(ui.printed[2], "Error 11: Division by zero");
    // The pause after each command shows the new values.
    assert_eq!(ui.pauses[2].locals[0].value, "100");
}

#[test]
fn errors_pause_and_retry() {
    let code = "Sub Main()\n    Dim d\n    d = 0\n    Debug.Print 10 / d\nEnd Sub\n";
    let d = || Debugger::default();
    // Fixed from the Immediate window, then run again.
    let (ui, out) = debug(code, d(), vec![DebugCmd::Eval("d = 5".into()), DebugCmd::Continue]);
    assert!(out.error.is_none());
    assert_eq!(ui.printed, [" 2 "]);
    assert!(matches!(&ui.pauses[0].reason, Reason::Error(e) if e.number == 11 && e.line == 4));
    // Ended: the error doesn't come back.
    let (_, out) = debug(code, d(), vec![DebugCmd::End]);
    assert!(out.error.is_none());
    // An On Error handler catches it: no pause.
    let code = "Sub Main()\n    On Error GoTo Fail\n    Inner\n    Exit Sub\nFail:\n    Debug.Print Err.Number\nEnd Sub\nSub Inner()\n    Debug.Print 1 / 0\nEnd Sub\n";
    let (ui, out) = debug(code, d(), vec![]);
    assert!(out.error.is_none() && ui.pauses.is_empty());
    assert_eq!(ui.printed, [" 11 "]);
}

#[test]
fn stop_and_assert() {
    let code = "Sub Main()\n    Debug.Print 1\n    Stop\n    Debug.Assert 1 = 2\n    Debug.Assert True\n    Debug.Print 2\nEnd Sub\n";
    let (ui, out) = debug(code, Debugger::default(), vec![]);
    assert!(out.error.is_none());
    assert_eq!(ui.pauses.iter().map(|p| (p.line, p.reason.clone())).collect::<Vec<_>>(), [(3, Reason::Stop), (4, Reason::Assert)]);
    assert_eq!(ui.printed, [" 1 ", " 2 "]);
    // Without the debugger, Stop ends the macro and Assert does nothing.
    assert_eq!(body("Debug.Print 1\nDebug.Assert False\nStop\nDebug.Print 2"), [" 1 "]);
}

#[test]
fn watches() {
    let w = |e: &str, kind| Watch { expr: e.into(), kind };
    let code = "Sub Main()\n    Dim i As Long, t As Long\n    For i = 1 To 5\n        t = t + i\n    Next\n    Debug.Print t\nEnd Sub\n";
    let d = Debugger { watches: vec![w("t", WatchKind::Watch), w("t > 5", WatchKind::BreakWhenTrue)], ..Default::default() };
    let (ui, _) = debug(code, d, vec![]);
    assert_eq!(ui.pauses.len(), 1);
    assert_eq!(ui.pauses[0].reason, Reason::Watch(1));
    assert_eq!(ui.pauses[0].watches[0].value, "6");
    let d = Debugger { watches: vec![w("t", WatchKind::BreakWhenChanged), w("nope +", WatchKind::Watch)], ..Default::default() };
    let (ui, _) = debug(code, d, vec![]);
    assert_eq!(ui.pauses.len(), 5);
    assert_eq!(ui.pauses[0].watches[0].value, "1");
    assert_eq!(ui.pauses[0].watches[1].ty, "");
}

#[test]
fn set_next_statement() {
    let code = "Sub Main()\n    Debug.Print 1\n    Debug.Print 2\n    If True Then\n        Debug.Print 3\n    End If\n    Debug.Print 4\nEnd Sub\n";
    // Skip line 3; then try to move into the If from outside (refused).
    let cmds = vec![DebugCmd::SetNext(4), DebugCmd::SetNext(5), DebugCmd::Continue];
    let (ui, out) = debug(code, Debugger { breakpoints: vec![bp(3)], ..Default::default() }, cmds);
    assert!(out.error.is_none());
    assert_eq!(lines(&ui), [3, 4, 4]);
    assert_eq!(ui.printed, [" 1 ", " 3 ", " 4 "]);
    // From inside the If back out of it, to an earlier line.
    let cmds = vec![DebugCmd::SetNext(7)];
    let (ui, _) = debug(code, Debugger { breakpoints: vec![bp(5)], ..Default::default() }, cmds);
    assert_eq!(ui.printed, [" 1 ", " 2 ", " 4 "]);
}

#[test]
fn arrays_and_collections_in_locals() {
    let code = "Sub Main()\n    Dim a(1 To 2, 0 To 1) As Integer, c As New Collection, s As String\n    a(2, 1) = 7\n    c.Add \"x\"\n    s = \"hi\"\n    Stop\nEnd Sub\n";
    let (ui, _) = debug(code, Debugger::default(), vec![]);
    let l = &ui.pauses[0].locals;
    let a = l.iter().find(|v| v.name == "a").unwrap();
    assert_eq!((a.ty.as_str(), a.value.as_str(), a.children.len()), ("Integer()", "(1 To 2, 0 To 1)", 4));
    assert_eq!((a.children[3].name.as_str(), a.children[3].value.as_str()), ("(2, 1)", "7"));
    let c = l.iter().find(|v| v.name == "c").unwrap();
    assert_eq!(c.children[0].value, "\"x\"");
    assert_eq!(l.iter().find(|v| v.name == "s").unwrap().value, "\"hi\"");
}

#[test]
fn interrupting() {
    let code = "Sub Main()\n    Dim i As Long\n    Do\n        i = i + 1\n    Loop\nEnd Sub\n";
    let p = project(code);
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mut ui = DebugUi { cmds: vec![DebugCmd::End].into(), ..Default::default() };
    let opts = Options { debug: Some(Debugger::default()), interrupt: Some(flag.clone()), ..Options::default() };
    let out = apex_vba::run_macro(&p, &mut workbook(), &mut ui, &opts, "Main");
    assert!(out.error.is_none());
    assert_eq!(ui.pauses.len(), 1);
    assert_eq!(ui.pauses[0].reason, Reason::Step);
    // Without the debugger it stops the macro.
    flag.store(true, std::sync::atomic::Ordering::Relaxed);
    let opts = Options { interrupt: Some(flag), ..Options::default() };
    let out = apex_vba::run_macro(&p, &mut workbook(), &mut DebugUi::default(), &opts, "Main");
    assert_eq!(out.error.map(|e| e.number), Some(18));
}

#[test]
fn immediate_with_no_macro_running() {
    let p = project("Public Total As Long\nFunction Twice(x)\n    Twice = x * 2\nEnd Function\n");
    let mut wb = workbook();
    let mut ui = DebugUi::default();
    for line in ["?Twice(21)", "Range(\"A1\").Value = 5", "Total = 3: Debug.Print Total + Range(\"A1\").Value", "?nope("] {
        apex_vba::immediate(&p, &mut wb, &mut ui, &Options::default(), line);
    }
    assert_eq!(&ui.printed[..2], [" 42 ", " 8 "]);
    assert_eq!(ui.printed.len(), 3);
    assert_eq!(text(&wb, 0, "A1"), "5");
}
