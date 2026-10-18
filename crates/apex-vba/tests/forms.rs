//! UserForms: a form's controls in its code, Show, the events of what the
//! user does in it (from a script here), Unload, ControlSource and
//! RowSource.

mod common;

use std::collections::VecDeque;
use std::sync::Arc;

use apex_sheet::{CellRef, Workbook};
use apex_vba::forms::{Action, Change, Control, Design, Event, Kind, View};
use apex_vba::{ModuleKind, Options, Project, Source, Ui};
use common::*;

type Step = Box<dyn FnMut(&View) -> Option<Event>>;

/// Answers the forms shown with the steps given, on the last form.
#[derive(Default)]
struct FormUi {
    steps: VecDeque<Step>,
    printed: Vec<String>,
    shown: Vec<String>,
    /// The forms each step saw (caption and how many).
    seen: Vec<(String, usize)>,
}

impl Ui for FormUi {
    fn msg_box(&mut self, prompt: &str, _buttons: i32, _title: &str) -> i32 {
        self.shown.push(prompt.to_string());
        1
    }
    fn input_box(&mut self, _prompt: &str, _title: &str, default: &str) -> Option<String> {
        Some(default.to_string())
    }
    fn debug_print(&mut self, text: &str) {
        self.printed.push(text.to_string());
    }
    fn can_show_forms(&self) -> bool {
        true
    }
    fn form_event(&mut self, forms: &[Arc<View>]) -> Option<Event> {
        let last = forms.last()?;
        self.seen.push((last.caption.clone(), forms.len()));
        let mut step = self.steps.pop_front()?;
        step(last)
    }
}

fn ev(v: &View, changes: Vec<(&str, Change)>, action: Action) -> Option<Event> {
    let changes = changes.into_iter().map(|(n, c)| (v.find(n).expect(n), c)).collect();
    Some(Event { form: v.id, changes, action })
}

fn click(name: &'static str) -> Step {
    Box::new(move |v| ev(v, vec![], Action::Click(v.find(name).expect(name))))
}

fn type_in(name: &'static str, text: &'static str) -> Step {
    Box::new(move |v| ev(v, vec![(name, Change::Text(text.into()))], Action::Changes))
}

fn close() -> Step {
    Box::new(|v| ev(v, vec![], Action::Close))
}

fn design(controls: Vec<Control>) -> Arc<Design> {
    Arc::new(Design { caption: "Datos".into(), controls, ..Design::default() })
}

fn ctl(kind: Kind, name: &str) -> Control {
    let mut c = Control::new(kind, name);
    if matches!(kind, Kind::CommandButton | Kind::Label | Kind::CheckBox | Kind::OptionButton) {
        c.caption = name.to_string();
    }
    c
}

fn standard() -> Arc<Design> {
    design(vec![ctl(Kind::TextBox, "txtName"), ctl(Kind::CommandButton, "cmdOK"), ctl(Kind::CommandButton, "cmdCancel"), ctl(Kind::ListBox, "lstItems"), ctl(Kind::CheckBox, "chkVip"), ctl(Kind::OptionButton, "optA"), ctl(Kind::OptionButton, "optB"), ctl(Kind::Label, "lblInfo")])
}

struct Run {
    out: apex_vba::Outcome,
    ui: FormUi,
    wb: Workbook,
}

fn run_form(form: &str, design: Arc<Design>, main: &str, steps: Vec<Step>, wb: Option<Workbook>) -> Run {
    let src = vec![Source { name: "UserForm1".into(), kind: ModuleKind::Form, code: form.into() }, Source { name: "Module1".into(), kind: ModuleKind::Standard, code: main.into() }];
    let p = Project::parse(&src).unwrap_or_else(|e| panic!("{} line {}: {e}", e.module, e.line));
    let mut wb = wb.unwrap_or_else(workbook);
    let mut ui = FormUi { steps: steps.into(), ..FormUi::default() };
    let opts = Options { forms: vec![("UserForm1".into(), design)], ..Options::default() };
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &opts, "Main");
    Run { out, ui, wb }
}

fn ok(r: &Run) {
    if let Some(e) = &r.out.error {
        panic!("{} line {}: {e}", e.module, e.line);
    }
}

