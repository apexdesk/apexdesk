//! Templates: a few built in (in the interface language) and the user's
//! own, saved as regular files in `templates/` in the config directory.
//! New from Template opens a copy as an untitled tab; Save as Template
//! writes the active tab there. An organization's shared folder (a
//! setting or policy) is listed too.

use crate::widgets::DialogResult;
use std::path::{Path, PathBuf};

use apex_doc::Document;
use apex_io::{Format, Kind, Loaded};
use apex_sheet::{CellRef, Range, Workbook};
use eframe::egui;

use crate::i18n::{tr, trf};
use crate::icons::Icon;

#[derive(Clone)]
enum Source {
    Builtin(fn() -> Loaded),
    File(PathBuf),
    /// In the organization's shared folder (not deleted from here).
    Shared(PathBuf),
}

impl Source {
    /// 0 built in, 1 shared, 2 the user's.
    fn group(&self) -> u8 {
        match self {
            Source::Builtin(_) => 0,
            Source::Shared(_) => 1,
            Source::File(_) => 2,
        }
    }
}

#[derive(Clone)]
struct Template {
    name: String,
    kind: Kind,
    source: Source,
    /// A shared template's subfolder ("": none).
    category: String,
}

fn dir() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("templates"))
}

/// Template file extensions: the regular ones and the template variants.
const EXTS: &[&str] = &["docx", "dotx", "odt", "ott", "md", "xlsx", "xltx", "ods", "ots", "pptx", "potx", "odp", "otp"];

static SHARED: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// The shared templates folder (None: none).
pub fn set_shared_dir(d: Option<PathBuf>) {
    *SHARED.lock().unwrap_or_else(|e| e.into_inner()) = d;
}

fn shared_dir() -> Option<PathBuf> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

static ONLY_SHARED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether only the shared templates are offered (a setting or policy).
pub fn set_only_shared(on: bool) {
    ONLY_SHARED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Save as Template is offered (not with only the shared templates).
pub fn can_save() -> bool {
    !ONLY_SHARED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The gallery's list: built in, the user's and the shared ones, or only
/// the shared ones.
fn all_templates(user: Option<&Path>, shared: Option<&Path>, only_shared: bool) -> Vec<Template> {
    let mut v = if only_shared { Vec::new() } else { builtin() };
    if !only_shared {
        v.extend(user.map(|d| templates_in(d, false)).unwrap_or_default());
    }
    v.extend(shared.map(shared_templates).unwrap_or_default());
    v
}

/// The shared folder's templates, then its subfolders' (one level deep,
/// the subfolder their category).
fn shared_templates(d: &Path) -> Vec<Template> {
    let mut v = templates_in(d, true);
    let mut subs: Vec<PathBuf> = std::fs::read_dir(d).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect()).unwrap_or_default();
    subs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    for s in subs {
        let Some(cat) = s.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')) else { continue };
        v.extend(templates_in(&s, true).into_iter().map(|t| Template { category: cat.clone(), ..t }));
    }
    v
}

/// The shared folder's Normal template for new documents (Normal.dotx…)
/// or workbooks (Book.xltx…), any case.
fn normal_in(d: &Path, kind: Kind) -> Option<PathBuf> {
    let (stem, exts): (&str, &[&str]) = match kind {
        Kind::Doc => ("normal", &["dotx", "docx", "ott", "odt"]),
        Kind::Sheet => ("book", &["xltx", "xlsx", "ots", "ods"]),
        Kind::Slides => return None,
    };
    let files: Vec<PathBuf> = std::fs::read_dir(d).ok()?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect();
    exts.iter().find_map(|x| {
        files
            .iter()
            .find(|p| p.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(stem)) && p.extension().is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(x)))
            .cloned()
    })
}

/// A new blank document or workbook from the shared Normal template, as
/// an untitled copy (None: there is none, or it doesn't open).
pub fn normal(kind: Kind) -> Option<Loaded> {
    let p = normal_in(&shared_dir()?, kind)?;
    load(&Template { name: String::new(), kind, source: Source::Shared(p), category: String::new() }).ok()
}

