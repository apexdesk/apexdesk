//! Suggestions, after Hunspell's SuggestMgr: typical-error edits first (REP,
//! MAP, swaps, keyboard neighbours, missing/extra/wrong letters, split
//! words), then, only if those found nothing, an n-gram similarity search
//! over the whole dictionary with a time limit.

use std::sync::atomic::{AtomicBool, Ordering};
use web_time::{Duration, Instant};

use crate::Dictionary;
use crate::Homs;
use crate::aff::ONLYUPCASE;
use crate::casing::{Cap, captype, initcap, lower, lower_char, upper};
use crate::check::MAX_WORD_CHARS;
use crate::hash::FxHashSet;

/// Stems kept from the first n-gram pass, then expanded with their affixes.
const MAX_ROOTS: usize = 100;
/// Word forms kept from the expansion for the final scoring.
const MAX_GUESS: usize = 200;
/// Farthest distance for moving or swapping a single letter.
const MAX_CHAR_DISTANCE: usize = 4;
/// Candidates tried from MAP combinations (they grow quadratically).
const MAP_LIMIT: usize = 600;
/// Time budget of each n-gram pass (the scan of all stems and the expansion).
/// A safety net for huge dictionaries or a busy machine: a pass normally
/// takes a few milliseconds, and cutting it short makes results depend on
/// timing.
const NGRAM_BUDGET: Duration = Duration::from_millis(60);

/// Whether the n-gram passes keep to [`NGRAM_BUDGET`].
static BUDGET: AtomicBool = AtomicBool::new(true);

/// Turns the n-gram passes' time limit off (or on again): they then run to
/// the end, and the same word gets the same suggestions however busy the
/// machine is (tests clicking through them).
pub fn set_time_budget(on: bool) {
    BUDGET.store(on, Ordering::Relaxed);
}

/// Whether a pass started at `start` has used up its time.
fn over_budget(start: Instant) -> bool {
    BUDGET.load(Ordering::Relaxed) && start.elapsed() > NGRAM_BUDGET
}

const NGRAM_LONGER_WORSE: u8 = 1;
const NGRAM_ANY_MISMATCH: u8 = 2;
const NGRAM_WEIGHTED: u8 = 4;

/// Stems usable as suggestions, packed contiguously for the n-gram scan,
/// which visits all of them (walking the hash map instead costs a cache miss
/// per stem). Built on the first n-gram search.
pub(crate) struct NgramIndex {
    /// Lower-cased stems, concatenated.
    low: String,
    /// Stems as in the dictionary, concatenated.
    orig: String,
    entries: Vec<NgramEntry>,
}

struct NgramEntry {
    low: (u32, u32),
    orig: (u32, u32),
    chars: u16,
    /// Characters present (see [`char_bit`]), for a cheap score bound.
    mask: u64,
    /// Capitalized like a proper noun ("Paris"; not "NASA").
    initcap: bool,
}

/// Bit standing for `c` in a character-set mask. Collisions only make the
/// derived bound looser, never wrong.
#[inline]
fn char_bit(c: char) -> u64 {
    let c = c as u32;
    if c < 128 { 1 << (c & 63) } else { 1 << (c.wrapping_mul(0x9E37_79B1) >> 26) }
}

/// Steps all of one word's suggestions may take (see `check`).
const SUGGEST_STEPS: usize = 1 << 22;

impl NgramIndex {
    fn build(d: &Dictionary) -> NgramIndex {
        let a = &d.aff;
        let mut idx = NgramIndex { low: String::new(), orig: String::new(), entries: Vec::with_capacity(d.words.len()) };
        let mut keys: Vec<&str> = d
            .words
            .iter()
            .filter(|(_, h)| {
                h.as_slice().iter().any(|&r| {
                    !d.sets.has_opt(r, a.forbidden)
                        && !d.sets.has_opt(r, a.nosuggest)
                        && !d.sets.has_opt(r, a.onlyincompound)
                        && !d.sets.has(r, ONLYUPCASE)
                })
            })
            .map(|(k, _)| &**k)
            .collect();
        // Deterministic order, so ties rank the same on every run.
        keys.sort_unstable();
        for k in keys {
            let (l0, o0) = (idx.low.len() as u32, idx.orig.len() as u32);
            idx.low.extend(k.chars().map(lower_char));
            idx.orig.push_str(k);
            let low = &idx.low[l0 as usize..];
            let chars = low.chars().count().min(u16::MAX as usize) as u16;
            let mask = low.chars().fold(0, |m, c| m | char_bit(c));
            idx.entries.push(NgramEntry {
                low: (l0, idx.low.len() as u32),
                orig: (o0, idx.orig.len() as u32),
                chars,
                mask,
                initcap: captype(k) == Cap::Init,
            });
        }
        idx
    }
}

