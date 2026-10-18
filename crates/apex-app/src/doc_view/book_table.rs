//! The table of a book: a row a scene with its chapter, status, point of
//! view, tags, words and synopsis, sorted by any column. Its cells are
//! edited in place; the order of the text stays as it is.

use super::*;
use apex_doc::book::SceneStatus;

/// A column of the table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Col {
    Order,
    Scene,
    Chapter,
    Status,
    Pov,
    Tags,
    Words,
    Synopsis,
}

/// How wide a column's cells are.
fn width(c: Col) -> f32 {
    match c {
        Col::Order => 28.0,
        Col::Scene | Col::Chapter => 170.0,
        Col::Status => 100.0,
        Col::Pov => 110.0,
        Col::Tags => 140.0,
        Col::Words => 56.0,
        Col::Synopsis => 320.0,
    }
}

const COLS: [Col; 8] = [Col::Order, Col::Scene, Col::Chapter, Col::Status, Col::Pov, Col::Tags, Col::Words, Col::Synopsis];

/// A scene as the table shows it.
#[derive(Clone, Debug)]
pub(super) struct Row {
    pub order: usize,
    pub para: usize,
    pub scene: String,
    pub chapter: String,
    pub status: SceneStatus,
    pub pov: String,
    pub tags: String,
    pub words: usize,
    pub synopsis: String,
}

/// How the table is sorted and the cell being edited.
#[derive(Default)]
pub(super) struct Table {
    sort: Option<(Col, bool)>,
    /// The cell being edited: its scene, column and text as typed.
    edit: Option<(usize, Col, String)>,
    focus: bool,
}

fn col_name(c: Col) -> &'static str {
    tr(match c {
        Col::Order => "book.col_order",
        Col::Scene => "book.col_scene",
        Col::Chapter => "book.col_chapter",
        Col::Status => "book.col_status",
        Col::Pov => "book.pov",
        Col::Tags => "book.tags",
        Col::Words => "book.col_words",
        Col::Synopsis => "book.synopsis",
    })
}

/// Sorts `rows` by `col` (`desc`: the other way); ties keep the book's
/// order.
pub(super) fn sort_rows(rows: &mut [Row], col: Col, desc: bool) {
    let f = crate::palette::fold;
    rows.sort_by(|a, b| {
        let o = match col {
            Col::Order => a.order.cmp(&b.order),
            Col::Scene => f(&a.scene).cmp(&f(&b.scene)),
            Col::Chapter => a.order.cmp(&b.order),
            Col::Status => SceneStatus::ALL.iter().position(|s| *s == a.status).cmp(&SceneStatus::ALL.iter().position(|s| *s == b.status)),
            Col::Pov => f(&a.pov).cmp(&f(&b.pov)),
            Col::Tags => f(&a.tags).cmp(&f(&b.tags)),
            Col::Words => a.words.cmp(&b.words),
            Col::Synopsis => f(&a.synopsis).cmp(&f(&b.synopsis)),
        };
        let o = if desc { o.reverse() } else { o };
        o.then(a.order.cmp(&b.order))
    });
}

impl DocView {
    /// The scenes of the book, in its order.
    pub(super) fn table_rows(&self) -> Vec<Row> {
        let mut chapter = String::new();
        let mut rows = Vec::new();
        for n in self.doc.book_nodes() {
            if n.level == 1 {
                chapter = n.title.clone();
                continue;
            }
            let info = self.doc.scene_info(n.para).cloned().unwrap_or_default();
            rows.push(Row { order: rows.len() + 1, para: n.para, scene: n.title, chapter: chapter.clone(), status: info.status, pov: info.pov, tags: info.tags.join(", "), words: n.words, synopsis: info.synopsis });
        }
        rows
    }

    /// Saves what was typed in a cell.
    fn commit_cell(&mut self, para: usize, col: Col, text: &str) {
        match col {
            Col::Scene => self.rename_heading(para, text),
            Col::Pov => {
                let v = text.trim().to_string();
                if self.doc.scene_info(para).map_or(String::new(), |i| i.pov.clone()) != v {
                    self.doc.edit_scene(para, self.sel, |s| s.pov = v);
                }
            }
            Col::Tags => {
                let v = super::book_panel::parse_tags(text);
                if self.doc.scene_info(para).map_or(Vec::new(), |i| i.tags.clone()) != v {
                    self.doc.edit_scene(para, self.sel, |s| s.tags = v);
                }
            }
            Col::Synopsis => {
                let v = text.trim().to_string();
                if self.doc.scene_info(para).map_or(String::new(), |i| i.synopsis.clone()) != v {
                    self.doc.edit_scene(para, self.sel, |s| s.synopsis = v);
                }
            }
            _ => {}
        }
    }

