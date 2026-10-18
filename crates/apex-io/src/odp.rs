//! OpenDocument presentations (.odp, .otp): the master pages, the slides
//! (`draw:page`) with their frames, shapes, pictures, tables and groups,
//! backgrounds and speaker notes, read into `apex_slides::Deck`. Every
//! slide and object keeps the XML it was read from, and the package keeps
//! its files, so that writing back ([`crate::odp_write`]) loses nothing
//! the model doesn't hold: unchanged pieces go back as they were.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Attrs, KeptPart, Theme, VAnchor};
use apex_slides::fill::{Arrow, Background, Dash, Fill, FormatScheme, Gradient, Line, PictureFill};
use apex_slides::object::{Frame, Geometry, Group, Media, ObjKind, Object, PhType, Picture, Placeholder, Raw, Src};
use apex_slides::table::{Cell, Row, Table};
use apex_slides::text::{Autofit, ListStyle, ParaProps, RunProps};
use apex_slides::{ClrMap, Deck, Layout, Master, Notes, Origin, Package, SColor, Slide, SlideTheme};

use crate::Error;
use crate::drawingml::{Node, read_fragment};
use crate::odf_shape::{self, Styles, hex_color};
use crate::odp_text::{self, ListStyles, TextIn};
use crate::parse_length_pt;
use crate::zip::ZipReader;

/// Fills named in `office:styles`: gradients and pictures.
#[derive(Default)]
struct Named {
    gradients: HashMap<String, crate::odf_grad::Def>,
    opacities: HashMap<String, crate::odf_grad::Def>,
    images: HashMap<String, String>,
}

/// What reading a part needs.
struct Cx<'a> {
    z: &'a ZipReader<'a>,
    st: &'a Styles,
    lists: &'a ListStyles,
    named: &'a Named,
    pics: HashMap<String, Arc<[u8]>>,
    /// The embedded objects' files (see `odf_ole::load`).
    objects: HashMap<String, Arc<[u8]>>,
    next: u32,
    /// The page's element ids (`xml:id`, `draw:id`) and their objects.
    ids: HashMap<String, u32>,
    /// Object ids the page's elements keep (`apx{page}_{id}`).
    reserved: HashSet<u32>,
    taken: HashSet<u32>,
    /// What the objects' source may still take: nested groups copy
    /// their objects again.
    raw_left: usize,
    /// Objects the page may still have, groups' members included.
    objects_left: usize,
    /// Each chart object read, by its link (frames may share one).
    charts: HashMap<String, Option<apex_doc::ChartObject>>,
}

impl Cx<'_> {
    fn text(&self) -> TextIn<'_> {
        TextIn { st: self.st, lists: self.lists }
    }

    fn picture(&mut self, href: &str) -> Option<Arc<[u8]>> {
        let path = href.trim_start_matches("./").to_string();
        if let Some(p) = self.pics.get(&path) {
            return Some(p.clone());
        }
        let data: Arc<[u8]> = self.z.read(&path).ok()?.into();
        self.pics.insert(path, data.clone());
        Some(data)
    }

    fn id(&mut self) -> u32 {
        loop {
            self.next += 1;
            if !self.reserved.contains(&self.next) && !self.taken.contains(&self.next) {
                self.taken.insert(self.next);
                return self.next;
            }
        }
    }

    /// Starts a page: the object ids its elements keep reserved.
    fn page(&mut self, page: Option<&Node>) {
        self.next = 1;
        self.ids.clear();
        self.reserved.clear();
        self.taken.clear();
        self.objects_left = apex_slides::MAX_OBJECTS;
        if let Some(p) = page {
            let mut all = Vec::new();
            ids_in(p, &mut all);
            self.reserved = all.into_iter().collect();
        }
    }
}

/// The object id of an element id we wrote (`apx{page}_{id}`).
fn kept_id(x: &str) -> Option<u32> {
    x.strip_prefix("apx")?.split_once('_')?.1.parse().ok().filter(|&n| n > 1)
}

fn ids_in(n: &Node, out: &mut Vec<u32>) {
    if let Some(i) = crate::odp_anim::exact(n, "xml:id").and_then(kept_id) {
        out.push(i);
    }
    for k in &n.kids {
        ids_in(k, out);
    }
}

fn len(n: &Node, key: &str) -> Option<f32> {
    n.attr(key).and_then(parse_length_pt).filter(|v| v.is_finite())
}

/// The frame of an element with `svg:x`... or a `draw:transform`.
fn frame_of(n: &Node) -> Frame {
    let (w, h) = (len(n, "svg:width").unwrap_or(0.0).max(0.0), len(n, "svg:height").unwrap_or(0.0).max(0.0));
    let mut f = match n.attr("draw:transform").map(odf_shape::parse_transform) {
        Some((rot, Some((tx, ty)))) => {
            let t = -rot;
            let (sn, c) = t.sin_cos();
            let (hw, hh) = (w / 2.0, h / 2.0);
            let (cx, cy) = (tx + hw * c - hh * sn, ty + hw * sn + hh * c);
            let mut f = Frame::new(cx - hw, cy - hh, w, h);
            f.rot = t.to_degrees();
            f
        }
        _ => Frame::new(len(n, "svg:x").unwrap_or(0.0), len(n, "svg:y").unwrap_or(0.0), w, h),
    };
    f.sanitize();
    f
}

/// A fill from graphic or drawing page properties (`get` looks one up).
fn fill_of(get: &dyn Fn(&str) -> Option<String>, named: &Named, cx: Option<&mut Cx>) -> Option<Fill> {
    let color = || hex_color(get("draw:fill-color").as_deref()).map(SColor::rgb);
    Some(match get("draw:fill")?.as_str() {
        "none" => Fill::None,
        "solid" => Fill::Solid(color().unwrap_or(SColor::rgb(0x729FCF))),
        "gradient" => match get("draw:fill-gradient-name").and_then(|g| named.gradients.get(&g)) {
            Some(d) => {
                let op = get("draw:opacity-name").and_then(|o| named.opacities.get(&o));
                let uniform = get("draw:opacity").and_then(|v| v.trim().trim_end_matches('%').parse::<f32>().ok()).map(|p| p / 100.0);
                Fill::Gradient(Gradient::from_doc(&d.gradient(op, uniform)))
            }
            None => Fill::Solid(color().unwrap_or(SColor::rgb(0x729FCF))),
        },
        "bitmap" => {
            let href = get("draw:fill-image-name").and_then(|i| named.images.get(&i).cloned());
            match (href, cx) {
                (Some(h), Some(cx)) => match cx.picture(&h) {
                    Some(data) => Fill::Picture(PictureFill { data, tile: get("style:repeat").as_deref() == Some("repeat"), crop: [0.0; 4], alpha: 1.0 }),
                    None => Fill::None,
                },
                _ => Fill::None,
            }
        }
        _ => Fill::Solid(color().unwrap_or(SColor::rgb(0xFFFFFF))),
    })
}

