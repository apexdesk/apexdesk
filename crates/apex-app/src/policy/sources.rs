//! Where policies come from: the Windows registry, macOS managed
//! preferences, policy files. Each gives `(name, value, source)` entries,
//! the highest precedence first.

use std::path::Path;

type Entries = Vec<(String, String, String)>;

/// The macOS preferences domain (the bundle identifier).
pub const DOMAIN: &str = "com.github.apexdesk.apexdesk";

pub fn read() -> Entries {
    let mut out = Entries::new();
    #[cfg(windows)]
    {
        out.extend(registry::read(registry::HKLM, "HKLM"));
        out.extend(registry::read(registry::HKCU, "HKCU"));
    }
    #[cfg(target_os = "macos")]
    {
        let root = Path::new("/Library/Managed Preferences");
        out.extend(plist_file(&root.join(format!("{DOMAIN}.plist"))));
        // The account's name from the system: $USER is the user's to set
        // (to load nobody's policies, or another's).
        // SAFETY: getpwuid's entry is read at once, before any other call.
        let user = unsafe {
            let pw = libc::getpwuid(libc::getuid());
            (!pw.is_null() && !(*pw).pw_name.is_null()).then(|| std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned())
        };
        if let Some(user) = user.filter(|u| !u.is_empty() && !u.contains('/') && u != "..") {
            out.extend(plist_file(&root.join(user).join(format!("{DOMAIN}.plist"))));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        out.extend(policy_file(Path::new("/etc/apexdesk/policy")));
        if let Ok(rd) = std::fs::read_dir("/etc/apexdesk/policy.d") {
            let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "conf")).collect();
            files.sort();
            for f in files {
                out.extend(policy_file(&f));
            }
        }
    }
    if let Some(p) = std::env::var_os("APEXDESK_POLICY") {
        out.extend(policy_file(Path::new(&p)));
    }
    out
}

fn policy_file(path: &Path) -> Entries {
    let Ok(text) = std::fs::read_to_string(path) else { return Entries::new() };
    parse_lines(&text, &path.display().to_string())
}

/// `Name = value` lines; `#` starts a comment line; a list policy repeats
/// its name once per value.
pub fn parse_lines(text: &str, source: &str) -> Entries {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with(';'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string(), source.to_string()))
        .collect()
}

#[cfg(target_os = "macos")]
fn plist_file(path: &Path) -> Entries {
    let Ok(data) = std::fs::read(path) else { return Entries::new() };
    // Profiles install binary property lists; plutil makes them XML.
    let xml = if data.starts_with(b"bplist") {
        match std::process::Command::new("/usr/bin/plutil").args(["-convert", "xml1", "-o", "-"]).arg(path).output() {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
            _ => return Entries::new(),
        }
    } else {
        String::from_utf8_lossy(&data).into_owned()
    };
    parse_plist(&xml, &path.display().to_string())
}

/// The top-level dictionary of an XML property list: strings, integers,
/// booleans and arrays of strings. Other values are skipped.
#[cfg(any(target_os = "macos", test))]
pub fn parse_plist(xml: &str, source: &str) -> Entries {
    let mut out = Entries::new();
    let mut tags = Tags { s: xml };
    // Down to the first <dict>.
    while let Some(t) = tags.next() {
        if t.0 == "dict" {
            break;
        }
    }
    let mut depth = 0;
    let mut key: Option<String> = None;
    let mut array: Option<(String, Vec<String>)> = None;
    while let Some((tag, text)) = tags.next() {
        match tag.as_str() {
            "key" if depth == 0 && array.is_none() => key = Some(text),
            "string" | "integer" | "real" if depth == 0 => {
                if let Some((_, v)) = &mut array {
                    v.push(text);
                } else if let Some(k) = key.take() {
                    out.push((k, text, source.to_string()));
                }
            }
            "true" | "false" if depth == 0 && array.is_none() => {
                if let Some(k) = key.take() {
                    out.push((k, if tag == "true" { "1" } else { "0" }.to_string(), source.to_string()));
                }
            }
            "array" if depth == 0 => array = key.take().map(|k| (k, Vec::new())),
            "/array" if depth == 0 => {
                if let Some((k, v)) = array.take() {
                    out.extend(v.into_iter().map(|x| (k.clone(), x, source.to_string())));
                }
            }
            "dict" => depth += 1,
            "/dict" if depth == 0 => break,
            "/dict" => depth -= 1,
            _ => {}
        }
    }
    out
}

/// Elements of a simple XML text: `(name, text inside)` for elements with
/// text, `(name, "")` for empty ones and `("/name", "")` for end tags of
/// containers.
#[cfg(any(target_os = "macos", test))]
struct Tags<'a> {
    s: &'a str,
}

