//! Access databases: our own reader of the page format of Jet 3 (Access
//! 97, `.mdb`), Jet 4 (Access 2000-2003, `.mdb`) and ACE (Access 2007 and
//! later, `.accdb`).
//!
//! The file is a run of pages (2 KB in Jet 3, 4 KB later). Page 0 is the
//! header (partly masked with a fixed RC4 key); tables are described by
//! table definition pages ([`tdef`]) and their rows live on data pages
//! ([`rows`]). The catalog is itself a table, `MSysObjects`, whose
//! definition is always on page 2; relations and the saved queries' parts
//! are the tables `MSysRelationships` and `MSysQueries` ([`queries`]).
//!
//! "Encoded" Jet databases (every page RC4-masked with a key in the
//! header) are read; databases encrypted with a password by Access 2007
//! and later (AES) are reported as [`Error::Encrypted`].

mod props;
mod queries;
mod rows;
mod tdef;

use std::collections::HashMap;
use std::path::Path;

use crate::model::{ColType, Database, Relation, Table, Value};
use crate::Error;

pub use tdef::TableDef;

/// The version of the page format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Jet {
    /// Access 97.
    Jet3,
    /// Access 2000, 2002 and 2003.
    Jet4,
    /// Access 2007 and later (the header's version number).
    Ace(u8),
}

impl Jet {
    pub fn is_jet3(self) -> bool {
        self == Jet::Jet3
    }

    /// The Access version that writes it.
    pub fn name(self) -> &'static str {
        match self {
            Jet::Jet3 => "Access 97",
            Jet::Jet4 => "Access 2000-2003",
            Jet::Ace(2) => "Access 2007",
            Jet::Ace(3) => "Access 2010",
            Jet::Ace(_) => "Access 2013+",
        }
    }
}

/// An object of the catalog (`MSysObjects`).
#[derive(Clone, Debug, Default)]
pub struct Object {
    pub id: i32,
    pub parent: i32,
    pub name: String,
    pub ty: i32,
    pub flags: i32,
    /// Linked tables: the file (or connection) and the table's name there.
    pub database: String,
    pub connect: String,
    pub foreign_name: String,
    /// Its properties, as stored (see [`props`]).
    pub props: Vec<u8>,
}

/// Catalog types.
const TYPE_TABLE: i32 = 1;
const TYPE_QUERY: i32 = 5;
const TYPE_LINKED_ODBC: i32 = 4;
const TYPE_LINKED: i32 = 6;
const SYSTEM_FLAGS: i32 = 0x8000_0002u32 as i32;

pub struct AccessFile {
    data: Vec<u8>,
    pub jet: Jet,
    page_size: usize,
    /// Code page of Jet 3 text.
    pub codepage: u16,
    catalog: Vec<Object>,
    /// Whether the file had a database password (Jet 3 and 4 only store it,
    /// the data is not encrypted by it).
    pub has_password: bool,
    /// What memo and OLE values may still add up to: their lengths come
    /// from the file, and many cells may share one chain of rows.
    long_left: std::sync::atomic::AtomicUsize,
    /// The cells all tables' rows may still add up to.
    pub(super) cells_left: std::sync::atomic::AtomicUsize,
    /// The bytes rows' values may still copy: rows and values may overlap.
    pub(super) inline_left: std::sync::atomic::AtomicUsize,
}

/// The header's RC4 key and how many bytes from 0x18 it masks.
const HEADER_KEY: [u8; 4] = [0xc7, 0xda, 0x39, 0x6b];

impl AccessFile {
    pub fn open(path: &Path) -> Result<AccessFile, Error> {
        let data = crate::read_file(path)?;
        AccessFile::from_bytes(data)
    }

