//! Insert > Quick Parts: the building blocks ([`crate::blocks`]). The
//! organizer lists them by category with a preview and inserts one; Save
//! Selection as Building Block (Alt+F3) keeps the selection in the user's
//! glossary; F3 after a block's name puts the block in its place.

use apex_doc::{Document, Pos, Selection};
use eframe::egui;

use super::DocView;
use crate::blocks::{self, Item};
use crate::i18n::{tr, trf};
use crate::widgets::{self, DialogResult};

pub(super) enum BlocksDlg {
    Organizer { items: Vec<Item>, filter: String, pick: Option<usize> },
    Save { name: String, category: String, doc: Document },
}

impl DocView {
    pub(super) fn blocks_menu(&mut self, ui: &mut egui::Ui, a: egui::Color32) {
        use crate::icons::Icon;
        use crate::toolstrip;
        toolstrip::large_menu(ui, Icon::Template, tr("blocks.quick_parts"), tr("blocks.quick_parts_tip"), a, |ui| {
            let items = blocks::all();
            let mut cat: Option<String> = None;
            for it in items.iter().take(30) {
                if cat.as_deref() != Some(it.entry.category.as_str()) {
                    ui.weak(if it.entry.category.is_empty() { tr("blocks.general") } else { it.entry.category.as_str() });
                    cat = Some(it.entry.category.clone());
                }
                if ui.button(&it.entry.name).on_hover_text(&it.entry.preview).clicked() {
                    self.insert_block(it, None);
                    ui.close();
                }
            }
            if !items.is_empty() {
                ui.separator();
            }
            if toolstrip::menu_item_keys(ui, Icon::Template, &format!("{}…", tr("blocks.organizer")), "F3").clicked() {
                self.open_blocks_organizer();
                ui.close();
            }
            if toolstrip::menu_item_keys(ui, Icon::Save, &format!("{}…", tr("blocks.save")), "Alt+F3").clicked() {
                self.open_save_block();
                ui.close();
            }
        });
    }

    pub(super) fn open_blocks_organizer(&mut self) {
        self.blocks = Some(BlocksDlg::Organizer { items: blocks::all(), filter: String::new(), pick: None });
    }

    /// Alt+F3: the selection as a new block.
    pub(super) fn open_save_block(&mut self) {
        if self.sel.is_empty() {
            self.notices.push(crate::app::Notice::info(tr("blocks.select_first")));
            return;
        }
        let doc = Document::from_paragraphs(self.doc.slice(self.sel.start(), self.sel.end()));
        // Word's default name: the start of the text.
        let name: String = doc.paragraphs.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(" ").split_whitespace().take(3).collect::<Vec<_>>().join(" ");
        self.blocks = Some(BlocksDlg::Save { name: name.chars().take(32).collect(), category: String::new(), doc });
    }

    /// Puts a block at the selection (or over `name`, the typed name).
    fn insert_block(&mut self, item: &Item, name: Option<Selection>) {
        let Some(doc) = blocks::load(item) else {
            self.notices.push(crate::app::Notice::error(trf("blocks.cant_read", &[("name", &item.entry.name)])));
            return;
        };
        let sel = name.unwrap_or(self.sel);
        let p = if doc.paragraphs.len() == 1 && doc.paragraphs[0].runs.iter().all(|r| r.attrs == apex_doc::Attrs::default()) && doc.paragraphs[0].style.kind == apex_doc::ParaKind::Normal {
            // Plain text takes the formatting where it goes, as Word's
            // AutoText does for text without formatting.
            let attrs = Some(self.attrs_at_caret());
            self.doc.insert_text(sel, &doc.paragraphs[0].text, attrs)
        } else {
            self.doc.insert_paragraphs(sel, &doc.paragraphs)
        };
        self.sel = Selection::caret(p);
        self.scroll_to_caret = true;
    }

    /// F3: the block named by the word before the caret, in its place.
    /// Returns whether there was one.
    pub(super) fn expand_block(&mut self) -> bool {
        if !self.sel.is_empty() {
            return false;
        }
        let p = self.sel.cursor;
        let text = &self.doc.paragraphs[p.para].text;
        let start = blocks::name_start(text, p.offset);
        let word = text[start..p.offset].to_string();
        let items = blocks::all();
        let Some(item) = blocks::find(&items, &word) else { return false };
        self.insert_block(item, Some(Selection { anchor: Pos::new(p.para, start), cursor: p }));
        true
    }

