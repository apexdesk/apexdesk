//! The file browser that answers the file dialogs on Android: the shared
//! storage's folders, drawn inside the window.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::Sender;

use eframe::egui;

use crate::file_dialog::Kind;

/// What the code asked for (see `file_dialog::FileDialog`).
pub struct Request {
    pub kind: Kind,
    pub title: String,
    pub file_name: String,
    pub exts: Vec<String>,
    pub dir: Option<PathBuf>,
}

struct Browser {
    req: Request,
    tx: Sender<Option<Vec<PathBuf>>>,
    dir: PathBuf,
    name: String,
    picked: Vec<PathBuf>,
}

static OPEN: Mutex<Option<Browser>> = Mutex::new(None);
/// The folder of the last answer, where the next browser starts (kept in
/// a file for the next time the program starts).
static LAST: Mutex<Option<PathBuf>> = Mutex::new(None);

fn last_file() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("last-folder"))
}

/// The shared storage ("Internal storage" in the system's file manager).
const STORAGE: &str = "/storage/emulated/0";

pub fn ask(req: Request, tx: Sender<Option<Vec<PathBuf>>>) {
    let last = LAST.lock().unwrap_or_else(|e| e.into_inner()).clone().or_else(|| last_file().and_then(|f| std::fs::read_to_string(f).ok()).map(PathBuf::from)).filter(|d| d.is_dir());
    let dir = req.dir.clone().filter(|d| d.is_dir()).or(last).unwrap_or_else(|| PathBuf::from(STORAGE));
    let name = req.file_name.clone();
    let old = OPEN.lock().unwrap_or_else(|e| e.into_inner()).replace(Browser { req, tx, dir, name, picked: Vec::new() });
    if let Some(old) = old {
        let _ = old.tx.send(None);
    }
}

fn spanish() -> bool {
    apex_i18n::current() == "es"
}

fn t(es: &'static str, en: &'static str) -> &'static str {
    if spanish() { es } else { en }
}

/// Folders first, then the files the request takes, by name.
fn entries(dir: &Path, exts: &[String], folders_only: bool) -> std::io::Result<Vec<(String, bool)>> {
    let mut v: Vec<(String, bool)> = std::fs::read_dir(dir)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Hidden files, and the lock files of open documents ("~$a.docx").
            if name.starts_with('.') || name.starts_with("~$") {
                return None;
            }
            let is_dir = e.file_type().ok()?.is_dir() || e.path().is_dir();
            let ext = Path::new(&name).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            let takes = is_dir || (!folders_only && (exts.is_empty() || exts.iter().any(|x| x.eq_ignore_ascii_case(&ext) || x == "*")));
            takes.then_some((name, is_dir))
        })
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    Ok(v)
}

