//! The question of the first start: open office files with ApexDesk?
//! Asked once, when ApexDesk doesn't open spreadsheets or documents yet
//! (never in a portable copy); File > Options > General does it any time.

use std::sync::mpsc;

use eframe::egui::{self, RichText};

use super::{App, Notice};
use crate::default_apps::{self, Done, GROUPS, Group};
use crate::i18n::{tr, trf};
use crate::widgets::{self, DialogResult};

#[derive(Default)]
pub(super) enum Offer {
    /// Not looked at yet.
    #[default]
    Idle,
    /// Asking the system, in a thread (it may run a program).
    Checking(mpsc::Receiver<bool>),
    /// The question, with the groups ticked.
    Open([bool; 4]),
    Done,
}

/// The groups ticked at first: all but PDF, which has a viewer of its own
/// on most systems.
pub(super) fn first_choice() -> [bool; 4] {
    GROUPS.map(|g| g != Group::Pdf)
}

/// Makes ApexDesk the default for the groups ticked; what it did, as a
/// notice.
pub(super) fn make_default(ticked: [bool; 4]) -> Notice {
    let groups: Vec<Group> = GROUPS.into_iter().zip(ticked).filter(|(_, t)| *t).map(|(g, _)| g).collect();
    match default_apps::make_default(&groups) {
        Ok(Done::Set) => Notice::info(tr("default_apps.done")),
        Ok(Done::Settings) => Notice::info(tr("default_apps.settings_opened")),
        Err(e) => Notice::error(trf("default_apps.failed", &[("error", &e)])),
    }
}

/// The groups' checkboxes.
pub(super) fn group_boxes(ui: &mut egui::Ui, ticked: &mut [bool; 4]) {
    for (g, t) in GROUPS.iter().zip(ticked.iter_mut()) {
        ui.checkbox(t, tr(g.key()));
    }
}

impl App {
    pub(super) fn default_apps_offer(&mut self, ctx: &egui::Context) {
        match &mut self.default_offer {
            Offer::Idle => {
                if cfg!(test) || cfg!(target_os = "android") || self.settings.default_asked || crate::portable::enabled() {
                    self.default_offer = Offer::Done;
                    return;
                }
                let (tx, rx) = mpsc::channel();
                apex_fs::thread::spawn(move || {
                    let not_default = [Group::Sheets, Group::Docs].iter().any(|g| default_apps::is_default(*g) == Some(false));
                    let _ = tx.send(not_default);
                });
                self.default_offer = Offer::Checking(rx);
            }
            Offer::Checking(rx) => match rx.try_recv() {
                Ok(true) => self.default_offer = Offer::Open(first_choice()),
                Ok(false) | Err(mpsc::TryRecvError::Disconnected) => self.default_offer = Offer::Done,
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(web_time::Duration::from_millis(100)),
            },
            Offer::Open(ticked) => {
                let res = widgets::dialog(ctx, "default_apps", tr("default_apps.title"), 420.0, |ui| {
                    ui.label(tr("default_apps.question"));
                    ui.add_space(6.0);
                    group_boxes(ui, ticked);
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr("default_apps.later")).weak());
                });
                if res == DialogResult::None {
                    return;
                }
                let ticked = *ticked;
                self.default_offer = Offer::Done;
                // Asked once, whatever the answer.
                self.settings.default_asked = true;
                self.settings.save();
                if res == DialogResult::Ok {
                    let n = make_default(ticked);
                    self.notify(n);
                }
            }
            Offer::Done => {}
        }
    }
}
