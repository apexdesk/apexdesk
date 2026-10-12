//! Mail merge: merge fields are written in the text as «Name» (as Word
//! shows them); merging makes one copy of the document per record with
//! the fields replaced, each copy starting on a new page. Each copy is
//! also assembled ([`crate::assemble`]): controls take their fields'
//! values and conditional sections stay or go.

use crate::{Document, Paragraph, Pos, Selection};

pub const FIELD_OPEN: char = '«';
pub const FIELD_CLOSE: char = '»';

/// The text of a merge field.
pub fn field_text(name: &str) -> String {
    format!("{FIELD_OPEN}{name}{FIELD_CLOSE}")
}

/// Names of the merge fields used in the document, in order of first use.
pub fn fields_used(doc: &Document) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in &doc.paragraphs {
        let mut rest = p.text.as_str();
        while let Some(a) = rest.find(FIELD_OPEN) {
            let after = &rest[a + FIELD_OPEN.len_utf8()..];
            let Some(b) = after.find(FIELD_CLOSE) else { break };
            let name = &after[..b];
            // «IF ...» and «END IF» mark sections (see `assemble`).
            let section = crate::assemble::section_marker(name).is_some();
            if !section && !name.is_empty() && !name.contains(FIELD_OPEN) && seen.insert(name) {
                out.push(name.to_string());
            }
            rest = &after[b + FIELD_CLOSE.len_utf8()..];
        }
    }
    out
}

/// A copy of the whole document (styles, headers, notes, images...) with
/// every «field» of `names` replaced by the record's value (formatting of
/// the field text kept). The copy has no undo history.
pub fn fill(doc: &Document, names: &[String], record: &[String]) -> Document {
    let mut d = doc.clone();
    // Filling is not an edit to track.
    let track = std::mem::replace(&mut d.track_changes, false);
    // Named content controls and conditional sections.
    crate::assemble::apply(&mut d, names, record);
    // The first of each name.
    let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (i, n) in names.iter().enumerate() {
        index.entry(n.as_str()).or_insert(i);
    }
    // One pass from the start: each «, the field it opens if that is one
    // of `names`, then on after the value put in (a value holding a
    // «field» is left as it is, not filled again).
    let open = FIELD_OPEN.to_string();
    let mut from = Pos::new(0, 0);
    while let Some(pos) = d.find_text_from(&open, from) {
        let inner = pos.offset + FIELD_OPEN.len_utf8();
        let text = &d.paragraphs[pos.para].text[inner..];
        let field = text.find(FIELD_CLOSE).map(|b| &text[..b]).filter(|n| !n.contains(FIELD_OPEN)).and_then(|n| index.get(n).map(|&i| (i, n.len())));
        let Some((i, len)) = field else {
            from = Pos::new(pos.para, inner);
            continue;
        };
        let value = record.get(i).map_or("", String::as_str);
        let sel = Selection { anchor: pos, cursor: Pos::new(pos.para, inner + len + FIELD_CLOSE.len_utf8()) };
        // The value takes the formatting of the field, not of the text
        // before it.
        let attrs = d.paragraphs[pos.para].attrs_at(pos.offset + FIELD_OPEN.len_utf8());
        from = d.insert_text(sel, value, Some(attrs));
    }
    d.track_changes = track;
    d.undo.clear();
    d.redo.clear();
    d.modified = true;
    d
}

/// One copy of `doc` per record, each one starting on a new page. `names`
/// are the data source's field names, `records` its rows.
pub fn merge(doc: &Document, names: &[String], records: &[Vec<String>]) -> Document {
    let max_table = doc.paragraphs.iter().filter_map(|p| p.cell.as_ref().map(|c| c.table)).max().unwrap_or(0) + 1;
    let mut paras: Vec<Paragraph> = Vec::new();
    for (k, rec) in records.iter().enumerate() {
        let filled = fill(doc, names, rec);
        let mut ps = filled.paragraphs;
        for p in &mut ps {
            // Tables of each copy stay separate tables; bookmarks stay unique.
            if let Some(c) = p.cell.as_mut() {
                c.table += k as u32 * max_table;
            }
            if k > 0 {
                p.marks.clear();
            }
        }
        if k > 0
            && let Some(first) = ps.first_mut()
        {
            if first.cell.is_some() {
                // A page break needs a paragraph outside the table.
                let mut brk = Paragraph::new("");
                brk.style.page_break_before = true;
                paras.push(brk);
            } else {
                first.style.page_break_before = true;
            }
        }
        paras.extend(ps);
    }
    // Everything but the text (styles, headers, notes, page...) comes from
    // the template.
    let mut out = doc.clone();
    out.paragraphs = Document::from_paragraphs(paras).paragraphs;
    out.undo.clear();
    out.redo.clear();
    out.modified = true;
    out
}

// --- Labels and envelopes -------------------------------------------------------------------

const MM: f32 = 72.0 / 25.4;

