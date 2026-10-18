//! Smooth nonlinear minimization for Solver's nonlinear method: an
//! augmented Lagrangian (PHR) outer loop over a projected BFGS inner loop,
//! with gradients by central finite differences. It finds local optima,
//! like Excel's GRG Nonlinear.

use web_time::Instant;

use super::lp::Cmp;

/// Evaluates the objective and the constraint functions at a point
/// (`None` when a cell shows an error).
pub(crate) type Eval<'a> = dyn FnMut(&[f64]) -> Option<(f64, Vec<f64>)> + 'a;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NlpStatus {
    /// Optimality and feasibility tolerances met.
    Converged,
    /// No further progress, but not proven optimal.
    Stalled,
    Limit,
    EvalError,
    Unbounded,
}

#[derive(Clone, Debug)]
pub(crate) struct NlpResult {
    pub status: NlpStatus,
    pub x: Vec<f64>,
    pub f: f64,
    /// Largest constraint violation at `x`.
    pub violation: f64,
}

pub(crate) struct Limits {
    pub max_iter: usize,
    pub deadline: Option<Instant>,
    /// Constraint precision (Excel's default is 1e-6).
    pub precision: f64,
}

impl Limits {
    pub(crate) fn out_of_time(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }
}

/// Largest violation of `g cmp 0`.
pub(crate) fn violation(kinds: &[Cmp], g: &[f64]) -> f64 {
    kinds
        .iter()
        .zip(g)
        .map(|(k, v)| match k {
            Cmp::Le => v.max(0.0),
            Cmp::Ge => (-v).max(0.0),
            Cmp::Eq => v.abs(),
        })
        .fold(0.0, f64::max)
}

struct Problem<'a, 'b> {
    eval: &'a mut Eval<'b>,
    lb: &'a [f64],
    ub: &'a [f64],
    evals: usize,
}

/// A point with its function values and Jacobian.
#[derive(Clone)]
struct Point {
    x: Vec<f64>,
    f: f64,
    g: Vec<f64>,
    /// Gradient of f, then of each g_i.
    jac: Vec<Vec<f64>>,
}

impl Problem<'_, '_> {
    fn project(&self, x: &mut [f64]) {
        for (j, v) in x.iter_mut().enumerate() {
            *v = v.max(self.lb[j]).min(self.ub[j]);
        }
    }

    fn value(&mut self, x: &[f64]) -> Option<(f64, Vec<f64>)> {
        self.evals += 1;
        let (f, g) = (self.eval)(x)?;
        (f.is_finite() && g.iter().all(|v| v.is_finite())).then_some((f, g))
    }

    /// Central differences, one-sided next to a bound.
    fn point(&mut self, x: Vec<f64>, f: f64, g: Vec<f64>) -> Option<Point> {
        let n = x.len();
        let m = g.len();
        let mut jac = vec![vec![0.0; n]; m + 1];
        let mut probe = x.clone();
        for j in 0..n {
            let h = 1e-6 * x[j].abs().max(1.0);
            let up = x[j] + h <= self.ub[j];
            let down = x[j] - h >= self.lb[j];
            let (a, b, span) = match (up, down) {
                (true, true) => (x[j] + h, x[j] - h, 2.0 * h),
                (true, false) => (x[j] + h, x[j], h),
                (false, true) => (x[j], x[j] - h, h),
                (false, false) => continue,
            };
            let mut at = |v: f64, this: &mut Self| -> Option<(f64, Vec<f64>)> {
                if v == x[j] {
                    return Some((f, g.clone()));
                }
                probe[j] = v;
                let r = this.value(&probe);
                probe[j] = x[j];
                r
            };
            let (fa, ga) = at(a, self)?;
            let (fb, gb) = at(b, self)?;
            jac[0][j] = (fa - fb) / span;
            for i in 0..m {
                jac[i + 1][j] = (ga[i] - gb[i]) / span;
            }
        }
        Some(Point { x, f, g, jac })
    }
}

/// Augmented Lagrangian parameters.
struct Al {
    lam: Vec<f64>,
    rho: f64,
}

