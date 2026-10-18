//! Named styles in odt (a child module of `odt`, to read its style table):
//! changes to the built-in paragraph styles, custom paragraph styles and
//! character (text) styles.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Align, Attrs, Document, LineRule, ParaKind, ParaStyle, Paragraph, StyleDef, Styles};

use super::{Ctx, kind_from_name, named_style, pt};
use crate::is_mono_font;
use crate::xml::esc;

/// A style name as the file writes it (an NCName: odt escapes other
/// characters as `_xx_`, like LibreOffice's `_20_` for spaces).
pub(super) fn file_name(id: &str) -> String {
    let mut out = String::new();
    for (i, c) in id.chars().enumerate() {
        if c.is_alphanumeric() && !(i == 0 && c.is_numeric()) || (i > 0 && matches!(c, '-' | '.')) {
            out.push(c);
        } else {
            let _ = write!(out, "_{:x}_", c as u32);
        }
    }
    out
}

/// Names that are Normal under another name.
fn is_normal(name: &str, display: &str) -> bool {
    ["standard", "default", "default paragraph style", "normal"].iter().any(|n| name.eq_ignore_ascii_case(n) || display.eq_ignore_ascii_case(n))
}

/// The style that stands for Normal, whatever the hashing order:
/// "Standard" first, then the other names in order, by name then display
/// name (ties: the smallest name).
fn normal_style(para: &HashMap<String, super::StyleDef>) -> Option<&super::StyleDef> {
    let names = ["standard", "normal", "default", "default paragraph style"];
    let rank = |name: &str, s: &super::StyleDef| names.iter().position(|n| name.eq_ignore_ascii_case(n)).map(|i| i * 2).or_else(|| names.iter().position(|n| s.display.eq_ignore_ascii_case(n)).map(|i| i * 2 + 1));
    para.iter().filter(|(_, s)| !s.auto).filter_map(|(name, s)| Some((rank(name, s)?, name, s))).min_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1))).map(|x| x.2)
}

impl Ctx {
    /// The first named (not automatic) paragraph style of a chain.
    pub(super) fn named_para(&self, name: &str) -> Option<String> {
        let mut cur = Some(name.to_string());
        let mut n = 0;
        while let Some(c) = cur {
            n += 1;
            let s = self.para.get(&c)?;
            if !s.auto {
                return Some(c);
            }
            if n > 12 {
                return None;
            }
            cur = s.parent.clone();
        }
        None
    }

    /// The first named text style of a chain, unless it is a link or code
    /// style (those have their own looks).
    pub(super) fn named_text(&self, name: &str) -> Option<String> {
        let mut cur = Some(name.to_string());
        let mut n = 0;
        while let Some(c) = cur {
            n += 1;
            let s = self.text.get(&c)?;
            if !s.auto {
                let d = s.display.to_ascii_lowercase();
                let code = s.mono == Some(true) || d.contains("source") || d.contains("code") || d.contains("teletype");
                let link = d.contains("internet link") || d.contains("hyperlink") || d.contains("visited");
                return (!code && !link).then_some(c);
            }
            if n > 12 {
                return None;
            }
            cur = s.parent.clone();
        }
        None
    }

    /// Text attributes from the automatic styles of a chain only.
    pub(super) fn auto_text_attrs(&self, name: &str) -> Attrs {
        let mut chain = Vec::new();
        let mut cur = Some(name);
        while let Some(c) = cur {
            let Some(s) = self.text.get(c).filter(|s| s.auto) else { break };
            if chain.len() > 12 {
                break;
            }
            chain.push(s);
            cur = s.parent.as_deref();
        }
        let mut a = Attrs::default();
        for s in chain.iter().rev() {
            super::overlay(&mut a, &s.attrs);
        }
        a
    }

