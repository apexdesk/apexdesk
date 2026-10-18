//! The pieces of the Get Data editor: the settings of a source and of each
//! kind of step, a one-line summary of a step, and the data preview.

use apex_query::{DataTable, Val};
use apex_sheet::query::{AggFn, Aggregate, ColType, DbKind, FilterOp, JoinKind, Source, Step};
use eframe::egui::{self, RichText};

use crate::i18n::{tr, trf};
use crate::widgets;

/// The kinds of step, in the Add Step menu's order.
pub(super) const STEP_KINDS: [&str; 18] = [
    "getdata.step.filter",
    "getdata.step.sort",
    "getdata.step.choose_cols",
    "getdata.step.remove_cols",
    "getdata.step.rename",
    "getdata.step.move",
    "getdata.step.type",
    "getdata.step.split",
    "getdata.step.add_col",
    "getdata.step.replace",
    "getdata.step.dedupe",
    "getdata.step.keep_rows",
    "getdata.step.promote",
    "getdata.step.group",
    "getdata.step.pivot",
    "getdata.step.unpivot",
    "getdata.step.append",
    "getdata.step.merge",
];

/// A new step of kind `key` over columns `cols`.
pub(super) fn new_step(key: &str, cols: &[String], others: &[String]) -> Step {
    let first = cols.first().cloned().unwrap_or_default();
    let second = cols.get(1).cloned().unwrap_or_else(|| first.clone());
    let other = others.first().cloned().unwrap_or_default();
    match key {
        "getdata.step.filter" => Step::Filter { column: first, op: FilterOp::Eq, value: String::new(), case: false },
        "getdata.step.sort" => Step::Sort(vec![(first, true)]),
        "getdata.step.choose_cols" => Step::ChooseColumns(cols.to_vec()),
        "getdata.step.remove_cols" => Step::RemoveColumns(Vec::new()),
        "getdata.step.rename" => Step::Rename(Vec::new()),
        "getdata.step.move" => Step::Move { column: first, to: 0 },
        "getdata.step.type" => Step::ChangeType { column: first, ty: ColType::Text },
        "getdata.step.split" => Step::Split { column: first, delimiter: ",".into() },
        "getdata.step.add_col" => Step::AddColumn { name: unique_col(cols, &tr("getdata.new_column").to_string()), formula: String::new() },
        "getdata.step.replace" => Step::Replace { column: first, find: String::new(), replace: String::new(), whole: false },
        "getdata.step.dedupe" => Step::RemoveDuplicates(Vec::new()),
        "getdata.step.keep_rows" => Step::KeepRows { skip: 0, take: 100 },
        "getdata.step.promote" => Step::PromoteHeaders,
        "getdata.step.group" => Step::Group { by: vec![first], aggs: vec![Aggregate { column: String::new(), func: AggFn::Count, name: tr("getdata.agg.count").to_string() }] },
        "getdata.step.pivot" => Step::Pivot { column: first, values: second, func: AggFn::Sum },
        "getdata.step.unpivot" => Step::Unpivot { columns: Vec::new(), attribute: tr("getdata.attribute").to_string(), value: tr("getdata.value").to_string() },
        "getdata.step.append" => Step::Append { query: other },
        _ => Step::Merge { query: other, keys: vec![(first.clone(), first)], join: JoinKind::Left, columns: Vec::new(), names: Vec::new() },
    }
}

fn unique_col(cols: &[String], base: &str) -> String {
    if !cols.iter().any(|c| c.eq_ignore_ascii_case(base)) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !cols.iter().any(|c| c.eq_ignore_ascii_case(n))).unwrap_or_default()
}

fn op_key(op: FilterOp) -> &'static str {
    match op {
        FilterOp::Eq => "getdata.op.eq",
        FilterOp::Ne => "getdata.op.ne",
        FilterOp::Gt => "getdata.op.gt",
        FilterOp::Ge => "getdata.op.ge",
        FilterOp::Lt => "getdata.op.lt",
        FilterOp::Le => "getdata.op.le",
        FilterOp::Contains => "getdata.op.contains",
        FilterOp::NotContains => "getdata.op.not_contains",
        FilterOp::Begins => "getdata.op.begins",
        FilterOp::Ends => "getdata.op.ends",
        FilterOp::Empty => "getdata.op.empty",
        FilterOp::NotEmpty => "getdata.op.not_empty",
    }
}

