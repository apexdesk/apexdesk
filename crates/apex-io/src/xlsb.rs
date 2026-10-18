//! Excel binary workbooks (.xlsb, BIFF12), read only: cells, formulas,
//! formats, merged cells, column widths and row heights, frozen panes,
//! notes, hyperlinks, defined names, conditional formats, data validation,
//! autofilters, and the pictures and charts of drawing parts (XML, as in
//! .xlsx).

use std::collections::HashMap;
use std::sync::Arc;

use apex_sheet::formula::parse_canonical;
use apex_sheet::sheet::ROTATION_STACKED;
use apex_sheet::data::CfStyle;
use apex_sheet::{Borders, CellRef, CellStyle, Content, HAlign, Line, LineStyle, Link, Note, Range, Theme, ThemeColor, VAlign, Value, Workbook};

use crate::Error;
use crate::biff::{self, Cur, Refs};
use crate::xlsb_data::Color;
use crate::xlsx::resolve_target;
use crate::zip::ZipReader;

/// The records of a part: (type, data).
fn records(d: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    let mut p = 0;
    let var = |p: &mut usize, bytes: usize| -> Option<u32> {
        let mut v = 0u32;
        for i in 0..bytes {
            let b = *d.get(*p)?;
            *p += 1;
            v |= u32::from(b & 0x7F) << (7 * i);
            if b & 0x80 == 0 {
                break;
            }
        }
        Some(v)
    };
    while p < d.len() {
        // The type keeps its continuation bits (1 or 2 bytes).
        let Some(&b0) = d.get(p) else { break };
        let ty = if b0 & 0x80 != 0 {
            let Some(&b1) = d.get(p + 1) else { break };
            p += 2;
            u32::from(b0 & 0x7F) | u32::from(b1 & 0x7F) << 7
        } else {
            p += 1;
            u32::from(b0)
        };
        let Some(len) = var(&mut p, 4) else { break };
        let Some(data) = d.get(p..p + len as usize) else { break };
        p += len as usize;
        out.push((ty, data));
    }
    out
}

/// `XLWideString`: a 32-bit length and UTF-16.
fn wide(c: &mut Cur) -> Option<String> {
    let n = c.u32()? as usize;
    if n == 0xFFFF_FFFF {
        return Some(String::new());
    }
    c.utf16(n.min(c.left() / 2))
}

/// A color (`BrtColor`).
fn color(c: &mut Cur, theme: &Theme, palette: &[u32; 56]) -> Option<Option<(u32, Option<ThemeColor>)>> {
    let flags = c.u8()?;
    let index = c.u8()?;
    let tint = c.u16()? as i16;
    let (r, g, b, _) = (c.u8()?, c.u8()?, c.u8()?, c.u8()?);
    let rgb = u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b);
    Some(match flags >> 1 {
        1 => match index {
            0..=7 => Some(([0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF][index as usize], None)),
            8..=63 => Some((palette[index as usize - 8], None)),
            _ => None,
        },
        2 => Some((rgb, None)),
        3 => crate::xlsx_styles::theme_color(theme, index as usize, f64::from(tint) / 32767.0),
        _ => None,
    })
}

struct Font {
    name: String,
    size: u16,
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    color: Option<(u32, Option<ThemeColor>)>,
    scheme: u8,
}