    /// Properties of a named style and its ancestors up to (not including)
    /// the first built-in style or Normal.
    fn own_chain(map: &HashMap<String, super::StyleDef>, name: &str, stop_at_builtin: bool) -> StyleDef {
        let mut chain = Vec::new();
        let mut cur = Some(name.to_string());
        while let Some(c) = cur {
            let Some(s) = map.get(&c) else { break };
            if chain.len() > 12 || is_normal(&c, &s.display) || (stop_at_builtin && !chain.is_empty() && kind_from_name(&s.display).is_some()) {
                break;
            }
            chain.push(s);
            cur = s.parent.clone();
        }
        Self::props(&chain)
    }

    /// Properties of a chain of styles (the first one wins).
    fn props(chain: &[&super::StyleDef]) -> StyleDef {
        let mut d = StyleDef::default();
        for s in chain.iter().rev() {
            let a = &s.attrs;
            if a.font.is_some() && s.mono != Some(true) {
                d.font.clone_from(&a.font);
            }
            if a.size.is_some() {
                d.size = a.size;
            }
            // (Plain over a parent's: not bold, italic or underlined.)
            if a.bold || (s.plain[0] && d.bold.is_some()) {
                d.bold = Some(a.bold);
            }
            if a.italic || (s.plain[1] && d.italic.is_some()) {
                d.italic = Some(a.italic);
            }
            if a.underline || (s.plain[2] && d.underline.is_some()) {
                d.underline = Some(a.underline);
            }
            if a.color.is_some() {
                d.color = a.color;
            }
            if let Some(fx) = &a.fx {
                let mut f = d.fx.as_deref().cloned().unwrap_or_default();
                f.overlay(fx);
                d.fx = Some(std::sync::Arc::new(f));
            }
            if s.align.is_some() {
                d.align = s.align;
            }
            if let Some((LineRule::Multiple, v)) = s.line {
                d.line_spacing = Some((v * 100.0).round() / 100.0);
            }
            let [left, _, indent, top, bottom] = s.margins;
            if left.is_some() {
                d.indent_left = left;
            }
            if indent.is_some() {
                d.first_line = indent;
            }
            if top.is_some() {
                d.space_before = top;
            }
            if bottom.is_some() {
                d.space_after = bottom;
            }
            if s.background.is_some() {
                d.shading = s.background;
            }
            if s.no_widows {
                d.no_widow_control = Some(true);
            }
            if s.contextual {
                d.contextual_spacing = Some(true);
            }
            let mut deco = ParaStyle::default();
            crate::odt_layout::apply_para(&mut deco, &s.extra, d.indent_left.unwrap_or(0.0));
            if !deco.borders.is_empty() {
                d.borders = Some(deco.borders);
            }
            if let Some(x) = &a.asian {
                let mut m = d.asian.as_deref().cloned().unwrap_or_default();
                m.overlay(x);
                d.asian = Some(Arc::new(m));
            }
            d.asian_para.overlay(&s.extra.asian);
            if s.extra.tabs.is_some() {
                d.tabs = Some(deco.tabs);
            }
        }
        d
    }

    /// Our id of the paragraph style a file's style name stands for.
    fn para_id(&self, name: &str) -> Option<Arc<str>> {
        let s = self.para.get(name)?;
        if is_normal(name, &s.display) {
            return Some("Normal".into());
        }
        match kind_from_name(&s.display).or_else(|| kind_from_name(name)) {
            Some(k) if !k.is_list() => Some(apex_doc::kind_id(k).into()),
            _ => Some(name.into()),
        }
    }
}

/// Reader side: named paragraph styles seen, by temporary number.
#[derive(Default)]
pub(super) struct Used {
    names: Vec<String>,
}

impl Used {
    pub(super) fn number(&mut self, name: &str) -> u16 {
        match self.names.iter().position(|n| n == name) {
            Some(i) => i as u16 + 1,
            None => {
                self.names.push(name.to_string());
                self.names.len() as u16
            }
        }
    }
}

