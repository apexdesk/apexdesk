//! ApexDesk: a fast, lightweight spreadsheet and word processor.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// Helpers only ApexDesk Empresa uses (checks, brands) stay in the free build.
#![cfg_attr(not(feature = "empresa"), allow(dead_code))]

mod app;
mod autocorrect;
mod barcode;
mod pii;
mod brand;
#[cfg_attr(not(feature = "empresa"), path = "seats_free.rs")]
mod seats;
mod blocks;
mod charts;
mod chart_picture;
mod mail;
mod mcp;
mod merge_data;
mod cli;
mod clip_history;
mod common;
mod diag;
mod doc_view;
mod file_dialog;
mod fonts;
mod cjk_fonts;
mod formula_help;
mod headless;
mod i18n;
mod icons;
mod images;
mod ink;
mod instance;
mod labels_ui;
mod library;
mod links;
#[cfg_attr(not(feature = "empresa"), path = "license_free.rs")]
mod license;
mod lockfile;
mod media;
mod net;
mod portable;
mod objects;
mod imposition;
mod pdf_out;
mod pdf_view;
mod painter;
mod opendoc;
mod palette;
mod policy;
mod print;
mod read_aloud;
mod toolstrip;
mod db_view;
mod note_view;
mod script;
mod settings;
mod selection_pane;
mod shapes;
mod gradient;
mod gradient_ui;
mod text_fx;
mod text_fx_ui;
mod slides_view;
mod vtext;
mod speech;
mod sheet_view;
mod spell;
mod thesaurus;
mod symbols;
mod sysclip;
mod templates;
mod theme;
mod themes_ui;
mod widgets;

use std::path::PathBuf;

use eframe::egui;

/// On Windows, libraries asked for by name (Vulkan, EGL...) are looked for
/// in System32, the program's folder and the ones it adds, never in the
/// working folder: opening a document from a folder sets it there, and a
/// DLL may have come in the same ZIP.
fn safe_library_search() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetDefaultDllDirectories(flags: u32) -> i32;
            fn SetDllDirectoryW(path: *const u16) -> i32;
        }
        // LOAD_LIBRARY_SEARCH_APPLICATION_DIR | USER_DIRS | SYSTEM32.
        const DIRS: u32 = 0x200 | 0x400 | 0x800;
        // SAFETY: plain Win32 calls; the empty string is NUL-terminated.
        unsafe {
            SetDefaultDllDirectories(DIRS);
            SetDllDirectoryW([0u16].as_ptr());
        }
    }
}

fn main() -> eframe::Result<()> {
    let started = std::time::Instant::now();
    safe_library_search();
    system_locale();
    settings::adopt_old_folder();
    // Cropped pictures written to web pages, e-books and Markdown are cut.
    apex_io::picture::set_decoder(|b| images::decode(b).map(|d| (d.width, d.height, d.rgba.to_vec())));
    // Commands (`apexdesk convert ...`, `apexdesk tui`) run without a window.
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }
    // Crash reports stay on this computer (File > Diagnostics).
    diag::install();
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    // Scripted runs stay independent of any open window.
    let scripted = std::env::var_os("APEXDESK_SCRIPT").is_some();
    // A tab taken to a new window opens there, not back in this one.
    if !scripted && std::env::var_os(instance::NEW_WINDOW).is_none() && instance::forward(&files) {
        return Ok(());
    }
    // Scripted runs may ask for a window size ("1920x1080"), to measure
    // big windows.
    let size = std::env::var("APEXDESK_SIZE").ok().filter(|_| scripted).and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some([w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?])
    });
    // Documents the Finder opens (a double-clicked document) reach the
    // application, not the command line: before the window starts.
    opendoc::install();
    let prefs = settings::Settings::load();
    let renderer = renderer(&prefs);
    if prefs.gpu_high {
        high_performance_gpu();
    }
    let result = eframe::run_native("ApexDesk", options(size, renderer, &prefs, scripted), Box::new(move |cc| {
        let incoming = if scripted { None } else { instance::listen(cc.egui_ctx.clone()) };
        Ok(Box::new(app::App::new(cc, files, incoming, started)))
    }));
    instance::cleanup();
    mcp::link::cleanup();
    // The back end could not start (no driver, a remote desktop without
    // graphics card): run again with the other one. The window library
    // allows one event loop per process, so it is a new process.
    if let Err(e) = &result
        && std::env::var_os("APEXDESK_RENDERER").is_none()
        && matches!(e, eframe::Error::Wgpu(_) | eframe::Error::Glutin(_) | eframe::Error::NoGlutinConfigs(..) | eframe::Error::OpenGL(_))
    {
        let other = if renderer == eframe::Renderer::Wgpu { "glow" } else { "wgpu" };
        eprintln!("apexdesk: {renderer} failed ({e}); trying {other}");
        diag::warning(&format!("{renderer} failed ({e}); trying {other}"));
        let status = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args(std::env::args_os().skip(1)).env("APEXDESK_RENDERER", other).status());
        if let Ok(s) = status {
            std::process::exit(s.code().unwrap_or(1));
        }
    }
    result
}

