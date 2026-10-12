//! Sample national holidays, to start a project calendar from. Only the
//! days off of the whole country (regional ones vary); the user edits the
//! list.

use super::calendar::{Exception, weekday};
use crate::format::date_to_serial;

/// Countries with sample holidays: (code, name in its language).
pub const COUNTRIES: [(&str, &str); 11] = [
    ("ES", "España"),
    ("MX", "México"),
    ("AR", "Argentina"),
    ("US", "United States"),
    ("GB", "United Kingdom"),
    ("FR", "France"),
    ("DE", "Deutschland"),
    ("IT", "Italia"),
    ("PT", "Portugal"),
    ("BR", "Brasil"),
    ("JP", "日本"),
];

fn day(y: i64, m: i64, d: i64) -> i64 {
    date_to_serial(y, m, d) as i64
}

/// Easter Sunday (Gregorian, anonymous algorithm).
pub fn easter(y: i64) -> i64 {
    let a = y % 19;
    let b = y / 100;
    let c = y % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let dd = (h + l - 7 * m + 114) % 31 + 1;
    day(y, month, dd)
}

/// The `n`th (1-based) `wd` (Monday = 0) of a month; `n` = 0: the last.
fn nth(y: i64, m: i64, wd: usize, n: i64) -> i64 {
    if n == 0 {
        let last = day(y, m + 1, 1) - 1;
        return last - ((weekday(last) + 7 - wd) % 7) as i64;
    }
    let first = day(y, m, 1);
    first + ((wd + 7 - weekday(first)) % 7) as i64 + 7 * (n - 1)
}

