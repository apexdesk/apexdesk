//! Pages shown inside pages (`![[Page]]`, `![[Page#Heading]]`): a copy of
//! the page, or of the part under one of its headings, kept in a locked
//! control whose tag says where it comes from, and brought up to date
//! when the page holding it opens.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use super::fold;
use crate::{ContentControl, ControlKind, Document, ParaKind, Paragraph};

/// The bar beside an embed and its background (0xRRGGBB).
const BAR: u32 = 0x8A63D2;
const TINT: u32 = 0xF5F2FB;

/// The tag of an embed's control: "embed:12" or "embed:12#Heading".
pub const EMBED: &str = "embed:";

pub fn tag(page: i64, heading: Option<&str>) -> String {
    match heading.map(str::trim).filter(|h| !h.is_empty()) {
        Some(h) => format!("{EMBED}{page}#{h}"),
        None => format!("{EMBED}{page}"),
    }
}

/// The page and heading an embed's tag names.
pub fn parse(tag: &str) -> Option<(i64, Option<String>)> {
    let rest = tag.strip_prefix(EMBED)?;
    let (id, heading) = rest.split_once('#').map_or((rest, None), |(i, h)| (i, Some(h.to_string())));
    Some((id.parse().ok()?, heading))
}

fn level(p: &Paragraph) -> Option<u8> {
    match p.style.kind {
        ParaKind::Heading(n) => Some(n),
        _ => None,
    }
}

/// The headings of `doc`, to embed the part under one.
pub fn headings(doc: &Document) -> Vec<String> {
    doc.paragraphs.iter().filter(|p| level(p).is_some() && !p.text.trim().is_empty()).map(|p| p.text.trim().to_string()).collect()
}

/// What an embed shows of `doc`: all of it, or the heading named and
/// what's under it until a heading as high. None: no such heading.
pub fn excerpt(doc: &Document, heading: Option<&str>) -> Option<Vec<Paragraph>> {
    let paras = &doc.paragraphs;
    let Some(h) = heading else {
        let mut v = paras.clone();
        while v.last().is_some_and(|p| p.text.trim().is_empty() && p.cell.is_none()) {
            v.pop();
        }
        return Some(v);
    };
    let at = paras.iter().position(|p| level(p).is_some() && fold(p.text.trim()) == fold(h.trim()))?;
    let top = level(&paras[at]).unwrap_or(1);
    let end = paras[at + 1..].iter().position(|p| level(p).is_some_and(|l| l <= top)).map_or(paras.len(), |i| at + 1 + i);
    Some(paras[at..end].to_vec())
}

/// `paras` held in control `c` (whatever controls they had are dropped).
pub fn wrap(paras: Vec<Paragraph>, c: &ContentControl) -> Vec<Paragraph> {
    let c = Arc::new(c.clone());
    let mut v: Vec<Paragraph> = paras
        .into_iter()
        .map(|mut p| {
            p.marks.clear();
            for r in &mut p.runs {
                r.attrs.control = Some(c.clone());
            }
            // A bar on the left and a tint tell it from the page's own text.
            if p.cell.is_none() {
                p.style.borders.left = Some(crate::Line::new(crate::LineStyle::Single, 2.25, BAR, 8.0));
                p.style.shading = Some(TINT);
            }
            p
        })
        .collect();
    if v.is_empty() {
        let mut p = Paragraph::new(" ");
        p.runs[0].attrs.control = Some(c);
        v.push(p);
    }
    v
}

/// The control of an embed: locked, its tab naming the page.
pub fn control(id: u32, page: i64, heading: Option<&str>, title: &str) -> ContentControl {
    let mut c = ContentControl::new(id, ControlKind::RichText);
    c.tag = tag(page, heading);
    c.title = match heading {
        Some(h) => format!("↪ {title} › {h}"),
        None => format!("↪ {title}"),
    };
    c.lock_content = true;
    c
}

/// The embed a paragraph is part of: its control.
fn embed_of(p: &Paragraph) -> Option<&Arc<ContentControl>> {
    p.runs.iter().filter_map(|r| r.attrs.control.as_ref()).find(|c| c.tag.starts_with(EMBED))
}

/// The embeds of `doc`: the paragraphs each takes and its control. Lines
/// next to each other with the same tag are one (a file read again may
/// have a control per line, and none on empty lines).
pub fn embeds(doc: &Document) -> Vec<(Range<usize>, Arc<ContentControl>)> {
    let mut out: Vec<(Range<usize>, Arc<ContentControl>)> = Vec::new();
    for (i, p) in doc.paragraphs.iter().enumerate() {
        let Some(c) = embed_of(p) else { continue };
        match out.last_mut() {
            Some((r, last)) if last.tag == c.tag && doc.paragraphs[r.end..i].iter().all(|p| p.text.is_empty()) => r.end = i + 1,
            _ => out.push((i..i + 1, c.clone())),
        }
    }
    out
}

