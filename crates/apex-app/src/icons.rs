//! Small vector icons drawn with the painter, so they never depend on which
//! glyphs the system fonts happen to have.

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, pos2, vec2};

// Some icons are drawn for commands still to come.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Undo,
    Redo,
    AlignLeft,
    AlignCenter,
    AlignRight,
    Justify,
    Wrap,
    Borders,
    Fill,
    Highlight,
    Bullets,
    Numbering,
    Indent,
    Outdent,
    SortAsc,
    SortDesc,
    Search,
    Doc,
    Sheet,
    Folder,
    New,
    Save,
    Cut,
    Copy,
    Paste,
    Brush,
    ClearFormat,
    Superscript,
    Subscript,
    ChangeCase,
    GrowFont,
    ShrinkFont,
    AlignTop,
    AlignMiddle,
    AlignBottom,
    Merge,
    Currency,
    Thousands,
    Calendar,
    CellIndent,
    CellOutdent,
    Orientation,
    FillDown,
    Eraser,
    Pen,
    Pencil,
    Lasso,
    Function,
    Pilcrow,
    LineSpacing,
    Shading,
    NoList,
    Sigma,
    Percent,
    Decimals,
    FewerDecimals,
    ChevronUp,
    ChevronDown,
    Filter,
    CondFormat,
    Validation,
    Table,
    /// Navigation pane: a side panel beside lines.
    NavPane,
    // Charts.
    /// Insert chart: a generic column chart.
    Chart,
    ChartColumn,
    ChartBar,
    ChartLine,
    ChartArea,
    ChartPie,
    ChartDoughnut,
    ChartScatter,
    /// Columns with a line over them.
    ChartCombo,
    ChartWaterfall,
    ChartHistogram,
    ChartRadar,
    ChartBubble,
    ChartStock,
    ChartSurface,
    ChartTreemap,
    ChartSunburst,
    ChartBox,
    ChartFunnel,
    ChartPareto,
    ChartMekko,
    ChartTimeline,
    /// A tiny line chart inside a cell.
    Sparkline,
    // Insert.
    Picture,
    /// A picture fitted inside a cell of a grid.
    PictureInCell,
    /// A checked box (Insert > Checkbox).
    Checkbox,
    Shapes,
    TextBox,
    /// Pivot table: a grid with header row and column and a turning arrow.
    Pivot,
    Link,
    /// Sticky note.
    Note,
    Comment,
    Symbol,
    Equation,
    /// Embedded object: a page inside a frame.
    Object,
    /// QR code or barcode: three finder squares and some modules.
    QrCode,
    HeaderFooter,
    PageNumber,
    PageBreak,
    Hyphen,
    Brightness,
    Recolor,
    Transparency,
    PicStyles,
    Control,
    Thesaurus,
    DateTime,
    // Page layout.
    Margins,
    PageOrientation,
    PageSize,
    /// Text in two columns.
    Columns,
    PrintArea,
    PrintTitles,
    Watermark,
    PageColor,
    PageBorders,
    Themes,
    Styles,
    CellStyles,
    FormatAsTable,
    // Review and references.
    Spelling,
    /// Read Aloud and its player buttons.
    ReadAloud,
    SpeakPlay,
    SpeakPause,
    SpeakStop,
    SpeakPrev,
    SpeakNext,
    /// A video: a film frame with a play mark.
    Video,
    /// A sound: a speaker and a note.
    Audio,
    /// Padlock.
    Protect,
    TrackChanges,
    Accept,
    Reject,
    Compare,
    /// Table of contents.
    Toc,
    Footnote,
    Caption,
    Bookmark,
    /// A closed book (book mode).
    Book,
    /// A new chapter: a page with a big heading.
    Chapter,
    /// A new scene: the scene break mark.
    Scene,
    Cards,
    CrossRef,
    /// A field code: braces.
    Field,
    MailMerge,
    Labels,
    Envelope,
    /// A multilevel list.
    Multilevel,
    /// Quick Analysis: a table with a lightning bolt.
    QuickAnalysis,
    // View.
    ZoomIn,
    ZoomOut,
    Zoom100,
    /// Freeze panes: a grid with its top row and first column set apart.
    Freeze,
    Gridlines,
    ShowFormulas,
    Fullscreen,
    SideBySide,
    Ruler,
    /// Outline levels: indented lines.
    Outline,
    Window,
    /// Read mode: an open book.
    ReadMode,
    /// A presentation: a screen on a stand with a bar chart.
    Slides,
    /// A new slide: a slide with a plus.
    NewSlide,
    /// Print layout: a page with lines.
    PrintLayout,
    /// Draft: lines with a dotted page break.
    Draft,
    /// Split: a window cut in two.
    Split,
    /// Focus: corners around a line.
    Focus,
    /// Two pages next to each other.
    MultiPage,
    // Formulas and data.
    TracePrecedents,
    TraceDependents,
    RemoveArrows,
    ErrorCheck,
    /// Calculate now: a calculator.
    Calculate,
    NameManager,
    GoalSeek,
    Solver,
    Subtotal,
    Group,
    Ungroup,
    TextToColumns,
    RemoveDuplicates,
    /// What-if analysis: a question mark over cells.
    Scenario,
    // Cells.
    InsertRows,
    InsertCols,
    DeleteRows,
    DeleteCols,
    CellFormat,
    AutoFit,
    Hide,
    // Documents.
    DropCap,
    /// Artistic text: a letter with an outline and a shadow.
    TextArt,
    Replace,
    Select,
    // App and files.
    Print,
    Pdf,
    Export,
    Template,
    Close,
    Options,
    Keyboard,
    Help,
    Language,
    /// Light or dark theme: a half-filled circle.
    Theme,
    Graph,
    Canvas,
    /// A plus sign alone (add a section...).
    Add,
    /// A list with ticked boxes.
    Tasks,
    /// A waste bin.
    Trash,
    /// A spiral notebook (ApexDesk's notebooks), in one colour.
    Notebook,
}

/// Every icon, in declaration order (for the contact sheet test, and to
/// find icons by name).
pub const ALL: &[Icon] = &[
    Icon::Undo,
    Icon::Redo,
    Icon::AlignLeft,
    Icon::AlignCenter,
    Icon::AlignRight,
    Icon::Justify,
    Icon::Wrap,
    Icon::Borders,
    Icon::Fill,
    Icon::Highlight,
    Icon::Bullets,
    Icon::Numbering,
    Icon::Indent,
    Icon::Outdent,
    Icon::SortAsc,
    Icon::SortDesc,
    Icon::Search,
    Icon::Doc,
    Icon::Sheet,
    Icon::Folder,
    Icon::New,
    Icon::Save,
    Icon::Cut,
    Icon::Copy,
    Icon::Paste,
    Icon::Brush,
    Icon::ClearFormat,
    Icon::Superscript,
    Icon::Subscript,
    Icon::ChangeCase,
    Icon::GrowFont,
    Icon::ShrinkFont,
    Icon::AlignTop,
    Icon::AlignMiddle,
    Icon::AlignBottom,
    Icon::Merge,
    Icon::Currency,
    Icon::Thousands,
    Icon::Calendar,
    Icon::CellIndent,
    Icon::CellOutdent,
    Icon::Orientation,
    Icon::FillDown,
    Icon::Eraser,
    Icon::Pen,
    Icon::Pencil,
    Icon::Lasso,
    Icon::Function,
    Icon::Pilcrow,
    Icon::LineSpacing,
    Icon::Shading,
    Icon::NoList,
    Icon::Sigma,
    Icon::Percent,
    Icon::Decimals,
    Icon::FewerDecimals,
    Icon::ChevronUp,
    Icon::ChevronDown,
    Icon::Filter,
    Icon::CondFormat,
    Icon::Validation,
    Icon::Table,
    Icon::NavPane,
    Icon::Chart,
    Icon::ChartColumn,
    Icon::ChartBar,
    Icon::ChartLine,
    Icon::ChartArea,
    Icon::ChartPie,
    Icon::ChartDoughnut,
    Icon::ChartScatter,
    Icon::ChartCombo,
    Icon::ChartWaterfall,
    Icon::ChartHistogram,
    Icon::ChartRadar,
    Icon::ChartBubble,
    Icon::ChartStock,
    Icon::ChartSurface,
    Icon::ChartTreemap,
    Icon::ChartSunburst,
    Icon::ChartBox,
    Icon::ChartFunnel,
    Icon::ChartPareto,
    Icon::ChartMekko,
    Icon::ChartTimeline,
    Icon::Sparkline,
    Icon::Picture,
    Icon::PictureInCell,
    Icon::Checkbox,
    Icon::Shapes,
    Icon::TextBox,
    Icon::Pivot,
    Icon::Link,
    Icon::Note,
    Icon::Comment,
    Icon::Symbol,
    Icon::Equation,
    Icon::Object,
    Icon::QrCode,
    Icon::HeaderFooter,
    Icon::PageNumber,
    Icon::PageBreak,
    Icon::Hyphen,
    Icon::Brightness,
    Icon::Recolor,
    Icon::Transparency,
    Icon::PicStyles,
    Icon::Control,
    Icon::Thesaurus,
    Icon::DateTime,
    Icon::Margins,
    Icon::PageOrientation,
    Icon::PageSize,
    Icon::Columns,
    Icon::PrintArea,
    Icon::PrintTitles,
    Icon::Watermark,
    Icon::PageColor,
    Icon::PageBorders,
    Icon::Themes,
    Icon::Styles,
    Icon::CellStyles,
    Icon::FormatAsTable,
    Icon::Spelling,
    Icon::ReadAloud,
    Icon::SpeakPlay,
    Icon::SpeakPause,
    Icon::SpeakStop,
    Icon::SpeakPrev,
    Icon::SpeakNext,
    Icon::Video,
    Icon::Audio,
    Icon::Protect,
    Icon::TrackChanges,
    Icon::Accept,
    Icon::Reject,
    Icon::Compare,
    Icon::Toc,
    Icon::Footnote,
    Icon::Caption,
    Icon::Bookmark,
    Icon::Book,
    Icon::Chapter,
    Icon::Scene,
    Icon::Cards,
    Icon::CrossRef,
    Icon::Field,
    Icon::MailMerge,
    Icon::Labels,
    Icon::Envelope,
    Icon::Multilevel,
    Icon::QuickAnalysis,
    Icon::ZoomIn,
    Icon::ZoomOut,
    Icon::Zoom100,
    Icon::Freeze,
    Icon::Gridlines,
    Icon::ShowFormulas,
    Icon::Fullscreen,
    Icon::SideBySide,
    Icon::Ruler,
    Icon::Outline,
    Icon::Window,
    Icon::ReadMode,
    Icon::Slides,
    Icon::NewSlide,
    Icon::PrintLayout,
    Icon::Draft,
    Icon::Split,
    Icon::Focus,
    Icon::MultiPage,
    Icon::TracePrecedents,
    Icon::TraceDependents,
    Icon::RemoveArrows,
    Icon::ErrorCheck,
    Icon::Calculate,
    Icon::NameManager,
    Icon::GoalSeek,
    Icon::Solver,
    Icon::Subtotal,
    Icon::Group,
    Icon::Ungroup,
    Icon::TextToColumns,
    Icon::RemoveDuplicates,
    Icon::Scenario,
    Icon::InsertRows,
    Icon::InsertCols,
    Icon::DeleteRows,
    Icon::DeleteCols,
    Icon::CellFormat,
    Icon::AutoFit,
    Icon::Hide,
    Icon::DropCap,
    Icon::TextArt,
    Icon::Replace,
    Icon::Select,
    Icon::Print,
    Icon::Pdf,
    Icon::Export,
    Icon::Template,
    Icon::Close,
    Icon::Options,
    Icon::Keyboard,
    Icon::Help,
    Icon::Language,
    Icon::Theme,
    Icon::Graph,
    Icon::Canvas,
    Icon::Add,
    Icon::Tasks,
    Icon::Trash,
    Icon::Notebook,
];

/// Draws `icon` in one color.
pub fn paint(p: &egui::Painter, r: Rect, icon: Icon, color: Color32) {
    paint_accent(p, r, icon, color, color);
}

