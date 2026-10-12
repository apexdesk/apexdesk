//! Spreadsheet functions: information and engineering.

use super::*;

pub(super) fn array_params(f: Func) -> Option<u64> {
    use Func::*;
    match f {
        // These look at the argument itself (its type, or that it is a reference).
        Type | IsRef | IsFormula | SheetNum | SheetsNum => Some(ALL),
        CellInfo => Some(0b10),
        _ => None,
    }
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    use Func::*;
    match f {
        IsEven | IsOdd | Type | ErrorType | CellInfo | IsLogical | IsNonText | IsRef | IsErr | IsFormula | SheetNum | SheetsNum => Some(info(env, f, a)),
        Bin2Dec | Bin2Hex | Bin2Oct | Dec2Bin | Dec2Hex | Dec2Oct | Hex2Bin | Hex2Dec | Hex2Oct | Oct2Bin | Oct2Dec | Oct2Hex | Convert | Delta | GeStep | BitAnd | BitOr | BitXor | BitLShift | BitRShift | Erf | Erfc => Some(eng(env, f, a)),
        _ => None,
    }
}

fn need(a: &[Expr], min: usize, max: usize) -> R<()> {
    if a.len() < min || a.len() > max { Err(ErrorKind::Value) } else { Ok(()) }
}

// ---------------------------------------------------------------------------
// Information

fn info(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    use Func::*;
    Ok(match f {
        IsEven | IsOdd => {
            need(a, 1, 1)?;
            let x = match env.scalar(&a[0]) {
                Value::Bool(_) => return Err(ErrorKind::Value),
                v => v.to_number()?,
            };
            Value::Bool((x.trunc() % 2.0 != 0.0) == (f == IsOdd))
        }
        Type => {
            need(a, 1, 1)?;
            Value::Number(match env.eval(&a[0]) {
                Value::Empty | Value::Number(_) => 1.0,
                Value::Text(_) => 2.0,
                Value::Bool(_) => 4.0,
                Value::Error(_) => 16.0,
                Value::Array(_) => 64.0,
                Value::Lambda(_) => 128.0,
            })
        }
        ErrorType => {
            need(a, 1, 1)?;
            let Value::Error(k) = env.scalar(&a[0]) else { return Err(ErrorKind::Na) };
            Value::Number(match k {
                ErrorKind::Null => 1.0,
                ErrorKind::Div0 => 2.0,
                ErrorKind::Value => 3.0,
                ErrorKind::Ref => 4.0,
                ErrorKind::Name => 5.0,
                ErrorKind::Num => 6.0,
                ErrorKind::Na => 7.0,
                ErrorKind::Spill => 9.0,
                ErrorKind::Connect => 10.0,
                ErrorKind::Calc => 14.0,
                // Excel has no circular-reference error value.
                ErrorKind::Circular => return Err(ErrorKind::Na),
            })
        }
        IsLogical | IsNonText | IsErr => {
            need(a, 1, 1)?;
            let v = env.scalar(&a[0]);
            Value::Bool(match f {
                IsLogical => matches!(v, Value::Bool(_)),
                IsNonText => !matches!(v, Value::Text(_)),
                _ => matches!(v, Value::Error(k) if k != ErrorKind::Na),
            })
        }
        IsRef => {
            need(a, 1, 1)?;
            Value::Bool(matches!(env.reference(&a[0]), Some(Ok(_))))
        }
        IsFormula => {
            need(a, 1, 1)?;
            let (s, r) = env.reference(&a[0]).ok_or(ErrorKind::Value)??;
            if r.cell_count() > MAX_ARRAY as u64 {
                return Err(ErrorKind::Num);
            }
            let data = (r.start.row..=r.end.row)
                .flat_map(|row| (r.start.col..=r.end.col).map(move |col| CellRef::new(row, col)))
                .map(|c| Value::Bool(env.ctx.formula_text(s, c).is_some()))
                .collect();
            Matrix::new(r.rows() as usize, r.cols() as usize, data).into_value()
        }
        SheetNum => {
            need(a, 0, 1)?;
            let s = match a.first() {
                None | Some(Expr::Missing) => env.sheet,
                Some(e) => match env.reference(e) {
                    Some(r) => r?.0,
                    None => match env.eval(e) {
                        Value::Error(k) => return Err(k),
                        Value::Text(t) => env.ctx.sheet_index(&t).ok_or(ErrorKind::Na)?,
                        Value::Array(_) => return Err(ErrorKind::Value),
                        _ => return Err(ErrorKind::Na),
                    },
                },
            };
            Value::Number(s as f64 + 1.0)
        }
        SheetsNum => {
            need(a, 0, 1)?;
            match a.first() {
                None | Some(Expr::Missing) => Value::Number(env.ctx.sheet_count() as f64),
                // References span a single sheet (no 3-D references).
                Some(e) => match env.reference(e) {
                    Some(r) => r.map(|_| Value::Number(1.0))?,
                    None => return Err(ErrorKind::Ref),
                },
            }
        }
        CellInfo => {
            need(a, 1, 2)?;
            let what = env.text(&a[0])?.to_lowercase();
            let (s, c) = match a.get(1) {
                None | Some(Expr::Missing) => (env.sheet, env.cell),
                Some(e) => {
                    let (s, r) = env.reference(e).ok_or(ErrorKind::Value)??;
                    (s, r.start)
                }
            };
            let v = env.ctx.value(s, c);
            match what.as_str() {
                "address" => {
                    let mut out = String::new();
                    if s != env.sheet {
                        let name = env.ctx.sheet_name(s).unwrap_or_default();
                        let plain = !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.');
                        if plain { out.push_str(&name) } else { out.push_str(&format!("'{}'", name.replace('\'', "''"))) }
                        out.push('!');
                    }
                    out.push_str(&format!("${}${}", crate::addr::col_name(c.col), c.row + 1));
                    Value::text(out)
                }
                "col" => Value::Number(c.col as f64 + 1.0),
                "row" => Value::Number(c.row as f64 + 1.0),
                "contents" => match v {
                    Value::Empty => Value::Number(0.0),
                    v => v,
                },
                "type" => Value::text(match v {
                    Value::Empty => "b",
                    Value::Text(_) => "l",
                    _ => "v",
                }),
                // Text is left-aligned by default; alignment isn't visible from here.
                "prefix" => Value::text(if matches!(v, Value::Text(_)) { "'" } else { "" }),
                "width" => Value::Number(env.ctx.cell_format(s, c).1.trunc() as f64),
                "format" | "color" | "parentheses" => {
                    let (code, color, parens) = format_code(&env.ctx.cell_format(s, c).0);
                    match what.as_str() {
                        "format" => Value::text(code),
                        "color" => Value::Number(color as u8 as f64),
                        _ => Value::Number(parens as u8 as f64),
                    }
                }
                "filename" => Value::text(""),
                "protect" => Value::Number(1.0),
                _ => return Err(ErrorKind::Value),
            }
        }
        _ => return Err(ErrorKind::Name),
    })
}

