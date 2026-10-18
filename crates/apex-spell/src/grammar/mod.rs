//! Grammar and style checking with rules, as LanguageTool does: patterns
//! over words and what the dictionary says about them (stem, plural,
//! verb form). Catches the frequent mistakes, not the ones of meaning.
//!
//! ```
//! let issues = apex_spell::check_grammar("Vino el el lunes.", "es_ES", None);
//! assert_eq!(issues[0].rule, "repeated");
//! assert_eq!(issues[0].suggestions, ["el"]);
//! ```

mod en;
mod es;

use std::ops::Range;

use crate::Dictionary;

/// A mistake in a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// Byte range of the words to change.
    pub range: Range<usize>,
    /// Which rule found it (see [`RULES`]).
    pub rule: &'static str,
    /// Replacements for the whole range, best first; may be empty.
    pub suggestions: Vec<String>,
}

/// Every rule, by id. Rules of one language start with its code.
pub const RULES: &[&str] = &[
    "repeated",
    "space_before",
    "space_after",
    "double_space",
    "capital",
    "es_agreement",
    "es_contraction",
    "es_a_ver",
    "es_sino",
    "es_porque",
    "es_dequeismo",
    "es_marks",
    "en_a_an",
    "en_of_have",
    "en_than",
    "en_youre",
    "en_i",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Word,
    Num,
    Punct,
    Space,
}

#[derive(Clone, Debug)]
pub(crate) struct Tok<'a> {
    pub r: Range<usize>,
    pub s: &'a str,
    pub kind: Kind,
}

impl Tok<'_> {
    pub fn is(&self, w: &str) -> bool {
        self.kind == Kind::Word && (self.s.eq_ignore_ascii_case(w) || (!self.s.is_ascii() && self.s.to_lowercase() == w))
    }

    pub fn lower(&self) -> String {
        self.s.to_lowercase()
    }

    /// Plain spaces, no line break or tab.
    pub fn is_gap(&self) -> bool {
        self.kind == Kind::Space && self.s.chars().all(|c| c == ' ' || c == '\u{A0}')
    }

    pub fn starts_upper(&self) -> bool {
        self.s.chars().next().is_some_and(char::is_uppercase)
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\u{AD}' || ('\u{300}'..='\u{36F}').contains(&c)
}

