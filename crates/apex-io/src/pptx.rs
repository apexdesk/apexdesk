//! PowerPoint presentations (.pptx, .pptm, .potx, .ppsx...): read into an
//! `apex_slides::Deck` and written back. The package is patched: parts
//! left unchanged (slides, layouts, masters, themes, media, comments,
//! custom XML...) go back byte for byte, changed ones are written again,
//! keeping the XML of the objects in them that didn't change, and parts
//! nothing points at any more are left out.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::KeptPart;
use apex_slides::color::ClrMap;
use apex_slides::object::Src;
use apex_slides::{Deck, Layout, Master, Notes, ObjKind, Origin, Package, Section, ShowSettings, Slide, SlideTheme};

use crate::Error;
use crate::drawingml::{Node, read_fragment};
use crate::ooxml_keep::{ContentTypes, read_rels, relative, rels_path};
use crate::pptx_fill::{self as fill, Images};
use crate::pptx_shape::{self as shape, PartCx, PartOut};
use crate::pptx_text::{self as text, Links};
use crate::xml::{Ev, HEADER, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";

const REL_OFFICE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
const REL_CORE: &str = "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
const REL_APP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties";
const REL_CUSTOM_PROPS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";

const CT_SLIDE: &str = "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";
const CT_LAYOUT: &str = "application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml";
const CT_MASTER: &str = "application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml";
const CT_NOTES: &str = "application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml";
const CT_NOTES_MASTER: &str = "application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml";
const CT_PRES_PROPS: &str = "application/vnd.openxmlformats-officedocument.presentationml.presProps+xml";
const CT_VIEW_PROPS: &str = "application/vnd.openxmlformats-officedocument.presentationml.viewProps+xml";
const CT_TABLE_STYLES: &str = "application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml";
const CT_CHART: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
const CT_XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// The GUID of the extension holding sections.
const SECTIONS_EXT: &str = "{521415D9-36F7-43E2-AB2F-B90AF26B5E84}";
const NS_P14: &str = "http://schemas.microsoft.com/office/powerpoint/2010/main";

/// Presentation, template or show, with or without macros.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PptFlavor {
    pub macros: bool,
    pub template: bool,
    /// Opens straight into the show (.ppsx).
    pub show: bool,
}

impl PptFlavor {
    /// The flavor of a file name; other extensions keep macros if any.
    pub fn for_path(p: &std::path::Path, m: &apex_doc::Macros) -> PptFlavor {
        let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        match ext.as_str() {
            "pptx" | "pptm" | "potx" | "potm" | "ppsx" | "ppsm" => PptFlavor { macros: ext.ends_with('m'), template: ext.starts_with("pot"), show: ext.starts_with("pps") },
            _ => PptFlavor { macros: m.project.is_some(), ..Default::default() },
        }
    }

    fn main_type(self) -> &'static str {
        match (self.macros, self.template, self.show) {
            (false, true, _) => "application/vnd.openxmlformats-officedocument.presentationml.template.main+xml",
            (true, true, _) => "application/vnd.ms-powerpoint.template.macroEnabled.main+xml",
            (false, _, true) => "application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml",
            (true, _, true) => "application/vnd.ms-powerpoint.slideshow.macroEnabled.main+xml",
            (true, false, false) => "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml",
            (false, false, false) => "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
        }
    }
}

// --- reading --------------------------------------------------------------------------------

/// Children of a part's root kept: a slide, layout or master has about ten
/// (a part made of millions of empty elements would take all the memory).
/// Past that, only the first of each child the reader uses.
const MAX_ROOT_CHILDREN: usize = 256;

fn keep(kept: &[(Node, &str)], n: &Node) -> bool {
    const USED: &[&str] = &["cSld", "clrMap", "clrMapOvr", "txStyles", "sldLayoutIdLst", "transition", "timing", "hf", "AlternateContent", "extLst"];
    kept.len() < MAX_ROOT_CHILDREN || (USED.contains(&n.local()) && !kept.iter().any(|k| k.0.name == n.name))
}

/// The children of a part's root element, each with its source.
fn root_children(xml: &str) -> (Option<Node>, Vec<(Node, &str)>) {
    let mut r = Reader::new(xml);
    let root = loop {
        match r.next() {
            Ev::Start(t) => break crate::drawingml::Node { name: t.name.to_string(), attrs: t.attrs().map(|(k, v)| (k.to_string(), v.into_owned())).collect(), ..Default::default() },
            Ev::Eof => return (None, Vec::new()),
            _ => {}
        }
    };
    let mut out = Vec::new();
    // A part's worth of nodes.
    let mut count = 0;
    loop {
        let before = r.pos();
        match r.next() {
            Ev::Start(t) => {
                let n = crate::drawingml::read_node_within(&mut r, &t, false, &mut count);
                if keep(&out, &n) {
                    out.push((n, &xml[before..r.pos()]));
                }
            }
            Ev::Empty(t) => {
                let n = crate::drawingml::read_node_within(&mut r, &t, true, &mut count);
                if keep(&out, &n) {
                    out.push((n, &xml[before..r.pos()]));
                }
            }
            Ev::End(_) | Ev::Eof => break,
            _ => {}
        }
    }
    (Some(root), out)
}

/// The XML inside the first element named `local` (from after its start
/// tag to after its end tag).
fn inner_of<'a>(xml: &'a str, local: &str) -> Option<&'a str> {
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Start(t) if t.local() == local => return Some(r.raw_element()),
            Ev::Eof => return None,
            _ => {}
        }
    }
}

/// The package part's text.
fn part_text(z: &ZipReader, path: &str) -> Option<String> {
    let b = z.read(path).ok()?;
    Some(crate::ooxml_keep::decode_xml(&b).0)
}

/// A part's relationships of a type (by its last segment), as targets.
fn rel_targets(z: &ZipReader, path: &str, ty: &str) -> Vec<String> {
    read_rels(z, path).into_iter().filter(|r| !r.3 && r.1.rsplit('/').next() == Some(ty)).map(|r| r.2).collect()
}

/// Reads a theme part.
fn read_slide_theme(z: &ZipReader, path: &str) -> SlideTheme {
    let Some(xml) = part_text(z, path) else { return SlideTheme { theme: apex_doc::Theme::default(), fmt: apex_slides::FormatScheme::office(), origin: None } };
    let theme = crate::theme_xml::read_theme(&xml);
    let cx = PartCx::new(z, path, &theme, ClrMap::default(), &HashMap::new());
    let tree = read_fragment(&xml);
    let fmt = tree.find("fmtScheme").map(|f| fill::read_fmt_scheme(f, &cx.media)).unwrap_or_else(apex_slides::FormatScheme::office);
    SlideTheme { theme, fmt, origin: Origin::pptx(path) }
}

/// What a slide, layout or master shares: background, objects, color map
/// override, name.
struct Common {
    name: String,
    background: Option<apex_slides::Background>,
    objects: Vec<apex_slides::Object>,
    map: Option<ClrMap>,
}

fn clr_map(n: &Node) -> ClrMap {
    ClrMap::from_attrs(n.attrs.iter().map(|(k, v)| (crate::xml::local(k), v.as_str())))
}

/// The map override of a `p:clrMapOvr`.
fn map_override(n: &Node) -> Option<ClrMap> {
    n.child("overrideClrMapping").map(clr_map)
}

fn read_common(children: &[(Node, &str)], cx: &mut PartCx) -> Common {
    let mut c = Common { name: String::new(), background: None, objects: Vec::new(), map: None };
    for (n, raw) in children {
        match n.local() {
            "cSld" => {
                c.name = n.attr("name").unwrap_or("").to_string();
                c.background = n.child("bg").and_then(|b| fill::read_background(b, &cx.media));
                if let Some(inner) = inner_of(raw, "spTree") {
                    c.objects = shape::read_tree(inner, cx);
                }
            }
            "clrMapOvr" => c.map = map_override(n),
            _ => {}
        }
    }
    c
}

/// Reads a presentation.
pub fn read(data: &[u8]) -> Result<Deck, Error> {
    let mut deck = {
        let _once = crate::pptx_shape::ReadOnce::new();
        read_all(data)?
    };
    apex_slides::bound(&mut deck);
    Ok(deck)
}

