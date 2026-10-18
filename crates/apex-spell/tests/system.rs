//! Integration tests over the system's Hunspell dictionaries. Each test is
//! skipped (passes with a note) when the dictionary isn't installed.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use apex_spell::Dictionary;

const DIR: &str = "/usr/share/hunspell";

fn paths(lang: &str) -> Option<(PathBuf, PathBuf)> {
    let aff = Path::new(DIR).join(format!("{lang}.aff"));
    let dic = aff.with_extension("dic");
    (aff.is_file() && dic.is_file()).then_some((aff, dic))
}

fn load(lang: &str) -> Option<&'static Dictionary> {
    static EN: OnceLock<Option<Dictionary>> = OnceLock::new();
    static ES: OnceLock<Option<Dictionary>> = OnceLock::new();
    let cell = if lang == "en_US" { &EN } else { &ES };
    let d = cell.get_or_init(|| {
        let (aff, dic) = paths(lang)?;
        Some(Dictionary::load(&aff, &dic).expect("system dictionary loads"))
    });
    if d.is_none() {
        eprintln!("skipping: {lang} not installed in {DIR}");
    }
    d.as_ref()
}

#[test]
fn en_us_check() {
    let Some(d) = load("en_US") else { return };
    assert_eq!(d.language(), "en_US");
    for w in [
        "houses", "unhappiness", "didn't", "didn’t", "Paris", "PARIS", "1st", "21st", "11th", "112th", "22nd",
        "Hello", "HELLO", "hello", "well-known", "NASA", "e-mail", "1990", "3.5", "iOS", "IOS",
    ] {
        assert!(d.check(w), "{w} should be correct");
    }
    for w in ["housse", "teh", "paris", "recieve", "1th", "21th", "hELLO", "Ios", "unhappyness"] {
        assert!(!d.check(w), "{w} should be wrong");
    }
}

#[test]
fn en_us_suggest() {
    let Some(d) = load("en_US") else { return };
    let s = d.suggest("teh", 8);
    assert!(s.iter().any(|x| x == "the"), "{s:?}");
    let s = d.suggest("recieve", 5);
    assert!(s.iter().take(2).any(|x| x == "receive"), "{s:?}");
    let s = d.suggest("Recieve", 5);
    assert_eq!(s.first().map(String::as_str), Some("Receive"), "{s:?}");
    let s = d.suggest("RECIEVE", 5);
    assert_eq!(s.first().map(String::as_str), Some("RECEIVE"), "{s:?}");
    let s = d.suggest("housse", 5);
    assert!(s.iter().any(|x| x == "house") && s.iter().any(|x| x == "houses"), "{s:?}");
    let s = d.suggest("alot", 5);
    assert!(s.iter().any(|x| x == "a lot"), "{s:?}");
    let s = d.suggest("paris", 5);
    // Hunspell itself gives ["pairs", "Paris", ...].
    assert!(s.iter().take(2).any(|x| x == "Paris"), "{s:?}");
    // Two errors: only the n-gram search finds it.
    let s = d.suggest("exbition", 5);
    assert!(s.iter().any(|x| x == "exhibition"), "{s:?}");
    for s in d.suggest("wierd", 10) {
        assert!(s.split(' ').all(|p| d.check(p)), "suggestion {s} must be correct");
    }
}

#[test]
fn es_es_check() {
    let Some(d) = load("es_ES") else { return };
    // "cómpraselo" (from the task list) is rejected by Hunspell itself with
    // the installed es_ES; "cántaselo"/"dígaselo" exercise the same enclitics.
    for w in [
        "canción", "canciones", "cántaselo", "dígaselo", "dámelo", "comprándoselo", "rápidamente", "Madrid",
        "MADRID", "colaboraciones", "desagradable", "Árbol", "ÁRBOL", "árbol",
    ] {
        assert!(d.check(w), "{w} should be correct");
    }
    for w in ["cancion", "rapidamente", "compraciones", "madrid", "arbol", "inconstitucionalmente"] {
        assert!(!d.check(w), "{w} should be wrong");
    }
}

