//! Text of presentation shapes, table cells and notes (`p:txBody`,
//! `a:txBody`): the text frame, list styles, paragraphs and runs, read
//! into `apex_slides::TextBody` and written back.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Align, Attrs, CharFx, Field, FieldKind, LINE_BREAK, OBJECT, Paragraph, Run, Theme, VAnchor};
use apex_slides::color::{ClrMap, ColorCx};
use apex_slides::text::{Autofit, BodyProps, Bullet, Caps, ListStyle, ParaProps, RunProps, Spacing, TextBody};

use crate::drawingml::{EMU_PT, Node};
use crate::pptx_fill::{read_color, write_color};
use crate::xml::esc;

/// What reading text needs: the theme and color map (run colors are
/// kept as RGB, as documents keep them) and the part's links.
pub(crate) struct TextCx<'a> {
    pub theme: &'a Theme,
    pub map: ClrMap,
    /// Hyperlink targets by relationship id.
    pub links: &'a HashMap<String, Arc<str>>,
}

/// Writing text: where hyperlinks get their relationship ids.
pub(crate) trait Links {
    fn link(&mut self, target: &str) -> String;
}

fn emu(n: &Node, k: &str) -> Option<f32> {
    n.num(k).map(|v| v.clamp(-1e8, 1e8) as f32 / EMU_PT)
}

fn flag(n: &Node, k: &str) -> Option<bool> {
    n.attr(k).map(|v| matches!(v, "1" | "true" | "on"))
}

fn align_of(s: &str) -> Align {
    match s {
        "ctr" => Align::Center,
        "r" => Align::Right,
        "just" | "dist" | "justLow" | "thaiDist" => Align::Justify,
        _ => Align::Left,
    }
}

fn align_name(a: Align) -> &'static str {
    match a {
        Align::Center => "ctr",
        Align::Right => "r",
        Align::Justify => "just",
        _ => "l",
    }
}

fn spacing(n: &Node) -> Option<Spacing> {
    if let Some(p) = n.child("spcPct") {
        return Some(Spacing::Pct((p.num("val").unwrap_or(100_000.0) / 100_000.0).clamp(0.0, 132.0) as f32));
    }
    n.child("spcPts").map(|p| Spacing::Pts((p.num("val").unwrap_or(0.0) / 100.0).clamp(0.0, 1584.0) as f32))
}

