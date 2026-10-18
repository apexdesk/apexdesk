//! What Excel keeps inside cells beyond values: check boxes (a cell
//! format extension that points into `xl/featurePropertyBag`) and
//! pictures placed in cells (rich values in `xl/richData`, reached from
//! the `vm` attribute of a cell through `xl/metadata.xml`).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use apex_sheet::{CellPicture, CellRef, Content, ImageFormat, Workbook};

use crate::xlsx::resolve_target;
use crate::xml::{Ev, HEADER, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

pub(crate) const REL_FPB: &str = "http://schemas.microsoft.com/office/2022/11/relationships/FeaturePropertyBag";
pub(crate) const CT_FPB: &str = "application/vnd.ms-excel.featurepropertybag+xml";
/// `ext` of an `xf` holding its `xfComplement`.
pub(crate) const XF_EXT_URI: &str = "{C7286773-470A-42A8-94C5-96B5CB345126}";
const NS_FPB: &str = "http://schemas.microsoft.com/office/spreadsheetml/2022/featurepropertybag";

const REL_METADATA: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata";
const REL_RV: &str = "http://schemas.microsoft.com/office/2017/06/relationships/rdRichValue";
const REL_RV_STRUCT: &str = "http://schemas.microsoft.com/office/2017/06/relationships/rdRichValueStructure";
const REL_RV_TYPES: &str = "http://schemas.microsoft.com/office/2017/06/relationships/rdRichValueTypes";
const REL_RV_REL: &str = "http://schemas.microsoft.com/office/2022/10/relationships/richValueRel";
const REL_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
const NS_RD: &str = "http://schemas.microsoft.com/office/spreadsheetml/2017/richdata";

/// The `xf` extension that marks a check box format (`i` 0 is the only
/// complement written).
pub(crate) fn xf_checkbox_ext() -> String {
    format!(r#"<extLst><ext uri="{XF_EXT_URI}" xmlns:xfpb="{NS_FPB}"><xfpb:xfComplement i="0"/></ext></extLst>"#)
}

/// The feature property bags of a workbook with check boxes: Checkbox <-
/// XFControls (CellControl) <- XFComplement (XFControls) <- XFComplements.
pub(crate) fn feature_bags_xml() -> String {
    format!(
        r#"{HEADER}<FeaturePropertyBags xmlns="{NS_FPB}"><bag type="Checkbox"/><bag type="XFControls"><bagId k="CellControl">0</bagId></bag><bag type="XFComplement"><bagId k="XFControls">1</bagId></bag><bag type="XFComplements" extRef="XFComplementsMapperExtRef"><a k="MappedFeaturePropertyBags"><bagId>2</bagId></a></bag></FeaturePropertyBags>"#
    )
}

/// Which `xfComplement` indices of `styles.xml` stand for a check box.
pub(crate) fn read_checkbox_complements(z: &ZipReader, wb_rels: &HashMap<String, (String, String, bool)>) -> HashSet<u32> {
    let path = wb_rels.values().find(|(t, _, _)| t == REL_FPB).map_or_else(|| "xl/featurePropertyBag/featurePropertyBag.xml".to_string(), |(_, t, _)| resolve_target("xl", t));
    let Ok(xml) = z.read_string(&path) else { return HashSet::new() };
    // Each bag: its type and its bag references (key, index).
    let mut bags: Vec<(String, Vec<(String, usize)>)> = Vec::new();
    let mut key = String::new();
    let mut in_bag = false;
    let mut r = Reader::new(&xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "bag" => {
                bags.push((t.attr("type").unwrap_or_default().into_owned(), Vec::new()));
                in_bag = true;
            }
            Ev::Empty(t) if t.local() == "bag" => bags.push((t.attr("type").unwrap_or_default().into_owned(), Vec::new())),
            Ev::Start(t) if in_bag && t.local() == "a" => key = t.attr("k").unwrap_or_default().into_owned(),
            Ev::Start(t) if in_bag && t.local() == "bagId" => {
                let k = t.attr("k").map_or_else(|| key.clone(), |k| k.into_owned());
                if let (Some(b), Ok(i)) = (bags.last_mut(), r.text().trim().parse()) {
                    b.1.push((k, i));
                }
            }
            Ev::End(n) if crate::xml::local(n) == "bag" => in_bag = false,
            Ev::End(n) if crate::xml::local(n) == "a" => key.clear(),
            _ => {}
        }
    }
    let follow = |i: usize, want: &str, k: &str| -> Option<usize> {
        let (ty, refs) = bags.get(i)?;
        (ty == want).then(|| refs.iter().find(|(kk, _)| kk == k).map(|r| r.1)).flatten()
    };
    let mut out = HashSet::new();
    for (ty, refs) in &bags {
        if ty != "XFComplements" {
            continue;
        }
        for (n, (_, comp)) in refs.iter().filter(|(k, _)| k == "MappedFeaturePropertyBags").enumerate() {
            let control = follow(*comp, "XFComplement", "XFControls").and_then(|c| follow(c, "XFControls", "CellControl"));
            if control.and_then(|c| bags.get(c)).is_some_and(|(t, _)| t == "Checkbox") {
                out.insert(n as u32);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Pictures in cells

/// The picture of each value metadata record (`vm` 1 is index 0).
pub(crate) fn read_cell_pictures(z: &ZipReader, wb_rels: &HashMap<String, (String, String, bool)>) -> Vec<Option<Arc<CellPicture>>> {
    let part = |ty: &str, default: &str| wb_rels.values().find(|(t, _, _)| t == ty).map_or_else(|| default.to_string(), |(_, t, _)| resolve_target("xl", t));
    let Ok(meta) = z.read_string(&part(REL_METADATA, "xl/metadata.xml")) else { return Vec::new() };
    let rv_path = part(REL_RV, "xl/richData/rdrichvalue.xml");
    let (Ok(rv), Ok(st)) = (z.read_string(&rv_path), z.read_string(&part(REL_RV_STRUCT, "xl/richData/rdrichvaluestructure.xml"))) else { return Vec::new() };
    let rel_path = part(REL_RV_REL, "xl/richData/richValueRel.xml");
    let rel_ids = z.read_string(&rel_path).map(|x| read_rel_ids(&x)).unwrap_or_default();
    let (dir, file) = rel_path.rsplit_once('/').unwrap_or(("", &rel_path));
    let rels = z.read_string(&format!("{dir}/_rels/{file}.rels")).map(|x| crate::xlsx_notes::read_rels(&x)).unwrap_or_default();

    // Metadata: type names, the rich value of each XLRICHVALUE future
    // record, and (type, record) of each value metadata block.
    let mut types = Vec::new();
    let mut future: Vec<Option<usize>> = Vec::new();
    let mut in_rich = false;
    let mut value_meta: Vec<Option<(usize, usize)>> = Vec::new();
    let mut in_value = false;
    let mut r = Reader::new(&meta);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "metadataType" => types.push(t.attr("name").unwrap_or_default().into_owned()),
            Ev::Start(t) if t.local() == "futureMetadata" => in_rich = t.attr("name").as_deref() == Some("XLRICHVALUE"),
            Ev::End(n) if crate::xml::local(n) == "futureMetadata" => in_rich = false,
            Ev::Start(t) | Ev::Empty(t) if in_rich && t.local() == "bk" => future.push(None),
            Ev::Start(t) | Ev::Empty(t) if in_rich && t.local() == "rvb" => {
                if let Some(last) = future.last_mut() {
                    *last = t.attr("i").and_then(|v| v.trim().parse().ok());
                }
            }
            Ev::Start(t) if t.local() == "valueMetadata" => in_value = true,
            Ev::End(n) if crate::xml::local(n) == "valueMetadata" => in_value = false,
            Ev::Start(t) | Ev::Empty(t) if in_value && t.local() == "bk" => value_meta.push(None),
            Ev::Start(t) | Ev::Empty(t) if in_value && t.local() == "rc" => {
                let num = |k: &str| t.attr(k).and_then(|v| v.trim().parse::<usize>().ok());
                if let (Some(last), Some(ty), Some(v)) = (value_meta.last_mut(), num("t"), num("v")) {
                    if last.is_none() {
                        *last = Some((ty, v));
                    }
                }
            }
            _ => {}
        }
    }

    // Structures: type and key names.
    let mut structs: Vec<(String, Vec<String>)> = Vec::new();
    let mut r = Reader::new(&st);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "s" => structs.push((t.attr("t").unwrap_or_default().into_owned(), Vec::new())),
            Ev::Start(t) | Ev::Empty(t) if t.local() == "k" => {
                if let Some(s) = structs.last_mut() {
                    s.1.push(t.attr("n").unwrap_or_default().into_owned());
                }
            }
            _ => {}
        }
    }

    // Rich values: the picture of each local image.
    let mut images: HashMap<String, Option<(Arc<[u8]>, ImageFormat)>> = HashMap::new();
    let mut values: Vec<Option<Arc<CellPicture>>> = Vec::new();
    let mut r = Reader::new(&rv);
    let mut cur: Option<(usize, Vec<String>)> = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "rv" => cur = Some((t.attr("s").and_then(|v| v.trim().parse().ok()).unwrap_or(usize::MAX), Vec::new())),
            Ev::Empty(t) if t.local() == "rv" => values.push(None),
            Ev::Start(t) if t.local() == "v" => {
                let text = r.text();
                if let Some((_, vs)) = &mut cur {
                    vs.push(text);
                }
            }
            Ev::Empty(t) if t.local() == "v" => {
                if let Some((_, vs)) = &mut cur {
                    vs.push(String::new());
                }
            }
            Ev::End(n) if crate::xml::local(n) == "rv" => {
                let Some((s, vs)) = cur.take() else { continue };
                let pic = structs.get(s).filter(|(ty, _)| ty == "_localImage").and_then(|(_, keys)| {
                    let get = |k: &str| keys.iter().position(|x| x == k).and_then(|i| vs.get(i));
                    let rel: usize = get("_rvRel:LocalImageIdentifier")?.trim().parse().ok()?;
                    let target = rels.get(rel_ids.get(rel)?)?;
                    let path = resolve_target(dir, &target.1);
                    let (data, format) = images
                        .entry(path.clone())
                        .or_insert_with(|| {
                            let bytes = z.read(&path).ok()?;
                            let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
                            let f = ImageFormat::detect(&bytes, ext)?;
                            Some((Arc::from(bytes), f))
                        })
                        .clone()?;
                    let mut p = CellPicture::new(data, format);
                    p.alt = get("Text").cloned().unwrap_or_default();
                    Some(Arc::new(p))
                });
                values.push(pic);
            }
            _ => {}
        }
    }

    value_meta
        .into_iter()
        .map(|m| {
            let (ty, rec) = m?;
            (types.get(ty.checked_sub(1)?)? == "XLRICHVALUE").then_some(())?;
            values.get((*future.get(rec)?)?)?.clone()
        })
        .collect()
}

