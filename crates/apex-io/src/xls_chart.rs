//! Charts embedded in .xls sheets: the chart substream (`BOF` of type
//! chart) that follows the chart's `OBJ` record. Read: the type of each
//! chart group (column, bar, line, area, pie and doughnut, scatter and
//! bubble, radar), stacking, the series with their references and colors,
//! the titles, the legend, gridlines and axis bounds.

use apex_sheet::chart::{Axis, Blanks, DataRef, Grouping, LegendPos, Marker, Series};
use apex_sheet::{CellRef, Chart, ChartType};

use crate::biff::Cur;
use crate::xls::Rec;
use crate::xls_data::Fmla;

/// Series of one chart at most (hostile files).
const MAX_SERIES: usize = 255;

/// A chart group (`CHARTFORMAT` and its type record).
struct Group {
    kind: ChartType,
    grouping: Grouping,
    /// On the secondary axes.
    secondary: bool,
}

fn rgb(d: &[u8]) -> u32 {
    u32::from(d[0]) << 16 | u32::from(d[1]) << 8 | u32::from(d[2])
}

/// `SERIESTEXT`: an id, then a short string.
fn series_text(d: &[u8]) -> Option<String> {
    let mut c = Cur::new(d.get(2..)?);
    let n = c.u8()? as usize;
    let wide = c.u8()? & 1 != 0;
    c.chars8(n, wide)
}

