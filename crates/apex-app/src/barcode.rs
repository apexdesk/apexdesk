//! QR codes and barcodes (Insert > QR code). A code goes in as a picture,
//! so every program shows and prints it; its alt text says what it holds
//! ("QR: https://…"), for screen readers and to open it again for editing.

use std::sync::Arc;

use eframe::egui::{self, Color32, ColorImage, FontId, Pos2, Rect, vec2};

use crate::i18n::{tr, trf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Qr,
    Code128,
    Ean13,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Qr, Kind::Code128, Kind::Ean13];

    /// Its name, the same in every language (it starts the alt text).
    pub fn name(self) -> &'static str {
        match self {
            Kind::Qr => "QR",
            Kind::Code128 => "Code 128",
            Kind::Ean13 => "EAN-13",
        }
    }

    fn hint(self) -> &'static str {
        tr(match self {
            Kind::Qr => "barcode.hint_qr",
            Kind::Code128 => "barcode.hint_128",
            Kind::Ean13 => "barcode.hint_ean",
        })
    }
}

/// Why some text can't be a code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Empty,
    TooLong,
    BadChar(char),
    /// An EAN-13 needs 12 digits, or 13 with the check digit.
    Digits,
    /// The check digit is wrong; this is the right one.
    Check(u8),
}

impl Error {
    pub fn text(&self, kind: Kind) -> String {
        match self {
            Error::Empty => tr("barcode.empty").to_string(),
            Error::TooLong => trf("barcode.too_long", &[("kind", &kind.name())]),
            Error::BadChar(c) => trf("barcode.bad_char", &[("char", c), ("kind", &kind.name())]),
            Error::Digits => tr("barcode.digits").to_string(),
            Error::Check(d) => trf("barcode.check", &[("digit", d)]),
        }
    }
}

/// The alt text of a code: its kind and what it holds.
pub fn alt(kind: Kind, text: &str) -> String {
    format!("{}: {text}", kind.name())
}

/// The code a picture's alt text describes, if it is one.
pub fn from_alt(alt: &str) -> Option<(Kind, String)> {
    Kind::ALL.into_iter().find_map(|k| Some((k, alt.strip_prefix(k.name())?.strip_prefix(": ")?.to_string())))
}

/// A code's modules (its smallest squares or bars), dark ones true.
#[derive(Debug, PartialEq)]
enum Modules {
    /// A square `n` modules wide, row by row.
    Grid(usize, Vec<bool>),
    /// A row of bars; `long` ones reach down into the text line, and
    /// `labels` are the text under them, centered at module positions
    /// (counted from the first bar).
    Bars { bars: Vec<bool>, long: Vec<bool>, labels: Vec<(f32, String)> },
}

fn encode(kind: Kind, text: &str) -> Result<Modules, Error> {
    if text.trim().is_empty() {
        return Err(Error::Empty);
    }
    match kind {
        Kind::Qr => {
            let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).map_err(|_| Error::TooLong)?;
            let dark = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
            Ok(Modules::Grid(code.width(), dark))
        }
        Kind::Code128 => {
            let values = code128(text)?;
            let mut bars = Vec::new();
            for v in values {
                for (i, w) in PATTERNS_128[v as usize].bytes().enumerate() {
                    bars.extend(std::iter::repeat_n(i % 2 == 0, (w - b'0') as usize));
                }
            }
            let n = bars.len();
            Ok(Modules::Bars { long: vec![false; n], labels: vec![(n as f32 / 2.0, text.to_string())], bars })
        }
        Kind::Ean13 => ean13(text),
    }
}

// --- Code 128 ------------------------------------------------------------------------

/// Bar and space widths of the Code 128 symbols 0 to 105, and the stop.
const PATTERNS_128: [&str; 107] = [
    "212222", "222122", "222221", "121223", "121322", "131222", "122213", "122312", "132212", "221213", "221312", "231212", "112232", "122132", "122231", "113222", "123122", "123221", "223211", "221132", "221231", "213212", "223112", "312131", "311222", "321122", "321221", "312212", "322112", "322211", "212123", "212321", "232121", "111323", "131123", "131321", "112313", "132113", "132311", "211313", "231113", "231311", "112133", "112331", "132131", "113123", "113321", "133121", "313121", "211331", "231131", "213113", "213311", "213131", "311123", "311321", "331121", "312113", "312311", "332111", "314111", "221411", "431111", "111224", "111422", "121124", "121421", "141122", "141221", "112214", "112412", "122114", "122411", "142112", "142211", "241211", "221114", "413111", "241112", "134111", "111242", "121142", "121241", "114212", "124112", "124211", "411212", "421112", "421211", "212141", "214121", "412121", "111143", "111341", "131141", "114113", "114311", "411113", "411311", "113141", "114131", "311141", "411131", "211412", "211214", "211232", "2331112",
];
const START_B: u8 = 104;
const START_C: u8 = 105;
const TO_B: u8 = 100;
const TO_C: u8 = 99;
const STOP: u8 = 106;
/// Longest text taken (a longer code doesn't fit a page's width).
const MAX_128: usize = 80;