/// The sample holidays of `country` in year `y`, by date.
pub fn holidays(country: &str, y: i64) -> Vec<Exception> {
    let e = easter(y);
    let f = |m: i64, d: i64| day(y, m, d);
    let list: Vec<(&str, i64)> = match country {
        "ES" => vec![("Año Nuevo", f(1, 1)), ("Epifanía del Señor", f(1, 6)), ("Viernes Santo", e - 2), ("Fiesta del Trabajo", f(5, 1)), ("Asunción de la Virgen", f(8, 15)), ("Fiesta Nacional de España", f(10, 12)), ("Todos los Santos", f(11, 1)), ("Día de la Constitución", f(12, 6)), ("Inmaculada Concepción", f(12, 8)), ("Navidad", f(12, 25))],
        "MX" => vec![("Año Nuevo", f(1, 1)), ("Día de la Constitución", nth(y, 2, 0, 1)), ("Natalicio de Benito Juárez", nth(y, 3, 0, 3)), ("Día del Trabajo", f(5, 1)), ("Día de la Independencia", f(9, 16)), ("Día de la Revolución", nth(y, 11, 0, 3)), ("Navidad", f(12, 25))],
        "AR" => vec![("Año Nuevo", f(1, 1)), ("Carnaval", e - 48), ("Carnaval", e - 47), ("Día de la Memoria", f(3, 24)), ("Día de las Malvinas", f(4, 2)), ("Viernes Santo", e - 2), ("Día del Trabajador", f(5, 1)), ("Revolución de Mayo", f(5, 25)), ("Paso a la Inmortalidad de Belgrano", f(6, 20)), ("Día de la Independencia", f(7, 9)), ("Inmaculada Concepción", f(12, 8)), ("Navidad", f(12, 25))],
        "US" => vec![("New Year's Day", f(1, 1)), ("Martin Luther King Jr. Day", nth(y, 1, 0, 3)), ("Presidents' Day", nth(y, 2, 0, 3)), ("Memorial Day", nth(y, 5, 0, 0)), ("Juneteenth", f(6, 19)), ("Independence Day", f(7, 4)), ("Labor Day", nth(y, 9, 0, 1)), ("Columbus Day", nth(y, 10, 0, 2)), ("Veterans Day", f(11, 11)), ("Thanksgiving Day", nth(y, 11, 3, 4)), ("Christmas Day", f(12, 25))],
        "GB" => vec![("New Year's Day", f(1, 1)), ("Good Friday", e - 2), ("Easter Monday", e + 1), ("Early May Bank Holiday", nth(y, 5, 0, 1)), ("Spring Bank Holiday", nth(y, 5, 0, 0)), ("Summer Bank Holiday", nth(y, 8, 0, 0)), ("Christmas Day", f(12, 25)), ("Boxing Day", f(12, 26))],
        "FR" => vec![("Jour de l'an", f(1, 1)), ("Lundi de Pâques", e + 1), ("Fête du Travail", f(5, 1)), ("Victoire 1945", f(5, 8)), ("Ascension", e + 39), ("Lundi de Pentecôte", e + 50), ("Fête nationale", f(7, 14)), ("Assomption", f(8, 15)), ("Toussaint", f(11, 1)), ("Armistice 1918", f(11, 11)), ("Noël", f(12, 25))],
        "DE" => vec![("Neujahr", f(1, 1)), ("Karfreitag", e - 2), ("Ostermontag", e + 1), ("Tag der Arbeit", f(5, 1)), ("Christi Himmelfahrt", e + 39), ("Pfingstmontag", e + 50), ("Tag der Deutschen Einheit", f(10, 3)), ("1. Weihnachtstag", f(12, 25)), ("2. Weihnachtstag", f(12, 26))],
        "IT" => vec![("Capodanno", f(1, 1)), ("Epifania", f(1, 6)), ("Lunedì dell'Angelo", e + 1), ("Festa della Liberazione", f(4, 25)), ("Festa del Lavoro", f(5, 1)), ("Festa della Repubblica", f(6, 2)), ("Ferragosto", f(8, 15)), ("Ognissanti", f(11, 1)), ("Immacolata Concezione", f(12, 8)), ("Natale", f(12, 25)), ("Santo Stefano", f(12, 26))],
        "PT" => vec![("Ano Novo", f(1, 1)), ("Sexta-feira Santa", e - 2), ("Páscoa", e), ("Dia da Liberdade", f(4, 25)), ("Dia do Trabalhador", f(5, 1)), ("Corpo de Deus", e + 60), ("Dia de Portugal", f(6, 10)), ("Assunção de Nossa Senhora", f(8, 15)), ("Implantação da República", f(10, 5)), ("Dia de Todos os Santos", f(11, 1)), ("Restauração da Independência", f(12, 1)), ("Imaculada Conceição", f(12, 8)), ("Natal", f(12, 25))],
        "BR" => vec![("Confraternização Universal", f(1, 1)), ("Sexta-feira Santa", e - 2), ("Tiradentes", f(4, 21)), ("Dia do Trabalho", f(5, 1)), ("Independência do Brasil", f(9, 7)), ("Nossa Senhora Aparecida", f(10, 12)), ("Finados", f(11, 2)), ("Proclamação da República", f(11, 15)), ("Dia da Consciência Negra", f(11, 20)), ("Natal", f(12, 25))],
        "JP" => {
            // Equinoxes: the usual approximation, good for 1980-2099.
            let k = y - 1980;
            let spring = (20.8431 + 0.242194 * k as f64).floor() as i64 - k / 4;
            let autumn = (23.2488 + 0.242194 * k as f64).floor() as i64 - k / 4;
            vec![("元日", f(1, 1)), ("成人の日", nth(y, 1, 0, 2)), ("建国記念の日", f(2, 11)), ("天皇誕生日", f(2, 23)), ("春分の日", f(3, spring)), ("昭和の日", f(4, 29)), ("憲法記念日", f(5, 3)), ("みどりの日", f(5, 4)), ("こどもの日", f(5, 5)), ("海の日", nth(y, 7, 0, 3)), ("山の日", f(8, 11)), ("敬老の日", nth(y, 9, 0, 3)), ("秋分の日", f(9, autumn)), ("スポーツの日", nth(y, 10, 0, 2)), ("文化の日", f(11, 3)), ("勤労感謝の日", f(11, 23))]
        }
        _ => Vec::new(),
    };
    let mut out: Vec<Exception> = list.into_iter().map(|(n, d)| Exception::holiday(n, d)).collect();
    out.sort_by_key(|e| e.from);
    out
}

/// The holidays of `country` for the years `from..=to`.
pub fn holidays_between(country: &str, from: i64, to: i64) -> Vec<Exception> {
    (from..=to.max(from)).flat_map(|y| holidays(country, y)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easter_dates() {
        assert_eq!(easter(2024), day(2024, 3, 31));
        assert_eq!(easter(2025), day(2025, 4, 20));
        assert_eq!(easter(2026), day(2026, 4, 5));
        assert_eq!(easter(2027), day(2027, 3, 28));
    }

    #[test]
    fn sample_holidays() {
        let es = holidays("ES", 2026);
        assert!(es.iter().any(|h| h.from == day(2026, 4, 3) && h.name == "Viernes Santo"));
        assert!(es.iter().any(|h| h.from == day(2026, 10, 12)));
        // Thanksgiving 2026: 26 November; Memorial Day: 25 May.
        let us = holidays("US", 2026);
        assert!(us.iter().any(|h| h.from == day(2026, 11, 26)));
        assert!(us.iter().any(|h| h.from == day(2026, 5, 25)));
        assert!(holidays("JP", 2026).iter().any(|h| h.from == day(2026, 3, 20)));
        assert!(holidays("XX", 2026).is_empty());
        for (c, _) in COUNTRIES {
            let l = holidays(c, 2027);
            assert!(!l.is_empty(), "{c}");
            assert!(l.windows(2).all(|w| w[0].from <= w[1].from));
        }
        assert_eq!(holidays_between("DE", 2026, 2027).len(), 18);
    }
}
