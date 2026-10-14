//! Keys that run a macro (Application.OnKey and the shortcuts of Macro
//! Options) and macros due at a time (Application.OnTime).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use apex_vba::{Key, Request};
use eframe::egui;

use super::{App, Entry, View, allowed};

/// Most macros waiting for their time per workbook.
const MAX_TIMERS: usize = 256;

/// OnTime: a macro to run at `due` (the date serial `at` the macro gave).
struct Timer {
    tab: u64,
    at: f64,
    due: Instant,
    /// Not after this (LatestTime); none: whenever the app is free.
    latest: Option<Instant>,
    procedure: String,
}

#[derive(Default)]
pub(super) struct Schedule {
    /// OnKey per tab: the macro of a key (`""`: the key does nothing).
    keys: HashMap<u64, Vec<(Key, String)>>,
    /// OnKey for macros of the personal workbook or an add-in
    /// (`Book!Macro`): in every workbook.
    shared_keys: Vec<(Key, String)>,
    /// The shortcuts in the code per tab, and the project they came from.
    shortcuts: HashMap<u64, (usize, Vec<(Key, String)>)>,
    timers: Vec<Timer>,
    /// Macro Options.
    dialog: Option<ShortcutDlg>,
    /// The macros of each tab's code (for buttons), and its project.
    names: HashMap<u64, (usize, Vec<String>)>,
}

/// The local time now as a date serial (what Now gives a macro).
fn now_serial() -> f64 {
    apex_sheet::format::now_serial() + crate::common::local_utc_offset() as f64 / 86_400.0
}

/// When date serial `at` comes, from now (at most a year away: it stays
/// a while, the app doesn't run for longer).
fn instant_of(at: f64, now: Instant) -> Instant {
    let secs = ((at - now_serial()) * 86_400.0).clamp(0.0, 366.0 * 86_400.0);
    now + Duration::from_secs_f64(secs)
}

/// The key egui calls `k`, and its modifiers.
fn egui_key(k: &Key) -> Option<(egui::Modifiers, egui::Key)> {
    let key = match k.name.as_str() {
        "ENTER" => egui::Key::Enter,
        "TAB" => egui::Key::Tab,
        "ESC" => egui::Key::Escape,
        "BACKSPACE" => egui::Key::Backspace,
        "DELETE" => egui::Key::Delete,
        "INSERT" => egui::Key::Insert,
        "HOME" => egui::Key::Home,
        "END" => egui::Key::End,
        "PGUP" => egui::Key::PageUp,
        "PGDN" => egui::Key::PageDown,
        "UP" => egui::Key::ArrowUp,
        "DOWN" => egui::Key::ArrowDown,
        "LEFT" => egui::Key::ArrowLeft,
        "RIGHT" => egui::Key::ArrowRight,
        n => egui::Key::from_name(n)?,
    };
    let mut m = egui::Modifiers::NONE;
    if k.ctrl {
        m = m | egui::Modifiers::COMMAND;
    }
    if k.shift {
        m = m | egui::Modifiers::SHIFT;
    }
    if k.alt {
        m = m | egui::Modifiers::ALT;
    }
    Some((m, key))
}

impl Schedule {
    /// Tab `id` closed or holds other code: its keys and timers go.
    pub(super) fn forget(&mut self, id: u64) {
        self.keys.remove(&id);
        self.shortcuts.remove(&id);
        self.names.remove(&id);
        self.timers.retain(|t| t.tab != id);
    }
}

