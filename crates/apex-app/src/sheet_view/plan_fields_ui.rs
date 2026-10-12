//! Planning > Custom Field: a column of the plan whose values come from a
//! formula over the task's columns, rolled up into summary tasks, with an
//! optional traffic light (a conditional-format icon set).

use apex_sheet::data::ICON_SETS;
use apex_sheet::plan::{CustomField, Rollup};
use apex_sheet::workbook::{Indicator, TableId};

use super::f10::parse_num;
use super::*;
use crate::widgets::DialogResult;

/// The Custom Field dialog.
pub(super) struct CustomDlg {
    id: TableId,
    /// The custom fields of the plan when the dialog opened.
    existing: Vec<CustomField>,
    /// The field being edited: its index in `existing` (None: a new one).
    pick: Option<usize>,
    field: CustomField,
    light: bool,
    set: usize,
    low: String,
    high: String,
    reverse: bool,
    /// Why OK was refused.
    error: Option<String>,
}

/// The i18n key of a roll-up.
pub(super) fn rollup_key(r: Rollup) -> &'static str {
    match r {
        Rollup::None => "plan.rollup.none",
        Rollup::Sum => "plan.rollup.sum",
        Rollup::Average => "plan.rollup.average",
        Rollup::Max => "plan.rollup.max",
        Rollup::Min => "plan.rollup.min",
    }
}

/// The icon sets of three icons, which take two thresholds.
fn three_icon_sets() -> Vec<&'static str> {
    ICON_SETS.iter().filter(|x| x.1 == 3).map(|x| x.0).collect()
}

impl CustomDlg {
    fn new(wb: &Workbook, id: TableId) -> Self {
        let existing = wb.table(id).and_then(|t| t.plan.as_ref()).map(|p| p.custom.clone()).unwrap_or_default();
        let set = three_icon_sets().iter().position(|s| *s == "3TrafficLights1").unwrap_or(0);
        let mut d = CustomDlg { id, existing, pick: None, field: CustomField::default(), light: false, set, low: "1".into(), high: "2".into(), reverse: false, error: None };
        if !d.existing.is_empty() {
            d.load(wb, Some(0));
        }
        d
    }

    /// Shows field `pick` (None: a new one).
    fn load(&mut self, wb: &Workbook, pick: Option<usize>) {
        self.pick = pick;
        self.error = None;
        self.field = pick.and_then(|i| self.existing.get(i).cloned()).unwrap_or_default();
        let light = pick.and_then(|_| wb.plan_indicator(self.id, &self.field.column));
        self.light = light.is_some();
        if let Some(l) = light {
            self.set = three_icon_sets().iter().position(|s| *s == l.set).unwrap_or(self.set);
            self.low = fmt_num(l.low);
            self.high = fmt_num(l.high);
            self.reverse = l.reverse;
        }
    }

    /// The traffic light as typed, or the key of the error.
    fn indicator(&self, wb: &Workbook) -> Result<Option<Indicator>, &'static str> {
        if !self.light {
            return Ok(None);
        }
        let (Some(low), Some(high)) = (parse_num(wb, &self.low), parse_num(wb, &self.high)) else { return Err("plan.custom_bad_threshold") };
        Ok(Some(Indicator { set: three_icon_sets()[self.set].to_string(), low, high, reverse: self.reverse }))
    }
}

fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 { format!("{x:.0}") } else { format!("{x}") }
}

impl SheetView {
    pub(super) fn open_custom_fields(&mut self, id: TableId) {
        self.settle_edit();
        self.plan.custom = Some(Box::new(CustomDlg::new(&self.wb, id)));
    }

