//! Multilevel lists: list definitions with nine levels (numbering style,
//! label pattern and start value), set numbering values and numbered
//! headings (1, 1.1, 1.1.1).

use crate::{Document, EditKind, ParaKind, Selection, Styles, bullet_char, number_label};

pub const LEVELS: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum NumStyle {
    #[default]
    Decimal,
    /// 01, 02...
    DecimalZero,
    LowerLetter,
    UpperLetter,
    LowerRoman,
    UpperRoman,
    Bullet,
    /// No number: the label is the pattern's text alone.
    None,
}

impl NumStyle {
    pub const ALL: [NumStyle; 8] = [NumStyle::Decimal, NumStyle::DecimalZero, NumStyle::LowerLetter, NumStyle::UpperLetter, NumStyle::LowerRoman, NumStyle::UpperRoman, NumStyle::Bullet, NumStyle::None];

    /// `n` in this style ("" for bullets and none).
    pub fn format(self, n: u32) -> String {
        let n = n as usize;
        match self {
            NumStyle::Decimal => n.to_string(),
            NumStyle::DecimalZero => format!("{n:02}"),
            NumStyle::LowerLetter => crate::letters(n),
            NumStyle::UpperLetter => crate::letters(n).to_uppercase(),
            NumStyle::LowerRoman => crate::roman(n),
            NumStyle::UpperRoman => crate::roman(n).to_uppercase(),
            NumStyle::Bullet | NumStyle::None => String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Hash)]
pub struct ListLevel {
    pub style: NumStyle,
    /// The label: `%1`..`%9` stand for the numbers of levels 1..9; for
    /// bullets, the glyph.
    pub text: String,
    pub start: u32,
    /// Legal style: the numbers of all levels in this label are arabic.
    pub legal: bool,
}

impl ListLevel {
    pub fn new(style: NumStyle, text: &str) -> ListLevel {
        ListLevel { style, text: text.to_string(), start: 1, legal: false }
    }
}

#[derive(Clone, Debug, PartialEq, Hash, Default)]
pub struct ListDef {
    pub levels: Vec<ListLevel>,
}

/// What goes before a list paragraph.
#[derive(Clone, Debug, PartialEq)]
pub enum Marker {
    Bullet(char),
    Text(String),
}

impl ListDef {
    fn from(f: impl Fn(usize) -> ListLevel) -> ListDef {
        ListDef { levels: (0..LEVELS).map(f).collect() }
    }

    /// 1. 1.1. 1.1.1.
    pub fn outline() -> ListDef {
        ListDef::from(|l| ListLevel::new(NumStyle::Decimal, &(1..=l + 1).map(|k| format!("%{k}.")).collect::<String>()))
    }

    /// 1) a) i) (1) (a) (i) 1. a. i.
    pub fn classic() -> ListDef {
        const T: [(NumStyle, &str); 9] = [
            (NumStyle::Decimal, "%1)"),
            (NumStyle::LowerLetter, "%2)"),
            (NumStyle::LowerRoman, "%3)"),
            (NumStyle::Decimal, "(%4)"),
            (NumStyle::LowerLetter, "(%5)"),
            (NumStyle::LowerRoman, "(%6)"),
            (NumStyle::Decimal, "%7."),
            (NumStyle::LowerLetter, "%8."),
            (NumStyle::LowerRoman, "%9."),
        ];
        ListDef::from(|l| ListLevel::new(T[l].0, T[l].1))
    }

    /// I. A. 1. a) (1) (a) i) ...
    pub fn roman() -> ListDef {
        const T: [(NumStyle, &str); 9] = [
            (NumStyle::UpperRoman, "%1."),
            (NumStyle::UpperLetter, "%2."),
            (NumStyle::Decimal, "%3."),
            (NumStyle::LowerLetter, "%4)"),
            (NumStyle::Decimal, "(%5)"),
            (NumStyle::LowerLetter, "(%6)"),
            (NumStyle::LowerRoman, "%7."),
            (NumStyle::LowerLetter, "%8."),
            (NumStyle::LowerRoman, "%9."),
        ];
        ListDef::from(|l| ListLevel::new(T[l].0, T[l].1))
    }

    /// Bullets • ◦ ▪ by level.
    pub fn bullets() -> ListDef {
        ListDef::from(|l| ListLevel::new(NumStyle::Bullet, &bullet_char(0, l as u8).to_string()))
    }

    /// Headings: 1, 1.1, 1.1.1 (no final dot).
    pub fn headings() -> ListDef {
        ListDef::from(|l| ListLevel::new(NumStyle::Decimal, &(1..=l + 1).map(|k| format!("%{k}")).collect::<Vec<_>>().join(".")))
    }

