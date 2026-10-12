//! Numeric helpers shared by the analysis tools: distributions (through the
//! spreadsheet functions, so results match the cells), basic statistics, a
//! seeded random generator and small dense linear algebra.

use std::sync::Arc;

use crate::addr::{CellRef, Range};
use crate::format::Locale;
use crate::formula::Expr;
use crate::functions::{Env, EvalContext, Func};
use crate::value::Value;

/// An evaluation context without cells, for calling pure functions.
struct NoCells;

impl EvalContext for NoCells {
    fn value(&self, _: usize, _: CellRef) -> Value {
        Value::Empty
    }
    fn sheet_index(&self, _: &str) -> Option<usize> {
        None
    }
    fn for_each(&self, _: usize, _: &Range, _: &mut dyn FnMut(CellRef, &Value)) {}
    fn used_extent(&self, _: usize) -> CellRef {
        CellRef::default()
    }
}

/// Calls a spreadsheet function (by its English name) on numbers and
/// booleans; NaN when it fails. Reusing the functions keeps the tools'
/// p-values and critical values identical to T.DIST, F.INV and friends.
pub(crate) fn call(name: &str, args: &[Value]) -> f64 {
    let Some(func) = Func::lookup(name) else { return f64::NAN };
    let args = args.iter().map(|v| Expr::Lit(v.clone())).collect();
    let e = Expr::Call { name: Arc::from(name), func: Some(func), args };
    match Env::new(&NoCells, 0, CellRef::default(), Locale::EN).eval(&e) {
        Value::Number(n) if n.is_finite() => n,
        _ => f64::NAN,
    }
}

fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|n| Value::Number(*n)).collect()
}

/// P(T > t) for Student's t with `df` degrees of freedom (any sign of t).
pub(crate) fn t_right(t: f64, df: f64) -> f64 {
    if !t.is_finite() {
        return if t > 0.0 { 0.0 } else { 1.0 };
    }
    1.0 - call("T.DIST", &[Value::Number(t), Value::Number(df), Value::Bool(true)])
}

/// Two-tailed critical t (T.INV.2T).
pub(crate) fn t_inv_2t(p: f64, df: f64) -> f64 {
    call("T.INV.2T", &nums(&[p, df]))
}

/// One-tailed upper critical t: P(T > t) = p.
pub(crate) fn t_inv_right(p: f64, df: f64) -> f64 {
    -call("T.INV", &nums(&[p, df]))
}

/// P(F > f) (F.DIST.RT).
pub(crate) fn f_right(f: f64, d1: f64, d2: f64) -> f64 {
    call("F.DIST.RT", &nums(&[f, d1, d2]))
}

/// F.DIST cumulative.
pub(crate) fn f_cdf(f: f64, d1: f64, d2: f64) -> f64 {
    call("F.DIST", &[Value::Number(f), Value::Number(d1), Value::Number(d2), Value::Bool(true)])
}

pub(crate) fn f_inv(p: f64, d1: f64, d2: f64) -> f64 {
    call("F.INV", &nums(&[p, d1, d2]))
}

pub(crate) fn f_inv_rt(p: f64, d1: f64, d2: f64) -> f64 {
    call("F.INV.RT", &nums(&[p, d1, d2]))
}

// ---------------------------------------------------------------------------
// Statistics on plain slices (callers drop missing values first).

pub(crate) fn mean(v: &[f64]) -> f64 {
    if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 }
}

/// Sum of squared deviations from the mean.
pub(crate) fn dev_sq(v: &[f64]) -> f64 {
    let m = mean(v);
    v.iter().map(|x| (x - m) * (x - m)).sum()
}

/// Sample variance (VAR.S).
pub(crate) fn var_s(v: &[f64]) -> f64 {
    if v.len() < 2 { f64::NAN } else { dev_sq(v) / (v.len() - 1) as f64 }
}

pub(crate) fn sorted(v: &[f64]) -> Vec<f64> {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s
}

pub(crate) fn median(v: &[f64]) -> f64 {
    let s = sorted(v);
    let n = s.len();
    if n == 0 {
        f64::NAN
    } else if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    }
}

/// MODE.SNGL: the most frequent value, the first one to reach that count;
/// None when no value repeats.
pub(crate) fn mode(v: &[f64]) -> Option<f64> {
    let mut best: Option<(f64, usize)> = None;
    let mut counts: Vec<(f64, usize)> = Vec::new();
    for &x in v {
        match counts.iter_mut().find(|(y, _)| *y == x) {
            Some(e) => e.1 += 1,
            None => counts.push((x, 1)),
        }
    }
    // Ties go to the value that appears first, like Excel.
    for &(x, c) in &counts {
        if c > 1 && best.is_none_or(|(_, b)| c > b) {
            best = Some((x, c));
        }
    }
    best.map(|b| b.0)
}

