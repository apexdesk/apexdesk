//! A presentation made to follow another's design (Design > Apply
//! Company Template): the template's masters, with their theme, text
//! styles, backgrounds, decoration and layouts, replace the deck's, and
//! each slide moves to the template layout of the same type.

use crate::object::{Frame, Object};
use crate::{Deck, Master};

/// What else changes besides the masters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Adapt {
    /// Take the template's slide size, scaling what is on the slides.
    pub size: bool,
    /// Drop the slides' own backgrounds and color maps.
    pub backgrounds: bool,
    /// Placeholders go where the template's layouts put them.
    pub positions: bool,
}

impl Default for Adapt {
    fn default() -> Adapt {
        Adapt { size: true, backgrounds: true, positions: true }
    }
}

/// The template's masters, cut loose from its file: written anew from
/// the model when the deck is saved (their pictures are in the model).
fn masters_of(tpl: &Deck) -> Vec<std::sync::Arc<Master>> {
    tpl.masters
        .iter()
        .map(|m| {
            let mut m = (**m).clone();
            m.origin = None;
            m.theme.origin = None;
            m.objects.iter_mut().for_each(Object::detach);
            for l in &mut m.layouts {
                l.origin = None;
                l.objects.iter_mut().for_each(Object::detach);
            }
            std::sync::Arc::new(m)
        })
        .collect()
}

/// The template layout for a slide that had layout `kind` named `name`.
fn matching(m: &Master, kind: &str, name: &str) -> usize {
    let by_kind = (kind != "cust").then(|| m.layouts.iter().position(|l| &*l.kind == kind)).flatten();
    by_kind
        .or_else(|| m.layouts.iter().position(|l| l.name.eq_ignore_ascii_case(name)))
        .or_else(|| m.layouts.iter().position(|l| &*l.kind == "obj"))
        .or_else(|| (m.layouts.len() > 1).then_some(1))
        .unwrap_or(0)
}

/// Makes `deck` follow `tpl`'s design.
pub fn apply_template(deck: &mut Deck, tpl: &Deck, o: &Adapt) {
    if tpl.masters.is_empty() {
        return;
    }
    // Each slide's master and layout type, before they go.
    let was: Vec<(usize, String, String)> = deck
        .slides
        .iter()
        .map(|s| {
            let l = deck.layout_of(s);
            (s.master, l.map(|l| l.kind.to_string()).unwrap_or_default(), l.map(|l| l.name.clone()).unwrap_or_default())
        })
        .collect();
    if o.size && ((tpl.width - deck.width).abs() > 0.01 || (tpl.height - deck.height).abs() > 0.01) {
        let (kx, ky) = (tpl.width / deck.width.max(1.0), tpl.height / deck.height.max(1.0));
        for i in 0..deck.slides.len() {
            for ob in &mut deck.slide_mut(i).objects {
                if let Some(f) = &mut ob.sp.frame {
                    *f = Frame { x: f.x * kx, y: f.y * ky, w: f.w * kx, h: f.h * ky, ..*f };
                    ob.raw = None;
                }
            }
        }
        deck.width = tpl.width;
        deck.height = tpl.height;
    }
    deck.masters = masters_of(tpl);
    deck.default_style = tpl.default_style.clone();
    for (i, (master, kind, name)) in was.into_iter().enumerate() {
        let m = if master < deck.masters.len() { master } else { 0 };
        let l = matching(&deck.masters[m], &kind, &name);
        crate::edit::set_layout(deck, i, m, l);
        let s = deck.slide_mut(i);
        if o.backgrounds {
            s.background = None;
            s.map = None;
        }
        if o.positions {
            for ob in &mut s.objects {
                if ob.ph.is_some_and(|p| !p.kind.is_hf()) && ob.sp.frame.is_some() {
                    ob.sp.frame = None;
                    ob.raw = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{STANDARD, new_deck};

    #[test]
    fn slides_take_the_template_design() {
        let mut deck = new_deck(0);
        crate::edit::add_slide(&mut deck, 1, 0, 1);
        let title_kind = deck.layout_of(&deck.slides[0]).unwrap().kind.to_string();
        let second_kind = deck.layout_of(&deck.slides[1]).unwrap().kind.to_string();
        deck.slide_mut(0).background = Some(crate::fill::Background::Fill(crate::fill::Fill::None));
        let title_id = deck.slides[0].objects.iter().find(|o| o.is_title()).unwrap().id;
        let t = crate::edit::find_mut(&mut deck.slide_mut(0).objects, title_id).unwrap();
        t.sp.frame = Some(Frame::new(10.0, 10.0, 100.0, 40.0));

        let mut tpl = new_deck(3);
        tpl.width = STANDARD.0;
        tpl.height = STANDARD.1;
        // Only two layouts in the template, in another order.
        let m = tpl.master_mut(0);
        let obj = m.layouts.iter().position(|l| &*l.kind == "obj").unwrap();
        let title = m.layouts.iter().position(|l| &*l.kind == "title").unwrap();
        m.layouts = vec![m.layouts[obj].clone(), m.layouts[title].clone()];
        m.origin = None;

        apply_template(&mut deck, &tpl, &Adapt::default());
        assert_eq!(deck.masters.len(), 1);
        assert_eq!(deck.masters[0].theme.theme.name, tpl.masters[0].theme.theme.name);
        assert_eq!((deck.width, deck.height), STANDARD);
        assert_eq!(deck.slides[0].layout, 1);
        assert_eq!(&*deck.layout_of(&deck.slides[0]).unwrap().kind, title_kind.as_str());
        assert_eq!(&*deck.layout_of(&deck.slides[1]).unwrap().kind, second_kind.as_str());
        assert!(deck.slides[0].background.is_none());
        assert!(deck.slides[0].objects.iter().find(|o| o.is_title()).unwrap().sp.frame.is_none());
    }
}
