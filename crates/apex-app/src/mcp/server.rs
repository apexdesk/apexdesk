//! `apexdesk mcp`: the MCP server on standard input and output. Messages
//! are JSON-RPC 2.0, one per line; it answers `initialize`, `ping`,
//! `tools/list` and `tools/call`, and passes each call to the window.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// Protocol versions it speaks, newest first.
const VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Tools to read and edit the spreadsheets and documents open in the ApexDesk window. \
Call list_documents first. Edits show in the window at once and each call is one step the user can undo.";

pub const MCP_HELP: &str = "Usage: apexdesk mcp

Runs an MCP (Model Context Protocol) server on standard input and output,
so an AI assistant can read and edit the workbooks and documents open in
the ApexDesk window. The window must be running, with File > AI Assistants
(MCP) turned on. Add it to the assistant's configuration as the command
'apexdesk' with the argument 'mcp'.
";

fn reply(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The answer to one message (`None` for notifications). `call` runs a
/// tool in the window.
pub fn handle(msg: &Value, call: &mut dyn FnMut(&str, &Value) -> Result<String, String>) -> Option<Value> {
    let method = msg["method"].as_str();
    let Some(id) = msg.get("id").filter(|i| !i.is_null()) else {
        // Notifications (initialized, cancelled) need no answer.
        return None;
    };
    let Some(method) = method else {
        // A response to something we never ask.
        return None;
    };
    let params = &msg["params"];
    Some(match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(VERSIONS[0]);
            reply(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "apexdesk", "title": "ApexDesk", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => reply(id, json!({})),
        "tools/list" => reply(id, json!({ "tools": super::tools::catalog() })),
        "tools/call" => {
            let Some(name) = params["name"].as_str() else { return Some(error(id, -32602, "missing tool name")) };
            if !super::tools::exists(name) {
                return Some(error(id, -32602, &format!("unknown tool: {name}")));
            }
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let (text, is_error) = match call(name, &args) {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            reply(id, json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
        }
        _ => error(id, -32601, &format!("method not found: {method}")),
    })
}

/// Serves `input` until it ends, writing answers to `output`.
pub fn serve(input: impl BufRead, mut output: impl Write, call: &mut dyn FnMut(&str, &Value) -> Result<String, String>) {
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let out = match serde_json::from_str::<Value>(&line) {
            // Batches were dropped from the protocol; answer each anyway.
            Ok(Value::Array(msgs)) => {
                let all: Vec<Value> = msgs.iter().filter_map(|m| handle(m, call)).collect();
                if all.is_empty() { None } else { Some(Value::Array(all)) }
            }
            Ok(m) => handle(&m, call),
            Err(e) => Some(error(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(o) = out {
            if writeln!(output, "{o}").and_then(|_| output.flush()).is_err() {
                break;
            }
        }
    }
}

/// `apexdesk mcp`.
pub fn run() -> i32 {
    let stdin = std::io::stdin();
    serve(stdin.lock(), std::io::stdout().lock(), &mut |tool, args| super::link::call(tool, args));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_lines(input: &str, call: &mut dyn FnMut(&str, &Value) -> Result<String, String>) -> Vec<Value> {
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, call);
        String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn speaks_the_protocol() {
        let input = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read_range","arguments":{"range":"A1"}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"write_cells","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"format_disk"}}"#,
            r#"{"jsonrpc":"2.0","id":"x","method":"nope"}"#,
            "not json",
            r#"{"jsonrpc":"2.0","id":6,"method":"ping"}"#,
        ]
        .join("\n");
        let mut calls = Vec::new();
        let out = run_lines(&input, &mut |t, a| {
            calls.push((t.to_string(), a.clone()));
            if t == "write_cells" { Err("the sheet is protected".into()) } else { Ok("[[1]]".into()) }
        });
        assert_eq!(out.len(), 8, "{out:?}");
        assert_eq!(out[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(out[0]["result"]["serverInfo"]["name"], "apexdesk");
        let tools = out[1]["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "write_cells" && t["inputSchema"]["type"] == "object"));
        assert_eq!(out[2]["result"]["content"][0]["text"], "[[1]]");
        assert_eq!(out[2]["result"]["isError"], false);
        assert_eq!(out[3]["result"]["isError"], true);
        assert_eq!(out[4]["error"]["code"], -32602);
        assert_eq!(out[5]["id"], "x");
        assert_eq!(out[5]["error"]["code"], -32601);
        assert_eq!(out[6]["error"]["code"], -32700);
        assert_eq!(out[7]["result"], json!({}));
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1["range"], "A1");
    }
}