fn read_all(data: &[u8]) -> Result<Deck, Error> {
    let z = ZipReader::new(data)?;
    let ct = ContentTypes::read(&z);
    let pres_path = read_rels(&z, "").into_iter().find(|r| r.1 == REL_OFFICE).map(|r| r.2).unwrap_or_else(|| "ppt/presentation.xml".into());
    let pres_xml = part_text(&z, &pres_path).ok_or_else(|| Error::new("not a presentation: no presentation part"))?;
    let pres = read_fragment(&pres_xml);
    let pres = pres.kids.iter().find(|k| k.local() == "presentation").ok_or_else(|| Error::new("not a presentation"))?;
    let rels: HashMap<String, (String, String, bool)> = read_rels(&z, &pres_path).into_iter().map(|r| (r.0, (r.1, r.2, r.3))).collect();
    let target = |id: Option<&str>| id.and_then(|i| rels.get(i)).filter(|r| !r.2).map(|r| r.1.clone());

    let mut deck = apex_slides::build::new_deck(0);
    deck.slides.clear();
    deck.masters.clear();
    if let Some(s) = pres.child("sldSz") {
        deck.width = (s.num("cx").unwrap_or(9_144_000.0) as f32 / crate::drawingml::EMU_PT).clamp(72.0, 4000.0);
        deck.height = (s.num("cy").unwrap_or(6_858_000.0) as f32 / crate::drawingml::EMU_PT).clamp(72.0, 4000.0);
    }
    if let Some(s) = pres.child("notesSz") {
        deck.notes_size = (s.num("cx").unwrap_or(6_858_000.0) as f32 / crate::drawingml::EMU_PT, s.num("cy").unwrap_or(9_144_000.0) as f32 / crate::drawingml::EMU_PT);
    }
    deck.first_number = pres.num("firstSlideNum").map_or(1, |v| v.max(0.0) as u32);
    if let Some(d) = pres.child("defaultTextStyle") {
        deck.default_style = text::read_list_style(d);
    }
    let mut slide_list: Vec<(u32, String)> = pres.child("sldIdLst").map_or(Vec::new(), |l| l.children("sldId").filter_map(|s| Some((s.num("id")? as u32, target(s.attr("r:id"))?))).collect());
    // A slide listed again is the same part (PowerPoint repairs such a
    // file): once, or a list of the same slide repeated makes thousands.
    let mut seen = std::collections::HashSet::new();
    slide_list.retain(|(_, p)| seen.insert(p.clone()));
    let slide_ids: HashMap<String, u32> = slide_list.iter().map(|(id, p)| (p.clone(), *id)).collect();
    let mut master_paths: Vec<String> = pres.child("sldMasterIdLst").map_or(Vec::new(), |l| l.children("sldMasterId").filter_map(|s| target(s.attr("r:id"))).collect());
    let mut seen = std::collections::HashSet::new();
    master_paths.retain(|p| seen.insert(p.clone()));

    // Masters and their layouts.
    let mut layout_at: HashMap<String, (usize, usize)> = HashMap::new();
    for mp in &master_paths {
        let Some(xml) = part_text(&z, mp) else { continue };
        let theme = rel_targets(&z, mp, "theme").first().map(|t| read_slide_theme(&z, t)).unwrap_or_else(|| SlideTheme { theme: apex_doc::Theme::default(), fmt: apex_slides::FormatScheme::office(), origin: None });
        let (_, kids) = root_children(&xml);
        let map = kids.iter().find(|k| k.0.local() == "clrMap").map_or(ClrMap::default(), |k| clr_map(&k.0));
        let mut cx = PartCx::new(&z, mp, &theme.theme, map, &slide_ids);
        let common = read_common(&kids, &mut cx);
        let (mut title_style, mut body_style, mut other_style) = Default::default();
        if let Some(ts) = kids.iter().find(|k| k.0.local() == "txStyles") {
            title_style = ts.0.child("titleStyle").map(text::read_list_style).unwrap_or_default();
            body_style = ts.0.child("bodyStyle").map(text::read_list_style).unwrap_or_default();
            other_style = ts.0.child("otherStyle").map(text::read_list_style).unwrap_or_default();
        }
        let mrels: HashMap<String, String> = read_rels(&z, mp).into_iter().filter(|r| !r.3).map(|r| (r.0, r.2)).collect();
        let mut layout_paths: Vec<String> = kids.iter().find(|k| k.0.local() == "sldLayoutIdLst").map_or(Vec::new(), |k| k.0.children("sldLayoutId").filter_map(|l| mrels.get(l.attr("r:id")?).cloned()).collect());
        // A layout listed again, or under another master, is read once.
        layout_paths.retain(|p| !layout_at.contains_key(p) && seen.insert(p.clone()));
        let mi = deck.masters.len();
        let mut layouts = Vec::new();
        for lp in &layout_paths {
            let Some(lxml) = part_text(&z, lp) else { continue };
            let (root, lkids) = root_children(&lxml);
            let root = root.unwrap_or_default();
            let lmap = lkids.iter().find(|k| k.0.local() == "clrMapOvr").and_then(|k| map_override(&k.0));
            let mut lcx = PartCx::new(&z, lp, &theme.theme, lmap.unwrap_or(map), &slide_ids);
            let c = read_common(&lkids, &mut lcx);
            layout_at.insert(lp.clone(), (mi, layouts.len()));
            layouts.push(Layout {
                name: c.name,
                kind: Arc::from(root.attr("type").unwrap_or("cust")),
                objects: c.objects,
                background: c.background,
                map: lmap,
                show_master: root.attr("showMasterSp").is_none_or(|v| v != "0" && v != "false"),
                origin: Origin::pptx(lp.clone()),
            });
        }
        deck.masters.push(Arc::new(Master {
            name: theme.theme.name.to_string(),
            theme,
            map,
            background: common.background,
            objects: common.objects,
            title_style,
            body_style,
            other_style,
            layouts,
            origin: Origin::pptx(mp.clone()),
        }));
    }
    if deck.masters.is_empty() {
        let (w, h) = (deck.width, deck.height);
        deck.masters.push(Arc::new(apex_slides::build::new_master(0, w, h)));
    }

    // Slides.
    for (id, sp) in &slide_list {
        let Some(xml) = part_text(&z, sp) else { continue };
        let (root, kids) = root_children(&xml);
        let root = root.unwrap_or_default();
        let (m, l) = rel_targets(&z, sp, "slideLayout").first().and_then(|t| layout_at.get(t).copied()).unwrap_or((0, 0));
        let master = deck.masters[m].clone();
        let lmap = master.layouts.get(l).and_then(|x| x.map);
        let smap = kids.iter().find(|k| k.0.local() == "clrMapOvr").and_then(|k| map_override(&k.0));
        let map = smap.or(lmap).unwrap_or(master.map);
        let mut cx = PartCx::new(&z, sp, &master.theme.theme, map, &slide_ids);
        let c = read_common(&kids, &mut cx);
        let mut s = Slide::new(*id, m, l);
        s.name = c.name;
        s.objects = c.objects;
        s.background = c.background;
        s.map = smap;
        s.hidden = root.attr("show").is_some_and(|v| v == "0" || v == "false");
        s.show_master = root.attr("showMasterSp").is_none_or(|v| v != "0" && v != "false");
        let mut media_nodes = Vec::new();
        for (n, raw) in &kids {
            match n.local() {
                "transition" => s.transition = crate::pptx_anim::read_transition(n, raw),
                "timing" => {
                    s.timing = crate::pptx_anim::read_timing(n, raw);
                    media_nodes = crate::pptx_anim::read_media_nodes(n);
                }
                "AlternateContent" if n.find("transition").is_some() => s.transition = crate::pptx_anim::read_transition(n, raw),
                "AlternateContent" if n.find("timing").is_some() => {
                    if let Some(t) = n.find("timing") {
                        s.timing = crate::pptx_anim::read_timing(t, raw);
                        media_nodes = crate::pptx_anim::read_media_nodes(t);
                    }
                }
                _ => {}
            }
        }
        media_settings(&mut s, &media_nodes);
        if let Some(np) = rel_targets(&z, sp, "notesSlide").first() {
            s.notes = read_notes(&z, np, &master.theme.theme, map, &slide_ids);
        }
        s.origin = Origin::pptx(sp.clone());
        deck.slides.push(Arc::new(s));
    }

    // Sections.
    if let Some(ext) = pres.child("extLst").and_then(|e| e.children("ext").find(|x| x.attr("uri") == Some(SECTIONS_EXT))) {
        let mut secs = Vec::new();
        ext.find_all("section", &mut secs);
        let known: HashSet<u32> = deck.slides.iter().map(|s| s.id).collect();
        for sec in secs {
            let mut ids = Vec::new();
            sec.find_all("sldId", &mut ids);
            deck.sections.push(Section { name: sec.attr("name").unwrap_or("").to_string(), id: sec.attr("id").unwrap_or("").to_string(), slides: ids.iter().filter_map(|i| i.num("id")).map(|v| v as u32).filter(|v| known.contains(v)).collect() });
        }
        deck.sections.retain(|s| !s.slides.is_empty() || deck.slides.is_empty());
    }
    // Show settings.
    if let Some(pp) = rel_targets(&z, &pres_path, "presProps").first().and_then(|p| part_text(&z, p)) {
        let t = read_fragment(&pp);
        if let Some(sh) = t.find("showPr") {
            deck.show = ShowSettings::new(
                sh.attr("loop").is_some_and(|v| v == "1" || v == "true"),
                sh.child("kiosk").is_some(),
                sh.attr("useTimings").is_none_or(|v| v != "0" && v != "false"),
            );
        }
    }
    deck.meta = crate::props::read_ooxml(&z);
    deck.macros = crate::macro_parts::read(&z, "ppt", &z.read_string(&rels_path(&pres_path)).unwrap_or_default());
    // Every part, to write back what isn't changed.
    let mut parts = Vec::new();
    for name in z.names() {
        if name.ends_with('/') {
            continue;
        }
        let Ok(d) = z.read(name) else { continue };
        let ct_s = if name.ends_with(".rels") || name == "[Content_Types].xml" { String::new() } else { ct.get(name) };
        parts.push(KeptPart { path: name.to_string(), content_type: ct_s, data: Arc::from(d) });
    }
    deck.package = Arc::new(Package { src: Some(Src::Pptx), parts });
    if deck.slides.is_empty() && slide_list.is_empty() && master_paths.is_empty() {
        return Err(Error::new("not a presentation"));
    }
    Ok(deck)
}

