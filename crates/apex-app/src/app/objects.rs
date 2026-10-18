//! Objects embedded in a document or workbook, edited in a tab of their
//! own ("Object in Report.docx"): saving or closing it puts the object
//! back in its container, with a new preview.

use std::sync::Arc;

use apex_doc::{EmbedKind, Embedded};
use apex_io::{Format, Loaded};
use apex_sheet::Workbook;

use super::{App, Notice, View};
use crate::doc_view::DocView;
use crate::i18n::{tr, trf};
use crate::sheet_view::SheetView;

/// The object a tab edits: the tab holding it and the object as it was
/// opened (or last put back).
pub(super) struct EmbedLink {
    parent: u64,
    object: Arc<Embedded>,
    format: Format,
}

/// Puts `object` (and its preview) in place of the picture holding the
/// object with the same id; false when it is gone.
fn replace_in_workbook(wb: &mut Workbook, object: Embedded, preview: Option<Arc<[u8]>>) -> bool {
    for si in 0..wb.sheets.len() {
        let found = wb.sheets[si].images.iter().find(|i| i.object.as_ref().is_some_and(|o| o.id == object.id)).cloned();
        if let Some(mut img) = found {
            if let Some(p) = preview {
                img.data = p;
                img.format = apex_sheet::ImageFormat::Png;
            }
            img.object = Some(Arc::new(object));
            wb.set_image(si, img);
            return true;
        }
    }
    false
}

impl App {
    /// Opens an object of the active tab: workbooks and documents in a
    /// tab, other files with the system's program.
    pub(super) fn open_object(&mut self, obj: Arc<Embedded>) {
        let Some(parent) = self.tabs.get(self.active).map(|t| t.id) else { return };
        if let Some(i) = self.tabs.iter().position(|t| t.embed.as_ref().is_some_and(|e| e.parent == parent && e.object.id == obj.id)) {
            self.active = i;
            return;
        }
        if let Some(c) = obj.chart() {
            let Some((wb, si, _)) = apex_io::chart_object::open(&c.book) else { return };
            let mut v = SheetView::new(wb);
            v.select_sheet(si);
            let name = self.tabs[self.active].name();
            self.push_tab(View::Sheet(Box::new(v)), None, None);
            if let Some(t) = self.tabs.last_mut() {
                t.untitled = trf("chart.data_tab", &[("doc", &name)]);
                t.embed = Some(EmbedLink { parent, object: obj, format: Format::Xlsx });
            }
            if self.is_protected(parent) {
                self.protect_last();
            }
            return;
        }
        if obj.diagram().is_some() {
            match crate::diagram_ui::Editor::new(obj).filter(|_| !self.is_protected(parent)) {
                Some(e) => self.diagram_editor = Some((parent, e)),
                None => self.notify(Notice::info(tr("object.diagram"))),
            }
            return;
        }
        let Some(o) = obj.ole() else { return };
        let Some((data, fmt)) = apex_io::embed::openable(o) else {
            if let Err(e) = crate::objects::open_with_system(&obj) {
                self.notify(Notice::error(trf("object.open_failed", &[("error", &e)])));
            }
            return;
        };
        let view = match apex_io::embed::load(&data, fmt) {
            Ok(Loaded::Sheet(wb)) => View::Sheet(Box::new(SheetView::new(*wb))),
            Ok(Loaded::Doc(d)) => View::Doc(Box::new(DocView::new(*d))),
            Ok(Loaded::Slides(d)) => View::Slides(Box::new(crate::slides_view::SlidesView::new(*d))),
            Err(e) => {
                self.notify(Notice::error(trf("object.open_failed", &[("error", &e)])));
                return;
            }
        };
        let name = self.tabs[self.active].name();
        let downloaded = self.tabs.iter().find(|t| t.id == parent).is_some_and(super::Tab::downloaded);
        self.push_tab(view, None, None);
        if let Some(t) = self.tabs.last_mut() {
            t.untitled = trf("object.tab", &[("doc", &name)]);
            t.embed = Some(EmbedLink { parent, object: obj, format: fmt });
            t.downloaded = downloaded;
        }
        // What a document in Protected View carries is protected too.
        if self.is_protected(parent) {
            self.protect_last();
        }
    }

    /// Whether the tab edits an object whose container is still open.
    pub(super) fn edits_object(&self, idx: usize) -> bool {
        self.tabs.get(idx).and_then(|t| t.embed.as_ref()).is_some_and(|e| self.tabs.iter().any(|t| t.id == e.parent))
    }

