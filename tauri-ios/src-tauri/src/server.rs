//! Loopback-only HTTP server for the app.
//!  * serves the bundled `vscode-web` files (a real http origin is required: custom URI schemes have
//!    an opaque origin in WebKit, which breaks `new URL(..., window.location.origin)`), and
//!  * exposes `/__fs/*`: a file API over the app's whole Documents folder (what the Files app shows;
//!    `Linux` inside it is protected), used by the `ios-files` web extension, plus `open` (hand an
//!    https URL to Safari) and the GitHub device-flow proxy (github.com/login/* has no CORS).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use percent_encoding::percent_decode_str;
use serde_json::json;

/// Recent non-200 responses, exposed at /__log to help debug a workbench that fails to load.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Fixed port keeps the page origin stable so IndexedDB / localStorage persist across launches.
pub const PREFERRED_PORT: u16 = 38621;

const MAX_BODY: usize = 512 * 1024 * 1024;

/// Opens an https URL outside the app (Safari).
pub type OpenUrl = Arc<dyn Fn(String) + Send + Sync>;

pub enum ReadResult {
    Data(Vec<u8>),
    Timeout,
    Closed,
}

/// The Linux terminal backend (implemented in linux.rs; mocked in tests).
pub trait Terminal: Send + Sync {
    fn status(&self) -> (String, String);
    /// (phase, message, url): phase is starting | ready | fallback.
    fn boot_status(&self) -> (String, String, String);
    fn open(&self, cols: u16, rows: u16) -> Result<i32, String>;
    fn read(&self, id: i32, wait: Duration) -> ReadResult;
    fn write(&self, id: i32, data: &[u8]) -> Result<(), String>;
    fn resize(&self, id: i32, cols: u16, rows: u16) -> Result<(), String>;
    fn close(&self, id: i32);
}

struct Ctx {
    web_root: Option<PathBuf>,
    fs_root: Option<PathBuf>,
    open_url: Option<OpenUrl>,
    term: Option<Arc<dyn Terminal>>,
    open_token: String,
    port: u16,
}

struct Request {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct Reply {
    status: u16,
    mime: String,
    body: Vec<u8>,
}

fn reply(status: u16, mime: &str, body: Vec<u8>) -> Reply {
    Reply { status, mime: mime.to_string(), body }
}

fn text(status: u16, msg: &str) -> Reply {
    reply(status, "text/plain", msg.as_bytes().to_vec())
}

pub fn start(web_root: Option<PathBuf>, fs_root: Option<PathBuf>, open_url: Option<OpenUrl>, term: Option<Arc<dyn Terminal>>, open_token: String) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))?;
    let port = listener.local_addr()?.port();
    let ctx = Arc::new(Ctx { web_root, fs_root, open_url, term, open_token, port });
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let ctx = ctx.clone();
            thread::spawn(move || handle(stream, &ctx));
        }
    });
    Ok(port)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut data: Vec<u8> = Vec::new();
    let mut buf = [0u8; 16384];
    let header_end = loop {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(pos) = find(&data, b"\r\n\r\n") {
            break pos + 4;
        }
        if data.len() > 64 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if length > MAX_BODY {
        return None;
    }
    let mut body = data[header_end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }
    body.truncate(length);
    Some(Request { method, target, headers, body })
}

fn handle(mut stream: TcpStream, ctx: &Ctx) {
    let Some(req) = read_request(&mut stream) else { return };
    let (path, query) = match req.target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (req.target.clone(), String::new()),
    };

    let r = if path.starts_with("/__fs/") {
        api(ctx, &req, &path["/__fs/".len()..], &query)
    } else if path == "/__open" && req.method == "GET" {
        // Used by pages served from other loopback ports (the VS Code server): a per-launch token stands in
        // for the custom header, because a simple cross-origin request can't carry one.
        let q = parse_query(&query);
        let ok = q.get("t").map(|t| *t == ctx.open_token).unwrap_or(false);
        match (ok, q.get("u"), &ctx.open_url) {
            (true, Some(u), Some(open)) if u.starts_with("https://") => { open(u.clone()); text(200, "ok") }
            _ => text(403, "Forbidden"),
        }
    } else if req.method != "GET" && req.method != "HEAD" {
        text(405, "Method Not Allowed")
    } else if path == "/__log" {
        let log = LOG.lock().map(|l| l.join("\n")).unwrap_or_default();
        reply(200, "text/plain", log.into_bytes())
    } else {
        serve(ctx.web_root.as_deref(), &path)
    };

    if r.status != 200 && !path.starts_with("/__fs/") {
        if let Ok(mut l) = LOG.lock() {
            if l.len() < 300 {
                l.push(format!("{} {}", r.status, path));
            }
        }
    }

    let reason = match r.status {
        200 => "OK",
        204 => "No Content",
        410 => "Gone",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        502 => "Bad Gateway",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        r.status, reason, r.mime, r.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    if req.method != "HEAD" {
        let _ = stream.write_all(&r.body);
    }
}

