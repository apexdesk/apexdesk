//! Custom fields of a plan (Planning > Custom Field): columns of the
//! user's whose values come from a formula over the task's columns, rolled
//! up into the summary tasks, with an optional traffic light (an icon set
//! of conditional formatting, which Excel and LibreOffice show too).
//!
//! The values are written as plain values after every edit, like the
//! scheduler's, so other programs see them; the definitions are kept with
//! the plan.

use super::{Op, TableId, Workbook};
use crate::addr::{CellRef, Range};
use crate::data::{CfKind, CfRule, CfStyle, Cfvo};
use crate::formula;
use crate::functions::Env;
use crate::plan::{CustomField, Rollup};
use crate::value::Value;

/// A traffic light on a custom field: icons by thresholds.
#[derive(Clone, Debug, PartialEq)]
pub struct Indicator {
    /// Icon set (see [`crate::data::ICON_SETS`]), of three icons.
    pub set: String,
    /// Values from `low` show the second icon, from `high` the third.
    pub low: f64,
    pub high: f64,
    /// The icons in reverse order (low values green).
    pub reverse: bool,
}

impl Workbook {
    /// The data rows of column `name` of plan `id`.
    fn custom_range(&self, id: TableId, name: &str) -> Option<Range> {
        let t = self.table(id)?;
        let c = t.column_index(name)?;
        let (d0, d1) = t.data_rows();
        let col = t.col_at(c);
        Some(Range::new(CellRef::new(d0, col), CellRef::new(d1.max(d0), col)))
    }

    /// The traffic light on custom column `name`, if there is one.
    pub fn plan_indicator(&self, id: TableId, name: &str) -> Option<Indicator> {
        let r = self.custom_range(id, name)?;
        self.sheets[id.0].cond_formats.iter().find(|x| x.ranges == [r]).and_then(|x| match &x.kind {
            CfKind::IconSet { set, reverse, points, .. } => {
                let num = |p: Option<&Cfvo>| match p {
                    Some(Cfvo::Num(n)) => Some(*n),
                    _ => None,
                };
                Some(Indicator { set: set.clone(), low: num(points.first())?, high: num(points.get(1))?, reverse: *reverse })
            }
            _ => None,
        })
    }

    /// Adds or changes custom field `field` of plan `id` (its column is
    /// added when missing) with traffic light `light`, in one step.
    pub fn plan_set_custom(&mut self, id: TableId, field: CustomField, light: Option<Indicator>) {
        let Some(t) = self.table(id) else { return };
        let Some(mut plan) = t.plan.as_deref().cloned() else { return };
        let name = field.column.trim().to_string();
        if name.is_empty() {
            return;
        }
        let before = self.undo.len();
        if t.column_index(&name).is_none() {
            let (_, add) = self.plan_with_columns(id, &[(name.clone(), None)]);
            if let Some(o) = add {
                self.execute_as(o, "undo.plan_custom");
            }
        }
        let Some(t) = self.table(id) else { return };
        // The column's name as the table spells it.
        let name = t.column_index(&name).map_or(name, |i| t.columns[i].name.clone());
        plan.custom.retain(|c| !c.column.eq_ignore_ascii_case(&name));
        plan.custom.push(CustomField { column: name.clone(), ..field });
        let mut list = self.sheets[id.0].tables.clone();
        list[id.1].plan = Some(Box::new(plan));
        let mut ops = vec![Op::Tables { sheet: id.0, list }];
        if let Some(r) = self.custom_range(id, &name) {
            let mut cf: Vec<CfRule> = self.sheets[id.0].cond_formats.iter().filter(|x| !(x.ranges == [r] && matches!(x.kind, CfKind::IconSet { .. }))).cloned().collect();
            if let Some(l) = light {
                let (low, high) = (l.low.min(l.high), l.low.max(l.high));
                cf.push(CfRule { ranges: vec![r], kind: CfKind::IconSet { set: l.set, reverse: l.reverse, show_value: true, points: vec![Cfvo::Num(low), Cfvo::Num(high)] }, style: CfStyle::default(), stop: false });
            }
            if cf != self.sheets[id.0].cond_formats {
                ops.push(Op::CondFormats { sheet: id.0, list: cf });
            }
        }
        self.execute_as(Op::Batch(ops), "undo.plan_custom");
        let n = self.undo.len() - before;
        self.merge_undo(n, "undo.plan_custom");
    }

