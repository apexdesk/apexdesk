//! Form fields (AcroForm): reading them with their values, filling them
//! in (the value, the widgets' look and the simple calculations of
//! `AFSimple_Calculate`), and adding empty signature fields with a visible
//! look that `apex-sign` then signs.

use std::collections::{HashMap, HashSet};

use super::super::obj::{Dict, Obj};
use super::annots::Picture;
use super::appearance::{self as ap, n};
use super::{PdfDoc, obj_text, text_obj};

const READ_ONLY: i64 = 1;
const REQUIRED: i64 = 2;
const MULTILINE: i64 = 1 << 12;
const PASSWORD: i64 = 1 << 13;
const RADIO: i64 = 1 << 15;
const PUSH: i64 = 1 << 16;
const COMBO: i64 = 1 << 17;
const EDIT: i64 = 1 << 18;
const MULTI_SELECT: i64 = 1 << 21;

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    /// `date`: the date format asked for (`AFDate_FormatEx`).
    Text { multiline: bool, password: bool, max_len: Option<usize>, date: Option<String> },
    Check,
    Radio,
    /// Choices: (value, text shown).
    Combo { options: Vec<(String, String)>, edit: bool },
    List { options: Vec<(String, String)>, multi: bool },
    Push,
    Signature,
}

/// Where a field shows on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub id: u32,
    pub page: usize,
    /// Points on the page as shown.
    pub rect: [f32; 4],
    /// Check boxes and radio buttons: the value this one stands for.
    pub on: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalcOp {
    Sum,
    Product,
    Average,
    Min,
    Max,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// The full name ("parent.child").
    pub name: String,
    /// The object holding the value.
    pub holder: u32,
    pub kind: FieldKind,
    /// Text, the chosen value, or a check box's on value ("Off": off).
    pub value: String,
    pub widgets: Vec<Widget>,
    pub read_only: bool,
    pub required: bool,
    /// 0: fits the box.
    pub font_size: f32,
    /// 0 left, 1 centre, 2 right.
    pub align: u8,
    pub calc: Option<(CalcOp, Vec<String>)>,
    /// Decimals of a number format (`AFNumber_Format`).
    pub decimals: Option<usize>,
}

/// What a visible signature shows: lines of text, and a picture or a
/// drawn signature (strokes in points from the box's top left).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SigLook {
    pub lines: Vec<String>,
    pub picture: Option<Picture>,
    pub ink: Vec<Vec<[f32; 2]>>,
    pub color: u32,
}

/// The JavaScript of an action (string or stream).
fn js_of(doc: &PdfDoc, action: &Obj) -> String {
    let Some(d) = doc.resolve(action).dict().cloned() else { return String::new() };
    match doc.key(&d, "JS") {
        Obj::Str(b) => super::super::text_string(&b),
        Obj::Stream(sd, raw) => String::from_utf8_lossy(&doc.decode(&sd, &raw)).into_owned(),
        _ => String::new(),
    }
}

/// The quoted strings of a piece of JavaScript.
fn quoted(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '"' || c == '\'' {
            let mut v = String::new();
            let mut esc = false;
            for d in it.by_ref() {
                if esc {
                    v.push(d);
                    esc = false;
                } else if d == '\\' {
                    esc = true;
                } else if d == c {
                    break;
                } else {
                    v.push(d);
                }
            }
            out.push(v);
        }
    }
    out
}

/// `AFSimple_Calculate("SUM", new Array("a", "b"))`.
fn parse_calc(js: &str) -> Option<(CalcOp, Vec<String>)> {
    let at = js.find("AFSimple_Calculate")?;
    let args = quoted(&js[at..]);
    let op = match args.first()?.to_ascii_uppercase().as_str() {
        "SUM" => CalcOp::Sum,
        "PRD" => CalcOp::Product,
        "AVG" => CalcOp::Average,
        "MIN" => CalcOp::Min,
        "MAX" => CalcOp::Max,
        _ => return None,
    };
    // Names may also come as one comma separated string.
    let names: Vec<String> = args[1..].iter().flat_map(|a| a.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()).filter(|s| !s.is_empty()).collect();
    Some((op, names))
}

