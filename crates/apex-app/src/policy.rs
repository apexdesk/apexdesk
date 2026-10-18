//! Central configuration: settings an organization fixes for everyone.
//!
//! Policies are read once, at start, from (highest precedence first):
//! - Windows: the registry, `HKLM` then `HKCU\Software\Policies\ApexDesk`
//!   (what the ADMX templates in `packaging/windows/policy` write);
//! - macOS: managed preferences (configuration profiles) for
//!   `com.github.apexdesk.apexdesk`, the computer's then the user's;
//! - Linux and other Unix: `/etc/apexdesk/policy`, then the files in
//!   `/etc/apexdesk/policy.d` (`Name = value` lines);
//! - everywhere, last, the file named by `APEXDESK_POLICY` (for tests and
//!   portable setups).
//! The first source that sets a policy wins. A policy overrides the user's
//! setting, which Options shows greyed out ("Set by your organization");
//! the user's own value stays in the settings file for when the policy
//! goes away.

pub(crate) mod admx;
pub(crate) mod sources;

use std::sync::OnceLock;

/// What values a policy takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 1 or 0 (a DWORD in the registry, a boolean in a profile).
    Bool,
    Text,
    /// A folder path.
    Folder,
    Number(u32, u32),
    Choice(&'static [&'static str]),
    /// Several texts (a subkey of values in the registry, an array in a
    /// profile, repeated lines in a file).
    List,
}

pub struct Item {
    /// The policy's name in the registry, profiles and policy files.
    pub name: &'static str,
    /// The settings line it sets (`key=value`, see `Settings::set`).
    pub key: &'static str,
    pub kind: Kind,
}

pub const MACRO_MODES: &[&str] = &["disabled", "ask", "signed"];
pub const SHEET_FORMATS: &[&str] = &["xlsx", "xlsm", "ods", "csv"];
pub const DOC_FORMATS: &[&str] = &["docx", "docm", "odt", "rtf"];

pub const ITEMS: &[Item] = &[
    Item { name: "MacroSecurity", key: "macros", kind: Kind::Choice(MACRO_MODES) },
    Item { name: "TrustedLocations", key: "trusted_location", kind: Kind::List },
    Item { name: "TrustedPublishers", key: "trusted_publisher", kind: Kind::List },
    Item { name: "MacroRevocation", key: "macro_revocation", kind: Kind::Choice(&["check", "off", "require"]) },
    Item { name: "DisableMacroEditor", key: "macro_editor_off", kind: Kind::Bool },
    Item { name: "TemplatesFolder", key: "templates_folder", kind: Kind::Folder },
    Item { name: "BuildingBlocksFolder", key: "building_blocks_folder", kind: Kind::Folder },
    Item { name: "LibraryFolder", key: "library_folder", kind: Kind::Folder },
    Item { name: "SeatsFolder", key: "seats_folder", kind: Kind::Folder },
    Item { name: "OnlySharedTemplates", key: "only_shared_templates", kind: Kind::Bool },
    Item { name: "DefaultSheetFormat", key: "save_sheet_format", kind: Kind::Choice(SHEET_FORMATS) },
    Item { name: "DefaultDocFormat", key: "save_doc_format", kind: Kind::Choice(DOC_FORMATS) },
    Item { name: "UILanguage", key: "lang", kind: Kind::Text },
    Item { name: "SpellingLanguage", key: "spell_lang", kind: Kind::Text },
    Item { name: "DictionariesFolder", key: "dictionaries_folder", kind: Kind::Folder },
    Item { name: "DisableGetData", key: "get_data_off", kind: Kind::Bool },
    Item { name: "Proxy", key: "proxy", kind: Kind::Text },
    Item { name: "ProxyBypass", key: "proxy_bypass", kind: Kind::Text },
    Item { name: "SignInSites", key: "sign_in_sites", kind: Kind::Text },
    Item { name: "SupportEmail", key: "support_email", kind: Kind::Text },
    Item { name: "FontSubstitutes", key: "font_substitutes", kind: Kind::Text },
    Item { name: "CertificateRoots", key: "cert_roots", kind: Kind::Folder },
    Item { name: "SignatureLevel", key: "sign_level", kind: Kind::Choice(&["b", "t", "lt", "lta"]) },
    Item { name: "TimestampURL", key: "tsa_url", kind: Kind::Text },
    Item { name: "Renderer", key: "renderer", kind: Kind::Choice(&["glow", "wgpu"]) },
    Item { name: "LockFiles", key: "lock_files", kind: Kind::Bool },
    Item { name: "LinkSecurity", key: "link_security", kind: Kind::Choice(&["ask", "allow", "block"]) },
    Item { name: "ProtectedView", key: "protected_view", kind: Kind::Bool },
    Item { name: "AllowAIAssistants", key: "mcp", kind: Kind::Bool },
    Item { name: "AllowAutomationAPI", key: "automation_api", kind: Kind::Bool },
    Item { name: "AutosaveMinutes", key: "autosave_minutes", kind: Kind::Number(0, 120) },
    Item { name: "RequiredProperties", key: "required_property", kind: Kind::List },
    Item { name: "CheckBeforeSharing", key: "share_check", kind: Kind::Choice(&["off", "warn", "block"]) },
    Item { name: "BrandFonts", key: "brand_font", kind: Kind::List },
    Item { name: "BrandColors", key: "brand_color", kind: Kind::List },
    Item { name: "ForbiddenTerms", key: "forbidden_term", kind: Kind::List },
];

