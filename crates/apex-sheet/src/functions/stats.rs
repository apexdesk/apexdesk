//! Spreadsheet functions: statistics and probability distributions.

use std::collections::HashMap;
use std::f64::consts::PI;

use super::*;

/// Ranges at 0, 1, 3, 5...; criteria at 2, 4... (like SUMIFS).
const IFS_MASK: u64 = 0xAAAA_AAAA_AAAA_AAAB;

pub(super) fn array_params(f: Func) -> Option<u64> {
    use Func::*;
    Some(match f {
        AverageA | MaxA | MinA | Mode | ModeMult | StdevP | StdevA | StdevPA | VarP | VarA | VarPA | AveDev | DevSq | GeoMean | HarMean | Kurt | Skew | SkewP | Correl | Pearson | Rsq | Covar | CovarianceS | Slope | Intercept | Steyx | Frequency | ChisqTest | FTest => ALL,
        AverageIfs | MaxIfs | MinIfs => IFS_MASK,
        Percentile | PercentileExc | Quartile | QuartileExc | PercentRank | PercentRankExc | TrimMean | ZTest => 0b1,
        RankAvg => 0b10,
        Forecast | ForecastEts | ForecastEtsConfint => 0b110,
        ForecastEtsSeasonality | ForecastEtsStat => 0b11,
        Trend | Growth => 0b111,
        Linest | Logest | TTest | Prob => 0b11,
        _ => return None,
    })
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    // The statistics block of the catalogue is contiguous.
    let legacy = matches!(f, Func::BetaDistLegacy | Func::BetaInvLegacy | Func::HypgeomDistLegacy | Func::LognormDistLegacy);
    if !legacy && !(Func::AverageA as usize..=Func::Gauss as usize).contains(&(f as usize)) {
        return None;
    }
    Some(eval(env, f, a))
}

