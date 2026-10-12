//! Sheet and workbook protection. Not encryption: like in other
//! spreadsheets, the password only guards changes made through the UI.

use crate::addr::{CellRef, Range};
use crate::hash::{base64, sha256, sha512, unbase64};

/// A stored protection password, in one of the forms files use.
#[derive(Clone, Debug, PartialEq)]
pub enum Password {
    /// Excel's old 16-bit hash (`password="CBEB"`).
    Legacy(u16),
    /// Excel's salted, iterated hash (`algorithmName="SHA-512"`, base64).
    Salted { algorithm: String, hash: String, salt: String, spin: u32 },
    /// OpenDocument's `table:protection-key`: a base64 digest of the UTF-8
    /// password; `algorithm` is the digest URI.
    Digest { algorithm: String, key: String },
}

pub const ODF_SHA256: &str = "http://www.w3.org/2000/09/xmldsig#sha256";
/// Iterations for new salted hashes (Excel uses 100000).
const SPIN: u32 = 100_000;

impl Password {
    /// Hashes for a new password, one per file format.
    pub fn new(pw: &str) -> Vec<Password> {
        let salt: Vec<u8> = (0..2).flat_map(|_| random_u64().to_le_bytes()).collect();
        let hash = salted(&salt, pw, SPIN);
        vec![
            Password::Salted { algorithm: "SHA-512".into(), hash: base64(&hash), salt: base64(&salt), spin: SPIN },
            Password::Digest { algorithm: ODF_SHA256.into(), key: base64(&sha256(pw.as_bytes())) },
        ]
    }

    /// Whether `pw` matches; `None` when the algorithm is unknown.
    pub fn check(&self, pw: &str) -> Option<bool> {
        match self {
            Password::Legacy(h) => Some(legacy_hash(pw) == *h),
            Password::Salted { algorithm, hash, salt, spin } if algorithm.eq_ignore_ascii_case("SHA-512") => {
                let salt = unbase64(salt)?;
                Some(base64(&salted(&salt, pw, (*spin).min(10_000_000))) == *hash)
            }
            Password::Digest { algorithm, key } if algorithm == ODF_SHA256 => Some(base64(&sha256(pw.as_bytes())) == *key),
            _ => None,
        }
    }
}

/// Excel's salted hash: SHA-512 of salt + UTF-16LE password, then `spin`
/// rounds of SHA-512 of the hash + the round number.
fn salted(salt: &[u8], pw: &str, spin: u32) -> [u8; 64] {
    let mut data = salt.to_vec();
    data.extend(pw.encode_utf16().flat_map(|u| u.to_le_bytes()));
    let mut h = sha512(&data);
    let mut buf = [0u8; 68];
    for i in 0..spin {
        buf[..64].copy_from_slice(&h);
        buf[64..].copy_from_slice(&i.to_le_bytes());
        h = sha512(&buf);
    }
    h
}

pub fn legacy_hash(pw: &str) -> u16 {
    let bytes: Vec<u8> = pw.chars().take(15).map(|c| c as u32 as u8).collect();
    let rot = |w: u16| ((w >> 14) & 1) | ((w << 1) & 0x7fff);
    let mut w: u16 = 0;
    for &b in bytes.iter().rev() {
        w = rot(w) ^ b as u16;
    }
    rot(w) ^ bytes.len() as u16 ^ 0xCE4B
}

fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.finish()
}

/// Actions users may still do on a protected sheet.
pub mod allow {
    pub const SELECT_LOCKED: u32 = 1;
    pub const SELECT_UNLOCKED: u32 = 1 << 1;
    pub const FORMAT_CELLS: u32 = 1 << 2;
    pub const FORMAT_COLUMNS: u32 = 1 << 3;
    pub const FORMAT_ROWS: u32 = 1 << 4;
    pub const INSERT_COLUMNS: u32 = 1 << 5;
    pub const INSERT_ROWS: u32 = 1 << 6;
    pub const INSERT_LINKS: u32 = 1 << 7;
    pub const DELETE_COLUMNS: u32 = 1 << 8;
    pub const DELETE_ROWS: u32 = 1 << 9;
    pub const SORT: u32 = 1 << 10;
    pub const AUTOFILTER: u32 = 1 << 11;
    pub const PIVOT_TABLES: u32 = 1 << 12;
    pub const OBJECTS: u32 = 1 << 13;
    pub const SCENARIOS: u32 = 1 << 14;
    /// Excel's defaults.
    pub const DEFAULT: u32 = SELECT_LOCKED | SELECT_UNLOCKED;
    /// Each flag with its xlsx `sheetProtection` attribute (where "1"
    /// means the action is *blocked*, except for objects and scenarios
    /// whose attributes say they are protected too).
    pub const XLSX: [(u32, &str); 15] = [
        (SELECT_LOCKED, "selectLockedCells"),
        (SELECT_UNLOCKED, "selectUnlockedCells"),
        (FORMAT_CELLS, "formatCells"),
        (FORMAT_COLUMNS, "formatColumns"),
        (FORMAT_ROWS, "formatRows"),
        (INSERT_COLUMNS, "insertColumns"),
        (INSERT_ROWS, "insertRows"),
        (INSERT_LINKS, "insertHyperlinks"),
        (DELETE_COLUMNS, "deleteColumns"),
        (DELETE_ROWS, "deleteRows"),
        (SORT, "sort"),
        (AUTOFILTER, "autoFilter"),
        (PIVOT_TABLES, "pivotTables"),
        (OBJECTS, "objects"),
        (SCENARIOS, "scenarios"),
    ];
}

