//! Working calendars and working-time arithmetic.
//!
//! A moment is a serial date whose fraction is the part of that day's
//! *working* time already done (not the clock): 45000.5 is half way
//! through the work of day 45000. Work that fills a day ends at
//! `day + END` (just before midnight), so the end of Friday and the start
//! of Monday stay different moments; a start is always written in start
//! form (`day + 0.0`, or later in the day).

/// A day's work ends here (the end form of a day).
pub const END: f64 = 1.0 - 1e-9;

/// Tolerance of moment comparisons, in days.
pub const TOL: f64 = 1e-6;

/// How far a search for a working day goes before giving up (ten years).
const SEARCH: i64 = 3660;

/// Days that differ from the working week: holidays (not working) or
/// working weekends.
#[derive(Clone, Debug, PartialEq)]
pub struct Exception {
    pub name: String,
    /// First and last day (serial dates, inclusive).
    pub from: i64,
    pub to: i64,
    pub working: bool,
}

impl Exception {
    pub fn holiday(name: impl Into<String>, day: i64) -> Exception {
        Exception { name: name.into(), from: day, to: day, working: false }
    }
}

/// A working calendar: the working days of the week, the working hours of
/// a day and the exceptions.
#[derive(Clone, Debug, PartialEq)]
pub struct Calendar {
    pub name: String,
    /// Working days of the week, Monday first.
    pub weekdays: [bool; 7],
    /// Working hours of a working day, as (from, to) in hours.
    pub hours: Vec<(f64, f64)>,
    pub exceptions: Vec<Exception>,
    /// The project calendar's exceptions apply too (a resource calendar
    /// that keeps the project's holidays).
    pub inherit: bool,
}

impl Default for Calendar {
    fn default() -> Self {
        Calendar::standard("Standard")
    }
}

/// Monday = 0 … Sunday = 6 of a serial date (serial 0 is a Saturday).
pub fn weekday(day: i64) -> usize {
    (day + 5).rem_euclid(7) as usize
}

impl Calendar {
    /// Monday to Friday, 8:00-12:00 and 13:00-17:00.
    pub fn standard(name: impl Into<String>) -> Calendar {
        Calendar { name: name.into(), weekdays: [true, true, true, true, true, false, false], hours: vec![(8.0, 12.0), (13.0, 17.0)], exceptions: Vec::new(), inherit: true }
    }

    /// Working hours in a working day (8 by default).
    pub fn hours_per_day(&self) -> f64 {
        let h: f64 = self.hours.iter().map(|(a, b)| (b - a).max(0.0)).sum();
        if h > 0.0 { h } else { 8.0 }
    }

    /// The exception that covers `day`, if any (the last one listed wins).
    pub fn exception(&self, day: i64) -> Option<&Exception> {
        self.exceptions.iter().rev().find(|e| e.from <= day && day <= e.to)
    }

    /// Whether `day` is a working day of this calendar alone (`base`: the
    /// project calendar, whose exceptions apply when `inherit`).
    pub fn is_working(&self, day: i64, base: Option<&Calendar>) -> bool {
        if let Some(e) = self.exception(day) {
            return e.working;
        }
        if self.inherit
            && let Some(e) = base.and_then(|b| b.exception(day))
        {
            return e.working;
        }
        self.weekdays[weekday(day)]
    }

    /// Adds the holidays of `list` that are not there yet.
    pub fn add_holidays(&mut self, list: &[Exception]) {
        for e in list {
            if !self.exceptions.iter().any(|x| x.from == e.from && x.to == e.to) {
                self.exceptions.push(e.clone());
            }
        }
        self.exceptions.sort_by_key(|e| e.from);
    }
}

/// The working time of a task: the project calendar, narrowed by the
/// calendars of its resources (a day works when all of them work).
#[derive(Clone, Copy)]
pub struct WorkTime<'a> {
    pub base: &'a Calendar,
    pub others: &'a [&'a Calendar],
}

