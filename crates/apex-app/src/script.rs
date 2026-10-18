//! Drives the UI from a script file so features can be
//! checked end to end without a person at the keyboard.
//!
//! `APEXDESK_SCRIPT=file apexdesk [files]`, one command per line:
//!
//! ```text
//! text Hello world        # typed text
//! key ctrl+B              # a key with optional modifiers (ctrl, shift, alt)
//! alt                     # Alt pressed and released alone (KeyTips)
//! click 400 300           # click at logical coordinates (also: dclick, rclick)
//! drag 100 100 300 200    # press, move, release
//! path 100 100 150 80 200 120  # press, move through the points, release
//! paste some text         # a clipboard paste
//! copy / cut              # clipboard commands
//! wait 10                 # idle frames
//! sleep 3500              # real time (ms), for delays such as tooltips
//! move 400 300            # move the pointer
//! scroll -300             # mouse wheel (points; negative = down)
//! scroll -300 0           # horizontal and vertical (negative = right)
//! perf start / perf NAME  # collect frame times / print them
//! shot /tmp/out.ppm       # save a screenshot (PPM)
//! pdf /tmp/out.pdf        # export the active tab as PDF
//! export /tmp/out.html    # export the active tab as HTML (or .epub)
//! recipients /tmp/a.csv    # mail merge data source for the active document
//! hover /path/file.docx   # drag a file over the window (until `drop`)
//! drop /path/file.docx    # drop it
//! image /path/pic.png     # insert a picture in the active sheet
//! quit
//! ```

use std::collections::VecDeque;
use std::path::PathBuf;

use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, pos2};

pub enum Step {
    Input(Vec<Event>, Modifiers),
    Shot(PathBuf),
    /// Export the active tab as PDF.
    Pdf(PathBuf),
    /// Export the active tab as HTML or EPUB (by extension).
    Export(PathBuf),
    /// Load a mail merge data source into the active document.
    Recipients(PathBuf),
    PerfStart,
    PerfReport(String),
    Quit,
    Hover(PathBuf),
    Drop(PathBuf),
    /// Insert a picture file in the active sheet.
    Image(PathBuf),
    /// Get Data from a file (a new query in the active sheet).
    Query(PathBuf),
    /// Show a PDF in File > View PDF.
    ViewPdf(PathBuf),
    Idle,
    /// Idle frames for this long (milliseconds).
    Sleep(u64),
}

/// A file "dropped" by the script, or dropped on the window on Wayland
/// (`sysclip::wayland_drops`).
#[derive(Debug)]
pub struct ScriptFile(pub PathBuf);

impl egui::DroppedFile for ScriptFile {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn bytes(&self) -> Result<Vec<u8>, String> {
        apex_fs::read(&self.0).map_err(|e| e.to_string())
    }

    #[cfg(target_arch = "wasm32")]
    fn bytes_async(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + '_>> {
        Box::pin(async move { apex_fs::read(&self.0).map_err(|e| e.to_string()) })
    }
}

pub struct Script {
    steps: VecDeque<Step>,
}

fn key_events(spec: &str) -> Option<(Vec<Event>, Modifiers)> {
    let mut mods = Modifiers::NONE;
    let mut name = spec;
    for part in spec.split('+') {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "cmd" => mods = mods.plus(Modifiers::COMMAND),
            "shift" => mods = mods.plus(Modifiers::SHIFT),
            "alt" => mods = mods.plus(Modifiers::ALT),
            _ => name = part,
        }
    }
    let key = Key::from_name(name)?;
    let ev = |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: mods };
    let mut events = vec![ev(true), ev(false)];
    // Like egui-winit, clipboard shortcuts (with any extra modifiers) also
    // arrive as clipboard events.
    if mods.command {
        match key {
            Key::C => events.push(Event::Copy),
            Key::X => events.push(Event::Cut),
            Key::V => events.push(Event::Paste(String::new())),
            _ => {}
        }
    }
    Some((events, mods))
}

