//! Scenarios (Scenario Manager) in Excel files: `<scenarios>` of an .xlsx
//! worksheet (read and written) and the `SCENMAN` and `SCENARIO` records of
//! an .xls sheet (read). Values are kept as typed in the workbook's locale;
//! files hold them locale-neutral (see [`crate::whatif_values`]).

use std::fmt::Write as _;

use apex_sheet::analysis::{Scenario, ScenarioInfo};
use apex_sheet::{CellRef, Range, Workbook};

use crate::biff::Cur;
use crate::whatif_values::{file_value, typed_value};
use crate::xls::Rec;
use crate::xml::{Ev, Reader, esc};

fn flag(v: Option<std::borrow::Cow<'_, str>>) -> bool {
    v.is_some_and(|v| v == "1" || v == "true")
}

fn cell(s: &str) -> Option<CellRef> {
    CellRef::parse(&s.replace('$', ""))
}

/// Ranges of a `sqref` list ("A1:B2 D4").
fn sqref(s: &str) -> Vec<Range> {
    s.split_whitespace().filter_map(|p| {
        let p = p.replace('$', "");
        Range::parse(&p).or_else(|| CellRef::parse(&p).map(Range::single))
    }).collect()
}

/// Reads the `<scenarios>` of worksheet `si`.
pub(crate) fn read_xlsx(xml: &str, wb: &mut Workbook, si: usize) {
    let Some(at) = xml.find("<scenarios").or_else(|| xml.find(":scenarios")) else { return };
    let mut list: Vec<Scenario> = Vec::new();
    let mut info = ScenarioInfo::default();
    let mut r = Reader::new(&xml[xml[..at].rfind('<').unwrap_or(at)..]);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::End(n) if crate::xml::local(n) == "scenarios" => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "scenarios" => {
                info.current = t.attr("current").and_then(|v| v.parse().ok());
                info.selected = t.attr("show").and_then(|v| v.parse().ok());
                info.results = t.attr("sqref").map_or(Vec::new(), |v| sqref(&v));
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "scenario" => {
                let s = |k: &str| t.attr(k).unwrap_or_default().into_owned();
                list.push(Scenario { name: s("name"), comment: s("comment"), user: s("user"), locked: flag(t.attr("locked")), hidden: flag(t.attr("hidden")), ..Default::default() });
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "inputCells" => {
                if let (Some(sc), Some(c)) = (list.last_mut(), t.attr("r").and_then(|v| cell(&v))) {
                    sc.changing.push(c);
                    sc.values.push(typed_value(wb, &t.attr("val").unwrap_or_default()));
                }
            }
            _ => {}
        }
    }
    store(wb, si, list, info);
}

/// Keeps the scenarios read for sheet `si`.
pub(crate) fn store(wb: &mut Workbook, si: usize, list: Vec<Scenario>, info: ScenarioInfo) {
    let list: Vec<Scenario> = list.into_iter().filter(|s| !s.name.is_empty() && !s.changing.is_empty()).collect();
    if list.is_empty() {
        return;
    }
    let name = wb.sheets[si].name.clone();
    if info != ScenarioInfo::default() {
        wb.analysis.scenario_info.insert(name.clone(), info);
    }
    wb.analysis.scenarios.insert(name, list);
}

/// The index of `i` when it points into a list of `n`.
pub(crate) fn index(i: Option<usize>, n: usize) -> Option<usize> {
    i.filter(|&i| i < n)
}

