//! The text of planning cells: durations ("3d", "2 sem", "4h"),
//! predecessors ("2;3FS+2d;5SS-10%") and assignments ("Ana;Luis[50%]").
//! Readers accept the English codes and those of the interface languages;
//! writers use the English ones.

use super::{DepType, Lag, Link};

/// Duration units, in working days (`hpd`: hours in a working day).
fn unit_days(u: &str, hpd: f64) -> Option<(f64, bool)> {
    let u = u.trim().trim_end_matches('.').to_lowercase();
    // (days per unit, elapsed)
    Some(match u.as_str() {
        "" | "d" | "day" | "days" | "día" | "días" | "dia" | "dias" | "j" | "jour" | "jours" | "t" | "tag" | "tage" | "g" | "gg" | "giorno" | "giorni" => (1.0, false),
        "w" | "wk" | "wks" | "week" | "weeks" | "s" | "sem" | "semana" | "semanas" | "sm" | "semaine" | "semaines" | "wo" | "woche" | "wochen" | "settimana" | "settimane" => (5.0, false),
        "h" | "hr" | "hrs" | "hour" | "hours" | "hora" | "horas" | "heure" | "heures" | "std" | "stunde" | "stunden" | "ora" | "ore" => (1.0 / hpd, false),
        "min" | "m" | "minute" | "minutes" | "minuto" | "minutos" | "minuten" | "minuti" => (1.0 / (hpd * 60.0), false),
        "mo" | "mon" | "month" | "months" | "mes" | "meses" | "mois" | "monat" | "monate" | "mese" | "mesi" => (20.0, false),
        "ed" | "edays" | "dt" | "jc" | "ft" | "gt" | "dc" => (1.0, true),
        "ew" => (7.0, true),
        _ => return None,
    })
}

/// Splits "12.5d" into ("12.5", "d"); decimal commas count.
fn number_unit(s: &str) -> Option<(f64, &str)> {
    let s = s.trim();
    let end = s.char_indices().find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || *c == ',' || (*i == 0 && (*c == '-' || *c == '+')))).map_or(s.len(), |(i, _)| i);
    let n: f64 = s[..end].replace(',', ".").parse().ok()?;
    Some((n, s[end..].trim()))
}

/// A duration in working days (estimated durations end in '?', which is
/// ignored). None when the text is not a duration.
pub fn duration(s: &str, hpd: f64) -> Option<f64> {
    let s = s.trim().trim_end_matches('?');
    let (n, u) = number_unit(s)?;
    let (k, _) = unit_days(u, hpd)?;
    let d = n * k;
    (d.is_finite() && d >= 0.0).then_some(d.min(100_000.0))
}

/// Dependency type codes: English and the interface languages. Each line
/// is (FS, SS, FF, SF).
const CODES: [[&str; 4]; 6] = [
    ["FS", "SS", "FF", "SF"],
    // Spanish: fin-comienzo…
    ["FC", "CC", "FF", "CF"],
    // French: fin-début…
    ["FD", "DD", "FF", "DF"],
    // German: Ende-Anfang…
    ["EA", "AA", "EE", "AE"],
    // Italian: fine-inizio…
    ["FI", "II", "FF", "IF"],
    // Portuguese: término-início…
    ["TI", "II", "TT", "IT"],
];

fn dep_type(code: &str) -> Option<DepType> {
    let c = code.to_uppercase();
    for line in CODES {
        if let Some(i) = line.iter().position(|x| *x == c) {
            return Some([DepType::FS, DepType::SS, DepType::FF, DepType::SF][i]);
        }
    }
    None
}

