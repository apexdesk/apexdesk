//! Spreadsheet functions: double-byte text (the B variants, ASC, DBCS),
//! PHONETIC and BAHTTEXT.

use super::*;

pub(super) fn array_params(f: Func) -> Option<u64> {
    match f {
        Func::Phonetic => Some(0b1),
        _ => None,
    }
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    use Func::*;
    let mine = matches!(f, LenB | LeftB | RightB | MidB | FindB | SearchB | ReplaceB | Asc | Dbcs | Phonetic | BahtText);
    mine.then(|| eval(env, f, a))
}

fn eval(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    use Func::*;
    let n = a.len();
    let need = |min: usize, max: usize| if n < min || n > max { Err(ErrorKind::Value) } else { Ok(()) };
    // Byte counts: whole numbers, never negative.
    let count = |e: &Expr| -> R<usize> {
        let k = env.num(e)?.trunc();
        if k < 0.0 { Err(ErrorKind::Value) } else { Ok(k.min(1e9) as usize) }
    };
    Ok(match f {
        LenB => Value::Number(bytes(&env.text1(a)?) as f64),
        LeftB | RightB => {
            need(1, 2)?;
            let s = env.text(&a[0])?;
            let k = match a.get(1) {
                None | Some(Expr::Missing) => 1,
                Some(e) => count(e)?,
            };
            let len = bytes(&s);
            Value::text(if f == LeftB { mid(&s, 0, k) } else { mid(&s, len.saturating_sub(k), len) })
        }
        MidB => {
            need(3, 3)?;
            let s = env.text(&a[0])?;
            let start = env.num(&a[1])?.trunc();
            if start < 1.0 {
                return Err(ErrorKind::Value);
            }
            let from = start.min(1e9) as usize - 1;
            Value::text(mid(&s, from, from.saturating_add(count(&a[2])?)))
        }
        FindB | SearchB => {
            need(2, 3)?;
            let needle = env.text(&a[0])?;
            let hay = env.text(&a[1])?;
            let start = env.opt_num(a, 2, 1.0)?.trunc();
            if start < 1.0 || start > (bytes(&hay) + 1) as f64 {
                return Err(ErrorKind::Value);
            }
            let low = |s: &str| -> Vec<char> {
                s.chars().map(|c| if f == SearchB { c.to_lowercase().next().unwrap_or(c) } else { c }).collect()
            };
            let (h, nd) = (low(&hay), low(&needle));
            // Byte offset where each character starts.
            let offs: Vec<usize> = h.iter().scan(0, |o, &c| Some(std::mem::replace(o, *o + width(c)))).collect();
            let from = offs.iter().position(|&o| o + 1 >= start as usize).unwrap_or(h.len());
            match (from..=h.len().saturating_sub(nd.len())).find(|&i| h[i..].starts_with(&nd)) {
                Some(i) => Value::Number(offs.get(i).map_or(bytes(&hay), |&o| o) as f64 + 1.0),
                None => return Err(ErrorKind::Value),
            }
        }
        ReplaceB => {
            need(4, 4)?;
            let s = env.text(&a[0])?;
            let start = env.num(&a[1])?.trunc();
            if start < 1.0 {
                return Err(ErrorKind::Value);
            }
            let from = start.min(1e9) as usize - 1;
            let len = count(&a[2])?;
            let new = env.text(&a[3])?;
            Value::text(format!("{}{}{}", mid(&s, 0, from), new, mid(&s, from.saturating_add(len), usize::MAX)))
        }
        Asc => Value::text(to_half(&env.text1(a)?)),
        Dbcs => Value::text(to_full(&env.text1(a)?)),
        Phonetic => {
            need(1, 1)?;
            // Phonetic guides aren't kept: the text itself stands for them.
            let t = env.tab(&a[0])?;
            let mut out = String::new();
            for i in 0..t.len() {
                match t.nth(i) {
                    Value::Text(s) if out.len() + s.len() <= MAX_TEXT_BYTES => out.push_str(&s),
                    Value::Text(_) => return Err(ErrorKind::Value),
                    Value::Error(k) => return Err(k),
                    _ => {}
                }
            }
            Value::text(out)
        }
        BahtText => Value::text(baht_text(env.num1(a)?)),
        _ => return Err(ErrorKind::Name),
    })
}

/// Bytes a character takes in a double-byte charset: East Asian wide and
/// full-width characters count two.
fn width(c: char) -> usize {
    let u = c as u32;
    let wide = matches!(u,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF
        | 0xA960..=0xA97F | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE10..=0xFE19 | 0xFE30..=0xFE6F | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD);
    1 + wide as usize
}

