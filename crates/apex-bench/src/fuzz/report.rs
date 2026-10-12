//! The files of the fuzzer's output folder: one trace file per distinct
//! failure (crashes/NAME.txt, next to the reduced file), the runs
//! (runs.tsv), the results of `big` (big/results.tsv), and report.md made
//! from them all (with notes.md, written by hand, at the top).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub struct CrashInfo {
    pub reader: String,
    pub kind: String,
    pub stage: String,
    pub signature: String,
    pub message: String,
    pub seed: String,
    pub file: String,
    pub original: usize,
    pub minimized: usize,
    pub hits: usize,
    pub trace: String,
}

const TRACE: &str = "--- trace";
/// The trace file of a kept failure: NAME.trace.txt next to NAME.ext.
pub const TRACE_EXT: &str = ".trace.txt";

impl CrashInfo {
    pub fn to_text(&self) -> String {
        format!(
            "reader: {}\nkind: {}\nstage: {}\nsignature: {}\nmessage: {}\nseed: {}\nfile: {}\noriginal: {}\nminimized: {}\nhits: {}\n{TRACE}\n{}\n",
            self.reader,
            self.kind,
            self.stage,
            self.signature,
            self.message.replace('\n', " "),
            self.seed,
            self.file,
            self.original,
            self.minimized,
            self.hits,
            self.trace
        )
    }

    pub fn parse(text: &str) -> Option<CrashInfo> {
        let (head, trace) = text.split_once(&format!("{TRACE}\n")).unwrap_or((text, ""));
        let fields: HashMap<&str, &str> = head.lines().filter_map(|l| l.split_once(": ")).collect();
        let get = |k: &str| fields.get(k).map(|s| s.to_string()).unwrap_or_default();
        let num = |k: &str| fields.get(k).and_then(|s| s.parse().ok()).unwrap_or(0);
        Some(CrashInfo {
            reader: fields.get("reader")?.to_string(),
            kind: get("kind"),
            stage: get("stage"),
            signature: get("signature"),
            message: get("message"),
            seed: get("seed"),
            file: get("file"),
            original: num("original"),
            minimized: num("minimized"),
            hits: num("hits"),
            trace: trace.to_string(),
        })
    }

    pub fn key(&self) -> String {
        format!("{}|{}|{}", self.reader, self.kind, self.signature)
    }
}

fn all(crashes: &Path) -> Vec<(PathBuf, CrashInfo)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(crashes) {
        for e in rd.flatten() {
            let p = e.path();
            if p.to_string_lossy().ends_with(TRACE_EXT)
                && let Ok(t) = std::fs::read_to_string(&p)
                && let Some(c) = CrashInfo::parse(&t)
            {
                out.push((p, c));
            }
        }
    }
    out.sort_by(|a, b| (&a.1.reader, &a.1.kind, &a.1.signature).cmp(&(&b.1.reader, &b.1.kind, &b.1.signature)));
    out
}

/// The failures already kept: key -> (trace file, hits).
pub fn known(crashes: &Path) -> HashMap<String, (PathBuf, usize)> {
    all(crashes).into_iter().map(|(p, c)| (c.key(), (p, c.hits))).collect()
}

pub fn add_hits(txt: &Path, hits: usize) {
    if let Ok(t) = std::fs::read_to_string(txt)
        && let Some(mut c) = CrashInfo::parse(&t)
    {
        c.hits = hits;
        let _ = std::fs::write(txt, c.to_text());
    }
}

pub struct RunRow {
    pub reader: String,
    pub secs: f64,
    pub tests: usize,
    pub ok: usize,
    pub rejected: usize,
    pub failures: String,
    pub wall: f64,
    pub jobs: usize,
}

