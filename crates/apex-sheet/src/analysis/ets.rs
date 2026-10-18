//! Exponential triple smoothing: the additive (AAA) version of ETS behind
//! the FORECAST.ETS functions and Data > Forecast Sheet.
//!
//! A timeline with a regular step is built from the points (duplicates
//! aggregated, missing steps filled), the seasonality is detected from the
//! autocorrelation when not given, and the smoothing factors are the ones
//! with the least squared one-step error.

/// How missing points of the timeline are filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    /// The average of the neighbours (a straight line between them).
    Interpolate,
    Zero,
}

/// How values sharing a time are combined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aggregation {
    Average,
    Count,
    CountA,
    Max,
    Median,
    Min,
    Sum,
}

impl Aggregation {
    /// From the argument of the functions: 1 average ... 7 sum.
    pub fn from_code(n: f64) -> Option<Aggregation> {
        use Aggregation::*;
        Some(match n as i64 {
            1 => Average,
            2 => Count,
            3 => CountA,
            4 => Max,
            5 => Median,
            6 => Min,
            7 => Sum,
            _ => return None,
        })
    }

    fn apply(self, v: &[Option<f64>]) -> Option<f64> {
        let mut nums: Vec<f64> = v.iter().flatten().copied().collect();
        if nums.is_empty() && !matches!(self, Aggregation::Count | Aggregation::CountA) {
            return None;
        }
        let n = nums.len() as f64;
        Some(match self {
            Aggregation::Average => nums.iter().sum::<f64>() / n,
            Aggregation::Count => n,
            Aggregation::CountA => v.len() as f64,
            Aggregation::Max => nums.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            Aggregation::Min => nums.iter().copied().fold(f64::INFINITY, f64::min),
            Aggregation::Sum => nums.iter().sum(),
            Aggregation::Median => {
                nums.sort_by(f64::total_cmp);
                let k = nums.len() / 2;
                if nums.len() % 2 == 1 { nums[k] } else { (nums[k - 1] + nums[k]) / 2.0 }
            }
        })
    }
}

/// Why a series can't be forecast (the functions return #NUM!).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EtsError {
    /// Too few points, or no step between them.
    TooFew,
    /// The times are not on a regular step.
    Irregular,
}

/// Most points a filled timeline may have.
const MAX_POINTS: usize = 1_000_000;
/// Longest season considered.
pub const MAX_SEASON: usize = 8760;

/// Values on a regular timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Regular {
    pub start: f64,
    pub step: f64,
    pub values: Vec<f64>,
    /// A timeline of dates in months: `step` counts months and this is the
    /// day of each date (0 = the last of the month).
    pub months: Option<u32>,
}

/// The months since year 0 of a date serial.
fn month_index(t: f64) -> i64 {
    let (y, m, _) = crate::format::serial_to_date(t);
    y * 12 + m as i64 - 1
}

/// The date of month `m` (from `month_index`) on `day` (0 = the last).
fn month_date(m: i64, day: u32) -> f64 {
    let (y, mo) = (m.div_euclid(12), m.rem_euclid(12) + 1);
    if day == 0 { crate::format::date_to_serial(y, mo + 1, 1) - 1.0 } else { crate::format::date_to_serial(y, mo, day as i64) }
}

/// The common day of dates on a monthly timeline.
fn month_day(times: &[f64]) -> Option<u32> {
    if times.iter().any(|t| t.fract() != 0.0 || *t < 1.0) {
        return None;
    }
    let last = |t: f64| crate::format::serial_to_date(t + 1.0).2 == 1;
    if times.iter().all(|t| last(*t)) {
        return Some(0);
    }
    let d = crate::format::serial_to_date(times[0]).2;
    (d <= 28 && times.iter().all(|t| crate::format::serial_to_date(*t).2 == d)).then_some(d)
}

