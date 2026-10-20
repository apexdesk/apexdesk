//! Pictures shared by the spreadsheet and document views: decoding, GPU
//! textures and painting.
//!
//! Typical use from a view, every frame:
//! ```ignore
//! let key = images::key(&bytes);                       // memoized per Arc
//! let tex = images::texture(ctx, key, &bytes, false);  // None until decoded
//! images::paint(&painter, tex.as_ref(), rect, rotation, crop);
//! ```
//! Large pictures decode on a background thread; `paint` shows a gray box
//! meanwhile and the view is repainted when the texture is ready. Printing
//! passes `wait = true` so pages never show the placeholder. The PDF writer
//! finds the file behind a painted texture with `source`.
//!
//! Textures not used for a while are dropped (and decoded again if needed).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};
use web_time::{Duration, Instant};

use apex_doc::PicFx;
use apex_sheet::ImageFormat;
use eframe::egui::{Color32, Context, Mesh, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, TextureHandle, TextureId, TextureOptions, epaint::Vertex, pos2, vec2};

/// Decoded pictures are shrunk to fit these.
const MAX_SIDE: u32 = 8000;
const MAX_PIXELS: u64 = 64_000_000;
/// Larger files are not decoded at all (memory).
const MAX_SOURCE_PIXELS: u64 = 400_000_000;
/// Textures are shrunk further: screens need no more.
const MAX_TEXTURE_SIDE: u32 = 4096;
/// Pictures up to this many pixels decode on the UI thread.
const SYNC_PIXELS: u64 = 1 << 20;
/// Unused textures are dropped after this.
const IDLE: Duration = Duration::from_secs(120);

/// A decoded picture: RGBA8, not premultiplied, row major.
#[derive(Clone)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

/// Decodes PNG, JPEG, GIF (first frame), BMP, WebP, TIFF, or draws an EMF or
/// WMF picture (with the system's fonts). Pictures over
/// 8000 px a side or 64 Mpx come back shrunk. `None` for other formats
/// and broken files.
pub fn decode(bytes: &[u8]) -> Option<Decoded> {
    let (w, h, rgba) = match ImageFormat::sniff(bytes)? {
        ImageFormat::Png => decode_png(bytes)?,
        ImageFormat::Jpeg => decode_jpeg(bytes)?,
        ImageFormat::Gif => decode_gif(bytes)?,
        ImageFormat::Bmp => decode_bmp(bytes)?,
        ImageFormat::Webp => decode_webp(bytes)?,
        ImageFormat::Tiff => decode_tiff(bytes)?,
        ImageFormat::Emf | ImageFormat::Wmf => decode_metafile(bytes)?,
        _ => return None,
    };
    if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
        return None;
    }
    let (w, h, rgba) = fit(w, h, rgba, MAX_SIDE, MAX_PIXELS);
    Some(Decoded { width: w, height: h, rgba: rgba.into() })
}

/// Metafiles are drawn at 3 pixels to the point (216 dpi).
fn decode_metafile(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let p = apex_io::metafile::render(bytes, 3.0, &crate::pdf_view::sysfonts::font)?;
    Some((p.width, p.height, p.rgba))
}

fn too_big(w: u32, h: u32) -> bool {
    w as u64 * h as u64 > MAX_SOURCE_PIXELS
}

fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: 1 << 31 });
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().ok()?;
    let (w, h) = (reader.info().width, reader.info().height);
    if too_big(w, h) {
        return None;
    }
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let n = w as usize * h as usize;
    let px = buf.get(..info.buffer_size())?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    (rgba.len() == n * 4).then_some((w, h, rgba))
}

fn decode_jpeg(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    let (w, h) = apex_sheet::image::pixel_size(bytes)?;
    if too_big(w, h) {
        return None;
    }
    let opts = DecoderOptions::default()
        .set_max_width(1 << 16)
        .set_max_height(1 << 16)
        .jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(bytes), opts);
    // The size the decoder goes by (a header can read otherwise).
    dec.decode_headers().ok()?;
    let info = dec.info()?;
    if too_big(info.width.into(), info.height.into()) {
        return None;
    }
    let rgba = dec.decode().ok()?;
    let info = dec.info()?;
    Some((info.width.into(), info.height.into(), rgba))
}

/// A WebP picture (its first frame if animated), as RGBA.
fn decode_webp(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = image_webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
    let (w, h) = dec.dimensions();
    if too_big(w, h) {
        return None;
    }
    let mut buf = vec![0; dec.output_buffer_size()?];
    dec.read_image(&mut buf).ok()?;
    let rgba = if dec.has_alpha() { buf } else { buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect() };
    Some((w, h, rgba))
}

