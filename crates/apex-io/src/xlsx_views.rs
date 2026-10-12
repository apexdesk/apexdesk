//! Custom views in xlsx: `<customWorkbookViews>` in the workbook and
//! `<customSheetViews>` in each sheet.

use std::borrow::Cow;
use std::fmt::Write as _;

use apex_sheet::data::CfStyle;
use apex_sheet::views::{CustomView, Hidden, SheetView};
use apex_sheet::{CellRef, Range, Sheet, Workbook};

use crate::xml::{Ev, Reader, esc};

/// `<customWorkbookViews>`, after `<calcPr>`.
pub fn write_book(w: &mut String, wb: &Workbook) {
    if wb.custom_views.is_empty() {
        return;
    }
    w.push_str("<customWorkbookViews>");
    for v in &wb.custom_views {
        let id = wb.sheet_index(&v.active).unwrap_or(0) + 1;
        let _ = write!(w, r#"<customWorkbookView name="{}" guid="{}" maximized="1" windowWidth="1600" windowHeight="900" activeSheetId="{id}"/>"#, esc(&v.name), esc(&v.guid));
    }
    w.push_str("</customWorkbookViews>");
}

/// `<customSheetViews>`, between the autofilter and the merged cells.
pub fn write_sheet(s: &mut String, sh: &Sheet, dxfs: &[CfStyle]) {
    if sh.custom_views.is_empty() {
        return;
    }
    s.push_str("<customSheetViews>");
    for v in &sh.custom_views {
        let _ = write!(s, r#"<customSheetView guid="{}""#, esc(&v.guid));
        if v.zoom != 100 {
            let _ = write!(s, r#" scale="{}""#, v.zoom.clamp(10, 400));
        }
        if v.hidden_sheet {
            s.push_str(r#" state="hidden""#);
        }
        if let Some(h) = &v.hidden {
            if h.filter.is_some() {
                s.push_str(r#" filter="1" showAutoFilter="1""#);
            }
            if !h.rows.is_empty() {
                s.push_str(r#" hiddenRows="1""#);
            }
            if !h.cols.is_empty() {
                s.push_str(r#" hiddenColumns="1""#);
            }
        }
        if v.page.as_ref().is_some_and(|p| p.fit.is_some()) {
            s.push_str(r#" fitToPage="1""#);
        }
        if v.top_left != CellRef::new(0, 0) {
            let _ = write!(s, r#" topLeftCell="{}""#, v.top_left.to_a1());
        }
        s.push('>');
        let (rows, cols) = v.frozen;
        if rows > 0 || cols > 0 {
            let pane = match (rows > 0, cols > 0) {
                (true, true) => "bottomRight",
                (true, false) => "bottomLeft",
                _ => "topRight",
            };
            s.push_str("<pane");
            if cols > 0 {
                let _ = write!(s, r#" xSplit="{cols}""#);
            }
            if rows > 0 {
                let _ = write!(s, r#" ySplit="{rows}""#);
            }
            let _ = write!(s, r#" topLeftCell="{}" activePane="{pane}" state="frozen"/>"#, CellRef::new(rows, cols).to_a1());
        }
        let _ = write!(s, r#"<selection activeCell="{}" sqref="{}"/>"#, v.cursor.to_a1(), v.selection.to_a1());
        if let Some(page) = &v.page {
            let mut tmp = Sheet::new("");
            tmp.page = (**page).clone();
            let mut part = String::new();
            crate::xlsx_page::write_sheet_page(&mut part, &tmp);
            // The view's own order: breaks, margins, options, setup, header.
            for tag in ["rowBreaks", "colBreaks", "pageMargins", "printOptions", "pageSetup", "headerFooter"] {
                s.push_str(element(&part, tag));
            }
        }
        if let Some(f) = v.hidden.as_ref().and_then(|h| h.filter.as_ref()) {
            crate::xlsx_data::write_filter(s, f, dxfs);
        }
        s.push_str("</customSheetView>");
    }
    s.push_str("</customSheetViews>");
}

/// The first `<tag …/>` or `<tag …>…</tag>` in `xml` ("" if none).
fn element<'a>(xml: &'a str, tag: &str) -> &'a str {
    let open = format!("<{tag}");
    let Some(i) = xml.match_indices(&open).map(|(i, _)| i).find(|&i| xml[i + open.len()..].starts_with([' ', '>', '/'])) else { return "" };
    let rest = &xml[i..];
    let Some(gt) = rest.find('>') else { return "" };
    if rest[..gt].ends_with('/') {
        return &rest[..=gt];
    }
    let close = format!("</{tag}>");
    rest.find(&close).map_or("", |e| &rest[..e + close.len()])
}

/// The sheet XML without its `<customSheetViews>`, so the other readers
/// don't take a view's page setup or filter for the sheet's own.
pub fn strip(xml: &str) -> Cow<'_, str> {
    let block = element(xml, "customSheetViews");
    if block.is_empty() { Cow::Borrowed(xml) } else { Cow::Owned(xml.replacen(block, "", 1)) }
}

/// This sheet's part of the custom views.
pub fn read_sheet(xml: &str, sh: &mut Sheet, dxfs: &[CfStyle]) {
    let block = element(xml, "customSheetViews");
    if block.is_empty() {
        return;
    }
    let mut rest = block;
    while let Some(i) = rest.find("<customSheetView ") {
        let one = element(&rest[i..], "customSheetView");
        if one.is_empty() {
            break;
        }
        rest = &rest[i + one.len()..];
        let mut r = Reader::new(one);
        let mut v = SheetView { guid: String::new(), zoom: 100, top_left: CellRef::new(0, 0), cursor: CellRef::new(0, 0), selection: Range::single(CellRef::new(0, 0)), frozen: (0, 0), hidden_sheet: false, hidden: None, page: None };
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) | Ev::Empty(t) => match t.local() {
                    "customSheetView" => {
                        v.guid = t.attr("guid").unwrap_or_default().into_owned();
                        v.zoom = t.attr("scale").and_then(|x| x.parse().ok()).unwrap_or(100);
                        v.top_left = t.attr("topLeftCell").and_then(|x| CellRef::parse(&x)).unwrap_or(v.top_left);
                        v.hidden_sheet = t.attr("state").is_some_and(|x| x == "hidden" || x == "veryHidden");
                        let on = |k: &str| t.attr(k).is_some_and(|x| x == "1" || x == "true");
                        if on("filter") || on("hiddenRows") || on("hiddenColumns") {
                            v.hidden = Some(Hidden::default());
                        }
                    }
                    "pane" if t.attr("state").is_some_and(|x| x == "frozen" || x == "frozenSplit") => {
                        let n = |k: &str| t.attr(k).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) as u32;
                        v.frozen = (n("ySplit"), n("xSplit"));
                    }
                    "selection" => {
                        if let Some(c) = t.attr("activeCell").and_then(|x| CellRef::parse(&x)) {
                            v.cursor = c;
                        }
                        let first = t.attr("sqref").and_then(|x| x.split_whitespace().next().and_then(|a| Range::parse(a).or_else(|| CellRef::parse(a).map(Range::single))));
                        v.selection = first.unwrap_or(Range::single(v.cursor));
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if ["pageMargins", "pageSetup", "printOptions", "headerFooter"].iter().any(|t| !element(one, t).is_empty()) {
            let mut page = apex_sheet::SheetPage::default();
            crate::xlsx_page::read_sheet_page(one, &mut page);
            if one.contains(r#"fitToPage="1""#) && page.fit.is_none() {
                page.fit = Some((1, 1));
            }
            v.page = Some(Box::new(page));
        }
        let filter = element(one, "autoFilter");
        if !filter.is_empty() {
            let mut tmp = Sheet::new("");
            crate::xlsx_data::read_tools(&format!("</sheetData>{filter}"), &mut tmp, dxfs);
            v.hidden.get_or_insert_with(Hidden::default).filter = tmp.filter;
        }
        sh.custom_views.push(v);
    }
}

/// The views listed in the workbook (active sheet by its 1-based id).
pub fn read_book(xml: &str, wb: &mut Workbook, sheet_names: &[String]) {
    let block = element(xml, "customWorkbookViews");
    let mut r = Reader::new(block);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "customWorkbookView" => {
                let id: usize = t.attr("activeSheetId").and_then(|x| x.parse().ok()).unwrap_or(1);
                wb.custom_views.push(CustomView {
                    name: t.attr("name").unwrap_or_default().into_owned(),
                    guid: t.attr("guid").unwrap_or_default().into_owned(),
                    active: sheet_names.get(id.saturating_sub(1)).cloned().unwrap_or_default(),
                });
            }
            _ => {}
        }
    }
}
