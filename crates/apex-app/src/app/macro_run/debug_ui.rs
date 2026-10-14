//! The debugger: breakpoints and watches kept per tab, the Immediate
//! window's lines, and the window shown while a macro is paused (its code
//! with the line running, the step buttons and keys of the VBA editor,
//! Locals, Watches, Immediate and the call stack). The sheet behind shows
//! the workbook as the macro left it so far.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::Duration;

use apex_vba::debug::{Breakpoint, DebugCmd, Pause, Reason, VarView, Watch, WatchKind};
use eframe::egui::{self, RichText};

use super::super::App;
use super::super::macros_ui::{Marks, code_area, cursor_row};
use crate::i18n::{tr, trf};

/// Lines the Immediate window keeps.
const MAX_LOG: usize = 2000;

/// Breakpoints, watches and the Immediate window of a tab.
#[derive(Default)]
pub(in crate::app) struct DebugConf {
    pub breakpoints: Vec<Breakpoint>,
    pub watches: Vec<Watch>,
    pub log: Vec<String>,
    /// Lines typed in the Immediate window, for Up and Down.
    pub history: Vec<String>,
}

impl DebugConf {
    /// The lines of `module` with a breakpoint.
    pub fn lines(&self, module: &str) -> Vec<u32> {
        self.breakpoints.iter().filter(|b| b.module.eq_ignore_ascii_case(module)).map(|b| b.line).collect()
    }

    pub fn toggle(&mut self, module: &str, line: u32) {
        let at = self.breakpoints.iter().position(|b| b.line == line && b.module.eq_ignore_ascii_case(module));
        match at {
            Some(i) => {
                self.breakpoints.remove(i);
            }
            None => self.breakpoints.push(Breakpoint { module: module.to_string(), line, condition: None }),
        }
    }

    pub fn print(&mut self, text: &str) {
        self.log.extend(text.split('\n').map(|l| l.trim_end_matches('\r').to_string()));
        let extra = self.log.len().saturating_sub(MAX_LOG);
        self.log.drain(..extra);
    }

    /// `delta` lines were added (or removed) after line `after` of
    /// `module`: its breakpoints below move with their code; those on
    /// removed lines go.
    pub fn shift(&mut self, module: &str, after: u32, delta: i64) {
        if delta == 0 {
            return;
        }
        self.breakpoints.retain_mut(|b| {
            if !b.module.eq_ignore_ascii_case(module) || b.line <= after {
                return true;
            }
            let line = b.line as i64 + delta;
            if line <= after as i64 {
                return false;
            }
            b.line = line as u32;
            true
        });
    }

    /// Renames the module of breakpoints (the module was renamed).
    pub fn rename(&mut self, old: &str, new: &str) {
        for b in &mut self.breakpoints {
            if b.module.eq_ignore_ascii_case(old) {
                b.module = new.to_string();
            }
        }
    }
}