/// Hunspell collects up to this many suggestions internally; callers asking
/// for fewer get the same top of the list.
const MAX_SUGGESTIONS: usize = 15;

struct Collector<'d> {
    dict: &'d Dictionary,
    list: Vec<String>,
    tested: FxHashSet<String>,
    max: usize,
}

impl Collector<'_> {
    #[inline]
    fn full(&self) -> bool {
        self.list.len() >= self.max
    }

    /// Adds `cand` if it is a correct word, in exactly this case, not
    /// suggested yet. Words with a space ("a lot") need every part correct.
    fn add(&mut self, cand: String) -> bool {
        if self.full() || self.tested.contains(&cand) {
            return false;
        }
        let ok = if cand.contains(' ') {
            cand.split(' ').all(|p| !p.is_empty() && self.dict.exact_ok(p))
        } else {
            self.dict.exact_ok(&cand)
        };
        if ok {
            self.list.push(cand.clone());
        }
        self.tested.insert(cand);
        ok
    }

    fn add_chars(&mut self, chars: &[char]) -> bool {
        self.add(chars.iter().collect())
    }

    /// Adds a suggestion known to be good.
    fn push(&mut self, s: String) {
        if !self.full() && !self.list.contains(&s) {
            self.tested.insert(s.clone());
            self.list.push(s);
        }
    }
}

impl Dictionary {
    /// Up to `max` suggestions for a misspelled word, best first: REP table,
    /// MAP, TRY-based edits (swap, delete, insert, replace, split in two
    /// words), KEY adjacency, then an n-gram fallback limited in time. The
    /// case pattern of `word` (Capitalized / UPPER) is kept.
    pub fn suggest(&self, word: &str, max: usize) -> Vec<String> {
        // Every candidate's check shares one budget.
        crate::check::budgeted_with(SUGGEST_STEPS, || self.suggest_in(word, max))
    }

    fn suggest_in(&self, word: &str, max: usize) -> Vec<String> {
        let word = word.trim();
        if word.is_empty() || max == 0 {
            return Vec::new();
        }
        let norm = self.normalize(word);
        let w: &str = &norm;
        if w.chars().count() > MAX_WORD_CHARS {
            return Vec::new();
        }
        let cap = captype(w);
        let mut c =
            Collector { dict: self, list: Vec::new(), tested: FxHashSet::default(), max: max.max(MAX_SUGGESTIONS) };
        // As Hunspell: edits of the word as typed, then of its lower-case
        // (and for capitals, Capitalized) form; candidates must match the
        // dictionary exactly, and the case is restored at the end.
        let mut good = false;
        match cap {
            Cap::No => {
                // "nasa" -> "NASA"
                c.add(upper(w));
                good |= self.edits(w, &mut c);
            }
            Cap::Init => {
                good |= self.edits(w, &mut c);
                good |= self.edits(&lower(w), &mut c);
            }
            Cap::All => {
                let l = lower(w);
                good |= self.edits(&l, &mut c);
                if self.aff.keepcase.is_some() && self.check(&l) {
                    c.push(l.clone());
                }
                good |= self.edits(&initcap(&l), &mut c);
            }
            Cap::Huh | Cap::HuhInit => {
                good |= self.edits(w, &mut c);
                let l = lower(w);
                if cap == Cap::HuhInit && self.check(&initcap(&l)) {
                    c.list.insert(0, initcap(&l));
                }
                if self.check(&l) {
                    c.list.insert(0, l.clone());
                }
                good |= self.edits(&l, &mut c);
            }
        }
        // Hyphenated words: fix the wrong parts ("well-knwn" -> "well-known").
        let parts: Vec<&str> = w.split('-').collect();
        if parts.len() > 1 && parts.iter().all(|p| !p.is_empty()) {
            for (i, p) in parts.iter().enumerate() {
                if c.full() || self.check(p) {
                    continue;
                }
                for s in self.suggest(p, max) {
                    let mut v = parts.clone();
                    v[i] = &s;
                    c.push(v.join("-"));
                }
            }
        }
        // Like Hunspell 1.7, the n-gram search adds a few more guesses unless
        // REP/MAP already explained the error.
        if !good && c.list.len() < max {
            if cap == Cap::All {
                // Hunspell upper-cases the list before the n-gram search.
                for s in &mut c.list {
                    *s = upper(s);
                }
            }
            // Lower-case typos don't get proper nouns from the n-gram search
            // (as in Hunspell), except in German where all nouns are capitalized.
            let lang = if self.aff.lang.is_empty() { self.language() } else { &self.aff.lang };
            let nocap = cap == Cap::No && !lang.starts_with("de");
            self.ngram_sugs(&lower(w), nocap, &mut c);
        }

        let mut out = Vec::new();
        let mut seen = FxHashSet::default();
        for s in c.list {
            let s = match cap {
                Cap::Init | Cap::HuhInit => initcap(&s),
                Cap::All => {
                    let u = upper(&s);
                    if u.split(' ').all(|p| self.check(p)) { u } else { s }
                }
                _ => s,
            };
            let s = self.aff.oconv.apply(&s).into_owned();
            if s != word && seen.insert(s.clone()) {
                out.push(s);
                if out.len() >= max {
                    break;
                }
            }
        }
        out
    }

