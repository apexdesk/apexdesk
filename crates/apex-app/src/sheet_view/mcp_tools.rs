//! The MCP tools on a workbook (see `crate::mcp`): reading cells, writing
//! values and formulas, formatting. Each edit is one undo step.

use std::sync::Arc;

use apex_sheet::sheet::HAlign;
use apex_sheet::{CellRef, Locale, Range, Value};
use serde_json::{Value as Json, json};

use super::SheetView;
use crate::mcp::tools::{opt_bool, opt_color, opt_str, to_text};

/// Cells one call reads or writes at most.
const MAX_CELLS: u64 = 50_000;

impl SheetView {
    /// What `list_documents` says about the workbook.
    pub(crate) fn mcp_info(&self) -> Json {
        let sheets: Vec<&str> = self.wb.sheets.iter().filter(|s| !s.hidden).map(|s| s.name.as_str()).collect();
        json!({ "sheets": sheets, "sheet": self.wb.sheets[self.sheet].name, "selection": self.sel().to_a1() })
    }

    fn mcp_sheet(&self, args: &Json) -> Result<usize, String> {
        match opt_str(args, "sheet") {
            None => Ok(self.sheet),
            // Hidden sheets stay out of reach, as out of `list_documents`.
            Some(n) => self.wb.sheets.iter().position(|s| !s.hidden && s.name.eq_ignore_ascii_case(n.trim())).ok_or_else(|| format!("no sheet named \"{n}\"")),
        }
    }

    fn mcp_blocked(&mut self) -> Result<(), String> {
        if self.wb.take_blocked() { Err("the workbook is read-only or the sheet is protected".to_string()) } else { Ok(()) }
    }

    pub(crate) fn mcp_call(&mut self, tool: &str, args: &Json) -> Result<String, String> {
        match tool {
            "read_range" => self.mcp_read(args),
            "write_cells" => self.mcp_write(args),
            "format_cells" => self.mcp_format(args),
            _ => Err(format!("{tool} works on text documents, and this tab is a spreadsheet")),
        }
    }

    fn mcp_read(&self, args: &Json) -> Result<String, String> {
        let s = self.mcp_sheet(args)?;
        let sh = &self.wb.sheets[s];
        let r = match opt_str(args, "range") {
            Some(t) => Range::parse(t).ok_or_else(|| format!("\"{t}\" is not a range"))?,
            None => match sh.extent() {
                Some(e) => Range::new(CellRef::new(0, 0), e),
                None => return Ok(to_text(&json!({ "sheet": sh.name, "range": null, "values": [] }))),
            },
        };
        // Whole columns or rows read as far as the used area.
        let r = if r.cell_count() > MAX_CELLS { self.wb.clamp_to_used(s, &r).unwrap_or(Range::single(r.start)) } else { r };
        if (r.rows() as u64) * (r.cols() as u64) > MAX_CELLS {
            return Err(format!("{} has more than {MAX_CELLS} cells: read it in parts", r.to_a1()));
        }
        let mut rows = Vec::new();
        let mut formulas = serde_json::Map::new();
        let mut shown = serde_json::Map::new();
        for row in r.start.row..=r.end.row {
            let mut out = Vec::new();
            for col in r.start.col..=r.end.col {
                let c = CellRef::new(row, col);
                out.push(match sh.value(c) {
                    Value::Empty => Json::Null,
                    Value::Number(n) => {
                        // Dates, percentages, currencies: also as shown.
                        let text = self.wb.display(s, c).0;
                        if text != self.wb.format_value(0, &Value::Number(*n)).0 {
                            shown.insert(c.to_a1(), json!(text));
                        }
                        json!(n)
                    }
                    Value::Text(t) => json!(t.as_ref()),
                    Value::Bool(b) => json!(b),
                    _ => json!(self.wb.display(s, c).0),
                });
                if let Some(f) = sh.get(c).and_then(|x| x.formula()) {
                    formulas.insert(c.to_a1(), json!(f.expr().to_display(&Locale::EN)));
                }
            }
            rows.push(Json::Array(out));
        }
        let mut res = json!({ "sheet": sh.name, "range": r.to_a1(), "values": rows });
        if !formulas.is_empty() {
            res["formulas"] = Json::Object(formulas);
        }
        if !shown.is_empty() {
            res["shown"] = Json::Object(shown);
        }
        Ok(to_text(&res))
    }

