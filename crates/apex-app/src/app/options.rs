//! File > Options: interface, proofing, spreadsheet, the custom lists
//! used by fill series and sorting (Excel keeps them in Options > Advanced
//! > Edit Custom Lists), macros, signing, and network. What an
//! organization's policy fixes is greyed out (see `policy`).

use apex_sheet::analysis::lists;
use eframe::egui::{self, RichText};

use super::App;
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::policy::{self, guard};
use crate::widgets::{self, DialogResult};

pub(super) struct OptionsDlg {
    page: Page,
    classic: bool,
    show_toolbar: bool,
    mini_toolbar: bool,
    developer_tab: bool,
    dark_canvas: bool,
    contrast: bool,
    sharp_text: bool,
    scroll_speed: f32,
    /// The file types to open with ApexDesk (`default_apps::GROUPS`).
    default_groups: [bool; 4],
    /// What the last "use ApexDesk" did.
    default_note: Option<super::Notice>,
    /// The vertical ruler in the draft view of documents.
    vruler: bool,
    draft_vruler: bool,
    formula_bar: bool,
    dark: Option<bool>,
    lang: String,
    user_name: String,
    spell_lang: String,
    spell_auto: bool,
    status_stats: u8,
    /// Minutes between recovery copies (0: off).
    autosave: u32,
    lock_files: bool,
    gpu_high: bool,
    speech_voice: String,
    speech_rate: f32,
    lists: Vec<Vec<String>>,
    /// Index in `policy::MACRO_MODES`.
    macro_mode: usize,
    protected_view: bool,
    trusted: Vec<std::path::PathBuf>,
    /// Files whose macros may change files without asking (0: forgotten).
    file_grants: usize,
    publishers: Vec<String>,
    addins: Vec<std::path::PathBuf>,
    templates_folder: String,
    building_blocks_folder: String,
    library_folder: String,
    only_shared_templates: bool,
    save_sheet_format: String,
    save_doc_format: String,
    dictionaries_folder: String,
    renderer: String,
    get_data_off: bool,
    proxy: String,
    proxy_bypass: String,
    sign_in_sites: String,
    support_email: String,
    font_substitutes: String,
    cert_roots: String,
    tsa_url: String,
    /// The PKCS #11 module of smart cards and tokens (signing).
    pkcs11_module: String,
    /// 0 is NEW LIST, then the built-in lists, then the user's.
    selected: usize,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Proofing,
    Sheet,
    Lists,
    Macros,
    Signatures,
    Advanced,
}

const PAGES: [(Page, Icon, &str); 7] = [
    (Page::General, Icon::Options, "options.general"),
    (Page::Proofing, Icon::Spelling, "options.proofing"),
    (Page::Sheet, Icon::Sheet, "options.sheet"),
    (Page::Lists, Icon::SortAsc, "options.custom_lists"),
    (Page::Macros, Icon::Protect, "options.macros"),
    (Page::Signatures, Icon::Protect, "sign.title"),
    (Page::Advanced, Icon::Link, "options.advanced"),
];

const STAT_NAMES: [&str; 6] = ["stats.sum", "stats.average", "stats.count", "stats.numbers", "stats.min", "stats.max"];

impl OptionsDlg {
    fn user_index(&self) -> Option<usize> {
        self.selected.checked_sub(1 + lists::BUILTIN.len()).filter(|i| *i < self.lists.len())
    }
}

/// Settings lines: one `custom_list=` line per list, items tab separated.
pub(crate) fn encode(list: &[String]) -> String {
    list.iter().map(|s| s.replace(['\t', '\n', '\r'], " ")).collect::<Vec<_>>().join("\t")
}