impl Item {
    /// Whether it applies without a business license: the policies that
    /// keep people safe (macros, links, downloaded files, sources of data,
    /// certificates, signatures)
    /// and the renderer, a fix for broken graphics drivers. The rest manage
    /// many computers alike and need a license with `management`.
    pub fn free(&self) -> bool {
        matches!(self.name, "MacroSecurity" | "TrustedLocations" | "TrustedPublishers" | "MacroRevocation" | "DisableGetData" | "CertificateRoots" | "SignatureLevel" | "TimestampURL" | "Renderer" | "LinkSecurity" | "ProtectedView")
    }

    pub fn by_name(name: &str) -> Option<&'static Item> {
        ITEMS.iter().find(|i| i.name.eq_ignore_ascii_case(name.trim()))
    }

    /// The settings file lines the policy stands for.
    pub fn saved_keys(&'static self) -> &'static [&'static str] {
        match self.key {
            "macros" => &["macros_enabled", "macros_signed_only"],
            _ => std::slice::from_ref(&self.key),
        }
    }

    /// A value as the settings take it, or None when it isn't valid.
    fn normalize(&self, v: &str) -> Option<String> {
        let v = v.trim();
        match self.kind {
            Kind::Bool => match v.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Some("1".into()),
                "0" | "false" | "no" | "off" => Some("0".into()),
                _ => None,
            },
            Kind::Number(lo, hi) => v.parse::<u32>().ok().filter(|n| (lo..=hi).contains(n)).map(|n| n.to_string()),
            Kind::Choice(c) => c.iter().find(|x| x.eq_ignore_ascii_case(v)).map(|x| x.to_string()),
            Kind::Folder => Some(expand(v)).filter(|s| !s.is_empty()),
            Kind::Text | Kind::List => Some(v.to_string()),
        }
    }
}

