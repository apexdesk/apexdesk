//! UserForms while a macro runs. A form module's object holds the form's
//! controls ([`View`]); `UserForm1` alone is its default object, made
//! (Load, UserForm_Initialize) the first time it is named. `Show` hands
//! the form to [`crate::Ui::form_event`] and runs what the user does
//! (a click, a text typed...) as the form's event procedures until it is
//! hidden or unloaded; a modeless one (`Show vbModeless`) keeps going
//! once the macro is done, until it is closed. The controls' properties,
//! lists and Controls are those of MSForms; `ControlSource` and
//! `RowSource` reach the workbook as any other macro change does.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use super::{Interp, VType, slot};
use crate::ast::{Arg, ModuleKind, ProcKind};
use crate::error::{Error, R};
use crate::forms::{Action, Change, Control, Design, Event, Item, Kind, View, handles};
use crate::object::{Instance, Obj, RangeRef};
use crate::value::{Array, Value};

/// Rows a list may get from a RowSource or a List of an array.
const MAX_ROWS: usize = 100_000;
/// Columns of a list.
const MAX_COLS: usize = 64;
/// Controls a form may get with Controls.Add.
const MAX_ADDED: usize = 2000;
/// QueryClose's CloseMode: the close button, or Unload in code.
const CLOSE_BUTTON: i32 = 0;
const CLOSE_CODE: i32 = 1;

/// The forms of a run.
#[derive(Default)]
pub(crate) struct Forms {
    /// The default object of each form module that has one.
    defaults: Vec<(usize, Rc<Instance>)>,
    /// The forms shown, the last over the others.
    shown: Vec<Rc<Instance>>,
    next_id: u64,
}

fn obj(o: Obj) -> Value {
    Value::Object(o)
}

fn checked(v: &str) -> bool {
    v == "1" || v.eq_ignore_ascii_case("true") || v == "-1"
}

/// A form's object's view, borrowed.
fn view(inst: &Instance) -> R<std::cell::Ref<'_, View>> {
    inst.form.as_ref().map(|v| v.borrow()).ok_or_else(|| Error::no_member("form"))
}

fn view_mut(inst: &Instance) -> R<std::cell::RefMut<'_, View>> {
    inst.form.as_ref().map(|v| v.borrow_mut()).ok_or_else(|| Error::no_member("form"))
}

/// The value of a list's row `r` in column `c` (from 0).
fn cell_of(it: &Item, r: usize, c: usize) -> Value {
    match it.list.get(r).and_then(|row| row.get(c)) {
        Some(s) => Value::str(s.clone()),
        None if r < it.list.len() => Value::Null,
        None => Value::Empty,
    }
}

