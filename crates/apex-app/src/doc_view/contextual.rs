//! The commands of contextual tabs that aren't showing, for the command
//! search: each tab is drawn off screen on a small sample document with a
//! picture, a shape and a table.

use apex_doc::{DocShape, Image, OBJECT, Shape, ShapeKind, Table};

use super::*;
use crate::toolstrip::{self, Entry, TabDef};

/// A 1x1 PNG.
pub const SAMPLE_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn object_para(attrs: Attrs) -> Paragraph {
    let mut p = Paragraph::new("");
    p.push(&OBJECT.to_string(), attrs);
    p
}

impl DocView {
    /// Contextual tabs other than `showing`, with their commands.
    pub fn contextual_commands(ui: &mut egui::Ui, shell: &mut Shell, showing: &[TabDef]) -> Vec<(TabDef, Vec<Entry>)> {
        let pic = Attrs { image: Some(Arc::new(Image::new(Arc::from(SAMPLE_PNG), 20.0, 20.0))), ..Default::default() };
        let mut shape = DocShape::new(Shape::new(ShapeKind::Rect), 40.0, 30.0);
        shape.wrap = apex_doc::ShapeWrap::Inline;
        let shp = Attrs { shape: Some(Arc::new(shape)), ..Default::default() };
        let mut paras = vec![object_para(pic), object_para(shp)];
        paras.extend(Table::new(2, 2, 200.0).into_paras(1));
        paras.push(Paragraph::new(""));
        let mut v = DocView::new(Document::from_paragraphs(paras));
        v.layout = v.build_layout(ui.ctx(), 1.0, (1, 0, 0, 0, 0), Layout::default());
        let object = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, OBJECT.len_utf8()) };
        let mut out = Vec::new();
        for id in ["picture", "shape", "table", "outline"] {
            if showing.iter().any(|t| t.id == id) {
                continue;
            }
            v.views.mode = if id == "outline" { ViewMode::Outline } else { ViewMode::Print };
            v.sel = match id {
                "picture" => object,
                "shape" => Selection { anchor: Pos::new(1, 0), cursor: Pos::new(1, OBJECT.len_utf8()) },
                "table" => Selection::caret(Pos::new(2, 0)),
                _ => Selection::caret(Pos::new(0, 0)),
            };
            let Some(t) = v.toolstrip_tabs().into_iter().find(|t| t.id == id) else { continue };
            let entries = toolstrip::collect(|| toolstrip::within(tr(t.label), || v.toolstrip(ui, id, shell)));
            out.push((t, entries));
        }
        out
    }
}
