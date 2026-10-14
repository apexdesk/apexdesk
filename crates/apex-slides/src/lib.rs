//! ApexDesk presentations: slides and their objects, the masters and
//! layouts they inherit from, themes, notes, sections, transitions and
//! animations, with the edits of the editor and their undo history. No
//! user interface (the view is `apex-app`'s `slides_view`) and no file
//! formats (pptx, odp and ppt are in `apex-io`), like `apex-doc` and
//! `apex-sheet`.
//!
//! What a file holds that isn't modelled is kept: each slide, layout,
//! master and object remembers where it came from and, while unchanged,
//! is written back as it was read (see [`Origin`] and [`object::Raw`]).

use std::sync::Arc;

/// The most objects a slide, layout or master read from a file keeps, and
/// the most of them that stay placeholders (each is looked up on the
/// layout and master whenever it is painted).
pub const MAX_OBJECTS: usize = 10_000;
/// The most slides a deck read from a file keeps.
pub const MAX_SLIDES: usize = 10_000;
pub const MAX_PLACEHOLDERS: usize = 64;
/// The most animations a slide plays (the show looks each up per frame).
pub const MAX_ANIMS: usize = 1_000;

/// A deck read from a file, kept within [`MAX_OBJECTS`] and
/// [`MAX_PLACEHOLDERS`].
pub fn bound(deck: &mut Deck) {
    fn cap(objs: &mut Vec<object::Object>) {
        objs.truncate(MAX_OBJECTS);
        let mut phs = 0;
        for o in objs.iter_mut() {
            o.walk_mut(&mut |o| {
                if o.ph.is_some() {
                    phs += 1;
                    if phs > MAX_PLACEHOLDERS {
                        o.ph = None;
                    }
                }
            });
        }
    }
    for m in &mut deck.masters {
        let m = Arc::make_mut(m);
        cap(&mut m.objects);
        for l in &mut m.layouts {
            cap(&mut l.objects);
        }
    }
    for s in &mut deck.slides {
        let s = Arc::make_mut(s);
        cap(&mut s.objects);
        s.timing.anims.truncate(MAX_ANIMS);
    }
}

pub mod anim;
pub mod build;
pub mod color;
pub mod combine;
pub mod edit;
pub mod fill;
pub mod geom;
pub mod object;
pub mod resolve;
pub mod table;
pub mod template;
pub mod text;

pub use anim::{Anim, Timing, Transition};
pub use color::{ClrMap, ColorCx, Rgba, SColor, Scheme};
pub use fill::{Background, Fill, FormatScheme, Line};
pub use object::{Frame, Geometry, ObjKind, Object, PhType, Placeholder, Src};
pub use resolve::Chain;
pub use table::Table;
pub use text::{BodyProps, ListStyle, ParaProps, RunProps, TextBody};

use apex_doc::{KeptPart, Theme};

/// Points per EMU (DrawingML's unit: 12700 per point).
pub const EMU_PT: f32 = 12700.0;

/// Where a part of the presentation came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Origin {
    pub src: Src,
    /// pptx: the part's path ("ppt/slides/slide3.xml"); odp: its name.
    pub part: String,
    /// odp: the element as read.
    pub xml: Option<Arc<str>>,
    /// Unchanged since read: the part goes back as it was.
    pub same: bool,
}

impl Origin {
    pub fn pptx(part: impl Into<String>) -> Option<Origin> {
        Some(Origin { src: Src::Pptx, part: part.into(), xml: None, same: true })
    }
}

/// Marks a changed part (it keeps its path, for the references of the
/// objects kept as they were).
fn touch(o: &mut Option<Origin>) {
    if let Some(o) = o {
        o.same = false;
    }
}

/// A theme: colors and fonts, and the fills and outlines styles refer to.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SlideTheme {
    pub theme: Theme,
    pub fmt: FormatScheme,
    pub origin: Option<Origin>,
}

/// A layout: placeholders and decoration a slide starts from.
#[derive(Clone, Debug)]
pub struct Layout {
    pub name: String,
    /// DrawingML layout type ("title", "obj", "twoObj", "blank"...).
    pub kind: Arc<str>,
    pub objects: Vec<object::Object>,
    pub background: Option<Background>,
    pub map: Option<ClrMap>,
    /// The master's shapes show on it.
    pub show_master: bool,
    pub origin: Option<Origin>,
}

/// A slide master: theme, text styles, background, decoration and the
/// layouts that follow it.
#[derive(Clone, Debug)]
pub struct Master {
    pub name: String,
    pub theme: SlideTheme,
    pub map: ClrMap,
    pub background: Option<Background>,
    pub objects: Vec<object::Object>,
    /// Text styles of titles, bodies and the rest (`p:txStyles`).
    pub title_style: ListStyle,
    pub body_style: ListStyle,
    pub other_style: ListStyle,
    pub layouts: Vec<Layout>,
    pub origin: Option<Origin>,
}

