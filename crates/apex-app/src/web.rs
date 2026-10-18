//! The browser version (crates/apex-web builds this program for it): the
//! same window drawn on a page's canvas. The browser keeps the files away
//! from the program, so it works on its own (`apex_fs`, in memory): files
//! come in uploaded or dropped on the page and go back out as downloads
//! when saved. Served by `apexdesk serve`, they are the user's files on
//! the server instead, read at the start and written back when saved.
//!
//! The settings live in the browser's storage between visits.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eframe::egui;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_time::{Duration, Instant, SystemTime};

#[path = "android/files.rs"]
pub mod files;
mod server;

/// The folder of the user's files.
pub const FILES: &str = "/files";
/// The fonts that come with the page (web/build.sh puts them there).
pub const FONTS: &str = "/fonts";
/// The settings' folder, kept in the browser's storage.
pub const CONFIG: &str = "/config/apexdesk";

/// Pictures in the file browser come from their format's icon here.
pub mod thumbs {
    use std::path::Path;

    use eframe::egui;

    pub const EXTS: &[&str] = &[];

    pub fn get(_ctx: &egui::Context, _path: &Path) -> Option<Option<(egui::TextureId, egui::Vec2)>> {
        None
    }

    pub fn forget_queue() {}
}

thread_local! {
    /// Files to open, dropped on the page.
    static OPENED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    /// The text of the last paste.
    static LAST_PASTE: RefCell<Option<String>> = const { RefCell::new(None) };
    /// When each of the user's files was last written, as last seen: one
    /// written since is saved out (downloaded, or sent to the server).
    static SEEN: RefCell<BTreeMap<PathBuf, SystemTime>> = const { RefCell::new(BTreeMap::new()) };
    /// The settings' files as last kept in the browser's storage.
    static KEPT: RefCell<BTreeMap<PathBuf, SystemTime>> = const { RefCell::new(BTreeMap::new()) };
    static LAST_CHECK: RefCell<Option<Instant>> = const { RefCell::new(None) };
    static CTX: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// Starts the program on the page's canvas (index.html calls it).
#[wasm_bindgen]
pub async fn start(canvas: web_sys::HtmlCanvasElement) -> Result<(), JsValue> {
    let started = Instant::now();
    std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&format!("ApexDesk: {info}").into())));
    locale();
    let _ = apex_fs::create_dir_all(FILES);
    let _ = apex_fs::create_dir_all(apex_fs::temp_dir());
    restore_config();
    load_fonts().await;
    // The server's files, when it serves the page.
    if server::connect().await {
        server::load(FILES).await;
        let _ = apex_query::sources::db::BRIDGE.set(server::db);
    }
    note_files();
    apex_io::picture::set_decoder(|b| crate::images::decode(b).map(|d| (d.width, d.height, d.rgba.to_vec())));
    let options = eframe::WebOptions::default();
    eframe::WebRunner::new()
        .start(canvas, options, Box::new(move |cc| {
            CTX.with_borrow_mut(|c| *c = Some(cc.egui_ctx.clone()));
            Ok(Box::new(crate::app::App::new(cc, Vec::new(), None, started)))
        }))
        .await
}

/// The fonts listed in fonts/index.txt next to the page, into [`FONTS`].
async fn load_fonts() {
    let Some(list) = server::get("fonts/index.txt").await else { return };
    for name in String::from_utf8_lossy(&list).lines().map(str::trim).filter(|n| !n.is_empty() && !n.contains(['/', '\\'])) {
        if let Some(b) = server::get(&format!("fonts/{name}")).await {
            let _ = apex_fs::write(Path::new(FONTS).join(name), b);
        }
    }
}

/// The browser's language ("es-ES") for the interface, the dictionary
/// and the numbers.
fn locale() {
    let lang = web_sys::window().and_then(|w| w.navigator().language()).unwrap_or_else(|| "en-US".into());
    let tag = lang.replace('-', "_");
    apex_i18n::set_system_locale(&tag);
    apex_sheet::Locale::set_system(&tag);
}

/// Each frame: the file browser, and what was saved goes out.
pub fn frame(ctx: &egui::Context) {
    files::show(ctx);
    let due = LAST_CHECK.with_borrow_mut(|t| {
        let due = t.is_none_or(|t| t.elapsed() >= Duration::from_millis(500));
        if due {
            *t = Some(Instant::now());
        }
        due
    });
    if due {
        save_out();
        keep_config();
    }
}

