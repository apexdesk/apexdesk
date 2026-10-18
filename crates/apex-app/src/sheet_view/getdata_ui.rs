//! Data > Get Data: queries that read a file, a folder, a web page or API
//! or a database, shape the data with steps and load it into a table. The
//! editor previews every step; reading runs on another thread, and loading
//! or refreshing is one undo step.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use web_time::{Duration, Instant};

use apex_query::{DataTable, Error};
use apex_sheet::query::{self, Query, Source, Step};

use super::getdata_forms as forms;
use super::getdata_run::{self as run, Done, Preview, PreviewOk};
use super::*;
use crate::widgets::DialogResult;

/// Sources of the Get Data menu: (i18n key, icon).
pub(super) const SOURCES: [(&str, Icon); 14] = [
    ("getdata.src.text", Icon::Doc),
    ("getdata.src.workbook", Icon::Sheet),
    ("getdata.src.folder", Icon::Folder),
    ("getdata.src.json", Icon::Field),
    ("getdata.src.xml", Icon::Field),
    ("getdata.src.pdf", Icon::Pdf),
    ("getdata.src.html", Icon::Link),
    ("getdata.src.sqlite", Icon::Table),
    ("getdata.src.dbfile", Icon::Table),
    ("getdata.src.postgres", Icon::Table),
    ("getdata.src.mysql", Icon::Table),
    ("getdata.src.sqlserver", Icon::Table),
    ("getdata.src.table", Icon::FormatAsTable),
    ("getdata.src.query", Icon::Pivot),
];

/// Queries the user said yes to running or wrote, by file (this
/// session): each query on its own, so saying yes to one isn't saying
/// yes to the others the file brought.
static TRUSTED: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

/// A query being edited.
pub(super) struct Editor {
    /// Its name in the workbook (None: a new query).
    orig: Option<String>,
    q: Query,
    /// Load it into a table (else it is only a connection).
    load: bool,
    /// 0: the source; i: step i.
    sel: usize,
    generation: u64,
    source: Option<Arc<DataTable>>,
    choices: Vec<String>,
    /// Edited and not previewed yet (since then).
    dirty: Option<Instant>,
    source_dirty: bool,
    /// A query of the file that reads from elsewhere (another computer,
    /// the web, a server) is previewed only once the user asks to.
    held: bool,
    rx: Option<Receiver<Preview>>,
    shown: Option<Result<PreviewOk, Error>>,
    password: String,
    /// The full run of the Load button.
    loading: Option<Receiver<Vec<Done>>>,
    message: Option<String>,
}

impl Editor {
    fn new(orig: Option<String>, q: Query) -> Box<Editor> {
        let load = orig.is_none() || q.table.is_some();
        Box::new(Editor { orig, q, load, sel: 0, generation: 0, source: None, choices: Vec::new(), dirty: Some(Instant::now() - Duration::from_secs(1)), source_dirty: true, held: false, rx: None, shown: None, password: String::new(), loading: None, message: None })
    }

    #[cfg(test)]
    pub(super) fn parts(&self) -> (&Query, Option<&String>) {
        (&self.q, self.message.as_ref())
    }

    fn touch(&mut self, source: bool) {
        self.dirty = Some(Instant::now());
        self.source_dirty |= source;
    }
}

/// A refresh (or the editor's load) running.
struct Refresh {
    rx: Receiver<Vec<Done>>,
    /// The query list to save with the results.
    queries: Vec<Query>,
    label: &'static str,
}

#[derive(Default)]
pub(super) struct GetDataState {
    pub editor: Option<Box<Editor>>,
    refresh: Option<Refresh>,
    /// A database password asked before refreshing: (key, typed, queries).
    ask: Option<(String, String, Vec<String>)>,
    /// Refresh on open was looked at.
    open_checked: bool,
    /// Queries waiting for the user to say yes to running what the file
    /// brought, and whether they refresh because the file opened.
    confirm: Option<(Vec<String>, bool)>,
    /// Database passwords typed for this workbook while it is open, by
    /// [`apex_query::password_key`] (kind, user, server, port, database):
    /// another workbook naming the same server asks again.
    passwords: std::collections::HashMap<String, String>,
}

/// The message of an error, in the interface language.
pub(super) fn error_text(e: &Error) -> String {
    let (key, arg) = e.key();
    trf(key, &[("x", &arg)])
}

/// Renames the query `old` in the others' steps and sources.
fn rename_refs(list: &mut [Query], old: &str, new: &str) {
    if old.eq_ignore_ascii_case(new) {
        return;
    }
    for q in list {
        if let Source::Query { name } = &mut q.source
            && name.eq_ignore_ascii_case(old)
        {
            *name = new.to_string();
        }
        for s in &mut q.steps {
            if let Step::Append { query } | Step::Merge { query, .. } = s
                && query.eq_ignore_ascii_case(old)
            {
                *query = new.to_string();
            }
        }
    }
}

