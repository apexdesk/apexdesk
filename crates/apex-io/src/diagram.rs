//! SmartArt diagrams in docx and xlsx: the parts are kept whole and the
//! drawing Office stored (`dsp:drawing`, the `diagramDrawing` relation)
//! is read into shapes to show them.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Diagram, DiagramPart, DiagramRole, DiagramShape, Shape};

use crate::drawingml::{EMU_PT, Node, read_fragment, read_look, read_tx_body};
use crate::xml::esc;
use crate::zip::ZipReader;

pub(crate) const URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
/// Shapes read from a drawing at most (hostile files).
const MAX_SHAPES: usize = 2000;

const REL_DATA: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramData";
const REL_LAYOUT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramLayout";
const REL_STYLE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramQuickStyle";
const REL_COLORS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramColors";
const REL_DRAWING: &str = "http://schemas.microsoft.com/office/2007/relationships/diagramDrawing";

fn rel_type(role: DiagramRole) -> &'static str {
    match role {
        DiagramRole::Data => REL_DATA,
        DiagramRole::Layout => REL_LAYOUT,
        DiagramRole::Style => REL_STYLE,
        DiagramRole::Colors => REL_COLORS,
        DiagramRole::Drawing => REL_DRAWING,
    }
}

fn role_of(rel: &str) -> Option<DiagramRole> {
    DiagramRole::ALL.into_iter().find(|r| rel == rel_type(*r) || rel.rsplit('/').next() == rel_type(*r).rsplit('/').next())
}

/// Part name and media type.
fn part_info(role: DiagramRole) -> (&'static str, &'static str) {
    match role {
        DiagramRole::Data => ("data", "application/vnd.openxmlformats-officedocument.drawingml.diagramData+xml"),
        DiagramRole::Layout => ("layout", "application/vnd.openxmlformats-officedocument.drawingml.diagramLayout+xml"),
        DiagramRole::Style => ("quickStyle", "application/vnd.openxmlformats-officedocument.drawingml.diagramStyle+xml"),
        DiagramRole::Colors => ("colors", "application/vnd.openxmlformats-officedocument.drawingml.diagramColors+xml"),
        DiagramRole::Drawing => ("drawing", "application/vnd.ms-office.drawingml.diagramDrawing+xml"),
    }
}

/// What a part's diagrams may copy of their parts (bytes).
const DIAGRAM_COPIES: usize = 64 << 20;

/// The diagram parts a part relates to, by relationship id; the diagrams
/// read and what their copies may still take.
pub(crate) struct Parts(HashMap<String, DiagramPart>, std::cell::RefCell<HashMap<String, Diagram>>, std::cell::Cell<usize>);

impl Default for Parts {
    fn default() -> Parts {
        Parts(HashMap::new(), Default::default(), std::cell::Cell::new(DIAGRAM_COPIES))
    }
}

impl Parts {
    /// Reads the diagram parts of the part at `path` (from its .rels).
    pub(crate) fn load(z: &ZipReader, path: &str) -> Parts {
        let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
        let rels = z.read_string(&format!("{dir}/_rels/{file}.rels")).map(|s| crate::xlsx_notes::read_rels(&s)).unwrap_or_default();
        let mut out = HashMap::new();
        for (id, (ty, target, external)) in &rels {
            let Some(role) = role_of(ty).filter(|_| !external) else { continue };
            let p = crate::xlsx::resolve_target(dir, target);
            let Ok(xml) = z.read(&p) else { continue };
            let (pdir, pfile) = p.rsplit_once('/').unwrap_or(("", &p));
            let mut prels = Vec::new();
            let own = z.read_string(&format!("{pdir}/_rels/{pfile}.rels")).map(|s| crate::xlsx_notes::read_rels(&s)).unwrap_or_default();
            for (rid, (rty, rtarget, rext)) in own {
                if rext {
                    continue;
                }
                let rp = crate::xlsx::resolve_target(pdir, &rtarget);
                if let Ok(bytes) = z.read(&rp) {
                    let name = rp.rsplit('/').next().unwrap_or("file").to_string();
                    prels.push((rid, rty, name, Arc::from(bytes)));
                }
            }
            prels.sort_by(|a, b| a.0.cmp(&b.0));
            out.insert(id.clone(), DiagramPart { role, xml: Arc::from(xml), rels: prels });
        }
        Parts(out, Default::default(), std::cell::Cell::new(DIAGRAM_COPIES))
    }

