//! Macros of a file, kept so saving never loses them: the VBA project of
//! .xlsm/.docm files (and of .xls/.doc) with the package parts that go
//! with it, and the Basic libraries of LibreOffice files. ApexDesk does
//! not run them; `apex_io::vba` reads and rewrites the VBA project.

use std::sync::Arc;

/// Whom a kept package part hangs from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartOwner {
    /// The VBA project (vbaProject.bin's relationships): signatures and
    /// Word's vbaData.xml. Its path is relative to the project's folder.
    Project,
    /// The package root (_rels/.rels): ribbon customizations (customUI).
    Root,
    /// Another kept part (images of a customUI, a .rels file): copied as is.
    Other,
}

/// A part of an OOXML package, or a file of an ODF package, kept as read.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptPart {
    /// Path in the package (see [`PartOwner`]).
    pub path: String,
    /// Content type (ODF: media type of the manifest; may be empty).
    pub content_type: String,
    /// Relationship type from the owner (empty for [`PartOwner::Other`]).
    pub rel_type: String,
    pub owner: PartOwner,
    pub data: Arc<Vec<u8>>,
}

/// The macros of a workbook or document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Macros {
    /// The VBA project: an OLE compound file with the `VBA` storage and
    /// the `PROJECT` stream (vbaProject.bin).
    pub project: Option<Arc<Vec<u8>>>,
    /// OOXML parts that go with the project or customize the ribbon.
    pub parts: Vec<KeptPart>,
    /// Basic libraries and scripts of an ODF file (`Basic/…`, `Scripts/…`).
    pub odf: Vec<KeptPart>,
    /// The project came with a digital signature (a changed project loses it).
    pub signed: bool,
}

impl Macros {
    /// Whether there are macros to warn about (a VBA project or Basic code).
    pub fn has_code(&self) -> bool {
        self.project.is_some() || self.odf.iter().any(|p| p.path.starts_with("Basic/") && !p.path.ends_with("script-lc.xml") && !p.path.ends_with("script-lb.xml"))
    }

    pub fn is_empty(&self) -> bool {
        self.project.is_none() && self.parts.is_empty() && self.odf.is_empty()
    }

    /// Replaces the VBA project with an edited one: its signatures no
    /// longer match, so they go.
    pub fn set_project(&mut self, bin: Vec<u8>) {
        self.project = Some(Arc::new(bin));
        self.parts.retain(|p| !p.rel_type.contains("vbaProjectSignature"));
        self.signed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edited_project_drops_signatures() {
        let part = |name: &str, rel: &str| KeptPart { path: name.into(), content_type: String::new(), rel_type: rel.into(), owner: PartOwner::Project, data: Arc::new(vec![1]) };
        let mut m = Macros { project: Some(Arc::new(vec![0])), parts: vec![part("vbaData.xml", "http://schemas.microsoft.com/office/2006/relationships/wordVbaData"), part("vbaProjectSignature.bin", "http://schemas.microsoft.com/office/2006/relationships/vbaProjectSignature")], odf: Vec::new(), signed: true };
        assert!(m.has_code());
        m.set_project(vec![2]);
        assert_eq!(m.parts.len(), 1);
        assert!(!m.signed);
        assert_eq!(m.project.as_deref(), Some(&vec![2]));
    }
}
