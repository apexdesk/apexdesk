//! Searching a notebook's text: every word asked for, without caring for
//! case or accents, best pages first.

use super::Notebook;

/// A page that has every word searched for.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub page: i64,
    /// The title has one of the words.
    pub in_title: bool,
    /// Times the words are in the text.
    pub count: usize,
    /// Text around the first one, and where it is in it (bytes).
    pub snippet: String,
    pub mark: Option<(usize, usize)>,
}

/// One char for each char of `c`: lowercase without accents.
fn fold_char(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' => 'a',
        'é' | 'è' | 'ë' | 'ê' => 'e',
        'í' | 'ì' | 'ï' | 'î' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
        'ú' | 'ù' | 'ü' | 'û' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        c => c,
    }
}

/// Lowercase without accents, a char for each char.
pub fn fold(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

/// Byte ranges in `text` where `word` (folded) is.
pub(super) fn find_all(text: &str, word: &str) -> Vec<(usize, usize)> {
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect();
    let folded: Vec<char> = text.chars().map(fold_char).collect();
    let w: Vec<char> = word.chars().collect();
    if w.is_empty() || w.len() > folded.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + w.len() <= folded.len() {
        if folded[i..i + w.len()] == w[..] {
            out.push((starts[i], starts[i + w.len()]));
            i += w.len();
        } else {
            i += 1;
        }
    }
    out
}

/// About `around` chars of `text` on each side of `at`.
fn snippet(text: &str, at: (usize, usize), around: usize) -> (String, (usize, usize)) {
    let from = text[..at.0].char_indices().rev().nth(around).map_or(0, |(i, _)| i);
    let to = text[at.1..].char_indices().nth(around).map_or(text.len(), |(i, _)| at.1 + i);
    let mut s: String = text[from..to].chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut mark = (at.0 - from, at.1 - from);
    if from > 0 {
        s.insert(0, '…');
        mark = (mark.0 + '…'.len_utf8(), mark.1 + '…'.len_utf8());
    }
    if to < text.len() {
        s.push('…');
    }
    (s, mark)
}

impl Notebook {
    /// The pages (not in the trash) with every word of `query`, best
    /// first: with them in the title, then the most times.
    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        let words: Vec<String> = fold(query).split_whitespace().map(String::from).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        for p in &self.pages {
            if !self.is_live(p.id) {
                continue;
            }
            let text = self.texts.get(&p.id).map_or("", String::as_str);
            let title = fold(&p.title);
            let mut count = 0;
            let mut first: Option<(usize, usize)> = None;
            let mut all = true;
            let mut in_title = false;
            for w in &words {
                let found = find_all(text, w);
                let t = title.contains(w.as_str());
                if found.is_empty() && !t {
                    all = false;
                    break;
                }
                in_title |= t;
                count += found.len();
                if let Some(&f) = found.first() {
                    first = Some(first.map_or(f, |x| x.min(f)));
                }
            }
            if !all {
                continue;
            }
            let (snippet, mark) = match first {
                Some(at) => {
                    let (s, m) = snippet(text, at, 40);
                    (s, Some(m))
                }
                None => (text.chars().take(80).map(|c| if c.is_control() { ' ' } else { c }).collect(), None),
            };
            hits.push(SearchHit { page: p.id, in_title, count, snippet, mark });
        }
        hits.sort_by_key(|h| (!h.in_title, std::cmp::Reverse(h.count)));
        hits
    }
}
