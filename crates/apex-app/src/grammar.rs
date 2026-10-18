//! Grammar checking in documents: the rules of `apex_spell::check_grammar`
//! in the spelling language, the ones turned off and what was ignored.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use apex_spell::Issue;

use crate::i18n::tr;

struct State {
    /// Check as you type (blue lines) and with F7.
    auto: bool,
    /// Rules not to flag again (a setting).
    off: HashSet<String>,
    /// "rule\u{1}text" ignored until the program closes.
    ignored: HashSet<String>,
    generation: u64,
    /// The setting changed (to save).
    changed: bool,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State { auto: true, off: HashSet::new(), ignored: HashSet::new(), generation: 1, changed: false }));

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// From the settings: checking on, and the rules turned off ("a,b").
pub fn init(auto: bool, off: &str) {
    let mut s = state();
    s.auto = auto;
    s.off = off.split(',').map(str::trim).filter(|r| !r.is_empty()).map(str::to_string).collect();
    s.generation += 1;
}

pub fn auto() -> bool {
    state().auto
}

pub fn set_auto(on: bool) {
    let mut s = state();
    if s.auto != on {
        s.auto = on;
        s.generation += 1;
        s.changed = true;
    }
}

/// Changes over which earlier results are stale (the dictionary's too).
pub fn generation() -> u64 {
    state().generation.wrapping_mul(1 << 20).wrapping_add(crate::spell::generation())
}

/// Whether the settings changed since the last call.
pub fn take_changed() -> bool {
    std::mem::take(&mut state().changed)
}

/// The rules turned off, for the settings.
pub fn off_rules() -> String {
    let s = state();
    let mut v: Vec<&str> = s.off.iter().map(String::as_str).collect();
    v.sort_unstable();
    v.join(",")
}

pub fn any_off() -> bool {
    !state().off.is_empty()
}

/// "Don't flag this rule again".
pub fn turn_off(rule: &str) {
    let mut s = state();
    s.off.insert(rule.to_string());
    s.generation += 1;
    s.changed = true;
}

/// Flags every rule again.
pub fn reset_rules() {
    let mut s = state();
    s.off.clear();
    s.generation += 1;
    s.changed = true;
}

/// Ignores this mistake (the same words and rule) until the program closes.
pub fn ignore(rule: &str, text: &str) {
    let mut s = state();
    s.ignored.insert(format!("{rule}\u{1}{text}"));
    s.generation += 1;
}

/// What a rule says is wrong.
pub fn message(rule: &str) -> &'static str {
    crate::i18n::lookup(&format!("grammar.{rule}")).unwrap_or("")
}

/// The mistakes of a paragraph's text in the spelling language. `spans`
/// are as for [`crate::spell::misspelled_in`]: text in another language or
/// not to be proofed isn't checked.
pub fn issues(text: &str, spans: &[(Range<usize>, Option<Arc<str>>, bool)]) -> Vec<Issue> {
    let lang = crate::spell::language();
    let dict = crate::spell::main_dict();
    if lang.is_empty() || !auto() {
        return Vec::new();
    }
    let base = |l: &str| l.split(['_', '-']).next().unwrap_or("").to_ascii_lowercase();
    let main = base(&lang);
    let mut out = apex_spell::check_grammar(text, &lang, dict.as_deref());
    let s = state();
    out.retain(|i| {
        !s.off.contains(i.rule)
            && !s.ignored.contains(&format!("{}\u{1}{}", i.rule, &text[i.range.clone()]))
            && !spans.iter().any(|(r, l, no_proof)| r.start < i.range.end && i.range.start < r.end && (*no_proof || l.as_deref().is_some_and(|l| base(l) != main)))
    });
    out
}

/// Grammar items of the Review menu: checking on or off.
pub fn review_items(ui: &mut eframe::egui::Ui) {
    use crate::{icons::Icon, toolstrip};
    let on = auto();
    if toolstrip::check(ui, on, Icon::ErrorCheck, tr("grammar.auto")) {
        set_auto(!on);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_has_a_message() {
        for r in apex_spell::RULES {
            assert!(!message(r).is_empty(), "grammar.{r}");
        }
    }

    #[test]
    fn rules_turned_off_are_saved_sorted() {
        init(true, " b, a ,");
        assert_eq!(off_rules(), "a,b");
        reset_rules();
        assert!(!any_off() && take_changed());
        turn_off("x");
        assert_eq!(off_rules(), "x");
        init(true, "");
    }
}
