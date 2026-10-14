//! Editing VBA code as the VBA editor does: indenting after Enter and
//! closing blocks, indenting and commenting several lines, writing known
//! words in their case, and what completion and parameter info offer at
//! the cursor. Positions are in characters, as egui's cursors count them.

use std::collections::HashMap;

use apex_vba::catalog::{self, Kind};

pub(super) const INDENT: &str = "    ";

fn byte_at(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map_or(s.len(), |(i, _)| i)
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// The line of a character position: (its start in bytes, its text).
fn line_at(code: &str, at: usize) -> (usize, &str) {
    let b = byte_at(code, at);
    let start = code[..b].rfind('\n').map_or(0, |i| i + 1);
    let end = code[b..].find('\n').map_or(code.len(), |i| b + i);
    (start, &code[start..end])
}

/// The code of a line without its comment or strings' contents (they are
/// blanked, keeping positions), for looking at its words.
fn bare(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_str = false;
    for c in line.chars() {
        if c == '"' {
            in_str = !in_str;
            out.push('"');
        } else if in_str {
            out.push(' ');
        } else if c == '\'' {
            break;
        } else {
            out.push(c);
        }
    }
    let t = out.trim_start();
    if t.len() >= 3 && t[..3].eq_ignore_ascii_case("rem") && t[3..].chars().next().is_none_or(char::is_whitespace) {
        return String::new();
    }
    out
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).map(str::to_ascii_lowercase).collect()
}

/// The statement a line opens and the one that closes it (`If x Then` →
/// `End If`); None for lines that don't open a block.
fn opens(line: &str) -> Option<&'static str> {
    let b = bare(line);
    let w = words(&b);
    let first = w.first()?.as_str();
    let skip_scope = matches!(first, "public" | "private" | "friend" | "static");
    let head = if skip_scope { w.get(1).map_or("", String::as_str) } else { first };
    match head {
        "sub" => Some("End Sub"),
        "function" => Some("End Function"),
        "property" => Some("End Property"),
        "type" if skip_scope || w.len() == 2 => Some("End Type"),
        "enum" => Some("End Enum"),
        "if" | "elseif" if !skip_scope => {
            // Only the block form: nothing after Then.
            let t = b.trim_end().to_ascii_lowercase();
            (t.ends_with(" then") || t.ends_with("\tthen")).then_some("End If")
        }
        "else" if !skip_scope => Some("End If"),
        "for" if !skip_scope => Some("Next"),
        "do" if !skip_scope => Some("Loop"),
        "while" if !skip_scope => Some("Wend"),
        "with" if !skip_scope => Some("End With"),
        "select" if !skip_scope => Some("End Select"),
        "case" if !skip_scope => Some("End Select"),
        _ => None,
    }
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// Enter at `at`: a new line with the indent of this one, one more after a
/// line that opens a block, and the block's end added when it isn't
/// there yet (`If x Then` ⏎ → the cursor inside, `End If` below).
pub(super) fn enter(code: &str, at: usize) -> (String, usize) {
    let b = byte_at(code, at);
    let (start, line) = line_at(code, at);
    let before = &code[start..b];
    let indent = indent_of(line).to_string();
    let after_cursor = code[b..start + line.len()].trim().is_empty();
    let block = if after_cursor { opens(before) } else { None };
    let inner = if block.is_some() { format!("{indent}{INDENT}") } else { indent.clone() };
    let mut out = String::with_capacity(code.len() + 32);
    out.push_str(&code[..b]);
    out.push('\n');
    out.push_str(&inner);
    let cursor = chars(&out);
    let rest = &code[b..];
    // `Else` and `Case` lines continue a block already closed.
    let continues = words(&bare(before)).first().is_some_and(|w| matches!(w.as_str(), "else" | "elseif" | "case"));
    if let Some(end) = block.filter(|_| !continues)
        && !closed(&code[start + line.len()..], &indent, end)
    {
        out.push_str(rest.trim_end_matches([' ', '\t']).strip_prefix('\n').map_or("", |_| ""));
        out.push('\n');
        out.push_str(&indent);
        out.push_str(end);
        out.push_str(rest);
    } else {
        out.push_str(rest);
    }
    (out, cursor)
}

/// Whether the block opened just before `rest` (at `indent`) already has
/// its `end`: the next line not more indented is it (or continues it).
fn closed(rest: &str, indent: &str, end: &str) -> bool {
    for line in rest.split('\n').skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let ind = indent_of(line);
        if ind.len() > indent.len() && ind.starts_with(indent) {
            continue;
        }
        let t = words(&bare(line));
        let want = words(end);
        return t.starts_with(&want) || (end == "End If" && t.first().is_some_and(|w| w == "else" || w == "elseif")) || (end == "End Select" && t.first().is_some_and(|w| w == "case"));
    }
    false
}

/// The lines a selection touches: (first, last) line numbers.
fn lines_of(code: &str, sel: (usize, usize)) -> (usize, usize) {
    let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let row = |at: usize| code[..byte_at(code, at)].matches('\n').count();
    let mut last = row(b);
    // A selection ending at the start of a line doesn't take it.
    if b > a && code[..byte_at(code, b)].ends_with('\n') {
        last -= 1;
    }
    (row(a), last.max(row(a)))
}

/// Applies `f` to the lines a selection touches; the selection then spans
/// them whole.
fn map_lines(code: &str, sel: (usize, usize), f: impl Fn(&str) -> String) -> (String, (usize, usize)) {
    let (first, last) = lines_of(code, sel);
    let mut out = Vec::new();
    let (mut a, mut b) = (0, 0);
    let mut pos = 0;
    for (i, line) in code.split('\n').enumerate() {
        let new = if (first..=last).contains(&i) { f(line) } else { line.to_string() };
        if i == first {
            a = pos;
        }
        pos += chars(&new) + 1;
        if i == last {
            b = pos - 1;
        }
        out.push(new);
    }
    (out.join("\n"), (a, b))
}

