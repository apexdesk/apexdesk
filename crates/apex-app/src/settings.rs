//! Persistent user settings, stored as `key=value` lines in the platform's
//! config directory. Policies (see `policy`) override some of them; the
//! user's own values of those stay in the file.

use std::path::PathBuf;

const MAX_RECENT: usize = 12;
/// Files remembered as allowed to let their macros change files.
pub const MAX_GRANTS: usize = 500;

static MINI_TOOLBAR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Show the Mini Toolbar on selection.
pub fn mini_toolbar() -> bool {
    MINI_TOOLBAR.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_mini_toolbar(on: bool) {
    MINI_TOOLBAR.store(on, std::sync::atomic::Ordering::Relaxed);
}

static VRULER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static DRAFT_VRULER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Documents show the vertical ruler (with View → Ruler on).
pub fn vruler() -> bool {
    VRULER.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_vruler(on: bool) {
    VRULER.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Documents show the vertical ruler in the draft view too.
pub fn draft_vruler() -> bool {
    DRAFT_VRULER.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_draft_vruler(on: bool) {
    DRAFT_VRULER.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The range of `Settings::scroll_speed`.
pub const SCROLL_SPEED: (f32, f32) = (0.5, 5.0);

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Interface language code; empty means the base language.
    pub lang: String,
    /// `None` follows the system theme.
    pub dark: Option<bool>,
    pub recent: Vec<PathBuf>,
    pub show_toolbar: bool,
    /// Menus and a toolbar instead of the toolstrip.
    pub classic: bool,
    /// On a phone, its own interface (else the whole one, as on a tablet).
    pub phone_ui: bool,
    pub show_formula_bar: bool,
    pub recent_colors: Vec<u32>,
    pub recent_fonts: Vec<String>,
    /// Insert Symbol dialog, most recent first.
    pub recent_symbols: Vec<char>,
    /// Status bar statistics of the spreadsheet (bit mask).
    pub status_stats: u8,
    /// Spelling dictionary ("es_ES"); empty picks one from the locale.
    pub spell_lang: String,
    /// Check spelling as you type.
    pub spell_auto: bool,
    /// The Mini Toolbar shows on selection.
    pub mini_toolbar: bool,
    /// The Developer tab shows (macros, form controls), off at first as
    /// in Office.
    pub developer_tab: bool,
    /// With the dark theme, pages, cells and slides are drawn dark too
    /// (only on screen).
    pub dark_canvas: bool,
    /// The interface text a little brighter (dark theme) or darker (light).
    pub contrast: bool,
    /// Glyphs on whole pixels: sharper text, slightly less even spacing.
    pub sharp_text: bool,
    /// The vertical ruler shows in the draft view of documents too.
    pub vruler: bool,
    pub draft_vruler: bool,
    /// The mouse wheel and touchpad scroll this many times as far (0.5–5).
    pub scroll_speed: f32,
    /// The first start asked whether to open office files with ApexDesk.
    pub default_asked: bool,
    /// The window was maximized when the program last closed.
    pub window_maximized: bool,
    /// The normal (not maximized) window size when it last closed.
    pub window_size: Option<[f32; 2]>,
    /// AI assistants may read and edit the open files (`apexdesk mcp`).
    pub mcp: bool,
    /// Scripts may drive ApexDesk through `apexdesk api` (business license).
    pub automation_api: bool,
    /// The terminal interface (`apexdesk tui`) shows its menu bar.
    pub tui_menu_bar: bool,
    /// Graphics back end: "" (automatic: OpenGL, then wgpu), "glow" or "wgpu".
    pub renderer: String,
    /// Draw with the high-performance graphics card, not the integrated one.
    pub gpu_high: bool,
    /// Author of comments and tracked changes; empty uses the login name.
    pub user_name: String,
    /// Custom lists for fills and sorts (File > Options).
    pub custom_lists: Vec<Vec<String>>,
    /// Minutes between recovery copies of changed files (0: off).
    pub autosave_minutes: u32,
    /// Write lock files (Office and LibreOffice style) next to open files
    /// and warn about others'.
    pub lock_files: bool,
    /// Read Aloud: the voice (empty: one for the text's language) and the
    /// speed (1.0 normal).
    pub speech_voice: String,
    pub speech_rate: f32,
    /// VBA macros may run (always asking first, except Workbook_Open of
    /// files in a trusted location). Off by default.
    pub macros_enabled: bool,
    /// Folders whose files' macros run without asking (when enabled).
    pub trusted_locations: Vec<PathBuf>,
    /// Workbooks whose macros may change files without asking: hashes of
    /// the file's path and its macros' code (other code, other file:
    /// asked again).
    pub macro_file_grants: Vec<String>,
    /// Installed add-ins (`.xlam`, `.xlsm`): their macros are reachable
    /// from Run Macro and `Application.Run`.
    pub addins: Vec<PathBuf>,
    /// Macros only run when their project is signed by a trusted
    /// certificate.
    pub macros_signed_only: bool,
    /// Signers whose macros run under `macros_signed_only` even without a
    /// chain to a trusted root: their certificates' SHA-256 (or SHA-1).
    pub trusted_publishers: Vec<String>,
    /// The macro editor can't be opened (an organization's policy).
    pub macro_editor_off: bool,
    /// The macro editor: new modules start with `Option Explicit`
    /// (Require Variable Declaration), and the code's size.
    pub vba_explicit: bool,
    pub vba_font_size: f32,
    /// The macro editor in a window of its own (not inside the program's).
    pub vba_window: bool,
    /// Syntax colors chosen in the macro editor (`keyword=#RRGGBB;...`).
    pub vba_colors: String,
    /// Files downloaded from the Internet open in Protected View
    /// (read-only, macros off) until the user enables editing.
    pub protected_view: bool,
    /// A shared folder of templates, listed next to the user's own.
    pub templates_folder: String,
    /// A shared folder of building blocks (.docx/.dotx glossaries).
    pub building_blocks_folder: String,
    /// A shared folder of pictures (Insert > Library), its subfolders
    /// categories.
    pub library_folder: String,
    /// New from Template lists only the shared templates.
    pub only_shared_templates: bool,
    /// The format Save As offers first for new files ("": xlsx, docx).
    pub save_sheet_format: String,
    pub save_doc_format: String,
    /// A shared folder of Hunspell dictionaries.
    pub dictionaries_folder: String,
    /// Get Data may not read files, the web or databases.
    pub get_data_off: bool,
    /// The proxy for web requests: "" the system's, "direct", or a URL.
    pub proxy: String,
    /// Hosts reached without the proxy (commas); added to the system's.
    pub proxy_bypass: String,
    /// Hosts that get the session's sign-in (Kerberos or NTLM).
    pub sign_in_sites: String,
    /// Where crash reports are mailed to (the user sends them).
    pub support_email: String,
    /// Font replacements, "Name=Other; Name2=Other2".
    pub font_substitutes: String,
    /// A folder of the organization's root certificates (.cer, .crt, .pem).
    pub cert_roots: String,
    /// Signing PDFs: the last certificate file (never its password) and
    /// the time-stamp authority (empty: no time stamp).
    pub sign_cert: String,
    pub tsa_url: String,
    /// The long-term level signatures get: "t", "lt" or "lta".
    pub sign_level: String,
    /// Properties a file must have before it is saved (policy only).
    pub required_properties: Vec<String>,
    /// The check before exporting or sending a file: "" (none), "warn"
    /// or "block" (policy only).
    pub share_check: String,
    /// Links in documents to files and other programs: "ask", "allow" or "block".
    pub link_security: String,
    /// Where computers record the license seats they use (policy only).
    pub seats_folder: String,
    /// The organization's fonts, colors and forbidden terms (policy only).
    pub brand_fonts: Vec<String>,
    pub brand_colors: Vec<u32>,
    pub forbidden_terms: Vec<String>,
    /// Where the last signing certificate came from ("file", "store",
    /// "token") and, for the store and tokens, its SHA-1; the PKCS #11
    /// module of cards and tokens. Never a PIN.
    pub sign_source: String,
    pub sign_key: String,
    pub pkcs11_module: String,
    /// Commands on the Quick Access Toolbar.
    pub qat: Vec<Command>,
    /// Shortcuts the user gave commands.
    pub keybinds: Vec<(Command, String)>,
}

/// A command of the toolstrip or the File menu, for the Quick Access Toolbar
/// and custom shortcuts.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    /// Of documents (else of spreadsheets).
    pub doc: bool,
    /// Toolstrip tab id ("file": the File menu).
    pub tab: String,
    /// Its place and name (see `toolstrip::Entry::key`).
    pub key: String,
    pub label: String,
    /// Icon name.
    pub icon: String,
    /// Opens a menu.
    pub menu: bool,
}

impl Command {
    fn encode(&self) -> Vec<String> {
        vec![if self.doc { "doc" } else { "sheet" }.to_string(), self.tab.clone(), self.key.clone(), self.label.clone(), self.icon.clone(), (self.menu as u8).to_string()]
    }

    fn decode(v: &[String]) -> Option<Command> {
        let [doc, tab, key, label, icon, menu] = v else { return None };
        Some(Command { doc: doc == "doc", tab: tab.clone(), key: key.clone(), label: label.clone(), icon: icon.clone(), menu: menu == "1" })
    }
}

/// Tab-separated fields; empty fields kept.
fn encode_fields(v: &[String]) -> String {
    v.iter().map(|s| s.replace(['\t', '\n', '\r'], " ")).collect::<Vec<_>>().join("\t")
}

fn decode_fields(line: &str) -> Vec<String> {
    line.split('\t').map(str::to_string).collect()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            lang: String::new(),
            dark: None,
            recent: Vec::new(),
            show_toolbar: true,
            classic: false,
            phone_ui: true,
            show_formula_bar: true,
            recent_colors: Vec::new(),
            recent_fonts: Vec::new(),
            recent_symbols: Vec::new(),
            status_stats: 0b000111,
            spell_lang: String::new(),
            spell_auto: true,
            mini_toolbar: true,
            developer_tab: false,
            dark_canvas: false,
            contrast: false,
            sharp_text: false,
            vruler: true,
            draft_vruler: false,
            scroll_speed: 1.0,
            default_asked: false,
            window_maximized: false,
            window_size: None,
            mcp: false,
            automation_api: false,
            tui_menu_bar: true,
            renderer: String::new(),
            gpu_high: false,
            user_name: String::new(),
            custom_lists: Vec::new(),
            autosave_minutes: 10,
            lock_files: true,
            speech_voice: String::new(),
            speech_rate: 1.0,
            macros_enabled: false,
            trusted_locations: Vec::new(),
            macro_file_grants: Vec::new(),
            addins: Vec::new(),
            macros_signed_only: false,
            trusted_publishers: Vec::new(),
            macro_editor_off: false,
            vba_explicit: false,
            vba_font_size: 13.0,
            vba_window: false,
            vba_colors: String::new(),
            protected_view: true,
            templates_folder: String::new(),
            building_blocks_folder: String::new(),
            library_folder: String::new(),
            only_shared_templates: false,
            save_sheet_format: String::new(),
            save_doc_format: String::new(),
            dictionaries_folder: String::new(),
            get_data_off: false,
            proxy: String::new(),
            proxy_bypass: String::new(),
            sign_in_sites: String::new(),
            support_email: String::new(),
            font_substitutes: String::new(),
            cert_roots: String::new(),
            sign_cert: String::new(),
            tsa_url: String::new(),
            sign_level: String::new(),
            required_properties: Vec::new(),
            share_check: String::new(),
            link_security: String::new(),
            seats_folder: String::new(),
            brand_fonts: Vec::new(),
            brand_colors: Vec::new(),
            forbidden_terms: Vec::new(),
            sign_source: String::new(),
            sign_key: String::new(),
            pkcs11_module: String::new(),
            qat: Vec::new(),
            keybinds: Vec::new(),
        }
    }
}

/// Creates `dir` (and its parents) and makes it this user's only: it
/// holds the proxy's password and copies of the documents being edited.
pub(crate) fn make_private_dir(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
}

pub(crate) fn config_dir() -> Option<PathBuf> {
    // Tests never read or write the user's profile.
    if cfg!(test) {
        // Those of earlier runs are left behind (tests have no end to
        // clean up at): removed once their process is gone.
        static SWEPT: std::sync::Once = std::sync::Once::new();
        SWEPT.call_once(|| {
            let Ok(dir) = std::fs::read_dir(std::env::temp_dir()) else { return };
            for e in dir.flatten() {
                let name = e.file_name();
                let Some(pid) = name.to_str().and_then(|n| n.strip_prefix("apexdesk-test-")).and_then(|p| p.parse::<u32>().ok()) else { continue };
                if cfg!(target_os = "linux") && pid != std::process::id() && !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        });
        return Some(std::env::temp_dir().join(format!("apexdesk-test-{}", std::process::id())));
    }
    if let Some(d) = crate::portable::data_dir() {
        return Some(d);
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join("apexdesk"))
}

/// The program's earlier names, newest first.
pub(crate) const OLD_NAMES: [&str; 2] = ["demosoffice", "gensworkoffice"];

/// Moves the folder of an earlier name of the program to the current
/// one, once: settings, license, recovery and dictionaries.
pub(crate) fn adopt_old_folder() {
    if cfg!(test) || crate::portable::data_dir().is_some() {
        return;
    }
    let Some(new) = config_dir() else { return };
    if new.exists() {
        return;
    }
    let Some(old_name) = OLD_NAMES.into_iter().find(|n| new.with_file_name(n).is_dir()) else { return };
    if std::fs::rename(new.with_file_name(old_name), &new).is_err() {
        return;
    }
    for name in ["lic", "log"] {
        let _ = std::fs::rename(new.join(format!("{old_name}.{name}")), new.join(format!("apexdesk.{name}")));
    }
}

impl Settings {
    /// The user's settings with the policies in force applied.
    pub fn load() -> Settings {
        let mut s = Settings::load_user();
        s.apply_policy(crate::policy::current());
        s
    }

    /// The settings as the user left them.
    pub fn load_user() -> Settings {
        let mut s = Settings::default();
        let Some(text) = config_dir().and_then(|d| std::fs::read_to_string(d.join("settings.txt")).ok()) else { return s };
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                s.set(k.trim(), v);
            }
        }
        s
    }

    /// Overrides what the policies fix.
    pub fn apply_policy(&mut self, p: &crate::policy::Policy) {
        for set in &p.set {
            match set.item.key {
                "trusted_location" => self.trusted_locations.clear(),
                "trusted_publisher" => self.trusted_publishers.clear(),
                "addin" => self.addins.clear(),
                "required_property" => self.required_properties.clear(),
                "brand_font" => self.brand_fonts.clear(),
                "brand_color" => self.brand_colors.clear(),
                "forbidden_term" => self.forbidden_terms.clear(),
                _ => {}
            }
            for v in &set.values {
                self.set(set.item.key, v);
            }
        }
    }

    /// Applies one `key=value` line (`macros` stands for the macro mode a
    /// policy sets: disabled, ask or signed).
    fn set(&mut self, k: &str, v: &str) {
        let s = self;
        {
            match k {
                "macros" => {
                    s.macros_enabled = v.trim() != "disabled";
                    s.macros_signed_only = v.trim() == "signed";
                }
                "macros_signed_only" => s.macros_signed_only = v.trim() == "1",
                "protected_view" => s.protected_view = v.trim() != "0",
                "templates_folder" => s.templates_folder = v.trim().to_string(),
                "building_blocks_folder" => s.building_blocks_folder = v.trim().to_string(),
                "library_folder" => s.library_folder = v.trim().to_string(),
                "only_shared_templates" => s.only_shared_templates = v.trim() == "1",
                "save_sheet_format" => s.save_sheet_format = v.trim().to_string(),
                "save_doc_format" => s.save_doc_format = v.trim().to_string(),
                "dictionaries_folder" => s.dictionaries_folder = v.trim().to_string(),
                "get_data_off" => s.get_data_off = v.trim() == "1",
                "proxy" => s.proxy = v.trim().to_string(),
                "proxy_bypass" => s.proxy_bypass = v.trim().to_string(),
                "sign_in_sites" => s.sign_in_sites = v.trim().to_string(),
                "support_email" => s.support_email = v.trim().to_string(),
                "font_substitutes" => s.font_substitutes = v.trim().to_string(),
                "cert_roots" => s.cert_roots = v.trim().to_string(),
                "lang" => s.lang = v.trim().to_string(),
                "theme" => {
                    s.dark = match v.trim() {
                        "dark" => Some(true),
                        "light" => Some(false),
                        _ => None,
                    }
                }
                "recent" => s.recent.push(PathBuf::from(v)),
                "toolbar" => s.show_toolbar = v.trim() != "0",
                "interface" => s.classic = v.trim() == "classic",
                "phone_ui" => s.phone_ui = v.trim() != "0",
                "formula_bar" => s.show_formula_bar = v.trim() != "0",
                "recent_colors" => s.recent_colors = v.split(',').filter_map(|c| u32::from_str_radix(c.trim(), 16).ok()).collect(),
                "recent_symbols" => s.recent_symbols = v.split(',').filter_map(|c| u32::from_str_radix(c.trim(), 16).ok().and_then(char::from_u32)).collect(),
                "recent_font" => s.recent_fonts.push(v.trim().to_string()),
                "spell_lang" => s.spell_lang = v.trim().to_string(),
                "spell_auto" => s.spell_auto = v.trim() != "0",
                "mini_toolbar" => s.mini_toolbar = v.trim() != "0",
                "developer_tab" => s.developer_tab = v.trim() == "1",
                "dark_canvas" => s.dark_canvas = v.trim() == "1",
                "contrast" => s.contrast = v.trim() == "1",
                "sharp_text" => s.sharp_text = v.trim() == "1",
                "vruler" => s.vruler = v.trim() == "1",
                "draft_vruler" => s.draft_vruler = v.trim() == "1",
                "scroll_speed" => s.scroll_speed = v.trim().parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(1.0).clamp(SCROLL_SPEED.0, SCROLL_SPEED.1),
                "default_asked" => s.default_asked = v.trim() == "1",
                "window_maximized" => s.window_maximized = v.trim() == "1",
                "window_size" => {
                    s.window_size = v.split_once('x').and_then(|(w, h)| Some([w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?])).filter(|z| z.iter().all(|n| n.is_finite()))
                }
                "mcp" => s.mcp = v.trim() == "1",
                "automation_api" => s.automation_api = v.trim() == "1",
                "tui_menu_bar" => s.tui_menu_bar = v.trim() != "0",
                "renderer" => s.renderer = v.trim().to_string(),
                "gpu" => s.gpu_high = v.trim() == "high",
                "user_name" => s.user_name = v.trim().to_string(),
                "autosave_minutes" => s.autosave_minutes = v.trim().parse().unwrap_or(10),
                "lock_files" => s.lock_files = v.trim() != "0",
                "vba_explicit" => s.vba_explicit = v.trim() == "1",
                "vba_window" => s.vba_window = v.trim() == "1",
                "vba_colors" => s.vba_colors = v.trim().chars().take(200).collect(),
                "vba_font_size" => s.vba_font_size = v.trim().parse::<f32>().ok().filter(|f| (8.0..=32.0).contains(f)).unwrap_or(13.0),
                "speech_voice" => s.speech_voice = v.trim().to_string(),
                "speech_rate" => s.speech_rate = v.trim().parse::<f32>().unwrap_or(1.0).clamp(0.5, 2.0),
                "sign_cert" => s.sign_cert = v.trim().to_string(),
                "tsa_url" => s.tsa_url = v.trim().to_string(),
                "sign_level" => s.sign_level = v.trim().to_string(),
                "required_property" => s.required_properties.extend(v.split(';').map(str::trim).filter(|n| !n.is_empty()).map(String::from)),
                "share_check" => s.share_check = v.trim().to_ascii_lowercase(),
                "link_security" => s.link_security = v.trim().to_ascii_lowercase(),
                "seats_folder" => s.seats_folder = v.trim().to_string(),
                "brand_font" => s.brand_fonts.extend(v.split(';').map(str::trim).filter(|n| !n.is_empty()).map(String::from)),
                "brand_color" => s.brand_colors.extend(v.split([';', ',']).filter_map(crate::brand::parse_color)),
                "forbidden_term" => s.forbidden_terms.extend(v.split(';').map(str::trim).filter(|n| !n.is_empty()).map(String::from)),
                "sign_source" => s.sign_source = v.trim().to_string(),
                "sign_key" => s.sign_key = v.trim().to_string(),
                "pkcs11_module" => s.pkcs11_module = v.trim().to_string(),
                "status_stats" => s.status_stats = v.trim().parse().unwrap_or(0b000111),
                "macros_enabled" => s.macros_enabled = v.trim() == "1",
                "trusted_location" => s.trusted_locations.push(PathBuf::from(v)),
                "macro_file_grant" if s.macro_file_grants.len() < MAX_GRANTS && v.trim().len() == 64 && v.trim().bytes().all(|b| b.is_ascii_hexdigit()) => s.macro_file_grants.push(v.trim().to_ascii_lowercase()),
                "trusted_publisher" if !v.trim().is_empty() => s.trusted_publishers.push(v.trim().to_string()),
                "macro_editor_off" => s.macro_editor_off = v.trim() == "1",
                "addin" => s.addins.push(PathBuf::from(v)),
                "qat" => s.qat.extend(Command::decode(&decode_fields(v))),
                "keybind" => {
                    let mut f = decode_fields(v);
                    if let Some(keys) = f.pop()
                        && let Some(c) = Command::decode(&f)
                    {
                        s.keybinds.push((c, keys));
                    }
                }
                "custom_list" => {
                    let l = crate::app::decode_list(v);
                    if !l.is_empty() {
                        s.custom_lists.push(l);
                    }
                }
                _ => {}
            }
        }
    }

    /// Settings everything reads through globals: the shared templates and
    /// dictionaries, Get Data, font substitutes, the organization's roots.
    pub fn apply_globals(&self) {
        crate::templates::set_shared_dir(Some(self.templates_folder.trim()).filter(|d| !d.is_empty()).map(PathBuf::from));
        crate::blocks::set_shared_dir(Some(self.building_blocks_folder.trim()).filter(|d| !d.is_empty()).map(PathBuf::from));
        crate::library::set_dir(Some(self.library_folder.trim()).filter(|d| !d.is_empty()).map(PathBuf::from));
        crate::seats::set_dir(Some(self.seats_folder.trim()).filter(|d| !d.is_empty()).map(PathBuf::from));
        crate::templates::set_only_shared(self.only_shared_templates);
        crate::brand::set(crate::brand::Brand { fonts: self.brand_fonts.clone(), colors: self.brand_colors.clone(), terms: self.forbidden_terms.clone() });
        // Dictionaries an installer added (the program folder is searched
        // already; macOS keeps them in /Library/Application Support).
        for d in crate::portable::install_dirs().into_iter().skip(1) {
            if d.join("dictionaries").is_dir() {
                apex_spell::add_search_dir(d.join("dictionaries"));
            }
        }
        if !self.dictionaries_folder.trim().is_empty() {
            apex_spell::add_search_dir(PathBuf::from(self.dictionaries_folder.trim()));
        }
        apex_query::set_disabled(self.get_data_off);
        crate::links::set_mode(&self.link_security);
        crate::fonts::set_substitutes(&self.font_substitutes);
        crate::net::apply(self);
        if !self.cert_roots.trim().is_empty() {
            load_roots(std::path::Path::new(self.cert_roots.trim()));
        }
    }

    pub fn save(&self) {
        let Some(dir) = config_dir() else { return };
        make_private_dir(&dir);
        let mut out = format!("lang={}\ntheme={}\n", self.lang.replace('\n', " "), match self.dark {
            Some(true) => "dark",
            Some(false) => "light",
            None => "system",
        });
        out.push_str(&format!("interface={}\nphone_ui={}\n", if self.classic { "classic" } else { "toolstrip" }, self.phone_ui as u8));
        out.push_str(&format!("toolbar={}\nformula_bar={}\nstatus_stats={}\n", self.show_toolbar as u8, self.show_formula_bar as u8, self.status_stats));
        out.push_str(&format!("spell_lang={}\nspell_auto={}\n", self.spell_lang.replace('\n', " "), self.spell_auto as u8));
        out.push_str(&format!("user_name={}\n", self.user_name.replace('\n', " ")));
        out.push_str(&format!("autosave_minutes={}\n", self.autosave_minutes));
        out.push_str(&format!("vba_explicit={}\nvba_font_size={}\nvba_window={}\nvba_colors={}\n", self.vba_explicit as u8, self.vba_font_size, self.vba_window as u8, self.vba_colors));
        out.push_str(&format!("lock_files={}\nspeech_voice={}\nspeech_rate={}\n", self.lock_files as u8, self.speech_voice.replace('\n', " "), self.speech_rate));
        out.push_str(&format!("mini_toolbar={}\n", self.mini_toolbar as u8));
        out.push_str(&format!("developer_tab={}\n", self.developer_tab as u8));
        out.push_str(&format!("dark_canvas={}\n", self.dark_canvas as u8));
        out.push_str(&format!("contrast={}\n", self.contrast as u8));
        out.push_str(&format!("sharp_text={}\n", self.sharp_text as u8));
        out.push_str(&format!("vruler={}\n", self.vruler as u8));
        out.push_str(&format!("draft_vruler={}\n", self.draft_vruler as u8));
        out.push_str(&format!("scroll_speed={}\n", self.scroll_speed));
        out.push_str(&format!("default_asked={}\n", self.default_asked as u8));
        out.push_str(&format!("window_maximized={}\n", self.window_maximized as u8));
        if let Some([w, h]) = self.window_size {
            out.push_str(&format!("window_size={w:.0}x{h:.0}\n"));
        }
        out.push_str(&format!("mcp={}\n", self.mcp as u8));
        out.push_str(&format!("automation_api={}\n", self.automation_api as u8));
        out.push_str(&format!("protected_view={}\n", self.protected_view as u8));
        out.push_str(&format!("tui_menu_bar={}\n", self.tui_menu_bar as u8));
        out.push_str(&format!("gpu={}\n", if self.gpu_high { "high" } else { "low" }));
        if !self.renderer.is_empty() {
            out.push_str(&format!("renderer={}\n", self.renderer.replace('\n', " ")));
        }
        out.push_str(&format!("macros_enabled={}\n", self.macros_enabled as u8));
        // A line break in a path (a file's name may have one) would start
        // a setting of its own: such paths aren't kept.
        for p in self.trusted_locations.iter().map(|p| p.to_string_lossy()).filter(|p| !p.contains('\n')) {
            out.push_str(&format!("trusted_location={p}\n"));
        }
        for g in &self.macro_file_grants {
            out.push_str(&format!("macro_file_grant={g}\n"));
        }
        for p in self.trusted_publishers.iter().filter(|p| !p.contains('\n')) {
            out.push_str(&format!("trusted_publisher={p}\n"));
        }
        if self.macro_editor_off {
            out.push_str("macro_editor_off=1\n");
        }
        for p in self.addins.iter().map(|p| p.to_string_lossy()).filter(|p| !p.contains('\n')) {
            out.push_str(&format!("addin={p}\n"));
        }
        out.push_str(&format!("sign_cert={}\ntsa_url={}\nsign_level={}\n", self.sign_cert.replace('\n', " "), self.tsa_url.replace('\n', " "), self.sign_level.replace('\n', " ")));
        out.push_str(&format!("sign_source={}\nsign_key={}\npkcs11_module={}\n", self.sign_source.replace('\n', " "), self.sign_key.replace('\n', " "), self.pkcs11_module.replace('\n', " ")));
        let colors: Vec<String> = self.recent_colors.iter().map(|c| format!("{c:06X}")).collect();
        out.push_str(&format!("recent_colors={}\n", colors.join(",")));
        let symbols: Vec<String> = self.recent_symbols.iter().map(|&c| format!("{:04X}", c as u32)).collect();
        out.push_str(&format!("recent_symbols={}\n", symbols.join(",")));
        for f in self.recent_fonts.iter().filter(|f| !f.contains('\n')) {
            out.push_str("recent_font=");
            out.push_str(f);
            out.push('\n');
        }
        for l in &self.custom_lists {
            out.push_str("custom_list=");
            out.push_str(&crate::app::encode_list(l));
            out.push('\n');
        }
        for c in &self.qat {
            out.push_str(&format!("qat={}\n", encode_fields(&c.encode())));
        }
        for (c, keys) in &self.keybinds {
            let mut f = c.encode();
            f.push(keys.clone());
            out.push_str(&format!("keybind={}\n", encode_fields(&f)));
        }
        for r in self.recent.iter().map(|p| p.to_string_lossy()).filter(|p| !p.contains('\n')) {
            out.push_str("recent=");
            out.push_str(&r);
            out.push('\n');
        }
        out.push_str(&format!("macros_signed_only={}\ntemplates_folder={}\n", self.macros_signed_only as u8, self.templates_folder.replace('\n', " ")));
        out.push_str(&format!("building_blocks_folder={}\n", self.building_blocks_folder.replace('\n', " ")));
        out.push_str(&format!("library_folder={}\nonly_shared_templates={}\n", self.library_folder.replace('\n', " "), self.only_shared_templates as u8));
        out.push_str(&format!("save_sheet_format={}\nsave_doc_format={}\n", self.save_sheet_format.replace('\n', " "), self.save_doc_format.replace('\n', " ")));

        out.push_str(&format!("dictionaries_folder={}\nget_data_off={}\n", self.dictionaries_folder.replace('\n', " "), self.get_data_off as u8));
        out.push_str(&format!("proxy={}\nfont_substitutes={}\n", self.proxy.replace('\n', " "), self.font_substitutes.replace('\n', " ")));
        out.push_str(&format!("proxy_bypass={}\nsign_in_sites={}\n", self.proxy_bypass.replace('\n', " "), self.sign_in_sites.replace('\n', " ")));
        out.push_str(&format!("support_email={}\n", self.support_email.trim().replace('\n', " ")));
        out.push_str(&format!("cert_roots={}\n", self.cert_roots.replace('\n', " ")));

        let out = keep_user_values(&out, std::fs::read_to_string(dir.join("settings.txt")).unwrap_or_default().as_str(), crate::policy::current());
        let _ = std::fs::write(dir.join("settings.txt"), out);
    }

    pub fn add_recent(&mut self, p: PathBuf) {
        let p = p.canonicalize().unwrap_or(p);
        self.recent.retain(|x| *x != p);
        self.recent.insert(0, p);
        self.recent.truncate(MAX_RECENT);
    }
}