pub fn show(ctx: &egui::Context) {
    let mut guard = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let Some(b) = guard.as_mut() else { return };
    let mut answer: Option<Option<Vec<PathBuf>>> = None;
    let title = if b.req.title.is_empty() {
        match b.req.kind {
            Kind::Save => t("Guardar", "Save"),
            Kind::Folder => t("Elegir carpeta", "Choose folder"),
            _ => t("Abrir", "Open"),
        }
        .to_string()
    } else {
        b.req.title.clone()
    };
    let screen = ctx.content_rect();
    let shown = b.dir.clone();
    egui::Modal::new(egui::Id::new("android-files")).show(ctx, |ui| {
        ui.set_width((screen.width() * 0.8).min(900.0));
        ui.heading(&title);
        ui.horizontal(|ui| {
            if ui.add_enabled(b.dir.parent().is_some(), egui::Button::new("⬆")).clicked()
                && let Some(p) = b.dir.parent()
            {
                b.dir = p.to_path_buf();
            }
            if ui.button(t("Almacenamiento", "Storage")).clicked() {
                b.dir = PathBuf::from(STORAGE);
            }
            if ui.button(t("Descargas", "Downloads")).clicked() {
                b.dir = PathBuf::from(STORAGE).join("Download");
            }
            if ui.button(t("Documentos", "Documents")).clicked() {
                b.dir = PathBuf::from(STORAGE).join("Documents");
            }
        });
        ui.label(egui::RichText::new(b.dir.display().to_string()).weak());
        // Above the list, where the keyboard leaves it in sight.
        if b.req.kind == Kind::Save {
            ui.horizontal(|ui| {
                ui.label(t("Nombre:", "Name:"));
                ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(f32::INFINITY));
            });
        }
        ui.separator();
        let list = entries(&b.dir, &b.req.exts, b.req.kind == Kind::Folder);
        let row_h = 44.0;
        let line_h = 56.0;
        // One per folder: each starts at its top.
        let scroll = egui::ScrollArea::vertical().id_salt(&b.dir).max_height((screen.height() * 0.6).max(120.0)).auto_shrink([false, false]);
        match &list {
            Ok(list) if list.is_empty() => {
                scroll.show(ui, |ui| {
                    ui.label(t("(vacía)", "(empty)"));
                    no_access_note(ui);
                });
            }
            // Only the rows in sight are drawn (and their pictures decoded).
            Ok(list) => {
                scroll.show_rows(ui, line_h, list.len(), |ui, rows| {
                    for (name, is_dir) in &list[rows] {
                        let path = b.dir.join(name);
                        let selected = b.picked.contains(&path);
                        let r = row(ui, &path, name, *is_dir, selected, line_h);
                        if r.clicked() {
                            if *is_dir {
                                b.dir = path;
                                b.picked.clear();
                            } else {
                                match b.req.kind {
                                    Kind::Pick => answer = Some(Some(vec![path])),
                                    Kind::PickMany if r.double_clicked() => answer = Some(Some(vec![path])),
                                    Kind::PickMany => {
                                        if selected {
                                            b.picked.retain(|p| p != &path);
                                        } else {
                                            b.picked.push(path);
                                        }
                                    }
                                    Kind::Save => b.name = name.clone(),
                                    Kind::Folder => {}
                                }
                            }
                        }
                    }
                });
            }
            Err(e) => {
                scroll.show(ui, |ui| {
                    ui.label(format!("{e}"));
                    no_access_note(ui);
                });
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.add_sized([140.0, row_h], egui::Button::new(t("Cancelar", "Cancel"))).clicked() {
                answer = Some(None);
            }
            let (ok_label, ok_enabled) = match b.req.kind {
                Kind::Save => (t("Guardar", "Save"), !b.name.trim().is_empty()),
                Kind::Folder => (t("Elegir esta carpeta", "Choose this folder"), true),
                Kind::PickMany => (t("Abrir", "Open"), !b.picked.is_empty()),
                Kind::Pick => ("", false),
            };
            if !ok_label.is_empty() && ui.add_enabled(ok_enabled, egui::Button::new(ok_label).min_size(egui::vec2(140.0, row_h))).clicked() {
                answer = Some(Some(match b.req.kind {
                    Kind::Save => vec![b.dir.join(b.name.trim())],
                    Kind::Folder => vec![b.dir.clone()],
                    _ => b.picked.clone(),
                }));
            }
        });
    });
    if b.dir != shown {
        super::thumbs::forget_queue();
    }
    if let Some(a) = answer {
        let b = guard.take().expect("open");
        *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(b.dir.clone());
        if let Some(f) = last_file() {
            let _ = std::fs::write(f, b.dir.as_os_str().as_encoded_bytes());
        }
        let _ = b.tx.send(a);
        ctx.request_repaint();
    }
}

/// A row of the list: the file's thumbnail or its format's icon, and its
/// name.
fn row(ui: &mut egui::Ui, path: &Path, name: &str, is_dir: bool, selected: bool, h: f32) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, name));
    if !ui.is_rect_visible(rect) {
        return r;
    }
    let v = ui.style().interact_selectable(&r, selected);
    let painter = ui.painter_at(rect);
    if selected || r.hovered() || r.has_focus() {
        painter.rect(rect, v.corner_radius, v.weak_bg_fill, v.bg_stroke, egui::StrokeKind::Inside);
    }
    let side = h - 12.0;
    let icon = egui::Rect::from_min_size(egui::pos2(rect.left() + 8.0, rect.center().y - side / 2.0), egui::vec2(side, side));
    let ext = Path::new(name).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
    let format = if is_dir { Format::Folder } else { Format::of(&ext) };
    match (format == Format::Image).then(|| super::thumbs::get(ui.ctx(), path)).flatten() {
        Some(Some((tex, size))) => {
            let s = side / size.x.max(size.y).max(1.0);
            let pic = egui::Rect::from_center_size(icon.center(), size * s);
            painter.image(tex, pic, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            painter.rect_stroke(pic, 0.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Outside);
        }
        // Being made, or none: the format's icon.
        _ => paint_icon(&painter, icon, format, &ext),
    }
    let x = icon.right() + 14.0;
    let galley = egui::WidgetText::from(egui::RichText::new(name).size(18.0)).into_galley(ui, Some(egui::TextWrapMode::Truncate), (rect.right() - 8.0 - x).max(0.0), egui::TextStyle::Button);
    painter.galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, v.text_color());
    r
}

