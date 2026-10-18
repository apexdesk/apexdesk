//! Pagination against Word: a .docx saved by Word stores its page count
//! (docProps/app.xml `Pages`) and marks where each page began when Word
//! last laid it out (`w:lastRenderedPageBreak`). The marks can be stale,
//! so only documents whose marks add up to `Pages` are compared. Our page
//! starts come from `apexdesk info --page-starts` (the printed layout);
//! paragraphs are matched between Word's XML and our model by their text.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::{Value as Json, json};

use crate::oracle_zip::{Ev, Xml, Zip, attr, element_text, local, unescape};

/// What Word's XML says about the body.
#[derive(Debug, Default, PartialEq)]
pub struct WordBody {
    /// Text of each body paragraph (w:p, tables included, text boxes not).
    pub paras: Vec<String>,
    /// Where each page after the first starts: paragraph, and whether the
    /// paragraph already had text before the mark (a page that goes on
    /// with a paragraph).
    pub breaks: Vec<(usize, bool)>,
    pub hard_breaks: usize,
}

pub fn word_body(xml: &str) -> WordBody {
    let mut out = WordBody::default();
    // Depth inside content that isn't body flow (text boxes, fallbacks).
    let mut skip = 0usize;
    let mut in_t = false;
    let mut cur: Option<usize> = None;
    let mut has_text = false;
    for ev in Xml::new(xml) {
        match ev {
            Ev::Start(n, _) if skip > 0 => {
                if matches!(local(n), "txbxContent" | "Fallback") {
                    skip += 1;
                }
            }
            Ev::End(n) if skip > 0 => {
                if matches!(local(n), "txbxContent" | "Fallback") {
                    skip -= 1;
                }
            }
            _ if skip > 0 => {}
            Ev::Start(n, _) if matches!(local(n), "txbxContent" | "Fallback") => skip = 1,
            Ev::Start(n, _) if n == "w:p" => {
                out.paras.push(String::new());
                cur = Some(out.paras.len() - 1);
                has_text = false;
            }
            Ev::Empty(n, _) if n == "w:p" => {
                out.paras.push(String::new());
                cur = None;
            }
            Ev::End(n) if n == "w:p" => cur = None,
            Ev::Start(n, _) if n == "w:t" => in_t = true,
            Ev::End(n) if n == "w:t" => in_t = false,
            Ev::Text(t) if in_t => {
                if let Some(p) = cur {
                    let t = unescape(t);
                    has_text |= !t.trim().is_empty();
                    out.paras[p].push_str(&t);
                }
            }
            Ev::Empty(n, _) if n == "w:lastRenderedPageBreak" => {
                if let Some(p) = cur {
                    out.breaks.push((p, has_text));
                }
            }
            Ev::Empty(n, a) if n == "w:br" && attr(a, "w:type").as_deref() == Some("page") => out.hard_breaks += 1,
            _ => {}
        }
    }
    out
}