fn bytes(s: &str) -> usize {
    s.chars().map(width).sum()
}

/// The characters within bytes `from..to`; one cut in half becomes a space.
fn mid(s: &str, from: usize, to: usize) -> String {
    let mut out = String::new();
    let mut o = 0;
    for c in s.chars() {
        let end = o + width(c);
        if o >= to {
            break;
        }
        if o >= from && end <= to {
            out.push(c);
        } else if end > from {
            out.push(' ');
        }
        o = end;
    }
    out
}

/// Full-width forms of the half-width katakana U+FF61..=U+FF9F.
const KANA: [char; 63] = [
    '。', '「', '」', '、', '・', 'ヲ', 'ァ', 'ィ', 'ゥ', 'ェ', 'ォ', 'ャ', 'ュ', 'ョ', 'ッ', 'ー', 'ア', 'イ', 'ウ', 'エ', 'オ', 'カ', 'キ', 'ク', 'ケ', 'コ', 'サ', 'シ', 'ス', 'セ', 'ソ', 'タ',
    'チ', 'ツ', 'テ', 'ト', 'ナ', 'ニ', 'ヌ', 'ネ', 'ノ', 'ハ', 'ヒ', 'フ', 'ヘ', 'ホ', 'マ', 'ミ', 'ム', 'メ', 'モ', 'ヤ', 'ユ', 'ヨ', 'ラ', 'リ', 'ル', 'レ', 'ロ', 'ワ', 'ン', '゛', '゜',
];

/// Full-width signs U+FFE0..=U+FFE6 and their narrow forms.
const SIGNS: [char; 7] = ['¢', '£', '¬', '¯', '¦', '¥', '₩'];

/// Katakana that take the voiced mark (ka..to, ha..ho, u, wa, wo).
fn voices(c: char) -> bool {
    matches!(c, 'カ'..='ト' | 'ハ'..='ホ' | 'ウ' | 'ワ' | 'ヲ') && c != 'ッ' && KANA.contains(&c)
}

/// Katakana that take the semi-voiced mark.
fn semi_voices(c: char) -> bool {
    matches!(c, 'ハ' | 'ヒ' | 'フ' | 'ヘ' | 'ホ')
}

/// The voiced form of a full-width kana: ガ from カ...
fn voiced(c: char) -> Option<char> {
    match c {
        'ウ' => Some('ヴ'),
        'ワ' => Some('ヷ'),
        'ヲ' => Some('ヺ'),
        _ => char::from_u32(c as u32 + 1),
    }
}

/// ASC: full-width letters, digits, signs and katakana to half-width.
fn to_half(s: &str) -> String {
    let half = |c: char| KANA.iter().position(|&k| k == c).and_then(|i| char::from_u32(0xFF61 + i as u32));
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let u = c as u32;
        match u {
            0xFF01..=0xFF5E => out.push(char::from_u32(u - 0xFEE0).unwrap_or(c)),
            0x3000 => out.push(' '),
            0xFFE0..=0xFFE6 => out.push(SIGNS[(u - 0xFFE0) as usize]),
            _ => {
                if let Some(h) = half(c) {
                    out.push(h);
                    continue;
                }
                let base = |d: u32| char::from_u32(u - d).filter(|&b| voices(b) && voiced(b) == Some(c));
                let semi = char::from_u32(u.wrapping_sub(2)).filter(|&b| semi_voices(b));
                let (b, mark) = match c {
                    'ヴ' => ('ウ', 'ﾞ'),
                    'ヷ' => ('ワ', 'ﾞ'),
                    'ヺ' => ('ヲ', 'ﾞ'),
                    _ => match (base(1), semi) {
                        (Some(b), _) => (b, 'ﾞ'),
                        (None, Some(b)) => (b, 'ﾟ'),
                        _ => {
                            out.push(c);
                            continue;
                        }
                    },
                };
                out.extend(half(b));
                out.push(mark);
            }
        }
    }
    out
}

/// DBCS: the reverse of [`to_half`], joining voiced marks to their kana.
fn to_full(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        let u = c as u32;
        match u {
            0x21..=0x7E => out.push(char::from_u32(u + 0xFEE0).unwrap_or(c)),
            0x20 => out.push('\u{3000}'),
            0xFF61..=0xFF9F => {
                let k = KANA[(u - 0xFF61) as usize];
                let joined = match it.peek() {
                    Some('ﾞ') if voices(k) => voiced(k),
                    Some('ﾟ') if semi_voices(k) => char::from_u32(k as u32 + 2),
                    _ => None,
                };
                if joined.is_some() {
                    it.next();
                }
                out.push(joined.unwrap_or(k));
            }
            _ => match SIGNS.iter().position(|&x| x == c) {
                Some(i) => out.push(char::from_u32(0xFFE0 + i as u32).unwrap_or(c)),
                None => out.push(c),
            },
        }
    }
    out
}

