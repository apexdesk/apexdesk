//! Code edited back into the document: the text of the code view (the
//! document written as Markdown or plain text) read again keeps what the
//! text can't say (fonts, colors, sizes, paragraph styles, tables'
//! looks, comments) where it wasn't edited. What changed is found by
//! comparing the text as written (`base`) with the edited text (`new`),
//! both read back; each unchanged paragraph or table is the document's
//! own, and a changed one keeps the formatting of the characters that
//! are still there, with what the text says (bold, italic, a link...)
//! put over it.

use std::ops::Range;

use super::*;

/// Past this many cells the comparison gives up (everything changed).
const LIMIT: usize = 4_000_000;

/// The document's paragraphs (`old`, written as the code `base`) with the
/// code `new` read in instead: see the module.
pub fn merge_code(old: &[Paragraph], base: &[Paragraph], new: Vec<Paragraph>) -> Vec<Paragraph> {
    let (ob, bb, nb) = (blocks(old), blocks(base), blocks(&new));
    // Which of the document's blocks each block of `base` was written
    // from: by their text, and else by their place among the changes.
    fn key<'a>(ps: &'a [Paragraph], r: &Range<usize>) -> (bool, Vec<&'a str>) {
        (ps[r.start].cell.is_some(), ps[r.clone()].iter().map(|p| p.text.as_str()).collect())
    }
    let okeys: Vec<_> = ob.iter().map(|r| key(old, r)).collect();
    let bkeys: Vec<_> = bb.iter().map(|r| key(base, r)).collect();
    let mut from = vec![None; bb.len()];
    for_hunks(&diff(&bkeys, &okeys, LIMIT), |same, dels, ins| match same {
        Some((b, o)) => from[b] = Some(o),
        None => {
            for (&b, &o) in dels.iter().zip(ins) {
                if bkeys[b].0 == okeys[o].0 {
                    from[b] = Some(o);
                }
            }
        }
    });
    let mut used = vec![false; ob.len()];
    for o in from.iter().flatten() {
        used[*o] = true;
    }
    let mut out = Vec::with_capacity(new.len());
    // Blocks of the document the code didn't have (none written for
    // them) stay where they were.
    let mut next = 0;
    let mut upto = |out: &mut Vec<Paragraph>, o: usize| {
        while next < o {
            if !used[next] {
                out.extend_from_slice(&old[ob[next].clone()]);
            }
            next += 1;
        }
        next = next.max(o + 1);
    };
    let bslices: Vec<&[Paragraph]> = bb.iter().map(|r| &base[r.clone()]).collect();
    let nslices: Vec<&[Paragraph]> = nb.iter().map(|r| &new[r.clone()]).collect();
    for_hunks(&diff(&bslices, &nslices, LIMIT), |same, dels, ins| {
        if let Some((b, _)) = same {
            if let Some(o) = from[b] {
                upto(&mut out, o);
                out.extend_from_slice(&old[ob[o].clone()]);
            } else {
                out.extend_from_slice(nslices[same.unwrap().1]);
            }
            return;
        }
        for (k, &b) in dels.iter().enumerate() {
            let o = from[b];
            if let Some(o) = o {
                upto(&mut out, o);
            }
            if let (Some(&n), Some(o)) = (ins.get(k), o) {
                out.extend(merge_block(&old[ob[o].clone()], bslices[b], nslices[n]));
            } else if let Some(&n) = ins.get(k) {
                out.extend_from_slice(nslices[n]);
            }
        }
        for &n in ins.iter().skip(dels.len()) {
            out.extend_from_slice(nslices[n]);
        }
    });
    upto(&mut out, ob.len());
    out
}

/// The paragraphs in blocks: a table (its paragraphs) or one paragraph.
fn blocks(ps: &[Paragraph]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for (i, p) in ps.iter().enumerate() {
        let table = p.cell.as_ref().map(|c| c.table);
        match out.last_mut() {
            Some(r) if table.is_some() && ps[r.start].cell.as_ref().map(|c| c.table) == table => r.end = i + 1,
            _ => out.push(i..i + 1),
        }
    }
    out
}

