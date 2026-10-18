//! Find and Replace "More" options (wildcards, regular expressions, special
//! characters, formatting) and the Go To dialog (page, section, line,
//! bookmark, comment, notes, heading, table, graphic, equation).

use apex_doc::{FindFormat, FindMode, NoteKind};

use super::*;
use crate::widgets::DialogResult;
use dialogs::FindBar;

/// Special characters for the query: code and description.
const SPECIAL: [(&str, &str); 12] = [
    ("^p", "find.sp.para"),
    ("^t", "find.sp.tab"),
    ("^l", "find.sp.line"),
    ("^?", "find.sp.any_char"),
    ("^#", "find.sp.digit"),
    ("^$", "find.sp.letter"),
    ("^w", "find.sp.white"),
    ("^s", "find.sp.nbsp"),
    ("^+", "find.sp.em_dash"),
    ("^=", "find.sp.en_dash"),
    ("^g", "find.sp.object"),
    ("^^", "find.sp.caret"),
];
const WILDCARDS: [(&str, &str); 9] = [
    ("?", "find.w.one"),
    ("*", "find.w.any"),
    ("@", "find.w.repeat"),
    ("<", "find.w.word_start"),
    (">", "find.w.word_end"),
    ("[]", "find.w.set"),
    ("[!]", "find.w.not_set"),
    ("{1,2}", "find.w.count"),
    ("()", "find.w.group"),
];
const REGEX: [(&str, &str); 8] = [(".", "find.r.any"), (r"\d", "find.sp.digit"), (r"\w", "find.r.word"), (r"\s", "find.sp.white"), (r"\b", "find.r.boundary"), ("^", "find.r.start"), ("$", "find.r.end"), (r"\n", "find.sp.para")];
/// Special characters for the replacement.
const REPL_SPECIAL: [(&str, &str); 8] = [("^p", "find.sp.para"), ("^t", "find.sp.tab"), ("^l", "find.sp.line"), ("^&", "find.sp.found"), ("^s", "find.sp.nbsp"), ("^+", "find.sp.em_dash"), ("^=", "find.sp.en_dash"), ("^^", "find.sp.caret")];

/// Paragraph styles for the format lists: (kind, custom number), name.
pub(super) type StyleList = Vec<((ParaKind, u16), String)>;

