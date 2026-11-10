//! Loopback-only HTTP server for the app.
//!  * serves the bundled `vscode-web` files (a real http origin is required: custom URI schemes have
//!    an opaque origin in WebKit, which breaks `new URL(..., window.location.origin)`), and
//!  * exposes `/__fs/*`, a small file API over <Documents>/Linux and <Documents>/Documents,
//!    used by the `ios-files` web extension so the workbench can browse those folders.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::UNIX_EPOCH;

use percent_encoding::percent_decode_str;
use serde_json::json;

/// Recent non-200 responses, exposed at /__log to help debug a workbench that fails to load.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Fixed port keeps the page origin stable so IndexedDB / localStorage persist across launches.
pub const PREFERRED_PORT: u16 = 38621;

const MAX_BODY: usize = 512 * 1024 * 1024;
const ROOTS: [&str; 2] = ["Linux", "Documents"];

struct Ctx {
    web_root: Option<PathBuf>,
    fs_root: Option<PathBuf>,
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

pub fn start(web_root: Option<PathBuf>, fs_root: Option<PathBuf>) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))?;
    let port = listener.local_addr()?.port();
    let ctx = Arc::new(Ctx { web_root, fs_root, port });
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
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
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

/// Maps a workbench path like "/Linux/projects/a.txt" onto the real folder. Only the two top-level
/// folders are reachable. Returns the real path and how many path components were given.
fn resolve(fs_root: &Path, p: &str) -> Result<(PathBuf, usize), u16> {
    let mut comps: Vec<std::ffi::OsString> = Vec::new();
    for c in Path::new(p.trim_start_matches('/')).components() {
        match c {
            Component::Normal(s) => comps.push(s.to_owned()),
            Component::CurDir => {}
            _ => return Err(403),
        }
    }
    if let Some(first) = comps.first() {
        if !ROOTS.iter().any(|r| first.to_string_lossy() == *r) {
            return Err(403);
        }
    }
    let real = comps.iter().fold(fs_root.to_path_buf(), |a, c| a.join(c));
    Ok((real, comps.len()))
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

fn api(ctx: &Ctx, req: &Request, op: &str, query: &str) -> Reply {
    // A custom header forces a CORS preflight for cross-site pages (which we never answer), and the
    // Host check blocks DNS-rebinding. Same-origin requests from the workbench pass both.
    let host_ok = req.headers.get("host").map(|h| *h == format!("127.0.0.1:{}", ctx.port)).unwrap_or(false);
    if !host_ok || !req.headers.contains_key("x-vscode-fs") {
        return text(403, "Forbidden");
    }
    let Some(fs_root) = ctx.fs_root.as_deref() else {
        return text(500, "Documents folder unavailable");
    };
    let q = parse_query(query);
    let get = |k: &str| q.get(k).cloned().unwrap_or_default();

    let res = |p: &str| resolve(fs_root, p);
    let io_err = |e: std::io::Error| text(status_of(&e), &e.to_string());

    match (req.method.as_str(), op) {
        ("GET", "stat") => {
            let (real, n) = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            if n == 0 {
                return reply(200, "application/json", json!({"type": 2, "size": 0, "ctime": 0, "mtime": 0}).to_string().into_bytes());
            }
            match std::fs::metadata(&real) {
                Ok(m) => reply(200, "application/json", json!({
                    "type": if m.is_dir() { 2 } else { 1 },
                    "size": m.len(),
                    "ctime": ms(m.created()),
                    "mtime": ms(m.modified()),
                }).to_string().into_bytes()),
                Err(e) => io_err(e),
            }
        }
        ("GET", "list") => {
            let (real, n) = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            if n == 0 {
                let names: Vec<_> = ROOTS.iter().map(|r| json!([r, 2])).collect();
                return reply(200, "application/json", json!(names).to_string().into_bytes());
            }
            match std::fs::read_dir(&real) {
                Ok(rd) => {
                    let mut out = Vec::new();
                    for e in rd.flatten() {
                        let t = if e.path().is_dir() { 2 } else { 1 };
                        out.push(json!([e.file_name().to_string_lossy(), t]));
                    }
                    reply(200, "application/json", json!(out).to_string().into_bytes())
                }
                Err(e) => io_err(e),
            }
        }
        ("GET", "read") => {
            let (real, n) = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            if n == 0 { return text(403, "denied"); }
            match std::fs::read(&real) {
                Ok(b) => reply(200, "application/octet-stream", b),
                Err(e) => io_err(e),
            }
        }
        ("PUT", "write") => {
            let (real, n) = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            if n < 2 { return text(403, "denied"); }
            match std::fs::write(&real, &req.body) {
                Ok(_) => text(200, "ok"),
                Err(e) => io_err(e),
            }
        }
        ("POST", "mkdir") => {
            let (real, n) = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            if n < 2 { return text(403, "denied"); }
            match std::fs::create_dir(&real) {
                Ok(_) => text(200, "ok"),
                Err(e) => io_err(e),
            }
        }
        ("DELETE", "delete") => {
            let (real, n) = match res(&get("p")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            if n < 2 { return text(403, "denied"); } // never delete Linux / Documents themselves
            let result = if real.is_dir() {
                if get("recursive") == "1" { std::fs::remove_dir_all(&real) } else { std::fs::remove_dir(&real) }
            } else {
                std::fs::remove_file(&real)
            };
            match result { Ok(_) => text(200, "ok"), Err(e) => io_err(e) }
        }
        ("POST", "rename") | ("POST", "copy") => {
            let (from, nf) = match res(&get("from")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            let (to, nt) = match res(&get("to")) { Ok(v) => v, Err(s) => return text(s, "denied") };
            if nf < 2 || nt < 2 { return text(403, "denied"); }
            if !from.exists() { return text(404, "source not found"); }
            if to.exists() {
                if get("overwrite") != "1" { return text(409, "exists"); }
                if let Err(e) = remove_any(&to) { return io_err(e); }
            }
            let result = if op == "rename" { std::fs::rename(&from, &to) } else { copy_recursive(&from, &to) };
            match result { Ok(_) => text(200, "ok"), Err(e) => io_err(e) }
        }
        _ => text(404, "unknown operation"),
    }
}