impl SheetView {
    fn base_dir(&self) -> Option<PathBuf> {
        self.file_dir.clone()
    }

    /// Get Data > a source: a new query in the editor (file sources ask
    /// for their file first).
    /// A new query reading the file `path` (for scripts: no dialog).
    pub fn new_query_file(&mut self, path: &std::path::Path) {
        self.settle_edit();
        let key = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) { "getdata.src.pdf" } else { "getdata.src.text" };
        let mut src = forms::new_source(key);
        forms::set_location(&mut src, path.to_string_lossy().into_owned());
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let q = Query::new(query::unique_name(&self.wb.queries, &name), src);
        self.getdata.editor = Some(Editor::new(None, q));
    }

    pub(super) fn new_query(&mut self, key: &'static str) {
        self.settle_edit();
        let mut src = forms::new_source(key);
        let mut name = tr(key).to_string();
        if let Some((label, exts)) = forms::file_filter(&src) {
            let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr(key)).add_filter(tr(label), exts);
            if let Some(d) = &self.file_dir {
                dlg = dlg.set_directory(d);
            }
            dlg.pick_file(crate::file_dialog::in_view(move |v: &mut SheetView, p: std::path::PathBuf| {
                let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(name);
                forms::set_location(&mut src, p.to_string_lossy().into_owned());
                v.edit_new_query(&name, src);
            }));
            return;
        } else if matches!(src, Source::Folder { .. }) {
            crate::file_dialog::FileDialog::new().set_title(tr(key)).pick_folder(crate::file_dialog::in_view(move |v: &mut SheetView, p: std::path::PathBuf| {
                let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or(name);
                forms::set_location(&mut src, p.to_string_lossy().into_owned());
                v.edit_new_query(&name, src);
            }));
            return;
        } else if let Source::Table { name: t } = &mut src {
            // The table the cursor is in, else the first one.
            let at = self.wb.table_at(self.sheet, self.cursor).and_then(|id| self.wb.table(id)).map(|t| t.name.clone());
            *t = at.or_else(|| self.wb.tables().next().map(|(_, t)| t.name.clone())).unwrap_or_default();
            name = t.clone();
        }
        self.edit_new_query(&name, src);
    }

    /// The query editor on a new query named after `name`.
    fn edit_new_query(&mut self, name: &str, src: Source) {
        let q = Query::new(query::unique_name(&self.wb.queries, name), src);
        self.getdata.editor = Some(Editor::new(None, q));
    }

    /// Opens the editor on a query converted from Power Query, with what
    /// didn't convert as its message.
    pub(super) fn edit_converted_query(&mut self, mut q: Query, note: Option<String>) {
        self.settle_edit();
        q.name = query::unique_name(&self.wb.queries, &q.name);
        let mut ed = Editor::new(None, q);
        ed.message = note;
        self.hold_remote(&mut ed);
        self.getdata.editor = Some(ed);
    }

    /// Opens the editor on the workbook's query `name`.
    pub(super) fn edit_query(&mut self, name: &str) {
        let Some(q) = self.wb.queries.iter().find(|q| q.name == name).cloned() else { return };
        self.settle_edit();
        let mut ed = Editor::new(Some(q.name.clone()), q);
        self.hold_remote(&mut ed);
        self.getdata.editor = Some(ed);
    }

    /// Holds the preview of a query the file brought when it, or a query
    /// it reads from, reads from elsewhere: opening it to look is not
    /// saying yes to connecting there.
    fn hold_remote(&self, ed: &mut Editor) {
        let mut list = self.wb.queries.clone();
        match ed.orig.as_deref().and_then(|o| list.iter().position(|q| q.name == o)) {
            Some(i) => list[i] = ed.q.clone(),
            None => list.push(ed.q.clone()),
        }
        let dests: Vec<String> = with_dependencies(&list, std::slice::from_ref(&ed.q.name)).iter().filter(|q| q.source.is_remote()).map(|q| shown(&q.source.describe(), false)).collect();
        if dests.is_empty() {
            return;
        }
        ed.held = true;
        let held = trf("getdata.held", &[("dest", &dests.join(", "))]);
        ed.message = Some(match ed.message.take() {
            Some(m) => format!("{m}\n{held}"),
            None => held,
        });
    }

    /// Deletes a query (its table and data stay).
    pub(super) fn delete_query(&mut self, name: &str) {
        let mut list = self.wb.queries.clone();
        list.retain(|q| q.name != name);
        self.wb.set_queries(list);
    }

    /// Runs the queries `names` and loads their results (one undo step);
    /// asks for database passwords first.
    pub(super) fn refresh_queries(&mut self, names: Vec<String>) {
        if names.is_empty() || self.getdata.refresh.is_some() {
            return;
        }
        let ctx = run::context(&self.wb, self.wb.queries.clone(), self.base_dir(), &self.getdata.passwords);
        // Passwords of the databases these queries (or the ones they use) read.
        let mut todo: Vec<String> = names.clone();
        let mut seen: Vec<String> = Vec::new();
        while let Some(n) = todo.pop() {
            if seen.contains(&n) {
                continue;
            }
            seen.push(n.clone());
            let Some(q) = self.wb.queries.iter().find(|q| q.name.eq_ignore_ascii_case(&n)) else { continue };
            if let Some(key) = apex_query::password_key(&q.source)
                && !ctx.passwords.contains_key(&key)
            {
                self.getdata.ask = Some((key, String::new(), names));
                return;
            }
            todo.extend(query::dependencies(q).into_iter().map(str::to_string));
        }
        let Some(egui_ctx) = self.ctx.clone() else { return };
        let rx = run::run(&egui_ctx, names, ctx);
        self.getdata.refresh = Some(Refresh { rx, queries: self.wb.queries.clone(), label: "undo.query_refresh" });
    }

    /// Data > Refresh All: every query loaded into a table.
    pub(super) fn refresh_all_queries(&mut self) {
        self.settle_edit();
        let names = self.wb.queries.iter().filter(|q| q.table.is_some()).map(|q| q.name.clone()).collect();
        self.ask_refresh(names, false);
    }

    /// Refreshes the query whose table holds the cursor, else all.
    pub(super) fn refresh_query_here(&mut self) {
        self.settle_edit();
        let here = self.wb.table_at(self.sheet, self.cursor).and_then(|id| self.wb.table(id)).map(|t| t.name.clone());
        let q = here.and_then(|t| self.wb.queries.iter().find(|q| q.table.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(&t))).map(|q| q.name.clone()));
        match q {
            Some(n) => self.ask_refresh(vec![n], false),
            None => self.refresh_all_queries(),
        }
    }

    pub(super) fn getdata_busy(&self) -> bool {
        self.getdata.refresh.is_some()
    }

    /// Writes finished runs into their tables.
    fn load_results(&mut self, done: Vec<Done>, queries: Vec<Query>, label: &'static str) -> Result<(), String> {
        let before = self.wb.undo_len();
        let mut list = queries;
        let mut errors = Vec::new();
        let mut last = None;
        for d in done {
            match d.result {
                Ok(t) => match self.wb.load_query(list.clone(), &d.name, &t.to_result(), label) {
                    Ok(table) => {
                        list = self.wb.queries.clone();
                        last = Some(table);
                    }
                    Err(e) => errors.push(format!("{}: {}", d.name, tr(match e {
                        apex_sheet::LoadError::Overlap => "getdata.err.overlap",
                        apex_sheet::LoadError::TooBig => "getdata.err.too_big",
                        apex_sheet::LoadError::NoQuery => "getdata.err.no_query",
                    }))),
                },
                Err(e) => errors.push(format!("{}: {}", d.name, error_text(&e))),
            }
        }
        let n = self.wb.undo_len().saturating_sub(before);
        self.wb.merge_undo(n, label);
        if let Some(t) = last {
            self.goto_table(&t);
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) }
    }

    /// Polls running work and shows the Get Data windows.
    pub(super) fn getdata_frame(&mut self, ctx: &egui::Context) {
        self.getdata_open_check();
        if let Some(r) = &self.getdata.refresh {
            match r.rx.try_recv() {
                Ok(done) => {
                    let r = self.getdata.refresh.take().unwrap();
                    let n = done.len();
                    match self.load_results(done, r.queries, r.label) {
                        Ok(()) => self.notices.push(Notice::info(trf("getdata.refreshed", &[("n", &n)]))),
                        Err(e) => self.notices.push(Notice::error(e)),
                    }
                }
                Err(TryRecvError::Disconnected) => self.getdata.refresh = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        self.getdata_editor(ctx);
        self.getdata_password(ctx);
    }

    /// Asks for a database password before a refresh.
    fn getdata_password(&mut self, ctx: &egui::Context) {
        let Some((key, pw, _)) = &mut self.getdata.ask else { return };
        let res = widgets::dialog(ctx, "getdata_password", tr("getdata.password_title"), 420.0, |ui| {
            // Where it goes as the queries say it (the key is user@server).
            let to = self.wb.queries.iter().find(|q| apex_query::password_key(&q.source).as_ref() == Some(key)).map_or_else(|| key.clone(), |q| q.source.describe());
            ui.label(trf("getdata.password_for", &[("x", &shown(&to, false))]));
            if self.wb.queries.iter().any(|q| apex_query::password_key(&q.source).as_ref() == Some(key) && apex_query::windows_login(&q.source)) {
                ui.colored_label(ui.visuals().warn_fg_color, tr("db.windows_account"));
            }
            ui.add_space(4.0);
            ui.add(egui::TextEdit::singleline(pw).password(true).desired_width(f32::INFINITY)).request_focus();
            ui.label(RichText::new(tr("getdata.password_note")).small().weak());
        });
        match res {
            DialogResult::Ok => {
                let (key, pw, names) = self.getdata.ask.take().unwrap();
                self.getdata.passwords.insert(key, pw);
                self.refresh_queries(names);
            }
            DialogResult::Cancel => self.getdata.ask = None,
            DialogResult::None => {}
        }
    }

    /// The queries refreshing on opening runs: the ones marked so and loaded
    /// into a table.
    fn open_names(&self) -> Vec<String> {
        self.wb.queries.iter().filter(|q| q.refresh_on_open && q.table.is_some()).map(|q| q.name.clone()).collect()
    }

    /// The first frame of a workbook whose queries refresh on opening:
    /// refresh if its file was trusted this session, else ask.
    fn getdata_open_check(&mut self) {
        if self.getdata.open_checked {
            return;
        }
        self.getdata.open_checked = true;
        if self.open_names().is_empty() {
            return;
        }
        self.settle_edit();
        self.ask_refresh(self.open_names(), true);
    }

    /// Refreshes `names` when the user said yes to the workbook's queries
    /// this session (or wrote them), else asks first: a query the file
    /// brought may read a share (handing over the Windows login), a web
    /// address (telling its owner the file was opened) or a private local
    /// file into the sheet, so pressing Refresh isn't saying yes to that.
    pub(super) fn ask_refresh(&mut self, names: Vec<String>, on_open: bool) {
        if names.is_empty() {
            return;
        }
        if self.trusted(&names) {
            self.refresh_queries(names);
        } else {
            self.getdata.confirm = Some((names, on_open));
        }
    }

    /// The file and what a query reads and does (not where it loads): a
    /// query changed, or another file saved over this one, is asked about
    /// again.
    fn trust_key(&self, q: &Query) -> (PathBuf, String) {
        let file = self.file_dir.as_ref().map(|d| d.join(&self.print.file_name)).unwrap_or_default();
        (file, format!("{:?}", (&q.name, &q.source, &q.steps)))
    }

    /// Whether every query `names` runs (with the ones they read from) was
    /// said yes to.
    fn trusted(&self, names: &[String]) -> bool {
        let Ok(t) = TRUSTED.lock() else { return false };
        with_dependencies(&self.wb.queries, names).iter().all(|q| t.contains(&self.trust_key(q)))
    }

    /// Remembers `queries` as said yes to.
    fn trust<'q>(&self, queries: impl IntoIterator<Item = &'q Query>) {
        let Ok(mut t) = TRUSTED.lock() else { return };
        for q in queries {
            let k = self.trust_key(q);
            if !t.contains(&k) {
                t.push(k);
            }
        }
    }

    /// The bar asking whether to run the file's queries (drawn over the
    /// grid).
    pub(super) fn getdata_bar(&mut self, ui: &mut egui::Ui) {
        let Some((names, on_open)) = self.getdata.confirm.clone() else { return };
        let dark = ui.visuals().dark_mode;
        let (fill, edge) = if dark { (Color32::from_rgb(0x4A, 0x40, 0x1C), Color32::from_rgb(0x8A, 0x74, 0x2A)) } else { (Color32::from_rgb(0xFF, 0xF4, 0xCE), Color32::from_rgb(0xE8, 0xD0, 0x7A)) };
        let (mut yes, mut no) = (false, false);
        // What would run, for the user to see before saying yes: the
        // queries refreshed and every query they read from, each source
        // and, for a database, its SQL (the workbook's, run with the
        // user's password).
        let mut runs = with_dependencies(&self.wb.queries, &names);
        // Servers and shares first, where they can't scroll out of sight.
        runs.sort_by_key(|q| !q.source.is_remote());
        let runs_owned: Vec<Query> = runs.iter().map(|q| (*q).clone()).collect();
        let n = runs.len();
        let remote = runs.iter().any(|q| q.source.is_remote());
        let details = runs
            .iter()
            .map(|q| match &q.source {
                Source::Database { sql, .. } => format!("{}: {}\n    {}", shown(&q.name, false), shown(&q.source.describe(), false), shown(sql.trim(), true)),
                s => format!("{}: {}", shown(&q.name, false), shown(&s.describe(), false)),
            })
            .collect::<Vec<_>>()
            .join("\n");
        egui::Frame::new().fill(fill).stroke(Stroke::new(1.0, edge)).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("getdata.open_title")).strong());
                let key = match (on_open, remote) {
                    (true, true) => "getdata.open_bar_remote",
                    (true, false) => "getdata.open_bar",
                    (false, true) => "getdata.run_bar_remote",
                    (false, false) => "getdata.run_bar",
                };
                ui.label(trf(key, &[("n", &n)])).on_hover_text(&details);
                yes = ui.button(tr("getdata.open_refresh")).clicked();
                no = ui.button(tr("getdata.open_skip")).clicked();
            });
            // The files and servers themselves, not only on hovering.
            egui::ScrollArea::vertical().max_height(90.0).id_salt("getdata_open_details").show(ui, |ui| {
                ui.label(RichText::new(&details).small().monospace());
            });
        });
        if yes || no {
            self.getdata.confirm = None;
        }
        if yes {
            self.trust(runs_owned.iter());
            self.settle_edit();
            self.refresh_queries(names);
        }
    }

    /// Starts a preview of the editor's selected step when it changed.
    fn editor_preview(&mut self, ed: &mut Editor, ctx: &egui::Context) {
        if let Some(rx) = &ed.rx {
            match rx.try_recv() {
                Ok(p) if p.generation == ed.generation => {
                    if let Some((t, choices)) = p.source {
                        ed.source = Some(t);
                        ed.choices = choices;
                    }
                    ed.shown = Some(p.result);
                    ed.rx = None;
                }
                Ok(_) | Err(TryRecvError::Disconnected) => ed.rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if ed.held {
            return;
        }
        let Some(at) = ed.dirty else { return };
        // Typing settles before reading a source again.
        let wait = if ed.source_dirty { Duration::from_millis(600) } else { Duration::from_millis(150) };
        if at.elapsed() < wait {
            ctx.request_repaint_after(wait - at.elapsed());
            return;
        }
        ed.dirty = None;
        ed.generation += 1;
        if ed.source_dirty {
            ed.source = None;
            ed.source_dirty = false;
        }
        let mut list = self.wb.queries.clone();
        match ed.orig.as_deref().and_then(|o| list.iter().position(|q| q.name == o)) {
            Some(i) => list[i] = ed.q.clone(),
            None => list.push(ed.q.clone()),
        }
        let rctx = run::context(&self.wb, list, self.base_dir(), &self.getdata.passwords);
        ed.rx = Some(run::preview(ctx, ed.generation, ed.q.clone(), ed.sel, ed.source.clone(), rctx));
    }

    fn getdata_editor(&mut self, ctx: &egui::Context) {
        let Some(mut ed) = self.getdata.editor.take() else { return };
        self.editor_preview(&mut ed, ctx);
        // The Load button's run.
        if let Some(rx) = &ed.loading {
            match rx.try_recv() {
                Ok(done) => {
                    ed.loading = None;
                    let list = self.editor_list(&ed);
                    match self.load_results(done, list, "undo.query_load") {
                        Ok(()) => {
                            self.focus_grid = true;
                            return;
                        }
                        Err(e) => ed.message = Some(e),
                    }
                }
                Err(TryRecvError::Disconnected) => ed.loading = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let tables: Vec<String> = self.wb.tables().map(|(_, t)| t.name.clone()).collect();
        let others: Vec<String> = self.wb.queries.iter().filter(|q| Some(&q.name) != ed.orig.as_ref()).map(|q| q.name.clone()).collect();
        let title = if ed.orig.is_some() { tr("getdata.edit_title") } else { tr("getdata.new_title") };
        let mut browse = None;
        let mut connect = false;
        let res = widgets::dialog(ctx, "getdata_editor", title, 980.0, |ui| {
            // Name and where the result goes.
            ui.horizontal(|ui| {
                ui.label(tr("getdata.name"));
                if ui.add(egui::TextEdit::singleline(&mut ed.q.name).desired_width(220.0)).changed() {
                    ed.message = None;
                }
                ui.add_space(16.0);
                ui.checkbox(&mut ed.load, tr("getdata.load_table"));
                if ed.load {
                    let to = ed.q.table.clone().filter(|t| self.wb.find_table(t).is_some()).map_or_else(|| tr("getdata.new_sheet").to_string(), |t| trf("getdata.into_table", &[("x", &t)]));
                    ui.label(RichText::new(to).weak());
                    ui.add_space(16.0);
                    ui.checkbox(&mut ed.q.refresh_on_open, tr("getdata.refresh_on_open"));
                }
            });
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                // Applied steps.
                ui.vertical(|ui| {
                    ui.set_width(230.0);
                    widgets::section(ui, tr("getdata.steps"));
                    egui::ScrollArea::vertical().id_salt("gd_steps").max_height(380.0).auto_shrink([false, false]).show(ui, |ui| {
                        let src_label = format!("{}: {}", tr("getdata.source"), tr(ed.q.source.kind_key()));
                        if ui.selectable_label(ed.sel == 0, RichText::new(src_label).strong()).clicked() && ed.sel != 0 {
                            ed.sel = 0;
                            ed.touch(false);
                        }
                        for i in 0..ed.q.steps.len() {
                            let s = &ed.q.steps[i];
                            let sum = forms::summary(s);
                            let label = if sum.is_empty() { format!("{}. {}", i + 1, tr(s.kind_key())) } else { format!("{}. {} · {sum}", i + 1, tr(s.kind_key())) };
                            let short: String = label.chars().take(44).collect();
                            let r = ui.selectable_label(ed.sel == i + 1, if short.len() < label.len() { format!("{short}…") } else { short });
                            if r.on_hover_text(&label).clicked() && ed.sel != i + 1 {
                                ed.sel = i + 1;
                                ed.touch(false);
                            }
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let cols = match &ed.shown {
                            Some(Ok(p)) => p.table.columns.clone(),
                            _ => Vec::new(),
                        };
                        crate::widgets::menu_button(ui, format!("+ {}", tr("getdata.add_step")), |ui| {
                            for key in forms::STEP_KINDS {
                                if ui.button(tr(key)).clicked() {
                                    let step = forms::new_step(key, &cols, &others);
                                    ed.q.steps.insert(ed.sel, step);
                                    ed.sel += 1;
                                    ed.touch(false);
                                    ui.close();
                                }
                            }
                        });
                        let i = ed.sel;
                        let n = ed.q.steps.len();
                        ui.add_enabled_ui(i > 1, |ui| {
                            if ui.small_button("↑").on_hover_text(tr("getdata.move_up")).clicked() {
                                ed.q.steps.swap(i - 1, i - 2);
                                ed.sel -= 1;
                                ed.touch(false);
                            }
                        });
                        ui.add_enabled_ui(i >= 1 && i < n, |ui| {
                            if ui.small_button("↓").on_hover_text(tr("getdata.move_down")).clicked() {
                                ed.q.steps.swap(i - 1, i);
                                ed.sel += 1;
                                ed.touch(false);
                            }
                        });
                        ui.add_enabled_ui(i >= 1, |ui| {
                            if ui.small_button("✕").on_hover_text(tr("getdata.delete_step")).clicked() {
                                ed.q.steps.remove(i - 1);
                                ed.sel -= 1;
                                ed.touch(false);
                            }
                        });
                    });
                });
                ui.add_space(12.0);
                // The selected step's settings and preview.
                ui.vertical(|ui| {
                    ui.set_width(700.0);
                    let heading = if ed.sel == 0 { tr(ed.q.source.kind_key()) } else { tr(ed.q.steps[ed.sel - 1].kind_key()) };
                    widgets::section(ui, heading);
                    egui::ScrollArea::vertical().id_salt(("gd_settings", ed.sel)).max_height(190.0).auto_shrink([false, true]).show(ui, |ui| {
                        if ed.sel == 0 {
                            let out = forms::source_form(ui, &mut ed.q.source, &ed.choices, &tables, &others, &mut ed.password);
                            if out.changed {
                                ed.touch(true);
                            }
                            browse = out.browse;
                            connect = out.connect;
                        } else {
                            let (cols, other) = match &ed.shown {
                                Some(Ok(p)) => (p.input.clone(), p.other.clone()),
                                _ => (Vec::new(), Vec::new()),
                            };
                            if forms::step_form(ui, &mut ed.q.steps[ed.sel - 1], &cols, &others, &other) {
                                ed.touch(false);
                            }
                        }
                    });
                    ui.add_space(4.0);
                    let busy = ed.rx.is_some() || ed.dirty.is_some();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(tr("getdata.preview")).strong());
                        match &ed.shown {
                            Some(Ok(p)) => {
                                ui.label(RichText::new(trf("getdata.size", &[("rows", &p.rows), ("cols", &p.table.width())])).weak());
                                if p.rows > run::PREVIEW_ROWS {
                                    ui.label(RichText::new(trf("getdata.first_rows", &[("n", &run::PREVIEW_ROWS)])).weak());
                                }
                            }
                            _ => {}
                        }
                        if busy {
                            ui.spinner();
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(tr("getdata.reload")).on_hover_text(tr("getdata.reload_tip")).clicked() {
                                ed.held = false;
                                ed.touch(true);
                                ed.dirty = Some(Instant::now() - Duration::from_secs(1));
                            }
                        });
                    });
                    match &ed.shown {
                        Some(Ok(p)) => forms::preview_grid(ui, &p.table, 230.0),
                        Some(Err(Error::NeedsPassword(_))) => {
                            ui.add_sized([680.0, 230.0], egui::Label::new(RichText::new(tr("getdata.need_password")).weak()));
                        }
                        Some(Err(e)) => {
                            ui.add_sized([680.0, 230.0], egui::Label::new(RichText::new(error_text(e)).color(ui.visuals().error_fg_color)).wrap());
                        }
                        None => {
                            ui.add_sized([680.0, 230.0], egui::Spinner::new());
                        }
                    }
                });
            });
            if ed.loading.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("getdata.loading"));
                });
            }
            if let Some(m) = &ed.message {
                ui.colored_label(ui.visuals().error_fg_color, m);
            }
        });
        if let Some(folder) = browse {
            let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr(ed.q.source.kind_key()));
            if let Some(d) = &self.file_dir {
                dlg = dlg.set_directory(d);
            }
            // The answer goes to the editor open then.
            let then = crate::file_dialog::in_view(|v: &mut SheetView, p: std::path::PathBuf| {
                if let Some(ed) = &mut v.getdata.editor {
                    forms::set_location(&mut ed.q.source, p.to_string_lossy().into_owned());
                    ed.touch(true);
                }
            });
            if folder {
                dlg.pick_folder(then);
            } else {
                if let Some((label, exts)) = forms::file_filter(&ed.q.source) {
                    dlg = dlg.add_filter(tr(label), exts);
                }
                dlg.pick_file(then);
            }
        }
        if connect && let Some(key) = apex_query::password_key(&ed.q.source) {
            self.getdata.passwords.insert(key, ed.password.clone());
            ed.held = false;
            ed.touch(true);
            ed.dirty = Some(Instant::now() - Duration::from_secs(1));
        }
        match res {
            DialogResult::Cancel => {
                self.focus_grid = true;
                return;
            }
            DialogResult::Ok if ed.loading.is_none() => {
                if let Err(m) = self.editor_ok(&mut ed, ctx) {
                    ed.message = Some(m);
                } else if ed.loading.is_none() {
                    self.focus_grid = true;
                    return;
                }
            }
            _ => {}
        }
        self.getdata.editor = Some(ed);
    }

    /// The workbook's queries with the editor's in place.
    fn editor_list(&self, ed: &Editor) -> Vec<Query> {
        let mut list = self.wb.queries.clone();
        let mut q = ed.q.clone();
        q.name = q.name.trim().to_string();
        if !ed.load {
            q.table = None;
            q.refresh_on_open = false;
        }
        match ed.orig.as_deref().and_then(|o| list.iter().position(|x| x.name == o)) {
            Some(i) => {
                let old = list[i].name.clone();
                list[i] = q.clone();
                rename_refs(&mut list, &old, &q.name);
            }
            None => list.push(q),
        }
        list
    }

    /// OK in the editor: saves the query, and runs and loads it when it
    /// goes into a table.
    fn editor_ok(&mut self, ed: &mut Editor, ctx: &egui::Context) -> Result<(), String> {
        let name = ed.q.name.trim().to_string();
        if name.is_empty() || self.wb.queries.iter().any(|q| q.name.eq_ignore_ascii_case(&name) && Some(&q.name) != ed.orig.as_ref()) {
            return Err(tr("getdata.err.name").to_string());
        }
        let list = self.editor_list(ed);
        // The user saw and wrote this query, unless it is held: closing a
        // query the file brought, looked at and not read, isn't saying yes
        // to it. Queries it reads from not said yes to are asked about
        // before running.
        if !ed.held {
            self.trust([&ed.q]);
        }
        if !ed.load {
            self.wb.set_queries(list);
            return Ok(());
        }
        let runs = with_dependencies(&list, std::slice::from_ref(&name));
        if runs.iter().any(|q| TRUSTED.lock().map_or(true, |t| !t.contains(&self.trust_key(q)))) {
            self.wb.set_queries(list);
            self.getdata.confirm = Some((vec![name], false));
            return Ok(());
        }
        if let Some(key) = apex_query::password_key(&ed.q.source)
            && !self.getdata.passwords.contains_key(&key)
        {
            return Err(tr("getdata.need_password").to_string());
        }
        let rctx = run::context(&self.wb, list, self.base_dir(), &self.getdata.passwords);
        ed.loading = Some(run::run(ctx, vec![name], rctx));
        ed.message = None;
        Ok(())
    }
}

