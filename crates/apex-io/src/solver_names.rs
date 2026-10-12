//! Solver models as Excel saves them: hidden names local to the sheet
//! (`solver_opt`, `solver_adj`, `solver_lhs1`...), which LibreOffice also
//! reads and writes, in .xlsx and in .ods. Reading turns them into the
//! sheet's `SolverModel`, so they don't show as defined names; the ones we
//! don't use are kept in the model and saved back.

use std::collections::BTreeMap;

use apex_sheet::analysis::{Constraint, ConstraintOp, SolverGoal, SolverMethod, SolverModel};
use apex_sheet::formula::{Expr, parse_canonical};
use apex_sheet::{CellRef, Range, Workbook};

use crate::whatif_values::num_text;

/// A defined name as the readers pass it: name, local sheet, formula text
/// (Excel syntax), comment, hidden.
pub(crate) type RawName = (String, Option<usize>, String, String, bool);

/// Excel's "no limit" for time and iterations.
const UNLIMITED: f64 = 2_147_483_647.0;

pub(crate) fn is_solver_name(name: &str) -> bool {
    name.len() > 7 && name.is_char_boundary(7) && name[..7].eq_ignore_ascii_case("solver_")
}

/// Takes the Solver names out of `list` into the sheets' models; returns
/// the other names, and the Solver names of sheets whose model can't be
/// read (they stay ordinary hidden names). Hidden workbook-level Solver
/// names are dropped.
pub(crate) fn read(wb: &mut Workbook, list: Vec<RawName>) -> Vec<RawName> {
    let mut by_sheet: BTreeMap<usize, Vec<RawName>> = BTreeMap::new();
    let mut rest = Vec::new();
    for n in list {
        match n.1 {
            Some(si) if is_solver_name(&n.0) && si < wb.sheets.len() => by_sheet.entry(si).or_default().push(n),
            // LibreOffice's copies of the names of its scenario tables,
            // which have no sheet in the file; Solver only uses local ones.
            None if is_solver_name(&n.0) && n.4 => {}
            _ => rest.push(n),
        }
    }
    for (si, names) in by_sheet {
        let map: BTreeMap<String, (String, String)> = names.iter().map(|n| (n.0.to_ascii_lowercase(), (n.0.clone(), n.2.trim().trim_start_matches('=').to_string()))).collect();
        match model_of(wb, &wb.sheets[si].name, &map) {
            Some(m) => {
                let key = wb.sheets[si].name.clone();
                wb.analysis.solver_models.insert(key, m);
            }
            None => rest.extend(names),
        }
    }
    rest
}

/// The cells of `text` (`Sheet1!$A$1:$B$2,Sheet1!$D$1`) on sheet `sheet`.
fn ranges(text: &str, sheet: &str) -> Option<Vec<Range>> {
    let t = text.trim();
    let t = t.strip_prefix('(').and_then(|x| x.strip_suffix(')')).unwrap_or(t);
    let mut out = Vec::new();
    for part in split_commas(t) {
        let (sh, r) = match parse_canonical(part.trim()).ok()? {
            Expr::Ref { sheet, addr } => (sheet, Range::single(CellRef::new(addr.row, addr.col))),
            Expr::Range { sheet, a, b, .. } => (sheet, Range::new(CellRef::new(a.row, a.col), CellRef::new(b.row, b.col))),
            _ => return None,
        };
        if sh.is_some_and(|s| !s.eq_ignore_ascii_case(sheet)) {
            return None;
        }
        out.push(r);
    }
    (!out.is_empty()).then_some(out)
}

