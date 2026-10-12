//! Equations. They are typed in a linear format, a practical subset of
//! UnicodeMath (what Word's equation editor takes), parsed into a tree and
//! laid out into boxes of text, lines and curves.
//!
//! An equation sits in a paragraph as an `OBJECT` character whose `math`
//! attribute holds it, like a picture.

use std::sync::Arc;

use crate::{Attrs, Document, EditKind, OBJECT, Paragraph, Pos, Selection};

/// An equation tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Row(Vec<Node>),
    /// Letters, shown in italics.
    Ident(String),
    Number(String),
    /// A binary operator, relation or punctuation (spaced).
    Op(char),
    /// Any other symbol (∞, ∂, ′, a lone bracket...), upright.
    Sym(String),
    /// Upright text: `"..."`.
    Text(String),
    Frac(Box<Node>, Box<Node>),
    Script { base: Box<Node>, sub: Option<Box<Node>>, sup: Option<Box<Node>> },
    Radical { index: Option<Box<Node>>, body: Box<Node> },
    /// A big operator (∑, ∫...) with its limits and operand.
    Nary { op: char, sub: Option<Box<Node>>, sup: Option<Box<Node>>, body: Box<Node> },
    /// Brackets that grow with their content; `None` is an invisible one.
    Delim { open: Option<char>, close: Option<char>, body: Box<Node> },
    /// A function (`name` is `Text`, possibly with scripts) applied to `body`.
    Func { name: Box<Node>, body: Box<Node> },
    Matrix(Vec<Vec<Node>>),
}

impl Node {
    fn empty() -> Node {
        Node::Row(Vec::new())
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Node::Row(v) if v.is_empty())
    }
}

/// An equation: its linear source and parse tree.
#[derive(Clone, Debug)]
pub struct Equation {
    source: String,
    root: Node,
}

impl Equation {
    pub fn new(source: &str) -> Self {
        Equation { source: source.to_string(), root: parse(source) }
    }

    /// An equation for a tree read from a file: its linear form, parsed again.
    pub fn from_node(node: &Node) -> Self {
        Equation::new(&to_linear(node))
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    /// The equation written as LaTeX (for Markdown).
    pub fn latex(&self) -> String {
        to_latex(&self.root)
    }
}

impl PartialEq for Equation {
    fn eq(&self, o: &Self) -> bool {
        self.source == o.source
    }
}

impl Eq for Equation {}

impl std::hash::Hash for Equation {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.source.hash(h);
    }
}

/// Whether the paragraph is only an equation: a display equation, shown
/// centered and with limits above and below big operators.
pub fn is_display(p: &Paragraph) -> bool {
    let t = p.text.trim_matches([' ', '\t']);
    t.len() == OBJECT.len_utf8() && t.starts_with(OBJECT) && {
        let at = p.text.find(OBJECT).unwrap_or(0) + OBJECT.len_utf8();
        p.attrs_at(at).math.is_some()
    }
}

// --- document editing ------------------------------------------------------------

impl Document {
    /// Replaces the selection with an equation. Returns its extent.
    pub fn insert_equation(&mut self, sel: Selection, eq: Equation) -> Selection {
        let a = sel.start();
        let mut attrs = self.typing_attrs(a);
        (attrs.link, attrs.link_tip) = (None, None);
        attrs.math = Some(Arc::new(eq));
        let end = self.insert_text(sel, &OBJECT.to_string(), Some(attrs));
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Insert;
        }
        Selection { anchor: Pos::new(end.para, end.offset - OBJECT.len_utf8()), cursor: end }
    }

    /// The equation at `p` (the character right after it) and its extent.
    pub fn equation_at(&self, p: Pos) -> Option<(Selection, Arc<Equation>)> {
        let para = self.paragraphs.get(p.para)?;
        if !para.text.get(p.offset..)?.starts_with(OBJECT) {
            return None;
        }
        let eq = para.attrs_at(p.offset + OBJECT.len_utf8()).math?;
        Some((Selection { anchor: p, cursor: Pos::new(p.para, p.offset + OBJECT.len_utf8()) }, eq))
    }

    /// The equation the selection covers, or the one right before or after
    /// an empty selection.
    pub fn equation_near(&self, sel: Selection) -> Option<(Selection, Arc<Equation>)> {
        let (a, b) = (sel.start(), sel.end());
        if !sel.is_empty() {
            return self.equation_at(a).filter(|(s, _)| s.end() == b);
        }
        self.equation_at(a).or_else(|| (a.offset > 0).then(|| self.equation_at(self.prev_char(a))).flatten())
    }

    /// Changes the equation at `p` (one undo step).
    pub fn set_equation(&mut self, p: Pos, eq: Equation) {
        let Some((sel, _)) = self.equation_at(p) else { return };
        let eq = Arc::new(eq);
        self.format_range(sel, &|a: &mut Attrs| a.math = Some(eq.clone()));
    }
}

// --- linear format ---------------------------------------------------------------------

/// `\name` escapes: Greek letters, symbols and structures.
const ESCAPES: &[(&str, &str)] = &[
    ("alpha", "α"), ("beta", "β"), ("gamma", "γ"), ("delta", "δ"), ("epsilon", "ϵ"), ("varepsilon", "ε"), ("zeta", "ζ"),
    ("eta", "η"), ("theta", "θ"), ("vartheta", "ϑ"), ("iota", "ι"), ("kappa", "κ"), ("lambda", "λ"), ("mu", "μ"),
    ("nu", "ν"), ("xi", "ξ"), ("omicron", "ο"), ("pi", "π"), ("varpi", "ϖ"), ("rho", "ρ"), ("varrho", "ϱ"),
    ("sigma", "σ"), ("varsigma", "ς"), ("tau", "τ"), ("upsilon", "υ"), ("phi", "ϕ"), ("varphi", "φ"), ("chi", "χ"),
    ("psi", "ψ"), ("omega", "ω"), ("Gamma", "Γ"), ("Delta", "Δ"), ("Theta", "Θ"), ("Lambda", "Λ"), ("Xi", "Ξ"),
    ("Pi", "Π"), ("Sigma", "Σ"), ("Upsilon", "Υ"), ("Phi", "Φ"), ("Psi", "Ψ"), ("Omega", "Ω"),
    ("pm", "±"), ("mp", "∓"), ("times", "×"), ("cdot", "·"), ("div", "÷"), ("ast", "∗"), ("circ", "∘"), ("bullet", "∙"),
    ("le", "≤"), ("leq", "≤"), ("ge", "≥"), ("geq", "≥"), ("ne", "≠"), ("neq", "≠"), ("approx", "≈"), ("equiv", "≡"),
    ("sim", "∼"), ("simeq", "≃"), ("cong", "≅"), ("propto", "∝"), ("ll", "≪"), ("gg", "≫"), ("to", "→"),
    ("rightarrow", "→"), ("leftarrow", "←"), ("leftrightarrow", "↔"), ("Rightarrow", "⇒"), ("Leftarrow", "⇐"),
    ("Leftrightarrow", "⇔"), ("implies", "⇒"), ("iff", "⇔"), ("mapsto", "↦"), ("in", "∈"), ("notin", "∉"), ("ni", "∋"),
    ("subset", "⊂"), ("supset", "⊃"), ("subseteq", "⊆"), ("supseteq", "⊇"), ("cup", "∪"), ("cap", "∩"),
    ("wedge", "∧"), ("vee", "∨"), ("oplus", "⊕"), ("otimes", "⊗"), ("setminus", "∖"), ("perp", "⊥"),
    ("parallel", "∥"), ("mid", "∣"), ("infty", "∞"), ("partial", "∂"), ("nabla", "∇"), ("forall", "∀"),
    ("exists", "∃"), ("neg", "¬"), ("emptyset", "∅"), ("ldots", "…"), ("dots", "…"), ("cdots", "⋯"), ("vdots", "⋮"),
    ("ddots", "⋱"), ("prime", "′"), ("hbar", "ℏ"), ("ell", "ℓ"), ("Re", "ℜ"), ("Im", "ℑ"), ("aleph", "ℵ"),
    ("degree", "°"), ("angle", "∠"), ("sqrt", "√"), ("cbrt", "∛"), ("qdrt", "∜"), ("sum", "∑"), ("prod", "∏"),
    ("coprod", "∐"), ("int", "∫"), ("iint", "∬"), ("iiint", "∭"), ("oint", "∮"), ("bigcup", "⋃"), ("bigcap", "⋂"),
    ("bigvee", "⋁"), ("bigwedge", "⋀"), ("matrix", "■"), ("pmatrix", "\u{E000}"), ("bmatrix", "\u{E001}"),
    ("vmatrix", "\u{E002}"), ("naryand", "▒"), ("funcapply", " "), ("langle", "⟨"), ("rangle", "⟩"), ("lfloor", "⌊"),
    ("rfloor", "⌋"), ("lceil", "⌈"), ("rceil", "⌉"), ("lbrace", "{"), ("rbrace", "}"), ("Vert", "‖"), ("norm", "‖"),
    ("quad", " "), ("qquad", " "),
];

/// Names typeset upright as functions.
pub const FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "sinh", "cosh", "tanh", "coth", "arcsin", "arccos", "arctan", "arg", "deg",
    "det", "dim", "exp", "gcd", "hom", "inf", "ker", "lg", "lim", "liminf", "limsup", "ln", "log", "max", "min", "Pr",
    "sup", "mod",
];

/// Functions whose limits go below them in display equations.
fn limit_func(name: &str) -> bool {
    matches!(name, "lim" | "liminf" | "limsup" | "max" | "min" | "sup" | "inf" | "det" | "gcd" | "Pr")
}