/// Sample excess kurtosis (KURT).
pub(crate) fn kurt(v: &[f64]) -> f64 {
    let n = v.len() as f64;
    if v.len() < 4 {
        return f64::NAN;
    }
    let m = mean(v);
    let s = var_s(v).sqrt();
    if s == 0.0 {
        return f64::NAN;
    }
    let sum4: f64 = v.iter().map(|x| ((x - m) / s).powi(4)).sum();
    n * (n + 1.0) / ((n - 1.0) * (n - 2.0) * (n - 3.0)) * sum4 - 3.0 * (n - 1.0) * (n - 1.0) / ((n - 2.0) * (n - 3.0))
}

/// Sample skewness (SKEW).
pub(crate) fn skew(v: &[f64]) -> f64 {
    let n = v.len() as f64;
    if v.len() < 3 {
        return f64::NAN;
    }
    let m = mean(v);
    let s = var_s(v).sqrt();
    if s == 0.0 {
        return f64::NAN;
    }
    let sum3: f64 = v.iter().map(|x| ((x - m) / s).powi(3)).sum();
    n / ((n - 1.0) * (n - 2.0)) * sum3
}

/// Pearson correlation of two equally long slices.
pub(crate) fn correl(a: &[f64], b: &[f64]) -> f64 {
    let (ma, mb) = (mean(a), mean(b));
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        sab += (x - ma) * (y - mb);
        saa += (x - ma) * (x - ma);
        sbb += (y - mb) * (y - mb);
    }
    sab / (saa * sbb).sqrt()
}

/// Population covariance (COVARIANCE.P).
pub(crate) fn covar_p(a: &[f64], b: &[f64]) -> f64 {
    let (ma, mb) = (mean(a), mean(b));
    a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum::<f64>() / a.len() as f64
}

// ---------------------------------------------------------------------------
// Random numbers (xoshiro256**, seeded through splitmix64).

#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
    spare: Option<f64>,
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut z = seed;
        let mut next = || {
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            x ^ (x >> 31)
        };
        Rng { s: [next(), next(), next(), next()], spare: None }
    }

    /// Seeded from the clock (no seed given).
    pub fn from_time() -> Rng {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x1234_5678);
        Rng::new(t)
    }

    pub fn next_u64(&mut self) -> u64 {
        let r = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        r
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform integer in 0..n (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        ((self.uniform() * n as f64) as usize).min(n.saturating_sub(1))
    }

    /// Standard normal (Box-Muller, keeping the second value).
    pub fn normal(&mut self) -> f64 {
        if let Some(z) = self.spare.take() {
            return z;
        }
        let u1 = 1.0 - self.uniform();
        let u2 = self.uniform();
        let r = (-2.0 * u1.ln()).sqrt();
        let t = std::f64::consts::TAU * u2;
        self.spare = Some(r * t.sin());
        r * t.cos()
    }

    pub fn bernoulli(&mut self, p: f64) -> f64 {
        if self.uniform() < p { 1.0 } else { 0.0 }
    }

    /// Binomial by inversion (walks the pmf from 0; fine for the sizes a
    /// sheet asks for, and exact).
    pub fn binomial(&mut self, n: u64, p: f64) -> f64 {
        if n == 0 || p <= 0.0 {
            return 0.0;
        }
        if p >= 1.0 {
            return n as f64;
        }
        // Use the smaller tail for speed and accuracy.
        let (q, flip) = if p > 0.5 { (1.0 - p, true) } else { (p, false) };
        if n > 10_000 {
            // Normal approximation for very large n.
            let m = n as f64 * q;
            let sd = (m * (1.0 - q)).sqrt();
            let k = (m + sd * self.normal()).round().clamp(0.0, n as f64);
            return if flip { n as f64 - k } else { k };
        }
        let u = self.uniform();
        let ratio = q / (1.0 - q);
        let mut pk = (1.0 - q).powf(n as f64);
        let mut cdf = pk;
        let mut k = 0u64;
        while u > cdf && k < n {
            pk *= ratio * (n - k) as f64 / (k + 1) as f64;
            k += 1;
            cdf += pk;
            if pk < 1e-300 && cdf > 1.0 - 1e-15 {
                break;
            }
        }
        if flip { (n - k) as f64 } else { k as f64 }
    }

    /// Poisson: Knuth's product method for small means, PTRS (Hörmann's
    /// transformed rejection) for large ones.
    pub fn poisson(&mut self, lambda: f64) -> f64 {
        if lambda <= 0.0 || !lambda.is_finite() {
            return 0.0;
        }
        if lambda < 30.0 {
            let l = (-lambda).exp();
            let mut k = 0.0;
            let mut p = 1.0;
            loop {
                p *= self.uniform();
                if p <= l {
                    return k;
                }
                k += 1.0;
            }
        }
        let slam = lambda.sqrt();
        let loglam = lambda.ln();
        let b = 0.931 + 2.53 * slam;
        let a = -0.059 + 0.02483 * b;
        let inv_alpha = 1.1239 + 1.1328 / (b - 3.4);
        let vr = 0.9277 - 3.6224 / (b - 2.0);
        loop {
            let u = self.uniform() - 0.5;
            let v = self.uniform();
            let us = 0.5 - u.abs();
            let k = ((2.0 * a / us + b) * u + lambda + 0.43).floor();
            if us >= 0.07 && v <= vr {
                return k;
            }
            if k < 0.0 || (us < 0.013 && v > us) {
                continue;
            }
            if (v * inv_alpha / (a / (us * us) + b)).ln() <= -lambda + k * loglam - ln_factorial(k) {
                return k;
            }
        }
    }
}