/// The pages `doc` embeds, each once.
pub fn embedded_pages(doc: &Document) -> Vec<i64> {
    let mut v: Vec<i64> = Vec::new();
    for (_, c) in embeds(doc) {
        if let Some((id, _)) = parse(&c.tag)
            && !v.contains(&id)
        {
            v.push(id);
        }
    }
    v
}

/// Brings `doc`'s embeds up to date: `page` gives a page's title and
/// text (None: gone, and the copy stays). Whether anything changed.
pub fn refresh(doc: &mut Document, page: &mut dyn FnMut(i64) -> Option<(String, Document)>) -> bool {
    // A page with many embeds (of a big page) mustn't take a copy each:
    // the first ones are kept up to date, each page read once.
    const MAX_EMBEDS: usize = 64;
    let mut changed = false;
    let mut read: HashMap<i64, Option<(String, Document)>> = HashMap::new();
    let mut all = embeds(doc);
    all.truncate(MAX_EMBEDS);
    for (range, c) in all.into_iter().rev() {
        let Some((id, heading)) = parse(&c.tag) else { continue };
        let Some((title, src)) = read.entry(id).or_insert_with(|| page(id)).as_ref() else { continue };
        let Some(paras) = excerpt(&src, heading.as_deref()) else { continue };
        let fresh = wrap(paras, &control(c.id, id, heading.as_deref(), &title));
        if doc.paragraphs[range.clone()] != fresh[..] {
            doc.paragraphs.splice(range, fresh);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(lines: &[(&str, Option<u8>)]) -> Document {
        Document::from_paragraphs(
            lines
                .iter()
                .map(|(t, h)| {
                    let mut p = Paragraph::new(*t);
                    if let Some(n) = h {
                        p.style.kind = ParaKind::Heading(*n);
                    }
                    p
                })
                .collect(),
        )
    }

    #[test]
    fn an_embed_shows_a_page_or_a_heading_and_follows_it() {
        let src = doc(&[("Intro", None), ("Torre", Some(2)), ("Alta", None), ("Escalera", Some(3)), ("Fin torre", None), ("Puerto", Some(2)), ("Barcas", None)]);
        let texts = |v: &[Paragraph]| v.iter().map(|p| p.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&excerpt(&src, Some("torre")).unwrap()), ["Torre", "Alta", "Escalera", "Fin torre"]);
        assert_eq!(excerpt(&src, Some("Nada")), None);
        assert_eq!(headings(&src), ["Torre", "Escalera", "Puerto"]);
        let mut page = doc(&[("Antes", None), ("Después", None)]);
        let c = control(7, 3, Some("Puerto"), "Faro");
        let block = wrap(excerpt(&src, Some("Puerto")).unwrap(), &c);
        page.paragraphs.splice(1..1, block);
        assert_eq!(embeds(&page).iter().map(|(r, c)| (r.clone(), c.tag.as_str())).collect::<Vec<_>>(), [(1..3, "embed:3#Puerto")]);
        assert_eq!(embedded_pages(&page), [3]);
        // The page changes, and so does the embed when the page opens.
        let newer = doc(&[("Puerto", Some(2)), ("Barcas", None), ("Redes", None), ("Otra", Some(1))]);
        assert!(refresh(&mut page, &mut |id| (id == 3).then(|| ("Faro".to_string(), newer.clone()))));
        assert_eq!(texts(&page.paragraphs), ["Antes", "Puerto", "Barcas", "Redes", "Después"]);
        assert!(!refresh(&mut page, &mut |_| Some(("Faro".to_string(), newer.clone()))), "nothing new");
        assert!(!refresh(&mut page, &mut |_| None), "a page gone leaves the copy");
        assert_eq!(page.paragraphs.len(), 5);
        // As read again from the file: a control per line, none on an
        // empty one.
        let with_gap = doc(&[("Puerto", Some(2)), ("", None), ("Barcas", None)]);
        refresh(&mut page, &mut |_| Some(("Faro".to_string(), with_gap.clone())));
        for (n, p) in page.paragraphs[1..4].iter_mut().enumerate() {
            for r in &mut p.runs {
                r.attrs.control = (n != 1).then(|| Arc::new(control(20 + n as u32, 3, Some("Puerto"), "Faro")));
            }
        }
        assert_eq!(embeds(&page).len(), 1);
        refresh(&mut page, &mut |_| Some(("Faro".to_string(), with_gap.clone())));
        assert_eq!(texts(&page.paragraphs), ["Antes", "Puerto", "", "Barcas", "Después"]);
    }
}