/// `s` as shown where the user says yes to it: what the file wrote can't
/// hide where a query reads (line breaks pushing it out of sight, text
/// direction marks reordering it, invisible characters, spaces around it
/// that reading drops) — each shown as its code. `lines` keeps line
/// breaks and tabs (SQL).
fn shown(s: &str, lines: bool) -> String {
    let s = if lines { s } else { s.trim() };
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let hidden = c.is_control() && !(lines && matches!(c, '\n' | '\t'))
            || matches!(c, '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}');
        if hidden {
            out.push_str(&format!("\\u{{{:04X}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// The queries `names` and every query they read from, each once.
fn with_dependencies<'q>(queries: &'q [Query], names: &[String]) -> Vec<&'q Query> {
    let mut out: Vec<&Query> = Vec::new();
    let mut todo: Vec<&str> = names.iter().map(String::as_str).collect();
    while let Some(n) = todo.pop() {
        let Some(q) = queries.iter().find(|q| q.name.eq_ignore_ascii_case(n)) else { continue };
        if out.iter().any(|o| std::ptr::eq(*o, q)) {
            continue;
        }
        out.push(q);
        todo.extend(query::dependencies(q));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_runs_only_what_the_bar_shows() {
        let mut wb = Workbook::new();
        wb.sheets.push(apex_sheet::Sheet::new("Data"));
        let text = |l: &str| Source::Text { location: l.into(), delimiter: None, encoding: String::new(), header: true, skip: 0 };
        let mut shown = Query::new("Local", Source::Query { name: "Base".into() });
        shown.table = Some("T1".into());
        shown.refresh_on_open = true;
        let base = Query::new("Base", text("https://example.org/a.csv"));
        let mut hidden = Query::new("Hidden", text("/home/u/.ssh/id_rsa"));
        hidden.table = Some("T2".into());
        wb.queries.extend([shown, base, hidden]);
        let v = SheetView::new(wb);
        assert_eq!(v.open_names(), vec!["Local".to_string()]);
        let runs: Vec<&str> = with_dependencies(&v.wb.queries, &v.open_names()).iter().map(|q| q.name.as_str()).collect();
        // The source it reads is shown (and is remote); the other table's
        // query isn't run.
        assert_eq!(runs, vec!["Local", "Base"]);
        // Trust holds for these queries only.
        let mut v = v;
        v.file_dir = Some(PathBuf::from("/tmp"));
        // Pressing Refresh All asks before running what the file brought.
        v.refresh_all_queries();
        assert!(v.getdata.refresh.is_none());
        assert_eq!(v.getdata.confirm.as_ref().map(|c| (c.0.len(), c.1)), Some((2, false)));
        // Saying yes to one query isn't saying yes to the others.
        v.trust([&v.wb.queries[0]]);
        assert!(!v.trusted(&["Local".to_string()]));
        v.trust([&v.wb.queries[1]]);
        assert!(v.trusted(&["Local".to_string()]));
        assert!(!v.trusted(&["Hidden".to_string()]));
        // Nor to the query once changed.
        v.wb.queries[1].source = text("https://example.org/other.csv");
        assert!(!v.trusted(&["Local".to_string()]));
        assert!(with_dependencies(&v.wb.queries, &v.open_names()).iter().any(|q| q.source.is_remote()));
    }

    #[test]
    fn hidden_characters_show() {
        assert_eq!(shown("  Sales\n\n\\\\x\\s ", false), "Sales\\u{000A}\\u{000A}\\\\x\\s");
        assert_eq!(shown("a\u{202E}b", false), "a\\u{202E}b");
        assert_eq!(shown("SELECT 1\nFROM t", true), "SELECT 1\nFROM t");
    }

    fn with_server_query() -> SheetView {
        let mut wb = Workbook::new();
        wb.sheets.push(apex_sheet::Sheet::new("Data"));
        let src = Source::Database { kind: query::DbKind::Postgres, host: "db.example.org".into(), port: 5432, database: "sales".into(), user: "ana".into(), sql: "SELECT 1".into(), tls: false };
        wb.queries.push(Query::new("Sales", src));
        SheetView::new(wb)
    }

    #[test]
    fn passwords_stay_with_their_workbook() {
        let mut a = with_server_query();
        let key = apex_query::password_key(&a.wb.queries[0].source).unwrap();
        a.getdata.passwords.insert(key.clone(), "secret".into());
        a.refresh_queries(vec!["Sales".into()]);
        assert!(a.getdata.ask.is_none(), "the workbook it was typed for doesn't ask again");
        // Another workbook naming the same server, database and user asks.
        let mut b = with_server_query();
        b.refresh_queries(vec!["Sales".into()]);
        assert_eq!(b.getdata.ask.as_ref().map(|a| a.0.as_str()), Some(key.as_str()));
    }
}
