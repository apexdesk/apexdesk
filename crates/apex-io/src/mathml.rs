//! Equations as MathML: embedded formula objects in odt (with a StarMath
//! annotation, the formula's source text for LibreOffice) and `<math>` in
//! HTML. Also a small element tree used to read MathML and OMML.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use apex_doc::math::{self, Equation, Node};

use crate::xml::{Ev, Reader, Tag, esc, local};

// --- element tree -------------------------------------------------------------------------

/// An element read into memory (names without prefix).
#[derive(Debug, Default)]
pub(crate) struct El {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub kids: Vec<Kid>,
}

#[derive(Debug)]
pub(crate) enum Kid {
    El(El),
    Text(String),
}

/// Deeper elements are skipped.
const MAX_XML_DEPTH: usize = 128;

impl El {
    fn from_tag(t: &Tag) -> El {
        El { name: t.local().to_string(), attrs: t.attrs().map(|(k, v)| (local(k).to_string(), v.into_owned())).collect(), kids: Vec::new() }
    }

    /// Reads the element whose tag `t` was just returned (`start` when it
    /// has content).
    pub fn read(r: &mut Reader, t: &Tag, start: bool) -> El {
        let mut stack = vec![El::from_tag(t)];
        if !start {
            return stack.pop().unwrap_or_default();
        }
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(c) => {
                    if stack.len() >= MAX_XML_DEPTH {
                        r.skip();
                    } else {
                        stack.push(El::from_tag(&c));
                    }
                }
                Ev::Empty(c) => {
                    if let Some(top) = stack.last_mut() {
                        top.kids.push(Kid::El(El::from_tag(&c)));
                    }
                }
                Ev::Text(s) => {
                    if let Some(top) = stack.last_mut() {
                        top.kids.push(Kid::Text(s.into_owned()));
                    }
                }
                Ev::End(_) => {
                    let Some(done) = stack.pop() else { break };
                    match stack.last_mut() {
                        Some(parent) => parent.kids.push(Kid::El(done)),
                        None => return done,
                    }
                }
            }
        }
        // Unclosed at the end of the file: fold what's open.
        while stack.len() > 1 {
            if let Some(done) = stack.pop() {
                if let Some(parent) = stack.last_mut() {
                    parent.kids.push(Kid::El(done));
                }
            }
        }
        stack.pop().unwrap_or_default()
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn els(&self) -> impl Iterator<Item = &El> {
        self.kids.iter().filter_map(|k| match k {
            Kid::El(e) => Some(e),
            Kid::Text(_) => None,
        })
    }

    pub fn child(&self, name: &str) -> Option<&El> {
        self.els().find(|e| e.name == name)
    }

    /// All the text inside.
    pub fn text(&self) -> String {
        let mut s = String::new();
        fn walk(e: &El, s: &mut String) {
            for k in &e.kids {
                match k {
                    Kid::Text(t) => s.push_str(t),
                    Kid::El(c) => walk(c, s),
                }
            }
        }
        walk(self, &mut s);
        s
    }
}

pub(crate) fn row(mut v: Vec<Node>) -> Node {
    if v.len() == 1 { v.pop().unwrap_or(Node::Row(Vec::new())) } else { Node::Row(v) }
}

/// Splits math text into letters, numbers, operators and symbols.
pub(crate) fn tokens(t: &str, upright: bool, out: &mut Vec<Node>) {
    let c: Vec<char> = t.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_alphabetic() {
            let s = i;
            while i < c.len() && c[i].is_alphabetic() {
                i += 1;
            }
            let w: String = c[s..i].iter().collect();
            let func = math::FUNCTIONS.contains(&w.as_str());
            match out.last_mut() {
                // Letters of separate runs join again ("d", "x": dx).
                Some(Node::Ident(prev)) if !upright && !func && !math::FUNCTIONS.contains(&prev.as_str()) => prev.push_str(&w),
                _ => out.push(if upright && !func { Node::Text(w) } else { Node::Ident(w) }),
            }
            continue;
        }
        if ch.is_ascii_digit() {
            let s = i;
            while i < c.len() && (c[i].is_ascii_digit() || (c[i] == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit()))) {
                i += 1;
            }
            out.push(Node::Number(c[s..i].iter().collect()));
            continue;
        }
        i += 1;
        match ch {
            ' ' | '\u{2061}' | '\u{2062}' | '\u{2063}' | '\u{200B}' | '\u{A0}' => {}
            '−' => out.push(Node::Op('-')),
            c if math::op_class(c).is_some() => out.push(Node::Op(c)),
            c => out.push(Node::Sym(c.to_string())),
        }
    }
}