/// The `\name` escape for a character (for LaTeX), if any.
fn escape_of(c: char) -> Option<&'static str> {
    let mut buf = [0u8; 4];
    let s: &str = c.encode_utf8(&mut buf);
    if c == ' ' || ('\u{E000}'..='\u{F8FF}').contains(&c) {
        return None;
    }
    ESCAPES.iter().find(|(n, r)| *r == s && !matches!(*n, "leq" | "geq" | "neq" | "rightarrow" | "implies" | "iff" | "dots" | "norm" | "lbrace" | "rbrace")).map(|(n, _)| *n)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpClass {
    Rel,
    Bin,
    Punct,
}

/// Spacing class of an operator character; `None` for other characters.
pub fn op_class(c: char) -> Option<OpClass> {
    match c {
        '=' | '≠' | '<' | '>' | '≤' | '≥' | '≈' | '≡' | '∼' | '≃' | '≅' | '∝' | '≪' | '≫' | '→' | '←' | '↔' | '⇒' | '⇐' | '⇔'
        | '↦' | '∈' | '∉' | '∋' | '⊂' | '⊃' | '⊆' | '⊇' | '∥' | '⊥' | '∣' | ':' | '≔' => Some(OpClass::Rel),
        '+' | '-' | '−' | '±' | '∓' | '×' | '·' | '÷' | '∗' | '*' | '∘' | '∙' | '∪' | '∩' | '∧' | '∨' | '⊕' | '⊗' | '∖' => Some(OpClass::Bin),
        ',' | ';' => Some(OpClass::Punct),
        _ => None,
    }
}

/// The character shown for an operator or symbol typed as `c`.
pub fn shown_char(c: char) -> char {
    match c {
        '-' => '−',
        '*' => '∗',
        '\'' => '′',
        _ => c,
    }
}

fn is_nary(c: char) -> bool {
    matches!(c, '∑' | '∏' | '∐' | '∫' | '∬' | '∭' | '∮' | '∯' | '∰' | '⋃' | '⋂' | '⋁' | '⋀' | '⨁' | '⨂')
}

pub fn is_integral(c: char) -> bool {
    matches!(c, '∫' | '∬' | '∭' | '∮' | '∯' | '∰')
}

fn is_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '⟩' | '⌋' | '⌉' | '〗' | '┤')
}

fn is_opener(c: char) -> bool {
    matches!(c, '(' | '[' | '{' | '⟨' | '⌊' | '⌈' | '|' | '‖' | '├')
}

/// Expands `\name` escapes and turns invisible separators into spaces.
fn expand(src: &str) -> Vec<char> {
    let s: Vec<char> = src.chars().collect();
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == '\\' {
            let mut j = i + 1;
            while j < s.len() && s[j].is_ascii_alphabetic() {
                j += 1;
            }
            if j > i + 1 {
                let name: String = s[i + 1..j].iter().collect();
                if FUNCTIONS.contains(&name.as_str()) {
                    out.extend(name.chars());
                    i = j;
                    continue;
                }
                if let Some((_, rep)) = ESCAPES.iter().find(|(n, _)| *n == name) {
                    out.extend(rep.chars());
                    i = j;
                    // The space that ends the name is eaten, as in Word.
                    if s.get(i) == Some(&' ') {
                        i += 1;
                    }
                    continue;
                }
            } else if let Some(&n) = s.get(i + 1) {
                let rep = match n {
                    '{' | '}' | '[' | ']' | '(' | ')' | '_' | '^' | '&' | '$' | '#' | '%' => Some(n),
                    '|' => Some('‖'),
                    ',' | ';' | ':' | '!' | ' ' => Some(' '),
                    _ => None,
                };
                if let Some(r) = rep {
                    out.push(r);
                    i += 2;
                    continue;
                }
            }
            out.push('\\');
            i += 1;
            continue;
        }
        out.push(match c {
            '−' => '-',
            '\u{2061}' | '\u{2062}' | '\u{2063}' | '\u{200B}' | '\u{A0}' | '\t' | '\n' | '\r' | '\u{2009}' | '\u{200A}' => ' ',
            c => c,
        });
        i += 1;
    }
    out
}

/// Characters that end a row.
#[derive(Clone, Copy, Default)]
struct Stops {
    close: bool,
    bar: bool,
    dbar: bool,
    amp: bool,
    at: bool,
}

impl Stops {
    const CLOSE: Stops = Stops { close: true, bar: false, dbar: false, amp: false, at: false };

    fn hit(self, c: char) -> bool {
        (self.close && is_closer(c)) || (self.bar && c == '|') || (self.dbar && c == '‖') || (self.amp && c == '&') || (self.at && c == '@')
    }
}

/// Nesting deeper than this is read as plain symbols (no stack overflow).
const MAX_DEPTH: u32 = 48;

struct Parser {
    c: Vec<char>,
    i: usize,
    depth: u32,
}

enum Item {
    Operand(Vec<Node>),
    Op(char),
}

/// A row from its items: a single item stands alone.
fn row(mut v: Vec<Node>) -> Node {
    if v.len() == 1 { v.pop().unwrap_or_else(Node::empty) } else { Node::Row(v) }
}

/// Drops the parentheses around an argument, as Word does: `(a+b)/c`.
fn unparen(mut v: Vec<Node>) -> Node {
    if v.len() == 1 {
        let n = v.pop().unwrap_or_else(Node::empty);
        return unparen_node(n);
    }
    Node::Row(v)
}

fn unparen_node(n: Node) -> Node {
    match n {
        Node::Delim { open: Some('('), close: Some(')'), body } => *body,
        n => n,
    }
}