    /// Exact-case check used for suggestion candidates (no NOSUGGEST words).
    fn exact_ok(&self, w: &str) -> bool {
        self.check_word(w, true, captype(w) == Cap::Init).is_some()
    }

    /// Edit-based suggestions; true if the REP or MAP tables produced some
    /// (Hunspell's "good" suggestions, which make the n-gram search moot).
    fn edits(&self, word: &str, c: &mut Collector) -> bool {
        let before = c.list.len();
        self.rep_map(word, c);
        let good = c.list.len() > before;
        self.letter_edits(word, c);
        good
    }

    fn rep_map(&self, word: &str, c: &mut Collector) {
        let a = &self.aff;

        // REP: typical misspellings of the language.
        for r in &a.rep {
            if c.full() {
                return;
            }
            if r.from.is_empty() {
                continue;
            }
            if r.at_start {
                if word.starts_with(&r.from) && (!r.at_end || word.len() == r.from.len()) {
                    c.add(format!("{}{}", r.to, &word[r.from.len()..]));
                }
            } else if r.at_end {
                if word.ends_with(&r.from) {
                    c.add(format!("{}{}", &word[..word.len() - r.from.len()], r.to));
                }
            } else {
                let hits: Vec<usize> = word.match_indices(r.from.as_str()).map(|(i, _)| i).collect();
                for i in hits {
                    c.add(format!("{}{}{}", &word[..i], r.to, &word[i + r.from.len()..]));
                }
            }
        }

        // MAP: related characters (accents): one substitution, then two.
        if !a.map.is_empty() {
            let mut subs: Vec<(usize, usize, &str)> = Vec::new();
            for (i, _) in word.char_indices() {
                for group in &a.map {
                    for item in group.iter().filter(|it| word[i..].starts_with(it.as_str())) {
                        for other in group.iter().filter(|o| *o != item).take(4096 - subs.len().min(4096)) {
                            subs.push((i, i + item.len(), other));
                        }
                    }
                }
            }
            for &(s, e, r) in &subs {
                c.add(format!("{}{}{}", &word[..s], r, &word[e..]));
            }
            let mut tries = 0;
            'pairs: for (x, &(s1, e1, r1)) in subs.iter().enumerate() {
                for &(s2, e2, r2) in &subs[x + 1..] {
                    if s2 < e1 {
                        continue;
                    }
                    if c.full() || tries >= MAP_LIMIT {
                        break 'pairs;
                    }
                    tries += 1;
                    c.add(format!("{}{}{}{}{}", &word[..s1], r1, &word[e1..s2], r2, &word[e2..]));
                }
            }
        }
    }

    /// Swaps, keyboard neighbours, extra/missing/moved/wrong letters and
    /// split words, in Hunspell's order.
    fn letter_edits(&self, word: &str, c: &mut Collector) {
        let chars: Vec<char> = word.chars().collect();
        let n = chars.len();
        let a = &self.aff;
        let mut buf = chars.clone();
        // Adjacent letters swapped: "teh" -> "the".
        for i in 0..n.saturating_sub(1) {
            if chars[i] != chars[i + 1] {
                buf.swap(i, i + 1);
                c.add_chars(&buf);
                buf.swap(i, i + 1);
            }
        }
        // Two swaps in short words: "ahev" -> "have".
        if n == 4 || n == 5 {
            let pairs: &[(usize, usize)] = if n == 4 { &[(0, 2)] } else { &[(0, 3), (1, 3)] };
            for &(i, j) in pairs {
                buf.swap(i, i + 1);
                buf.swap(j, j + 1);
                c.add_chars(&buf);
                buf.copy_from_slice(&chars);
            }
        }
        // Distant letters swapped.
        for i in 0..n {
            for j in i + 2..n.min(i + MAX_CHAR_DISTANCE + 1) {
                if chars[i] != chars[j] {
                    buf.swap(i, j);
                    c.add_chars(&buf);
                    buf.swap(i, j);
                }
            }
        }
        // Wrong case of a letter or neighbouring key pressed.
        for i in 0..n {
            let ch = chars[i];
            let mut up = ch.to_uppercase();
            if let (Some(u), None) = (up.next(), up.next())
                && u != ch
            {
                buf[i] = u;
                c.add_chars(&buf);
            }
            for row in &a.key {
                for (k, &kc) in row.iter().enumerate() {
                    if kc != ch {
                        continue;
                    }
                    if k > 0 {
                        buf[i] = row[k - 1];
                        c.add_chars(&buf);
                    }
                    if let Some(&next) = row.get(k + 1) {
                        buf[i] = next;
                        c.add_chars(&buf);
                    }
                }
            }
            buf[i] = ch;
        }
        if c.full() {
            return;
        }
        // Extra letter. Like Hunspell, positions are tried from the end
        // (and TRY letters in their order), which ranks well in practice.
        if n > 1 {
            for i in (0..n).rev() {
                let mut v = chars.clone();
                v.remove(i);
                c.add_chars(&v);
            }
        }
        // Missing letter.
        let mut v = Vec::with_capacity(n + 1);
        for &t in &a.try_chars {
            if c.full() {
                return;
            }
            for i in (0..=n).rev() {
                v.clear();
                v.extend_from_slice(&chars[..i]);
                v.push(t);
                v.extend_from_slice(&chars[i..]);
                c.add_chars(&v);
            }
        }
        // Letter typed in the wrong place: moved forward, then backward.
        for i in 0..n {
            for j in i + 2..n.min(i + MAX_CHAR_DISTANCE + 1) {
                let mut v = chars.clone();
                let ch = v.remove(i);
                v.insert(j, ch);
                c.add_chars(&v);
            }
        }
        for j in (1..n).rev() {
            for i in (j.saturating_sub(MAX_CHAR_DISTANCE)..j.saturating_sub(1)).rev() {
                let mut v = chars.clone();
                let ch = v.remove(j);
                v.insert(i, ch);
                c.add_chars(&v);
            }
        }
        // Wrong letter.
        for &t in &a.try_chars {
            if c.full() {
                return;
            }
            for i in (0..n).rev() {
                if t != chars[i] {
                    buf[i] = t;
                    c.add_chars(&buf);
                    buf[i] = chars[i];
                }
            }
        }
        // Doubled pair of letters: "vacacation" -> "vacation".
        for i in 3..n {
            if chars[i - 3] == chars[i - 1] && chars[i - 2] == chars[i] {
                let mut v = chars.clone();
                v.drain(i - 1..=i);
                c.add_chars(&v);
            }
        }
        // Missing space: "alot" -> "a lot"; also with a hyphen when TRY
        // suggests the language uses them (as Hunspell does).
        if !a.nosplitsugs && n > 1 {
            let dash = a.try_chars.iter().any(|&t| t == '-' || t == 'a');
            for i in 1..n {
                let mut s: String = chars[..i].iter().collect();
                s.push(' ');
                s.extend(&chars[i..]);
                if c.add(s.clone()) && dash && i > 1 && n - i > 1 && !c.full() {
                    // Added as is: the hyphenated form may have been
                    // tested (and refused as one word) by an earlier edit.
                    let d = s.replacen(' ', "-", 1);
                    c.push(d);
                }
            }
        }
    }

    /// Similarity search over the whole dictionary (Hunspell's ngsuggest):
    /// best stems by 3-gram score, their affixed forms by n-gram score, then a
    /// finer ranking. `word` is lower-case.
    fn ngram_sugs(&self, word: &str, nocap: bool, c: &mut Collector) {
        let idx = self.ngram_index.get_or_init(|| NgramIndex::build(self));
        let start = Instant::now();
        let a = &self.aff;
        let w: Vec<char> = word.chars().collect();
        let n = w.len();
        if n == 0 {
            return;
        }
        let usable = |r: u32| {
            !self.sets.has_opt(r, a.forbidden)
                && !self.sets.has_opt(r, a.nosuggest)
                && !self.sets.has_opt(r, a.onlyincompound)
                && !self.sets.has(r, ONLYUPCASE)
        };

        // Pass 1: stems.
        let wbits: Vec<u64> = w.iter().map(|&c| char_bit(c)).collect();
        let mut best: Vec<(i32, u32)> = Vec::with_capacity(MAX_ROOTS);
        let mut min_i = 0;
        let mut buf: Vec<char> = Vec::with_capacity(64);
        for (k, e) in idx.entries.iter().enumerate() {
            if k % 4096 == 4095 && over_budget(start) {
                break;
            }
            if e.chars as usize > n + 4 || (nocap && e.initcap) {
                continue;
            }
            let low = &idx.low[e.low.0 as usize..e.low.1 as usize];
            if best.len() == MAX_ROOTS {
                // Skip stems whose best possible score can't make the list:
                // an n-gram can only be present if all its letters are.
                let has = |i: usize| e.mask & wbits[i] != 0;
                let ns1 = (0..n).filter(|&i| has(i)).count() as i32;
                let mut ub = ns1;
                if ns1 >= 2 {
                    let ns2 = (0..n.saturating_sub(1)).filter(|&i| has(i) && has(i + 1)).count() as i32;
                    ub += ns2;
                    if ns2 >= 2 {
                        ub += (0..n.saturating_sub(2)).filter(|&i| has(i) && has(i + 1) && has(i + 2)).count() as i32;
                    }
                }
                ub -= (e.chars as i32 - n as i32 - 2).max(0);
                ub += low.chars().zip(&w).take_while(|(x, y)| x == *y).count() as i32;
                if ub <= best[min_i].0 {
                    continue;
                }
            }
            buf.clear();
            buf.extend(low.chars());
            let sc = ngram(3, &w, &buf, NGRAM_LONGER_WORSE) + left_common(&w, &buf);
            if best.len() < MAX_ROOTS {
                best.push((sc, k as u32));
                if best.len() == MAX_ROOTS {
                    min_i = argmin(&best);
                }
            } else if sc > best[min_i].0 {
                best[min_i] = (sc, k as u32);
                min_i = argmin(&best);
            }
        }
        best.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.cmp(&y.1)));
        let roots: Vec<(&str, &Homs)> = best
            .iter()
            .filter_map(|&(_, k)| {
                let e = &idx.entries[k as usize];
                let stem = &idx.orig[e.orig.0 as usize..e.orig.1 as usize];
                self.words.get_key_value(stem).map(|(s, h)| (&**s, h))
            })
            .collect();

        // Threshold: the score of the word with every 4th letter garbled.
        let mut thresh = 0;
        for sp in 1..4 {
            let mut mw = w.clone();
            let mut k = sp;
            while k < n {
                mw[k] = '*';
                k += 4;
            }
            thresh += ngram(n, &w, &mw, NGRAM_ANY_MISMATCH);
        }
        let thresh = thresh / 3 - 1;

        // Pass 2: affixed forms of the best stems.
        let start2 = Instant::now();
        let mut guesses: Vec<(i32, String)> = Vec::with_capacity(MAX_GUESS);
        let mut guess_set: FxHashSet<String> = FxHashSet::default();
        let mut gmin = 0;
        let mut gl: Vec<char> = Vec::with_capacity(64);
        for &(stem, h) in &roots {
            if over_budget(start2) {
                break;
            }
            for &r in h.as_slice() {
                if !usable(r) {
                    continue;
                }
                self.expand(stem, r, word, &mut |form: &str| {
                    gl.clear();
                    gl.extend(form.chars().map(lower_char));
                    let sc = ngram(n, &w, &gl, NGRAM_ANY_MISMATCH) + left_common(&w, &gl);
                    if sc <= thresh || guess_set.contains(form) {
                        return;
                    }
                    if guesses.len() < MAX_GUESS {
                        guess_set.insert(form.to_string());
                        guesses.push((sc, form.to_string()));
                        if guesses.len() == MAX_GUESS {
                            gmin = argmin_g(&guesses);
                        }
                    } else if sc > guesses[gmin].0 {
                        guess_set.remove(&guesses[gmin].1);
                        guess_set.insert(form.to_string());
                        guesses[gmin] = (sc, form.to_string());
                        gmin = argmin_g(&guesses);
                    }
                });
            }
        }

        // Pass 3: finer ranking.
        let mut scored: Vec<(i32, String)> = guesses
            .into_iter()
            .map(|(_, g)| {
                gl.clear();
                gl.extend(g.chars().map(lower_char));
                let m = gl.len();
                let lcs = lcs_len(&w, &gl) as i32;
                if n == m && lcs == n as i32 {
                    return (2000, g);
                }
                let re = ngram(2, &w, &gl, NGRAM_ANY_MISMATCH | NGRAM_WEIGHTED)
                    + ngram(2, &gl, &w, NGRAM_ANY_MISMATCH | NGRAM_WEIGHTED);
                let (same_pos, swap) = common_positions(&w, &gl);
                let sc = 2 * lcs - (n as i32 - m as i32).abs()
                    + left_common(&w, &gl)
                    + i32::from(same_pos > 0)
                    + if swap { 10 } else { 0 }
                    + ngram(4, &w, &gl, NGRAM_ANY_MISMATCH)
                    + re
                    + if re < (n + m) as i32 { -1000 } else { 0 };
                (sc, g)
            })
            .collect();
        scored.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));

        let limit = a.maxngramsugs.min(c.max);
        let mut added = 0;
        let mut excellent = false;
        for (sc, g) in scored {
            if added >= limit || c.full() {
                break;
            }
            if excellent && sc <= 1000 {
                continue;
            }
            if sc > 1000 {
                excellent = true;
            } else if sc < -100 {
                // Poor matches only when nothing better was found.
                excellent = true;
                if added > 0 {
                    break;
                }
            }
            // Nor a longer form of something already suggested
            // ("backwoodsman" after "backwoods").
            if c.list.iter().any(|s| g.contains(s.as_str())) {
                continue;
            }
            // As Hunspell, the edit phase's tested set isn't consulted: for
            // UPPER input the list is upper-case here, so a lower-case repeat
            // takes a slot and is merged away at the end.
            if self.exact_ok(&g) {
                c.list.push(g);
                added += 1;
            }
        }
    }

    /// Calls `f` with the stem and its single-affix and prefix+suffix forms.
    /// Like Hunspell, only affixes that `bad` (the misspelled word) itself
    /// starts or ends with are applied: that keeps the expansion small and
    /// the guesses close to what was typed.
    fn expand(&self, stem: &str, r: u32, bad: &str, f: &mut dyn FnMut(&str)) {
        let a = &self.aff;
        let blocked = |cont: u32| {
            self.sets.has_opt(cont, a.needaffix)
                || self.sets.has_opt(cont, a.onlyincompound)
                || self.sets.has_opt(cont, a.circumfix)
        };
        let sfx_fits = |app: &str| app.is_empty() || (bad.len() > app.len() && bad.ends_with(app));
        let pfx_fits = |app: &str| app.is_empty() || (bad.len() > app.len() && bad.starts_with(app));
        if !self.sets.has_opt(r, a.needaffix) {
            f(stem);
        }
        let flags = self.sets.get(r);
        let mut form = String::new();
        for &fl in flags {
            if let Some(list) = a.sfx_by_flag.get(&fl) {
                for &i in list {
                    let s = &a.suffixes[i as usize];
                    if !sfx_fits(&s.append) || blocked(s.cont) {
                        continue;
                    }
                    let Some(head) = stem.strip_suffix(&*s.strip) else { continue };
                    if (head.is_empty() && !a.fullstrip) || !s.sfx_cond(head) {
                        continue;
                    }
                    form.clear();
                    form.push_str(head);
                    form.push_str(&s.append);
                    f(&form);
                    if !s.cross {
                        continue;
                    }
                    for &pf in flags {
                        let Some(plist) = a.pfx_by_flag.get(&pf) else { continue };
                        for &j in plist {
                            let p = &a.prefixes[j as usize];
                            if !p.cross || !pfx_fits(&p.append) {
                                continue;
                            }
                            let Some(tail) = form.strip_prefix(&*p.strip) else { continue };
                            if tail.is_empty() || !p.pfx_cond(tail) {
                                continue;
                            }
                            let pform = format!("{}{}", p.append, tail);
                            f(&pform);
                        }
                    }
                }
            }
            if let Some(list) = a.pfx_by_flag.get(&fl) {
                for &j in list {
                    let p = &a.prefixes[j as usize];
                    if !pfx_fits(&p.append) || blocked(p.cont) {
                        continue;
                    }
                    let Some(tail) = stem.strip_prefix(&*p.strip) else { continue };
                    if (tail.is_empty() && !a.fullstrip) || !p.pfx_cond(tail) {
                        continue;
                    }
                    form.clear();
                    form.push_str(&p.append);
                    form.push_str(tail);
                    f(&form);
                }
            }
        }
    }
}

