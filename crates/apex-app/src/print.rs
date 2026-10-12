//! Printing and PDF export. Views paint their pages with egui into a
//! capture layer; the shapes are then written as PDF, sent to the printer
//! (as PDF through CUPS' `lp`, or on Windows through the PDF viewer) or
//! shown in the print preview.

use std::sync::mpsc::Receiver;

use apex_doc::{HeaderFooter, HfContext, HfText, PAPER_SIZES, expand_fields};
use eframe::egui::{self, Color32, Id, LayerId, Order, Painter, Rect, RichText, Stroke, Vec2, emath::TSTransform, epaint::ClippedShape, pos2, vec2};

use crate::fonts::{self, Face};
use crate::imposition::{Layout, PER_SHEET, Side, Sides};
use crate::i18n::{tr, trf};

/// Pixels per point in printed pages (the views' 96 dpi).
pub const PT: f32 = 1.333;

#[derive(Clone)]
pub enum LinkTarget {
    Uri(String),
    /// A page and a y offset in pixels from its top.
    Page(usize, f32),
}

/// One printed page, in pixels from its top-left corner.
#[derive(Clone)]
pub struct Page {
    pub size: Vec2,
    pub shapes: Vec<ClippedShape>,
    pub links: Vec<(Rect, LinkTarget)>,
    /// Where the parts of the document's structure are (tagged PDF);
    /// what lies outside them is decoration.
    pub tags: Vec<Tag>,
}

/// A part of a page in the document's structure: a line of a paragraph.
#[derive(Clone)]
pub struct Tag {
    pub rect: Rect,
    /// PDF structure type: P, H1..H6, BlockQuote.
    pub role: &'static str,
    /// The same for every piece of one paragraph.
    pub key: u64,
}

/// The PDF options last used (tagged by default, as Word does).
fn last_options() -> &'static std::sync::Mutex<PdfOptions> {
    static LAST: std::sync::OnceLock<std::sync::Mutex<PdfOptions>> = std::sync::OnceLock::new();
    LAST.get_or_init(|| std::sync::Mutex::new(PdfOptions { pdfa: false, tagged: true }))
}

/// Kinds of PDF the export writes.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct PdfOptions {
    /// PDF/A-2b, for archiving.
    pub pdfa: bool,
    /// Tagged (accessible) PDF: documents only.
    pub tagged: bool,
}

#[derive(Default)]
pub struct Job {
    pub title: String,
    pub author: String,
    /// Pages painted in advance (documents).
    pub pages: Vec<Page>,
    /// Sheets: their pages are only planned, and each one is painted when
    /// it is needed (see [`Paint`]), so that a sheet of thousands of pages
    /// never holds the shapes of all of them.
    pub plan: Option<crate::sheet_view::PrintPlan>,
    /// Bookmarks: title, level, page, y in pixels.
    pub outline: Vec<(String, u8, usize, f32)>,
    /// Picture textures the pages use, kept alive with the job.
    pub textures: Vec<egui::TextureHandle>,
    pub pdf: PdfOptions,
    /// Language of the text (BCP 47) and the pictures' alternative texts
    /// by `images::key`, for tagged PDF.
    pub lang: String,
    pub alts: std::collections::HashMap<u64, String>,
}

/// Paints page `n` of a job whose pages are planned (see [`Job::plan`]),
/// with the view the job comes from.
pub type Paint<'a> = dyn FnMut(&Job, usize) -> Option<Page> + 'a;

/// For jobs painted in advance.
pub fn no_paint(_: &Job, _: usize) -> Option<Page> {
    None
}

impl Job {
    /// Number of pages.
    pub fn len(&self) -> usize {
        self.plan.as_ref().map_or(self.pages.len(), |p| p.count())
    }

    /// Page `n`: the one painted in advance or, for a planned job, painted
    /// now by `paint`.
    pub fn page<'a>(&'a self, n: usize, paint: &mut Paint<'_>) -> Option<std::borrow::Cow<'a, Page>> {
        match self.plan {
            Some(_) if n < self.len() => paint(self, n).map(std::borrow::Cow::Owned),
            Some(_) => None,
            None => self.pages.get(n).map(std::borrow::Cow::Borrowed),
        }
    }
}

/// Paints with `f` and returns the shapes instead of showing them.
pub fn capture(ctx: &egui::Context, f: impl FnOnce(&Painter)) -> Vec<ClippedShape> {
    let layer = LayerId::new(Order::Background, Id::new("apex-print-capture"));
    let painter = Painter::new(ctx.clone(), layer, Rect::EVERYTHING);
    f(&painter);
    ctx.graphics_mut(|g| std::mem::take(g.entry(layer))).all_entries().cloned().collect()
}

/// Header and footer distances from the page edges, and the side margins,
/// in pixels.
#[derive(Clone, Copy)]
pub struct HfBox {
    pub header: f32,
    pub footer: f32,
    pub left: f32,
    pub right: f32,
}

/// Values of the header and footer fields that don't depend on the page.
pub struct HfValues {
    pub date: String,
    pub time: String,
    pub file: String,
}