/// Turns the temporary style numbers and run style names into document
/// styles.
pub(super) fn import(ctx: &Ctx, paras: &mut [Paragraph], used: &Used) -> Styles {
    let mut styles = Styles::default();
    let mut numbers: HashMap<u16, u16> = HashMap::new();
    let mut builtins: Vec<(ParaKind, String)> = Vec::new();
    for p in paras.iter_mut() {
        let n = std::mem::take(&mut p.style.custom);
        if n == 0 {
            continue;
        }
        let Some(name) = used.names.get(n as usize - 1) else { continue };
        let Some(s) = ctx.para.get(name) else { continue };
        let kind = p.style.kind;
        let builtin = kind_from_name(&s.display).or_else(|| kind_from_name(name));
        if builtin.is_some() || is_normal(name, &s.display) || kind.is_list() {
            if matches!(kind, ParaKind::Title | ParaKind::Heading(_) | ParaKind::Quote) && builtin == Some(kind) && !builtins.iter().any(|(k, _)| *k == kind) {
                builtins.push((kind, name.clone()));
            }
            continue;
        }
        let custom = match numbers.get(&n) {
            Some(&c) => c,
            None => {
                let mut def = Ctx::own_chain(&ctx.para, name, true);
                def.id = name.as_str().into();
                def.name = s.display.as_str().into();
                def.kind = if kind == ParaKind::Code { ParaKind::Normal } else { kind };
                def.next = s.next.as_deref().and_then(|n| ctx.para_id(n)).filter(|n| **n != **name);
                styles.defs.push(def);
                let c = styles.defs.len() as u16;
                numbers.insert(n, c);
                c
            }
        };
        p.style.custom = custom;
        if let Some(d) = styles.custom(custom) {
            let st = &mut p.style;
            if st.align == Align::Left {
                if let Some(a) = d.align {
                    st.align = a;
                }
            }
            if st.space_before.is_none() {
                st.space_before = d.space_before;
            }
            if st.space_after.is_none() {
                st.space_after = d.space_after;
            }
            fill_deco(st, d);
            if st.line_spacing == 0.0 {
                if let Some(l) = d.line_spacing.filter(|l| (l - 1.0).abs() > 0.01) {
                    st.line_spacing = l;
                }
            }
            if st.indent_left == 0.0 && st.level == 0 {
                if let Some(l) = d.indent_left {
                    st.indent_left = l;
                }
            }
            if st.first_line == 0.0 {
                if let Some(f) = d.first_line {
                    st.first_line = f;
                }
            }
        }
    }
    // Normal as the file has it (what it doesn't set comes from the
    // document defaults), and the space around normal paragraphs that
    // don't set it: Normal's, else the document defaults'.
    let normal = normal_style(&ctx.para).map(|n| {
        let mut def = Ctx::props(&[n]);
        // Our documents' Normal isn't a change.
        if ctx.defaults.is_none() && (def.space_before, def.space_after, def.line_spacing) == (Some(0.0), Some(7.0), Some(1.15)) {
            (def.space_before, def.space_after, def.line_spacing) = (None, None, None);
        }
        def
    });
    let defaults = ctx.defaults.as_ref().map(|d| &d.0);
    let before = normal.as_ref().and_then(|d| d.space_before).or(defaults.and_then(|d| d.space_before));
    let after = normal.as_ref().and_then(|d| d.space_after).or(defaults.and_then(|d| d.space_after));
    let (db, da) = ParaStyle::default().default_spacing();
    for p in paras.iter_mut().filter(|p| p.style.kind == ParaKind::Normal) {
        if p.style.space_before.is_none() {
            p.style.space_before = before.filter(|&b| b != db);
        }
        if p.style.space_after.is_none() {
            p.style.space_after = after.filter(|&a| a != da);
        }
    }
    if let Some(def) = normal.filter(|d| !d.is_empty()) {
        // Widow control off in Normal is off in the styles based on it.
        if def.no_widow_control == Some(true) {
            for p in paras.iter_mut().filter(|p| styles.custom(p.style.custom).is_none_or(|d| d.no_widow_control.is_none())) {
                p.style.no_widow_control = true;
            }
        }
        styles.defs.push(StyleDef { id: apex_doc::kind_id(ParaKind::Normal).into(), name: "Normal".into(), kind: ParaKind::Normal, builtin: true, ..def });
    }
    // Changes to built-in styles: character formatting and alignment only
    // (spacing and indents of the built-in styles vary between programs).
    for (kind, name) in builtins {
        let full = Ctx::own_chain(&ctx.para, &name, false);
        let base = ParaStyle { kind, ..Default::default() };
        let mut def = StyleDef::default();
        if full.size.is_some_and(|s| s != kind.base_size()) {
            def.size = full.size;
        }
        if full.bold.unwrap_or(false) != kind.base_bold() {
            def.bold = Some(full.bold.unwrap_or(false));
        }
        if full.italic.is_some() && full.italic != Some(kind == ParaKind::Quote) {
            def.italic = full.italic;
        }
        if full.color.is_some() && !(kind == ParaKind::Quote && full.color == Some(0x555555)) {
            def.color = full.color;
        }
        if full.font.as_deref().is_some_and(|f| !is_mono_font(f)) {
            def.font = full.font;
        }
        if full.align.is_some_and(|a| a != base.align) {
            def.align = full.align;
        }
        def.shading = full.shading;
        def.borders = full.borders;
        def.tabs = full.tabs;
        def.asian = full.asian;
        def.asian_para = full.asian_para;
        def.next = ctx.para.get(&name).and_then(|s| s.next.as_deref()).and_then(|n| ctx.para_id(n)).filter(|n| &**n != "Normal");
        if def.is_empty() && def.next.is_none() {
            continue;
        }
        def.id = apex_doc::kind_id(kind).into();
        def.name = ctx.para.get(&name).map_or(name.as_str(), |s| s.display.as_str()).into();
        def.kind = kind;
        def.builtin = true;
        for p in paras.iter_mut().filter(|p| p.style.kind == kind && p.style.custom == 0) {
            if p.style.align == Align::Left {
                if let Some(a) = def.align {
                    p.style.align = a;
                }
            }
            fill_deco(&mut p.style, &def);
        }
        styles.defs.push(def);
    }
    // Character styles.
    let mut keep: HashMap<Arc<str>, bool> = HashMap::new();
    for p in paras.iter_mut() {
        for r in &mut p.runs {
            let Some(id) = r.attrs.char_style.clone() else { continue };
            if r.attrs.link.is_some() {
                r.attrs.char_style = None;
                continue;
            }
            let ok = *keep.entry(id.clone()).or_insert_with(|| {
                let Some(s) = ctx.text.get(&*id) else { return false };
                let full = Ctx::own_chain(&ctx.text, &id, false);
                let mut def = StyleDef { font: full.font, size: full.size, bold: full.bold, italic: full.italic, underline: full.underline, color: full.color, fx: full.fx, asian: full.asian, ..Default::default() };
                if def.is_empty() {
                    return false;
                }
                def.id = id.clone();
                def.name = s.display.as_str().into();
                def.char_style = true;
                styles.defs.push(def);
                true
            });
            if !ok {
                // An unknown or empty style: keep its look as direct formatting.
                let (a, _) = ctx.text_attrs(&id);
                r.attrs.char_style = None;
                super::overlay(&mut r.attrs, &a);
            }
        }
    }
    styles
}