fn argmin(v: &[(i32, u32)]) -> usize {
    v.iter().enumerate().min_by_key(|(_, x)| x.0).map(|(i, _)| i).unwrap_or(0)
}

fn argmin_g(v: &[(i32, String)]) -> usize {
    v.iter().enumerate().min_by_key(|(_, x)| x.0).map(|(i, _)| i).unwrap_or(0)
}

/// Hunspell's n-gram similarity of `s1` against `s2`.
fn ngram(n: usize, s1: &[char], s2: &[char], opt: u8) -> i32 {
    let (l1, l2) = (s1.len(), s2.len());
    if l2 == 0 {
        return 0;
    }
    let weighted = opt & NGRAM_WEIGHTED != 0;
    let mut nscore = 0i32;
    for j in 1..=n.min(l1) {
        let mut ns = 0i32;
        for i in 0..=(l1 - j) {
            let sub = &s1[i..i + j];
            if contains_gram(s2, sub) {
                ns += 1;
            } else if weighted {
                ns -= 1;
                if i == 0 || i == l1 - j {
                    ns -= 1; // side weight
                }
            }
        }
        nscore += ns;
        if ns < 2 && !weighted {
            break;
        }
    }
    let mut d = 0i32;
    if opt & NGRAM_LONGER_WORSE != 0 {
        d = l2 as i32 - l1 as i32 - 2;
    }
    if opt & NGRAM_ANY_MISMATCH != 0 {
        d = (l2 as i32 - l1 as i32).abs() - 2;
    }
    nscore - d.max(0)
}

