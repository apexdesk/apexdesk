//! Appearances of text fields as pdfium makes them when the form asks for
//! them (NeedAppearances): its CPDF_GenerateAP, with the text laid out as
//! its CPVT_VariableText lays it out.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use super::super::obj::{Dict, Obj};
use super::R;

/// pdfium's sizes for text of size 0 (fitted to the field).
const AUTO_SIZES: [f64; 25] = [4.0, 6.0, 8.0, 9.0, 10.0, 12.0, 14.0, 18.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0, 130.0, 144.0];

/// A colour operator from an annotation's colour array (none when it is
/// transparent).
fn color_op(c: &[f64], stroke: bool) -> String {
    let op = match c.len() {
        1 => "g",
        3 => "rg",
        4 => "k",
        _ => return String::new(),
    };
    let nums: Vec<String> = c.iter().map(|v| format!("{v}")).collect();
    format!("{} {}\n", nums.join(" "), if stroke { op.to_uppercase() } else { op.to_string() })
}

/// The font's ascent and descent (thousandths): its descriptor's, else
/// those of the standard font's box, as pdfium's font has them.
fn font_metrics(f: &super::super::file::PdfFile, fd: &Dict) -> (f64, f64) {
    if let Some(desc) = f.key_dict(fd, "FontDescriptor") {
        let (a, d) = (f.key_num(&desc, "Ascent").unwrap_or(0.0), f.key_num(&desc, "Descent").unwrap_or(0.0));
        if a != 0.0 || d != 0.0 {
            return (a, d);
        }
        let b = f.nums(desc.get("FontBBox").unwrap_or(&Obj::Null));
        if b.len() == 4 && (b[1] != 0.0 || b[3] != 0.0) {
            return (b[3], b[1]);
        }
    }
    let base = f.key(fd, "BaseFont").name().unwrap_or("").to_string();
    let base = base.split_once('+').map_or(base.as_str(), |(_, r)| r).to_string();
    let bold = base.contains("Bold");
    let italic = base.contains("Italic") || base.contains("Oblique");
    if base.starts_with("Times") {
        match (bold, italic) {
            (false, false) => (898.0, -218.0),
            (true, false) => (935.0, -218.0),
            (false, true) => (883.0, -217.0),
            (true, true) => (921.0, -218.0),
        }
    } else if base.starts_with("Courier") {
        if bold { (801.0, -250.0) } else { (805.0, -250.0) }
    } else if base.starts_with("Symbol") {
        (1010.0, -293.0)
    } else if base.starts_with("ZapfDingbats") {
        (820.0, -143.0)
    } else if bold {
        (962.0, -228.0)
    } else {
        (931.0, -225.0)
    }
}

/// The text of a field laid out: each line's words as (code, x, width)
/// with the line's baseline.
struct Laid {
    lines: Vec<(f64, Vec<(u8, f64)>)>,
    width: f64,
    height: f64,
}

impl R<'_, '_> {
    /// A field attribute, from the widget or the fields above it.
    pub(super) fn field_attr(&self, ad: &Dict, key: &str) -> Obj {
        let f = self.f;
        let mut node = Some(ad.clone());
        for _ in 0..16 {
            let Some(d) = node else { break };
            let v = f.key(&d, key);
            if !matches!(v, Obj::Null) {
                return v;
            }
            node = f.key_dict(&d, "Parent");
        }
        Obj::Null
    }

