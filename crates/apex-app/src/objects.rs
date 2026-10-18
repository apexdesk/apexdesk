//! Embedded objects, shared by both programs: painting them (their
//! preview, an icon frame, or a Diagram's drawing), the Insert > Object
//! dialog, opening other objects with the system's program and the
//! preview of an object edited in ApexDesk.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use apex_doc::{EmbedKind, Embedded, OleObject};
use apex_io::{Format, Loaded};
use eframe::egui::{self, Color32, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};

/// What a view asks the application to do with an object.
pub enum Request {
    /// Open it: in a tab (workbooks and documents) or with the system.
    Open(Arc<Embedded>),
}

/// Names of objects' files, by object id (reading them parses the data).
fn names() -> &'static Mutex<HashMap<u64, String>> {
    static N: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();
    N.get_or_init(Default::default)
}

/// The file name an object holds ("Worksheet.xlsx", "report.pdf").
pub fn file_name(obj: &Embedded) -> String {
    let Some(o) = obj.ole() else { return String::new() };
    if let Some(n) = names().lock().ok().and_then(|m| m.get(&obj.id).cloned()) {
        return n;
    }
    let n = apex_io::embed::contents(o).0;
    if let Ok(mut m) = names().lock() {
        m.insert(obj.id, n.clone());
    }
    n
}

/// The icon of an object's kind of file.
fn icon(o: &OleObject) -> Icon {
    let p = o.prog_id.to_ascii_lowercase();
    if p.starts_with("excel") || p.contains("calc") || p.contains("sheet") {
        Icon::Sheet
    } else if p.starts_with("word") || p.contains("writer") || p.contains("document") {
        Icon::Doc
    } else {
        Icon::Object
    }
}

/// Whether ApexDesk can show the picture (previews of objects are often
/// EMF or WMF; an EMF of EMF+ records only isn't drawn).
pub fn shows(preview: &[u8]) -> bool {
    use apex_sheet::ImageFormat as F;
    match F::sniff(preview) {
        Some(F::Png | F::Jpeg | F::Gif | F::Bmp | F::Webp) => true,
        Some(F::Emf | F::Wmf) => apex_io::metafile::drawable(preview),
        _ => false,
    }
}

/// Paints an object in `rect`: its preview, or a frame with an icon and
/// its name; a Diagram's shapes scaled to the box.
pub fn paint(p: &Painter, rect: Rect, preview: &Arc<[u8]>, obj: &Embedded, rotation: f32, wait: bool) {
    if !rect.is_finite() {
        return;
    }
    match &obj.kind {
        EmbedKind::Chart(c) => match live_chart(c) {
            Some(live) => {
                let (chart, data, loc) = &*live;
                let z = (rect.width() / chart.width.max(1.0)).min(rect.height() / chart.height.max(1.0));
                let clip = rect.intersect(p.clip_rect());
                crate::charts::paint(&p.with_clip_rect(clip), rect, chart, data, z, loc);
            }
            None => frame(p, rect, Icon::Chart, ""),
        },
        EmbedKind::Diagram(d) if !d.shapes.is_empty() => crate::diagram_ui::paint(p, rect, d),
        _ if shows(preview) => crate::images::paint_picture(p, preview, rect, rotation, [0.0; 4], None, rect.width(), wait),
        EmbedKind::Diagram(d) => frame(p, rect, Icon::Shapes, &d.texts.join(" · ")),
        EmbedKind::Ole(o) => {
            let name = file_name(obj);
            let label = if name.starts_with("Object.") || name.is_empty() { o.prog_id.clone() } else { name };
            frame(p, rect, icon(o), &label);
        }
    }
}

/// A chart object's chart and the data it shows, read once for each
/// version of its workbook.
type Live = Arc<(apex_sheet::Chart, apex_sheet::ChartData, apex_sheet::Locale)>;

fn live_chart(c: &apex_doc::ChartObject) -> Option<Live> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Option<Live>>>> = OnceLock::new();
    let key = apex_doc::data_key(&c.book);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().ok()?.get(&key) {
        return v.clone();
    }
    let v = apex_io::chart_object::chart_of(&c.book).map(Arc::new);
    let mut m = cache.lock().ok()?;
    if m.len() > 64 {
        m.clear();
    }
    m.insert(key, v.clone());
    v
}