/// Calls `f` with each unchanged pair (`Some((a, b))`) and with each run
/// of changes (`None`, the deleted and the inserted, in order).
fn for_hunks(ops: &[DiffOp], mut f: impl FnMut(Option<(usize, usize)>, &[usize], &[usize])) {
    let (mut dels, mut ins) = (Vec::new(), Vec::new());
    for op in ops {
        match *op {
            DiffOp::Same(a, b) => {
                if !dels.is_empty() || !ins.is_empty() {
                    f(None, &dels, &ins);
                    dels.clear();
                    ins.clear();
                }
                f(Some((a, b)), &[], &[]);
            }
            DiffOp::Del(a) => dels.push(a),
            DiffOp::Ins(b) => ins.push(b),
        }
    }
    if !dels.is_empty() || !ins.is_empty() {
        f(None, &dels, &ins);
    }
}

/// A changed block: the document's (`old`), written as `base`, edited to
/// `new`.
fn merge_block(old: &[Paragraph], base: &[Paragraph], new: &[Paragraph]) -> Vec<Paragraph> {
    let cells = |ps: &[Paragraph]| ps.iter().map(|p| p.cell.as_ref().map(|c| (c.row, c.col))).collect::<Vec<_>>();
    if old.len() == base.len() && base.len() == new.len() && cells(old) == cells(new) {
        return old.iter().zip(base).zip(new).map(|((o, b), n)| merge_para(o, b, n)).collect();
    }
    // A table with other rows or columns: as the code has it, in the
    // place (and as the table) it was.
    let table = old.first().and_then(|p| p.cell.as_ref()).map(|c| c.table);
    new.iter()
        .cloned()
        .map(|mut p| {
            if let (Some(c), Some(t)) = (p.cell.as_mut(), table) {
                c.table = t;
            }
            p
        })
        .collect()
}

/// What the text can say of a character's look.
fn said(a: &Attrs) -> (bool, bool, bool, bool, &Option<Arc<str>>, bool) {
    (a.bold, a.italic, a.strike, a.code, &a.link, a.image.is_some())
}

/// `a` with what the text says of `by`.
fn told(a: &Attrs, by: &Attrs) -> Attrs {
    if by.image.is_some() || a.image.is_some() || a.field.is_some() || a.note.is_some() || a.math.is_some() || a.shape.is_some() || a.control.is_some() {
        // An object: as the code has it.
        return by.clone();
    }
    let mut out = a.clone();
    (out.bold, out.italic, out.strike, out.code) = (by.bold, by.italic, by.strike, by.code);
    if out.link != by.link {
        (out.link, out.link_tip) = (by.link.clone(), None);
    }
    out
}

/// The attributes of the character at byte `at` of `p`.
fn attrs_of(p: &Paragraph, at: usize) -> &Attrs {
    let mut pos = 0;
    for r in &p.runs {
        if at < pos + r.len {
            return &r.attrs;
        }
        pos += r.len;
    }
    &p.runs.last().expect("a paragraph has a run").attrs
}

/// A changed paragraph: the document's (`old`), written as `base`,
/// edited to `new`.
fn merge_para(old: &Paragraph, base: &Paragraph, new: &Paragraph) -> Paragraph {
    let style = if base.style == new.style { old.style.clone() } else { new.style.clone() };
    if old.text != base.text {
        // Written otherwise than it is (a field, a picture): as the code
        // has it, in the paragraph's place.
        return Paragraph { style, cell: old.cell.clone(), section: old.section.clone(), ..new.clone() };
    }
    let (o, n) = (&old.text, &new.text);
    let head = o.chars().zip(n.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a.len_utf8()).sum::<usize>();
    let tail = o[head..].chars().rev().zip(n[head..].chars().rev()).take_while(|(a, b)| a == b).map(|(a, _)| a.len_utf8()).sum::<usize>();
    let (o_mid, n_mid) = (head..o.len() - tail, head..n.len() - tail);
    let shift = |at: usize| if at < o_mid.end { at } else { at + n.len() - o.len() };
    // Typed text looks like the character before it (else after).
    let near = if head > 0 { attrs_at_left(old, head) } else if o.is_empty() { &old.runs[0].attrs } else { attrs_of(old, o_mid.end.min(o.len() - 1)) };
    let mut runs: Vec<Run> = Vec::new();
    let mut add = |len: usize, a: Attrs| match runs.last_mut() {
        Some(r) if r.attrs == a => r.len += len,
        _ => runs.push(Run { len, attrs: a }),
    };
    for (i, c) in n.char_indices() {
        let a = if n_mid.contains(&i) {
            told(near, attrs_of(new, i))
        } else {
            // Still there: its own look, and what the code changed of it.
            let at = if i < head { i } else { i + o.len() - n.len() };
            let (was, now) = (attrs_of(base, at), attrs_of(new, i));
            if said(was) == said(now) { attrs_of(old, at).clone() } else { told(attrs_of(old, at), now) }
        };
        add(c.len_utf8(), a);
    }
    if runs.is_empty() {
        runs.push(Run { len: 0, attrs: old.runs.first().map(|r| r.attrs.clone()).unwrap_or_default() });
    }
    // Bookmarks and comment anchors move with their text; those in the
    // replaced text go to where it starts.
    let marks = old.marks.iter().map(|m| Mark { offset: if m.offset > head && m.offset < o_mid.end { head } else { shift(m.offset) }, ..m.clone() }).collect();
    Paragraph { text: n.clone(), runs, style, marks, cell: old.cell.clone(), section: old.section.clone() }
}

