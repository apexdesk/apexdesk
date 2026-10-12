//! Scenario values and Solver right sides between the text typed in the
//! workbook's locale (what the model keeps) and the files' locale-neutral
//! text: numbers with `.`, formulas in Excel syntax.

use apex_sheet::formula::parse_canonical;
use apex_sheet::{Content, Value, Workbook};

/// A number as files write it.
pub(crate) fn num_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

/// A typed input as a file value: numbers in canonical form, formulas in
/// Excel syntax with `=`, text as is.
pub(crate) fn file_value(wb: &Workbook, text: &str) -> String {
    match wb.content_from_input(text).0 {
        Content::Value(Value::Number(n)) => num_text(n),
        Content::Value(Value::Bool(b)) => if b { "TRUE" } else { "FALSE" }.into(),
        Content::Value(Value::Text(t)) => t.to_string(),
        Content::Formula { expr, .. } => format!("={}", expr.expr().to_xlsx()),
        _ => text.to_string(),
    }
}

/// A file value as typed input (see [`file_value`]).
pub(crate) fn typed_value(wb: &Workbook, v: &str) -> String {
    if let Some(f) = v.strip_prefix('=').filter(|f| !f.is_empty()) {
        return match parse_canonical(f) {
            Ok(e) => e.to_display(&wb.locale),
            Err(_) => format!("'{v}"),
        };
    }
    if let Some(n) = v.trim().parse::<f64>().ok().filter(|n| n.is_finite() && !v.trim().is_empty()) {
        return apex_sheet::format::edit_number(n, wb.locale.decimal);
    }
    // Text that would read as something else keeps its apostrophe.
    match apex_sheet::format::parse_input(v, &wb.locale).value {
        Value::Text(t) if &*t == v => v.to_string(),
        Value::Empty => String::new(),
        _ => format!("'{v}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        let mut wb = Workbook::new();
        wb.locale = apex_sheet::Locale::ES;
        assert_eq!(file_value(&wb, "1,5"), "1.5");
        assert_eq!(file_value(&wb, "=SUM(A1;2,5)"), "=SUM(A1,2.5)");
        assert_eq!(file_value(&wb, "hola"), "hola");
        assert_eq!(typed_value(&wb, "1.5"), "1,5");
        assert_eq!(typed_value(&wb, "100"), "100");
        assert_eq!(typed_value(&wb, "=SUM(A1,2.5)"), "=SUM(A1;2,5)");
        assert_eq!(typed_value(&wb, "hola"), "hola");
        assert_eq!(typed_value(&wb, "1/2/2024"), "'1/2/2024");
        assert_eq!(typed_value(&wb, ""), "");
    }
}
