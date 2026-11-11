#!/usr/bin/env bash
# Builds the standalone web workbench and stages it in src-tauri/web (bundled as an app resource).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEST="$ROOT/tauri-ios/src-tauri/web"
cd "$ROOT"

corepack enable || true
pnpm install --frozen-lockfile
pnpm run gulp vscode-web-min        # output lands in ../vscode-web (build/gulpfile.vscode.web.ts)

rm -rf "$DEST"
cp -R "$ROOT/../vscode-web" "$DEST"
cp -R "$ROOT/tauri-ios/ios-fs-ext" "$DEST/ios-fs-ext"
cp -R "$ROOT/tauri-ios/ios-shell" "$DEST/ios-shell"
node "$ROOT/tauri-ios/scripts/render-index.mjs" "$DEST"
echo "Staged $(du -sh "$DEST" | cut -f1) into $DEST"
