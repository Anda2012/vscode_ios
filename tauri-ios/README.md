# VS Code on iOS (Tauri shell)

Tauri 2 iOS app that bundles the standalone `vscode-web` build as an app resource and serves it
to the WKWebView through a custom `vscode://` URI scheme (no HTTP server). Build pipeline follows
CrossCode's: `tauri ios init --ci` -> `tauri ios build --no-sign` -> wrap the `.app` as an unsigned `.ipa`.

Build: GitHub Actions -> "iOS Tauri unsigned IPA" -> Run workflow -> download artifact -> import in LiveContainer.

Known limits: web workbench only (no terminal / Node extension host); files live in IndexedDB;
untested end to end. Serial/BLE would be added as a Rust Tauri plugin.