/// One predecessor: "3", "3FS", "3SS+2d", "3FF-25%", "3FS+1ed".
pub fn link(s: &str, hpd: f64) -> Option<Link> {
    let s = s.trim();
    let digits = s.char_indices().find(|(_, c)| !c.is_ascii_digit()).map_or(s.len(), |(i, _)| i);
    let id: usize = s[..digits].parse().ok()?;
    let rest = s[digits..].trim();
    let letters = rest.char_indices().find(|(_, c)| !c.is_alphabetic()).map_or(rest.len(), |(i, _)| i);
    let kind = if letters == 0 { DepType::FS } else { dep_type(&rest[..letters])? };
    let rest = rest[letters..].trim();
    let lag = if rest.is_empty() {
        Lag::Days(0.0)
    } else {
        if !rest.starts_with(['+', '-']) {
            return None;
        }
        let (n, u) = number_unit(rest)?;
        if u == "%" {
            Lag::Percent(n)
        } else {
            let (k, elapsed) = unit_days(u, hpd)?;
            if elapsed { Lag::Elapsed(n * k) } else { Lag::Days(n * k) }
        }
    };
    (id >= 1).then(|| Link { pred: id - 1, kind, lag })
}

/// A predecessor list (separated by ';' or ','). Wrong items are returned
/// apart, as written.
pub fn links(s: &str, hpd: f64) -> (Vec<Link>, Vec<String>) {
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    // "2,3" is a list; "3FS+1,5d" is a decimal comma: a comma followed by
    // a digit after a sign belongs to a number.
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut after_sign = false;
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '+' || c == '-' {
            after_sign = true;
        }
        let decimal = c == ',' && after_sign && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit()) && chars[..i].last().is_some_and(|p| p.is_ascii_digit());
        if (c == ';' || c == ',') && !decimal {
            items.push(std::mem::take(&mut cur));
            after_sign = false;
        } else {
            cur.push(c);
        }
    }
    items.push(cur);
    for it in items {
        if it.trim().is_empty() {
            continue;
        }
        match link(&it, hpd) {
            Some(l) => ok.push(l),
            None => bad.push(it.trim().to_string()),
        }
    }
    (ok, bad)
}

fn num(n: f64) -> String {
    let r = (n * 100.0).round() / 100.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}

/// A lag as text ("+2d", "-25%", "" for none).
pub fn lag_text(l: Lag) -> String {
    match l {
        Lag::Days(d) if d == 0.0 => String::new(),
        Lag::Days(d) => format!("{}{}d", if d > 0.0 { "+" } else { "" }, num(d)),
        Lag::Elapsed(d) => format!("{}{}ed", if d >= 0.0 { "+" } else { "" }, num(d)),
        Lag::Percent(p) => format!("{}{}%", if p >= 0.0 { "+" } else { "" }, num(p)),
    }
}

/// A predecessor list as text (1-based ids; plain FS links are just the id).
pub fn links_text(list: &[Link]) -> String {
    let mut out = Vec::new();
    for l in list {
        let lag = lag_text(l.lag);
        let kind = if l.kind == DepType::FS && lag.is_empty() { "" } else { l.kind.code() };
        out.push(format!("{}{kind}{lag}", l.pred + 1));
    }
    out.join(";")
}

/// Renumbers the ids of a predecessor text: `map(old 0-based id)` gives the
/// new one (None drops the link). Items that do not parse are kept.
pub fn renumber(s: &str, hpd: f64, map: &dyn Fn(usize) -> Option<usize>) -> String {
    let (list, bad) = links(s, hpd);
    let mut out: Vec<Link> = Vec::new();
    for mut l in list {
        if let Some(n) = map(l.pred) {
            l.pred = n;
            out.push(l);
        }
    }
    let mut t = links_text(&out);
    for b in bad {
        if !t.is_empty() {
            t.push(';');
        }
        t.push_str(&b);
    }
    t
}

/// One assignment: a resource and its units (1.0 = full time; for a
/// material resource, the quantity).
#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    pub name: String,
    pub units: Option<f64>,
}