/// Parses the linear format. Never fails: anything it doesn't understand
/// is kept as symbols.
pub fn parse(src: &str) -> Node {
    let mut p = Parser { c: expand(src), i: 0, depth: 0 };
    let mut out = Vec::new();
    loop {
        out.extend(p.row(Stops::default()));
        // A stray closer at the top level is a symbol.
        match p.peek() {
            Some(c) => {
                out.push(Node::Sym(c.to_string()));
                p.i += 1;
            }
            None => break,
        }
    }
    row(out)
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    fn skip_spaces(&mut self) {
        while self.peek() == Some(' ') {
            self.i += 1;
        }
    }

    /// Operands and operators up to a stop character or the end.
    fn row(&mut self, stops: Stops) -> Vec<Node> {
        let mut items: Vec<Item> = Vec::new();
        loop {
            self.skip_spaces();
            let Some(c) = self.peek() else { break };
            if stops.hit(c) {
                break;
            }
            if op_class(c).is_some() {
                items.push(Item::Op(c));
                self.i += 1;
                continue;
            }
            if c == '/' {
                self.i += 1;
                let num = match items.last() {
                    Some(Item::Operand(_)) => match items.pop() {
                        Some(Item::Operand(v)) => v,
                        _ => Vec::new(),
                    },
                    _ => Vec::new(),
                };
                self.skip_spaces();
                let den = self.signed_operand(stops);
                items.push(Item::Operand(vec![Node::Frac(Box::new(unparen(num)), Box::new(unparen(den)))]));
                continue;
            }
            let before = self.i;
            let v = self.operand(stops);
            if !v.is_empty() {
                items.push(Item::Operand(v));
            }
            if self.i == before {
                // No progress (can't happen, but never loop forever).
                items.push(Item::Operand(vec![Node::Sym(c.to_string())]));
                self.i += 1;
            }
        }
        let mut out = Vec::new();
        for it in items {
            match it {
                Item::Operand(v) => out.extend(v),
                Item::Op(c) => out.push(Node::Op(c)),
            }
        }
        out
    }

    /// An operand that may start with a sign (a denominator: `1/-x`).
    fn signed_operand(&mut self, stops: Stops) -> Vec<Node> {
        match self.peek() {
            Some(c @ ('-' | '+' | '−' | '±' | '∓')) => {
                self.i += 1;
                let mut v = vec![Node::Op(c)];
                v.extend(self.operand(stops));
                vec![Node::Row(v)]
            }
            _ => self.operand(stops),
        }
    }

    /// Factors up to a space, an operator, a fraction bar or a stop.
    fn operand(&mut self, stops: Stops) -> Vec<Node> {
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            if c == ' ' || c == '/' || stops.hit(c) || op_class(c).is_some() {
                break;
            }
            let before = self.i;
            if let Some(f) = self.factor(stops) {
                out.push(f);
            }
            if self.i == before {
                self.i += 1;
                out.push(Node::Sym(c.to_string()));
            }
        }
        out
    }

    /// A primary with its scripts.
    fn factor(&mut self, stops: Stops) -> Option<Node> {
        let base = match self.peek()? {
            '_' | '^' => Node::empty(),
            _ => self.primary(stops)?,
        };
        Some(self.scripts(base))
    }

    fn scripts(&mut self, mut base: Node) -> Node {
        let (mut sub, mut sup): (Option<Box<Node>>, Option<Box<Node>>) = (None, None);
        let mut nested = 0;
        loop {
            let is_sub = match self.peek() {
                Some('_') => true,
                Some('^') => false,
                _ => break,
            };
            let taken = if is_sub { sub.is_some() } else { sup.is_some() };
            if taken && (nested >= 8 || self.depth >= MAX_DEPTH) {
                // Keep the tree shallow: `x^^^^...` stays flat.
                break;
            }
            self.i += 1;
            let arg = Box::new(self.script_arg());
            if taken {
                nested += 1;
                // A second subscript (or superscript) applies to the whole.
                base = Node::Script { base: Box::new(base), sub: sub.take(), sup: sup.take() };
            }
            if is_sub {
                sub = Some(arg);
            } else {
                sup = Some(arg);
            }
        }
        if sub.is_none() && sup.is_none() { base } else { Node::Script { base: Box::new(base), sub, sup } }
    }

    fn script_arg(&mut self) -> Node {
        let Some(c) = self.peek() else { return Node::empty() };
        match c {
            '(' => self.primary(Stops::default()).map_or_else(Node::empty, unparen_node),
            '-' | '+' | '−' | '±' | '∓' => {
                self.i += 1;
                let rest = match self.peek() {
                    Some(n) if op_class(n).is_some() => Node::empty(),
                    _ => self.script_arg(),
                };
                Node::Row(vec![Node::Op(c), rest])
            }
            c if c.is_alphanumeric() => {
                let mut v = Vec::new();
                while let Some(c) = self.peek().filter(|c| c.is_alphanumeric()) {
                    let digit = c.is_ascii_digit();
                    let start = self.i;
                    while self.peek().is_some_and(|c| c.is_alphanumeric() && c.is_ascii_digit() == digit) {
                        self.i += 1;
                    }
                    let s: String = self.c[start..self.i].iter().collect();
                    v.push(if digit { Node::Number(s) } else { Node::Ident(s) });
                }
                row(v)
            }
            ' ' | '/' | '_' | '^' => Node::empty(),
            c if is_closer(c) || op_class(c).is_some() || c == '&' || c == '@' => Node::empty(),
            _ => self.primary(Stops::default()).unwrap_or_else(Node::empty),
        }
    }

    fn primary(&mut self, stops: Stops) -> Option<Node> {
        let c = self.peek()?;
        if self.depth >= MAX_DEPTH {
            self.i += 1;
            return Some(Node::Sym(c.to_string()));
        }
        self.depth += 1;
        let n = self.primary_inner(c, stops);
        self.depth -= 1;
        n
    }

    fn primary_inner(&mut self, c: char, stops: Stops) -> Option<Node> {
        if c.is_ascii_digit() || (c == '.' && self.c.get(self.i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = self.i;
            while let Some(d) = self.peek() {
                if d.is_ascii_digit() || (d == '.' && self.c.get(self.i + 1).is_some_and(|d| d.is_ascii_digit())) {
                    self.i += 1;
                } else {
                    break;
                }
            }
            return Some(Node::Number(self.c[start..self.i].iter().collect()));
        }
        if c.is_alphabetic() {
            let start = self.i;
            while self.peek().is_some_and(|c| c.is_alphabetic()) {
                self.i += 1;
            }
            let name: String = self.c[start..self.i].iter().collect();
            if FUNCTIONS.contains(&name.as_str()) {
                return Some(self.func(name, stops));
            }
            return Some(Node::Ident(name));
        }
        if is_opener(c) {
            return Some(self.delim(c));
        }
        if is_nary(c) {
            return Some(self.nary(c, stops));
        }
        self.i += 1;
        match c {
            '〖' => {
                let v = self.row(Stops::CLOSE);
                if self.peek() == Some('〗') {
                    self.i += 1;
                }
                Some(row(v))
            }
            '"' => {
                let start = self.i;
                while self.peek().is_some_and(|c| c != '"') {
                    self.i += 1;
                }
                let s: String = self.c[start..self.i].iter().collect();
                if self.peek() == Some('"') {
                    self.i += 1;
                }
                Some(Node::Text(s))
            }
            '√' | '∛' | '∜' => Some(self.radical(c)),
            '■' | '\u{E000}' | '\u{E001}' | '\u{E002}' => {
                if self.peek() != Some('(') {
                    return Some(Node::Sym("■".into()));
                }
                let m = self.matrix();
                Some(match c {
                    '\u{E000}' => Node::Delim { open: Some('('), close: Some(')'), body: Box::new(m) },
                    '\u{E001}' => Node::Delim { open: Some('['), close: Some(']'), body: Box::new(m) },
                    '\u{E002}' => Node::Delim { open: Some('|'), close: Some('|'), body: Box::new(m) },
                    _ => m,
                })
            }
            '▒' => None,
            _ => Some(Node::Sym(c.to_string())),
        }
    }

    fn delim(&mut self, open: char) -> Node {
        self.i += 1;
        let stops = Stops { close: true, bar: open == '|', dbar: open == '‖', ..Default::default() };
        let body = self.row(stops);
        let close = match self.peek() {
            Some('┤') => {
                self.i += 1;
                None
            }
            // Bars only close bars; brackets close any bracket: [0,1).
            Some(c) if if matches!(open, '|' | '‖') { c == open } else { is_closer(c) && c != '〗' } => {
                self.i += 1;
                Some(c)
            }
            _ => None,
        };
        Node::Delim { open: (open != '├').then_some(open), close, body: Box::new(row(body)) }
    }

    fn radical(&mut self, c: char) -> Node {
        let degree = match c {
            '∛' => Some(Box::new(Node::Number("3".into()))),
            '∜' => Some(Box::new(Node::Number("4".into()))),
            _ => None,
        };
        if self.peek() == Some('(') {
            self.i += 1;
            let first = self.row(Stops { close: true, amp: true, ..Default::default() });
            let (index, body) = if self.peek() == Some('&') {
                self.i += 1;
                let b = self.row(Stops::CLOSE);
                (Some(Box::new(row(first))), row(b))
            } else {
                (degree, row(first))
            };
            if self.peek() == Some(')') {
                self.i += 1;
            }
            return Node::Radical { index, body: Box::new(body) };
        }
        self.skip_spaces();
        let body = match self.peek() {
            Some(c) if c != '/' && op_class(c).is_none() && !is_closer(c) && c != '&' && c != '@' => self.factor(Stops::default()).unwrap_or_else(Node::empty),
            _ => Node::empty(),
        };
        Node::Radical { index: degree, body: Box::new(body) }
    }

    fn nary(&mut self, op: char, stops: Stops) -> Node {
        self.i += 1;
        let (mut sub, mut sup) = (None, None);
        loop {
            match self.peek() {
                Some('_') => {
                    self.i += 1;
                    sub = Some(Box::new(self.script_arg()));
                }
                Some('^') => {
                    self.i += 1;
                    sup = Some(Box::new(self.script_arg()));
                }
                _ => break,
            }
        }
        self.skip_spaces();
        let body = if self.peek() == Some('▒') {
            self.i += 1;
            self.skip_spaces();
            match self.peek() {
                Some(c) if !stops.hit(c) && op_class(c).is_none() && c != '/' => self.factor(stops).unwrap_or_else(Node::empty),
                _ => Node::empty(),
            }
        } else {
            row(self.operand(stops))
        };
        Node::Nary { op, sub, sup, body: Box::new(body) }
    }

    fn func(&mut self, name: String, stops: Stops) -> Node {
        let name = self.scripts(Node::Text(name));
        self.skip_spaces();
        let body = match self.peek() {
            Some(c) if !stops.hit(c) && op_class(c).is_none() && c != '/' && c != '_' && c != '^' => row(self.operand(stops)),
            _ => Node::empty(),
        };
        Node::Func { name: Box::new(name), body: Box::new(body) }
    }

    fn matrix(&mut self) -> Node {
        // At the '('.
        self.i += 1;
        let mut rows = vec![Vec::new()];
        loop {
            let cell = self.row(Stops { close: true, amp: true, at: true, ..Default::default() });
            if let Some(r) = rows.last_mut() {
                r.push(row(cell));
            }
            match self.peek() {
                Some('&') => self.i += 1,
                Some('@') => {
                    self.i += 1;
                    rows.push(Vec::new());
                }
                Some(')') => {
                    self.i += 1;
                    break;
                }
                _ => break,
            }
        }
        Node::Matrix(rows)
    }
}

// --- back to text -----------------------------------------------------------------------

/// The linear form of a tree; `parse` reads it back to the same tree.
pub fn to_linear(n: &Node) -> String {
    let mut s = String::new();
    lin(n, &mut s);
    s
}

