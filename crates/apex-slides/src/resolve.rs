//! What an object looks like once everything it inherits is applied: the
//! master's text styles, the master's and the layout's placeholders, the
//! theme's style references and color map, then its own settings.

use std::sync::Arc;

use apex_doc::{Attrs, Paragraph, Theme};

use crate::color::{Base, ClrMap, ColorCx, Rgba, SColor, Scheme};
use crate::fill::{Background, Fill, Gradient, Line};
use crate::object::{Frame, Geometry, Object, PhType, Placeholder};
use crate::text::{BodyProps, Caps, ParaProps, TextBody};
use crate::{Deck, Layout, Master, Slide};

/// The masters, layouts and theme an object on a slide (or a layout, or
/// a master) inherits from.
#[derive(Clone, Copy)]
pub struct Chain<'a> {
    pub deck: &'a Deck,
    pub master: &'a Master,
    /// None for the master's own objects.
    pub layout: Option<&'a Layout>,
    pub slide: Option<&'a Slide>,
    pub map: ClrMap,
}

/// A run's look, all resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct RunFmt {
    /// Points.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Rgba,
    pub font: Arc<str>,
    pub caps: Caps,
    /// Letter spacing, points.
    pub spacing: f32,
    /// Raised (positive) or lowered, share of the size.
    pub baseline: f32,
    pub highlight: Option<u32>,
    pub link: Option<Arc<str>>,
}

