use super::*;

const DAY: i64 = 86_400;

fn sample() -> (Notebook, i64, i64, i64, i64) {
    let mut nb = Notebook::default();
    let ideas = nb.add_section("Ideas", 0);
    let chars = nb.add_section("Personajes", 0);
    let a = nb.add_page(ideas, None, None, "Trama", 0);
    let b = nb.add_page(ideas, None, None, "Escenarios", 0);
    nb.texts.insert(a, "Ana llega al puerto de noche.".into());
    nb.texts.insert(b, "El puerto, la lonja y el faro.".into());
    (nb, ideas, chars, a, b)
}

#[test]
fn sections_and_pages_keep_their_order() {
    let (mut nb, ideas, chars, a, b) = sample();
    let c = nb.add_page(ideas, None, Some(a), "Final", 0);
    let titles = |nb: &Notebook| nb.tree(ideas).iter().map(|(d, p)| format!("{d}{}", p.title)).collect::<Vec<_>>();
    assert_eq!(titles(&nb), ["0Trama", "0Final", "0Escenarios"]);
    // A subpage, then its page moved with it to another section.
    let sub = nb.add_page(ideas, Some(b), None, "Faro", 0);
    assert_eq!(titles(&nb), ["0Trama", "0Final", "0Escenarios", "1Faro"]);
    assert!(!nb.move_page(b, ideas, Some(sub), None));
    assert!(nb.move_page(b, chars, None, None));
    assert_eq!(nb.page(sub).unwrap().section, chars);
    assert_eq!(nb.tree(chars).len(), 2);
    // Before another page; under a page.
    assert!(nb.move_page(c, ideas, None, Some(a)));
    assert_eq!(titles(&nb), ["0Final", "0Trama"]);
    assert!(nb.move_page(c, ideas, Some(a), None));
    assert_eq!(titles(&nb), ["0Trama", "1Final"]);
    nb.move_section(chars, Some(ideas));
    assert_eq!(nb.live_sections().iter().map(|s| s.id).collect::<Vec<_>>(), [chars, ideas]);
}

#[test]
fn the_trash_keeps_things_thirty_days() {
    let (mut nb, ideas, _, a, b) = sample();
    let sub = nb.add_page(ideas, Some(a), None, "Detalle", 0);
    nb.delete_page(a, 10 * DAY);
    assert!(!nb.is_live(sub) && nb.is_live(b));
    assert_eq!(nb.trash(), [(Trashed::Page(a), 10 * DAY)]);
    assert_eq!(nb.search("puerto").len(), 1);
    nb.restore(Trashed::Page(a));
    assert!(nb.is_live(sub));
    // A section's pages go with it.
    nb.delete_section(ideas, 20 * DAY);
    assert!(!nb.is_live(a));
    assert_eq!(nb.trash(), [(Trashed::Section(ideas), 20 * DAY)]);
    assert!(nb.purge(49 * DAY, None).is_empty());
    let gone = nb.purge(51 * DAY, None);
    assert_eq!(gone.len(), 3);
    assert!(nb.section(ideas).is_none() && nb.texts.is_empty());
}

#[test]
fn a_page_whose_parent_is_gone_comes_back_at_the_top() {
    let (mut nb, ideas, _, a, _) = sample();
    let sub = nb.add_page(ideas, Some(a), None, "Detalle", 0);
    nb.delete_page(sub, DAY);
    nb.delete_page(a, 2 * DAY);
    nb.restore(Trashed::Page(sub));
    assert!(nb.is_live(sub));
    assert_eq!(nb.page(sub).unwrap().parent, None);
}

