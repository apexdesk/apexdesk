//! Form controls in xlsx: the VML shapes whose `x:ClientData` is a
//! button, check box, list... (Excel keeps them there for every version),
//! read into sheet shapes carrying a [`FormControl`] and written back
//! into the sheet's VML drawing, next to notes and objects. Settings not
//! understood are kept verbatim.

use std::fmt::Write as _;

use apex_doc::{Shape, ShapeKind};
use apex_sheet::{Anchor, ControlKind, FormControl, Sheet, SheetShape};

use crate::xlsx_notes::SheetParts;
use crate::xml::{esc, unescape};
use crate::zip::ZipReader;

const REL_VML: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing";
const SHAPETYPE: &str = r#"<v:shapetype id="_x0000_t201" coordsize="21600,21600" o:spt="201" path="m,l,21600r21600,l21600,xe"><v:stroke joinstyle="miter"/><v:path shadowok="f" o:extrusionok="f" strokeok="f" fillok="f" o:connecttype="rect"/><o:lock v:ext="edit" shapetype="t"/></v:shapetype>"#;
/// Settings written from the model (the rest goes to `extra`).
const KNOWN: [&str; 17] = [
    "Anchor", "MoveWithCells", "SizeWithCells", "FmlaLink", "FmlaRange", "Checked", "Sel", "Min", "Max", "Inc", "Page", "Val", "FmlaMacro", "DropLines", "Horiz", "Dx", "SelType",
];
const MAX_CONTROLS: usize = 2000;

/// Children of an `x:ClientData`: name, text (None when empty) and the
/// raw element.
fn children(body: &str) -> Vec<(&str, Option<&str>, &str)> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find('<') {
        let s = &rest[i..];
        let Some(gt) = s.find('>') else { break };
        let tag = &s[1..gt];
        if tag.starts_with('/') {
            rest = &s[gt + 1..];
            continue;
        }
        let name = tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        let local = name.rsplit(':').next().unwrap_or(name);
        if tag.ends_with('/') {
            out.push((local, None, &s[..gt + 1]));
            rest = &s[gt + 1..];
            continue;
        }
        let close = format!("</{name}>");
        let Some(e) = s.find(&close) else { break };
        out.push((local, Some(&s[gt + 1..e]), &s[..e + close.len()]));
        rest = &s[e + close.len()..];
    }
    out
}

/// Text of a VML text box: tags stripped, line breaks kept.
fn box_text(vml: &str) -> String {
    let Some(a) = vml.find("<v:textbox") else { return String::new() };
    let Some(b) = vml[a..].find("</v:textbox>") else { return String::new() };
    let inner = &vml[a..a + b];
    let inner = inner.find('>').map_or("", |i| &inner[i + 1..]);
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for ch in inner.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let t = tag.trim_start_matches('/').to_ascii_lowercase();
                if (t.starts_with("br") || (tag.starts_with("/div") && !out.is_empty())) && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            _ if in_tag => tag.push(ch),
            _ => out.push(ch),
        }
    }
    unescape(out.trim_end_matches('\n')).into_owned()
}

/// The form controls of the worksheet part at `path`, added to the
/// sheet's shapes.
pub fn read(z: &ZipReader, path: &str, sh: &mut Sheet) {
    let rels = crate::xlsx_ole::rels_of(z, path);
    let dir = path.rsplit_once('/').map_or("", |x| x.0);
    let Some(vml) = rels.values().find(|(ty, _, ext)| ty == REL_VML && !ext).and_then(|(_, t, _)| z.read_string(&crate::xlsx::resolve_target(dir, t)).ok()) else { return };
    read_vml(&vml, sh);
}