/// Excel's CELL("format") code for a number format ("F2", "C0-", "D1"...),
/// plus whether negatives are colored and positives use parentheses.
fn format_code(fmt: &str) -> (String, bool, bool) {
    // Split into sections and strip literals, keeping what matters.
    let mut sections: Vec<(String, bool)> = vec![(String::new(), false)];
    let mut it = fmt.chars().peekable();
    while let Some(ch) = it.next() {
        let Some((cur, color)) = sections.last_mut() else { break };
        match ch {
            ';' => sections.push((String::new(), false)),
            '"' => {
                for c in it.by_ref() {
                    if c == '"' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                it.next();
            }
            '[' => {
                let mut tag = String::new();
                for c in it.by_ref() {
                    if c == ']' {
                        break;
                    }
                    tag.push(c);
                }
                let t = tag.to_lowercase();
                if t.starts_with('$') {
                    cur.push('$');
                } else if matches!(t.as_str(), "black" | "blue" | "cyan" | "green" | "magenta" | "red" | "white" | "yellow") || t.starts_with("color") {
                    *color = true;
                } else if matches!(t.chars().next(), Some('h' | 'm' | 's')) {
                    cur.push_str(&t);
                }
            }
            c => cur.extend(c.to_lowercase()),
        }
    }
    let p = sections[0].0.trim().to_string();
    let color = sections.get(1).map_or(sections[0].1, |s| s.1);
    let parens = p.contains('(');
    let has = |c: char| p.contains(c);
    let code = if p.is_empty() || p == "general" {
        "G".to_string()
    } else if p.contains('@') {
        "@".to_string()
    } else if has('y') || has('d') || has('h') || has('s') || (has('m') && !has('0') && !has('#')) {
        let ampm = p.contains("am/pm") || p.contains("a/p");
        let code = if has('y') || has('d') {
            match (p.contains("mmm"), has('y'), has('d')) {
                (true, true, false) => "D3",
                (true, false, true) => "D2",
                (true, _, _) => "D1",
                (false, false, true) => "D5",
                _ => "D4",
            }
        } else if has('h') || has('s') {
            match (has('s'), ampm) {
                (true, true) => "D6",
                (true, false) => "D8",
                (false, true) => "D7",
                (false, false) => "D9",
            }
        } else {
            "D3"
        };
        code.to_string()
    } else if (has('?') && has('/')) || !(has('0') || has('#') || has('?')) {
        "G".to_string()
    } else {
        let decimals = p.split_once('.').map_or(0, |(_, d)| d.chars().take_while(|c| matches!(c, '0' | '#' | '?')).count());
        let kind = if has('%') {
            "P"
        } else if p.contains("e+") || p.contains("e-") {
            "S"
        } else if p.contains(['$', '€', '£', '¥']) {
            "C"
        } else if has(',') {
            ","
        } else {
            "F"
        };
        format!("{kind}{decimals}")
    };
    (format!("{code}{}{}", if color { "-" } else { "" }, if parens { "()" } else { "" }), color, parens)
}

// ---------------------------------------------------------------------------
// Engineering

/// A number argument; booleans are rejected like Excel's engineering functions do.
fn eng_num(env: &Env, e: &Expr) -> R<f64> {
    match env.scalar(e) {
        Value::Bool(_) => Err(ErrorKind::Value),
        v => v.to_number(),
    }
}

fn opt_eng_num(env: &Env, a: &[Expr], i: usize) -> R<Option<f64>> {
    match a.get(i) {
        None | Some(Expr::Missing) => Ok(None),
        Some(e) => eng_num(env, e).map(Some),
    }
}

/// Bits in the 10-digit two's complement form of each base.
fn base_bits(radix: u32) -> u32 {
    match radix {
        2 => 10,
        8 => 30,
        _ => 40,
    }
}

/// Parses up to 10 digits; a leading 1 bit in the full width means negative.
fn parse_base(env: &Env, e: &Expr, radix: u32) -> R<f64> {
    let s = match env.scalar(e) {
        Value::Bool(_) => return Err(ErrorKind::Value),
        v => v.to_text()?,
    };
    if s.chars().count() > 10 {
        return Err(ErrorKind::Num);
    }
    let mut v: i64 = 0;
    for c in s.chars() {
        v = v * radix as i64 + c.to_digit(radix).ok_or(ErrorKind::Num)? as i64;
    }
    let bits = base_bits(radix);
    Ok(if v >= 1 << (bits - 1) { (v - (1 << bits)) as f64 } else { v as f64 })
}

fn to_base(n: f64, radix: u32, places: Option<f64>) -> R<String> {
    let bits = base_bits(radix);
    let half = (1i64 << (bits - 1)) as f64;
    if !(n >= -half && n < half) {
        return Err(ErrorKind::Num);
    }
    let n = n as i64;
    let digits = |v: i64| match radix {
        2 => format!("{v:b}"),
        8 => format!("{v:o}"),
        _ => format!("{v:X}"),
    };
    if n < 0 {
        // Negative numbers always use all 10 digits; places is ignored.
        return Ok(digits(n + (1 << bits)));
    }
    let s = digits(n);
    match places {
        None => Ok(s),
        Some(p) if p >= s.len() as f64 && p <= 10.0 => Ok(format!("{s:0>w$}", w = p as usize)),
        Some(_) => Err(ErrorKind::Num),
    }
}

/// Integer in 0..2^48, as the BIT functions require.
fn bit_arg(x: f64) -> R<u64> {
    if x.fract() != 0.0 || !(0.0..281_474_976_710_656.0).contains(&x) {
        return Err(ErrorKind::Num);
    }
    Ok(x as u64)
}

fn eng(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    use Func::*;
    Ok(match f {
        Bin2Dec | Bin2Hex | Bin2Oct | Dec2Bin | Dec2Hex | Dec2Oct | Hex2Bin | Hex2Dec | Hex2Oct | Oct2Bin | Oct2Dec | Oct2Hex => {
            let (from, to) = match f {
                Bin2Dec => (2, 10),
                Bin2Hex => (2, 16),
                Bin2Oct => (2, 8),
                Dec2Bin => (10, 2),
                Dec2Hex => (10, 16),
                Dec2Oct => (10, 8),
                Hex2Bin => (16, 2),
                Hex2Dec => (16, 10),
                Hex2Oct => (16, 8),
                Oct2Bin => (8, 2),
                Oct2Dec => (8, 10),
                _ => (8, 16),
            };
            need(a, 1, if to == 10 { 1 } else { 2 })?;
            let n = if from == 10 { eng_num(env, &a[0])?.trunc() } else { parse_base(env, &a[0], from)? };
            if to == 10 {
                Value::Number(n)
            } else {
                let places = opt_eng_num(env, a, 1)?.map(f64::trunc);
                Value::text(to_base(n, to, places)?)
            }
        }
        Convert => {
            need(a, 3, 3)?;
            let x = eng_num(env, &a[0])?;
            let (from, to) = (env.text(&a[1])?, env.text(&a[2])?);
            let (Some(u), Some(v)) = (unit(&from), unit(&to)) else { return Err(ErrorKind::Na) };
            match (u, v) {
                (Conv::Lin(c1, f1), Conv::Lin(c2, f2)) if c1 == c2 => num_ok(x * f1 / f2),
                (Conv::Temp(t1), Conv::Temp(t2)) => num_ok(t2.from_c(t1.to_c(x))),
                _ => return Err(ErrorKind::Na),
            }
        }
        Delta | GeStep => {
            need(a, 1, 2)?;
            let x = eng_num(env, &a[0])?;
            let y = opt_eng_num(env, a, 1)?.unwrap_or(0.0);
            Value::Number(if f == Delta { x == y } else { x >= y } as u8 as f64)
        }
        BitAnd | BitOr | BitXor => {
            need(a, 2, 2)?;
            let (x, y) = (bit_arg(eng_num(env, &a[0])?)?, bit_arg(eng_num(env, &a[1])?)?);
            Value::Number(match f {
                BitAnd => x & y,
                BitOr => x | y,
                _ => x ^ y,
            } as f64)
        }
        BitLShift | BitRShift => {
            need(a, 2, 2)?;
            let x = bit_arg(eng_num(env, &a[0])?)? as u128;
            let s = eng_num(env, &a[1])?.trunc();
            if s.abs() > 53.0 {
                return Err(ErrorKind::Num);
            }
            let s = if f == BitRShift { -s } else { s } as i32;
            let r = if s >= 0 { x << s } else { x >> -s };
            if r >= 1 << 48 {
                return Err(ErrorKind::Num);
            }
            Value::Number(r as f64)
        }
        Erf => {
            need(a, 1, 2)?;
            let lo = eng_num(env, &a[0])?;
            num_ok(match opt_eng_num(env, a, 1)? {
                Some(hi) => erf(hi) - erf(lo),
                None => erf(lo),
            })
        }
        Erfc => {
            need(a, 1, 1)?;
            num_ok(erfc(eng_num(env, &a[0])?))
        }
        _ => return Err(ErrorKind::Name),
    })
}

// --- error function ---------------------------------------------------------

/// Maclaurin series, accurate for |x| < 2.
fn erf_series(x: f64) -> f64 {
    let (x2, mut term, mut sum) = (x * x, x, x);
    for n in 1..200 {
        term *= -x2 / n as f64;
        let t = term / (2 * n + 1) as f64;
        sum += t;
        if t.abs() < 1e-17 * sum.abs() {
            break;
        }
    }
    sum * std::f64::consts::FRAC_2_SQRT_PI
}

/// Continued fraction (modified Lentz) for erfc, x >= 2.
fn erfc_cf(x: f64) -> f64 {
    const TINY: f64 = 1e-300;
    let (mut f, mut c, mut d) = (x, x, 0.0);
    for k in 1..2000 {
        let a = k as f64 / 2.0;
        d = x + a * d;
        if d == 0.0 {
            d = TINY;
        }
        c = x + a / c;
        if c == 0.0 {
            c = TINY;
        }
        d = 1.0 / d;
        let delta = c * d;
        f *= delta;
        if (delta - 1.0).abs() < 1e-16 {
            break;
        }
    }
    (-x * x).exp() / (f * std::f64::consts::PI.sqrt())
}

fn erf(x: f64) -> f64 {
    if x.abs() < 2.0 { erf_series(x) } else { x.signum() * (1.0 - erfc_cf(x.abs())) }
}

fn erfc(x: f64) -> f64 {
    if x >= 2.0 {
        erfc_cf(x)
    } else if x <= -2.0 {
        2.0 - erfc_cf(-x)
    } else {
        1.0 - erf_series(x)
    }
}

// --- CONVERT ----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cat {
    Mass,
    Length,
    Time,
    Pressure,
    Force,
    Energy,
    Power,
    Magnetism,
    Volume,
    Area,
    Info,
    Speed,
}