/// The step (the most common gap) and the values of the filled timeline.
fn fill(times: &[f64], vals: &[Option<f64>], completion: Completion) -> Result<(f64, Vec<f64>), EtsError> {
    let mut gaps: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_by(f64::total_cmp);
    let (mut step, mut best, mut k) = (gaps[0], 0, 0);
    while k < gaps.len() {
        let mut l = k;
        while l < gaps.len() && (gaps[l] - gaps[k]).abs() <= gaps[k].abs() * 1e-9 {
            l += 1;
        }
        if l - k > best {
            (best, step) = (l - k, gaps[k]);
        }
        k = l;
    }
    if !(step > 0.0) {
        return Err(EtsError::TooFew);
    }
    let mut slots: Vec<Option<f64>> = Vec::new();
    for (idx, (t, v)) in times.iter().zip(vals).enumerate() {
        let pos = (t - times[0]) / step;
        if (pos - pos.round()).abs() > 1e-6 {
            return Err(EtsError::Irregular);
        }
        let pos = pos.round() as usize;
        if pos >= MAX_POINTS {
            return Err(EtsError::Irregular);
        }
        if idx > 0 && pos <= slots.len().saturating_sub(1) {
            return Err(EtsError::Irregular);
        }
        slots.resize(pos, None);
        slots.push(*v);
    }
    // Up to 30% of the points may be missing, as in Excel.
    let missing = slots.iter().filter(|v| v.is_none()).count();
    if missing * 10 > slots.len() * 3 {
        return Err(EtsError::Irregular);
    }
    let values = match completion {
        Completion::Zero => slots.iter().map(|v| v.unwrap_or(0.0)).collect(),
        Completion::Interpolate => {
            let known: Vec<usize> = (0..slots.len()).filter(|&i| slots[i].is_some()).collect();
            if known.is_empty() {
                return Err(EtsError::TooFew);
            }
            (0..slots.len())
                .map(|i| {
                    if let Some(v) = slots[i] {
                        return v;
                    }
                    let after = known.partition_point(|&k| k < i);
                    match (after.checked_sub(1).map(|b| known[b]), known.get(after)) {
                        (Some(a), Some(&b)) => {
                            let (va, vb) = (slots[a].unwrap_or(0.0), slots[b].unwrap_or(0.0));
                            va + (vb - va) * (i - a) as f64 / (b - a) as f64
                        }
                        (Some(a), None) => slots[a].unwrap_or(0.0),
                        (None, Some(&b)) => slots[b].unwrap_or(0.0),
                        (None, None) => 0.0,
                    }
                })
                .collect()
        }
    };
    Ok((step, values))
}

impl Regular {
    /// Makes the timeline regular: sorted, duplicates aggregated, missing
    /// steps (and missing values) filled.
    pub fn new(points: &[(f64, Option<f64>)], completion: Completion, agg: Aggregation) -> Result<Regular, EtsError> {
        let mut pts: Vec<(f64, Option<f64>)> = points.iter().copied().filter(|p| p.0.is_finite()).collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Aggregate duplicates.
        let mut times: Vec<f64> = Vec::new();
        let mut vals: Vec<Option<f64>> = Vec::new();
        let mut i = 0;
        while i < pts.len() {
            let mut j = i;
            while j < pts.len() && pts[j].0 == pts[i].0 {
                j += 1;
            }
            let group: Vec<Option<f64>> = pts[i..j].iter().map(|p| p.1).collect();
            times.push(pts[i].0);
            vals.push(if j - i == 1 { group[0] } else { agg.apply(&group) });
            i = j;
        }
        if times.len() < 3 {
            return Err(EtsError::TooFew);
        }
        match fill(&times, &vals, completion) {
            Ok((step, values)) => Ok(Regular { start: times[0], step, values, months: None }),
            // Dates a whole number of months apart (the 1st, the 15th or the
            // last day of each month): the steps are months.
            Err(e) => {
                let day = month_day(&times).ok_or(e)?;
                let idx: Vec<f64> = times.iter().map(|t| month_index(*t) as f64).collect();
                let (step, values) = fill(&idx, &vals, completion)?;
                Ok(Regular { start: times[0], step, values, months: Some(day) })
            }
        }
    }

    /// The time of the last point.
    pub fn end(&self) -> f64 {
        self.time((self.values.len() - 1) as f64)
    }

    /// The time `k` steps after the start.
    pub fn time(&self, k: f64) -> f64 {
        match self.months {
            None => self.start + self.step * k,
            Some(day) => {
                let m = month_index(self.start) + (k * self.step).round() as i64;
                month_date(m, day)
            }
        }
    }

    /// Steps from the last point to `t` (fractional between points).
    pub fn ahead(&self, t: f64) -> f64 {
        match self.months {
            None => (t - self.end()) / self.step,
            Some(day) => {
                let m = month_index(t);
                let anchor = month_date(m, day);
                let len = month_date(m + 1, day) - anchor;
                let months = (m - month_index(self.end())) as f64 + (t - anchor) / len;
                months / self.step
            }
        }
    }
}