/// The symbols of `text` in Code 128: printable ASCII in code set B, and
/// runs of digits two to a symbol in code set C; with the start, the check
/// symbol and the stop.
fn code128(text: &str) -> Result<Vec<u8>, Error> {
    if let Some(c) = text.chars().find(|c| !(' '..='~').contains(c)) {
        return Err(Error::BadChar(c));
    }
    let s = text.as_bytes();
    if s.len() > MAX_128 {
        return Err(Error::TooLong);
    }
    let digits = |i: usize| s[i..].iter().take_while(|b| b.is_ascii_digit()).count();
    let (n, d0) = (s.len(), digits(0));
    // Set C pays off for 4 digits at either end, 6 in the middle.
    let mut c = d0 >= 4 || (d0 == n && n >= 2);
    let mut out = vec![if c { START_C } else { START_B }];
    let mut i = 0;
    while i < n {
        let d = digits(i);
        if c {
            if d >= 2 {
                out.push((s[i] - b'0') * 10 + (s[i + 1] - b'0'));
                i += 2;
            } else {
                out.push(TO_B);
                c = false;
            }
        } else if d >= 6 || (d >= 4 && i + d == n) {
            if d % 2 == 1 {
                out.push(s[i] - b' ');
                i += 1;
            }
            out.push(TO_C);
            c = true;
        } else {
            out.push(s[i] - b' ');
            i += 1;
        }
    }
    let check = out.iter().enumerate().map(|(k, &v)| k.max(1) as u32 * v as u32).sum::<u32>() % 103;
    out.push(check as u8);
    out.push(STOP);
    Ok(out)
}

// --- EAN-13 --------------------------------------------------------------------------

/// The left-hand odd parity (L) patterns of the digits, 7 modules each.
const EAN_L: [u8; 10] = [0b0001101, 0b0011001, 0b0010011, 0b0111101, 0b0100011, 0b0110001, 0b0101111, 0b0111011, 0b0110111, 0b0001011];
/// Which of the left six digits use even parity (G), by the first digit
/// (bit 5 is the second digit).
const EAN_PARITY: [u8; 10] = [0b000000, 0b001011, 0b001101, 0b001110, 0b010011, 0b011001, 0b011100, 0b010101, 0b010110, 0b011010];

/// The check digit of the first 12 digits.
fn ean_check(d: &[u8]) -> u8 {
    let sum: u32 = d.iter().enumerate().map(|(i, &x)| x as u32 * if i % 2 == 0 { 1 } else { 3 }).sum();
    ((10 - sum % 10) % 10) as u8
}

fn ean13(text: &str) -> Result<Modules, Error> {
    let mut d = Vec::new();
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '-') {
        d.push(c.to_digit(10).ok_or(Error::BadChar(c))? as u8);
    }
    if d.len() != 12 && d.len() != 13 {
        return Err(Error::Digits);
    }
    let check = ean_check(&d[..12]);
    match d.get(12) {
        None => d.push(check),
        Some(&x) if x != check => return Err(Error::Check(check)),
        Some(_) => {}
    }
    let mut bars = Vec::with_capacity(95);
    let mut long = Vec::with_capacity(95);
    let mut put = |pattern: u8, width: usize, guard: bool| {
        for i in (0..width).rev() {
            bars.push(pattern >> i & 1 == 1);
            long.push(guard);
        }
    };
    put(0b101, 3, true);
    for (i, &x) in d[1..7].iter().enumerate() {
        let even = EAN_PARITY[d[0] as usize] >> (5 - i) & 1 == 1;
        // G is R (L inverted) read backwards.
        let p = if even { (!EAN_L[x as usize] & 0x7F).reverse_bits() >> 1 } else { EAN_L[x as usize] };
        put(p, 7, false);
    }
    put(0b01010, 5, true);
    for &x in &d[7..] {
        put(!EAN_L[x as usize] & 0x7F, 7, false);
    }
    put(0b101, 3, true);
    let mut labels = vec![(-6.0, d[0].to_string())];
    labels.extend((1..7).map(|i| (3.0 + 7.0 * (i - 1) as f32 + 3.5, d[i].to_string())));
    labels.extend((7..13).map(|i| (50.0 + 7.0 * (i - 7) as f32 + 3.5, d[i].to_string())));
    Ok(Modules::Bars { bars, long, labels })
}

