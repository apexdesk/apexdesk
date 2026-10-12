//! Cell addresses and ranges (A1 notation).

pub const MAX_ROWS: u32 = 1_048_576;
pub const MAX_COLS: u32 = 16_384;

/// Zero-based cell position inside a sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
}

impl CellRef {
    pub const fn new(row: u32, col: u32) -> Self {
        Self { row, col }
    }

    /// Parses `B12`, `$B$12`, `b12`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let b = s.as_bytes();
        let mut i = 0;
        if b.get(i) == Some(&b'$') {
            i += 1;
        }
        let cs = i;
        while i < b.len() && b[i].is_ascii_alphabetic() {
            i += 1;
        }
        let col = parse_col(&s[cs..i])?;
        if b.get(i) == Some(&b'$') {
            i += 1;
        }
        let rs = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i != b.len() || rs == i {
            return None;
        }
        let row: u32 = s[rs..i].parse().ok()?;
        if row == 0 || row > MAX_ROWS {
            return None;
        }
        Some(Self::new(row - 1, col))
    }

    pub fn to_a1(self) -> String {
        let mut s = col_name(self.col);
        s.push_str(&(self.row + 1).to_string());
        s
    }
}

impl std::fmt::Display for CellRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_a1())
    }
}

/// `0 -> A`, `25 -> Z`, `26 -> AA`.
pub fn col_name(mut col: u32) -> String {
    let mut buf = [0u8; 4];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'A' + (col % 26) as u8;
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    String::from_utf8_lossy(&buf[i..]).into_owned()
}

/// `A -> 0`, case-insensitive. Returns `None` for empty or out-of-range input.
pub fn parse_col(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 3 {
        return None;
    }
    let mut n: u32 = 0;
    for c in s.bytes() {
        if !c.is_ascii_alphabetic() {
            return None;
        }
        n = n * 26 + (c.to_ascii_uppercase() - b'A' + 1) as u32;
    }
    if n == 0 || n > MAX_COLS {
        return None;
    }
    Some(n - 1)
}

/// Inclusive, normalized rectangular range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Range {
    pub start: CellRef,
    pub end: CellRef,
}

impl Range {
    pub fn new(a: CellRef, b: CellRef) -> Self {
        Self {
            start: CellRef::new(a.row.min(b.row), a.col.min(b.col)),
            end: CellRef::new(a.row.max(b.row), a.col.max(b.col)),
        }
    }

    pub fn single(c: CellRef) -> Self {
        Self { start: c, end: c }
    }

    /// Parses `A1:B3` or `A1`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.split_once(':') {
            Some((a, b)) => Some(Self::new(CellRef::parse(a)?, CellRef::parse(b)?)),
            None => Some(Self::single(CellRef::parse(s)?)),
        }
    }

    #[inline]
    pub fn contains(&self, c: CellRef) -> bool {
        c.row >= self.start.row && c.row <= self.end.row && c.col >= self.start.col && c.col <= self.end.col
    }

    pub fn rows(&self) -> u32 {
        self.end.row - self.start.row + 1
    }

    pub fn cols(&self) -> u32 {
        self.end.col - self.start.col + 1
    }

    pub fn cell_count(&self) -> u64 {
        self.rows() as u64 * self.cols() as u64
    }

    pub fn is_single(&self) -> bool {
        self.start == self.end
    }

    pub fn intersect(&self, o: &Range) -> Option<Range> {
        let r0 = self.start.row.max(o.start.row);
        let r1 = self.end.row.min(o.end.row);
        let c0 = self.start.col.max(o.start.col);
        let c1 = self.end.col.min(o.end.col);
        (r0 <= r1 && c0 <= c1).then(|| Range::new(CellRef::new(r0, c0), CellRef::new(r1, c1)))
    }

    pub fn to_a1(&self) -> String {
        if self.is_single() {
            self.start.to_a1()
        } else {
            format!("{}:{}", self.start.to_a1(), self.end.to_a1())
        }
    }
}

