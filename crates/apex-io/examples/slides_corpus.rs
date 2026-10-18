//! Round trip check of presentations: `slides_corpus <dir|file>...` reads
//! every .pptx given or under the folders, writes it back as it is and with every
//! part written again, re-reads both and reports what changed (slides,
//! objects, text), errors and panics. With `SLIDES_OUT` set the written
//! files are kept in that folder to open them elsewhere.

use std::path::{Path, PathBuf};

use apex_slides::Deck;

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["pptx", "pptm", "potx", "ppsx"].contains(&e.to_ascii_lowercase().as_str())) {
            out.push(p);
        }
    }
}

/// What must survive: slides, objects and text of each.
fn summary(d: &Deck) -> Vec<(usize, String)> {
    d.slides
        .iter()
        .map(|s| {
            let mut n = 0;
            let mut text = String::new();
            for o in &s.objects {
                o.walk(&mut |_| n += 1);
                for t in o.text_bodies() {
                    text.push_str(&t.text());
                    text.push('|');
                }
            }
            if let Some(nt) = &s.notes {
                text.push_str(&nt.text.text());
            }
            (n, text)
        })
        .collect()
}

/// Every part and object marked changed, so all is written again.
fn touch_all(d: &mut Deck) {
    for i in 0..d.slides.len() {
        let s = d.slide_mut(i);
        for o in &mut s.objects {
            o.walk_mut(&mut |o| o.touch());
        }
        if let Some(n) = &mut s.notes {
            n.origin = None;
        }
    }
    for m in 0..d.masters.len() {
        let ms = d.master_mut(m);
        ms.theme.origin = None;
        for o in &mut ms.objects {
            o.walk_mut(&mut |o| o.touch());
        }
        for l in 0..d.masters[m].layouts.len() {
            let lay = d.layout_mut(m, l);
            for o in &mut lay.objects {
                o.walk_mut(&mut |o| o.touch());
            }
        }
    }
}

fn check(p: &Path, out: Option<&Path>) -> Result<(), String> {
    let data = std::fs::read(p).map_err(|e| e.to_string())?;
    let deck = apex_io::pptx::read(&data).map_err(|e| format!("read: {e}"))?;
    let want = summary(&deck);
    let mut touched = deck.clone();
    touch_all(&mut touched);
    for (tag, d) in [("same", &deck), ("all", &touched)] {
        let bytes = apex_io::pptx::write(d);
        if let Some(o) = out {
            let name = format!("{}.{tag}.pptx", p.file_stem().unwrap_or_default().to_string_lossy());
            let _ = std::fs::write(o.join(name), &bytes);
        }
        let back = apex_io::pptx::read(&bytes).map_err(|e| format!("{tag}: reread: {e}"))?;
        let got = summary(&back);
        if got.len() != want.len() {
            return Err(format!("{tag}: {} slides -> {}", want.len(), got.len()));
        }
        for (i, (a, b)) in want.iter().zip(&got).enumerate() {
            if a.0 != b.0 {
                return Err(format!("{tag}: slide {}: {} objects -> {}", i + 1, a.0, b.0));
            }
            if a.1 != b.1 {
                // Around the first difference.
                let at = a.1.chars().zip(b.1.chars()).take_while(|(x, y)| x == y).count().saturating_sub(20);
                let cut = |s: &str| s.chars().skip(at).take(60).collect::<String>();
                return Err(format!("{tag}: slide {}: text {:?} -> {:?}", i + 1, cut(&a.1), cut(&b.1)));
            }
        }
    }
    Ok(())
}

fn main() {
    // Folders or files; `SLIDES_OUT` keeps what is written.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: slides_corpus <dir|file>...");
        std::process::exit(2);
    }
    let out = std::env::var_os("SLIDES_OUT").map(PathBuf::from);
    if let Some(o) = &out {
        let _ = std::fs::create_dir_all(o);
    }
    let mut list = Vec::new();
    for a in &args {
        if Path::new(a).is_file() {
            list.push(PathBuf::from(a));
        }
        files(Path::new(a), &mut list);
    }
    list.sort();
    let (mut ok, mut bad) = (0, 0);
    for p in &list {
        let r = std::panic::catch_unwind(|| check(p, out.as_deref()));
        match r {
            Ok(Ok(())) => ok += 1,
            Ok(Err(e)) => {
                bad += 1;
                println!("FAIL {}: {e}", p.display());
            }
            Err(e) => {
                bad += 1;
                let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                println!("PANIC {}: {msg}", p.display());
            }
        }
    }
    println!("{ok} ok, {bad} failed of {}", list.len());
}
