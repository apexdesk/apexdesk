//! Solver: optimizes a cell by changing variable cells under constraints.
//!
//! The model types describe cells; `Workbook::solve` turns them into
//! functions of the variables (by setting the cells and recalculating) and
//! calls [`run`]. Simplex LP builds the coefficient matrix by finite
//! differences, checks linearity at extra points and runs a dense simplex
//! with branch and bound for integer variables. Nonlinear uses an
//! augmented Lagrangian with projected BFGS (optionally multi-start).

use web_time::Duration;
use web_time::Instant;

use super::lp::{self, Cmp, Lp, LpResult};
use super::nlp::{self, Limits, NlpStatus};
use super::numeric::Rng;
use crate::addr::{CellRef, Range};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SolverGoal {
    Max,
    Min,
    /// Make the objective equal to this value.
    Value(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintOp {
    Le,
    Ge,
    Eq,
    /// The cells must be integers (the right side is ignored).
    Int,
    /// The cells must be 0 or 1.
    Bin,
}

/// `lhs op rhs`. The left side is a cell or range of the model's sheet;
/// the right side is a number, a reference (a single cell or a range the
/// size of `lhs`) or a formula, as typed in the dialog (without `=`, which
/// is also accepted).
#[derive(Clone, Debug, PartialEq)]
pub struct Constraint {
    pub lhs: Range,
    pub op: ConstraintOp,
    pub rhs: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SolverMethod {
    #[default]
    Nonlinear,
    SimplexLp,
}

/// A Solver model on one sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct SolverModel {
    pub objective: CellRef,
    pub goal: SolverGoal,
    /// Cells changed by Solver (they must hold constants).
    pub variables: Vec<Range>,
    pub constraints: Vec<Constraint>,
    pub method: SolverMethod,
    /// Variables without an explicit lower bound are kept >= 0.
    pub non_negative: bool,
    /// Seconds; 0 means no limit.
    pub max_time: f64,
    /// Iterations (simplex pivots, BFGS steps, branch and bound nodes).
    pub max_iter: usize,
    /// Constraint precision.
    pub precision: f64,
    /// Nonlinear only: also start from random points and keep the best.
    pub multistart: bool,
    pub seed: Option<u64>,
    /// Settings of other programs' Solvers kept as read, to be saved back:
    /// `solver_*` names we don't use, with their formula text (Excel
    /// syntax). Constraints we can't represent are kept as `solver_lhs`,
    /// `solver_rel` and `solver_rhs` (without a number).
    pub kept: Vec<(String, String)>,
}

impl Default for SolverModel {
    fn default() -> Self {
        SolverModel {
            objective: CellRef::default(),
            goal: SolverGoal::Max,
            variables: Vec::new(),
            constraints: Vec::new(),
            method: SolverMethod::Nonlinear,
            non_negative: true,
            max_time: 100.0,
            max_iter: 10_000,
            precision: 1e-6,
            multistart: false,
            seed: None,
            kept: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolverStatus {
    /// All constraints and optimality conditions are satisfied.
    Optimal,
    /// A feasible point where Solver could not improve further.
    Feasible,
    Infeasible,
    Unbounded,
    /// Stopped by the iteration or time limit (values: best point found).
    IterationLimit,
    /// Simplex LP was chosen but the model is not linear.
    NotLinear,
    /// The objective or a constraint shows an error value.
    ErrorValue,
    /// The model can't be set up (no variables, a variable holds a
    /// formula, a right side that doesn't parse, sizes that don't match).
    BadModel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SolverResult {
    pub status: SolverStatus,
    /// One value per variable cell, in the order of `variables` (each
    /// range row by row).
    pub values: Vec<f64>,
    /// Objective cell value at `values`.
    pub objective: f64,
}

impl SolverResult {
    /// Whether the values are worth keeping.
    pub fn is_solution(&self) -> bool {
        matches!(self.status, SolverStatus::Optimal | SolverStatus::Feasible)
    }
}

/// The model as functions of the variable vector.
pub(crate) struct Core {
    pub kinds: Vec<Cmp>,
    pub lb: Vec<f64>,
    pub ub: Vec<f64>,
    pub int: Vec<bool>,
    pub goal: SolverGoal,
    pub method: SolverMethod,
    pub max_iter: usize,
    pub deadline: Option<Instant>,
    pub precision: f64,
    pub multistart: bool,
    pub seed: Option<u64>,
}

impl Core {
    pub(crate) fn new(n: usize, m: &SolverModel) -> Core {
        let lb = if m.non_negative { 0.0 } else { f64::NEG_INFINITY };
        Core {
            kinds: Vec::new(),
            lb: vec![lb; n],
            ub: vec![f64::INFINITY; n],
            int: vec![false; n],
            goal: m.goal,
            method: m.method,
            max_iter: m.max_iter.max(1),
            // Always some limit (a file may ask for none), at most ten minutes.
            deadline: Some(Instant::now() + Duration::from_secs_f64(if m.max_time > 0.0 { m.max_time.min(600.0) } else { 100.0 })),
            precision: if m.precision > 0.0 { m.precision } else { 1e-6 },
            multistart: m.multistart,
            seed: m.seed,
        }
    }

    fn out_of_time(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }
}

/// `eval(x)` returns the objective cell value and the constraint
/// functions (`lhs - rhs`, compared with 0 by `core.kinds`).
pub(crate) fn run(core: &Core, eval: &mut nlp::Eval, x0: &[f64]) -> SolverResult {
    let n = x0.len();
    let target = match core.goal {
        SolverGoal::Value(v) => Some(v),
        _ => None,
    };
    let sign = if core.goal == SolverGoal::Max { -1.0 } else { 1.0 };
    let mut kinds = core.kinds.clone();
    if target.is_some() {
        kinds.push(Cmp::Eq);
    }
    // Minimization form: Max negates, Value(v) becomes f - v = 0.
    let mut min_eval = |x: &[f64]| -> Option<(f64, Vec<f64>)> {
        let (f, mut g) = eval(x)?;
        if !f.is_finite() {
            return None;
        }
        Some(match target {
            Some(v) => {
                g.push(f - v);
                (0.0, g)
            }
            None => (sign * f, g),
        })
    };
    let mut x0: Vec<f64> = x0.iter().enumerate().map(|(j, v)| clamp_start(*v, core.lb[j], core.ub[j])).collect();
    for j in 0..n {
        if core.int[j] {
            x0[j] = clamp_start(x0[j].round(), core.lb[j].ceil(), core.ub[j].floor());
        }
    }
    let outcome = match core.method {
        SolverMethod::SimplexLp => solve_linear(core, &kinds, &mut min_eval, &x0),
        SolverMethod::Nonlinear => solve_nonlinear(core, &kinds, &mut min_eval, &x0),
    };
    let (status, x) = match outcome {
        Ok(v) => v,
        Err(s) => return SolverResult { status: s, values: x0, objective: f64::NAN },
    };
    // Report the objective cell itself (not the minimization form).
    let objective = eval(&x).map_or(f64::NAN, |r| r.0);
    SolverResult { status, values: x, objective }
}

fn clamp_start(v: f64, lb: f64, ub: f64) -> f64 {
    let v = if v.is_finite() { v } else { 0.0 };
    if lb > ub { v } else { v.clamp(lb, ub) }
}

type Outcome = Result<(SolverStatus, Vec<f64>), SolverStatus>;

/// A relaxation result for branch and bound.
enum Relax {
    Opt(Vec<f64>, f64),
    /// Feasible but not proven optimal (nonlinear).
    Feasible(Vec<f64>, f64),
    Infeasible,
    Unbounded,
    Limit(Option<(Vec<f64>, f64)>),
    Error,
}

fn solve_linear(core: &Core, kinds: &[Cmp], eval: &mut nlp::Eval, x0: &[f64]) -> Outcome {
    let n = x0.len();
    let (f0, g0) = eval(x0).ok_or(SolverStatus::ErrorValue)?;
    let m = g0.len();
    // Coefficients by unit steps (exact for linear functions).
    let mut cols: Vec<(f64, Vec<f64>)> = Vec::with_capacity(n);
    let mut probe = x0.to_vec();
    for j in 0..n {
        probe[j] = x0[j] + 1.0;
        let (f1, g1) = eval(&probe).ok_or(SolverStatus::ErrorValue)?;
        probe[j] = x0[j];
        cols.push((f1 - f0, (0..m).map(|i| g1[i] - g0[i]).collect()));
    }
    // Linearity check at two other points.
    for k in 0..2 {
        let d: Vec<f64> = (0..n)
            .map(|j| {
                let r = 0.3 + ((j * 7 + k * 5 + 3) % 11) as f64 / 13.0;
                let s = if (j + k) % 2 == 0 { 1.0 } else { -1.0 };
                s * r * (1.0 + x0[j].abs())
            })
            .collect();
        let pt: Vec<f64> = (0..n).map(|j| x0[j] + d[j]).collect();
        let (f, g) = eval(&pt).ok_or(SolverStatus::NotLinear)?;
        let close = |actual: f64, base: f64, coef: &dyn Fn(usize) -> f64| {
            let pred = base + (0..n).map(|j| coef(j) * d[j]).sum::<f64>();
            let scale = 1.0 + actual.abs() + base.abs() + (0..n).map(|j| (coef(j) * d[j]).abs()).sum::<f64>();
            (actual - pred).abs() <= 1e-8 * scale
        };
        if !close(f, f0, &|j| cols[j].0) || (0..m).any(|i| !close(g[i], g0[i], &|j| cols[j].1[i])) {
            return Err(SolverStatus::NotLinear);
        }
    }
    let c: Vec<f64> = cols.iter().map(|c| c.0).collect();
    let mut rows = Vec::new();
    for i in 0..m {
        let a: Vec<f64> = cols.iter().map(|c| c.1[i]).collect();
        // g(x) = g0 + a.(x - x0)  cmp 0   ->   a.x cmp a.x0 - g0
        let b = a.iter().zip(x0).map(|(p, q)| p * q).sum::<f64>() - g0[i];
        if a.iter().all(|v| v.abs() < 1e-15) {
            let ok = match kinds[i] {
                Cmp::Le => g0[i] <= core.precision,
                Cmp::Ge => g0[i] >= -core.precision,
                Cmp::Eq => g0[i].abs() <= core.precision,
            };
            if !ok {
                return Err(SolverStatus::Infeasible);
            }
            continue;
        }
        rows.push((a, kinds[i], b));
    }
    let base = Lp { c, rows, lb: core.lb.clone(), ub: core.ub.clone() };
    let mut relax = |lb: &[f64], ub: &[f64]| -> Relax {
        let mut p = base.clone();
        p.lb = lb.to_vec();
        p.ub = ub.to_vec();
        match lp::solve_by(&p, 100_000, core.deadline) {
            LpResult::Optimal(x, v) => Relax::Opt(x, v),
            LpResult::Infeasible => Relax::Infeasible,
            LpResult::Unbounded => Relax::Unbounded,
            LpResult::IterationLimit => Relax::Limit(None),
        }
    };
    branch_and_bound(core, &mut relax)
}

fn solve_nonlinear(core: &Core, kinds: &[Cmp], eval: &mut nlp::Eval, x0: &[f64]) -> Outcome {
    let n = x0.len();
    let mut iters = 0usize;
    let lim = Limits { max_iter: core.max_iter, deadline: core.deadline, precision: core.precision };
    let mut rng = core.seed.map_or_else(Rng::from_time, Rng::new);
    let mut start = x0.to_vec();
    let mut relax = |lb: &[f64], ub: &[f64]| -> Relax {
        let starts = if core.multistart { 8 } else { 1 };
        let mut best: Option<(Vec<f64>, f64, NlpStatus)> = None;
        let mut last = Relax::Infeasible;
        for s in 0..starts {
            let from: Vec<f64> = if s == 0 {
                (0..n).map(|j| clamp_start(start[j], lb[j], ub[j])).collect()
            } else {
                (0..n)
                    .map(|j| {
                        let lo = if lb[j].is_finite() { lb[j] } else { start[j] - 10.0 * (1.0 + start[j].abs()) };
                        let hi = if ub[j].is_finite() { ub[j] } else { start[j] + 10.0 * (1.0 + start[j].abs()) };
                        lo + (hi - lo) * rng.uniform()
                    })
                    .collect()
            };
            let r = nlp::minimize(eval, &from, kinds, lb, ub, &lim, &mut iters);
            match r.status {
                NlpStatus::EvalError if s == 0 => return Relax::Error,
                NlpStatus::Unbounded => return Relax::Unbounded,
                NlpStatus::EvalError => continue,
                _ => {}
            }
            let feasible = r.violation <= core.precision * 10.0;
            if feasible && best.as_ref().is_none_or(|b| r.f < b.1) {
                best = Some((r.x.clone(), r.f, r.status));
            }
            if r.status == NlpStatus::Limit {
                last = Relax::Limit(best.as_ref().map(|b| (b.0.clone(), b.1)));
                break;
            }
        }
        match best {
            Some((x, f, st)) => {
                start = x.clone();
                match st {
                    NlpStatus::Converged => Relax::Opt(x, f),
                    NlpStatus::Limit => Relax::Limit(Some((x, f))),
                    _ => Relax::Feasible(x, f),
                }
            }
            None => last,
        }
    };
    branch_and_bound(core, &mut relax)
}

/// Depth-first branch and bound over the integer variables (a plain
/// relaxation solve when there are none).
fn branch_and_bound(core: &Core, relax: &mut dyn FnMut(&[f64], &[f64]) -> Relax) -> Outcome {
    let n = core.lb.len();
    let mut lb = core.lb.clone();
    let mut ub = core.ub.clone();
    for j in 0..n {
        if core.int[j] {
            lb[j] = lb[j].ceil();
            ub[j] = ub[j].floor();
        }
    }
    let tol = 1e-6;
    let mut best: Option<(Vec<f64>, f64, bool)> = None;
    let mut stack = vec![(lb, ub)];
    let mut nodes = 0usize;
    let mut limited = false;
    let mut unbounded = false;
    let mut error = false;
    while let Some((lb, ub)) = stack.pop() {
        nodes += 1;
        if nodes > core.max_iter || core.out_of_time() {
            limited = true;
            break;
        }
        // Integer bounds with no integer between them.
        if (0..n).any(|j| lb[j] > ub[j]) {
            continue;
        }
        let (x, f, proven) = match relax(&lb, &ub) {
            Relax::Opt(x, f) => (x, f, true),
            Relax::Feasible(x, f) => (x, f, false),
            Relax::Infeasible => continue,
            Relax::Unbounded => {
                unbounded = true;
                continue;
            }
            Relax::Error => {
                error = true;
                continue;
            }
            Relax::Limit(p) => {
                limited = true;
                if let Some((x, f)) = p {
                    if core.int.iter().all(|i| !i) && best.as_ref().is_none_or(|b| f < b.1) {
                        best = Some((x, f, false));
                    }
                }
                break;
            }
        };
        if best.as_ref().is_some_and(|b| f >= b.1 - 1e-9 * (1.0 + b.1.abs())) {
            continue;
        }
        // Most fractional integer variable.
        let frac = (0..n).filter(|&j| core.int[j]).map(|j| (j, (x[j] - x[j].round()).abs())).filter(|(_, d)| *d > tol).max_by(|a, b| a.1.total_cmp(&b.1));
        match frac {
            None => {
                let mut x = x;
                for j in 0..n {
                    if core.int[j] {
                        x[j] = x[j].round();
                    }
                }
                best = Some((x, f, proven));
            }
            Some((j, _)) => {
                let v = x[j];
                let (mut lo_ub, mut hi_lb) = (ub.clone(), lb.clone());
                lo_ub[j] = v.floor();
                hi_lb[j] = v.ceil();
                // Explore the nearer side first (pushed last).
                let down = (lb.clone(), lo_ub);
                let up = (hi_lb, ub.clone());
                if v - v.floor() < 0.5 {
                    stack.push(up);
                    stack.push(down);
                } else {
                    stack.push(down);
                    stack.push(up);
                }
            }
        }
    }
    match best {
        Some((x, _, proven)) => Ok((
            if limited {
                SolverStatus::IterationLimit
            } else if proven {
                SolverStatus::Optimal
            } else {
                SolverStatus::Feasible
            },
            x,
        )),
        None if error => Err(SolverStatus::ErrorValue),
        None if unbounded => Err(SolverStatus::Unbounded),
        None if limited => Err(SolverStatus::IterationLimit),
        None => Err(SolverStatus::Infeasible),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(n: usize, goal: SolverGoal, method: SolverMethod, kinds: &[Cmp]) -> Core {
        let m = SolverModel { goal, method, seed: Some(1), ..Default::default() };
        let mut c = Core::new(n, &m);
        c.kinds = kinds.to_vec();
        c
    }

    #[test]
    fn product_mix_lp() {
        // max 20a + 30b st a + 2b <= 40, 3a + b <= 45
        let c = core(2, SolverGoal::Max, SolverMethod::SimplexLp, &[Cmp::Le, Cmp::Le]);
        let mut f = |x: &[f64]| Some((20.0 * x[0] + 30.0 * x[1], vec![x[0] + 2.0 * x[1] - 40.0, 3.0 * x[0] + x[1] - 45.0]));
        let r = run(&c, &mut f, &[0.0, 0.0]);
        assert_eq!(r.status, SolverStatus::Optimal);
        assert!((r.values[0] - 10.0).abs() < 1e-9 && (r.values[1] - 15.0).abs() < 1e-9);
        assert!((r.objective - 650.0).abs() < 1e-9);
    }

    #[test]
    fn nonlinear_model_is_refused_by_simplex() {
        let c = core(1, SolverGoal::Min, SolverMethod::SimplexLp, &[]);
        let mut f = |x: &[f64]| Some((x[0] * x[0], vec![]));
        assert_eq!(run(&c, &mut f, &[1.0]).status, SolverStatus::NotLinear);
    }

    #[test]
    fn knapsack_binary() {
        // values 10, 13, 7, 8; weights 5, 7, 4, 3; capacity 10 -> items 1 & 3? (5+4=9 -> 17), 1&4 (8 -> 18), 2&4 (10 -> 21)
        let mut c = core(4, SolverGoal::Max, SolverMethod::SimplexLp, &[Cmp::Le]);
        c.int = vec![true; 4];
        c.ub = vec![1.0; 4];
        let (v, w) = ([10.0, 13.0, 7.0, 8.0], [5.0, 7.0, 4.0, 3.0]);
        let mut f = |x: &[f64]| Some(((0..4).map(|i| v[i] * x[i]).sum(), vec![(0..4).map(|i| w[i] * x[i]).sum::<f64>() - 10.0]));
        let r = run(&c, &mut f, &[0.0; 4]);
        assert_eq!(r.status, SolverStatus::Optimal);
        assert_eq!(r.values, vec![0.0, 1.0, 0.0, 1.0]);
        assert_eq!(r.objective, 21.0);
    }

    #[test]
    fn value_goal_and_infeasible() {
        let c = core(1, SolverGoal::Value(10.0), SolverMethod::Nonlinear, &[]);
        let mut f = |x: &[f64]| Some((x[0] * x[0] + 1.0, vec![]));
        let r = run(&c, &mut f, &[1.0]);
        assert!(r.is_solution(), "{r:?}");
        assert!((r.values[0] - 3.0).abs() < 1e-4, "{r:?}");
        let c = core(1, SolverGoal::Min, SolverMethod::SimplexLp, &[Cmp::Ge, Cmp::Le]);
        let mut f = |x: &[f64]| Some((x[0], vec![x[0] - 5.0, x[0] - 3.0]));
        assert_eq!(run(&c, &mut f, &[0.0]).status, SolverStatus::Infeasible);
        let c = core(1, SolverGoal::Max, SolverMethod::SimplexLp, &[]);
        let mut f = |x: &[f64]| Some((x[0], vec![]));
        assert_eq!(run(&c, &mut f, &[0.0]).status, SolverStatus::Unbounded);
    }

    #[test]
    fn integer_without_integer_in_bounds() {
        // 0.2 <= x <= 0.8 holds no integer: must not panic.
        for m in [SolverMethod::SimplexLp, SolverMethod::Nonlinear] {
            let mut c = core(1, SolverGoal::Min, m, &[]);
            c.int = vec![true];
            (c.lb, c.ub) = (vec![0.2], vec![0.8]);
            let mut f = |x: &[f64]| Some((x[0], vec![]));
            let r = run(&c, &mut f, &[0.5]);
            assert!(!r.is_solution(), "{m:?} {r:?}");
        }
    }

    #[test]
    fn integer_nonlinear() {
        // min (x - 2.6)^2 + (y - 1.2)^2 with integers -> (3, 1)
        let mut c = core(2, SolverGoal::Min, SolverMethod::Nonlinear, &[]);
        c.int = vec![true, true];
        let mut f = |x: &[f64]| Some(((x[0] - 2.6).powi(2) + (x[1] - 1.2).powi(2), vec![]));
        let r = run(&c, &mut f, &[0.0, 0.0]);
        assert!(r.is_solution(), "{r:?}");
        assert_eq!(r.values, vec![3.0, 1.0]);
    }
}
