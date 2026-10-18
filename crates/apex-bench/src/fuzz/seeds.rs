//! Seeds: files found in the folders given (corpus, samples) and a set
//! made here (`apex-bench fuzz --make-seeds DIR`): workbooks and
//! documents saved by ApexDesk in every format, with a VBA project, with
//! a password; converted by LibreOffice (headless, profile of its own)
//! into the formats of other programs (xls, doc, their xlsx/docx) when it
//! is installed; and made by openpyxl with `--python PYTHON`.

use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use apex_io::Format;
use apex_sheet::{CellRef, Range, Sheet, Workbook};

use super::mutate::{self, Seed};

/// Files for seeds under the given paths: known formats, at most
/// `max_size` bytes (small files make fast test cases), and at most
/// `per_reader` of each reader, taken in turns from the folders under
/// each path (the corpora) so that every source is represented.
pub fn collect(paths: &[PathBuf], max_size: u64, per_reader: usize) -> Vec<Seed> {
    // reader guess by extension -> source -> files
    let mut groups: BTreeMap<String, BTreeMap<String, Vec<PathBuf>>> = BTreeMap::new();
    for root in paths {
        let mut files = Vec::new();
        walk(root, &mut files);
        for f in files {
            let Some(ext) = f.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase) else { continue };
            let guess = if ext == "bin" { "vba".to_string() } else if let Some(fmt) = apex_io::Format::from_path(&f) { fmt.extension().to_string() } else { continue };
            if std::fs::metadata(&f).map_or(true, |m| m.len() == 0 || m.len() > max_size) {
                continue;
            }
            let source = f.strip_prefix(root).ok().and_then(|r| r.components().next()).map(|c| format!("{}/{}", root.display(), c.as_os_str().to_string_lossy())).unwrap_or_default();
            groups.entry(guess).or_default().entry(source).or_default().push(f);
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (_, sources) in groups {
        let mut lists: Vec<Vec<PathBuf>> = sources.into_values().collect();
        for l in &mut lists {
            // A fixed shuffle: the same seeds on every run.
            l.sort_by_key(|p| super::fnv(&p.to_string_lossy()));
        }
        let mut taken = 0;
        let mut round = 0;
        while taken < per_reader && lists.iter().any(|l| round < l.len()) {
            for l in &lists {
                if taken >= per_reader {
                    break;
                }
                let Some(f) = l.get(round) else { continue };
                if let Some(s) = mutate::load(f)
                    && seen.insert(apex_io::zip::crc32(&s.data) as u64 ^ ((s.data.len() as u64) << 32))
                {
                    out.push(s);
                    taken += 1;
                }
            }
            round += 1;
        }
    }
    out
}

fn walk(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let path = e.path();
            // The fuzzer's own output is not a seed.
            if path.file_name().is_some_and(|n| n == "crashes" || n == "work" || n == "big") {
                continue;
            }
            walk(&path, out);
        }
    } else if p.is_file() {
        out.push(p.to_path_buf());
    }
}

/// A workbook with the usual things: numbers, text, formulas of several
/// kinds, errors, a second sheet referenced from the first, merged cells.
pub fn workbook() -> Workbook {
    let mut wb = Workbook::new();
    wb.locale = apex_sheet::Locale::EN;
    let rows: &[&[&str]] = &[
        &["Item", "Qty", "Price", "Total", "Note"],
        &["Pears", "3", "1.25", "=B2*C2", "fresh"],
        &["Apples", "10", "0.5", "=B3*C3", "=UPPER(E2)"],
        &["Figs", "-2", "1e3", "=B4*C4", "=IF(B4<0,\"neg\",\"pos\")"],
        &["Date", "2024-02-29", "12:30", "=TODAY()", "50%"],
        &["Sum", "=SUM(B2:B4)", "=AVERAGE(C2:C4)", "=SUM(D2:D4)", "=1/0"],
        &["Look", "=VLOOKUP(\"Apples\",A2:D4,4,FALSE)", "=INDEX(A1:E6,2,2)", "=Data!A1*2", "=CONCAT(A2,\"-\",A3)"],
        &["Text", "ñandú €", "\"quoted\"", "=LEN(B8)", "TRUE"],
    ];
    for (r, row) in rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            wb.set_input(0, CellRef::new(r as u32, c as u32), v);
        }
    }
    wb.sheets.push(Sheet::new("Data"));
    for r in 0..30u32 {
        wb.set_input(1, CellRef::new(r, 0), &(r * 7 % 13).to_string());
        wb.set_input(1, CellRef::new(r, 1), &format!("=A{}+B{}", r + 1, r.max(1)));
    }
    wb.sheets[0].merges.push(Range::new(CellRef::new(9, 0), CellRef::new(10, 2)));
    wb
}

