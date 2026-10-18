//! A book's revision: words repeated close together, filler words, how
//! much of each scene is dialogue and names written in more than one way.
//! Only body text counts, not headings.

use crate::{Document, ParaKind};

/// A word in the text: its paragraph and byte range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub para: usize,
    pub start: usize,
    pub end: usize,
}

/// A word written again soon after.
#[derive(Clone, Debug, PartialEq)]
pub struct Repeat {
    pub word: String,
    pub first: Hit,
    pub again: Hit,
}

/// Lowercase without accents.
fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

/// The words of a text with their byte ranges (an apostrophe or hyphen
/// between letters stays in the word).
fn words(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let inner = matches!(c, '\'' | '’' | '-') && start.is_some() && it.peek().is_some_and(|(_, n)| n.is_alphanumeric());
        if c.is_alphanumeric() || inner {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push((s, i));
        }
    }
    if let Some(s) = start {
        out.push((s, text.len()));
    }
    out
}

/// Common words not worth a warning when repeated, by language.
fn common(lang: &str) -> &'static [&'static str] {
    match lang {
        "es" => &["porque", "cuando", "donde", "entre", "desde", "hasta", "sobre", "todos", "todas", "estaba", "estaban", "habia", "habian", "tenia", "despues", "antes", "mientras", "aunque", "nunca", "siempre", "ahora", "entonces", "algo", "nada", "otra", "otro", "otros", "otras", "mismo", "misma", "puede", "hacia", "tiene", "estoy", "estas", "quiero", "dijo", "pregunto", "respondio"],
        "en" => &["their", "there", "where", "which", "while", "would", "could", "should", "about", "after", "before", "other", "these", "those", "never", "always", "again", "still", "something", "nothing", "into", "from", "with", "what", "when", "were", "said", "asked"],
        "fr" => &["comme", "quand", "alors", "avait", "était", "etait", "aussi", "toujours", "jamais", "encore", "quelque", "autre", "autres", "leurs", "notre", "votre", "entre", "depuis", "avant", "apres", "pendant", "parce", "dans", "avec", "pour", "sans", "sous", "elle", "elles"],
        "de" => &["nicht", "einen", "einem", "einer", "eines", "wieder", "immer", "schon", "noch", "dann", "aber", "oder", "wenn", "weil", "dass", "sich", "sein", "seine", "ihre", "ihren", "hatte", "hatten", "waren", "wurde", "sagte"],
        "it" => &["quando", "perche", "mentre", "ancora", "sempre", "allora", "anche", "dopo", "prima", "aveva", "erano", "questo", "questa", "quello", "quella", "altro", "altra", "nella", "della", "dalla", "sulla", "disse"],
        "pt" => &["porque", "quando", "onde", "entre", "desde", "sobre", "todos", "todas", "estava", "tinha", "depois", "antes", "enquanto", "embora", "nunca", "sempre", "agora", "entao", "outra", "outro", "mesmo", "mesma", "disse"],
        _ => &[],
    }
}

/// Filler words to look for, by language.
pub fn fillers_of(lang: &str) -> &'static [&'static str] {
    match lang {
        "es" => &["realmente", "simplemente", "básicamente", "literalmente", "totalmente", "absolutamente", "justo", "de repente", "en realidad", "de hecho", "un poco", "algo así", "como que", "entonces", "bueno", "pues", "muy", "solo", "casi", "incluso", "lentamente", "suavemente"],
        "en" => &["really", "very", "just", "actually", "basically", "literally", "totally", "suddenly", "somehow", "simply", "quite", "rather", "a bit", "a little", "kind of", "sort of", "started to", "began to", "slowly", "then"],
        "fr" => &["vraiment", "très", "juste", "simplement", "en fait", "littéralement", "totalement", "soudain", "soudainement", "un peu", "plutôt", "alors", "lentement", "doucement"],
        "de" => &["wirklich", "sehr", "eigentlich", "einfach", "plötzlich", "irgendwie", "ziemlich", "ein bisschen", "ein wenig", "halt", "eben", "langsam"],
        "it" => &["davvero", "molto", "proprio", "semplicemente", "praticamente", "letteralmente", "totalmente", "improvvisamente", "un po'", "piuttosto", "allora", "lentamente"],
        "pt" => &["realmente", "muito", "apenas", "simplesmente", "basicamente", "literalmente", "totalmente", "de repente", "na verdade", "um pouco", "meio que", "então", "lentamente"],
        _ => &[],
    }
}

