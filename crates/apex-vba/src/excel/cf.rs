//! Conditional formatting: Range.FormatConditions, its rules
//! (FormatCondition, and the color scales, data bars and icon sets) and
//! their Font, Interior and thresholds. A rule is the sheet's rule at an
//! index (its priority), as Excel numbers them.

use apex_sheet::data::{CfKind, CfRule, CfStyle, Cfvo, Period, TextOp, icon_count, ranges_minus};
use apex_sheet::{CellRef, Op};

use super::parts::cmp_op;
use super::{Args, Interp, PALETTE, areas, obj, swap_rb};
use crate::error::{Error, R};
use crate::object::{Obj, Part, RangeRef};
use crate::value::Value;

fn object_error() -> Error {
    Error::app("Application-defined or object-defined error")
}

fn part(p: Part) -> Value {
    obj(Obj::Part(Box::new(p)))
}

/// XlIconSet, in order (those Excel has and the workbook doesn't are
/// empty).
const ICONS: [&str; 20] = [
    "3Arrows", "3ArrowsGray", "3Flags", "3TrafficLights1", "3TrafficLights2", "3Signs", "3Symbols", "3Symbols2", "4Arrows", "4ArrowsGray", "4RedToBlack", "4Rating", "4TrafficLights", "5Arrows", "", "5Rating", "", "3Stars", "3Triangles", "",
];

/// XlTimePeriods, in order.
const PERIODS: [Period; 10] = [Period::Today, Period::Yesterday, Period::Last7Days, Period::ThisWeek, Period::LastWeek, Period::LastMonth, Period::Tomorrow, Period::NextWeek, Period::NextMonth, Period::ThisMonth];

/// XlContainsOperator, in order.
const TEXT_OPS: [TextOp; 4] = [TextOp::Contains, TextOp::NotContains, TextOp::BeginsWith, TextOp::EndsWith];

/// XlFormatConditionType of a rule.
fn type_of(k: &CfKind) -> i32 {
    match k {
        CfKind::CellIs { .. } => 1,
        CfKind::Expression(_) => 2,
        CfKind::ColorScale(_) => 3,
        CfKind::DataBar { .. } => 4,
        CfKind::Top { .. } => 5,
        CfKind::IconSet { .. } => 6,
        CfKind::Duplicates(_) => 8,
        CfKind::Text { .. } => 9,
        CfKind::Blanks(true) => 10,
        CfKind::Period(_) => 11,
        CfKind::Average { .. } => 12,
        CfKind::Blanks(false) => 13,
        CfKind::Errors(true) => 16,
        CfKind::Errors(false) => 17,
    }
}

/// A threshold as XlConditionValueTypes and its value.
fn cfvo_out(c: &Cfvo) -> (i32, Value) {
    match c {
        Cfvo::Min => (1, Value::Empty),
        Cfvo::Max => (2, Value::Empty),
        Cfvo::Num(n) => (0, Value::Double(*n)),
        Cfvo::Percent(n) => (3, Value::Double(*n)),
        Cfvo::Percentile(n) => (5, Value::Double(*n)),
        Cfvo::Formula(f) => (4, Value::str(format!("={f}"))),
    }
}

fn color_in(v: &Value) -> R<u32> {
    Ok(swap_rb(v.to_i64()? as u32 & 0xFF_FFFF))
}

