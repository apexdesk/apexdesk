//! Document properties (the information dictionary and its XMP copy),
//! how the file opens (page layout, panel, window options), and turning
//! a file into PDF/A-2b for archiving.

use std::sync::Arc;

use super::{PdfDoc, obj_text, pdf_date_now, text_obj};
use crate::Error;
use crate::pdf::obj::{Dict, Obj};

/// The information dictionary's entries (empty when missing).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub creator: String,
    pub producer: String,
    /// PDF dates (`D:YYYYMMDD…`).
    pub created: String,
    pub modified: String,
}

/// How the file opens in a viewer. Layout and mode are the catalog's
/// names (`SinglePage`, `TwoColumnLeft`…, `UseNone`, `UseOutlines`…);
/// empty means the viewer decides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub layout: String,
    pub mode: String,
    pub fit_window: bool,
    pub center_window: bool,
    pub show_title: bool,
}

pub const LAYOUTS: [&str; 6] = ["SinglePage", "OneColumn", "TwoColumnLeft", "TwoColumnRight", "TwoPageLeft", "TwoPageRight"];
pub const MODES: [&str; 5] = ["UseNone", "UseOutlines", "UseThumbs", "FullScreen", "UseAttachments"];

/// What making a file PDF/A found that it could not fix.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PdfaReport {
    /// Fonts used without their glyphs in the file.
    pub unembedded_fonts: Vec<String>,
}

fn xml_esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A PDF date as an XMP date.
fn xmp_date(pdf: &str) -> String {
    let d = pdf.trim_start_matches("D:");
    let g = |a: usize, b: usize| d.get(a..b).filter(|s| s.bytes().all(|c| c.is_ascii_digit())).unwrap_or("00");
    format!("{}-{}-{}T{}:{}:{}Z", g(0, 4), g(4, 6), g(6, 8), g(8, 10), g(10, 12), g(12, 14))
}

/// The PDF/A part and conformance an XMP packet claims.
fn pdfa_of(xmp: &str) -> Option<(String, String)> {
    let field = |k: &str| {
        let at = xmp.find(&format!("pdfaid:{k}"))? + 7 + k.len();
        let rest = &xmp[at..];
        let rest = rest.trim_start_matches(['>', '=', '"', '\'', ' ']);
        Some(rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>()).filter(|s| !s.is_empty())
    };
    Some((field("part")?, field("conformance").unwrap_or_else(|| "B".into())))
}

