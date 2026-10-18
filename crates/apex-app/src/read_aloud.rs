//! Read Aloud (Review > Read Aloud): reads a list of texts (the paragraphs
//! of a document, the selected cells of a sheet) one after another with a
//! voice of the system, and shows the floating player: previous, play or
//! pause, next, stop, and the speed and voice.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use web_time::Duration;

use eframe::egui::{self, Rect, RichText, vec2};

use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::speech::{self, Event, Status};
use crate::widgets::{self, Face};

/// A text to read, in a language ("es-ES").
#[derive(Clone, Debug)]
pub struct Item {
    pub text: String,
    pub lang: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Play {
    /// The engine is still being looked for.
    Waiting,
    Playing,
    Paused,
    Done,
}

/// What happened since the last frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tick {
    Nothing,
    /// Another item or word is being read.
    Moved,
    /// The end, or stopped.
    Finished,
    /// There is no voice: why.
    Missing(String),
}

static NEXT: AtomicU64 = AtomicU64::new(1);
/// The reader speaking now (only one at a time).
static ACTIVE: AtomicU64 = AtomicU64::new(0);

static PREFS: Mutex<(String, f32)> = Mutex::new((String::new(), 1.0));
static CHANGED: AtomicBool = AtomicBool::new(false);

/// Voice id (empty: by the text's language) and speed.
pub fn prefs() -> (String, f32) {
    PREFS.lock().map(|p| p.clone()).unwrap_or_else(|_| (String::new(), 1.0))
}

pub fn set_prefs(voice: &str, rate: f32) {
    if let Ok(mut p) = PREFS.lock() {
        *p = (voice.to_string(), rate.clamp(0.5, 2.0));
    }
}

/// Whether the player changed the voice or speed (for the settings).
pub fn take_changed() -> bool {
    CHANGED.swap(false, Ordering::Relaxed)
}

/// The language of text without one: the spelling language, else the
/// interface's.
pub fn default_lang() -> String {
    let l = crate::spell::language();
    speech::norm_lang(if l.is_empty() { crate::i18n::current() } else { &l })
}

pub struct Reader {
    items: Vec<Item>,
    /// The item being read.
    pub cur: usize,
    /// The word being read (byte range of the item's text), when the
    /// engine tells.
    pub word: Option<(usize, usize)>,
    play: Play,
    utt: u64,
    id: u64,
}

impl Reader {
    /// Starts reading `items` (empty ones are skipped), once the engine
    /// is ready (see `speech::start`).
    pub fn new(items: Vec<Item>) -> Reader {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        ACTIVE.store(id, Ordering::Relaxed);
        speech::stop();
        let mut r = Reader { items, cur: 0, word: None, play: Play::Waiting, utt: 0, id };
        r.skip_blank(1);
        r
    }

    /// Moves `cur` past empty items in direction `dir`.
    fn skip_blank(&mut self, dir: isize) {
        while self.items.get(self.cur).is_some_and(|i| i.text.trim().is_empty()) {
            match self.cur.checked_add_signed(dir) {
                Some(c) => self.cur = c,
                None => {
                    self.cur = self.items.len();
                    return;
                }
            }
        }
    }

    fn say(&mut self) {
        self.word = None;
        let Some(item) = self.items.get(self.cur) else {
            self.finish();
            return;
        };
        self.utt = NEXT.fetch_add(1, Ordering::Relaxed);
        let (voice, rate) = prefs();
        if speech::speak(self.utt, &item.text, &item.lang, &voice, rate) {
            self.play = Play::Playing;
        } else {
            self.finish();
        }
    }

    fn finish(&mut self) {
        if self.play != Play::Done {
            self.play = Play::Done;
            self.word = None;
            if ACTIVE.load(Ordering::Relaxed) == self.id {
                speech::stop();
            }
        }
    }