    /// Puts the object a tab edits back in its container, with a new
    /// preview; false when it couldn't.
    pub(super) fn save_object(&mut self, idx: usize) -> bool {
        let ctx = self.ctx.clone();
        let Some(link) = self.tabs[idx].embed.as_ref() else { return false };
        let (parent, old, fmt) = (link.parent, link.object.clone(), crate::objects::save_format(link.format));
        if old.chart().is_some() {
            return self.save_chart_object(idx, parent, &old);
        }
        let Some(o) = old.ole() else { return false };
        let tab = &mut self.tabs[idx];
        let bytes = match &mut tab.view {
            View::Sheet(v) => {
                v.commit_edit();
                if fmt == Format::Ods { apex_io::ods::write(&v.wb) } else { apex_io::xlsx::write(&v.wb) }
            }
            View::Doc(v) => {
                v.end_hf_edit();
                v.update_for_output();
                if fmt == Format::Odt { apex_io::odt::write(&v.doc) } else { apex_io::docx::write(&v.doc) }
            }
            View::Slides(v) => {
                v.commit_edit();
                apex_io::pptx::write(&v.pres.deck)
            }
            View::Pdf(_) | View::Db(_) | View::Note(_) => return false,
        };
        let job = match &mut tab.view {
            View::Sheet(v) => v.print_job("", crate::print::Scope::ActiveSheet),
            View::Doc(v) => v.print_job(&ctx, ""),
            View::Slides(v) => v.print_job(&ctx, ""),
            View::Pdf(_) | View::Db(_) | View::Note(_) => return false,
        };
        let first = job.page(0, &mut |job, n| match &mut tab.view {
            View::Sheet(v) => v.paint_planned(&ctx, job.plan.as_ref()?, n),
            View::Doc(_) | View::Slides(_) => None,
            View::Pdf(_) | View::Db(_) | View::Note(_) => None,
        });
        let preview = first.and_then(|p| crate::objects::page_png(&ctx, &p)).map(Arc::from);
        let new = Embedded { id: old.id, kind: EmbedKind::Ole(apex_io::embed::replaced(o, bytes, fmt)) };
        let Some(pi) = self.tabs.iter().position(|t| t.id == parent) else { return false };
        // An object shown as an icon keeps its icon.
        let preview = preview.filter(|_| !o.icon);
        let put = match &mut self.tabs[pi].view {
            View::Doc(v) => v.doc.replace_object(new.clone(), preview),
            View::Sheet(v) => replace_in_workbook(&mut v.wb, new.clone(), preview),
            View::Slides(_) => false,
            View::Pdf(_) | View::Db(_) | View::Note(_) => false,
        };
        if !put {
            self.notify(Notice::error(tr("object.gone")));
            return false;
        }
        let tab = &mut self.tabs[idx];
        if let Some(l) = &mut tab.embed {
            l.object = Arc::new(new);
            l.format = fmt;
        }
        match &mut tab.view {
            View::Sheet(v) => v.wb.modified = false,
            View::Doc(v) => v.doc.modified = false,
            View::Slides(v) => v.pres.modified = false,
            View::Pdf(_) | View::Db(_) | View::Note(_) => {}
        }
        self.notify(Notice::info(tr("object.updated")));
        true
    }

    /// Puts the workbook of a chart back in its document: the chart is
    /// drawn from it (no preview).
    fn save_chart_object(&mut self, idx: usize, parent: u64, old: &Embedded) -> bool {
        let View::Sheet(v) = &mut self.tabs[idx].view else { return false };
        v.commit_edit();
        if !v.wb.sheets.iter().any(|s| !s.charts.is_empty()) {
            self.notify(Notice::error(tr("chart.data_no_chart")));
            return false;
        }
        let book = apex_io::chart_object::book(&v.wb);
        let new = Embedded { id: old.id, kind: EmbedKind::Chart(apex_doc::ChartObject { book }) };
        let Some(pi) = self.tabs.iter().position(|t| t.id == parent) else { return false };
        let put = match &mut self.tabs[pi].view {
            View::Doc(d) => d.doc.replace_object(new.clone(), None),
            View::Slides(s) => s.replace_chart(&new),
            View::Sheet(_) => false,
            View::Pdf(_) | View::Db(_) | View::Note(_) => false,
        };
        if !put {
            self.notify(Notice::error(tr("object.gone")));
            return false;
        }
        let tab = &mut self.tabs[idx];
        if let Some(l) = &mut tab.embed {
            l.object = Arc::new(new);
        }
        if let View::Sheet(v) = &mut tab.view {
            v.wb.modified = false;
        }
        self.notify(Notice::info(tr("object.updated")));
        true
    }

    /// The dialog editing a diagram; what it makes goes back in its
    /// document or workbook, as one undo step.
    pub(super) fn diagram_dialog(&mut self) {
        let ctx = self.ctx.clone();
        let Some((parent, e)) = &mut self.diagram_editor else { return };
        let Some(done) = e.ui(&ctx) else { return };
        let parent = *parent;
        self.diagram_editor = None;
        let Some(new) = done else { return };
        let put = match self.tabs.iter_mut().find(|t| t.id == parent).map(|t| &mut t.view) {
            Some(View::Doc(d)) => d.doc.replace_object(new, None),
            Some(View::Sheet(s)) => replace_in_workbook(&mut s.wb, new, None),
            _ => false,
        };
        if !put {
            self.notify(Notice::error(tr("object.gone")));
        }
        ctx.request_repaint();
    }

    /// Asks the views for objects to open.
    pub(super) fn object_requests(&mut self) {
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        let req = match &mut t.view {
            View::Doc(v) => v.take_object_request(),
            View::Sheet(v) => v.take_object_request(),
            View::Slides(v) => v.take_object_request(),
            View::Pdf(_) | View::Db(_) | View::Note(_) => None,
        };
        match req {
            Some(crate::objects::Request::Open(o)) => self.open_object(o),
            None => {}
        }
    }
}