const FORM: &str = r#"
Public Accepted As Boolean

Private Sub UserForm_Initialize()
    Me.Caption = "Alta de cliente"
    lstItems.AddItem "Uno"
    lstItems.AddItem "Dos"
    lstItems.AddItem "Tres"
    txtName.Value = "nadie"
    Debug.Print "init"
End Sub

Private Sub txtName_Change()
    lblInfo.Caption = "Hola " & txtName.Text
End Sub

Private Sub cmdOK_Click()
    Accepted = True
    Me.Hide
End Sub

Private Sub cmdCancel_Click()
    Unload Me
End Sub

Private Sub lstItems_Click()
    Debug.Print "list " & lstItems.ListIndex & " " & lstItems.Value
End Sub

Private Sub UserForm_QueryClose(Cancel As Integer, CloseMode As Integer)
    Debug.Print "close " & CloseMode
End Sub

Private Sub UserForm_Terminate()
    Debug.Print "terminate"
End Sub
"#;

#[test]
fn a_modal_form_runs_its_events_until_it_is_hidden() {
    let main = r#"
Sub Main()
    UserForm1.Show
    Debug.Print UserForm1.Accepted & "|" & UserForm1.txtName.Value & "|" & UserForm1.lblInfo.Caption
    Debug.Print UserForm1.lstItems.ListCount & "|" & UserForm1.lstItems.List(1)
    Range("A1").Value = UserForm1.txtName
    Unload UserForm1
End Sub
"#;
    let steps: Vec<Step> = vec![type_in("txtName", "Ana"), Box::new(|v| ev(v, vec![("lstItems", Change::Index(1))], Action::Changes)), click("cmdOK")];
    let r = run_form(FORM, standard(), main, steps, None);
    ok(&r);
    assert_eq!(r.ui.printed, vec!["init", "list 1 Dos", "True|Ana|Hola Ana", "3|Dos", "close 1", "terminate"]);
    assert_eq!(r.ui.seen[0].0, "Alta de cliente");
    assert_eq!(text(&r.wb, 0, "A1"), "Ana");
}

#[test]
fn closing_the_window_asks_query_close_and_unloads() {
    let main = r#"
Sub Main()
    UserForm1.Show
    Debug.Print "after|" & UserForm1.Accepted
End Sub
"#;
    let r = run_form(FORM, standard(), main, vec![close()], None);
    ok(&r);
    // The form named again after its Unload is a new one.
    assert_eq!(r.ui.printed, vec!["init", "close 0", "terminate", "init", "after|False"]);
}

#[test]
fn query_close_may_keep_the_form() {
    let form = r#"
Private Sub UserForm_QueryClose(Cancel As Integer, CloseMode As Integer)
    If CloseMode = 0 And txtName.Text = "" Then Cancel = True: Debug.Print "kept"
End Sub
"#;
    let main = "Sub Main()\n    Dim f As New UserForm1\n    f.Show\n    Debug.Print \"done|\" & f.txtName.Text\nEnd Sub\n";
    let r = run_form(form, standard(), main, vec![close(), type_in("txtName", "x"), close()], None);
    ok(&r);
    assert_eq!(r.ui.printed, vec!["kept", "done|x"]);
}

#[test]
fn key_press_may_reject_a_key() {
    let form = r#"
Private Sub txtName_KeyPress(ByVal KeyAscii As MSForms.ReturnInteger)
    If KeyAscii < 48 Or KeyAscii > 57 Then KeyAscii = 0
End Sub
Private Sub txtName_Change()
    Debug.Print "change " & txtName.Text
End Sub
"#;
    let main = "Sub Main()\n    UserForm1.Show\nEnd Sub\n";
    let steps: Vec<Step> = vec![
        Box::new(|v| ev(v, vec![("txtName", Change::Text("a".into()))], Action::Key(v.find("txtName").unwrap(), 'a'))),
        Box::new(|v| ev(v, vec![("txtName", Change::Text("7".into()))], Action::Key(v.find("txtName").unwrap(), '7'))),
        close(),
    ];
    let r = run_form(form, standard(), main, steps, None);
    ok(&r);
    assert_eq!(r.ui.printed, vec!["change 7"]);
}