impl HfValues {
    pub fn now(file: &str) -> HfValues {
        let (date, time) = crate::common::date_time_now();
        HfValues { date, time, file: file.to_string() }
    }

    pub fn context<'a>(&'a self, page: usize, pages: usize, sheet: &'a str) -> HfContext<'a> {
        HfContext { page, pages, date: &self.date, time: &self.time, file: &self.file, sheet }
    }
}

/// Paints the header and footer of page `index` (0-based) inside `page`.
pub fn paint_hf(p: &Painter, hf: &HeaderFooter, index: usize, cx: &HfContext, page: Rect, b: HfBox, z: f32, color: Color32) {
    let (header, footer) = hf.for_page(index);
    let font = fonts::font(Face::Sans, false, 10.0 * PT * z);
    for (t, top) in [(header, true), (footer, false)] {
        let parts = t.parts();
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            let text = expand_fields(part, cx).replace('\n', " ");
            let g = p.layout_no_wrap(text, font.clone(), color);
            let x = match i {
                0 => page.min.x + b.left,
                1 => page.center().x - g.size().x / 2.0,
                _ => page.max.x - b.right - g.size().x,
            };
            let y = if top { page.min.y + b.header } else { page.max.y - b.footer - g.size().y };
            p.galley(pos2(x, y), g, color);
        }
    }
}

// --- page ranges ------------------------------------------------------------------

/// Parses "1-3, 5, 8-" into 0-based page indexes below `n`.
pub fn parse_range(s: &str, n: usize) -> Option<Vec<usize>> {
    let mut out = Vec::new();
    for part in s.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => {
                let a: usize = if a.trim().is_empty() { 1 } else { a.trim().parse().ok()? };
                let b: usize = if b.trim().is_empty() { n } else { b.trim().parse().ok()? };
                (a, b)
            }
            None => {
                let a: usize = part.parse().ok()?;
                (a, a)
            }
        };
        if a == 0 || b < a {
            return None;
        }
        out.extend((a..=b.min(n)).map(|i| i - 1));
    }
    out.dedup();
    Some(out)
}

// --- printers ----------------------------------------------------------------------