/// The kinds of file the list tells apart by their icons.
#[derive(Clone, Copy, PartialEq)]
enum Format {
    Folder,
    Sheet,
    Csv,
    Text,
    Slides,
    Pdf,
    Database,
    Image,
    Other,
}

impl Format {
    fn of(ext: &str) -> Format {
        match ext {
            "xlsx" | "xlsm" | "xlsb" | "xls" | "xltx" | "xltm" | "xlt" | "ods" | "ots" | "fods" | "numbers" => Format::Sheet,
            "csv" | "tsv" | "tab" | "prn" => Format::Csv,
            "docx" | "docm" | "dotx" | "dotm" | "doc" | "dot" | "odt" | "ott" | "fodt" | "rtf" | "txt" | "md" | "wpd" | "wps" | "pages" | "html" | "htm" | "epub" | "apexnote" => Format::Text,
            "pptx" | "pptm" | "ppsx" | "potx" | "ppt" | "pps" | "pot" | "odp" | "otp" | "fodp" | "key" => Format::Slides,
            "pdf" => Format::Pdf,
            "accdb" | "mdb" | "odb" | "sqlite" | "sqlite3" | "db" | "dbf" => Format::Database,
            e if super::thumbs::EXTS.contains(&e) || matches!(e, "tif" | "tiff" | "svg" | "emf" | "wmf" | "ico") => Format::Image,
            _ => Format::Other,
        }
    }

    fn color(self) -> egui::Color32 {
        use egui::Color32 as C;
        match self {
            Format::Folder => C::from_rgb(0xE8, 0xB0, 0x30),
            Format::Sheet => C::from_rgb(0x21, 0x8A, 0x4C),
            Format::Csv => C::from_rgb(0x15, 0x8A, 0x8A),
            Format::Text => C::from_rgb(0x2B, 0x5C, 0xB8),
            Format::Slides => C::from_rgb(0xD2, 0x5A, 0x2A),
            Format::Pdf => C::from_rgb(0xC6, 0x28, 0x28),
            Format::Database => C::from_rgb(0x6A, 0x3D, 0x9A),
            Format::Image => C::from_rgb(0x7A, 0x8B, 0x3A),
            Format::Other => C::from_rgb(0x80, 0x86, 0x8C),
        }
    }
}

