//! Data > Advanced (filter with a criteria range) and Data > Consolidate.

use apex_sheet::workbook::{AdvancedFilterError, ConsolidateFn};

use super::f10::{self, F10Dlg, abs_range, ref_edit, tool_window};
use super::*;
use crate::widgets::DialogResult;

pub(super) struct AdvDlg {
    copy: bool,
    list: String,
    criteria: String,
    to: String,
    unique: bool,
}

pub(super) struct ConsDlg {
    func: usize,
    reference: String,
    refs: Vec<String>,
    selected: Option<usize>,
    top: bool,
    left: bool,
    /// Where the result goes: the active cell when the dialog opened.
    dest: (usize, CellRef),
}

const FN_KEYS: [&str; 11] = ["cons.sum", "cons.count", "cons.average", "cons.max", "cons.min", "cons.product", "cons.count_nums", "cons.stdev", "cons.stdevp", "cons.var", "cons.varp"];

impl SheetView {
    pub(super) fn open_advanced_filter(&mut self) {
        let region = self.wb.current_region(self.sheet, self.cursor);
        let list = if self.sel().is_single() { region } else { self.sel() };
        let d = AdvDlg { copy: false, list: abs_range(&list), criteria: String::new(), to: String::new(), unique: false };
        self.open_f10(F10Dlg::AdvFilter(d), Some(1));
    }

    pub(super) fn open_consolidate(&mut self) {
        let d = ConsDlg { func: 0, reference: String::new(), refs: Vec::new(), selected: None, top: false, left: false, dest: (self.sheet, self.cursor) };
        self.open_f10(F10Dlg::Consolidate(d), Some(0));
    }