/// Temperature scale: celsius = (x - off) * num / den + base.
#[derive(Clone, Copy, Debug)]
struct Temp {
    off: f64,
    num: f64,
    den: f64,
    base: f64,
}

impl Temp {
    fn to_c(self, x: f64) -> f64 {
        (x - self.off) * self.num / self.den + self.base
    }

    fn from_c(self, c: f64) -> f64 {
        (c - self.base) * self.den / self.num + self.off
    }
}

#[derive(Clone, Copy, Debug)]
enum Conv {
    /// Category and size in SI base units (grams for mass, bits for information).
    Lin(Cat, f64),
    Temp(Temp),
}

/// SI prefixes allowed.
const P: u8 = 1;
/// Binary prefixes (kibi, mebi...) allowed too.
const B: u8 = 3;

const IN: f64 = 0.0254;
const FT: f64 = 0.3048;
const LBF: f64 = 4.4482216152605;
const HP: f64 = 745.699_871_582_270_2;

static UNITS: &[(&str, Cat, f64, u8)] = {
    use Cat::*;
    &[
        // Weight and mass (grams)
        ("g", Mass, 1.0, P),
        ("sg", Mass, LBF / FT * 1000.0, 0),
        ("lbm", Mass, 453.59237, 0),
        ("u", Mass, 1.660538782e-24, P),
        ("ozm", Mass, 28.349523125, 0),
        ("grain", Mass, 0.06479891, 0),
        ("cwt", Mass, 45359.237, 0),
        ("shweight", Mass, 45359.237, 0),
        ("uk_cwt", Mass, 50802.34544, 0),
        ("lcwt", Mass, 50802.34544, 0),
        ("hweight", Mass, 50802.34544, 0),
        ("stone", Mass, 6350.29318, 0),
        ("ton", Mass, 907184.74, 0),
        ("uk_ton", Mass, 1016046.9088, 0),
        ("LTON", Mass, 1016046.9088, 0),
        ("brton", Mass, 1016046.9088, 0),
        // Distance (meters)
        ("m", Length, 1.0, P),
        ("mi", Length, 1609.344, 0),
        ("Nmi", Length, 1852.0, 0),
        ("in", Length, IN, 0),
        ("ft", Length, FT, 0),
        ("yd", Length, 0.9144, 0),
        ("ang", Length, 1e-10, P),
        ("ell", Length, 1.143, 0),
        ("ly", Length, 9_460_730_472_580_800.0, P),
        ("parsec", Length, 3.08567758128155e16, P),
        ("pc", Length, 3.08567758128155e16, P),
        ("Pica", Length, IN / 72.0, 0),
        ("Picapt", Length, IN / 72.0, 0),
        ("pica", Length, IN / 6.0, 0),
        ("survey_mi", Length, 5280.0 * 1200.0 / 3937.0, 0),
        // Time (seconds)
        ("yr", Time, 31_557_600.0, 0),
        ("day", Time, 86_400.0, 0),
        ("d", Time, 86_400.0, 0),
        ("hr", Time, 3_600.0, 0),
        ("mn", Time, 60.0, 0),
        ("min", Time, 60.0, 0),
        ("sec", Time, 1.0, P),
        ("s", Time, 1.0, P),
        // Pressure (pascals)
        ("Pa", Pressure, 1.0, P),
        ("p", Pressure, 1.0, P),
        ("atm", Pressure, 101_325.0, P),
        ("at", Pressure, 101_325.0, P),
        ("mmHg", Pressure, 133.322, P),
        ("psi", Pressure, LBF / (IN * IN), 0),
        ("Torr", Pressure, 101_325.0 / 760.0, 0),
        // Force (newtons)
        ("N", Force, 1.0, P),
        ("dyn", Force, 1e-5, P),
        ("dy", Force, 1e-5, P),
        ("lbf", Force, LBF, 0),
        ("pond", Force, 0.00980665, P),
        // Energy (joules)
        ("J", Energy, 1.0, P),
        ("e", Energy, 1e-7, P),
        ("c", Energy, 4.184, P),
        ("cal", Energy, 4.1868, P),
        ("eV", Energy, 1.602176487e-19, P),
        ("ev", Energy, 1.602176487e-19, P),
        ("HPh", Energy, HP * 3600.0, 0),
        ("hh", Energy, HP * 3600.0, 0),
        ("Wh", Energy, 3600.0, P),
        ("wh", Energy, 3600.0, P),
        // Excel's value (it is really the foot-poundal).
        ("flb", Energy, 0.0421401100938048, 0),
        ("BTU", Energy, 1055.05585262, 0),
        ("btu", Energy, 1055.05585262, 0),
        // Power (watts)
        ("HP", Power, HP, 0),
        ("h", Power, HP, 0),
        ("PS", Power, 735.49875, 0),
        ("W", Power, 1.0, P),
        ("w", Power, 1.0, P),
        // Magnetism (teslas)
        ("T", Magnetism, 1.0, P),
        ("ga", Magnetism, 1e-4, P),
        // Volume (cubic meters)
        ("tsp", Volume, 4.92892159375e-6, 0),
        ("tspm", Volume, 5e-6, 0),
        ("tbs", Volume, 1.478676478125e-5, 0),
        ("oz", Volume, 2.95735295625e-5, 0),
        ("cup", Volume, 2.365882365e-4, 0),
        ("pt", Volume, 4.73176473e-4, 0),
        ("us_pt", Volume, 4.73176473e-4, 0),
        ("uk_pt", Volume, 5.6826125e-4, 0),
        ("qt", Volume, 9.46352946e-4, 0),
        ("uk_qt", Volume, 1.1365225e-3, 0),
        ("gal", Volume, 3.785411784e-3, 0),
        ("uk_gal", Volume, 4.54609e-3, 0),
        ("l", Volume, 1e-3, P),
        ("L", Volume, 1e-3, P),
        ("lt", Volume, 1e-3, P),
        ("barrel", Volume, 0.158987294928, 0),
        ("bushel", Volume, 0.03523907016688, 0),
        ("GRT", Volume, 2.8316846592, 0),
        ("regton", Volume, 2.8316846592, 0),
        ("MTON", Volume, 1.13267386368, 0),
        // Area (square meters)
        ("uk_acre", Area, 4046.8564224, 0),
        ("us_acre", Area, 4046.872_609_874_252, 0),
        ("ar", Area, 100.0, P),
        ("ha", Area, 10_000.0, 0),
        ("Morgen", Area, 2500.0, 0),
        // Information (bits)
        ("bit", Info, 1.0, B),
        ("byte", Info, 8.0, B),
        // Speed (meters per second)
        ("admkn", Speed, 6080.0 * FT / 3600.0, 0),
        ("kn", Speed, 1852.0 / 3600.0, 0),
        ("m/h", Speed, 1.0 / 3600.0, P),
        ("m/hr", Speed, 1.0 / 3600.0, P),
        ("m/s", Speed, 1.0, P),
        ("m/sec", Speed, 1.0, P),
        ("mph", Speed, 1609.344 / 3600.0, 0),
    ]
};