/// An outline from graphic properties.
fn line_of(get: &dyn Fn(&str) -> Option<String>) -> Option<Line> {
    let mut l = match get("draw:stroke")?.as_str() {
        "none" => return Some(Line::none()),
        kind => {
            let mut l = Line::solid(SColor::rgb(hex_color(get("svg:stroke-color").as_deref()).unwrap_or(0x3465A4)), get("svg:stroke-width").and_then(|w| parse_length_pt(&w)).filter(|w| *w > 0.0).unwrap_or(0.75));
            if kind == "dash" {
                l.dash = Some(Dash::parse("dash"));
            }
            l
        }
    };
    if get("draw:marker-start").is_some_and(|m| !m.is_empty()) {
        l.head = Some(Arrow::Triangle);
    }
    if get("draw:marker-end").is_some_and(|m| !m.is_empty()) {
        l.tail = Some(Arrow::Triangle);
    }
    Some(l)
}

/// Character settings of a style as run properties (for master styles).
fn run_props(st: &Styles, name: &str) -> RunProps {
    let mut a = Attrs::default();
    odp_text::run_attrs(st, Some(name), &mut a);
    let has = |k: &str| st.get(Some(name), k).is_some();
    RunProps {
        size: a.size.map(|s| s as f32 / 2.0),
        bold: has("fo:font-weight").then_some(a.bold),
        italic: has("fo:font-style").then_some(a.italic),
        underline: has("style:text-underline-style").then_some(a.underline),
        color: a.color.map(SColor::rgb),
        font: a.font,
        ..Default::default()
    }
}

/// A master text style level from a presentation style.
fn style_level(st: &Styles, name: &str, pp: &mut ParaProps) {
    if st.own(name).is_none() {
        return;
    }
    let mut own = ParaProps::default();
    odp_text::para_props(st, Some(name), &mut own);
    // The bullets and indents of outlines come from their list style.
    own.mar_l = None;
    own.indent = None;
    own.run = run_props(st, name);
    pp.overlay(&own);
}

/// The master's title and body styles from its presentation styles
/// ("<master>-title", "<master>-outline1"...).
fn master_styles(st: &Styles, name: &str) -> (ListStyle, ListStyle, ListStyle) {
    let (mut title, mut body, mut other) = apex_slides::build::office_styles();
    style_level(st, &format!("{name}-title"), title.level_mut(0));
    for l in 0..9u8 {
        style_level(st, &format!("{name}-outline{}", l + 1), body.level_mut(l));
    }
    for l in 0..9u8 {
        let lv = other.level_mut(l);
        lv.run.overlay(&run_props(st, "standard"));
    }
    (title, body, other)
}

/// Reads the gradients and bitmaps of `office:styles`.
fn read_named(root: &Node) -> Named {
    let mut n = Named::default();
    let mut all = Vec::new();
    root.find_all("gradient", &mut all);
    root.find_all("opacity", &mut all);
    for g in all {
        let Some(name) = g.attr("draw:name") else { continue };
        let mut d = crate::odf_grad::Def::of(&|k: &str| g.attr(k).map(str::to_string));
        for st in g.kids.iter().filter(|k| matches!(k.local(), "gradient-stop" | "opacity-stop")) {
            d.stop(&|k: &str| st.attr(k).map(str::to_string));
        }
        let map = if g.local() == "opacity" { &mut n.opacities } else { &mut n.gradients };
        map.insert(name.to_string(), d);
    }
    let mut all = Vec::new();
    root.find_all("fill-image", &mut all);
    for i in all {
        if let (Some(name), Some(href)) = (i.attr("draw:name"), i.attr("xlink:href")) {
            n.images.insert(name.to_string(), href.to_string());
        }
    }
    n
}