pub(crate) fn decode(line: &str) -> Vec<String> {
    line.split('\t').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

impl App {
    pub(super) fn open_options(&mut self) {
        let st = &self.settings;
        self.options = Some(OptionsDlg {
            page: Page::General,
            classic: st.classic,
            show_toolbar: st.show_toolbar,
            mini_toolbar: st.mini_toolbar,
            developer_tab: st.developer_tab,
            dark_canvas: st.dark_canvas,
            contrast: st.contrast,
            sharp_text: st.sharp_text,
            scroll_speed: st.scroll_speed,
            default_groups: super::default_apps_ui::first_choice(),
            default_note: None,
            vruler: st.vruler,
            draft_vruler: st.draft_vruler,
            formula_bar: st.show_formula_bar,
            dark: st.dark,
            lang: st.lang.clone(),
            user_name: crate::doc_view::user_setting(),
            spell_lang: crate::spell::language(),
            spell_auto: crate::spell::auto(),
            status_stats: crate::sheet_view::status_stats(),
            autosave: st.autosave_minutes,
            lock_files: st.lock_files,
            gpu_high: st.gpu_high,
            speech_voice: crate::read_aloud::prefs().0,
            speech_rate: crate::read_aloud::prefs().1,
            lists: st.custom_lists.clone(),
            macro_mode: if !st.macros_enabled { 0 } else if st.macros_signed_only { 2 } else { 1 },
            protected_view: st.protected_view,
            trusted: st.trusted_locations.clone(),
            file_grants: st.macro_file_grants.len(),
            publishers: st.trusted_publishers.clone(),
            addins: st.addins.clone(),
            templates_folder: st.templates_folder.clone(),
            building_blocks_folder: st.building_blocks_folder.clone(),
            library_folder: st.library_folder.clone(),
            only_shared_templates: st.only_shared_templates,
            save_sheet_format: st.save_sheet_format.clone(),
            save_doc_format: st.save_doc_format.clone(),
            dictionaries_folder: st.dictionaries_folder.clone(),
            renderer: st.renderer.clone(),
            get_data_off: st.get_data_off,
            proxy: st.proxy.clone(),
            proxy_bypass: st.proxy_bypass.clone(),
            sign_in_sites: st.sign_in_sites.clone(),
            support_email: st.support_email.clone(),
            font_substitutes: st.font_substitutes.clone(),
            cert_roots: st.cert_roots.clone(),
            tsa_url: st.tsa_url.clone(),
            pkcs11_module: st.pkcs11_module.clone(),
            selected: 0,
            text: String::new(),
        });
    }

    /// Options, on the page of macro security.
    pub(super) fn open_macro_options(&mut self) {
        self.open_options();
        if let Some(d) = &mut self.options {
            d.page = Page::Macros;
        }
    }

    pub(super) fn options_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.options else { return };
        let pages: Vec<(Icon, &'static str)> = PAGES.iter().map(|(_, icon, key)| (*icon, *key)).collect();
        let mut page = PAGES.iter().position(|(p, ..)| *p == d.page).unwrap_or(0);
        let res = widgets::nav_dialog(ctx, "options", tr("options.title"), &pages, &mut page, egui::vec2(560.0, 400.0), |ui, i| match PAGES[i].0 {
            Page::General => general_page(ui, d),
            Page::Proofing => proofing_page(ui, d),
            Page::Sheet => sheet_page(ui, d),
            Page::Lists => lists_page(ui, d),
            Page::Macros => macros_page(ui, d),
            Page::Signatures => {
                super::signatures_ui::module_option(ui, &mut d.pkcs11_module, |app, p| {
                    if let Some(d) = &mut app.options {
                        d.pkcs11_module = p;
                    }
                });
                signing_page(ui, d);
            }
            Page::Advanced => advanced_page(ui, d),
        });
        d.page = PAGES[page].0;
        match res {
            DialogResult::None => {}
            DialogResult::Cancel => self.options = None,
            DialogResult::Ok => {
                if let Some(d) = self.options.take() {
                    self.apply_options(ctx, d);
                }
            }
        }
    }

    fn apply_options(&mut self, ctx: &egui::Context, d: OptionsDlg) {
        let st = &mut self.settings;
        st.classic = d.classic;
        st.show_toolbar = d.show_toolbar;
        st.mini_toolbar = d.mini_toolbar;
        st.developer_tab = d.developer_tab;
        crate::settings::set_mini_toolbar(d.mini_toolbar);
        st.show_formula_bar = d.formula_bar;
        st.dark_canvas = d.dark_canvas;
        if st.contrast != d.contrast || st.sharp_text != d.sharp_text {
            st.contrast = d.contrast;
            st.sharp_text = d.sharp_text;
            crate::theme::setup(ctx, d.contrast, d.sharp_text);
        }
        st.scroll_speed = d.scroll_speed;
        st.vruler = d.vruler;
        crate::settings::set_vruler(d.vruler);
        st.draft_vruler = d.draft_vruler;
        crate::settings::set_draft_vruler(d.draft_vruler);
        if st.dark != d.dark {
            st.dark = d.dark;
            super::apply_theme(ctx, d.dark);
        }
        if st.lang != d.lang {
            // None chosen: the one of the system, if translated.
            if d.lang.is_empty() {
                if !crate::i18n::env_locale().is_some_and(|l| crate::i18n::set(&l)) {
                    crate::i18n::set(crate::i18n::BASE);
                }
            } else {
                crate::i18n::set(&d.lang);
            }
            st.lang = d.lang;
        }
        crate::doc_view::set_user_name(&d.user_name, false);
        st.user_name = crate::doc_view::user_setting();
        if !d.spell_lang.is_empty() && d.spell_lang != crate::spell::language() {
            crate::spell::set_language(ctx, &d.spell_lang);
        }
        crate::spell::set_auto(d.spell_auto);
        st.spell_lang = crate::spell::language();
        st.spell_auto = d.spell_auto;
        crate::sheet_view::set_status_stats(d.status_stats);
        st.status_stats = d.status_stats;
        st.autosave_minutes = d.autosave;
        st.lock_files = d.lock_files;
        st.gpu_high = d.gpu_high;
        crate::read_aloud::set_prefs(&d.speech_voice, d.speech_rate);
        (st.speech_voice, st.speech_rate) = crate::read_aloud::prefs();
        st.custom_lists = d.lists.into_iter().filter(|l| !l.is_empty()).collect();
        crate::sheet_view::set_custom_lists(st.custom_lists.clone());
        st.macros_enabled = d.macro_mode > 0;
        st.macros_signed_only = d.macro_mode == 2;
        st.protected_view = d.protected_view;
        st.trusted_locations = d.trusted;
        if d.file_grants == 0 {
            st.macro_file_grants.clear();
        }
        st.trusted_publishers = d.publishers;
        st.addins = d.addins;
        st.pkcs11_module = d.pkcs11_module.trim().to_string();
        st.templates_folder = d.templates_folder.trim().to_string();
        st.building_blocks_folder = d.building_blocks_folder.trim().to_string();
        st.library_folder = d.library_folder.trim().to_string();
        st.only_shared_templates = d.only_shared_templates;
        st.save_sheet_format = d.save_sheet_format;
        st.save_doc_format = d.save_doc_format;
        st.dictionaries_folder = d.dictionaries_folder.trim().to_string();
        st.renderer = d.renderer;
        st.get_data_off = d.get_data_off;
        st.proxy = d.proxy.trim().to_string();
        st.proxy_bypass = d.proxy_bypass.trim().to_string();
        st.sign_in_sites = d.sign_in_sites.trim().to_string();
        st.support_email = d.support_email.trim().to_string();
        st.font_substitutes = d.font_substitutes.trim().to_string();
        st.cert_roots = d.cert_roots.trim().to_string();
        st.tsa_url = d.tsa_url.trim().to_string();
        // What policies fix stays as they say.
        st.apply_policy(policy::current());
        st.apply_globals();
        st.save();
    }
}

fn section(ui: &mut egui::Ui, key: &'static str) {
    widgets::section(ui, tr(key));
}

fn general_page(ui: &mut egui::Ui, d: &mut OptionsDlg) {
    section(ui, "options.ui");
    ui.label(tr("options.interface"));
    ui.indent("iface", |ui| {
        ui.radio_value(&mut d.classic, false, tr("options.toolstrip"));
        ui.radio_value(&mut d.classic, true, tr("options.classic"));
    });
    ui.checkbox(&mut d.show_toolbar, tr(if d.classic { "options.show_toolbar" } else { "options.show_toolstrip" }));
    ui.checkbox(&mut d.formula_bar, tr("view.formula_bar"));
    ui.checkbox(&mut d.mini_toolbar, tr("options.mini_toolbar"));
    ui.checkbox(&mut d.developer_tab, tr("options.developer_tab"));
    ui.checkbox(&mut d.vruler, tr("options.vruler"));
    ui.indent("draft_vruler", |ui| ui.add_enabled(d.vruler, egui::Checkbox::new(&mut d.draft_vruler, tr("options.draft_vruler"))));
    ui.add_space(4.0);
    egui::Grid::new("opt_general").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        // Short labels: a narrow page must not break them letter by letter.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        ui.label(tr("menu.theme"));
        let name = |t: Option<bool>| tr(match t {
            None => "menu.theme_system",
            Some(false) => "menu.theme_light",
            Some(true) => "menu.theme_dark",
        });
        widgets::combo_name(
            egui::ComboBox::from_id_salt("opt_theme").selected_text(name(d.dark)).width(200.0).show_ui(ui, |ui| {
                for t in [None, Some(false), Some(true)] {
                    ui.selectable_value(&mut d.dark, t, name(t));
                }
            }),
            tr("menu.theme"),
        );
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut d.dark_canvas, tr("dw.title")).on_hover_text(tr("dw.tip"));
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut d.contrast, tr("options.contrast")).on_hover_text(tr("options.contrast_tip"));
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut d.sharp_text, tr("options.sharp_text")).on_hover_text(tr("options.sharp_text_tip"));
        ui.end_row();
        ui.label(tr("options.scroll_speed"));
        let (lo, hi) = crate::settings::SCROLL_SPEED;
        ui.add(egui::Slider::new(&mut d.scroll_speed, lo..=hi).step_by(0.1).custom_formatter(|v, _| format!("{v:.1}×"))).on_hover_text(tr("options.scroll_speed_tip"));
        ui.end_row();
        ui.label(tr("menu.language"));
        let cur = crate::i18n::available().find(|l| l.code == d.lang).map_or(tr("options.lang_system"), |l| l.name);
        ui.horizontal(|ui| {
            guard(ui, "UILanguage", |ui| {
                widgets::combo_name(
                    egui::ComboBox::from_id_salt("opt_lang").selected_text(cur).width(200.0).show_ui(ui, |ui| {
                        if ui.selectable_label(d.lang.is_empty(), tr("options.lang_system")).clicked() {
                            d.lang.clear();
                        }
                        for l in crate::i18n::available() {
                            if ui.selectable_label(d.lang == l.code, l.name).clicked() {
                                d.lang = l.code.to_string();
                            }
                        }
                    }),
                    tr("menu.language"),
                );
            })
        });
        ui.end_row();
        ui.label(tr("options.renderer"));
        ui.horizontal(|ui| {
            guard(ui, "Renderer", |ui| {
                let name = |r: &str| if r == "wgpu" { "wgpu" } else { "OpenGL" };
                widgets::combo_name(
                    egui::ComboBox::from_id_salt("opt_renderer").selected_text(name(&d.renderer)).width(200.0).show_ui(ui, |ui| {
                        for r in ["glow", "wgpu"] {
                            if ui.selectable_label(name(&d.renderer) == name(r), name(r)).clicked() {
                                d.renderer = r.to_string();
                            }
                        }
                    }),
                    tr("options.renderer"),
                );
            })
        });
        ui.end_row();
    });
    ui.checkbox(&mut d.gpu_high, tr("options.gpu_high"));
    ui.label(RichText::new(tr("options.gpu_high_hint")).weak());
    section(ui, "options.personalize");
    ui.label(tr("comment.user_name_hint"));
    ui.add(egui::TextEdit::singleline(&mut d.user_name).desired_width(280.0));
    section(ui, "options.save");
    egui::Grid::new("opt_formats").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        ui.label(tr("options.save_sheet_format"));
        ui.horizontal(|ui| guard(ui, "DefaultSheetFormat", |ui| format_combo(ui, "opt_fmt_sheet", tr("options.save_sheet_format"), &mut d.save_sheet_format, policy::SHEET_FORMATS)));
        ui.end_row();
        ui.label(tr("options.save_doc_format"));
        ui.horizontal(|ui| guard(ui, "DefaultDocFormat", |ui| format_combo(ui, "opt_fmt_doc", tr("options.save_doc_format"), &mut d.save_doc_format, policy::DOC_FORMATS)));
        ui.end_row();
    });
    ui.horizontal(|ui| {
        guard(ui, "AutosaveMinutes", |ui| {
            let mut on = d.autosave > 0;
            if ui.checkbox(&mut on, tr("options.autosave")).changed() {
                d.autosave = if on { 10 } else { 0 };
            }
            let mut m = d.autosave.max(1);
            if ui.add_enabled(on, egui::DragValue::new(&mut m).range(1..=120)).changed() {
                d.autosave = m;
            }
            ui.label(tr("options.minutes"));
        })
    });
    ui.label(RichText::new(tr("options.autosave_hint")).weak());
    ui.horizontal(|ui| guard(ui, "LockFiles", |ui| ui.checkbox(&mut d.lock_files, tr("options.lock_files"))));
    ui.label(RichText::new(tr("options.lock_files_hint")).weak());
    ui.label(tr("policy.TemplatesFolder"));
    folder_row(ui, "TemplatesFolder", &mut d.templates_folder, |d| &mut d.templates_folder);
    ui.label(tr("policy.BuildingBlocksFolder"));
    folder_row(ui, "BuildingBlocksFolder", &mut d.building_blocks_folder, |d| &mut d.building_blocks_folder);
    ui.label(tr("policy.LibraryFolder"));
    folder_row(ui, "LibraryFolder", &mut d.library_folder, |d| &mut d.library_folder);
    ui.horizontal(|ui| guard(ui, "OnlySharedTemplates", |ui| ui.checkbox(&mut d.only_shared_templates, tr("policy.OnlySharedTemplates"))));

    section(ui, "default_apps.section");
    if crate::portable::enabled() || cfg!(target_os = "android") {
        ui.label(RichText::new(tr("default_apps.portable")).weak());
    } else {
        ui.label(tr("default_apps.intro"));
        super::default_apps_ui::group_boxes(ui, &mut d.default_groups);
        if ui.add_enabled(d.default_groups.contains(&true), egui::Button::new(tr("default_apps.button"))).clicked() {
            d.default_note = Some(super::default_apps_ui::make_default(d.default_groups));
        }
        if let Some(n) = &d.default_note {
            ui.label(if n.error { RichText::new(&n.text).color(ui.visuals().error_fg_color) } else { RichText::new(&n.text).weak() });
        }
    }

    section(ui, "speak.button");
    crate::speech::start(ui.ctx());
    egui::Grid::new("opt_speech").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        ui.label(tr("speak.voice"));
        ui.vertical(|ui| crate::read_aloud::voice_menu(ui, &mut d.speech_voice));
        ui.end_row();
        ui.label(tr("speak.speed"));
        ui.add(egui::Slider::new(&mut d.speech_rate, 0.5..=2.0).step_by(0.05).custom_formatter(|v, _| format!("{v:.2}×")));
        ui.end_row();
    });
}

