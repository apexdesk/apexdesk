//! Parsing of Hunspell .aff files into the tables `check`/`suggest` use.
//!
//! Parsing is lenient: unknown directives and malformed lines are skipped, as
//! Hunspell does, so a slightly broken dictionary still works.

use std::borrow::Cow;

use crate::hash::{FxHashMap, FxHashSet};

pub(crate) type Flag = u32;

/// Internal flag of the hidden Capitalized copies of mixed-case and
/// upper-case stems (Hunspell's ONLYUPCASEFLAG); never produced by parsing
/// (character flags stop at 0x10FFFF, long ones repeat a 16-bit pattern).
pub(crate) const ONLYUPCASE: Flag = 0xFFFF_FFF0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FlagMode {
    /// One character per flag (the default).
    Char,
    /// Two characters per flag.
    Long,
    /// Comma-separated decimal numbers.
    Num,
    /// One Unicode character per flag.
    Utf8,
}

/// Interned, sorted flag sets. Index 0 is always the empty set.
pub(crate) struct FlagSets {
    sets: Vec<Box<[Flag]>>,
    index: FxHashMap<Box<[Flag]>, u32>,
}

impl FlagSets {
    pub(crate) fn new() -> FlagSets {
        let mut s = FlagSets { sets: Vec::new(), index: FxHashMap::default() };
        s.intern(Vec::new());
        s
    }

    pub(crate) fn intern(&mut self, mut v: Vec<Flag>) -> u32 {
        v.sort_unstable();
        v.dedup();
        if let Some(&i) = self.index.get(v.as_slice()) {
            return i;
        }
        let i = self.sets.len() as u32;
        let b: Box<[Flag]> = v.into_boxed_slice();
        self.index.insert(b.clone(), i);
        self.sets.push(b);
        i
    }

    /// Drops the interning index once loading is done.
    pub(crate) fn freeze(&mut self) {
        self.index = FxHashMap::default();
        self.sets.shrink_to_fit();
    }

    #[inline]
    pub(crate) fn get(&self, i: u32) -> &[Flag] {
        self.sets.get(i as usize).map(|b| &**b).unwrap_or(&[])
    }

    #[inline]
    pub(crate) fn has(&self, i: u32, f: Flag) -> bool {
        self.get(i).binary_search(&f).is_ok()
    }

    #[inline]
    pub(crate) fn has_opt(&self, i: u32, f: Option<Flag>) -> bool {
        f.is_some_and(|f| self.has(i, f))
    }
}

pub(crate) enum CondElem {
    Any,
    Char(char),
    In(Box<[char]>),
    NotIn(Box<[char]>),
}

impl CondElem {
    #[inline]
    fn matches(&self, c: char) -> bool {
        match self {
            CondElem::Any => true,
            CondElem::Char(x) => *x == c,
            CondElem::In(set) => set.contains(&c),
            CondElem::NotIn(set) => !set.contains(&c),
        }
    }
}

fn parse_cond(s: &str) -> Box<[CondElem]> {
    if s == "." {
        return Box::new([]);
    }
    let mut out = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '.' => out.push(CondElem::Any),
            '[' => {
                let mut neg = false;
                let mut set = Vec::new();
                let mut first = true;
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    if first && c == '^' {
                        neg = true;
                    } else {
                        set.push(c);
                    }
                    first = false;
                }
                let set = set.into_boxed_slice();
                out.push(if neg { CondElem::NotIn(set) } else { CondElem::In(set) });
            }
            c => out.push(CondElem::Char(c)),
        }
    }
    out.into_boxed_slice()
}

pub(crate) struct Affix {
    pub flag: Flag,
    pub cross: bool,
    pub strip: Box<str>,
    pub append: Box<str>,
    cond: Box<[CondElem]>,
    /// Continuation class (flag set index).
    pub cont: u32,
}

impl Affix {
    /// Suffix condition on the stem `head + strip`.
    #[inline]
    pub(crate) fn sfx_cond(&self, head: &str) -> bool {
        let mut it = self.strip.chars().rev().chain(head.chars().rev());
        self.cond.iter().rev().all(|c| it.next().is_some_and(|ch| c.matches(ch)))
    }