#[test]
fn es_es_suggest() {
    let Some(d) = load("es_ES") else { return };
    assert_eq!(d.suggest("cancion", 5).first().map(String::as_str), Some("canción"));
    assert_eq!(d.suggest("Cancion", 5).first().map(String::as_str), Some("Canción"));
    assert_eq!(d.suggest("CANCION", 5).first().map(String::as_str), Some("CANCIÓN"));
    assert_eq!(d.suggest("rapidamente", 5).first().map(String::as_str), Some("rápidamente"));
    let s = d.suggest("tambien", 5);
    assert_eq!(s.first().map(String::as_str), Some("también"), "{s:?}");
    let s = d.suggest("madrid", 5);
    assert_eq!(s.first().map(String::as_str), Some("Madrid"), "{s:?}");
}

#[test]
fn installed_lists_system_dictionaries() {
    if paths("en_US").is_none() {
        eprintln!("skipping: en_US not installed");
        return;
    }
    let list = apex_spell::installed();
    assert!(list.iter().any(|(l, a, d)| l == "en_US" && a.is_file() && d.is_file()), "{list:?}");
    let mut sorted = list.clone();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(list.iter().map(|x| &x.0).collect::<Vec<_>>(), sorted.iter().map(|x| &x.0).collect::<Vec<_>>());
    // Symlinked copies (es_AR -> es_ES...) are listed once.
    let es: Vec<_> = list.iter().filter(|x| x.0.starts_with("es_")).collect();
    if paths("es_ES").is_some() && paths("es_AR").is_some() {
        assert_eq!(es.iter().filter(|x| x.0 == "es_ES").count(), 1);
        let canon: Vec<_> = es.iter().filter_map(|x| std::fs::canonicalize(&x.1).ok()).collect();
        let mut dedup = canon.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(canon.len(), dedup.len(), "{es:?}");
    }
}

/// Deterministic typo generator for the comparison sample.
fn misspell(w: &str, i: usize) -> String {
    let mut c: Vec<char> = w.chars().collect();
    if c.len() < 3 {
        return format!("{w}x");
    }
    let p = 1 + i % (c.len() - 1);
    match i % 4 {
        0 => c.swap(p - 1, p),
        1 => {
            c.remove(p);
        }
        2 => {
            let x = c[p];
            c.insert(p, x);
        }
        _ => c[p] = if c[p] == 'e' { 'a' } else { 'e' },
    }
    c.into_iter().collect()
}

/// Compares ~2000 words (dictionary forms plus typos, some capitalized)
/// with the `hunspell` command, when installed.
fn compare_with_hunspell(lang: &str) {
    let Some(d) = load(lang) else { return };
    let Some((aff, dic)) = paths(lang) else { return };
    if Command::new("hunspell").arg("-v").output().is_err() {
        eprintln!("skipping: hunspell command not installed");
        return;
    }
    let text = std::fs::read_to_string(&dic).unwrap_or_default();
    let stems: Vec<&str> = text
        .lines()
        .skip(1)
        .step_by(53)
        .filter_map(|l| l.split(['/', '\t', ' ']).next())
        .filter(|w| w.chars().count() >= 2 && w.chars().all(char::is_alphabetic))
        .take(1000)
        .collect();
    let mut sample: Vec<String> = Vec::new();
    for (i, w) in stems.iter().enumerate() {
        sample.push(w.to_string());
        let t = misspell(w, i);
        sample.push(match i % 5 {
            0 => t.to_uppercase(),
            1 => {
                let mut c = t.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
            }
            _ => t,
        });
    }
    let base = aff.with_extension("");
    let mut child = match Command::new("hunspell")
        .args(["-i", "UTF-8", "-d"])
        .arg(&base)
        .arg("-l")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping: can't run hunspell: {e}");
            return;
        }
    };
    let input = sample.join("\n") + "\n";
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    let Ok(out) = child.wait_with_output() else { return };
    let wrong: std::collections::HashSet<&str> = std::str::from_utf8(&out.stdout).unwrap_or("").lines().collect();
    let diffs: Vec<(&String, bool)> =
        sample.iter().filter(|w| d.check(w) == wrong.contains(w.as_str())).map(|w| (w, d.check(w))).collect();
    // The only known difference: Hunspell reads non-BMP UTF-8 flags (emoji
    // flags in es_ES) as UTF-16 halves that collide; we don't.
    assert!(diffs.len() * 200 <= sample.len(), "{} of {} differ from hunspell: {diffs:?}", diffs.len(), sample.len());
}

