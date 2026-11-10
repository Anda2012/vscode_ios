mod server;

use std::path::PathBuf;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Finds the bundled `vscode-web` build inside the app's resources.
/// (Resource layout differs between platforms, so a couple of candidates are tried.)
fn web_root(app: &AppHandle) -> Option<PathBuf> {
    let res = app.path().resource_dir().ok()?;
    [res.join("web"), res.join("assets").join("web")]
        .into_iter()
        .find(|p| p.join("out").is_dir())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let port = server::start(web_root(app.handle()))?;
            let url = format!("http://127.0.0.1:{port}/");
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse().unwrap()))
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