fn command(cmd: &str) -> std::process::Command {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing up.
        c.creation_flags(0x0800_0000);
    }
    c
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = command(cmd).args(args).stdin(std::process::Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A PowerShell string literal.
#[cfg(windows)]
fn ps_quote(s: &str) -> String {
    // PowerShell also ends a literal at the typographic single quotes.
    let mut out = String::from("'");
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Installed printers and the default one.
#[cfg(windows)]
pub(crate) fn list_printers() -> (Vec<String>, Option<String>) {
    let ps = |script: &str| run(&apex_io::safe_path::windows_program(r"System32\WindowsPowerShell\v1.0\powershell.exe").to_string_lossy(), &["-NoProfile", "-NonInteractive", "-Command", script]);
    let names: Vec<String> = ps("Get-Printer | ForEach-Object Name").map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default();
    let default = ps("(Get-CimInstance Win32_Printer -Filter 'Default=true').Name").map(|s| s.trim().to_string()).filter(|d| names.contains(d));
    (names, default)
}

/// Sends a PDF file to a printer, through the PDF viewer registered for
/// printing (Windows has no PDF printing of its own).
#[cfg(windows)]
fn send_to_printer(printer: &str, copies: u32, _sides: Sides, path: &std::path::Path) -> Result<(), String> {
    // Tests print nothing.
    if cfg!(test) {
        return Ok(());
    }
    // The viewer prints with the printer's own settings: both sides are
    // set in its preferences (or printed by hand, see Sides::Manual).
    let script = format!(
        "$ErrorActionPreference = 'Stop'; 1..{} | ForEach-Object {{ Start-Process -FilePath {} -Verb PrintTo -ArgumentList {} -WindowStyle Hidden }}",
        copies.max(1),
        ps_quote(&path.to_string_lossy()),
        ps_quote(&format!("\"{printer}\"")),
    );
    let out = command(&apex_io::safe_path::windows_program(r"System32\WindowsPowerShell\v1.0\powershell.exe").to_string_lossy()).args(["-NoProfile", "-NonInteractive", "-Command", &script]).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// Installed printers (CUPS) and the default one.
#[cfg(not(windows))]
pub(crate) fn list_printers() -> (Vec<String>, Option<String>) {
    let names: Vec<String> = run("lpstat", &["-e"]).map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default();
    let default = run("lpstat", &["-d"]).and_then(|s| s.rsplit_once(':').map(|(_, d)| d.trim().to_string())).filter(|d| names.contains(d));
    (names, default)
}

/// Sends a PDF file to a printer.
#[cfg(not(windows))]
fn send_to_printer(printer: &str, copies: u32, sides: Sides, path: &std::path::Path) -> Result<(), String> {
    // Tests print nothing.
    if cfg!(test) {
        return Ok(());
    }
    // Both sides by the printer: the IPP "sides" option (a manual pass is
    // one-sided).
    let sides = match sides {
        Sides::LongEdge => "sides=two-sided-long-edge",
        Sides::ShortEdge => "sides=two-sided-short-edge",
        Sides::One | Sides::Manual => "sides=one-sided",
    };
    let out = command("lp")
        .args(["-d", printer, "-n", &copies.max(1).to_string(), "-o", sides])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

// --- the print dialog --------------------------------------------------------------

/// What part of a workbook is printed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    ActiveSheet,
    Workbook,
    Selection,
    /// Presentations: a notes page per slide.
    Notes,
    /// Presentations: handouts of this many slides a page.
    Handouts(u8),
}

pub enum PrintAction {
    /// Re-render the job (the scope changed).
    Rerender,
    Print { printer: String, copies: u32, pages: Vec<usize>, layout: Layout },
    ExportPdf { pages: Vec<usize>, layout: Layout },
    PageSetup,
    Close,
}

pub struct PrintDialog {
    /// Tab the job comes from.
    pub tab: u64,
    pub job: Job,
    pub sheet: bool,
    /// A presentation: slides, notes pages or handouts.
    pub slides: bool,
    pub scope: Scope,
    printers: Vec<String>,
    printer: usize,
    loading: Option<Receiver<(Vec<String>, Option<String>)>>,
    copies: u32,
    all_pages: bool,
    range: String,
    page: usize,
    /// The page shown (painted when it is shown, for a planned job).
    shown: Option<(usize, Page)>,
    /// Preview shapes for (side, scale).
    cache: Option<(usize, f32, Vec<ClippedShape>)>,
    /// Pages per sheet, booklet and printing on both sides.
    pub layout: Layout,
    /// The sides chosen before the booklet turned them to the short edge.
    sides_before_booklet: Option<Sides>,
    /// PDF/A and tags for the export.
    pdf: PdfOptions,
    /// File > Export > PDF: the export's options, no printer.
    pub export: bool,
}

impl PrintDialog {
    pub fn new(tab: u64, job: Job, sheet: bool) -> PrintDialog {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("printers".into())
            .spawn(move || {
                tx.send(list_printers()).ok();
            })
            .ok();
        PrintDialog {
            tab,
            job,
            sheet,
            slides: false,
            scope: Scope::ActiveSheet,
            printers: Vec::new(),
            printer: 0,
            loading: Some(rx),
            copies: 1,
            all_pages: true,
            range: String::new(),
            page: 0,
            shown: None,
            cache: None,
            layout: Layout::default(),
            sides_before_booklet: None,
            pdf: last_options().lock().map(|o| *o).unwrap_or_default(),
            export: false,
        }
    }

    pub fn set_job(&mut self, job: Job) {
        self.job = job;
        self.page = self.page.min(self.job.len().saturating_sub(1));
        self.shown = None;
        self.cache = None;
    }

    /// The sides of paper printed for the pages chosen, and the paper
    /// size (that of the first page).
    fn sides(&self, paint: &mut Paint<'_>) -> Vec<Side> {
        let pages = self.pages().unwrap_or_default();
        if self.layout.is_plain() {
            return pages.into_iter().map(Side::Page).collect();
        }
        let paper = pages.first().and_then(|&p| self.job.page(p, paint)).map_or(vec2(794.0, 1123.0), |p| p.size);
        crate::imposition::sides(&pages, self.layout, paper)
    }

    fn pages(&self) -> Option<Vec<usize>> {
        let n = self.job.len();
        if self.all_pages { Some((0..n).collect()) } else { parse_range(&self.range, n).filter(|v| !v.is_empty()) }
    }

    /// `paint` paints the pages of a planned job (see [`Job::page`]).
    pub fn ui(&mut self, ctx: &egui::Context, paint: &mut Paint<'_>) -> Option<PrintAction> {
        if let Some(rx) = &self.loading {
            if let Ok((names, default)) = rx.try_recv() {
                self.printer = default.and_then(|d| names.iter().position(|n| *n == d)).unwrap_or(0);
                self.printers = names;
                self.loading = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        let mut action = None;
        let mut open = true;
        let screen = ctx.content_rect();
        egui::Window::new(tr(if self.export { "print.export_pdf" } else { "print.title" }))
            .id(Id::new("print-dialog"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            // Centered by its size (known before it shows): it doesn't
            // move once seen.
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .show(ctx, |ui| {
                // A fixed size: what is chosen doesn't move it, and it
                // fits a small window (the settings scroll).
                let h = self.height(ctx);
                ui.horizontal_top(|ui| {
                    egui::ScrollArea::vertical().id_salt("print-settings").auto_shrink([false, false]).max_width(280.0).max_height(h).show(ui, |ui| {
                        // The scroll takes the row's layout: the settings go down.
                        ui.vertical(|ui| {
                            ui.set_width(268.0);
                            self.settings(ui, &mut action);
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| self.preview(ui, paint));
                });
            });
        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Some(PrintAction::Close);
        }
        action
    }

    /// The height of the dialog's body.
    fn height(&self, ctx: &egui::Context) -> f32 {
        (crate::widgets::dialog_room(ctx).y + ctx.global_style().spacing.interact_size.y).min(600.0)
    }

    fn settings(&mut self, ui: &mut egui::Ui, action: &mut Option<PrintAction>) {
        let n = self.job.len();
        if !self.export {
            self.printer_ui(ui);
        }
        if self.sheet {
            crate::widgets::section(ui, tr("print.what"));
            let before = self.scope;
            for (s, key) in [(Scope::ActiveSheet, "print.active_sheet"), (Scope::Workbook, "print.workbook"), (Scope::Selection, "print.selection")] {
                ui.radio_value(&mut self.scope, s, tr(key));
            }
            if self.scope != before {
                *action = Some(PrintAction::Rerender);
            }
        }
        if self.slides {
            crate::widgets::section(ui, tr("print.what"));
            let before = self.scope;
            ui.radio_value(&mut self.scope, Scope::ActiveSheet, tr("slides.print_slides"));
            ui.radio_value(&mut self.scope, Scope::Notes, tr("slides.print_notes"));
            ui.horizontal(|ui| {
                let on = matches!(self.scope, Scope::Handouts(_));
                if ui.radio(on, tr("slides.print_handouts")).clicked() && !on {
                    self.scope = Scope::Handouts(6);
                }
                let mut n = if let Scope::Handouts(n) = self.scope { n } else { 6 };
                let mut picked = false;
                crate::widgets::combo_name(
                    egui::ComboBox::from_id_salt("handouts").width(60.0).selected_text(n.to_string()).show_ui(ui, |ui| {
                        for k in crate::slides_view::handouts::HANDOUTS {
                            picked |= ui.selectable_value(&mut n, k, k.to_string()).clicked();
                        }
                    }),
                    tr("slides.print_handouts"),
                );
                if picked {
                    self.scope = Scope::Handouts(n);
                }
            });
            if self.scope != before {
                *action = Some(PrintAction::Rerender);
            }
        }
        crate::widgets::section(ui, tr("print.pages"));
        ui.radio_value(&mut self.all_pages, true, tr("print.all_pages"));
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.all_pages, false, tr("print.page_range"));
            let r = ui.add(egui::TextEdit::singleline(&mut self.range).desired_width(110.0).hint_text("1-3, 5"));
            if r.changed() {
                self.all_pages = false;
            }
        });
        let pages = self.pages();
        if pages.is_none() {
            ui.label(RichText::new(tr("print.bad_range")).color(ui.visuals().error_fg_color));
        }
        ui.add_space(4.0);
        ui.label(RichText::new(trf("print.sheets_count", &[("n", &n)])).weak());
        self.layout_ui(ui);
        crate::widgets::section(ui, tr("pdf.options"));
        if !self.sheet {
            ui.checkbox(&mut self.pdf.tagged, tr("pdf.tagged")).on_hover_text(tr("pdf.tagged_tip"));
        }
        ui.checkbox(&mut self.pdf.pdfa, tr("pdf.pdfa")).on_hover_text(tr("pdf.pdfa_tip"));
        crate::widgets::section(ui, tr("page.setup"));
        if ui.button(format!("{}…", tr("page.setup"))).clicked() {
            *action = Some(PrintAction::PageSetup);
        }
        ui.add_space(24.0);
        ui.separator();
        ui.horizontal(|ui| {
            let can_print = pages.is_some() && n > 0 && !self.printers.is_empty();
            if !self.export && ui.add_enabled(can_print, egui::Button::new(RichText::new(tr("print.print")).strong())).clicked() {
                if let (Some(pages), Some(printer)) = (pages.clone(), self.printers.get(self.printer)) {
                    *action = Some(PrintAction::Print { printer: printer.clone(), copies: self.copies, pages, layout: self.layout });
                }
            }
            let mut text = RichText::new(format!("{}…", tr("print.export_pdf")));
            if self.export {
                text = text.strong();
            }
            if ui.add_enabled(pages.is_some() && n > 0, egui::Button::new(text)).clicked() {
                if let Some(pages) = pages {
                    if let Ok(mut o) = last_options().lock() {
                        *o = self.pdf;
                    }
                    self.job.pdf = PdfOptions { tagged: self.pdf.tagged && !self.sheet, ..self.pdf };
                    *action = Some(PrintAction::ExportPdf { pages, layout: self.layout });
                }
            }
            if ui.button(tr("dialog.cancel")).clicked() {
                *action = Some(PrintAction::Close);
            }
        });
    }

    /// The printer and the number of copies.
    fn printer_ui(&mut self, ui: &mut egui::Ui) {
        crate::widgets::section(ui, tr("print.printer"));
        // One height whatever shows: what is below stays put once the
        // printers are found (the no-printers note takes two lines).
        ui.horizontal(|ui| {
            ui.set_min_height(40.0);
            if self.loading.is_some() {
                ui.spinner();
                ui.label(tr("print.looking"));
            } else if self.printers.is_empty() {
                ui.add(egui::Label::new(RichText::new(tr("print.no_printers")).weak().small()).wrap());
            } else {
                crate::widgets::combo_name(
                    egui::ComboBox::from_id_salt("printer").width(250.0).selected_text(self.printers.get(self.printer).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                        for (i, p) in self.printers.iter().enumerate() {
                            ui.selectable_value(&mut self.printer, i, p);
                        }
                    }),
                    tr("print.printer"),
                );
            }
        });
        ui.add_space(4.0);
        crate::widgets::form(ui, "print_copies", |ui| {
            crate::widgets::form_row(ui, tr("print.copies"), |ui| ui.add(egui::DragValue::new(&mut self.copies).range(1..=999)));
        });
    }

    /// Pages per sheet, booklet and both sides.
    fn layout_ui(&mut self, ui: &mut egui::Ui) {
        crate::widgets::section(ui, tr("print.layout"));
        let before = self.layout;
        crate::widgets::form(ui, "print_layout", |ui| {
            crate::widgets::form_row(ui, tr("print.per_sheet"), |ui| {
                ui.add_enabled_ui(!self.layout.booklet, |ui| {
                    crate::widgets::combo_name(
                        egui::ComboBox::from_id_salt("per_sheet").width(150.0).selected_text(self.layout.per_sheet.to_string()).show_ui(ui, |ui| {
                            for n in PER_SHEET {
                                ui.selectable_value(&mut self.layout.per_sheet, n, n.to_string());
                            }
                        }),
                        tr("print.per_sheet"),
                    );
                })
                .response
            });
            crate::widgets::form_row(ui, tr("print.sides"), |ui| {
                crate::widgets::combo_name(
                    egui::ComboBox::from_id_salt("print_sides").width(150.0).selected_text(tr(self.layout.sides.key())).show_ui(ui, |ui| {
                        for s in Sides::ALL {
                            ui.selectable_value(&mut self.layout.sides, s, tr(s.key()));
                        }
                    }),
                    tr("print.sides"),
                )
                .response
            });
        });
        if ui.checkbox(&mut self.layout.booklet, tr("print.booklet")).on_hover_text(tr("print.booklet_tip")).changed() {
            booklet_sides(&mut self.layout, &mut self.sides_before_booklet);
        }
        if cfg!(windows) && matches!(self.layout.sides, Sides::LongEdge | Sides::ShortEdge) {
            ui.label(RichText::new(tr("print.sides_driver")).weak().small());
        }
        if self.layout != before {
            self.page = 0;
            self.shown = None;
            self.cache = None;
        }
    }

    fn preview(&mut self, ui: &mut egui::Ui, paint: &mut Paint<'_>) {
        if !self.layout.is_plain() {
            return self.preview_sides(ui, paint);
        }
        let n = self.job.len();
        let area = vec2((crate::widgets::dialog_room(ui.ctx()).x - 300.0).clamp(160.0, 460.0), self.height(ui.ctx()) - ui.spacing().interact_size.y - ui.spacing().item_spacing.y);
        let (rect, _) = ui.allocate_exact_size(area, egui::Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 4.0, crate::theme::workspace_bg(ui.visuals()));
        if n == 0 {
            p.text(rect.center(), egui::Align2::CENTER_CENTER, tr("print.nothing"), egui::FontId::proportional(14.0), ui.visuals().weak_text_color());
        } else {
            self.page = self.page.min(n - 1);
            // Only the page shown is painted: a sheet of thousands of
            // pages opens the dialog at once.
            if self.shown.as_ref().is_none_or(|(i, _)| *i != self.page) {
                self.shown = self.job.page(self.page, paint).map(|p| (self.page, p.into_owned()));
            }
            let Some((_, page)) = &self.shown else { return };
            let room = rect.shrink(16.0);
            let scale = (room.width() / page.size.x).min(room.height() / page.size.y);
            let size = page.size * scale;
            let r = Rect::from_center_size(room.center(), size);
            p.rect_filled(r.translate(vec2(0.0, 2.0)).expand(1.0), 1.0, Color32::from_black_alpha(50));
            p.rect_filled(r, 0.0, Color32::WHITE);
            if self.cache.as_ref().is_none_or(|(i, s, _)| *i != self.page || *s != scale) {
                let t = TSTransform::from_scaling(scale);
                let shapes = page
                    .shapes
                    .iter()
                    .map(|c| {
                        let mut s = c.clone();
                        s.shape.transform(t);
                        s.clip_rect = t.mul_rect(s.clip_rect);
                        s
                    })
                    .collect();
                self.cache = Some((self.page, scale, shapes));
            }
            if let Some((_, _, shapes)) = &self.cache {
                let off = TSTransform::from_translation(r.min.to_vec2());
                for c in shapes {
                    let mut s = c.shape.clone();
                    s.transform(off);
                    let clip = off.mul_rect(c.clip_rect).intersect(r);
                    p.with_clip_rect(clip.intersect(rect)).add(s);
                }
            }
            p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_gray(160)), egui::StrokeKind::Outside);
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(self.page > 0, egui::Button::new("<")).clicked() {
                self.page -= 1;
            }
            ui.label(trf("print.page_of", &[("n", &(self.page + 1).min(n)), ("total", &n)]));
            if ui.add_enabled(self.page + 1 < n, egui::Button::new(">")).clicked() {
                self.page += 1;
            }
        });
    }
}

impl PrintDialog {
    /// The preview of the sides of paper when pages go several per sheet
    /// or as a booklet.
    fn preview_sides(&mut self, ui: &mut egui::Ui, paint: &mut Paint<'_>) {
        let sides = self.sides(paint);
        let n = sides.len();
        self.page = self.page.min(n.saturating_sub(1));
        if self.shown.as_ref().is_none_or(|(i, _)| *i != self.page) {
            self.shown = sides.get(self.page).map(|s| (self.page, self.side_page(s, paint)));
        }
        let area = vec2((crate::widgets::dialog_room(ui.ctx()).x - 300.0).clamp(160.0, 460.0), self.height(ui.ctx()) - ui.spacing().interact_size.y - ui.spacing().item_spacing.y);
        let (rect, _) = ui.allocate_exact_size(area, egui::Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 4.0, crate::theme::workspace_bg(ui.visuals()));
        if let Some((_, page)) = &self.shown {
            let room = rect.shrink(16.0);
            let scale = (room.width() / page.size.x).min(room.height() / page.size.y);
            let r = Rect::from_center_size(room.center(), page.size * scale);
            p.rect_filled(r.translate(vec2(0.0, 2.0)).expand(1.0), 1.0, Color32::from_black_alpha(50));
            p.rect_filled(r, 0.0, Color32::WHITE);
            let t = TSTransform::new(r.min.to_vec2(), scale);
            for c in &page.shapes {
                let mut sh = c.shape.clone();
                sh.transform(t);
                p.with_clip_rect(t.mul_rect(c.clip_rect).intersect(r)).add(sh);
            }
            p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_gray(160)), egui::StrokeKind::Outside);
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(self.page > 0, egui::Button::new("<")).clicked() {
                self.page -= 1;
            }
            ui.label(trf("print.side_of", &[("n", &(self.page + 1).min(n)), ("total", &n)]));
            if ui.add_enabled(self.page + 1 < n, egui::Button::new(">")).clicked() {
                self.page += 1;
            }
        });
    }

    /// A side of paper as one page, its pages scaled into their cells
    /// (for the preview; the PDF places them with its own transforms).
    fn side_page(&self, side: &Side, paint: &mut Paint<'_>) -> Page {
        match side {
            Side::Page(p) => self.job.page(*p, paint).map(|p| p.into_owned()).unwrap_or(Page { size: vec2(794.0, 1123.0), shapes: Vec::new(), links: Vec::new(), tags: Vec::new() }),
            Side::Sheet { size, cells } => {
                let mut shapes = Vec::new();
                for (p, cell) in cells {
                    let Some(page) = p.and_then(|p| self.job.page(p, paint)) else { continue };
                    let t = crate::imposition::fit(*cell, page.size);
                    let frame = t.mul_rect(Rect::from_min_size(pos2(0.0, 0.0), page.size));
                    for c in &page.shapes {
                        let mut c = c.clone();
                        c.shape.transform(t);
                        c.clip_rect = t.mul_rect(c.clip_rect).intersect(frame);
                        shapes.push(c);
                    }
                    // The page's edge, as a light frame.
                    shapes.push(ClippedShape { clip_rect: Rect::EVERYTHING, shape: egui::Shape::rect_stroke(frame, 0.0, Stroke::new(0.5, Color32::from_gray(200)), egui::StrokeKind::Inside) });
                }
                Page { size: *size, shapes, links: Vec::new(), tags: Vec::new() }
            }
        }
    }
}

