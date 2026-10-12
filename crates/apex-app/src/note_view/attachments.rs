//! Attachments: files kept inside the notebook, linked from a page and
//! shown in a gallery. Pictures dropped on a page go into its text.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use apex_io::notebook_store::Attachment;

use super::*;

/// Links to an attachment start with this.
pub const ATTACH: &str = "attach:";

/// The attachment a link points at.
pub fn parse_attach(target: &str) -> Option<i64> {
    target.strip_prefix(ATTACH)?.parse().ok()
}

pub(super) fn is_picture(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|e| crate::sheet_view::PICTURE_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// The gallery's state.
#[derive(Default)]
pub(super) struct Gallery {
    list: Vec<Attachment>,
    /// Asked to delete, waiting for the yes.
    confirm: Option<i64>,
    /// Pictures' contents, read once.
    thumbs: std::collections::HashMap<i64, Arc<[u8]>>,
}

impl NoteView {
    /// A file dropped on the open page: a picture into its text, anything
    /// else kept in the notebook with a link to it at the caret.
    pub fn drop_file(&mut self, path: &Path) {
        let picture = is_picture(&path.to_string_lossy());
        if picture && let Some(ed) = &mut self.editor {
            ed.insert_picture_file(path);
            return;
        }
        self.attach_file(path);
    }

    /// Keeps the file at `path` in the notebook, with a link at the caret
    /// of the page open.
    pub fn attach_file(&mut self, path: &Path) {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => return self.notices.push(Notice::error(e.to_string())),
        };
        let name = path.file_name().map_or_else(|| "file".to_string(), |n| n.to_string_lossy().into_owned());
        // From the gallery the page is out of sight: the file is the
        // notebook's, with no link written where it can't be seen.
        let shown = !self.view_open();
        let id = match self.store.add_attachment(&name, &data, self.page.filter(|_| shown), now()) {
            Ok(id) => id,
            Err(e) => return self.report(Err(e)),
        };
        if let Some(ed) = self.editor.as_mut().filter(|_| shown) {
            ed.insert_note_link(&name, format!("{ATTACH}{id}").into());
        }
        if let Some(g) = &mut self.gallery {
            g.list.clear();
        }
        self.notices.push(Notice::info(trf("nb.attached", &[("file", &name)])));
    }

    pub(super) fn pick_attachment(&self) {
        crate::file_dialog::FileDialog::new().set_title(tr("nb.attach")).pick_files(crate::file_dialog::in_view(|v: &mut NoteView, paths: Vec<PathBuf>| {
            for p in paths {
                v.attach_file(&p);
            }
        }));
    }

    /// A copy of attachment `id` in the temporary folder, for the app to
    /// open.
    pub(super) fn open_attachment(&mut self, id: i64) {
        let Some(a) = self.store.attachments().ok().and_then(|l| l.into_iter().find(|a| a.id == id)) else {
            return self.notices.push(Notice::info(tr("nb.attach_gone")));
        };
        let data = match self.store.attachment_data(id) {
            Ok(d) => d,
            Err(e) => return self.report(Err(e)),
        };
        // What ApexDesk doesn't open goes to the system's program: never a
        // program (by its name, or its contents when the name says little).
        let name = apex_io::safe_path::safe_file_name(&a.name, "attachment.bin");
        if apex_io::Format::from_path(Path::new(&name)).is_none() && crate::links::copy_runs(&name, &data) {
            return self.notices.push(Notice::error(trf("links.object_blocked", &[("name", &a.name)])));
        }
        // The name is the notebook's to choose: keep it inside our own folder.
        let written = apex_io::safe_path::private_temp_dir("apexdesk-attachments", id).and_then(|dir| {
            let path = apex_io::safe_path::join_safe(&dir, &a.name, "attachment.bin");
            crate::common::write_out(&path, data)?;
            apex_io::safe_path::mark_untrusted(&path);
            Ok(path)
        });
        match written {
            Ok(path) => self.open_file = Some(path),
            Err(e) => self.notices.push(Notice::error(e.to_string())),
        }
    }

    fn save_attachment(&mut self, id: i64, path: &Path) {
        match self.store.attachment_data(id) {
            Ok(d) => {
                match crate::common::write_out(path, d) {
                    Ok(()) => apex_io::safe_path::mark_untrusted(path),
                    Err(e) => self.notices.push(Notice::error(e.to_string())),
                }
            }
            Err(e) => self.report(Err(e)),
        }
    }

    pub fn open_gallery(&mut self) {
        self.flush();
        self.close_graph();
        self.gallery = Some(Gallery::default());
    }

    pub(super) fn gallery_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut g) = self.gallery.take() else { return };
        if g.list.is_empty() {
            g.list = self.store.attachments().unwrap_or_default();
        }
        let mut close = false;
        let mut open = None;
        let mut go = None;
        let mut save = None;
        let mut delete = None;
        egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 12)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(tr("nb.gallery"));
                ui.add_space(16.0);
                if ui.button(format!("{}…", tr("nb.attach"))).clicked() {
                    self.pick_attachment();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("nb.graph_close")).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(6.0);
            if g.list.is_empty() {
                ui.label(RichText::new(tr("nb.gallery_empty")).weak());
                return;
            }
            egui::ScrollArea::vertical().id_salt(("note_gallery", self.uid)).auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
                    for a in &g.list {
                        let tile = egui::Frame::new().fill(ui.visuals().faint_bg_color).stroke(ui.visuals().widgets.noninteractive.bg_stroke).corner_radius(6.0).inner_margin(8.0);
                        tile.show(ui, |ui| {
                            ui.set_width(170.0);
                            ui.vertical(|ui| {
                                let (rect, r) = ui.allocate_exact_size(vec2(170.0, 110.0), Sense::click());
                                let mut shown = false;
                                if is_picture(&a.name) {
                                    if !g.thumbs.contains_key(&a.id)
                                        && let Ok(d) = self.store.attachment_data(a.id)
                                    {
                                        g.thumbs.insert(a.id, d.into());
                                    }
                                    if let Some(bytes) = g.thumbs.get(&a.id)
                                        && let Some(tex) = crate::images::texture(ui.ctx(), crate::images::key(bytes), bytes, false)
                                    {
                                        let size = tex.size_vec2();
                                        let s = (rect.width() / size.x).min(rect.height() / size.y).min(1.0);
                                        ui.painter().image(tex.id(), egui::Rect::from_center_size(rect.center(), size * s), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                                        shown = true;
                                    }
                                }
                                if !shown {
                                    let icon = if a.name.to_lowercase().ends_with(".pdf") { crate::icons::Icon::Pdf } else { crate::icons::Icon::Doc };
                                    crate::icons::paint(ui.painter(), egui::Rect::from_center_size(rect.center(), vec2(48.0, 48.0)), icon, ui.visuals().weak_text_color());
                                }
                                if r.on_hover_text(tr("nb.attach_open")).clicked() {
                                    open = Some(a.id);
                                }
                                ui.add(egui::Label::new(RichText::new(&a.name).strong()).truncate());
                                ui.label(RichText::new(format!("{} · {}", crate::db_view::format_size(a.size), date_text(a.added))).small().weak());
                                if let Some(p) = a.page.and_then(|p| self.nb.page(p)).filter(|p| self.nb.is_live(p.id))
                                    && ui.link(RichText::new(&p.title).small()).clicked()
                                {
                                    go = Some(p.id);
                                }
                                ui.horizontal(|ui| {
                                    if g.confirm == Some(a.id) {
                                        ui.label(RichText::new(tr("nb.attach_delete_ask")).small());
                                        if ui.small_button(tr("nb.attach_delete")).clicked() {
                                            delete = Some(a.id);
                                        }
                                        if ui.small_button(tr("dialog.cancel")).clicked() {
                                            g.confirm = None;
                                        }
                                    } else {
                                        if ui.small_button(tr("nb.attach_open")).clicked() {
                                            open = Some(a.id);
                                        }
                                        if ui.small_button(format!("{}…", tr("nb.attach_save"))).clicked() {
                                            save = Some((a.id, apex_io::safe_path::safe_file_name(&a.name, "attachment.bin")));
                                        }
                                        if ui.small_button(tr("nb.attach_delete")).clicked() {
                                            g.confirm = Some(a.id);
                                        }
                                    }
                                });
                            });
                        });
                    }
                });
            });
        });
        if let Some(id) = delete {
            let r = self.store.delete_attachment(id);
            self.report(r);
            g.list.clear();
            g.confirm = None;
            g.thumbs.remove(&id);
        }
        if !close {
            self.gallery = Some(g);
        }
        if let Some(id) = open {
            self.open_attachment(id);
        }
        if let Some((id, name)) = save {
            crate::file_dialog::FileDialog::new().set_title(tr("nb.attach_save")).set_file_name(name).save_file(crate::file_dialog::in_view(move |v: &mut NoteView, path: PathBuf| v.save_attachment(id, &path)));
        }
        if let Some(p) = go {
            self.gallery = None;
            self.open_page(p);
            self.grid = false;
        }
    }
}
