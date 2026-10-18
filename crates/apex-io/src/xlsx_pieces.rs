//! A worksheet part read a piece at a time. Its cells (inside
//! `sheetData`) can be hundreds of MB of XML; holding them whole, as text
//! and as bytes, took more memory than the cells themselves.

use memchr::memmem;

use crate::Error;
use crate::zip::ZipReader;

/// Cells are handed over in pieces of about this size.
const PIECE: usize = 4 << 20;

/// Reads the worksheet `path`, handing `f` its XML in pieces that start and
/// end between elements: the start up to `<sheetData>`, the cells a few
/// MiB at a time, and the end from `</sheetData>`; `f` is told which
/// pieces are cells. Returns the part without the cells, for the readers
/// of the rest of a sheet.
pub(crate) fn read(z: &ZipReader, path: &str, mut f: impl FnMut(String, bool)) -> Result<String, Error> {
    let mut s = Splitter { f: &mut f, buf: Vec::new(), scan: 0, cut_scan: 0, stage: Stage::Head, rest: Vec::new(), started: false, fed: false };
    let res = z.read_pieces(path, |b| s.push(b));
    // A part that is missing, or damaged before its cells, reads as
    // before: not at all. Damage later keeps the cells read.
    if let Err(e) = res {
        if !s.fed {
            return Err(e);
        }
    }
    s.finish();
    Ok(String::from_utf8_lossy(&s.rest).into_owned())
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    /// Before the cells.
    Head,
    /// Inside `sheetData`.
    Cells,
    /// From `</sheetData>` on.
    Tail,
}

struct Splitter<'f> {
    f: &'f mut dyn FnMut(String, bool),
    /// Bytes not handed over yet.
    buf: Vec<u8>,
    /// Where to look on in `buf` for what ends the stage.
    scan: usize,
    /// Where to look on in `buf` for the end of a cell (none before).
    cut_scan: usize,
    stage: Stage,
    /// The part without its cells.
    rest: Vec<u8>,
    started: bool,
    fed: bool,
}

impl Splitter<'_> {
    fn push(&mut self, mut b: &[u8]) {
        if !self.started {
            self.started = true;
            b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
        }
        self.buf.extend_from_slice(b);
        loop {
            match self.stage {
                Stage::Head => {
                    let Some(end) = open_tag_end(&self.buf, &mut self.scan, b"sheetData") else { return };
                    let empty = self.buf[end - 2] == b'/';
                    self.hand(end, true);
                    self.stage = if empty { Stage::Tail } else { Stage::Cells };
                }
                Stage::Cells => {
                    if let Some(start) = close_tag_start(&self.buf, &mut self.scan, b"sheetData") {
                        self.hand(start, false);
                        self.stage = Stage::Tail;
                        continue;
                    }
                    if self.buf.len() >= PIECE {
                        // Only what came since the last look: looking through
                        // all again on each piece would take hours.
                        match last_cell_end(&self.buf[self.cut_scan..]) {
                            Some(cut) => self.hand(self.cut_scan + cut, false),
                            None => self.cut_scan = self.buf.len().saturating_sub(64),
                        }
                    }
                    return;
                }
                Stage::Tail => return,
            }
        }
    }

    /// Hands over the first `n` bytes of `buf`, keeping them in `rest`
    /// unless they are cells.
    fn hand(&mut self, n: usize, keep: bool) {
        if n == 0 {
            return;
        }
        if keep {
            self.rest.extend_from_slice(&self.buf[..n]);
        }
        // The piece leaves with the buffer it is in; what follows it goes
        // to a new one, with room for the next piece.
        let mut tail = Vec::with_capacity((self.buf.len() - n).max(PIECE + (2 << 20)));
        tail.extend_from_slice(&self.buf[n..]);
        let mut piece = std::mem::replace(&mut self.buf, tail);
        piece.truncate(n);
        let text = String::from_utf8(piece).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        (self.f)(text, !keep);
        self.scan = 0;
        self.cut_scan = 0;
        self.fed = true;
    }

    fn finish(&mut self) {
        let keep = self.stage != Stage::Cells;
        self.hand(self.buf.len(), keep);
    }
}

/// Whether `b` is a character of an XML name.
fn name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.') || b >= 0x80
}

/// The start of the tag that `name` at `i` of `buf` names (any prefix),
/// when it is a tag's name: its `<`, and whether it is an end tag.
fn tag_start(buf: &[u8], i: usize, name: &[u8]) -> Option<(usize, bool)> {
    let after = *buf.get(i + name.len())?;
    if !(after == b'>' || after == b'/' || after.is_ascii_whitespace()) {
        return None;
    }
    let mut j = i;
    if j > 0 && buf[j - 1] == b':' {
        j -= 1;
        while j > 0 && name_char(buf[j - 1]) {
            j -= 1;
        }
    }
    match (j.checked_sub(1).map(|k| buf[k]), j.checked_sub(2).map(|k| buf[k])) {
        (Some(b'/'), Some(b'<')) => Some((j - 2, true)),
        (Some(b'<'), _) => Some((j - 1, false)),
        _ => None,
    }
}

