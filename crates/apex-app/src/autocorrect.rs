//! AutoCorrect: the replacement list and the options shared by both
//! programs (capital at the start of sentences, smart quotes, automatic
//! lists), kept in `autocorrect.txt` in the config directory, and their
//! dialog.

use std::sync::{LazyLock, Mutex, MutexGuard};

use apex_doc::ParaKind;
use eframe::egui;

use crate::i18n::tr;
use crate::icons::Icon;
use crate::widgets::{self, DialogResult};

#[derive(Clone)]
pub struct Options {
    pub capitalize: bool,
    pub smart_quotes: bool,
    pub lists: bool,
    pub replace: bool,
    /// Typed text -> replacement, sorted by typed text.
    pub entries: Vec<(String, String)>,
}

struct State {
    opts: Options,
    /// The list was loaded from the file (else it follows the language).
    custom: bool,
    loaded: bool,
    dialog: Option<Dialog>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        opts: Options { capitalize: true, smart_quotes: true, lists: true, replace: true, entries: Vec::new() },
        custom: false,
        loaded: false,
        dialog: None,
    })
});

fn state() -> MutexGuard<'static, State> {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if !s.loaded {
        s.loaded = true;
        load(&mut s);
    }
    s
}

const SYMBOLS: &[(&str, &str)] = &[("(c)", "©"), ("(r)", "®"), ("(tm)", "™"), ("(e)", "€"), ("...", "…"), ("--", "–"), ("1/2", "½"), ("1/4", "¼"), ("3/4", "¾")];

const ENGLISH: &[(&str, &str)] = &[
    ("i", "I"),
    ("teh", "the"),
    ("adn", "and"),
    ("hte", "the"),
    ("recieve", "receive"),
    ("recieved", "received"),
    ("seperate", "separate"),
    ("definately", "definitely"),
    ("occured", "occurred"),
    ("untill", "until"),
    ("wich", "which"),
    ("becuase", "because"),
    ("thier", "their"),
    ("alot", "a lot"),
    ("dont", "don't"),
    ("doesnt", "doesn't"),
    ("didnt", "didn't"),
    ("cant", "can't"),
    ("isnt", "isn't"),
    ("wasnt", "wasn't"),
    ("im", "I'm"),
    ("ive", "I've"),
    ("tommorow", "tomorrow"),
    ("tomorow", "tomorrow"),
    ("accomodate", "accommodate"),
    ("acheive", "achieve"),
    ("beleive", "believe"),
    ("goverment", "government"),
    ("neccessary", "necessary"),
    ("occurence", "occurrence"),
    ("reccomend", "recommend"),
    ("truely", "truly"),
];

const SPANISH: &[(&str, &str)] = &[
    ("tambien", "también"),
    ("despues", "después"),
    ("ademas", "además"),
    ("todavia", "todavía"),
    ("aqui", "aquí"),
    ("alli", "allí"),
    ("facil", "fácil"),
    ("dificil", "difícil"),
    ("util", "útil"),
    ("pagina", "página"),
    ("numero", "número"),
    ("telefono", "teléfono"),
    ("informacion", "información"),
    ("direccion", "dirección"),
    ("atencion", "atención"),
    ("razon", "razón"),
    ("ningun", "ningún"),
    ("algun", "algún"),
    ("haci", "así"),
    ("porfavor", "por favor"),
    ("nose", "no sé"),
    ("osea", "o sea"),
    ("aveces", "a veces"),
    ("enserio", "en serio"),
];

/// The built-in list for the spelling language.
fn defaults() -> Vec<(String, String)> {
    let lang = crate::spell::language();
    let lang = if lang.is_empty() { crate::i18n::current().to_string() } else { lang };
    let mut v: Vec<(String, String)> = SYMBOLS.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    let extra = if lang.starts_with("es") { SPANISH } else if lang.starts_with("en") || lang.is_empty() { ENGLISH } else { &[] };
    v.extend(extra.iter().map(|(a, b)| (a.to_string(), b.to_string())));
    v.sort();
    v
}

fn path() -> Option<std::path::PathBuf> {
    crate::settings::config_dir().map(|d| d.join("autocorrect.txt"))
}