impl App {
    /// What a macro of tab `id` asked for after it.
    pub(super) fn macro_requests(&mut self, id: u64, requests: Vec<Request>) {
        let now = Instant::now();
        let shared: Vec<bool> = requests.iter().map(|r| matches!(r, Request::OnKey { procedure: Some(p), .. } if self.is_library_macro(id, p))).collect();
        let mut book = Vec::new();
        let s = &mut self.macro_run.schedule;
        for (r, shared) in requests.into_iter().zip(shared) {
            match r {
                r @ (Request::SavedAs { .. } | Request::Close { .. } | Request::Print { .. }) => book.push(r),
                Request::OnKey { key, procedure } => {
                    let keys = s.keys.entry(id).or_default();
                    keys.retain(|(k, _)| *k != key);
                    s.shared_keys.retain(|(k, _)| *k != key);
                    match procedure {
                        Some(p) if shared => s.shared_keys.push((key, p)),
                        Some(p) => keys.push((key, p)),
                        None => {}
                    }
                }
                Request::OnTime { at, procedure, latest, schedule: true } => {
                    if s.timers.iter().filter(|t| t.tab == id).count() < MAX_TIMERS {
                        let latest = latest.map(|l| instant_of(l, now));
                        s.timers.push(Timer { tab: id, at, due: instant_of(at, now), latest, procedure });
                    }
                }
                Request::OnTime { at, procedure, schedule: false, .. } => {
                    // The one set for that time (the same serial).
                    if let Some(i) = s.timers.iter().position(|t| t.tab == id && t.procedure.eq_ignore_ascii_case(&procedure) && (t.at - at).abs() < 1e-6) {
                        s.timers.remove(i);
                    }
                }
            }
        }
        for r in book {
            self.book_request(id, r);
        }
    }