// --- reading MathML --------------------------------------------------------------------------------

/// The equation in a `<math>` element.
pub(crate) fn read_math(el: &El) -> Equation {
    let mut v = Vec::new();
    seq(el, &mut v, 0);
    Equation::from_node(&row(v))
}

fn one(el: Option<&El>, depth: u32) -> Node {
    let Some(el) = el else { return Node::Row(Vec::new()) };
    let mut v = Vec::new();
    conv(el, &mut v, depth);
    row(v)
}

fn seq(el: &El, out: &mut Vec<Node>, depth: u32) {
    for k in el.els() {
        conv(k, out, depth);
    }
}

fn nary_of(n: &Node) -> Option<char> {
    match n {
        Node::Sym(s) | Node::Text(s) => {
            let mut it = s.chars();
            let c = it.next()?;
            (it.next().is_none() && matches!(c, '∑' | '∏' | '∐' | '∫' | '∬' | '∭' | '∮' | '∯' | '∰' | '⋃' | '⋂' | '⋁' | '⋀')).then_some(c)
        }
        _ => None,
    }
}

fn scripted(base: Node, sub: Option<Node>, sup: Option<Node>) -> Node {
    let (sub, sup) = (sub.filter(|n| !n.is_empty()).map(Box::new), sup.filter(|n| !n.is_empty()).map(Box::new));
    match nary_of(&base) {
        Some(op) => Node::Nary { op, sub, sup, body: Box::new(Node::Row(Vec::new())) },
        None if sub.is_none() && sup.is_none() => base,
        None => Node::Script { base: Box::new(base), sub, sup },
    }
}

fn conv(el: &El, out: &mut Vec<Node>, depth: u32) {
    if depth > 64 {
        return;
    }
    let d = depth + 1;
    let kid = |i: usize| el.els().nth(i);
    match el.name.as_str() {
        "mi" => {
            let t = el.text().trim().to_string();
            if t.is_empty() {
                return;
            }
            let normal = el.attr("mathvariant") == Some("normal");
            if t.chars().count() > 1 && el.attr("mathvariant") == Some("italic") && t.chars().all(char::is_alphabetic) {
                out.push(Node::Ident(t));
            } else if t.chars().count() > 1 {
                out.push(if math::FUNCTIONS.contains(&t.as_str()) { Node::Ident(t) } else { Node::Text(t) });
            } else if normal && !t.chars().all(char::is_alphabetic) {
                out.push(Node::Sym(t));
            } else if t.chars().all(char::is_alphabetic) {
                // Letters written one per element join again: "ac".
                match out.last_mut() {
                    Some(Node::Ident(prev)) if !math::FUNCTIONS.contains(&prev.as_str()) => prev.push_str(&t),
                    _ => out.push(Node::Ident(t)),
                }
            } else {
                tokens(&t, false, out);
            }
        }
        "mn" => tokens(el.text().trim(), false, out),
        "mo" => {
            let t = el.text();
            let t = t.trim();
            let mut it = t.chars();
            match (it.next(), it.next()) {
                (None, _) => {}
                (Some('−'), None) => out.push(Node::Op('-')),
                (Some(c), None) if math::op_class(c).is_some() => out.push(Node::Op(c)),
                (Some('\u{2061}' | '\u{2062}' | '\u{2063}'), None) => {}
                _ => out.push(Node::Sym(t.to_string())),
            }
        }
        "mtext" | "ms" => {
            let t = el.text();
            // LibreOffice writes quoted operators as text: <mtext>=</mtext>.
            if !t.trim().is_empty() && t.trim().chars().all(|c| math::op_class(c).is_some() || c == '−') {
                tokens(t.trim(), false, out);
            } else if !t.trim().is_empty() {
                out.push(Node::Text(t));
            }
        }
        "mfrac" => out.push(Node::Frac(Box::new(one(kid(0), d)), Box::new(one(kid(1), d)))),
        "msqrt" => {
            let mut v = Vec::new();
            seq(el, &mut v, d);
            out.push(Node::Radical { index: None, body: Box::new(row(v)) });
        }
        "mroot" => out.push(Node::Radical { index: Some(Box::new(one(kid(1), d))), body: Box::new(one(kid(0), d)) }),
        "msub" | "munder" => out.push(scripted(one(kid(0), d), Some(one(kid(1), d)), None)),
        "msup" | "mover" => out.push(scripted(one(kid(0), d), None, Some(one(kid(1), d)))),
        "msubsup" | "munderover" => out.push(scripted(one(kid(0), d), Some(one(kid(1), d)), Some(one(kid(2), d)))),
        "mtable" => {
            let rows: Vec<Vec<Node>> = el.els().filter(|r| r.name == "mtr" || r.name == "mlabeledtr").map(|r| r.els().filter(|c| c.name == "mtd").map(|c| one(Some(c), d)).collect()).collect();
            if !rows.is_empty() {
                out.push(Node::Matrix(rows));
            }
        }
        "mfenced" => {
            let open = el.attr("open").unwrap_or("(").chars().next();
            let close = el.attr("close").unwrap_or(")").chars().next();
            let sep = el.attr("separators").unwrap_or(",").trim().chars().next();
            let mut v = Vec::new();
            for (i, k) in el.els().enumerate() {
                if i > 0 {
                    if let Some(s) = sep {
                        v.push(if math::op_class(s).is_some() { Node::Op(s) } else { Node::Sym(s.to_string()) });
                    }
                }
                conv(k, &mut v, d);
            }
            out.push(Node::Delim { open, close, body: Box::new(row(v)) });
        }
        "semantics" => {
            if let Some(k) = el.els().next() {
                conv(k, out, d);
            }
        }
        "maction" => {
            if let Some(k) = el.els().next() {
                conv(k, out, d);
            }
        }
        "annotation" | "annotation-xml" | "mspace" | "mphantom" | "none" | "mprescripts" => {}
        "mrow" => {
            // Fences: <mo fence="true">(</mo> ... <mo fence="true">)</mo>.
            let kids: Vec<&El> = el.els().collect();
            let fence = |e: &El, left: bool| -> Option<Option<char>> {
                if e.name != "mo" {
                    return None;
                }
                let t = e.text();
                let t = t.trim();
                let c = t.chars().next();
                let form = e.attr("form") == Some(if left { "prefix" } else { "postfix" });
                let bracket = c.is_some_and(|c| if left { "([{⟨⌊⌈|‖".contains(c) } else { ")]}⟩⌋⌉|‖".contains(c) });
                ((e.attr("fence") == Some("true") && (form || bracket)) || (t.is_empty() && form)).then_some(c)
            };
            if let (true, Some(first), Some(last)) = (kids.len() >= 2, kids.first(), kids.last()) {
                if let (Some(open), Some(close)) = (fence(first, true), fence(last, false)) {
                    let mut v = Vec::new();
                    for k in &kids[1..kids.len() - 1] {
                        conv(k, &mut v, d);
                    }
                    out.push(Node::Delim { open, close, body: Box::new(row(v)) });
                    return;
                }
            }
            seq(el, out, d);
        }
        _ => seq(el, out, d),
    }
}

