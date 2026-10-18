//! Equations in docx: Office Math Markup Language (`m:oMath`).

use std::fmt::Write;
use std::sync::Arc;

use apex_doc::math::{self, Equation, Node};
use apex_doc::{Attrs, OBJECT};

use crate::mathml::{El, row, tokens};
use crate::xml::{Reader, Tag, esc};

// --- writing -------------------------------------------------------------------------------------

struct W<'a> {
    out: &'a mut String,
    /// Run properties of every math run (font, size, color).
    rpr: String,
    display: bool,
}

/// Writes the equations of `text` (their `OBJECT` characters), as display
/// math when the paragraph is only the equation.
pub(crate) fn write(out: &mut String, text: &str, a: &Attrs, display: bool) {
    let Some(eq) = &a.math else { return };
    let mut rpr = String::from(r#"<w:rPr><w:rFonts w:ascii="Cambria Math" w:hAnsi="Cambria Math"/>"#);
    if a.bold {
        rpr.push_str("<w:b/>");
    }
    if let Some(c) = a.color {
        let _ = write!(rpr, r#"<w:color w:val="{:06X}"/>"#, c & 0xFF_FFFF);
    }
    if let Some(sz) = a.size {
        let _ = write!(rpr, r#"<w:sz w:val="{sz}"/><w:szCs w:val="{sz}"/>"#);
    }
    rpr.push_str("</w:rPr>");
    for _ in text.matches(OBJECT) {
        write_equation(out, eq, &rpr, display);
    }
}

/// One equation, each run with the run properties `rpr` (`w:rPr` in Word,
/// `a:rPr` in PowerPoint).
pub(crate) fn write_equation(out: &mut String, eq: &Equation, rpr: &str, display: bool) {
    if display {
        out.push_str("<m:oMathPara>");
    }
    out.push_str("<m:oMath>");
    let mut w = W { out, rpr: rpr.to_string(), display };
    w.node(eq.root());
    out.push_str("</m:oMath>");
    if display {
        out.push_str("</m:oMathPara>");
    }
}

impl W<'_> {
    fn run(&mut self, t: &str, plain: bool) {
        if t.is_empty() {
            return;
        }
        self.out.push_str("<m:r>");
        if plain {
            self.out.push_str(r#"<m:rPr><m:sty m:val="p"/></m:rPr>"#);
        }
        self.out.push_str(&self.rpr);
        let _ = write!(self.out, r#"<m:t xml:space="preserve">{}</m:t></m:r>"#, esc(t));
    }

    fn arg(&mut self, tag: &str, n: Option<&Node>) {
        match n {
            Some(n) if !n.is_empty() => {
                let _ = write!(self.out, "<m:{tag}>");
                self.node(n);
                let _ = write!(self.out, "</m:{tag}>");
            }
            _ => {
                let _ = write!(self.out, "<m:{tag}/>");
            }
        }
    }

    fn scripts(&mut self, base: &dyn Fn(&mut Self), sub: Option<&Node>, sup: Option<&Node>) {
        let tag = match (sub, sup) {
            (Some(_), Some(_)) => "sSubSup",
            (Some(_), None) => "sSub",
            (None, Some(_)) => "sSup",
            (None, None) => {
                base(self);
                return;
            }
        };
        let _ = write!(self.out, "<m:{tag}><m:e>");
        base(self);
        self.out.push_str("</m:e>");
        if let Some(s) = sub {
            self.arg("sub", Some(s));
        }
        if let Some(s) = sup {
            self.arg("sup", Some(s));
        }
        let _ = write!(self.out, "</m:{tag}>");
    }

    fn node(&mut self, n: &Node) {
        match n {
            Node::Row(v) => {
                for x in v {
                    self.node(x);
                }
            }
            Node::Ident(s) | Node::Number(s) => self.run(s, false),
            Node::Op(c) => self.run(&math::shown_char(*c).to_string(), false),
            Node::Sym(s) => {
                let t: String = s.chars().map(math::shown_char).collect();
                self.run(&t, false);
            }
            Node::Text(s) => self.run(s, true),
            Node::Frac(a, b) => {
                self.out.push_str("<m:f>");
                self.arg("num", Some(a));
                self.arg("den", Some(b));
                self.out.push_str("</m:f>");
            }
            Node::Script { base, sub, sup } => self.scripts(&|w: &mut Self| w.node(base), sub.as_deref(), sup.as_deref()),
            Node::Radical { index, body } => {
                self.out.push_str("<m:rad>");
                match index {
                    Some(i) => self.arg("deg", Some(i)),
                    None => self.out.push_str(r#"<m:radPr><m:degHide m:val="1"/></m:radPr><m:deg/>"#),
                }
                self.arg("e", Some(body));
                self.out.push_str("</m:rad>");
            }
            Node::Nary { op, sub, sup, body } => {
                let loc = if math::is_integral(*op) { "subSup" } else { "undOvr" };
                let _ = write!(self.out, r#"<m:nary><m:naryPr><m:chr m:val="{}"/><m:limLoc m:val="{loc}"/>"#, esc(&op.to_string()));
                if sub.is_none() {
                    self.out.push_str(r#"<m:subHide m:val="1"/>"#);
                }
                if sup.is_none() {
                    self.out.push_str(r#"<m:supHide m:val="1"/>"#);
                }
                self.out.push_str("</m:naryPr>");
                self.arg("sub", sub.as_deref());
                self.arg("sup", sup.as_deref());
                self.arg("e", Some(body));
                self.out.push_str("</m:nary>");
            }
            Node::Delim { open, close, body } => {
                let chr = |c: Option<char>| c.map(|c| esc(&c.to_string()).into_owned()).unwrap_or_default();
                let _ = write!(self.out, r#"<m:d><m:dPr><m:begChr m:val="{}"/><m:endChr m:val="{}"/></m:dPr>"#, chr(*open), chr(*close));
                self.arg("e", Some(body));
                self.out.push_str("</m:d>");
            }
            Node::Func { name, body } => {
                self.out.push_str("<m:func><m:fName>");
                match &**name {
                    Node::Text(t) => self.run(t, true),
                    Node::Script { base, sub, sup } => match &**base {
                        // Limits go under "lim" (like Word writes it).
                        Node::Text(t) if self.display && sup.is_none() && matches!(t.as_str(), "lim" | "max" | "min" | "sup" | "inf" | "liminf" | "limsup") => {
                            self.out.push_str("<m:limLow><m:e>");
                            self.run(t, true);
                            self.out.push_str("</m:e>");
                            self.arg("lim", sub.as_deref());
                            self.out.push_str("</m:limLow>");
                        }
                        Node::Text(t) => {
                            let t = t.clone();
                            self.scripts(&|w: &mut Self| w.run(&t, true), sub.as_deref(), sup.as_deref());
                        }
                        _ => self.node(name),
                    },
                    n => self.node(n),
                }
                self.out.push_str("</m:fName>");
                self.arg("e", Some(body));
                self.out.push_str("</m:func>");
            }
            Node::Matrix(rows) => {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                let _ = write!(self.out, r#"<m:m><m:mPr><m:mcs><m:mc><m:mcPr><m:count m:val="{cols}"/><m:mcJc m:val="center"/></m:mcPr></m:mc></m:mcs></m:mPr>"#);
                for r in rows {
                    self.out.push_str("<m:mr>");
                    for j in 0..cols {
                        self.arg("e", r.get(j));
                    }
                    self.out.push_str("</m:mr>");
                }
                self.out.push_str("</m:m>");
            }
        }
    }
}

// --- reading --------------------------------------------------------------------------------------

/// Reads an `m:oMath` or `m:oMathPara` (tag `t` just read) into equations,
/// with the font size of their runs.
pub(crate) fn read(r: &mut Reader, t: &Tag) -> Vec<(Arc<Equation>, Option<u16>)> {
    let el = El::read(r, t, true);
    let mut out = Vec::new();
    let mut add = |m: &El| {
        let mut v = Vec::new();
        seq(m, &mut v, 0);
        if !v.is_empty() {
            out.push((Arc::new(Equation::from_node(&row(v))), size_of(m)));
        }
    };
    if el.name == "oMathPara" {
        for m in el.els().filter(|e| e.name == "oMath") {
            add(m);
        }
    } else {
        add(&el);
    }
    out
}

/// The font size (half points) of the first run that sets one.
fn size_of(el: &El) -> Option<u16> {
    if el.name == "sz" {
        return el.attr("val").and_then(|v| v.parse().ok());
    }
    el.els().find_map(size_of)
}

fn one(el: Option<&El>, depth: u32) -> Node {
    let mut v = Vec::new();
    if let Some(e) = el {
        seq(e, &mut v, depth);
    }
    row(v)
}

fn val(el: &El, pr: &str, name: &str) -> Option<String> {
    el.child(pr)?.child(name).map(|e| e.attr("val").unwrap_or("").to_string())
}

fn on(el: &El, pr: &str, name: &str) -> bool {
    val(el, pr, name).is_some_and(|v| !matches!(v.as_str(), "0" | "off" | "false"))
}

fn opt(n: Node) -> Option<Box<Node>> {
    (!n.is_empty()).then(|| Box::new(n))
}

fn seq(el: &El, out: &mut Vec<Node>, depth: u32) {
    if depth > 64 {
        return;
    }
    let d = depth + 1;
    for k in el.els() {
        let e = |name: &str| k.child(name);
        match k.name.as_str() {
            "r" => {
                let plain = k.child("rPr").is_some_and(|p| p.child("nor").is_some() || p.child("sty").and_then(|s| s.attr("val")) == Some("p"));
                let text: String = k.els().filter(|c| c.name == "t").map(El::text).collect();
                if plain && text.chars().any(char::is_alphabetic) && !math::FUNCTIONS.contains(&text.trim()) {
                    out.push(Node::Text(text));
                } else {
                    tokens(&text, plain, out);
                }
            }
            "f" => out.push(Node::Frac(Box::new(one(e("num"), d)), Box::new(one(e("den"), d)))),
            "sSub" | "sSup" | "sSubSup" => out.push(Node::Script { base: Box::new(one(e("e"), d)), sub: opt(one(e("sub"), d)), sup: opt(one(e("sup"), d)) }),
            "sPre" => {
                out.push(Node::Script { base: Box::new(Node::Row(Vec::new())), sub: opt(one(e("sub"), d)), sup: opt(one(e("sup"), d)) });
                seq(k.child("e").unwrap_or(k), out, d);
            }
            "rad" => {
                let index = if on(k, "radPr", "degHide") { None } else { opt(one(e("deg"), d)) };
                out.push(Node::Radical { index, body: Box::new(one(e("e"), d)) });
            }
            "nary" => {
                let op = val(k, "naryPr", "chr").and_then(|c| c.chars().next()).unwrap_or('∫');
                let sub = if on(k, "naryPr", "subHide") { None } else { opt(one(e("sub"), d)) };
                let sup = if on(k, "naryPr", "supHide") { None } else { opt(one(e("sup"), d)) };
                out.push(Node::Nary { op, sub, sup, body: Box::new(one(e("e"), d)) });
            }
            "d" => {
                let open = match val(k, "dPr", "begChr") {
                    Some(v) => v.chars().next(),
                    None => Some('('),
                };
                let close = match val(k, "dPr", "endChr") {
                    Some(v) => v.chars().next(),
                    None => Some(')'),
                };
                let sep = val(k, "dPr", "sepChr").map_or(Some('│'), |v| v.chars().next());
                let mut v = Vec::new();
                for (i, part) in k.els().filter(|c| c.name == "e").enumerate() {
                    if i > 0 {
                        if let Some(s) = sep {
                            v.push(Node::Sym(s.to_string()));
                        }
                    }
                    let p = one(Some(part), d);
                    // A part that is a row stays one group.
                    v.push(p);
                }
                let body = if v.len() == 1 { v.pop().unwrap_or(Node::Row(Vec::new())) } else { Node::Row(v) };
                out.push(Node::Delim { open, close, body: Box::new(body) });
            }
            "func" => {
                let name = match one(e("fName"), d) {
                    Node::Ident(t) => Node::Text(t),
                    Node::Script { base, sub, sup } => match *base {
                        Node::Ident(t) => Node::Script { base: Box::new(Node::Text(t)), sub, sup },
                        b => Node::Script { base: Box::new(b), sub, sup },
                    },
                    n => n,
                };
                out.push(Node::Func { name: Box::new(name), body: Box::new(one(e("e"), d)) });
            }
            "limLow" | "limUpp" => {
                let base = match one(e("e"), d) {
                    Node::Ident(t) => Node::Text(t),
                    b => b,
                };
                let lim = opt(one(e("lim"), d));
                let (sub, sup) = if k.name == "limLow" { (lim, None) } else { (None, lim) };
                out.push(Node::Script { base: Box::new(base), sub, sup });
            }
            "m" => {
                let rows: Vec<Vec<Node>> = k.els().filter(|r| r.name == "mr").map(|r| r.els().filter(|c| c.name == "e").map(|c| one(Some(c), d)).collect()).collect();
                if !rows.is_empty() {
                    out.push(Node::Matrix(rows));
                }
            }
            "eqArr" => {
                let rows: Vec<Vec<Node>> = k.els().filter(|c| c.name == "e").map(|c| vec![one(Some(c), d)]).collect();
                if !rows.is_empty() {
                    out.push(Node::Matrix(rows));
                }
            }
            "acc" | "groupChr" => {
                let default = if k.name == "acc" { "\u{302}" } else { "\u{23DF}" };
                let chr = val(k, if k.name == "acc" { "accPr" } else { "groupChrPr" }, "chr").unwrap_or_else(|| default.to_string());
                let mark = match chr.as_str() {
                    "\u{302}" => "^".to_string(),
                    "\u{303}" => "~".to_string(),
                    "\u{304}" | "\u{305}" | "\u{AF}" => "¯".to_string(),
                    "\u{307}" => "˙".to_string(),
                    "\u{308}" => "¨".to_string(),
                    "\u{20D7}" | "\u{20D1}" => "→".to_string(),
                    c => c.to_string(),
                };
                let below = val(k, "groupChrPr", "pos").as_deref() == Some("bot") || (k.name == "groupChr" && val(k, "groupChrPr", "pos").is_none());
                let m = Some(Box::new(Node::Sym(mark)));
                let (sub, sup) = if below { (m, None) } else { (None, m) };
                out.push(Node::Script { base: Box::new(one(e("e"), d)), sub, sup });
            }
            "bar" | "box" | "borderBox" | "phant" => {
                if !(k.name == "phant" && val(k, "phantPr", "show").as_deref() == Some("0")) {
                    seq(k.child("e").unwrap_or(k), out, d);
                }
            }
            "oMath" => seq(k, out, d),
            // Word runs inside math: plain text.
            "t" => {
                let t = k.text();
                if !t.is_empty() {
                    out.push(Node::Text(t));
                }
            }
            n if n.ends_with("Pr") => {}
            _ => seq(k, out, d),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::xml::Ev;

    fn back(eq: &Equation, display: bool) -> Equation {
        let mut s = String::new();
        write(&mut s, "\u{FFFC}", &Attrs { math: Some(Arc::new(eq.clone())), ..Default::default() }, display);
        let mut r = Reader::new(&s);
        loop {
            match r.next() {
                Ev::Start(t) => return read(&mut r, &t).into_iter().next().expect("equation").0.as_ref().clone(),
                Ev::Eof => panic!("no math"),
                _ => {}
            }
        }
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
            "|x|+[0,1)",
            "α+β≤γ",
            "x_1_2",
        ] {
            for display in [false, true] {
                let eq = Equation::new(s);
                let b = back(&eq, display);
                assert_eq!(b.root(), eq.root(), "{s} -> {}", b.source());
            }
        }
    }

    /// A document with an equation in a sentence and one on its own.
    pub(crate) fn with_equations() -> apex_doc::Document {
        use apex_doc::{Document, Paragraph, Selection};
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Area "), Paragraph::new(""), Paragraph::new("end")]);
        d.insert_equation(Selection::caret(apex_doc::Pos::new(0, 5)), Equation::new("π r^2"));
        d.insert_equation(Selection::caret(apex_doc::Pos::new(1, 0)), Equation::new("x=(-b±√(b^2-4ac))/2a"));
        d
    }

    pub(crate) fn check_equations(back: &apex_doc::Document) {
        let eqs: Vec<(usize, Node, bool)> = back
            .paragraphs
            .iter()
            .enumerate()
            .flat_map(|(i, p)| p.runs.iter().filter_map(move |r| r.attrs.math.clone().map(|m| (i, m.root().clone(), math::is_display(p)))))
            .collect();
        assert_eq!(eqs.len(), 2, "{eqs:?}");
        assert_eq!(eqs[0], (0, Equation::new("π r^2").root().clone(), false));
        assert_eq!(eqs[1], (1, Equation::new("x=(-b±√(b^2-4ac))/2a").root().clone(), true));
        assert!(back.paragraphs[0].text.starts_with("Area "));
    }

    #[test]
    fn docx_round_trip() {
        let d = with_equations();
        let back = crate::docx::read(&crate::docx::write(&d)).expect("docx");
        check_equations(&back);
    }

    #[test]
    fn word_omml() {
        // As Word writes the quadratic formula.
        let xml = r#"<m:oMathPara><m:oMath><m:r><w:rPr><w:rFonts w:ascii="Cambria Math" w:hAnsi="Cambria Math"/></w:rPr><m:t>x=</m:t></m:r><m:f><m:num><m:r><m:t>-b±</m:t></m:r><m:rad><m:radPr><m:degHide m:val="1"/></m:radPr><m:deg/><m:e><m:sSup><m:e><m:r><m:t>b</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup><m:r><m:t>-4ac</m:t></m:r></m:e></m:rad></m:num><m:den><m:r><m:t>2a</m:t></m:r></m:den></m:f><m:func><m:funcPr/><m:fName><m:limLow><m:e><m:r><m:rPr><m:sty m:val="p"/></m:rPr><m:t>lim</m:t></m:r></m:e><m:lim><m:r><m:t>n→∞</m:t></m:r></m:lim></m:limLow></m:fName><m:e><m:r><m:t>a</m:t></m:r></m:e></m:func></m:oMath></m:oMathPara>"#;
        let mut r = Reader::new(xml);
        let Ev::Start(t) = r.next() else { panic!() };
        let eqs = read(&mut r, &t);
        assert_eq!(eqs.len(), 1);
        assert_eq!(eqs[0].0.root(), Equation::new("x=(-b±√(b^2-4ac))/2a \\lim_(n→∞) a").root(), "{}", eqs[0].0.source());
    }
}
