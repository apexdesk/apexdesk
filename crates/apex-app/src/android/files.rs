//! The file browser that answers the file dialogs on Android: the shared
//! storage's folders, drawn inside the window. The browser version uses it
//! too (`web::files`), on its own files: those uploaded from the computer
//! or, with `apexdesk serve`, the server's.

use apex_fs::PathExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use web_time::{Duration, Instant};

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
    /// The list shown: of which folder and when it was read. Reading a
    /// folder of the shared storage takes milliseconds (tens with a few
    /// hundred files), too long to do every frame while scrolling.
    listing: Option<(PathBuf, Instant, Result<Vec<(String, bool)>, String>)>,
    /// Saving over this existing file was asked once: the next press replaces it.
    replace: Option<PathBuf>,
}

/// How long a folder's list is shown before it is read again (files the
/// user adds meanwhile show then).
const RELIST: Duration = Duration::from_secs(3);

static OPEN: Mutex<Option<Browser>> = Mutex::new(None);
/// The folder of the last answer, where the next browser starts (kept in
/// a file for the next time the program starts).
static LAST: Mutex<Option<PathBuf>> = Mutex::new(None);

fn last_file() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("last-folder"))
}

/// The shared storage ("Internal storage" in the system's file manager).
#[cfg(target_os = "android")]
const STORAGE: &str = "/storage/emulated/0";
/// The browser version's files.
#[cfg(target_arch = "wasm32")]
const STORAGE: &str = crate::web::FILES;

pub fn ask(req: Request, tx: Sender<Option<Vec<PathBuf>>>) {
    let last = LAST.lock().unwrap_or_else(|e| e.into_inner()).clone().or_else(|| last_file().and_then(|f| apex_fs::read_to_string(f).ok()).map(PathBuf::from)).filter(|d| d.fs_is_dir());
    let dir = req.dir.clone().filter(|d| d.fs_is_dir()).or(last).unwrap_or_else(|| PathBuf::from(STORAGE));
    let name = req.file_name.clone();
    let old = OPEN.lock().unwrap_or_else(|e| e.into_inner()).replace(Browser { req, tx, dir, name, picked: Vec::new(), listing: None, replace: None });
    if let Some(old) = old {
        let _ = old.tx.send(None);
    }
}

/// Files uploaded while the browser is open: opening, they are the answer;
/// otherwise they show in the list.
#[cfg(target_arch = "wasm32")]
pub fn uploaded(ctx: &egui::Context, paths: Vec<PathBuf>) {
    let mut guard = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let Some(b) = guard.as_mut() else { return };
    b.listing = None;
    if !paths.is_empty() && matches!(b.req.kind, Kind::Pick | Kind::PickMany) {
        let b = guard.take().expect("open");
        let _ = b.tx.send(Some(if b.req.kind == Kind::Pick { paths[..1].to_vec() } else { paths }));
    }
    ctx.request_repaint();
}

fn spanish() -> bool {
    apex_i18n::current() == "es"
}

fn t(es: &'static str, en: &'static str) -> &'static str {
    if spanish() { es } else { en }
}

