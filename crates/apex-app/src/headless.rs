//! Printed pages without a window, for the command line and the terminal
//! interface: the views paint their pages exactly as for the print dialog
//! and `pdf_out` writes them, with the same fonts as the window.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use web_time::{Duration, Instant};

use apex_tui::Book;
use eframe::egui;

use crate::print::{Job, Scope};

/// A context with the system fonts, shared by every job of the process.
pub(crate) fn context() -> &'static egui::Context {
    static CTX: OnceLock<egui::Context> = OnceLock::new();
    CTX.get_or_init(|| {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        // Documents name their fonts: wait for the scan of the families.
        let start = Instant::now();
        while crate::fonts::families().is_empty() && start.elapsed() < Duration::from_secs(20) {
            apex_fs::thread::sleep(Duration::from_millis(10));
        }
        ctx
    })
}

static PDFA: AtomicBool = AtomicBool::new(false);

/// Makes the PDFs of this process PDF/A (`apexdesk convert --pdfa`).
pub fn set_pdfa(on: bool) {
    PDFA.store(on, Ordering::Relaxed);
}

/// PDF options without a window: documents tagged, as Word does.
fn options(doc: bool) -> crate::print::PdfOptions {
    crate::print::PdfOptions { pdfa: PDFA.load(Ordering::Relaxed), tagged: doc }
}

/// One frame: fonts requested by the last layout become usable.
fn frame(ctx: &egui::Context) {
    ctx.run_ui(Default::default(), |ui| crate::fonts::apply(ui.ctx())).textures_delta.clear();
}

/// The printed pages of a document.
fn doc_job(doc: &apex_doc::Document, name: &str) -> Job {
    let ctx = context();
    let paint = || {
        let mut v = crate::doc_view::DocView::new(doc.clone());
        v.update_for_output();
        v.print_job(ctx, name)
    };
    frame(ctx);
    frame(ctx);
    let mut job = paint();
    // Fonts first used by this file load in the next frames; lay out again
    // until nothing changes.
    for _ in 0..3 {
        let generation = crate::fonts::generation();
        frame(ctx);
        frame(ctx);
        if crate::fonts::generation() == generation {
            break;
        }
        job = paint();
    }
    job
}

/// A presentation as PDF: one page per slide (or notes pages or handouts,
/// by `what`), fonts loaded as for documents.
pub fn slides_pdf(deck: &apex_slides::Deck, name: &str, what: Scope) -> Vec<u8> {
    let ctx = context();
    let paint = || crate::slides_view::SlidesView::new(deck.clone()).print_job_as(ctx, name, what);
    frame(ctx);
    frame(ctx);
    let mut job = paint();
    for _ in 0..3 {
        let generation = crate::fonts::generation();
        frame(ctx);
        frame(ctx);
        if crate::fonts::generation() == generation {
            break;
        }
        job = paint();
    }
    job.pdf = options(false);
    let pages: Vec<usize> = (0..job.len()).collect();
    crate::pdf_out::to_pdf(&job, &pages, &mut crate::print::no_paint)
}

/// The slides as PNG pictures `width` pixels wide.
pub fn slides_png(deck: &apex_slides::Deck, width: u32) -> Vec<Vec<u8>> {
    let ctx = context();
    frame(ctx);
    frame(ctx);
    let mut out = Vec::new();
    for i in 0..deck.slides.len() {
        // A first paint asks for the fonts; paint again once they load.
        let _ = crate::slides_view::handouts::slide_png(ctx, deck, i, 8);
        frame(ctx);
        frame(ctx);
        out.push(crate::slides_view::handouts::slide_png(ctx, deck, i, width).unwrap_or_default());
    }
    out
}

/// The whole file as PDF.
pub fn pdf(book: Book<'_>, name: &str) -> Vec<u8> {
    match book {
        Book::Sheet(wb) => sheet_pdf(crate::sheet_view::SheetView::new(wb.clone()), name),
        Book::Doc(doc) => {
            let mut job = doc_job(doc, name);
            job.pdf = options(true);
            let pages: Vec<usize> = (0..job.len()).collect();
            crate::pdf_out::to_pdf(&job, &pages, &mut crate::print::no_paint)
        }
    }
}

/// Every visible sheet as PDF. The pages are painted, written and
/// dropped one at a time, with frames in between: they empty egui's cache
/// of laid out text (else it would hold the text of every page) and load
/// the fonts a page asks for, and then the page is painted again.
pub fn sheet_pdf(mut v: crate::sheet_view::SheetView, name: &str) -> Vec<u8> {
    let ctx = context();
    frame(ctx);
    frame(ctx);
    let mut job = v.print_job(name, Scope::Workbook);
    job.pdf = options(false);
    let pages: Vec<usize> = (0..job.len()).collect();
    let mut out = crate::pdf_out::Out::new(&job, &pages);
    let Some(plan) = &job.plan else { return out.finish(&job) };
    for &n in &pages {
        let mut page = v.paint_planned(ctx, plan, n);
        for _ in 0..3 {
            let generation = crate::fonts::generation();
            frame(ctx);
            if crate::fonts::generation() == generation {
                break;
            }
            frame(ctx);
            page = v.paint_planned(ctx, plan, n);
        }
        if let Some(p) = page {
            out.page(&p);
        }
    }
    out.finish(&job)
}