/// `%VAR%` (Windows) and `$VAR` / `${VAR}` in folder policies.
fn expand(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(['%', '$']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let (name, len) = if let Some(t) = tail.strip_prefix('%') {
            match t.find('%') {
                Some(j) => (&t[..j], j + 2),
                None => ("", 0),
            }
        } else if let Some(t) = tail.strip_prefix("${") {
            match t.find('}') {
                Some(j) => (&t[..j], j + 3),
                None => ("", 0),
            }
        } else {
            let t = &tail[1..];
            let j = t.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(t.len());
            (&t[..j], j + 1)
        };
        match std::env::var(name).ok().filter(|_| !name.is_empty()) {
            Some(v) => {
                out.push_str(&v);
                rest = &tail[len..];
            }
            None => {
                out.push_str(&tail[..1]);
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A policy in force: its values and where it comes from.
pub struct Set {
    pub item: &'static Item,
    pub values: Vec<String>,
    /// The key, profile or file that sets it.
    pub source: String,
}

#[cfg(test)]
impl Set {
    pub fn value(&self) -> &str {
        self.values.first().map_or("", String::as_str)
    }
}

#[derive(Default)]
pub struct Policy {
    pub set: Vec<Set>,
    /// Values that were ignored (unknown policy, wrong value).
    pub ignored: Vec<String>,
    /// Policies set but not applied: they need a business license.
    pub unlicensed: Vec<String>,
}

impl Policy {
    /// From `(name, value, source)` entries, highest precedence first;
    /// without `licensed`, only the free policies (see [`Item::free`]).
    pub fn from_entries(entries: Vec<(String, String, String)>, licensed: bool) -> Policy {
        let mut p = Policy::default();
        for (name, value, source) in entries {
            let Some(item) = Item::by_name(&name) else {
                p.ignored.push(format!("{source}: {name}"));
                continue;
            };
            let Some(v) = item.normalize(&value) else {
                p.ignored.push(format!("{source}: {name} = {value}"));
                continue;
            };
            if !licensed && !item.free() {
                if !p.unlicensed.iter().any(|n| n == item.name) {
                    p.unlicensed.push(item.name.to_string());
                }
                continue;
            }
            match p.set.iter_mut().find(|s| std::ptr::eq(s.item, item)) {
                // A list gathers the values of its source.
                Some(s) if item.kind == Kind::List && s.source == source => {
                    if !v.is_empty() {
                        s.values.push(v);
                    }
                }
                Some(_) => {}
                None => p.set.push(Set { item, values: if v.is_empty() && item.kind == Kind::List { Vec::new() } else { vec![v] }, source }),
            }
        }
        p
    }

    pub fn get(&self, name: &str) -> Option<&Set> {
        self.set.iter().find(|s| s.item.name == name)
    }

    /// Whether the settings line `key` is fixed by a policy.
    pub fn locks_key(&self, key: &str) -> bool {
        self.set.iter().any(|s| s.item.saved_keys().contains(&key))
    }
}

static CURRENT: OnceLock<Policy> = OnceLock::new();

/// The policies in force (read on first use).
pub fn current() -> &'static Policy {
    CURRENT.get_or_init(|| Policy::from_entries(sources::read(), crate::license::has("management")))
}

/// Whether the policy `name` (see `ITEMS`) is set.
pub fn locked(name: &str) -> bool {
    current().get(name).is_some()
}

/// "Set by your organization" under a control a policy fixes.
pub fn note(ui: &mut eframe::egui::Ui, name: &str) {
    if let Some(s) = current().get(name) {
        ui.label(eframe::egui::RichText::new(crate::i18n::tr("policy.locked")).small().italics().weak()).on_hover_text(&s.source);
    }
}

/// Runs `add` greyed out when the policy `name` is set, with the note.
pub fn guard<R>(ui: &mut eframe::egui::Ui, name: &str, add: impl FnOnce(&mut eframe::egui::Ui) -> R) -> R {
    let r = ui.add_enabled_ui(!locked(name), add).inner;
    note(ui, name);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(n: &str, v: &str, s: &str) -> (String, String, String) {
        (n.into(), v.into(), s.into())
    }

    #[test]
    fn precedence_and_values() {
        let p = Policy::from_entries(vec![
            e("macrosecurity", "Signed", "HKLM"),
            e("TrustedLocations", "C:\\A", "HKLM"),
            e("TrustedLocations", "C:\\B", "HKLM"),
            e("MacroSecurity", "ask", "HKCU"),
            e("TrustedLocations", "C:\\C", "HKCU"),
            e("DisableGetData", "true", "HKCU"),
            e("AutosaveMinutes", "500", "HKLM"),
            e("Nonsense", "1", "HKLM"),
        ], true);
        assert_eq!(p.get("MacroSecurity").unwrap().value(), "signed");
        assert_eq!(p.get("MacroSecurity").unwrap().source, "HKLM");
        assert_eq!(p.get("TrustedLocations").unwrap().values, ["C:\\A", "C:\\B"]);
        assert_eq!(p.get("DisableGetData").unwrap().value(), "1");
        assert!(p.get("AutosaveMinutes").is_none());
        assert_eq!(p.ignored.len(), 2);
        assert!(p.locks_key("macros_signed_only") && p.locks_key("get_data_off") && !p.locks_key("lang"));
    }

    #[test]
    fn management_policies_need_a_license() {
        let entries = || vec![e("MacroSecurity", "disabled", "f"), e("UILanguage", "es", "f"), e("TemplatesFolder", "/srv/t", "f"), e("UILanguage", "fr", "g")];
        let p = Policy::from_entries(entries(), false);
        assert_eq!(p.get("MacroSecurity").unwrap().value(), "disabled");
        assert!(p.get("UILanguage").is_none() && p.get("TemplatesFolder").is_none());
        assert_eq!(p.unlicensed, ["UILanguage", "TemplatesFolder"]);
        assert!(p.ignored.is_empty());
        let p = Policy::from_entries(entries(), true);
        assert_eq!(p.get("UILanguage").unwrap().value(), "es");
        assert!(p.unlicensed.is_empty());
    }

    #[test]
    fn folders_expand_variables() {
        // SAFETY: tests of this module don't read this variable elsewhere.
        unsafe { std::env::set_var("APEXDESK_TEST_SHARE", "/srv/share") };
        assert_eq!(expand("%APEXDESK_TEST_SHARE%/tpl"), "/srv/share/tpl");
        assert_eq!(expand("${APEXDESK_TEST_SHARE}/tpl"), "/srv/share/tpl");
        assert_eq!(expand("$APEXDESK_TEST_SHARE/tpl"), "/srv/share/tpl");
        assert_eq!(expand("50% off $NOPE_X"), "50% off $NOPE_X");
    }

    #[test]
    fn every_item_saves_its_key() {
        for i in ITEMS {
            assert!(i.saved_keys().contains(&i.key) || i.key == "macros", "{}", i.name);
        }
    }
}