    /// Workbook.SaveAs (the tab is that file now) and Workbook.Close.
    pub(super) fn book_request(&mut self, id: u64, r: Request) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        match r {
            Request::SavedAs { path, clean } => {
                let path = std::path::PathBuf::from(path);
                let doc = matches!(self.tabs[idx].view, View::Doc(_));
                // `.txt` is written as tab-separated text from a workbook.
                let fmt = match apex_io::Format::from_path(&path) {
                    Some(apex_io::Format::Text) if !doc => apex_io::Format::Tsv,
                    Some(f @ (apex_io::Format::Docx | apex_io::Format::Odt | apex_io::Format::Fodt | apex_io::Format::Rtf | apex_io::Format::Text | apex_io::Format::Markdown)) if doc => f,
                    Some(f) if !doc && f.kind() == apex_io::Kind::Sheet => f,
                    _ => return,
                };
                self.signature_dropped(id);
                let encrypted = self.tabs[idx].password.is_some();
                self.after_save(idx, path, fmt, Ok(()), encrypted);
                if clean {
                    match &mut self.tabs[idx].view {
                        View::Sheet(v) => v.wb.modified = false,
                        View::Doc(v) => v.doc.modified = false,
                        _ => {}
                    }
                }
            }
            Request::Close { save: Some(true) } => self.save_then(idx, false, move |app, ok| {
                if let Some(idx) = app.tabs.iter().position(|t| t.id == id)
                    && ok
                {
                    app.remove_tab(idx);
                }
            }),
            Request::Close { save: Some(false) } => self.remove_tab(idx),
            Request::Close { save: None } => self.close_tab_checked(idx),
            // The print dialog of the workbook: the user prints, or not.
            Request::Print { sheet } => {
                self.active = idx;
                if let (Some(s), View::Sheet(v)) = (sheet, &mut self.tabs[idx].view)
                    && s < v.wb.sheets.len()
                    && s != v.macro_context().0
                {
                    v.select_sheet(s);
                }
                self.pending_print = Some(crate::common::AppCmd::Print);
            }
            _ => {}
        }
    }

    /// The shortcuts of tab `id`'s code (looked for again when it
    /// changes).
    fn code_shortcuts(&mut self, id: u64) -> Vec<(Key, String)> {
        let Some(t) = self.tabs.iter().find(|t| t.id == id) else { return Vec::new() };
        let View::Sheet(v) = &t.view else { return Vec::new() };
        let Some(bin) = &v.wb.macros.project else { return Vec::new() };
        let ptr = std::sync::Arc::as_ptr(bin) as usize;
        let s = &mut self.macro_run.schedule;
        if s.shortcuts.get(&id).is_none_or(|(p, _)| *p != ptr) {
            let found = apex_io::vba::read(bin).map(|p| p.modules.iter().flat_map(|m| apex_vba::macro_shortcuts(&m.code)).collect()).unwrap_or_default();
            s.shortcuts.insert(id, (ptr, found));
        }
        s.shortcuts.get(&id).map(|(_, k)| k.clone()).unwrap_or_default()
    }

    /// The macros of tab `id` that run without arguments, as Run Macro
    /// lists them (looked for again when the code changes).
    pub(in crate::app) fn macro_names(&mut self, id: u64) -> Vec<String> {
        let Some(t) = self.tabs.iter().find(|t| t.id == id) else { return Vec::new() };
        let View::Sheet(v) = &t.view else { return Vec::new() };
        let Some(bin) = &v.wb.macros.project else { return Vec::new() };
        let ptr = std::sync::Arc::as_ptr(bin) as usize;
        let s = &mut self.macro_run.schedule;
        if s.names.get(&id).is_none_or(|(p, _)| *p != ptr) {
            s.names.insert(id, (ptr, super::parse(bin).map(|p| p.macros()).unwrap_or_default()));
        }
        s.names.get(&id).map(|(_, n)| n.clone()).unwrap_or_default()
    }

    /// Runs macro `name` of the active workbook (a toolbar button).
    pub(in crate::app) fn run_active_macro(&mut self, name: String) {
        if let Some(id) = self.tabs.get(self.active).filter(|t| matches!(t.view, View::Sheet(_))).map(|t| t.id) {
            self.run_macro_named(id, name);
        }
    }

    /// A key of OnKey or of a macro's shortcut pressed in the active
    /// workbook (not while typing): runs its macro. True: it took the key.
    pub(in crate::app) fn macro_keys(&mut self, ctx: &egui::Context) -> bool {
        let Some(id) = self.tabs.get(self.active).map(|t| t.id) else { return false };
        let own = allowed(&self.settings, &self.tabs, id) && self.macro_run.is_enabled(id);
        let shared = self.settings.macros_enabled && !self.macro_run.schedule.shared_keys.is_empty();
        if self.macro_run.job.is_some() || ctx.text_edit_focused() || !(own || shared) || self.is_protected(id) {
            return false;
        }
        match self.tabs.get(self.active).map(|t| &t.view) {
            Some(View::Sheet(v)) if !v.is_editing() => {}
            _ => return false,
        }
        if !ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))) {
            return false;
        }
        // OnKey first; more modifiers before fewer (egui lets Shift by
        // when it isn't asked for).
        let mut keys = if own { self.macro_run.schedule.keys.get(&id).cloned().unwrap_or_default() } else { Vec::new() };
        if shared {
            let s = &self.macro_run.schedule.shared_keys;
            keys.extend(s.iter().filter(|(k, _)| !keys.iter().any(|(o, _)| o == k)).cloned().collect::<Vec<_>>());
        }
        if own {
            let code: Vec<_> = self.code_shortcuts(id).into_iter().filter(|(k, _)| !keys.iter().any(|(o, _)| o == k)).collect();
            keys.extend(code);
        }
        keys.sort_by_key(|(k, _)| std::cmp::Reverse(k.ctrl as u8 + k.shift as u8 + k.alt as u8));
        for (k, procedure) in keys {
            let Some((mods, key)) = egui_key(&k) else { continue };
            if ctx.input_mut(|i| i.consume_key(mods, key)) {
                if !procedure.is_empty() {
                    self.run_macro_named(id, procedure);
                }
                return true;
            }
        }
        false
    }

    /// Runs a macro whose time came, when nothing else runs; asks for a
    /// frame when the next one comes.
    pub(super) fn macro_timers(&mut self, ctx: &egui::Context) {
        let s = &mut self.macro_run.schedule;
        if s.timers.is_empty() {
            return;
        }
        let tabs: Vec<u64> = self.tabs.iter().map(|t| t.id).collect();
        s.timers.retain(|t| tabs.contains(&t.tab));
        let now = Instant::now();
        // Too late for LatestTime: they don't run.
        s.timers.retain(|t| t.latest.is_none_or(|l| l >= now));
        let free = self.macro_run.job.is_none() && self.macro_run.queue.is_none() && self.macro_run.consent.is_none() && self.macro_run.events.is_empty();
        let due = s.timers.iter().enumerate().filter(|(_, t)| t.due <= now).min_by_key(|(_, t)| t.due).map(|(i, _)| i);
        if let Some(i) = due.filter(|_| free) {
            let t = self.macro_run.schedule.timers.remove(i);
            // Only while its macros run (else it is dropped, not asked).
            let own = allowed(&self.settings, &self.tabs, t.tab) && self.macro_run.is_enabled(t.tab);
            if (own || self.is_library_macro(t.tab, &t.procedure)) && !self.is_protected(t.tab) {
                self.macro_run.queue = Some((t.tab, Entry::Macro(t.procedure)));
            }
            ctx.request_repaint();
        } else if let Some(next) = self.macro_run.schedule.timers.iter().map(|t| t.due).min() {
            ctx.request_repaint_after(next.saturating_duration_since(now).max(Duration::from_millis(50)));
        }
    }
}

