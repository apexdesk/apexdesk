//! Citations and bibliography: the toolstrip group, the source editor and the
//! source manager.

use apex_doc::{CiteStyle, Field, FieldKind, Source, SourceKind, TocOpts};

use super::refs::RefDialog;
use super::*;
use crate::widgets::DialogResult;

const KINDS: [(SourceKind, &str); 5] = [(SourceKind::Book, "cite.kind_book"), (SourceKind::Article, "cite.kind_article"), (SourceKind::Web, "cite.kind_web"), (SourceKind::Report, "cite.kind_report"), (SourceKind::Other, "cite.kind_other")];

fn kind_label(k: SourceKind) -> &'static str {
    KINDS.iter().find(|x| x.0 == k).map_or("", |x| tr(x.1))
}

pub(super) struct SourceDlg {
    src: Source,
    /// The tag of the source being edited (None: a new one).
    editing: Option<String>,
    /// Cite the new source at the caret.
    cite: bool,
    error: Option<&'static str>,
}

pub(super) struct SourcesDlg {
    pick: Option<usize>,
}

/// A short line for a source in lists.
/// A citation style's name in the interface language.
fn style_name(st: CiteStyle) -> &'static str {
    match st {
        CiteStyle::ChicagoAuthorDate => tr("cite.chicago_ad"),
        CiteStyle::ChicagoNotes => tr("cite.chicago_notes"),
        _ => st.name(),
    }
}

fn source_line(s: &Source) -> String {
    let who = s.people().first().map_or_else(String::new, |p| p.0.clone());
    let mut out = String::new();
    if !who.is_empty() {
        out.push_str(&who);
        out.push_str(": ");
    }
    out.push_str(&s.title);
    if !s.year.is_empty() {
        out.push_str(&format!(" ({})", s.year));
    }
    if out.chars().count() > 60 {
        out = format!("{}…", out.chars().take(60).collect::<String>());
    }
    out
}

impl DocView {
    /// The Citations & Bibliography group of the References tab.
    pub(super) fn biblio_group(&mut self, b: &mut crate::toolstrip::Body) {
        use crate::toolstrip;
        let a = b.accent();
        b.group("toolstrip.g.citations", Icon::Caption, |ui| {
            toolstrip::large_menu(ui, Icon::Caption, tr("cite.insert"), "", a, |ui| {
                let sources: Vec<(String, String)> = self.doc.biblio.sources.iter().map(|s| (s.tag.clone(), source_line(s))).collect();
                for (tag, line) in sources {
                    if ui.button(line).clicked() {
                        self.insert_citation(&tag);
                        ui.close();
                    }
                }
                if !self.doc.biblio.sources.is_empty() {
                    ui.separator();
                }
                // The sources of the notebooks open.
                let noted = crate::note_view::sources::all();
                if !noted.is_empty() {
                    crate::widgets::menu_button(ui, tr("cite.from_notebooks"), |ui| {
                        for s in noted {
                            if ui.button(source_line(&s)).clicked() {
                                self.cite_source(s);
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                }
                if toolstrip::menu_item(ui, Icon::New, &format!("{}…", tr("cite.add_source"))).clicked() {
                    self.refs.dialog = Some(RefDialog::Source(Box::new(SourceDlg { src: Source::default(), editing: None, cite: true, error: None })));
                    ui.close();
                }
            });
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Bookmark), &format!("{}…", tr("cite.manage")), "", a, false).clicked() {
                    self.refs.dialog = Some(RefDialog::Sources(Box::new(SourcesDlg { pick: None })));
                }
                let cur = self.doc.biblio.style;
                toolstrip::small_menu(ui, Some(Icon::Styles), &format!("{}: {}", tr("cite.style"), style_name(cur)), "", a, |ui| {
                    for st in CiteStyle::ALL {
                        if ui.radio(cur == st, style_name(st)).clicked() {
                            self.set_cite_style(st);
                            ui.close();
                        }
                    }
                });
                if toolstrip::small(ui, Some(Icon::Toc), tr("cite.bibliography"), "", a, false).clicked() {
                    self.insert_bibliography();
                }
            });
        });
    }

