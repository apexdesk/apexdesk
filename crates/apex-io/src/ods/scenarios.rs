//! Scenarios and Solver models in ODF, as LibreOffice keeps them. A
//! scenario is a hidden table right after its sheet, opened by a
//! `<table:scenario>` (ranges, comment, active, protected, border and copy
//! settings) and holding its values in those ranges. Solver models are the
//! Excel names (`solver_opt`...) as named ranges and expressions of the
//! sheet.

use std::collections::HashSet;
use std::fmt::Write as _;

use apex_sheet::analysis::{Scenario, ScenarioInfo};
use apex_sheet::formula::{Expr, parse_canonical};
use apex_sheet::{CellRef, Content, Range, Value, Workbook};

use super::data::{merge_rects, parse_ranges, ranges_addr, split_top};
use crate::solver_names::RawName;
use crate::whatif_values::num_text;
use crate::xml::{Ev, Reader, Tag, esc};

/// Attributes of `<table:scenario>` the model has fields for.
const OWN: [&str; 4] = ["table:scenario-ranges", "table:comment", "table:protected", "table:is-active"];

/// Style of the scenario tables (hidden).
pub(super) const STYLE: &str = r#"<style:style style:name="taScenario" style:family="table"><style:table-properties table:display="false"/></style:style>"#;

/// The attributes of the `<table:scenario>` of the table whose start tag
/// `r` just read, when it is a scenario.
pub(super) fn peek(r: &Reader) -> Option<Vec<(String, String)>> {
    let mut r = r.clone();
    loop {
        match r.next() {
            Ev::Start(t) | Ev::Empty(t) if t.name == "table:scenario" => return Some(t.attrs().map(|(k, v)| (k.to_string(), v.into_owned())).collect()),
            // What the schema allows before it.
            Ev::Start(t) if matches!(t.name, "table:title" | "table:desc" | "table:table-source" | "office:dde-source") => r.skip(),
            Ev::Empty(t) if matches!(t.name, "table:title" | "table:desc" | "table:table-source" | "office:dde-source") => {}
            Ev::Text(_) => {}
            _ => return None,
        }
    }
}

/// A scenario read from its table (`tmp`, sheet 0) and attributes; also
/// whether it is the active one.
pub(super) fn read_table(tmp: &Workbook, name: &str, attrs: &[(String, String)]) -> Option<(Scenario, bool)> {
    let get = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
    let (_, ranges) = parse_ranges(get("table:scenario-ranges")?);
    let mut sc = Scenario { name: name.to_string(), comment: get("table:comment").unwrap_or_default().to_string(), locked: get("table:protected") == Some("true"), ..Default::default() };
    let mut seen = std::collections::HashSet::new();
    'ranges: for r in ranges {
        for row in r.start.row..=r.end.row.min(r.start.row + 1000) {
            for col in r.start.col..=r.end.col.min(r.start.col + 1000) {
                let c = CellRef::new(row, col);
                // Far more changing cells than anyone types in (Excel
                // allows 32), and many scenarios may each ask.
                if sc.changing.len() >= 1 << 12 {
                    break 'ranges;
                }
                if seen.insert(c) {
                    sc.changing.push(c);
                    sc.values.push(tmp.edit_text(0, c));
                }
            }
        }
    }
    sc.kept = attrs.iter().filter(|(k, _)| !OWN.contains(&k.as_str())).cloned().collect();
    (!sc.changing.is_empty()).then_some((sc, get("table:is-active") == Some("true")))
}

/// Keeps the scenarios read: (sheet, scenario, active).
pub(super) fn store(wb: &mut Workbook, list: Vec<(usize, Scenario, bool)>) {
    let mut by_sheet: Vec<(usize, Vec<Scenario>, ScenarioInfo)> = Vec::new();
    for (si, sc, active) in list {
        if by_sheet.last().is_none_or(|x| x.0 != si) {
            by_sheet.push((si, Vec::new(), ScenarioInfo::default()));
        }
        let e = by_sheet.last_mut().unwrap();
        if active {
            e.2.current = Some(e.1.len());
        }
        e.1.push(sc);
    }
    for (si, list, info) in by_sheet {
        crate::scenario_io::store(wb, si, list, info);
    }
}