fn lin(n: &Node, out: &mut String) {
    match n {
        Node::Row(v) => {
            let mut prev: Option<&Node> = None;
            for x in v {
                if let Some(p) = prev {
                    if !matches!(p, Node::Op(_)) && !matches!(x, Node::Op(_)) {
                        out.push(' ');
                    }
                }
                // A row in a row is a group.
                if matches!(x, Node::Row(_)) {
                    group(x, out);
                } else {
                    lin(x, out);
                }
                prev = Some(x);
            }
        }
        Node::Ident(s) | Node::Number(s) => out.push_str(s),
        Node::Op(c) => out.push(*c),
        Node::Sym(s) => {
            for c in s.chars() {
                out.push(if c == '/' { '∕' } else { c });
            }
        }
        Node::Text(s) => {
            out.push('"');
            out.extend(s.chars().filter(|&c| c != '"'));
            out.push('"');
        }
        Node::Frac(a, b) => {
            frac_arg(a, out);
            out.push('/');
            frac_arg(b, out);
        }
        Node::Script { base, sub, sup } => {
            match &**base {
                Node::Ident(_) | Node::Number(_) | Node::Sym(_) | Node::Text(_) | Node::Delim { .. } | Node::Matrix(_) => lin(base, out),
                b if b.is_empty() => {}
                b => group(b, out),
            }
            if let Some(s) = sub {
                out.push('_');
                script_arg(s, out);
            }
            if let Some(s) = sup {
                out.push('^');
                script_arg(s, out);
            }
        }
        Node::Radical { index, body } => {
            out.push('√');
            match index {
                Some(i) => {
                    out.push('(');
                    lin(i, out);
                    out.push('&');
                    lin(body, out);
                    out.push(')');
                }
                None => match &**body {
                    Node::Ident(_) | Node::Number(_) => lin(body, out),
                    b => {
                        out.push('(');
                        lin(b, out);
                        out.push(')');
                    }
                },
            }
        }
        Node::Nary { op, sub, sup, body } => {
            out.push(*op);
            if let Some(s) = sub {
                out.push('_');
                script_arg(s, out);
            }
            if let Some(s) = sup {
                out.push('^');
                script_arg(s, out);
            }
            if !body.is_empty() {
                out.push('▒');
                body_arg(body, out);
            }
        }
        Node::Delim { open, close, body } => {
            out.push(open.unwrap_or('├'));
            lin(body, out);
            out.push(close.unwrap_or('┤'));
        }
        Node::Func { name, body } => {
            match &**name {
                Node::Text(s) => out.push_str(s),
                Node::Script { base, sub, sup } if matches!(&**base, Node::Text(_)) => {
                    if let Node::Text(s) = &**base {
                        out.push_str(s);
                    }
                    if let Some(s) = sub {
                        out.push('_');
                        script_arg(s, out);
                    }
                    if let Some(s) = sup {
                        out.push('^');
                        script_arg(s, out);
                    }
                }
                other => group(other, out),
            }
            if !body.is_empty() {
                out.push(' ');
                body_arg(body, out);
            }
        }
        Node::Matrix(rows) => {
            out.push_str("■(");
            for (i, r) in rows.iter().enumerate() {
                if i > 0 {
                    out.push('@');
                }
                for (j, c) in r.iter().enumerate() {
                    if j > 0 {
                        out.push('&');
                    }
                    lin(c, out);
                }
            }
            out.push(')');
        }
    }
}

fn group(n: &Node, out: &mut String) {
    out.push('〖');
    lin(n, out);
    out.push('〗');
}

/// Arguments that stand alone without parentheses.
fn is_atom(n: &Node) -> bool {
    match n {
        Node::Ident(_) | Node::Number(_) | Node::Text(_) | Node::Script { .. } | Node::Matrix(_) => true,
        Node::Sym(s) => s.chars().all(|c| !is_opener(c) && !is_closer(c) && !"_^&@\"▒■√∛∜〖〗\\".contains(c) && !is_nary(c)),
        Node::Delim { open, .. } => *open != Some('('),
        _ => false,
    }
}

fn frac_arg(n: &Node, out: &mut String) {
    if is_atom(n) {
        lin(n, out);
    } else {
        out.push('(');
        lin(n, out);
        out.push(')');
    }
}

fn script_arg(n: &Node, out: &mut String) {
    let simple = match n {
        Node::Ident(s) => s.chars().all(char::is_alphabetic),
        Node::Number(s) => s.chars().all(|c| c.is_ascii_digit()),
        Node::Sym(s) => s.chars().count() == 1 && is_atom(n) && !s.starts_with(|c: char| c.is_alphanumeric() || c == ' '),
        _ => false,
    };
    if simple {
        lin(n, out);
    } else {
        out.push('(');
        lin(n, out);
        out.push(')');
    }
}

/// The operand of a big operator or function.
fn body_arg(n: &Node, out: &mut String) {
    match n {
        Node::Ident(_) | Node::Number(_) | Node::Delim { .. } | Node::Text(_) | Node::Matrix(_) => lin(n, out),
        Node::Script { base, .. } if !matches!(&**base, Node::Row(_) | Node::Func { .. }) => lin(n, out),
        _ => group(n, out),
    }
}

// --- LaTeX ---------------------------------------------------------------------------------------

/// The tree written as LaTeX.
pub fn to_latex(n: &Node) -> String {
    let mut s = String::new();
    tex(n, &mut s);
    s.trim().to_string()
}

fn tex_char(c: char, out: &mut String) {
    let c = shown_char(c);
    match c {
        '−' => out.push('-'),
        '{' | '}' | '_' | '^' | '&' | '#' | '%' | '$' => {
            out.push('\\');
            out.push(c);
        }
        '∗' => out.push('*'),
        '′' => out.push('\''),
        // Never a command of the equation's own making (\input, \catcode).
        '\\' => out.push_str("\\backslash "),
        '~' => out.push_str("\\sim "),
        _ => match escape_of(c) {
            Some(n) => {
                out.push('\\');
                out.push_str(n);
                out.push(' ');
            }
            None => out.push(c),
        },
    }
}

fn tex_group(n: &Node, out: &mut String) {
    out.push('{');
    tex(n, out);
    let t = out.trim_end().len();
    out.truncate(t);
    out.push('}');
}

fn tex(n: &Node, out: &mut String) {
    match n {
        Node::Row(v) => {
            for x in v {
                tex(x, out);
            }
        }
        Node::Ident(s) | Node::Number(s) | Node::Sym(s) => {
            for c in s.chars() {
                tex_char(c, out);
            }
        }
        Node::Op(c) => tex_char(*c, out),
        Node::Text(s) => {
            out.push_str("\\text{");
            for c in s.chars() {
                match c {
                    '{' | '}' => {}
                    '$' | '%' | '#' | '&' | '_' => {
                        out.push('\\');
                        out.push(c);
                    }
                    '<' => out.push_str("\\textless{}"),
                    '>' => out.push_str("\\textgreater{}"),
                    '\\' => out.push_str("\\textbackslash{}"),
                    '^' => out.push_str("\\textasciicircum{}"),
                    '~' => out.push_str("\\textasciitilde{}"),
                    c => out.push(c),
                }
            }
            out.push('}');
        }
        Node::Frac(a, b) => {
            out.push_str("\\frac");
            tex_group(a, out);
            tex_group(b, out);
        }
        Node::Script { base, sub, sup } => {
            match &**base {
                Node::Ident(s) | Node::Number(s) if s.chars().count() == 1 => tex(base, out),
                b => tex_group(b, out),
            }
            if let Some(s) = sub {
                out.push('_');
                tex_group(s, out);
            }
            if let Some(s) = sup {
                out.push('^');
                tex_group(s, out);
            }
        }
        Node::Radical { index, body } => {
            out.push_str("\\sqrt");
            if let Some(i) = index {
                out.push('[');
                tex(i, out);
                out.push(']');
            }
            tex_group(body, out);
        }
        Node::Nary { op, sub, sup, body } => {
            match escape_of(*op) {
                Some(n) => {
                    out.push('\\');
                    out.push_str(n);
                }
                None => out.push(*op),
            }
            if let Some(s) = sub {
                out.push('_');
                tex_group(s, out);
            }
            if let Some(s) = sup {
                out.push('^');
                tex_group(s, out);
            }
            out.push(' ');
            tex(body, out);
        }
        Node::Delim { open, close, body } => {
            let side = |c: Option<char>, out: &mut String| match c {
                None => out.push('.'),
                Some('{') => out.push_str("\\{"),
                Some('}') => out.push_str("\\}"),
                Some('‖') => out.push_str("\\|"),
                Some(c) => match escape_of(c) {
                    Some(n) => {
                        out.push('\\');
                        out.push_str(n);
                        out.push(' ');
                    }
                    None => out.push(c),
                },
            };
            out.push_str("\\left");
            side(*open, out);
            tex(body, out);
            out.push_str("\\right");
            side(*close, out);
        }
        Node::Func { name, body } => {
            let (base, sub, sup) = match &**name {
                Node::Script { base, sub, sup } => (&**base, sub.as_deref(), sup.as_deref()),
                n => (n, None, None),
            };
            match base {
                Node::Text(s) if FUNCTIONS.contains(&s.as_str()) => {
                    out.push('\\');
                    out.push_str(s);
                }
                Node::Text(s) => {
                    out.push_str("\\operatorname{");
                    out.push_str(s);
                    out.push('}');
                }
                b => tex_group(b, out),
            }
            if let Some(s) = sub {
                out.push('_');
                tex_group(s, out);
            }
            if let Some(s) = sup {
                out.push('^');
                tex_group(s, out);
            }
            out.push(' ');
            tex(body, out);
        }
        Node::Matrix(rows) => {
            out.push_str("\\begin{matrix}");
            for (i, r) in rows.iter().enumerate() {
                if i > 0 {
                    out.push_str("\\\\");
                }
                for (j, c) in r.iter().enumerate() {
                    if j > 0 {
                        out.push('&');
                    }
                    tex(c, out);
                }
            }
            out.push_str("\\end{matrix}");
        }
    }
    // A letter after a command name would join it.
    if out.ends_with(|c: char| c.is_ascii_alphabetic()) {
        let cmd = out.rfind('\\').is_some_and(|i| out[i + 1..].chars().all(|c| c.is_ascii_alphabetic()));
        if cmd {
            out.push(' ');
        }
    }
}

/// Converts LaTeX (as written in Markdown between dollars) to the linear
/// format. Commands the linear format knows pass through as escapes.
pub fn latex_to_linear(src: &str) -> String {
    let c: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    latex_seq(&c, &mut i, &mut out, None, 0);
    out
}

