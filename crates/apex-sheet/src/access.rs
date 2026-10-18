//! Accessibility checker (Review > Check Accessibility): objects without
//! alternative text, tables without a header row, default sheet names,
//! hard-to-read text, merged cells and hidden rows or columns with data.

use crate::addr::{CellRef, Range};
use crate::value::Value;
use crate::workbook::Workbook;

/// What kind of problem an issue is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// A picture, chart or shape without alternative text.
    MissingAlt,
    /// A table without a header row.
    NoHeader,
    /// A sheet with content still named "Sheet1", "Hoja2"...
    DefaultSheetName,
    /// Text whose color hardly differs from its fill (WCAG, below 4.5:1).
    Contrast,
    MergedCells,
    /// Hidden rows or columns holding data.
    HiddenData,
}

impl Category {
    pub const ALL: [Category; 6] = [Category::MissingAlt, Category::NoHeader, Category::DefaultSheetName, Category::Contrast, Category::MergedCells, Category::HiddenData];

    /// Errors must be fixed; warnings may be fine.
    pub fn is_error(self) -> bool {
        !matches!(self, Category::HiddenData)
    }
}

/// The object or the cells an issue is about.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Image { sheet: usize, id: u32 },
    Chart { sheet: usize, id: u32 },
    Shape { sheet: usize, id: u32 },
    /// Index into `Sheet::tables`.
    Table { sheet: usize, index: usize },
    Sheet(usize),
    Cells { sheet: usize, range: Range },
    /// Hidden rows (or columns) `from..=to`.
    Hidden { sheet: usize, rows: bool, from: u32, to: u32 },
}