fn ln_factorial(k: f64) -> f64 {
    call("GAMMALN", &[Value::Number(k + 1.0)])
}

// ---------------------------------------------------------------------------
// Linear algebra.

/// A least squares fit: columns that are (nearly) dependent on earlier
/// ones are dropped with a zero coefficient, like LINEST does.
pub(crate) struct LeastSquares {
    pub coef: Vec<f64>,
    /// (X'X)^-1 over all columns (zero rows/cols for dropped ones).
    pub xtx_inv: Vec<Vec<f64>>,
    pub kept: Vec<bool>,
}

/// Fits `y ~ x` (`x` row-major, n x p) through the normal equations and a
/// Gauss-Jordan sweep; regressions from a sheet have few columns.
pub(crate) fn least_squares(x: &[Vec<f64>], y: &[f64]) -> Option<LeastSquares> {
    let n = x.len();
    let p = x.first().map_or(0, Vec::len);
    if n == 0 || p == 0 {
        return None;
    }
    let mut a = vec![vec![0.0; p]; p];
    let mut b = vec![0.0; p];
    for (row, yi) in x.iter().zip(y) {
        for i in 0..p {
            b[i] += row[i] * yi;
            for j in i..p {
                a[i][j] += row[i] * row[j];
            }
        }
    }
    for i in 0..p {
        for j in 0..i {
            a[i][j] = a[j][i];
        }
    }
    let diag: Vec<f64> = (0..p).map(|i| a[i][i]).collect();
    // Gauss-Jordan sweep with a relative pivot tolerance.
    let mut m = a.clone();
    let mut inv = vec![vec![0.0; p]; p];
    for (i, r) in inv.iter_mut().enumerate() {
        r[i] = 1.0;
    }
    let mut kept = vec![false; p];
    for k in 0..p {
        let piv = m[k][k];
        if !(piv.abs() > 1e-10 * diag[k].abs().max(1e-300)) || diag[k] == 0.0 {
            continue;
        }
        kept[k] = true;
        for j in 0..p {
            m[k][j] /= piv;
            inv[k][j] /= piv;
        }
        for i in 0..p {
            if i != k && m[i][k] != 0.0 {
                let f = m[i][k];
                for j in 0..p {
                    m[i][j] -= f * m[k][j];
                    inv[i][j] -= f * inv[k][j];
                }
            }
        }
    }
    // Dropped columns: zero rows and columns of the inverse.
    for i in 0..p {
        for j in 0..p {
            if !kept[i] || !kept[j] {
                inv[i][j] = 0.0;
            }
        }
    }
    let coef: Vec<f64> = (0..p).map(|i| if kept[i] { (0..p).map(|j| inv[i][j] * b[j]).sum() } else { 0.0 }).collect();
    Some(LeastSquares { coef, xtx_inv: inv, kept })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distributions_through_functions() {
        assert!((t_inv_2t(0.05, 10.0) - 2.228138852).abs() < 1e-6);
        assert!((t_right(2.228138852, 10.0) - 0.025).abs() < 1e-7);
        assert!((f_right(3.0, 2.0, 10.0) - 0.0954).abs() < 1e-3);
        assert!(call("NOPE", &[]).is_nan());
    }

    #[test]
    fn basic_stats() {
        let v = [3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0];
        assert_eq!(median(&v), 3.5);
        assert_eq!(mode(&v), Some(1.0));
        assert_eq!(mode(&[1.0, 2.0]), None);
        assert!((var_s(&v) - 7.553571429).abs() < 1e-8);
        assert!((kurt(&v) - 0.2766058).abs() < 1e-6);
        assert!((skew(&v) - 0.8335031).abs() < 1e-6);
    }

    #[test]
    fn rng_is_reproducible() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..10 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut r = Rng::new(7);
        let n = 20_000;
        let m: f64 = (0..n).map(|_| r.normal()).sum::<f64>() / n as f64;
        assert!(m.abs() < 0.05);
        let pm: f64 = (0..n).map(|_| r.poisson(50.0)).sum::<f64>() / n as f64;
        assert!((pm - 50.0).abs() < 0.5, "{pm}");
        let bm: f64 = (0..n).map(|_| r.binomial(20, 0.3)).sum::<f64>() / n as f64;
        assert!((bm - 6.0).abs() < 0.1, "{bm}");
    }

    #[test]
    fn least_squares_drops_collinear() {
        let x: Vec<Vec<f64>> = (0..5).map(|i| vec![1.0, i as f64, 2.0 * i as f64]).collect();
        let y: Vec<f64> = (0..5).map(|i| 1.0 + 3.0 * i as f64).collect();
        let ls = least_squares(&x, &y).unwrap();
        assert_eq!(ls.kept, vec![true, true, false]);
        assert!((ls.coef[0] - 1.0).abs() < 1e-9 && (ls.coef[1] - 3.0).abs() < 1e-9);
    }
}