pub fn append_runs(out: &Path, rows: &[RunRow]) -> Result<(), String> {
    use std::io::Write;
    let path = out.join("runs.tsv");
    let new = !path.exists();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| e.to_string())?;
    let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if new {
        writeln!(f, "when\treader\ttests\tok\trejected\tfailures\tchild_secs\twall_secs\tjobs").map_err(|e| e.to_string())?;
    }
    for r in rows {
        writeln!(f, "{when}\t{}\t{}\t{}\t{}\t{}\t{:.0}\t{:.0}\t{}", r.reader, r.tests, r.ok, r.rejected, r.failures, r.secs, r.wall, r.jobs).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// Writes OUT/report.md.
pub fn write(out: &Path) -> Result<(), String> {
    let mut md = String::new();
    md.push_str("# Robustez de los lectores: fuzzing y ficheros grandes\n\n");
    if let Ok(notes) = std::fs::read_to_string(out.join("notes.md")) {
        md.push_str(&notes);
        md.push_str("\n\n");
    }
    // Runs, summed by reader.
    let mut totals: BTreeMap<String, (usize, usize, usize, f64, BTreeMap<String, usize>)> = BTreeMap::new();
    if let Ok(t) = std::fs::read_to_string(out.join("runs.tsv")) {
        for l in t.lines().skip(1) {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 8 {
                continue;
            }
            let e = totals.entry(f[1].to_string()).or_default();
            e.0 += f[2].parse::<usize>().unwrap_or(0);
            e.1 += f[3].parse::<usize>().unwrap_or(0);
            e.2 += f[4].parse::<usize>().unwrap_or(0);
            e.3 += f[6].parse::<f64>().unwrap_or(0.0);
            for kv in f[5].split_whitespace() {
                if let Some((k, v)) = kv.split_once('=') {
                    *e.4.entry(k.to_string()).or_default() += v.parse::<usize>().unwrap_or(0);
                }
            }
        }
    }
    let crashes = all(&out.join("crashes"));
    md.push_str("## Fuzzing: pruebas por lector\n\n");
    md.push_str("Cada prueba abre el fichero mutado en un proceso hijo (10 s y 1,5 GB de montón como límite; la primera sesión, 2 GB), recalcula (hojas), guarda en todos los formatos que escribe ApexDesk y vuelve a abrir lo guardado. «rechazado» = el lector dio un error limpio.\n\n");
    md.push_str("| lector | pruebas | abre bien | rechazado | fallos (veces) | fallos distintos | horas de CPU en hijos |\n|---|--:|--:|--:|---|--:|--:|\n");
    for (r, (tests, ok, rej, secs, fails)) in &totals {
        let distinct = crashes.iter().filter(|(_, c)| &c.reader == r).count();
        let f: Vec<String> = fails.iter().map(|(k, v)| format!("{k} {v}")).collect();
        let _ = writeln!(md, "| {r} | {tests} | {ok} | {rej} | {} | {distinct} | {:.2} |", f.join(", "), secs / 3600.0);
    }
    md.push_str("\n## Fallos distintos por lector\n\nFicheros reducidos en `crashes/` (el `.trace.txt` del mismo nombre trae la traza). Firma = primer punto del código de ApexDesk en la traza (o la función donde estaba colgado).\n\n");
    // Hangs run again alone on a quiet machine (hang-replay.tsv: name, result).
    let replay: HashMap<String, String> = std::fs::read_to_string(out.join("hang-replay.tsv"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(n, r)| (n.to_string(), r.split(" at ").next().unwrap_or("").to_string()))
        .collect();
    if !replay.is_empty() {
        md.push_str("Columna «otra vez»: resultado de repetir el cuelgue a solas con la máquina descargada (`hang` = se repite; `ok` = solo era lento con la máquina cargada).\n\n");
    }
    let mut by_reader: BTreeMap<&str, Vec<&CrashInfo>> = BTreeMap::new();
    for (_, c) in &crashes {
        by_reader.entry(&c.reader).or_default().push(c);
    }
    if by_reader.is_empty() {
        md.push_str("Ninguno.\n");
    }
    for (r, list) in by_reader {
        let _ = writeln!(md, "### {r} ({} distintos)\n", list.len());
        md.push_str("| tipo | fase | firma | mensaje | fichero mínimo | bytes | veces | otra vez |\n|---|---|---|---|---|--:|--:|---|\n");
        for c in list {
            let msg: String = c.message.chars().take(120).collect();
            let stem = c.file.rsplit_once('.').map(|x| x.0).unwrap_or(&c.file);
            let again = replay.get(stem).map(String::as_str).unwrap_or("");
            let _ = writeln!(md, "| {} | {} | `{}` | {} | [{}](crashes/{}) | {} | {} | {again} |", c.kind, c.stage, md_cell(&c.signature), md_cell(&msg), c.file, c.file, c.minimized, c.hits);
        }
        md.push('\n');
    }
    md.push_str("## Ficheros grandes\n\n");
    match std::fs::read_to_string(out.join("big").join("results.md")) {
        Ok(t) => md.push_str(&t),
        Err(_) => md.push_str("Sin resultados todavía (`apex-bench big`).\n"),
    }
    std::fs::write(out.join("report.md"), md).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_info_roundtrip() {
        let c = CrashInfo {
            reader: "xlsx".into(),
            kind: "panic".into(),
            stage: "open".into(),
            signature: "crates/apex-io/src/x.rs:1".into(),
            message: "boom\nline".into(),
            seed: "s.xlsx".into(),
            file: "xlsx-panic-1.xlsx".into(),
            original: 100,
            minimized: 10,
            hits: 3,
            trace: "a: b\nc".into(),
        };
        let p = CrashInfo::parse(&c.to_text()).unwrap();
        assert_eq!((p.key(), p.message.as_str(), p.hits, p.trace.as_str()), (c.key(), "boom line", 3, "a: b\nc\n"));
    }
}
