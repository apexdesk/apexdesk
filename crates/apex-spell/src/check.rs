//! Word checking: case handling, affix stripping (including cross products
//! and twofold suffixes), compounding and BREAK splitting, after Hunspell.

use std::borrow::Cow;
use std::cell::Cell;

use crate::Dictionary;
use crate::aff::{Affix, Flag, ONLYUPCASE, Quant};
use crate::casing::{Cap, captype, initcap_lower, lower};

/// Hunspell refuses longer words; so do we (keeps pathological input cheap).
pub(crate) const MAX_WORD_CHARS: usize = 100;
const MAX_BREAK_DEPTH: usize = 10;
/// Root "flag set" reported for compounds (no single root to test KEEPCASE on).
const COMPOUND_ROOT: u32 = u32::MAX;

/// One way of deriving a word from a dictionary stem.
pub(crate) struct Hit<'a> {
    pub root: u32,
    pub pfx: Option<&'a Affix>,
    /// The outer suffix.
    pub sfx: Option<&'a Affix>,
    /// The inner suffix of a twofold suffix ("compra" + "ción" + "es").
    pub sfx2: Option<&'a Affix>,
}

impl Hit<'_> {
    fn affixes(&self) -> impl Iterator<Item = &Affix> {
        self.pfx.into_iter().chain(self.sfx).chain(self.sfx2)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Begin,
    Middle,
    End,
}

#[inline]
fn concat(a: &str, b: &str) -> String {
    let mut s = String::with_capacity(a.len() + b.len());
    s.push_str(a);
    s.push_str(b);
    s
}

/// Numbers such as 1990, 3.5, 1,000 or 10-20 are always correct.
pub(crate) fn is_number(w: &str) -> bool {
    let mut prev_digit = false;
    let mut any = false;
    for c in w.chars() {
        if c.is_ascii_digit() {
            prev_digit = true;
            any = true;
        } else if matches!(c, '.' | ',' | '-') && prev_digit {
            prev_digit = false;
        } else {
            return false;
        }
    }
    any && prev_digit
}

impl Dictionary {
    /// Is the word correct? Handles case ("Hello", "HELLO" and "hello" per
    /// Hunspell rules, KEEPCASE), words with apostrophes and dots, hyphenated
    /// words (BREAK), and treats numbers like 1990 or 3.5 as correct.
    pub fn check(&self, word: &str) -> bool {
        let word = word.trim();
        if word.is_empty() {
            return true;
        }
        let w = self.normalize(word);
        if w.chars().count() > MAX_WORD_CHARS {
            return false;
        }
        budgeted(|| self.spell(&w, 0))
    }