/// The season length found in `y`: the lag with the strongest
/// autocorrelation of the detrended values, or 1 (none).
pub fn detect_season(y: &[f64]) -> usize {
    let n = y.len();
    if n < 6 {
        return 1;
    }
    // Detrend with a straight line.
    let nf = n as f64;
    let mx = (nf - 1.0) / 2.0;
    let my = y.iter().sum::<f64>() / nf;
    let sxy: f64 = y.iter().enumerate().map(|(i, v)| (i as f64 - mx) * (v - my)).sum();
    let sxx: f64 = (0..n).map(|i| (i as f64 - mx).powi(2)).sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let r: Vec<f64> = y.iter().enumerate().map(|(i, v)| v - my - slope * (i as f64 - mx)).collect();
    let var: f64 = r.iter().map(|v| v * v).sum();
    if var <= 1e-12 * (1.0 + my * my) * nf {
        return 1;
    }
    let max_lag = (n / 2).min(MAX_SEASON);
    let acf: Vec<f64> = (0..=max_lag + 1).map(|k| if k >= n { 0.0 } else { (0..n - k).map(|i| r[i] * r[i + k]).sum::<f64>() / var }).collect();
    let mut best = (1, 0.3);
    for k in 2..=max_lag {
        let peak = acf[k] >= acf[k - 1] && acf[k] >= acf[k + 1];
        // A season must repeat at least twice in the data.
        if peak && acf[k] > best.1 + 1e-9 && n >= 2 * k {
            best = (k, acf[k]);
        }
    }
    best.0
}

/// A fitted model.
#[derive(Clone, Debug)]
pub struct Model {
    pub alpha: f64,
    pub beta: f64,
    pub gamma: f64,
    /// Season length; 1 = none.
    pub period: usize,
    level: f64,
    trend: f64,
    /// Seasonal terms by position in the season.
    season: Vec<f64>,
    /// Points of the series.
    n: usize,
    /// One-step errors and forecasts.
    errors: Vec<f64>,
    fitted: Vec<f64>,
    actual: Vec<f64>,
}

struct Run {
    sse: f64,
    level: f64,
    trend: f64,
    season: Vec<f64>,
    errors: Vec<f64>,
    fitted: Vec<f64>,
    actual: Vec<f64>,
}

fn run(y: &[f64], m: usize, a: f64, b: f64, g: f64) -> Run {
    let n = y.len();
    let (mut level, mut trend, mut season, start) = if m >= 2 {
        let first = y[..m].iter().sum::<f64>() / m as f64;
        let second = y[m..2 * m].iter().sum::<f64>() / m as f64;
        let trend = (second - first) / m as f64;
        let mid = (m as f64 - 1.0) / 2.0;
        let season: Vec<f64> = (0..m).map(|i| y[i] - (first + trend * (i as f64 - mid))).collect();
        (first + trend * mid, trend, season, m)
    } else {
        (y[0], y[1] - y[0], vec![0.0], 1)
    };
    let mut out = Run { sse: 0.0, level: 0.0, trend: 0.0, season: Vec::new(), errors: Vec::with_capacity(n), fitted: Vec::with_capacity(n), actual: Vec::with_capacity(n) };
    for (t, &yt) in y.iter().enumerate().skip(start) {
        let si = t % season.len();
        let s = if m >= 2 { season[si] } else { 0.0 };
        let pred = level + trend + s;
        let e = yt - pred;
        out.sse += e * e;
        out.errors.push(e);
        out.fitted.push(pred);
        out.actual.push(yt);
        let new_level = a * (yt - s) + (1.0 - a) * (level + trend);
        trend = b * (new_level - level) + (1.0 - b) * trend;
        if m >= 2 {
            season[si] = g * (yt - new_level) + (1.0 - g) * s;
        }
        level = new_level;
    }
    (out.level, out.trend, out.season) = (level, trend, season);
    out
}

