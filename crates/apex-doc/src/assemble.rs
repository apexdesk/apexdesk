//! Document assembly, done with each record of a mail merge (or the
//! answers of a form): content controls named like a field (by the node
//! of their custom XML binding, else their tag or title) take its value,
//! and conditional sections stay or go. A section is a control tagged
//! `if:condition`, or text between `«IF condition»` and `«END IF»` (which
//! may span paragraphs and nest). A condition is `Name` (not empty, 0,
//! false or no), `not Name`, `Name = value` or `Name != value`; names and
//! values are compared ignoring case.

use crate::merge::{FIELD_CLOSE, FIELD_OPEN};
use crate::{CHECKED, ContentControl, ControlKind, Document, Pos, Selection, UNCHECKED};

/// The field a control shows: the last step of its binding's XPath, else
/// its tag, else its title. Conditional controls have none.
pub fn control_name(c: &ContentControl) -> Option<String> {
    if c.tag.trim_start().to_ascii_lowercase().starts_with("if:") {
        return None;
    }
    let from_xpath = c.binding.as_ref().and_then(|b| {
        let last = b.xpath.trim_end_matches('/').rsplit('/').next()?;
        let last = last.split('[').next()?.trim_start_matches('@');
        let local = last.rsplit(':').next()?;
        Some(local.to_string()).filter(|s| !s.is_empty())
    });
    from_xpath.or_else(|| Some(c.tag.trim().to_string()).filter(|s| !s.is_empty())).or_else(|| Some(c.title.trim().to_string()).filter(|s| !s.is_empty()))
}

/// A condition written in a section: its field names.
fn condition_names(cond: &str) -> Vec<String> {
    let c = cond.trim();
    let c = c.strip_prefix("not ").or_else(|| c.strip_prefix("NOT ")).unwrap_or(c);
    let name = c.split("!=").next().unwrap_or(c).split('=').next().unwrap_or(c).trim();
    if name.is_empty() { Vec::new() } else { vec![name.to_string()] }
}

/// A merge-field text that opens (`Some(condition)`) or closes (`None`) a
/// section; other fields aren't sections.
pub fn section_marker(name: &str) -> Option<Option<&str>> {
    let n = name.trim();
    if n.eq_ignore_ascii_case("END IF") || n.eq_ignore_ascii_case("ENDIF") {
        return Some(None);
    }
    let head = n.get(..3)?;
    head.eq_ignore_ascii_case("IF ").then(|| Some(n[3..].trim()))
}

/// Every name a form for the document asks for: merge fields, named
/// controls and the fields of conditions, in order of first use.
pub fn form_fields(doc: &Document) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |n: String| {
        if !n.is_empty() && seen.insert(n.to_ascii_lowercase()) {
            out.push(n);
        }
    };
    for p in &doc.paragraphs {
        let mut rest = p.text.as_str();
        while let Some(a) = rest.find(FIELD_OPEN) {
            let after = &rest[a + FIELD_OPEN.len_utf8()..];
            let Some(b) = after.find(FIELD_CLOSE) else { break };
            match section_marker(&after[..b]) {
                Some(Some(cond)) => condition_names(cond).into_iter().for_each(&mut add),
                Some(None) => {}
                None => add(after[..b].to_string()),
            }
            rest = &after[b + FIELD_CLOSE.len_utf8()..];
        }
    }
    for at in doc.controls() {
        match control_name(&at.control) {
            Some(n) => add(n),
            None => {
                let tag = at.control.tag.trim_start();
                if tag.get(..3).is_some_and(|h| h.eq_ignore_ascii_case("if:")) {
                    condition_names(&tag[3..]).into_iter().for_each(&mut add);
                }
            }
        }
    }
    out
}

fn truthy(v: &str) -> bool {
    let v = v.trim().to_ascii_lowercase();
    !(v.is_empty() || matches!(v.as_str(), "0" | "false" | "no" | "falso" | "nein" | "non" | "n"))
}

/// Whether `cond` holds for the record (`get` gives a field's value).
pub fn holds(cond: &str, get: &dyn Fn(&str) -> Option<String>) -> bool {
    // "not" taken off in a loop (a long chain mustn't recurse).
    let mut c = cond.trim();
    let mut negated = false;
    while let Some(rest) = c.strip_prefix("not ").or_else(|| c.strip_prefix("NOT ")) {
        c = rest.trim();
        negated = !negated;
    }
    negated != holds_plain(c, get)
}

