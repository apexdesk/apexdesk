//! Spell checking shared by every tab: the dictionary of the chosen
//! language (loaded in the background), the personal word list and the
//! words ignored for this session, plus the Spelling dialog (F7) both
//! programs use.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use apex_spell::{Dictionary, Personal};
use eframe::egui;

use crate::i18n::{tr, trf};
use crate::widgets::FitWindow as _;

mod install;

struct State {
    /// Chosen dictionary ("es_ES"); empty when none is installed.
    lang: String,
    dict: Option<Arc<Dictionary>>,
    loading: bool,
    personal: Personal,
    ignored: HashSet<String>,
    /// Check as you type (red underlines).
    auto: bool,
    /// Bumped whenever a word's verdict may change.
    generation: u64,
    /// Word -> correct, for the current generation.
    cache: HashMap<String, bool>,
    /// The language was chosen (a setting to save).
    lang_changed: bool,
    /// The automatic check was turned on or off (a setting to save).
    auto_changed: bool,
    /// Dictionaries of other languages, for text marked in them.
    others: HashMap<String, Other>,
    /// To repaint when one of them has loaded.
    ctx: Option<egui::Context>,
}

enum Other {
    Loading,
    Ready(Arc<Dictionary>),
}

/// Which dictionary checks text in a language.
enum Pick {
    Main,
    Other(String),
    /// None installed: the text isn't checked.
    Skip,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        lang: String::new(),
        dict: None,
        loading: false,
        personal: Personal::new(),
        ignored: HashSet::new(),
        auto: true,
        generation: 1,
        cache: HashMap::new(),
        lang_changed: false,
        auto_changed: false,
        others: HashMap::new(),
        ctx: None,
    })
});

/// The dictionaries found, looked for again after one is installed.
static INSTALLED: Mutex<Option<Arc<Vec<(String, PathBuf, PathBuf)>>>> = Mutex::new(None);

fn installed() -> Arc<Vec<(String, PathBuf, PathBuf)>> {
    INSTALLED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(|| {
            // Those the user installed, on every system.
            if let Some(d) = install::user_dir() {
                apex_spell::add_search_dir(d);
            }
            Arc::new(apex_spell::installed())
        })
        .clone()
}

/// Looks for the installed dictionaries again.
fn rescan() {
    *INSTALLED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn personal_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("personal.dic"))
}

/// Installed dictionaries (language codes).
pub fn languages() -> Vec<String> {
    installed().iter().map(|(l, _, _)| l.clone()).collect()
}

/// A language code as people read it ("es_ES" -> "Español (ES)").
pub fn language_name(code: &str) -> String {
    let (l, region) = code.split_once(['_', '-']).unwrap_or((code, ""));
    let name = match l {
        "en" => "English",
        "es" => "Español",
        "ca" => "Català",
        "gl" => "Galego",
        "eu" => "Euskara",
        "pt" => "Português",
        "fr" => "Français",
        "de" => "Deutsch",
        "it" => "Italiano",
        "nl" => "Nederlands",
        _ => l,
    };
    if region.is_empty() { name.to_string() } else { format!("{name} ({region})") }
}

/// The installed dictionary that best matches a locale like "es_ES.UTF-8".
fn best_match(locale: &str) -> Option<String> {
    let code = locale.split(['.', '@']).next().unwrap_or("").replace('-', "_");
    if code.is_empty() {
        return None;
    }
    let lang = code.split('_').next().unwrap_or("");
    let langs = languages();
    let found = langs.iter().find(|l| l.eq_ignore_ascii_case(&code)).or_else(|| {
        // es_ES before es_AR when only the language matches.
        let main = format!("{lang}_{}", lang.to_ascii_uppercase());
        langs.iter().find(|l| **l == main).or_else(|| langs.iter().find(|l| l.split('_').next() == Some(lang)))
    });
    found.cloned()
}

/// Chooses the dictionary (the saved one, else the system locale's, else
/// the interface language's, else English) and starts loading it.
pub fn init(ctx: &egui::Context, saved: &str, auto: bool) {
    let lang = chosen(saved, system_locale().as_deref());
    {
        let mut s = state();
        s.ctx = Some(ctx.clone());
        s.auto = auto;
        if let Some(p) = personal_path() {
            s.personal = Personal::load(&p).unwrap_or_default();
        }
    }
    load(ctx, lang);
}