#[test]
fn search_finds_every_word_without_accents() {
    let (mut nb, _, _, a, b) = sample();
    let hits = nb.search("PUERTO");
    assert_eq!(hits.len(), 2);
    let h = &hits[0];
    assert_eq!(&h.snippet[h.mark.unwrap().0..h.mark.unwrap().1], "puerto");
    assert_eq!(nb.search("puerto faro").iter().map(|h| h.page).collect::<Vec<_>>(), [b]);
    // The title first.
    nb.page_mut(b).unwrap().title = "Puerto".into();
    assert_eq!(nb.search("puerto")[0].page, b);
    nb.texts.insert(a, "Él camina por la ESTACIÓN".into());
    let h = &nb.search("estacion")[0];
    assert_eq!(&h.snippet[h.mark.unwrap().0..h.mark.unwrap().1], "ESTACIÓN");
    assert!(nb.search("  ").is_empty());
}

#[test]
fn links_say_what_they_mean_and_find_their_way_back() {
    use super::links::*;
    let (mut nb, ideas, _, a, b) = sample();
    let c = nb.add_page(ideas, None, None, "El faro", 0);
    nb.page_mut(c).unwrap().aliases = vec!["Faro de Cabo".into()];
    assert_eq!(parse(&target(c, Rel::Supports)), Some((c, Rel::Supports)));
    assert_eq!(parse(&target(c, Rel::Link)), Some((c, Rel::Link)));
    assert_eq!(parse("https://x"), None);
    nb.links.insert(a, vec![(b, Rel::Link)]);
    nb.links.insert(b, vec![(c, Rel::Contradicts)]);
    assert_eq!(nb.backlinks(b), [(a, Rel::Link)]);
    let rel = nb.related(a);
    assert_eq!(rel[0], Related { page: b, rel: Rel::Link, via: None });
    assert_eq!(rel[1], Related { page: c, rel: Rel::Contradicts, via: Some(b) });
    // "el faro" is in Escenarios' text, but it already links there.
    assert!(nb.mentions(c).is_empty());
    nb.texts.insert(a, "Ana ve el Faro desde el puerto.".into());
    assert_eq!(nb.mentions(c), [a]);
    assert_eq!(nb.find_pages("faro"), [c]);
    assert_eq!(nb.find_pages("cabo"), [c]);
    assert_eq!(nb.page_named("faro de cabo"), Some(c));
    // Pages in the trash have no links.
    nb.delete_page(b, 0);
    assert!(nb.backlinks(c).is_empty() && nb.outgoing(a).is_empty());
}

#[test]
fn links_in_a_page_follow_its_title() {
    use super::links::*;
    use crate::{Document, Paragraph, Pos, Selection};
    let mut doc = Document::from_paragraphs(vec![Paragraph::new("Ver ")]);
    let end = Pos::new(0, 4);
    doc.insert_link(Selection { anchor: end, cursor: end }, "Trama", target(7, Rel::Expands), None);
    assert_eq!(page_links(&doc), [(7, Rel::Expands)]);
    assert!(retitle_links(&mut doc, 7, "Trama", "La trama"));
    assert_eq!(doc.paragraphs[0].text, "Ver La trama");
    assert_eq!(page_links(&doc), [(7, Rel::Expands)]);
    let mut doc = Document::from_paragraphs(vec![Paragraph::new("El faro de noche; el faro.")]);
    assert!(link_mention(&mut doc, "Faro", 9));
    assert_eq!(doc.link_at(Pos::new(0, 4)).map(|l| (l.0.start().offset, l.0.end().offset)), Some((3, 7)));
    assert!(!link_mention(&mut doc, "Lonja", 9));
}

#[test]
fn tags_have_levels() {
    use super::links::tags_in;
    assert_eq!(tags_in("#lugar/costa y #idea, no es#esto ni #1 ni C#"), ["lugar/costa", "idea"]);
    let (mut nb, _, _, a, b) = sample();
    nb.texts.insert(a, "#lugar/costa #Idea".into());
    nb.texts.insert(b, "#lugar/monte".into());
    let tags = nb.tags();
    assert_eq!(tags.keys().collect::<Vec<_>>(), ["idea", "lugar", "lugar/costa", "lugar/monte"]);
    assert_eq!(nb.tagged("lugar"), [a, b]);
}

