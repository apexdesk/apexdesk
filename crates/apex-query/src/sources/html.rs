//! Tables of HTML pages. Real pages are rarely valid XML, so this is a
//! forgiving scanner: `<table>`, `<tr>`, `<td>`/`<th>` (with `colspan`),
//! the rest of the markup dropped and the text tidied.

use crate::table::DataTable;

#[derive(Default)]
struct Building {
    rows: Vec<Vec<String>>,
    row: Option<Vec<String>>,
    cell: Option<String>,
    span: usize,
    /// Rows that are all `<th>` or inside `<thead>`, by index.
    head_rows: Vec<usize>,
    row_all_th: bool,
    in_head: bool,
}

impl Building {
    fn end_cell(&mut self) {
        if let Some(c) = self.cell.take() {
            let text = c.split_whitespace().collect::<Vec<_>>().join(" ");
            let row = self.row.get_or_insert_with(Vec::new);
            row.push(text);
            for _ in 1..self.span {
                row.push(String::new());
            }
        }
    }

    fn end_row(&mut self) {
        self.end_cell();
        if let Some(r) = self.row.take() {
            if !r.is_empty() {
                if self.row_all_th || self.in_head {
                    self.head_rows.push(self.rows.len());
                }
                self.rows.push(r);
            }
        }
    }

    fn finish(mut self) -> Option<DataTable> {
        self.end_row();
        if self.rows.is_empty() {
            return None;
        }
        let header = self.head_rows.first() == Some(&0);
        Some(DataTable::from_text_rows(self.rows, header))
    }
}

fn entity_text(s: &str) -> String {
    let s = s.replace("&nbsp;", " ").replace("&#160;", " ");
    apex_io::xml::unescape(&s).into_owned()
}

/// The value of attribute `name` in a tag's source.
fn attr(tag: &str, name: &str) -> Option<String> {
    let low = tag.to_ascii_lowercase();
    let i = low.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[i..];
    let v = match rest.chars().next()? {
        q @ ('"' | '\'') => rest[1..].split(q).next()?,
        _ => rest.split(|c: char| c.is_whitespace() || c == '>').next()?,
    };
    Some(v.to_string())
}

/// Every table of the page, in the order they start.
pub fn tables(html: &str) -> Vec<DataTable> {
    let mut done: Vec<(usize, DataTable)> = Vec::new();
    let mut stack: Vec<(usize, Building)> = Vec::new();
    let mut count = 0;
    let b = html.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            let end = html[i..].find('<').map_or(html.len(), |k| i + k);
            if let Some((_, t)) = stack.last_mut()
                && let Some(c) = t.cell.as_mut()
            {
                c.push_str(&entity_text(&html[i..end]));
            }
            i = end;
            continue;
        }
        if html[i..].starts_with("<!--") {
            i = html[i..].find("-->").map_or(html.len(), |k| i + k + 3);
            continue;
        }
        let end = html[i..].find('>').map_or(html.len(), |k| i + k + 1);
        let tag = &html[i + 1..end.saturating_sub(1).max(i + 1)];
        i = end;
        let closing = tag.starts_with('/');
        let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        match (name.as_str(), closing) {
            ("script" | "style", false) => {
                let close = format!("</{name}");
                i = find_ci(&html[i..], &close).map_or(html.len(), |k| i + k);
            }
            ("table", false) => {
                stack.push((count, Building::default()));
                count += 1;
            }
            ("table", true) => {
                if let Some((n, t)) = stack.pop()
                    && let Some(t) = t.finish()
                {
                    done.push((n, t));
                }
            }
            (_, _) if stack.is_empty() => {}
            ("tr", false) => {
                let t = &mut stack.last_mut().unwrap().1;
                t.end_row();
                t.row = Some(Vec::new());
                t.row_all_th = true;
            }
            ("tr", true) => stack.last_mut().unwrap().1.end_row(),
            ("thead", closing) => {
                let t = &mut stack.last_mut().unwrap().1;
                t.end_row();
                t.in_head = !closing;
            }
            ("tbody" | "tfoot", _) => {
                let t = &mut stack.last_mut().unwrap().1;
                t.end_row();
                t.in_head = false;
            }
            ("td" | "th", false) => {
                let t = &mut stack.last_mut().unwrap().1;
                t.end_cell();
                if t.row.is_none() {
                    t.row = Some(Vec::new());
                    t.row_all_th = true;
                }
                if name == "td" {
                    t.row_all_th = false;
                }
                t.span = attr(tag, "colspan").and_then(|v| v.trim().parse().ok()).unwrap_or(1).clamp(1, 100);
                t.cell = Some(String::new());
            }
            ("td" | "th", true) => stack.last_mut().unwrap().1.end_cell(),
            ("br" | "p" | "div" | "li", _) => {
                if let Some(c) = stack.last_mut().unwrap().1.cell.as_mut() {
                    c.push(' ');
                }
            }
            _ => {}
        }
    }
    // Tables never closed.
    while let Some((n, t)) = stack.pop() {
        if let Some(t) = t.finish() {
            done.push((n, t));
        }
    }
    done.sort_by_key(|(n, _)| *n);
    done.into_iter().map(|(_, t)| t).collect()
}

/// Where `close` (lowercase ASCII) next is in `s`, in any case.
fn find_ci(s: &str, close: &str) -> Option<usize> {
    let (s, n) = (s.as_bytes(), close.as_bytes());
    s.windows(n.len()).position(|w| w.eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Val;

    #[test]
    fn page_tables() {
        let page = r#"<html><head><style>td{}</style><script>var s = "<table>";</script></head><body>
            <p>Intro</p>
            <table class=x><thead><tr><th>Country<th>Capital</th><th>Pop.</th></tr></thead>
            <tbody><tr><td>Spain</td><td><a href="/m">Madrid</a></td><td>48.6</td></tr>
            <tr><td>France&nbsp;(FR)</td><td colspan=2>Paris</td></tr>
            <tr><td><table><tr><td>inner</td></tr></table>Italy</td><td>Rome</td><td>59</td></tr></tbody></table>
            <table><tr><td>1</td><td>2</td></tr></table>
        "#;
        let ts = tables(page);
        assert_eq!(ts.len(), 3);
        let t = &ts[0];
        assert_eq!(t.columns, ["Country", "Capital", "Pop."]);
        assert_eq!(t.rows[0], vec![Val::text("Spain"), Val::text("Madrid"), Val::Num(48.6)]);
        assert_eq!(t.rows[1], vec![Val::text("France (FR)"), Val::text("Paris"), Val::Null]);
        assert_eq!(t.rows[2][0], Val::text("Italy"));
        assert_eq!(ts[1].rows, vec![vec![Val::text("inner")]]);
        assert_eq!(ts[2].columns, ["Column1", "Column2"]);
    }
}