/// The system's locale ("es_ES.UTF-8").
fn system_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|v| std::env::var(v).ok()).find(|v| !v.is_empty() && v != "C" && v != "POSIX").or_else(|| apex_i18n::env_locale().filter(|v| v != "C" && v != "POSIX"))
}

/// Dictionaries were just installed (`langs`): they're listed and used at
/// once; with none until now, the one that fits best becomes the
/// language.
fn installed_now(langs: &[String]) {
    rescan();
    let ctx = {
        let mut s = state();
        for l in langs {
            s.others.remove(l);
        }
        forget(&mut s);
        s.ctx.clone()
    };
    let Some(ctx) = ctx else { return };
    let cur = language();
    if langs.contains(&cur) {
        // A new version of the one in use.
        load(&ctx, cur);
    } else if !ready() && !loading() {
        let lang = chosen(&cur, system_locale().as_deref());
        state().lang_changed = true;
        load(&ctx, lang);
    }
}

/// The dictionary to use: the saved one, else the system locale's, else
/// the interface language's, else English, else the first.
fn chosen(saved: &str, sys: Option<&str>) -> String {
    let langs = languages();
    langs
        .iter()
        .find(|l| *l == saved)
        .cloned()
        .or_else(|| sys.and_then(best_match))
        .or_else(|| best_match(crate::i18n::current()))
        .or_else(|| best_match("en_US"))
        .or_else(|| langs.first().cloned())
        .unwrap_or_default()
}

fn load(ctx: &egui::Context, lang: String) {
    let Some((_, aff, dic)) = installed().iter().find(|(l, _, _)| *l == lang).cloned() else {
        let mut s = state();
        s.lang = lang;
        s.dict = None;
        return;
    };
    {
        let mut s = state();
        s.lang = lang.clone();
        s.loading = true;
    }
    let ctx = ctx.clone();
    apex_fs::thread::spawn(move || {
        let dict = Dictionary::load(&aff, &dic).ok().map(Arc::new);
        let mut s = state();
        // A newer choice may have replaced this one while it loaded.
        if s.lang == lang {
            s.dict = dict;
            s.loading = false;
            s.generation += 1;
            s.cache.clear();
        }
        drop(s);
        ctx.request_repaint();
    });
}

pub fn language() -> String {
    state().lang.clone()
}

pub fn set_language(ctx: &egui::Context, code: &str) {
    if state().lang == code {
        return;
    }
    {
        let mut s = state();
        s.dict = None;
        s.generation += 1;
        s.cache.clear();
        s.lang_changed = true;
    }
    load(ctx, code.to_string());
}

pub fn loading() -> bool {
    state().loading
}

/// The dictionary of the spelling language, once loaded.
pub fn main_dict() -> Option<Arc<Dictionary>> {
    state().dict.clone()
}

/// Whether a dictionary is loaded.
pub fn ready() -> bool {
    state().dict.is_some()
}

/// Check spelling as you type.
pub fn auto() -> bool {
    state().auto
}

pub fn set_auto(on: bool) {
    let mut s = state();
    if s.auto != on {
        s.auto = on;
        s.generation += 1;
        s.auto_changed = true;
    }
}

/// Changes over which cached results of [`misspelled`] are stale.
pub fn generation() -> u64 {
    let s = state();
    s.generation
}

/// Whether the language and the automatic check (in that order) were
/// changed since the last call: the app saves just those with the
/// settings (the language found at start isn't a choice to keep).
pub fn take_changed() -> (bool, bool) {
    let mut s = state();
    (std::mem::take(&mut s.lang_changed), std::mem::take(&mut s.auto_changed))
}

fn word_ok(s: &mut State, word: &str) -> bool {
    if let Some(&ok) = s.cache.get(word) {
        return ok;
    }
    let ok = s.ignored.contains(word) || s.personal.contains(word) || s.dict.as_ref().is_none_or(|d| d.check(word));
    if s.cache.len() > 50_000 {
        s.cache.clear();
    }
    s.cache.insert(word.to_string(), ok);
    ok
}

/// Byte ranges of the misspelled words of a text (none while no dictionary
/// is loaded).
pub fn misspelled(text: &str) -> Vec<Range<usize>> {
    misspelled_in(text, &[])
}

