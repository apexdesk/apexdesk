//! The workbook's tests (Formulas > Tests) in a part of their own
//! (`urn:apexdesk:checks`): a custom XML part of xlsx, which Excel keeps
//! untouched, and `ApexDesk/checks.xml` in ods (listed in the manifest;
//! LibreOffice ignores it).

use std::fmt::Write as _;

use apex_sheet::Check;

use crate::xml::{Ev, HEADER, Reader, esc};

pub const NS: &str = "urn:apexdesk:checks";

/// Path of the part in an ods package.
pub(crate) const ODS_PATH: &str = "ApexDesk/checks.xml";

/// Path of the part in an xlsx package (kept custom XML parts are
/// numbered from 2, and the queries are item1).
pub(crate) const XLSX_PATH: &str = "customXml/itemChecks.xml";

pub fn write(checks: &[Check]) -> String {
    let mut out = format!(r#"{HEADER}<checks xmlns="{NS}" version="1">"#);
    for c in checks {
        let _ = write!(out, r#"<check name="{}" sheet="{}" formula="{}"/>"#, esc(&c.name), esc(&c.sheet), esc(&c.formula));
    }
    out.push_str("</checks>");
    out
}

/// Whether an XML part is ours.
pub(crate) fn is_ours(xml: &str) -> bool {
    xml.contains(NS) && xml.contains("<checks")
}

pub fn read(xml: &str) -> Vec<Check> {
    let mut out = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "check" => {
                let a = |k: &str| t.attr(k).map(|v| v.into_owned()).unwrap_or_default();
                out.push(Check { name: a("name"), sheet: a("sheet"), formula: a("formula") });
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let list = vec![Check { name: "Totals & <more>".into(), sheet: "Hoja \"1\"".into(), formula: "SUM(A1:A3)=A4".into() }, Check::default()];
        let x = write(&list);
        assert!(is_ours(&x));
        assert_eq!(read(&x), list);
    }
}