fn eval(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    use Func::*;
    let n = a.len();
    let need = |min: usize, max: usize| if n < min || n > max { Err(ErrorKind::Value) } else { Ok(()) };
    let num = |i: usize| env.num(&a[i]);
    // Counts and degrees of freedom are truncated to integers, like Excel.
    let int = |i: usize| env.num(&a[i]).map(f64::trunc);
    let flag = |i: usize| env.boolean(&a[i]);
    let prob_open = |p: f64| p > 0.0 && p < 1.0;
    let nums = |args: &[Expr]| -> R<Vec<f64>> {
        let mut v = Vec::new();
        env.numbers(args, &mut v)?;
        Ok(v)
    };
    Ok(match f {
        AverageA | MaxA | MinA | StdevA | StdevPA | VarA | VarPA => describe(f, &numbers_a(env, a)?)?,
        StdevP | VarP | AveDev | DevSq | GeoMean | HarMean | Kurt | Skew | SkewP => describe(f, &nums(a)?)?,
        AverageIfs | MaxIfs | MinIfs => {
            if n < 3 || n % 2 == 0 {
                return Err(ErrorKind::Value);
            }
            let (bs, br) = env.range_arg(&a[0])?;
            let conds = a[1..].chunks(2).map(|p| Ok((env.range_arg(&p[0])?, Criteria::new(env.scalar(&p[1]), &env.locale)))).collect::<R<Vec<_>>>()?;
            if conds.iter().any(|((_, r), _)| r.rows() != br.rows() || r.cols() != br.cols()) {
                return Err(ErrorKind::Value);
            }
            let mut v = Vec::new();
            for dr in 0..br.rows() {
                for dc in 0..br.cols() {
                    if conds.iter().all(|((s, r), c)| c.matches(&env.ctx.value(*s, CellRef::new(r.start.row + dr, r.start.col + dc)))) {
                        if let Value::Number(x) = env.ctx.value(bs, CellRef::new(br.start.row + dr, br.start.col + dc)) {
                            v.push(x);
                        }
                    }
                }
            }
            match f {
                AverageIfs if v.is_empty() => return Err(ErrorKind::Div0),
                AverageIfs => num_ok(v.iter().sum::<f64>() / v.len() as f64),
                _ if v.is_empty() => Value::Number(0.0),
                MaxIfs => Value::Number(v.iter().cloned().fold(f64::NEG_INFINITY, f64::max)),
                _ => Value::Number(v.iter().cloned().fold(f64::INFINITY, f64::min)),
            }
        }
        Mode | ModeMult => {
            let mut v = Vec::new();
            for e in a {
                v.extend(ordered_numbers(env, e)?);
            }
            // Distinct values in order of first appearance, with their counts.
            let mut seen: HashMap<u64, usize> = HashMap::new();
            let mut counts: Vec<(f64, usize)> = Vec::new();
            for x in v {
                let x = if x == 0.0 { 0.0 } else { x };
                let i = *seen.entry(x.to_bits()).or_insert_with(|| {
                    counts.push((x, 0));
                    counts.len() - 1
                });
                counts[i].1 += 1;
            }
            let top = counts.iter().map(|c| c.1).max().unwrap_or(0);
            if top < 2 {
                return Err(ErrorKind::Na);
            }
            let modes: Vec<Value> = counts.iter().filter(|c| c.1 == top).map(|c| Value::Number(c.0)).collect();
            if f == Mode {
                modes[0].clone()
            } else {
                Matrix::new(modes.len(), 1, modes).into_value()
            }
        }
        Percentile | PercentileExc | Quartile | QuartileExc => {
            need(2, 2)?;
            let mut v = nums(&a[..1])?;
            let k = num(1)?;
            let k = match f {
                Quartile if !(0.0..5.0).contains(&k) => return Err(ErrorKind::Num),
                QuartileExc if k.trunc() <= 0.0 || k.trunc() >= 4.0 => return Err(ErrorKind::Num),
                Quartile | QuartileExc => k.trunc() / 4.0,
                _ => k,
            };
            if v.is_empty() {
                return Err(ErrorKind::Num);
            }
            sort(&mut v);
            let len = v.len() as f64;
            let rank = if matches!(f, Percentile | Quartile) {
                if !(0.0..=1.0).contains(&k) {
                    return Err(ErrorKind::Num);
                }
                k * (len - 1.0)
            } else {
                let r = k * (len + 1.0) - 1.0;
                if !prob_open(k) || r < 0.0 || r > len - 1.0 {
                    return Err(ErrorKind::Num);
                }
                r
            };
            let i = rank.floor() as usize;
            num_ok(if i + 1 < v.len() { v[i] + (rank - i as f64) * (v[i + 1] - v[i]) } else { v[i] })
        }
        PercentRank | PercentRankExc => {
            need(2, 3)?;
            let mut v = nums(&a[..1])?;
            let x = num(1)?;
            let sig = env.opt_num(a, 2, 3.0)?.trunc();
            if sig < 1.0 || v.is_empty() {
                return Err(ErrorKind::Num);
            }
            sort(&mut v);
            if x < v[0] || x > v[v.len() - 1] {
                return Err(ErrorKind::Na);
            }
            // Fractional position of x in the sorted data.
            let lt = v.partition_point(|&y| y < x);
            let pos = if v[lt] == x { lt as f64 } else { (lt - 1) as f64 + (x - v[lt - 1]) / (v[lt] - v[lt - 1]) };
            let len = v.len() as f64;
            let r = match f {
                PercentRank if v.len() == 1 => 1.0,
                PercentRank => pos / (len - 1.0),
                _ => (pos + 1.0) / (len + 1.0),
            };
            // The result is truncated (not rounded) to `sig` digits.
            let p = 10f64.powi(sig.min(15.0) as i32);
            let y = r * p;
            Value::Number(if (y - y.round()).abs() < 1e-9 { y.round() } else { y.floor() } / p)
        }
        RankAvg => {
            need(2, 3)?;
            let x = num(0)?;
            let v = nums(&a[1..2])?;
            let asc = env.opt_num(a, 2, 0.0)? != 0.0;
            let ties = v.iter().filter(|&&y| y == x).count();
            if ties == 0 {
                return Err(ErrorKind::Na);
            }
            let better = v.iter().filter(|&&y| if asc { y < x } else { y > x }).count();
            Value::Number(better as f64 + (ties as f64 + 1.0) / 2.0)
        }
        TrimMean => {
            need(2, 2)?;
            let mut v = nums(&a[..1])?;
            let pct = num(1)?;
            if !(0.0..1.0).contains(&pct) || v.is_empty() {
                return Err(ErrorKind::Num);
            }
            // The number of excluded points is rounded down to an even count.
            let k = (v.len() as f64 * pct / 2.0).floor() as usize;
            sort(&mut v);
            let kept = &v[k..v.len() - k];
            num_ok(kept.iter().sum::<f64>() / kept.len() as f64)
        }
        Correl | Pearson | Rsq | Covar | CovarianceS | Slope | Intercept | Steyx | Forecast => {
            let (x0, first) = if f == Forecast {
                need(3, 3)?;
                (num(0)?, 1)
            } else {
                need(2, 2)?;
                (0.0, 0)
            };
            let (y, x) = pairs(env, &a[first], &a[first + 1])?;
            let len = y.len() as f64;
            if y.is_empty() {
                return Err(ErrorKind::Div0);
            }
            let (my, mx) = (mean(&y), mean(&x));
            let sxx: f64 = x.iter().map(|v| (v - mx).powi(2)).sum();
            let syy: f64 = y.iter().map(|v| (v - my).powi(2)).sum();
            let sxy: f64 = x.iter().zip(&y).map(|(u, v)| (u - mx) * (v - my)).sum();
            let r = match f {
                Covar => sxy / len,
                CovarianceS if y.len() < 2 => return Err(ErrorKind::Div0),
                CovarianceS => sxy / (len - 1.0),
                Correl | Pearson | Rsq if sxx == 0.0 || syy == 0.0 => return Err(ErrorKind::Div0),
                Correl | Pearson => sxy / (sxx * syy).sqrt(),
                Rsq => sxy * sxy / (sxx * syy),
                _ if sxx == 0.0 => return Err(ErrorKind::Div0),
                Slope => sxy / sxx,
                Intercept => my - sxy / sxx * mx,
                Forecast => my + sxy / sxx * (x0 - mx),
                _ if y.len() < 3 => return Err(ErrorKind::Div0),
                _ => ((syy - sxy * sxy / sxx).max(0.0) / (len - 2.0)).sqrt(),
            };
            num_ok(r)
        }
        ForecastEts | ForecastEtsConfint | ForecastEtsSeasonality | ForecastEtsStat => ets(env, f, a)?,
        Linest | Logest => {
            need(1, 4)?;
            let d = design(env, a, f == Logest)?;
            let konst = env.opt_bool(a, 2, true)?;
            let stats = env.opt_bool(a, 3, false)?;
            linest(&fit(&d, konst)?, konst, stats, f == Logest)
        }
        Trend | Growth => {
            need(1, 4)?;
            let d = design(env, a, f == Growth)?;
            let konst = env.opt_bool(a, 3, true)?;
            let fit = fit(&d, konst)?;
            let nm = match a.get(2) {
                None | Some(Expr::Missing) => d.xm.clone(),
                Some(e) => env.tab(e)?.to_matrix()?,
            };
            let nv = all_numbers(&nm)?;
            let k = d.xs.len();
            let predict = |xs: &mut dyn Iterator<Item = f64>| {
                let y = fit.b + xs.zip(&fit.coef).map(|(x, c)| x * c).sum::<f64>();
                num_ok(if f == Growth { y.exp() } else { y })
            };
            let out = if k == 1 {
                Matrix::new(nm.rows, nm.cols, nv.iter().map(|&x| predict(&mut std::iter::once(x))).collect())
            } else if !d.by_rows {
                if nm.cols != k {
                    return Err(ErrorKind::Ref);
                }
                Matrix::new(nm.rows, 1, nv.chunks(k).map(|row| predict(&mut row.iter().cloned())).collect())
            } else {
                if nm.rows != k {
                    return Err(ErrorKind::Ref);
                }
                Matrix::new(1, nm.cols, (0..nm.cols).map(|c| predict(&mut (0..k).map(|r| nv[r * nm.cols + c]))).collect())
            };
            out.into_value()
        }
        Frequency => {
            need(2, 2)?;
            let data = nums(&a[..1])?;
            let bins = ordered_numbers(env, &a[1])?;
            if bins.is_empty() {
                return Ok(Value::Number(data.len() as f64));
            }
            let mut order: Vec<usize> = (0..bins.len()).collect();
            order.sort_by(|&i, &j| bins[i].partial_cmp(&bins[j]).unwrap_or(Ordering::Equal));
            let sorted: Vec<f64> = order.iter().map(|&i| bins[i]).collect();
            let mut counts = vec![0usize; bins.len() + 1];
            for x in data {
                let j = sorted.partition_point(|&b| b < x);
                counts[if j < order.len() { order[j] } else { bins.len() }] += 1;
            }
            Matrix::new(counts.len(), 1, counts.into_iter().map(|c| Value::Number(c as f64)).collect()).into_value()
        }
        Standardize => {
            need(3, 3)?;
            let (x, m, s) = (num(0)?, num(1)?, num(2)?);
            if s <= 0.0 {
                return Err(ErrorKind::Num);
            }
            num_ok((x - m) / s)
        }
        NormDist | NormDistLegacy => {
            need(4, 4)?;
            let (x, m, s, c) = (num(0)?, num(1)?, num(2)?, flag(3)?);
            if s <= 0.0 {
                return Err(ErrorKind::Num);
            }
            let z = (x - m) / s;
            num_ok(if c { norm_cdf(z) } else { norm_pdf(z) / s })
        }
        NormSDist => {
            need(2, 2)?;
            let (z, c) = (num(0)?, flag(1)?);
            num_ok(if c { norm_cdf(z) } else { norm_pdf(z) })
        }
        NormSDistLegacy => num_ok(norm_cdf(env.num1(a)?)),
        NormInv | NormInvLegacy => {
            need(3, 3)?;
            let (p, m, s) = (num(0)?, num(1)?, num(2)?);
            if !prob_open(p) || s <= 0.0 {
                return Err(ErrorKind::Num);
            }
            num_ok(m + s * norm_inv(p))
        }
        NormSInv | NormSInvLegacy => {
            let p = env.num1(a)?;
            if !prob_open(p) {
                return Err(ErrorKind::Num);
            }
            num_ok(norm_inv(p))
        }
        LognormDist | LognormDistLegacy => {
            // Three arguments: the legacy LOGNORMDIST (cumulative).
            need(3, if f == LognormDist { 4 } else { 3 })?;
            let (x, m, s) = (num(0)?, num(1)?, num(2)?);
            let c = if n == 4 { flag(3)? } else { true };
            if x <= 0.0 || s <= 0.0 {
                return Err(ErrorKind::Num);
            }
            let z = (x.ln() - m) / s;
            num_ok(if c { norm_cdf(z) } else { norm_pdf(z) / (x * s) })
        }
        LognormInv => {
            need(3, 3)?;
            let (p, m, s) = (num(0)?, num(1)?, num(2)?);
            if !prob_open(p) || s <= 0.0 {
                return Err(ErrorKind::Num);
            }
            num_ok((m + s * norm_inv(p)).exp())
        }
        TDist => {
            need(3, 3)?;
            let (x, df, c) = (num(0)?, int(1)?, flag(2)?);
            if df < 1.0 {
                return Err(ErrorKind::Num);
            }
            num_ok(if c { t_upper(-x, df) } else { t_pdf(x, df) })
        }
        TDist2T | TDistRT => {
            need(2, 2)?;
            let (x, df) = (num(0)?, int(1)?);
            if df < 1.0 || (f == TDist2T && x < 0.0) {
                return Err(ErrorKind::Num);
            }
            num_ok(if f == TDist2T { 2.0 * t_upper(x, df) } else { t_upper(x, df) })
        }
        TDistLegacy => {
            need(3, 3)?;
            let (x, df, tails) = (num(0)?, int(1)?, int(2)?);
            if x < 0.0 || df < 1.0 || !(tails == 1.0 || tails == 2.0) {
                return Err(ErrorKind::Num);
            }
            num_ok(tails * t_upper(x, df))
        }
        TInv | TInv2T => {
            need(2, 2)?;
            let (p, df) = (num(0)?, int(1)?);
            if df < 1.0 || p <= 0.0 || p > 1.0 || (f == TInv && p == 1.0) {
                return Err(ErrorKind::Num);
            }
            // By symmetry, the lower quantile is minus the upper one.
            num_ok(if f == TInv { -t_inv_upper(p, df) } else { t_inv_upper(p / 2.0, df) })
        }
        TTest => t_test(env, a)?,
        ChisqDist => {
            need(3, 3)?;
            let (x, df, c) = (num(0)?, int(1)?, flag(2)?);
            if x < 0.0 || !(1.0..=1e10).contains(&df) {
                return Err(ErrorKind::Num);
            }
            num_ok(if c { gamma_pq(df / 2.0, x / 2.0).0 } else { gamma_pdf(x, df / 2.0, 2.0) })
        }
        ChisqDistRT => {
            need(2, 2)?;
            let (x, df) = (num(0)?, int(1)?);
            if x < 0.0 || !(1.0..=1e10).contains(&df) {
                return Err(ErrorKind::Num);
            }
            num_ok(gamma_pq(df / 2.0, x / 2.0).1)
        }
        ChisqInv | ChisqInvRT => {
            need(2, 2)?;
            let (p, df) = (num(0)?, int(1)?);
            if !(0.0..=1.0).contains(&p) || !(1.0..=1e10).contains(&df) {
                return Err(ErrorKind::Num);
            }
            num_ok(match f {
                ChisqInv if p == 0.0 => 0.0,
                ChisqInvRT if p == 1.0 => 0.0,
                _ if p == 0.0 || p == 1.0 => return Err(ErrorKind::Num),
                ChisqInv => solve_pos(p, true, df, |x| gamma_pq(df / 2.0, x / 2.0).0),
                _ => solve_pos(p, false, df, |x| gamma_pq(df / 2.0, x / 2.0).1),
            })
        }
        ChisqTest => chisq_test(env, a)?,
        FDist | FDistRT => {
            let arity = if f == FDist { 4 } else { 3 };
            need(arity, arity)?;
            let (x, d1, d2) = (num(0)?, int(1)?, int(2)?);
            let c = if f == FDist { flag(3)? } else { true };
            if x < 0.0 || d1 < 1.0 || d2 < 1.0 || d1 > 1e10 || d2 > 1e10 {
                return Err(ErrorKind::Num);
            }
            num_ok(match f {
                FDist if c => f_cdf(x, d1, d2).0,
                FDist => f_pdf(x, d1, d2),
                _ => f_cdf(x, d1, d2).1,
            })
        }
        FInv | FInvRT => {
            need(3, 3)?;
            let (p, d1, d2) = (num(0)?, int(1)?, int(2)?);
            if !(0.0..=1.0).contains(&p) || d1 < 1.0 || d2 < 1.0 || d1 > 1e10 || d2 > 1e10 {
                return Err(ErrorKind::Num);
            }
            num_ok(match f {
                FInv if p == 0.0 => 0.0,
                FInvRT if p == 1.0 => 0.0,
                _ if p == 0.0 || p == 1.0 => return Err(ErrorKind::Num),
                FInv => solve_pos(p, true, 1.0, |x| f_cdf(x, d1, d2).0),
                _ => solve_pos(p, false, 1.0, |x| f_cdf(x, d1, d2).1),
            })
        }
        FTest => {
            need(2, 2)?;
            let (u, v) = (nums(&a[..1])?, nums(&a[1..])?);
            if u.len() < 2 || v.len() < 2 {
                return Err(ErrorKind::Div0);
            }
            let (vu, vv) = (variance(&u), variance(&v));
            if vu == 0.0 || vv == 0.0 {
                return Err(ErrorKind::Div0);
            }
            let (lower, upper) = f_cdf(vu / vv, (u.len() - 1) as f64, (v.len() - 1) as f64);
            num_ok(2.0 * lower.min(upper))
        }
        BinomDist => {
            need(4, 4)?;
            let (k, trials, p, c) = (int(0)?, int(1)?, num(2)?, flag(3)?);
            if k < 0.0 || k > trials || !(0.0..=1.0).contains(&p) {
                return Err(ErrorKind::Num);
            }
            num_ok(if c { binom_cdf(k, trials, p) } else { binom_pmf(k, trials, p) })
        }
        BinomDistRange => {
            need(3, 4)?;
            let (trials, p, lo) = (int(0)?, num(1)?, int(2)?);
            let hi = if n > 3 { int(3)? } else { lo };
            if trials < 0.0 || !(0.0..=1.0).contains(&p) || lo < 0.0 || lo > trials || hi < lo || hi > trials {
                return Err(ErrorKind::Num);
            }
            let mut sum = 0.0;
            let mut k = lo;
            while k <= hi {
                sum += binom_pmf(k, trials, p);
                k += 1.0;
            }
            num_ok(sum.min(1.0))
        }
        BinomInv => {
            need(3, 3)?;
            let (trials, p, alpha) = (int(0)?, num(1)?, num(2)?);
            if trials < 0.0 || !(0.0..=1.0).contains(&p) || !(0.0..=1.0).contains(&alpha) {
                return Err(ErrorKind::Num);
            }
            // Smallest k whose cumulative probability reaches alpha.
            let (mut lo, mut hi) = (0.0, trials);
            while lo < hi {
                let mid = ((lo + hi) / 2.0).floor();
                let c = binom_cdf(mid, trials, p);
                if c.is_nan() {
                    return Err(ErrorKind::Num);
                }
                if c >= alpha * (1.0 - 1e-12) {
                    hi = mid;
                } else {
                    lo = mid + 1.0;
                }
            }
            Value::Number(lo)
        }
        PoissonDist => {
            need(3, 3)?;
            let (k, m, c) = (int(0)?, num(1)?, flag(2)?);
            if k < 0.0 || m < 0.0 {
                return Err(ErrorKind::Num);
            }
            num_ok(match c {
                _ if m == 0.0 => (c || k == 0.0) as u8 as f64,
                true => gamma_pq(k + 1.0, m).1,
                false => (xlogy(k, m) - m - ln_gamma(k + 1.0)).exp(),
            })
        }
        ExponDist => {
            need(3, 3)?;
            let (x, l, c) = (num(0)?, num(1)?, flag(2)?);
            if x < 0.0 || l <= 0.0 {
                return Err(ErrorKind::Num);
            }
            num_ok(if c { -(-l * x).exp_m1() } else { l * (-l * x).exp() })
        }
        GammaDist => {
            need(4, 4)?;
            let (x, al, be, c) = (num(0)?, num(1)?, num(2)?, flag(3)?);
            if x < 0.0 || al <= 0.0 || be <= 0.0 {
                return Err(ErrorKind::Num);
            }
            num_ok(if c { gamma_pq(al, x / be).0 } else { gamma_pdf(x, al, be) })
        }
        GammaInv => {
            need(3, 3)?;
            let (p, al, be) = (num(0)?, num(1)?, num(2)?);
            if !(0.0..1.0).contains(&p) || al <= 0.0 || be <= 0.0 {
                return Err(ErrorKind::Num);
            }
            num_ok(if p == 0.0 { 0.0 } else { be * solve_pos(p, true, al, |x| gamma_pq(al, x).0) })
        }
        Gamma => {
            let x = env.num1(a)?;
            if x <= 0.0 && x == x.floor() {
                return Err(ErrorKind::Num);
            }
            num_ok(gamma(x))
        }
        GammaLn => {
            let x = env.num1(a)?;
            if x <= 0.0 {
                return Err(ErrorKind::Num);
            }
            num_ok(ln_gamma(x))
        }
        BetaDist => {
            need(4, 6)?;
            let (x, al, be, c) = (num(0)?, num(1)?, num(2)?, flag(3)?);
            let (lo, hi) = (env.opt_num(a, 4, 0.0)?, env.opt_num(a, 5, 1.0)?);
            if al <= 0.0 || be <= 0.0 || x < lo || x > hi || lo == hi {
                return Err(ErrorKind::Num);
            }
            let w = hi - lo;
            let (t, u) = ((x - lo) / w, (hi - x) / w);
            num_ok(if c { beta_inc(t, u, al, be) } else { (xlogy(al - 1.0, t) + xlogy(be - 1.0, u) - ln_beta(al, be)).exp() / w })
        }
        BetaDistLegacy => {
            need(3, 5)?;
            let (x, al, be) = (num(0)?, num(1)?, num(2)?);
            let (lo, hi) = (env.opt_num(a, 3, 0.0)?, env.opt_num(a, 4, 1.0)?);
            if al <= 0.0 || be <= 0.0 || x < lo || x > hi || lo == hi {
                return Err(ErrorKind::Num);
            }
            num_ok(beta_inc((x - lo) / (hi - lo), (hi - x) / (hi - lo), al, be))
        }
        BetaInv | BetaInvLegacy => {
            need(3, 5)?;
            let (p, al, be) = (num(0)?, num(1)?, num(2)?);
            let (lo, hi) = (env.opt_num(a, 3, 0.0)?, env.opt_num(a, 4, 1.0)?);
            if p <= 0.0 || p > 1.0 || al <= 0.0 || be <= 0.0 || lo >= hi {
                return Err(ErrorKind::Num);
            }
            let t = bisect(0.0, 1.0, |x| beta_inc(x, 1.0 - x, al, be) < p);
            num_ok(lo + t * (hi - lo))
        }
        WeibullDist => {
            need(4, 4)?;
            let (x, al, be, c) = (num(0)?, num(1)?, num(2)?, flag(3)?);
            if x < 0.0 || al <= 0.0 || be <= 0.0 {
                return Err(ErrorKind::Num);
            }
            let z = (x / be).powf(al);
            num_ok(if c { -(-z).exp_m1() } else { al / be * (x / be).powf(al - 1.0) * (-z).exp() })
        }
        HypgeomDist | HypgeomDistLegacy => {
            // The legacy HYPGEOMDIST has no `cumulative` argument.
            let legacy = f == HypgeomDistLegacy;
            let m = if legacy { 4 } else { 5 };
            need(m, m)?;
            let (k, draws, succ, pop, c) = (int(0)?, int(1)?, int(2)?, int(3)?, !legacy && flag(4)?);
            let lo = (draws - pop + succ).max(0.0);
            if k < lo || k > draws.min(succ) || draws <= 0.0 || draws > pop || succ <= 0.0 || succ > pop {
                return Err(ErrorKind::Num);
            }
            let pmf = |i: f64| (ln_choose(succ, i) + ln_choose(pop - succ, draws - i) - ln_choose(pop, draws)).exp();
            if !c {
                return Ok(num_ok(pmf(k)));
            }
            // Sum from k away from the mode (terms shrink that way, so the
            // loop stops early); above the mode, through the complement.
            let (hi, rest) = (draws.min(succ), pop - succ - draws);
            let mode = ((draws + 1.0) * (succ + 1.0) / (pop + 2.0)).floor();
            let (mut i, below) = if k <= mode { (k, true) } else { (k + 1.0, false) };
            if i > hi {
                return Ok(Value::Number(1.0));
            }
            let mut t = pmf(i);
            let mut s = t;
            while t > s * 1e-17 && if below { i > lo } else { i < hi } {
                if below {
                    t *= i * (rest + i) / ((succ - i + 1.0) * (draws - i + 1.0));
                    i -= 1.0;
                } else {
                    t *= (succ - i) * (draws - i) / ((i + 1.0) * (rest + i + 1.0));
                    i += 1.0;
                }
                s += t;
            }
            num_ok(if below { s.min(1.0) } else { (1.0 - s).max(0.0) })
        }
        NegbinomDist => {
            // Three arguments: the legacy NEGBINOMDIST (probability mass).
            need(3, 4)?;
            let (fails, succ, p) = (int(0)?, int(1)?, num(2)?);
            let c = if n == 4 { flag(3)? } else { false };
            if fails < 0.0 || succ < 1.0 || !(0.0..=1.0).contains(&p) {
                return Err(ErrorKind::Num);
            }
            num_ok(if c {
                beta_inc(p, 1.0 - p, succ, fails + 1.0)
            } else {
                (ln_choose(fails + succ - 1.0, fails) + xlogy(succ, p) + xlog1py(fails, -p)).exp()
            })
        }
        ConfidenceNorm | ConfidenceT => {
            need(3, 3)?;
            let (alpha, sd, size) = (num(0)?, num(1)?, int(2)?);
            if !prob_open(alpha) || sd <= 0.0 || size < 1.0 {
                return Err(ErrorKind::Num);
            }
            let q = if f == ConfidenceNorm {
                -norm_inv(alpha / 2.0)
            } else if size == 1.0 {
                return Err(ErrorKind::Div0);
            } else {
                t_inv_upper(alpha / 2.0, size - 1.0)
            };
            num_ok(q * sd / size.sqrt())
        }
        ZTest => {
            need(2, 3)?;
            let v = nums(&a[..1])?;
            let x = num(1)?;
            let sigma = match a.get(2) {
                Some(e) if !matches!(e, Expr::Missing) => Some(env.num(e)?),
                _ => None,
            };
            let sigma = match sigma {
                _ if v.is_empty() => return Err(ErrorKind::Na),
                Some(s) => s,
                None if v.len() < 2 => return Err(ErrorKind::Div0),
                None => variance(&v).sqrt(),
            };
            if sigma <= 0.0 {
                return Err(if sigma == 0.0 { ErrorKind::Div0 } else { ErrorKind::Num });
            }
            num_ok(norm_cdf(-(mean(&v) - x) / (sigma / (v.len() as f64).sqrt())))
        }
        Fisher => {
            let x = env.num1(a)?;
            if x <= -1.0 || x >= 1.0 {
                return Err(ErrorKind::Num);
            }
            num_ok(x.atanh())
        }
        FisherInv => num_ok(env.num1(a)?.tanh()),
        Prob => {
            need(3, 4)?;
            let (xs, ps) = (env.tab(&a[0])?.to_matrix()?, env.tab(&a[1])?.to_matrix()?);
            if xs.data.len() != ps.data.len() {
                return Err(ErrorKind::Na);
            }
            let lo = num(2)?;
            let hi = match a.get(3) {
                Some(e) if !matches!(e, Expr::Missing) => env.num(e)?,
                _ => lo,
            };
            let (mut total, mut hit) = (0.0, 0.0);
            for (x, p) in xs.data.iter().zip(&ps.data) {
                match (x, p) {
                    (Value::Error(k), _) | (_, Value::Error(k)) => return Err(*k),
                    (Value::Number(x), Value::Number(p)) => {
                        if !(0.0..=1.0).contains(p) {
                            return Err(ErrorKind::Num);
                        }
                        total += p;
                        if lo <= *x && *x <= hi {
                            hit += p;
                        }
                    }
                    _ => {}
                }
            }
            if (total - 1.0).abs() > 1e-9 {
                return Err(ErrorKind::Num);
            }
            Value::Number(hit)
        }
        Phi => num_ok(norm_pdf(env.num1(a)?)),
        Gauss => {
            // Φ(z) - 0.5 = erf(z/√2)/2, without cancellation near 0.
            let z = env.num1(a)?;
            num_ok(0.5 * z.signum() * gamma_pq(0.5, z * z / 2.0).0)
        }
        _ => return Err(ErrorKind::Name),
    })
}