fn proofing_page(ui: &mut egui::Ui, d: &mut OptionsDlg) {
    section(ui, "options.spelling");
    ui.horizontal(|ui| {
        ui.label(tr("spell.language"));
        if policy::locked("SpellingLanguage") {
            ui.label(crate::spell::language_name(&d.spell_lang));
            policy::note(ui, "SpellingLanguage");
            return;
        }
        let langs: Vec<&str> = crate::spell::languages().collect();
        if langs.is_empty() {
            ui.label(RichText::new(tr("spell.none_installed")).weak());
        } else {
            let text = if d.spell_lang.is_empty() { String::new() } else { crate::spell::language_name(&d.spell_lang) };
            widgets::combo_name(
                egui::ComboBox::from_id_salt("opt_spell").selected_text(text).width(200.0).show_ui(ui, |ui| {
                    for l in langs {
                        if ui.selectable_label(d.spell_lang == l, format!("{} - {l}", crate::spell::language_name(l))).clicked() {
                            d.spell_lang = l.to_string();
                        }
                    }
                }),
                tr("spell.language"),
            );
        }
    });
    ui.checkbox(&mut d.spell_auto, tr("spell.auto"));
    ui.label(tr("policy.DictionariesFolder"));
    folder_row(ui, "DictionariesFolder", &mut d.dictionaries_folder, |d| &mut d.dictionaries_folder);
    ui.label(RichText::new(tr("options.restart_hint")).small().weak());
    section(ui, "options.autocorrect");
    ui.label(RichText::new(tr("options.autocorrect_hint")).weak());
    if ui.button(format!("{}…", tr("ac.title"))).clicked() {
        crate::autocorrect::open_dialog();
    }
}