    pub fn from_bytes(mut data: Vec<u8>) -> Result<AccessFile, Error> {
        if data.len() < 2048 || data[0..4] != [0, 1, 0, 0] || !(data[4..19] == *b"Standard Jet DB" || data[4..19] == *b"Standard ACE DB") {
            return Err(Error::Format("not an Access database".into()));
        }
        let jet = match data[0x14] {
            0 => Jet::Jet3,
            1 => Jet::Jet4,
            v => Jet::Ace(v),
        };
        let page_size = if jet.is_jet3() { 2048 } else { 4096 };
        let masked = if jet.is_jet3() { 126 } else { 128 };
        let mut header = data[0x18..0x18 + masked].to_vec();
        rc4(&HEADER_KEY, &mut header);
        let at = |o: usize| header[o - 0x18];
        let codepage = u16::from_le_bytes([at(0x3c), at(0x3d)]);
        let key = [at(0x3e), at(0x3f), at(0x40), at(0x41)];
        let pw_len = if jet.is_jet3() { 20 } else { 40 };
        let has_password = (0x42..0x42 + pw_len).any(|o| o - 0x18 < header.len() && at(o) != 0);
        // Encoded Jet databases: every page after the header masked with
        // the key and the page's number.
        if key != [0; 4] && matches!(jet, Jet::Jet3 | Jet::Jet4) {
            for (n, page) in data.chunks_mut(page_size).enumerate().skip(1) {
                let k = (u32::from_le_bytes(key) ^ n as u32).to_le_bytes();
                rc4(&k, page);
            }
        }
        // Each byte copied about once in a sound file.
        let inline = data.len().saturating_mul(2).saturating_add(64 << 20);
        let mut f = AccessFile { data, jet, page_size, codepage: if codepage == 0 { 1252 } else { codepage }, catalog: Vec::new(), has_password, long_left: (1usize << 30).into(), cells_left: (64usize << 20).into(), inline_left: inline.into() };
        // The catalog's definition is on page 2: anything else there means
        // the pages are encrypted (Access 2007+ passwords) or damaged.
        if f.page(2).first() != Some(&0x02) {
            return Err(if matches!(jet, Jet::Ace(_)) { Error::Encrypted } else { Error::Format("damaged Access database".into()) });
        }
        f.catalog = f.read_catalog()?;
        Ok(f)
    }

    /// Page `n`, or an empty slice past the end.
    fn page(&self, n: u32) -> &[u8] {
        let a = n as usize * self.page_size;
        self.data.get(a..a + self.page_size).unwrap_or(&[])
    }

    fn pages(&self) -> u32 {
        (self.data.len() / self.page_size) as u32
    }

    fn read_catalog(&self) -> Result<Vec<Object>, Error> {
        let def = self.tdef(2, "MSysObjects")?;
        let rows = self.rows(&def);
        let col = |n: &str| def.columns.iter().position(|c| c.col.name.eq_ignore_ascii_case(n));
        let (id, parent, name, ty, flags) = (col("Id"), col("ParentId"), col("Name"), col("Type"), col("Flags"));
        let (database, connect, foreign, lvprop) = (col("Database"), col("Connect"), col("ForeignName"), col("LvProp"));
        let int = |r: &[Value], c: Option<usize>| c.and_then(|c| r.get(c)).and_then(Value::as_f64).unwrap_or(0.0) as i64 as i32;
        let text = |r: &[Value], c: Option<usize>| c.and_then(|c| r.get(c)).map(Value::to_text).unwrap_or_default();
        Ok(rows
            .iter()
            .map(|r| Object {
                id: int(r, id),
                parent: int(r, parent),
                name: text(r, name),
                ty: int(r, ty),
                flags: int(r, flags),
                database: text(r, database),
                connect: text(r, connect),
                foreign_name: text(r, foreign),
                props: match lvprop.and_then(|c| r.get(c)) {
                    Some(Value::Blob(b)) => b.clone(),
                    _ => Vec::new(),
                },
            })
            .collect())
    }

    /// The table definition on page `page`.
    fn tdef(&self, page: u32, name: &str) -> Result<TableDef, Error> {
        self.tdef_within(page, name, &mut 10000)
    }