fn holds_plain(c: &str, get: &dyn Fn(&str) -> Option<String>) -> bool {
    let value = |name: &str| get(name.trim()).unwrap_or_default();
    let unquote = |v: &str| v.trim().trim_matches('"').trim().to_lowercase();
    if let Some((n, v)) = c.split_once("!=") {
        return value(n).trim().to_lowercase() != unquote(v);
    }
    if let Some((n, v)) = c.split_once('=') {
        return value(n).trim().to_lowercase() == unquote(v);
    }
    truthy(&value(c))
}

/// Applies the record (`names` and their values) to `d`: named controls,
/// control sections and marker sections. Edits go to the undo history;
/// the caller clears it.
pub(crate) fn apply(d: &mut Document, names: &[String], record: &[String]) {
    let get = |n: &str| names.iter().position(|x| x.eq_ignore_ascii_case(n.trim())).map(|i| record.get(i).cloned().unwrap_or_default());
    marker_sections(d, &get);
    control_sections(d, &get);
    fill_controls(d, &get);
}

/// Deletes `a..b`, and the paragraph left empty if the range was all of
/// its text (so a section on paragraphs of its own leaves no blank line).
fn delete_block(d: &mut Document, a: Pos, b: Pos) {
    let whole = a.offset == 0 && b.offset == d.paragraphs[b.para].len();
    let (a, b) = if whole && b.para + 1 < d.paragraphs.len() {
        (a, Pos::new(b.para + 1, 0))
    } else if whole && a.para > 0 {
        (Pos::new(a.para - 1, d.paragraphs[a.para - 1].len()), b)
    } else {
        (a, b)
    };
    d.delete(Selection { anchor: a, cursor: b });
}

/// The markers of the document in order: (start, end, condition or None).
fn markers(d: &Document) -> Vec<(Pos, Pos, Option<String>)> {
    let mut out = Vec::new();
    for (i, p) in d.paragraphs.iter().enumerate() {
        let mut from = 0;
        while let Some(a) = p.text[from..].find(FIELD_OPEN).map(|x| x + from) {
            let inner = a + FIELD_OPEN.len_utf8();
            let Some(b) = p.text[inner..].find(FIELD_CLOSE).map(|x| x + inner) else { break };
            let end = b + FIELD_CLOSE.len_utf8();
            if let Some(m) = section_marker(&p.text[inner..b]) {
                out.push((Pos::new(i, a), Pos::new(i, end), m.map(str::to_string)));
            }
            from = end;
        }
    }
    out
}

fn marker_sections(d: &mut Document, get: &dyn Fn(&str) -> Option<String>) {
    // One innermost section at a time: positions change after each.
    // Each one reads the whole document: no more than a template has.
    for _ in 0..2_000 {
        let m = markers(d);
        let Some(k) = (0..m.len().saturating_sub(1)).find(|&k| m[k].2.is_some() && m[k + 1].2.is_none()) else {
            // Unpaired markers stay as written.
            return;
        };
        let (open, close) = (&m[k], &m[k + 1]);
        if holds(open.2.as_deref().unwrap_or(""), get) {
            // Keep the text: take the markers away, the closing one first.
            delete_block(d, close.0, close.1);
            delete_block(d, open.0, open.1);
        } else {
            delete_block(d, open.0, close.1);
        }
    }
}

fn control_sections(d: &mut Document, get: &dyn Fn(&str) -> Option<String>) {
    let failing: Vec<_> = d
        .controls()
        .into_iter()
        .filter(|at| {
            let tag = at.control.tag.trim_start();
            tag.get(..3).is_some_and(|h| h.eq_ignore_ascii_case("if:")) && !holds(&tag[3..], get)
        })
        .collect();
    for at in failing.iter().rev() {
        delete_block(d, Pos::new(at.para, at.range.start), Pos::new(at.para, at.range.end));
    }
}