fn sheet_page(ui: &mut egui::Ui, d: &mut OptionsDlg) {
    section(ui, "options.status_bar");
    ui.label(RichText::new(tr("options.status_hint")).weak());
    for (i, key) in STAT_NAMES.iter().enumerate() {
        let mut on = d.status_stats & (1 << i) != 0;
        if ui.checkbox(&mut on, tr(key)).changed() {
            d.status_stats ^= 1 << i;
        }
    }
}

fn macros_page(ui: &mut egui::Ui, d: &mut OptionsDlg) {
    section(ui, "options.macros");
    guard(ui, "MacroSecurity", |ui| {
        for (i, key) in ["policy.MacroSecurity.disabled", "policy.MacroSecurity.ask", "policy.MacroSecurity.signed"].into_iter().enumerate() {
            ui.radio_value(&mut d.macro_mode, i, tr(key));
        }
    });
    ui.label(RichText::new(tr("options.macros_hint")).weak());
    section(ui, "protected.title");
    guard(ui, "ProtectedView", |ui| {
        ui.checkbox(&mut d.protected_view, tr("options.protected_view"));
    });
    ui.label(RichText::new(tr("options.protected_hint")).weak());
    addins_section(ui, d);
    publishers_section(ui, d);
    if d.file_grants > 0 {
        section(ui, "options.file_grants");
        ui.label(RichText::new(tr("options.file_grants_hint")).weak());
        ui.horizontal(|ui| {
            ui.label(trf("options.file_grants_count", &[("n", &d.file_grants)]));
            if ui.button(tr("options.file_grants_forget")).clicked() {
                d.file_grants = 0;
            }
        });
    }
    section(ui, "options.trusted");
    ui.label(RichText::new(tr("options.trusted_hint")).weak());
    if policy::locked("TrustedLocations") {
        policy::note(ui, "TrustedLocations");
        for p in &d.trusted {
            ui.label(p.to_string_lossy());
        }
        return;
    }
    let mut remove = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt("opt_trusted").max_height(140.0).show(ui, |ui| {
            if d.trusted.is_empty() {
                ui.label(RichText::new(tr("options.trusted_none")).weak());
            }
            for (i, p) in d.trusted.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("×").on_hover_text(tr("options.delete")).clicked() {
                        remove = Some(i);
                    }
                    ui.label(p.to_string_lossy());
                });
            }
        });
    });
    if let Some(i) = remove {
        d.trusted.remove(i);
    }
    if ui.button(format!("{}…", tr("options.trusted_add"))).clicked() {
        crate::file_dialog::FileDialog::new().set_title(tr("options.trusted_add")).pick_folder(|app, dir| {
            if let Some(d) = &mut app.options
                && !d.trusted.contains(&dir)
            {
                d.trusted.push(dir);
            }
        });
    }
}

