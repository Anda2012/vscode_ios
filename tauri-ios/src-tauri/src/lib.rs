mod linux;
mod server;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

/// Finds the bundled `vscode-web` build inside the app's resources.
/// (Resource layout differs between platforms, so a couple of candidates are tried.)
fn web_root(app: &AppHandle) -> Option<PathBuf> {
    let res = app.path().resource_dir().ok()?;
    [res.join("web"), res.join("assets").join("web")]
        .into_iter()
        .find(|p| p.join("out").is_dir())
}

/// The app's Documents directory is what the Files app shows (file sharing is enabled in
/// Info.ios.plist) under "On My iPad/iPhone" -> VScode, and it is the workbench's file system root.
/// `Linux` is created inside it; it will become the Linux environment's main drive.
fn prepare_user_folders(app: &AppHandle) -> Option<PathBuf> {
    let docs = app.path().document_dir().ok()?;
    let _ = std::fs::create_dir_all(docs.join("Linux"));
    Some(docs)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let docs = prepare_user_folders(app.handle());
            let handle = app.handle().clone();
            let open_url: server::OpenUrl = Arc::new(move |url: String| {
                let _ = handle.opener().open_url(url, None::<&str>);
            });
            let term: Arc<dyn server::Terminal> = Arc::new(linux::Linux);
            if let Some(docs) = docs.clone() {
                let res = app.path().resource_dir().ok();
                let rootfs_zip = res.as_ref().and_then(|r| {
                    [r.join("linux").join("ubuntu-fakefs.zip"), r.join("assets").join("linux").join("ubuntu-fakefs.zip")]
                        .into_iter()
                        .find(|p| p.is_file())
                });
                let support = app.path().app_local_data_dir().unwrap_or_else(|_| docs.join(".support"));
                let _ = std::fs::create_dir_all(&support);
                // AF_UNIX paths are short-limited: use /var/... spelling of the temp dir (see LinuxRunner).
                let tmp = std::env::temp_dir();
                let tmp = tmp.to_string_lossy().replacen("/private/var/", "/var/", 1);
                linux::init(linux::Paths { support_dir: support, documents_dir: docs, tmp_dir: PathBuf::from(tmp), rootfs_zip });
            }
            let port = server::start(web_root(app.handle()), docs, Some(open_url), Some(term))?;
            let url = format!("http://127.0.0.1:{port}/");
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse().unwrap()))
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