/// Parses a formula object's content.xml.
pub(crate) fn read_document(xml: &str) -> Option<Equation> {
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => return None,
            Ev::Start(t) if t.local() == "math" => return Some(read_math(&El::read(&mut r, &t, true))),
            Ev::Start(t) if t.local() == "document" || t.local() == "body" => continue,
            _ => {}
        }
    }
}

/// Reads a `draw:frame` that holds a formula object. `r` is at the frame
/// start; returns the reader after the frame, and the equation. Frames
/// that aren't formulas return `None` (the caller reads them from `r`).
pub(crate) fn read_frame<'a>(r: &Reader<'a>, start: bool, files: &HashMap<String, Arc<[u8]>>) -> Option<(Reader<'a>, Arc<Equation>)> {
    if !start {
        return None;
    }
    let mut rr = r.clone();
    let mut depth = 1;
    let mut eq = None;
    let object = |t: &Tag| {
        let h = t.attr("xlink:href")?;
        let path = format!("{}/content.xml", h.trim_start_matches("./").trim_end_matches('/'));
        let d = files.get(&path)?;
        read_document(&String::from_utf8_lossy(d))
    };
    loop {
        match rr.next() {
            Ev::Eof => break,
            // Inline MathML (flat odt).
            Ev::Start(t) if t.local() == "math" => {
                let el = El::read(&mut rr, &t, true);
                eq = eq.or_else(|| Some(read_math(&el)));
            }
            Ev::Empty(t) if t.local() == "object" => eq = eq.or_else(|| object(&t)),
            Ev::Start(t) => {
                if t.local() == "object" {
                    eq = eq.or_else(|| object(&t));
                }
                depth += 1;
            }
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    eq.map(|e| (rr, Arc::new(e)))
}

// --- writing MathML ----------------------------------------------------------------------------------

/// MathML for an equation; `annotation` adds the StarMath source.
pub fn to_mathml(eq: &Equation, display: bool, annotation: bool) -> String {
    let mut body = String::new();
    ml(eq.root(), &mut body, display);
    let mut s = String::new();
    let _ = write!(s, r#"<math xmlns="http://www.w3.org/1998/Math/MathML" display="{}">"#, if display { "block" } else { "inline" });
    if annotation {
        let _ = write!(s, "<semantics><mrow>{body}</mrow><annotation encoding=\"StarMath 5.0\">{}</annotation></semantics>", esc(&starmath(eq.root())));
    } else {
        let _ = write!(s, "<mrow>{body}</mrow>");
    }
    s.push_str("</math>");
    s
}

fn ml_arg(n: &Node, out: &mut String, display: bool) {
    out.push_str("<mrow>");
    ml(n, out, display);
    out.push_str("</mrow>");
}

fn mo(c: &str, out: &mut String, stretchy: bool) {
    let _ = write!(out, r#"<mo stretchy="{stretchy}">{}</mo>"#, esc(c));
}

fn ml(n: &Node, out: &mut String, display: bool) {
    match n {
        Node::Row(v) => {
            for x in v {
                if matches!(x, Node::Row(_)) {
                    ml_arg(x, out, display);
                } else {
                    ml(x, out, display);
                }
            }
        }
        Node::Ident(s) => {
            for c in s.chars() {
                if ('Α'..='Ω').contains(&c) {
                    let _ = write!(out, r#"<mi mathvariant="normal">{c}</mi>"#);
                } else {
                    let _ = write!(out, "<mi>{}</mi>", esc(&c.to_string()));
                }
            }
        }
        Node::Number(s) => {
            let _ = write!(out, "<mn>{}</mn>", esc(s));
        }
        Node::Op(c) => {
            let _ = write!(out, "<mo>{}</mo>", esc(&math::shown_char(*c).to_string()));
        }
        Node::Sym(s) => {
            let t: String = s.chars().map(math::shown_char).collect();
            if t.chars().all(|c| "∞∂∇∅ℏℓ".contains(c)) {
                let _ = write!(out, r#"<mi mathvariant="normal">{}</mi>"#, esc(&t));
            } else {
                mo(&t, out, false);
            }
        }
        Node::Text(s) => {
            let _ = write!(out, "<mtext>{}</mtext>", esc(s));
        }
        Node::Frac(a, b) => {
            out.push_str("<mfrac>");
            ml_arg(a, out, false);
            ml_arg(b, out, false);
            out.push_str("</mfrac>");
        }
        Node::Script { base, sub, sup } => {
            let tag = match (sub, sup) {
                (Some(_), Some(_)) => "msubsup",
                (Some(_), None) => "msub",
                _ => "msup",
            };
            let _ = write!(out, "<{tag}>");
            ml_arg(base, out, display);
            for s in [sub, sup].into_iter().flatten() {
                ml_arg(s, out, false);
            }
            let _ = write!(out, "</{tag}>");
        }
        Node::Radical { index, body } => match index {
            Some(i) => {
                out.push_str("<mroot>");
                ml_arg(body, out, display);
                ml_arg(i, out, false);
                out.push_str("</mroot>");
            }
            None => {
                out.push_str("<msqrt>");
                ml(body, out, display);
                out.push_str("</msqrt>");
            }
        },
        Node::Nary { op, sub, sup, body } => {
            out.push_str("<mrow>");
            let under = display && !math::is_integral(*op);
            let tag = match (sub, sup, under) {
                (Some(_), Some(_), true) => "munderover",
                (Some(_), None, true) => "munder",
                (None, Some(_), true) => "mover",
                (Some(_), Some(_), false) => "msubsup",
                (Some(_), None, false) => "msub",
                (None, Some(_), false) => "msup",
                (None, None, _) => "",
            };
            if !tag.is_empty() {
                let _ = write!(out, "<{tag}>");
            }
            mo(&op.to_string(), out, false);
            for s in [sub, sup].into_iter().flatten() {
                ml_arg(s, out, false);
            }
            if !tag.is_empty() {
                let _ = write!(out, "</{tag}>");
            }
            ml_arg(body, out, display);
            out.push_str("</mrow>");
        }
        Node::Delim { open, close, body } => {
            out.push_str("<mrow>");
            let _ = write!(out, r#"<mo fence="true" form="prefix" stretchy="true">{}</mo>"#, open.map(|c| esc(&c.to_string()).into_owned()).unwrap_or_default());
            ml_arg(body, out, display);
            let _ = write!(out, r#"<mo fence="true" form="postfix" stretchy="true">{}</mo>"#, close.map(|c| esc(&c.to_string()).into_owned()).unwrap_or_default());
            out.push_str("</mrow>");
        }
        Node::Func { name, body } => {
            out.push_str("<mrow>");
            let fname = |b: &Node, out: &mut String| match b {
                Node::Text(t) => {
                    let _ = write!(out, "<mi>{}</mi>", esc(t));
                }
                b => ml_arg(b, out, display),
            };
            match &**name {
                Node::Script { base, sub, sup } => {
                    let under = display && matches!(&**base, Node::Text(t) if matches!(t.as_str(), "lim" | "liminf" | "limsup" | "max" | "min" | "sup" | "inf"));
                    let tag = match (sub.is_some(), sup.is_some(), under) {
                        (true, true, true) => "munderover",
                        (true, false, true) => "munder",
                        (false, _, true) => "mover",
                        (true, true, false) => "msubsup",
                        (true, false, false) => "msub",
                        (false, _, false) => "msup",
                    };
                    let _ = write!(out, "<{tag}>");
                    fname(base, out);
                    for s in [sub, sup].into_iter().flatten() {
                        ml_arg(s, out, false);
                    }
                    let _ = write!(out, "</{tag}>");
                }
                n => fname(n, out),
            }
            out.push_str("<mo>\u{2061}</mo>");
            ml_arg(body, out, display);
            out.push_str("</mrow>");
        }
        Node::Matrix(rows) => {
            out.push_str("<mtable>");
            for r in rows {
                out.push_str("<mtr>");
                for c in r {
                    out.push_str("<mtd>");
                    ml(c, out, false);
                    out.push_str("</mtd>");
                }
                out.push_str("</mtr>");
            }
            out.push_str("</mtable>");
        }
    }
}

// --- StarMath (LibreOffice's formula text) ------------------------------------------------------

const SM_GREEK: [&str; 24] = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa", "lambda", "mu", "nu", "xi", "omicron", "pi", "rho", "sigma", "tau", "upsilon", "phi", "chi", "psi", "omega"];

fn sm_char(c: char, out: &mut String) {
    let c = math::shown_char(c);
    // Greek letters by name: lowercase italic (%i...), capitals upright.
    let lower = ('α'..='ω').contains(&c) && c != 'ς';
    let upper = ('Α'..='Ω').contains(&c) && c != '\u{3A2}';
    if lower || upper {
        let base = if lower { 'α' } else { 'Α' } as u32;
        let mut i = c as u32 - base;
        // Skip final sigma's slot.
        if i > 17 {
            i -= 1;
        }
        if let Some(n) = SM_GREEK.get(i as usize) {
            if lower {
                let _ = write!(out, " %i{n} ");
            } else {
                let _ = write!(out, " %{} ", n.to_uppercase());
            }
            return;
        }
    }
    let word = match c {
        '±' => "+-",
        '∓' => "-+",
        '×' => "times",
        '·' => "cdot",
        '÷' => "div",
        '≠' => "<>",
        '≤' => "<=",
        '≥' => ">=",
        '≈' => "approx",
        '≡' => "equiv",
        '∼' => "sim",
        '≃' => "simeq",
        '∝' => "prop",
        '→' => "toward",
        '⇒' => "drarrow",
        '⇐' => "dlarrow",
        '⇔' => "dlrarrow",
        '∈' => "in",
        '∉' => "notin",
        '∋' => "owns",
        '⊂' => "subset",
        '⊃' => "supset",
        '⊆' => "subseteq",
        '⊇' => "supseteq",
        '∪' => "union",
        '∩' => "intersection",
        '∧' => "and",
        '∨' => "or",
        '∞' => "infinity",
        '∂' => "partial",
        '∇' => "nabla",
        '∀' => "forall",
        '∃' => "exists",
        '¬' => "neg",
        '∅' => "emptyset",
        '…' => "dotslow",
        '⋯' => "dotsaxis",
        '⋮' => "dotsvert",
        'ℏ' => "hbar",
        '⊥' => "ortho",
        '∥' => "parallel",
        '∘' => "circ",
        '−' => "-",
        '∗' => "*",
        '(' => "\\(",
        ')' => "\\)",
        '[' => "\\[",
        ']' => "\\]",
        '{' => "\\{",
        '}' => "\\}",
        '|' => "\\lline",
        '⟨' => "\\langle",
        '⟩' => "\\rangle",
        '"' => "",
        '%' => "\"%\"",
        '#' => "\"#\"",
        '&' => "\"&\"",
        '~' => "\"~\"",
        '`' => "\"`\"",
        _ => {
            out.push(c);
            return;
        }
    };
    let _ = write!(out, " {word} ");
}

fn sm_group(n: &Node, out: &mut String) {
    let mut s = String::new();
    sm(n, &mut s);
    out.push('{');
    out.push_str(s.trim());
    out.push('}');
}

/// The StarMath text of a tree.
pub fn starmath(n: &Node) -> String {
    let mut s = String::new();
    sm(n, &mut s);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sm(n: &Node, out: &mut String) {
    match n {
        Node::Row(v) => {
            for x in v {
                if matches!(x, Node::Row(_)) {
                    sm_group(x, out);
                } else {
                    sm(x, out);
                }
                out.push(' ');
            }
        }
        Node::Ident(s) => {
            // Letters apart, so that they don't spell a keyword.
            for c in s.chars() {
                sm_char(c, out);
                out.push(' ');
            }
        }
        Node::Number(s) => out.push_str(s),
        Node::Op(c) => sm_char(*c, out),
        Node::Sym(s) => {
            for c in s.chars() {
                sm_char(c, out);
            }
        }
        Node::Text(s) => {
            let _ = write!(out, " \"{}\" ", s.replace('"', ""));
        }
        Node::Frac(a, b) => {
            sm_group(a, out);
            out.push_str(" over ");
            sm_group(b, out);
        }
        Node::Script { base, sub, sup } => {
            sm_group(base, out);
            if let Some(s) = sub {
                out.push_str("_");
                sm_group(s, out);
            }
            if let Some(s) = sup {
                out.push_str("^");
                sm_group(s, out);
            }
        }
        Node::Radical { index, body } => match index {
            Some(i) => {
                out.push_str(" nroot");
                sm_group(i, out);
                sm_group(body, out);
            }
            None => {
                out.push_str(" sqrt");
                sm_group(body, out);
            }
        },
        Node::Nary { op, sub, sup, body } => {
            let word = match op {
                '∑' => Some("sum"),
                '∏' => Some("prod"),
                '∐' => Some("coprod"),
                '∫' => Some("int"),
                '∬' => Some("iint"),
                '∭' => Some("iiint"),
                '∮' => Some("lint"),
                '∯' => Some("llint"),
                '∰' => Some("lllint"),
                _ => None,
            };
            match word {
                Some(w) => {
                    let _ = write!(out, " {w}");
                    if let Some(s) = sub {
                        out.push_str(" from");
                        sm_group(s, out);
                    }
                    if let Some(s) = sup {
                        out.push_str(" to");
                        sm_group(s, out);
                    }
                }
                None => {
                    let _ = write!(out, " {{{op}}}");
                    if let Some(s) = sub {
                        out.push('_');
                        sm_group(s, out);
                    }
                    if let Some(s) = sup {
                        out.push('^');
                        sm_group(s, out);
                    }
                }
            }
            out.push(' ');
            sm_group(body, out);
        }
        Node::Delim { open, close, body } => {
            let side = |c: Option<char>, left: bool| -> &'static str {
                match c {
                    None => "none",
                    Some('(') => "(",
                    Some(')') => ")",
                    Some('[') => "[",
                    Some(']') => "]",
                    Some('{') => "lbrace",
                    Some('}') => "rbrace",
                    Some('|') => if left { "lline" } else { "rline" },
                    Some('‖') => if left { "ldline" } else { "rdline" },
                    Some('⟨') => "langle",
                    Some('⟩') => "rangle",
                    Some('⌊') => "lfloor",
                    Some('⌋') => "rfloor",
                    Some('⌈') => "lceil",
                    Some('⌉') => "rceil",
                    Some(_) => if left { "(" } else { ")" },
                }
            };
            let _ = write!(out, " left {} ", side(*open, true));
            sm_group(body, out);
            let _ = write!(out, " right {} ", side(*close, false));
        }
        Node::Func { name, body } => {
            let (base, sub, sup) = match &**name {
                Node::Script { base, sub, sup } => (&**base, sub.as_deref(), sup.as_deref()),
                n => (n, None, None),
            };
            const BUILTIN: [&str; 18] = ["sin", "cos", "tan", "cot", "sinh", "cosh", "tanh", "coth", "arcsin", "arccos", "arctan", "ln", "log", "exp", "lim", "liminf", "limsup", "arccot"];
            match base {
                Node::Text(t) if matches!(t.as_str(), "lim" | "liminf" | "limsup") => {
                    let _ = write!(out, " {t}");
                    if let Some(s) = sub {
                        out.push_str(" from");
                        sm_group(s, out);
                    }
                    if let Some(s) = sup {
                        out.push_str(" to");
                        sm_group(s, out);
                    }
                }
                Node::Text(t) => {
                    if BUILTIN.contains(&t.as_str()) {
                        let _ = write!(out, " {{{t}}}");
                    } else {
                        let _ = write!(out, " {{func {t}}}");
                    }
                    if let Some(s) = sub {
                        out.push('_');
                        sm_group(s, out);
                    }
                    if let Some(s) = sup {
                        out.push('^');
                        sm_group(s, out);
                    }
                }
                b => sm_group(b, out),
            }
            out.push(' ');
            sm_group(body, out);
        }
        Node::Matrix(rows) => {
            out.push_str(" matrix{");
            for (i, r) in rows.iter().enumerate() {
                if i > 0 {
                    out.push_str(" ## ");
                }
                for (j, c) in r.iter().enumerate() {
                    if j > 0 {
                        out.push_str(" # ");
                    }
                    sm_group(c, out);
                }
            }
            out.push('}');
        }
    }
}

// --- Markdown ----------------------------------------------------------------------------------------

/// Writes an equation as LaTeX between dollars (doubled for display ones).
pub(crate) fn md_write(out: &mut String, eq: &Equation, display: bool) {
    let d = if display { "$$" } else { "$" };
    out.push_str(d);
    // No `<` that a reader without math takes for HTML.
    out.push_str(&eq.latex().replace('\n', " ").replace('<', "\\lt ").replace('>', "\\gt "));
    out.push_str(d);
}

/// An equation at the start of `s`: `$$...$$`, or `$...$` with pandoc's
/// rules (no space just inside the dollars, no digit right after the
/// closing one, so that prices stay text). Its length and equation.
/// Not one: how far into `s` no other equation of the same kind
/// (`$$` or `$`) can start either, for they would end at the same place.
pub(crate) fn md_math(s: &str) -> Result<(usize, Equation), usize> {
    if let Some(r) = s.strip_prefix("$$") {
        let Some(end) = r.find("$$") else { return Err(s.len()) };
        let body = r[..end].trim();
        return if body.is_empty() { Err(0) } else { Ok((end + 4, Equation::new(&math::latex_to_linear(body)))) };
    }
    let Some(r) = s.strip_prefix('$') else { return Err(0) };
    if r.is_empty() || r.starts_with(char::is_whitespace) {
        return Err(0);
    }
    let mut prev = '\0';
    for (i, c) in r.char_indices() {
        if c == '$' && prev != '\\' {
            if prev.is_whitespace() || r[i + 1..].starts_with(|c: char| c.is_ascii_digit()) {
                return Err(i + 1);
            }
            return Ok((i + 2, Equation::new(&math::latex_to_linear(&r[..i]))));
        }
        prev = c;
    }
    Err(s.len())
}

// --- odt formula objects ----------------------------------------------------------------------------

/// Formula objects written to an odt package.
#[derive(Default)]
pub(crate) struct Formulas {
    objects: Vec<Arc<[u8]>>,
}

impl Formulas {
    fn path(n: usize) -> String {
        format!("Object {n}")
    }

    /// A `draw:frame` with the formula; its sub-document is kept for `files`.
    pub fn frame(&mut self, out: &mut String, eq: &Equation, display: bool, size_pt: f32) {
        let doc = format!("{}{}", crate::xml::HEADER, to_mathml(eq, display, true));
        self.objects.push(Arc::from(doc.into_bytes()));
        let n = self.objects.len();
        let b = math::layout(eq.root(), size_pt, display, &mut math::Approx);
        let _ = write!(
            out,
            r#"<draw:frame draw:style-name="frMath" draw:name="Formula{n}" text:anchor-type="as-char" svg:y="{:.3}pt" svg:width="{:.3}pt" svg:height="{:.3}pt" draw:z-index="0"><draw:object xlink:href="./{}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/></draw:frame>"#,
            -b.ascent,
            b.width,
            b.ascent + b.descent,
            Self::path(n)
        );
    }

    pub fn styles(&self) -> &'static str {
        if self.objects.is_empty() {
            ""
        } else {
            r#"<style:style style:name="frMath" style:family="graphic"><style:graphic-properties style:vertical-pos="from-top" style:vertical-rel="baseline" fo:margin-left="0pt" fo:margin-right="0pt" fo:margin-top="0pt" fo:margin-bottom="0pt" fo:padding="0pt" fo:border="none"/></style:style>"#
        }
    }

    pub fn manifest(&self) -> String {
        let mut s = String::new();
        for i in 1..=self.objects.len() {
            let p = Self::path(i);
            let _ = write!(s, r#"<manifest:file-entry manifest:full-path="{p}/content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="{p}/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.formula"/>"#);
        }
        s
    }

    pub fn files(&self) -> impl Iterator<Item = (String, &Arc<[u8]>)> {
        self.objects.iter().enumerate().map(|(i, d)| (format!("{}/content.xml", Self::path(i + 1)), d))
    }
}

/// The content.xml of every embedded object (formulas among them).
pub(crate) fn load_objects(z: &crate::zip::ZipReader, out: &mut HashMap<String, Arc<[u8]>>) {
    let names: Vec<String> = z.names().filter(|n| n.ends_with("/content.xml") && !n.starts_with("Pictures/") && n.matches('/').count() == 1).map(str::to_string).collect();
    for n in names {
        if let Ok(d) = z.read(&n) {
            out.insert(n, Arc::from(d));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn back(eq: &Equation, display: bool) -> Equation {
        let xml = to_mathml(eq, display, true);
        read_document(&xml).expect("math")
    }

    #[test]
    fn round_trip() {
        for s in [
            "x=(-b±√(b^2-4ac))/2a",
            "\\sum_(i=1)^n i^2=(n(n+1)(2n+1))/6",
            "∫_0^∞ e^(-x^2) dx=√π/2",
            "(\\matrix(a&b@c&d))",
            "\\lim_(n\\to ∞) (1+1/n)^n=e",
            "√(3&x+1)",
            "\\sin^2 x+\\cos^2 x=1",
            "\"if \"x>0",
            "|x|",
            "α+β≤γ",
        ] {
            for display in [false, true] {
                let eq = Equation::new(s);
                let b = back(&eq, display);
                assert_eq!(b.root(), eq.root(), "{s} -> {}", b.source());
            }
        }
    }

    #[test]
    fn odt_round_trip() {
        let d = crate::omml::tests::with_equations();
        let bytes = crate::odt::write(&d);
        let z = crate::zip::ZipReader::new(&bytes).expect("zip");
        let manifest = z.read_string("META-INF/manifest.xml").expect("manifest");
        assert!(manifest.contains(r#"manifest:full-path="Object 1/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.formula""#));
        assert!(manifest.contains(r#"manifest:full-path="Object 2/content.xml""#));
        let back = crate::odt::read(&bytes).expect("odt");
        crate::omml::tests::check_equations(&back);
    }

    #[test]
    fn markdown_round_trip() {
        let d = crate::omml::tests::with_equations();
        let md = crate::markdown::write(&d);
        assert!(md.contains("Area $\\pi r^{2}$") && md.contains("$$x="), "{md}");
        crate::omml::tests::check_equations(&crate::markdown::read(&md));
        // Prices are not equations.
        let p = &crate::markdown::read("costs $5 and $10, or $ x $").paragraphs[0];
        assert!(p.runs.iter().all(|r| r.attrs.math.is_none()), "{p:?}");
        assert!(crate::markdown::read("a \\$ b").paragraphs[0].text == "a $ b");
    }

    #[test]
    fn html_math() {
        let d = crate::omml::tests::with_equations();
        let h = crate::html::doc_html(&d);
        assert!(h.contains(r#"<math xmlns="http://www.w3.org/1998/Math/MathML" display="block">"#) && h.contains("<mfrac>"), "{h}");
    }

    #[test]
    fn starmath_text() {
        assert_eq!(starmath(Equation::new("a/b").root()), "{a} over {b}");
        assert_eq!(starmath(Equation::new("\\sum_(i=1)^n i").root()), "sum from{i = 1} to{n} {i}");
        assert_eq!(starmath(Equation::new("√x≤\\alpha").root()), "sqrt{x} <= %ialpha");
        assert_eq!(starmath(Equation::new("(a)").root()), "left ( {a} right )");
    }

    #[test]
    fn libreoffice_mathml() {
        // As LibreOffice writes "sum from{i=1} to{n} i^2 = left( a over b right)".
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><math xmlns="http://www.w3.org/1998/Math/MathML" display="block"><semantics><mrow><munderover><mo stretchy="false">∑</mo><mrow><mi>i</mi><mo stretchy="false">=</mo><mn>1</mn></mrow><mi>n</mi></munderover><msup><mi>i</mi><mn>2</mn></msup><mo stretchy="false">=</mo><mrow><mo fence="true" form="prefix" stretchy="true">(</mo><mfrac><mi>a</mi><mi>b</mi></mfrac><mo fence="true" form="postfix" stretchy="true">)</mo></mrow></mrow><annotation encoding="StarMath 5.0">sum from{i=1} to{n} i^2 = left( a over b right)</annotation></semantics></math>"#;
        let eq = read_document(xml).expect("math");
        assert_eq!(eq.root(), Equation::new("∑_(i=1)^n i^2=(a/b)").root(), "{}", eq.source());
    }
}