// --- pictures ------------------------------------------------------------------------

/// A code drawn: its pixels, its PNG file, and its size on the page.
pub struct Picture {
    pub image: ColorImage,
    pub png: Vec<u8>,
    /// In points.
    pub size: (f32, f32),
}

/// Pixels per module of a QR code, and the quiet zone around it (modules).
const QR_PX: usize = 10;
const QR_QUIET: usize = 4;
/// Pixels per module of a barcode; its bars' height and the text line
/// under them (modules).
const BAR_PX: usize = 4;
const BAR_H: usize = 60;
const TEXT_H: usize = 14;

/// Draws the code `kind` of `text`.
pub fn picture(ctx: &egui::Context, kind: Kind, text: &str) -> Result<Picture, Error> {
    let image = match encode(kind, text)? {
        Modules::Grid(n, dark) => {
            let side = (n + 2 * QR_QUIET) * QR_PX;
            let mut img = ColorImage::new([side, side], vec![Color32::WHITE; side * side]);
            for (i, _) in dark.iter().enumerate().filter(|(_, d)| **d) {
                let (x, y) = ((i % n + QR_QUIET) * QR_PX, (i / n + QR_QUIET) * QR_PX);
                fill(&mut img, x, y, QR_PX, QR_PX);
            }
            img
        }
        Modules::Bars { bars, long, labels } => {
            // EAN-13 prints its first digit in the left quiet zone.
            let (left, right) = if kind == Kind::Ean13 { (11, 7) } else { (10, 10) };
            let w = (left + bars.len() + right) * BAR_PX;
            let h = (BAR_H + TEXT_H) * BAR_PX;
            let mut img = ColorImage::new([w, h], vec![Color32::WHITE; w * h]);
            let size = TEXT_H as f32 * BAR_PX as f32 * 0.72;
            text_line(ctx, &mut img, &labels, (left * BAR_PX) as f32, ((BAR_H + 1) * BAR_PX) as f32, size);
            for (i, _) in bars.iter().enumerate().filter(|(_, b)| **b) {
                let tall = if long[i] { BAR_H + TEXT_H / 2 } else { BAR_H };
                fill(&mut img, (left + i) * BAR_PX, 0, BAR_PX, tall * BAR_PX);
            }
            img
        }
    };
    let png = crate::charts::encode_png(&image).ok_or(Error::TooLong)?;
    let [w, h] = image.size;
    // QR codes about 3 cm and at least a point a module; barcodes with
    // 0.33 mm modules (EAN-13 at 100 %) or 0.25 mm (Code 128).
    let size = match kind {
        Kind::Qr => {
            let s = (w / QR_PX) as f32 * (85.0 / (w / QR_PX) as f32).max(1.0);
            (s, s)
        }
        _ => {
            let pt = if kind == Kind::Ean13 { 0.935 } else { 0.71 } / BAR_PX as f32;
            (w as f32 * pt, h as f32 * pt)
        }
    };
    Ok(Picture { image, png, size })
}

fn fill(img: &mut ColorImage, x: usize, y: usize, w: usize, h: usize) {
    let width = img.size[0];
    for row in y..(y + h).min(img.size[1]) {
        img.pixels[row * width + x..row * width + (x + w).min(width)].fill(Color32::BLACK);
    }
}

/// Draws `labels` (centered at module positions from `x0`) with their tops
/// at `y`, `size` pixels high.
fn text_line(ctx: &egui::Context, img: &mut ColorImage, labels: &[(f32, String)], x0: f32, y: f32, size: f32) {
    let ppp = ctx.pixels_per_point();
    let font = FontId::proportional(size / ppp);
    let mut shapes = Vec::new();
    for (at, t) in labels {
        let g = ctx.fonts_mut(|f| f.layout_no_wrap(t.clone(), font.clone(), Color32::BLACK));
        let x = x0 + at * BAR_PX as f32;
        shapes.push(egui::Shape::galley(Pos2::new(x / ppp - g.size().x / 2.0, y / ppp), g, Color32::BLACK));
    }
    let clipped = shapes.into_iter().map(|shape| egui::epaint::ClippedShape { clip_rect: Rect::EVERYTHING, shape }).collect();
    let prims = ctx.tessellate(clipped, ppp);
    let atlas = ctx.fonts(|f| f.image());
    for p in prims {
        if let egui::epaint::Primitive::Mesh(m) = p.primitive {
            if m.texture_id == egui::TextureId::default() {
                crate::charts::raster(img, &m, &atlas, ppp);
            }
        }
    }
}

