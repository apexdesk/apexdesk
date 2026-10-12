//! The real terminal: raw mode and the alternate screen (restored on exit
//! and on panics), the event loop, colors and the system clipboard.

use std::io::{self, Write};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui::style::Color;

use crate::App;

/// How colors of the file are shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Colors {
    /// 24-bit colors.
    Rgb,
    /// The nearest of the 256 xterm colors.
    Indexed,
}

impl Colors {
    /// Terminals that announce 24-bit colors (`COLORTERM`), and those known
    /// to have them; others get the 256-color palette.
    pub fn detect() -> Colors {
        let env = |k: &str| std::env::var(k).unwrap_or_default().to_ascii_lowercase();
        let ct = env("COLORTERM");
        let prog = env("TERM_PROGRAM");
        if ct.contains("truecolor") || ct.contains("24bit") || std::env::var_os("WT_SESSION").is_some() || ["iterm.app", "wezterm", "vscode", "ghostty"].contains(&prog.as_str()) || env("TERM").contains("direct") {
            Colors::Rgb
        } else {
            Colors::Indexed
        }
    }

    pub fn color(self, rgb: u32) -> Color {
        let (r, g, b) = ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
        match self {
            Colors::Rgb => Color::Rgb(r, g, b),
            Colors::Indexed => Color::Indexed(xterm256(r, g, b)),
        }
    }
}

/// The nearest xterm color: the 6×6×6 cube or the gray ramp.
fn xterm256(r: u8, g: u8, b: u8) -> u8 {
    let level = |v: u8| if v < 48 { 0 } else if v < 115 { 1 } else { (v - 35) / 40 };
    let steps = [0u8, 95, 135, 175, 215, 255];
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = 16 + 36 * ri + 6 * gi + bi;
    let d = |a: (u8, u8, u8)| (a.0 as i32 - r as i32).pow(2) + (a.1 as i32 - g as i32).pow(2) + (a.2 as i32 - b as i32).pow(2);
    let cube_d = d((steps[ri as usize], steps[gi as usize], steps[bi as usize]));
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gi = if avg > 238 { 23 } else { (avg.saturating_sub(3) / 10) as u8 };
    let gv = 8 + 10 * gi;
    if d((gv, gv, gv)) < cube_d { 232 + gi } else { cube }
}

/// Relative luminance (0..1) of a 0xRRGGBB color.
pub fn luminance(rgb: u32) -> f32 {
    let (r, g, b) = ((rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255);
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}

/// Base64 for OSC 52.
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for ch in data.chunks(3) {
        let n = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= ch.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Puts text on the system clipboard with the OSC 52 sequence (most
/// terminals, also over SSH; others ignore it).
fn osc52(text: &str) -> io::Result<()> {
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()
}

fn restore(enhanced: bool) {
    if enhanced {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();
}

pub fn run(app: &mut App) -> io::Result<()> {
    let mut terminal: DefaultTerminal = ratatui::try_init()?;
    // Keys like Ctrl+I (apart from Tab) where the terminal can tell them.
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        let _ = execute!(io::stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    }
    let _ = execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore(enhanced);
        hook(info);
    }));
    let res = (|| -> io::Result<()> {
        while !app.should_quit() {
            terminal.draw(|f| app.draw(f))?;
            if let Some(text) = app.take_clipboard() {
                osc52(&text)?;
            }
            app.handle(event::read()?);
        }
        Ok(())
    })();
    restore(enhanced);
    let _ = std::panic::take_hook();
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_and_base64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(xterm256(255, 0, 0), 196);
        assert_eq!(xterm256(128, 128, 128), 244);
        assert_eq!(xterm256(0, 0, 0), 16);
        assert!(luminance(0xFFFFFF) > 0.99 && luminance(0) < 0.01);
    }
}
