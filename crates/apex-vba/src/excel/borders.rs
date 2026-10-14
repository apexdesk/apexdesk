//! Range.Borders, Range.Borders(xlEdgeTop) and Range.BorderAround. A
//! range's edge is the outer side of its cells; the inside lines are the
//! sides between them.

use apex_sheet::sheet::{Line, LineStyle, border};
use apex_sheet::{CellRef, CellStyle};

use super::{Args, Interp, PALETTE, obj, swap_rb};
use crate::error::{Error, R};
use crate::object::{Obj, RangeRef};
use crate::value::Value;

/// XlBordersIndex values.
const LEFT: i32 = 7;
const TOP: i32 = 8;
const BOTTOM: i32 = 9;
const RIGHT: i32 = 10;
const INSIDE_V: i32 = 11;
const INSIDE_H: i32 = 12;
/// No index: every side of every cell.
pub(crate) const ALL: i32 = 0;

/// The sides of cell `c` of `r` that border `index` of the range is.
fn sides(r: &RangeRef, index: i32, c: CellRef) -> u8 {
    let rg = r.range;
    let mut m = 0;
    let on = |i: i32| index == i || index == ALL;
    if on(LEFT) && c.col == rg.start.col || on(INSIDE_V) && c.col > rg.start.col {
        m |= border::LEFT;
    }
    if on(RIGHT) && c.col == rg.end.col || on(INSIDE_V) && c.col < rg.end.col {
        m |= border::RIGHT;
    }
    if on(TOP) && c.row == rg.start.row || on(INSIDE_H) && c.row > rg.start.row {
        m |= border::TOP;
    }
    if on(BOTTOM) && c.row == rg.end.row || on(INSIDE_H) && c.row < rg.end.row {
        m |= border::BOTTOM;
    }
    m
}

fn side(s: &mut CellStyle, flag: u8) -> &mut Line {
    match flag {
        border::TOP => &mut s.borders.top,
        border::BOTTOM => &mut s.borders.bottom,
        border::LEFT => &mut s.borders.left,
        _ => &mut s.borders.right,
    }
}

/// A change to one line.
#[derive(Clone, Copy)]
enum Set {
    Style(i64),
    Weight(i64),
    Color(Option<u32>),
}

fn change(l: &mut Line, set: Set) {
    let solid = matches!(l.style, LineStyle::Thin | LineStyle::Medium | LineStyle::Thick | LineStyle::Hair);
    match set {
        Set::Style(v) => {
            l.style = match v {
                -4142 => LineStyle::None,
                1 => if solid { l.style } else { LineStyle::Thin },
                -4115 | 4 | 5 | 13 => LineStyle::Dashed,
                -4118 => LineStyle::Dotted,
                -4119 => LineStyle::Double,
                _ => LineStyle::Thin,
            };
            if l.style == LineStyle::None {
                l.color = None;
            }
        }
        Set::Weight(v) => {
            if l.style == LineStyle::None || solid {
                l.style = match v {
                    1 => LineStyle::Hair,
                    -4138 => LineStyle::Medium,
                    4 => LineStyle::Thick,
                    _ => LineStyle::Thin,
                };
            }
        }
        Set::Color(c) => {
            if l.style == LineStyle::None {
                l.style = LineStyle::Thin;
            }
            l.color = c;
        }
    }
}

