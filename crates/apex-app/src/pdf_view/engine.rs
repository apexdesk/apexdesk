//! Drawing pages: one thread draws the pages asked for with ApexDesk's own
//! renderer ([`apex_io::pdf::render`]). Built with the `pdfium` feature (a
//! development tool, to compare against), when a copy of pdfium is there
//! (a development build's `target/pdfium`, made by `tools/pdfium-fetch.sh`,
//! `components/pdfium` next to the program, or `APEXDESK_PDFIUM` naming the
//! library or its folder), the thread loads it and draws with it instead.

#[cfg(feature = "pdfium")]
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};

/// A page to draw.
pub struct Job {
    /// Which document (a view's own number) and its revision.
    pub doc: u64,
    pub rev: u64,
    pub bytes: Arc<[u8]>,
    pub password: Option<String>,
    pub page: usize,
    /// Width in pixels.
    pub width: u32,
    /// Draw annotations and form fields (pdfium; ours always does).
    #[cfg_attr(not(feature = "pdfium"), allow(dead_code))]
    pub annots: bool,
    /// Draw it for the dark workspace (screen only).
    pub dark: bool,
    pub reply: Sender<Done>,
    pub ctx: Option<eframe::egui::Context>,
}

/// A drawn page: RGBA pixels.
pub struct Done {
    pub rev: u64,
    pub page: usize,
    pub width: u32,
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    /// Pixel boxes of the pictures drawn (kept in their colors when dark).
    pub images: Vec<[u32; 4]>,
    /// The pixels are drawn dark.
    pub dark: bool,
}

struct Engine {
    tx: Mutex<Sender<Job>>,
}

static ENGINE: OnceLock<Option<Engine>> = OnceLock::new();

/// The library's file name here.
#[cfg(feature = "pdfium")]
fn lib_name() -> &'static str {
    if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

/// Where the library is, if anywhere.
#[cfg(feature = "pdfium")]
pub fn library() -> Option<PathBuf> {
    let name = lib_name();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("APEXDESK_PDFIUM").map(PathBuf::from) {
        // "off" turns PDFium off (our own renderer draws the pages).
        if p.as_os_str() == "off" {
            return None;
        }
        if p.is_file() {
            return Some(p);
        }
        dirs.push(p.clone());
        dirs.push(p.join("lib"));
        dirs.push(p.join("bin"));
    }
    for d in crate::portable::install_dirs() {
        for sub in ["components/pdfium/lib", "components/pdfium/bin", "components/pdfium", ""] {
            dirs.push(d.join(sub));
        }
    }
    if cfg!(target_os = "linux") {
        dirs.push(PathBuf::from("/usr/lib/apexdesk/components/pdfium/lib"));
    }
    // In ApexDesk.app: Contents/Frameworks, beside Contents/MacOS.
    if cfg!(target_os = "macos")
        && let Some(d) = crate::portable::program_dir()
    {
        dirs.push(d.join("../Frameworks"));
    }
    // A build's own copy (target/pdfium beside target/debug, or its deps);
    // installed, the folders above the program's may be anyone's (C:\).
    if cfg!(debug_assertions)
        && let Some(d) = crate::portable::program_dir()
    {
        dirs.extend(d.ancestors().skip(1).take(2).map(|p| p.join("pdfium/lib")));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

fn engine() -> Option<&'static Engine> {
    ENGINE.get_or_init(|| {
        #[cfg(feature = "pdfium")]
        if let Some(path) = library() {
            let (tx, rx) = channel::<Job>();
            let (ok_tx, ok_rx) = channel::<bool>();
            if std::thread::Builder::new().name("pdfium".into()).spawn(move || worker(path, rx, ok_tx)).is_ok() && ok_rx.recv().unwrap_or(false) {
                return Some(Engine { tx: Mutex::new(tx) });
            }
        }
        let (tx, rx) = channel::<Job>();
        std::thread::Builder::new().name("pdf-render".into()).spawn(move || own_worker(rx)).ok()?;
        Some(Engine { tx: Mutex::new(tx) })
    }).as_ref()
}

/// Whether pages are drawn as pictures (by pdfium or our own renderer).
pub fn available() -> bool {
    engine().is_some()
}

/// Asks for a page; false when there is nothing to draw it.
pub fn request(job: Job) -> bool {
    match engine() {
        Some(e) => e.tx.lock().map(|tx| tx.send(job).is_ok()).unwrap_or(false),
        None => false,
    }
}

/// Draws a page and waits for it (printing, pictures).
pub fn render_now(bytes: Arc<[u8]>, password: Option<String>, page: usize, width: u32) -> Option<Done> {
    let (reply, rx) = channel();
    if !request(Job { doc: 0, rev: 0, bytes, password, page, width, annots: true, dark: false, reply, ctx: None }) {
        return None;
    }
    rx.recv().ok().filter(|d| d.width > 0)
}

#[cfg(feature = "pdfium")]
fn worker(path: PathBuf, rx: Receiver<Job>, ok: Sender<bool>) {
    use pdfium_render::prelude::*;
    let bindings = match Pdfium::bind_to_library(&path) {
        Ok(b) => b,
        Err(e) => {
            crate::diag::error(&format!("pdfium: {e:?}"));
            let _ = ok.send(false);
            return;
        }
    };
    let pdfium = Pdfium::new(bindings);
    let _ = ok.send(true);
    // The document drawn last, kept open for its next pages.
    let mut current: Option<((u64, u64, usize), PdfDocument)> = None;
    serve(rx, |job| {
        let key = (job.doc, job.rev, job.bytes.len());
        if job.doc == 0 || current.as_ref().is_none_or(|c| c.0 != key) {
            current = None;
            current = Some((key, pdfium.load_pdf_from_byte_vec(job.bytes.to_vec(), job.password.as_deref()).ok()?));
        }
        let (_, doc) = current.as_ref()?;
        let page = doc.pages().get(job.page as PdfPageIndex).ok()?;
        let cfg = PdfRenderConfig::new().set_target_width(job.width as Pixels).render_annotations(job.annots).render_form_data(job.annots).set_clear_color(PdfColor::WHITE);
        let bmp = page.render_with_config(&cfg).ok()?;
        let size = [bmp.width() as usize, bmp.height() as usize];
        let done = Done { rev: job.rev, page: job.page, width: job.width, size, rgba: bmp.as_rgba_bytes(), images: Vec::new(), dark: false };
        if job.doc == 0 {
            current = None;
        }
        Some(done)
    });
}

/// Without pdfium: pages drawn by our own renderer, with system fonts for
/// the fonts PDFs don't embed.
fn own_worker(rx: Receiver<Job>) {
    use apex_io::pdf::render::Document;
    // The document drawn last, kept open (with its fonts and pictures)
    // for its next pages.
    let mut current: Option<((u64, u64, usize), Document)> = None;
    serve(rx, |job| {
        let key = (job.doc, job.rev, job.bytes.len());
        // A page that makes the renderer panic is left blank (and the
        // document opened again); the thread stays to draw the others.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if job.doc == 0 || current.as_ref().is_none_or(|c| c.0 != key) {
                current = None;
                current = Some((key, Document::open(job.bytes.clone(), job.password.as_deref())?));
            }
            let (_, doc) = current.as_mut()?;
            doc.render(job.page, job.width, &super::sysfonts::font)
        }));
        if job.doc == 0 || r.is_err() {
            current = None;
        }
        let r = r.ok()??;
        Some(Done { rev: job.rev, page: job.page, width: job.width, size: [r.width as usize, r.height as usize], rgba: r.rgba, images: r.images, dark: false })
    });
}

