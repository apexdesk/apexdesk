//! The dark workspace for PDF pages: a drawn page is recolored pixel by
//! pixel with the same mapping as documents ([`crate::theme::dark_color`]),
//! keeping pictures as they are. Screen only: the file, printing and
//! export are drawn from the file, not from here.

use std::collections::HashMap;

/// Recolors `rgba` (opaque pixels, `width` wide), except the pixel boxes
/// `keep` (pictures).
pub fn darken_page(rgba: &mut [u8], width: usize, keep: &[[u32; 4]]) {
    if width == 0 {
        return;
    }
    let height = rgba.len() / 4 / width;
    let mut kept = vec![false; width * height];
    for b in keep {
        let (x0, x1) = ((b[0] as usize).min(width), (b[2] as usize).min(width));
        for y in (b[1] as usize).min(height)..(b[3] as usize).min(height) {
            kept[y * width + x0..y * width + x1].fill(true);
        }
    }
    // A page has few distinct colors (and runs of the same one).
    let mut seen: HashMap<[u8; 3], [u8; 3]> = HashMap::new();
    let mut last: Option<([u8; 3], [u8; 3])> = None;
    for (px, keep) in rgba.chunks_exact_mut(4).zip(kept) {
        if keep {
            continue;
        }
        let c = [px[0], px[1], px[2]];
        let out = match last {
            Some((from, to)) if from == c => to,
            _ => {
                let to = *seen.entry(c).or_insert_with(|| {
                    let d = crate::theme::dark_color(eframe::egui::Color32::from_rgb(c[0], c[1], c[2]));
                    [d.r(), d.g(), d.b()]
                });
                last = Some((c, to));
                to
            }
        };
        px[..3].copy_from_slice(&out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paper goes dark, ink light, a red stays red, and a picture's box
    /// keeps its pixels.
    #[test]
    fn pages_go_dark_and_pictures_stay() {
        let px = |c: [u8; 3]| [c[0], c[1], c[2], 255];
        let white = px([255, 255, 255]);
        let black = px([0, 0, 0]);
        let red = px([255, 0, 0]);
        let photo = px([250, 250, 250]);
        let mut v: Vec<u8> = [white, black, red, photo].concat();
        darken_page(&mut v, 4, &[[3, 0, 4, 1]]);
        assert!(v[0] < 40 && v[4] > 210, "{v:?}");
        assert!(v[8] > 200 && v[9] < 30, "{v:?}");
        assert_eq!(&v[12..16], &photo);
        assert_eq!(v[3], 255);
    }

    /// With the option on, the view draws its pages dark, and shows the
    /// real colors when asked; the file's own rendering is untouched.
    #[test]
    fn the_view_follows_the_option_and_the_toggle() {
        crate::theme::set_dark_canvas(true, true);
        let mut v = crate::pdf_view::PdfView::open(crate::pdf_view::tests::sample(), None).unwrap();
        assert!(v.dark());
        assert!(v.paper().r() < 40);
        v.see_light = true;
        assert!(!v.dark());
        assert_eq!(v.paper(), eframe::egui::Color32::WHITE);
        crate::theme::set_dark_canvas(false, false);
        v.see_light = false;
        assert!(!v.dark());
    }

    /// The drawing thread hands the page back dark when asked, light when not.
    #[test]
    fn the_drawing_thread_darkens_when_asked() {
        let bytes: std::sync::Arc<[u8]> = crate::pdf_view::tests::sample().into();
        let draw = |dark| {
            let (reply, rx) = std::sync::mpsc::channel();
            let job = crate::pdf_view::engine::Job { doc: 0, rev: 0, bytes: bytes.clone(), password: None, page: 0, width: 120, annots: true, dark, reply, ctx: None };
            assert!(crate::pdf_view::engine::request(job));
            rx.recv().unwrap()
        };
        let (light, dark) = (draw(false), draw(true));
        assert!(!light.dark && dark.dark);
        assert_eq!(&light.rgba[..3], &[255, 255, 255]);
        assert!(dark.rgba[0] < 40, "{:?}", &dark.rgba[..4]);
        assert!(dark.rgba.chunks_exact(4).any(|p| p[0] > 120), "text goes light");
    }
}