impl Interp<'_> {
    /// Range.Borders, with or without an index.
    pub(super) fn borders_call(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        Ok(match self.arg_i64(a, 0, "index")? {
            Some(i) => obj(Obj::Border(r, check_index(i)?)),
            None => obj(Obj::Border(r, ALL)),
        })
    }

    /// Range.BorderAround LineStyle, Weight, ColorIndex, Color.
    pub(super) fn border_around(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let style = self.arg_i64(a, 0, "linestyle")?.unwrap_or(1);
        let weight = self.arg_i64(a, 1, "weight")?.unwrap_or(2);
        let color = match (self.arg_i64(a, 3, "color")?, self.arg_i64(a, 2, "colorindex")?) {
            (Some(c), _) => Some(Some(swap_rb(c as u32 & 0xFF_FFFF))),
            (None, Some(i @ 1..=56)) => Some(Some(PALETTE[i as usize - 1])),
            _ => None,
        };
        for i in [LEFT, TOP, BOTTOM, RIGHT] {
            self.border_apply(r, i, Set::Style(style))?;
            if style != -4142 {
                self.border_apply(r, i, Set::Weight(weight))?;
                if let Some(c) = color {
                    self.border_apply(r, i, Set::Color(c))?;
                }
            }
        }
        Ok(Value::Empty)
    }

    fn border_apply(&mut self, r: RangeRef, index: i32, set: Set) -> R<()> {
        self.restyle_by(
            r,
            |c| sides(&r, index, c),
            |m, s| {
                for f in [border::TOP, border::BOTTOM, border::LEFT, border::RIGHT] {
                    if m & f != 0 {
                        change(side(s, f), set);
                    }
                }
            },
        )
    }

    pub(super) fn border_get(&mut self, r: RangeRef, index: i32, name: &str, a: &Args) -> R<Value> {
        if matches!(name, "item" | "_default") && index == ALL {
            let i = self.arg_i64(a, 0, "index")?.ok_or_else(Error::invalid_call)?;
            return Ok(obj(Obj::Border(r, check_index(i)?)));
        }
        let c = r.range.start;
        let m = sides(&r, if index == ALL { TOP } else { index }, c);
        let s = self.wb.cell(r.sheet, c).map_or(0, |x| x.style);
        let mut st = self.wb.styles.get(s).clone();
        let l = *side(&mut st, if m == 0 { border::TOP } else { m & m.wrapping_neg() });
        Ok(match name {
            "linestyle" => Value::Long(match l.style {
                LineStyle::None => -4142,
                LineStyle::Dashed => -4115,
                LineStyle::Dotted => -4118,
                LineStyle::Double => -4119,
                _ => 1,
            }),
            "weight" => Value::Long(match l.style {
                LineStyle::Hair => 1,
                LineStyle::Medium => -4138,
                LineStyle::Thick | LineStyle::Double => 4,
                _ => 2,
            }),
            "color" => Value::Long(swap_rb(l.color.unwrap_or(0)) as i32),
            "colorindex" => Value::Long(match l.color {
                None => -4105,
                Some(c) => PALETTE.iter().position(|&p| p == c).map_or(-4105, |i| i as i32 + 1),
            }),
            "count" if index == ALL => Value::Long(6),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn border_set(&mut self, r: RangeRef, index: i32, name: &str, v: Value) -> R<()> {
        let set = match name {
            "linestyle" => Set::Style(v.to_i64()?),
            "weight" => Set::Weight(v.to_i64()?),
            "color" => Set::Color(Some(swap_rb(v.to_i64()? as u32 & 0xFF_FFFF))),
            "colorindex" => Set::Color(match v.to_i64()? {
                -4105 | -4142 | 0 => None,
                i @ 1..=56 => Some(PALETTE[i as usize - 1]),
                _ => return Err(Error::app("Unable to set the ColorIndex property of the Border class")),
            }),
            "tintandshade" | "themecolor" => return Ok(()),
            _ => return Err(Error::no_member(name)),
        };
        self.border_apply(r, index, set)
    }
}

/// The diagonals aren't drawn: they take nothing.
fn check_index(i: i64) -> R<i32> {
    match i {
        5 | 6 | 7..=12 => Ok(i as i32),
        _ => Err(Error::subscript()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::Range;

    #[test]
    fn edges_and_insides() {
        let r = RangeRef::new(0, Range::parse("B2:C3").unwrap());
        let c = |s: &str| CellRef::parse(s).unwrap();
        assert_eq!(sides(&r, TOP, c("B2")), border::TOP);
        assert_eq!(sides(&r, TOP, c("B3")), 0);
        assert_eq!(sides(&r, INSIDE_H, c("B2")), border::BOTTOM);
        assert_eq!(sides(&r, INSIDE_H, c("C3")), border::TOP);
        assert_eq!(sides(&r, ALL, c("C3")), border::ALL);
    }
}