/// Before the program reads the frame's input: dropped files are read in
/// (they open once read), and a paste's text noted.
pub fn input(ctx: &egui::Context, raw: &mut egui::RawInput) {
    for e in &raw.events {
        if let egui::Event::Paste(t) = e {
            LAST_PASTE.with_borrow_mut(|p| *p = Some(t.clone()));
        }
    }
    for f in std::mem::take(&mut raw.dropped_files) {
        let Some(file) = f.web_file().cloned() else { continue };
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(p) = read_in(&file, Path::new(FILES)).await {
                OPENED.with_borrow_mut(|o| o.push(p));
                ctx.request_repaint();
            }
        });
    }
}

/// The files dropped on the page, read in since the last call.
pub fn opened() -> Vec<PathBuf> {
    OPENED.with_borrow_mut(std::mem::take)
}

/// The text of the last paste, to tell whether the program's own copy is
/// what was pasted.
pub fn last_paste() -> Option<String> {
    LAST_PASTE.with_borrow(Clone::clone)
}

/// A file of the computer into `dir`; its path there.
async fn read_in(file: &web_sys::File, dir: &Path) -> Option<PathBuf> {
    let buf = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await.ok()?;
    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
    let name = file.name().replace(['/', '\\'], "_");
    let path = dir.join(if name.is_empty() { "file".to_string() } else { name });
    apex_fs::write(&path, bytes).ok()?;
    // In: nothing to save out.
    seen(&path);
    if server::connected() {
        server::put(&path).await;
    }
    Some(path)
}

/// Asks for files of the computer (the browser's own dialog); they go to
/// `dir` and answer the open file browser.
pub fn upload(dir: &Path, exts: &[String], many: bool) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    let Ok(input) = doc.create_element("input") else { return };
    let Ok(input) = input.dyn_into::<web_sys::HtmlInputElement>() else { return };
    input.set_type("file");
    input.set_multiple(many);
    if !exts.is_empty() && !exts.iter().any(|e| e == "*") {
        input.set_accept(&exts.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(","));
    }
    let dir = dir.to_path_buf();
    let picked = input.clone();
    let on_change = Closure::once(move || {
        let Some(list) = picked.files() else { return };
        let files: Vec<web_sys::File> = (0..list.length()).filter_map(|i| list.get(i)).collect();
        wasm_bindgen_futures::spawn_local(async move {
            let mut paths = Vec::new();
            for f in &files {
                if let Some(p) = read_in(f, &dir).await {
                    paths.push(p);
                }
            }
            if let Some(ctx) = CTX.with_borrow(Clone::clone) {
                files::uploaded(&ctx, paths);
            }
        });
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();
    input.click();
}

/// A line above the file browser's list saying where saved files go.
pub fn files_note(ui: &mut egui::Ui, saving: bool) {
    if !saving {
        return;
    }
    let es = apex_i18n::current() == "es";
    let text = match (server::connected(), es) {
        (true, true) => "Se guarda en el servidor.",
        (true, false) => "Saved on the server.",
        (false, true) => "Al guardar, el navegador descarga el archivo.",
        (false, false) => "Saving downloads the file.",
    };
    ui.label(egui::RichText::new(text).weak());
}

/// Notes the user's files as they are now: none of them is due to go out.
fn note_files() {
    for (p, t) in user_files() {
        SEEN.with_borrow_mut(|s| s.insert(p, t));
    }
}

fn seen(path: &Path) {
    if let Ok(t) = apex_fs::metadata(path).and_then(|m| m.modified()) {
        SEEN.with_borrow_mut(|s| s.insert(path.to_path_buf(), t));
    }
}

/// The user's documents under [`FILES`] with when each was written, not
/// counting the program's own (lock files, half-written copies).
fn user_files() -> Vec<(PathBuf, SystemTime)> {
    let mut out = Vec::new();
    let mut dirs = vec![PathBuf::from(FILES)];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = apex_fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.starts_with("~$") || name.ends_with('~') || name.ends_with(".tmp") {
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                dirs.push(e.path());
            } else if let Ok(t) = m.modified() {
                out.push((e.path(), t));
            }
        }
    }
    out
}