/// The presentations in the shared folder, for Design > Company
/// Template: their names ("Category / Name" in a subfolder) and files.
pub fn shared_presentations() -> Vec<(String, PathBuf)> {
    let Some(d) = shared_dir() else { return Vec::new() };
    shared_templates(&d)
        .into_iter()
        .filter_map(|t| match (t.kind, t.source) {
            (Kind::Slides, Source::Shared(p)) => Some((if t.category.is_empty() { t.name } else { format!("{} / {}", t.category, t.name) }, p)),
            _ => None,
        })
        .collect()
}

fn templates_in(d: &Path, shared: bool) -> Vec<Template> {
    let Ok(rd) = std::fs::read_dir(d) else { return Vec::new() };
    let mut v: Vec<Template> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| {
            let ext = p.extension()?.to_str()?.to_ascii_lowercase();
            if !EXTS.contains(&ext.as_str()) {
                return None;
            }
            let kind = match ext.as_str() {
                "xlsx" | "xltx" | "ods" | "ots" => Kind::Sheet,
                "pptx" | "potx" | "odp" | "otp" => Kind::Slides,
                _ => Kind::Doc,
            };
            let source = if shared { Source::Shared(p.clone()) } else { Source::File(p.clone()) };
            Some(Template { name: p.file_stem()?.to_string_lossy().into_owned(), kind, source, category: String::new() })
        })
        .collect();
    v.sort_by_key(|t| t.name.to_lowercase());
    v
}

fn builtin() -> Vec<Template> {
    let b = |key: &'static str, kind, f: fn() -> Loaded| Template { name: tr(key).to_string(), kind, source: Source::Builtin(f), category: String::new() };
    vec![
        b("tpl.blank_doc", Kind::Doc, || Loaded::Doc(Box::new(Document::new()))),
        b("tpl.letter", Kind::Doc, letter),
        b("tpl.report", Kind::Doc, report),
        b("tpl.minutes", Kind::Doc, minutes),
        b("tpl.blank_sheet", Kind::Sheet, blank_sheet),
        b("tpl.budget", Kind::Sheet, budget),
        b("tpl.invoice", Kind::Sheet, invoice),
        b("tpl.blank_slides", Kind::Slides, || Loaded::Slides(Box::new(apex_slides::build::new_deck(0)))),
    ]
}

fn doc_from_md(md: &str) -> Loaded {
    let mut d = apex_io::markdown::read(md);
    d.modified = false;
    Loaded::Doc(Box::new(d))
}

fn letter() -> Loaded {
    doc_from_md(&format!(
        "{}  \n{}  \n{}\n\n{}\n\n{}  \n{}  \n{}\n\n**{}**\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}\n",
        tr("tpl.your_name"),
        tr("tpl.street"),
        tr("tpl.city"),
        tr("tpl.date"),
        tr("tpl.recipient"),
        tr("tpl.company"),
        tr("tpl.city"),
        tr("tpl.subject"),
        tr("tpl.dear"),
        tr("tpl.letter_body"),
        tr("tpl.letter_body2"),
        tr("tpl.regards"),
        tr("tpl.your_name"),
    ))
}

fn report() -> Loaded {
    doc_from_md(&format!(
        "# {}\n\n*{}*\n\n## {}\n\n{}\n\n## {}\n\n{}\n\n- {}\n- {}\n- {}\n\n## {}\n\n| {} | {} | {} |\n|---|--:|--:|\n| A | 0 | 0 |\n| B | 0 | 0 |\n\n## {}\n\n{}\n",
        tr("tpl.report_title"),
        tr("tpl.report_subtitle"),
        tr("tpl.summary"),
        tr("tpl.summary_text"),
        tr("tpl.introduction"),
        tr("tpl.intro_text"),
        tr("tpl.point"),
        tr("tpl.point"),
        tr("tpl.point"),
        tr("tpl.results"),
        tr("tpl.item"),
        tr("tpl.value"),
        tr("tpl.change"),
        tr("tpl.conclusions"),
        tr("tpl.conclusions_text"),
    ))
}

fn minutes() -> Loaded {
    doc_from_md(&format!(
        "# {}\n\n**{}:** {}  \n**{}:** {}  \n**{}:** {}\n\n## {}\n\n1. {}\n2. {}\n3. {}\n\n## {}\n\n{}\n\n## {}\n\n| {} | {} | {} |\n|---|---|---|\n| | | |\n| | | |\n",
        tr("tpl.minutes_title"),
        tr("tpl.date_label"),
        tr("tpl.date"),
        tr("tpl.attendees"),
        tr("tpl.names"),
        tr("tpl.place"),
        tr("tpl.place_value"),
        tr("tpl.agenda"),
        tr("tpl.topic"),
        tr("tpl.topic"),
        tr("tpl.topic"),
        tr("tpl.discussion"),
        tr("tpl.discussion_text"),
        tr("tpl.actions"),
        tr("tpl.action"),
        tr("tpl.owner"),
        tr("tpl.due"),
    ))
}

