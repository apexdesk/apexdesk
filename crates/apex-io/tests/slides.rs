//! Presentations: .pptx read and written back, as they were and after
//! edits, with every part accounted for. The files in data/pptx come
//! from python-pptx's tests (MIT license).

use std::collections::HashSet;

use apex_io::pptx::{self, PptFlavor};
use apex_io::xml::{Ev, Reader};
use apex_io::zip::ZipReader;
use apex_slides::{Deck, Notes, TextBody, build, edit};

const FILES: &[&str] = &["sld-notes", "tbl-cell", "shp-groupshape", "shp-embedded-pptx", "txt-font-props", "sld-background", "test"];

fn sample(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/pptx/{name}.pptx", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Every part has a content type, every relationship a part and every
/// XML part is well formed.
fn check_package(data: &[u8]) {
    let z = ZipReader::new(data).unwrap();
    let names: Vec<String> = z.names().map(String::from).collect();
    let cts = z.read_string("[Content_Types].xml").unwrap();
    let (mut defaults, mut overrides) = (Vec::new(), Vec::new());
    let mut r = Reader::new(&cts);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Empty(t) | Ev::Start(t) if t.local() == "Default" => defaults.push(t.attr("Extension").unwrap().to_lowercase()),
            Ev::Empty(t) | Ev::Start(t) if t.local() == "Override" => overrides.push(t.attr("PartName").unwrap().trim_start_matches('/').to_string()),
            _ => {}
        }
    }
    let mut seen = HashSet::new();
    for o in &overrides {
        assert!(names.contains(o), "override of a missing part {o}");
        assert!(seen.insert(o.clone()), "override twice: {o}");
    }
    for n in names.iter().filter(|n| *n != "[Content_Types].xml") {
        let ext = n.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
        assert!(overrides.contains(n) || defaults.contains(&ext), "no content type for {n}");
        if ext == "xml" || ext == "rels" {
            let x = z.read_string(n).unwrap();
            let mut q = quick_xml::Reader::from_str(&x);
            loop {
                match q.read_event() {
                    Ok(quick_xml::events::Event::Eof) => break,
                    Ok(_) => {}
                    Err(e) => panic!("{n} is not well formed: {e}"),
                }
            }
        }
    }
    for n in names.iter().filter(|n| n.ends_with(".rels")) {
        let dir = n.trim_end_matches(".rels").rsplit_once("_rels/").map_or(String::new(), |(d, _)| d.trim_end_matches('/').to_string());
        let xml = z.read_string(n).unwrap();
        let mut ids = HashSet::new();
        let mut r = Reader::new(&xml);
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Empty(t) | Ev::Start(t) if t.local() == "Relationship" => {
                    assert!(ids.insert(t.attr("Id").unwrap().into_owned()), "id twice in {n}");
                    if t.attr("TargetMode").is_some_and(|m| m == "External") {
                        continue;
                    }
                    let target = t.attr("Target").unwrap();
                    let mut parts: Vec<&str> = if target.starts_with('/') { Vec::new() } else { dir.split('/').filter(|s| !s.is_empty()).collect() };
                    for seg in target.split('/') {
                        match seg {
                            ".." => {
                                parts.pop();
                            }
                            "" | "." => {}
                            s => parts.push(s),
                        }
                    }
                    let path = parts.join("/");
                    assert!(names.contains(&path), "{n} points at a missing part {path}");
                }
                _ => {}
            }
        }
    }
}

/// Slides, objects and text: what must survive.
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

fn touch_all(d: &mut Deck) {
    for i in 0..d.slides.len() {
        let s = d.slide_mut(i);
        for o in &mut s.objects {
            o.walk_mut(&mut |o| o.touch());
        }
    }
    for m in 0..d.masters.len() {
        d.master_mut(m).theme.origin = None;
        for l in 0..d.masters[m].layouts.len() {
            d.layout_mut(m, l);
        }
    }
}

#[test]
fn samples_round_trip_unchanged() {
    for name in FILES {
        let data = sample(name);
        let deck = pptx::read(&data).unwrap();
        assert!(!deck.slides.is_empty(), "{name}: no slides");
        let out = pptx::write(&deck);
        check_package(&out);
        // Parts nobody changed go back byte for byte.
        let (a, b) = (ZipReader::new(&data).unwrap(), ZipReader::new(&out).unwrap());
        for n in a.names().filter(|n| n.starts_with("ppt/slides/slide") || n.starts_with("ppt/media/")) {
            assert_eq!(a.read(n).unwrap(), b.read(n).unwrap_or_default(), "{name}: {n} changed");
        }
        let back = pptx::read(&out).unwrap();
        assert_eq!(summary(&deck), summary(&back), "{name}");
    }
}