impl<'a> Chain<'a> {
    pub fn for_slide(deck: &'a Deck, slide: &'a Slide) -> Chain<'a> {
        let master = deck.master_of(slide);
        let layout = master.layouts.get(slide.layout);
        let map = slide.map.or(layout.and_then(|l| l.map)).unwrap_or(master.map);
        Chain { deck, master, layout, slide: Some(slide), map }
    }

    pub fn for_layout(deck: &'a Deck, m: usize, l: usize) -> Chain<'a> {
        let master = &deck.masters[m];
        let layout = master.layouts.get(l);
        let map = layout.and_then(|l| l.map).unwrap_or(master.map);
        Chain { deck, master, layout, slide: None, map }
    }

    pub fn for_master(deck: &'a Deck, m: usize) -> Chain<'a> {
        let master = &deck.masters[m];
        Chain { deck, master, layout: None, slide: None, map: master.map }
    }

    pub fn theme(&self) -> &'a Theme {
        &self.master.theme.theme
    }

    pub fn cx(&self) -> ColorCx<'a> {
        ColorCx { theme: self.theme(), map: self.map, ph: None }
    }

    pub fn color(&self, c: &SColor) -> Rgba {
        c.resolve(&self.cx())
    }

    /// The objects a placeholder follows: on the layout (for a slide's)
    /// and on the master.
    pub fn sources(&self, ph: Placeholder, own_layout: bool) -> (Option<&'a Object>, Option<&'a Object>) {
        let key = (self.layout.map_or(0, |l| l as *const Layout as usize), self.master as *const Master as usize, ph, own_layout);
        let at = |objs: &[Object], o: Option<&Object>| o.and_then(|o| objs.iter().position(|x| std::ptr::eq(x, o)));
        let found = MEMO.with(|m| {
            let mut m = m.borrow_mut();
            let m = m.as_mut()?;
            if let Some(&(l, s)) = m.get(&key) {
                return Some((l, s));
            }
            let (lo, so) = self.sources_now(ph, own_layout);
            let v = (at(self.layout.map_or(&[], |l| &l.objects), lo), at(&self.master.objects, so));
            m.insert(key, v);
            Some(v)
        });
        match found {
            Some((l, s)) => (l.and_then(|i| self.layout?.objects.get(i)), s.and_then(|i| self.master.objects.get(i))),
            None => self.sources_now(ph, own_layout),
        }
    }

    fn sources_now(&self, ph: Placeholder, own_layout: bool) -> (Option<&'a Object>, Option<&'a Object>) {
        let lay = if own_layout { None } else { self.layout.and_then(|l| find_ph(&l.objects, ph)) };
        let kind = lay.and_then(|o| o.ph).map_or(ph.kind, |p| p.kind).master_kind();
        let mas = self.master.objects.iter().find(|o| o.ph.is_some_and(|p| p.kind.master_kind() == kind));
        (lay, mas)
    }

    /// The sources of `o`: none unless it's a placeholder, only the
    /// master's for a layout's own objects.
    fn chain_of(&self, o: &Object) -> (Option<&'a Object>, Option<&'a Object>) {
        let Some(ph) = o.ph else { return (None, None) };
        if self.slide.is_none() && self.layout.is_none() {
            return (None, None);
        }
        self.sources(ph, self.slide.is_none())
    }

    pub fn frame(&self, o: &Object) -> Frame {
        if let Some(f) = o.sp.frame {
            return f;
        }
        let (l, m) = self.chain_of(o);
        l.and_then(|x| x.sp.frame).or(m.and_then(|x| x.sp.frame)).unwrap_or_else(|| Frame::new(0.0, 0.0, self.deck.width / 2.0, self.deck.height / 4.0))
    }

    pub fn geometry(&self, o: &Object) -> Geometry {
        let (l, m) = self.chain_of(o);
        o.sp.geom.clone().or_else(|| l.and_then(|x| x.sp.geom.clone())).or_else(|| m.and_then(|x| x.sp.geom.clone())).unwrap_or_default()
    }

    /// The fill, `None` when there is none.
    pub fn fill(&self, o: &Object) -> Option<Fill> {
        let (l, m) = self.chain_of(o);
        let own = o.sp.fill.clone().or_else(|| l.and_then(|x| x.sp.fill.clone())).or_else(|| m.and_then(|x| x.sp.fill.clone()));
        let f = match own {
            Some(f) => f,
            None => {
                let style = o.style.as_ref().or_else(|| l.and_then(|x| x.style.as_ref())).or_else(|| m.and_then(|x| x.style.as_ref()));
                let (idx, c) = style.and_then(|s| s.fill.clone())?;
                subst_ph(self.master.theme.fmt.fill(idx)?, &c)?
            }
        };
        if f == Fill::None { None } else { Some(f) }
    }

    /// The outline, `None` when there is none.
    pub fn line(&self, o: &Object) -> Option<Line> {
        let (l, m) = self.chain_of(o);
        let mut out = Line::default();
        let style = o.style.as_ref().or_else(|| l.and_then(|x| x.style.as_ref())).or_else(|| m.and_then(|x| x.style.as_ref()));
        if let Some((idx, c)) = style.and_then(|s| s.line.clone()) {
            if let Some(sl) = self.master.theme.fmt.line(idx) {
                out = sl.clone();
                out.fill = out.fill.and_then(|f| subst_ph(&f, &c));
            }
        }
        for x in [m, l].into_iter().flatten() {
            if let Some(ln) = &x.sp.line {
                out.overlay(ln);
            }
        }
        if let Some(ln) = &o.sp.line {
            out.overlay(ln);
        }
        match &out.fill {
            None | Some(Fill::None) => None,
            _ => Some(out),
        }
    }

    /// The text frame's settings.
    pub fn body(&self, o: &Object, t: &TextBody) -> BodyProps {
        let (l, m) = self.chain_of(o);
        let mut b = BodyProps::default();
        for x in [m, l].into_iter().flatten() {
            if let Some(tb) = x.text() {
                b.overlay(&tb.body);
            }
        }
        b.overlay(&t.body);
        b
    }

    /// The master text style the object's text starts from.
    fn base_style(&self, o: &Object, out: &mut ParaProps, level: u8) {
        match o.ph.map(|p| p.kind) {
            Some(k) if k.is_title() => self.master.title_style.apply(level, out),
            Some(k) if k.is_hf() => self.master.other_style.apply(level, out),
            Some(PhType::SlideImage) => self.master.other_style.apply(level, out),
            Some(_) => self.master.body_style.apply(level, out),
            None => {
                if self.deck.default_style.is_empty() {
                    self.master.other_style.apply(level, out)
                } else {
                    self.deck.default_style.apply(level, out)
                }
            }
        }
    }

    /// Paragraph `i` of text `t` of object `o`: its properties with what
    /// it inherits.
    pub fn para(&self, o: &Object, t: &TextBody, i: usize) -> ParaProps {
        let level = t.paras.get(i).map_or(0, |p| p.style.level.min(8));
        let mut out = ParaProps::default();
        self.base_style(o, &mut out, level);
        // A shape's font reference colors its text over the master's
        // styles, under its own settings.
        if o.ph.is_none() {
            if let Some(c) = self.style_font_color(o) {
                out.run.color = Some(c);
            }
        }
        let (l, m) = self.chain_of(o);
        for x in [m, l].into_iter().flatten() {
            if let Some(tb) = x.text() {
                tb.list.apply(level, &mut out);
            }
        }
        t.list.apply(level, &mut out);
        if let Some(pp) = t.para_props(i) {
            out.overlay(pp);
        }
        out
    }

    /// The text color of the object's style reference (`a:fontRef`).
    fn style_font_color(&self, o: &Object) -> Option<SColor> {
        let (l, m) = self.chain_of(o);
        let style = o.style.as_ref().or_else(|| l.and_then(|x| x.style.as_ref())).or_else(|| m.and_then(|x| x.style.as_ref()));
        style.and_then(|s| s.font.as_ref()).and_then(|f| f.1.clone())
    }

    /// The color text takes when nothing sets one.
    fn default_text_color(&self, o: &Object) -> SColor {
        self.style_font_color(o).unwrap_or_else(|| SColor::scheme(Scheme::Tx1))
    }

    /// The look of a run with attributes `a` in a paragraph with
    /// properties `pp` (from [`Chain::para`]).
    pub fn run(&self, o: &Object, pp: &ParaProps, a: &Attrs) -> RunFmt {
        let r = &pp.run;
        let theme = self.theme();
        let font = a.font.clone().or_else(|| r.font.clone()).unwrap_or_else(|| Arc::from(apex_doc::theme::MINOR_TOKEN));
        let font = theme.token_font(&font).cloned().unwrap_or(font);
        let color = match a.color {
            Some(c) => Rgba::opaque(c),
            None => self.color(&r.color.clone().unwrap_or_else(|| self.default_text_color(o))),
        };
        let fx = a.fx.as_deref();
        let caps = match fx {
            Some(f) if f.all_caps => Caps::All,
            Some(f) if f.small_caps => Caps::Small,
            _ => r.caps.unwrap_or(Caps::None),
        };
        let baseline = if a.superscript {
            0.3
        } else if a.subscript {
            -0.25
        } else {
            r.baseline.unwrap_or(0.0) / 100.0
        };
        RunFmt {
            size: a.size.map_or(r.size.unwrap_or(18.0), |s| s as f32 / 2.0),
            bold: a.bold || r.bold.unwrap_or(false),
            italic: a.italic || r.italic.unwrap_or(false),
            underline: a.underline || r.underline.unwrap_or(false),
            strike: a.strike || r.strike.unwrap_or(false),
            color,
            font,
            caps,
            spacing: fx.filter(|f| f.spacing != 0).map_or(r.spacing.unwrap_or(0.0), |f| f.spacing as f32 / 20.0),
            baseline,
            highlight: a.highlight,
            link: a.link.clone(),
        }
    }

    /// The first run's look of paragraph `i` (for bullets and empty lines).
    pub fn para_run(&self, o: &Object, t: &TextBody, i: usize) -> (ParaProps, RunFmt) {
        let pp = self.para(o, t, i);
        let a = t.paras.get(i).and_then(|p: &Paragraph| p.runs.first()).map(|r| r.attrs.clone()).unwrap_or_default();
        let f = self.run(o, &pp, &a);
        (pp, f)
    }

    /// The background of the slide (or layout, or master).
    pub fn background(&self) -> Fill {
        let bg = self.slide.and_then(|s| s.background.as_ref()).or_else(|| self.layout.and_then(|l| l.background.as_ref())).or(self.master.background.as_ref());
        match bg {
            Some(Background::Fill(f)) => f.clone(),
            Some(Background::Ref(idx, c)) => self.master.theme.fmt.fill(*idx).and_then(|f| subst_ph(f, c)).unwrap_or_else(|| Fill::Solid(c.clone())),
            None => Fill::Solid(SColor::scheme(Scheme::Bg1)),
        }
    }

    /// Objects drawn under the slide's own: the master's (unless hidden)
    /// and the layout's, without their placeholders.
    pub fn decoration(&self) -> Vec<(&'a Object, Chain<'a>)> {
        let mut out = Vec::new();
        let show_master = self.slide.is_none_or(|s| s.show_master) && self.layout.is_none_or(|l| l.show_master);
        let as_master = Chain { layout: None, slide: None, ..*self };
        if self.layout.is_some() && show_master {
            out.extend(self.master.objects.iter().filter(|o| o.ph.is_none()).map(|o| (o, as_master)));
        }
        if let (Some(l), Some(_)) = (self.layout, self.slide) {
            let as_layout = Chain { slide: None, ..*self };
            out.extend(l.objects.iter().filter(|o| o.ph.is_none()).map(|o| (o, as_layout)));
        }
        out
    }
}