    /// The galleries' lists (the last two are for headings).
    pub fn presets() -> Vec<ListDef> {
        vec![ListDef::outline(), ListDef::classic(), ListDef::roman(), ListDef::bullets(), ListDef::headings(), ListDef::outline()]
    }

    pub fn level(&self, l: usize) -> Option<&ListLevel> {
        self.levels.get(l.min(LEVELS - 1))
    }

    /// Several levels make up each label (1.1), or the styles differ from
    /// what a simple list would give.
    pub fn is_multilevel(&self) -> bool {
        self.levels.iter().any(|l| l.text.matches('%').count() > 1)
    }

    /// The marker of level `l` with the current `counters`.
    pub fn marker(&self, l: usize, counters: &[u32; LEVELS]) -> Marker {
        let Some(lv) = self.level(l) else { return Marker::Text(String::new()) };
        if lv.style == NumStyle::Bullet {
            return Marker::Bullet(lv.text.chars().next().unwrap_or('•'));
        }
        let mut out = String::new();
        let mut chars = lv.text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%' {
                if let Some(k) = chars.peek().and_then(|d| d.to_digit(10)).filter(|d| (1..=9).contains(d)) {
                    chars.next();
                    let k = k as usize - 1;
                    let style = match self.levels.get(k).map(|x| x.style) {
                        Some(s) if lv.legal && k != l => if matches!(s, NumStyle::Bullet | NumStyle::None) { s } else { NumStyle::Decimal },
                        Some(s) => s,
                        None => NumStyle::Decimal,
                    };
                    out.push_str(&style.format(counters[k]));
                    continue;
                }
            }
            out.push(c);
        }
        Marker::Text(out)
    }
}

impl Styles {
    /// The list definition `id` (1-based).
    pub fn list(&self, id: u16) -> Option<&ListDef> {
        self.lists.get((id as usize).checked_sub(1)?)
    }

    /// The id of `def`, added when new.
    fn list_id(&mut self, def: &ListDef) -> u16 {
        match self.lists.iter().position(|d| d == def) {
            Some(i) => i as u16 + 1,
            None => {
                self.lists.push(def.clone());
                self.lists.len() as u16
            }
        }
    }
}

impl Document {
    /// The marker of every paragraph: list items, and headings when they
    /// are numbered.
    pub fn list_markers(&self) -> Vec<Option<Marker>> {
        let mut counters: std::collections::HashMap<u16, ([u32; LEVELS], [bool; LEVELS])> = Default::default();
        let heading = self.styles.heading_list;
        let mut out = Vec::with_capacity(self.paragraphs.len());
        let mut numbers: Option<Vec<usize>> = None;
        for (i, p) in self.paragraphs.iter().enumerate() {
            let st = &p.style;
            let (id, level) = match st.kind {
                ParaKind::Heading(h) if heading > 0 => (heading, h as usize - 1),
                ParaKind::Bullet | ParaKind::Numbered if st.list > 0 => (st.list, st.level as usize),
                ParaKind::Numbered => {
                    out.push(Some(Marker::Text(number_label(st.list_format, st.level, numbers.get_or_insert_with(|| self.list_numbers())[i]))));
                    continue;
                }
                ParaKind::Bullet => {
                    out.push(Some(Marker::Bullet(bullet_char(st.list_format, st.level))));
                    continue;
                }
                _ => {
                    out.push(None);
                    continue;
                }
            };
            let Some(def) = self.styles.list(id) else {
                out.push(None);
                continue;
            };
            let level = level.min(LEVELS - 1);
            let start = |l: usize| def.level(l).map_or(1, |x| x.start);
            let (c, started) = counters.entry(id).or_insert_with(|| (std::array::from_fn(start), [false; LEVELS]));
            if st.restart {
                *c = std::array::from_fn(start);
                *started = [false; LEVELS];
            }
            c[level] = if st.start_at > 0 {
                st.start_at as u32
            } else if started[level] {
                c[level] + 1
            } else {
                start(level)
            };
            started[level] = true;
            for l in level + 1..LEVELS {
                c[l] = start(l);
                started[l] = false;
            }
            out.push(Some(def.marker(level, c)));
        }
        out
    }

    /// Records a change of paragraphs `from..=to` that also changed the
    /// lists (`styles` is what they were).
    fn record_lists(&mut self, from: usize, before: Vec<crate::Paragraph>, sel: Selection, styles: Styles) {
        let n = before.len();
        self.record(from, before, n, sel, sel, EditKind::Paragraph);
        if let Some(e) = self.undo.last_mut() {
            e.styles = Some(Box::new(styles));
        }
    }