fn type_key(t: ColType) -> &'static str {
    match t {
        ColType::Any => "getdata.type.any",
        ColType::Text => "getdata.type.text",
        ColType::Number => "getdata.type.number",
        ColType::Integer => "getdata.type.integer",
        ColType::Bool => "getdata.type.bool",
        ColType::Date => "getdata.type.date",
        ColType::DateTime => "getdata.type.datetime",
    }
}

fn agg_key(f: AggFn) -> &'static str {
    match f {
        AggFn::Count => "getdata.agg.count",
        AggFn::CountDistinct => "getdata.agg.count_distinct",
        AggFn::Sum => "getdata.agg.sum",
        AggFn::Average => "getdata.agg.average",
        AggFn::Min => "getdata.agg.min",
        AggFn::Max => "getdata.agg.max",
        AggFn::First => "getdata.agg.first",
        AggFn::Last => "getdata.agg.last",
    }
}

fn join_key(j: JoinKind) -> &'static str {
    match j {
        JoinKind::Inner => "getdata.join.inner",
        JoinKind::Left => "getdata.join.left",
        JoinKind::Right => "getdata.join.right",
        JoinKind::Full => "getdata.join.full",
        JoinKind::Anti => "getdata.join.anti",
    }
}

/// "a, b, c" (at most four names).
fn names(list: &[String]) -> String {
    let mut s = list.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
    if list.len() > 4 {
        s.push_str(", …");
    }
    s
}

/// What a step does, in a few words.
pub(super) fn summary(s: &Step) -> String {
    match s {
        Step::PromoteHeaders => String::new(),
        Step::KeepRows { skip, take } => trf("getdata.sum.keep_rows", &[("skip", skip), ("take", &if *take == 0 { "∞".to_string() } else { take.to_string() })]),
        Step::Filter { column, op, value, case } => {
            let s = format!("{column} {} {value}", tr(op_key(*op)).to_lowercase());
            if *case { format!("{s} ({})", tr("find.case").to_lowercase()) } else { s }
        }
        Step::RemoveColumns(c) | Step::ChooseColumns(c) | Step::RemoveDuplicates(c) => names(c),
        Step::Rename(p) => p.iter().take(3).map(|(a, b)| format!("{a} → {b}")).collect::<Vec<_>>().join(", "),
        Step::Move { column, to } => format!("{column} → {}", to + 1),
        Step::ChangeType { column, ty } => format!("{column}: {}", tr(type_key(*ty)).to_lowercase()),
        Step::Split { column, delimiter } => format!("{column} «{delimiter}»"),
        Step::AddColumn { name, formula } => format!("{name} = {formula}"),
        Step::Replace { column, find, replace, .. } => format!("{column}: «{find}» → «{replace}»"),
        Step::Sort(k) => k.iter().map(|(c, a)| format!("{c} {}", if *a { "↑" } else { "↓" })).collect::<Vec<_>>().join(", "),
        Step::Group { by, aggs } => format!("{} ({})", names(by), aggs.iter().map(|a| tr(agg_key(a.func)).to_lowercase()).collect::<Vec<_>>().join(", ")),
        Step::Pivot { column, values, func } => format!("{column} × {} {values}", tr(agg_key(*func)).to_lowercase()),
        Step::Unpivot { columns, .. } => names(columns),
        Step::Append { query } => query.clone(),
        Step::Merge { query, join, .. } => format!("{query} ({})", tr(join_key(*join)).to_lowercase()),
    }
}

/// A drop-down of the columns `cols`; true when it changed.
fn col_combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, value: &mut String, cols: &[String]) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).width(200.0).selected_text(value.as_str()).show_ui(ui, |ui| {
        for c in cols {
            if ui.selectable_label(value == c, c).clicked() && value != c {
                *value = c.clone();
                changed = true;
            }
        }
    });
    changed
}

/// A drop-down of `items` (i18n keys) for a value of type `T`.
fn key_combo<T: Copy + PartialEq>(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, value: &mut T, items: &[T], key: fn(T) -> &'static str) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).width(200.0).selected_text(tr(key(*value))).show_ui(ui, |ui| {
        for &it in items {
            if ui.selectable_label(*value == it, tr(key(it))).clicked() && *value != it {
                *value = it;
                changed = true;
            }
        }
    });
    changed
}