    /// Prefix condition on the stem `strip + tail`.
    #[inline]
    pub(crate) fn pfx_cond(&self, tail: &str) -> bool {
        let mut it = self.strip.chars().chain(tail.chars());
        self.cond.iter().all(|c| it.next().is_some_and(|ch| c.matches(ch)))
    }
}

/// Affixes grouped by their appended text, so a word only visits the entries
/// whose affix it actually ends (or starts) with.
#[derive(Default)]
pub(crate) struct AffixIndex {
    map: FxHashMap<Box<str>, Vec<u32>>,
    max_len: usize,
}

impl AffixIndex {
    fn build(affixes: &[Affix]) -> AffixIndex {
        let mut idx = AffixIndex::default();
        for (i, a) in affixes.iter().enumerate() {
            idx.max_len = idx.max_len.max(a.append.len());
            idx.map.entry(a.append.clone()).or_default().push(i as u32);
        }
        // Entries sharing a strip produce the same stem: keep them adjacent.
        for v in idx.map.values_mut() {
            v.sort_by(|&a, &b| affixes[a as usize].strip.cmp(&affixes[b as usize].strip));
        }
        idx
    }

    /// Calls `f(boundary, entries)` for every suffix group matching the end of
    /// `word`; `boundary` is where the appended text starts. Stops when `f`
    /// returns true.
    #[inline]
    pub(crate) fn suffixes(&self, word: &str, fullstrip: bool, mut f: impl FnMut(usize, &[u32]) -> bool) -> bool {
        let min = if fullstrip { 0 } else { 1 };
        let lo = word.len().saturating_sub(self.max_len).max(min);
        for i in (lo..=word.len()).rev() {
            if !word.is_char_boundary(i) {
                continue;
            }
            if let Some(v) = self.map.get(&word[i..])
                && f(i, v)
            {
                return true;
            }
        }
        false
    }

    /// Like [`suffixes`](Self::suffixes) for the start of the word; the
    /// boundary is where the prefix ends.
    #[inline]
    pub(crate) fn prefixes(&self, word: &str, fullstrip: bool, mut f: impl FnMut(usize, &[u32]) -> bool) -> bool {
        let hi = if fullstrip { word.len() } else { word.len().saturating_sub(1) };
        let hi = hi.min(self.max_len);
        for i in 0..=hi {
            if !word.is_char_boundary(i) {
                continue;
            }
            if let Some(v) = self.map.get(&word[..i])
                && f(i, v)
            {
                return true;
            }
        }
        false
    }
}

/// A REP/ICONV/OCONV style replacement.
pub(crate) struct Rep {
    pub from: String,
    pub to: String,
    pub at_start: bool,
    pub at_end: bool,
}

/// ICONV/OCONV: at each position the longest matching pattern wins.
#[derive(Default)]
pub(crate) struct ConvTable {
    pairs: Vec<(String, String)>,
}

impl ConvTable {
    pub(crate) fn apply<'a>(&self, s: &'a str) -> Cow<'a, str> {
        if self.pairs.is_empty() || !self.pairs.iter().any(|(f, _)| s.contains(f.as_str())) {
            return Cow::Borrowed(s);
        }
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < s.len() {
            let rest = &s[i..];
            let best = self
                .pairs
                .iter()
                .filter(|(f, _)| !f.is_empty() && rest.starts_with(f.as_str()))
                .max_by_key(|(f, _)| f.len());
            match best {
                Some((f, t)) => {
                    out.push_str(t);
                    i += f.len();
                }
                None => {
                    let c = rest.chars().next().unwrap_or('\0');
                    out.push(c);
                    i += c.len_utf8().max(1);
                }
            }
        }
        Cow::Owned(out)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Quant {
    One,
    Star,
    Opt,
}

pub(crate) struct Aff {
    pub flag_mode: FlagMode,
    aliases: Vec<u32>,
    pub prefixes: Vec<Affix>,
    pub suffixes: Vec<Affix>,
    pub pidx: AffixIndex,
    pub sidx: AffixIndex,
    /// Flags that occur in some suffix's continuation class (twofold suffixes).
    pub sfx_cont_flags: FxHashSet<Flag>,
    pub pfx_by_flag: FxHashMap<Flag, Vec<u32>>,
    pub sfx_by_flag: FxHashMap<Flag, Vec<u32>>,