fn fill_controls(d: &mut Document, get: &dyn Fn(&str) -> Option<String>) {
    let all = d.controls();
    for at in all.iter().rev() {
        let Some(value) = control_name(&at.control).and_then(|n| get(&n)) else { continue };
        let mut c: ContentControl = (*at.control).clone();
        let text = match &mut c.kind {
            ControlKind::CheckBox { checked } => {
                *checked = truthy(&value);
                (if *checked { CHECKED } else { UNCHECKED }).to_string()
            }
            // Empty: the control keeps its placeholder.
            _ if value.trim().is_empty() => continue,
            ControlKind::Date { date, .. } => {
                let iso = value.trim();
                if let Some(day) = iso.get(..10).filter(|d| d.as_bytes()[4] == b'-' && d.as_bytes()[7] == b'-') {
                    *date = Some(day.to_string());
                }
                value.clone()
            }
            _ => value.replace('\n', " "),
        };
        c.placeholder = false;
        d.set_control(at, c, &text, Selection::caret(Pos::new(at.para, at.range.start)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DataBinding, Paragraph, merge};

    fn doc(texts: &[&str]) -> Document {
        Document::from_paragraphs(texts.iter().map(|t| Paragraph::new(*t)).collect())
    }

    fn texts(d: &Document) -> Vec<String> {
        d.paragraphs.iter().map(|p| p.text.clone()).collect()
    }

    #[test]
    fn conditions() {
        let get = |n: &str| match n.to_ascii_lowercase().as_str() {
            "vip" => Some("Yes".to_string()),
            "country" => Some("Spain".to_string()),
            "debt" => Some("0".to_string()),
            _ => None,
        };
        assert!(holds("VIP", &get));
        assert!(!holds("Debt", &get));
        assert!(holds("not Debt", &get));
        assert!(holds("Country = \"spain\"", &get));
        assert!(holds("Country != France", &get));
        assert!(!holds("Missing", &get));
        assert_eq!(section_marker("IF VIP = yes"), Some(Some("VIP = yes")));
        assert_eq!(section_marker("end if"), Some(None));
        assert_eq!(section_marker("Iffy"), None);
    }

    #[test]
    fn marker_sections_nest_and_span_paragraphs() {
        let d = doc(&["Dear «Name»,", "«IF VIP»", "Thanks for being a VIP«IF Country = Spain» in Spain«END IF».", "«END IF»", "Pay «Debt».«IF not Debt» Nothing due.«END IF»", "Bye"]);
        assert_eq!(form_fields(&d), ["Name", "VIP", "Country", "Debt"]);
        assert_eq!(merge::fields_used(&d), ["Name", "Debt"]);
        let names: Vec<String> = ["Name", "VIP", "Country", "Debt"].iter().map(|s| s.to_string()).collect();
        let yes = merge::fill(&d, &names, &["Ana".into(), "yes".into(), "Spain".into(), "0".into()]);
        assert_eq!(texts(&yes), ["Dear Ana,", "Thanks for being a VIP in Spain.", "Pay 0. Nothing due.", "Bye"]);
        let no = merge::fill(&d, &names, &["Bob".into(), "".into(), "France".into(), "12".into()]);
        assert_eq!(texts(&no), ["Dear Bob,", "Pay 12.", "Bye"]);
        assert!(!no.can_undo());
    }

    #[test]
    fn controls_fill_and_go() {
        let mut d = doc(&["Client: ", "Approved: ", "Premium terms apply.", "End"]);
        let mut name = ContentControl::new(1, ControlKind::PlainText);
        name.binding = Some(DataBinding { xpath: "/ns0:data[1]/ns0:Client[1]".into(), store_item: "{X}".into(), prefixes: String::new() });
        d.insert_control(Selection::caret(Pos::new(0, 8)), ContentControl { placeholder: true, ..name }, "Type here");
        let mut ok = ContentControl::new(2, ControlKind::CheckBox { checked: false });
        ok.title = "Approved".into();
        d.insert_control(Selection::caret(Pos::new(1, 10)), ok, "☐");
        let mut sec = ContentControl::new(3, ControlKind::RichText);
        sec.tag = "if:Premium".into();
        let whole = Selection { anchor: Pos::new(2, 0), cursor: Pos::new(2, d.paragraphs[2].len()) };
        d.insert_control(whole, sec, "");
        assert_eq!(form_fields(&d), ["Client", "Approved", "Premium"]);
        let names: Vec<String> = ["client", "approved", "premium"].iter().map(|s| s.to_string()).collect();
        let a = merge::fill(&d, &names, &["ACME".into(), "1".into(), "".into()]);
        assert_eq!(texts(&a), ["Client: ACME", "Approved: ☒", "End"]);
        let c = a.controls();
        assert!(!c[0].control.placeholder && c[0].control.binding.is_some());
        assert_eq!(c[1].control.kind, ControlKind::CheckBox { checked: true });
        let b = merge::fill(&d, &names, &["".into(), "no".into(), "yes".into()]);
        assert_eq!(texts(&b), ["Client: Type here", "Approved: ☐", "Premium terms apply.", "End"]);
        assert!(b.controls()[0].control.placeholder);
    }

    #[test]
    fn a_control_without_name_has_no_field() {
        let mut d = doc(&["Ω ñandú"]);
        d.insert_control(Selection::caret(Pos::new(0, 0)), ContentControl::new(1, ControlKind::CheckBox { checked: false }), "☐");
        let mut cond = ContentControl::new(2, ControlKind::RichText);
        cond.tag = " IF:Vip".into();
        let end = d.paragraphs[0].text.len();
        d.insert_control(Selection::caret(Pos::new(0, end)), cond, "sí");
        assert_eq!(form_fields(&d), ["Vip"]);
    }

}