/// Protection of a sheet (or of the workbook's structure).
#[derive(Clone, Debug, PartialEq)]
pub struct Protection {
    /// Empty when there is no password.
    pub passwords: Vec<Password>,
    /// [`allow`] flags (sheets only).
    pub allow: u32,
}

impl Default for Protection {
    fn default() -> Self {
        Protection { passwords: Vec::new(), allow: allow::DEFAULT }
    }
}

impl Protection {
    pub fn with_password(pw: &str, allow: u32) -> Protection {
        Protection { passwords: if pw.is_empty() { Vec::new() } else { Password::new(pw) }, allow }
    }

    pub fn has_password(&self) -> bool {
        !self.passwords.is_empty()
    }

    /// Whether `pw` unlocks it. Hashes we can't check (other algorithms)
    /// don't lock the user out.
    pub fn unlocks(&self, pw: &str) -> bool {
        unlocks(&self.passwords, pw)
    }

    pub fn allows(&self, flag: u32) -> bool {
        self.allow & flag != 0
    }
}

fn unlocks(passwords: &[Password], pw: &str) -> bool {
    let known: Vec<bool> = passwords.iter().filter_map(|p| p.check(pw)).collect();
    known.is_empty() || known.contains(&true)
}

/// Cells users may still edit on a protected sheet (Review > Allow Edit
/// Ranges), with an optional password of their own.
#[derive(Clone, Debug, PartialEq)]
pub struct EditRange {
    pub title: String,
    pub ranges: Vec<Range>,
    /// Empty when there is no password.
    pub passwords: Vec<Password>,
    /// Its password was given in this session (never saved).
    pub unlocked: bool,
}

impl EditRange {
    pub fn new(title: impl Into<String>, ranges: Vec<Range>, pw: &str) -> EditRange {
        EditRange { title: title.into(), ranges, passwords: if pw.is_empty() { Vec::new() } else { Password::new(pw) }, unlocked: false }
    }

    pub fn contains(&self, c: CellRef) -> bool {
        self.ranges.iter().any(|r| r.contains(c))
    }

    pub fn has_password(&self) -> bool {
        !self.passwords.is_empty()
    }

    /// Whether its cells can be edited now: no password, or given already.
    pub fn is_open(&self) -> bool {
        !self.has_password() || self.unlocked
    }

    pub fn unlocks(&self, pw: &str) -> bool {
        unlocks(&self.passwords, pw)
    }

    /// The ranges as a reference list (`A1:B4,D2`), sheet-less.
    pub fn refers_to(&self) -> String {
        self.ranges.iter().map(|r| r.to_a1()).collect::<Vec<_>>().join(",")
    }

    /// Ranges from a list such as `A1:B4,D2` or `$A$1:$B$4 D2` (a sheet
    /// name before `!` is dropped); None if any part is not a reference.
    pub fn parse_refs(s: &str) -> Option<Vec<Range>> {
        let v: Option<Vec<Range>> = s
            .split([',', ';', ' '])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| Range::parse(&p.rsplit_once('!').map_or(p, |x| x.1).replace('$', "")))
            .collect();
        v.filter(|v| !v.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords() {
        assert_eq!(legacy_hash("test"), 0xCBEB);
        assert_eq!(legacy_hash("password"), 0x83AF);
        let p = Protection::with_password("secret", allow::DEFAULT);
        assert!(p.unlocks("secret"));
        assert!(!p.unlocks("Secret"));
        assert!(Protection::default().unlocks("anything"));
        let legacy = Protection { passwords: vec![Password::Legacy(0xCBEB)], allow: 0 };
        assert!(legacy.unlocks("test") && !legacy.unlocks("tost"));
        // A salted hash as Excel writes it, checked with few rounds.
        let salt = [7u8; 16];
        let pw = Password::Salted { algorithm: "SHA-512".into(), hash: base64(&salted(&salt, "abc", 3)), salt: base64(&salt), spin: 3 };
        assert_eq!(pw.check("abc"), Some(true));
        assert_eq!(pw.check("abd"), Some(false));
        let other = Protection { passwords: vec![Password::Digest { algorithm: "sha1".into(), key: "x".into() }], allow: 0 };
        assert!(other.unlocks("whatever"));
    }

    #[test]
    fn edit_ranges() {
        let r = EditRange::new("Prices", EditRange::parse_refs("Sheet1!$A$1:$B$4, D2").unwrap(), "pw");
        assert_eq!(r.refers_to(), "A1:B4,D2");
        assert!(r.contains(CellRef::new(3, 1)) && r.contains(CellRef::new(1, 3)) && !r.contains(CellRef::new(4, 0)));
        assert!(r.has_password() && !r.is_open());
        assert!(r.unlocks("pw") && !r.unlocks("PW"));
        assert!(EditRange::new("Free", vec![], "").is_open());
        assert!(EditRange::parse_refs("A1,zz").is_none() && EditRange::parse_refs(" ").is_none());
    }
}