    pub(super) fn adv_filter_dialog(&mut self, ctx: &egui::Context, mut d: AdvDlg) -> Option<AdvDlg> {
        let focus = self.grid_has_focus(ctx);
        let mut pick = std::mem::take(&mut self.f10.pick);
        let res = tool_window(ctx, "adv_filter", tr("adv.title"), 380.0, focus, |ui| {
            ui.label(RichText::new(tr("adv.action")).strong());
            ui.radio_value(&mut d.copy, false, tr("adv.in_place"));
            ui.radio_value(&mut d.copy, true, tr("adv.copy"));
            ui.add_space(6.0);
            egui::Grid::new("adv_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(tr("adv.list"));
                ref_edit(ui, &mut d.list, 0, &mut pick, 200.0);
                ui.end_row();
                ui.label(tr("adv.criteria"));
                ref_edit(ui, &mut d.criteria, 1, &mut pick, 200.0);
                ui.end_row();
                ui.add_enabled_ui(d.copy, |ui| ui.label(tr("adv.to")));
                ui.add_enabled_ui(d.copy, |ui| ref_edit(ui, &mut d.to, 2, &mut pick, 200.0));
                ui.end_row();
            });
            ui.checkbox(&mut d.unique, tr("adv.unique"));
        });
        self.f10.pick = pick;
        match res {
            DialogResult::None => Some(d),
            DialogResult::Cancel => None,
            DialogResult::Ok => match self.run_advanced_filter(&d) {
                Ok(()) => None,
                Err(e) => {
                    self.notices.push(Notice::error(e));
                    Some(d)
                }
            },
        }
    }

    fn run_advanced_filter(&mut self, d: &AdvDlg) -> Result<(), String> {
        let origin = self.f10.pick.origin;
        let bad = || tr("whatif.err_range").to_string();
        let (si, list) = f10::parse_ref(&self.wb, origin, &d.list).ok_or_else(bad)?;
        let (ci, crit) = f10::parse_ref(&self.wb, origin, &d.criteria).ok_or_else(bad)?;
        let to = if d.copy { Some(f10::parse_ref(&self.wb, origin, &d.to).map(|(s, r)| (s, r.start)).ok_or_else(bad)?) } else { None };
        let list = self.wb.clamp_to_used(si, &list).unwrap_or(list);
        let crit = self.wb.clamp_to_used(ci, &crit).unwrap_or(crit);
        match self.wb.advanced_filter(si, &list, ci, &crit, d.unique, to) {
            Ok(n) => {
                if to.is_none() {
                    self.data.advanced = Some((si, list));
                }
                self.notices.push(Notice::info(trf("adv.found", &[("n", &n.to_string())])));
                Ok(())
            }
            Err(AdvancedFilterError::List) => Err(tr("adv.err_list").to_string()),
            Err(AdvancedFilterError::Criteria) => Err(tr("adv.err_criteria").to_string()),
        }
    }

    /// Shows the rows the last in-place advanced filter hid.
    pub(super) fn clear_advanced_filter(&mut self) -> bool {
        let Some((si, list)) = self.data.advanced.take() else { return false };
        if si < self.wb.sheets.len() && list.rows() > 1 {
            let sizes: Vec<(u32, Option<f32>)> = (list.start.row + 1..=list.end.row).filter(|r| self.wb.sheets[si].row_heights.get(r) == Some(&0.0)).map(|r| (r, None)).collect();
            if !sizes.is_empty() {
                self.wb.execute_as(Op::RowHeights { sheet: si, sizes }, "undo.filter");
            }
        }
        true
    }

    pub(super) fn consolidate_dialog(&mut self, ctx: &egui::Context, mut d: ConsDlg) -> Option<ConsDlg> {
        let focus = self.grid_has_focus(ctx);
        let mut pick = std::mem::take(&mut self.f10.pick);
        let res = tool_window(ctx, "consolidate", tr("cons.title"), 420.0, focus, |ui| {
            ui.label(tr("cons.function"));
            egui::ComboBox::from_id_salt("cons_fn").width(200.0).selected_text(tr(FN_KEYS[d.func])).show_ui(ui, |ui| {
                for (i, k) in FN_KEYS.iter().enumerate() {
                    ui.selectable_value(&mut d.func, i, tr(k));
                }
            });
            ui.add_space(6.0);
            ui.label(tr("cons.reference"));
            ui.horizontal(|ui| {
                ref_edit(ui, &mut d.reference, 0, &mut pick, 240.0);
                // A reference, not in the list yet.
                let t = d.reference.trim().to_string();
                if ui.add_enabled(!t.is_empty() && !d.refs.contains(&t), egui::Button::new(tr("cons.add"))).clicked() {
                    d.refs.push(t);
                }
            });
            ui.label(tr("cons.all"));
            ui.horizontal(|ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_size(vec2(240.0, 90.0));
                    egui::ScrollArea::vertical().max_height(110.0).show(ui, |ui| {
                        for (i, r) in d.refs.iter().enumerate() {
                            if ui.selectable_label(d.selected == Some(i), r).clicked() {
                                d.selected = Some(i);
                                d.reference = r.clone();
                            }
                        }
                    });
                });
                ui.vertical(|ui| {
                    if ui.add_enabled(d.selected.is_some(), egui::Button::new(tr("cons.remove"))).clicked() {
                        if let Some(i) = d.selected.take() {
                            d.refs.remove(i);
                        }
                    }
                });
            });
            ui.add_space(6.0);
            ui.label(RichText::new(tr("cons.labels")).strong());
            ui.checkbox(&mut d.top, tr("cons.top"));
            ui.checkbox(&mut d.left, tr("cons.left"));
        });
        self.f10.pick = pick;
        match res {
            DialogResult::None => Some(d),
            DialogResult::Cancel => None,
            DialogResult::Ok => match self.run_consolidate(&d) {
                Ok(()) => None,
                Err(e) => {
                    self.notices.push(Notice::error(e));
                    Some(d)
                }
            },
        }
    }

    fn run_consolidate(&mut self, d: &ConsDlg) -> Result<(), String> {
        let origin = self.f10.pick.origin;
        let mut texts = d.refs.clone();
        if texts.is_empty() && !d.reference.trim().is_empty() {
            texts.push(d.reference.trim().to_string());
        }
        let refs: Vec<(usize, Range)> = texts
            .iter()
            .map(|t| f10::parse_ref(&self.wb, origin, t).map(|(s, r)| (s, self.wb.clamp_to_used(s, &r).unwrap_or(r))))
            .collect::<Option<_>>()
            .filter(|v: &Vec<_>| !v.is_empty())
            .ok_or_else(|| tr("whatif.err_range").to_string())?;
        let (sheet, at) = d.dest;
        let r = self.wb.consolidate(sheet, at, &refs, ConsolidateFn::ALL[d.func], d.top, d.left).ok_or_else(|| tr("cons.err").to_string())?;
        if sheet != self.sheet {
            self.select_sheet(sheet);
        }
        self.anchor = r.end;
        self.cursor = r.start;
        Ok(())
    }
}