/// The dictionary for text in `lang` ("en-US").
fn pick(s: &State, lang: &str) -> Pick {
    let code = lang.replace('-', "_");
    let base = |c: &str| c.split('_').next().unwrap_or("").to_ascii_lowercase();
    if code.eq_ignore_ascii_case(&s.lang) {
        return Pick::Main;
    }
    let installed = installed();
    if let Some((l, _, _)) = installed.iter().find(|(l, _, _)| l.eq_ignore_ascii_case(&code)) {
        return Pick::Other(l.clone());
    }
    if base(&code) == base(&s.lang) {
        return Pick::Main;
    }
    match installed.iter().find(|(l, _, _)| base(l) == base(&code)) {
        Some((l, _, _)) => Pick::Other(l.clone()),
        None => Pick::Skip,
    }
}

/// A dictionary of another language: loads it the first time.
fn other(s: &mut State, code: &str) -> Option<Arc<Dictionary>> {
    match s.others.get(code) {
        Some(Other::Ready(d)) => return Some(d.clone()),
        Some(Other::Loading) => return None,
        None => {}
    }
    let (_, aff, dic) = installed().iter().find(|(l, _, _)| l == code)?.clone();
    s.others.insert(code.to_string(), Other::Loading);
    let code = code.to_string();
    let ctx = s.ctx.clone();
    apex_fs::thread::spawn(move || {
        let d = Dictionary::load(&aff, &dic).ok().map(Arc::new);
        let mut s = state();
        match d {
            Some(d) => {
                s.others.insert(code, Other::Ready(d));
            }
            None => {
                s.others.remove(&code);
            }
        }
        s.generation += 1;
        drop(s);
        if let Some(c) = ctx {
            c.request_repaint();
        }
    });
    None
}

/// Misspelled words of a text whose parts may be in other languages or
/// not to be checked: `spans` are (byte range, language, no proofing);
/// text outside them is in the main language.
pub fn misspelled_in(text: &str, spans: &[(Range<usize>, Option<Arc<str>>, bool)]) -> Vec<Range<usize>> {
    let mut s = state();
    let mut out = Vec::new();
    for (r, w) in apex_spell::words(text).skip_all_caps(true) {
        let span = spans.iter().find(|x| x.0.start <= r.start && r.start < x.0.end);
        if span.is_some_and(|x| x.2) {
            continue;
        }
        let which = match span.and_then(|x| x.1.as_deref()) {
            None => Pick::Main,
            Some(l) => pick(&s, l),
        };
        let bad = match which {
            Pick::Main => s.dict.is_some() && !word_ok(&mut s, w),
            Pick::Skip => false,
            Pick::Other(code) => {
                let key = format!("{code}\u{1}{w}");
                match s.cache.get(&key) {
                    Some(&ok) => !ok,
                    None => match other(&mut s, &code) {
                        Some(d) => {
                            let ok = s.ignored.contains(w) || s.personal.contains(w) || d.check(w);
                            s.cache.insert(key, ok);
                            !ok
                        }
                        None => false,
                    },
                }
            }
        };
        if bad {
            out.push(r);
        }
    }
    out
}

/// The language spans of a paragraph for [`misspelled_in`].
pub fn para_spans(p: &apex_doc::Paragraph) -> Vec<(Range<usize>, Option<Arc<str>>, bool)> {
    p.spans().filter(|(s, e, a)| e > s && (a.lang.is_some() || a.no_proof)).map(|(s, e, a)| (s..e, a.lang.clone(), a.no_proof)).collect()
}

/// The misspelled word of `text` around byte `at`.
pub fn misspelled_at(text: &str, at: usize) -> Option<Range<usize>> {
    misspelled(text).into_iter().find(|r| r.start <= at && at <= r.end)
}

/// Suggestions for a word (slow for long words: call once, not per frame).
pub fn suggest(word: &str, max: usize) -> Vec<String> {
    let dict = state().dict.clone();
    dict.map(|d| d.suggest(word, max)).unwrap_or_default()
}

fn forget(s: &mut State) {
    s.generation += 1;
    s.cache.clear();
}

/// Ignores the word everywhere until the program closes.
pub fn ignore_all(word: &str) {
    let mut s = state();
    s.ignored.insert(word.to_string());
    forget(&mut s);
}