#[test]
fn samples_round_trip_rewritten() {
    for name in FILES {
        let mut deck = pptx::read(&sample(name)).unwrap();
        let want = summary(&deck);
        touch_all(&mut deck);
        let out = pptx::write(&deck);
        check_package(&out);
        let back = pptx::read(&out).unwrap();
        assert_eq!(want, summary(&back), "{name}");
        assert_eq!(deck.masters.len(), back.masters.len(), "{name}");
        assert_eq!(deck.masters[0].layouts.len(), back.masters[0].layouts.len(), "{name}");
    }
}

#[test]
fn new_deck_round_trip() {
    let mut deck = build::new_deck(0);
    let i = edit::add_slide(&mut deck, 1, 0, 1);
    let s = deck.slide_mut(i);
    s.notes = Some(Notes { text: TextBody::plain("Speaker notes"), origin: None });
    for o in &mut s.objects {
        if let Some(t) = o.text_or_new() {
            *t = TextBody::plain("Hello slides");
            break;
        }
    }
    let id = edit::fresh_guid(&deck, 1);
    edit::add_section(&mut deck, 0, "Intro", id);
    for flavor in [PptFlavor::default(), PptFlavor { template: true, ..Default::default() }, PptFlavor { show: true, ..Default::default() }] {
        let out = pptx::write_as(&deck, flavor);
        check_package(&out);
        let back = pptx::read(&out).unwrap();
        assert_eq!(back.slides.len(), 2);
        assert_eq!(summary(&deck), summary(&back));
        assert_eq!(back.sections.len(), 1);
        assert_eq!(back.sections[0].name, "Intro");
        assert_eq!(back.sections[0].slides.len(), 2);
        assert_eq!(back.masters[0].layouts.len(), deck.masters[0].layouts.len());
        assert_eq!(back.slides[1].layout, 1);
        assert!((back.width - deck.width).abs() < 0.01);
    }
    let ct = ZipReader::new(&pptx::write_as(&deck, PptFlavor { template: true, ..Default::default() })).unwrap().read_string("[Content_Types].xml").unwrap();
    assert!(ct.contains("presentationml.template.main+xml"));
}

#[test]
fn edits_keep_the_rest() {
    let data = sample("test");
    let mut d = pptx::read(&data).unwrap();
    if d.slides.len() < 2 {
        edit::add_slide(&mut d, 1, 0, 1);
    }
    let n = d.slides.len();
    // A copy of the first slide at the end, the second deleted.
    edit::duplicate_slides(&mut d, &[0]);
    let last = d.slides.len() - 1;
    edit::move_slides(&mut d, &[1], last + 1);
    edit::delete_slides(&mut d, &[0]);
    let out = pptx::write(&d);
    check_package(&out);
    let back = pptx::read(&out).unwrap();
    assert_eq!(summary(&d), summary(&back));
    assert_eq!(back.slides.len(), n);
    // Slide ids are kept, the copy got a new one.
    let ids: Vec<u32> = d.slides.iter().map(|s| s.id).collect();
    assert_eq!(ids, back.slides.iter().map(|s| s.id).collect::<Vec<_>>());
}

#[test]
fn macro_project_written_into_pptm() {
    // A new project, as the macro editor makes it, goes into a .pptm.
    let mut deck = build::new_deck(0);
    let bin = apex_io::vba::new_presentation_project();
    deck.macros.set_project(bin.clone());
    let data = pptx::write_as(&deck, PptFlavor { macros: true, ..Default::default() });
    check_package(&data);
    let back = pptx::read(&data).unwrap();
    assert_eq!(back.macros.project.as_deref(), Some(&bin));
    // Edited again, the new code replaces the one read.
    let mut edited = back.clone();
    let mut p = apex_io::vba::read(&bin).unwrap();
    let mut m = apex_io::vba::VbaModule::standard("Module1");
    m.code = "Sub Hola()\nEnd Sub\n".into();
    p.modules.push(m);
    edited.macros.set_project(p.write().unwrap());
    let again = pptx::read(&pptx::write_as(&edited, PptFlavor { macros: true, ..Default::default() })).unwrap();
    let code = apex_io::vba::read(again.macros.project.as_deref().unwrap()).unwrap();
    assert!(code.modules[0].code.contains("Sub Hola"));
    // As .pptx the project goes.
    let plain = pptx::read(&pptx::write_as(&edited, PptFlavor::default())).unwrap();
    assert!(plain.macros.project.is_none());
}