/// The macro, paused, waiting for a command.
pub(super) struct Stopped {
    pub at: Box<Pause>,
    pub reply: mpsc::Sender<DebugCmd>,
    /// An error: "Debug" was chosen (until then, only the error shows).
    pub debugging: bool,
    /// A command was sent: shown (inactive) until the next pause, or until
    /// the macro has run a moment.
    pub sent: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Pane {
    #[default]
    Locals,
    Watches,
    Immediate,
    Stack,
}

/// The debugger window's state, kept from pause to pause.
#[derive(Default)]
pub(super) struct Panel {
    pane: Pane,
    input: String,
    focus_input: bool,
    /// Position in the history (Up and Down).
    hist_at: Option<usize>,
    /// A caller picked in the call stack (0: the line running).
    frame: usize,
    scroll: Option<usize>,
    /// Paths of the variables opened (arrays, collections).
    expanded: HashSet<String>,
    new_watch: String,
    watch_kind: usize,
    /// The pause the code was last scrolled to.
    shown: Option<(String, u32)>,
}

/// The watch kinds, in the order the list shows them.
const KINDS: [(WatchKind, &str); 3] = [(WatchKind::Watch, "debug.watch_expr"), (WatchKind::BreakWhenTrue, "debug.watch_true"), (WatchKind::BreakWhenChanged, "debug.watch_change")];

/// Names as the code writes them (the interpreter has them lower case).
pub(super) fn casing(sources: &[(String, String)]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (_, code) in sources {
        let mut word = String::new();
        let mut in_str = false;
        for c in code.chars().chain(std::iter::once(' ')) {
            if c == '"' {
                in_str = !in_str;
            }
            if !in_str && (c.is_alphanumeric() || c == '_') {
                word.push(c);
                continue;
            }
            if c == '\n' {
                in_str = false;
            }
            if word.starts_with(|c: char| c.is_alphabetic()) {
                out.entry(word.to_lowercase()).or_insert_with(|| word.clone());
            }
            word.clear();
        }
    }
    out
}

/// What the macro stopped for, as the window's title says it.
fn reason_text(at: &Pause) -> String {
    let place = trf("debug.paused_at", &[("module", &at.module), ("line", &at.line)]);
    let why = match &at.reason {
        Reason::Breakpoint => tr("debug.why_breakpoint"),
        Reason::Step => return place,
        Reason::Stop => tr("debug.why_stop"),
        Reason::Assert => tr("debug.why_assert"),
        Reason::Watch(i) => return format!("{place} — {}", trf("debug.why_watch", &[("n", &(i + 1))])),
        Reason::Error(e) => return format!("{place} — {}", trf("debug.runtime_error", &[("number", &e.number)])),
    };
    format!("{place} — {why}")
}

/// A variable's row and, when open, those of its elements.
/// Variables in columns: name, value, type.
fn vars_grid(ui: &mut egui::Ui, id: &str, vars: &[VarView], path: &str, expanded: &mut HashSet<String>, names: &HashMap<String, String>) {
    egui::Grid::new(id).num_columns(3).spacing([16.0, 4.0]).striped(true).show(ui, |ui| {
        for v in vars {
            var_rows(ui, v, path, 0, expanded, names);
        }
    });
}

fn var_rows(ui: &mut egui::Ui, v: &VarView, path: &str, depth: usize, expanded: &mut HashSet<String>, names: &HashMap<String, String>) {
    let path = format!("{path}/{}", v.name);
    let open = expanded.contains(&path);
    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 14.0);
        let (rect, r) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), if v.children.is_empty() { egui::Sense::hover() } else { egui::Sense::click() });
        if !v.children.is_empty() {
            egui::collapsing_header::paint_default_icon(ui, if open { 1.0 } else { 0.0 }, &r.clone().with_new_rect(rect));
            if r.clicked() {
                if open {
                    expanded.remove(&path);
                } else {
                    expanded.insert(path.clone());
                }
            }
        }
        let name = if depth == 0 { names.get(&v.name).cloned().unwrap_or_else(|| v.name.clone()) } else { v.name.clone() };
        ui.label(RichText::new(name).monospace());
    });
    // Long strings cut, whole on hover.
    let value = match v.value.char_indices().nth(80) {
        Some((at, _)) => format!("{}…", &v.value[..at]),
        None => v.value.clone(),
    };
    ui.label(RichText::new(value).monospace()).on_hover_text(&v.value);
    ui.label(RichText::new(&v.ty).weak());
    ui.end_row();
    if open {
        for c in &v.children {
            var_rows(ui, c, &path, depth + 1, expanded, names);
        }
    }
}

/// The watches: expression, how it stops the macro, and (paused) its
/// value. Returns whether the list changed.
pub(in crate::app) fn watch_list(ui: &mut egui::Ui, watches: &mut Vec<Watch>, values: Option<&[VarView]>, new_watch: &mut String, kind: &mut usize) -> bool {
    let mut changed = false;
    let mut remove = None;
    egui::ScrollArea::vertical().id_salt("debug_watches").auto_shrink([false, true]).max_height((ui.available_height() - 34.0).max(60.0)).show(ui, |ui| {
        if watches.is_empty() {
            ui.label(RichText::new(tr("debug.no_watches")).weak());
        }
        for (i, w) in watches.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button("✕").on_hover_text(tr("debug.watch_remove")).clicked() {
                    remove = Some(i);
                }
                let icon = match w.kind {
                    WatchKind::Watch => "👓",
                    WatchKind::BreakWhenTrue => "⏸",
                    WatchKind::BreakWhenChanged => "Δ",
                };
                ui.label(icon).on_hover_text(tr(KINDS.iter().find(|k| k.0 == w.kind).map_or("", |k| k.1)));
                ui.add_sized([160.0, 18.0], egui::Label::new(RichText::new(&w.expr).monospace()).truncate());
                if let Some(v) = values.and_then(|v| v.get(i)) {
                    let color = if v.ty.is_empty() { ui.visuals().weak_text_color() } else { ui.visuals().text_color() };
                    ui.add_sized([180.0, 18.0], egui::Label::new(RichText::new(&v.value).monospace().color(color)).truncate()).on_hover_text(&v.value);
                    ui.label(RichText::new(&v.ty).weak());
                }
            });
        }
    });
    if let Some(i) = remove {
        watches.remove(i);
        changed = true;
    }
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(new_watch).hint_text(tr("debug.watch_hint")).desired_width(180.0).font(egui::TextStyle::Monospace));
        egui::ComboBox::from_id_salt("debug_watch_kind").selected_text(tr(KINDS[(*kind).min(2)].1)).show_ui(ui, |ui| {
            for (i, (_, key)) in KINDS.iter().enumerate() {
                ui.selectable_value(kind, i, tr(key));
            }
        });
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.add_enabled(!new_watch.trim().is_empty(), egui::Button::new(tr("debug.watch_add"))).clicked() || enter) && !new_watch.trim().is_empty() {
            watches.push(Watch { expr: new_watch.trim().to_string(), kind: KINDS[(*kind).min(2)].0 });
            new_watch.clear();
            changed = true;
            if enter {
                r.request_focus();
            }
        }
    });
    changed
}