/// Lengths that also exist squared and cubed ("ft2", "m^3"...).
static POWERED: &[(&str, f64, u8)] = &[
    ("m", 1.0, P),
    ("mi", 1609.344, 0),
    ("Nmi", 1852.0, 0),
    ("in", IN, 0),
    ("ft", FT, 0),
    ("yd", 0.9144, 0),
    ("ang", 1e-10, P),
    ("ly", 9_460_730_472_580_800.0, P),
    ("Pica", IN / 72.0, 0),
    ("Picapt", IN / 72.0, 0),
];

static PREFIXES: &[(&str, f64, bool)] = &[
    // Two-letter prefixes first so "da" and "ki" win over "d" and "k".
    ("da", 1e1, false),
    ("ki", 1024.0, true),
    ("Mi", 1_048_576.0, true),
    ("Gi", 1_073_741_824.0, true),
    ("Ti", 1_099_511_627_776.0, true),
    ("Pi", 1_125_899_906_842_624.0, true),
    ("Ei", 1_152_921_504_606_846_976.0, true),
    ("Zi", 1_180_591_620_717_411_303_424.0, true),
    ("Yi", 1_208_925_819_614_629_174_706_176.0, true),
    ("Y", 1e24, false),
    ("Z", 1e21, false),
    ("E", 1e18, false),
    ("P", 1e15, false),
    ("T", 1e12, false),
    ("G", 1e9, false),
    ("M", 1e6, false),
    ("k", 1e3, false),
    ("h", 1e2, false),
    ("e", 1e1, false),
    ("d", 1e-1, false),
    ("c", 1e-2, false),
    ("m", 1e-3, false),
    ("u", 1e-6, false),
    ("n", 1e-9, false),
    ("p", 1e-12, false),
    ("f", 1e-15, false),
    ("a", 1e-18, false),
    ("z", 1e-21, false),
    ("y", 1e-24, false),
];