    /// [`Self::tdef`], reading at most `left` pages (the definitions of a
    /// file's tables may share the tail of one long chain).
    fn tdef_within(&self, page: u32, name: &str, left: &mut usize) -> Result<TableDef, Error> {
        let mut buf = Vec::new();
        let mut pg = page;
        let mut seen = std::collections::HashSet::new();
        // A chain that comes back to a page ends there.
        while pg != 0 && seen.len() < 10000 && *left > 0 && seen.insert(pg) {
            *left -= 1;
            let p = self.page(pg);
            if p.first() != Some(&0x02) {
                return Err(Error::Format(format!("{name}: bad table definition page {pg}")));
            }
            buf.extend_from_slice(if buf.is_empty() { p } else { &p[8..] });
            pg = u32::from_le_bytes([p[4], p[5], p[6], p[7]]);
        }
        tdef::parse(&buf, name, page, self)
    }

    /// Every object of the catalog.
    pub fn catalog(&self) -> &[Object] {
        &self.catalog
    }

    fn object(&self, name: &str) -> Option<&Object> {
        self.catalog.iter().find(|o| o.name.eq_ignore_ascii_case(name) && o.ty == TYPE_TABLE)
    }

    /// The user's tables (not the system ones), as they are defined.
    pub fn tables(&self) -> Vec<TableDef> {
        let mut out = Vec::new();
        // Each definition once, however many entries name it.
        let mut seen = std::collections::HashSet::new();
        // Pages of definitions read in all: about the file's, twice.
        let mut left = 2 * self.pages() as usize + 10000;
        for o in &self.catalog {
            if o.ty != TYPE_TABLE || o.flags & SYSTEM_FLAGS != 0 || o.name.starts_with("MSys") || o.name.starts_with("~") {
                continue;
            }
            if !seen.insert(o.id & 0x00ff_ffff) {
                continue;
            }
            if let Ok(mut t) = self.tdef_within((o.id & 0x00ff_ffff) as u32, &o.name, &mut left) {
                self.apply_props(&mut t, &o.props);
                out.push(t);
            }
        }
        out
    }

    /// Sets what the columns' properties say: the result type of
    /// calculated columns, required, default values and validation.
    fn apply_props(&self, t: &mut TableDef, raw: &[u8]) {
        let jet3 = self.jet.is_jet3();
        let map = props::parse(raw, jet3, self.codepage);
        for c in &mut t.columns {
            let Some(p) = map.get(&c.col.name) else { continue };
            let text = |n: &str| p.get(n).map(|v| props::text(v, jet3, self.codepage)).unwrap_or_default();
            if c.calculated {
                if let Some(rt) = p.get("ResultType").and_then(|v| v.data.first()) {
                    c.result = *rt;
                    c.col.ty = tdef::col_type(*rt);
                }
                c.col.expression = text("Expression");
            }
            if p.get("Required").and_then(|v| v.data.first()).is_some_and(|b| *b != 0) {
                c.col.required = true;
            }
            c.col.default = text("DefaultValue");
            c.col.validation = text("ValidationRule");
            c.col.validation_text = text("ValidationText");
            c.col.description = text("Description");
        }
    }

    /// Linked tables: name, where they are and their name there.
    pub fn linked(&self) -> Vec<(String, String, String)> {
        self.catalog
            .iter()
            .filter(|o| (o.ty == TYPE_LINKED || o.ty == TYPE_LINKED_ODBC) && o.flags & SYSTEM_FLAGS == 0)
            .map(|o| (o.name.clone(), if o.database.is_empty() { o.connect.clone() } else { o.database.clone() }, o.foreign_name.clone()))
            .collect()
    }

