use super::*;
use std::path::PathBuf;

fn dir(name: &str) -> PathBuf {
    let d = apex_fs::temp_dir().join(format!("apex_note_{name}_{}", apex_fs::process_id()));
    let _ = apex_fs::remove_dir_all(&d);
    apex_fs::create_dir_all(&d).unwrap();
    d
}

fn type_in(v: &mut NoteView, text: &str) {
    let ed = v.editor.as_mut().expect("a page open");
    ed.doc.insert_text(apex_doc::Selection::default(), text, None);
}

#[test]
fn a_page_typed_in_is_in_the_file_and_found() {
    let d = dir("typed");
    let path = d.join("c.apexnote");
    {
        let mut v = NoteView::create(&path).unwrap();
        assert!(v.page().is_some());
        type_in(&mut v, "La lonja abre al amanecer");
        v.new_page(false);
        type_in(&mut v, "Otra cosa");
        // Dropping it writes the last page.
    }
    let v = NoteView::open(&path).unwrap();
    assert_eq!(v.nb.tree(v.section().unwrap()).len(), 2);
    let hits = v.nb.search("LONJA");
    assert_eq!(hits.len(), 1);
    assert_eq!(v.page(), Some(hits[0].page));
    assert!(plain_text(&v.editor.as_ref().unwrap().doc).contains("amanecer"));
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_notebook_opens_in_one_place_only() {
    let d = dir("lock");
    let path = d.join("c.apexnote");
    let v = NoteView::create(&path).unwrap();
    assert!(matches!(NoteView::open(&path), Err(NoteError::Locked)));
    drop(v);
    assert!(NoteView::open(&path).is_ok());
    assert!(is_note_file(Path::new("x.APEXNOTE")) && !is_note_file(Path::new("x.docx")));
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn structure_changes_undo_and_the_trash_gives_back() {
    let d = dir("undo");
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    let first = v.page().unwrap();
    v.new_page(true);
    let sub = v.page().unwrap();
    assert_eq!(v.nb.page(sub).unwrap().parent, Some(first));
    v.delete_page(first);
    assert!(!v.nb.is_live(sub) && v.page().is_none());
    v.undo();
    assert!(v.nb.is_live(sub));
    v.redo();
    v.restore(Trashed::Page(first));
    assert!(v.nb.is_live(sub) && v.page() == Some(first));
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn the_notebook_remembers_its_cards() {
    let d = dir("cards");
    let path = d.join("c.apexnote");
    let mut v = NoteView::create(&path).unwrap();
    assert!(!v.cards);
    v.set_cards(true);
    drop(v);
    let v = NoteView::open(&path).unwrap();
    assert!(v.cards && v.grid);
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn links_to_pages_follow_renames_and_are_kept() {
    use apex_doc::notebook::{Rel, links};
    let d = dir("links");
    let path = d.join("c.apexnote");
    let (first, faro);
    {
        let mut v = NoteView::create(&path).unwrap();
        first = v.page().unwrap();
        v.new_page(false);
        faro = v.page().unwrap();
        v.rename(Naming::Page(faro), "Faro");
        v.open_page(first);
        type_in(&mut v, "Ver [[");
        let ed = v.editor.as_mut().unwrap();
        ed.put_caret(apex_doc::Pos::new(0, 6));
        ed.check_link_prompt("[");
        let at = ed.link_prompt.take().expect("[[ asks for a page");
        ed.finish_link_prompt(at, "Faro", links::target(faro, Rel::Supports));
        assert_eq!(ed.doc.paragraphs[0].text, "Ver Faro");
        v.flush();
        assert_eq!(v.nb.backlinks(faro), [(first, Rel::Supports)]);
        // Renamed: the link shows the new title, in the open page too.
        v.rename(Naming::Page(faro), "El faro");
        assert_eq!(v.editor.as_ref().unwrap().doc.paragraphs[0].text, "Ver El faro");
        v.open_page(faro);
        type_in(&mut v, "#lugar/costa");
    }
    let v = NoteView::open(&path).unwrap();
    assert_eq!(v.nb.backlinks(faro), [(first, Rel::Supports)]);
    assert_eq!(v.nb.tagged("lugar"), [faro]);
    let doc = v.store.load_page(first).unwrap();
    assert_eq!(links::page_links(&doc), [(faro, Rel::Supports)]);
    assert_eq!(doc.paragraphs[0].text, "Ver El faro");
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_canvas_page_keeps_its_cards_and_its_arrows_link_pages() {
    use apex_doc::notebook::Rel;
    let d = dir("canvas");
    let path = d.join("c.apexnote");
    let first;
    let board;
    {
        let mut v = NoteView::create(&path).unwrap();
        first = v.page().unwrap();
        v.new_page(false);
        let second = v.page().unwrap();
        v.new_canvas();
        board = v.page().unwrap();
        assert!(v.editor.is_none() && v.board_open());
        v.put_card(first, 0.0, 0.0);
        v.put_card(second, 400.0, 0.0);
        let b = v.board.as_mut().unwrap();
        let ids: Vec<i64> = b.canvas.items.iter().map(|i| i.id).collect();
        b.edit(|c| {
            c.connect(ids[0], ids[1], Rel::Causes);
        });
        b.add_note(eframe::egui::vec2(0.0, 300.0), "La marea sube");
        v.flush();
        assert_eq!(v.nb.backlinks(second), [(first, Rel::Causes)]);
    }
    let mut v = NoteView::open(&path).unwrap();
    assert_eq!(v.nb.outgoing(first).len(), 1);
    assert_eq!(v.nb.search("marea")[0].page, board);
    v.open_page(board);
    assert_eq!(v.board.as_ref().unwrap().canvas.items.len(), 3);
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn an_idea_is_a_page_of_the_section_shown_newest_first() {
    let d = dir("inbox");
    let path = d.join("c.apexnote");
    let mut v = NoteView::create(&path).unwrap();
    let open = v.page();
    let sections = v.nb.live_sections().len();
    let shown = v.section.unwrap();
    v.capture("Primera idea\ncon detalle #faro");
    let steps = v.undo.len();
    v.capture("Segunda idea");
    assert_eq!(v.undo.len(), steps + 1);
    assert_eq!(v.page(), open);
    // No section of its own: the ideas are on top of the one shown.
    assert_eq!(v.nb.live_sections().len(), sections);
    let titles: Vec<String> = v.nb.tree(shown).iter().map(|(_, p)| p.title.clone()).collect();
    assert_eq!(titles[..2], ["Segunda idea", "Primera idea"]);
    assert_eq!(v.nb.tagged("faro").len(), 1);
    // Today's page too, and only once.
    v.today();
    let day = v.page().unwrap();
    v.today();
    assert_eq!(v.page(), Some(day));
    assert_eq!(v.nb.page(day).unwrap().section, shown);
    assert_eq!(v.nb.live_sections().len(), sections);
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_source_page_is_cited_from_a_document() {
    let d = dir("source");
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    v.new_source();
    let id = v.page().unwrap();
    let s = apex_doc::Source { authors: "Varela, Ana".into(), title: "Faros del Cantábrico".into(), year: "2019".into(), ..Default::default() };
    let put = s.clone();
    v.change(|nb| nb.page_mut(id).unwrap().source = Some(put));
    let found = sources::all().into_iter().find(|x| x.title == s.title).expect("published");
    let mut doc = crate::doc_view::DocView::new(apex_doc::Document::from_plain_text("Texto\n"));
    doc.cite_source(found.clone());
    doc.cite_source(found);
    assert_eq!(doc.doc.biblio.sources.len(), 1);
    assert_eq!(doc.doc.biblio.sources[0].tag, "Varela19");
    // Back to a plain page: no longer offered.
    v.set_kind(id, types::Kind::Plain);
    assert!(!sources::all().iter().any(|x| x.title == s.title));
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_fragment_goes_under_its_source_and_links_back() {
    use fragments::{Spot, parse_origin};
    let d = dir("fragment");
    let pdf = d.join("Faros.pdf");
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    let a = v.add_fragment("La luz gira\ncada diez segundos", &pdf, Spot::Page(3), "Faros");
    let b = v.add_fragment("Los fareros vivían aislados", &pdf, Spot::Page(7), "Faros");
    let src = v.nb.page(a).unwrap().parent.expect("under its source");
    assert_eq!(v.nb.page(b).unwrap().parent, Some(src));
    assert_eq!(v.nb.page(src).unwrap().source.as_ref().unwrap().url, pdf.display().to_string());
    assert_eq!(v.nb.page(a).unwrap().title, "La luz gira");
    // One step undoes the fragment.
    v.undo();
    assert!(!v.nb.is_live(b));
    v.redo();
    // The page quotes the passage and links to its source and back.
    let doc = v.store.load_page(a).unwrap();
    assert_eq!(doc.paragraphs[1].text, "cada diez segundos");
    assert_eq!(doc.paragraphs[0].style.kind, apex_doc::ParaKind::Quote);
    assert!(v.nb.links[&a].iter().any(|l| l.0 == src));
    let back = doc.paragraphs[2].runs.iter().filter_map(|r| r.attrs.link.as_deref()).find_map(parse_origin);
    assert_eq!(back, Some((pdf.clone(), Spot::Page(3))));
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_question_keeps_its_status_and_undoes_it() {
    use apex_doc::notebook::{Claim, ClaimKind, Status};
    let d = dir("question");
    let path = d.join("c.apexnote");
    let mut v = NoteView::create(&path).unwrap();
    v.new_question();
    let id = v.page().unwrap();
    assert_eq!(v.nb.page(id).unwrap().claim, Some(Claim::new(ClaimKind::Question)));
    let done = Claim { kind: ClaimKind::Hypothesis, status: Status::Supported };
    v.set_claim(id, Some(done));
    v.undo();
    assert_eq!(v.nb.page(id).unwrap().claim, Some(Claim::new(ClaimKind::Question)));
    v.redo();
    drop(v);
    let v = NoteView::open(&path).unwrap();
    assert_eq!(v.nb.page(id).unwrap().claim, Some(done));
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn tasks_are_typed_listed_and_checked_from_the_panel() {
    let d = dir("tasks");
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    let first = v.page().unwrap();
    type_in(&mut v, "[] ");
    let ed = v.editor.as_mut().unwrap();
    ed.put_caret(apex_doc::Pos::new(0, 3));
    ed.check_task_prompt(" ");
    let end = apex_doc::Selection::caret(ed.doc.end());
    ed.doc.insert_text(end, "Llamar al farero", None);
    v.new_page(false);
    let second = v.page().unwrap();
    type_in(&mut v, "Revisar el mapa");
    v.editor.as_mut().unwrap().insert_task();
    v.flush();
    let all: Vec<(i64, String, bool)> = v.nb.all_tasks().iter().map(|(p, t)| (*p, t.text.clone(), t.done)).collect();
    assert_eq!(all, [(first, "Llamar al farero".to_string(), false), (second, "Revisar el mapa".to_string(), false)]);
    // Checked from the panel: the page not open is written.
    v.toggle_task(first, 0);
    assert!(v.nb.tasks[&first][0].done);
    assert!(v.store.load_page(first).unwrap().paragraphs[0].text.starts_with(apex_doc::CHECKED));
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_task_dragged_on_the_board_keeps_its_column_and_done_checks_it() {
    let d = dir("board");
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    let page = v.page().unwrap();
    type_in(&mut v, "Reservar la sala");
    v.editor.as_mut().unwrap().insert_task();
    v.new_page(false);
    v.flush();
    let cols: Vec<String> = ["Ideas", "Doing", "Review", "Done"].map(String::from).to_vec();
    v.set_task_columns(&cols);
    assert_eq!(v.task_columns(), cols);
    let col = |v: &NoteView| apex_doc::notebook::tasks::column_of(&v.nb.tasks[&page][0], &cols);
    assert_eq!(col(&v), 0);
    v.move_task(page, 0, 2, &cols);
    assert_eq!((col(&v), v.nb.tasks[&page][0].done), (2, false));
    // the last column checks it; unchecked, it goes back where it was
    v.move_task(page, 0, 3, &cols);
    assert_eq!((col(&v), v.nb.tasks[&page][0].done), (3, true));
    v.toggle_task(page, 0);
    assert_eq!(col(&v), 2);
    // a column renamed takes its tasks with it
    let edited = vec![(Some("Ideas".to_string()), "Ideas".to_string()), (Some("Doing".into()), "Doing".into()), (Some("Review".into()), "Checking".into()), (Some("Done".into()), "Done".into())];
    assert!(v.apply_columns(&edited));
    assert_eq!(v.nb.tasks[&page][0].meta.col.as_deref(), Some("Checking"));
    assert!(!v.apply_columns(&[(None, "Solo".into())]));
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_file_dropped_on_a_page_is_kept_inside_and_opens_from_its_link() {
    let d = dir("attach");
    let file = d.join("plano.txt");
    apex_fs::write(&file, "faro").unwrap();
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    type_in(&mut v, "Planos:");
    v.editor.as_mut().unwrap().put_caret(apex_doc::Pos::new(0, 7));
    v.drop_file(&file);
    let ed = v.editor.as_ref().unwrap();
    let link = ed.doc.paragraphs[1].runs.iter().find_map(|r| r.attrs.link.clone()).expect("a link to it");
    assert_eq!(ed.doc.paragraphs[1].text, "plano.txt");
    let id = attachments::parse_attach(&link).unwrap();
    apex_fs::remove_file(&file).unwrap();
    v.open_attachment(id);
    let copy = v.open_file.take().expect("copied out");
    assert_eq!(apex_fs::read_to_string(&copy).unwrap(), "faro");
    let _ = apex_fs::remove_dir_all(copy.parent().unwrap());
    // The folder of copies, when no other copy is there.
    let _ = apex_fs::remove_dir(copy.parent().and_then(|p| p.parent()).unwrap());
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_page_of_a_type_starts_from_its_template_and_keeps_its_fields() {
    use apex_doc::notebook::{Field, FieldKind};
    let d = dir("types");
    let path = d.join("c.apexnote");
    let mut v = NoteView::create(&path).unwrap();
    let template = v.page().unwrap();
    type_in(&mut v, "Aspecto:\nPasado:");
    v.flush();
    let mut ty = 0;
    v.change(|nb| {
        ty = nb.add_type("Personaje");
        let t = nb.page_type_mut(ty).unwrap();
        t.fields = vec![Field::new("Edad", FieldKind::Number), Field::new("Nacida", FieldKind::Date)];
        t.template = Some(template);
    });
    v.new_typed_page(ty);
    let ana = v.page().unwrap();
    assert_ne!(ana, template);
    assert_eq!(v.nb.page(ana).unwrap().ptype, Some(ty));
    assert!(plain_text(&v.editor.as_ref().unwrap().doc).contains("Pasado:"));
    assert!(v.nb.texts[&ana].contains("Aspecto"));
    v.change(|nb| nb.set_field(ana, "Edad", "34"));
    // Renaming the field keeps the value; undo brings the old name back.
    v.change(|nb| nb.rename_field(ty, "Edad", "Años"));
    assert_eq!(v.nb.field_value(ana, "Años"), "34");
    v.undo();
    assert_eq!(v.nb.field_value(ana, "Edad"), "34");
    drop(v);
    let v = NoteView::open(&path).unwrap();
    assert_eq!(v.nb.page_type(ty).unwrap().fields[0].name, "Edad");
    assert_eq!(v.nb.pages_of_type(ty), vec![ana]);
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn the_table_shows_a_type_and_adds_rows_of_it() {
    use apex_doc::notebook::{Field, FieldKind};
    let d = dir("table");
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    let mut ty = 0;
    v.change(|nb| {
        ty = nb.add_type("Lugar");
        nb.page_type_mut(ty).unwrap().fields = vec![Field::new("Habitantes", FieldKind::Number)];
    });
    v.open_table(Some(ty));
    assert!(v.table.is_some() && v.graph.is_none());
    let q = v.table.as_ref().unwrap().query.clone();
    v.add_row(&q);
    v.add_row(&q);
    let rows = v.nb.run_query(&q, &table::day);
    assert_eq!(rows.len(), 2);
    v.change(|nb| nb.set_field(rows[0], "Habitantes", "1200"));
    assert_eq!(v.nb.field_value(rows[0], "Habitantes"), "1200");
    // Another view closes it.
    v.open_tasks();
    assert!(v.table.is_none());
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn the_table_of_a_page_without_a_type_lists_its_section() {
    let d = dir("table_plain");
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    v.change(|nb| {
        nb.add_type("Lugar");
    });
    v.new_page(false);
    v.new_page(false);
    v.open_table(None);
    let q = v.table.as_ref().unwrap().query.clone();
    assert_eq!((q.ptype, q.section), (None, v.section));
    assert!(v.nb.run_query(&q, &table::day).len() >= 2);
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_saved_query_opens_the_table_with_its_filters() {
    use apex_doc::notebook::query::{Cond, Op, Query};
    use apex_doc::notebook::{Field, FieldKind};
    let d = dir("query");
    let path = d.join("c.apexnote");
    let mut v = NoteView::create(&path).unwrap();
    let mut ty = 0;
    v.change(|nb| {
        ty = nb.add_type("Personaje");
        nb.page_type_mut(ty).unwrap().fields = vec![Field::new("Edad", FieldKind::Number)];
    });
    v.open_table(Some(ty));
    let q = v.table.as_ref().unwrap().query.clone();
    v.add_row(&q);
    v.add_row(&q);
    let rows = v.nb.run_query(&q, &table::day);
    v.change(|nb| {
        nb.set_field(rows[0], "Edad", "70");
        nb.set_field(rows[1], "Edad", "12");
    });
    let old = Query { conds: vec![Cond { field: "Edad".into(), op: Op::More, value: "65".into() }], ..q };
    let mut id = 0;
    v.change(|nb| id = nb.save_query(&old, "Mayores"));
    drop(v);
    let mut v = NoteView::open(&path).unwrap();
    v.open_query(id);
    let t = v.table.as_ref().unwrap();
    assert!(t.filters);
    assert_eq!(v.nb.run_query(&t.query, &table::day), vec![rows[0]]);
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn an_obsidian_vault_comes_in_with_its_folders_links_and_files() {
    const PNG: [u8; 70] = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 100, 96, 248, 95, 15, 0, 2, 135, 1, 128, 235, 71, 186, 146, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    let d = dir("vault");
    let vault = d.join("Mi bóveda");
    for f in ["Lugares", "adjuntos", ".obsidian"] {
        apex_fs::create_dir_all(vault.join(f)).unwrap();
    }
    apex_fs::write(vault.join(".obsidian/app.json"), "{}").unwrap();
    apex_fs::write(vault.join("Inicio.md"), "---\ntags: [idea]\n---\nVer [[Faro|el faro]], [[Torre]] y [[plano.pdf]].\n![[mapa.png]]\n- [x] Hecho\n").unwrap();
    // A file outside the vault, which no note may bring in.
    apex_fs::write(d.join("fuera.txt"), b"privado").unwrap();
    apex_fs::write(vault.join("Lugares/Lugares.md"), "Todos los lugares. [[../../fuera.txt]] [x](../../fuera.txt)\n").unwrap();
    apex_fs::write(vault.join("Lugares/Faro.md"), "---\naliases: [Torre]\n---\nVuelve a [[Inicio]].\n").unwrap();
    apex_fs::write(vault.join("adjuntos/mapa.png"), PNG).unwrap();
    apex_fs::write(vault.join("adjuntos/plano.pdf"), b"%PDF-1.4").unwrap();
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    let s = v.import_vault(&vault).unwrap();
    assert_eq!(v.nb.section(s).unwrap().name, "Mi bóveda");
    let titles: Vec<(usize, String)> = v.nb.tree(s).iter().map(|(depth, p)| (*depth, p.title.clone())).collect();
    // The folder note is the folder's page; a folder of files only is none.
    assert_eq!(titles, [(0, "Inicio".to_string()), (0, "Lugares".into()), (1, "Faro".into())]);
    let id = |t: &str| v.nb.tree(s).iter().find(|(_, p)| p.title == t).unwrap().1.id;
    let (inicio, faro, lugares) = (id("Inicio"), id("Faro"), id("Lugares"));
    assert_eq!(v.nb.page(faro).unwrap().aliases, ["Torre"]);
    assert_eq!(v.nb.links[&inicio].iter().map(|l| l.0).collect::<Vec<_>>(), [faro]);
    assert_eq!(v.nb.links[&faro].iter().map(|l| l.0).collect::<Vec<_>>(), [inicio]);
    assert!(v.nb.texts[&inicio].contains("#idea") && v.nb.texts[&lugares].contains("Todos los lugares"));
    assert!(v.nb.tagged("idea").contains(&inicio));
    let doc = v.store.load_page(inicio).unwrap();
    assert!(doc.paragraphs.iter().any(|p| p.spans().any(|(_, _, a)| a.image.is_some())), "the picture is in the text");
    assert_eq!(v.nb.tasks[&inicio].iter().map(|t| (t.done, t.text.as_str())).collect::<Vec<_>>(), [(true, "Hecho")]);
    let files = v.store.attachments().unwrap();
    assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["plano.pdf"]);
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_section_exports_with_its_pages_under_their_titles() {
    use super::export::{Format, Scope};
    let d = dir("export");
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    let first = v.page().unwrap();
    v.rename(Naming::Page(first), "Trama");
    v.title = "Trama".into();
    type_in(&mut v, "El faro se apaga.");
    v.new_page(true);
    let sub = v.page().unwrap();
    v.rename(Naming::Page(sub), "Escena uno");
    type_in(&mut v, "Llega la barca.");
    let s = v.section().unwrap();
    let md_path = d.join("s.md");
    v.export(Scope::Section(s), Format::Markdown, &md_path);
    let md = apex_fs::read_to_string(&md_path).unwrap();
    let section = v.nb.section(s).unwrap().name.clone();
    assert_eq!(md, format!("# {section}\n\n## Trama\n\nEl faro se apaga.\n\n### Escena uno\n\nLlega la barca.\n"));
    v.export(Scope::Page(sub), Format::Docx, &d.join("p.docx"));
    let back = apex_io::docx::read(&apex_fs::read(d.join("p.docx")).unwrap()).unwrap();
    assert_eq!(back.paragraphs.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["Escena uno", "Llega la barca."]);
    v.export(Scope::Page(first), Format::Pdf, &d.join("p.pdf"));
    assert!(apex_fs::read(d.join("p.pdf")).unwrap().starts_with(b"%PDF"));
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_page_goes_back_to_an_earlier_text_and_keeps_the_one_it_had() {
    let d = dir("history");
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    let p = v.page().unwrap();
    type_in(&mut v, "Primera idea. ");
    v.flush();
    type_in(&mut v, "Luego otra. ");
    v.flush();
    let versions = v.store.versions(p).unwrap();
    assert_eq!(versions.len(), 1);
    v.restore_version(p, versions[0].id);
    assert_eq!(v.editor.as_ref().unwrap().doc.paragraphs[0].text, "Primera idea. ");
    assert!(v.nb.texts[&p].starts_with("Primera idea."));
    let after = v.store.versions(p).unwrap();
    assert_eq!(after.len(), 2);
    assert!(plain_text(&v.store.load_version(after[0].id).unwrap()).starts_with("Luego otra. Primera idea."), "the text it had is kept");
    v.open_history(p);
    assert!(v.history.is_some());
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_canvas_goes_beside_a_text_page_and_takes_turns_with_it() {
    let d = dir("split_canvas");
    let path = d.join("c.apexnote");
    let (a, c);
    {
        let mut v = NoteView::create(&path).unwrap();
        a = v.page().unwrap();
        v.new_canvas();
        c = v.page().unwrap();
        v.open_page(a);
        v.open_beside(c);
        assert!(v.editor.is_some() && v.board.is_none());
        // Going to it swaps them: the canvas is open, the text beside.
        v.open_page(c);
        assert_eq!((v.page(), v.side.as_ref().map(|s| s.page)), (Some(c), Some(a)));
        assert!(v.board.is_some() && v.editor.is_none());
        v.board.as_mut().unwrap().add_note(eframe::egui::Vec2::ZERO, "Faro");
        v.swap_sides();
        assert_eq!(v.page(), Some(a));
        assert!(v.side.as_mut().unwrap().editor_mut().is_none());
        v.side.as_mut().and_then(|s| if let split::Pane::Canvas(b) = &mut s.pane { Some(b) } else { None }).unwrap().add_note(eframe::egui::Vec2::new(300.0, 0.0), "Puerto");
    }
    let mut v = NoteView::open(&path).unwrap();
    v.open_page(c);
    let text = v.board.as_ref().unwrap().canvas.text();
    assert!(text.contains("Faro") && text.contains("Puerto"), "{text}");
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_page_beside_is_written_and_takes_turns_with_the_open_one() {
    let d = dir("split");
    let path = d.join("c.apexnote");
    let (a, b);
    {
        let mut v = NoteView::create(&path).unwrap();
        a = v.page().unwrap();
        v.new_page(false);
        b = v.page().unwrap();
        v.open_page(a);
        v.open_beside(b);
        v.open_beside(a);
        assert_eq!(v.side.as_ref().map(|s| s.page), Some(b), "the open page isn't put beside itself");
        v.side.as_mut().unwrap().editor_mut().unwrap().doc.insert_text(apex_doc::Selection::default(), "Al lado", None);
        // Going to the page beside swaps them.
        v.open_page(b);
        assert_eq!((v.page(), v.side.as_ref().map(|s| s.page)), (Some(b), Some(a)));
        assert!(v.side_left);
        assert!(v.nb.texts[&b].contains("Al lado"));
        type_in(&mut v, "Y aquí ");
        v.side.as_mut().unwrap().editor_mut().unwrap().doc.insert_text(apex_doc::Selection::default(), "Primera ", None);
        // Dropping it writes both.
    }
    let v = NoteView::open(&path).unwrap();
    assert!(v.nb.texts[&a].starts_with("Primera "));
    assert!(v.nb.texts[&b].starts_with("Y aquí Al lado"));
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_page_shown_in_another_follows_it() {
    let d = dir("embed");
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    let host = v.page().unwrap();
    v.new_page(false);
    let faro = v.page().unwrap();
    v.rename(Naming::Page(faro), "Faro");
    type_in(&mut v, "Luz roja.\nPuerto\nBarcas.");
    v.editor.as_mut().unwrap().doc.paragraphs[1].style.kind = apex_doc::ParaKind::Heading(2);
    v.open_page(host);
    type_in(&mut v, "Ver ![[");
    let ed = v.editor.as_mut().unwrap();
    ed.put_caret(apex_doc::Pos::new(0, 7));
    ed.check_link_prompt("[");
    v.editor_requests_for_test();
    let Some(super::links::Finder { want: super::links::Want::Embed(at), .. }) = v.finder.take() else { panic!("![[ asks for a page to show") };
    v.embed_page(at, faro, Some("puerto"));
    let texts = |v: &NoteView| v.editor.as_ref().unwrap().doc.paragraphs.iter().map(|p| p.text.clone()).collect::<Vec<_>>();
    assert_eq!(texts(&v), ["Ver ", "Puerto", "Barcas.", ""]);
    v.flush();
    assert_eq!(v.nb.links[&host], [(faro, apex_doc::notebook::Rel::Link)], "showing a page links to it");
    // Faro changes; the page showing it follows when it opens.
    v.open_page(faro);
    type_in(&mut v, "Hoy ");
    let ed = v.editor.as_mut().unwrap();
    let end = ed.doc.paragraphs[2].text.len();
    ed.doc.insert_text(apex_doc::Selection::caret(apex_doc::Pos::new(2, end)), " Y redes.", None);
    v.open_page(host);
    assert_eq!(texts(&v), ["Ver ", "Puerto", "Barcas. Y redes.", ""]);
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_book_sees_the_pages_of_a_notebook_open_as_cards() {
    let d = dir("shelf");
    let path = d.join("Mundo.apexnote");
    let mine = |uid: u64| shelf::cards().1.iter().filter(|c| c.uid == uid).cloned().collect::<Vec<_>>();
    let uid;
    {
        let mut v = NoteView::create(&path).unwrap();
        uid = v.uid();
        let a = v.page().unwrap();
        v.change(|nb| {
            let p = nb.page_mut(a).unwrap();
            p.title = "Ana Soler".into();
            p.aliases = vec!["la farera".into()];
        });
        type_in(&mut v, "Vive en el faro.");
        v.flush();
        let cards = mine(uid);
        assert_eq!(cards.len(), 1);
        assert_eq!((cards[0].notebook.as_str(), cards[0].title.as_str(), cards[0].text.as_str()), ("Mundo", "Ana Soler", "Vive en el faro."));
        assert_eq!(cards[0].names().collect::<Vec<_>>(), ["ana soler", "la farera"]);
    }
    assert!(mine(uid).is_empty(), "a notebook closed takes its cards away");
}

#[test]
fn a_section_under_a_password_hides_its_pages_until_unlocked() {
    let d = dir("password");
    let path = d.join("c.apexnote");
    let (sec, a);
    {
        let mut v = NoteView::create(&path).unwrap();
        sec = v.section().unwrap();
        a = v.page().unwrap();
        type_in(&mut v, "Diario secreto");
        v.set_section_lock(sec, "faro");
        assert!(v.nb.is_live(a), "unlocked while the password was just given");
        v.lock_section_now(sec);
        assert!(!v.nb.is_live(a) && v.page().is_none());
        assert!(v.nb.tree(sec).is_empty());
        // Nothing goes in while locked.
        v.show_section(sec);
        v.new_page(false);
        assert!(v.nb.pages.iter().all(|p| p.section != sec || p.id == a));
        assert!(v.lock_dialog.is_some());
    }
    assert!(!apex_fs::read(&path).unwrap().windows(7).any(|w| w == b"secreto"));
    let mut v = NoteView::open(&path).unwrap();
    assert!(v.nb.sealed.contains(&sec) && !v.nb.is_live(a));
    assert!(!v.unlock_section(sec, "otra"));
    assert!(v.unlock_section(sec, "faro"));
    assert!(v.nb.texts[&a].contains("Diario secreto"));
    v.show_section(sec);
    assert_eq!(v.page(), Some(a));
    v.remove_section_lock(sec);
    drop(v);
    assert!(apex_fs::read(&path).unwrap().windows(7).any(|w| w == b"secreto"), "in the clear again");
}

#[test]
fn the_timeline_places_pages_by_their_dates() {
    use apex_doc::notebook::{Field, FieldKind};
    use timeline::{Timeline, When};
    let d = dir("timeline");
    let path = d.join("c.apexnote");
    let mut v = NoteView::create(&path).unwrap();
    let plain = v.page().unwrap();
    let mut ty = 0;
    v.change(|nb| {
        ty = nb.add_type("Personaje");
        nb.page_type_mut(ty).unwrap().fields = vec![Field::new("Nacida", FieldKind::Date), Field::new("Muere", FieldKind::Date), Field::new("Edad", FieldKind::Number)];
    });
    v.new_typed_page(ty);
    let ana = v.page().unwrap();
    v.new_typed_page(ty);
    let luis = v.page().unwrap();
    v.change(|nb| {
        nb.set_field(ana, "Nacida", "1890-03-12");
        nb.set_field(ana, "Muere", "1950-01-02");
        nb.set_field(luis, "Nacida", "1901-07-30");
        nb.set_field(luis, "Muere", "algún día");
    });
    let at = |t: &Timeline| v.stops(t).into_iter().map(|s| (s.date, s.page, s.field)).collect::<Vec<_>>();
    assert_eq!(
        at(&Timeline::default()),
        [("1890-03-12".into(), ana, "Nacida".into()), ("1901-07-30".into(), luis, "Nacida".into()), ("1950-01-02".into(), ana, "Muere".into())]
    );
    let t = Timeline { when: When::Field("Muere".into()), newest: true, ..Timeline::default() };
    assert_eq!(at(&t), [("1950-01-02".into(), ana, "Muere".into())], "only real dates");
    let t = Timeline { when: When::Created, ..Timeline::default() };
    assert_eq!(v.stops(&t).len(), 3, "every page has its date created");
    assert!(v.stops(&t).iter().any(|s| s.page == plain));
    v.open_timeline();
    assert!(v.timeline.is_some() && !v.page_editing());
    v.close_graph();
    assert!(v.timeline.is_none());
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_map_keeps_markers_that_lead_to_pages() {
    let d = dir("map");
    let path = d.join("c.apexnote");
    let (map, faro);
    {
        let mut v = NoteView::create(&path).unwrap();
        faro = v.page().unwrap();
        v.change(|nb| nb.page_mut(faro).unwrap().title = "El faro".into());
        v.new_canvas();
        map = v.page().unwrap();
        v.board.as_mut().unwrap().set_map(vec![1, 2, 3]);
        v.write_board();
        v.put_pin(faro, 120.0, 80.0);
        assert!(v.nb.backlinks(faro).iter().any(|l| l.0 == map), "the map is related to the page it marks");
    }
    let v = NoteView::open(&path).unwrap();
    assert!(v.nb.backlinks(faro).iter().any(|l| l.0 == map));
    let c = v.store.load_canvas(map).unwrap();
    assert!(c.map().is_some() && c.pinned() == [faro]);
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_page_is_one_kind_at_a_time() {
    use apex_doc::notebook::ClaimKind;
    let d = dir("kinds");
    let mut v = NoteView::create(&d.join("k.apexnote")).unwrap();
    v.new_source();
    let id = v.page().unwrap();
    v.set_kind(id, types::Kind::Claim(ClaimKind::Question));
    let p = v.nb.page(id).unwrap();
    assert!(p.source.is_none() && p.claim.is_some_and(|c| c.kind == ClaimKind::Question));
    v.set_kind(id, types::Kind::Source);
    let p = v.nb.page(id).unwrap();
    assert!(p.source.is_some() && p.claim.is_none() && p.ptype.is_none());
    v.undo();
    assert!(v.nb.page(id).unwrap().claim.is_some());
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

/// Undoing a new page while it is open drops it without trying to write
/// it into a file that no longer has it.
#[test]
fn undoing_a_new_page_typed_in_writes_nothing_amiss() {
    let d = dir("undo_new");
    let mut v = NoteView::create(&d.join("c.apexnote")).unwrap();
    let first = v.page().unwrap();
    v.new_page(false);
    type_in(&mut v, "Nada");
    v.undo();
    assert_eq!(v.page(), Some(first));
    v.new_section();
    type_in(&mut v, "Tampoco");
    v.undo();
    v.flush();
    assert!(v.notices.is_empty(), "{:?}", v.notices.iter().map(|n| n.text.clone()).collect::<Vec<_>>());
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_page_dropped_on_the_graph_shows_whatever_the_filters_say() {
    let d = dir("graph_add");
    let mut v = NoteView::create(&d.join("g.apexnote")).unwrap();
    let first = v.page().unwrap();
    let section = v.nb.page(first).unwrap().section;
    v.new_section();
    let other = v.page().unwrap();
    let mut g = graph::Graph::default();
    g.section = Some(section);
    let (nodes, _, loose) = v.graph_parts(&g);
    assert!(!nodes.contains(&other) && !loose.contains(&other));
    g.added.insert(other);
    let (nodes, _, loose) = v.graph_parts(&g);
    // Shown though it is in another section and has no links.
    assert!(nodes.contains(&other) && !loose.contains(&other), "{nodes:?} {loose:?}");
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}

#[test]
fn a_link_drawn_in_the_graph_is_written_in_the_page_once() {
    let d = dir("graph_link");
    let mut v = NoteView::create(&d.join("l.apexnote")).unwrap();
    let a = v.page().unwrap();
    type_in(&mut v, "Texto");
    v.new_page(false);
    let b = v.page().unwrap();
    // From a page not open, then again: one link.
    v.connect(a, b, apex_doc::notebook::Rel::Supports);
    v.connect(a, b, apex_doc::notebook::Rel::Link);
    assert_eq!(v.nb.outgoing(a), vec![(b, apex_doc::notebook::Rel::Supports)]);
    let doc = v.store.load_page(a).unwrap();
    assert_eq!(doc.paragraphs.len(), 2, "the link on a line of its own");
    drop(v);
    let _ = apex_fs::remove_dir_all(&d);
}
