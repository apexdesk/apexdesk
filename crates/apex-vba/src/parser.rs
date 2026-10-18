//! Parser: tokens of a module into its declarations and procedures.
//! Statements follow VBA's grammar, with its ambiguities settled the way
//! the VBA editor does: `a = b` at the start of a statement assigns and
//! elsewhere compares, and `Foo (1) + 2` calls Foo with `(1) + 2`.

use std::collections::HashSet;
use std::rc::Rc;

use crate::ast::*;
use crate::error::{Error, R};
use crate::lexer::{Tok, Token, tokenize};
use crate::value::{VType, Value};

struct Parser<'a> {
    toks: Vec<Token>,
    i: usize,
    module: &'a str,
    /// Inside a single-line If: `Else` ends a statement.
    single_if: usize,
    /// How deep the parser is in itself (blocks, parentheses, signs): a
    /// file's code must not take it deeper than the stack goes.
    depth: usize,
    /// Operators read in the current statement (long chains make deep
    /// trees too).
    ops: usize,
    /// The project's `Enum` names, lowercase: `As` one of them is a Long.
    /// Its module names are here too, as `"module:" + name`: a class
    /// named like Excel's enumerations stays a class.
    enums: Rc<HashSet<String>>,
}

/// Deepest nesting of blocks and expressions read.
const MAX_NESTING: usize = 64;
/// Most operators in one statement.
const MAX_OPS: usize = 1024;

/// Reads a module's code (as the editor shows it, `Attribute` lines left
/// out or not).
pub fn parse_module(name: &str, kind: ModuleKind, code: &str) -> R<Module> {
    let toks = tokenize(code, name)?;
    let enums = Rc::new(enum_names(&toks).collect());
    parse_tokens(name, kind, toks, enums)
}

/// A module of a project whose `Enum` names are `enums`.
pub fn parse_tokens(name: &str, kind: ModuleKind, toks: Vec<Token>, enums: Rc<HashSet<String>>) -> R<Module> {
    let mut p = Parser { toks, i: 0, module: name, single_if: 0, depth: 0, ops: 0, enums };
    p.module_body(name, kind)
}

/// The names declared by `[Public|Private] Enum Name` lines, lowercase.
pub fn enum_names(toks: &[Token]) -> impl Iterator<Item = String> + '_ {
    toks.windows(2).enumerate().filter_map(|(i, w)| {
        let starts = i == 0
            || matches!(&toks[i - 1].tok, Tok::Eol | Tok::Colon)
            || matches!(&toks[i - 1].tok, Tok::Word(m) if (m.eq_ignore_ascii_case("public") || m.eq_ignore_ascii_case("private")) && (i == 1 || matches!(toks[i - 2].tok, Tok::Eol | Tok::Colon)));
        match (&w[0].tok, &w[1].tok) {
            (Tok::Word(e), Tok::Word(n)) if starts && e.eq_ignore_ascii_case("enum") => Some(n.to_ascii_lowercase()),
            _ => None,
        }
    })
}

/// The enumerations of VBA, Excel and Office used as types
/// (`Dim r As VbMsgBoxResult`, `As XlCalculation`): Longs.
fn builtin_enum(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let vb = ["vbmsgboxresult", "vbmsgboxstyle", "vbvartype", "vbcomparemethod", "vbdayofweek", "vbfirstweekofyear", "vbtristate", "vbfileattribute", "vbappwinstyle", "vbstrconv", "vbcalltype", "vbdatetimeformat", "vbquerycloseconstants"];
    let upper = |i: usize| name.chars().nth(i).is_some_and(|c| c.is_ascii_uppercase());
    vb.contains(&n.as_str()) || (n.starts_with("xl") && upper(2)) || (n.starts_with("mso") && upper(3)) || (n.starts_with("wd") && upper(2))
}

