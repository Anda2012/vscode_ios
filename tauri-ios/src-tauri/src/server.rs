//! Tiny loopback-only HTTP server for the bundled `vscode-web` files.
//! A real http://127.0.0.1:<port> origin is needed: custom URI schemes have an opaque
//! origin ("null") in WebKit, which breaks `new URL(..., window.location.origin)` in workbench.html.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::thread;

use percent_encoding::percent_decode_str;

/// Fixed port keeps the page origin stable so IndexedDB / localStorage persist across launches.
pub const PREFERRED_PORT: u16 = 38621;

pub fn start(root: Option<PathBuf>) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))?;
    let port = listener.local_addr()?.port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let root = root.clone();
            thread::spawn(move || handle(stream, root.as_deref()));
        }
    });
    Ok(port)
}

fn handle(mut stream: TcpStream, root: Option<&Path>) {
    let mut buf = [0u8; 8192];
    let n = match stream.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let request = String::from_utf8_lossy(&buf[..n]);
    let mut parts = request.lines().next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");

    let (status, mime, body) = if method != "GET" && method != "HEAD" {
        (405, "text/plain".to_string(), b"Method Not Allowed".to_vec())
    } else {
        serve(root, target.split('?').next().unwrap_or("/"))
    };

    let reason = match status {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    if method != "HEAD" {
        let _ = stream.write_all(&body);
    }
}

fn serve(root: Option<&Path>, uri_path: &str) -> (u16, String, Vec<u8>) {
    let Some(root) = root else {
        return (
            500,
            "text/plain".into(),
            b"Bundled workbench (web/out) not found in app resources".to_vec(),
        );
    };

    let decoded = percent_decode_str(uri_path).decode_utf8_lossy().to_string();
    let rel = decoded.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };

    // Refuse anything that tries to leave the web root.
    if Path::new(rel).components().any(|c| !matches!(c, Component::Normal(_))) {
        return (403, "text/plain".into(), b"Forbidden".to_vec());
    }

    let file = root.join(rel);
    match std::fs::read(&file) {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&file).first_or_octet_stream().to_string();
            (200, mime, bytes)
        }
        Err(_) => (404, "text/plain".into(), b"Not found".to_vec()),
    }
}
