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
    /// The name after `As` when it isn't a built-in type (a class or a
    /// user-defined type), lowercase.
    pub ty_name: Option<String>,
    /// `String * n`.
    pub fixed: Option<u32>,
    /// `ReDim p.list(9)`: the fields after the variable, lowercase.
    pub fields: Vec<String>,
    /// `WithEvents`: its object's events run `name_Event` of this module.
    pub withevents: bool,
}

/// `Type Name ... End Type`.
#[derive(Clone, Debug)]
pub struct TypeDef {
    /// As written.
    pub name: String,
    pub fields: Vec<DimVar>,
    pub public: bool,
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
    /// `GoSub label`: runs from the label until `Return`.
    GoSub(String),
    Return,
    /// `LSet s = x` (`right`: RSet): `x` aligned in the length `s` has.
    Align { target: Expr, value: Expr, right: bool },
    Label(String),
    Erase(Vec<Expr>),
    /// `RaiseEvent Name(args)` in a class module.
    RaiseEvent(String, Vec<Arg>),
    /// `Debug.Print a; b, c`.
    Print(Vec<PrintItem>),
    /// `End`: stops everything.
    End,
    /// `Stop`: pauses in the debugger (ends the macro without one).
    Stop,
    /// `Debug.Assert expr`: pauses in the debugger when False.
    Assert(Expr),
    /// `Open`, `Print #`, `Close`... (see [`FileStmt`]).
    File(FileStmt),
    /// Statements ApexDesk doesn't run (Declare...): the reason.
    Unsupported(String),
}

/// How `Open` opens a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileMode {
    Input,
    Output,
    Append,
    Binary,
    Random,
}

/// A part of `Print #n, ...`.
#[derive(Clone, Debug)]
pub enum PrintItem {
    Expr(Expr),
    /// `,`: to the next print zone.
    Comma,
    /// `;`: right after.
    Semi,
    /// `Spc(n)`.
    Spc(Expr),
    /// `Tab(n)` (`Tab` alone: the next print zone).
    Tab(Option<Expr>),
}

/// The file statements; files are named by their number (`#1`).
#[derive(Clone, Debug)]
pub enum FileStmt {
    /// `Open path For mode [Access ...] [lock] As #n [Len = len]`; `read`/`write`
    /// are the Access clause (both false without one).
    Open { path: Expr, mode: FileMode, read: bool, write: bool, num: Expr, len: Option<Expr> },
    /// `Close #1, #2` (`Close` or `Reset` alone: all).
    Close(Vec<Expr>),
    Print { num: Expr, items: Vec<PrintItem> },
    /// `Write #n, a, b`: whether a `,`/`;` ended it.
    Write { num: Expr, items: Vec<Expr>, open: bool },
    Input { num: Expr, vars: Vec<Expr> },
    LineInput { num: Expr, var: Expr },
    /// `Get #n, [pos], var` (`put`: `Put`).
    Get { num: Expr, pos: Option<Expr>, var: Expr, put: bool },
    Seek { num: Expr, pos: Expr },
    /// `Name from As to`.
    Name { from: Expr, to: Expr },
    /// `Lock`, `Unlock`, `Width #`: nothing to do here.
    Nothing,
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
    /// The return type's name when it isn't built-in, lowercase.
    pub ret_name: Option<String>,
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
    /// Functions declared with `Declare`.
    pub declares: Vec<Declare>,
    /// User-defined types.
    pub types: Vec<TypeDef>,    /// The classes a class module implements (`Implements IShape`), lowercase.
    pub implements: Vec<String>,
}

/// `Declare Function Name Lib "lib" Alias "api"`, lowercase.
#[derive(Debug, Clone)]
pub struct Declare {
    pub name: String,
    pub lib: String,
    /// The alias, or the name when there is none.
    pub api: String,
}

impl Module {
    pub fn proc(&self, lname: &str) -> Option<&Rc<Proc>> {
        self.procs.iter().find(|p| p.name.eq_ignore_ascii_case(lname) && p.kind != ProcKind::PropertyLet)
    }
}
