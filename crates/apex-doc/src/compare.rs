//! Compare documents: the differences between an original and a revised
//! document as tracked changes (paragraphs matched by an LCS of their
//! text, changed paragraphs diffed word by word).

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Same(usize, usize),
    Del(usize),
    Ins(usize),
}

/// Longest-common-subsequence diff of two sequences. Past `limit` cells
/// it gives up and reports everything as deleted and inserted.
pub fn diff<T: PartialEq>(a: &[T], b: &[T], limit: usize) -> Vec<Op> {
    // Common head and tail first: the usual case is a few changes.
    let head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut ops: Vec<Op> = (0..head).map(|i| Op::Same(i, i)).collect();
    let (n, m) = (am.len(), bm.len());
    if n * m > limit {
        ops.extend((0..n).map(|i| Op::Del(head + i)));
        ops.extend((0..m).map(|j| Op::Ins(head + j)));
    } else {
        // lcs[i][j]: LCS length of am[i..] and bm[j..].
        let w = m + 1;
        let mut lcs = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * w + j] = if am[i] == bm[j] { lcs[(i + 1) * w + j + 1] + 1 } else { lcs[(i + 1) * w + j].max(lcs[i * w + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && am[i] == bm[j] {
                ops.push(Op::Same(head + i, head + j));
                i += 1;
                j += 1;
            } else if j < m && (i == n || lcs[i * w + j + 1] > lcs[(i + 1) * w + j]) {
                // Deletions first on ties, so a replaced item reads "old, new".
                ops.push(Op::Ins(head + j));
                j += 1;
            } else {
                ops.push(Op::Del(head + i));
                i += 1;
            }
        }
    }
    ops.extend((0..tail).map(|k| Op::Same(a.len() - tail + k, b.len() - tail + k)));
    ops
}

/// Byte ranges of the words, spaces and single other characters of `t`.
fn tokens(t: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut it = t.char_indices().peekable();
    while let Some((s, c)) = it.next() {
        let class = |c: char| if c.is_alphanumeric() || c == '_' { 1 } else if c.is_whitespace() { 2 } else { 0 };
        let k = class(c);
        let mut e = s + c.len_utf8();
        while k != 0 && it.peek().is_some_and(|&(_, n)| class(n) == k) {
            let (i, n) = it.next().unwrap_or((e, ' '));
            e = i + n.len_utf8();
        }
        out.push((s, e));
    }
    out
}

/// Appends `p`'s text `s..e` to `out` with its formatting and `rev`.
fn copy_text(out: &mut Paragraph, p: &Paragraph, s: usize, e: usize, rev: &Option<Arc<Revision>>) {
    for (rs, re, a) in p.spans() {
        let (x, y) = (rs.max(s), re.min(e));
        if x < y {
            out.push(&p.text[x..y], Attrs { rev: rev.clone(), ..a.clone() });
        }
    }
}

fn blank_like(p: &Paragraph) -> Paragraph {
    let mut out = Paragraph::new("");
    out.style = p.style;
    out.cell = p.cell.clone();
    out
}

/// Word diff of two paragraphs into one with tracked changes; `None` when
/// they have too little in common to be the same paragraph edited.
fn diff_para(a: &Paragraph, b: &Paragraph, ins: &Arc<Revision>, del: &Arc<Revision>) -> Option<Paragraph> {
    let (ta, tb) = (tokens(&a.text), tokens(&b.text));
    let wa: Vec<&str> = ta.iter().map(|&(s, e)| &a.text[s..e]).collect();
    let wb: Vec<&str> = tb.iter().map(|&(s, e)| &b.text[s..e]).collect();
    let ops = diff(&wa, &wb, 4_000_000);
    let same: usize = ops.iter().filter_map(|o| if let Op::Same(i, _) = o { Some(wa[*i].trim().len()) } else { None }).sum();
    let total = a.text.trim().len().max(b.text.trim().len()).max(1);
    if same * 3 < total {
        return None;
    }
    let mut out = blank_like(b);
    let (ins, del) = (Some(ins.clone()), Some(del.clone()));
    // Deletions go before the insertions that replace them.
    let mut pending_ins: Vec<usize> = Vec::new();
    let flush = |out: &mut Paragraph, pending: &mut Vec<usize>| {
        for j in pending.drain(..) {
            copy_text(out, b, tb[j].0, tb[j].1, &ins);
        }
    };
    for op in ops {
        match op {
            Op::Same(_, j) => {
                flush(&mut out, &mut pending_ins);
                copy_text(&mut out, b, tb[j].0, tb[j].1, &None);
            }
            Op::Del(i) => copy_text(&mut out, a, ta[i].0, ta[i].1, &del),
            Op::Ins(j) => pending_ins.push(j),
        }
    }
    flush(&mut out, &mut pending_ins);
    if out.runs.is_empty() {
        out.runs.push(Run { len: 0, attrs: Attrs::default() });
    }
    Some(out)
}

fn whole(p: &Paragraph, rev: &Arc<Revision>, cell: bool) -> Paragraph {
    let mut out = blank_like(p);
    if !cell {
        out.cell = None;
    }
    copy_text(&mut out, p, 0, p.len(), &Some(rev.clone()));
    if out.runs.is_empty() {
        out.runs.push(Run { len: 0, attrs: Attrs::default() });
    }
    out
}

/// A document showing how `revised` differs from `original` as tracked
/// changes by `author` (both taken with their own changes accepted). It
/// keeps the revised document's layout; comments and bookmarks aren't
/// carried over.
pub fn compare(original: &Document, revised: &Document, author: &str) -> Document {
    let (a, b) = (original.resolved(true), revised.resolved(true));
    let date = now_iso();
    let ins = Revision::new(RevKind::Insert, author, &date);
    let del = Revision::new(RevKind::Delete, author, &date);
    let key = |p: &Paragraph| p.text.clone();
    let ka: Vec<String> = a.paragraphs.iter().map(key).collect();
    let kb: Vec<String> = b.paragraphs.iter().map(key).collect();
    let ops = diff(&ka, &kb, 50_000_000);
    let mut out: Vec<Paragraph> = Vec::new();
    // Paragraphs of the original that go, waiting for the next insertion
    // (a replaced paragraph) or a common one.
    let mut dels: Vec<usize> = Vec::new();
    let in_table = |out: &Vec<Paragraph>, next: Option<&Paragraph>| out.last().is_some_and(|p| p.cell.is_some()) && next.is_some_and(|p| p.cell.is_some());
    let flush_dels = |out: &mut Vec<Paragraph>, dels: &mut Vec<usize>, next: Option<&Paragraph>| {
        for i in dels.drain(..) {
            let p = whole(&a.paragraphs[i], &del, false);
            // Inside a table the text joins the previous cell instead.
            if in_table(out, next) {
                if let Some(last) = out.last_mut() {
                    last.push(" ", Attrs { rev: Some(del.clone()), ..Attrs::default() });
                    for (s, e, at) in p.spans() {
                        if e > s {
                            last.push(&p.text[s..e], at.clone());
                        }
                    }
                }
            } else {
                out.push(p);
            }
        }
    };
    for op in ops {
        match op {
            Op::Same(_, j) => {
                flush_dels(&mut out, &mut dels, Some(&b.paragraphs[j]));
                let mut p = b.paragraphs[j].clone();
                p.marks.clear();
                out.push(p);
            }
            Op::Del(i) => dels.push(i),
            Op::Ins(j) => {
                let bp = &b.paragraphs[j];
                // Pair with a deleted paragraph when they're alike.
                let paired = dels.iter().enumerate().filter(|(_, i)| a.paragraphs[**i].cell.is_some() == bp.cell.is_some()).find_map(|(k, &i)| diff_para(&a.paragraphs[i], bp, &ins, &del).map(|p| (k, p)));
                match paired {
                    Some((k, p)) => {
                        let rest: Vec<usize> = dels.drain(..k).collect();
                        dels.remove(0);
                        let mut before = rest;
                        flush_dels(&mut out, &mut before, Some(bp));
                        out.push(p);
                    }
                    None => {
                        flush_dels(&mut out, &mut dels, Some(bp));
                        out.push(whole(bp, &ins, true));
                    }
                }
            }
        }
    }
    flush_dels(&mut out, &mut dels, None);
    let mut d = Document::from_paragraphs(out);
    d.page = b.page;
    d.headers = b.headers.clone();
    d.odd_even = b.odd_even;
    d.default_font = b.default_font.clone();
    d.meta = b.meta.clone();
    d.user = Arc::from(author);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(texts: &[&str]) -> Document {
        Document::from_paragraphs(texts.iter().map(|t| Paragraph::new(*t)).collect())
    }

    fn marked(p: &Paragraph) -> String {
        p.spans()
            .filter(|(s, e, _)| e > s)
            .map(|(s, e, a)| match a.rev.as_ref().map(|r| r.kind) {
                Some(RevKind::Insert) => format!("[+{}]", &p.text[s..e]),
                Some(RevKind::Delete) => format!("[-{}]", &p.text[s..e]),
                None => p.text[s..e].to_string(),
            })
            .collect()
    }

    #[test]
    fn diffs() {
        let ops = diff(&[1, 2, 3, 4], &[1, 3, 5, 4], 100);
        assert_eq!(ops, [Op::Same(0, 0), Op::Del(1), Op::Same(2, 1), Op::Ins(2), Op::Same(3, 3)]);
        assert_eq!(tokens("Hola, mundo  bonito").len(), 6);
        let a = doc(&["Title", "The quick brown fox jumps.", "Gone paragraph here.", "Same end"]);
        let b = doc(&["Title", "The quick red fox jumps high.", "New one entirely different", "Same end"]);
        let d = compare(&a, &b, "Ana");
        let got: Vec<String> = d.paragraphs.iter().map(marked).collect();
        assert_eq!(got, ["Title", "The quick [-brown][+red] fox jumps[+ high].", "[-Gone paragraph here.]", "[+New one entirely different]", "Same end"]);
        assert_eq!(d.resolved(true).to_plain_text(), b.to_plain_text());
        assert_eq!(d.resolved(false).to_plain_text(), a.to_plain_text());
        // An edited paragraph after a deleted one still diffs by words.
        let a = doc(&["Title", "Launched in August.", "Costs remained stable."]);
        let b = doc(&["Title", "Costs remained stable despite prices."]);
        let got: Vec<String> = compare(&a, &b, "Ana").paragraphs.iter().map(marked).collect();
        assert_eq!(got, ["Title", "[-Launched in August.]", "Costs remained stable[+ despite prices]."]);
        // Identical documents: no changes.
        assert!(!compare(&a, &a, "x").has_revisions());
    }
}
