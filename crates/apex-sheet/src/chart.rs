//! Charts floating over a sheet and sparklines in its cells.
//!
//! A chart takes its data from references (kept as formulas, so they follow
//! inserted and deleted rows and renamed sheets like any formula) and keeps
//! the values the file had as a fallback for references that can't be read
//! (another workbook, a union). [`Workbook::chart_data`] turns a chart into
//! plain numbers and labels for painting and saving.

use std::sync::Arc;

use crate::formula::{self, Expr, RangeKind};
use crate::functions::Env;
use crate::image::Anchor;
use crate::value::Value;
use crate::workbook::Op;
use crate::{CellRef, Locale, MAX_COLS, MAX_ROWS, Range, Sheet, Workbook};

/// Values read from one reference at most (hostile files, whole columns).
pub const MAX_POINTS: usize = 100_000;
/// Series in one chart at most.
pub const MAX_SERIES: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ChartType {
    #[default]
    Column,
    Bar,
    Line,
    Area,
    Pie,
    Doughnut,
    Scatter,
    Waterfall,
    Histogram,
    /// Spider web: one spoke per category.
    Radar,
    /// Scatter whose points are circles sized by a third value.
    Bubble,
    /// High-low-close (3 series) or open-high-low-close (4 series).
    Stock,
    /// A grid of values as a 3-D surface or (not `view3d`) a contour map.
    Surface,
    /// Nested rectangles sized by value, grouped by the category levels.
    Treemap,
    /// Rings of the category levels, the innermost the outer level.
    Sunburst,
    /// Quartiles, whiskers, mean and outliers of each series.
    BoxWhisker,
    /// Centered bars getting narrower, one per category.
    Funnel,
    /// Columns sorted from the largest with the cumulative percent line.
    Pareto,
    /// Marimekko: 100% stacked columns as wide as their total. The first
    /// series holds the column widths, the others the segments.
    Mekko,
    /// Milestones on a time axis. The first series holds the dates (serial
    /// numbers), its categories the labels.
    Timeline,
}

impl ChartType {
    pub const ALL: [ChartType; 20] = [
        ChartType::Column,
        ChartType::Bar,
        ChartType::Line,
        ChartType::Area,
        ChartType::Pie,
        ChartType::Doughnut,
        ChartType::Scatter,
        ChartType::Bubble,
        ChartType::Radar,
        ChartType::Stock,
        ChartType::Surface,
        ChartType::Treemap,
        ChartType::Sunburst,
        ChartType::Histogram,
        ChartType::Pareto,
        ChartType::BoxWhisker,
        ChartType::Waterfall,
        ChartType::Funnel,
        ChartType::Mekko,
        ChartType::Timeline,
    ];

    /// Pie and doughnut: no axes, one color per point.
    pub fn is_round(self) -> bool {
        matches!(self, ChartType::Pie | ChartType::Doughnut)
    }

    /// Has a category axis and a value axis (as the axis settings apply).
    pub fn has_axes(self) -> bool {
        !self.is_round() && !matches!(self, ChartType::Treemap | ChartType::Sunburst | ChartType::Mekko | ChartType::Timeline)
    }

    /// Drawn with depth when `view3d` is on.
    pub fn has_3d(self) -> bool {
        matches!(self, ChartType::Column | ChartType::Bar | ChartType::Line | ChartType::Area | ChartType::Pie | ChartType::Surface)
    }

    /// Only in Excel's newer charts (chartex parts).
    pub fn is_chartex(self) -> bool {
        matches!(self, ChartType::Waterfall | ChartType::Histogram | ChartType::Treemap | ChartType::Sunburst | ChartType::BoxWhisker | ChartType::Funnel | ChartType::Pareto)
    }

    /// Kinds of ours that no other program knows: saved as an emulation
    /// with the data in an extension.
    pub fn is_apex_only(self) -> bool {
        matches!(self, ChartType::Mekko | ChartType::Timeline)
    }

    /// Takes numeric x values from the categories.
    pub fn is_xy(self) -> bool {
        matches!(self, ChartType::Scatter | ChartType::Bubble)
    }

    /// One color per point (category) rather than per series.
    pub fn colors_points(self) -> bool {
        matches!(self, ChartType::Pie | ChartType::Doughnut | ChartType::Treemap | ChartType::Sunburst | ChartType::Funnel)
    }

    /// Stacking applies (column, bar, line, area).
    pub fn can_stack(self) -> bool {
        matches!(self, ChartType::Column | ChartType::Bar | ChartType::Line | ChartType::Area)
    }

    /// Stable name used in tests and i18n keys (`chart.type.<key>`).
    pub fn key(self) -> &'static str {
        match self {
            ChartType::Column => "column",
            ChartType::Bar => "bar",
            ChartType::Line => "line",
            ChartType::Area => "area",
            ChartType::Pie => "pie",
            ChartType::Doughnut => "doughnut",
            ChartType::Scatter => "scatter",
            ChartType::Waterfall => "waterfall",
            ChartType::Histogram => "histogram",
            ChartType::Radar => "radar",
            ChartType::Bubble => "bubble",
            ChartType::Stock => "stock",
            ChartType::Surface => "surface",
            ChartType::Treemap => "treemap",
            ChartType::Sunburst => "sunburst",
            ChartType::BoxWhisker => "boxwhisker",
            ChartType::Funnel => "funnel",
            ChartType::Pareto => "pareto",
            ChartType::Mekko => "mekko",
            ChartType::Timeline => "timeline",
        }
    }
}

