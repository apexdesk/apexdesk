//! Pages joined to pages: the finder `[[` and Ctrl+P open, a page's
//! preview over a link to it, what is related at the foot of a page, and
//! the day's page.

use apex_doc::Pos;
use apex_doc::notebook::links::{self, Rel};

use super::*;

/// Pages a finder lists at most.
const SHOWN: usize = 8;

/// What a finder is for.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Want {
    /// A link where `[[` was typed.
    Link(Pos),
    /// Going to a page.
    GoTo,
    /// A page's card on the canvas open, there.
    Card(f32, f32),
    /// A page's marker on the map open, there.
    Pin(f32, f32),
    /// A page to show beside the open one.
    Beside,
    /// A page (or the part under one of its headings) to show in the open
    /// one, where `![[` was typed (None: at the caret).
    Embed(Option<Pos>),
}

pub(super) struct Finder {
    pub want: Want,
    pub query: String,
    /// The row picked with the arrows.
    pub row: usize,
    pub rel: Rel,
    /// Where it shows (under the caret, or at the top).
    pub at: Option<egui::Pos2>,
    focused: bool,
    /// Embedding: the headings of the page named before `#`.
    heads: Option<(i64, Vec<String>)>,
}

/// What a finder row does.
#[derive(Clone)]
enum Choice {
    Page(i64),
    Create(String),
    /// The part of a page under a heading.
    Heading(i64, String),
}

impl Finder {
    pub fn new(want: Want) -> Finder {
        Finder { want, query: String::new(), row: 0, rel: Rel::Link, at: None, focused: false, heads: None }
    }
}

impl NoteView {
    /// Ctrl+P: the finder for going to a page.
    pub fn open_go_to(&mut self) {
        self.finder = Some(Finder::new(Want::GoTo));
    }

    /// The live page `title` names, or a new one with that title at the
    /// end of the section shown.
    fn page_or_new(&mut self, title: &str) -> Option<i64> {
        if let Some(id) = self.nb.page_named(title) {
            return Some(id);
        }
        let s = self.section?;
        let mut id = 0;
        let title = title.trim().to_string();
        self.change(|nb| id = nb.add_page(s, None, None, &title, now()));
        Some(id)
    }

    /// Changes the text of page `id` with `f` and writes it (the open
    /// page is changed in its editor and written as usual).
    pub(super) fn edit_page(&mut self, id: i64, f: impl FnOnce(&mut Document) -> bool) {
        if self.page == Some(id) {
            if let Some(ed) = &mut self.editor {
                f(&mut ed.doc);
            }
            return;
        }
        let mut doc = match self.store.load_page(id) {
            Ok(d) => d,
            Err(e) => return self.report(Err(e)),
        };
        if !f(&mut doc) {
            return;
        }
        let text = plain_text(&doc);
        let found = links::page_links(&doc);
        let t = now();
        let r = self.store.save_page(id, &doc, &text, &found, t);
        self.nb.texts.insert(id, text);
        self.nb.links.insert(id, found);
        self.nb.tasks.insert(id, apex_doc::notebook::tasks::tasks_in(&doc));
        if let Some(p) = self.nb.page_mut(id) {
            p.modified = t;
        }
        self.report(r);
    }

    /// After page `id` went from `old` to `new`: the links to it that
    /// showed its title show the new one.
    pub(super) fn retitle_links(&mut self, id: i64, old: &str, new: &str) {
        let mut from: Vec<i64> = self.nb.backlinks(id).into_iter().map(|t| t.0).collect();
        if self.page.is_some_and(|p| p != id && !from.contains(&p)) {
            from.push(self.page.unwrap_or_default());
        }
        for p in from {
            self.edit_page(p, |d| links::retitle_links(d, id, old, new));
        }
    }

    /// Opens the page a link points at.
    fn follow(&mut self, target: &str) {
        if let Some(id) = super::attachments::parse_attach(target) {
            self.open_attachment(id);
            return;
        }
        if let Some(back) = super::fragments::parse_origin(target) {
            self.open_request = Some(back);
            return;
        }
        match links::parse(target) {
            Some((id, _)) if self.nb.is_live(id) => {
                self.open_page(id);
                self.grid = false;
            }
            _ => self.notices.push(Notice::info(tr("nb.link_gone"))),
        }
    }