/// Check boxes for picking some of `cols` (the picked ones in `sel`, in
/// the columns' order).
fn col_checks(ui: &mut egui::Ui, sel: &mut Vec<String>, cols: &[String]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.small_button(tr("getdata.all")).clicked() {
            *sel = cols.to_vec();
            changed = true;
        }
        if ui.small_button(tr("getdata.none")).clicked() {
            sel.clear();
            changed = true;
        }
    });
    egui::ScrollArea::vertical().id_salt("col_checks").max_height(130.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for c in cols {
                let mut on = sel.iter().any(|s| s == c);
                if ui.checkbox(&mut on, c).changed() {
                    changed = true;
                    if on {
                        sel.push(c.clone());
                        sel.sort_by_key(|s| cols.iter().position(|x| x == s).unwrap_or(usize::MAX));
                    } else {
                        sel.retain(|s| s != c);
                    }
                }
            }
        });
    });
    changed
}

fn text(ui: &mut egui::Ui, s: &mut String, width: f32, hint: &str) -> bool {
    ui.add(egui::TextEdit::singleline(s).desired_width(width).hint_text(hint)).changed()
}

/// The settings of a step; `cols` are its input columns, `queries` the
/// other queries and `other` the columns of the one a merge joins.
pub(super) fn step_form(ui: &mut egui::Ui, s: &mut Step, cols: &[String], queries: &[String], other: &[String]) -> bool {
    let mut ch = false;
    match s {
        Step::PromoteHeaders => {
            ui.label(RichText::new(tr("getdata.promote_help")).weak());
        }
        Step::KeepRows { skip, take } => widgets::form(ui, "gd_keep", |ui| {
            widgets::form_row(ui, tr("getdata.skip_rows"), |ui| ch |= ui.add(egui::DragValue::new(skip).range(0..=10_000_000)).changed());
            widgets::form_row(ui, tr("getdata.take_rows"), |ui| {
                ch |= ui.add(egui::DragValue::new(take).range(0..=10_000_000)).changed();
                ui.label(RichText::new(tr("getdata.zero_all")).weak());
            });
        }),
        Step::Filter { column, op, value, case } => widgets::form(ui, "gd_filter", |ui| {
            widgets::form_row(ui, tr("getdata.column"), |ui| ch |= col_combo(ui, "gd_fc", column, cols));
            widgets::form_row(ui, tr("getdata.condition"), |ui| ch |= key_combo(ui, "gd_fo", op, &FilterOp::ALL, op_key));
            if op.takes_value() {
                widgets::form_row(ui, tr("getdata.value"), |ui| ch |= text(ui, value, 200.0, ""));
                widgets::form_row(ui, "", |ui| ch |= ui.checkbox(case, tr("find.case")).changed());
            }
        }),
        Step::RemoveDuplicates(sel) => {
            ui.label(RichText::new(tr("getdata.dedupe_help")).weak());
            ch |= col_checks(ui, sel, cols);
        }
        Step::RemoveColumns(sel) | Step::ChooseColumns(sel) => ch |= col_checks(ui, sel, cols),
        Step::Rename(pairs) => {
            egui::ScrollArea::vertical().id_salt("gd_rename").max_height(170.0).show(ui, |ui| {
                egui::Grid::new("gd_rename_grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                    for c in cols {
                        let mut new = pairs.iter().find(|(a, _)| a == c).map_or_else(|| c.clone(), |(_, b)| b.clone());
                        ui.label(c);
                        if ui.add(egui::TextEdit::singleline(&mut new).desired_width(200.0)).changed() {
                            ch = true;
                            pairs.retain(|(a, _)| a != c);
                            if new != *c {
                                pairs.push((c.clone(), new));
                            }
                        }
                        ui.end_row();
                    }
                });
            });
        }
        Step::Move { column, to } => widgets::form(ui, "gd_move", |ui| {
            widgets::form_row(ui, tr("getdata.column"), |ui| ch |= col_combo(ui, "gd_mc", column, cols));
            let mut pos = *to + 1;
            widgets::form_row(ui, tr("getdata.position"), |ui| {
                if ui.add(egui::DragValue::new(&mut pos).range(1..=cols.len().max(1))).changed() {
                    *to = pos - 1;
                    ch = true;
                }
            });
        }),
        Step::ChangeType { column, ty } => widgets::form(ui, "gd_type", |ui| {
            widgets::form_row(ui, tr("getdata.column"), |ui| ch |= col_combo(ui, "gd_tc", column, cols));
            widgets::form_row(ui, tr("getdata.type"), |ui| ch |= key_combo(ui, "gd_tt", ty, &ColType::ALL, type_key));
        }),
        Step::Split { column, delimiter } => widgets::form(ui, "gd_split", |ui| {
            widgets::form_row(ui, tr("getdata.column"), |ui| ch |= col_combo(ui, "gd_sc", column, cols));
            widgets::form_row(ui, tr("getdata.delimiter"), |ui| ch |= text(ui, delimiter, 80.0, ""));
        }),
        Step::AddColumn { name, formula } => {
            widgets::form(ui, "gd_add", |ui| {
                widgets::form_row(ui, tr("getdata.new_name"), |ui| ch |= text(ui, name, 200.0, ""));
                widgets::form_row(ui, tr("getdata.insert_column"), |ui| {
                    let mut pick = String::new();
                    if col_combo(ui, "gd_ai", &mut pick, cols) {
                        formula.push_str(&format!("[{pick}]"));
                        ch = true;
                    }
                });
            });
            let r = ui.add(egui::TextEdit::multiline(formula).code_editor().desired_rows(2).desired_width(f32::INFINITY).hint_text("[Price] * [Qty]"));
            ui.data_mut(|m| m.insert_temp(r.id.with("multiline"), true));
            ch |= r.changed();
            ui.label(RichText::new(trf("getdata.formula_help", &[("list", &apex_query::expr::FUNCTIONS.join(", "))])).small().weak());
        }
        Step::Replace { column, find, replace, whole } => widgets::form(ui, "gd_repl", |ui| {
            widgets::form_row(ui, tr("getdata.column"), |ui| ch |= col_combo(ui, "gd_rc", column, cols));
            widgets::form_row(ui, tr("getdata.find"), |ui| ch |= text(ui, find, 200.0, ""));
            widgets::form_row(ui, tr("getdata.replace_with"), |ui| ch |= text(ui, replace, 200.0, ""));
            widgets::form_row(ui, "", |ui| ch |= ui.checkbox(whole, tr("getdata.whole")).changed());
        }),
        Step::Sort(keys) => {
            let mut drop = None;
            for (i, (c, asc)) in keys.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(if i == 0 { tr("getdata.sort_by") } else { tr("getdata.then_by") });
                    ch |= col_combo(ui, ("gd_sort", i), c, cols);
                    ch |= ui.selectable_value(asc, true, tr("getdata.asc")).changed();
                    ch |= ui.selectable_value(asc, false, tr("getdata.desc")).changed();
                    if i > 0 && ui.small_button("✕").on_hover_text(tr("getdata.remove")).clicked() {
                        drop = Some(i);
                    }
                });
            }
            if let Some(i) = drop.filter(|_| keys.len() > 1) {
                keys.remove(i);
                ch = true;
            }
            if ui.button(tr("getdata.add_level")).clicked() {
                keys.push((cols.first().cloned().unwrap_or_default(), true));
                ch = true;
            }
        }
        Step::Group { by, aggs } => {
            ui.label(RichText::new(tr("getdata.group_by")).strong());
            ch |= col_checks(ui, by, cols);
            ui.add_space(4.0);
            ui.label(RichText::new(tr("getdata.aggregations")).strong());
            let mut drop = None;
            let mut with_blank = vec![String::new()];
            with_blank.extend(cols.iter().cloned());
            for (i, a) in aggs.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ch |= text(ui, &mut a.name, 140.0, tr("getdata.new_name"));
                    ch |= key_combo(ui, ("gd_af", i), &mut a.func, &AggFn::ALL, agg_key);
                    if a.func != AggFn::Count {
                        ch |= col_combo(ui, ("gd_ac", i), &mut a.column, cols);
                    }
                    if ui.small_button("✕").on_hover_text(tr("getdata.remove")).clicked() {
                        drop = Some(i);
                    }
                });
            }
            if let Some(i) = drop {
                aggs.remove(i);
                ch = true;
            }
            if ui.button(tr("getdata.add_agg")).clicked() {
                let c = cols.iter().find(|c| !by.contains(c)).cloned().unwrap_or_default();
                aggs.push(Aggregate { name: format!("{} {c}", tr("getdata.agg.sum")), column: c, func: AggFn::Sum });
                ch = true;
            }
        }
        Step::Pivot { column, values, func } => widgets::form(ui, "gd_pivot", |ui| {
            widgets::form_row(ui, tr("getdata.pivot_column"), |ui| ch |= col_combo(ui, "gd_pc", column, cols));
            widgets::form_row(ui, tr("getdata.values_column"), |ui| ch |= col_combo(ui, "gd_pv", values, cols));
            widgets::form_row(ui, tr("getdata.aggregate"), |ui| ch |= key_combo(ui, "gd_pf", func, &AggFn::ALL, agg_key));
        }),
        Step::Unpivot { columns, attribute, value } => {
            ch |= col_checks(ui, columns, cols);
            widgets::form(ui, "gd_unpivot", |ui| {
                widgets::form_row(ui, tr("getdata.attribute_name"), |ui| ch |= text(ui, attribute, 200.0, ""));
                widgets::form_row(ui, tr("getdata.value_name"), |ui| ch |= text(ui, value, 200.0, ""));
            });
        }
        Step::Append { query } => widgets::form(ui, "gd_append", |ui| {
            widgets::form_row(ui, tr("getdata.query"), |ui| ch |= col_combo(ui, "gd_aq", query, queries));
        }),
        Step::Merge { query, keys, join, columns, .. } => {
            widgets::form(ui, "gd_merge", |ui| {
                widgets::form_row(ui, tr("getdata.query"), |ui| ch |= col_combo(ui, "gd_mq", query, queries));
                widgets::form_row(ui, tr("getdata.join"), |ui| ch |= key_combo(ui, "gd_mj", join, &JoinKind::ALL, join_key));
            });
            ui.label(RichText::new(tr("getdata.keys")).strong());
            let mut drop = None;
            for (i, (l, r)) in keys.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ch |= col_combo(ui, ("gd_kl", i), l, cols);
                    ui.label("=");
                    ch |= col_combo(ui, ("gd_kr", i), r, other);
                    if ui.small_button("✕").on_hover_text(tr("getdata.remove")).clicked() {
                        drop = Some(i);
                    }
                });
            }
            if let Some(i) = drop {
                keys.remove(i);
                ch = true;
            }
            if ui.button(tr("getdata.add_key")).clicked() {
                keys.push((cols.first().cloned().unwrap_or_default(), other.first().cloned().unwrap_or_default()));
                ch = true;
            }
            if *join != JoinKind::Anti {
                ui.label(RichText::new(tr("getdata.bring_columns")).strong());
                let avail: Vec<String> = other.iter().filter(|c| !keys.iter().any(|(_, r)| r == *c)).cloned().collect();
                ch |= col_checks(ui, columns, &avail);
            }
        }
    }
    ch
}

