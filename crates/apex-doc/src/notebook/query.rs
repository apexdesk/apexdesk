//! Queries: the pages of a type, a section or a tag whose fields and
//! dates meet some conditions, without writing code. Saved ones keep a
//! name.

use super::types::number;
use super::{Notebook, fold};

/// How a field is compared with a condition's value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Op {
    #[default]
    Is,
    IsNot,
    Contains,
    /// Numbers as numbers; dates and text in order.
    Less,
    More,
    Empty,
    NotEmpty,
}

impl Op {
    pub const ALL: [Op; 7] = [Op::Is, Op::IsNot, Op::Contains, Op::Less, Op::More, Op::Empty, Op::NotEmpty];

    pub fn key(self) -> &'static str {
        match self {
            Op::Is => "is",
            Op::IsNot => "not",
            Op::Contains => "has",
            Op::Less => "lt",
            Op::More => "gt",
            Op::Empty => "empty",
            Op::NotEmpty => "set",
        }
    }

    pub fn from_key(k: &str) -> Op {
        Op::ALL.into_iter().find(|o| o.key() == k).unwrap_or_default()
    }

    /// Whether it compares with a value at all.
    pub fn takes_value(self) -> bool {
        !matches!(self, Op::Empty | Op::NotEmpty)
    }

    fn test(self, field: &str, value: &str) -> bool {
        let (f, v) = (fold(field.trim()), fold(value.trim()));
        let order = || match (number(field), number(value)) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            _ => Some(f.cmp(&v)),
        };
        match self {
            Op::Is => f == v,
            Op::IsNot => f != v,
            Op::Contains => f.contains(&v),
            Op::Less => !f.is_empty() && order() == Some(std::cmp::Ordering::Less),
            Op::More => !f.is_empty() && order() == Some(std::cmp::Ordering::Greater),
            Op::Empty => f.is_empty(),
            Op::NotEmpty => !f.is_empty(),
        }
    }
}

/// A condition on a field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cond {
    pub field: String,
    pub op: Op,
    pub value: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub id: i64,
    pub name: String,
    pub ptype: Option<i64>,
    pub section: Option<i64>,
    /// A `#tag` (without the #; "" any). Tags under it count too.
    pub tag: String,
    pub conds: Vec<Cond>,
    /// Dates (yyyy-mm-dd, "" open) the page was changed between, or made
    /// between with `by_created`.
    pub since: String,
    pub until: String,
    pub by_created: bool,
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

impl Query {
    /// How many filters it has besides the type and the section.
    pub fn active_filters(&self) -> usize {
        usize::from(!self.tag.trim().is_empty()) + usize::from(!self.since.is_empty() || !self.until.is_empty()) + self.conds.len()
    }

    /// As kept in the file: "key\tvalue" lines, a condition per line.
    pub fn key(&self) -> String {
        let mut v = Vec::new();
        if let Some(t) = self.ptype {
            v.push(format!("type\t{t}"));
        }
        if let Some(s) = self.section {
            v.push(format!("section\t{s}"));
        }
        for (k, val) in [("tag", &self.tag), ("since", &self.since), ("until", &self.until)] {
            if !val.is_empty() {
                v.push(format!("{k}\t{}", clean(val)));
            }
        }
        if self.by_created {
            v.push("created\t1".into());
        }
        for c in &self.conds {
            v.push(format!("cond\t{}\t{}\t{}", clean(&c.field), c.op.key(), clean(&c.value)));
        }
        v.join("\n")
    }

    pub fn from_key(id: i64, name: &str, s: &str) -> Query {
        let mut q = Query { id, name: name.to_string(), ..Query::default() };
        for line in s.lines() {
            let mut it = line.split('\t');
            let (Some(k), Some(v)) = (it.next(), it.next()) else { continue };
            match k {
                "type" => q.ptype = v.parse().ok(),
                "section" => q.section = v.parse().ok(),
                "tag" => q.tag = v.to_string(),
                "since" => q.since = v.to_string(),
                "until" => q.until = v.to_string(),
                "created" => q.by_created = v == "1",
                "cond" => q.conds.push(Cond { field: v.to_string(), op: Op::from_key(it.next().unwrap_or("")), value: it.next().unwrap_or("").to_string() }),
                _ => {}
            }
        }
        q
    }
}