// --- data helpers -----------------------------------------------------------

/// Numbers for the A functions: in ranges and arrays text counts as 0 and
/// booleans as 0/1; values typed into the call are coerced.
fn numbers_a(env: &Env, args: &[Expr]) -> R<Vec<f64>> {
    let mut out = Vec::new();
    for e in args {
        let mut err = None;
        env.each(e, &mut |v, direct| {
            let x = match v {
                Value::Number(n) => *n,
                Value::Bool(b) => *b as u8 as f64,
                Value::Empty => return,
                Value::Text(_) if !direct => 0.0,
                v => match v.to_number() {
                    Ok(n) => n,
                    Err(k) => {
                        err.get_or_insert(k);
                        return;
                    }
                },
            };
            out.push(x);
        });
        if let Some(k) = err {
            return Err(k);
        }
    }
    Ok(out)
}

/// The numbers of one argument in row-major order (for MODE and FREQUENCY).
fn ordered_numbers(env: &Env, e: &Expr) -> R<Vec<f64>> {
    let (vals, direct) = match env.arg(e) {
        Arg::V(v) => (vec![v], true),
        Arg::R(s, r) => (range_matrix(env.ctx, s, &r)?.data, false),
        Arg::A(m) => (m.data.clone(), false),
    };
    let mut out = Vec::new();
    for v in vals {
        match v {
            Value::Number(x) => out.push(x),
            Value::Error(k) => return Err(k),
            Value::Empty => {}
            v if direct => out.push(v.to_number()?),
            _ => {}
        }
    }
    Ok(out)
}

/// Positions where both arrays hold numbers (arrays of different sizes: #N/A).
fn pairs(env: &Env, ea: &Expr, eb: &Expr) -> R<(Vec<f64>, Vec<f64>)> {
    let (ma, mb) = (env.tab(ea)?.to_matrix()?, env.tab(eb)?.to_matrix()?);
    if ma.data.len() != mb.data.len() {
        return Err(ErrorKind::Na);
    }
    let (mut u, mut v) = (Vec::new(), Vec::new());
    for (x, y) in ma.data.iter().zip(&mb.data) {
        match (x, y) {
            (Value::Error(k), _) | (_, Value::Error(k)) => return Err(*k),
            (Value::Number(x), Value::Number(y)) => {
                u.push(*x);
                v.push(*y);
            }
            _ => {}
        }
    }
    Ok((u, v))
}

/// Every element must be a number (regression inputs).
fn all_numbers(m: &Matrix) -> R<Vec<f64>> {
    m.data
        .iter()
        .map(|v| match v {
            Value::Number(x) => Ok(*x),
            Value::Error(k) => Err(*k),
            _ => Err(ErrorKind::Value),
        })
        .collect()
}

fn sort(v: &mut [f64]) {
    v.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Equal));
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// Sample variance (needs two or more values).
fn variance(v: &[f64]) -> f64 {
    let m = mean(v);
    v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64
}

/// Single-list statistics.
fn describe(f: Func, v: &[f64]) -> R<Value> {
    use Func::*;
    let n = v.len() as f64;
    let m = mean(v);
    let ss: f64 = v.iter().map(|x| (x - m).powi(2)).sum();
    // Standardized moment sum: Σ((x - m)/s)^k.
    let moment = |s: f64, k: i32| v.iter().map(|x| ((x - m) / s).powi(k)).sum::<f64>();
    Ok(num_ok(match f {
        MaxA | MinA if v.is_empty() => 0.0,
        MaxA => v.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        MinA => v.iter().cloned().fold(f64::INFINITY, f64::min),
        AverageA | StdevP | VarP | StdevPA | VarPA if v.is_empty() => return Err(ErrorKind::Div0),
        AverageA => m,
        StdevP | StdevPA => (ss / n).sqrt(),
        VarP | VarPA => ss / n,
        StdevA | VarA if v.len() < 2 => return Err(ErrorKind::Div0),
        StdevA => (ss / (n - 1.0)).sqrt(),
        VarA => ss / (n - 1.0),
        AveDev | DevSq | GeoMean | HarMean if v.is_empty() => return Err(ErrorKind::Num),
        AveDev => v.iter().map(|x| (x - m).abs()).sum::<f64>() / n,
        DevSq => ss,
        GeoMean | HarMean if v.iter().any(|&x| x <= 0.0) => return Err(ErrorKind::Num),
        GeoMean => (v.iter().map(|x| x.ln()).sum::<f64>() / n).exp(),
        HarMean => n / v.iter().map(|x| 1.0 / x).sum::<f64>(),
        _ if ss == 0.0 => return Err(ErrorKind::Div0),
        Kurt if v.len() < 4 => return Err(ErrorKind::Div0),
        Kurt => {
            let s = (ss / (n - 1.0)).sqrt();
            n * (n + 1.0) / ((n - 1.0) * (n - 2.0) * (n - 3.0)) * moment(s, 4) - 3.0 * (n - 1.0).powi(2) / ((n - 2.0) * (n - 3.0))
        }
        Skew | SkewP if v.len() < 3 => return Err(ErrorKind::Div0),
        Skew => n / ((n - 1.0) * (n - 2.0)) * moment((ss / (n - 1.0)).sqrt(), 3),
        _ => moment((ss / n).sqrt(), 3) / n,
    }))
}

fn t_test(env: &Env, a: &[Expr]) -> R<Value> {
    if a.len() != 4 {
        return Err(ErrorKind::Value);
    }
    let (tails, kind) = (env.num(&a[2])?.trunc(), env.num(&a[3])?.trunc());
    if !(tails == 1.0 || tails == 2.0) || !(1.0..=3.0).contains(&kind) {
        return Err(ErrorKind::Num);
    }
    let (t, df) = if kind == 1.0 {
        let (u, v) = pairs(env, &a[0], &a[1])?;
        let d: Vec<f64> = u.iter().zip(&v).map(|(x, y)| x - y).collect();
        if d.len() < 2 {
            return Err(ErrorKind::Div0);
        }
        let n = d.len() as f64;
        (mean(&d) / (variance(&d) / n).sqrt(), n - 1.0)
    } else {
        let (mut u, mut v) = (Vec::new(), Vec::new());
        env.numbers(&a[..1], &mut u)?;
        env.numbers(&a[1..2], &mut v)?;
        if u.len() < 2 || v.len() < 2 {
            return Err(ErrorKind::Div0);
        }
        let (n1, n2) = (u.len() as f64, v.len() as f64);
        let (v1, v2) = (variance(&u), variance(&v));
        let diff = mean(&u) - mean(&v);
        if kind == 2.0 {
            let pooled = ((n1 - 1.0) * v1 + (n2 - 1.0) * v2) / (n1 + n2 - 2.0);
            (diff / (pooled * (1.0 / n1 + 1.0 / n2)).sqrt(), n1 + n2 - 2.0)
        } else {
            // Welch: unequal variances, fractional degrees of freedom.
            let (s1, s2) = (v1 / n1, v2 / n2);
            (diff / (s1 + s2).sqrt(), (s1 + s2).powi(2) / (s1 * s1 / (n1 - 1.0) + s2 * s2 / (n2 - 1.0)))
        }
    };
    if !t.is_finite() {
        return Err(ErrorKind::Div0);
    }
    Ok(num_ok(tails * t_upper(t.abs(), df)))
}