/// The chart of a chart substream (the records after its `BOF`), placed
/// at A1.
pub(crate) fn read(recs: &[Rec], fmla: Fmla) -> Option<Chart> {
    let mut chart = Chart::new(ChartType::Column, CellRef::new(0, 0), 480.0, 288.0);
    chart.legend = LegendPos::None;
    chart.show_title = false;
    let mut groups: Vec<Group> = Vec::new();
    // Series with their chart group.
    let mut series: Vec<(Series, usize)> = Vec::new();
    // What each BEGIN opened, and the last record at this level.
    let mut stack: Vec<u16> = Vec::new();
    let mut last = 0u16;
    let mut secondary = false;
    // The axis being read: (secondary, type).
    let mut axis: Option<(bool, u16)> = None;
    // The data format being read: point (None for the whole series).
    let mut point: Option<Option<u32>> = None;
    let mut text: (Option<String>, Option<u16>) = (None, None);
    let mut x_axis = Axis::default();
    let mut y_axis = Axis::default();
    let mut y2_axis = Axis::default();
    let mut deep = 0u32;
    for r in recs {
        let d = &r.data;
        let mut c = Cur::new(d);
        let inside = |ty: u16| stack.contains(&ty);
        let in_series = inside(0x1003);
        match r.ty {
            // Levels past 64 are counted, not kept.
            0x1033 if stack.len() >= 64 => {
                deep += 1;
                continue;
            }
            0x1033 => {
                stack.push(last);
                continue;
            }
            0x1034 if deep > 0 => {
                deep -= 1;
                last = 0;
                continue;
            }
            0x1034 => {
                match stack.pop() {
                    Some(0x1025) => {
                        let (t, link) = std::mem::take(&mut text);
                        let t = t.unwrap_or_default();
                        match link {
                            Some(1) => {
                                chart.title = t;
                                chart.show_title = true;
                            }
                            Some(2) => y_axis.title = t,
                            Some(3) => x_axis.title = t,
                            _ => {}
                        }
                    }
                    Some(0x101D) => axis = None,
                    Some(0x1006) => point = None,
                    Some(0x1041) => secondary = false,
                    _ => {}
                }
                last = 0;
                continue;
            }
            0x1003 => {
                if series.len() < MAX_SERIES {
                    series.push((Series::default(), 0));
                }
            }
            0x1051 if in_series && !inside(0x1025) => {
                let r = (|| {
                    let id = c.u8()?;
                    let rt = c.u8()?;
                    c.skip(4)?;
                    let n = c.u16()? as usize;
                    Some((id, rt, c.bytes(n)?))
                })();
                if let (Some((id, 2, rgce)), Some((s, _))) = (r, series.last_mut()) {
                    // A lost reference (#REF!, #N/A) leaves the literal text.
                    let dr = fmla(rgce, CellRef::new(0, 0)).filter(|t| !t.starts_with('#')).map(|t| DataRef::parse(&t)).unwrap_or_default();
                    match id {
                        0 => s.name = dr,
                        1 => s.values = dr,
                        2 => s.categories = dr,
                        3 => s.sizes = dr,
                        _ => {}
                    }
                }
            }
            0x100D => {
                let t = series_text(d);
                if inside(0x1025) {
                    text.0 = t;
                } else if let (true, Some(t), Some((s, _))) = (in_series, t, series.last_mut()) {
                    if s.name.is_empty() {
                        s.name = DataRef::text(&t);
                    }
                }
            }
            0x1027 if inside(0x1025) => text.1 = c.u16(),
            0x1045 => {
                if let (Some(g), Some((_, grp))) = (c.u16(), series.last_mut()) {
                    *grp = g as usize;
                }
            }
            0x1006 if in_series => {
                point = c.u16().map(|xi| (xi != 0xFFFF).then_some(u32::from(xi)));
            }
            0x100A if in_series && point.is_some() && d.len() >= 12 => {
                let flags = u16::from_le_bytes([d[10], d[11]]);
                let (color, none) = ((flags & 1 == 0).then(|| rgb(d)), u16::from_le_bytes([d[8], d[9]]) == 0);
                if let (Some((s, _)), Some(p)) = (series.last_mut(), point) {
                    match p {
                        None => {
                            if none && flags & 1 == 0 {
                                s.no_fill = true;
                            } else if let Some(c) = color {
                                s.color = Some(c);
                            }
                        }
                        Some(i) => {
                            if let (Some(c), false) = (color, none) {
                                s.point_colors.push((i, c));
                            }
                        }
                    }
                }
            }
            0x1007 if in_series && point == Some(None) && d.len() >= 10 => {
                let (lns, flags) = (u16::from_le_bytes([d[4], d[5]]), u16::from_le_bytes([d[8], d[9]]));
                if let Some((s, _)) = series.last_mut() {
                    if lns == 5 {
                        s.no_line = true;
                    } else if flags & 1 == 0 {
                        s.line_color = Some(rgb(d));
                    }
                }
            }
            0x1009 if in_series && point == Some(None) && d.len() >= 12 => {
                let (imk, flags) = (u16::from_le_bytes([d[8], d[9]]), u16::from_le_bytes([d[10], d[11]]));
                if let (Some((s, _)), false) = (series.last_mut(), flags & 1 != 0) {
                    s.marker = match imk {
                        0 => Marker::None,
                        1 => Marker::Square,
                        2 => Marker::Diamond,
                        3 => Marker::Triangle,
                        _ => Marker::Circle,
                    };
                }
            }
            0x100B if in_series && point == Some(None) => {
                if let (Some(v), Some((s, _))) = (c.u16(), series.last_mut()) {
                    s.explode = v.min(255) as u8;
                }
            }
            0x105D if in_series => {
                if let (Some(f), Some((s, _))) = (c.u16(), series.last_mut()) {
                    s.smooth = f & 1 != 0;
                }
            }
            0x1044 => {
                if let (Some(flags), Some(blanks)) = (c.u16(), c.u8()) {
                    chart.visible_only = flags & 2 != 0;
                    chart.blanks = match blanks {
                        1 => Blanks::Zero,
                        2 => Blanks::Span,
                        _ => Blanks::Gap,
                    };
                }
            }
            0x1041 => secondary = c.u16() == Some(1),
            0x101D => {
                axis = c.u16().map(|t| (secondary, t));
                if let Some(a) = axis_of(axis, &mut x_axis, &mut y_axis, &mut y2_axis) {
                    a.major_grid = false;
                }
            }
            0x101F if d.len() >= 42 => {
                if let Some(a) = axis_of(axis, &mut x_axis, &mut y_axis, &mut y2_axis) {
                    let v = |i: usize| f64::from_le_bytes(d[i * 8..i * 8 + 8].try_into().unwrap_or_default());
                    let flags = u16::from_le_bytes([d[40], d[41]]);
                    a.min = (flags & 1 == 0).then(|| v(0));
                    a.max = (flags & 2 == 0).then(|| v(1));
                    a.major = (flags & 4 == 0).then(|| v(2));
                    a.minor = (flags & 8 == 0).then(|| v(3));
                    a.log_base = (flags & 0x20 != 0).then_some(10.0);
                    a.reverse = flags & 0x40 != 0;
                }
            }
            0x1021 => {
                if let (Some(a), Some(id)) = (axis_of(axis, &mut x_axis, &mut y_axis, &mut y2_axis), c.u16()) {
                    match id {
                        1 => a.major_grid = true,
                        2 => a.minor_grid = true,
                        _ => {}
                    }
                }
            }
            0x1014 => groups.push(Group { kind: ChartType::Column, grouping: Grouping::Clustered, secondary }),
            0x1017 | 0x1018 | 0x1019 | 0x101A | 0x101B | 0x103E | 0x1040 | 0x103F | 0x1061 if inside(0x1014) => {
                let first = groups.len() == 1;
                let Some(g) = groups.last_mut() else { continue };
                let flags = |i: usize| d.get(i..i + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
                let stacked = |f: u16, s: u16, p: u16| if f & p != 0 { Grouping::Percent } else if f & s != 0 { Grouping::Stacked } else { Grouping::Clustered };
                match r.ty {
                    0x1017 => {
                        let f = flags(4);
                        g.kind = if f & 1 != 0 { ChartType::Bar } else { ChartType::Column };
                        g.grouping = stacked(f, 2, 4);
                        if first {
                            chart.overlap = (-(flags(0) as i16)).clamp(-100, 100) as i8;
                            chart.gap = flags(2).min(500);
                        }
                    }
                    0x1018 | 0x101A => {
                        g.kind = if r.ty == 0x1018 { ChartType::Line } else { ChartType::Area };
                        g.grouping = stacked(flags(0), 1, 2);
                    }
                    0x1019 | 0x1061 => {
                        let hole = flags(2);
                        g.kind = if r.ty == 0x1019 && hole > 0 { ChartType::Doughnut } else { ChartType::Pie };
                        if first {
                            chart.first_angle = flags(0) % 360;
                            if hole > 0 {
                                chart.hole = hole.clamp(10, 90) as u8;
                            }
                        }
                    }
                    0x101B => g.kind = if flags(4) & 1 != 0 { ChartType::Bubble } else { ChartType::Scatter },
                    0x103F => g.kind = ChartType::Surface,
                    _ => {
                        g.kind = ChartType::Radar;
                        if first {
                            chart.filled = r.ty == 0x1040;
                        }
                    }
                }
            }
            0x103A if inside(0x1014) => chart.view3d = true,
            0x1015 if inside(0x1014) && groups.len() == 1 => {
                // LEGEND: where it docks.
                chart.legend = match d.get(16) {
                    Some(0) => LegendPos::Bottom,
                    Some(2) => LegendPos::Top,
                    Some(4) => LegendPos::Left,
                    _ => LegendPos::Right,
                };
            }
            _ => {}
        }
        last = r.ty;
    }
    let main = groups.first()?;
    chart.set_kind(main.kind);
    chart.grouping = main.grouping;
    chart.vary_colors = chart.kind.is_round();
    for (mut s, g) in series {
        let grp = groups.get(g).unwrap_or(main);
        if g > 0 && (grp.kind != main.kind || grp.secondary) {
            s.kind = Some(grp.kind);
        }
        s.secondary = grp.secondary;
        if matches!(grp.kind, ChartType::Line | ChartType::Scatter | ChartType::Radar) {
            // Lines take the line's color.
            s.color = s.line_color.take().or(s.color);
        }
        chart.series.push(s);
    }
    if matches!(chart.kind, ChartType::Line | ChartType::Scatter | ChartType::Radar) && !chart.series.is_empty() {
        chart.markers = !chart.series.iter().all(|s| s.marker == Marker::None);
        if chart.kind == ChartType::Scatter {
            chart.lines = !chart.series.iter().all(|s| s.no_line);
        }
        for s in &mut chart.series {
            if !chart.markers {
                s.marker = Marker::Auto;
            }
            if chart.kind == ChartType::Scatter && !chart.lines {
                s.no_line = false;
            }
        }
        if chart.series.iter().all(|s| s.smooth) {
            chart.smooth = true;
            for s in &mut chart.series {
                s.smooth = false;
            }
        }
    }
    if chart.kind.has_axes() {
        (chart.x_axis, chart.y_axis, chart.y2_axis) = (x_axis, y_axis, y2_axis);
    }
    crate::xlsx_chart::detect_palette(&mut chart);
    chart.sanitize();
    Some(chart)
}

/// The axis being read: category (or X) and value axes of the main group,
/// and the secondary value axis.
fn axis_of<'a>(at: Option<(bool, u16)>, x: &'a mut Axis, y: &'a mut Axis, y2: &'a mut Axis) -> Option<&'a mut Axis> {
    match at? {
        (false, 0) => Some(x),
        (false, 1) => Some(y),
        (true, 1) => Some(y2),
        _ => None,
    }
}