/// Volume, muting, looping and how they start, from the slide's timing,
/// on its videos and sounds.
fn media_settings(s: &mut Slide, nodes: &[crate::pptx_anim::MediaNode]) {
    // The first node and the first play animation of each shape.
    let mut node_of = std::collections::HashMap::new();
    for n in nodes {
        node_of.entry(n.shape).or_insert(n);
    }
    let mut start_of = std::collections::HashMap::new();
    for a in s.timing.anims.iter().filter(|a| a.class == apex_slides::anim::Class::Media) {
        start_of.entry(a.shape).or_insert(a.trigger);
    }
    for o in &mut s.objects {
        o.walk_mut(&mut |o| {
            let id = o.id;
            if let ObjKind::Picture(p) = &mut o.kind
                && let Some(m) = &mut p.media
            {
                if let Some(n) = node_of.get(&id) {
                    (m.volume, m.mute, m.looped) = (n.volume, n.mute, n.looped);
                }
                m.autoplay = start_of.get(&id).is_some_and(|t| *t != apex_slides::anim::Trigger::OnClick);
            }
        });
    }
}

/// The media nodes a slide's regenerated timing needs.
fn media_of(s: &Slide) -> Vec<crate::pptx_anim::MediaNode> {
    let mut out = Vec::new();
    for o in &s.objects {
        o.walk(&mut |o| {
            if let ObjKind::Picture(p) = &o.kind
                && let Some(m) = &p.media
            {
                out.push(crate::pptx_anim::MediaNode { shape: o.id, video: m.video, volume: m.volume, mute: m.mute, looped: m.looped });
            }
        });
    }
    out
}

fn read_notes(z: &ZipReader, path: &str, theme: &apex_doc::Theme, map: ClrMap, slide_ids: &HashMap<String, u32>) -> Option<Notes> {
    let xml = part_text(z, path)?;
    let cx = PartCx::new(z, path, theme, map, slide_ids);
    let tree = read_fragment(&xml);
    let mut sps = Vec::new();
    tree.find_all("sp", &mut sps);
    let body = sps.into_iter().find(|s| s.find("ph").is_some_and(|p| p.attr("type") == Some("body")))?;
    let t = body.child("txBody").map(|b| text::read_text_body(b, &cx.text_cx())).unwrap_or_default();
    Some(Notes { text: t, origin: Origin::pptx(path) })
}

// --- writing --------------------------------------------------------------------------------

/// The package being written.
struct Pkg<'a> {
    files: BTreeMap<String, Vec<u8>>,
    types: BTreeMap<String, String>,
    /// Relationships of the parts written: path → (id, type, target, external).
    rels: BTreeMap<String, Vec<(String, String, String, bool)>>,
    orig: &'a Package,
    /// Part names taken (the original package's and ours).
    used: BTreeSet<String>,
    /// Pictures and media written, by content.
    media: HashMap<(u64, usize), String>,
    charts: usize,
    diagrams: usize,
    /// Per prefix, the number `fresh` tries first (names below are taken).
    next: HashMap<String, usize>,
}

impl<'a> Pkg<'a> {
    fn new(orig: &'a Package) -> Pkg<'a> {
        let used = orig.parts.iter().map(|p| p.path.to_ascii_lowercase()).collect();
        Pkg { files: BTreeMap::new(), types: BTreeMap::new(), rels: BTreeMap::new(), orig, used, media: HashMap::new(), charts: 0, diagrams: 0, next: HashMap::new() }
    }

    /// A part name not taken: `prefix` + number + `.ext`.
    fn fresh(&mut self, prefix: &str, ext: &str) -> String {
        let key = format!("{prefix}.{ext}");
        let from = self.next.get(&key).copied().unwrap_or(1);
        for n in from.. {
            let p = format!("{prefix}{n}.{ext}");
            if !self.used.contains(&p.to_ascii_lowercase()) {
                self.used.insert(p.to_ascii_lowercase());
                self.next.insert(key, n + 1);
                return p;
            }
        }
        unreachable!()
    }

    fn add(&mut self, path: &str, data: Vec<u8>, ct: &str) {
        self.used.insert(path.to_ascii_lowercase());
        if !ct.is_empty() {
            self.types.insert(path.to_string(), ct.to_string());
        }
        self.files.insert(path.to_string(), data);
    }

    fn orig_part(&self, path: &str) -> Option<&'a KeptPart> {
        self.orig.part(path)
    }

    /// The original relationships of a part.
    fn orig_rels(&self, path: &str) -> Vec<(String, String, String, bool)> {
        let Some(p) = self.orig_part(&rels_path(path)) else { return Vec::new() };
        crate::ooxml_keep::parse_rels(&String::from_utf8_lossy(&p.data), path.rsplit_once('/').map_or("", |x| x.0))
    }

    /// A picture or media file in `ppt/media`; its path.
    fn media_file(&mut self, data: &Arc<[u8]>, ext: Option<&str>) -> String {
        let key = (apex_doc::data_key(data), data.len());
        if let Some(p) = self.media.get(&key) {
            return p.clone();
        }
        let (e, ct) = match ext {
            Some(e) => (e.to_string(), shape::mime_of(&format!("x.{e}")).to_string()),
            None => {
                let (e, ct) = crate::docx_img::image_type(data);
                (e.to_string(), ct.to_string())
            }
        };
        let path = self.fresh(if ext.is_some() { "ppt/media/media" } else { "ppt/media/image" }, &e);
        self.add(&path, data.to_vec(), &ct);
        self.media.insert(key, path.clone());
        path
    }
}

/// Relationships of one part being written.
struct PartW<'p, 'a> {
    pkg: &'p mut Pkg<'a>,
    path: String,
    rels: Vec<(String, String, String, bool)>,
    next: u32,
    /// Slide id → part, for links between slides.
    slide_parts: &'p HashMap<u32, String>,
}

impl<'p, 'a> PartW<'p, 'a> {
    fn new(pkg: &'p mut Pkg<'a>, path: &str, keep: Vec<(String, String, String, bool)>, slide_parts: &'p HashMap<u32, String>) -> Self {
        let next = keep.iter().filter_map(|r| r.0.strip_prefix("rId").and_then(|n| n.parse::<u32>().ok())).max().unwrap_or(0) + 1;
        PartW { pkg, path: path.to_string(), rels: keep, next, slide_parts }
    }