/// Encodings offered for text files (WHATWG labels; empty: detect).
const ENCODINGS: [&str; 11] = ["", "utf-8", "utf-16le", "utf-16be", "windows-1252", "cp850", "iso-8859-1", "iso-8859-15", "shift_jis", "gb18030", "koi8-r"];

/// Delimiters offered (None: detect).
const DELIMS: [(Option<char>, &str); 6] = [(None, "getdata.delim.auto"), (Some(','), "getdata.delim.comma"), (Some(';'), "getdata.delim.semicolon"), (Some('\t'), "getdata.delim.tab"), (Some('|'), "getdata.delim.pipe"), (Some(' '), "getdata.delim.space")];

/// What the source form asks the editor to do.
#[derive(Default)]
pub(crate) struct SourceOut {
    /// The source changed (read it again).
    pub changed: bool,
    /// Browse for a file (true: a folder) and put it in the source.
    pub browse: Option<bool>,
    /// A password typed and "Connect" pressed.
    pub connect: bool,
}

fn location_row(ui: &mut egui::Ui, label: &str, loc: &mut String, hint: &str, browse: Option<bool>, out: &mut SourceOut) {
    widgets::form_row(ui, label, |ui| {
        out.changed |= ui.add(egui::TextEdit::singleline(loc).desired_width(360.0).hint_text(hint)).changed();
        if let Some(folder) = browse
            && ui.button(format!("{}…", tr("getdata.browse"))).clicked()
        {
            out.browse = Some(folder);
        }
    });
}