/// Minimizes `f` over the unit box (Nelder-Mead, parameters clamped).
fn minimize(dim: usize, f: &dyn Fn(&[f64]) -> f64) -> Vec<f64> {
    let clamp = |x: &[f64]| x.iter().map(|v| v.clamp(0.0, 1.0)).collect::<Vec<f64>>();
    let start = [0.3, 0.1, 0.1];
    let mut simplex: Vec<Vec<f64>> = vec![start[..dim].to_vec()];
    for i in 0..dim {
        let mut p = start[..dim].to_vec();
        p[i] += 0.25;
        simplex.push(p);
    }
    let mut vals: Vec<f64> = simplex.iter().map(|p| f(&clamp(p))).collect();
    for _ in 0..400 {
        let mut order: Vec<usize> = (0..simplex.len()).collect();
        order.sort_by(|&i, &j| vals[i].total_cmp(&vals[j]));
        simplex = order.iter().map(|&i| simplex[i].clone()).collect();
        vals = order.iter().map(|&i| vals[i]).collect();
        if (vals[dim] - vals[0]).abs() <= 1e-10 * (1.0 + vals[0].abs()) {
            break;
        }
        let centroid: Vec<f64> = (0..dim).map(|k| simplex[..dim].iter().map(|p| p[k]).sum::<f64>() / dim as f64).collect();
        let along = |t: f64| -> Vec<f64> { (0..dim).map(|k| centroid[k] + t * (simplex[dim][k] - centroid[k])).collect() };
        let r = along(-1.0);
        let fr = f(&clamp(&r));
        if fr < vals[0] {
            let e = along(-2.0);
            let fe = f(&clamp(&e));
            (simplex[dim], vals[dim]) = if fe < fr { (e, fe) } else { (r, fr) };
        } else if fr < vals[dim - 1] {
            (simplex[dim], vals[dim]) = (r, fr);
        } else {
            let c = along(if fr < vals[dim] { -0.5 } else { 0.5 });
            let fc = f(&clamp(&c));
            if fc < vals[dim].min(fr) {
                (simplex[dim], vals[dim]) = (c, fc);
            } else {
                // Shrink toward the best point.
                for i in 1..=dim {
                    simplex[i] = (0..dim).map(|k| simplex[0][k] + 0.5 * (simplex[i][k] - simplex[0][k])).collect();
                    vals[i] = f(&clamp(&simplex[i]));
                }
            }
        }
    }
    let best = (0..simplex.len()).min_by(|&i, &j| vals[i].total_cmp(&vals[j])).unwrap_or(0);
    clamp(&simplex[best])
}

impl Model {
    /// Fits the series with the given season length (None: detected; 0
    /// or 1: none).
    pub fn fit(y: &[f64], season: Option<usize>) -> Result<Model, EtsError> {
        if y.len() < 3 {
            return Err(EtsError::TooFew);
        }
        let m = match season {
            None => detect_season(y),
            Some(s) => s.max(1),
        };
        // A season needs two full cycles to start from.
        let m = if m >= 2 && y.len() >= 2 * m + 1 { m } else { 1 };
        let dim = if m >= 2 { 3 } else { 2 };
        let best = minimize(dim, &|p: &[f64]| run(y, m, p[0], p[1], p.get(2).copied().unwrap_or(0.0)).sse);
        let (alpha, beta, gamma) = (best[0], best[1], best.get(2).copied().unwrap_or(0.0));
        let r = run(y, m, alpha, beta, gamma);
        Ok(Model { alpha, beta, gamma, period: m, level: r.level, trend: r.trend, season: r.season, n: y.len(), errors: r.errors, fitted: r.fitted, actual: r.actual })
    }

    /// The forecast `h` steps after the last point (fractions between
    /// steps are interpolated).
    pub fn forecast(&self, h: f64) -> f64 {
        let at = |k: usize| {
            let s = if self.period >= 2 { self.season[(self.n - 1 + k) % self.period] } else { 0.0 };
            self.level + self.trend * k as f64 + s
        };
        let h = h.max(0.0);
        let (k0, f) = (h.floor() as usize, h - h.floor());
        if f == 0.0 { at(k0) } else { at(k0) + (at(k0 + 1) - at(k0)) * f }
    }

    /// Standard deviation of the one-step errors.
    pub fn sigma(&self) -> f64 {
        let k = self.errors.len().max(1) as f64;
        (self.errors.iter().map(|e| e * e).sum::<f64>() / k).sqrt()
    }

    /// Half the width of the prediction interval `h` steps ahead, for the
    /// normal quantile `z`.
    pub fn interval(&self, h: f64, z: f64) -> f64 {
        let steps = h.ceil().max(1.0) as usize;
        let mut var = 1.0;
        for j in 1..steps.min(100_000) {
            let seasonal = if self.period >= 2 && j % self.period == 0 { self.gamma } else { 0.0 };
            let c = self.alpha * (1.0 + j as f64 * self.beta) + seasonal;
            var += c * c;
        }
        z * self.sigma() * var.sqrt()
    }