/// Tab and Shift+Tab on a selection of lines.
pub(super) fn indent_lines(code: &str, sel: (usize, usize), outdent: bool) -> (String, (usize, usize)) {
    map_lines(code, sel, |l| {
        if outdent {
            let ind = indent_of(l);
            let drop = if ind.starts_with('\t') { 1 } else { ind.len().min(INDENT.len()) };
            l[drop..].to_string()
        } else if l.trim().is_empty() {
            l.to_string()
        } else {
            format!("{INDENT}{l}")
        }
    })
}

/// Comment Block / Uncomment Block: `'` before the code of each line.
pub(super) fn comment_lines(code: &str, sel: (usize, usize), comment: bool) -> (String, (usize, usize)) {
    map_lines(code, sel, |l| {
        let ind = indent_of(l);
        let body = &l[ind.len()..];
        if comment {
            if body.is_empty() { l.to_string() } else { format!("{ind}'{body}") }
        } else {
            body.strip_prefix('\'').map_or_else(|| l.to_string(), |b| format!("{ind}{b}"))
        }
    })
}

/// Names the project declares, as their declarations write them: module
/// and local variables, constants, parameters, procedures, types.
pub(super) fn declared(sources: &[&str]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for code in sources {
        for line in code.lines() {
            for name in declarations(line) {
                out.insert(name.to_ascii_lowercase(), name);
            }
        }
    }
    out
}

/// The names a line declares (`Dim a As Long, B` → a, B).
fn declarations(line: &str) -> Vec<String> {
    let b = bare(line);
    let toks: Vec<&str> = b.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
    let low: Vec<String> = toks.iter().map(|t| t.to_ascii_lowercase()).collect();
    let mut i = 0;
    while i < low.len() && matches!(low[i].as_str(), "public" | "private" | "friend" | "static" | "global" | "dim" | "const" | "redim" | "preserve" | "withevents") {
        i += 1;
        if low.get(i - 1).is_some_and(|w| w == "dim" || w == "const" || w == "redim") {
            break;
        }
    }
    let first = low.first().map_or("", String::as_str);
    let is_decl = i > 0 && matches!(first, "public" | "private" | "friend" | "static" | "global" | "dim" | "const" | "redim");
    let mut out = Vec::new();
    match low.get(i).map(String::as_str) {
        Some("sub" | "function" | "property") if i == 0 || is_decl => {
            let mut j = i + 1;
            if low.get(j).is_some_and(|w| matches!(w.as_str(), "get" | "let" | "set")) {
                j += 1;
            }
            if let Some(name) = toks.get(j) {
                out.push(name.to_string());
            }
            // Parameters: the first name of each part between the parentheses.
            if let (Some(open), Some(close)) = (b.find('('), b.rfind(')')) {
                for part in b[open + 1..close.max(open + 1)].split(',') {
                    let names: Vec<&str> = part.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
                    if let Some(n) = names.iter().find(|w| !matches!(w.to_ascii_lowercase().as_str(), "byval" | "byref" | "optional" | "paramarray")) {
                        out.push(n.to_string());
                    }
                }
            }
        }
        Some("type" | "enum") if is_decl || i == 0 => {
            if let Some(name) = toks.get(i + 1) {
                out.push(name.to_string());
            }
        }
        _ if is_decl => {
            // Each part's first name, after the scope words.
            let at = b.to_ascii_lowercase().find(&low[i - 1]).map_or(0, |p| p + low[i - 1].len());
            let mut depth = 0;
            let mut part = String::new();
            for c in b[at..].chars().chain(std::iter::once(',')) {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    ',' if depth == 0 => {
                        let w: Vec<&str> = part.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
                        if let Some(n) = w.iter().find(|w| !matches!(w.to_ascii_lowercase().as_str(), "withevents" | "preserve")) {
                            out.push(n.to_string());
                        }
                        part.clear();
                        continue;
                    }
                    _ => {}
                }
                if depth == 0 {
                    part.push(c);
                }
            }
        }
        _ => {}
    }
    out
}

/// The line with its known words in their case: the project's names as
/// declared, then VBA's and the object model's (`msgbox` → `MsgBox`).
/// Strings and comments stay as they are.
pub(super) fn normalize_line(line: &str, names: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(line.len());
    let mut word = String::new();
    let mut in_str = false;
    let mut comment = false;
    let mut prev = ' ';
    let flush = |word: &mut String, out: &mut String, member: bool| {
        if word.is_empty() {
            return;
        }
        let low = word.to_ascii_lowercase();
        let known = if member { catalog::case_of(&low).map(str::to_string).or_else(|| names.get(&low).cloned()) } else { names.get(&low).cloned().or_else(|| catalog::case_of(&low).map(str::to_string)) };
        out.push_str(known.as_deref().filter(|k| k.len() == word.len()).unwrap_or(word));
        word.clear();
    };
    let mut member = false;
    for c in line.chars() {
        if comment {
            out.push(c);
            continue;
        }
        if in_str {
            out.push(c);
            if c == '"' {
                in_str = false;
            }
            continue;
        }
        if c.is_alphanumeric() || c == '_' {
            if word.is_empty() {
                member = prev == '.';
                // Numbers and the type suffixes after them aren't words.
                if c.is_ascii_digit() {
                    out.push(c);
                    prev = c;
                    continue;
                }
            }
            if word.is_empty() && prev.is_ascii_digit() {
                out.push(c);
                prev = c;
                continue;
            }
            word.push(c);
            prev = c;
            continue;
        }
        flush(&mut word, &mut out, member);
        if word.is_empty() && out.trim_start().eq_ignore_ascii_case("rem") {
            comment = true;
        }
        match c {
            '"' => in_str = true,
            '\'' => comment = true,
            _ => {}
        }
        out.push(c);
        prev = c;
    }
    flush(&mut word, &mut out, member);
    out
}