    /// The diagram of a `dgm:relIds` element (anywhere in `n`), shown in a
    /// `size` box (points). `None` without its data part.
    pub(crate) fn diagram(&self, n: &Node, size: (f32, f32)) -> Option<Diagram> {
        let ids = if n.local() == "relIds" { n } else { n.find("relIds")? };
        // Each diagram read once, however many places show it.
        // Ids of no part are no part.
        let key = ["r:dm", "r:lo", "r:qs", "r:cs"].map(|k| ids.attr(k).filter(|id| self.0.contains_key::<str>(id)).unwrap_or_default()).join("\u{0}");
        // Each copy is about its parts' size: a document copies at most
        // DIAGRAM_COPIES bytes of them.
        let charge = |d: &Diagram| self.2.get().checked_sub(d.parts.iter().map(|p| p.xml.len()).sum::<usize>()).map(|l| self.2.set(l));
        if let Some(d) = self.1.borrow().get(&key) {
            charge(d)?;
            return Some(Diagram { size, ..d.clone() });
        }
        let d = self.read(ids, size)?;
        charge(&d)?;
        self.1.borrow_mut().insert(key, d.clone());
        Some(d)
    }

    fn read(&self, ids: &Node, size: (f32, f32)) -> Option<Diagram> {
        let data = self.0.get(ids.attr("r:dm")?)?.clone();
        let mut d = Diagram { size, ..Default::default() };
        let data_xml = String::from_utf8_lossy(&data.xml).into_owned();
        d.parts.push(data);
        for key in ["r:lo", "r:qs", "r:cs"] {
            if let Some(p) = ids.attr(key).and_then(|id| self.0.get(id)) {
                d.parts.push(p.clone());
            }
        }
        let tree = read_fragment(&data_xml);
        let drawing = tree.find("dataModelExt").and_then(|e| e.attr("relId")).and_then(|id| self.0.get(id)).filter(|p| p.role == DiagramRole::Drawing);
        if let Some(p) = drawing {
            d.shapes = read_drawing(&String::from_utf8_lossy(&p.xml));
            d.parts.push(p.clone());
        }
        d.texts = node_texts(&tree);
        Some(d)
    }
}

/// The text of the diagram's nodes (points that aren't connections or
/// presentation).
pub(crate) fn node_texts(data: &Node) -> Vec<String> {
    let mut pts = Vec::new();
    data.find_all("pt", &mut pts);
    pts.iter()
        .filter(|p| matches!(p.attr("type"), None | Some("node")))
        .filter_map(|p| {
            let t = p.child("t")?;
            let mut runs = Vec::new();
            t.find_all("t", &mut runs);
            let s: String = runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>().join(" ").chars().take(4096).collect();
            (!s.trim().is_empty()).then_some(s)
        })
        .take(500)
        .collect()
}

fn emu_pt(n: &Node, key: &str) -> f32 {
    n.num(key).map_or(0.0, |v| (v as f32 / EMU_PT).clamp(-1.0e5, 1.0e5))
}

/// The shapes of a `dsp:drawing`, in points from the diagram's corner.
pub(crate) fn read_drawing(xml: &str) -> Vec<DiagramShape> {
    let root = read_fragment(xml);
    let mut out = Vec::new();
    if let Some(tree) = root.find("spTree") {
        shapes_of(tree, (0.0, 0.0), &mut out);
    }
    out
}