    /// ICONV conversion and IGNORE removal, applied to every input word.
    pub(crate) fn normalize<'a>(&self, word: &'a str) -> Cow<'a, str> {
        // Soft hyphens are invisible hyphenation hints, never part of the word.
        let word: Cow<str> = if word.contains('\u{AD}') { Cow::Owned(word.replace('\u{AD}', "")) } else { Cow::Borrowed(word) };
        let w = match word {
            Cow::Borrowed(b) => self.aff.iconv.apply(b),
            Cow::Owned(o) => Cow::Owned(self.aff.iconv.apply(&o).into_owned()),
        };
        if self.aff.ignore.is_empty() || !w.chars().any(|c| self.aff.ignore.contains(&c)) {
            return w;
        }
        Cow::Owned(w.chars().filter(|c| !self.aff.ignore.contains(c)).collect())
    }

    fn spell(&self, w: &str, depth: usize) -> bool {
        if !step() {
            return false;
        }
        if is_number(w) {
            return true;
        }
        // Abbreviations: "etc." is fine if "etc" or "etc." is in the dictionary.
        let stripped = w.trim_end_matches('.');
        if stripped.is_empty() {
            return true;
        }
        if self.check_case(stripped, false) {
            return true;
        }
        if stripped.len() < w.len() && self.check_case(&concat(stripped, "."), false) {
            return true;
        }
        // Typographic apostrophe in dictionaries that only list the ASCII one.
        if stripped.contains('\u{2019}') {
            let a = stripped.replace('\u{2019}', "'");
            if self.check_case(&a, false) {
                return true;
            }
        }
        depth < MAX_BREAK_DEPTH && self.check_break(stripped, depth)
    }

    /// Hunspell's BREAK: "well-known" is correct when both parts are.
    fn check_break(&self, w: &str, depth: usize) -> bool {
        for pat in &self.aff.breaks {
            if let Some(p) = pat.strip_prefix('^') {
                if !p.is_empty() && w.len() > p.len() && w.starts_with(p) && self.spell(&w[p.len()..], depth + 1) {
                    return true;
                }
            } else if let Some(p) = pat.strip_suffix('$')
                && !p.is_empty()
            {
                if w.len() > p.len() && w.ends_with(p) && self.spell(&w[..w.len() - p.len()], depth + 1) {
                    return true;
                }
            }
        }
        for pat in &self.aff.breaks {
            if pat.starts_with('^') || (pat.ends_with('$') && pat.len() > 1) || pat.is_empty() {
                continue;
            }
            let Some(first) = w.find(pat.as_str()) else { continue };
            // Hunspell tries the second occurrence first, so dictionary words
            // that contain the break pattern themselves still match.
            let next = first + pat.chars().next().map_or(1, char::len_utf8);
            let second = w[next..].find(pat.as_str()).map(|i| i + next);
            for pos in second.into_iter().chain(Some(first)) {
                if pos == 0 || pos + pat.len() >= w.len() {
                    continue;
                }
                if self.spell(&w[pos + pat.len()..], depth + 1) && self.spell(&w[..pos], depth + 1) {
                    return true;
                }
            }
        }
        false
    }

    /// Checks `w` with Hunspell's capitalization rules (no BREAK splitting).
    /// `sug` excludes NOSUGGEST words (used when validating suggestions).
    pub(crate) fn check_case(&self, w: &str, sug: bool) -> bool {
        let keepcase = |r: u32| r != COMPOUND_ROOT && self.sets.has_opt(r, self.aff.keepcase);
        match captype(w) {
            Cap::No | Cap::Huh | Cap::HuhInit => self.check_word(w, sug, false).is_some(),
            Cap::Init => {
                if self.check_word(w, sug, true).is_some() {
                    return true;
                }
                matches!(self.check_word(&lower(w), sug, false), Some(r) if !keepcase(r))
            }
            Cap::All => {
                if self.check_word(w, sug, false).is_some() {
                    return true;
                }
                // "L'AMOUR" -> "l'Amour", "L'Amour"
                if let Some(pos) = w.find('\'')
                    && pos + 1 < w.len()
                {
                    let (a, b) = (&w[..=pos], &w[pos + 1..]);
                    let tail = initcap_lower(b);
                    for head in [lower(a), initcap_lower(a)] {
                        if matches!(self.check_word(&concat(&head, &tail), sug, false), Some(r) if !keepcase(r)) {
                            return true;
                        }
                    }
                }
                let ic = initcap_lower(w);
                if matches!(self.check_word(&ic, sug, false), Some(r) if !keepcase(r)) {
                    return true;
                }
                matches!(self.check_word(&lower(w), sug, false), Some(r) if !keepcase(r))
            }
        }
    }

    /// Exact-case lookup: plain stem, affixed forms, then compounds. Returns
    /// the root's flag set (to test KEEPCASE). `initcap` marks a word typed
    /// Capitalized, which must not match the hidden forms added for
    /// upper-case input ("Ios" from "iOS" only accepts "IOS").
    pub(crate) fn check_word(&self, w: &str, sug: bool, initcap: bool) -> Option<u32> {
        let a = &self.aff;
        if let Some(h) = self.words.get(w) {
            let hs = h.as_slice();
            if hs.iter().any(|&s| self.sets.has_opt(s, a.forbidden)) {
                return None;
            }
            for &s in hs {
                if !self.sets.has_opt(s, a.needaffix)
                    && !self.sets.has_opt(s, a.onlyincompound)
                    && !(sug && self.sets.has_opt(s, a.nosuggest))
                    && !(initcap && self.sets.has(s, ONLYUPCASE))
                {
                    return Some(s);
                }
            }
        }
        if let Some(r) = self.affixed(w, sug, initcap) {
            return Some(r);
        }
        if self.check_compound(w, sug) {
            return Some(COMPOUND_ROOT);
        }
        None
    }

    fn affixed(&self, w: &str, sug: bool, initcap: bool) -> Option<u32> {
        let a = &self.aff;
        let mut found = None;
        self.affix_search(w, true, true, &mut |hit| {
            let r = hit.root;
            if self.sets.has_opt(r, a.forbidden)
                || self.sets.has_opt(r, a.onlyincompound)
                || (sug && self.sets.has_opt(r, a.nosuggest))
                || (initcap && self.sets.has(r, ONLYUPCASE))
            {
                return false;
            }
            if hit.affixes().any(|x| self.sets.has_opt(x.cont, a.onlyincompound)) {
                return false;
            }
            if hit.affixes().all(|x| self.sets.has_opt(x.cont, a.needaffix)) {
                return false;
            }
            if !self.circumfix_ok(hit) {
                return false;
            }
            found = Some(r);
            true
        });
        found
    }

    fn circumfix_ok(&self, hit: &Hit) -> bool {
        let Some(c) = self.aff.circumfix else { return true };
        let p = hit.pfx.is_some_and(|x| self.sets.has(x.cont, c));
        let s = hit.sfx.is_some_and(|x| self.sets.has(x.cont, c)) || hit.sfx2.is_some_and(|x| self.sets.has(x.cont, c));
        p == s
    }

    #[inline]
    fn root_has(&self, r: u32, f: Flag) -> bool {
        self.sets.has(r, f)
    }

    /// Enumerates the ways `word` can be derived from a stem with affixes and
    /// calls `accept` for each until it returns true. Only the structural
    /// rules (flags on the stem, cross product, continuation classes) are
    /// enforced here; `accept` applies the context-specific ones.
    pub(crate) fn affix_search(
        &self,
        word: &str,
        allow_pfx: bool,
        allow_sfx: bool,
        accept: &mut dyn FnMut(&Hit) -> bool,
    ) -> bool {
        let a = &self.aff;
        let fullstrip = a.fullstrip;
        if allow_sfx
            && a.sidx.suffixes(word, fullstrip, |i, entries| {
                let head = &word[..i];
                // Entries are grouped by strip: build and look up each stem once.
                let mut cur: Option<&str> = None;
                let mut stem = String::new();
                let mut homs = None;
                for &e in entries {
                    if !step() {
                        return false;
                    }
                    let s = &a.suffixes[e as usize];
                    if !s.sfx_cond(head) {
                        continue;
                    }
                    if cur != Some(&*s.strip) {
                        cur = Some(&s.strip);
                        stem.clear();
                        stem.push_str(head);
                        stem.push_str(&s.strip);
                        homs = self.words.get(stem.as_str());
                    }
                    if let Some(h) = homs {
                        for &r in h.as_slice() {
                            if self.root_has(r, s.flag) && accept(&Hit { root: r, pfx: None, sfx: Some(s), sfx2: None }) {
                                return true;
                            }
                        }
                    }
                    if a.sfx_cont_flags.contains(&s.flag) && self.twofold(&stem, s, None, accept) {
                        return true;
                    }
                }
                false
            })
        {
            return true;
        }
        if !allow_pfx {
            return false;
        }
        a.pidx.prefixes(word, fullstrip, |i, entries| {
            let tail = &word[i..];
            for &e in entries {
                if !step() {
                    return false;
                }
                let p = &a.prefixes[e as usize];
                if !p.pfx_cond(tail) {
                    continue;
                }
                let stem = concat(&p.strip, tail);
                if let Some(h) = self.words.get(stem.as_str()) {
                    for &r in h.as_slice() {
                        if self.root_has(r, p.flag) && accept(&Hit { root: r, pfx: Some(p), sfx: None, sfx2: None }) {
                            return true;
                        }
                    }
                }
                if !(p.cross && allow_sfx) {
                    continue;
                }
                // Prefix + suffix (cross product).
                let hit = a.sidx.suffixes(&stem, fullstrip, |j, sentries| {
                    let head = &stem[..j];
                    for &se in sentries {
                        if !step() {
                            return false;
                        }
                        let s = &a.suffixes[se as usize];
                        if !s.cross || !s.sfx_cond(head) {
                            continue;
                        }
                        let root = concat(head, &s.strip);
                        if let Some(h) = self.words.get(root.as_str()) {
                            for &r in h.as_slice() {
                                let ok = (self.root_has(r, s.flag)
                                    && (self.root_has(r, p.flag) || self.sets.has(s.cont, p.flag)))
                                    || (self.root_has(r, p.flag) && self.sets.has(p.cont, s.flag));
                                if ok && accept(&Hit { root: r, pfx: Some(p), sfx: Some(s), sfx2: None }) {
                                    return true;
                                }
                            }
                        }
                        if a.sfx_cont_flags.contains(&s.flag) && self.twofold(&root, s, Some(p), accept) {
                            return true;
                        }
                    }
                    false
                });
                if hit {
                    return true;
                }
            }
            false
        })
    }

    /// Inner suffix of a twofold suffix: `stem` is the word with the outer
    /// suffix `outer` removed; find a suffix whose continuation allows `outer`.
    fn twofold(&self, stem: &str, outer: &Affix, pfx: Option<&Affix>, accept: &mut dyn FnMut(&Hit) -> bool) -> bool {
        let a = &self.aff;
        a.sidx.suffixes(stem, a.fullstrip, |i, entries| {
            let head = &stem[..i];
            for &e in entries {
                if !step() {
                    return false;
                }
                let s2 = &a.suffixes[e as usize];
                if !self.sets.has(s2.cont, outer.flag) || !s2.sfx_cond(head) {
                    continue;
                }
                if pfx.is_some() && !s2.cross {
                    continue;
                }
                let root = concat(head, &s2.strip);
                if let Some(h) = self.words.get(root.as_str()) {
                    for &r in h.as_slice() {
                        if !self.root_has(r, s2.flag) {
                            continue;
                        }
                        if let Some(p) = pfx
                            && !(self.root_has(r, p.flag) || self.sets.has(s2.cont, p.flag))
                        {
                            continue;
                        }
                        if accept(&Hit { root: r, pfx, sfx: Some(outer), sfx2: Some(s2) }) {
                            return true;
                        }
                    }
                }
            }
            false
        })
    }

    fn check_compound(&self, w: &str, sug: bool) -> bool {
        let a = &self.aff;
        budgeted(|| (!a.compound_rules.is_empty() && self.rule_compound(w, &mut Vec::new())) || (a.has_compounding() && self.flag_compound(w, 0, sug)))
    }

    /// COMPOUNDRULE: split into stems whose flags spell out one of the rules.
    fn rule_compound(&self, w: &str, stack: &mut Vec<u32>) -> bool {
        let a = &self.aff;
        let mut count = 0;
        for (i, c) in w.char_indices() {
            if !step() {
                return false;
            }
            count += 1;
            if count < a.compound_min {
                continue;
            }
            let end = i + c.len_utf8();
            let Some(h) = self.words.get(&w[..end]) else { continue };
            for &r in h.as_slice() {
                if self.sets.has_opt(r, a.forbidden) || !self.sets.get(r).iter().any(|f| a.compound_rule_flags.contains(f)) {
                    continue;
                }
                stack.push(r);
                let ok = if end == w.len() {
                    stack.len() >= 2 && a.compound_rules.iter().any(|rule| self.rule_match(rule, stack, false))
                } else {
                    a.compound_rules.iter().any(|rule| self.rule_match(rule, stack, true))
                        && self.rule_compound(&w[end..], stack)
                };
                stack.pop();
                if ok {
                    return true;
                }
            }
        }
        false
    }

    /// Does the sequence of stems match `rule`? With `partial`, a prefix of
    /// the rule is enough (more stems may follow).
    fn rule_match(&self, rule: &[(Flag, Quant)], seq: &[u32], partial: bool) -> bool {
        if !step() {
            return false;
        }
        let Some((&s, rest_seq)) = seq.split_first() else {
            return partial || rule.iter().all(|e| e.1 != Quant::One);
        };
        let Some((&(f, q), rest_rule)) = rule.split_first() else { return false };
        let hit = self.sets.has(s, f);
        match q {
            Quant::One => hit && self.rule_match(rest_rule, rest_seq, partial),
            Quant::Opt => (hit && self.rule_match(rest_rule, rest_seq, partial)) || self.rule_match(rest_rule, seq, partial),
            Quant::Star => (hit && self.rule_match(rule, rest_seq, partial)) || self.rule_match(rest_rule, seq, partial),
        }
    }

    /// COMPOUNDFLAG/BEGIN/MIDDLE/END compounding.
    fn flag_compound(&self, w: &str, nparts: usize, sug: bool) -> bool {
        let a = &self.aff;
        let cmin = a.compound_min.max(1);
        let bounds: Vec<usize> = w.char_indices().map(|(i, _)| i).skip(1).collect();
        let n = bounds.len() + 1;
        if n < cmin * 2 {
            return false;
        }
        if let Some(max) = a.compound_word_max
            && nparts + 2 > max
        {
            return false;
        }
        for k in cmin..=(n - cmin) {
            if !step() {
                return false;
            }
            let split = bounds[k - 1];
            let (first, rest) = w.split_at(split);
            let pos = if nparts == 0 { Part::Begin } else { Part::Middle };
            if !self.compound_part(first, pos, sug) {
                continue;
            }
            if self.compound_part(rest, Part::End, sug) && !(a.check_compound_dup && first == rest) {
                return true;
            }
            if self.flag_compound(rest, nparts + 1, sug) {
                return true;
            }
        }
        false
    }

    fn compound_part(&self, s: &str, pos: Part, sug: bool) -> bool {
        let a = &self.aff;
        let posflag = match pos {
            Part::Begin => a.compoundbegin,
            Part::Middle => a.compoundmiddle,
            Part::End => a.compoundend,
        };
        let flag_ok = |set: u32| self.sets.has_opt(set, a.compoundflag) || self.sets.has_opt(set, posflag);
        let root_ok = |r: u32| {
            !self.sets.has_opt(r, a.forbidden) && !(sug && self.sets.has_opt(r, a.nosuggest))
        };
        if let Some(h) = self.words.get(s) {
            for &r in h.as_slice() {
                if root_ok(r) && !self.sets.has_opt(r, a.needaffix) && flag_ok(r) {
                    return true;
                }
            }
        }
        self.affix_search(s, true, true, &mut |hit| {
            if !root_ok(hit.root) {
                return false;
            }
            if !(flag_ok(hit.root) || hit.affixes().any(|x| flag_ok(x.cont))) {
                return false;
            }
            if hit.affixes().any(|x| self.sets.has_opt(x.cont, a.compoundforbid)) {
                return false;
            }
            let permit = |x: &Affix| self.sets.has_opt(x.cont, a.compoundpermit);
            if let Some(p) = hit.pfx
                && pos != Part::Begin
                && !permit(p)
            {
                return false;
            }
            if pos != Part::End && hit.sfx.into_iter().chain(hit.sfx2).any(|x| !permit(x)) {
                return false;
            }
            self.circumfix_ok(hit)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        for n in ["1990", "3.5", "1,000", "10-20", "0"] {
            assert!(is_number(n), "{n}");
        }
        for n in ["", "1.", ".5", "1..2", "12a", "-"] {
            assert!(!is_number(n), "{n}");
        }
    }
}

thread_local! {
    /// Steps a word's check may still take (None outside one).
    static STEPS: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Steps one word's check may take: splitting it into compounds or at
/// BREAK patterns branches, and a dictionary may make it branch forever;
/// so does trying each affix entry.
const MAX_STEPS: usize = 1 << 18;

/// Runs `f` with a budget of steps, unless already within one.
fn budgeted<T>(f: impl FnOnce() -> T) -> T {
    budgeted_with(MAX_STEPS, f)
}

/// Runs `f` with a budget of `n` steps, unless already within one.
pub(crate) fn budgeted_with<T>(n: usize, f: impl FnOnce() -> T) -> T {
    if STEPS.with(Cell::get).is_some() {
        return f();
    }
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            STEPS.with(|s| s.set(None));
        }
    }
    STEPS.with(|s| s.set(Some(n)));
    let _done = Done;
    f()
}

/// Takes a step; false once the word's budget is spent.
fn step() -> bool {
    STEPS.with(|s| match s.get() {
        None => true,
        Some(0) => false,
        Some(n) => {
            s.set(Some(n - 1));
            true
        }
    })
}