/// "Ana; Luis[50%]; Cemento[20]" (Project writes units in brackets).
pub fn assignments(s: &str) -> Vec<Assignment> {
    let mut out = Vec::new();
    for it in s.split([';', ',']) {
        let it = it.trim();
        if it.is_empty() {
            continue;
        }
        let (name, units) = match it.find('[') {
            Some(i) => {
                let u = it[i + 1..].trim_end_matches(']').trim();
                let units = if let Some(p) = u.strip_suffix('%') { p.trim().replace(',', ".").parse::<f64>().ok().map(|p| p / 100.0) } else { u.replace(',', ".").parse::<f64>().ok() };
                (it[..i].trim(), units)
            }
            None => (it, None),
        };
        if !name.is_empty() {
            out.push(Assignment { name: name.to_string(), units });
        }
    }
    out
}

/// Assignments as text (full-time work units are left out).
pub fn assignments_text(list: &[Assignment]) -> String {
    list.iter()
        .map(|a| match a.units {
            None => a.name.clone(),
            Some(u) => format!("{}[{}%]", a.name, num(u * 100.0)),
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration("3", 8.0), Some(3.0));
        assert_eq!(duration("3d", 8.0), Some(3.0));
        assert_eq!(duration("2 sem", 8.0), Some(10.0));
        assert_eq!(duration("4h", 8.0), Some(0.5));
        assert_eq!(duration("1,5 días", 8.0), Some(1.5));
        assert_eq!(duration("2 wochen", 8.0), Some(10.0));
        assert_eq!(duration("1mo", 8.0), Some(20.0));
        assert_eq!(duration("5d?", 8.0), Some(5.0));
        assert_eq!(duration("0", 8.0), Some(0.0));
        assert_eq!(duration("abc", 8.0), None);
        assert_eq!(duration("-2", 8.0), None);
        assert_eq!(duration("3 xyz", 8.0), None);
    }

    #[test]
    fn predecessor_lists() {
        let (l, bad) = links("2;3FS+2d; 5ss-1d,7FF+50%;9SF;4FS+3ed;x;0", 8.0);
        assert_eq!(bad, vec!["x".to_string(), "0".to_string()]);
        assert_eq!(l[0], Link { pred: 1, kind: DepType::FS, lag: Lag::Days(0.0) });
        assert_eq!(l[1], Link { pred: 2, kind: DepType::FS, lag: Lag::Days(2.0) });
        assert_eq!(l[2], Link { pred: 4, kind: DepType::SS, lag: Lag::Days(-1.0) });
        assert_eq!(l[3], Link { pred: 6, kind: DepType::FF, lag: Lag::Percent(50.0) });
        assert_eq!(l[4], Link { pred: 8, kind: DepType::SF, lag: Lag::Days(0.0) });
        assert_eq!(l[5], Link { pred: 3, kind: DepType::FS, lag: Lag::Elapsed(3.0) });
        // Spanish codes and a decimal comma.
        let (l, bad) = links("3CC+1,5d;4FC", 8.0);
        assert!(bad.is_empty());
        assert_eq!(l[0], Link { pred: 2, kind: DepType::SS, lag: Lag::Days(1.5) });
        assert_eq!(l[1].kind, DepType::FS);
        // Hours and weeks of lag.
        let (l, _) = links("2FS+4h;3FS-1w", 8.0);
        assert_eq!(l[0].lag, Lag::Days(0.5));
        assert_eq!(l[1].lag, Lag::Days(-5.0));
        assert_eq!(links_text(&links("2;3SS+2d;4FF-25%", 8.0).0), "2;3SS+2d;4FF-25%");
    }

    #[test]
    fn renumbering() {
        // Task 3 (index 2) was deleted: 4 becomes 3.
        let map = |i: usize| match i {
            2 => None,
            i if i > 2 => Some(i - 1),
            i => Some(i),
        };
        assert_eq!(renumber("1;3;4SS+1d;bad", 8.0, &map), "1;3SS+1d;bad");
    }

    #[test]
    fn assignment_lists() {
        let a = assignments("Ana; Luis[50%]; Cemento[20]");
        assert_eq!(a, vec![Assignment { name: "Ana".into(), units: None }, Assignment { name: "Luis".into(), units: Some(0.5) }, Assignment { name: "Cemento".into(), units: Some(20.0) }]);
        assert_eq!(assignments_text(&a[..2]), "Ana;Luis[50%]");
    }
}