/// What the cursor's code knows: the module's kind and the project's
/// names, for completion.
pub(super) struct Scope<'a> {
    /// The code of the module being edited.
    pub code: &'a str,
    /// A document's macros: Word's object model, not Excel's.
    pub word: bool,
    /// What `Me` is: `Worksheet`, `Workbook`, `Document` or none.
    pub me: Option<&'static str>,
    /// Document modules of the project: (name, class) — `Sheet1` is a
    /// Worksheet.
    pub objects: Vec<(String, &'static str)>,
    /// Procedures of the project: (name, parameters as written).
    pub procs: Vec<(String, String)>,
    /// Class modules of the project.
    pub classes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Item {
    pub name: String,
    pub kind: Kind,
    /// Parameters and type, for the list's tooltip.
    pub detail: String,
}

/// The list to show at `at`: where the word being completed starts and
/// the names that fit it. `forced`: Ctrl+Space (also names with no object).
pub(super) fn complete(scope: &Scope, at: usize, forced: bool) -> Option<(usize, Vec<Item>)> {
    let code = scope.code;
    let b = byte_at(code, at);
    let (line_start, _) = line_at(code, at);
    let before = &code[line_start..b];
    if in_string_or_comment(before) {
        return None;
    }
    let prefix_len = before.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').count();
    let prefix: String = before.chars().skip(chars(before) - prefix_len).collect();
    let head = &before[..before.len() - prefix.len()];
    let start = at - prefix_len;
    let fits = |n: &str| n.len() >= prefix.len() && n[..prefix.len()].eq_ignore_ascii_case(&prefix) && n.is_char_boundary(prefix.len());
    let mut items: Vec<Item> = Vec::new();
    if let Some(chain) = head.strip_suffix('.') {
        // Members of what is before the dot.
        let class = type_of(scope, at, chain)?;
        let c = catalog::class_of(scope.word, class)?;
        items.extend(c.members.iter().filter(|m| m.kind != Kind::Event && fits(m.name)).map(item));
    } else if ends_with_word(head, "as") || ends_with_word(head, "new") {
        let new = ends_with_word(head, "new");
        let mut types: Vec<(String, Kind)> = Vec::new();
        if !new {
            types.extend(["Boolean", "Byte", "Currency", "Date", "Double", "Integer", "Long", "LongLong", "LongPtr", "Object", "Single", "String", "Variant"].iter().map(|t| (t.to_string(), Kind::Keyword)));
            let host: &[&str] = if scope.word { &["Application", "Document", "Font", "Paragraph", "Range", "Table"] } else { &["Application", "Font", "Interior", "Range", "Workbook", "Worksheet"] };
            types.extend(host.iter().map(|t| (t.to_string(), Kind::Method)));
        }
        types.extend(["Collection", "Scripting.Dictionary"].iter().map(|t| (t.to_string(), Kind::Method)));
        types.extend(scope.classes.iter().map(|c| (c.clone(), Kind::Method)));
        items.extend(types.into_iter().filter(|t| fits(&t.0)).map(|(name, kind)| Item { name, kind, detail: String::new() }));
    } else if forced {
        let mut seen = std::collections::HashSet::new();
        // The procedure's and module's names, the project's, then VBA's.
        for spelled in declared(&[code]).into_values() {
            if fits(&spelled) && seen.insert(spelled.to_ascii_lowercase()) {
                items.push(Item { name: spelled, kind: Kind::Property, detail: String::new() });
            }
        }
        for (name, params) in &scope.procs {
            if fits(name) && seen.insert(name.to_ascii_lowercase()) {
                items.push(Item { name: name.clone(), kind: Kind::Function, detail: params.clone() });
            }
        }
        for (name, _) in &scope.objects {
            if fits(name) && seen.insert(name.to_ascii_lowercase()) {
                items.push(Item { name: name.clone(), kind: Kind::ReadOnly, detail: String::new() });
            }
        }
        let globals: &[&str] = if scope.word { &["VBA", "Global", "Constants", "WordConstants", "Keywords"] } else { &["VBA", "Global", "Constants", "Keywords"] };
        for c in globals {
            for m in catalog::class_of(scope.word, c).map_or(&[][..], |c| &c.members) {
                if fits(m.name) && seen.insert(m.name.to_ascii_lowercase()) {
                    items.push(item(m));
                }
            }
        }
    } else {
        return None;
    }
    items.sort_by_key(|i| i.name.to_ascii_lowercase());
    items.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    Some((start, items))
}

fn item(m: &catalog::Member) -> Item {
    let detail = match (m.params.is_empty(), m.ty.is_empty()) {
        (true, true) => String::new(),
        (true, false) => format!("{}: {}", m.name, m.ty),
        (false, true) => format!("{}{}", m.name, m.params),
        (false, false) => format!("{}{}: {}", m.name, m.params, m.ty),
    };
    Item { name: m.name.to_string(), kind: m.kind, detail }
}

fn ends_with_word(s: &str, w: &str) -> bool {
    let t = s.trim_end();
    t.len() < s.len() && t.len() >= w.len() && t[t.len() - w.len()..].eq_ignore_ascii_case(w) && t[..t.len() - w.len()].chars().last().is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
}

fn in_string_or_comment(before: &str) -> bool {
    let mut in_str = false;
    for c in before.chars() {
        match c {
            '"' => in_str = !in_str,
            '\'' if !in_str => return true,
            _ => {}
        }
    }
    in_str
}

/// The parts of a chain of members ending the text (`Sheets("a").Range("A1")`
/// → [("Sheets", true), ("Range", true)]): name and whether it has
/// arguments. An empty first name is a leading dot (inside With).
fn chain(text: &str) -> Option<Vec<(String, bool)>> {
    let cs: Vec<char> = text.chars().collect();
    let mut i = cs.len();
    let mut parts = Vec::new();
    loop {
        let mut args = false;
        if i > 0 && cs[i - 1] == ')' {
            // Back over the arguments.
            let mut depth = 0;
            let mut in_str = false;
            while i > 0 {
                i -= 1;
                match cs[i] {
                    '"' => in_str = !in_str,
                    ')' if !in_str => depth += 1,
                    '(' if !in_str => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if depth != 0 {
                return None;
            }
            args = true;
        }
        let end = i;
        while i > 0 && (cs[i - 1].is_alphanumeric() || cs[i - 1] == '_') {
            i -= 1;
        }
        let name: String = cs[i..end].iter().collect();
        parts.push((name, args));
        if i > 0 && cs[i - 1] == '.' {
            i -= 1;
            continue;
        }
        break;
    }
    parts.reverse();
    // A leading dot stays as an empty name; anything else before is fine.
    if parts.first().is_some_and(|p| p.0.is_empty()) && !(i > 0 && text[..byte_at(text, i)].trim_end().ends_with(|c: char| !(c.is_alphanumeric() || c == '_' || c == ')'))) && i > 0 && !text[..byte_at(text, i)].trim().is_empty() {
        return None;
    }
    Some(parts)
}

/// The class of the chain before a dot at `at`, if known.
fn type_of(scope: &Scope, at: usize, text: &str) -> Option<&'static str> {
    let parts = chain(text)?;
    let mut class: Option<&'static str> = None;
    for (i, (name, args)) in parts.iter().enumerate() {
        let next = if i == 0 {
            if name.is_empty() { with_type(scope, at) } else { first_type(scope, at, name) }
        } else {
            let m = catalog::member_of(scope.word, class?, name)?;
            known(scope.word, m.ty)
        };
        // `Worksheets("a")`: an item of the collection.
        class = match next {
            Some(c) if *args && collection(scope.word, c) => known(scope.word, catalog::member_of(scope.word, c, "item")?.ty),
            n => n,
        };
    }
    class
}

/// `Worksheets("a")` (Word: `Paragraphs(1)`, `Tables(1)`...) is an item
/// of the collection.
fn collection(word: bool, c: &str) -> bool {
    catalog::member_of(word, c, "item").is_some() && (word || c == "Worksheets")
}

/// A class of the catalog by name (Variant, String... aren't).
fn known(word: bool, ty: &str) -> Option<&'static str> {
    let host_only = |n: &str| if word { n == "WordConstants" } else { n == "Document" };
    catalog::class_of(word, ty).filter(|c| !matches!(c.name, "Keywords" | "VBA" | "Global" | "Constants" | "Class") && !host_only(c.name)).map(|c| c.name)
}

fn first_type(scope: &Scope, at: usize, name: &str) -> Option<&'static str> {
    let low = name.to_ascii_lowercase();
    if low == "me" {
        return scope.me.and_then(|t| known(scope.word, t));
    }
    if let Some(ty) = variable_type(scope.code, at, &low) {
        return known(scope.word, &ty);
    }
    if let Some((_, class)) = scope.objects.iter().find(|o| o.0.eq_ignore_ascii_case(name)) {
        return known(scope.word, class);
    }
    if low == "err" {
        return Some("ErrObject");
    }
    let (c, m) = catalog::global_of(scope.word, name)?;
    if c.name == "Global" { known(scope.word, m.ty) } else { known(scope.word, m.name) }
}