/// A path (`svg:d`) or points (`draw:points`) as DrawingML custom
/// geometry in the view box's units.
pub(crate) fn cust_geom(n: &Node) -> Option<String> {
    let vb: Vec<f32> = n.attr("svg:viewBox")?.split_whitespace().filter_map(|v| v.parse().ok()).collect();
    if vb.len() != 4 || vb[2] <= 0.0 || vb[3] <= 0.0 {
        return None;
    }
    let (ox, oy) = (vb[0], vb[1]);
    let mut body = String::new();
    let pt = |x: f32, y: f32| format!(r#"<a:pt x="{}" y="{}"/>"#, x - ox, y - oy);
    let open = n.local() == "polyline";
    if let Some(points) = n.attr("draw:points") {
        // As many points as a path takes.
        let nums: Vec<f32> = points.split([' ', ',']).filter_map(|v| v.parse().ok()).take(200_000).collect();
        for (i, p) in nums.chunks_exact(2).enumerate() {
            let tag = if i == 0 { "moveTo" } else { "lnTo" };
            let _ = write!(body, "<a:{tag}>{}</a:{tag}>", pt(p[0], p[1]));
        }
        if !open {
            body.push_str("<a:close/>");
        }
    } else {
        svg_path(n.attr("svg:d")?, &mut body, &pt);
    }
    let fill = if open { r#" fill="none""# } else { "" };
    Some(format!(r#"<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="0" t="0" r="r" b="b"/><a:pathLst><a:path w="{}" h="{}"{fill}>{body}</a:path></a:pathLst></a:custGeom>"#, vb[2], vb[3]))
}

/// SVG path commands (M L H V C S Q T Z, absolute and relative) as
/// DrawingML path elements; arcs become lines to their end.
fn svg_path(d: &str, out: &mut String, pt: &dyn Fn(f32, f32) -> String) {
    // Far more than the commands kept below take.
    let mut end = d.len().min(1 << 20);
    while !d.is_char_boundary(end) {
        end -= 1;
    }
    let d = &d[..end];
    let mut toks: Vec<Result<char, f32>> = Vec::new();
    let mut num = String::new();
    let flush = |num: &mut String, toks: &mut Vec<Result<char, f32>>| {
        if let Ok(v) = num.parse::<f32>() {
            toks.push(Err(v));
        }
        num.clear();
    };
    for ch in d.chars() {
        if ch.is_ascii_alphabetic() && ch != 'e' && ch != 'E' {
            flush(&mut num, &mut toks);
            toks.push(Ok(ch));
        } else if ch == '-' && !num.is_empty() && !num.ends_with(['e', 'E']) {
            flush(&mut num, &mut toks);
            num.push(ch);
        } else if ch == ',' || ch.is_whitespace() {
            flush(&mut num, &mut toks);
        } else if ch == '.' && num.contains('.') {
            flush(&mut num, &mut toks);
            num.push(ch);
        } else {
            num.push(ch);
        }
    }
    flush(&mut num, &mut toks);
    let (mut x, mut y, mut sx, mut sy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut cmd = 'M';
    let mut i = 0;
    let take = |i: &mut usize, n: usize| -> Option<Vec<f32>> {
        let mut v = Vec::with_capacity(n);
        for k in 0..n {
            match toks.get(*i + k) {
                Some(Err(x)) => v.push(*x),
                _ => return None,
            }
        }
        *i += n;
        Some(v)
    };
    let mut guard = 0;
    while i < toks.len() && guard < 100_000 {
        guard += 1;
        if let Ok(c) = toks[i] {
            cmd = c;
            i += 1;
            if matches!(c, 'Z' | 'z') {
                out.push_str("<a:close/>");
                (x, y) = (sx, sy);
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            'M' => {
                let Some(v) = take(&mut i, 2) else { break };
                (x, y) = (bx + v[0], by + v[1]);
                (sx, sy) = (x, y);
                let _ = write!(out, "<a:moveTo>{}</a:moveTo>", pt(x, y));
                // Further pairs are lines.
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' | 'T' => {
                let Some(v) = take(&mut i, 2) else { break };
                (x, y) = (bx + v[0], by + v[1]);
                let _ = write!(out, "<a:lnTo>{}</a:lnTo>", pt(x, y));
            }
            'H' => {
                let Some(v) = take(&mut i, 1) else { break };
                x = bx + v[0];
                let _ = write!(out, "<a:lnTo>{}</a:lnTo>", pt(x, y));
            }
            'V' => {
                let Some(v) = take(&mut i, 1) else { break };
                y = (if rel { y } else { 0.0 }) + v[0];
                let _ = write!(out, "<a:lnTo>{}</a:lnTo>", pt(x, y));
            }
            'C' => {
                let Some(v) = take(&mut i, 6) else { break };
                let _ = write!(out, "<a:cubicBezTo>{}{}{}</a:cubicBezTo>", pt(bx + v[0], by + v[1]), pt(bx + v[2], by + v[3]), pt(bx + v[4], by + v[5]));
                (x, y) = (bx + v[4], by + v[5]);
            }
            'S' => {
                let Some(v) = take(&mut i, 4) else { break };
                let _ = write!(out, "<a:cubicBezTo>{}{}{}</a:cubicBezTo>", pt(x, y), pt(bx + v[0], by + v[1]), pt(bx + v[2], by + v[3]));
                (x, y) = (bx + v[2], by + v[3]);
            }
            'Q' => {
                let Some(v) = take(&mut i, 4) else { break };
                let _ = write!(out, "<a:quadBezTo>{}{}</a:quadBezTo>", pt(bx + v[0], by + v[1]), pt(bx + v[2], by + v[3]));
                (x, y) = (bx + v[2], by + v[3]);
            }
            'A' => {
                let Some(v) = take(&mut i, 7) else { break };
                (x, y) = (bx + v[5], by + v[6]);
                let _ = write!(out, "<a:lnTo>{}</a:lnTo>", pt(x, y));
            }
            _ => {
                i += 1;
            }
        }
    }
}

/// The placeholder kind of a `presentation:class`.
fn ph_kind(class: &str) -> Option<PhType> {
    Some(match class {
        "title" => PhType::Title,
        "subtitle" => PhType::SubTitle,
        "outline" | "text" => PhType::Body,
        "date-time" => PhType::Date,
        "footer" => PhType::Footer,
        "page-number" => PhType::SlideNum,
        "header" => PhType::Header,
        _ => return None,
    })
}

/// A graphic property of an element's styles.
fn getter<'a>(st: &'a Styles, n: &'a Node) -> impl Fn(&str) -> Option<String> + 'a {
    let style = n.attr("draw:style-name").or_else(|| n.attr("presentation:style-name"));
    move |k: &str| st.graphic(style, k).map(str::to_string)
}

/// The text frame settings of an element's style.
fn body_props(t: &mut apex_slides::TextBody, get: &dyn Fn(&str) -> Option<String>) {
    let pad = |k: &str| get(k).and_then(|v| parse_length_pt(&v)).filter(|v| v.is_finite());
    t.body.insets = [pad("fo:padding-left"), pad("fo:padding-top"), pad("fo:padding-right"), pad("fo:padding-bottom")];
    if let Some(v) = get("draw:textarea-vertical-align") {
        t.body.anchor = Some(match v.as_str() {
            "middle" => VAnchor::Middle,
            "bottom" => VAnchor::Bottom,
            _ => VAnchor::Top,
        });
    }
    if get("draw:fit-to-size").is_some_and(|v| v == "shrink-to-fit" || v == "true") || get("style:shrink-to-fit").as_deref() == Some("true") {
        t.body.autofit = Some(Autofit::Shrink { scale: 1.0, reduce: 0.0 });
    }
    if get("fo:wrap-option").as_deref() == Some("no-wrap") || get("draw:auto-grow-width").as_deref() == Some("true") {
        t.body.wrap = Some(false);
    }
    if t.body.autofit.is_none() && get("draw:auto-grow-height").as_deref() == Some("true") {
        t.body.autofit = Some(Autofit::Resize);
    }
}

/// A picture of a frame: the first bitmap among its images (the others
/// are fallbacks or vector versions).
fn frame_picture(n: &Node, cx: &mut Cx) -> Option<Arc<[u8]>> {
    let imgs: Vec<&Node> = n.children("image").collect();
    let pick = imgs.iter().find(|i| i.attr("xlink:href").is_some_and(|h| !h.ends_with(".svg"))).or(imgs.first())?;
    cx.picture(pick.attr("xlink:href")?)
}

/// A video or sound (`draw:plugin` of LibreOffice's media player): its
/// file, in the package or linked, and its settings.
fn read_media(pl: &Node, cx: &mut Cx) -> Option<Media> {
    let href = pl.attr("xlink:href")?;
    let param = |name: &str| pl.children("param").find(|p| p.attr("draw:name") == Some(name)).and_then(|p| p.attr("draw:value"));
    let mime = param("MimeType").map(str::to_string).filter(|m| m.starts_with("video/") || m.starts_with("audio/")).unwrap_or_else(|| crate::pptx_shape::mime_of(href).to_string());
    let is_media = pl.attr("draw:mime-type") == Some("application/vnd.sun.star.media") || mime.starts_with("video/") || mime.starts_with("audio/");
    if !is_media {
        return None;
    }
    let on = |name: &str| param(name).is_some_and(|v| v == "true");
    let secs = |name: &str| param(name).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).map_or(0.0, |v| v.max(0.0));
    let mut m = Media { video: !mime.starts_with("audio/"), mime, volume: 1.0, ..Default::default() };
    let inside = !href.contains(':') && !href.starts_with('/') && !href.starts_with("../");
    match inside.then(|| cx.picture(href)).flatten() {
        Some(d) => m.data = Some(d),
        None => m.link = Some(href.to_string()),
    }
    m.looped = on("Loop");
    m.mute = on("Mute");
    if let Some(db) = param("VolumeDB").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()) {
        m.volume = 10f32.powf(db.clamp(-40.0, 0.0) / 20.0);
    }
    m.trim = (secs("TrimStart"), secs("TrimEnd"));
    m.autoplay = on("AutoPlay");
    Some(m)
}

/// A table (`table:table` in a frame).
fn read_table(t: &Node, cx: &Cx) -> Table {
    let mut cols = Vec::new();
    for c in t.children("table-column") {
        let w = cx.st.get(c.attr("table:style-name"), "style:column-width").and_then(parse_length_pt).unwrap_or(72.0);
        let rep = c.attr("table:number-columns-repeated").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).min(256);
        cols.extend(std::iter::repeat_n(w, rep));
    }
    let mut rows = Vec::new();
    let mut all_rows: Vec<&Node> = Vec::new();
    for k in &t.kids {
        match k.local() {
            "table-row" => all_rows.push(k),
            "table-header-rows" | "table-rows" => all_rows.extend(k.children("table-row")),
            _ => {}
        }
    }
    // A slide's table is small: a file's repeats and spans are held to
    // what can show.
    const MAX_ROWS: usize = 1000;
    const MAX_COLS: usize = 256;
    // Text the repeats may copy, all cells together, and cells in all.
    let mut copy_left: usize = 64 << 20;
    const MAX_CELLS: usize = 4096;
    let mut cells_left = MAX_CELLS;
    for r in all_rows.into_iter().take(MAX_ROWS) {
        if cells_left == 0 {
            break;
        }
        let height = cx.st.get(r.attr("table:style-name"), "style:row-height").or_else(|| cx.st.get(r.attr("table:style-name"), "style:min-row-height")).and_then(parse_length_pt).unwrap_or(24.0);
        let mut cells = Vec::new();
        for c in &r.kids {
            let covered = c.local() == "covered-table-cell";
            if !covered && c.local() != "table-cell" {
                continue;
            }
            let mut cell = Cell::new("");
            if covered {
                cell.h_merge = true;
            } else {
                let style = c.attr("table:style-name");
                let get = |k: &str| cx.st.get(style, k).map(str::to_string);
                cell.text = odp_text::read_body(c, &cx.text(), &Attrs::default(), true);
                cell.span = c.attr("table:number-columns-spanned").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
                cell.rspan = c.attr("table:number-rows-spanned").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
                cell.fill = fill_of(&get, cx.named, None).or_else(|| hex_color(get("fo:background-color").as_deref()).map(|c| Fill::Solid(SColor::rgb(c))));
                // Borders: left, right, top, bottom.
                for (i, side) in ["left", "right", "top", "bottom"].into_iter().enumerate() {
                    let v = get(&format!("fo:border-{side}")).or_else(|| get("fo:border"));
                    if let Some(v) = v {
                        cell.borders[i] = Some(match odf_shape::border(&v) {
                            Some((w, c)) => Line::solid(SColor::rgb(c), w),
                            None => Line::none(),
                        });
                    }
                }
            }
            let rep = c.attr("table:number-columns-repeated").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).min(256);
            let size = cell.text.paras.iter().map(|p| p.text.len() + 1).sum::<usize>();
            let rep = if size * rep > copy_left { 1 } else { rep };
            copy_left = copy_left.saturating_sub(size * rep);
            for _ in 0..rep.min(MAX_COLS.saturating_sub(cells.len())).min(cells_left) {
                cells.push(cell.clone());
                cells_left -= 1;
            }
        }
        rows.push(Row { height, cells });
    }
    let n = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0).max(cols.len()).min(MAX_COLS);
    // Filled out, the rows stay within the cells allowed.
    rows.truncate((2 * MAX_CELLS / n.max(1)).max(1));
    cols.resize(n, 72.0);
    for r in &mut rows {
        r.cells.resize(n, Cell::new(""));
    }
    // Cells under a vertical span are covered from above.
    let count = rows.len();
    for r in &mut rows {
        for (c, cell) in r.cells.iter_mut().enumerate() {
            cell.span = cell.span.max(1).min((n - c) as u32);
        }
    }
    for c in 0..n {
        let mut r = 0;
        while r < count {
            let cell = &mut rows[r].cells[c];
            let span = (cell.rspan.max(1) as usize).min(count - r);
            cell.rspan = span as u32;
            for row in rows.iter_mut().skip(r + 1).take(span - 1) {
                row.cells[c].h_merge = false;
                row.cells[c].v_merge = true;
            }
            r += span;
        }
    }
    Table { cols, rows, style: None, look: Default::default() }
}

fn raw(n: &Node, cx: &mut Cx) -> Option<Raw> {
    let mut s = String::new();
    if !xml_within(&mut s, n, cx.raw_left) {
        cx.raw_left = 0;
        return None;
    }
    cx.raw_left -= s.len();
    Some(Raw { src: Src::Odp, xml: Arc::from(s) })
}

/// Writes `n` as XML to `out`; false (stopping) once it's longer than `max`.
fn xml_within(out: &mut String, n: &Node, max: usize) -> bool {
    if n.kids.is_empty() || n.name == crate::drawingml::TEXT {
        crate::pptx_shape::node_xml(out, n);
        return out.len() <= max;
    }
    out.push('<');
    out.push_str(&n.name);
    for (k, v) in &n.attrs {
        out.push_str(&format!(r#" {k}="{}""#, crate::xml::esc(v)));
    }
    out.push('>');
    for k in &n.kids {
        if out.len() > max || !xml_within(out, k, max) {
            return false;
        }
    }
    out.push_str(&format!("</{}>", n.name));
    out.len() <= max
}

/// Reads a drawing element of a page or master page.
fn read_obj(n: &Node, cx: &mut Cx, master: bool) -> Option<Object> {
    cx.objects_left = cx.objects_left.checked_sub(1)?;
    let local = n.local();
    let get = getter(cx.st, n);
    let class = n.attr("presentation:class");
    let empty_ph = n.attr("presentation:placeholder") == Some("true");
    let kind = match local {
        "frame" => {
            if master && class.is_some() {
                // The master's placeholders only give styles.
                return None;
            }
            if let Some(tb) = n.child("text-box") {
                let ph = class.and_then(ph_kind);
                if class.is_some() && ph.is_none() {
                    return None;
                }
                let mut base = Attrs::default();
                if ph.is_none() {
                    odp_text::run_attrs(cx.st, n.attr("draw:style-name"), &mut base);
                }
                let mut t = odp_text::read_body(tb, &cx.text(), &base, true);
                body_props(&mut t, &get);
                let mut o = Object::new(0, "", ObjKind::Shape { text: Some(t), text_box: ph.is_none(), connector: false });
                o.ph = ph.map(|k| Placeholder::new(k, 0));
                o.sp.geom = Some(Geometry::preset("rect"));
                o.sp.fill = Some(fill_of(&get, cx.named, Some(cx)).unwrap_or(Fill::None));
                o.sp.line = Some(line_of(&get).unwrap_or_else(Line::none));
                if empty_ph {
                    if let Some(t) = o.text_mut() {
                        t.paras.truncate(1);
                    }
                }
                return Some(finish(o, n, cx));
            }
            if let Some(m) = n.child("plugin").and_then(|pl| read_media(pl, cx)) {
                let f = frame_of(n);
                let data = frame_picture(n, cx).unwrap_or_else(|| Arc::from(&[][..]));
                ObjKind::Picture(Box::new(Picture { image: apex_doc::Image::new(data, f.w, f.h), media: Some(m) }))
            } else if let Some(c) = n.child("object").and_then(|x| x.attr("xlink:href")).and_then(|h| {
                let objects = &cx.objects;
                cx.charts
                    .entry(h.to_string())
                    .or_insert_with(|| {
                        crate::odf_ole::doc_object(objects, h).and_then(|e| match e.kind {
                            apex_doc::EmbedKind::Chart(c) => Some(c),
                            _ => None,
                        })
                    })
                    .clone()
            }) {
                // A chart with its data; the frame's XML stays while unchanged.
                ObjKind::Chart(c)
            } else if n.child("object").is_some() || n.child("object-ole").is_some() || n.child("plugin").is_some() {
                let what = if n.child("plugin").is_some() { "media" } else { "object" };
                ObjKind::Other { preview: frame_picture(n, cx), what: Arc::from(what), xml: None }
            } else if let Some(t) = n.child("table") {
                ObjKind::Table(Box::new(read_table(t, cx)))
            } else if n.child("image").is_some() {
                let data = frame_picture(n, cx)?;
                let f = frame_of(n);
                ObjKind::Picture(Box::new(Picture { image: apex_doc::Image::new(data, f.w, f.h), media: None }))
            } else {
                return None;
            }
        }
        "custom-shape" | "rect" | "ellipse" | "circle" | "line" => {
            let r = odf_shape::read(n, cx.st)?;
            let s = &r.shape;
            let geom = match &s.preset {
                Some(p) => Geometry::Preset(p.clone(), s.adj.clone()),
                None => Geometry::Preset(Arc::from(s.kind.preset()), s.adj.clone()),
            };
            let line = s.kind.is_line();
            let has_text = n.kids.iter().any(|k| matches!(k.local(), "p" | "list"));
            let text = if has_text && !line {
                let mut base = Attrs::default();
                odp_text::run_attrs(cx.st, n.attr("draw:style-name"), &mut base);
                let mut t = odp_text::read_body(n, &cx.text(), &base, true);
                body_props(&mut t, &get);
                if t.body.anchor.is_none() {
                    t.body.anchor = Some(VAnchor::Middle);
                }
                for i in 0..t.paras.len() {
                    t.edit_props(i, |pp| {
                        if pp.align.is_none() {
                            pp.align = Some(apex_doc::Align::Center);
                        }
                    });
                }
                Some(t)
            } else {
                None
            };
            let mut o = Object::new(0, s.name.clone(), ObjKind::Shape { text, text_box: false, connector: false });
            let mut f = Frame::new(r.x, r.y, r.w, r.h);
            f.rot = s.rotation;
            f.flip_h = s.flip_h;
            f.flip_v = s.flip_v;
            f.sanitize();
            o.sp.frame = Some(f);
            o.sp.geom = Some(geom);
            o.sp.fill = Some(match s.fill {
                Some(c) if !line => fill_of(&get, cx.named, Some(cx)).filter(|f| !matches!(f, Fill::None)).unwrap_or(Fill::Solid(SColor::rgb(c))),
                _ => Fill::None,
            });
            o.sp.line = Some(match s.line {
                Some(c) => {
                    let mut l = Line::solid(SColor::rgb(c), if s.line_width > 0.0 { s.line_width } else { 0.75 });
                    if s.dash != apex_doc::Dash::Solid {
                        l.dash = Some(Dash::parse(s.dash.preset()));
                    }
                    l.head = s.arrow_start.then_some(Arrow::Triangle);
                    l.tail = s.arrow_end.then_some(Arrow::Triangle);
                    l
                }
                None => Line::none(),
            });
            o.descr = s.alt.clone();
            return Some(finish(o, n, cx));
        }
        "connector" => {
            let (x1, y1, x2, y2) = (len(n, "svg:x1").unwrap_or(0.0), len(n, "svg:y1").unwrap_or(0.0), len(n, "svg:x2").unwrap_or(0.0), len(n, "svg:y2").unwrap_or(0.0));
            let mut o = Object::new(0, "", ObjKind::Shape { text: None, text_box: false, connector: true });
            let mut f = Frame::new(x1.min(x2), y1.min(y2), (x2 - x1).abs(), (y2 - y1).abs());
            f.flip_h = x2 < x1;
            f.flip_v = y2 < y1;
            f.sanitize();
            o.sp.frame = Some(f);
            o.sp.geom = Some(Geometry::preset("straightConnector1"));
            o.sp.line = Some(line_of(&get).unwrap_or_else(|| Line::solid(SColor::rgb(0x3465A4), 0.75)));
            return Some(finish(o, n, cx));
        }
        "polygon" | "polyline" | "path" => {
            let g = cust_geom(n)?;
            let mut o = Object::new(0, "", ObjKind::Shape { text: None, text_box: false, connector: false });
            o.sp.frame = Some(frame_of(n));
            o.sp.geom = Some(Geometry::Custom(Arc::from(g)));
            o.sp.fill = Some(if local == "polyline" { Fill::None } else { fill_of(&get, cx.named, Some(cx)).unwrap_or(Fill::Solid(SColor::rgb(0x729FCF))) });
            o.sp.line = Some(line_of(&get).unwrap_or_else(|| Line::solid(SColor::rgb(0x3465A4), 0.75)));
            return Some(finish(o, n, cx));
        }
        "g" => {
            let children: Vec<Object> = n.kids.iter().filter_map(|k| read_obj(k, cx, master)).collect();
            if children.is_empty() {
                return None;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for c in &children {
                let f = c.sp.frame.unwrap_or_default();
                let (x, y, w, h) = f.bounds();
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + w), y1.max(y + h));
            }
            let child = Frame::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0));
            let mut o = Object::new(0, "", ObjKind::Group(Box::new(Group { children, child })));
            o.sp.frame = Some(child);
            return Some(finish(o, n, cx));
        }
        "control" | "measure" | "scene" | "caption" | "regular-polygon" => {
            if n.attr("svg:width").is_none() {
                return None;
            }
            ObjKind::Other { preview: None, what: Arc::from(local), xml: None }
        }
        _ => return None,
    };
    let mut o = Object::new(0, "", kind);
    o.sp.frame = Some(frame_of(n));
    if let ObjKind::Table(_) = o.kind {
        o.sp.geom = None;
    }
    Some(finish(o, n, cx))
}

