//! Flash Fill (Ctrl+E): learns a string program from a few examples and
//! applies it to the other rows.
//!
//! Programs are concatenations of atoms: constant text, or a piece of an
//! input cell (the whole cell, the k-th word/number/token from either end,
//! a field between delimiters, or a span of tokens), optionally cased
//! (lower, upper, proper) and cut (first/last n characters, a middle
//! slice). The search is a uniform-cost search over the positions reached
//! in every example's output at once, so the cheapest (simplest) program
//! consistent with all examples wins.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Class {
    /// Letters.
    Word,
    /// Digits.
    Number,
    /// Letters and digits.
    Alnum,
    /// Uppercase letters.
    Upper,
    /// Anything but spaces.
    NonSpace,
}

const CLASSES: [Class; 5] = [Class::Word, Class::Number, Class::Alnum, Class::Upper, Class::NonSpace];

impl Class {
    fn has(self, c: char) -> bool {
        match self {
            Class::Word => c.is_alphabetic(),
            Class::Number => c.is_ascii_digit(),
            Class::Alnum => c.is_alphanumeric(),
            Class::Upper => c.is_uppercase(),
            Class::NonSpace => !c.is_whitespace(),
        }
    }

    fn cost(self) -> f64 {
        match self {
            Class::Word | Class::Number => 0.0,
            Class::Alnum => 0.05,
            Class::NonSpace => 0.08,
            Class::Upper => 0.1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Sel {
    Whole,
    /// k-th token (negative counts from the end).
    Token(Class, i32),
    /// k-th field between `char` delimiters.
    Split(char, i32),
    /// From the start of token a to the end of token b.
    Span(Class, i32, i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Case {
    Keep,
    Lower,
    Upper,
    Proper,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Part {
    All,
    First(usize),
    Last(usize),
    /// `len` characters from `start`.
    Mid(usize, usize),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Atom {
    Const(String),
    Sub { input: usize, sel: Sel, case: Case, part: Part },
}

/// A learned string transformation.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    atoms: Vec<Atom>,
}

impl Program {
    /// The output for one row of inputs, or None when the program doesn't
    /// apply (a token that isn't there).
    pub fn apply(&self, inputs: &[String]) -> Option<String> {
        let mut out = String::new();
        for a in &self.atoms {
            out.push_str(&eval(a, inputs)?);
        }
        Some(out)
    }
}

fn index(k: i32, len: usize) -> Option<usize> {
    let i = if k < 0 { len as i64 + k as i64 } else { k as i64 };
    (i >= 0 && (i as usize) < len).then_some(i as usize)
}

/// Char ranges of the tokens of a class.
fn tokens(s: &[char], c: Class) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, &ch) in s.iter().enumerate() {
        match (c.has(ch), start) {
            (true, None) => start = Some(i),
            (false, Some(st)) => {
                out.push((st, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(st) = start {
        out.push((st, s.len()));
    }
    out
}

fn fields(s: &[char], d: char) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut st = 0;
    for (i, &ch) in s.iter().enumerate() {
        if ch == d {
            out.push((st, i));
            st = i + 1;
        }
    }
    out.push((st, s.len()));
    out
}

fn select(s: &[char], sel: &Sel) -> Option<Vec<char>> {
    let (a, b) = match *sel {
        Sel::Whole => (0, s.len()),
        Sel::Token(c, k) => {
            let t = tokens(s, c);
            t[index(k, t.len())?]
        }
        Sel::Split(d, k) => {
            let f = fields(s, d);
            if f.len() < 2 {
                return None;
            }
            f[index(k, f.len())?]
        }
        Sel::Span(c, x, y) => {
            let t = tokens(s, c);
            let (i, j) = (index(x, t.len())?, index(y, t.len())?);
            if i >= j {
                return None;
            }
            (t[i].0, t[j].1)
        }
    };
    Some(s[a..b].to_vec())
}

fn recase(s: &[char], case: Case) -> Vec<char> {
    match case {
        Case::Keep => s.to_vec(),
        Case::Lower => s.iter().flat_map(|c| c.to_lowercase()).collect(),
        Case::Upper => s.iter().flat_map(|c| c.to_uppercase()).collect(),
        Case::Proper => {
            let mut out = Vec::with_capacity(s.len());
            let mut prev_letter = false;
            for &c in s {
                if prev_letter {
                    out.extend(c.to_lowercase());
                } else {
                    out.extend(c.to_uppercase());
                }
                prev_letter = c.is_alphabetic();
            }
            out
        }
    }
}

fn cut(s: &[char], part: Part) -> Option<Vec<char>> {
    Some(match part {
        Part::All => s.to_vec(),
        Part::First(n) if n <= s.len() => s[..n].to_vec(),
        Part::Last(n) if n <= s.len() => s[s.len() - n..].to_vec(),
        Part::Mid(a, n) if a + n <= s.len() => s[a..a + n].to_vec(),
        _ => return None,
    })
}

fn eval(a: &Atom, inputs: &[String]) -> Option<String> {
    match a {
        Atom::Const(c) => Some(c.clone()),
        Atom::Sub { input, sel, case, part } => {
            let s: Vec<char> = inputs.get(*input)?.chars().collect();
            let base = select(&s, sel)?;
            let v = cut(&recase(&base, *case), *part)?;
            Some(v.into_iter().collect())
        }
    }
}

fn idx_cost(k: i32) -> f64 {
    if k >= 0 { 0.1 * k as f64 } else { 0.1 * (-k - 1) as f64 + 0.05 }
}

fn cost(a: &Atom) -> f64 {
    match a {
        // Letters and digits usually come from the inputs; punctuation
        // and spaces are the typical constants.
        Atom::Const(c) => 1.0 + c.chars().map(|ch| if ch.is_alphanumeric() { 2.0 } else { 0.5 }).sum::<f64>(),
        Atom::Sub { sel, case, part, .. } => {
            let s = match sel {
                Sel::Whole => 0.0,
                Sel::Token(c, k) => 0.2 + c.cost() + idx_cost(*k),
                Sel::Split(_, k) => 0.3 + idx_cost(*k),
                Sel::Span(c, a, b) => 0.6 + c.cost() + idx_cost(*a) + idx_cost(*b),
            };
            let c = if *case == Case::Keep { 0.0 } else { 0.3 };
            let p = match part {
                Part::All => 0.0,
                Part::First(1) => 0.2,
                Part::First(_) | Part::Last(_) => 0.3,
                Part::Mid(..) => 1.0,
            };
            1.0 + s + c + p
        }
    }
}

/// Selections worth trying on an input of the first example.
fn selections(s: &[char]) -> Vec<Sel> {
    let mut out = vec![Sel::Whole];
    for c in CLASSES {
        let n = tokens(s, c).len() as i32;
        // Long texts: only tokens near either end, and no spans.
        for k in 0..n.min(30) {
            out.push(Sel::Token(c, k));
            out.push(Sel::Token(c, -k - 1));
        }
        if n > 12 {
            continue;
        }
        for a in 0..n {
            for b in a + 1..n {
                for (x, y) in [(a, b), (a, b - n), (a - n, b), (a - n, b - n)] {
                    out.push(Sel::Span(c, x, y));
                }
            }
        }
    }
    let mut delims: Vec<char> = s.iter().copied().filter(|c| !c.is_alphanumeric()).collect();
    delims.sort_unstable();
    delims.dedup();
    for d in delims {
        let n = fields(s, d).len() as i32;
        for k in 0..n.min(30) {
            out.push(Sel::Split(d, k));
            out.push(Sel::Split(d, -k - 1));
        }
    }
    out.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    out.dedup();
    out
}

/// Atoms producing a nonempty prefix of `target` from the inputs of the
/// first example.
fn candidates(inputs: &[String], target: &[char], sels: &[Vec<Sel>]) -> Vec<(Atom, usize)> {
    let mut out = Vec::new();
    for (i, inp) in inputs.iter().enumerate() {
        let s: Vec<char> = inp.chars().collect();
        for sel in &sels[i] {
            let Some(base) = select(&s, sel) else { continue };
            if base.is_empty() {
                continue;
            }
            for case in [Case::Keep, Case::Lower, Case::Upper, Case::Proper] {
                let v = recase(&base, case);
                if case != Case::Keep && v == base {
                    continue;
                }
                let n = v.len();
                let full = matches!(sel, Sel::Whole) || v.len() == base.len();
                // The whole value.
                if target.starts_with(&v) {
                    out.push((Atom::Sub { input: i, sel: sel.clone(), case, part: Part::All }, n));
                }
                if matches!(sel, Sel::Span(..)) || !full {
                    continue;
                }
                // Pieces of it that match the start of the target.
                for a in 0..n {
                    let common = v[a..].iter().zip(target).take_while(|(x, y)| x == y).count();
                    for len in 1..=common {
                        if a == 0 && len == n {
                            continue;
                        }
                        let part = if a == 0 {
                            Part::First(len)
                        } else if a + len == n {
                            Part::Last(len)
                        } else {
                            Part::Mid(a, len)
                        };
                        out.push((Atom::Sub { input: i, sel: sel.clone(), case, part }, len));
                    }
                }
            }
        }
    }
    for len in 1..=target.len() {
        out.push((Atom::Const(target[..len].iter().collect()), len));
    }
    out
}

#[derive(PartialEq)]
struct Entry {
    cost: f64,
    state: Vec<usize>,
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Entry {
    fn cmp(&self, o: &Self) -> Ordering {
        o.cost.total_cmp(&self.cost).then_with(|| o.state.cmp(&self.state))
    }
}

/// Learns the simplest program mapping every example's inputs to its output.
pub fn synthesize(examples: &[(Vec<String>, String)]) -> Option<Program> {
    let first = examples.first()?;
    let outs: Vec<Vec<char>> = examples.iter().map(|e| e.1.chars().collect()).collect();
    if outs[0].is_empty() || outs.iter().any(|o| o.len() > 500) {
        return None;
    }
    let sels: Vec<Vec<Sel>> = first.0.iter().map(|s| selections(&s.chars().collect::<Vec<_>>())).collect();
    let start = vec![0usize; examples.len()];
    let mut best: HashMap<Vec<usize>, f64> = HashMap::new();
    let mut back: HashMap<Vec<usize>, (Vec<usize>, Atom)> = HashMap::new();
    let mut heap = BinaryHeap::new();
    best.insert(start.clone(), 0.0);
    heap.push(Entry { cost: 0.0, state: start });
    let mut expanded = 0;
    while let Some(Entry { cost: c, state }) = heap.pop() {
        if best.get(&state).is_some_and(|b| c > *b) {
            continue;
        }
        if state.iter().zip(&outs).all(|(p, o)| *p == o.len()) {
            let mut atoms = Vec::new();
            let mut s = state;
            while let Some((prev, a)) = back.get(&s) {
                atoms.push(a.clone());
                s = prev.clone();
            }
            atoms.reverse();
            return Some(Program { atoms: merge_consts(atoms) });
        }
        expanded += 1;
        if expanded > 20_000 || state[0] >= outs[0].len() {
            if expanded > 20_000 {
                return None;
            }
            continue;
        }
        for (atom, len0) in candidates(&first.0, &outs[0][state[0]..], &sels) {
            let mut next = state.clone();
            next[0] += len0;
            let mut ok = true;
            let mut extra = 0.0;
            for k in 1..examples.len() {
                let Some(v) = eval(&atom, &examples[k].0) else {
                    ok = false;
                    break;
                };
                let v: Vec<char> = v.chars().collect();
                if !outs[k][state[k]..].starts_with(&v) {
                    ok = false;
                    break;
                }
                if v.is_empty() {
                    extra += 0.5;
                }
                next[k] += v.len();
            }
            if !ok {
                continue;
            }
            let nc = c + cost(&atom) + extra;
            if best.get(&next).is_none_or(|b| nc < *b - 1e-12) {
                best.insert(next.clone(), nc);
                back.insert(next.clone(), (state.clone(), atom));
                heap.push(Entry { cost: nc, state: next });
            }
        }
    }
    None
}

/// Adjacent constants read better as one.
fn merge_consts(atoms: Vec<Atom>) -> Vec<Atom> {
    let mut out: Vec<Atom> = Vec::new();
    for a in atoms {
        match (out.last_mut(), &a) {
            (Some(Atom::Const(p)), Atom::Const(c)) => p.push_str(c),
            _ => out.push(a),
        }
    }
    out
}

/// Flash Fill: `examples` are (row inputs, typed output) pairs; returns
/// the output for each of `rows` (empty where the pattern doesn't apply),
/// or None when no program explains the examples.
pub fn flash_fill(examples: &[(Vec<String>, String)], rows: &[Vec<String>]) -> Option<Vec<String>> {
    let p = synthesize(examples)?;
    Some(rows.iter().map(|r| p.apply(r).unwrap_or_default()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(pairs: &[(&[&str], &str)]) -> Vec<(Vec<String>, String)> {
        pairs.iter().map(|(i, o)| (i.iter().map(|s| s.to_string()).collect(), o.to_string())).collect()
    }

    fn rows(r: &[&[&str]]) -> Vec<Vec<String>> {
        r.iter().map(|i| i.iter().map(|s| s.to_string()).collect()).collect()
    }

    #[test]
    fn first_and_last_names() {
        let e = ex(&[(&["John Smith"], "John")]);
        assert_eq!(flash_fill(&e, &rows(&[&["Ana Lopez"], &["Bob Stone"]])).unwrap(), vec!["Ana", "Bob"]);
        let e = ex(&[(&["John Smith"], "Smith")]);
        assert_eq!(flash_fill(&e, &rows(&[&["Mary Ann Jones"]])).unwrap(), vec!["Jones"]);
        let e = ex(&[(&["John Smith"], "Smith, John")]);
        assert_eq!(flash_fill(&e, &rows(&[&["Ana Lopez"]])).unwrap(), vec!["Lopez, Ana"]);
    }

    #[test]
    fn initials_and_case() {
        let e = ex(&[(&["john smith"], "JS")]);
        assert_eq!(flash_fill(&e, &rows(&[&["ana lopez"]])).unwrap(), vec!["AL"]);
        let e = ex(&[(&["john smith"], "J. Smith")]);
        assert_eq!(flash_fill(&e, &rows(&[&["ana lopez"]])).unwrap(), vec!["A. Lopez"]);
        let e = ex(&[(&["mARy jONES"], "Mary Jones")]);
        assert_eq!(flash_fill(&e, &rows(&[&["bOB sTONE"]])).unwrap(), vec!["Bob Stone"]);
    }

    #[test]
    fn dates_and_phones() {
        let e = ex(&[(&["2024-03-15"], "15/03/2024")]);
        assert_eq!(flash_fill(&e, &rows(&[&["1999-12-01"]])).unwrap(), vec!["01/12/1999"]);
        let e = ex(&[(&["5551234567"], "(555) 123-4567")]);
        assert_eq!(flash_fill(&e, &rows(&[&["9876543210"]])).unwrap(), vec!["(987) 654-3210"]);
        let e = ex(&[(&["555.123.4567"], "555-123-4567")]);
        assert_eq!(flash_fill(&e, &rows(&[&["800.555.0199"]])).unwrap(), vec!["800-555-0199"]);
    }

    #[test]
    fn email_domain_and_user() {
        let e = ex(&[(&["john.smith@example.com"], "example.com"), (&["ana@mail.co.uk"], "mail.co.uk")]);
        assert_eq!(flash_fill(&e, &rows(&[&["x@y.org"]])).unwrap(), vec!["y.org"]);
        let e = ex(&[(&["john.smith@example.com"], "john.smith")]);
        assert_eq!(flash_fill(&e, &rows(&[&["ana.lopez@mail.es"]])).unwrap(), vec!["ana.lopez"]);
    }

    #[test]
    fn combine_columns() {
        let e = ex(&[(&["John", "Smith"], "John Smith")]);
        assert_eq!(flash_fill(&e, &rows(&[&["Ana", "Lopez"]])).unwrap(), vec!["Ana Lopez"]);
        let e = ex(&[(&["Madrid", "ES"], "Madrid (ES)")]);
        assert_eq!(flash_fill(&e, &rows(&[&["Paris", "FR"]])).unwrap(), vec!["Paris (FR)"]);
    }

    #[test]
    fn no_program_or_missing_tokens() {
        assert!(flash_fill(&[], &[]).is_none());
        // Examples no program explains.
        let e = ex(&[(&["abc"], "x1"), (&["abd"], "y2")]);
        assert!(flash_fill(&e, &rows(&[&["abe"]])).is_none());
        // A row without the token gives an empty result.
        let e = ex(&[(&["Ann Lee Park"], "Lee")]);
        assert_eq!(flash_fill(&e, &rows(&[&["Solo"]])).unwrap(), vec![""]);
    }
}