#[test]
fn en_us_matches_hunspell_command() {
    compare_with_hunspell("en_US");
}

#[test]
fn es_es_matches_hunspell_command() {
    compare_with_hunspell("es_ES");
}

fn grammar(text: &str, lang: &str, d: &Dictionary) -> Vec<(&'static str, String, Vec<String>)> {
    apex_spell::check_grammar(text, lang, Some(d)).into_iter().map(|i| (i.rule, text[i.range].to_string(), i.suggestions)).collect()
}

#[test]
fn es_es_agreement() {
    let Some(d) = load("es_ES") else { return };
    let one = |t: &str| {
        let r = grammar(t, "es_ES", d);
        assert_eq!(r.len(), 1, "{t}: {r:?}");
        let (rule, text, s) = r.into_iter().next().unwrap();
        assert_eq!(rule, "es_agreement", "{t}");
        (text, s)
    };
    assert_eq!(one("Vi los problema"), ("los problema".into(), vec!["los problemas".into(), "el problema".into()]));
    assert_eq!(one("Pinté el casa"), ("el casa".into(), vec!["la casa".into()]));
    assert_eq!(one("Hay una problema"), ("una problema".into(), vec!["un problema".into()]));
    assert_eq!(one("Cayó en las casa"), ("las casa".into(), vec!["las casas".into(), "la casa".into()]));
    assert_eq!(one("Cayó en la agua"), ("la agua".into(), vec!["el agua".into()]));
    assert_eq!(one("Mira el niñas"), ("el niñas".into(), vec!["las niñas".into()]));
    assert_eq!(one("Vino del ciudades"), ("del ciudades".into(), vec!["de las ciudades".into()]));
    // Right, or a pronoun before a verb.
    for t in [
        "El agua fría y un hacha. Esta agua.", "Juan las casa mañana.", "Los lunes y las crisis.", "La mano, el día, el problema, la foto.", "Las once y los cuatro.", "La presidente y el poeta.", "Los que vienen y las que no.",
        "Al comer, el deber.", "Los mismos niños y las otras niñas.", "Mis casas y su casa.", "Un buen día.", "La ciudad, la canción, el avión.",
    ] {
        assert!(grammar(t, "es_ES", d).is_empty(), "{t}: {:?}", grammar(t, "es_ES", d));
    }
}

#[test]
fn es_es_verbs() {
    let Some(d) = load("es_ES") else { return };
    assert_eq!(grammar("Llámame sino vienes.", "es_ES", d), [("es_sino", "sino".into(), vec!["si no".into()])]);
    assert!(grammar("No quiero comer sino dormir. No es rojo sino azul.", "es_ES", d).is_empty());
    assert_eq!(grammar("Creo de que sí.", "es_ES", d), [("es_dequeismo", "de que".into(), vec!["que".into()])]);
    assert_eq!(grammar("Pensaron de que sí.", "es_ES", d), [("es_dequeismo", "de que".into(), vec!["que".into()])]);
    assert!(grammar("Se acordó de que sí. Trata de que vengan.", "es_ES", d).is_empty());
    assert_eq!(grammar("Debió a ver llegado.", "es_ES", d), [("es_a_ver", "a ver".into(), vec!["haber".into()])]);
}

/// Real Spanish text, the program's own translation, has few false alarms.
#[test]
fn es_es_few_false_alarms() {
    let Some(d) = load("es_ES") else { return };
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../apex-app/i18n/es.txt")).unwrap();
    let mut n = 0;
    let mut hits = Vec::new();
    for line in text.lines() {
        let Some((_, v)) = line.split_once(" = ") else { continue };
        n += 1;
        for i in apex_spell::check_grammar(v, "es_ES", Some(d)) {
            hits.push(format!("{}: {} [{}]", i.rule, &v[i.range], v));
        }
    }
    for h in &hits {
        eprintln!("{h}");
    }
    assert!(n > 1000 && hits.len() * 200 < n, "{} alarms in {n} strings", hits.len());
}