#[test]
fn option_buttons_of_a_group_exclude_each_other() {
    let form = r#"
Private Sub optA_Change()
    Debug.Print "A " & optA.Value
End Sub
Private Sub optB_Change()
    Debug.Print "B " & optB.Value
End Sub
Private Sub chkVip_Click()
    Debug.Print "vip " & chkVip.Value
End Sub
"#;
    let main = "Sub Main()\n    UserForm1.optA.Value = True\n    UserForm1.Show\n    Debug.Print UserForm1.optA & \"|\" & UserForm1.optB\nEnd Sub\n";
    let steps: Vec<Step> = vec![Box::new(|v| ev(v, vec![("optB", Change::Checked(true))], Action::Changes)), Box::new(|v| ev(v, vec![("chkVip", Change::Checked(true))], Action::Changes)), Box::new(|v| ev(v, vec![], Action::Click(v.find("cmdOK").unwrap())))];
    let form = format!("{form}\nPrivate Sub cmdOK_Click()\n    Me.Hide\nEnd Sub\n");
    let r = run_form(&form, standard(), main, steps, None);
    ok(&r);
    assert_eq!(r.ui.printed, vec!["A True", "A False", "B True", "vip True", "False|True"]);
}

#[test]
fn focus_runs_before_update_exit_and_enter() {
    let form = r#"
Private Sub txtName_Enter()
    Debug.Print "enter"
End Sub
Private Sub txtName_BeforeUpdate(ByVal Cancel As MSForms.ReturnBoolean)
    Debug.Print "before " & txtName.Text
    If txtName.Text = "" Then Cancel = True
End Sub
Private Sub txtName_AfterUpdate()
    Debug.Print "after"
End Sub
Private Sub txtName_Exit(ByVal Cancel As MSForms.ReturnBoolean)
    Debug.Print "exit"
End Sub
"#;
    let main = "Sub Main()\n    UserForm1.Show\nEnd Sub\n";
    let to = |name: &'static str| -> Step { Box::new(move |v| ev(v, vec![], Action::Focus(v.find(name)))) };
    let steps: Vec<Step> = vec![
        to("txtName"),
        Box::new(|v| ev(v, vec![("txtName", Change::Text("".into()))], Action::Focus(v.find("cmdOK")))),
        Box::new(|v| ev(v, vec![("txtName", Change::Text("x".into()))], Action::Focus(v.find("cmdOK")))),
        Box::new(|v| {
            // The focus left the text box.
            assert_eq!(v.focus, v.find("cmdOK"));
            ev(v, vec![], Action::Close)
        }),
    ];
    let mut d = (*standard()).clone();
    d.controls[0].value = "a".into();
    let r = run_form(form, Arc::new(d), main, steps, None);
    ok(&r);
    assert_eq!(r.ui.printed, vec!["enter", "before ", "before x", "after", "exit"]);
}

#[test]
fn control_source_and_row_source_reach_the_sheet() {
    let mut wb = workbook();
    for (i, t) in ["Norte", "Sur", "Este"].iter().enumerate() {
        wb.set_input(0, CellRef::new(i as u32, 1), t);
    }
    wb.set_input(0, CellRef::new(0, 0), "viejo");
    let mut d = (*standard()).clone();
    d.controls[0].control_source = "A1".into();
    d.controls[3].row_source = "B1:B3".into();
    let form = "Private Sub cmdOK_Click()\n    Unload Me\nEnd Sub\n";
    let main = "Sub Main()\n    UserForm1.Show\nEnd Sub\n";
    let steps: Vec<Step> = vec![
        Box::new(|v| {
            assert_eq!(v.items[0].c.value, "viejo");
            assert_eq!(v.items[3].list.len(), 3);
            ev(v, vec![("txtName", Change::Text("42".into())), ("lstItems", Change::Index(2))], Action::Changes)
        }),
        click("cmdOK"),
    ];
    let r = run_form(form, Arc::new(d), main, steps, Some(wb));
    ok(&r);
    assert_eq!(text(&r.wb, 0, "A1"), "42");
    assert!(r.out.changed);
}