/// First frame of a GIF, transparency included.
fn decode_gif(b: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let u16le = |i: usize| -> Option<u16> { Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?])) };
    let (w, h) = (u16le(6)? as u32, u16le(8)? as u32);
    if too_big(w, h) {
        return None;
    }
    let flags = *b.get(10)?;
    let mut i = 13;
    let mut global: &[u8] = &[];
    if flags & 0x80 != 0 {
        let n = 3 << ((flags & 7) + 1);
        global = b.get(i..i + n)?;
        i += n;
    }
    let mut transparent = None;
    loop {
        match *b.get(i)? {
            0x21 => {
                // Extension: keep the transparent index of a graphic control.
                if *b.get(i + 1)? == 0xF9 && *b.get(i + 3)? & 1 != 0 {
                    transparent = Some(*b.get(i + 6)?);
                }
                i += 2;
                while *b.get(i)? != 0 {
                    i += *b.get(i)? as usize + 1;
                }
                i += 1;
            }
            0x2C => break,
            _ => return None,
        }
    }
    let (fx, fy, fw, fh) = (u16le(i + 1)? as u32, u16le(i + 3)? as u32, u16le(i + 5)? as u32, u16le(i + 7)? as u32);
    let iflags = *b.get(i + 9)?;
    i += 10;
    let mut palette = global;
    if iflags & 0x80 != 0 {
        let n = 3 << ((iflags & 7) + 1);
        palette = b.get(i..i + n)?;
        i += n;
    }
    let min_code = *b.get(i)?;
    if !(1..=11).contains(&min_code) || w == 0 || h == 0 {
        return None;
    }
    i += 1;
    let mut data = Vec::new();
    loop {
        let n = *b.get(i)? as usize;
        if n == 0 {
            break;
        }
        data.extend_from_slice(b.get(i + 1..i + 1 + n)?);
        i += n + 1;
    }
    // As many indices as land on the picture at most: the code stream
    // can say far more than its size (each code a long string).
    let mut idx = vec![0u8; (fw as usize * fh as usize).min(w as usize * h as usize)];
    let mut dec = weezl::decode::Decoder::new(weezl::BitOrder::Lsb, min_code);
    let (mut inp, mut got) = (&data[..], 0);
    while got < idx.len() {
        let r = dec.decode_bytes(inp, &mut idx[got..]);
        inp = &inp[r.consumed_in..];
        got += r.consumed_out;
        if !matches!(r.status, Ok(weezl::LzwStatus::Ok)) || r.consumed_in + r.consumed_out == 0 {
            break;
        }
    }
    idx.truncate(got);
    // Interlaced rows come in four passes.
    let rows: Vec<u32> = if iflags & 0x40 != 0 {
        [(0, 8), (4, 8), (2, 4), (1, 2)].iter().flat_map(|&(s, st)| (s..fh).step_by(st)).collect()
    } else {
        (0..fh).collect()
    };
    let mut out = vec![0u8; w as usize * h as usize * 4];
    for (k, &row) in rows.iter().enumerate() {
        let y = fy + row;
        if y >= h || k * fw as usize >= idx.len() {
            continue;
        }
        // The frame's own size is the file's to say: only what lands on
        // the picture is looked at.
        for col in 0..fw.min(w.saturating_sub(fx)) {
            let x = fx + col;
            let Some(&c) = idx.get(k * fw as usize + col as usize) else { continue };
            if x >= w || y >= h || Some(c) == transparent {
                continue;
            }
            if let Some(p) = palette.get(3 * c as usize..3 * c as usize + 3) {
                let o = 4 * (y as usize * w as usize + x as usize);
                out[o..o + 4].copy_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
    }
    Some((w, h, out))
}

/// TIFF (the first image): gray, RGB and CMYK, 8 or 16 bits.
fn decode_tiff(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    use tiff::ColorType as C;
    use tiff::decoder::{Decoder, DecodingResult, Limits};
    let mut dec = Decoder::new(Cursor::new(bytes)).ok()?.with_limits(Limits::default());
    let (w, h) = dec.dimensions().ok()?;
    if too_big(w, h) {
        return None;
    }
    let color = dec.colortype().ok()?;
    // Samples as 8 bits.
    let px: Vec<u8> = match dec.read_image().ok()? {
        DecodingResult::U8(v) => v,
        DecodingResult::U16(v) => v.iter().map(|x| (x >> 8) as u8).collect(),
        _ => return None,
    };
    let n = w as usize * h as usize;
    let rgba = match color {
        C::Gray(8 | 16) => px.get(..n)?.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        C::GrayA(8 | 16) => px.get(..n * 2)?.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        C::RGB(8 | 16) => px.get(..n * 3)?.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        C::RGBA(8 | 16) => px.get(..n * 4)?.to_vec(),
        C::CMYK(8) => px
            .get(..n * 4)?
            .chunks_exact(4)
            .flat_map(|p| {
                let k = 255 - p[3] as u32;
                let ch = |c: u8| ((255 - c as u32) * k / 255) as u8;
                [ch(p[0]), ch(p[1]), ch(p[2]), 255]
            })
            .collect(),
        _ => return None,
    };
    Some((w, h, rgba))
}

/// Uncompressed BMP: 8 bits with palette, 24 or 32 bits.
fn decode_bmp(b: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let u32le = |i: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?)) };
    let u16le = |i: usize| -> Option<u16> { Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?)) };
    let offset = u32le(10)? as usize;
    let header = u32le(14)? as usize;
    let w = u32le(18)? as i32;
    let h = u32le(22)? as i32;
    let bits = u16le(28)?;
    let compression = u32le(30)?;
    if w <= 0 || h == 0 || h == i32::MIN || !(compression == 0 || compression == 3 && bits == 32) {
        return None;
    }
    let (w, top_down, h) = (w as u32, h < 0, h.unsigned_abs());
    if too_big(w, h) {
        return None;
    }
    let stride = (w as usize * bits as usize).div_ceil(32) * 4;
    let palette = b.get(14 + header..offset).unwrap_or(&[]);
    // Masks from BITFIELDS: 32 bits with alpha only if the file says so.
    let has_alpha = bits == 32 && (compression == 0 || u32le(54 + 12).is_some_and(|m| m != 0) && header >= 56);
    let mut out = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h as usize {
        let src_row = if top_down { y } else { h as usize - 1 - y };
        let row = b.get(offset + src_row * stride..offset + src_row * stride + stride)?;
        for x in 0..w as usize {
            let px = match bits {
                8 => {
                    let c = row[x] as usize;
                    let p = palette.get(4 * c..4 * c + 3)?;
                    [p[2], p[1], p[0], 255]
                }
                24 => [row[3 * x + 2], row[3 * x + 1], row[3 * x], 255],
                32 => [row[4 * x + 2], row[4 * x + 1], row[4 * x], if has_alpha { row[4 * x + 3] } else { 255 }],
                _ => return None,
            };
            let o = 4 * (y * w as usize + x);
            out[o..o + 4].copy_from_slice(&px);
        }
    }
    // 32-bit files often leave alpha at 0 everywhere: then it is unused.
    if bits == 32 && out.chunks_exact(4).all(|p| p[3] == 0) {
        out.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    }
    Some((w, h, out))
}

