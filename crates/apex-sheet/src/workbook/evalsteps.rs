//! Evaluate Formula: works out a formula one part at a time, as Excel's
//! dialog does. Each step evaluates the next part (innermost first, left
//! to right) and puts its value in its place; IF, IFERROR and IFNA only
//! evaluate the branch they take. References that are arguments of a
//! function stay as they are (the function reads the cells).

use super::*;
use crate::formula::Expr;

/// A formula being evaluated step by step.
#[derive(Clone, Debug)]
pub struct EvalSteps {
    pub sheet: usize,
    pub cell: CellRef,
    expr: Expr,
}

/// The formula as shown: text before the next part, the next part
/// (underlined in the dialog) and the text after it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvalView {
    pub before: String,
    pub next: String,
    pub after: String,
}

fn is_ref(e: &Expr) -> bool {
    matches!(e, Expr::Ref { .. } | Expr::Range { .. } | Expr::SpillRef { .. } | Expr::Structured(_))
}

/// A value put back into the formula.
fn literal(v: Value) -> Expr {
    match v {
        Value::Array(m) if m.rows * m.cols <= 1000 => Expr::Array((0..m.rows).map(|r| (0..m.cols).map(|c| Expr::Lit(m.get(r, c).clone())).collect()).collect()),
        v => Expr::Lit(v),
    }
}

fn is_done(e: &Expr) -> bool {
    match e {
        Expr::Number(_) | Expr::Text(_) | Expr::Bool(_) | Expr::Error(_) | Expr::Missing | Expr::Lit(_) => true,
        Expr::Array(rows) => rows.iter().flatten().all(is_done),
        _ => false,
    }
}

fn truth(e: &Expr) -> Option<bool> {
    match e {
        Expr::Lit(Value::Bool(b)) | Expr::Bool(b) => Some(*b),
        Expr::Lit(Value::Number(n)) | Expr::Number(n) => Some(*n != 0.0),
        Expr::Lit(Value::Empty) | Expr::Missing => Some(false),
        _ => None,
    }
}

fn children(e: &Expr) -> Vec<&Expr> {
    match e {
        Expr::Unary(_, a) | Expr::Percent(a) | Expr::Paren(a) => vec![a],
        Expr::Binary(_, a, b) => vec![a, b],
        Expr::Call { args, .. } => args.iter().collect(),
        _ => Vec::new(),
    }
}

fn child_mut(e: &mut Expr, i: usize) -> Option<&mut Expr> {
    match e {
        Expr::Unary(_, a) | Expr::Percent(a) | Expr::Paren(a) if i == 0 => Some(a),
        Expr::Binary(_, a, b) => match i {
            0 => Some(a),
            1 => Some(b),
            _ => None,
        },
        Expr::Call { args, .. } => args.get_mut(i),
        _ => None,
    }
}

/// The path (child indices) of the next part to evaluate.
fn next_path(e: &Expr) -> Option<Vec<usize>> {
    if is_done(e) {
        return None;
    }
    let step_into = |i: usize, c: &Expr| -> Option<Vec<usize>> {
        next_path(c).map(|mut p| {
            p.insert(0, i);
            p
        })
    };
    match e {
        Expr::Call { func: Some(f @ (Func::If | Func::IfError | Func::IfNa)), args, .. } => {
            let cond = args.first()?;
            if let Some(p) = step_into(0, cond) {
                return Some(p);
            }
            let branch = match f {
                Func::If => truth(cond).map(|t| if t { 1 } else { 2 }),
                Func::IfError => Some(if matches!(cond, Expr::Lit(Value::Error(_)) | Expr::Error(_)) { 1 } else { usize::MAX }),
                _ => Some(if matches!(cond, Expr::Lit(Value::Error(ErrorKind::Na)) | Expr::Error(ErrorKind::Na)) { 1 } else { usize::MAX }),
            };
            if let Some(b) = branch {
                if let Some(p) = args.get(b).and_then(|a| step_into(b, a)) {
                    return Some(p);
                }
            }
            Some(Vec::new())
        }
        Expr::Call { args, .. } => {
            for (i, a) in args.iter().enumerate() {
                if is_ref(a) {
                    continue;
                }
                if let Some(p) = step_into(i, a) {
                    return Some(p);
                }
            }
            Some(Vec::new())
        }
        _ => {
            for (i, c) in children(e).into_iter().enumerate() {
                if let Some(p) = step_into(i, c) {
                    return Some(p);
                }
            }
            Some(Vec::new())
        }
    }
}

fn node<'e>(e: &'e Expr, path: &[usize]) -> Option<&'e Expr> {
    path.iter().try_fold(e, |n, i| children(n).get(*i).copied())
}

fn node_mut<'e>(e: &'e mut Expr, path: &[usize]) -> Option<&'e mut Expr> {
    path.iter().try_fold(e, |n, i| child_mut(n, *i))
}

/// Formula text without the leading `=`.
fn text(e: &Expr, loc: &Locale) -> String {
    let t = e.to_display(loc);
    t.strip_prefix('=').map(str::to_string).unwrap_or(t)
}