/// The graphics back end: OpenGL, which starts fastest, or wgpu
/// (Direct3D 12, Metal, Vulkan, or their software renderers without
/// graphics card) when OpenGL fails.
/// `APEXDESK_RENDERER=glow|wgpu` or `renderer=` in the settings forces one.
fn renderer(prefs: &settings::Settings) -> eframe::Renderer {
    let chosen = std::env::var("APEXDESK_RENDERER").unwrap_or_else(|_| prefs.renderer.clone());
    match chosen.trim() {
        "wgpu" => eframe::Renderer::Wgpu,
        _ => eframe::Renderer::Glow,
    }
}

/// On Linux, OpenGL draws with the discrete card of a laptop with two
/// (PRIME offload: Mesa's DRI_PRIME, NVIDIA's own variables when its
/// driver is installed). wgpu picks it by the power preference. Windows
/// chooses per program in its graphics settings.
fn high_performance_gpu() {
    #[cfg(target_os = "linux")]
    {
        let mut vars = vec![("DRI_PRIME", "1")];
        if std::path::Path::new("/usr/share/glvnd/egl_vendor.d/10_nvidia.json").exists() {
            vars.extend([("__NV_PRIME_RENDER_OFFLOAD", "1"), ("__GLX_VENDOR_LIBRARY_NAME", "nvidia"), ("__VK_LAYER_NV_optimus", "NVIDIA_only")]);
        }
        for (k, v) in vars {
            if std::env::var_os(k).is_none() {
                // SAFETY: at startup, before any other thread exists.
                unsafe { std::env::set_var(k, v) };
            }
        }
    }
}

fn options(size: Option<[f32; 2]>, renderer: eframe::Renderer, prefs: &settings::Settings, scripted: bool) -> eframe::NativeOptions {
    let gpu_high = prefs.gpu_high;
    // The last window state is remembered, except in scripted runs (their
    // sizes are fixed). The size is the normal one, so un-maximizing returns to it.
    let (maximized, remembered) = if scripted { (false, None) } else { (prefs.window_maximized, prefs.window_size) };
    let remembered = remembered.map(|[w, h]| [w.clamp(640.0, 16384.0), h.clamp(400.0, 16384.0)]);
    let mut o = eframe::NativeOptions {
        renderer,
        viewport: egui::ViewportBuilder::default()
            .with_title("ApexDesk")
            .with_app_id("apexdesk")
            .with_inner_size(size.or(remembered).unwrap_or([1280.0, 800.0]))
            .with_maximized(maximized)
            .with_min_inner_size([640.0, 400.0])
            .with_drag_and_drop(true)
            .with_icon(egui::IconData { rgba: include_bytes!("../assets/icon64.rgba").to_vec(), width: 64, height: 64 }),
        event_loop_builder: backend_hook(),
        ..Default::default()
    };
    // wgpu: the integrated graphics card unless asked otherwise (a second
    // card is slow to wake and drains the battery). WGPU_POWER_PREF wins.
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(c) = &mut o.wgpu_options.wgpu_setup
        && std::env::var_os("WGPU_POWER_PREF").is_none()
    {
        c.power_preference = if gpu_high { eframe::wgpu::PowerPreference::HighPerformance } else { eframe::wgpu::PowerPreference::LowPower };
    }
    // Scripted runs measure frame times: no waiting for the screen.
    if scripted {
        o.glow_options.vsync = false;
        o.wgpu_options.surface.present_mode = eframe::wgpu::PresentMode::AutoNoVsync;
    }
    o
}