#[test]
fn a_canvas_holds_things_and_its_arrows_link_pages() {
    use super::canvas::*;
    let (mut nb, ideas, _, a, b) = sample();
    let board = nb.add_page(ideas, None, None, "Lienzo", 0);
    nb.page_mut(board).unwrap().kind = PageKind::Canvas;
    let mut c = Canvas::default();
    let g = c.add(ItemKind::Group("Costa".into()), [0.0, 0.0, 400.0, 300.0], NOTE_COLORS[2]);
    let ca = c.add(ItemKind::Page(a), [20.0, 40.0, 200.0, 120.0], 0);
    let cb = c.add(ItemKind::Page(b), [500.0, 40.0, 200.0, 120.0], 0);
    let n = c.add(ItemKind::Note("Ojo con la marea".into()), [20.0, 200.0, 150.0, 80.0], NOTE_COLORS[0]);
    assert_eq!(c.items[0].id, g);
    assert_eq!(c.inside(g), [ca, n]);
    assert_eq!(c.item_at(30.0, 50.0), Some(ca));
    assert_eq!(c.item_at(300.0, 20.0), Some(g));
    assert!(c.connect(ca, cb, Rel::Supports).is_some());
    assert!(c.connect(ca, cb, Rel::Link).is_none() && c.connect(ca, ca, Rel::Link).is_none());
    c.connect(n, cb, Rel::Link);
    assert_eq!(c.page_links(board), [(a, b, Rel::Supports)]);
    assert_eq!(c.text(), "Costa\nOjo con la marea");
    nb.canvas_links.insert(board, c.page_links(board));
    assert_eq!(nb.outgoing(a), [(b, Rel::Supports)]);
    assert_eq!(nb.backlinks(b), [(a, Rel::Supports)]);
    // The canvas in the trash: no more links.
    nb.delete_page(board, 0);
    assert!(nb.outgoing(a).is_empty());
    nb.restore(Trashed::Page(board));
    c.remove(&[cb]);
    assert!(c.arrows.is_empty() && c.items.len() == 3);
}

#[test]
fn a_hypothesis_counts_the_pages_for_and_against_it() {
    let mut nb = Notebook::default();
    let s = nb.add_section("Ideas", 0);
    let h = nb.add_page(s, None, None, "El faro se apagó por la tormenta", 0);
    let yes = nb.add_page(s, None, None, "Parte del puerto", 0);
    let no = nb.add_page(s, None, None, "Diario del farero", 0);
    let aside = nb.add_page(s, None, None, "Mapa", 0);
    nb.page_mut(h).unwrap().claim = Some(Claim::new(ClaimKind::Hypothesis));
    nb.links.insert(yes, vec![(h, Rel::Supports)]);
    nb.links.insert(no, vec![(h, Rel::Contradicts)]);
    nb.links.insert(aside, vec![(h, Rel::Link)]);
    assert_eq!(nb.evidence(h), Evidence { supports: vec![yes], contradicts: vec![no] });
    assert_eq!(nb.claims(), [h]);
    nb.delete_page(no, 1);
    assert!(nb.evidence(h).contradicts.is_empty());
    let c = Claim { kind: ClaimKind::Question, status: Status::Supported };
    assert_eq!(Claim::from_key(&c.key()), Some(c));
    assert_eq!(Claim::from_key(""), None);
}

#[test]
fn a_task_keeps_its_date_and_priority_in_its_tag() {
    use tasks::*;
    let m = TaskMeta { due: Some("2026-10-05".into()), priority: 2, col: None };
    assert_eq!(TaskMeta::from_tag(&m.tag()), m);
    assert_eq!(TaskMeta::from_tag("due=mañana;pri=9"), TaskMeta { due: None, priority: TOP_PRIORITY, col: None });
    assert_eq!(TaskMeta::default().tag(), "");
    let mut doc = crate::Document::from_plain_text("Texto\nRevisar mapa\n");
    let c = crate::ContentControl::new(1, crate::ControlKind::CheckBox { checked: false });
    doc.insert_control(crate::Selection::caret(crate::Pos::new(1, 0)), c, &crate::UNCHECKED.to_string());
    let t = tasks_in(&doc);
    assert_eq!((t.len(), t[0].para, t[0].done, t[0].text.as_str()), (1, 1, false, "Revisar mapa"));
}