/// Macro Options: the Ctrl+ letter of a macro.
pub(super) struct ShortcutDlg {
    tab: u64,
    /// As the Run Macro list names it (`Proc` or `Module.Proc`).
    name: String,
    letter: String,
    /// The letter's field takes the keys when it opens.
    focus: bool,
}

/// `code` with the shortcut of procedure `proc` set to `letter` (none:
/// taken away): the `Attribute` line Excel writes below its `Sub` line.
/// None when the module has no such procedure.
pub(super) fn set_shortcut(code: &str, proc: &str, letter: Option<char>) -> Option<String> {
    let attr = format!("{proc}.VB_ProcData.VB_Invoke_Func");
    let mut lines: Vec<String> = code
        .split('\n')
        .filter(|l| {
            let t = l.trim();
            !(t.get(..10).is_some_and(|h| h.eq_ignore_ascii_case("attribute ")) && t[10..].split('=').next().is_some_and(|lhs| lhs.trim().eq_ignore_ascii_case(&attr)))
        })
        .map(str::to_string)
        .collect();
    let joined = lines.join("\n");
    let (name, row) = super::super::vba_edit::proc_rows(&joined).into_iter().find(|p| p.0.eq_ignore_ascii_case(proc))?;
    if let Some(c) = letter {
        lines.insert(row + 1, format!("Attribute {name}.VB_ProcData.VB_Invoke_Func = \"{c}\\n14\""));
    }
    Some(lines.join("\n"))
}

impl App {
    /// Macro Options for macro `name` of tab `tab` (Run Macro's button).
    pub(super) fn open_macro_shortcut(&mut self, tab: u64, name: String) {
        let proc = name.rsplit('.').next().unwrap_or(&name).to_string();
        let letter = self.code_shortcuts(tab).into_iter().find(|(_, p)| p.eq_ignore_ascii_case(&proc)).map(|(k, _)| if k.shift { k.name.clone() } else { k.name.to_ascii_lowercase() }).unwrap_or_default();
        self.macro_run.schedule.dialog = Some(ShortcutDlg { tab, name, letter, focus: true });
    }

