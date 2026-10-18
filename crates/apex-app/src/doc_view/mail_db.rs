//! Mailings > Select Recipients > From a Database or From a Get Data
//! Query: the records come from a SQLite file, a PostgreSQL, MySQL or SQL
//! Server server, or what a workbook's query gives, read on a thread.

use std::path::PathBuf;
use apex_fs::thread::JoinHandle;

use apex_sheet::query::Source;
use eframe::egui::{self, RichText};

use super::DocView;
use super::mailings::Recipients;
use crate::i18n::{tr, trf};
use crate::merge_data::Records;
use crate::sheet_view::getdata_forms as forms;
use crate::widgets::{self, DialogResult};

/// The kinds offered: a `getdata.src.*` key each, then the query one.
const KINDS: [&str; 5] = ["getdata.src.sqlite", "getdata.src.postgres", "getdata.src.mysql", "getdata.src.sqlserver", "mail.from_query"];

pub(super) struct DbDlg {
    kind: usize,
    src: Source,
    password: String,
    /// Tables of the database, once connected.
    tables: Vec<String>,
    /// The workbook and its queries, for the query kind.
    book: String,
    /// Each query, with where it connects to outside this computer.
    queries: Vec<(String, Option<String>)>,
    query: String,
    /// The user lets the query chosen connect where it says.
    allow: bool,
    error: Option<String>,
}

impl DbDlg {
    pub(super) fn new(query: bool) -> DbDlg {
        let kind = if query { KINDS.len() - 1 } else { 0 };
        DbDlg { kind, src: forms::new_source(KINDS[0]), password: String::new(), tables: Vec::new(), book: String::new(), queries: Vec::new(), query: String::new(), allow: false, error: None }
    }

    fn is_query(&self) -> bool {
        self.kind == KINDS.len() - 1
    }

    /// Where the query chosen connects to, if outside this computer.
    fn target(&self) -> Option<&str> {
        self.queries.iter().find(|q| q.0 == self.query).and_then(|q| q.1.as_deref())
    }

    /// Where the records come from, for the toolstrip.
    fn describe(&self) -> String {
        if self.is_query() {
            let file = std::path::Path::new(&self.book).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            format!("{file} [{}]", self.query)
        } else {
            self.src.describe()
        }
    }
}

/// Records being read, with where they come from.
pub(super) type Job = (String, JoinHandle<Result<Records, String>>);

impl DocView {
    pub(super) fn open_mail_db(&mut self, query: bool) {
        self.mail.db = Some(DbDlg::new(query));
    }

