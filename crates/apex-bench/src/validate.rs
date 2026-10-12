//! `apex-bench validate`: what Office and ODF programs would say about the
//! files we save. A sample of the corpus is opened and saved as
//! docx/odt (documents) or xlsx/ods (spreadsheets), each in a child
//! process; then the saved files are checked with Microsoft's Open XML SDK
//! validator (tools/ooxml-validate, .NET) and the ODF Toolkit's
//! odfvalidator (Java). Errors are grouped by kind and element.
//!
//!   apex-bench validate PATH... [--out DIR] [--saved] [--limit N] [--jobs N] [--tools DIR]
//!
//! `--saved` validates the given files as they are (already saved by us).
//! `--tools` is the directory with dotnet/, ooxml-validate/ and
//! odfvalidator-*.jar (default: target/tools; tools/ooxml-validate/setup.sh
//! installs them).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{Value as Json, json};

use crate::oracle_proc;

const SOURCES: &[&str] = &["xlsx", "xlsm", "xls", "ods", "docx", "docm", "doc", "odt", "rtf"];

/// `apex-bench save-as FILE OUTDIR`: the child that opens and saves one
/// file. Prints one JSON line.
pub fn save_main(args: &[String]) -> Result<(), String> {
    let [file, dir] = args else { return Err("usage: save-as FILE OUTDIR".into()) };
    let path = Path::new(file);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut h);
    // The stem without dots, so that the saved extension is the only one.
    let stem: String = path.file_stem().map(|s| s.to_string_lossy().chars().take(40).map(|c| if c == '.' { '_' } else { c }).collect()).unwrap_or_default();
    let base = Path::new(dir).join(format!("{stem}-{:08x}", h.finish() as u32));
    let r = match apex_io::open(path) {
        Err(e) => json!({"file": file, "status": "error", "reason": format!("open: {e}")}),
        Ok((loaded, _)) => {
            let mut saved = Vec::new();
            let mut errors = Vec::new();
            let targets: &[(&str, apex_io::Format)] = match loaded {
                apex_io::Loaded::Sheet(_) => &[("xlsx", apex_io::Format::Xlsx), ("ods", apex_io::Format::Ods)],
                apex_io::Loaded::Doc(_) => &[("docx", apex_io::Format::Docx), ("odt", apex_io::Format::Odt)],
                apex_io::Loaded::Slides(_) => &[("pptx", apex_io::Format::Pptx)],
            };
            for (ext, fmt) in targets {
                let out = PathBuf::from(format!("{}.{ext}", base.display()));
                let res = match &loaded {
                    apex_io::Loaded::Sheet(wb) => apex_io::save_sheet(wb, &out, *fmt),
                    apex_io::Loaded::Doc(doc) => apex_io::save_doc(doc, &out, *fmt),
                    apex_io::Loaded::Slides(deck) => apex_io::save_slides(deck, &out, *fmt),
                };
                match res {
                    Ok(()) => saved.push(out.display().to_string()),
                    Err(e) => errors.push(format!("{ext}: {e}")),
                }
            }
            json!({"file": file, "status": "saved", "saved": saved, "errors": errors})
        }
    };
    println!("{r}");
    Ok(())
}

struct Opts {
    paths: Vec<String>,
    out: PathBuf,
    saved: bool,
    limit: usize,
    jobs: usize,
    tools: PathBuf,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let target = crate::oracle::default_out("x").parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("target"));
    let mut o = Opts { paths: Vec::new(), out: crate::oracle::default_out("oracle"), saved: false, limit: 300, jobs: 2, tools: std::env::var_os("APEX_BENCH_TOOLS").map(PathBuf::from).unwrap_or_else(|| target.join("tools")) };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--out" => o.out = PathBuf::from(val()?),
            "--saved" => o.saved = true,
            "--limit" => o.limit = val()?.parse().map_err(|_| "--limit: a number")?,
            "--jobs" => o.jobs = val()?.parse().map_err(|_| "--jobs: a number")?,
            "--tools" => o.tools = PathBuf::from(val()?),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => o.paths.push(a.clone()),
        }
    }
    if o.paths.is_empty() {
        return Err("no files or directories given".into());
    }
    o.jobs = o.jobs.clamp(1, 2);
    Ok(o)
}