/// Prints a PDF: writes it to a temporary file and hands it to the
/// printer.
pub fn print(bytes: Vec<u8>, printer: &str, copies: u32, sides: Sides) -> Result<(), String> {
    let path = apex_io::safe_path::private_temp_dir("apexdesk-print", 0).map_err(|e| e.to_string())?.join("document").with_extension("pdf");
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let r = send_to_printer(printer, copies, sides, &path);
    // lp copies the file into the spool before returning; on Windows the
    // viewer opens it later, and the next print overwrites it.
    if !cfg!(windows) {
        std::fs::remove_file(&path).ok();
    }
    r
}

// --- page setup widgets ----------------------------------------------------------

const MM_PER_PT: f32 = 25.4 / 72.0;

/// Paper size choice (sizes in points, portrait width/height as given).
pub fn paper_ui(ui: &mut egui::Ui, w: &mut f32, h: &mut f32, landscape: bool) {
    crate::widgets::section(ui, tr("page.paper"));
    let (pw, ph) = if landscape { (*h, *w) } else { (*w, *h) };
    let current = PAPER_SIZES.iter().find(|(_, a, b)| (a - pw).abs() < 2.0 && (b - ph).abs() < 2.0);
    let label = |n: &str, a: f32, b: f32| format!("{n}  ({:.0} x {:.0} mm)", a * MM_PER_PT, b * MM_PER_PT);
    let text = match current {
        Some((n, a, b)) => label(n, *a, *b),
        None => format!("{}  ({:.0} x {:.0} mm)", tr("page.custom"), pw * MM_PER_PT, ph * MM_PER_PT),
    };
    let r = egui::ComboBox::from_id_salt("paper").width(300.0).selected_text(text).show_ui(ui, |ui| {
        for (n, a, b) in PAPER_SIZES {
            if ui.selectable_label(current.is_some_and(|c| c.0 == n), label(n, a, b)).clicked() {
                if landscape {
                    (*w, *h) = (b, a);
                } else {
                    (*w, *h) = (a, b);
                }
            }
        }
    });
    crate::widgets::combo_name(r, tr("page.paper"));
}

