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

/// Creates the two user-visible folders in the app's Documents directory. With file sharing
/// enabled in Info.ios.plist they show up in the Files app under "On My iPad/iPhone" -> Vscode.
fn prepare_user_folders(app: &AppHandle) -> Option<PathBuf> {
    let docs = app.path().document_dir().ok()?;
    for name in ["Linux", "Documents"] {
        let _ = std::fs::create_dir_all(docs.join(name));
    }
    Some(docs)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let docs = prepare_user_folders(app.handle());
            let port = server::start(web_root(app.handle()), docs)?;
            let url = format!("http://127.0.0.1:{port}/");
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse().unwrap()))
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