pub const MARKDOWN: &str = "# Title\n\nSome **bold**, *italic*, `code` and a [link](https://example.com).\n\n## Lists\n\n- one\n- two\n  - nested\n1. first\n2. second\n\n> A quote\n> over two lines\n\n| A | B | C |\n|---|:-:|--:|\n| 1 | 2 | 3 |\n| x | ñ | € |\n\n```\nfn main() {}\n```\n\n- [ ] task\n- [x] done\n\nFootnote[^1].\n\n[^1]: The note.\n\n---\n\nLast paragraph with a very long line that goes on and on to make the layout wrap several times over the width of the page, again and again and again.\n";

/// A vbaProject.bin like Office writes: a document module, a standard
/// module and a class (MS-OVBA).
pub fn vba_project() -> Vec<u8> {
    fn rec(out: &mut Vec<u8>, id: u16, data: &[u8]) {
        out.extend(id.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
    }
    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }
    let mut dir = Vec::new();
    rec(&mut dir, 0x0001, &1u32.to_le_bytes());
    rec(&mut dir, 0x0002, &0x409u32.to_le_bytes());
    rec(&mut dir, 0x0014, &0x409u32.to_le_bytes());
    rec(&mut dir, 0x0003, &1252u16.to_le_bytes());
    rec(&mut dir, 0x0004, b"VBAProject");
    rec(&mut dir, 0x0005, b"");
    rec(&mut dir, 0x0040, b"");
    rec(&mut dir, 0x0006, b"");
    rec(&mut dir, 0x003D, b"");
    rec(&mut dir, 0x0007, &[0; 4]);
    rec(&mut dir, 0x0008, &[0; 4]);
    // PROJECTVERSION: its size field says 4, six bytes follow.
    dir.extend(0x0009u16.to_le_bytes());
    dir.extend(4u32.to_le_bytes());
    dir.extend([0x5B, 0x9B, 0x31, 0x1A, 0x06, 0x00]);
    rec(&mut dir, 0x000C, b"");
    rec(&mut dir, 0x003C, b"");
    let modules = [
        ("ThisWorkbook", false, "Attribute VB_Name = \"ThisWorkbook\"\r\nAttribute VB_Base = \"0{00020819-0000-0000-C000-000000000046}\"\r\nPrivate Sub Workbook_Open()\r\n    Range(\"A1\").Value = 1\r\nEnd Sub\r\n"),
        ("Module1", true, "Attribute VB_Name = \"Module1\"\r\nSub Hello()\r\n    MsgBox \"Hola, año\"\r\n    Dim i As Long\r\n    For i = 1 To 10\r\n        Cells(i, 2).Value = i * 2\r\n    Next\r\nEnd Sub\r\n"),
        ("Class1", false, "VERSION 1.0 CLASS\r\nBEGIN\r\n  MultiUse = -1  'True\r\nEND\r\nAttribute VB_Name = \"Class1\"\r\nPublic Value As Long\r\n"),
    ];
    rec(&mut dir, 0x000F, &(modules.len() as u16).to_le_bytes());
    rec(&mut dir, 0x0013, &[0xFF, 0xFF]);
    let mut streams = Vec::new();
    for (name, standard, src) in modules {
        let cache = vec![0xAAu8; 21];
        rec(&mut dir, 0x0019, name.as_bytes());
        rec(&mut dir, 0x0047, &utf16(name));
        rec(&mut dir, 0x001A, name.as_bytes());
        rec(&mut dir, 0x0032, &utf16(name));
        rec(&mut dir, 0x001C, b"");
        rec(&mut dir, 0x0048, b"");
        rec(&mut dir, 0x0031, &(cache.len() as u32).to_le_bytes());
        rec(&mut dir, 0x001E, &[0; 4]);
        rec(&mut dir, 0x002C, &[0xFF, 0xFF]);
        rec(&mut dir, if standard { 0x0021 } else { 0x0022 }, b"");
        rec(&mut dir, 0x002B, b"");
        let mut data = cache;
        data.extend(apex_io::vba::compress(src.as_bytes()));
        streams.push((name, data));
    }
    rec(&mut dir, 0x0010, b"");
    let project = "ID=\"{11111111-2222-3333-4444-555555555555}\"\r\nDocument=ThisWorkbook/&H00000000\r\nModule=Module1\r\nClass=Class1\r\nName=\"VBAProject\"\r\nHelpContextID=\"0\"\r\nCMG=\"0000\"\r\nDPB=\"0000\"\r\nGC=\"0000\"\r\n\r\n[Host Extender Info]\r\n&H00000001={3832D640-CF90-11CF-8E43-00A0C911005A};VBE;&H00000000\r\n";
    let mut c = cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new())).expect("in memory");
    let _ = c.create_storage("/VBA");
    let mut put = |p: &str, d: &[u8]| {
        if let Ok(mut s) = c.create_stream(p) {
            let _ = s.write_all(d);
        }
    };
    put("/VBA/dir", &apex_io::vba::compress(&dir));
    put("/VBA/_VBA_PROJECT", &[0xCC, 0x61, 0xFF, 0xFF, 0x00, 0x00, 0x00]);
    for (name, data) in &streams {
        put(&format!("/VBA/{name}"), data);
    }
    put("/PROJECT", project.as_bytes());
    let _ = c.flush();
    c.into_inner().into_inner()
}