    pub(super) fn mail_db_dialog(&mut self, ctx: &egui::Context) {
        self.poll_mail_job(ctx);
        let Some(mut d) = self.mail.db.take() else { return };
        let title = if d.is_query() { tr("mail.from_query") } else { tr("mail.from_db") };
        let mut browse = false;
        let res = widgets::dialog(ctx, "mail_db", title, 560.0, |ui| {
            widgets::form(ui, "mail_db_kind", |ui| {
                widgets::form_row(ui, tr("mail.db_kind"), |ui| {
                    let before = d.kind;
                    egui::ComboBox::from_id_salt("mail_db_kind").width(260.0).selected_text(tr(KINDS[d.kind])).show_ui(ui, |ui| {
                        for (i, k) in KINDS.iter().enumerate() {
                            ui.selectable_value(&mut d.kind, i, tr(k));
                        }
                    });
                    if d.kind != before && !d.is_query() {
                        d.src = forms::new_source(KINDS[d.kind]);
                        d.tables.clear();
                    }
                });
            });
            ui.add_space(4.0);
            if d.is_query() {
                widgets::form(ui, "mail_db_query", |ui| {
                    widgets::form_row(ui, tr("mail.workbook"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.book).desired_width(320.0).interactive(false));
                        browse = ui.button(format!("{}…", tr("getdata.browse"))).clicked();
                    });
                    widgets::form_row(ui, tr("getdata.query"), |ui| {
                        let before = d.query.clone();
                        egui::ComboBox::from_id_salt("mail_db_q").width(260.0).selected_text(d.query.as_str()).show_ui(ui, |ui| {
                            for (q, _) in &d.queries {
                                ui.selectable_value(&mut d.query, q.clone(), q);
                            }
                        });
                        if d.query != before {
                            d.allow = false;
                        }
                    });
                    widgets::form_row(ui, tr("getdata.password"), |ui| ui.add(egui::TextEdit::singleline(&mut d.password).password(true).desired_width(240.0)));
                });
                ui.label(RichText::new(tr("mail.query_note")).small().weak());
                if let Some(dest) = d.target().map(str::to_string) {
                    ui.add_space(4.0);
                    ui.colored_label(ui.visuals().warn_fg_color, trf("mail.query_target", &[("name", &d.query), ("dest", &dest)]));
                    ui.checkbox(&mut d.allow, tr("mail.query_allow"));
                }
            } else {
                let out = forms::source_form(ui, &mut d.src, &d.tables, &[], &[], &mut d.password);
                browse = out.browse.is_some();
                if out.connect || (matches!(d.src, Source::Sqlite { .. }) && out.changed && d.tables.is_empty()) {
                    // Listing the tables is quick (or fails quickly).
                    match apex_query::sources::db::list_tables(&d.src, &d.password) {
                        Ok(t) => {
                            d.tables = t;
                            d.error = None;
                        }
                        Err(e) => d.error = Some(trf(e.key().0, &[("x", &e.key().1)])),
                    }
                }
            }
            if let Some(e) = &d.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        });
        if browse {
            self.mail_db_browse(&mut d);
        }
        match res {
            DialogResult::None => self.mail.db = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                if self.mail.job.is_some() {
                    d.error = Some(tr("mail.db_busy").to_string());
                    self.mail.db = Some(d);
                    return;
                }
                if d.is_query() && (d.book.is_empty() || d.query.is_empty()) {
                    d.error = Some(tr("mail.pick_query").to_string());
                    self.mail.db = Some(d);
                    return;
                }
                let what = d.describe();
                let job = if d.is_query() {
                    let (book, q, pw) = (PathBuf::from(&d.book), d.query.clone(), d.password.clone());
                    let seen = d.target().filter(|_| d.allow).map(str::to_string);
                    apex_fs::thread::spawn(move || crate::merge_data::from_query(&book, &q, &pw, &|dest| seen.as_deref() == Some(dest)))
                } else {
                    let (src, pw) = (d.src.clone(), d.password.clone());
                    apex_fs::thread::spawn(move || crate::merge_data::from_source(&src, &pw))
                };
                self.mail.job = Some((what, job));
                self.notices.push(crate::app::Notice::info(tr("mail.db_loading")));
            }
        }
    }

    fn mail_db_browse(&mut self, d: &mut DbDlg) {
        // The answer goes to the dialog open then.
        fn dlg(v: &mut DocView) -> Option<&mut DbDlg> {
            v.mail.db.as_mut()
        }
        if d.is_query() {
            crate::file_dialog::FileDialog::new().add_filter(tr("dialog.filter_sheets"), &["xlsx", "xlsm", "ods"]).pick_file(crate::file_dialog::in_view(|v: &mut DocView, p: std::path::PathBuf| {
                let Some(d) = dlg(v) else { return };
                d.book = p.display().to_string();
                match crate::merge_data::query_names(&p) {
                    Ok(q) if q.is_empty() => d.error = Some(tr("mail.no_queries").to_string()),
                    Ok(q) => {
                        d.query = q[0].0.clone();
                        d.allow = false;
                        d.queries = q;
                        d.error = None;
                    }
                    Err(e) => d.error = Some(e),
                }
            }));
        } else if let Some((label, exts)) = forms::file_filter(&d.src) {
            crate::file_dialog::FileDialog::new().add_filter(tr(label), exts).pick_file(crate::file_dialog::in_view(|v: &mut DocView, p: std::path::PathBuf| {
                let Some(d) = dlg(v) else { return };
                forms::set_location(&mut d.src, p.display().to_string());
                d.tables = apex_query::sources::db::list_tables(&d.src, "").unwrap_or_default();
            }));
        }
    }

    /// Takes the records read, when they are.
    fn poll_mail_job(&mut self, ctx: &egui::Context) {
        let Some((_, job)) = &self.mail.job else { return };
        if !job.is_finished() {
            ctx.request_repaint_after(web_time::Duration::from_millis(100));
            return;
        }
        let Some((what, job)) = self.mail.job.take() else { return };
        match job.join().unwrap_or_else(|_| Err("the reading stopped".into())) {
            Ok((fields, records)) => {
                self.notices.push(crate::app::Notice::info(trf("mail.loaded", &[("n", &records.len()), ("fields", &fields.len())])));
                self.mail.recipients = Some(Recipients::new(what, fields, records));
            }
            Err(e) => self.notices.push(crate::app::Notice::error(trf("mail.db_failed", &[("error", &e)]))),
        }
    }
}