/// A folder, or a page in the format's colour with a mark of its kind and
/// the file's extension.
fn paint_icon(painter: &egui::Painter, r: egui::Rect, format: Format, ext: &str) {
    use egui::{Color32, Shape, Stroke, pos2, vec2};
    let s = r.width();
    let color = format.color();
    let light = Color32::from_white_alpha(200);
    if format == Format::Folder {
        let back = egui::Rect::from_min_max(pos2(r.left() + 0.04 * s, r.top() + 0.18 * s), pos2(r.right() - 0.04 * s, r.bottom() - 0.12 * s));
        let tab = egui::Rect::from_min_size(back.left_top() - vec2(0.0, 0.08 * s), vec2(0.38 * s, 0.14 * s));
        let dark = color.gamma_multiply(0.75).to_opaque();
        painter.rect_filled(tab, 0.05 * s, dark);
        painter.rect_filled(back, 0.06 * s, dark);
        painter.rect_filled(egui::Rect::from_min_max(back.left_top() + vec2(0.0, 0.12 * s), back.right_bottom()), 0.06 * s, color);
        return;
    }
    let (w, fold) = (0.76 * s, 0.22 * s);
    let (l, t, b) = (r.center().x - w / 2.0, r.top() + 0.02 * s, r.bottom() - 0.02 * s);
    let rt = l + w;
    painter.add(Shape::convex_polygon(vec![pos2(l, t), pos2(rt - fold, t), pos2(rt, t + fold), pos2(rt, b), pos2(l, b)], color, Stroke::NONE));
    painter.add(Shape::convex_polygon(vec![pos2(rt - fold, t), pos2(rt - fold, t + fold), pos2(rt, t + fold)], Color32::from_white_alpha(120), Stroke::NONE));
    // The kind's mark, above the extension.
    let mark = egui::Rect::from_min_max(pos2(l + 0.12 * s, t + 0.24 * s), pos2(rt - 0.12 * s, t + 0.56 * s));
    let thin = Stroke::new((0.035 * s).max(1.0), light);
    match format {
        Format::Sheet | Format::Csv => {
            painter.rect_stroke(mark, 0.0, thin, egui::StrokeKind::Middle);
            for i in 1..3 {
                let y = mark.top() + mark.height() * i as f32 / 3.0;
                painter.line_segment([pos2(mark.left(), y), pos2(mark.right(), y)], thin);
            }
            let cols = if format == Format::Sheet { 3 } else { 2 };
            for i in 1..cols {
                let x = mark.left() + mark.width() * i as f32 / cols as f32;
                painter.line_segment([pos2(x, mark.top()), pos2(x, mark.bottom())], thin);
            }
        }
        Format::Text | Format::Other => {
            for i in 0..4 {
                let y = mark.top() + mark.height() * i as f32 / 3.0;
                let end = if i == 3 { mark.left() + mark.width() * 0.6 } else { mark.right() };
                painter.line_segment([pos2(mark.left(), y), pos2(end, y)], thin);
            }
        }
        Format::Slides => {
            painter.rect_stroke(mark, 0.03 * s, thin, egui::StrokeKind::Middle);
            painter.add(Shape::convex_polygon(
                vec![pos2(mark.center().x - 0.06 * s, mark.center().y - 0.08 * s), pos2(mark.center().x + 0.09 * s, mark.center().y), pos2(mark.center().x - 0.06 * s, mark.center().y + 0.08 * s)],
                light,
                Stroke::NONE,
            ));
        }
        Format::Pdf => {
            painter.text(mark.center(), egui::Align2::CENTER_CENTER, "A", egui::FontId::proportional(0.4 * s), light);
        }
        Format::Database => {
            let (cx, rx, ry) = (mark.center().x, mark.width() / 2.0, 0.05 * s);
            painter.rect_filled(egui::Rect::from_min_max(pos2(cx - rx, mark.top() + ry), pos2(cx + rx, mark.bottom() - ry)), 0.0, light);
            painter.add(Shape::ellipse_filled(pos2(cx, mark.bottom() - ry), vec2(rx, ry), light));
            painter.add(Shape::ellipse_filled(pos2(cx, mark.top() + ry), vec2(rx, ry), color.lerp_to_gamma(Color32::WHITE, 0.4)));
        }
        Format::Image => {
            painter.add(Shape::convex_polygon(vec![pos2(mark.left(), mark.bottom()), pos2(mark.left() + mark.width() * 0.4, mark.top() + mark.height() * 0.3), pos2(mark.left() + mark.width() * 0.8, mark.bottom())], light, Stroke::NONE));
            painter.circle_filled(pos2(mark.right() - 0.07 * s, mark.top() + 0.07 * s), 0.06 * s, light);
        }
        Format::Folder => {}
    }
    let label: String = ext.chars().take(4).collect::<String>().to_uppercase();
    painter.text(pos2(r.center().x, b - 0.17 * s), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(0.2 * s), Color32::WHITE);
}

/// Without "All files access" the shared storage looks empty.
fn no_access_note(ui: &mut egui::Ui) {
    if super::all_files_access() {
        return;
    }
    ui.add_space(8.0);
    ui.label(t(
        "ApexDesk necesita permiso para ver tus archivos.",
        "ApexDesk needs permission to see your files.",
    ));
    if ui.button(t("Dar acceso a los archivos", "Allow access to files")).clicked() {
        super::ask_all_files_access();
    }
}