/// A white frame with an icon and a label under it.
fn frame(p: &Painter, rect: Rect, ic: Icon, label: &str) {
    p.rect_filled(rect, 2.0, Color32::WHITE);
    p.rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::from_gray(150)), StrokeKind::Inside);
    let side = (rect.height() * 0.5).min(rect.width() * 0.6).clamp(4.0, 64.0);
    let text_h = (side * 0.3).clamp(7.0, 14.0);
    let top = rect.center().y - (side + text_h * 1.4) / 2.0;
    let ir = Rect::from_min_size(pos2(rect.center().x - side / 2.0, top.max(rect.top())), vec2(side, side));
    icons::paint(p, ir, ic, Color32::from_rgb(0x1A, 0x73, 0xE8));
    if rect.height() > side + text_h && !label.is_empty() {
        let font = egui::FontId::proportional(text_h);
        let galley = p.layout(label.to_string(), font, Color32::from_gray(40), rect.width() - 6.0);
        let at = pos2(rect.center().x - galley.size().x / 2.0, ir.bottom() + text_h * 0.3);
        p.with_clip_rect(rect.intersect(p.clip_rect())).galley(at, galley, Color32::from_gray(40));
    }
}

/// What an object's context menu chose.
pub enum MenuAction {
    Open,
    SaveAs,
}

/// Open and Save as file, at the top of an object's context menu.
pub fn menu(ui: &mut egui::Ui, obj: &Embedded) -> Option<MenuAction> {
    if obj.chart().is_some() {
        // A chart: its data opens in a tab.
        let open = ui.button(egui::RichText::new(format!("{}…", tr("chart.edit_data"))).strong()).clicked();
        if open {
            ui.close();
        }
        ui.separator();
        return open.then_some(MenuAction::Open);
    }
    obj.ole()?;
    let mut out = None;
    if ui.button(egui::RichText::new(tr("object.open")).strong()).clicked() {
        out = Some(MenuAction::Open);
        ui.close();
    }
    if ui.button(format!("{}…", tr("object.save_as"))).clicked() {
        out = Some(MenuAction::SaveAs);
        ui.close();
    }
    ui.separator();
    out
}

// --- opening and saving -----------------------------------------------------------

/// A private folder for objects opened with other programs.
fn temp_dir(id: u64) -> std::io::Result<PathBuf> {
    apex_io::safe_path::private_temp_dir("apexdesk-objects", id)
}

/// A file name that is safe to create: no folders, no reserved characters.
fn safe_name(name: &str) -> String {
    apex_io::safe_path::safe_file_name(name, "embedded.bin")
}

/// Whether an object named `name` (written as `file`) would run something.
fn blocked_name(name: &str, file: &str) -> bool {
    crate::links::runs(name) || crate::links::runs(file)
}

/// As [`blocked_name`], also by its contents (a program, whatever its name).
fn blocked(name: &str, file: &str, data: &[u8]) -> bool {
    blocked_name(name, file) || crate::links::copy_runs(file, data)
}

/// Writes the object's file to a private folder and opens it with the
/// system's program for it (once the user agrees).
pub fn open_with_system(obj: &Embedded) -> Result<(), String> {
    let Some(o) = obj.ole() else { return Err(String::new()) };
    let (name, data) = apex_io::embed::contents(o);
    // A package's file name is the document's to choose: never run one.
    // The name checked is the one written, as cleaning it may uncover an
    // extension (`a:b.exe` becomes `a_b.exe`).
    let file = safe_name(&name);
    if blocked(&name, &file, &data) {
        return Err(trf("links.object_blocked", &[("name", &name)]));
    }
    let path = temp_dir(obj.id).map_err(|e| e.to_string())?.join(&file);
    apex_fs::write(&path, data).map_err(|e| e.to_string())?;
    apex_io::safe_path::mark_untrusted(&path);
    // As a link to the file would: the user is asked first, and a policy
    // that blocks links blocks this too.
    crate::links::follow(&path.to_string_lossy())
}

/// Removes the files of objects opened with other programs.
pub fn clean_temp() {
    let _ = apex_fs::remove_dir_all(apex_io::safe_path::private_temp_root("apexdesk-objects"));
    let _ = apex_fs::remove_dir_all(apex_io::safe_path::private_temp_root("apexdesk-attachments"));
    let _ = apex_fs::remove_dir_all(apex_io::safe_path::private_temp_root("apexdesk-print"));
    for prefix in ["apexdesk-batch", "apexdesk-dossier", "apexdesk-mail", "apexdesk-net"] {
        let _ = apex_fs::remove_dir_all(apex_io::safe_path::private_temp_root(prefix));
    }
}

/// Asks where to save the object's file and writes it.
pub fn save_as_file(obj: &Embedded) {
    let Some(o) = obj.ole() else { return };
    let (name, data) = apex_io::embed::contents(o);
    let data = data.to_vec();
    crate::file_dialog::FileDialog::new().set_title(tr("object.save_as")).set_file_name(safe_name(&name)).save_file(move |app, path| {
        // Marked as come from elsewhere, as the document's file is.
        match crate::common::write_out(&path, data) {
            Ok(()) => apex_io::safe_path::mark_untrusted(&path),
            Err(e) => app.notify(crate::app::Notice::error(crate::i18n::trf("error.save", &[("error", &e.to_string())]))),
        }
    });
}

