//! An Obsidian vault: its notes (Markdown files in folders) and their
//! own syntax — `[[links]]`, `![[embeds]]`, YAML front matter — turned
//! into plain Markdown the reader takes.

use std::path::{Path, PathBuf};

/// A note or a folder of the vault, in the order they go.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The folders it's in, from the vault's root.
    pub folders: Vec<String>,
    /// The file's name without `.md`, or the folder's.
    pub title: String,
    /// The note's file (None: a folder).
    pub path: Option<PathBuf>,
    /// When it was last changed, in seconds.
    pub modified: i64,
}

/// What a note's front matter says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Front {
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
}

/// What a file a note points at becomes.
pub enum FileRef {
    /// A picture shown in the text.
    Picture(Vec<u8>),
    /// A link to this target.
    Link(String),
}

fn hidden(name: &str) -> bool {
    name.starts_with('.') || name == "node_modules"
}

fn mtime(p: &Path) -> i64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

/// The vault's folders with notes and the notes, a folder before what's
/// in it, each level by name. Also the other files, which notes may point at.
pub fn scan(root: &Path) -> (Vec<Entry>, Vec<PathBuf>) {
    fn walk(dir: &Path, folders: &[String], out: &mut Vec<Entry>, files: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        if folders.len() > 64 {
            return;
        }
        // Links are left out: they may loop, or reach outside the vault.
        let mut items: Vec<(String, PathBuf)> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| !t.is_symlink()))
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .filter(|(n, _)| !hidden(n))
            .collect();
        items.sort_by_key(|(n, _)| n.to_lowercase());
        for (name, path) in items {
            if path.is_dir() {
                let mut inner = folders.to_vec();
                inner.push(name.clone());
                let at = out.len();
                out.push(Entry { folders: folders.to_vec(), title: name, path: None, modified: mtime(&path) });
                walk(&path, &inner, out, files);
                // A folder of files only (pictures, say) is no page.
                if !out[at..].iter().any(|e| e.path.is_some()) {
                    out.truncate(at);
                }
            } else if let Some(stem) = name.strip_suffix(".md").or_else(|| name.strip_suffix(".MD")).filter(|_| path.is_file()) {
                out.push(Entry { folders: folders.to_vec(), title: stem.to_string(), path: Some(path.clone()), modified: mtime(&path) });
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    let (mut out, mut files) = (Vec::new(), Vec::new());
    walk(root, &[], &mut out, &mut files);
    (out, files)
}

/// A YAML list value: `[a, b]`, `a, b` or one item.
fn yaml_items(v: &str) -> Vec<String> {
    let v = v.trim().trim_start_matches('[').trim_end_matches(']');
    v.split(',').map(|s| s.trim().trim_matches(['"', '\'']).trim_start_matches('#').to_string()).filter(|s| !s.is_empty()).collect()
}

/// Splits off the front matter (`---` lines at the very start).
pub fn split_front(src: &str) -> (Front, &str) {
    let mut front = Front::default();
    let Some(rest) = src.strip_prefix("---\n").or_else(|| src.strip_prefix("---\r\n")) else { return (front, src) };
    let mut end = None;
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" || line.trim_end() == "..." {
            end = Some((at, at + line.len()));
            break;
        }
        at += line.len();
    }
    let Some((yaml_end, body)) = end else { return (front, src) };
    let mut list: Option<&mut Vec<String>> = None;
    for line in rest[..yaml_end].lines() {
        if let Some(item) = line.trim_start().strip_prefix("- ").filter(|_| line.starts_with([' ', '-', '\t'])) {
            if let Some(l) = list.as_deref_mut() {
                l.extend(yaml_items(item));
            }
            continue;
        }
        list = None;
        let Some((k, v)) = line.split_once(':') else { continue };
        let target = match k.trim().to_lowercase().as_str() {
            "tags" | "tag" => &mut front.tags,
            "aliases" | "alias" => &mut front.aliases,
            _ => continue,
        };
        target.extend(yaml_items(v));
        list = Some(target);
    }
    (front, &rest[body..])
}