/// F4 in a formula: cycles the reference at char position `at` of `text`
/// through A1, $A$1, A$1 and $A1 (both ends of a range together). Returns
/// the new text and the char position after the reference.
pub fn toggle_absolute(text: &str, at: usize) -> Option<(String, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let at = at.min(chars.len());
    let in_ref = |c: char| c.is_ascii_alphanumeric() || c == '$' || c == ':';
    let mut s = at;
    while s > 0 && in_ref(chars[s - 1]) {
        s -= 1;
    }
    let mut e = at;
    while e < chars.len() && in_ref(chars[e]) {
        e += 1;
    }
    // Not a function name or part of a longer word.
    if s == e || chars.get(e) == Some(&'(') || (s > 0 && (chars[s - 1].is_alphanumeric() || chars[s - 1] == '_' || chars[s - 1] == '.')) {
        return None;
    }
    let token: String = chars[s..e].iter().collect();
    let parse = |p: &str| -> Option<(String, String)> {
        let p = p.replace('$', "");
        let split = p.find(|c: char| c.is_ascii_digit())?;
        let (col, row) = p.split_at(split);
        let ok = (1..=3).contains(&col.len()) && col.chars().all(|c| c.is_ascii_alphabetic()) && !row.is_empty() && row.chars().all(|c| c.is_ascii_digit()) && !row.starts_with('0');
        ok.then(|| (col.to_string(), row.to_string()))
    };
    let parts: Vec<&str> = token.split(':').collect();
    if parts.len() > 2 {
        return None;
    }
    let cells: Vec<(String, String)> = parts.iter().map(|p| parse(p)).collect::<Option<_>>()?;
    let first = parts[0];
    let col_abs = first.starts_with('$');
    let row_abs = first.trim_start_matches('$').contains('$');
    let (c, r) = match (col_abs, row_abs) {
        (false, false) => (true, true),
        (true, true) => (false, true),
        (false, true) => (true, false),
        (true, false) => (false, false),
    };
    let d = |on: bool| if on { "$" } else { "" };
    let new: Vec<String> = cells.iter().map(|(col, row)| format!("{}{col}{}{row}", d(c), d(r))).collect();
    let new = new.join(":");
    let pos = s + new.chars().count();
    let out: String = chars[..s].iter().collect::<String>() + &new + &chars[e..].iter().collect::<String>();
    Some((out, pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f4_cycles_references() {
        let t = |s: &str, at: usize| toggle_absolute(s, at);
        assert_eq!(t("=A1+B2", 3), Some(("=$A$1+B2".into(), 5)));
        assert_eq!(t("=$A$1+B2", 5), Some(("=A$1+B2".into(), 4)));
        assert_eq!(t("=A$1+B2", 2), Some(("=$A1+B2".into(), 4)));
        assert_eq!(t("=$A1+B2", 4), Some(("=A1+B2".into(), 3)));
        assert_eq!(t("=SUM(A1:B10)", 11), Some(("=SUM($A$1:$B$10)".into(), 15)));
        assert_eq!(t("=Hoja1!C3*2", 9), Some(("=Hoja1!$C$3*2".into(), 11)));
        // Function names and numbers aren't references.
        assert_eq!(t("=SUM(1)", 3), None);
        assert_eq!(t("=LOG10(2)", 6), None);
        assert_eq!(t("=2+3", 2), None);
    }

    #[test]
    fn col_names_roundtrip() {
        for (n, s) in [(0, "A"), (25, "Z"), (26, "AA"), (51, "AZ"), (52, "BA"), (701, "ZZ"), (702, "AAA"), (16383, "XFD")] {
            assert_eq!(col_name(n), s);
            assert_eq!(parse_col(s), Some(n));
        }
        assert_eq!(parse_col("XFE"), None);
    }

    #[test]
    fn parse_refs() {
        assert_eq!(CellRef::parse("$B$12"), Some(CellRef::new(11, 1)));
        assert_eq!(CellRef::parse("a1"), Some(CellRef::new(0, 0)));
        assert_eq!(CellRef::parse("A0"), None);
        assert_eq!(CellRef::parse("1A"), None);
        assert_eq!(Range::parse("C3:A1").unwrap().to_a1(), "A1:C3");
    }
}