// --- previews -----------------------------------------------------------------------

/// Rasterizes the part of a printed page that has something on it, as PNG.
pub fn page_png(ctx: &egui::Context, page: &crate::print::Page) -> Option<Vec<u8>> {
    let area = page.size.x * page.size.y;
    let mut bounds = Rect::NOTHING;
    for s in &page.shapes {
        let r = s.shape.visual_bounding_rect();
        // Page backgrounds don't count.
        if r.is_finite() && r.width() * r.height() < area * 0.8 {
            bounds = bounds.union(r);
        }
    }
    let page_rect = Rect::from_min_size(Pos2::ZERO, page.size);
    let bounds = bounds.expand(6.0).intersect(page_rect);
    let ppp = ctx.pixels_per_point();
    let longest = bounds.width().max(bounds.height()) * ppp;
    crate::charts::encode_png(&page_image(ctx, page, bounds, longest.min(1600.0) * bounds.width() / bounds.width().max(bounds.height()))?)
}

/// The part `bounds` of a printed page as an image `width` pixels wide,
/// on white.
pub fn page_image(ctx: &egui::Context, page: &crate::print::Page, bounds: Rect, width: f32) -> Option<egui::ColorImage> {
    if !bounds.is_positive() || width < 1.0 {
        return None;
    }
    let ppp = ctx.pixels_per_point();
    let scale = width / (bounds.width() * ppp);
    let (w, h) = ((bounds.width() * ppp * scale).ceil() as usize, (bounds.height() * ppp * scale).ceil() as usize);
    let shift = -bounds.min.to_vec2();
    let shapes: Vec<egui::epaint::ClippedShape> = page
        .shapes
        .iter()
        .map(|s| {
            let mut shape = s.shape.clone();
            shape.translate(shift);
            egui::epaint::ClippedShape { clip_rect: s.clip_rect.translate(shift), shape }
        })
        .collect();
    let prims = ctx.tessellate(shapes, ppp);
    let atlas = ctx.fonts(|f| f.image());
    let mut img = egui::ColorImage::new([w, h], vec![Color32::WHITE; w * h]);
    let mut textures: HashMap<egui::TextureId, Option<egui::ColorImage>> = HashMap::new();
    for prim in prims {
        let egui::epaint::Primitive::Mesh(m) = prim.primitive else { continue };
        if m.texture_id == egui::TextureId::default() {
            crate::charts::raster(&mut img, &m, &atlas, ppp * scale);
            continue;
        }
        let tex = textures.entry(m.texture_id).or_insert_with(|| {
            let d = crate::images::pixels(m.texture_id).or_else(|| crate::images::source(m.texture_id).and_then(|b| crate::images::decode(&b)))?;
            let px: Vec<Color32> = d.rgba.chunks_exact(4).map(|c| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])).collect();
            Some(egui::ColorImage::new([d.width as usize, d.height as usize], px))
        });
        if let Some(t) = tex {
            crate::charts::raster(&mut img, &m, t, ppp * scale);
        }
    }
    Some(img)
}

/// The preview of a workbook or document: its first printed page.
pub fn preview(ctx: &egui::Context, loaded: &mut Loaded) -> Option<Vec<u8>> {
    match loaded {
        Loaded::Sheet(wb) => {
            let mut v = crate::sheet_view::SheetView::new((**wb).clone());
            let job = v.print_job("", crate::print::Scope::ActiveSheet);
            page_png(ctx, &v.paint_planned(ctx, job.plan.as_ref()?, 0)?)
        }
        Loaded::Doc(d) => page_png(ctx, crate::doc_view::DocView::new((**d).clone()).print_job(ctx, "").pages.first()?),
        Loaded::Slides(d) => page_png(ctx, crate::slides_view::SlidesView::new((**d).clone()).print_job(ctx, "").pages.first()?),
    }
}

/// The size (points) to show a preview at: its pixels at 96 dpi, fitted
/// in `max` points.
pub fn preview_size(png: &[u8], max: (f32, f32)) -> (f32, f32) {
    let (w, h) = apex_io::pixel_size(png).map_or((max.0, max.1 / 2.0), |(w, h)| (w as f32 * 0.75, h as f32 * 0.75));
    let k = (max.0 / w.max(1.0)).min(max.1 / h.max(1.0)).min(1.0);
    (w * k, h * k)
}

// --- Insert > Object -----------------------------------------------------------------

/// What Insert > Object makes.
pub enum NewObject {
    /// A new workbook or document (true), to edit in its tab at once.
    Blank(bool),
    File(PathBuf),
}

