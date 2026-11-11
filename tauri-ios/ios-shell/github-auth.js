// GitHub sign-in for the embedded workbench, using GitHub's OAuth *device flow*.
// The built-in github-authentication extension has no flow that works in a web extension host inside
// a custom embedder, so the app supplies its own `github` authentication provider instead.
// The github.com/login/* endpoints don't allow CORS, so those two calls go through the app's local
// server (/__fs/gh-device, /__fs/gh-token), which forwards them over HTTPS.

// VS Code's public GitHub OAuth app (device flow enabled). To use your own, create an OAuth App on
// GitHub (Settings > Developer settings), tick "Enable Device Flow", and paste its client id here.
const CLIENT_ID = '01ab8ac9400c4e429b23';
const STORE = 'ios.gh.sessions';
const HEADERS = { 'X-Vscode-Fs': '1' };

function load() { try { return JSON.parse(localStorage.getItem(STORE) || '[]'); } catch (e) { return []; } }
function save(sessions) { try { localStorage.setItem(STORE, JSON.stringify(sessions)); } catch (e) { /* ignore */ } }
function sleep(ms) { return new Promise(function (r) { setTimeout(r, ms); }); }

async function proxy(op, params) {
  const response = await fetch('/__fs/' + op, {
    method: 'POST',
    headers: Object.assign({ 'Content-Type': 'application/x-www-form-urlencoded' }, HEADERS),
    body: new URLSearchParams(params).toString()
  });
  if (!response.ok) { throw new Error('GitHub request failed (' + response.status + ')'); }
  return response.json();
}

function button(label, handler) {
  const b = document.createElement('button');
  b.textContent = label;
  b.style.cssText = 'margin:6px;padding:10px 16px;border:0;border-radius:8px;background:#0e639c;color:#fff;font:inherit';
  b.onclick = handler;
  return b;
}

function showCode(code, uri, onCancel) {
  const overlay = document.createElement('div');
  overlay.style.cssText = 'position:fixed;inset:0;z-index:2147483646;background:rgba(0,0,0,.6);display:flex;' +
    'align-items:center;justify-content:center;font:16px -apple-system,system-ui,sans-serif';
  const box = document.createElement('div');
  box.style.cssText = 'background:#252526;color:#ddd;padding:24px;border-radius:12px;max-width:340px;text-align:center';
  const title = document.createElement('div');
  title.textContent = 'Sign in to GitHub';
  title.style.cssText = 'font-size:20px;margin-bottom:12px';
  const help = document.createElement('div');
  help.textContent = 'Open the GitHub page, enter this code, and approve. This window closes by itself.';
  const codeEl = document.createElement('div');
  codeEl.textContent = code;
  codeEl.style.cssText = 'font:700 30px ui-monospace,Menlo,monospace;letter-spacing:3px;margin:16px 0;user-select:all';
  box.append(title, help, codeEl,
    button('Copy code', function () { try { navigator.clipboard.writeText(code); } catch (e) { /* ignore */ } }),
    button('Open GitHub', function () { window.open(uri); }),
    button('Cancel', function () { onCancel(); }));
  overlay.append(box);
  document.body.append(overlay);
  return { close: function () { overlay.remove(); } };
}

export function createGitHubProvider(m) {
  const emitter = new m.Emitter();
  return {
    id: 'github',
    label: 'GitHub',
    supportsMultipleAccounts: false,
    onDidChangeSessions: emitter.event,

    async getSessions(scopes) {
      const all = load();
      if (!scopes || !scopes.length) { return all; }
      return all.filter(function (s) { return scopes.every(function (x) { return s.scopes.indexOf(x) !== -1; }); });
    },

    async createSession(scopes) {
      const dc = await proxy('gh-device', { client_id: CLIENT_ID, scope: scopes.join(' ') });
      if (!dc.device_code) { throw new Error(dc.error_description || 'Could not start GitHub sign-in'); }
      let cancelled = false;
      const ui = showCode(dc.user_code, dc.verification_uri || 'https://github.com/login/device', function () { cancelled = true; });
      try {
        let interval = (dc.interval || 5) * 1000;
        const deadline = Date.now() + (dc.expires_in || 900) * 1000;
        let token;
        while (!cancelled && Date.now() < deadline) {
          await sleep(interval);
          const t = await proxy('gh-token', {
            client_id: CLIENT_ID,
            device_code: dc.device_code,
            grant_type: 'urn:ietf:params:oauth:grant-type:device_code'
          });
          if (t.access_token) { token = t.access_token; break; }
          if (t.error === 'authorization_pending') { continue; }
          if (t.error === 'slow_down') { interval += 5000; continue; }
          throw new Error(t.error_description || t.error || 'Sign-in failed');
        }
        if (!token) { throw new Error(cancelled ? 'Sign-in cancelled' : 'Sign-in timed out'); }

        const res = await fetch('https://api.github.com/user', {
          headers: { Authorization: 'Bearer ' + token, Accept: 'application/vnd.github+json' }
        });
        if (!res.ok) { throw new Error('Could not read your GitHub profile (' + res.status + ')'); }
        const user = await res.json();
        const session = {
          id: (window.crypto && crypto.randomUUID) ? crypto.randomUUID() : String(Date.now()),
          accessToken: token,
          account: { label: user.login, id: String(user.id) },
          scopes: scopes.slice()
        };
        save(load().concat([session]));
        emitter.fire({ added: [session], removed: [], changed: [] });
        return session;
      } finally {
        ui.close();
      }
    },

    async removeSession(id) {
      const all = load();
      const removed = all.filter(function (s) { return s.id === id; });
      save(all.filter(function (s) { return s.id !== id; }));
      if (removed.length) { emitter.fire({ added: [], removed: removed, changed: [] }); }
    }
  };
}