// --- dialog --------------------------------------------------------------------------

/// A code made by the dialog, to insert or to put in place of the one
/// being edited.
pub struct Made {
    pub png: Arc<[u8]>,
    /// In points.
    pub size: (f32, f32),
    pub alt: String,
    pub name: String,
}

/// The code `kind` of `text`, ready to go in.
pub fn made(ctx: &egui::Context, kind: Kind, text: &str) -> Result<Made, Error> {
    let pic = picture(ctx, kind, text)?;
    Ok(Made { png: Arc::from(pic.png), size: pic.size, alt: alt(kind, text), name: kind.name().to_string() })
}

/// The Insert > QR code dialog.
#[derive(Default)]
pub struct Dialog {
    open: bool,
    kind: Kind,
    text: String,
    /// The preview, and the kind and text it shows.
    shown: Option<(Kind, String, Result<egui::TextureHandle, Error>)>,
}

impl Dialog {
    /// Opens the dialog for a new code.
    pub fn show(&mut self) {
        let kind = self.kind;
        *self = Dialog { open: true, kind, ..Default::default() };
    }

    /// Opens the dialog on a code in place.
    pub fn edit(&mut self, kind: Kind, text: String) {
        *self = Dialog { open: true, kind, text, shown: None };
    }

    /// The code being edited, while the dialog is open.
    #[cfg(test)]
    pub fn editing(&self) -> Option<(Kind, &str)> {
        self.open.then_some((self.kind, self.text.as_str()))
    }