/// A sheet of labels (sizes in millimetres).
pub struct LabelSheet {
    pub name: &'static str,
    /// Page width and height.
    pub page: (f32, f32),
    pub top: f32,
    pub left: f32,
    pub width: f32,
    pub height: f32,
    /// Distance between the left edges of neighbouring labels, and between
    /// their top edges.
    pub h_pitch: f32,
    pub v_pitch: f32,
    pub cols: usize,
    pub rows: usize,
}

const A4: (f32, f32) = (210.0, 297.0);
const LETTER: (f32, f32) = (215.9, 279.4);

pub const LABELS: &[LabelSheet] = &[
    LabelSheet { name: "Avery L7160 (63.5 x 38.1 mm, 21)", page: A4, top: 15.15, left: 7.25, width: 63.5, height: 38.1, h_pitch: 66.0, v_pitch: 38.1, cols: 3, rows: 7 },
    LabelSheet { name: "Avery L7159 (63.5 x 33.9 mm, 24)", page: A4, top: 12.9, left: 7.25, width: 63.5, height: 33.9, h_pitch: 66.0, v_pitch: 33.9, cols: 3, rows: 8 },
    LabelSheet { name: "Avery L7163 (99.1 x 38.1 mm, 14)", page: A4, top: 15.15, left: 4.65, width: 99.1, height: 38.1, h_pitch: 101.6, v_pitch: 38.1, cols: 2, rows: 7 },
    LabelSheet { name: "Avery L7165 (99.1 x 67.7 mm, 8)", page: A4, top: 13.1, left: 4.65, width: 99.1, height: 67.7, h_pitch: 101.6, v_pitch: 67.7, cols: 2, rows: 4 },
    LabelSheet { name: "Avery L7651 (38.1 x 21.2 mm, 65)", page: A4, top: 10.7, left: 4.75, width: 38.1, height: 21.2, h_pitch: 40.6, v_pitch: 21.2, cols: 5, rows: 13 },
    LabelSheet { name: "Avery 5160 (1\" x 2 5/8\", 30)", page: LETTER, top: 12.7, left: 4.8, width: 66.7, height: 25.4, h_pitch: 69.85, v_pitch: 25.4, cols: 3, rows: 10 },
    LabelSheet { name: "Avery 5163 (2\" x 4\", 10)", page: LETTER, top: 12.7, left: 4.0, width: 101.6, height: 50.8, h_pitch: 104.8, v_pitch: 50.8, cols: 2, rows: 5 },
];

fn text_paras(text: &str, size_half_pt: u16) -> Vec<Paragraph> {
    text.lines()
        .map(|l| {
            let mut p = Paragraph::new(l);
            p.style.space_before = Some(0.0);
            p.style.space_after = Some(0.0);
            for r in &mut p.runs {
                r.attrs.size = Some(size_half_pt);
            }
            p
        })
        .collect()
}

/// A document with one label per text (lines separated by '\n'), laid
/// out in a borderless table that matches the label sheet.
pub fn labels(sheet: &LabelSheet, texts: &[String]) -> Document {
    use crate::{Cell, CellProps, Table, TableProps, VAlign};
    let gap = sheet.h_pitch - sheet.width;
    let spacer = gap > 0.5;
    let ncols = if spacer { sheet.cols * 2 - 1 } else { sheet.cols };
    let mut widths = Vec::new();
    for c in 0..sheet.cols {
        if c > 0 && spacer {
            widths.push(gap * MM);
        }
        widths.push(sheet.width * MM);
    }
    let count = texts.len().max(1);
    let nrows = count.div_ceil(sheet.cols);
    let props = TableProps { widths: widths.clone(), style: None, row_heights: vec![sheet.v_pitch * MM; nrows], ..Default::default() };
    let mut rows = Vec::new();
    for r in 0..nrows {
        let mut row = Vec::new();
        for c in 0..ncols {
            let (label, is_spacer) = if spacer { (c / 2, c % 2 == 1) } else { (c, false) };
            let idx = r * sheet.cols + label;
            let mut cell = Cell::empty();
            if !is_spacer
                && let Some(t) = texts.get(idx)
            {
                let ps = text_paras(t, 20);
                if !ps.is_empty() {
                    cell.paras = ps;
                }
                for p in &mut cell.paras {
                    p.style.indent_left = 6.0;
                }
            }
            cell.props = CellProps { valign: VAlign::Center, ..CellProps::default() };
            row.push(cell);
        }
        rows.push(row);
    }
    let table = Table { props, rows };
    let mut paras = table.into_paras(1);
    paras.push(Paragraph::new(""));
    let mut d = Document::from_paragraphs(paras);
    let used_w = (sheet.cols - 1) as f32 * sheet.h_pitch + sheet.width;
    let used_h = (sheet.rows - 1) as f32 * sheet.v_pitch + sheet.height;
    d.page = crate::PageSetup {
        width: sheet.page.0 * MM,
        height: sheet.page.1 * MM,
        margin_top: sheet.top * MM,
        margin_left: sheet.left * MM,
        margin_right: (sheet.page.0 - sheet.left - used_w).max(0.0) * MM,
        // A hair more room than the rows need, so that exactly `rows` fit.
        margin_bottom: (sheet.page.1 - sheet.top - used_h - 0.5).max(0.0) * MM,
        margin_header: 0.0,
        margin_footer: 0.0,
        line_numbers: None,
        ..crate::PageSetup::A4
    };
    d.modified = true;
    d
}