/// The Insert > Object dialog.
#[derive(Default)]
pub struct InsertDlg {
    open: bool,
    from_file: bool,
    doc: bool,
    path: Option<PathBuf>,
    icon: bool,
    /// Browse's answer.
    picked: crate::file_dialog::Slot<PathBuf>,
}

/// An object made by the dialog: the object, its preview, and whether
/// to open it for editing now.
pub struct Made {
    pub object: Arc<Embedded>,
    pub preview: Arc<[u8]>,
    /// Size in points.
    pub size: (f32, f32),
    pub edit: bool,
}

impl InsertDlg {
    pub fn show(&mut self) {
        *self = InsertDlg { open: true, ..Default::default() };
    }

    /// Shows the dialog; returns the object made when accepted.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Made> {
        if !self.open {
            return None;
        }
        if let Some(p) = self.picked.take() {
            self.path = Some(p);
        }
        let mut done = None;
        let res = crate::widgets::dialog(ctx, "insert-object", tr("object.insert"), 420.0, |ui| {
            ui.radio_value(&mut self.from_file, false, tr("object.create_new"));
            ui.add_enabled_ui(!self.from_file, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    ui.radio_value(&mut self.doc, false, tr("object.new_sheet"));
                    ui.radio_value(&mut self.doc, true, tr("object.new_doc"));
                });
            });
            ui.radio_value(&mut self.from_file, true, tr("object.from_file"));
            ui.add_enabled_ui(self.from_file, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    let shown = self.path.as_ref().and_then(|p| p.file_name()).map_or_else(|| tr("object.no_file").to_string(), |n| n.to_string_lossy().into_owned());
                    ui.label(shown);
                    if ui.button(format!("{}…", tr("object.browse"))).clicked() {
                        crate::file_dialog::FileDialog::new().set_title(tr("object.from_file")).pick_file(self.picked.fill());
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    ui.checkbox(&mut self.icon, tr("object.as_icon"));
                });
            });
        });
        match res {
            crate::widgets::DialogResult::Ok => {
                let what = if self.from_file { self.path.clone().map(NewObject::File) } else { Some(NewObject::Blank(self.doc)) };
                self.open = false;
                done = what.and_then(|w| make(ctx, w, self.icon));
            }
            crate::widgets::DialogResult::Cancel => self.open = false,
            crate::widgets::DialogResult::None => {}
        }
        done
    }
}

/// Makes an object: a blank workbook or document, or a file (Office and
/// ODF files show their first page; others, or `icon`, an icon).
pub fn make(ctx: &egui::Context, what: NewObject, icon: bool) -> Option<Made> {
    let (object, edit) = match what {
        NewObject::Blank(doc) => {
            let (name, data) = if doc {
                ("Document.docx", apex_io::docx::write(&apex_doc::Document::new()))
            } else {
                let mut wb = apex_sheet::Workbook::empty();
                wb.sheets.push(apex_sheet::Sheet::new(crate::i18n::trf("sheet.default_name", &[("n", &1)])));
                ("Worksheet.xlsx", apex_io::xlsx::write(&wb))
            };
            (apex_io::embed::from_file(name, data), true)
        }
        NewObject::File(path) => {
            let data = apex_fs::read(&path).ok()?;
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            (apex_io::embed::from_file(&name, data), false)
        }
    };
    let object = OleObject { icon: icon || apex_io::embed::openable(&object).is_none(), ..object };
    let png = if object.icon {
        None
    } else {
        apex_io::embed::openable(&object).and_then(|(data, fmt)| apex_io::embed::load(&data, fmt).ok()).and_then(|mut l| preview(ctx, &mut l))
    };
    let (preview, size) = match png {
        Some(p) => {
            let size = preview_size(&p, (400.0, 300.0));
            (Arc::from(p), size)
        }
        None if edit => (Arc::from(&[][..]), (240.0, 120.0)),
        None => (Arc::from(&[][..]), (72.0, 64.0)),
    };
    Some(Made { object: Arc::new(Embedded::new(EmbedKind::Ole(object))), preview, size, edit })
}

/// The format an object's file is saved back in (legacy files come back
/// as Office Open XML).
pub fn save_format(fmt: Format) -> Format {
    match fmt {
        Format::Xls | Format::Xlsb => Format::Xlsx,
        Format::Doc => Format::Docx,
        f => f,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked_as_written() {
        for n in ["Invoice:x.exe", "evil.exe\u{a0}", "a.pdf:b.hta", "dir/x.js\u{2003}", "report.exe"] {
            assert!(blocked_name(n, &safe_name(n)), "{n} -> {}", safe_name(n));
        }
        assert!(!blocked_name("report.pdf", &safe_name("report.pdf")));
        assert!(!blocked_name("data.xlsx", &safe_name("data.xlsx")));
    }
}