/// Portrait or landscape, as two pictures of a page.
pub fn orientation_ui(ui: &mut egui::Ui, landscape: &mut bool) {
    crate::widgets::section(ui, tr("page.orientation"));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (wide, key) in [(false, "page.portrait"), (true, "page.landscape")] {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(96.0, 84.0), egui::Sense::click());
            crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, tr(key), Some(*landscape == wide));
            let v = ui.visuals();
            let on = *landscape == wide;
            if on {
                ui.painter().rect_filled(rect, 5.0, v.selection.bg_fill.gamma_multiply(0.3));
                ui.painter().rect_stroke(rect, 5.0, egui::Stroke::new(1.5, v.selection.bg_fill), egui::StrokeKind::Inside);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 5.0, v.widgets.hovered.weak_bg_fill);
            }
            let size = if wide { egui::vec2(44.0, 32.0) } else { egui::vec2(32.0, 44.0) };
            let page = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.min.y + 32.0), size);
            ui.painter().rect_filled(page, 2.0, egui::Color32::WHITE);
            ui.painter().rect_stroke(page, 2.0, egui::Stroke::new(1.0, egui::Color32::GRAY), egui::StrokeKind::Inside);
            for k in 0..4 {
                let y = page.min.y + 7.0 + k as f32 * 5.0;
                ui.painter().hline(page.min.x + 5.0..=page.max.x - 5.0, y, egui::Stroke::new(1.5, egui::Color32::from_gray(190)));
            }
            ui.painter().text(egui::pos2(rect.center().x, rect.max.y - 12.0), egui::Align2::CENTER_CENTER, tr(key), egui::FontId::proportional(12.5), v.text_color());
            if resp.clicked() {
                *landscape = wide;
            }
        }
    });
}