/// An even spread of `n` items.
fn spread<T: Clone>(v: Vec<T>, n: usize) -> Vec<T> {
    if n == 0 || v.len() <= n {
        return v;
    }
    let step = v.len() as f64 / n as f64;
    (0..n).map(|k| v[(k as f64 * step) as usize].clone()).collect()
}

pub fn main(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    let saved_dir = o.out.join("saved");
    std::fs::create_dir_all(&saved_dir).map_err(|e| format!("{}: {e}", saved_dir.display()))?;
    // (saved file, source file)
    let mut files: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut save_rows = Vec::new();
    if o.saved {
        for f in spread(oracle_proc::files(&o.paths, &["docx", "docm", "xlsx", "xlsm", "odt", "ods"]), o.limit) {
            files.push((f.clone(), f));
        }
    } else {
        // A sample balanced between the sources given (each path its share).
        let per = (o.limit / o.paths.len()).max(1);
        let mut sources = Vec::new();
        for p in &o.paths {
            sources.extend(spread(oracle_proc::files(std::slice::from_ref(p), SOURCES), per));
        }
        eprintln!("validate: saving {} files", sources.len());
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("apex-bench"));
        save_rows = oracle_proc::pool(
            &sources,
            o.jobs,
            |f| {
                let mut cmd = Command::new(&exe);
                cmd.arg("save-as").arg(f).arg(&saved_dir);
                let out = oracle_proc::run(cmd, Duration::from_secs(300), Some(1536 << 20));
                out.stdout.lines().rev().find_map(|l| serde_json::from_str::<Json>(l).ok()).unwrap_or_else(|| {
                    let status = if out.timed_out { "timeout" } else { "crash" };
                    let why = crate::oracle::crash_reason(&out.stderr);
                    json!({"file": f.display().to_string(), "status": status, "reason": why})
                })
            },
            |_, _| {},
        );
        for r in &save_rows {
            for s in r["saved"].as_array().into_iter().flatten() {
                files.push((PathBuf::from(s.as_str().unwrap_or("")), PathBuf::from(r["file"].as_str().unwrap_or(""))));
            }
        }
    }
    let ooxml: Vec<&(PathBuf, PathBuf)> = files.iter().filter(|(f, _)| matches!(ext(f).as_str(), "docx" | "docm" | "xlsx" | "xlsm")).collect();
    let odf: Vec<&(PathBuf, PathBuf)> = files.iter().filter(|(f, _)| matches!(ext(f).as_str(), "odt" | "ods")).collect();
    eprintln!("validate: {} OOXML and {} ODF files", ooxml.len(), odf.len());
    let ooxml_rows = run_ooxml(&o, &ooxml);
    let odf_rows = run_odf(&o, &odf);
    let mut all = String::new();
    for r in save_rows.iter().chain(&ooxml_rows).chain(&odf_rows) {
        all.push_str(&r.to_string());
        all.push('\n');
    }
    std::fs::write(o.out.join("validate.jsonl"), all).map_err(|e| e.to_string())?;
    std::fs::write(o.out.join("validate.md"), report(&save_rows, &ooxml_rows, &odf_rows, o.saved)).map_err(|e| e.to_string())?;
    crate::oracle::assemble_report(&o.out)?;
    eprintln!("report: {}", o.out.join("report.md").display());
    Ok(())
}