fn read_styles(d: &[u8], wb: &mut Workbook, theme: &Theme) -> Vec<u32> {
    let palette = biff::PALETTE;
    let mut formats: HashMap<u32, String> = HashMap::new();
    let mut fonts: Vec<Font> = Vec::new();
    let mut fills: Vec<Option<(u32, Option<ThemeColor>)>> = Vec::new();
    let mut borders: Vec<(Borders, [Option<ThemeColor>; 4])> = Vec::new();
    let mut xfs = Vec::new();
    let mut in_cell_xfs = false;
    for (ty, data) in records(d) {
        let mut c = Cur::new(data);
        match ty {
            0x0269 => in_cell_xfs = true,
            0x026A => in_cell_xfs = false,
            0x002C => {
                if let (Some(id), Some(code)) = (c.u16(), wide(&mut c)) {
                    formats.insert(u32::from(id), short(code));
                }
            }
            0x002B => {
                let f = (|| {
                    let size = c.u16()?;
                    let flags = c.u16()?;
                    let weight = c.u16()?;
                    c.skip(2)?;
                    let underline = c.u8()? != 0;
                    c.skip(3)?;
                    let color = color(&mut c, theme, &palette)?;
                    let scheme = c.u8()?;
                    let name = short(wide(&mut c)?);
                    Some(Font { name, size, bold: weight >= 600, italic: flags & 2 != 0, strike: flags & 8 != 0, underline, color, scheme })
                })();
                fonts.push(f.unwrap_or(Font { name: String::new(), size: 220, bold: false, italic: false, strike: false, underline: false, color: None, scheme: 0 }));
            }
            0x002D => {
                let fill = (|| {
                    let pattern = c.u32()?;
                    let fore = color(&mut c, theme, &palette)?;
                    Some(if pattern == 1 { fore } else { None })
                })();
                fills.push(fill.flatten());
            }
            0x002E => {
                let b = (|| {
                    c.skip(1)?;
                    let mut b = Borders::NONE;
                    let mut refs = [None; 4];
                    // Top, bottom, left, right.
                    for (i, side) in [&mut b.top, &mut b.bottom, &mut b.left, &mut b.right].into_iter().enumerate() {
                        let dg = c.u8()?;
                        c.skip(1)?;
                        let col = color(&mut c, theme, &palette)?;
                        let st = match dg {
                            0 => LineStyle::None,
                            1 => LineStyle::Thin,
                            2 | 8 | 10 | 12 => LineStyle::Medium,
                            3 | 9 | 11 | 13 => LineStyle::Dashed,
                            4 => LineStyle::Dotted,
                            5 => LineStyle::Thick,
                            6 => LineStyle::Double,
                            _ => LineStyle::Hair,
                        };
                        let rgb = col.filter(|&(c, tc)| st != LineStyle::None && (c != 0 || tc.is_some()));
                        *side = Line::new(st, rgb.map(|x| x.0));
                        refs[i] = rgb.and_then(|x| x.1);
                    }
                    Some((b, refs))
                })();
                borders.push(b.unwrap_or((Borders::NONE, [None; 4])));
            }
            // As many formats as Excel keeps.
            0x002F if in_cell_xfs && xfs.len() < 64_000 => xfs.push(data.to_vec()),
            _ => {}
        }
    }
    let base = fonts.first().map(|f| (f.name.clone(), f.size));
    let mut out = Vec::new();
    for x in xfs {
        let mut c = Cur::new(&x);
        let mut s = CellStyle::default();
        let parsed = (|| {
            c.skip(2)?;
            let fmt = c.u16()?;
            let font = c.u16()?;
            let fill = c.u16()?;
            let border = c.u16()?;
            let rot = c.u8()?;
            let indent = c.u8()?;
            let flags = c.u32()?;
            Some((fmt, font, fill, border, rot, indent, flags))
        })();
        if let Some((fmt, font, fill, border, rot, indent, flags)) = parsed {
            s.num_fmt = Arc::from(crate::xlsx_styles::numfmt_code(u32::from(fmt), &formats, wb));
            if let Some(f) = fonts.get(font as usize) {
                if base.as_ref().is_some_and(|b| b.0 != f.name) || f.scheme == 1 {
                    s.font = Some(Arc::from(f.name.as_str()));
                    s.theme.font = match f.scheme {
                        1 => apex_sheet::sheet::THEME_MAJOR,
                        2 => apex_sheet::sheet::THEME_MINOR,
                        _ => 0,
                    };
                }
                if base.as_ref().is_some_and(|b| b.1 != f.size) && f.size > 0 {
                    s.font_size = f.size / 2;
                }
                s.bold = f.bold;
                s.italic = f.italic;
                s.strike = f.strike;
                s.underline = f.underline;
                if let Some((c, tc)) = f.color.filter(|&(c, _)| c != 0) {
                    s.color = Some(c);
                    s.theme.color = tc;
                }
            }
            if let Some(Some((c, tc))) = fills.get(fill as usize) {
                s.fill = Some(*c);
                s.theme.fill = *tc;
            }
            if let Some((b, refs)) = borders.get(border as usize) {
                s.borders = *b;
                s.theme.borders = *refs;
            }
            s.rotation = match rot {
                255 => ROTATION_STACKED,
                v @ 1..=90 => i16::from(v),
                v @ 91..=180 => -(i16::from(v) - 90),
                _ => 0,
            };
            s.indent = indent.min(15);
            s.halign = match flags & 7 {
                1 => HAlign::Left,
                2 | 6 => HAlign::Center,
                3 => HAlign::Right,
                5 | 7 => HAlign::Justify,
                _ => HAlign::General,
            };
            s.valign = match (flags >> 3) & 7 {
                0 => VAlign::Top,
                1 | 3 | 4 => VAlign::Center,
                _ => VAlign::Bottom,
            };
            s.wrap = flags & 0x40 != 0;
            s.unlocked = flags & 0x1000 == 0;
            s.formula_hidden = flags & 0x2000 != 0;
        }
        out.push(wb.styles.intern(s));
    }
    out
}

/// A cell record's position and style.
fn cell_head(c: &mut Cur, row: u32) -> Option<(CellRef, u32)> {
    let col = c.u32()?;
    let style = c.u32()? & 0xFF_FFFF;
    Some((CellRef::new(row, col), style))
}

/// `CellParsedFormula`: tokens and extra data.
fn parsed_formula(c: &mut Cur) -> Option<(Vec<u8>, Vec<u8>)> {
    let cce = c.u32()? as usize;
    let rgce = c.bytes(cce)?.to_vec();
    let cb = c.u32()? as usize;
    let rgcb = c.bytes(cb.min(c.left()))?.to_vec();
    Some((rgce, rgcb))
}

fn rfx(c: &mut Cur) -> Option<Range> {
    let (r1, r2, c1, c2) = (c.u32()?, c.u32()?, c.u32()?, c.u32()?);
    Some(Range::new(CellRef::new(r1, c1), CellRef::new(r2, c2)))
}

struct Book {
    sheets: Vec<String>,
    /// Supporting books: true for this workbook.
    own: Vec<bool>,
    xti: Vec<(u32, i32, i32)>,
    names: Vec<String>,
}

impl Book {
    fn sheet_ref(&self, ixti: u16) -> Option<String> {
        let &(book, first, last) = self.xti.get(ixti as usize)?;
        if !*self.own.get(book as usize)? || first < 0 || last < 0 {
            return None;
        }
        let (a, b) = (self.sheets.get(first as usize)?, self.sheets.get(last as usize)?);
        Some(if a == b { biff::quote_sheet(a) } else { biff::quote_sheet(&format!("{a}:{b}")) })
    }

    fn name(&self, i: u32) -> Option<String> {
        self.names.get((i as usize).checked_sub(1)?).cloned().filter(|n| !n.is_empty())
    }
}

/// A name, format code or author: at most 255 characters, as Excel.
fn short(s: String) -> String {
    if s.len() <= 255 { s } else { s.chars().take(255).collect() }
}

