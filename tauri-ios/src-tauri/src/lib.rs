use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use percent_encoding::percent_decode_str;
use tauri::http::{header, Response, StatusCode};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Finds the bundled `vscode-web` build inside the app's resources.
/// (Resource layout differs between platforms, so a couple of candidates are tried.)
fn web_root(app: &AppHandle) -> Option<PathBuf> {
    let res = app.path().resource_dir().ok()?;
    [res.join("web"), res.join("assets").join("web")]
        .into_iter()
        .find(|p| p.join("out").is_dir())
}

fn respond(status: StatusCode, mime: &str, body: Vec<u8>) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Cow::Owned(body))
        .unwrap()
}

fn serve(app: &AppHandle, uri_path: &str) -> Response<Cow<'static, [u8]>> {
    let Some(root) = web_root(app) else {
        return respond(
            StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain",
            b"Bundled workbench (web/out) not found in app resources".to_vec(),
        );
    };

    let decoded = percent_decode_str(uri_path).decode_utf8_lossy().to_string();
    let rel = decoded.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };

    // Refuse anything that tries to leave the web root.
    if Path::new(rel).components().any(|c| !matches!(c, Component::Normal(_))) {
        return respond(StatusCode::FORBIDDEN, "text/plain", b"Forbidden".to_vec());
    }

    let file = root.join(rel);
    match std::fs::read(&file) {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&file).first_or_octet_stream().to_string();
            respond(StatusCode::OK, &mime, bytes)
        }
        Err(_) => respond(StatusCode::NOT_FOUND, "text/plain", b"Not found".to_vec()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // vscode://localhost/... is served straight from the bundled files; no HTTP server involved.
        .register_uri_scheme_protocol("vscode", |ctx, request| {
            serve(ctx.app_handle(), request.uri().path())
        })
        .setup(|app| {
            WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External("vscode://localhost/".parse().unwrap()),
            )
            .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