/// The id, name, description, frame and kept XML every object gets.
fn finish(mut o: Object, n: &Node, cx: &mut Cx) -> Object {
    let xid = crate::odp_anim::exact(n, "xml:id");
    o.id = match xid.and_then(kept_id).filter(|i| !cx.taken.contains(i)) {
        Some(i) => {
            cx.taken.insert(i);
            i
        }
        None => cx.id(),
    };
    for x in [xid, crate::odp_anim::exact(n, "draw:id")].into_iter().flatten() {
        cx.ids.insert(x.to_string(), o.id);
    }
    if let Some(name) = n.attr("draw:name") {
        o.name = name.to_string();
    }
    o.hidden = crate::odf_shape::is_hidden(n);
    if o.name.is_empty() {
        let base = match &o.kind {
            ObjKind::Picture(_) => "Picture",
            ObjKind::Group(_) => "Group",
            ObjKind::Table(_) => "Table",
            ObjKind::Shape { text_box: true, .. } => "TextBox",
            _ if o.ph.is_some() => "Placeholder",
            _ => "Shape",
        };
        o.name = format!("{base} {}", o.id.saturating_sub(1));
    }
    if let Some(d) = n.child("desc").or_else(|| n.child("title")) {
        o.descr = d.text.clone();
    }
    if o.sp.frame.is_none() {
        o.sp.frame = Some(frame_of(n));
    }
    if let (Some(fl), Some(f)) = (crate::odp_anim::exact(n, "apx:flip"), o.sp.frame.as_mut()) {
        f.flip_h = fl.contains('h');
        f.flip_v = fl.contains('v');
    }
    // What the file's styles show, but isn't the object's own.
    if crate::odp_anim::exact(n, "apx:fill") == Some("inherit") {
        o.sp.fill = None;
    }
    match crate::odp_anim::exact(n, "apx:line") {
        Some("inherit") => o.sp.line = None,
        Some(set) => {
            let has = |k: &str| set.split_whitespace().any(|s| s == k);
            let l = o.sp.line.take().unwrap_or_default();
            let style = n.attr("draw:style-name").or_else(|| n.attr("presentation:style-name"));
            let hidden = style.and_then(|st| cx.st.graphic(Some(st), "svg:stroke-width")).and_then(parse_length_pt);
            let drawn = l.fill.as_ref().is_some_and(|f| !matches!(f, Fill::None));
            o.sp.line = Some(Line {
                width: has("width").then(|| if drawn { l.width } else { hidden.or(l.width) }.unwrap_or(0.75)),
                fill: has("fill").then(|| l.fill.unwrap_or(Fill::None)),
                dash: has("dash").then(|| l.dash.unwrap_or_else(|| Dash::parse("solid"))),
                head: has("head").then(|| l.head.unwrap_or(Arrow::None)),
                tail: has("tail").then(|| l.tail.unwrap_or(Arrow::None)),
            });
        }
        None => {}
    }
    if let Some(a) = n.child("event-listeners").and_then(|e| e.child("event-listener")).and_then(|e| e.attr("xlink:href")) {
        o.click = Some(Arc::from(a));
    }
    o.raw = raw(n, cx);
    o
}