fn read_vml(vml: &str, sh: &mut Sheet) {
    let mut rest = vml;
    while let Some(i) = rest.find("<v:shape ") {
        let s = &rest[i..];
        let end = s.find("</v:shape>").map_or(s.len(), |e| e + "</v:shape>".len());
        rest = &s[end..];
        let shape = &s[..end];
        let Some(cd) = shape.find("<x:ClientData") else { continue };
        let cd = &shape[cd..];
        let Some(gt) = cd.find('>') else { continue };
        let ty = cd[..gt].split("ObjectType=\"").nth(1).and_then(|x| x.split('"').next()).unwrap_or("");
        let Some(kind) = ControlKind::from_vml(ty) else { continue };
        if sh.shapes.len() >= MAX_CONTROLS {
            return;
        }
        let body = cd[gt + 1..].split("</x:ClientData>").next().unwrap_or("");
        let mut c = FormControl::new(kind);
        let (mut anchor, mut moves, mut sizes) = (None, false, false);
        for (name, text, raw) in children(body) {
            let t = unescape(text.unwrap_or("").trim()).into_owned();
            let num = || t.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(-1.0e6, 1.0e6) as i32);
            match name {
                "Anchor" => {
                    let v: Vec<f32> = t.split(',').filter_map(|x| x.trim().parse::<f32>().ok()).collect();
                    if let &[c0, o0, r0, o1, c1, o2, r1, o3] = &v[..] {
                        let n = |x: f32| x.max(0.0) as u32;
                        anchor = Some([(n(c0), n(r0), o0, o1), (n(c1), n(r1), o2, o3)]);
                    }
                }
                "MoveWithCells" => moves = true,
                "SizeWithCells" => sizes = true,
                "FmlaLink" => c.link = t,
                "FmlaRange" => c.items = t,
                "FmlaMacro" => c.macro_name = t,
                "Checked" => c.value = num().unwrap_or(1),
                "Sel" | "Val" => c.value = num().unwrap_or(0),
                "Min" => c.min = num().unwrap_or(0),
                "Max" => c.max = num().unwrap_or(100),
                "Inc" => c.inc = num().unwrap_or(1),
                "Page" => c.page = num().unwrap_or(10),
                "DropLines" => c.lines = num().unwrap_or(8).max(1) as u32,
                "Horiz" => c.horizontal = true,
                // Excel's defaults, kept when they are not.
                "Dx" | "SelType" if matches!(t.as_str(), "" | "22" | "16" | "Single") => {}
                _ => c.extra.push_str(raw),
            }
        }
        let Some([from, to]) = anchor else { continue };
        let Some(img) = crate::xlsx_drawing::place_box(sh, from, to, !sizes) else { continue };
        if kind.has_text() {
            c.text = box_text(shape);
        }
        let mut s = SheetShape::new(Shape::new(ShapeKind::Rect), img.from, img.width, img.height);
        (s.dx, s.dy) = (img.dx, img.dy);
        s.anchor = match (moves, sizes) {
            (_, true) => Anchor::TwoCell,
            (true, false) => Anchor::OneCell,
            _ => Anchor::Absolute,
        };
        c.sanitize();
        s.control = Some(Box::new(c));
        s.id = sh.shapes.iter().map(|x| x.id).max().unwrap_or(0) + 1;
        sh.shapes.push(s);
    }
}

