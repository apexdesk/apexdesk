//! Content streams as a list of operations, to change them and write them
//! back: redaction removes what is under its areas, and hidden content is
//! dropped this way.

use super::super::obj::{Item, Lexer, Obj};
use super::write::write_obj;

#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    pub args: Vec<Obj>,
    pub op: String,
    /// An inline image's bytes between `BI` and `EI` (dictionary, `ID`
    /// and data), for `BI`.
    pub inline: Option<Vec<u8>>,
}

impl Op {
    pub fn new(op: &str, args: Vec<Obj>) -> Op {
        Op { args, op: op.to_string(), inline: None }
    }

    pub fn num(&self, i: usize) -> f64 {
        self.args.get(i).and_then(Obj::num).unwrap_or(0.0)
    }
}

/// The operations of a content stream.
pub fn parse(data: &[u8]) -> Vec<Op> {
    let mut out = Vec::new();
    let mut lx = Lexer::new(data, 0);
    let mut args: Vec<Obj> = Vec::new();
    while let Some(item) = lx.next(false) {
        match item {
            Item::Obj(o) => {
                if args.len() < 1024 {
                    args.push(o);
                }
            }
            Item::Kw(b"BI") => {
                let start = lx.p;
                let mut end = data.len();
                let mut resume = data.len();
                if let Some(id) = memchr::memmem::find(&data[start..], b"ID") {
                    let mut i = start + id + 3;
                    while let Some(e) = memchr::memmem::find(&data[i..], b"EI") {
                        let at = i + e;
                        if at > 0 && super::super::obj::is_ws(data[at - 1]) && data.get(at + 2).is_none_or(|&b| super::super::obj::is_ws(b)) {
                            end = at;
                            resume = at + 2;
                            break;
                        }
                        i = at + 2;
                    }
                }
                out.push(Op { args: Vec::new(), op: "BI".into(), inline: Some(data[start..end.max(start)].to_vec()) });
                lx.p = resume.min(data.len());
                args.clear();
            }
            Item::Kw(k) => {
                if k.is_empty() {
                    lx.p += 1;
                    continue;
                }
                out.push(Op { args: std::mem::take(&mut args), op: String::from_utf8_lossy(k).into_owned(), inline: None });
            }
        }
    }
    out
}

/// A content stream from operations.
pub fn write(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::new();
    for op in ops {
        if let Some(raw) = &op.inline {
            out.extend_from_slice(b"BI");
            out.extend_from_slice(raw);
            out.extend_from_slice(b"EI\n");
            continue;
        }
        for a in &op.args {
            write_obj(a, &mut out);
            out.push(b' ');
        }
        out.extend_from_slice(op.op.as_bytes());
        out.push(b'\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let src = b"q 1 0 0 1 10 20 cm BT /F1 12 Tf (Hello \\(x\\)) Tj [(A) -120 (B)] TJ ET BI /W 2 /H 1 /BPC 8 /CS /G ID \x01\x02 EI Q";
        let ops = parse(src);
        let names: Vec<&str> = ops.iter().map(|o| o.op.as_str()).collect();
        assert_eq!(names, ["q", "cm", "BT", "Tf", "Tj", "TJ", "ET", "BI", "Q"]);
        assert_eq!(ops[4].args, vec![Obj::Str(b"Hello (x)".to_vec())]);
        let again = parse(&write(&ops));
        assert_eq!(again, ops);
    }
}