/// Folders first, then the files the request takes, by name.
fn entries(dir: &Path, exts: &[String], folders_only: bool) -> std::io::Result<Vec<(String, bool)>> {
    let mut v: Vec<(String, bool)> = apex_fs::read_dir(dir)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Hidden files, and the lock files of open documents ("~$a.docx").
            if name.starts_with('.') || name.starts_with("~$") {
                return None;
            }
            let is_dir = e.file_type().ok()?.is_dir() || e.path().fs_is_dir();
            let ext = Path::new(&name).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            let takes = is_dir || (!folders_only && (exts.is_empty() || exts.iter().any(|x| x.eq_ignore_ascii_case(&ext) || x == "*")));
            takes.then_some((name, is_dir))
        })
        .collect();
    v.sort_by_cached_key(|(name, is_dir)| (!is_dir, name.to_lowercase()));
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
            #[cfg(target_os = "android")]
            {
                if ui.button(t("Almacenamiento", "Storage")).clicked() {
                    b.dir = PathBuf::from(STORAGE);
                }
                if ui.button(t("Descargas", "Downloads")).clicked() {
                    b.dir = PathBuf::from(STORAGE).join("Download");
                }
                if ui.button(t("Documentos", "Documents")).clicked() {
                    b.dir = PathBuf::from(STORAGE).join("Documents");
                }
            }
            // Files of the computer come in by upload; saved ones go back
            // as downloads (or to the server).
            #[cfg(target_arch = "wasm32")]
            {
                if ui.button(t("Mis archivos", "My files")).clicked() {
                    b.dir = PathBuf::from(STORAGE);
                }
                if b.req.kind != Kind::Folder && ui.button(format!("⬆ {}", t("Subir desde el equipo…", "Upload from this computer…"))).clicked() {
                    crate::web::upload(&b.dir, &b.req.exts, b.req.kind == Kind::PickMany);
                }
            }
        });
        ui.label(egui::RichText::new(b.dir.display().to_string()).weak());
        // Without access Android lists the folders but none of the files.
        #[cfg(target_os = "android")]
        no_access_note(ui);
        #[cfg(target_arch = "wasm32")]
        crate::web::files_note(ui, b.req.kind == Kind::Save);
        // Above the list, where the keyboard leaves it in sight.
        if b.req.kind == Kind::Save {
            ui.horizontal(|ui| {
                ui.label(t("Nombre:", "Name:"));
                ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(f32::INFINITY));
            });
            if b.replace.as_ref().is_some_and(|p| *p == b.dir.join(b.name.trim())) {
                ui.colored_label(ui.visuals().warn_fg_color, t("Ya existe un archivo con ese nombre: «Reemplazar» lo sustituye.", "A file with that name exists: Replace overwrites it."));
            }
        }
        ui.separator();
        if !b.listing.as_ref().is_some_and(|(d, at, _)| *d == b.dir && at.elapsed() < RELIST) {
            let list = entries(&b.dir, &b.req.exts, b.req.kind == Kind::Folder).map_err(|e| e.to_string());
            b.listing = Some((b.dir.clone(), Instant::now(), list));
            // Read again when it is due, even if nothing moves.
            ui.ctx().request_repaint_after(RELIST);
        }
        // Taken out while the rows change the browser, put back after.
        let listing = b.listing.take().expect("just listed");
        let list = &listing.2;
        let row_h = 44.0;
        let line_h = 56.0;
        // One per folder: each starts at its top.
        let scroll = egui::ScrollArea::vertical().id_salt(&b.dir).max_height((screen.height() * 0.6).max(120.0)).auto_shrink([false, false]);
        match list {
            Ok(list) if list.is_empty() => {
                scroll.show(ui, |ui| {
                    ui.label(t("(vacía)", "(empty)"));
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
                });
            }
        }
        b.listing = Some(listing);
        ui.separator();
        ui.horizontal(|ui| {
            if ui.add_sized([140.0, row_h], egui::Button::new(t("Cancelar", "Cancel"))).clicked() {
                answer = Some(None);
            }
            let target = b.dir.join(b.name.trim());
            let replacing = b.req.kind == Kind::Save && b.replace.as_ref() == Some(&target);
            let (ok_label, ok_enabled) = match b.req.kind {
                // A name, not a path: the file goes in the folder shown.
                Kind::Save if replacing => (t("Reemplazar", "Replace"), true),
                Kind::Save => (t("Guardar", "Save"), !b.name.trim().is_empty() && !b.name.contains('/') && !matches!(b.name.trim(), "." | "..")),
                Kind::Folder => (t("Elegir esta carpeta", "Choose this folder"), true),
                Kind::PickMany => (t("Abrir", "Open"), !b.picked.is_empty()),
                Kind::Pick => ("", false),
            };
            if !ok_label.is_empty() && ui.add_enabled(ok_enabled, egui::Button::new(ok_label).min_size(egui::vec2(140.0, row_h))).clicked() {
                // As the system's dialogs do, an existing file is replaced
                // only when asked twice.
                if b.req.kind == Kind::Save && !replacing && target.fs_exists() {
                    b.replace = Some(target.clone());
                    return;
                }
                answer = Some(Some(match b.req.kind {
                    Kind::Save => vec![target.clone()],
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
            let _ = apex_fs::write(f, b.dir.as_os_str().as_encoded_bytes());
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

/// Without access to the files a bar above the list asks for it.
#[cfg(target_os = "android")]
fn no_access_note(ui: &mut egui::Ui) {
    // Asked of the system once a second, not every frame (a call into
    // Java); the user may come back from the settings with it.
    static ACCESS: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut access = ACCESS.lock().unwrap_or_else(|e| e.into_inner());
    let ok = match *access {
        Some((at, ok)) if at.elapsed() < Duration::from_secs(1) => ok,
        _ => {
            let ok = super::all_files_access();
            *access = Some((Instant::now(), ok));
            ok
        }
    };
    drop(access);
    if ok {
        return;
    }
    ui.ctx().request_repaint_after(Duration::from_secs(1));
    egui::Frame::new().fill(ui.visuals().warn_fg_color.gamma_multiply(0.15)).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(t(
            "ApexDesk necesita permiso para ver tus archivos: sin él solo se ven las carpetas.",
            "ApexDesk needs permission to see your files: without it only the folders show.",
        ));
        if ui.add_sized([ui.available_width().min(260.0), 44.0], egui::Button::new(t("Dar acceso a los archivos", "Allow access to files"))).clicked() {
            super::ask_all_files_access();
        }
    });
}
