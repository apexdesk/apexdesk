//! Page setup, headers and footers and print settings of xlsx worksheets.

use std::fmt::Write as _;

use apex_sheet::addr::{col_name, parse_col};
use apex_sheet::{CellRef, HfText, ImageFormat, MAX_COLS, MAX_ROWS, Range, Sheet, SheetBackground, SheetPage, ViewMode, Workbook};

use crate::xml::{Ev, Reader, Tag, esc};

const PT_PER_IN: f32 = 72.0;

/// Excel paper size codes and their portrait size in points.
const PAPER_CODES: [(u32, f32, f32); 8] = [
    (9, 595.3, 841.9),
    (1, 612.0, 792.0),
    (5, 612.0, 1008.0),
    (8, 841.9, 1190.6),
    (11, 419.5, 595.3),
    (13, 498.9, 708.7),
    (7, 522.0, 756.0),
    (3, 792.0, 1224.0),
];

/// The paper code of a portrait size, if it is a known one.
fn paper_code(w: f32, h: f32) -> Option<u32> {
    PAPER_CODES.iter().find(|p| (p.1 - w).abs() < 2.0 && (p.2 - h).abs() < 2.0).map(|p| p.0)
}

fn flag(t: &Tag, key: &str) -> bool {
    t.attr(key).is_some_and(|v| v == "1" || v == "true")
}

fn num(t: &Tag, key: &str) -> Option<f32> {
    t.attr(key).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite())
}

/// Reads an Excel header or footer string ("&LLeft&CPage &P") into its
/// three parts, dropping font and format codes.
pub(crate) fn parse_hf(s: &str) -> HfText {
    let mut parts = [String::new(), String::new(), String::new()];
    let mut sec = 1;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '&' {
            if c != '\r' {
                parts[sec].push(c);
            }
            continue;
        }
        let Some(code) = it.next() else { break };
        match code {
            'L' | 'l' => sec = 0,
            'C' | 'c' => sec = 1,
            'R' | 'r' => sec = 2,
            'P' | 'N' | 'D' | 'T' | 'F' | 'A' | 'p' | 'n' | 'd' | 't' | 'f' | 'a' => {
                parts[sec].push('&');
                parts[sec].push(code.to_ascii_uppercase());
            }
            '&' => parts[sec].push_str("&&"),
            // Font: &"Name,Style".
            '"' => {
                for c in it.by_ref() {
                    if c == '"' {
                        break;
                    }
                }
            }
            // Font size: &12.
            '0'..='9' => {
                while it.peek().is_some_and(|c| c.is_ascii_digit()) {
                    it.next();
                }
            }
            // Color: &KRRGGBB or a theme color like &K01+000.
            'K' | 'k' => {
                for _ in 0..6 {
                    if it.peek().is_some_and(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '-') {
                        it.next();
                    }
                }
            }
            // Styles, path (&Z) and picture (&G) have no counterpart.
            _ => {}
        }
    }
    let [left, center, right] = parts;
    HfText { left, center, right }
}

/// Writes the three parts back as an Excel header or footer string.
fn hf_string(t: &HfText) -> String {
    let mut s = String::new();
    for (code, part) in [("&L", &t.left), ("&C", &t.center), ("&R", &t.right)] {
        if !part.is_empty() {
            s.push_str(code);
            s.push_str(part);
        }
    }
    s
}

