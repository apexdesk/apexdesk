//! File > AI Assistants (MCP): turning the connection on, and how to add
//! `apexdesk mcp` to an assistant; and the calls that arrive from it (see
//! `crate::mcp`), run on the tabs between frames.

use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use super::{App, View};
use crate::i18n::tr;
use crate::mcp::link::Request;
use crate::mcp::tools::to_text;

impl App {
    /// Runs the calls that arrived since the last frame.
    pub(super) fn mcp_requests(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.mcp else { return };
        let calls: Vec<Request> = rx.try_iter().collect();
        if calls.is_empty() {
            return;
        }
        for r in calls {
            let res = if self.settings.mcp { self.mcp_run(&r.tool, &r.args) } else { Err("Connections from AI assistants are turned off in ApexDesk: File > AI Assistants (MCP).".to_string()) };
            let _ = r.reply.send(res);
        }
        ctx.request_repaint();
    }

    /// Tabs an assistant can use: one per workbook shown in several windows.
    fn mcp_tabs(&self) -> Vec<usize> {
        (0..self.tabs.len()).filter(|&i| self.tabs[i].link.is_none() || self.tabs[i].holds).collect()
    }

    fn mcp_tab(&self, args: &Value) -> Result<usize, String> {
        let tabs = self.mcp_tabs();
        let pick = match args.get("document") {
            None | Some(Value::Null) => self.tabs.get(self.active).map(|t| (t.link, t.id)).and_then(|(link, id)| tabs.iter().copied().find(|&i| self.tabs[i].id == id || link.is_some() && self.tabs[i].link == link)),
            Some(v) => {
                let s = v.as_str().map(str::trim).map(str::to_string).unwrap_or_else(|| v.to_string());
                match s.parse::<usize>() {
                    Ok(n) => tabs.get(n).copied(),
                    Err(_) => tabs.iter().copied().find(|&i| self.tabs[i].name().eq_ignore_ascii_case(&s)).or_else(|| {
                        let low = s.to_lowercase();
                        tabs.iter().copied().find(|&i| self.tabs[i].name().to_lowercase().contains(&low))
                    }),
                }
            }
        };
        pick.ok_or_else(|| "no such tab: call list_documents".to_string())
    }

    fn mcp_list(&self) -> String {
        let docs: Vec<Value> = self
            .mcp_tabs()
            .into_iter()
            .enumerate()
            .map(|(n, i)| {
                let t = &self.tabs[i];
                let (kind, more) = match &t.view {
                    View::Sheet(v) => ("spreadsheet", v.mcp_info()),
                    View::Doc(v) => ("document", v.mcp_info()),
                    View::Slides(_) => ("presentation", json!({})),
                    View::Pdf(_) => ("pdf", json!({})),
                    View::Db(_) => ("database", json!({})),
                    View::Note(_) => ("notebook", json!({})),
                };
                let mut o = json!({ "document": n, "name": t.name(), "kind": kind, "file": t.path.as_ref().map(|p| p.to_string_lossy()), "unsaved": t.view.modified(), "shown": i == self.active });
                if let (Some(o), Value::Object(m)) = (o.as_object_mut(), more) {
                    o.extend(m);
                }
                o
            })
            .collect();
        to_text(&json!({ "documents": docs }))
    }

