//! The events of a workbook and its sheets: what the user does (editing,
//! moving between sheets and cells, double and right clicks, adding a
//! sheet, saving, printing, closing) runs the handlers of the code, and
//! some of it waits for them, which may cancel it (`Cancel = True`).
//!
//! Events fire only for the active tab, once its macros are enabled and
//! while Application.EnableEvents is on; what a macro does itself fires
//! none (no handler can call itself over and over).

use apex_sheet::{Content, Range};
use apex_vba::{EventArg, Owner};
use eframe::egui;

use super::{App, Entry, View, allowed};
use crate::common::AppCmd;

/// An event to fire: whose (the workbook or a sheet), which (`Change`,
/// `BeforeSave`...) and with what.
#[derive(Clone, Debug)]
pub(super) struct Event {
    pub owner: Owner,
    pub name: &'static str,
    pub args: Vec<EventArg>,
}

impl Event {
    fn book(name: &'static str, args: Vec<EventArg>) -> Event {
        Event { owner: Owner::Workbook, name, args }
    }

    fn sheet(sheet: usize, name: &'static str, args: Vec<EventArg>) -> Event {
        Event { owner: Owner::Sheet(sheet), name, args }
    }

    /// The name of its handler, in lower case.
    fn handler(&self) -> String {
        let who = if self.owner == Owner::Workbook { "workbook" } else { "worksheet" };
        format!("{who}_{}", self.name.to_ascii_lowercase())
    }
}

/// What the user did that waits for the events: done when they end,
/// unless a handler cancelled it or failed.
#[derive(Clone, Debug)]
pub(super) enum After {
    Save(bool),
    Close,
    Print(AppCmd),
    /// A double (false) or right (true) click on cells.
    Click(bool),
    /// Closing the window.
    Quit,
}

/// Where the user was in a workbook, to tell what changed.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Place {
    sheet: usize,
    sel: Range,
    sheets: usize,
}

impl App {
    /// The handlers of the code of tab `id` that events may run, when
    /// they may run now.
    fn live_handlers(&mut self, id: u64) -> Option<&[String]> {
        if !allowed(&self.settings, &self.tabs, id) || self.is_protected(id) {
            return None;
        }
        let t = self.tabs.iter().find(|t| t.id == id)?;
        let View::Sheet(v) = &t.view else { return None };
        let bin = v.wb.macros.project.as_ref()?;
        let en = self.macro_run.enabled.get_mut(&id).filter(|e| e.events)?;
        // The code may have changed in the editor: look again.
        if std::sync::Arc::as_ptr(bin) as usize != en.project {
            en.project = std::sync::Arc::as_ptr(bin) as usize;
            en.handlers = super::parse(bin).map(|p| p.event_names()).unwrap_or_default();
        }
        Some(&en.handlers)
    }

    /// Fires `events` of tab `id` that its code handles, then does
    /// `after`. False: none is handled (do it now).
    pub(super) fn fire_events(&mut self, id: u64, events: Vec<Event>, after: Option<After>) -> bool {
        let Some(handlers) = self.live_handlers(id) else { return false };
        let events: Vec<Event> = events.into_iter().filter(|e| handlers.contains(&e.handler())).collect();
        if events.is_empty() {
            return false;
        }
        // Moving around fast: only the last selection counts.
        if events.iter().all(|e| e.name == "SelectionChange") {
            self.macro_run.events.retain(|(t, ev, a)| !(*t == id && a.is_none() && ev.iter().all(|e| e.name == "SelectionChange")));
        }
        self.macro_run.events.push_back((id, events, after));
        true
    }

    /// Workbook_<name> of tab `idx` before something the user does,
    /// which it may cancel: true when it fires (`after` waits for it).
    fn before(&mut self, idx: usize, name: &'static str, args: Vec<EventArg>, after: After) -> bool {
        let Some(id) = self.tabs.get(idx).map(|t| t.id) else { return false };
        if !matches!(self.tabs[idx].view, View::Sheet(_)) || self.macro_run.job.as_ref().is_some_and(|j| j.tab == id) {
            return false;
        }
        self.fire_events(id, vec![Event::book(name, args)], Some(after))
    }

    /// Saving tab `idx`: Workbook_BeforeSave first; true when it fires
    /// (saving waits for it).
    pub(in crate::app) fn before_save(&mut self, idx: usize, save_as: bool) -> bool {
        self.before(idx, "BeforeSave", vec![EventArg::Bool(save_as), EventArg::Cancel], After::Save(save_as))
    }