impl Notebook {
    /// The live pages `q` finds, by title. `day` gives the date
    /// (yyyy-mm-dd) of a time in seconds.
    pub fn run_query(&self, q: &Query, day: &dyn Fn(i64) -> String) -> Vec<i64> {
        let tagged = (!q.tag.trim().is_empty()).then(|| self.tagged(q.tag.trim().trim_start_matches('#')));
        let mut v: Vec<&super::Page> = self
            .pages
            .iter()
            .filter(|p| self.is_live(p.id))
            .filter(|p| q.ptype.is_none_or(|t| p.ptype == Some(t)))
            .filter(|p| q.section.is_none_or(|s| p.section == s))
            .filter(|p| tagged.as_ref().is_none_or(|t| t.contains(&p.id)))
            .filter(|p| {
                if q.since.is_empty() && q.until.is_empty() {
                    return true;
                }
                let d = day(if q.by_created { p.created } else { p.modified });
                (q.since.is_empty() || d >= q.since) && (q.until.is_empty() || d <= q.until)
            })
            // A condition still without its value doesn't filter yet.
            .filter(|p| q.conds.iter().filter(|c| !c.op.takes_value() || !c.value.trim().is_empty()).all(|c| c.op.test(self.field_value(p.id, &c.field), &c.value)))
            .collect();
        v.sort_by_key(|p| fold(&p.title));
        v.into_iter().map(|p| p.id).collect()
    }

    pub fn query(&self, id: i64) -> Option<&Query> {
        self.queries.iter().find(|q| q.id == id)
    }

    /// Saves `q` under `name` as a new query. Returns its id.
    pub fn save_query(&mut self, q: &Query, name: &str) -> i64 {
        let id = self.queries.iter().map(|q| q.id).max().unwrap_or(0) + 1;
        self.queries.push(Query { id, name: clean(name.trim()), ..q.clone() });
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_filters_by_type_field_tag_and_date() {
        let mut nb = Notebook::default();
        let s = nb.add_section("Notes", 0);
        let t = nb.add_type("Personaje");
        let ana = nb.add_page(s, None, None, "Ana", 86_400 * 10);
        let bea = nb.add_page(s, None, None, "Bea", 86_400 * 20);
        let faro = nb.add_page(s, None, None, "Faro", 86_400 * 20);
        for (p, age, role) in [(ana, "34", "Protagonista"), (bea, "9", "Secundario")] {
            nb.page_mut(p).unwrap().ptype = Some(t);
            nb.set_field(p, "Edad", age);
            nb.set_field(p, "Rol", role);
        }
        nb.texts.insert(bea, "Vive en #lugar/costa".into());
        let day = |s: i64| format!("1970-01-{:02}", s / 86_400 + 1);
        let all = Query { ptype: Some(t), ..Query::default() };
        assert_eq!(nb.run_query(&all, &day), vec![ana, bea]);
        // Numbers compare as numbers: 9 < 20 < 34.
        let older = Query { conds: vec![Cond { field: "Edad".into(), op: Op::More, value: "20".into() }], ..all.clone() };
        assert_eq!(nb.run_query(&older, &day), vec![ana]);
        let role = Query { conds: vec![Cond { field: "Rol".into(), op: Op::Is, value: "secundario".into() }], ..all.clone() };
        assert_eq!(nb.run_query(&role, &day), vec![bea]);
        assert_eq!(nb.run_query(&Query { tag: "#lugar".into(), ..Query::default() }, &day), vec![bea]);
        let recent = Query { since: "1970-01-15".into(), ..Query::default() };
        assert_eq!(nb.run_query(&recent, &day), vec![bea, faro]);
        let id = nb.save_query(&older, "Mayores");
        let q = nb.query(id).unwrap();
        assert_eq!(Query::from_key(id, &q.name, &q.key()), *q);
    }
}
