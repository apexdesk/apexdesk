//! `apexdesk inspect FILE...`: what each file carries that doesn't show
//! (the File > Inspect File report), and `--remove` to take it out for
//! good. Signatures are counted the way `inventory` does.

use std::path::Path;

use apex_io::Loaded;
use apex_io::inspect::{self, Found, Hidden};

use super::args::Parsed;
use super::{Fail, Res, emit, error, open, output_of, usage};

pub const INSPECT_HELP: &str = "Usage: apexdesk inspect FILE... [options]

Lists what each spreadsheet, document or presentation carries that
doesn't show on screen: macros, links to other files, hidden sheets,
rows, columns and slides, comments, tracked changes, properties,
signatures, data connections, custom XML, embedded objects...
One line per kind: its name, how many and which.

Options:
      --json           print the report as JSON
      --remove KINDS   remove these kinds (comma-separated names as the
                       report prints them, or 'all') and save
  -o, --output FILE    with --remove: save to FILE (default: the file itself)
      --password PW    password of an encrypted file
";

/// The report of one loaded file, with its signatures (counted in the
/// bytes on disk).
fn report(loaded: &Loaded, data: &[u8], ext: &str) -> Vec<Found> {
    let mut out = match loaded {
        Loaded::Sheet(wb) => inspect::workbook(wb),
        Loaded::Doc(d) => inspect::document(d),
        Loaded::Slides(d) => inspect::deck(d),
    };
    let sigs = apex_io::probe::facts(data, ext).signatures;
    if sigs > 0 {
        out.push(Found { what: Hidden::Signatures, count: sigs, items: Vec::new() });
    }
    out
}

fn json_str(s: &str) -> String {
    let mut o = String::from('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn text_line(f: &Found) -> String {
    let items = if f.items.is_empty() { String::new() } else { format!(" ({})", super::one_line(&f.items.join("; "))) };
    format!("  {}: {}{items}\n", f.what.code(), f.count)
}

fn json_line(f: &Found) -> String {
    format!("{{\"kind\":{},\"count\":{},\"items\":[{}]}}", json_str(f.what.code()), f.count, f.items.iter().map(|i| json_str(i)).collect::<Vec<_>>().join(","))
}

pub fn inspect(a: &Parsed) -> Res<i32> {
    if a.pos.is_empty() {
        return usage("no file given");
    }
    if let Some(r) = a.value("remove") {
        let [file] = a.pos.as_slice() else { return usage("--remove takes one file") };
        return remove(Path::new(file), r, a);
    }
    let json = a.flag("json");
    let mut out = String::new();
    let mut entries = Vec::new();
    for f in &a.pos {
        let path = Path::new(f);
        let data = std::fs::read(path).map_err(|e| Fail::Error(format!("{f}: {e}")))?;
        let (loaded, _) = open(path, a)?;
        let ext = path.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let found = report(&loaded, &data, &ext);
        if json {
            entries.push(format!("{{\"file\":{},\"found\":[{}]}}", json_str(f), found.iter().map(json_line).collect::<Vec<_>>().join(",")));
        } else {
            out.push_str(&format!("{f}\n"));
            if found.is_empty() {
                out.push_str("  nothing hidden\n");
            }
            out.extend(found.iter().map(text_line));
        }
    }
    if json {
        out = format!("[{}]\n", entries.join(","));
    }
    emit(&out);
    Ok(0)
}

fn remove(path: &Path, kinds: &str, a: &Parsed) -> Res<i32> {
    let mut list = Vec::new();
    for k in kinds.split(',').map(str::trim).filter(|k| !k.is_empty()) {
        if k == "all" {
            list.extend(Hidden::ALL);
        } else {
            list.push(Hidden::from_code(k).ok_or_else(|| Fail::Usage(format!("unknown kind '{k}'")))?);
        }
    }
    let (mut loaded, fmt) = open(path, a)?;
    let (out, out_fmt) = output_of(path, fmt, a)?;
    if out_fmt.kind() != fmt.kind() || !out_fmt.can_save() {
        return error(format!("{}: can't save as .{}", out.display(), out_fmt.extension()));
    }
    let mut removed = 0;
    for &k in &list {
        let done = match &mut loaded {
            Loaded::Sheet(wb) => inspect::clean_workbook(wb, k, false),
            Loaded::Doc(d) => inspect::clean_document(d, k),
            Loaded::Slides(d) => {
                let mut p = apex_slides::Presentation::new((**d).clone());
                let done = inspect::clean_deck(&mut p, k, false);
                **d = p.deck;
                done
            }
        };
        removed += done as usize;
    }
    // A file written again carries no signature: nothing else to do for them.
    let pw = a.value("password").filter(|_| apex_io::can_encrypt(out_fmt));
    let res = match &loaded {
        Loaded::Sheet(wb) => apex_io::save_sheet_with(wb, &out, out_fmt, pw),
        Loaded::Doc(d) => apex_io::save_doc_with(d, &out, out_fmt, pw),
        Loaded::Slides(d) => apex_io::save_slides_with(d, &out, out_fmt, pw),
    };
    res.map_err(|e| Fail::Error(format!("{}: {e}", out.display())))?;
    eprintln!("{}: {removed} kinds removed", out.display());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_and_removes() {
        let dir = std::env::temp_dir().join(format!("apexdesk-inspect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.xlsx");
        let mut wb = apex_sheet::Workbook::new();
        wb.execute(apex_sheet::Op::AddSheet { index: 1, sheet: Box::new(apex_sheet::Sheet::new("Secret")) });
        wb.execute(apex_sheet::Op::SheetProps { index: 1, hidden: true, tab_color: None });
        wb.meta.author = "Ana".into();
        apex_io::save_sheet(&wb, &src, apex_io::Format::Xlsx).unwrap();
        let data = std::fs::read(&src).unwrap();
        let (loaded, _) = apex_io::open(&src).unwrap();
        let found = report(&loaded, &data, "xlsx");
        let kinds: Vec<Hidden> = found.iter().map(|f| f.what).collect();
        assert!(kinds.contains(&Hidden::HiddenSheets) && kinds.contains(&Hidden::Properties), "{kinds:?}");
        assert!(json_line(&found[0]).starts_with("{\"kind\":"));
        let out = dir.join("b.xlsx");
        let args: Vec<String> = [src.to_str().unwrap(), "--remove", "hidden-sheets,properties", "-o", out.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
        let a = super::super::args::parse(&args, &super::super::args::Spec { values: &["remove", "output", "password"], flags: &["json"] }).unwrap();
        assert_eq!(inspect(&a).ok(), Some(0));
        let (loaded, _) = apex_io::open(&out).unwrap();
        let Loaded::Sheet(wb) = &loaded else { panic!() };
        assert_eq!(wb.sheets.len(), 1);
        assert!(report(&loaded, &std::fs::read(&out).unwrap(), "xlsx").iter().all(|f| f.what != Hidden::HiddenSheets && f.what != Hidden::Properties));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