/// Margin fields in centimeters (top, bottom, left, right, header, footer)
/// beside a picture of the page (`paper`: width and height as printed).
pub fn margins_ui(ui: &mut egui::Ui, m: [&mut f32; 6], paper: (f32, f32)) {
    crate::widgets::section(ui, tr("page.tab_margins"));
    let keys = ["page.top", "page.bottom", "page.left", "page.right", "page.header", "page.footer"];
    let mut vals = [0.0f32; 6];
    ui.horizontal_top(|ui| {
        egui::Grid::new("margins").num_columns(2).min_col_width(110.0).spacing([10.0, 8.0]).show(ui, |ui| {
            for (i, (v, key)) in m.into_iter().zip(keys).enumerate() {
                ui.label(tr(key));
                let mut cm = *v * MM_PER_PT / 10.0;
                if ui.add(egui::DragValue::new(&mut cm).range(0.0..=20.0).speed(0.05).fixed_decimals(2).suffix(" cm")).changed() {
                    *v = cm * 10.0 / MM_PER_PT;
                }
                vals[i] = *v;
                ui.end_row();
            }
        });
        ui.add_space(24.0);
        // The page with its margins, to scale.
        let (pw, ph) = paper;
        let k = 170.0 / pw.max(ph).max(1.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(180.0, 180.0), egui::Sense::hover());
        let page = egui::Rect::from_center_size(rect.center(), egui::vec2(pw * k, ph * k));
        let p = ui.painter();
        p.rect_filled(page.translate(egui::vec2(2.0, 2.0)), 1.0, egui::Color32::from_black_alpha(60));
        p.rect_filled(page, 1.0, egui::Color32::WHITE);
        let [top, bottom, left, right, header, footer] = vals;
        let body = egui::Rect::from_min_max(page.min + egui::vec2(left * k, top * k), page.max - egui::vec2(right * k, bottom * k));
        if body.is_positive() {
            p.rect_filled(body, 0.0, egui::Color32::from_gray(236));
            for y in (0..).map(|i| body.min.y + 6.0 + i as f32 * 6.0).take_while(|y| *y < body.max.y - 3.0) {
                p.hline(body.min.x + 4.0..=body.max.x - 4.0, y, egui::Stroke::new(1.0, egui::Color32::from_gray(200)));
            }
        }
        let dash = |a: egui::Pos2, b: egui::Pos2| p.extend(egui::Shape::dashed_line(&[a, b], egui::Stroke::new(1.0, crate::theme::ACCENT), 3.0, 2.0));
        dash(egui::pos2(page.min.x, body.min.y), egui::pos2(page.max.x, body.min.y));
        dash(egui::pos2(page.min.x, body.max.y), egui::pos2(page.max.x, body.max.y));
        dash(egui::pos2(body.min.x, page.min.y), egui::pos2(body.min.x, page.max.y));
        dash(egui::pos2(body.max.x, page.min.y), egui::pos2(body.max.x, page.max.y));
        let faint = egui::Stroke::new(1.0, egui::Color32::from_gray(160));
        p.hline(page.min.x + 3.0..=page.max.x - 3.0, page.min.y + header * k, faint);
        p.hline(page.min.x + 3.0..=page.max.x - 3.0, page.max.y - footer * k, faint);
    });
}