    pub(super) fn book_table(&mut self, ui: &mut egui::Ui) {
        let editing = self.book.table.edit.is_some();
        let (undo, redo, esc) = ui.input_mut(|i| {
            if editing {
                return (false, false, false);
            }
            (i.consume_key(Modifiers::COMMAND, Key::Z), i.consume_key(Modifiers::COMMAND, Key::Y) || i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z), i.consume_key(Modifiers::NONE, Key::Escape))
        });
        if undo {
            self.undo();
        } else if redo {
            self.redo();
        }
        if esc {
            self.book.view = super::book_ui::BookView::Text;
            self.focus = true;
            return;
        }
        let mut t = std::mem::take(&mut self.book.table);
        let mut rows = self.table_rows();
        if let Some((c, d)) = t.sort {
            sort_rows(&mut rows, c, d);
        }
        let current = self.book_scene_at(self.sel.cursor.para);
        let mut commit: Option<(usize, Col, String)> = None;
        let mut status: Option<(usize, SceneStatus)> = None;
        let mut open: Option<usize> = None;
        egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(12.0).show(ui, |ui| {
            if rows.is_empty() {
                ui.label(RichText::new(tr("book.empty")).weak());
                return;
            }
            egui::ScrollArea::both().id_salt(("book_table", self.uid)).auto_shrink([false, false]).show(ui, |ui| {
                egui::Grid::new(("book_table_grid", self.uid)).striped(true).spacing(vec2(14.0, 6.0)).min_col_width(24.0).show(ui, |ui| {
                    for c in COLS {
                        let arrow = match t.sort {
                            Some((s, false)) if s == c => " ⏶",
                            Some((s, true)) if s == c => " ⏷",
                            _ => "",
                        };
                        let r = ui.add(egui::Label::new(RichText::new(format!("{}{arrow}", col_name(c))).strong()).selectable(false).sense(Sense::click())).on_hover_text(tr("book.sort_tip"));
                        if r.clicked() {
                            t.sort = match t.sort {
                                Some((s, false)) if s == c => Some((c, true)),
                                Some((s, true)) if s == c => None,
                                _ => Some((c, false)),
                            };
                        }
                    }
                    ui.end_row();
                    for row in &rows {
                        for c in COLS {
                            let text = match c {
                                Col::Order => row.order.to_string(),
                                Col::Scene => row.scene.clone(),
                                Col::Chapter => row.chapter.clone(),
                                Col::Status => String::new(),
                                Col::Pov => row.pov.clone(),
                                Col::Tags => row.tags.clone(),
                                Col::Words => row.words.to_string(),
                                Col::Synopsis => row.synopsis.clone(),
                            };
                            if c == Col::Status {
                                egui::ComboBox::from_id_salt(("book_table_status", row.para)).selected_text(super::book_ui::status_name(row.status)).width(width(Col::Status)).show_ui(ui, |ui| {
                                    for s in SceneStatus::ALL {
                                        if ui.selectable_label(row.status == s, super::book_ui::status_name(s)).clicked() && s != row.status {
                                            status = Some((row.para, s));
                                        }
                                    }
                                });
                                continue;
                            }
                            if let Some((p, ec, buf)) = &mut t.edit
                                && *p == row.para
                                && *ec == c
                            {
                                let r = ui.add(egui::TextEdit::singleline(buf).desired_width(width(c)));
                                if std::mem::take(&mut t.focus) {
                                    r.request_focus();
                                }
                                if r.lost_focus() {
                                    if !ui.input(|i| i.key_pressed(Key::Escape)) {
                                        commit = Some((row.para, c, buf.clone()));
                                    }
                                    t.edit = None;
                                }
                                continue;
                            }
                            let editable = matches!(c, Col::Scene | Col::Pov | Col::Tags | Col::Synopsis);
                            let mut rt = RichText::new(if text.is_empty() && editable { "—".to_string() } else { text.clone() });
                            if text.is_empty() || matches!(c, Col::Order | Col::Words | Col::Chapter) {
                                rt = rt.weak();
                            }
                            if c == Col::Scene && current == Some(row.para) {
                                rt = rt.strong();
                            }
                            let lbl = egui::Label::new(rt).selectable(false).sense(Sense::click()).truncate();
                            let w = width(c);
                            let r = ui.allocate_ui_with_layout(vec2(w, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.set_min_width(w);
                                ui.add(lbl)
                            })
                            .inner;
                            let r = if matches!(c, Col::Scene | Col::Chapter | Col::Tags | Col::Synopsis) && !text.is_empty() { r.on_hover_text(&text) } else { r };
                            if r.double_clicked() && c == Col::Order {
                                open = Some(row.para);
                            } else if r.clicked() && editable {
                                t.edit = Some((row.para, c, text.clone()));
                                t.focus = true;
                            } else if r.clicked() {
                                self.sel = Selection::caret(Pos::new(row.para, 0));
                            }
                            if c == Col::Order {
                                r.on_hover_text(tr("book.row_open_tip"));
                            }
                        }
                        ui.end_row();
                    }
                });
            });
        });
        self.book.table = t;
        if let Some((p, c, text)) = commit {
            self.commit_cell(p, c, &text);
        }
        if let Some((p, s)) = status {
            self.doc.edit_scene(p, self.sel, |i| i.status = s);
        }
        if let Some(p) = open {
            self.go_to_heading(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_sorts_and_edits_scenes() {
        let h = |t: &str, l: u8| {
            let mut p = Paragraph::new(t);
            p.style.kind = ParaKind::Heading(l);
            p
        };
        let doc = Document::from_paragraphs(vec![h("Uno", 1), h("Bruma", 2), Paragraph::new("a b c"), h("Alba", 2), Paragraph::new("d")]);
        let mut v = DocView::new(doc);
        v.make_book();
        let mut rows = v.table_rows();
        assert_eq!(rows.iter().map(|r| (r.order, r.scene.as_str(), r.chapter.as_str())).collect::<Vec<_>>(), vec![(1, "Bruma", "Uno"), (2, "Alba", "Uno")]);
        sort_rows(&mut rows, Col::Scene, false);
        assert_eq!(rows[0].scene, "Alba");
        sort_rows(&mut rows, Col::Words, true);
        assert_eq!(rows[0].scene, "Bruma");
        v.commit_cell(1, Col::Pov, " Marta ");
        v.commit_cell(1, Col::Tags, "#mar, noche");
        v.commit_cell(3, Col::Scene, "Amanecer");
        let rows = v.table_rows();
        assert_eq!((rows[0].pov.as_str(), rows[0].tags.as_str()), ("Marta", "mar, noche"));
        assert_eq!(rows[1].scene, "Amanecer");
        // The text keeps its order.
        assert_eq!(v.doc.paragraphs[1].text, "Bruma");
    }
}