/// Letters and digits, lower case: what both sides agree on.
fn key(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Pairs (word, ours) of paragraphs with the same non-empty text, in
/// order: a greedy walk that looks a little ahead on both sides.
pub fn align(word: &[String], ours: &[String]) -> Vec<(usize, usize)> {
    const AHEAD: usize = 60;
    let wk: Vec<String> = word.iter().map(|s| key(s)).collect();
    let ok: Vec<String> = ours.iter().map(|s| key(s)).collect();
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < wk.len() && j < ok.len() {
        if wk[i].is_empty() {
            i += 1;
            continue;
        }
        if ok[j].is_empty() {
            j += 1;
            continue;
        }
        if wk[i] == ok[j] {
            out.push((i, j));
            i += 1;
            j += 1;
            continue;
        }
        let in_ours = (j + 1..ok.len().min(j + AHEAD)).find(|&k| ok[k] == wk[i]);
        let in_word = (i + 1..wk.len().min(i + AHEAD)).find(|&k| wk[k] == ok[j]);
        match (in_ours, in_word) {
            (Some(a), Some(b)) if a - j <= b - i => j = a,
            (Some(a), None) => j = a,
            (_, Some(b)) => i = b,
            (None, None) => {
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// Word's paragraph for one of ours: the last aligned pair at or before
/// it, moved by the distance from it.
pub fn to_word(pairs: &[(usize, usize)], ours: usize) -> usize {
    let k = pairs.partition_point(|&(_, o)| o <= ours);
    match k.checked_sub(1).map(|k| pairs[k]) {
        Some((w, o)) => w + (ours - o),
        None => ours,
    }
}

/// Checks one document; `apexdesk` gives our page starts.
pub fn check(path: &Path, apexdesk: &Path) -> Json {
    let file = path.display().to_string();
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => return json!({"file": file, "status": "error", "reason": e.to_string()}),
    };
    let Some(zip) = Zip::new(&data) else { return json!({"file": file, "status": "skipped", "reason": "not a zip package"}) };
    let appxml = zip.read_string("docProps/app.xml").unwrap_or_default();
    let app = element_text(&appxml, "Application").unwrap_or_default();
    if !(app.contains("Microsoft") && app.contains("Word")) {
        return json!({"file": file, "status": "skipped", "reason": format!("saved by '{app}'")});
    }
    let Some(pages) = element_text(&appxml, "Pages").and_then(|p| p.parse::<usize>().ok()) else {
        return json!({"file": file, "status": "skipped", "reason": "no Pages in app.xml", "app": app});
    };
    // The main part is usually word/document.xml; follow the package
    // relationship otherwise.
    let main = zip
        .read_string("_rels/.rels")
        .and_then(|r| {
            Xml::new(&r).find_map(|ev| match ev {
                Ev::Empty(n, a) | Ev::Start(n, a) if local(n) == "Relationship" && attr(a, "Type").is_some_and(|t| t.ends_with("/officeDocument")) => attr(a, "Target").map(|t| t.trim_start_matches('/').to_string()),
                _ => None,
            })
        })
        .unwrap_or_else(|| "word/document.xml".into());
    let Some(xml) = zip.read_string(&main) else { return json!({"file": file, "status": "error", "reason": "no main part", "app": app}) };
    let body = word_body(&xml);
    if body.breaks.len() + 1 != pages {
        let reason = if body.breaks.is_empty() { "no lastRenderedPageBreak marks".to_string() } else { "marks don't match Pages (stale)".to_string() };
        return json!({"file": file, "status": "skipped", "reason": reason, "app": app, "word_pages": pages, "marks": body.breaks.len()});
    }
    // Our paragraphs (same loader as apexdesk) and page starts.
    let doc = match apex_io::open(path) {
        Ok((apex_io::Loaded::Doc(doc), _)) => doc,
        Ok(_) => return json!({"file": file, "status": "error", "reason": "opened as a spreadsheet", "app": app}),
        Err(e) => return json!({"file": file, "status": "error", "reason": format!("open: {e}"), "app": app}),
    };
    let mut cmd = Command::new(apexdesk);
    cmd.args(["info", "--json", "--page-starts"]).arg(path);
    let out = crate::oracle_proc::run(cmd, Duration::from_secs(240), None);
    if out.timed_out || out.code != Some(0) {
        let why = if out.timed_out { "timeout".to_string() } else { format!("exit {:?}: {}", out.code, out.stderr.lines().last().unwrap_or("")) };
        return json!({"file": file, "status": "error", "reason": format!("apexdesk info: {why}"), "app": app});
    }
    let info: Json = serde_json::from_str(&out.stdout).unwrap_or(Json::Null);
    let Some(starts) = info["page_starts"].as_array() else { return json!({"file": file, "status": "error", "reason": "apexdesk info: no page_starts", "app": app}) };
    let ours: Vec<(usize, bool)> = starts.iter().map(|s| (s["paragraph"].as_u64().unwrap_or(u64::MAX) as usize, s["continued"].as_bool().unwrap_or(false))).collect();
    let our_texts: Vec<String> = doc.paragraphs.iter().map(|p| p.text.clone()).collect();
    let pairs = align(&body.paras, &our_texts);
    let word: Vec<(usize, bool)> = std::iter::once((0, false)).chain(body.breaks.iter().copied()).collect();
    let mapped: Vec<(usize, bool)> = ours.iter().map(|&(p, c)| (if p == usize::MAX { p } else { to_word(&pairs, p) }, c)).collect();
    let compared = word.len().min(mapped.len());
    let same = (0..compared).filter(|&k| word[k].0 == mapped[k].0).count();
    let near = (0..compared).filter(|&k| word[k].0.abs_diff(mapped[k].0) <= 1).count();
    // Page starts of Word that are also one of ours, wherever (a shift
    // earlier in the document doesn't count against later pages).
    let set: std::collections::BTreeSet<usize> = mapped.iter().map(|m| m.0).collect();
    let found = word.iter().filter(|w| set.contains(&w.0)).count();
    let rows: Vec<Json> = (0..word.len().max(mapped.len()).min(300)).map(|k| json!([word.get(k).map(|w| w.0), mapped.get(k).map(|m| m.0)])).collect();
    json!({
        "file": file, "status": "checked", "app": app,
        "word_pages": pages, "our_pages": ours.len(),
        "paragraphs": [body.paras.len(), our_texts.len()], "aligned": pairs.len(),
        "compared": compared, "same": same, "near": near, "found": found,
        "hard_breaks": body.hard_breaks, "rows": rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_word_marks() {
        let xml = r#"<w:document><w:body><w:p><w:r><w:t>One</w:t></w:r></w:p><w:p><w:r><w:lastRenderedPageBreak/><w:t>Two &amp; more</w:t></w:r><w:r><w:pict><v:textbox><w:txbxContent><w:p><w:r><w:t>box</w:t></w:r></w:p></w:txbxContent></v:textbox></w:pict></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t><w:lastRenderedPageBreak/><w:t>x</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/><w:p><w:r><w:br w:type="page"/></w:r></w:p></w:body></w:document>"#;
        let b = word_body(xml);
        assert_eq!(b.paras, vec!["One", "Two & more", "cellx", "", ""]);
        assert_eq!(b.breaks, vec![(1, false), (2, true)]);
        assert_eq!(b.hard_breaks, 1);
    }

    #[test]
    fn aligns_paragraphs() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let word = s(&["Title", "", "Intro text.", "Extra in Word", "Body", "End"]);
        let ours = s(&["Title", "Intro text", "", "Body", "Inserted by us", "End"]);
        let pairs = align(&word, &ours);
        assert_eq!(pairs, vec![(0, 0), (2, 1), (4, 3), (5, 5)]);
        assert_eq!(to_word(&pairs, 3), 4);
        assert_eq!(to_word(&pairs, 2), 3);
    }
}
