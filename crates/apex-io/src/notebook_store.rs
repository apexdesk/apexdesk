//! A notebook's file (.apexnote): a SQLite database with its sections,
//! its pages (each one's text as a .docx and as plain text, to search)
//! and room for their history. Every change is written as it is made;
//! the file is held while open so that no one else changes it meanwhile.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use apex_doc::Document;
use apex_doc::{Source, SourceKind};
use apex_doc::notebook::canvas::{Arrow, Canvas, Item, ItemKind};
use apex_doc::notebook::tasks::{Task, TaskMeta, tasks_in};
use apex_doc::notebook::query::Query;
use apex_doc::notebook::types::{fields_from_key, fields_key, values_from_key, values_key};
use apex_doc::notebook::{Claim, NoteSection, Notebook, Page, PageKind, PageType, Rel};
use rusqlite::{Connection, OptionalExtension, params};

use crate::note_crypt::{self, Key};

/// File extension.
pub const EXT: &str = "apexnote";
const APP: &str = "apexnote";
const VERSION: i64 = 1;

/// Why a notebook didn't open.
#[derive(Debug, Clone, PartialEq)]
pub enum NoteError {
    /// Another program (or window) has it open.
    Locked,
    /// It isn't a notebook.
    NotNotebook,
    /// The page is in a section under a password not given yet.
    NeedsPassword,
    Other(String),
}

impl std::fmt::Display for NoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoteError::Locked => f.write_str("locked"),
            NoteError::NotNotebook => f.write_str("not a notebook"),
            NoteError::NeedsPassword => f.write_str("the section is locked"),
            NoteError::Other(e) => f.write_str(e),
        }
    }
}

/// No SQL a file brings runs with more than plain functions, and the
/// schema can't be written behind SQLite's back (as `apex_db::sqlite::harden`
/// does, but ATTACH stays: copies are made through it).
fn harden(conn: &Connection) -> rusqlite::Result<()> {
    use rusqlite::config::DbConfig;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    // No value bigger than a page's pictures may be.
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 256 << 20)?;
    Ok(())
}

fn other(e: rusqlite::Error) -> NoteError {
    NoteError::Other(e.to_string())
}