/// The publishers whose signed macros run under "signed only".
fn publishers_section(ui: &mut egui::Ui, d: &mut OptionsDlg) {
    section(ui, "options.publishers");
    ui.label(RichText::new(tr("options.publishers_hint")).weak());
    let locked = policy::locked("TrustedPublishers");
    if locked {
        policy::note(ui, "TrustedPublishers");
    }
    let mut remove = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt("opt_publishers").max_height(120.0).show(ui, |ui| {
            if d.publishers.is_empty() {
                ui.label(RichText::new(tr("options.publishers_none")).weak());
            }
            for (i, p) in d.publishers.iter().enumerate() {
                let (sha, name) = p.split_once(' ').unwrap_or((p, ""));
                ui.horizontal(|ui| {
                    if !locked && ui.small_button("×").on_hover_text(tr("options.delete")).clicked() {
                        remove = Some(i);
                    }
                    let shown = if name.trim().is_empty() { sha.to_string() } else { apex_sign::x509::one_line(name) };
                    ui.label(shown).on_hover_text(format!("SHA-256: {sha}"));
                });
            }
        });
    });
    if let Some(i) = remove {
        d.publishers.remove(i);
    }
}

/// The installed add-ins and where the personal macro workbook is.
fn addins_section(ui: &mut egui::Ui, d: &mut OptionsDlg) {
    section(ui, "macros.addins");
    ui.label(RichText::new(tr("options.addins_hint")).weak());
    if let Some(p) = super::macro_run::personal_path() {
        ui.label(RichText::new(format!("{}: {}", tr("macros.personal"), p.display())).weak());
    }
    let mut remove = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt("opt_addins").max_height(100.0).show(ui, |ui| {
            if d.addins.is_empty() {
                ui.label(RichText::new(tr("options.addins_none")).weak());
            }
            for (i, p) in d.addins.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("×").on_hover_text(tr("options.delete")).clicked() {
                        remove = Some(i);
                    }
                    ui.label(p.to_string_lossy());
                });
            }
        });
    });
    if let Some(i) = remove {
        d.addins.remove(i);
    }
    if ui.button(format!("{}…", tr("options.addins_add"))).clicked() {
        crate::file_dialog::FileDialog::new().set_title(tr("options.addins_add")).add_filter("Excel", &["xlam", "xlsm", "xla", "xls", "xlsb"]).pick_file(|app, file| {
            if let Some(d) = &mut app.options
                && !d.addins.contains(&file)
            {
                d.addins.push(file);
            }
        });
    }
}