    /// FORECAST.ETS.STAT: 1 alpha, 2 beta, 3 gamma, 4 MASE, 5 SMAPE,
    /// 6 MAE, 7 RMSE (8, the step, comes from the timeline).
    pub fn stat(&self, k: u8) -> Option<f64> {
        let n = self.errors.len().max(1) as f64;
        let mae = self.errors.iter().map(|e| e.abs()).sum::<f64>() / n;
        Some(match k {
            1 => self.alpha,
            2 => self.beta,
            3 => self.gamma,
            4 => {
                let naive: Vec<f64> = self.actual.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
                let d = naive.iter().sum::<f64>() / naive.len().max(1) as f64;
                if d == 0.0 {
                    return None;
                }
                mae / d
            }
            5 => self.actual.iter().zip(&self.fitted).map(|(a, f)| if a.abs() + f.abs() == 0.0 { 0.0 } else { 2.0 * (a - f).abs() / (a.abs() + f.abs()) }).sum::<f64>() / n,
            6 => mae,
            7 => self.sigma(),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seasonal(n: usize) -> Vec<f64> {
        let pattern = [10.0, 14.0, 8.0, 12.0];
        (0..n).map(|i| 100.0 + 2.0 * i as f64 + pattern[i % 4]).collect()
    }

    #[test]
    fn regular_timelines() {
        let p = |t: f64, v: f64| (t, Some(v));
        // Unsorted, a duplicate averaged, a gap filled by interpolation.
        let r = Regular::new(&[p(3.0, 30.0), p(1.0, 10.0), p(2.0, 18.0), p(2.0, 22.0), p(5.0, 50.0), p(6.0, 60.0)], Completion::Interpolate, Aggregation::Average).unwrap();
        assert_eq!((r.start, r.step), (1.0, 1.0));
        assert_eq!(r.values, [10.0, 20.0, 30.0, 40.0, 50.0, 60.0]);
        let z = Regular::new(&[p(0.0, 1.0), p(7.0, 2.0), p(14.0, 3.0), p(28.0, 5.0), p(35.0, 6.0)], Completion::Zero, Aggregation::Sum).unwrap();
        assert_eq!((z.step, z.values.clone()), (7.0, vec![1.0, 2.0, 3.0, 0.0, 5.0, 6.0]));
        assert_eq!(z.end(), 35.0);
        assert_eq!(Regular::new(&[p(0.0, 1.0), p(1.0, 1.0), p(2.5, 1.0), p(3.0, 1.0)], Completion::Zero, Aggregation::Sum), Err(EtsError::Irregular));
        assert_eq!(Regular::new(&[p(0.0, 1.0), p(0.0, 2.0)], Completion::Zero, Aggregation::Sum), Err(EtsError::TooFew));
    }

    #[test]
    fn season_and_forecast() {
        let y = seasonal(24);
        assert_eq!(detect_season(&y), 4);
        assert_eq!(detect_season(&(0..20).map(|i| i as f64 * 3.0).collect::<Vec<_>>()), 1);
        let m = Model::fit(&y, None).unwrap();
        assert_eq!(m.period, 4);
        // The pattern goes on: next come positions 0, 1, 2, 3 of the season.
        for h in 1..=8 {
            let t = 23 + h;
            let want = 100.0 + 2.0 * t as f64 + [10.0, 14.0, 8.0, 12.0][t % 4];
            assert!((m.forecast(h as f64) - want).abs() < 0.5, "{h}: {} {want}", m.forecast(h as f64));
        }
        assert!(m.stat(6).unwrap() < 0.5 && m.stat(1).is_some());
        // Wider further ahead.
        assert!(m.interval(8.0, 1.96) >= m.interval(1.0, 1.96));
        // A straight line with no season.
        let line: Vec<f64> = (0..10).map(|i| 5.0 + 3.0 * i as f64).collect();
        let m = Model::fit(&line, Some(0)).unwrap();
        assert!((m.forecast(2.0) - (5.0 + 3.0 * 11.0)).abs() < 1e-6);
        assert!((m.forecast(1.5) - (5.0 + 3.0 * 10.5)).abs() < 1e-6);
    }
}
