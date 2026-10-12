//! Macros in packages: the VBA project of xlsm/docm files (vbaProject.bin,
//! its signatures, Word's vbaData.xml) and ribbon customizations (customUI)
//! read as opaque parts and written back with their relationships; the
//! Basic libraries of ODF files likewise. See [`apex_doc::Macros`].

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::macros::{KeptPart, Macros, PartOwner};

use crate::xml::{Ev, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

pub(crate) const REL_VBA: &str = "http://schemas.microsoft.com/office/2006/relationships/vbaProject";
const CT_VBA: &str = "application/vnd.ms-office.vbaProject";
/// Ribbon customizations (Office 2007 and 2010 flavors).
const REL_UI: [&str; 2] = ["http://schemas.microsoft.com/office/2006/relationships/ui/extensibility", "http://schemas.microsoft.com/office/2007/relationships/ui/extensibility"];

/// Macro-enabled and template flavors of xlsx and docx, told by the
/// extension (.xlsm, .xltx, .xltm, .docm, .dotx, .dotm).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flavor {
    pub macros: bool,
    pub template: bool,
}

impl Flavor {
    pub fn from_path(p: &std::path::Path) -> Flavor {
        let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        Flavor { macros: matches!(ext.as_str(), "xlsm" | "xltm" | "docm" | "dotm"), template: matches!(ext.as_str(), "xltx" | "xltm" | "dotx" | "dotm") }
    }

    /// The flavor of `path`; for other extensions (a temporary file), the
    /// one that keeps the macros.
    pub fn for_path(p: &std::path::Path, m: &Macros) -> Flavor {
        let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        if ["xlsx", "xlsm", "xltx", "xltm", "docx", "docm", "dotx", "dotm"].contains(&ext.as_str()) { Flavor::from_path(p) } else { Flavor::for_macros(m) }
    }

    /// Macro-enabled when there is a project to keep.
    pub fn for_macros(m: &Macros) -> Flavor {
        Flavor { macros: m.project.is_some(), template: false }
    }

    /// Content type of workbook.xml.
    pub(crate) fn workbook_type(self) -> &'static str {
        match (self.macros, self.template) {
            (false, false) => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
            (false, true) => "application/vnd.openxmlformats-officedocument.spreadsheetml.template.main+xml",
            (true, false) => "application/vnd.ms-excel.sheet.macroEnabled.main+xml",
            (true, true) => "application/vnd.ms-excel.template.macroEnabled.main+xml",
        }
    }

    /// Content type of document.xml.
    pub(crate) fn document_type(self) -> &'static str {
        match (self.macros, self.template) {
            (false, false) => "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
            (false, true) => "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
            (true, false) => "application/vnd.ms-word.document.macroEnabled.main+xml",
            (true, true) => "application/vnd.ms-word.template.macroEnabledTemplate.main+xml",
        }
    }
}

/// Content types of a package: (defaults by extension, overrides by part).
fn content_types(z: &ZipReader) -> (HashMap<String, String>, HashMap<String, String>) {
    let (mut defaults, mut overrides) = (HashMap::new(), HashMap::new());
    let Ok(xml) = z.read_string("[Content_Types].xml") else { return (defaults, overrides) };
    let mut r = Reader::new(&xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => {
                let ct = t.attr("ContentType").unwrap_or_default().into_owned();
                if t.local() == "Default" {
                    defaults.insert(t.attr("Extension").unwrap_or_default().to_ascii_lowercase(), ct);
                } else if t.local() == "Override" {
                    overrides.insert(t.attr("PartName").unwrap_or_default().trim_start_matches('/').to_string(), ct);
                }
            }
            _ => {}
        }
    }
    (defaults, overrides)
}

/// Relationships of a .rels part: (type, target) of the internal ones.
fn rels(z: &ZipReader, path: &str) -> Vec<(String, String)> {
    let Ok(xml) = z.read_string(path) else { return Vec::new() };
    crate::xlsx_notes::read_rels(&xml).into_values().filter(|(_, _, external)| !external).map(|(ty, target, _)| (ty, target)).collect()
}

/// The .rels part of `path`.
fn rels_path(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{path}.rels"),
    }
}