fn new_workbook() -> Workbook {
    let mut wb = Workbook::empty();
    wb.sheets.push(apex_sheet::Sheet::new(trf("sheet.default_name", &[("n", &1)])));
    wb
}

fn blank_sheet() -> Loaded {
    let mut wb = new_workbook();
    wb.finish_load();
    Loaded::Sheet(Box::new(wb))
}

fn put(wb: &mut Workbook, cell: &str, text: &str) {
    if let Some(c) = CellRef::parse(cell) {
        wb.set_input(0, c, text);
    }
}

fn style(wb: &mut Workbook, range: &str, f: impl Fn(&mut apex_sheet::CellStyle)) {
    if let Some(r) = Range::parse(range).or_else(|| CellRef::parse(range).map(Range::single)) {
        wb.modify_style(0, &r, f);
    }
}

const HEADER_FILL: u32 = 0x1F4E79;
const BAND_FILL: u32 = 0xDDEBF7;

fn header(wb: &mut Workbook, range: &str) {
    style(wb, range, |s| {
        s.bold = true;
        s.color = Some(0xFFFFFF);
        s.fill = Some(HEADER_FILL);
    });
}

fn money(wb: &mut Workbook, range: &str) {
    style(wb, range, |s| s.num_fmt = "#,##0.00".into());
}

fn budget() -> Loaded {
    let mut wb = new_workbook();
    put(&mut wb, "A1", tr("tpl.budget_title"));
    style(&mut wb, "A1", |s| {
        s.bold = true;
        s.font_size = 160;
    });
    for (c, key) in ["A3", "B3", "C3", "D3"].iter().zip(["tpl.category", "tpl.planned", "tpl.actual", "tpl.difference"]) {
        put(&mut wb, c, tr(key));
    }
    header(&mut wb, "A3:D3");
    let cats = ["tpl.cat_housing", "tpl.cat_food", "tpl.cat_transport", "tpl.cat_utilities", "tpl.cat_leisure", "tpl.cat_savings", "tpl.cat_other"];
    for (i, key) in cats.iter().enumerate() {
        let r = i + 4;
        put(&mut wb, &format!("A{r}"), tr(key));
        put(&mut wb, &format!("B{r}"), "0");
        put(&mut wb, &format!("C{r}"), "0");
        put(&mut wb, &format!("D{r}"), &format!("=B{r}-C{r}"));
    }
    let last = cats.len() + 3;
    let total = last + 1;
    put(&mut wb, &format!("A{total}"), tr("tpl.total"));
    for col in ["B", "C", "D"] {
        put(&mut wb, &format!("{col}{total}"), &format!("=SUM({col}4:{col}{last})"));
    }
    style(&mut wb, &format!("A{total}:D{total}"), |s| {
        s.bold = true;
        s.fill = Some(BAND_FILL);
    });
    money(&mut wb, &format!("B4:D{total}"));
    let sh = &mut wb.sheets[0];
    let w = sh.default_col_width;
    sh.row_heights.insert(0, sh.default_row_height * 1.5);
    sh.col_widths.insert(0, w * 2.2);
    for c in 1..4 {
        sh.col_widths.insert(c, w * 1.4);
    }
    wb.finish_load();
    Loaded::Sheet(Box::new(wb))
}

