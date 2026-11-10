# Vscode on iOS (Tauri shell)

Tauri 2 iOS app that bundles the standalone `vscode-web` build as an app resource and serves it
to the WKWebView from a loopback-only HTTP server (127.0.0.1:38621) written in Rust. Build pipeline follows
CrossCode's: `tauri ios init --ci` -> `tauri ios build --no-sign` -> wrap the `.app` as an unsigned `.ipa`.

Build: GitHub Actions -> "iOS Tauri unsigned IPA" -> Run workflow -> download artifact -> import in LiveContainer.

Files: `<Documents>/Linux` and `<Documents>/Documents` are exposed to the workbench as the `ios:` file system (ios-fs-ext + /__fs API in server.rs).

Known limits: web workbench only (no terminal / Node extension host); files live in IndexedDB;
untested end to end. Serial/BLE would be added as a Rust Tauri plugin.