/// A combo of save formats, named `label`; "" is the first.
fn format_combo(ui: &mut egui::Ui, id: &str, label: &str, value: &mut String, formats: &[&str]) {
    let shown = if value.is_empty() { formats[0] } else { value.as_str() }.to_string();
    widgets::combo_name(
        egui::ComboBox::from_id_salt(id).selected_text(&shown).width(120.0).show_ui(ui, |ui| {
            for f in formats {
                if ui.selectable_label(shown == *f, *f).clicked() {
                    *value = f.to_string();
                }
            }
        }),
        label,
    );
}

/// A folder: its path and a Browse button, greyed out under a policy.
/// `field` finds the value again for the Browse dialog's answer.
fn folder_row(ui: &mut egui::Ui, name: &str, value: &mut String, field: fn(&mut OptionsDlg) -> &mut String) {
    ui.horizontal(|ui| {
        guard(ui, name, |ui| {
            ui.add(egui::TextEdit::singleline(value).desired_width(300.0));
            if ui.button(format!("{}…", tr("sign.browse"))).clicked() {
                crate::file_dialog::FileDialog::new().pick_folder(move |app, dir| {
                    if let Some(d) = &mut app.options {
                        *field(d) = dir.display().to_string();
                    }
                });
            }
        })
    });
}

