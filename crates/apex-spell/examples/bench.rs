//! Load/check/suggest timings over the system dictionaries.
//!
//! cargo run --release -p apex-spell --example bench [lang...] [-- word...]
//!
//! With words after `--`, prints check/suggest results for them instead.

use std::path::PathBuf;
use std::time::Instant;

use apex_spell::Dictionary;

fn paths(lang: &str) -> Option<(PathBuf, PathBuf)> {
    apex_spell::installed().into_iter().find(|(l, _, _)| l == lang).map(|(_, a, d)| (a, d)).or_else(|| {
        let a = PathBuf::from(format!("/usr/share/hunspell/{lang}.aff"));
        let d = a.with_extension("dic");
        (a.is_file() && d.is_file()).then_some((a, d))
    })
}

/// Resident set size in KiB (Linux), to show the memory a dictionary takes.
fn rss_kib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Deterministic typos: swap, drop, double or replace one letter.
fn misspell(w: &str, i: usize) -> String {
    let mut c: Vec<char> = w.chars().collect();
    if c.len() < 3 {
        return w.to_string();
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (langs, words) = match args.iter().position(|a| a == "--") {
        Some(i) => (args[..i].to_vec(), args[i + 1..].to_vec()),
        None => (args.clone(), Vec::new()),
    };
    let langs = if langs.is_empty() { vec!["en_US".to_string(), "es_ES".to_string()] } else { langs };

    for lang in &langs {
        let Some((aff, dic)) = paths(lang) else {
            println!("{lang}: not installed");
            continue;
        };
        let rss0 = rss_kib();
        let t = Instant::now();
        let d = match Dictionary::load(&aff, &dic) {
            Ok(d) => d,
            Err(e) => {
                println!("{lang}: {e}");
                continue;
            }
        };
        let load = t.elapsed();
        let mem = rss0.zip(rss_kib()).map(|(a, b)| b.saturating_sub(a));
        println!(
            "{lang}: load {:.1} ms, {} stems, ~{} MiB",
            load.as_secs_f64() * 1e3,
            d.len(),
            mem.map(|k| format!("{:.1}", k as f64 / 1024.0)).unwrap_or("?".into())
        );

        if words.len() == 1 && (words[0] == "-" || words[0] == "-s") {
            // Batch mode for comparisons, one word per stdin line:
            // "-" prints "word\t1|0", "-s" prints "word\tsug1|sug2...".
            let mut out = String::new();
            for w in std::io::stdin().lines().map_while(Result::ok) {
                if words[0] == "-s" {
                    out.push_str(&format!("{w}\t{}\n", d.suggest(&w, 8).join("|")));
                } else {
                    out.push_str(&format!("{w}\t{}\n", d.check(&w) as u8));
                }
            }
            print!("{out}");
            continue;
        }
        if !words.is_empty() {
            for w in &words {
                let t = Instant::now();
                let ok = d.check(w);
                let tc = t.elapsed();
                let t = Instant::now();
                let s = if ok { Vec::new() } else { d.suggest(w, 8) };
                println!(
                    "  {w}: {} ({:.1} us) {:?} ({:.1} ms)",
                    if ok { "ok" } else { "WRONG" },
                    tc.as_secs_f64() * 1e6,
                    s,
                    t.elapsed().as_secs_f64() * 1e3
                );
            }
            continue;
        }

        // Sample of dictionary stems (their plain forms) plus typos of them.
        let text = std::fs::read_to_string(&dic).unwrap_or_default();
        let stems: Vec<&str> = text
            .lines()
            .skip(1)
            .step_by(37)
            .filter_map(|l| l.split('/').next())
            .map(str::trim)
            .filter(|w| w.chars().count() >= 4 && w.chars().all(char::is_alphabetic))
            .take(2000)
            .collect();
        let typos: Vec<String> = stems.iter().enumerate().map(|(i, w)| misspell(w, i)).collect();

        let t = Instant::now();
        let mut ok = 0;
        for w in &stems {
            ok += d.check(w) as usize;
        }
        for w in &typos {
            ok += d.check(w) as usize;
        }
        let n = stems.len() + typos.len();
        println!(
            "  check: {:.2} us/word avg over {n} words ({ok} accepted)",
            t.elapsed().as_secs_f64() * 1e6 / n.max(1) as f64
        );

        // Warm up (the n-gram index is built on first use).
        let t = Instant::now();
        d.suggest("qxzqxzqxz", 5);
        println!("  first n-gram suggestion (builds its index): {:.1} ms", t.elapsed().as_secs_f64() * 1e3);

        // One typo (found by the edit rules) and two typos (n-gram search).
        let doubles: Vec<String> = typos.iter().enumerate().map(|(i, w)| misspell(w, i * 7 + 3)).collect();
        for (label, set) in [("1 typo", &typos), ("2 typos", &doubles)] {
            let bad: Vec<(&String, &str)> =
                set.iter().zip(stems.iter()).filter(|(t, _)| !d.check(t)).take(300).map(|(t, o)| (t, *o)).collect();
            let mut times: Vec<f64> = Vec::new();
            let mut found = 0;
            for (w, orig) in &bad {
                let t = Instant::now();
                let s = d.suggest(w, 5);
                times.push(t.elapsed().as_secs_f64() * 1e3);
                found += s.iter().any(|x| x == orig) as usize;
            }
            times.sort_by(f64::total_cmp);
            let n = times.len().max(1);
            println!(
                "  suggest, {label}: {:.2} ms avg, {:.1} ms p95, {:.1} ms max over {} words; intended word in top 5: {:.0}%",
                times.iter().sum::<f64>() / n as f64,
                times.get(n * 95 / 100).copied().unwrap_or(0.0),
                times.last().copied().unwrap_or(0.0),
                bad.len(),
                100.0 * found as f64 / n as f64
            );
        }
    }
}