impl EvalSteps {
    /// The formula shown with its next part marked.
    pub fn view(&self, loc: &Locale) -> EvalView {
        let Some(path) = next_path(&self.expr) else { return EvalView { before: self.result_text(loc), ..Default::default() } };
        let Some(target) = node(&self.expr, &path) else { return EvalView::default() };
        let next = text(target, loc);
        // Print the formula with a marker in place of the next part.
        let mut marked = self.expr.clone();
        if let Some(n) = node_mut(&mut marked, &path) {
            *n = Expr::Name(Arc::from("\u{E000}"));
        }
        let all = format!("={}", text(&marked, loc));
        match all.split_once('\u{E000}') {
            Some((a, b)) => EvalView { before: a.to_string(), next, after: b.to_string() },
            None => EvalView { before: all, ..Default::default() },
        }
    }

    fn result_text(&self, loc: &Locale) -> String {
        let t = text(&self.expr, loc);
        // Empty cells read as 0, like Excel shows them.
        if matches!(self.expr, Expr::Lit(Value::Empty)) { "0".to_string() } else { t }
    }

    /// Whether the whole formula has been evaluated.
    pub fn done(&self) -> bool {
        next_path(&self.expr).is_none()
    }

    /// The value, once done.
    pub fn result(&self, wb: &Workbook) -> Option<Value> {
        self.done().then(|| Env::new(wb, self.sheet, self.cell, wb.locale).eval(&self.expr))
    }

    /// Evaluates the next part.
    pub fn step(&mut self, wb: &Workbook) {
        let Some(path) = next_path(&self.expr) else { return };
        let Some(n) = node(&self.expr, &path) else { return };
        let v = Env::new(wb, self.sheet, self.cell, wb.locale).eval(n);
        if let Some(n) = node_mut(&mut self.expr, &path) {
            *n = literal(v);
        }
    }

    /// The cell with a formula that the next part refers to (Step In).
    pub fn step_in_target(&self, wb: &Workbook) -> Option<(usize, CellRef)> {
        let path = next_path(&self.expr)?;
        let Expr::Ref { sheet, addr } = node(&self.expr, &path)? else { return None };
        let s = match sheet {
            Some(name) => wb.sheet_index(name)?,
            None => self.sheet,
        };
        let c = CellRef::new(addr.row, addr.col);
        matches!(wb.sheets.get(s)?.get(c)?.content, Content::Formula { .. }).then_some((s, c))
    }

    /// Puts `v` in place of the next part (after stepping out of it).
    pub fn replace_next(&mut self, v: Value) {
        let Some(path) = next_path(&self.expr) else { return };
        if let Some(n) = node_mut(&mut self.expr, &path) {
            *n = literal(v);
        }
    }
}

impl Workbook {
    /// Starts evaluating the formula of a cell step by step.
    pub fn eval_steps(&self, sheet: usize, c: CellRef) -> Option<EvalSteps> {
        match &self.sheets.get(sheet)?.get(c)?.content {
            Content::Formula { expr, .. } => Some(EvalSteps { sheet, cell: c, expr: expr.expr().into_owned() }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(wb: &Workbook, at: &str) -> Vec<String> {
        let mut s = wb.eval_steps(0, CellRef::parse(at).unwrap()).unwrap();
        let mut out = Vec::new();
        for _ in 0..30 {
            let v = s.view(&wb.locale);
            out.push(if v.next.is_empty() { v.before } else { format!("{}[{}]{}", v.before, v.next, v.after) });
            if s.done() {
                break;
            }
            s.step(wb);
        }
        out
    }

    #[test]
    fn steps() {
        let mut wb = Workbook::new();
        wb.locale = Locale::EN;
        let c = |a: &str| CellRef::parse(a).unwrap();
        wb.set_input(0, c("A1"), "4");
        wb.set_input(0, c("A2"), "6");
        wb.set_input(0, c("A3"), "=A1*2");
        wb.set_input(0, c("B1"), "=(A1+A2)*2-SUM(A1:A2)");
        assert_eq!(run(&wb, "B1"), ["=([A1]+A2)*2-SUM(A1:A2)", "=(4+[A2])*2-SUM(A1:A2)", "=([4+6])*2-SUM(A1:A2)", "=[(10)]*2-SUM(A1:A2)", "=[10*2]-SUM(A1:A2)", "=20-[SUM(A1:A2)]", "=[20-10]", "10"]);
        // IF evaluates only the branch it takes.
        wb.set_input(0, c("B2"), "=IF(A1>5,A1/0,A2+1)");
        assert_eq!(run(&wb, "B2"), ["=IF([A1]>5,A1/0,A2+1)", "=IF([4>5],A1/0,A2+1)", "=IF(FALSE,A1/0,[A2]+1)", "=IF(FALSE,A1/0,[6+1])", "=[IF(FALSE,A1/0,7)]", "7"]);
        // Step in to a cell with a formula.
        wb.set_input(0, c("B3"), "=A3+1");
        let mut s = wb.eval_steps(0, c("B3")).unwrap();
        assert_eq!(s.step_in_target(&wb), Some((0, c("A3"))));
        s.replace_next(Value::Number(8.0));
        assert!(s.step_in_target(&wb).is_none());
        s.step(&wb);
        assert_eq!(s.result(&wb), Some(Value::Number(9.0)));
        assert!(wb.eval_steps(0, c("A1")).is_none());
    }
}