/// Number of printed pages. Sheets are only split into pages, not painted.
pub fn pages(book: Book<'_>) -> usize {
    match book {
        Book::Sheet(wb) => {
            let visible: Vec<usize> = (0..wb.sheets.len()).filter(|&i| !wb.sheets[i].hidden).collect();
            crate::sheet_view::PrintPlan::new(wb, &visible, None, "").count()
        }
        Book::Doc(doc) => doc_job(doc, "").pages.len(),
    }
}

/// Where each printed page of a document starts (see
/// `DocView::page_starts`): one entry per page.
pub fn page_starts(doc: &apex_doc::Document) -> Vec<(usize, bool)> {
    // Laid out once as for printing, so that the fonts it uses are loaded.
    doc_job(doc, "");
    let mut v = crate::doc_view::DocView::new(doc.clone());
    v.update_for_output();
    v.page_starts(context())
}

/// `Hooks::pdf` of the terminal interface.
pub fn write_pdf(book: Book<'_>, path: &std::path::Path) -> Result<(), String> {
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    apex_io::write_atomic(path, &pdf(book, &name)).map_err(|e| e.to_string())
}


#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::{CellRef, Content, Value, Workbook};

    /// A sheet's print job holds a plan, not painted pages: pages are
    /// painted one at a time as the PDF is written, so memory does not
    /// grow with the number of pages.
    #[test]
    fn sheet_pages_are_painted_one_at_a_time() {
        let mut wb = Workbook::empty();
        wb.sheets.push(apex_sheet::Sheet::new("S"));
        for r in 0..600 {
            for c in 0..12 {
                wb.load_cell(0, CellRef::new(r, c), Content::Value(Value::Number((r * 12 + c) as f64)), 0);
            }
        }
        wb.finish_load();
        let counted = pages(Book::Sheet(&wb));
        let mut v = crate::sheet_view::SheetView::new(wb);
        let job = v.print_job("big", Scope::Workbook);
        assert!(counted > 10, "{counted} pages");
        assert_eq!(job.len(), counted);
        assert!(job.pages.is_empty(), "nothing painted in advance");
        let plan = job.plan.as_ref().unwrap();
        let ctx = context();
        frame(ctx);
        let last = v.paint_planned(ctx, plan, counted - 1).expect("last page");
        assert!(!last.shapes.is_empty());
        assert!(v.paint_planned(ctx, plan, counted).is_none());
        let pdf = sheet_pdf(v, "big");
        let written = pdf.windows(12).filter(|w| w == b"/Type /Page ").count();
        assert_eq!(written, counted);
    }

    /// The dark workspace is only for the screen: printed pages (and so
    /// PDFs) of sheets, documents and slides come out the same with it on,
    /// while the screen's painting goes dark.
    #[test]
    fn dark_workspace_leaves_print_alone() {
        let mut wb = Workbook::empty();
        wb.sheets.push(apex_sheet::Sheet::new("S"));
        for r in 0..5 {
            wb.load_cell(0, CellRef::new(r, 0), Content::Value(Value::Text(format!("Row {r}").into())), 0);
        }
        wb.finish_load();
        let doc = apex_doc::Document::from_paragraphs(vec![apex_doc::Paragraph::new("Some dark text")]);
        let deck = apex_slides::build::new_deck(0);
        // A context of its own: painting pages over and over on the shared
        // one would take other tests' pages from its capture layer.
        context();
        let own_ctx = egui::Context::default();
        crate::fonts::install(&own_ctx);
        let ctx = &own_ctx;
        frame(ctx);
        let printed = || {
            let mut v = crate::sheet_view::SheetView::new(wb.clone());
            let job = v.print_job("s", Scope::Workbook);
            let sheet = v.paint_planned(ctx, job.plan.as_ref().unwrap(), 0).expect("sheet page");
            let mut d = crate::doc_view::DocView::new(doc.clone());
            d.update_for_output();
            let doc = d.print_job(ctx, "d");
            let slides = crate::slides_view::SlidesView::new(deck.clone()).print_job(ctx, "p");
            let mut out = format!("{:?}", sheet.shapes);
            for p in doc.pages.iter().chain(&slides.pages) {
                out.push_str(&format!("{:?}", p.shapes));
            }
            assert!(!doc.pages.is_empty());
            // Only the colors: where glyphs sit in the font atlas changes
            // as other tests lay out text.
            out.split('#').skip(1).map(|s| s.get(..11).unwrap_or(s)).collect::<Vec<_>>().join(" ")
        };
        // Other tests share the context and load fonts meanwhile: compare
        // once the light pages come out the same before and after.
        let mut same = false;
        for _ in 0..10 {
            crate::theme::set_dark_canvas(false, true);
            let light = printed();
            crate::theme::set_dark_canvas(true, true);
            let dark = printed();
            crate::theme::set_dark_canvas(false, true);
            if printed() == light {
                assert!(light == dark, "the dark workspace changed the printed pages");
                same = true;
                break;
            }
            frame(ctx);
        }
        assert!(same, "the printed pages never settled");
        // The screen's painting does change (on a context of its own: the
        // capture layer of the shared one is busy with other tests).
        let own = egui::Context::default();
        crate::theme::set_dark_canvas(true, true);
        let screen = crate::print::capture(&own, |p| crate::theme::dark_paint(p, || {
            p.rect_filled(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(10.0, 10.0)), 0.0, egui::Color32::WHITE);
        }));
        assert!(format!("{screen:?}").contains("fill: #1F_1F_1F_FF"), "{screen:?}");
        crate::theme::set_dark_canvas(false, false);
    }
}