/// Reads the macros of an xlsx/docx package whose main part is in `dir`
/// ("xl", "word") and has the relationships `main_rels`.
pub(crate) fn read(z: &ZipReader, dir: &str, main_rels: &str) -> Macros {
    let mut m = Macros::default();
    let (defaults, overrides) = content_types(z);
    let type_of = |path: &str| overrides.get(path).cloned().or_else(|| path.rsplit_once('.').and_then(|(_, e)| defaults.get(&e.to_ascii_lowercase()).cloned())).unwrap_or_default();
    let mut keep = |m: &mut Macros, path: &str, rel_type: &str, owner: PartOwner, name: String| {
        if m.parts.iter().any(|p| p.path == name) {
            return false;
        }
        let Ok(data) = z.read(path) else { return false };
        m.parts.push(KeptPart { path: name, content_type: type_of(path), rel_type: rel_type.to_string(), owner, data: Arc::new(data) });
        true
    };
    // A kept part's own parts (images of a customUI): copied as they are.
    let keep_children = |m: &mut Macros, path: &str, keep: &mut dyn FnMut(&mut Macros, &str, &str, PartOwner, String) -> bool| {
        let rp = rels_path(path);
        let dir = path.rsplit_once('/').map_or("", |d| d.0);
        let children = rels(z, &rp);
        if keep(m, &rp, "", PartOwner::Other, rp.clone()) {
            for (_, target) in children {
                let p = crate::xlsx::resolve_target(dir, &target);
                keep(m, &p, "", PartOwner::Other, p.clone());
            }
        }
    };
    let main = crate::xlsx_notes::read_rels(main_rels);
    if let Some((_, target, _)) = main.values().find(|(t, _, _)| t == REL_VBA) {
        let path = crate::xlsx::resolve_target(dir, target);
        if let Ok(bin) = z.read(&path) {
            m.project = Some(Arc::new(bin));
            let base = path.rsplit_once('/').map_or("", |d| d.0).to_string();
            for (ty, target) in rels(z, &rels_path(&path)) {
                let p = crate::xlsx::resolve_target(&base, &target);
                // Kept relative to the project's folder.
                let name = p.strip_prefix(&format!("{base}/")).unwrap_or(&p).to_string();
                m.signed |= ty.contains("vbaProjectSignature");
                keep(&mut m, &p, &ty, PartOwner::Project, name);
            }
        }
    }
    for (ty, target) in rels(z, "_rels/.rels") {
        if REL_UI.contains(&ty.as_str()) {
            let p = crate::xlsx::resolve_target("", &target);
            if keep(&mut m, &p, &ty, PartOwner::Root, p.clone()) {
                keep_children(&mut m, &p, &mut keep);
            }
        }
    }
    m
}

/// What [`write`] adds to the package's own parts.
#[derive(Default)]
pub(crate) struct Written {
    /// Overrides for [Content_Types].xml.
    pub types: String,
    /// Relationships of the main part (to the project).
    pub main_rels: String,
    /// Relationships of the package root (customUI).
    pub root_rels: String,
}