fn busy(e: &rusqlite::Error) -> bool {
    matches!(e.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS sections(id INTEGER PRIMARY KEY, name TEXT NOT NULL, color INTEGER NOT NULL, pos INTEGER NOT NULL, deleted INTEGER);
CREATE TABLE IF NOT EXISTS pages(id INTEGER PRIMARY KEY, section INTEGER NOT NULL, parent INTEGER, pos INTEGER NOT NULL, title TEXT NOT NULL, created INTEGER NOT NULL, modified INTEGER NOT NULL, deleted INTEGER, text TEXT NOT NULL DEFAULT '', body BLOB);
CREATE TABLE IF NOT EXISTS history(id INTEGER PRIMARY KEY, page INTEGER NOT NULL, at INTEGER NOT NULL, body BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS links(src INTEGER NOT NULL, dst INTEGER NOT NULL, rel TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS links_src ON links(src);
CREATE TABLE IF NOT EXISTS canvas_items(page INTEGER NOT NULL, id INTEGER NOT NULL, pos INTEGER NOT NULL, kind TEXT NOT NULL, x REAL NOT NULL, y REAL NOT NULL, w REAL NOT NULL, h REAL NOT NULL, color INTEGER NOT NULL, ref INTEGER, text TEXT NOT NULL DEFAULT '', data BLOB);
CREATE INDEX IF NOT EXISTS canvas_items_page ON canvas_items(page, pos);
CREATE TABLE IF NOT EXISTS sources(page INTEGER PRIMARY KEY, kind TEXT NOT NULL, authors TEXT NOT NULL, title TEXT NOT NULL, year TEXT NOT NULL, city TEXT NOT NULL, publisher TEXT NOT NULL, journal TEXT NOT NULL, volume TEXT NOT NULL, pages TEXT NOT NULL, url TEXT NOT NULL, tag TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tasks(page INTEGER NOT NULL, para INTEGER NOT NULL, done INTEGER NOT NULL, text TEXT NOT NULL, tag TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS attachments(id INTEGER PRIMARY KEY, page INTEGER, name TEXT NOT NULL, added INTEGER NOT NULL, size INTEGER NOT NULL, data BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS page_types(id INTEGER PRIMARY KEY, name TEXT NOT NULL, color INTEGER NOT NULL, pos INTEGER NOT NULL, fields TEXT NOT NULL, template INTEGER);
CREATE TABLE IF NOT EXISTS queries(id INTEGER PRIMARY KEY, name TEXT NOT NULL, pos INTEGER NOT NULL, spec TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS canvas_arrows(page INTEGER NOT NULL, id INTEGER NOT NULL, src INTEGER NOT NULL, dst INTEGER NOT NULL, label TEXT NOT NULL DEFAULT '', rel TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS canvas_arrows_page ON canvas_arrows(page);
";

/// A file kept in a notebook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub id: i64,
    /// The page it was attached from.
    pub page: Option<i64>,
    pub name: String,
    /// Seconds since 1970.
    pub added: i64,
    pub size: u64,
}

/// The setting that says the tasks table is complete.
const TASKS_KEPT: &str = "tasks_kept";

/// How long opening and reading a notebook may take: a file's tables
/// can be made to take for ever.
const READ_SECONDS: u64 = 30;

/// Keeps the tasks of page `id` as they are in `doc`.
fn write_tasks(conn: &Connection, id: i64, doc: &Document) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM tasks WHERE page = ?1", params![id])?;
    for t in tasks_in(doc) {
        conn.execute("INSERT INTO tasks(page, para, done, text, tag) VALUES (?1, ?2, ?3, ?4, ?5)", params![id, t.para as i64, t.done, t.text, t.meta.tag()])?;
    }
    Ok(())
}

/// Columns added after the first files were made.
/// A page's text is kept as a version when it changes at least this long
/// (in seconds) after the last version kept…
const VERSION_GAP: i64 = 600;
/// …and a page keeps this many.
const VERSIONS_KEPT: i64 = 100;

/// An earlier text of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    pub id: i64,
    /// When the page was like this.
    pub at: i64,
    pub size: u64,
}

/// Keeps page `id`'s text as it is in the file among its versions,
/// unless it's the last one kept or that one is less than `VERSION_GAP`
/// older than `now` (the time of the text replacing it; None: kept
/// anyway).
fn keep_version(conn: &Connection, id: i64, now: Option<i64>) -> rusqlite::Result<()> {
    let Some((body, modified)) = conn.query_row("SELECT body, modified FROM pages WHERE id = ?1", params![id], |r| Ok((r.get::<_, Option<Vec<u8>>>(0)?, r.get::<_, i64>(1)?))).optional()? else { return Ok(()) };
    let Some(body) = body.filter(|b| !b.is_empty()) else { return Ok(()) };
    let last: Option<(i64, Vec<u8>)> = conn.query_row("SELECT at, body FROM history WHERE page = ?1 ORDER BY at DESC, id DESC LIMIT 1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    if last.as_ref().is_some_and(|(at, b)| *b == body || now.is_some_and(|t| t - at < VERSION_GAP)) {
        return Ok(());
    }
    conn.execute("INSERT INTO history(page, at, body) VALUES (?1, ?2, ?3)", params![id, modified, body])?;
    conn.execute("DELETE FROM history WHERE page = ?1 AND id NOT IN (SELECT id FROM history WHERE page = ?1 ORDER BY at DESC, id DESC LIMIT ?2)", params![id, VERSIONS_KEPT])?;
    Ok(())
}

fn upgrade(conn: &Connection) -> rusqlite::Result<()> {
    let has = conn.prepare("SELECT 1 FROM pragma_table_info('pages') WHERE name = 'aliases'")?.exists([])?;
    if !has {
        conn.execute_batch("ALTER TABLE pages ADD COLUMN aliases TEXT NOT NULL DEFAULT ''")?;
    }
    let has = conn.prepare("SELECT 1 FROM pragma_table_info('pages') WHERE name = 'kind'")?.exists([])?;
    if !has {
        conn.execute_batch("ALTER TABLE pages ADD COLUMN kind TEXT NOT NULL DEFAULT 'text'")?;
    }
    let has = conn.prepare("SELECT 1 FROM pragma_table_info('pages') WHERE name = 'claim'")?.exists([])?;
    if !has {
        conn.execute_batch("ALTER TABLE pages ADD COLUMN claim TEXT NOT NULL DEFAULT ''")?;
    }
    let has = conn.prepare("SELECT 1 FROM pragma_table_info('pages') WHERE name = 'ptype'")?.exists([])?;
    if !has {
        conn.execute_batch("ALTER TABLE pages ADD COLUMN ptype INTEGER; ALTER TABLE pages ADD COLUMN fields TEXT NOT NULL DEFAULT ''")?;
    }
    Ok(())
}

pub struct NoteStore {
    conn: Connection,
    /// The keys of the sections unlocked.
    keys: HashMap<i64, Key>,
    /// Pages the user took out of a section under a password (or whose
    /// password was taken off) while unlocked: only theirs is opened
    /// into the clear.
    freed: HashSet<i64>,
}

/// Where a section's lock is kept, in `meta`.
/// A short text sealed: a mark that can't start one typed, and the
/// sealed bytes in hex. An empty one stays empty.
fn seal_text(key: &Key, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    format!("\u{1}{}", note_crypt::hex(&note_crypt::seal(key, text.as_bytes())))
}

/// A text as written, opened when sealed (None without its key).
fn open_text<'a>(mut keys: impl Iterator<Item = &'a Key>, text: &str) -> Option<String> {
    let Some(sealed) = text.strip_prefix('\u{1}') else { return Some(text.to_string()) };
    let sealed = note_crypt::unhex(sealed)?;
    keys.find_map(|k| note_crypt::open(k, &sealed)).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Seals what canvases and attachments hold in pages of sections under a
/// password (whose key is known), and opens it elsewhere with `keys`.
/// Only pages in `freed` are opened: a sealed page found outside its
/// section was put there by the file, not by the user.
fn reseal_parts(tx: &Connection, locks: &HashSet<i64>, known: &HashMap<i64, Key>, keys: &[Key], freed: &HashSet<i64>) -> rusqlite::Result<()> {
    let sections: HashMap<i64, i64> = tx.prepare("SELECT id, section FROM pages")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    // Some(key) to seal, None to open; a locked section whose key isn't
    // known is left as it is.
    let wanted = |page: i64| -> Option<Option<&Key>> {
        let s = sections.get(&page)?;
        if locks.contains(s) {
            known.get(s).map(Some)
        } else {
            freed.contains(&page).then_some(None)
        }
    };
    let open = |b: &[u8]| keys.iter().find_map(|k| note_crypt::open(k, b));
    const SEALED: &str = "substr(data, 1, 5) = CAST('APXE1' AS BLOB)";
    let items: Vec<(i64, i64, String, bool, bool)> = tx
        .prepare(&format!("SELECT rowid, page, kind, coalesce({SEALED}, 0), text != '' OR data IS NOT NULL FROM canvas_items"))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<Result<_, _>>()?;
    for (row, page, kind, sealed, full) in items {
        match wanted(page) {
            Some(Some(k)) if full && !sealed => {
                let (text, data): (String, Option<Vec<u8>>) = tx.query_row("SELECT text, data FROM canvas_items WHERE rowid = ?1", params![row], |r| Ok((r.get(0)?, r.get(1)?)))?;
                let b = note_crypt::seal(k, data.as_deref().unwrap_or(text.as_bytes()));
                tx.execute("UPDATE canvas_items SET text = '', data = ?2 WHERE rowid = ?1", params![row, b])?;
            }
            Some(None) if sealed => {
                let data: Vec<u8> = tx.query_row("SELECT data FROM canvas_items WHERE rowid = ?1", params![row], |r| r.get(0))?;
                let Some(plain) = open(&data) else { continue };
                match kind.as_str() {
                    "note" | "group" => tx.execute("UPDATE canvas_items SET text = ?2, data = NULL WHERE rowid = ?1", params![row, String::from_utf8_lossy(&plain)])?,
                    _ => tx.execute("UPDATE canvas_items SET data = ?2 WHERE rowid = ?1", params![row, plain])?,
                };
            }
            _ => {}
        }
    }
    let arrows: Vec<(i64, i64, String)> = tx.prepare("SELECT rowid, page, label FROM canvas_arrows WHERE label != ''")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    for (row, page, label) in arrows {
        let sealed = label.starts_with('\u{1}');
        let label = match wanted(page) {
            Some(Some(k)) if !sealed => seal_text(k, &label),
            Some(None) if sealed => match open_text(keys.iter(), &label) {
                Some(l) => l,
                None => continue,
            },
            _ => continue,
        };
        tx.execute("UPDATE canvas_arrows SET label = ?2 WHERE rowid = ?1", params![row, label])?;
    }
    let files: Vec<(i64, i64, bool)> = tx.prepare(&format!("SELECT id, page, {SEALED} FROM attachments WHERE page IS NOT NULL"))?.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    for (id, page, sealed) in files {
        let (name, data): (String, Vec<u8>) = match wanted(page) {
            Some(Some(_)) if !sealed => tx.query_row("SELECT name, data FROM attachments WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))?,
            Some(None) if sealed => tx.query_row("SELECT name, data FROM attachments WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))?,
            _ => continue,
        };
        let (name, data) = match wanted(page) {
            Some(Some(k)) => (seal_text(k, &name), note_crypt::seal(k, &data)),
            _ => match (open_text(keys.iter(), &name), open(&data)) {
                (Some(n), Some(d)) => (n, d),
                _ => continue,
            },
        };
        tx.execute("UPDATE attachments SET name = ?2, data = ?3 WHERE id = ?1", params![id, name, data])?;
    }
    // A canvas's words to search, like a page's text.
    if !locks.is_empty() {
        let ids = locks.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        tx.execute(&format!("UPDATE pages SET text = '' WHERE section IN ({ids}) AND text != ''"), [])?;
    }
    Ok(())
}

fn lock_key(section: i64) -> String {
    format!("lock:{section}")
}

impl NoteStore {
    /// A new, empty notebook at `path` (replacing any file there).
    pub fn create(path: &Path) -> Result<NoteStore, NoteError> {
        // A link there is removed, not written through.
        apex_db::sqlite::clear(path).map_err(|e| NoteError::Other(e.to_string()))?;
        use rusqlite::OpenFlags as F;
        let flags = F::SQLITE_OPEN_READ_WRITE | F::SQLITE_OPEN_CREATE | F::SQLITE_OPEN_NOFOLLOW | F::SQLITE_OPEN_URI | F::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags).map_err(other)?;
        harden(&conn).map_err(other)?;
        conn.execute_batch(SCHEMA).map_err(other)?;
        upgrade(&conn).map_err(other)?;
        conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES ('app', ?1), ('version', ?2)", params![APP, VERSION.to_string()]).map_err(other)?;
        Self::hold(conn)
    }

    /// The notebook at `path`, held until it is closed.
    /// The notebook's name: its file's, without the extension.
    pub fn name(&self) -> String {
        self.conn.path().map(Path::new).and_then(Path::file_stem).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    pub fn open(path: &Path) -> Result<NoteStore, NoteError> {
        apex_db::sqlite::check_journal(path).map_err(|e| NoteError::Other(e.to_string()))?;
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(other)?;
        // The file is anyone's: no SQL of its own runs here.
        harden(&conn).map_err(other)?;
        // Held by someone else: say so at once.
        conn.busy_timeout(web_time::Duration::ZERO).map_err(other)?;
        // Nothing of the file's is read before its schema is checked (a
        // `meta` view could run for ever), and all of it in a time limit.
        let end = web_time::Instant::now() + web_time::Duration::from_secs(READ_SECONDS);
        let _ = conn.progress_handler(100_000, Some(move || web_time::Instant::now() > end));
        // A notebook has no triggers or views: one brought by the file
        // would run on our own writes.
        // Nor computed columns, checks, defaults worked out or indexes of
        // its own, which would compute whatever the file says.
        let foreign: i64 = conn
            .query_row(
                "SELECT (SELECT count(*) FROM sqlite_master WHERE type IN ('trigger', 'view'))
                      + (SELECT count(*) FROM sqlite_master m, pragma_table_xinfo(m.name) x WHERE m.type = 'table' AND (x.hidden IN (2, 3) OR (instr(x.dflt_value, '(') > 0 AND substr(x.dflt_value, 1, 1) != '''')))
                      + (SELECT count(*) FROM sqlite_master WHERE type = 'table' AND instr(upper(sql), 'CHECK') > 0)
                      + (SELECT count(*) FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL
                         AND replace(replace(replace(upper(sql), ' ', ''), char(10), ''), char(9), '') NOT IN ('CREATEINDEXLINKS_SRCONLINKS(SRC)', 'CREATEINDEXCANVAS_ITEMS_PAGEONCANVAS_ITEMS(PAGE,POS)', 'CREATEINDEXCANVAS_ARROWS_PAGEONCANVAS_ARROWS(PAGE)'))
                      + (SELECT count(*) FROM sqlite_master m WHERE m.type = 'table' AND m.name IN ('sections', 'pages', 'history', 'attachments', 'page_types', 'queries')
                         AND NOT EXISTS (SELECT 1 FROM pragma_table_info(m.name) x WHERE x.name = 'id' AND x.pk = 1))",
                [],
                |r| r.get(0),
            )
            .map_err(|e| if busy(&e) { NoteError::Locked } else { NoteError::NotNotebook })?;
        if foreign > 0 {
            return Err(NoteError::NotNotebook);
        }
        let app: Option<String> = match conn.query_row("SELECT value FROM meta WHERE key = 'app'", [], |r| r.get(0)).optional() {
            Ok(v) => v,
            Err(e) if busy(&e) => return Err(NoteError::Locked),
            Err(_) => return Err(NoteError::NotNotebook),
        };
        if app.as_deref() != Some(APP) {
            return Err(NoteError::NotNotebook);
        }
        conn.execute_batch(SCHEMA).map_err(|e| if busy(&e) { NoteError::Locked } else { other(e) })?;
        upgrade(&conn).map_err(other)?;
        let _ = conn.progress_handler(0, None::<fn() -> bool>);
        Self::hold(conn)
    }

    /// Takes the file for this connection alone until it closes.
    fn hold(conn: Connection) -> Result<NoteStore, NoteError> {
        conn.busy_timeout(web_time::Duration::ZERO).map_err(other)?;
        // What is deleted or replaced (a text once sealed) is wiped, not
        // left in the file's free space.
        conn.execute_batch("PRAGMA secure_delete = ON;").map_err(other)?;
        conn.execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE; COMMIT;").map_err(|e| if busy(&e) { NoteError::Locked } else { other(e) })?;
        Ok(NoteStore { conn, keys: HashMap::new(), freed: HashSet::new() })
    }

    /// The sections and pages, with each page's plain text.
    pub fn load(&self) -> Result<Notebook, NoteError> {
        let end = web_time::Instant::now() + web_time::Duration::from_secs(READ_SECONDS);
        let _ = self.conn.progress_handler(100_000, Some(move || web_time::Instant::now() > end));
        let r = self.load_all();
        let _ = self.conn.progress_handler(0, None::<fn() -> bool>);
        r
    }

    fn load_all(&self) -> Result<Notebook, NoteError> {
        let mut nb = Notebook::default();
        let mut q = self.conn.prepare("SELECT id, name, color, pos, deleted FROM sections").map_err(other)?;
        nb.sections = q.query_map([], |r| Ok(NoteSection { id: r.get(0)?, name: r.get(1)?, color: r.get::<_, i64>(2)? as u32, pos: r.get(3)?, deleted: r.get(4)? })).map_err(other)?.collect::<Result<_, _>>().map_err(other)?;
        let mut q = self.conn.prepare("SELECT id, section, parent, pos, title, created, modified, deleted, text, aliases, kind, claim, ptype, fields FROM pages").map_err(other)?;
        let rows = q
            .query_map([], |r| {
                let aliases: String = r.get(9)?;
                let aliases = aliases.split('\n').filter(|a| !a.is_empty()).map(String::from).collect();
                let kind = PageKind::from_key(&r.get::<_, String>(10)?);
                Ok((Page { id: r.get(0)?, section: r.get(1)?, parent: r.get(2)?, pos: r.get(3)?, title: r.get(4)?, aliases, kind, source: None, claim: Claim::from_key(&r.get::<_, String>(11)?), ptype: r.get(12)?, fields: values_from_key(&r.get::<_, String>(13)?), created: r.get(5)?, modified: r.get(6)?, deleted: r.get(7)? }, r.get::<_, String>(8)?))
            })
            .map_err(other)?;
        for row in rows {
            let (p, text) = row.map_err(other)?;
            // Each id once (a file's table may lack its key).
            if nb.texts.contains_key(&p.id) {
                continue;
            }
            nb.texts.insert(p.id, text);
            nb.pages.push(p);
        }
        nb.fix_parents();
        let mut q = self.conn.prepare("SELECT id, name, color, fields, template FROM page_types ORDER BY pos, id").map_err(other)?;
        let rows = q.query_map([], |r| Ok(PageType { id: r.get(0)?, name: r.get(1)?, color: r.get::<_, i64>(2)? as u32, fields: fields_from_key(&r.get::<_, String>(3)?), template: r.get(4)? })).map_err(other)?;
        nb.types = rows.collect::<Result<_, _>>().map_err(other)?;
        let mut q = self.conn.prepare("SELECT id, name, spec FROM queries ORDER BY pos, id").map_err(other)?;
        let rows = q.query_map([], |r| Ok(Query::from_key(r.get(0)?, &r.get::<_, String>(1)?, &r.get::<_, String>(2)?))).map_err(other)?;
        nb.queries = rows.collect::<Result<_, _>>().map_err(other)?;
        let mut q = self.conn.prepare("SELECT page, kind, authors, title, year, city, publisher, journal, volume, pages, url, tag FROM sources").map_err(other)?;
        let rows = q
            .query_map([], |r| {
                let s = Source {
                    kind: SourceKind::from_key(&r.get::<_, String>(1)?),
                    authors: r.get(2)?,
                    title: r.get(3)?,
                    year: r.get(4)?,
                    city: r.get(5)?,
                    publisher: r.get(6)?,
                    journal: r.get(7)?,
                    volume: r.get(8)?,
                    pages: r.get(9)?,
                    url: r.get(10)?,
                    tag: r.get(11)?,
                };
                Ok((r.get::<_, i64>(0)?, s))
            })
            .map_err(other)?;
        let at: HashMap<i64, usize> = nb.pages.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
        for row in rows {
            let (id, s) = row.map_err(other)?;
            if let Some(&i) = at.get(&id) {
                nb.pages[i].source = Some(s);
            }
        }
        let mut q = self.conn.prepare("SELECT src, dst, rel FROM links ORDER BY rowid").map_err(other)?;
        let rows = q.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))).map_err(other)?;
        for row in rows {
            let (src, dst, rel) = row.map_err(other)?;
            nb.links.entry(src).or_default().push((dst, Rel::from_key(&rel)));
        }
        self.index_tasks()?;
        let mut q = self.conn.prepare("SELECT page, para, done, text, tag FROM tasks ORDER BY page, para").map_err(other)?;
        let rows = q.query_map([], |r| Ok((r.get::<_, i64>(0)?, Task { para: r.get::<_, i64>(1)? as usize, done: r.get(2)?, text: r.get(3)?, meta: TaskMeta::from_tag(&r.get::<_, String>(4)?) }))).map_err(other)?;
        for row in rows {
            let (page, t) = row.map_err(other)?;
            nb.tasks.entry(page).or_default().push(t);
        }
        let canvases: Vec<i64> = nb.pages.iter().filter(|p| p.kind == PageKind::Canvas).map(|p| p.id).collect();
        for c in canvases {
            // One under a password still locked: its links show once unlocked.
            let links = match self.load_canvas(c) {
                Ok(canvas) => canvas.page_links(c),
                Err(NoteError::NeedsPassword) => continue,
                Err(e) => return Err(e),
            };
            if !links.is_empty() {
                nb.canvas_links.insert(c, links);
            }
        }
        Ok(nb)
    }

    /// Writes the sections and the pages' places, titles and dates as
    /// they are in `nb` (removing those it no longer has).
    pub fn save_structure(&mut self, nb: &Notebook) -> Result<(), NoteError> {
        let locks = self.locks();
        let was: HashMap<i64, i64> = {
            let mut q = self.conn.prepare("SELECT id, section FROM pages").map_err(other)?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(other)?.collect::<Result<_, _>>().map_err(other)?
        };
        for p in &nb.pages {
            if was.get(&p.id).is_some_and(|s| *s != p.section && self.keys.contains_key(s)) && !locks.contains(&p.section) {
                self.freed.insert(p.id);
            }
        }
        let tx = self.conn.transaction().map_err(other)?;
        {
            let mut up = tx.prepare("INSERT INTO sections(id, name, color, pos, deleted) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(id) DO UPDATE SET name = ?2, color = ?3, pos = ?4, deleted = ?5").map_err(other)?;
            for s in &nb.sections {
                up.execute(params![s.id, s.name, s.color as i64, s.pos, s.deleted]).map_err(other)?;
            }
            let mut up = tx
                .prepare("INSERT INTO pages(id, section, parent, pos, title, created, modified, deleted, text, aliases, kind, claim, ptype, fields) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) ON CONFLICT(id) DO UPDATE SET section = ?2, parent = ?3, pos = ?4, title = ?5, modified = ?7, deleted = ?8, aliases = ?10, kind = ?11, claim = ?12, ptype = ?13, fields = ?14")
                .map_err(other)?;
            for p in &nb.pages {
                let text = nb.texts.get(&p.id).map_or("", String::as_str);
                up.execute(params![p.id, p.section, p.parent, p.pos, p.title, p.created, p.modified, p.deleted, text, p.aliases.join("\n"), p.kind.key(), p.claim.map(Claim::key).unwrap_or_default(), p.ptype, values_key(&p.fields)]).map_err(other)?;
            }
            tx.execute("DELETE FROM page_types", []).map_err(other)?;
            let mut up = tx.prepare("INSERT INTO page_types(id, name, color, pos, fields, template) VALUES (?1, ?2, ?3, ?4, ?5, ?6)").map_err(other)?;
            for (i, t) in nb.types.iter().enumerate() {
                up.execute(params![t.id, t.name, t.color as i64, i as i64, fields_key(&t.fields), t.template]).map_err(other)?;
            }
            tx.execute("DELETE FROM queries", []).map_err(other)?;
            let mut up = tx.prepare("INSERT INTO queries(id, name, pos, spec) VALUES (?1, ?2, ?3, ?4)").map_err(other)?;
            for (i, q) in nb.queries.iter().enumerate() {
                up.execute(params![q.id, q.name, i as i64, q.key()]).map_err(other)?;
            }
            tx.execute("DELETE FROM sources", []).map_err(other)?;
            let mut up = tx.prepare("INSERT INTO sources(page, kind, authors, title, year, city, publisher, journal, volume, pages, url, tag) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)").map_err(other)?;
            for p in &nb.pages {
                if let Some(s) = &p.source {
                    up.execute(params![p.id, s.kind.key(), s.authors, s.title, s.year, s.city, s.publisher, s.journal, s.volume, s.pages, s.url, s.tag]).map_err(other)?;
                }
            }
        }
        let ids = |v: Vec<i64>| v.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        tx.execute(&format!("DELETE FROM sections WHERE id NOT IN ({})", ids(nb.sections.iter().map(|s| s.id).collect())), []).map_err(other)?;
        let pages = ids(nb.pages.iter().map(|p| p.id).collect());
        tx.execute(&format!("DELETE FROM pages WHERE id NOT IN ({pages})"), []).map_err(other)?;
        tx.execute(&format!("DELETE FROM history WHERE page NOT IN ({pages})"), []).map_err(other)?;
        tx.execute(&format!("DELETE FROM links WHERE src NOT IN ({pages})"), []).map_err(other)?;
        tx.execute(&format!("DELETE FROM canvas_items WHERE page NOT IN ({pages})"), []).map_err(other)?;
        tx.execute(&format!("DELETE FROM canvas_arrows WHERE page NOT IN ({pages})"), []).map_err(other)?;
        tx.commit().map_err(other)?;
        self.reseal()
    }

    /// The sections under a password.
    pub fn locks(&self) -> HashSet<i64> {
        let mut q = match self.conn.prepare("SELECT substr(key, 6) FROM meta WHERE key LIKE 'lock:%'") {
            Ok(q) => q,
            Err(_) => return HashSet::new(),
        };
        q.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(|r| r.ok()?.parse().ok()).collect()).unwrap_or_default()
    }

    fn lock_of(&self, section: i64) -> Option<String> {
        self.conn.query_row("SELECT value FROM meta WHERE key = ?1", params![lock_key(section)], |r| r.get(0)).optional().ok().flatten()
    }

    /// Whether section `section` is under a password and its key known.
    pub fn is_unlocked(&self, section: i64) -> bool {
        self.keys.contains_key(&section)
    }

    /// Unlocks section `section` with `password`: whether it was right.
    pub fn unlock(&mut self, section: i64, password: &str) -> bool {
        let Some(key) = self.lock_of(section).and_then(|l| note_crypt::try_lock(&l, password)) else { return false };
        self.keys.insert(section, key);
        true
    }

    /// Forgets the key of section `section`: locked again.
    pub fn forget(&mut self, section: i64) {
        self.keys.remove(&section);
    }

    /// Puts section `section` under `password`: its pages' texts and
    /// their versions are sealed. It stays unlocked.
    pub fn set_lock(&mut self, section: i64, password: &str) -> Result<(), NoteError> {
        if self.lock_of(section).is_some() {
            return Err(NoteError::Other("already locked".into()));
        }
        let (lock, key) = note_crypt::new_lock(password);
        self.conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)", params![lock_key(section), lock]).map_err(other)?;
        self.keys.insert(section, key);
        self.reseal()
    }

    /// Takes the password off section `section` (unlocked): its texts are
    /// in the clear again. Their plain text and tasks come back as each
    /// page is saved.
    pub fn remove_lock(&mut self, section: i64) -> Result<(), NoteError> {
        let key = self.keys.get(&section).cloned().ok_or(NoteError::NeedsPassword)?;
        self.conn.execute("DELETE FROM meta WHERE key = ?1", params![lock_key(section)]).map_err(other)?;
        self.keys.remove(&section);
        let ids: Vec<i64> = self.conn.prepare("SELECT id FROM pages WHERE section = ?1").map_err(other)?.query_map(params![section], |r| r.get(0)).map_err(other)?.collect::<Result<_, _>>().map_err(other)?;
        self.freed.extend(ids);
        self.reseal_with(&[key])
    }

    /// The key page `id` is written with: None in a section without a
    /// password.
    fn page_key(&self, id: i64) -> Result<Option<Key>, NoteError> {
        let section: Option<i64> = self.conn.query_row("SELECT section FROM pages WHERE id = ?1", params![id], |r| r.get(0)).optional().map_err(other)?;
        let Some(section) = section.filter(|s| self.lock_of(*s).is_some()) else { return Ok(None) };
        self.keys.get(&section).cloned().map(Some).ok_or(NoteError::NeedsPassword)
    }

    /// A body of page `page` as written, opened when sealed.
    fn unseal(&self, page: i64, body: Vec<u8>) -> Result<Vec<u8>, NoteError> {
        if !note_crypt::is_sealed(&body) {
            return Ok(body);
        }
        self.keys_for(page)?.into_iter().find_map(|k| note_crypt::open(k, &body)).ok_or(NoteError::NeedsPassword)
    }

    /// The keys page `page`'s sealed parts may be opened with: its
    /// section's first, then any other known (a page moved between
    /// sections under passwords). Outside those, none unless the user
    /// moved it there: what is opened is saved in the clear.
    fn keys_for(&self, page: i64) -> Result<Vec<&Key>, NoteError> {
        let section: Option<i64> = self.conn.query_row("SELECT section FROM pages WHERE id = ?1", params![page], |r| r.get(0)).optional().map_err(other)?;
        let locked = section.is_some_and(|s| self.lock_of(s).is_some());
        if !locked && !self.freed.contains(&page) {
            return Ok(Vec::new());
        }
        let first = section.and_then(|s| self.keys.get(&s));
        Ok(first.into_iter().chain(self.keys.values()).collect())
    }

    /// Seals the texts of pages in sections under a password, and opens
    /// those elsewhere, as far as the keys known allow.
    fn reseal(&mut self) -> Result<(), NoteError> {
        self.reseal_with(&[])
    }

    fn reseal_with(&mut self, extra: &[Key]) -> Result<(), NoteError> {
        let locks = self.locks();
        let rows: Vec<(i64, i64, bool)> = {
            let mut q = self.conn.prepare("SELECT id, section, substr(body, 1, 5) = CAST('APXE1' AS BLOB) FROM pages WHERE body IS NOT NULL AND length(body) > 0").map_err(other)?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(other)?.collect::<Result<_, _>>().map_err(other)?
        };
        let wrong: Vec<(i64, Option<Key>)> = rows
            .into_iter()
            .filter(|(_, s, sealed)| locks.contains(s) != *sealed)
            .filter_map(|(id, s, sealed)| match sealed {
                false => self.keys.get(&s).cloned().map(|k| (id, Some(k))),
                true => self.freed.contains(&id).then_some((id, None)),
            })
            .collect();
        let keys: Vec<Key> = self.keys.values().cloned().chain(extra.iter().cloned()).collect();
        let open = |b: Vec<u8>| -> Option<Vec<u8>> {
            if !note_crypt::is_sealed(&b) {
                return Some(b);
            }
            keys.iter().find_map(|k| note_crypt::open(k, &b))
        };
        let tx = self.conn.transaction().map_err(other)?;
        for (id, seal) in wrong {
            let mut bodies: Vec<(Option<i64>, Vec<u8>)> = vec![(None, tx.query_row("SELECT body FROM pages WHERE id = ?1", params![id], |r| r.get(0)).map_err(other)?)];
            let mut q = tx.prepare("SELECT id, body FROM history WHERE page = ?1").map_err(other)?;
            bodies.extend(q.query_map(params![id], |r| Ok((Some(r.get(0)?), r.get(1)?))).map_err(other)?.collect::<Result<Vec<_>, _>>().map_err(other)?);
            drop(q);
            for (hist, b) in bodies {
                // A key not known: it stays as it is.
                let Some(plain) = open(b) else { continue };
                let b = match &seal {
                    Some(k) => note_crypt::seal(k, &plain),
                    None => plain,
                };
                match hist {
                    Some(h) => tx.execute("UPDATE history SET body = ?2 WHERE id = ?1", params![h, b]),
                    None => tx.execute("UPDATE pages SET body = ?2 WHERE id = ?1", params![id, b]),
                }
                .map_err(other)?;
            }
            if seal.is_some() {
                tx.execute("UPDATE pages SET text = '' WHERE id = ?1", params![id]).map_err(other)?;
                tx.execute("DELETE FROM tasks WHERE page = ?1", params![id]).map_err(other)?;
            }
        }
        reseal_parts(&tx, &locks, &self.keys, &keys, &self.freed).map_err(other)?;
        tx.commit().map_err(other)
    }

    /// Writes page `id`'s text (as it is in `doc` and as plain `text`)
    /// and the pages it links to.
    pub fn save_page(&mut self, id: i64, doc: &Document, text: &str, links: &[(i64, Rel)], modified: i64) -> Result<(), NoteError> {
        let mut body = crate::docx::write(doc);
        // Under a password, only sealed: neither its text nor its tasks
        // are kept in the clear.
        let key = self.page_key(id)?;
        let text = if key.is_some() { "" } else { text };
        if let Some(k) = &key {
            body = note_crypt::seal(k, &body);
        }
        let tx = self.conn.transaction().map_err(other)?;
        keep_version(&tx, id, Some(modified)).map_err(other)?;
        let n = tx.execute("UPDATE pages SET body = ?2, text = ?3, modified = ?4 WHERE id = ?1", params![id, body, text, modified]).map_err(other)?;
        if n == 0 {
            return Err(NoteError::Other(format!("no page {id}")));
        }
        if key.is_some() {
            tx.execute("DELETE FROM tasks WHERE page = ?1", params![id]).map_err(other)?;
        } else {
            write_tasks(&tx, id, doc).map_err(other)?;
        }
        tx.execute("DELETE FROM links WHERE src = ?1", params![id]).map_err(other)?;
        for (to, rel) in links {
            tx.execute("INSERT INTO links(src, dst, rel) VALUES (?1, ?2, ?3)", params![id, to, rel.key()]).map_err(other)?;
        }
        tx.commit().map_err(other)
    }

    /// Page `id`'s earlier versions, newest first.
    pub fn versions(&self, id: i64) -> Result<Vec<Version>, NoteError> {
        let mut q = self.conn.prepare("SELECT id, at, length(body) FROM history WHERE page = ?1 ORDER BY at DESC, id DESC").map_err(other)?;
        let rows = q.query_map(params![id], |r| Ok(Version { id: r.get(0)?, at: r.get(1)?, size: r.get::<_, i64>(2)? as u64 })).map_err(other)?;
        rows.collect::<Result<_, _>>().map_err(other)
    }

    /// The text of version `id`.
    pub fn load_version(&self, id: i64) -> Result<Document, NoteError> {
        let (page, body): (i64, Vec<u8>) = self.conn.query_row("SELECT page, body FROM history WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(other)?;
        let body = self.unseal(page, body)?;
        crate::docx::read(&body).map_err(|e| NoteError::Other(e.to_string()))
    }

    /// Keeps page `id` as it is now among its versions (before it's
    /// replaced by an older one, say).
    pub fn keep_version(&mut self, id: i64) -> Result<(), NoteError> {
        keep_version(&self.conn, id, None).map_err(other)
    }

    /// Files made before tasks were kept: their tasks, found once.
    fn index_tasks(&self) -> Result<(), NoteError> {
        if self.setting(TASKS_KEPT).is_some() {
            return Ok(());
        }
        let ids: Vec<i64> = self.conn.prepare("SELECT id FROM pages WHERE text LIKE '%☐%' OR text LIKE '%☒%'").map_err(other)?.query_map([], |r| r.get(0)).map_err(other)?.collect::<Result<_, _>>().map_err(other)?;
        for id in ids {
            let doc = match self.load_page(id) {
                Ok(doc) => doc,
                // Sealed: its tasks aren't kept anyway.
                Err(NoteError::NeedsPassword) => continue,
                Err(e) => return Err(e),
            };
            write_tasks(&self.conn, id, &doc).map_err(other)?;
        }
        self.set_setting(TASKS_KEPT, "1")
    }

    /// Keeps a file in the notebook, attached from page `page`. Returns
    /// its id.
    /// Under a password, its name and contents are sealed.
    pub fn add_attachment(&mut self, name: &str, data: &[u8], page: Option<i64>, now: i64) -> Result<i64, NoteError> {
        let (name, data) = match page.map(|p| self.page_key(p)).transpose()?.flatten() {
            Some(k) => (seal_text(&k, name), note_crypt::seal(&k, data)),
            None => (name.to_string(), data.to_vec()),
        };
        self.conn.execute("INSERT INTO attachments(page, name, added, size, data) VALUES (?1, ?2, ?3, ?4, ?5)", params![page, name, now, data.len() as i64, data]).map_err(other)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The files kept in the notebook (without their contents), newest
    /// first; those of sections still locked are left out.
    pub fn attachments(&self) -> Result<Vec<Attachment>, NoteError> {
        let mut q = self.conn.prepare("SELECT id, page, name, added, size FROM attachments ORDER BY added DESC, id DESC").map_err(other)?;
        let rows = q.query_map([], |r| Ok(Attachment { id: r.get(0)?, page: r.get(1)?, name: r.get(2)?, added: r.get(3)?, size: r.get::<_, i64>(4)? as u64 })).map_err(other)?;
        let all: Vec<Attachment> = rows.collect::<Result<_, _>>().map_err(other)?;
        Ok(all.into_iter().filter_map(|mut a| {
            a.name = open_text(self.keys.values(), &a.name)?;
            Some(a)
        }).collect())
    }

    /// The contents of attachment `id`.
    pub fn attachment_data(&self, id: i64) -> Result<Vec<u8>, NoteError> {
        let data: Vec<u8> = self.conn.query_row("SELECT data FROM attachments WHERE id = ?1", params![id], |r| r.get(0)).map_err(other)?;
        if !note_crypt::is_sealed(&data) {
            return Ok(data);
        }
        self.keys.values().find_map(|k| note_crypt::open(k, &data)).ok_or(NoteError::NeedsPassword)
    }

    pub fn delete_attachment(&mut self, id: i64) -> Result<(), NoteError> {
        self.conn.execute("DELETE FROM attachments WHERE id = ?1", params![id]).map(|_| ()).map_err(other)
    }

    /// Page `id`'s text (an empty document when it has none yet).
    pub fn load_page(&self, id: i64) -> Result<Document, NoteError> {
        let body: Option<Vec<u8>> = self.conn.query_row("SELECT body FROM pages WHERE id = ?1", params![id], |r| r.get(0)).optional().map_err(other)?.flatten();
        match body {
            Some(b) if !b.is_empty() => crate::docx::read(&self.unseal(id, b)?).map_err(|e| NoteError::Other(e.to_string())),
            // In the font it will be saved with, from the first letter.
            _ => {
                let mut d = Document::from_paragraphs(Vec::new());
                d.default_font = Some("Calibri".into());
                Ok(d)
            }
        }
    }

    /// Canvas page `id` (empty when it has nothing yet).
    pub fn load_canvas(&self, id: i64) -> Result<Canvas, NoteError> {
        type Row = (i64, String, [f32; 4], u32, Option<i64>, String, Option<Vec<u8>>);
        let mut c = Canvas::default();
        let mut q = self.conn.prepare("SELECT id, kind, x, y, w, h, color, ref, text, data FROM canvas_items WHERE page = ?1 ORDER BY pos").map_err(other)?;
        let rows: Vec<Row> = q
            .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, [r.get::<_, f64>(2)? as f32, r.get::<_, f64>(3)? as f32, r.get::<_, f64>(4)? as f32, r.get::<_, f64>(5)? as f32], r.get::<_, i64>(6)? as u32, r.get(7)?, r.get(8)?, r.get(9)?)))
            .map_err(other)?
            .collect::<Result<_, _>>()
            .map_err(other)?;
        for (item, kind, rect, color, page, mut text, mut data) in rows {
            // Under a password, what it holds is sealed in its data.
            if let Some(d) = data.take() {
                let d = self.unseal(id, d)?;
                match kind.as_str() {
                    "note" | "group" => text = String::from_utf8_lossy(&d).into_owned(),
                    _ => data = Some(d),
                }
            }
            let kind = match kind.as_str() {
                "page" => ItemKind::Page(page.unwrap_or_default()),
                "picture" => ItemKind::Picture(data.unwrap_or_default().into()),
                "group" => ItemKind::Group(text),
                "map" => ItemKind::Map(data.unwrap_or_default().into()),
                "pin" => ItemKind::Pin(page.unwrap_or_default()),
                _ => ItemKind::Note(text),
            };
            c.items.push(Item { id: item, kind, rect, color });
        }
        let mut q = self.conn.prepare("SELECT id, src, dst, label, rel FROM canvas_arrows WHERE page = ?1 ORDER BY rowid").map_err(other)?;
        let rows = q.query_map(params![id], |r| Ok(Arrow { id: r.get(0)?, from: r.get(1)?, to: r.get(2)?, label: r.get(3)?, rel: Rel::from_key(&r.get::<_, String>(4)?) })).map_err(other)?;
        for row in rows {
            let mut a = row.map_err(other)?;
            a.label = open_text(self.keys_for(id)?.into_iter(), &a.label).ok_or(NoteError::NeedsPassword)?;
            c.arrows.push(a);
        }
        Ok(c)
    }

    /// Writes canvas page `id` as `c` is, with its words to search.
    /// Under a password, what its items say and hold is sealed.
    pub fn save_canvas(&mut self, id: i64, c: &Canvas, modified: i64) -> Result<(), NoteError> {
        let key = self.page_key(id)?;
        let tx = self.conn.transaction().map_err(other)?;
        tx.execute("DELETE FROM canvas_items WHERE page = ?1", params![id]).map_err(other)?;
        tx.execute("DELETE FROM canvas_arrows WHERE page = ?1", params![id]).map_err(other)?;
        {
            let mut ins = tx.prepare("INSERT INTO canvas_items(page, id, pos, kind, x, y, w, h, color, ref, text, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)").map_err(other)?;
            for (n, i) in c.items.iter().enumerate() {
                let (kind, page, text, data): (&str, Option<i64>, &str, Option<&[u8]>) = match &i.kind {
                    ItemKind::Page(p) => ("page", Some(*p), "", None),
                    ItemKind::Note(t) => ("note", None, t, None),
                    ItemKind::Picture(d) => ("picture", None, "", Some(d)),
                    ItemKind::Group(t) => ("group", None, t, None),
                    ItemKind::Map(d) => ("map", None, "", Some(d)),
                    ItemKind::Pin(p) => ("pin", Some(*p), "", None),
                };
                let [x, y, w, h] = i.rect;
                let sealed = key.as_ref().filter(|_| !text.is_empty() || data.is_some()).map(|k| note_crypt::seal(k, data.unwrap_or(text.as_bytes())));
                let (text, data) = match &sealed {
                    Some(b) => ("", Some(b.as_slice())),
                    None => (text, data),
                };
                ins.execute(params![id, i.id, n as i64, kind, x as f64, y as f64, w as f64, h as f64, i.color as i64, page, text, data]).map_err(other)?;
            }
            let mut ins = tx.prepare("INSERT INTO canvas_arrows(page, id, src, dst, label, rel) VALUES (?1, ?2, ?3, ?4, ?5, ?6)").map_err(other)?;
            for a in &c.arrows {
                let label = match &key {
                    Some(k) => seal_text(k, &a.label),
                    None => a.label.clone(),
                };
                ins.execute(params![id, a.id, a.from, a.to, label, a.rel.key()]).map_err(other)?;
            }
        }
        let text = if key.is_some() { String::new() } else { c.text() };
        tx.execute("UPDATE pages SET text = ?2, modified = ?3 WHERE id = ?1", params![id, text, modified]).map_err(other)?;
        tx.commit().map_err(other)
    }

    /// A setting kept in the notebook (how it is shown).
    pub fn setting(&self, key: &str) -> Option<String> {
        self.conn.query_row("SELECT value FROM meta WHERE key = ?1", params![format!("ui.{key}")], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), NoteError> {
        self.conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)", params![format!("ui.{key}"), value]).map(|_| ()).map_err(other)
    }

    /// A copy of the whole notebook at `path` (Save As).
    pub fn copy_to(&self, path: &Path) -> Result<(), NoteError> {
        apex_db::sqlite::write_new(path, |tmp| self.conn.execute("VACUUM INTO ?1", params![tmp.to_string_lossy()]).map(|_| ()).map_err(std::io::Error::other))
            .map_err(|e| NoteError::Other(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notebook_bringing_triggers_is_refused() {
        let dir = apex_fs::temp_dir().join(format!("apexnote-trigger-test-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.apexnote");
        drop(NoteStore::create(&path).unwrap());
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE IF NOT EXISTS spy(x); CREATE TRIGGER t AFTER INSERT ON meta BEGIN INSERT INTO spy VALUES (new.value); END;").unwrap();
        drop(c);
        assert_eq!(NoteStore::open(&path).err(), Some(NoteError::NotNotebook));
        // Nor an index or check of its own.
        for extra in ["CREATE INDEX x ON pages(abs(id))", "CREATE TABLE c(x CHECK (x > 0))", "CREATE TABLE d(x DEFAULT (random()))"] {
            drop(NoteStore::create(&path).unwrap());
            Connection::open(&path).unwrap().execute_batch(extra).unwrap();
            assert_eq!(NoteStore::open(&path).err(), Some(NoteError::NotNotebook), "{extra}");
        }
        drop(NoteStore::create(&path).unwrap());
        assert!(NoteStore::open(&path).is_ok());
        let _ = apex_fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_notebook_keeps_its_pages_and_is_held_while_open() {
        let dir = apex_fs::temp_dir().join(format!("apexnote-test-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.apexnote");
        let mut s = NoteStore::create(&path).unwrap();
        let mut nb = Notebook::default();
        let sec = nb.add_section("Ideas", 0);
        let a = nb.add_page(sec, None, None, "Trama", 5);
        let b = nb.add_page(sec, Some(a), None, "Detalle", 5);
        s.save_structure(&nb).unwrap();
        let mut doc = Document::from_paragraphs(vec![apex_doc::Paragraph::new("Ana llega al puerto.")]);
        doc.paragraphs[0].style.kind = apex_doc::ParaKind::Heading(1);
        s.save_page(a, &doc, "Ana llega al puerto.", &[(b, Rel::Supports)], 9).unwrap();
        // Someone else can't open it meanwhile.
        assert_eq!(NoteStore::open(&path).err(), Some(NoteError::Locked));
        nb.delete_page(b, 7);
        nb.page_mut(a).unwrap().title = "La trama".into();
        nb.page_mut(a).unwrap().modified = 9;
        nb.page_mut(a).unwrap().aliases = vec!["Argumento".into()];
        s.save_structure(&nb).unwrap();
        let copy = dir.join("copia.apexnote");
        s.copy_to(&copy).unwrap();
        drop(s);
        let s = NoteStore::open(&path).unwrap();
        let back = s.load().unwrap();
        assert_eq!(back.page(a).unwrap().title, "La trama");
        assert_eq!(back.page(a).unwrap().modified, 9);
        assert_eq!(back.page(b).unwrap().deleted, Some(7));
        assert_eq!(back.texts[&a], "Ana llega al puerto.");
        assert_eq!(back.page(a).unwrap().aliases, ["Argumento"]);
        assert_eq!(back.links[&a], [(b, Rel::Supports)]);
        let d = s.load_page(a).unwrap();
        assert_eq!((d.paragraphs[0].text.as_str(), d.paragraphs[0].style.kind), ("Ana llega al puerto.", apex_doc::ParaKind::Heading(1)));
        assert_eq!(s.load_page(b).unwrap().paragraphs.len(), 1);
        drop(s);
        assert_eq!(NoteStore::open(&copy).unwrap().load().unwrap().pages.len(), 2);
        // Purged pages go from the file.
        let mut s = NoteStore::open(&path).unwrap();
        let mut nb = s.load().unwrap();
        nb.purge(0, Some(apex_doc::notebook::Trashed::Page(b)));
        s.save_structure(&nb).unwrap();
        assert_eq!(s.load().unwrap().pages.len(), 1);
        drop(s);
        // A canvas, its arrows making links.
        let mut s = NoteStore::open(&path).unwrap();
        let mut nb = s.load().unwrap();
        let sec = nb.sections[0].id;
        let board = nb.add_page(sec, None, None, "Lienzo", 1);
        nb.page_mut(board).unwrap().kind = PageKind::Canvas;
        let src = Source { kind: SourceKind::Web, authors: "Doe, Ann".into(), title: "Faros".into(), url: "https://faros.example".into(), ..Default::default() };
        nb.page_mut(a).unwrap().source = Some(src.clone());
        let claim = Claim { kind: apex_doc::notebook::ClaimKind::Hypothesis, status: apex_doc::notebook::Status::Refuted };
        nb.page_mut(board).unwrap().claim = Some(claim);
        let ty = nb.add_type("Personaje");
        nb.page_type_mut(ty).unwrap().fields = vec![apex_doc::notebook::Field::new("Edad", apex_doc::notebook::FieldKind::Number)];
        nb.page_type_mut(ty).unwrap().template = Some(board);
        nb.page_mut(a).unwrap().ptype = Some(ty);
        nb.set_field(a, "Edad", "34");
        nb.save_query(&Query { ptype: Some(ty), tag: "faro".into(), ..Query::default() }, "Con faro");
        s.save_structure(&nb).unwrap();
        let mut c = Canvas::default();
        let x = c.add(ItemKind::Page(a), [0.0, 0.0, 100.0, 60.0], 0);
        let y = c.add(ItemKind::Picture(vec![1u8, 2, 3].into()), [200.0, 0.0, 100.0, 60.0], 0);
        let z = c.add(ItemKind::Page(board), [0.0, 200.0, 100.0, 60.0], 0);
        c.add(ItemKind::Group("Zona".into()), [-10.0, -10.0, 400.0, 400.0], 7);
        c.set_map(vec![9u8, 8].into(), [-50.0, -50.0, 900.0, 700.0]);
        c.add(ItemKind::Pin(a), [300.0, 300.0, 20.0, 40.0], 0);
        c.connect(x, z, Rel::Causes);
        c.connect(x, y, Rel::Link);
        c.arrows[0].label = "lleva a".into();
        s.save_canvas(board, &c, 3).unwrap();
        drop(s);
        let s = NoteStore::open(&path).unwrap();
        let back = s.load().unwrap();
        assert_eq!(back.page(board).unwrap().kind, PageKind::Canvas);
        assert_eq!(back.page(a).unwrap().source.as_ref(), Some(&src));
        assert_eq!(back.page(board).unwrap().source, None);
        assert_eq!((back.page(board).unwrap().claim, back.page(a).unwrap().claim), (Some(claim), None));
        assert_eq!(back.types, nb.types);
        assert_eq!(back.queries, nb.queries);
        assert_eq!((back.page(a).unwrap().ptype, back.field_value(a, "Edad")), (Some(ty), "34"));
        assert_eq!(s.load_canvas(board).unwrap(), c);
        // A page's check boxes are its tasks.
        let mut doc = Document::from_plain_text("Llamar al farero\nNada\n");
        let mut box_ = apex_doc::ContentControl::new(1, apex_doc::ControlKind::CheckBox { checked: true });
        box_.tag = "due=2026-10-05;pri=3".into();
        doc.insert_control(apex_doc::Selection::caret(apex_doc::Pos::new(0, 0)), box_, &apex_doc::CHECKED.to_string());
        drop(s);
        let mut s = NoteStore::open(&path).unwrap();
        s.save_page(a, &doc, "", &[], 4).unwrap();
        let t = &s.load().unwrap().tasks[&a];
        assert_eq!((t.len(), t[0].para, t[0].done, t[0].text.as_str()), (1, 0, true, "Llamar al farero"));
        assert_eq!(t[0].meta, TaskMeta { due: Some("2026-10-05".into()), priority: 3, col: None });
        // Files kept inside.
        let f = s.add_attachment("plano.pdf", b"%PDF-1.4", Some(a), 5).unwrap();
        let list = s.attachments().unwrap();
        assert_eq!((list.len(), list[0].name.as_str(), list[0].size, list[0].page), (1, "plano.pdf", 8, Some(a)));
        assert_eq!(s.attachment_data(f).unwrap(), b"%PDF-1.4");
        s.delete_attachment(f).unwrap();
        assert!(s.attachments().unwrap().is_empty());
        assert_eq!(back.canvas_links[&board], [(a, board, Rel::Causes), (board, a, Rel::Link)]);
        assert_eq!(back.texts[&board], "Zona\nlleva a");
        drop(s);
        // Not a notebook.
        let other = dir.join("x.apexnote");
        apex_fs::write(&other, b"hola").unwrap();
        assert!(NoteStore::open(&other).is_err());
        apex_fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_page_keeps_its_earlier_texts() {
        let dir = apex_fs::temp_dir().join(format!("apexnote-versions-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let mut s = NoteStore::create(&dir.join("c.apexnote")).unwrap();
        let mut nb = Notebook::default();
        let sec = nb.add_section("S", 0);
        let p = nb.add_page(sec, None, None, "P", 0);
        s.save_structure(&nb).unwrap();
        let save = |s: &mut NoteStore, text: &str, at: i64| s.save_page(p, &Document::from_plain_text(text), text, &[], at).unwrap();
        save(&mut s, "uno", 1000);
        assert!(s.versions(p).unwrap().is_empty(), "nothing before the first text");
        save(&mut s, "dos", 1010);
        // Changes close together are one version.
        save(&mut s, "tres", 1020);
        save(&mut s, "cuatro", 1000 + VERSION_GAP + 30);
        let v = s.versions(p).unwrap();
        assert_eq!(v.iter().map(|v| v.at).collect::<Vec<_>>(), [1020, 1000]);
        let text = |d: Document| d.paragraphs[0].text.clone();
        assert_eq!(text(s.load_version(v[0].id).unwrap()), "tres");
        assert_eq!(text(s.load_version(v[1].id).unwrap()), "uno");
        s.keep_version(p).unwrap();
        s.keep_version(p).unwrap();
        assert_eq!(s.versions(p).unwrap().len(), 3, "kept once");
        drop(s);
        apex_fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_section_under_a_password_keeps_its_pages_sealed() {
        let dir = apex_fs::temp_dir().join(format!("apexnote-lock-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.apexnote");
        let mut s = NoteStore::create(&path).unwrap();
        let mut nb = Notebook::default();
        let sec = nb.add_section("Diario", 0);
        let other = nb.add_section("Ideas", 0);
        let p = nb.add_page(sec, None, None, "Lunes", 0);
        let q = nb.add_page(other, None, None, "Libre", 0);
        s.save_structure(&nb).unwrap();
        let doc = |t: &str| Document::from_plain_text(t);
        s.save_page(p, &doc("Secreto del faro ☐ llamar"), "Secreto del faro ☐ llamar", &[], 1000).unwrap();
        s.save_page(p, &doc("Secreto segundo"), "Secreto segundo", &[], 1000 + VERSION_GAP + 1).unwrap();
        s.save_page(q, &doc("A la vista"), "A la vista", &[], 1000).unwrap();
        // Its canvas and attachments too.
        let mut c = Canvas::default();
        c.items.push(Item { id: 1, kind: ItemKind::Note("Secreto nota".into()), rect: [0.0; 4], color: 0 });
        c.items.push(Item { id: 2, kind: ItemKind::Picture(b"Secreto foto".to_vec().into()), rect: [0.0; 4], color: 0 });
        c.arrows.push(Arrow { id: 3, from: 1, to: 2, label: "Secreto flecha".into(), rel: Rel::Link });
        s.save_canvas(p, &c, 1000).unwrap();
        let file = s.add_attachment("Secreto.txt", b"Secreto adjunto", Some(p), 1000).unwrap();
        s.set_lock(sec, "faro").unwrap();
        let raw = || apex_fs::read(&path).unwrap();
        assert!(!raw().windows(7).any(|w| w == b"Secreto"), "neither the text, its versions, canvas nor attachments in the clear");
        assert_eq!(s.load_page(p).unwrap().paragraphs[0].text, "Secreto segundo");
        assert_eq!(s.load_canvas(p).unwrap(), c);
        assert_eq!(s.attachments().unwrap()[0].name, "Secreto.txt");
        assert_eq!(s.attachment_data(file).unwrap(), b"Secreto adjunto");
        s.save_canvas(p, &c, 2000).unwrap();
        s.add_attachment("Secreto2.txt", b"Secreto otro", Some(p), 2000).unwrap();
        assert!(!raw().windows(7).any(|w| w == b"Secreto"), "nor what is written while unlocked");
        // Locked again: nothing to read until the password is given.
        s.forget(sec);
        assert!(s.attachments().unwrap().is_empty());
        assert!(matches!(s.attachment_data(file), Err(NoteError::NeedsPassword)));
        assert!(matches!(s.load_canvas(p), Err(NoteError::NeedsPassword)));
        assert!(matches!(s.load_page(p), Err(NoteError::NeedsPassword)));
        assert!(matches!(s.save_page(p, &doc("x"), "x", &[], 5000), Err(NoteError::NeedsPassword)));
        assert_eq!(s.load_page(q).unwrap().paragraphs[0].text, "A la vista");
        assert!(!s.unlock(sec, "Faro"));
        assert!(s.unlock(sec, "faro"));
        let v = s.versions(p).unwrap();
        assert_eq!(s.load_version(v[0].id).unwrap().paragraphs[0].text, "Secreto del faro ☐ llamar");
        let back = s.load().unwrap();
        assert_eq!((back.texts[&p].as_str(), back.tasks.get(&p).map(Vec::len)), ("", None));
        assert_eq!(s.locks(), HashSet::from([sec]));
        // A page moved in is sealed; one moved out, opened.
        nb.page_mut(q).unwrap().section = sec;
        nb.page_mut(p).unwrap().section = other;
        s.save_structure(&nb).unwrap();
        s.forget(sec);
        assert!(matches!(s.load_page(q), Err(NoteError::NeedsPassword)));
        assert_eq!(s.load_page(p).unwrap().paragraphs[0].text, "Secreto segundo");
        assert!(s.unlock(sec, "faro"));
        s.remove_lock(sec).unwrap();
        s.forget(sec);
        assert!(s.locks().is_empty());
        assert_eq!(s.load_canvas(p).unwrap(), c);
        assert_eq!(s.attachment_data(file).unwrap(), b"Secreto adjunto");
        assert!(raw().windows(15).any(|w| w == b"Secreto adjunto"));
        assert_eq!(s.load_page(q).unwrap().paragraphs[0].text, "A la vista");
        drop(s);
        apex_fs::remove_dir_all(&dir).unwrap();
    }
}