fn serve(root: Option<&Path>, uri_path: &str) -> Reply {
    let Some(root) = root else {
        return text(500, "Bundled workbench (web/out) not found in app resources");
    };
    let decoded = percent_decode_str(uri_path).decode_utf8_lossy().to_string();
    let rel = decoded.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    if Path::new(rel).components().any(|c| !matches!(c, Component::Normal(_))) {
        return text(403, "Forbidden");
    }
    let file = root.join(rel);
    match std::fs::read(&file) {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&file).first_or_octet_stream().to_string();
            reply(200, &mime, bytes)
        }
        Err(_) => text(404, "Not found"),
    }
}

// ---------------------------------------------------------------- file API

fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), percent_decode_str(v).decode_utf8_lossy().to_string()))
        .collect()
}

struct Resolved {
    real: PathBuf,
    depth: usize,
    top: Option<String>,
}

/// Maps a workbench path like "/Linux/projects/a.txt" onto the real folder under the Documents root.
/// Anything that would leave the root (`..`, absolute tricks) is refused.
fn resolve(fs_root: &Path, p: &str) -> Result<Resolved, u16> {
    let mut comps: Vec<std::ffi::OsString> = Vec::new();
    for c in Path::new(p.trim_start_matches('/')).components() {
        match c {
            Component::Normal(s) => comps.push(s.to_owned()),
            Component::CurDir => {}
            _ => return Err(403),
        }
    }
    let top = comps.first().map(|c| c.to_string_lossy().to_string());
    let real = comps.iter().fold(fs_root.to_path_buf(), |a, c| a.join(c));
    Ok(Resolved { real, depth: comps.len(), top })
}

/// The root itself and the `Linux` folder (the Linux root filesystem, later) can't be deleted,
/// renamed over, or replaced.
fn protected(r: &Resolved) -> bool {
    r.depth == 0 || (r.depth == 1 && r.top.as_deref() == Some("Linux"))
}

fn status_of(e: &std::io::Error) -> u16 {
    match e.kind() {
        std::io::ErrorKind::NotFound => 404,
        std::io::ErrorKind::AlreadyExists => 409,
        std::io::ErrorKind::PermissionDenied => 403,
        _ => 500,
    }
}

fn ms(t: std::io::Result<std::time::SystemTime>) -> u64 {
    t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

fn remove_any(p: &Path) -> std::io::Result<()> {
    if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }
}

fn proxy_post(url: &str, body: &[u8]) -> Reply {
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(20)).build();
    let result = agent
        .post(url)
        .set("Accept", "application/json")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_bytes(body);
    let resp = match result {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => return text(502, &e.to_string()),
    };
    let mut buf = Vec::new();
    if resp.into_reader().take(1 << 20).read_to_end(&mut buf).is_err() {
        return text(502, "read failed");
    }
    reply(200, "application/json", buf)
}

