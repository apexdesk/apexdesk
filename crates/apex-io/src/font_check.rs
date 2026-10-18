//! TrueType outlines a file brings, checked before they are drawn: a
//! composite glyph may name others (or itself) over and over, so a few
//! bytes unfold into millions of points that take for ever to draw.
//! (CFF charstrings are held to a budget where they run, in
//! vendor/read-fonts.)

use std::collections::HashMap;

use skrifa::raw::tables::glyf::{Glyf, Glyph};
use skrifa::raw::tables::loca::Loca;
use skrifa::raw::types::GlyphId;
use skrifa::raw::{FontRef, TableProvider};

/// The points one glyph may unfold into.
const MAX_POINTS: u64 = 1 << 17;
/// How deep components may nest (as the outline loader takes them).
const MAX_DEPTH: usize = 32;

/// Glyphs already measured: their points, or `None` while measuring (a
/// glyph met again then is a cycle).
type Seen = HashMap<u32, Option<u64>>;

/// Whether every glyph of the TrueType outlines of `data` (if it has
/// any) draws within bounds.
pub fn outlines_bounded(data: &[u8]) -> bool {
    let Ok(font) = FontRef::from_index(data, 0) else { return false };
    let n = font.maxp().map(|m| m.num_glyphs() as u32).unwrap_or(0);
    let mut seen = Seen::new();
    (0..n).all(|g| glyph_bounded_in(&font, GlyphId::new(g), &mut seen))
}

/// Whether glyph `gid` draws within bounds.
pub fn glyph_bounded(font: &FontRef, gid: GlyphId) -> bool {
    glyph_bounded_in(font, gid, &mut Seen::new())
}

fn glyph_bounded_in(font: &FontRef, gid: GlyphId, seen: &mut Seen) -> bool {
    let (Ok(loca), Ok(glyf)) = (font.loca(None), font.glyf()) else { return true };
    points(gid, 0, seen, &loca, &glyf).is_some_and(|n| n <= MAX_POINTS)
}

/// The points glyph `g` unfolds into; none when it is broken (a cycle, too
/// deep, too many, or a simple glyph without points: read-fonts panics on
/// some of these).
fn points(g: GlyphId, depth: usize, seen: &mut Seen, loca: &Loca, glyf: &Glyf) -> Option<u64> {
    match seen.get(&g.to_u32()) {
        Some(Some(n)) => return Some(*n),
        Some(None) => return None,
        None => {}
    }
    if depth > MAX_DEPTH {
        return None;
    }
    seen.insert(g.to_u32(), None);
    let n = match loca.get_glyf(g, glyf).ok().flatten() {
        Some(Glyph::Simple(s)) if s.num_points() == 0 => return None,
        Some(Glyph::Simple(s)) => s.num_points() as u64,
        Some(Glyph::Composite(c)) => {
            let mut n = 0u64;
            for k in c.components() {
                n = n.saturating_add(1).saturating_add(points(k.glyph.into(), depth + 1, seen, loca, glyf)?);
                if n > MAX_POINTS {
                    return None;
                }
            }
            n
        }
        None => 0,
    };
    seen.insert(g.to_u32(), Some(n));
    Some(n)
}