fn ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Runs the Open XML SDK validator in batches; one row per file with its
/// errors.
fn run_ooxml(o: &Opts, files: &[&(PathBuf, PathBuf)]) -> Vec<Json> {
    if files.is_empty() {
        return Vec::new();
    }
    let dotnet = o.tools.join("dotnet").join(if cfg!(windows) { "dotnet.exe" } else { "dotnet" });
    let dll = o.tools.join("ooxml-validate").join("ooxml-validate.dll");
    if !dll.exists() {
        eprintln!("validate: {} missing (build tools/ooxml-validate, see its Program.cs)", dll.display());
        return Vec::new();
    }
    let batches: Vec<(usize, Vec<&(PathBuf, PathBuf)>)> = files.chunks(40).map(|c| c.to_vec()).enumerate().collect();
    let rows = oracle_proc::pool(
        &batches,
        o.jobs.min(2),
        |(k, batch)| {
            let list = o.out.join(format!("saved/.list-{k}.txt"));
            let names: Vec<String> = batch.iter().map(|(f, _)| f.display().to_string()).collect();
            let _ = std::fs::write(&list, names.join("\n"));
            let mut cmd = Command::new(&dotnet);
            cmd.arg(&dll).arg(format!("@{}", list.display()));
            cmd.env("DOTNET_CLI_HOME", o.tools.join("dotnet-home")).env("DOTNET_CLI_TELEMETRY_OPTOUT", "1").env("DOTNET_NOLOGO", "1");
            let out = oracle_proc::run(cmd, Duration::from_secs(900), None);
            let _ = std::fs::remove_file(&list);
            let mut got: BTreeMap<String, Json> = BTreeMap::new();
            for l in out.stdout.lines() {
                if let Ok(j) = serde_json::from_str::<Json>(l) {
                    got.insert(j["file"].as_str().unwrap_or("").to_string(), j);
                }
            }
            batch
                .iter()
                .map(|(f, src)| {
                    let mut r = got.remove(&f.display().to_string()).unwrap_or_else(|| json!({"file": f.display().to_string(), "ok": false, "open_error": format!("validator gave no result: {}", out.stderr.lines().last().unwrap_or(""))}));
                    r["validator"] = json!("ooxml");
                    r["source"] = json!(src.display().to_string());
                    r["size"] = json!(size(src));
                    r
                })
                .collect::<Vec<_>>()
        },
        |d, n| eprintln!("ooxml-validate: batch {d}/{n}"),
    );
    rows.into_iter().flatten().collect()
}

/// Runs odfvalidator in batches; one row per file with its messages.
fn run_odf(o: &Opts, files: &[&(PathBuf, PathBuf)]) -> Vec<Json> {
    if files.is_empty() {
        return Vec::new();
    }
    let jar = std::fs::read_dir(&o.tools).ok().and_then(|rd| {
        rd.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("odfvalidator") && n.ends_with(".jar"))).max()
    });
    let Some(jar) = jar else {
        eprintln!("validate: no odfvalidator-*.jar in {}", o.tools.display());
        return Vec::new();
    };
    let batches: Vec<Vec<&(PathBuf, PathBuf)>> = files.chunks(40).map(|c| c.to_vec()).collect();
    let rows = oracle_proc::pool(
        &batches,
        o.jobs.min(2),
        |batch| {
            let mut cmd = Command::new("java");
            // Extended conformance: foreign elements (LibreOffice's loext:
            // and the like) are allowed, as in the files LibreOffice writes.
            cmd.arg("-Xmx1g").arg("-jar").arg(&jar).arg("-e").arg("-w");
            for (f, _) in batch {
                cmd.arg(f);
            }
            let out = oracle_proc::run(cmd, Duration::from_secs(900), None);
            let text = format!("{}\n{}", out.stdout, out.stderr);
            batch
                .iter()
                .map(|(f, src)| {
                    let name = f.display().to_string();
                    let msgs = odf_messages(&text, &name);
                    let errors: Vec<&Json> = msgs.iter().filter(|m| m["level"] == "Error" || m["level"] == "Fatal").collect();
                    json!({"validator": "odf", "file": name, "ok": errors.is_empty(), "errors": msgs, "source": src.display().to_string(), "size": size(src)})
                })
                .collect::<Vec<_>>()
        },
        |d, n| eprintln!("odfvalidator: batch {d}/{n}"),
    );
    rows.into_iter().flatten().collect()
}

/// odfvalidator's lines about `file`: `FILE/content.xml[3,120]:  Error: message`
/// or `FILE:  Error: message`.
pub fn odf_messages(text: &str, file: &str) -> Vec<Json> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix(file) else { continue };
        let (part, msg) = match rest.find(':') {
            Some(i) => (&rest[..i], rest[i + 1..].trim()),
            None => continue,
        };
        let part = part.trim_start_matches('/');
        let part = part.split('[').next().unwrap_or("");
        let Some((level, text)) = msg.split_once(':') else { continue };
        let level = level.trim();
        if !matches!(level, "Error" | "Fatal" | "Warning") {
            continue;
        }
        out.push(json!({"level": level, "part": part, "description": text.trim()}));
    }
    out
}