/// The background of a page or master page style.
fn background(st: &Styles, style: Option<&str>, named: &Named, cx: &mut Cx) -> Option<Background> {
    let props = st.own(style?)?.clone();
    let get = |k: &str| props.get(k).cloned();
    fill_of(&get, named, Some(cx)).filter(|f| !matches!(f, Fill::None)).map(Background::Fill)
}

/// The speaker notes of a page.
fn read_notes(n: &Node, cx: &Cx) -> Option<Notes> {
    let frame = n.children("frame").find(|f| f.attr("presentation:class") == Some("notes"))?;
    let tb = frame.child("text-box")?;
    let text = odp_text::read_body(tb, &cx.text(), &Attrs::default(), true);
    Some(Notes { text, origin: None })
}

/// Reads an .odp package.
/// The footer, date and page number frames of a master page, which show
/// on the slides whose page style says so.
fn master_footers(m: &Node, cx: &mut Cx) -> Vec<Object> {
    m.kids.iter().filter(|k| k.local() == "frame" && matches!(k.attr("presentation:class"), Some("footer" | "date-time" | "page-number"))).filter_map(|k| read_obj(k, cx, false)).collect()
}

/// The footers and dates the slides use (`presentation:footer-decl`,
/// `presentation:date-time-decl`): by name, the text and whether it
/// is today's date.
fn footer_decls(root: &Node) -> HashMap<String, (String, bool)> {
    let mut out = HashMap::new();
    for tag in ["footer-decl", "date-time-decl"] {
        let mut all = Vec::new();
        root.find_all(tag, &mut all);
        for d in all {
            if let Some(name) = d.attr("presentation:name") {
                let today = tag == "date-time-decl" && d.attr("presentation:source") == Some("current-date");
                out.insert(name.to_string(), (d.text.clone(), today));
            }
        }
    }
    out
}