fn chisq_test(env: &Env, a: &[Expr]) -> R<Value> {
    if a.len() != 2 {
        return Err(ErrorKind::Value);
    }
    let (obs, exp) = (env.tab(&a[0])?.to_matrix()?, env.tab(&a[1])?.to_matrix()?);
    if (obs.rows, obs.cols) != (exp.rows, exp.cols) {
        return Err(ErrorKind::Na);
    }
    let mut chi = 0.0;
    for (o, e) in obs.data.iter().zip(&exp.data) {
        match (o, e) {
            (Value::Error(k), _) | (_, Value::Error(k)) => return Err(*k),
            (Value::Number(o), Value::Number(e)) => {
                if *e == 0.0 {
                    return Err(ErrorKind::Div0);
                }
                if *e < 0.0 {
                    return Err(ErrorKind::Num);
                }
                chi += (o - e).powi(2) / e;
            }
            _ => {}
        }
    }
    let df = match (obs.rows, obs.cols) {
        (1, c) => c - 1,
        (r, 1) => r - 1,
        (r, c) => (r - 1) * (c - 1),
    };
    if df == 0 {
        return Err(ErrorKind::Na);
    }
    Ok(num_ok(gamma_pq(df as f64 / 2.0, chi / 2.0).1))
}

// --- regression ---------------------------------------------------------------

/// LINEST-style inputs: y, one list per regressor and the x table as given
/// (the default new_x of TREND and GROWTH).
struct Design {
    y: Vec<f64>,
    xs: Vec<Vec<f64>>,
    xm: Matrix,
    /// Regressors laid out in rows (y is a row).
    by_rows: bool,
}

fn design(env: &Env, a: &[Expr], log: bool) -> R<Design> {
    let ym = env.tab(&a[0])?.to_matrix()?;
    let mut y = all_numbers(&ym)?;
    if log {
        if y.iter().any(|&v| v <= 0.0) {
            return Err(ErrorKind::Num);
        }
        y.iter_mut().for_each(|v| *v = v.ln());
    }
    let n = y.len();
    let xm = match a.get(1) {
        None | Some(Expr::Missing) => Matrix::new(ym.rows, ym.cols, (1..=n).map(|i| Value::Number(i as f64)).collect()),
        Some(e) => env.tab(e)?.to_matrix()?,
    };
    let xv = all_numbers(&xm)?;
    let (xs, by_rows) = if xv.len() == n {
        (vec![xv], false)
    } else if ym.cols == 1 && xm.rows == n {
        ((0..xm.cols).map(|j| (0..n).map(|r| xv[r * xm.cols + j]).collect()).collect(), false)
    } else if ym.rows == 1 && xm.cols == n {
        (xv.chunks(n).map(<[f64]>::to_vec).collect(), true)
    } else {
        return Err(ErrorKind::Ref);
    };
    Ok(Design { y, xs, xm, by_rows })
}

/// Least-squares fit. Regressors that are (nearly) linear combinations of
/// earlier ones are dropped with a zero coefficient, as Excel does.
struct Fit {
    coef: Vec<f64>,
    se: Vec<f64>,
    b: f64,
    se_b: f64,
    df: f64,
    ssreg: f64,
    ssresid: f64,
    kept: usize,
}

fn fit(d: &Design, konst: bool) -> R<Fit> {
    let (n, k) = (d.y.len(), d.xs.len());
    if n == 0 {
        return Err(ErrorKind::Value);
    }
    let avg = |v: &[f64]| if konst { mean(v) } else { 0.0 };
    let means: Vec<f64> = d.xs.iter().map(|x| avg(x)).collect();
    let my = avg(&d.y);
    // Cross-products of the (centered) regressors, with y as the last column.
    let cols: Vec<&[f64]> = d.xs.iter().map(Vec::as_slice).chain(std::iter::once(d.y.as_slice())).collect();
    let centers: Vec<f64> = means.iter().cloned().chain(std::iter::once(my)).collect();
    let p = k + 1;
    let mut m = vec![vec![0.0; p]; p];
    for i in 0..p {
        for j in i..p {
            let s: f64 = (0..n).map(|r| (cols[i][r] - centers[i]) * (cols[j][r] - centers[j])).sum();
            m[i][j] = s;
            m[j][i] = s;
        }
    }
    let sstot = m[k][k];
    let scale: Vec<f64> = (0..k).map(|i| m[i][i]).collect();
    let mut kept = vec![false; k];
    for i in 0..k {
        // What is left of the regressor after the earlier sweeps.
        if m[i][i] > 1e-12 * scale[i] && m[i][i].is_finite() {
            sweep(&mut m, i);
            kept[i] = true;
        }
    }
    let nk = kept.iter().filter(|&&b| b).count();
    let df = n as f64 - nk as f64 - konst as u8 as f64;
    let ssresid = m[k][k].max(0.0);
    let s2 = ssresid / df;
    let coef: Vec<f64> = (0..k).map(|i| if kept[i] { m[i][k] } else { 0.0 }).collect();
    let se: Vec<f64> = (0..k).map(|i| if kept[i] { (s2 * m[i][i]).sqrt() } else { 0.0 }).collect();
    let b = my - coef.iter().zip(&means).map(|(c, x)| c * x).sum::<f64>();
    let mut q = 1.0 / n as f64;
    for i in (0..k).filter(|&i| kept[i]) {
        for j in (0..k).filter(|&j| kept[j]) {
            q += means[i] * means[j] * m[i][j];
        }
    }
    Ok(Fit { coef, se, b, se_b: (s2 * q).sqrt(), df, ssreg: sstot - ssresid, ssresid, kept: nk })
}

/// Gauss-Jordan sweep of a symmetric cross-product matrix on pivot `k`:
/// afterwards the swept block holds the inverse, the y column the
/// coefficients and the y diagonal the residual sum of squares.
fn sweep(m: &mut [Vec<f64>], k: usize) {
    let p = m.len();
    let d = m[k][k];
    for j in 0..p {
        m[k][j] /= d;
    }
    for i in 0..p {
        if i == k {
            continue;
        }
        let b = m[i][k];
        for j in 0..p {
            if j != k {
                m[i][j] -= b * m[k][j];
            }
        }
        m[i][k] = -b / d;
    }
    m[k][k] = 1.0 / d;
}

/// LINEST/LOGEST result: coefficients last regressor first, then b; with
/// `stats`, four more rows (standard errors, r² and sey, F and df, ssreg
/// and ssresid) padded with #N/A.
fn linest(fit: &Fit, konst: bool, stats: bool, log: bool) -> Value {
    let k = fit.coef.len();
    let w = k + 1;
    let tr = |x: f64| num_ok(if log { x.exp() } else { x });
    let mut data: Vec<Value> = fit.coef.iter().rev().map(|&c| tr(c)).collect();
    data.push(tr(fit.b));
    if !stats {
        return Matrix::new(1, w, data).into_value();
    }
    let na = || Value::Error(ErrorKind::Na);
    data.extend(fit.se.iter().rev().map(|&s| num_ok(s)));
    data.push(if konst { num_ok(fit.se_b) } else { na() });
    let sstot = fit.ssreg + fit.ssresid;
    let rows = [
        [fit.ssreg / sstot, (fit.ssresid / fit.df).sqrt()],
        [(fit.ssreg / fit.kept as f64) / (fit.ssresid / fit.df), fit.df],
        [fit.ssreg, fit.ssresid],
    ];
    for r in rows {
        data.push(num_ok(r[0]));
        data.push(num_ok(r[1]));
        data.extend((2..w).map(|_| na()));
    }
    Matrix::new(5, w, data).into_value()
}

// --- special functions ------------------------------------------------------

const LN_SQRT_2PI: f64 = 0.918_938_533_204_672_8;
const SQRT_2PI: f64 = 2.506_628_274_631_000_2;
const EPS: f64 = f64::EPSILON;
const TINY: f64 = 1e-300;
const MAX_ITER: usize = 1_000_000;

/// Lanczos coefficients (g = 7, n = 9).
const LANCZOS: [f64; 9] = [
    0.999_999_999_999_809_9,
    676.520_368_121_885_1,
    -1_259.139_216_722_402_8,
    771.323_428_777_653_1,
    -176.615_029_162_140_6,
    12.507_343_278_686_905,
    -0.138_571_095_265_720_12,
    9.984_369_578_019_572e-6,
    1.505_632_735_149_311_6e-7,
];

fn lanczos_sum(z: f64) -> f64 {
    LANCZOS.iter().enumerate().skip(1).fold(LANCZOS[0], |s, (i, c)| s + c / (z + i as f64))
}

fn ln_gamma(x: f64) -> f64 {
    if x < 0.5 {
        // Reflection: Γ(x)Γ(1-x) = π / sin(πx).
        return (PI / (PI * x).sin().abs()).ln() - ln_gamma(1.0 - x);
    }
    let z = x - 1.0;
    let t = z + 7.5;
    LN_SQRT_2PI + (z + 0.5) * t.ln() - t + lanczos_sum(z).ln()
}

fn gamma(x: f64) -> f64 {
    if x == x.floor() && (1.0..=171.0).contains(&x) {
        return (2..x as u32).fold(1.0, |p, i| p * i as f64);
    }
    if x < 0.5 {
        return PI / ((PI * x).sin() * gamma(1.0 - x));
    }
    let z = x - 1.0;
    let t = z + 7.5;
    // Split the power so it doesn't overflow before e^-t scales it down.
    let p = t.powf((z + 0.5) / 2.0);
    SQRT_2PI * p * (-t).exp() * p * lanczos_sum(z)
}

/// ln Γ(x) - Stirling's approximation, for x >= 10.
fn stirling_corr(x: f64) -> f64 {
    let r = 1.0 / (x * x);
    (1.0 / 12.0 - r * (1.0 / 360.0 - r * (1.0 / 1260.0 - r * (1.0 / 1680.0 - r / 1188.0)))) / x
}

fn ln_beta(a: f64, b: f64) -> f64 {
    if a < 10.0 || b < 10.0 {
        return ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b);
    }
    let s = a + b;
    LN_SQRT_2PI + (a - 0.5) * (a / s).ln() + (b - 0.5) * (b / s).ln() - 0.5 * s.ln() + stirling_corr(a) + stirling_corr(b) - stirling_corr(s)
}

/// ln C(n, k), through ln B for its precision with big arguments.
fn ln_choose(n: f64, k: f64) -> f64 {
    -(n + 1.0).ln() - ln_beta(n - k + 1.0, k + 1.0)
}

/// x·ln(y), taken as 0 when x is 0 (so 0·ln 0 = 0).
fn xlogy(x: f64, y: f64) -> f64 {
    if x == 0.0 { 0.0 } else { x * y.ln() }
}

/// x·ln(1 + y), 0 when x is 0.
fn xlog1py(x: f64, y: f64) -> f64 {
    if x == 0.0 { 0.0 } else { x * y.ln_1p() }
}

/// ln(x^a e^-x / Γ(a)). For large a the Stirling series avoids the
/// cancellation between a·ln x and ln Γ(a).
fn ln_gamma_prefix(a: f64, x: f64) -> f64 {
    if a < 10.0 {
        return xlogy(a, x) - x - ln_gamma(a);
    }
    let d = (x - a) / a;
    a * (d.ln_1p() - d) + 0.5 * a.ln() - LN_SQRT_2PI - stirling_corr(a)
}

