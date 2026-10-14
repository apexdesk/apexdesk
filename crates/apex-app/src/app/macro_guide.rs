//! The macro guide of the editor (its Guide button): getting started,
//! what macros can't do here and why, and formulas (LET, LAMBDA) that do
//! what many macros do. The texts are in the language files: `## ` starts
//! a section, `` ` `` a line of code, `- ` an item.

use eframe::egui::{self, RichText};

use crate::i18n::tr;

/// (title, text) of each topic.
const TOPICS: [(&str, &str); 3] = [("macros.guide_start_title", "macros.guide_start"), ("macros.guide_limits_title", "macros.guide_limits"), ("macros.guide_lambda_title", "macros.guide_lambda")];

/// A line of a text as it shows.
#[derive(Debug, PartialEq)]
enum Line<'a> {
    Heading(&'a str),
    Code(&'a str),
    Item(&'a str),
    Text(&'a str),
    Gap,
}

fn parse(text: &str) -> Vec<Line<'_>> {
    text.lines()
        .map(|l| {
            if let Some(h) = l.strip_prefix("## ") {
                Line::Heading(h.trim())
            } else if let Some(c) = l.strip_prefix('`') {
                Line::Code(c)
            } else if let Some(i) = l.strip_prefix("- ") {
                Line::Item(i.trim())
            } else if l.trim().is_empty() {
                Line::Gap
            } else {
                Line::Text(l.trim())
            }
        })
        .collect()
}

/// The guide's window, at topic `open` while it is Some.
pub(super) fn show(ctx: &egui::Context, open: &mut Option<usize>) {
    let Some(topic) = open.as_mut() else { return };
    let mut keep = true;
    egui::Window::new(tr("macros.guide_title")).id(egui::Id::new("macro_guide")).open(&mut keep).default_size([720.0, 500.0]).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).show(ctx, |ui| {
        egui::Panel::left(egui::Id::new("macro_guide_topics")).resizable(false).default_size(190.0).show(ui, |ui| {
            for (i, (title, _)) in TOPICS.iter().enumerate() {
                if ui.selectable_label(*topic == i, tr(title)).clicked() {
                    *topic = i;
                }
            }
        });
        let text = tr(TOPICS[(*topic).min(TOPICS.len() - 1)].1);
        egui::ScrollArea::vertical().id_salt(("macro_guide_text", *topic)).auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let code_bg = ui.visuals().extreme_bg_color;
            let mut code: Vec<&str> = Vec::new();
            let lines = parse(text);
            for (k, line) in lines.iter().enumerate() {
                if let Line::Code(c) = line {
                    code.push(c);
                    if matches!(lines.get(k + 1), Some(Line::Code(_))) {
                        continue;
                    }
                    egui::Frame::new().fill(code_bg).inner_margin(6.0).corner_radius(4.0).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(code.join("\n")).monospace());
                    });
                    code.clear();
                    continue;
                }
                match line {
                    Line::Heading(h) => {
                        ui.add_space(6.0);
                        ui.label(RichText::new(*h).strong().size(15.0));
                    }
                    Line::Item(t) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("•");
                            ui.label(*t);
                        });
                    }
                    Line::Text(t) => {
                        ui.label(*t);
                    }
                    Line::Gap => ui.add_space(2.0),
                    Line::Code(_) => {}
                }
            }
        });
    });
    if !keep {
        *open = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_of_a_text() {
        let l = parse("## Title\nSome text.\n`Sub A()\n`    x = 1\n- an item\n\nEnd.");
        assert_eq!(l, [Line::Heading("Title"), Line::Text("Some text."), Line::Code("Sub A()"), Line::Code("    x = 1"), Line::Item("an item"), Line::Gap, Line::Text("End.")]);
    }

    #[test]
    fn every_language_has_the_guide() {
        for (title, text) in TOPICS {
            assert!(!tr(title).is_empty() && tr(text).contains("## "), "{text}");
        }
        let ctx = egui::Context::default();
        let mut open = Some(2);
        for _ in 0..2 {
            ctx.run_ui(Default::default(), |ui| show(ui.ctx(), &mut open)).textures_delta.clear();
        }
        assert_eq!(open, Some(2));
    }
}
