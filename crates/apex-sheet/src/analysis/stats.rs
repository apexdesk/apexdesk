//! Analysis ToolPak style statistics tools. Each tool takes the input data
//! already read from the sheet (see `Workbook::read_series`) and returns a
//! [`Table`] of values with captions, laid out like Excel's output.
//!
//! Missing values (empty cells, text, errors) are skipped; tools that pair
//! values (regression, paired t-test, correlation) drop incomplete rows.

use super::labels::Lang;
use super::numeric::{self, Rng, correl, covar_p, dev_sq, mean, median, sorted, var_s};
use super::{Table, na, num};
use crate::value::Value;

/// One input variable: a column (or row) of the input range.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    /// Caption from the first row/column when "Labels" is on.
    pub label: Option<String>,
    /// One entry per cell; None for anything that is not a number.
    pub values: Vec<Option<f64>>,
}

impl Series {
    pub fn new(label: Option<&str>, values: &[f64]) -> Series {
        Series { label: label.map(str::to_string), values: values.iter().map(|v| Some(*v)).collect() }
    }

    /// The numbers, skipping missing values.
    pub fn nums(&self) -> Vec<f64> {
        self.values.iter().flatten().copied().filter(|v| v.is_finite()).collect()
    }

    fn name(&self, lang: Lang, key: &str, i: usize) -> String {
        self.label.clone().unwrap_or_else(|| lang.fmt(key, &[&(i + 1).to_string()]))
    }
}

/// Why a tool could not produce output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatsError {
    /// Too few numbers for the statistic (or no input at all).
    NotEnoughData,
    /// Input ranges of different sizes where they must match.
    SizeMismatch,
    /// A parameter out of range (interval, damping, alpha...).
    BadParameter,
}

/// Rows present in every series (for tools that pair observations).
fn complete_rows(series: &[&Series]) -> Vec<Vec<f64>> {
    let n = series.iter().map(|s| s.values.len()).min().unwrap_or(0);
    (0..n)
        .filter_map(|i| series.iter().map(|s| s.values[i].filter(|v| v.is_finite())).collect::<Option<Vec<f64>>>())
        .collect()
}

fn t(s: impl Into<std::sync::Arc<str>>) -> Value {
    Value::Text(s.into())
}

// ---------------------------------------------------------------------------
// Descriptive statistics

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DescriptiveOpts {
    pub summary: bool,
    /// Confidence level for the mean, in percent (95.0), if wanted.
    pub confidence: Option<f64>,
    pub kth_largest: Option<usize>,
    pub kth_smallest: Option<usize>,
}

impl Default for DescriptiveOpts {
    fn default() -> Self {
        DescriptiveOpts { summary: true, confidence: Some(95.0), kth_largest: None, kth_smallest: None }
    }
}