    /// Cites a source from elsewhere: the same one of the document, or
    /// added to it.
    pub fn cite_source(&mut self, s: Source) {
        let same = |x: &Source| Source { tag: String::new(), ..x.clone() } == Source { tag: String::new(), ..s.clone() };
        if let Some(tag) = self.doc.biblio.sources.iter().find(|x| same(x)).map(|x| x.tag.clone()) {
            self.insert_citation(&tag);
            return;
        }
        let mark = self.doc.undo_mark();
        let old = self.doc.biblio.clone();
        let mut src = s;
        src.tag = src.tag.trim().to_string();
        if src.tag.is_empty() || self.doc.biblio.source(&src.tag).is_some() {
            src.tag = self.doc.biblio.new_tag(&src);
        }
        let tag = src.tag.clone();
        self.doc.biblio.sources.push(src);
        self.insert_citation(&tag);
        self.biblio_changed(mark, old);
    }

    fn insert_citation(&mut self, tag: &str) {
        let field = Field::new(FieldKind::Citation(tag.to_string()));
        if self.doc.biblio.style.in_notes() {
            // Chicago notes: the citation goes in a new footnote.
            let mut field = field;
            field.cached = self.doc.biblio.source(tag).map(|s| s.citation(self.doc.biblio.style, 0)).unwrap_or_default();
            self.clear_block();
            let end = self.doc.insert_note_with(self.sel, apex_doc::Note::with_field(apex_doc::NoteKind::Foot, field));
            self.sel = apex_doc::Selection::caret(end);
            self.scroll_to_caret = true;
        } else {
            self.insert_field(field);
        }
        self.update_bibliographies();
    }

    fn insert_bibliography(&mut self) {
        let o = TocOpts { bib: true, links: false, page_numbers: false, ..TocOpts::default() };
        let s = self.doc.insert_toc(self.sel, o, tr("cite.no_sources"));
        self.set_sel(s);
    }

    fn set_cite_style(&mut self, st: CiteStyle) {
        if self.doc.biblio.style == st {
            return;
        }
        let mark = self.doc.undo_mark();
        let old = self.doc.biblio.clone();
        self.doc.biblio.style = st;
        self.biblio_changed(mark, old);
    }

    /// After the sources or the style change (from `old`, edits since
    /// `mark`): citations show anew and bibliographies are rebuilt, all
    /// one undo step.
    fn biblio_changed(&mut self, mark: apex_doc::UndoMark, old: apex_doc::Bibliography) {
        let new = std::mem::replace(&mut self.doc.biblio, old);
        if new != self.doc.biblio {
            self.doc.set_biblio(new, self.sel);
        }
        self.update_bibliographies();
        self.doc.join_undo(mark);
    }

    fn update_bibliographies(&mut self) {
        for (a, _) in self.doc.toc_blocks().into_iter().rev() {
            if self.doc.paragraphs[a].style.toc.is_some_and(|t| t.opts.bib) {
                let sel = self.sel;
                if self.doc.update_toc(a, sel, tr("cite.no_sources"))
                    && let Some(s) = self.doc.last_sel_after()
                {
                    self.sel = s;
                }
            }
        }
    }