/// Saved files go out: to the server, or downloaded.
fn save_out() {
    let now = user_files();
    let mut changed = Vec::new();
    SEEN.with_borrow_mut(|s| {
        for (p, t) in &now {
            if s.get(p) != Some(t) {
                s.insert(p.clone(), *t);
                changed.push(p.clone());
            }
        }
        // Removed ones are removed from the server too.
        let gone: Vec<PathBuf> = s.keys().filter(|p| !now.iter().any(|(q, _)| q == *p)).cloned().collect();
        for p in gone {
            s.remove(&p);
            if server::connected() {
                wasm_bindgen_futures::spawn_local(async move { server::delete(&p).await });
            }
        }
    });
    for p in changed {
        if server::connected() {
            wasm_bindgen_futures::spawn_local(async move { server::put(&p).await });
        } else if let Ok(bytes) = apex_fs::read(&p) {
            download(&p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), &bytes);
        }
    }
}

/// Gives `bytes` to the user as a download named `name`.
pub fn download(name: &str, bytes: &[u8]) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    let arr = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&arr);
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence(&parts) else { return };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else { return };
    if let Ok(a) = doc.create_element("a").and_then(|a| a.dyn_into::<web_sys::HtmlAnchorElement>().map_err(Into::into)) {
        a.set_href(&url);
        a.set_download(name);
        a.click();
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

/// Shows a PDF in a new tab of the browser, which prints it; downloaded
/// when the browser doesn't let the page open one.
pub fn show_pdf(bytes: &[u8]) {
    let arr = js_sys::Uint8Array::from(bytes);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/pdf");
    let shown = web_sys::Blob::new_with_u8_array_sequence_and_options(&js_sys::Array::of1(&arr), &opts)
        .ok()
        .and_then(|blob| web_sys::Url::create_object_url_with_blob(&blob).ok())
        .and_then(|url| web_sys::window()?.open_with_url_and_target(&url, "_blank").ok().flatten());
    if shown.is_none() {
        download("document.pdf", bytes);
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

/// The settings' files kept in the browser's storage at the last visit.
fn restore_config() {
    let _ = apex_fs::create_dir_all(CONFIG);
    let Some(st) = storage() else { return };
    let n = st.length().unwrap_or(0);
    for i in 0..n {
        let Ok(Some(key)) = st.key(i) else { continue };
        let Some(path) = key.strip_prefix("apexdesk:") else { continue };
        if !path.starts_with(CONFIG) {
            continue;
        }
        if let Ok(Some(v)) = st.get_item(&key)
            && let Some(bytes) = unstore(&v)
        {
            let _ = apex_fs::write(path, bytes);
        }
    }
    for (p, t) in config_files() {
        KEPT.with_borrow_mut(|k| k.insert(p, t));
    }
}

/// The settings' files (small ones: the storage holds a few megabytes).
fn config_files() -> Vec<(PathBuf, SystemTime)> {
    let mut out = Vec::new();
    let mut dirs = vec![PathBuf::from(CONFIG)];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = apex_fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                dirs.push(e.path());
            } else if m.len() <= 256 * 1024
                && let Ok(t) = m.modified()
            {
                out.push((e.path(), t));
            }
        }
    }
    out
}

/// Settings written since the last look go to the browser's storage.
fn keep_config() {
    let Some(st) = storage() else { return };
    let now = config_files();
    KEPT.with_borrow_mut(|k| {
        for (p, t) in &now {
            if k.get(p) != Some(t) {
                k.insert(p.clone(), *t);
                if let Ok(bytes) = apex_fs::read(p) {
                    let _ = st.set_item(&format!("apexdesk:{}", p.display()), &store(&bytes));
                }
            }
        }
        let gone: Vec<PathBuf> = k.keys().filter(|p| !now.iter().any(|(q, _)| q == *p)).cloned().collect();
        for p in gone {
            k.remove(&p);
            let _ = st.remove_item(&format!("apexdesk:{}", p.display()));
        }
    });
}

/// A file's bytes as the storage's text: as they are if text, else in hex.
fn store(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(t) => format!("t:{t}"),
        Err(_) => format!("x:{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()),
    }
}

fn unstore(v: &str) -> Option<Vec<u8>> {
    if let Some(t) = v.strip_prefix("t:") {
        return Some(t.as_bytes().to_vec());
    }
    let h = v.strip_prefix("x:")?.as_bytes();
    h.chunks(2).map(|c| std::str::from_utf8(c).ok().and_then(|s| u8::from_str_radix(s, 16).ok())).collect()
}