/// The type a variable is declared with (`Dim r As Range`, `ws As
/// Worksheet` in the parameters), in the procedure at `at` or the module.
fn variable_type(code: &str, at: usize, low: &str) -> Option<String> {
    let b = byte_at(code, at);
    let lines: Vec<&str> = code[..b].lines().collect();
    // The procedure's lines, back to its Sub/Function line; then the
    // module's declarations (before the first procedure).
    let mut found = None;
    for line in lines.iter().rev() {
        if let Some(t) = declared_type(line, low) {
            found = Some(t);
            break;
        }
        let w = words(&bare(line));
        let proc = w.iter().take(3).any(|x| matches!(x.as_str(), "sub" | "function" | "property")) && !w.first().is_some_and(|x| x == "end" || x == "exit");
        if proc {
            break;
        }
    }
    found.or_else(|| {
        for line in code.lines() {
            let w = words(&bare(line));
            if w.iter().take(3).any(|x| matches!(x.as_str(), "sub" | "function" | "property")) {
                break;
            }
            if let Some(t) = declared_type(line, low) {
                return Some(t);
            }
        }
        None
    })
}

/// `name As [New] Type` in a declaration line.
fn declared_type(line: &str, low: &str) -> Option<String> {
    let b = bare(line);
    let w: Vec<String> = b.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).filter(|w| !w.is_empty()).map(str::to_string).collect();
    let lw: Vec<String> = w.iter().map(|x| x.to_ascii_lowercase()).collect();
    let decl = lw.first().is_some_and(|f| matches!(f.as_str(), "dim" | "public" | "private" | "static" | "global" | "sub" | "function" | "property" | "for"));
    if !decl {
        return None;
    }
    let i = lw.iter().position(|x| x == low)?;
    let mut j = i + 1;
    if lw.get(j).is_some_and(|x| x == "as") {
        j += 1;
        if lw.get(j).is_some_and(|x| x == "new") {
            j += 1;
        }
        return w.get(j).map(|t| t.rsplit('.').next().unwrap_or(t).to_string());
    }
    None
}