    /// The source editor and the source manager.
    pub(super) fn biblio_dialog(&mut self, ctx: &egui::Context, d: RefDialog) -> Option<RefDialog> {
        match d {
            RefDialog::Source(mut s) => {
                let title = tr(if s.editing.is_some() { "cite.edit_title" } else { "cite.new_title" });
                let style = self.doc.biblio.style;
                let res = widgets::dialog(ctx, "doc_source", title, 460.0, |ui| source_page(ui, &mut s, style));
                match res {
                    DialogResult::None => return Some(RefDialog::Source(s)),
                    DialogResult::Cancel => self.focus = true,
                    DialogResult::Ok => {
                        if s.src.title.trim().is_empty() && s.src.authors.trim().is_empty() {
                            s.error = Some("cite.need_title");
                            return Some(RefDialog::Source(s));
                        }
                        let mark = self.doc.undo_mark();
                        let old = self.doc.biblio.clone();
                        let b = &mut self.doc.biblio;
                        match &s.editing {
                            Some(tag) => {
                                if let Some(x) = b.sources.iter_mut().find(|x| x.tag == *tag) {
                                    *x = Source { tag: tag.clone(), ..s.src.clone() };
                                }
                            }
                            None => {
                                let mut src = s.src.clone();
                                src.tag = src.tag.trim().to_string();
                                if src.tag.is_empty() || b.source(&src.tag).is_some() {
                                    src.tag = b.new_tag(&src);
                                }
                                let tag = src.tag.clone();
                                b.sources.push(src);
                                if s.cite {
                                    self.insert_citation(&tag);
                                }
                            }
                        }
                        self.biblio_changed(mark, old);
                        self.focus = true;
                    }
                }
                None
            }
            RefDialog::Sources(mut m) => {
                let mut next = None;
                let sources: Vec<String> = self.doc.biblio.sources.iter().map(|s| format!("{}  —  {}", s.tag, source_line(s))).collect();
                let cited: Vec<bool> = {
                    let n = self.doc.cite_numbers();
                    self.doc.biblio.sources.iter().map(|s| n.contains_key(&s.tag.to_lowercase())).collect()
                };
                let mut delete = None;
                let res = widgets::dialog(ctx, "doc_sources", tr("cite.manage_title"), 520.0, |ui| {
                    widgets::section(ui, tr("cite.sources_in_doc").trim_end_matches(':'));
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_size(vec2(ui.available_width(), 220.0));
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            if sources.is_empty() {
                                ui.label(egui::RichText::new(tr("cite.no_sources")).weak());
                            }
                            for (i, s) in sources.iter().enumerate() {
                                let mark = if cited[i] { "✓ " } else { "   " };
                                if ui.selectable_label(m.pick == Some(i), format!("{mark}{s}")).clicked() {
                                    m.pick = Some(i);
                                }
                            }
                        });
                    });
                    ui.label(egui::RichText::new(tr("cite.cited_hint")).small().weak());
                    ui.horizontal(|ui| {
                        if ui.button(format!("{}…", tr("cite.new"))).clicked() {
                            next = Some(RefDialog::Source(Box::new(SourceDlg { src: Source::default(), editing: None, cite: false, error: None })));
                        }
                        let picked = m.pick.filter(|&i| i < sources.len());
                        if ui.add_enabled(picked.is_some(), egui::Button::new(format!("{}…", tr("cite.edit")))).clicked()
                            && let Some(i) = picked
                        {
                            let src = self.doc.biblio.sources[i].clone();
                            next = Some(RefDialog::Source(Box::new(SourceDlg { editing: Some(src.tag.clone()), src, cite: false, error: None })));
                        }
                        if ui.add_enabled(picked.is_some_and(|i| !cited[i]), egui::Button::new(tr("cite.delete"))).on_disabled_hover_text(tr("cite.delete_cited")).clicked() {
                            delete = picked;
                        }
                    });
                });
                if let Some(i) = delete {
                    let mark = self.doc.undo_mark();
                    let old = self.doc.biblio.clone();
                    self.doc.biblio.sources.remove(i);
                    m.pick = None;
                    self.biblio_changed(mark, old);
                }
                if next.is_some() {
                    return next;
                }
                match res {
                    DialogResult::None => Some(RefDialog::Sources(m)),
                    _ => {
                        self.focus = true;
                        None
                    }
                }
            }
            other => Some(other),
        }
    }
}