/// A message without its specifics (numbers, quoted values) for grouping.
pub fn normalize(msg: &str) -> String {
    let mut out = String::new();
    let mut chars = msg.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            out.push('N');
            while chars.peek().is_some_and(|d| d.is_ascii_digit() || *d == '.') {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    // The value in 'value "xyz" is not valid' style messages.
    for q in ["value \"", "value '"] {
        if let Some(i) = out.find(q) {
            let close = q.chars().last().unwrap_or('"');
            if let Some(j) = out[i + q.len()..].find(close) {
                out.replace_range(i + q.len()..i + q.len() + j, "…");
            }
        }
    }
    out.chars().take(160).collect()
}

/// The first double-quoted word of a message (the element or attribute
/// odfvalidator complains about).
fn first_quoted(msg: &str) -> Option<&str> {
    let a = msg.find('"')?;
    let b = msg[a + 1..].find('"')?;
    Some(&msg[a + 1..a + 1 + b])
}

/// A part name with numbers generalized: /xl/worksheets/sheet12.xml ->
/// /xl/worksheets/sheetN.xml.
fn part_kind(p: &str) -> String {
    let mut out = String::new();
    let mut prev = false;
    for c in p.chars() {
        if c.is_ascii_digit() {
            if !prev {
                out.push('N');
            }
            prev = true;
        } else {
            out.push(c);
            prev = false;
        }
    }
    out
}

fn md(s: &str) -> String {
    s.chars().take(140).collect::<String>().replace('|', "\\|").replace('\n', " ")
}

pub fn report(saves: &[Json], ooxml: &[Json], odf: &[Json], as_is: bool) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "## Validation of the files we save\n");
    if as_is {
        let _ = writeln!(s, "Files validated as given (`--saved`).\n");
    } else {
        let ok = saves.iter().filter(|r| r["status"] == "saved").count();
        let _ = writeln!(s, "A sample of {} corpus files opened and saved (documents as docx and odt, spreadsheets as xlsx and ods): {} saved, {} failed to open or crashed.\n", saves.len(), ok, saves.len() - ok);
        let mut fails: Vec<&Json> = saves.iter().filter(|r| r["status"] != "saved" || r["errors"].as_array().is_some_and(|e| !e.is_empty())).collect();
        fails.sort_by_key(|r| size(Path::new(r["file"].as_str().unwrap_or(""))));
        if !fails.is_empty() {
            let _ = writeln!(s, "| status | file | reason |\n|---|---|---|");
            for r in fails.iter().take(15) {
                let why = r["reason"].as_str().map(str::to_string).unwrap_or_else(|| r["errors"].to_string());
                let _ = writeln!(s, "| {} | {} | {} |", r["status"].as_str().unwrap_or(""), md(crate::oracle::short_path(r["file"].as_str().unwrap_or(""))), md(&why));
            }
            let _ = writeln!(s);
        }
    }
    for (name, rows, key) in [("Open XML SDK (Office 365 rules)", ooxml, "ooxml"), ("ODF Toolkit odfvalidator", odf, "odf")] {
        let valid = rows.iter().filter(|r| r["ok"] == true).count();
        let _ = writeln!(s, "### {name}\n");
        let _ = writeln!(s, "{} files validated, {} valid ({}).\n", rows.len(), valid, if rows.is_empty() { "-".into() } else { format!("{:.1}%", 100.0 * valid as f64 / rows.len() as f64) });
        // By saved format.
        let mut by_ext: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for r in rows {
            let e = by_ext.entry(ext(Path::new(r["file"].as_str().unwrap_or("")))).or_default();
            e.0 += 1;
            if r["ok"] == true {
                e.1 += 1;
            }
        }
        let _ = writeln!(s, "| format | files | valid |\n|---|---:|---:|");
        for (e, (n, v)) in &by_ext {
            let _ = writeln!(s, "| {e} | {n} | {v} |");
        }
        // Groups: (kind, element or part, message) with the smallest source.
        let mut groups: BTreeMap<(String, String, String), (u64, BTreeSet<String>, Option<(u64, String, String)>)> = BTreeMap::new();
        for r in rows {
            let src = r["source"].as_str().unwrap_or("").to_string();
            let sz = r["size"].as_u64().unwrap_or(0);
            let mut items: Vec<(String, String, String, String)> = Vec::new();
            if let Some(e) = r["open_error"].as_str() {
                items.push(("OpenError".into(), String::new(), normalize(e), e.to_string()));
            }
            for e in r["errors"].as_array().into_iter().flatten() {
                if key == "odf" && e["level"] == "Warning" {
                    continue;
                }
                let desc = e["description"].as_str().unwrap_or("").to_string();
                if key == "ooxml" {
                    let id = e["id"].as_str().unwrap_or("").to_string();
                    let el = format!("{} in {}", e["element"].as_str().unwrap_or("?"), part_kind(e["part"].as_str().unwrap_or("")));
                    items.push((id, el, normalize(&desc), desc));
                } else {
                    let el = format!("{} in {}", first_quoted(&desc).unwrap_or("?"), e["part"].as_str().unwrap_or(""));
                    items.push((e["level"].as_str().unwrap_or("").to_string(), el, normalize(&desc), desc));
                }
            }
            for (k, el, norm, desc) in items {
                let g = groups.entry((k, el, norm)).or_insert((0, BTreeSet::new(), None));
                g.0 += 1;
                g.1.insert(src.clone());
                if g.2.as_ref().is_none_or(|(z, _, _)| sz < *z) {
                    g.2 = Some((sz, src.clone(), desc));
                }
            }
        }
        let mut gv: Vec<_> = groups.into_iter().collect();
        gv.sort_by_key(|(_, (n, files, _))| (std::cmp::Reverse(files.len()), std::cmp::Reverse(*n)));
        if !gv.is_empty() {
            let _ = writeln!(s, "\nErrors grouped by kind and element (most files first):\n");
            let _ = writeln!(s, "| kind | element / part | errors | files | smallest source file | message |\n|---|---|---:|---:|---|---|");
            for ((k, el, _), (n, files, ex)) in gv.iter().take(40) {
                let (file, desc) = ex.as_ref().map(|(_, f, d)| (f.as_str(), d.as_str())).unwrap_or(("", ""));
                let _ = writeln!(s, "| {} | {} | {n} | {} | {} | {} |", md(k), md(el), files.len(), md(crate::oracle::short_path(file)), md(desc));
            }
        }
        let _ = writeln!(s);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_odf_messages() {
        let text = "/t/a.odt/content.xml[2,120]:  Error: element \"text:foo\" was found where no element may occur\n/t/a.odt:  Info: ODF version 1.3\n/t/a.odt/styles.xml[1,5]:  Warning: unexpected attribute \"x\"\n/t/b.odt/content.xml[1,1]:  Error: other\n";
        let m = odf_messages(text, "/t/a.odt");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0]["part"], "content.xml");
        assert_eq!(m[0]["level"], "Error");
        assert_eq!(m[1]["level"], "Warning");
        assert_eq!(normalize("value \"12pt\" of attribute at 3,120 is invalid"), "value \"…\" of attribute at N,N is invalid");
        assert_eq!(part_kind("/xl/worksheets/sheet12.xml"), "/xl/worksheets/sheetN.xml");
        assert_eq!(first_quoted("tag name \"loext:x\" is not allowed"), Some("loext:x"));
    }

    #[test]
    fn groups_errors() {
        let rows = report(&[json!({"file": "in.md", "status": "saved", "saved": [], "errors": []})], &[json!({"file": "x.docx", "ok": false, "source": "in.md", "size": 3, "errors": [{"id": "Sch_X", "element": "w:foo", "part": "/word/document.xml", "description": "bad"}]})], &[], false);
        assert!(rows.contains("| Sch_X | w:foo in /word/document.xml | 1 | 1 | in.md | bad |"), "{rows}");
    }
}