/// Writes the seeds made by ApexDesk itself into `dir`.
pub fn make(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let own = dir.join("apex");
    std::fs::create_dir_all(&own).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let e = |e: apex_io::Error| e.to_string();
    let wb = workbook();
    for fmt in [Format::Xlsx, Format::Ods, Format::Csv, Format::Tsv] {
        let p = own.join(format!("book.{}", fmt.extension()));
        apex_io::save_sheet(&wb, &p, fmt).map_err(e)?;
        out.push(p);
    }
    let mut small = Workbook::new();
    small.set_input(0, CellRef::new(0, 0), "=1+1");
    let p = own.join("tiny.xlsx");
    apex_io::save_sheet(&small, &p, Format::Xlsx).map_err(e)?;
    out.push(p);
    let vba = vba_project();
    std::fs::write(own.join("vbaProject.bin"), &vba).map_err(|e| e.to_string())?;
    out.push(own.join("vbaProject.bin"));
    let mut macro_wb = workbook();
    macro_wb.macros.project = Some(Arc::new(vba.clone()));
    let p = own.join("macros.xlsm");
    apex_io::save_sheet(&macro_wb, &p, Format::Xlsx).map_err(e)?;
    out.push(p);
    let doc = apex_io::markdown::read(MARKDOWN);
    for fmt in [Format::Docx, Format::Odt, Format::Rtf, Format::Markdown, Format::Text] {
        let p = own.join(format!("doc.{}", fmt.extension()));
        apex_io::save_doc(&doc, &p, fmt).map_err(e)?;
        out.push(p);
    }
    let mut macro_doc = doc.clone();
    macro_doc.macros.project = Some(Arc::new(vba));
    let p = own.join("macros.docm");
    apex_io::save_doc(&macro_doc, &p, Format::Docx).map_err(e)?;
    out.push(p);
    // Encrypted: the password is the one the child tries.
    for fmt in [Format::Xlsx, Format::Ods] {
        let p = own.join(format!("secret-book.{}", fmt.extension()));
        apex_io::save_sheet_with(&wb, &p, fmt, Some(super::child::PASSWORD)).map_err(e)?;
        out.push(p);
    }
    for fmt in [Format::Docx, Format::Odt] {
        let p = own.join(format!("secret-doc.{}", fmt.extension()));
        apex_io::save_doc_with(&doc, &p, fmt, Some(super::child::PASSWORD)).map_err(e)?;
        out.push(p);
    }
    // Text files of other shapes.
    let texts: &[(&str, &[u8])] = &[
        ("semicolon.csv", "a;b;c\n1,5;2;\"x;y\"\n\"multi\nline\";;=A1\n".as_bytes()),
        ("quotes.csv", b"\"a\"\"b\",c\r\n,,\r\n\"unclosed,1\r\n"),
        ("latin1.csv", b"nombre,ciudad\nJos\xe9,M\xe1laga\n"),
        ("plain.txt", "Line one\nLine two\ttab\n\nñ € 日本語\n".as_bytes()),
        ("table.rtf", b"{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Arial;}}{\\colortbl;\\red255\\green0\\blue0;}\\f0\\fs24 Hello {\\b bold} \\cf1 red\\cf0\\par\\trowd\\cellx2000\\cellx4000 a\\cell b\\cell\\row\\pard \\u241?and\\'fa\\par}"),
    ];
    for (name, data) in texts {
        std::fs::write(own.join(name), data).map_err(|e| e.to_string())?;
        out.push(own.join(name));
    }
    let mut utf16 = vec![0xFF, 0xFE];
    utf16.extend("a,b\n1,ñ\n".encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(own.join("utf16.csv"), utf16).map_err(|e| e.to_string())?;
    out.push(own.join("utf16.csv"));
    Ok(out)
}

/// The VBA projects of macro files under `corpus` (xlsm, docm and the
/// like), as vbaProject.bin files: seeds of the VBA reader.
pub fn vba_from(dir: &Path, corpus: &Path, max: usize) -> Vec<PathBuf> {
    let out = dir.join("vba");
    let _ = std::fs::create_dir_all(&out);
    let mut files = Vec::new();
    walk(corpus, &mut files);
    files.retain(|f| f.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "xlsm" | "docm" | "xltm" | "dotm")));
    files.sort_by_key(|p| super::fnv(&p.to_string_lossy()));
    let mut made = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for f in files {
        if made.len() >= max {
            break;
        }
        let Ok(data) = std::fs::read(&f) else { continue };
        let Some(parts) = std::panic::catch_unwind(|| super::zip::parts(&data)).ok().flatten() else { continue };
        for p in parts.iter().filter(|p| p.name.to_ascii_lowercase().ends_with("vbaproject.bin")) {
            if p.data.len() < 512 << 10 && seen.insert(apex_io::zip::crc32(&p.data)) {
                let path = out.join(format!("vbaProject-{:03}.bin", made.len()));
                if std::fs::write(&path, &p.data).is_ok() {
                    made.push(path);
                }
            }
        }
    }
    made
}