#[test]
fn controls_can_be_listed_added_and_removed() {
    let form = r#"
Private Sub UserForm_Initialize()
    Dim c As Control, n As Long
    For Each c In Me.Controls
        If TypeOf c Is MSForms.TextBox Then n = n + 1
    Next
    Debug.Print Me.Controls.Count & "|" & n & "|" & TypeName(Me.Controls(0))
    Dim t As Object
    Set t = Me.Controls.Add("Forms.TextBox.1", "txtExtra", True)
    t.Text = "nuevo"
    Debug.Print Controls("txtExtra").Text & "|" & Controls.Count
    Controls.Remove "txtExtra"
    Debug.Print Controls.Count
    lstItems.List = Array("a", "b")
    lstItems.ListIndex = 1
    Debug.Print lstItems.Text & "|" & lstItems.ListCount
    lstItems.Clear
    Debug.Print lstItems.ListCount & "|" & IsNull(lstItems.Value)
End Sub
"#;
    let main = "Sub Main()\n    Load UserForm1\nEnd Sub\n";
    let r = run_form(form, standard(), main, vec![], None);
    ok(&r);
    assert_eq!(r.ui.printed, vec!["8|1|TextBox", "nuevo|9", " 8 ", "b|2", "0|True"]);
}

#[test]
fn a_modeless_form_keeps_going_after_the_macro() {
    let form = "Private Sub cmdOK_Click()\n    Debug.Print \"ok \" & txtName.Text\n    Unload Me\nEnd Sub\n";
    let main = "Sub Main()\n    UserForm1.Show vbModeless\n    Debug.Print \"macro done\"\nEnd Sub\n";
    let r = run_form(form, standard(), main, vec![type_in("txtName", "z"), click("cmdOK")], None);
    ok(&r);
    assert_eq!(r.ui.printed, vec!["macro done", "ok z"]);
}

#[test]
fn forms_need_a_window() {
    let src = vec![Source { name: "UserForm1".into(), kind: ModuleKind::Form, code: String::new() }, Source { name: "Module1".into(), kind: ModuleKind::Standard, code: "Sub Main()\n    UserForm1.Show\nEnd Sub\n".into() }];
    let p = Project::parse(&src).unwrap();
    let mut wb = workbook();
    let mut ui = TestUi::default();
    let out = apex_vba::run_macro(&p, &mut wb, &mut ui, &Options::default(), "Main");
    assert!(out.error.unwrap().to_string().contains("Forms can't be shown"));
}

#[test]
fn a_window_that_goes_ends_the_forms() {
    let main = "Sub Main()\n    UserForm1.Show\n    Debug.Print \"back\"\nEnd Sub\n";
    // No steps: the window answers None.
    let r = run_form(FORM, standard(), main, vec![], None);
    ok(&r);
    assert_eq!(r.ui.printed, vec!["init", "back"]);
}

#[test]
fn a_form_in_a_loop_of_events_is_stopped_by_the_step_limit() {
    let form = "Private Sub txtName_Change()\n    txtName.Text = txtName.Text & \"x\"\nEnd Sub\n";
    let main = "Sub Main()\n    UserForm1.Show\nEnd Sub\n";
    let r = run_form(form, standard(), main, vec![type_in("txtName", "a")], None);
    let e = r.out.error.expect("an error");
    assert!(e.number == 28 || e.number == 18 || e.number == 14, "{e}");
}

#[test]
fn the_first_control_in_tab_order_takes_the_focus() {
    let main = "Sub Main()\n    UserForm1.Show\nEnd Sub\n";
    let mut d = (*standard()).clone();
    // The label can't take it; the check box comes first in the tab order.
    for (i, c) in d.controls.iter_mut().enumerate() {
        c.tab_index = match c.name.as_str() {
            "lblInfo" => 0,
            "chkVip" => 1,
            _ => 2 + i as i32,
        };
    }
    let steps: Vec<Step> = vec![Box::new(|v| {
        assert_eq!(v.focus, v.find("chkVip"));
        ev(v, vec![], Action::Close)
    })];
    let r = run_form("", Arc::new(d), main, steps, None);
    ok(&r);
}