impl Al {
    /// Multiplier weight of constraint i at value g (derivative of the
    /// penalty term with respect to g).
    fn weight(&self, k: Cmp, i: usize, g: f64) -> f64 {
        match k {
            Cmp::Eq => self.lam[i] + self.rho * g,
            Cmp::Le => (self.lam[i] + self.rho * g).max(0.0),
            Cmp::Ge => -(self.lam[i] - self.rho * g).max(0.0),
        }
    }

    fn value(&self, kinds: &[Cmp], f: f64, g: &[f64]) -> f64 {
        let mut l = f;
        for (i, (k, v)) in kinds.iter().zip(g).enumerate() {
            let lam = self.lam[i];
            l += match k {
                Cmp::Eq => lam * v + 0.5 * self.rho * v * v,
                Cmp::Le => ((lam + self.rho * v).max(0.0).powi(2) - lam * lam) / (2.0 * self.rho),
                Cmp::Ge => ((lam - self.rho * v).max(0.0).powi(2) - lam * lam) / (2.0 * self.rho),
            };
        }
        l
    }

    fn grad(&self, kinds: &[Cmp], p: &Point) -> Vec<f64> {
        let mut gr = p.jac[0].clone();
        for (i, k) in kinds.iter().enumerate() {
            let w = self.weight(*k, i, p.g[i]);
            if w != 0.0 {
                for (a, b) in gr.iter_mut().zip(&p.jac[i + 1]) {
                    *a += w * b;
                }
            }
        }
        gr
    }

    fn update(&mut self, kinds: &[Cmp], g: &[f64]) {
        for (i, (k, v)) in kinds.iter().zip(g).enumerate() {
            self.lam[i] = match k {
                Cmp::Eq => self.lam[i] + self.rho * v,
                Cmp::Le => (self.lam[i] + self.rho * v).max(0.0),
                Cmp::Ge => (self.lam[i] - self.rho * v).max(0.0),
            };
        }
    }
}