fn signing_page(ui: &mut egui::Ui, d: &mut OptionsDlg) {
    section(ui, "options.cert_section");
    ui.label(tr("policy.CertificateRoots"));
    folder_row(ui, "CertificateRoots", &mut d.cert_roots, |d| &mut d.cert_roots);
    ui.label(tr("sign.tsa"));
    ui.horizontal(|ui| guard(ui, "TimestampURL", |ui| ui.add(egui::TextEdit::singleline(&mut d.tsa_url).desired_width(300.0))));
}

fn advanced_page(ui: &mut egui::Ui, d: &mut OptionsDlg) {
    section(ui, "options.network");
    ui.horizontal(|ui| guard(ui, "DisableGetData", |ui| ui.checkbox(&mut d.get_data_off, tr("policy.DisableGetData"))));
    ui.label(tr("policy.Proxy"));
    ui.horizontal(|ui| guard(ui, "Proxy", |ui| ui.add(egui::TextEdit::singleline(&mut d.proxy).hint_text(tr("options.proxy_hint")).desired_width(300.0))));
    ui.label(tr("policy.ProxyBypass"));
    ui.horizontal(|ui| guard(ui, "ProxyBypass", |ui| ui.add(egui::TextEdit::singleline(&mut d.proxy_bypass).hint_text(".corp.example.com, intranet").desired_width(300.0))));
    ui.label(tr("policy.SignInSites"));
    ui.horizontal(|ui| guard(ui, "SignInSites", |ui| ui.add(egui::TextEdit::singleline(&mut d.sign_in_sites).hint_text(".corp.example.com").desired_width(300.0))));
    ui.label(RichText::new(tr("options.sign_in_hint")).small().weak());
    section(ui, "options.diagnostics");
    ui.label(tr("policy.SupportEmail"));
    ui.horizontal(|ui| guard(ui, "SupportEmail", |ui| ui.add(egui::TextEdit::singleline(&mut d.support_email).hint_text("helpdesk@example.com").desired_width(300.0))));
    section(ui, "options.fonts");
    ui.label(tr("policy.FontSubstitutes"));
    ui.horizontal(|ui| guard(ui, "FontSubstitutes", |ui| ui.add(egui::TextEdit::singleline(&mut d.font_substitutes).hint_text("Calibri=Carlito; Arial=Liberation Sans").desired_width(300.0))));
}

