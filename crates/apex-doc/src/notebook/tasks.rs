//! Tasks: the check boxes of a notebook's pages. A box's tag keeps its
//! due date, priority and board column ("due=2026-10-05;pri=2;col=Doing").

use super::Notebook;
use crate::{CHECKED, ControlKind, Document, UNCHECKED, para_controls};

/// Highest priority (0 is none, then low, medium, high).
pub const TOP_PRIORITY: u8 = 3;

/// What a task's box keeps besides being checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskMeta {
    /// yyyy-mm-dd.
    pub due: Option<String>,
    pub priority: u8,
    /// The board column it was put in (by name).
    pub col: Option<String>,
}

impl TaskMeta {
    pub fn from_tag(tag: &str) -> TaskMeta {
        let mut m = TaskMeta::default();
        for part in tag.split(';') {
            match part.split_once('=') {
                Some(("due", d)) if is_date(d) => m.due = Some(d.to_string()),
                Some(("pri", p)) => m.priority = p.parse::<u8>().unwrap_or(0).min(TOP_PRIORITY),
                Some(("col", c)) if !c.is_empty() => m.col = Some(c.to_string()),
                _ => {}
            }
        }
        m
    }

    pub fn tag(&self) -> String {
        let mut parts = Vec::new();
        if let Some(d) = &self.due {
            parts.push(format!("due={d}"));
        }
        if self.priority > 0 {
            parts.push(format!("pri={}", self.priority));
        }
        if let Some(c) = &self.col {
            parts.push(format!("col={c}"));
        }
        parts.join(";")
    }
}

/// A board column's name as a tag can keep it: no ';', '=' or line breaks.
pub fn column_name(s: &str) -> String {
    s.replace([';', '=', '\n', '\r'], " ").trim().to_string()
}

/// The board column a task goes in: the last column is for the tasks
/// done; a task not done goes in its column, or the first one.
pub fn column_of(t: &Task, cols: &[String]) -> usize {
    let last = cols.len().saturating_sub(1);
    if t.done {
        return last;
    }
    t.meta.col.as_ref().and_then(|c| cols[..last].iter().position(|n| n == c)).unwrap_or(0)
}

/// Whether `s` is a date as yyyy-mm-dd.
pub fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// A check box of a page and the text beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub para: usize,
    pub done: bool,
    pub text: String,
    pub meta: TaskMeta,
}

/// The tasks of a page: each paragraph with a check box (its first one).
pub fn tasks_in(doc: &Document) -> Vec<Task> {
    doc.paragraphs
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let c = para_controls(p, i).into_iter().find(|c| matches!(c.control.kind, ControlKind::CheckBox { .. }))?;
            let done = matches!(c.control.kind, ControlKind::CheckBox { checked: true });
            let text = p.text.replace([CHECKED, UNCHECKED], "").trim().to_string();
            Some(Task { para: i, done, text, meta: TaskMeta::from_tag(&c.control.tag) })
        })
        .collect()
}

impl Notebook {
    /// The tasks of the live pages, in notebook order.
    pub fn all_tasks(&self) -> Vec<(i64, &Task)> {
        self.live_sections().iter().flat_map(|s| self.tree(s.id)).flat_map(|(_, p)| self.tasks.get(&p.id).into_iter().flatten().map(move |t| (p.id, t))).collect()
    }
}

#[cfg(test)]
mod board_tests {
    use super::*;

    fn task(done: bool, col: Option<&str>) -> Task {
        Task { para: 0, done, text: String::new(), meta: TaskMeta { col: col.map(str::to_string), ..Default::default() } }
    }

    #[test]
    fn a_tag_keeps_the_column() {
        let m = TaskMeta { due: Some("2026-10-05".into()), priority: 2, col: Some("Doing".into()) };
        assert_eq!(m.tag(), "due=2026-10-05;pri=2;col=Doing");
        assert_eq!(TaskMeta::from_tag(&m.tag()), m);
        assert_eq!(TaskMeta::from_tag("due=2026-10-05").col, None);
    }

    #[test]
    fn done_tasks_go_in_the_last_column() {
        let cols: Vec<String> = ["To do", "Doing", "Done"].map(String::from).to_vec();
        assert_eq!(column_of(&task(false, None), &cols), 0);
        assert_eq!(column_of(&task(false, Some("Doing")), &cols), 1);
        assert_eq!(column_of(&task(true, Some("Doing")), &cols), 2);
        // a column that is gone, or the last one without the box checked
        assert_eq!(column_of(&task(false, Some("Review")), &cols), 0);
        assert_eq!(column_of(&task(false, Some("Done")), &cols), 0);
    }

    #[test]
    fn a_column_name_fits_in_a_tag() {
        assert_eq!(column_name(" In; review=now\n"), "In  review now");
    }
}