fn invoice() -> Loaded {
    let mut wb = new_workbook();
    put(&mut wb, "A1", tr("tpl.invoice_title"));
    style(&mut wb, "A1", |s| {
        s.bold = true;
        s.font_size = 200;
        s.color = Some(HEADER_FILL);
    });
    put(&mut wb, "A3", tr("tpl.company"));
    put(&mut wb, "A4", tr("tpl.street"));
    put(&mut wb, "A5", tr("tpl.city"));
    put(&mut wb, "D3", tr("tpl.invoice_no"));
    put(&mut wb, "E3", "'0001");
    style(&mut wb, "E3", |s| s.halign = apex_sheet::HAlign::Right);
    put(&mut wb, "D4", tr("tpl.date_label"));
    put(&mut wb, "E4", "=TODAY()");
    style(&mut wb, "E4", |s| s.num_fmt = "dd/mm/yyyy".into());
    put(&mut wb, "A7", tr("tpl.bill_to"));
    style(&mut wb, "A7", |s| s.bold = true);
    put(&mut wb, "A8", tr("tpl.recipient"));
    for (c, key) in ["A10", "B10", "C10", "D10", "E10"].iter().zip(["tpl.description", "tpl.quantity", "tpl.unit_price", "tpl.tax_rate", "tpl.amount"]) {
        put(&mut wb, c, tr(key));
    }
    header(&mut wb, "A10:E10");
    for r in 11..=18 {
        put(&mut wb, &format!("E{r}"), &format!("=IF(B{r}=\"\",\"\",B{r}*C{r})"));
    }
    put(&mut wb, "A11", tr("tpl.item"));
    put(&mut wb, "B11", "1");
    put(&mut wb, "C11", "100");
    put(&mut wb, "D11", "21%");
    put(&mut wb, "D20", tr("tpl.subtotal"));
    put(&mut wb, "E20", "=SUM(E11:E18)");
    put(&mut wb, "D21", tr("tpl.tax"));
    put(&mut wb, "E21", "=SUMPRODUCT(B11:B18,C11:C18,D11:D18)");
    put(&mut wb, "D22", tr("tpl.total"));
    put(&mut wb, "E22", "=E20+E21");
    style(&mut wb, "D22:E22", |s| {
        s.bold = true;
        s.fill = Some(BAND_FILL);
    });
    money(&mut wb, "C11:C18");
    money(&mut wb, "E11:E22");
    style(&mut wb, "D11:D18", |s| s.num_fmt = "0%".into());
    let sh = &mut wb.sheets[0];
    let w = sh.default_col_width;
    sh.row_heights.insert(0, sh.default_row_height * 1.8);
    sh.col_widths.insert(0, w * 3.0);
    for c in 1..5 {
        sh.col_widths.insert(c, w * 1.3);
    }
    wb.finish_load();
    Loaded::Sheet(Box::new(wb))
}

fn load(t: &Template) -> Result<Loaded, String> {
    match &t.source {
        Source::Builtin(f) => Ok(f()),
        Source::File(p) | Source::Shared(p) => {
            let (mut l, _) = apex_io::open(p).map_err(|e| e.to_string())?;
            match &mut l {
                Loaded::Doc(d) => d.modified = false,
                Loaded::Sheet(w) => w.modified = false,
                Loaded::Slides(_) => {}
            }
            Ok(l)
        }
    }
}

// --- Dialogs ----------------------------------------------------------------------------------

/// New from Template.
pub struct Gallery {
    list: Vec<Template>,
    selected: usize,
    filter: Option<Kind>,
}

impl Gallery {
    pub fn new(filter: Option<Kind>) -> Gallery {
        let only = !can_save();
        let list = all_templates(dir().as_deref(), shared_dir().as_deref(), only);
        Gallery { list, selected: 0, filter }
    }