/// Draws `icon` with its lines in `color` and its highlights (chart bars,
/// a selected cell, a lens...) in `accent`; with `accent == color` it is
/// the same as [`paint`]. A few icons keep fixed colors that carry meaning
/// (a red cross, a green check, a yellow note).
pub fn paint_accent(p: &egui::Painter, r: Rect, icon: Icon, color: Color32, accent: Color32) {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    let s = Stroke::new((r.width() / 12.0).max(1.2), color);
    let at = |x: f32, y: f32| pos2(r.min.x + x * r.width(), r.min.y + y * r.height());
    let line = |x0: f32, y0: f32, x1: f32, y1: f32| p.line_segment([at(x0, y0), at(x1, y1)], s);
    let tri = |a: Pos2, b: Pos2, c: Pos2| p.add(Shape::convex_polygon(vec![a, b, c], color, Stroke::NONE));
    // Helpers for the two-tone icons, all in the same 0..1 units.
    let w = r.width();
    let thin = Stroke::new(s.width * 0.6, color);
    let acc = Stroke::new(s.width, accent);
    let soft = accent.gamma_multiply(0.3);
    let mid = accent.gamma_multiply(0.6);
    let red = Color32::from_rgb(0xD0, 0x30, 0x30);
    let green = Color32::from_rgb(0x2E, 0x9D, 0x4F);
    let rc = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(at(x0, y0), at(x1, y1));
    let frame = |x0: f32, y0: f32, x1: f32, y1: f32, st: Stroke| {
        p.rect_stroke(rc(x0, y0, x1, y1), w * 0.03, st, egui::StrokeKind::Middle);
    };
    let fill = |x0: f32, y0: f32, x1: f32, y1: f32, c: Color32| {
        p.rect_filled(rc(x0, y0, x1, y1), 0.0, c);
    };
    let seg = |x0: f32, y0: f32, x1: f32, y1: f32, st: Stroke| {
        p.line_segment([at(x0, y0), at(x1, y1)], st);
    };
    let pts = |v: &[(f32, f32)]| v.iter().map(|&(x, y)| at(x, y)).collect::<Vec<Pos2>>();
    let path = |v: &[(f32, f32)], st: Stroke| {
        p.add(Shape::line(pts(v), st));
    };
    let closed = |v: &[(f32, f32)], st: Stroke| {
        p.add(Shape::closed_line(pts(v), st));
    };
    let solid = |v: &[(f32, f32)], c: Color32| {
        p.add(Shape::convex_polygon(pts(v), c, Stroke::NONE));
    };
    let dashed = |x0: f32, y0: f32, x1: f32, y1: f32, st: Stroke| {
        p.extend(Shape::dashed_line(&[at(x0, y0), at(x1, y1)], st, w * 0.1, w * 0.07));
    };
    let text = |x: f32, y: f32, t: &str, size: f32, c: Color32| {
        p.text(at(x, y), egui::Align2::CENTER_CENTER, t, egui::FontId::proportional(r.height() * size), c);
    };
    // Points along an elliptic arc from angle `a0` to `a1`.
    let ellipse = |cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32| -> Vec<Pos2> {
        (0..=24)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / 24.0;
                at(cx + rx * a.cos(), cy + ry * a.sin())
            })
            .collect()
    };
    let arrow = |x0: f32, y0: f32, x1: f32, y1: f32, st: Stroke| {
        let (a, b) = (at(x0, y0), at(x1, y1));
        let d = (b - a).normalized();
        let head = (w * 0.2).max(st.width * 2.6);
        let base = b - d * head;
        p.line_segment([a, base + d * head * 0.3], st);
        let n = d.rot90() * head * 0.55;
        p.add(Shape::convex_polygon(vec![b, base + n, base - n], st.color, Stroke::NONE));
    };
    // A double-headed arrow.
    let arrows = |x0: f32, y0: f32, x1: f32, y1: f32, st: Stroke| {
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        arrow(mx, my, x0, y0, st);
        arrow(mx, my, x1, y1, st);
    };
    // A sheet of paper with a folded corner.
    let page_pts = |x0: f32, y0: f32, x1: f32, y1: f32| {
        let f = (x1 - x0) * 0.3;
        [(x0, y0), (x1 - f, y0), (x1, y0 + f), (x1, y1), (x0, y1)]
    };
    let page = |x0: f32, y0: f32, x1: f32, y1: f32, st: Stroke| {
        let v = page_pts(x0, y0, x1, y1);
        closed(&v, st);
        path(&[v[1], (v[1].0, v[2].1), v[2]], st);
    };
    let page_fill = |x0: f32, y0: f32, x1: f32, y1: f32, c: Color32| solid(&page_pts(x0, y0, x1, y1), c);
    let axes = || path(&[(0.12, 0.1), (0.12, 0.88), (0.92, 0.88)], s);
    let cross = |cx: f32, cy: f32, h: f32, st: Stroke| {
        seg(cx - h, cy - h, cx + h, cy + h, st);
        seg(cx - h, cy + h, cx + h, cy - h, st);
    };
    let plus = |cx: f32, cy: f32, h: f32, st: Stroke| {
        seg(cx - h, cy, cx + h, cy, st);
        seg(cx, cy - h, cx, cy + h, st);
    };
    let gear = |cx: f32, cy: f32, rad: f32, st: Stroke| {
        let (c, rr) = (at(cx, cy), rad * w);
        p.circle_stroke(c, rr * 0.66, st);
        p.circle_stroke(c, rr * 0.26, st);
        for k in 0..8 {
            let a = k as f32 * TAU / 8.0;
            let d = vec2(a.cos(), a.sin());
            p.line_segment([c + d * rr * 0.6, c + d * rr], Stroke::new(rr * 0.32, st.color));
        }
    };
    // A pencil from its end at (x0, y0) to its tip at (x1, y1).
    let pencil = |x0: f32, y0: f32, x1: f32, y1: f32| {
        let (a, b) = (at(x0, y0), at(x1, y1));
        let d = (b - a).normalized();
        let n = d.rot90() * w * 0.09;
        let base = b - d * w * 0.16;
        let body = vec![a + n, a - n, base - n, base + n];
        p.add(Shape::convex_polygon(body.clone(), accent, Stroke::NONE));
        p.add(Shape::closed_line(body, thin));
        p.add(Shape::convex_polygon(vec![base + n, base - n, b], color, Stroke::NONE));
    };
    // A link of a chain: a capsule along angle `ang`.
    let capsule = |cx: f32, cy: f32, len: f32, rad: f32, ang: f32, st: Stroke| {
        let (dx, dy) = (ang.cos() * len / 2.0, ang.sin() * len / 2.0);
        let mut v = ellipse(cx + dx, cy + dy, rad, rad, ang - FRAC_PI_2, ang + FRAC_PI_2);
        v.extend(ellipse(cx - dx, cy - dy, rad, rad, ang + FRAC_PI_2, ang + 3.0 * FRAC_PI_2));
        p.add(Shape::closed_line(v, st));
    };
    match icon {
        Icon::Undo | Icon::Redo => {
            let flip = icon == Icon::Redo;
            let fx = |x: f32| if flip { 1.0 - x } else { x };
            let c = at(0.5, 0.58);
            let rad = r.width() * 0.3;
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.1 + 1.25 * i as f32 / 16.0);
                    let x = (a.cos() * rad) / r.width();
                    pos2(r.min.x + fx(0.5 + x) * r.width(), c.y + a.sin() * rad)
                })
                .collect();
            let start = pts[0];
            p.add(Shape::line(pts, s));
            let d = r.width() * 0.16;
            let dir = if flip { 1.0 } else { -1.0 };
            tri(start + vec2(dir * d, -d * 0.2), start + vec2(-dir * d * 0.3, -d * 1.2), start + vec2(-dir * d * 0.3, d * 0.8));
        }
        Icon::AlignLeft | Icon::AlignCenter | Icon::AlignRight | Icon::Justify => {
            for (i, full) in [true, false, true, false].iter().enumerate() {
                let y = 0.22 + i as f32 * 0.19;
                let w = if *full || icon == Icon::Justify { 0.76 } else { 0.5 };
                let x0 = match icon {
                    Icon::AlignCenter => 0.5 - w / 2.0,
                    Icon::AlignRight => 0.88 - w,
                    _ => 0.12,
                };
                line(x0, y, x0 + w, y);
            }
        }
        Icon::Wrap => {
            line(0.12, 0.25, 0.88, 0.25);
            line(0.12, 0.75, 0.4, 0.75);
            let pts = vec![at(0.12, 0.5), at(0.74, 0.5), at(0.86, 0.56), at(0.88, 0.64), at(0.84, 0.72), at(0.72, 0.75), at(0.6, 0.75)];
            p.add(Shape::line(pts, s));
            tri(at(0.52, 0.75), at(0.64, 0.64), at(0.64, 0.86));
        }
        Icon::Borders => {
            p.rect_stroke(Rect::from_min_max(at(0.15, 0.15), at(0.85, 0.85)), 1.0, s, egui::StrokeKind::Middle);
            let thin = Stroke::new(s.width * 0.6, color.gamma_multiply(0.6));
            p.line_segment([at(0.5, 0.15), at(0.5, 0.85)], thin);
            p.line_segment([at(0.15, 0.5), at(0.85, 0.5)], thin);
        }
        Icon::Fill => {
            let pts = vec![at(0.2, 0.45), at(0.48, 0.17), at(0.76, 0.45), at(0.48, 0.73)];
            p.add(Shape::closed_line(pts, s));
            line(0.34, 0.31, 0.62, 0.59);
            p.circle_filled(at(0.82, 0.62), r.width() * 0.07, color);
        }
        Icon::Highlight => {
            let pts = vec![at(0.3, 0.62), at(0.62, 0.18), at(0.8, 0.32), at(0.48, 0.76)];
            p.add(Shape::closed_line(pts, s));
            tri(at(0.3, 0.62), at(0.48, 0.76), at(0.22, 0.82));
        }
        Icon::Bullets | Icon::Numbering => {
            for i in 0..3 {
                let y = 0.25 + i as f32 * 0.25;
                if icon == Icon::Bullets {
                    p.circle_filled(at(0.18, y), r.width() * 0.065, color);
                } else {
                    p.text(at(0.18, y), egui::Align2::CENTER_CENTER, (i + 1).to_string(), egui::FontId::proportional(r.height() * 0.32), color);
                }
                line(0.36, y, 0.88, y);
            }
        }
        Icon::ChevronUp => {
            line(0.22, 0.64, 0.5, 0.36);
            line(0.5, 0.36, 0.78, 0.64);
        }
        Icon::ChevronDown => {
            line(0.22, 0.36, 0.5, 0.64);
            line(0.5, 0.64, 0.78, 0.36);
        }
        Icon::Indent | Icon::Outdent => {
            line(0.12, 0.2, 0.88, 0.2);
            line(0.46, 0.4, 0.88, 0.4);
            line(0.46, 0.6, 0.88, 0.6);
            line(0.12, 0.8, 0.88, 0.8);
            if icon == Icon::Indent {
                tri(at(0.12, 0.36), at(0.32, 0.5), at(0.12, 0.64));
            } else {
                tri(at(0.32, 0.36), at(0.12, 0.5), at(0.32, 0.64));
            }
        }
        Icon::SortAsc | Icon::SortDesc => {
            let (top, bottom) = if icon == Icon::SortAsc { ("A", "Z") } else { ("Z", "A") };
            let f = egui::FontId::proportional(r.height() * 0.4);
            p.text(at(0.3, 0.27), egui::Align2::CENTER_CENTER, top, f.clone(), color);
            p.text(at(0.3, 0.73), egui::Align2::CENTER_CENTER, bottom, f, color);
            line(0.72, 0.15, 0.72, 0.75);
            tri(at(0.58, 0.68), at(0.86, 0.68), at(0.72, 0.88));
        }
        Icon::Search => {
            p.circle_stroke(at(0.42, 0.42), r.width() * 0.24, s);
            p.line_segment([at(0.6, 0.6), at(0.85, 0.85)], Stroke::new(s.width * 1.5, color));
        }
        Icon::Slides => {
            p.rect_stroke(Rect::from_min_max(at(0.1, 0.14), at(0.9, 0.7)), 1.0, s, egui::StrokeKind::Middle);
            line(0.5, 0.7, 0.5, 0.86);
            line(0.34, 0.9, 0.66, 0.9);
            line(0.32, 0.58, 0.32, 0.46);
            line(0.5, 0.58, 0.5, 0.32);
            line(0.68, 0.58, 0.68, 0.4);
        }
        Icon::NewSlide => {
            p.rect_stroke(Rect::from_min_max(at(0.08, 0.2), at(0.78, 0.72)), 1.0, s, egui::StrokeKind::Middle);
            line(0.2, 0.36, 0.56, 0.36);
            line(0.2, 0.5, 0.48, 0.5);
            line(0.8, 0.62, 0.8, 0.94);
            line(0.64, 0.78, 0.96, 0.78);
        }
        Icon::Doc | Icon::Sheet => {
            let pts = vec![at(0.2, 0.08), at(0.62, 0.08), at(0.82, 0.28), at(0.82, 0.92), at(0.2, 0.92)];
            p.add(Shape::closed_line(pts, s));
            line(0.62, 0.08, 0.62, 0.28);
            line(0.62, 0.28, 0.82, 0.28);
            if icon == Icon::Doc {
                for y in [0.45, 0.6, 0.75] {
                    line(0.32, y, 0.7, y);
                }
            } else {
                p.rect_stroke(Rect::from_min_max(at(0.3, 0.42), at(0.72, 0.8)), 0.0, Stroke::new(s.width * 0.8, color), egui::StrokeKind::Middle);
                line(0.51, 0.42, 0.51, 0.8);
                line(0.3, 0.61, 0.72, 0.61);
            }
        }
        Icon::New => {
            let pts = vec![at(0.2, 0.08), at(0.62, 0.08), at(0.82, 0.28), at(0.82, 0.92), at(0.2, 0.92)];
            p.add(Shape::closed_line(pts, s));
            line(0.62, 0.08, 0.62, 0.28);
            line(0.62, 0.28, 0.82, 0.28);
            line(0.51, 0.45, 0.51, 0.78);
            line(0.35, 0.615, 0.67, 0.615);
        }
        Icon::Save => {
            let pts = vec![at(0.12, 0.12), at(0.74, 0.12), at(0.88, 0.26), at(0.88, 0.88), at(0.12, 0.88)];
            p.add(Shape::closed_line(pts, s));
            p.rect_stroke(Rect::from_min_max(at(0.3, 0.12), at(0.66, 0.36)), 0.0, s, egui::StrokeKind::Middle);
            p.rect_stroke(Rect::from_min_max(at(0.26, 0.56), at(0.74, 0.88)), 0.0, s, egui::StrokeKind::Middle);
        }
        Icon::Cut => {
            p.circle_stroke(at(0.28, 0.76), r.width() * 0.12, s);
            p.circle_stroke(at(0.72, 0.76), r.width() * 0.12, s);
            line(0.36, 0.66, 0.74, 0.1);
            line(0.64, 0.66, 0.26, 0.1);
        }
        Icon::Copy => {
            p.rect_stroke(Rect::from_min_max(at(0.14, 0.1), at(0.6, 0.66)), 1.0, s, egui::StrokeKind::Middle);
            p.rect_filled(Rect::from_min_max(at(0.4, 0.34), at(0.86, 0.9)), 1.0, Color32::from_black_alpha(0));
            p.rect_stroke(Rect::from_min_max(at(0.4, 0.34), at(0.86, 0.9)), 1.0, s, egui::StrokeKind::Middle);
        }
        Icon::Paste => {
            p.rect_stroke(Rect::from_min_max(at(0.16, 0.16), at(0.84, 0.92)), 1.5, s, egui::StrokeKind::Middle);
            p.rect_filled(Rect::from_min_max(at(0.34, 0.06), at(0.66, 0.24)), 1.5, color);
            line(0.32, 0.46, 0.68, 0.46);
            line(0.32, 0.62, 0.68, 0.62);
            line(0.32, 0.78, 0.56, 0.78);
        }
        Icon::Brush => {
            p.rect_stroke(Rect::from_min_max(at(0.14, 0.1), at(0.82, 0.34)), 2.0, s, egui::StrokeKind::Middle);
            line(0.82, 0.22, 0.9, 0.22);
            line(0.9, 0.22, 0.9, 0.46);
            line(0.9, 0.46, 0.5, 0.46);
            line(0.5, 0.46, 0.5, 0.6);
            p.rect_filled(Rect::from_min_max(at(0.42, 0.6), at(0.58, 0.92)), 1.0, color);
        }
        Icon::ClearFormat => {
            let f = egui::FontId::proportional(r.height() * 0.72);
            p.text(at(0.38, 0.46), egui::Align2::CENTER_CENTER, "A", f, color);
            let red = Color32::from_rgb(0xD0, 0x30, 0x30);
            let pts = vec![at(0.56, 0.62), at(0.76, 0.42), at(0.94, 0.6), at(0.74, 0.8)];
            p.add(Shape::convex_polygon(pts, red, Stroke::NONE));
            line(0.1, 0.92, 0.9, 0.92);
        }
        Icon::Superscript | Icon::Subscript => {
            let f = egui::FontId::proportional(r.height() * 0.62);
            p.text(at(0.34, 0.58), egui::Align2::CENTER_CENTER, "x", f, color);
            let small = egui::FontId::proportional(r.height() * 0.42);
            let y = if icon == Icon::Superscript { 0.24 } else { 0.84 };
            p.text(at(0.76, y), egui::Align2::CENTER_CENTER, "2", small, color);
        }
        Icon::ChangeCase => {
            p.text(at(0.32, 0.54), egui::Align2::CENTER_CENTER, "A", egui::FontId::proportional(r.height() * 0.7), color);
            p.text(at(0.76, 0.62), egui::Align2::CENTER_CENTER, "a", egui::FontId::proportional(r.height() * 0.52), color);
        }
        Icon::GrowFont | Icon::ShrinkFont => {
            p.text(at(0.38, 0.56), egui::Align2::CENTER_CENTER, "A", egui::FontId::proportional(r.height() * if icon == Icon::GrowFont { 0.78 } else { 0.58 }), color);
            if icon == Icon::GrowFont {
                tri(at(0.72, 0.34), at(0.96, 0.34), at(0.84, 0.12));
            } else {
                tri(at(0.72, 0.14), at(0.96, 0.14), at(0.84, 0.36));
            }
        }
        Icon::AlignTop | Icon::AlignMiddle | Icon::AlignBottom => {
            let y = match icon {
                Icon::AlignTop => 0.12,
                Icon::AlignMiddle => 0.5,
                _ => 0.88,
            };
            line(0.1, y, 0.9, y);
            let (a, b) = match icon {
                Icon::AlignTop => (0.26, 0.72),
                Icon::AlignMiddle => (0.24, 0.76),
                _ => (0.28, 0.74),
            };
            line(0.5, a, 0.5, b);
            match icon {
                Icon::AlignTop => tri(at(0.34, 0.4), at(0.66, 0.4), at(0.5, 0.2)),
                Icon::AlignBottom => tri(at(0.34, 0.6), at(0.66, 0.6), at(0.5, 0.8)),
                _ => {
                    tri(at(0.38, 0.14), at(0.62, 0.14), at(0.5, 0.36));
                    tri(at(0.38, 0.86), at(0.62, 0.86), at(0.5, 0.64))
                }
            };
        }
        Icon::Merge => {
            p.rect_stroke(Rect::from_min_max(at(0.08, 0.2), at(0.92, 0.8)), 1.0, s, egui::StrokeKind::Middle);
            line(0.2, 0.5, 0.42, 0.5);
            line(0.58, 0.5, 0.8, 0.5);
            tri(at(0.44, 0.5), at(0.34, 0.4), at(0.34, 0.6));
            tri(at(0.56, 0.5), at(0.66, 0.4), at(0.66, 0.6));
        }
        Icon::Currency => {
            p.text(at(0.5, 0.52), egui::Align2::CENTER_CENTER, "€", egui::FontId::proportional(r.height() * 0.82), color);
        }
        Icon::Thousands => {
            p.text(at(0.5, 0.5), egui::Align2::CENTER_CENTER, "000", egui::FontId::proportional(r.height() * 0.46), color);
        }
        Icon::Percent => {
            p.text(at(0.5, 0.52), egui::Align2::CENTER_CENTER, "%", egui::FontId::proportional(r.height() * 0.78), color);
        }
        Icon::Decimals | Icon::FewerDecimals => {
            let f = egui::FontId::proportional(r.height() * 0.4);
            let (t, arrow_left) = if icon == Icon::Decimals { (".00", true) } else { (".0", false) };
            p.text(at(0.62, 0.34), egui::Align2::CENTER_CENTER, t, f, color);
            line(0.2, 0.76, 0.8, 0.76);
            if arrow_left {
                tri(at(0.12, 0.76), at(0.3, 0.64), at(0.3, 0.88));
            } else {
                tri(at(0.88, 0.76), at(0.7, 0.64), at(0.7, 0.88));
            }
        }
        Icon::Calendar => {
            p.rect_stroke(Rect::from_min_max(at(0.12, 0.2), at(0.88, 0.88)), 1.5, s, egui::StrokeKind::Middle);
            p.rect_filled(Rect::from_min_max(at(0.12, 0.2), at(0.88, 0.38)), 1.5, color);
            line(0.32, 0.08, 0.32, 0.26);
            line(0.68, 0.08, 0.68, 0.26);
            for (x, y) in [(0.3, 0.54), (0.5, 0.54), (0.7, 0.54), (0.3, 0.72), (0.5, 0.72)] {
                p.circle_filled(at(x, y), r.width() * 0.045, color);
            }
        }
        Icon::CellIndent | Icon::CellOutdent => {
            line(0.46, 0.3, 0.88, 0.3);
            line(0.46, 0.5, 0.88, 0.5);
            line(0.46, 0.7, 0.88, 0.7);
            line(0.1, 0.12, 0.1, 0.88);
            if icon == Icon::CellIndent {
                tri(at(0.18, 0.36), at(0.36, 0.5), at(0.18, 0.64));
            } else {
                tri(at(0.36, 0.36), at(0.18, 0.5), at(0.36, 0.64));
            }
        }
        Icon::Orientation => {
            let f = egui::FontId::proportional(r.height() * 0.44);
            p.text(at(0.32, 0.64), egui::Align2::CENTER_CENTER, "ab", f, color);
            line(0.34, 0.9, 0.86, 0.2);
            tri(at(0.9, 0.12), at(0.74, 0.22), at(0.86, 0.32));
        }
        Icon::FillDown => {
            p.rect_stroke(Rect::from_min_max(at(0.14, 0.62), at(0.86, 0.9)), 1.0, s, egui::StrokeKind::Middle);
            line(0.5, 0.08, 0.5, 0.42);
            tri(at(0.34, 0.4), at(0.66, 0.4), at(0.5, 0.58));
        }
        Icon::Eraser => {
            let pts = vec![at(0.1, 0.6), at(0.5, 0.2), at(0.9, 0.56), at(0.54, 0.92), at(0.4, 0.92)];
            p.add(Shape::closed_line(pts, s));
            line(0.3, 0.4, 0.7, 0.76);
            line(0.54, 0.92, 0.92, 0.92);
        }
        Icon::Pen => {
            // A pen nib, point down-left, over a stroke.
            let pts = vec![at(0.12, 0.88), at(0.2, 0.6), at(0.66, 0.14), at(0.86, 0.34), at(0.4, 0.8)];
            p.add(Shape::closed_line(pts, s));
            line(0.12, 0.88, 0.32, 0.68);
            line(0.56, 0.24, 0.76, 0.44);
        }
        Icon::Pencil => {
            let pts = vec![at(0.24, 0.62), at(0.7, 0.16), at(0.86, 0.32), at(0.4, 0.78)];
            p.add(Shape::closed_line(pts, s));
            tri(at(0.24, 0.62), at(0.4, 0.78), at(0.1, 0.92));
            line(0.62, 0.24, 0.78, 0.4);
        }
        Icon::Lasso => {
            let c = at(0.52, 0.4);
            let n = 24;
            let pts: Vec<Pos2> = (0..n).map(|i| {
                let t = i as f32 / n as f32 * std::f32::consts::TAU;
                c + vec2(t.cos() * r.width() * 0.38, t.sin() * r.height() * 0.26)
            }).collect();
            let dashes: Vec<Shape> = pts.chunks(2).map(|w| Shape::line_segment([w[0], w[1]], s)).collect();
            p.extend(dashes);
            let tail = vec![at(0.3, 0.62), at(0.22, 0.78), at(0.34, 0.92)];
            p.add(Shape::line(tail, s));
        }
        Icon::Function => {
            p.text(at(0.5, 0.5), egui::Align2::CENTER_CENTER, "fx", egui::FontId::proportional(r.height() * 0.72), color);
        }
        Icon::Sigma => {
            p.text(at(0.5, 0.52), egui::Align2::CENTER_CENTER, "Σ", egui::FontId::proportional(r.height() * 0.86), color);
        }
        Icon::Pilcrow => {
            p.circle_filled(at(0.42, 0.32), r.width() * 0.2, color);
            line(0.5, 0.12, 0.5, 0.9);
            line(0.72, 0.12, 0.72, 0.9);
            line(0.42, 0.12, 0.84, 0.12);
        }
        Icon::LineSpacing => {
            for y in [0.24, 0.5, 0.76] {
                line(0.42, y, 0.9, y);
            }
            line(0.2, 0.18, 0.2, 0.82);
            tri(at(0.08, 0.28), at(0.32, 0.28), at(0.2, 0.08));
            tri(at(0.08, 0.72), at(0.32, 0.72), at(0.2, 0.92));
        }
        Icon::Shading => {
            p.rect_filled(Rect::from_min_max(at(0.1, 0.18), at(0.9, 0.82)), 1.0, color.gamma_multiply(0.35));
            for y in [0.34, 0.5, 0.66] {
                line(0.22, y, 0.78, y);
            }
        }
        Icon::NoList => {
            for i in 0..3 {
                let y = 0.25 + i as f32 * 0.25;
                p.circle_stroke(at(0.18, y), r.width() * 0.06, Stroke::new(s.width * 0.7, color));
                line(0.36, y, 0.88, y);
            }
            p.line_segment([at(0.08, 0.92), at(0.92, 0.08)], Stroke::new(s.width, Color32::from_rgb(0xD0, 0x30, 0x30)));
        }
        Icon::Filter => {
            let pts = vec![at(0.1, 0.16), at(0.9, 0.16), at(0.58, 0.52), at(0.58, 0.86), at(0.42, 0.76), at(0.42, 0.52)];
            p.add(Shape::closed_line(pts, s));
        }
        Icon::CondFormat => {
            let cell = |x: f32, y: f32, c: Color32| p.rect_filled(Rect::from_min_max(at(x, y), at(x + 0.36, y + 0.3)), 1.0, c);
            cell(0.08, 0.12, Color32::from_rgb(0xF8, 0x69, 0x6B));
            cell(0.52, 0.12, Color32::from_rgb(0xFF, 0xEB, 0x84));
            cell(0.08, 0.56, Color32::from_rgb(0x63, 0xBE, 0x7B));
            p.rect_filled(Rect::from_min_max(at(0.52, 0.6), at(0.92, 0.82)), 1.0, color.gamma_multiply(0.5));
        }
        Icon::Validation => {
            p.rect_stroke(Rect::from_min_max(at(0.1, 0.14), at(0.9, 0.86)), 1.0, s, egui::StrokeKind::Inside);
            p.add(Shape::line(vec![at(0.28, 0.5), at(0.44, 0.66), at(0.74, 0.34)], Stroke::new(s.width * 1.2, Color32::from_rgb(0x2E, 0x9D, 0x4F))));
        }
        Icon::Table => {
            p.rect_filled(Rect::from_min_max(at(0.12, 0.18), at(0.88, 0.4)), 0.0, color.gamma_multiply(0.35));
            p.rect_stroke(Rect::from_min_max(at(0.12, 0.18), at(0.88, 0.82)), 1.0, s, egui::StrokeKind::Middle);
            for f in [0.4, 0.61] {
                line(0.12, f, 0.88, f);
            }
            for f in [0.37, 0.63] {
                line(f, 0.18, f, 0.82);
            }
        }
        Icon::Folder => {
            let pts = vec![at(0.08, 0.22), at(0.38, 0.22), at(0.46, 0.32), at(0.92, 0.32), at(0.92, 0.82), at(0.08, 0.82)];
            p.add(Shape::closed_line(pts, s));
            line(0.08, 0.42, 0.92, 0.42);
        }
        Icon::NavPane => {
            p.rect_stroke(Rect::from_min_max(at(0.1, 0.16), at(0.9, 0.84)), 1.0, s, egui::StrokeKind::Middle);
            p.rect_filled(Rect::from_min_max(at(0.1, 0.16), at(0.38, 0.84)), 0.0, color.gamma_multiply(0.35));
            line(0.38, 0.16, 0.38, 0.84);
            for (x, y) in [(0.16, 0.32), (0.2, 0.48), (0.2, 0.64)] {
                line(x, y, 0.32, y);
            }
        }
        // --- charts ---
        Icon::Chart | Icon::ChartColumn => {
            axes();
            for (x, top) in [(0.2, 0.5), (0.43, 0.24), (0.66, 0.6)] {
                fill(x, top, x + 0.16, 0.88, accent);
            }
        }
        Icon::ChartBar => {
            axes();
            for (y, right) in [(0.18, 0.6), (0.42, 0.86), (0.66, 0.44)] {
                fill(0.12, y, right, y + 0.16, accent);
            }
        }
        Icon::ChartLine => {
            axes();
            path(&[(0.2, 0.7), (0.4, 0.42), (0.6, 0.58), (0.86, 0.2)], Stroke::new(s.width * 1.1, accent));
        }
        Icon::ChartArea => {
            let top = [(0.12, 0.62), (0.36, 0.36), (0.58, 0.52), (0.92, 0.2)];
            for k in top.windows(2) {
                solid(&[k[0], k[1], (k[1].0, 0.88), (k[0].0, 0.88)], mid);
            }
            path(&top, acc);
            axes();
        }
        Icon::ChartPie => {
            p.circle_stroke(at(0.5, 0.52), w * 0.38, s);
            let mut v = vec![at(0.5, 0.52)];
            v.extend(ellipse(0.5, 0.52, 0.38, 0.38, -FRAC_PI_2, PI / 6.0));
            p.add(Shape::convex_polygon(v, accent, Stroke::NONE));
            seg(0.5, 0.52, 0.5 + 0.38 * (5.0 * PI / 6.0).cos(), 0.52 + 0.38 * (5.0 * PI / 6.0).sin(), s);
        }
        Icon::ChartDoughnut => {
            let ring = w * 0.15;
            p.circle_stroke(at(0.5, 0.5), w * 0.3, Stroke::new(ring, color.gamma_multiply(0.35)));
            p.add(Shape::line(ellipse(0.5, 0.5, 0.3, 0.3, -FRAC_PI_2, PI / 4.0), Stroke::new(ring, accent)));
        }
        Icon::ChartScatter => {
            axes();
            for (x, y) in [(0.3, 0.68), (0.42, 0.5), (0.56, 0.6), (0.66, 0.36), (0.82, 0.24)] {
                p.circle_filled(at(x, y), w * 0.06, accent);
            }
        }
        Icon::ChartCombo => {
            axes();
            for (x, top) in [(0.2, 0.5), (0.43, 0.36), (0.66, 0.58)] {
                fill(x, top, x + 0.16, 0.88, mid);
            }
            path(&[(0.22, 0.58), (0.51, 0.2), (0.8, 0.36)], s);
        }
        Icon::ChartWaterfall => {
            axes();
            fill(0.2, 0.52, 0.34, 0.88, accent);
            fill(0.38, 0.28, 0.52, 0.52, accent);
            fill(0.56, 0.28, 0.7, 0.44, color.gamma_multiply(0.45));
            fill(0.74, 0.44, 0.88, 0.88, accent);
        }
        Icon::ChartHistogram => {
            axes();
            for (i, top) in [0.62, 0.36, 0.2, 0.42, 0.66].into_iter().enumerate() {
                let x = 0.17 + i as f32 * 0.146;
                fill(x, top, x + 0.13, 0.88, accent);
            }
        }
        Icon::ChartRadar => {
            let spoke = |k: usize, r: f32| {
                let a = -FRAC_PI_2 + k as f32 * TAU / 5.0;
                (0.5 + r * a.cos(), 0.54 + r * a.sin())
            };
            let web: Vec<(f32, f32)> = (0..=5).map(|k| spoke(k % 5, 0.4)).collect();
            path(&web, s);
            for k in 0..5 {
                let (x, y) = spoke(k, 0.4);
                seg(0.5, 0.54, x, y, thin);
            }
            let inner: Vec<(f32, f32)> = [0.3, 0.2, 0.36, 0.14, 0.26, 0.3].iter().enumerate().map(|(k, r)| spoke(k % 5, *r)).collect();
            path(&inner, Stroke::new(s.width * 1.1, accent));
        }
        Icon::ChartBubble => {
            axes();
            for (x, y, r) in [(0.32, 0.64, 0.08), (0.5, 0.4, 0.14), (0.76, 0.26, 0.1)] {
                p.circle_filled(at(x, y), w * r, accent.gamma_multiply(0.8));
            }
        }
        Icon::ChartStock => {
            axes();
            for (x, hi, lo, close) in [(0.28, 0.3, 0.7, 0.4), (0.5, 0.2, 0.56, 0.5), (0.72, 0.34, 0.76, 0.46)] {
                seg(x, hi, x, lo, Stroke::new(s.width, accent));
                seg(x, close, x + 0.08, close, Stroke::new(s.width, accent));
            }
        }
        Icon::ChartSurface => {
            let row = |y: f32, d: f32| vec![(0.1, y + d), (0.3, y - 0.14 + d), (0.5, y + 0.02 + d), (0.7, y - 0.18 + d), (0.9, y - 0.02 + d)];
            for k in 0..3 {
                path(&row(0.5 + k as f32 * 0.14, 0.0), if k == 1 { Stroke::new(s.width, accent) } else { s });
            }
            for x in [0.1, 0.5, 0.9] {
                seg(x, 0.36, x, 0.8, thin);
            }
        }
        Icon::ChartTreemap => {
            fill(0.1, 0.16, 0.56, 0.84, accent);
            fill(0.6, 0.16, 0.9, 0.48, mid);
            fill(0.6, 0.52, 0.74, 0.84, soft);
            fill(0.78, 0.52, 0.9, 0.84, mid);
        }
        Icon::ChartSunburst => {
            p.circle_stroke(at(0.5, 0.5), w * 0.33, Stroke::new(w * 0.14, color.gamma_multiply(0.35)));
            p.add(Shape::line(ellipse(0.5, 0.5, 0.33, 0.33, -FRAC_PI_2, 0.2), Stroke::new(w * 0.14, accent)));
            p.circle_stroke(at(0.5, 0.5), w * 0.16, Stroke::new(w * 0.1, mid));
        }
        Icon::ChartBox => {
            axes();
            for (x, lo, q1, q3, hi) in [(0.3, 0.8, 0.62, 0.42, 0.26), (0.62, 0.7, 0.5, 0.3, 0.16)] {
                seg(x + 0.08, hi, x + 0.08, lo, s);
                fill(x, q3, x + 0.16, q1, accent);
                seg(x, (q1 + q3) / 2.0, x + 0.16, (q1 + q3) / 2.0, Stroke::new(s.width, color));
            }
        }
        Icon::ChartFunnel => {
            for (k, half) in [0.4f32, 0.3, 0.2, 0.1].into_iter().enumerate() {
                let y = 0.16 + k as f32 * 0.18;
                fill(0.5 - half, y, 0.5 + half, y + 0.14, if k % 2 == 0 { accent } else { mid });
            }
        }
        Icon::ChartPareto => {
            axes();
            for (i, top) in [0.24, 0.46, 0.62, 0.74].into_iter().enumerate() {
                let x = 0.18 + i as f32 * 0.18;
                fill(x, top, x + 0.14, 0.88, mid);
            }
            path(&[(0.25, 0.46), (0.43, 0.26), (0.61, 0.18), (0.79, 0.14)], Stroke::new(s.width * 1.1, accent));
        }
        Icon::ChartMekko => {
            axes();
            for (x0, x1, split) in [(0.16, 0.5, 0.5), (0.5, 0.72, 0.36), (0.72, 0.88, 0.62)] {
                fill(x0, 0.2, x1, split, mid);
                fill(x0, split, x1, 0.88, accent);
            }
        }
        Icon::ChartTimeline => {
            seg(0.08, 0.5, 0.92, 0.5, s);
            for (x, up) in [(0.22, true), (0.5, false), (0.78, true)] {
                let y = if up { 0.24 } else { 0.76 };
                seg(x, 0.5, x, y, thin);
                fill(x - 0.09, if up { y - 0.1 } else { y }, x + 0.09, if up { y } else { y + 0.1 }, mid);
                p.circle_filled(at(x, 0.5), w * 0.06, accent);
            }
        }
        Icon::Sparkline => {
            frame(0.06, 0.24, 0.94, 0.76, s);
            path(&[(0.18, 0.6), (0.32, 0.42), (0.46, 0.54), (0.6, 0.36), (0.72, 0.5), (0.84, 0.38)], acc);
        }
        // --- insert ---
        Icon::Picture => {
            solid(&[(0.14, 0.78), (0.4, 0.46), (0.66, 0.78)], mid);
            solid(&[(0.48, 0.78), (0.66, 0.56), (0.86, 0.78)], mid);
            path(&[(0.14, 0.78), (0.4, 0.46), (0.57, 0.67), (0.66, 0.56), (0.86, 0.78)], thin);
            p.circle_filled(at(0.34, 0.36), w * 0.08, accent);
            frame(0.1, 0.18, 0.9, 0.82, s);
        }
        Icon::PictureInCell => {
            // A grid with a small picture in its middle cell.
            frame(0.08, 0.14, 0.92, 0.86, thin);
            seg(0.08, 0.38, 0.92, 0.38, thin);
            seg(0.08, 0.62, 0.92, 0.62, thin);
            seg(0.3, 0.14, 0.3, 0.86, thin);
            seg(0.7, 0.14, 0.7, 0.86, thin);
            solid(&[(0.34, 0.6), (0.46, 0.46), (0.56, 0.6)], mid);
            solid(&[(0.48, 0.6), (0.58, 0.5), (0.67, 0.6)], mid);
            p.circle_filled(at(0.4, 0.44), w * 0.035, accent);
            frame(0.3, 0.38, 0.7, 0.62, s);
        }
        Icon::Checkbox => {
            p.rect_filled(rc(0.18, 0.18, 0.82, 0.82), w * 0.08, accent);
            p.add(Shape::line(vec![at(0.3, 0.5), at(0.44, 0.65), at(0.7, 0.36)], Stroke::new(s.width * 1.2, Color32::WHITE)));
        }
        Icon::Shapes => {
            p.circle_stroke(at(0.3, 0.3), w * 0.19, s);
            closed(&[(0.72, 0.1), (0.92, 0.48), (0.52, 0.48)], s);
            fill(0.3, 0.56, 0.72, 0.9, soft);
            frame(0.3, 0.56, 0.72, 0.9, s);
        }
        Icon::TextBox => {
            frame(0.08, 0.16, 0.92, 0.84, s);
            text(0.32, 0.5, "A", 0.52, accent);
            line(0.56, 0.38, 0.8, 0.38);
            line(0.56, 0.62, 0.8, 0.62);
        }
        Icon::Pivot => {
            fill(0.1, 0.1, 0.9, 0.3, soft);
            fill(0.1, 0.3, 0.3, 0.9, soft);
            seg(0.1, 0.3, 0.9, 0.3, thin);
            seg(0.3, 0.1, 0.3, 0.9, thin);
            frame(0.1, 0.1, 0.9, 0.9, s);
            p.add(Shape::line(ellipse(0.44, 0.44, 0.3, 0.3, 0.0, FRAC_PI_2), acc));
            solid(&[(0.74, 0.36), (0.64, 0.5), (0.84, 0.5)], accent);
            solid(&[(0.36, 0.74), (0.5, 0.64), (0.5, 0.84)], accent);
        }
        Icon::Link => {
            capsule(0.36, 0.64, 0.24, 0.13, -PI / 4.0, s);
            capsule(0.64, 0.36, 0.24, 0.13, -PI / 4.0, acc);
        }
        Icon::Note => {
            let v = [(0.14, 0.14), (0.86, 0.14), (0.86, 0.64), (0.64, 0.86), (0.14, 0.86)];
            solid(&v, Color32::from_rgb(0xFF, 0xD2, 0x4D));
            closed(&v, s);
            path(&[(0.64, 0.86), (0.64, 0.64), (0.86, 0.64)], s);
            let ink = Stroke::new(thin.width, Color32::from_rgb(0x80, 0x5A, 0x00));
            seg(0.28, 0.36, 0.72, 0.36, ink);
            seg(0.28, 0.52, 0.54, 0.52, ink);
        }
        Icon::Comment => {
            let v = [(0.1, 0.14), (0.9, 0.14), (0.9, 0.68), (0.5, 0.68), (0.28, 0.88), (0.3, 0.68), (0.1, 0.68)];
            fill(0.1, 0.14, 0.9, 0.68, soft);
            solid(&[(0.3, 0.68), (0.5, 0.68), (0.28, 0.88)], soft);
            closed(&v, s);
            seg(0.26, 0.34, 0.74, 0.34, thin);
            seg(0.26, 0.5, 0.6, 0.5, thin);
        }
        Icon::Symbol => text(0.5, 0.52, "Ω", 0.84, color),
        Icon::Equation => text(0.5, 0.48, "π", 0.9, color),
        Icon::Object => {
            frame(0.06, 0.14, 0.94, 0.86, thin);
            page(0.3, 0.24, 0.7, 0.76, s);
            seg(0.38, 0.44, 0.62, 0.44, thin);
            seg(0.38, 0.56, 0.62, 0.56, thin);
            p.circle_filled(at(0.8, 0.26), w * 0.06, accent);
        }
        Icon::QrCode => {
            for (x, y) in [(0.1, 0.1), (0.6, 0.1), (0.1, 0.6)] {
                frame(x, y, x + 0.3, y + 0.3, thin);
                fill(x + 0.09, y + 0.09, x + 0.21, y + 0.21, color);
            }
            for (x, y) in [(0.6, 0.6), (0.78, 0.6), (0.69, 0.7), (0.6, 0.8), (0.8, 0.8), (0.45, 0.45), (0.45, 0.2)] {
                fill(x, y, x + 0.1, y + 0.1, accent);
            }
        }
        Icon::HeaderFooter => {
            fill(0.18, 0.06, 0.82, 0.26, mid);
            fill(0.18, 0.74, 0.82, 0.94, mid);
            frame(0.18, 0.06, 0.82, 0.94, s);
            seg(0.3, 0.42, 0.7, 0.42, thin);
            seg(0.3, 0.58, 0.7, 0.58, thin);
        }
        Icon::PageNumber => {
            page(0.18, 0.06, 0.82, 0.94, s);
            text(0.5, 0.62, "#", 0.5, accent);
        }
        Icon::PageBreak => {
            path(&[(0.2, 0.38), (0.2, 0.08), (0.8, 0.08), (0.8, 0.38)], s);
            path(&[(0.2, 0.62), (0.2, 0.92), (0.8, 0.92), (0.8, 0.62)], s);
            seg(0.32, 0.24, 0.68, 0.24, thin);
            seg(0.32, 0.76, 0.68, 0.76, thin);
            dashed(0.06, 0.5, 0.94, 0.5, acc);
        }
        Icon::Thesaurus => {
            // A book with two linked words.
            fill(0.18, 0.12, 0.82, 0.88, soft);
            path(&[(0.18, 0.12), (0.82, 0.12), (0.82, 0.88), (0.18, 0.88), (0.18, 0.12)], s);
            seg(0.3, 0.35, 0.7, 0.35, thin);
            seg(0.3, 0.65, 0.7, 0.65, thin);
            seg(0.5, 0.4, 0.5, 0.6, acc);
        }
        Icon::Control => {
            // A form field: a box with a title tab.
            fill(0.1, 0.4, 0.9, 0.78, soft);
            path(&[(0.1, 0.4), (0.9, 0.4), (0.9, 0.78), (0.1, 0.78), (0.1, 0.4)], s);
            fill(0.1, 0.22, 0.45, 0.4, accent);
            seg(0.2, 0.59, 0.6, 0.59, thin);
        }
        Icon::Hyphen => {
            // Text lines, the first ending in a hyphen.
            seg(0.1, 0.25, 0.62, 0.25, s);
            seg(0.72, 0.25, 0.9, 0.25, acc);
            seg(0.1, 0.5, 0.9, 0.5, thin);
            seg(0.1, 0.75, 0.6, 0.75, thin);
        }
        Icon::Brightness => {
            // A sun.
            p.circle_filled(at(0.5, 0.5), w * 0.2, soft);
            p.circle_stroke(at(0.5, 0.5), w * 0.2, s);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let (sn, cs) = a.sin_cos();
                seg(0.5 + cs * 0.3, 0.5 + sn * 0.3, 0.5 + cs * 0.42, 0.5 + sn * 0.42, acc);
            }
        }
        Icon::Recolor => {
            // A picture half in color, half gray.
            fill(0.1, 0.18, 0.5, 0.82, accent);
            fill(0.5, 0.18, 0.9, 0.82, soft);
            frame(0.1, 0.18, 0.9, 0.82, s);
        }
        Icon::Transparency => {
            // A checkerboard.
            for (x, y) in [(0.1, 0.18), (0.5, 0.5), (0.3, 0.34), (0.7, 0.66), (0.1, 0.5), (0.5, 0.18), (0.3, 0.66), (0.7, 0.34)] {
                fill(x, y, x + 0.2, y + 0.16, soft);
            }
            frame(0.1, 0.18, 0.9, 0.82, s);
        }
        Icon::PicStyles => {
            // A framed picture with a shadow.
            fill(0.2, 0.26, 0.94, 0.9, soft);
            fill(0.08, 0.12, 0.8, 0.76, accent);
            frame(0.08, 0.12, 0.8, 0.76, s);
            path(&[(0.14, 0.7), (0.34, 0.46), (0.5, 0.6), (0.62, 0.5), (0.74, 0.7)], thin);
        }
        Icon::DateTime => {
            p.circle_filled(at(0.5, 0.5), w * 0.38, soft);
            p.circle_stroke(at(0.5, 0.5), w * 0.38, s);
            path(&[(0.5, 0.24), (0.5, 0.5), (0.68, 0.6)], acc);
        }
        // --- page layout ---
        Icon::Margins => {
            fill(0.3, 0.22, 0.7, 0.78, soft);
            let guide = Stroke::new(thin.width, accent);
            seg(0.3, 0.06, 0.3, 0.94, guide);
            seg(0.7, 0.06, 0.7, 0.94, guide);
            seg(0.14, 0.22, 0.86, 0.22, guide);
            seg(0.14, 0.78, 0.86, 0.78, guide);
            frame(0.14, 0.06, 0.86, 0.94, s);
        }
        Icon::PageOrientation => {
            path(&[(0.3, 0.66), (0.08, 0.66), (0.08, 0.06), (0.5, 0.06), (0.5, 0.46)], s);
            fill(0.3, 0.46, 0.94, 0.9, soft);
            frame(0.3, 0.46, 0.94, 0.9, s);
        }
        Icon::PageSize => {
            frame(0.34, 0.26, 0.9, 0.94, s);
            arrows(0.34, 0.1, 0.9, 0.1, Stroke::new(thin.width, accent));
            arrows(0.14, 0.26, 0.14, 0.94, Stroke::new(thin.width, accent));
        }
        Icon::Columns => {
            for y in [0.2, 0.4, 0.6, 0.8] {
                line(0.1, y, 0.44, y);
                line(0.56, y, if y > 0.7 { 0.76 } else { 0.9 }, y);
            }
        }
        Icon::PrintArea => {
            let faint = Stroke::new(thin.width, color.gamma_multiply(0.5));
            for f in [0.36, 0.64] {
                seg(f, 0.08, f, 0.92, faint);
                seg(0.08, f, 0.92, f, faint);
            }
            frame(0.08, 0.08, 0.92, 0.92, faint);
            fill(0.08, 0.08, 0.64, 0.64, soft);
            for (x0, y0, x1, y1) in [(0.08, 0.08, 0.64, 0.08), (0.64, 0.08, 0.64, 0.64), (0.64, 0.64, 0.08, 0.64), (0.08, 0.64, 0.08, 0.08)] {
                dashed(x0, y0, x1, y1, acc);
            }
        }
        Icon::PrintTitles => {
            path(&[(0.4, 0.7), (0.06, 0.7), (0.06, 0.06), (0.56, 0.06), (0.56, 0.3)], s);
            fill(0.06, 0.06, 0.56, 0.2, mid);
            fill(0.4, 0.3, 0.94, 0.46, mid);
            frame(0.4, 0.3, 0.94, 0.94, s);
            seg(0.5, 0.62, 0.84, 0.62, thin);
            seg(0.5, 0.78, 0.84, 0.78, thin);
        }
        Icon::Watermark => {
            page(0.16, 0.06, 0.84, 0.94, s);
            text(0.48, 0.58, "A", 0.62, accent.gamma_multiply(0.45));
        }
        Icon::PageColor => {
            page_fill(0.06, 0.06, 0.56, 0.8, mid);
            page(0.06, 0.06, 0.56, 0.8, s);
            closed(&[(0.56, 0.66), (0.72, 0.5), (0.88, 0.66), (0.72, 0.82)], s);
            p.circle_filled(at(0.9, 0.86), w * 0.06, accent);
        }
        Icon::PageBorders => {
            frame(0.14, 0.06, 0.86, 0.94, s);
            frame(0.26, 0.18, 0.74, 0.82, acc);
        }
        Icon::Themes => {
            text(0.5, 0.36, "Aa", 0.52, color);
            fill(0.1, 0.7, 0.34, 0.92, accent);
            fill(0.38, 0.7, 0.62, 0.92, Color32::from_rgb(0xED, 0x7D, 0x31));
            fill(0.66, 0.7, 0.9, 0.92, Color32::from_rgb(0x70, 0xAD, 0x47));
        }
        Icon::Styles => {
            path(&[(0.22, 0.22), (0.22, 0.08), (0.92, 0.08), (0.92, 0.76), (0.78, 0.76)], thin);
            fill(0.08, 0.22, 0.78, 0.92, soft);
            frame(0.08, 0.22, 0.78, 0.92, s);
            text(0.43, 0.57, "Aa", 0.42, color);
        }
        Icon::CellStyles => {
            fill(0.08, 0.14, 0.5, 0.5, accent);
            fill(0.5, 0.14, 0.92, 0.5, Color32::from_rgb(0xED, 0x7D, 0x31).gamma_multiply(0.7));
            fill(0.5, 0.5, 0.92, 0.86, Color32::from_rgb(0x70, 0xAD, 0x47).gamma_multiply(0.7));
            seg(0.08, 0.5, 0.92, 0.5, thin);
            seg(0.5, 0.14, 0.5, 0.86, thin);
            frame(0.08, 0.14, 0.92, 0.86, s);
        }
        Icon::FormatAsTable => {
            fill(0.08, 0.14, 0.92, 0.32, accent);
            fill(0.08, 0.5, 0.92, 0.68, soft);
            for x in [0.36, 0.64] {
                seg(x, 0.32, x, 0.86, thin);
            }
            frame(0.08, 0.14, 0.92, 0.86, s);
        }
        // --- review and references ---
        Icon::Spelling => {
            text(0.5, 0.28, "ABC", 0.36, color);
            path(&[(0.26, 0.64), (0.42, 0.82), (0.78, 0.46)], Stroke::new(s.width * 1.2, accent));
        }
        Icon::ReadAloud => {
            // A speaker and its sound waves.
            closed(&[(0.1, 0.38), (0.26, 0.38), (0.48, 0.18), (0.48, 0.82), (0.26, 0.62), (0.1, 0.62)], s);
            for rad in [0.16, 0.32] {
                p.add(Shape::line(ellipse(0.52, 0.5, rad, rad, -0.9, 0.9), acc));
            }
        }
        Icon::SpeakPlay => solid(&[(0.28, 0.16), (0.82, 0.5), (0.28, 0.84)], color),
        Icon::SpeakPause => {
            fill(0.24, 0.18, 0.42, 0.82, color);
            fill(0.58, 0.18, 0.76, 0.82, color);
        }
        Icon::SpeakStop => fill(0.22, 0.22, 0.78, 0.78, color),
        Icon::SpeakPrev => {
            fill(0.18, 0.2, 0.3, 0.8, color);
            solid(&[(0.82, 0.2), (0.34, 0.5), (0.82, 0.8)], color);
        }
        Icon::SpeakNext => {
            fill(0.7, 0.2, 0.82, 0.8, color);
            solid(&[(0.18, 0.2), (0.66, 0.5), (0.18, 0.8)], color);
        }
        Icon::Video => {
            frame(0.08, 0.2, 0.92, 0.8, s);
            for x in [0.16, 0.8] {
                for y in [0.3, 0.5, 0.7] {
                    fill(x, y - 0.04, x + 0.05, y + 0.04, color);
                }
            }
            solid(&[(0.4, 0.34), (0.64, 0.5), (0.4, 0.66)], accent);
        }
        Icon::Audio => {
            closed(&[(0.08, 0.4), (0.22, 0.4), (0.42, 0.22), (0.42, 0.78), (0.22, 0.6), (0.08, 0.6)], s);
            seg(0.72, 0.2, 0.72, 0.66, s);
            seg(0.72, 0.2, 0.9, 0.28, s);
            p.circle_filled(at(0.66, 0.7), w * 0.07, accent);
        }
        Icon::Protect => {
            let mut v = vec![at(0.3, 0.48)];
            v.extend(ellipse(0.5, 0.34, 0.2, 0.2, PI, TAU));
            v.push(at(0.7, 0.48));
            p.add(Shape::line(v, s));
            fill(0.2, 0.48, 0.8, 0.9, mid);
            frame(0.2, 0.48, 0.8, 0.9, s);
            p.circle_filled(at(0.5, 0.64), w * 0.06, color);
            line(0.5, 0.64, 0.5, 0.78);
        }
        Icon::TrackChanges => {
            page(0.1, 0.06, 0.68, 0.9, s);
            seg(0.22, 0.42, 0.56, 0.42, thin);
            seg(0.22, 0.58, 0.44, 0.58, thin);
            pencil(0.92, 0.36, 0.52, 0.92);
        }
        Icon::Accept | Icon::Reject => {
            page(0.1, 0.06, 0.68, 0.9, s);
            seg(0.22, 0.42, 0.56, 0.42, thin);
            seg(0.22, 0.58, 0.44, 0.58, thin);
            if icon == Icon::Accept {
                path(&[(0.5, 0.72), (0.64, 0.86), (0.92, 0.52)], Stroke::new(s.width * 1.4, green));
            } else {
                cross(0.76, 0.72, 0.14, Stroke::new(s.width * 1.4, red));
            }
        }
        Icon::Compare => {
            path(&[(0.44, 0.74), (0.08, 0.74), (0.08, 0.08), (0.56, 0.08), (0.56, 0.26)], s);
            seg(0.18, 0.24, 0.46, 0.24, thin);
            seg(0.18, 0.4, 0.34, 0.4, thin);
            fill(0.44, 0.26, 0.92, 0.92, soft);
            frame(0.44, 0.26, 0.92, 0.92, s);
            seg(0.54, 0.46, 0.82, 0.46, thin);
            seg(0.54, 0.62, 0.82, 0.62, thin);
            seg(0.54, 0.78, 0.72, 0.78, thin);
        }
        Icon::Toc => {
            for (i, (x, y)) in [(0.08, 0.2), (0.2, 0.4), (0.2, 0.6), (0.08, 0.8)].into_iter().enumerate() {
                seg(x, y, 0.44, y, if i == 0 { acc } else { s });
                for dx in [0.54, 0.64, 0.74] {
                    p.circle_filled(at(dx, y), w * 0.035, color);
                }
                line(0.84, y, 0.92, y);
            }
        }
        Icon::Footnote => {
            text(0.36, 0.36, "ab", 0.56, color);
            text(0.8, 0.2, "1", 0.34, accent);
            seg(0.1, 0.72, 0.46, 0.72, thin);
            line(0.1, 0.88, 0.9, 0.88);
        }
        Icon::Caption => {
            solid(&[(0.2, 0.52), (0.42, 0.28), (0.64, 0.52)], mid);
            p.circle_filled(at(0.68, 0.26), w * 0.06, accent);
            frame(0.12, 0.08, 0.88, 0.56, s);
            line(0.12, 0.74, 0.88, 0.74);
            line(0.12, 0.9, 0.6, 0.9);
        }
        Icon::Bookmark => {
            solid(&[(0.26, 0.08), (0.5, 0.08), (0.5, 0.68), (0.26, 0.9)], mid);
            solid(&[(0.5, 0.08), (0.74, 0.08), (0.74, 0.9), (0.5, 0.68)], mid);
            closed(&[(0.26, 0.08), (0.74, 0.08), (0.74, 0.9), (0.5, 0.68), (0.26, 0.9)], s);
        }
        Icon::Book => {
            fill(0.2, 0.08, 0.82, 0.92, soft);
            frame(0.2, 0.08, 0.82, 0.92, s);
            seg(0.32, 0.08, 0.32, 0.92, s);
            seg(0.44, 0.3, 0.7, 0.3, thin);
            seg(0.44, 0.42, 0.66, 0.42, thin);
            solid(&[(0.6, 0.08), (0.72, 0.08), (0.72, 0.62), (0.66, 0.54), (0.6, 0.62)], accent);
        }
        Icon::Chapter => {
            page(0.14, 0.06, 0.74, 0.94, s);
            seg(0.26, 0.26, 0.6, 0.26, Stroke::new(s.width * 2.0, accent));
            seg(0.26, 0.46, 0.62, 0.46, thin);
            seg(0.26, 0.6, 0.62, 0.6, thin);
            seg(0.26, 0.74, 0.5, 0.74, thin);
        }
        Icon::Scene => {
            seg(0.1, 0.22, 0.9, 0.22, thin);
            seg(0.1, 0.34, 0.7, 0.34, thin);
            for x in [0.3, 0.5, 0.7] {
                p.circle_filled(at(x, 0.52), w * 0.05, accent);
            }
            seg(0.1, 0.7, 0.9, 0.7, thin);
            seg(0.1, 0.82, 0.6, 0.82, thin);
        }
        Icon::Cards => {
            for (x, y) in [(0.08, 0.12), (0.54, 0.12), (0.08, 0.56), (0.54, 0.56)] {
                frame(x, y, x + 0.38, y + 0.32, s);
                fill(x, y, x + 0.38, y + 0.07, accent);
            }
        }
        Icon::CrossRef => {
            page(0.08, 0.06, 0.6, 0.92, s);
            seg(0.18, 0.3, 0.38, 0.3, thin);
            seg(0.18, 0.51, 0.46, 0.51, thin);
            seg(0.18, 0.72, 0.38, 0.72, thin);
            path(&[(0.44, 0.3), (0.86, 0.3), (0.86, 0.72), (0.56, 0.72)], acc);
            solid(&[(0.42, 0.72), (0.56, 0.62), (0.56, 0.82)], accent);
        }
        Icon::Field => text(0.5, 0.48, "{ }", 0.72, color),
        Icon::MailMerge => {
            fill(0.06, 0.08, 0.64, 0.5, soft);
            frame(0.06, 0.08, 0.64, 0.5, s);
            path(&[(0.06, 0.08), (0.35, 0.3), (0.64, 0.08)], s);
            for y in [0.68, 0.88] {
                fill(0.3, y - 0.06, 0.42, y + 0.06, accent);
                line(0.52, y, 0.94, y);
            }
        }
        Icon::Labels => {
            frame(0.12, 0.04, 0.88, 0.96, s);
            for (x0, x1) in [(0.22, 0.46), (0.54, 0.78)] {
                for (y0, y1) in [(0.14, 0.34), (0.4, 0.6), (0.66, 0.86)] {
                    p.rect_filled(rc(x0, y0, x1, y1), w * 0.03, mid);
                }
            }
        }
        Icon::Envelope => {
            fill(0.08, 0.2, 0.92, 0.8, soft);
            frame(0.08, 0.2, 0.92, 0.8, s);
            path(&[(0.08, 0.2), (0.5, 0.54), (0.92, 0.2)], s);
        }
        Icon::Multilevel => {
            for (i, x) in [0.12f32, 0.3, 0.48].iter().enumerate() {
                let y = 0.22 + i as f32 * 0.28;
                p.circle_filled(at(*x, y), r.width() * 0.055, accent);
                line(x + 0.14, y, 0.9, y);
            }
        }
        Icon::QuickAnalysis => {
            fill(0.08, 0.14, 0.72, 0.34, soft);
            frame(0.08, 0.14, 0.72, 0.78, s);
            seg(0.08, 0.34, 0.72, 0.34, thin);
            seg(0.08, 0.56, 0.5, 0.56, thin);
            seg(0.34, 0.14, 0.34, 0.78, thin);
            let bolt = [(0.7, 0.4), (0.5, 0.7), (0.64, 0.7), (0.56, 0.94), (0.86, 0.6), (0.72, 0.6), (0.82, 0.4)];
            p.add(Shape::closed_line(pts(&bolt), Stroke::new(s.width * 0.8, color)));
            solid(&[(0.7, 0.4), (0.82, 0.4), (0.72, 0.6), (0.6, 0.6)], accent);
            solid(&[(0.5, 0.7), (0.86, 0.6), (0.56, 0.94)], accent);
        }
        // --- view ---
        Icon::ZoomIn | Icon::ZoomOut | Icon::Zoom100 => {
            p.circle_filled(at(0.42, 0.42), w * 0.3, soft);
            p.circle_stroke(at(0.42, 0.42), w * 0.3, s);
            p.line_segment([at(0.64, 0.64), at(0.88, 0.88)], Stroke::new(s.width * 1.5, color));
            match icon {
                Icon::ZoomIn => plus(0.42, 0.42, 0.14, s),
                Icon::ZoomOut => seg(0.28, 0.42, 0.56, 0.42, s),
                _ => text(0.42, 0.42, "1:1", 0.26, color),
            }
        }
        Icon::Freeze => {
            fill(0.08, 0.08, 0.92, 0.34, soft);
            fill(0.08, 0.34, 0.34, 0.92, soft);
            seg(0.63, 0.08, 0.63, 0.92, thin);
            seg(0.08, 0.63, 0.92, 0.63, thin);
            seg(0.34, 0.08, 0.34, 0.92, acc);
            seg(0.08, 0.34, 0.92, 0.34, acc);
            frame(0.08, 0.08, 0.92, 0.92, s);
        }
        Icon::Gridlines => {
            let grid = Stroke::new(thin.width, accent);
            for f in [0.36, 0.64] {
                seg(f, 0.08, f, 0.92, grid);
                seg(0.08, f, 0.92, f, grid);
            }
            frame(0.08, 0.08, 0.92, 0.92, s);
        }
        Icon::ShowFormulas => {
            path(&[(0.24, 0.14), (0.1, 0.14), (0.1, 0.86), (0.24, 0.86)], s);
            path(&[(0.76, 0.14), (0.9, 0.14), (0.9, 0.86), (0.76, 0.86)], s);
            text(0.5, 0.5, "fx", 0.56, accent);
        }
        Icon::Fullscreen => {
            path(&[(0.1, 0.34), (0.1, 0.1), (0.34, 0.1)], s);
            path(&[(0.66, 0.1), (0.9, 0.1), (0.9, 0.34)], s);
            path(&[(0.9, 0.66), (0.9, 0.9), (0.66, 0.9)], s);
            path(&[(0.34, 0.9), (0.1, 0.9), (0.1, 0.66)], s);
        }
        Icon::SideBySide => {
            for x in [0.06, 0.54] {
                fill(x, 0.16, x + 0.4, 0.32, mid);
                frame(x, 0.16, x + 0.4, 0.84, s);
                seg(x + 0.08, 0.5, x + 0.32, 0.5, thin);
                seg(x + 0.08, 0.66, x + 0.24, 0.66, thin);
            }
        }
        Icon::Ruler => {
            fill(0.06, 0.3, 0.94, 0.7, soft);
            for i in 1..6 {
                let x = 0.06 + i as f32 * 0.147;
                seg(x, 0.3, x, if i % 2 == 0 { 0.5 } else { 0.42 }, thin);
            }
            frame(0.06, 0.3, 0.94, 0.7, s);
        }
        Icon::Outline => {
            for (x, y) in [(0.08, 0.18), (0.28, 0.4), (0.28, 0.62), (0.08, 0.84)] {
                fill(x, y - 0.06, x + 0.12, y + 0.06, accent);
                line(x + 0.2, y, 0.92, y);
            }
        }
        Icon::Window => {
            fill(0.08, 0.14, 0.92, 0.34, mid);
            frame(0.08, 0.14, 0.92, 0.86, s);
        }
        Icon::ReadMode => {
            closed(&[(0.5, 0.26), (0.08, 0.16), (0.08, 0.78), (0.5, 0.88)], s);
            closed(&[(0.5, 0.26), (0.92, 0.16), (0.92, 0.78), (0.5, 0.88)], s);
            seg(0.5, 0.26, 0.5, 0.88, s);
            for y in [0.38, 0.52, 0.66] {
                seg(0.18, y - 0.03, 0.4, y + 0.02, thin);
                seg(0.6, y + 0.02, 0.82, y - 0.03, Stroke::new(thin.width, accent));
            }
        }
        Icon::PrintLayout => {
            frame(0.2, 0.06, 0.8, 0.94, s);
            for (i, y) in [0.24, 0.38, 0.52, 0.66].into_iter().enumerate() {
                seg(0.32, y, if i == 3 { 0.54 } else { 0.68 }, y, if i == 0 { acc } else { thin });
            }
        }
        Icon::Draft => {
            for (i, y) in [0.16, 0.32, 0.72, 0.88].into_iter().enumerate() {
                line(0.1, y, if i == 1 { 0.62 } else { 0.9 }, y);
            }
            dashed(0.06, 0.52, 0.94, 0.52, acc);
        }
        Icon::Split => {
            frame(0.08, 0.1, 0.92, 0.9, s);
            seg(0.08, 0.5, 0.92, 0.5, acc);
            for y in [0.26, 0.7] {
                seg(0.2, y, 0.8, y, thin);
            }
        }
        Icon::Focus => {
            for (x, y, dx, dy) in [(0.08, 0.1, 1.0, 1.0), (0.92, 0.1, -1.0, 1.0), (0.08, 0.9, 1.0, -1.0), (0.92, 0.9, -1.0, -1.0)] {
                path(&[(x, y + 0.22 * dy), (x, y), (x + 0.22 * dx, y)], s);
            }
            seg(0.28, 0.42, 0.72, 0.42, acc);
            seg(0.28, 0.58, 0.6, 0.58, thin);
        }
        Icon::MultiPage => {
            for x in [0.06, 0.54] {
                frame(x, 0.14, x + 0.4, 0.86, s);
                for y in [0.32, 0.46, 0.6] {
                    seg(x + 0.08, y, x + 0.32, y, thin);
                }
            }
            fill(0.14, 0.2, 0.38, 0.24, accent);
        }
        // --- formulas and data ---
        Icon::TracePrecedents => {
            frame(0.06, 0.12, 0.34, 0.36, s);
            frame(0.06, 0.64, 0.34, 0.88, s);
            fill(0.62, 0.38, 0.94, 0.62, soft);
            frame(0.62, 0.38, 0.94, 0.62, s);
            for (y0, y1) in [(0.24, 0.46), (0.76, 0.54)] {
                p.circle_filled(at(0.2, y0), w * 0.05, accent);
                arrow(0.2, y0, 0.62, y1, acc);
            }
        }
        Icon::TraceDependents => {
            fill(0.06, 0.38, 0.38, 0.62, soft);
            frame(0.06, 0.38, 0.38, 0.62, s);
            frame(0.66, 0.12, 0.94, 0.36, s);
            frame(0.66, 0.64, 0.94, 0.88, s);
            p.circle_filled(at(0.22, 0.5), w * 0.05, accent);
            arrow(0.22, 0.5, 0.68, 0.26, acc);
            arrow(0.22, 0.5, 0.68, 0.74, acc);
        }
        Icon::RemoveArrows => {
            for (y0, y1) in [(0.4, 0.12), (0.76, 0.44)] {
                p.circle_filled(at(0.12, y0), w * 0.05, accent);
                arrow(0.12, y0, 0.62, y1, acc);
            }
            cross(0.76, 0.76, 0.13, Stroke::new(s.width * 1.2, red));
        }
        Icon::ErrorCheck => {
            frame(0.06, 0.1, 0.8, 0.5, thin);
            let v = [(0.62, 0.3), (0.94, 0.9), (0.3, 0.9)];
            solid(&v, Color32::from_rgb(0xF2, 0xB0, 0x1E).gamma_multiply(0.85));
            closed(&v, s);
            line(0.62, 0.5, 0.62, 0.7);
            p.circle_filled(at(0.62, 0.8), w * 0.045, color);
        }
        Icon::Calculate => {
            frame(0.2, 0.06, 0.8, 0.94, s);
            fill(0.3, 0.16, 0.7, 0.36, mid);
            for y in [0.52, 0.67, 0.82] {
                for x in [0.35, 0.5, 0.65] {
                    p.circle_filled(at(x, y), w * 0.045, color);
                }
            }
        }
        Icon::NameManager => {
            let v = [(0.08, 0.28), (0.64, 0.28), (0.92, 0.5), (0.64, 0.72), (0.08, 0.72)];
            solid(&v, soft);
            closed(&v, s);
            p.circle_stroke(at(0.68, 0.5), w * 0.06, thin);
            seg(0.2, 0.5, 0.48, 0.5, s);
        }
        Icon::GoalSeek => {
            p.circle_stroke(at(0.44, 0.56), w * 0.36, s);
            p.circle_stroke(at(0.44, 0.56), w * 0.2, s);
            p.circle_filled(at(0.44, 0.56), w * 0.07, accent);
            arrow(0.94, 0.06, 0.48, 0.52, acc);
        }
        Icon::Solver => {
            axes();
            let v: Vec<(f32, f32)> = (0..=12).map(|i| {
                let x = 0.2 + 0.66 * i as f32 / 12.0;
                (x, 0.7 - 3.6 * (x - 0.53) * (x - 0.53))
            }).collect();
            path(&v, s);
            p.circle_filled(at(0.53, 0.7), w * 0.075, accent);
        }
        Icon::Subtotal => {
            text(0.24, 0.5, "Σ", 0.62, accent);
            for y in [0.22, 0.42, 0.62] {
                seg(0.52, y, 0.92, y, thin);
            }
            line(0.52, 0.82, 0.92, 0.82);
        }
        Icon::Group | Icon::Ungroup => {
            for y in [0.18, 0.38, 0.58, 0.82] {
                line(0.44, y, 0.92, y);
            }
            let st = Stroke::new(thin.width, accent);
            if icon == Icon::Group {
                path(&[(0.32, 0.18), (0.2, 0.18), (0.2, 0.62)], st);
            } else {
                path(&[(0.32, 0.18), (0.2, 0.18), (0.2, 0.3)], st);
                path(&[(0.2, 0.46), (0.2, 0.62)], st);
            }
            frame(0.08, 0.66, 0.32, 0.9, st);
            seg(0.13, 0.78, 0.27, 0.78, st);
            if icon == Icon::Ungroup {
                seg(0.2, 0.71, 0.2, 0.85, st);
            }
        }
        Icon::TextToColumns => {
            frame(0.06, 0.22, 0.94, 0.78, s);
            seg(0.16, 0.5, 0.4, 0.5, thin);
            seg(0.6, 0.5, 0.84, 0.5, thin);
            seg(0.5, 0.08, 0.5, 0.92, acc);
        }
        Icon::RemoveDuplicates => {
            frame(0.06, 0.1, 0.76, 0.4, s);
            seg(0.16, 0.25, 0.6, 0.25, thin);
            fill(0.06, 0.52, 0.76, 0.82, soft);
            frame(0.06, 0.52, 0.76, 0.82, thin);
            seg(0.16, 0.67, 0.5, 0.67, thin);
            cross(0.8, 0.78, 0.12, Stroke::new(s.width * 1.2, red));
        }
        Icon::Scenario => {
            let faint = Stroke::new(thin.width, color.gamma_multiply(0.6));
            seg(0.5, 0.1, 0.5, 0.9, faint);
            seg(0.08, 0.5, 0.92, 0.5, faint);
            frame(0.08, 0.1, 0.92, 0.9, s);
            text(0.5, 0.52, "?", 0.72, accent);
        }
        // --- cells ---
        Icon::InsertRows | Icon::DeleteRows => {
            fill(0.36, 0.37, 0.92, 0.63, mid);
            seg(0.36, 0.37, 0.92, 0.37, thin);
            seg(0.36, 0.63, 0.92, 0.63, thin);
            frame(0.36, 0.1, 0.92, 0.9, s);
            if icon == Icon::InsertRows {
                plus(0.16, 0.5, 0.11, acc);
            } else {
                cross(0.16, 0.5, 0.09, Stroke::new(s.width, red));
            }
        }
        Icon::InsertCols | Icon::DeleteCols => {
            fill(0.37, 0.36, 0.63, 0.92, mid);
            seg(0.37, 0.36, 0.37, 0.92, thin);
            seg(0.63, 0.36, 0.63, 0.92, thin);
            frame(0.1, 0.36, 0.9, 0.92, s);
            if icon == Icon::InsertCols {
                plus(0.5, 0.16, 0.11, acc);
            } else {
                cross(0.5, 0.16, 0.09, Stroke::new(s.width, red));
            }
        }
        Icon::CellFormat => {
            fill(0.06, 0.06, 0.36, 0.36, soft);
            seg(0.36, 0.06, 0.36, 0.66, thin);
            seg(0.06, 0.36, 0.66, 0.36, thin);
            frame(0.06, 0.06, 0.66, 0.66, s);
            gear(0.72, 0.72, 0.24, Stroke::new(thin.width, accent));
        }
        Icon::AutoFit => {
            fill(0.1, 0.1, 0.9, 0.9, soft);
            line(0.1, 0.1, 0.1, 0.9);
            line(0.9, 0.1, 0.9, 0.9);
            arrows(0.2, 0.5, 0.8, 0.5, Stroke::new(thin.width, accent));
        }
        Icon::Hide => {
            let lid = |sign: f32| (0..=16).map(|i| {
                let t = i as f32 / 16.0;
                at(0.08 + 0.84 * t, 0.5 + sign * 0.26 * (PI * t).sin())
            }).collect::<Vec<Pos2>>();
            p.add(Shape::line(lid(-1.0), s));
            p.add(Shape::line(lid(1.0), s));
            p.circle_filled(at(0.5, 0.5), w * 0.12, accent);
            line(0.16, 0.86, 0.84, 0.14);
        }
        // --- documents ---
        Icon::DropCap => {
            text(0.24, 0.34, "A", 0.62, accent);
            for y in [0.16, 0.34, 0.52] {
                line(0.5, y, 0.92, y);
            }
            line(0.08, 0.72, 0.92, 0.72);
            line(0.08, 0.9, 0.7, 0.9);
        }
        Icon::TextArt => {
            text(0.56, 0.56, "A", 0.86, color.gamma_multiply(0.35));
            text(0.5, 0.5, "A", 0.86, accent);
            line(0.14, 0.94, 0.86, 0.94);
        }
        Icon::Replace => {
            text(0.3, 0.28, "ab", 0.42, color);
            text(0.7, 0.74, "cd", 0.42, accent);
            path(&[(0.6, 0.26), (0.84, 0.26), (0.84, 0.46)], acc);
            solid(&[(0.74, 0.44), (0.94, 0.44), (0.84, 0.58)], accent);
            path(&[(0.4, 0.76), (0.16, 0.76), (0.16, 0.56)], s);
            tri(at(0.06, 0.58), at(0.26, 0.58), at(0.16, 0.44));
        }
        Icon::Select => {
            for (x0, y0, x1, y1) in [(0.08, 0.08, 0.7, 0.08), (0.7, 0.08, 0.7, 0.4), (0.4, 0.7, 0.08, 0.7), (0.08, 0.7, 0.08, 0.08)] {
                dashed(x0, y0, x1, y1, Stroke::new(thin.width, accent));
            }
            let k = 0.03;
            let cur = |x: f32, y: f32| (0.44 + x * k, 0.36 + y * k);
            solid(&[cur(0.0, 0.0), cur(0.0, 16.0), cur(11.0, 11.0)], accent);
            closed(&[cur(0.0, 0.0), cur(0.0, 16.0), cur(4.0, 12.0), cur(7.0, 18.0), cur(9.0, 17.0), cur(6.0, 11.0), cur(11.0, 11.0)], thin);
        }
        // --- app and files ---
        Icon::Print => {
            frame(0.26, 0.08, 0.74, 0.36, s);
            solid(&[(0.08, 0.36), (0.92, 0.36), (0.92, 0.74), (0.08, 0.74)], soft);
            path(&[(0.26, 0.74), (0.08, 0.74), (0.08, 0.36), (0.92, 0.36), (0.92, 0.74), (0.74, 0.74)], s);
            frame(0.26, 0.6, 0.74, 0.92, s);
            seg(0.36, 0.76, 0.64, 0.76, thin);
            p.circle_filled(at(0.8, 0.48), w * 0.045, accent);
        }
        Icon::Pdf => {
            page(0.22, 0.06, 0.86, 0.94, s);
            p.rect_filled(rc(0.06, 0.46, 0.76, 0.8), w * 0.04, red);
            text(0.41, 0.63, "PDF", 0.27, Color32::WHITE);
        }
        Icon::Export => {
            page(0.08, 0.06, 0.64, 0.92, s);
            seg(0.2, 0.36, 0.44, 0.36, thin);
            seg(0.2, 0.76, 0.44, 0.76, thin);
            arrow(0.3, 0.56, 0.96, 0.56, acc);
        }
        Icon::Template => {
            page(0.16, 0.06, 0.84, 0.94, s);
            fill(0.28, 0.3, 0.72, 0.42, mid);
            fill(0.28, 0.5, 0.46, 0.82, soft);
            for y in [0.54, 0.68, 0.8] {
                seg(0.54, y, 0.72, y, thin);
            }
        }
        Icon::Close => {
            line(0.22, 0.22, 0.78, 0.78);
            line(0.22, 0.78, 0.78, 0.22);
        }
        Icon::Options => gear(0.5, 0.5, 0.4, s),
        Icon::Keyboard => {
            fill(0.04, 0.24, 0.96, 0.78, soft);
            frame(0.04, 0.24, 0.96, 0.78, s);
            for y in [0.4, 0.54] {
                for x in [0.2, 0.35, 0.5, 0.65, 0.8] {
                    p.circle_filled(at(x, y), w * 0.04, color);
                }
            }
            seg(0.3, 0.66, 0.7, 0.66, acc);
        }
        Icon::Help => {
            p.circle_filled(at(0.5, 0.5), w * 0.4, soft);
            p.circle_stroke(at(0.5, 0.5), w * 0.4, s);
            text(0.5, 0.52, "?", 0.56, color);
        }
        Icon::Language => {
            p.circle_filled(at(0.5, 0.5), w * 0.4, soft);
            p.add(Shape::closed_line(ellipse(0.5, 0.5, 0.17, 0.4, 0.0, TAU), thin));
            seg(0.1, 0.5, 0.9, 0.5, thin);
            seg(0.154, 0.3, 0.846, 0.3, thin);
            seg(0.154, 0.7, 0.846, 0.7, thin);
            p.circle_stroke(at(0.5, 0.5), w * 0.4, s);
        }
        Icon::Theme => {
            p.add(Shape::convex_polygon(ellipse(0.5, 0.5, 0.36, 0.36, -FRAC_PI_2, FRAC_PI_2), accent, Stroke::NONE));
            p.circle_stroke(at(0.5, 0.5), w * 0.36, s);
        }
        Icon::Graph => {
            let n = [(0.2, 0.28), (0.72, 0.18), (0.5, 0.58), (0.18, 0.82), (0.84, 0.76)];
            for (a, b) in [(0, 2), (1, 2), (2, 3), (2, 4), (0, 1)] {
                seg(n[a].0, n[a].1, n[b].0, n[b].1, thin);
            }
            for (k, (x, y)) in n.iter().enumerate() {
                let c = if k == 2 { accent } else { color };
                p.circle_filled(at(*x, *y), w * if k == 2 { 0.12 } else { 0.085 }, c);
            }
        }
        Icon::Canvas => {
            frame(0.06, 0.1, 0.94, 0.9, s);
            fill(0.16, 0.22, 0.42, 0.42, soft);
            frame(0.16, 0.22, 0.42, 0.42, thin);
            fill(0.56, 0.56, 0.84, 0.78, soft);
            frame(0.56, 0.56, 0.84, 0.78, thin);
            path(&[(0.42, 0.34), (0.62, 0.34), (0.62, 0.5)], acc);
            solid(&[(0.56, 0.48), (0.68, 0.48), (0.62, 0.56)], accent);
        }
        Icon::Add => plus(0.5, 0.5, 0.32, s),
        Icon::Tasks => {
            for y in [0.24, 0.5, 0.76] {
                frame(0.08, y - 0.1, 0.28, y + 0.1, thin);
                seg(0.4, y, 0.92, y, s);
            }
            path(&[(0.1, 0.24), (0.17, 0.31), (0.3, 0.14)], acc);
            path(&[(0.1, 0.5), (0.17, 0.57), (0.3, 0.4)], acc);
        }
        Icon::Notebook => {
            fill(0.26, 0.08, 0.86, 0.92, soft);
            frame(0.26, 0.08, 0.86, 0.92, s);
            seg(0.38, 0.08, 0.38, 0.92, thin);
            for y in [0.22, 0.4, 0.58, 0.76] {
                seg(0.14, y, 0.32, y, s);
            }
            for y in [0.32, 0.48, 0.64] {
                seg(0.5, y, 0.76, y, thin);
            }
        }
        Icon::Trash => {
            line(0.14, 0.24, 0.86, 0.24);
            path(&[(0.38, 0.24), (0.38, 0.12), (0.62, 0.12), (0.62, 0.24)], s);
            path(&[(0.22, 0.24), (0.28, 0.9), (0.72, 0.9), (0.78, 0.24)], s);
            seg(0.42, 0.38, 0.42, 0.78, thin);
            seg(0.58, 0.38, 0.58, 0.78, thin);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws every icon into a PNG at `$ICON_SHEET` to look at them: on
    /// dark and light, at 32 px with a blue accent and at 16 px plain.
    #[test]
    #[ignore]
    fn contact_sheet() {
        let Ok(out) = std::env::var("ICON_SHEET") else { return };
        let ppp: f32 = std::env::var("ICON_PPP").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0);
        const COLS: usize = 10;
        let cell = vec2(68.0, 56.0);
        let half = vec2(cell.x * COLS as f32, cell.y * ALL.len().div_ceil(COLS) as f32);
        let size = vec2(half.x * 2.0, half.y);
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        ctx.set_pixels_per_point(ppp);
        let input = || egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), ..Default::default() };
        ctx.run_ui(input(), |_| {}).textures_delta.clear();
        let blue = Color32::from_rgb(0x2B, 0x7C, 0xD3);
        let themes = [(Color32::from_rgb(0x1E, 0x1E, 0x1E), Color32::from_rgb(0xD4, 0xD4, 0xD4)), (Color32::WHITE, Color32::from_rgb(0x3B, 0x3B, 0x3B))];
        let mut full = ctx.run_ui(input(), |ui| {
            let p = ui.ctx().layer_painter(egui::LayerId::background());
            for (k, (bg, fg)) in themes.into_iter().enumerate() {
                let origin = pos2(k as f32 * half.x, 0.0);
                p.rect_filled(Rect::from_min_size(origin, half), 0.0, bg);
                for (i, icon) in ALL.iter().enumerate() {
                    let c = origin + vec2((i % COLS) as f32 * cell.x, (i / COLS) as f32 * cell.y);
                    paint_accent(&p, Rect::from_min_size(c + vec2(8.0, 6.0), vec2(32.0, 32.0)), *icon, fg, blue);
                    paint(&p, Rect::from_min_size(c + vec2(46.0, 14.0), vec2(16.0, 16.0)), *icon, fg);
                    p.text(c + vec2(cell.x / 2.0, cell.y - 8.0), egui::Align2::CENTER_CENTER, format!("{icon:?}"), egui::FontId::proportional(8.0), fg.gamma_multiply(0.7));
                }
            }
        });
        full.textures_delta.clear();
        let prims = ctx.tessellate(std::mem::take(&mut full.shapes), ppp);
        let atlas = ctx.fonts(|f| f.image());
        let (w, h) = ((size.x * ppp) as usize, (size.y * ppp) as usize);
        let mut img = egui::ColorImage::new([w, h], vec![Color32::BLACK; w * h]);
        for prim in prims {
            if let egui::epaint::Primitive::Mesh(m) = prim.primitive {
                if m.texture_id == egui::TextureId::default() {
                    crate::charts::raster(&mut img, &m, &atlas, ppp);
                }
            }
        }
        std::fs::write(out, crate::charts::encode_png(&img).expect("png")).expect("write");
    }
}