impl<'a> WorkTime<'a> {
    pub fn new(base: &'a Calendar) -> WorkTime<'a> {
        WorkTime { base, others: &[] }
    }

    pub fn working(&self, day: i64) -> bool {
        if self.others.is_empty() {
            return self.base.is_working(day, None);
        }
        self.others.iter().all(|c| c.is_working(day, Some(self.base)))
    }

    /// Whether any day in the next ten years works (a calendar with no
    /// working day at all counts every day as working).
    fn any_work(&self, from: i64) -> bool {
        (0..=14).any(|k| self.working(from + k)) || (0..SEARCH).any(|k| self.working(from + k))
    }

    /// The first working day from `day` on.
    pub fn next_working(&self, day: i64) -> i64 {
        (day..day + SEARCH).find(|d| self.working(*d)).unwrap_or(day)
    }

    /// The last working day up to `day`.
    pub fn prev_working(&self, day: i64) -> i64 {
        (day - SEARCH..=day).rev().find(|d| self.working(*d)).unwrap_or(day)
    }

    /// `t` in start form: moved to the start of the next working day when
    /// it falls at the end of a day or on a day off.
    pub fn norm_start(&self, t: f64) -> f64 {
        let (mut day, mut f) = split(t);
        if f >= 1.0 - TOL {
            day += 1;
            f = 0.0;
        }
        if !self.working(day) {
            day = self.next_working(day);
            f = 0.0;
        }
        day as f64 + f
    }

    /// `t` in end form: moved to the end of the previous working day when
    /// it falls at the start of a day or on a day off.
    pub fn norm_end(&self, t: f64) -> f64 {
        let (mut day, mut f) = split(t);
        if f <= TOL {
            day -= 1;
            f = END;
        }
        if !self.working(day) {
            day = self.prev_working(day);
            f = END;
        }
        day as f64 + f
    }

    /// The moment `d` working days after `t` (end form when it fills a
    /// day). Negative `d` goes back.
    pub fn add(&self, t: f64, d: f64) -> f64 {
        if d < 0.0 {
            return self.sub(t, -d);
        }
        if d <= TOL || !self.any_work(t.floor() as i64) {
            return t + d;
        }
        let t = self.norm_start(t);
        let (mut day, mut f) = split(t);
        let mut left = d;
        loop {
            let avail = 1.0 - f;
            if left <= avail + TOL {
                let r = f + left;
                return if r >= 1.0 - TOL { day as f64 + END } else { day as f64 + r };
            }
            left -= avail;
            day = self.next_working(day + 1);
            f = 0.0;
        }
    }

    /// The moment `d` working days before `t` (start form).
    pub fn sub(&self, t: f64, d: f64) -> f64 {
        if d < 0.0 {
            return self.add(t, -d);
        }
        if d <= TOL || !self.any_work(t.floor() as i64 - SEARCH / 2) {
            return t - d;
        }
        let t = self.norm_end(t);
        let (mut day, mut f) = split(t);
        if f >= 1.0 - TOL {
            f = 1.0;
        }
        let mut left = d;
        loop {
            if left <= f + TOL {
                let r = (f - left).max(0.0);
                return day as f64 + if r <= TOL { 0.0 } else { r };
            }
            left -= f;
            day = self.prev_working(day - 1);
            f = 1.0;
        }
    }

    /// Working days from `a` to `b` (negative when `b` is earlier).
    pub fn work(&self, a: f64, b: f64) -> f64 {
        if b < a {
            return -self.work(b, a);
        }
        if b - a <= TOL {
            return 0.0;
        }
        let (da, fa) = split(a);
        let (db, fb) = split(b);
        let frac = |f: f64| if f >= 1.0 - TOL { 1.0 } else { f };
        if da == db {
            return if self.working(da) { frac(fb) - frac(fa) } else { 0.0 };
        }
        let mut w = if self.working(da) { 1.0 - frac(fa) } else { 0.0 };
        // Whole weeks at a time.
        let mut d = da + 1;
        while d + 7 <= db {
            w += (0..7).filter(|k| self.working(d + k)).count() as f64;
            d += 7;
        }
        while d < db {
            if self.working(d) {
                w += 1.0;
            }
            d += 1;
        }
        if self.working(db) {
            w += frac(fb);
        }
        w
    }

    /// The day a task starting at `t` begins work.
    pub fn start_day(&self, t: f64) -> i64 {
        split(self.norm_start(t)).0
    }

    /// The last day a task finishing at `t` works.
    pub fn finish_day(&self, t: f64) -> i64 {
        split(self.norm_end(t)).0
    }

    /// Calendar days `n` after `t` (elapsed time, weekends included).
    pub fn elapsed(&self, t: f64, n: f64) -> f64 {
        t + n
    }
}

/// Day and fraction of a moment.
pub fn split(t: f64) -> (i64, f64) {
    let day = t.floor();
    (day as i64, t - day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::date_to_serial;

    fn d(y: i64, m: i64, dd: i64) -> i64 {
        date_to_serial(y, m, dd) as i64
    }

    #[test]
    fn weekdays() {
        // 2026-09-28 is a Monday.
        assert_eq!(weekday(d(2026, 9, 28)), 0);
        assert_eq!(weekday(d(2026, 10, 4)), 6);
        assert_eq!(weekday(2), 0);
    }

    #[test]
    fn add_and_sub() {
        let c = Calendar::standard("S");
        let w = WorkTime::new(&c);
        let mon = d(2026, 9, 28) as f64;
        // 5 days from Monday end on Friday.
        let f = w.add(mon, 5.0);
        assert_eq!(w.finish_day(f), d(2026, 10, 2));
        // The next task starts on Monday.
        assert_eq!(w.start_day(f), d(2026, 10, 5));
        // 6 days skip the weekend.
        assert_eq!(w.finish_day(w.add(mon, 6.0)), d(2026, 10, 5));
        // Half days.
        let h = w.add(mon, 0.5);
        assert!((h - (mon + 0.5)).abs() < 1e-9);
        assert_eq!(w.finish_day(w.add(h, 1.0)), d(2026, 9, 29));
        // Back from the end of Friday to the start of Monday.
        assert!((w.sub(f, 5.0) - mon).abs() < 1e-9);
        assert!((w.work(mon, f) - 5.0).abs() < 1e-6);
        assert!((w.work(mon, w.add(mon, 23.5)) - 23.5).abs() < 1e-6);
        // Starting on a Saturday means Monday.
        assert_eq!(w.start_day(d(2026, 10, 3) as f64), d(2026, 10, 5));
        assert_eq!(w.finish_day(d(2026, 10, 3) as f64), d(2026, 10, 2));
    }

    #[test]
    fn holidays_and_resources() {
        let mut c = Calendar::standard("S");
        c.exceptions.push(Exception::holiday("Fiesta", d(2026, 10, 12)));
        let w = WorkTime::new(&c);
        let fri = d(2026, 10, 9) as f64;
        // Friday + 2 days: Friday and Tuesday (Monday is a holiday).
        assert_eq!(w.finish_day(w.add(fri, 2.0)), d(2026, 10, 13));
        // A resource that works Saturdays and keeps the project holidays.
        let mut r = Calendar::standard("R");
        r.weekdays[5] = true;
        let others = [&r];
        let rw = WorkTime { base: &c, others: &others };
        assert_eq!(rw.finish_day(rw.add(fri, 2.0)), d(2026, 10, 10));
        assert!(!rw.working(d(2026, 10, 12)));
        // One that does not.
        r.inherit = false;
        let others = [&r];
        let rw = WorkTime { base: &c, others: &others };
        assert!(rw.working(d(2026, 10, 12)));
        // A working Saturday for everyone.
        c.exceptions.push(Exception { name: "Extra".into(), from: d(2026, 10, 17), to: d(2026, 10, 17), working: true });
        assert!(WorkTime::new(&c).working(d(2026, 10, 17)));
    }

    #[test]
    fn no_working_days() {
        let mut c = Calendar::standard("S");
        c.weekdays = [false; 7];
        let w = WorkTime::new(&c);
        assert_eq!(w.add(100.0, 3.0), 103.0);
        assert_eq!(w.sub(100.0, 3.0), 97.0);
    }
}