impl Target {
    pub fn sheet(&self) -> usize {
        match *self {
            Target::Image { sheet, .. } | Target::Chart { sheet, .. } | Target::Shape { sheet, .. } | Target::Table { sheet, .. } | Target::Cells { sheet, .. } | Target::Hidden { sheet, .. } => sheet,
            Target::Sheet(s) => s,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub category: Category,
    pub target: Target,
    /// What to show: the object's name, the sheet's name or the cells.
    pub label: String,
    /// Contrast issues: the ratio found.
    pub ratio: Option<f64>,
}

/// WCAG's minimum contrast for normal text.
pub const MIN_CONTRAST: f64 = 4.5;
/// At most this many issues of one category are listed.
pub const MAX_PER_CATEGORY: usize = 500;

/// Relative luminance of an sRGB color (0xRRGGBB), WCAG 2.
pub fn luminance(rgb: u32) -> f64 {
    let lin = |c: u32| {
        let s = c as f64 / 255.0;
        if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin((rgb >> 16) & 0xFF) + 0.7152 * lin((rgb >> 8) & 0xFF) + 0.0722 * lin(rgb & 0xFF)
}

/// Contrast ratio of two colors, from 1 (same) to 21 (black on white).
pub fn contrast_ratio(a: u32, b: u32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Black or white, whichever reads better on `fill`.
pub fn readable_on(fill: u32) -> u32 {
    if contrast_ratio(0x000000, fill) >= contrast_ratio(0xFFFFFF, fill) { 0x000000 } else { 0xFFFFFF }
}

/// Names spreadsheets give new sheets, in the languages we meet most.
const DEFAULT_NAMES: [&str; 8] = ["sheet", "hoja", "feuil", "tabelle", "foglio", "planilha", "folha", "blad"];

/// Whether a sheet name is one given by default ("Sheet1", "Hoja 2").
pub fn is_default_sheet_name(name: &str) -> bool {
    let low = name.trim().to_lowercase();
    let digits = low.trim_end_matches(|c: char| c.is_ascii_digit());
    digits.len() < low.len() && DEFAULT_NAMES.contains(&digits.trim_end())
}

impl Workbook {
    /// Every accessibility issue of the workbook, sheet by sheet (hidden
    /// sheets are skipped, as nobody reads them).
    pub fn check_accessibility(&self) -> Vec<Issue> {
        let mut out = Vec::new();
        let mut counts = [0usize; Category::ALL.len()];
        let mut push = |out: &mut Vec<Issue>, i: Issue| {
            let k = Category::ALL.iter().position(|c| *c == i.category).unwrap_or(0);
            if counts[k] < MAX_PER_CATEGORY {
                counts[k] += 1;
                out.push(i);
            }
        };
        for (si, sh) in self.sheets.iter().enumerate() {
            if sh.hidden {
                continue;
            }
            let issue = |category, target, label: String| Issue { category, target, label, ratio: None };
            for img in &sh.images {
                if img.alt.trim().is_empty() {
                    push(&mut out, issue(Category::MissingAlt, Target::Image { sheet: si, id: img.id }, format!("{} ({})", img.name, sh.name)));
                }
            }
            for c in &sh.charts {
                if c.alt.trim().is_empty() {
                    push(&mut out, issue(Category::MissingAlt, Target::Chart { sheet: si, id: c.id }, format!("{} ({})", c.name, sh.name)));
                }
            }
            for s in &sh.shapes {
                // Lines and text boxes with text speak for themselves.
                if s.shape.alt.trim().is_empty() && s.shape.plain_text().trim().is_empty() {
                    push(&mut out, issue(Category::MissingAlt, Target::Shape { sheet: si, id: s.id }, format!("{} ({})", s.shape.name, sh.name)));
                }
            }
            for (i, t) in sh.tables.iter().enumerate() {
                if !t.header {
                    push(&mut out, issue(Category::NoHeader, Target::Table { sheet: si, index: i }, format!("{} ({})", t.name, sh.name)));
                }
            }
            let has_content = sh.cell_count() > 0 || !sh.images.is_empty() || !sh.charts.is_empty() || !sh.shapes.is_empty();
            if has_content && is_default_sheet_name(&sh.name) {
                push(&mut out, issue(Category::DefaultSheetName, Target::Sheet(si), sh.name.clone()));
            }
            for (range, ratio) in self.low_contrast(si) {
                push(&mut out, Issue { category: Category::Contrast, target: Target::Cells { sheet: si, range }, label: format!("{}!{}", sh.name, range.to_a1()), ratio: Some(ratio) });
            }
            for m in &sh.merges {
                push(&mut out, issue(Category::MergedCells, Target::Cells { sheet: si, range: *m }, format!("{}!{}", sh.name, m.to_a1())));
            }
            for rows in [true, false] {
                for (from, to) in self.hidden_with_data(si, rows) {
                    let label = if rows { format!("{}!{}:{}", sh.name, from + 1, to + 1) } else { format!("{}!{}:{}", sh.name, crate::addr::col_name(from), crate::addr::col_name(to)) };
                    push(&mut out, issue(Category::HiddenData, Target::Hidden { sheet: si, rows, from, to }, label));
                }
            }
        }
        out
    }

    /// Cells with text that is hard to read on their fill, joined into runs
    /// along each row, with the ratio of each run.
    pub fn low_contrast(&self, sheet: usize) -> Vec<(Range, f64)> {
        let sh = &self.sheets[sheet];
        let mut cells: Vec<(CellRef, u32, f64)> = Vec::new();
        for (c, cell) in sh.iter() {
            if cell.style == 0 || matches!(cell.value(), Value::Empty) || matches!(cell.value(), Value::Text(t) if t.trim().is_empty()) {
                continue;
            }
            let st = self.styles.get(cell.style);
            let (fg, bg) = (st.color.unwrap_or(0x000000), st.fill.unwrap_or(0xFFFFFF));
            let ratio = contrast_ratio(fg, bg);
            if ratio < MIN_CONTRAST {
                cells.push((c, cell.style, ratio));
            }
        }
        cells.sort_by_key(|(c, ..)| (c.row, c.col));
        let mut out: Vec<(Range, f64, u32)> = Vec::new();
        for (c, style, ratio) in cells {
            match out.last_mut() {
                Some((r, _, s)) if *s == style && r.end.row == c.row && r.end.col + 1 == c.col => r.end.col = c.col,
                _ => out.push((Range::single(c), ratio, style)),
            }
        }
        out.into_iter().map(|(r, ratio, _)| (r, ratio)).collect()
    }

    /// Runs of hidden rows (or columns) with at least one filled cell.
    pub fn hidden_with_data(&self, sheet: usize, rows: bool) -> Vec<(u32, u32)> {
        let sh = &self.sheets[sheet];
        let sizes = if rows { &sh.row_heights } else { &sh.col_widths };
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for (&i, &v) in sizes.iter() {
            if v != 0.0 {
                continue;
            }
            match runs.last_mut() {
                Some((_, to)) if *to + 1 == i => *to = i,
                _ => runs.push((i, i)),
            }
        }
        let Some(ext) = sh.extent() else { return Vec::new() };
        runs.retain(|&(from, to)| {
            let r = if rows { Range::new(CellRef::new(from, 0), CellRef::new(to, ext.col)) } else { Range::new(CellRef::new(0, from), CellRef::new(ext.row, to)) };
            let mut any = false;
            sh.for_each_in(&r, |_, cell| any |= !cell.value().is_empty());
            any
        });
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Chart, ChartType, ImageFormat, SheetImage, SheetShape};

    #[test]
    fn contrast_math() {
        assert!((contrast_ratio(0x000000, 0xFFFFFF) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(0x777777, 0x777777) - 1.0).abs() < 1e-9);
        // WCAG's example: #767676 on white is just about 4.54.
        let r = contrast_ratio(0x767676, 0xFFFFFF);
        assert!(r > 4.5 && r < 4.6, "{r}");
        assert_eq!(readable_on(0x003366), 0xFFFFFF);
        assert_eq!(readable_on(0xFFFF00), 0x000000);
        assert!(is_default_sheet_name("Sheet1") && is_default_sheet_name("Hoja 12") && is_default_sheet_name("Feuil3"));
        assert!(!is_default_sheet_name("Sheet") && !is_default_sheet_name("Sales 2024") && !is_default_sheet_name("Budget"));
    }

    #[test]
    fn finds_every_kind() {
        let mut wb = Workbook::new();
        let c = |a: &str| CellRef::parse(a).unwrap();
        let r = |a: &str| Range::parse(a).unwrap();
        // An empty default-named sheet is fine.
        assert!(wb.check_accessibility().is_empty());
        wb.set_input(0, c("A1"), "Name");
        wb.set_input(0, c("B1"), "Amount");
        wb.set_input(0, c("A2"), "x");
        wb.set_input(0, c("B2"), "1");
        let found = wb.check_accessibility();
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].category, &found[0].target), (Category::DefaultSheetName, &Target::Sheet(0)));
        wb.execute(crate::Op::RenameSheet { index: 0, name: "Data".into() });
        assert!(wb.check_accessibility().is_empty());

        // Light gray text on white, over two cells next to each other.
        wb.modify_style(0, &r("A2:B2"), |s| s.color = Some(0xCCCCCC));
        // Readable white on dark blue.
        wb.set_input(0, c("D1"), "ok");
        wb.modify_style(0, &r("D1"), |s| {
            s.color = Some(0xFFFFFF);
            s.fill = Some(0x003366);
        });
        let found = wb.check_accessibility();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].category, Category::Contrast);
        assert_eq!(found[0].target, Target::Cells { sheet: 0, range: r("A2:B2") });
        assert!(found[0].ratio.unwrap() < 2.0);
        wb.modify_style(0, &r("A2:B2"), |s| s.color = None);