/// Adds the word to the personal dictionary (saved at once).
pub fn add_word(word: &str) {
    let mut s = state();
    if s.personal.add(word)
        && let Some(p) = personal_path()
    {
        if let Some(dir) = p.parent() {
            let _ = apex_fs::create_dir_all(dir);
        }
        let _ = s.personal.save(&p);
    }
    forget(&mut s);
}

/// Tests: each window starts from the same spelling state.
#[cfg(test)]
pub mod test {
    use super::*;

    /// Forgets the words added and ignored, goes back to the language the
    /// first window had (another may have picked one), with its dictionary
    /// loaded, and stops the suggestions' time limit: a word gets the same
    /// suggestions in every window.
    pub fn reset() {
        static FIRST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        apex_spell::set_time_budget(false);
        let start = web_time::Instant::now();
        while loading() && start.elapsed() < web_time::Duration::from_secs(30) {
            apex_fs::thread::sleep(web_time::Duration::from_millis(10));
        }
        let first = FIRST.get_or_init(language).clone();
        let mut s = state();
        s.ignored.clear();
        s.personal = Personal::new();
        // As a new window's settings have it: checking as you type.
        s.auto = true;
        s.lang_changed = false;
        s.auto_changed = false;
        if s.lang != first {
            s.dict = installed().iter().find(|(l, _, _)| *l == first).and_then(|(_, aff, dic)| Dictionary::load(aff, dic).ok()).map(Arc::new);
            s.lang = first;
            s.loading = false;
        }
        forget(&mut s);
    }

    #[test]
    fn reset_forgets_words() {
        ignore_all("qwzxv");
        state().personal.add("zzxqv");
        set_auto(false);
        reset();
        let s = state();
        assert!(s.ignored.is_empty() && !s.personal.contains("zzxqv"));
        // A window after it starts as its settings say.
        assert!(s.auto && !s.auto_changed);
    }
}

// --- Spelling dialog --------------------------------------------------------------------------

/// What the user chose in the Spelling dialog.
pub enum Action {
    IgnoreOnce,
    IgnoreAll,
    Add,
    Change(String),
    ChangeAll(String),
    /// The dictionary changed: look again from the same word.
    Recheck,
    /// A grammar mistake: don't flag its rule again.
    IgnoreRule,
    Close,
}

/// The Spelling dialog's state; the view finds the words and applies the
/// changes.
#[derive(Default)]
pub struct Dialog {
    pub word: String,
    /// The sentence around the word and the word's byte range in it.
    pub context: String,
    pub range: Range<usize>,
    pub suggestions: Vec<String>,
    pub replacement: String,
    /// No more misspelled words.
    pub done: bool,
    /// A grammar mistake (its rule) rather than a misspelled word.
    pub rule: Option<&'static str>,
    /// Grammar is checked too (the title says so).
    pub grammar: bool,
}

impl Dialog {
    /// Shows a misspelled word found in `context` at `range`.
    pub fn found(&mut self, context: &str, range: Range<usize>) {
        let word = context[range.clone()].to_string();
        let suggestions = suggest(&word, 8);
        self.show(context, range, None, suggestions);
    }

    /// Shows a grammar mistake found by `rule`.
    pub fn found_issue(&mut self, context: &str, range: Range<usize>, rule: &'static str, suggestions: Vec<String>) {
        self.show(context, range, Some(rule), suggestions);
    }

    fn show(&mut self, context: &str, range: Range<usize>, rule: Option<&'static str>, suggestions: Vec<String>) {
        self.word = context[range.clone()].to_string();
        self.rule = rule;
        self.suggestions = suggestions;
        self.replacement = self.suggestions.first().cloned().unwrap_or_else(|| self.word.clone());
        // Keep a readable window of text around the word.
        let mut a = range.start.saturating_sub(80);
        while !context.is_char_boundary(a) {
            a -= 1;
        }
        let mut b = (range.end + 80).min(context.len());
        while !context.is_char_boundary(b) {
            b += 1;
        }
        self.context = context[a..b].to_string();
        self.range = range.start - a..range.end - a;
        self.done = false;
    }