    /// Follows the engine; call once a frame.
    pub fn tick(&mut self, ctx: &egui::Context) -> Tick {
        if self.play == Play::Done {
            return Tick::Finished;
        }
        if ACTIVE.load(Ordering::Relaxed) != self.id {
            // Another document started reading.
            self.play = Play::Done;
            self.word = None;
            return Tick::Finished;
        }
        match speech::status() {
            Status::NotStarted | Status::Loading => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return Tick::Nothing;
            }
            Status::Missing(why) => {
                self.play = Play::Done;
                return Tick::Missing(why);
            }
            Status::Ready => {}
        }
        if self.play == Play::Waiting {
            // The first words not taken: say so rather than stop unseen.
            let first = self.items.get(self.cur).is_some();
            self.say();
            return match self.play {
                Play::Done if first => Tick::Missing(crate::i18n::tr("speak.failed").to_string()),
                Play::Done => Tick::Finished,
                _ => Tick::Moved,
            };
        }
        let mut moved = false;
        for e in speech::events() {
            match e {
                Event::Word { utt, start, len } if utt == self.utt => {
                    self.word = Some((start, start + len));
                    moved = true;
                }
                Event::End { utt } if utt == self.utt => {
                    self.cur += 1;
                    self.skip_blank(1);
                    self.say();
                    moved = true;
                }
                _ => {}
            }
        }
        if self.play == Play::Done {
            Tick::Finished
        } else if moved {
            Tick::Moved
        } else {
            Tick::Nothing
        }
    }

    pub fn toggle_pause(&mut self) {
        match self.play {
            Play::Playing => {
                speech::pause();
                self.play = Play::Paused;
            }
            Play::Paused => {
                speech::resume();
                self.play = Play::Playing;
            }
            _ => {}
        }
    }

    /// Reads the previous (`-1`) or next (`1`) item.
    pub fn step(&mut self, dir: isize) {
        if self.play == Play::Done || self.play == Play::Waiting {
            return;
        }
        let old = self.cur;
        match self.cur.checked_add_signed(dir) {
            Some(c) if c < self.items.len() => self.cur = c,
            _ if dir > 0 => return self.finish(),
            // Before the first: that one again.
            _ => {}
        }
        self.skip_blank(dir);
        if self.cur >= self.items.len() {
            if dir > 0 {
                return self.finish();
            }
            self.cur = old;
        }
        self.say();
    }

    pub fn stop(&mut self) {
        self.finish();
    }

    /// The floating player at the top right of `area`. Returns true when
    /// closed.
    pub fn bar(&mut self, ctx: &egui::Context, area: Rect) -> bool {
        let mut closed = false;
        egui::Area::new(egui::Id::new(("read_aloud", self.id)))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(area.right_top() + vec2(-24.0, 12.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let waiting = self.play == Play::Waiting;
                        ui.add_enabled_ui(!waiting, |ui| {
                            if widgets::tool_button(ui, Face::Icon(Icon::SpeakPrev), false, tr("speak.prev")).clicked() {
                                self.step(-1);
                            }
                            let (icon, tip) = if self.play == Play::Paused { (Icon::SpeakPlay, "speak.play") } else { (Icon::SpeakPause, "speak.pause") };
                            if widgets::tool_button(ui, Face::Icon(icon), false, tr(tip)).clicked() {
                                self.toggle_pause();
                            }
                            if widgets::tool_button(ui, Face::Icon(Icon::SpeakNext), false, tr("speak.next")).clicked() {
                                self.step(1);
                            }
                        });
                        if widgets::tool_button(ui, Face::Icon(Icon::SpeakStop), false, tr("speak.stop")).clicked() {
                            self.stop();
                            closed = true;
                        }
                        ui.separator();
                        self.settings_menu(ui);
                        if waiting {
                            ui.spinner();
                        } else {
                            ui.label(RichText::new(format!("{}/{}", (self.cur + 1).min(self.items.len()), self.items.len())).weak().small());
                        }
                    });
                });
            });
        closed
    }

    fn settings_menu(&mut self, ui: &mut egui::Ui) {
        let (mut voice, mut rate) = prefs();
        let before = (voice.clone(), rate);
        crate::widgets::menu_button(ui, tr("speak.settings"), |ui| {
            ui.set_min_width(260.0);
            ui.label(tr("speak.speed"));
            ui.add(egui::Slider::new(&mut rate, 0.5..=2.0).step_by(0.05).custom_formatter(|v, _| format!("{v:.2}×")));
            ui.add_space(4.0);
            ui.label(tr("speak.voice"));
            voice_menu(ui, &mut voice);
        });
        if (voice.clone(), rate) != before {
            set_prefs(&voice, rate);
            CHANGED.store(true, Ordering::Relaxed);
            // Heard at once: the current item starts again.
            if self.play == Play::Playing {
                self.say();
            }
        }
    }
}

/// The voices of the system to choose from ("" = by the text's language).
pub fn voice_menu(ui: &mut egui::Ui, voice: &mut String) {
    let voices = speech::voices();
    let name = |id: &str| if id.is_empty() { tr("speak.voice_auto").to_string() } else { voices.iter().find(|v| v.id == id).map_or_else(|| id.to_string(), |v| v.name.clone()) };
    crate::widgets::combo_name(
        egui::ComboBox::from_id_salt("speak_voice").selected_text(name(voice)).width(240.0).show_ui(ui, |ui| {
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                ui.selectable_value(voice, String::new(), tr("speak.voice_auto"));
                for v in &voices {
                    let label = if v.lang.is_empty() { v.name.clone() } else { format!("{} ({})", v.name, v.lang) };
                    ui.selectable_value(voice, v.id.clone(), label);
                }
            });
        }),
        tr("speak.voice"),
    );
    match speech::status() {
        Status::Missing(why) => {
            ui.label(RichText::new(trf("speak.no_voice", &[("why", &why)])).weak().small());
        }
        Status::Loading => {
            ui.label(RichText::new(tr("speak.loading")).weak().small());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(texts: &[&str]) -> Vec<Item> {
        texts.iter().map(|t| Item { text: t.to_string(), lang: "es".into() }).collect()
    }

    #[test]
    fn skips_blank_items() {
        let mut r = Reader::new(items(&["", "  ", "Uno", "", "Dos"]));
        assert_eq!(r.cur, 2);
        // Without an engine answering yet, moving waits.
        r.step(1);
        assert_eq!(r.cur, 2);
        r.play = Play::Paused;
        r.cur = 4;
        r.skip_blank(-1);
        assert_eq!(r.cur, 4);
        r.cur = 3;
        r.skip_blank(-1);
        assert_eq!(r.cur, 2);
        let r = Reader::new(items(&["", " "]));
        assert_eq!(r.cur, 2);
    }
}
