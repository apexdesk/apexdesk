//! Reduces a failing file while it keeps failing the same way: parts of
//! a package, then elements of its XML parts, then bytes; streams and
//! records of a compound file; chunks of a text file (delta debugging).

use std::time::Instant;

use super::{ole, xml, zip};

pub struct Budget {
    pub until: Instant,
    pub tests: usize,
}

impl Budget {
    fn spent(&self) -> bool {
        self.tests == 0 || Instant::now() > self.until
    }
}

/// `fails` runs a candidate and says whether it still fails the same way.
pub struct Ctx<'a> {
    pub fails: &'a mut dyn FnMut(&[u8]) -> bool,
    pub budget: Budget,
}

impl Ctx<'_> {
    fn test(&mut self, d: &[u8]) -> bool {
        if self.budget.spent() {
            return false;
        }
        self.budget.tests -= 1;
        (self.fails)(d)
    }
}

pub fn minimize(data: &[u8], ctx: &mut Ctx) -> Vec<u8> {
    let caught = |f: &dyn Fn() -> Option<Vec<zip::Part>>| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok().flatten();
    if data.starts_with(b"PK") {
        if let Some(ps) = caught(&|| zip::parts(data)) {
            let flat = zip::write(&ps, false);
            if flat.len() <= data.len() * 4 && ctx.test(&flat) {
                return zip::write(&package(ps, ctx), false);
            }
        }
        return ddmin(data.to_vec(), ctx);
    }
    if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        let s = std::panic::catch_unwind(|| ole::read(data)).ok().flatten();
        if let Some(s) = s
            && let Some(flat) = ole::write(&s)
            && ctx.test(&flat)
        {
            return compound(s, ctx).unwrap_or(flat);
        }
        return truncate(data.to_vec(), ctx);
    }
    ddmin(data.to_vec(), ctx)
}

fn package(mut ps: Vec<zip::Part>, ctx: &mut Ctx) -> Vec<zip::Part> {
    // Parts.
    let mut i = 0;
    while i < ps.len() && !ctx.budget.spent() {
        let mut c = ps.clone();
        c.remove(i);
        if ctx.test(&zip::write(&c, false)) {
            ps = c;
        } else {
            i += 1;
        }
    }
    // Elements of the XML parts, biggest parts first.
    let mut order: Vec<usize> = (0..ps.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(ps[i].data.len()));
    for i in order {
        if ctx.budget.spent() {
            break;
        }
        let Ok(text) = String::from_utf8(ps[i].data.clone()) else {
            let d = ps[i].data.clone();
            let mut c = ps.clone();
            let small = ddmin_with(d, ctx, &mut |x| {
                c[i].data = x.to_vec();
                zip::write(&c, false)
            });
            ps[i].data = small;
            continue;
        };
        // Blank runs (compression bombs) first.
        let squeezed = squeeze(&text);
        let mut text = text;
        if squeezed.len() < text.len() {
            let mut c = ps.clone();
            c[i].data = squeezed.clone().into_bytes();
            if ctx.test(&zip::write(&c, false)) {
                text = squeezed;
            }
        }
        text = elements(text, ctx, &mut |t| {
            let mut c = ps.clone();
            c[i].data = t.as_bytes().to_vec();
            zip::write(&c, false)
        });
        ps[i].data = text.into_bytes();
    }
    ps
}

/// Blank runs made single spaces.
fn squeeze(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(1 << 20));
    let mut blank = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !blank {
                out.push(' ');
            }
            blank = true;
        } else {
            out.push(c);
            blank = false;
        }
    }
    out
}

/// Removes elements of an XML text, biggest first; `wrap` builds the
/// whole file around a candidate text.
fn elements(mut text: String, ctx: &mut Ctx, wrap: &mut dyn FnMut(&str) -> Vec<u8>) -> String {
    let mut skip = 0;
    loop {
        if ctx.budget.spent() {
            return text;
        }
        let mut els = xml::elements(&text);
        // The root stays (a document without it is another failure).
        let root = els.first().map(|e| e.start);
        els.sort_by_key(|e| std::cmp::Reverse(e.end - e.start));
        let Some(e) = els.iter().filter(|e| Some(e.start) != root).nth(skip) else {
            return text;
        };
        let mut c = text.clone();
        c.replace_range(e.start..e.end, "");
        if ctx.test(&wrap(&c)) {
            text = c;
        } else {
            skip += 1;
        }
    }
}