/// A unit without prefix: conversion, allowed prefixes and the power a
/// prefix is raised to (km2 = (1000 m)^2).
fn base_unit(name: &str) -> Option<(Conv, u8, i32)> {
    let temp = |off, num, den, base| Some((Conv::Temp(Temp { off, num, den, base }), 0, 1));
    match name {
        "C" | "cel" => return temp(0.0, 1.0, 1.0, 0.0),
        "F" | "fah" => return temp(32.0, 5.0, 9.0, 0.0),
        "K" | "kel" => return Some((Conv::Temp(Temp { off: 0.0, num: 1.0, den: 1.0, base: -273.15 }), P, 1)),
        "Rank" => return temp(0.0, 5.0, 9.0, -273.15),
        "Reau" => return temp(0.0, 5.0, 4.0, 0.0),
        _ => {}
    }
    if let Some(&(_, cat, f, flags)) = UNITS.iter().find(|u| u.0 == name) {
        return Some((Conv::Lin(cat, f), flags, 1));
    }
    for (suffix, pow, cat) in [("2", 2, Cat::Area), ("^2", 2, Cat::Area), ("3", 3, Cat::Volume), ("^3", 3, Cat::Volume)] {
        if let Some(stem) = name.strip_suffix(suffix) {
            if let Some(&(_, f, flags)) = POWERED.iter().find(|u| u.0 == stem) {
                return Some((Conv::Lin(cat, f.powi(pow)), flags, pow));
            }
        }
    }
    None
}