#[cfg(any(target_os = "macos", test))]
impl Iterator for Tags<'_> {
    type Item = (String, String);

    fn next(&mut self) -> Option<(String, String)> {
        loop {
            let i = self.s.find('<')?;
            let rest = &self.s[i + 1..];
            let j = rest.find('>')?;
            let inner = &rest[..j];
            self.s = &rest[j + 1..];
            if inner.starts_with('?') || inner.starts_with('!') {
                continue;
            }
            if let Some(name) = inner.strip_prefix('/') {
                return Some((format!("/{}", name.trim()), String::new()));
            }
            let name = inner.trim_end_matches('/').split_whitespace().next().unwrap_or("").to_string();
            if inner.ends_with('/') || matches!(name.as_str(), "dict" | "array" | "plist") {
                return Some((name, String::new()));
            }
            // A text element: up to its end tag.
            let end = format!("</{name}>");
            let k = self.s.find(&end).unwrap_or(self.s.len());
            let text = unescape(self.s[..k].trim());
            self.s = &self.s[(k + end.len()).min(self.s.len())..];
            return Some((name, text));
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

#[cfg(windows)]
pub(crate) mod registry {
    //! `Software\Policies\ApexDesk`: values of the key, and a subkey of
    //! values for each list policy (as the ADMX `list` element writes).

    use super::Entries;

    pub(crate) const HKCU: isize = 0x8000_0001u32 as i32 as isize;
    pub const HKLM: isize = 0x8000_0002u32 as i32 as isize;
    const KEY_READ: u32 = 0x20019;
    const KEY_WOW64_64KEY: u32 = 0x0100;
    const REG_SZ: u32 = 1;
    const REG_EXPAND_SZ: u32 = 2;
    const REG_DWORD: u32 = 4;
    const REG_MULTI_SZ: u32 = 7;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(key: isize, sub: *const u16, options: u32, sam: u32, out: *mut isize) -> i32;
        fn RegEnumValueW(key: isize, index: u32, name: *mut u16, name_len: *mut u32, reserved: *mut u32, kind: *mut u32, data: *mut u8, data_len: *mut u32) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The values of `root\path`: (name, texts).
    pub(crate) fn values(root: isize, path: &str) -> Vec<(String, Vec<String>)> {
        let mut out = Vec::new();
        let mut key = 0isize;
        // SAFETY: the path is NUL-terminated and `key` is closed below.
        if unsafe { RegOpenKeyExW(root, wide(path).as_ptr(), 0, KEY_READ | KEY_WOW64_64KEY, &mut key) } != 0 {
            return out;
        }
        let mut name = vec![0u16; 16384];
        let mut data = vec![0u8; 65536];
        for index in 0.. {
            let (mut name_len, mut data_len, mut kind) = (name.len() as u32, data.len() as u32, 0u32);
            // SAFETY: the buffers are as long as the lengths passed.
            let r = unsafe { RegEnumValueW(key, index, name.as_mut_ptr(), &mut name_len, std::ptr::null_mut(), &mut kind, data.as_mut_ptr(), &mut data_len) };
            if r == 259 {
                break;
            }
            if r != 0 {
                continue;
            }
            let n = String::from_utf16_lossy(&name[..name_len as usize]);
            let d = &data[..data_len as usize];
            let texts = match kind {
                REG_DWORD if d.len() >= 4 => vec![u32::from_le_bytes([d[0], d[1], d[2], d[3]]).to_string()],
                REG_SZ | REG_EXPAND_SZ | REG_MULTI_SZ => {
                    let units: Vec<u16> = d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    String::from_utf16_lossy(&units).split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect()
                }
                _ => continue,
            };
            out.push((n, texts));
        }
        // SAFETY: opened above.
        unsafe { RegCloseKey(key) };
        out
    }

    pub fn read(root: isize, label: &str) -> Entries {
        const PATH: &str = "Software\\Policies\\ApexDesk";
        let source = format!("{label}\\{PATH}");
        let mut out = Entries::new();
        for (name, texts) in values(root, PATH) {
            out.extend(texts.into_iter().map(|t| (name.clone(), t, source.clone())));
        }
        for item in super::super::ITEMS.iter().filter(|i| i.kind == super::super::Kind::List) {
            for (_, texts) in values(root, &format!("{PATH}\\{}", item.name)) {
                out.extend(texts.into_iter().map(|t| (item.name.to_string(), t, source.clone())));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_lines() {
        let e = parse_lines("# ApexDesk\nMacroSecurity = signed\n\nTrustedLocations=/srv/a\nTrustedLocations = /srv/b\nbroken line\n", "f");
        assert_eq!(e.len(), 3);
        assert_eq!(e[0], ("MacroSecurity".into(), "signed".into(), "f".into()));
        assert_eq!(e[2].1, "/srv/b");
    }

    #[test]
    fn property_list() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>MacroSecurity</key>
    <string>disabled</string>
    <key>DisableGetData</key>
    <true/>
    <key>AutosaveMinutes</key>
    <integer>5</integer>
    <key>TrustedLocations</key>
    <array>
        <string>/Volumes/Share &amp; Co</string>
        <string>/Users/Shared/Macros</string>
    </array>
    <key>Nested</key>
    <dict><key>Inner</key><string>x</string></dict>
    <key>Proxy</key>
    <string>http://proxy:3128</string>
</dict>
</plist>"#;
        let e = parse_plist(xml, "p");
        let pairs: Vec<(&str, &str)> = e.iter().map(|(k, v, _)| (k.as_str(), v.as_str())).collect();
        assert_eq!(pairs, [("MacroSecurity", "disabled"), ("DisableGetData", "1"), ("AutosaveMinutes", "5"), ("TrustedLocations", "/Volumes/Share & Co"), ("TrustedLocations", "/Users/Shared/Macros"), ("Proxy", "http://proxy:3128")]);
    }
}