/// A number typed in a field ("1.234,5" and "1,234.5" both read).
pub fn parse_number(s: &str) -> Option<f64> {
    let t: String = s.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-')).collect();
    if t.is_empty() {
        return None;
    }
    let (dot, comma) = (t.rfind('.'), t.rfind(','));
    let t = match (dot, comma) {
        (Some(d), Some(c)) if c > d => t.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => t.replace(',', ""),
        (None, Some(_)) => t.replace(',', "."),
        _ => t,
    };
    t.parse().ok()
}

pub fn calculate(op: CalcOp, vals: &[f64]) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    match op {
        CalcOp::Sum => vals.iter().sum(),
        CalcOp::Product => vals.iter().product(),
        CalcOp::Average => vals.iter().sum::<f64>() / vals.len() as f64,
        CalcOp::Min => vals.iter().copied().fold(f64::INFINITY, f64::min),
        CalcOp::Max => vals.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    }
}

fn format_number(v: f64, decimals: Option<usize>) -> String {
    match decimals {
        Some(d) => format!("{v:.d$}"),
        None => {
            let s = format!("{:.6}", v);
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        }
    }
}

/// What walking the field tree carries down.
#[derive(Clone, Default)]
struct Inherit {
    name: String,
    ft: Option<String>,
    ff: i64,
    v: Option<Obj>,
    da: Option<String>,
    q: Option<i64>,
    opt: Option<Obj>,
    holder: u32,
}

