// Pre-renders workbench.html into web/index.html, doing what
// src/vs/server/node/webClientServer.ts does at request time.
import { readFileSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const webDir = process.argv[2];
const template = readFileSync(join(webDir, 'out/vs/code/browser/workbench/workbench.html'), 'utf8');

const config = {
  enableWorkspaceTrust: true,
  productConfiguration: { embedderIdentifier: 'ios-tauri-shell' },
};

const values = {
  WORKBENCH_WEB_CONFIGURATION: JSON.stringify(config).replace(/"/g, '&quot;'),
  WORKBENCH_AUTH_SESSION: '',
  WORKBENCH_WEB_BASE_URL: '',
  WORKBENCH_NLS_URL: '',
  WORKBENCH_NLS_FALLBACK_URL: '/out/nls.messages.js',
  WORKBENCH_SCRIPT_NONCE: randomUUID(),
};

let html = template;
for (const [k, v] of Object.entries(values)) html = html.replaceAll(`{{${k}}}`, v);
// Debug overlay: collects page errors and shows them on screen if the workbench never starts.
const overlay = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'debug-overlay.js'), 'utf8');
html = html.replace('<head>', () => `<head>\n<script>${overlay}</script>`);
writeFileSync(join(webDir, 'index.html'), html);
console.log('Wrote', join(webDir, 'index.html'));