impl Document {
    /// Body paragraphs among `paras`.
    fn body(&self, paras: std::ops::Range<usize>) -> impl Iterator<Item = usize> + '_ {
        paras.filter(|&i| self.paragraphs.get(i).is_some_and(|p| !matches!(p.style.kind, ParaKind::Heading(_)) && !p.text.is_empty()))
    }

    /// Words of `min` letters or more written again within `window` words
    /// in paragraphs `paras` (common words of `lang` left out).
    pub fn repeats(&self, paras: std::ops::Range<usize>, window: usize, min: usize, lang: &str) -> Vec<Repeat> {
        let skip = common(lang);
        let mut recent: std::collections::VecDeque<(String, Hit, usize)> = std::collections::VecDeque::new();
        let mut out = Vec::new();
        let mut n = 0;
        for i in self.body(paras) {
            let text = &self.paragraphs[i].text;
            for (s, e) in words(text) {
                n += 1;
                while recent.front().is_some_and(|r| r.2 + window < n) {
                    recent.pop_front();
                }
                let w = &text[s..e];
                if w.chars().count() < min || w.chars().any(|c| c.is_numeric()) {
                    continue;
                }
                let f = fold(w);
                if skip.contains(&f.as_str()) {
                    continue;
                }
                let hit = Hit { para: i, start: s, end: e };
                if let Some((_, first, _)) = recent.iter().find(|(x, ..)| *x == f) {
                    out.push(Repeat { word: w.to_string(), first: *first, again: hit });
                }
                recent.push_back((f, hit, n));
            }
        }
        out
    }

    /// Where each of `fillers` is written, most used first.
    pub fn fillers(&self, paras: std::ops::Range<usize>, fillers: &[&str]) -> Vec<(String, Vec<Hit>)> {
        let mut out: Vec<(String, Vec<Hit>)> = fillers.iter().map(|f| (f.to_string(), Vec::new())).collect();
        let wanted: Vec<Vec<String>> = fillers.iter().map(|f| f.split_whitespace().map(fold).collect()).collect();
        for i in self.body(paras) {
            let text = &self.paragraphs[i].text;
            let ws = words(text);
            let folded: Vec<String> = ws.iter().map(|&(s, e)| fold(&text[s..e])).collect();
            for (k, want) in wanted.iter().enumerate() {
                let n = want.len();
                if n == 0 || n > folded.len() {
                    continue;
                }
                for j in 0..=folded.len() - n {
                    if folded[j..j + n] == want[..] {
                        out[k].1.push(Hit { para: i, start: ws[j].0, end: ws[j + n - 1].1 });
                    }
                }
            }
        }
        out.retain(|x| !x.1.is_empty());
        out.sort_by_key(|x| std::cmp::Reverse(x.1.len()));
        out
    }

    /// How much of paragraphs `paras` is dialogue (0 to 1): paragraphs
    /// opened by a dash and text between quotes.
    pub fn dialogue_share(&self, paras: std::ops::Range<usize>) -> f32 {
        let (mut all, mut said) = (0usize, 0usize);
        for i in self.body(paras) {
            let text = &self.paragraphs[i].text;
            let n = text.chars().filter(|c| !c.is_whitespace()).count();
            all += n;
            let t = text.trim_start();
            if t.starts_with(['—', '–', '―']) || t.starts_with("- ") {
                said += n;
                continue;
            }
            let mut inside = None::<char>;
            for c in text.chars() {
                match (inside, c) {
                    (None, '“' | '«' | '"' | '„' | '「') => inside = Some(c),
                    (Some('“'), '”') | (Some('«'), '»') | (Some('"'), '"') | (Some('„'), '“') | (Some('「'), '」') => inside = None,
                    (Some(_), c) if !c.is_whitespace() => said += 1,
                    _ => {}
                }
            }
        }
        if all == 0 { 0.0 } else { said as f32 / all as f32 }
    }

    /// Names written in more than one way: capitalized words not opening
    /// a sentence that differ only in accents or by one letter. Each group
    /// is its spellings with where they are, most used first.
    pub fn name_variants(&self, paras: std::ops::Range<usize>) -> Vec<Vec<(String, Vec<Hit>)>> {
        // Each capitalized word with where it is and whether it was ever
        // seen inside a sentence; a word opening sentences that is also
        // written in lowercase is an ordinary word.
        let mut names: Vec<(String, Vec<Hit>, bool)> = Vec::new();
        let mut lower = std::collections::HashSet::new();
        for i in self.body(paras.clone()) {
            let text = &self.paragraphs[i].text;
            let mut prev_end = 0;
            for (s, e) in words(text) {
                let w = &text[s..e];
                let before = text[prev_end..s].trim();
                prev_end = e;
                if !w.chars().next().is_some_and(char::is_uppercase) {
                    lower.insert(w.to_string());
                    continue;
                }
                if w.chars().count() < 3 || w.chars().skip(1).any(char::is_uppercase) {
                    continue;
                }
                let opens = text[..s].trim().is_empty() || before.ends_with(['.', '!', '?', '¡', '¿', '—', '–', '"', '“', '«', ':']);
                let hit = Hit { para: i, start: s, end: e };
                match names.iter_mut().find(|x| x.0 == w) {
                    Some(x) => {
                        x.1.push(hit);
                        x.2 |= !opens;
                    }
                    None => names.push((w.to_string(), vec![hit], !opens)),
                }
            }
        }
        names.retain(|x| x.2 || !lower.contains(&x.0.to_lowercase()));
        let close = |a: &str, b: &str| {
            let (fa, fb) = (fold(a), fold(b));
            fa == fb || fa.chars().count() >= 5 && edit1(&fa, &fb)
        };
        let mut used = vec![false; names.len()];
        let mut out = Vec::new();
        for i in 0..names.len() {
            if used[i] {
                continue;
            }
            let mut group = vec![i];
            for j in i + 1..names.len() {
                if !used[j] && group.iter().any(|&k| close(&names[k].0, &names[j].0)) {
                    group.push(j);
                    used[j] = true;
                }
            }
            if group.len() > 1 && group.iter().any(|&k| names[k].2) {
                let mut g: Vec<(String, Vec<Hit>)> = group.into_iter().map(|k| (names[k].0.clone(), names[k].1.clone())).collect();
                g.sort_by_key(|x| std::cmp::Reverse(x.1.len()));
                out.push(g);
            }
        }
        out
    }
}