/// ln(x^a y^b / B(a, b)) with y = 1 - x; for large a and b written around
/// the peak at x = a/(a+b) so the big terms cancel analytically.
fn ln_beta_prefix(x: f64, y: f64, a: f64, b: f64) -> f64 {
    if a < 10.0 || b < 10.0 {
        return xlogy(a, x) + xlogy(b, y) - ln_beta(a, b);
    }
    let s = a + b;
    // x·s - a = x·b - y·a, without cancellation.
    let e = x * b - y * a;
    a * (e / a).ln_1p() + b * (-e / b).ln_1p() + 0.5 * (a / s * b).ln() - LN_SQRT_2PI - stirling_corr(a) - stirling_corr(b) + stirling_corr(s)
}

/// Regularized incomplete gamma functions (P, Q) = (γ(a,x), Γ(a,x)) / Γ(a):
/// a series below a + 1, a continued fraction above. NaN when they don't
/// converge (enormous a).
fn gamma_pq(a: f64, x: f64) -> (f64, f64) {
    if x <= 0.0 {
        return (0.0, 1.0);
    }
    if x.is_infinite() {
        return (1.0, 0.0);
    }
    let pre = ln_gamma_prefix(a, x).exp();
    if x < a + 1.0 {
        let (mut ap, mut del) = (a, 1.0 / a);
        let mut sum = del;
        for _ in 0..MAX_ITER {
            ap += 1.0;
            del *= x / ap;
            sum += del;
            if del.abs() < sum.abs() * EPS {
                let p = (sum * pre).min(1.0);
                return (p, 1.0 - p);
            }
        }
    } else {
        // Modified Lentz.
        let mut b = x + 1.0 - a;
        let (mut c, mut d) = (1.0 / TINY, 1.0 / b);
        let mut h = d;
        for i in 1..MAX_ITER {
            let an = -(i as f64) * (i as f64 - a);
            b += 2.0;
            d = 1.0 / tiny_floor(an * d + b);
            c = tiny_floor(b + an / c);
            let del = d * c;
            h *= del;
            if (del - 1.0).abs() < EPS {
                let q = (h * pre).min(1.0);
                return (1.0 - q, q);
            }
        }
    }
    (f64::NAN, f64::NAN)
}

/// Keeps Lentz's denominators away from zero.
fn tiny_floor(v: f64) -> f64 {
    if v.abs() < TINY { TINY } else { v }
}

/// Regularized incomplete beta I_x(a, b); `y` is 1 - x, passed separately
/// so callers keep its precision.
fn beta_inc(x: f64, y: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if y <= 0.0 {
        return 1.0;
    }
    let bt = ln_beta_prefix(x, y, a, b).exp();
    if x < (a + 1.0) / (a + b + 2.0) { bt * beta_cf(a, b, x) / a } else { 1.0 - bt * beta_cf(b, a, y) / b }
}

/// Continued fraction for the incomplete beta (modified Lentz); NaN when
/// it doesn't converge.
fn beta_cf(a: f64, b: f64, x: f64) -> f64 {
    let (qab, qap, qam) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 / tiny_floor(1.0 - qab * x / qap);
    let mut h = d;
    for m in 1..MAX_ITER {
        let m = m as f64;
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 / tiny_floor(1.0 + aa * d);
        c = tiny_floor(1.0 + aa / c);
        h *= d * c;
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 / tiny_floor(1.0 + aa * d);
        c = tiny_floor(1.0 + aa / c);
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < EPS {
            return h;
        }
    }
    f64::NAN
}

fn norm_pdf(z: f64) -> f64 {
    (-0.5 * z * z).exp() / SQRT_2PI
}

/// Standard normal CDF through erfc(|z|/√2) = Q(1/2, z²/2), accurate in
/// both tails.
fn norm_cdf(z: f64) -> f64 {
    if z.is_nan() {
        return z;
    }
    let tail = 0.5 * gamma_pq(0.5, z * z / 2.0).1;
    if z < 0.0 { tail } else { 1.0 - tail }
}

/// The FORECAST.ETS functions.
fn ets(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    use crate::analysis::ets::{Aggregation, Completion, EtsError, MAX_SEASON, Model, Regular};
    use Func::*;
    let n = a.len();
    let (min, max, data) = match f {
        ForecastEts => (3, 6, 1),
        ForecastEtsConfint => (3, 7, 1),
        ForecastEtsSeasonality => (2, 4, 0),
        _ => (3, 6, 0),
    };
    if n < min || n > max {
        return Err(ErrorKind::Value);
    }
    // Where the optional arguments start: seasonality, completion, aggregation.
    let opt = match f {
        ForecastEts => 3,
        ForecastEtsConfint => 4,
        ForecastEtsSeasonality => 1,
        _ => 3,
    };
    let (vals, times) = (env.tab(&a[data])?, env.tab(&a[data + 1])?);
    if vals.len() != times.len() {
        return Err(ErrorKind::Na);
    }
    let mut points = Vec::with_capacity(vals.len());
    for i in 0..vals.len() {
        let t = match times.nth(i) {
            Value::Number(t) => t,
            Value::Error(e) => return Err(e),
            _ => return Err(ErrorKind::Value),
        };
        let v = match vals.nth(i) {
            Value::Number(v) => Some(v),
            Value::Empty => None,
            Value::Error(e) => return Err(e),
            _ => return Err(ErrorKind::Value),
        };
        points.push((t, v));
    }
    let season = if f == ForecastEtsSeasonality { 1.0 } else { env.opt_num(a, opt, 1.0)?.trunc() };
    let completion = env.opt_num(a, opt + 1, 1.0)?.trunc();
    let agg = env.opt_num(a, opt + 2, 1.0)?.trunc();
    let completion = match completion as i64 {
        0 => Completion::Zero,
        1 => Completion::Interpolate,
        _ => return Err(ErrorKind::Num),
    };
    let agg = Aggregation::from_code(agg).ok_or(ErrorKind::Num)?;
    if !(0.0..=MAX_SEASON as f64).contains(&season) {
        return Err(ErrorKind::Num);
    }
    let reg = Regular::new(&points, completion, agg).map_err(|e| match e {
        EtsError::TooFew | EtsError::Irregular => ErrorKind::Num,
    })?;
    let model = Model::fit(&reg.values, if season == 1.0 { None } else { Some(season as usize) }).map_err(|_| ErrorKind::Num)?;
    // Steps from the end of the timeline to the target date.
    let ahead = |x: f64| -> R<f64> {
        let h = reg.ahead(x);
        if !h.is_finite() || h < 0.0 || h > 1e6 { Err(ErrorKind::Num) } else { Ok(h) }
    };
    Ok(match f {
        ForecastEts => num_ok(model.forecast(ahead(env.num(&a[0])?)?)),
        ForecastEtsConfint => {
            let conf = env.opt_num(a, 3, 0.95)?;
            if !(conf > 0.0 && conf < 1.0) {
                return Err(ErrorKind::Num);
            }
            num_ok(model.interval(ahead(env.num(&a[0])?)?, norm_inv(0.5 + conf / 2.0)))
        }
        ForecastEtsSeasonality => Value::Number(if model.period >= 2 { model.period as f64 } else { 0.0 }),
        _ => {
            let k = env.num(&a[2])?.trunc();
            match k as i64 {
                8 => num_ok(reg.step),
                1..=7 => model.stat(k as u8).map_or(Value::Error(ErrorKind::Div0), num_ok),
                _ => return Err(ErrorKind::Num),
            }
        }
    })
}

/// Inverse standard normal: Wichura's AS 241 (PPND16), polished with one
/// Halley step.
pub(crate) fn norm_inv(p: f64) -> f64 {
    let q = p - 0.5;
    let mut x = if q.abs() <= 0.425 {
        let r = 0.180625 - q * q;
        q * (((((((2509.080_928_730_122_7 * r + 33430.575_583_588_13) * r + 67265.770_927_008_7) * r + 45921.953_931_549_87) * r + 13731.693_765_509_46) * r + 1971.590_950_306_551_4) * r + 133.141_667_891_784_38) * r + 3.387_132_872_796_366_5)
            / (((((((5226.495_278_852_546 * r + 28729.085_735_721_943) * r + 39307.895_800_092_71) * r + 21213.794_301_586_597) * r + 5394.196_021_424_751) * r + 687.187_007_492_057_9) * r + 42.313_330_701_600_91) * r + 1.0)
    } else {
        let r = if q < 0.0 { p } else { 1.0 - p };
        let r = (-r.ln()).sqrt();
        let v = if r <= 5.0 {
            let r = r - 1.6;
            (((((((7.745_450_142_783_414e-4 * r + 0.022_723_844_989_269_184) * r + 0.241_780_725_177_450_6) * r + 1.270_458_252_452_368_4) * r + 3.647_848_324_763_204_5) * r + 5.769_497_221_460_691) * r + 4.630_337_846_156_546) * r + 1.423_437_110_749_683_5)
                / (((((((1.050_750_071_644_416_9e-9 * r + 5.475_938_084_995_345e-4) * r + 0.015_198_666_563_616_457) * r + 0.148_103_976_427_480_08) * r + 0.689_767_334_985_1) * r + 1.676_384_830_183_803_8) * r + 2.053_191_626_637_759) * r + 1.0)
        } else {
            let r = r - 5.0;
            (((((((2.010_334_399_292_288_1e-7 * r + 2.711_555_568_743_487_6e-5) * r + 0.001_242_660_947_388_078_4) * r + 0.026_532_189_526_576_124) * r + 0.296_560_571_828_504_9) * r + 1.784_826_539_917_291_3) * r + 5.463_784_911_164_114) * r + 6.657_904_643_501_103)
                / (((((((2.044_263_103_389_939_7e-15 * r + 1.421_511_758_316_446e-7) * r + 1.846_318_317_510_054_8e-5) * r + 7.868_691_311_456_133e-4) * r + 0.014_875_361_290_850_615) * r + 0.136_929_880_922_735_8) * r + 0.599_832_206_555_888) * r + 1.0)
        };
        if q < 0.0 { -v } else { v }
    };
    // The error is measured in the tail on x's side, where it is precise.
    let e = if x < 0.0 { norm_cdf(x) - p } else { (1.0 - p) - norm_cdf(-x) };
    let u = e * SQRT_2PI * (x * x / 2.0).exp();
    if u.is_finite() {
        x -= u / (1.0 + x * u / 2.0);
    }
    x
}

/// Upper tail P(T > t) of Student's t with `v` degrees of freedom.
fn t_upper(t: f64, v: f64) -> f64 {
    let t2 = t * t;
    let (x, y) = if t2.is_finite() { (v / (v + t2), t2 / (v + t2)) } else { (v / t.abs() / t.abs(), 1.0) };
    let tail = 0.5 * beta_inc(x, y, v / 2.0, 0.5);
    if t > 0.0 { tail } else { 1.0 - tail }
}

fn t_pdf(t: f64, v: f64) -> f64 {
    (ln_gamma((v + 1.0) / 2.0) - ln_gamma(v / 2.0) - 0.5 * (v * PI).ln() - (v + 1.0) / 2.0 * (t * t / v).ln_1p()).exp()
}

/// The t with P(T > t) = p.
fn t_inv_upper(p: f64, v: f64) -> f64 {
    if p == 0.5 {
        0.0
    } else if p > 0.5 {
        -t_inv_upper(1.0 - p, v)
    } else {
        solve_pos(p, false, 1.0, |x| t_upper(x, v))
    }
}

/// (CDF, upper tail) of the F distribution.
fn f_cdf(x: f64, d1: f64, d2: f64) -> (f64, f64) {
    if x.is_infinite() {
        return (1.0, 0.0);
    }
    let (u, w) = (d1 * x, d1 * x + d2);
    let lower = beta_inc(u / w, d2 / w, d1 / 2.0, d2 / 2.0);
    let upper = beta_inc(d2 / w, u / w, d2 / 2.0, d1 / 2.0);
    (lower, upper)
}

fn f_pdf(x: f64, d1: f64, d2: f64) -> f64 {
    if x == 0.0 {
        return if d1 < 2.0 { f64::INFINITY } else if d1 == 2.0 { 1.0 } else { 0.0 };
    }
    let u = d1 * x;
    (0.5 * (d1 * u.ln() + d2 * d2.ln() - (d1 + d2) * (u + d2).ln()) - x.ln() - ln_beta(d1 / 2.0, d2 / 2.0)).exp()
}

/// Gamma density with shape `a` and scale `s`.
fn gamma_pdf(x: f64, a: f64, s: f64) -> f64 {
    let z = x / s;
    if z == 0.0 {
        return if a < 1.0 { f64::INFINITY } else if a == 1.0 { 1.0 / s } else { 0.0 };
    }
    ln_gamma_prefix(a, z).exp() / (z * s)
}

fn binom_pmf(k: f64, n: f64, p: f64) -> f64 {
    (ln_choose(n, k) + xlogy(k, p) + xlog1py(n - k, -p)).exp()
}