/// The `r:id` of each `rel` of `richValueRel.xml`, in order.
fn read_rel_ids(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "rel" => out.push(t.attr("r:id").or_else(|| t.attr("id")).unwrap_or_default().into_owned()),
            _ => {}
        }
    }
    out
}

/// Pictures in cells of a workbook being written: the `vm` of each cell.
#[derive(Default)]
pub(crate) struct CellPictures {
    /// (sheet, cell) -> vm (1-based).
    pub vm: HashMap<(usize, CellRef), usize>,
    /// Each rich value: its relationship index and alt text.
    values: Vec<(usize, String)>,
    /// Media file of each relationship.
    rels: Vec<String>,
}

impl CellPictures {
    pub(crate) fn collect(wb: &Workbook, media: &mut crate::xlsx_drawing::Media) -> CellPictures {
        let mut out = CellPictures::default();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        for (si, sh) in wb.sheets.iter().enumerate() {
            let mut cells: Vec<(CellRef, &CellPicture)> = sh.iter().filter_map(|(c, cell)| Some((c, cell.picture()?))).collect();
            cells.sort_by_key(|(c, _)| (c.row, c.col));
            for (c, p) in cells {
                let name = media.add_data(&p.data, p.format);
                let rel = *by_name.entry(name.clone()).or_insert_with(|| {
                    out.rels.push(name);
                    out.rels.len() - 1
                });
                out.values.push((rel, p.alt.clone()));
                out.vm.insert((si, c), out.values.len());
            }
        }
        out
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Content types, workbook relationships (ids `rIdRV*`) and parts.
    pub(crate) fn write(&self, z: &mut ZipWriter, ct: &mut String, rels: &mut String) {
        if self.is_empty() {
            return;
        }
        for (name, ty) in [
            ("rdrichvalue.xml", "application/vnd.ms-excel.rdrichvalue+xml"),
            ("rdrichvaluestructure.xml", "application/vnd.ms-excel.rdrichvaluestructure+xml"),
            ("rdRichValueTypes.xml", "application/vnd.ms-excel.rdrichvaluetypes+xml"),
            ("richValueRel.xml", "application/vnd.ms-excel.richvaluerel+xml"),
        ] {
            let _ = write!(ct, r#"<Override PartName="/xl/richData/{name}" ContentType="{ty}"/>"#);
        }
        for (i, (ty, name)) in [(REL_RV_REL, "richValueRel.xml"), (REL_RV, "rdrichvalue.xml"), (REL_RV_STRUCT, "rdrichvaluestructure.xml"), (REL_RV_TYPES, "rdRichValueTypes.xml")].into_iter().enumerate() {
            let _ = write!(rels, r#"<Relationship Id="rIdRV{}" Type="{ty}" Target="richData/{name}"/>"#, i + 1);
        }
        // Structure 0 without alt text, 1 with it.
        let s = format!(
            r#"{HEADER}<rvStructures xmlns="{NS_RD}" count="2"><s t="_localImage"><k n="_rvRel:LocalImageIdentifier" t="i"/><k n="CalcOrigin" t="i"/></s><s t="_localImage"><k n="_rvRel:LocalImageIdentifier" t="i"/><k n="CalcOrigin" t="i"/><k n="Text" t="s"/></s></rvStructures>"#
        );
        z.add("xl/richData/rdrichvaluestructure.xml", s.as_bytes(), true);
        let mut v = format!(r#"{HEADER}<rvData xmlns="{NS_RD}" count="{}">"#, self.values.len());
        for (rel, alt) in &self.values {
            // CalcOrigin 5: a value stored in the cell, not computed.
            if alt.is_empty() {
                let _ = write!(v, r#"<rv s="0"><v>{rel}</v><v>5</v></rv>"#);
            } else {
                let _ = write!(v, r#"<rv s="1"><v>{rel}</v><v>5</v><v>{}</v></rv>"#, esc(alt));
            }
        }
        v.push_str("</rvData>");
        z.add("xl/richData/rdrichvalue.xml", v.as_bytes(), true);
        let mut x = format!(r#"{HEADER}<richValueRels xmlns="http://schemas.microsoft.com/office/spreadsheetml/2022/richvaluerel" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#);
        let mut rr = format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
        for (i, name) in self.rels.iter().enumerate() {
            let _ = write!(x, r#"<rel r:id="rId{}"/>"#, i + 1);
            let _ = write!(rr, r#"<Relationship Id="rId{}" Type="{REL_IMAGE}" Target="../media/{name}"/>"#, i + 1);
        }
        x.push_str("</richValueRels>");
        rr.push_str("</Relationships>");
        z.add("xl/richData/richValueRel.xml", x.as_bytes(), true);
        z.add("xl/richData/_rels/richValueRel.xml.rels", rr.as_bytes(), true);
        z.add("xl/richData/rdRichValueTypes.xml", RV_TYPES.as_bytes(), true);
    }
}

/// `xl/metadata.xml`: dynamic arrays (`cm="1"`) and, after them, the rich
/// values of pictures (`vm="n"`).
pub(crate) fn metadata_xml(arrays: bool, pictures: usize) -> String {
    let mut types = String::new();
    let mut future = String::new();
    let mut n = 0;
    if arrays {
        n += 1;
        types.push_str(r#"<metadataType name="XLDAPR" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1" cellMeta="1"/>"#);
        future.push_str(r#"<futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata>"#);
    }
    let rich = n + 1;
    if pictures > 0 {
        n += 1;
        types.push_str(r#"<metadataType name="XLRICHVALUE" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1"/>"#);
        let _ = write!(future, r#"<futureMetadata name="XLRICHVALUE" count="{pictures}">"#);
        for i in 0..pictures {
            let _ = write!(future, r#"<bk><extLst><ext uri="{{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}}"><xlrd:rvb i="{i}"/></ext></extLst></bk>"#);
        }
        future.push_str("</futureMetadata>");
    }
    let mut s = format!(
        r#"{HEADER}<metadata xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xlrd="{NS_RD}" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray"><metadataTypes count="{n}">{types}</metadataTypes>{future}"#
    );
    if arrays {
        s.push_str(r#"<cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata>"#);
    }
    if pictures > 0 {
        let _ = write!(s, r#"<valueMetadata count="{pictures}">"#);
        for i in 0..pictures {
            let _ = write!(s, r#"<bk><rc t="{rich}" v="{i}"/></bk>"#);
        }
        s.push_str("</valueMetadata>");
    }
    s.push_str("</metadata>");
    s
}

/// Whether any cell format of the workbook is a check box.
pub(crate) fn has_checkboxes(wb: &Workbook) -> bool {
    wb.styles.iter().any(|(_, s)| s.checkbox)
}

/// Puts the pictures read from `vm` attributes into their cells.
pub(crate) fn apply_picture(sh: &mut apex_sheet::Sheet, c: CellRef, pic: Arc<CellPicture>) {
    if matches!(sh.get(c).map(|x| &x.content), Some(Content::Value(apex_sheet::Value::Error(_)))) {
        sh.edit_extra(c, |x| x.picture = Some(pic));
    }
}

const RV_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<rvTypesInfo xmlns="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata2" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="x" xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><global><keyFlags><key name="_Self"><flag name="ExcludeFromFile" value="1"/><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_DisplayString"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_Flags"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_Format"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_SubLabel"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_Attribution"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_Icon"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_Display"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_CanonicalPropertyNames"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_ClassificationId"><flag name="ExcludeFromCalcComparison" value="1"/></key></keyFlags></global></rvTypesInfo>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::value::ErrorKind;
    use apex_sheet::workbook::Op;
    use apex_sheet::{Range, Sheet, SheetImage, Value};

    const PNG_4X2: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 4, 0, 0, 0, 2, 8, 6, 0, 0, 0];

    fn part(data: &[u8], name: &str) -> String {
        ZipReader::new(data).unwrap().read_string(name).unwrap()
    }

    #[test]
    fn checkboxes_roundtrip() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(1, 0), "TRUE");
        wb.insert_checkboxes(0, &Range::new(CellRef::new(0, 0), CellRef::new(2, 0)));
        wb.modify_style(0, &Range::single(CellRef::new(2, 0)), |s| s.bold = true);
        let data = crate::xlsx::write(&wb);
        let styles = part(&data, "xl/styles.xml");
        assert!(styles.contains(r#"<xfpb:xfComplement i="0"/>"#), "{styles}");
        let bags = part(&data, "xl/featurePropertyBag/featurePropertyBag.xml");
        assert!(bags.contains(r#"<bag type="Checkbox"/>"#));
        assert!(part(&data, "[Content_Types].xml").contains(CT_FPB));
        assert!(part(&data, "xl/_rels/workbook.xml.rels").contains(REL_FPB));
        let back = crate::xlsx::read(&data).unwrap();
        for r in 0..3 {
            assert!(back.is_checkbox(0, CellRef::new(r, 0)), "row {r}");
        }
        let v = |r| back.sheets[0].get(CellRef::new(r, 0)).unwrap().value().clone();
        assert_eq!((v(0), v(1), v(2)), (Value::Bool(false), Value::Bool(true), Value::Bool(false)));
        assert!(back.styles.get(back.sheets[0].get(CellRef::new(2, 0)).unwrap().style).bold);
        assert!(!back.is_checkbox(0, CellRef::new(3, 0)));
        // Without check boxes, no bag part.
        let plain = crate::xlsx::write(&Workbook::new());
        assert!(!ZipReader::new(&plain).unwrap().contains("xl/featurePropertyBag/featurePropertyBag.xml"));
    }

    #[test]
    fn bags_follow_the_chain() {
        // The bags may come in another order.
        let xml = r#"<FeaturePropertyBags xmlns="x"><bag type="XFComplements" extRef="XFComplementsMapperExtRef"><a k="MappedFeaturePropertyBags"><bagId>3</bagId><bagId>1</bagId></a></bag><bag type="XFComplement"><bagId k="XFControls">2</bagId></bag><bag type="XFControls"><bagId k="CellControl">4</bagId></bag><bag type="XFComplement"/><bag type="Checkbox"/></FeaturePropertyBags>"#;
        let mut z = ZipWriter::new();
        z.add("xl/featurePropertyBag/featurePropertyBag.xml", xml.as_bytes(), true);
        let data = z.finish();
        let got = read_checkbox_complements(&ZipReader::new(&data).unwrap(), &HashMap::new());
        assert_eq!(got, HashSet::from([1]));
    }

    #[test]
    fn pictures_in_cells_roundtrip() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "=SEQUENCE(2)");
        let mut img = SheetImage::new(Arc::from(PNG_4X2), ImageFormat::Png, CellRef::new(1, 2), 40.0, 20.0);
        img.alt = "Logo & co".into();
        let id = wb.add_image(0, img);
        wb.place_in_cell(0, id);
        wb.set_cell_picture(0, CellRef::new(4, 3), CellPicture::new(Arc::from(PNG_4X2), ImageFormat::Png));
        wb.execute(Op::AddSheet { index: 1, sheet: Box::new(Sheet::new("Two")) });
        wb.set_cell_picture(1, CellRef::new(0, 0), CellPicture::new(Arc::from(PNG_4X2), ImageFormat::Png));
        let data = crate::xlsx::write(&wb);
        let sheet = part(&data, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains(r#"<c r="C2" t="e" vm="1"><v>#VALUE!</v></c>"#), "{sheet}");
        assert!(sheet.contains(r#"<c r="D5" t="e" vm="2">"#));
        assert!(part(&data, "xl/worksheets/sheet2.xml").contains(r#"vm="3""#));
        let meta = part(&data, "xl/metadata.xml");
        assert!(meta.contains(r#"<metadataType name="XLDAPR""#) && meta.contains(r#"<rc t="2" v="2"/>"#), "{meta}");
        assert!(part(&data, "xl/richData/rdrichvaluestructure.xml").contains("_rvRel:LocalImageIdentifier"));
        let rv = part(&data, "xl/richData/rdrichvalue.xml");
        assert!(rv.contains("<v>Logo &amp; co</v>"), "{rv}");
        // One media file for the three cells.
        let rels = part(&data, "xl/richData/_rels/richValueRel.xml.rels");
        assert_eq!(rels.matches("<Relationship ").count(), 1);
        assert!(part(&data, "[Content_Types].xml").contains("application/vnd.ms-excel.rdrichvalue+xml"));

        let back = crate::xlsx::read(&data).unwrap();
        let pic = |s: usize, r, c| back.sheets[s].get(CellRef::new(r, c)).and_then(|x| x.picture().cloned());
        let p1 = pic(0, 1, 2).expect("C2");
        assert_eq!((p1.alt.as_str(), p1.format, &*p1.data), ("Logo & co", ImageFormat::Png, PNG_4X2));
        assert_eq!(pic(0, 4, 3).map(|p| p.alt), Some(String::new()));
        assert!(pic(1, 0, 0).is_some());
        assert!(back.sheets[0].images.is_empty());
        // The array formula still spills.
        assert_eq!(back.sheets[0].get(CellRef::new(1, 0)).map(|c| c.value().clone()), Some(Value::Number(2.0)));
    }

    #[test]
    fn ods_keeps_values_and_floats_pictures() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "TRUE");
        wb.insert_checkboxes(0, &Range::new(CellRef::new(0, 0), CellRef::new(1, 0)));
        let mut p = CellPicture::new(Arc::from(PNG_4X2), ImageFormat::Png);
        p.alt = "logo".into();
        wb.set_cell_picture(0, CellRef::new(2, 1), p);
        let back = crate::ods::read(&crate::ods::write(&wb)).unwrap();
        let v = |r| back.sheets[0].get(CellRef::new(r, 0)).unwrap().value().clone();
        assert_eq!((v(0), v(1)), (Value::Bool(true), Value::Bool(false)));
        let sh = &back.sheets[0];
        assert!(sh.get(CellRef::new(2, 1)).is_none_or(|c| c.picture().is_none() && c.value().is_empty()));
        assert_eq!(sh.images.len(), 1);
        assert_eq!((sh.images[0].from, sh.images[0].alt.as_str()), (CellRef::new(2, 1), "logo"));
        // Fitted in the cell, 2:1.
        assert!(sh.images[0].height <= sh.row_height(2) + 0.5 && (sh.images[0].width / sh.images[0].height - 2.0).abs() < 0.05);
    }

    #[test]
    fn image_formula_is_kept() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "=IMAGE(\"https://example.com/a.png\";\"alt\")");
        assert_eq!(wb.sheets[0].get(CellRef::new(0, 0)).unwrap().value(), &Value::Error(ErrorKind::Connect));
        let data = crate::xlsx::write(&wb);
        let sheet = part(&data, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains(r#"<f>_xlfn.IMAGE(&quot;https://example.com/a.png&quot;,&quot;alt&quot;)</f><v>#VALUE!</v>"#), "{sheet}");
        let back = crate::xlsx::read(&data).unwrap();
        assert_eq!(back.sheets[0].get(CellRef::new(0, 0)).unwrap().value(), &Value::Error(ErrorKind::Connect));
        assert_eq!(back.edit_text(0, CellRef::new(0, 0)), "=IMAGE(\"https://example.com/a.png\";\"alt\")");
    }
}