thread_local! {
    /// Sources found while [`memo`] runs, by layout, master and
    /// placeholder.
    static MEMO: std::cell::RefCell<Option<std::collections::HashMap<(usize, usize, Placeholder, bool), (Option<usize>, Option<usize>)>>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` (painting, which changes no layout or master) with the
/// sources of placeholders looked up once each, not at every paragraph
/// and run.
pub fn memo<T>(f: impl FnOnce() -> T) -> T {
    let outer = MEMO.with(|m| {
        let mut m = m.borrow_mut();
        let outer = m.is_some();
        if !outer {
            *m = Some(std::collections::HashMap::new());
        }
        outer
    });
    struct Clear(bool);
    impl Drop for Clear {
        fn drop(&mut self) {
            if !self.0 {
                MEMO.with(|m| *m.borrow_mut() = None);
            }
        }
    }
    let _clear = Clear(outer);
    f()
}

/// The placeholder in `objs` a placeholder follows: same index, else same kind.
pub fn find_ph(objs: &[Object], ph: Placeholder) -> Option<&Object> {
    let by_idx = |o: &&Object| o.ph.is_some_and(|p| p.idx == ph.idx && (ph.idx != 0 || p.kind.master_kind() == ph.kind.master_kind()));
    objs.iter().find(by_idx).or_else(|| objs.iter().find(|o| o.ph.is_some_and(|p| p.kind == ph.kind))).or_else(|| {
        if ph.kind.is_title() { objs.iter().find(|o| o.ph.is_some_and(|p| p.kind.is_title())) } else { None }
    })
}

/// A theme style's fill with its "phClr" replaced by the reference's color.
pub fn subst_ph(f: &Fill, c: &SColor) -> Option<Fill> {
    let sub = |x: &SColor| -> SColor {
        if x.base == Base::Placeholder {
            let mut out = c.clone();
            out.mods.extend(x.mods.iter().copied());
            out
        } else {
            x.clone()
        }
    };
    Some(match f {
        Fill::Solid(x) => Fill::Solid(sub(x)),
        Fill::Gradient(g) => Fill::Gradient(Gradient { stops: g.stops.iter().map(|(p, x)| (*p, sub(x))).collect(), angle: g.angle, path: g.path.clone() }),
        Fill::Pattern { preset, fg, bg } => Fill::Pattern { preset: preset.clone(), fg: sub(fg), bg: sub(bg) },
        other => other.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_inherit_from_layout_and_master() {
        let deck = crate::build::new_deck(0);
        let s = &deck.slides[0];
        let chain = Chain::for_slide(&deck, s);
        let title = s.objects.iter().find(|o| o.is_title()).unwrap();
        let f = chain.frame(title);
        assert!(f.w > 100.0 && f.h > 20.0);
        let t = title.text().unwrap();
        let pp = chain.para(title, t, 0);
        let rf = chain.run(title, &pp, &Attrs::default());
        assert!(rf.size >= 40.0);
        assert_eq!(rf.color, Rgba::opaque(0));
        assert_ne!(&*rf.font, apex_doc::theme::MAJOR_TOKEN);
        assert!(matches!(chain.background(), Fill::Solid(_)));
    }
}
