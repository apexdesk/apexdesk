//! The thesaurus of the spelling language, loaded in the background the
//! first time it's needed.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use apex_spell::{Meaning, Thesaurus};
use eframe::egui;

enum Slot {
    Loading,
    Ready(Arc<Thesaurus>),
    Missing,
}

/// Language -> thesaurus.
static LOADED: LazyLock<Mutex<Vec<(String, Slot)>>> = LazyLock::new(|| Mutex::new(Vec::new()));

pub enum Lookup {
    Found(Vec<Meaning>),
    Loading,
    /// No thesaurus for the language.
    Missing,
}

fn start(ctx: &egui::Context, lang: &str, path: PathBuf) {
    let ctx = ctx.clone();
    let lang = lang.to_string();
    apex_fs::thread::spawn(move || {
        let t = Thesaurus::load(&path).ok().map(Arc::new);
        let mut all = LOADED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = all.iter_mut().find(|(l, _)| *l == lang) {
            s.1 = t.map_or(Slot::Missing, Slot::Ready);
        }
        drop(all);
        ctx.request_repaint();
    });
}

/// The meanings of a word in the spelling language.
pub fn lookup(ctx: &egui::Context, word: &str) -> Lookup {
    let lang = crate::spell::language();
    let mut all = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    match all.iter().find(|(l, _)| *l == lang) {
        Some((_, Slot::Ready(t))) => Lookup::Found(t.lookup(word)),
        Some((_, Slot::Loading)) => Lookup::Loading,
        Some((_, Slot::Missing)) => Lookup::Missing,
        None => match apex_spell::thesaurus_for(&lang) {
            Some(path) => {
                all.push((lang.clone(), Slot::Loading));
                drop(all);
                start(ctx, &lang, path);
                Lookup::Loading
            }
            None => {
                all.push((lang, Slot::Missing));
                Lookup::Missing
            }
        },
    }
}

/// The first synonyms of a word, for short menus (empty while loading or
/// without a thesaurus).
pub fn quick(ctx: &egui::Context, word: &str, max: usize) -> Vec<String> {
    let Lookup::Found(m) = lookup(ctx, word) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    // One from each meaning first, then the rest.
    for round in 0..8 {
        for x in &m {
            if let Some(w) = x.words.get(round)
                && !out.contains(w)
            {
                out.push(w.clone());
            }
            if out.len() >= max {
                return out;
            }
        }
    }
    out
}