    pub(super) fn macro_shortcut_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.macro_run.schedule.dialog else { return };
        let (mut ok, mut cancel) = (false, false);
        let m = egui::Modal::new(egui::Id::new("macro_shortcut")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading(crate::i18n::tr("macros.options_title"));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(&d.name).strong());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(crate::i18n::tr("macros.shortcut"));
                let r = ui.add(egui::TextEdit::singleline(&mut d.letter).char_limit(1).desired_width(28.0));
                if std::mem::take(&mut d.focus) {
                    r.request_focus();
                }
            });
            ui.label(egui::RichText::new(crate::i18n::tr("macros.shortcut_tip")).weak());
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                cancel = ui.button(crate::i18n::tr("dialog.cancel")).clicked();
                ok = ui.button(crate::i18n::tr("dialog.ok")).clicked();
            });
        });
        if m.is_top_modal {
            ok |= ctx.input(|i| i.key_pressed(egui::Key::Enter));
            cancel |= ctx.input(|i| i.key_pressed(egui::Key::Escape));
        }
        if !(ok || cancel) {
            return;
        }
        let Some(d) = self.macro_run.schedule.dialog.take() else { return };
        if !ok {
            return;
        }
        let letter = d.letter.trim().chars().next();
        if letter.is_some_and(|c| !c.is_ascii_alphabetic()) {
            self.notify(super::Notice::info(crate::i18n::tr("macros.shortcut_tip")));
            return;
        }
        self.set_macro_shortcut(d.tab, &d.name, letter);
    }

    /// Writes the shortcut into the code of the module that has the macro
    /// (not into a locked or signed project: that would break it).
    fn set_macro_shortcut(&mut self, tab: u64, name: &str, letter: Option<char>) {
        self.flush_macros(tab);
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab) else { return };
        let View::Sheet(v) = &self.tabs[idx].view else { return };
        let Some(bin) = v.wb.macros.project.clone() else { return };
        let Ok(mut project) = apex_io::vba::read(&bin) else { return };
        if project.protected || v.wb.macros.signed {
            self.notify(super::Notice::info(crate::i18n::tr(if project.protected { "macros.locked" } else { "macros.signed" })));
            return;
        }
        let (module, proc) = match name.split_once('.') {
            Some((m, p)) => (Some(m), p),
            None => (None, name),
        };
        let found = project.modules.iter_mut().filter(|m| module.is_none_or(|n| m.name.eq_ignore_ascii_case(n))).find_map(|m| {
            let code = set_shortcut(&m.code, proc, letter)?;
            m.code = code;
            Some(())
        });
        if found.is_none() {
            return;
        }
        match project.write() {
            Ok(bin) => {
                self.hold(idx);
                // The editor shows the code as it is now.
                self.macros.reload(tab, &bin);
                if let View::Sheet(v) = &mut self.tabs[idx].view {
                    v.wb.macros.set_project(bin);
                }
                super::super::macros_ui::mark_modified(&mut self.tabs[idx].view, true);
            }
            Err(e) => self.notify(super::Notice::error(crate::i18n::trf("macros.bad_project", &[("error", &e)]))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::set_shortcut;

    #[test]
    fn shortcut_lines() {
        let code = "Sub Go()\n    x = 1\nEnd Sub\nSub Other()\nAttribute Other.VB_ProcData.VB_Invoke_Func = \"q\\n14\"\nEnd Sub";
        let set = set_shortcut(code, "go", Some('K')).unwrap();
        assert!(set.starts_with("Sub Go()\nAttribute Go.VB_ProcData.VB_Invoke_Func = \"K\\n14\"\n    x = 1"), "{set}");
        let keys = apex_vba::macro_shortcuts(&set);
        assert_eq!(keys.len(), 2);
        assert!(keys[0].0.shift && keys[0].0.name == "K");
        // Changed, then taken away.
        let again = set_shortcut(&set, "Go", Some('j')).unwrap();
        assert_eq!(again.matches("VB_Invoke_Func").count(), 2);
        let none = set_shortcut(&set_shortcut(&again, "Go", None).unwrap(), "Other", None).unwrap();
        assert_eq!(none, "Sub Go()\n    x = 1\nEnd Sub\nSub Other()\nEnd Sub");
        assert!(set_shortcut(code, "Missing", Some('a')).is_none());
    }
}