    fn mcp_run(&mut self, tool: &str, args: &Value) -> Result<String, String> {
        if tool == "list_documents" {
            return Ok(self.mcp_list());
        }
        let i = self.mcp_tab(args)?;
        let id = self.tabs[i].id;
        let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        match tool {
            "read_macros" => {
                let modules: Vec<Value> = self.read_modules(id)?.into_iter().map(|(name, kind, code)| json!({ "module": name, "kind": kind, "code": code })).collect();
                let enabled = super::macro_run::allowed(&self.settings, &self.tabs, id) && self.macro_run.is_enabled(id);
                return Ok(to_text(&json!({ "modules": modules, "macros_enabled": enabled })));
            }
            "write_macro" => {
                let name = text("module").ok_or("module is required")?;
                let code = args.get("code").and_then(Value::as_str).ok_or("code is required")?;
                let created = self.write_module(id, name, code, args.get("kind").and_then(Value::as_str) == Some("class"))?;
                return Ok(to_text(&json!({ "created": created, "macros_enabled": false, "note": "The file's macros are disabled until the user enables them again in ApexDesk." })));
            }
            "run_macro" => {
                let name = text("macro").ok_or("macro is required")?;
                let list: Vec<String> = args.get("args").and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string)).collect()).unwrap_or_default();
                self.mcp_run_macro(id, name, list)?;
                return Ok(to_text(&json!({ "started": true })));
            }
            _ => {}
        }
        match (&mut self.tabs[i].view, tool) {
            (View::Sheet(v), "undo" | "redo") => {
                let can = if tool == "undo" { v.wb.can_undo() } else { v.wb.can_redo() };
                if tool == "undo" { v.undo() } else { v.redo() }
                Ok(to_text(&json!({ "done": can })))
            }
            (View::Doc(v), "undo" | "redo") => {
                let can = if tool == "undo" { v.doc.can_undo() } else { v.doc.can_redo() };
                if tool == "undo" { v.undo() } else { v.redo() }
                Ok(to_text(&json!({ "done": can })))
            }
            (View::Sheet(v), _) => v.mcp_call(tool, args),
            (View::Doc(v), _) => v.mcp_call(tool, args),
            _ => Err("only spreadsheets and text documents can be read and edited".to_string()),
        }
    }

    pub(super) fn open_mcp(&mut self) {
        self.mcp_dlg = true;
    }

    pub(super) fn mcp_dialog(&mut self, ctx: &egui::Context) {
        if !self.mcp_dlg {
            return;
        }
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "apexdesk".into());
        let config = serde_json::to_string_pretty(&json!({ "mcpServers": { "apexdesk": { "command": exe, "args": ["mcp"] } } })).unwrap_or_default();
        let mut on = self.settings.mcp;
        let mut api = self.settings.automation_api;
        let listening = self.mcp.is_some();
        let closed = crate::widgets::info_dialog(ctx, "mcp_dialog", tr("mcp.title"), 560.0, |ui| {
            ui.label(tr("mcp.intro"));
            ui.add_space(8.0);
            crate::policy::guard(ui, "AllowAIAssistants", |ui| ui.checkbox(&mut on, RichText::new(tr("mcp.enable")).strong()));
            if !listening {
                ui.label(RichText::new(tr("mcp.other_window")).weak());
            }
            ui.add_space(8.0);
            ui.label(tr("mcp.how"));
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("\"{exe}\" mcp")).monospace());
                if ui.small_button(tr("mcp.copy")).clicked() {
                    ui.ctx().copy_text(format!("\"{exe}\" mcp"));
                }
            });
            ui.add_space(6.0);
            ui.label(tr("mcp.config"));
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(RichText::new(&config).monospace());
            });
            if ui.small_button(tr("mcp.copy")).clicked() {
                ui.ctx().copy_text(config.clone());
            }
            ui.add_space(6.0);
            ui.label(RichText::new(tr("mcp.undo_note")).weak());
            if crate::license::has("management") {
                crate::widgets::section(ui, tr("api.title"));
                ui.label(tr("api.intro"));
                crate::policy::guard(ui, "AllowAutomationAPI", |ui| ui.checkbox(&mut api, RichText::new(tr("api.enable")).strong()));
                ui.label(RichText::new(tr("api.note")).weak());
            }
        });
        if on != self.settings.mcp || api != self.settings.automation_api {
            self.settings.mcp = on;
            self.settings.automation_api = api;
            self.settings.save();
        }
        if closed {
            self.mcp_dlg = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macros_for_assistants() {
        let ctx = egui::Context::default();
        let settings = crate::settings::Settings { macros_enabled: true, mcp: true, ..Default::default() };
        let mut app = App::blank(&ctx, settings, None, std::time::Instant::now());
        app.push_tab(View::Sheet(Box::new(crate::sheet_view::SheetView::new(apex_sheet::Workbook::new()))), None, None);
        app.active = 0;
        let id = app.tabs[0].id;
        let call = |app: &mut App, tool: &str, args: Value| app.mcp_run(tool, &args).map(|t| serde_json::from_str::<Value>(&t).unwrap());
        // A workbook without macros gets a project.
        let r = call(&mut app, "write_macro", json!({ "module": "Tools", "code": "Sub Fill()\r\n    Range(\"A1\").Value = 7\r\nEnd Sub\r\n" })).unwrap();
        assert_eq!(r["created"], true);
        let r = call(&mut app, "read_macros", json!({})).unwrap();
        let tools = r["modules"].as_array().unwrap().iter().find(|m| m["module"] == "Tools").unwrap();
        assert_eq!(tools["code"], "Sub Fill()\n    Range(\"A1\").Value = 7\nEnd Sub\n");
        assert_eq!(r["macros_enabled"], false);
        assert!(call(&mut app, "write_macro", json!({ "module": "1bad", "code": "" })).is_err());
        // Runs only once the user enabled them, and writing disables them.
        assert!(call(&mut app, "run_macro", json!({ "macro": "Fill" })).unwrap_err().contains("enable"));
        app.enable_macros(id);
        assert_eq!(call(&mut app, "run_macro", json!({ "macro": "Fill" })).unwrap()["started"], true);
        assert!(call(&mut app, "run_macro", json!({ "macro": "Fill" })).unwrap_err().contains("already"));
        app.macro_run_take_queue_for_test();
        assert!(call(&mut app, "run_macro", json!({ "macro": "PERSONAL.XLSB!Fill" })).is_err());
        call(&mut app, "write_macro", json!({ "module": "Tools", "code": "Sub Fill()\nEnd Sub\n" })).unwrap();
        assert!(!app.macro_run.is_enabled(id));
        assert!(call(&mut app, "run_macro", json!({ "macro": "Fill" })).is_err());
        // Undone as one step.
        assert_eq!(call(&mut app, "undo", json!({})).unwrap()["done"], true);
        let r = call(&mut app, "read_macros", json!({})).unwrap();
        assert!(r["modules"].as_array().unwrap().iter().any(|m| m["code"].as_str().unwrap().contains("= 7")));
    }
}