/// The speaker notes of a slide.
#[derive(Clone, Debug, Default)]
pub struct Notes {
    pub text: TextBody,
    pub origin: Option<Origin>,
}

/// A comment on a slide.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Comment {
    pub author: String,
    pub initials: String,
    /// ISO 8601.
    pub date: String,
    pub text: String,
    /// Where it is pinned, points.
    pub pos: (f32, f32),
    pub replies: Vec<Comment>,
}

#[derive(Clone, Debug)]
pub struct Slide {
    /// Unique in the presentation (pptx `sldId`, 256 and up).
    pub id: u32,
    pub master: usize,
    pub layout: usize,
    pub name: String,
    pub objects: Vec<object::Object>,
    pub background: Option<Background>,
    pub map: Option<ClrMap>,
    pub show_master: bool,
    pub hidden: bool,
    pub notes: Option<Notes>,
    pub transition: Transition,
    pub timing: Timing,
    pub comments: Vec<Comment>,
    pub origin: Option<Origin>,
}

impl Slide {
    pub fn new(id: u32, master: usize, layout: usize) -> Slide {
        Slide { id, master, layout, name: String::new(), objects: Vec::new(), background: None, map: None, show_master: true, hidden: false, notes: None, transition: Transition::default(), timing: Timing::default(), comments: Vec::new(), origin: None }
    }

    /// The title placeholder's text.
    pub fn title(&self) -> String {
        self.objects.iter().find(|o| o.is_title()).and_then(|o| o.text()).map(|t| t.text().replace('\n', " ")).unwrap_or_default()
    }

    pub fn notes_text(&self) -> String {
        self.notes.as_ref().map(|n| n.text.text()).unwrap_or_default()
    }

    /// A new object id on this slide.
    pub fn next_id(&self) -> u32 {
        self.objects.iter().map(object::Object::max_id).max().unwrap_or(1).max(1) + 1
    }

    /// The object with this id, in groups too.
    pub fn find(&self, id: u32) -> Option<&object::Object> {
        fn look(v: &[object::Object], id: u32) -> Option<&object::Object> {
            for o in v {
                if o.id == id {
                    return Some(o);
                }
                if let ObjKind::Group(g) = &o.kind {
                    if let Some(f) = look(&g.children, id) {
                        return Some(f);
                    }
                }
            }
            None
        }
        look(&self.objects, id)
    }
}

/// A named run of slides (pptx sections).
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub name: String,
    /// Its GUID in pptx.
    pub id: String,
    /// Slide ids, in order.
    pub slides: Vec<u32>,
}

/// How the show runs.
#[derive(Clone, Debug, PartialEq)]
pub struct ShowSettings {
    /// Starts again after the last slide until Esc.
    pub looped: bool,
    /// Browsed at a kiosk: slides advance by their timings only.
    pub kiosk: bool,
    /// Slides advance by their timings (else only by clicks).
    pub use_timings: bool,
    /// The settings as the file had them (`looped`, `kiosk`,
    /// `use_timings`; the defaults for a new deck).
    pub read: (bool, bool, bool),
}

impl ShowSettings {
    /// Settings as read from a file.
    pub fn new(looped: bool, kiosk: bool, use_timings: bool) -> Self {
        ShowSettings { looped, kiosk, use_timings, read: (looped, kiosk, use_timings) }
    }

    /// The settings are no longer what the file had: its own settings
    /// part has to be written again.
    pub fn changed(&self) -> bool {
        (self.looped, self.kiosk, self.use_timings) != self.read
    }
}

impl Default for ShowSettings {
    fn default() -> Self {
        ShowSettings::new(false, false, true)
    }
}

/// Files of the package as read: what isn't written again goes back
/// from here (media, embedded objects, custom XML, comments...).
#[derive(Clone, Debug, Default)]
pub struct Package {
    pub src: Option<Src>,
    pub parts: Vec<KeptPart>,
}

impl Package {
    pub fn part(&self, path: &str) -> Option<&KeptPart> {
        self.parts.iter().find(|p| p.path == path)
    }
}

/// Everything the undo history snapshots.
#[derive(Clone, Debug)]
pub struct Deck {
    /// Slide size, points.
    pub width: f32,
    pub height: f32,
    pub notes_size: (f32, f32),
    pub slides: Vec<Arc<Slide>>,
    pub masters: Vec<Arc<Master>>,
    pub sections: Vec<Section>,
    /// Text style of shapes that aren't placeholders (`p:defaultTextStyle`).
    pub default_style: ListStyle,
    pub show: ShowSettings,
    /// Number of the first slide.
    pub first_number: u32,
    pub meta: apex_doc::Meta,
    pub macros: apex_doc::Macros,
    pub package: Arc<Package>,
    /// Header and footer of new slides: date, slide number, footer text.
    pub hf: HeaderFooter,
}

/// Header and footer settings (Insert > Header & Footer).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeaderFooter {
    pub date: bool,
    pub number: bool,
    pub footer: bool,
    pub footer_text: String,
    /// A fixed date (else today's).
    pub fixed_date: Option<String>,
    /// Not on the title slide.
    pub not_on_title: bool,
}