/// Escapes text shown inside `[...]`.
fn label(s: &str) -> String {
    s.replace('\\', "\\\\").replace('[', "\\[").replace(']', "\\]")
}

/// `%20` and the like in a Markdown link's path.
fn unescape_url(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(h) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(h);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_local(url: &str) -> bool {
    !url.is_empty() && !url.contains("://") && !url.starts_with('#') && !url.starts_with("mailto:") && !url.starts_with("data:")
}

/// Turns a note's body into Markdown. `page` gives the link target of a
/// note named (as written, without `#heading`); `file` what a file named
/// becomes (`embed`: written with `!`). Links nothing answers stay as
/// their text.
pub fn convert(body: &str, page: &dyn Fn(&str) -> Option<String>, file: &mut dyn FnMut(&str, bool) -> Option<FileRef>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut fence: Option<&str> = None;
    for line in body.split_inclusive('\n') {
        let t = line.trim_start();
        if let Some(f) = fence {
            if t.starts_with(f) {
                fence = None;
            }
            out.push_str(line);
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = Some(&t[..3]);
            out.push_str(line);
            continue;
        }
        convert_line(line, page, file, &mut out);
    }
    out
}

fn picture(alt: &str, data: &[u8]) -> String {
    let mime = crate::docx_img::image_type(data).1;
    format!("![{}](data:{mime};base64,{})", label(alt), crate::markdown::base64_encode(data))
}

fn convert_line(line: &str, page: &dyn Fn(&str) -> Option<String>, file: &mut dyn FnMut(&str, bool) -> Option<FileRef>, out: &mut String) {
    let mut rest = line;
    // The next closers, kept while still ahead: looking again from every
    // `[` would cost the line's length squared. `Some(None)`: none left.
    // A link is `(mid, end, last [ before mid)`; one that led nowhere is
    // not tried again from a later `[` (`(mid, img)`).
    let mut wiki_at: Option<Option<usize>> = None;
    let mut link_at: Option<Option<(usize, usize, Option<usize>)>> = None;
    let mut dead_link: Option<(usize, bool)> = None;
    let mut open_fences = std::collections::HashSet::new();
    while !rest.is_empty() {
        // Code spans are kept as they are.
        if rest.starts_with('`') {
            let ticks = rest.len() - rest.trim_start_matches('`').len();
            let fence = &rest[..ticks];
            let end = if open_fences.contains(&ticks) { None } else { rest[ticks..].find(fence) };
            if end.is_none() {
                open_fences.insert(ticks);
            }
            if let Some(end) = end {
                let n = ticks + end + ticks;
                out.push_str(&rest[..n]);
                rest = &rest[n..];
                continue;
            }
        }
        let embed = rest.starts_with("![[");
        let at = line.len() - rest.len();
        let close = if embed || rest.starts_with("[[") {
            if wiki_at.is_none_or(|w| w.is_some_and(|w| w < at)) {
                wiki_at = Some(rest.find("]]").map(|c| at + c));
            }
            wiki_at.flatten().map(|c| c - at)
        } else {
            None
        };
        if let Some(close) = close {
            let inner = &rest[if embed { 3 } else { 2 }..close];
            rest = &rest[close + 2..];
            let (target, alias) = inner.split_once('|').map_or((inner, None), |(t, a)| (t, Some(a)));
            let target = target.trim();
            let name = target.split('#').next().unwrap_or("").trim();
            let shown = alias.filter(|a| !embed && !a.trim().is_empty()).map_or_else(|| target.replace('#', " › "), |a| a.trim().to_string());
            if name.is_empty() {
                // A heading of the same note.
                out.push_str(&label(&shown));
            } else if let Some(t) = page(name) {
                out.push_str(&format!("[{}]({t})", label(&shown)));
            } else {
                match file(name, embed) {
                    Some(FileRef::Picture(data)) => out.push_str(&picture(name, &data)),
                    Some(FileRef::Link(t)) => out.push_str(&format!("[{}]({t})", label(if embed { name } else { &shown }))),
                    None => out.push_str(&label(&shown)),
                }
            }
            continue;
        }
        // Markdown links to notes and files of the vault.
        let img = rest.starts_with("![");
        let link = if img || rest.starts_with('[') {
            if link_at.is_none_or(|l| l.is_some_and(|(m, _, _)| m < at)) {
                link_at = Some(rest.find("](").and_then(|mid| Some((at + mid, rest[mid + 2..].find(')')?, line[..at + mid].rfind('[')))));
            }
            link_at.flatten().filter(|&(m, _, _)| dead_link != Some((m, img))).map(|(m, e, b)| (m - at, e, b))
        } else {
            None
        };
        if let Some((mid, end, bracket)) = link {
            let text = &rest[if img { 2 } else { 1 }..mid];
            let url = rest[mid + 2..mid + 2 + end].trim().trim_start_matches('<').trim_end_matches('>');
            let nested = bracket.is_some_and(|b| b >= at + if img { 2 } else { 1 });
            if !nested && !is_local(url) {
                dead_link = Some((at + mid, img));
            }
            if is_local(url) && !nested {
                let path = unescape_url(url.split('#').next().unwrap_or(""));
                let note = path.strip_suffix(".md").filter(|_| !img);
                let done = match note.and_then(page) {
                    Some(t) => Some(format!("[{text}]({t})")),
                    None => match file(&path, img) {
                        Some(FileRef::Picture(data)) => Some(picture(text, &data)),
                        Some(FileRef::Link(t)) => Some(format!("[{}]({t})", if text.is_empty() { label(&path) } else { text.to_string() })),
                        None => None,
                    },
                };
                if let Some(d) = done {
                    out.push_str(&d);
                    rest = &rest[mid + 2 + end + 1..];
                    continue;
                }
                dead_link = Some((at + mid, img));
            }
        }
        let ch = rest.chars().next().expect("not empty");
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_gives_tags_and_aliases() {
        let src = "---\ntitle: X\ntags: [idea, \"libro/trama\"]\naliases:\n  - Faro\n  - El faro\n---\nBody\n";
        let (f, body) = split_front(src);
        assert_eq!(f.tags, ["idea", "libro/trama"]);
        assert_eq!(f.aliases, ["Faro", "El faro"]);
        assert_eq!(body, "Body\n");
        assert_eq!(split_front("No front\n").1, "No front\n");
    }

    #[test]
    fn links_and_embeds_become_markdown() {
        let page = |n: &str| (n.to_lowercase() == "faro" || n == "Lugares/Faro").then(|| "note:7".to_string());
        let mut file = |n: &str, embed: bool| match n {
            "mapa.png" if embed => Some(FileRef::Picture(vec![0x89, b'P', b'N', b'G'])),
            "plan.pdf" => Some(FileRef::Link("attach:3".into())),
            _ => None,
        };
        let src = "Ver [[Faro]], [[Lugares/Faro#Torre|la torre]] y [[Nadie]].\n![[mapa.png]] [[plan.pdf]] [x](Faro.md)\n`[[Faro]]`\n```\n[[Faro]]\n```\n";
        let md = convert(src, &page, &mut file);
        assert!(md.starts_with("Ver [Faro](note:7), [la torre](note:7) y Nadie.\n![mapa.png](data:"), "{md}");
        assert!(md.contains(" [plan.pdf](attach:3) [x](note:7)\n`[[Faro]]`\n```\n[[Faro]]\n```\n"), "{md}");
    }

    #[test]
    fn long_lines_of_openers_stay_linear() {
        let page = |n: &str| (n == "Faro").then(|| "note:7".to_string());
        let mut file = |_: &str, _: bool| None;
        assert_eq!(convert("[a [b](Faro.md) [c](web.md)", &page, &mut file), "[a [b](note:7) [c](web.md)");
        for src in ["[".repeat(200_000) + "](x)", "[[".repeat(100_000), "![".repeat(100_000) + "](http://x)", "`".repeat(1000) + &" `".repeat(100_000)] {
            let t = std::time::Instant::now();
            assert_eq!(convert(&src, &page, &mut file), src);
            assert!(t.elapsed().as_secs() < 5);
        }
    }
}