/// The Immediate window: its lines and the line to type. Returns a line
/// entered.
pub(in crate::app) fn immediate_pane(ui: &mut egui::Ui, conf: &mut DebugConf, input: &mut String, focus: &mut bool, hist_at: &mut Option<usize>, enabled: bool) -> Option<String> {
    let mut entered = None;
    // The room left by the frame and the line to type (no more: the
    // editor's window would grow every frame).
    let style = ui.style();
    let below = style.spacing.interact_size.y + 2.0 * style.spacing.item_spacing.y + 16.0;
    let h = (ui.available_height() - below).max(40.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt("debug_immediate").stick_to_bottom(true).auto_shrink([false, false]).max_height(h).show(ui, |ui| {
            let text = conf.log.join("\n");
            ui.add(egui::Label::new(RichText::new(text).monospace()).selectable(true));
        });
    });
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            let r = ui.add(egui::TextEdit::singleline(input).id(egui::Id::new("debug_immediate_input")).hint_text(tr("debug.immediate_hint")).desired_width(ui.available_width() - 70.0).font(egui::TextStyle::Monospace));
            if std::mem::take(focus) {
                r.request_focus();
            }
            if r.has_focus() {
                let (up, down) = ui.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
                if (up || down) && !conf.history.is_empty() {
                    let n = conf.history.len();
                    let at = match (*hist_at, up) {
                        (None, true) => Some(n - 1),
                        (None, false) => None,
                        (Some(i), true) => Some(i.saturating_sub(1)),
                        (Some(i), false) => (i + 1 < n).then_some(i + 1),
                    };
                    *hist_at = at;
                    *input = at.map(|i| conf.history[i].clone()).unwrap_or_default();
                }
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button(tr("debug.immediate_run")).clicked() || enter) && !input.trim().is_empty() {
                let line = std::mem::take(input);
                conf.print(&line);
                if conf.history.last() != Some(&line) {
                    conf.history.push(line.clone());
                }
                *hist_at = None;
                entered = Some(line);
                r.request_focus();
            }
        });
    });
    entered
}