    /// Shows the Custom Field dialog while it is open.
    pub(super) fn custom_fields_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.plan.custom.take() else { return };
        let columns: Vec<String> = self.wb.table(d.id).map(|t| t.columns.iter().map(|c| c.name.clone()).collect()).unwrap_or_default();
        let mut remove = false;
        let wb = &self.wb;
        let res = super::f10::tool_window(ctx, "plan_custom", tr("plan.custom"), 460.0, false, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr("plan.custom_field"));
                let shown = d.pick.map_or_else(|| tr("plan.custom_new").to_string(), |i| d.existing[i].column.clone());
                let mut pick = d.pick;
                egui::ComboBox::from_id_salt("plan_custom_pick").width(200.0).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut pick, None, tr("plan.custom_new"));
                    for (i, f) in d.existing.iter().enumerate() {
                        ui.selectable_value(&mut pick, Some(i), &f.column);
                    }
                });
                if pick != d.pick {
                    d.load(wb, pick);
                }
                if d.pick.is_some() && ui.button(tr("plan.custom_remove")).on_hover_text(tr("plan.custom_remove_tip")).clicked() {
                    remove = true;
                }
            });
            ui.separator();
            egui::Grid::new("plan_custom_grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                ui.label(tr("plan.custom_name"));
                ui.add_enabled(d.pick.is_none(), egui::TextEdit::singleline(&mut d.field.column).desired_width(220.0));
                ui.end_row();
                ui.label(tr("plan.custom_formula"));
                ui.vertical(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.field.formula).desired_width(300.0).font(egui::TextStyle::Monospace).hint_text("[Duration]*8"));
                    crate::widgets::menu_button(ui, tr("plan.custom_insert_column"), |ui| {
                        for c in &columns {
                            if ui.button(c).clicked() {
                                d.field.formula.push_str(&format!("[{c}]"));
                                ui.close();
                            }
                        }
                    });
                });
                ui.end_row();
                ui.label(tr("plan.custom_rollup"));
                egui::ComboBox::from_id_salt("plan_custom_rollup").selected_text(tr(rollup_key(d.field.rollup))).show_ui(ui, |ui| {
                    for r in Rollup::ALL {
                        ui.selectable_value(&mut d.field.rollup, r, tr(rollup_key(r)));
                    }
                });
                ui.end_row();
            });
            ui.label(RichText::new(tr("plan.custom_help")).small().weak());
            ui.separator();
            ui.checkbox(&mut d.light, tr("plan.custom_light"));
            ui.add_enabled_ui(d.light, |ui| {
                egui::Grid::new("plan_custom_light").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                    ui.label(tr("cf.icon_style"));
                    let sets = three_icon_sets();
                    egui::ComboBox::from_id_salt("plan_custom_set").width(180.0).selected_text(sets[d.set]).show_ui(ui, |ui| {
                        for (i, s) in sets.iter().enumerate() {
                            ui.selectable_value(&mut d.set, i, *s);
                        }
                    });
                    ui.end_row();
                    ui.label(tr("plan.custom_low"));
                    ui.add(egui::TextEdit::singleline(&mut d.low).desired_width(80.0));
                    ui.end_row();
                    ui.label(tr("plan.custom_high"));
                    ui.add(egui::TextEdit::singleline(&mut d.high).desired_width(80.0));
                    ui.end_row();
                });
                ui.checkbox(&mut d.reverse, tr("plan.custom_reverse"));
            });
            if let Some(e) = &d.error {
                ui.colored_label(Color32::from_rgb(200, 40, 40), e);
            }
        });
        if remove {
            if let Some(i) = d.pick {
                let name = d.existing[i].column.clone();
                self.wb.plan_remove_custom(d.id, &name);
                d = Box::new(CustomDlg::new(&self.wb, d.id));
            }
            self.plan.custom = Some(d);
            return;
        }
        match res {
            DialogResult::None => self.plan.custom = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                let err = if d.field.column.trim().is_empty() {
                    Some(tr("plan.custom_no_name").to_string())
                } else if d.pick.is_none() && self.wb.table(d.id).is_some_and(|t| t.column_index(d.field.column.trim()).is_some_and(|i| t.columns[i].field.is_some())) {
                    Some(tr("plan.custom_taken").to_string())
                } else if let Some(e) = self.wb.plan_formula_error(&d.field.formula) {
                    Some(format!("{}: {e}", tr("plan.custom_bad_formula")))
                } else {
                    d.indicator(&self.wb).err().map(|k| tr(k).to_string())
                };
                if let Some(e) = err {
                    d.error = Some(e);
                    self.plan.custom = Some(d);
                    return;
                }
                let light = d.indicator(&self.wb).ok().flatten();
                self.wb.plan_set_custom(d.id, d.field.clone(), light);
            }
        }
    }
}