    pub forbidden: Option<Flag>,
    pub nosuggest: Option<Flag>,
    pub keepcase: Option<Flag>,
    pub needaffix: Option<Flag>,
    pub onlyincompound: Option<Flag>,
    pub circumfix: Option<Flag>,
    pub compoundflag: Option<Flag>,
    pub compoundbegin: Option<Flag>,
    pub compoundmiddle: Option<Flag>,
    pub compoundend: Option<Flag>,
    pub compoundpermit: Option<Flag>,
    pub compoundforbid: Option<Flag>,
    pub compound_min: usize,
    pub compound_word_max: Option<usize>,
    pub check_compound_dup: bool,
    pub compound_rules: Vec<Vec<(Flag, Quant)>>,
    pub compound_rule_flags: FxHashSet<Flag>,
    pub fullstrip: bool,

    pub iconv: ConvTable,
    pub oconv: ConvTable,
    pub ignore: Vec<char>,
    pub wordchars: String,
    pub breaks: Vec<String>,
    pub rep: Vec<Rep>,
    pub map: Vec<Vec<String>>,
    pub key: Vec<Vec<char>>,
    pub try_chars: Vec<char>,
    pub lang: String,
    pub nosplitsugs: bool,
    pub maxngramsugs: usize,
}

impl Aff {
    pub(crate) fn has_compounding(&self) -> bool {
        self.compoundflag.is_some() || self.compoundbegin.is_some() || self.compoundmiddle.is_some() || self.compoundend.is_some()
    }

    fn parse_flag(&self, s: &str) -> Option<Flag> {
        parse_flag(self.flag_mode, s)
    }

    /// Parses the flag field of a .dic line or an affix continuation class,
    /// resolving AF aliases.
    pub(crate) fn parse_flags(&self, s: &str, sets: &mut FlagSets) -> u32 {
        if !self.aliases.is_empty()
            && let Ok(n) = s.trim().parse::<usize>()
        {
            return n.checked_sub(1).and_then(|i| self.aliases.get(i)).copied().unwrap_or(0);
        }
        sets.intern(decode_flags(self.flag_mode, s))
    }
}

fn parse_flag(mode: FlagMode, s: &str) -> Option<Flag> {
    let s = s.trim();
    match mode {
        FlagMode::Char | FlagMode::Utf8 => s.chars().next().map(|c| c as u32),
        FlagMode::Long => {
            let mut it = s.chars();
            let a = it.next()? as u32;
            let b = it.next().map(|c| c as u32).unwrap_or(0);
            Some(((a & 0xFFFF) << 16) | (b & 0xFFFF))
        }
        FlagMode::Num => s.parse::<u32>().ok().filter(|&n| n > 0),
    }
}

fn decode_flags(mode: FlagMode, s: &str) -> Vec<Flag> {
    match mode {
        FlagMode::Char | FlagMode::Utf8 => s.chars().map(|c| c as u32).collect(),
        FlagMode::Long => {
            let chars: Vec<char> = s.chars().collect();
            chars
                .chunks(2)
                .map(|p| {
                    let a = p[0] as u32;
                    let b = p.get(1).map(|&c| c as u32).unwrap_or(0);
                    ((a & 0xFFFF) << 16) | (b & 0xFFFF)
                })
                .collect()
        }
        FlagMode::Num => s.split(',').filter_map(|p| p.trim().parse::<u32>().ok()).filter(|&n| n > 0).collect(),
    }
}