/// Header and footer editor: three parts each, field buttons and the
/// first-page / odd-even options.
pub fn hf_ui(ui: &mut egui::Ui, hf: &mut HeaderFooter, which: &mut usize, sheet: bool) {
    crate::widgets::section(ui, tr("page.hf_options"));
    ui.checkbox(&mut hf.different_first, tr("page.hf_first"));
    ui.checkbox(&mut hf.odd_even, tr("page.hf_odd_even"));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.selectable_value(which, 0, tr(if hf.odd_even { "page.hf_odd" } else { "page.hf_all" }));
        if hf.different_first {
            ui.selectable_value(which, 1, tr("page.hf_first_page"));
        }
        if hf.odd_even {
            ui.selectable_value(which, 2, tr("page.hf_even"));
        }
    });
    if (*which == 1 && !hf.different_first) || (*which == 2 && !hf.odd_even) {
        *which = 0;
    }
    let (header, footer) = match which {
        1 => (&mut hf.first_header, &mut hf.first_footer),
        2 => (&mut hf.even_header, &mut hf.even_footer),
        _ => (&mut hf.header, &mut hf.footer),
    };
    let fields: &[(&str, &str)] = if sheet {
        &[("page.field_page", "&P"), ("page.field_pages", "&N"), ("page.field_date", "&D"), ("page.field_time", "&T"), ("page.field_file", "&F"), ("page.field_sheet", "&A")]
    } else {
        &[("page.field_page", "&P"), ("page.field_pages", "&N"), ("page.field_date", "&D"), ("page.field_time", "&T"), ("page.field_file", "&F")]
    };
    for (t, key, id) in [(header, "page.header", "h"), (footer, "page.footer", "f")] {
        crate::widgets::section(ui, tr(key));
        hf_parts(ui, t, id);
    }
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tr("page.insert_field")).weak());
        for (key, code) in fields {
            if ui.small_button(tr(key)).on_hover_text(*code).clicked() {
                // Append to the part that had the focus last.
                let focus: Option<(String, usize)> = ui.data(|d| d.get_temp(egui::Id::new("hf-last-focus")));
                let (id, part) = focus.unwrap_or(("h".into(), 1));
                {
                    let t = match (id.as_str(), *which) {
                        ("h", 1) => &mut hf.first_header,
                        ("f", 1) => &mut hf.first_footer,
                        ("h", 2) => &mut hf.even_header,
                        ("f", 2) => &mut hf.even_footer,
                        ("h", _) => &mut hf.header,
                        _ => &mut hf.footer,
                    };
                    let s = match part {
                        0 => &mut t.left,
                        1 => &mut t.center,
                        _ => &mut t.right,
                    };
                    s.push_str(code);
                }
            }
        }
    });
}