impl App {
    /// The debugger, while the macro is paused; whether it showed.
    pub(super) fn debugger_ui(&mut self, ctx: &egui::Context) -> bool {
        let Some(job) = self.macro_run.job.as_mut() else { return false };
        let Some(stop) = job.pause.as_mut() else { return false };
        let tab = job.tab;
        // An error first asks, like Excel: Debug or End.
        if let (Reason::Error(e), false) = (&stop.at.reason, stop.debugging) {
            let mut cmd = None;
            let m = egui::Modal::new(egui::Id::new("macro_debug_error")).show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.set_max_width(540.0);
                ui.heading(tr("macros.error_title"));
                ui.add_space(8.0);
                ui.label(RichText::new(trf("debug.runtime_error", &[("number", &e.number)])).strong());
                ui.label(e.message.as_str());
                ui.label(RichText::new(trf("macros.error_at", &[("module", &e.module), ("line", &e.line)])).weak());
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("debug.end")).clicked() {
                        cmd = Some(false);
                    }
                    if ui.button(RichText::new(tr("debug.debug")).strong()).clicked() {
                        cmd = Some(true);
                    }
                });
            });
            if cmd.is_none() && m.is_top_modal {
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cmd = Some(false);
                } else if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    cmd = Some(true);
                }
            }
            match cmd {
                Some(true) => stop.debugging = true,
                Some(false) => {
                    let _ = stop.reply.send(DebugCmd::End);
                    job.pause = None;
                    job.resumed = std::time::Instant::now();
                }
                None => {}
            }
            return true;
        }
        let waiting = stop.sent;
        if waiting && job.resumed.elapsed() > Duration::from_millis(400) {
            // Running on: the running dialog shows.
            return false;
        }
        let conf = self.macro_run.debug.entry(tab).or_default();
        let at = &stop.at;
        let panel = &mut job.panel;
        let frame = panel.frame.min(at.stack.len().saturating_sub(1));
        let (module, line) = match at.stack.get(frame) {
            Some(f) if frame > 0 => (f.module.clone(), f.line),
            _ => (at.module.clone(), at.line),
        };
        let code = job.sources.iter().find(|(n, _)| n.eq_ignore_ascii_case(&module)).map(|s| s.1.clone()).unwrap_or_default();
        if panel.shown.as_ref() != Some(&(module.clone(), line)) {
            panel.shown = Some((module.clone(), line));
            panel.scroll = Some(line.saturating_sub(1) as usize);
        }
        let code_id = egui::Id::new(("debug_code", module.to_lowercase()));
        let names = &job.names;
        let mut cmd: Option<DebugCmd> = None;
        let mut toggled: Option<u32> = None;
        let mut watches_changed = false;
        let screen = ctx.content_rect();
        let size = egui::vec2((screen.width() - 24.0).clamp(320.0, 1180.0), (screen.height() * 0.58).clamp(240.0, 560.0));
        let area = egui::Modal::default_area(egui::Id::new("macro_debugger")).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -8.0));
        // The sheet stays visible behind (as the macro left it).
        let m = egui::Modal::new(egui::Id::new("macro_debugger")).area(area).backdrop_color(egui::Color32::from_black_alpha(25)).show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            if waiting {
                ui.disable();
            }
            ui.horizontal(|ui| {
                ui.heading(tr("debug.title"));
                ui.label(RichText::new(reason_text(at)).strong());
            });
            ui.horizontal_wrapped(|ui| {
                let mut b = |ui: &mut egui::Ui, label: &str, tip: &str, c: DebugCmd| {
                    if ui.button(label).on_hover_text(tip).clicked() {
                        cmd = Some(c);
                    }
                };
                b(ui, &format!("▶ {}", tr("debug.continue")), "F5", DebugCmd::Continue);
                b(ui, &tr("debug.step_into"), "F8", DebugCmd::StepInto);
                b(ui, &tr("debug.step_over"), &tr("debug.step_over_key"), DebugCmd::StepOver);
                b(ui, &tr("debug.step_out"), &tr("debug.step_out_key"), DebugCmd::StepOut);
                let row = cursor_row(ui.ctx(), code_id, &code) as u32 + 1;
                b(ui, &tr("debug.run_to"), &tr("debug.run_to_key"), DebugCmd::RunTo { module: module.clone(), line: row });
                if frame == 0 {
                    b(ui, &tr("debug.set_next"), &tr("debug.set_next_key"), DebugCmd::SetNext(row));
                }
                if ui.button(tr("debug.toggle_breakpoint")).on_hover_text("F9").clicked() {
                    toggled = Some(row);
                }
                b(ui, &format!("■ {}", tr("debug.reset")), &tr("debug.reset_tip"), DebugCmd::End);
            });
            ui.separator();
            let body_h = ui.available_height();
            ui.horizontal_top(|ui| {
                // The code, read only, with the line running.
                let code_w = (ui.available_width() * 0.55).max(240.0);
                ui.allocate_ui(egui::vec2(code_w, body_h), |ui| {
                    ui.set_min_size(egui::vec2(code_w, body_h));
                    let lines = conf.lines(&module);
                    let value_of = |w: &str| -> Option<String> {
                        let v = at.locals.iter().chain(&at.module_vars).find(|v| v.name == w)?;
                        let name = names.get(w).map_or(w, String::as_str);
                        Some(format!("{name} = {}", v.value))
                    };
                    let mut marks = Marks { breakpoints: &lines, current: (frame == 0).then_some(line), frame: (frame > 0).then_some(line), values: if frame == 0 { Some(&value_of) } else { None }, ..Default::default() };
                    code_area(ui, code_id, &mut code.as_str(), &mut panel.scroll, &mut marks);
                    if let Some(l) = marks.clicked {
                        toggled = Some(l);
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        for (p, key) in [(Pane::Locals, "debug.locals"), (Pane::Watches, "debug.watches"), (Pane::Immediate, "debug.immediate"), (Pane::Stack, "debug.stack")] {
                            if ui.selectable_label(panel.pane == p, tr(key)).clicked() {
                                panel.pane = p;
                                panel.focus_input = p == Pane::Immediate;
                            }
                        }
                    });
                    ui.separator();
                    match panel.pane {
                        Pane::Locals => {
                            egui::ScrollArea::vertical().id_salt("debug_locals").auto_shrink([false, false]).show(ui, |ui| {
                                if let Some(f) = at.stack.first() {
                                    ui.label(RichText::new(format!("{}.{}", f.module, f.proc)).weak());
                                }
                                if at.locals.is_empty() {
                                    ui.label(RichText::new(tr("debug.no_locals")).weak());
                                }
                                vars_grid(ui, "debug_locals_grid", &at.locals, "l", &mut panel.expanded, names);
                                if !at.module_vars.is_empty() {
                                    ui.add_space(6.0);
                                    ui.label(RichText::new(trf("debug.module_vars", &[("module", &at.module)])).weak());
                                    vars_grid(ui, "debug_module_grid", &at.module_vars, "m", &mut panel.expanded, names);
                                }
                            });
                        }
                        Pane::Watches => {
                            watches_changed = watch_list(ui, &mut conf.watches, Some(&at.watches), &mut panel.new_watch, &mut panel.watch_kind);
                        }
                        Pane::Immediate => {
                            if let Some(line) = immediate_pane(ui, conf, &mut panel.input, &mut panel.focus_input, &mut panel.hist_at, true) {
                                cmd = Some(DebugCmd::Eval(line));
                            }
                        }
                        Pane::Stack => {
                            egui::ScrollArea::vertical().id_salt("debug_stack").auto_shrink([false, false]).show(ui, |ui| {
                                for (i, f) in at.stack.iter().enumerate() {
                                    let text = trf("debug.stack_entry", &[("module", &f.module), ("proc", &f.proc), ("line", &f.line)]);
                                    if ui.selectable_label(i == frame, RichText::new(text).monospace()).clicked() {
                                        panel.frame = i;
                                    }
                                }
                            });
                        }
                    }
                });
            });
        });
        // The VBA editor's keys.
        if m.is_top_modal && cmd.is_none() && !waiting {
            let row = cursor_row(ctx, code_id, &code) as u32 + 1;
            ctx.input_mut(|i| {
                use egui::{Key, Modifiers as M};
                let cs = M::COMMAND | M::SHIFT;
                if i.consume_key(M::NONE, Key::F5) {
                    cmd = Some(DebugCmd::Continue);
                } else if i.consume_key(cs, Key::F8) {
                    cmd = Some(DebugCmd::StepOut);
                } else if i.consume_key(M::SHIFT, Key::F8) {
                    cmd = Some(DebugCmd::StepOver);
                } else if i.consume_key(M::COMMAND, Key::F8) {
                    cmd = Some(DebugCmd::RunTo { module: module.clone(), line: row });
                } else if i.consume_key(M::NONE, Key::F8) {
                    cmd = Some(DebugCmd::StepInto);
                } else if i.consume_key(M::COMMAND, Key::F9) && frame == 0 {
                    cmd = Some(DebugCmd::SetNext(row));
                } else if i.consume_key(M::NONE, Key::F9) {
                    toggled = Some(row);
                } else if i.consume_key(M::COMMAND, Key::G) {
                    panel.pane = Pane::Immediate;
                    panel.focus_input = true;
                } else if i.consume_key(M::COMMAND, Key::L) {
                    panel.pane = Pane::Stack;
                }
            });
        }
        if let Some(l) = toggled {
            conf.toggle(&module, l);
            cmd = cmd.or(Some(DebugCmd::Breakpoints(conf.breakpoints.clone())));
        }
        if watches_changed {
            cmd = cmd.or(Some(DebugCmd::Watches(conf.watches.clone())));
        }
        if let Some(c) = cmd.filter(|_| !waiting) {
            // Moving on: back to the line running next time.
            if !matches!(c, DebugCmd::Eval(_) | DebugCmd::Breakpoints(_) | DebugCmd::Watches(_)) {
                panel.frame = 0;
            }
            let _ = stop.reply.send(c);
            stop.sent = true;
            job.resumed = std::time::Instant::now();
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoints_follow_their_code() {
        let mut c = DebugConf::default();
        c.toggle("Module1", 5);
        c.toggle("module1", 9);
        c.toggle("Sheet1", 9);
        assert_eq!(c.lines("MODULE1"), [5, 9]);
        c.toggle("Module1", 5);
        assert_eq!(c.lines("Module1"), [9]);
        // Two lines inserted after line 3: line 9 is now 11.
        c.shift("Module1", 3, 2);
        assert_eq!(c.lines("Module1"), [11]);
        assert_eq!(c.lines("Sheet1"), [9]);
        // Lines 4 to 11 removed: the breakpoint goes with them.
        c.shift("Module1", 3, -8);
        assert!(c.lines("Module1").is_empty());
        c.rename("Sheet1", "Hoja1");
        assert_eq!(c.lines("Hoja1"), [9]);
    }

    #[test]
    fn names_as_written() {
        let m = casing(&[("M".into(), "Sub Main()\n    Dim TotalRows As Long ' totalrows\n    s = \"Hidden\"\nEnd Sub\n".into())]);
        assert_eq!(m.get("totalrows").map(String::as_str), Some("TotalRows"));
        assert_eq!(m.get("main").map(String::as_str), Some("Main"));
        assert!(!m.contains_key("hidden"));
    }

    /// An app with a workbook whose Module1 has `code`, macros enabled.
    fn app_with(code: &str) -> (egui::Context, App, u64) {
        let ctx = egui::Context::default();
        let settings = crate::settings::Settings { macros_enabled: true, ..Default::default() };
        let mut app = App::blank(&ctx, settings, None, std::time::Instant::now());
        let bin = apex_io::vba::new_workbook_project("ThisWorkbook", &["Sheet1".to_string()]).unwrap();
        let mut p = apex_io::vba::read(&bin).unwrap();
        let mut m = apex_io::vba::VbaModule::standard("Module1");
        m.code = code.to_string();
        p.modules.push(m);
        let mut wb = apex_sheet::Workbook::new();
        wb.macros.set_project(p.write().unwrap());
        app.push_tab(super::super::View::Sheet(Box::new(crate::sheet_view::SheetView::new(wb))), None, None);
        app.active = 0;
        let id = app.tabs[0].id;
        app.enable_macros(id);
        (ctx, app, id)
    }

    #[test]
    fn organization_turns_the_editor_off() {
        let (_, mut app, _) = app_with("Sub A()\nEnd Sub\n");
        app.settings.macro_editor_off = true;
        app.open_macros();
        assert!(app.macros.editor.is_none());
        app.settings.macro_editor_off = false;
        app.open_macros();
        assert!(app.macros.editor.is_some());
    }

    #[test]
    fn own_macros_signed() {
        let (_, mut app, id) = app_with("Sub A()\n    Range(\"A1\").Value = 1\nEnd Sub\n");
        let data = |name: &str| std::fs::read(format!("{}/../apex-sign/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        // Only with a certificate for signing code.
        let alice = apex_sign::p12::read(&data("alice-aes.p12"), "secreto").unwrap();
        assert!(app.sign_macros(id, &alice).is_err());
        let carol = apex_sign::p12::read(&data("carol-code.p12"), "").unwrap();
        app.sign_macros(id, &carol).unwrap();
        let super::super::super::View::Sheet(v) = &app.tabs[0].view else { panic!() };
        assert!(v.wb.macros.signed && v.wb.modified);
        // Saved as .xlsm and read back, it is signed for its code by Carol,
        // whom the user may choose to trust.
        let flavor = apex_io::Flavor::from_path(std::path::Path::new("a.xlsm"));
        let wb = apex_io::xlsx::read(&apex_io::xlsx::write_as(&v.wb, flavor)).unwrap();
        assert!(wb.macros.signed);
        let s: Vec<_> = super::super::signers(&wb);
        assert!(s.len() == 1 && s[0].is_sound() && s[0].cert.der == carol.cert.der);
        assert!(s[0].is_trusted_with(&[carol.cert.fingerprint()], false));
        assert!(super::super::untrusted_publisher(&app.settings, &app.tabs, id).is_some_and(|c| c.der == carol.cert.der));
        // Editing the code drops the signature.
        assert!(app.edit_signed_macros_for_test(id, "Sub A()\nEnd Sub\n"));
        let super::super::super::View::Sheet(v) = &app.tabs[0].view else { panic!() };
        assert!(!v.wb.macros.signed && v.wb.macros.parts.is_empty());
    }

    #[test]
    fn a_documents_macros_signed() {
        let ctx = egui::Context::default();
        let mut app = App::blank(&ctx, crate::settings::Settings { macros_enabled: true, ..Default::default() }, None, std::time::Instant::now());
        let mut doc = apex_doc::Document::from_plain_text("Hola");
        let mut p = apex_io::vba::read(&apex_io::vba::new_document_project()).unwrap();
        let mut m = apex_io::vba::VbaModule::standard("Module1");
        m.code = "Sub A()\nEnd Sub\n".into();
        p.modules.push(m);
        doc.macros.set_project(p.write().unwrap());
        app.push_tab(super::super::View::Doc(Box::new(crate::doc_view::DocView::new(doc))), None, None);
        let id = app.tabs[0].id;
        let data = |name: &str| std::fs::read(format!("{}/../apex-sign/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let carol = apex_sign::p12::read(&data("carol-code.p12"), "").unwrap();
        app.sign_macros(id, &carol).unwrap();
        let super::super::super::View::Doc(v) = &app.tabs[0].view else { panic!() };
        assert!(v.doc.macros.signed);
        // Saved as .docm and read back, it is signed for its code by Carol.
        let flavor = apex_io::Flavor::from_path(std::path::Path::new("a.docm"));
        let d = apex_io::docx::read(&apex_io::docx::write_as(&v.doc.shared(), flavor)).unwrap();
        assert!(d.macros.signed);
        let s = super::super::macro_signers(&d.macros);
        assert!(s.len() == 1 && s[0].is_sound() && s[0].cert.der == carol.cert.der);
    }

    #[test]
    fn suspicious_macros_warn() {
        let (_, mut app, id) = app_with("Sub Auto_Open()\n    CreateObject(\"WScript.Shell\").Run \"powershell -nop\"\nEnd Sub\n");
        let text = super::super::super::macros_ui::danger_text(&app.macro_signals(id)).unwrap();
        assert!(text.contains(&crate::i18n::tr("macros.sig_programs")), "{text}");
        assert!(text.contains(&crate::i18n::tr("macros.sig_autorun")), "{text}");
        let (_, mut app, id) = app_with("Sub Auto_Open()\n    Range(\"A1\").Value = 1\nEnd Sub\n");
        assert_eq!(super::super::super::macros_ui::danger_text(&app.macro_signals(id)), None);
    }

    /// Polls the macro until it pauses (true) or ends (false).
    fn wait(app: &mut App) -> bool {
        let t0 = std::time::Instant::now();
        while t0.elapsed() < Duration::from_secs(20) {
            app.poll_macro();
            match &app.macro_run.job {
                None => return false,
                Some(j) if j.pause.as_ref().is_some_and(|p| !p.sent) => return true,
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        panic!("the macro neither paused nor ended");
    }

    fn a1(app: &App) -> String {
        let super::super::View::Sheet(v) = &app.tabs[0].view else { unreachable!() };
        v.wb.display(0, apex_sheet::CellRef::new(0, 0)).0
    }

    fn send(app: &mut App, cmd: DebugCmd) {
        let stop = app.macro_run.job.as_mut().and_then(|j| j.pause.as_mut()).expect("paused");
        stop.reply.send(cmd).unwrap();
        stop.sent = true;
    }

    #[test]
    fn paused_macro_shows_its_workbook() {
        let (ctx, mut app, id) = app_with("Sub Main()\n    Range(\"A1\").Value = 1\n    Debug.Print \"half\"\n    Range(\"A1\").Value = 2\nEnd Sub\n");
        app.macro_run.debug.entry(id).or_default().toggle("Module1", 4);
        app.start_macro(&ctx, id, super::super::Entry::Macro("Main".into()), None);
        assert!(wait(&mut app));
        // The sheet shows what the macro did so far; Debug.Print went to
        // the Immediate window.
        assert_eq!(a1(&app), "1");
        assert_eq!(app.macro_run.debug[&id].log, ["half"]);
        let at = &app.macro_run.job.as_ref().unwrap().pause.as_ref().unwrap().at;
        assert_eq!((at.module.as_str(), at.line, &at.reason), ("Module1", 4, &Reason::Breakpoint));
        // The Immediate window, then on to the end.
        send(&mut app, DebugCmd::Eval("?Range(\"A1\").Value + 10".into()));
        assert!(wait(&mut app));
        assert_eq!(app.macro_run.debug[&id].log, ["half", " 11 "]);
        send(&mut app, DebugCmd::Continue);
        assert!(!wait(&mut app));
        assert_eq!(a1(&app), "2");
        // One undo step, back to the workbook as it was.
        let super::super::View::Sheet(v) = &mut app.tabs[0].view else { unreachable!() };
        assert!(v.wb.undo());
        assert_eq!(a1(&app), "");
    }

    /// Runs `code`'s Main on a workbook saved in `dir`, allowing it to
    /// change files.
    fn run_in(dir: &std::path::Path, code: &str) -> (App, u64) {
        let (ctx, mut app, id) = app_with(code);
        app.tabs[0].path = Some(dir.join("Book1.xlsm"));
        app.tabs[0].format = Some(apex_io::Format::Xlsx);
        app.start_macro(&ctx, id, super::super::Entry::Macro("Main".into()), None);
        let t0 = std::time::Instant::now();
        while app.macro_run.job.is_some() {
            assert!(t0.elapsed() < Duration::from_secs(20), "the macro didn't end");
            app.poll_macro();
            if let Some(p) = app.macro_run.job.as_ref().and_then(|j| j.pause.as_ref()) {
                panic!("paused: {:?}", p.at.reason);
            }
            let job = app.macro_run.job.as_mut();
            if let Some(job) = job
                && let Some(super::super::Msg::AllowFiles { reply, .. }) = &job.ask
            {
                reply.send(true).unwrap();
                job.ask = None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        (app, id)
    }

    #[test]
    fn save_as_and_close() {
        let dir = std::env::temp_dir().join(format!("apex-saveas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (app, _) = run_in(&dir, "Sub Main()\n    Range(\"A1\").Value = 5\n    ThisWorkbook.SaveCopyAs ThisWorkbook.Path & \"/copy.xlsx\"\n    ThisWorkbook.SaveAs ThisWorkbook.Path & \"/new.ods\"\nEnd Sub\n");
        // The tab is the new file, saved; the copy is a workbook too.
        assert_eq!(app.tabs[0].path.as_deref(), Some(dir.join("new.ods").as_path()));
        assert_eq!(app.tabs[0].format, Some(apex_io::Format::Ods));
        assert!(!app.tab_modified(0));
        let Ok((apex_io::Loaded::Sheet(copy), _)) = apex_io::open(&dir.join("copy.xlsx")) else { panic!("no copy") };
        assert_eq!(copy.display(0, apex_sheet::CellRef::new(0, 0)).0, "5");
        assert!(dir.join("new.ods").is_file());
        // Close False drops the changes: the tab goes, nothing is saved.
        let (app, id) = run_in(&dir, "Sub Main()\n    Range(\"A1\").Value = 6\n    ThisWorkbook.Close False\n    Range(\"A1\").Value = 7\nEnd Sub\n");
        assert!(app.tabs.iter().all(|t| t.id != id));
        assert!(!dir.join("Book1.xlsm").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ending_at_an_error() {
        let (ctx, mut app, id) = app_with("Sub Main()\n    Range(\"A1\").Value = 7\n    x = 1 / 0\nEnd Sub\n");
        app.start_macro(&ctx, id, super::super::Entry::Macro("Main".into()), None);
        assert!(wait(&mut app));
        let at = &app.macro_run.job.as_ref().unwrap().pause.as_ref().unwrap().at;
        assert!(matches!(&at.reason, Reason::Error(e) if e.number == 11 && e.line == 3));
        send(&mut app, DebugCmd::End);
        assert!(!wait(&mut app));
        // Ended by the user: no error dialog; what it did stays.
        assert!(app.macro_run.failure.is_none());
        assert_eq!(a1(&app), "7");
    }

    #[test]
    fn stepping_from_the_start_and_ending_unchanged() {
        let (ctx, mut app, id) = app_with("Sub Main()\n    Dim x\n    x = 1\nEnd Sub\n");
        app.start_macro(&ctx, id, super::super::Entry::Step("Main".into()), None);
        assert!(wait(&mut app));
        assert_eq!(app.macro_run.job.as_ref().unwrap().pause.as_ref().unwrap().at.line, 3);
        send(&mut app, DebugCmd::StepInto);
        assert!(!wait(&mut app));
        // Nothing changed: the tab has its own workbook back, unmodified.
        let super::super::View::Sheet(v) = &app.tabs[0].view else { unreachable!() };
        assert!(!v.wb.modified);
        assert!(!v.wb.can_undo());
    }

    #[test]
    fn immediate_log_is_capped() {
        let mut c = DebugConf::default();
        for i in 0..MAX_LOG + 10 {
            c.print(&i.to_string());
        }
        assert_eq!(c.log.len(), MAX_LOG);
        assert_eq!(c.log[0], "10");
    }
}