/// Writes the form controls of sheet `n` (1-based) into its VML drawing,
/// after the notes and objects already there.
pub fn sheet(sh: &Sheet, n: usize, parts: &mut SheetParts) {
    let mut vml = String::new();
    let mut id = parts.vml.as_deref().map_or(1024, |v| {
        v.match_indices("_x0000_s").filter_map(|(i, _)| v[i + 8..].split(|c: char| !c.is_ascii_digit()).next()?.parse::<usize>().ok()).max().unwrap_or(1024)
    });
    for (z, s) in sh.shapes.iter().enumerate() {
        let Some(c) = &s.control else { continue };
        id += 1;
        let [from, to] = crate::xlsx_drawing::box_corners(sh, s.from, s.dx, s.dy, s.width, s.height);
        let (x, y) = s.pos(sh);
        let text = if c.kind.has_text() {
            let lines: Vec<String> = c.text.lines().map(|l| esc(l).into_owned()).collect();
            format!(r#"<v:textbox style="mso-direction-alt:auto" o:singleclick="f"><div style="text-align:left"><font face="Calibri" size="220" color="auto">{}</font></div></v:textbox>"#, lines.join("<br/>"))
        } else {
            String::new()
        };
        let fill = match c.kind {
            ControlKind::Button => r#" fillcolor="buttonFace [67]" strokecolor="windowText [64]" o:insetmode="auto""#,
            _ => r#" filled="f" fillcolor="window [65]" stroked="f" strokecolor="windowText [64]" o:insetmode="auto""#,
        };
        let _ = write!(
            vml,
            r##"<v:shape id="_x0000_s{id}" type="#_x0000_t201" style="position:absolute;margin-left:{:.2}pt;margin-top:{:.2}pt;width:{:.2}pt;height:{:.2}pt;z-index:{};mso-wrap-style:tight"{fill}>{text}<x:ClientData ObjectType="{}">"##,
            x * 0.75,
            y * 0.75,
            s.width * 0.75,
            s.height * 0.75,
            z + 1,
            c.kind.vml()
        );
        if s.anchor != Anchor::Absolute {
            vml.push_str("<x:MoveWithCells/>");
        }
        if s.anchor == Anchor::TwoCell {
            vml.push_str("<x:SizeWithCells/>");
        }
        let _ = write!(vml, "<x:Anchor>{}, {}, {}, {}, {}, {}, {}, {}</x:Anchor>", from.0, from.2.round() as i64, from.1, from.3.round() as i64, to.0, to.2.round() as i64, to.1, to.3.round() as i64);
        let el = |v: &mut String, k: &str, t: &str| {
            if !t.is_empty() {
                let _ = write!(v, "<x:{k}>{}</x:{k}>", esc(t));
            }
        };
        el(&mut vml, "FmlaMacro", &c.macro_name);
        if c.kind.has_link() {
            el(&mut vml, "FmlaLink", &c.link);
        }
        match c.kind {
            ControlKind::CheckBox | ControlKind::Radio if c.value > 0 => {
                let _ = write!(vml, "<x:Checked>{}</x:Checked>", c.value);
            }
            ControlKind::ComboBox | ControlKind::ListBox => {
                el(&mut vml, "FmlaRange", &c.items);
                let _ = write!(vml, "<x:Sel>{}</x:Sel>", c.value);
                if c.kind == ControlKind::ComboBox {
                    let _ = write!(vml, "<x:DropLines>{}</x:DropLines>", c.lines);
                }
            }
            ControlKind::Spin | ControlKind::ScrollBar => {
                let _ = write!(vml, "<x:Val>{}</x:Val><x:Min>{}</x:Min><x:Max>{}</x:Max><x:Inc>{}</x:Inc><x:Page>{}</x:Page>", c.value, c.min, c.max, c.inc, c.page);
                if c.horizontal {
                    vml.push_str("<x:Horiz/>");
                }
            }
            _ => {}
        }
        for (name, _, raw) in children(&c.extra) {
            if !KNOWN.contains(&name) || matches!(name, "Dx" | "SelType") {
                vml.push_str(raw);
            }
        }
        vml.push_str("</x:ClientData></v:shape>");
    }
    if vml.is_empty() {
        return;
    }
    let shapes = format!("{SHAPETYPE}{vml}");
    match &mut parts.vml {
        Some(v) => {
            if let Some(i) = v.rfind("</xml>") {
                v.insert_str(i, &shapes);
            }
        }
        None => {
            parts.vml = Some(format!(r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout>{shapes}</xml>"#));
            crate::xlsx_ole::with_rels(&mut parts.rels, &format!(r#"<Relationship Id="rIdVmlC" Type="{REL_VML}" Target="../drawings/vmlDrawing{n}.vml"/>"#));
            parts.legacy_drawing = r#"<legacyDrawing r:id="rIdVmlC"/>"#.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::{CellRef, Workbook};

    fn control(wb: &mut Workbook, kind: ControlKind, f: impl Fn(&mut FormControl)) {
        let mut s = SheetShape::new(Shape::new(ShapeKind::Rect), CellRef::new(2, 3), 110.0, 24.0);
        let mut c = FormControl::new(kind);
        f(&mut c);
        s.control = Some(Box::new(c));
        wb.add_shape(0, s);
    }

    #[test]
    fn controls_roundtrip() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "one");
        wb.set_input(0, CellRef::new(1, 0), "two");
        control(&mut wb, ControlKind::CheckBox, |c| {
            c.text = "Paid & sent".into();
            c.link = "$B$1".into();
            c.value = 1;
        });
        control(&mut wb, ControlKind::ComboBox, |c| {
            c.items = "$A$1:$A$2".into();
            c.link = "$B$2".into();
            c.value = 2;
            c.extra = "<x:AutoFill>False</x:AutoFill>".into();
        });
        control(&mut wb, ControlKind::ScrollBar, |c| {
            (c.min, c.max, c.value, c.horizontal) = (5, 50, 7, true);
        });
        control(&mut wb, ControlKind::Button, |c| {
            c.text = "Run".into();
            c.macro_name = "Module1.Go".into();
        });
        wb.set_note(0, CellRef::new(5, 5), Some(apex_sheet::Note { text: "note".into(), ..Default::default() }));
        let back = crate::xlsx::read(&crate::xlsx::write(&wb)).unwrap();
        let got: Vec<_> = back.sheets[0].shapes.iter().map(|s| (s.from, s.control.as_deref().cloned().unwrap())).collect();
        assert_eq!(got.len(), 4);
        let want: Vec<_> = wb.sheets[0].shapes.iter().map(|s| (s.from, s.control.as_deref().cloned().unwrap())).collect();
        assert_eq!(got, want);
        assert!(back.sheets[0].iter().any(|(_, c)| c.extra.as_ref().is_some_and(|x| x.note.is_some())), "notes kept");
    }

    #[test]
    fn excel_client_data() {
        let vml = r##"<xml><v:shape id="_x0000_s1026" type="#_x0000_t201"><v:textbox><div style='text-align:left'><font face="Segoe UI" size="160">Check&amp;<br>Box 1</font></div></v:textbox><x:ClientData ObjectType="Checkbox"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor> 1, 15, 2, 4, 3, 10, 3, 8</x:Anchor><x:AutoFill>False</x:AutoFill><x:Checked>1</x:Checked><x:FmlaLink>$A$1</x:FmlaLink><x:NoThreeD/></x:ClientData></v:shape><v:shape id="_x0000_s1027"><x:ClientData ObjectType="Note"><x:Anchor>1, 1, 1, 1, 2, 2, 2, 2</x:Anchor></x:ClientData></v:shape></xml>"##;
        let mut sh = Sheet::new("S");
        read_vml(vml, &mut sh);
        assert_eq!(sh.shapes.len(), 1);
        let c = sh.shapes[0].control.as_ref().unwrap();
        assert_eq!((c.kind, c.value, c.link.as_str(), c.text.as_str()), (ControlKind::CheckBox, 1, "$A$1", "Check&\nBox 1"));
        assert_eq!(c.extra, "<x:AutoFill>False</x:AutoFill><x:NoThreeD/>");
        assert_eq!((sh.shapes[0].from, sh.shapes[0].anchor), (CellRef::new(2, 1), Anchor::TwoCell));
    }
}