    /// What typing would enter for `v`: formulas turned from English to
    /// the workbook's syntax, numbers to its decimal sign.
    fn mcp_input(&self, v: &Json, c: CellRef) -> Result<String, String> {
        let loc = self.wb.locale;
        Ok(match v {
            Json::Null => String::new(),
            Json::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
            Json::Number(n) => {
                let n = n.as_f64().ok_or("bad number")?;
                let t = n.to_string();
                if loc.decimal == '.' { t } else { t.replace('.', &loc.decimal.to_string()) }
            }
            Json::String(t) if t.len() > 1 && t.starts_with('=') => {
                let e = apex_sheet::formula::parse(t, &Locale::EN).map_err(|e| format!("{}: {e}", c.to_a1()))?;
                if loc == Locale::EN { t.clone() } else { e.to_display(&loc) }
            }
            Json::String(t) => t.clone(),
            _ => return Err(format!("{}: a cell is a number, text, true/false or null", c.to_a1())),
        })
    }

    fn mcp_write(&mut self, args: &Json) -> Result<String, String> {
        let s = self.mcp_sheet(args)?;
        let start = opt_str(args, "start").and_then(CellRef::parse).ok_or("start must be a cell like B2")?;
        let rows = args.get("values").and_then(Json::as_array).ok_or("values must be an array of rows")?;
        let mut inputs = Vec::new();
        let mut end = start;
        for (i, row) in rows.iter().enumerate() {
            let cells: Vec<&Json> = match row {
                Json::Array(a) => a.iter().collect(),
                // A plain value is a row of one cell.
                v => vec![v],
            };
            for (j, v) in cells.into_iter().enumerate() {
                let c = CellRef::new(start.row + i as u32, start.col + j as u32);
                if c.row > super::LAST.row || c.col > super::LAST.col {
                    return Err("the values go past the end of the sheet".to_string());
                }
                inputs.push((c, self.mcp_input(v, c)?));
                end = CellRef::new(end.row.max(c.row), end.col.max(c.col));
            }
        }
        if inputs.is_empty() {
            return Err("no values to write".to_string());
        }
        if inputs.len() as u64 > MAX_CELLS {
            return Err(format!("more than {MAX_CELLS} cells: write them in parts"));
        }
        let n = inputs.len();
        self.settle_edit();
        self.wb.set_inputs(s, inputs);
        self.mcp_blocked()?;
        Ok(to_text(&json!({ "written": n, "range": Range::new(start, end).to_a1() })))
    }