/// `<scenarios>` of a worksheet (between `<protectedRanges>` and
/// `<autoFilter>`).
pub(crate) fn write_xlsx(s: &mut String, wb: &Workbook, sheet: &str) {
    let Some(list) = wb.analysis.scenarios.get(sheet).filter(|l| !l.is_empty()) else { return };
    let info = wb.analysis.scenario_info.get(sheet).cloned().unwrap_or_default();
    s.push_str("<scenarios");
    if let Some(i) = index(info.current, list.len()) {
        let _ = write!(s, r#" current="{i}""#);
    }
    if let Some(i) = index(info.selected, list.len()) {
        let _ = write!(s, r#" show="{i}""#);
    }
    if !info.results.is_empty() {
        let _ = write!(s, r#" sqref="{}""#, info.results.iter().map(Range::to_a1).collect::<Vec<_>>().join(" "));
    }
    s.push('>');
    for sc in list {
        let _ = write!(s, r#"<scenario name="{}""#, esc(&sc.name));
        if sc.locked {
            s.push_str(r#" locked="1""#);
        }
        if sc.hidden {
            s.push_str(r#" hidden="1""#);
        }
        let _ = write!(s, r#" count="{}""#, sc.changing.len());
        if !sc.user.is_empty() {
            let _ = write!(s, r#" user="{}""#, esc(&sc.user));
        }
        if !sc.comment.is_empty() {
            let _ = write!(s, r#" comment="{}""#, esc(&sc.comment));
        }
        s.push('>');
        for (c, v) in sc.changing.iter().zip(&sc.values) {
            let _ = write!(s, r#"<inputCells r="{}" val="{}"/>"#, c.to_a1(), esc(&file_value(wb, v)));
        }
        s.push_str("</scenario>");
    }
    s.push_str("</scenarios>");
}

/// `XLUnicodeString`: a 16-bit length, flags and the characters.
fn xl_string(c: &mut Cur) -> Option<String> {
    let n = c.u16()? as usize;
    let wide = c.u8()? & 1 != 0;
    c.chars8(n, wide)
}

/// A `SCENARIO` record (BIFF8).
fn read_record(d: &[u8], wb: &Workbook) -> Option<Scenario> {
    let mut c = Cur::new(d);
    let n = c.u16()? as usize;
    let locked = c.u8()? & 1 != 0;
    let hidden = c.u8()? & 1 != 0;
    let (cch_name, cch_comment, _cch_user) = (c.u8()? as usize, c.u8()?, c.u8()?);
    let wide = c.u8()? & 1 != 0;
    let name = c.chars8(cch_name, wide)?;
    let user = xl_string(&mut c)?;
    let comment = if cch_comment > 0 { xl_string(&mut c)? } else { String::new() };
    let mut changing = Vec::with_capacity(n);
    for _ in 0..n {
        let (row, col) = (c.u16()?, c.u16()?);
        changing.push(CellRef::new(u32::from(row), u32::from(col)));
    }
    let values = (0..n).map(|_| xl_string(&mut c).map(|v| typed_value(wb, &v))).collect::<Option<Vec<_>>>()?;
    Some(Scenario { name, changing, values, comment, locked, hidden, user, kept: Vec::new() })
}

/// The scenarios of an .xls sheet, from its records (starting at its
/// `BOF`). BIFF5 files are left out.
pub(crate) fn read_xls(recs: &[Rec], wb: &mut Workbook, si: usize, biff5: bool) {
    if biff5 {
        return;
    }
    let (mut list, mut info) = (Vec::new(), ScenarioInfo::default());
    let mut depth = 0;
    for r in recs {
        match r.ty {
            0x0809 => depth += 1,
            0x000A => {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
            }
            _ if depth > 1 => {}
            // SCENMAN: count, current, shown and the result cells.
            0x00AE => {
                let mut c = Cur::new(&r.data);
                let (_, current, shown, n) = (c.u16(), c.u16(), c.u16(), c.u16().unwrap_or(0));
                info.current = current.map(usize::from);
                info.selected = shown.map(usize::from);
                for _ in 0..n {
                    let (Some(row), Some(col)) = (c.u16(), c.u16()) else { break };
                    info.results.push(Range::single(CellRef::new(u32::from(row), u32::from(col))));
                }
            }
            0x00AF => list.extend(read_record(&r.data, wb)),
            _ => {}
        }
    }
    if info.current == Some(0) && info.selected == Some(0) && info.results.is_empty() {
        info = ScenarioInfo::default();
    }
    store(wb, si, list, info);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wb_with_scenarios() -> Workbook {
        // Values typed in the locale of the machine, as files are read.
        let mut wb = Workbook::new();
        let half = apex_sheet::format::edit_number(2.5, wb.locale.decimal);
        wb.set_input(0, CellRef::new(0, 0), "10");
        wb.set_input(0, CellRef::new(1, 0), "20");
        wb.set_input(0, CellRef::new(0, 1), "=A1*2+A2");
        let a = |s: &str| CellRef::parse(s).unwrap();
        wb.add_scenario(0, Scenario { name: "Best & co".into(), changing: vec![a("A1"), a("A2")], values: vec!["100".into(), half], comment: "good <case>".into(), locked: true, user: "Ann".into(), ..Default::default() }).unwrap();
        wb.add_scenario(0, Scenario { name: "Worst".into(), changing: vec![a("A1")], values: vec!["=5*2".into()], hidden: true, ..Default::default() }).unwrap();
        wb.show_scenario(0, "Worst").unwrap();
        wb
    }

    #[test]
    fn xlsx_roundtrip() {
        let wb = wb_with_scenarios();
        let data = crate::xlsx::write(&wb);
        let z = crate::zip::ZipReader::new(&data).unwrap();
        let sheet = z.read_string("xl/worksheets/sheet1.xml").unwrap();
        assert!(sheet.contains(r#"<scenarios current="1"><scenario name="Best &amp; co" locked="1" count="2" user="Ann" comment="good &lt;case&gt;"><inputCells r="A1" val="100"/>"#), "{sheet}");
        // Schema order: after the sheet data, before the autofilter and merges.
        assert!(sheet.find("</sheetData>").unwrap() < sheet.find("<scenarios").unwrap());
        let mut back = crate::xlsx::read(&data).unwrap();
        assert_eq!(back.analysis.scenarios, wb.analysis.scenarios);
        assert_eq!(back.analysis.scenario_info, wb.analysis.scenario_info);
        back.show_scenario(0, "Best & co").unwrap();
        assert_eq!(back.sheets[0].value(CellRef::new(0, 1)), &apex_sheet::Value::Number(202.5));
    }

    #[test]
    fn xls_records() {
        // What LibreOffice writes: SCENMAN and two SCENARIO records.
        let hex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        let rec = |ty: u16, h: &str| Rec { ty, offset: 0, data: hex(h), breaks: Vec::new() };
        let recs = vec![
            rec(0x0809, ""),
            rec(0x00AE, "020001000100010000000300"),
            rec(0x00AF, "02000100040904004265737404000043616c63090000676f6f642063617365000000000100000003000031303003000032303000000000"),
            rec(0x00AF, "0200000005000400576f72737404000043616c630000000001000000010000310100003200000000"),
            rec(0x000A, ""),
        ];
        let mut wb = Workbook::new();
        read_xls(&recs, &mut wb, 0, false);
        let list = wb.scenarios(0);
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].name.as_str(), list[0].user.as_str(), list[0].comment.as_str(), list[0].locked), ("Best", "Calc", "good case", true));
        assert_eq!(list[0].changing, vec![CellRef::new(0, 0), CellRef::new(1, 0)]);
        assert_eq!(list[0].values, vec!["100", "200"]);
        assert_eq!((list[1].name.as_str(), list[1].comment.as_str(), list[1].locked), ("Worst", "", false));
        let info = &wb.analysis.scenario_info["Sheet1"];
        assert_eq!((info.current, info.selected, info.results.clone()), (Some(1), Some(1), vec![Range::parse("D1").unwrap()]));
    }
}
