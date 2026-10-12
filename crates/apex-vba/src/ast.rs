//! Syntax tree of a VBA module: declarations, procedures, statements and
//! expressions. Names are kept lowercase (VBA ignores case) next to their
//! spelling where messages show them.

use std::rc::Rc;

use crate::value::{VType, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Pow,
    Mul,
    Div,
    IntDiv,
    Mod,
    Add,
    Sub,
    Concat,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Like,
    Is,
    And,
    Or,
    Xor,
    Eqv,
    Imp,
}

#[derive(Clone, Debug)]
pub struct Arg {
    /// `name:=value`.
    pub name: Option<String>,
    /// `None`: left out (`f(1, , 3)`).
    pub expr: Option<Expr>,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Lit(Value),
    /// A name (lowercase).
    Name(String),
    /// `obj.name`.
    Member(Box<Expr>, String),
    /// `.name` inside a With block.
    WithMember(String),
    /// `f(args)`, `a(i)`, `obj.m(args)`.
    Call(Box<Expr>, Vec<Arg>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// `(expr)`: passed ByVal even to a ByRef parameter.
    Paren(Box<Expr>),
    /// `New Class`.
    New(String),
    /// `TypeOf x Is Class`.
    TypeOf(Box<Expr>, String),
    Me,
}

#[derive(Clone, Debug)]
pub struct DimVar {
    pub name: String,
    /// Array bounds: `(lower, upper)`; `Some(empty)` is a dynamic array `x()`.
    pub bounds: Option<Vec<(Option<Expr>, Expr)>>,
    pub ty: VType,
    /// `As New Collection`.
    pub new: Option<String>,
}

#[derive(Clone, Debug)]
pub enum CaseTest {
    Value(Expr),
    Range(Expr, Expr),
    Is(BinOp, Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitKind {
    Sub,
    For,
    Do,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnError {
    Off,
    ResumeNext,
    GoTo(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeKind {
    Same,
    Next,
    Label(String),
}

#[derive(Clone, Debug)]
pub struct DoCond {
    /// `Until` (else `While`).
    pub until: bool,
    /// Tested before the body (`Do While x`) or after it (`Loop While x`).
    pub pre: bool,
    pub expr: Expr,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Dim { vars: Vec<DimVar>, is_static: bool },
    ReDim { preserve: bool, vars: Vec<DimVar> },
    Const(Vec<(String, Expr)>),
    Assign { target: Expr, value: Expr, set: bool },
    Call { target: Expr, args: Vec<Arg> },
    If { branches: Vec<(Expr, Vec<Line>)>, other: Option<Vec<Line>> },
    Select { expr: Expr, cases: Vec<(Vec<CaseTest>, Vec<Line>)>, other: Option<Vec<Line>> },
    For { var: Expr, from: Expr, to: Expr, step: Option<Expr>, body: Vec<Line> },
    ForEach { var: Expr, group: Expr, body: Vec<Line> },
    Do { cond: Option<DoCond>, body: Vec<Line> },
    With { obj: Expr, body: Vec<Line> },
    Exit(ExitKind),
    OnError(OnError),
    Resume(ResumeKind),
    GoTo(String),
    Label(String),
    Erase(Vec<Expr>),
    /// `Debug.Print a; b, c`: the parts and whether a `;`/`,` ended it.
    Print(Vec<Expr>, bool),
    /// `End` (stops everything) and `Stop`.
    End,
    /// Statements ApexDesk doesn't run (file I/O, Declare...): the reason.
    Unsupported(String),
}

/// A statement and the line it starts on.
#[derive(Clone, Debug)]
pub struct Line {
    pub line: u32,
    pub stmt: Stmt,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub byval: bool,
    pub optional: bool,
    pub paramarray: bool,
    pub default: Option<Expr>,
    pub ty: VType,
    /// `x()`.
    pub array: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcKind {
    Sub,
    Function,
    PropertyGet,
    PropertyLet,
}

#[derive(Clone, Debug)]
pub struct Proc {
    /// As written.
    pub name: String,
    pub kind: ProcKind,
    pub public: bool,
    pub params: Vec<Param>,
    pub ret: VType,
    pub body: Vec<Line>,
    pub line: u32,
    /// All locals are Static.
    pub is_static: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    Standard,
    /// ThisWorkbook and the sheets' modules.
    Document,
    Class,
    Form,
}

#[derive(Clone, Debug)]
pub struct Module {
    pub name: String,
    pub kind: ModuleKind,
    pub explicit: bool,
    /// Option Compare Text.
    pub compare_text: bool,
    /// Option Base.
    pub base: i64,
    /// Module-level variables: (declaration, public).
    pub vars: Vec<(DimVar, bool)>,
    /// Module-level constants (Enum members too): (name, value, public).
    pub consts: Vec<(String, Expr, bool)>,
    pub procs: Vec<Rc<Proc>>,
    /// Functions declared with `Declare` (lowercase): calling them fails.
    pub declares: Vec<String>,
}

impl Module {
    pub fn proc(&self, lname: &str) -> Option<&Rc<Proc>> {
        self.procs.iter().find(|p| p.name.eq_ignore_ascii_case(lname) && p.kind != ProcKind::PropertyLet)
    }
}