/// Adds to slide `s` (read from `page`) the master's footer frames its
/// page style shows, with the slide's footer and date. They are the
/// master's: writing the .odp again leaves them out (an empty kept XML),
/// the page's settings bring them back.
fn add_footers(s: &mut Slide, page: &Node, frames: &[Object], st: &Styles, decls: &HashMap<String, (String, bool)>) {
    let props = page.attr("draw:style-name").and_then(|n| st.own(n));
    let on = |k: &str| props.and_then(|p| p.get(k)).is_some_and(|v| v == "true");
    for f in frames {
        let Some(kind) = f.ph.map(|p| p.kind) else { continue };
        let (shown, decl) = match kind {
            PhType::Footer => (on("presentation:display-footer"), "presentation:use-footer-name"),
            PhType::Date => (on("presentation:display-date-time"), "presentation:use-date-time-name"),
            PhType::SlideNum => (on("presentation:display-page-number"), ""),
            _ => (false, ""),
        };
        if !shown || s.objects.iter().any(|o| o.is_placeholder(kind)) {
            continue;
        }
        let mut o = f.clone();
        o.id = s.next_id();
        o.raw = Some(Raw { src: Src::Odp, xml: Arc::from("") });
        let decl = page.attr(decl).and_then(|n| decls.get(n));
        let Some(t) = o.text_mut() else { continue };
        if t.paras.is_empty() {
            t.paras.push(apex_doc::Paragraph::default());
        }
        t.paras.truncate(1);
        let p = &mut t.paras[0];
        let has_field = p.runs.iter().any(|r| r.attrs.field.is_some());
        let attrs = p.runs.first().map(|r| Attrs { field: None, ..r.attrs.clone() }).unwrap_or_default();
        let field = |kind: apex_doc::FieldKind, format: &str| {
            let mut f = apex_doc::Field::new(kind);
            f.format = Some(format.into());
            Attrs { field: Some(Arc::new(f)), ..attrs.clone() }
        };
        let (text, a) = match (kind, decl) {
            (PhType::SlideNum, _) if has_field => (None, attrs.clone()),
            (PhType::SlideNum, _) => (Some(apex_doc::OBJECT.to_string()), field(apex_doc::FieldKind::Page, "slidenum")),
            (_, Some((_, true))) => (Some(apex_doc::OBJECT.to_string()), field(apex_doc::FieldKind::Date, "datetime1")),
            (_, Some((text, false))) if !text.is_empty() => (Some(text.clone()), attrs.clone()),
            _ => continue,
        };
        if let Some(text) = text {
            p.text.clear();
            p.runs.clear();
            p.push(&text, a);
        }
        s.objects.push(o);
    }
}