fn pointer(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

impl Script {
    /// Script from `APEXDESK_SCRIPT` (a file) or `APEXDESK_SCREENSHOT` (take
    /// one screenshot and quit).
    pub fn from_env() -> Option<Script> {
        if let Some(p) = std::env::var_os("APEXDESK_SCRIPT") {
            return Some(Script::parse(&apex_fs::read_to_string(p).ok()?));
        }
        let shot = std::env::var("APEXDESK_SCREENSHOT").ok()?;
        Some(Script::parse(&format!("wait 25\nshot {shot}\nquit")))
    }

    pub fn parse(text: &str) -> Script {
        let mut steps = VecDeque::new();
        // Let the first frames lay out before acting.
        for _ in 0..5 {
            steps.push_back(Step::Idle);
        }
        for line in text.lines() {
            let line = line.trim_start();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
            let nums: Vec<f32> = rest.split_whitespace().filter_map(|n| n.parse().ok()).collect();
            let none = Modifiers::NONE;
            match cmd {
                "text" => steps.push_back(Step::Input(vec![Event::Text(rest.to_string())], none)),
                "paste" => steps.push_back(Step::Input(vec![Event::Paste(rest.replace("\\n", "\n").replace("\\t", "\t"))], none)),
                "alt" => {
                    steps.push_back(Step::Input(Vec::new(), Modifiers::ALT));
                    steps.push_back(Step::Input(Vec::new(), none));
                }
                "copy" => steps.push_back(Step::Input(vec![Event::Copy], none)),
                "cut" => steps.push_back(Step::Input(vec![Event::Cut], none)),
                "key" => match key_events(rest.trim()) {
                    Some((ev, m)) => steps.push_back(Step::Input(ev, m)),
                    None => eprintln!("script: unknown key {rest}"),
                },
                "rclick" if nums.len() == 2 => {
                    let p = pos2(nums[0], nums[1]);
                    steps.push_back(Step::Input(vec![Event::PointerMoved(p)], none));
                    let ev = |pressed| Event::PointerButton { pos: p, button: PointerButton::Secondary, pressed, modifiers: none };
                    steps.push_back(Step::Input(vec![ev(true)], none));
                    steps.push_back(Step::Input(vec![ev(false)], none));
                }
                "click" | "dclick" if nums.len() == 2 => {
                    let p = pos2(nums[0], nums[1]);
                    steps.push_back(Step::Input(vec![Event::PointerMoved(p)], none));
                    let times = if cmd == "dclick" { 2 } else { 1 };
                    for _ in 0..times {
                        steps.push_back(Step::Input(vec![pointer(p, true)], none));
                        steps.push_back(Step::Input(vec![pointer(p, false)], none));
                    }
                }
                "sclick" if nums.len() == 2 => {
                    // Shift+click.
                    let p = pos2(nums[0], nums[1]);
                    let m = egui::Modifiers::SHIFT;
                    steps.push_back(Step::Input(vec![Event::PointerMoved(p)], m));
                    for pressed in [true, false] {
                        steps.push_back(Step::Input(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: m }], m));
                    }
                }
                "drag" | "sdrag" if nums.len() == 4 => {
                    // sdrag: with Shift held.
                    let m = if cmd == "sdrag" { egui::Modifiers::SHIFT } else { none };
                    let press = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: m };
                    let (a, b) = (pos2(nums[0], nums[1]), pos2(nums[2], nums[3]));
                    steps.push_back(Step::Input(vec![Event::PointerMoved(a)], m));
                    steps.push_back(Step::Input(vec![press(a, true)], m));
                    for i in 1..=8 {
                        steps.push_back(Step::Input(vec![Event::PointerMoved(a + (b - a) * (i as f32 / 8.0))], m));
                    }
                    steps.push_back(Step::Input(vec![press(b, false)], m));
                }
                "path" if nums.len() >= 4 && nums.len() % 2 == 0 => {
                    let pts: Vec<Pos2> = nums.chunks(2).map(|c| pos2(c[0], c[1])).collect();
                    steps.push_back(Step::Input(vec![Event::PointerMoved(pts[0])], none));
                    steps.push_back(Step::Input(vec![pointer(pts[0], true)], none));
                    for w in pts.windows(2) {
                        for i in 1..=4 {
                            steps.push_back(Step::Input(vec![Event::PointerMoved(w[0] + (w[1] - w[0]) * (i as f32 / 4.0))], none));
                        }
                    }
                    steps.push_back(Step::Input(vec![pointer(pts[pts.len() - 1], false)], none));
                }
                "scroll" if nums.len() == 1 => steps.push_back(Step::Input(
                    vec![Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, nums[0]), phase: egui::TouchPhase::Move, modifiers: none }],
                    none,
                )),
                "scroll" if nums.len() == 2 => steps.push_back(Step::Input(
                    vec![Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(nums[0], nums[1]), phase: egui::TouchPhase::Move, modifiers: none }],
                    none,
                )),
                "perf" if rest.trim() == "start" => steps.push_back(Step::PerfStart),
                "perf" => steps.push_back(Step::PerfReport(rest.trim().to_string())),
                "move" if nums.len() == 2 => steps.push_back(Step::Input(vec![Event::PointerMoved(pos2(nums[0], nums[1]))], none)),
                "sleep" if nums.len() == 1 => steps.push_back(Step::Sleep(nums[0] as u64)),
                "wait" => {
                    for _ in 0..nums.first().copied().unwrap_or(1.0) as usize {
                        steps.push_back(Step::Idle);
                    }
                }
                "shot" => {
                    steps.push_back(Step::Idle);
                    steps.push_back(Step::Shot(PathBuf::from(rest.trim())));
                }
                "pdf" => steps.push_back(Step::Pdf(PathBuf::from(rest.trim()))),
                "export" => steps.push_back(Step::Export(PathBuf::from(rest.trim()))),
                "recipients" => steps.push_back(Step::Recipients(PathBuf::from(rest.trim()))),
                "quit" => steps.push_back(Step::Quit),
                "hover" => steps.push_back(Step::Hover(PathBuf::from(rest.trim()))),
                "drop" => steps.push_back(Step::Drop(PathBuf::from(rest.trim()))),
                "image" => steps.push_back(Step::Image(PathBuf::from(rest.trim()))),
                "query" => steps.push_back(Step::Query(PathBuf::from(rest.trim()))),
                "viewpdf" => steps.push_back(Step::ViewPdf(PathBuf::from(rest.trim()))),
                _ => eprintln!("script: bad line: {line}"),
            }
            // One quiet frame between steps so the UI reacts like it would
            // to a person.
            steps.push_back(Step::Idle);
        }
        Script { steps }
    }

    pub fn next(&mut self) -> Option<Step> {
        self.steps.pop_front()
    }
}

/// Writes a screenshot as a binary PPM file.
pub fn save_ppm(path: &std::path::Path, img: &egui::ColorImage) {
    let mut out = format!("P6\n{} {}\n255\n", img.size[0], img.size[1]).into_bytes();
    for p in &img.pixels {
        out.extend_from_slice(&[p.r(), p.g(), p.b()]);
    }
    let _ = crate::common::write_out(path, out);
}
