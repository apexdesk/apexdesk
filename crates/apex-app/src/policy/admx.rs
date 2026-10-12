//! The files administrators deploy policies with, made from `ITEMS` and
//! the interface translations: Windows ADMX/ADML templates (one ADML per
//! interface language), a macOS configuration profile and a Linux policy
//! file, both samples. `apexdesk policy --templates DIR` writes them; the
//! copies in `packaging/` are checked against them by a test.

use super::{ITEMS, Item, Kind};

/// Interface language -> Windows culture folder of its ADML.
const CULTURES: &[(&str, &str)] = &[("en", "en-US"), ("de", "de-DE"), ("es", "es-ES"), ("fr", "fr-FR"), ("it", "it-IT"), ("ja", "ja-JP"), ("ko", "ko-KR"), ("pt", "pt-BR"), ("zh", "zh-CN"), ("zh-Hant", "zh-TW")];

const KEY: &str = "Software\\Policies\\ApexDesk";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The text of `key` in language `code` (English when missing).
fn text(code: &str, key: &str) -> String {
    let langs = apex_i18n::langs();
    let find = |c: &str| langs.iter().find(|l| l.code == c).and_then(|l| l.get(key));
    find(code).or_else(|| find(apex_i18n::BASE)).unwrap_or(key).to_string()
}

/// A choice's label: translated for macro modes, the value
/// itself otherwise.
fn choice_label(code: &str, item: &Item, v: &str) -> String {
    match item.name {
        "MacroSecurity" | "CheckBeforeSharing" | "LinkSecurity" => text(code, &format!("policy.{}.{v}", item.name)),
        "SignatureLevel" => v.to_ascii_uppercase(),
        "Renderer" if v == "glow" => "OpenGL".into(),
        _ => v.to_string(),
    }
}

/// Every file, as (path under `packaging/`, contents).
pub fn files() -> Vec<(String, String)> {
    let mut out = vec![("windows/policy/ApexDesk.admx".to_string(), admx())];
    for (code, culture) in CULTURES {
        out.push((format!("windows/policy/{culture}/ApexDesk.adml"), adml(code)));
    }
    out.push(("macos/ApexDesk-policy.mobileconfig".into(), mobileconfig()));
    out.push(("linux/policy.example".into(), linux_sample()));
    out
}

fn admx() -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>
<!-- Made by the apexdesk policy command: edit the program, not this file. -->
<policyDefinitions xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" revision=\"1.0\" schemaVersion=\"1.0\" xmlns=\"http://schemas.microsoft.com/GroupPolicy/2006/07/PolicyDefinitions\">
  <policyNamespaces>
    <target prefix=\"apexdesk\" namespace=\"ApexDesk.Policies.ApexDesk\" />
    <using prefix=\"windows\" namespace=\"Microsoft.Policies.Windows\" />
  </policyNamespaces>
  <resources minRequiredRevision=\"1.0\" />
  <supportedOn>
    <definitions>
      <definition name=\"SUPPORTED_ApexDesk\" displayName=\"$(string.SUPPORTED_ApexDesk)\" />
    </definitions>
  </supportedOn>
  <categories>
    <category name=\"ApexDesk\" displayName=\"$(string.ApexDesk)\" />
  </categories>
  <policies>
",
    );
    for i in ITEMS {
        let n = i.name;
        let head = format!("    <policy name=\"{n}\" class=\"Both\" displayName=\"$(string.{n})\" explainText=\"$(string.{n}_Help)\" key=\"{KEY}\"");
        let tail = "      <parentCategory ref=\"ApexDesk\" />\n      <supportedOn ref=\"SUPPORTED_ApexDesk\" />\n";
        match i.kind {
            Kind::Bool => {
                s.push_str(&format!("{head} valueName=\"{n}\">\n{tail}"));
                s.push_str("      <enabledValue><decimal value=\"1\" /></enabledValue>\n      <disabledValue><decimal value=\"0\" /></disabledValue>\n");
            }
            _ => {
                s.push_str(&format!("{head} presentation=\"$(presentation.{n})\">\n{tail}      <elements>\n"));
                match i.kind {
                    Kind::Choice(c) => {
                        s.push_str(&format!("        <enum id=\"{n}\" valueName=\"{n}\" required=\"true\">\n"));
                        for v in c {
                            s.push_str(&format!("          <item displayName=\"$(string.{n}_{v})\"><value><string>{v}</string></value></item>\n"));
                        }
                        s.push_str("        </enum>\n");
                    }
                    Kind::Number(lo, hi) => s.push_str(&format!("        <decimal id=\"{n}\" valueName=\"{n}\" minValue=\"{lo}\" maxValue=\"{hi}\" required=\"true\" />\n")),
                    Kind::List => s.push_str(&format!("        <list id=\"{n}\" key=\"{KEY}\\{n}\" valuePrefix=\"\" />\n")),
                    Kind::Folder => s.push_str(&format!("        <text id=\"{n}\" valueName=\"{n}\" required=\"true\" expandable=\"true\" />\n")),
                    _ => s.push_str(&format!("        <text id=\"{n}\" valueName=\"{n}\" required=\"true\" />\n")),
                }
                s.push_str("      </elements>\n");
            }
        }
        s.push_str("    </policy>\n");
    }
    s.push_str("  </policies>\n</policyDefinitions>\n");
    s
}