fn lists_page(ui: &mut egui::Ui, d: &mut OptionsDlg) {

    section(ui, "options.custom_lists");
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.label(tr("options.lists"));
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(240.0);
                egui::ScrollArea::vertical().id_salt("opt_lists").max_height(220.0).show(ui, |ui| {
                    let mut pick = None;
                    if ui.add(egui::Button::selectable(d.selected == 0, tr("options.new_list")).truncate()).clicked() {
                        pick = Some((0, String::new()));
                    }
                    for (i, l) in lists::BUILTIN.iter().enumerate() {
                        let label = RichText::new(l.join(", ")).weak();
                        if ui.add(egui::Button::selectable(d.selected == i + 1, label).truncate()).clicked() {
                            pick = Some((i + 1, l.join("\n")));
                        }
                    }
                    for (i, l) in d.lists.iter().enumerate() {
                        let k = 1 + lists::BUILTIN.len() + i;
                        if ui.add(egui::Button::selectable(d.selected == k, l.join(", ")).truncate()).clicked() {
                            pick = Some((k, l.join("\n")));
                        }
                    }
                    if let Some((k, t)) = pick {
                        d.selected = k;
                        d.text = t;
                    }
                });
            });
        });
        ui.vertical(|ui| {
            ui.label(tr("options.entries"));
            let builtin = d.selected >= 1 && d.selected <= lists::BUILTIN.len();
            let id = egui::Id::new("opt_entries");
            ui.data_mut(|m| m.insert_temp(id.with("multiline"), true));
            ui.add_enabled(!builtin, egui::TextEdit::multiline(&mut d.text).id(id).desired_width(200.0).desired_rows(11));
            ui.horizontal(|ui| {
                let label = if d.user_index().is_some() { tr("options.update") } else { tr("options.add") };
                if ui.add_enabled(!builtin && !lists::parse_list(&d.text).is_empty(), egui::Button::new(label)).clicked() {
                    let l = lists::parse_list(&d.text);
                    match d.user_index() {
                        Some(i) => d.lists[i] = l,
                        None => {
                            d.lists.push(l);
                            d.selected = lists::BUILTIN.len() + d.lists.len();
                        }
                    }
                }
                if ui.add_enabled(d.user_index().is_some(), egui::Button::new(tr("options.delete"))).clicked() {
                    if let Some(i) = d.user_index() {
                        d.lists.remove(i);
                        d.selected = 0;
                        d.text.clear();
                    }
                }
            });
        });
    });
    ui.add_space(4.0);
    ui.label(RichText::new(tr("options.lists_hint")).small().weak());

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_lines() {
        let l = vec!["Low".to_string(), "Mid, high".to_string()];
        assert_eq!(decode(&encode(&l)), l);
        assert!(decode("").is_empty());
    }
}