    fn mcp_format(&mut self, args: &Json) -> Result<String, String> {
        let s = self.mcp_sheet(args)?;
        let t = opt_str(args, "range").ok_or("range is missing")?;
        let r = Range::parse(t).ok_or_else(|| format!("\"{t}\" is not a range"))?;
        let (bold, italic, underline, wrap) = (opt_bool(args, "bold"), opt_bool(args, "italic"), opt_bool(args, "underline"), opt_bool(args, "wrap"));
        let font: Option<Arc<str>> = opt_str(args, "font").map(Arc::from);
        let size = match args.get("font_size").and_then(Json::as_f64) {
            Some(p) if (1.0..=409.0).contains(&p) => Some((p * 10.0).round() as u16),
            Some(_) => return Err("font_size must be 1 to 409 points".to_string()),
            None => None,
        };
        let (color, fill) = (opt_color(args, "color")?, opt_color(args, "fill")?);
        let align = match opt_str(args, "align") {
            None => None,
            Some("general") => Some(HAlign::General),
            Some("left") => Some(HAlign::Left),
            Some("center") => Some(HAlign::Center),
            Some("right") => Some(HAlign::Right),
            Some("justify") => Some(HAlign::Justify),
            Some(a) => return Err(format!("unknown alignment \"{a}\"")),
        };
        let num_fmt: Option<Arc<str>> = opt_str(args, "number_format").map(Arc::from);
        self.settle_edit();
        self.wb.modify_style_as(s, &r, "undo.format", |st| {
            if let Some(b) = bold {
                st.bold = b;
            }
            if let Some(b) = italic {
                st.italic = b;
            }
            if let Some(b) = underline {
                st.underline = b;
            }
            if let Some(b) = wrap {
                st.wrap = b;
            }
            if let Some(f) = &font {
                st.font = Some(f.clone());
            }
            if let Some(z) = size {
                st.font_size = z;
            }
            if let Some(c) = color {
                st.color = c;
            }
            if let Some(c) = fill {
                st.fill = c;
            }
            if let Some(a) = align {
                st.halign = a;
            }
            if let Some(f) = &num_fmt {
                st.num_fmt = f.clone();
            }
        });
        self.mcp_blocked()?;
        Ok(to_text(&json!({ "formatted": r.to_a1() })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::Workbook;

    #[test]
    fn read_write_and_format_undo() {
        let mut v = SheetView::new(Workbook::new());
        let res = v.mcp_call("write_cells", &json!({ "start": "B2", "values": [["Qty", "Price"], [2, 1.5], [3, "=SUM(B3:B3, 1)"], [true, null]] })).unwrap();
        assert!(res.contains("\"range\":\"B2:C5\""), "{res}");
        assert_eq!(v.wb.sheets[0].value(CellRef::new(3, 2)), &Value::Number(3.0));
        let undo_after_write = v.wb.undo_len();
        let read: Json = serde_json::from_str(&v.mcp_call("read_range", &json!({ "range": "B2:C4" })).unwrap()).unwrap();
        assert_eq!(read["values"], json!([["Qty", "Price"], [2.0, 1.5], [3.0, 3.0]]));
        assert_eq!(read["formulas"]["C4"], "=SUM(B3:B3,1)");
        // The used area when no range is given.
        let all: Json = serde_json::from_str(&v.mcp_call("read_range", &json!({})).unwrap()).unwrap();
        assert_eq!(all["range"], "A1:C5");
        v.mcp_call("format_cells", &json!({ "range": "B2:C2", "bold": true, "fill": "#FFEE00", "number_format": "0.00" })).unwrap();
        let st = v.wb.styles.get(v.wb.sheets[0].get(CellRef::new(1, 2)).unwrap().style).clone();
        assert!(st.bold && st.fill == Some(0xFFEE00) && st.num_fmt.as_ref() == "0.00");
        assert_eq!(v.wb.undo_len(), undo_after_write + 1);
        v.undo();
        assert!(!v.wb.styles.get(v.wb.sheets[0].get(CellRef::new(1, 2)).unwrap().style).bold);
        // The write was one step.
        v.undo();
        assert_eq!(v.wb.sheets[0].value(CellRef::new(1, 1)), &Value::Empty);
        // Errors.
        assert!(v.mcp_call("write_cells", &json!({ "start": "A1", "values": [["=SUM("]] })).is_err());
        assert!(v.mcp_call("read_range", &json!({ "sheet": "Nope" })).is_err());
        assert!(v.mcp_call("insert_text", &json!({})).is_err());
    }

    #[test]
    fn formulas_in_english_whatever_the_locale() {
        let mut wb = Workbook::new();
        wb.locale = Locale { decimal: ',', thousands: '.', arg_sep: ';', ..Locale::EN };
        let mut v = SheetView::new(wb);
        v.mcp_call("write_cells", &json!({ "start": "A1", "values": [[1.5, 2], ["=ROUND(A1*B1, 0)"]] })).unwrap();
        assert_eq!(v.wb.sheets[0].value(CellRef::new(0, 0)), &Value::Number(1.5));
        assert_eq!(v.wb.sheets[0].value(CellRef::new(1, 0)), &Value::Number(3.0));
        let read: Json = serde_json::from_str(&v.mcp_call("read_range", &json!({ "range": "A2" })).unwrap()).unwrap();
        assert_eq!(read["formulas"]["A2"], "=ROUND(A1*B1,0)");
    }
}
