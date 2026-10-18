//! The Office clipboard: the last things copied in any window, to paste
//! again from the Clipboard pane. It is also how formatted text and cells
//! go from one document to another: the system clipboard only takes their
//! text, and a view pasting that text finds here what was copied with it.

use std::sync::Arc;
use crate::widgets::FitWindow as _;
#[cfg(not(test))]
use std::sync::{LazyLock, Mutex};

use apex_doc::Paragraph;
use eframe::egui;

use crate::i18n::tr;

/// Copies kept.
pub const MAX: usize = 24;

/// What a copy holds besides its text.
#[derive(Clone)]
pub enum Rich {
    Text,
    /// Formatted text of a document, and what it needs from that document
    /// to look the same in another (see `Document::adopt`).
    Doc(Arc<Vec<Paragraph>>, Arc<apex_doc::Origin>),
    /// Cells of a sheet, with their formats (see `Workbook::adopt_clip`).
    Cells(Arc<apex_sheet::Clip>),
    /// A shape of a sheet (the text is its text or its name).
    Shape(Arc<apex_sheet::SheetShape>),
    /// Text of a slide: paragraphs whose text says all of its look itself
    /// (what its placeholder and master gave it too).
    Runs(Arc<Vec<Paragraph>>),
}

#[derive(Clone)]
pub struct Entry {
    pub text: String,
    pub rich: Rich,
}

/// Newest first.
#[cfg(not(test))]
static ITEMS: LazyLock<Mutex<Vec<Entry>>> = LazyLock::new(|| Mutex::new(Vec::new()));

#[cfg(not(test))]
fn with_items<R>(f: impl FnOnce(&mut Vec<Entry>) -> R) -> R {
    f(&mut ITEMS.lock().unwrap_or_else(|e| e.into_inner()))
}

// Each test has its own clipboard (tests run side by side, one a thread).
#[cfg(test)]
thread_local! {
    static ITEMS: std::cell::RefCell<Vec<Entry>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn with_items<R>(f: impl FnOnce(&mut Vec<Entry>) -> R) -> R {
    ITEMS.with_borrow_mut(f)
}

/// Keeps a copy (a copy of the same text replaces the older one; plain
/// text doesn't replace what was copied here with its formatting).
pub fn push(mut e: Entry) {
    if e.text.trim().is_empty() && matches!(e.rich, Rich::Text) {
        return;
    }
    with_items(|v| {
        if let Some(i) = v.iter().position(|x| x.text == e.text) {
            let old = v.remove(i);
            if matches!(e.rich, Rich::Text) {
                e = old;
            }
        }
        v.insert(0, e);
        v.truncate(MAX);
    });
}

pub fn items() -> Vec<Entry> {
    with_items(|v| v.clone())
}

/// The last thing copied (or pasted from another program).
pub fn newest() -> Option<Entry> {
    with_items(|v| v.first().cloned())
}

/// What was copied in any window with the text `text` (the text the
/// system clipboard gives back on paste).
pub fn find(text: &str) -> Option<Entry> {
    with_items(|v| v.iter().find(|e| e.text == text).cloned())
}

/// What the pane asks for.
pub enum Action {
    Paste(Entry),
    /// Everything, oldest first.
    PasteAll(Vec<Entry>),
}

/// A line or two of an entry's text.
fn preview(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).collect();
    let s = lines.join(" ⏎ ").replace('\t', "  ");
    if s.chars().count() > 120 { format!("{}…", s.chars().take(120).collect::<String>()) } else { s }
}

/// The Clipboard pane of a window; `all`: offer Paste All.
pub fn pane(ctx: &egui::Context, id: egui::Id, open: &mut bool, all: bool) -> Option<Action> {
    let mut action = None;
    let screen = ctx.content_rect();
    crate::widgets::esc_closes(ctx, id, open);
    egui::Window::new(tr("clipboard.title"))
        .id(id)
        .open(open)
        .default_pos(egui::pos2(screen.min.x + 12.0, screen.min.y + 180.0))
        .default_size(egui::vec2(270.0, 440.0))
        .resizable(true)
        .collapsible(false)
        .show_fit(ctx, |ui| {
            let items = items();
            ui.horizontal(|ui| {
                if all && ui.add_enabled(!items.is_empty(), egui::Button::new(tr("clipboard.paste_all"))).clicked() {
                    action = Some(Action::PasteAll(items.iter().rev().cloned().collect()));
                }
                if ui.add_enabled(!items.is_empty(), egui::Button::new(tr("clipboard.clear_all"))).clicked() {
                    with_items(|v| v.clear());
                }
            });
            ui.separator();
            if items.is_empty() {
                ui.weak(tr("clipboard.empty"));
                return;
            }
            ui.weak(tr("clipboard.hint"));
            let mut remove = None;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (i, e) in items.iter().enumerate() {
                    let kind = match e.rich {
                        Rich::Text => "clipboard.kind_text",
                        Rich::Doc(..) | Rich::Runs(_) => "clipboard.kind_doc",
                        Rich::Cells(_) => "clipboard.kind_cells",
                        Rich::Shape(_) => "clipboard.kind_shape",
                    };
                    let frame = egui::Frame::group(ui.style()).inner_margin(6.0);
                    let r = frame
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.small(tr(kind));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("×").on_hover_text(tr("clipboard.delete")).clicked() {
                                        remove = Some(i);
                                    }
                                });
                            });
                            ui.add(egui::Label::new(preview(&e.text)).wrap());
                        })
                        .response;
                    let r = ui.interact(r.rect, id.with(("clip", i)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tr("clipboard.paste_this"));
                    crate::widgets::name(&r, egui::WidgetType::Button, &format!("{}: {}", tr("clipboard.paste_this"), preview(&e.text)), None);
                    if r.hovered() {
                        ui.painter().rect_stroke(r.rect, 3.0, ui.visuals().selection.stroke, egui::StrokeKind::Inside);
                    }
                    if r.clicked() && remove.is_none() {
                        action = Some(Action::Paste(e.clone()));
                    }
                }
            });
            if let Some(i) = remove {
                with_items(|v| {
                    if i < v.len() {
                        v.remove(i);
                    }
                });
            }
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_last_copies() {
        for i in 0..30 {
            push(Entry { text: format!("t{i}"), rich: Rich::Text });
        }
        push(Entry { text: "t29".into(), rich: Rich::Text });
        push(Entry { text: "  ".into(), rich: Rich::Text });
        let v = items();
        assert_eq!(v.len(), MAX);
        assert_eq!(v[0].text, "t29");
        assert_eq!(v[1].text, "t28");
        assert_eq!(preview("a\n\n b\tc\nd\ne"), "a ⏎ b  c ⏎ d");
        // Pasting the text back doesn't lose what was copied with it.
        push(Entry { text: "t5".into(), rich: Rich::Cells(Default::default()) });
        push(Entry { text: "t5".into(), rich: Rich::Text });
        assert!(matches!(find("t5").map(|e| e.rich), Some(Rich::Cells(_))));
        assert_eq!(items()[0].text, "t5");
        assert!(find("nothing").is_none());
    }
}