/// Reads the page settings of a worksheet. Only the parts before and after
/// the cell data are scanned.
pub fn read_sheet_page(xml: &str, p: &mut SheetPage) {
    let head_end = xml.find("sheetData").and_then(|i| xml[..i].rfind('<')).unwrap_or(xml.len());
    let tail = xml.rfind("sheetData").filter(|&i| i >= head_end).map_or("", |i| &xml[i..]);
    let mut fit_to_page = false;
    let mut fit = (1u16, 1u16);
    for part in [&xml[..head_end], tail] {
        let mut r = Reader::new(part);
        // Which breaks list is open: rows (true) or columns.
        let mut in_rows = true;
        loop {
            let ev = r.next();
            let start = matches!(ev, Ev::Start(_));
            let t = match ev {
                Ev::Eof => break,
                Ev::Start(t) | Ev::Empty(t) => t,
                _ => continue,
            };
            match t.local() {
                "pageSetUpPr" => fit_to_page = flag(&t, "fitToPage"),
                "printOptions" => {
                    p.center_h = flag(&t, "horizontalCentered");
                    p.center_v = flag(&t, "verticalCentered");
                    p.gridlines = flag(&t, "gridLines");
                    p.headings = flag(&t, "headings");
                }
                "pageMargins" => {
                    let m = |k: &str, d: f32| num(&t, k).map_or(d, |v| (v * PT_PER_IN).clamp(0.0, 1000.0));
                    p.margin_left = m("left", p.margin_left);
                    p.margin_right = m("right", p.margin_right);
                    p.margin_top = m("top", p.margin_top);
                    p.margin_bottom = m("bottom", p.margin_bottom);
                    p.margin_header = m("header", p.margin_header);
                    p.margin_footer = m("footer", p.margin_footer);
                }
                "pageSetup" => {
                    // A missing paperSize means Letter; unknown codes keep A4.
                    let code = t.attr("paperSize").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1);
                    (p.paper_width, p.paper_height) = PAPER_CODES.iter().find(|c| c.0 == code).map_or((595.3, 841.9), |c| (c.1, c.2));
                    let len = |k: &str| t.attr(k).and_then(|v| crate::parse_length_pt(&v)).filter(|v| *v > 36.0 && *v < 10000.0);
                    if let (Some(w), Some(h)) = (len("paperWidth"), len("paperHeight")) {
                        (p.paper_width, p.paper_height) = (w.min(h), w.max(h));
                    }
                    p.landscape = t.attr("orientation").as_deref() == Some("landscape");
                    if let Some(s) = num(&t, "scale") {
                        p.scale = s.clamp(10.0, 400.0) as u16;
                    }
                    let pages = |k: &str| num(&t, k).map_or(1, |v| v.clamp(0.0, 32767.0) as u16);
                    fit = (pages("fitToWidth"), pages("fitToHeight"));
                    p.across_first = t.attr("pageOrder").as_deref() == Some("overThenDown");
                    p.first_page = if flag(&t, "useFirstPageNumber") { num(&t, "firstPageNumber").map(|v| v.clamp(0.0, 1e6) as u32) } else { None };
                }
                "headerFooter" => {
                    p.hf.different_first = flag(&t, "differentFirst");
                    p.hf.odd_even = flag(&t, "differentOddEven");
                }
                name @ ("oddHeader" | "oddFooter" | "evenHeader" | "evenFooter" | "firstHeader" | "firstFooter") => {
                    let text = if start { parse_hf(&r.text()) } else { HfText::default() };
                    let hf = &mut p.hf;
                    *match name {
                        "oddHeader" => &mut hf.header,
                        "oddFooter" => &mut hf.footer,
                        "evenHeader" => &mut hf.even_header,
                        "evenFooter" => &mut hf.even_footer,
                        "firstHeader" => &mut hf.first_header,
                        _ => &mut hf.first_footer,
                    } = text;
                }
                "rowBreaks" => in_rows = true,
                "colBreaks" => in_rows = false,
                "brk" => {
                    // A page starts at `id` (the break follows row `id - 1`).
                    let id = num(&t, "id").map(|v| v as u32).filter(|&v| v > 0);
                    if let (Some(id), true) = (id, flag(&t, "man")) {
                        if in_rows && id < MAX_ROWS {
                            p.row_breaks.insert(id);
                        } else if !in_rows && id < MAX_COLS {
                            p.col_breaks.insert(id);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    p.fit = fit_to_page.then_some(fit);
}

/// `<pageSetUpPr>` for `<sheetPr>`, when the sheet fits to pages.
pub fn sheet_pr(sh: &Sheet) -> Option<&'static str> {
    sh.page.fit.is_some().then_some(r#"<pageSetUpPr fitToPage="1"/>"#)
}

/// printOptions, pageMargins, pageSetup, headerFooter, rowBreaks and
/// colBreaks, in schema order.
pub fn write_sheet_page(s: &mut String, sh: &Sheet) {
    let p = &sh.page;
    if p.center_h || p.center_v || p.gridlines || p.headings {
        s.push_str("<printOptions");
        for (on, k) in [(p.center_h, "horizontalCentered"), (p.center_v, "verticalCentered"), (p.headings, "headings"), (p.gridlines, "gridLines")] {
            if on {
                let _ = write!(s, r#" {k}="1""#);
            }
        }
        s.push_str("/>");
    }
    let inch = |v: f32| (v / PT_PER_IN * 1000.0).round() / 1000.0;
    let _ = write!(
        s,
        r#"<pageMargins left="{}" right="{}" top="{}" bottom="{}" header="{}" footer="{}"/>"#,
        inch(p.margin_left),
        inch(p.margin_right),
        inch(p.margin_top),
        inch(p.margin_bottom),
        inch(p.margin_header),
        inch(p.margin_footer)
    );
    match paper_code(p.paper_width, p.paper_height) {
        Some(code) => {
            let _ = write!(s, r#"<pageSetup paperSize="{code}""#);
        }
        None => {
            let mm = |v: f32| (v / PT_PER_IN * 25.4 * 10.0).round() / 10.0;
            let _ = write!(s, r#"<pageSetup paperHeight="{}mm" paperWidth="{}mm""#, mm(p.paper_height), mm(p.paper_width));
        }
    }
    if p.scale != 100 {
        let _ = write!(s, r#" scale="{}""#, p.scale.clamp(10, 400));
    }
    if let Some(n) = p.first_page {
        let _ = write!(s, r#" firstPageNumber="{n}""#);
    }
    if let Some((w, h)) = p.fit {
        let _ = write!(s, r#" fitToWidth="{w}" fitToHeight="{h}""#);
    }
    if p.across_first {
        s.push_str(r#" pageOrder="overThenDown""#);
    }
    let _ = write!(s, r#" orientation="{}""#, if p.landscape { "landscape" } else { "portrait" });
    if p.first_page.is_some() {
        s.push_str(r#" useFirstPageNumber="1""#);
    }
    s.push_str("/>");
    let hf = &p.hf;
    if !hf.is_empty() || hf.different_first || hf.odd_even {
        s.push_str("<headerFooter");
        if hf.odd_even {
            s.push_str(r#" differentOddEven="1""#);
        }
        if hf.different_first {
            s.push_str(r#" differentFirst="1""#);
        }
        s.push('>');
        let mut part = |name: &str, t: &HfText, used: bool| {
            if used && !t.is_empty() {
                let _ = write!(s, "<{name}>{}</{name}>", esc(&hf_string(t)));
            }
        };
        part("oddHeader", &hf.header, true);
        part("oddFooter", &hf.footer, true);
        part("evenHeader", &hf.even_header, hf.odd_even);
        part("evenFooter", &hf.even_footer, hf.odd_even);
        part("firstHeader", &hf.first_header, hf.different_first);
        part("firstFooter", &hf.first_footer, hf.different_first);
        s.push_str("</headerFooter>");
    }
    for (tag, breaks, max) in [("rowBreaks", &p.row_breaks, MAX_COLS - 1), ("colBreaks", &p.col_breaks, MAX_ROWS - 1)] {
        // Excel allows 1026 manual breaks per direction.
        let breaks: Vec<u32> = breaks.iter().copied().filter(|&b| b > 0).take(1026).collect();
        if breaks.is_empty() {
            continue;
        }
        let _ = write!(s, r#"<{tag} count="{0}" manualBreakCount="{0}">"#, breaks.len());
        for b in breaks {
            let _ = write!(s, r#"<brk id="{b}" max="{max}" man="1"/>"#);
        }
        let _ = write!(s, "</{tag}>");
    }
    if !sh.cell_watches.is_empty() {
        s.push_str("<cellWatches>");
        for c in &sh.cell_watches {
            let _ = write!(s, r#"<cellWatch r="{}"/>"#, c.to_a1());
        }
        s.push_str("</cellWatches>");
    }
}

/// The Watch Window cells of a worksheet (`<cellWatches>`).
pub fn read_cell_watches(xml: &str) -> Vec<CellRef> {
    let Some(i) = xml.find("cellWatches>") else { return Vec::new() };
    let Some(start) = xml[..i].rfind("<") else { return Vec::new() };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut r = Reader::new(&xml[start..]);
    loop {
        match r.next() {
            Ev::Start(t) | Ev::Empty(t) if t.local() == "cellWatch" => {
                if let Some(c) = t.attr("r").and_then(|a| CellRef::parse(&a)) {
                    if seen.insert(c) {
                        out.push(c);
                    }
                }
            }
            Ev::End(t) if crate::xml::local(t) == "cellWatches" => break,
            Ev::Eof => break,
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Print area and print titles (defined names)

/// Splits a list of references at the commas outside quotes.
fn split_refs(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            ',' if !quoted => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Sheet name of a reference like `'My sheet'!$A$1`, if it has one.
fn ref_sheet(r: &str) -> Option<String> {
    let (sheet, _) = r.trim().rsplit_once('!')?;
    Some(sheet.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')).map_or(sheet.to_string(), |x| x.replace("''", "'")))
}

/// One side of an area: a row number, a column name or a cell.
enum Side {
    Row(u32),
    Col(u32),
    Cell(CellRef),
}

fn side(s: &str) -> Option<Side> {
    let s = s.replace('$', "");
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        let n: u32 = s.parse().ok()?;
        return (1..=MAX_ROWS).contains(&n).then(|| Side::Row(n - 1));
    }
    if s.bytes().all(|b| b.is_ascii_alphabetic()) {
        return parse_col(&s).map(Side::Col);
    }
    CellRef::parse(&s).map(Side::Cell)
}

/// Parses the area part of a reference (after the `!`).
fn area(r: &str) -> Option<(Side, Side)> {
    let a = r.trim().rsplit_once('!').map_or(r.trim(), |x| x.1);
    let (a, b) = a.split_once(':').unwrap_or((a, a));
    Some((side(a)?, side(b)?))
}

fn to_range(a: Side, b: Side) -> Option<Range> {
    let (s, e) = match (a, b) {
        (Side::Cell(a), Side::Cell(b)) => (a, b),
        (Side::Row(a), Side::Row(b)) => (CellRef::new(a, 0), CellRef::new(b, MAX_COLS - 1)),
        (Side::Col(a), Side::Col(b)) => (CellRef::new(0, a), CellRef::new(MAX_ROWS - 1, b)),
        _ => return None,
    };
    Some(Range::new(s, e))
}

/// Applies a `_xlnm.Print_Area` or `_xlnm.Print_Titles` defined name.
pub fn apply_defined_name(wb: &mut Workbook, name: &str, local: Option<usize>, text: &str) {
    let refs = split_refs(text);
    let Some(si) = local.filter(|&i| i < wb.sheets.len()).or_else(|| refs.first().and_then(|r| ref_sheet(r)).and_then(|n| wb.sheet_index(&n))) else {
        return;
    };
    let page = &mut wb.sheets[si].page;
    if name.eq_ignore_ascii_case("_xlnm.Print_Area") {
        // Several areas print on separate pages.
        page.print_areas = refs.iter().filter_map(|r| area(r)).filter_map(|(a, b)| to_range(a, b)).collect();
    } else if name.eq_ignore_ascii_case("_xlnm.Print_Titles") {
        for r in refs {
            match area(r) {
                Some((Side::Row(a), Side::Row(b))) => page.title_rows = Some((a.min(b), a.max(b))),
                Some((Side::Col(a), Side::Col(b))) => page.title_cols = Some((a.min(b), a.max(b))),
                // A cell area spanning every column or every row.
                Some((Side::Cell(a), Side::Cell(b))) if a.col == 0 && b.col == MAX_COLS - 1 => page.title_rows = Some((a.row.min(b.row), a.row.max(b.row))),
                Some((Side::Cell(a), Side::Cell(b))) if a.row == 0 && b.row == MAX_ROWS - 1 => page.title_cols = Some((a.col.min(b.col), a.col.max(b.col))),
                _ => {}
            }
        }
    }
}

/// `_xlnm.Print_Area` and `_xlnm.Print_Titles` names for `workbook.xml`.
pub fn print_names(wb: &Workbook) -> String {
    let mut s = String::new();
    for (i, sh) in wb.sheets.iter().enumerate() {
        let p = &sh.page;
        let q = format!("'{}'", sh.name.replace('\'', "''"));
        if !p.print_areas.is_empty() {
            let r: Vec<String> = p.print_areas.iter().map(|a| format!("{q}!${}${}:${}${}", col_name(a.start.col), a.start.row + 1, col_name(a.end.col), a.end.row + 1)).collect();
            let _ = write!(s, r#"<definedName name="_xlnm.Print_Area" localSheetId="{i}">{}</definedName>"#, esc(&r.join(",")));
        }
        let mut titles = Vec::new();
        if let Some((a, b)) = p.title_cols {
            titles.push(format!("{q}!${}:${}", col_name(a), col_name(b)));
        }
        if let Some((a, b)) = p.title_rows {
            titles.push(format!("{q}!${}:${}", a + 1, b + 1));
        }
        if !titles.is_empty() {
            let _ = write!(s, r#"<definedName name="_xlnm.Print_Titles" localSheetId="{i}">{}</definedName>"#, esc(&titles.join(",")));
        }
    }
    s
}

/// `sheetView/@view`.
pub fn view_mode(v: Option<&str>) -> ViewMode {
    match v {
        Some("pageBreakPreview") => ViewMode::PageBreakPreview,
        Some("pageLayout") => ViewMode::PageLayout,
        _ => ViewMode::Normal,
    }
}

pub fn view_attr(v: ViewMode) -> Option<&'static str> {
    match v {
        ViewMode::Normal => None,
        ViewMode::PageBreakPreview => Some("pageBreakPreview"),
        ViewMode::PageLayout => Some("pageLayout"),
    }
}

/// Relationship id of the sheet background (`<picture r:id>`).
pub const BACKGROUND_RID: &str = "rIdBg1";

/// The sheet background: `<picture r:id>` pointing to an image part.
pub fn read_background(z: &crate::zip::ZipReader, path: &str, xml: &str) -> Option<SheetBackground> {
    let tail = &xml[xml.find("</sheetData>")?..];
    let i = tail.find("<picture ").or_else(|| tail.find(":picture "))?;
    let mut from = i.saturating_sub(10);
    while !tail.is_char_boundary(from) {
        from -= 1;
    }
    let mut r = Reader::new(&tail[from..]);
    let rid = loop {
        match r.next() {
            Ev::Eof => return None,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "picture" => break t.attr("r:id").or_else(|| t.attr("id"))?.into_owned(),
            _ => {}
        }
    };
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    let rels = crate::xlsx_notes::read_rels(&z.read_string(&format!("{dir}/_rels/{file}.rels")).ok()?);
    let (_, target, false) = rels.get(&rid)? else { return None };
    let mpath = crate::xlsx::resolve_target(dir, target);
    let data = z.read(&mpath).ok()?;
    let format = ImageFormat::detect(&data, mpath.rsplit_once('.').map_or("", |x| x.1))?;
    Some(SheetBackground { data: data.into(), format })
}

/// Adds the background's image part and its relationship to the sheet's
/// relationships.
pub fn background_rel(sh: &Sheet, rels: &mut Option<String>, media: &mut crate::xlsx_drawing::Media) {
    let Some(bg) = &sh.page.background else { return };
    let name = media.add_data(&bg.data, bg.format);
    let rel = format!(r#"<Relationship Id="{BACKGROUND_RID}" Type="{}" Target="../media/{name}"/>"#, crate::xlsx_drawing::REL_IMAGE);
    match rels {
        Some(r) => {
            let at = r.rfind("</Relationships>").unwrap_or(r.len());
            r.insert_str(at, &rel);
        }
        None => *rels = Some(format!(r#"{}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rel}</Relationships>"#, crate::xml::HEADER)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use apex_sheet::HeaderFooter;

    /// A page setup using every setting, shared by the xlsx and ods tests.
    pub fn sample_page() -> SheetPage {
        let mut p = SheetPage {
            paper_width: 612.0,
            paper_height: 1008.0,
            landscape: true,
            margin_top: 72.0,
            margin_bottom: 64.8,
            margin_left: 36.0,
            margin_right: 43.2,
            margin_header: 28.8,
            margin_footer: 21.6,
            scale: 100,
            fit: Some((1, 0)),
            print_areas: vec![Range::parse("B2:F40").unwrap(), Range::parse("H50:J60").unwrap()],
            title_rows: Some((0, 1)),
            title_cols: Some((0, 0)),
            center_h: true,
            center_v: false,
            gridlines: true,
            headings: true,
            across_first: true,
            first_page: Some(3),
            hf: HeaderFooter {
                header: HfText { left: "&F".into(), center: "Report && more".into(), right: "&D &T".into() },
                footer: HfText::center("Page &P of &N"),
                different_first: true,
                first_header: HfText::default(),
                first_footer: HfText { right: "&A".into(), ..Default::default() },
                odd_even: true,
                even_header: HfText { left: "Even".into(), ..Default::default() },
                even_footer: HfText { left: "&P".into(), ..Default::default() },
            },
            ..SheetPage::default()
        };
        p.row_breaks.insert(20);
        p.col_breaks.insert(3);
        p
    }

    /// Checks that `back` kept every setting of `orig`, with margins within
    /// `tol` points.
    pub fn check_page(orig: &SheetPage, back: &SheetPage, tol: f32) {
        let near = |a: f32, b: f32, what: &str| assert!((a - b).abs() <= tol, "{what}: {a} vs {b}");
        near(orig.paper_width, back.paper_width, "paper width");
        near(orig.paper_height, back.paper_height, "paper height");
        near(orig.margin_top, back.margin_top, "top");
        near(orig.margin_bottom, back.margin_bottom, "bottom");
        near(orig.margin_left, back.margin_left, "left");
        near(orig.margin_right, back.margin_right, "right");
        near(orig.margin_header, back.margin_header, "header");
        near(orig.margin_footer, back.margin_footer, "footer");
        assert_eq!(
            (orig.landscape, orig.scale, orig.fit, &orig.print_areas, orig.title_rows, orig.title_cols),
            (back.landscape, back.scale, back.fit, &back.print_areas, back.title_rows, back.title_cols)
        );
        assert_eq!(
            (orig.center_h, orig.center_v, orig.gridlines, orig.headings, orig.across_first, orig.first_page),
            (back.center_h, back.center_v, back.gridlines, back.headings, back.across_first, back.first_page)
        );
        assert_eq!((&orig.row_breaks, &orig.col_breaks), (&back.row_breaks, &back.col_breaks));
        assert_eq!(orig.hf, back.hf);
    }

    #[test]
    fn hf_codes() {
        let t = parse_hf(r#"&L&"Arial,Bold"&12Title &K00FF00x&C&BPage &P of &N&R&Z&F &G&& co"#);
        assert_eq!(t, HfText { left: "Title x".into(), center: "Page &P of &N".into(), right: "&F && co".into() });
        assert_eq!(parse_hf("Plain &P"), HfText::center("Plain &P"));
        assert_eq!(parse_hf(&hf_string(&t)), t);
        assert_eq!(parse_hf("&K01+000Theme"), HfText::center("Theme"));
    }

    #[test]
    fn names() {
        let mut wb = Workbook::new();
        wb.sheets[0].name = "It's 1".into();
        wb.sheets[0].page = sample_page();
        let xml = print_names(&wb);
        assert!(xml.contains("&apos;It&apos;&apos;s 1&apos;!$B$2:$F$40") || xml.contains("'It''s 1'!$B$2:$F$40"), "{xml}");
        let mut back = Workbook::new();
        back.sheets[0].name = "It's 1".into();
        apply_defined_name(&mut back, "_xlnm.Print_Area", None, "'It''s 1'!$B$2:$F$40,'It''s 1'!$H$1:$H$2");
        apply_defined_name(&mut back, "_xlnm.Print_Titles", Some(0), "'It''s 1'!$A:$A,'It''s 1'!$1:$2");
        let p = &back.sheets[0].page;
        assert_eq!((p.print_areas.clone(), p.title_rows, p.title_cols), (vec![Range::parse("B2:F40").unwrap(), Range::parse("H1:H2").unwrap()], Some((0, 1)), Some((0, 0))));
        apply_defined_name(&mut back, "_xlnm.Print_Titles", Some(0), "Sheet1!$A$3:$XFD$4");
        assert_eq!(back.sheets[0].page.title_rows, Some((2, 3)));
    }
}