/// The workbook in an .xlsb file.
pub fn read(data: &[u8]) -> Result<Workbook, Error> {
    let z = ZipReader::new(data)?;
    let mut wb = Workbook::empty();
    let rels = z.read_string("xl/_rels/workbook.bin.rels").map(|s| crate::xlsx::read_rels(&s)).unwrap_or_default();
    let theme = rels
        .values()
        .find(|t| t.contains("theme/"))
        .and_then(|t| z.read_string(&resolve_target("xl", t)).ok())
        .map(|s| crate::theme_xml::read_theme(&s))
        .unwrap_or_default();
    wb.cell_styles = apex_sheet::CellStyles::new(theme.clone(), &wb.locale);
    let styles = z.read("xl/styles.bin").unwrap_or_default();
    let xfs = read_styles(&styles, &mut wb, &theme);
    let rgb = |c: &mut Cur| color(c, &theme, &biff::PALETTE).map(|o| o.map(|x| x.0));
    let dxfs = crate::xlsb_data::read_dxfs(&records(&styles), &rgb);
    let strings: Vec<Arc<str>> = z
        .read("xl/sharedStrings.bin")
        .map(|d| {
            records(&d)
                .into_iter()
                .filter(|(ty, _)| *ty == 0x0013)
                .map(|(_, data)| {
                    let mut c = Cur::new(data);
                    c.skip(1);
                    Arc::from(wide(&mut c).unwrap_or_default())
                })
                .collect()
        })
        .unwrap_or_default();
    let wbd = z.read("xl/workbook.bin")?;
    let mut book = Book { sheets: Vec::new(), own: Vec::new(), xti: Vec::new(), names: Vec::new() };
    // Sheet name, part, hidden.
    let mut parts: Vec<(String, String, bool)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut seen_parts = std::collections::HashSet::new();
    // Name, local sheet, tokens, extra, hidden.
    let mut defined: Vec<(String, Option<usize>, Vec<u8>, Vec<u8>, bool)> = Vec::new();
    let mut date1904 = false;
    for (ty, data) in records(&wbd) {
        let mut c = Cur::new(data);
        match ty {
            0x0099 => date1904 = c.u32().is_some_and(|f| f & 1 != 0),
            0x009C => {
                let r = (|| {
                    let state = c.u32()?;
                    c.skip(4)?;
                    let rel = wide(&mut c)?;
                    let name = short(wide(&mut c)?);
                    Some((name, rel, state != 0))
                })();
                if let Some((name, rel, hidden)) = r {
                    let part = rels.get(&rel).map(|t| resolve_target("xl", t)).unwrap_or_default();
                    book.sheets.push(name.clone());
                    // A sheet listed again (same name or same part) is
                    // read once, as Excel repairs it.
                    if !seen.contains(&name.to_lowercase()) && seen_parts.insert(part.to_lowercase()) {
                        seen.insert(name.to_lowercase());
                        parts.push((name, part, hidden));
                    }
                }
            }
            0x0162 | 0x0163 => book.own.push(true),
            0x0165 | 0x0166 => book.own.push(false),
            0x016A => {
                let n = c.u32().unwrap_or(0);
                for _ in 0..n.min(65_536) {
                    let (Some(b), Some(f), Some(l)) = (c.u32(), c.i32(), c.i32()) else { break };
                    book.xti.push((b, f, l));
                }
            }
            0x0027 => {
                let n = (|| {
                    let flags = c.u32()?;
                    c.skip(1)?;
                    let itab = c.u32()?;
                    let mut name = short(wide(&mut c)?);
                    if flags & 0x20 != 0 && !name.starts_with("_xlnm.") {
                        name = format!("_xlnm.{name}");
                    }
                    let (rgce, rgcb) = parsed_formula(&mut c)?;
                    Some((name, (itab != 0xFFFF_FFFF).then_some(itab as usize), rgce, rgcb, flags & 1 != 0))
                })();
                let n = n.unwrap_or_default();
                book.names.push(n.0.strip_prefix("_xlnm.").unwrap_or(&n.0).to_string());
                defined.push(n);
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        return Err(Error::new("the workbook has no sheets"));
    }
    for (name, _, hidden) in &parts {
        let mut sh = apex_sheet::Sheet::new(name.clone());
        sh.hidden = *hidden;
        wb.sheets.push(sh);
    }
    crate::xlsx_chart::set_theme(Some(&theme));
    let mut chart_points = crate::xlsx_drawing::BOOK_CHART_POINTS;
    for (si, (_, part, _)) in parts.iter().enumerate() {
        if let Ok(d) = z.read(part) {
            read_sheet(&z, part, &d, &mut wb, si, &book, &strings, &xfs, (&dxfs, &rgb), &mut chart_points);
        }
    }
    crate::xlsx_chart::set_theme(None);
    let mut user = Vec::new();
    for (name, local, rgce, rgcb, hidden) in defined {
        let sheet = |i: u32| book.sheet_ref(i as u16);
        let name_of = |i: u32| book.name(i);
        let ext = |_: u16, i: u32| book.name(i);
        let r = Refs { biff12: true, biff5: false, base: CellRef::new(0, 0), in_name: false, sheet: &sheet, name: &name_of, extern_name: &ext };
        let Some(text) = biff::decompile(&rgce, &rgcb, &r) else { continue };
        let local = local.filter(|&i| i < wb.sheets.len());
        if name.starts_with("_xlnm.Print_") {
            crate::xlsx_page::apply_defined_name(&mut wb, &name, local, &text);
        } else if !name.starts_with("_xlnm.") && !name.starts_with("_xlfn.") && !name.is_empty() {
            user.push((name, local, text, String::new(), hidden));
        }
    }
    crate::xlsx::read_user_names(&mut wb, user);
    wb.refresh_cell_styles();
    if date1904 {
        wb.shift_dates(apex_sheet::workbook::DAYS_1904);
    }
    wb.meta = crate::props::read_ooxml(&z);
    wb.macros = crate::macro_parts::read(&z, "xl", &z.read_string("xl/_rels/workbook.bin.rels").unwrap_or_default());
    wb.finish_load();
    Ok(wb)
}

#[allow(clippy::too_many_arguments)]
fn read_sheet(z: &ZipReader, part: &str, d: &[u8], wb: &mut Workbook, si: usize, book: &Book, strings: &[Arc<str>], xfs: &[u32], (dxfs, rgb): (&[CfStyle], Color), chart_points: &mut usize) {
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    let sheet = |i: u32| book.sheet_ref(i as u16);
    let name_of = |i: u32| book.name(i);
    let ext = |_: u16, i: u32| book.name(i);
    let fmla = |rgce: &[u8], base: CellRef| biff::decompile(rgce, &[], &Refs { biff12: true, biff5: false, base, in_name: false, sheet: &sheet, name: &name_of, extern_name: &ext });
    let mut tools = crate::xlsb_data::Tools::default();
    let rels = z.read_string(&format!("{dir}/_rels/{file}.rels")).map(|s| crate::xlsx_notes::read_rels(&s)).unwrap_or_default();
    let style_of = |i: u32| xfs.get(i as usize).copied().unwrap_or(0);
    let mut row = 0u32;
    // Cell, style, cached value, tokens and extra data (or the row of a
    // shared formula).
    let mut formulas: Vec<(CellRef, u32, Value, Option<(Vec<u8>, Vec<u8>)>, Option<u32>)> = Vec::new();
    let mut shared: Vec<(Range, Vec<u8>, Vec<u8>, bool)> = Vec::new();
    // Column records read: a sheet's worth.
    let mut col_infos = 0u32;
    let mut links = Vec::new();
    let mut link_bytes = 0usize;
    for (ty, data) in records(d) {
        let mut c = Cur::new(data);
        match ty {
            0x0000 => {
                let r = (|| {
                    let r = c.u32()?;
                    c.skip(4)?;
                    let h = c.u16()?;
                    c.skip(1)?;
                    let flags = c.u8()?;
                    Some((r, h, flags))
                })();
                if let Some((r, h, flags)) = r {
                    row = r;
                    let sh = &mut wb.sheets[si];
                    let level = flags & 7;
                    if level > 0 {
                        sh.row_levels.insert(r, level);
                    }
                    if flags & 0x10 != 0 {
                        sh.row_heights.insert(r, 0.0);
                    } else if flags & 0x20 != 0 {
                        sh.row_heights.insert(r, (f32::from(h) / 20.0 * crate::xlsx::ROW_SCALE).round());
                    }
                }
            }
            0x0001..=0x0007 => {
                let Some((cell, xf)) = cell_head(&mut c, row) else { continue };
                let v = match ty {
                    0x0002 => c.u32().map(|v| Value::Number(rk(v))),
                    0x0003 => c.u8().map(|e| Value::Error(biff::error_kind(e))),
                    0x0004 => c.u8().map(|b| Value::Bool(b != 0)),
                    0x0005 => c.f64().map(Value::Number),
                    0x0006 => wide(&mut c).map(Value::text),
                    0x0007 => c.u32().and_then(|i| strings.get(i as usize)).map(|s| Value::Text(s.clone())),
                    _ => Some(Value::Empty),
                };
                let content = match v.unwrap_or(Value::Empty) {
                    Value::Empty => Content::Empty,
                    v => Content::Value(v),
                };
                let style = style_of(xf);
                if !matches!(content, Content::Empty) || style != 0 {
                    wb.load_cell(si, cell, content, style);
                }
            }
            0x0008..=0x000B => {
                let f = (|| {
                    let (cell, xf) = cell_head(&mut c, row)?;
                    let v = match ty {
                        0x0008 => Value::text(wide(&mut c)?),
                        0x0009 => Value::Number(c.f64()?),
                        0x000A => Value::Bool(c.u8()? != 0),
                        _ => Value::Error(biff::error_kind(c.u8()?)),
                    };
                    c.skip(2)?;
                    let (rgce, rgcb) = parsed_formula(&mut c)?;
                    let exp = (rgce.len() == 5 && rgce[0] == 0x01).then(|| u32::from_le_bytes([rgce[1], rgce[2], rgce[3], rgce[4]]));
                    Some((cell, style_of(xf), v, if exp.is_some() { None } else { Some((rgce, rgcb)) }, exp))
                })();
                if let Some(f) = f {
                    formulas.push(f);
                }
            }
            0x01AA | 0x01AB => {
                let f = (|| {
                    let area = rfx(&mut c)?;
                    if ty == 0x01AA {
                        c.skip(1)?;
                    }
                    let (rgce, rgcb) = parsed_formula(&mut c)?;
                    Some((area, rgce, rgcb, ty == 0x01AA))
                })();
                if let Some(f) = f {
                    shared.push(f);
                }
            }
            0x003C if col_infos < 16_384 => {
                col_infos += 1;
                if let (Some(first), Some(last), Some(w), Some(_), Some(flags)) = (c.u32(), c.u32(), c.u32(), c.u32(), c.u16()) {
                    let sh = &mut wb.sheets[si];
                    let level = ((flags >> 8) & 7) as u8;
                    for col in first..=last.min(first.saturating_add(2000)).min(16_383) {
                        if level > 0 {
                            sh.col_levels.insert(col, level);
                        }
                        if flags & 1 != 0 {
                            sh.col_widths.insert(col, 0.0);
                        } else {
                            let px = crate::xlsx::width_from_chars(w as f32 / 256.0);
                            if (px - sh.default_col_width).abs() > 1.0 {
                                sh.col_widths.insert(col, px);
                            }
                        }
                    }
                }
            }
            0x01E5 => {
                if let (Some(_), Some(w), Some(h)) = (c.u32(), c.u16(), c.u16()) {
                    let sh = &mut wb.sheets[si];
                    if w > 0 {
                        sh.default_col_width = crate::xlsx::default_size(crate::xlsx::width_from_chars(f32::from(w) + 0.71), apex_sheet::sheet::DEFAULT_COL_WIDTH);
                    }
                    if h > 0 {
                        sh.default_row_height = (f32::from(h) / 20.0 * crate::xlsx::ROW_SCALE).round().max(8.0);
                    }
                }
            }
            0x00B0 => {
                if let Some(r) = rfx(&mut c) {
                    wb.sheets[si].merges.push(r);
                }
            }
            0x0097 => {
                let p = (|| {
                    let x = c.f64()?;
                    let y = c.f64()?;
                    c.skip(12)?;
                    Some((x, y, c.u8()? & 1 != 0))
                })();
                if let Some((x, y, true)) = p {
                    wb.sheets[si].frozen_cols = x as u32;
                    wb.sheets[si].frozen_rows = y as u32;
                }
            }
            0x01EE => {
                let l = (|| {
                    let area = rfx(&mut c)?;
                    let rel = wide(&mut c)?;
                    let place = wide(&mut c)?;
                    let tip = wide(&mut c)?;
                    let mut target = rels.get(&rel).map(|r| r.1.clone()).unwrap_or_default();
                    if !place.is_empty() {
                        target = if target.is_empty() { format!("#{place}") } else { format!("{target}#{place}") };
                    }
                    (!target.is_empty()).then_some((area, Link { target, tip }))
                })();
                if let Some(l) = l {
                    link_bytes += l.1.target.len() + l.1.tip.len();
                    if link_bytes <= crate::xlsx_notes::LINK_BUDGET {
                        links.push(l);
                    }
                }
            }
            // BrtDrawing: the relation of the sheet's drawing part.
            0x0226 => {
                if let Some(rid) = wide(&mut c) {
                    crate::xlsx_drawing::read_drawing_rel(z, part, &rid, &mut wb.sheets[si], chart_points);
                }
            }
            _ => tools.record(&mut wb.sheets[si], ty, data, dxfs, &fmla, rgb),
        }
    }
    tools.finish(&mut wb.sheets[si]);
    // Formulas. Shared formulas by first row, then first column.
    let mut by_row: HashMap<u32, std::collections::BTreeMap<u32, usize>> = HashMap::new();
    for (i, s) in shared.iter().enumerate() {
        by_row.entry(s.0.start.row).or_default().entry(s.0.start.col).or_insert(i);
    }
    let mut spilled = Vec::new();
    // Each cell of a shared formula gets a tree of its own: long ones are
    // copied within a budget, past which the cells keep their values.
    let mut copies_left = crate::biff::SHARED_COPIES;
    for (cell, style, value, tokens, exp) in formulas {
        let r = Refs { biff12: true, biff5: false, base: cell, in_name: false, sheet: &sheet, name: &name_of, extern_name: &ext };
        let (text, array) = match (&tokens, exp) {
            (Some((rgce, rgcb)), _) => (biff::decompile(rgce, rgcb, &r), false),
            (None, Some(row)) => match by_row.get(&row).and_then(|m| m.range(..=cell.col).next_back()).map(|(_, i)| &shared[*i]).filter(|s| s.0.contains(cell)) {
                Some((area, rgce, rgcb, true)) if area.start == cell => {
                    spilled.push(*area);
                    (biff::decompile(rgce, rgcb, &r), true)
                }
                Some((_, rgce, rgcb, false)) if biff::charge_copy(&mut copies_left, rgce, rgcb) => (biff::decompile(rgce, rgcb, &r), false),
                _ => (None, false),
            },
            _ => (None, false),
        };
        let expr = text.and_then(|t| parse_canonical(&t).ok()).map(|e| Arc::new(if array { e } else { e.legacy() }));
        let content = match expr {
            Some(expr) => Content::Formula { expr: expr.into(), value },
            None if value == Value::Empty => Content::Empty,
            None => Content::Value(value),
        };
        wb.load_cell(si, cell, content, style);
    }
    let mut work = 0;
    for area in spilled {
        if work > crate::xlsx::MAX_ARRAY_WORK {
            break;
        }
        work += wb.sheets[si].mark_spilled(&area);
    }
    let mut budget = crate::xlsx_notes::LINK_BUDGET;
    for (area, link) in links {
        for (c, l) in crate::xlsx_notes::spread_link(&area, link, &mut budget) {
            wb.sheets[si].edit_extra(c, |x| x.link = Some(l));
        }
    }
    // Notes.
    let comments = rels.values().find(|(t, _, _)| t.ends_with("/comments")).and_then(|(_, t, _)| z.read(&resolve_target(dir, t)).ok());
    if let Some(cd) = comments {
        let mut authors = Vec::new();
        let mut cur: Option<(CellRef, usize)> = None;
        for (ty, data) in records(&cd) {
            let mut c = Cur::new(data);
            match ty {
                0x0278 => authors.push(short(wide(&mut c).unwrap_or_default())),
                0x027B => {
                    if let (Some(a), Some(r)) = (c.u32(), rfx(&mut c)) {
                        cur = Some((r.start, a as usize));
                    }
                }
                0x027D => {
                    c.skip(1);
                    if let (Some((cell, a)), Some(text)) = (cur, wide(&mut c)) {
                        let author = authors.get(a).cloned().unwrap_or_default();
                        wb.sheets[si].edit_extra(cell, |x| x.note = Some(Note { text: crate::xlsx_notes::without_author(&text, &author), author, ..Default::default() }));
                    }
                }
                _ => {}
            }
        }
    }
}

fn rk(v: u32) -> f64 {
    let n = if v & 2 != 0 { ((v as i32) >> 2) as f64 } else { f64::from_bits(u64::from(v & 0xFFFF_FFFC) << 32) };
    if v & 1 != 0 { n / 100.0 } else { n }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_headers() {
        // A one-byte type, a two-byte type (BrtBundleSh) and a two-byte
        // length.
        let mut d = vec![0x07, 0x02, 0xAA, 0xBB, 0x9C, 0x01, 0x00, 0x05, 0x81, 0x01];
        d.extend(std::iter::repeat_n(9u8, 129));
        let recs = records(&d);
        assert_eq!(recs[0], (7, &[0xAA, 0xBB][..]));
        assert_eq!(recs[1], (156, &[][..]));
        assert_eq!((recs[2].0, recs[2].1.len()), (5, 129));
    }

    fn rec(out: &mut Vec<u8>, ty: u32, data: &[u8]) {
        if ty < 0x80 {
            out.push(ty as u8);
        } else {
            out.push((ty & 0x7F) as u8 | 0x80);
            out.push((ty >> 7) as u8);
        }
        let mut n = data.len();
        loop {
            let b = (n & 0x7F) as u8;
            n >>= 7;
            out.push(if n > 0 { b | 0x80 } else { b });
            if n == 0 {
                break;
            }
        }
        out.extend_from_slice(data);
    }

    fn ws(s: &str) -> Vec<u8> {
        let u: Vec<u16> = s.encode_utf16().collect();
        let mut v = (u.len() as u32).to_le_bytes().to_vec();
        for c in u {
            v.extend_from_slice(&c.to_le_bytes());
        }
        v
    }

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    #[test]
    fn reads_a_workbook() {
        let mut wbin = Vec::new();
        rec(&mut wbin, 0x0099, &0u32.to_le_bytes());
        rec(&mut wbin, 0x009C, &cat(&[&0u32.to_le_bytes(), &1u32.to_le_bytes(), &ws("rId1"), &ws("Ventas")]));
        rec(&mut wbin, 0x009C, &cat(&[&1u32.to_le_bytes(), &2u32.to_le_bytes(), &ws("rId2"), &ws("Oculta")]));
        rec(&mut wbin, 0x0162, &[]);
        rec(&mut wbin, 0x016A, &cat(&[&1u32.to_le_bytes(), &0u32.to_le_bytes(), &1i32.to_le_bytes(), &1i32.to_le_bytes()]));
        // Doble = Oculta!$A$1
        let name_f = cat(&[&[0x3A], &0u16.to_le_bytes(), &0u32.to_le_bytes(), &0u16.to_le_bytes()]);
        rec(&mut wbin, 0x0027, &cat(&[&0u32.to_le_bytes(), &[0], &0xFFFF_FFFFu32.to_le_bytes(), &ws("Base"), &(name_f.len() as u32).to_le_bytes(), &name_f, &0u32.to_le_bytes()]));
        let mut sst = Vec::new();
        rec(&mut sst, 0x0013, &cat(&[&[0], &ws("hola")]));
        let mut styles = Vec::new();
        rec(&mut styles, 0x0263, &[]);
        let font = |bold: u16| cat(&[&220u16.to_le_bytes(), &0u16.to_le_bytes(), &bold.to_le_bytes(), &[0; 6], &[1 << 1, 8, 0, 0, 0, 0, 0, 0], &[2], &ws("Calibri")]);
        rec(&mut styles, 0x002B, &font(400));
        rec(&mut styles, 0x002B, &font(700));
        rec(&mut styles, 0x0269, &[]);
        let xf = |font: u16, fmt: u16| cat(&[&0u16.to_le_bytes(), &fmt.to_le_bytes(), &font.to_le_bytes(), &0u16.to_le_bytes(), &0u16.to_le_bytes(), &[0, 0], &0x1002u32.to_le_bytes()]);
        rec(&mut styles, 0x002F, &xf(0, 0));
        rec(&mut styles, 0x002F, &xf(1, 4));
        rec(&mut styles, 0x026A, &[]);
        let mut s1 = Vec::new();
        let cell = |col: u32, style: u32| cat(&[&col.to_le_bytes(), &style.to_le_bytes()]);
        rec(&mut s1, 0x0000, &cat(&[&0u32.to_le_bytes(), &0u32.to_le_bytes(), &600u16.to_le_bytes(), &[0, 0x20, 0]]));
        rec(&mut s1, 0x0007, &cat(&[&cell(0, 1), &0u32.to_le_bytes()]));
        rec(&mut s1, 0x0005, &cat(&[&cell(1, 0), &2.5f64.to_le_bytes()]));
        rec(&mut s1, 0x0002, &cat(&[&cell(2, 0), &((3u32 << 2) | 2).to_le_bytes()]));
        // D1 = SUM(B1:C1)*Base, with a shared formula in E1:F1 (=B1*2).
        let f = cat(&[&[0x25], &0u32.to_le_bytes(), &0u32.to_le_bytes(), &0xC001u16.to_le_bytes(), &0xC002u16.to_le_bytes(), &[0x22, 1], &4u16.to_le_bytes(), &[0x23], &1u32.to_le_bytes(), &[0x05]]);
        rec(&mut s1, 0x0009, &cat(&[&cell(3, 0), &0f64.to_le_bytes(), &0u16.to_le_bytes(), &(f.len() as u32).to_le_bytes(), &f, &0u32.to_le_bytes()]));
        let exp = cat(&[&[0x01], &0u32.to_le_bytes()]);
        for col in [4u32, 5] {
            rec(&mut s1, 0x0009, &cat(&[&cell(col, 0), &0f64.to_le_bytes(), &0u16.to_le_bytes(), &(exp.len() as u32).to_le_bytes(), &exp, &0u32.to_le_bytes()]));
            if col == 4 {
                let sf = cat(&[&[0x2C], &0u32.to_le_bytes(), &0xFFFDu16.to_le_bytes(), &[0x1E], &2u16.to_le_bytes(), &[0x05]]);
                rec(&mut s1, 0x01AB, &cat(&[&0u32.to_le_bytes(), &0u32.to_le_bytes(), &4u32.to_le_bytes(), &5u32.to_le_bytes(), &(sf.len() as u32).to_le_bytes(), &sf, &0u32.to_le_bytes()]));
            }
        }
        rec(&mut s1, 0x00B0, &cat(&[&1u32.to_le_bytes(), &2u32.to_le_bytes(), &0u32.to_le_bytes(), &1u32.to_le_bytes()]));
        let mut s2 = Vec::new();
        rec(&mut s2, 0x0000, &cat(&[&0u32.to_le_bytes(), &0u32.to_le_bytes(), &300u16.to_le_bytes(), &[0, 0, 0]]));
        rec(&mut s2, 0x0005, &cat(&[&cell(0, 0), &10f64.to_le_bytes()]));
        let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="x/worksheet" Target="worksheets/sheet1.bin"/><Relationship Id="rId2" Type="x/worksheet" Target="worksheets/sheet2.bin"/></Relationships>"#;
        let mut z = crate::zip::ZipWriter::new();
        z.add("xl/workbook.bin", &wbin, true);
        z.add("xl/_rels/workbook.bin.rels", rels.as_bytes(), true);
        z.add("xl/sharedStrings.bin", &sst, true);
        z.add("xl/styles.bin", &styles, true);
        z.add("xl/worksheets/sheet1.bin", &s1, true);
        z.add("xl/worksheets/sheet2.bin", &s2, true);
        let data = z.finish();
        let wb = read(&data).unwrap();
        assert_eq!(wb.sheets.len(), 2);
        assert!(wb.sheets[1].hidden);
        let sh = &wb.sheets[0];
        let a1 = sh.get(CellRef::new(0, 0)).unwrap();
        assert_eq!(a1.value(), &Value::text("hola"));
        assert!(wb.styles.get(a1.style).bold);
        assert_eq!(&*wb.styles.get(a1.style).num_fmt, "#,##0.00");
        assert_eq!(sh.get(CellRef::new(0, 2)).unwrap().value(), &Value::Number(3.0));
        let d1 = sh.get(CellRef::new(0, 3)).unwrap();
        assert_eq!(d1.formula().unwrap().expr().to_canonical(), "SUM(B1:C1)*Base");
        assert_eq!(d1.value(), &Value::Number(55.0));
        assert_eq!(sh.get(CellRef::new(0, 4)).unwrap().formula().unwrap().expr().to_canonical(), "B1*2");
        assert_eq!(sh.get(CellRef::new(0, 5)).unwrap().formula().unwrap().expr().to_canonical(), "C1*2");
        assert_eq!(sh.merges, vec![Range::parse("A2:B3").unwrap()]);
        assert_eq!(sh.row_heights.get(&0), Some(&(30.0 * crate::xlsx::ROW_SCALE).round()));
        assert_eq!(wb.names.len(), 1);
    }

    #[test]
    fn reads_sheet_tools_and_drawings() {
        use apex_sheet::data::{CfKind, Cfvo, CmpOp, FilterKind, ValKind};
        use apex_sheet::functions::CritOp;
        let u32s = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let rgb = |c: u32| cat(&[&[5, 0, 0, 0], &[(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]]);
        let mut wbin = Vec::new();
        rec(&mut wbin, 0x009C, &cat(&[&0u32.to_le_bytes(), &1u32.to_le_bytes(), &ws("rId1"), &ws("Datos")]));
        // A differential format: pink fill, bold.
        let mut styles = Vec::new();
        rec(&mut styles, 0x01F9, &1u32.to_le_bytes());
        rec(&mut styles, 0x01FB, &cat(&[&[0, 0, 2, 0, 2, 0, 12, 0], &rgb(0xFFC7CE), &[0x18, 0, 6, 0], &700u16.to_le_bytes()]));
        rec(&mut styles, 0x01FA, &[]);
        let mut s = Vec::new();
        rec(&mut s, 0x0000, &cat(&[&0u32.to_le_bytes(), &0u32.to_le_bytes(), &300u16.to_le_bytes(), &[0, 0, 0]]));
        rec(&mut s, 0x0005, &cat(&[&0u32.to_le_bytes(), &0u32.to_le_bytes(), &4f64.to_le_bytes()]));
        rec(&mut s, 0x0226, &ws("rId1"));
        // Conditional formats on A1:A4: value > 10 (priority 2) and a
        // two-color scale (priority 1).
        rec(&mut s, 0x01CD, &u32s(&[2, 0, 1, 0, 3, 0, 0]));
        let f = cat(&[&3u32.to_le_bytes(), &[0x1E, 10, 0], &0u32.to_le_bytes()]);
        rec(&mut s, 0x01CF, &cat(&[&u32s(&[1, 0, 0, 2, 5, 0, 0]), &[0, 0], &u32s(&[f.len() as u32, 0, 0, 0xFFFF_FFFF]), &f]));
        rec(&mut s, 0x01D0, &[]);
        rec(&mut s, 0x01CF, &cat(&[&u32s(&[3, 2, 0xFFFF_FFFF, 1, 0, 0, 0]), &[0, 0], &u32s(&[0, 0, 0, 0xFFFF_FFFF])]));
        rec(&mut s, 0x01D5, &[]);
        for ty in [2u32, 3] {
            rec(&mut s, 0x01D7, &cat(&[&ty.to_le_bytes(), &0f64.to_le_bytes(), &u32s(&[0, 0, 0])]));
        }
        rec(&mut s, 0x01DB, &rgb(0xF8696B));
        rec(&mut s, 0x01DB, &rgb(0x63BE7B));
        rec(&mut s, 0x01D6, &[]);
        rec(&mut s, 0x01D0, &[]);
        rec(&mut s, 0x01CE, &[]);
        // Whole numbers > 0 on B2:B3.
        let zero = cat(&[&3u32.to_le_bytes(), &[0x1E, 0, 0], &0u32.to_le_bytes()]);
        rec(&mut s, 0x0040, &cat(&[&(1u32 | 4 << 20 | 1 << 19).to_le_bytes(), &u32s(&[1, 1, 2, 1, 1]), &ws("E"), &ws("Mal"), &u32s(&[0xFFFF_FFFF, 0xFFFF_FFFF]), &zero, &u32s(&[0, 0])]));
        // Autofilter on A1:B4: A is x or y, B > 3.
        rec(&mut s, 0x00A1, &u32s(&[0, 3, 0, 1]));
        rec(&mut s, 0x00A3, &cat(&[&0u32.to_le_bytes(), &[0, 0]]));
        rec(&mut s, 0x00A5, &u32s(&[0, 0]));
        rec(&mut s, 0x00A7, &ws("x"));
        rec(&mut s, 0x00A7, &ws("y"));
        rec(&mut s, 0x00A6, &[]);
        rec(&mut s, 0x00A4, &[]);
        rec(&mut s, 0x00A3, &cat(&[&1u32.to_le_bytes(), &[0, 0]]));
        rec(&mut s, 0x00AC, &1u32.to_le_bytes());
        rec(&mut s, 0x00AE, &cat(&[&[4, 4], &3f64.to_le_bytes()]));
        rec(&mut s, 0x00AF, &[]);
        rec(&mut s, 0x00A4, &[]);
        rec(&mut s, 0x00A2, &[]);
        let rels = |ty: &str, target: &str| format!(r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{ty}" Target="{target}"/></Relationships>"#);
        let drawing = r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><xdr:twoCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>5</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:pic><xdr:nvPicPr><xdr:cNvPr id="2" name="Imagen 1"/><xdr:cNvPicPr/></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill><xdr:spPr/></xdr:pic><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#;
        let png = cat(&[b"\x89PNG\r\n\x1a\n", &[0; 24]]);
        let mut z = crate::zip::ZipWriter::new();
        z.add("xl/workbook.bin", &wbin, true);
        z.add("xl/_rels/workbook.bin.rels", rels("worksheet", "worksheets/sheet1.bin").as_bytes(), true);
        z.add("xl/styles.bin", &styles, true);
        z.add("xl/worksheets/sheet1.bin", &s, true);
        z.add("xl/worksheets/_rels/sheet1.bin.rels", rels("drawing", "../drawings/drawing1.xml").as_bytes(), true);
        z.add("xl/drawings/drawing1.xml", drawing.as_bytes(), true);
        z.add("xl/drawings/_rels/drawing1.xml.rels", rels("image", "../media/image1.png").as_bytes(), true);
        z.add("xl/media/image1.png", &png, false);
        let wb = read(&z.finish()).unwrap();
        let sh = &wb.sheets[0];
        assert_eq!(sh.cond_formats.len(), 2);
        assert_eq!(sh.cond_formats[0].kind, CfKind::ColorScale(vec![(Cfvo::Min, 0xF8696B), (Cfvo::Max, 0x63BE7B)]));
        let r = &sh.cond_formats[1];
        assert_eq!(r.ranges, vec![Range::parse("A1:A4").unwrap()]);
        assert_eq!(r.kind, CfKind::CellIs { op: CmpOp::Greater, f1: "10".into(), f2: String::new() });
        assert_eq!((r.style.fill, r.style.bold), (Some(0xFFC7CE), Some(true)));
        let v = &sh.validations[0];
        assert_eq!(v.ranges, vec![Range::parse("B2:B3").unwrap()]);
        assert_eq!((v.kind, v.op, v.f1.as_str(), v.error_msg.as_str()), (ValKind::Whole, CmpOp::Greater, "0", "Mal"));
        let f = sh.filter.as_ref().unwrap();
        assert_eq!(f.range, Range::parse("A1:B4").unwrap());
        assert_eq!(f.cols.len(), 2);
        assert_eq!(f.cols[0].kind, FilterKind::Values { items: vec!["x".into(), "y".into()], blanks: false });
        assert_eq!(f.cols[1].kind, FilterKind::Custom { and: true, a: (CritOp::Gt, "3".into()), b: None });
        assert_eq!(sh.images.len(), 1);
        assert_eq!(sh.images[0].from, CellRef::new(1, 1));
    }
}