/// Shading, borders, tabs and widow control of a style the paragraph
/// doesn't set itself.
fn fill_deco(st: &mut ParaStyle, d: &StyleDef) {
    if st.shading.is_none() {
        st.shading = d.shading;
    }
    if d.no_widow_control == Some(true) {
        st.no_widow_control = true;
    }
    if d.contextual_spacing == Some(true) {
        st.contextual_spacing = true;
    }
    if st.borders.is_empty() {
        if let Some(b) = d.borders {
            st.borders = b;
        }
    }
    if st.tabs.is_empty() {
        if let Some(t) = d.tabs {
            st.tabs = t;
        }
    }
}

// ---------------------------------------------------------------------------
// Writing

fn para_props(d: &StyleDef) -> String {
    let mut p = String::new();
    if let Some(a) = d.align {
        let v = match a {
            Align::Left => "start",
            Align::Center => "center",
            Align::Right => "end",
            Align::Justify => "justify",
        };
        let _ = write!(p, r#" fo:text-align="{v}""#);
    }
    if let Some(b) = d.space_before {
        let _ = write!(p, r#" fo:margin-top="{}""#, pt(b.max(0.0)));
    }
    if let Some(a) = d.space_after {
        let _ = write!(p, r#" fo:margin-bottom="{}""#, pt(a.max(0.0)));
    }
    if let Some(l) = d.line_spacing {
        let _ = write!(p, r#" fo:line-height="{}%""#, (l * 100.0).round() as i32);
    }
    if let Some(l) = d.indent_left {
        let _ = write!(p, r#" fo:margin-left="{}""#, pt(l));
    }
    if let Some(f) = d.first_line {
        let _ = write!(p, r#" fo:text-indent="{}""#, pt(f));
    }
    if let Some(c) = d.shading {
        let _ = write!(p, r##" fo:background-color="#{:06x}""##, c & 0xFF_FFFF);
    }
    if let Some(w) = d.no_widow_control {
        let n = if w { 0 } else { 2 };
        let _ = write!(p, r#" fo:widows="{n}" fo:orphans="{n}""#);
    }
    if let Some(c) = d.contextual_spacing {
        let _ = write!(p, r#" style:contextual-spacing="{c}""#);
    }
    let deco = ParaStyle { borders: d.borders.unwrap_or_default(), tabs: d.tabs.unwrap_or_default(), asian: d.asian_para, ..Default::default() };
    let (props, children) = crate::odt_layout::para_xml(&deco, d.indent_left.unwrap_or(0.0));
    p.push_str(&props);
    match (p.is_empty(), children.is_empty()) {
        (true, true) => p,
        (_, true) => format!("<style:paragraph-properties{p}/>"),
        _ => format!("<style:paragraph-properties{p}>{children}</style:paragraph-properties>"),
    }
}

/// `style:next-style-name` of a style with a next style.
fn next_attr(doc: &Document, d: &StyleDef) -> Option<String> {
    let (kind, custom) = doc.styles.para_style_of(d.next.as_deref()?)?;
    let name = doc.styles.custom(custom).map_or_else(|| named_style(kind).to_string(), |c| file_name(&c.id));
    Some(format!(r#" style:next-style-name="{}""#, esc(&name)))
}

fn text_props(d: &StyleDef) -> String {
    let mut p = String::new();
    if let Some(f) = &d.font {
        let f = crate::theme_xml::font_name(f);
        let _ = write!(p, r#" style:font-name="{f}" fo:font-family="'{f}'""#, f = esc(&f));
    }
    if let Some(s) = d.size {
        let _ = write!(p, r#" fo:font-size="{}pt""#, s as f32 / 2.0);
    }
    if let Some(b) = d.bold {
        let _ = write!(p, r#" fo:font-weight="{}""#, if b { "bold" } else { "normal" });
    }
    if let Some(i) = d.italic {
        let _ = write!(p, r#" fo:font-style="{}""#, if i { "italic" } else { "normal" });
    }
    let fx = d.fx.as_deref().cloned().unwrap_or_default();
    match d.underline {
        Some(true) => p.push_str(&crate::odf_fx::underline_attrs(&fx)),
        Some(false) => p.push_str(r#" style:text-underline-style="none""#),
        None => {}
    }
    if fx.double_strike {
        p.push_str(r#" style:text-line-through-style="solid" style:text-line-through-type="double""#);
    }
    p.push_str(&crate::odf_fx::attrs(&fx, false, false));
    if let Some(x) = &d.asian {
        p.push_str(&crate::odt_asian::text_attrs(x, None));
    }
    if let Some(c) = d.color {
        let _ = write!(p, r##" fo:color="#{:06x}""##, c & 0xFF_FFFF);
    }
    if p.is_empty() { p } else { format!("<style:text-properties{p}/>") }
}

/// Whether the built-in style of `kind` is written from the document's
/// styles instead of our fixed definition.
pub(super) fn overridden(doc: &Document, kind: ParaKind) -> bool {
    doc.styles.builtin(kind).is_some()
}

/// Named style definitions for styles.xml.
pub(super) fn styles_xml(doc: &Document) -> String {
    let mut s = String::new();
    for d in &doc.styles.defs {
        if d.builtin && !d.char_style {
            let kind = d.kind;
            let mut full = StyleDef::default();
            if kind != ParaKind::Normal {
                full.size = Some(kind.base_size());
                full.bold = Some(kind.base_bold());
            }
            if kind == ParaKind::Quote {
                full.italic = Some(true);
                full.color = Some(0x555555);
            }
            if d.size.is_some() {
                full.size = d.size;
            }
            if d.bold.is_some() {
                full.bold = d.bold;
            }
            if d.italic.is_some() {
                full.italic = d.italic;
            }
            if d.color.is_some() {
                full.color = d.color;
            }
            full.font.clone_from(&d.font);
            full.underline = d.underline;
            full.align = d.align;
            full.space_before = d.space_before;
            full.space_after = d.space_after;
            full.line_spacing = d.line_spacing;
            full.indent_left = d.indent_left;
            full.first_line = d.first_line;
            full.shading = d.shading;
            full.borders = d.borders;
            full.tabs = d.tabs;
            full.no_widow_control = d.no_widow_control;
            full.contextual_spacing = d.contextual_spacing;
            full.asian.clone_from(&d.asian);
            full.asian_para = d.asian_para;
            let name = named_style(kind);
            let (display, parent, extra) = match kind {
                ParaKind::Normal => (None, None, r#" style:class="text""#.to_string()),
                ParaKind::Heading(n) => (Some(format!("Heading {n}")), Some("Heading"), format!(r#" style:next-style-name="Standard" style:default-outline-level="{n}" style:class="text""#)),
                ParaKind::Title => (None, Some("Heading"), r#" style:next-style-name="Standard" style:class="chapter""#.to_string()),
                _ => (None, Some("Standard"), r#" style:class="html""#.to_string()),
            };
            let extra = match next_attr(doc, d) {
                Some(n) if extra.contains("next-style-name") => {
                    let i = extra.find(r#" style:next-style-name="Standard""#).unwrap_or(0);
                    format!("{}{n}{}", &extra[..i], &extra[i + r#" style:next-style-name="Standard""#.len()..])
                }
                Some(n) => format!("{n}{extra}"),
                None => extra,
            };
            let _ = write!(s, r#"<style:style style:name="{name}""#);
            if let Some(dn) = display {
                let _ = write!(s, r#" style:display-name="{dn}""#);
            }
            let _ = write!(s, r#" style:family="paragraph""#);
            if let Some(p) = parent {
                let _ = write!(s, r#" style:parent-style-name="{p}""#);
            }
            let _ = write!(s, r#"{extra}>{}{}</style:style>"#, para_props(&full), text_props(&full));
        } else {
            let family = if d.char_style { "text" } else { "paragraph" };
            let _ = write!(s, r#"<style:style style:name="{}" style:display-name="{}" style:family="{family}""#, esc(&file_name(&d.id)), esc(&d.name));
            if !d.char_style {
                let _ = write!(s, r#" style:parent-style-name="{}""#, named_style(d.kind));
                if let ParaKind::Heading(n) = d.kind {
                    let _ = write!(s, r#" style:default-outline-level="{}""#, n.clamp(1, 9));
                }
                if let Some(n) = next_attr(doc, d) {
                    s.push_str(&n);
                }
            }
            s.push('>');
            if !d.char_style {
                s.push_str(&para_props(d));
            }
            s.push_str(&text_props(d));
            s.push_str("</style:style>");
        }
    }
    s
}

/// The named parent style of a paragraph: its custom style or its kind's.
pub(super) fn para_parent(doc: &Document, st: &ParaStyle) -> String {
    match doc.styles.custom(st.custom) {
        Some(d) => file_name(&d.id),
        None => named_style(st.kind).to_string(),
    }
}
