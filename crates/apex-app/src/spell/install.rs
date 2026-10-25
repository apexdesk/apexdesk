//! Installing a dictionary the user downloaded: a LibreOffice extension
//! (.oxt) or a zip with the dictionaries in it, or the .aff and .dic
//! themselves. They're copied to the user's folder of dictionaries
//! (searched on every system, Android too), named by their language, and
//! found without restarting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use apex_fs::PathExt;

use crate::i18n::{tr, trf};

/// Where to get them: LibreOffice's dictionary extensions.
pub const DOWNLOAD_URL: &str = "https://extensions.libreoffice.org/?Tags%5B%5D=50";

/// The user's folder of dictionaries (in the portable folder when the
/// program is portable).
pub fn user_dir() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("dictionaries"))
}

/// Asks for the downloaded files and installs them.
pub fn pick() {
    crate::file_dialog::FileDialog::new()
        .set_title(tr("spell.install"))
        .add_filter(tr("spell.dictionaries"), &["oxt", "zip", "aff", "dic"])
        .pick_files(|app, files| match user_dir().ok_or_else(|| tr("spell.install_none").to_string()).and_then(|dir| install_into(&dir, &files)) {
            Ok(langs) => {
                super::installed_now(&langs);
                let names: Vec<String> = langs.iter().map(|l| super::language_name(l)).collect();
                app.notify(crate::app::Notice::info(trf("spell.installed", &[("langs", &names.join(", "))])));
            }
            Err(e) => app.notify(crate::app::Notice::error(e)),
        });
}

/// A file to write in the folder: its name and its bytes.
struct Out {
    name: String,
    data: Vec<u8>,
}

/// Installs the dictionaries `files` hold in `dir`; returns their
/// languages ("es_ES").
pub(super) fn install_into(dir: &Path, files: &[PathBuf]) -> Result<Vec<String>, String> {
    let mut out: Vec<Out> = Vec::new();
    let mut pairs = Vec::new();
    for f in files {
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "aff" | "dic" => {
                // Both halves: chosen together or side by side.
                let base = f.with_extension("");
                if pairs.contains(&base) {
                    continue;
                }
                pairs.push(base.clone());
                let half = |e: &str| files.iter().find(|g| g.with_extension("") == base && g.extension().is_some_and(|x| x.eq_ignore_ascii_case(e))).cloned().unwrap_or_else(|| base.with_extension(e));
                let (aff, dic) = (half("aff"), half("dic"));
                let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if !aff.fs_is_file() || !dic.fs_is_file() {
                    return Err(trf("spell.install_pair", &[("name", &name)]));
                }
                let stem = base.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let folder = base.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let Some(lang) = code_of(&stem).or_else(|| code_of(&folder)) else {
                    return Err(trf("spell.install_lang", &[("name", &name)]));
                };
                let read = |p: &Path| apex_fs::read(p).map_err(|e| trf("spell.install_failed", &[("err", &e)]));
                out.push(Out { name: format!("{lang}.aff"), data: read(&aff)? });
                out.push(Out { name: format!("{lang}.dic"), data: read(&dic)? });
            }
            _ => out.extend(from_archive(f)?),
        }
    }
    let langs: Vec<String> = out.iter().filter_map(|o| o.name.strip_suffix(".aff")).map(str::to_string).collect();
    if langs.is_empty() {
        return Err(tr("spell.install_none").to_string());
    }
    let failed = |e: std::io::Error| trf("spell.install_failed", &[("err", &e)]);
    apex_fs::create_dir_all(dir).map_err(failed)?;
    for o in &out {
        // Written beside it and moved in: a dictionary half written is
        // never read.
        let tmp = dir.join(format!("{}.part", o.name));
        apex_fs::write(&tmp, &o.data).map_err(failed)?;
        apex_fs::rename(&tmp, dir.join(&o.name)).map_err(failed)?;
    }
    Ok(langs)
}