fn parse_rule(mode: FlagMode, s: &str) -> Vec<(Flag, Quant)> {
    let mut out: Vec<(Flag, Quant)> = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '?' => {
                if let Some(last) = out.last_mut() {
                    last.1 = if c == '*' { Quant::Star } else { Quant::Opt };
                }
            }
            '(' => {
                let inner: String = chars.by_ref().take_while(|&c| c != ')').collect();
                if let Some(f) = parse_flag(mode, &inner) {
                    out.push((f, Quant::One));
                }
            }
            c => {
                if mode == FlagMode::Long {
                    // Long flags outside parentheses: take two characters.
                    let b = chars.next().map(|c| c as u32).unwrap_or(0);
                    out.push(((((c as u32) & 0xFFFF) << 16) | (b & 0xFFFF), Quant::One));
                } else {
                    out.push((c as u32, Quant::One));
                }
            }
        }
    }
    out
}

fn parse_rep(from: &str, to: &str) -> Rep {
    let mut from = from;
    let at_start = from.starts_with('^');
    if at_start {
        from = &from[1..];
    }
    let at_end = from.ends_with('$') && !from.is_empty();
    if at_end {
        from = &from[..from.len() - 1];
    }
    Rep { from: from.replace('_', " "), to: to.replace('_', " "), at_start, at_end }
}

/// Directives whose first line is an entry count (`REP 20`) followed by that
/// many entries.
const TABLES: &[&str] =
    &["REP", "MAP", "ICONV", "OCONV", "BREAK", "AF", "AM", "COMPOUNDRULE", "PHONE", "CHECKCOMPOUNDPATTERN"];