fn adml(code: &str) -> String {
    let mut s = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>
<!-- Made by the apexdesk policy command: edit the program, not this file. -->
<policyDefinitionResources xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" revision=\"1.0\" schemaVersion=\"1.0\" xmlns=\"http://schemas.microsoft.com/GroupPolicy/2006/07/PolicyDefinitions\">
  <displayName>ApexDesk</displayName>
  <description>{}</description>
  <resources>
    <stringTable>
      <string id=\"ApexDesk\">ApexDesk</string>
      <string id=\"SUPPORTED_ApexDesk\">{}</string>
",
        esc(&text(code, "policy.title")),
        esc(&text(code, "policy.supported"))
    );
    for i in ITEMS {
        let n = i.name;
        s.push_str(&format!("      <string id=\"{n}\">{}</string>\n", esc(&text(code, &format!("policy.{n}")))));
        s.push_str(&format!("      <string id=\"{n}_Help\">{}</string>\n", esc(&text(code, &format!("policy.{n}.help")))));
        if let Kind::Choice(c) = i.kind {
            for v in c {
                s.push_str(&format!("      <string id=\"{n}_{v}\">{}</string>\n", esc(&choice_label(code, i, v))));
            }
        }
    }
    s.push_str("    </stringTable>\n    <presentationTable>\n");
    for i in ITEMS {
        let n = i.name;
        let label = esc(&text(code, &format!("policy.{n}")));
        let control = match i.kind {
            Kind::Bool => continue,
            Kind::Choice(_) => format!("<dropdownList refId=\"{n}\" noSort=\"true\">{label}</dropdownList>"),
            Kind::Number(..) => format!("<decimalTextBox refId=\"{n}\">{label}</decimalTextBox>"),
            Kind::List => format!("<listBox refId=\"{n}\">{label}</listBox>"),
            _ => format!("<textBox refId=\"{n}\"><label>{label}</label></textBox>"),
        };
        s.push_str(&format!("      <presentation id=\"{n}\">\n        {control}\n      </presentation>\n"));
    }
    s.push_str("    </presentationTable>\n  </resources>\n</policyDefinitionResources>\n");
    s
}

/// A sample value per policy, for the macOS and Linux samples.
fn sample(i: &Item) -> Vec<&'static str> {
    match i.name {
        "MacroSecurity" => vec!["signed"],
        "TrustedLocations" => vec!["/Volumes/Company/Macros", "/Users/Shared/Macros"],
        "TemplatesFolder" => vec!["/Volumes/Company/Templates"],
        "BuildingBlocksFolder" => vec!["/Volumes/Company/Building Blocks"],
        "LibraryFolder" => vec!["/Volumes/Company/Library"],
        "SeatsFolder" => vec!["/Volumes/Company/ApexDesk Seats"],
        "OnlySharedTemplates" => vec!["1"],
        "DefaultSheetFormat" => vec!["xlsx"],
        "DefaultDocFormat" => vec!["docx"],
        "UILanguage" => vec!["es"],
        "SpellingLanguage" => vec!["es_ES"],
        "DictionariesFolder" => vec!["/Volumes/Company/Dictionaries"],
        "DisableGetData" => vec!["0"],
        "Proxy" => vec!["http://proxy.example.com:3128"],
        "ProxyBypass" => vec![".corp.example.com, intranet"],
        "SignInSites" => vec![".corp.example.com"],
        "SupportEmail" => vec!["helpdesk@example.com"],
        "FontSubstitutes" => vec!["Corporate Sans=Liberation Sans; Corporate Serif=Liberation Serif"],
        "CertificateRoots" => vec!["/Volumes/Company/Certificates"],
        "SignatureLevel" => vec!["lt"],
        "TimestampURL" => vec!["http://timestamp.example.com"],
        "Renderer" => vec!["glow"],
        "LockFiles" => vec!["1"],
        "LinkSecurity" => vec!["ask"],
        "ProtectedView" => vec!["1"],
        "AllowAIAssistants" => vec!["0"],
        "AllowAutomationAPI" => vec!["0"],
        "RequiredProperties" => vec!["Client", "Case number"],
        "CheckBeforeSharing" => vec!["warn"],
        "BrandFonts" => vec!["Inter", "Source Serif 4"],
        "BrandColors" => vec!["#1F4E79", "#C00000", "#F2A900"],
        "ForbiddenTerms" => vec!["Confidential draft", "ACME Corp"],
        _ => vec!["5"],
    }
}