/// The settings text to write: the lines a policy fixes are the user's
/// own ones from the file (`old`), not the policy's values.
fn keep_user_values(new: &str, old: &str, p: &crate::policy::Policy) -> String {
    if p.set.is_empty() {
        return new.to_string();
    }
    let locked = |line: &str| line.split_once('=').is_some_and(|(k, _)| p.locks_key(k.trim()));
    let mut out: String = new.lines().filter(|l| !locked(l)).map(|l| format!("{l}\n")).collect();
    for l in old.lines().filter(|l| locked(l)) {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// The organization's root certificates, once per process.
fn load_roots(dir: &std::path::Path) {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        apex_sign::trust::add_roots_from(dir);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_state_parses() {
        let mut s = Settings::default();
        s.set("window_maximized", "1");
        s.set("window_size", "1100x700");
        assert!(s.window_maximized);
        assert_eq!(s.window_size, Some([1100.0, 700.0]));
        s.set("window_size", "junk");
        assert_eq!(s.window_size, None);
    }

    #[test]
    fn macro_file_grants() {
        let mut s = Settings::default();
        s.set("macro_file_grant", &"AB".repeat(32));
        s.set("macro_file_grant", "short");
        s.set("macro_file_grant", &"zz".repeat(32));
        assert_eq!(s.macro_file_grants, ["ab".repeat(32)]);
    }

    #[test]
    fn scroll_speed_stays_in_range() {
        let mut s = Settings::default();
        assert_eq!(s.scroll_speed, 1.0);
        s.set("scroll_speed", "2.5");
        assert_eq!(s.scroll_speed, 2.5);
        s.set("scroll_speed", "40");
        assert_eq!(s.scroll_speed, 5.0);
        s.set("scroll_speed", "0.1");
        assert_eq!(s.scroll_speed, 0.5);
        s.set("scroll_speed", "NaN");
        assert_eq!(s.scroll_speed, 1.0);
    }

    #[test]
    fn policies_override_and_leave_user_values() {
        let p = crate::policy::Policy::from_entries(vec![("MacroSecurity".into(), "signed".into(), "t".into()), ("TrustedLocations".into(), "/srv/m".into(), "t".into()), ("AutosaveMinutes".into(), "3".into(), "t".into())], true);
        let mut s = Settings::default();
        s.set("macros_enabled", "0");
        s.set("trusted_location", "/home/me/m");
        s.apply_policy(&p);
        assert!(s.macros_enabled && s.macros_signed_only);
        assert_eq!(s.trusted_locations, [PathBuf::from("/srv/m")]);
        assert_eq!(s.autosave_minutes, 3);
        let new = "lang=es\nmacros_enabled=1\nmacros_signed_only=1\ntrusted_location=/srv/m\nautosave_minutes=3\n";
        let old = "lang=en\nmacros_enabled=0\ntrusted_location=/home/me/m\nautosave_minutes=10\n";
        assert_eq!(keep_user_values(new, old, &p), "lang=es\nmacros_enabled=0\ntrusted_location=/home/me/m\nautosave_minutes=10\n");
    }

    #[test]
    fn trusted_publishers_are_kept_or_fixed() {
        let mut s = Settings::default();
        s.set("trusted_publisher", "AA:BB Mine");
        s.set("trusted_publisher", " ");
        s.set("macro_editor_off", "1");
        assert_eq!(s.trusted_publishers, ["AA:BB Mine"]);
        assert!(s.macro_editor_off);
        let p = crate::policy::Policy::from_entries(vec![("TrustedPublishers".into(), "CC:DD".into(), "t".into())], true);
        s.apply_policy(&p);
        assert_eq!(s.trusted_publishers, ["CC:DD"]);
    }
}