    /// The page for today (`YYYY-MM-DD`) wherever it is, else made on top
    /// of the section shown.
    pub fn today(&mut self) {
        let (y, m, d, _, _) = common::local_time(web_time::SystemTime::now());
        let title = format!("{y:04}-{m:02}-{d:02}");
        let found = self.nb.pages.iter().find(|p| p.title == title && self.nb.is_live(p.id) && !self.nb.sealed.contains(&p.section)).map(|p| p.id);
        let id = match found {
            Some(p) => p,
            None => {
                let steps = self.undo.len();
                let section = self.here();
                let mut id = 0;
                self.change(|nb| {
                    let first = nb.children(section, None).first().map(|p| p.id);
                    id = nb.add_page(section, None, None, &title, now());
                    // The newest day on top.
                    nb.move_page(id, section, None, first);
                });
                // One step for both.
                self.undo.truncate(steps + 1);
                id
            }
        };
        self.close_graph();
        self.open_page(id);
        self.grid = false;
    }

    /// What the editor asked for this frame: a link followed, `[[`.
    #[cfg(test)]
    pub(super) fn editor_requests_for_test(&mut self) {
        let ctx = egui::Context::default();
        let mut ui = egui::Ui::new(ctx, Id::new("test"), egui::UiBuilder::new());
        self.editor_requests(&mut ui);
    }

    pub(super) fn editor_requests(&mut self, ui: &egui::Ui) {
        let Some(ed) = &mut self.editor else { return };
        // What the page links to, as it is typed.
        if ed.doc.revision != self.links_rev
            && let Some(p) = self.page
        {
            self.links_rev = ed.doc.revision;
            self.nb.links.insert(p, links::page_links(&ed.doc));
        }
        let follow = ed.note_follow.take();
        let prompt = ed.link_prompt.take();
        // `![[` shows a page in this one.
        let bang = prompt.is_some_and(|at| at.offset > 0 && ed.doc.paragraphs.get(at.para).is_some_and(|p| p.text.get(at.offset - 1..at.offset) == Some("!")));
        let caret = ed.caret_screen;
        let hover = ed.hover_link.clone();
        if let Some(t) = follow {
            self.follow(&t);
        }
        if let Some(at) = prompt {
            let want = if bang { Want::Embed(Some(at)) } else { Want::Link(at) };
            self.finder = Some(Finder { at: caret.map(|c| c.left_bottom() + vec2(0.0, 4.0)), ..Finder::new(want) });
        }
        if let Some(t) = hover
            && self.finder.is_none()
            && let Some((id, rel)) = links::parse(&t)
        {
            self.preview(ui.ctx(), id, rel);
        }
    }