#[test]
fn a_name_is_found_under_a_place_in_the_text() {
    let names: Vec<String> = ["ana", "ana soler", "faro", "isla"].map(String::from).to_vec();
    let text = "Llega Ana Soler al Faro, y Ana mira la isla de Anabel.";
    let at = |w: &str| text.find(w).unwrap();
    assert_eq!(links::name_at(text, at("Soler"), &names), Some((1, at("Ana Soler")..at(" al"))));
    assert_eq!(links::name_at(text, at("aro,"), &names).map(|n| n.0), Some(2));
    assert_eq!(links::name_at(text, at("Ana mira"), &names).map(|n| n.0), Some(0));
    assert_eq!(links::name_at(text, at("Anabel"), &names), None, "whole words only");
    assert_eq!(links::name_at(text, at("Llega"), &names), None);
}

#[test]
fn a_map_stays_behind_and_its_markers_link_their_pages() {
    use super::canvas::*;
    let (mut nb, ideas, _, a, b) = sample();
    let map = nb.add_page(ideas, None, None, "Mapa", 0);
    nb.page_mut(map).unwrap().kind = PageKind::Canvas;
    let mut c = Canvas::default();
    let g = c.add(ItemKind::Group("Costa".into()), [0.0, 0.0, 400.0, 300.0], NOTE_COLORS[2]);
    let m = c.set_map(vec![1u8, 2].into(), [-100.0, -100.0, 1000.0, 800.0]);
    let pa = c.add(ItemKind::Pin(a), [90.0, 60.0, 20.0, 40.0], 0);
    c.add(ItemKind::Pin(a), [190.0, 60.0, 20.0, 40.0], 0);
    let pb = c.add(ItemKind::Pin(b), [590.0, 460.0, 20.0, 40.0], 0);
    assert_eq!((c.items[0].id, c.items[1].id), (m, g), "the map behind the groups");
    assert_eq!(c.item_at(100.0, 80.0), Some(pa));
    assert_eq!(c.item_at(800.0, 600.0), None, "the map is not picked");
    assert_eq!(c.pinned(), [a, b]);
    c.connect(pa, pb, Rel::Causes);
    assert_eq!(c.page_links(map), [(a, b, Rel::Causes), (map, a, Rel::Link), (map, b, Rel::Link)]);
    nb.canvas_links.insert(map, c.page_links(map));
    assert!(nb.backlinks(b).contains(&(map, Rel::Link)));
    // Another picture takes the map's place.
    let m2 = c.set_map(vec![3u8].into(), [0.0, 0.0, 10.0, 10.0]);
    assert!(c.item(m).is_none() && c.map().map(|i| i.id) == Some(m2) && c.items[0].id == m2);
}

#[test]
fn a_file_with_a_loop_of_parents_gets_a_tree() {
    let mut nb = Notebook::default();
    let s = nb.add_section("Ideas", 0);
    let other = nb.add_section("Otra", 0);
    let a = nb.add_page(s, None, None, "A", 0);
    let b = nb.add_page(s, Some(a), None, "B", 0);
    let c = nb.add_page(s, None, None, "C", 0);
    let d = nb.add_page(other, None, None, "D", 0);
    // As a crafted file could have them: A under B under A, C under
    // itself, D under a page of another section.
    nb.page_mut(a).unwrap().parent = Some(b);
    nb.page_mut(c).unwrap().parent = Some(c);
    nb.page_mut(d).unwrap().parent = Some(a);
    nb.fix_parents();
    assert_eq!(nb.page(a).unwrap().parent, None);
    assert_eq!(nb.page(b).unwrap().parent, Some(a));
    assert_eq!(nb.page(c).unwrap().parent, None);
    assert_eq!(nb.page(d).unwrap().parent, None);
    assert!(nb.is_live(b));
    assert_eq!(nb.subtree(a), [a, b]);
}