fn hf_parts(ui: &mut egui::Ui, t: &mut HfText, id: &str) {
    ui.horizontal(|ui| {
        for (i, (s, key)) in [(&mut t.left, "page.left_part"), (&mut t.center, "page.center_part"), (&mut t.right, "page.right_part")].into_iter().enumerate() {
            ui.vertical(|ui| {
                ui.label(RichText::new(tr(key)).small().weak());
                let r = ui.add(egui::TextEdit::singleline(s).desired_width(150.0));
                if r.has_focus() || r.gained_focus() {
                    ui.data_mut(|d| d.insert_temp(egui::Id::new("hf-last-focus"), (id.to_string(), i)));
                }
            });
        }
    });
}

/// A booklet was turned on or off in `l`: it is printed on both sides,
/// turned on the short edge; turned off, the sides it changed come back
/// (`before`), unless they were changed since.
fn booklet_sides(l: &mut Layout, before: &mut Option<Sides>) {
    if l.booklet {
        if matches!(l.sides, Sides::One | Sides::LongEdge) {
            *before = Some(l.sides);
            l.sides = Sides::ShortEdge;
        }
    } else if let Some(s) = before.take() {
        if l.sides == Sides::ShortEdge {
            l.sides = s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("1-3, 5", 10), Some(vec![0, 1, 2, 4]));
        assert_eq!(parse_range("8-", 10), Some(vec![7, 8, 9]));
        assert_eq!(parse_range("-2", 10), Some(vec![0, 1]));
        assert_eq!(parse_range("12", 10), Some(vec![]));
        assert_eq!(parse_range("3-1", 10), None);
        assert_eq!(parse_range("x", 10), None);
    }

    // Booklet on and off again leaves the sides as they were.
    #[test]
    fn booklet_gives_the_sides_back() {
        let (mut l, mut before) = (Layout::default(), None);
        l.booklet = true;
        booklet_sides(&mut l, &mut before);
        assert_eq!(l.sides, Sides::ShortEdge);
        l.booklet = false;
        booklet_sides(&mut l, &mut before);
        assert_eq!(l, Layout::default());
        // Sides chosen meanwhile stay.
        l.booklet = true;
        booklet_sides(&mut l, &mut before);
        l.sides = Sides::LongEdge;
        l.booklet = false;
        booklet_sides(&mut l, &mut before);
        assert_eq!(l.sides, Sides::LongEdge);
    }
}