fn write_spacing(out: &mut String, tag: &str, s: Spacing) {
    match s {
        Spacing::Pct(v) => {
            let _ = write!(out, r#"<a:{tag}><a:spcPct val="{}"/></a:{tag}>"#, (v * 100_000.0).round() as i64);
        }
        Spacing::Pts(v) => {
            let _ = write!(out, r#"<a:{tag}><a:spcPts val="{}"/></a:{tag}>"#, (v * 100.0).round() as i64);
        }
    }
}

/// Character properties that may be unset (`a:defRPr`, and a run's).
pub(crate) fn read_run_props(n: &Node) -> RunProps {
    let typeface = |k: &str| n.child(k).and_then(|f| f.attr("typeface")).filter(|f| !f.is_empty()).map(Arc::from);
    RunProps {
        size: n.num("sz").filter(|v| v.is_finite()).map(|v| (v / 100.0).clamp(1.0, 4000.0) as f32),
        bold: flag(n, "b"),
        italic: flag(n, "i"),
        underline: n.attr("u").map(|u| u != "none"),
        strike: n.attr("strike").map(|u| u != "noStrike"),
        color: n.child("solidFill").and_then(read_color),
        font: typeface("latin"),
        ea_font: typeface("ea"),
        cs_font: typeface("cs"),
        caps: n.attr("cap").map(|c| match c {
            "all" => Caps::All,
            "small" => Caps::Small,
            _ => Caps::None,
        }),
        spacing: n.num("spc").map(|v| (v / 100.0) as f32),
        baseline: n.num("baseline").map(|v| (v / 1000.0) as f32),
    }
}

pub(crate) fn write_run_props(out: &mut String, tag: &str, r: &RunProps) {
    let _ = write!(out, "<a:{tag}");
    if let Some(v) = r.size {
        let _ = write!(out, r#" sz="{}""#, (v * 100.0).round() as i64);
    }
    for (k, v) in [("b", r.bold), ("i", r.italic)] {
        if let Some(v) = v {
            let _ = write!(out, r#" {k}="{}""#, v as u8);
        }
    }
    if let Some(u) = r.underline {
        let _ = write!(out, r#" u="{}""#, if u { "sng" } else { "none" });
    }
    if let Some(s) = r.strike {
        let _ = write!(out, r#" strike="{}""#, if s { "sngStrike" } else { "noStrike" });
    }
    if let Some(c) = r.caps {
        let _ = write!(out, r#" cap="{}""#, match c {
            Caps::All => "all",
            Caps::Small => "small",
            Caps::None => "none",
        });
    }
    if let Some(s) = r.spacing {
        let _ = write!(out, r#" spc="{}""#, (s * 100.0).round() as i64);
    }
    if let Some(b) = r.baseline {
        let _ = write!(out, r#" baseline="{}""#, (b * 1000.0).round() as i64);
    }
    let fonts = [("latin", &r.font), ("ea", &r.ea_font), ("cs", &r.cs_font)];
    if r.color.is_none() && fonts.iter().all(|f| f.1.is_none()) {
        out.push_str("/>");
        return;
    }
    out.push('>');
    if let Some(c) = &r.color {
        out.push_str("<a:solidFill>");
        write_color(out, c);
        out.push_str("</a:solidFill>");
    }
    for (k, f) in fonts {
        if let Some(f) = f {
            let _ = write!(out, r#"<a:{k} typeface="{}"/>"#, esc(f));
        }
    }
    let _ = write!(out, "</a:{tag}>");
}

/// Paragraph properties (`a:pPr`, `a:lvl1pPr`...); also the level.
pub(crate) fn read_para_props(n: &Node) -> (ParaProps, u8) {
    let mut p = ParaProps {
        align: n.attr("algn").map(align_of),
        mar_l: emu(n, "marL"),
        indent: emu(n, "indent"),
        rtl: flag(n, "rtl"),
        ..Default::default()
    };
    for k in &n.kids {
        match k.local() {
            "lnSpc" => p.line = spacing(k),
            "spcBef" => p.before = spacing(k),
            "spcAft" => p.after = spacing(k),
            "buNone" => p.bullet = Some(Bullet::None),
            "buChar" => p.bullet = k.attr("char").and_then(|c| c.chars().next()).map(Bullet::Char),
            "buAutoNum" => p.bullet = Some(Bullet::Auto(Arc::from(k.attr("type").unwrap_or("arabicPeriod")), k.num("startAt").unwrap_or(1.0) as u32)),
            "buClrTx" => p.bullet_color = Some(None),
            "buClr" => p.bullet_color = read_color(k).map(Some),
            "buSzPct" => p.bullet_size = k.num("val").map(|v| (v / 100_000.0).clamp(0.25, 4.0) as f32),
            "buSzTx" => p.bullet_size = Some(1.0),
            "buFontTx" => p.bullet_font = Some(None),
            "buFont" => p.bullet_font = k.attr("typeface").map(|f| Some(Arc::from(f))),
            "defRPr" => p.run = read_run_props(k),
            _ => {}
        }
    }
    (p, n.num("lvl").unwrap_or(0.0).clamp(0.0, 8.0) as u8)
}

pub(crate) fn write_para_props(out: &mut String, tag: &str, p: &ParaProps, level: u8) {
    let _ = write!(out, "<a:{tag}");
    if let Some(v) = p.mar_l {
        let _ = write!(out, r#" marL="{}""#, (v * EMU_PT).round() as i64);
    }
    if level > 0 {
        let _ = write!(out, r#" lvl="{level}""#);
    }
    if let Some(v) = p.indent {
        let _ = write!(out, r#" indent="{}""#, (v * EMU_PT).round() as i64);
    }
    if let Some(a) = p.align {
        let _ = write!(out, r#" algn="{}""#, align_name(a));
    }
    if let Some(r) = p.rtl {
        let _ = write!(out, r#" rtl="{}""#, r as u8);
    }
    let mut kids = String::new();
    for (t, s) in [("lnSpc", p.line), ("spcBef", p.before), ("spcAft", p.after)] {
        if let Some(s) = s {
            write_spacing(&mut kids, t, s);
        }
    }
    match &p.bullet_color {
        Some(None) => kids.push_str("<a:buClrTx/>"),
        Some(Some(c)) => {
            kids.push_str("<a:buClr>");
            write_color(&mut kids, c);
            kids.push_str("</a:buClr>");
        }
        None => {}
    }
    if let Some(s) = p.bullet_size {
        let _ = write!(kids, r#"<a:buSzPct val="{}"/>"#, (s * 100_000.0).round() as i64);
    }
    match &p.bullet_font {
        Some(None) => kids.push_str("<a:buFontTx/>"),
        Some(Some(f)) => {
            let _ = write!(kids, r#"<a:buFont typeface="{}"/>"#, esc(f));
        }
        None => {}
    }
    match &p.bullet {
        Some(Bullet::None) => kids.push_str("<a:buNone/>"),
        Some(Bullet::Char(c)) => {
            let _ = write!(kids, r#"<a:buChar char="{}"/>"#, esc(&c.to_string()));
        }
        Some(Bullet::Auto(t, start)) => {
            let _ = write!(kids, r#"<a:buAutoNum type="{}""#, esc(t));
            if *start != 1 {
                let _ = write!(kids, r#" startAt="{start}""#);
            }
            kids.push_str("/>");
        }
        None => {}
    }
    if !p.run.is_empty() {
        write_run_props(&mut kids, "defRPr", &p.run);
    }
    if kids.is_empty() {
        out.push_str("/>");
    } else {
        let _ = write!(out, ">{kids}</a:{tag}>");
    }
}

/// A list style (`a:lstStyle`, `p:titleStyle`...).
pub(crate) fn read_list_style(n: &Node) -> ListStyle {
    let mut l = ListStyle::default();
    for k in &n.kids {
        let name = k.local();
        if name == "defPPr" {
            l.def = Some(read_para_props(k).0);
        } else if let Some(d) = name.strip_prefix("lvl").and_then(|s| s.strip_suffix("pPr")).and_then(|d| d.parse::<usize>().ok()) {
            if (1..=9).contains(&d) {
                l.levels[d - 1] = Some(read_para_props(k).0);
            }
        }
    }
    l
}

/// The list style's elements, in `tag` (nothing when empty and `tag`
/// isn't required).
pub(crate) fn write_list_style(out: &mut String, tag: &str, l: &ListStyle) {
    if l.is_empty() {
        let _ = write!(out, "<{tag}/>");
        return;
    }
    let _ = write!(out, "<{tag}>");
    if let Some(d) = &l.def {
        write_para_props(out, "defPPr", d, 0);
    }
    for (i, p) in l.levels.iter().enumerate() {
        if let Some(p) = p {
            write_para_props(out, &format!("lvl{}pPr", i + 1), p, 0);
        }
    }
    let _ = write!(out, "</{tag}>");
}

/// The text frame (`a:bodyPr`).
pub(crate) fn read_body_props(n: &Node) -> BodyProps {
    let mut b = BodyProps {
        insets: ["lIns", "tIns", "rIns", "bIns"].map(|k| emu(n, k)),
        anchor: n.attr("anchor").map(|a| match a {
            "ctr" => VAnchor::Middle,
            "b" => VAnchor::Bottom,
            _ => VAnchor::Top,
        }),
        anchor_ctr: flag(n, "anchorCtr"),
        wrap: n.attr("wrap").map(|w| w != "none"),
        vert: n.attr("vert").filter(|v| *v != "horz").map(Arc::from),
        columns: n.num("numCol").map(|v| v.clamp(1.0, 16.0) as u8),
        col_gap: emu(n, "spcCol"),
        rot: n.num("rot").map(|v| (v / 60_000.0) as f32),
        autofit: None,
    };
    for k in &n.kids {
        b.autofit = match k.local() {
            "noAutofit" => Some(Autofit::None),
            "spAutoFit" => Some(Autofit::Resize),
            "normAutofit" => Some(Autofit::Shrink {
                scale: (k.num("fontScale").unwrap_or(100_000.0) / 100_000.0) as f32,
                reduce: (k.num("lnSpcReduction").unwrap_or(0.0) / 100_000.0) as f32,
            }),
            _ => continue,
        };
    }
    b
}

pub(crate) fn write_body_props(out: &mut String, b: &BodyProps) {
    out.push_str("<a:bodyPr");
    if let Some(r) = b.rot {
        let _ = write!(out, r#" rot="{}""#, (r * 60_000.0).round() as i64);
    }
    if let Some(v) = &b.vert {
        let _ = write!(out, r#" vert="{}""#, esc(v));
    }
    if let Some(w) = b.wrap {
        let _ = write!(out, r#" wrap="{}""#, if w { "square" } else { "none" });
    }
    for (k, v) in ["lIns", "tIns", "rIns", "bIns"].iter().zip(b.insets) {
        if let Some(v) = v {
            let _ = write!(out, r#" {k}="{}""#, (v * EMU_PT).round() as i64);
        }
    }
    if let Some(c) = b.columns {
        let _ = write!(out, r#" numCol="{c}""#);
    }
    if let Some(g) = b.col_gap {
        let _ = write!(out, r#" spcCol="{}""#, (g * EMU_PT).round() as i64);
    }
    if let Some(a) = b.anchor {
        let _ = write!(out, r#" anchor="{}""#, match a {
            VAnchor::Top => "t",
            VAnchor::Middle => "ctr",
            VAnchor::Bottom => "b",
        });
    }
    if let Some(c) = b.anchor_ctr {
        let _ = write!(out, r#" anchorCtr="{}""#, c as u8);
    }
    match b.autofit {
        None => out.push_str("/>"),
        Some(Autofit::None) => out.push_str("><a:noAutofit/></a:bodyPr>"),
        Some(Autofit::Resize) => out.push_str("><a:spAutoFit/></a:bodyPr>"),
        Some(Autofit::Shrink { scale, reduce }) => {
            out.push_str("><a:normAutofit");
            if scale < 1.0 {
                let _ = write!(out, r#" fontScale="{}""#, (scale * 100_000.0).round() as i64);
            }
            if reduce > 0.0 {
                let _ = write!(out, r#" lnSpcReduction="{}""#, (reduce * 100_000.0).round() as i64);
            }
            out.push_str("/></a:bodyPr>");
        }
    }
}

/// A run's attributes from its `a:rPr` (colors resolved through the theme).
fn run_attrs(n: Option<&Node>, cx: &TextCx) -> Attrs {
    let mut a = Attrs::default();
    let Some(n) = n else { return a };
    let rp = read_run_props(n);
    a.size = rp.size.map(|s| (s * 2.0).round().clamp(1.0, 8000.0) as u16);
    a.bold = rp.bold.unwrap_or(false);
    a.italic = rp.italic.unwrap_or(false);
    a.underline = rp.underline.unwrap_or(false);
    a.strike = rp.strike.unwrap_or(false);
    a.color = rp.color.map(|c| c.resolve(&ColorCx { theme: cx.theme, map: cx.map, ph: None }).rgb);
    a.font = rp.font;
    match rp.baseline {
        Some(b) if b > 0.0 => a.superscript = true,
        Some(b) if b < 0.0 => a.subscript = true,
        _ => {}
    }
    let mut fx = CharFx::default();
    match rp.caps {
        Some(Caps::All) => fx.all_caps = true,
        Some(Caps::Small) => fx.small_caps = true,
        _ => {}
    }
    if let Some(s) = rp.spacing {
        fx.spacing = (s * 20.0).round().clamp(-32000.0, 32000.0) as i16;
    }
    if !fx.is_default() {
        a.fx = Some(Arc::new(fx));
    }
    if let Some(h) = n.child("highlight").and_then(read_color) {
        a.highlight = Some(h.resolve(&ColorCx { theme: cx.theme, map: cx.map, ph: None }).rgb);
    }
    if let Some(h) = n.child("hlinkClick") {
        let id = h.attr("r:id").unwrap_or("");
        a.link = cx.links.get(id).cloned().or_else(|| h.attr("action").map(|x| Arc::from(action_link(x))));
    }
    a.lang = n.attr("lang").map(Arc::from);
    a.no_proof = flag(n, "noProof").unwrap_or(false);
    a.text_fx = crate::text_fx_xml::read_all(n);
    a
}

/// Links of `ppaction://` actions ("#next"...).
fn action_link(action: &str) -> String {
    match action.rsplit('=').next().unwrap_or("") {
        "nextslide" => "#next".into(),
        "previousslide" => "#prev".into(),
        "firstslide" => "#first".into(),
        "lastslide" => "#last".into(),
        "endshow" => "#end".into(),
        _ => String::new(),
    }
}

/// A paragraph (`a:p`) into `t`.
pub(crate) fn read_para(n: &Node, cx: &TextCx, t: &mut TextBody) {
    let mut p = Paragraph::new("");
    let mut own = None;
    if let Some(ppr) = n.child("pPr") {
        let (pp, level) = read_para_props(ppr);
        p.style.level = level;
        own = Some(pp);
    }
    for k in &n.kids {
        match k.local() {
            "r" => {
                let s = k.child("t").map_or(String::new(), |x| x.text.clone());
                p.push(&s.replace(['\n', '\r'], " "), run_attrs(k.child("rPr"), cx));
            }
            "br" => p.push(&LINE_BREAK.to_string(), run_attrs(k.child("rPr"), cx)),
            "fld" => {
                let cached = k.child("t").map_or(String::new(), |x| x.text.clone());
                let ty = k.attr("type").unwrap_or("");
                let mut a = run_attrs(k.child("rPr"), cx);
                let kind = if ty == "slidenum" {
                    Some(FieldKind::Page)
                } else if ty.starts_with("datetime") {
                    Some(FieldKind::Date)
                } else {
                    None
                };
                match kind {
                    Some(kind) => {
                        let mut f = Field::new(kind);
                        f.cached = cached;
                        f.format = Some(ty.to_string());
                        a.field = Some(Arc::new(f));
                        p.push(&OBJECT.to_string(), a);
                    }
                    None => p.push(&cached, a),
                }
            }
            "AlternateContent" => {
                // Math and newer pieces: their fallback text.
                if let Some(fb) = k.child("Fallback") {
                    for r in fb.children("r") {
                        let s = r.child("t").map_or(String::new(), |x| x.text.clone());
                        p.push(&s, run_attrs(r.child("rPr"), cx));
                    }
                }
            }
            _ => {}
        }
    }
    if p.text.is_empty() {
        if let Some(end) = n.child("endParaRPr") {
            p.runs = vec![Run { len: 0, attrs: run_attrs(Some(end), cx) }];
        }
    }
    t.paras.push(p);
    if let Some(pp) = own {
        t.set_props(t.paras.len() - 1, pp);
    }
}

/// A text body (`p:txBody`, `a:txBody`, `p:txBody` of notes).
pub(crate) fn read_text_body(n: &Node, cx: &TextCx) -> TextBody {
    let mut t = TextBody::default();
    if let Some(b) = n.child("bodyPr") {
        t.body = read_body_props(b);
    }
    if let Some(l) = n.child("lstStyle") {
        t.list = read_list_style(l);
    }
    for p in n.children("p") {
        read_para(p, cx, &mut t);
    }
    if t.paras.is_empty() {
        t.paras.push(Paragraph::new(""));
    }
    // Equal property entries shared.
    let props = std::mem::take(&mut t.props);
    for i in 0..t.paras.len() {
        let c = t.paras[i].style.custom as usize;
        let pp = if c == 0 { ParaProps::default() } else { props[c - 1].clone() };
        t.set_props(i, pp);
    }
    t
}

fn write_run_attrs(out: &mut String, a: &Attrs, links: &mut dyn Links) {
    let mut rp = RunProps {
        size: a.size.map(|s| s as f32 / 2.0),
        bold: a.bold.then_some(true),
        italic: a.italic.then_some(true),
        underline: a.underline.then_some(true),
        strike: a.strike.then_some(true),
        color: a.color.map(apex_slides::SColor::rgb),
        font: a.font.clone(),
        ..Default::default()
    };
    if a.superscript {
        rp.baseline = Some(30.0);
    } else if a.subscript {
        rp.baseline = Some(-25.0);
    }
    if let Some(fx) = &a.fx {
        if fx.all_caps {
            rp.caps = Some(Caps::All);
        } else if fx.small_caps {
            rp.caps = Some(Caps::Small);
        }
        if fx.spacing != 0 {
            rp.spacing = Some(fx.spacing as f32 / 20.0);
        }
    }
    // Attributes first, then the children in schema order.
    let mut s = String::new();
    write_run_props(&mut s, "rPr", &rp);
    let mut extra_attrs = String::new();
    if let Some(l) = &a.lang {
        let _ = write!(extra_attrs, r#" lang="{}""#, esc(l));
    }
    if a.no_proof {
        extra_attrs.push_str(r#" noProof="1""#);
    }
    let mut s = s.replacen("<a:rPr", &format!("<a:rPr{extra_attrs}"), 1);
    let mut kids = String::new();
    if let Some(h) = a.highlight {
        // After the fill, before the underline and the fonts.
        let hl = format!(r#"<a:highlight><a:srgbClr val="{h:06X}"/></a:highlight>"#);
        let at = ["<a:uLnTx", "<a:uLn", "<a:uFill", "<a:latin", "<a:ea ", "<a:cs ", "<a:sym"].iter().filter_map(|t| s.find(t)).min();
        match at {
            Some(i) => s.insert_str(i, &hl),
            None => kids.push_str(&hl),
        }
    }
    if let Some(l) = &a.link {
        match l.strip_prefix('#') {
            Some(act) => {
                let verb = match act {
                    "prev" => "previousslide",
                    "first" => "firstslide",
                    "last" => "lastslide",
                    "end" => "endshow",
                    _ => "nextslide",
                };
                let _ = write!(kids, r#"<a:hlinkClick r:id="" action="ppaction://hlinkshowjump?jump={verb}"/>"#);
            }
            None => {
                let id = links.link(l);
                let _ = write!(kids, r#"<a:hlinkClick r:id="{id}"/>"#);
            }
        }
    }
    let mut res = String::new();
    if kids.is_empty() {
        res.push_str(&s);
    } else if let Some(open) = s.strip_suffix("/>") {
        let _ = write!(res, "{open}>{kids}</a:rPr>");
    } else {
        // Color and fonts come before the link.
        let at = s.rfind("</a:rPr>").unwrap_or(s.len());
        let _ = write!(res, "{}{kids}</a:rPr>", &s[..at]);
    }
    match &a.text_fx {
        Some(fx) => out.push_str(&crate::text_fx_xml::into_dml(&res, fx)),
        None => out.push_str(&res),
    }
}

/// Writes a paragraph (`a:p`).
pub(crate) fn write_para(out: &mut String, p: &Paragraph, props: Option<&ParaProps>, links: &mut dyn Links) {
    out.push_str("<a:p>");
    let empty = ParaProps::default();
    let pp = props.unwrap_or(&empty);
    if !pp.is_empty() || p.style.level > 0 {
        write_para_props(out, "pPr", pp, p.style.level);
    }
    let mut at = 0;
    let mut last = Attrs::default();
    for r in &p.runs {
        let piece = &p.text[at.min(p.text.len())..(at + r.len).min(p.text.len())];
        at += r.len;
        last = r.attrs.clone();
        let mut buf = String::new();
        let flush = |out: &mut String, buf: &mut String, links: &mut dyn Links| {
            if !buf.is_empty() {
                out.push_str("<a:r>");
                write_run_attrs(out, &r.attrs, links);
                let _ = write!(out, "<a:t>{}</a:t></a:r>", esc(buf));
                buf.clear();
            }
        };
        for c in piece.chars() {
            match c {
                LINE_BREAK => {
                    flush(out, &mut buf, links);
                    out.push_str("<a:br>");
                    write_run_attrs(out, &r.attrs, links);
                    out.push_str("</a:br>");
                }
                OBJECT => {
                    flush(out, &mut buf, links);
                    if let Some(f) = &r.attrs.field {
                        let ty = match (&f.kind, f.format.as_deref()) {
                            (FieldKind::Page, _) => "slidenum".to_string(),
                            (_, Some(t)) if t.starts_with("datetime") => t.to_string(),
                            _ => "datetime1".to_string(),
                        };
                        let _ = write!(out, r#"<a:fld id="{{{}}}" type="{ty}">"#, field_guid(&ty, at));
                        write_run_attrs(out, &r.attrs, links);
                        let _ = write!(out, "<a:t>{}</a:t></a:fld>", esc(if f.cached.is_empty() { "‹#›" } else { &f.cached }));
                    }
                }
                c => buf.push(c),
            }
        }
        flush(out, &mut buf, links);
    }
    let mut end = String::new();
    write_run_attrs(&mut end, &last, links);
    out.push_str(&end.replacen("<a:rPr", "<a:endParaRPr", 1).replace("</a:rPr>", "</a:endParaRPr>"));
    out.push_str("</a:p>");
}

/// A GUID for a field (stable for the same place and type).
fn field_guid(ty: &str, at: usize) -> String {
    let h = ty.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193)) ^ at as u32;
    format!("{h:08X}-5D2A-4B4E-9F3C-{:012X}", (h as u64).wrapping_mul(2_654_435_761) & 0xFFFF_FFFF_FFFF)
}

/// Writes a text body as `tag` ("p:txBody", "a:txBody").
pub(crate) fn write_text_body(out: &mut String, tag: &str, t: &TextBody, links: &mut dyn Links) {
    let _ = write!(out, "<{tag}>");
    write_body_props(out, &t.body);
    write_list_style(out, "a:lstStyle", &t.list);
    for (i, p) in t.paras.iter().enumerate() {
        write_para(out, p, t.para_props(i), links);
    }
    if t.paras.is_empty() {
        out.push_str("<a:p/>");
    }
    let _ = write!(out, "</{tag}>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawingml::read_fragment;

    struct NoLinks;
    impl Links for NoLinks {
        fn link(&mut self, _: &str) -> String {
            "rId9".into()
        }
    }

    #[test]
    fn text_round_trip() {
        let xml = r#"<p:txBody xmlns:p="p" xmlns:a="a"><a:bodyPr anchor="ctr" lIns="0"><a:normAutofit fontScale="90000"/></a:bodyPr><a:lstStyle><a:lvl1pPr marL="0"><a:defRPr sz="2000"/></a:lvl1pPr></a:lstStyle><a:p><a:pPr lvl="1" algn="ctr"><a:buFont typeface="Arial"/><a:buChar char="–"/></a:pPr><a:r><a:rPr lang="en-US" sz="2400" b="1"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:rPr><a:t>Hello</a:t></a:r><a:br/><a:fld id="{X}" type="slidenum"><a:rPr/><a:t>3</a:t></a:fld></a:p><a:p><a:endParaRPr sz="1200"/></a:p></p:txBody>"#;
        let theme = Theme::default();
        let links = HashMap::new();
        let cx = TextCx { theme: &theme, map: ClrMap::default(), links: &links };
        let t = read_text_body(&read_fragment(xml).kids[0], &cx);
        assert_eq!(t.paras.len(), 2);
        assert_eq!(t.paras[0].style.level, 1);
        assert_eq!(t.para_props(0).unwrap().bullet, Some(Bullet::Char('–')));
        assert_eq!(t.paras[0].runs[0].attrs.size, Some(48));
        assert_eq!(t.paras[0].runs[0].attrs.color, Some(0xFF0000));
        assert_eq!(t.paras[1].runs[0].attrs.size, Some(24));
        assert_eq!(t.body.anchor, Some(VAnchor::Middle));
        let mut out = String::new();
        write_text_body(&mut out, "p:txBody", &t, &mut NoLinks);
        let back = read_text_body(&read_fragment(&out).kids[0], &cx);
        assert_eq!(back.paras, t.paras);
        assert_eq!(back.props, t.props);
        assert_eq!(back.body, t.body);
        assert_eq!(back.list, t.list);
    }
}