    fn rel(&mut self, ty: &str, target: &str, external: bool) -> String {
        if let Some(r) = self.rels.iter().find(|r| r.1 == ty && r.2 == target && r.3 == external) {
            return r.0.clone();
        }
        let id = loop {
            let id = format!("rId{}", self.next);
            self.next += 1;
            if !self.rels.iter().any(|r| r.0 == id) {
                break id;
            }
        };
        self.rels.push((id.clone(), ty.to_string(), target.to_string(), external));
        id
    }

    fn finish(self) {
        self.pkg.rels.insert(self.path, self.rels);
    }

    /// Drops the relationships kept from the part as read that `xml` (the
    /// part written again) no longer uses: what was removed from a slide
    /// (an embedded workbook, a hidden picture, a link to a file) mustn't
    /// still travel in the package. The layout, notes and comments aren't
    /// named in the part's XML.
    fn prune(&mut self, xml: &str) {
        self.rels.retain(|r| {
            r.1.ends_with("/slideLayout") || r.1.ends_with("/notesSlide") || r.1.ends_with("/comments") || xml.contains(&format!("\"{}\"", r.0)) || xml.contains(&format!("'{}'", r.0))
        });
    }
}

impl Images for PartW<'_, '_> {
    fn image(&mut self, data: &Arc<[u8]>) -> String {
        let p = self.pkg.media_file(data, None);
        self.rel(shape::REL_IMAGE, &p, false)
    }
}

impl Links for PartW<'_, '_> {
    fn link(&mut self, target: &str) -> String {
        if let Some(id) = target.strip_prefix("#slide:").and_then(|s| s.parse::<u32>().ok()) {
            if let Some(p) = self.slide_parts.get(&id).cloned() {
                return self.rel(shape::REL_SLIDE, &p, false);
            }
        }
        self.rel(shape::REL_HYPERLINK, target, true)
    }
}

impl PartOut for PartW<'_, '_> {
    fn chart(&mut self, c: &apex_doc::ChartObject) -> Option<String> {
        let (wb, si, ci) = crate::chart_object::open(&c.book)?;
        let xml = crate::xlsx_chart::write_chart(&wb, si, &wb.sheets[si].charts[ci]);
        self.pkg.charts += 1;
        let path = self.pkg.fresh("ppt/charts/chart", "xml");
        let book = self.pkg.fresh("ppt/embeddings/Microsoft_Excel_Worksheet", "xlsx");
        let link = r#"<c:externalData r:id="rId1"><c:autoUpdate val="0"/></c:externalData>"#;
        let at = xml.rfind("<c:extLst>").filter(|&i| !xml[i..].contains("</c:chart>")).or_else(|| xml.rfind("</c:chartSpace>"))?;
        let xml = format!("{}{link}{}", &xml[..at], &xml[at..]);
        self.pkg.add(&path, xml.into_bytes(), CT_CHART);
        self.pkg.add(&book, c.book.to_vec(), CT_XLSX);
        self.pkg.rels.insert(path.clone(), vec![("rId1".into(), shape::REL_PACKAGE.into(), book, false)]);
        Some(self.rel(shape::REL_CHART, &path, false))
    }

    fn diagram(&mut self, d: &apex_doc::Diagram) -> String {
        self.pkg.diagrams += 1;
        let n = (1..).find(|n| !self.pkg.used.contains(&format!("ppt/diagrams/data{n}.xml"))).unwrap_or(1);
        let from = self.path.rsplit_once('/').map_or("", |x| x.0).to_string();
        let w = crate::diagram::write(d, n, "ppt", "../");
        let mut ids = w.rel_ids.clone();
        // The written relationships, re-made in this part.
        let rels = format!("<r>{}</r>", w.rels);
        let t = read_fragment(&rels);
        for r in t.find("r").map(|r| r.kids.iter().collect::<Vec<_>>()).unwrap_or_default() {
            let (Some(id), Some(ty), Some(tg)) = (r.attr("Id"), r.attr("Type"), r.attr("Target")) else { continue };
            let target = crate::xlsx::resolve_target(&from, tg);
            let new = self.rel(ty, &target, false);
            ids = ids.replace(&format!("\"{id}\""), &format!("\"{new}\""));
        }
        let types = read_fragment(&format!("<t>{}</t>", w.types));
        let ct_of = |p: &str| -> String {
            types.find("t").and_then(|t| t.kids.iter().find(|o| o.attr("PartName").is_some_and(|n| n.trim_start_matches('/') == p))).and_then(|o| o.attr("ContentType")).unwrap_or("").to_string()
        };
        for (path, data) in &w.files {
            let ct = if path.ends_with(".rels") { String::new() } else { ct_of(path) };
            if path.ends_with(".rels") {
                // Diagram part relationships are kept as written.
                self.pkg.files.insert(path.clone(), data.to_vec());
                self.pkg.used.insert(path.to_ascii_lowercase());
            } else {
                self.pkg.add(path, data.to_vec(), &ct);
            }
        }
        ids
    }

    fn media(&mut self, data: &Arc<[u8]>, mime: &str, video: bool) -> (String, String) {
        let ext = match shape::ext_of_mime(mime) {
            "bin" if video => "mp4",
            "bin" => "mp3",
            e => e,
        };
        let p = self.pkg.media_file(data, Some(ext));
        let link = self.rel(if video { shape::REL_VIDEO } else { shape::REL_AUDIO }, &p, false);
        let emb = self.rel(shape::REL_MEDIA, &p, false);
        (link, emb)
    }

    fn media_link(&mut self, target: &str, video: bool) -> String {
        self.rel(if video { shape::REL_VIDEO } else { shape::REL_AUDIO }, target, true)
    }
}