/// The content of a `{...}` group starting at `c[*i] == '{'`.
fn latex_brace(c: &[char], i: &mut usize, depth: u32) -> String {
    let mut s = String::new();
    if c.get(*i) == Some(&'{') {
        *i += 1;
        latex_seq(c, i, &mut s, Some('}'), depth + 1);
        if c.get(*i) == Some(&'}') {
            *i += 1;
        }
    } else if let Some(&ch) = c.get(*i) {
        // A single token argument.
        if ch == '\\' {
            let mut j = *i + 1;
            while j < c.len() && c[j].is_ascii_alphabetic() {
                j += 1;
            }
            let j = if j == *i + 1 { (j + 1).min(c.len()) } else { j };
            let mut t = String::new();
            let mut k = *i;
            latex_seq(&c[..j], &mut k, &mut t, None, depth + 1);
            *i = j;
            return t;
        }
        s.push(ch);
        *i += 1;
    }
    s
}

fn latex_seq(c: &[char], i: &mut usize, out: &mut String, end: Option<char>, depth: u32) {
    if depth > MAX_DEPTH {
        *i = c.len();
        return;
    }
    while *i < c.len() {
        let ch = c[*i];
        if Some(ch) == end {
            return;
        }
        match ch {
            '{' => {
                let g = latex_brace(c, i, depth);
                out.push('〖');
                out.push_str(&g);
                out.push('〗');
            }
            '_' | '^' => {
                out.push(ch);
                *i += 1;
                while c.get(*i) == Some(&' ') {
                    *i += 1;
                }
                let g = latex_brace(c, i, depth);
                out.push('(');
                out.push_str(&g);
                out.push(')');
            }
            '\\' => {
                let mut j = *i + 1;
                while j < c.len() && c[j].is_ascii_alphabetic() {
                    j += 1;
                }
                let name: String = c[*i + 1..j].iter().collect();
                if name.is_empty() {
                    // `\{`, `\\`, `\,`...
                    match c.get(*i + 1) {
                        Some('\\') => out.push('@'),
                        Some(&n) => {
                            out.push('\\');
                            out.push(n);
                        }
                        None => {}
                    }
                    *i = (*i + 2).min(c.len());
                    continue;
                }
                *i = j;
                let skip_ws = |i: &mut usize| {
                    while c.get(*i) == Some(&' ') {
                        *i += 1;
                    }
                };
                match name.as_str() {
                    "frac" | "dfrac" | "tfrac" => {
                        skip_ws(i);
                        let a = latex_brace(c, i, depth);
                        skip_ws(i);
                        let b = latex_brace(c, i, depth);
                        out.push_str(&format!("({a})/({b})"));
                    }
                    "sqrt" => {
                        skip_ws(i);
                        let mut index = None;
                        if c.get(*i) == Some(&'[') {
                            let close = c[*i..].iter().position(|&x| x == ']').map(|p| *i + p);
                            if let Some(e) = close {
                                let mut t = String::new();
                                let mut k = *i + 1;
                                latex_seq(&c[..e], &mut k, &mut t, None, depth + 1);
                                index = Some(t);
                                *i = e + 1;
                            }
                        }
                        skip_ws(i);
                        let b = latex_brace(c, i, depth);
                        match index {
                            Some(n) => out.push_str(&format!("√({n}&{b})")),
                            None => out.push_str(&format!("√({b})")),
                        }
                    }
                    "left" | "right" | "bigl" | "bigr" | "Bigl" | "Bigr" | "big" | "Big" => {
                        skip_ws(i);
                        let left = name.ends_with('l') || name == "left";
                        match c.get(*i) {
                            Some('.') => {
                                out.push(if left { '├' } else { '┤' });
                                *i += 1;
                            }
                            Some('\\') => {
                                // `\{`, `\|`, `\langle`: handled next round.
                            }
                            Some(&d) => {
                                out.push(d);
                                *i += 1;
                            }
                            None => {}
                        }
                    }
                    "text" | "mathrm" | "textrm" | "mbox" | "operatorname" | "mathit" | "mathbf" => {
                        skip_ws(i);
                        let t = latex_brace(c, i, depth);
                        let plain: String = t.chars().filter(|&c| c != '〖' && c != '〗').collect();
                        if name == "mathit" || name == "mathbf" {
                            out.push_str(&plain);
                        } else if FUNCTIONS.contains(&plain.as_str()) {
                            out.push_str(&plain);
                            out.push(' ');
                        } else {
                            out.push('"');
                            out.push_str(&plain.replace('"', ""));
                            out.push('"');
                        }
                    }
                    "begin" | "end" => {
                        skip_ws(i);
                        let env = latex_brace(c, i, depth);
                        let begin = name == "begin";
                        let (o, cl) = match env.as_str() {
                            "pmatrix" => ("(", ")"),
                            "bmatrix" => ("[", "]"),
                            "vmatrix" => ("|", "|"),
                            "Bmatrix" => ("{", "}"),
                            _ => ("", ""),
                        };
                        if begin {
                            out.push_str(o);
                            out.push_str("■(");
                        } else {
                            out.push(')');
                            out.push_str(cl);
                        }
                    }
                    _ if ESCAPES.iter().any(|(n, _)| *n == name) || FUNCTIONS.contains(&name.as_str()) => {
                        out.push('\\');
                        out.push_str(&name);
                        // The linear format eats one space after a name;
                        // another keeps `\pi r` two factors.
                        let func = FUNCTIONS.contains(&name.as_str());
                        out.push_str(match (matches!(c.get(*i), Some('_' | '^')), func) {
                            (true, true) => "",
                            (true, false) | (false, true) => " ",
                            (false, false) => "  ",
                        });
                    }
                    _ => {
                        out.push('\\');
                        out.push_str(&name);
                    }
                }
            }
            '&' => {
                out.push('&');
                *i += 1;
            }
            _ => {
                out.push(ch);
                *i += 1;
            }
        }
    }
}

// --- layout --------------------------------------------------------------------------------------

/// Text measurement for the layout: advance width of `text` at `size`.
pub trait Measure {
    fn width(&mut self, text: &str, size: f32, italic: bool) -> f32;
}

/// A rough measure (average glyph widths), for exports that need sizes.
pub struct Approx;

impl Measure for Approx {
    fn width(&mut self, text: &str, size: f32, _italic: bool) -> f32 {
        text.chars().map(|c| if c == ' ' { 0.25 } else if c.is_ascii_digit() || c.is_lowercase() { 0.5 } else { 0.62 }).sum::<f32>() * size
    }
}

/// Something to draw, relative to the equation's origin on its baseline
/// (y grows downwards).
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Text { x: f32, y: f32, text: String, size: f32, italic: bool },
    /// A polyline `width` thick.
    Line { points: Vec<(f32, f32)>, width: f32 },
}

/// A laid out box: extent around its baseline and what to draw.
#[derive(Clone, Debug, Default)]
pub struct MathBox {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub items: Vec<Paint>,
}

impl MathBox {
    fn place(&mut self, b: MathBox, dx: f32, dy: f32) {
        self.ascent = self.ascent.max(b.ascent - dy);
        self.descent = self.descent.max(b.descent + dy);
        self.width = self.width.max(dx + b.width);
        for it in b.items {
            self.items.push(match it {
                Paint::Text { x, y, text, size, italic } => Paint::Text { x: x + dx, y: y + dy, text, size, italic },
                Paint::Line { points, width } => Paint::Line { points: points.into_iter().map(|(x, y)| (x + dx, y + dy)).collect(), width },
            });
        }
    }

    fn line(&mut self, points: Vec<(f32, f32)>, width: f32) {
        self.items.push(Paint::Line { points, width });
    }
}

/// Math style: 0 display, 1 text, 2 script, 3 script-script.
#[derive(Clone, Copy)]
struct Style(u8);

impl Style {
    fn scale(self) -> f32 {
        [1.0, 1.0, 0.71, 0.5][self.0.min(3) as usize]
    }

    fn script(self) -> Style {
        Style(if self.0 <= 1 { 2 } else { 3 })
    }

    fn frac(self) -> Style {
        Style((self.0 + 1).min(3))
    }
}

/// Height of the math axis (where fraction bars and minus signs sit) above
/// the baseline, per unit of font size.
const AXIS: f32 = 0.25;

/// Extent of a pair of brackets.
#[derive(Clone, Copy)]
struct Geom {
    small: bool,
    top: f32,
    bot: f32,
    s: f32,
}

struct Layout<'a> {
    m: &'a mut dyn Measure,
    base: f32,
}

/// Lays out an equation at font `size`.
pub fn layout(root: &Node, size: f32, display: bool, m: &mut dyn Measure) -> MathBox {
    let mut l = Layout { m, base: size };
    let mut b = l.node(root, Style(if display { 0 } else { 1 }), 0);
    // Room for an empty equation's caret, and a little air on each side.
    if b.width == 0.0 {
        b.ascent = size * 0.7;
        b.width = size * 0.3;
    }
    let pad = size * 0.04;
    let mut out = MathBox { width: 0.0, ascent: size * 0.7, descent: size * 0.2, items: Vec::new() };
    out.place(b, pad, 0.0);
    out.width += pad;
    out
}

fn text_extent(t: &str, s: f32) -> (f32, f32) {
    let low = t.chars().all(|c| "acemnorsuvwxz+-−=×·÷<>≤≥:~∼≈≠±∓∗∘∙ ,.αεικνοπστυω".contains(c));
    let deep = t.chars().any(|c| "gjpqy,;()[]{}|/βγζημξρςφϕχψ∫Q@µ".contains(c));
    (if low { 0.52 } else { 0.74 } * s, if deep { 0.24 } else { 0.02 } * s)
}