    /// A table's rows as values (attachments and multi-value fields filled
    /// in from their hidden tables).
    pub fn read_table(&self, def: &TableDef) -> Result<Table, Error> {
        let mut rows = self.rows(def);
        for (i, c) in def.columns.iter().enumerate() {
            if c.raw_type == tdef::T_COMPLEX {
                let values = self.complex_values(def, &c.col.name);
                for r in &mut rows {
                    let id = r[i].as_f64().map(|v| v as i32);
                    // Rows sharing an id get copies: charged like long values.
                    r[i] = match id.and_then(|id| values.get(&id)) {
                        Some(v) if self.charge(match v {
                            Value::Attachments(l) => l.iter().map(|a| a.data.len() + a.name.len()).sum(),
                            Value::List(l) => l.len() * 32,
                            _ => 0,
                        }) =>
                        {
                            v.clone()
                        }
                        _ => Value::Null,
                    };
                }
            }
        }
        let mut t = Table::new(def.name.clone(), def.columns.iter().map(|c| c.col.clone()).collect());
        t.rows = rows;
        t.indexes = def.indexes.clone();
        Ok(t)
    }

    /// The values of complex column `col` of table `def` by complex id,
    /// read from its hidden flat table.
    fn complex_values(&self, def: &TableDef, col: &str) -> HashMap<i32, Value> {
        let mut out = HashMap::new();
        let Some(table_id) = self.catalog.iter().find(|o| o.ty == TYPE_TABLE && (o.id & 0x00ff_ffff) as u32 == def.page).map(|o| o.id) else { return out };
        let Some(cc) = self.object("MSysComplexColumns").and_then(|o| self.tdef((o.id & 0x00ff_ffff) as u32, "MSysComplexColumns").ok()) else { return out };
        let ccrows = self.rows(&cc);
        let ci = |n: &str| cc.columns.iter().position(|c| c.col.name.eq_ignore_ascii_case(n));
        let (Some(cname), Some(ctable), Some(cflat)) = (ci("ColumnName"), ci("ConceptualTableID"), ci("FlatTableID")) else { return out };
        let Some(flat_id) = ccrows.iter().find(|r| r[cname].to_text().eq_ignore_ascii_case(col) && r[ctable].as_f64() == Some(table_id as f64)).and_then(|r| r[cflat].as_f64()) else { return out };
        let Ok(flat) = self.tdef((flat_id as i64 as u32) & 0x00ff_ffff, "flat") else { return out };
        let frows = self.rows(&flat);
        let fi = |n: &str| flat.columns.iter().position(|c| c.col.name.eq_ignore_ascii_case(n));
        // The column pointing back to the row: the flat table's own
        // number ("_<column>") or, failing that, the first Long column
        // other than the known ones.
        let known = ["FileData", "FileFlags", "FileName", "FileTimeStamp", "FileType", "FileURL", "value"];
        let key = flat
            .columns
            .iter()
            .position(|c| c.col.ty == ColType::Int32 && !known.iter().any(|k| k.eq_ignore_ascii_case(&c.col.name)) && !c.col.auto)
            .or_else(|| flat.columns.iter().position(|c| c.col.ty == ColType::Int32 && !c.col.auto));
        let Some(key) = key else { return out };
        let attach = fi("FileData").zip(fi("FileName"));
        let value = fi("value");
        for r in &frows {
            let Some(id) = r[key].as_f64().map(|v| v as i32) else { continue };
            let entry = out.entry(id).or_insert_with(|| if attach.is_some() { Value::Attachments(Vec::new()) } else { Value::List(Vec::new()) });
            match (attach, entry) {
                (Some((data, name)), Value::Attachments(list)) => {
                    let bytes = match &r[data] {
                        Value::Blob(b) => self.attachment_data(b),
                        _ => Vec::new(),
                    };
                    list.push(crate::model::Attachment { name: r[name].to_text(), data: bytes });
                }
                (None, Value::List(list)) => {
                    if let Some(v) = value.and_then(|v| r.get(v)) {
                        list.push(v.clone());
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The relations of `MSysRelationships`.
    pub fn relations(&self) -> Vec<Relation> {
        let Some(o) = self.object("MSysRelationships") else { return Vec::new() };
        let Ok(def) = self.tdef((o.id & 0x00ff_ffff) as u32, "MSysRelationships") else { return Vec::new() };
        let rows = self.rows(&def);
        let ci = |n: &str| def.columns.iter().position(|c| c.col.name.eq_ignore_ascii_case(n));
        let (Some(grbit), Some(icol), Some(col), Some(obj), Some(rcol), Some(robj), Some(name)) =
            (ci("grbit"), ci("icolumn"), ci("szColumn"), ci("szObject"), ci("szReferencedColumn"), ci("szReferencedObject"), ci("szRelationship"))
        else {
            return Vec::new();
        };
        let mut parts: Vec<(String, i64, &Vec<Value>)> = rows.iter().map(|r| (r[name].to_text(), r[icol].as_f64().unwrap_or(0.0) as i64, r)).collect();
        parts.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut out: Vec<Relation> = Vec::new();
        for (n, _, r) in parts {
            if out.last().is_none_or(|l| l.name != n) {
                let bits = r[grbit].as_f64().unwrap_or(0.0) as i64 as u32;
                out.push(Relation {
                    name: n.clone(),
                    table: r[obj].to_text(),
                    ref_table: r[robj].to_text(),
                    enforce: bits & 0x2 == 0,
                    cascade_update: bits & 0x100 != 0,
                    cascade_delete: bits & 0x1000 != 0,
                    one_to_one: bits & 0x1 != 0,
                    ..Default::default()
                });
            }
            let l = out.last_mut().unwrap();
            l.columns.push(r[col].to_text());
            l.ref_columns.push(r[rcol].to_text());
        }
        out
    }

    /// Everything: the user's tables with their rows, relations and saved
    /// queries.
    pub fn read_all(&self) -> Result<Database, Error> {
        let mut db = Database::default();
        for def in self.tables() {
            db.tables.push(self.read_table(&def)?);
        }
        db.relations = self.relations();
        db.queries = queries::read(self);
        for (name, place, _) in self.linked() {
            db.warnings.push(format!("{name}: linked table in {place}"));
        }
        Ok(db)
    }
}

/// The file inside an attachment's FileData: a header (flags, length),
/// deflated or raw content, which starts with a header of its own (length,
/// flags, the file's extension).
impl AccessFile {
    fn attachment_data(&self, b: &[u8]) -> Vec<u8> {
        if b.len() < 8 {
            return Vec::new();
        }
        let kind = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let left = self.long_left.load(std::sync::atomic::Ordering::Relaxed).min(256 << 20);
        let content = if kind == 1 { miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&b[8..], left).unwrap_or_default() } else { b[8..].to_vec() };
        if content.len() < 8 || !self.charge(content.len()) {
            return Vec::new();
        }
        let head = u32::from_le_bytes([content[0], content[1], content[2], content[3]]) as usize;
        content.get(head..).map(<[u8]>::to_vec).unwrap_or_default()
    }

    /// Takes `n` bytes from the file's budget for long values, if left.
    fn charge(&self, n: usize) -> bool {
        use std::sync::atomic::Ordering;
        self.long_left.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |l| l.checked_sub(n)).is_ok()
    }
}

/// RC4 applied to `data` in place.
pub(crate) fn rc4(key: &[u8], data: &mut [u8]) {
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    for b in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        *b ^= s[s[i as usize].wrapping_add(s[j as usize]) as usize];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_known() {
        // The "Key"/"Plaintext" test vector.
        let mut d = b"Plaintext".to_vec();
        rc4(b"Key", &mut d);
        assert_eq!(d, [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]);
    }

    #[test]
    fn not_access() {
        assert!(AccessFile::from_bytes(vec![0; 4096]).is_err());
    }
}