/// `hay` contains the sequence `g` (short grams unrolled: this is the hot
/// loop of the n-gram scan).
#[inline]
fn contains_gram(hay: &[char], g: &[char]) -> bool {
    match *g {
        [a] => hay.contains(&a),
        [a, b] => hay.windows(2).any(|x| x[0] == a && x[1] == b),
        [a, b, c] => hay.windows(3).any(|x| x[0] == a && x[1] == b && x[2] == c),
        _ => hay.windows(g.len()).any(|x| x == g),
    }
}

fn left_common(a: &[char], b: &[char]) -> i32 {
    a.iter().zip(b).take_while(|(x, y)| x == y).count() as i32
}

fn lcs_len(a: &[char], b: &[char]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    let mut cur = vec![0usize; b.len() + 1];
    for &x in a {
        for (j, &y) in b.iter().enumerate() {
            cur[j + 1] = if x == y { prev[j] + 1 } else { cur[j].max(prev[j + 1]) };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Equal characters at equal positions, and whether the words differ by
/// exactly one (not necessarily adjacent) swap.
fn common_positions(a: &[char], b: &[char]) -> (usize, bool) {
    let mut same = 0;
    let mut diff = Vec::new();
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if x == y {
            same += 1;
        } else {
            diff.push(i);
        }
    }
    let swap = a.len() == b.len() && diff.len() == 2 && a[diff[0]] == b[diff[1]] && a[diff[1]] == b[diff[0]];
    (same, swap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn similarity_helpers() {
        assert_eq!(lcs_len(&v("receive"), &v("recieve")), 6);
        assert_eq!(left_common(&v("house"), &v("hose")), 2);
        assert_eq!(common_positions(&v("abcd"), &v("adcb")), (2, true));
        assert!(ngram(3, &v("house"), &v("house"), 0) > ngram(3, &v("house"), &v("mouse"), 0));
        assert!(ngram(3, &v("hello"), &v("hellooooo"), NGRAM_LONGER_WORSE) < ngram(3, &v("hello"), &v("hello"), 0));
    }
}