fn binom_cdf(k: f64, n: f64, p: f64) -> f64 {
    if k >= n || p == 0.0 {
        1.0
    } else if p == 1.0 {
        0.0
    } else {
        beta_inc(1.0 - p, p, n - k, k + 1.0)
    }
}

/// Finds x > 0 where the monotone `f` (increasing if `up`) equals `p`:
/// brackets by doubling or halving from the guess `x0`, then narrows the
/// bracket by false position (Illinois variant) to full precision.
fn solve_pos(p: f64, up: bool, x0: f64, f: impl Fn(f64) -> f64) -> f64 {
    // g is increasing, with its root at the answer.
    let g = |x: f64| if up { f(x) - p } else { p - f(x) };
    let x0 = if x0 > 0.0 && x0 < 1e300 { x0 } else { 1.0 };
    let (mut lo, mut hi) = (x0 / 2.0, x0);
    let (mut glo, mut ghi) = (g(lo), g(hi));
    while ghi < 0.0 {
        (lo, glo) = (hi, ghi);
        hi *= 2.0;
        if hi > 1e307 {
            return f64::INFINITY;
        }
        ghi = g(hi);
    }
    while glo >= 0.0 {
        (hi, ghi) = (lo, glo);
        lo *= 0.5;
        if lo < 1e-307 {
            return 0.0;
        }
        glo = g(lo);
    }
    if glo.is_nan() || ghi.is_nan() {
        return f64::NAN;
    }
    let mut side = 0;
    for _ in 0..300 {
        if hi - lo <= 4.0 * EPS * hi {
            break;
        }
        let c = (lo * ghi - hi * glo) / (ghi - glo);
        let c = if c > lo && c < hi { c } else { 0.5 * (lo + hi) };
        let gc = g(c);
        if gc == 0.0 {
            return c;
        }
        if gc < 0.0 {
            (lo, glo) = (c, gc);
            if side == -1 {
                ghi /= 2.0;
            }
            side = -1;
        } else {
            (hi, ghi) = (c, gc);
            if side == 1 {
                glo /= 2.0;
            }
            side = 1;
        }
    }
    0.5 * (lo + hi)
}