fn load(s: &mut State) {
    let Some(text) = path().and_then(|p| apex_fs::read_to_string(p).ok()) else { return };
    let mut entries = Vec::new();
    let mut listed = false;
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=')
            && !k.contains('\t')
        {
            let on = v.trim() != "0";
            match k.trim() {
                "capitalize" => s.opts.capitalize = on,
                "smart_quotes" => s.opts.smart_quotes = on,
                "lists" => s.opts.lists = on,
                "replace" => s.opts.replace = on,
                "custom_list" => listed = on,
                _ => {}
            }
        } else if let Some((a, b)) = line.split_once('\t')
            && !a.is_empty()
        {
            entries.push((a.to_string(), b.to_string()));
        }
    }
    if listed {
        entries.sort();
        s.opts.entries = entries;
        s.custom = true;
    }
}

fn save(s: &State) {
    let Some(p) = path() else { return };
    let o = &s.opts;
    let mut out = format!("capitalize={}\nsmart_quotes={}\nlists={}\nreplace={}\ncustom_list={}\n", o.capitalize as u8, o.smart_quotes as u8, o.lists as u8, o.replace as u8, s.custom as u8);
    if s.custom {
        for (a, b) in &o.entries {
            out.push_str(&format!("{a}\t{b}\n"));
        }
    }
    if let Some(d) = p.parent() {
        let _ = apex_fs::create_dir_all(d);
    }
    let _ = apex_fs::write(p, out);
}

pub fn options() -> Options {
    let s = state();
    let mut o = s.opts.clone();
    if !s.custom {
        o.entries = defaults();
    }
    o
}

// --- Rules ------------------------------------------------------------------------------------

/// The replacement for a typed word: an exact entry, or a lower-case entry
/// typed with a capital ("Teh" -> "The").
pub fn replacement(entries: &[(String, String)], word: &str) -> Option<String> {
    cased_replacement(entries, word).filter(|r| r != word)
}

fn cased_replacement(entries: &[(String, String)], word: &str) -> Option<String> {
    let find = |w: &str| entries.binary_search_by(|(a, _)| a.as_str().cmp(w)).ok().map(|i| entries[i].1.clone());
    if let Some(r) = find(word) {
        return Some(r);
    }
    let lower = word.to_lowercase();
    if lower == word {
        return None;
    }
    let r = find(&lower)?;
    let mut chars = word.chars();
    let first_upper = chars.next().is_some_and(char::is_uppercase);
    if word.chars().count() > 1 && word.chars().all(|c| !c.is_lowercase()) {
        return Some(r.to_uppercase());
    }
    if first_upper {
        let mut c = r.chars();
        return c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str());
    }
    None
}

/// Abbreviations after which a word doesn't start a sentence.
const ABBREVIATIONS: &[&str] = &["e.g", "i.e", "etc", "vs", "mr", "mrs", "ms", "dr", "sr", "sra", "srta", "dra", "p.ej", "ej", "pág", "núm", "no", "fig", "cf", "approx", "aprox", "ud", "uds", "vd", "st", "ave", "av", "avda"];

/// Whether a word that follows `before` (the paragraph text up to it)
/// starts a sentence.
pub fn starts_sentence(before: &str) -> bool {
    let t = before.trim_end_matches(|c: char| c.is_whitespace() || "¿¡\"'“‘«([".contains(c));
    if t.is_empty() {
        return true;
    }
    if before.len() == t.len() {
        // No space between: part of the same word.
        return false;
    }
    let Some(last) = t.chars().last() else { return true };
    if !matches!(last, '.' | '!' | '?' | '…') {
        return false;
    }
    let prev = t[..t.len() - last.len_utf8()].rsplit(char::is_whitespace).next().unwrap_or("");
    let prev = prev.trim_start_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    // "etc." or a single initial ("J. Smith") aren't sentence ends.
    !(last == '.' && (ABBREVIATIONS.contains(&prev.as_str()) || prev.chars().count() == 1 && prev.chars().all(char::is_alphabetic)))
}

/// Whether a word should get a capital when it starts a sentence (not an
/// address, a number or a word with capitals of its own).
pub fn wants_capital(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else { return false };
    first.is_lowercase() && !word.contains(['.', '@', '/', ':', '_']) && !word.chars().any(|c| c.is_ascii_digit()) && !chars.any(char::is_uppercase)
}

/// The typographic quote for a typed `"` or `'` after `prev`.
pub fn smart_quote(prev: Option<char>, q: char) -> char {
    let opening = prev.is_none_or(|c| c.is_whitespace() || "([{¿¡«“‘—–-/".contains(c));
    match (q, opening) {
        ('"', true) => '“',
        ('"', false) => '”',
        (_, true) => '‘',
        (_, false) => '’',
    }
}