    /// Shows the dialog; returns the code made when accepted.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Made> {
        if !self.open {
            return None;
        }
        if self.shown.as_ref().is_none_or(|(k, t, _)| *k != self.kind || *t != self.text) {
            let pic = picture(ctx, self.kind, &self.text).map(|p| ctx.load_texture("barcode-preview", p.image, egui::TextureOptions::NEAREST));
            self.shown = Some((self.kind, self.text.clone(), pic));
        }
        let res = crate::widgets::dialog(ctx, "barcode", tr("barcode.title"), 440.0, |ui| {
            crate::widgets::form(ui, "barcode-form", |ui| {
                crate::widgets::form_row(ui, tr("barcode.kind"), |ui| {
                    ui.horizontal(|ui| {
                        for k in Kind::ALL {
                            ui.radio_value(&mut self.kind, k, k.name());
                        }
                    });
                });
                crate::widgets::form_row(ui, tr("barcode.text"), |ui| {
                    let edit = if self.kind == Kind::Qr { egui::TextEdit::multiline(&mut self.text).desired_rows(3) } else { egui::TextEdit::singleline(&mut self.text) };
                    let r = ui.add(edit.hint_text(self.kind.hint()).desired_width(280.0));
                    // Enter in a QR code's text starts a line, not OK (the
                    // field keeps its id when the kind changes).
                    ui.data_mut(|m| m.insert_temp(r.id.with("multiline"), self.kind == Kind::Qr));
                    if ui.memory(|m| m.focused().is_none()) {
                        r.request_focus();
                    }
                });
            });
            ui.add_space(8.0);
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 190.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 4.0, Color32::WHITE);
            match self.shown.as_ref().map(|s| &s.2) {
                Some(Ok(tex)) => {
                    let s = tex.size_vec2();
                    let k = ((rect.width() - 16.0) / s.x).min((rect.height() - 16.0) / s.y);
                    let r = Rect::from_center_size(rect.center(), s * k);
                    ui.painter().image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
                Some(Err(e)) if !self.text.is_empty() => {
                    let g = ui.painter().layout(e.text(self.kind), FontId::proportional(13.0), ui.visuals().error_fg_color, rect.width() - 24.0);
                    ui.painter().galley(rect.center() - g.size() / 2.0, g, Color32::RED);
                }
                _ => {}
            }
        });
        match res {
            crate::widgets::DialogResult::Ok => {
                let m = made(ctx, self.kind, &self.text).ok()?;
                self.open = false;
                Some(m)
            }
            crate::widgets::DialogResult::Cancel => {
                self.open = false;
                None
            }
            crate::widgets::DialogResult::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars(kind: Kind, text: &str) -> String {
        match encode(kind, text).unwrap() {
            Modules::Bars { bars, .. } => bars.iter().map(|&b| if b { '1' } else { '0' }).collect(),
            Modules::Grid(..) => unreachable!(),
        }
    }

    #[test]
    fn code128_patterns_are_sound() {
        let mut seen = std::collections::HashSet::new();
        for (i, p) in PATTERNS_128.iter().enumerate() {
            let sum: u32 = p.bytes().map(|b| (b - b'0') as u32).sum();
            assert_eq!(sum, if i == 106 { 13 } else { 11 }, "symbol {i}");
            assert!(seen.insert(p), "symbol {i} repeats");
        }
    }

    #[test]
    fn code128_sets_and_check() {
        // "Wikipedia" in set B: (104 + 55 + 2·73 + 3·75 + … + 9·65) mod 103.
        let v = code128("Wikipedia").unwrap();
        assert_eq!(v[0], START_B);
        assert_eq!(v[v.len() - 2], 88);
        // Digits alone go two to a symbol.
        assert_eq!(code128("123456").unwrap(), vec![START_C, 12, 34, 56, ((105 + 12 + 2 * 34 + 3 * 56) % 103) as u8, STOP]);
        // An odd count ends in set B.
        assert_eq!(&code128("12345").unwrap()[..5], &[START_C, 12, 34, TO_B, b'5' - b' ']);
        // A long run of digits inside text switches to set C.
        let v = code128("AB1234567").unwrap();
        assert_eq!(&v[..6], &[START_B, 33, 34, b'1' - b' ', TO_C, 23]);
        assert_eq!(code128("año"), Err(Error::BadChar('ñ')));
        // Bars start with a bar and end with the stop's.
        let b = bars(Kind::Code128, "Hi");
        assert!(b.starts_with("11010010000") && b.ends_with("1100011101011"), "{b}");
    }

    #[test]
    fn ean13_bars_and_check_digit() {
        // 4006381333931 (a sample with a known bar pattern).
        let b = bars(Kind::Ean13, "400638133393");
        assert_eq!(b.len(), 95);
        assert_eq!(&b[..3], "101");
        assert_eq!(&b[3..17], "00011010100111", "the first digit 4 makes the 0s L then G");
        assert_eq!(&b[45..50], "01010");
        assert_eq!(&b[92..], "101");
        assert_eq!(bars(Kind::Ean13, "4006381333931"), b);
        assert_eq!(encode(Kind::Ean13, "4006381333932"), Err(Error::Check(1)));
        assert_eq!(encode(Kind::Ean13, "12345"), Err(Error::Digits));
        assert_eq!(encode(Kind::Ean13, "40063813339x"), Err(Error::BadChar('x')));
        assert_eq!(ean_check(&[8, 4, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0]), 5);
    }

    #[test]
    fn qr_and_alt_text() {
        let Modules::Grid(n, dark) = encode(Kind::Qr, "https://example.com").unwrap() else { unreachable!() };
        assert_eq!(n, 25, "version 2");
        assert_eq!(dark.len(), n * n);
        assert!(dark[..7].iter().all(|&d| d), "finder pattern");
        assert_eq!(encode(Kind::Qr, "  "), Err(Error::Empty));
        assert_eq!(encode(Kind::Qr, &"x".repeat(3000)), Err(Error::TooLong));
        let a = alt(Kind::Code128, "A: b");
        assert_eq!(from_alt(&a), Some((Kind::Code128, "A: b".into())));
        assert_eq!(from_alt("A photo"), None);
    }

    #[test]
    fn pictures() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let p = picture(&ctx, Kind::Qr, "hola").unwrap();
        assert_eq!(p.image.size, [(21 + 8) * QR_PX; 2]);
        assert!((p.size.0 - 85.0).abs() < 0.01 && p.size.0 == p.size.1, "{:?}", p.size);
        assert_eq!(crate::images::decode(&p.png).map(|d| (d.width, d.height)), Some((290, 290)));
        let p = picture(&ctx, Kind::Ean13, "8412345678905").unwrap();
        assert_eq!(p.image.size, [(11 + 95 + 7) * BAR_PX, (BAR_H + TEXT_H) * BAR_PX]);
        // The digits are drawn under the bars.
        let [w, h] = p.image.size;
        let text_row = (BAR_H + 6) * BAR_PX;
        assert!((0..w).any(|x| p.image.pixels[text_row * w + x].r() < 128), "digits");
        assert!(p.image.pixels[(h - 1) * w..].iter().all(|c| c.r() > 200), "bottom row blank");
    }
}