fn choice_row(ui: &mut egui::Ui, label: &str, id: &str, value: &mut String, choices: &[String], out: &mut SourceOut) {
    if choices.is_empty() {
        return;
    }
    widgets::form_row(ui, label, |ui| out.changed |= col_combo(ui, id, value, choices));
}

/// The settings of a source. `choices` are what the source offers (sheets,
/// page tables, database tables); `tables` and `queries` the workbook's.
pub(crate) fn source_form(ui: &mut egui::Ui, src: &mut Source, choices: &[String], tables: &[String], queries: &[String], password: &mut String) -> SourceOut {
    let mut out = SourceOut::default();
    let file_or_url = tr("getdata.file_or_url");
    widgets::form(ui, "gd_source", |ui| match src {
        Source::Text { location, delimiter, encoding, header, skip } => {
            location_row(ui, tr("getdata.location"), location, file_or_url, Some(false), &mut out);
            widgets::form_row(ui, tr("getdata.delimiter"), |ui| {
                let label = DELIMS.iter().find(|(d, _)| d == delimiter).map_or_else(|| delimiter.map(String::from).unwrap_or_default(), |(_, k)| tr(k).to_string());
                egui::ComboBox::from_id_salt("gd_delim").width(160.0).selected_text(label).show_ui(ui, |ui| {
                    for (d, k) in DELIMS {
                        if ui.selectable_label(*delimiter == d, tr(k)).clicked() {
                            *delimiter = d;
                            out.changed = true;
                        }
                    }
                });
                let mut custom = delimiter.filter(|d| !DELIMS.iter().any(|(x, _)| *x == Some(*d))).map(String::from).unwrap_or_default();
                if ui.add(egui::TextEdit::singleline(&mut custom).desired_width(40.0).char_limit(1).hint_text(tr("getdata.other"))).changed() {
                    *delimiter = custom.chars().next().or(*delimiter);
                    out.changed = true;
                }
            });
            widgets::form_row(ui, tr("getdata.encoding"), |ui| {
                let label = |e: &str| if e.is_empty() { tr("getdata.detect").to_string() } else { e.to_string() };
                egui::ComboBox::from_id_salt("gd_enc").width(160.0).selected_text(label(encoding)).show_ui(ui, |ui| {
                    for e in ENCODINGS {
                        if ui.selectable_label(encoding == e, label(e)).clicked() {
                            *encoding = e.to_string();
                            out.changed = true;
                        }
                    }
                });
            });
            widgets::form_row(ui, tr("getdata.skip_lines"), |ui| out.changed |= ui.add(egui::DragValue::new(skip).range(0..=100_000)).changed());
            widgets::form_row(ui, "", |ui| out.changed |= ui.checkbox(header, tr("getdata.header")).changed());
        }
        Source::Workbook { path, item, header } => {
            location_row(ui, tr("getdata.file"), path, "", Some(false), &mut out);
            choice_row(ui, tr("getdata.sheet_or_table"), "gd_item", item, choices, &mut out);
            widgets::form_row(ui, "", |ui| out.changed |= ui.checkbox(header, tr("getdata.header")).changed());
        }
        Source::Folder { path, extension, header, file_column } => {
            location_row(ui, tr("getdata.folder"), path, "", Some(true), &mut out);
            widgets::form_row(ui, tr("getdata.file_type"), |ui| {
                let label = |e: &str| if e.is_empty() { tr("getdata.all_files").to_string() } else { format!("*.{e}") };
                egui::ComboBox::from_id_salt("gd_ext").width(160.0).selected_text(label(extension)).show_ui(ui, |ui| {
                    for e in ["", "csv", "txt", "tsv", "json", "xml", "xlsx", "xls", "ods"] {
                        if ui.selectable_label(extension == e, label(e)).clicked() {
                            *extension = e.to_string();
                            out.changed = true;
                        }
                    }
                });
            });
            widgets::form_row(ui, "", |ui| out.changed |= ui.checkbox(header, tr("getdata.header")).changed());
            widgets::form_row(ui, "", |ui| out.changed |= ui.checkbox(file_column, tr("getdata.file_column")).changed());
        }
        Source::Json { location, path } => {
            location_row(ui, tr("getdata.location"), location, file_or_url, Some(false), &mut out);
            widgets::form_row(ui, tr("getdata.json_path"), |ui| out.changed |= text(ui, path, 200.0, tr("getdata.auto_hint")));
        }
        Source::Xml { location, element } => {
            location_row(ui, tr("getdata.location"), location, file_or_url, Some(false), &mut out);
            widgets::form_row(ui, tr("getdata.xml_element"), |ui| out.changed |= text(ui, element, 200.0, tr("getdata.auto_hint")));
        }
        Source::Html { location, index } => {
            location_row(ui, tr("getdata.url"), location, "https://", Some(false), &mut out);
            if !choices.is_empty() {
                widgets::form_row(ui, tr("getdata.page_table"), |ui| {
                    egui::ComboBox::from_id_salt("gd_html").width(360.0).selected_text(choices.get(*index).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                        for (i, c) in choices.iter().enumerate() {
                            if ui.selectable_label(*index == i, c).clicked() {
                                *index = i;
                                out.changed = true;
                            }
                        }
                    });
                });
            }
        }
        Source::Pdf { location, index, header } => {
            location_row(ui, tr("getdata.location"), location, file_or_url, Some(false), &mut out);
            if !choices.is_empty() {
                widgets::form_row(ui, tr("getdata.pdf_table"), |ui| {
                    egui::ComboBox::from_id_salt("gd_pdf").width(360.0).selected_text(choices.get(*index).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                        for (i, c) in choices.iter().enumerate() {
                            if ui.selectable_label(*index == i, c).clicked() {
                                *index = i;
                                out.changed = true;
                            }
                        }
                    });
                });
            }
            widgets::form_row(ui, "", |ui| out.changed |= ui.checkbox(header, tr("getdata.header")).changed());
        }
        Source::Sqlite { path, sql } => {
            location_row(ui, tr("getdata.file"), path, "", Some(false), &mut out);
            sql_rows(ui, sql, choices, &mut out);
        }
        Source::DbFile { path, table } => {
            location_row(ui, tr("getdata.file"), path, "", Some(false), &mut out);
            if !choices.is_empty() {
                widgets::form_row(ui, tr("getdata.table"), |ui| out.changed |= col_combo(ui, "gd_dbf", table, choices));
            }
        }
        Source::Database { kind, host, port, database, user, sql, tls } => {
            widgets::form_row(ui, tr("getdata.server"), |ui| {
                out.changed |= text(ui, host, 240.0, "localhost");
                ui.label(":");
                out.changed |= ui.add(egui::DragValue::new(port).range(1..=65535)).changed();
            });
            widgets::form_row(ui, tr("getdata.database"), |ui| out.changed |= text(ui, database, 240.0, ""));
            widgets::form_row(ui, tr("getdata.user"), |ui| out.changed |= text(ui, user, 240.0, ""));
            widgets::form_row(ui, tr("getdata.password"), |ui| {
                let r = ui.add(egui::TextEdit::singleline(password).password(true).desired_width(240.0));
                if ui.button(tr("getdata.connect")).clicked() || r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    out.connect = true;
                }
            });
            let sql_server = *kind == DbKind::SqlServer;
            widgets::form_row(ui, "", |ui| {
                out.changed |= ui.checkbox(tls, tr("getdata.tls")).changed();
                ui.label(RichText::new(tr("getdata.password_note")).small().weak());
            });
            if sql_server && apex_query::sources::sqlserver::INTEGRATED {
                widgets::form_row(ui, "", |ui| ui.label(RichText::new(tr("getdata.windows_login")).small().weak()));
                widgets::form_row(ui, "", |ui| ui.label(RichText::new(tr("getdata.domain_login")).small().weak()));
            }
            sql_rows(ui, sql, choices, &mut out);
        }
        Source::Table { name } => {
            widgets::form_row(ui, tr("getdata.table"), |ui| out.changed |= col_combo(ui, "gd_tbl", name, tables));
        }
        Source::Query { name } => {
            widgets::form_row(ui, tr("getdata.query"), |ui| out.changed |= col_combo(ui, "gd_src_q", name, queries));
        }
    });
    out
}