    /// Makes paragraphs `from..=to` items of the list `def`. When they
    /// all belong to one multilevel list already, the whole list changes.
    pub fn apply_list(&mut self, from: usize, to: usize, sel: Selection, def: &ListDef) {
        let styles = self.styles.clone();
        let cur = self.paragraphs[from].style.list;
        let same = cur > 0 && self.paragraphs[from..=to].iter().all(|p| p.style.kind.is_list() && p.style.list == cur);
        if same {
            self.styles.lists[cur as usize - 1] = def.clone();
            // The kind of each item follows its level's style.
            let (a, b) = (0, self.paragraphs.len() - 1);
            let before = self.paragraphs.clone();
            for p in &mut self.paragraphs[a..=b] {
                if p.style.kind.is_list() && p.style.list == cur {
                    p.style.kind = kind_for(def, p.style.level);
                }
            }
            self.record_lists(0, before, sel, styles);
            return;
        }
        let id = self.styles.list_id(def);
        let before = self.paragraphs[from..=to].to_vec();
        for p in &mut self.paragraphs[from..=to] {
            if !p.style.kind.is_list() {
                p.style.level = 0;
            }
            p.style.list = id;
            p.style.list_format = 0;
            p.style.kind = kind_for(def, p.style.level);
        }
        self.record_lists(from, before, sel, styles);
    }

    /// Set Numbering Value: the item `para` gets `value` (None: continues
    /// the list; `Some(0)`... is taken as 1).
    pub fn set_list_value(&mut self, para: usize, sel: Selection, value: Option<u32>) {
        let v = value.map_or(0, |v| v.clamp(1, u16::MAX as u32) as u16);
        self.set_para_style(para, para, sel, &|s| {
            s.start_at = v;
            s.restart = false;
        });
    }

    /// Numbers the headings with `def` (None: no numbers).
    pub fn set_heading_numbering(&mut self, def: Option<&ListDef>, sel: Selection) {
        let styles = self.styles.clone();
        self.styles.heading_list = match def {
            Some(d) => self.styles.list_id(d),
            None => 0,
        };
        if self.styles != styles {
            let before = vec![self.paragraphs[0].clone()];
            self.record_lists(0, before, sel, styles);
        }
    }

    /// The multilevel list of paragraph `i`, or of the headings.
    pub fn list_def_of(&self, i: usize) -> Option<&ListDef> {
        let st = &self.paragraphs.get(i)?.style;
        match st.kind {
            ParaKind::Heading(_) => self.styles.list(self.styles.heading_list),
            k if k.is_list() => self.styles.list(st.list),
            _ => None,
        }
    }
}

fn kind_for(def: &ListDef, level: u8) -> ParaKind {
    match def.level(level as usize).map(|l| l.style) {
        Some(NumStyle::Bullet) => ParaKind::Bullet,
        _ => ParaKind::Numbered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Paragraph, Pos};

    fn text(m: &Option<Marker>) -> String {
        match m {
            Some(Marker::Text(t)) => t.clone(),
            Some(Marker::Bullet(c)) => c.to_string(),
            None => String::new(),
        }
    }

    #[test]
    fn outline_numbers() {
        let mut d = Document::from_paragraphs((0..6).map(|i| Paragraph::new(&format!("p{i}"))).collect());
        let sel = Selection::caret(Pos::new(0, 0));
        d.apply_list(0, 5, sel, &ListDef::outline());
        for (i, l) in [0, 1, 1, 2, 0, 1].iter().enumerate() {
            d.paragraphs[i].style.level = *l;
        }
        let m: Vec<String> = d.list_markers().iter().map(text).collect();
        assert_eq!(m, ["1.", "1.1.", "1.2.", "1.2.1.", "2.", "2.1."]);
        d.set_list_value(4, sel, Some(7));
        let m: Vec<String> = d.list_markers().iter().map(text).collect();
        assert_eq!(m[4..], ["7.".to_string(), "7.1.".to_string()]);
        // Changing the list of all its items changes the definition.
        d.apply_list(0, 5, sel, &ListDef::roman());
        assert_eq!(d.styles.lists.len(), 1);
        assert_eq!(text(&d.list_markers()[3]), "1.");
        assert_eq!(text(&d.list_markers()[1]), "A.");
        d.undo();
        assert_eq!(d.styles.lists[0], ListDef::outline());
    }

    #[test]
    fn heading_numbers() {
        let mut d = Document::from_paragraphs(["A", "x", "B", "C", "D"].iter().map(|t| Paragraph::new(*t)).collect());
        for (i, h) in [(0, 1), (2, 2), (3, 2), (4, 1)] {
            d.paragraphs[i].style.kind = ParaKind::Heading(h);
        }
        d.set_heading_numbering(Some(&ListDef::headings()), Selection::caret(Pos::new(0, 0)));
        let m: Vec<String> = d.list_markers().iter().map(text).collect();
        assert_eq!(m, ["1", "", "1.1", "1.2", "2"]);
        d.undo();
        assert!(d.list_markers().iter().all(|m| m.is_none()));
    }
}