    /// Returns a loaded template to open, when chosen; `Err` closes.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Result<Loaded, String>> {
        let mut result = None;
        let mut remove = None;
        let pages = [(Icon::Template, "tpl.all"), (Icon::Doc, "tpl.documents"), (Icon::Sheet, "tpl.spreadsheets"), (Icon::Slides, "tpl.presentations")];
        let mut page = match self.filter {
            None => 0,
            Some(Kind::Doc) => 1,
            Some(Kind::Sheet) => 2,
            Some(Kind::Slides) => 3,
        };
        let filter = self.filter;
        let shown: Vec<usize> = (0..self.list.len()).filter(|&i| filter.is_none_or(|k| self.list[i].kind == k)).collect();
        if !shown.contains(&self.selected) {
            self.selected = shown.first().copied().unwrap_or(0);
        }
        let res = crate::widgets::nav_dialog(ctx, "templates", tr("tpl.new_title"), &pages, &mut page, egui::vec2(560.0, 380.0), |ui, _| {
            // Shared templates by category (their subfolder).
            let mut sections: Vec<(String, u8, String)> = vec![(tr("tpl.builtin").to_string(), 0, String::new()), (tr("tpl.shared").to_string(), 1, String::new())];
            for &i in &shown {
                let c = &self.list[i].category;
                if !c.is_empty() && !sections.iter().any(|s| s.1 == 1 && s.2 == *c) {
                    sections.push((format!("{} · {c}", tr("tpl.shared")), 1, c.clone()));
                }
            }
            sections.push((tr("tpl.mine").to_string(), 2, String::new()));
            for (title, g, cat) in sections {
                let user = g == 2;
                let group: Vec<usize> = shown.iter().copied().filter(|&i| self.list[i].source.group() == g && self.list[i].category == cat).collect();

                if group.is_empty() {
                    continue;
                }
                crate::widgets::section(ui, &title);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                    for i in group {
                        let t = &self.list[i];
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(120.0, 118.0), egui::Sense::click());
                        crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &t.name, Some(i == self.selected));
                        let sel = i == self.selected;
                        let v = ui.visuals();
                        let bg = if sel { v.selection.bg_fill } else if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { v.faint_bg_color };
                        ui.painter().rect_filled(rect, 6.0, bg);
                        // A page thumbnail with the program's icon.
                        let page = egui::Rect::from_center_size(rect.center_top() + egui::vec2(0.0, 44.0), egui::vec2(52.0, 66.0));
                        ui.painter().rect_filled(page, 2.0, egui::Color32::WHITE);
                        ui.painter().rect_stroke(page, 2.0, egui::Stroke::new(1.0, egui::Color32::from_gray(180)), egui::StrokeKind::Inside);
                        let (icon, accent) = match t.kind {
                            Kind::Sheet => (Icon::Sheet, crate::theme::SHEET_ACCENT),
                            Kind::Doc => (Icon::Doc, crate::theme::DOC_ACCENT),
                            Kind::Slides => (Icon::Slides, crate::theme::SLIDES_ACCENT),
                        };
                        crate::icons::paint(ui.painter(), egui::Rect::from_center_size(page.center(), egui::vec2(24.0, 24.0)), icon, accent);
                        let color = if sel { v.selection.stroke.color } else { v.text_color() };
                        let label = ui.painter().layout(t.name.clone(), egui::FontId::proportional(12.5), color, rect.width() - 8.0);
                        ui.painter().galley(egui::pos2(rect.center().x - label.size().x / 2.0, page.max.y + 6.0), label, color);
                        if user {
                            resp.context_menu(|ui| {
                                if ui.button(tr("tpl.delete")).clicked() {
                                    remove = Some(i);
                                    ui.close();
                                }
                            });
                        }
                        if resp.clicked() {
                            self.selected = i;
                        }
                        if resp.double_clicked() {
                            self.selected = i;
                            result = Some(load(&self.list[i]));
                        }
                    }
                });
            }
            if let Some(d) = dir().filter(|_| can_save()) {
                ui.add_space(8.0);
                ui.add(egui::Label::new(egui::RichText::new(trf("tpl.folder", &[("dir", &d.display())])).small().weak()).truncate());
            }
        });
        self.filter = [None, Some(Kind::Doc), Some(Kind::Sheet), Some(Kind::Slides)][page.min(3)];
        match res {
            DialogResult::Ok if !shown.is_empty() => result = self.list.get(self.selected).map(load),
            DialogResult::Ok | DialogResult::Cancel => result = Some(Err(String::new())),
            DialogResult::None => {}
        }
        if let Some(i) = remove
            && let Source::File(p) = &self.list[i].source
        {
            let _ = std::fs::remove_file(p);
            self.list.remove(i);
        }
        result
    }
}

/// Save as Template: the name to save under.
pub struct SaveDialog {
    pub name: String,
    kind: Kind,
    focus: bool,
}

impl SaveDialog {
    pub fn new(kind: Kind, name: String) -> SaveDialog {
        SaveDialog { name, kind, focus: true }
    }