/// On Linux, prefer X11 (XWayland under Wayland sessions): winit's Wayland
/// backend does not support dropping files on the window.
/// `APEXDESK_WAYLAND=1` keeps the native Wayland backend.
#[cfg(target_os = "linux")]
fn backend_hook() -> Option<eframe::EventLoopBuilderHook> {
    use winit::platform::x11::EventLoopBuilderExtX11;
    let has_x11 = std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty());
    if !has_x11 || std::env::var_os("APEXDESK_WAYLAND").is_some() {
        return None;
    }
    Some(Box::new(|b| {
        b.with_x11();
    }))
}

/// Windows has no `LANG`: number formats and the spelling dictionary read
/// it, so it is taken from the user's locale ("es-ES"), and the interface
/// language (`LC_MESSAGES`) from the display language, which can differ
/// (English Windows with Spanish formats).
#[cfg(windows)]
fn system_locale() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultLocaleName(name: *mut u16, len: i32) -> i32;
        fn GetUserDefaultUILanguage() -> u16;
        fn LCIDToLocaleName(lcid: u32, name: *mut u16, len: i32, flags: u32) -> i32;
    }
    if ["LC_ALL", "LANG"].iter().any(|v| std::env::var_os(v).is_some()) {
        return;
    }
    let mut buf = [0u16; 85];
    let name = |n: i32, buf: &[u16]| (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]).replace('-', "_"));
    // SAFETY: the buffer holds LOCALE_NAME_MAX_LENGTH (85) characters.
    if let Some(l) = name(unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) }, &buf) {
        // SAFETY: no other threads exist yet.
        unsafe { std::env::set_var("LANG", l) };
    }
    if std::env::var_os("LC_MESSAGES").is_some() {
        return;
    }
    // SAFETY: as above.
    if let Some(l) = name(unsafe { LCIDToLocaleName(GetUserDefaultUILanguage() as u32, buf.as_mut_ptr(), buf.len() as i32, 0) }, &buf) {
        // SAFETY: no other threads exist yet.
        unsafe { std::env::set_var("LC_MESSAGES", l) };
    }
}

/// Programs started from the Finder get no `LANG` either: it comes from the
/// user's region setting ("es_ES"), and the interface language
/// (`LC_MESSAGES`) from the first preferred language, which can differ.
#[cfg(target_os = "macos")]
fn system_locale() {
    if ["LC_ALL", "LANG"].iter().any(|v| std::env::var_os(v).is_some()) {
        return;
    }
    let read = |key: &str| {
        let out = std::process::Command::new("defaults").args(["read", "-g", key]).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    // "es_ES@currency=EUR" -> "es_ES".
    if let Some(out) = read("AppleLocale") {
        let name = out.trim().split('@').next().unwrap_or("").to_string();
        if !name.is_empty() {
            // SAFETY: no other threads exist yet.
            unsafe { std::env::set_var("LANG", format!("{name}.UTF-8")) };
        }
    }
    if std::env::var_os("LC_MESSAGES").is_some() {
        return;
    }
    // A list, most preferred first: ("en-ES", es) -> "en_ES".
    if let Some(out) = read("AppleLanguages")
        && let Some(first) = out.split(['(', ')', ',', '\n']).map(|s| s.trim().trim_matches('"')).find(|s| !s.is_empty())
    {
        // SAFETY: no other threads exist yet.
        unsafe { std::env::set_var("LC_MESSAGES", format!("{}.UTF-8", first.replace('-', "_"))) };
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_locale() {}

#[cfg(not(target_os = "linux"))]
fn backend_hook() -> Option<eframe::EventLoopBuilderHook> {
    None
}