fn compound(mut s: ole::Streams, ctx: &mut Ctx) -> Option<Vec<u8>> {
    let mut i = 0;
    while i < s.streams.len() && !ctx.budget.spent() {
        let removed = s.streams.remove(i);
        if ole::write(&s).is_some_and(|d| ctx.test(&d)) {
            continue;
        }
        s.streams.insert(i, removed);
        i += 1;
    }
    // Records of a workbook stream, then bytes of the other streams.
    for i in 0..s.streams.len() {
        if ctx.budget.spent() {
            break;
        }
        let d = s.streams[i].1.clone();
        let name = s.streams[i].0.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let mut c = ole::Streams { storages: s.storages.clone(), streams: s.streams.clone() };
        let small = if name == "Workbook" || name == "Book" {
            records(d, ctx, &mut |x| {
                c.streams[i].1 = x.to_vec();
                ole::write(&c).unwrap_or_default()
            })
        } else {
            ddmin_with(d, ctx, &mut |x| {
                c.streams[i].1 = x.to_vec();
                ole::write(&c).unwrap_or_default()
            })
        };
        s.streams[i].1 = small;
    }
    ole::write(&s)
}

/// Removes BIFF records one at a time.
fn records(mut d: Vec<u8>, ctx: &mut Ctx, wrap: &mut dyn FnMut(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let mut k = 1;
    loop {
        if ctx.budget.spent() {
            return d;
        }
        let recs = ole::biff_records(&d);
        let Some(&(p, _, n)) = recs.get(k) else { return d };
        let mut c = d.clone();
        c.drain(p..p + 4 + n);
        if ctx.test(&wrap(&c)) {
            d = c;
        } else {
            k += 1;
        }
    }
}

/// Shortens a file that can't be taken apart: the shortest prefix that
/// still fails, by halves.
fn truncate(mut d: Vec<u8>, ctx: &mut Ctx) -> Vec<u8> {
    let mut step = d.len() / 2;
    while step >= 64 && !ctx.budget.spent() {
        let c = d[..d.len() - step].to_vec();
        if ctx.test(&c) {
            d = c;
        } else {
            step /= 2;
        }
    }
    d
}

/// Classic delta debugging on bytes: remove chunks, halving their size.
fn ddmin(d: Vec<u8>, ctx: &mut Ctx) -> Vec<u8> {
    ddmin_with(d, ctx, &mut |x| x.to_vec())
}

fn ddmin_with(mut d: Vec<u8>, ctx: &mut Ctx, wrap: &mut dyn FnMut(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let mut chunk = (d.len() / 2).max(1);
    loop {
        let mut i = 0;
        while i < d.len() && !ctx.budget.spent() {
            let end = (i + chunk).min(d.len());
            let mut c = d.clone();
            c.drain(i..end);
            if ctx.test(&wrap(&c)) {
                d = c;
            } else {
                i += chunk;
            }
        }
        if chunk == 1 || ctx.budget.spent() {
            return d;
        }
        chunk /= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn budget() -> Budget {
        Budget { until: Instant::now() + Duration::from_secs(30), tests: 10_000 }
    }

    #[test]
    fn text_shrinks_to_the_cause() {
        let data = b"lots of harmless text BOOM and more harmless text".to_vec();
        let mut fails = |d: &[u8]| d.windows(4).any(|w| w == b"BOOM");
        let mut ctx = Ctx { fails: &mut fails, budget: budget() };
        assert_eq!(minimize(&data, &mut ctx), b"BOOM");
    }

    #[test]
    fn packages_lose_parts_and_elements() {
        let ps = vec![
            zip::Part { name: "a.xml".into(), data: b"<r><x>1</x><bad v=\"-1\"/><y><z/></y></r>".to_vec() },
            zip::Part { name: "b.xml".into(), data: b"<r/>".to_vec() },
        ];
        let data = zip::write(&ps, false);
        let mut fails = |d: &[u8]| zip::parts(d).is_some_and(|ps| ps.iter().any(|p| p.data.windows(4).any(|w| w == b"-1\"/")));
        let mut ctx = Ctx { fails: &mut fails, budget: budget() };
        let small = zip::parts(&minimize(&data, &mut ctx)).unwrap();
        assert_eq!(small.len(), 1);
        assert_eq!(small[0].data, b"<r><bad v=\"-1\"/></r>");
    }
}