    pub(super) fn blocks_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.blocks.take() else { return };
        match &mut d {
            BlocksDlg::Organizer { items, filter, pick } => {
                let mut insert = false;
                let mut delete = None;
                let res = widgets::dialog(ctx, "blocks_organizer", tr("blocks.organizer"), 560.0, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(tr("blocks.filter"));
                        ui.add(egui::TextEdit::singleline(filter).desired_width(220.0));
                    });
                    if items.is_empty() {
                        ui.weak(tr("blocks.none"));
                    }
                    let f = filter.to_lowercase();
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        egui::Grid::new("blocks_grid").num_columns(3).striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
                            ui.strong(tr("blocks.name"));
                            ui.strong(tr("blocks.category"));
                            ui.strong(tr("blocks.source"));
                            ui.end_row();
                            for (i, it) in items.iter().enumerate() {
                                let e = &it.entry;
                                if !f.is_empty() && !format!("{} {} {}", e.name, e.category, e.preview).to_lowercase().contains(&f) {
                                    continue;
                                }
                                let r = ui.selectable_label(*pick == Some(i), &e.name);
                                if r.clicked() {
                                    *pick = Some(i);
                                }
                                if r.double_clicked() {
                                    insert = true;
                                }
                                ui.label(if e.category.is_empty() { tr("blocks.general") } else { e.category.as_str() });
                                let src = it.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                ui.label(if it.shared { src } else { tr("blocks.personal").to_string() });
                                ui.end_row();
                            }
                        });
                    });
                    ui.separator();
                    let cur = pick.and_then(|i| items.get(i));
                    ui.label(egui::RichText::new(cur.map_or("", |it| it.entry.preview.as_str())).weak());
                    ui.horizontal(|ui| {
                        if ui.add_enabled(cur.is_some(), egui::Button::new(tr("blocks.insert"))).clicked() {
                            insert = true;
                        }
                        let own = cur.is_some_and(|it| !it.shared);
                        if ui.add_enabled(own, egui::Button::new(tr("blocks.delete"))).on_disabled_hover_text(tr("blocks.shared_hint")).clicked() {
                            delete = *pick;
                        }
                    });
                });
                if let Some(i) = delete {
                    let it = items[i].clone();
                    if let Err(e) = blocks::delete_in(&it.file, &it.entry.name) {
                        self.notices.push(crate::app::Notice::error(e.to_string()));
                    }
                    *items = blocks::all();
                    *pick = None;
                }
                if insert || res == DialogResult::Ok {
                    if let Some(it) = pick.and_then(|i| items.get(i)).cloned() {
                        self.insert_block(&it, None);
                    }
                    return;
                }
                if res == DialogResult::Cancel {
                    return;
                }
            }
            BlocksDlg::Save { name, category, doc } => {
                let cats: Vec<String> = {
                    let mut c: Vec<String> = blocks::all().into_iter().map(|i| i.entry.category).filter(|c| !c.is_empty()).collect();
                    c.dedup();
                    c
                };
                let res = widgets::dialog(ctx, "blocks_save", tr("blocks.save"), 400.0, |ui| {
                    widgets::form(ui, "blocks_save_grid", |ui| {
                        widgets::form_row(ui, tr("blocks.name"), |ui| ui.add(egui::TextEdit::singleline(name).desired_width(220.0)));
                        widgets::form_row(ui, tr("blocks.category"), |ui| {
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(category).desired_width(160.0).hint_text(tr("blocks.general")));
                                if !cats.is_empty() {
                                    crate::widgets::menu_button(ui, "▾", |ui| {
                                        for c in &cats {
                                            if ui.button(c).clicked() {
                                                *category = c.clone();
                                                ui.close();
                                            }
                                        }
                                    });
                                }
                            })
                        });
                    });
                    ui.label(egui::RichText::new(tr("blocks.save_hint")).weak());
                });
                match res {
                    DialogResult::Ok if !name.trim().is_empty() => {
                        match blocks::save(name, category, doc) {
                            Ok(()) => self.notices.push(crate::app::Notice::info(trf("blocks.saved", &[("name", &name.trim())]))),
                            Err(e) => self.notices.push(crate::app::Notice::error(e)),
                        }
                        return;
                    }
                    DialogResult::Cancel => return,
                    _ => {}
                }
            }
        }
        self.blocks = Some(d);
    }
}
