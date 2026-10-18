//! `--external`: what LibreOffice and OnlyOffice see. Each converts the
//! original and every file we saved to PDF; poppler's pdfinfo and
//! pdftotext give its pages and words, compared with the original's as
//! that same program sees it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::child;
use super::result::External;

const TIMEOUT: Duration = Duration::from_secs(120);
const MEM_MB: u64 = 4096;

/// Where the tools are and where they may write.
pub struct Tools {
    /// Private LibreOffice profiles and scratch files (one per worker).
    pub work: PathBuf,
    /// OnlyOffice's font list (AllFonts.js), made by its desktop editors.
    pub onlyoffice_fonts: Option<PathBuf>,
}

impl Tools {
    /// OnlyOffice needs its font list: target/tools/onlyoffice/fonts, or
    /// the desktop editors' own.
    pub fn find_fonts(tools_dir: &Path) -> Option<PathBuf> {
        let own = tools_dir.join("onlyoffice/fonts");
        if own.join("AllFonts.js").exists() {
            return Some(own);
        }
        let home = std::env::var_os("HOME")?;
        let p = Path::new(&home).join(".var/app/org.onlyoffice.desktopeditors/data/onlyoffice/desktopeditors/data/fonts");
        p.join("AllFonts.js").exists().then_some(p)
    }
}

/// Pages and words of a PDF.
pub(super) fn read_pdf(pdf: &Path) -> Result<(u64, Vec<String>), String> {
    let info = Command::new("pdfinfo").arg(pdf).output().map_err(|e| format!("pdfinfo: {e}"))?;
    let info = String::from_utf8_lossy(&info.stdout);
    let pages = info.lines().find_map(|l| l.strip_prefix("Pages:")).and_then(|v| v.trim().parse().ok()).ok_or("no page count")?;
    let text = Command::new("pdftotext").arg(pdf).arg("-").output().map_err(|e| format!("pdftotext: {e}"))?;
    let words = String::from_utf8_lossy(&text.stdout).split_whitespace().map(str::to_lowercase).collect();
    Ok((pages, words))
}

/// Words in common over the longer text (1: the same words).
pub fn similarity(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut count: HashMap<&str, i64> = HashMap::new();
    for w in a {
        *count.entry(w).or_default() += 1;
    }
    let mut common = 0;
    for w in b {
        if let Some(n) = count.get_mut(w.as_str())
            && *n > 0
        {
            *n -= 1;
            common += 1;
        }
    }
    common as f64 / a.len().max(b.len()) as f64
}

fn libreoffice(input: &Path, out_dir: &Path, profile: &Path) -> Result<PathBuf, String> {
    let mut cmd = Command::new(if cfg!(windows) { "soffice.exe" } else { "soffice" });
    cmd.arg(format!("-env:UserInstallation=file://{}", profile.display()))
        .args(["--headless", "--norestore", "--convert-to", "pdf", "--outdir"])
        .arg(out_dir)
        .arg(input);
    let exit = child::run(cmd, TIMEOUT, MEM_MB).map_err(|e| format!("soffice: {e}"))?;
    if exit.killed.is_some() {
        return Err(format!("{:?}", exit.killed.unwrap()).to_lowercase());
    }
    let pdf = out_dir.join(input.file_stem().unwrap_or_default()).with_extension("pdf");
    if pdf.exists() { Ok(pdf) } else { Err(format!("no PDF: {}", exit.stderr_tail())) }
}

/// OnlyOffice's converter, x2t, runs inside its flatpak with an XML task.
fn onlyoffice(input: &Path, out_dir: &Path, fonts: &Path) -> Result<PathBuf, String> {
    let pdf = out_dir.join(input.file_stem().unwrap_or_default()).with_extension("pdf");
    let xml = out_dir.join("x2t.xml");
    let esc = |p: &Path| p.display().to_string().replace('&', "&amp;").replace('<', "&lt;");
    let task = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<TaskQueueDataConvert>\n<m_sFileFrom>{}</m_sFileFrom>\n<m_sFileTo>{}</m_sFileTo>\n<m_nFormatTo>513</m_nFormatTo>\n<m_sAllFontsPath>{}</m_sAllFontsPath>\n<m_sFontDir>{}</m_sFontDir>\n<m_bIsNoBase64>true</m_bIsNoBase64>\n</TaskQueueDataConvert>\n",
        esc(input),
        esc(&pdf),
        esc(&fonts.join("AllFonts.js")),
        esc(fonts)
    );
    std::fs::write(&xml, task).map_err(|e| e.to_string())?;
    let mut cmd = Command::new("flatpak");
    cmd.arg("run");
    for dir in [input.parent(), Some(out_dir), Some(fonts)].into_iter().flatten() {
        cmd.arg(format!("--filesystem={}", dir.display()));
    }
    cmd.args(["--command=/app/bin/opt/onlyoffice/desktopeditors/converter/x2t", "org.onlyoffice.desktopeditors"]).arg(&xml);
    let exit = child::run(cmd, TIMEOUT, MEM_MB).map_err(|e| format!("x2t: {e}"))?;
    if exit.killed.is_some() {
        return Err(format!("{:?}", exit.killed.unwrap()).to_lowercase());
    }
    if pdf.exists() {
        Ok(pdf)
    } else {
        Err(format!("x2t failed ({}): {}", exit.status.and_then(|s| s.code()).unwrap_or(-1), exit.stderr_tail()))
    }
}

/// The original and the saved files (target extension, path) through both
/// programs. `slot` picks a private LibreOffice profile.
pub fn check(tools: &Tools, slot: usize, original: &Path, saved: &[(String, PathBuf)]) -> Vec<External> {
    let mut out = Vec::new();
    let work = tools.work.join(format!("w{slot}"));
    let profile = tools.work.join(format!("lo{slot}"));
    let mut words: HashMap<(&str, String), Vec<String>> = HashMap::new();
    let files: Vec<(String, &Path)> = std::iter::once(("orig".to_string(), original)).chain(saved.iter().map(|(e, p)| (e.clone(), p.as_path()))).collect();
    for tool in ["libreoffice", "onlyoffice"] {
        if tool == "onlyoffice" && tools.onlyoffice_fonts.is_none() {
            continue;
        }
        for (name, path) in &files {
            let _ = std::fs::remove_dir_all(&work);
            let _ = std::fs::create_dir_all(&work);
            // Copy in first: the name is ours and the flatpak sees it.
            let input = work.join(format!("in-{name}.{}", path.extension().and_then(|e| e.to_str()).unwrap_or("bin")));
            let pdf = std::fs::copy(path, &input).map_err(|e| e.to_string()).and_then(|_| match tool {
                "libreoffice" => libreoffice(&input, &work, &profile),
                _ => onlyoffice(&input, &work, tools.onlyoffice_fonts.as_deref().expect("checked")),
            });
            let mut x = External { tool: tool.into(), file: name.clone(), ..Default::default() };
            match pdf.and_then(|p| read_pdf(&p)) {
                Ok((pages, w)) => {
                    x.pages = Some(pages);
                    if let Some(orig) = words.get(&(tool, "orig".to_string())) {
                        x.similarity = Some(similarity(orig, &w));
                    }
                    words.insert((tool, name.clone()), w);
                }
                Err(e) => x.error = Some(e),
            }
            out.push(x);
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn similarity_of_words() {
        assert_eq!(similarity(&words("a b c d"), &words("a b c d")), 1.0);
        assert_eq!(similarity(&words("a b c d"), &words("a b")), 0.5);
        assert_eq!(similarity(&words("a a"), &words("a b")), 0.5);
    }
}