/// The list a paragraph starting with `marker` and a space turns into:
/// "*", "-" or "•" start a bulleted list; "1.", "1)", "a)", "A.", "i." and
/// "I." a numbered one in that style.
pub fn list_marker(marker: &str) -> Option<(ParaKind, u8)> {
    match marker {
        "*" | "•" => return Some((ParaKind::Bullet, 1)),
        "-" | "–" => return Some((ParaKind::Bullet, 4)),
        ">" => return Some((ParaKind::Bullet, 6)),
        _ => {}
    }
    let format = match marker {
        "1." => 1,
        "1)" => 2,
        "a)" => 3,
        "A." => 4,
        "i." => 5,
        "I." => 6,
        _ => return None,
    };
    Some((ParaKind::Numbered, format))
}

/// Replacements in a whole text (spreadsheet cells, on entry). Entries
/// with digits are left out: "1/2" in a cell is a date.
pub fn correct_text(text: &str) -> Option<String> {
    let mut o = options();
    if !o.replace || text.starts_with(['=', '\'']) {
        return None;
    }
    o.entries.retain(|(a, _)| !a.chars().any(|c| c.is_ascii_digit()));
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut rest = text;
    while !rest.is_empty() {
        let ws = rest.find(|c: char| !c.is_whitespace()).unwrap_or(rest.len());
        out.push_str(&rest[..ws]);
        rest = &rest[ws..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..end];
        // Punctuation around the word stays.
        let core_start = word.find(|c: char| c.is_alphanumeric() || c == '(').unwrap_or(word.len());
        let core_end = word.rfind(|c: char| c.is_alphanumeric() || c == ')').map_or(core_start, |i| i + word[i..].chars().next().map_or(1, char::len_utf8));
        let core = &word[core_start..core_end.max(core_start)];
        match replacement(&o.entries, word).map(|r| (r, 0, word.len())).or_else(|| replacement(&o.entries, core).map(|r| (r, core_start, core_end))) {
            Some((r, a, b)) if !core.is_empty() || a == 0 => {
                out.push_str(&word[..a]);
                out.push_str(&r);
                out.push_str(&word[b..]);
                changed = true;
            }
            _ => out.push_str(word),
        }
        rest = &rest[end..];
    }
    changed.then_some(out)
}

// --- Dialog -----------------------------------------------------------------------------------

struct Dialog {
    typed: String,
    with: String,
    page: usize,
    /// The options being edited (saved on OK).
    opts: Options,
    orig: Vec<(String, String)>,
    /// The list went back to the language's.
    reset: bool,
}

pub fn open_dialog() {
    let opts = options();
    let orig = opts.entries.clone();
    state().dialog = Some(Dialog { typed: String::new(), with: String::new(), page: 0, opts, orig, reset: false });
}

/// Closes the dialog without saving (it outlives the window that opened
/// it; each test window starts without it).
#[cfg(test)]
pub fn close_dialog() {
    state().dialog = None;
}

/// Whether Add (or Replace) has something to store: a new entry, or
/// another replacement for one in the list.
fn can_store(entries: &[(String, String)], typed: &str, with: &str) -> bool {
    if typed.is_empty() || with.is_empty() || typed.contains('\t') || with.contains('\t') {
        return false;
    }
    entries.binary_search_by(|(a, _)| a.as_str().cmp(typed)).map_or(true, |i| entries[i].1 != with)
}