    /// `Some(Some(path))` to save there, `Some(None)` to cancel.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Option<PathBuf>> {
        let name = self.name.trim().replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        let ext = match self.kind {
            Kind::Sheet => "xlsx",
            Kind::Doc => "docx",
            Kind::Slides => "pptx",
        };
        let target = dir().map(|d| d.join(format!("{name}.{ext}")));
        let res = crate::widgets::dialog(ctx, "save_template", tr("tpl.save_title"), 420.0, |ui| {
            crate::widgets::section(ui, tr("tpl.template"));
            crate::widgets::form(ui, "save_template_form", |ui| {
                crate::widgets::form_row(ui, tr("tpl.name_label"), |ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(220.0));
                    if std::mem::take(&mut self.focus) {
                        r.request_focus();
                    }
                });
                if let Some(d) = dir() {
                    crate::widgets::form_row(ui, tr("tpl.location"), |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(d.display().to_string()).weak()).truncate());
                    });
                }
            });
            if target.as_ref().is_some_and(|t| t.exists()) && !name.is_empty() {
                ui.add_space(4.0);
                ui.colored_label(ui.visuals().warn_fg_color, tr("tpl.exists"));
            }
        });
        match res {
            DialogResult::Ok if !name.is_empty() => target.map(Some).or(Some(None)),
            DialogResult::Ok | DialogResult::None => None,
            DialogResult::Cancel => Some(None),
        }
    }
}

/// Writes a template file (creating the folder).
pub fn save(path: &std::path::Path, doc: Option<&Document>, wb: Option<&Workbook>, deck: Option<&apex_slides::Deck>) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let r = match (doc, wb, deck) {
        (Some(d), _, _) => apex_io::save_doc(d, path, Format::Docx),
        (_, Some(w), _) => apex_io::save_sheet(w, path, Format::Xlsx),
        (_, _, Some(k)) => apex_io::save_slides(k, path, Format::Pptx),
        _ => return Ok(()),
    };
    r.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_load() {
        for t in builtin() {
            match load(&t) {
                Ok(Loaded::Doc(d)) => assert!(!d.modified, "{}", t.name),
                Ok(Loaded::Sheet(w)) => {
                    assert!(!w.modified, "{}", t.name);
                    assert!(!w.sheets.is_empty());
                }
                Ok(Loaded::Slides(d)) => assert!(!d.slides.is_empty(), "{}", t.name),
                Err(e) => panic!("{}: {e}", t.name),
            }
        }
        // The invoice computes.
        let Ok(Loaded::Sheet(w)) = load(&Template { name: String::new(), kind: Kind::Sheet, source: Source::Builtin(invoice), category: String::new() }) else { panic!() };
        let total = w.cell(0, CellRef::parse("E22").expect("ref")).map(|c| c.value().clone());
        assert_eq!(total, Some(apex_sheet::Value::Number(121.0)));
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("apex-tpl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn shared_categories_and_only_shared() {
        let (user, shared) = (temp("user"), temp("shared"));
        std::fs::create_dir_all(shared.join("Legal")).unwrap();
        for f in ["Memo.dotx", "Legal/Contract.docx", "Legal/notes.txt"] {
            std::fs::write(shared.join(f), b"x").unwrap();
        }
        std::fs::write(user.join("Mine.xlsx"), b"x").unwrap();
        let all = all_templates(Some(&user), Some(&shared), false);
        assert!(all.iter().any(|t| t.source.group() == 0));
        assert!(all.iter().any(|t| t.name == "Mine" && t.source.group() == 2));
        let contract = all.iter().find(|t| t.name == "Contract").expect("in subfolder");
        assert_eq!((contract.category.as_str(), contract.source.group()), ("Legal", 1));
        let only = all_templates(Some(&user), Some(&shared), true);
        let names: Vec<&str> = only.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Memo", "Contract"]);
        assert!(only.iter().all(|t| t.source.group() == 1));
        let _ = (std::fs::remove_dir_all(&user), std::fs::remove_dir_all(&shared));
    }

    #[test]
    fn normal_template_lookup() {
        let d = temp("normal");
        assert_eq!(normal_in(&d, Kind::Doc), None);
        std::fs::write(d.join("NORMAL.ODT"), b"x").unwrap();
        std::fs::write(d.join("normal.dotx"), b"x").unwrap();
        std::fs::write(d.join("Book.ods"), b"x").unwrap();
        std::fs::write(d.join("Normal.xltx"), b"x").unwrap();
        // The Word template comes first.
        assert_eq!(normal_in(&d, Kind::Doc), Some(d.join("normal.dotx")));
        assert_eq!(normal_in(&d, Kind::Sheet), Some(d.join("Book.ods")));
        assert_eq!(normal_in(&d, Kind::Slides), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
