//! Bringing in an Obsidian vault: a new section with the vault's notes,
//! its folders as pages holding theirs, and the links, tags and files the
//! notes point at kept.

use apex_fs::PathExt;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use apex_doc::notebook::fold;
use apex_doc::notebook::links::{Rel, page_links, target};
use apex_io::obsidian::{self, FileRef, Front};

use super::attachments::{ATTACH, is_picture};
use super::*;

/// How a name is looked up: folded, without `.md` and leading `./`.
fn key(name: &str) -> String {
    let n = name.trim().trim_start_matches("./");
    fold(n.strip_suffix(".md").unwrap_or(n))
}

/// `[ ] ` and `[x] ` starting a paragraph (Markdown's task lists) become
/// task boxes.
fn make_tasks(doc: &mut apex_doc::Document) {
    use apex_doc::{CHECKED, ContentControl, ControlKind, Pos, Selection, UNCHECKED};
    for i in 0..doc.paragraphs.len() {
        let t = &doc.paragraphs[i].text;
        let checked = match t.get(..4) {
            Some("[ ] ") => false,
            Some("[x] " | "[X] ") => true,
            _ => continue,
        };
        doc.insert_text(Selection { anchor: Pos::new(i, 0), cursor: Pos::new(i, 3) }, "", None);
        let id = doc.next_control_id();
        let mark = if checked { CHECKED } else { UNCHECKED };
        doc.insert_control(Selection::caret(Pos::new(i, 0)), ContentControl::new(id, ControlKind::CheckBox { checked }), &mark.to_string());
    }
}

impl NoteView {
    pub(super) fn pick_vault(&self) {
        crate::file_dialog::FileDialog::new().set_title(tr("nb.import_vault")).pick_folder(crate::file_dialog::in_view(|v: &mut NoteView, dir: PathBuf| {
            v.import_vault(&dir);
        }));
    }