/// The dictionaries (and thesauri) of a LibreOffice extension or a zip.
fn from_archive(path: &Path) -> Result<Vec<Out>, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let data = apex_fs::read(path).map_err(|e| trf("spell.install_failed", &[("err", &e)]))?;
    let zip = apex_io::zip::ZipReader::new(&data).map_err(|_| trf("spell.install_not_zip", &[("name", &name)]))?;
    let entries: Vec<String> = zip.names().map(str::to_string).collect();
    // What the extension says each .aff is for.
    let mut declared: HashMap<String, Vec<String>> = HashMap::new();
    for e in entries.iter().filter(|e| e.to_ascii_lowercase().ends_with(".xcu")) {
        if let Ok(xcu) = zip.read_string(e) {
            declared.extend(declared_locales(&xcu));
        }
    }
    let archive = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out: Vec<Out> = Vec::new();
    for e in &entries {
        let (folder, file) = e.rsplit_once('/').unwrap_or(("", e));
        let lower = file.to_ascii_lowercase();
        let Some(stem) = lower.strip_suffix(".aff").map(|_| &file[..file.len() - 4]) else {
            // A thesaurus goes along under its own name (th_es_ES_v2.dat),
            // its two halves together.
            let other = if lower.ends_with(".dat") { "idx" } else { "dat" };
            let both = entries.iter().any(|x| x.len() == e.len() && x.get(..e.len() - 3) == e.get(..e.len() - 3) && x.to_ascii_lowercase().ends_with(other));
            if lower.starts_with("th_") && (lower.ends_with(".dat") || lower.ends_with(".idx")) && both && file.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)) {
                out.push(Out { name: file.to_string(), data: zip.read(e).map_err(|_| trf("spell.install_not_zip", &[("name", &name)]))? });
            }
            continue;
        };
        if lower.starts_with("hyph_") || lower.starts_with("th_") {
            continue;
        }
        let Some(dic) = entries.iter().find(|d| d.len() == e.len() && d.get(..e.len() - 4) == e.get(..e.len() - 4) && d.to_ascii_lowercase().ends_with(".dic")) else { continue };
        let folder = folder.rsplit('/').next().unwrap_or("");
        let from_names = code_of(stem).or_else(|| code_of(folder)).or_else(|| code_of(&archive)).or_else(|| archive.strip_prefix("dict-").and_then(code_of));
        let lang = match declared.get(file) {
            Some(locales) => pick_locale(locales, from_names.as_deref()).or(from_names),
            None => from_names,
        };
        let Some(lang) = lang else { continue };
        if out.iter().any(|o| o.name == format!("{lang}.aff")) {
            continue;
        }
        let read = |n: &str| zip.read(n).map_err(|_| trf("spell.install_not_zip", &[("name", &name)]));
        out.push(Out { name: format!("{lang}.aff"), data: read(e)? });
        out.push(Out { name: format!("{lang}.dic"), data: read(dic)? });
    }
    Ok(out)
}

/// The spelling dictionaries of an extension's dictionaries.xcu: .aff file
/// name -> the locales it's for ("es-ES").
fn declared_locales(xcu: &str) -> Vec<(String, Vec<String>)> {
    let value = |node: &str, prop: &str| -> Option<String> {
        let at = node.find(&format!("oor:name=\"{prop}\""))?;
        let rest = &node[at..];
        let start = rest.find("<value>")? + "<value>".len();
        let end = rest[start..].find("</value>")?;
        Some(rest[start..start + end].to_string())
    };
    xcu.split("<node")
        .filter(|node| node.contains("DICT_SPELL"))
        .filter_map(|node| {
            let aff = value(node, "Locations")?.split_whitespace().find(|l| l.to_ascii_lowercase().ends_with(".aff"))?.rsplit('/').next()?.to_string();
            let locales = value(node, "Locales")?.split_whitespace().map(str::to_string).collect();
            Some((aff, locales))
        })
        .collect()
}

/// The locale to name a dictionary for: the one its file names, else the
/// language's main one (es-ES among es-AR, es-ES...), else the first.
fn pick_locale(locales: &[String], named: Option<&str>) -> Option<String> {
    let codes: Vec<String> = locales.iter().filter_map(|l| code_of(l)).collect();
    let main = |c: &&String| c.split_once('_').is_some_and(|(l, r)| l.eq_ignore_ascii_case(r));
    codes.iter().find(|c| Some(c.as_str()) == named).or_else(|| codes.iter().find(main)).or_else(|| codes.first()).cloned()
}