pub(crate) fn parse(text: &str, sets: &mut FlagSets) -> Aff {
    let mut aff = Aff {
        flag_mode: FlagMode::Char,
        aliases: Vec::new(),
        prefixes: Vec::new(),
        suffixes: Vec::new(),
        pidx: AffixIndex::default(),
        sidx: AffixIndex::default(),
        sfx_cont_flags: FxHashSet::default(),
        pfx_by_flag: FxHashMap::default(),
        sfx_by_flag: FxHashMap::default(),
        forbidden: None,
        nosuggest: None,
        keepcase: None,
        needaffix: None,
        onlyincompound: None,
        circumfix: None,
        compoundflag: None,
        compoundbegin: None,
        compoundmiddle: None,
        compoundend: None,
        compoundpermit: None,
        compoundforbid: None,
        compound_min: 3,
        compound_word_max: None,
        check_compound_dup: false,
        compound_rules: Vec::new(),
        compound_rule_flags: FxHashSet::default(),
        fullstrip: false,
        iconv: ConvTable::default(),
        oconv: ConvTable::default(),
        ignore: Vec::new(),
        wordchars: String::new(),
        breaks: Vec::new(),
        rep: Vec::new(),
        map: Vec::new(),
        key: Vec::new(),
        try_chars: Vec::new(),
        lang: String::new(),
        nosplitsugs: false,
        maxngramsugs: 4,
    };
    let mut breaks_set = false;
    let mut key_set = false;
    // Remaining entry lines of the current table directive.
    let mut pending: FxHashMap<&str, usize> = FxHashMap::default();
    // Affix header state: flag -> (cross product, entries left).
    let mut affix_hdr: FxHashMap<(bool, Flag), (bool, usize)> = FxHashMap::default();

    for raw in text.lines() {
        let line = raw.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = trimmed.split_whitespace().collect();
        let Some(&dir) = f.first() else { continue };
        let arg = f.get(1).copied().unwrap_or("");

        if TABLES.contains(&dir) {
            let left = pending.get(dir).copied().unwrap_or(0);
            if left == 0 && f.len() == 2 && arg.parse::<usize>().is_ok() {
                pending.insert(dir, arg.parse().unwrap_or(0));
                if dir == "BREAK" {
                    breaks_set = true;
                }
                continue;
            }
            if left > 0 {
                pending.insert(dir, left - 1);
            }
        }

        match dir {
            "SET" => {}
            "FLAG" => {
                aff.flag_mode = match arg {
                    "long" => FlagMode::Long,
                    "num" => FlagMode::Num,
                    "UTF-8" | "utf-8" | "UTF8" => FlagMode::Utf8,
                    _ => FlagMode::Char,
                }
            }
            "AF" => {
                let v = decode_flags(aff.flag_mode, arg);
                aff.aliases.push(sets.intern(v));
            }
            "PFX" | "SFX" => {
                let is_pfx = dir == "PFX";
                let Some(flag) = aff.parse_flag(arg) else { continue };
                let key = (is_pfx, flag);
                let left = affix_hdr.get(&key).map(|h| h.1).unwrap_or(0);
                if left == 0 {
                    // Header: PFX flag Y|N count
                    if f.len() >= 4 {
                        let cross = f[2] == "Y";
                        let n = f[3].parse::<usize>().unwrap_or(0);
                        affix_hdr.insert(key, (cross, n));
                    }
                    continue;
                }
                let cross = affix_hdr.get(&key).map(|h| h.0).unwrap_or(false);
                affix_hdr.insert(key, (cross, left - 1));
                if f.len() < 4 {
                    continue;
                }
                let strip = if f[2] == "0" { "" } else { f[2] };
                let (app, cont) = match split_unescaped_slash(f[3]) {
                    Some((a, c)) => (a, Some(c)),
                    None => (f[3], None),
                };
                let app = if app == "0" { "" } else { app };
                let cond = f.get(4).copied().unwrap_or(".");
                let cont = cont.map(|c| aff.parse_flags(c, sets)).unwrap_or(0);
                let strip: String = strip.chars().filter(|c| !aff.ignore.contains(c)).collect();
                let app: String = app.chars().filter(|c| !aff.ignore.contains(c)).collect();
                let a = Affix {
                    flag,
                    cross,
                    strip: strip.into(),
                    append: app.into(),
                    cond: parse_cond(cond),
                    cont,
                };
                if is_pfx { aff.prefixes.push(a) } else { aff.suffixes.push(a) }
            }
            "FORBIDDENWORD" => aff.forbidden = aff.parse_flag(arg),
            "NOSUGGEST" => aff.nosuggest = aff.parse_flag(arg),
            "KEEPCASE" => aff.keepcase = aff.parse_flag(arg),
            "NEEDAFFIX" | "PSEUDOROOT" => aff.needaffix = aff.parse_flag(arg),
            "ONLYINCOMPOUND" => aff.onlyincompound = aff.parse_flag(arg),
            "CIRCUMFIX" => aff.circumfix = aff.parse_flag(arg),
            "COMPOUNDFLAG" => aff.compoundflag = aff.parse_flag(arg),
            "COMPOUNDBEGIN" => aff.compoundbegin = aff.parse_flag(arg),
            "COMPOUNDMIDDLE" => aff.compoundmiddle = aff.parse_flag(arg),
            "COMPOUNDEND" | "COMPOUNDLAST" => aff.compoundend = aff.parse_flag(arg),
            "COMPOUNDPERMITFLAG" => aff.compoundpermit = aff.parse_flag(arg),
            "COMPOUNDFORBIDFLAG" => aff.compoundforbid = aff.parse_flag(arg),
            "COMPOUNDMIN" => aff.compound_min = arg.parse::<usize>().unwrap_or(3).max(1),
            "COMPOUNDWORDMAX" => aff.compound_word_max = arg.parse::<usize>().ok(),
            "CHECKCOMPOUNDDUP" => aff.check_compound_dup = true,
            "COMPOUNDRULE" => {
                let rule = parse_rule(aff.flag_mode, arg);
                if !rule.is_empty() {
                    aff.compound_rule_flags.extend(rule.iter().map(|r| r.0));
                    aff.compound_rules.push(rule);
                }
            }
            "FULLSTRIP" => aff.fullstrip = true,
            "NOSPLITSUGS" => aff.nosplitsugs = true,
            "MAXNGRAMSUGS" => aff.maxngramsugs = arg.parse().unwrap_or(4),
            "ICONV" | "OCONV" => {
                if let Some(&to) = f.get(2) {
                    let t = if dir == "ICONV" { &mut aff.iconv } else { &mut aff.oconv };
                    t.pairs.push((arg.replace('_', " "), to.replace('_', " ")));
                }
            }
            "IGNORE" => aff.ignore = arg.chars().collect(),
            "WORDCHARS" => aff.wordchars = arg.to_string(),
            "BREAK" => {
                breaks_set = true;
                // Each pattern makes a word's check branch.
                if !arg.is_empty() && aff.breaks.len() < 64 {
                    aff.breaks.push(arg.to_string());
                }
            }
            "REP" => {
                if let Some(&to) = f.get(2)
                    && aff.rep.len() < 8192
                {
                    aff.rep.push(parse_rep(arg, to));
                }
            }
            "MAP" => {
                let mut group = Vec::new();
                let mut chars = arg.chars();
                while let Some(c) = chars.next() {
                    if c == '(' {
                        let inner: String = chars.by_ref().take_while(|&c| c != ')').collect();
                        if !inner.is_empty() {
                            group.push(inner);
                        }
                    } else {
                        group.push(c.to_string());
                    }
                }
                if group.len() > 1 {
                    if aff.map.len() < 256 {
                        aff.map.push(group);
                    }
                }
            }
            "KEY" => {
                key_set = true;
                aff.key = arg.split('|').map(|r| r.chars().collect()).collect();
            }
            // Each character a candidate per position of the word.
            "TRY" => aff.try_chars = arg.chars().take(256).collect(),
            "LANG" => aff.lang = arg.to_string(),
            _ => {}
        }
    }

    if !breaks_set {
        aff.breaks = vec!["-".into(), "^-".into(), "-$".into()];
    }
    if !key_set {
        aff.key = ["qwertyuiop", "asdfghjkl", "zxcvbnm"].iter().map(|r| r.chars().collect()).collect();
    }
    aff.pidx = AffixIndex::build(&aff.prefixes);
    aff.sidx = AffixIndex::build(&aff.suffixes);
    for (i, s) in aff.suffixes.iter().enumerate() {
        aff.sfx_cont_flags.extend(sets.get(s.cont).iter().copied());
        aff.sfx_by_flag.entry(s.flag).or_default().push(i as u32);
    }
    for (i, p) in aff.prefixes.iter().enumerate() {
        aff.pfx_by_flag.entry(p.flag).or_default().push(i as u32);
    }
    aff
}