/// How series of a column, bar, line or area chart share the space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Grouping {
    /// Side by side (columns, bars) or on their own (lines, areas).
    #[default]
    Clustered,
    Stacked,
    /// Stacked to 100%.
    Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum LegendPos {
    None,
    #[default]
    Right,
    Left,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Marker {
    /// The chart decides (lines and scatter with markers show them).
    #[default]
    Auto,
    None,
    Circle,
    Square,
    Diamond,
    Triangle,
}

/// What an empty cell becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Blanks {
    #[default]
    Gap,
    Zero,
    /// Lines connect the points around it.
    Span,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrendKind {
    Linear,
    Exponential,
    Logarithmic,
    /// Order 2..=6.
    Polynomial(u8),
    Power,
    /// Period 2..
    MovingAverage(u8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trendline {
    pub kind: TrendKind,
    pub show_equation: bool,
    pub show_r2: bool,
}

/// How long error bars are.
#[derive(Clone, Debug, PartialEq)]
pub enum ErrorAmount {
    /// The same amount for every point.
    Fixed(f64),
    /// A percent of each value.
    Percent(f64),
    /// A number of standard deviations of the series, around its mean.
    StdDev(f64),
    /// The standard error of the series.
    StdErr,
    /// From cells: the plus and the minus amounts.
    Custom { plus: DataRef, minus: DataRef },
}

/// Which way error bars go from the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ErrorDir {
    #[default]
    Both,
    Plus,
    Minus,
}

/// Error bars of the values of a series (vertical; horizontal on bars).
#[derive(Clone, Debug, PartialEq)]
pub struct ErrorBars {
    pub amount: ErrorAmount,
    pub dir: ErrorDir,
    /// Short crossbars at the ends.
    pub cap: bool,
}

impl Default for ErrorBars {
    fn default() -> Self {
        ErrorBars { amount: ErrorAmount::Percent(5.0), dir: ErrorDir::Both, cap: true }
    }
}

impl ErrorBars {
    /// The bars of each point: (low, high) ends, or None. `plus` and
    /// `minus` are the values of custom amounts.
    pub fn ends(&self, values: &[Option<f64>], plus: &[Option<f64>], minus: &[Option<f64>]) -> Vec<Option<(f64, f64)>> {
        let nums: Vec<f64> = values.iter().flatten().copied().filter(|v| v.is_finite()).collect();
        let n = nums.len() as f64;
        let mean = if n > 0.0 { nums.iter().sum::<f64>() / n } else { 0.0 };
        let sd = if n > 1.0 { (nums.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt() } else { 0.0 };
        values
            .iter()
            .enumerate()
            .map(|(j, v)| {
                let v = (*v)?;
                let (lo, hi) = match &self.amount {
                    ErrorAmount::Fixed(a) => (v - a, v + a),
                    ErrorAmount::Percent(p) => (v - (v * p / 100.0).abs(), v + (v * p / 100.0).abs()),
                    // Excel centers standard deviation bars on the mean.
                    ErrorAmount::StdDev(k) => (mean - k * sd, mean + k * sd),
                    ErrorAmount::StdErr => (v - sd / n.max(1.0).sqrt(), v + sd / n.max(1.0).sqrt()),
                    ErrorAmount::Custom { .. } => (v - minus.get(j).copied().flatten().unwrap_or(0.0), v + plus.get(j).copied().flatten().unwrap_or(0.0)),
                };
                let (lo, hi) = match self.dir {
                    ErrorDir::Both => (lo, hi),
                    ErrorDir::Plus => (v, hi),
                    ErrorDir::Minus => (lo, v),
                };
                (lo.is_finite() && hi.is_finite()).then_some((lo, hi))
            })
            .collect()
    }
}

/// Bins of a histogram.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Bins {
    #[default]
    Auto,
    Width(f64),
    Count(u32),
}

/// Where a series' names, categories or values come from: a reference or
/// formula, and the values last seen (from the file, or literal data).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DataRef {
    pub formula: Option<Arc<Expr>>,
    pub cache: Vec<Value>,
}

impl DataRef {
    /// A reference in canonical syntax (`Sheet1!$B$2:$B$9`); an unreadable
    /// one is kept as nothing.
    pub fn parse(text: &str) -> DataRef {
        let t = text.trim();
        let t = t.strip_prefix('=').unwrap_or(t);
        if t.is_empty() {
            return DataRef::default();
        }
        DataRef { formula: formula::parse_canonical(t).ok().map(Arc::new), cache: Vec::new() }
    }

    /// A reference to `range` of the sheet named `sheet`, absolute.
    pub fn range(sheet: &str, range: Range) -> DataRef {
        DataRef::parse(&format!("{}!{}", quote_sheet(sheet), abs_a1(&range)))
    }

    pub fn literal(values: Vec<Value>) -> DataRef {
        DataRef { formula: None, cache: values }
    }

    pub fn text(s: &str) -> DataRef {
        DataRef::literal(vec![Value::Text(Arc::from(s))])
    }

    pub fn is_empty(&self) -> bool {
        self.formula.is_none() && self.cache.is_empty()
    }

    /// The formula in canonical syntax, without `=`.
    pub fn to_canonical(&self) -> Option<String> {
        self.formula.as_ref().map(|f| f.to_canonical())
    }

    /// The formula as the user edits it (`=Sheet1!$B$2:$B$9`).
    pub fn to_display(&self, loc: &Locale) -> String {
        self.formula.as_ref().map(|f| f.to_display(loc)).unwrap_or_default()
    }

    /// The sheet and area when the formula is a single reference.
    pub fn area(&self) -> Option<(Option<Arc<str>>, Range)> {
        match self.formula.as_deref()? {
            Expr::Ref { sheet, addr } => Some((sheet.clone(), Range::single(CellRef::new(addr.row, addr.col)))),
            Expr::Range { sheet, a, b, kind } => {
                let (a, b) = match kind {
                    RangeKind::Cells => (CellRef::new(a.row, a.col), CellRef::new(b.row, b.col)),
                    RangeKind::Cols => (CellRef::new(0, a.col), CellRef::new(MAX_ROWS - 1, b.col)),
                    RangeKind::Rows => (CellRef::new(a.row, 0), CellRef::new(b.row, MAX_COLS - 1)),
                };
                Some((sheet.clone(), Range::new(a, b)))
            }
            _ => None,
        }
    }

    fn edit_formula(&mut self, f: impl FnOnce(&mut Expr) -> bool) {
        if let Some(e) = &mut self.formula {
            let mut x = (**e).clone();
            if f(&mut x) {
                *e = Arc::new(x);
            }
        }
    }
}

/// Sheet name as written in a reference.
pub fn quote_sheet(name: &str) -> String {
    if let Some((_, rest)) = crate::external::split_external(name) {
        if rest.is_empty() || quote_sheet(rest) == rest {
            return name.to_string();
        }
    }
    let plain = name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.');
    // Names that look like cells ("A1", "R1C1") need quotes too.
    if plain && CellRef::parse(name).is_none() {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

/// `$A$1:$B$2`.
pub fn abs_a1(r: &Range) -> String {
    let one = |c: CellRef| format!("${}${}", crate::addr::col_name(c.col), c.row + 1);
    if r.is_single() { one(r.start) } else { format!("{}:{}", one(r.start), one(r.end)) }
}

/// A fill other than a plain color.
#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    /// Colors at positions 0..=1; `angle` in degrees (0 = left to right,
    /// 90 = top to bottom); `radial` spreads from the center instead.
    Gradient { stops: Vec<(f32, u32)>, angle: f32, radial: bool },
    /// A DrawingML preset (`pct50`, `ltUpDiag`, `smGrid`...) in `fg` over `bg`.
    Pattern { preset: String, fg: u32, bg: u32 },
}

impl Fill {
    /// The color that stands for it where only one fits.
    pub fn main_color(&self) -> u32 {
        match self {
            Fill::Gradient { stops, .. } => stops.first().map_or(0, |s| s.1),
            Fill::Pattern { fg, .. } => *fg,
        }
    }

    /// The color at `t` (0..=1) along a gradient.
    pub fn at(stops: &[(f32, u32)], t: f32) -> u32 {
        let Some(first) = stops.first() else { return 0 };
        if t <= first.0 {
            return first.1;
        }
        for w in stops.windows(2) {
            let ((a, ca), (b, cb)) = (w[0], w[1]);
            if t <= b {
                let k = if b > a { (t - a) / (b - a) } else { 1.0 };
                let ch = |sh: u32| {
                    let (x, y) = ((ca >> sh & 255) as f32, (cb >> sh & 255) as f32);
                    ((x + (y - x) * k).round().clamp(0.0, 255.0) as u32) << sh
                };
                return ch(16) | ch(8) | ch(0);
            }
        }
        stops.last().map_or(0, |s| s.1)
    }
}

/// Look of the chart area or the plot area.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BoxFormat {
    /// None = the style's.
    pub fill: Option<u32>,
    pub fancy: Option<Fill>,
    pub no_fill: bool,
    pub border: Option<u32>,
    pub no_border: bool,
    /// Text (chart area: every text of the chart).
    pub text_color: Option<u32>,
    /// Font size in points.
    pub font_size: Option<f32>,
    pub font: Option<String>,
    pub bold: Option<bool>,
}

impl BoxFormat {
    pub fn is_default(&self) -> bool {
        *self == BoxFormat::default()
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Series {
    pub name: DataRef,
    /// Category labels (x values of a scatter chart).
    pub categories: DataRef,
    pub values: DataRef,
    /// Fill (and line of lines and scatter); None = from the palette.
    pub color: Option<u32>,
    /// Border of columns, bars, areas and slices; line of lines.
    pub line_color: Option<u32>,
    /// Line width in points; None = the style's.
    pub line_width: Option<f32>,
    pub no_fill: bool,
    pub no_line: bool,
    pub marker: Marker,
    pub smooth: bool,
    /// Drawn as another type (combo charts): column, line or area.
    pub kind: Option<ChartType>,
    /// On the secondary value axis.
    pub secondary: bool,
    pub trendline: Option<Trendline>,
    /// Colors of single points (pie slices): index, color.
    pub point_colors: Vec<(u32, u32)>,
    /// Gradient or pattern instead of `color` (which keeps its main color).
    pub fancy: Option<Fill>,
    /// Pie slices pulled out, percent of the radius.
    pub explode: u8,
    /// Single slices pulled out: index, percent.
    pub point_explode: Vec<(u32, u8)>,
    /// Bubble sizes (bubble charts).
    pub sizes: DataRef,
    pub error_bars: Option<ErrorBars>,
    /// Text of the data labels, from cells ("value from cells").
    pub label_range: DataRef,
}

impl Series {
    /// How far slice `j` is pulled out, percent of the radius.
    pub fn explosion(&self, j: usize) -> u8 {
        self.point_explode.iter().find(|p| p.0 as usize == j).map_or(self.explode, |p| p.1)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    pub title: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub major: Option<f64>,
    pub minor: Option<f64>,
    /// Number format code; empty = from the source cells.
    pub format: String,
    /// Logarithmic scale with this base.
    pub log_base: Option<f64>,
    /// Values (or categories) in reverse order.
    pub reverse: bool,
    pub major_grid: bool,
    pub minor_grid: bool,
    pub visible: bool,
}

impl Default for Axis {
    fn default() -> Self {
        Axis { title: String::new(), min: None, max: None, major: None, minor: None, format: String::new(), log_base: None, reverse: false, major_grid: false, minor_grid: false, visible: true }
    }
}

impl Axis {
    fn value_axis() -> Axis {
        Axis { major_grid: true, ..Default::default() }
    }

    fn sanitize(&mut self) {
        let fin = |v: &mut Option<f64>| {
            if v.is_some_and(|x| !x.is_finite()) {
                *v = None;
            }
        };
        fin(&mut self.min);
        fin(&mut self.max);
        fin(&mut self.major);
        fin(&mut self.minor);
        if self.major.is_some_and(|m| m <= 0.0) {
            self.major = None;
        }
        if self.minor.is_some_and(|m| m <= 0.0) {
            self.minor = None;
        }
        if let (Some(a), Some(b)) = (self.min, self.max) {
            if a >= b {
                self.max = None;
            }
        }
        if self.log_base.is_some_and(|b| !(b.is_finite() && (2.0..=1000.0).contains(&b))) {
            self.log_base = Some(10.0);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Labels {
    pub value: bool,
    pub percent: bool,
    pub category: bool,
    pub series: bool,
    /// The text of the series' label range.
    pub cells: bool,
}

impl Labels {
    pub fn any(&self) -> bool {
        self.value || self.percent || self.category || self.series || self.cells
    }
}

/// Number of styles in the gallery and of color palettes.
pub const STYLES: u8 = 8;
pub const PALETTES: u8 = 6;

/// The colors of palette `p`: Office, blues, oranges, greens, grays and
/// LibreOffice's default colors.
pub fn palette(p: u8) -> [u32; 10] {
    match p {
        1 => [0x1F4E79, 0x2E75B6, 0x5B9BD5, 0x9DC3E6, 0x203864, 0x2F5597, 0x8FAADC, 0xBDD7EE, 0x4472C4, 0xDEEBF7],
        2 => [0x843C0C, 0xC55A11, 0xED7D31, 0xF4B183, 0x7F6000, 0xBF9000, 0xFFC000, 0xFFD966, 0x9E480E, 0xF8CBAD],
        3 => [0x375623, 0x548235, 0x70AD47, 0xA9D18E, 0x385723, 0x7F9F3F, 0xC5E0B4, 0x92D050, 0x00B050, 0xE2F0D9],
        4 => [0x262626, 0x595959, 0x7F7F7F, 0xA6A6A6, 0x3A3838, 0x757171, 0xBFBFBF, 0xD9D9D9, 0x404040, 0x0D0D0D],
        // LibreOffice's default colors.
        5 => [0x004586, 0xFF420E, 0xFFD320, 0x579D1C, 0x7E0021, 0x83CAFF, 0x314004, 0xAECF00, 0x4B1F6F, 0xFF950E],
        _ => [0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5, 0x70AD47, 0x264478, 0x9E480E, 0x636363, 0x997300],
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    /// Unique within its sheet (0 = not assigned yet).
    pub id: u32,
    pub name: String,
    /// Alternative text, for screen readers.
    pub alt: String,
    /// Cell holding the top-left corner, the offset inside it and the size,
    /// in sheet pixels (96 dpi), as for pictures.
    pub from: CellRef,
    pub dx: f32,
    pub dy: f32,
    pub width: f32,
    pub height: f32,
    pub anchor: Anchor,
    pub kind: ChartType,
    pub grouping: Grouping,
    /// Lines (and scatter) show markers.
    pub markers: bool,
    /// Scatter points are joined by lines.
    pub lines: bool,
    pub smooth: bool,
    /// Doughnut hole, percent of the radius.
    pub hole: u8,
    /// Space between column groups, percent of a column.
    pub gap: u16,
    /// Overlap of columns in a group, percent (-100..=100).
    pub overlap: i8,
    /// Angle of the first slice, degrees clockwise from the top.
    pub first_angle: u16,
    /// Pie slices (and single-series columns) take a color each.
    pub vary_colors: bool,
    pub bins: Bins,
    /// Waterfall points that are totals (drawn from zero).
    pub totals: Vec<u32>,
    pub title: String,
    pub show_title: bool,
    pub x_axis: Axis,
    pub y_axis: Axis,
    /// Secondary value axis (series with `secondary`).
    pub y2_axis: Axis,
    pub legend: LegendPos,
    pub labels: Labels,
    /// Gallery style, 0..STYLES.
    pub style: u8,
    /// Color palette, 0..PALETTES.
    pub palette: u8,
    pub series: Vec<Series>,
    /// Hidden rows and columns are left out.
    pub visible_only: bool,
    pub blanks: Blanks,
    /// Data range the series were built from (the wizard's range) and
    /// whether each row is a series.
    pub source: Option<DataRef>,
    pub by_rows: bool,
    /// Chart area (the whole chart; its text settings apply to all text).
    pub area: BoxFormat,
    /// Plot area.
    pub plot: BoxFormat,
    /// Accent colors of the workbook's theme, for the automatic colors of
    /// palette 0 (None: Office's).
    pub accents: Option<[u32; 6]>,
    /// Drawn with depth (3-D column, bar, line, area, pie; a surface seen
    /// in perspective rather than from above).
    pub view3d: bool,
    /// Radar areas are filled.
    pub filled: bool,
    /// Box and whisker: mean markers and outlier points.
    pub show_mean: bool,
    pub show_outliers: bool,
    /// Bubble size, percent of the default.
    pub bubble_scale: u16,
    /// A table of the values under the plot (category charts).
    pub data_table: bool,
    /// The data table shows the legend key of each series.
    pub table_keys: bool,
}

impl Chart {
    pub fn new(kind: ChartType, from: CellRef, width: f32, height: f32) -> Chart {
        let mut c = Chart {
            id: 0,
            name: String::new(),
            alt: String::new(),
            from,
            dx: 0.0,
            dy: 0.0,
            width,
            height,
            anchor: Anchor::TwoCell,
            kind,
            grouping: Grouping::Clustered,
            markers: matches!(kind, ChartType::Scatter),
            lines: false,
            smooth: false,
            hole: 50,
            gap: match kind {
                ChartType::Histogram | ChartType::Funnel => 6,
                ChartType::BoxWhisker => 100,
                _ => 150,
            },
            overlap: 0,
            first_angle: 0,
            vary_colors: kind.is_round(),
            bins: Bins::Auto,
            totals: Vec::new(),
            title: String::new(),
            show_title: true,
            x_axis: Axis::default(),
            y_axis: Axis::value_axis(),
            y2_axis: Axis::default(),
            legend: LegendPos::Right,
            labels: Labels::default(),
            style: 0,
            palette: 0,
            series: Vec::new(),
            visible_only: true,
            blanks: Blanks::Gap,
            source: None,
            by_rows: false,
            area: BoxFormat::default(),
            plot: BoxFormat::default(),
            accents: None,
            view3d: kind == ChartType::Surface,
            filled: false,
            show_mean: true,
            show_outliers: true,
            bubble_scale: 100,
            data_table: false,
            table_keys: true,
        };
        c.sanitize();
        c
    }

    /// The colors of the automatic series and slices.
    pub fn colors(&self) -> [u32; 10] {
        match self.accents {
            Some(a) if self.palette == 0 => {
                let dark = |c: u32| {
                    let ch = |sh: u32| (((c >> sh & 255) as f32 * 0.6).round() as u32) << sh;
                    ch(16) | ch(8) | ch(0)
                };
                [a[0], a[1], a[2], a[3], a[4], a[5], dark(a[0]), dark(a[1]), dark(a[2]), dark(a[3])]
            }
            _ => palette(self.palette),
        }
    }

    /// Changes the type, keeping what still applies.
    pub fn set_kind(&mut self, kind: ChartType) {
        if self.kind == kind {
            return;
        }
        self.kind = kind;
        self.vary_colors = kind.is_round();
        if !kind.has_3d() {
            self.view3d = false;
        }
        if kind == ChartType::Surface {
            self.view3d = true;
        }
        if kind == ChartType::Scatter {
            self.markers = true;
        }
        if !kind.can_stack() {
            self.grouping = Grouping::Clustered;
        }
        if kind == ChartType::Histogram {
            self.gap = 6;
        } else if self.gap == 0 {
            self.gap = 150;
        }
        if kind == ChartType::Scatter && self.x_axis.major_grid {
            self.x_axis.major_grid = false;
        }
        for s in &mut self.series {
            if kind != ChartType::Column && kind != ChartType::Bar && kind != ChartType::Line && kind != ChartType::Area {
                s.kind = None;
                s.secondary = false;
            }
        }
    }

    /// Takes the look of a chart template: type, layout, axes, colors and
    /// the format of each series (repeated when there are more), keeping
    /// this chart's data, place, size and title text.
    pub fn apply_template(&mut self, t: &Chart) {
        let mut c = t.clone();
        c.id = self.id;
        c.name = std::mem::take(&mut self.name);
        c.alt = std::mem::take(&mut self.alt);
        (c.from, c.dx, c.dy, c.anchor, c.width, c.height) = (self.from, self.dx, self.dy, self.anchor, self.width, self.height);
        c.title = std::mem::take(&mut self.title);
        c.source = self.source.take();
        c.by_rows = self.by_rows;
        c.totals = std::mem::take(&mut self.totals);
        for (x, y) in [(&mut c.x_axis, &self.x_axis), (&mut c.y_axis, &self.y_axis), (&mut c.y2_axis, &self.y2_axis)] {
            x.title = y.title.clone();
        }
        let n = t.series.len();
        c.series = std::mem::take(&mut self.series)
            .into_iter()
            .enumerate()
            .map(|(i, mut s)| {
                if n > 0 {
                    let f = &t.series[i % n];
                    s.color = f.color;
                    s.line_color = f.line_color;
                    s.line_width = f.line_width;
                    s.no_fill = f.no_fill;
                    s.no_line = f.no_line;
                    s.marker = f.marker.clone();
                    s.smooth = f.smooth;
                    s.kind = f.kind;
                    s.secondary = f.secondary;
                    s.trendline = f.trendline.clone();
                    s.point_colors = f.point_colors.clone();
                    s.fancy = f.fancy.clone();
                    s.explode = f.explode;
                    s.point_explode = f.point_explode.clone();
                    // Fixed error amounts carry over; ranges belong to the data.
                    s.error_bars = f.error_bars.clone().filter(|e| !matches!(e.amount, ErrorAmount::Custom { .. }));
                }
                s
            })
            .collect();
        *self = c;
    }

    /// Turns the chart into a combo: the last series (of several) becomes a
    /// line on the secondary axis.
    pub fn make_combo(&mut self) {
        self.set_kind(ChartType::Column);
        self.grouping = Grouping::Clustered;
        let n = self.series.len();
        for (i, s) in self.series.iter_mut().enumerate() {
            let line = n > 1 && i == n - 1;
            s.kind = line.then_some(ChartType::Line);
            s.secondary = line;
        }
    }

    pub fn is_combo(&self) -> bool {
        self.series.iter().any(|s| s.kind.is_some_and(|k| k != self.kind))
    }

    pub fn has_secondary(&self) -> bool {
        self.kind.has_axes() && self.series.iter().any(|s| s.secondary)
    }

    /// Type a series is drawn as.
    pub fn series_kind(&self, s: &Series) -> ChartType {
        match s.kind {
            Some(k @ (ChartType::Column | ChartType::Line | ChartType::Area)) if matches!(self.kind, ChartType::Column | ChartType::Line | ChartType::Area) => k,
            Some(k @ (ChartType::Bar | ChartType::Line | ChartType::Area)) if self.kind == ChartType::Bar => k,
            _ => self.kind,
        }
    }

    /// Top-left corner in sheet pixels.
    pub fn pos(&self, sh: &Sheet) -> (f32, f32) {
        (sh.col_x(self.from.col) + self.dx, sh.row_y(self.from.row) + self.dy)
    }

    /// Moves the top-left corner to `(x, y)` sheet pixels.
    pub fn set_pos(&mut self, sh: &Sheet, x: f32, y: f32) {
        let (x, y) = (sane(x, 0.0, f32::MAX, 0.0), sane(y, 0.0, f32::MAX, 0.0));
        let col = sh.col_at(x).min(MAX_COLS - 1);
        let row = sh.row_at(y).min(MAX_ROWS - 1);
        self.from = CellRef::new(row, col);
        self.dx = (x - sh.col_x(col)).max(0.0);
        self.dy = (y - sh.row_y(row)).max(0.0);
    }

    /// The cell under the bottom-right corner.
    pub fn last_cell(&self, sh: &Sheet) -> CellRef {
        let [x, y, w, h] = self.rect(sh);
        CellRef::new(sh.row_at((y + h - 0.5).max(0.0)).min(MAX_ROWS - 1), sh.col_at((x + w - 0.5).max(0.0)).min(MAX_COLS - 1))
    }

    /// x, y, width, height in sheet pixels.
    pub fn rect(&self, sh: &Sheet) -> [f32; 4] {
        let (x, y) = self.pos(sh);
        [x, y, self.width, self.height]
    }

    /// Keeps values usable whatever a file said.
    pub fn sanitize(&mut self) {
        self.width = sane(self.width, 8.0, 1.0e5, 480.0);
        self.height = sane(self.height, 8.0, 1.0e5, 288.0);
        self.dx = sane(self.dx, 0.0, 1.0e7, 0.0);
        self.dy = sane(self.dy, 0.0, 1.0e7, 0.0);
        self.from = CellRef::new(self.from.row.min(MAX_ROWS - 1), self.from.col.min(MAX_COLS - 1));
        self.hole = self.hole.clamp(10, 90);
        self.gap = self.gap.min(500);
        self.overlap = self.overlap.clamp(-100, 100);
        self.first_angle %= 360;
        self.bubble_scale = self.bubble_scale.clamp(1, 300);
        if !self.kind.has_3d() {
            self.view3d = false;
        }
        self.style = self.style.min(STYLES - 1);
        self.palette = self.palette.min(PALETTES - 1);
        self.series.truncate(MAX_SERIES);
        for b in [&mut self.area, &mut self.plot] {
            if b.font_size.is_some_and(|f| !(f.is_finite() && (1.0..=400.0).contains(&f))) {
                b.font_size = None;
            }
        }
        for s in &mut self.series {
            if let Some(Fill::Gradient { stops, angle, .. }) = &mut s.fancy {
                stops.retain(|x| x.0.is_finite());
                stops.iter_mut().for_each(|x| x.0 = x.0.clamp(0.0, 1.0));
                stops.sort_by(|a, b| a.0.total_cmp(&b.0));
                if !angle.is_finite() {
                    *angle = 0.0;
                }
                if stops.is_empty() {
                    s.fancy = None;
                }
            }
            if s.line_width.is_some_and(|w| !(w.is_finite() && (0.0..=20.0).contains(&w))) {
                s.line_width = None;
            }
            if let Some(t) = &mut s.trendline {
                match &mut t.kind {
                    TrendKind::Polynomial(n) => *n = (*n).clamp(2, 6),
                    TrendKind::MovingAverage(n) => *n = (*n).max(2),
                    _ => {}
                }
            }
            if let Some(e) = &mut s.error_bars {
                if let ErrorAmount::Fixed(v) | ErrorAmount::Percent(v) | ErrorAmount::StdDev(v) = &mut e.amount {
                    *v = if v.is_finite() { v.abs().min(1e300) } else { 1.0 };
                }
            }
        }
        match &mut self.bins {
            Bins::Width(w) if !(w.is_finite() && *w > 0.0) => self.bins = Bins::Auto,
            Bins::Count(n) => *n = (*n).clamp(1, 1000),
            _ => {}
        }
        self.x_axis.sanitize();
        self.y_axis.sanitize();
        self.y2_axis.sanitize();
    }

    /// Moves the anchor for rows (or columns) inserted or deleted at `at`.
    pub(crate) fn shift(&mut self, rows: bool, at: u32, count: i64) {
        if self.anchor == Anchor::Absolute {
            return;
        }
        let v = if rows { self.from.row } else { self.from.col };
        if v < at {
            return;
        }
        let max = if rows { MAX_ROWS - 1 } else { MAX_COLS - 1 };
        let nv = if count >= 0 {
            (v as i64 + count).min(max as i64) as u32
        } else if (v as i64) < at as i64 - count {
            if rows {
                self.dy = 0.0;
            } else {
                self.dx = 0.0;
            }
            at
        } else {
            (v as i64 + count) as u32
        };
        if rows {
            self.from.row = nv;
        } else {
            self.from.col = nv;
        }
    }

    /// Every data reference of the chart.
    fn refs_mut(&mut self) -> impl Iterator<Item = &mut DataRef> {
        self.series
            .iter_mut()
            .flat_map(|s| {
                let (plus, minus) = match &mut s.error_bars {
                    Some(ErrorBars { amount: ErrorAmount::Custom { plus, minus }, .. }) => (Some(plus), Some(minus)),
                    _ => (None, None),
                };
                [Some(&mut s.name), Some(&mut s.categories), Some(&mut s.values), Some(&mut s.sizes), Some(&mut s.label_range), plus, minus].into_iter().flatten()
            })
            .chain(self.source.as_mut())
    }
}

fn sane(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

// ---------------------------------------------------------------------------
// Sparklines

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum SparkKind {
    #[default]
    Line,
    Column,
    WinLoss,
}

/// Minimum or maximum of the vertical axis of sparklines.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum SparkAxis {
    /// Each sparkline its own.
    #[default]
    Individual,
    /// The same for the whole group.
    Group,
    Custom(f64),
}

/// A small chart inside a cell.
#[derive(Clone, Debug, PartialEq)]
pub struct Sparkline {
    pub cell: CellRef,
    pub data: DataRef,
}

/// Sparklines sharing their look.
#[derive(Clone, Debug, PartialEq)]
pub struct SparkGroup {
    pub kind: SparkKind,
    pub color: u32,
    pub negative_color: u32,
    pub marker_color: u32,
    pub high_color: u32,
    pub low_color: u32,
    pub first_color: u32,
    pub last_color: u32,
    pub markers: bool,
    pub high: bool,
    pub low: bool,
    pub first: bool,
    pub last: bool,
    pub negative: bool,
    /// Line width in points.
    pub line_weight: f32,
    pub min: SparkAxis,
    pub max: SparkAxis,
    /// Draws the horizontal axis at zero.
    pub show_axis: bool,
    pub blanks: Blanks,
    pub right_to_left: bool,
    /// Hidden rows and columns are left out.
    pub visible_only: bool,
    pub sparklines: Vec<Sparkline>,
}

impl Default for SparkGroup {
    fn default() -> Self {
        // Excel's default look.
        SparkGroup {
            kind: SparkKind::Line,
            color: 0x376092,
            negative_color: 0xD00000,
            marker_color: 0xD00000,
            high_color: 0xD00000,
            low_color: 0xD00000,
            first_color: 0xD00000,
            last_color: 0xD00000,
            markers: false,
            high: false,
            low: false,
            first: false,
            last: false,
            negative: false,
            line_weight: 0.75,
            min: SparkAxis::Individual,
            max: SparkAxis::Individual,
            show_axis: false,
            blanks: Blanks::Gap,
            right_to_left: false,
            visible_only: true,
            sparklines: Vec::new(),
        }
    }
}

impl SparkGroup {
    pub fn sanitize(&mut self) {
        if !(self.line_weight.is_finite() && (0.0..=20.0).contains(&self.line_weight)) {
            self.line_weight = 0.75;
        }
        for a in [&mut self.min, &mut self.max] {
            if matches!(a, SparkAxis::Custom(v) if !v.is_finite()) {
                *a = SparkAxis::Individual;
            }
        }
        for s in &mut self.sparklines {
            s.cell = CellRef::new(s.cell.row.min(MAX_ROWS - 1), s.cell.col.min(MAX_COLS - 1));
        }
    }

    /// Moves the sparklines for rows (or columns) inserted or deleted at
    /// `at`; those in deleted cells go away.
    pub(crate) fn shift(&mut self, rows: bool, at: u32, count: i64) {
        self.sparklines.retain_mut(|s| {
            let v = if rows { &mut s.cell.row } else { &mut s.cell.col };
            if *v < at {
                return true;
            }
            if count < 0 && (*v as i64) < at as i64 - count {
                return false;
            }
            let nv = *v as i64 + count;
            let max = if rows { MAX_ROWS } else { MAX_COLS } as i64;
            if nv >= max {
                return false;
            }
            *v = nv as u32;
            true
        });
    }
}

// ---------------------------------------------------------------------------
// Computed data

/// A chart's data as numbers and labels.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChartData {
    pub series: Vec<SeriesData>,
    /// Category labels (from the first series that has them, else 1, 2...).
    pub categories: Vec<String>,
    /// Number format of the first value cell ("General" when unknown).
    pub value_format: String,
    /// Number format of the first category cell (scatter x axis).
    pub cat_format: String,
    /// Category levels of each point, outermost first (treemap and
    /// sunburst with categories of several columns; else one level).
    pub levels: Vec<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct SeriesData {
    pub name: String,
    pub values: Vec<Option<f64>>,
    /// Numeric x values (scatter); categories are 1, 2... when not numbers.
    pub xs: Vec<Option<f64>>,
    pub categories: Vec<String>,
    /// Bubble sizes.
    pub sizes: Vec<Option<f64>>,
    /// Custom error amounts.
    pub err_plus: Vec<Option<f64>>,
    pub err_minus: Vec<Option<f64>>,
    /// Data label texts from cells.
    pub label_texts: Vec<String>,
}

impl ChartData {
    /// Most points in a series.
    pub fn points(&self) -> usize {
        self.series.iter().map(|s| s.values.len()).max().unwrap_or(0)
    }
}

/// Values read from a reference, with their shown text.
struct Resolved {
    values: Vec<Value>,
    texts: Vec<String>,
    format: Option<String>,
}

impl Workbook {
    /// Values, texts and number format of a data reference evaluated from
    /// sheet `sheet`.
    fn resolve_ref(&self, sheet: usize, r: &DataRef, visible_only: bool) -> Resolved {
        let from_cache = |r: &DataRef| Resolved {
            values: r.cache.clone(),
            texts: r.cache.iter().map(|v| self.format_value(0, v).0).collect(),
            format: None,
        };
        let Some(expr) = &r.formula else { return from_cache(r) };
        if let Some((name, area)) = r.area() {
            let si = match &name {
                Some(n) => self.sheet_index(n),
                None => Some(sheet),
            };
            let Some(si) = si.filter(|&i| i < self.sheets.len()) else { return from_cache(r) };
            let sh = &self.sheets[si];
            // Whole rows and columns: only the used part.
            let ext = sh.extent().unwrap_or_default();
            let end = CellRef::new(area.end.row.min(ext.row.max(area.start.row)), area.end.col.min(ext.col.max(area.start.col)));
            let area = Range::new(area.start, end);
            let mut out = Resolved { values: Vec::new(), texts: Vec::new(), format: None };
            'rows: for row in area.start.row..=area.end.row {
                if visible_only && sh.row_heights.get(&row) == Some(&0.0) {
                    continue;
                }
                for col in area.start.col..=area.end.col {
                    if visible_only && sh.col_widths.get(&col) == Some(&0.0) {
                        continue;
                    }
                    if out.values.len() >= MAX_POINTS {
                        break 'rows;
                    }
                    let c = CellRef::new(row, col);
                    let v = sh.value(c).clone();
                    if out.format.is_none() && !matches!(v, Value::Empty) {
                        let style = sh.get(c).map_or(0, |x| x.style);
                        out.format = Some(self.styles.get(style).num_fmt.to_string());
                    }
                    out.texts.push(self.display(si, c).0);
                    out.values.push(v);
                }
            }
            return out;
        }
        let v = Env::new(self, sheet, CellRef::default(), self.locale).eval(expr);
        let values: Vec<Value> = match v {
            Value::Array(m) => m.data.iter().take(MAX_POINTS).cloned().collect(),
            Value::Error(_) if !r.cache.is_empty() => return from_cache(r),
            v => vec![v],
        };
        Resolved { texts: values.iter().map(|v| self.format_value(0, v).0).collect(), values, format: None }
    }

    /// The numbers and labels a chart shows now.
    pub fn chart_data(&self, sheet: usize, chart: &Chart) -> ChartData {
        let mut data = ChartData { value_format: String::new(), cat_format: String::new(), ..Default::default() };
        let num = |v: &Value| match v {
            Value::Number(n) if n.is_finite() => Some(*n),
            Value::Bool(b) => Some(*b as u8 as f64),
            _ => None,
        };
        for s in &chart.series {
            let vals = self.resolve_ref(sheet, &s.values, chart.visible_only);
            if data.value_format.is_empty() {
                if let Some(f) = vals.format {
                    data.value_format = f;
                }
            }
            let values: Vec<Option<f64>> = vals
                .values
                .iter()
                .map(|v| match (v, chart.blanks) {
                    (Value::Empty, Blanks::Zero) => Some(0.0),
                    // Text in a value cell counts as zero, as in Excel.
                    (Value::Text(_), _) => Some(0.0),
                    (v, _) => num(v),
                })
                .collect();
            let cats = self.resolve_ref(sheet, &s.categories, chart.visible_only);
            if data.cat_format.is_empty() {
                if let Some(f) = cats.format {
                    data.cat_format = f;
                }
            }
            let numeric = !cats.values.is_empty() && cats.values.iter().all(|v| matches!(v, Value::Number(_) | Value::Empty));
            let xs = if chart.kind.is_xy() && numeric {
                cats.values.iter().map(num).collect()
            } else {
                (1..=values.len()).map(|i| Some(i as f64)).collect()
            };
            let name = if s.name.is_empty() {
                String::new()
            } else {
                self.resolve_ref(sheet, &s.name, false).texts.into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ")
            };
            let sizes = if chart.kind == ChartType::Bubble {
                let r = if s.sizes.is_empty() { Vec::new() } else { self.resolve_ref(sheet, &s.sizes, chart.visible_only).values };
                (0..values.len()).map(|j| r.get(j).map_or(Some(1.0), num)).collect()
            } else {
                Vec::new()
            };
            let nums = |r: &DataRef| if r.is_empty() { Vec::new() } else { self.resolve_ref(sheet, r, chart.visible_only).values.iter().map(num).collect() };
            let (err_plus, err_minus) = match &s.error_bars {
                Some(ErrorBars { amount: ErrorAmount::Custom { plus, minus }, .. }) => (nums(plus), nums(minus)),
                _ => (Vec::new(), Vec::new()),
            };
            let label_texts = if chart.labels.cells && !s.label_range.is_empty() { self.resolve_ref(sheet, &s.label_range, chart.visible_only).texts } else { Vec::new() };
            data.series.push(SeriesData { name, values, xs, categories: cats.texts, sizes, err_plus, err_minus, label_texts });
        }
        let n = data.points();
        data.categories = match data.series.iter().find(|s| !s.categories.is_empty()) {
            Some(s) => {
                let mut c = s.categories.clone();
                c.resize(n.max(c.len()), String::new());
                c
            }
            None => (1..=n).map(|i| i.to_string()).collect(),
        };
        data.levels = match self.category_levels(sheet, chart, n) {
            Some(levels) => {
                data.categories = levels.iter().map(|l| l.last().cloned().unwrap_or_default()).collect();
                levels
            }
            None => data.categories.iter().map(|c| vec![c.clone()]).collect(),
        };
        for (i, s) in data.series.iter_mut().enumerate() {
            if s.name.is_empty() {
                s.name = format!("Series{}", i + 1);
            }
        }
        if data.value_format.is_empty() {
            data.value_format = "General".into();
        }
        if data.cat_format.is_empty() {
            data.cat_format = "General".into();
        }
        data
    }

    /// Category levels of a treemap or sunburst whose categories are
    /// several columns (or rows): one path per point, outermost first.
    /// Blank outer cells repeat the one above; blank inner cells are left
    /// out (the point sits on its parent's level).
    fn category_levels(&self, sheet: usize, chart: &Chart, n: usize) -> Option<Vec<Vec<String>>> {
        if !matches!(chart.kind, ChartType::Treemap | ChartType::Sunburst) {
            return None;
        }
        let (name, area) = chart.series.first()?.categories.area()?;
        let si = match &name {
            Some(n) => self.sheet_index(n)?,
            None => sheet,
        };
        self.sheets.get(si)?;
        let (rows, cols) = ((area.end.row - area.start.row + 1) as usize, (area.end.col - area.start.col + 1) as usize);
        // Points down the rows, levels across the columns (or the other way).
        let across = cols > 1 && (rows >= n || rows >= cols);
        let (points, depth) = if across { (rows, cols) } else { (cols, rows) };
        if depth < 2 || depth > 16 {
            return None;
        }
        let points = points.min(n).min(MAX_POINTS);
        let mut out: Vec<Vec<String>> = Vec::with_capacity(points);
        let mut prev: Vec<String> = vec![String::new(); depth];
        for p in 0..points {
            let mut raw: Vec<String> = (0..depth)
                .map(|k| {
                    let c = if across { CellRef::new(area.start.row + p as u32, area.start.col + k as u32) } else { CellRef::new(area.start.row + k as u32, area.start.col + p as u32) };
                    self.display(si, c).0
                })
                .collect();
            // Fill down the outer levels while the row starts blank.
            let first = raw.iter().position(|t| !t.is_empty()).unwrap_or(depth);
            for k in 0..first.min(depth - 1) {
                raw[k] = prev[k].clone();
            }
            prev = raw.clone();
            out.push(raw.into_iter().filter(|t| !t.is_empty()).collect());
        }
        out.resize(n, Vec::new());
        Some(out)
    }

    /// Values of one sparkline.
    pub fn sparkline_data(&self, sheet: usize, group: &SparkGroup, s: &Sparkline) -> Vec<Option<f64>> {
        self.resolve_ref(sheet, &s.data, group.visible_only)
            .values
            .iter()
            .map(|v| match (v, group.blanks) {
                (Value::Number(n), _) if n.is_finite() => Some(*n),
                (Value::Empty, Blanks::Zero) => Some(0.0),
                _ => None,
            })
            .collect()
    }

    /// A chart of the data in `range` (as Insert > Chart does with the
    /// selection): the first row and column hold names and categories when
    /// they are text; series go down the columns unless there are more
    /// columns than rows (or `by_rows` says otherwise).
    pub fn chart_from_range(&self, sheet: usize, range: Range, kind: ChartType, by_rows: Option<bool>) -> Chart {
        let mut chart = Chart::new(kind, CellRef::new(range.start.row, (range.end.col + 2).min(MAX_COLS - 1)), 480.0, 288.0);
        let range = self.clamp_to_used(sheet, &range).map_or(range, |u| Range::new(range.start, u.end));
        let by_rows = by_rows.unwrap_or(range.cols() > range.rows() && !kind.is_xy());
        chart.source = Some(DataRef::range(&self.sheets[sheet].name, range));
        chart.by_rows = by_rows;
        self.chart_series(sheet, &mut chart, range, by_rows);
        // Excel's new charts: legend at the bottom, none for a single
        // series (its name is the title) or a histogram.
        chart.legend = if kind == ChartType::Histogram || (chart.series.len() == 1 && !kind.is_round() && kind != ChartType::Waterfall) { LegendPos::None } else { LegendPos::Bottom };
        if matches!(kind, ChartType::Stock | ChartType::Funnel | ChartType::Treemap | ChartType::Pareto | ChartType::Timeline) {
            chart.legend = LegendPos::None;
        }
        if kind == ChartType::Funnel {
            chart.labels.value = true;
        }
        chart
    }

    /// Rebuilds the series of a chart from its source range, series in rows
    /// or columns (Switch Row/Column).
    pub fn rebuild_series(&self, sheet: usize, chart: &mut Chart, by_rows: bool) {
        let Some((name, range)) = chart.source.as_ref().and_then(|s| s.area()) else { return };
        let si = name.as_deref().and_then(|n| self.sheet_index(n)).unwrap_or(sheet);
        let old: Vec<Series> = std::mem::take(&mut chart.series);
        chart.by_rows = by_rows;
        self.chart_series(si, chart, range, by_rows);
        // Keep the look of the series that remain.
        for (s, o) in chart.series.iter_mut().zip(old) {
            s.color = o.color;
            s.line_color = o.line_color;
            s.line_width = o.line_width;
            s.marker = o.marker;
            s.kind = o.kind;
            s.secondary = o.secondary;
            s.trendline = o.trendline;
        }
    }

    fn chart_series(&self, sheet: usize, chart: &mut Chart, range: Range, by_rows: bool) {
        let sh = &self.sheets[sheet];
        let name = sh.name.clone();
        let is_text = |c: CellRef| matches!(sh.value(c), Value::Text(_) | Value::Empty);
        // Transposed view: `lines` are the series, `points` run along them.
        let (lines, points) = if by_rows { (range.rows(), range.cols()) } else { (range.cols(), range.rows()) };
        let at = |line: u32, point: u32| if by_rows { CellRef::new(range.start.row + line, range.start.col + point) } else { CellRef::new(range.start.row + point, range.start.col + line) };
        let span = |line: u32, a: u32, b: u32| -> Range { Range::new(at(line, a), at(line, b)) };
        let scatter = chart.kind.is_xy();
        // A timeline is a column of dates and a column of labels.
        if chart.kind == ChartType::Timeline && lines >= 2 {
            let p0 = if is_text(at(0, 0)) && points > 1 { 1 } else { 0 };
            let mut s = Series { values: DataRef::range(&name, span(0, p0, points - 1)), categories: DataRef::range(&name, span(1, p0, points - 1)), ..Default::default() };
            if p0 == 1 {
                s.name = DataRef::range(&name, Range::single(at(0, 0)));
            }
            chart.series.push(s);
            return;
        }
        // Names: the first point of every line is text (and there is more).
        let has_names = points > 1 && (0..lines).all(|l| is_text(at(l, 0))) && (0..lines).any(|l| !matches!(sh.value(at(l, 0)), Value::Empty));
        // Categories: the first line is text, or the corner is empty with
        // names above, or scatter x values.
        let first_line_text = (if has_names { 1 } else { 0 }..points).all(|p| is_text(at(0, p)));
        let corner_empty = matches!(sh.value(at(0, 0)), Value::Empty);
        let has_cats = lines > 1 && (first_line_text || (has_names && corner_empty) || scatter);
        let p0 = if has_names { 1 } else { 0 };
        let l0 = if has_cats { 1 } else { 0 };
        // Treemaps and sunbursts take every leading text line as a level
        // of the categories.
        let levels = if matches!(chart.kind, ChartType::Treemap | ChartType::Sunburst) && has_cats {
            ((0..lines).take_while(|&l| (p0..points).all(|p| is_text(at(l, p)))).count() as u32).clamp(1, lines - 1)
        } else {
            1
        };
        let cats = if has_cats && p0 < points { DataRef::range(&name, Range::new(at(0, p0), at(levels - 1, points - 1))) } else { DataRef::default() };
        let l0 = if has_cats { levels } else { l0 };
        // Bubbles take their lines in pairs: y values, then sizes.
        let step = if chart.kind == ChartType::Bubble { 2 } else { 1 };
        for l in (l0..lines).step_by(step) {
            if chart.series.len() >= MAX_SERIES || p0 >= points {
                break;
            }
            let mut s = Series { values: DataRef::range(&name, span(l, p0, points - 1)), categories: cats.clone(), ..Default::default() };
            if step == 2 && l + 1 < lines {
                s.sizes = DataRef::range(&name, span(l + 1, p0, points - 1));
            }
            if has_names {
                s.name = DataRef::range(&name, Range::single(at(l, 0)));
            }
            chart.series.push(s);
        }
        if chart.series.is_empty() && p0 < points {
            // A single line of numbers.
            chart.series.push(Series { values: DataRef::range(&name, span(0, p0, points - 1)), ..Default::default() });
        }
    }

    // --- undoable edits ---------------------------------------------------------------

    /// Adds a chart on top of the others (undoable); returns its id.
    pub fn add_chart(&mut self, sheet: usize, mut chart: Chart) -> u32 {
        let sh = &self.sheets[sheet];
        chart.id = sh.charts.iter().map(|c| c.id).max().unwrap_or(0) + 1;
        if chart.name.is_empty() {
            chart.name = format!("Chart {}", chart.id);
        }
        chart.sanitize();
        let id = chart.id;
        let mut list = sh.charts.clone();
        list.push(chart);
        self.execute_as(Op::Charts { sheet, list }, "undo.insert_chart");
        id
    }

    /// Replaces the chart with the same id (undoable).
    pub fn set_chart(&mut self, sheet: usize, mut chart: Chart) {
        chart.sanitize();
        let sh = &self.sheets[sheet];
        let Some(i) = sh.charts.iter().position(|x| x.id == chart.id) else { return };
        if sh.charts[i] == chart {
            return;
        }
        let mut list = sh.charts.clone();
        list[i] = chart;
        self.execute_as(Op::Charts { sheet, list }, "undo.chart");
    }

    pub fn remove_chart(&mut self, sheet: usize, id: u32) {
        let mut list = self.sheets[sheet].charts.clone();
        let n = list.len();
        list.retain(|x| x.id != id);
        if list.len() != n {
            self.execute_as(Op::Charts { sheet, list }, "undo.delete_chart");
        }
    }

    /// Replaces the sparkline groups of a sheet (undoable).
    pub fn set_sparklines(&mut self, sheet: usize, mut list: Vec<SparkGroup>) {
        list.retain(|g| !g.sparklines.is_empty());
        for g in &mut list {
            g.sanitize();
        }
        if self.sheets[sheet].sparklines != list {
            self.execute_as(Op::Sparklines { sheet, list }, "undo.sparklines");
        }
    }

    /// Adds sparklines, replacing any in the same cells.
    pub fn add_sparklines(&mut self, sheet: usize, group: SparkGroup) {
        let cells: Vec<CellRef> = group.sparklines.iter().map(|s| s.cell).collect();
        let mut list = self.sheets[sheet].sparklines.clone();
        for g in &mut list {
            g.sparklines.retain(|s| !cells.contains(&s.cell));
        }
        list.push(group);
        self.set_sparklines(sheet, list);
    }

    /// Removes the sparklines in `range`.
    pub fn clear_sparklines(&mut self, sheet: usize, range: &Range) {
        let mut list = self.sheets[sheet].sparklines.clone();
        for g in &mut list {
            g.sparklines.retain(|s| !range.contains(s.cell));
        }
        self.set_sparklines(sheet, list);
    }

    // --- following structure changes ----------------------------------------------------

    /// Snapshots of the charts and sparklines of every sheet that has some
    /// (restored on undo of a deletion, which shifting back can't do).
    pub(crate) fn object_snapshots(&self) -> Vec<Op> {
        let mut ops = Vec::new();
        for (sheet, sh) in self.sheets.iter().enumerate() {
            if !sh.charts.is_empty() {
                ops.push(Op::Charts { sheet, list: sh.charts.clone() });
            }
            if !sh.sparklines.is_empty() {
                ops.push(Op::Sparklines { sheet, list: sh.sparklines.clone() });
            }
            if !sh.shapes.is_empty() {
                ops.push(Op::Shapes { sheet, list: sh.shapes.clone() });
            }
        }
        ops
    }

    fn each_object_ref(&mut self, mut f: impl FnMut(&str, &mut Expr) -> bool) {
        for sh in &mut self.sheets {
            let own = sh.name.clone();
            for c in &mut sh.charts {
                for r in c.refs_mut() {
                    r.edit_formula(|e| f(&own, e));
                }
            }
            for g in &mut sh.sparklines {
                for s in &mut g.sparklines {
                    s.data.edit_formula(|e| f(&own, e));
                }
            }
        }
    }

    /// Rewrites chart and sparkline references for rows or columns
    /// inserted or deleted in sheet `target`.
    pub(crate) fn objects_structure(&mut self, target: &str, rows: bool, at: u32, count: i64) {
        self.each_object_ref(|own, e| e.adjust_structure(own, target, rows, at, count));
        self.names_edit(|own, e| e.adjust_structure(own, target, rows, at, count));
    }

    pub(crate) fn objects_rows_at(&mut self, target: &str, points: &[u32], insert: bool) {
        self.each_object_ref(|own, e| e.adjust_rows_at(own, target, points, insert));
        self.names_edit(|own, e| e.adjust_rows_at(own, target, points, insert));
    }

    pub(crate) fn objects_rename(&mut self, old: &str, new: &str) {
        self.each_object_ref(|_, e| e.rename_sheet(old, new));
        self.names_rename_sheet(old, new);
    }
}

// ---------------------------------------------------------------------------
// Math shared by the painter and the files

/// Axis scale: first and last value and the step between labels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
    pub major: f64,
    pub minor: f64,
    pub log: Option<f64>,
}

impl Scale {
    /// Positions of the major ticks.
    pub fn ticks(&self) -> Vec<f64> {
        self.steps(self.major)
    }

    pub fn minor_ticks(&self) -> Vec<f64> {
        self.steps(self.minor)
    }

    fn steps(&self, step: f64) -> Vec<f64> {
        let mut v = Vec::new();
        if let Some(b) = self.log {
            let (a, z) = (self.min.log(b), self.max.log(b));
            let mut e = a;
            while e <= z + 1e-9 && v.len() < 200 {
                v.push(b.powf(e));
                e += step.max(1.0).round();
            }
            return v;
        }
        if !(step > 0.0 && step.is_finite()) {
            return vec![self.min, self.max];
        }
        let n = ((self.max - self.min) / step + 1e-9).floor();
        if !(0.0..=1000.0).contains(&n) {
            return vec![self.min, self.max];
        }
        for i in 0..=n as usize {
            let x = self.min + i as f64 * step;
            // Clean -0.0000001 and friends.
            v.push(if (x / step).abs() < 1e-9 { 0.0 } else { x });
        }
        v
    }

    /// Where `v` falls, 0..1 from min to max.
    pub fn frac(&self, v: f64) -> f64 {
        match self.log {
            Some(b) if v > 0.0 => (v.log(b) - self.min.log(b)) / (self.max.log(b) - self.min.log(b)),
            Some(_) => 0.0,
            None => (v - self.min) / (self.max - self.min),
        }
    }
}

/// A "nice" step near `raw` (1, 2, 2.5 or 5 times a power of ten).
pub fn nice_step(raw: f64) -> f64 {
    if !(raw.is_finite() && raw > 0.0) {
        return 1.0;
    }
    let p = 10f64.powf(raw.log10().floor());
    let f = raw / p;
    let n = if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 2.5 {
        2.5
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    };
    n * p
}

/// The scale for data from `lo` to `hi` with about `ticks` labels, as
/// Excel picks it: zero is included when the data are all on one side and
/// not far from it. Fixed values of the axis win.
pub fn scale_for(lo: f64, hi: f64, ticks: usize, axis: &Axis) -> Scale {
    if let Some(b) = axis.log_base {
        let b = if b.is_finite() && b >= 2.0 { b } else { 10.0 };
        let lo = if lo > 0.0 { lo } else { 1.0 };
        let hi = if hi > lo { hi } else { lo * b };
        let min = axis.min.filter(|v| *v > 0.0).unwrap_or_else(|| b.powf(lo.log(b).floor()));
        let mut max = axis.max.filter(|v| *v > min).unwrap_or_else(|| b.powf(hi.log(b).ceil()));
        if max <= min {
            max = min * b;
        }
        return Scale { min, max, major: 1.0, minor: 1.0, log: Some(b) };
    }
    let (mut lo, mut hi) = if lo.is_finite() && hi.is_finite() && lo <= hi { (lo, hi) } else { (0.0, 1.0) };
    if let Some(m) = axis.min {
        lo = m;
    }
    if let Some(m) = axis.max {
        hi = m;
    }
    // Excel starts at zero unless the data are bunched far from it (their
    // spread under a sixth of the largest value).
    if axis.min.is_none() && lo > 0.0 && hi - lo >= hi / 6.0 {
        lo = 0.0;
    }
    if axis.max.is_none() && hi < 0.0 && hi - lo >= -lo / 6.0 {
        hi = 0.0;
    }
    if hi <= lo {
        if axis.max.is_none() {
            hi = if lo == 0.0 { 1.0 } else { lo + lo.abs() };
        } else {
            lo = hi - hi.abs().max(1.0);
        }
    }
    let major = axis.major.filter(|m| (hi - lo) / m <= 1000.0).unwrap_or_else(|| nice_step((hi - lo) / ticks.max(1) as f64));
    let min = if axis.min.is_some() { lo } else { (lo / major).floor() * major };
    let mut max = if axis.max.is_some() { hi } else { (hi / major).ceil() * major };
    if max <= min {
        max = min + major;
    }
    let minor = axis.minor.filter(|m| (max - min) / m <= 5000.0).unwrap_or(major / 5.0);
    Scale { min, max, major, minor, log: None }
}

/// Histogram bins of `values`: (low, high, count). The last bin includes
/// its upper end.
pub fn histogram(values: &[f64], bins: Bins) -> Vec<(f64, f64, u64)> {
    let v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return Vec::new();
    }
    let lo = v.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (width, count) = match bins {
        Bins::Width(w) if w > 0.0 => (w, (((hi - lo) / w).ceil() as usize).clamp(1, 10_000)),
        Bins::Count(n) => {
            let n = n.clamp(1, 1000) as usize;
            (if hi > lo { (hi - lo) / n as f64 } else { 1.0 }, n)
        }
        _ => {
            // Scott's rule, as Excel uses.
            let n = v.len() as f64;
            let mean = v.iter().sum::<f64>() / n;
            let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0)).sqrt();
            let w = 3.5 * sd / n.cbrt();
            if w > 0.0 && hi > lo {
                (w, (((hi - lo) / w).ceil() as usize).clamp(1, 10_000))
            } else {
                (1.0, 1)
            }
        }
    };
    let mut out: Vec<(f64, f64, u64)> = (0..count).map(|i| (lo + i as f64 * width, lo + (i + 1) as f64 * width, 0)).collect();
    for x in v {
        let mut i = ((x - lo) / width).floor() as usize;
        // Bins include their upper end (Excel's "(a, b]"), the first also
        // its lower end.
        if i > 0 && (x - (lo + i as f64 * width)).abs() < width * 1e-9 {
            i -= 1;
        }
        let i = i.min(count - 1);
        out[i].2 += 1;
    }
    out
}

/// A fitted trendline.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    pub kind: TrendKind,
    /// Linear, polynomial: c0 + c1 x + c2 x^2...; exponential: c0 e^(c1 x);
    /// logarithmic: c0 + c1 ln x; power: c0 x^c1.
    pub coef: Vec<f64>,
    pub r2: f64,
}

impl Fit {
    pub fn eval(&self, x: f64) -> Option<f64> {
        let c = &self.coef;
        let y = match self.kind {
            TrendKind::Linear | TrendKind::Polynomial(_) => c.iter().rev().fold(0.0, |acc, k| acc * x + k),
            TrendKind::Exponential => c[0] * (c[1] * x).exp(),
            TrendKind::Logarithmic if x > 0.0 => c[0] + c[1] * x.ln(),
            TrendKind::Power if x > 0.0 => c[0] * x.powf(c[1]),
            _ => return None,
        };
        y.is_finite().then_some(y)
    }

    /// "y = 2.5x + 1" style text.
    pub fn equation(&self, loc: &Locale) -> String {
        let n = |v: f64| {
            let s = format!("{:.4}", v);
            let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
            if loc.decimal == ',' { s.replace('.', ",") } else { s }
        };
        let c = &self.coef;
        match self.kind {
            TrendKind::Linear | TrendKind::Polynomial(_) => {
                let mut s = String::from("y =");
                for (i, k) in c.iter().enumerate().rev() {
                    let sign = if k < &0.0 { " - " } else if s.len() > 3 { " + " } else { " " };
                    let body = match i {
                        0 => n(k.abs()),
                        1 => format!("{}x", n(k.abs())),
                        _ => format!("{}x^{i}", n(k.abs())),
                    };
                    s.push_str(sign);
                    s.push_str(&body);
                }
                s
            }
            TrendKind::Exponential => format!("y = {}e^({}x)", n(c[0]), n(c[1])),
            TrendKind::Logarithmic => format!("y = {}ln(x) + {}", n(c[1]), n(c[0])),
            TrendKind::Power => format!("y = {}x^{}", n(c[0]), n(c[1])),
            TrendKind::MovingAverage(_) => String::new(),
        }
    }
}

/// Least squares polynomial of degree `deg` through the points.
fn polyfit(xs: &[f64], ys: &[f64], deg: usize) -> Option<Vec<f64>> {
    let m = deg + 1;
    if xs.len() < m {
        return None;
    }
    // Normal equations, solved by Gaussian elimination with pivoting.
    let mut a = vec![vec![0.0; m + 1]; m];
    for (&x, &y) in xs.iter().zip(ys) {
        let mut p = vec![1.0; 2 * m];
        for i in 1..2 * m {
            p[i] = p[i - 1] * x;
        }
        for r in 0..m {
            for c in 0..m {
                a[r][c] += p[r + c];
            }
            a[r][m] += p[r] * y;
        }
    }
    for col in 0..m {
        let piv = (col..m).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        for r in 0..m {
            if r != col {
                let f = a[r][col] / a[col][col];
                for c in col..=m {
                    a[r][c] -= f * a[col][c];
                }
            }
        }
    }
    let coef: Vec<f64> = (0..m).map(|i| a[i][m] / a[i][i]).collect();
    coef.iter().all(|c| c.is_finite()).then_some(coef)
}

/// Fits a trendline to the points with both coordinates.
pub fn fit_trend(kind: TrendKind, xs: &[Option<f64>], ys: &[Option<f64>]) -> Option<Fit> {
    let pts: Vec<(f64, f64)> = xs.iter().zip(ys).filter_map(|(x, y)| Some(((*x)?, (*y)?))).collect();
    let keep = |f: &dyn Fn(f64, f64) -> bool| -> (Vec<f64>, Vec<f64>) { pts.iter().filter(|(x, y)| f(*x, *y)).copied().unzip() };
    let coef = match kind {
        TrendKind::Linear => {
            let (x, y) = keep(&|_, _| true);
            polyfit(&x, &y, 1)?
        }
        TrendKind::Polynomial(n) => {
            let (x, y) = keep(&|_, _| true);
            polyfit(&x, &y, n.clamp(2, 6) as usize)?
        }
        TrendKind::Exponential => {
            let (x, y) = keep(&|_, y| y > 0.0);
            let ly: Vec<f64> = y.iter().map(|v| v.ln()).collect();
            let c = polyfit(&x, &ly, 1)?;
            vec![c[0].exp(), c[1]]
        }
        TrendKind::Logarithmic => {
            let (x, y) = keep(&|x, _| x > 0.0);
            let lx: Vec<f64> = x.iter().map(|v| v.ln()).collect();
            polyfit(&lx, &y, 1)?
        }
        TrendKind::Power => {
            let (x, y) = keep(&|x, y| x > 0.0 && y > 0.0);
            let lx: Vec<f64> = x.iter().map(|v| v.ln()).collect();
            let ly: Vec<f64> = y.iter().map(|v| v.ln()).collect();
            let c = polyfit(&lx, &ly, 1)?;
            vec![c[0].exp(), c[1]]
        }
        TrendKind::MovingAverage(_) => return None,
    };
    let mut fit = Fit { kind, coef, r2: 0.0 };
    // R² on the values the fit was made for (log space for exp/power, as
    // Excel reports it).
    let tr = |v: f64| if matches!(kind, TrendKind::Exponential | TrendKind::Power) { v.ln() } else { v };
    let used: Vec<(f64, f64)> = pts.iter().copied().filter(|(x, y)| fit.eval(*x).is_some() && (!matches!(kind, TrendKind::Exponential | TrendKind::Power) || *y > 0.0)).collect();
    if !used.is_empty() {
        let mean = used.iter().map(|(_, y)| tr(*y)).sum::<f64>() / used.len() as f64;
        let ss_tot: f64 = used.iter().map(|(_, y)| (tr(*y) - mean).powi(2)).sum();
        let ss_res: f64 = used.iter().filter_map(|(x, y)| Some((tr(*y) - tr(fit.eval(*x)?)).powi(2))).sum();
        fit.r2 = if ss_tot > 0.0 { 1.0 - ss_res / ss_tot } else { 1.0 };
    }
    Some(fit)
}

/// Moving average of `period` points (None until there are enough).
pub fn moving_average(ys: &[Option<f64>], period: usize) -> Vec<Option<f64>> {
    let p = period.max(1);
    (0..ys.len())
        .map(|i| {
            if i + 1 < p {
                return None;
            }
            let w: Vec<f64> = ys[i + 1 - p..=i].iter().flatten().copied().collect();
            (!w.is_empty()).then(|| w.iter().sum::<f64>() / w.len() as f64)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_keeps_data() {
        let mut t = Chart::new(ChartType::Line, CellRef::new(0, 0), 400.0, 300.0);
        t.markers = true;
        t.title = "Template".into();
        t.legend = LegendPos::Bottom;
        let mut s = Series::default();
        s.color = Some(0xFF0000);
        t.series.push(s);
        let mut c = Chart::new(ChartType::Column, CellRef::new(5, 5), 480.0, 288.0);
        c.title = "Sales".into();
        for _ in 0..2 {
            c.series.push(Series::default());
        }
        c.series[1].values = DataRef::parse("Sheet1!B2:B9");
        c.apply_template(&t);
        assert_eq!((c.kind, c.markers, c.legend), (ChartType::Line, true, LegendPos::Bottom));
        assert_eq!((c.title.as_str(), c.from, c.width), ("Sales", CellRef::new(5, 5), 480.0));
        assert_eq!(c.series.len(), 2);
        assert_eq!(c.series[1].color, Some(0xFF0000));
        assert_ne!(c.series[1].values, DataRef::default());
    }

    #[test]
    fn mekko_and_timeline_defaults() {
        assert!(ChartType::Mekko.is_apex_only() && ChartType::Timeline.is_apex_only());
        assert!(!ChartType::Mekko.is_chartex() && !ChartType::Timeline.has_axes());
        for k in [ChartType::Mekko, ChartType::Timeline] {
            assert_eq!(ChartType::ALL.iter().filter(|t| **t == k).count(), 1);
            assert_eq!(ChartType::ALL.into_iter().find(|t| t.key() == k.key()), Some(k));
        }
        let mut wb = Workbook::new();
        for (r, (d, l)) in [("Date", "Event"), ("45300", "Start"), ("45400", "End")].iter().enumerate() {
            wb.set_input(0, CellRef::new(r as u32, 0), d);
            wb.set_input(0, CellRef::new(r as u32, 1), l);
        }
        let c = wb.chart_from_range(0, Range::parse("A1:B3").unwrap(), ChartType::Timeline, None);
        assert_eq!(c.series.len(), 1);
        let data = wb.chart_data(0, &c);
        assert_eq!(data.series[0].values, [Some(45300.0), Some(45400.0)]);
        assert_eq!(data.categories, ["Start", "End"]);
    }

    #[test]
    fn error_bar_ends() {
        let v = [Some(10.0), None, Some(20.0), Some(30.0)];
        let e = |amount, dir| ErrorBars { amount, dir, cap: true }.ends(&v, &[Some(1.0), None, Some(2.0), None], &[Some(3.0), None, None, Some(4.0)]);
        assert_eq!(e(ErrorAmount::Fixed(2.0), ErrorDir::Both), [Some((8.0, 12.0)), None, Some((18.0, 22.0)), Some((28.0, 32.0))]);
        assert_eq!(e(ErrorAmount::Percent(10.0), ErrorDir::Plus)[2], Some((20.0, 22.0)));
        assert_eq!(e(ErrorAmount::Percent(10.0), ErrorDir::Minus)[3], Some((27.0, 30.0)));
        // Standard deviation (10) around the mean (20); standard error sd/sqrt(n).
        assert_eq!(e(ErrorAmount::StdDev(1.0), ErrorDir::Both)[0], Some((10.0, 30.0)));
        let se = 10.0 / 3f64.sqrt();
        assert_eq!(e(ErrorAmount::StdErr, ErrorDir::Both)[2], Some((20.0 - se, 20.0 + se)));
        let custom = e(ErrorAmount::Custom { plus: DataRef::default(), minus: DataRef::default() }, ErrorDir::Both);
        assert_eq!(custom, [Some((7.0, 11.0)), None, Some((20.0, 22.0)), Some((26.0, 30.0))]);
    }

    #[test]
    fn error_and_label_ranges_come_from_cells() {
        let mut wb = Workbook::new();
        for (r, row) in [["a", "1", "0.5", "x"], ["b", "2", "1", "y"]].iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                wb.set_input(0, CellRef::new(r as u32, c as u32), v);
            }
        }
        let mut c = wb.chart_from_range(0, Range::parse("A1:B2").unwrap(), ChartType::Column, Some(false));
        c.labels.cells = true;
        c.series[0].label_range = DataRef::parse("Sheet1!$D$1:$D$2");
        c.series[0].error_bars = Some(ErrorBars { amount: ErrorAmount::Custom { plus: DataRef::parse("Sheet1!$C$1:$C$2"), minus: DataRef::default() }, ..Default::default() });
        let d = wb.chart_data(0, &c);
        assert_eq!(d.series[0].label_texts, ["x", "y"]);
        assert_eq!(d.series[0].err_plus, [Some(0.5), Some(1.0)]);
        assert!(d.series[0].err_minus.is_empty());
        // Inserting a column moves the ranges with their cells.
        wb.add_chart(0, c);
        wb.execute(crate::Op::InsertCols { sheet: 0, at: 0, count: 1 });
        let s = &wb.sheets[0].charts[0].series[0];
        assert_eq!(s.label_range.to_canonical().as_deref(), Some("Sheet1!$E$1:$E$2"));
        let Some(ErrorBars { amount: ErrorAmount::Custom { plus, .. }, .. }) = &s.error_bars else { panic!() };
        assert_eq!(plus.to_canonical().as_deref(), Some("Sheet1!$D$1:$D$2"));
    }

    fn book() -> Workbook {
        let mut wb = Workbook::new();
        wb.locale = Locale::EN;
        let rows = [["", "North", "South"], ["Jan", "10", "4"], ["Feb", "20", "5"], ["Mar", "15", "=B4/3"]];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                if !v.is_empty() {
                    wb.set_input(0, CellRef::new(r as u32, c as u32), v);
                }
            }
        }
        wb
    }

    #[test]
    fn series_from_a_selection() {
        let wb = book();
        let r = Range::parse("A1:C4").unwrap();
        let chart = wb.chart_from_range(0, r, ChartType::Column, None);
        assert!(!chart.by_rows);
        assert_eq!(chart.series.len(), 2);
        assert_eq!(chart.series[0].values.to_canonical().unwrap(), "Sheet1!$B$2:$B$4");
        assert_eq!(chart.series[0].name.to_canonical().unwrap(), "Sheet1!$B$1");
        assert_eq!(chart.series[1].categories.to_canonical().unwrap(), "Sheet1!$A$2:$A$4");
        let d = wb.chart_data(0, &chart);
        assert_eq!(d.categories, ["Jan", "Feb", "Mar"]);
        assert_eq!(d.series[0].name, "North");
        assert_eq!(d.series[1].values, [Some(4.0), Some(5.0), Some(5.0)]);
        // Series in rows.
        let t = wb.chart_from_range(0, r, ChartType::Column, Some(true));
        assert_eq!(t.series.len(), 3);
        let d = wb.chart_from_range(0, r, ChartType::Line, Some(true));
        let d = wb.chart_data(0, &d);
        assert_eq!((d.series[0].name.as_str(), d.categories.as_slice()), ("Jan", &["North".to_string(), "South".to_string()][..]));
        // Only numbers.
        let n = wb.chart_from_range(0, Range::parse("B2:B4").unwrap(), ChartType::Pie, None);
        assert_eq!(n.series.len(), 1);
        assert!(n.series[0].name.is_empty());
        assert_eq!(wb.chart_data(0, &n).categories, ["1", "2", "3"]);
        let mut sw = chart.clone();
        wb.rebuild_series(0, &mut sw, true);
        assert_eq!(sw.series.len(), 3);
    }

    #[test]
    fn live_data_and_structure() {
        let mut wb = book();
        let chart = wb.chart_from_range(0, Range::parse("A1:C4").unwrap(), ChartType::Line, None);
        let id = wb.add_chart(0, chart);
        wb.set_input(0, CellRef::new(1, 1), "99");
        let c = wb.sheets[0].charts[0].clone();
        assert_eq!(wb.chart_data(0, &c).series[0].values[0], Some(99.0));
        // A row inserted above the data moves the references and the chart.
        let from = c.from;
        wb.execute(Op::InsertRows { sheet: 0, at: 0, count: 2 });
        let c = wb.sheets[0].charts[0].clone();
        assert_eq!(c.series[0].values.to_canonical().unwrap(), "Sheet1!$B$4:$B$6");
        assert_eq!(c.from.row, from.row + 2);
        assert_eq!(wb.chart_data(0, &c).series[0].values[0], Some(99.0));
        // Deleting data rows shrinks the reference; undo restores it.
        wb.execute(Op::DeleteRows { sheet: 0, at: 4, count: 1 });
        assert_eq!(wb.sheets[0].charts[0].series[0].values.to_canonical().unwrap(), "Sheet1!$B$4:$B$5");
        wb.undo();
        assert_eq!(wb.sheets[0].charts[0].series[0].values.to_canonical().unwrap(), "Sheet1!$B$4:$B$6");
        // Renaming the sheet renames the references.
        wb.execute(Op::RenameSheet { index: 0, name: "Data Q1".into() });
        assert_eq!(wb.sheets[0].charts[0].series[0].values.to_canonical().unwrap(), "'Data Q1'!$B$4:$B$6");
        let c = wb.sheets[0].charts[0].clone();
        assert_eq!(wb.chart_data(0, &c).series[0].values.len(), 3);
        // Edits and undo.
        let mut c2 = c.clone();
        c2.title = "Sales".into();
        wb.set_chart(0, c2);
        assert_eq!(wb.sheets[0].charts[0].title, "Sales");
        wb.undo();
        assert_eq!(wb.sheets[0].charts[0].title, "");
        wb.remove_chart(0, id);
        assert!(wb.sheets[0].charts.is_empty());
        wb.undo();
        assert_eq!(wb.sheets[0].charts.len(), 1);
    }

    #[test]
    fn hidden_rows_and_fallbacks() {
        let mut wb = book();
        let chart = wb.chart_from_range(0, Range::parse("A1:B4").unwrap(), ChartType::Column, None);
        wb.execute(Op::RowHeights { sheet: 0, sizes: vec![(2, Some(0.0))] });
        let d = wb.chart_data(0, &chart);
        assert_eq!(d.series[0].values, [Some(10.0), Some(15.0)]);
        assert_eq!(d.categories, ["Jan", "Mar"]);
        // A reference to a missing sheet uses the cached values.
        let mut c = chart.clone();
        c.series[0].values = DataRef::parse("[1]Other!A1:A3");
        c.series[0].values.cache = vec![Value::Number(1.0), Value::Number(2.0)];
        assert_eq!(wb.chart_data(0, &c).series[0].values, [Some(1.0), Some(2.0)]);
        c.series[0].values = DataRef::parse("Nope!A1:A2");
        c.series[0].values.cache = vec![Value::Number(7.0)];
        assert_eq!(wb.chart_data(0, &c).series[0].values, [Some(7.0)]);
        // Junk everywhere does not panic.
        c.width = f32::NAN;
        c.hole = 255;
        c.x_axis.min = Some(f64::NAN);
        c.y_axis.min = Some(5.0);
        c.y_axis.max = Some(1.0);
        c.sanitize();
        assert!(c.width >= 8.0 && c.x_axis.min.is_none() && c.y_axis.max.is_none());
    }

    #[test]
    fn sparklines_follow_cells() {
        let mut wb = book();
        let g = SparkGroup { sparklines: vec![Sparkline { cell: CellRef::new(1, 4), data: DataRef::parse("B2:C2") }, Sparkline { cell: CellRef::new(2, 4), data: DataRef::parse("B3:C3") }], ..Default::default() };
        wb.add_sparklines(0, g);
        let sh = &wb.sheets[0];
        assert_eq!(wb.sparkline_data(0, &sh.sparklines[0], &sh.sparklines[0].sparklines[0]), [Some(10.0), Some(4.0)]);
        wb.execute(Op::DeleteRows { sheet: 0, at: 1, count: 1 });
        let g = &wb.sheets[0].sparklines[0];
        assert_eq!(g.sparklines.len(), 1);
        assert_eq!((g.sparklines[0].cell, g.sparklines[0].data.to_canonical().unwrap()), (CellRef::new(1, 4), "B2:C2".to_string()));
        wb.undo();
        assert_eq!(wb.sheets[0].sparklines[0].sparklines.len(), 2);
        wb.clear_sparklines(0, &Range::parse("E1:E10").unwrap());
        assert!(wb.sheets[0].sparklines.is_empty());
    }

    #[test]
    fn scales_bins_and_fits() {
        let a = Axis::default();
        let s = scale_for(3.0, 97.0, 5, &a);
        assert_eq!((s.min, s.max, s.major), (0.0, 100.0, 20.0));
        let s = scale_for(-12.0, 7.0, 5, &a);
        assert!(s.min <= -12.0 && s.max >= 7.0 && s.ticks().contains(&0.0));
        let s = scale_for(1000.0, 1010.0, 5, &a);
        assert!(s.min > 900.0, "{s:?}");
        let fixed = Axis { min: Some(5.0), max: Some(6.0), major: Some(0.25), ..Default::default() };
        let s = scale_for(0.0, 100.0, 5, &fixed);
        assert_eq!(s.ticks(), [5.0, 5.25, 5.5, 5.75, 6.0]);
        let log = Axis { log_base: Some(10.0), ..Default::default() };
        let s = scale_for(3.0, 4000.0, 5, &log);
        assert_eq!((s.min, s.max), (1.0, 10000.0));
        assert_eq!(s.ticks().len(), 5);
        assert!((s.frac(100.0) - 0.5).abs() < 1e-9);
        assert_eq!(nice_step(0.7), 1.0);
        assert_eq!(nice_step(0.023), 0.025);
        let s = scale_for(f64::NAN, 1.0, 5, &a);
        assert!(s.max > s.min);

        let h = histogram(&[1.0, 2.0, 2.0, 3.0, 4.0, 10.0], Bins::Width(3.0));
        assert_eq!(h.iter().map(|b| b.2).collect::<Vec<_>>(), [5, 0, 1]);
        let h = histogram(&[1.0, 2.0, 3.0, 4.0], Bins::Count(2));
        assert_eq!(h.iter().map(|b| b.2).sum::<u64>(), 4);
        assert_eq!(h.len(), 2);
        assert!(!histogram(&[5.0, 5.0], Bins::Auto).is_empty());
        assert!(histogram(&[], Bins::Auto).is_empty());

        let xs: Vec<Option<f64>> = (1..=5).map(|i| Some(i as f64)).collect();
        let ys: Vec<Option<f64>> = (1..=5).map(|i| Some(2.0 * i as f64 + 1.0)).collect();
        let f = fit_trend(TrendKind::Linear, &xs, &ys).unwrap();
        assert!((f.coef[0] - 1.0).abs() < 1e-9 && (f.coef[1] - 2.0).abs() < 1e-9 && (f.r2 - 1.0).abs() < 1e-9);
        assert_eq!(f.equation(&Locale::EN), "y = 2x + 1");
        let ys2: Vec<Option<f64>> = (1..=5).map(|i| Some(3.0 * (0.5 * i as f64).exp())).collect();
        let f = fit_trend(TrendKind::Exponential, &xs, &ys2).unwrap();
        assert!((f.coef[0] - 3.0).abs() < 1e-6 && (f.coef[1] - 0.5).abs() < 1e-9);
        let q: Vec<Option<f64>> = (1..=5).map(|i| Some((i * i) as f64)).collect();
        let f = fit_trend(TrendKind::Polynomial(2), &xs, &q).unwrap();
        assert!((f.eval(6.0).unwrap() - 36.0).abs() < 1e-6);
        assert!(fit_trend(TrendKind::Linear, &xs[..1], &ys[..1]).is_none());
        assert_eq!(moving_average(&[Some(1.0), Some(3.0), None, Some(5.0)], 2), [None, Some(2.0), Some(3.0), Some(5.0)]);
    }

    #[test]
    fn references() {
        assert_eq!(quote_sheet("Sheet1"), "Sheet1");
        assert_eq!(quote_sheet("My Sheet"), "'My Sheet'");
        assert_eq!(quote_sheet("A1"), "'A1'");
        assert_eq!(quote_sheet("O'Neil"), "'O''Neil'");
        let r = DataRef::range("My Sheet", Range::parse("B2:B5").unwrap());
        assert_eq!(r.to_canonical().unwrap(), "'My Sheet'!$B$2:$B$5");
        assert_eq!(r.area().unwrap().1, Range::parse("B2:B5").unwrap());
        assert!(DataRef::parse("").is_empty());
        assert!(DataRef::parse("=((").formula.is_none());
    }
}
