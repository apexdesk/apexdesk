//! The data model on a workbook: its tables read as model data, the
//! source of pivot tables built on the model, and undoable edits of the
//! relationships and measures.

use super::*;
use crate::analysis::pivot::{self, PivotSpec, Source};
use crate::model::data::TableData;
use crate::model::eval::{self, Context, Engine, PivotMeasures};
use crate::model::{DataModel, ModelData};

impl Workbook {
    /// Every table of the workbook (all of them are in the model), data
    /// rows only.
    pub fn model_tables(&self) -> Vec<TableData> {
        self.tables()
            .map(|(si, t)| {
                let sh = &self.sheets[si];
                let r = t.data_range();
                let rows = r.rows() as usize;
                let mut cols = vec![vec![Value::Empty; rows]; t.columns.len()];
                let mut formats: Vec<Option<Arc<str>>> = vec![None; t.columns.len()];
                let mut probed = vec![false; t.columns.len()];
                if rows > 0 {
                    sh.for_each_in(&r, |at, cell| {
                        let (ri, ci) = ((at.row - r.start.row) as usize, (at.col - r.start.col) as usize);
                        if let Some(col) = cols.get_mut(ci) {
                            col[ri] = cell.value().clone();
                            if !probed[ci] && matches!(cell.value(), Value::Number(_)) {
                                probed[ci] = true;
                                if self.styles.format(cell.style).is_date() {
                                    formats[ci] = Some(self.styles.get(cell.style).num_fmt.clone());
                                }
                            }
                        }
                    });
                }
                TableData { name: t.name.clone(), columns: t.columns.iter().map(|c| c.name.clone()).collect(), cols, rows, formats }
            })
            .collect()
    }

    /// The model's tables with its relationships resolved.
    pub fn model_data(&self) -> ModelData {
        ModelData::new(self.model_tables(), &self.analysis.model.relationships)
    }

    /// Replaces the relationships and measures, refreshing the pivot tables
    /// built on the model (one undo step).
    pub fn set_data_model(&mut self, model: DataModel) {
        if self.analysis.model == model {
            return;
        }
        let before = self.undo.len();
        let mut st = self.analysis.clone();
        st.model = model;
        self.execute_as(Op::Analysis(Box::new(st)), "undo.data_model");
        for i in 0..self.analysis.pivots.len() {
            if self.analysis.pivots[i].spec.model.is_some() {
                let _ = self.refresh_pivot(i);
            }
        }
        let steps = self.undo.len().saturating_sub(before);
        self.merge_undo(steps, "undo.data_model");
    }

    /// The value of a measure (or of a formula, when `formula` is given)
    /// with no filters: the Measures dialog's check.
    pub fn measure_total(&self, name: &str, formula: Option<&str>) -> Result<Value, crate::model::DaxError> {
        let data = self.model_data();
        let mut measures = self.analysis.model.measures.clone();
        if let Some(f) = formula {
            eval::check_measure(name, f, &data, &measures)?;
            match measures.iter_mut().find(|m| m.name.eq_ignore_ascii_case(name)) {
                Some(m) => m.formula = f.to_string(),
                None => measures.push(crate::model::Measure { name: name.to_string(), formula: f.to_string(), ..Default::default() }),
            }
        }
        let compiled = eval::compile_all(&data, &measures);
        let engine = Engine::new(&data, &compiled);
        if let Some(e) = engine.error(name) {
            return Err(e.clone());
        }
        Ok(engine.measure(name, &Context::default()))
    }