    pub fn finish(&mut self) {
        self.done = true;
    }

    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Action> {
        let mut action = None;
        let mut open = true;
        let title = format!("{} ({})", tr(if self.grammar { "spell.title_grammar" } else { "spell.title" }), language_name(&language()));
        crate::widgets::esc_closes(ctx, egui::Id::new("spell_dialog"), &mut open);
        egui::Window::new(title).id(egui::Id::new("spell_dialog")).open(&mut open).collapsible(false).resizable(false).default_width(460.0).default_pos(ctx.content_rect().right_top() + egui::vec2(-500.0, 130.0)).show_fit(ctx, |ui| {
            if self.done {
                ui.label(tr(if ready() { "spell.complete" } else { "spell.no_dictionary" }));
                if !ready() && !loading() {
                    ui.label(egui::RichText::new(tr("spell.get_hint")).weak());
                    ui.horizontal(|ui| get_dictionaries(ui));
                }
                ui.add_space(8.0);
                if ui.button(tr("dialog.ok")).clicked() {
                    action = Some(Action::Close);
                }
                return;
            }
            let mark = match self.rule {
                Some(r) => {
                    crate::widgets::section(ui, tr("grammar.title"));
                    ui.label(crate::grammar::message(r));
                    GRAMMAR_BLUE
                }
                None => {
                    crate::widgets::section(ui, tr("spell.not_in_dictionary").trim_end_matches(':'));
                    egui::Color32::from_rgb(0xC0, 0x20, 0x20)
                }
            };
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let mut job = egui::text::LayoutJob::default();
                let normal = egui::TextFormat { color: ui.visuals().text_color(), ..Default::default() };
                let bad = egui::TextFormat { color: mark, underline: egui::Stroke::new(1.0, mark), ..normal.clone() };
                job.append(&self.context[..self.range.start], 0.0, normal.clone());
                job.append(&self.context[self.range.clone()], 0.0, bad);
                job.append(&self.context[self.range.end..], 0.0, normal);
                job.wrap.max_width = ui.available_width();
                ui.label(job);
            });
            crate::widgets::section(ui, tr("spell.suggestions").trim_end_matches(':'));
            crate::widgets::form(ui, "spell_change", |ui| {
                crate::widgets::form_row(ui, tr("spell.change_to").trim_end_matches(':'), |ui| ui.add(egui::TextEdit::singleline(&mut self.replacement).desired_width(260.0)));
            });
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_size(egui::vec2(220.0, 150.0));
                        egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                            if self.suggestions.is_empty() {
                                ui.weak(tr("spell.no_suggestions"));
                            }
                            for s in &self.suggestions {
                                let r = ui.selectable_label(*s == self.replacement, s);
                                if r.clicked() {
                                    self.replacement = s.clone();
                                }
                                if r.double_clicked() {
                                    action = Some(Action::Change(s.clone()));
                                }
                            }
                        });
                    });
                });
                ui.vertical(|ui| {
                    let w = egui::vec2(170.0, 0.0);
                    if ui.add(egui::Button::new(tr("spell.ignore_once")).min_size(w)).clicked() {
                        action = Some(Action::IgnoreOnce);
                    }
                    if self.rule.is_some() {
                        if ui.add(egui::Button::new(tr("grammar.ignore_rule")).min_size(w)).clicked() {
                            action = Some(Action::IgnoreRule);
                        }
                        ui.add_space(6.0);
                        if ui.add_enabled(!self.suggestions.is_empty(), egui::Button::new(tr("spell.change")).min_size(w)).clicked() {
                            action = Some(Action::Change(self.replacement.clone()));
                        }
                        return;
                    }
                    if ui.add(egui::Button::new(tr("spell.ignore_all")).min_size(w)).clicked() {
                        action = Some(Action::IgnoreAll);
                    }
                    if ui.add(egui::Button::new(tr("spell.add")).min_size(w)).clicked() {
                        action = Some(Action::Add);
                    }
                    ui.add_space(6.0);
                    let can = !self.replacement.trim().is_empty();
                    if ui.add_enabled(can, egui::Button::new(tr("spell.change")).min_size(w)).clicked() {
                        action = Some(Action::Change(self.replacement.clone()));
                    }
                    if ui.add_enabled(can, egui::Button::new(tr("spell.change_all")).min_size(w)).clicked() {
                        action = Some(Action::ChangeAll(self.replacement.clone()));
                    }
                });
            });
            crate::widgets::section(ui, tr("spell.sec_options"));
            crate::widgets::form(ui, "spell_options", |ui| {
                crate::widgets::form_row(ui, tr("spell.language").trim_end_matches(':'), |ui| {
                    if language_menu(ui) {
                        action = Some(Action::Recheck);
                    }
                });
            });
            if ui.button(format!("{}…", tr("ac.title"))).clicked() {
                crate::autocorrect::open_dialog();
            }
        });
        if !open {
            action = Some(Action::Close);
        }
        action
    }
}

