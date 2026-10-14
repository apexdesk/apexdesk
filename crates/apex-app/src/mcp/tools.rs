//! The MCP tools: their descriptions and input schemas, and small helpers
//! for the code that runs them on a tab (`sheet_view::mcp_tools`,
//! `doc_view::mcp_tools`, `app::mcp_ui`).

use serde_json::{Value, json};

const DOCUMENT: &str = "Tab name or number from list_documents; the tab shown when left out.";
const SHEET: &str = "Sheet name; the sheet shown when left out.";

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    let mut p = json!({ "document": { "type": "string", "description": DOCUMENT } });
    if let (Some(all), Value::Object(more)) = (p.as_object_mut(), props) {
        all.extend(more);
    }
    json!({ "name": name, "description": description, "inputSchema": { "type": "object", "properties": p, "required": required } })
}

fn color_prop(what: &str) -> Value {
    json!({ "type": "string", "description": format!("{what} as #RRGGBB; \"none\" removes it.") })
}

/// Every tool, for `tools/list`.
pub fn catalog() -> Vec<Value> {
    let format_cells = json!({
        "sheet": { "type": "string", "description": SHEET },
        "range": { "type": "string", "description": "Cells in A1 notation, like B2:D9." },
        "bold": { "type": "boolean" },
        "italic": { "type": "boolean" },
        "underline": { "type": "boolean" },
        "font": { "type": "string", "description": "Font family." },
        "font_size": { "type": "number", "description": "Points." },
        "color": color_prop("Text color"),
        "fill": color_prop("Background color"),
        "align": { "type": "string", "enum": ["general", "left", "center", "right", "justify"] },
        "wrap": { "type": "boolean" },
        "number_format": { "type": "string", "description": "Number format code, like 0.00, #,##0 or yyyy-mm-dd." },
    });
    let format_text = json!({
        "paragraph": { "type": "integer", "description": "First paragraph (0-based, from read_text)." },
        "to_paragraph": { "type": "integer", "description": "Last paragraph; the first one when left out." },
        "start": { "type": "integer", "description": "First character in the first paragraph; 0 when left out." },
        "end": { "type": "integer", "description": "Character after the last one in the last paragraph; its end when left out." },
        "bold": { "type": "boolean" },
        "italic": { "type": "boolean" },
        "underline": { "type": "boolean" },
        "strike": { "type": "boolean" },
        "font": { "type": "string" },
        "size": { "type": "number", "description": "Points." },
        "color": color_prop("Text color"),
        "highlight": color_prop("Highlight"),
        "style": { "type": "string", "enum": ["normal", "title", "heading1", "heading2", "heading3", "heading4", "heading5", "heading6", "quote", "code", "bullet", "numbered"], "description": "Paragraph style of the paragraphs." },
        "align": { "type": "string", "enum": ["left", "center", "right", "justify"] },
    });
    vec![
        json!({ "name": "list_documents", "description": "Lists the tabs open in ApexDesk: number, name, kind (spreadsheet, document, ...), file, whether it has unsaved changes, and for spreadsheets their sheets and the selected cells.", "inputSchema": { "type": "object", "properties": {} } }),
        tool("read_range", "Reads cells of a spreadsheet: their values (numbers, text, true/false, errors as text, null when empty) row by row, and the formulas of the cells that have one.", json!({
            "sheet": { "type": "string", "description": SHEET },
            "range": { "type": "string", "description": "Cells in A1 notation, like A1:D20; the used area when left out." },
        }), &[]),
        tool("write_cells", "Writes values or formulas into a spreadsheet from `start`, row by row, as one step the user can undo. Numbers and true/false go as JSON values; text starting with = is a formula in English syntax (commas between arguments, like =SUM(A1:A9, 2)); other text is entered as if typed (dates and percentages are recognized); null empties a cell.", json!({
            "sheet": { "type": "string", "description": SHEET },
            "start": { "type": "string", "description": "Top left cell, like B2." },
            "values": { "type": "array", "description": "Rows, each an array of cells.", "items": { "type": "array", "items": {} } },
        }), &["start", "values"]),
        tool("format_cells", "Formats cells of a spreadsheet (font, colors, alignment, number format) as one step the user can undo. Only the properties given change.", format_cells, &["range"]),
        tool("read_text", "Reads the paragraphs of a text document: number, style and text (in tables, the table cell is noted).", json!({
            "from": { "type": "integer", "description": "First paragraph (0-based); 0 when left out." },
            "count": { "type": "integer", "description": "Paragraphs to read; at most 500." },
        }), &[]),
        tool("insert_text", "Inserts text into a text document, as one step the user can undo. A line break (\\n) starts a new paragraph.", json!({
            "text": { "type": "string" },
            "paragraph": { "type": "integer", "description": "Paragraph to insert into (0-based); after the last one when left out, in a new paragraph." },
            "offset": { "type": "integer", "description": "Character in the paragraph to insert before; its end when left out." },
        }), &["text"]),
        tool("set_paragraph", "Replaces the text of one paragraph of a text document, keeping its style, as one step the user can undo.", json!({
            "paragraph": { "type": "integer", "description": "Paragraph number (0-based)." },
            "text": { "type": "string" },
        }), &["paragraph", "text"]),
        tool("replace_text", "Replaces every occurrence of a text in a text document, as one step the user can undo. Returns how many were replaced.", json!({
            "find": { "type": "string" },
            "replace": { "type": "string" },
            "match_case": { "type": "boolean" },
            "whole_word": { "type": "boolean" },
        }), &["find", "replace"]),
        tool("format_text", "Formats characters and paragraphs of a text document (bold, font, size, colors, paragraph style, alignment) as one step the user can undo. Only the properties given change.", format_text, &["paragraph"]),
        tool("read_macros", "Reads the VBA macros of a spreadsheet: each module's name, kind (module, class, document, form) and code, and whether the user enabled the file's macros.", json!({}), &[]),
        tool("write_macro", "Writes the VBA code of a module of a spreadsheet (a new module when none has that name), as one step the user can undo. The file's macros are disabled until the user enables them again, and a signed project loses its signature.", json!({
            "module": { "type": "string", "description": "Module name, like Module1 or ThisWorkbook." },
            "code": { "type": "string", "description": "The module's whole code, without the Attribute lines." },
            "kind": { "type": "string", "enum": ["module", "class"], "description": "For a new module; module when left out." },
        }), &["module", "code"]),
        tool("run_macro", "Starts a macro of a spreadsheet whose macros the user enabled; it isn't waited for (its message boxes show to the user). Read the result with read_range.", json!({
            "macro": { "type": "string", "description": "Sub name, like Main or Module1.Main." },
            "args": { "type": "array", "description": "Arguments for its parameters: numbers, true/false or text.", "items": {} },
        }), &["macro"]),
        tool("undo", "Undoes the last change in a tab (the assistant's or the user's), like Ctrl+Z.", json!({}), &[]),
        tool("redo", "Redoes the last undone change in a tab, like Ctrl+Y.", json!({}), &[]),
    ]
}