fn unit(name: &str) -> Option<Conv> {
    if let Some((c, _, _)) = base_unit(name) {
        return Some(c);
    }
    PREFIXES.iter().find_map(|&(p, scale, binary)| {
        let (conv, flags, pow) = base_unit(name.strip_prefix(p)?)?;
        if flags & P == 0 || (binary && flags != B) {
            return None;
        }
        Some(match conv {
            Conv::Lin(cat, f) => Conv::Lin(cat, f * scale.powi(pow)),
            Conv::Temp(t) => Conv::Temp(Temp { num: t.num * scale, ..t }),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellRef, Locale, Op, Sheet, Workbook};

    fn wb() -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        w
    }

    fn c(a1: &str) -> CellRef {
        CellRef::parse(a1).unwrap()
    }

    fn num(w: &Workbook, f: &str) -> f64 {
        match w.eval_formula(0, f) {
            Value::Number(n) => n,
            v => panic!("{f}: {v:?}"),
        }
    }

    fn close(w: &Workbook, f: &str, want: f64) {
        let got = num(w, f);
        assert!((got - want).abs() <= 1e-9 * want.abs().max(1e-300), "{f}: {got} != {want}");
    }

    fn text(w: &Workbook, f: &str) -> String {
        match w.eval_formula(0, f) {
            Value::Text(t) => t.to_string(),
            v => panic!("{f}: {v:?}"),
        }
    }

    fn err(w: &Workbook, f: &str) -> ErrorKind {
        match w.eval_formula(0, f) {
            Value::Error(k) => k,
            v => panic!("{f}: {v:?}"),
        }
    }

    fn b(w: &Workbook, f: &str) -> bool {
        match w.eval_formula(0, f) {
            Value::Bool(x) => x,
            v => panic!("{f}: {v:?}"),
        }
    }

    #[test]
    fn is_functions() {
        let mut w = wb();
        w.set_input(0, c("A1"), "5");
        w.set_input(0, c("A2"), "hello");
        w.set_input(0, c("A3"), "=A1*2");
        assert!(!b(&w, "=ISEVEN(-1)"));
        assert!(b(&w, "=ISEVEN(2.5)"));
        assert!(!b(&w, "=ISEVEN(5)"));
        assert!(b(&w, "=ISEVEN(A9)"));
        assert!(b(&w, "=ISODD(-1)"));
        assert!(!b(&w, "=ISODD(2.5)"));
        assert!(b(&w, "=ISODD(5)"));
        assert_eq!(err(&w, "=ISEVEN(\"x\")"), ErrorKind::Value);
        assert_eq!(err(&w, "=ISODD(TRUE)"), ErrorKind::Value);
        assert!(b(&w, "=ISLOGICAL(TRUE)"));
        assert!(!b(&w, "=ISLOGICAL(\"TRUE\")"));
        assert!(b(&w, "=ISNONTEXT(A9)"));
        assert!(b(&w, "=ISNONTEXT(#N/A)"));
        assert!(!b(&w, "=ISNONTEXT(A2)"));
        assert!(!b(&w, "=ISERR(#N/A)"));
        assert!(b(&w, "=ISERR(#REF!)"));
        assert!(b(&w, "=ISERR(1/0)"));
        assert!(b(&w, "=ISREF(A1)"));
        assert!(b(&w, "=ISREF(A1:B3)"));
        assert!(!b(&w, "=ISREF(1)"));
        assert!(!b(&w, "=ISREF(\"A1\")"));
        assert!(b(&w, "=ISFORMULA(A3)"));
        assert!(!b(&w, "=ISFORMULA(A1)"));
        assert_eq!(err(&w, "=ISFORMULA(1)"), ErrorKind::Value);
        match w.eval_formula(0, "=ISFORMULA(A1:A3)") {
            Value::Array(m) => assert_eq!(m.data, vec![Value::Bool(false), Value::Bool(false), Value::Bool(true)]),
            v => panic!("{v:?}"),
        }
        // Lifted over arrays.
        match w.eval_formula(0, "=ISEVEN({1,2,3})") {
            Value::Array(m) => assert_eq!(m.data, vec![Value::Bool(false), Value::Bool(true), Value::Bool(false)]),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn type_and_error_type() {
        let w = wb();
        assert_eq!(num(&w, "=TYPE(2)"), 1.0);
        assert_eq!(num(&w, "=TYPE(A9)"), 1.0);
        assert_eq!(num(&w, "=TYPE(\"Smith\")"), 2.0);
        assert_eq!(num(&w, "=TYPE(TRUE)"), 4.0);
        assert_eq!(num(&w, "=TYPE(1/0)"), 16.0);
        assert_eq!(num(&w, "=TYPE({1,2;3,4})"), 64.0);
        assert_eq!(num(&w, "=TYPE(LAMBDA(x,x))"), 128.0);
        assert_eq!(num(&w, "=ERROR.TYPE(#NULL!)"), 1.0);
        assert_eq!(num(&w, "=ERROR.TYPE(1/0)"), 2.0);
        assert_eq!(num(&w, "=ERROR.TYPE(#VALUE!)"), 3.0);
        assert_eq!(num(&w, "=ERROR.TYPE(#REF!)"), 4.0);
        assert_eq!(num(&w, "=ERROR.TYPE(#NAME?)"), 5.0);
        assert_eq!(num(&w, "=ERROR.TYPE(#NUM!)"), 6.0);
        assert_eq!(num(&w, "=ERROR.TYPE(#N/A)"), 7.0);
        assert_eq!(err(&w, "=ERROR.TYPE(5)"), ErrorKind::Na);
    }

    #[test]
    fn cell_and_sheets() {
        let mut w = wb();
        w.set_input(0, c("A1"), "12.5");
        w.set_input(0, c("A2"), "text");
        w.modify_style(0, &crate::Range::single(c("A1")), |s| s.num_fmt = Arc::from("0.00"));
        assert_eq!(num(&w, "=CELL(\"row\",C5)"), 5.0);
        assert_eq!(num(&w, "=CELL(\"col\",C5)"), 3.0);
        assert_eq!(num(&w, "=CELL(\"col\",C5:D9)"), 3.0);
        assert_eq!(text(&w, "=CELL(\"address\",C5)"), "$C$5");
        assert_eq!(num(&w, "=CELL(\"contents\",A1)"), 12.5);
        assert_eq!(text(&w, "=CELL(\"contents\",A2)"), "text");
        assert_eq!(text(&w, "=CELL(\"type\",A1)"), "v");
        assert_eq!(text(&w, "=CELL(\"type\",A2)"), "l");
        assert_eq!(text(&w, "=CELL(\"type\",A3)"), "b");
        assert_eq!(text(&w, "=CELL(\"prefix\",A2)"), "'");
        assert_eq!(text(&w, "=CELL(\"prefix\",A1)"), "");
        assert_eq!(text(&w, "=CELL(\"format\",A1)"), "F2");
        assert_eq!(text(&w, "=CELL(\"format\",A2)"), "G");
        assert_eq!(num(&w, "=CELL(\"protect\",A1)"), 1.0);
        assert_eq!(text(&w, "=CELL(\"filename\",A1)"), "");
        assert!(num(&w, "=CELL(\"width\",A1)") >= 1.0);
        assert_eq!(num(&w, "=CELL(\"row\")"), 1.0);
        assert_eq!(err(&w, "=CELL(\"nope\",A1)"), ErrorKind::Value);
        assert_eq!(err(&w, "=CELL(\"row\",5)"), ErrorKind::Value);

        assert_eq!(num(&w, "=SHEET()"), 1.0);
        assert_eq!(num(&w, "=SHEETS()"), 1.0);
        w.execute(Op::AddSheet { index: 1, sheet: Box::new(Sheet::new("Data")) });
        assert_eq!(num(&w, "=SHEETS()"), 2.0);
        assert_eq!(num(&w, "=SHEET(\"Data\")"), 2.0);
        assert_eq!(num(&w, "=SHEET(Data!B2)"), 2.0);
        assert_eq!(num(&w, "=SHEETS(Data!A1:B2)"), 1.0);
        assert_eq!(err(&w, "=SHEET(\"Nope\")"), ErrorKind::Na);
        assert_eq!(text(&w, "=CELL(\"address\",Data!B2)"), "Data!$B$2");
    }

    #[test]
    fn format_codes() {
        let cases = [
            ("General", "G"),
            ("0", "F0"),
            ("#,##0", ",0"),
            ("0.00", "F2"),
            ("#,##0.00", ",2"),
            ("$#,##0_);($#,##0)", "C0"),
            ("$#,##0_);[Red]($#,##0)", "C0-"),
            ("$#,##0.00_);($#,##0.00)", "C2"),
            ("$#,##0.00_);[Red]($#,##0.00)", "C2-"),
            ("0%", "P0"),
            ("0.00%", "P2"),
            ("0.00E+00", "S2"),
            ("# ?/?", "G"),
            ("# ??/??", "G"),
            ("m/d/yy", "D4"),
            ("m/d/yy h:mm", "D4"),
            ("mm/dd/yy", "D4"),
            ("d-mmm-yy", "D1"),
            ("dd-mmm-yy", "D1"),
            ("d-mmm", "D2"),
            ("mmm-yy", "D3"),
            ("mm/dd", "D5"),
            ("h:mm AM/PM", "D7"),
            ("h:mm:ss AM/PM", "D6"),
            ("h:mm", "D9"),
            ("h:mm:ss", "D8"),
            ("(#,##0)", ",0()"),
            ("@", "@"),
        ];
        for (fmt, want) in cases {
            assert_eq!(format_code(fmt).0, want, "{fmt}");
        }
        assert!(format_code("0;[Red]-0").1);
        assert!(!format_code("0").1);
    }

    #[test]
    fn base_conversions() {
        let w = wb();
        assert_eq!(num(&w, "=BIN2DEC(1100100)"), 100.0);
        assert_eq!(num(&w, "=BIN2DEC(1111111111)"), -1.0);
        assert_eq!(text(&w, "=BIN2HEX(11111011,4)"), "00FB");
        assert_eq!(text(&w, "=BIN2HEX(1110)"), "E");
        assert_eq!(text(&w, "=BIN2HEX(1111111111)"), "FFFFFFFFFF");
        assert_eq!(text(&w, "=BIN2OCT(1001,3)"), "011");
        assert_eq!(text(&w, "=BIN2OCT(1100100)"), "144");
        assert_eq!(text(&w, "=BIN2OCT(1111111111)"), "7777777777");
        assert_eq!(text(&w, "=DEC2BIN(9,4)"), "1001");
        assert_eq!(text(&w, "=DEC2BIN(-100)"), "1110011100");
        assert_eq!(text(&w, "=DEC2HEX(100,4)"), "0064");
        assert_eq!(text(&w, "=DEC2HEX(-54)"), "FFFFFFFFCA");
        assert_eq!(text(&w, "=DEC2HEX(28)"), "1C");
        assert_eq!(err(&w, "=DEC2HEX(64,1)"), ErrorKind::Num);
        assert_eq!(text(&w, "=DEC2OCT(58,3)"), "072");
        assert_eq!(text(&w, "=DEC2OCT(-100)"), "7777777634");
        assert_eq!(text(&w, "=HEX2BIN(\"F\",8)"), "00001111");
        assert_eq!(text(&w, "=HEX2BIN(\"B7\")"), "10110111");
        assert_eq!(text(&w, "=HEX2BIN(\"FFFFFFFE00\")"), "1000000000");
        assert_eq!(num(&w, "=HEX2DEC(\"A5\")"), 165.0);
        assert_eq!(num(&w, "=HEX2DEC(\"FFFFFFFF5B\")"), -165.0);
        assert_eq!(num(&w, "=HEX2DEC(\"3DA408B9\")"), 1034160313.0);
        assert_eq!(num(&w, "=HEX2DEC(\"a5\")"), 165.0);
        assert_eq!(text(&w, "=HEX2OCT(\"F\",3)"), "017");
        assert_eq!(text(&w, "=HEX2OCT(\"3B4E\")"), "35516");
        assert_eq!(text(&w, "=HEX2OCT(\"FFFFFFFF00\")"), "7777777400");
        assert_eq!(text(&w, "=OCT2BIN(3,3)"), "011");
        assert_eq!(text(&w, "=OCT2BIN(7777777000)"), "1000000000");
        assert_eq!(num(&w, "=OCT2DEC(54)"), 44.0);
        assert_eq!(num(&w, "=OCT2DEC(7777777533)"), -165.0);
        assert_eq!(text(&w, "=OCT2HEX(100,4)"), "0040");
        assert_eq!(text(&w, "=OCT2HEX(7777777533)"), "FFFFFFFF5B");
        // Limits and bad input.
        assert_eq!(text(&w, "=DEC2BIN(511)"), "111111111");
        assert_eq!(text(&w, "=DEC2BIN(-512)"), "1000000000");
        assert_eq!(err(&w, "=DEC2BIN(512)"), ErrorKind::Num);
        assert_eq!(err(&w, "=DEC2BIN(-513)"), ErrorKind::Num);
        assert_eq!(err(&w, "=DEC2BIN(1,11)"), ErrorKind::Num);
        assert_eq!(err(&w, "=DEC2BIN(1,0)"), ErrorKind::Num);
        assert_eq!(err(&w, "=DEC2HEX(1E+300)"), ErrorKind::Num);
        assert_eq!(err(&w, "=HEX2BIN(\"200\")"), ErrorKind::Num);
        assert_eq!(err(&w, "=BIN2DEC(12)"), ErrorKind::Num);
        assert_eq!(err(&w, "=BIN2DEC(11111111111)"), ErrorKind::Num);
        assert_eq!(err(&w, "=BIN2DEC(TRUE)"), ErrorKind::Value);
        assert_eq!(err(&w, "=BIN2DEC(1,2)"), ErrorKind::Value);
        assert_eq!(err(&w, "=HEX2DEC(\"G\")"), ErrorKind::Num);
        assert_eq!(err(&w, "=DEC2BIN(\"abc\")"), ErrorKind::Value);
        assert_eq!(text(&w, "=DEC2HEX(549755813887)"), "7FFFFFFFFF");
        assert_eq!(text(&w, "=DEC2HEX(-549755813888)"), "8000000000");
        assert_eq!(err(&w, "=DEC2HEX(549755813888)"), ErrorKind::Num);
    }

    #[test]
    fn convert() {
        let w = wb();
        close(&w, "=CONVERT(1,\"lbm\",\"kg\")", 0.45359237);
        close(&w, "=CONVERT(68,\"F\",\"C\")", 20.0);
        assert_eq!(num(&w, "=CONVERT(68,\"F\",\"C\")"), 20.0);
        assert_eq!(err(&w, "=CONVERT(2.5,\"ft\",\"sec\")"), ErrorKind::Na);
        close(&w, "=CONVERT(CONVERT(100,\"ft\",\"m\"),\"ft\",\"m\")", 9.290304);
        close(&w, "=CONVERT(1,\"gal\",\"l\")", 3.785411784);
        close(&w, "=CONVERT(1,\"km\",\"m\")", 1000.0);
        close(&w, "=CONVERT(1,\"dam\",\"m\")", 10.0);
        close(&w, "=CONVERT(1,\"km2\",\"m2\")", 1e6);
        close(&w, "=CONVERT(1,\"ft^2\",\"m2\")", 0.09290304);
        close(&w, "=CONVERT(1,\"m3\",\"l\")", 1000.0);
        close(&w, "=CONVERT(1,\"cm^3\",\"ml\")", 1.0);
        close(&w, "=CONVERT(1,\"kibyte\",\"bit\")", 8192.0);
        close(&w, "=CONVERT(1,\"kbyte\",\"byte\")", 1000.0);
        close(&w, "=CONVERT(100,\"C\",\"K\")", 373.15);
        close(&w, "=CONVERT(0,\"K\",\"F\")", -459.67);
        assert!(num(&w, "=CONVERT(491.67,\"Rank\",\"C\")").abs() < 1e-9);
        close(&w, "=CONVERT(80,\"Reau\",\"C\")", 100.0);
        close(&w, "=CONVERT(1000,\"mK\",\"K\")", 1.0);
        close(&w, "=CONVERT(1,\"hr\",\"mn\")", 60.0);
        close(&w, "=CONVERT(1,\"yr\",\"day\")", 365.25);
        close(&w, "=CONVERT(1,\"mph\",\"m/s\")", 0.44704);
        close(&w, "=CONVERT(36,\"km/h\",\"m/s\")", 10.0);
        close(&w, "=CONVERT(1,\"atm\",\"mmHg\")", 760.002100178515);
        close(&w, "=CONVERT(1,\"atm\",\"Torr\")", 760.0);
        close(&w, "=CONVERT(1,\"HP\",\"W\")", 745.69987158227);
        close(&w, "=CONVERT(1,\"BTU\",\"J\")", 1055.05585262);
        close(&w, "=CONVERT(1,\"lbf\",\"N\")", 4.4482216152605);
        close(&w, "=CONVERT(1,\"T\",\"ga\")", 10000.0);
        close(&w, "=CONVERT(1,\"mi\",\"ft\")", 5280.0);
        close(&w, "=CONVERT(1,\"survey_mi\",\"m\")", 1609.3472186944373);
        close(&w, "=CONVERT(1,\"in\",\"Pica\")", 72.0);
        close(&w, "=CONVERT(1,\"ha\",\"ar\")", 100.0);
        close(&w, "=CONVERT(1,\"us_acre\",\"m2\")", 4046.87260987425);
        close(&w, "=CONVERT(1,\"stone\",\"lbm\")", 14.0);
        close(&w, "=CONVERT(1,\"ozm\",\"g\")", 28.349523125);
        close(&w, "=CONVERT(1,\"cal\",\"J\")", 4.1868);
        close(&w, "=CONVERT(1,\"kWh\",\"J\")", 3.6e6);
        close(&w, "=CONVERT(1,\"barrel\",\"gal\")", 42.0);
        close(&w, "=CONVERT(1,\"GRT\",\"ft3\")", 100.0);
        close(&w, "=CONVERT(1,\"kn\",\"m/h\")", 1852.0);
        assert_eq!(err(&w, "=CONVERT(1,\"xyz\",\"m\")"), ErrorKind::Na);
        assert_eq!(err(&w, "=CONVERT(1,\"kft\",\"m\")"), ErrorKind::Na);
        assert_eq!(err(&w, "=CONVERT(1,\"kim\",\"m\")"), ErrorKind::Na);
        assert_eq!(err(&w, "=CONVERT(1,\"C\",\"m\")"), ErrorKind::Na);
        assert_eq!(err(&w, "=CONVERT(\"a\",\"m\",\"ft\")"), ErrorKind::Value);
    }

    #[test]
    fn delta_gestep_bits() {
        let w = wb();
        assert_eq!(num(&w, "=DELTA(5,4)"), 0.0);
        assert_eq!(num(&w, "=DELTA(5,5)"), 1.0);
        assert_eq!(num(&w, "=DELTA(0.5,0)"), 0.0);
        assert_eq!(num(&w, "=DELTA(0)"), 1.0);
        assert_eq!(err(&w, "=DELTA(TRUE)"), ErrorKind::Value);
        assert_eq!(num(&w, "=GESTEP(5,4)"), 1.0);
        assert_eq!(num(&w, "=GESTEP(5,5)"), 1.0);
        assert_eq!(num(&w, "=GESTEP(-4,-5)"), 1.0);
        assert_eq!(num(&w, "=GESTEP(-1)"), 0.0);
        assert_eq!(num(&w, "=BITAND(1,5)"), 1.0);
        assert_eq!(num(&w, "=BITAND(13,25)"), 9.0);
        assert_eq!(num(&w, "=BITOR(23,10)"), 31.0);
        assert_eq!(num(&w, "=BITXOR(5,3)"), 6.0);
        assert_eq!(num(&w, "=BITLSHIFT(4,2)"), 16.0);
        assert_eq!(num(&w, "=BITLSHIFT(4,-2)"), 1.0);
        assert_eq!(num(&w, "=BITRSHIFT(13,2)"), 3.0);
        assert_eq!(num(&w, "=BITRSHIFT(13,53)"), 0.0);
        assert_eq!(num(&w, "=BITAND(2^48-1,1)"), 1.0);
        assert_eq!(err(&w, "=BITAND(2^48,1)"), ErrorKind::Num);
        assert_eq!(err(&w, "=BITAND(-1,1)"), ErrorKind::Num);
        assert_eq!(err(&w, "=BITAND(1.5,1)"), ErrorKind::Num);
        assert_eq!(err(&w, "=BITLSHIFT(1,54)"), ErrorKind::Num);
        assert_eq!(err(&w, "=BITLSHIFT(1,48)"), ErrorKind::Num);
        assert_eq!(num(&w, "=BITLSHIFT(1,47)"), 140737488355328.0);
    }

    #[test]
    fn erf_functions() {
        let w = wb();
        close(&w, "=ERF(0.745)", 0.7079289200957377);
        close(&w, "=ERF(1)", 0.842700792949715);
        close(&w, "=ERF(0,1)", 0.842700792949715);
        close(&w, "=ERF(1,2)", 0.152621472069238);
        close(&w, "=ERF(-1)", -0.842700792949715);
        close(&w, "=ERF(2.5)", 0.999593047982555);
        close(&w, "=ERF.PRECISE(0.745)", 0.7079289200957377);
        close(&w, "=ERFC(1)", 0.157299207050285);
        close(&w, "=ERFC(1.9)", 0.0072095707647425325);
        close(&w, "=ERFC(1.99)", 0.004888586800383003);
        close(&w, "=ERFC(2.01)", 0.004475150644751763);
        close(&w, "=ERF(2.01)", 0.9955248493552482);
        close(&w, "=ERFC(3)", 2.20904969985854e-05);
        close(&w, "=ERFC(-1)", 1.84270079294971);
        close(&w, "=ERFC.PRECISE(1)", 0.157299207050285);
        close(&w, "=ERFC(0)", 1.0);
        assert_eq!(num(&w, "=ERF(0)"), 0.0);
        assert_eq!(err(&w, "=ERF(\"x\")"), ErrorKind::Value);
    }
}
