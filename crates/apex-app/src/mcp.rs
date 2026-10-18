//! MCP (Model Context Protocol): `apexdesk mcp` lets an AI assistant read
//! and edit the workbooks and documents open in the window. The assistant
//! starts it and talks JSON-RPC on its standard input and output
//! ([`server`]); each tool call goes to the running window over a local
//! socket ([`link`]), which runs it on its tabs ([`tools`]) as ordinary
//! undoable edits, so they show at once and Ctrl+Z takes them back.

pub mod link;
pub mod server;
pub mod tools;
