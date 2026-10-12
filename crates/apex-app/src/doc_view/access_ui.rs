//! Review > Check Accessibility for documents: a window listing what
//! makes the document hard to read for people with disabilities (see
//! `apex_doc::access`); clicking an item goes to it and its button fixes
//! it (or opens the dialog that does).

use apex_doc::access::{Category, Issue, Target, lang_tag};
use apex_doc::{ParaKind, Pos, Selection};
use eframe::egui::{self, Id, RichText};

use super::DocView;
use crate::i18n::tr;
use crate::widgets::{self, DialogResult};

#[derive(Default)]
pub(super) struct AccessState {
    pub open: bool,
    /// The issues and the document revision they are for.
    cache: Option<(u64, Vec<Issue>)>,
    sel: Option<usize>,
    /// The document title being typed.
    title: Option<String>,
}

fn category_key(c: Category) -> &'static str {
    match c {
        Category::MissingAlt => "access.cat_alt",
        Category::NoHeader => "access.cat_header",
        Category::Contrast => "access.cat_contrast",
        Category::HeadingOrder => "access.cat_heading",
        Category::LinkText => "access.cat_link",
        Category::NoTitle => "access.cat_title",
        Category::NoLanguage => "access.cat_lang",
    }
}

fn why_key(c: Category) -> &'static str {
    match c {
        Category::MissingAlt => "access.why_alt",
        Category::NoHeader => "access.why_header",
        Category::Contrast => "access.why_contrast",
        Category::HeadingOrder => "access.why_heading",
        Category::LinkText => "access.why_link",
        Category::NoTitle => "access.why_title",
        Category::NoLanguage => "access.why_lang",
    }
}

fn fix_key(c: Category) -> &'static str {
    match c {
        Category::MissingAlt => "access.fix_alt",
        Category::NoHeader => "access.fix_header",
        Category::Contrast => "access.fix_color",
        Category::HeadingOrder => "access.fix_heading",
        Category::LinkText => "access.fix_link",
        Category::NoTitle => "access.fix_title",
        Category::NoLanguage => "access.fix_lang",
    }
}

impl DocView {
    pub(super) fn toggle_accessibility(&mut self) {
        self.access.open = !self.access.open;
    }

    fn access_issues(&mut self) -> Vec<Issue> {
        let rev = self.doc.revision;
        if self.access.cache.as_ref().is_none_or(|(r, _)| *r != rev) {
            self.access.cache = Some((rev, self.doc.check_accessibility()));
        }
        self.access.cache.as_ref().map(|c| c.1.clone()).unwrap_or_default()
    }