impl Interp<'_> {
    /// The formula of a rule as the sheet keeps it: relative references
    /// are written from the active cell (as in Excel) and kept from the
    /// rule's top-left cell.
    pub(super) fn rule_formula(&mut self, v: Value, origin: CellRef) -> R<String> {
        let v = self.scalar(v)?;
        let text = match &v {
            Value::Str(s) => s.trim().to_string(),
            Value::Date(d) => return Ok(d.to_string()),
            Value::Bool(b) => return Ok(if *b { "TRUE" } else { "FALSE" }.into()),
            v => return Ok(v.to_f64()?.to_string()),
        };
        let src = text.strip_prefix('=').unwrap_or(&text);
        if src.is_empty() {
            return Ok(String::new());
        }
        let mut e = apex_sheet::formula::parse_canonical(src).map_err(|e| Error::app(format!("The formula contains an error: {text} ({e})")))?;
        let at = self.selection.start;
        e.offset_wrap(origin.row as i64 - at.row as i64, origin.col as i64 - at.col as i64);
        Ok(e.to_canonical())
    }

    fn rule(&self, s: usize, i: usize) -> R<CfRule> {
        self.wb.sheets.get(s).and_then(|sh| sh.cond_formats.get(i)).cloned().ok_or_else(object_error)
    }

    /// The rules of the sheet that apply to cells of `r`, by priority.
    fn rules_in(&self, r: RangeRef) -> Vec<usize> {
        let Some(sh) = self.wb.sheets.get(r.sheet) else { return Vec::new() };
        sh.cond_formats.iter().enumerate().filter(|(_, x)| x.ranges.iter().any(|g| g.intersect(&r.range).is_some())).map(|(i, _)| i).collect()
    }

    fn set_rules(&mut self, s: usize, list: Vec<CfRule>) -> R<()> {
        self.apply(Op::CondFormats { sheet: s, list })
    }

    fn change_rule(&mut self, s: usize, i: usize, f: impl FnOnce(&mut CfRule) -> R<()>) -> R<()> {
        let mut list = self.wb.sheets.get(s).ok_or_else(Error::subscript)?.cond_formats.clone();
        f(list.get_mut(i).ok_or_else(object_error)?)?;
        self.set_rules(s, list)
    }

    /// Adds a rule after the others; the new FormatCondition.
    fn add_rule(&mut self, r: RangeRef, kind: CfKind) -> R<Value> {
        let mut list = self.wb.sheets.get(r.sheet).ok_or_else(Error::subscript)?.cond_formats.clone();
        list.push(CfRule { ranges: vec![r.range], kind, style: CfStyle::default(), stop: false });
        let i = list.len() - 1;
        self.set_rules(r.sheet, list)?;
        Ok(part(Part::Condition(r.sheet, i)))
    }

    pub(super) fn conditions_member(&mut self, r: RangeRef, name: &str, a: &Args) -> R<Value> {
        let origin = r.range.start;
        match name {
            "count" => Ok(Value::Long(self.rules_in(r).len() as i32)),
            "item" | "_default" => {
                let i = self.arg_i64(a, 0, "index")?.ok_or_else(Error::invalid_call)?;
                let list = self.rules_in(r);
                let k = usize::try_from(i - 1).ok().and_then(|k| list.get(k)).ok_or_else(Error::subscript)?;
                Ok(part(Part::Condition(r.sheet, *k)))
            }
            "add" => {
                let ty = self.arg_i64(a, 0, "type")?.ok_or_else(Error::invalid_call)?;
                let kind = match ty {
                    1 => {
                        let op = cmp_op(self.arg_i64(a, 1, "operator")?.unwrap_or(1))?;
                        let f1 = match a.get(2, "formula1") {
                            Some(v) => self.rule_formula(v, origin)?,
                            None => return Err(Error::invalid_call()),
                        };
                        let f2 = match a.get(3, "formula2") {
                            Some(v) => self.rule_formula(v, origin)?,
                            None if op.two_args() => return Err(Error::invalid_call()),
                            None => String::new(),
                        };
                        CfKind::CellIs { op, f1, f2 }
                    }
                    2 => match a.get(2, "formula1") {
                        Some(v) => CfKind::Expression(self.rule_formula(v, origin)?),
                        None => return Err(Error::invalid_call()),
                    },
                    9 => {
                        let text = self.arg_str(a, 4, "string")?.ok_or_else(Error::invalid_call)?;
                        let op = self.arg_i64(a, 5, "textoperator")?.unwrap_or(0);
                        let op = *usize::try_from(op).ok().and_then(|i| TEXT_OPS.get(i)).ok_or_else(Error::invalid_call)?;
                        CfKind::Text { op, text }
                    }
                    10 | 13 => CfKind::Blanks(ty == 10),
                    16 | 17 => CfKind::Errors(ty == 16),
                    11 => {
                        let p = self.arg_i64(a, 6, "dateoperator")?.unwrap_or(0);
                        CfKind::Period(*usize::try_from(p).ok().and_then(|i| PERIODS.get(i)).ok_or_else(Error::invalid_call)?)
                    }
                    5 => CfKind::Top { bottom: false, percent: false, rank: 10 },
                    8 => CfKind::Duplicates(false),
                    12 => CfKind::Average { below: false, equal: false },
                    _ => return Err(Error::invalid_call()),
                };
                self.add_rule(r, kind)
            }
            "addcolorscale" => {
                let n = self.arg_i64(a, 0, "colorscaletype")?.unwrap_or(3);
                let stops = match n {
                    2 => vec![(Cfvo::Min, 0xF8696B), (Cfvo::Max, 0x63BE7B)],
                    3 => vec![(Cfvo::Min, 0xF8696B), (Cfvo::Percentile(50.0), 0xFFEB84), (Cfvo::Max, 0x63BE7B)],
                    _ => return Err(Error::invalid_call()),
                };
                self.add_rule(r, CfKind::ColorScale(stops))
            }
            "adddatabar" => self.add_rule(r, CfKind::DataBar { color: 0x638EC6, min: Cfvo::Min, max: Cfvo::Max }),
            "addiconsetcondition" => self.add_rule(r, CfKind::IconSet { set: "3TrafficLights1".into(), reverse: false, show_value: true, points: vec![Cfvo::Percent(33.0), Cfvo::Percent(67.0)] }),
            "addtop10" => self.add_rule(r, CfKind::Top { bottom: false, percent: false, rank: 10 }),
            "addaboveaverage" => self.add_rule(r, CfKind::Average { below: false, equal: false }),
            "adduniquevalues" => self.add_rule(r, CfKind::Duplicates(false)),
            "delete" => {
                let list: Vec<CfRule> = self.wb.sheets.get(r.sheet).ok_or_else(Error::subscript)?.cond_formats.iter().cloned().filter_map(|mut x| {
                    x.ranges = ranges_minus(&x.ranges, &r.range);
                    (!x.ranges.is_empty()).then_some(x)
                }).collect();
                if list != self.wb.sheets[r.sheet].cond_formats {
                    self.set_rules(r.sheet, list)?;
                }
                Ok(Value::Empty)
            }
            "parent" => Ok(obj(Obj::Range(r))),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn condition_member(&mut self, s: usize, i: usize, name: &str, a: &Args) -> R<Value> {
        let rule = self.rule(s, i)?;
        let k = &rule.kind;
        let bad = || Error::no_member(name);
        let formula = |f: &str| if f.is_empty() { Value::str("") } else { Value::str(format!("={f}")) };
        Ok(match name {
            "type" => Value::Long(type_of(k)),
            "priority" => Value::Long(i as i32 + 1),
            "stopiftrue" => Value::Bool(rule.stop),
            "appliesto" => areas::areas(s, rule.ranges.clone())?,
            "font" if !k.is_graphic() => part(Part::ConditionFont(s, i)),
            "interior" if !k.is_graphic() => part(Part::ConditionInterior(s, i)),
            "delete" => {
                let mut list = self.wb.sheets[s].cond_formats.clone();
                list.remove(i);
                self.set_rules(s, list)?;
                Value::Empty
            }
            "setfirstpriority" | "setlastpriority" => {
                let mut list = self.wb.sheets[s].cond_formats.clone();
                let x = list.remove(i);
                if name == "setfirstpriority" { list.insert(0, x) } else { list.push(x) }
                self.set_rules(s, list)?;
                Value::Empty
            }
            "modifyappliestorange" => {
                let rs = match a.get(0, "range") {
                    Some(v) => areas::ranges_of(&v).ok_or_else(Error::type_mismatch)?,
                    None => return Err(Error::invalid_call()),
                };
                if rs.iter().any(|x| x.sheet != s) {
                    return Err(object_error());
                }
                self.change_rule(s, i, |x| {
                    x.ranges = rs.iter().map(|x| x.range).collect();
                    Ok(())
                })?;
                Value::Empty
            }
            "modify" => {
                // The type, operator and formulas, as Add takes them.
                let origin = rule.origin();
                let ty = self.arg_i64(a, 0, "type")?.ok_or_else(Error::invalid_call)?;
                let kind = match ty {
                    1 => {
                        let op = cmp_op(self.arg_i64(a, 1, "operator")?.unwrap_or(1))?;
                        let f1 = match a.get(2, "formula1") {
                            Some(v) => self.rule_formula(v, origin)?,
                            None => return Err(Error::invalid_call()),
                        };
                        let f2 = match a.get(3, "formula2") {
                            Some(v) => self.rule_formula(v, origin)?,
                            None => String::new(),
                        };
                        CfKind::CellIs { op, f1, f2 }
                    }
                    2 => CfKind::Expression(match a.get(2, "formula1") {
                        Some(v) => self.rule_formula(v, origin)?,
                        None => return Err(Error::invalid_call()),
                    }),
                    _ => return Err(Error::invalid_call()),
                };
                self.change_rule(s, i, |x| {
                    x.kind = kind;
                    Ok(())
                })?;
                Value::Empty
            }
            "parent" => areas::areas(s, rule.ranges.clone())?,
            _ => match k {
                CfKind::CellIs { op, f1, f2 } => match name {
                    "operator" => Value::Long(super::parts::CMP_OPS.iter().position(|o| o == op).map_or(1, |i| i as i32 + 1)),
                    "formula1" => formula(f1),
                    "formula2" => formula(f2),
                    _ => return Err(bad()),
                },
                CfKind::Expression(f) if name == "formula1" => formula(f),
                CfKind::Text { op, text } => match name {
                    "text" => Value::str(text.clone()),
                    "textoperator" => Value::Long(TEXT_OPS.iter().position(|o| o == op).unwrap_or(0) as i32),
                    _ => return Err(bad()),
                },
                CfKind::Period(p) if name == "dateoperator" => Value::Long(PERIODS.iter().position(|x| x == p).unwrap_or(0) as i32),
                CfKind::Top { bottom, percent, rank } => match name {
                    "topbottom" => Value::Long(if *bottom { 0 } else { 1 }),
                    "rank" => Value::Long(*rank as i32),
                    "percent" => Value::Bool(*percent),
                    _ => return Err(bad()),
                },
                CfKind::Average { below, equal } if name == "abovebelow" => Value::Long(match (below, equal) {
                    (false, false) => 0,
                    (true, false) => 1,
                    (false, true) => 2,
                    (true, true) => 3,
                }),
                CfKind::Duplicates(d) if name == "dupeunique" => Value::Long(*d as i32),
                CfKind::ColorScale(stops) => match name {
                    "colorscalecriteria" => match a.get(0, "index") {
                        None => part(Part::Criteria(s, i)),
                        Some(v) => {
                            let n = self.scalar(v)?.to_i64()?;
                            if n < 1 || n as usize > stops.len() {
                                return Err(Error::subscript());
                            }
                            part(Part::Criterion(s, i, n as usize))
                        }
                    },
                    _ => return Err(bad()),
                },
                CfKind::DataBar { .. } => match name {
                    "minpoint" => part(Part::Criterion(s, i, 1)),
                    "maxpoint" => part(Part::Criterion(s, i, 2)),
                    "barcolor" => part(Part::CfColor(s, i, 0)),
                    _ => return Err(bad()),
                },
                CfKind::IconSet { set, reverse, show_value, points } => match name {
                    "iconcriteria" => match a.get(0, "index") {
                        None => part(Part::Criteria(s, i)),
                        Some(v) => {
                            let n = self.scalar(v)?.to_i64()?;
                            if n < 1 || n as usize > points.len() + 1 {
                                return Err(Error::subscript());
                            }
                            part(Part::Criterion(s, i, n as usize))
                        }
                    },
                    "iconset" => part(Part::IconSet(ICONS.iter().position(|x| x == set).map_or(4, |i| i + 1))),
                    "reverseorder" => Value::Bool(*reverse),
                    "showicononly" => Value::Bool(!show_value),
                    _ => return Err(bad()),
                },
                _ => return Err(bad()),
            },
        })
    }

    pub(super) fn condition_set(&mut self, s: usize, i: usize, name: &str, v: Value) -> R<()> {
        let rule = self.rule(s, i)?;
        let origin = rule.origin();
        // Formulas first: they need the interpreter.
        let f = match name {
            "formula1" | "formula2" => Some(self.rule_formula(v.clone(), origin)?),
            _ => None,
        };
        self.change_rule(s, i, |x| {
            let bad = || Error::no_member(name);
            match (name, &mut x.kind) {
                ("stopiftrue", _) => x.stop = v.to_bool()?,
                ("formula1", CfKind::CellIs { f1, .. }) | ("formula1", CfKind::Expression(f1)) => *f1 = f.unwrap_or_default(),
                ("formula2", CfKind::CellIs { f2, .. }) => *f2 = f.unwrap_or_default(),
                ("operator", CfKind::CellIs { op, .. }) => *op = cmp_op(v.to_i64()?)?,
                ("text", CfKind::Text { text, .. }) => *text = v.to_str()?,
                ("textoperator", CfKind::Text { op, .. }) => *op = *usize::try_from(v.to_i64()?).ok().and_then(|i| TEXT_OPS.get(i)).ok_or_else(Error::invalid_call)?,
                ("dateoperator", CfKind::Period(p)) => *p = *usize::try_from(v.to_i64()?).ok().and_then(|i| PERIODS.get(i)).ok_or_else(Error::invalid_call)?,
                ("topbottom", CfKind::Top { bottom, .. }) => *bottom = v.to_i64()? == 0,
                ("rank", CfKind::Top { rank, .. }) => {
                    *rank = u32::try_from(v.to_i64()?).ok().filter(|r| (1..=1000).contains(r)).ok_or_else(object_error)?;
                }
                ("percent", CfKind::Top { percent, .. }) => *percent = v.to_bool()?,
                ("abovebelow", CfKind::Average { below, equal }) => {
                    (*below, *equal) = match v.to_i64()? {
                        0 => (false, false),
                        1 => (true, false),
                        2 => (false, true),
                        3 => (true, true),
                        _ => return Err(Error::invalid_call()),
                    };
                }
                ("dupeunique", CfKind::Duplicates(d)) => *d = v.to_i64()? == 1,
                ("iconset", CfKind::IconSet { set, points, .. }) => {
                    let n = match &v {
                        Value::Object(Obj::Part(p)) => match **p {
                            Part::IconSet(n) => n,
                            _ => return Err(Error::type_mismatch()),
                        },
                        v => usize::try_from(v.to_i64()?).map_err(|_| Error::invalid_call())?,
                    };
                    let name = n.checked_sub(1).and_then(|k| ICONS.get(k)).filter(|x| !x.is_empty()).ok_or_else(|| Error::unsupported("this icon set"))?;
                    *set = name.to_string();
                    // The thresholds Excel gives the icons: even percents.
                    let count = icon_count(name);
                    *points = (1..count).map(|j| Cfvo::Percent(((j * 100) as f64 / count as f64).round())).collect();
                }
                ("reverseorder", CfKind::IconSet { reverse, .. }) => *reverse = v.to_bool()?,
                ("showicononly", CfKind::IconSet { show_value, .. }) => *show_value = !v.to_bool()?,
                _ => return Err(bad()),
            }
            Ok(())
        })
    }

    pub(super) fn condition_font(&mut self, s: usize, i: usize, name: &str, set: Option<Value>) -> R<Value> {
        let st = self.rule(s, i)?.style;
        let Some(v) = set else {
            return Ok(match name {
                "bold" => st.bold.map_or(Value::Null, Value::Bool),
                "italic" => st.italic.map_or(Value::Null, Value::Bool),
                "strikethrough" => st.strike.map_or(Value::Null, Value::Bool),
                "underline" => st.underline.map_or(Value::Null, |u| Value::Long(if u { 2 } else { -4142 })),
                "color" => st.color.map_or(Value::Null, |c| Value::Long(swap_rb(c) as i32)),
                "colorindex" => st.color.map_or(Value::Long(-4105), |c| Value::Long(PALETTE.iter().position(|&p| p == c).map_or(-4105, |i| i as i32 + 1))),
                _ => return Err(Error::no_member(name)),
            });
        };
        if matches!(name, "tintandshade" | "themecolor") {
            return Ok(Value::Empty);
        }
        self.change_rule(s, i, |x| {
            let st = &mut x.style;
            match name {
                "bold" => st.bold = Some(v.to_bool()?),
                "italic" => st.italic = Some(v.to_bool()?),
                "strikethrough" => st.strike = Some(v.to_bool()?),
                "underline" => {
                    st.underline = Some(match v {
                        Value::Bool(b) => b,
                        v => !matches!(v.to_i64()?, -4142 | 0),
                    })
                }
                "color" => st.color = Some(color_in(&v)?),
                "colorindex" => {
                    st.color = match v.to_i64()? {
                        -4105 | -4142 | 0 => None,
                        i @ 1..=56 => Some(PALETTE[i as usize - 1]),
                        _ => return Err(Error::app("Unable to set the ColorIndex property of the Font class")),
                    }
                }
                _ => return Err(Error::no_member(name)),
            }
            Ok(())
        })?;
        Ok(Value::Empty)
    }

    pub(super) fn condition_interior(&mut self, s: usize, i: usize, name: &str, set: Option<Value>) -> R<Value> {
        let st = self.rule(s, i)?.style;
        let Some(v) = set else {
            return Ok(match name {
                "color" => st.fill.map_or(Value::Null, |c| Value::Long(swap_rb(c) as i32)),
                "colorindex" => st.fill.map_or(Value::Long(-4142), |c| Value::Long(PALETTE.iter().position(|&p| p == c).map_or(-4142, |i| i as i32 + 1))),
                "pattern" => Value::Long(if st.fill.is_some() { 1 } else { -4142 }),
                _ => return Err(Error::no_member(name)),
            });
        };
        let fill = match name {
            "color" => Some(color_in(&v)?),
            "colorindex" => match v.to_i64()? {
                -4142 | -4105 | 0 => None,
                i @ 1..=56 => Some(PALETTE[i as usize - 1]),
                _ => return Err(Error::app("Unable to set the ColorIndex property of the Interior class")),
            },
            "pattern" if v.to_i64()? == -4142 => None,
            "pattern" | "patterncolor" | "patterncolorindex" | "tintandshade" | "themecolor" => return Ok(Value::Empty),
            _ => return Err(Error::no_member(name)),
        };
        self.change_rule(s, i, |x| {
            x.style.fill = fill;
            Ok(())
        })?;
        Ok(Value::Empty)
    }

    /// The thresholds of a rule: ColorScaleCriteria, IconCriteria.
    pub(super) fn criteria_member(&mut self, s: usize, i: usize, name: &str, a: &Args) -> R<Value> {
        let n = match &self.rule(s, i)?.kind {
            CfKind::ColorScale(stops) => stops.len(),
            CfKind::IconSet { points, .. } => points.len() + 1,
            _ => 2,
        };
        match name {
            "count" => Ok(Value::Long(n as i32)),
            "item" | "_default" => {
                let k = self.arg_i64(a, 0, "index")?.ok_or_else(Error::invalid_call)?;
                if k < 1 || k as usize > n {
                    return Err(Error::subscript());
                }
                Ok(part(Part::Criterion(s, i, k as usize)))
            }
            _ => Err(Error::no_member(name)),
        }
    }

    /// The threshold `k` (1-based) of a rule: a stop of a color scale, the
    /// MinPoint (1) or MaxPoint (2) of a data bar, an icon's lower bound.
    fn cfvo(kind: &CfKind, k: usize) -> Option<Cfvo> {
        match kind {
            CfKind::ColorScale(stops) => stops.get(k.checked_sub(1)?).map(|x| x.0.clone()),
            CfKind::DataBar { min, max, .. } => match k {
                1 => Some(min.clone()),
                2 => Some(max.clone()),
                _ => None,
            },
            // The first icon starts at the lowest value.
            CfKind::IconSet { points, .. } => if k == 1 { Some(Cfvo::Percent(0.0)) } else { points.get(k.checked_sub(2)?).cloned() },
            _ => None,
        }
    }

    pub(super) fn criterion_member(&mut self, s: usize, i: usize, k: usize, name: &str, a: &Args) -> R<Value> {
        let rule = self.rule(s, i)?;
        let c = Self::cfvo(&rule.kind, k).ok_or_else(object_error)?;
        let (ty, val) = cfvo_out(&c);
        match name {
            "type" => Ok(Value::Long(ty)),
            "value" => Ok(val),
            "formatcolor" if matches!(rule.kind, CfKind::ColorScale(_)) => Ok(part(Part::CfColor(s, i, k))),
            "index" => Ok(Value::Long(k as i32)),
            // DataBar MinPoint.Modify newtype, newvalue.
            "modify" => {
                let ty = self.arg_i64(a, 0, "newtype")?.ok_or_else(Error::invalid_call)?;
                let v = a.get(1, "newvalue");
                self.set_criterion(s, i, k, Some(ty), v)?;
                Ok(Value::Empty)
            }
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn criterion_set(&mut self, s: usize, i: usize, k: usize, name: &str, v: Value) -> R<()> {
        match name {
            "type" => self.set_criterion(s, i, k, Some(v.to_i64()?), None),
            "value" => self.set_criterion(s, i, k, None, Some(v)),
            _ => Err(Error::no_member(name)),
        }
    }

    /// Changes the type or value of a threshold.
    fn set_criterion(&mut self, s: usize, i: usize, k: usize, ty: Option<i64>, v: Option<Value>) -> R<()> {
        let rule = self.rule(s, i)?;
        let old = Self::cfvo(&rule.kind, k).ok_or_else(object_error)?;
        let (old_ty, old_v) = cfvo_out(&old);
        let ty = ty.unwrap_or(old_ty as i64);
        let num = |v: &Value| -> R<f64> { v.to_f64() };
        let v = v.unwrap_or(old_v);
        let c = match ty {
            1 | 7 => Cfvo::Min,
            2 | 8 => Cfvo::Max,
            0 => Cfvo::Num(num(&v)?),
            3 => Cfvo::Percent(num(&v)?),
            5 => Cfvo::Percentile(num(&v)?),
            4 => Cfvo::Formula(self.rule_formula(v, rule.origin())?),
            _ => return Err(Error::invalid_call()),
        };
        if let (CfKind::IconSet { .. }, 1) = (&rule.kind, k) {
            // The first icon always starts at the lowest value.
            return Ok(());
        }
        self.change_rule(s, i, |x| {
            match &mut x.kind {
                CfKind::ColorScale(stops) => stops[k - 1].0 = c,
                CfKind::DataBar { min, max, .. } => *(if k == 1 { min } else { max }) = c,
                CfKind::IconSet { points, .. } => points[k - 2] = c,
                _ => return Err(object_error()),
            }
            Ok(())
        })
    }

    /// The color of a stop of a color scale (`k`), or a data bar's (0).
    pub(super) fn cf_color(&mut self, s: usize, i: usize, k: usize, name: &str, set: Option<Value>) -> R<Value> {
        let rule = self.rule(s, i)?;
        let now = match (&rule.kind, k) {
            (CfKind::DataBar { color, .. }, 0) => *color,
            (CfKind::ColorScale(stops), k) if k >= 1 && k <= stops.len() => stops[k - 1].1,
            _ => return Err(object_error()),
        };
        match (name, set) {
            ("color", None) => Ok(Value::Long(swap_rb(now) as i32)),
            ("color", Some(v)) => {
                let c = color_in(&v)?;
                self.change_rule(s, i, |x| {
                    match &mut x.kind {
                        CfKind::DataBar { color, .. } => *color = c,
                        CfKind::ColorScale(stops) => stops[k - 1].1 = c,
                        _ => return Err(object_error()),
                    }
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            ("tintandshade" | "themecolor", Some(_)) => Ok(Value::Empty),
            _ => Err(Error::no_member(name)),
        }
    }

    /// Workbook.IconSets(XlIconSet).
    pub(super) fn icon_sets(&mut self, a: &Args) -> R<Value> {
        let Some(v) = a.get(0, "index") else { return Ok(part(Part::IconSets)) };
        let n = self.scalar(v)?.to_i64()?;
        match usize::try_from(n).ok().filter(|n| (1..=ICONS.len()).contains(n)) {
            Some(n) => Ok(part(Part::IconSet(n))),
            None => Err(Error::subscript()),
        }
    }
}

