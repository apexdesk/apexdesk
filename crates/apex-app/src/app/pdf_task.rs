//! PDF export and printing a few pages per frame: the pages of a sheet
//! are painted, written and dropped one at a time (text laid out in a
//! frame stays in egui's cache until the next one), with a progress
//! window that can cancel, so that a sheet of thousands of pages neither
//! freezes the window nor fills the memory. Printing on both sides by
//! hand prints the odd sides, waits for the user to turn the paper over,
//! then prints the even ones.

use std::path::PathBuf;
use web_time::{Duration, Instant};

use eframe::egui;

use super::{App, Notice, View};
use crate::i18n::{tr, trf};
use crate::imposition::{Layout, Side, Sides};
use crate::pdf_out::Out;
use crate::print::{Job, Page};
use crate::widgets::FitWindow as _;

/// Where the PDF goes.
pub(super) enum PdfDest {
    File(PathBuf),
    Printer { printer: String, copies: u32, sides: Sides },
}

pub(super) struct PdfTask {
    /// Tab the job comes from.
    tab: u64,
    job: Job,
    sides: Vec<Side>,
    /// The size of the job's pages, for sheets holding several.
    paper: egui::Vec2,
    /// Sides written so far.
    done: usize,
    out: Option<Out>,
    dest: PdfDest,
    /// The even sides, printed after the user turns the paper over.
    second: Option<Vec<Side>>,
    /// Waiting for the paper to be turned over.
    waiting: bool,
}

/// Time spent painting pages in each frame.
const SLICE: Duration = Duration::from_millis(40);

impl App {
    /// Page `n` of a job of tab `idx`, painted now when the job is planned.
    pub(super) fn paint_page(tabs: &mut [super::Tab], ctx: &egui::Context, idx: usize, job: &Job, n: usize) -> Option<Page> {
        match &mut tabs.get_mut(idx)?.view {
            View::Sheet(v) => v.paint_planned(ctx, job.plan.as_ref()?, n),
            View::Doc(_) | View::Slides(_) => None,
            View::Pdf(_) | View::Db(_) | View::Note(_) => None,
        }
    }

    /// The whole PDF of `pages` of a job of tab `idx` at once (small jobs:
    /// signing, where the file is needed right away).
    pub(super) fn job_pdf(&mut self, ctx: &egui::Context, idx: usize, job: &Job, pages: &[usize]) -> Vec<u8> {
        let tabs = &mut self.tabs;
        crate::pdf_out::to_pdf(job, pages, &mut |job, n| Self::paint_page(tabs, ctx, idx, job, n))
    }

    /// Starts writing `pages` of a job of tab `idx` (see [`App::pdf_task`]).
    pub(super) fn start_pdf(&mut self, idx: usize, job: Job, pages: Vec<usize>, dest: PdfDest) {
        let sides = pages.into_iter().map(Side::Page).collect();
        self.start_sides(idx, job, sides, egui::Vec2::ZERO, dest, None);
    }

    /// Starts writing `pages` laid out on paper by `layout`.
    pub(super) fn start_layout(&mut self, ctx: &egui::Context, idx: usize, job: Job, pages: Vec<usize>, layout: Layout, dest: PdfDest) {
        let (sides, paper) = if layout.is_plain() {
            (pages.into_iter().map(Side::Page).collect(), egui::Vec2::ZERO)
        } else {
            let tabs = &mut self.tabs;
            let paper = pages.first().and_then(|&p| job.page(p, &mut |job, n| Self::paint_page(tabs, ctx, idx, job, n))).map_or(egui::vec2(794.0, 1123.0), |p| p.size);
            (crate::imposition::sides(&pages, layout, paper), paper)
        };
        match &dest {
            PdfDest::Printer { sides: Sides::Manual, .. } if sides.len() > 1 => {
                let (odd, even) = crate::imposition::manual_passes(sides);
                self.start_sides(idx, job, odd, paper, dest, Some(even));
            }
            _ => self.start_sides(idx, job, sides, paper, dest, None),
        }
    }

    fn start_sides(&mut self, idx: usize, job: Job, sides: Vec<Side>, paper: egui::Vec2, dest: PdfDest, second: Option<Vec<Side>>) {
        let Some(tab) = self.tabs.get(idx) else { return };
        let out = Out::with_sides(&job, &sides, paper);
        self.pdf_task = Some(PdfTask { tab: tab.id, job, sides, paper, done: 0, out: Some(out), dest, second, waiting: false });
    }

    /// Writes the pages of the running export for a slice of the frame,
    /// shows its progress and, at the end, saves or prints the PDF.
    pub(super) fn pdf_task(&mut self, ctx: &egui::Context) {
        let Some(task) = &mut self.pdf_task else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == task.tab) else {
            self.pdf_task = None;
            return;
        };
        if task.waiting {
            return self.turn_paper(ctx, idx);
        }
        let start = Instant::now();
        if let Some(out) = &mut task.out {
            while task.done < task.sides.len() && start.elapsed() < SLICE {
                let side = &task.sides[task.done];
                out.side(&task.job, side, &mut |job, n| Self::paint_page(&mut self.tabs, ctx, idx, job, n));
                task.done += 1;
            }
        }
        if task.done < task.sides.len() {
            let mut cancel = false;
            let (done, total) = (task.done, task.sides.len());
            egui::Window::new(tr("print.export_pdf")).id(egui::Id::new("pdf-task")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).show_fit(ctx, |ui| {
                ui.set_width(320.0);
                ui.label(trf("print.page_of", &[("n", &(done + 1)), ("total", &total)]));
                ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32));
                cancel = ui.button(tr("dialog.cancel")).clicked();
            });
            if cancel {
                self.pdf_task = None;
            } else {
                ctx.request_repaint();
            }
            return;
        }
        let Some(out) = task.out.take() else { return };
        let bytes = out.finish(&task.job);
        let result = match &task.dest {
            PdfDest::File(path) => match crate::common::write_out(path, bytes) {
                Ok(()) => Notice::info(trf("print.exported", &[("file", &path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())])),
                Err(e) => Notice::error(trf("error.save", &[("error", &e.to_string())])),
            },
            PdfDest::Printer { printer, copies, sides } => match crate::print::print(bytes, printer, *copies, *sides) {
                Ok(()) => Notice::info(trf("print.sent", &[("printer", printer)])),
                Err(e) => Notice::error(trf("print.failed", &[("error", &e)])),
            },
        };
        let failed = result.error;
        let wait = task.second.is_some() && !failed;
        self.notify(result);
        match &mut self.pdf_task {
            Some(task) if wait => task.waiting = true,
            _ => self.pdf_task = None,
        }
    }

    /// Printing on both sides by hand: asks the user to turn the printed
    /// stack over, then prints the even sides.
    fn turn_paper(&mut self, ctx: &egui::Context, idx: usize) {
        let mut go = None;
        egui::Window::new(tr("print.turn_title")).id(egui::Id::new("pdf-turn")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).show_fit(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(tr("print.turn_text"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(egui::RichText::new(tr("print.turn_continue")).strong()).clicked() {
                    go = Some(true);
                }
                if ui.button(tr("dialog.cancel")).clicked() {
                    go = Some(false);
                }
            });
        });
        match go {
            Some(true) => {
                let Some(task) = self.pdf_task.take() else { return };
                let sides = task.second.unwrap_or_default();
                self.start_sides(idx, task.job, sides, task.paper, task.dest, None);
            }
            Some(false) => self.pdf_task = None,
            None => {}
        }
    }
}