    /// Workbook_AfterSave of tab `id`.
    pub(in crate::app) fn after_save_event(&mut self, id: u64, ok: bool) {
        self.fire_events(id, vec![Event::book("AfterSave", vec![EventArg::Bool(ok)])], None);
    }

    /// Closing tab `idx`: Workbook_BeforeClose first.
    pub(in crate::app) fn before_close(&mut self, idx: usize) -> bool {
        self.before(idx, "BeforeClose", vec![EventArg::Cancel], After::Close)
    }

    /// Printing the active tab: Workbook_BeforePrint first.
    pub(in crate::app) fn before_print(&mut self, cmd: &AppCmd) -> bool {
        if std::mem::take(&mut self.macro_run.printing) || !matches!(cmd, AppCmd::Print) {
            return false;
        }
        self.before(self.active, "BeforePrint", vec![EventArg::Cancel], After::Print(cmd.clone()))
    }

    /// Closing the window: Workbook_BeforeClose of every workbook first;
    /// true while they run (closing waits for them).
    pub(in crate::app) fn before_quit(&mut self) -> bool {
        if self.macro_run.quitting.is_some() {
            return true;
        }
        let ids: Vec<u64> = self.tabs.iter().filter(|t| matches!(t.view, View::Sheet(_))).map(|t| t.id).collect();
        let waiting: Vec<u64> = ids.into_iter().filter(|id| self.fire_events(*id, vec![Event::book("BeforeClose", vec![EventArg::Cancel])], Some(After::Quit))).collect();
        if waiting.is_empty() {
            return false;
        }
        self.macro_run.quitting = Some(waiting);
        true
    }

    /// The events let what the user did go on.
    pub(super) fn event_after(&mut self, id: u64, after: After) {
        if let After::Quit = after {
            if let Some(w) = &mut self.macro_run.quitting {
                w.retain(|t| *t != id);
                if w.is_empty() {
                    self.macro_run.quitting = None;
                    self.macro_run.quit_now = true;
                }
            }
            return;
        }
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        match after {
            After::Save(save_as) => self.save_then(idx, save_as, move |app, ok| app.after_save_event(id, ok)),
            After::Close => self.close_tab_checked(idx),
            After::Print(cmd) => {
                self.active = idx;
                self.macro_run.printing = true;
                self.pending_print = Some(cmd);
            }
            After::Click(right) => {
                if let View::Sheet(v) = &mut self.tabs[idx].view {
                    v.cell_click_done(right, false);
                }
            }
            After::Quit => {}
        }
    }

    /// The events cancelled what the user did (or failed).
    pub(super) fn event_cancelled(&mut self, id: u64, after: After) {
        if let After::Quit = after {
            self.macro_run.quitting = None;
        }
        if let After::Click(right) = after
            && let Some(View::Sheet(v)) = self.tabs.iter_mut().find(|t| t.id == id).map(|t| &mut t.view)
        {
            v.cell_click_done(right, true);
        }
    }