/// Descriptive statistics: two columns (caption, value) per series.
pub fn descriptive(series: &[Series], o: &DescriptiveOpts, lang: Lang) -> Result<Table, StatsError> {
    if series.is_empty() {
        return Err(StatsError::NotEnoughData);
    }
    let mut out = Table::new();
    for (i, s) in series.iter().enumerate() {
        let v = s.nums();
        let n = v.len() as f64;
        let sd = var_s(&v).sqrt();
        let srt = sorted(&v);
        let mut tb = Table::new();
        tb.push(vec![t(s.name(lang, "column_n", i))]);
        tb.blank();
        if o.summary {
            let (min, max) = (srt.first().copied().unwrap_or(f64::NAN), srt.last().copied().unwrap_or(f64::NAN));
            let rows: [(&str, Value); 13] = [
                ("mean", num(mean(&v))),
                ("std_error", num(sd / n.sqrt())),
                ("median", num(median(&v))),
                ("mode", numeric::mode(&v).map_or(na(), num)),
                ("std_dev", num(sd)),
                ("sample_variance", num(var_s(&v))),
                ("kurtosis", num(numeric::kurt(&v))),
                ("skewness", num(numeric::skew(&v))),
                ("range", num(max - min)),
                ("minimum", num(min)),
                ("maximum", num(max)),
                ("sum", num(v.iter().sum())),
                ("count", num(n)),
            ];
            for (k, val) in rows {
                tb.push(vec![t(lang.text(k)), val]);
            }
        }
        if let Some(k) = o.kth_largest {
            let val = if k >= 1 && k <= srt.len() { num(srt[srt.len() - k]) } else { Value::Error(crate::value::ErrorKind::Num) };
            tb.push(vec![t(lang.fmt("largest", &[&k.to_string()])), val]);
        }
        if let Some(k) = o.kth_smallest {
            let val = if k >= 1 && k <= srt.len() { num(srt[k - 1]) } else { Value::Error(crate::value::ErrorKind::Num) };
            tb.push(vec![t(lang.fmt("smallest", &[&k.to_string()])), val]);
        }
        if let Some(cl) = o.confidence {
            if !(0.0 < cl && cl < 100.0) {
                return Err(StatsError::BadParameter);
            }
            let half = numeric::t_inv_2t(1.0 - cl / 100.0, n - 1.0) * sd / n.sqrt();
            tb.push(vec![t(lang.fmt("confidence", &[&lang.pct1(cl)])), num(half)]);
        }
        if i == 0 {
            out = tb;
        } else {
            out.beside(tb, 0);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Correlation and covariance

fn matrix_tool(series: &[Series], lang: Lang, f: impl Fn(&[f64], &[f64]) -> f64) -> Result<Table, StatsError> {
    if series.is_empty() {
        return Err(StatsError::NotEnoughData);
    }
    let mut out = Table::new();
    let mut head = vec![Value::Empty];
    head.extend(series.iter().enumerate().map(|(i, s)| t(s.name(lang, "column_n", i))));
    out.push(head);
    for (i, a) in series.iter().enumerate() {
        let mut row = vec![t(a.name(lang, "column_n", i))];
        for b in &series[..=i] {
            let rows = complete_rows(&[a, b]);
            let (x, y): (Vec<f64>, Vec<f64>) = rows.iter().map(|r| (r[0], r[1])).unzip();
            row.push(if x.is_empty() { Value::Error(crate::value::ErrorKind::Div0) } else { num(f(&x, &y)) });
        }
        out.push(row);
    }
    Ok(out)
}

/// Correlation matrix (lower triangle, like the ToolPak).
pub fn correlation(series: &[Series], lang: Lang) -> Result<Table, StatsError> {
    matrix_tool(series, lang, |a, b| if a == b { 1.0 } else { correl(a, b) })
}

/// Covariance matrix (population covariance, as the ToolPak computes it).
pub fn covariance(series: &[Series], lang: Lang) -> Result<Table, StatsError> {
    matrix_tool(series, lang, covar_p)
}

// ---------------------------------------------------------------------------
// Regression

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegressionOpts {
    /// Force the intercept to zero.
    pub constant_zero: bool,
    /// Extra confidence level for the coefficients, in percent.
    pub confidence: f64,
    pub residuals: bool,
    pub standardized: bool,
}

impl Default for RegressionOpts {
    fn default() -> Self {
        RegressionOpts { constant_zero: false, confidence: 95.0, residuals: false, standardized: false }
    }
}

/// Multiple linear regression of `y` on `xs` (Excel's SUMMARY OUTPUT).
pub fn regression(y: &Series, xs: &[Series], o: &RegressionOpts, lang: Lang) -> Result<Table, StatsError> {
    if xs.is_empty() {
        return Err(StatsError::NotEnoughData);
    }
    if xs.iter().any(|x| x.values.len() != y.values.len()) {
        return Err(StatsError::SizeMismatch);
    }
    if !(0.0 < o.confidence && o.confidence < 100.0) {
        return Err(StatsError::BadParameter);
    }
    let mut all: Vec<&Series> = vec![y];
    all.extend(xs.iter());
    let rows = complete_rows(&all);
    let n = rows.len();
    let k = xs.len();
    let konst = !o.constant_zero;
    if n < 2 || n <= k + konst as usize - 1 {
        return Err(StatsError::NotEnoughData);
    }
    let yv: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let ybar = mean(&yv);
    let xbar: Vec<f64> = (0..k).map(|j| rows.iter().map(|r| r[j + 1]).sum::<f64>() / n as f64).collect();
    // Centering keeps the normal equations well conditioned.
    let xm: Vec<Vec<f64>> = rows.iter().map(|r| (0..k).map(|j| r[j + 1] - if konst { xbar[j] } else { 0.0 }).collect()).collect();
    let ym: Vec<f64> = yv.iter().map(|v| v - if konst { ybar } else { 0.0 }).collect();
    let ls = numeric::least_squares(&xm, &ym).ok_or(StatsError::NotEnoughData)?;
    let rank = ls.kept.iter().filter(|x| **x).count();
    let b0 = if konst { ybar - (0..k).map(|j| ls.coef[j] * xbar[j]).sum::<f64>() } else { 0.0 };
    let pred: Vec<f64> = rows.iter().map(|r| b0 + (0..k).map(|j| ls.coef[j] * r[j + 1]).sum::<f64>()).collect();
    let resid: Vec<f64> = yv.iter().zip(&pred).map(|(a, p)| a - p).collect();
    let sse: f64 = resid.iter().map(|e| e * e).sum();
    let sst: f64 = if konst { dev_sq(&yv) } else { yv.iter().map(|v| v * v).sum() };
    let ssr = sst - sse;
    let df_reg = rank as f64;
    let df_res = n as f64 - rank as f64 - konst as u8 as f64;
    if df_res < 1.0 {
        return Err(StatsError::NotEnoughData);
    }
    let r2 = if sst > 0.0 { ssr / sst } else { f64::NAN };
    let adj = 1.0 - (1.0 - r2) * (n as f64 - konst as u8 as f64) / df_res;
    let s2 = sse / df_res;
    let f = (ssr / df_reg) / s2;
    let sig_f = numeric::f_right(f, df_reg, df_res);

    let mut out = Table::new();
    out.push(vec![t(lang.text("summary_output"))]);
    out.blank();
    out.push(vec![t(lang.text("regression_statistics"))]);
    out.push(vec![t(lang.text("multiple_r")), num(r2.sqrt())]);
    out.push(vec![t(lang.text("r_square")), num(r2)]);
    out.push(vec![t(lang.text("adjusted_r_square")), num(adj)]);
    out.push(vec![t(lang.text("std_error")), num(s2.sqrt())]);
    out.push(vec![t(lang.text("observations")), num(n as f64)]);
    out.blank();
    out.push(vec![t(lang.text("anova"))]);
    out.push(["", "df", "ss", "ms", "f", "significance_f"].iter().map(|k| if k.is_empty() { Value::Empty } else { t(lang.text(k)) }).collect());
    out.push(vec![t(lang.text("regression")), num(df_reg), num(ssr), num(ssr / df_reg), num(f), num(sig_f)]);
    out.push(vec![t(lang.text("residual")), num(df_res), num(sse), num(s2)]);
    out.push(vec![t(lang.text("total")), num(df_reg + df_res), num(sst)]);
    out.blank();
    let cl = o.confidence;
    let mut head = vec![Value::Empty];
    for k in ["coefficients", "std_error", "t_stat", "p_value"] {
        head.push(t(lang.text(k)));
    }
    head.push(t(lang.fmt("lower_pct", &["95"])));
    head.push(t(lang.fmt("upper_pct", &["95"])));
    head.push(t(lang.fmt("lower_pct", &[&lang.pct1(cl)])));
    head.push(t(lang.fmt("upper_pct", &[&lang.pct1(cl)])));
    out.push(head);
    let t95 = numeric::t_inv_2t(0.05, df_res);
    let tcl = numeric::t_inv_2t(1.0 - cl / 100.0, df_res);
    let coef_row = |name: String, b: f64, se: f64| -> Vec<Value> {
        let ts = b / se;
        let p = 2.0 * numeric::t_right(ts.abs(), df_res);
        vec![t(name), num(b), num(se), num(ts), num(p), num(b - t95 * se), num(b + t95 * se), num(b - tcl * se), num(b + tcl * se)]
    };
    if konst {
        let mut q = 0.0;
        for i in 0..k {
            for j in 0..k {
                q += xbar[i] * ls.xtx_inv[i][j] * xbar[j];
            }
        }
        out.push(coef_row(lang.text("intercept"), b0, (s2 * (1.0 / n as f64 + q)).sqrt()));
    } else {
        let mut row = vec![t(lang.text("intercept")), num(0.0)];
        row.extend((0..7).map(|_| na()));
        out.push(row);
    }
    for (j, x) in xs.iter().enumerate() {
        let se = (s2 * ls.xtx_inv[j][j]).sqrt();
        out.push(coef_row(x.name(lang, "x_variable_n", j), ls.coef[j], se));
    }
    if o.residuals || o.standardized {
        out.blank();
        out.blank();
        out.blank();
        out.push(vec![t(lang.text("residual_output"))]);
        out.blank();
        let yname = y.label.clone().unwrap_or_else(|| lang.text("y"));
        let mut head = vec![t(lang.text("observation")), t(lang.fmt("predicted_y", &[&yname]))];
        if o.residuals {
            head.push(t(lang.text("residuals")));
        }
        if o.standardized {
            head.push(t(lang.text("standard_residuals")));
        }
        out.push(head);
        // Excel scales by the residuals' standard deviation (n - 1).
        let sd = (sse / (n as f64 - 1.0)).sqrt();
        for i in 0..n {
            let mut row = vec![num((i + 1) as f64), num(pred[i])];
            if o.residuals {
                row.push(num(resid[i]));
            }
            if o.standardized {
                row.push(num(resid[i] / sd));
            }
            out.push(row);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Histogram

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HistogramOpts {
    /// Also list the bins sorted by frequency (Pareto).
    pub pareto: bool,
    pub cumulative: bool,
}

/// Bins spread evenly between the minimum and the maximum when none are
/// given: about sqrt(n) of them, the last one at the maximum.
pub fn auto_bins(data: &[f64]) -> Vec<f64> {
    let s = sorted(data);
    let (Some(&min), Some(&max)) = (s.first(), s.last()) else { return Vec::new() };
    if min == max {
        return vec![min];
    }
    let k = ((s.len() as f64).sqrt().floor() as usize).clamp(1, 1000);
    let w = (max - min) / k as f64;
    (0..=k).map(|i| if i == k { max } else { min + w * i as f64 }).collect()
}

/// Frequency table: `Bin, Frequency[, Cumulative %]` with a final "More"
/// row, plus the Pareto (sorted) copy to the right.
pub fn histogram(data: &[f64], bins: Option<&[f64]>, bin_label: Option<&str>, o: &HistogramOpts, lang: Lang) -> Result<Table, StatsError> {
    let data: Vec<f64> = data.iter().copied().filter(|v| v.is_finite()).collect();
    let bins: Vec<f64> = match bins {
        Some(b) => sorted(&b.iter().copied().filter(|v| v.is_finite()).collect::<Vec<_>>()),
        None => auto_bins(&data),
    };
    if bins.is_empty() {
        return Err(StatsError::NotEnoughData);
    }
    let mut counts = vec![0usize; bins.len() + 1];
    for v in &data {
        let i = bins.partition_point(|b| b < v);
        counts[i] += 1;
    }
    let total = data.len().max(1) as f64;
    let head = |out: &mut Table| {
        let mut h = vec![t(bin_label.map_or_else(|| lang.text("bin"), str::to_string)), t(lang.text("frequency"))];
        if o.cumulative {
            h.push(t(lang.text("cumulative_pct")));
        }
        out.push(h);
    };
    let label = |i: usize| if i < bins.len() { num(bins[i]) } else { t(lang.text("more")) };
    let mut out = Table::new();
    head(&mut out);
    let mut acc = 0usize;
    for (i, c) in counts.iter().enumerate() {
        acc += c;
        let mut row = vec![label(i), num(*c as f64)];
        if o.cumulative {
            row.push(num(acc as f64 / total));
            out.set_format(out.height(), 2, "0.00%");
        }
        out.push(row);
    }
    if o.pareto {
        let mut order: Vec<usize> = (0..counts.len()).collect();
        order.sort_by(|a, b| counts[*b].cmp(&counts[*a]));
        let mut p = Table::new();
        head(&mut p);
        let mut acc = 0usize;
        for i in order {
            acc += counts[i];
            let mut row = vec![label(i), num(counts[i] as f64)];
            if o.cumulative {
                row.push(num(acc as f64 / total));
                p.set_format(p.height(), 2, "0.00%");
            }
            p.push(row);
        }
        out.beside(p, 0);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// t-Tests, F-Test, ANOVA

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TTestKind {
    Paired,
    EqualVariance,
    UnequalVariance,
}

/// Two-sample t-test for means.
pub fn t_test(a: &Series, b: &Series, kind: TTestKind, hyp_diff: f64, alpha: f64, lang: Lang) -> Result<Table, StatsError> {
    if !(0.0 < alpha && alpha < 1.0) {
        return Err(StatsError::BadParameter);
    }
    let (x, y) = if kind == TTestKind::Paired {
        if a.values.len() != b.values.len() {
            return Err(StatsError::SizeMismatch);
        }
        complete_rows(&[a, b]).into_iter().map(|r| (r[0], r[1])).unzip()
    } else {
        (a.nums(), b.nums())
    };
    let (n1, n2) = (x.len() as f64, y.len() as f64);
    if n1 < 2.0 || n2 < 2.0 {
        return Err(StatsError::NotEnoughData);
    }
    let (m1, m2, v1, v2) = (mean(&x), mean(&y), var_s(&x), var_s(&y));
    let mut out = Table::new();
    let title = match kind {
        TTestKind::Paired => "t_paired",
        TTestKind::EqualVariance => "t_equal",
        TTestKind::UnequalVariance => "t_unequal",
    };
    out.push(vec![t(lang.text(title))]);
    out.blank();
    out.push(vec![Value::Empty, t(a.name(lang, "variable_n", 0)), t(b.name(lang, "variable_n", 1))]);
    out.push(vec![t(lang.text("mean")), num(m1), num(m2)]);
    out.push(vec![t(lang.text("variance")), num(v1), num(v2)]);
    out.push(vec![t(lang.text("observations")), num(n1), num(n2)]);
    let (ts, df) = match kind {
        TTestKind::Paired => {
            out.push(vec![t(lang.text("pearson")), num(correl(&x, &y))]);
            let d: Vec<f64> = x.iter().zip(&y).map(|(p, q)| p - q).collect();
            ((mean(&d) - hyp_diff) / (var_s(&d) / n1).sqrt(), n1 - 1.0)
        }
        TTestKind::EqualVariance => {
            let pooled = ((n1 - 1.0) * v1 + (n2 - 1.0) * v2) / (n1 + n2 - 2.0);
            out.push(vec![t(lang.text("pooled_variance")), num(pooled)]);
            ((m1 - m2 - hyp_diff) / (pooled * (1.0 / n1 + 1.0 / n2)).sqrt(), n1 + n2 - 2.0)
        }
        TTestKind::UnequalVariance => {
            let (a1, a2) = (v1 / n1, v2 / n2);
            // The ToolPak rounds Welch's degrees of freedom.
            let df = ((a1 + a2).powi(2) / (a1 * a1 / (n1 - 1.0) + a2 * a2 / (n2 - 1.0))).round();
            ((m1 - m2 - hyp_diff) / (a1 + a2).sqrt(), df)
        }
    };
    out.push(vec![t(lang.text("hyp_mean_diff")), num(hyp_diff)]);
    out.push(vec![t(lang.text("df")), num(df)]);
    out.push(vec![t(lang.text("t_stat")), num(ts)]);
    let p1 = numeric::t_right(ts.abs(), df);
    out.push(vec![t(lang.text("p_one_tail")), num(p1)]);
    out.push(vec![t(lang.text("t_crit_one_tail")), num(numeric::t_inv_right(alpha, df))]);
    out.push(vec![t(lang.text("p_two_tail")), num(2.0 * p1)]);
    out.push(vec![t(lang.text("t_crit_two_tail")), num(numeric::t_inv_2t(alpha, df))]);
    Ok(out)
}

/// F-Test two-sample for variances.
pub fn f_test(a: &Series, b: &Series, alpha: f64, lang: Lang) -> Result<Table, StatsError> {
    if !(0.0 < alpha && alpha < 1.0) {
        return Err(StatsError::BadParameter);
    }
    let (x, y) = (a.nums(), b.nums());
    if x.len() < 2 || y.len() < 2 {
        return Err(StatsError::NotEnoughData);
    }
    let (v1, v2) = (var_s(&x), var_s(&y));
    let (d1, d2) = (x.len() as f64 - 1.0, y.len() as f64 - 1.0);
    let f = v1 / v2;
    // Below 1 the ToolPak reports the left tail.
    let (p, crit) = if f < 1.0 { (numeric::f_cdf(f, d1, d2), numeric::f_inv(alpha, d1, d2)) } else { (numeric::f_right(f, d1, d2), numeric::f_inv_rt(alpha, d1, d2)) };
    let mut out = Table::new();
    out.push(vec![t(lang.text("f_test"))]);
    out.blank();
    out.push(vec![Value::Empty, t(a.name(lang, "variable_n", 0)), t(b.name(lang, "variable_n", 1))]);
    out.push(vec![t(lang.text("mean")), num(mean(&x)), num(mean(&y))]);
    out.push(vec![t(lang.text("variance")), num(v1), num(v2)]);
    out.push(vec![t(lang.text("observations")), num(x.len() as f64), num(y.len() as f64)]);
    out.push(vec![t(lang.text("df")), num(d1), num(d2)]);
    out.push(vec![t(lang.text("f")), num(f)]);
    out.push(vec![t(lang.text("pf_one_tail")), num(p)]);
    out.push(vec![t(lang.text("f_crit_one_tail")), num(crit)]);
    Ok(out)
}

/// Anova: single factor.
pub fn anova_single(groups: &[Series], alpha: f64, lang: Lang, by_rows: bool) -> Result<Table, StatsError> {
    if !(0.0 < alpha && alpha < 1.0) {
        return Err(StatsError::BadParameter);
    }
    let data: Vec<Vec<f64>> = groups.iter().map(Series::nums).collect();
    let n: usize = data.iter().map(Vec::len).sum();
    let k = data.iter().filter(|g| !g.is_empty()).count();
    if k < 2 || n <= k {
        return Err(StatsError::NotEnoughData);
    }
    let all: Vec<f64> = data.iter().flatten().copied().collect();
    let gm = mean(&all);
    let ssb: f64 = data.iter().filter(|g| !g.is_empty()).map(|g| g.len() as f64 * (mean(g) - gm).powi(2)).sum();
    let ssw: f64 = data.iter().map(|g| dev_sq(g)).filter(|v| v.is_finite()).sum();
    let (dfb, dfw) = ((k - 1) as f64, (n - k) as f64);
    let (msb, msw) = (ssb / dfb, ssw / dfw);
    let f = msb / msw;
    let mut out = Table::new();
    out.push(vec![t(lang.text("anova_single"))]);
    out.blank();
    out.push(vec![t(lang.text("summary"))]);
    out.push(["groups", "count", "sum", "average", "variance"].iter().map(|k| t(lang.text(k))).collect());
    for (i, (g, s)) in data.iter().zip(groups).enumerate() {
        let key = if by_rows { "row_n" } else { "column_n" };
        out.push(vec![t(s.name(lang, key, i)), num(g.len() as f64), num(g.iter().sum()), num(mean(g)), num(var_s(g))]);
    }
    out.blank();
    out.blank();
    out.push(vec![t(lang.text("anova"))]);
    out.push(["source_of_variation", "ss", "df", "ms", "f", "p_value", "f_crit"].iter().map(|k| t(lang.text(k))).collect());
    out.push(vec![t(lang.text("between_groups")), num(ssb), num(dfb), num(msb), num(f), num(numeric::f_right(f, dfb, dfw)), num(numeric::f_inv_rt(alpha, dfb, dfw))]);
    out.push(vec![t(lang.text("within_groups")), num(ssw), num(dfw), num(msw)]);
    out.blank();
    out.push(vec![t(lang.text("total")), num(ssb + ssw), num(dfb + dfw)]);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Moving average, exponential smoothing

/// Moving average over `interval` points; with `std_errors`, a second
/// column like the ToolPak's (#N/A until enough points).
pub fn moving_average(data: &[Option<f64>], interval: usize, std_errors: bool) -> Result<Table, StatsError> {
    if interval < 2 {
        return Err(StatsError::BadParameter);
    }
    if data.len() < interval {
        return Err(StatsError::NotEnoughData);
    }
    let x: Vec<f64> = data.iter().map(|v| v.unwrap_or(f64::NAN)).collect();
    let f: Vec<f64> = (0..x.len()).map(|i| if i + 1 < interval { f64::NAN } else { mean(&x[i + 1 - interval..=i]) }).collect();
    Ok(smoothing_table(&x, &f, if std_errors { Some(interval) } else { None }, interval - 1))
}

/// Exponential smoothing with a damping factor (1 - alpha).
pub fn exp_smoothing(data: &[Option<f64>], damping: f64, std_errors: bool) -> Result<Table, StatsError> {
    if !(0.0..=1.0).contains(&damping) {
        return Err(StatsError::BadParameter);
    }
    if data.len() < 2 {
        return Err(StatsError::NotEnoughData);
    }
    let x: Vec<f64> = data.iter().map(|v| v.unwrap_or(f64::NAN)).collect();
    let mut f = vec![f64::NAN; x.len()];
    f[1] = x[0];
    for i in 2..x.len() {
        f[i] = (1.0 - damping) * x[i - 1] + damping * f[i - 1];
    }
    Ok(smoothing_table(&x, &f, if std_errors { Some(3) } else { None }, 1))
}

/// Forecast column (and standard errors over a window) as a table.
fn smoothing_table(x: &[f64], f: &[f64], window: Option<usize>, first: usize) -> Table {
    let mut out = Table::new();
    for i in 0..x.len() {
        let mut row = vec![if i < first { na() } else { num(f[i]) }];
        if let Some(w) = window {
            let start = (i + 1).checked_sub(w);
            let se = match start {
                Some(s) if s >= first => (x[s..=i].iter().zip(&f[s..=i]).map(|(a, b)| (a - b).powi(2)).sum::<f64>() / w as f64).sqrt(),
                _ => f64::NAN,
            };
            row.push(if se.is_nan() { na() } else { num(se) });
        }
        out.push(row);
    }
    out
}

// ---------------------------------------------------------------------------
// Rank and percentile

/// Point, value, rank and percent rank of every value, largest first.
pub fn rank_percentile(series: &[Series], lang: Lang) -> Result<Table, StatsError> {
    if series.is_empty() {
        return Err(StatsError::NotEnoughData);
    }
    let mut out = Table::new();
    for (si, s) in series.iter().enumerate() {
        let pts: Vec<(usize, f64)> = s.values.iter().enumerate().filter_map(|(i, v)| v.filter(|x| x.is_finite()).map(|x| (i + 1, x))).collect();
        let n = pts.len();
        let mut order = pts.clone();
        order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut tb = Table::new();
        tb.push(vec![t(lang.text("point")), t(s.name(lang, "column_n", si)), t(lang.text("rank")), t(lang.text("percent"))]);
        for (p, v) in &order {
            let greater = pts.iter().filter(|x| x.1 > *v).count();
            let less = pts.iter().filter(|x| x.1 < *v).count();
            // PERCENTRANK.INC truncates to three decimals.
            let pr = if n > 1 { ((less as f64 / (n - 1) as f64) * 1000.0 + 1e-9).floor() / 1000.0 } else { 1.0 };
            tb.set_format(tb.height(), 3, "0.00%");
            tb.push(vec![num(*p as f64), num(*v), num((greater + 1) as f64), num(pr)]);
        }
        if si == 0 {
            out = tb;
        } else {
            out.beside(tb, 0);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Random numbers and sampling

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RandomDist {
    Uniform { from: f64, to: f64 },
    Normal { mean: f64, sd: f64 },
    Bernoulli { p: f64 },
    Binomial { p: f64, trials: u64 },
    Poisson { lambda: f64 },
}

/// A `count` x `vars` block of random numbers; the same seed gives the
/// same numbers.
pub fn random_numbers(vars: usize, count: usize, dist: RandomDist, seed: Option<u64>) -> Result<Table, StatsError> {
    let ok = match dist {
        RandomDist::Uniform { from, to } => from.is_finite() && to.is_finite() && from <= to,
        RandomDist::Normal { mean, sd } => mean.is_finite() && sd.is_finite() && sd >= 0.0,
        RandomDist::Bernoulli { p } | RandomDist::Binomial { p, .. } => (0.0..=1.0).contains(&p),
        RandomDist::Poisson { lambda } => lambda.is_finite() && lambda > 0.0,
    };
    if !ok || vars == 0 || count == 0 || vars.saturating_mul(count) > 10_000_000 || count > crate::MAX_ROWS as usize || vars > crate::MAX_COLS as usize {
        return Err(StatsError::BadParameter);
    }
    let mut rng = seed.map_or_else(Rng::from_time, Rng::new);
    let mut out = Table::new();
    for _ in 0..count {
        let row = (0..vars)
            .map(|_| {
                num(match dist {
                    RandomDist::Uniform { from, to } => from + (to - from) * rng.uniform(),
                    RandomDist::Normal { mean, sd } => mean + sd * rng.normal(),
                    RandomDist::Bernoulli { p } => rng.bernoulli(p),
                    RandomDist::Binomial { p, trials } => rng.binomial(trials, p),
                    RandomDist::Poisson { lambda } => rng.poisson(lambda),
                })
            })
            .collect();
        out.push(row);
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleMethod {
    /// Every `period`-th value.
    Periodic(usize),
    /// `n` values picked at random (with replacement, like the ToolPak).
    Random(usize),
}

/// A sample of `data` as one column.
pub fn sampling(data: &[f64], method: SampleMethod, seed: Option<u64>) -> Result<Table, StatsError> {
    if data.is_empty() {
        return Err(StatsError::NotEnoughData);
    }
    let picked: Vec<f64> = match method {
        SampleMethod::Periodic(k) if k >= 1 && k <= data.len() => data.iter().skip(k - 1).step_by(k).copied().collect(),
        SampleMethod::Random(n) if n >= 1 && n <= crate::MAX_ROWS as usize => {
            let mut rng = seed.map_or_else(Rng::from_time, Rng::new);
            (0..n).map(|_| data[rng.below(data.len())]).collect()
        }
        _ => return Err(StatsError::BadParameter),
    };
    let mut out = Table::new();
    for v in picked {
        out.push(vec![num(v)]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(v: &Value, want: f64, tol: f64) {
        match v {
            Value::Number(n) => assert!((n - want).abs() <= tol * want.abs().max(1.0), "{n} vs {want}"),
            other => panic!("{other:?} vs {want}"),
        }
    }

    /// Row whose first cell is `label`.
    fn row<'a>(tb: &'a Table, label: &str) -> &'a [Value] {
        tb.rows.iter().find(|r| r.first() == Some(&Value::text(label))).map(|r| &r[..]).unwrap_or_else(|| panic!("no row {label}"))
    }

    #[test]
    fn descriptive_matches_excel() {
        let s = Series::new(Some("Data"), &[3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0]);
        let tb = descriptive(&[s], &DescriptiveOpts { kth_largest: Some(2), kth_smallest: Some(9), ..Default::default() }, Lang::En).unwrap();
        assert_eq!(tb.get(0, 0), &Value::text("Data"));
        close(&row(&tb, "Mean")[1], 3.875, 1e-12);
        close(&row(&tb, "Standard Error")[1], 0.9716977, 1e-6);
        close(&row(&tb, "Median")[1], 3.5, 1e-12);
        close(&row(&tb, "Mode")[1], 1.0, 0.0);
        close(&row(&tb, "Standard Deviation")[1], 2.7483761, 1e-6);
        close(&row(&tb, "Kurtosis")[1], 0.2766058, 1e-6);
        close(&row(&tb, "Range")[1], 8.0, 0.0);
        close(&row(&tb, "Count")[1], 8.0, 0.0);
        close(&row(&tb, "Largest(2)")[1], 6.0, 0.0);
        assert!(matches!(row(&tb, "Smallest(9)")[1], Value::Error(_)));
        close(&row(&tb, "Confidence Level(95.0%)")[1], 2.364624252 * 0.9716977043, 1e-6);
        // Two series side by side; no repeats gives #N/A for the mode.
        let tb = descriptive(&[Series::new(None, &[1.0, 2.0]), Series::new(None, &[5.0, 7.0, 7.0])], &DescriptiveOpts::default(), Lang::Es).unwrap();
        assert_eq!(tb.get(0, 2), &Value::text("Columna 2"));
        assert_eq!(tb.get(5, 1), &na());
        close(tb.get(5, 3), 7.0, 0.0);
    }

    #[test]
    fn correlation_and_covariance() {
        let a = Series::new(Some("A"), &[1.0, 2.0, 3.0, 4.0]);
        let b = Series::new(Some("B"), &[2.0, 4.0, 5.0, 9.0]);
        let tb = correlation(&[a.clone(), b.clone()], Lang::En).unwrap();
        close(tb.get(1, 1), 1.0, 0.0);
        close(tb.get(2, 1), 0.9647638, 1e-6);
        close(tb.get(2, 2), 1.0, 0.0);
        assert_eq!(tb.get(1, 2), &Value::Empty);
        let tb = covariance(&[a, b], Lang::En).unwrap();
        close(tb.get(1, 1), 1.25, 1e-12);
        close(tb.get(2, 1), 2.75, 1e-12);
    }

    #[test]
    fn regression_matches_linest() {
        // y = 1 + 2 x1 + noise, x2 unrelated.
        let x1 = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
        let x2 = [2.0, 1.0, 4.0, 3.0, 6.0, 5.0, 7.0];
        let y = [3.1, 4.9, 7.2, 8.8, 11.1, 13.2, 14.8];
        let tb = regression(&Series::new(None, &y), &[Series::new(None, &x1), Series::new(None, &x2)], &RegressionOpts { residuals: true, standardized: true, ..Default::default() }, Lang::En).unwrap();
        close(&row(&tb, "Observations")[1], 7.0, 0.0);
        // Reference values from LINEST(y, x1:x2, TRUE, TRUE).
        close(&row(&tb, "Intercept")[1], 1.03692722, 1e-7);
        close(&row(&tb, "Intercept")[2], 0.16511335, 1e-7);
        close(&row(&tb, "X Variable 1")[1], 1.91383648, 1e-7);
        close(&row(&tb, "X Variable 2")[1], 0.08050314, 1e-7);
        close(&row(&tb, "X Variable 2")[2], 0.08018806, 1e-7);
        close(&row(&tb, "R Square")[1], 0.99867929, 1e-7);
        let reg = row(&tb, "Regression");
        close(&reg[1], 2.0, 0.0);
        let res = row(&tb, "Residual");
        close(&res[1], 4.0, 0.0);
        assert!(tb.rows.iter().any(|r| r.first() == Some(&Value::text("RESIDUAL OUTPUT"))));
        // A simple case checked by hand: exact fit except one point.
        let tb = regression(&Series::new(None, &[1.0, 3.0, 2.0, 5.0]), &[Series::new(None, &[1.0, 2.0, 3.0, 4.0])], &RegressionOpts::default(), Lang::En).unwrap();
        close(&row(&tb, "X Variable 1")[1], 1.1, 1e-12);
        assert!(row(&tb, "Intercept")[1].as_number().unwrap().abs() < 1e-12);
        close(&row(&tb, "Intercept")[2], 1.42302495, 1e-7);
        close(&row(&tb, "X Variable 1")[2], 0.51961524, 1e-7);
        close(&row(&tb, "X Variable 1")[3], 2.11695099, 1e-7);
        close(&row(&tb, "X Variable 1")[4], 0.16847816, 1e-6);
        let anova = row(&tb, "Regression");
        close(&anova[4], 4.48148148, 1e-7);
        close(&anova[5], 0.16847816, 1e-6);
        assert_eq!(regression(&Series::new(None, &[1.0]), &[Series::new(None, &[1.0])], &RegressionOpts::default(), Lang::En), Err(StatsError::NotEnoughData));
    }

    #[test]
    fn regression_through_origin() {
        let tb = regression(&Series::new(None, &[2.0, 4.1, 5.9]), &[Series::new(None, &[1.0, 2.0, 3.0])], &RegressionOpts { constant_zero: true, ..Default::default() }, Lang::En).unwrap();
        close(&row(&tb, "Intercept")[1], 0.0, 0.0);
        assert_eq!(row(&tb, "Intercept")[2], na());
        close(&row(&tb, "X Variable 1")[1], 27.9 / 14.0, 1e-12);
    }

    #[test]
    fn histogram_counts() {
        let data = [1.0, 2.0, 2.0, 3.0, 5.0, 8.0, 9.0];
        let tb = histogram(&data, Some(&[2.0, 5.0]), None, &HistogramOpts { pareto: true, cumulative: true }, Lang::En).unwrap();
        assert_eq!(tb.rows[0][0], Value::text("Bin"));
        close(&tb.rows[1][1], 3.0, 0.0);
        close(&tb.rows[2][1], 2.0, 0.0);
        assert_eq!(tb.rows[3][0], Value::text("More"));
        close(&tb.rows[3][2], 1.0, 1e-12);
        // Pareto: largest count first.
        close(&tb.rows[1][4], 3.0, 0.0);
        assert!(tb.formats.iter().any(|((r, c), _)| (*r, *c) == (1, 2)));
        let auto = auto_bins(&data);
        assert_eq!(auto.first(), Some(&1.0));
        assert_eq!(auto.last(), Some(&9.0));
        assert!(histogram(&[], None, None, &HistogramOpts::default(), Lang::En).is_err());
    }

    #[test]
    fn t_tests_match_excel() {
        let a = Series::new(None, &[10.0, 12.0, 9.0, 11.0, 13.0]);
        let b = Series::new(None, &[8.0, 11.0, 9.0, 10.0, 10.0]);
        let tb = t_test(&a, &b, TTestKind::Paired, 0.0, 0.05, Lang::En).unwrap();
        close(&row(&tb, "t Stat")[1], 2.74562589, 1e-7);
        close(&row(&tb, "P(T<=t) two-tail")[1], 0.05160596, 1e-6);
        close(&row(&tb, "t Critical two-tail")[1], 2.776445, 1e-5);
        let tb = t_test(&a, &b, TTestKind::EqualVariance, 0.0, 0.05, Lang::En).unwrap();
        close(&row(&tb, "Pooled Variance")[1], 1.9, 1e-12);
        close(&row(&tb, "t Stat")[1], 1.60591014, 1e-7);
        close(&row(&tb, "P(T<=t) two-tail")[1], 0.14696093, 1e-6);
        let tb = t_test(&a, &b, TTestKind::UnequalVariance, 0.0, 0.05, Lang::En).unwrap();
        close(&row(&tb, "df")[1], 7.0, 0.0);
        let tb = f_test(&a, &b, 0.05, Lang::En).unwrap();
        close(&row(&tb, "F")[1], 2.5 / 1.3, 1e-12);
        close(&row(&tb, "P(F<=f) one-tail")[1], 0.27103076, 1e-6);
        close(&row(&tb, "F Critical one-tail")[1], 6.388233, 1e-5);
    }

    #[test]
    fn anova_matches_excel() {
        let g = [Series::new(None, &[6.0, 8.0, 4.0, 5.0, 3.0, 4.0]), Series::new(None, &[8.0, 12.0, 9.0, 11.0, 6.0, 8.0]), Series::new(None, &[13.0, 9.0, 11.0, 8.0, 7.0, 12.0])];
        let tb = anova_single(&g, 0.05, Lang::En, false).unwrap();
        let bg = row(&tb, "Between Groups");
        close(&bg[1], 84.0, 1e-9);
        close(&bg[2], 2.0, 0.0);
        close(&bg[4], 9.264706, 1e-5);
        close(&bg[5], 0.00239878, 1e-6);
        close(&bg[6], 3.682320, 1e-5);
        close(&row(&tb, "Within Groups")[1], 68.0, 1e-9);
        close(&row(&tb, "Total")[2], 17.0, 0.0);
    }

    #[test]
    fn smoothing_and_moving_average() {
        let d: Vec<Option<f64>> = [1.0, 2.0, 3.0, 4.0, 5.0].iter().map(|v| Some(*v)).collect();
        let tb = moving_average(&d, 3, true).unwrap();
        assert_eq!(tb.rows[1][0], na());
        close(&tb.rows[2][0], 2.0, 0.0);
        assert_eq!(tb.rows[3][1], na());
        close(&tb.rows[4][1], 1.0, 1e-12);
        let tb = exp_smoothing(&d, 0.3, true).unwrap();
        assert_eq!(tb.rows[0][0], na());
        close(&tb.rows[1][0], 1.0, 0.0);
        close(&tb.rows[2][0], 0.7 * 2.0 + 0.3 * 1.0, 1e-12);
        assert_eq!(tb.rows[2][1], na());
        assert!(matches!(tb.rows[3][1], Value::Number(_)));
        assert!(moving_average(&d, 1, false).is_err());
    }

    #[test]
    fn rank_and_percentile() {
        let tb = rank_percentile(&[Series::new(None, &[3.0, 9.0, 3.0, 5.0])], Lang::En).unwrap();
        assert_eq!(tb.rows[1][..3], [num(2.0), num(9.0), num(1.0)]);
        assert_eq!(tb.rows[3][..3], [num(1.0), num(3.0), num(3.0)]);
        assert_eq!(tb.rows[4][..3], [num(3.0), num(3.0), num(3.0)]);
        close(&tb.rows[2][3], 0.666, 1e-12);
        close(&tb.rows[3][3], 0.0, 0.0);
    }

    #[test]
    fn random_and_sampling() {
        let a = random_numbers(2, 50, RandomDist::Uniform { from: 5.0, to: 6.0 }, Some(1)).unwrap();
        let b = random_numbers(2, 50, RandomDist::Uniform { from: 5.0, to: 6.0 }, Some(1)).unwrap();
        assert_eq!(a, b);
        assert!(a.rows.iter().flatten().all(|v| matches!(v, Value::Number(n) if (5.0..6.0).contains(n))));
        let bern = random_numbers(1, 100, RandomDist::Bernoulli { p: 0.5 }, Some(3)).unwrap();
        assert!(bern.rows.iter().all(|r| r[0] == num(0.0) || r[0] == num(1.0)));
        assert!(random_numbers(1, 1, RandomDist::Poisson { lambda: -1.0 }, None).is_err());
        let s = sampling(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0], SampleMethod::Periodic(3), None).unwrap();
        assert_eq!(s.rows, vec![vec![num(3.0)], vec![num(6.0)]]);
        let s = sampling(&[1.0, 2.0], SampleMethod::Random(10), Some(9)).unwrap();
        assert_eq!(s.height(), 10);
    }
}