/// The attributes of the character before byte `at`.
fn attrs_at_left(p: &Paragraph, at: usize) -> &Attrs {
    let before = p.text[..at].chars().next_back().map_or(0, |c| at - c.len_utf8());
    attrs_of(p, before)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colored(text: &str, color: u32) -> Paragraph {
        let mut p = Paragraph::new("");
        p.runs.clear();
        p.text = text.into();
        p.runs.push(Run { len: text.len(), attrs: Attrs { color: Some(color), font: Some(Arc::from("Georgia")), ..Attrs::default() } });
        p
    }

    fn plain(ps: &[Paragraph]) -> Vec<Paragraph> {
        ps.iter().map(|p| Paragraph::new(p.text.clone())).collect()
    }

    #[test]
    fn untouched_paragraphs_keep_their_look() {
        let style = ParaStyle { align: Align::Center, ..ParaStyle::default() };
        let old = vec![colored("Uno rojo", 0xff0000).with_style(style.clone()), colored("Dos azul", 0x0000ff), colored("Tres verde", 0x00ff00)];
        let base = plain(&old);
        let mut new = base.clone();
        new[1] = Paragraph::new("Dos azul y más");
        new.insert(2, Paragraph::new("Nuevo"));
        let out = merge_code(&old, &base, new);
        assert_eq!(out.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["Uno rojo", "Dos azul y más", "Nuevo", "Tres verde"]);
        assert_eq!(out[0], old[0]);
        assert_eq!(out[3], old[2]);
        // The edited one: all of it blue, typed text too.
        assert!(out[1].runs.iter().all(|r| r.attrs.color == Some(0x0000ff)));
        assert_eq!(out[1].runs.iter().map(|r| r.len).sum::<usize>(), out[1].text.len());
    }

    #[test]
    fn what_the_code_says_goes_over_the_look() {
        let old = vec![colored("Hola mundo", 0xff0000)];
        let base = plain(&old);
        let mut new = Paragraph::new("");
        new.push("Hola ", Attrs::default());
        new.push("mundo", Attrs { bold: true, ..Attrs::default() });
        let out = merge_code(&old, &base, vec![new]);
        let p = &out[0];
        assert_eq!(p.text, "Hola mundo");
        assert!(p.runs.iter().all(|r| r.attrs.color == Some(0xff0000)));
        assert!(!attrs_of(p, 0).bold && attrs_of(p, 6).bold);
    }

    #[test]
    fn deleted_and_unwritten_paragraphs() {
        let old = vec![colored("A", 1), colored("B", 2), colored("C", 3)];
        // "B" wasn't written to the code (say, a page break): it stays.
        let base = vec![Paragraph::new("A"), Paragraph::new("C")];
        let out = merge_code(&old, &base, vec![Paragraph::new("C")]);
        assert_eq!(out, vec![old[1].clone(), old[2].clone()]);
    }

    #[test]
    fn marks_follow_their_text() {
        let mut old = colored("abc def", 9);
        old.marks.push(Mark { name: Arc::from("m"), offset: 4, end: false });
        let base = plain(std::slice::from_ref(&old));
        let out = merge_code(&[old], &base, vec![Paragraph::new("XYabc def")]);
        assert_eq!(out[0].marks[0].offset, 6);
    }
}