    /// The events of what the user did in the active workbook since the
    /// last frame: editing, another sheet or selection, a new sheet, and
    /// double and right clicks.
    pub(super) fn sheet_events(&mut self, ctx: &egui::Context) {
        let Some(id) = self.tabs.get(self.active).map(|t| t.id) else { return };
        // Another workbook came to the front.
        if let Some(old) = self.macro_run.front.replace(id).filter(|o| *o != id) {
            let wn = || vec![EventArg::Nothing];
            self.fire_events(old, vec![Event::book("WindowDeactivate", wn()), Event::book("Deactivate", vec![])], None);
            self.fire_events(id, vec![Event::book("Activate", vec![]), Event::book("WindowActivate", wn())], None);
        }
        let handlers = self.live_handlers(id).map(<[String]>::to_vec);
        let View::Sheet(v) = &mut self.tabs[self.active].view else { return };
        let has = |name: &str| handlers.as_ref().is_some_and(|h| h.iter().any(|x| x == name));
        v.cell_hooks.double = has("worksheet_beforedoubleclick");
        v.cell_hooks.right = has("worksheet_beforerightclick");
        let (sheet, sel) = v.macro_place();
        let now = Place { sheet, sel, sheets: v.wb.sheets.len() };
        let edits = v.wb.edits;
        let edit = v.wb.last_edit();
        let click = v.cell_hooks.clicked.take();
        let followed = v.cell_hooks.followed.take();
        let calc: Vec<usize> = if has("worksheet_calculate") || has("workbook_sheetcalculate") {
            v.wb.sheets.iter().enumerate().filter(|(_, s)| s.iter().any(|(_, c)| matches!(c.content, Content::Formula { .. }))).map(|(i, _)| i).collect()
        } else {
            Vec::new()
        };
        let seen = self.macro_run.seen.insert(id, edits);
        let was = self.macro_run.places.get(&id).copied();
        // Wait for the selection to end (dragging) and for a second click
        // (a handler running now would take it).
        let busy = ctx.input(|i| i.pointer.primary_down() || i.pointer.time_since_last_click() < 0.35);
        if busy && click.is_none() && was.is_some() {
            ctx.request_repaint_after(web_time::Duration::from_millis(400));
            if let Some(s) = seen {
                self.macro_run.seen.insert(id, s);
            }
            if let View::Sheet(v) = &mut self.tabs[self.active].view {
                v.cell_hooks.followed = followed;
            }
            return;
        }
        self.macro_run.places.insert(id, now);
        if handlers.is_none() {
            return;
        }
        let mut events = Vec::new();
        if let Some(w) = was {
            if now.sheets > w.sheets {
                events.push(Event::book("NewSheet", vec![EventArg::Sheet(now.sheet)]));
            }
            if w.sheet != now.sheet {
                if w.sheet < now.sheets && now.sheets == w.sheets {
                    events.push(Event::sheet(w.sheet, "Deactivate", vec![]));
                    events.push(Event::book("SheetDeactivate", vec![EventArg::Sheet(w.sheet)]));
                }
                events.push(Event::sheet(now.sheet, "Activate", vec![]));
                events.push(Event::book("SheetActivate", vec![EventArg::Sheet(now.sheet)]));
            } else if w.sel != now.sel {
                events.push(Event::sheet(sheet, "SelectionChange", vec![EventArg::Range(sheet, sel)]));
                events.push(Event::book("SheetSelectionChange", vec![EventArg::Sheet(sheet), EventArg::Range(sheet, sel)]));
            }
        }
        if seen.is_some_and(|s| s != edits)
            && let Some((s, target)) = edit
        {
            events.push(Event::sheet(s, "Change", vec![EventArg::Range(s, target)]));
            events.push(Event::book("SheetChange", vec![EventArg::Sheet(s), EventArg::Range(s, target)]));
            for i in calc {
                events.push(Event::sheet(i, "Calculate", vec![]));
                events.push(Event::book("SheetCalculate", vec![EventArg::Sheet(i)]));
            }
        }
        if let Some((s, c)) = followed {
            events.push(Event::sheet(s, "FollowHyperlink", vec![EventArg::Hyperlink(s, c)]));
            events.push(Event::book("SheetFollowHyperlink", vec![EventArg::Sheet(s), EventArg::Hyperlink(s, c)]));
        }
        if !events.is_empty() {
            self.fire_events(id, events, None);
        }
        if let Some((right, s, target)) = click {
            let name = if right { "BeforeRightClick" } else { "BeforeDoubleClick" };
            let ev = Event::sheet(s, name, vec![EventArg::Range(s, target), EventArg::Cancel]);
            if !self.fire_events(id, vec![ev], Some(After::Click(right)))
                && let View::Sheet(v) = &mut self.tabs[self.active].view
            {
                v.cell_click_done(right, false);
            }
        }
    }

    /// Where a macro left tab `id`: what it did fires no events.
    pub(in crate::app) fn events_settled(&mut self, id: u64) {
        let Some(View::Sheet(v)) = self.tabs.iter().find(|t| t.id == id).map(|t| &t.view) else { return };
        let (sheet, sel) = v.macro_place();
        let place = Place { sheet, sel, sheets: v.wb.sheets.len() };
        let edits = v.wb.edits;
        self.macro_run.places.insert(id, place);
        self.macro_run.seen.insert(id, edits);
    }

    /// The next events waiting, when nothing runs.
    pub(super) fn next_events(&mut self) -> Option<(u64, Entry, Option<After>)> {
        let (id, events, after) = self.macro_run.events.pop_front()?;
        Some((id, Entry::Events(events), after))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_names() {
        assert_eq!(Event::book("BeforeSave", vec![]).handler(), "workbook_beforesave");
        assert_eq!(Event::sheet(2, "SelectionChange", vec![]).handler(), "worksheet_selectionchange");
    }
}