/// Shrinks (box filter, alpha weighted) to fit `max_side` and `max_px`.
pub(crate) fn fit(w: u32, h: u32, rgba: Vec<u8>, max_side: u32, max_px: u64) -> (u32, u32, Vec<u8>) {
    let px = w as f64 * h as f64;
    let s = (max_side as f64 / w.max(h) as f64).min((max_px as f64 / px).sqrt());
    if s >= 1.0 {
        return (w, h, rgba);
    }
    let (nw, nh) = (((w as f64 * s) as u32).max(1), ((h as f64 * s) as u32).max(1));
    let (w, h, nwu, nhu) = (w as usize, h as usize, nw as usize, nh as usize);
    let span = |i: usize, n: usize, m: usize| (i * n / m, ((i + 1) * n / m).max(i * n / m + 1).min(n));
    // Horizontal pass into premultiplied sums, then vertical.
    let mut tmp = vec![0f32; nwu * h * 4];
    for y in 0..h {
        for x in 0..nwu {
            let (a, b) = span(x, w, nwu);
            let mut acc = [0f32; 4];
            for sx in a..b {
                let p = &rgba[4 * (y * w + sx)..4 * (y * w + sx) + 4];
                let al = p[3] as f32;
                acc[0] += p[0] as f32 * al;
                acc[1] += p[1] as f32 * al;
                acc[2] += p[2] as f32 * al;
                acc[3] += al;
            }
            let n = (b - a) as f32;
            tmp[4 * (y * nwu + x)..4 * (y * nwu + x) + 4].copy_from_slice(&acc.map(|v| v / n));
        }
    }
    let mut out = vec![0u8; nwu * nhu * 4];
    for y in 0..nhu {
        let (a, b) = span(y, h, nhu);
        for x in 0..nwu {
            let mut acc = [0f32; 4];
            for sy in a..b {
                let p = &tmp[4 * (sy * nwu + x)..4 * (sy * nwu + x) + 4];
                for c in 0..4 {
                    acc[c] += p[c];
                }
            }
            let o = 4 * (y * nwu + x);
            if acc[3] > 0.0 {
                for c in 0..3 {
                    out[o + c] = (acc[c] / acc[3]).round().clamp(0.0, 255.0) as u8;
                }
                out[o + 3] = (acc[3] / (b - a) as f32).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    (nw, nh, out)
}

enum State {
    Pending,
    Ready(TextureHandle),
    Failed,
}

struct Entry {
    state: State,
    used: Instant,
}

/// What a texture shows, for the PDF writer.
#[derive(Clone)]
enum Source {
    File(Arc<[u8]>),
    /// A picture changed by effects.
    Pixels(Decoded),
}

/// Changes decoded pixels (picture effects) before they become a texture.
type Process = Arc<dyn Fn(u32, u32, Vec<u8>) -> (u32, u32, Vec<u8>) + Send + Sync>;

struct Cache {
    entries: HashMap<u64, Entry>,
    /// Texture -> what it shows, for the PDF writer.
    sources: HashMap<TextureId, Source>,
    /// Memoized keys by buffer address.
    keys: HashMap<usize, (Weak<[u8]>, u64)>,
    swept: Instant,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| {
    Mutex::new(Cache { entries: HashMap::new(), sources: HashMap::new(), keys: HashMap::new(), swept: Instant::now() })
});

fn cache() -> MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

impl Cache {
    fn sweep(&mut self, now: Instant) {
        if now.duration_since(self.swept) < Duration::from_secs(5) {
            return;
        }
        self.swept = now;
        self.entries.retain(|_, e| matches!(e.state, State::Pending) || now.duration_since(e.used) < IDLE);
        self.keys.retain(|_, (w, _)| w.strong_count() > 0);
        // Sources outlive their textures (a print job may still use the
        // id), but not forever.
        if self.sources.len() > 4096 {
            let live: std::collections::HashSet<TextureId> =
                self.entries.values().filter_map(|e| if let State::Ready(t) = &e.state { Some(t.id()) } else { None }).collect();
            self.sources.retain(|id, _| live.contains(id));
        }
    }
}

/// Content key of a picture file. Hashes the bytes once per buffer.
pub fn key(bytes: &Arc<[u8]>) -> u64 {
    let addr = bytes.as_ptr() as usize;
    let mut c = cache();
    if let Some((w, k)) = c.keys.get(&addr) {
        if w.upgrade().is_some_and(|a| Arc::ptr_eq(&a, bytes)) {
            return *k;
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    let k = h.finish();
    c.keys.insert(addr, (Arc::downgrade(bytes), k));
    k
}

/// The texture of a picture (`key` from [`key`]). `None` while a large
/// picture decodes in the background (the view is repainted when ready)
/// and for undecodable files. `wait` decodes now whatever the size.
pub fn texture(ctx: &Context, key: u64, bytes: &Arc<[u8]>, wait: bool) -> Option<TextureHandle> {
    texture_with(ctx, key, bytes, wait, None)
}

/// Pictures decoding at once in the background: large ones take
/// gigabytes each, and a document can show many.
const DECODERS: usize = 2;

/// A turn to decode in the background, given back when dropped.
struct Decoding;

static DECODING: (Mutex<usize>, std::sync::Condvar) = (Mutex::new(0), std::sync::Condvar::new());

impl Decoding {
    fn wait() -> Self {
        let mut n = DECODING.0.lock().unwrap_or_else(|e| e.into_inner());
        while *n >= DECODERS {
            n = DECODING.1.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n += 1;
        Decoding
    }
}

impl Drop for Decoding {
    fn drop(&mut self) {
        *DECODING.0.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        DECODING.1.notify_one();
    }
}

fn texture_with(ctx: &Context, key: u64, bytes: &Arc<[u8]>, wait: bool, process: Option<Process>) -> Option<TextureHandle> {
    texture_opts(ctx, key, bytes, wait, process, false)
}

fn texture_opts(ctx: &Context, key: u64, bytes: &Arc<[u8]>, wait: bool, process: Option<Process>, tiled: bool) -> Option<TextureHandle> {
    let now = Instant::now();
    {
        let mut c = cache();
        c.sweep(now);
        if let Some(e) = c.entries.get_mut(&key) {
            e.used = now;
            match &e.state {
                State::Ready(t) => return Some(t.clone()),
                State::Failed => return None,
                State::Pending if !wait => return None,
                State::Pending => {}
            }
        }
        // A metafile has no pixel size; a short one draws quickly.
        let small = match apex_sheet::ImageFormat::sniff(bytes) {
            Some(apex_sheet::ImageFormat::Emf | apex_sheet::ImageFormat::Wmf) => bytes.len() <= 256 << 10,
            _ => apex_sheet::image::pixel_size(bytes).is_some_and(|(w, h)| w as u64 * h as u64 <= SYNC_PIXELS),
        };
        if !wait && !small {
            use std::sync::atomic::{AtomicUsize, Ordering};
            // Threads waiting for a turn: a document showing thousands of
            // large pictures gets the rest started on later frames.
            static QUEUED: AtomicUsize = AtomicUsize::new(0);
            if QUEUED.load(Ordering::Relaxed) >= 16 {
                ctx.request_repaint_after(web_time::Duration::from_millis(200));
                return None;
            }
            c.entries.insert(key, Entry { state: State::Pending, used: now });
            let (ctx, bytes, process) = (ctx.clone(), bytes.clone(), process.clone());
            QUEUED.fetch_add(1, Ordering::Relaxed);
            let spawned = apex_fs::thread::Builder::new().name("apex-image".into()).spawn(move || {
                // Given back however the thread ends.
                struct Queued;
                impl Drop for Queued {
                    fn drop(&mut self) {
                        QUEUED.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                let _queued = Queued;
                let _turn = Decoding::wait();
                let t = load(&ctx, key, &bytes, process.as_ref(), tiled);
                store(key, t);
                ctx.request_repaint();
            });
            if spawned.is_ok() {
                return None;
            }
            QUEUED.fetch_sub(1, Ordering::Relaxed);
        }
    }
    let t = load(ctx, key, bytes, process.as_ref(), tiled);
    store(key, t.clone());
    t.map(|t| t.0)
}

fn load(ctx: &Context, key: u64, bytes: &Arc<[u8]>, process: Option<&Process>, tiled: bool) -> Option<(TextureHandle, Source)> {
    let wrap_mode = if tiled { eframe::egui::TextureWrapMode::Repeat } else { TextureOptions::LINEAR.wrap_mode };
    let (t, px) = load_with(ctx, format!("apex-image-{key:x}"), bytes, TextureOptions { wrap_mode, mipmap_mode: Some(eframe::egui::TextureFilter::Linear), ..TextureOptions::LINEAR }, process)?;
    Some((t, px.map_or_else(|| Source::File(bytes.clone()), Source::Pixels)))
}

/// A texture of the picture; with `process`, also the changed pixels.
fn load_with(ctx: &Context, name: String, bytes: &[u8], opts: TextureOptions, process: Option<&Process>) -> Option<(TextureHandle, Option<Decoded>)> {
    let d = decode(bytes)?;
    let side = (ctx.input(|i| i.max_texture_side) as u32).clamp(64, MAX_TEXTURE_SIDE);
    let (w, h, rgba) = fit(d.width, d.height, d.rgba.to_vec(), side, u64::MAX);
    let (w, h, rgba, src) = match process {
        Some(f) => {
            let (w, h, rgba) = f(w, h, rgba);
            let rgba: Arc<[u8]> = rgba.into();
            (w, h, rgba.clone(), Some(Decoded { width: w, height: h, rgba }))
        }
        None => (w, h, rgba.into(), None),
    };
    let img = eframe::egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
    Some((ctx.load_texture(name, img, opts), src))
}

/// A texture of a picture that repeats past its edges (tiled
/// backgrounds and fills), cached like [`texture`]. `key` from [`key`].
pub fn tiled_texture(ctx: &Context, key: u64, bytes: &Arc<[u8]>) -> Option<TextureHandle> {
    // Its own cache slot: the same picture may also be shown untiled.
    // Large pictures decode in the background like any other.
    texture_opts(ctx, key ^ 0x7469_6c65_645f_7478, bytes, false, None, true)
}

fn store(key: u64, t: Option<(TextureHandle, Source)>) {
    let mut c = cache();
    let state = match t {
        Some((t, src)) => {
            c.sources.insert(t.id(), src);
            State::Ready(t)
        }
        None => State::Failed,
    };
    c.entries.insert(key, Entry { state, used: Instant::now() });
}

/// The picture file a texture from [`texture`] shows.
pub fn source(id: TextureId) -> Option<Arc<[u8]>> {
    match cache().sources.get(&id)? {
        Source::File(b) => Some(b.clone()),
        Source::Pixels(_) => None,
    }
}

/// The pixels of a texture of a picture changed by effects.
pub fn pixels(id: TextureId) -> Option<Decoded> {
    match cache().sources.get(&id)? {
        Source::Pixels(d) => Some(d.clone()),
        Source::File(_) => None,
    }
}

// --- picture effects -------------------------------------------------------------------------

fn fx_key(key: u64, fx: &PicFx, soft: f32, variant: u8) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (key, fx.brightness, fx.contrast, fx.color, fx.transparency, (soft * 1000.0).round() as i64, variant).hash(&mut h);
    h.finish()
}

/// The texture of a picture with its pixel effects (`shown_w`: its width
/// in points, for soft edges).
pub fn texture_fx(ctx: &Context, bytes: &Arc<[u8]>, fx: &PicFx, shown_w: f32, wait: bool) -> Option<TextureHandle> {
    let key = key(bytes);
    if !fx.changes_pixels() {
        return texture(ctx, key, bytes, wait);
    }
    let soft = fx.soft_edges / shown_w.max(1.0);
    let f = fx.clone();
    let process: Process = Arc::new(move |w, h, mut rgba| {
        f.apply(w, h, &mut rgba, shown_w);
        (w, h, rgba)
    });
    texture_with(ctx, fx_key(key, fx, soft, 0), bytes, wait, Some(process))
}

/// The reflection of a picture: upside down, fading out halfway.
pub fn reflection_texture(ctx: &Context, bytes: &Arc<[u8]>, fx: &PicFx, shown_w: f32, wait: bool) -> Option<TextureHandle> {
    let key = key(bytes);
    let soft = fx.soft_edges / shown_w.max(1.0);
    let f = fx.clone();
    let process: Process = Arc::new(move |w, h, mut rgba| {
        if f.changes_pixels() {
            f.apply(w, h, &mut rgba, shown_w);
        }
        let row = w as usize * 4;
        let mut out = vec![0u8; rgba.len()];
        for y in 0..h as usize {
            let src = &rgba[(h as usize - 1 - y) * row..][..row];
            let dst = &mut out[y * row..][..row];
            dst.copy_from_slice(src);
            let t = (1.0 - y as f32 / (h as f32 * 0.5)).max(0.0) * 0.45;
            for px in dst.chunks_exact_mut(4) {
                px[3] = (px[3] as f32 * t) as u8;
            }
        }
        (w, h, out)
    });
    texture_with(ctx, fx_key(key, fx, soft, 1), bytes, wait, Some(process))
}

/// Paints a picture with its effects: glow and shadow behind it, the
/// reflection under it and the border on top. `shown_w` is its width in
/// points (the rect's size gives the zoom).
pub fn paint_picture(painter: &Painter, bytes: &Arc<[u8]>, rect: Rect, rotation: f32, crop: [f32; 4], fx: Option<&PicFx>, shown_w: f32, wait: bool) {
    let Some(fx) = fx.filter(|f| !f.is_default()) else {
        let tex = texture(painter.ctx(), key(bytes), bytes, wait);
        return paint(painter, tex.as_ref(), rect, rotation, crop);
    };
    if !rect.is_finite() {
        return;
    }
    let z = rect.width() / shown_w.max(0.1);
    let turned = |r: Rect| corners(r, rotation).to_vec();
    let opacity = 1.0 - fx.transparency.min(100) as f32 / 100.0;
    // Glow: layers fading outwards.
    if let Some((c, size)) = fx.glow {
        let [r, g, b] = [(c >> 16) as u8, (c >> 8) as u8, c as u8];
        const N: usize = 8;
        for i in (1..=N).rev() {
            let e = size * z * i as f32 / N as f32;
            let a = (0.6 / N as f32 * 255.0 * opacity) as u8;
            painter.add(Shape::convex_polygon(turned(rect.expand(e)), Color32::from_rgba_unmultiplied(r, g, b, a), Stroke::NONE));
        }
    }
    // Shadow: down and to the right, blurred.
    if fx.shadow {
        let off = vec2(1.0, 1.0) * apex_doc::SHADOW_OFFSET * z;
        const N: usize = 5;
        for i in (0..N).rev() {
            let e = apex_doc::SHADOW_BLUR * z * i as f32 / (N - 1) as f32;
            let a = (0.5 / N as f32 * 255.0 * opacity) as u8;
            painter.add(Shape::convex_polygon(turned(rect.translate(off).expand(e)), Color32::from_rgba_unmultiplied(0, 0, 0, a), Stroke::NONE));
        }
    }
    // Reflection (only for pictures that aren't turned).
    if fx.reflection && rotation == 0.0 {
        let tex = reflection_texture(painter.ctx(), bytes, fx, shown_w, wait);
        let [l, t, r, b] = crop.map(|v| v.clamp(0.0, 1.0));
        let under = Rect::from_min_size(pos2(rect.min.x, rect.max.y + apex_doc::REFLECTION_GAP * z), vec2(rect.width(), rect.height() * 0.5));
        if let Some(tex) = tex {
            let (v0, v1) = (b, b + (1.0 - t - b) * 0.5);
            let mut mesh = Mesh::with_texture(tex.id());
            for (p, uv) in [under.left_top(), under.right_top(), under.right_bottom(), under.left_bottom()].into_iter().zip([pos2(l, v0), pos2(1.0 - r, v0), pos2(1.0 - r, v1), pos2(l, v1)]) {
                mesh.vertices.push(Vertex { pos: p, uv, color: Color32::WHITE });
            }
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            painter.add(Shape::mesh(mesh));
        }
    }
    let tex = texture_fx(painter.ctx(), bytes, fx, shown_w, wait);
    paint(painter, tex.as_ref(), rect, rotation, crop);
    if let Some((c, w)) = fx.border {
        let color = Color32::from_rgba_unmultiplied((c >> 16) as u8, (c >> 8) as u8, c as u8, (255.0 * opacity) as u8);
        painter.add(Shape::closed_line(turned(rect), Stroke::new((w * z).max(0.5), color)));
    }
}

/// Textures used by `shapes` (meshes), to keep them alive with a print job.
pub fn used_by<'a>(shapes: impl IntoIterator<Item = &'a Shape>) -> Vec<TextureHandle> {
    let c = cache();
    let mut ids = Vec::new();
    for s in shapes {
        collect_ids(s, &mut ids);
    }
    c.entries
        .values()
        .filter_map(|e| match &e.state {
            State::Ready(t) if ids.contains(&t.id()) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn collect_ids(s: &Shape, ids: &mut Vec<TextureId>) {
    match s {
        Shape::Vec(v) => v.iter().for_each(|s| collect_ids(s, ids)),
        Shape::Mesh(m) if !ids.contains(&m.texture_id) => ids.push(m.texture_id),
        _ => {}
    }
}

/// Corners (top-left, top-right, bottom-right, bottom-left) of `rect`
/// turned `rotation` degrees clockwise about its center.
pub fn corners(rect: Rect, rotation: f32) -> [Pos2; 4] {
    let c = rect.center();
    let (s, co) = rotation.to_radians().sin_cos();
    [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()].map(|p| {
        let d = p - c;
        c + vec2(d.x * co - d.y * s, d.x * s + d.y * co)
    })
}

/// Paints a picture filling `rect` turned `rotation` degrees clockwise,
/// showing only what `crop` (fractions cut from left, top, right, bottom)
/// leaves. Without a texture: a gray box.
pub fn paint(painter: &Painter, tex: Option<&TextureHandle>, rect: Rect, rotation: f32, crop: [f32; 4]) {
    if !rect.is_finite() || !rotation.is_finite() {
        return;
    }
    let pts = corners(rect, rotation);
    let Some(tex) = tex else {
        painter.add(Shape::convex_polygon(pts.to_vec(), Color32::from_gray(222), Stroke::new(1.0, Color32::from_gray(170))));
        return;
    };
    let [l, t, r, b] = crop.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
    let uv = [pos2(l, t), pos2(1.0 - r, t), pos2(1.0 - r, 1.0 - b), pos2(l, 1.0 - b)];
    let mut mesh = Mesh::with_texture(tex.id());
    for (p, uv) in pts.iter().zip(uv) {
        mesh.vertices.push(Vertex { pos: *p, uv, color: Color32::WHITE });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(Shape::mesh(mesh));
}

/// Thin outline of a picture's turned box (selection, drop previews).
pub fn outline(painter: &Painter, rect: Rect, rotation: f32, stroke: Stroke) {
    if rotation == 0.0 {
        painter.rect_stroke(rect, 0.0, stroke, StrokeKind::Middle);
    } else {
        painter.add(Shape::closed_line(corners(rect, rotation).to_vec(), stroke));
    }
}

/// A hash of a picture's pixels: finds our own copies coming back from
/// the system clipboard.
pub fn pixel_hash(rgba: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    rgba.hash(&mut h);
    h.finish()
}

/// Reads a picture from the system clipboard, as PNG. `None` when it holds
/// text (a text paste handles it) or no picture. The text of a picture
/// copied in Word is the object mark: no text.
#[cfg(any(windows, target_os = "macos"))]
pub fn read_clipboard_image() -> Option<Vec<u8>> {
    let mut cb = arboard::Clipboard::new().ok()?;
    if cb.get_text().is_ok_and(|t| t.chars().any(|c| !c.is_whitespace() && c != apex_doc::OBJECT)) {
        return None;
    }
    let Ok(img) = cb.get_image() else {
        return crate::charts::app_clip().map(|b| b.to_vec());
    };
    let img = eframe::egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.bytes);
    crate::charts::encode_png(&img)
}

/// Reads a picture from the system clipboard (Wayland or X11). `None`
/// when it holds text (a text paste handles it) or no picture.
#[cfg(not(any(windows, target_os = "macos")))]
pub fn read_clipboard_image() -> Option<Vec<u8>> {
    // Tests never touch the desktop's clipboard: on GNOME each wl-paste
    // shows up as an app and can wait there for ever.
    if cfg!(test) {
        return crate::charts::app_clip().map(|b| b.to_vec());
    }
    let run = |cmd: &str, args: &[&str]| -> Option<Vec<u8>> {
        use std::io::Read;
        use std::process::{Command, Stdio};
        let mut child = Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        let mut pipe = child.stdout.take()?;
        let reader = apex_fs::thread::spawn(move || {
            let mut buf = Vec::new();
            pipe.read_to_end(&mut buf).map(|_| buf)
        });
        // A clipboard owner that never answers must not freeze the window.
        let deadline = web_time::Instant::now() + web_time::Duration::from_secs(2);
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) if web_time::Instant::now() < deadline => apex_fs::thread::sleep(web_time::Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        };
        let out = reader.join().ok()?.ok()?;
        status.success().then_some(out)
    };
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    // Without a clipboard tool, a chart copied in this window is still
    // there (the app's own clipboard).
    let Some(list) = (if wayland { run("wl-paste", &["--list-types"]) } else { run("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"]) }) else {
        return crate::charts::app_clip().map(|b| b.to_vec());
    };
    let list = String::from_utf8_lossy(&list);
    let types: Vec<&str> = list.lines().map(str::trim).collect();
    if types.iter().any(|t| t.starts_with("text/plain") || *t == "UTF8_STRING" || *t == "STRING") {
        return None;
    }
    let ty = ["image/png", "image/jpeg", "image/gif", "image/bmp"].into_iter().find(|t| types.contains(t))?;
    let data = if wayland { run("wl-paste", &["--no-newline", "--type", ty]) } else { run("xclip", &["-selection", "clipboard", "-t", ty, "-o"]) }?;
    (!data.is_empty()).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, color: png::ColorType, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, w, h);
            e.set_color(color);
            e.set_depth(png::BitDepth::Eight);
            let mut wr = e.write_header().unwrap();
            wr.write_image_data(data).unwrap();
        }
        out
    }

    #[test]
    fn decodes_png_variants() {
        let d = decode(&png(2, 1, png::ColorType::Rgb, &[255, 0, 0, 0, 0, 255])).unwrap();
        assert_eq!((d.width, d.height, &d.rgba[..]), (2, 1, &[255, 0, 0, 255, 0, 0, 255, 255][..]));
        let d = decode(&png(1, 1, png::ColorType::GrayscaleAlpha, &[9, 128])).unwrap();
        assert_eq!(&d.rgba[..], &[9, 9, 9, 128]);
        assert!(decode(b"\x89PNG\r\n\x1a\n garbage").is_none());
        assert!(decode(b"").is_none());
        assert!(decode(b"GIF89a").is_none());
        assert!(decode(b"BM").is_none());
    }

    #[test]
    fn decodes_bmp_and_gif() {
        // 2x2, 24 bits, bottom-up: rows padded to 8 bytes.
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 8]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&2i32.to_le_bytes());
        bmp.extend_from_slice(&2i32.to_le_bytes());
        bmp.extend_from_slice(&[1, 0, 24, 0]);
        bmp.extend_from_slice(&[0; 24]);
        bmp.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0]); // bottom row: blue, green
        bmp.extend_from_slice(&[0, 0, 255, 255, 255, 255, 0, 0]); // top row: red, white
        let d = decode(&bmp).unwrap();
        assert_eq!(&d.rgba[..], &[255, 0, 0, 255, 255, 255, 255, 255, 0, 0, 255, 255, 0, 255, 0, 255]);
        assert!(decode(&bmp[..60]).is_none());
        // 1x1 GIF, red, from a common encoder.
        let gif = [
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 1, 0, 1, 0, 0x80, 0, 0, 255, 0, 0, 0, 0, 0, 0x21, 0xF9, 4, 0, 0, 0, 0, 0, 0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2,
            2, 0x44, 0x01, 0, 0x3B,
        ];
        let d = decode(&gif).unwrap();
        assert_eq!(&d.rgba[..], &[255, 0, 0, 255]);
        for n in 0..gif.len() {
            let _ = decode(&gif[..n]);
        }
        // A frame far larger than the picture, with a long code stream:
        // only what lands on the picture is decoded.
        let mut big = gif[..29].to_vec();
        big.extend_from_slice(&[0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0, 2]);
        for _ in 0..4000 {
            big.push(255);
            big.extend_from_slice(&[0; 255]);
        }
        big.extend_from_slice(&[0, 0x3B]);
        let started = web_time::Instant::now();
        let _ = decode(&big);
        assert!(started.elapsed() < web_time::Duration::from_secs(5));
    }

    #[test]
    fn shrinks_big_pictures() {
        let (w, h, px) = fit(4, 2, [10u8, 20, 30, 255].repeat(8), 2, u64::MAX);
        assert_eq!((w, h), (2, 1));
        assert_eq!(&px[..4], &[10, 20, 30, 255]);
        // Transparent pixels do not darken the average.
        let mut src = vec![200, 0, 0, 255, 0, 0, 0, 0];
        src.extend_from_slice(&src.clone());
        let (_, _, px) = fit(2, 2, src, 1, u64::MAX);
        assert_eq!(px, [200, 0, 0, 128]);
        let (w, h, _) = fit(100, 100, vec![0; 40000], 8000, 400);
        assert_eq!((w, h), (20, 20));
    }

    #[test]
    fn keys_follow_content() {
        let a: Arc<[u8]> = Arc::from(&b"abc"[..]);
        let b: Arc<[u8]> = Arc::from(&b"abc"[..]);
        let c: Arc<[u8]> = Arc::from(&b"abd"[..]);
        assert_eq!(key(&a), key(&b));
        assert_ne!(key(&a), key(&c));
        assert_eq!(key(&a), key(&a));
    }

    #[test]
    fn corners_turn_clockwise() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(4.0, 2.0));
        let p = corners(r, 90.0);
        // Top-left goes to the top-right of the turned box.
        assert!((p[0] - pos2(3.0, -1.0)).length() < 1e-4, "{p:?}");
    }
}