pub fn exists(name: &str) -> bool {
    catalog().iter().any(|t| t["name"] == name)
}

/// A tool's result as JSON text.
pub fn to_text(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

pub fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

pub fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub fn opt_usize(args: &Value, key: &str) -> Result<Option<usize>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(|n| Some(n as usize)).ok_or_else(|| format!("{key} must be a number from 0")),
    }
}

/// A color argument: `Some(None)` removes the color.
pub fn opt_color(args: &Value, key: &str) -> Result<Option<Option<u32>>, String> {
    let Some(s) = opt_str(args, key) else { return Ok(None) };
    let t = s.trim().trim_start_matches('#');
    if t.eq_ignore_ascii_case("none") {
        return Ok(Some(None));
    }
    match u32::from_str_radix(t, 16) {
        Ok(n) if t.len() == 6 => Ok(Some(Some(n))),
        _ => Err(format!("{key}: \"{s}\" is not a #RRGGBB color")),
    }
}

/// The byte offset of character `n` of `s` (its end when past it).
pub fn byte_of(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        let a = json!({ "c": "#1e8e4e", "n": "none", "bad": "#12", "i": 3, "neg": -1 });
        assert_eq!(opt_color(&a, "c"), Ok(Some(Some(0x1E8E4E))));
        assert_eq!(opt_color(&a, "n"), Ok(Some(None)));
        assert!(opt_color(&a, "bad").is_err());
        assert_eq!(opt_color(&a, "missing"), Ok(None));
        assert_eq!(opt_usize(&a, "i"), Ok(Some(3)));
        assert!(opt_usize(&a, "neg").is_err());
        assert_eq!(byte_of("añb", 2), 3);
        assert_eq!(byte_of("añb", 9), 4);
        assert!(exists("write_cells") && !exists("rm"));
        // Every schema names its required properties.
        for t in catalog() {
            let props = &t["inputSchema"]["properties"];
            for r in t["inputSchema"]["required"].as_array().into_iter().flatten() {
                assert!(props.get(r.as_str().unwrap()).is_some(), "{}", t["name"]);
            }
        }
    }
}