/// An XMP packet with the document information (and PDF/A identification).
pub fn xmp(info: &Info, pdfa: Option<(&str, &str)>) -> String {
    let mut x = String::from("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    x.push_str(r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#);
    x.push_str(r#"<rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:pdf="http://ns.adobe.com/pdf/1.3/">"#);
    if let Some((part, conf)) = pdfa {
        x.push_str(&format!("<pdfaid:part>{}</pdfaid:part><pdfaid:conformance>{}</pdfaid:conformance>", xml_esc(part), xml_esc(conf)));
    }
    if !info.title.is_empty() {
        x.push_str(&format!(r#"<dc:title><rdf:Alt><rdf:li xml:lang="x-default">{}</rdf:li></rdf:Alt></dc:title>"#, xml_esc(&info.title)));
    }
    if !info.author.is_empty() {
        x.push_str(&format!("<dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>", xml_esc(&info.author)));
    }
    if !info.subject.is_empty() {
        x.push_str(&format!(r#"<dc:description><rdf:Alt><rdf:li xml:lang="x-default">{}</rdf:li></rdf:Alt></dc:description>"#, xml_esc(&info.subject)));
    }
    let tags = [("pdf:Keywords", &info.keywords), ("pdf:Producer", &info.producer), ("xmp:CreatorTool", &info.creator)];
    for (tag, v) in tags {
        if !v.is_empty() {
            x.push_str(&format!("<{tag}>{}</{tag}>", xml_esc(v)));
        }
    }
    for (tag, v) in [("xmp:CreateDate", &info.created), ("xmp:ModifyDate", &info.modified)] {
        if !v.is_empty() {
            x.push_str(&format!("<{tag}>{}</{tag}>", xmp_date(v)));
        }
    }
    x.push_str("</rdf:Description></rdf:RDF></x:xmpmeta>\n<?xpacket end=\"w\"?>");
    x
}

const INFO_KEYS: [&str; 8] = ["Title", "Author", "Subject", "Keywords", "Creator", "Producer", "CreationDate", "ModDate"];

impl PdfDoc {
    fn info_dict(&self) -> Dict {
        self.key_dict(&self.trailer, "Info").unwrap_or_default()
    }

    pub fn info(&self) -> Info {
        let d = self.info_dict();
        let v: Vec<String> = INFO_KEYS.iter().map(|k| obj_text(&self.key(&d, k))).collect();
        Info { title: v[0].clone(), author: v[1].clone(), subject: v[2].clone(), keywords: v[3].clone(), creator: v[4].clone(), producer: v[5].clone(), created: v[6].clone(), modified: v[7].clone() }
    }

    /// The XMP metadata packet of the catalog, if any.
    fn catalog_xmp(&self) -> Option<String> {
        let (_, cat) = self.catalog()?;
        match self.key(&cat, "Metadata") {
            Obj::Stream(d, raw) => Some(String::from_utf8_lossy(&self.decode(&d, &raw)).into_owned()),
            _ => None,
        }
    }

    /// Writes the information dictionary and its XMP copy (inside a step).
    fn write_info(&mut self, info: &Info, pdfa: Option<(&str, &str)>, always_xmp: bool) {
        let mut d = self.info_dict();
        let vals = [&info.title, &info.author, &info.subject, &info.keywords, &info.creator, &info.producer, &info.created, &info.modified];
        for (k, v) in INFO_KEYS.iter().zip(vals) {
            if v.is_empty() {
                d.remove(k);
            } else {
                d.set(k, text_obj(v));
            }
        }
        match self.trailer.get("Info") {
            Some(Obj::Ref(n, _)) if self.get(*n).is_some() => {
                let n = *n;
                self.set(n, Obj::Dict(d));
            }
            _ => {
                let n = self.add(Obj::Dict(d));
                let mut t = self.trailer.clone();
                t.set("Info", self.r(n));
                self.set_trailer(t);
            }
        }
        let old = self.catalog_xmp();
        if old.is_none() && !always_xmp {
            return;
        }
        let kept = old.as_deref().and_then(pdfa_of);
        let pdfa = pdfa.or(kept.as_ref().map(|(p, c)| (p.as_str(), c.as_str())));
        let packet = xmp(info, pdfa);
        // Uncompressed, so archiving tools can find it.
        let meta = Obj::Stream(Dict::of(vec![("Type", Obj::name_of("Metadata")), ("Subtype", Obj::name_of("XML")), ("Length", Obj::Int(packet.len() as i64))]), Arc::from(packet.into_bytes()));
        let cat = self.catalog().map(|c| c.1).unwrap_or_default();
        match cat.get("Metadata") {
            Some(Obj::Ref(n, _)) if self.get(*n).is_some() => {
                let n = *n;
                self.set(n, meta);
            }
            _ => {
                let n = self.add(meta);
                let r = self.r(n);
                self.set_catalog("Metadata", Some(r));
            }
        }
    }

    /// Changes the document properties; the date of change is now.
    pub fn set_info(&mut self, info: &Info) {
        let mut info = info.clone();
        info.modified = pdf_date_now();
        self.begin("pdf.undo.properties");
        self.write_info(&info, None, false);
        self.end();
    }

    pub fn view(&self) -> View {
        let Some((_, cat)) = self.catalog() else { return View::default() };
        let name = |k: &str| self.key(&cat, k).name().unwrap_or("").to_string();
        let prefs = self.key_dict(&cat, "ViewerPreferences").unwrap_or_default();
        let flag = |k: &str| matches!(self.key(&prefs, k), Obj::Bool(true));
        View { layout: name("PageLayout"), mode: name("PageMode"), fit_window: flag("FitWindow"), center_window: flag("CenterWindow"), show_title: flag("DisplayDocTitle") }
    }

    /// Changes how the file opens (other viewer preferences stay).
    pub fn set_view(&mut self, v: &View) {
        let Some((_, cat)) = self.catalog() else { return };
        self.begin("pdf.undo.initial_view");
        let name = |s: &str| (!s.is_empty()).then(|| Obj::name_of(s));
        self.set_catalog("PageLayout", name(&v.layout));
        self.set_catalog("PageMode", name(&v.mode));
        let mut prefs = self.key_dict(&cat, "ViewerPreferences").unwrap_or_default();
        for (k, on) in [("FitWindow", v.fit_window), ("CenterWindow", v.center_window), ("DisplayDocTitle", v.show_title)] {
            if on {
                prefs.set(k, Obj::Bool(true));
            } else {
                prefs.remove(k);
            }
        }
        self.set_catalog("ViewerPreferences", (!prefs.0.is_empty()).then_some(Obj::Dict(prefs)));
        self.end();
    }

    /// The names of the fonts the pages use without embedding them.
    pub fn unembedded_fonts(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut stack: Vec<Dict> = self.pages().into_iter().map(|p| self.resources(p)).collect();
        while let Some(res) = stack.pop() {
            if let Some(fonts) = self.key_dict(&res, "Font") {
                for (_, f) in &fonts.0 {
                    if let Obj::Ref(n, _) = f
                        && !seen.insert(*n)
                    {
                        continue;
                    }
                    let Obj::Dict(fd) = self.resolve(f) else { continue };
                    if fd.get("Subtype").and_then(Obj::name) == Some("Type3") {
                        if let Some(r) = self.key_dict(&fd, "Resources") {
                            stack.push(r);
                        }
                        continue;
                    }
                    let target = match self.key(&fd, "DescendantFonts") {
                        Obj::Array(a) => a.first().map(|d| self.resolve(d)).and_then(|d| d.dict().cloned()).unwrap_or_default(),
                        _ => fd.clone(),
                    };
                    let desc = self.key_dict(&target, "FontDescriptor").unwrap_or_default();
                    if !["FontFile", "FontFile2", "FontFile3"].iter().any(|k| desc.get(k).is_some()) {
                        let base = self.key(&fd, "BaseFont").name().unwrap_or("?").to_string();
                        if !out.contains(&base) {
                            out.push(base);
                        }
                    }
                }
            }
            // Fonts of form XObjects.
            if let Some(xs) = self.key_dict(&res, "XObject") {
                for (_, x) in &xs.0 {
                    if let Obj::Ref(n, _) = x
                        && !seen.insert(*n)
                    {
                        continue;
                    }
                    if let Obj::Stream(d, _) = self.resolve(x)
                        && d.get("Subtype").and_then(Obj::name) == Some("Form")
                        && let Some(r) = self.key_dict(&d, "Resources")
                    {
                        stack.push(r);
                    }
                }
            }
        }
        out
    }

    /// Makes the file PDF/A-2b: an sRGB output intent with `icc`, XMP
    /// metadata that claims PDF/A and matches the information, no scripts,
    /// attachments or automatic actions, annotations that print. The file
    /// is written whole from then on. Fonts that are not embedded cannot
    /// be fixed here and are reported.
    pub fn make_pdfa(&mut self, icc: &[u8]) -> Result<PdfaReport, Error> {
        if self.is_encrypted() {
            return Err(Error::new("encrypted files cannot be PDF/A"));
        }
        let Some((_, cat)) = self.catalog() else { return Err(Error::new("the PDF file is damaged")) };
        self.begin("pdf.undo.pdfa");
        for k in ["OpenAction", "AA"] {
            self.set_catalog(k, None);
        }
        if let Some(mut names) = self.key_dict(&cat, "Names") {
            names.remove("JavaScript");
            names.remove("EmbeddedFiles");
            self.set_catalog("Names", Some(Obj::Dict(names)));
        }
        if let Some(mut form) = self.key_dict(&cat, "AcroForm")
            && form.remove("NeedAppearances").is_some()
        {
            self.set_catalog("AcroForm", Some(Obj::Dict(form)));
        }
        // Output intent with the profile.
        let icc_obj = super::write::deflate(Dict::of(vec![("N", Obj::Int(3))]), icc);
        let icc_n = self.add(icc_obj);
        let intent = Dict::of(vec![
            ("Type", Obj::name_of("OutputIntent")),
            ("S", Obj::name_of("GTS_PDFA1")),
            ("OutputConditionIdentifier", Obj::Str(b"sRGB IEC61966-2.1".to_vec())),
            ("Info", Obj::Str(b"sRGB IEC61966-2.1".to_vec())),
            ("DestOutputProfile", self.r(icc_n)),
        ]);
        self.set_catalog("OutputIntents", Some(Obj::Array(vec![Obj::Dict(intent)])));
        // Pages: no automatic actions; annotations print and show.
        for pn in self.pages() {
            let Some(mut d) = self.dict(pn) else { continue };
            if d.remove("AA").is_some() {
                self.set(pn, Obj::Dict(d.clone()));
            }
            let annots: Vec<Obj> = match self.key(&d, "Annots") {
                Obj::Array(a) => a,
                _ => Vec::new(),
            };
            for a in annots {
                let Obj::Ref(n, _) = a else { continue };
                let Some(mut ad) = self.dict(n) else { continue };
                if ad.get("Subtype").and_then(Obj::name) == Some("Popup") {
                    continue;
                }
                let f = self.key(&ad, "F").int().unwrap_or(0);
                // Print on; Invisible, Hidden, ToggleNoView and NoView off.
                let nf = (f | 4) & !(1 | 2 | 32 | 256);
                let js = matches!(self.key_dict(&ad, "A").and_then(|x| x.get("S").and_then(Obj::name).map(str::to_string)).as_deref(), Some("JavaScript" | "Launch"));
                if nf != f || js || ad.get("AA").is_some() {
                    ad.set("F", Obj::Int(nf));
                    ad.remove("AA");
                    if js {
                        ad.remove("A");
                    }
                    self.set(n, Obj::Dict(ad));
                }
            }
        }
        let mut info = self.info();
        if info.producer.is_empty() {
            info.producer = "ApexDesk".into();
        }
        if info.created.is_empty() {
            info.created = pdf_date_now();
        }
        info.modified = pdf_date_now();
        self.write_info(&info, Some(("2", "B")), true);
        self.need_whole();
        self.end();
        Ok(PdfaReport { unembedded_fonts: self.unembedded_fonts() })
    }

    /// Whether the file's metadata claims PDF/A (part and conformance).
    pub fn pdfa_claim(&self) -> Option<(String, String)> {
        self.catalog_xmp().as_deref().and_then(pdfa_of)
    }
}