    /// Makes custom column `name` a plain column again (its values and
    /// traffic light stay).
    pub fn plan_remove_custom(&mut self, id: TableId, name: &str) {
        let Some(mut plan) = self.table(id).and_then(|t| t.plan.as_deref()).cloned() else { return };
        let n = plan.custom.len();
        plan.custom.retain(|c| !c.column.eq_ignore_ascii_case(name));
        if plan.custom.len() == n {
            return;
        }
        let mut list = self.sheets[id.0].tables.clone();
        list[id.1].plan = Some(Box::new(plan));
        self.execute_as(Op::Tables { sheet: id.0, list }, "undo.plan_custom");
    }

    /// Checks a custom field's formula: None when the sheet can read it,
    /// else the error.
    pub fn plan_formula_error(&self, formula: &str) -> Option<String> {
        let f = CustomField { formula: formula.into(), ..Default::default() };
        if f.formula.trim().is_empty() {
            return None;
        }
        formula::parse(&f.source(), &self.locale).err().map(|e| format!("{e:?}"))
    }

    /// The values of the custom fields of plan `id` written to the cells:
    /// formulas on each task, then the roll-up of the summaries.
    pub(super) fn plan_custom_outputs(&mut self, id: TableId) -> Option<Op> {
        let t = self.table(id)?;
        let custom = t.plan.as_ref()?.custom.clone();
        if custom.is_empty() {
            return None;
        }
        let view = self.plan_view(id)?;
        let t = self.table(id)?;
        let cols: Vec<Option<u32>> = custom.iter().map(|f| t.column_index(&f.column).map(|i| t.col_at(i))).collect();
        let mut cells = Vec::new();
        for (f, col) in custom.iter().zip(cols) {
            let Some(col) = col else { continue };
            let expr = (!f.formula.trim().is_empty()).then(|| formula::parse(&f.source(), &self.locale).ok()).flatten();
            let n = view.rows.len();
            let mut vals: Vec<Option<f64>> = vec![None; n];
            let mut out: Vec<Option<Value>> = vec![None; n];
            // Deepest first: subtasks come after their summary.
            for i in (0..n).rev() {
                let row = view.rows[i];
                let at = CellRef::new(row, col);
                let o = &view.schedule.tasks[i];
                let v = if o.summary && f.rollup != Rollup::None {
                    let kids: Vec<f64> = (i + 1..n).filter(|&k| view.schedule.tasks[k].parent == Some(i)).filter_map(|k| vals[k]).collect();
                    Some(f.rollup.apply(&kids).map_or(Value::Empty, |x| Value::Number((x * 1e9).round() / 1e9)))
                } else if let Some(e) = &expr {
                    Some(match Env::new(self, id.0, at, self.locale).eval(e) {
                        Value::Array(m) => m.data.first().cloned().unwrap_or(Value::Empty),
                        Value::Lambda(_) => Value::Error(crate::value::ErrorKind::Value),
                        v => v,
                    })
                } else {
                    None
                };
                vals[i] = match &v {
                    Some(Value::Number(x)) => Some(*x),
                    Some(_) => None,
                    None => match self.sheets[id.0].value(at) {
                        Value::Number(x) => Some(*x),
                        _ => None,
                    },
                };
                out[i] = v;
            }
            for (i, v) in out.into_iter().enumerate() {
                if let Some(v) = v {
                    cells.extend(self.plan_put(id.0, CellRef::new(view.rows[i], col), v, None, None));
                }
            }
        }
        (!cells.is_empty()).then_some(Op::SetCells { sheet: id.0, cells })
    }
}