/// The language code a name starts with: "es_ES" from "de_DE_frami",
/// "en-US", "en_US-large"; "es" from "es" or "es_ANY"; none from "index".
fn code_of(name: &str) -> Option<String> {
    let name = name.replace('-', "_");
    let mut parts = name.split('_');
    let lang = parts.next()?;
    if !(2..=3).contains(&lang.len()) || !lang.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let lang = lang.to_ascii_lowercase();
    match parts.next() {
        Some(r) if r.len() == 2 && r.bytes().all(|b| b.is_ascii_alphabetic()) => Some(format!("{lang}_{}", r.to_ascii_uppercase())),
        _ => Some(lang),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = apex_fs::temp_dir().join(format!("apex-dict-{name}-{}", apex_fs::process_id()));
        let _ = apex_fs::remove_dir_all(&d);
        apex_fs::create_dir_all(&d).expect("mkdir");
        d
    }

    const XCU: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:component-data xmlns:oor="http://openoffice.org/2001/registry" oor:name="Linguistic" oor:package="org.openoffice.Office">
 <node oor:name="ServiceManager">
  <node oor:name="Dictionaries">
   <node oor:name="HunSpellDic_es" oor:op="fuse">
    <prop oor:name="Locations" oor:type="oor:string-list"><value>%origin%/es.aff %origin%/es.dic</value></prop>
    <prop oor:name="Format" oor:type="xs:string"><value>DICT_SPELL</value></prop>
    <prop oor:name="Locales" oor:type="oor:string-list"><value>es-AR es-BO es-ES es-MX</value></prop>
   </node>
   <node oor:name="HunSpellDic_de-DE_frami" oor:op="fuse">
    <prop oor:name="Locations" oor:type="oor:string-list"><value>%origin%/de/de_DE_frami.aff %origin%/de/de_DE_frami.dic</value></prop>
    <prop oor:name="Format" oor:type="xs:string"><value>DICT_SPELL</value></prop>
    <prop oor:name="Locales" oor:type="oor:string-list"><value>de-DE</value></prop>
   </node>
   <node oor:name="HyphDic_es" oor:op="fuse">
    <prop oor:name="Locations" oor:type="oor:string-list"><value>%origin%/hyph_es.dic</value></prop>
    <prop oor:name="Format" oor:type="xs:string"><value>DICT_HYPH</value></prop>
    <prop oor:name="Locales" oor:type="oor:string-list"><value>es-ES</value></prop>
   </node>
  </node>
 </node>
</oor:component-data>"#;

    #[test]
    fn codes() {
        assert_eq!(code_of("de_DE_frami").as_deref(), Some("de_DE"));
        assert_eq!(code_of("en-us").as_deref(), Some("en_US"));
        assert_eq!(code_of("en_US-large").as_deref(), Some("en_US"));
        assert_eq!(code_of("es_ANY").as_deref(), Some("es"));
        assert_eq!(code_of("index"), None);
        assert_eq!(pick_locale(&["es-AR".into(), "es-ES".into()], Some("es")).as_deref(), Some("es_ES"));
        assert_eq!(pick_locale(&["en-GB".into(), "en-IE".into()], Some("en_GB")).as_deref(), Some("en_GB"));
    }

    // An extension as LibreOffice's site has them: its dictionaries in
    // folders and named as the .xcu says, hyphenation left out, the
    // thesaurus kept; then found and loaded from the folder.
    #[test]
    fn installs_an_extension() {
        let src = temp("oxt-src");
        let dir = temp("oxt-dir");
        let mut z = apex_io::zip::ZipWriter::new();
        z.add("META-INF/manifest.xml", b"<manifest/>", true);
        z.add("dictionaries.xcu", XCU.as_bytes(), true);
        z.add("es.aff", b"SET UTF-8\n", true);
        z.add("es.dic", b"1\nhola\n", true);
        z.add("de/de_DE_frami.aff", b"SET UTF-8\n", true);
        z.add("de/de_DE_frami.dic", b"1\nhallo\n", true);
        z.add("hyph_es.dic", b"UTF-8\n", true);
        z.add("th_es_ES_v2.dat", b"UTF-8\n", true);
        z.add("th_es_ES_v2.idx", b"UTF-8\n", true);
        let oxt = src.join("dict-es.oxt");
        apex_fs::write(&oxt, z.finish()).expect("write");
        let mut langs = install_into(&dir, &[oxt]).expect("install");
        langs.sort();
        assert_eq!(langs, ["de_DE", "es_ES"]);
        for f in ["es_ES.aff", "es_ES.dic", "de_DE.aff", "de_DE.dic", "th_es_ES_v2.dat", "th_es_ES_v2.idx"] {
            assert!(dir.join(f).fs_is_file(), "{f}");
        }
        assert!(!dir.join("hyph_es.dic").fs_exists() && !dir.join("es_ES.aff.part").fs_exists());
        apex_spell::add_search_dir(dir.clone());
        let found = apex_spell::installed();
        let (_, aff, dic) = found.iter().find(|(l, aff, _)| l == "es_ES" && aff.starts_with(&dir)).expect("listed");
        assert!(apex_spell::Dictionary::load(aff, dic).expect("load").check("hola"));
        let _ = apex_fs::remove_dir_all(&src);
        let _ = apex_fs::remove_dir_all(&dir);
    }

    // The two files chosen, or one with the other beside it.
    #[test]
    fn installs_the_files() {
        let src = temp("files-src");
        let dir = temp("files-dir");
        apex_fs::write(src.join("fr-FR.aff"), "SET UTF-8\n").expect("write");
        apex_fs::write(src.join("fr-FR.dic"), "1\nbonjour\n").expect("write");
        assert_eq!(install_into(&dir, &[src.join("fr-FR.aff"), src.join("fr-FR.dic")]).expect("install"), ["fr_FR"]);
        assert_eq!(install_into(&dir, &[src.join("fr-FR.dic")]).expect("install"), ["fr_FR"]);
        assert!(dir.join("fr_FR.aff").fs_is_file() && dir.join("fr_FR.dic").fs_is_file());
        // Half of one isn't installed.
        apex_fs::write(src.join("it_IT.aff"), "SET UTF-8\n").expect("write");
        assert!(install_into(&dir, &[src.join("it_IT.aff")]).is_err());
        // Nor a zip without dictionaries.
        let mut z = apex_io::zip::ZipWriter::new();
        z.add("readme.txt", b"hi", true);
        apex_fs::write(src.join("other.zip"), z.finish()).expect("write");
        assert!(install_into(&dir, &[src.join("other.zip")]).is_err());
        assert!(!dir.join("it_IT.aff").fs_exists());
        let _ = apex_fs::remove_dir_all(&src);
        let _ = apex_fs::remove_dir_all(&dir);
    }
}