/// The end (past `>`) of the first start tag `name` from `*scan` on; when
/// there is none yet, `*scan` moves to where to look again.
fn open_tag_end(buf: &[u8], scan: &mut usize, name: &[u8]) -> Option<usize> {
    let mut from = *scan;
    while let Some(k) = memmem::find(&buf[from..], name) {
        let i = from + k;
        if let Some((_, false)) = tag_start(buf, i, name) {
            let Some(gt) = memchr::memchr(b'>', &buf[i..]) else {
                *scan = i;
                return None;
            };
            return Some(i + gt + 1);
        }
        from = i + 1;
    }
    *scan = buf.len().saturating_sub(name.len() + 64);
    None
}

/// The start (its `<`) of the first end tag `name` from `*scan` on.
fn close_tag_start(buf: &[u8], scan: &mut usize, name: &[u8]) -> Option<usize> {
    let mut from = *scan;
    while let Some(k) = memmem::find(&buf[from..], name) {
        let i = from + k;
        if let Some((lt, true)) = tag_start(buf, i, name) {
            return Some(lt);
        }
        from = i + 1;
    }
    *scan = buf.len().saturating_sub(name.len() + 64);
    None
}

/// The end of the last `</row>` or `</c>` (any prefix) in `buf`: the cells
/// can be cut there.
fn last_cell_end(buf: &[u8]) -> Option<usize> {
    let mut end = buf.len();
    while let Some(i) = memmem::rfind(&buf[..end], b"</") {
        if let Some(gt) = memchr::memchr(b'>', &buf[i..]) {
            let tag = &buf[i + 2..i + gt];
            let local = tag.rsplit(|&b| b == b':').next().unwrap_or(tag);
            if local == b"row" || local == b"c" {
                return Some(i + gt + 1);
            }
        }
        end = i;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Splits `xml` as `read` does, fed `step` bytes at a time.
    fn split(xml: &str, step: usize) -> (Vec<String>, String) {
        let mut pieces = Vec::new();
        let mut f = |p: String, _| pieces.push(p);
        let mut s = Splitter { f: &mut f, buf: Vec::new(), scan: 0, cut_scan: 0, stage: Stage::Head, rest: Vec::new(), started: false, fed: false };
        for b in xml.as_bytes().chunks(step) {
            s.push(b);
        }
        s.finish();
        let rest = String::from_utf8(s.rest).unwrap();
        (pieces, rest)
    }

    #[test]
    fn pieces_join_to_the_part_and_the_rest_has_no_cells() {
        let xml = r#"<worksheet><cols><col min="1" max="1" width="9"/></cols><x:sheetData><row r="1"><c r="A1"><v>1</v></c></row><row r="2"/></x:sheetData><mergeCells><mergeCell ref="A1:B1"/></mergeCells></worksheet>"#;
        for step in [1, 3, 7, 1000] {
            let (pieces, rest) = split(xml, step);
            assert_eq!(pieces.concat(), xml, "step {step}");
            assert_eq!(rest, r#"<worksheet><cols><col min="1" max="1" width="9"/></cols><x:sheetData></x:sheetData><mergeCells><mergeCell ref="A1:B1"/></mergeCells></worksheet>"#);
            assert!(pieces[0].ends_with("<x:sheetData>"), "{pieces:?}");
        }
    }

    #[test]
    fn an_empty_sheet_keeps_its_tag() {
        let xml = "\u{FEFF}<worksheet><sheetDataX/><sheetData/><pageMargins/></worksheet>";
        let (pieces, rest) = split(xml, 5);
        assert_eq!(pieces.concat(), &xml[3..]);
        assert_eq!(rest, &xml[3..]);
    }

    #[test]
    fn big_cells_are_cut_after_a_cell() {
        let row = r#"<row r="1"><c r="A1" t="inlineStr"><is><t>&lt;/c&gt; ü</t></is></c></row>"#;
        let xml = format!("<worksheet><sheetData>{}</sheetData></worksheet>", row.repeat(PIECE / row.len() * 3));
        let (pieces, _) = split(&xml, 1 << 20);
        assert!(pieces.len() > 3, "{}", pieces.len());
        assert_eq!(pieces.concat(), xml);
        for p in &pieces[1..pieces.len() - 1] {
            assert!(p.ends_with("</c>") || p.ends_with("</row>"), "{}", &p[p.len() - 20..]);
        }
    }
}