/// The AutoCorrect Options dialog (shown by the app every frame).
pub fn dialog_ui(ctx: &egui::Context) {
    let Some(mut d) = state().dialog.take() else { return };
    let pages = [(Icon::Replace, "ac.page_replace"), (Icon::Bullets, "ac.page_format")];
    let Dialog { typed, with, page, opts: o, reset, .. } = &mut d;
    let res = widgets::nav_dialog(ctx, "autocorrect", tr("ac.title"), &pages, page, egui::vec2(470.0, 400.0), |ui, page| {
        if page == 1 {
            widgets::section(ui, tr("ac.sec_replace_typing"));
            ui.checkbox(&mut o.smart_quotes, tr("ac.smart_quotes"));
            widgets::section(ui, tr("ac.sec_apply_typing"));
            ui.checkbox(&mut o.lists, tr("ac.lists"));
            return;
        }
        widgets::section(ui, tr("ac.sec_rules"));
        ui.checkbox(&mut o.capitalize, tr("ac.capitalize"));
        widgets::section(ui, tr("ac.sec_replacements"));
        ui.checkbox(&mut o.replace, tr("ac.replace"));
        ui.add_space(4.0);
        ui.add_enabled_ui(o.replace, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(tr("ac.typed"));
                    ui.add(egui::TextEdit::singleline(typed).desired_width(130.0));
                });
                ui.vertical(|ui| {
                    ui.label(tr("ac.with"));
                    ui.add(egui::TextEdit::singleline(with).desired_width(ui.available_width()));
                });
            });
            let pos = o.entries.binary_search_by(|(a, _)| a.as_str().cmp(typed.trim()));
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().max_height(170.0).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Grid::new("ac_entries").num_columns(2).striped(true).min_col_width(130.0).spacing([24.0, 2.0]).show(ui, |ui| {
                        for (i, (a, b)) in o.entries.iter().enumerate() {
                            let sel = pos == Ok(i);
                            if ui.selectable_label(sel, a).clicked() {
                                *typed = a.clone();
                                *with = b.clone();
                            }
                            ui.label(b);
                            ui.end_row();
                        }
                    });
                });
            });
            ui.horizontal(|ui| {
                let t = typed.trim().to_string();
                let label = if pos.is_ok() { tr("ac.update") } else { tr("ac.add") };
                if ui.add_enabled(can_store(&o.entries, &t, with), egui::Button::new(label)).clicked() {
                    match pos {
                        Ok(i) => o.entries[i].1 = with.clone(),
                        Err(i) => o.entries.insert(i, (t.clone(), with.clone())),
                    }
                }
                if ui.add_enabled(pos.is_ok(), egui::Button::new(tr("ac.delete"))).clicked()
                    && let Ok(i) = pos
                {
                    o.entries.remove(i);
                    typed.clear();
                    with.clear();
                }
                if ui.button(tr("ac.reset")).clicked() {
                    o.entries = defaults();
                    *reset = true;
                }
            });
        });
    });
    match res {
        DialogResult::None => state().dialog = Some(d),
        DialogResult::Cancel => {}
        DialogResult::Ok => {
            let mut s = state();
            let defaults = defaults();
            if d.opts.entries != d.orig {
                s.custom = !(d.reset && d.opts.entries == defaults);
            } else if d.reset {
                s.custom = false;
            }
            s.opts = d.opts;
            if !s.custom {
                s.opts.entries.clear();
            }
            save(&s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_only_changes() {
        let l = vec![("(c)".to_string(), "©".to_string())];
        assert!(!can_store(&l, "(c)", "©"), "the same replacement again");
        assert!(can_store(&l, "(c)", "(C)"));
        assert!(can_store(&l, "(p)", "℗"));
        assert!(!can_store(&l, "", "x") && !can_store(&l, "x", ""));
    }

    fn list() -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = [("(c)", "©"), ("teh", "the"), ("i", "I")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        v.sort();
        v
    }

    #[test]
    fn replacements() {
        let l = list();
        assert_eq!(replacement(&l, "teh").as_deref(), Some("the"));
        assert_eq!(replacement(&l, "Teh").as_deref(), Some("The"));
        assert_eq!(replacement(&l, "TEH").as_deref(), Some("THE"));
        assert_eq!(replacement(&l, "(c)").as_deref(), Some("©"));
        assert_eq!(replacement(&l, "the"), None);
        assert_eq!(replacement(&l, "I"), None);
    }

    #[test]
    fn sentences() {
        assert!(starts_sentence(""));
        assert!(starts_sentence("Hello. "));
        assert!(starts_sentence("¿Qué? ¿"));
        assert!(!starts_sentence("Hello "));
        assert!(!starts_sentence("apples, pears, etc. "));
        assert!(!starts_sentence("J. "));
        assert!(!starts_sentence("abc"));
        assert!(wants_capital("hello"));
        assert!(!wants_capital("iPhone"));
        assert!(!wants_capital("www.x.com"));
        assert!(!wants_capital("Hello"));
    }

    #[test]
    fn quotes_and_lists() {
        assert_eq!(smart_quote(None, '"'), '“');
        assert_eq!(smart_quote(Some('a'), '"'), '”');
        assert_eq!(smart_quote(Some(' '), '\''), '‘');
        assert_eq!(smart_quote(Some('n'), '\''), '’');
        assert_eq!(list_marker("*"), Some((ParaKind::Bullet, 1)));
        assert_eq!(list_marker("1."), Some((ParaKind::Numbered, 1)));
        assert_eq!(list_marker("a)"), Some((ParaKind::Numbered, 3)));
        assert_eq!(list_marker("2."), None);
    }
}