/// Bisection on [lo, hi] where `below(x)` tells x is left of the root.
fn bisect(mut lo: f64, mut hi: f64, below: impl Fn(f64) -> bool) -> f64 {
    for _ in 0..2200 {
        let mid = 0.5 * (lo + hi);
        if mid <= lo || mid >= hi {
            break;
        }
        if below(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

#[cfg(test)]
mod tests {
    use crate::format::Locale;
    use crate::value::{ErrorKind, Value};
    use crate::{CellRef, Workbook};

    fn wb() -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        w
    }

    /// Fills a block starting at (row, col), row by row.
    fn fill(w: &mut Workbook, row: u32, col: u32, rows: &[&[&str]]) {
        for (r, line) in rows.iter().enumerate() {
            for (c, text) in line.iter().enumerate() {
                w.set_input(0, CellRef::new(row + r as u32, col + c as u32), text);
            }
        }
    }

    fn num(w: &Workbook, f: &str) -> f64 {
        match w.eval_formula(0, f) {
            Value::Number(n) => n,
            v => panic!("{f}: {v:?}"),
        }
    }

    #[test]
    fn forecast_ets() {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        // Monthly dates (days 1, 31...), a trend and a season of 4.
        let pattern = [10.0, 14.0, 8.0, 12.0];
        for i in 0..24u32 {
            w.set_input(0, CellRef::new(i, 0), &(1000 + i * 30).to_string());
            w.set_input(0, CellRef::new(i, 1), &(100.0 + 2.0 * i as f64 + pattern[i as usize % 4] + [0.3, -0.2, 0.1, -0.4, 0.2][i as usize % 5]).to_string());
        }
        // Next point: t = 24, season position 0.
        near(&w, "=FORECAST.ETS(1720,B1:B24,A1:A24)", 100.0 + 48.0 + 10.0, 0.005);
        near(&w, "=FORECAST.ETS(1720,B1:B24,A1:A24,4)", 158.0, 0.005);
        near(&w, "=PRONOSTICO.ETS(1720,B1:B24,A1:A24)", 158.0, 0.005);
        close(&w, "=FORECAST.ETS.SEASONALITY(B1:B24,A1:A24)", 4.0);
        close(&w, "=FORECAST.ETS.STAT(B1:B24,A1:A24,8)", 30.0);
        let a = num(&w, "=FORECAST.ETS.STAT(B1:B24,A1:A24,1)");
        assert!((0.0..=1.0).contains(&a));
        let c1 = num(&w, "=FORECAST.ETS.CONFINT(1720,B1:B24,A1:A24)");
        let c9 = num(&w, "=FORECAST.ETS.CONFINT(1960,B1:B24,A1:A24,0.99)");
        assert!(c1 >= 0.0 && c9 > c1, "{c1} {c9}");
        // Without seasonality the season is lost: a line.
        let flat = num(&w, "=FORECAST.ETS(1720,B1:B24,A1:A24,0)");
        assert!((flat - 158.0).abs() > 0.5);
        // A date before the end, a bad argument, sizes that differ.
        err(&w, "=FORECAST.ETS(1000,B1:B24,A1:A24)", ErrorKind::Num);
        err(&w, "=FORECAST.ETS(1720,B1:B24,A1:A24,1,2)", ErrorKind::Num);
        err(&w, "=FORECAST.ETS(1720,B1:B24,A1:A23)", ErrorKind::Na);
        err(&w, "=FORECAST.ETS.CONFINT(1720,B1:B24,A1:A24,1)", ErrorKind::Num);
        // Missing values are filled; duplicates aggregated.
        w.set_input(0, CellRef::new(5, 1), "");
        near(&w, "=FORECAST.ETS(1720,B1:B24,A1:A24,4)", 158.0, 0.02);
        assert_eq!(crate::functions::Func::ForecastEts.odf_name(), "COM.MICROSOFT.FORECAST.ETS");
        assert_eq!(crate::functions::Func::lookup("org.libreoffice.forecast.ets.pi.add"), Some(crate::functions::Func::ForecastEtsConfint));
    }

    #[track_caller]
    fn close(w: &Workbook, f: &str, want: f64) {
        near(w, f, want, 1e-9);
    }

    #[track_caller]
    fn near(w: &Workbook, f: &str, want: f64, tol: f64) {
        let got = num(w, f);
        assert!((got - want).abs() <= tol * want.abs().max(1e-300), "{f}: got {got}, want {want}");
    }

    #[track_caller]
    fn err(w: &Workbook, f: &str, k: ErrorKind) {
        assert_eq!(w.eval_formula(0, f), Value::Error(k), "{f}");
    }

    /// An array result: rows, columns and values row by row.
    fn array(w: &Workbook, f: &str) -> (usize, usize, Vec<Value>) {
        match w.eval_formula(0, f) {
            Value::Array(m) => (m.rows, m.cols, m.data.clone()),
            v => panic!("{f}: {v:?}"),
        }
    }

    #[track_caller]
    fn nums_near(got: &[Value], want: &[f64], tol: f64) {
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(want) {
            match g {
                Value::Number(x) => assert!((x - w).abs() <= tol * w.abs().max(1e-12), "got {x}, want {w}"),
                v => panic!("got {v:?}, want {w}"),
            }
        }
    }

    #[test]
    fn a_functions() {
        let mut w = wb();
        fill(&mut w, 0, 0, &[&["10"], &["7"], &["9"], &["2"], &["Not available"], &["TRUE"]]);
        close(&w, "=AVERAGEA(A1:A5)", 5.6);
        close(&w, "=AVERAGEA(A1:A6)", 29.0 / 6.0);
        fill(&mut w, 0, 1, &[&["0"], &["0.2"], &["0.5"], &["0.4"], &["TRUE"]]);
        close(&w, "=MAXA(B1:B5)", 1.0);
        close(&w, "=MINA(A1:A5)", 0.0);
        close(&w, "=MINA(B7:B9)", 0.0);
        close(&w, "=AVERAGEA(1,TRUE,\"3\")", 5.0 / 3.0);
        err(&w, "=AVERAGEA(1,\"x\")", ErrorKind::Value);
        err(&w, "=AVERAGEA(B7:B9)", ErrorKind::Div0);
    }

    #[test]
    fn spread() {
        let mut w = wb();
        let data = ["1345", "1301", "1368", "1322", "1310", "1370", "1318", "1350", "1303", "1299"];
        for (i, d) in data.iter().enumerate() {
            w.set_input(0, CellRef::new(i as u32, 0), d);
        }
        close(&w, "=VAR.P(A1:A10)", 678.84);
        close(&w, "=VARP(A1:A10)", 678.84);
        close(&w, "=VARPA(A1:A10)", 678.84);
        close(&w, "=VARA(A1:A10)", 754.266_666_666_666_7);
        close(&w, "=STDEV.P(A1:A10)", 26.054_558_142_482_477);
        close(&w, "=STDEVPA(A1:A10)", 26.054_558_142_482_477);
        close(&w, "=STDEVA(A1:A10)", 27.463_915_719_843_1);
        w.set_input(0, CellRef::new(10, 0), "TRUE");
        close(&w, "=VAR.P(A1:A11)", 678.84);
        assert!(num(&w, "=VARPA(A1:A11)") > 100_000.0);
        err(&w, "=STDEVA(5)", ErrorKind::Div0);
        err(&w, "=VAR.P(A20:A21)", ErrorKind::Div0);
        close(&w, "=AVEDEV(4,5,6,7,5,4,3)", 1.020_408_163_265_306);
        close(&w, "=DEVSQ(4,5,8,7,11,4,3)", 48.0);
        close(&w, "=GEOMEAN(4,5,8,7,11,4,3)", 5.476_986_969_656_962);
        close(&w, "=HARMEAN(4,5,8,7,11,4,3)", 5.028_375_962_061_728);
        err(&w, "=GEOMEAN(4,0)", ErrorKind::Num);
        err(&w, "=HARMEAN(4,-1)", ErrorKind::Num);
        close(&w, "=TRIMMEAN({4,5,6,7,2,3,4,5,1,2,3},0.2)", 3.777_777_777_777_778);
        err(&w, "=TRIMMEAN({1,2},1)", ErrorKind::Num);
        close(&w, "=KURT(3,4,5,2,3,4,5,6,4,7)", -0.151_799_637_208_414_05);
        close(&w, "=SKEW(3,4,5,2,3,4,5,6,4,7)", 0.359_543_071_406_797_4);
        close(&w, "=SKEW.P(3,4,5,2,3,4,5,6,4,7)", 0.303_193_339_354_143_94);
        err(&w, "=KURT(1,2,3)", ErrorKind::Div0);
        err(&w, "=SKEW(2,2,2,2)", ErrorKind::Div0);
        close(&w, "=STANDARDIZE(42,40,1.5)", 4.0 / 3.0);
        err(&w, "=STANDARDIZE(42,40,0)", ErrorKind::Num);
    }

    #[test]
    fn conditional() {
        let mut w = wb();
        fill(&mut w, 0, 0, &[&["89", "1", "a"], &["93", "2", "b"], &["96", "2", "a"], &["85", "3", "b"], &["91", "1", "a"], &["88", "1", "b"]]);
        close(&w, "=MAXIFS(A1:A6,B1:B6,1)", 91.0);
        close(&w, "=MINIFS(A1:A6,B1:B6,1)", 88.0);
        close(&w, "=MINIFS(A1:A6,B1:B6,1,C1:C6,\"a\")", 89.0);
        close(&w, "=AVERAGEIFS(A1:A6,B1:B6,\">1\",C1:C6,\"b\")", 89.0);
        close(&w, "=AVERAGEIFS(A1:A6,A1:A6,\">85\",A1:A6,\"<95\")", 90.25);
        close(&w, "=MAXIFS(A1:A6,B1:B6,9)", 0.0);
        err(&w, "=AVERAGEIFS(A1:A6,B1:B6,9)", ErrorKind::Div0);
        err(&w, "=MAXIFS(A1:A6,B1:B5,1)", ErrorKind::Value);
        err(&w, "=AVERAGEIFS(A1:A6,B1:B6)", ErrorKind::Value);
        // An array of criteria gives one result per criterion.
        let (r, c, v) = array(&w, "=MAXIFS(A1:A6,B1:B6,{1,2,3})");
        assert_eq!((r, c), (1, 3));
        nums_near(&v, &[91.0, 96.0, 85.0], 0.0);
    }

    #[test]
    fn modes_and_ranks() {
        let mut w = wb();
        close(&w, "=MODE.SNGL(5.6,4,4,3,2,4)", 4.0);
        close(&w, "=MODE(1,2,2,1)", 1.0);
        err(&w, "=MODE(1,2,3)", ErrorKind::Na);
        let (r, c, v) = array(&w, "=MODE.MULT({1,2,3,4,3,2,1,2,3,5,6,1})");
        assert_eq!((r, c), (3, 1));
        nums_near(&v, &[1.0, 2.0, 3.0], 0.0);
        close(&w, "=PERCENTILE.INC({1,3,2,4},0.3)", 1.9);
        close(&w, "=PERCENTILE({1,3,2,4},1)", 4.0);
        close(&w, "=PERCENTILE.EXC({1,2,3,6,6,6,7,8,9},0.25)", 2.5);
        err(&w, "=PERCENTILE.EXC({1,2,3,6,6,6,7,8,9},0.01)", ErrorKind::Num);
        err(&w, "=PERCENTILE.INC({1,2},1.5)", ErrorKind::Num);
        close(&w, "=QUARTILE.INC({1,2,4,7,8,9,10,12},1)", 3.5);
        close(&w, "=QUARTILE({1,2,4,7,8,9,10,12},4.5)", 12.0);
        close(&w, "=QUARTILE.EXC({6,7,15,36,39,40,41,42,43,47,49},1)", 15.0);
        close(&w, "=QUARTILE.EXC({6,7,15,36,39,40,41,42,43,47,49},3)", 43.0);
        err(&w, "=QUARTILE.EXC({1,2,3},4)", ErrorKind::Num);
        let (_, _, v) = array(&w, "=PERCENTILE.INC({1,3,2,4},{0,0.5,1})");
        nums_near(&v, &[1.0, 2.5, 4.0], 1e-12);
        let data = "{13,12,11,8,4,3,2,1,1,1}";
        close(&w, &format!("=PERCENTRANK.INC({data},2)"), 0.333);
        close(&w, &format!("=PERCENTRANK.INC({data},4)"), 0.555);
        close(&w, &format!("=PERCENTRANK.INC({data},8)"), 0.666);
        close(&w, &format!("=PERCENTRANK({data},5)"), 0.583);
        close(&w, &format!("=PERCENTRANK.INC({data},5,5)"), 0.58333);
        err(&w, &format!("=PERCENTRANK.INC({data},20)"), ErrorKind::Na);
        err(&w, &format!("=PERCENTRANK.INC({data},5,0)"), ErrorKind::Num);
        close(&w, "=PERCENTRANK.EXC({1,2,3,6,6,6,7,8,9},7)", 0.7);
        close(&w, "=PERCENTRANK.EXC({1,2,3,6,6,6,7,8,9},5.43)", 0.381);
        close(&w, "=PERCENTRANK.EXC({1,2,3,6,6,6,7,8,9},5.43,1)", 0.3);
        fill(&mut w, 0, 0, &[&["89"], &["88"], &["92"], &["101"], &["94"], &["97"], &["95"], &["94"]]);
        close(&w, "=RANK.AVG(94,A1:A7)", 4.0);
        close(&w, "=RANK.AVG(94,A1:A8)", 4.5);
        close(&w, "=RANK.AVG(94,A1:A8,1)", 4.5);
        close(&w, "=RANK.AVG(89,A1:A8,1)", 2.0);
        err(&w, "=RANK.AVG(1,A1:A8)", ErrorKind::Na);
        let (_, _, v) = array(&w, "=FREQUENCY({79,85,78,85,50,81,95,88,97},{70,79,89})");
        nums_near(&v, &[1.0, 2.0, 4.0, 2.0], 0.0);
        // Bins out of order keep their positions.
        let (_, _, v) = array(&w, "=FREQUENCY({79,85,78,85,50,81,95,88,97},{89;70;79})");
        nums_near(&v, &[4.0, 1.0, 2.0, 2.0], 0.0);
    }

    #[test]
    fn paired() {
        let w = wb();
        close(&w, "=CORREL({3,2,4,5,6},{9,7,12,15,17})", 0.997_054_485_501_581_3);
        close(&w, "=PEARSON({9,7,5,3,1},{10,6,1,5,3})", 0.699_378_606_180_235_3);
        close(&w, "=RSQ({2,3,9,1,8,7,5},{6,5,11,7,5,4,4})", 0.057_950_191_570_881_2);
        close(&w, "=COVAR({3,2,4,5,6},{9,7,12,15,17})", 5.2);
        close(&w, "=COVARIANCE.P({3,2,4,5,6},{9,7,12,15,17})", 5.2);
        close(&w, "=COVARIANCE.S({2,4,8},{5,11,12})", 9.666_666_666_666_666);
        close(&w, "=SLOPE({2,3,9,1,8,7,5},{6,5,11,7,5,4,4})", 0.305_555_555_555_555_6);
        close(&w, "=INTERCEPT({2,3,9,1,8},{6,5,11,7,5})", 0.048_387_096_774_193_33);
        close(&w, "=STEYX({2,3,9,1,8,7,5},{6,5,11,7,5,4,4})", 3.305_718_950_210_977);
        close(&w, "=FORECAST(30,{6,7,9,15,21},{20,28,31,38,40})", 10.607_253_086_419_75);
        close(&w, "=FORECAST.LINEAR(30,{6,7,9,15,21},{20,28,31,38,40})", 10.607_253_086_419_75);
        err(&w, "=CORREL({1,2,3},{1,2})", ErrorKind::Na);
        err(&w, "=CORREL({1,1,1},{1,2,3})", ErrorKind::Div0);
        err(&w, "=SLOPE({1,2},{3,3})", ErrorKind::Div0);
        // Pairs with text on either side are skipped.
        close(&w, "=SLOPE({1,2,\"x\",3},{1,2,5,3})", 1.0);
    }

    #[test]
    fn regression() {
        let mut w = wb();
        let rows: [[&str; 5]; 11] = [
            ["2310", "2", "2", "20", "142000"],
            ["2333", "2", "2", "12", "144000"],
            ["2356", "3", "1.5", "33", "151000"],
            ["2379", "3", "2", "43", "150000"],
            ["2402", "2", "3", "53", "139000"],
            ["2425", "4", "2", "23", "169000"],
            ["2448", "2", "1.5", "99", "126000"],
            ["2471", "2", "2", "34", "142900"],
            ["2494", "3", "3", "23", "163000"],
            ["2517", "4", "4", "55", "169000"],
            ["2540", "2", "3", "22", "149000"],
        ];
        let rows: Vec<&[&str]> = rows.iter().map(|r| &r[..]).collect();
        fill(&mut w, 0, 0, &rows);
        let (r, c, v) = array(&w, "=LINEST(E1:E11,A1:D11,TRUE,TRUE)");
        assert_eq!((r, c), (5, 5));
        nums_near(&v[..5], &[-234.237_164_5, 2553.210_66, 12_529.768_17, 27.641_387_37, 52_317.830_51], 1e-9);
        nums_near(&v[5..10], &[13.268_011_48, 530.669_151_9, 400.066_838_2, 5.429_374_042, 12_237.3616], 1e-8);
        nums_near(&v[10..12], &[0.996_747_993_384_510_1, 970.578_462_928_499_2], 1e-9);
        nums_near(&v[15..17], &[459.753_674_225_399_1, 6.0], 1e-9);
        nums_near(&v[20..22], &[1_732_393_319.229_250_7, 5_652_135.316_203_889], 1e-8);
        assert_eq!(v[12], Value::Error(ErrorKind::Na));
        near(&w, "=INDEX(LINEST(E1:E11,A1:D11),1,4)", 27.641_387_37, 1e-9);
        // TREND with several regressors predicts one value per row.
        let predicted = 52_317.830_51 + 27.641_387_37 * 2500.0 + 12_529.768_17 * 3.0 + 2553.210_66 * 2.0 - 234.237_164_5 * 25.0;
        near(&w, "=TREND(E1:E11,A1:D11,{2500,3,2,25})", predicted, 1e-8);

        let (_, _, v) = array(&w, "=LINEST({1,9,5,7},{0,4,2,3})");
        nums_near(&v, &[2.0, 1.0], 1e-12);
        let (_, _, v) = array(&w, "=LINEST({1,9,5,7},{0,4,2,3},FALSE,TRUE)");
        nums_near(&v[..2], &[2.310_344_827_586_207, 0.0], 1e-12);
        nums_near(&v[2..3], &[0.117_781_043_286_891_93], 1e-9);
        assert_eq!(v[3], Value::Error(ErrorKind::Na));
        nums_near(&v[4..6], &[0.992_263_483_642_794, 0.634_270_329_256_156_1], 1e-9);
        // A collinear regressor gets a zero coefficient.
        let (_, _, v) = array(&w, "=LINEST({1;2;3;5},{1,2;2,4;3,6;4,8})");
        nums_near(&v, &[0.0, 1.3, -0.5], 1e-9);

        let units = "{33100,47300,69000,102000,150000,220000}";
        let (_, _, v) = array(&w, &format!("=LOGEST({units},{{11,12,13,14,15,16}})"));
        nums_near(&v, &[1.463_275_628_116_175_6, 495.304_770_158_728_7], 1e-9);
        let (r, c, v) = array(&w, &format!("=GROWTH({units},{{11,12,13,14,15,16}},{{17;18}})"));
        assert_eq!((r, c), (2, 1));
        nums_near(&v, &[320_196.718_363_47, 468_536.054_184_05], 1e-9);
        err(&w, "=LOGEST({1,-2},{1,2})", ErrorKind::Num);
        let y = "{133890,135000,135790,137300,138130,139100,139900,141120,141890,143230,144000,145290}";
        close(&w, &format!("=TREND({y},,13)"), 146_171.515_151_515_05);
        let (r, c, v) = array(&w, &format!("=TREND({y})"));
        assert_eq!((r, c), (1, 12));
        nums_near(&v[..1], &[133_953.333_333_333_34], 1e-9);
        err(&w, "=TREND({1,2,3},{1,2})", ErrorKind::Ref);
        err(&w, "=LINEST({1,\"a\",3})", ErrorKind::Value);
    }

    #[test]
    fn normal() {
        let w = wb();
        close(&w, "=NORM.DIST(42,40,1.5,TRUE)", 0.908_788_780_274_132_1);
        close(&w, "=NORMDIST(42,40,1.5,FALSE)", 0.109_340_049_783_995_77);
        close(&w, "=NORM.S.DIST(-9,TRUE)", 1.128_588_405_953_832_4e-19);
        close(&w, "=NORM.S.DIST(1.333333,FALSE)", 0.164_010_147_569_367_22);
        close(&w, "=NORMSDIST(0.3)", 0.617_911_422_188_952_6);
        close(&w, "=NORM.INV(0.908789,40,1.5)", 42.000_002_009_566_16);
        close(&w, "=NORMINV(0.908789,40,1.5)", 42.000_002_009_566_16);
        close(&w, "=NORM.S.INV(1E-20)", -9.262_340_089_798_409);
        close(&w, "=NORM.S.INV(0.02)", -2.053_748_910_631_823);
        close(&w, "=NORMSINV(0.999)", 3.090_232_306_167_813);
        assert_eq!(num(&w, "=NORM.S.INV(0.5)"), 0.0);
        err(&w, "=NORM.S.INV(1)", ErrorKind::Num);
        err(&w, "=NORM.DIST(1,0,0,TRUE)", ErrorKind::Num);
        err(&w, "=NORM.DIST(1,0,1)", ErrorKind::Value);
        close(&w, "=LOGNORM.DIST(4,3.5,1.2,TRUE)", 0.039_083_555_706_800_47);
        close(&w, "=LOGNORM.DIST(4,3.5,1.2,FALSE)", 0.017_617_596_681_819_23);
        close(&w, "=LOGNORM.INV(0.039084,3.5,1.2)", 4.000_025_218_680_635);
        close(&w, "=LOGINV(0.039084,3.5,1.2)", 4.000_025_218_680_635);
        err(&w, "=LOGNORM.DIST(0,3.5,1.2,TRUE)", ErrorKind::Num);
        close(&w, "=CONFIDENCE.NORM(0.05,2.5,50)", 0.692_951_912_174_839_1);
        close(&w, "=CONFIDENCE(0.05,2.5,50)", 0.692_951_912_174_839_1);
        close(&w, "=CONFIDENCE.T(0.05,1,50)", 0.284_196_855_495_729_9);
        err(&w, "=CONFIDENCE.T(0.05,1,1)", ErrorKind::Div0);
        close(&w, "=Z.TEST({3,6,7,8,6,5,4,2,1,9},4)", 0.090_574_196_851_363_86);
        close(&w, "=ZTEST({3,6,7,8,6,5,4,2,1,9},6)", 0.863_043_389_129_529_9);
        close(&w, "=FISHER(0.75)", 0.972_955_074_527_656_6);
        close(&w, "=FISHERINV(0.9729550745276566)", 0.75);
        err(&w, "=FISHER(1)", ErrorKind::Num);
        close(&w, "=PHI(0.75)", 0.301_137_432_154_804_43);
        close(&w, "=GAUSS(2)", 0.477_249_868_051_820_8);
        close(&w, "=GAUSS(1E-10)", 3.989_422_804_014_327e-11);
        let (_, _, v) = array(&w, "=NORM.S.DIST({0,1},TRUE)");
        nums_near(&v, &[0.5, 0.841_344_746_068_542_9], 1e-12);
    }

    #[test]
    fn student_chi_f() {
        let w = wb();
        close(&w, "=T.DIST(60,1,TRUE)", 0.994_695_326_367_376_7);
        close(&w, "=T.DIST(8,3,FALSE)", 0.000_736_906_520_946_926_4);
        close(&w, "=T.DIST(-2.5,7,TRUE)", 0.020_496_109_292_876_444);
        close(&w, "=T.DIST.2T(1.959999998,60)", 0.054_644_929_975_920_9);
        close(&w, "=T.DIST.RT(1.959999998,60)", 0.027_322_464_987_960_45);
        close(&w, "=TDIST(1.959999998,60,1)", 0.027_322_464_987_960_45);
        close(&w, "=T.DIST.RT(1.5,2.5)", 0.136_196_562_445_500_55);
        close(&w, "=T.DIST.RT(3,1000000)", 0.001_349_931_270_710_9);
        err(&w, "=T.DIST.2T(-1,5)", ErrorKind::Num);
        err(&w, "=TDIST(1,5,3)", ErrorKind::Num);
        close(&w, "=T.INV(0.75,2)", 0.816_496_580_927_726_1);
        close(&w, "=T.INV(0.001,5)", -5.893_429_531_356_009);
        close(&w, "=T.INV.2T(0.546449,60)", 0.606_533_075_825_754_9);
        close(&w, "=TINV(0.546449,60)", 0.606_533_075_825_754_9);
        err(&w, "=T.INV(0,5)", ErrorKind::Num);
        let a = "{3,4,5,8,9,1,2,4,5}";
        let b = "{6,19,3,2,14,4,5,17,1}";
        close(&w, &format!("=T.TEST({a},{b},2,1)"), 0.196_015_784_925_282_04);
        close(&w, &format!("=TTEST({a},{b},2,2)"), 0.191_995_886_760_396_22);
        close(&w, &format!("=T.TEST({a},{b},1,3)"), 0.101_146_961_684_338_98);
        err(&w, &format!("=T.TEST({a},{b},3,1)"), ErrorKind::Num);
        err(&w, "=T.TEST({1,2,3},{1,2},2,1)", ErrorKind::Na);

        close(&w, "=CHISQ.DIST(0.5,1,TRUE)", 0.520_499_877_813_046_6);
        close(&w, "=CHISQ.DIST(2,3,FALSE)", 0.207_553_748_710_297_4);
        close(&w, "=CHISQ.DIST.RT(18.307,10)", 0.050_000_589_091_398_12);
        close(&w, "=CHIDIST(300,100)", 7.412_100_857_323_017_5e-22);
        near(&w, "=CHISQ.DIST(1000500,1000000,TRUE)", 0.638_317_768_289_695_7, 1e-8);
        close(&w, "=CHISQ.INV(0.93,1)", 3.283_020_286_759_539);
        close(&w, "=CHISQ.INV(0.6,2)", 1.832_581_463_748_31);
        close(&w, "=CHISQ.INV(1E-10,1)", 1.570_796_326_794_896e-20);
        close(&w, "=CHISQ.INV.RT(0.050001,10)", 18.306_973_456_961_06);
        close(&w, "=CHIINV(0.050001,10)", 18.306_973_456_961_06);
        err(&w, "=CHISQ.DIST(-1,2,TRUE)", ErrorKind::Num);
        err(&w, "=CHISQ.INV(0.5,0)", ErrorKind::Num);
        let obs = "{58,35;11,25;10,23}";
        let exp = "{45.35,47.65;17.56,18.44;16.09,16.91}";
        close(&w, &format!("=CHISQ.TEST({obs},{exp})"), 0.000_308_192_017_008_309_5);
        err(&w, "=CHISQ.TEST({1,2},{1,2,3})", ErrorKind::Na);

        close(&w, "=F.DIST(15.2069,6,4,TRUE)", 0.990_000_043_002_762_7);
        close(&w, "=F.DIST(15.2069,6,4,FALSE)", 0.001_223_791_708_783_18);
        close(&w, "=F.DIST(0.5,1,1,TRUE)", 0.391_826_552_030_607_23);
        close(&w, "=F.DIST.RT(15.2068649,6,4)", 0.009_999_999_952_464_6);
        close(&w, "=FDIST(15.2068649,6,4)", 0.009_999_999_952_464_6);
        close(&w, "=F.INV(0.01,6,4)", 0.109_309_914_124_578_52);
        close(&w, "=F.INV.RT(0.01,6,4)", 15.206_864_861_157_523);
        close(&w, "=FINV(0.01,6,4)", 15.206_864_861_157_523);
        err(&w, "=F.DIST(1,0,4,TRUE)", ErrorKind::Num);
        close(&w, "=F.TEST({6,7,9,15,21},{20,28,31,38,40})", 0.648_317_846_786_174_5);
        err(&w, "=F.TEST({1},{2,3})", ErrorKind::Div0);
    }

    #[test]
    fn discrete() {
        let w = wb();
        close(&w, "=BINOM.DIST(6,10,0.5,FALSE)", 0.205_078_125);
        close(&w, "=BINOMDIST(6,10,0.3,TRUE)", 0.989_407_921_6);
        close(&w, "=BINOM.DIST(40,100,0.45,TRUE)", 0.183_056_944_214_465_4);
        close(&w, "=BINOM.DIST(10,10,0.5,TRUE)", 1.0);
        // Excel's examples.
        close(&w, "=BINOM.DIST.RANGE(60,0.75,48)", 0.083_974_967_429);
        close(&w, "=BINOM.DIST.RANGE(60,0.75,45,50)", 0.523_629_793);
        assert_eq!(w.eval_formula(0, "=BINOM.DIST.RANGE(10,0.5,6,5)"), Value::Error(ErrorKind::Num));
        err(&w, "=BINOM.DIST(11,10,0.5,TRUE)", ErrorKind::Num);
        close(&w, "=BINOM.INV(6,0.5,0.75)", 4.0);
        close(&w, "=CRITBINOM(1000,0.3,0.9)", 319.0);
        close(&w, "=BINOM.INV(6,0.5,0.65625)", 3.0);
        close(&w, "=POISSON.DIST(2,5,TRUE)", 0.124_652_019_483_081_08);
        close(&w, "=POISSON(2,5,FALSE)", 0.084_224_337_488_568_32);
        close(&w, "=POISSON.DIST(0,0,FALSE)", 1.0);
        close(&w, "=HYPGEOM.DIST(1,4,8,20,TRUE)", 0.465_428_276_573_787_4);
        close(&w, "=HYPGEOM.DIST(1,4,8,20,FALSE)", 0.363_261_093_911_248_67);
        err(&w, "=HYPGEOM.DIST(5,4,8,20,FALSE)", ErrorKind::Num);
        close(&w, "=NEGBINOM.DIST(10,5,0.25,TRUE)", 0.313_514_058_478_176_6);
        close(&w, "=NEGBINOM.DIST(10,5,0.25,FALSE)", 0.055_048_660_375_177_88);
        close(&w, "=NEGBINOMDIST(10,5,0.25)", 0.055_048_660_375_177_88);
        close(&w, "=PROB({0,1,2,3},{0.2,0.3,0.1,0.4},2)", 0.1);
        close(&w, "=PROB({0,1,2,3},{0.2,0.3,0.1,0.4},1,3)", 0.8);
        err(&w, "=PROB({0,1},{0.2,0.3},1)", ErrorKind::Num);
        err(&w, "=PROB({0,1},{1},1)", ErrorKind::Na);
    }

    #[test]
    fn continuous() {
        let w = wb();
        close(&w, "=EXPON.DIST(0.2,10,TRUE)", 0.864_664_716_763_387_3);
        close(&w, "=EXPONDIST(0.2,10,FALSE)", 1.353_352_832_366_127);
        err(&w, "=EXPON.DIST(0.2,0,TRUE)", ErrorKind::Num);
        close(&w, "=GAMMA.DIST(10.00001131,9,2,FALSE)", 0.032_639_130_418_293_96);
        close(&w, "=GAMMADIST(10.00001131,9,2,TRUE)", 0.068_094_003_869_787_2);
        close(&w, "=GAMMA.DIST(150,120,1,TRUE)", 0.994_895_288_396_849_6);
        close(&w, "=GAMMA.INV(0.068094,9,2)", 10.000_011_191_437_181);
        close(&w, "=GAMMA(2.5)", 1.329_340_388_179_137);
        close(&w, "=GAMMA(-3.75)", 0.267_866_128_861_416_53);
        close(&w, "=GAMMA(0.1)", 9.513_507_698_668_732);
        close(&w, "=GAMMA(100.5)", 9.320_963_104_082_716e156);
        assert_eq!(num(&w, "=GAMMA(5)"), 24.0);
        err(&w, "=GAMMA(0)", ErrorKind::Num);
        err(&w, "=GAMMA(-2)", ErrorKind::Num);
        err(&w, "=GAMMA(200)", ErrorKind::Num);
        close(&w, "=GAMMALN(4)", 1.791_759_469_228_055);
        close(&w, "=GAMMALN.PRECISE(0.2)", 1.524_063_822_430_784_5);
        close(&w, "=GAMMALN(1000.3)", 5_907.292_644_785_879);
        err(&w, "=GAMMALN(0)", ErrorKind::Num);
        close(&w, "=BETA.DIST(2,8,10,TRUE,1,3)", 0.685_470_581_054_687_5);
        close(&w, "=BETA.DIST(2,8,10,FALSE,1,3)", 1.483_764_648_437_499_6);
        close(&w, "=BETA.DIST(0.2,0.5,3,TRUE)", 0.733_430_296_619_931);
        err(&w, "=BETA.DIST(4,8,10,TRUE,1,3)", ErrorKind::Num);
        close(&w, "=BETA.INV(0.685470581,8,10,1,3)", 1.999_999_999_963_142_8);
        close(&w, "=BETA.INV(0.1,0.5,3)", 0.002_855_295_084_880_771_6);
        close(&w, "=WEIBULL.DIST(105,20,100,TRUE)", 0.929_581_390_069_276_9);
        close(&w, "=WEIBULL(105,20,100,FALSE)", 0.035_588_864_024_504_334);
        // Extreme inputs give errors or numbers, never panics.
        close(&w, "=T.INV(1E-100,1)", -3.183_098_861_837_907e99);
        num(&w, "=T.INV(1E-300,1)");
        err(&w, "=NORM.S.DIST(\"x\",TRUE)", ErrorKind::Value);
        num(&w, "=CHISQ.INV(0.5,1E10)");
        close(&w, "=BINOM.INV(1E9,0.5,0.5)", 5e8);
        num(&w, "=GAMMA.INV(0.5,1E-5,1)");
    }

    /// The legacy variants, against LibreOffice.
    #[test]
    fn legacy() {
        let w = wb();
        close(&w, "=BETADIST(0.5,2,3)", 0.6875);
        close(&w, "=BETADIST(2,8,10,1,3)", 0.685_470_581_054_688);
        close(&w, "=BETAINV(0.5,2,3)", 0.385_727_568_132_39);
        close(&w, "=BETAINV(0.685470581,8,10,1,3)", 2.0);
        err(&w, "=BETADIST(0.5,2,3,TRUE)", ErrorKind::Num);
        close(&w, "=HYPGEOMDIST(1,4,8,20)", 0.363_261_093_911_249);
        err(&w, "=HYPGEOMDIST(1,4,8,20,TRUE)", ErrorKind::Value);
        close(&w, "=LOGNORMDIST(4,3.5,1.2)", 0.039_083_555_706_800_5);
        err(&w, "=LOGNORMDIST(4,3.5,1.2,TRUE)", ErrorKind::Value);
        close(&w, "=ECMA.CEILING(2.5,1)", 3.0);
        close(&w, "=ECMA.CEILING(-2.5,2)", -2.0);
    }
}