/// Writes the kept macro parts of an xlsx/docx package (main part in
/// `dir`); without `flavor.macros` the VBA project is left out, as Office
/// does when saving a macro-free file.
pub(crate) fn write(z: &mut ZipWriter, m: &Macros, dir: &str, flavor: Flavor) -> Written {
    let mut w = Written::default();
    let mut project_rels = String::new();
    let project = m.project.as_ref().filter(|_| flavor.macros);
    if let Some(bin) = project {
        z.add(&format!("{dir}/vbaProject.bin"), bin, true);
        let _ = write!(w.types, r#"<Override PartName="/{dir}/vbaProject.bin" ContentType="{CT_VBA}"/>"#);
        let _ = write!(w.main_rels, r#"<Relationship Id="rIdVBA" Type="{REL_VBA}" Target="vbaProject.bin"/>"#);
    }
    for (i, p) in m.parts.iter().enumerate() {
        let path = match p.owner {
            PartOwner::Project if project.is_none() => continue,
            PartOwner::Project => {
                let _ = write!(project_rels, r#"<Relationship Id="rId{}" Type="{}" Target="{}"/>"#, i + 1, esc(&p.rel_type), esc(&p.path));
                format!("{dir}/{}", p.path)
            }
            PartOwner::Root => {
                let _ = write!(w.root_rels, r#"<Relationship Id="rIdUI{}" Type="{}" Target="{}"/>"#, i + 1, esc(&p.rel_type), esc(&p.path));
                p.path.clone()
            }
            PartOwner::Other => p.path.clone(),
        };
        if !p.content_type.is_empty() && !path.ends_with(".rels") {
            let _ = write!(w.types, r#"<Override PartName="/{}" ContentType="{}"/>"#, esc(&path), esc(&p.content_type));
        }
        z.add(&path, &p.data, true);
    }
    if !project_rels.is_empty() {
        z.add(&format!("{dir}/_rels/vbaProject.bin.rels"), format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{project_rels}</Relationships>"#).as_bytes(), true);
    }
    w
}

// --- ODF -------------------------------------------------------------------------------

/// The Basic libraries and scripts of an ODF package.
pub(crate) fn read_odf(z: &ZipReader) -> Vec<KeptPart> {
    let names: Vec<String> = z.names().filter(|n| (n.starts_with("Basic/") || n.starts_with("Scripts/")) && !n.ends_with('/')).map(str::to_string).collect();
    names
        .into_iter()
        .filter_map(|n| {
            let data = z.read(&n).ok()?;
            let content_type = if n.ends_with(".xml") { "text/xml".to_string() } else { String::new() };
            Some(KeptPart { path: n, content_type, rel_type: String::new(), owner: PartOwner::Other, data: Arc::new(data) })
        })
        .collect()
}

/// Files for an ODF package: the kept Basic libraries, or else the VBA
/// modules as a VBA-compatible Basic library, as LibreOffice saves them.
/// Returns (path, media type, data).
pub(crate) fn odf_files(m: &Macros) -> Vec<(String, String, Arc<Vec<u8>>)> {
    if !m.odf.is_empty() {
        return m.odf.iter().map(|p| (p.path.clone(), p.content_type.clone(), p.data.clone())).collect();
    }
    let Some(project) = m.project.as_ref().and_then(|b| crate::vba::read(b).ok()) else { return Vec::new() };
    // Names from the document become folders and files in the package.
    let dir = crate::safe_path::safe_file_name(&project.name, "VBAProject");
    let (modules, library) = (project.modules, esc(&dir).into_owned());
    const HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE "#;
    let xml = |s: String| Arc::new(s.into_bytes());
    let mut out = Vec::new();
    let mut elements = String::new();
    // Each module its own file, none the library's index.
    let mut taken: std::collections::HashSet<String> = ["script-lb".to_string()].into();
    for md in &modules {
        let base = crate::safe_path::safe_file_name(&md.name, "Module");
        let mut name = base.clone();
        let mut k = 1;
        while !taken.insert(name.to_lowercase()) {
            k += 1;
            name = format!("{base}_{k}");
        }
        use crate::vba::ModuleKind;
        let (ty, attr) = match md.kind {
            ModuleKind::Standard => ("normal", "VBAModule"),
            ModuleKind::Class => ("class", "VBAClassModule"),
            ModuleKind::Document => ("document", "VBADocumentModule"),
            ModuleKind::Form => ("form", "VBAFormModule"),
        };
        // Basic has no procedure attributes (LibreOffice drops them too).
        let code: String = md.code.split_inclusive('\n').filter(|l| !l.trim_start().starts_with("Attribute ")).collect();
        let source = format!("Rem Attribute VBA_ModuleType={attr}\nOption VBASupport 1\n{code}");
        out.push((format!("Basic/{dir}/{name}.xml"), "text/xml".to_string(), xml(format!(r#"{HEAD}script:module PUBLIC "-//OpenOffice.org//DTD OfficeDocument 1.0//EN" "module.dtd">
<script:module xmlns:script="http://openoffice.org/2000/script" script:name="{}" script:language="StarBasic" script:moduleType="{ty}">{}</script:module>"#, esc(&name), esc(&source).replace("&#10;", "\n")))));
        let _ = write!(elements, "\n <library:element library:name=\"{}\"/>", esc(&name));
    }
    out.push((format!("Basic/{dir}/script-lb.xml"), "text/xml".to_string(), xml(format!(r#"{HEAD}library:library PUBLIC "-//OpenOffice.org//DTD OfficeDocument 1.0//EN" "library.dtd">
<library:library xmlns:library="http://openoffice.org/2000/library" library:name="{library}" library:readonly="false" library:passwordprotected="false">{elements}
</library:library>"#))));
    out.push(("Basic/script-lc.xml".to_string(), "text/xml".to_string(), xml(format!(r#"{HEAD}library:libraries PUBLIC "-//OpenOffice.org//DTD OfficeDocument 1.0//EN" "libraries.dtd">
<library:libraries xmlns:library="http://openoffice.org/2000/library" xmlns:xlink="http://www.w3.org/1999/xlink">
 <library:library library:name="{library}" library:link="false"/>
</library:libraries>"#))));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::Document;
    use apex_sheet::Workbook;

    fn with_macros() -> Macros {
        let part = |path: &str, ct: &str, rel: &str, owner| KeptPart { path: path.into(), content_type: ct.into(), rel_type: rel.into(), owner, data: Arc::new(b"<x/>".to_vec()) };
        Macros {
            project: Some(Arc::new(crate::vba::tests::sample(false))),
            parts: vec![part("vbaData.xml", "application/vnd.ms-word.vbaData+xml", "http://schemas.microsoft.com/office/2006/relationships/wordVbaData", PartOwner::Project), part("customUI/customUI14.xml", "application/xml", REL_UI[1], PartOwner::Root)],
            odf: Vec::new(),
            signed: false,
        }
    }

    #[test]
    fn xlsm_roundtrip() {
        let mut wb = Workbook::new();
        wb.macros = with_macros();
        let data = crate::xlsx::write(&wb);
        let z = ZipReader::new(&data).unwrap();
        assert!(z.read_string("[Content_Types].xml").unwrap().contains("sheet.macroEnabled.main+xml"));
        let back = crate::xlsx::read(&data).unwrap();
        assert_eq!(back.macros, wb.macros);
        // Saved as .xlsx: the project goes, the ribbon stays.
        let plain = crate::xlsx::write_as(&wb, Flavor::default());
        let z = ZipReader::new(&plain).unwrap();
        assert!(!z.contains("xl/vbaProject.bin") && !z.contains("xl/vbaData.xml") && z.contains("customUI/customUI14.xml"));
        let back = crate::xlsx::read(&plain).unwrap();
        assert!(back.macros.project.is_none() && back.macros.parts.len() == 1);
    }

    #[test]
    fn docm_roundtrip() {
        let mut doc = Document::new();
        doc.macros = with_macros();
        let data = crate::docx::write_as(&doc, Flavor { macros: true, template: true });
        let z = ZipReader::new(&data).unwrap();
        assert!(z.read_string("[Content_Types].xml").unwrap().contains("macroEnabledTemplate.main+xml"));
        assert!(z.read_string("word/_rels/vbaProject.bin.rels").unwrap().contains("vbaData.xml"));
        assert_eq!(crate::docx::read(&data).unwrap().macros, doc.macros);
    }

    #[test]
    fn odf_basic() {
        // A VBA project becomes a VBA-compatible Basic library…
        let mut wb = Workbook::new();
        wb.macros = with_macros();
        let ods = crate::ods::write(&wb);
        let z = ZipReader::new(&ods).unwrap();
        let module = z.read_string("Basic/VBAProject/Module1.xml").unwrap();
        assert!(module.contains("Option VBASupport 1\nSub Hello()"), "{module}");
        assert!(z.read_string("META-INF/manifest.xml").unwrap().contains("Basic/VBAProject/script-lb.xml"));
        // …and Basic libraries are kept as they are.
        let back = crate::ods::read(&ods).unwrap();
        assert!(back.macros.project.is_none() && back.macros.has_code());
        let again = ZipReader::new(&crate::ods::write(&back)).unwrap().read_string("Basic/VBAProject/Module1.xml").unwrap();
        assert_eq!(again, module);
        let mut doc = Document::new();
        doc.macros = back.macros.clone();
        assert_eq!(crate::odt::read(&crate::odt::write(&doc)).unwrap().macros.odf, back.macros.odf);
    }

    #[test]
    fn flavors() {
        let f = |p: &str| Flavor::from_path(std::path::Path::new(p));
        assert_eq!(f("a.xlsm"), Flavor { macros: true, template: false });
        assert_eq!(f("a.DOTM"), Flavor { macros: true, template: true });
        assert_eq!(f("a.xltx"), Flavor { macros: false, template: true });
        assert_eq!(f("a.docx"), Flavor::default());
        assert_eq!(f("a.xlsm").workbook_type(), "application/vnd.ms-excel.sheet.macroEnabled.main+xml");
    }
}