fn shapes_of(group: &Node, off: (f32, f32), out: &mut Vec<DiagramShape>) {
    for k in &group.kids {
        if out.len() >= MAX_SHAPES {
            return;
        }
        match k.local() {
            "sp" => {
                let sp_pr = k.child("spPr");
                let Some(x) = sp_pr.and_then(|s| s.child("xfrm")) else { continue };
                let (o, e) = (x.child("off"), x.child("ext"));
                let (px, py) = o.map_or((0.0, 0.0), |o| (emu_pt(o, "x"), emu_pt(o, "y")));
                let (w, h) = e.map_or((0.0, 0.0), |e| (emu_pt(e, "cx").max(0.0), emu_pt(e, "cy").max(0.0)));
                let style = k.child("style");
                let mut shape: Shape = read_look(sp_pr, style, false);
                if let Some(body) = k.child("txBody") {
                    read_tx_body(body, style, &mut shape);
                }
                shape.sanitize();
                out.push(DiagramShape { x: px + off.0, y: py + off.1, w, h, shape });
            }
            "grpSp" => {
                // Groups in diagram drawings don't scale their children.
                let o = k.child("grpSpPr").and_then(|g| g.child("xfrm")).and_then(|x| x.child("off"));
                let ch = k.child("grpSpPr").and_then(|g| g.child("xfrm")).and_then(|x| x.child("chOff"));
                let dx = o.map_or(0.0, |o| emu_pt(o, "x")) - ch.map_or(0.0, |c| emu_pt(c, "x"));
                let dy = o.map_or(0.0, |o| emu_pt(o, "y")) - ch.map_or(0.0, |c| emu_pt(c, "y"));
                shapes_of(k, (off.0 + dx, off.1 + dy), out);
            }
            _ => {}
        }
    }
}

/// A diagram written to a package.
pub(crate) struct Written {
    /// The `dgm:relIds` element of the graphic frame.
    pub rel_ids: String,
    /// Relationships of the part holding it.
    pub rels: String,
    /// Files: path in the package and bytes.
    pub files: Vec<(String, Arc<[u8]>)>,
    /// `Override` content types.
    pub types: String,
}

