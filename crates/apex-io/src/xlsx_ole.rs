//! Embedded objects in xlsx: the sheet's `oleObjects` (placed by the
//! anchor newer files give, or by their VML shape), the VML shapes that
//! show them, Excel's hidden stand-ins in the drawing, and SmartArt
//! frames of the drawing (see [`crate::diagram`]).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{EmbedKind, Embedded};
use apex_sheet::{Anchor, ImageFormat, Sheet, SheetImage};

use crate::drawingml::{Node, read_fragment};
use crate::xlsx_drawing::{Corner, Media, REL_IMAGE};
use crate::xlsx_notes::SheetParts;
use crate::xml::{Ev, HEADER, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

const REL_VML: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing";
const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const NS_X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
const NS_A14: &str = "http://schemas.microsoft.com/office/drawing/2010/main";
/// First VML shape id of a sheet (block 1 of the `o:idmap`).
const FIRST_SHAPE: usize = 1025;

// --- reading ----------------------------------------------------------------------

/// A VML shape: its anchor (cell corners in pixels), whether it sizes
/// with the cells, and the relationship of its picture.
#[derive(Default)]
struct VmlShape {
    anchor: Option<[Corner; 2]>,
    size_with_cells: bool,
    picture: Option<String>,
}

fn vml_shapes(vml: &str) -> HashMap<String, VmlShape> {
    let mut out = HashMap::new();
    let mut r = Reader::new(vml);
    let mut cur: Option<(String, VmlShape)> = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "shape" => {
                if let Some((id, s)) = cur.take() {
                    out.insert(id, s);
                }
                cur = Some((t.attr("id").unwrap_or_default().into_owned(), VmlShape::default()));
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "imagedata" => {
                if let Some((_, s)) = &mut cur {
                    s.picture = t.attr("o:relid").or_else(|| t.attr("r:id")).map(|v| v.into_owned());
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "SizeWithCells" => {
                if let Some((_, s)) = &mut cur {
                    s.size_with_cells = true;
                }
            }
            Ev::Start(t) if t.local() == "Anchor" => {
                let v: Vec<f32> = r.text().split(',').filter_map(|x| x.trim().parse::<f32>().ok()).collect();
                if let (Some((_, s)), &[c0, o0, r0, o1, c1, o2, r1, o3]) = (&mut cur, &v[..]) {
                    let n = |x: f32| x.max(0.0) as u32;
                    s.anchor = Some([(n(c0), n(r0), o0, o1), (n(c1), n(r1), o2, o3)]);
                }
            }
            _ => {}
        }
    }
    if let Some((id, s)) = cur {
        out.insert(id, s);
    }
    out
}

/// A picture part, its bytes and format.
fn picture(z: &ZipReader, dir: &str, rels: &HashMap<String, (String, String, bool)>, rid: &str) -> Option<(Arc<[u8]>, ImageFormat)> {
    let (_, target, false) = rels.get(rid)? else { return None };
    let p = crate::xlsx::resolve_target(dir, target);
    let data = z.read(&p).ok()?;
    let fmt = ImageFormat::detect(&data, p.rsplit_once('.').map_or("", |x| x.1))?;
    Some((Arc::from(data), fmt))
}

pub(crate) fn rels_of(z: &ZipReader, path: &str) -> HashMap<String, (String, String, bool)> {
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    z.read_string(&format!("{dir}/_rels/{file}.rels")).map(|s| crate::xlsx_notes::read_rels(&s)).unwrap_or_default()
}

fn corner(n: &Node) -> Corner {
    let num = |k: &str| n.child(k).map_or("0", |c| c.text.trim()).to_string();
    let cell = |k: &str| num(k).parse::<u32>().unwrap_or(0);
    (cell("col"), cell("row"), crate::xlsx_drawing::emu(&num("colOff")), crate::xlsx_drawing::emu(&num("rowOff")))
}

/// The embedded objects of the worksheet part at `path` (with its xml),
/// added to the sheet's pictures.
pub fn read(z: &ZipReader, path: &str, xml: &str, sh: &mut Sheet) {
    let tail = xml.find("</sheetData>").map_or(xml, |i| &xml[i..]);
    let Some(start) = tail.find("<oleObjects") else { return };
    let end = tail[start..].find("</oleObjects>").map_or(tail.len(), |e| start + e + "</oleObjects>".len());
    let root = read_fragment(&tail[start..end]);
    let mut list = Vec::new();
    root.find_all("oleObject", &mut list);
    if list.is_empty() {
        return;
    }
    let objs = crate::docx_ole::Objects::load(z, path);
    let rels = rels_of(z, path);
    let dir = path.rsplit_once('/').map_or("", |x| x.0);
    // The VML shapes (older files place objects only there).
    let vml_path = rels.values().find(|(ty, _, ext)| ty == REL_VML && !ext).map(|(_, t, _)| crate::xlsx::resolve_target(dir, t));
    let vml = vml_path.as_ref().and_then(|p| z.read_string(p).ok()).map(|v| vml_shapes(&v)).unwrap_or_default();
    let vml_rels = vml_path.as_ref().map(|p| rels_of(z, p)).unwrap_or_default();
    let vml_dir = vml_path.as_ref().and_then(|p| p.rsplit_once('/')).map_or("", |x| x.0).to_string();
    let mut seen = Vec::new();
    for o in list {
        let shape_id = o.attr("shapeId").unwrap_or_default().to_string();
        // The fallback repeats the choice.
        if seen.contains(&shape_id) {
            continue;
        }
        seen.push(shape_id.clone());
        let icon = o.attr("dvAspect") == Some("DVASPECT_ICON");
        let Some(ole) = o.attr("r:id").and_then(|rid| objs.ole(rid, o.attr("progId").unwrap_or_default(), icon)) else { continue };
        let v = vml.get(&format!("_x0000_s{shape_id}"));
        let placed = match (o.find("anchor"), v.and_then(|v| v.anchor)) {
            (Some(a), _) => match (a.child("from"), a.child("to")) {
                (Some(f), Some(t)) => Some((corner(f), corner(t), a.attr("sizeWithCells") != Some("1") && a.attr("sizeWithCells") != Some("true"))),
                _ => None,
            },
            (None, Some([f, t])) => Some((f, t, !v.is_some_and(|v| v.size_with_cells))),
            _ => None,
        };
        let Some((from, to, one_cell)) = placed else { continue };
        let Some(mut img) = crate::xlsx_drawing::place_box(sh, from, to, one_cell) else { continue };
        let preview = o.find("objectPr").and_then(|p| p.attr("r:id")).and_then(|rid| picture(z, dir, &rels, rid));
        let preview = preview.or_else(|| v.and_then(|v| v.picture.as_deref()).and_then(|rid| picture(z, &vml_dir, &vml_rels, rid)));
        if let Some((data, format)) = preview {
            (img.data, img.format) = (data, format);
        }
        img.name = format!("Object {}", sh.images.len() + 1);
        img.object = Some(Arc::new(Embedded::new(EmbedKind::Ole(ole))));
        img.id = sh.images.len() as u32 + 1;
        sh.images.push(img);
    }
}

// --- writing ----------------------------------------------------------------------

/// Embedded files and diagram parts of the workbook being written.
#[derive(Default)]
pub struct Parts {
    /// Data, extension, media type, relationship type.
    files: Vec<(Arc<[u8]>, String, String, &'static str)>,
    diagrams: Vec<crate::diagram::Written>,
    /// Relationships of VML drawings: path and xml.
    vml_rels: Vec<(String, String)>,
}

impl Parts {
    fn name(&self, k: usize) -> String {
        crate::docx_ole::embedding_name(k, &self.files[k - 1].1)
    }

    pub(crate) fn content_types(&self) -> String {
        let mut s = String::new();
        for (k, f) in self.files.iter().enumerate() {
            let _ = write!(s, r#"<Override PartName="/xl/embeddings/{}" ContentType="{}"/>"#, self.name(k + 1), esc(&f.2));
        }
        for d in &self.diagrams {
            s.push_str(&d.types);
        }
        s
    }

    pub(crate) fn write_files(&self, z: &mut ZipWriter) {
        for (k, f) in self.files.iter().enumerate() {
            z.add(&format!("xl/embeddings/{}", self.name(k + 1)), &f.0, !f.1.starts_with("xls") && !f.1.starts_with("doc") && !f.1.starts_with("ppt"));
        }
        for d in &self.diagrams {
            for (path, data) in &d.files {
                z.add(path, data, true);
            }
        }
        for (path, xml) in &self.vml_rels {
            z.add(path, xml.as_bytes(), true);
        }
    }
}

/// VML shape id of the sheet's picture `i` (an embedded object): after
/// the notes' shapes.
fn shape_id(sh: &Sheet, i: usize) -> usize {
    let notes = sh.iter().filter(|(_, c)| c.extra.as_ref().is_some_and(|x| x.note.is_some())).count();
    FIRST_SHAPE + notes + sh.images[..i].iter().filter(|m| m.object.as_ref().is_some_and(|o| o.ole().is_some())).count()
}

pub(crate) fn with_rels(rels: &mut Option<String>, add: &str) {
    match rels {
        Some(r) => {
            if let Some(i) = r.rfind("</Relationships>") {
                r.insert_str(i, add);
            }
        }
        None => *rels = Some(format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{add}</Relationships>"#)),
    }
}

/// The picture shown for an object: its preview, or a blank one.
fn preview(img: &SheetImage) -> (Arc<[u8]>, ImageFormat) {
    if img.data.is_empty() || ImageFormat::sniff(&img.data).is_none() {
        (crate::docx_ole::blank_png(), ImageFormat::Png)
    } else {
        (img.data.clone(), img.format)
    }
}

/// Adds the embedded objects of sheet `n` (1-based): their `oleObjects`,
/// VML shapes, relationships and files.
pub fn sheet(sh: &Sheet, n: usize, parts: &mut SheetParts, media: &mut Media) {
    let mut ole = String::new();
    let mut vml = String::new();
    let mut vml_rels = String::new();
    let mut rels = String::new();
    let mut k = 0;
    for (i, img) in sh.images.iter().enumerate() {
        let Some(o) = img.object.as_ref().and_then(|o| o.ole()) else { continue };
        k += 1;
        let converted = crate::embed::for_family(o, false);
        let o = converted.as_ref().unwrap_or(o);
        let part = crate::embed::ooxml_part(o);
        let rel_type = part.3;
        media.objects.files.push(part);
        let file = media.objects.name(media.objects.files.len());
        let (data, format) = preview(img);
        let pic = media.add_data(&data, format);
        let id = shape_id(sh, i);
        let _ = write!(rels, r#"<Relationship Id="rIdOle{k}" Type="{rel_type}" Target="../embeddings/{file}"/><Relationship Id="rIdOlp{k}" Type="{REL_IMAGE}" Target="../media/{pic}"/>"#);
        let _ = write!(vml_rels, r#"<Relationship Id="rId{k}" Type="{REL_IMAGE}" Target="../media/{pic}"/>"#);
        let [from, to] = crate::xlsx_drawing::box_corners(sh, img.from, img.dx, img.dy, img.width, img.height);
        let sized = img.anchor != Anchor::OneCell;
        let _ = write!(
            vml,
            r##"<v:shape id="_x0000_s{id}" type="#_x0000_t75" style="position:absolute;margin-left:0;margin-top:0;width:{:.2}pt;height:{:.2}pt;z-index:{k}" filled="f" stroked="f" o:insetmode="auto"><v:imagedata o:relid="rId{k}" o:title=""/><x:ClientData ObjectType="Pict">{}<x:Anchor>{}, {}, {}, {}, {}, {}, {}, {}</x:Anchor><x:CF>Pict</x:CF><x:AutoPict/></x:ClientData></v:shape>"##,
            img.width * 0.75,
            img.height * 0.75,
            if sized { "<x:SizeWithCells/>" } else { "" },
            from.0,
            from.2.round() as i64,
            from.1,
            from.3.round() as i64,
            to.0,
            to.2.round() as i64,
            to.1,
            to.3.round() as i64
        );
        let prog = if o.prog_id.is_empty() { "Package" } else { o.prog_id.as_str() };
        let aspect = if o.icon { r#" dvAspect="DVASPECT_ICON""# } else { "" };
        let head = format!(r#"<oleObject progId="{}"{aspect} shapeId="{id}" r:id="rIdOle{k}""#, esc(prog));
        let _ = write!(
            ole,
            r#"<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:x14="{NS_X14}" Requires="x14">{head}><objectPr defaultSize="0" autoPict="0" r:id="rIdOlp{k}"><anchor xmlns:xdr="{NS_XDR}" moveWithCells="1"{}><from>{}</from><to>{}</to></anchor></objectPr></oleObject></mc:Choice><mc:Fallback>{head}/></mc:Fallback></mc:AlternateContent>"#,
            if sized { r#" sizeWithCells="1""# } else { "" },
            crate::xlsx_drawing::corner_xml(from),
            crate::xlsx_drawing::corner_xml(to)
        );
    }
    if k == 0 {
        return;
    }
    parts.ole_objects = format!("<oleObjects>{ole}</oleObjects>");
    let shapes = format!("{}{vml}", crate::docx_ole::SHAPETYPE);
    match &mut parts.vml {
        Some(v) => {
            if let Some(i) = v.rfind("</xml>") {
                v.insert_str(i, &shapes);
            }
        }
        None => {
            parts.vml = Some(format!(r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout>{shapes}</xml>"#));
            let _ = write!(rels, r#"<Relationship Id="rIdVml" Type="{REL_VML}" Target="../drawings/vmlDrawing{n}.vml"/>"#);
            parts.legacy_drawing = r#"<legacyDrawing r:id="rIdVml"/>"#.into();
        }
    }
    with_rels(&mut parts.rels, &rels);
    media.objects.vml_rels.push((format!("xl/drawings/_rels/vmlDrawing{n}.vml.rels"), format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{vml_rels}</Relationships>"#)));
}

/// Writes picture `i` of the sheet, an embedded object, in the drawing:
/// a hidden stand-in for Excel (the object itself is in `oleObjects`) or
/// a SmartArt frame. `anchor` holds the anchor's opening and closing tags.
pub(crate) fn drawing_object(x: &mut String, rels: &mut String, anchor: (&str, &str), sh: &Sheet, i: usize, media: &mut Media) {
    let img = &sh.images[i];
    let Some(o) = img.object.as_deref() else { return };
    let (open, close) = anchor;
    match &o.kind {
        EmbedKind::Ole(_) => {
            let id = shape_id(sh, i);
            let _ = write!(
                x,
                r#"<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:a14="{NS_A14}" Requires="a14">{open}<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="{id}" name="{}" hidden="1"><a:extLst><a:ext uri="{{63B3BB69-23CF-44E3-9099-C40C66FF867C}}"><a14:compatExt spid="_x0000_s{id}"/></a:ext></a:extLst></xdr:cNvPr><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr></xdr:sp><xdr:clientData/>{close}</mc:Choice><mc:Fallback/></mc:AlternateContent>"#,
                esc(&img.name)
            );
        }
        EmbedKind::Diagram(d) => {
            let w = crate::diagram::write(d, media.objects.diagrams.len() + 1, "xl", "../");
            rels.push_str(&w.rels);
            let _ = write!(
                x,
                r#"{open}<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{}" name="{}"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm>{}</xdr:graphicFrame><xdr:clientData/>{close}"#,
                i + 2,
                esc(&img.name),
                crate::diagram::graphic(&w.rel_ids)
            );
            media.objects.diagrams.push(w);
        }
        // Charts of documents: sheets hold charts of their own.
        EmbedKind::Chart(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::{CellRef, Workbook};

    #[test]
    fn objects_and_diagrams_round_trip() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "Note here");
        wb.set_note(0, CellRef::new(0, 0), Some(apex_sheet::Note { text: "hi".into(), author: "A".into(), ..Default::default() }));
        let inner = crate::xlsx::write(&Workbook::new());
        let obj = crate::embed::from_file("Book.xlsx", inner.clone());
        let pdf = crate::embed::from_file("a.pdf", b"%PDF-1.4 x".to_vec());
        let sh = &mut wb.sheets[0];
        let mut a = SheetImage::new(crate::docx_ole::blank_png(), ImageFormat::Png, CellRef::new(2, 1), 200.0, 80.0);
        a.object = Some(Arc::new(Embedded::new(EmbedKind::Ole(obj))));
        let mut b = SheetImage::new(Arc::from(&[][..]), ImageFormat::Png, CellRef::new(8, 1), 40.0, 40.0);
        b.object = Some(Arc::new(Embedded::new(EmbedKind::Ole(apex_doc::OleObject { icon: true, ..pdf }))));
        b.anchor = Anchor::OneCell;
        let mut c = SheetImage::new(Arc::from(&[][..]), ImageFormat::Png, CellRef::new(12, 1), 300.0, 150.0);
        c.object = Some(Arc::new(Embedded::new(EmbedKind::Diagram(crate::diagram::tests::sample()))));
        c.name = "Diagram 1".into();
        sh.images = vec![a, b, c];
        let bytes = crate::xlsx::write(&wb);
        let back = crate::xlsx::read(&bytes).unwrap();
        let sh2 = &back.sheets[0];
        assert!(sh2.shapes.is_empty(), "stand-ins are not shapes");
        assert_eq!(sh2.images.len(), 3);
        let objs: Vec<_> = sh2.images.iter().filter_map(|i| i.object.as_ref()?.ole().map(|o| (i, o))).collect();
        assert_eq!(objs.len(), 2);
        let (ia, oa) = objs.iter().find(|(_, o)| o.prog_id == "Excel.Sheet.12").unwrap();
        assert_eq!(&*oa.data, &inner[..]);
        assert_eq!(ia.from, CellRef::new(2, 1));
        assert!((ia.width - 200.0).abs() < 2.0 && (ia.height - 80.0).abs() < 2.0, "{} {}", ia.width, ia.height);
        let (ib, ob) = objs.iter().find(|(_, o)| o.prog_id == "Package").unwrap();
        assert!(ob.icon);
        assert_eq!(ib.anchor, Anchor::OneCell);
        assert_eq!(crate::embed::contents(ob).1, b"%PDF-1.4 x");
        let d = sh2.images.iter().find_map(|i| i.object.as_ref()?.diagram()).unwrap();
        assert_eq!(d.shapes.len(), 2);
        assert_eq!(d.texts, ["Plan", "Do"]);
        // The note survives next to the objects' shapes.
        assert!(back.sheets[0].get(CellRef::new(0, 0)).and_then(|c| c.extra.as_ref()).is_some_and(|x| x.note.is_some()));
        let z = ZipReader::new(&bytes).unwrap();
        let vml = z.read_string("xl/drawings/vmlDrawing1.vml").unwrap();
        assert!(vml.contains("_x0000_s1026") && vml.contains("_x0000_s1027") && vml.contains("ObjectType=\"Note\""));
        assert!(z.read_string("xl/drawings/_rels/vmlDrawing1.vml.rels").is_ok());
    }
}
