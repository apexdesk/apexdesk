//! Depuración > Compilar: what the VBA editor finds before running, beyond
//! syntax. Only certain errors: names declared twice, GoTo to a label the
//! procedure doesn't have and, with `Option Explicit`, assigning to a
//! variable never declared.

use std::collections::HashSet;

use crate::Project;
use crate::ast::{Expr, Line, Module, ModuleKind, OnError, ProcKind, ResumeKind, Stmt};
use crate::error::Error;

/// The errors of the project, in module and line order.
pub fn check(project: &Project) -> Vec<Error> {
    // Public names other modules can assign to.
    let mut globals = HashSet::new();
    for m in &project.modules {
        for p in &m.procs {
            globals.insert(p.name.to_lowercase());
        }
        if m.kind == ModuleKind::Standard {
            globals.extend(m.vars.iter().filter(|v| v.1).map(|v| v.0.name.to_lowercase()));
            globals.extend(m.consts.iter().filter(|c| c.2).map(|c| c.0.to_lowercase()));
        }
    }
    let mut out = Vec::new();
    for m in &project.modules {
        module(m, &globals, &mut out);
    }
    out
}

fn module(m: &Module, globals: &HashSet<String>, out: &mut Vec<Error>) {
    let err = |line: u32, msg: String| Error::syntax(&m.name, line, msg);
    let mut seen = HashSet::new();
    for (v, _) in &m.vars {
        if !seen.insert(v.name.to_lowercase()) {
            out.push(err(0, format!("Duplicate declaration in current scope: {}", v.name)));
        }
    }
    // A Property Get and its Let share a name.
    let mut procs: HashSet<(String, bool)> = HashSet::new();
    for p in &m.procs {
        if !procs.insert((p.name.to_lowercase(), p.kind == ProcKind::PropertyLet)) {
            out.push(err(p.line, format!("Ambiguous name detected: {}", p.name)));
        }
    }
    let module_names: HashSet<String> = m.vars.iter().map(|v| v.0.name.to_lowercase()).chain(m.consts.iter().map(|c| c.0.to_lowercase())).collect();
    for p in &m.procs {
        let mut labels = HashSet::new();
        let mut decls = Vec::new();
        walk(&p.body, &mut |l| match &l.stmt {
            Stmt::Label(n) => {
                labels.insert(n.to_lowercase());
            }
            Stmt::Dim { vars, .. } => decls.extend(vars.iter().map(|v| (v.name.to_lowercase(), l.line))),
            Stmt::Const(cs) => decls.extend(cs.iter().map(|c| (c.0.to_lowercase(), l.line))),
            _ => {}
        });
        let mut local: HashSet<String> = p.params.iter().map(|a| a.name.to_lowercase()).collect();
        for (name, line) in &decls {
            if !local.insert(name.clone()) {
                out.push(err(*line, format!("Duplicate declaration in current scope: {name}")));
            }
        }
        // ReDim declares an array too.
        walk(&p.body, &mut |l| {
            if let Stmt::ReDim { vars, .. } = &l.stmt {
                local.extend(vars.iter().map(|v| v.name.to_lowercase()));
            }
        });
        local.insert(p.name.to_lowercase());
        let declared = |n: &str| local.contains(n) || module_names.contains(n) || globals.contains(n);
        walk(&p.body, &mut |l| {
            let label = match &l.stmt {
                Stmt::GoTo(t) | Stmt::OnError(OnError::GoTo(t)) | Stmt::Resume(ResumeKind::Label(t)) => Some(t),
                _ => None,
            };
            // `GoTo 0`-like numbers that aren't labels are the parser's business.
            if let Some(t) = label.filter(|t| !labels.contains(&t.to_lowercase())) {
                out.push(err(l.line, format!("Label not defined: {t}")));
            }
            if m.explicit {
                let target = match &l.stmt {
                    Stmt::Assign { target: Expr::Name(n), .. } | Stmt::For { var: Expr::Name(n), .. } | Stmt::ForEach { var: Expr::Name(n), .. } => Some(n),
                    _ => None,
                };
                if let Some(n) = target.filter(|n| !declared(&n.to_lowercase())) {
                    out.push(err(l.line, format!("Variable not defined: {n}")));
                }
            }
        });
    }
}

/// Every statement, nested blocks too, in order.
fn walk(lines: &[Line], f: &mut impl FnMut(&Line)) {
    for l in lines {
        f(l);
        match &l.stmt {
            Stmt::If { branches, other } => {
                for (_, b) in branches {
                    walk(b, f);
                }
                if let Some(o) = other {
                    walk(o, f);
                }
            }
            Stmt::Select { cases, other, .. } => {
                for (_, b) in cases {
                    walk(b, f);
                }
                if let Some(o) = other {
                    walk(o, f);
                }
            }
            Stmt::For { body, .. } | Stmt::ForEach { body, .. } | Stmt::Do { body, .. } | Stmt::With { body, .. } => walk(body, f),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Project, Source};

    fn errors(code: &str) -> Vec<(u32, String)> {
        let p = Project::parse(&[Source { name: "Module1".into(), kind: crate::ModuleKind::Standard, code: code.into() }]).unwrap();
        super::check(&p).into_iter().map(|e| (e.line, e.message)).collect()
    }

    #[test]
    fn finds_what_vba_finds() {
        assert!(errors("Option Explicit\nPublic Total As Long\nConst K = 2\nSub A(x)\n    Dim i As Long\n    For i = 1 To 3\n        Total = Total + i * K\n    Next\n    x = 1\n    On Error GoTo Fail\n    Exit Sub\nFail:\n    Resume Next\nEnd Sub\nFunction F()\n    F = 1\nEnd Function\n").is_empty());
        let e = errors("Option Explicit\nSub A()\n    Dim i\n    Dim i\n    totl = 1\n    For j = 1 To 2\n    Next\n    GoTo Nowhere\nEnd Sub\nSub A()\nEnd Sub\n");
        let msgs: Vec<_> = e.iter().map(|(l, m)| format!("{l}: {m}")).collect();
        assert_eq!(msgs, ["10: Ambiguous name detected: A", "4: Duplicate declaration in current scope: i", "5: Variable not defined: totl", "6: Variable not defined: j", "8: Label not defined: nowhere"]);
        // Without Option Explicit, new names are fine.
        assert!(errors("Sub A()\n    totl = 1\nEnd Sub\n").is_empty());
    }
}
