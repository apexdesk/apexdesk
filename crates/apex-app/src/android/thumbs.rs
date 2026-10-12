//! Thumbnails of the pictures in the file browser: decoded and shrunk on a
//! background thread, the latest asked for first, and kept for the most
//! recently drawn files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::SystemTime;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

/// Thumbnails are shrunk to fit this many pixels a side.
const SIDE: u32 = 96;
/// Thumbnails kept (the least recently drawn go first).
const KEEP: usize = 200;
/// Files waiting to be decoded (the oldest asks are forgotten).
const QUEUE: usize = 64;
/// Larger files, and pictures of more pixels, get no thumbnail (memory).
const MAX_FILE: u64 = 32 << 20;
const MAX_PIXELS: u64 = 48_000_000;

/// The extensions that get a thumbnail.
pub const EXTS: &[&str] = &["png", "jpg", "jpeg", "jpe", "jfif", "gif", "bmp", "webp"];

/// A file as it was when its thumbnail was made: a changed file is decoded
/// again.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
}

enum Thumb {
    /// Decoded, not yet a texture (textures are made on the UI thread).
    Decoded(ColorImage),
    Ready(TextureHandle),
    /// Not a picture we read, too large, or broken.
    Failed,
}

#[derive(Default)]
struct Cache {
    done: HashMap<Key, (Thumb, u64)>,
    /// Asked for, not yet decoded: the last is decoded first.
    queue: Vec<Key>,
    /// Counts the lookups, which entries keep to say when they were drawn.
    clock: u64,
    ctx: Option<egui::Context>,
    worker: bool,
}

static CACHE: LazyLock<(Mutex<Cache>, Condvar)> = LazyLock::new(Default::default);

fn lock() -> std::sync::MutexGuard<'static, Cache> {
    CACHE.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// The thumbnail of `path`: `Some(None)` while it is being made, `None`
/// when there is none (the caller draws the format's icon).
pub fn get(ctx: &egui::Context, path: &Path) -> Option<Option<(egui::TextureId, egui::Vec2)>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    let key = Key { path: path.to_path_buf(), modified: meta.modified().ok(), len: meta.len() };
    let mut c = lock();
    c.clock += 1;
    let now = c.clock;
    if let Some((thumb, used)) = c.done.get_mut(&key) {
        *used = now;
        if let Thumb::Decoded(img) = thumb {
            let img = std::mem::replace(img, ColorImage::default());
            *thumb = Thumb::Ready(ctx.load_texture(format!("thumb:{}", path.display()), img, TextureOptions::LINEAR));
        }
        return match thumb {
            Thumb::Ready(t) => Some(Some((t.id(), t.size_vec2()))),
            _ => None,
        };
    }
    if let Some(i) = c.queue.iter().position(|k| k == &key) {
        // Drawn again: to the front.
        let k = c.queue.remove(i);
        c.queue.push(k);
    } else {
        c.queue.push(key);
        if c.queue.len() > QUEUE {
            c.queue.remove(0);
        }
    }
    c.ctx.get_or_insert_with(|| ctx.clone());
    if !c.worker {
        c.worker = std::thread::Builder::new().name("thumbnails".into()).spawn(work).is_ok();
    }
    CACHE.1.notify_one();
    Some(None)
}

/// Forgets the files asked for and not yet decoded (another folder).
pub fn forget_queue() {
    lock().queue.clear();
}

fn work() {
    loop {
        let key = {
            let mut c = lock();
            loop {
                if let Some(k) = c.queue.pop() {
                    break k;
                }
                c = CACHE.1.wait(c).unwrap_or_else(|e| e.into_inner());
            }
        };
        // A broken file must not take the thread with it.
        let thumb = std::panic::catch_unwind(|| decode(&key.path)).ok().flatten().map_or(Thumb::Failed, Thumb::Decoded);
        let mut c = lock();
        c.clock += 1;
        let now = c.clock;
        c.done.insert(key, (thumb, now));
        while c.done.len() > KEEP {
            let Some(oldest) = c.done.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k.clone()) else { break };
            c.done.remove(&oldest);
        }
        if let Some(ctx) = &c.ctx {
            ctx.request_repaint();
        }
    }
}

fn decode(path: &Path) -> Option<ColorImage> {
    let bytes = std::fs::read(path).ok().filter(|b| b.len() as u64 <= MAX_FILE)?;
    let (w, h) = apex_sheet::image::pixel_size(&bytes)?;
    if w as u64 * h as u64 > MAX_PIXELS {
        return None;
    }
    let d = crate::images::decode(&bytes)?;
    drop(bytes);
    let (w, h, rgba) = crate::images::fit(d.width, d.height, d.rgba.to_vec(), SIDE, u64::MAX);
    (rgba.len() == w as usize * h as usize * 4).then(|| ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
}
