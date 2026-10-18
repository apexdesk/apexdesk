//! Round trips of comments and tracked changes through docx and odt.

use apex_doc::{Attrs, Document, Paragraph, Pos, RevKind, Revision, Selection};

fn para(parts: &[(&str, Option<(RevKind, &str)>)]) -> Paragraph {
    let mut p = Paragraph::new("");
    for (t, rev) in parts {
        let rev = rev.map(|(k, who)| Revision::new(k, who, "2024-05-01T10:00:00Z"));
        p.push(t, Attrs { rev, ..Attrs::default() });
    }
    p
}

fn sample() -> Document {
    let ins = Some((RevKind::Insert, "Ana"));
    let del = Some((RevKind::Delete, "Bob"));
    let mut d = Document::from_paragraphs(vec![
        para(&[("hello ", None), ("big ", ins), ("world", None)]),
        para(&[("second ", del), ("para", None)]),
        para(&[("gone para", del)]),
        para(&[("new para", ins)]),
        para(&[("after", None)]),
    ]);
    d.user = "Ana Pérez".into();
    d.track_changes = true;
    let id = d.add_comment(Selection { anchor: Pos::new(0, 6), cursor: Pos::new(0, 15) }, "Line one\nLine <two> & more");
    d.add_reply(id, "A reply", Selection::default());
    d.set_comment_resolved(id, true, Selection::default());
    d.add_comment(Selection::caret(Pos::new(4, 2)), "Point");
    d
}

/// Paragraph texts with the changes marked, and the comments.
fn summary(d: &Document) -> (Vec<String>, Vec<String>) {
    let paras = d
        .paragraphs
        .iter()
        .map(|p| {
            p.spans()
                .filter(|(s, e, _)| e > s)
                .map(|(s, e, a)| match a.rev.as_deref() {
                    Some(r) => format!("[{}{}:{}:{}]", if r.kind == RevKind::Insert { '+' } else { '-' }, &p.text[s..e], r.author, r.date.trim_end_matches('Z')),
                    None => p.text[s..e].to_string(),
                })
                .collect()
        })
        .collect();
    let comments = d
        .comment_spans()
        .iter()
        .filter_map(|s| {
            let c = d.comment(s.id)?;
            let replies: Vec<String> = c.replies.iter().map(|r| format!("{}: {}", r.author, r.text)).collect();
            Some(format!("{:?}-{:?} {} {:?} {} {:?}", s.start, s.end, c.author, c.text, c.resolved, replies))
        })
        .collect();
    (paras, comments)
}

#[test]
fn docx_roundtrip() {
    let d = sample();
    let back = crate::docx::read(&crate::docx::write(&d)).expect("docx");
    assert_eq!(summary(&back), summary(&d));
    assert!(back.track_changes);
    assert!(back.bookmarks().is_empty());
    // Accepting all gives the same text either way.
    assert_eq!(back.resolved(true).to_plain_text(), d.resolved(true).to_plain_text());
}

#[test]
fn odt_roundtrip() {
    let d = sample();
    let back = crate::odt::read(&crate::odt::write(&d)).expect("odt");
    assert_eq!(summary(&back), summary(&d));
    assert!(back.track_changes);
    assert!(back.bookmarks().is_empty());
    assert_eq!(back.resolved(false).to_plain_text(), d.resolved(false).to_plain_text());
}

#[test]
fn plain_documents_stay_plain() {
    let d = Document::from_paragraphs(vec![Paragraph::new("x")]);
    for back in [crate::docx::read(&crate::docx::write(&d)).expect("docx"), crate::odt::read(&crate::odt::write(&d)).expect("odt")] {
        assert!(!back.track_changes && !back.has_revisions() && back.comments.is_empty());
    }
}
