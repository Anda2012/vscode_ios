#!/usr/bin/env bash
# Builds the standalone web workbench and stages it in src-tauri/web (bundled as an app resource).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEST="$ROOT/tauri-ios/src-tauri/web"
cd "$ROOT"

corepack enable || true
# Retry: native/postinstall downloads (ripgrep, sqlite3 prebuilds) occasionally fail on shared runners.
for attempt in 1 2 3; do
  pnpm install --frozen-lockfile && break
  [ "$attempt" = 3 ] && { echo "pnpm install failed 3 times"; exit 1; }
  echo "pnpm install failed (attempt $attempt), retrying in 30s..."; sleep 30
done
pnpm run gulp vscode-web-min        # output lands in ../vscode-web (build/gulpfile.vscode.web.ts)

rm -rf "$DEST"
cp -R "$ROOT/../vscode-web" "$DEST"
cp -R "$ROOT/tauri-ios/ios-fs-ext" "$DEST/ios-fs-ext"
cp -R "$ROOT/tauri-ios/ios-shell" "$DEST/ios-shell"
node "$ROOT/tauri-ios/scripts/render-index.mjs" "$DEST"
echo "Staged $(du -sh "$DEST" | cut -f1) into $DEST"
