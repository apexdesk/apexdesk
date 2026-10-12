//! Custom lists for fills and sorts: the built-in weekday and month lists
//! (English and Spanish) and the user's own lists (kept in
//! `analysis::State::custom_lists`). Filling from "Mon" gives "Tue",
//! "Wed"...; from "ene", "mar" (a step of two) gives "may", "jul"...

/// Built-in lists, in the order they are tried.
pub const BUILTIN: [&[&str]; 8] = [
    &["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
    &["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"],
    &["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
    &["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"],
    &["lun", "mar", "mié", "jue", "vie", "sáb", "dom"],
    &["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"],
    &["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"],
    &["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"],
];

/// All lists (the user's first), as owned strings, for a settings dialog.
pub fn all(user: &[Vec<String>]) -> Vec<Vec<String>> {
    user.iter().cloned().chain(BUILTIN.iter().map(|l| l.iter().map(|s| s.to_string()).collect())).collect()
}

/// Parses the text of a list typed in the dialog: items separated by
/// commas or new lines.
pub fn parse_list(text: &str) -> Vec<String> {
    text.split([',', '\n', ';']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    AsList,
    Upper,
    Lower,
}

fn style_of(s: &str) -> Style {
    let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() > 1 && letters.iter().all(|c| c.is_uppercase()) {
        Style::Upper
    } else if letters.iter().all(|c| c.is_lowercase()) {
        Style::Lower
    } else {
        Style::AsList
    }
}

fn pos(list: &[&str], s: &str) -> Option<usize> {
    let s = s.trim().to_lowercase();
    list.iter().position(|x| x.to_lowercase() == s)
}

/// The list item `k` steps from `seq[0]`, following the step between the
/// items of `seq` (all of which must belong to one list). None when `seq`
/// isn't a list sequence.
pub fn series_item(seq: &[String], k: i64, user: &[Vec<String>]) -> Option<String> {
    let first = seq.first()?;
    let user_lists: Vec<Vec<&str>> = user.iter().map(|l| l.iter().map(String::as_str).collect()).collect();
    let lists = user_lists.iter().map(Vec::as_slice).chain(BUILTIN.iter().copied());
    for list in lists {
        let len = list.len() as i64;
        if len == 0 {
            continue;
        }
        let Some(idx) = seq.iter().map(|s| pos(list, s)).collect::<Option<Vec<usize>>>() else { continue };
        let step = if idx.len() >= 2 { (idx[1] as i64 - idx[0] as i64).rem_euclid(len) } else { 1 };
        if idx.windows(2).any(|w| (w[1] as i64 - w[0] as i64).rem_euclid(len) != step) {
            continue;
        }
        let item = list[(idx[0] as i64 + step * k).rem_euclid(len) as usize];
        // Keep the typed case (MON -> TUE, lunes -> martes).
        return Some(match style_of(first) {
            Style::Upper => item.to_uppercase(),
            Style::Lower if item.chars().any(char::is_uppercase) && first.chars().any(char::is_alphabetic) => item.to_lowercase(),
            _ => item.to_string(),
        });
    }
    None
}

/// The `count` items that follow `seq`.
pub fn continue_series(seq: &[String], count: usize, user: &[Vec<String>]) -> Option<Vec<String>> {
    let n = seq.len() as i64;
    (0..count as i64).map(|i| series_item(seq, n + i, user)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn builtin_lists() {
        assert_eq!(continue_series(&s(&["Mon"]), 3, &[]).unwrap(), s(&["Tue", "Wed", "Thu"]));
        assert_eq!(continue_series(&s(&["Friday"]), 3, &[]).unwrap(), s(&["Saturday", "Sunday", "Monday"]));
        assert_eq!(continue_series(&s(&["JAN", "MAR"]), 2, &[]).unwrap(), s(&["MAY", "JUL"]));
        assert_eq!(continue_series(&s(&["lunes"]), 2, &[]).unwrap(), s(&["martes", "miércoles"]));
        // "mar" alone is Tuesday in the Spanish day list... unless the
        // English month list matches first.
        assert_eq!(continue_series(&s(&["mar", "mié"]), 1, &[]).unwrap(), s(&["jue"]));
        assert_eq!(continue_series(&s(&["dic"]), 1, &[]).unwrap(), s(&["ene"]));
        assert_eq!(series_item(&s(&["Wed"]), -1, &[]).unwrap(), "Tue");
        assert!(series_item(&s(&["Mon", "Wed", "Thu"]), 3, &[]).is_none());
        assert!(series_item(&s(&["hello"]), 1, &[]).is_none());
    }

    #[test]
    fn user_lists_win() {
        let user = vec![s(&["North", "South", "East", "West"])];
        assert_eq!(continue_series(&s(&["south"]), 3, &user).unwrap(), s(&["east", "west", "north"]));
        assert_eq!(parse_list("a, b\nc;; d"), s(&["a", "b", "c", "d"]));
        assert_eq!(all(&user).len(), 9);
    }
}
