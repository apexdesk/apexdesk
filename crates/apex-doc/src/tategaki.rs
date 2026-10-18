//! Vertical pages (tategaki): sections whose lines run down the page and
//! follow each other from right to left (Word's `w:textDirection
//! w:val="tbRl"`, ODF's `style:writing-mode="tb-rl"`). The text is laid
//! out on the page as the lines see it, turned a quarter to the left
//! (see [`PageSetup::flow_setup`]), and drawn turned back.

use crate::{PageSetup, TextFlow};

impl PageSetup {
    /// The text runs in vertical lines from right to left.
    pub fn vertical(&self) -> bool {
        matches!(self.text_flow, Some(TextFlow::TbRl | TextFlow::TbRlV))
    }

    /// Vertical with Chinese, Japanese and Korean characters upright
    /// (`tbRl`; `tbRlV` turns all of it).
    pub fn upright(&self) -> bool {
        self.text_flow == Some(TextFlow::TbRl)
    }

    /// Makes the text horizontal or vertical (right to left).
    pub fn set_vertical(&mut self, on: bool) {
        if on != self.vertical() {
            self.text_flow = on.then_some(TextFlow::TbRl);
        }
    }

    /// The page as vertical lines see it: turned a quarter to the left,
    /// its top edge is the lines' start (the left margin of the turned
    /// page), its right edge where the first line goes (the top margin).
    /// The gutter and mirrored margins become plain margins of the first
    /// page.
    pub fn flow_setup(&self) -> PageSetup {
        let (left, top) = (self.text_left(0), self.text_top());
        let (w, h) = (self.content_width().max(1.0), self.content_height().max(1.0));
        PageSetup {
            width: self.height,
            height: self.width,
            margin_left: top,
            margin_right: (self.height - top - h).max(0.0),
            margin_top: (self.width - left - w).max(0.0),
            margin_bottom: left,
            gutter: 0.0,
            gutter_top: false,
            mirror: false,
            // The header and footer stay at the top and bottom.
            margin_header: 0.0,
            margin_footer: 0.0,
            ..*self
        }
    }
}

/// Digits typed in vertical text that Word keeps horizontal when asked:
/// one to four of them fit a character's cell.
pub fn upright_fits(text: &str) -> bool {
    let n = text.chars().count();
    (1..=4).contains(&n) && !text.contains(['\n', '\t'])
}

#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn flow_setup_turns_the_page() {
        let mut p = PageSetup::A4;
        (p.margin_top, p.margin_bottom, p.margin_left, p.margin_right) = (99.0, 85.0, 70.0, 60.0);
        assert!(!p.vertical());
        p.set_vertical(true);
        assert!(p.vertical() && p.upright() && p.text_flow == Some(TextFlow::TbRl));
        let f = p.flow_setup();
        // Lines are as long as the text area is high; there are as many
        // of them as fit its width.
        assert_eq!((f.width, f.height), (p.height, p.width));
        assert!((f.content_width() - p.content_height()).abs() < 1e-3);
        assert!((f.content_height() - p.content_width()).abs() < 1e-3);
        // The first line is at the right margin; lines start at the top.
        assert_eq!((f.margin_top, f.margin_left, f.margin_bottom, f.margin_right), (60.0, 99.0, 70.0, 85.0));
        p.set_vertical(false);
        assert_eq!(p.text_flow, None);
        assert!(upright_fits("12") && upright_fits("2026") && !upright_fits("12345") && !upright_fits(""));
    }
}
