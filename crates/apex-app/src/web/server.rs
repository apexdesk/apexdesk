//! The user's files on the server that serves the page (`apexdesk serve`):
//! listed and read at the start, written back when saved. The session is
//! the server's cookie, given at its sign-in page.

use std::cell::Cell;
use std::path::Path;

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

thread_local! {
    static CONNECTED: Cell<bool> = const { Cell::new(false) };
}

/// Whether the page came from `apexdesk serve` with a signed-in user.
pub fn connected() -> bool {
    CONNECTED.get()
}

async fn fetch(method: &str, url: &str, body: Option<&[u8]>) -> Option<web_sys::Response> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    if let Some(b) = body {
        let arr = js_sys::Uint8Array::from(b);
        init.set_body(&arr);
    }
    let req = web_sys::Request::new_with_str_and_init(url, &init).ok()?;
    let resp = JsFuture::from(web_sys::window()?.fetch_with_request(&req)).await.ok()?;
    resp.dyn_into::<web_sys::Response>().ok()
}

async fn bytes(resp: &web_sys::Response) -> Option<Vec<u8>> {
    let buf = JsFuture::from(resp.array_buffer().ok()?).await.ok()?;
    Some(js_sys::Uint8Array::new(&buf).to_vec())
}

/// A file next to the page (fonts...).
pub async fn get(url: &str) -> Option<Vec<u8>> {
    let r = fetch("GET", url, None).await.filter(|r| r.ok())?;
    bytes(&r).await
}

/// Asks the server who is signed in (nothing answers on a plain web host).
pub async fn connect() -> bool {
    let ok = match fetch("GET", "api/me", None).await {
        Some(r) if r.ok() => bytes(&r).await.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).is_some_and(|v| v.get("user").is_some()),
        _ => false,
    };
    CONNECTED.set(ok);
    ok
}

/// The address of a file of the user's: its path under `root`, each part
/// encoded.
fn url_of(path: &Path) -> Option<String> {
    let rel = path.strip_prefix(super::FILES).ok()?;
    let parts: Vec<String> = rel.components().map(|c| String::from(js_sys::encode_uri_component(&c.as_os_str().to_string_lossy()))).collect();
    (!parts.is_empty()).then(|| format!("api/files/{}", parts.join("/")))
}

/// Reads the user's files into `root`.
pub async fn load(root: &str) {
    let Some(r) = fetch("GET", "api/files", None).await.filter(|r| r.ok()) else { return };
    let Some(list) = bytes(&r).await.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else { return };
    for item in list.as_array().into_iter().flatten() {
        let Some(rel) = item.get("path").and_then(|p| p.as_str()) else { continue };
        // Only plain relative paths.
        if rel.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
            continue;
        }
        let path = Path::new(root).join(rel);
        if item.get("dir").and_then(|d| d.as_bool()) == Some(true) {
            let _ = apex_fs::create_dir_all(&path);
            continue;
        }
        let Some(url) = url_of(&path) else { continue };
        if let Some(r) = fetch("GET", &url, None).await.filter(|r| r.ok())
            && let Some(b) = bytes(&r).await
        {
            let _ = apex_fs::write(&path, b);
        }
    }
}

/// Writes a file of the user's to the server.
pub async fn put(path: &Path) {
    let (Some(url), Ok(b)) = (url_of(path), apex_fs::read(path)) else { return };
    if !fetch("PUT", &url, Some(&b)).await.is_some_and(|r| r.ok()) {
        web_sys::console::error_1(&format!("ApexDesk: could not save {} on the server", path.display()).into());
    }
}

/// Removes a file of the user's from the server.
pub async fn delete(path: &Path) {
    let Some(url) = url_of(path) else { return };
    let _ = fetch("DELETE", &url, None).await;
}

/// Runs a database source on the server (Get Data's databases, which the
/// browser can't reach). The query waits for the answer, like on the
/// desktop.
pub fn db(src: &apex_sheet::query::Source, password: &str) -> Result<apex_query::table::DataTable, apex_query::Error> {
    use apex_query::Error;
    let body = crate::db_wire::request(src, password).ok_or_else(|| Error::Invalid("not a database".into()))?;
    let fail = |_| Error::Network("the server could not be reached".into());
    let xhr = web_sys::XmlHttpRequest::new().map_err(fail)?;
    xhr.open_with_async("POST", "api/db", false).map_err(fail)?;
    xhr.set_request_header("Content-Type", "application/json").map_err(fail)?;
    xhr.send_with_opt_str(Some(&body)).map_err(fail)?;
    let text = xhr.response_text().map_err(fail)?.unwrap_or_default();
    if xhr.status().map_err(fail)? != 200 {
        let msg = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.get("error")?.as_str().map(str::to_string));
        return Err(Error::Database(msg.unwrap_or_else(|| "the server did not run the query".into())));
    }
    crate::db_wire::parse_table(&text).ok_or_else(|| Error::Database("the server's answer could not be read".into()))
}