    /// The appearance pdfium gives a text field when the form asks for
    /// appearances: its background and border, and the value in the font
    /// and size DA names, as its variable text lays it out.
    pub(super) fn text_field_appearance(&mut self, ad: &Dict) -> Option<Obj> {
        let f = self.f;
        let kind = match self.field_attr(ad, "FT").name() {
            Some("Tx") => 'T',
            Some("Ch") if self.field_attr(ad, "Ff").int().unwrap_or(0) & (1 << 17) != 0 => 'C',
            Some("Ch") => 'L',
            _ => return None,
        };
        let root = f.key_dict(&f.trailer, "Root")?;
        let form = f.key_dict(&root, "AcroForm")?;
        let da_of = |o: Obj| match o {
            Obj::Str(b) => String::from_utf8_lossy(&b).into_owned(),
            _ => String::new(),
        };
        let mut da = da_of(self.field_attr(ad, "DA"));
        if da.is_empty() {
            da = da_of(f.key(&form, "DA"));
        }
        let toks: Vec<&str> = da.split_whitespace().collect();
        // The font (the first Tf) and the text colour (the first g, else
        // rg, else k), as pdfium reads DA.
        let tf = toks.iter().position(|t| *t == "Tf").filter(|&i| i >= 2)?;
        let font_name = toks[tf - 2].strip_prefix('/')?.to_string();
        let size: f64 = toks[tf - 1].parse().ok()?;
        let color = ["g", "rg", "k"].iter().find_map(|op| {
            let n = match *op {
                "g" => 1,
                "rg" => 3,
                _ => 4,
            };
            let i = toks.iter().position(|t| t == op).filter(|&i| i >= n)?;
            Some(toks[i - n..=i].join(" "))
        });
        let dr_fonts = f.key_dict(&form, "DR").and_then(|dr| f.key_dict(&dr, "Font"))?;
        let font_ref = match dr_fonts.get(&font_name) {
            Some(o) => o.clone(),
            None => Obj::Dict(Dict::of(vec![("Type", Obj::name_of("Font")), ("Subtype", Obj::name_of("Type1")), ("BaseFont", Obj::name_of("Helvetica")), ("Encoding", Obj::name_of("WinAnsiEncoding"))])),
        };
        let fd = f.resolve(&font_ref).dict()?.clone();
        if f.key(&fd, "Subtype").name() == Some("Type0") {
            return None;
        }
        let entry = match &font_ref {
            Obj::Ref(n, _) => self.font_obj(Some(*n), &font_ref)?,
            o => self.font_obj(None, o)?,
        };
        // Each character's code and width (thousandths).
        let mut codes: HashMap<char, (u8, f64)> = HashMap::new();
        for code in (1..=255u8).rev() {
            if let Some(g) = entry.font.glyphs(&[code]).first()
                && let [c] = g.text[..]
            {
                codes.insert(c, (code, g.width * 1000.0));
            }
        }
        let (asc, desc) = font_metrics(f, &fd);

        let r = f.nums(ad.get("Rect")?);
        if r.len() != 4 {
            return None;
        }
        let (w, h) = ((r[2] - r[0]).abs(), (r[3] - r[1]).abs());
        let mk = f.key_dict(ad, "MK");
        let rot = mk.as_ref().and_then(|m| f.key_num(m, "R")).unwrap_or(0.0) as i64;
        let (bw, bh, matrix) = match rot.rem_euclid(360) {
            90 => (h, w, [0.0, 1.0, -1.0, 0.0, w, 0.0]),
            180 => (w, h, [-1.0, 0.0, 0.0, -1.0, w, h]),
            270 => (h, w, [0.0, -1.0, 1.0, 0.0, 0.0, h]),
            _ => (w, h, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        };
        // The border.
        let mut width = 1.0;
        let mut style = 'S';
        let mut dash = (3, 0, 0);
        if let Some(bs) = f.key_dict(ad, "BS") {
            if let Some(v) = f.key_num(&bs, "W") {
                width = v;
            }
            style = f.key(&bs, "S").name().and_then(|s| s.chars().next()).unwrap_or('S');
            match style {
                'B' | 'I' => width *= 2.0,
                'D' => {
                    let d = f.nums(bs.get("D").unwrap_or(&Obj::Null));
                    if !d.is_empty() {
                        dash = (d[0] as i64, d.get(1).copied().unwrap_or(0.0) as i64, d.get(2).copied().unwrap_or(0.0) as i64);
                    }
                }
                _ => {}
            }
        }
        let arr = |k: &str| mk.as_ref().map(|m| f.nums(m.get(k).unwrap_or(&Obj::Null))).unwrap_or_default();
        let (bc, bg) = (arr("BC"), arr("BG"));
        let mut s = String::new();
        let bgop = color_op(&bg, false);
        if !bgop.is_empty() {
            let _ = write!(s, "q\n{bgop}0 0 {bw} {bh} re f\nQ\n");
        }
        let border = border_ap((0.0, 0.0, bw, bh), width, &bc, style, dash);
        if !border.is_empty() {
            let _ = write!(s, "q\n{border}Q\n");
        }
        let body = (width, width, bw - width, bh - width);
        let (bx0, by0, bx1, by1) = (body.0.min(body.2), body.1.min(body.3), body.0.max(body.2), body.1.max(body.3));

        let flags = self.field_attr(ad, "Ff").int().unwrap_or(0);
        let color = color.map(|c| c + "\n").unwrap_or_default();
        let text_of = |o: &Obj| match f.resolve(o) {
            Obj::Str(b) => Some(super::super::text_string(&b)),
            _ => None,
        };
        let fit = |size: f64, plate: (f64, f64), multi: bool, lay: &dyn Fn(f64) -> Laid| -> f64 {
            if size.abs() >= 1e-4 {
                return size;
            }
            let steps = if multi { &AUTO_SIZES[..AUTO_SIZES.len() / 4] } else { &AUTO_SIZES[..] };
            let bigger = |sz: f64| {
                let l = lay(sz);
                l.width > plate.0 + 1e-4 || l.height > plate.1 + 1e-4
            };
            let (mut lo, mut hi) = (0i64, steps.len() as i64 - 1);
            let mut mid = steps.len() as i64 / 2;
            while lo <= hi {
                if bigger(steps[mid as usize]) {
                    hi = mid - 1;
                } else {
                    lo = mid + 1;
                }
                mid = (lo + hi) / 2;
            }
            steps[mid.max(0) as usize]
        };
        match kind {
            'T' => {
                let multi = flags & (1 << 12) != 0;
                let password = flags & (1 << 13) != 0;
                let comb = flags & (1 << 24) != 0;
                let max_len = self.field_attr(ad, "MaxLen").int().unwrap_or(0).max(0) as usize;
                let align = self.field_attr(ad, "Q").int().unwrap_or(0);
                let mut value: Vec<char> = text_of(&self.field_attr(ad, "V")).unwrap_or_default().chars().collect();
                if !multi {
                    value.retain(|c| !matches!(c, '\r' | '\n'));
                }
                for c in value.iter_mut() {
                    if *c == '\t' {
                        *c = ' ';
                    }
                }
                if max_len > 0 || comb {
                    value.truncate(max_len);
                }
                if password {
                    value.iter_mut().filter(|c| !matches!(c, '\r' | '\n')).for_each(|c| *c = '*');
                }
                let plate = (bx1 - bx0, by1 - by0);
                let lay = |size: f64| layout(&value, &codes, size, asc, desc, plate, multi, if comb { max_len } else { 0 }, align);
                let size = fit(size, plate, multi, &lay);
                let laid = lay(size);
                // One line: centred on the field's height.
                let dy = if multi { 0.0 } else { (laid.height - plate.1) / 2.0 };
                let text = edit_ap(&laid, (bx0, by1 + dy), comb, &font_name, size);
                if !text.is_empty() {
                    s.push_str("/Tx BMC\nq\n");
                    if laid.width > plate.0 || laid.height > plate.1 {
                        let _ = write!(s, "{bx0} {by0} {} {} re\nW\nn\n", plate.0, plate.1);
                    }
                    let _ = write!(s, "BT\n{color}{text}ET\nQ\nEMC\n");
                }
            }
            'C' => {
                // The value beside a button with an arrow.
                let value: Vec<char> = text_of(&self.field_attr(ad, "V")).unwrap_or_default().chars().collect();
                let (ex1, kx0) = ((bx1 - 13.0).max(bx0), (bx1 - 13.0).min(bx1));
                let plate = (ex1 - bx0, by1 - by0);
                let lay = |size: f64| layout(&value, &codes, size, asc, desc, plate, false, 0, 0);
                let size = fit(size, plate, false, &lay);
                let laid = lay(size);
                let dy = (laid.height - plate.1) / 2.0;
                let text = edit_ap(&laid, (bx0, by1 + dy), false, &font_name, size);
                if !text.is_empty() {
                    let _ = write!(s, "/Tx BMC\nq\n{bx0} {by0} {} {} re\nW\nn\nBT\n{color}{text}ET\nQ\nEMC\n", plate.0, plate.1);
                }
                let (kw, kh) = (bx1 - kx0, by1 - by0);
                if kw > 0.0 && kh > 0.0 {
                    let grey = color_op(&[220.0 / 255.0, 220.0 / 255.0, 220.0 / 255.0], false);
                    let _ = write!(s, "q\n{grey}{kx0} {by0} {kw} {kh} re f\nQ\n");
                    let _ = write!(s, "q\n{}Q\n", border_ap((kx0, by0, bx1, by1), 2.0, &[0.0], 'B', (3, 0, 0)));
                    if kw > 6.0 && kh > 6.0 {
                        let (cx, cy) = ((kx0 + bx1) / 2.0, (by0 + by1) / 2.0);
                        let _ = write!(s, "q\n 0 g\n{} {} m\n{} {} l\n{cx} {} l\n{} {} l f\n{grey}Q\n", cx - 3.0, cy + 1.5, cx + 3.0, cy + 1.5, cy - 1.5, cx - 3.0, cy + 1.5);
                    }
                }
            }
            _ => {
                // Rows from the top index, those of V's values highlighted
                // (pdfium's appearance leaves I aside).
                let Obj::Array(opts) = f.resolve(&self.field_attr(ad, "Opt")) else { return None };
                let export = |o: &Obj| match f.resolve(o) {
                    Obj::Array(p) => p.first().and_then(text_of),
                    o => text_of(&o),
                };
                let index = |o: &Obj| {
                    let v = text_of(o)?;
                    opts.iter().position(|p| export(p).as_ref() == Some(&v)).map(|i| i as i64)
                };
                let sel: Vec<i64> = match f.resolve(&self.field_attr(ad, "V")) {
                    Obj::Array(a) => a.iter().filter_map(index).collect(),
                    v => index(&v).into_iter().collect(),
                };
                let top = self.field_attr(ad, "TI").int().unwrap_or(0).max(0) as usize;
                let size = if size.abs() < 1e-4 { 12.0 } else { size };
                let mut body = String::new();
                let mut fy = by1;
                for (i, o) in opts.iter().enumerate().skip(top) {
                    if fy < by0 {
                        break;
                    }
                    let item = match f.resolve(o) {
                        Obj::Array(p) => p.get(1).and_then(text_of),
                        o => text_of(&o),
                    }
                    .unwrap_or_default();
                    let value: Vec<char> = item.chars().collect();
                    let laid = layout(&value, &codes, size, asc, desc, (bx1 - bx0, 0.0), false, 0, 0);
                    let text = edit_ap(&laid, (bx0, fy), false, &font_name, size);
                    if sel.contains(&(i as i64)) {
                        let blue = color_op(&[0.0, 51.0 / 255.0, 113.0 / 255.0], false);
                        let _ = write!(body, "q\n{blue}{bx0} {} {} {} re f\nQ\nBT\n1 g\n{text}ET\n", fy - laid.height, bx1 - bx0, laid.height);
                    } else {
                        let _ = write!(body, "BT\n{color}{text}ET\n");
                    }
                    fy -= laid.height;
                }
                if !body.is_empty() {
                    let _ = write!(s, "/Tx BMC\nq\n{bx0} {by0} {} {} re\nW\nn\n{body}Q\nEMC\n", bx1 - bx0, by1 - by0);
                }
            }
        }
        let mut fonts = Dict::default();
        fonts.set(&font_name, font_ref);
        let mut res = Dict::default();
        res.set("Font", Obj::Dict(fonts));
        let mut dict = Dict::default();
        dict.set("BBox", Obj::Array(vec![Obj::Real(0.0), Obj::Real(0.0), Obj::Real(bw), Obj::Real(bh)]));
        dict.set("Matrix", Obj::Array(matrix.iter().map(|v| Obj::Real(*v)).collect()));
        dict.set("Resources", Obj::Dict(res));
        Some(Obj::Stream(dict, Arc::from(s.into_bytes())))
    }
}

impl R<'_, '_> {
    /// The appearance pdfium gives a check box that has none when the form
    /// asks for appearances: its background and border, and when it is on
    /// a check mark (CPWL's) in the box less twice the border's width.
    pub(super) fn check_box_appearance(&self, ad: &Dict) -> Option<Obj> {
        let f = self.f;
        let flags = self.field_attr(ad, "Ff").int().unwrap_or(0);
        if self.field_attr(ad, "FT").name() != Some("Btn") || flags & (3 << 15) != 0 {
            return None;
        }
        let r = f.nums(ad.get("Rect")?);
        if r.len() != 4 {
            return None;
        }
        let (w, h) = ((r[2] - r[0]).abs(), (r[3] - r[1]).abs());
        let mk = f.key_dict(ad, "MK");
        let arr = |k: &str| mk.as_ref().map(|m| f.nums(m.get(k).unwrap_or(&Obj::Null))).unwrap_or_default();
        let width = f.key_dict(ad, "BS").and_then(|bs| f.key_num(&bs, "W")).unwrap_or(1.0);
        let mut s = String::new();
        let bg = color_op(&arr("BG"), false);
        if !bg.is_empty() {
            let _ = write!(s, "q\n{bg}0 0 {w} {h} re f\nQ\n");
        }
        let border = border_ap((0.0, 0.0, w, h), width, &arr("BC"), 'S', (3, 0, 0));
        if !border.is_empty() {
            let _ = write!(s, "q\n{border}Q\n");
        }
        let on = f.key(ad, "AS").name().is_some_and(|n| n != "Off");
        if on {
            let d = width * 2.0;
            let (x0, y0, cw, ch) = (d, d, (w - 2.0 * d).max(0.0), (h - 2.0 * d).max(0.0));
            const PTS: [[(f64, f64); 3]; 8] = [
                [(0.28, 0.52), (0.27, 0.48), (0.29, 0.40)],
                [(0.30, 0.33), (0.31, 0.29), (0.31, 0.28)],
                [(0.39, 0.28), (0.49, 0.29), (0.77, 0.67)],
                [(0.76, 0.68), (0.78, 0.69), (0.76, 0.75)],
                [(0.76, 0.75), (0.73, 0.80), (0.68, 0.75)],
                [(0.68, 0.74), (0.68, 0.74), (0.44, 0.47)],
                [(0.43, 0.47), (0.40, 0.47), (0.41, 0.58)],
                [(0.40, 0.60), (0.28, 0.66), (0.30, 0.56)],
            ];
            const K: f64 = 0.5522847498308;
            let p = |(x, y): (f64, f64)| (x0 + x * cw, y0 + y * ch);
            let start = p(PTS[0][0]);
            let _ = write!(s, "0 g\n{} {} m\n", start.0, start.1);
            for i in 0..8 {
                let (a, b, c) = (p(PTS[i][0]), p(PTS[i][1]), p(PTS[i][2]));
                let next = p(PTS[(i + 1) % 8][0]);
                let _ = writeln!(s, "{} {} {} {} {} {} c", a.0 + (b.0 - a.0) * K, a.1 + (b.1 - a.1) * K, next.0 + (c.0 - next.0) * K, next.1 + (c.1 - next.1) * K, next.0, next.1);
            }
            s.push_str("f\n");
        }
        let mut dict = Dict::default();
        dict.set("BBox", Obj::Array(vec![Obj::Real(0.0), Obj::Real(0.0), Obj::Real(w), Obj::Real(h)]));
        Some(Obj::Stream(dict, Arc::from(s.into_bytes())))
    }
}

/// pdfium's text of laid out lines, from the plate's top left corner
/// (`at`): a move to each line (each character of a comb) and its codes.
fn edit_ap(laid: &Laid, at: (f64, f64), comb: bool, font: &str, size: f64) -> String {
    let mut text = String::new();
    let mut old = (0.0, 0.0);
    let mut set_font = false;
    for (base, words) in &laid.lines {
        let Some(&(_, x0)) = words.first() else { continue };
        let runs: Vec<&[(u8, f64)]> = if comb { words.chunks(1).collect() } else { vec![&words[..]] };
        for run in runs {
            let x = if comb { run[0].1 } else { x0 };
            let p = (at.0 + x, at.1 - base);
            if comb || p != old {
                let _ = writeln!(text, "{} {} Td", fmt(p.0 - old.0), fmt(p.1 - old.1));
                old = p;
            }
            if !set_font {
                let _ = writeln!(text, "/{font} {} Tf", fmt(size));
                set_font = true;
            }
            let hex: String = run.iter().map(|(c, _)| format!("{c:02X}")).collect();
            let _ = writeln!(text, "<{hex}> Tj");
        }
    }
    text
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// pdfium's border of a generated field appearance.
fn border_ap((l, b, r, t): (f64, f64, f64, f64), bw: f64, color: &[f64], style: char, dash: (i64, i64, i64)) -> String {
    let mut s = String::new();
    if bw <= 0.0 {
        return s;
    }
    let hw = bw / 2.0;
    match style {
        'D' => {
            let c = color_op(color, true);
            if !c.is_empty() {
                let _ = write!(s, "{c}{bw} w [{} {}] {} d\n", dash.0, dash.1, dash.2);
                let _ = write!(s, "{} {} m\n{} {} l\n{} {} l\n{} {} l\n{} {} l S\n", l + hw, b + hw, l + hw, t - hw, r - hw, t - hw, r - hw, b + hw, l + hw, b + hw);
            }
        }
        'B' | 'I' => {
            let (lt, rb): (&[f64], &[f64]) = if style == 'B' { (&[1.0], &[0.5]) } else { (&[0.5], &[0.75]) };
            let _ = write!(s, "{}{} {} m\n{} {} l\n{} {} l\n{} {} l\n{} {} l\n{} {} l f\n", color_op(lt, false), l + hw, b + hw, l + hw, t - hw, r - hw, t - hw, r - bw, t - bw, l + bw, t - bw, l + bw, b + bw);
            let _ = write!(s, "{}{} {} m\n{} {} l\n{} {} l\n{} {} l\n{} {} l\n{} {} l f\n", color_op(rb, false), r - hw, t - hw, r - hw, b + hw, l + hw, b + hw, l + bw, b + bw, r - bw, b + bw, r - bw, t - bw);
            let c = color_op(color, false);
            if !c.is_empty() {
                let _ = write!(s, "{c}{l} {b} {} {} re\n{} {} {} {} re f*\n", r - l, t - b, l + hw, b + hw, r - l - bw, t - b - bw);
            }
        }
        'U' => {
            let c = color_op(color, true);
            if !c.is_empty() {
                let _ = write!(s, "{c}{bw} w\n{l} {} m\n{r} {} l S\n", b + hw, b + hw);
            }
        }
        _ => {
            let c = color_op(color, false);
            if !c.is_empty() {
                let _ = write!(s, "{c}{l} {b} {} {} re\n{} {} {} {} re\nf*\n", r - l, t - b, l + bw, b + bw, r - l - bw * 2.0, t - b - bw * 2.0);
            }
        }
    }
    s
}

/// The value laid out in the plate (width, height) at a size: lines from
/// the top, each word wrapped onto the next when it doesn't fit (several
/// lines), aligned (0 left, 1 centre, 2 right), or one character to each
/// of `cells` cells (a comb).
#[allow(clippy::too_many_arguments)]
fn layout(text: &[char], codes: &HashMap<char, (u8, f64)>, size: f64, asc: f64, desc: f64, plate: (f64, f64), multi: bool, cells: usize, align: i64) -> Laid {
    let k = size / 1000.0;
    let (a, d) = (asc * k, desc * k);
    let line_h = a - d;
    let glyph = |c: char| codes.get(&c).map(|&(code, w)| (code, w * k));
    if cells > 0 {
        let node = plate.0 / cells as f64;
        let n = text.iter().filter(|c| glyph(**c).is_some()).count();
        let start = match align {
            1 => (cells as i64 - n as i64) / 2,
            2 => cells as i64 - n as i64,
            _ => 0,
        };
        let words: Vec<(u8, f64)> = text.iter().filter_map(|c| glyph(*c)).enumerate().map(|(i, (code, w))| (code, node * (start + i as i64) as f64 + (node - w) / 2.0)).collect();
        return Laid { lines: vec![(a, words)], width: plate.0, height: line_h };
    }
    // Paragraphs, then lines.
    let mut lines: Vec<Vec<(u8, f64)>> = Vec::new();
    let paras: Vec<&[char]> = if multi {
        let mut v = Vec::new();
        let mut start = 0;
        let mut i = 0;
        while i < text.len() {
            if matches!(text[i], '\r' | '\n') {
                v.push(&text[start..i]);
                if text[i] == '\r' && text.get(i + 1) == Some(&'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            i += 1;
        }
        v.push(&text[start..]);
        v
    } else {
        vec![text]
    };
    for p in paras {
        let gl: Vec<(char, u8, f64)> = p.iter().filter_map(|&c| glyph(c).map(|(code, w)| (c, code, w))).collect();
        if !multi {
            lines.push(gl.iter().map(|&(_, c, w)| (c, w)).collect());
            continue;
        }
        let mut line: Vec<(u8, f64)> = Vec::new();
        let mut lw = 0.0;
        let mut i = 0;
        while i < gl.len() {
            // A word: the characters up to and with the spaces after it.
            let mut j = i;
            while j < gl.len() && gl[j].0 != ' ' {
                j += 1;
            }
            while j < gl.len() && gl[j].0 == ' ' {
                j += 1;
            }
            let ww: f64 = gl[i..j].iter().map(|g| g.2).sum();
            let ink: f64 = gl[i..j].iter().filter(|g| g.0 != ' ').map(|g| g.2).sum();
            if !line.is_empty() && lw + ink > plate.0 {
                lines.push(std::mem::take(&mut line));
                lw = 0.0;
            }
            for g in &gl[i..j] {
                if !line.is_empty() && g.0 != ' ' && lw + g.2 > plate.0 {
                    lines.push(std::mem::take(&mut line));
                    lw = 0.0;
                }
                line.push((g.1, g.2));
                lw += g.2;
            }
            let _ = ww;
            i = j;
        }
        lines.push(line);
    }
    let mut out = Vec::new();
    let mut width: f64 = 0.0;
    for (n, l) in lines.into_iter().enumerate() {
        let lw: f64 = l.iter().map(|g| g.1).sum();
        width = width.max(lw);
        let x0 = match align {
            1 => (plate.0 - lw) / 2.0,
            2 => plate.0 - lw,
            _ => 0.0,
        };
        let mut x = x0;
        let words = l
            .into_iter()
            .map(|(c, w)| {
                let p = (c, x);
                x += w;
                p
            })
            .collect();
        out.push((a + n as f64 * line_h, words));
    }
    let height = out.len() as f64 * line_h;
    Laid { lines: out, width, height }
}