impl PdfDoc {
    /// The document's form fields (empty without a form).
    pub fn fields(&self) -> Vec<Field> {
        let Some((_, cat)) = self.catalog() else { return Vec::new() };
        let Some(form) = self.key_dict(&cat, "AcroForm") else { return Vec::new() };
        let pages = self.pages();
        // Which page each annotation is on.
        let mut page_of: HashMap<u32, usize> = HashMap::new();
        for (i, &p) in pages.iter().enumerate() {
            for a in self.annots_of(p).1 {
                if let Obj::Ref(n, _) = a {
                    page_of.insert(n, i);
                }
            }
        }
        let default_da = obj_text(&self.key(&form, "DA"));
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        if let Obj::Array(list) = self.key(&form, "Fields") {
            for f in list {
                if let Obj::Ref(n, _) = f {
                    let inh = Inherit { da: Some(default_da.clone()).filter(|s| !s.is_empty()), ..Default::default() };
                    self.walk_field(n, &inh, &pages, &page_of, &mut out, &mut seen, 0);
                }
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_field(&self, id: u32, up: &Inherit, pages: &[u32], page_of: &HashMap<u32, usize>, out: &mut Vec<Field>, seen: &mut HashSet<u32>, depth: u32) {
        if depth > 32 || !seen.insert(id) {
            return;
        }
        let Some(d) = self.dict(id) else { return };
        let mut inh = up.clone();
        let part = obj_text(&self.key(&d, "T"));
        if d.get("T").is_some() {
            inh.name = if up.name.is_empty() { part } else { format!("{}.{part}", up.name) };
            inh.holder = id;
            inh.v = None;
        }
        if inh.holder == 0 {
            inh.holder = id;
        }
        if let Some(ft) = self.key(&d, "FT").name() {
            inh.ft = Some(ft.to_string());
        }
        if let Some(ff) = self.key(&d, "Ff").int() {
            inh.ff = ff;
        }
        if let Some(v) = d.get("V") {
            inh.v = Some(self.resolve(v));
        }
        if d.get("DA").is_some() {
            inh.da = Some(obj_text(&self.key(&d, "DA")));
        }
        if let Some(q) = self.key(&d, "Q").int() {
            inh.q = Some(q);
        }
        if d.get("Opt").is_some() {
            inh.opt = Some(self.key(&d, "Opt"));
        }
        let kids: Vec<u32> = match self.key(&d, "Kids") {
            Obj::Array(a) => a.iter().filter_map(|k| if let Obj::Ref(n, _) = k { Some(*n) } else { None }).collect(),
            _ => Vec::new(),
        };
        // Kids with names are fields of their own; the others are widgets.
        let field_kids: Vec<u32> = kids.iter().copied().filter(|k| self.dict(*k).is_some_and(|kd| kd.get("T").is_some())).collect();
        if !field_kids.is_empty() {
            for k in field_kids {
                self.walk_field(k, &inh, pages, page_of, out, seen, depth + 1);
            }
            return;
        }
        let widget_ids: Vec<u32> = if kids.is_empty() { vec![id] } else { kids };
        let mut widgets = Vec::new();
        for w in widget_ids {
            let Some(wd) = self.dict(w) else { continue };
            let page = page_of.get(&w).copied().or_else(|| match wd.get("P") {
                Some(Obj::Ref(p, _)) => pages.iter().position(|x| x == p),
                _ => None,
            });
            let Some(page) = page else { continue };
            let r = self.nums(wd.get("Rect").unwrap_or(&Obj::Null));
            if r.len() != 4 {
                continue;
            }
            let g = self.geom(pages[page]);
            let rect = g.rect_to_view([r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
            let on = self.key_dict(&wd, "AP").and_then(|a| self.key_dict(&a, "N")).and_then(|nd| nd.0.iter().map(|(k, _)| k.to_string()).find(|k| k != "Off")).unwrap_or_else(|| "Yes".into());
            widgets.push(Widget { id: w, page, rect, on });
        }
        let ff = inh.ff;
        let options = || -> Vec<(String, String)> {
            match inh.opt.as_ref().map(|o| self.resolve(o)) {
                Some(Obj::Array(a)) => a
                    .iter()
                    .map(|o| match self.resolve(o) {
                        Obj::Array(p) if p.len() >= 2 => (obj_text(&self.resolve(&p[0])), obj_text(&self.resolve(&p[1]))),
                        o => {
                            let t = obj_text(&o);
                            (t.clone(), t)
                        }
                    })
                    .collect(),
                _ => Vec::new(),
            }
        };
        let aa = self.key_dict(&d, "AA");
        let js = |k: &str| aa.as_ref().and_then(|a| a.get(k)).map(|o| js_of(self, o)).unwrap_or_default();
        let format_js = js("F");
        let kind = match inh.ft.as_deref() {
            Some("Tx") => {
                let date = format_js.find("AFDate_Format").map(|at| quoted(&format_js[at..]).into_iter().next().unwrap_or_else(|| "dd/mm/yyyy".into()));
                FieldKind::Text { multiline: ff & MULTILINE != 0, password: ff & PASSWORD != 0, max_len: self.key(&d, "MaxLen").int().map(|m| m.max(0) as usize), date }
            }
            Some("Btn") if ff & PUSH != 0 => FieldKind::Push,
            Some("Btn") if ff & RADIO != 0 => FieldKind::Radio,
            Some("Btn") => FieldKind::Check,
            Some("Ch") if ff & COMBO != 0 => FieldKind::Combo { options: options(), edit: ff & EDIT != 0 },
            Some("Ch") => FieldKind::List { options: options(), multi: ff & MULTI_SELECT != 0 },
            Some("Sig") => FieldKind::Signature,
            _ => return,
        };
        let value = match (&kind, &inh.v) {
            (FieldKind::Check | FieldKind::Radio, Some(v)) => v.name().unwrap_or("Off").to_string(),
            (FieldKind::Check | FieldKind::Radio, None) => "Off".into(),
            (FieldKind::Signature, v) => if v.is_some() { "signed".into() } else { String::new() },
            (_, Some(Obj::Array(a))) => a.iter().map(obj_text).collect::<Vec<_>>().join("\n"),
            (_, Some(v)) => obj_text(v),
            (_, None) => String::new(),
        };
        let da = inh.da.clone().unwrap_or_default();
        let font_size = da.split_whitespace().collect::<Vec<_>>().windows(2).find(|w| w[1] == "Tf").and_then(|w| w[0].parse::<f32>().ok()).unwrap_or(0.0);
        let decimals = format_js.find("AFNumber_Format").and_then(|at| {
            let rest = &format_js[at..];
            let open = rest.find('(')?;
            rest[open + 1..].split(',').next()?.trim().parse::<usize>().ok()
        });
        out.push(Field {
            name: inh.name.clone(),
            holder: inh.holder,
            kind,
            value,
            widgets,
            read_only: ff & READ_ONLY != 0,
            required: ff & REQUIRED != 0,
            font_size,
            align: inh.q.unwrap_or(0).clamp(0, 2) as u8,
            calc: parse_calc(&js("C")),
            decimals,
        });
    }

    /// Fills field `name` with `value` (one undo step) and runs the
    /// calculations that depend on it.
    pub fn set_field(&mut self, name: &str, value: &str) {
        let fields = self.fields();
        let Some(f) = fields.iter().find(|f| f.name == name) else { return };
        if f.value == value || f.read_only {
            return;
        }
        self.begin("pdf.undo.fill");
        self.write_value(f, value);
        self.recalc();
        self.end();
    }

    /// Recomputes the calculated fields in the form's order.
    fn recalc(&mut self) {
        for _ in 0..4 {
            let fields = self.fields();
            let mut changed = false;
            let values: HashMap<&str, &str> = fields.iter().map(|f| (f.name.as_str(), f.value.as_str())).collect();
            for f in fields.iter().filter(|f| f.calc.is_some()) {
                let Some((op, names)) = &f.calc else { continue };
                let vals: Vec<f64> = names.iter().filter_map(|n| values.get(n.as_str()).and_then(|v| parse_number(v))).collect();
                let v = format_number(calculate(*op, &vals), f.decimals);
                if v != f.value {
                    self.write_value(f, &v);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn write_value(&mut self, f: &Field, value: &str) {
        let Some(mut h) = self.dict(f.holder) else { return };
        match f.kind {
            FieldKind::Check | FieldKind::Radio => {
                let v = if value.is_empty() { "Off" } else { value };
                h.set("V", Obj::name_of(v));
                self.set(f.holder, Obj::Dict(h));
                for w in &f.widgets {
                    if let Some(mut wd) = self.dict(w.id) {
                        wd.set("AS", Obj::name_of(if w.on == v { &w.on } else { "Off" }));
                        self.set(w.id, Obj::Dict(wd));
                    }
                }
            }
            FieldKind::List { multi: true, .. } if value.contains('\n') => {
                h.set("V", Obj::Array(value.lines().map(text_obj).collect()));
                self.set(f.holder, Obj::Dict(h));
                self.text_looks(f, value);
            }
            FieldKind::Push | FieldKind::Signature => {}
            _ => {
                h.set("V", text_obj(value));
                self.set(f.holder, Obj::Dict(h));
                self.text_looks(f, value);
            }
        }
    }

    /// Redraws the widgets of a text or choice field showing `value`.
    fn text_looks(&mut self, f: &Field, value: &str) {
        let shown = match &f.kind {
            FieldKind::Combo { options, .. } | FieldKind::List { options, .. } => value.lines().map(|v| options.iter().find(|o| o.0 == v).map_or(v, |o| o.1.as_str())).collect::<Vec<_>>().join("\n"),
            FieldKind::Text { password: true, .. } => "*".repeat(value.chars().count()),
            _ => value.to_string(),
        };
        let multiline = matches!(f.kind, FieldKind::Text { multiline: true, .. } | FieldKind::List { .. });
        for wg in &f.widgets {
            let Some(mut wd) = self.dict(wg.id) else { continue };
            let r = self.nums(wd.get("Rect").unwrap_or(&Obj::Null));
            if r.len() != 4 {
                continue;
            }
            let (bw, bh) = ((r[2] - r[0]).abs(), (r[3] - r[1]).abs());
            // A widget turned with /MK /R is drawn turned.
            let turn = self.key_dict(&wd, "MK").and_then(|mk| self.key(&mk, "R").int()).unwrap_or(0).rem_euclid(360);
            let (w, h, m) = match turn {
                90 => (bh, bw, [0.0, 1.0, -1.0, 0.0, bw, 0.0]),
                180 => (bw, bh, [-1.0, 0.0, 0.0, -1.0, bw, bh]),
                270 => (bh, bw, [0.0, -1.0, 1.0, 0.0, 0.0, bh]),
                _ => (bw, bh, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
            };
            let size = if f.font_size > 0.0 { f.font_size as f64 } else if multiline { 10.0 } else { ((h - 4.0) * 0.75).clamp(4.0, 12.0) };
            let lines = if multiline { ap::wrap(&shown, w - 4.0, size, false) } else { vec![shown.replace('\n', " ")] };
            let mut s = String::from("/Tx BMC\nq 1 1 ");
            s.push_str(&format!("{} {} re W n\n", n(w - 2.0), n(h - 2.0)));
            let color = obj_text(&self.key(&wd, "DA")).split_whitespace().collect::<Vec<_>>().windows(4).find(|x| x[3] == "rg").map(|x| format!("{} {} {}", x[0], x[1], x[2]));
            let block = if multiline {
                ap::text_block(&lines, w, h, size, 0, 2.0, f.align)
            } else {
                let l = lines.first().cloned().unwrap_or_default();
                let tw = ap::width(&l, false, size);
                let x = match f.align {
                    1 => (w - tw) / 2.0,
                    2 => w - 2.0 - tw,
                    _ => 2.0,
                };
                format!("BT /{} {} Tf 0 g 1 0 0 1 {} {} Tm {} Tj ET\n", ap::HELV, n(size), n(x), n((h - size * 0.72) / 2.0), ap::string(&l))
            };
            let block = match color {
                Some(c) => block.replacen(" 0 g ", &format!(" {c} rg "), 1).replacen(" 0 rg\n", &format!(" {c} rg\n"), 1),
                None => block,
            };
            s.push_str(&block);
            s.push_str("Q\nEMC\n");
            let form = ap::form(w, h, m, s.as_bytes(), ap::resources(None, None));
            let fnum = self.add(form);
            wd.set("AP", Obj::Dict(Dict::of(vec![("N", Obj::Ref(fnum, 0))])));
            self.set(wg.id, Obj::Dict(wd));
        }
    }

    /// Adds an empty signature field showing `look` at `rect` (points
    /// shown) on page `page`; returns its name, for signing it.
    pub fn add_signature_field(&mut self, page: usize, rect: [f32; 4], look: &SigLook) -> Option<String> {
        let pages = self.pages();
        let &pn = pages.get(page)?;
        let names: HashSet<String> = self.fields().into_iter().map(|f| f.name).collect();
        let name = (1..).map(|i| format!("Signature{i}")).find(|n| !names.contains(n))?;
        self.begin("pdf.undo.sign_field");
        let g = self.geom(pn);
        let user = g.rect_to_user(rect);
        let (w, h) = ((rect[2] - rect[0]) as f64, (rect[3] - rect[1]) as f64);
        let id = self.add(Obj::Null);
        let mut d = Dict::of(vec![
            ("Type", Obj::name_of("Annot")),
            ("Subtype", Obj::name_of("Widget")),
            ("FT", Obj::name_of("Sig")),
            ("T", text_obj(&name)),
            ("F", Obj::Int(4)),
            ("Rect", Obj::Array(user.iter().map(|v| Obj::Real(ap::round(*v))).collect())),
            ("P", Obj::Ref(pn, 0)),
        ]);
        let (content, xobjects) = self.sig_look(look, w, h);
        let mut res = ap::resources(None, None);
        if !xobjects.is_empty() {
            res.set("XObject", Obj::Dict(Dict(xobjects.iter().map(|(k, v)| (std::sync::Arc::from(k.as_str()), Obj::Ref(*v, 0))).collect())));
        }
        let f = self.add(ap::form(w, h, g.upright(), content.as_bytes(), res));
        d.set("AP", Obj::Dict(Dict::of(vec![("N", Obj::Ref(f, 0))])));
        self.set(id, Obj::Dict(d));
        let (_, mut list) = self.annots_of(pn);
        list.push(Obj::Ref(id, 0));
        self.set_annots_of(pn, list);
        // The form lists the field.
        let (cn, cat) = self.catalog()?;
        let (form_n, mut form) = match cat.get("AcroForm") {
            Some(Obj::Ref(n, _)) => (Some(*n), self.dict(*n).unwrap_or_default()),
            Some(Obj::Dict(fd)) => (None, fd.clone()),
            _ => (None, Dict::default()),
        };
        let fields_obj = form.get("Fields").cloned();
        match fields_obj {
            Some(Obj::Ref(fl, _)) => {
                let mut a = self.get(fl).and_then(Obj::array).map(<[Obj]>::to_vec).unwrap_or_default();
                a.push(Obj::Ref(id, 0));
                self.set(fl, Obj::Array(a));
            }
            Some(Obj::Array(mut a)) => {
                a.push(Obj::Ref(id, 0));
                form.set("Fields", Obj::Array(a));
            }
            _ => form.set("Fields", Obj::Array(vec![Obj::Ref(id, 0)])),
        }
        form.set("SigFlags", Obj::Int(3));
        match form_n {
            Some(fnum) => self.set(fnum, Obj::Dict(form)),
            None => {
                let mut cat = cat;
                cat.set("AcroForm", Obj::Dict(form));
                self.set(cn, Obj::Dict(cat));
            }
        }
        self.end();
        Some(name)
    }

    /// A signature's look: picture or strokes on the left (or all the
    /// box without text), the lines on the right.
    fn sig_look(&mut self, look: &SigLook, w: f64, h: f64) -> (String, Vec<(String, u32)>) {
        let mut s = String::new();
        let mut xobjects = Vec::new();
        let has_mark = look.picture.is_some() || !look.ink.is_empty();
        let mark_w = if look.lines.is_empty() { w } else if has_mark { w * 0.5 } else { 0.0 };
        if let Some(p) = &look.picture {
            let (img, mask) = ap::image(p.w, p.h, &p.rgba);
            let img = match (mask, img) {
                (Some(m), Obj::Stream(mut d, data)) => {
                    let mn = self.add(m);
                    d.set("SMask", Obj::Ref(mn, 0));
                    Obj::Stream(d, data)
                }
                (_, o) => o,
            };
            let inum = self.add(img);
            xobjects.push(("Im0".to_string(), inum));
            // Fitted, keeping its proportions.
            let z = (mark_w / p.w.max(1) as f64).min(h / p.h.max(1) as f64);
            let (iw, ih) = (p.w as f64 * z, p.h as f64 * z);
            s.push_str(&format!("q {} 0 0 {} {} {} cm /Im0 Do Q\n", n(iw), n(ih), n((mark_w - iw) / 2.0), n((h - ih) / 2.0)));
        }
        if !look.ink.is_empty() {
            // Strokes fitted in the mark's box.
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for p in look.ink.iter().flatten() {
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
            let (sw, sh) = (((x1 - x0) as f64).max(1.0), ((y1 - y0) as f64).max(1.0));
            let z = ((mark_w - 4.0) / sw).min((h - 4.0) / sh);
            let (ox, oy) = ((mark_w - sw * z) / 2.0, (h - sh * z) / 2.0);
            s.push_str(&format!("{} RG 1.5 w 1 J 1 j\n", ap::rgb(look.color)));
            for stroke in &look.ink {
                for (i, p) in stroke.iter().enumerate() {
                    let x = ox + (p[0] - x0) as f64 * z;
                    let y = h - (oy + (p[1] - y0) as f64 * z);
                    s.push_str(&format!("{} {} {}\n", n(x), n(y), if i == 0 { "m" } else { "l" }));
                }
                s.push_str("S\n");
            }
        }
        if !look.lines.is_empty() {
            let tw = w - mark_w;
            let longest = look.lines.iter().map(|l| ap::width(l, false, 10.0)).fold(1.0, f64::max);
            let size = (10.0 * (tw - 6.0) / longest).min((h - 4.0) / (look.lines.len() as f64 * 1.15)).clamp(3.0, 14.0);
            let block = ap::text_block(&look.lines, tw, h, size, look.color, 3.0, 0);
            s.push_str(&format!("q 1 0 0 1 {} 0 cm\n{block}Q\n", n(mark_w)));
        }
        (s, xobjects)
    }
}