fn sql_rows(ui: &mut egui::Ui, sql: &mut String, choices: &[String], out: &mut SourceOut) {
    if !choices.is_empty() {
        widgets::form_row(ui, tr("getdata.table"), |ui| {
            let mut pick = if choices.iter().any(|c| c == sql.trim()) { sql.trim().to_string() } else { String::new() };
            if col_combo(ui, "gd_dbt", &mut pick, choices) {
                *sql = pick;
                out.changed = true;
            }
        });
    }
    widgets::form_row(ui, tr("getdata.sql"), |ui| {
        let r = ui.add(egui::TextEdit::multiline(sql).code_editor().desired_rows(3).desired_width(360.0).hint_text("SELECT * FROM …"));
        ui.data_mut(|m| m.insert_temp(r.id.with("multiline"), true));
        out.changed |= r.changed();
    });
}

/// A fresh source of the kind `key` names (a `getdata.src.*` key).
pub(crate) fn new_source(key: &str) -> Source {
    let loc = String::new();
    match key {
        "getdata.src.text" => Source::Text { location: loc, delimiter: None, encoding: String::new(), header: true, skip: 0 },
        "getdata.src.workbook" => Source::Workbook { path: loc, item: String::new(), header: true },
        "getdata.src.folder" => Source::Folder { path: loc, extension: String::new(), header: true, file_column: true },
        "getdata.src.json" => Source::Json { location: loc, path: String::new() },
        "getdata.src.xml" => Source::Xml { location: loc, element: String::new() },
        "getdata.src.html" => Source::Html { location: loc, index: 0 },
        "getdata.src.pdf" => Source::Pdf { location: loc, index: 0, header: true },
        "getdata.src.sqlite" => Source::Sqlite { path: loc, sql: String::new() },
        "getdata.src.dbfile" => Source::DbFile { path: loc, table: String::new() },
        "getdata.src.postgres" => Source::Database { kind: DbKind::Postgres, host: "localhost".into(), port: 5432, database: String::new(), user: String::new(), sql: String::new(), tls: false },
        "getdata.src.mysql" => Source::Database { kind: DbKind::MySql, host: "localhost".into(), port: 3306, database: String::new(), user: String::new(), sql: String::new(), tls: false },
        "getdata.src.sqlserver" => Source::Database { kind: DbKind::SqlServer, host: "localhost".into(), port: 1433, database: String::new(), user: String::new(), sql: String::new(), tls: false },
        "getdata.src.query" => Source::Query { name: String::new() },
        _ => Source::Table { name: String::new() },
    }
}