/// The namespace declarations of a part's root as read (raw objects use
/// their prefixes), with ours.
fn root_ns(pkg: &Pkg, path: Option<&str>) -> String {
    let mut decl: Vec<(String, String)> = vec![("xmlns:a".into(), NS_A.into()), ("xmlns:r".into(), NS_R.into()), ("xmlns:p".into(), NS_P.into())];
    if let Some(p) = path.and_then(|p| pkg.orig_part(p)) {
        let xml = String::from_utf8_lossy(&p.data);
        let mut r = Reader::new(&xml);
        loop {
            match r.next() {
                Ev::Start(t) | Ev::Empty(t) => {
                    for (k, v) in t.attrs() {
                        if (k.starts_with("xmlns:") || k == "mc:Ignorable") && !decl.iter().any(|d| d.0 == k) {
                            decl.push((k.to_string(), v.into_owned()));
                        }
                    }
                    break;
                }
                Ev::Eof => break,
                _ => {}
            }
        }
    }
    let mut s = String::new();
    for (k, v) in decl {
        let _ = write!(s, r#" {k}="{}""#, esc(&v));
    }
    s
}

const TREE_START: &str = r#"<p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

/// `p:cSld` with its background and objects.
fn write_csld(out: &mut String, name: &str, bg: Option<&apex_slides::Background>, objects: &[apex_slides::Object], w: &mut PartW) {
    out.push_str("<p:cSld");
    if !name.is_empty() {
        let _ = write!(out, r#" name="{}""#, esc(name));
    }
    out.push('>');
    if let Some(b) = bg {
        fill::write_background(out, b, w);
    }
    out.push_str(TREE_START);
    shape::write_tree(out, objects, w);
    out.push_str("</p:spTree></p:cSld>");
}

fn map_ovr(out: &mut String, m: Option<ClrMap>) {
    match m {
        Some(m) => {
            let _ = write!(out, "<p:clrMapOvr><a:overrideClrMapping{}/></p:clrMapOvr>", m.attrs());
        }
        None => out.push_str("<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>"),
    }
}

/// Whether a part goes back as it was read.
fn same(o: &Option<Origin>, pkg: &Pkg) -> bool {
    o.as_ref().is_some_and(|o| o.same && o.src == Src::Pptx && pkg.orig_part(&o.part).is_some())
}

fn orig_path(o: &Option<Origin>, pkg: &Pkg) -> Option<String> {
    o.as_ref().filter(|o| o.src == Src::Pptx && pkg.orig_part(&o.part).is_some()).map(|o| o.part.clone())
}

/// Copies an original part and records its original relationships.
fn copy_orig(pkg: &mut Pkg, path: &str, ct: &str) {
    let Some(p) = pkg.orig_part(path) else { return };
    let rels = pkg.orig_rels(path);
    let ct = if p.content_type.is_empty() { ct.to_string() } else { p.content_type.clone() };
    pkg.add(path, p.data.to_vec(), &ct);
    pkg.rels.insert(path.to_string(), rels);
}

/// Replaces the element `tag` (`<tag .../>` or `<tag>...</tag>`) in `xml`.
fn replace_el(xml: &str, tag: &str, new: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}")).filter(|&i| xml[i + tag.len() + 1..].starts_with([' ', '>', '/']))?;
    let head_end = start + xml[start..].find('>')? + 1;
    let end = if xml[..head_end].ends_with("/>") { head_end } else { head_end + xml[head_end..].find(&format!("</{tag}>"))? + tag.len() + 3 };
    Some(format!("{}{new}{}", &xml[..start], &xml[end..]))
}

/// Writes a presentation (macros kept if there are any).
pub fn write(deck: &Deck) -> Vec<u8> {
    write_as(deck, PptFlavor { macros: deck.macros.project.is_some(), ..Default::default() })
}

/// Writes a presentation in a flavor (template, show, macro-enabled).
pub fn write_as(deck: &Deck, flavor: PptFlavor) -> Vec<u8> {
    let empty = Package::default();
    let orig: &Package = if deck.package.src == Some(Src::Pptx) { &deck.package } else { &empty };
    let mut pkg = Pkg::new(orig);
    let orig_pres = orig.parts.iter().find(|p| p.content_type.contains("presentationml") && p.content_type.ends_with(".main+xml")).map(|p| p.path.clone());
    let pres_path = orig_pres.clone().unwrap_or_else(|| "ppt/presentation.xml".into());
    pkg.used.insert(pres_path.to_ascii_lowercase());

    // Where masters, layouts, themes and slides go.
    let orig_masters: Vec<String> = orig_pres.as_ref().map_or(Vec::new(), |p| {
        let xml = pkg.orig_part(p).map(|x| String::from_utf8_lossy(&x.data).into_owned()).unwrap_or_default();
        let rels: HashMap<String, String> = pkg.orig_rels(p).into_iter().map(|r| (r.0, r.2)).collect();
        let t = read_fragment(&xml);
        t.find("sldMasterIdLst").map_or(Vec::new(), |l| l.children("sldMasterId").filter_map(|m| rels.get(m.attr("r:id")?).cloned()).collect())
    });
    let deck_masters: Vec<Option<String>> = deck.masters.iter().map(|m| orig_path(&m.origin, &pkg)).collect();
    let masters_same = !orig_masters.is_empty() && deck_masters.iter().map(|m| m.clone().unwrap_or_default()).collect::<Vec<_>>() == orig_masters && deck.masters.iter().all(|m| same(&m.origin, &pkg) && m.layouts.iter().all(|l| l.origin.as_ref().is_some_and(|o| pkg.orig_part(&o.part).is_some())));
    let mut master_paths = Vec::new();
    let mut layout_paths: Vec<Vec<String>> = Vec::new();
    let mut theme_paths = Vec::new();
    for m in &deck.masters {
        let mp = orig_path(&m.origin, &pkg).filter(|p| !master_paths.contains(p)).unwrap_or_else(|| pkg.fresh("ppt/slideMasters/slideMaster", "xml"));
        master_paths.push(mp);
        let tp = orig_path(&m.theme.origin, &pkg).filter(|p| !theme_paths.contains(p)).unwrap_or_else(|| pkg.fresh("ppt/theme/theme", "xml"));
        theme_paths.push(tp);
        let mut lps = Vec::new();
        for l in &m.layouts {
            let lp = orig_path(&l.origin, &pkg).filter(|p| !lps.contains(p) && !layout_paths.iter().flatten().any(|x| x == p)).unwrap_or_else(|| pkg.fresh("ppt/slideLayouts/slideLayout", "xml"));
            lps.push(lp);
        }
        layout_paths.push(lps);
    }
    let mut slide_paths = Vec::new();
    for s in &deck.slides {
        let sp = orig_path(&s.origin, &pkg).filter(|p| !slide_paths.contains(p)).unwrap_or_else(|| pkg.fresh("ppt/slides/slide", "xml"));
        slide_paths.push(sp);
    }
    let slide_parts: HashMap<u32, String> = deck.slides.iter().zip(&slide_paths).map(|(s, p)| (s.id, p.clone())).collect();

    // Themes.
    for (m, tp) in deck.masters.iter().zip(&theme_paths) {
        if same(&m.theme.origin, &pkg) && orig_path(&m.theme.origin, &pkg).as_deref() == Some(tp.as_str()) {
            copy_orig(&mut pkg, tp, crate::theme_xml::CONTENT_TYPE);
        } else {
            let new = crate::theme_xml::write_theme(&m.theme.theme);
            let xml = match orig_path(&m.theme.origin, &pkg).and_then(|p| pkg.orig_part(&p)) {
                Some(o) => patch_theme(&String::from_utf8_lossy(&o.data), &new),
                None => new,
            };
            let keep = orig_path(&m.theme.origin, &pkg).map(|p| pkg.orig_rels(&p)).unwrap_or_default();
            pkg.add(tp, xml.into_bytes(), crate::theme_xml::CONTENT_TYPE);
            pkg.rels.insert(tp.clone(), keep);
        }
    }

    // Masters and layouts.
    let mut next_id: u64 = 2_147_483_648;
    let mut master_ids = Vec::new();
    for (mi, m) in deck.masters.iter().enumerate() {
        let mp = master_paths[mi].clone();
        let master_id = next_id;
        next_id += 1;
        master_ids.push(master_id);
        let mut layout_ids = Vec::new();
        for (li, l) in m.layouts.iter().enumerate() {
            let lp = layout_paths[mi][li].clone();
            layout_ids.push(next_id);
            next_id += 1;
            if masters_same && same(&l.origin, &pkg) {
                copy_orig(&mut pkg, &lp, CT_LAYOUT);
                continue;
            }
            let keep: Vec<_> = orig_path(&l.origin, &pkg).map(|p| pkg.orig_rels(&p)).unwrap_or_default().into_iter().filter(|r| !r.1.ends_with("/slideMaster")).collect();
            let mut w = PartW::new(&mut pkg, &lp, keep, &slide_parts);
            w.rel(&format!("{R}slideMaster"), &mp, false);
            let mut x = format!("{HEADER}<p:sldLayout{}", root_ns(w.pkg, orig_path(&l.origin, w.pkg).as_deref()));
            if !l.kind.is_empty() && &*l.kind != "cust" {
                let _ = write!(x, r#" type="{}""#, esc(&l.kind));
            }
            if !l.show_master {
                x.push_str(r#" showMasterSp="0""#);
            }
            x.push_str(r#" preserve="1">"#);
            write_csld(&mut x, &l.name, l.background.as_ref(), &l.objects, &mut w);
            map_ovr(&mut x, l.map);
            x.push_str("</p:sldLayout>");
            w.finish();
            pkg.add(&lp, x.into_bytes(), CT_LAYOUT);
        }
        if masters_same && same(&m.origin, &pkg) {
            copy_orig(&mut pkg, &mp, CT_MASTER);
            // Its theme may have moved: point at where it is now.
            if let Some(r) = pkg.rels.get_mut(&mp) {
                for rel in r.iter_mut().filter(|r| r.1.ends_with("/theme")) {
                    rel.2 = theme_paths[mi].clone();
                }
            }
            continue;
        }
        let keep: Vec<_> = orig_path(&m.origin, &pkg).map(|p| pkg.orig_rels(&p)).unwrap_or_default().into_iter().filter(|r| !r.1.ends_with("/slideLayout") && !r.1.ends_with("/theme")).collect();
        let mut w = PartW::new(&mut pkg, &mp, keep, &slide_parts);
        let mut x = format!("{HEADER}<p:sldMaster{}>", root_ns(w.pkg, orig_path(&m.origin, w.pkg).as_deref()));
        write_csld(&mut x, "", m.background.as_ref(), &m.objects, &mut w);
        let _ = write!(x, "<p:clrMap{}/>", m.map.attrs());
        x.push_str("<p:sldLayoutIdLst>");
        for (li, lp) in layout_paths[mi].iter().enumerate() {
            let id = w.rel(&format!("{R}slideLayout"), lp, false);
            let _ = write!(x, r#"<p:sldLayoutId id="{}" r:id="{id}"/>"#, layout_ids[li]);
        }
        x.push_str("</p:sldLayoutIdLst><p:txStyles>");
        text::write_list_style(&mut x, "p:titleStyle", &m.title_style);
        text::write_list_style(&mut x, "p:bodyStyle", &m.body_style);
        text::write_list_style(&mut x, "p:otherStyle", &m.other_style);
        x.push_str("</p:txStyles></p:sldMaster>");
        w.rel(&format!("{R}theme"), &theme_paths[mi], false);
        w.finish();
        pkg.add(&mp, x.into_bytes(), CT_MASTER);
    }

    // Notes master (when a slide has notes).
    let orig_nm = orig_pres.as_ref().and_then(|p| pkg.orig_rels(p).into_iter().find(|r| r.1.ends_with("/notesMaster")).map(|r| r.2)).filter(|p| pkg.orig_part(p).is_some());
    let any_notes = deck.slides.iter().any(|s| s.notes.is_some());
    let notes_master = match (&orig_nm, any_notes) {
        (Some(p), _) => {
            copy_orig(&mut pkg, p, CT_NOTES_MASTER);
            Some(p.clone())
        }
        (None, true) => Some(write_notes_master(&mut pkg, deck)),
        (None, false) => None,
    };

    // Slides and notes.
    for (i, s) in deck.slides.iter().enumerate() {
        let sp = slide_paths[i].clone();
        let (m, l) = (s.master.min(deck.masters.len() - 1), s.layout);
        let lp = layout_paths[m].get(l).or_else(|| layout_paths[m].first()).cloned().unwrap_or_default();
        let notes_path = s.notes.as_ref().map(|n| {
            orig_path(&n.origin, &pkg).filter(|_| same(&s.origin, &pkg)).unwrap_or_else(|| {
                orig_path(&n.origin, &pkg).filter(|p| !pkg.files.contains_key(p)).unwrap_or_else(|| pkg.fresh("ppt/notesSlides/notesSlide", "xml"))
            })
        });
        if same(&s.origin, &pkg) && orig_path(&s.origin, &pkg).as_deref() == Some(sp.as_str()) {
            copy_orig(&mut pkg, &sp, CT_SLIDE);
            if let Some(r) = pkg.rels.get_mut(&sp) {
                for rel in r.iter_mut().filter(|r| r.1.ends_with("/slideLayout")) {
                    rel.2 = lp.clone();
                }
            }
        } else {
            let keep: Vec<_> = orig_path(&s.origin, &pkg).map(|p| pkg.orig_rels(&p)).unwrap_or_default().into_iter().filter(|r| !r.1.ends_with("/slideLayout") && !r.1.ends_with("/notesSlide")).collect();
            let mut w = PartW::new(&mut pkg, &sp, keep, &slide_parts);
            w.rel(&format!("{R}slideLayout"), &lp, false);
            let mut x = format!("{HEADER}<p:sld{}", root_ns(w.pkg, orig_path(&s.origin, w.pkg).as_deref()));
            if s.hidden {
                x.push_str(r#" show="0""#);
            }
            if !s.show_master {
                x.push_str(r#" showMasterSp="0""#);
            }
            x.push('>');
            write_csld(&mut x, &s.name, s.background.as_ref(), &s.objects, &mut w);
            map_ovr(&mut x, s.map);
            crate::pptx_anim::write_transition(&mut x, &s.transition);
            crate::pptx_anim::write_timing(&mut x, &s.timing, &media_of(s));
            x.push_str("</p:sld>");
            if let Some(np) = &notes_path {
                w.rel(&format!("{R}notesSlide"), np, false);
            }
            w.prune(&x);
            w.finish();
            pkg.add(&sp, x.into_bytes(), CT_SLIDE);
        }
        if let (Some(n), Some(np)) = (&s.notes, &notes_path) {
            if same(&n.origin, &pkg) && orig_path(&n.origin, &pkg).as_deref() == Some(np.as_str()) && !pkg.files.contains_key(np) {
                copy_orig(&mut pkg, np, CT_NOTES);
                if let Some(r) = pkg.rels.get_mut(np) {
                    for rel in r.iter_mut().filter(|r| r.1.ends_with("/slide")) {
                        rel.2 = sp.clone();
                    }
                }
            } else {
                write_notes(&mut pkg, np, &sp, notes_master.as_deref().unwrap_or(""), n, &slide_parts);
            }
        }
    }

    // The presentation part.
    // Slides and masters where they were keep their ids (custom shows
    // point at them).
    let pres_keep: Vec<_> = orig_pres
        .as_ref()
        .map(|p| pkg.orig_rels(p))
        .unwrap_or_default()
        .into_iter()
        .filter(|r| (!r.1.ends_with("/slide") || slide_paths.contains(&r.2)) && (!r.1.ends_with("/slideMaster") || master_paths.contains(&r.2)) && !r.1.ends_with("/theme") && !(r.1 == crate::macro_parts::REL_VBA && !flavor.macros))
        // Parts taken out of the package (comment authors, once comments
        // are removed) aren't pointed at.
        .filter(|r| r.3 || pkg.orig_part(&r.2).is_some())
        .collect();
    let mut w = PartW::new(&mut pkg, &pres_path, pres_keep, &slide_parts);
    let mut master_list = String::from("<p:sldMasterIdLst>");
    for (mi, mp) in master_paths.iter().enumerate() {
        let id = w.rel(&format!("{R}slideMaster"), mp, false);
        let _ = write!(master_list, r#"<p:sldMasterId id="{}" r:id="{id}"/>"#, master_ids[mi]);
    }
    master_list.push_str("</p:sldMasterIdLst>");
    let mut slide_list = String::from("<p:sldIdLst>");
    for (s, sp) in deck.slides.iter().zip(&slide_paths) {
        let id = w.rel(&format!("{R}slide"), sp, false);
        let _ = write!(slide_list, r#"<p:sldId id="{}" r:id="{id}"/>"#, s.id);
    }
    slide_list.push_str("</p:sldIdLst>");
    let nm_list = notes_master.as_ref().map(|p| {
        let id = w.rel(&format!("{R}notesMaster"), p, false);
        format!(r#"<p:notesMasterIdLst><p:notesMasterId r:id="{id}"/></p:notesMasterIdLst>"#)
    });
    w.rel(&format!("{R}theme"), &theme_paths[0], false);
    let size = format!(r#"<p:sldSz cx="{}" cy="{}"/>"#, (deck.width * crate::drawingml::EMU_PT).round() as i64, (deck.height * crate::drawingml::EMU_PT).round() as i64);
    let notes_size = format!(r#"<p:notesSz cx="{}" cy="{}"/>"#, (deck.notes_size.0 * crate::drawingml::EMU_PT).round() as i64, (deck.notes_size.1 * crate::drawingml::EMU_PT).round() as i64);
    let sections = sections_ext(&deck.sections);
    let orig_xml = orig_pres.as_ref().and_then(|p| w.pkg.orig_part(p)).map(|p| String::from_utf8_lossy(&p.data).into_owned());
    let patched = orig_xml.and_then(|x| {
        let x = replace_el(&x, "p:sldMasterIdLst", &master_list)?;
        let x = match replace_el(&x, "p:sldIdLst", &slide_list) {
            Some(x) => x,
            None => {
                let at = ["</p:handoutMasterIdLst>", "</p:notesMasterIdLst>", "</p:sldMasterIdLst>"].iter().find_map(|t| x.find(t).map(|i| i + t.len()))?;
                format!("{}{slide_list}{}", &x[..at], &x[at..])
            }
        };
        let x = match (&nm_list, x.contains("<p:notesMasterIdLst")) {
            (Some(l), false) => {
                let at = x.find("</p:sldMasterIdLst>")? + "</p:sldMasterIdLst>".len();
                format!("{}{l}{}", &x[..at], &x[at..])
            }
            _ => x,
        };
        let orig_size = read_fragment(&x).find("sldSz").map(|s| (s.num("cx").unwrap_or(0.0), s.num("cy").unwrap_or(0.0)));
        let want = ((deck.width * crate::drawingml::EMU_PT).round() as f64, (deck.height * crate::drawingml::EMU_PT).round() as f64);
        let x = if orig_size.is_some_and(|o| (o.0 - want.0).abs() > 1.0 || (o.1 - want.1).abs() > 1.0) { replace_el(&x, "p:sldSz", &size)? } else { x };
        let x = drop_show_refs(&x, &w.rels);
        let mut x = remove_ext(&x, SECTIONS_EXT);
        if !sections.is_empty() {
            match x.rfind("</p:extLst>") {
                Some(i) if x[i..].starts_with("</p:extLst></p:presentation>") || x[i..].trim_end().starts_with("</p:extLst>") => x.insert_str(i, &sections),
                _ => {
                    let at = x.rfind("</p:presentation>")?;
                    x.insert_str(at, &format!("<p:extLst>{sections}</p:extLst>"));
                }
            }
        }
        Some(x)
    });
    let pres_xml = patched.unwrap_or_else(|| {
        let mut x = format!(r#"{HEADER}<p:presentation xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}" saveSubsetFonts="1""#);
        if deck.first_number != 1 {
            let _ = write!(x, r#" firstSlideNum="{}""#, deck.first_number);
        }
        x.push('>');
        x.push_str(&master_list);
        if let Some(l) = &nm_list {
            x.push_str(l);
        }
        x.push_str(&slide_list);
        x.push_str(&size);
        x.push_str(&notes_size);
        text::write_list_style(&mut x, "p:defaultTextStyle", &deck.default_style);
        if !sections.is_empty() {
            let _ = write!(x, "<p:extLst>{sections}</p:extLst>");
        }
        x.push_str("</p:presentation>");
        x
    });
    // presProps, viewProps and table styles.
    let has = |w: &PartW, t: &str| w.rels.iter().any(|r| r.1.ends_with(t));
    if !has(&w, "/presProps") || deck.show.changed() {
        let p = w.rels.iter().find(|r| r.1.ends_with("/presProps")).map(|r| r.2.clone()).unwrap_or_else(|| "ppt/presProps.xml".into());
        let mut x = format!(r#"{HEADER}<p:presentationPr xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}">"#);
        let sh = &deck.show;
        if sh.looped || sh.kiosk || !sh.use_timings {
            x.push_str("<p:showPr");
            if sh.looped {
                x.push_str(r#" loop="1""#);
            }
            if !sh.use_timings {
                x.push_str(r#" useTimings="0""#);
            }
            x.push_str(if sh.kiosk { "><p:kiosk/><p:sldAll/></p:showPr>" } else { "><p:present/><p:sldAll/></p:showPr>" });
        }
        x.push_str("</p:presentationPr>");
        w.pkg.add(&p, x.into_bytes(), CT_PRES_PROPS);
        w.pkg.rels.insert(p.clone(), Vec::new());
        w.rel(&format!("{R}presProps"), &p, false);
    }
    if !has(&w, "/viewProps") {
        let x = format!(r#"{HEADER}<p:viewPr xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:normalViewPr><p:restoredLeft sz="15620"/><p:restoredTop sz="94660"/></p:normalViewPr><p:gridSpacing cx="76200" cy="76200"/></p:viewPr>"#);
        w.pkg.add("ppt/viewProps.xml", x.into_bytes(), CT_VIEW_PROPS);
        w.pkg.rels.insert("ppt/viewProps.xml".into(), Vec::new());
        w.rel(&format!("{R}viewProps"), "ppt/viewProps.xml", false);
    }
    if !has(&w, "/tableStyles") {
        let x = format!(r#"{HEADER}<a:tblStyleLst xmlns:a="{NS_A}" def="{}"/>"#, apex_slides::table::DEFAULT_STYLE);
        w.pkg.add("ppt/tableStyles.xml", x.into_bytes(), CT_TABLE_STYLES);
        w.pkg.rels.insert("ppt/tableStyles.xml".into(), Vec::new());
        w.rel(&format!("{R}tableStyles"), "ppt/tableStyles.xml", false);
    }
    // The VBA project: as read, or edited (or new) in the macro editor.
    if let (true, Some(bin)) = (flavor.macros, &deck.macros.project) {
        let path = w.rels.iter().find(|r| r.1 == crate::macro_parts::REL_VBA).map(|r| r.2.clone()).unwrap_or_else(|| "ppt/vbaProject.bin".into());
        if w.pkg.orig_part(&path).is_none_or(|o| *o.data != bin[..]) {
            // Changed: the signatures, if any, no longer match.
            w.pkg.add(&path, bin.to_vec(), "application/vnd.ms-office.vbaProject");
            w.pkg.rels.insert(path.clone(), Vec::new());
        }
        w.rel(crate::macro_parts::REL_VBA, &path, false);
    }
    w.finish();
    pkg.add(&pres_path, pres_xml.into_bytes(), flavor.main_type());

    // The package relationships: ours, then the kept ones.
    let mut root: Vec<(String, String, String, bool)> = vec![("rId1".into(), REL_OFFICE.into(), pres_path.clone(), false), ("rId2".into(), REL_CORE.into(), "docProps/core.xml".into(), false), ("rId3".into(), REL_APP.into(), "docProps/app.xml".into(), false)];
    if !deck.meta.custom.is_empty() {
        root.push(("rId4".into(), REL_CUSTOM_PROPS.into(), "docProps/custom.xml".into(), false));
    }
    let orig_root = if orig.parts.is_empty() { Vec::new() } else { pkg.orig_rels("") };
    for (k, r) in orig_root.into_iter().enumerate() {
        // Ours, also under other spellings (some writers lowercase them).
        let ty = r.1.to_ascii_lowercase();
        if [REL_OFFICE, REL_CORE, REL_APP, REL_CUSTOM_PROPS].iter().any(|t| t.to_ascii_lowercase() == ty) || ty.ends_with("/core-properties") || ty.ends_with("/extended-properties") || ty.ends_with("/custom-properties") || ty.ends_with("/officedocument") || ty.ends_with("/extendedproperties") || r.2 == pres_path || ["docProps/core.xml", "docProps/app.xml", "docProps/custom.xml"].contains(&r.2.as_str()) {
            continue;
        }
        let id = if root.iter().any(|x| x.0 == r.0) { format!("rIdK{}", k + 1) } else { r.0 };
        root.push((id, r.1, r.2, r.3));
    }
    pkg.rels.insert(String::new(), root);

    // A jump to a slide no longer there would bring it back whole (its
    // notes, comments and objects) though it isn't shown: it goes to the
    // slide it's on instead.
    for (p, rels) in pkg.rels.iter_mut() {
        if slide_paths.contains(p) {
            for r in rels.iter_mut().filter(|r| !r.3 && r.1.ends_with("/slide") && !slide_paths.contains(&r.2)) {
                r.2 = p.clone();
            }
        }
    }
    // What the relationships reach: written parts, and original ones copied.
    let mut todo: Vec<String> = pkg.rels.get("").map(|r| r.iter().filter(|x| !x.3).map(|x| x.2.clone()).collect()).unwrap_or_default();
    let mut seen = BTreeSet::new();
    while let Some(p) = todo.pop() {
        if !seen.insert(p.clone()) {
            continue;
        }
        if !pkg.files.contains_key(&p) {
            if p.starts_with("docProps/") && ["docProps/core.xml", "docProps/app.xml", "docProps/custom.xml"].contains(&p.as_str()) {
                continue;
            }
            // Slides deleted stay deleted, whatever still points at them.
            if p.starts_with("ppt/slides/") && !slide_paths.contains(&p) {
                continue;
            }
            let Some(o) = pkg.orig_part(&p) else { continue };
            let (ct, data) = (o.content_type.clone(), o.data.to_vec());
            let rels = pkg.orig_rels(&p);
            pkg.add(&p, data, &ct);
            if !rels.is_empty() || pkg.orig_part(&rels_path(&p)).is_some() {
                // Kept as written (a part we didn't change).
                if let Some(rp) = pkg.orig_part(&rels_path(&p)) {
                    let d = rp.data.to_vec();
                    pkg.files.insert(rels_path(&p), d);
                }
                todo.extend(rels.iter().filter(|r| !r.3).map(|r| r.2.clone()));
            }
            continue;
        }
        if let Some(r) = pkg.rels.get(&p) {
            todo.extend(r.iter().filter(|x| !x.3).map(|x| x.2.clone()));
        } else if let Some(rp) = pkg.files.get(&rels_path(&p)) {
            // Parts with relationships written as files (diagrams).
            let dir = p.rsplit_once('/').map_or("", |x| x.0);
            todo.extend(crate::ooxml_keep::parse_rels(&String::from_utf8_lossy(rp), dir).into_iter().filter(|r| !r.3).map(|r| r.2));
        }
    }
    // Relationship parts of what we wrote.
    let rel_parts: Vec<(String, String)> = pkg
        .rels
        .iter()
        .filter(|(p, _)| p.is_empty() || seen.contains(*p))
        .map(|(p, rels)| {
            let dir = p.rsplit_once('/').map_or("", |x| x.0);
            let mut x = format!(r#"{HEADER}<Relationships xmlns="{RELS_NS}">"#);
            for (id, ty, target, ext) in rels {
                if *ext {
                    let _ = write!(x, r#"<Relationship Id="{}" Type="{}" Target="{}" TargetMode="External"/>"#, esc(id), esc(ty), esc(target));
                } else {
                    let _ = write!(x, r#"<Relationship Id="{}" Type="{}" Target="{}"/>"#, esc(id), esc(ty), esc(&relative(dir, target)));
                }
            }
            x.push_str("</Relationships>");
            (rels_path(p).trim_start_matches('/').to_string(), x)
        })
        .collect();
    let rel_parts: Vec<(String, String)> = rel_parts.into_iter().map(|(p, x)| (if p == "_rels/.rels" || p.is_empty() { "_rels/.rels".into() } else { p }, x)).collect();

    let mut z = ZipWriter::new();
    let mut ct = format!(r#"{HEADER}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>"#);
    ct.push_str(r#"<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#);
    ct.push_str(crate::props::content_types(&deck.meta));
    for (p, t) in &pkg.types {
        if seen.contains(p) || p == &pres_path {
            let _ = write!(ct, r#"<Override PartName="/{}" ContentType="{}"/>"#, esc(p), esc(t));
        }
    }
    ct.push_str("</Types>");
    z.add("[Content_Types].xml", ct.as_bytes(), true);
    for (p, x) in &rel_parts {
        z.add(p, x.as_bytes(), true);
    }
    let written_rels: BTreeSet<&str> = rel_parts.iter().map(|x| x.0.as_str()).collect();
    for (p, d) in &pkg.files {
        let reached = seen.contains(p) || p == &pres_path;
        let is_rels_of_reached = p.contains("/_rels/") && !written_rels.contains(p.as_str()) && {
            let (dir, file) = p.rsplit_once("/_rels/").unwrap_or(("", ""));
            let target = if dir.is_empty() { file.trim_end_matches(".rels").to_string() } else { format!("{dir}/{}", file.trim_end_matches(".rels")) };
            seen.contains(&target)
        };
        if reached || is_rels_of_reached {
            let media = p.starts_with("ppt/media/");
            z.add(p, d, !media);
        }
    }
    crate::props::write_ooxml(&mut z, &deck.meta);
    z.finish()
}

/// The theme's color and font schemes put into the theme part as read.
fn patch_theme(orig: &str, new: &str) -> String {
    let mut out = orig.to_string();
    for tag in ["a:clrScheme", "a:fontScheme"] {
        let piece = |s: &str| -> Option<(usize, usize)> {
            let a = s.find(&format!("<{tag}"))?;
            let b = s[a..].find(&format!("</{tag}>"))? + a + tag.len() + 3;
            Some((a, b))
        };
        if let (Some((a, b)), Some((c, d))) = (piece(&out), piece(new)) {
            out = format!("{}{}{}", &out[..a], &new[c..d], &out[b..]);
        }
    }
    out
}

/// Custom shows without the slides deleted (`p:sld` whose relationship
/// is gone).
fn drop_show_refs(xml: &str, rels: &[(String, String, String, bool)]) -> String {
    let Some(a) = xml.find("<p:custShowLst") else { return xml.to_string() };
    let Some(b) = xml[a..].find("</p:custShowLst>").map(|b| a + b) else { return xml.to_string() };
    let mut list = xml[a..b].to_string();
    let mut from = 0;
    while let Some(i) = list[from..].find("<p:sld ").map(|i| from + i) {
        let Some(e) = list[i..].find("/>").map(|e| i + e + 2) else { break };
        let id = list[i..e].split("r:id=\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("");
        if rels.iter().any(|r| r.0 == id && r.1.ends_with("/slide")) {
            from = e;
        } else {
            list.replace_range(i..e, "");
            from = i;
        }
    }
    format!("{}{list}{}", &xml[..a], &xml[b..])
}

/// Removes an extension (`p:ext` with `uri`) from a part.
fn remove_ext(xml: &str, uri: &str) -> String {
    let Some(a) = xml.find(&format!(r#"<p:ext uri="{uri}""#)) else { return xml.to_string() };
    let Some(b) = xml[a..].find("</p:ext>").map(|b| a + b + 8) else { return xml.to_string() };
    let mut out = format!("{}{}", &xml[..a], &xml[b..]);
    out = out.replace("<p:extLst></p:extLst>", "");
    out
}

/// The sections extension.
fn sections_ext(sections: &[Section]) -> String {
    if sections.is_empty() {
        return String::new();
    }
    let mut s = format!(r#"<p:ext uri="{SECTIONS_EXT}"><p14:sectionLst xmlns:p14="{NS_P14}">"#);
    for sec in sections {
        let _ = write!(s, r#"<p14:section name="{}" id="{}"><p14:sldIdLst>"#, esc(&sec.name), esc(&sec.id));
        for id in &sec.slides {
            let _ = write!(s, r#"<p14:sldId id="{id}"/>"#);
        }
        s.push_str("</p14:sldIdLst></p14:section>");
    }
    s.push_str("</p14:sectionLst></p:ext>");
    s
}

/// A notes master for a presentation that had none.
fn write_notes_master(pkg: &mut Pkg, deck: &Deck) -> String {
    let path = pkg.fresh("ppt/notesMasters/notesMaster", "xml");
    let theme = pkg.fresh("ppt/theme/theme", "xml");
    pkg.add(&theme, crate::theme_xml::write_theme(&deck.masters[0].theme.theme).into_bytes(), crate::theme_xml::CONTENT_TYPE);
    pkg.rels.insert(theme.clone(), Vec::new());
    let (w, h) = (deck.notes_size.0 * crate::drawingml::EMU_PT, deck.notes_size.1 * crate::drawingml::EMU_PT);
    let e = |v: f32| v.round() as i64;
    let mut x = format!(r#"{HEADER}<p:notesMaster xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg>{TREE_START}"#);
    let _ = write!(
        x,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide Image Placeholder 1"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg" idx="2"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/><a:ln w="12700"><a:solidFill><a:prstClr val="black"/></a:solidFill></a:ln></p:spPr></p:sp>"#,
        e(w * 0.1),
        e(h * 0.075),
        e(w * 0.8),
        e(w * 0.8 * deck.height / deck.width)
    );
    let _ = write!(
        x,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" sz="quarter" idx="3"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp>"#,
        e(w * 0.1),
        e(h * 0.48),
        e(w * 0.8),
        e(h * 0.39)
    );
    let _ = write!(x, "</p:spTree></p:cSld><p:clrMap{}/>", ClrMap::default().attrs());
    x.push_str(r#"<p:notesStyle><a:lvl1pPr marL="0" algn="l" rtl="0"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl1pPr></p:notesStyle></p:notesMaster>"#);
    pkg.add(&path, x.into_bytes(), CT_NOTES_MASTER);
    pkg.rels.insert(path.clone(), vec![("rId1".into(), format!("{R}theme"), theme, false)]);
    path
}

/// A notes slide.
fn write_notes(pkg: &mut Pkg, path: &str, slide: &str, master: &str, n: &Notes, slide_parts: &HashMap<u32, String>) {
    let mut w = PartW::new(pkg, path, Vec::new(), slide_parts);
    if !master.is_empty() {
        w.rel(&format!("{R}notesMaster"), master, false);
    }
    w.rel(&format!("{R}slide"), slide, false);
    let mut x = format!(r#"{HEADER}<p:notes xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:cSld>{TREE_START}"#);
    x.push_str(r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide Image Placeholder 1"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp>"#);
    x.push_str(r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/>"#);
    text::write_text_body(&mut x, "p:txBody", &n.text, &mut w);
    x.push_str(r#"</p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#);
    w.finish();
    pkg.add(path, x.into_bytes(), CT_NOTES);
}