/// Converts some files with LibreOffice into the formats of other
/// programs (its own xlsx and docx, and the binary xls and doc).
pub fn libreoffice(dir: &Path, sources: &[PathBuf]) -> Vec<PathBuf> {
    let out_dir = absolute(&dir.join("libreoffice"));
    let profile = absolute(&dir.join("lo-profile"));
    let _ = std::fs::create_dir_all(&out_dir);
    // Sources by extension: one conversion per extension and target.
    let mut by_ext: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for src in sources {
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        by_ext.entry(ext).or_default().push(absolute(src));
    }
    let mut made = Vec::new();
    for (ext, files) in by_ext {
        let targets: &[&str] = match ext.as_str() {
            "xlsx" | "xlsm" | "ods" | "csv" => &["xls", "xlsx", "ods"],
            "docx" | "docm" | "odt" | "rtf" | "doc" => &["doc", "docx", "odt", "rtf"],
            _ => &[],
        };
        for t in targets.iter().filter(|t| **t != ext) {
            let sub = out_dir.join(format!("{ext}-to-{t}"));
            let _ = std::fs::create_dir_all(&sub);
            let mut args: Vec<std::ffi::OsString> = vec!["--convert-to".into(), (*t).into(), "--outdir".into(), sub.clone().into()];
            args.extend(files.iter().map(|f| f.clone().into_os_string()));
            if soffice(&profile, &args, std::time::Duration::from_secs(900)).is_some() {
                for f in &files {
                    let p = sub.join(f.file_stem().unwrap_or_default()).with_extension(t);
                    if p.exists() {
                        made.push(p);
                    }
                }
            }
        }
    }
    made
}

pub fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf()) }
}