/// Samples a smooth curve through `pts` (Catmull-Rom).
fn smooth(pts: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut out = Vec::with_capacity(pts.len() * 8);
    for i in 0..pts.len() - 1 {
        let p0 = pts[i.saturating_sub(1)];
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        for k in 0..8 {
            let t = k as f32 / 8.0;
            let (t2, t3) = (t * t, t * t * t);
            let f = |a: f32, b: f32, c: f32, d: f32| 0.5 * (2.0 * b + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3);
            out.push((f(p0.0, p1.0, p2.0, p3.0), f(p0.1, p1.1, p2.1, p3.1)));
        }
    }
    out.push(pts[pts.len() - 1]);
    out
}

impl Layout<'_> {
    fn text(&mut self, t: &str, s: f32, italic: bool) -> MathBox {
        let w = self.m.width(t, s, italic);
        let (a, d) = text_extent(t, s);
        MathBox { width: w, ascent: a, descent: d, items: vec![Paint::Text { x: 0.0, y: 0.0, text: t.to_string(), size: s, italic }] }
    }

    fn node(&mut self, n: &Node, st: Style, depth: u32) -> MathBox {
        let s = self.base * st.scale();
        if depth > MAX_DEPTH * 2 {
            return MathBox::default();
        }
        let d = depth + 1;
        match n {
            Node::Row(v) => self.row(v, st, d),
            Node::Ident(t) => {
                let italic = t.chars().all(|c| c.is_alphabetic() && !('Α'..='Ω').contains(&c));
                self.text(t, s, italic)
            }
            Node::Number(t) | Node::Text(t) => self.text(t, s, false),
            Node::Op(c) => self.text(&shown_char(*c).to_string(), s, false),
            Node::Sym(t) => {
                let t: String = t.chars().map(shown_char).collect();
                self.text(&t, s, false)
            }
            Node::Frac(a, b) => self.frac(a, b, st, d),
            Node::Script { base, sub, sup } => {
                let b = self.node(base, st, d);
                let italic = matches!(&**base, Node::Ident(_));
                self.scripts(b, sub.as_deref(), sup.as_deref(), st, italic, d)
            }
            Node::Radical { index, body } => self.radical(index.as_deref(), body, st, d),
            Node::Nary { op, sub, sup, body } => self.nary(*op, sub.as_deref(), sup.as_deref(), body, st, d),
            Node::Delim { open, close, body } => {
                let b = self.node(body, st, d);
                self.delim(*open, *close, b, s)
            }
            Node::Func { name, body } => self.func(name, body, st, d),
            Node::Matrix(rows) => self.matrix(rows, st, d),
        }
    }

    fn row(&mut self, v: &[Node], st: Style, d: u32) -> MathBox {
        let s = self.base * st.scale();
        let mut out = MathBox::default();
        let mut x = 0.0;
        let spaced = st.0 <= 1;
        for (i, n) in v.iter().enumerate() {
            let (before, after) = match n {
                Node::Op(c) => {
                    let unary = i == 0 || matches!(v[i - 1], Node::Op(_));
                    match op_class(*c) {
                        Some(OpClass::Rel) if spaced => (0.28, 0.28),
                        Some(OpClass::Bin) if spaced && !unary => (0.22, 0.22),
                        Some(OpClass::Punct) => (0.0, if spaced { 0.17 } else { 0.08 }),
                        _ => (0.0, 0.0),
                    }
                }
                _ => (0.0, 0.0),
            };
            x += before * s;
            let b = self.node(n, st, d);
            let w = b.width;
            out.place(b, x, 0.0);
            x += w + after * s;
        }
        out.width = x;
        out
    }

    fn frac(&mut self, a: &Node, b: &Node, st: Style, d: u32) -> MathBox {
        let s = self.base * st.scale();
        let inner = st.frac();
        let num = self.node(a, inner, d);
        let den = self.node(b, inner, d);
        let t = (0.055 * s).max(0.5);
        let gap = if st.0 == 0 { 0.16 } else { 0.1 } * s;
        let pad = 0.12 * s;
        let w = num.width.max(den.width) + 2.0 * pad;
        let axis = AXIS * s;
        let mut out = MathBox::default();
        let ny = -axis - t / 2.0 - gap - num.descent;
        let dy = -axis + t / 2.0 + gap + den.ascent;
        let (nw, dw) = (num.width, den.width);
        out.place(num, (w - nw) / 2.0, ny);
        out.place(den, (w - dw) / 2.0, dy);
        out.line(vec![(pad * 0.5, -axis), (w - pad * 0.5, -axis)], t);
        out.width = w;
        out
    }

    fn scripts(&mut self, b: MathBox, sub: Option<&Node>, sup: Option<&Node>, st: Style, italic: bool, d: u32) -> MathBox {
        let s = self.base * st.scale();
        let ss = st.script();
        let sub_b = sub.map(|n| self.node(n, ss, d));
        let sup_b = sup.map(|n| self.node(n, ss, d));
        let x = b.width;
        let mut up = (b.ascent - 0.25 * s).max(0.36 * s);
        let mut down = (b.descent + 0.05 * s).max(0.2 * s);
        if let (Some(p), Some(q)) = (&sup_b, &sub_b) {
            let gap = (down - q.ascent) - (-up + p.descent);
            if gap < 0.12 * s {
                let fix = (0.12 * s - gap) / 2.0;
                up += fix;
                down += fix;
            }
        }
        let mut out = MathBox::default();
        out.place(b, 0.0, 0.0);
        let mut w = x;
        if let Some(p) = sup_b {
            let dx = x + if italic { 0.06 * s } else { 0.02 * s };
            w = w.max(dx + p.width);
            out.place(p, dx, -up);
        }
        if let Some(q) = sub_b {
            let dx = x + 0.01 * s;
            w = w.max(dx + q.width);
            out.place(q, dx, down);
        }
        out.width = w + 0.04 * s;
        out
    }

    /// Limits centered above and below a box.
    fn limits(&mut self, b: MathBox, sub: Option<&Node>, sup: Option<&Node>, st: Style, d: u32) -> MathBox {
        let s = self.base * st.scale();
        let ss = st.script();
        let sub_b = sub.map(|n| self.node(n, ss, d));
        let sup_b = sup.map(|n| self.node(n, ss, d));
        let w = b.width.max(sub_b.as_ref().map_or(0.0, |x| x.width)).max(sup_b.as_ref().map_or(0.0, |x| x.width));
        let gap = 0.1 * s;
        let mut out = MathBox::default();
        let (ba, bd, bw) = (b.ascent, b.descent, b.width);
        out.place(b, (w - bw) / 2.0, 0.0);
        if let Some(p) = sup_b {
            let y = -ba - gap - p.descent;
            let pw = p.width;
            out.place(p, (w - pw) / 2.0, y);
        }
        if let Some(q) = sub_b {
            let y = bd + gap * 1.6 + q.ascent;
            let qw = q.width;
            out.place(q, (w - qw) / 2.0, y);
        }
        out.width = w;
        out
    }

    fn radical(&mut self, index: Option<&Node>, body: &Node, st: Style, d: u32) -> MathBox {
        let s = self.base * st.scale();
        let b = self.node(body, st, d);
        let t = (0.055 * s).max(0.5);
        let gap = 0.12 * s;
        let top = -(b.ascent.max(0.6 * s) + gap + t / 2.0);
        let bottom = b.descent.max(0.1 * s) + 0.04 * s;
        let h = bottom - top;
        let sign_w = 0.45 * s + 0.08 * h;
        let idx = index.map(|n| self.node(n, Style(3), d));
        // The index sits over the short stroke; the sign moves right if needed.
        let tick_x = sign_w * 0.4;
        let shift = idx.as_ref().map_or(0.0, |i| (i.width + 0.05 * s - tick_x).max(0.0));
        let mut out = MathBox::default();
        let x0 = shift;
        let pts = vec![
            (x0, bottom - h * 0.42),
            (x0 + sign_w * 0.18, bottom - h * 0.5),
            (x0 + sign_w * 0.45, bottom),
            (x0 + sign_w, top),
            (x0 + sign_w + b.width + 0.12 * s, top),
        ];
        let bw = b.width;
        out.place(b, x0 + sign_w + 0.06 * s, 0.0);
        out.line(pts, t);
        if let Some(i) = idx {
            let y = bottom - h * 0.55 - i.descent;
            out.place(i, 0.0, y);
        }
        out.ascent = out.ascent.max(-top + t);
        out.descent = out.descent.max(bottom);
        out.width = x0 + sign_w + bw + 0.18 * s;
        out
    }

    /// Draws a big operator `h` tall, centered on the axis.
    fn big_op(&mut self, op: char, h: f32, s: f32) -> MathBox {
        let axis = AXIS * s;
        let (top, bot) = (-axis - h / 2.0, -axis + h / 2.0);
        let lw = 0.065 * s;
        let mut b = MathBox { width: 0.0, ascent: -top, descent: bot, items: Vec::new() };
        match op {
            '∑' => {
                let w = h * 0.72;
                b.line(vec![(w, top + 0.14 * h), (w * 0.96, top), (0.0, top), (w * 0.52, (top + bot) / 2.0), (0.0, bot), (w * 0.96, bot), (w, bot - 0.14 * h)], lw);
                b.width = w;
            }
            '∏' | '∐' => {
                let w = h * 0.78;
                let (t, u) = if op == '∏' { (top, bot) } else { (bot, top) };
                b.line(vec![(0.0, t), (w, t)], lw * 1.2);
                b.line(vec![(0.16 * w, t), (0.16 * w, u)], lw * 1.3);
                b.line(vec![(0.84 * w, t), (0.84 * w, u)], lw * 1.3);
                b.line(vec![(0.0, u), (0.32 * w, u)], lw);
                b.line(vec![(0.68 * w, u), (w, u)], lw);
                b.width = w;
            }
            c if is_integral(c) => {
                let w = h * 0.42;
                let n = match c {
                    '∬' | '∯' => 2,
                    '∭' | '∰' => 3,
                    _ => 1,
                };
                for k in 0..n {
                    let x = k as f32 * w * 0.55;
                    let pts = [(x + w, top + 0.07 * h), (x + w * 0.82, top), (x + w * 0.62, top + 0.1 * h), (x + w * 0.38, bot - 0.1 * h), (x + w * 0.18, bot), (x, bot - 0.07 * h)];
                    b.line(smooth(&pts), lw * 1.15);
                }
                if matches!(c, '∮' | '∯' | '∰') {
                    let (cx, cy, r) = (w * 0.5 + (n - 1) as f32 * w * 0.275, (top + bot) / 2.0, w * 0.2 + (n - 1) as f32 * w * 0.12);
                    let pts: Vec<(f32, f32)> = (0..=24).map(|i| {
                        let a = i as f32 / 24.0 * std::f32::consts::TAU;
                        (cx + r * a.cos(), cy + r * 0.9 * a.sin())
                    }).collect();
                    b.line(pts, lw * 0.8);
                }
                b.width = w + (n - 1) as f32 * w * 0.55;
            }
            c => {
                // Other big operators: the glyph, enlarged.
                let size = h * 1.05;
                let t = c.to_string();
                let w = self.m.width(&t, size, false);
                b.items.push(Paint::Text { x: 0.0, y: bot - 0.2 * size, text: t, size, italic: false });
                b.width = w;
            }
        }
        b
    }

    fn nary(&mut self, op: char, sub: Option<&Node>, sup: Option<&Node>, body: &Node, st: Style, d: u32) -> MathBox {
        let s = self.base * st.scale();
        let integral = is_integral(op);
        let h = match (st.0 == 0, integral) {
            (true, true) => 2.1 * s,
            (true, false) => 1.45 * s,
            (false, true) => 1.4 * s,
            (false, false) => 1.05 * s,
        };
        let opb = self.big_op(op, h, s);
        let mut b = if st.0 == 0 && !integral {
            self.limits(opb, sub, sup, st, d)
        } else if integral {
            // Limits beside the sign, the lower one tucked under its hook.
            let ss = st.script();
            let (w, top, bot) = (opb.width, -opb.ascent, opb.descent);
            let sub_b = sub.map(|n| self.node(n, ss, d));
            let sup_b = sup.map(|n| self.node(n, ss, d));
            let mut out = MathBox::default();
            out.place(opb, 0.0, 0.0);
            let mut ww = w;
            if let Some(p) = sup_b {
                let x = w + 0.02 * s;
                ww = ww.max(x + p.width);
                let y = top + p.ascent * 0.9;
                out.place(p, x, y);
            }
            if let Some(q) = sub_b {
                let x = w - 0.15 * h * 0.42 + 0.02 * s;
                ww = ww.max(x + q.width);
                let y = bot - q.descent * 0.2;
                out.place(q, x, y);
            }
            out.width = ww;
            out
        } else {
            self.scripts(opb, sub, sup, st, false, d)
        };
        if !body.is_empty() {
            let x = b.width + 0.14 * s;
            let bb = self.node(body, st, d);
            let bw = bb.width;
            b.place(bb, x, 0.0);
            b.width = x + bw;
        }
        b
    }

    fn delim(&mut self, open: Option<char>, close: Option<char>, b: MathBox, s: f32) -> MathBox {
        let axis = AXIS * s;
        let small = b.ascent <= 0.8 * s && b.descent <= 0.3 * s;
        let ext = (b.ascent - axis).max(b.descent + axis).max(0.5 * s) + 0.08 * s;
        let (top, bot) = (-axis - ext, -axis + ext);
        let mut out = MathBox::default();
        let mut x = 0.0;
        let gap = 0.05 * s;
        let g = Geom { small, top, bot, s };
        self.delim_side(open, g, &mut out, &mut x, true);
        x += gap;
        let bw = b.width;
        out.place(b, x, 0.0);
        x += bw + gap;
        self.delim_side(close, g, &mut out, &mut x, false);
        out.width = x;
        out
    }

    fn delim_side(&mut self, c: Option<char>, g: Geom, out: &mut MathBox, x: &mut f32, left: bool) {
        let Geom { small, top, bot, s } = g;
        let h = bot - top;
        {
            let Some(c) = c else { return };
            if small || !matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '|' | '‖' | '⟨' | '⟩' | '⌊' | '⌋' | '⌈' | '⌉') {
                let t = self.text(&c.to_string(), s, false);
                let w = t.width;
                let (a, d) = (0.76 * s, 0.24 * s);
                out.place(MathBox { ascent: a, descent: d, ..t }, *x, 0.0);
                *x += w;
                return;
            }
            let bw = (0.22 * s + 0.06 * h).min(0.6 * s);
            let lw = 0.06 * s;
            let mid = (top + bot) / 2.0;
            let pad = 0.06 * s;
            let (x0, x1) = (*x + pad, *x + bw - pad);
            // Shapes for the opening side; closing ones are mirrored.
            let f = |u: f32| if left { x0 + u * (x1 - x0) } else { x1 - u * (x1 - x0) };
            let mut lines: Vec<Vec<(f32, f32)>> = Vec::new();
            let oc = match c {
                ')' => '(',
                ']' => '[',
                '}' => '{',
                '⟩' => '⟨',
                '⌋' => '⌊',
                '⌉' => '⌈',
                c => c,
            };
            match oc {
                '(' => lines.push((0..=24).map(|i| {
                    let t = i as f32 / 24.0;
                    (f(0.95 - 0.9 * (std::f32::consts::PI * t).sin()), top + t * h)
                }).collect()),
                '[' => lines.push(vec![(f(1.0), top), (f(0.15), top), (f(0.15), bot), (f(1.0), bot)]),
                '{' => lines.push(smooth(&[(f(1.0), top), (f(0.55), top + 0.06 * h), (f(0.5), mid - 0.08 * h), (f(0.05), mid), (f(0.5), mid + 0.08 * h), (f(0.55), bot - 0.06 * h), (f(1.0), bot)])),
                '|' => lines.push(vec![(f(0.5), top), (f(0.5), bot)]),
                '‖' => {
                    lines.push(vec![(f(0.25), top), (f(0.25), bot)]);
                    lines.push(vec![(f(0.75), top), (f(0.75), bot)]);
                }
                '⟨' => lines.push(vec![(f(1.0), top), (f(0.05), mid), (f(1.0), bot)]),
                '⌊' => lines.push(vec![(f(0.2), top), (f(0.2), bot), (f(1.0), bot)]),
                '⌈' => lines.push(vec![(f(1.0), top), (f(0.2), top), (f(0.2), bot)]),
                _ => {}
            }
            for l in lines {
                out.line(l, lw);
            }
            out.ascent = out.ascent.max(-top);
            out.descent = out.descent.max(bot);
            *x += bw;
        }
    }

    fn func(&mut self, name: &Node, body: &Node, st: Style, d: u32) -> MathBox {
        let s = self.base * st.scale();
        let mut b = match name {
            Node::Script { base, sub, sup } if st.0 == 0 && matches!(&**base, Node::Text(t) if limit_func(t)) => {
                let nb = self.node(base, st, d);
                self.limits(nb, sub.as_deref(), sup.as_deref(), st, d)
            }
            n => self.node(n, st, d),
        };
        if !body.is_empty() {
            let x = b.width + if matches!(body, Node::Delim { .. }) { 0.04 } else { 0.17 } * s;
            let bb = self.node(body, st, d);
            let bw = bb.width;
            b.place(bb, x, 0.0);
            b.width = x + bw;
        }
        b
    }

    fn matrix(&mut self, rows: &[Vec<Node>], st: Style, d: u32) -> MathBox {
        let s = self.base * st.scale();
        let cst = Style(st.0.max(1));
        let cells: Vec<Vec<MathBox>> = rows.iter().map(|r| r.iter().map(|c| self.node(c, cst, d)).collect()).collect();
        let cols = cells.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0.0f32; cols];
        for r in &cells {
            for (j, c) in r.iter().enumerate() {
                widths[j] = widths[j].max(c.width);
            }
        }
        let heights: Vec<(f32, f32)> = cells.iter().map(|r| r.iter().fold((0.7 * s, 0.2 * s), |(a, d), c| (a.max(c.ascent), d.max(c.descent)))).collect();
        let row_gap = 0.3 * s;
        let col_gap = 0.8 * s;
        let total: f32 = heights.iter().map(|(a, d)| a + d).sum::<f32>() + row_gap * heights.len().saturating_sub(1) as f32;
        let mut y = -AXIS * s - total / 2.0;
        let mut out = MathBox::default();
        for (r, (a, dd)) in cells.into_iter().zip(heights) {
            let base_y = y + a;
            let mut x = 0.0;
            for (j, c) in r.into_iter().enumerate() {
                let cw = c.width;
                out.place(c, x + (widths[j] - cw) / 2.0, base_y);
                x += widths[j] + col_gap;
            }
            y += a + dd + row_gap;
        }
        out.width = widths.iter().sum::<f32>() + col_gap * cols.saturating_sub(1) as f32;
        out.ascent = out.ascent.max(AXIS * s + total / 2.0);
        out.descent = out.descent.max(total / 2.0 - AXIS * s);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Node {
        Node::Ident(s.into())
    }

    fn num(s: &str) -> Node {
        Node::Number(s.into())
    }

    fn bx(n: Node) -> Box<Node> {
        Box::new(n)
    }

    #[test]
    fn fractions_and_scripts() {
        assert_eq!(parse("a/b"), Node::Frac(bx(id("a")), bx(id("b"))));
        assert_eq!(parse("(a+b)/c"), Node::Frac(bx(Node::Row(vec![id("a"), Node::Op('+'), id("b")])), bx(id("c"))));
        assert_eq!(parse("a+b/c"), Node::Row(vec![id("a"), Node::Op('+'), Node::Frac(bx(id("b")), bx(id("c")))]));
        assert_eq!(parse("2x/3"), Node::Frac(bx(Node::Row(vec![num("2"), id("x")])), bx(num("3"))));
        assert_eq!(parse("x_i^2"), Node::Script { base: bx(id("x")), sub: Some(bx(id("i"))), sup: Some(bx(num("2"))) });
        assert_eq!(parse("x^(n+1)"), Node::Script { base: bx(id("x")), sub: None, sup: Some(bx(Node::Row(vec![id("n"), Node::Op('+'), num("1")]))) });
        assert_eq!(parse("e^-x"), Node::Script { base: bx(id("e")), sub: None, sup: Some(bx(Node::Row(vec![Node::Op('-'), id("x")]))) });
    }

    #[test]
    fn structures() {
        assert_eq!(parse("\\sqrt(x+1)"), Node::Radical { index: None, body: bx(Node::Row(vec![id("x"), Node::Op('+'), num("1")])) });
        assert_eq!(parse("√(3&x)"), Node::Radical { index: Some(bx(num("3"))), body: bx(id("x")) });
        assert_eq!(parse("\\alpha \\le \\beta"), Node::Row(vec![id("α"), Node::Op('≤'), id("β")]));
        match parse("\\sum_(i=1)^n i^2") {
            Node::Nary { op: '∑', sub: Some(s), sup: Some(p), body } => {
                assert_eq!(*s, Node::Row(vec![id("i"), Node::Op('='), num("1")]));
                assert_eq!(*p, id("n"));
                assert!(matches!(*body, Node::Script { .. }));
            }
            n => panic!("{n:?}"),
        }
        match parse("\\sin x") {
            Node::Func { name, body } => {
                assert_eq!(*name, Node::Text("sin".into()));
                assert_eq!(*body, id("x"));
            }
            n => panic!("{n:?}"),
        }
        assert!(matches!(parse("lim_(x\\to 0) f(x)"), Node::Func { .. }));
        assert_eq!(parse("[0,1)"), Node::Delim { open: Some('['), close: Some(')'), body: bx(Node::Row(vec![num("0"), Node::Op(','), num("1")])) });
        assert_eq!(parse("|x|"), Node::Delim { open: Some('|'), close: Some('|'), body: bx(id("x")) });
        assert_eq!(parse("\\matrix(a&b@c&d)"), Node::Matrix(vec![vec![id("a"), id("b")], vec![id("c"), id("d")]]));
        assert!(matches!(parse("(\\matrix(1&0@0&1))"), Node::Delim { .. }));
        assert_eq!(parse("\"if \"x"), Node::Row(vec![Node::Text("if ".into()), id("x")]));
    }

    #[test]
    fn linear_round_trip() {
        for s in [
            "x=(-b±√(b^2-4ac))/2a",
            "\\sum_(i=1)^n i^2=(n(n+1)(2n+1))/6",
            "∫_0^∞ e^(-x^2) dx=√π/2",
            "(\\matrix(a&b@c&d))",
            "\\lim_(n\\to ∞) (1+1/n)^n=e",
            "a/b/c",
            "f'(x)",
            "x_(i+1)^(2k)",
            "√(3&x+1)",
            "{a|b}",
            "\\sin^2 x+\\cos^2 x=1",
            "\"text\" x",
            "|x|+‖v‖",
            "((a))/b",
            "a^b^c",
            "x_1_2",
            "(]",
            "∑▒〖a+b〗",
            "1/-x",
            "[0,1)",
            "e^(iπ)+1=0",
            "〖x+1〗^2",
            "log_2 n",
        ] {
            let t = parse(s);
            let l = to_linear(&t);
            assert_eq!(parse(&l), t, "{s} -> {l}");
        }
    }

    #[test]
    fn latex_round_trip() {
        for s in ["x=(-b±√(b^2-4ac))/2a", "\\sum_(i=1)^n i^2", "(\\matrix(a&b@c&d))", "\\lim_(n\\to ∞) (1+1/n)^n", "√(3&x)", "\\sin x+\\alpha", "\"if \"x>0"] {
            let t = parse(s);
            let tex = to_latex(&t);
            let back = parse(&latex_to_linear(&tex));
            assert_eq!(to_linear(&back), to_linear(&t), "{s} -> {tex}");
        }
        assert_eq!(latex_to_linear("\\frac{a}{b}"), "(a)/(b)");
        assert_eq!(parse(&latex_to_linear("\\frac{1}{2}")), Node::Frac(bx(num("1")), bx(num("2"))));
        assert!(matches!(parse(&latex_to_linear("\\begin{pmatrix}1&0\\\\0&1\\end{pmatrix}")), Node::Delim { body, .. } if matches!(*body, Node::Matrix(ref r) if r.len() == 2)));
    }

    #[test]
    fn never_panics() {
        // A simple generator over characters that matter to the parser.
        let alphabet: Vec<char> = "ab1 ()[]{}|‖/_^&@\"√∛∑∫■▒〖〗├┤\\sqrtsum+-=,.xα′⟨⟩⌊⌋\u{E000}".chars().collect();
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..3000 {
            let len = (seed % 24) as usize;
            let mut s = String::new();
            for _ in 0..len {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                s.push(alphabet[(seed % alphabet.len() as u64) as usize]);
            }
            let t = parse(&s);
            let l = to_linear(&t);
            let _ = parse(&l);
            let _ = to_latex(&t);
            let _ = parse(&latex_to_linear(&s));
            let _ = layout(&t, 12.0, seed % 2 == 0, &mut Approx);
        }
        // Deep nesting.
        let deep = "(".repeat(5000) + &"√".repeat(5000) + &"x^".repeat(5000);
        let t = parse(&deep);
        let _ = layout(&t, 12.0, true, &mut Approx);
        let _ = to_linear(&t);
        let _ = latex_to_linear(&"{\\frac{".repeat(3000));
    }

    #[test]
    fn layout_sizes() {
        let m = &mut Approx;
        let x = layout(&parse("x"), 20.0, false, m);
        let f = layout(&parse("a/b"), 20.0, false, m);
        // A fraction is taller than a letter, over and under the baseline.
        assert!(f.ascent > x.ascent && f.descent > x.descent);
        // Display fractions use bigger numerators.
        let fd = layout(&parse("a/b"), 20.0, true, m);
        assert!(fd.ascent > f.ascent);
        // Scripts are smaller and raised.
        let s = layout(&parse("x^2"), 20.0, false, m);
        assert!(s.width < 2.0 * x.width && s.ascent > x.ascent);
        // Brackets grow around tall content: drawn with lines.
        let d = layout(&parse("(a/b)"), 20.0, true, m);
        assert!(d.items.iter().filter(|i| matches!(i, Paint::Line { .. })).count() >= 3);
        let small = layout(&parse("(a)"), 20.0, true, m);
        assert!(small.items.iter().all(|i| matches!(i, Paint::Text { .. })));
        // Display sums put their limits above and below.
        let sd = layout(&parse("\\sum_(i=1)^n i"), 20.0, true, m);
        let si = layout(&parse("\\sum_(i=1)^n i"), 20.0, false, m);
        assert!(sd.ascent > si.ascent && sd.width < si.width + 20.0);
        // The radical covers its body.
        let r = layout(&parse("√x"), 20.0, false, m);
        assert!(r.width > x.width && r.ascent > x.ascent);
        // Matrices stack their rows.
        let mx = layout(&parse("\\matrix(a&b@c&d)"), 20.0, false, m);
        assert!(mx.ascent + mx.descent > 2.0 * (x.ascent + x.descent));
    }

    #[test]
    fn display_paragraphs() {
        let mut d = Document::new();
        let sel = d.insert_equation(Selection::default(), Equation::new("a/b"));
        assert!(is_display(&d.paragraphs[0]));
        assert!(d.equation_at(sel.start()).is_some());
        assert!(d.equation_near(Selection::caret(sel.end())).is_some());
        d.set_equation(sel.start(), Equation::new("c"));
        assert_eq!(d.equation_at(sel.start()).map(|e| e.1.source().to_string()).as_deref(), Some("c"));
        d.undo();
        assert_eq!(d.equation_at(sel.start()).map(|e| e.1.source().to_string()).as_deref(), Some("a/b"));
        // Typing after it is plain text.
        d.insert_text(Selection::caret(sel.end()), "x", None);
        assert!(!is_display(&d.paragraphs[0]));
        assert!(d.paragraphs[0].attrs_at(d.paragraphs[0].text.len()).math.is_none());
        // Copy and paste keep the equation.
        let clip = d.slice(sel.start(), sel.end());
        let end = d.end();
        let p = d.insert_paragraphs(Selection::caret(end), &clip);
        assert_eq!(d.equation_at(d.prev_char(p)).map(|e| e.1.source().to_string()).as_deref(), Some("a/b"));
    }
}
