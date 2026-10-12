//! Parts of an Office package ApexDesk doesn't edit but keeps on save:
//! custom XML, data connections and queries, the data model, pivot
//! tables, the sensitivity label list… They go back to the file as they
//! came, and what nothing points at any more is left out.
//!
//! Also sensitivity labels (Microsoft Purview): shown, never applied or
//! changed, since their policies live with the organization.

use std::sync::Arc;

use crate::{Meta, PropValue};

/// A part copied as it is.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptPart {
    /// Path in the package ("xl/connections.xml").
    pub path: String,
    /// Content type ("" for relationship parts).
    pub content_type: String,
    pub data: Arc<[u8]>,
}

/// A relationship to a kept part from a part ApexDesk writes.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptLink {
    pub rel_type: String,
    /// Path of the target in the package.
    pub target: String,
    /// Id in the source file (kept XML elements refer to it).
    pub id: String,
}

/// What a file kept for its next save.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeptParts {
    pub parts: Vec<KeptPart>,
    /// Relationships of the main part (workbook, document).
    pub links: Vec<KeptLink>,
    /// Relationships of the package (`_rels/.rels`).
    pub package_links: Vec<KeptLink>,
    /// Elements of the main part that point at kept parts, with the slot
    /// they go in ("pivotCache", "ext").
    pub xml: Vec<(String, String)>,
    /// Labels of `docMetadata/LabelInfo.xml`.
    pub labels: Vec<SensitivityLabel>,
}

impl KeptParts {
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    pub fn part(&self, path: &str) -> Option<&KeptPart> {
        self.parts.iter().find(|p| p.path == path)
    }

    /// Adds a part unless one is already at its path.
    pub fn add(&mut self, part: KeptPart) {
        if self.part(&part.path).is_none() {
            self.parts.push(part);
        }
    }

    /// The labels of the file: its custom properties (which may name them)
    /// and the label list of newer Office.
    pub fn sensitivity_labels(&self, meta: &Meta) -> Vec<SensitivityLabel> {
        let mut out = meta.sensitivity_labels();
        for l in &self.labels {
            if l.enabled && !out.iter().any(|o| same_id(&o.id, &l.id)) {
                out.push(l.clone());
            }
        }
        out
    }
}

/// Where a table's data comes from (an xlsx query table), kept with the
/// table while its columns stay as they were.
#[derive(Clone, Debug, PartialEq)]
pub struct TableQuery {
    pub link: KeptLink,
    /// The columns as the file had them: name, id and the rest of their
    /// attributes (`uniqueName`, `queryTableFieldId`).
    pub columns: Vec<(String, u32, String)>,
}

/// A sensitivity label on the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SensitivityLabel {
    /// Label GUID.
    pub id: String,
    /// Display name, when the file says it (older Office writes it).
    pub name: String,
    /// Tenant the label belongs to.
    pub site: String,
    /// "Standard" (by policy) or "Privileged" (chosen by a person).
    pub method: String,
    pub enabled: bool,
}

/// Prefix of the label custom properties (`MSIP_Label_<guid>_<field>`).
pub const LABEL_PREFIX: &str = "MSIP_Label_";

/// Whether a custom property belongs to a sensitivity label.
pub fn is_label_prop(name: &str) -> bool {
    name.starts_with(LABEL_PREFIX)
}

fn same_id(a: &str, b: &str) -> bool {
    let t = |s: &str| s.trim_matches(|c| c == '{' || c == '}').to_ascii_lowercase();
    t(a) == t(b)
}

impl Meta {
    /// Sensitivity labels named by the custom properties (enabled ones).
    pub fn sensitivity_labels(&self) -> Vec<SensitivityLabel> {
        let mut out: Vec<SensitivityLabel> = Vec::new();
        for p in &self.custom {
            let Some(rest) = p.name.strip_prefix(LABEL_PREFIX) else { continue };
            // The GUID has no underscores; the field follows the next one.
            let Some((id, field)) = rest.split_once('_') else { continue };
            let i = match out.iter().position(|l| same_id(&l.id, id)) {
                Some(i) => i,
                None => {
                    out.push(SensitivityLabel { id: id.to_string(), ..SensitivityLabel::default() });
                    out.len() - 1
                }
            };
            let text = p.value.text();
            let l = &mut out[i];
            match field {
                "Enabled" => l.enabled = matches!(&p.value, PropValue::Bool(true)) || text.eq_ignore_ascii_case("true") || text == "1",
                "Name" => l.name = text,
                "SiteId" => l.site = text,
                "Method" => l.method = text,
                _ => {}
            }
        }
        out.retain(|l| l.enabled);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CustomProp;

    #[test]
    fn labels_from_properties() {
        let prop = |n: &str, v: &str| CustomProp { name: format!("MSIP_Label_f42aa342-8706-4288-bd11-ebb85995028c_{n}"), value: PropValue::Text(v.into()) };
        let mut m = Meta::default();
        m.custom = vec![prop("Enabled", "true"), prop("Name", "Confidencial"), prop("SiteId", "72f988bf"), prop("Method", "Privileged"), CustomProp { name: "Otro".into(), value: PropValue::Text("x".into()) }];
        let l = m.sensitivity_labels();
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].name.as_str(), l[0].method.as_str(), l[0].site.as_str()), ("Confidencial", "Privileged", "72f988bf"));
        let mut kept = KeptParts::default();
        kept.labels.push(SensitivityLabel { id: "{F42AA342-8706-4288-BD11-EBB85995028C}".into(), enabled: true, ..SensitivityLabel::default() });
        kept.labels.push(SensitivityLabel { id: "0000".into(), enabled: true, ..SensitivityLabel::default() });
        assert_eq!(kept.sensitivity_labels(&m).len(), 2);
        m.custom[0] = prop("Enabled", "false");
        assert!(m.sensitivity_labels().is_empty());
    }
}