/// Runs LibreOffice without a window and with a profile of its own;
/// returns the time it took and its peak memory in KiB (0 where the
/// system doesn't say), or None if it failed or took longer than `limit`.
pub fn soffice(profile: &Path, args: &[std::ffi::OsString], limit: std::time::Duration) -> Option<(f64, u64)> {
    let _ = std::fs::create_dir_all(profile);
    let url = format!("file://{}", absolute(profile).display());
    let start = std::time::Instant::now();
    let mut cmd = Command::new("soffice");
    super::exec::limit_memory(&mut cmd, 10 << 30);
    let mut child = cmd
        .arg(format!("-env:UserInstallation={url}"))
        .args(["--headless", "--norestore", "--nologo", "--nodefault"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let (ok, rss) = super::exec::wait_limited(&mut child, limit);
    ok.then(|| (start.elapsed().as_secs_f64(), rss))
}

/// Workbooks made by openpyxl: another writer's xlsx, with the parts
/// ApexDesk doesn't write itself (validation, conditional formats,
/// comments, defined names, tables, charts, images).
pub const OPENPYXL: &str = r##"
import sys, os
from openpyxl import Workbook
from openpyxl.chart import BarChart, LineChart, PieChart, Reference
from openpyxl.comments import Comment
from openpyxl.formatting.rule import CellIsRule, ColorScaleRule, DataBarRule
from openpyxl.styles import Font, PatternFill, Alignment, Border, Side
from openpyxl.worksheet.datavalidation import DataValidation
from openpyxl.worksheet.table import Table, TableStyleInfo
from openpyxl.workbook.defined_name import DefinedName
out = sys.argv[1]
os.makedirs(out, exist_ok=True)
for variant in range(6):
    wb = Workbook()
    ws = wb.active
    ws.title = "Main"
    ws.append(["Name", "Qty", "Price", "Total", "When"])
    for i in range(2, 20 + variant * 10):
        ws.append([f"item{i}", i, i * 1.5, f"=B{i}*C{i}", f"=DATE(2024,1,{i % 28 + 1})"])
    ws["G1"] = "=SUM(D:D)"
    ws["G2"] = "=SUMPRODUCT(B2:B10,C2:C10)"
    ws["G3"] = '=IFERROR(VLOOKUP("item5",A:D,4,FALSE),"none")'
    ws["A1"].font = Font(bold=True, color="FF0000", size=14)
    ws["B2"].fill = PatternFill("solid", fgColor="FFFF00")
    ws["C3"].alignment = Alignment(horizontal="center", wrap_text=True, text_rotation=45)
    ws["D4"].border = Border(left=Side(style="thick"), bottom=Side(style="dashDot"))
    ws["B3"].number_format = "#,##0.00;[Red]-#,##0.00"
    ws.merge_cells("H5:J7")
    ws["A2"].comment = Comment("A note", "fuzz")
    dv = DataValidation(type="list", formula1='"a,b,c"', allow_blank=True)
    ws.add_data_validation(dv)
    dv.add("E2:E30")
    ws.conditional_formatting.add("B2:B30", CellIsRule(operator="greaterThan", formula=["10"], fill=PatternFill("solid", fgColor="00FF00")))
    ws.conditional_formatting.add("C2:C30", ColorScaleRule(start_type="min", start_color="FF0000", end_type="max", end_color="00FF00"))
    ws.conditional_formatting.add("D2:D30", DataBarRule(start_type="min", end_type="max", color="638EC6"))
    ws.freeze_panes = "B2"
    ws.auto_filter.ref = "A1:E10"
    wb.defined_names["Prices"] = DefinedName("Prices", attr_text="Main!$C$2:$C$10")
    t = wb.create_sheet("Table")
    t.append(["k", "v"])
    for i in range(10):
        t.append([f"k{i}", i * i])
    tab = Table(displayName=f"T{variant}", ref="A1:B11")
    tab.tableStyleInfo = TableStyleInfo(name="TableStyleMedium9", showRowStripes=True)
    t.add_table(tab)
    chart = [BarChart, LineChart, PieChart][variant % 3]()
    chart.add_data(Reference(ws, min_col=2, min_row=1, max_row=10), titles_from_data=True)
    chart.set_categories(Reference(ws, min_col=1, min_row=2, max_row=10))
    ws.add_chart(chart, "L2")
    if variant % 2:
        ws.row_dimensions[3].height = 40
        ws.column_dimensions["A"].width = 30
        ws.sheet_properties.tabColor = "1072BA"
        ws.print_title_rows = "1:1"
        ws.page_setup.orientation = "landscape"
        ws.protection.sheet = True
    wb.save(os.path.join(out, f"openpyxl{variant}.xlsx"))
"##;

pub fn openpyxl(dir: &Path, python: &str) -> Vec<PathBuf> {
    let out = dir.join("openpyxl");
    let ok = Command::new(python).arg("-c").arg(OPENPYXL).arg(&out).stdout(Stdio::null()).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("apex-bench: openpyxl seeds not made ({python} with openpyxl is needed)");
        return Vec::new();
    }
    let mut v = Vec::new();
    walk(&out, &mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_seeds_open() {
        let dir = std::env::temp_dir().join(format!("apex-bench-seeds-{}", std::process::id()));
        let made = make(&dir).unwrap();
        assert!(made.len() > 15);
        let seeds = collect(std::slice::from_ref(&dir), 1 << 20, 100);
        let readers: std::collections::HashSet<&str> = seeds.iter().map(|s| s.reader.as_str()).collect();
        for r in ["xlsx", "ods", "csv", "docx", "odt", "rtf", "md", "txt", "vba", "crypto"] {
            assert!(readers.contains(r), "{r}");
        }
        let vba = apex_io::vba::read(&vba_project()).unwrap();
        assert_eq!(vba.modules.len(), 3);
        for s in &seeds {
            if s.reader != "vba" && s.reader != "crypto" {
                assert!(apex_io::open(&s.path).is_ok(), "{}", s.path.display());
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