/// The color of grammar mistakes (spelling's is red).
pub const GRAMMAR_BLUE: egui::Color32 = egui::Color32::from_rgb(0x2B, 0x5C, 0xD6);

/// A combo box with the installed dictionaries; true when the choice changed.
pub fn language_menu(ui: &mut egui::Ui) -> bool {
    let cur = language();
    let mut changed = false;
    let label = if cur.is_empty() { tr("spell.none_installed").to_string() } else { language_name(&cur) };
    egui::ComboBox::from_id_salt("spell_lang").selected_text(label).show_ui(ui, |ui| {
        for l in &languages() {
            if ui.selectable_label(*l == cur, format!("{} - {l}", language_name(l))).clicked() {
                set_language(ui.ctx(), l);
                changed = true;
            }
        }
    });
    changed
}

/// "Download dictionaries…" (LibreOffice's) and "Install dictionary…" (the
/// file downloaded); true when one was clicked.
pub fn get_dictionaries(ui: &mut egui::Ui) -> bool {
    if cfg!(target_arch = "wasm32") {
        return false;
    }
    let mut clicked = false;
    if ui.button(format!("{}…", tr("spell.download"))).on_hover_text(install::DOWNLOAD_URL).clicked() {
        crate::common::open_external(install::DOWNLOAD_URL);
        clicked = true;
    }
    if ui.button(format!("{}…", tr("spell.install"))).on_hover_text(tr("spell.install_tip")).clicked() {
        install::pick();
        clicked = true;
    }
    clicked
}

/// Proofing items of the Review menus (`as_you_type`: the documents'
/// red underlines).
/// `text_lang`: set when "Language of the text…" is chosen (documents).
pub fn review_items(ui: &mut egui::Ui, as_you_type: bool, mut text_lang: Option<&mut bool>) {
    use crate::{icons::Icon, toolstrip};
    let on = auto();
    if as_you_type && toolstrip::check(ui, on, Icon::Spelling, tr("spell.auto")) {
        set_auto(!on);
    }
    if as_you_type {
        crate::grammar::review_items(ui);
    }
    let cur = language();
    toolstrip::small_menu(ui, Some(Icon::Language), tr("spell.language_menu"), "", toolstrip::accent(), |ui| {
        if let Some(t) = text_lang.as_deref_mut() {
            if ui.button(format!("{}…", tr("lang.title"))).clicked() {
                *t = true;
                ui.close();
            }
            ui.separator();
            ui.weak(tr("lang.default_dict"));
        }
        let langs = languages();
        if langs.is_empty() {
            ui.weak(tr("spell.none_installed"));
        }
        for l in &langs {
            if ui.radio(*l == cur, format!("{} - {l}", language_name(l))).clicked() {
                set_language(ui.ctx(), l);
                ui.close();
            }
        }
        ui.separator();
        if get_dictionaries(ui) {
            ui.close();
        }
    });
    if toolstrip::small(ui, Some(Icon::Replace), &format!("{}…", tr("ac.title")), "", toolstrip::accent(), false).clicked() {
        crate::autocorrect::open_dialog();
    }
}

/// Status text while the dictionary loads.
pub fn status() -> Option<String> {
    loading().then(|| trf("spell.loading", &[("lang", &language_name(&language()))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(language_name("es_ES"), "Español (ES)");
        assert_eq!(language_name("en_US"), "English (US)");
        assert_eq!(language_name("xx"), "xx");
    }

    // Turning the check off and on again changes only that setting: the
    // language found at start isn't saved as if it had been chosen.
    #[test]
    fn toggling_the_check_leaves_the_language_alone() {
        let on = auto();
        set_auto(!on);
        set_auto(on);
        assert_eq!(take_changed(), (false, true));
        assert_eq!(take_changed(), (false, false));
    }

    #[test]
    fn dialog_context() {
        let text = "a".repeat(200) + " wrod " + &"b".repeat(200);
        let mut d = Dialog::default();
        d.found(&text, 201..205);
        assert_eq!(&d.context[d.range.clone()], "wrod");
        assert!(d.context.len() < 200);
    }
}