fn mobileconfig() -> String {
    let mut keys = String::new();
    for i in ITEMS {
        let v = sample(i);
        keys.push_str(&format!("\t\t\t<key>{}</key>\n", i.name));
        match i.kind {
            Kind::Bool => keys.push_str(if v[0] == "1" { "\t\t\t<true/>\n" } else { "\t\t\t<false/>\n" }),
            Kind::Number(..) => keys.push_str(&format!("\t\t\t<integer>{}</integer>\n", v[0])),
            Kind::List => {
                keys.push_str("\t\t\t<array>\n");
                for x in v {
                    keys.push_str(&format!("\t\t\t\t<string>{}</string>\n", esc(x)));
                }
                keys.push_str("\t\t\t</array>\n");
            }
            _ => keys.push_str(&format!("\t\t\t<string>{}</string>\n", esc(v[0]))),
        }
    }
    let domain = super::sources::DOMAIN;
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<!-- A sample configuration profile with every ApexDesk policy: remove
     the ones not wanted, change the values and the identifiers, then
     deploy it with your MDM (made by the apexdesk policy command). -->
<plist version=\"1.0\">
<dict>
\t<key>PayloadContent</key>
\t<array>
\t\t<dict>
\t\t\t<key>PayloadType</key>
\t\t\t<string>{domain}</string>
\t\t\t<key>PayloadIdentifier</key>
\t\t\t<string>com.example.apexdesk.policy.settings</string>
\t\t\t<key>PayloadUUID</key>
\t\t\t<string>6B0E2C56-3F43-4B0B-9D0A-6B6C9E1C2A01</string>
\t\t\t<key>PayloadVersion</key>
\t\t\t<integer>1</integer>
{keys}\t\t</dict>
\t</array>
\t<key>PayloadDisplayName</key>
\t<string>ApexDesk</string>
\t<key>PayloadIdentifier</key>
\t<string>com.example.apexdesk.policy</string>
\t<key>PayloadScope</key>
\t<string>System</string>
\t<key>PayloadType</key>
\t<string>Configuration</string>
\t<key>PayloadUUID</key>
\t<string>6B0E2C56-3F43-4B0B-9D0A-6B6C9E1C2A00</string>
\t<key>PayloadVersion</key>
\t<integer>1</integer>
</dict>
</plist>
"
    )
}

fn linux_sample() -> String {
    let mut s = String::from(
        "# ApexDesk policies for every user of this computer: copy to
# /etc/apexdesk/policy (or a .conf file in /etc/apexdesk/policy.d) and
# keep only the lines wanted. `Name = value`; a list repeats its name.
# Made by `apexdesk policy --templates`.
",
    );
    for i in ITEMS {
        s.push_str(&format!("\n# {}\n", text("en", &format!("policy.{}.help", i.name))));
        if let Kind::Choice(c) = i.kind {
            s.push_str(&format!("# Values: {}\n", c.join(", ")));
        }
        for v in sample(i) {
            s.push_str(&format!("#{} = {v}\n", i.name));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The templates in `packaging/` are the ones the program makes
    /// (`APEXDESK_BLESS=1` rewrites them).
    #[test]
    fn packaging_templates_are_current() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging");
        let bless = std::env::var_os("APEXDESK_BLESS").is_some();
        for (path, text) in files() {
            let p = root.join(&path);
            if bless {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, &text).unwrap();
            }
            assert_eq!(std::fs::read_to_string(&p).unwrap_or_default(), text, "{path} is out of date: run the test with APEXDESK_BLESS=1");
        }
    }

    #[test]
    fn every_policy_is_translated() {
        for i in ITEMS {
            for key in [format!("policy.{}", i.name), format!("policy.{}.help", i.name)] {
                assert!(apex_i18n::langs()[0].get(&key).is_some(), "{key}");
            }
        }
    }

    #[test]
    fn samples_are_valid_policies() {
        // The payload is the dictionary inside PayloadContent.
        let profile = mobileconfig();
        let inner = &profile[profile.find("<array>").unwrap()..];
        let entries = super::super::sources::parse_plist(inner, "p");
        // The profile's own keys are ignored, every policy is read.
        let p = super::super::Policy::from_entries(entries, true);
        assert_eq!(p.set.len(), ITEMS.len());
        let lines: String = linux_sample().lines().filter_map(|l| l.strip_prefix('#')).filter(|l| l.contains(" = ")).map(|l| format!("{l}\n")).collect();
        let p = super::super::Policy::from_entries(super::super::sources::parse_lines(&lines, "f"), true);
        assert_eq!(p.set.len(), ITEMS.len());
    }
}
