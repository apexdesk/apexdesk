//! The MCP tools on a text document (see `crate::mcp`): reading the
//! paragraphs, inserting, replacing and formatting text. Positions are
//! paragraph numbers and characters in them; each edit is one undo step.

use std::sync::Arc;

use apex_doc::{Align, Attrs, ParaKind, ParaStyle, Pos, Selection};
use serde_json::{Value as Json, json};

use super::DocView;
use crate::mcp::tools::{byte_of, opt_bool, opt_color, opt_str, opt_usize, to_text};

fn kind_name(k: ParaKind) -> String {
    match k {
        ParaKind::Normal => "normal".into(),
        ParaKind::Heading(n) => format!("heading{n}"),
        ParaKind::Title => "title".into(),
        ParaKind::Quote => "quote".into(),
        ParaKind::Code => "code".into(),
        ParaKind::Bullet => "bullet".into(),
        ParaKind::Numbered => "numbered".into(),
    }
}

fn kind_of(s: &str) -> Option<ParaKind> {
    Some(match s {
        "normal" => ParaKind::Normal,
        "title" => ParaKind::Title,
        "quote" => ParaKind::Quote,
        "code" => ParaKind::Code,
        "bullet" => ParaKind::Bullet,
        "numbered" => ParaKind::Numbered,
        h => ParaKind::Heading(h.strip_prefix("heading")?.parse().ok().filter(|n| (1..=9).contains(n))?),
    })
}

impl DocView {
    /// What `list_documents` says about the document.
    pub(crate) fn mcp_info(&self) -> Json {
        json!({ "paragraphs": self.doc.paragraphs.len(), "words": self.doc.word_count() })
    }

    fn mcp_para(&self, args: &Json, key: &str) -> Result<usize, String> {
        let n = opt_usize(args, key)?.ok_or_else(|| format!("{key} is missing"))?;
        if n < self.doc.paragraphs.len() { Ok(n) } else { Err(format!("there are {} paragraphs, numbered from 0", self.doc.paragraphs.len())) }
    }

    fn mcp_editable(&self) -> Result<(), String> {
        if self.read_only || self.doc.edit_limit().is_some() { Err("the document is read-only or protected".to_string()) } else { Ok(()) }
    }

    /// The view's selection after an edit from outside.
    fn mcp_edited(&mut self, sel: Selection) {
        self.set_sel(sel);
        self.scroll_to_caret = false;
    }

    pub(crate) fn mcp_call(&mut self, tool: &str, args: &Json) -> Result<String, String> {
        match tool {
            "read_text" => self.mcp_read(args),
            "insert_text" | "set_paragraph" | "replace_text" | "format_text" => {
                self.mcp_editable()?;
                match tool {
                    "insert_text" => self.mcp_insert(args),
                    "set_paragraph" => self.mcp_set_paragraph(args),
                    "replace_text" => self.mcp_replace(args),
                    _ => self.mcp_format(args),
                }
            }
            _ => Err(format!("{tool} works on spreadsheets, and this tab is a text document")),
        }
    }