pub fn read(data: &[u8]) -> Result<Deck, Error> {
    let mut deck = read_all(data)?;
    apex_slides::bound(&mut deck);
    Ok(deck)
}

fn read_all(data: &[u8]) -> Result<Deck, Error> {
    let z = ZipReader::new(data)?;
    let content = z.read_string("content.xml")?;
    let styles = z.read_string("styles.xml").unwrap_or_default();
    let mut st = Styles::default();
    st.read(&styles);
    st.read(&content);
    let sroot = read_fragment(&styles);
    let croot = read_fragment(&content);
    let mut lists = ListStyles::new();
    odp_text::read_list_styles(&sroot, &mut lists);
    odp_text::read_list_styles(&croot, &mut lists);
    let named = read_named(&sroot);
    let mut objects = HashMap::new();
    crate::odf_ole::load(&z, &mut objects);
    let mut cx = Cx { z: &z, st: &st, lists: &lists, named: &named, pics: HashMap::new(), objects, next: 1, ids: HashMap::new(), reserved: HashSet::new(), taken: HashSet::new(), raw_left: content.len().saturating_mul(4).saturating_add(1 << 20), objects_left: apex_slides::MAX_OBJECTS, charts: HashMap::new() };

    // Page sizes.
    let mut layouts: HashMap<&str, (f32, f32)> = HashMap::new();
    let mut all = Vec::new();
    sroot.find_all("page-layout", &mut all);
    for pl in &all {
        if let (Some(name), Some(p)) = (pl.attr("style:name"), pl.child("page-layout-properties")) {
            if let (Some(w), Some(h)) = (len(p, "fo:page-width"), len(p, "fo:page-height")) {
                layouts.insert(name, (w, h));
            }
        }
    }
    let mut mps = Vec::new();
    sroot.find_all("master-page", &mut mps);
    let size = mps.first().and_then(|m| m.attr("style:page-layout-name")).and_then(|l| layouts.get(l).copied()).filter(|(w, h)| *w > 1.0 && *h > 1.0).unwrap_or((28.0 * 72.0 / 2.54, 21.0 * 72.0 / 2.54));

    let mut deck = apex_slides::build::new_deck(0);
    deck.slides.clear();
    deck.masters.clear();
    deck.width = size.0;
    deck.height = size.1;
    let mut default_style = ListStyle::default();
    let size_def = st.graphic(None, "fo:font-size").and_then(parse_length_pt).unwrap_or(18.0);
    default_style.def = Some(ParaProps { run: RunProps { size: Some(size_def), color: Some(SColor::rgb(0)), ..Default::default() }, ..Default::default() });
    deck.default_style = default_style;

    let mut master_ix: HashMap<String, usize> = HashMap::new();
    let mut footers: Vec<Vec<Object>> = Vec::new();
    for m in &mps {
        footers.push(master_footers(m, &mut cx));
        let name = m.attr("style:name").unwrap_or("Default").to_string();
        cx.objects_left = apex_slides::MAX_OBJECTS;
        let objects: Vec<Object> = m.kids.iter().filter_map(|k| read_obj(k, &mut cx, true)).collect();
        let bg = background(&st, m.attr("draw:style-name"), &named, &mut cx).or(Some(Background::Fill(Fill::Solid(SColor::rgb(0xFFFFFF)))));
        let (title_style, body_style, other_style) = master_styles(&st, &name);
        let blank = Layout { name: "Blank".into(), kind: Arc::from("blank"), objects: Vec::new(), background: None, map: None, show_master: true, origin: None };
        master_ix.insert(name.clone(), deck.masters.len());
        deck.masters.push(Arc::new(Master {
            name: m.attr("style:display-name").unwrap_or(&name).to_string(),
            theme: SlideTheme { theme: crate::odp_anim::read_theme(m).unwrap_or_else(|| Theme::builtin(0)), fmt: FormatScheme::office(), origin: None },
            map: ClrMap::default(),
            background: bg,
            objects,
            title_style,
            body_style,
            other_style,
            layouts: vec![blank],
            origin: Some(Origin { src: Src::Odp, part: name, xml: None, same: true }),
        }));
    }
    if deck.masters.is_empty() {
        let mut m = apex_slides::build::new_master(0, size.0, size.1);
        m.objects.clear();
        m.layouts.truncate(1);
        deck.masters.push(Arc::new(m));
    }

    let decls = footer_decls(&croot);
    let mut pages = Vec::new();
    if let Some(p) = croot.find("presentation") {
        pages.extend(p.children("page"));
    }
    for (i, page) in pages.iter().enumerate() {
        let master = page.attr("draw:master-page-name").and_then(|m| master_ix.get(m).copied()).unwrap_or(0);
        let mut s = Slide::new(256 + i as u32, master, 0);
        s.name = page.attr("draw:name").unwrap_or_default().to_string();
        let style = page.attr("draw:style-name");
        s.background = background(&st, style, &named, &mut cx);
        s.hidden = style.and_then(|n| st.own(n)).and_then(|p| p.get("presentation:visibility")).is_some_and(|v| v == "hidden");
        s.show_master = !style.and_then(|n| st.own(n)).and_then(|p| p.get("presentation:background-objects-visible")).is_some_and(|v| v == "false");
        cx.page(Some(page));
        let own = style.and_then(|n| st.own(n));
        s.transition = crate::odp_anim::read_transition(&|k| own.and_then(|p| p.get(k)).cloned());
        s.objects = page.kids.iter().filter_map(|k| read_obj(k, &mut cx, false)).collect();
        s.notes = page.child("notes").and_then(|n| read_notes(n, &cx));
        s.timing = crate::odp_anim::read_timing(page, &cx.ids);
        add_footers(&mut s, page, footers.get(master).map_or(&[], Vec::as_slice), &st, &decls);
        let mut xml = String::new();
        crate::pptx_shape::node_xml(&mut xml, page);
        s.origin = Some(Origin { src: Src::Odp, part: s.name.clone(), xml: Some(Arc::from(xml)), same: true });
        deck.slides.push(Arc::new(s));
    }
    if let Some(set) = croot.find("presentation").and_then(|p| p.child("settings")) {
        deck.show = crate::odp_anim::read_show(set);
    }
    if let Ok(m) = z.read_string("meta.xml") {
        deck.meta = crate::props::read_odf(&m);
    }
    let mut parts = Vec::new();
    let names: Vec<String> = z.names().map(str::to_string).collect();
    for name in names {
        if name.ends_with('/') {
            continue;
        }
        if let Ok(d) = z.read(&name) {
            parts.push(KeptPart { path: name, content_type: String::new(), data: d.into() });
        }
    }
    deck.package = Arc::new(Package { src: Some(Src::Odp), parts });
    deck.sections.clear();
    Ok(deck)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footers_of_the_master_show_where_the_page_says() {
        let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0""#;
        let frame = |class: &str, inner: &str, y: u32| format!(r#"<draw:frame presentation:class="{class}" svg:x="1cm" svg:y="{y}cm" svg:width="8cm" svg:height="1cm"><draw:text-box><text:p>{inner}</text:p></draw:text-box></draw:frame>"#);
        let styles = format!(
            r#"<office:document-styles {ns}><office:master-styles><style:master-page style:name="M">{}{}{}</style:master-page></office:master-styles></office:document-styles>"#,
            frame("footer", "<presentation:footer/>", 18),
            frame("date-time", "<presentation:date-time/>", 17),
            frame("page-number", "<text:page-number>&lt;number&gt;</text:page-number>", 16)
        );
        let content = format!(
            r#"<office:document-content {ns}><office:automatic-styles><style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties presentation:display-footer="true" presentation:display-page-number="true" presentation:display-date-time="true"/></style:style></office:automatic-styles><office:body><office:presentation><presentation:footer-decl presentation:name="ftr1">Our footer</presentation:footer-decl><presentation:date-time-decl presentation:name="dtd1" presentation:source="fixed">1 May</presentation:date-time-decl><draw:page draw:name="a" draw:style-name="dp1" draw:master-page-name="M" presentation:use-footer-name="ftr1" presentation:use-date-time-name="dtd1"/><draw:page draw:name="b" draw:master-page-name="M"/></office:presentation></office:body></office:document-content>"#
        );
        let mut z = crate::zip::ZipWriter::new();
        z.add("mimetype", b"application/vnd.oasis.opendocument.presentation", false);
        z.add("styles.xml", styles.as_bytes(), true);
        z.add("content.xml", content.as_bytes(), true);
        let deck = read(&z.finish()).unwrap();
        let text = |i: usize, k: PhType| deck.slides[i].objects.iter().find(|o| o.is_placeholder(k)).and_then(|o| o.text()).map(|t| t.text());
        assert_eq!(text(0, PhType::Footer).as_deref(), Some("Our footer"));
        assert_eq!(text(0, PhType::Date).as_deref(), Some("1 May"));
        let num = deck.slides[0].objects.iter().find(|o| o.is_placeholder(PhType::SlideNum)).and_then(|o| o.text()).unwrap();
        assert!(num.paras[0].runs.iter().any(|r| r.attrs.field.as_ref().is_some_and(|f| f.kind == apex_doc::FieldKind::Page)));
        // Not on a page that doesn't show them.
        assert!(text(1, PhType::Footer).is_none());
        // Written again, they stay the master's; the page keeps its footer.
        let mut d = deck.clone();
        d.slide_mut(0).name = "changed".into();
        let again = crate::odp_write::write(&d);
        let c = crate::zip::ZipReader::new(&again).unwrap().read_string("content.xml").unwrap();
        assert!(!c.contains("Our footer</text:") && c.contains(r#"presentation:use-footer-name="ftr1""#), "{c}");
        let back = read(&again).unwrap();
        assert_eq!(back.slides[0].objects.iter().filter(|o| o.is_placeholder(PhType::Footer)).count(), 1);
        // In pptx they are the slide's own placeholders.
        let p = crate::pptx::read(&crate::pptx::write(&deck)).unwrap();
        assert!(p.slides[0].objects.iter().any(|o| o.is_placeholder(PhType::Footer) && o.text().is_some_and(|t| t.text() == "Our footer")));
    }

    #[test]
    fn huge_table_spans_are_held_to_the_table() {
        let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0""#;
        let cell = r#"<table:table-cell table:number-rows-spanned="4294967295" table:number-columns-spanned="4294967295" table:number-columns-repeated="999999"><text:p>x</text:p></table:table-cell>"#;
        let row = format!("<table:table-row>{}</table:table-row>", cell.repeat(50));
        let content = format!(
            r#"<office:document-content {ns}><office:body><office:presentation><draw:page draw:name="a"><draw:frame svg:x="1cm" svg:y="1cm" svg:width="8cm" svg:height="4cm"><table:table>{}</table:table></draw:frame></draw:page></office:presentation></office:body></office:document-content>"#,
            row.repeat(3)
        );
        let mut z = crate::zip::ZipWriter::new();
        z.add("mimetype", b"application/vnd.oasis.opendocument.presentation", false);
        z.add("content.xml", content.as_bytes(), true);
        let t0 = web_time::Instant::now();
        let deck = read(&z.finish()).unwrap();
        assert!(t0.elapsed().as_secs() < 5);
        let t = deck.slides[0].objects.iter().find_map(|o| if let ObjKind::Table(t) = &o.kind { Some(t) } else { None }).unwrap();
        assert_eq!(t.rows.len(), 3);
        assert!(t.cols.len() <= 256);
        assert_eq!(t.rows[0].cells[0].rspan, 3);
        assert!(t.rows[1].cells[0].v_merge);
        assert!(t.rows.iter().all(|r| r.cells.iter().enumerate().all(|(c, x)| x.span as usize <= t.cols.len() - c)));
    }

    #[test]
    fn svg_paths_become_custom_geometry() {
        let n = read_fragment(r#"<draw:path svg:viewBox="10 10 100 100" svg:d="M10 10 l100 0 L110 110 C 60 110 10 60 10 10z"/>"#);
        let g = cust_geom(&n.kids[0]).unwrap();
        let p = apex_slides::geom::custom_paths(&g, 200.0, 200.0);
        assert_eq!(p.len(), 1);
        let (pts, closed) = &p[0].subpaths[0];
        assert!(*closed);
        assert_eq!(pts[0], (0.0, 0.0));
        assert_eq!(pts[1], (200.0, 0.0));
        assert_eq!(pts[2], (200.0, 200.0));
        let poly = read_fragment(r#"<draw:polyline svg:viewBox="0 0 10 10" draw:points="0,0 10,0 10,10"/>"#);
        let g = cust_geom(&poly.kids[0]).unwrap();
        assert!(g.contains(r#"fill="none""#) && !g.contains("close"));
    }
}