    pub(super) fn access_window(&mut self, ctx: &egui::Context) {
        self.title_dialog(ctx);
        if !self.access.open {
            return;
        }
        let issues = self.access_issues();
        let mut open = true;
        let (mut go, mut fix) = (None, None);
        let sel = self.access.sel.filter(|i| *i < issues.len());
        let screen = ctx.content_rect();
        let at = egui::pos2(screen.max.x - 380.0, screen.min.y + 140.0);
        crate::widgets::esc_closes(ctx, Id::new(("doc_access", self.uid)), &mut open);
        egui::Window::new(tr("access.title")).id(Id::new(("doc_access", self.uid))).default_pos(at).open(&mut open).collapsible(false).resizable(true).default_width(340.0).default_height(460.0).show(ctx, |ui| {
            if issues.is_empty() {
                ui.label(RichText::new(tr("access.none")).strong());
                ui.label(RichText::new(tr("access.doc_none_hint")).weak());
                return;
            }
            egui::ScrollArea::vertical().max_height((ui.available_height() - 110.0).max(160.0)).auto_shrink([false, true]).show(ui, |ui| {
                for errors in [true, false] {
                    let n = issues.iter().filter(|i| i.category.is_error() == errors).count();
                    if n == 0 {
                        continue;
                    }
                    let head = format!("{} ({n})", tr(if errors { "access.errors" } else { "access.warnings" }));
                    let color = if errors { ui.visuals().error_fg_color } else { ui.visuals().warn_fg_color };
                    egui::CollapsingHeader::new(RichText::new(head).strong().color(color)).id_salt(("doc_access_level", errors)).default_open(true).show(ui, |ui| {
                        for cat in Category::ALL.into_iter().filter(|c| c.is_error() == errors) {
                            let items: Vec<usize> = (0..issues.len()).filter(|&k| issues[k].category == cat).collect();
                            if items.is_empty() {
                                continue;
                            }
                            egui::CollapsingHeader::new(format!("{} ({})", tr(category_key(cat)), items.len())).id_salt(("doc_access_cat", cat)).default_open(true).show(ui, |ui| {
                                for k in items {
                                    let i = &issues[k];
                                    ui.horizontal(|ui| {
                                        let label = if i.label.is_empty() { tr("access.document").to_string() } else { i.label.clone() };
                                        let text = match i.ratio {
                                            Some(r) => format!("{label}  ({r:.1}:1)"),
                                            None => label,
                                        };
                                        if ui.add(egui::Button::selectable(sel == Some(k), text).truncate()).clicked() {
                                            go = Some(k);
                                        }
                                        if ui.small_button(tr(fix_key(cat))).on_hover_text(tr("access.fix_tip")).clicked() {
                                            fix = Some(k);
                                        }
                                    });
                                }
                            });
                        }
                    });
                }
            });
            ui.separator();
            match sel.map(|k| issues[k].category) {
                Some(cat) => {
                    ui.label(RichText::new(tr("access.why")).strong());
                    ui.label(tr(why_key(cat)));
                }
                None => {
                    ui.label(RichText::new(tr("access.hint")).weak());
                }
            }
        });
        if !open {
            self.access.open = false;
        }
        if let Some(k) = go.or(fix) {
            self.access.sel = Some(k);
            self.go_to_issue(&issues[k].target);
        }
        if let Some(k) = fix {
            self.fix_issue(&issues[k]);
        }
    }

    /// Selects what an issue is about.
    fn go_to_issue(&mut self, t: &Target) {
        let one = |p: Pos| Selection { anchor: p, cursor: Pos::new(p.para, p.offset + apex_doc::OBJECT.len_utf8()) };
        match *t {
            Target::Image(p) => self.set_sel(one(p)),
            Target::Shape(p) => self.select_shape(p),
            Target::Table(para) | Target::Heading { para, .. } => self.set_sel(Selection::caret(Pos::new(para, 0))),
            Target::Text(s) => self.set_sel(s),
            Target::Document => {}
        }
        self.scroll_to_caret = true;
    }

    fn fix_issue(&mut self, i: &Issue) {
        match i.target {
            Target::Image(p) => self.open_picture_props(p, 2),
            Target::Shape(_) => self.open_shape_dialog(4),
            Target::Table(para) => {
                self.doc.set_table_props(Selection::caret(Pos::new(para, 0)), &|t| t.header_rows = t.header_rows.max(1));
            }
            Target::Heading { para, level } => {
                self.doc.apply_style(para, para, self.sel, ParaKind::Heading(level), 0);
            }
            Target::Text(s) if i.category == Category::Contrast => self.doc.fix_contrast(s),
            Target::Text(_) => self.open_link_dialog(None),
            Target::Document if i.category == Category::NoTitle => self.access.title = Some(self.doc.suggested_title()),
            Target::Document => self.doc.set_language(lang_tag(apex_i18n::current()), self.sel),
        }
    }

    /// The document title, typed for the NoTitle fix.
    fn title_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut t) = self.access.title.take() else { return };
        let res = widgets::dialog(ctx, "doc_access_title", tr("access.fix_title"), 380.0, |ui| {
            ui.label(tr("access.title_prompt"));
            let r = ui.add(egui::TextEdit::singleline(&mut t).desired_width(f32::INFINITY));
            if ui.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
        });
        match res {
            DialogResult::None => self.access.title = Some(t),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                let t = t.trim().to_string();
                if !t.is_empty() && t != self.doc.meta.title {
                    self.doc.meta.title = t;
                    self.doc.modified = true;
                    // The list updates.
                    self.access.cache = None;
                }
            }
        }
    }
}