/// The object of the With block the cursor is in.
fn with_type(scope: &Scope, at: usize) -> Option<&'static str> {
    let code = scope.code;
    let b = byte_at(code, at);
    let mut depth = 0;
    let mut pos = b;
    for line in code[..b].lines().rev().skip(1) {
        pos -= line.len() + 1;
        let w = words(&bare(line));
        match (w.first().map(String::as_str), w.get(1).map(String::as_str)) {
            (Some("end"), Some("with")) => depth += 1,
            (Some("with"), _) if depth > 0 => depth -= 1,
            (Some("with"), _) => {
                let t = bare(line);
                let obj = t.trim_start()[4..].trim();
                let at_line = chars(&code[..pos.min(code.len())]);
                return type_of(scope, at_line, obj);
            }
            (Some("sub" | "function" | "property" | "private" | "public"), _) => return None,
            _ => {}
        }
    }
    None
}

/// Parameter info: the call the cursor is in the arguments of — the
/// signature to show and which argument the cursor is at.
pub(super) fn signature(scope: &Scope, at: usize) -> Option<(String, usize)> {
    let code = scope.code;
    let b = byte_at(code, at);
    let (start, _) = line_at(code, at);
    let before = &code[start..b];
    if in_string_or_comment(before) {
        return None;
    }
    // The innermost parenthesis still open, and the commas after it.
    let cs: Vec<char> = before.chars().collect();
    let mut depth = 0;
    let mut commas = 0;
    let mut in_str = false;
    let mut open = None;
    for i in (0..cs.len()).rev() {
        match cs[i] {
            '"' => in_str = !in_str,
            _ if in_str => {}
            ')' => depth += 1,
            '(' if depth == 0 => {
                open = Some(i);
                break;
            }
            '(' => depth -= 1,
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    let callee: String = match open {
        Some(i) => cs[..i].iter().collect(),
        None => {
            // `MsgBox "a", ` : a call without parentheses, first on the line.
            let t = before.trim_start();
            let name_len = t.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).unwrap_or(t.len());
            if name_len == 0 || !t[name_len..].starts_with(' ') {
                return None;
            }
            commas = t[name_len..].matches(',').count();
            t[..name_len].to_string()
        }
    };
    let parts = chain(callee.trim_end())?;
    let (last, _) = parts.last()?;
    let member = if parts.len() == 1 {
        if let Some((name, params)) = scope.procs.iter().find(|p| p.0.eq_ignore_ascii_case(last)) {
            return Some((format!("{name}{params}"), commas));
        }
        catalog::global_of(scope.word, last).map(|(_, m)| m)
    } else {
        let mut text = callee.trim_end().to_string();
        text.truncate(text.len() - last.len() - 1);
        let class = type_of(scope, at, &text)?;
        catalog::member_of(scope.word, class, last)
    }?;
    if member.params.is_empty() {
        return None;
    }
    let ty = if member.ty.is_empty() { String::new() } else { format!(": {}", member.ty) };
    Some((format!("{}{}{ty}", member.name, member.params), commas))
}

/// The procedures of a module's code with their parameters as written:
/// (name, `(a, [b])`).
pub(super) fn procedures(code: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in code.lines() {
        let b = bare(line);
        let w = words(&b);
        let mut i = 0;
        if w.first().is_some_and(|x| matches!(x.as_str(), "public" | "private" | "friend" | "static")) {
            i += 1;
        }
        if !w.get(i).is_some_and(|x| matches!(x.as_str(), "sub" | "function" | "property")) {
            continue;
        }
        let names = declarations(line);
        let Some(name) = names.first() else { continue };
        let params = match (b.find('('), b.rfind(')')) {
            (Some(o), Some(c)) if c > o => b[o..=c].to_string(),
            _ => "()".to_string(),
        };
        out.push((name.clone(), params));
    }
    out
}

/// The rows (0-based) where each procedure starts, with its name.
pub(super) fn proc_rows(code: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for (row, line) in code.split('\n').enumerate() {
        let w = words(&bare(line));
        let mut i = 0;
        if w.first().is_some_and(|x| matches!(x.as_str(), "public" | "private" | "friend" | "static")) {
            i += 1;
        }
        if w.get(i).is_some_and(|x| matches!(x.as_str(), "sub" | "function" | "property"))
            && let Some(name) = declarations(line).into_iter().next()
        {
            out.push((name, row));
        }
    }
    out
}

/// The event procedure `object_event` (`Worksheet_Change`): its row if
/// the module has it; else the code with its skeleton added at the end
/// and the row to write in.
pub(super) fn event_proc(code: &str, object: &str, event: &str, params: &str) -> Result<usize, (String, usize)> {
    let name = format!("{object}_{event}");
    if let Some((_, row)) = proc_rows(code).into_iter().find(|p| p.0.eq_ignore_ascii_case(&name)) {
        return Ok(row + 1);
    }
    let mut out = code.trim_end_matches(['\n', ' ', '\t']).to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    let params = if params.is_empty() { "()" } else { params };
    out.push_str(&format!("Private Sub {name}{params}\n{INDENT}\nEnd Sub\n"));
    let row = out.matches('\n').count() - 2;
    Err((out, row))
}

/// Where a name is declared (Mayús+F2): the module (index in `modules`)
/// and row. In `current`, a declaration in the procedure at `row` comes
/// first, then the module's; then other modules' public ones.
pub(super) fn definition(modules: &[&str], current: usize, row: usize, name: &str) -> Option<(usize, usize)> {
    let declares = |line: &str| declarations(line).iter().any(|d| d.eq_ignore_ascii_case(name));
    let code = modules.get(current)?;
    let lines: Vec<&str> = code.split('\n').collect();
    let procs = proc_rows(code);
    // The procedure the cursor is in: its first row up to the next one's.
    let start = procs.iter().rev().find(|p| p.1 <= row).map(|p| p.1);
    if let Some(start) = start {
        let end = procs.iter().find(|p| p.1 > start).map_or(lines.len(), |p| p.1);
        if let Some(r) = (start..end.min(lines.len())).find(|&r| declares(lines[r]) && (r > start || procs.iter().any(|p| p.1 == r && !p.0.eq_ignore_ascii_case(name)))) {
            return Some((current, r));
        }
    }
    // Module level: declarations before the first procedure, and procedures.
    let first = procs.first().map_or(lines.len(), |p| p.1);
    if let Some(r) = (0..first).find(|&r| declares(lines[r])) {
        return Some((current, r));
    }
    if let Some(p) = procs.iter().find(|p| p.0.eq_ignore_ascii_case(name)) {
        return Some((current, p.1));
    }
    for (i, code) in modules.iter().enumerate().filter(|m| m.0 != current) {
        let lines: Vec<&str> = code.split('\n').collect();
        let procs = proc_rows(code);
        let first = procs.first().map_or(lines.len(), |p| p.1);
        let private = |l: &str| words(&bare(l)).first().is_some_and(|w| w == "private" || w == "dim");
        if let Some(r) = (0..first).find(|&r| declares(lines[r]) && !private(lines[r])) {
            return Some((i, r));
        }
        if let Some(p) = procs.iter().find(|p| p.0.eq_ignore_ascii_case(name) && !private(lines[p.1])) {
            return Some((i, p.1));
        }
    }
    None
}

/// What Find looks for: the text and the options of the VBA editor's
/// Find dialog, plus regular expressions.
#[derive(Clone, Default)]
pub(super) struct Find {
    pub text: String,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
}

impl Find {
    fn pattern(&self) -> Result<regex::Regex, String> {
        let body = if self.regex { self.text.clone() } else { regex::escape(&self.text) };
        let body = if self.word { format!(r"\b(?:{body})\b") } else { body };
        let flags = if self.case { "" } else { "(?i)" };
        regex::RegexBuilder::new(&format!("{flags}{body}")).size_limit(1 << 20).build().map_err(|e| e.to_string())
    }

    /// Every match in `code`, as character ranges (empty ones skipped).
    pub(super) fn matches(&self, code: &str) -> Result<Vec<(usize, usize)>, String> {
        if self.text.is_empty() {
            return Ok(Vec::new());
        }
        let re = self.pattern()?;
        let mut out = Vec::new();
        let (mut byte, mut ch) = (0, 0);
        for m in re.find_iter(code).filter(|m| !m.is_empty()) {
            ch += code[byte..m.start()].chars().count();
            let len = m.as_str().chars().count();
            out.push((ch, ch + len));
            ch += len;
            byte = m.end();
        }
        Ok(out)
    }

    /// Replace All: the code and how many were replaced. With regular
    /// expressions `$1` is the first group.
    pub(super) fn replace_all(&self, code: &str, with: &str) -> Result<(String, usize), String> {
        if self.text.is_empty() {
            return Ok((code.to_string(), 0));
        }
        let re = self.pattern()?;
        let n = re.find_iter(code).filter(|m| !m.is_empty()).count();
        let out = if self.regex { re.replace_all(code, with) } else { re.replace_all(code, regex::NoExpand(with)) };
        Ok((out.into_owned(), n))
    }

    /// The text that replaces the match `found` (its groups, with regular
    /// expressions).
    pub(super) fn replacement(&self, found: &str, with: &str) -> String {
        match self.pattern() {
            Ok(re) if self.regex => re.replace(found, with).into_owned(),
            _ => with.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_mark(s: &str) -> (String, usize) {
        let i = s.find('|').unwrap();
        (s.replace('|', ""), chars(&s[..i]))
    }

    fn show(code: &str, at: usize) -> String {
        let b = byte_at(code, at);
        format!("{}|{}", &code[..b], &code[b..])
    }

    #[test]
    fn enter_indents_and_closes_blocks() {
        let e = |s: &str| {
            let (c, at) = at_mark(s);
            let (out, at) = enter(&c, at);
            show(&out, at)
        };
        assert_eq!(e("Sub A()|"), "Sub A()\n    |\nEnd Sub");
        assert_eq!(e("Sub A()\n    If x Then|\nEnd Sub"), "Sub A()\n    If x Then\n        |\n    End If\nEnd Sub");
        // Already closed: only the indent.
        assert_eq!(e("    For i = 1 To 3|\n        x = 1\n    Next"), "    For i = 1 To 3\n        |\n        x = 1\n    Next");
        assert_eq!(e("    If a Then|\n    Else\n    End If"), "    If a Then\n        |\n    Else\n    End If");
        // A one-line If opens nothing; plain lines keep their indent.
        assert_eq!(e("    If a Then b = 1|"), "    If a Then b = 1\n    |");
        assert_eq!(e("    x = 1|"), "    x = 1\n    |");
        // Mid-line: the rest moves down, nothing added.
        assert_eq!(e("    If a| Then"), "    If a\n    | Then");
        assert_eq!(e("    Select Case x|"), "    Select Case x\n        |\n    End Select");
        assert_eq!(e("        Case 1|\n    End Select"), "        Case 1\n            |\n    End Select");
        assert_eq!(e("    With Range(\"A1\")|"), "    With Range(\"A1\")\n        |\n    End With");
        assert_eq!(e("Private Function F(x) As Long|"), "Private Function F(x) As Long\n    |\nEnd Function");
        assert_eq!(e("    Do While x < 3 ' loop|"), "    Do While x < 3 ' loop\n        |\n    Loop");
    }

    #[test]
    fn block_indent_and_comments() {
        let code = "a\n    b\n\nc";
        let (out, sel) = indent_lines(code, (0, 4), false);
        assert_eq!(out, "    a\n        b\n\nc");
        assert_eq!(sel, (0, 15));
        let (back, _) = indent_lines(&out, (0, 15), true);
        assert_eq!(back, "a\n    b\n\nc");
        let (c, _) = comment_lines(code, (0, 3), true);
        assert_eq!(c, "'a\n    'b\n\nc");
        assert_eq!(comment_lines(&c, (0, 9), false).0, code);
        // A selection ending at the start of a line doesn't take it.
        assert_eq!(indent_lines("a\nb", (0, 2), false).0, "    a\nb");
    }

    #[test]
    fn known_words_in_their_case() {
        let names = declared(&["Dim TotalRows As Long\nSub DoIt(ByVal startRow, Optional n)\nEnd Sub\nPublic Const MAX_N = 5, Other = 2"]);
        assert_eq!(names.get("totalrows").map(String::as_str), Some("TotalRows"));
        assert_eq!(names.get("startrow").map(String::as_str), Some("startRow"));
        assert!(names.contains_key("n") && names.contains_key("doit") && names.contains_key("max_n") && names.contains_key("other"));
        let n = |s: &str| normalize_line(s, &names);
        assert_eq!(n("    if totalrows > 0 then msgbox \"msgbox\" ' msgbox"), "    If TotalRows > 0 Then MsgBox \"msgbox\" ' msgbox");
        assert_eq!(n("range(\"a1\").value = vbcrlf & x1"), "Range(\"a1\").Value = vbCrLf & x1");
        assert_eq!(n("dim r as range: set r = activesheet.usedrange"), "Dim r As Range: Set r = ActiveSheet.UsedRange");
        assert_eq!(n("rem msgbox"), "Rem msgbox");
        assert_eq!(n("x = 10e3 + 5#"), "x = 10e3 + 5#");
        assert_eq!(n("    r.Formula.b     = true"), "    r.Formula.b     = True");
        assert_eq!(enter("    r.Formula.b     = true", 26).0, "    r.Formula.b     = true\n    ");
    }

    fn scope(code: &str) -> Scope<'_> {
        Scope { code, word: false, me: Some("Worksheet"), objects: vec![("Sheet1".into(), "Worksheet"), ("ThisWorkbook".into(), "Workbook")], procs: vec![("Twice".into(), "(ByVal x As Long)".into())], classes: vec!["Point".into()] }
    }

    fn names(s: &str, forced: bool) -> Vec<String> {
        let (code, at) = at_mark(s);
        complete(&scope(&code), at, forced).map(|(_, items)| items.into_iter().map(|i| i.name).collect()).unwrap_or_default()
    }

    #[test]
    fn completion_of_words_objects() {
        let names = |s: &str, forced: bool| {
            let (code, at) = at_mark(s);
            let scope = Scope { code: &code, word: true, me: Some("Document"), objects: vec![("ThisDocument".into(), "Document")], procs: Vec::new(), classes: Vec::new() };
            complete(&scope, at, forced).map(|(_, items)| items.into_iter().map(|i| i.name).collect::<Vec<_>>()).unwrap_or_default()
        };
        assert!(names("ActiveDocument.|", false).contains(&"Paragraphs".to_string()));
        assert!(names("ActiveDocument.Paragraphs(1).Range.|", false).contains(&"InsertAfter".to_string()));
        assert!(names("Selection.Find.|", false).contains(&"Execute".to_string()));
        assert!(names("ThisDocument.Tables(1).|", false).contains(&"Cell".to_string()));
        assert!(names("Me.|", false).contains(&"Content".to_string()));
        assert!(names("Sub A()\n    Dim r As Range\n    r.|", false).contains(&"Collapse".to_string()));
        // Excel's members aren't Word's.
        assert!(!names("Sub A()\n    Dim r As Range\n    r.|", false).contains(&"Offset".to_string()));
        assert_eq!(names("x = wdCollapseE|", true), ["wdCollapseEnd"]);
        assert!(names("Documents.|", false).contains(&"Open".to_string()));
        let (code, at) = at_mark("ActiveDocument.Range(|");
        let scope = Scope { code: &code, word: true, me: None, objects: Vec::new(), procs: Vec::new(), classes: Vec::new() };
        assert!(signature(&scope, at).is_some_and(|s| s.0.starts_with("Range(")));
    }

    #[test]
    fn completion_by_type() {
        assert!(names("Range(\"A1\").|", false).contains(&"Offset".to_string()));
        assert_eq!(names("Range(\"A1\").Fo|", false), ["Font", "FormatConditions", "Formula", "Formula2", "FormulaHidden", "FormulaLocal", "FormulaR1C1", "FormulaR1C1Local"]);
        assert_eq!(names("x = Cells(1, 2).Font.B|", false), ["Bold"]);
        assert!(names("Worksheets(\"Data\").|", false).contains(&"UsedRange".to_string()));
        assert!(names("Worksheets.|", false).contains(&"Add".to_string()));
        assert!(names("Sub A()\n    Dim ws As Worksheet\n    ws.|", false).contains(&"Name".to_string()));
        assert!(names("Sub A(r As Range)\n    r.Offset(1).|", false).contains(&"Value".to_string()));
        assert!(names("Sub A()\n    With ActiveSheet.Range(\"A1\")\n        .|", false).contains(&"Interior".to_string()));
        assert!(names("Me.|", false).contains(&"Cells".to_string()));
        assert!(names("Sheet1.|", false).contains(&"Range".to_string()));
        assert!(names("ThisWorkbook.|", false).contains(&"Worksheets".to_string()));
        assert!(names("Application.|", false).contains(&"ScreenUpdating".to_string()));
        assert_eq!(names("Err.N|", false), ["Number"]);
        assert!(names("Dim d As New Scripting.Dictionary\nd.|", false).contains(&"Exists".to_string()));
        // Unknown: nothing.
        assert!(names("Dim v\nv.|", false).is_empty());
        assert!(names("x = \"a.|", false).is_empty());
        // After As: types and the project's classes.
        assert!(names("Dim p As Po|", false) == ["Point"]);
        assert!(names("Dim r As |", false).contains(&"Range".to_string()));
        // Ctrl+Space: the names in scope.
        let all = names("Dim TotalRows\nSub A()\n    msg|", true);
        assert_eq!(all, ["MsgBox"]);
        assert_eq!(names("Dim TotalRows\nSub A()\n    tot|", true), ["TotalRows"]);
        assert_eq!(names("Sub A()\n    Tw|", true), ["Twice"]);
        // Without Ctrl+Space, a word alone shows nothing.
        assert!(names("msg|", false).is_empty());
    }

    #[test]
    fn parameter_info() {
        let s = |t: &str| {
            let (code, at) = at_mark(t);
            signature(&scope(&code), at)
        };
        assert_eq!(s("MsgBox(\"a\", |"), Some(("MsgBox(Prompt, [Buttons], [Title]): VbMsgBoxResult".into(), 1)));
        assert_eq!(s("MsgBox \"a\", vbYesNo, |"), Some(("MsgBox(Prompt, [Buttons], [Title]): VbMsgBoxResult".into(), 2)));
        assert_eq!(s("x = Range(\"A1\").Offset(Len(\"x\"), |").map(|x| x.1), Some(1));
        assert_eq!(s("x = Range(\"A1\").Offset(Len(|").map(|x| x.0), Some("Len(Expression): Long".into()));
        assert_eq!(s("y = Twice(|"), Some(("Twice(ByVal x As Long)".into(), 0)));
        assert_eq!(s("y = 1 + (|"), None);
        assert_eq!(s("x = \"Len(|"), None);
        // Letters of other alphabets in names, before the cursor.
        assert_eq!(s("Ω|"), None);
        assert_eq!(s("ÑandúΩ x, |"), None);
    }

    #[test]
    fn procedures_listed() {
        let p = procedures("Option Explicit\nPublic Sub A()\nEnd Sub\nPrivate Function B(x, Optional y = 2) As Long\nEnd Function\nProperty Get C() As Long\nEnd Property\n' Sub D()\n");
        assert_eq!(p, [("A".to_string(), "()".to_string()), ("B".into(), "(x, Optional y = 2)".into()), ("C".into(), "()".into())]);
    }
    #[test]
    fn events_and_definitions() {
        let code = "Option Explicit\nDim Total As Long\n\nSub A()\n    Dim x\n    x = Total\nEnd Sub\n\nPrivate Sub Worksheet_Change(ByVal Target As Range)\nEnd Sub\n";
        assert_eq!(proc_rows(code), [("A".to_string(), 3), ("Worksheet_Change".into(), 8)]);
        assert_eq!(event_proc(code, "Worksheet", "change", "(ByVal Target As Range)"), Ok(9));
        let (new, row) = event_proc(code, "Worksheet", "Activate", "()").unwrap_err();
        assert!(new.ends_with("End Sub\n\nPrivate Sub Worksheet_Activate()\n    \nEnd Sub\n"));
        assert_eq!(new.split('\n').nth(row), Some("    "));
        assert_eq!(event_proc("", "Workbook", "Open", "()").unwrap_err(), ("Private Sub Workbook_Open()\n    \nEnd Sub\n".to_string(), 1));
        let other = "Public Function Helper()\nEnd Function\nPrivate Sub Hidden()\nEnd Sub";
        let m = [code, other];
        assert_eq!(definition(&m, 0, 5, "x"), Some((0, 4)));
        assert_eq!(definition(&m, 0, 5, "total"), Some((0, 1)));
        assert_eq!(definition(&m, 0, 5, "a"), Some((0, 3)));
        assert_eq!(definition(&m, 0, 5, "helper"), Some((1, 0)));
        assert_eq!(definition(&m, 0, 5, "hidden"), None);
        // A parameter: the procedure's own line.
        assert_eq!(definition(&m, 0, 9, "target"), Some((0, 8)));
    }
    #[test]
    fn find_and_replace() {
        let code = "Dim total\ntotal = Total + subtotal\nMsgBox \"total\"";
        let f = |text: &str, case, word, regex| Find { text: text.into(), case, word, regex };
        assert_eq!(f("total", false, false, false).matches(code).unwrap().len(), 5);
        assert_eq!(f("total", true, false, false).matches(code).unwrap().len(), 4);
        assert_eq!(f("total", false, true, false).matches(code).unwrap(), [(4, 9), (10, 15), (18, 23), (43, 48)]);
        assert_eq!(f("t.tal", false, true, true).matches(code).unwrap().len(), 4);
        assert_eq!(f("t.tal", false, false, false).matches(code).unwrap().len(), 0);
        assert!(f("(", false, false, true).matches(code).is_err());
        let (out, n) = f("total", false, true, false).replace_all(code, "Sum$1").unwrap();
        assert_eq!((out.as_str(), n), ("Dim Sum$1\nSum$1 = Sum$1 + subtotal\nMsgBox \"Sum$1\"", 4));
        let (out, n) = f(r"(\w+) = (\w+)", false, false, true).replace_all(code, "$2 = $1").unwrap();
        assert_eq!((out.lines().nth(1), n), (Some("Total = total + subtotal"), 1));
        assert_eq!(f(r"(\w+)tal", false, false, true).replacement("subtotal", "[$1]"), "[subto]");
        // Accents count as letters; positions are characters.
        assert_eq!(f("año", false, true, false).matches("x = año + años").unwrap(), [(4, 7)]);
    }
}