    /// A page's title and the start of its text, by the pointer.
    pub(super) fn preview(&self, ctx: &egui::Context, id: i64, rel: Rel) {
        let Some(pos) = ctx.pointer_hover_pos() else { return };
        egui::Area::new(Id::new(("note_preview", self.uid))).order(egui::Order::Tooltip).fixed_pos(pos + vec2(14.0, 18.0)).interactable(false).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(320.0);
                let Some(p) = self.nb.page(id).filter(|_| self.nb.is_live(id)) else {
                    ui.label(RichText::new(tr("nb.link_gone")).weak());
                    return;
                };
                if rel != Rel::Link {
                    ui.label(RichText::new(crate::note_view::rel_name(rel)).small().color(theme::NOTE_ACCENT));
                }
                ui.label(RichText::new(&p.title).strong().size(16.0));
                let text = self.nb.texts.get(&id).map_or("", String::as_str).trim();
                let snippet: String = text.chars().take(400).collect();
                if snippet.is_empty() {
                    ui.label(RichText::new(tr("nb.empty_page")).weak().italics());
                } else {
                    ui.label(RichText::new(if text.len() > snippet.len() { format!("{snippet}…") } else { snippet }).weak());
                }
                ui.label(RichText::new(tr("nb.click_to_open")).weak().small());
            });
        });
    }

    fn choices(&self, f: &Finder) -> Vec<Choice> {
        let embed = matches!(f.want, Want::Embed(_));
        if embed
            && let Some((_, h)) = f.query.split_once('#')
        {
            let h = apex_doc::notebook::fold(h.trim());
            return f.heads.iter().flat_map(|(id, heads)| heads.iter().filter(|x| apex_doc::notebook::fold(x).contains(&h)).map(|x| Choice::Heading(*id, x.clone()))).take(SHOWN).collect();
        }
        let q = f.query.trim();
        let mut v: Vec<Choice> = if q.is_empty() {
            let mut recent: Vec<&apex_doc::notebook::Page> = self.nb.pages.iter().filter(|p| self.nb.is_live(p.id)).collect();
            recent.sort_by_key(|p| std::cmp::Reverse(p.modified));
            recent.into_iter().map(|p| Choice::Page(p.id)).collect()
        } else {
            self.nb.find_pages(q).into_iter().map(Choice::Page).collect()
        };
        if f.want != Want::GoTo {
            v.retain(|c| !matches!(c, Choice::Page(p) if Some(*p) == self.page));
        }
        v.truncate(SHOWN);
        if embed {
            v.retain(|c| matches!(c, Choice::Page(p) if self.nb.page(*p).is_some_and(|p| p.kind == apex_doc::notebook::PageKind::Text)));
        } else if !q.is_empty() && self.nb.page_named(q).is_none() {
            v.push(Choice::Create(q.to_string()));
        }
        v
    }

    /// Embedding with `Page#`: the headings of the page named.
    fn find_heads(&self, f: &mut Finder) {
        let Some((name, _)) = f.query.split_once('#').filter(|_| matches!(f.want, Want::Embed(_))) else { return };
        let name = name.trim();
        let Some(id) = self.nb.page_named(name).or_else(|| self.nb.find_pages(name).first().copied()).filter(|&p| Some(p) != self.page) else {
            f.heads = None;
            return;
        };
        if f.heads.as_ref().is_none_or(|(p, _)| *p != id) {
            let heads = self.store.load_page(id).map(|d| apex_doc::notebook::embed::headings(&d)).unwrap_or_default();
            f.heads = Some((id, heads));
        }
    }

    /// Shows page `id` (or the part under `heading`) in the open one:
    /// where `![[` was typed (`at`), or after the caret's line.
    pub fn embed_page(&mut self, at: Option<Pos>, id: i64, heading: Option<&str>) {
        use apex_doc::notebook::embed;
        if Some(id) == self.page {
            return;
        }
        let Some(title) = self.nb.page(id).map(|p| p.title.clone()) else { return };
        if self.side.as_ref().is_some_and(|s| s.page == id) {
            self.flush_side();
        }
        let src = match self.store.load_page(id) {
            Ok(d) => d,
            Err(e) => return self.report(Err(e)),
        };
        let Some(ed) = &mut self.editor else { return };
        let Some(paras) = embed::excerpt(&src, heading) else { return };
        let c = embed::control(ed.doc.next_control_id(), id, heading, &title);
        ed.insert_embed(at, &embed::wrap(paras, &c));
    }

    /// The finder `[[` and Ctrl+P open.
    pub(super) fn finder_ui(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.finder.take() else { return };
        let choices = self.choices(&f);
        f.row = f.row.min(choices.len().saturating_sub(1));
        let (down, up, enter, esc) = ctx.input(|i| (i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if down {
            f.row = (f.row + 1).min(choices.len().saturating_sub(1));
        }
        if up {
            f.row = f.row.saturating_sub(1);
        }
        let mut chosen: Option<Choice> = None;
        let mut close = esc;
        let screen = ctx.content_rect();
        let at = f.at.unwrap_or(egui::pos2(screen.center().x - 200.0, screen.min.y + 120.0));
        let r = egui::Area::new(Id::new(("note_finder", self.uid))).order(egui::Order::Foreground).fixed_pos(at).constrain(true).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(400.0);
                let hint = match f.want {
                    Want::GoTo => tr("nb.go_to_hint"),
                    Want::Link(_) => tr("nb.link_hint"),
                    Want::Card(..) => tr("nb.card_hint"),
                    Want::Pin(..) => tr("nb.pin_hint"),
                    Want::Beside => tr("nb.split_hint"),
                    Want::Embed(_) => tr("nb.embed_hint"),
                };
                let te = ui.add(egui::TextEdit::singleline(&mut f.query).hint_text(hint).desired_width(f32::INFINITY));
                if !f.focused {
                    te.request_focus();
                    f.focused = true;
                }
                if te.changed() {
                    f.row = 0;
                    self.find_heads(&mut f);
                }
                if let Want::Link(_) = f.want {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(tr("nb.relation")).weak().small());
                        egui::ComboBox::from_id_salt(("note_rel", self.uid)).selected_text(crate::note_view::rel_name(f.rel)).show_ui(ui, |ui| {
                            for rel in Rel::ALL {
                                ui.selectable_value(&mut f.rel, rel, crate::note_view::rel_name(rel));
                            }
                        });
                    });
                }
                ui.separator();
                if choices.is_empty() {
                    ui.label(RichText::new(tr("nb.no_results")).weak());
                }
                for (n, c) in choices.iter().enumerate() {
                    let (label, sub) = match c {
                        Choice::Page(id) => {
                            let p = self.nb.page(*id);
                            let sec = p.and_then(|p| self.nb.section(p.section)).map(|s| s.name.clone()).unwrap_or_default();
                            (p.map(|p| p.title.clone()).unwrap_or_default(), sec)
                        }
                        Choice::Create(t) => (trf("nb.create_page", &[("title", t)]), String::new()),
                        Choice::Heading(id, h) => (h.clone(), self.nb.page(*id).map(|p| p.title.clone()).unwrap_or_default()),
                    };
                    let r = ui.horizontal(|ui| {
                        let r = ui.selectable_label(n == f.row, RichText::new(label).strong());
                        if !sub.is_empty() {
                            ui.label(RichText::new(sub).weak().small());
                        }
                        r
                    });
                    if r.inner.clicked() {
                        chosen = Some(c.clone());
                    }
                }
            });
        });
        if enter && choices.get(f.row).is_some() {
            chosen = choices.get(f.row).cloned();
        }
        // A click elsewhere closes it.
        if ctx.input(|i| i.pointer.any_pressed()) && !r.response.contains_pointer() && chosen.is_none() {
            close = true;
        }
        if let Some(c) = chosen {
            if let (Choice::Heading(id, h), Want::Embed(at)) = (&c, f.want) {
                self.embed_page(at, *id, Some(h));
                return;
            }
            let id = match c {
                Choice::Page(id) | Choice::Heading(id, _) => Some(id),
                Choice::Create(t) => self.page_or_new(&t),
            };
            if let Some(id) = id {
                match f.want {
                    Want::GoTo => {
                        self.open_page(id);
                        self.grid = false;
                    }
                    Want::Card(x, y) => self.put_card(id, x, y),
                    Want::Pin(x, y) => self.put_pin(id, x, y),
                    Want::Beside => self.open_beside(id),
                    Want::Embed(at) => self.embed_page(at, id, None),
                    Want::Link(at) => {
                        // The name typed when it is an alias; the title otherwise.
                        let p = self.nb.page(id);
                        let q = f.query.trim();
                        let shown = p.and_then(|p| p.aliases.iter().find(|a| apex_doc::notebook::fold(a) == apex_doc::notebook::fold(q))).or(p.map(|p| &p.title)).cloned().unwrap_or_default();
                        if let Some(ed) = &mut self.editor {
                            ed.finish_link_prompt(at, &shown, links::target(id, f.rel));
                            ctx.memory_mut(|m| m.request_focus(ed.canvas_id()));
                        }
                    }
                }
            }
            return;
        }
        if close {
            if let (Want::Link(_) | Want::Embed(Some(_)), Some(ed)) = (f.want, &self.editor) {
                ctx.memory_mut(|m| m.request_focus(ed.canvas_id()));
            }
            return;
        }
        self.finder = Some(f);
    }

    /// Whether page `id` has nothing related (its panel stays folded).
    pub(super) fn nothing_related(&self, id: i64) -> bool {
        self.nb.related(id).is_empty() && self.nb.mentions(id).is_empty()
    }

    /// What is related to the open page, at its foot.
    pub(super) fn related_ui(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.page else { return };
        let related = self.nb.related(id);
        let mentions = self.nb.mentions(id);
        let outgoing: Vec<i64> = self.nb.outgoing(id).into_iter().map(|t| t.0).collect();
        let n = related.iter().filter(|r| r.via.is_none()).count() + mentions.len();
        let empty = related.is_empty() && mentions.is_empty();
        let open = self.related_open && !empty;
        let head = ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            crate::icons::paint(ui.painter(), r, if open { crate::icons::Icon::ChevronDown } else { crate::icons::Icon::ChevronUp }, ui.visuals().text_color());
            ui.add(egui::Label::new(RichText::new(format!("{} ({n})", tr("nb.related"))).strong()).sense(Sense::click()))
        });
        if !empty && (head.inner.clicked() || head.response.clicked()) {
            self.related_open = !open;
            let r = self.store.set_setting("related", if self.related_open { "1" } else { "0" });
            self.report(r);
        }
        if !open {
            return;
        }
        let mut go = None;
        let mut link = None;
        egui::ScrollArea::vertical().id_salt(("note_related", self.uid)).auto_shrink([false, true]).show(ui, |ui| {
            let near: Vec<&apex_doc::notebook::Related> = related.iter().filter(|r| r.via.is_none()).collect();
            let far: Vec<&apex_doc::notebook::Related> = related.iter().filter(|r| r.via.is_some()).collect();
            for (out, key) in [(true, "nb.links_to"), (false, "nb.linked_from")] {
                let group: Vec<&&apex_doc::notebook::Related> = near.iter().filter(|r| outgoing.contains(&r.page) == out).collect();
                if group.is_empty() {
                    continue;
                }
                ui.label(RichText::new(tr(key)).weak().small());
                for r in group {
                    ui.horizontal(|ui| {
                        if ui.link(self.title_of(r.page)).clicked() {
                            go = Some(r.page);
                        }
                        if r.rel != Rel::Link {
                            ui.label(RichText::new(crate::note_view::rel_name(r.rel)).small().color(theme::NOTE_ACCENT));
                        }
                    });
                }
                ui.add_space(4.0);
            }
            if !mentions.is_empty() {
                ui.label(RichText::new(tr("nb.mentioned")).weak().small());
                for m in &mentions {
                    ui.horizontal(|ui| {
                        if ui.link(self.title_of(*m)).clicked() {
                            go = Some(*m);
                        }
                        if ui.small_button(tr("nb.make_link")).on_hover_text(tr("nb.make_link_tip")).clicked() {
                            link = Some(*m);
                        }
                    });
                }
            }
            if !far.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(tr("nb.two_steps")).weak().small());
                for r in far {
                    ui.horizontal(|ui| {
                        if ui.link(self.title_of(r.page)).clicked() {
                            go = Some(r.page);
                        }
                        let via = self.title_of(r.via.unwrap_or_default());
                        ui.label(RichText::new(trf("nb.via", &[("page", &via)])).weak().small());
                    });
                }
            }
        });
        if let Some(m) = link {
            let names: Vec<String> = self.nb.page(id).map(|p| std::iter::once(p.title.clone()).chain(p.aliases.clone()).collect()).unwrap_or_default();
            self.edit_page(m, |d| names.iter().any(|n| links::link_mention(d, n, id)));
        }
        if let Some(p) = go {
            self.open_page(p);
        }
    }

    pub(super) fn title_of(&self, id: i64) -> String {
        self.nb.page(id).map(|p| p.title.clone()).unwrap_or_default()
    }

    /// The tags of the notebook, to search one.
    pub(super) fn tags_ui(&mut self, ui: &mut egui::Ui, height: f32) {
        let tags = self.nb.tags();
        if tags.is_empty() {
            return;
        }
        ui.add_space(6.0);
        ui.label(RichText::new(tr("nb.tags")).strong());
        egui::ScrollArea::vertical().id_salt("note_tags_scroll").auto_shrink([false, true]).max_height(height).show(ui, |ui| {
            for (tag, pages) in &tags {
                let depth = tag.matches('/').count();
                let name = tag.rsplit('/').next().unwrap_or(tag);
                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 12.0);
                    let on = self.search.trim().eq_ignore_ascii_case(&format!("#{tag}"));
                    if ui.selectable_label(on, format!("#{name}")).clicked() {
                        self.search = if on { String::new() } else { format!("#{tag}") };
                    }
                    ui.label(RichText::new(pages.len().to_string()).weak().small());
                });
            }
        });
    }
}