/// A Solver name of a sheet as the readers pass names (Excel syntax).
pub(super) fn solver_name(t: &Tag, si: usize) -> Option<RawName> {
    let name = t.attr("table:name")?.into_owned();
    let text = if t.name == "table:named-range" {
        let addr = t.attr("table:cell-range-address")?;
        split_top(&addr, &[' ']).iter().filter(|p| !p.is_empty()).map(|p| super::odf_to_canonical(&format!("of:=[{p}]"))).collect::<Vec<_>>().join(",")
    } else {
        let e = t.attr("table:expression")?;
        let body = super::odf_to_canonical(&format!("of:={}", e.trim_start_matches("of:").trim_start_matches('=')));
        // Several areas: "[$S.$A$1:.$A$2];[$S.$C$1]".
        let parts = split_top(e.trim_start_matches("of:").trim_start_matches('='), &[';', '~']);
        if parts.len() > 1 && parts.iter().all(|p| p.trim().starts_with('[')) {
            parts.iter().map(|p| super::odf_to_canonical(&format!("of:={}", p.trim()))).collect::<Vec<_>>().join(",")
        } else {
            body
        }
    };
    Some((name, Some(si), text, String::new(), true))
}

/// `<table:named-range>` and `<table:named-expression>` elements of the
/// Solver model of sheet `si`, hidden as LibreOffice writes them.
pub(super) fn solver_named(wb: &Workbook, si: usize) -> String {
    let mut s = String::new();
    let base = format!("${}.$A$1", super::data::odf_sheet(&wb.sheets[si].name));
    for (_, name, text) in crate::solver_names::write(wb).into_iter().filter(|x| x.0 == si) {
        let parts: Vec<Option<String>> = crate::solver_names::split_commas(&text).into_iter().map(|p| match parse_canonical(p.trim()) {
            Ok(e @ (Expr::Ref { .. } | Expr::Range { .. })) => Some(e.to_odf().trim_start_matches("of:=").to_string()),
            _ => None,
        }).collect();
        if parts.len() == 1 && parts[0].is_some() {
            let addr = parts[0].as_deref().unwrap_or_default();
            let addr = addr.trim_start_matches('[').trim_end_matches(']');
            let _ = write!(s, r#"<table:named-range table:name="{}" table:base-cell-address="{}" table:cell-range-address="{}" table:range-usable-as="hidden"/>"#, esc(&name), esc(&base), esc(addr));
            continue;
        }
        let expr = if parts.iter().all(Option::is_some) {
            parts.into_iter().flatten().collect::<Vec<_>>().join(";")
        } else {
            match parse_canonical(&text) {
                // Numbers and words ("integer") as they are.
                Ok(Expr::Number(_) | Expr::Name(_)) | Err(_) => text.clone(),
                Ok(e) => e.to_odf(),
            }
        };
        let _ = write!(s, r#"<table:named-expression table:name="{}" table:base-cell-address="{}" table:expression="{}" loext:hidden="true"/>"#, esc(&name), esc(&base), esc(&expr));
    }
    s
}

/// A name for a scenario table that no sheet or other scenario has.
fn unique(name: &str, taken: &mut HashSet<String>) -> String {
    let mut out = name.to_string();
    let mut n = 1;
    while taken.contains(&out.to_lowercase()) {
        n += 1;
        out = format!("{name}_{n}");
    }
    taken.insert(out.to_lowercase());
    out
}

/// One cell of a scenario table.
fn cell(wb: &Workbook, text: &str) -> String {
    match wb.content_from_input(text).0 {
        Content::Value(Value::Number(n)) => format!(r#"<table:table-cell office:value-type="float" office:value="{0}"><text:p>{0}</text:p></table:table-cell>"#, num_text(n)),
        Content::Value(Value::Bool(b)) => format!(r#"<table:table-cell office:value-type="boolean" office:boolean-value="{b}"><text:p>{}</text:p></table:table-cell>"#, if b { "TRUE" } else { "FALSE" }),
        Content::Value(Value::Text(t)) => format!(r#"<table:table-cell office:value-type="string"><text:p>{}</text:p></table:table-cell>"#, esc(&t)),
        Content::Formula { expr, .. } => format!(r#"<table:table-cell table:formula="{}"/>"#, esc(&expr.expr().to_odf())),
        _ => "<table:table-cell/>".into(),
    }
}

/// The scenario tables of sheet `sheet`, written after it; `taken` holds
/// the table names used so far (lowercase).
pub(super) fn write_tables(wb: &Workbook, sheet: &str, taken: &mut HashSet<String>, body: &mut String) {
    let Some(list) = wb.analysis.scenarios.get(sheet) else { return };
    let current = wb.analysis.scenario_info.get(sheet).and_then(|i| crate::scenario_io::index(i.current, list.len()));
    for (i, sc) in list.iter().enumerate() {
        let name = unique(&sc.name, taken);
        let rects = merge_rects(sc.changing.iter().map(|c| Range::single(*c)).collect());
        let _ = write!(body, r#"<table:table table:name="{}" table:style-name="taScenario"><table:scenario"#, esc(&name));
        if sc.kept.is_empty() {
            // What LibreOffice sets for scenarios from Excel files.
            body.push_str(r##" table:display-border="false" table:border-color="#c0c0c0" table:copy-back="false" table:copy-styles="false" table:copy-formulas="false""##);
        }
        for (k, v) in &sc.kept {
            let _ = write!(body, r#" {k}="{}""#, esc(v));
        }
        if sc.locked {
            body.push_str(r#" table:protected="true""#);
        }
        let _ = write!(body, r#" table:is-active="{}" table:scenario-ranges="{}""#, current == Some(i), esc(&ranges_addr(&name, &rects)));
        if !sc.comment.is_empty() {
            let _ = write!(body, r#" table:comment="{}""#, esc(&sc.comment));
        }
        body.push_str("/>");
        let mut cells: Vec<(CellRef, &str)> = sc.changing.iter().copied().zip(sc.values.iter().map(String::as_str)).collect();
        cells.sort_by_key(|(c, _)| (c.row, c.col));
        let width = cells.iter().map(|(c, _)| c.col + 1).max().unwrap_or(1);
        let _ = write!(body, r#"<table:table-column table:number-columns-repeated="{width}"/>"#);
        let mut row = 0;
        let mut k = 0;
        while k < cells.len() {
            let r = cells[k].0.row;
            if r > row {
                let _ = write!(body, r#"<table:table-row table:number-rows-repeated="{}"><table:table-cell table:number-columns-repeated="{width}"/></table:table-row>"#, r - row);
            }
            body.push_str("<table:table-row>");
            let mut col = 0;
            while k < cells.len() && cells[k].0.row == r {
                let (c, text) = cells[k];
                if c.col > col {
                    let _ = write!(body, r#"<table:table-cell table:number-columns-repeated="{}"/>"#, c.col - col);
                }
                body.push_str(&cell(wb, text));
                col = c.col + 1;
                k += 1;
            }
            body.push_str("</table:table-row>");
            row = r + 1;
        }
        body.push_str("</table:table>");
    }
}

#[cfg(test)]
mod tests {
    use apex_sheet::analysis::{Constraint, ConstraintOp, SolverGoal, SolverMethod, SolverModel};

    use super::*;

    fn a1(s: &str) -> CellRef {
        CellRef::parse(s).unwrap()
    }

    #[test]
    fn roundtrip() {
        let mut wb = Workbook::new();
        wb.sheets[0].name = "Model".into();
        wb.sheets.push(apex_sheet::Sheet::new("Best"));
        let half = apex_sheet::format::edit_number(2.5, wb.locale.decimal);
        wb.add_scenario(0, Scenario { name: "Best".into(), changing: vec![a1("A1"), a1("A2"), a1("C4")], values: vec!["100".into(), half, "hi".into()], comment: "good".into(), locked: true, ..Default::default() }).unwrap();
        wb.add_scenario(0, Scenario { name: "Worst".into(), changing: vec![a1("B1")], values: vec!["=2*3".into()], ..Default::default() }).unwrap();
        wb.show_scenario(0, "Worst").unwrap();
        let m = SolverModel {
            objective: a1("D1"),
            goal: SolverGoal::Min,
            variables: vec![Range::parse("A1:A2").unwrap(), Range::parse("C1").unwrap()],
            constraints: vec![Constraint { lhs: Range::parse("A1").unwrap(), op: ConstraintOp::Le, rhs: "$C$4".into() }, Constraint { lhs: Range::parse("A2").unwrap(), op: ConstraintOp::Int, rhs: String::new() }],
            method: SolverMethod::SimplexLp,
            kept: vec![("solver_cvg".into(), "0.0001".into())],
            ..Default::default()
        };
        wb.save_solver_model(0, m.clone());
        let data = crate::ods::write(&wb);
        let content = crate::zip::ZipReader::new(&data).unwrap().read_string("content.xml").unwrap();
        // The scenario tables follow their sheet, before the next one; a
        // name a sheet has gets a suffix.
        let at = |s: &str| content.find(s).unwrap_or_else(|| panic!("{s}: {content}"));
        assert!(at(r#"table:name="Model""#) < at(r#"table:name="Best_2""#) && at(r#"table:name="Best_2""#) < at(r#"table:name="Worst""#) && at(r#"table:name="Worst""#) < at(r#"table:name="Best" "#));
        assert!(content.contains(r#"table:protected="true" table:is-active="false" table:scenario-ranges="Best_2.A1:Best_2.A2 Best_2.C4:Best_2.C4" table:comment="good"/>"#), "{content}");
        assert!(content.contains(r#"table:name="solver_adj" table:base-cell-address="$Model.$A$1" table:expression="[$Model.$A$1:.$A$2];[$Model.$C$1]" loext:hidden="true"/>"#), "{content}");
        assert!(content.contains(r#"<table:named-range table:name="solver_opt" table:base-cell-address="$Model.$A$1" table:cell-range-address="$Model.$D$1" table:range-usable-as="hidden"/>"#), "{content}");
        assert!(content.contains(r#"<table:named-expression table:name="solver_rhs2" table:base-cell-address="$Model.$A$1" table:expression="integer" loext:hidden="true"/>"#));
        let back = crate::ods::read(&data).unwrap();
        assert_eq!(back.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Model", "Best"]);
        let mut want = wb.analysis.scenarios["Model"].clone();
        want[0].name = "Best_2".into();
        for s in &mut want {
            s.kept = vec![("table:display-border".into(), "false".into()), ("table:border-color".into(), "#c0c0c0".into()), ("table:copy-back".into(), "false".into()), ("table:copy-styles".into(), "false".into()), ("table:copy-formulas".into(), "false".into())];
        }
        assert_eq!(back.analysis.scenarios["Model"], want);
        assert_eq!(back.analysis.scenario_info["Model"].current, Some(1));
        assert_eq!(back.solver_model(0), Some(&m));
        assert!(back.names.is_empty(), "{:?}", back.names);
        // Written again: the kept attributes stay.
        let again = crate::zip::ZipReader::new(&crate::ods::write(&back)).unwrap().read_string("content.xml").unwrap();
        assert!(again.contains(r##"<table:scenario table:display-border="false" table:border-color="#c0c0c0" table:copy-back="false""##), "{again}");
    }

    /// What LibreOffice writes for a sheet with a Solver model and two
    /// scenarios (it copies the sheet's names into the scenario tables).
    #[test]
    fn libreoffice_file() {
        let names = |s: &str| {
            format!(
                r#"<table:named-expressions><table:named-range table:name="solver_adj" table:base-cell-address="${s}.$A$1" table:cell-range-address="${s}.$A$1:.$A$2" table:range-usable-as="hidden"/><table:named-expression table:name="solver_eng" table:base-cell-address="${s}.$A$1" table:expression="2" loext:hidden="true"/><table:named-expression table:name="solver_lo_eng" table:base-cell-address="${s}.$A$1" table:expression="&quot;com.sun.star.comp.Calc.CoinMPSolver&quot;" loext:hidden="true"/><table:named-expression table:name="solver_num" table:base-cell-address="${s}.$A$1" table:expression="1" loext:hidden="true"/><table:named-range table:name="solver_lhs1" table:base-cell-address="${s}.$A$1" table:cell-range-address="${s}.$A$1" table:range-usable-as="hidden"/><table:named-expression table:name="solver_rel1" table:base-cell-address="${s}.$A$1" table:expression="3" loext:hidden="true"/><table:named-range table:name="solver_rhs1" table:base-cell-address="${s}.$A$1" table:cell-range-address="${s}.$C$1" table:range-usable-as="hidden"/><table:named-range table:name="solver_opt" table:base-cell-address="${s}.$A$1" table:cell-range-address="${s}.$B$1" table:range-usable-as="hidden"/><table:named-expression table:name="solver_typ" table:base-cell-address="${s}.$A$1" table:expression="2" loext:hidden="true"/></table:named-expressions>"#
            )
        };
        let row = |v: &str| format!(r#"<table:table-row><table:table-cell office:value-type="float" office:value="{v}"><text:p>{v}</text:p></table:table-cell></table:table-row>"#);
        let content = format!(
            r##"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:loext="urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0"><office:automatic-styles><style:style style:name="ta2" style:family="table"><style:table-properties table:display="false"/></style:style></office:automatic-styles><office:body><office:spreadsheet>
<table:table table:name="Model"><table:table-column/>{}{}{}</table:table>
<table:table table:name="Best" table:style-name="ta2"><table:scenario table:display-border="true" table:border-color="#c0c0c0" table:copy-back="true" table:copy-styles="false" table:copy-formulas="true" table:protected="true" table:is-active="true" table:scenario-ranges="Best.A1:Best.A2" table:comment="good case"/><table:table-column/>{}{}{}</table:table>
<table:table table:name="Worst" table:style-name="ta2"><table:scenario table:display-border="false" table:border-color="#c0c0c0" table:copy-back="false" table:copy-styles="false" table:copy-formulas="false" table:is-active="false" table:scenario-ranges="Worst.A1:Worst.A2"/><table:table-column/>{}{}{}</table:table>
<table:table table:name="Other"><table:table-column/>{}</table:table>
</office:spreadsheet></office:body></office:document-content>"##,
            row("10"), row("20"), names("Model"), row("100"), row("200"), names("Best"), row("1"), row("2"), names("Worst"), row("1")
        );
        let mut z = crate::zip::ZipWriter::new();
        z.add("mimetype", b"application/vnd.oasis.opendocument.spreadsheet", false);
        z.add("content.xml", content.as_bytes(), true);
        let wb = crate::ods::read(&z.finish()).unwrap();
        assert_eq!(wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Model", "Other"]);
        let list = wb.scenarios(0);
        assert_eq!(list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Best", "Worst"]);
        assert_eq!((list[0].values.clone(), list[0].comment.as_str(), list[0].locked), (vec!["100".to_string(), "200".into()], "good case", true));
        assert_eq!(list[0].kept[0], ("table:display-border".to_string(), "true".to_string()));
        assert_eq!(wb.analysis.scenario_info["Model"].current, Some(0));
        let m = wb.solver_model(0).unwrap();
        assert_eq!((m.objective, m.goal, m.method), (a1("B1"), SolverGoal::Min, SolverMethod::SimplexLp));
        assert_eq!(m.variables, vec![Range::parse("A1:A2").unwrap()]);
        assert_eq!((m.constraints[0].op, m.constraints[0].rhs.as_str()), (ConstraintOp::Ge, "$C$1"));
        assert_eq!(m.kept, vec![("solver_lo_eng".to_string(), "\"com.sun.star.comp.Calc.CoinMPSolver\"".to_string())]);
        assert!(wb.names.is_empty(), "{:?}", wb.names);
        assert!(wb.solver_model(1).is_none());
    }
}