        // Objects without alternative text.
        let img = SheetImage::new(std::sync::Arc::from(&b"\x89PNG\r\n\x1a\n"[..]), ImageFormat::Png, c("F2"), 40.0, 30.0);
        let id = wb.add_image(0, img);
        wb.add_chart(0, Chart::new(ChartType::Column, c("F10"), 300.0, 200.0));
        wb.add_shape(0, SheetShape::new(apex_doc::Shape::default(), c("F20"), 50.0, 50.0));
        let cats: Vec<Category> = wb.check_accessibility().iter().map(|i| i.category).collect();
        assert_eq!(cats, vec![Category::MissingAlt; 3]);
        let mut img = wb.sheets[0].image(id).unwrap().clone();
        img.alt = "Company logo".into();
        wb.set_image(0, img);
        assert_eq!(wb.check_accessibility().len(), 2);
        wb.sheets[0].charts[0].alt = "Sales by month".into();
        wb.sheets[0].shapes[0].shape.alt = "Arrow".into();
        assert!(wb.check_accessibility().is_empty());

        // Tables without headers, merged cells, hidden data.
        wb.set_input(0, c("H1"), "1");
        wb.set_input(0, c("H2"), "2");
        wb.create_table(0, r("H1:H2"), true, "TableStyleMedium2").unwrap();
        wb.sheets[0].tables[0].header = false;
        wb.merge(0, &r("J5:K5"), false, false);
        wb.set_input(0, c("A8"), "secret");
        wb.set_hidden(0, true, 6, 8, true);
        wb.set_hidden(0, false, 20, 21, true);
        let found = wb.check_accessibility();
        let cats: Vec<Category> = found.iter().map(|i| i.category).collect();
        assert_eq!(cats, vec![Category::NoHeader, Category::MergedCells, Category::HiddenData]);
        assert_eq!(found[2].target, Target::Hidden { sheet: 0, rows: true, from: 6, to: 8 });
        assert_eq!(found[2].label, "Data!7:9");
        assert!(!Category::HiddenData.is_error() && Category::MergedCells.is_error());
        // Hidden sheets are left out.
        wb.execute(crate::Op::SheetProps { index: 0, hidden: true, tab_color: None });
        assert!(wb.check_accessibility().is_empty());
    }
}