const TH_DIGITS: [&str; 10] = ["ศูนย์", "หนึ่ง", "สอง", "สาม", "สี่", "ห้า", "หก", "เจ็ด", "แปด", "เก้า"];

/// Thai words for 1..=999_999 (LibreOffice's and Excel's rules: "et" for
/// a final one only after tens, "yi" for twenty).
fn baht_block(out: &mut String, mut v: u64) {
    for (p, word) in [(100_000, "แสน"), (10_000, "หมื่น"), (1000, "พัน"), (100, "ร้อย")] {
        if v >= p {
            out.push_str(TH_DIGITS[(v / p) as usize]);
            out.push_str(word);
            v %= p;
        }
    }
    let (ten, one) = (v / 10, v % 10);
    if ten > 0 {
        match ten {
            1 => {}
            2 => out.push_str("ยี่"),
            t => out.push_str(TH_DIGITS[t as usize]),
        }
        out.push_str("สิบ");
    }
    if ten > 0 && one == 1 {
        out.push_str("เอ็ด");
    } else if one > 0 {
        out.push_str(TH_DIGITS[one as usize]);
    }
}

/// BAHTTEXT: an amount in Thai words, in baht and satang.
fn baht_text(x: f64) -> String {
    let cents = (x.abs() * 100.0 + 0.5).floor();
    let (mut baht, satang) = ((cents / 100.0).floor(), (cents % 100.0) as u64);
    let mut out = String::new();
    if baht == 0.0 && satang == 0 {
        out.push_str(TH_DIGITS[0]);
    }
    // Blocks of six digits, joined by "million".
    while baht > 0.0 {
        let mut block = String::new();
        let v = (baht % 1e6) as u64;
        baht = (baht / 1e6).floor();
        if v > 0 {
            baht_block(&mut block, v);
        }
        if baht > 0.0 {
            block.insert_str(0, "ล้าน");
        }
        out.insert_str(0, &block);
    }
    if !out.is_empty() {
        out.push_str("บาท");
    }
    if satang == 0 {
        out.push_str("ถ้วน");
    } else {
        baht_block(&mut out, satang);
        out.push_str("สตางค์");
    }
    if x < 0.0 && cents > 0.0 {
        out.insert_str(0, "ลบ");
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::format::Locale;
    use crate::value::{ErrorKind, Value};
    use crate::{CellRef, Workbook};

    fn wb() -> Workbook {
        let mut wb = Workbook::new();
        wb.locale = Locale::EN;
        wb
    }

    fn ev(w: &Workbook, f: &str) -> Value {
        w.eval_formula(0, f)
    }

    fn t(s: &str) -> Value {
        Value::text(s)
    }

    fn num(n: f64) -> Value {
        Value::Number(n)
    }

    // Expected values from LibreOffice.
    #[test]
    fn byte_functions() {
        let w = wb();
        assert_eq!(ev(&w, "=LENB(\"abc\")"), num(3.0));
        assert_eq!(ev(&w, "=LENB(\"日本語\")"), num(6.0));
        assert_eq!(ev(&w, "=LENB(\"é€АБВ\")"), num(5.0));
        assert_eq!(ev(&w, "=LENB(\"한국ＡＢ、。\")"), num(12.0));
        assert_eq!(ev(&w, "=LENB(\"ｱｲｳ\")"), num(3.0));
        assert_eq!(ev(&w, "=LEFTB(\"日本語\",3)"), t("日 "));
        assert_eq!(ev(&w, "=LEFTB(\"日本語\",1)"), t(" "));
        assert_eq!(ev(&w, "=LEFTB(\"日本語\")"), t(" "));
        assert_eq!(ev(&w, "=LEFTB(\"abc\",-1)"), Value::Error(ErrorKind::Value));
        assert_eq!(ev(&w, "=RIGHTB(\"日本語\",3)"), t(" 語"));
        assert_eq!(ev(&w, "=RIGHTB(\"日本語\",10)"), t("日本語"));
        assert_eq!(ev(&w, "=MIDB(\"a日本b\",2,3)"), t("日 "));
        assert_eq!(ev(&w, "=MIDB(\"a日本b\",3,2)"), t("  "));
        assert_eq!(ev(&w, "=MIDB(\"a日本b\",3,3)"), t(" 本"));
        assert_eq!(ev(&w, "=MIDB(\"a日本b\",2,1)"), t(" "));
        assert_eq!(ev(&w, "=MIDB(\"abc\",0,1)"), Value::Error(ErrorKind::Value));
        assert_eq!(ev(&w, "=FINDB(\"本\",\"a日本\")"), num(4.0));
        assert_eq!(ev(&w, "=FINDB(\"b\",\"a日本b\",3)"), num(6.0));
        assert_eq!(ev(&w, "=FINDB(\"B\",\"a日本b\")"), Value::Error(ErrorKind::Value));
        assert_eq!(ev(&w, "=SEARCHB(\"B\",\"a日本b\")"), num(6.0));
        assert_eq!(ev(&w, "=SEARCHB(\"本\",\"A日本\",3)"), num(4.0));
        assert_eq!(ev(&w, "=FINDB(\"\",\"日本\",3)"), num(3.0));
        assert_eq!(ev(&w, "=REPLACEB(\"日本語\",3,2,\"X\")"), t("日X語"));
        assert_eq!(ev(&w, "=REPLACEB(\"日本語\",2,2,\"X\")"), t(" X 語"));
        assert_eq!(ev(&w, "=REPLACEB(\"日本語\",2,1,\"X\")"), t(" X本語"));
        assert_eq!(ev(&w, "=REPLACEB(\"abc\",2,1,\"日\")"), t("a日c"));
    }

    #[test]
    fn asc_dbcs() {
        let w = wb();
        assert_eq!(ev(&w, "=ASC(\"ＡＢＣ　１２３ｱ\")"), t("ABC 123ｱ"));
        assert_eq!(ev(&w, "=ASC(\"アイウガパヴッヅ\")"), t("ｱｲｳｶﾞﾊﾟｳﾞｯﾂﾞ"));
        assert_eq!(ev(&w, "=ASC(\"、。「」ー・\")"), t("､｡｢｣ｰ･"));
        assert_eq!(ev(&w, "=ASC(\"ぁあ漢\")"), t("ぁあ漢"));
        assert_eq!(ev(&w, "=DBCS(\"ABC 123 ｶﾞｱﾟﾊﾟ\")"), t("ＡＢＣ　１２３　ガア゜パ"));
        assert_eq!(ev(&w, "=JIS(\"ｳﾞﾜｦﾝｰｯ\")"), t("ヴワヲンーッ"));
        assert_eq!(ev(&w, "=ASC(DBCS(\"Hi ｶﾞﾊﾟﾎﾟﾄﾞ!¥\"))"), t("Hi ｶﾞﾊﾟﾎﾟﾄﾞ!¥"));
    }

    #[test]
    fn phonetic() {
        let mut w = wb();
        w.set_input(0, CellRef::new(0, 0), "漢字");
        w.set_input(0, CellRef::new(1, 0), "かな");
        assert_eq!(ev(&w, "=PHONETIC(A1)"), t("漢字"));
        assert_eq!(ev(&w, "=PHONETIC(A1:A3)"), t("漢字かな"));
    }

    #[test]
    fn baht_text() {
        let w = wb();
        let cases = [
            ("1234", "หนึ่งพันสองร้อยสามสิบสี่บาทถ้วน"),
            ("0", "ศูนย์บาทถ้วน"),
            ("21", "ยี่สิบเอ็ดบาทถ้วน"),
            ("11", "สิบเอ็ดบาทถ้วน"),
            ("101", "หนึ่งร้อยหนึ่งบาทถ้วน"),
            ("1234.56", "หนึ่งพันสองร้อยสามสิบสี่บาทห้าสิบหกสตางค์"),
            ("0.5", "ห้าสิบสตางค์"),
            ("-15.25", "ลบสิบห้าบาทยี่สิบห้าสตางค์"),
            ("21000000", "ยี่สิบเอ็ดล้านบาทถ้วน"),
            ("1000001", "หนึ่งล้านหนึ่งบาทถ้วน"),
            ("0.255", "ยี่สิบหกสตางค์"),
            ("0.001", "ศูนย์บาทถ้วน"),
            ("0.995", "หนึ่งบาทถ้วน"),
            ("10000000000000", "สิบล้านล้านบาทถ้วน"),
            ("2111111", "สองล้านหนึ่งแสนหนึ่งหมื่นหนึ่งพันหนึ่งร้อยสิบเอ็ดบาทถ้วน"),
            ("1.01", "หนึ่งบาทหนึ่งสตางค์"),
        ];
        for (x, s) in cases {
            assert_eq!(ev(&w, &format!("=BAHTTEXT({x})")), t(s), "{x}");
        }
        assert_eq!(ev(&w, "=BAHTTEXT(\"x\")"), Value::Error(ErrorKind::Value));
    }
}