/// One expression (a breakpoint's condition, a watch).
pub fn parse_expression(module: &str, text: &str) -> R<Expr> {
    let toks = tokenize(text, module)?;
    let mut p = Parser { toks, i: 0, module, single_if: 0, depth: 0, ops: 0, enums: Rc::default() };
    while matches!(p.peek(), Tok::Eol) {
        p.i += 1;
    }
    let e = p.expr()?;
    while matches!(p.peek(), Tok::Eol) {
        p.i += 1;
    }
    if !matches!(p.peek(), Tok::Eof) {
        return Err(p.err("Expected: end of expression"));
    }
    Ok(e)
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        self.peek_at(0)
    }

    fn peek_at(&self, k: usize) -> &Tok {
        self.toks.get(self.i + k).map_or(&Tok::Eof, |t| &t.tok)
    }

    fn line(&self) -> u32 {
        self.toks.get(self.i).map_or(0, |t| t.line)
    }

    fn err(&self, msg: impl Into<String>) -> Error {
        Error::syntax(self.module, self.line(), msg)
    }

    fn is_word_at(&self, k: usize, w: &str) -> bool {
        matches!(self.peek_at(k), Tok::Word(x) if x.eq_ignore_ascii_case(w))
    }

    fn is_word(&self, w: &str) -> bool {
        self.is_word_at(0, w)
    }

    fn eat_word(&mut self, w: &str) -> bool {
        if self.is_word(w) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect_word(&mut self, w: &str) -> R<()> {
        if self.eat_word(w) { Ok(()) } else { Err(self.err(format!("Expected: {w}"))) }
    }

    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Sym(x) if *x == s)
    }

    fn eat_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect_sym(&mut self, s: &str) -> R<()> {
        if self.eat_sym(s) { Ok(()) } else { Err(self.err(format!("Expected: {s}"))) }
    }

    /// Any word (a name, or a keyword used as a member name).
    fn word(&mut self) -> R<String> {
        match self.peek().clone() {
            Tok::Word(w) => {
                self.i += 1;
                Ok(w)
            }
            _ => Err(self.err("Expected: identifier")),
        }
    }

    /// End of the statement: a new line, `:`, or `Else` in a single-line If.
    fn at_end(&self) -> bool {
        matches!(self.peek(), Tok::Eol | Tok::Colon | Tok::Eof) || self.single_if > 0 && self.is_word("else")
    }

    fn skip_eols(&mut self) {
        while matches!(self.peek(), Tok::Eol | Tok::Colon) {
            self.i += 1;
        }
    }

    fn skip_line(&mut self) {
        while !matches!(self.peek(), Tok::Eol | Tok::Eof) {
            self.i += 1;
        }
    }

    fn end_stmt(&mut self) -> R<()> {
        if self.at_end() { Ok(()) } else { Err(self.err("Expected: end of statement")) }
    }

    /// A type name after `As` (`Long`, `Excel.Range`, `String * 10`).
    fn type_name(&mut self) -> R<(VType, String)> {
        Ok(self.type_len()?.0)
    }

    /// A type after `As`, with the length of a `String * n`.
    fn type_len(&mut self) -> R<((VType, String), Option<u32>)> {
        let mut name = self.word()?;
        while self.eat_sym(".") {
            name = self.word()?;
        }
        let mut fixed = None;
        if self.eat_sym("*") {
            match self.peek() {
                Tok::Int(n) => {
                    // VBA's limit for a fixed-length string.
                    fixed = Some(u32::try_from(*n).ok().filter(|n| (1..=65_526).contains(n)).ok_or_else(|| self.err("Invalid fixed-length string length"))?);
                    self.i += 1;
                }
                Tok::Word(_) => self.i += 1,
                _ => return Err(self.err("Expected: length")),
            }
        }
        let mut ty = VType::parse(&name);
        let low = name.to_ascii_lowercase();
        if ty == VType::Object && (self.enums.contains(&low) || builtin_enum(&name) && !self.enums.contains(&format!("module:{low}"))) {
            ty = VType::Long;
        }
        Ok(((ty, name), fixed))
    }

    // --- module --------------------------------------------------------------

    fn module_body(&mut self, name: &str, kind: ModuleKind) -> R<Module> {
        let mut m = Module { name: name.to_string(), kind, explicit: false, compare_text: false, base: 0, vars: Vec::new(), consts: Vec::new(), procs: Vec::new(), declares: Vec::new(), types: Vec::new(), implements: Vec::new() };
        loop {
            self.skip_eols();
            match self.peek() {
                Tok::Eof => break,
                Tok::Label(_) => {
                    self.i += 1;
                    continue;
                }
                Tok::Sym("#") => {
                    // #If / #Else / #End If / #Const: both branches are read.
                    self.skip_line();
                    continue;
                }
                _ => {}
            }
            let mut public = None;
            let mut is_static = false;
            loop {
                if self.eat_word("public") || self.eat_word("global") || self.eat_word("friend") {
                    public = Some(true);
                } else if self.eat_word("private") {
                    public = Some(false);
                } else if self.eat_word("static") {
                    is_static = true;
                } else {
                    break;
                }
            }
            let w = match self.peek() {
                Tok::Word(w) => lower(w),
                _ => return Err(self.err("Statement not valid outside a procedure")),
            };
            match w.as_str() {
                "option" => {
                    self.i += 1;
                    if self.eat_word("explicit") {
                        m.explicit = true;
                    } else if self.eat_word("compare") {
                        m.compare_text = self.eat_word("text");
                        self.skip_line();
                    } else if self.eat_word("base") {
                        if let Tok::Int(b) = self.peek() {
                            m.base = *b;
                        }
                        self.skip_line();
                    } else {
                        self.skip_line();
                    }
                }
                "sub" | "function" | "property" => {
                    let p = self.procedure(public.unwrap_or(true), is_static)?;
                    m.procs.push(Rc::new(p));
                }
                "declare" => {
                    self.i += 1;
                    self.eat_word("ptrsafe");
                    if !(self.eat_word("sub") || self.eat_word("function")) {
                        return Err(self.err("Expected: Sub or Function"));
                    }
                    let name = lower(&self.word()?);
                    let text = |p: &mut Self, w: &str| {
                        if !p.eat_word(w) {
                            return None;
                        }
                        let Tok::Str(s) = p.peek().clone() else { return None };
                        p.i += 1;
                        Some(s.to_ascii_lowercase())
                    };
                    let lib = text(self, "lib").unwrap_or_default();
                    let api = text(self, "alias").unwrap_or_else(|| name.clone());
                    m.declares.push(Declare { name, lib, api });
                    self.skip_line();
                }
                "const" => {
                    self.i += 1;
                    for (n, e) in self.const_list()? {
                        m.consts.push((n, e, public.unwrap_or(false)));
                    }
                }
                "enum" => {
                    self.i += 1;
                    self.word()?;
                    let mut prev: Option<Expr> = None;
                    loop {
                        self.skip_eols();
                        if self.is_word("end") && self.is_word_at(1, "enum") {
                            self.i += 2;
                            break;
                        }
                        if matches!(self.peek(), Tok::Eof) {
                            return Err(self.err("Expected: End Enum"));
                        }
                        let n = lower(&self.word()?);
                        let e = if self.eat_sym("=") {
                            self.expr()?
                        } else {
                            match prev.take() {
                                Some(p) => Expr::Bin(BinOp::Add, Box::new(p), Box::new(Expr::Lit(Value::Long(1)))),
                                None => Expr::Lit(Value::Long(0)),
                            }
                        };
                        prev = Some(Expr::Name(n.clone()));
                        m.consts.push((n, e, public.unwrap_or(true)));
                        self.skip_line();
                    }
                }
                "type" => {
                    self.i += 1;
                    let name = self.word()?;
                    self.end_stmt()?;
                    let mut fields = Vec::new();
                    loop {
                        self.skip_eols();
                        if self.is_word("end") && self.is_word_at(1, "type") {
                            self.i += 2;
                            break;
                        }
                        if matches!(self.peek(), Tok::Eof) {
                            return Err(self.err("Expected: End Type"));
                        }
                        fields.extend(self.dim_list()?);
                        self.end_stmt()?;
                    }
                    if fields.is_empty() {
                        return Err(self.err("User-defined type without elements"));
                    }
                    m.types.push(TypeDef { name, fields, public: public.unwrap_or(true) });
                }
                "implements" => {
                    self.i += 1;
                    let n = self.word()?;
                    m.implements.push(lower(&n));
                }
                "event" | "attribute" | "version" | "begin" | "deflng" | "defint" | "defstr" | "defbool" | "defdbl" | "defvar" | "defobj" | "defdate" | "defcur" | "defsng" | "defbyte" => self.skip_line(),
                "dim" => {
                    self.i += 1;
                    for v in self.dim_list()? {
                        m.vars.push((v, public.unwrap_or(false)));
                    }
                }
                _ if public.is_some() => {
                    for v in self.dim_list()? {
                        m.vars.push((v, public.unwrap_or(false)));
                    }
                }
                _ => return Err(self.err("Statement not valid outside a procedure")),
            }
            self.end_stmt()?;
        }
        Ok(m)
    }

    fn procedure(&mut self, public: bool, is_static: bool) -> R<Proc> {
        let line = self.line();
        let kind = match lower(&self.word()?).as_str() {
            "sub" => ProcKind::Sub,
            "function" => ProcKind::Function,
            _ => {
                if self.eat_word("get") {
                    ProcKind::PropertyGet
                } else if self.eat_word("let") || self.eat_word("set") {
                    ProcKind::PropertyLet
                } else {
                    return Err(self.err("Expected: Get, Let or Set"));
                }
            }
        };
        let name = self.word()?;
        let mut params = Vec::new();
        if self.eat_sym("(") {
            if !self.eat_sym(")") {
                loop {
                    params.push(self.param()?);
                    if self.eat_sym(")") {
                        break;
                    }
                    self.expect_sym(",")?;
                }
            }
        }
        let mut ret = VType::Variant;
        let mut ret_name = None;
        if self.eat_word("as") {
            let (t, n) = self.type_name()?;
            ret = t;
            if t == VType::Object {
                ret_name = Some(lower(&n));
            }
            if self.eat_sym("(") {
                self.expect_sym(")")?;
                ret = VType::Variant;
                ret_name = None;
            }
        }
        self.end_stmt()?;
        let end = match kind {
            ProcKind::Sub => "sub",
            ProcKind::Function => "function",
            _ => "property",
        };
        let body = self.block(&[&["end", end]])?;
        self.i += 2;
        Ok(Proc { name, kind, public, params, ret, ret_name, body, line, is_static })
    }

    fn param(&mut self) -> R<Param> {
        let mut p = Param { name: String::new(), byval: false, optional: false, paramarray: false, default: None, ty: VType::Variant, array: false };
        loop {
            if self.eat_word("optional") {
                p.optional = true;
            } else if self.eat_word("byval") {
                p.byval = true;
            } else if self.eat_word("byref") {
                p.byval = false;
            } else if self.eat_word("paramarray") {
                p.paramarray = true;
            } else {
                break;
            }
        }
        p.name = lower(&self.word()?);
        if self.eat_sym("(") {
            self.expect_sym(")")?;
            p.array = true;
        }
        if self.eat_word("as") {
            p.ty = self.type_name()?.0;
        }
        if self.eat_sym("=") {
            p.default = Some(self.expr()?);
        }
        Ok(p)
    }

    fn const_list(&mut self) -> R<Vec<(String, Expr)>> {
        let mut out = Vec::new();
        loop {
            let n = lower(&self.word()?);
            if self.eat_word("as") {
                self.type_name()?;
            }
            self.expect_sym("=")?;
            out.push((n, self.expr()?));
            if !self.eat_sym(",") {
                break;
            }
        }
        Ok(out)
    }

    fn bounds(&mut self) -> R<Vec<(Option<Expr>, Expr)>> {
        let mut out = Vec::new();
        if self.eat_sym(")") {
            return Ok(out);
        }
        loop {
            let a = self.expr()?;
            if self.eat_word("to") {
                out.push((Some(a), self.expr()?));
            } else {
                out.push((None, a));
            }
            if self.eat_sym(")") {
                break;
            }
            self.expect_sym(",")?;
        }
        Ok(out)
    }

    fn dim_list(&mut self) -> R<Vec<DimVar>> {
        self.decl_list(false)
    }

    /// Variables of a Dim, or of a ReDim (`redim`: fields of a variable too).
    fn decl_list(&mut self, redim: bool) -> R<Vec<DimVar>> {
        let mut out = Vec::new();
        loop {
            let withevents = self.eat_word("withevents");
            let name = lower(&self.word()?);
            let mut fields = Vec::new();
            while redim && self.eat_sym(".") {
                fields.push(lower(&self.word()?));
            }
            let bounds = if self.eat_sym("(") { Some(self.bounds()?) } else { None };
            let mut ty = VType::Variant;
            let mut new = None;
            let mut ty_name = None;
            let mut fixed = None;
            if self.eat_word("as") {
                let is_new = self.eat_word("new");
                let ((t, n), len) = self.type_len()?;
                ty = t;
                fixed = len.filter(|_| t == VType::String);
                if is_new {
                    new = Some(lower(&n));
                } else if t == VType::Object {
                    ty_name = Some(lower(&n));
                }
            }
            out.push(DimVar { name, bounds, ty, new, ty_name, fixed, fields, withevents });
            if !self.eat_sym(",") {
                break;
            }
        }
        Ok(out)
    }

    // --- statements ----------------------------------------------------------

    /// Whether the statement at hand is one of `ends` (word sequences).
    fn at_block_end(&self, ends: &[&[&str]]) -> bool {
        ends.iter().any(|e| e.iter().enumerate().all(|(k, w)| self.is_word_at(k, w)))
            // `End` alone is the End statement, `EndIf` is End If.
            || ends.iter().any(|e| e == &["end", "if"]) && self.is_word("endif")
    }

    /// Statements up to one of `ends`, which is left for the caller.
    fn block(&mut self, ends: &[&[&str]]) -> R<Vec<Line>> {
        self.nested(|p| p.block_in(ends))
    }

    /// Counts an operator (or call or member) of the statement, failing
    /// past [`MAX_OPS`].
    fn op(&mut self) -> R<()> {
        self.ops += 1;
        if self.ops > MAX_OPS {
            return Err(self.err("Too complex: too many operators in one statement"));
        }
        Ok(())
    }

    /// Runs `f` one level deeper, failing past [`MAX_NESTING`].
    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        if self.depth >= MAX_NESTING {
            return Err(self.err("Too complex: nested too deeply"));
        }
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        r
    }

    fn block_in(&mut self, ends: &[&[&str]]) -> R<Vec<Line>> {
        let mut out = Vec::new();
        loop {
            self.skip_eols();
            let stray = self.is_word("end") && ["sub", "function", "property", "if", "with", "select"].iter().any(|w| self.is_word_at(1, w));
            if matches!(self.peek(), Tok::Eof) || stray && !self.at_block_end(ends) {
                let last = ends[ends.len() - 1];
                let want: Vec<String> = last.iter().map(|w| {
                    let mut c = w.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                }).collect();
                return Err(self.err(format!("Expected: {}", want.join(" "))));
            }
            if self.at_block_end(ends) {
                if self.is_word("endif") {
                    // Leave two tokens' worth for the caller: `End If`.
                    let line = self.line();
                    self.toks[self.i] = Token { tok: Tok::Word("End".into()), line };
                    self.toks.insert(self.i + 1, Token { tok: Tok::Word("If".into()), line });
                }
                return Ok(out);
            }
            out.push(self.statement()?);
            if !self.at_end() && !(self.single_if > 0) {
                return Err(self.err("Expected: end of statement"));
            }
        }
    }

    fn statement(&mut self) -> R<Line> {
        self.ops = 0;
        let line = self.line();
        let stmt = self.stmt()?;
        Ok(Line { line, stmt })
    }

    fn stmt(&mut self) -> R<Stmt> {
        if let Tok::Label(l) = self.peek().clone() {
            self.i += 1;
            return Ok(Stmt::Label(lower(&l)));
        }
        if self.is_sym("#") {
            self.skip_line();
            return Ok(Stmt::Label(String::new()));
        }
        let w = match self.peek() {
            Tok::Word(w) => lower(w),
            _ => String::new(),
        };
        let next_is = |p: &Parser, s: &str| matches!(p.peek_at(1), Tok::Sym(x) if *x == s);
        let assignment_like = next_is(self, "=") || next_is(self, ".") || next_is(self, "(");
        match w.as_str() {
            "dim" | "static" => {
                self.i += 1;
                Ok(Stmt::Dim { vars: self.dim_list()?, is_static: w == "static" })
            }
            "private" | "public" | "global" => Err(self.err("Invalid inside procedure")),
            "redim" => {
                self.i += 1;
                let preserve = self.eat_word("preserve");
                Ok(Stmt::ReDim { preserve, vars: self.decl_list(true)? })
            }
            "const" => {
                self.i += 1;
                Ok(Stmt::Const(self.const_list()?))
            }
            "set" => {
                self.i += 1;
                let target = self.postfix()?;
                self.expect_sym("=")?;
                Ok(Stmt::Assign { target, value: self.expr()?, set: true })
            }
            "let" => {
                self.i += 1;
                let target = self.postfix()?;
                self.expect_sym("=")?;
                Ok(Stmt::Assign { target, value: self.expr()?, set: false })
            }
            "if" => self.if_stmt(),
            "select" => self.select_stmt(),
            "for" => self.for_stmt(),
            "do" => {
                self.i += 1;
                let pre = self.do_cond(true)?;
                self.end_stmt()?;
                let body = self.block(&[&["loop"]])?;
                self.i += 1;
                let post = self.do_cond(false)?;
                if pre.is_some() && post.is_some() {
                    return Err(self.err("Loop condition given twice"));
                }
                Ok(Stmt::Do { cond: pre.or(post), body })
            }
            "while" => {
                self.i += 1;
                let expr = self.expr()?;
                self.end_stmt()?;
                let body = self.block(&[&["wend"]])?;
                self.i += 1;
                Ok(Stmt::Do { cond: Some(DoCond { until: false, pre: true, expr }), body })
            }
            "with" => {
                self.i += 1;
                let obj = self.expr()?;
                self.end_stmt()?;
                let body = self.block(&[&["end", "with"]])?;
                self.i += 2;
                Ok(Stmt::With { obj, body })
            }
            "exit" => {
                self.i += 1;
                let k = lower(&self.word()?);
                Ok(Stmt::Exit(match k.as_str() {
                    "sub" | "function" | "property" => ExitKind::Sub,
                    "for" => ExitKind::For,
                    "do" => ExitKind::Do,
                    _ => return Err(self.err("Expected: Sub, Function, For or Do")),
                }))
            }
            "on" if self.is_word_at(1, "error") => {
                self.i += 2;
                if self.eat_word("resume") {
                    self.expect_word("next")?;
                    return Ok(Stmt::OnError(OnError::ResumeNext));
                }
                self.expect_word("goto")?;
                match self.peek().clone() {
                    Tok::Int(_) => {
                        self.i += 1;
                        Ok(Stmt::OnError(OnError::Off))
                    }
                    Tok::Sym("-") => {
                        self.i += 2;
                        Ok(Stmt::OnError(OnError::Off))
                    }
                    _ => Ok(Stmt::OnError(OnError::GoTo(lower(&self.word()?)))),
                }
            }
            "resume" => {
                self.i += 1;
                if self.eat_word("next") {
                    return Ok(Stmt::Resume(ResumeKind::Next));
                }
                match self.peek().clone() {
                    Tok::Word(l) => {
                        self.i += 1;
                        Ok(Stmt::Resume(ResumeKind::Label(lower(&l))))
                    }
                    Tok::Int(0) => {
                        self.i += 1;
                        Ok(Stmt::Resume(ResumeKind::Same))
                    }
                    Tok::Int(n) => {
                        self.i += 1;
                        Ok(Stmt::Resume(ResumeKind::Label(n.to_string())))
                    }
                    _ => Ok(Stmt::Resume(ResumeKind::Same)),
                }
            }
            "raiseevent" => {
                self.i += 1;
                let name = lower(&self.word()?);
                let args = if self.eat_sym("(") {
                    let a = self.arg_list(Some(")"))?;
                    self.expect_sym(")")?;
                    a
                } else {
                    Vec::new()
                };
                Ok(Stmt::RaiseEvent(name, args))
            }
            "gosub" => {
                self.i += 1;
                match self.peek().clone() {
                    Tok::Int(n) => {
                        self.i += 1;
                        Ok(Stmt::GoSub(n.to_string()))
                    }
                    _ => Ok(Stmt::GoSub(lower(&self.word()?))),
                }
            }
            "return" => {
                self.i += 1;
                Ok(Stmt::Return)
            }
            "lset" | "rset" => {
                self.i += 1;
                let target = self.postfix()?;
                self.expect_sym("=")?;
                Ok(Stmt::Align { target, value: self.expr()?, right: w == "rset" })
            }
            "goto" => {
                self.i += 1;
                match self.peek().clone() {
                    Tok::Int(n) => {
                        self.i += 1;
                        Ok(Stmt::GoTo(n.to_string()))
                    }
                    _ => Ok(Stmt::GoTo(lower(&self.word()?))),
                }
            }
            "call" => {
                self.i += 1;
                match self.postfix()? {
                    Expr::Call(f, args) => Ok(Stmt::Call { target: *f, args }),
                    t => Ok(Stmt::Call { target: t, args: Vec::new() }),
                }
            }
            "end" if matches!(self.peek_at(1), Tok::Eol | Tok::Colon | Tok::Eof) => {
                self.i += 1;
                Ok(Stmt::End)
            }
            "stop" if matches!(self.peek_at(1), Tok::Eol | Tok::Colon | Tok::Eof) => {
                self.i += 1;
                Ok(Stmt::Stop)
            }
            "erase" => {
                self.i += 1;
                let mut list = vec![self.expr()?];
                while self.eat_sym(",") {
                    list.push(self.expr()?);
                }
                Ok(Stmt::Erase(list))
            }
            "debug" if next_is(self, ".") && self.is_word_at(2, "assert") => {
                self.i += 3;
                Ok(Stmt::Assert(self.expr()?))
            }
            "debug" if next_is(self, ".") && self.is_word_at(2, "print") => {
                self.i += 3;
                Ok(Stmt::Print(self.print_items()?))
            }
            "open" | "close" | "reset" | "print" | "write" | "input" | "line" | "get" | "put" | "seek" | "lock" | "unlock" | "width" | "name" if self.is_file_stmt(&w, assignment_like) => self.file_stmt(&w),
            "attribute" => {
                self.skip_line();
                Ok(Stmt::Label(String::new()))
            }
            _ => self.expr_stmt(),
        }
    }

    /// Whether the statement starting with `w` is a file statement.
    fn is_file_stmt(&mut self, w: &str, assignment_like: bool) -> bool {
        let hash = matches!(self.peek_at(1), Tok::Sym("#"));
        match w {
            "print" | "write" | "input" => hash,
            "line" => self.is_word_at(1, "input"),
            // `Name a As b`, not a call of a procedure `Name`.
            "name" => !assignment_like && self.name_stmt_ahead(),
            _ => !assignment_like,
        }
    }

    /// The file statements (`Open`, `Print #`...): apart from `stmt`, whose
    /// frame nests deep.
    #[inline(never)]
    fn file_stmt(&mut self, w: &str) -> R<Stmt> {
        let f = match w {
            "open" => return self.file_open(),
            "close" | "reset" => {
                self.i += 1;
                let mut nums = Vec::new();
                while !self.at_end() {
                    if !self.eat_sym(",") {
                        nums.push(self.file_num()?);
                    }
                }
                FileStmt::Close(nums)
            }
            "print" => {
                self.i += 1;
                let num = self.file_num()?;
                let mut items = Vec::new();
                if self.eat_sym(",") {
                    items = self.print_items()?;
                }
                FileStmt::Print { num, items }
            }
            "write" => {
                self.i += 1;
                let num = self.file_num()?;
                let (mut items, mut open) = (Vec::new(), false);
                if self.eat_sym(",") {
                    while !self.at_end() {
                        if self.eat_sym(",") || self.eat_sym(";") {
                            open = true;
                            continue;
                        }
                        items.push(self.expr()?);
                        open = false;
                    }
                }
                FileStmt::Write { num, items, open }
            }
            "input" => {
                self.i += 1;
                let num = self.file_num()?;
                let mut vars = Vec::new();
                while self.eat_sym(",") {
                    vars.push(self.postfix()?);
                }
                if vars.is_empty() {
                    return Err(self.err("Expected: ,"));
                }
                FileStmt::Input { num, vars }
            }
            "line" => {
                self.i += 2;
                let num = self.file_num()?;
                self.expect_sym(",")?;
                FileStmt::LineInput { num, var: self.postfix()? }
            }
            "get" | "put" => {
                self.i += 1;
                let num = self.file_num()?;
                self.expect_sym(",")?;
                let pos = if self.is_sym(",") { None } else { Some(self.expr()?) };
                self.expect_sym(",")?;
                FileStmt::Get { num, pos, var: self.postfix()?, put: w == "put" }
            }
            "seek" => {
                self.i += 1;
                let num = self.file_num()?;
                self.expect_sym(",")?;
                FileStmt::Seek { num, pos: self.expr()? }
            }
            "name" => {
                self.i += 1;
                let from = self.expr()?;
                self.expect_word("as")?;
                FileStmt::Name { from, to: self.expr()? }
            }
            // Lock, Unlock, Width #.
            _ => {
                self.skip_stmt();
                FileStmt::Nothing
            }
        };
        Ok(Stmt::File(f))
    }

    /// `[#]n`: a file's number.
    fn file_num(&mut self) -> R<Expr> {
        self.eat_sym("#");
        self.expr()
    }

    /// `Open path For mode [Access Read|Write|Read Write] [Shared|Lock ...]
    /// As [#]n [Len = len]`.
    fn file_open(&mut self) -> R<Stmt> {
        self.i += 1;
        let path = self.expr()?;
        self.expect_word("for")?;
        let mode = match lower(&self.word()?).as_str() {
            "input" => FileMode::Input,
            "output" => FileMode::Output,
            "append" => FileMode::Append,
            "binary" => FileMode::Binary,
            "random" => FileMode::Random,
            _ => return Err(self.err("Expected: Input, Output, Append, Binary or Random")),
        };
        let (mut read, mut write) = (false, false);
        if self.eat_word("access") {
            read = self.eat_word("read");
            write = self.eat_word("write");
            if !read && !write {
                return Err(self.err("Expected: Read or Write"));
            }
        }
        if !self.eat_word("shared") && self.eat_word("lock") {
            let read = self.eat_word("read");
            if !self.eat_word("write") && !read {
                return Err(self.err("Expected: Read or Write"));
            }
        }
        self.expect_word("as")?;
        let num = self.file_num()?;
        let len = if self.eat_word("len") {
            self.expect_sym("=")?;
            Some(self.expr()?)
        } else {
            None
        };
        Ok(Stmt::File(FileStmt::Open { path, mode, read, write, num, len }))
    }

    /// `Print #n, ...`'s items.
    fn print_items(&mut self) -> R<Vec<PrintItem>> {
        let mut items = Vec::new();
        while !self.at_end() {
            if self.eat_sym(",") {
                items.push(PrintItem::Comma);
            } else if self.eat_sym(";") {
                items.push(PrintItem::Semi);
            } else if self.is_word_at(0, "spc") && matches!(self.peek_at(1), Tok::Sym("(")) {
                self.i += 2;
                items.push(PrintItem::Spc(self.expr()?));
                self.expect_sym(")")?;
            } else if self.is_word_at(0, "tab") {
                self.i += 1;
                if self.eat_sym("(") {
                    items.push(PrintItem::Tab(Some(self.expr()?)));
                    self.expect_sym(")")?;
                } else {
                    items.push(PrintItem::Tab(None));
                }
            } else {
                items.push(PrintItem::Expr(self.expr()?));
            }
        }
        Ok(items)
    }

    /// Whether the statement is `Name from As to` (not a call of `Name`).
    fn name_stmt_ahead(&mut self) -> bool {
        let (at, ops) = (self.i, self.ops);
        self.i += 1;
        let ok = self.expr().is_ok() && self.is_word_at(0, "as");
        (self.i, self.ops) = (at, ops);
        ok
    }

    fn skip_stmt(&mut self) {
        while !self.at_end() {
            self.i += 1;
        }
    }

    /// An assignment, or a call with its arguments with or without parentheses.
    fn expr_stmt(&mut self) -> R<Stmt> {
        let (target, last_paren) = self.postfix_tracked()?;
        if self.eat_sym("=") {
            return Ok(Stmt::Assign { target, value: self.expr()?, set: false });
        }
        if self.at_end() {
            return Ok(match target {
                // `Foo (x)`: the parentheses make x an expression (passed ByVal).
                Expr::Call(f, mut args) if last_paren.is_some() && args.len() == 1 && args[0].name.is_none() => {
                    if let Some(e) = args[0].expr.take() {
                        args[0].expr = Some(Expr::Paren(Box::new(e)));
                    }
                    Stmt::Call { target: *f, args }
                }
                Expr::Call(f, args) => Stmt::Call { target: *f, args },
                t => Stmt::Call { target: t, args: Vec::new() },
            });
        }
        // `Foo (1) + 2, 3`: the parentheses belong to the first argument.
        let target = match (last_paren, target) {
            (Some((at, callee)), Expr::Call(..)) => {
                self.i = at;
                callee
            }
            (_, t) => t,
        };
        let args = self.arg_list(None)?;
        Ok(Stmt::Call { target, args })
    }

    /// `Do While x` / `Loop Until x`.
    fn do_cond(&mut self, pre: bool) -> R<Option<DoCond>> {
        let until = if self.eat_word("while") {
            false
        } else if self.eat_word("until") {
            true
        } else {
            return Ok(None);
        };
        Ok(Some(DoCond { until, pre, expr: self.expr()? }))
    }

    fn if_stmt(&mut self) -> R<Stmt> {
        self.i += 1;
        let cond = self.expr()?;
        self.expect_word("then")?;
        if !matches!(self.peek(), Tok::Eol | Tok::Eof) {
            // Single-line If.
            self.single_if += 1;
            let then = self.inline_stmts();
            let other = match then {
                Ok(_) if self.eat_word("else") => Some(self.inline_stmts()),
                _ => None,
            };
            self.single_if -= 1;
            let then = then?;
            let other = other.transpose()?;
            return Ok(Stmt::If { branches: vec![(cond, then)], other });
        }
        let mut branches = Vec::new();
        let ends: &[&[&str]] = &[&["elseif"], &["else"], &["end", "if"]];
        let body = self.block(ends)?;
        branches.push((cond, body));
        let mut other = None;
        loop {
            if self.eat_word("elseif") {
                let c = self.expr()?;
                self.expect_word("then")?;
                let body = self.block(ends)?;
                branches.push((c, body));
            } else if self.eat_word("else") {
                other = Some(self.block(&[&["end", "if"]])?);
            } else {
                self.expect_word("end")?;
                self.expect_word("if")?;
                break;
            }
        }
        Ok(Stmt::If { branches, other })
    }

    /// The statements of a single-line If branch: `a: b` up to the line end or `Else`.
    fn inline_stmts(&mut self) -> R<Vec<Line>> {
        let mut out = Vec::new();
        loop {
            if matches!(self.peek(), Tok::Eol | Tok::Eof) || self.is_word("else") {
                break;
            }
            out.push(self.statement()?);
            if !self.eat_sym_colon() {
                break;
            }
        }
        Ok(out)
    }

    fn eat_sym_colon(&mut self) -> bool {
        if matches!(self.peek(), Tok::Colon) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn select_stmt(&mut self) -> R<Stmt> {
        self.i += 1;
        self.expect_word("case")?;
        let expr = self.expr()?;
        let mut cases = Vec::new();
        let mut other = None;
        let ends: &[&[&str]] = &[&["case"], &["end", "select"]];
        loop {
            self.skip_eols();
            if self.eat_word("case") {
                if self.eat_word("else") {
                    other = Some(self.block(ends)?);
                    continue;
                }
                let mut tests = Vec::new();
                loop {
                    if self.eat_word("is") {
                        let op = match self.peek() {
                            Tok::Sym("=") => BinOp::Eq,
                            Tok::Sym("<>") => BinOp::Ne,
                            Tok::Sym("<") => BinOp::Lt,
                            Tok::Sym(">") => BinOp::Gt,
                            Tok::Sym("<=") => BinOp::Le,
                            Tok::Sym(">=") => BinOp::Ge,
                            _ => return Err(self.err("Expected: comparison operator")),
                        };
                        self.i += 1;
                        tests.push(CaseTest::Is(op, self.expr()?));
                    } else {
                        let a = self.expr()?;
                        if self.eat_word("to") {
                            tests.push(CaseTest::Range(a, self.expr()?));
                        } else {
                            tests.push(CaseTest::Value(a));
                        }
                    }
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                let body = self.block(ends)?;
                cases.push((tests, body));
            } else if self.eat_word("end") {
                self.expect_word("select")?;
                break;
            } else {
                return Err(self.err("Expected: Case"));
            }
        }
        Ok(Stmt::Select { expr, cases, other })
    }

    fn for_stmt(&mut self) -> R<Stmt> {
        self.i += 1;
        if self.eat_word("each") {
            let var = self.postfix()?;
            self.expect_word("in")?;
            let group = self.expr()?;
            self.end_stmt()?;
            let body = self.block(&[&["next"]])?;
            self.next_clause();
            return Ok(Stmt::ForEach { var, group, body });
        }
        let var = self.postfix()?;
        self.expect_sym("=")?;
        let from = self.expr()?;
        self.expect_word("to")?;
        let to = self.expr()?;
        let step = if self.eat_word("step") { Some(self.expr()?) } else { None };
        self.end_stmt()?;
        let body = self.block(&[&["next"]])?;
        self.next_clause();
        Ok(Stmt::For { var, from, to, step, body })
    }

    /// `Next`, `Next i` or `Next i, j` (which also closes the outer loop).
    fn next_clause(&mut self) {
        self.i += 1;
        if matches!(self.peek(), Tok::Word(_)) && !self.at_end() {
            self.i += 1;
            if self.is_sym(",") {
                let line = self.line();
                self.toks[self.i] = Token { tok: Tok::Word("Next".into()), line };
                self.toks.insert(self.i, Token { tok: Tok::Colon, line });
            }
        }
    }

    // --- expressions ---------------------------------------------------------

    fn arg_list(&mut self, close: Option<&str>) -> R<Vec<Arg>> {
        let mut args = Vec::new();
        let done = |p: &Parser| match close {
            Some(c) => p.is_sym(c),
            None => p.at_end(),
        };
        if done(self) {
            return Ok(args);
        }
        loop {
            if self.is_sym(",") {
                args.push(Arg { name: None, expr: None });
            } else if done(self) {
                args.push(Arg { name: None, expr: None });
                break;
            } else {
                let name = match (self.peek(), self.peek_at(1)) {
                    (Tok::Word(w), Tok::Sym(":=")) => Some(lower(w)),
                    _ => None,
                };
                if name.is_some() {
                    self.i += 2;
                }
                self.eat_word("byval");
                args.push(Arg { name, expr: Some(self.expr()?) });
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        Ok(args)
    }

    pub fn expr(&mut self) -> R<Expr> {
        self.nested(|p| p.binary(0))
    }

    /// Binary operators by precedence, loosest first.
    fn binary(&mut self, level: usize) -> R<Expr> {
        const LEVELS: &[&[(&str, BinOp)]] = &[
            &[("imp", BinOp::Imp)],
            &[("eqv", BinOp::Eqv)],
            &[("xor", BinOp::Xor)],
            &[("or", BinOp::Or)],
            &[("and", BinOp::And)],
            // (Not)
            &[("=", BinOp::Eq), ("<>", BinOp::Ne), ("<", BinOp::Lt), (">", BinOp::Gt), ("<=", BinOp::Le), (">=", BinOp::Ge), ("like", BinOp::Like), ("is", BinOp::Is)],
            &[("&", BinOp::Concat)],
            &[("+", BinOp::Add), ("-", BinOp::Sub)],
            &[("mod", BinOp::Mod)],
            &[("\\", BinOp::IntDiv)],
            &[("*", BinOp::Mul), ("/", BinOp::Div)],
        ];
        if level == 5 && self.is_word("not") {
            self.i += 1;
            return Ok(Expr::Not(Box::new(self.nested(|p| p.binary(5))?)));
        }
        if level >= LEVELS.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        loop {
            let op = LEVELS[level].iter().find(|(s, _)| match self.peek() {
                Tok::Sym(x) => x == s,
                Tok::Word(w) => s.chars().all(char::is_alphabetic) && w.eq_ignore_ascii_case(s),
                _ => false,
            });
            let Some(&(_, op)) = op else { break };
            self.i += 1;
            self.op()?;
            let right = self.binary(level + 1)?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> R<Expr> {
        if self.eat_sym("-") {
            return Ok(Expr::Neg(Box::new(self.nested(|p| p.unary())?)));
        }
        if self.eat_sym("+") {
            return self.nested(|p| p.unary());
        }
        let mut left = self.postfix()?;
        while self.eat_sym("^") {
            self.op()?;
            let right = if self.eat_sym("-") { Expr::Neg(Box::new(self.postfix()?)) } else { self.postfix()? };
            left = Expr::Bin(BinOp::Pow, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn postfix(&mut self) -> R<Expr> {
        Ok(self.postfix_tracked()?.0)
    }

    /// A primary with its calls and members; also where the last call's
    /// parentheses began and what was before them.
    fn postfix_tracked(&mut self) -> R<(Expr, Option<(usize, Expr)>)> {
        let mut e = self.primary()?;
        let mut last = None;
        loop {
            if self.is_sym("(") || self.is_sym(".") || self.is_sym("!") {
                // Each link nests the expression one more level.
                self.op()?;
            }
            if self.is_sym("(") {
                let at = self.i;
                self.i += 1;
                let args = self.arg_list(Some(")"))?;
                self.expect_sym(")")?;
                last = Some((at, e.clone()));
                e = Expr::Call(Box::new(e), args);
            } else if self.eat_sym(".") {
                let name = self.word()?;
                e = Expr::Member(Box::new(e), lower(&name));
                last = None;
            } else if self.eat_sym("!") {
                let name = self.word()?;
                e = Expr::Call(Box::new(e), vec![Arg { name: None, expr: Some(Expr::Lit(Value::str(name))) }]);
                last = None;
            } else {
                break;
            }
        }
        Ok((e, last))
    }

    fn primary(&mut self) -> R<Expr> {
        let tok = self.peek().clone();
        self.i += 1;
        Ok(match tok {
            Tok::Int(v) => Expr::Lit(if let Ok(x) = i16::try_from(v) {
                Value::Int(x)
            } else if let Ok(x) = i32::try_from(v) {
                Value::Long(x)
            } else {
                Value::Double(v as f64)
            }),
            Tok::Num(v) => Expr::Lit(Value::Double(v)),
            Tok::Str(s) => Expr::Lit(Value::str(s)),
            Tok::Date(d) => Expr::Lit(Value::Date(d)),
            Tok::Sym("(") => {
                let e = self.expr()?;
                self.expect_sym(")")?;
                Expr::Paren(Box::new(e))
            }
            Tok::Sym(".") => Expr::WithMember(lower(&self.word()?)),
            // `Input(n, #1)`, `EOF(#1)`: a file's number.
            Tok::Sym("#") if !matches!(self.peek(), Tok::Sym("#")) => return self.primary(),
            Tok::Word(w) => match lower(&w).as_str() {
                "true" => Expr::Lit(Value::Bool(true)),
                "false" => Expr::Lit(Value::Bool(false)),
                "nothing" => Expr::Lit(Value::Nothing),
                "empty" => Expr::Lit(Value::Empty),
                "null" => Expr::Lit(Value::Null),
                "me" => Expr::Me,
                "not" => Expr::Not(Box::new(self.nested(|p| p.binary(5))?)),
                "new" => {
                    let (_, name) = self.type_name()?;
                    Expr::New(lower(&name))
                }
                "typeof" => {
                    let e = self.postfix()?;
                    self.expect_word("is")?;
                    let (_, name) = self.type_name()?;
                    Expr::TypeOf(Box::new(e), lower(&name))
                }
                "addressof" => return Err(self.err("AddressOf is not supported")),
                l => Expr::Name(l.to_string()),
            },
            _ => {
                self.i -= 1;
                return Err(self.err("Expected: expression"));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(code: &str) -> Module {
        parse_module("Module1", ModuleKind::Standard, code).unwrap()
    }

    #[test]
    fn deep_code_is_refused() {
        // Macros run with a large stack (as the app runs them); a debug
        // build's frames overflow a test thread's 2 MB on some systems.
        std::thread::Builder::new().stack_size(16 << 20).spawn(deep_code_checks).unwrap().join().unwrap();
    }

    fn deep_code_checks() {
        let deep = |open: &str, close: &str, n: usize| format!("Sub M()\n  x = {}1{}\nEnd Sub\n", open.repeat(n), close.repeat(n));
        assert!(parse_module("M", ModuleKind::Standard, &deep("(", ")", 20)).is_ok());
        assert!(parse_module("M", ModuleKind::Standard, &deep("(", ")", 50_000)).is_err());
        assert!(parse_module("M", ModuleKind::Standard, &deep("-", "", 50_000)).is_err());
        assert!(parse_module("M", ModuleKind::Standard, &deep("Not ", "", 50_000)).is_err());
        let chain = format!("Sub M()\n  x = 1{}\nEnd Sub\n", " + 1".repeat(50_000));
        assert!(parse_module("M", ModuleKind::Standard, &chain).is_err());
        let ifs = format!("Sub M()\n{}{}End Sub\n", "If x Then\n".repeat(20_000), "End If\n".repeat(20_000));
        assert!(parse_module("M", ModuleKind::Standard, &ifs).is_err());
    }

    #[test]
    fn procedures() {
        let m = module("Option Explicit\nPrivate count As Long\nPublic Const PI2 = 6.28\n\nSub Main()\n  Dim i As Integer, s As String\n  For i = 1 To 10: s = s & i: Next i\nEnd Sub\n\nPrivate Function Twice(ByVal x As Double, Optional y) As Double\n  Twice = x * 2\nEnd Function\n");
        assert!(m.explicit);
        assert_eq!(m.vars.len(), 1);
        assert_eq!(m.consts.len(), 1);
        assert_eq!(m.procs.len(), 2);
        assert_eq!(m.procs[1].params.len(), 2);
        assert!(m.procs[1].params[0].byval && m.procs[1].params[1].optional);
        assert!(!m.procs[1].public);
        assert_eq!(m.procs[0].body.len(), 2);
    }

    #[test]
    fn statements() {
        let m = module("Sub A()\nIf x Then y = 1 Else y = 2\nIf a > 1 Then\n b\nElseIf a < 0 Then\n c\nElse\n d\nEnd If\nSelect Case n\nCase 1, 2: z = 1\nCase 3 To 5\nCase Is > 9\nCase Else\nEnd Select\nDo While i < 3: i = i + 1: Loop\nDo\nLoop Until i > 5\nWhile False\nWend\nWith Range(\"A1\")\n .Value = 1\n .Font.Bold = True\nEnd With\nMsgBox (\"a\") & \"b\", vbOKOnly\nOn Error GoTo Fin\nFin:\nDebug.Print 1; 2\nEnd Sub");
        let body = &m.procs[0].body;
        assert!(matches!(body[0].stmt, Stmt::If { ref other, .. } if other.is_some()));
        assert!(matches!(body[1].stmt, Stmt::If { ref branches, .. } if branches.len() == 2));
        assert!(matches!(body[2].stmt, Stmt::Select { ref cases, ref other, .. } if cases.len() == 3 && other.is_some()));
        assert!(matches!(body[3].stmt, Stmt::Do { cond: Some(DoCond { pre: true, until: false, .. }), .. }));
        assert!(matches!(body[4].stmt, Stmt::Do { cond: Some(DoCond { pre: false, until: true, .. }), .. }));
        assert!(matches!(body[6].stmt, Stmt::With { ref body, .. } if body.len() == 2));
        match &body[7].stmt {
            Stmt::Call { target: Expr::Name(n), args } => {
                assert_eq!(n, "msgbox");
                assert_eq!(args.len(), 2);
                assert!(matches!(args[0].expr, Some(Expr::Bin(BinOp::Concat, ..))));
            }
            s => panic!("{s:?}"),
        }
        assert!(matches!(body[9].stmt, Stmt::Label(ref l) if l == "fin"));
        assert!(matches!(body[10].stmt, Stmt::Print(ref p) if p.len() == 3));
        assert_eq!(body[1].line, 3);
    }

    #[test]
    fn precedence() {
        let mut p = Parser { toks: tokenize("-2 ^ 2 + 3 * 4 = 10 And Not x", "M").unwrap(), i: 0, module: "M", single_if: 0, depth: 0, ops: 0, enums: Rc::default() };
        let e = p.expr().unwrap();
        // ((-(2^2) + (3*4)) = 10) And (Not x)
        let Expr::Bin(BinOp::And, l, r) = e else { panic!() };
        assert!(matches!(*r, Expr::Not(_)));
        let Expr::Bin(BinOp::Eq, l, _) = *l else { panic!() };
        let Expr::Bin(BinOp::Add, l, _) = *l else { panic!() };
        assert!(matches!(*l, Expr::Neg(_)));
    }

    #[test]
    fn errors_have_lines() {
        let e = parse_module("M", ModuleKind::Standard, "Sub A()\n  If x Then\n    y = 1\nEnd Sub\n").unwrap_err();
        assert_eq!(e.module, "M");
        assert!(e.message.contains("End If"), "{}", e.message);
        let e = parse_module("M", ModuleKind::Standard, "Sub A()\n  x = = 2\nEnd Sub\n").unwrap_err();
        assert_eq!(e.line, 2);
    }
}
