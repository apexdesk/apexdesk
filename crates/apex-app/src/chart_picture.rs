//! A chart saved as a picture file: PNG (pixels at twice the sheet size,
//! on white) or PDF (vector shapes and real text, one page the size of
//! the chart).

use apex_sheet::Locale;
use apex_sheet::chart::Chart;
use eframe::egui::{self, Pos2, Rect, vec2};

use apex_sheet::chart::ChartData;

use crate::charts;
use crate::print::{Job, Page, no_paint};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Pdf,
}

impl Format {
    pub const ALL: [Format; 2] = [Format::Png, Format::Pdf];

    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Pdf => "pdf",
        }
    }

    /// The format a file name asks for (by its extension).
    pub fn from_path(p: &std::path::Path) -> Option<Format> {
        let e = p.extension()?.to_str()?.to_ascii_lowercase();
        Format::ALL.into_iter().find(|f| f.ext() == e)
    }
}

/// The bytes of the picture file of a chart.
pub fn picture(ctx: &egui::Context, chart: &Chart, data: &ChartData, loc: &Locale, format: Format) -> Option<Vec<u8>> {
    let (w, h) = (chart.width.max(1.0), chart.height.max(1.0));
    match format {
        Format::Png => charts::encode_png(&charts::render(ctx, chart, data, w, h, 2.0, loc)),
        Format::Pdf => {
            let rect = Rect::from_min_size(Pos2::ZERO, vec2(w, h));
            let mut shapes = vec![egui::Shape::rect_filled(rect, 0.0, egui::Color32::WHITE)];
            shapes.extend(charts::build(ctx, rect, chart, data, 1.0, loc));
            let shapes = shapes.into_iter().map(|shape| egui::epaint::ClippedShape { clip_rect: rect, shape }).collect();
            let title = if chart.title.is_empty() { chart.name.clone() } else { chart.title.clone() };
            let job = Job { title, pages: vec![Page { size: rect.size(), shapes, links: Vec::new(), tags: Vec::new() }], ..Job::default() };
            Some(crate::pdf_out::to_pdf(&job, &[0], &mut no_paint))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::chart::ChartType;

    #[test]
    fn png_and_pdf() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let mut c = Chart::new(ChartType::Column, apex_sheet::CellRef::new(0, 0), 200.0, 120.0);
        c.title = "Sales".into();
        c.series.push(Default::default());
        let series = vec![apex_sheet::chart::SeriesData { name: "S".into(), values: vec![Some(1.0), Some(3.0)], ..Default::default() }];
        let data = ChartData { categories: vec!["a".into(), "b".into()], series, value_format: "General".into(), cat_format: "General".into(), levels: Vec::new() };
        let png = picture(&ctx, &c, &data, &Locale::EN, Format::Png).unwrap();
        assert_eq!(crate::images::decode(&png).map(|d| (d.width, d.height)), Some((400, 240)));
        let pdf = picture(&ctx, &c, &data, &Locale::EN, Format::Pdf).unwrap();
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.starts_with("%PDF") && s.contains("/MediaBox [0 0 150.0"), "{:?}", s.find("/MediaBox").map(|i| &s[i..i + 30]));
        assert_eq!(Format::from_path(std::path::Path::new("x.PNG")), Some(Format::Png));
    }
}