/// File filters of the sources that read a file: (label key, extensions).
pub(crate) fn file_filter(src: &Source) -> Option<(&'static str, &'static [&'static str])> {
    match src {
        Source::Text { .. } => Some(("getdata.filter.text", &["csv", "tsv", "txt", "tab", "dat"])),
        Source::Workbook { .. } => Some(("dialog.filter_sheets", &["xlsx", "xlsm", "xls", "xlsb", "ods", "csv"])),
        Source::Json { .. } => Some(("getdata.filter.json", &["json", "jsonl", "geojson"])),
        Source::Xml { .. } => Some(("getdata.filter.xml", &["xml"])),
        Source::Html { .. } => Some(("getdata.filter.html", &["html", "htm"])),
        Source::Pdf { .. } => Some(("getdata.filter.pdf", &["pdf"])),
        Source::Sqlite { .. } => Some(("getdata.filter.sqlite", &["db", "sqlite", "sqlite3", "db3"])),
        Source::DbFile { .. } => Some(("getdata.filter.dbfile", apex_io::DB_EXTS)),
        _ => None,
    }
}

/// Where a browsed file or folder goes in the source.
pub(crate) fn set_location(src: &mut Source, path: String) {
    match src {
        Source::Text { location, .. } | Source::Json { location, .. } | Source::Xml { location, .. } | Source::Html { location, .. } => *location = path,
        Source::Pdf { location, index, .. } => {
            *location = path;
            *index = 0;
        }
        Source::Workbook { path: p, item, .. } => {
            *p = path;
            item.clear();
        }
        Source::Folder { path: p, .. } => *p = path,
        Source::Sqlite { path: p, sql } | Source::DbFile { path: p, table: sql } => {
            *p = path;
            sql.clear();
        }
        _ => {}
    }
}

/// The kind of values in a column, as the preview shows it.
fn col_kind(t: &DataTable, c: usize) -> &'static str {
    let mut kind = "";
    for r in &t.rows {
        let k = match r.get(c) {
            Some(Val::Num(_)) => "getdata.type.number",
            Some(Val::Text(s)) if !s.is_empty() => "getdata.type.text",
            Some(Val::Bool(_)) => "getdata.type.bool",
            Some(Val::Date(_)) => "getdata.type.date",
            _ => continue,
        };
        if kind.is_empty() {
            kind = k;
        } else if kind != k {
            return "getdata.type.any";
        }
    }
    if kind.is_empty() { "getdata.type.any" } else { kind }
}