/// Whether `a` and `b` differ by one letter changed, added or taken away
/// (or two letters swapped).
fn edit1(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a == b {
        return false;
    }
    let (s, l) = if a.len() <= b.len() { (&a, &b) } else { (&b, &a) };
    match l.len() - s.len() {
        0 => {
            let diff: Vec<usize> = (0..s.len()).filter(|&i| s[i] != l[i]).collect();
            diff.len() == 1 || diff.len() == 2 && diff[1] == diff[0] + 1 && s[diff[0]] == l[diff[1]] && s[diff[1]] == l[diff[0]]
        }
        1 => {
            let i = (0..s.len()).find(|&i| s[i] != l[i]).unwrap_or(s.len());
            s[i..] == l[i + 1..]
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paragraph;

    fn doc(texts: &[&str]) -> Document {
        Document::from_paragraphs(texts.iter().map(|&t| Paragraph::new(t)).collect())
    }

    #[test]
    fn words_repeated_close_together_are_found() {
        let d = doc(&["La puerta crujió. Nadie abrió la puerta, porque la casa dormía.", "Lejos, otra puerta."]);
        let r = d.repeats(0..2, 6, 5, "es");
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].word.as_str(), r[0].first.para, r[0].again.para), ("puerta", 0, 0));
        assert_eq!(&d.paragraphs[0].text[r[0].again.start..r[0].again.end], "puerta");
        // "porque" is common; far enough, it is not a repeat.
        assert_eq!(d.repeats(0..2, 40, 5, "es").len(), 2);
    }

    #[test]
    fn fillers_dialogue_and_names_are_found() {
        let d = doc(&["Realmente era muy tarde, de repente muy tarde.", "—¿Vienes? —preguntó Marta.", "Martha dijo «sí» y Ángel calló.", "Angel miró a Marta."]);
        let f = d.fillers(0..4, fillers_of("es"));
        assert_eq!(f[0].0, "muy");
        assert_eq!(f[0].1.len(), 2);
        assert!(f.iter().any(|x| x.0 == "de repente" && x.1.len() == 1));
        let s = d.dialogue_share(1..2);
        assert!((s - 1.0).abs() < 1e-6);
        let s = d.dialogue_share(2..3);
        assert!(s > 0.0 && s < 0.2, "{s}");
        let names = d.name_variants(0..4);
        let groups: Vec<Vec<&str>> = names.iter().map(|g| g.iter().map(|x| x.0.as_str()).collect()).collect();
        assert!(groups.contains(&vec!["Marta", "Martha"]), "{groups:?}");
        assert!(groups.iter().any(|g| g.contains(&"Ángel") && g.contains(&"Angel")), "{groups:?}");
        assert!(edit1("marta", "matra") && !edit1("marta", "carla"));
    }
}