/// Splits "text/flags" at the first slash that isn't escaped as "\/".
pub(crate) fn split_unescaped_slash(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut i = 1; // a leading slash belongs to the word
    while i < b.len() {
        if b[i] == b'/' && b[i - 1] != b'\\' {
            return Some((&s[..i], &s[i + 1..]));
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions() {
        let c = parse_cond("[^aeiou]y");
        let a = Affix {
            flag: 1,
            cross: true,
            strip: "y".into(),
            append: "ies".into(),
            cond: c,
            cont: 0,
        };
        assert!(a.sfx_cond("tr"));
        assert!(!a.sfx_cond("pla"));
        assert!(!a.sfx_cond(""));
        let p = Affix {
            flag: 1,
            cross: true,
            strip: "".into(),
            append: "in".into(),
            cond: parse_cond("[^bp]"),
            cont: 0,
        };
        assert!(p.pfx_cond("capaz"));
        assert!(!p.pfx_cond("posible"));
    }

    #[test]
    fn flags_and_rules() {
        assert_eq!(decode_flags(FlagMode::Long, "AaBb"), vec![(65 << 16) | 97, (66 << 16) | 98]);
        assert_eq!(decode_flags(FlagMode::Num, "1,22,333"), vec![1, 22, 333]);
        assert_eq!(decode_flags(FlagMode::Utf8, "áé"), vec!['á' as u32, 'é' as u32]);
        let r = parse_rule(FlagMode::Char, "n*1t");
        assert_eq!(r, vec![('n' as u32, Quant::Star), ('1' as u32, Quant::One), ('t' as u32, Quant::One)]);
        let r = parse_rule(FlagMode::Num, "(1)(22)?");
        assert_eq!(r, vec![(1, Quant::One), (22, Quant::Opt)]);
        assert_eq!(split_unescaped_slash("a\\/b/XY"), Some(("a\\/b", "XY")));
    }

    #[test]
    fn conversion_prefers_longest() {
        let t = ConvTable { pairs: vec![("a".into(), "1".into()), ("ab".into(), "2".into())] };
        assert_eq!(t.apply("xaby a"), "x2y 1");
        assert!(matches!(t.apply("zzz"), Cow::Borrowed(_)));
    }
}
