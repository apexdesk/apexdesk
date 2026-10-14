//! What running a macro needs from the spreadsheet view: the active sheet
//! and selection before it runs, and afterwards the workbook it changed
//! with the sheet and cells it left selected.

use apex_sheet::{Range, Workbook};
use eframe::egui;

use super::SheetView;

/// Double and right clicks on cells that the sheet's macros handle
/// (Worksheet_BeforeDoubleClick, Worksheet_BeforeRightClick): the click
/// waits for them, which may cancel the editing or the menu.
#[derive(Default)]
pub struct CellHooks {
    /// The sheet's module handles them (the app says so every frame).
    pub double: bool,
    pub right: bool,
    /// A click for the macros: (right click, sheet, Target).
    pub clicked: Option<(bool, usize, Range)>,
    /// A link followed from a cell (FollowHyperlink): sheet and cell.
    pub followed: Option<(usize, apex_sheet::CellRef)>,
    /// What they said: (right click, Cancel).
    answer: Option<(bool, bool)>,
    /// The Assign Macro dialog.
    assign: Option<AssignDlg>,
}

impl SheetView {
    /// The macros handled a click: unless cancelled, a double click edits
    /// the cell; a cancelled right click closes the menu.
    pub fn cell_click_done(&mut self, right: bool, cancel: bool) {
        self.cell_hooks.answer = Some((right, cancel));
    }

    pub(super) fn cell_hooks_frame(&mut self, ctx: &egui::Context) {
        match self.cell_hooks.answer.take() {
            Some((false, false)) => self.begin_edit(ctx, super::EditOrigin::Cell, false, None),
            Some((true, true)) => egui::Popup::close_all(ctx),
            _ => {}
        }
    }

    /// Commits the cell being edited; returns the active sheet and the
    /// selection (what ActiveSheet and Selection start as).
    pub fn macro_context(&mut self) -> (usize, Range) {
        self.settle_edit();
        (self.sheet, self.sel())
    }

    /// A cell is being typed in.
    pub fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// The active sheet and the selection, as they are (the cell being
    /// edited stays so).
    pub fn macro_place(&self) -> (usize, Range) {
        (self.sheet, self.sel())
    }

    /// Takes the workbook a macro changed (`None`: it changed nothing) and
    /// shows the sheet and selection it left.
    pub fn macro_done(&mut self, wb: Option<Workbook>, sheet: usize, sel: Range) {
        self.cancel_edit();
        if let Some(wb) = wb {
            self.wb = wb;
        }
        let sheet = sheet.min(self.wb.sheets.len().saturating_sub(1));
        if sheet != self.sheet || self.sheet >= self.wb.sheets.len() {
            self.select_sheet(sheet);
        }
        // The active cell is the top-left one, like Range.Select.
        self.anchor = sel.end;
        self.cursor = sel.start;
        self.extra.clear();
        self.buf_key = None;
        self.ensure_visible = true;
    }
}

/// What a click runs a macro of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Clickable {
    Shape(u32),
    Image(u32),
}

/// Assign Macro: the shape or picture, the name typed and the macros of
/// the workbook.
pub struct AssignDlg {
    sheet: usize,
    target: Clickable,
    name: String,
    macros: Vec<String>,
}

impl SheetView {
    /// A click on something with macro `name` (`[0]!Macro1` from files,
    /// `Module1.Macro1`): the app runs it.
    pub(super) fn click_macro(&mut self, name: &str) {
        let name = name.rsplit('!').next().unwrap_or_default().trim();
        if !name.is_empty() {
            self.run_macro = Some(name.to_string());
        }
    }

    /// The macros of the workbook (none when its code doesn't parse).
    fn macro_list(&self) -> Vec<String> {
        let Some(bin) = &self.wb.macros.project else { return Vec::new() };
        let Ok(p) = apex_io::vba::read(bin) else { return Vec::new() };
        let sources: Vec<apex_vba::Source> = p
            .modules
            .into_iter()
            .map(|m| {
                let kind = match m.kind {
                    apex_io::vba::ModuleKind::Standard => apex_vba::ModuleKind::Standard,
                    apex_io::vba::ModuleKind::Class => apex_vba::ModuleKind::Class,
                    apex_io::vba::ModuleKind::Document => apex_vba::ModuleKind::Document,
                    apex_io::vba::ModuleKind::Form => apex_vba::ModuleKind::Form,
                };
                apex_vba::Source { name: m.name, kind, code: m.code }
            })
            .collect();
        apex_vba::Project::parse(&sources).map(|p| p.macros()).unwrap_or_default()
    }

    pub(super) fn open_assign_macro(&mut self, target: Clickable) {
        let sh = self.sh();
        let name = match target {
            Clickable::Shape(id) => sh.shapes.iter().find(|s| s.id == id).map(|s| s.macro_name.clone()),
            Clickable::Image(id) => sh.image(id).map(|i| i.macro_name.clone()),
        };
        let Some(name) = name else { return };
        let macros = self.macro_list();
        self.cell_hooks.assign = Some(AssignDlg { sheet: self.sheet, target, name, macros });
    }

    /// Assign Macro: a macro of the workbook, or a name typed (empty:
    /// none).
    pub(super) fn assign_macro_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.cell_hooks.assign else { return };
        let (mut ok, mut cancel) = (false, false);
        let m = egui::Modal::new(egui::Id::new("assign_macro")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading(crate::i18n::tr("macros.assign"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(crate::i18n::tr("control.macro"));
                ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY));
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                    if d.macros.is_empty() {
                        ui.label(egui::RichText::new(crate::i18n::tr("macros.no_macros")).weak());
                    }
                    for n in &d.macros {
                        let r = ui.selectable_label(d.name.eq_ignore_ascii_case(n), n);
                        if r.clicked() {
                            d.name = n.clone();
                        }
                        if r.double_clicked() {
                            ok = true;
                        }
                    }
                });
            });
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                cancel = ui.button(crate::i18n::tr("dialog.cancel")).clicked();
                ok |= ui.button(crate::i18n::tr("dialog.ok")).clicked();
            });
        });
        if m.is_top_modal {
            ok |= ctx.input(|i| i.key_pressed(egui::Key::Enter));
            cancel |= ctx.input(|i| i.key_pressed(egui::Key::Escape));
        }
        if !(ok || cancel) {
            return;
        }
        let Some(d) = self.cell_hooks.assign.take() else { return };
        let name = d.name.trim().to_string();
        // A name, or nothing: not a formula or a path.
        if !ok || !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
            return;
        }
        let Some(sh) = self.wb.sheets.get(d.sheet) else { return };
        match d.target {
            Clickable::Shape(id) => {
                if let Some(mut s) = sh.shapes.iter().find(|s| s.id == id).cloned() {
                    s.macro_name = name;
                    self.wb.set_shape(d.sheet, s);
                }
            }
            Clickable::Image(id) => {
                if let Some(mut i) = sh.image(id).cloned() {
                    i.macro_name = name;
                    self.wb.set_image(d.sheet, i);
                }
            }
        }
    }
}
