//! Dense two-phase simplex for small linear programs (Solver's Simplex LP).
//!
//! Solves `min c.x` subject to rows `a.x (<=|>=|=) b` and bounds
//! `lb <= x <= ub` (either may be infinite). Bounds are handled by a change
//! of variables; the tableau is dense, which suits the few hundred
//! variables a sheet model has.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cmp {
    Le,
    Ge,
    Eq,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Lp {
    pub c: Vec<f64>,
    pub rows: Vec<(Vec<f64>, Cmp, f64)>,
    pub lb: Vec<f64>,
    pub ub: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LpResult {
    Optimal(Vec<f64>, f64),
    Infeasible,
    Unbounded,
    IterationLimit,
}

const EPS: f64 = 1e-9;

/// How an original variable maps onto the nonnegative ones.
#[derive(Clone, Copy)]
enum Map {
    /// x = lb + y
    Lower(f64, usize),
    /// x = ub - y
    Upper(f64, usize),
    /// x = y1 - y2
    Free(usize, usize),
}

thread_local! {
    /// When a solve under way must stop (see [`solve_by`]).
    static DEADLINE: std::cell::Cell<Option<web_time::Instant>> = const { std::cell::Cell::new(None) };
}

/// [`solve`], stopping at the iteration limit once `deadline` passes.
pub(crate) fn solve_by(lp: &Lp, max_iter: usize, deadline: Option<web_time::Instant>) -> LpResult {
    let outer = DEADLINE.replace(deadline);
    let r = solve(lp, max_iter);
    DEADLINE.set(outer);
    r
}

pub(crate) fn solve(lp: &Lp, max_iter: usize) -> LpResult {
    let n = lp.c.len();
    // Variables with lb > ub make the problem infeasible outright.
    if (0..n).any(|j| lp.lb[j] > lp.ub[j] + EPS) {
        return LpResult::Infeasible;
    }
    let mut maps = Vec::with_capacity(n);
    let mut ny = 0usize;
    let mut extra_rows: Vec<(usize, f64)> = Vec::new();
    for j in 0..n {
        let (l, u) = (lp.lb[j], lp.ub[j]);
        if l.is_finite() {
            maps.push(Map::Lower(l, ny));
            if u.is_finite() {
                extra_rows.push((ny, u - l));
            }
            ny += 1;
        } else if u.is_finite() {
            maps.push(Map::Upper(u, ny));
            ny += 1;
        } else {
            maps.push(Map::Free(ny, ny + 1));
            ny += 2;
        }
    }
    // Rewrites a row over x as a row over y (plus a constant moved to b).
    let convert = |a: &[f64]| -> (Vec<f64>, f64) {
        let mut row = vec![0.0; ny];
        let mut k = 0.0;
        for (j, m) in maps.iter().enumerate() {
            let aj = a.get(j).copied().unwrap_or(0.0);
            match *m {
                Map::Lower(l, y) => {
                    row[y] += aj;
                    k += aj * l;
                }
                Map::Upper(u, y) => {
                    row[y] -= aj;
                    k += aj * u;
                }
                Map::Free(p, q) => {
                    row[p] += aj;
                    row[q] -= aj;
                }
            }
        }
        (row, k)
    };
    let (cy, _) = convert(&lp.c);
    let mut rows: Vec<(Vec<f64>, Cmp, f64)> = lp
        .rows
        .iter()
        .map(|(a, cmp, b)| {
            let (r, k) = convert(a);
            (r, *cmp, b - k)
        })
        .collect();
    for (y, cap) in extra_rows {
        let mut r = vec![0.0; ny];
        r[y] = 1.0;
        rows.push((r, Cmp::Le, cap));
    }
    let y = match simplex(&cy, rows, max_iter) {
        Ok(y) => y,
        Err(r) => return r,
    };
    let x: Vec<f64> = maps
        .iter()
        .map(|m| match *m {
            Map::Lower(l, i) => l + y[i],
            Map::Upper(u, i) => u - y[i],
            Map::Free(p, q) => y[p] - y[q],
        })
        .collect();
    let obj = lp.c.iter().zip(&x).map(|(c, v)| c * v).sum();
    LpResult::Optimal(x, obj)
}

/// Two-phase tableau simplex on `min c.y, rows, y >= 0`.
fn simplex(c: &[f64], rows: Vec<(Vec<f64>, Cmp, f64)>, max_iter: usize) -> Result<Vec<f64>, LpResult> {
    let n = c.len();
    let m = rows.len();
    let n_slack = rows.iter().filter(|r| r.1 != Cmp::Eq).count();
    let n_art = rows.iter().filter(|r| (r.1 == Cmp::Ge && r.2 >= 0.0) || (r.1 == Cmp::Le && r.2 < 0.0) || r.1 == Cmp::Eq).count();
    let width = n + n_slack + n_art;
    let rhs = width;
    let mut t = vec![vec![0.0; width + 1]; m + 1];
    let mut basis = vec![0usize; m];
    let mut is_art = vec![false; width];
    let (mut s, mut a) = (n, n + n_slack);
    for (i, (coef, cmp, b)) in rows.into_iter().enumerate() {
        // Keep the right-hand side nonnegative.
        let flip = b < 0.0;
        let sign = if flip { -1.0 } else { 1.0 };
        for j in 0..n {
            t[i][j] = sign * coef.get(j).copied().unwrap_or(0.0);
        }
        t[i][rhs] = sign * b;
        let cmp = match (cmp, flip) {
            (Cmp::Le, true) => Cmp::Ge,
            (Cmp::Ge, true) => Cmp::Le,
            (c, _) => c,
        };
        match cmp {
            Cmp::Le => {
                t[i][s] = 1.0;
                basis[i] = s;
                s += 1;
            }
            Cmp::Ge => {
                t[i][s] = -1.0;
                s += 1;
                t[i][a] = 1.0;
                is_art[a] = true;
                basis[i] = a;
                a += 1;
            }
            Cmp::Eq => {
                t[i][a] = 1.0;
                is_art[a] = true;
                basis[i] = a;
                a += 1;
            }
        }
    }
    let mut iters = 0usize;
    // Phase 1: minimize the sum of artificials.
    if is_art.iter().any(|x| *x) {
        for j in 0..=width {
            t[m][j] = 0.0;
        }
        for i in 0..m {
            if is_art[basis[i]] {
                for j in 0..=width {
                    if j == width || !is_art[j] {
                        t[m][j] -= t[i][j];
                    }
                }
            }
        }
        let allowed = vec![true; width];
        match run(&mut t, &mut basis, &allowed, max_iter, &mut iters) {
            Step::Done => {}
            Step::Unbounded => return Err(LpResult::Infeasible),
            Step::Limit => return Err(LpResult::IterationLimit),
        }
        let scale = 1.0 + t.iter().take(m).map(|r| r[rhs].abs()).fold(0.0, f64::max);
        if -t[m][rhs] > 1e-7 * scale {
            return Err(LpResult::Infeasible);
        }
        // Drive zero-level artificials out of the basis.
        for i in 0..m {
            if is_art[basis[i]] {
                if let Some(j) = (0..width).find(|&j| !is_art[j] && t[i][j].abs() > 1e-9) {
                    pivot(&mut t, &mut basis, i, j);
                }
            }
        }
    }
    // Phase 2.
    for j in 0..=width {
        t[m][j] = if j < n { c[j] } else { 0.0 };
    }
    for i in 0..m {
        let b = basis[i];
        let cb = t[m][b];
        if cb != 0.0 {
            for j in 0..=width {
                t[m][j] -= cb * t[i][j];
            }
        }
    }
    let allowed: Vec<bool> = is_art.iter().map(|a| !a).collect();
    match run(&mut t, &mut basis, &allowed, max_iter, &mut iters) {
        Step::Done => {}
        Step::Unbounded => return Err(LpResult::Unbounded),
        Step::Limit => return Err(LpResult::IterationLimit),
    }
    let mut y = vec![0.0; n];
    for i in 0..m {
        if basis[i] < n {
            y[basis[i]] = t[i][rhs].max(0.0);
        }
    }
    Ok(y)
}

enum Step {
    Done,
    Unbounded,
    Limit,
}

fn run(t: &mut [Vec<f64>], basis: &mut [usize], allowed: &[bool], max_iter: usize, iters: &mut usize) -> Step {
    let m = basis.len();
    let rhs = t[0].len() - 1;
    let mut stalls = 0usize;
    loop {
        if *iters >= max_iter || (*iters % 16 == 0 && DEADLINE.get().is_some_and(|d| web_time::Instant::now() >= d)) {
            return Step::Limit;
        }
        *iters += 1;
        let obj = &t[m];
        // Dantzig's rule, switching to Bland's after degenerate pivots so
        // the method cannot cycle.
        let bland = stalls > 30;
        let mut enter = None;
        let mut best = -EPS;
        for j in 0..rhs {
            if allowed[j] && obj[j] < best {
                enter = Some(j);
                if bland {
                    break;
                }
                best = obj[j];
            }
        }
        let Some(j) = enter else { return Step::Done };
        let mut leave: Option<(usize, f64)> = None;
        for i in 0..m {
            let a = t[i][j];
            if a > EPS {
                let r = t[i][rhs] / a;
                match leave {
                    Some((li, lr)) if r > lr + 1e-12 || (r >= lr - 1e-12 && basis[i] >= basis[li]) => {}
                    _ => leave = Some((i, r)),
                }
            }
        }
        let Some((i, r)) = leave else { return Step::Unbounded };
        stalls = if r.abs() < 1e-12 { stalls + 1 } else { 0 };
        pivot(t, basis, i, j);
    }
}

fn pivot(t: &mut [Vec<f64>], basis: &mut [usize], r: usize, c: usize) {
    let p = t[r][c];
    for v in t[r].iter_mut() {
        *v /= p;
    }
    let prow = t[r].clone();
    for (i, row) in t.iter_mut().enumerate() {
        if i != r {
            let f = row[c];
            if f != 0.0 {
                for (v, pv) in row.iter_mut().zip(&prow) {
                    *v -= f * pv;
                }
                row[c] = 0.0;
            }
        }
    }
    basis[r] = c;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lp(c: &[f64], rows: &[(&[f64], Cmp, f64)], lb: f64) -> Lp {
        Lp { c: c.to_vec(), rows: rows.iter().map(|(a, k, b)| (a.to_vec(), *k, *b)).collect(), lb: vec![lb; c.len()], ub: vec![f64::INFINITY; c.len()] }
    }

    #[test]
    fn product_mix() {
        // max 3x + 5y st x <= 4, 2y <= 12, 3x + 2y <= 18 -> (2, 6), 36
        let p = lp(&[-3.0, -5.0], &[(&[1.0, 0.0], Cmp::Le, 4.0), (&[0.0, 2.0], Cmp::Le, 12.0), (&[3.0, 2.0], Cmp::Le, 18.0)], 0.0);
        match solve(&p, 1000) {
            LpResult::Optimal(x, v) => {
                assert!((x[0] - 2.0).abs() < 1e-9 && (x[1] - 6.0).abs() < 1e-9);
                assert!((v + 36.0).abs() < 1e-9);
            }
            r => panic!("{r:?}"),
        }
    }

    #[test]
    fn infeasible_unbounded_and_free() {
        let p = lp(&[1.0], &[(&[1.0], Cmp::Ge, 5.0), (&[1.0], Cmp::Le, 3.0)], 0.0);
        assert_eq!(solve(&p, 1000), LpResult::Infeasible);
        let p = lp(&[-1.0, 0.0], &[(&[1.0, -1.0], Cmp::Le, 1.0)], 0.0);
        assert_eq!(solve(&p, 1000), LpResult::Unbounded);
        // Free variables: min x st x >= -7 (as a row).
        let p = lp(&[1.0], &[(&[1.0], Cmp::Ge, -7.0)], f64::NEG_INFINITY);
        assert!(matches!(solve(&p, 1000), LpResult::Optimal(x, _) if (x[0] + 7.0).abs() < 1e-9));
        // Equalities and bounds.
        let mut p = lp(&[1.0, 2.0], &[(&[1.0, 1.0], Cmp::Eq, 10.0)], 1.0);
        p.ub = vec![6.0, 20.0];
        assert!(matches!(solve(&p, 1000), LpResult::Optimal(x, v) if (x[0] - 6.0).abs() < 1e-9 && (v - 14.0).abs() < 1e-9));
    }

    #[test]
    fn degenerate_does_not_cycle() {
        // Beale's classic cycling example.
        let p = lp(
            &[-0.75, 150.0, -0.02, 6.0],
            &[(&[0.25, -60.0, -0.04, 9.0], Cmp::Le, 0.0), (&[0.5, -90.0, -0.02, 3.0], Cmp::Le, 0.0), (&[0.0, 0.0, 1.0, 0.0], Cmp::Le, 1.0)],
            0.0,
        );
        assert!(matches!(solve(&p, 10_000), LpResult::Optimal(_, v) if (v + 0.05).abs() < 1e-9));
    }
}