/// The preview grid of a table.
pub(super) fn preview_grid(ui: &mut egui::Ui, t: &DataTable, height: f32) {
    let weak = ui.visuals().weak_text_color();
    egui::ScrollArea::both().id_salt("gd_preview").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("gd_preview_grid").striped(true).spacing([14.0, 4.0]).min_col_width(40.0).show(ui, |ui| {
            ui.label(RichText::new("").weak());
            for (c, name) in t.columns.iter().enumerate() {
                ui.vertical(|ui| {
                    ui.label(RichText::new(name).strong());
                    ui.label(RichText::new(tr(col_kind(t, c))).small().color(weak));
                });
            }
            ui.end_row();
            for (i, r) in t.rows.iter().enumerate() {
                ui.label(RichText::new((i + 1).to_string()).small().color(weak));
                for v in r {
                    match v {
                        Val::Null => ui.label(RichText::new("null").italics().small().color(weak)),
                        Val::Num(_) | Val::Date(_) => ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(v.to_text())).inner,
                        _ => {
                            let s = v.to_text();
                            let short: String = s.chars().take(60).collect();
                            let r = ui.label(if short.len() < s.len() { format!("{short}…") } else { short });
                            if s.chars().count() > 60 { r.on_hover_text(s) } else { r }
                        }
                    };
                }
                ui.end_row();
            }
        });
    });
}