/// Words (with inner apostrophes and hyphens), numbers, single punctuation
/// marks and runs of spaces.
pub(crate) fn tokens(text: &str) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let kind = if c.is_whitespace() {
            while it.peek().is_some_and(|x| x.1.is_whitespace()) {
                it.next();
            }
            Kind::Space
        } else if is_word_char(c) {
            let mut digits = c.is_ascii_digit();
            loop {
                match it.peek().copied() {
                    Some((_, d)) if is_word_char(d) => {
                        digits &= d.is_ascii_digit();
                        it.next();
                    }
                    // Joined by an apostrophe or hyphen ("don't", "e-mail"), or
                    // digits by a point or comma ("3,5").
                    Some((j, d)) if matches!(d, '\'' | '’' | '-') || (digits && matches!(d, '.' | ',')) => {
                        let next = text[j + d.len_utf8()..].chars().next();
                        if next.is_some_and(|n| if digits && d != '-' { n.is_ascii_digit() } else { is_word_char(n) }) {
                            it.next();
                        } else {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            if digits { Kind::Num } else { Kind::Word }
        } else {
            Kind::Punct
        };
        let end = it.peek().map_or(text.len(), |x| x.0);
        out.push(Tok { r: i..end, s: &text[i..end], kind });
    }
    out
}

/// What the rules see: the text, its tokens and the dictionary.
pub(crate) struct Ctx<'a> {
    pub text: &'a str,
    pub toks: Vec<Tok<'a>>,
    pub dict: Option<&'a Dictionary>,
    pub out: Vec<Issue>,
}

impl<'a> Ctx<'a> {
    /// The token after `i`, past one gap of plain spaces.
    pub fn next_word(&self, i: usize) -> Option<usize> {
        let j = i + 1;
        let t = self.toks.get(j)?;
        if t.is_gap() {
            let k = j + 1;
            self.toks.get(k).filter(|t| t.kind == Kind::Word).map(|_| k)
        } else {
            None
        }
    }

    /// The word before `i`, past one gap.
    pub fn prev_word(&self, i: usize) -> Option<usize> {
        let j = i.checked_sub(1)?;
        if !self.toks[j].is_gap() {
            return None;
        }
        let k = j.checked_sub(1)?;
        (self.toks[k].kind == Kind::Word).then_some(k)
    }


    pub fn push(&mut self, range: Range<usize>, rule: &'static str, suggestions: Vec<String>) {
        if !self.out.iter().any(|x| x.range.start < range.end && range.start < x.range.end) {
            self.out.push(Issue { range, rule, suggestions });
        }
    }

    /// The text from token `a` to token `b`, both included.
    pub fn span(&self, a: usize, b: usize) -> Range<usize> {
        self.toks[a].r.start..self.toks[b].r.end
    }
}

/// `w` with its first letter as `like`'s ("el" like "De" -> "El").
pub(crate) fn case_like(w: &str, like: &str) -> String {
    if like.chars().next().is_some_and(char::is_uppercase) { capitalize(w) } else { w.to_string() }
}

pub(crate) fn capitalize(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The mistakes of a text (a paragraph) in a language ("es_ES"), by
/// position. `dict` is that language's dictionary: without it the rules
/// that need to know what a word is are skipped.
pub fn check_grammar(text: &str, lang: &str, dict: Option<&Dictionary>) -> Vec<Issue> {
    let lang = lang.split(['_', '-']).next().unwrap_or("").to_ascii_lowercase();
    let mut cx = Ctx { text, toks: tokens(text), dict, out: Vec::new() };
    match lang.as_str() {
        "es" => es::check(&mut cx),
        "en" => en::check(&mut cx),
        _ => {}
    }
    repeated(&mut cx, &lang);
    spacing(&mut cx, &lang);
    capitals(&mut cx, &lang);
    let mut out = cx.out;
    out.sort_by_key(|x| x.range.start);
    out
}

/// "el el", "the the".
fn repeated(cx: &mut Ctx, lang: &str) {
    // Doubled on purpose.
    let fine: &[&str] = match lang {
        "en" => &["had", "that", "is", "do", "bye", "no", "ha", "very", "so"],
        "es" => &["no", "ja", "je", "muy", "sí", "tal", "más"],
        _ => &["no"],
    };
    for i in 0..cx.toks.len() {
        let Some(j) = cx.next_word(i) else { continue };
        let (a, b) = (&cx.toks[i], &cx.toks[j]);
        if a.kind != Kind::Word || a.lower() != b.lower() || fine.contains(&a.lower().as_str()) {
            continue;
        }
        // "de A a Z", "Power Query query": another word, unless the first
        // only starts a sentence ("El el").
        let starts = (0..i).rev().find(|&k| cx.toks[k].kind != Kind::Space).is_none_or(|k| matches!(cx.toks[k].s, "." | "?" | "!" | "¿" | "¡" | ":" | "\"" | "«" | "—"));
        if a.s != b.s && !(starts && a.starts_upper() && b.s.chars().next().is_some_and(char::is_lowercase) && a.s.chars().count() > 1) {
            continue;
        }
        let (r, s) = (cx.span(i, j), a.s.to_string());
        cx.push(r, "repeated", vec![s]);
    }
}

const CLOSING: &[char] = &[',', '.', ';', ':', ')', ']', '?', '!'];

fn spacing(cx: &mut Ctx, lang: &str) {
    for i in 0..cx.toks.len() {
        let t = &cx.toks[i];
        // "hola , adiós": not in French, which spaces ; : ! ?.
        if t.is_gap() && i > 0 && cx.toks[i - 1].kind != Kind::Space {
            if let Some(p) = cx.toks.get(i + 1)
                && p.kind == Kind::Punct
                && let Some(c) = p.s.chars().next()
                && CLOSING.contains(&c)
                && !(lang == "fr" && matches!(c, ';' | ':' | '!' | '?'))
                && matches!(cx.toks[i - 1].kind, Kind::Word | Kind::Num)
                // An ellipsis, or a word right after it (" .5", " .xlsm").
                && !cx.toks.get(i + 2).is_some_and(|n| n.s.starts_with(['.', '…']) || matches!(n.kind, Kind::Word | Kind::Num) || n.s == "{")
                // "Usa ? para", "escribiendo ![[": the sign itself.
                && !(matches!(c, '?' | '!') && cx.toks.get(i + 2).is_some_and(|n| !(n.kind == Kind::Space && cx.toks.get(i + 3).is_none_or(|w| !w.s.chars().next().is_some_and(char::is_lowercase))) && !matches!(n.s, "»" | "\"" | "”" | ")")))
            {
                let (r, s) = (t.r.start..p.r.end, p.s.to_string());
                cx.push(r, "space_before", vec![s]);
                continue;
            }
            // Two spaces between words.
            if t.s.chars().count() >= 2 && t.s.chars().all(|c| c == ' ') && i + 1 < cx.toks.len() {
                let r = t.r.clone();
                cx.push(r, "double_space", vec![" ".into()]);
            }
            continue;
        }
        // "hola,adiós" and "fin.Luego".
        if t.kind == Kind::Punct
            && i > 0
            && let (Some(a), Some(b)) = (cx.toks.get(i - 1), cx.toks.get(i + 1))
            && a.kind == Kind::Word
            && b.kind == Kind::Word
            // "Range.Value", "ThisWorkbook.RefreshAll": code.
            && (matches!(t.s, "," | ";") || (matches!(t.s, "." | "?" | "!") && b.starts_upper() && !a.starts_upper() && a.s.chars().count() > 1 && b.s.chars().count() > 1 && !b.s.chars().skip(1).any(char::is_uppercase)))
            && !in_address(cx.text, t.r.start)
            // "U.S.A.", "e.g.", "file.Txt".
            && !cx.toks.get(i + 2).is_some_and(|n| n.s == "." || n.s == "(")
            && !a.s.chars().all(char::is_uppercase)
            // Code goes on with punctuation: "dest.Value = src.Value)".
            && cx.toks.get(i + 2).is_none_or(|n| n.is_gap() && cx.toks.get(i + 3).is_some_and(|w| w.kind == Kind::Word))
            // "destino.Value": a word the dictionary doesn't know is code.
            && (t.s == "," || t.s == ";" || cx.dict.is_none_or(|d| d.check(b.s)))
        {
            let (r, s) = (t.r.clone(), format!("{} ", t.s));
            cx.push(r, "space_after", vec![s]);
        }
    }
}

/// Inside a web or mail address or a file name.
fn in_address(text: &str, at: usize) -> bool {
    let a = text[..at].rfind(char::is_whitespace).map_or(0, |i| i + 1);
    let b = text[at..].find(char::is_whitespace).map_or(text.len(), |i| at + i);
    let chunk = &text[a..b];
    chunk.contains('@') || chunk.contains('`') || chunk.contains("://") || chunk.contains("www.") || chunk.contains('/') || chunk.contains('\\')
}

/// Abbreviations that end in a point without ending the sentence.
const ABBREVIATIONS: &[&str] = &[
    "sr", "sra", "srta", "sres", "dr", "dra", "dña", "d", "ud", "uds", "vd", "vds", "etc", "pág", "págs", "núm", "nº", "art", "cap", "ej", "aprox", "av", "avda", "cía", "dpto", "ed", "fig", "ib", "íd", "op", "cit",
    "tel", "vol", "vols", "vs", "p", "pp", "mr", "mrs", "ms", "prof", "st", "jr", "sr", "approx", "no", "nos", "ch", "cf", "al", "inc", "ltd", "co", "corp", "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep", "sept", "oct",
    "nov", "dec", "ene", "abr", "ago", "dic", "lic", "ing", "arq", "gral", "pres", "min", "máx", "mín", "max", "ca", "approx", "dept", "est", "fr", "gen", "gov", "hon", "rev", "sgt", "capt", "col", "lt", "mt", "ft", "pt", "u",
    "s", "a", "c", "e", "i", "m",
];

/// "Hola. adiós": a sentence starts with a capital letter.
fn capitals(cx: &mut Ctx, lang: &str) {
    if !matches!(lang, "es" | "en" | "pt" | "fr" | "it" | "de" | "ca" | "gl" | "nl") {
        return;
    }
    for i in 0..cx.toks.len() {
        if cx.toks[i].s != "." {
            continue;
        }
        let Some(j) = cx.next_word(i) else { continue };
        let w = &cx.toks[j];
        let first = w.s.chars().next().unwrap_or(' ');
        // "iPhone", "eBay", "x2".
        if !first.is_lowercase() || w.s.chars().skip(1).any(|c| c.is_uppercase() || c.is_ascii_digit()) {
            continue;
        }
        // An ellipsis, a number ("1. de"), an abbreviation ("p. ej.").
        let Some(p) = i.checked_sub(1) else { continue };
        let prev = &cx.toks[p];
        if prev.kind != Kind::Word || prev.s.chars().count() == 1 || ABBREVIATIONS.contains(&prev.lower().as_str()) || prev.s.chars().all(char::is_uppercase) {
            continue;
        }
        if p > 0 && cx.toks[p - 1].s == "." {
            continue;
        }
        // Followed by a point itself ("ej.") or in an address.
        if cx.toks.get(j + 1).is_some_and(|n| n.s == ".") || in_address(cx.text, w.r.start) {
            continue;
        }
        let (r, s) = (w.r.clone(), capitalize(w.s));
        cx.push(r, "capital", vec![s]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(text: &str, lang: &str) -> Vec<(&'static str, String, Vec<String>)> {
        check_grammar(text, lang, None).into_iter().map(|i| (i.rule, text[i.range].to_string(), i.suggestions)).collect()
    }

    #[test]
    fn tokens_keep_words_and_numbers_whole() {
        let t: Vec<&str> = tokens("Don't  pay 3,50 €, e-mail.").iter().map(|t| t.s).collect();
        assert_eq!(t, ["Don't", "  ", "pay", " ", "3,50", " ", "€", ",", " ", "e-mail", "."]);
    }

    #[test]
    fn repeated_words() {
        assert_eq!(rules("Vino el el lunes", "es_ES"), [("repeated", "el el".into(), vec!["el".into()])]);
        assert!(rules("He said that that was fine, no no. A Power Query query.", "en_US").is_empty());
        assert_eq!(rules("El el perro", "es")[0].2, ["El"]);
        // Not across a line or a comma.
        assert!(rules("el\nel, el", "es").is_empty());
    }

    #[test]
    fn spaces_and_punctuation() {
        assert_eq!(rules("Hola , adiós", "es")[0], ("space_before", " ,".into(), vec![",".into()]));
        assert_eq!(rules("Hola,adiós", "es")[0], ("space_after", ",".into(), vec![", ".into()]));
        assert_eq!(rules("Llegó al fin.Luego", "es")[0], ("space_after", ".".into(), vec![". ".into()]));
        assert_eq!(rules("dos  espacios", "es")[0], ("double_space", "  ".into(), vec![" ".into()]));
        // Numbers, addresses, ellipses, French.
        assert!(rules("Cuesta 3,50 o 1.000. Ver www.apex.org/a,b o a@b.com, e.g. U.S.A. y bien ...", "es").is_empty());
        assert!(rules("Bonjour ; ça va ?", "fr").is_empty());
    }

    #[test]
    fn capital_after_a_point() {
        assert_eq!(rules("Vine. luego fui.", "es"), [("capital", "luego".into(), vec!["Luego".into()])]);
        assert!(rules("Lo dijo el Sr. García y el Dr. lópez, p. ej. aquí, etc. y más. 1. uno. iPhone. Es así... y no.", "es").is_empty());
        assert!(rules("Vine. luego", "ja").is_empty());
    }
}