/// Splits at the commas outside quotes and parentheses.
pub(crate) fn split_commas(s: &str) -> Vec<&str> {
    let (mut out, mut depth, mut quote, mut start) = (Vec::new(), 0i32, None, 0);
    for (i, c) in s.char_indices() {
        match c {
            '\'' | '"' if quote.is_none() => quote = Some(c),
            c if quote == Some(c) => quote = None,
            '(' if quote.is_none() => depth += 1,
            ')' if quote.is_none() => depth -= 1,
            ',' if quote.is_none() && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

/// Removes the sheet name from references to `sheet`.
fn unqualify(e: &mut Expr, sheet: &str) {
    e.visit_refs_mut(&mut |r| {
        if let Expr::Ref { sheet: s, .. } | Expr::Range { sheet: s, .. } = r {
            if s.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(sheet)) {
                *s = None;
            }
        }
    });
}

/// A right side as typed in the dialog.
fn typed_rhs(wb: &Workbook, text: &str, sheet: &str) -> String {
    match parse_canonical(text) {
        Ok(Expr::Number(n)) => apex_sheet::format::edit_number(n, wb.locale.decimal),
        Ok(mut e) => {
            unqualify(&mut e, sheet);
            e.to_display(&wb.locale).trim_start_matches('=').to_string()
        }
        Err(_) => text.to_string(),
    }
}

/// A right side in Excel syntax, references qualified with the sheet.
fn file_rhs(wb: &Workbook, text: &str, sheet: &str) -> String {
    let t = text.trim().trim_start_matches('=');
    if let apex_sheet::Value::Number(n) = apex_sheet::format::parse_input(t, &wb.locale).value {
        return num_text(n);
    }
    match apex_sheet::formula::parse(&format!("={t}"), &wb.locale) {
        Ok(mut e) => {
            let own: std::sync::Arc<str> = sheet.into();
            e.visit_refs_mut(&mut |r| {
                if let Expr::Ref { sheet: s, .. } | Expr::Range { sheet: s, .. } = r {
                    if s.is_none() {
                        *s = Some(own.clone());
                    }
                }
            });
            e.to_xlsx()
        }
        Err(_) => t.to_string(),
    }
}

/// The model the names of a sheet describe (by lowercase name: original
/// name and text); None without an objective cell on the sheet.
fn model_of(wb: &Workbook, sheet: &str, map: &BTreeMap<String, (String, String)>) -> Option<SolverModel> {
    let get = |k: &str| map.get(k).map(|x| x.1.as_str());
    let objective = ranges(get("solver_opt")?, sheet).filter(|v| v.len() == 1 && v[0].start == v[0].end)?[0].start;
    let mut m = SolverModel { objective, ..Default::default() };
    let int = |k: &str| get(k).and_then(number).map(|n| n as i64);
    let mut used: Vec<String> = ["solver_opt", "solver_typ", "solver_val", "solver_neg", "solver_tim", "solver_itr", "solver_pre", "solver_msl", "solver_rsd", "solver_num", "solver_ver"].map(String::from).to_vec();
    let val = get("solver_val").and_then(number).unwrap_or(0.0);
    m.goal = match int("solver_typ") {
        Some(2) => SolverGoal::Min,
        Some(3) => SolverGoal::Value(val),
        _ => SolverGoal::Max,
    };
    // Variables that can't be read are kept as they were.
    if let Some(v) = get("solver_adj").and_then(|t| ranges(t, sheet)) {
        m.variables = v;
        used.push("solver_adj".into());
    }
    // Evolutionary (3) runs as nonlinear; it is kept.
    if int("solver_eng") == Some(2) {
        m.method = SolverMethod::SimplexLp;
    }
    if int("solver_eng") != Some(3) {
        used.push("solver_eng".into());
    }
    if let Some(n) = int("solver_neg") {
        m.non_negative = n == 1;
    }
    if let Some(n) = get("solver_tim").and_then(number) {
        m.max_time = if n <= 0.0 || n >= UNLIMITED { 0.0 } else { n };
    }
    if let Some(n) = get("solver_itr").and_then(number).filter(|n| *n >= 1.0) {
        m.max_iter = n.min(UNLIMITED) as usize;
    }
    if let Some(n) = get("solver_pre").and_then(number).filter(|n| *n > 0.0) {
        m.precision = n;
    }
    if let Some(n) = int("solver_msl") {
        m.multistart = n == 1;
    }
    if let Some(n) = int("solver_rsd") {
        m.seed = (n > 0).then_some(n as u64);
    }
    let count = get("solver_num").and_then(number).unwrap_or(0.0).clamp(0.0, 10_000.0) as usize;
    let mut odd = Vec::new();
    for i in 1..=count {
        let keys = [format!("solver_lhs{i}"), format!("solver_rel{i}"), format!("solver_rhs{i}")];
        let (lhs, rel, rhs) = (get(&keys[0]).unwrap_or(""), get(&keys[1]).unwrap_or(""), get(&keys[2]).unwrap_or(""));
        used.extend(keys.iter().cloned());
        let op = match number(rel).map(|n| n as i64) {
            Some(1) => Some(ConstraintOp::Le),
            Some(2) => Some(ConstraintOp::Eq),
            Some(3) => Some(ConstraintOp::Ge),
            Some(4) => Some(ConstraintOp::Int),
            Some(5) => Some(ConstraintOp::Bin),
            _ => None,
        };
        match (ranges(lhs, sheet).filter(|v| v.len() == 1), op) {
            (Some(v), Some(op)) => {
                let rhs = if matches!(op, ConstraintOp::Int | ConstraintOp::Bin) { String::new() } else { typed_rhs(wb, rhs, sheet) };
                m.constraints.push(Constraint { lhs: v[0], op, rhs });
            }
            // All different (6) and the like are kept.
            _ => odd.extend([("solver_lhs".to_string(), lhs.to_string()), ("solver_rel".into(), rel.to_string()), ("solver_rhs".into(), rhs.to_string())]),
        }
    }
    m.kept = map.iter().filter(|(k, _)| !used.contains(k)).map(|(_, (name, text))| (name.clone(), text.clone())).collect();
    m.kept.extend(odd);
    Some(m)
}

/// The names of the Solver models of the workbook: (sheet index, name,
/// formula text in Excel syntax).
pub(crate) fn write(wb: &Workbook) -> Vec<(usize, String, String)> {
    let mut out = Vec::new();
    for (sheet, m) in &wb.analysis.solver_models {
        let Some(si) = wb.sheet_index(sheet) else { continue };
        let q = crate::biff::quote_sheet(sheet);
        let abs = |r: &Range| {
            let a = |c: CellRef| format!("${}${}", apex_sheet::addr::col_name(c.col), c.row + 1);
            if r.start == r.end { format!("{q}!{}", a(r.start)) } else { format!("{q}!{}:{}", a(r.start), a(r.end)) }
        };
        let kept = |k: &str| m.kept.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|x| x.1.as_str());
        let mut v: Vec<(String, String)> = Vec::new();
        let mut put = |k: &str, t: String| v.push((k.to_string(), t));
        put("solver_opt", abs(&Range::single(m.objective)));
        let (typ, val) = match m.goal {
            SolverGoal::Max => (1, 0.0),
            SolverGoal::Min => (2, 0.0),
            SolverGoal::Value(x) => (3, x),
        };
        put("solver_typ", typ.to_string());
        put("solver_val", num_text(val));
        if !m.variables.is_empty() {
            put("solver_adj", m.variables.iter().map(&abs).collect::<Vec<_>>().join(","));
        }
        let eng = match m.method {
            SolverMethod::SimplexLp => "2",
            SolverMethod::Nonlinear if kept("solver_eng").is_some_and(|e| e.trim() == "3") => "3",
            SolverMethod::Nonlinear => "1",
        };
        put("solver_eng", eng.into());
        put("solver_neg", if m.non_negative { "1" } else { "2" }.into());
        put("solver_tim", num_text(if m.max_time > 0.0 { m.max_time } else { UNLIMITED }));
        put("solver_itr", num_text(m.max_iter as f64));
        put("solver_pre", num_text(m.precision));
        put("solver_msl", if m.multistart { "1" } else { "2" }.into());
        put("solver_rsd", m.seed.unwrap_or(0).to_string());
        let mut n = 0;
        for c in &m.constraints {
            n += 1;
            let (rel, rhs) = match c.op {
                ConstraintOp::Le => (1, file_rhs(wb, &c.rhs, sheet)),
                ConstraintOp::Eq => (2, file_rhs(wb, &c.rhs, sheet)),
                ConstraintOp::Ge => (3, file_rhs(wb, &c.rhs, sheet)),
                ConstraintOp::Int => (4, "integer".into()),
                ConstraintOp::Bin => (5, "binary".into()),
            };
            put(&format!("solver_lhs{n}"), abs(&c.lhs));
            put(&format!("solver_rel{n}"), rel.to_string());
            put(&format!("solver_rhs{n}"), rhs);
        }
        for (k, t) in &m.kept {
            match k.as_str() {
                "solver_lhs" => {
                    n += 1;
                    put(&format!("solver_lhs{n}"), t.clone());
                }
                "solver_rel" | "solver_rhs" => put(&format!("{k}{n}"), t.clone()),
                _ => {}
            }
        }
        put("solver_num", n.to_string());
        put("solver_ver", "3".into());
        for (k, t) in &m.kept {
            if !matches!(k.as_str(), "solver_lhs" | "solver_rel" | "solver_rhs") && !v.iter().any(|(x, _)| x.eq_ignore_ascii_case(k)) {
                v.push((k.clone(), t.clone()));
            }
        }
        out.extend(v.into_iter().map(|(k, t)| (si, k, t)));
    }
    out
}

/// The `<definedName>` elements of the Solver models.
pub(crate) fn xlsx_names(wb: &Workbook) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for (si, name, text) in write(wb) {
        let _ = write!(s, r#"<definedName name="{}" localSheetId="{si}" hidden="1">{}</definedName>"#, crate::xml::esc(&name), crate::xml::esc(&text));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str, text: &str) -> RawName {
        (name.into(), Some(0), text.into(), String::new(), true)
    }

    #[test]
    fn read_and_write() {
        let mut wb = Workbook::new();
        wb.sheets[0].name = "My Sheet".into();
        let names = vec![
            raw("solver_opt", "'My Sheet'!$D$1"),
            raw("solver_typ", "3"),
            raw("solver_val", "12.5"),
            raw("solver_adj", "'My Sheet'!$B$2:$C$2,'My Sheet'!$E$5"),
            raw("solver_num", "3"),
            raw("solver_lhs1", "'My Sheet'!$D$3:$D$4"),
            raw("solver_rel1", "1"),
            raw("solver_rhs1", "'My Sheet'!$E$3:$E$4"),
            raw("solver_lhs2", "'My Sheet'!$B$2"),
            raw("solver_rel2", "4"),
            raw("solver_rhs2", "integer"),
            raw("solver_lhs3", "'My Sheet'!$B$2:$C$2"),
            raw("solver_rel3", "6"),
            raw("solver_rhs3", "AllDifferent"),
            raw("solver_eng", "3"),
            raw("solver_neg", "2"),
            raw("solver_tim", "2147483647"),
            raw("solver_itr", "500"),
            raw("solver_pre", "0.0001"),
            raw("solver_msl", "1"),
            raw("solver_rsd", "7"),
            raw("solver_cvg", "0.0001"),
            raw("solver_ver", "3"),
            raw("Other", "1"),
            ("solver_opt".into(), None, "junk".into(), String::new(), true),
            ("solver_adj".into(), None, "Best!$A$1".into(), String::new(), false),
            ("solver_adj".into(), None, "Best!$A$1".into(), String::new(), true),
        ];
        let rest = read(&mut wb, names);
        assert_eq!(rest.iter().map(|n| n.0.as_str()).collect::<Vec<_>>(), vec!["Other", "solver_adj"]);
        let m = wb.analysis.solver_models.get("My Sheet").unwrap().clone();
        assert_eq!(m.objective, CellRef::parse("D1").unwrap());
        assert_eq!(m.goal, SolverGoal::Value(12.5));
        assert_eq!(m.variables, vec![Range::parse("B2:C2").unwrap(), Range::parse("E5").unwrap()]);
        assert_eq!(m.constraints.len(), 2);
        assert_eq!((m.constraints[0].op, m.constraints[0].rhs.as_str()), (ConstraintOp::Le, "$E$3:$E$4"));
        assert_eq!(m.constraints[1].op, ConstraintOp::Int);
        assert_eq!((m.method, m.non_negative, m.max_time, m.max_iter, m.precision, m.multistart, m.seed), (SolverMethod::Nonlinear, false, 0.0, 500, 0.0001, true, Some(7)));
        assert!(m.kept.contains(&("solver_cvg".into(), "0.0001".into())) && m.kept.contains(&("solver_eng".into(), "3".into())));

        let out = write(&wb);
        let get = |k: &str| out.iter().find(|x| x.1 == k).map(|x| x.2.as_str());
        assert_eq!(get("solver_adj"), Some("'My Sheet'!$B$2:$C$2,'My Sheet'!$E$5"));
        assert_eq!(get("solver_rhs1"), Some("'My Sheet'!$E$3:$E$4"));
        assert_eq!((get("solver_rel3"), get("solver_rhs3"), get("solver_num")), (Some("6"), Some("AllDifferent"), Some("3")));
        assert_eq!((get("solver_eng"), get("solver_tim"), get("solver_cvg")), (Some("3"), Some("2147483647"), Some("0.0001")));
        assert_eq!(out.iter().filter(|x| x.1 == "solver_eng").count(), 1);
        // Read back: the same model.
        let mut back = Workbook::new();
        back.sheets[0].name = "My Sheet".into();
        read(&mut back, out.into_iter().map(|(si, k, t)| (k, Some(si), t, String::new(), true)).collect());
        assert_eq!(back.analysis.solver_models.get("My Sheet"), Some(&m));
    }

    #[test]
    fn xlsx_roundtrip() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "1");
        wb.set_input(0, CellRef::new(0, 1), "=A1*3");
        let half = apex_sheet::format::edit_number(0.5, wb.locale.decimal);
        let m = SolverModel {
            objective: CellRef::new(0, 1),
            goal: SolverGoal::Max,
            variables: vec![Range::parse("A1").unwrap()],
            constraints: vec![Constraint { lhs: Range::parse("A1").unwrap(), op: ConstraintOp::Le, rhs: format!("$C$1*{half}") }],
            max_time: 0.0,
            ..Default::default()
        };
        wb.save_solver_model(0, m.clone());
        let data = crate::xlsx::write(&wb);
        let book = crate::zip::ZipReader::new(&data).unwrap().read_string("xl/workbook.xml").unwrap();
        assert!(book.contains(r#"<definedName name="solver_opt" localSheetId="0" hidden="1">Sheet1!$B$1</definedName>"#), "{book}");
        assert!(book.contains(r#"<definedName name="solver_rhs1" localSheetId="0" hidden="1">Sheet1!$C$1*0.5</definedName>"#), "{book}");
        assert!(book.contains(r#"<definedName name="solver_tim" localSheetId="0" hidden="1">2147483647</definedName>"#));
        let back = crate::xlsx::read(&data).unwrap();
        assert_eq!(back.solver_model(0), Some(&m));
        // Not defined names of the workbook (the name manager lists those).
        assert!(back.names.is_empty(), "{:?}", back.names);
    }
}