    /// The source of a pivot table on the data model: the rows of the
    /// table every used table relates to (the "many"-most one), with the
    /// columns of related tables looked up.
    pub(super) fn model_pivot_source(&self, spec: &PivotSpec) -> Result<Source, AnalysisError> {
        let ms = spec.model.as_ref().ok_or(AnalysisError::NotFound)?;
        let data = Arc::new(self.model_data());
        if data.tables.is_empty() {
            return Err(AnalysisError::NoData);
        }
        let cols: Vec<Option<(usize, usize)>> = ms.cols.iter().map(|(t, c)| data.table(t).and_then(|ti| data.column(ti, c).map(|ci| (ti, ci)))).collect();
        let names: Vec<String> = ms.cols.iter().enumerate().map(|(k, (_, c))| cols[k].map_or_else(|| c.clone(), |(t, ci)| data.tables[t].columns[ci].clone())).collect();
        let used = spec.used_columns(&names);
        let mut tables: Vec<usize> = Vec::new();
        for &k in &used {
            let (t, _) = cols.get(k as usize).copied().flatten().ok_or(AnalysisError::BadRange)?;
            if !tables.contains(&t) {
                tables.push(t);
            }
        }
        let base = base_table(&data, &tables).ok_or(AnalysisError::NoRelationship)?;
        let records = data.rows(base);
        let mut out_cols: Vec<Vec<Value>> = vec![Vec::new(); names.len()];
        let mut formats: Vec<Option<Arc<str>>> = vec![None; names.len()];
        for &k in &used {
            let Some((t, c)) = cols[k as usize] else { continue };
            out_cols[k as usize] = if t == base { data.tables[t].cols[c].clone() } else { data.related_column(base, t, c).unwrap_or_default() };
            formats[k as usize] = data.tables[t].formats.get(c).cloned().flatten();
        }
        let measures = spec.values.iter().any(|v| v.col >= pivot::MEASURE_BASE).then(|| {
            let m: Arc<dyn pivot::MeasureEval> = Arc::new(PivotMeasures::new(data.clone(), &self.analysis.model.measures, ms.measures.clone(), cols.clone(), spec.lang));
            m
        });
        Ok(Source { names, cols: out_cols, formats, records, measures })
    }

    /// A new sheet holding the records behind a value of a pivot table on
    /// the model: the used columns of the rows `recs` of its source.
    pub(super) fn model_details_sheet(&mut self, src: &Source, recs: &[usize], name: String) -> Sheet {
        let mut sh = Sheet::new(name);
        let shown: Vec<usize> = (0..src.names.len()).filter(|k| !src.cols[*k].is_empty()).collect();
        for (x, &k) in shown.iter().enumerate() {
            sh.put(CellRef::new(0, x as u32), Some(Cell::new(Content::Value(Value::text(src.names[k].as_str())), 0)));
            let style = src.formats.get(k).cloned().flatten().map(|f| {
                let mut s = self.styles.get(0).clone();
                s.num_fmt = f;
                self.styles.intern(s)
            });
            for (y, r) in recs.iter().enumerate() {
                let v = src.cols[k].get(*r).cloned().unwrap_or(Value::Empty);
                if !v.is_empty() {
                    sh.put(CellRef::new(y as u32 + 1, x as u32), Some(Cell::new(Content::Value(v), style.unwrap_or(0))));
                }
            }
        }
        sh
    }

    /// The analysis state with table `old` renamed in the model and in the
    /// pivot tables on it, when anything refers to it.
    pub(super) fn model_rename_op(&self, old: &str, new: &str) -> Option<Op> {
        let mut st = self.analysis.clone();
        st.model.rename_table(old, new);
        for p in &mut st.pivots {
            if let Some(m) = &mut p.spec.model {
                for (t, _) in &mut m.cols {
                    if t.eq_ignore_ascii_case(old) {
                        *t = new.to_string();
                    }
                }
            }
        }
        (st != self.analysis).then(|| Op::Analysis(Box::new(st)))
    }
}

/// The table whose rows a model pivot table groups: one every used table
/// is reachable from; the most "one"-sided of them (fewest reachable
/// tables), so a table of dimensions lists all its items.
pub fn base_table(data: &ModelData, used: &[usize]) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for t in 0..data.tables.len() {
        let reach = data.reachable(t);
        if used.iter().all(|u| reach.contains(u)) && best.is_none_or(|(_, n)| reach.len() < n) {
            best = Some((t, reach.len()));
        }
    }
    best.map(|b| b.0)
}