/// An envelope size (millimetres).
pub struct Envelope {
    pub name: &'static str,
    pub width: f32,
    pub height: f32,
}

pub const ENVELOPES: &[Envelope] = &[
    Envelope { name: "DL (220 x 110 mm)", width: 220.0, height: 110.0 },
    Envelope { name: "C5 (229 x 162 mm)", width: 229.0, height: 162.0 },
    Envelope { name: "C4 (324 x 229 mm)", width: 324.0, height: 229.0 },
    Envelope { name: "C6 (162 x 114 mm)", width: 162.0, height: 114.0 },
    Envelope { name: "No. 10 (4 1/8\" x 9 1/2\")", width: 241.3, height: 104.8 },
];

/// A document with one envelope (page) per delivery address, the return
/// address at the top left and the delivery address right of the middle.
pub fn envelopes(env: &Envelope, return_address: &str, addresses: &[String]) -> Document {
    let margin = 12.0 * MM;
    let (w, h) = (env.width * MM, env.height * MM);
    let ret = text_paras(return_address, 18);
    let ret_h = ret.len() as f32 * 12.5;
    let mut paras = Vec::new();
    let one = [String::new()];
    let list = if addresses.is_empty() { &one[..] } else { addresses };
    for (k, addr) in list.iter().enumerate() {
        let mut block = ret.clone();
        let mut to = text_paras(addr, 24);
        if to.is_empty() {
            to.push(Paragraph::new(""));
        }
        for p in &mut to {
            p.style.indent_left = w * 0.45 - margin;
        }
        // The delivery address starts a little below the middle.
        to[0].style.space_before = Some((h * 0.45 - margin - ret_h).max(12.0));
        block.extend(to);
        if k > 0 {
            block[0].style.page_break_before = true;
        }
        paras.extend(block);
    }
    let mut d = Document::from_paragraphs(paras);
    d.page = crate::PageSetup { width: w, height: h, margin_top: margin, margin_bottom: margin * 0.5, margin_left: margin, margin_right: margin, margin_header: 0.0, margin_footer: 0.0, line_numbers: None, ..crate::PageSetup::A4 };
    d.modified = true;
    d
}

impl Document {
    /// The first occurrence of `needle` (case-sensitive).
    pub fn find_text(&self, needle: &str) -> Option<Pos> {
        self.find_text_from(needle, Pos::new(0, 0))
    }

    /// The first `needle` at or after `from`.
    pub fn find_text_from(&self, needle: &str, from: Pos) -> Option<Pos> {
        self.paragraphs.iter().enumerate().skip(from.para).find_map(|(i, p)| {
            let start = if i == from.para { from.offset.min(p.text.len()) } else { 0 };
            p.text.get(start..)?.find(needle).map(|o| Pos::new(i, start + o))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_records() {
        let mut d = Document::new();
        let p = d.insert_text(Selection::caret(Pos::new(0, 0)), "Dear «Name», you owe «Amount».", None);
        d.new_paragraph(Selection::caret(p));
        assert_eq!(fields_used(&d), ["Name", "Amount"]);
        let names = vec!["Name".to_string(), "Amount".to_string()];
        let recs = vec![vec!["Ann".to_string(), "10".to_string()], vec!["Bob".to_string()]];
        let m = merge(&d, &names, &recs);
        assert_eq!(m.paragraphs[0].text, "Dear Ann, you owe 10.");
        assert_eq!(m.paragraphs[2].text, "Dear Bob, you owe .");
        assert!(m.paragraphs[2].style.page_break_before);
        assert!(!m.paragraphs[0].style.page_break_before);
    }

    #[test]
    fn values_holding_fields_are_not_filled_again() {
        let mut d = Document::new();
        d.insert_text(Selection::caret(Pos::new(0, 0)), "«Name» owes «Amount».", None);
        let names = vec!["Name".to_string(), "Amount".to_string()];
        let m = fill(&d, &names, &["x«Name» «Amount»".to_string(), "10".to_string()]);
        assert_eq!(m.paragraphs[0].text, "x«Name» «Amount» owes 10.");
    }

    #[test]
    fn labels_and_envelopes() {
        let texts: Vec<String> = (0..25).map(|i| format!("Name {i}\nStreet {i}\nCity")).collect();
        let d = labels(&LABELS[0], &texts);
        let t = d.tables();
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].rows, t[0].cols), (9, 5));
        assert_eq!(d.paragraphs[0].text, "Name 0");
        let e = envelopes(&ENVELOPES[0], "Me\nHere", &["You\nThere".to_string(), "Other".to_string()]);
        assert!(e.page.width > e.page.height);
        assert_eq!(e.paragraphs.iter().filter(|p| p.style.page_break_before).count(), 1);
    }
}
