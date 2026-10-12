//! Insert > Library: the organization's pictures (a setting or policy),
//! a shared folder whose subfolders are categories.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui;

use crate::i18n::tr;

/// Picture extensions the views insert.
const EXTS: &[&str] = &["png", "jpg", "jpeg", "jpe", "gif", "bmp", "webp"];
/// Thumbnails are shrunk to fit this many pixels a side.
const THUMB: u32 = 96;

static DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// The library folder (None: none).
pub fn set_dir(d: Option<PathBuf>) {
    *DIR.lock().unwrap_or_else(|e| e.into_inner()) = d;
}

pub fn dir() -> Option<PathBuf> {
    DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether Insert > Library is offered.
pub fn available() -> bool {
    dir().is_some()
}

/// A category: its name ("" the folder itself) and pictures, by name.
#[derive(Debug)]
pub struct Category {
    pub name: String,
    pub pictures: Vec<PathBuf>,
}

fn pictures_in(d: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(d) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str())))
        .collect();
    v.sort_by_key(|p| stem(p).to_lowercase());
    v
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The folder's pictures, then each subfolder's (one level deep, by name);
/// categories without pictures are left out.
pub fn scan(d: &Path) -> Vec<Category> {
    let mut v = vec![Category { name: String::new(), pictures: pictures_in(d) }];
    let mut subs: Vec<PathBuf> = std::fs::read_dir(d).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect()).unwrap_or_default();
    subs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    for s in subs {
        let name = s.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        v.push(Category { name, pictures: pictures_in(&s) });
    }
    v.retain(|c| !c.pictures.is_empty());
    v
}

/// Whether picture `p` matches the filter `f` (lowercase).
fn matches(p: &Path, f: &str) -> bool {
    f.is_empty() || stem(p).to_lowercase().contains(f)
}

fn thumbnail(ctx: &egui::Context, p: &Path) -> Option<egui::TextureHandle> {
    let bytes = std::fs::read(p).ok()?;
    let d = crate::images::decode(&bytes)?;
    let k = (THUMB as f32 / d.width.max(d.height) as f32).min(1.0);
    let (w, h) = (((d.width as f32 * k).round() as u32).max(1), ((d.height as f32 * k).round() as u32).max(1));
    // Nearest sampling is enough for a thumbnail.
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let sy = ((y as f32 + 0.5) / k) as u32;
        for x in 0..w {
            let sx = ((x as f32 + 0.5) / k) as u32;
            let i = (sy.min(d.height - 1) as usize * d.width as usize + sx.min(d.width - 1) as usize) * 4;
            rgba.extend_from_slice(&d.rgba[i..i + 4]);
        }
    }
    let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
    Some(ctx.load_texture(format!("library:{}", p.display()), img, egui::TextureOptions::LINEAR))
}

/// The Insert > Library dialog.
#[derive(Default)]
pub struct Dialog {
    open: bool,
    categories: Vec<Category>,
    category: usize,
    filter: String,
    /// Thumbnails decoded so far (None: not a readable picture).
    thumbs: HashMap<PathBuf, Option<egui::TextureHandle>>,
}

impl Dialog {
    /// Opens the dialog, reading the folder.
    pub fn show(&mut self) {
        let categories = dir().map(|d| scan(&d)).unwrap_or_default();
        *self = Dialog { open: true, categories, ..Default::default() };
    }

    /// Shows the dialog; returns the picture clicked (and closes).
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        if !self.open {
            return None;
        }
        let mut picked = None;
        // A few thumbnails a frame, so a large folder doesn't stall.
        let mut budget = 6;
        let closed = crate::widgets::info_dialog(ctx, "library", tr("library.title"), 620.0, |ui| {
            if self.categories.is_empty() {
                ui.label(egui::RichText::new(tr("library.empty")).weak());
                return;
            }
            ui.horizontal(|ui| {
                if self.categories.len() > 1 {
                    let name = |c: &Category| if c.name.is_empty() { tr("library.general").to_string() } else { c.name.clone() };
                    egui::ComboBox::from_id_salt("library-category").selected_text(name(&self.categories[self.category])).show_ui(ui, |ui| {
                        for (i, c) in self.categories.iter().enumerate() {
                            ui.selectable_value(&mut self.category, i, name(c));
                        }
                    });
                }
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(tr("library.filter")).desired_width(200.0));
            });
            ui.add_space(6.0);
            let f = self.filter.trim().to_lowercase();
            let Some(cat) = self.categories.get(self.category.min(self.categories.len() - 1)) else { return };
            egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                    for p in cat.pictures.iter().filter(|p| matches(p, &f)) {
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(112.0, 130.0), egui::Sense::click());
                        let name = stem(p);
                        crate::widgets::name(&resp, egui::WidgetType::Button, &name, None);
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let v = ui.visuals();
                        let bg = if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { v.faint_bg_color };
                        ui.painter().rect_filled(rect, 6.0, bg);
                        let area = egui::Rect::from_min_size(rect.min + egui::vec2(8.0, 8.0), egui::vec2(96.0, 96.0));
                        if !self.thumbs.contains_key(p) && budget > 0 {
                            budget -= 1;
                            self.thumbs.insert(p.clone(), thumbnail(ui.ctx(), p));
                        }
                        match self.thumbs.get(p) {
                            Some(Some(t)) => {
                                let s = t.size_vec2();
                                let k = (area.width() / s.x).min(area.height() / s.y).min(1.0);
                                let r = egui::Rect::from_center_size(area.center(), s * k);
                                ui.painter().image(t.id(), r, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                            }
                            Some(None) => {}
                            None => ui.ctx().request_repaint(),
                        }
                        let color = v.text_color();
                        let label = ui.painter().layout(name, egui::FontId::proportional(12.0), color, rect.width() - 8.0);
                        let label_rect = egui::Rect::from_min_size(egui::pos2(rect.center().x - label.size().x.min(rect.width() - 8.0) / 2.0, area.max.y + 4.0), label.size());
                        ui.painter().with_clip_rect(rect).galley(label_rect.min, label, color);
                        let resp = resp.on_hover_text(p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                        if resp.clicked() {
                            picked = Some(p.clone());
                        }
                    }
                });
            });
        });
        if closed || picked.is_some() {
            *self = Dialog::default();
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_folder_and_categories() {
        let root = std::env::temp_dir().join(format!("apex-library-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Logos")).unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::create_dir_all(root.join("Icons")).unwrap();
        for f in ["b.PNG", "a.jpg", "notes.txt", "Logos/logo.gif", "Icons/x.bmp", "Icons/y.webp", "empty/readme.md"] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        let c = scan(&root);
        let names: Vec<&str> = c.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["", "Icons", "Logos"]);
        let files: Vec<String> = c[0].pictures.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(files, ["a.jpg", "b.PNG"]);
        assert_eq!(c[1].pictures.len(), 2);
        assert!(matches(&c[0].pictures[1], "b"));
        assert!(!matches(&c[0].pictures[1], "zz"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