impl Deck {
    pub fn slide(&self, i: usize) -> &Slide {
        &self.slides[i.min(self.slides.len().saturating_sub(1))]
    }

    pub fn master_of(&self, s: &Slide) -> &Master {
        &self.masters[s.master.min(self.masters.len() - 1)]
    }

    pub fn layout_of(&self, s: &Slide) -> Option<&Layout> {
        self.master_of(s).layouts.get(s.layout)
    }

    /// A slide id not used yet.
    pub fn new_slide_id(&self) -> u32 {
        self.slides.iter().map(|s| s.id).max().unwrap_or(255).max(255) + 1
    }

    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.slides.iter().position(|s| s.id == id)
    }

    /// Slide `i` to change (it is written again when saved).
    pub fn slide_mut(&mut self, i: usize) -> &mut Slide {
        let s = Arc::make_mut(&mut self.slides[i]);
        touch(&mut s.origin);
        s
    }

    pub fn master_mut(&mut self, m: usize) -> &mut Master {
        let s = Arc::make_mut(&mut self.masters[m]);
        touch(&mut s.origin);
        s
    }

    pub fn layout_mut(&mut self, m: usize, l: usize) -> &mut Layout {
        let lay = &mut Arc::make_mut(&mut self.masters[m]).layouts[l];
        touch(&mut lay.origin);
        lay
    }

    /// Object `id` of slide `i` to change: it and its slide are written again.
    pub fn object_mut(&mut self, i: usize, id: u32) -> Option<&mut object::Object> {
        edit::find_mut(&mut self.slide_mut(i).objects, id)
    }

    /// The name of the section slide `i` is in, if it starts one.
    pub fn section_starting(&self, i: usize) -> Option<&Section> {
        let id = self.slides.get(i)?.id;
        self.sections.iter().find(|s| s.slides.first() == Some(&id))
    }
}

/// A presentation being edited: the deck and its undo history.
pub struct Presentation {
    pub deck: Deck,
    undo: Vec<(Deck, &'static str, u64)>,
    redo: Vec<(Deck, &'static str, u64)>,
    pub modified: bool,
    /// Incremented on every change (caches of the view key on it).
    pub revision: u64,
}

/// Steps kept in the undo history.
const UNDO_LIMIT: usize = 200;

impl Presentation {
    pub fn new(deck: Deck) -> Presentation {
        Presentation { deck, undo: Vec::new(), redo: Vec::new(), modified: false, revision: 0 }
    }

    /// The deck to change as one undo step named `what` (a translation key).
    pub fn edit(&mut self, what: &'static str) -> &mut Deck {
        self.edit_merge(what, 0)
    }

    /// Like `edit`, but changes with the same nonzero `key` as the last
    /// step join it (typing in one box).
    pub fn edit_merge(&mut self, what: &'static str, key: u64) -> &mut Deck {
        let joins = key != 0 && self.undo.last().is_some_and(|u| u.2 == key);
        if !joins {
            self.undo.push((self.deck.clone(), what, key));
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.modified = true;
        self.revision += 1;
        &mut self.deck
    }

    /// Ends joining (the next typing starts a new step).
    pub fn seal(&mut self) {
        if let Some(u) = self.undo.last_mut() {
            u.2 = 0;
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_names(&self) -> Vec<&'static str> {
        self.undo.iter().rev().map(|u| u.1).collect()
    }

    pub fn redo_names(&self) -> Vec<&'static str> {
        self.redo.iter().rev().map(|u| u.1).collect()
    }

    pub fn undo(&mut self) -> bool {
        let Some((d, what, _)) = self.undo.pop() else { return false };
        let cur = std::mem::replace(&mut self.deck, d);
        self.redo.push((cur, what, 0));
        self.modified = true;
        self.revision += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some((d, what, _)) = self.redo.pop() else { return false };
        let cur = std::mem::replace(&mut self.deck, d);
        self.undo.push((cur, what, 0));
        self.modified = true;
        self.revision += 1;
        true
    }
}

impl std::ops::Deref for Presentation {
    type Target = Deck;

    fn deref(&self) -> &Deck {
        &self.deck
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_and_joining() {
        let mut p = Presentation::new(build::new_deck(0));
        let n = p.slides.len();
        p.edit("slides.undo.new_slide").slides.push(Arc::new(Slide::new(999, 0, 1)));
        assert_eq!(p.slides.len(), n + 1);
        p.edit_merge("slides.undo.typing", 7).slides[0] = Arc::new(Slide::new(1000, 0, 0));
        p.edit_merge("slides.undo.typing", 7).slides[0] = Arc::new(Slide::new(1001, 0, 0));
        assert_eq!(p.undo_names().len(), 2);
        assert!(p.undo());
        assert_ne!(p.slides[0].id, 1001);
        assert!(p.undo());
        assert_eq!(p.slides.len(), n);
        assert!(p.redo());
        assert_eq!(p.slides.len(), n + 1);
    }
}