/// Writes diagram `n` (1-based, unique in the package) of a part in
/// folder `dir` ("word", "xl/drawings"); its parts go to `root/diagrams`
/// ("word", "xl") and are related as `rel_dir` + "diagrams/...".
pub(crate) fn write(d: &Diagram, n: usize, root: &str, rel_dir: &str) -> Written {
    let mut w = Written { rel_ids: String::new(), rels: String::new(), files: Vec::new(), types: String::new() };
    let id = |role: DiagramRole| format!("rIdDg{n}{}", part_info(role).0);
    for p in &d.parts {
        let (name, ct) = part_info(p.role);
        let file = format!("{name}{n}.xml");
        let path = format!("{root}/diagrams/{file}");
        let _ = write!(w.rels, r#"<Relationship Id="{}" Type="{}" Target="{rel_dir}diagrams/{file}"/>"#, id(p.role), rel_type(p.role));
        let _ = write!(w.types, r#"<Override PartName="/{path}" ContentType="{ct}"/>"#);
        let xml: Arc<[u8]> = if p.role == DiagramRole::Data && d.parts.iter().any(|q| q.role == DiagramRole::Drawing) {
            Arc::from(point_drawing(&String::from_utf8_lossy(&p.xml), &id(DiagramRole::Drawing)).into_bytes())
        } else {
            p.xml.clone()
        };
        w.files.push((path, xml));
        if !p.rels.is_empty() {
            let mut rels = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
            for (k, (rid, ty, fname, bytes)) in p.rels.iter().enumerate() {
                let target = format!("dgm{n}_{name}_{k}_{fname}");
                let _ = write!(rels, r#"<Relationship Id="{}" Type="{}" Target="../media/{}"/>"#, esc(rid), esc(ty), esc(&target));
                let mpath = format!("{root}/media/{target}");
                let _ = write!(w.types, r#"<Override PartName="/{}" ContentType="{}"/>"#, esc(&mpath), crate::docx_img::image_type(bytes).1);
                w.files.push((mpath, bytes.clone()));
            }
            rels.push_str("</Relationships>");
            w.files.push((format!("{root}/diagrams/_rels/{file}.rels"), Arc::from(rels.into_bytes())));
        }
    }
    let has = |r: DiagramRole| d.parts.iter().any(|p| p.role == r);
    let _ = write!(w.rel_ids, r#"<dgm:relIds xmlns:dgm="{URI}" xmlns:r="{NS_R}""#);
    for (key, role) in [("dm", DiagramRole::Data), ("lo", DiagramRole::Layout), ("qs", DiagramRole::Style), ("cs", DiagramRole::Colors)] {
        if has(role) {
            let _ = write!(w.rel_ids, r#" r:{key}="{}""#, id(role));
        }
    }
    w.rel_ids.push_str("/>");
    w
}

/// The data part pointing at the drawing's new relationship id.
fn point_drawing(xml: &str, rid: &str) -> String {
    let Some(i) = xml.find("dataModelExt") else { return xml.to_string() };
    let Some(j) = xml[i..].find("relId=\"").map(|j| i + j + 7) else { return xml.to_string() };
    let Some(end) = xml[j..].find('"').map(|e| j + e) else { return xml.to_string() };
    format!("{}{}{}", &xml[..j], rid, &xml[end..])
}

/// A `a:graphic` holding a diagram.
pub(crate) fn graphic(rel_ids: &str) -> String {
    format!(r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="{URI}">{rel_ids}</a:graphicData></a:graphic>"#)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const DATA: &str = r#"<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dgm:ptLst><dgm:pt modelId="1" type="doc"/><dgm:pt modelId="2"><dgm:t><a:p><a:r><a:t>Plan</a:t></a:r></a:p></dgm:t></dgm:pt><dgm:pt modelId="3" type="parTrans"/><dgm:pt modelId="4"><dgm:t><a:p><a:r><a:t>Do</a:t></a:r></a:p></dgm:t></dgm:pt></dgm:ptLst><dgm:extLst><a:ext uri="x"><dsp:dataModelExt xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" relId="rId9"/></a:ext></dgm:extLst></dgm:dataModel>"#;
    pub(crate) const DRAWING: &str = r#"<dsp:drawing xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dsp:spTree><dsp:nvGrpSpPr/><dsp:grpSpPr/><dsp:sp><dsp:spPr><a:xfrm><a:off x="127000" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm><a:prstGeom prst="roundRect"/><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></dsp:spPr><dsp:style><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></dsp:style><dsp:txBody><a:bodyPr anchor="ctr"/><a:p><a:pPr algn="ctr"/><a:r><a:rPr sz="2300"/><a:t>Plan</a:t></a:r></a:p></dsp:txBody></dsp:sp><dsp:sp><dsp:spPr><a:xfrm><a:off x="1524000" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm><a:prstGeom prst="chevron"/></dsp:spPr></dsp:sp></dsp:spTree></dsp:drawing>"#;

    pub(crate) fn sample() -> Diagram {
        let part = |role, xml: &str| DiagramPart { role, xml: Arc::from(xml.as_bytes()), rels: Vec::new() };
        let mut d = Diagram { size: (240.0, 60.0), shapes: read_drawing(DRAWING), ..Default::default() };
        d.parts = vec![part(DiagramRole::Data, DATA), part(DiagramRole::Layout, "<dgm:layoutDef/>"), part(DiagramRole::Style, "<dgm:styleDef/>"), part(DiagramRole::Colors, "<dgm:colorsDef/>"), part(DiagramRole::Drawing, DRAWING)];
        d.texts = node_texts(&read_fragment(DATA));
        d
    }

    #[test]
    fn drawing_shapes_and_texts() {
        let d = sample();
        assert_eq!(d.texts, ["Plan", "Do"]);
        assert_eq!(d.shapes.len(), 2);
        let s = &d.shapes[0];
        assert_eq!((s.x, s.y, s.w, s.h), (10.0, 0.0, 100.0, 50.0));
        assert_eq!(s.shape.kind, apex_doc::ShapeKind::RoundRect);
        assert_eq!(s.shape.plain_text(), "Plan");
        assert_eq!(s.shape.text[0].runs[0].size, Some(23.0));
        assert_eq!(d.shapes[1].shape.preset.as_deref(), Some("chevron"));
        let w = write(&d, 3, "word", "");
        assert!(w.rel_ids.contains(r#"r:dm="rIdDg3data""#) && w.rel_ids.contains(r#"r:cs="rIdDg3colors""#));
        let data = &w.files.iter().find(|f| f.0 == "word/diagrams/data3.xml").unwrap().1;
        assert!(String::from_utf8_lossy(data).contains(r#"relId="rIdDg3drawing""#));
        assert_eq!(w.files.len(), 5);
        assert!(w.rels.contains(r#"Target="diagrams/drawing3.xml""#));
    }

    #[test]
    fn hostile_drawings() {
        assert!(read_drawing("<dsp:drawing><dsp:spTree><dsp:sp/><dsp:sp><dsp:spPr><a:xfrm><a:off x=\"NaN\"/><a:ext cx=\"-5\" cy=\"1e30\"/></a:xfrm></dsp:spPr></dsp:sp>").iter().all(|s| s.w >= 0.0 && s.h.is_finite()));
        assert_eq!(point_drawing("<x relId=\"a", "b"), "<x relId=\"a");
    }
}