/// Takes the jobs asked for, in order, skipping pages of revisions
/// already outdated, and answers each with what `draw` makes of it (an
/// empty page when it fails).
fn serve(rx: Receiver<Job>, mut draw: impl FnMut(&Job) -> Option<Done>) {
    let mut queue: Vec<Job> = Vec::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(j) => queue.push(j),
                Err(_) => return,
            }
        }
        queue.extend(rx.try_iter());
        // Pages of older revisions are not worth drawing.
        let newest: std::collections::HashMap<u64, u64> = queue.iter().fold(Default::default(), |mut m, j| {
            let e = m.entry(j.doc).or_insert(0);
            *e = (*e).max(j.rev);
            m
        });
        let job = queue.remove(0);
        if job.doc != 0 && newest.get(&job.doc).is_some_and(|&r| r > job.rev) {
            continue;
        }
        let mut done = draw(&job).unwrap_or(Done { rev: job.rev, page: job.page, width: 0, size: [0, 0], rgba: Vec::new(), images: Vec::new(), dark: false });
        if job.dark && !done.rgba.is_empty() {
            super::dark::darken_page(&mut done.rgba, done.size[0], &done.images);
            done.dark = true;
        }
        let _ = job.reply.send(done);
        if let Some(c) = &job.ctx {
            c.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page is drawn (by our own renderer, or pdfium with the `pdfium`
    /// feature and a copy of it).
    #[test]
    fn draws_a_page_when_there() {
        let pdf = apex_pdf_sample();
        match render_now(Arc::from(pdf), None, 0, 200) {
            Some(d) => {
                assert_eq!(d.size[0], 200);
                assert_eq!(d.rgba.len(), d.size[0] * d.size[1] * 4);
                // Some dark pixels: the text.
                assert!(d.rgba.chunks_exact(4).any(|p| p[0] < 100));
            }
            None => panic!("no page drawn"),
        }
    }

    fn apex_pdf_sample() -> Vec<u8> {
        let c = "BT /F1 48 Tf 20 700 Td (Hello) Tj ET";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 800] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offs = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offs.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let x = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offs {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }
}
