//! Building blocks (AutoText, Quick Parts): the user's own, kept in
//! "Building Blocks.docx" in the config directory, and an organization's,
//! read from every .docx/.dotx/.dotm glossary in a shared folder (a
//! setting or policy, read-only from here). Both are Word glossaries
//! ([`apex_io::glossary`]), so Word's Normal.dotm and Building
//! Blocks.dotx can be dropped in the shared folder as they are.

use std::path::{Path, PathBuf};

use apex_doc::Document;
use apex_io::glossary::{self, Block, Entry};

/// A block as listed in the organizer.
#[derive(Clone, Debug)]
pub struct Item {
    pub entry: Entry,
    /// The glossary file it is in.
    pub file: PathBuf,
    /// From the shared folder.
    pub shared: bool,
}

static SHARED: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// The shared building blocks folder (None: none).
pub fn set_shared_dir(d: Option<PathBuf>) {
    *SHARED.lock().unwrap_or_else(|e| e.into_inner()) = d;
}

fn shared_dir() -> Option<PathBuf> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The user's glossary.
pub fn personal_file() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("Building Blocks.docx"))
}

fn items_in(file: &Path, shared: bool) -> Vec<Item> {
    let Ok(data) = apex_fs::read(file) else { return Vec::new() };
    glossary::list(&data).into_iter().map(|entry| Item { entry, file: file.to_path_buf(), shared }).collect()
}

/// Glossaries in a folder (sorted by name).
fn files_in(d: &Path) -> Vec<PathBuf> {
    let Ok(rd) = apex_fs::read_dir(d) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "docx" | "dotx" | "dotm" | "docm")))
        .collect();
    v.sort();
    v
}

/// The user's blocks, then the shared ones, each sorted by category and
/// name.
pub fn all() -> Vec<Item> {
    let mut v = personal_file().map(|f| items_in(&f, false)).unwrap_or_default();
    for f in shared_dir().map(|d| files_in(&d)).unwrap_or_default() {
        v.extend(items_in(&f, true));
    }
    let key = |i: &Item| (i.shared, i.entry.category.to_lowercase(), i.entry.name.to_lowercase());
    v.sort_by_key(key);
    v
}

/// A block's content.
pub fn load(item: &Item) -> Option<Document> {
    glossary::load(&apex_fs::read(&item.file).ok()?, item.entry.index)
}

/// The block named `word` (F3): an exact name first, the user's before
/// the shared ones, else the only name starting with it (4 or more
/// letters, as Word's AutoComplete).
pub fn find<'a>(items: &'a [Item], word: &str) -> Option<&'a Item> {
    let word = word.trim();
    if word.is_empty() {
        return None;
    }
    let lower = word.to_lowercase();
    if let Some(i) = items.iter().find(|i| i.entry.name.to_lowercase() == lower) {
        return Some(i);
    }
    if word.chars().count() < 4 {
        return None;
    }
    let mut starts = items.iter().filter(|i| i.entry.name.to_lowercase().starts_with(&lower));
    let first = starts.next()?;
    starts.all(|i| i.entry.name.eq_ignore_ascii_case(&first.entry.name)).then_some(first)
}

/// Where the name before the caret starts: the text back to a space.
pub fn name_start(text: &str, offset: usize) -> usize {
    text[..offset].char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8())
}

/// Every block of a glossary, loaded (to write it back).
fn blocks_of(data: &[u8]) -> Vec<(Entry, Document)> {
    glossary::list(data).into_iter().filter_map(|e| glossary::load(data, e.index).map(|d| (e, d))).collect()
}

fn write_all(file: &Path, blocks: &[(Entry, Document)]) -> std::io::Result<()> {
    let list: Vec<Block> = blocks.iter().map(|(e, d)| Block { name: &e.name, category: &e.category, gallery: &e.gallery, doc: d }).collect();
    if let Some(d) = file.parent() {
        apex_fs::create_dir_all(d)?;
    }
    let tmp = file.with_extension("docx.tmp");
    apex_fs::write(&tmp, glossary::write(&list))?;
    apex_fs::rename(&tmp, file)
}

/// Adds a block to a glossary file, replacing one of the same name.
pub fn save_in(file: &Path, name: &str, category: &str, doc: &Document) -> std::io::Result<()> {
    let mut blocks = apex_fs::read(file).map(|d| blocks_of(&d)).unwrap_or_default();
    let entry = Entry { name: name.trim().to_string(), category: category.trim().to_string(), gallery: "autoTxt".into(), preview: String::new(), index: 0 };
    match blocks.iter().position(|(e, _)| e.name.eq_ignore_ascii_case(name.trim())) {
        Some(i) => blocks[i] = (entry, doc.clone()),
        None => blocks.push((entry, doc.clone())),
    }
    write_all(file, &blocks)
}

/// Removes block `name` from a glossary file.
pub fn delete_in(file: &Path, name: &str) -> std::io::Result<()> {
    let mut blocks = blocks_of(&apex_fs::read(file)?);
    blocks.retain(|(e, _)| e.name != name);
    write_all(file, &blocks)
}

/// Saves a block of the user's.
pub fn save(name: &str, category: &str, doc: &Document) -> Result<(), String> {
    let f = personal_file().ok_or_else(|| "no configuration folder".to_string())?;
    save_in(&f, name, category, doc).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::Paragraph;

    fn item(name: &str, shared: bool) -> Item {
        Item { entry: Entry { name: name.into(), category: String::new(), gallery: "autoTxt".into(), preview: String::new(), index: 0 }, file: PathBuf::new(), shared }
    }

    #[test]
    fn lookup() {
        let items = [item("sig", false), item("Signature long", true), item("Address", true), item("Addressee", true)];
        assert_eq!(find(&items, "SIG").unwrap().entry.name, "sig");
        assert_eq!(find(&items, "sign").unwrap().entry.name, "Signature long");
        assert!(find(&items, "addr").is_none(), "two names start with it");
        assert_eq!(find(&items, "address").unwrap().entry.name, "Address");
        assert!(find(&items, "si").is_none() && find(&items, " ").is_none());
        assert_eq!(name_start("Dear sig", 8), 5);
        assert_eq!(name_start("añadir", 7), 0);
        assert_eq!(name_start("a b\tcé", 7), 4);
    }

    #[test]
    fn personal_store() {
        let dir = apex_fs::temp_dir().join(format!("apex-blocks-{}", apex_fs::process_id()));
        let file = dir.join("Building Blocks.docx");
        let doc = |t: &str| Document::from_paragraphs(vec![Paragraph::new(t)]);
        save_in(&file, "sig", "Letters", &doc("Regards")).unwrap();
        save_in(&file, "addr", "", &doc("Main St")).unwrap();
        save_in(&file, "SIG", "Letters", &doc("Best")).unwrap();
        let items = items_in(&file, false);
        let names: Vec<&str> = items.iter().map(|i| i.entry.name.as_str()).collect();
        assert_eq!(names, ["SIG", "addr"]);
        assert_eq!(load(&items[0]).unwrap().paragraphs[0].text, "Best");
        delete_in(&file, "SIG").unwrap();
        let items = items_in(&file, false);
        assert_eq!(items.len(), 1);
        assert_eq!(load(&items[0]).unwrap().paragraphs[0].text, "Main St");
        assert_eq!(files_in(&dir), [file.clone()]);
        let _ = apex_fs::remove_dir_all(&dir);
    }
}