fn norm_inf(v: &[f64]) -> f64 {
    v.iter().fold(0.0, |m, x| m.max(x.abs()))
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Minimizes f subject to `g_i cmp 0` and bounds, from `x0`.
pub(crate) fn minimize(eval: &mut Eval, x0: &[f64], kinds: &[Cmp], lb: &[f64], ub: &[f64], lim: &Limits, iters: &mut usize) -> NlpResult {
    let n = x0.len();
    let mut pb = Problem { eval, lb, ub, evals: 0 };
    let mut x = x0.to_vec();
    pb.project(&mut x);
    let fail = |x: Vec<f64>, status| NlpResult { status, x, f: f64::NAN, violation: f64::INFINITY };
    let Some((f, g)) = pb.value(&x) else { return fail(x, NlpStatus::EvalError) };
    let Some(mut p) = pb.point(x, f, g) else { return fail(x0.to_vec(), NlpStatus::EvalError) };
    let tol = lim.precision.max(1e-12);
    let mut al = Al { lam: vec![0.0; kinds.len()], rho: 10.0 };
    let mut prev_viol = violation(kinds, &p.g);
    let mut status = NlpStatus::Stalled;
    let done = |p: Point, status| {
        let violation = violation(kinds, &p.g);
        NlpResult { status, x: p.x, f: p.f, violation }
    };
    for _outer in 0..60 {
        // Inner loop: projected BFGS on the augmented Lagrangian.
        let mut h: Vec<Vec<f64>> = identity(n);
        let mut fresh = true;
        let mut grad = al.grad(kinds, &p);
        let mut lval = al.value(kinds, p.f, &p.g);
        let mut converged = false;
        let mut stalls = 0;
        // Relative to the gradient at the start: a tolerance tied to the
        // objective's size would stop early on unbounded problems.
        let gtol = 1e-8 * norm_inf(&grad).max(1.0);
        loop {
            if *iters >= lim.max_iter || lim.out_of_time() {
                return done(p, NlpStatus::Limit);
            }
            *iters += 1;
            let pg: Vec<f64> = (0..n).map(|j| p.x[j] - (p.x[j] - grad[j]).max(lb[j]).min(ub[j])).collect();
            if norm_inf(&pg) <= gtol {
                converged = true;
                break;
            }
            let mut step = None;
            for attempt in 0..2 {
                if attempt == 1 {
                    if fresh {
                        break;
                    }
                    h = identity(n);
                    fresh = true;
                }
                let mut d: Vec<f64> = (0..n).map(|i| -dot(&h[i], &grad)).collect();
                for j in 0..n {
                    // Variables held at a bound stay there this step.
                    if (p.x[j] <= lb[j] && d[j] < 0.0) || (p.x[j] >= ub[j] && d[j] > 0.0) {
                        d[j] = 0.0;
                    }
                }
                if dot(&d, &grad) >= 0.0 {
                    d = (0..n).map(|j| if (p.x[j] <= lb[j] && grad[j] > 0.0) || (p.x[j] >= ub[j] && grad[j] < 0.0) { 0.0 } else { -grad[j] }).collect();
                    h = identity(n);
                    fresh = true;
                }
                let dn = norm_inf(&d);
                if dn == 0.0 {
                    break;
                }
                let mut alpha = if fresh { ((1.0 + norm_inf(&p.x)) / dn).min(1.0) } else { 1.0 };
                for _ in 0..40 {
                    let mut xn: Vec<f64> = (0..n).map(|j| p.x[j] + alpha * d[j]).collect();
                    pb.project(&mut xn);
                    if let Some((fn_, gn)) = pb.value(&xn) {
                        let ln = al.value(kinds, fn_, &gn);
                        let dec: f64 = (0..n).map(|j| grad[j] * (xn[j] - p.x[j])).sum();
                        if ln <= lval + 1e-4 * dec {
                            step = Some((xn, fn_, gn, ln));
                            break;
                        }
                    }
                    alpha *= 0.5;
                }
                if step.is_some() {
                    break;
                }
            }
            let Some((xn, fn_, gn, ln)) = step else { break };
            if fn_ < -1e30 || norm_inf(&xn) > 1e15 {
                let v = violation(kinds, &gn);
                return NlpResult { status: NlpStatus::Unbounded, x: xn, f: fn_, violation: v };
            }
            let Some(pn) = pb.point(xn, fn_, gn) else { return done(p, NlpStatus::EvalError) };
            let gn_ = al.grad(kinds, &pn);
            let s: Vec<f64> = (0..n).map(|j| pn.x[j] - p.x[j]).collect();
            let y: Vec<f64> = (0..n).map(|j| gn_[j] - grad[j]).collect();
            let sy = dot(&s, &y);
            if sy > 1e-12 * (dot(&s, &s) * dot(&y, &y)).sqrt() {
                if fresh {
                    // Shanno-Phua scaling of the first inverse Hessian.
                    let scale = sy / dot(&y, &y);
                    for (i, r) in h.iter_mut().enumerate() {
                        r[i] = scale;
                    }
                    fresh = false;
                }
                bfgs(&mut h, &s, &y, sy);
            }
            let small = (lval - ln).abs() <= 1e-14 * (1.0 + lval.abs()) && norm_inf(&s) <= 1e-12 * (1.0 + norm_inf(&pn.x));
            stalls = if small { stalls + 1 } else { 0 };
            p = pn;
            grad = gn_;
            lval = ln;
            if stalls >= 3 {
                break;
            }
        }
        let viol = violation(kinds, &p.g);
        if viol <= tol {
            if converged {
                status = NlpStatus::Converged;
                // One more multiplier update rarely changes a feasible
                // optimum; stop here.
                if kinds.is_empty() || al_stationary(&al, kinds, &p) {
                    break;
                }
            } else {
                status = NlpStatus::Stalled;
            }
        }
        if kinds.is_empty() {
            status = if converged { NlpStatus::Converged } else { NlpStatus::Stalled };
            break;
        }
        al.update(kinds, &p.g);
        if viol > 0.25 * prev_viol {
            al.rho = (al.rho * 10.0).min(1e12);
        }
        prev_viol = viol;
    }
    done(p, status)
}

/// Whether the multipliers already match the constraint values (the
/// update would not move them much).
fn al_stationary(al: &Al, kinds: &[Cmp], p: &Point) -> bool {
    kinds.iter().enumerate().all(|(i, k)| {
        let next = match k {
            Cmp::Eq => al.lam[i] + al.rho * p.g[i],
            Cmp::Le => (al.lam[i] + al.rho * p.g[i]).max(0.0),
            Cmp::Ge => (al.lam[i] - al.rho * p.g[i]).max(0.0),
        };
        (next - al.lam[i]).abs() <= 1e-4 * (1.0 + al.lam[i].abs())
    })
}

fn identity(n: usize) -> Vec<Vec<f64>> {
    (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect()
}

/// Inverse BFGS update: H = (I - r s y')H(I - r y s') + r s s'.
fn bfgs(h: &mut [Vec<f64>], s: &[f64], y: &[f64], sy: f64) {
    let n = s.len();
    let r = 1.0 / sy;
    let hy: Vec<f64> = (0..n).map(|i| dot(&h[i], y)).collect();
    let yhy = dot(y, &hy);
    for i in 0..n {
        for j in 0..n {
            h[i][j] += -r * (s[i] * hy[j] + hy[i] * s[j]) + (r * r * yhy + r) * s[i] * s[j];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim() -> Limits {
        Limits { max_iter: 5000, deadline: None, precision: 1e-6 }
    }

    #[test]
    fn rosenbrock() {
        let mut f = |x: &[f64]| Some(((1.0 - x[0]).powi(2) + 100.0 * (x[1] - x[0] * x[0]).powi(2), vec![]));
        let inf = f64::INFINITY;
        let mut it = 0;
        let r = minimize(&mut f, &[-1.2, 1.0], &[], &[-inf, -inf], &[inf, inf], &lim(), &mut it);
        assert_eq!(r.status, NlpStatus::Converged);
        assert!((r.x[0] - 1.0).abs() < 1e-4 && (r.x[1] - 1.0).abs() < 1e-4, "{:?}", r.x);
    }

    #[test]
    fn constrained_circle() {
        // min x + y st x^2 + y^2 <= 2 -> (-1, -1)
        let mut f = |x: &[f64]| Some((x[0] + x[1], vec![x[0] * x[0] + x[1] * x[1] - 2.0]));
        let inf = f64::INFINITY;
        let mut it = 0;
        let r = minimize(&mut f, &[0.5, 0.3], &[Cmp::Le], &[-inf, -inf], &[inf, inf], &lim(), &mut it);
        assert!((r.x[0] + 1.0).abs() < 1e-4 && (r.x[1] + 1.0).abs() < 1e-4, "{:?} {:?}", r.x, r.status);
        assert!(r.violation < 1e-5);
        // Bounds and an equality: min (x-3)^2 + (y-3)^2 st x + y = 2, x <= 0.5
        let mut f = |x: &[f64]| Some(((x[0] - 3.0).powi(2) + (x[1] - 3.0).powi(2), vec![x[0] + x[1] - 2.0]));
        let r = minimize(&mut f, &[0.0, 0.0], &[Cmp::Eq], &[-inf, -inf], &[0.5, inf], &lim(), &mut it);
        assert!((r.x[0] - 0.5).abs() < 1e-4 && (r.x[1] - 1.5).abs() < 1e-4, "{:?}", r.x);
    }

    #[test]
    fn errors_and_unbounded() {
        let mut f = |_: &[f64]| None;
        let mut it = 0;
        let r = minimize(&mut f, &[1.0], &[], &[0.0], &[2.0], &lim(), &mut it);
        assert_eq!(r.status, NlpStatus::EvalError);
        let mut f = |x: &[f64]| Some((-x[0], vec![]));
        let r = minimize(&mut f, &[1.0], &[], &[0.0], &[f64::INFINITY], &lim(), &mut it);
        assert_eq!(r.status, NlpStatus::Unbounded);
    }
}