fn special_menu(ui: &mut egui::Ui, target: &mut String, items: &[&[(&str, &'static str)]]) {
    crate::widgets::menu_button(ui, format!("{}…", tr("find.special")), |ui| {
        for (k, group) in items.iter().enumerate() {
            if k > 0 {
                ui.separator();
            }
            for (code, key) in group.iter() {
                if ui.button(format!("{code}   {}", tr(key))).clicked() {
                    target.push_str(code);
                    ui.close();
                }
            }
        }
    });
}

fn tri(ui: &mut egui::Ui, id: &str, label: &str, v: &mut Option<bool>) {
    ui.label(label);
    let text = |v: Option<bool>| match v {
        None => tr("find.any"),
        Some(true) => tr("find.yes"),
        Some(false) => tr("find.no"),
    };
    egui::ComboBox::from_id_salt(id).selected_text(text(*v)).width(70.0).show_ui(ui, |ui| {
        for o in [None, Some(true), Some(false)] {
            ui.selectable_value(v, o, text(o));
        }
    });
}

/// "Bold, Heading 1" for a format.
fn describe(f: &FindFormat, styles: &StyleList) -> String {
    let mut out: Vec<String> = Vec::new();
    for (v, on, off) in [(f.bold, "find.f.bold", "find.f.not_bold"), (f.italic, "find.f.italic", "find.f.not_italic"), (f.underline, "find.f.underline", "find.f.not_underline"), (f.strike, "find.f.strike", "find.f.not_strike"), (f.highlight, "find.f.highlight", "find.f.not_highlight")] {
        if let Some(b) = v {
            out.push(tr(if b { on } else { off }).to_string());
        }
    }
    if let Some(font) = &f.font {
        out.push(font.to_string());
    }
    if let Some(s) = f.size {
        out.push(format!("{} pt", s as f32 / 2.0));
    }
    if let Some(p) = f.para_style {
        out.push(styles.iter().find(|(k, _)| *k == p).map_or(String::new(), |(_, n)| n.clone()));
    }
    out.join(", ")
}

/// The format controls of the query (`replace` false) or replacement.
fn format_row(ui: &mut egui::Ui, f: &mut FindFormat, styles: &StyleList, replace: bool) -> bool {
    let mut close = false;
    ui.horizontal_wrapped(|ui| {
        ui.strong(tr(if replace { "find.format_replace" } else { "find.format_find" }));
        let salt = if replace { "r" } else { "f" };
        tri(ui, &format!("ff_b{salt}"), tr("find.f.bold"), &mut f.bold);
        tri(ui, &format!("ff_i{salt}"), tr("find.f.italic"), &mut f.italic);
        tri(ui, &format!("ff_u{salt}"), tr("find.f.underline"), &mut f.underline);
        tri(ui, &format!("ff_s{salt}"), tr("find.f.strike"), &mut f.strike);
        tri(ui, &format!("ff_h{salt}"), tr("find.f.highlight"), &mut f.highlight);
        ui.label(tr("find.f.font"));
        let mut font = f.font.as_deref().unwrap_or_default().to_string();
        if ui.add(egui::TextEdit::singleline(&mut font).hint_text(tr("find.any")).desired_width(110.0)).changed() {
            f.font = (!font.trim().is_empty()).then(|| font.trim().into());
        }
        let mut sized = f.size.is_some();
        if ui.checkbox(&mut sized, tr("find.f.size")).changed() {
            f.size = sized.then_some(24);
        }
        if let Some(s) = &mut f.size {
            let mut pt = *s as f32 / 2.0;
            if ui.add(egui::DragValue::new(&mut pt).range(1.0..=400.0).speed(0.5).suffix(" pt")).changed() {
                *s = (pt * 2.0).round() as u16;
            }
        }
        ui.label(tr("find.f.style"));
        let cur = f.para_style.and_then(|p| styles.iter().find(|(k, _)| *k == p)).map_or(tr("find.any").to_string(), |(_, n)| n.clone());
        egui::ComboBox::from_id_salt(format!("ff_st{salt}")).selected_text(cur).width(130.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut f.para_style, None, tr("find.any"));
            for (k, n) in styles {
                ui.selectable_value(&mut f.para_style, Some(*k), n);
            }
        });
        // Off with no formatting to take away.
        if ui.add_enabled(*f != FindFormat::default(), egui::Button::new(tr("find.no_format"))).clicked() {
            *f = FindFormat::default();
        }
        if ui.button("×").on_hover_text(tr("dialog.close")).clicked() {
            close = true;
        }
    });
    close
}

/// The "More" rows of the find bar.
pub(super) fn find_more(ui: &mut egui::Ui, f: &mut FindBar, styles: &StyleList) {
    ui.horizontal(|ui| {
        ui.radio_value(&mut f.mode, FindMode::Plain, tr("find.mode_text"));
        ui.radio_value(&mut f.mode, FindMode::Wildcards, tr("find.mode_wildcards"));
        ui.radio_value(&mut f.mode, FindMode::Regex, tr("find.mode_regex"));
        ui.separator();
        match f.mode {
            FindMode::Plain => special_menu(ui, &mut f.query, &[&SPECIAL]),
            FindMode::Wildcards => special_menu(ui, &mut f.query, &[&WILDCARDS, &SPECIAL]),
            FindMode::Regex => special_menu(ui, &mut f.query, &[&REGEX]),
        }
        if ui.selectable_label(f.fmt_open == 1, format!("{}…", tr("find.format"))).clicked() {
            f.fmt_open = if f.fmt_open == 1 { 0 } else { 1 };
        }
        if !f.format.is_empty() {
            ui.label(RichText::new(describe(&f.format, styles)).italics());
        }
        if f.show_replace {
            ui.separator();
            ui.label(tr("find.replace_with"));
            let groups: &[(&str, &'static str)] = match f.mode {
                FindMode::Plain => &[],
                FindMode::Wildcards => &[("\\1", "find.w.group1"), ("\\2", "find.w.group2")],
                FindMode::Regex => &[("$1", "find.w.group1"), ("$2", "find.w.group2"), (r"\n", "find.sp.para"), (r"\t", "find.sp.tab")],
            };
            if f.mode == FindMode::Regex {
                special_menu(ui, &mut f.replace, &[groups]);
            } else {
                special_menu(ui, &mut f.replace, &[&REPL_SPECIAL, groups]);
            }
            if ui.selectable_label(f.fmt_open == 2, format!("{}…", tr("find.format"))).clicked() {
                f.fmt_open = if f.fmt_open == 2 { 0 } else { 2 };
            }
            if !f.rformat.is_empty() {
                ui.label(RichText::new(describe(&f.rformat, styles)).italics());
            }
        }
    });
    let close = match f.fmt_open {
        1 => format_row(ui, &mut f.format, styles, false),
        2 if f.show_replace => format_row(ui, &mut f.rformat, styles, true),
        _ => false,
    };
    if close {
        f.fmt_open = 0;
    }
}

/// What Go To goes to.
const GOTO_KINDS: [&str; 11] = ["goto.page", "goto.section", "goto.line", "goto.bookmark", "goto.comment", "goto.footnote", "goto.endnote", "goto.heading", "goto.table", "goto.graphic", "goto.equation"];

#[derive(Default)]
pub(super) struct GoToDlg {
    what: usize,
    text: String,
}

impl DocView {
    pub(super) fn open_goto(&mut self) {
        self.dialog = Some(DocDialog::GoTo(Box::default()));
    }

    /// Paragraph styles for the format lists.
    pub(super) fn style_list(&self) -> StyleList {
        let mut out: StyleList = KINDS.iter().map(|k| ((*k, 0), kind_label(*k).to_string())).collect();
        out.extend(self.custom_styles().into_iter().map(|(n, name)| ((ParaKind::Normal, n), name)));
        out
    }

    /// Where each line of the document starts.
    fn line_starts(&self) -> Vec<Pos> {
        let mut out = Vec::new();
        for (i, pl) in self.layout.paras.iter().enumerate() {
            let Some(p) = self.doc.paragraphs.get(i) else { break };
            let mut ci = 0;
            for r in &pl.galley.rows {
                out.push(Pos::new(i, char_to_byte(&p.text, ci)));
                ci += r.char_count_including_newline().0;
            }
        }
        out
    }

    /// The places of a Go To kind (not pages or bookmarks), in order.
    fn goto_targets(&self, what: usize) -> Vec<Pos> {
        let d = &self.doc;
        let objects = |f: &dyn Fn(&Attrs) -> bool| -> Vec<Pos> { d.paragraphs.iter().enumerate().filter(|(_, p)| apex_doc::para_has_objects(p)).flat_map(|(i, p)| apex_doc::objects_of(p).filter(|(_, a)| f(a)).map(move |(o, _)| Pos::new(i, o))).collect() };
        let mut out = match what {
            1 => d.sections().iter().map(|s| Pos::new(s.first, 0)).collect(),
            2 => self.line_starts(),
            3 => d.bookmarks().into_iter().filter(|b| !b.is_hidden()).map(|b| b.start).collect(),
            4 => d.comment_spans().into_iter().map(|c| c.start).collect(),
            5 | 6 => {
                let kind = if what == 5 { NoteKind::Foot } else { NoteKind::End };
                d.notes().into_iter().filter(|(_, n)| n.kind == kind).map(|(p, _)| p).collect()
            }
            7 => d.paragraphs.iter().enumerate().filter(|(_, p)| matches!(p.style.kind, ParaKind::Heading(_)) && p.style.toc.is_none()).map(|(i, _)| Pos::new(i, 0)).collect(),
            8 => d.all_tables().into_iter().filter(|t| t.depth == 0).map(|t| Pos::new(t.start, 0)).collect(),
            9 => objects(&|a| a.image.is_some() || a.shape.is_some()),
            10 => objects(&|a| a.math.is_some()),
            _ => Vec::new(),
        };
        out.sort();
        out.dedup();
        out
    }

    /// Goes to the place `text` names ("3", "+2", "-1", a bookmark name;
    /// empty for the next one), or with `step` to the previous or next one.
    fn goto(&mut self, what: usize, text: &str, step: i32) -> bool {
        let t = text.trim();
        let rel = |cur: usize, n: usize| -> Option<usize> {
            if step != 0 || t.is_empty() {
                return None;
            }
            if let Some(k) = t.strip_prefix('+') {
                return Some(cur + k.parse::<usize>().ok()?);
            }
            if let Some(k) = t.strip_prefix('-') {
                return Some(cur.saturating_sub(k.parse().ok()?));
            }
            t.parse::<usize>().ok().filter(|&k| k >= 1 && k <= n).map(|k| k - 1)
        };
        if what == 0 {
            let (cur, n) = (self.caret_page(), self.layout.pages.max(1));
            let target = match step {
                -1 => cur.checked_sub(1),
                1 => Some(cur + 1).filter(|&p| p < n),
                _ if t.is_empty() => Some(cur + 1).filter(|&p| p < n),
                _ => rel(cur, n),
            };
            return target.map(|p| self.go_to_page(p.min(n - 1))).is_some();
        }
        if what == 3 && step == 0 && !t.is_empty() {
            if let Some(b) = self.doc.bookmark(t) {
                self.go_to_bookmark(&b);
                return true;
            }
        }
        let targets = self.goto_targets(what);
        let c = self.sel.start();
        let at = targets.partition_point(|p| *p <= c);
        let cur = at.saturating_sub(1);
        let idx = match step {
            -1 => targets.iter().rposition(|p| *p < c),
            1 => targets.iter().position(|p| *p > c),
            _ if t.is_empty() => targets.iter().position(|p| *p > c),
            _ => rel(cur, targets.len()).filter(|&i| i < targets.len()),
        };
        let Some(p) = idx.map(|i| targets[i]) else { return false };
        if what == 3 {
            if let Some(b) = self.doc.bookmarks().into_iter().find(|b| b.start == p && !b.is_hidden()) {
                self.go_to_bookmark(&b);
                return true;
            }
        }
        self.caret(p, false);
        self.desired_x = None;
        true
    }

    /// The Go To dialog; true when it is done.
    pub(super) fn goto_dialog(&mut self, ctx: &egui::Context, g: &mut GoToDlg) -> DialogResult {
        let mut step = 0;
        let names: Vec<String> = if g.what == 3 { self.doc.bookmarks().into_iter().filter(|b| !b.is_hidden()).map(|b| b.name.to_string()).collect() } else { Vec::new() };
        let total = match g.what {
            0 => self.layout.pages,
            3 => names.len(),
            k => self.goto_targets(k).len(),
        };
        let res = widgets::dialog(ctx, "doc_goto", tr("goto.title"), 440.0, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.label(tr("goto.what"));
                    ui.set_width(150.0);
                    for (i, k) in GOTO_KINDS.iter().enumerate() {
                        if ui.selectable_label(g.what == i, tr(k)).clicked() {
                            g.what = i;
                            g.text.clear();
                        }
                    }
                });
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    if g.what == 3 {
                        ui.label(tr("goto.bookmark_name"));
                        egui::ComboBox::from_id_salt("goto_bm").selected_text(g.text.clone()).width(220.0).show_ui(ui, |ui| {
                            for n in &names {
                                ui.selectable_value(&mut g.text, n.clone(), n);
                            }
                        });
                    } else {
                        ui.label(trf("goto.number", &[("what", &tr(GOTO_KINDS[g.what]).to_lowercase()), ("total", &total)]));
                        let r = ui.add(egui::TextEdit::singleline(&mut g.text).desired_width(220.0));
                        if !r.has_focus() && g.text.is_empty() && !ui.ctx().memory(|m| m.focused().is_some()) {
                            r.request_focus();
                        }
                        ui.label(RichText::new(tr("goto.tip")).small().weak());
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr("find.prev")).clicked() {
                            step = -1;
                        }
                        if ui.button(tr("find.next")).clicked() {
                            step = 1;
                        }
                    });
                });
            });
        });
        if step != 0 && !self.goto(g.what, "", step) {
            self.notices.push(Notice::info(tr("goto.none")));
        }
        if res == DialogResult::Ok && !self.goto(g.what, &g.text, 0) {
            self.notices.push(Notice::info(tr("goto.none")));
        }
        res
    }
}