/// The rows of a source's fields (in a form grid). True when one was
/// left or the kind changed: the time to keep what was typed.
fn source_rows(ui: &mut egui::Ui, salt: &str, src: &mut Source) -> bool {
    let mut done = false;
    widgets::form_row(ui, tr("cite.kind").trim_end_matches(':'), |ui| {
        egui::ComboBox::from_id_salt((salt, "kind")).width(200.0).selected_text(kind_label(src.kind)).show_ui(ui, |ui| {
            for (k, key) in KINDS {
                done |= ui.selectable_value(&mut src.kind, k, tr(key)).changed();
            }
        });
    });
    let mut row = |ui: &mut egui::Ui, key: &'static str, v: &mut String, hint: &str| {
        widgets::form_row(ui, tr(key).trim_end_matches(':'), |ui| done |= ui.add(egui::TextEdit::singleline(v).hint_text(hint).desired_width(f32::INFINITY)).lost_focus());
    };
    row(ui, "cite.authors", &mut src.authors, tr("cite.authors_hint"));
    row(ui, "cite.title", &mut src.title, "");
    row(ui, "cite.year", &mut src.year, "");
    match src.kind {
        SourceKind::Article => {
            row(ui, "cite.journal", &mut src.journal, "");
            row(ui, "cite.volume", &mut src.volume, "");
            row(ui, "cite.pages", &mut src.pages, "");
        }
        SourceKind::Web => {
            row(ui, "cite.site", &mut src.journal, "");
            row(ui, "cite.url", &mut src.url, "https://");
        }
        _ => {
            row(ui, "cite.city", &mut src.city, "");
            row(ui, "cite.publisher", &mut src.publisher, "");
        }
    }
    done
}

/// A source's fields in a form of their own (a notebook's source page).
pub fn source_fields(ui: &mut egui::Ui, id: &str, src: &mut Source) -> bool {
    widgets::form(ui, id, |ui| source_rows(ui, id, src))
}

/// A source as a bibliography entry, in plain text (APA).
pub fn source_entry(src: &Source) -> String {
    if src.title.trim().is_empty() && src.authors.trim().is_empty() {
        return String::new();
    }
    src.entry(CiteStyle::Apa, 1).into_iter().map(|(t, _)| t).collect()
}

fn source_page(ui: &mut egui::Ui, s: &mut SourceDlg, style: CiteStyle) {
    let src = &mut s.src;
    widgets::section(ui, tr("refs.sec_source"));
    widgets::form(ui, "source_grid", |ui| {
        source_rows(ui, "source", src);
        widgets::form_row(ui, tr("cite.tag").trim_end_matches(':'), |ui| ui.add_enabled(s.editing.is_none(), egui::TextEdit::singleline(&mut src.tag).hint_text(tr("cite.tag_auto")).desired_width(f32::INFINITY)));
    });
    widgets::section(ui, tr("cite.preview").trim_end_matches(':'));
    if s.src.title.trim().is_empty() && s.src.authors.trim().is_empty() {
        ui.label("");
        if let Some(e) = s.error {
            ui.label(egui::RichText::new(tr(e)).color(ui.visuals().error_fg_color));
        }
        return;
    }
    let mut job = egui::text::LayoutJob::default();
    let fmt = |it: bool| egui::TextFormat { font_id: egui::FontId::proportional(13.0), color: ui.visuals().text_color(), italics: it, ..Default::default() };
    for (t, it) in s.src.entry(style, 1) {
        job.append(&t, 0.0, fmt(it));
    }
    job.wrap.max_width = ui.available_width();
    ui.label(job);
    if let Some(e) = s.error {
        ui.label(egui::RichText::new(tr(e)).color(ui.visuals().error_fg_color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citation_style_undoes_with_its_bibliography() {
        let mut v = DocView::new(Document::from_plain_text("Text\n"));
        v.doc.biblio.sources.push(Source { tag: "Doe20".into(), authors: "Doe, John".into(), title: "A Book".into(), year: "2020".into(), ..Default::default() });
        v.sel = apex_doc::Selection::caret(apex_doc::Pos::new(0, 4));
        v.insert_citation("Doe20");
        v.sel = apex_doc::Selection::caret(apex_doc::Pos::new(1, 0));
        v.insert_bibliography();
        let before = (v.doc.to_plain_text(), v.doc.biblio.clone());
        v.set_cite_style(CiteStyle::Ieee);
        assert_ne!(v.doc.to_plain_text(), before.0);
        v.undo();
        assert_eq!((v.doc.to_plain_text(), v.doc.biblio.clone()), before);
        v.redo();
        assert_eq!(v.doc.biblio.style, CiteStyle::Ieee);
    }
}