fn terminal_api(ctx: &Ctx, req: &Request, op: &str, q: &HashMap<String, String>) -> Reply {
    let Some(term) = ctx.term.as_ref() else {
        return text(500, "terminal unavailable");
    };
    let num = |k: &str, default: u32| q.get(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(default);
    let id = q.get("id").and_then(|v| v.parse::<i32>().ok()).unwrap_or(-1);
    match (req.method.as_str(), op) {
        ("GET", "boot-status") => {
            let (phase, message, url) = term.boot_status();
            reply(200, "application/json", json!({"phase": phase, "message": message, "url": url}).to_string().into_bytes())
        }
        ("GET", "term-status") => {
            let (state, message) = term.status();
            reply(200, "application/json", json!({"state": state, "message": message}).to_string().into_bytes())
        }
        ("POST", "term-open") => match term.open(num("cols", 80).min(1000) as u16, num("rows", 24).min(1000) as u16) {
            Ok(id) => reply(200, "application/json", json!({"id": id}).to_string().into_bytes()),
            Err(e) => text(500, &e),
        },
        ("GET", "term-read") => match term.read(id, Duration::from_secs(20)) {
            ReadResult::Data(b) => reply(200, "application/octet-stream", b),
            ReadResult::Timeout => reply(204, "text/plain", Vec::new()),
            ReadResult::Closed => reply(410, "text/plain", Vec::new()),
        },
        ("POST", "term-write") => match term.write(id, &req.body) {
            Ok(_) => text(200, "ok"),
            Err(e) => text(404, &e),
        },
        ("POST", "term-resize") => match term.resize(id, num("cols", 80).min(1000) as u16, num("rows", 24).min(1000) as u16) {
            Ok(_) => text(200, "ok"),
            Err(e) => text(404, &e),
        },
        ("POST", "term-close") => {
            term.close(id);
            text(200, "ok")
        }
        _ => text(404, "unknown operation"),
    }
}

fn api(ctx: &Ctx, req: &Request, op: &str, query: &str) -> Reply {
    // A custom header forces a CORS preflight for cross-site pages (which we never answer), and the
    // Host check blocks DNS-rebinding. Same-origin requests from the workbench pass both.
    let host_ok = req.headers.get("host").map(|h| *h == format!("127.0.0.1:{}", ctx.port)).unwrap_or(false);
    if !host_ok || !req.headers.contains_key("x-vscode-fs") {
        return text(403, "Forbidden");
    }

    // Operations that don't touch files.
    match (req.method.as_str(), op) {
        ("POST", "open") => {
            let url = String::from_utf8_lossy(&req.body).trim().to_string();
            if !url.starts_with("https://") {
                return text(400, "only https URLs");
            }
            return match &ctx.open_url {
                Some(open) => { open(url); text(200, "ok") }
                None => text(500, "opener unavailable"),
            };
        }
        // Only these two GitHub endpoints are forwarded (not an open proxy).
        ("POST", "gh-device") => return proxy_post("https://github.com/login/device/code", &req.body),
        ("POST", "gh-token") => return proxy_post("https://github.com/login/oauth/access_token", &req.body),
        _ => {}
    }

    let q = parse_query(query);
    if op.starts_with("term-") || op == "boot-status" {
        return terminal_api(ctx, req, op, &q);
    }
    let Some(fs_root) = ctx.fs_root.as_deref() else {
        return text(500, "Documents folder unavailable");
    };
    let get = |k: &str| q.get(k).cloned().unwrap_or_default();
    let io_err = |e: std::io::Error| text(status_of(&e), &e.to_string());
    let res = |p: &str| resolve(fs_root, p);
    let json_ok = |v: serde_json::Value| reply(200, "application/json", v.to_string().into_bytes());

    match (req.method.as_str(), op) {
        ("GET", "stat") => {
            let r = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            match std::fs::metadata(&r.real) {
                Ok(m) => json_ok(json!({
                    "type": if m.is_dir() { 2 } else { 1 },
                    "size": m.len(),
                    "ctime": ms(m.created()),
                    "mtime": ms(m.modified()),
                })),
                Err(e) => io_err(e),
            }
        }
        ("GET", "list") => {
            let r = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            match std::fs::read_dir(&r.real) {
                Ok(rd) => {
                    let mut out = Vec::new();
                    for e in rd.flatten() {
                        let t = if e.path().is_dir() { 2 } else { 1 };
                        out.push(json!([e.file_name().to_string_lossy(), t]));
                    }
                    json_ok(json!(out))
                }
                Err(e) => io_err(e),
            }
        }
        ("GET", "read") => {
            let r = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            match std::fs::read(&r.real) {
                Ok(b) => reply(200, "application/octet-stream", b),
                Err(e) => io_err(e),
            }
        }
        ("PUT", "write") => {
            let r = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            if protected(&r) { return text(403, "denied"); }
            match std::fs::write(&r.real, &req.body) {
                Ok(_) => text(200, "ok"),
                Err(e) => io_err(e),
            }
        }
        ("POST", "mkdir") => {
            let r = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            if protected(&r) { return text(403, "denied"); }
            match std::fs::create_dir(&r.real) {
                Ok(_) => text(200, "ok"),
                Err(e) => io_err(e),
            }
        }
        ("DELETE", "delete") => {
            let r = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            if protected(&r) { return text(403, "denied"); }
            let result = if r.real.is_dir() {
                if get("recursive") == "1" { std::fs::remove_dir_all(&r.real) } else { std::fs::remove_dir(&r.real) }
            } else {
                std::fs::remove_file(&r.real)
            };
            match result { Ok(_) => text(200, "ok"), Err(e) => io_err(e) }
        }
        ("POST", "rename") | ("POST", "copy") => {
            let from = match res(&get("from")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            let to = match res(&get("to")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            if protected(&to) || (op == "rename" && protected(&from)) { return text(403, "denied"); }
            if !from.real.exists() { return text(404, "source not found"); }
            if to.real.exists() {
                if get("overwrite") != "1" { return text(409, "exists"); }
                if let Err(e) = remove_any(&to.real) { return io_err(e); }
            }
            let result = if op == "rename" { std::fs::rename(&from.real, &to.real) } else { copy_recursive(&from.real, &to.real) };
            match result { Ok(_) => text(200, "ok"), Err(e) => io_err(e) }
        }
        _ => text(404, "unknown operation"),
    }
}