impl Interp<'_> {
    /// The view of a new object of module `mi`, when it is a form.
    pub(crate) fn form_view(&mut self, mi: usize) -> Option<View> {
        let m = &self.project.modules[mi];
        if m.kind != ModuleKind::Form {
            return None;
        }
        let design = self.form_designs.iter().find(|(n, _)| n.eq_ignore_ascii_case(&m.name)).map(|(_, d)| d.clone()).unwrap_or_else(|| Arc::new(Design::default()));
        self.forms.next_id += 1;
        let mut v = View::new(self.forms.next_id, &m.name, &design);
        if v.caption.is_empty() {
            v.caption = m.name.clone();
        }
        v.handles = v.items.iter().map(|it| self.form_handles(mi, &it.c.name)).collect();
        Some(v)
    }

    /// Which events of control `name` module `mi` handles.
    fn form_handles(&self, mi: usize, name: &str) -> u8 {
        let m = &self.project.modules[mi];
        let mut out = 0;
        let prefix = format!("{}_", name.to_ascii_lowercase());
        for p in m.procs.iter().filter(|p| p.kind == ProcKind::Sub) {
            let Some(ev) = p.name.to_ascii_lowercase().strip_prefix(&prefix).map(str::to_string) else { continue };
            out |= match ev.as_str() {
                "change" | "spinup" | "spindown" | "scroll" => handles::CHANGE,
                "keypress" | "keydown" | "keyup" => handles::KEY,
                "enter" | "exit" | "beforeupdate" | "afterupdate" => handles::FOCUS,
                "click" | "mousedown" | "mouseup" => handles::CLICK,
                "dblclick" => handles::DBLCLICK,
                _ => 0,
            };
        }
        out
    }

    /// A form just made: its lists from their RowSource and values from
    /// their ControlSource.
    pub(crate) fn form_loaded(&mut self, inst: &Rc<Instance>) -> R<()> {
        let n = view(inst)?.items.len();
        for i in 0..n {
            let (rows, source) = {
                let v = view(inst)?;
                let c = &v.items[i].c;
                (c.row_source.clone(), c.control_source.clone())
            };
            if !rows.trim().is_empty() {
                self.fill_from_row_source(inst, i, &rows)?;
            }
            if !source.trim().is_empty() {
                let text = self.source_text(&source)?;
                let mut v = view_mut(inst)?;
                let it = &mut v.items[i];
                match it.c.kind {
                    Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton => it.c.value = if checked(&text) { "1".into() } else { "0".into() },
                    _ => it.c.value = text,
                }
                if matches!(it.c.kind, Kind::ListBox | Kind::ComboBox) {
                    let value = it.c.value.clone();
                    it.list_index = it.list.iter().position(|r| r.first() == Some(&value)).map_or(-1, |r| r as i32);
                }
            }
        }
        Ok(())
    }

    /// The text of the first cell of a ControlSource.
    fn source_text(&mut self, source: &str) -> R<String> {
        let r = self.parse_ref(source, self.active)?;
        Ok(self.wb.display(r.sheet, r.range.start).0)
    }

    fn fill_from_row_source(&mut self, inst: &Rc<Instance>, i: usize, source: &str) -> R<()> {
        let r = self.parse_ref(source, self.active)?;
        let rg = self.wb.clamp_to_used(r.sheet, &r.range).unwrap_or(apex_sheet::Range::single(r.range.start));
        let mut rows = Vec::new();
        for row in rg.start.row..=rg.end.row.min(rg.start.row.saturating_add(MAX_ROWS as u32 - 1)) {
            let mut cols = Vec::new();
            for col in rg.start.col..=rg.end.col.min(rg.start.col.saturating_add(MAX_COLS as u32 - 1)) {
                cols.push(self.wb.display(r.sheet, apex_sheet::CellRef::new(row, col)).0);
            }
            rows.push(cols);
        }
        let mut v = view_mut(inst)?;
        let it = &mut v.items[i];
        it.list = rows;
        it.selected = vec![false; it.list.len()];
        it.list_index = -1;
        Ok(())
    }

    /// Writes control `i`'s value to its ControlSource, if it has one.
    fn write_source(&mut self, inst: &Rc<Instance>, i: usize) -> R<()> {
        let (source, value) = {
            let v = view(inst)?;
            let it = &v.items[i];
            let value = match it.c.kind {
                Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton => Value::Bool(checked(&it.c.value)),
                Kind::SpinButton | Kind::ScrollBar => Value::Long(it.c.value.parse().unwrap_or(0)),
                _ => Value::str(it.c.value.clone()),
            };
            (it.c.control_source.clone(), value)
        };
        if source.trim().is_empty() {
            return Ok(());
        }
        let r = self.parse_ref(&source, self.active)?;
        // Text that reads as a number goes in as one, as Excel does.
        let value = match value {
            Value::Str(s) if !s.is_empty() => match Value::Str(s.clone()).to_f64() {
                Ok(n) if s.trim() == &*s => Value::Double(n),
                _ => Value::Str(s),
            },
            v => v,
        };
        self.set_range_value(RangeRef::new(r.sheet, apex_sheet::Range::single(r.range.start)), value)
    }

    /// The default object of form module `mi`, made the first time.
    pub(crate) fn form_default(&mut self, mi: usize) -> R<Rc<Instance>> {
        if let Some((_, i)) = self.forms.defaults.iter().find(|(m, _)| *m == mi) {
            return Ok(i.clone());
        }
        let inst = self.make_instance(mi)?;
        // Before Initialize, which may name the form.
        self.forms.defaults.push((mi, inst.clone()));
        self.initialize(&inst)?;
        Ok(inst)
    }

    /// The form the procedure running is of (Me in a form module).
    pub(crate) fn form_me(&self) -> Option<Rc<Instance>> {
        self.frames.last().and_then(|f| f.me.clone()).filter(|i| i.form.is_some())
    }

    /// Runs `<control>_<event>` (or `UserForm_<event>`) of `inst`'s module
    /// with `values`: what the handler left in them, or None when there
    /// is no handler.
    fn fire(&mut self, inst: &Rc<Instance>, control: &str, event: &str, values: Vec<Value>) -> R<Option<Vec<Value>>> {
        let project = self.project;
        let m = &project.modules[inst.module];
        let name = format!("{control}_{event}");
        let Some(p) = m.procs.iter().find(|p| p.kind == ProcKind::Sub && p.name.eq_ignore_ascii_case(&name)) else { return Ok(None) };
        // The arguments it declares, of those Excel passes.
        let n = p.params.len().min(values.len());
        let mut locals = std::collections::HashMap::new();
        let mut slots = Vec::new();
        for (param, v) in p.params.iter().zip(values.into_iter().take(n)) {
            // MSForms.ReturnBoolean and ReturnInteger: plain values here.
            let s = slot(if param.ty == VType::Object { VType::Variant } else { param.ty }, v.coerce(if param.ty == VType::Object { VType::Variant } else { param.ty })?);
            slots.push(s.clone());
            locals.insert(param.name.clone(), s);
        }
        if p.params.len() > n {
            return Err(Error::app(format!("Procedure declaration does not match description of event: {}", p.name)).at(&m.name, p.line));
        }
        self.run_handler(inst.module, p.clone(), locals, Some(inst.clone()))?;
        Ok(Some(slots.iter().map(|s| s.borrow().val.clone()).collect()))
    }

    fn fire_plain(&mut self, inst: &Rc<Instance>, i: usize, event: &str) -> R<()> {
        let name = view(inst)?.items[i].c.name.clone();
        self.fire(inst, &name, event, Vec::new()).map(|_| ())
    }

    /// Runs Initialize of a form or Class_Initialize of a class.
    pub(crate) fn initialize(&mut self, inst: &Rc<Instance>) -> R<()> {
        if inst.form.is_some() {
            self.form_loaded(inst)?;
            self.fire(inst, "UserForm", "Initialize", Vec::new())?;
            return Ok(());
        }
        let project = self.project;
        if let Some(p) = project.modules[inst.module].proc("class_initialize") {
            self.call_method(inst.clone(), p.clone(), &[])?;
        }
        Ok(())
    }

    // --- showing ------------------------------------------------------------------

    pub(crate) fn show_form(&mut self, inst: Rc<Instance>, modal: bool) -> R<()> {
        if !self.ui.can_show_forms() {
            return Err(Error::app("Forms can't be shown here: run the macro from ApexDesk's window"));
        }
        {
            let mut v = view_mut(&inst)?;
            if v.shown {
                if modal && !v.modal {
                    return Err(Error::new(400, "Form already displayed; can't show modally"));
                }
                return Ok(());
            }
            if self.forms.shown.len() >= 16 {
                return Err(Error::app("Too many forms shown at once"));
            }
            v.shown = true;
            v.modal = modal;
            v.unloaded = false;
        }
        self.forms.shown.push(inst.clone());
        self.fire(&inst, "UserForm", "Activate", Vec::new())?;
        // The first control in the tab order takes the focus.
        let first = {
            let v = view(&inst)?;
            if v.focus.is_some() { None } else { v.first_focus() }
        };
        if first.is_some() {
            self.move_focus(&inst, first)?;
        }
        if modal { self.form_loop(Some(&inst)) } else { Ok(()) }
    }

    pub(crate) fn hide_form(&mut self, inst: &Rc<Instance>) -> R<()> {
        let was = std::mem::replace(&mut view_mut(inst)?.shown, false);
        self.forms.shown.retain(|i| !Rc::ptr_eq(i, inst));
        if was {
            let views: Vec<Arc<View>> = self.forms.shown.iter().filter_map(|i| Some(Arc::new(i.form.as_ref()?.borrow().clone()))).collect();
            self.ui.forms_hidden(&views);
        }
        Ok(())
    }

    /// Unloads a form: QueryClose (when `mode` is given, which it may
    /// cancel), then Terminate. Whether it was unloaded.
    pub(crate) fn unload_form(&mut self, inst: &Rc<Instance>, mode: Option<i32>) -> R<bool> {
        if view(inst)?.unloaded {
            return Ok(true);
        }
        if let Some(mode) = mode
            && let Some(back) = self.fire(inst, "UserForm", "QueryClose", vec![Value::Bool(false), Value::Int(mode as i16)])?
            && back.first().is_some_and(|c| c.to_bool().unwrap_or(false) || c.to_i64().unwrap_or(0) != 0)
        {
            return Ok(false);
        }
        self.hide_form(inst)?;
        view_mut(inst)?.unloaded = true;
        self.forms.defaults.retain(|(_, i)| !Rc::ptr_eq(i, inst));
        self.fire(inst, "UserForm", "Terminate", Vec::new())?;
        Ok(true)
    }

    /// Hands the forms shown to the window and runs what is done in them,
    /// until `until` is hidden (None: until none is shown).
    pub(crate) fn form_loop(&mut self, until: Option<&Rc<Instance>>) -> R<()> {
        loop {
            let done = match until {
                Some(i) => !view(i)?.shown,
                None => self.forms.shown.is_empty(),
            };
            if done {
                return Ok(());
            }
            let last_modal = self.forms.shown.iter().rposition(|i| i.form.as_ref().is_some_and(|v| v.borrow().modal)).unwrap_or(0);
            let views: Vec<Arc<View>> = self
                .forms
                .shown
                .iter()
                .enumerate()
                .filter_map(|(k, i)| {
                    let mut v = i.form.as_ref()?.borrow().clone();
                    v.enabled = k >= last_modal;
                    Some(Arc::new(v))
                })
                .collect();
            let t = Instant::now();
            let ev = self.ui.form_event(&views);
            self.paused += t.elapsed();
            self.tick()?;
            if self.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
                return Err(Error::new(18, "The macro was stopped").fatal());
            }
            let Some(ev) = ev else {
                // The window went: the forms go with it.
                for i in std::mem::take(&mut self.forms.shown) {
                    let mut v = view_mut(&i)?;
                    v.shown = false;
                    v.unloaded = true;
                }
                return Ok(());
            };
            let Some(inst) = self.forms.shown.iter().find(|i| i.form.as_ref().is_some_and(|v| v.borrow().id == ev.form)).cloned() else { continue };
            self.form_event_done(&inst, ev)?;
        }
    }

    /// Runs what the user did in a form.
    fn form_event_done(&mut self, inst: &Rc<Instance>, ev: Event) -> R<()> {
        let n = view(inst)?.items.len();
        let mut rejected = None;
        if let Action::Key(i, ch) = ev.action
            && i < n
        {
            let name = view(inst)?.items[i].c.name.clone();
            if let Some(back) = self.fire(inst, &name, "KeyPress", vec![Value::Long(ch as i32)])? {
                let k = back.first().and_then(|v| v.to_i64().ok()).unwrap_or(ch as i64);
                if k == 0 {
                    rejected = Some(i);
                }
            }
        }
        for (i, ch) in ev.changes {
            if i >= n || rejected == Some(i) && matches!(ch, Change::Text(_)) {
                continue;
            }
            self.change(inst, i, ch)?;
        }
        match ev.action {
            Action::Changes | Action::Key(..) => Ok(()),
            Action::Click(i) if i < n => {
                let kind = view(inst)?.items[i].c.kind;
                // Lists and boxes fire Click with their change.
                if matches!(kind, Kind::CommandButton | Kind::Label | Kind::Image | Kind::Frame | Kind::Page) {
                    self.fire_plain(inst, i, "Click")?;
                }
                Ok(())
            }
            Action::DblClick(i) if i < n => {
                let name = view(inst)?.items[i].c.name.clone();
                self.fire(inst, &name, "DblClick", vec![Value::Bool(false)]).map(|_| ())
            }
            Action::Focus(to) => self.move_focus(inst, to.filter(|&t| t < n)),
            Action::Close => self.unload_form(inst, Some(CLOSE_BUTTON)).map(|_| ()),
            _ => Ok(()),
        }
    }

    /// The focus leaves a control for another: BeforeUpdate and
    /// AfterUpdate when it changed, Exit (each may keep it there), Enter.
    fn move_focus(&mut self, inst: &Rc<Instance>, to: Option<usize>) -> R<()> {
        let from = view(inst)?.focus;
        if from == to {
            return Ok(());
        }
        if let Some(f) = from {
            let (name, dirty) = {
                let v = view(inst)?;
                (v.items[f].c.name.clone(), v.items[f].dirty)
            };
            if dirty {
                if self.fire(inst, &name, "BeforeUpdate", vec![Value::Bool(false)])?.and_then(|b| b.first().cloned()).is_some_and(|c| c.to_bool().unwrap_or(false)) {
                    return Ok(());
                }
                view_mut(inst)?.items[f].dirty = false;
                self.fire(inst, &name, "AfterUpdate", Vec::new())?;
            }
            if self.fire(inst, &name, "Exit", vec![Value::Bool(false)])?.and_then(|b| b.first().cloned()).is_some_and(|c| c.to_bool().unwrap_or(false)) {
                return Ok(());
            }
        }
        view_mut(inst)?.focus = to;
        if let Some(t) = to {
            self.fire_plain(inst, t, "Enter")?;
        }
        Ok(())
    }

    /// A control's value changes (in the window or from code): it fires
    /// its events when it really changed.
    fn change(&mut self, inst: &Rc<Instance>, i: usize, ch: Change) -> R<()> {
        let kind = view(inst)?.items[i].c.kind;
        let mut click = false;
        let mut others: Vec<usize> = Vec::new();
        {
            let mut v = view_mut(inst)?;
            let it = &mut v.items[i];
            match ch {
                Change::Text(t) => {
                    if it.c.value == t {
                        return Ok(());
                    }
                    if it.c.max_length > 0 && kind == Kind::TextBox {
                        it.c.value = t.chars().take(it.c.max_length as usize).collect();
                    } else {
                        it.c.value = t;
                    }
                    if kind == Kind::ComboBox {
                        let value = it.c.value.clone();
                        it.list_index = it.list.iter().position(|r| r.first() == Some(&value)).map_or(-1, |r| r as i32);
                    }
                }
                Change::Checked(b) => {
                    let new = if b { "1" } else { "0" };
                    if it.c.value == new {
                        return Ok(());
                    }
                    it.c.value = new.into();
                    click = true;
                    if b && kind == Kind::OptionButton {
                        let (parent, group) = (it.parent, it.c.group.clone());
                        others = (0..v.items.len()).filter(|&k| k != i && !v.items[k].gone && v.items[k].c.kind == Kind::OptionButton && v.items[k].parent == parent && v.items[k].c.group.eq_ignore_ascii_case(&group) && checked(&v.items[k].c.value)).collect();
                    }
                }
                Change::Index(n) => {
                    let n = if matches!(kind, Kind::MultiPage) { n.clamp(0, (v.pages(i).len() as i32 - 1).max(0)) } else { n.clamp(-1, v.items[i].list.len() as i32 - 1) };
                    let it = &mut v.items[i];
                    if it.list_index == n {
                        return Ok(());
                    }
                    it.list_index = n;
                    if matches!(kind, Kind::ListBox | Kind::ComboBox) {
                        it.c.value = if n >= 0 { it.list.get(n as usize).and_then(|r| r.first()).cloned().unwrap_or_default() } else { String::new() };
                        click = true;
                    }
                }
                Change::Selected(sel) => {
                    let mut sel = sel;
                    sel.resize(it.list.len(), false);
                    if it.selected == sel {
                        return Ok(());
                    }
                    it.selected = sel;
                    click = true;
                }
                Change::Number(n) => {
                    let (lo, hi) = (it.c.min.min(it.c.max), it.c.min.max(it.c.max));
                    let n = n.clamp(lo, hi).to_string();
                    if it.c.value == n {
                        return Ok(());
                    }
                    it.c.value = n;
                }
            }
            v.items[i].dirty = true;
        }
        for k in others {
            view_mut(inst)?.items[k].c.value = "0".into();
            self.write_source(inst, k)?;
            self.fire_plain(inst, k, "Change")?;
        }
        self.write_source(inst, i)?;
        self.fire_plain(inst, i, "Change")?;
        if click {
            self.fire_plain(inst, i, "Click")?;
        }
        Ok(())
    }

    // --- the form's members -------------------------------------------------------

    /// Whether `name` is a control of `inst` or one of a form's own
    /// members: what a name alone means in the form's code.
    pub(crate) fn form_knows(&self, inst: &Instance, name: &str) -> bool {
        inst.form.as_ref().is_some_and(|v| v.borrow().find(name).is_some())
            || matches!(name, "caption" | "controls" | "hide" | "show" | "width" | "height" | "insidewidth" | "insideheight" | "left" | "top" | "repaint" | "activecontrol" | "backcolor" | "startupposition" | "tag" | "enabled")
    }

    /// `form.name(args)`: a control, or the form's own property or method.
    pub(crate) fn form_member(&mut self, inst: Rc<Instance>, name: &str, args: &[Arg]) -> R<Value> {
        let found = view(&inst)?.find(name);
        if let Some(i) = found {
            let c = obj(Obj::Control(inst.clone(), i));
            return if args.is_empty() { Ok(c) } else { self.index_value(c, args) };
        }
        let a = self.args(args)?;
        let v = view(&inst)?;
        let out = match name {
            "caption" => Value::str(v.caption.clone()),
            "name" => Value::str(v.name.clone()),
            "width" => Value::Double((v.width + 4.0) as f64),
            "height" => Value::Double((v.height + 22.0) as f64),
            "insidewidth" => Value::Double(v.width as f64),
            "insideheight" => Value::Double(v.height as f64),
            "left" => Value::Double(v.left as f64),
            "top" => Value::Double(v.top as f64),
            "startupposition" => Value::Long(v.start_up),
            "backcolor" => Value::Long(v.back.map_or(0x8000_000F_u32 as i32, |c| c as i32)),
            "visible" => Value::Bool(v.shown),
            "enabled" => Value::Bool(true),
            "tag" => Value::str(""),
            "activecontrol" => match v.focus {
                Some(f) => obj(Obj::Control(inst.clone(), f)),
                None => Value::Nothing,
            },
            "controls" => {
                drop(v);
                let c = obj(Obj::Controls(inst, None, false));
                return if args.is_empty() { Ok(c) } else { self.index_value(c, args) };
            }
            "font" => {
                drop(v);
                return Ok(obj(Obj::ControlFont(inst, None)));
            }
            "show" => {
                drop(v);
                let modal = match a.get(0, "modal") {
                    Some(m) => self.scalar(m)?.to_i64()? != 0,
                    None => true,
                };
                self.show_form(inst, modal)?;
                return Ok(Value::Empty);
            }
            "hide" => {
                drop(v);
                self.hide_form(&inst)?;
                return Ok(Value::Empty);
            }
            "repaint" | "setfocus" | "printform" => Value::Empty,
            "move" => {
                drop(v);
                let nums: Vec<Option<f64>> = (0..4).map(|k| a.get(k, ["left", "top", "width", "height"][k]).map(|x| self.scalar(x).and_then(|x| x.to_f64())).transpose()).collect::<R<_>>()?;
                let mut v = view_mut(&inst)?;
                if let Some(l) = nums[0] {
                    v.left = l as f32;
                }
                if let Some(t) = nums[1] {
                    v.top = t as f32;
                }
                if let Some(w) = nums[2] {
                    v.width = (w as f32 - 4.0).clamp(20.0, 4000.0);
                }
                if let Some(h) = nums[3] {
                    v.height = (h as f32 - 22.0).clamp(20.0, 4000.0);
                }
                return Ok(Value::Empty);
            }
            _ => return Err(Error::no_member(name)),
        };
        Ok(out)
    }

    pub(crate) fn form_set(&mut self, inst: Rc<Instance>, name: &str, v: Value) -> R<()> {
        let found = view(&inst)?.find(name);
        if let Some(i) = found {
            return self.control_set(inst, i, "value", &[], v);
        }
        let v = self.scalar(v)?;
        let mut f = view_mut(&inst)?;
        match name {
            "caption" => f.caption = v.to_str()?,
            "width" => f.width = (v.to_f64()? as f32 - 4.0).clamp(20.0, 4000.0),
            "height" => f.height = (v.to_f64()? as f32 - 22.0).clamp(20.0, 4000.0),
            "insidewidth" => f.width = (v.to_f64()? as f32).clamp(20.0, 4000.0),
            "insideheight" => f.height = (v.to_f64()? as f32).clamp(20.0, 4000.0),
            "left" => f.left = v.to_f64()? as f32,
            "top" => f.top = v.to_f64()? as f32,
            "startupposition" => f.start_up = v.to_i64()?.clamp(0, 3) as i32,
            "backcolor" => f.back = Some(v.to_i64()? as u32 & 0xFF_FFFF).filter(|c| c & 0xFF00_0000 == 0),
            "tag" | "enabled" => {}
            _ => return Err(Error::no_member(name)),
        }
        Ok(())
    }

    // --- controls -----------------------------------------------------------------

    /// `ctl.name(args)` of control `i`.
    pub(crate) fn control_member(&mut self, inst: Rc<Instance>, i: usize, name: &str, args: &[Arg]) -> R<Value> {
        let a = self.args(args)?;
        let arg = |it: &mut Self, k: usize, n: &str| -> R<Option<i64>> {
            match a.get(k, n) {
                Some(v) => Ok(Some(it.scalar(v)?.to_i64()?)),
                None => Ok(None),
            }
        };
        let (kind, list_len) = {
            let v = view(&inst)?;
            let it = v.items.get(i).ok_or_else(|| Error::app("The control is gone"))?;
            (it.c.kind, it.list.len())
        };
        // Methods first: they change the control.
        match name {
            "additem" => {
                let item = match a.get(0, "pvargitem") {
                    Some(v) => self.scalar(v)?.to_str()?,
                    None => String::new(),
                };
                let at = arg(self, 1, "pvargindex")?;
                let mut v = view_mut(&inst)?;
                let it = &mut v.items[i];
                if it.list.len() >= MAX_ROWS {
                    return Err(Error::app("The list is full"));
                }
                if !it.c.row_source.is_empty() {
                    return Err(Error::new(70, "Permission denied: the list comes from its RowSource"));
                }
                let at = match at {
                    Some(k) if (0..=it.list.len() as i64).contains(&k) => k as usize,
                    Some(_) => return Err(Error::invalid_call()),
                    None => it.list.len(),
                };
                it.list.insert(at, vec![item]);
                it.selected.insert(at.min(it.selected.len()), false);
                if it.list_index >= at as i32 {
                    it.list_index += 1;
                }
                return Ok(Value::Empty);
            }
            "removeitem" => {
                let k = arg(self, 0, "pvargindex")?.ok_or_else(Error::invalid_call)?;
                let mut v = view_mut(&inst)?;
                let it = &mut v.items[i];
                if k < 0 || k as usize >= it.list.len() {
                    return Err(Error::invalid_call());
                }
                it.list.remove(k as usize);
                if (k as usize) < it.selected.len() {
                    it.selected.remove(k as usize);
                }
                if it.list_index == k as i32 {
                    it.list_index = -1;
                } else if it.list_index > k as i32 {
                    it.list_index -= 1;
                }
                return Ok(Value::Empty);
            }
            "clear" => {
                let mut v = view_mut(&inst)?;
                let it = &mut v.items[i];
                if !it.c.row_source.is_empty() {
                    return Err(Error::new(70, "Permission denied: the list comes from its RowSource"));
                }
                it.list.clear();
                it.selected.clear();
                it.list_index = -1;
                if kind == Kind::ListBox {
                    it.c.value.clear();
                }
                return Ok(Value::Empty);
            }
            "setfocus" => {
                return self.move_focus(&inst, Some(i)).map(|_| Value::Empty);
            }
            "move" => {
                let nums: Vec<Option<f64>> = (0..4).map(|k| a.get(k, ["left", "top", "width", "height"][k]).map(|x| self.scalar(x).and_then(|x| x.to_f64())).transpose()).collect::<R<_>>()?;
                let mut v = view_mut(&inst)?;
                let c = &mut v.items[i].c;
                for (k, n) in nums.into_iter().enumerate() {
                    if let Some(n) = n {
                        let n = n as f32;
                        match k {
                            0 => c.left = n,
                            1 => c.top = n,
                            2 => c.width = n.max(0.0),
                            _ => c.height = n.max(0.0),
                        }
                    }
                }
                return Ok(Value::Empty);
            }
            "zorder" | "repaint" | "copy" | "cut" | "paste" | "dropdown" => return Ok(Value::Empty),
            "controls" => {
                let c = obj(Obj::Controls(inst, Some(i), false));
                return if args.is_empty() { Ok(c) } else { self.index_value(c, &args[..]) };
            }
            "pages" if kind == Kind::MultiPage => {
                let c = obj(Obj::Controls(inst, Some(i), true));
                return if args.is_empty() { Ok(c) } else { self.index_value(c, &args[..]) };
            }
            "selecteditem" if kind == Kind::MultiPage => {
                let v = view(&inst)?;
                let p = v.pages(i).get(v.items[i].list_index.max(0) as usize).copied();
                return Ok(p.map_or(Value::Nothing, |p| obj(Obj::Control(inst.clone(), p))));
            }
            "font" => return Ok(obj(Obj::ControlFont(inst, Some(i)))),
            "parent" => {
                let p = view(&inst)?.items[i].parent;
                return Ok(match p {
                    Some(p) => obj(Obj::Control(inst, p)),
                    None => obj(Obj::Instance(inst)),
                });
            }
            "object" => return Ok(obj(Obj::Control(inst, i))),
            "list" => {
                let r = arg(self, 0, "pvargindex")?;
                let c = arg(self, 1, "pvargcolumn")?;
                let v = view(&inst)?;
                let it = &v.items[i];
                return match (r, c) {
                    (Some(r), c) => {
                        if r < 0 || r as usize >= list_len {
                            return Err(Error::invalid_call());
                        }
                        Ok(cell_of(it, r as usize, c.unwrap_or(0).max(0) as usize))
                    }
                    (None, _) => {
                        if it.list.is_empty() {
                            return Ok(Value::Null);
                        }
                        let cols = it.list.iter().map(Vec::len).max().unwrap_or(1).max(it.c.column_count.max(1) as usize);
                        let mut data = Vec::with_capacity(it.list.len() * cols);
                        for r in 0..it.list.len() {
                            for c in 0..cols {
                                data.push(cell_of(it, r, c));
                            }
                        }
                        Ok(Value::Array(Rc::new(Array { bounds: vec![(0, it.list.len() as i64 - 1), (0, cols as i64 - 1)], data, ty: VType::Variant })))
                    }
                };
            }
            "column" => {
                let c = arg(self, 0, "pvargcolumn")?;
                let r = arg(self, 1, "pvargindex")?;
                let v = view(&inst)?;
                let it = &v.items[i];
                return match (c, r) {
                    (Some(c), Some(r)) if r >= 0 && (r as usize) < list_len => Ok(cell_of(it, r as usize, c.max(0) as usize)),
                    (Some(c), None) if it.list_index >= 0 => Ok(cell_of(it, it.list_index as usize, c.max(0) as usize)),
                    (Some(_), None) => Ok(Value::Null),
                    _ => Err(Error::invalid_call()),
                };
            }
            "selected" => {
                let r = arg(self, 0, "pvargindex")?.ok_or_else(Error::invalid_call)?;
                let v = view(&inst)?;
                let it = &v.items[i];
                if r < 0 || r as usize >= list_len {
                    return Err(Error::invalid_call());
                }
                let on = if it.c.multi_select == 0 { it.list_index == r as i32 } else { it.selected.get(r as usize).copied().unwrap_or(false) };
                return Ok(Value::Bool(on));
            }
            _ => {}
        }
        if !args.is_empty() {
            let v = self.control_member(inst, i, name, &[])?;
            return self.index_value(v, args);
        }
        let v = view(&inst)?;
        let it = &v.items[i];
        let c = &it.c;
        Ok(match name {
            "name" => Value::str(c.name.clone()),
            "value" | "_default" => self.control_value(&v, i),
            "text" => match kind {
                Kind::ListBox if it.list_index >= 0 => Value::str(v.row_text(i, it.list_index as usize)),
                Kind::ListBox => Value::str(""),
                Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton | Kind::Label | Kind::CommandButton => Value::str(c.caption.clone()),
                _ => Value::str(c.value.clone()),
            },
            "caption" => Value::str(c.caption.clone()),
            "visible" => Value::Bool(c.visible),
            "enabled" => Value::Bool(c.enabled),
            "locked" => Value::Bool(c.locked),
            "left" => Value::Double(c.left as f64),
            "top" => Value::Double(c.top as f64),
            "width" => Value::Double(c.width as f64),
            "height" => Value::Double(c.height as f64),
            "tag" => Value::str(c.tag.clone()),
            "controltiptext" => Value::str(c.tip.clone()),
            "tabindex" => Value::Long(c.tab_index),
            "tabstop" => Value::Bool(c.tab_stop),
            "default" => Value::Bool(c.default),
            "cancel" => Value::Bool(c.cancel),
            "groupname" => Value::str(c.group.clone()),
            "controlsource" => Value::str(c.control_source.clone()),
            "rowsource" => Value::str(c.row_source.clone()),
            "multiline" => Value::Bool(c.multiline),
            "wordwrap" => Value::Bool(c.word_wrap),
            "min" => Value::Long(c.min),
            "max" => Value::Long(c.max),
            "smallchange" => Value::Long(c.small_change),
            "largechange" => Value::Long(c.large_change),
            "maxlength" => Value::Long(c.max_length),
            "passwordchar" => Value::str(c.password_char.clone()),
            "columncount" => Value::Long(c.column_count),
            "multiselect" => Value::Long(c.multi_select as i32),
            "style" => Value::Long(if c.drop_down_list { 2 } else { 0 }),
            "forecolor" => Value::Long(c.fore.map_or(0x8000_0012_u32 as i32, |x| x as i32)),
            "backcolor" => Value::Long(c.back.map_or(0x8000_0005_u32 as i32, |x| x as i32)),
            "listindex" => Value::Long(it.list_index),
            "listcount" => Value::Long(it.list.len() as i32),
            "textlength" => Value::Long(c.value.chars().count() as i32),
            "linecount" => Value::Long(c.value.lines().count().max(1) as i32),
            "seltext" => Value::str(""),
            "selstart" => Value::Long(c.value.chars().count() as i32),
            "sellength" => Value::Long(0),
            "count" if kind == Kind::MultiPage => Value::Long(v.pages(i).len() as i32),
            "autosize" | "boundcolumn" | "textcolumn" | "columnheads" | "matchrequired" | "integralheight" | "specialeffect" | "borderstyle" | "picturesizemode" | "mousepointer" | "accelerator" | "takefocusonclick" | "enterkeybehavior" | "tabkeybehavior" | "scrollbars" | "matchentry" | "liststyle" | "orientation" | "delay" | "columnwidths" | "selectionmargin" | "autotab" | "autowordselect" | "hideselection" | "textalign" | "backstyle" | "bordercolor" | "picture" | "index" => Value::Empty,
            _ => return Err(Error::no_member(name)),
        })
    }

    /// The Value of control `i`.
    fn control_value(&self, v: &View, i: usize) -> Value {
        let it = &v.items[i];
        match it.c.kind {
            Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton => {
                if it.c.value.is_empty() && it.c.kind != Kind::CheckBox {
                    Value::Bool(false)
                } else if it.c.value.is_empty() {
                    Value::Bool(false)
                } else {
                    Value::Bool(checked(&it.c.value))
                }
            }
            Kind::SpinButton | Kind::ScrollBar => Value::Long(it.c.value.parse().unwrap_or(0)),
            Kind::MultiPage | Kind::TabStrip => Value::Long(it.list_index.max(0)),
            Kind::ListBox if it.c.multi_select != 0 => Value::Null,
            Kind::ListBox if it.list_index < 0 => Value::Null,
            Kind::ListBox => Value::str(v.row_text(i, it.list_index as usize)),
            Kind::CommandButton => Value::Bool(false),
            Kind::Label | Kind::Frame | Kind::Page => Value::str(it.c.caption.clone()),
            _ => Value::str(it.c.value.clone()),
        }
    }

    /// `ctl.name(args) = v` of control `i`.
    pub(crate) fn control_set(&mut self, inst: Rc<Instance>, i: usize, name: &str, args: &[Arg], v: Value) -> R<()> {
        let kind = view(&inst)?.items.get(i).ok_or_else(|| Error::app("The control is gone"))?.c.kind;
        // `.List = array`, `.List(r, c) = v`, `.Selected(r) = b`.
        match name {
            "list" if args.is_empty() => return self.set_list(&inst, i, v),
            "list" | "column" => {
                let a = self.args(args)?;
                let mut k = Vec::new();
                for n in 0..2 {
                    k.push(match a.get(n, "index") {
                        Some(x) => Some(self.scalar(x)?.to_i64()?),
                        None => None,
                    });
                }
                let (r, c) = if name == "list" { (k[0], k[1].unwrap_or(0)) } else { (k[1], k[0].unwrap_or(0)) };
                let r = r.ok_or_else(Error::invalid_call)?;
                let text = self.scalar(v)?.to_str()?;
                let mut f = view_mut(&inst)?;
                let it = &mut f.items[i];
                if r < 0 || r as usize >= it.list.len() || !(0..MAX_COLS as i64).contains(&c) {
                    return Err(Error::invalid_call());
                }
                let row = &mut it.list[r as usize];
                if row.len() <= c as usize {
                    row.resize(c as usize + 1, String::new());
                }
                row[c as usize] = text;
                return Ok(());
            }
            "selected" => {
                let a = self.args(args)?;
                let r = match a.get(0, "index") {
                    Some(x) => self.scalar(x)?.to_i64()?,
                    None => return Err(Error::invalid_call()),
                };
                let on = self.scalar(v)?.to_bool()?;
                let (multi, len, mut sel) = {
                    let f = view(&inst)?;
                    let it = &f.items[i];
                    (it.c.multi_select != 0, it.list.len(), it.selected.clone())
                };
                if r < 0 || r as usize >= len {
                    return Err(Error::invalid_call());
                }
                if !multi {
                    return self.change(&inst, i, Change::Index(if on { r as i32 } else { -1 }));
                }
                sel.resize(len, false);
                sel[r as usize] = on;
                return self.change(&inst, i, Change::Selected(sel));
            }
            _ if !args.is_empty() => return Err(Error::no_member(name)),
            _ => {}
        }
        let v = self.scalar(v)?;
        match name {
            "value" | "_default" | "text" => {
                let ch = match kind {
                    Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton => Change::Checked(match &v {
                        Value::Null | Value::Empty => false,
                        v => v.to_bool()?,
                    }),
                    Kind::SpinButton | Kind::ScrollBar => Change::Number(v.to_i64()?.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
                    Kind::MultiPage | Kind::TabStrip => Change::Index(v.to_i64()?.clamp(0, 10_000) as i32),
                    Kind::ListBox => {
                        let t = match &v {
                            Value::Null | Value::Empty => None,
                            v => Some(v.to_str()?),
                        };
                        let f = view(&inst)?;
                        let r = t.and_then(|t| f.items[i].list.iter().position(|row| row.first() == Some(&t)));
                        drop(f);
                        Change::Index(r.map_or(-1, |r| r as i32))
                    }
                    Kind::Label | Kind::CommandButton | Kind::Frame | Kind::Page if name == "value" || name == "_default" => {
                        if kind == Kind::CommandButton {
                            return Ok(());
                        }
                        view_mut(&inst)?.items[i].c.caption = v.to_str()?;
                        return Ok(());
                    }
                    _ => Change::Text(match &v {
                        Value::Null | Value::Empty => String::new(),
                        v => v.to_str()?,
                    }),
                };
                return self.change(&inst, i, ch);
            }
            "listindex" => {
                let n = v.to_i64()?;
                let len = view(&inst)?.items[i].list.len() as i64;
                if n < -1 || n >= len {
                    return Err(Error::new(380, "Could not set the ListIndex property. Invalid property value."));
                }
                return self.change(&inst, i, Change::Index(n as i32));
            }
            "rowsource" => {
                let t = v.to_str()?;
                view_mut(&inst)?.items[i].c.row_source = t.clone();
                if t.trim().is_empty() {
                    let mut f = view_mut(&inst)?;
                    f.items[i].list.clear();
                    f.items[i].list_index = -1;
                    return Ok(());
                }
                return self.fill_from_row_source(&inst, i, &t);
            }
            "controlsource" => {
                let t = v.to_str()?;
                view_mut(&inst)?.items[i].c.control_source = t.clone();
                if !t.trim().is_empty() {
                    let text = self.source_text(&t)?;
                    let mut f = view_mut(&inst)?;
                    f.items[i].c.value = match kind {
                        Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton => (if checked(&text) { "1" } else { "0" }).into(),
                        _ => text,
                    };
                }
                return Ok(());
            }
            _ => {}
        }
        let mut f = view_mut(&inst)?;
        let c = &mut f.items[i].c;
        let num = |v: &Value| -> R<f32> { Ok(v.to_f64()? as f32) };
        match name {
            "caption" => c.caption = v.to_str()?,
            "visible" => c.visible = v.to_bool()?,
            "enabled" => c.enabled = v.to_bool()?,
            "locked" => c.locked = v.to_bool()?,
            "left" => c.left = num(&v)?,
            "top" => c.top = num(&v)?,
            "width" => c.width = num(&v)?.max(0.0),
            "height" => c.height = num(&v)?.max(0.0),
            "tag" => c.tag = v.to_str()?,
            "controltiptext" => c.tip = v.to_str()?,
            "tabindex" => c.tab_index = v.to_i64()?.clamp(0, 10_000) as i32,
            "tabstop" => c.tab_stop = v.to_bool()?,
            "default" => c.default = v.to_bool()?,
            "cancel" => c.cancel = v.to_bool()?,
            "groupname" => c.group = v.to_str()?,
            "multiline" => c.multiline = v.to_bool()?,
            "wordwrap" => c.word_wrap = v.to_bool()?,
            "min" => c.min = v.to_i64()?.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            "max" => c.max = v.to_i64()?.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            "smallchange" => c.small_change = v.to_i64()?.clamp(1, i32::MAX as i64) as i32,
            "largechange" => c.large_change = v.to_i64()?.clamp(1, i32::MAX as i64) as i32,
            "maxlength" => c.max_length = v.to_i64()?.clamp(0, i32::MAX as i64) as i32,
            "passwordchar" => c.password_char = v.to_str()?.chars().take(1).collect(),
            "columncount" => c.column_count = v.to_i64()?.clamp(-1, MAX_COLS as i64) as i32,
            "multiselect" => c.multi_select = v.to_i64()?.clamp(0, 2) as u8,
            "style" => c.drop_down_list = v.to_i64()? == 2,
            "forecolor" => c.fore = Some(v.to_i64()? as u32).filter(|x| x & 0xFF00_0000 == 0),
            "backcolor" => c.back = Some(v.to_i64()? as u32).filter(|x| x & 0xFF00_0000 == 0),
            "seltext" => {
                let t = v.to_str()?;
                c.value.push_str(&t);
            }
            "selstart" | "sellength" | "autosize" | "boundcolumn" | "textcolumn" | "columnheads" | "matchrequired" | "integralheight" | "specialeffect" | "borderstyle" | "picturesizemode" | "mousepointer" | "accelerator" | "takefocusonclick" | "enterkeybehavior" | "tabkeybehavior" | "scrollbars" | "matchentry" | "liststyle" | "orientation" | "delay" | "columnwidths" | "selectionmargin" | "autotab" | "autowordselect" | "hideselection" | "textalign" | "backstyle" | "bordercolor" | "picture" | "index" => {}
            _ => return Err(Error::no_member(name)),
        }
        Ok(())
    }

    /// `.List = array`: the rows of a 1-D array, or of a 2-D one with its
    /// columns.
    fn set_list(&mut self, inst: &Rc<Instance>, i: usize, v: Value) -> R<()> {
        let v = match v {
            v @ Value::Object(Obj::Range(_)) => self.scalar(v)?,
            v => v,
        };
        let rows: Vec<Vec<String>> = match &v {
            Value::Array(a) => {
                let dims: Vec<usize> = a.bounds.iter().map(|(lo, hi)| (hi - lo + 1).max(0) as usize).collect();
                let text = |x: &Value| -> String {
                    match x {
                        Value::Empty | Value::Null => String::new(),
                        x => x.to_str().unwrap_or_default(),
                    }
                };
                match dims.len() {
                    1 => a.data.iter().take(MAX_ROWS).map(|x| vec![text(x)]).collect(),
                    2 => {
                        let (r, c) = (dims[0], dims[1]);
                        (0..r.min(MAX_ROWS)).map(|row| (0..c.min(MAX_COLS)).map(|col| text(&a.data[row * c + col])).collect()).collect()
                    }
                    _ => return Err(Error::type_mismatch()),
                }
            }
            Value::Empty | Value::Null => Vec::new(),
            other => vec![vec![other.to_str()?]],
        };
        let mut f = view_mut(inst)?;
        let it = &mut f.items[i];
        if !it.c.row_source.is_empty() {
            return Err(Error::new(70, "Permission denied: the list comes from its RowSource"));
        }
        it.selected = vec![false; rows.len()];
        it.list = rows;
        it.list_index = -1;
        Ok(())
    }

    // --- Controls and Pages -------------------------------------------------------

    /// The controls of a Controls (or the pages of a Pages).
    fn controls_of(&self, inst: &Instance, parent: Option<usize>, pages: bool) -> R<Vec<usize>> {
        let v = view(inst)?;
        Ok(match (pages, parent) {
            (true, Some(p)) => v.pages(p),
            (_, Some(p)) => v.within(Some(p)),
            (_, None) => v.within(None),
        })
    }

    pub(crate) fn controls_items(&self, inst: &Rc<Instance>, parent: Option<usize>, pages: bool) -> R<Vec<Value>> {
        Ok(self.controls_of(inst, parent, pages)?.into_iter().map(|i| obj(Obj::Control(inst.clone(), i))).collect())
    }

    pub(crate) fn controls_member(&mut self, inst: Rc<Instance>, parent: Option<usize>, pages: bool, name: &str, args: &[Arg]) -> R<Value> {
        let a = self.args(args)?;
        let list = self.controls_of(&inst, parent, pages)?;
        match name {
            "count" => Ok(Value::Long(list.len() as i32)),
            "item" | "_default" => {
                let k = a.get(0, "varg").ok_or_else(Error::invalid_call)?;
                let i = self.pick_control(&inst, &list, k)?;
                Ok(obj(Obj::Control(inst, i)))
            }
            "add" => {
                let (kind, name) = if pages {
                    (Kind::Page, self.arg_str(&a, 0, "bstrname")?.unwrap_or_default())
                } else {
                    let prog = self.arg_str(&a, 0, "bstrprogid")?.ok_or_else(Error::invalid_call)?;
                    let kind = Kind::from_prog_id(&prog).ok_or_else(|| Error::app(format!("Invalid class string: {prog}")))?;
                    (kind, self.arg_str(&a, 1, "name")?.unwrap_or_default())
                };
                let caption = if pages { self.arg_str(&a, 1, "bstrcaption")? } else { None };
                let visible = match a.get(2, "visible") {
                    Some(v) if !pages => self.scalar(v)?.to_bool()?,
                    _ => true,
                };
                let mut f = view_mut(&inst)?;
                if f.items.len() >= MAX_ADDED + 2000 {
                    return Err(Error::app("Too many controls"));
                }
                let name = if name.is_empty() {
                    let base = kind.type_name();
                    (1..).map(|n| format!("{base}{n}")).find(|n| f.find(n).is_none()).unwrap_or_default()
                } else if f.find(&name).is_some() {
                    return Err(Error::app(format!("Ambiguous name detected: {name}")));
                } else {
                    name
                };
                let mut c = Control::new(kind, &name);
                c.visible = visible;
                if kind == Kind::Page {
                    c.caption = caption.unwrap_or_else(|| name.clone());
                } else if matches!(kind, Kind::Label | Kind::CommandButton | Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton | Kind::Frame) {
                    c.caption = name.clone();
                }
                if matches!(kind, Kind::SpinButton | Kind::ScrollBar) {
                    c.value = "0".into();
                }
                f.items.push(Item { c, parent, list: Vec::new(), list_index: if kind == Kind::MultiPage { 0 } else { -1 }, selected: Vec::new(), gone: false, dirty: false });
                let mi = inst.module;
                let h = self.form_handles(mi, &name);
                f.handles.push(h);
                let i = f.items.len() - 1;
                drop(f);
                Ok(obj(Obj::Control(inst, i)))
            }
            "remove" => {
                let k = a.get(0, "varg").ok_or_else(Error::invalid_call)?;
                let i = self.pick_control(&inst, &list, k)?;
                let mut f = view_mut(&inst)?;
                let gone: Vec<usize> = (0..f.items.len()).filter(|&k| k == i || f.is_inside(k, Some(i))).collect();
                for k in gone {
                    f.items[k].gone = true;
                }
                if f.focus.is_some_and(|x| f.items[x].gone) {
                    f.focus = None;
                }
                Ok(Value::Empty)
            }
            "clear" => {
                let mut f = view_mut(&inst)?;
                for k in list {
                    f.items[k].gone = true;
                }
                f.focus = None;
                Ok(Value::Empty)
            }
            _ => Err(Error::no_member(name)),
        }
    }

    /// The control a Controls' index names: a name, or a number from 0.
    fn pick_control(&mut self, inst: &Instance, list: &[usize], k: Value) -> R<usize> {
        match self.scalar(k)? {
            Value::Str(n) => {
                let v = view(inst)?;
                list.iter().copied().find(|&i| v.items[i].c.name.eq_ignore_ascii_case(&n)).ok_or_else(|| Error::app(format!("Could not find the specified object: {n}")))
            }
            v => {
                let n = v.to_i64()?;
                usize::try_from(n).ok().and_then(|n| list.get(n).copied()).ok_or_else(|| Error::new(-2147024809_i32, "Could not find the specified object"))
            }
        }
    }

    // --- fonts --------------------------------------------------------------------

    pub(crate) fn control_font_get(&self, inst: &Instance, i: Option<usize>, name: &str) -> R<Value> {
        let v = view(inst)?;
        let Some(c) = i.and_then(|i| v.items.get(i)).map(|it| &it.c) else {
            return Ok(match name {
                "size" => Value::Double(8.25),
                "bold" | "italic" | "underline" | "strikethrough" => Value::Bool(false),
                "name" => Value::str("Tahoma"),
                _ => return Err(Error::no_member(name)),
            });
        };
        Ok(match name {
            "size" => Value::Double(c.font_size as f64),
            "bold" => Value::Bool(c.bold),
            "italic" => Value::Bool(c.italic),
            "underline" | "strikethrough" => Value::Bool(false),
            "name" => Value::str("Tahoma"),
            "weight" => Value::Long(if c.bold { 700 } else { 400 }),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(crate) fn control_font_set(&mut self, inst: &Instance, i: Option<usize>, name: &str, v: Value) -> R<()> {
        let v = self.scalar(v)?;
        let mut f = view_mut(inst)?;
        let Some(c) = i.and_then(|i| f.items.get_mut(i)).map(|it| &mut it.c) else { return Ok(()) };
        match name {
            "size" => c.font_size = (v.to_f64()? as f32).clamp(1.0, 400.0),
            "bold" => c.bold = v.to_bool()?,
            "italic" => c.italic = v.to_bool()?,
            "weight" => c.bold = v.to_i64()? >= 600,
            "name" | "underline" | "strikethrough" | "charset" => {}
            _ => return Err(Error::no_member(name)),
        }
        Ok(())
    }

    /// `Load form` and `Unload form`.
    pub(crate) fn load_statement(&mut self, unload: bool, args: &[Arg]) -> R<Value> {
        let a = self.args(args)?;
        let target = a.get(0, "object").ok_or_else(Error::invalid_call)?;
        let Value::Object(Obj::Instance(inst)) = target else {
            return Err(Error::new(361, "Can't load or unload this object"));
        };
        if inst.form.is_none() {
            return Err(Error::new(361, "Can't load or unload this object"));
        }
        if unload {
            // Unload Me in its own event: the rest of it still runs.
            self.unload_form(&inst, Some(CLOSE_CODE))?;
        }
        Ok(Value::Empty)
    }
}
