(async function () {
  // The workbench opens external links with window.open(), which WKWebView blocks. Hand them to the
  // app instead, which opens them in Safari.
  window.open = function (url) {
    if (url && /^https:/.test(String(url))) {
      fetch('/__fs/open', { method: 'POST', headers: { 'X-Vscode-Fs': '1' }, body: String(url) }).catch(function () { });
    }
    return null;
  };

  try {
    const [m, auth] = await Promise.all([
      import('/out/vs/workbench/workbench.web.main.internal.js'),
      import('/ios-shell/github-auth.js')
    ]);
    const cfg = __CONFIG__;

    // ios-fs-ext (served by the app) provides the `ios:` file system: everything in the app's folder.
    cfg.additionalBuiltinExtensions = [{ scheme: location.protocol.replace(':', ''), authority: location.host, path: '/ios-fs-ext' }];
    cfg.authenticationProviders = [auth.createGitHubProvider(m)];
    cfg.configurationDefaults = {
      // Newer WebKit may expose EditContext without handling it fully; the classic textarea input always works.
      'editor.editContext': false,
      // The Ubuntu terminal is provided by ios-fs-ext (works whichever OS the web view reports).
      'terminal.integrated.defaultProfile.linux': 'Ubuntu',
      'terminal.integrated.defaultProfile.osx': 'Ubuntu',
      'terminal.integrated.defaultProfile.windows': 'Ubuntu'
    };

    // "Open Folder" works by remembering the choice and reloading (there is only one window).
    var saved = null;
    try { saved = JSON.parse(localStorage.getItem('ios.workspace')); } catch (e) { /* ignore */ }
    saved = saved || { folder: 'ios:/' };
    cfg.workspaceProvider = {
      workspace: saved.workspace ? { workspaceUri: m.URI.parse(saved.workspace) } : { folderUri: m.URI.parse(saved.folder || 'ios:/') },
      trusted: true,
      open: function (w) {
        var next = w && w.workspaceUri ? { workspace: w.workspaceUri.toString() }
          : { folder: w && w.folderUri ? w.folderUri.toString() : 'ios:/' };
        try { localStorage.setItem('ios.workspace', JSON.stringify(next)); } catch (e) { return Promise.resolve(false); }
        setTimeout(function () { location.reload(); }, 100);
        return Promise.resolve(true);
      }
    };

    m.create(document.body, cfg);
    var f = document.getElementById('vscode-startup-fallback'); if (f) { f.remove(); }
  } catch (e) {
    console.error('BOOT FAILED ' + ((e && (e.stack || e.message)) || e));
  }
})();