    /// Brings in the vault at `dir` as a new section, which it returns.
    pub fn import_vault(&mut self, dir: &Path) -> Option<i64> {
        self.flush();
        let (entries, files) = obsidian::scan(dir);
        let notes: Vec<Option<(Front, String)>> = entries
            .iter()
            .map(|e| {
                let src = apex_fs::read(e.path.as_ref()?).ok()?;
                let src = String::from_utf8_lossy(&src).replace("\r\n", "\n");
                let (front, body) = obsidian::split_front(&src);
                Some((front, body.to_string()))
            })
            .collect();
        if notes.iter().all(Option::is_none) {
            self.notices.push(Notice::error(tr("nb.import_empty")));
            return None;
        }
        let name = dir.file_name().map_or_else(|| "Obsidian".to_string(), |n| n.to_string_lossy().into_owned());
        // The pages, as one step: a note named as its folder is the
        // folder's own page.
        let mut ids: Vec<i64> = Vec::with_capacity(entries.len());
        let mut section = 0;
        self.change(|nb| {
            section = nb.add_section(&name, now());
            let mut folders: HashMap<&[String], i64> = HashMap::new();
            let mut paths: Vec<Vec<String>> = Vec::new();
            for e in &entries {
                let mut p = e.folders.clone();
                p.push(e.title.clone());
                paths.push(p);
            }
            for (i, e) in entries.iter().enumerate() {
                let parent = folders.get(e.folders.as_slice()).copied();
                if e.path.is_some()
                    && let Some(&own) = folders.get(e.folders.as_slice())
                    && e.folders.last().is_some_and(|f| fold(f) == fold(&e.title))
                {
                    ids.push(own);
                    continue;
                }
                let id = nb.add_page(section, parent, None, &e.title, e.modified);
                if e.path.is_none() {
                    folders.insert(&paths[i], id);
                }
                if let Some(Some((front, _))) = notes.get(i)
                    && let Some(p) = nb.page_mut(id)
                {
                    p.aliases.clone_from(&front.aliases);
                }
                ids.push(id);
            }
        });
        // Where names lead: a note by its name, its path or an alias; a
        // file by its name or its path.
        let mut pages: HashMap<String, i64> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            let Some((front, _)) = &notes[i] else { continue };
            let path = e.folders.iter().chain([&e.title]).cloned().collect::<Vec<_>>().join("/");
            pages.insert(key(&path), ids[i]);
            for n in std::iter::once(&e.title).chain(&front.aliases) {
                pages.entry(key(n)).or_insert(ids[i]);
            }
        }
        let mut by_name: HashMap<String, PathBuf> = HashMap::new();
        for f in &files {
            if let Ok(rel) = f.strip_prefix(dir) {
                by_name.insert(fold(&rel.to_string_lossy().replace('\\', "/")), f.clone());
            }
            if let Some(n) = f.file_name() {
                by_name.entry(fold(&n.to_string_lossy())).or_insert_with(|| f.clone());
            }
        }
        let mut kept: HashMap<PathBuf, i64> = HashMap::new();
        // Notes may only bring files of the vault: a link that leads out of
        // it (`..`, an absolute path, a link on disk) is left as text.
        let root = dir.fs_canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        let inside = |p: &Path| p.fs_canonicalize().is_ok_and(|c| c.starts_with(&root));
        let mut count = 0;
        // Pictures shown in the notes, in bytes: past this they're
        // attached (once each) instead of copied into each note.
        let mut shown_left: u64 = 256 << 20;
        for (i, e) in entries.iter().enumerate() {
            let Some((front, body)) = &notes[i] else { continue };
            let id = ids[i];
            let near = e.path.as_deref().and_then(Path::parent).unwrap_or(dir).to_path_buf();
            let page = |n: &str| {
                let k = key(n);
                pages.get(&k).or_else(|| pages.get(k.rsplit('/').next().unwrap_or(&k))).map(|&p| target(p, Rel::Link).to_string())
            };
            let store = &mut self.store;
            let shown_left = &mut shown_left;
            let mut errors = Vec::new();
            let mut file = |n: &str, embed: bool| -> Option<FileRef> {
                // Nothing outside the vault is looked at: on Windows even
                // asking whether \\host\share\x exists signs in there.
                if Path::new(n).is_absolute() || n.starts_with(['/', '\\']) || n.split(['/', '\\']).any(|c| c == "..") || n.contains(':') || apex_sheet::query::is_network_path(n) {
                    return None;
                }
                let local = near.join(n);
                let path = if local.fs_is_file() && inside(&local) { local } else { by_name.get(&fold(n.trim_start_matches("./"))).or_else(|| by_name.get(&fold(n.rsplit('/').next()?)))?.clone() };
                if !inside(&path) {
                    return None;
                }
                if embed && is_picture(n) {
                    let len = apex_fs::metadata(&path).map_or(u64::MAX, |m| m.len());
                    if len <= *shown_left {
                        *shown_left -= len;
                        return apex_fs::read(&path).ok().map(FileRef::Picture);
                    }
                }
                if let Some(a) = kept.get(&path) {
                    return Some(FileRef::Link(format!("{ATTACH}{a}")));
                }
                let data = apex_fs::read(&path).ok()?;
                let fname = path.file_name()?.to_string_lossy().into_owned();
                match store.add_attachment(&fname, &data, Some(id), e.modified) {
                    Ok(a) => {
                        kept.insert(path, a);
                        Some(FileRef::Link(format!("{ATTACH}{a}")))
                    }
                    Err(err) => {
                        errors.push(err);
                        None
                    }
                }
            };
            let mut md = obsidian::convert(body, &page, &mut file);
            if !front.tags.is_empty() {
                let tags: Vec<String> = front.tags.iter().map(|t| format!("#{}", t.replace(' ', "_"))).collect();
                md = format!("{}\n\n{}\n", md.trim_end(), tags.join(" "));
            }
            let mut doc = apex_io::markdown::read(&md);
            make_tasks(&mut doc);
            let text = plain_text(&doc);
            let found = page_links(&doc);
            let r = self.store.save_page(id, &doc, &text, &found, e.modified);
            for err in errors {
                self.report(Err(err));
            }
            self.report(r);
            self.nb.texts.insert(id, text);
            self.nb.links.insert(id, found);
            self.nb.tasks.insert(id, apex_doc::notebook::tasks::tasks_in(&doc));
            count += 1;
        }
        self.close_graph();
        self.show_section(section);
        self.notices.push(Notice::info(trf("nb.imported", &[("n", &count), ("name", &name)])));
        Some(section)
    }
}