    fn mcp_read(&self, args: &Json) -> Result<String, String> {
        let total = self.doc.paragraphs.len();
        let from = opt_usize(args, "from")?.unwrap_or(0).min(total);
        let count = opt_usize(args, "count")?.unwrap_or(500).min(500);
        let paras: Vec<Json> = self.doc.paragraphs[from..(from + count).min(total)]
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut o = json!({ "n": from + i, "style": kind_name(p.style.kind), "text": p.text });
                if p.cell.is_some() {
                    o["in_table"] = json!(true);
                }
                o
            })
            .collect();
        Ok(to_text(&json!({ "total": total, "paragraphs": paras })))
    }

    fn mcp_insert(&mut self, args: &Json) -> Result<String, String> {
        let text = opt_str(args, "text").ok_or("text is missing")?;
        let (pos, text) = match opt_usize(args, "paragraph")? {
            // After the last paragraph, in a new one (or in it when empty).
            None => {
                let end = self.doc.end();
                if self.doc.paragraphs[end.para].text.is_empty() { (end, text.to_string()) } else { (end, format!("\n{text}")) }
            }
            Some(_) => {
                let p = self.mcp_para(args, "paragraph")?;
                let t = &self.doc.paragraphs[p].text;
                let off = opt_usize(args, "offset")?.map_or(t.len(), |n| byte_of(t, n));
                (Pos::new(p, off), text.to_string())
            }
        };
        if text.is_empty() {
            return Ok(to_text(&json!({ "inserted": 0 })));
        }
        let end = self.doc.insert_text(Selection::caret(pos), &text, None);
        self.mcp_edited(Selection::caret(end));
        Ok(to_text(&json!({ "inserted": text.chars().count(), "paragraphs": self.doc.paragraphs.len() })))
    }

    fn mcp_set_paragraph(&mut self, args: &Json) -> Result<String, String> {
        let p = self.mcp_para(args, "paragraph")?;
        let text = opt_str(args, "text").ok_or("text is missing")?;
        let len = self.doc.paragraphs[p].text.len();
        if len == 0 && text.is_empty() {
            return Ok(to_text(&json!({ "paragraph": p })));
        }
        let sel = Selection { anchor: Pos::new(p, 0), cursor: Pos::new(p, len) };
        let end = self.doc.insert_text(sel, text, None);
        self.mcp_edited(Selection::caret(end));
        Ok(to_text(&json!({ "paragraph": p, "paragraphs": self.doc.paragraphs.len() })))
    }

    fn mcp_replace(&mut self, args: &Json) -> Result<String, String> {
        let find = opt_str(args, "find").filter(|f| !f.is_empty()).ok_or("find is missing")?;
        let with = opt_str(args, "replace").ok_or("replace is missing")?;
        let n = self.doc.replace_all_opts(find, with, opt_bool(args, "match_case").unwrap_or(false), opt_bool(args, "whole_word").unwrap_or(false));
        let sel = self.sel;
        self.mcp_edited(sel);
        Ok(to_text(&json!({ "replaced": n })))
    }

    fn mcp_format(&mut self, args: &Json) -> Result<String, String> {
        let a = self.mcp_para(args, "paragraph")?;
        let b = match opt_usize(args, "to_paragraph")? {
            Some(_) => self.mcp_para(args, "to_paragraph")?,
            None => a,
        };
        if b < a {
            return Err("to_paragraph comes before paragraph".to_string());
        }
        let start = opt_usize(args, "start")?.map_or(0, |n| byte_of(&self.doc.paragraphs[a].text, n));
        let end = opt_usize(args, "end")?.map_or(self.doc.paragraphs[b].text.len(), |n| byte_of(&self.doc.paragraphs[b].text, n));
        if a == b && end < start {
            return Err("end comes before start".to_string());
        }
        let sel = Selection { anchor: Pos::new(a, start), cursor: Pos::new(b, end) };
        let (bold, italic, underline, strike) = (opt_bool(args, "bold"), opt_bool(args, "italic"), opt_bool(args, "underline"), opt_bool(args, "strike"));
        let font: Option<Arc<str>> = opt_str(args, "font").map(Arc::from);
        let size = match args.get("size").and_then(Json::as_f64) {
            Some(p) if (1.0..=1638.0).contains(&p) => Some((p * 2.0).round() as u16),
            Some(_) => return Err("size must be 1 to 1638 points".to_string()),
            None => None,
        };
        let (color, highlight) = (opt_color(args, "color")?, opt_color(args, "highlight")?);
        let kind = match opt_str(args, "style") {
            Some(s) => Some(kind_of(s).ok_or_else(|| format!("unknown style \"{s}\""))?),
            None => None,
        };
        let align = match opt_str(args, "align") {
            None => None,
            Some("left") => Some(Align::Left),
            Some("center") => Some(Align::Center),
            Some("right") => Some(Align::Right),
            Some("justify") => Some(Align::Justify),
            Some(x) => return Err(format!("unknown alignment \"{x}\"")),
        };
        let chars = |at: &mut Attrs| {
            if let Some(v) = bold {
                at.bold = v;
            }
            if let Some(v) = italic {
                at.italic = v;
            }
            if let Some(v) = underline {
                at.underline = v;
            }
            if let Some(v) = strike {
                at.strike = v;
            }
            if let Some(f) = &font {
                at.font = Some(f.clone());
            }
            if let Some(z) = size {
                at.size = Some(z);
            }
            if let Some(c) = color {
                at.color = c;
            }
            if let Some(c) = highlight {
                at.highlight = c;
            }
        };
        let styles = self.doc.styles.clone();
        let para = |st: &mut ParaStyle| {
            if let Some(k) = kind {
                styles.restyle(st, k, 0);
            }
            if let Some(al) = align {
                st.align = al;
            }
        };
        let para_too = kind.is_some() || align.is_some();
        if !sel.is_empty() {
            self.doc.format_with(sel, &|_, at| chars(at), if para_too { Some(&para) } else { None });
        } else if para_too {
            self.doc.set_para_style(a, b, sel, &para);
        }
        let keep = self.sel;
        self.mcp_edited(keep);
        Ok(to_text(&json!({ "formatted": { "from": [a, start], "to": [b, end] } })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::Document;

    #[test]
    fn read_insert_replace_format_undo() {
        let mut v = DocView::new(Document::from_plain_text("Hello world\nSecond"));
        let read: Json = serde_json::from_str(&v.mcp_call("read_text", &json!({})).unwrap()).unwrap();
        assert_eq!(read["total"], 2);
        assert_eq!(read["paragraphs"][1]["text"], "Second");
        v.mcp_call("insert_text", &json!({ "text": "Añadido" })).unwrap();
        assert_eq!(v.doc.paragraphs[2].text, "Añadido");
        v.mcp_call("insert_text", &json!({ "text": "big ", "paragraph": 0, "offset": 6 })).unwrap();
        assert_eq!(v.doc.paragraphs[0].text, "Hello big world");
        let r = v.mcp_call("replace_text", &json!({ "find": "world", "replace": "planet" })).unwrap();
        assert!(r.contains("\"replaced\":1"));
        v.mcp_call("set_paragraph", &json!({ "paragraph": 1, "text": "Otro" })).unwrap();
        assert_eq!(v.doc.paragraphs[1].text, "Otro");
        v.mcp_call("format_text", &json!({ "paragraph": 0, "start": 6, "end": 9, "bold": true, "color": "#C00000", "style": "heading1" })).unwrap();
        assert_eq!(v.doc.paragraphs[0].style.kind, ParaKind::Heading(1));
        assert!(v.doc.paragraphs[0].attrs_at(7).bold && !v.doc.paragraphs[0].attrs_at(2).bold);
        assert_eq!(v.doc.paragraphs[0].attrs_at(7).color, Some(0xC00000));
        // Each call is one step.
        for _ in 0..5 {
            v.undo();
        }
        assert_eq!(v.doc.to_plain_text(), "Hello world\nSecond");
        // Errors.
        assert!(v.mcp_call("format_text", &json!({ "paragraph": 9 })).is_err());
        assert!(v.mcp_call("format_text", &json!({ "paragraph": 0, "style": "fancy" })).is_err());
        assert!(v.mcp_call("write_cells", &json!({})).is_err());
        v.read_only = true;
        assert!(v.mcp_call("insert_text", &json!({ "text": "x" })).is_err());
    }
}
