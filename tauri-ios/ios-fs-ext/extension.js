// Web extension: a FileSystemProvider for the `ios:` scheme, backed by the app's loopback
// server (/__fs/*), which maps onto <Documents>/Linux and <Documents>/Documents.
const vscode = require('vscode');

const FILE = 1, DIRECTORY = 2;

function toError(status, uri, text) {
  if (status === 404) { return vscode.FileSystemError.FileNotFound(uri); }
  if (status === 409) { return vscode.FileSystemError.FileExists(uri); }
  if (status === 403) { return vscode.FileSystemError.NoPermissions(uri); }
  return vscode.FileSystemError.Unavailable(text || ('HTTP ' + status));
}

class IosFileSystem {
  constructor(base) {
    this.base = base; // e.g. http://127.0.0.1:38621/__fs
    this._emitter = new vscode.EventEmitter();
    this.onDidChangeFile = this._emitter.event;
  }

  watch() { return new vscode.Disposable(function () { }); }

  async _call(op, params, init, uri) {
    const query = Object.keys(params).map(function (k) { return k + '=' + encodeURIComponent(params[k]); }).join('&');
    const options = Object.assign({}, init || {});
    options.headers = Object.assign({ 'X-Vscode-Fs': '1' }, options.headers || {});
    let response;
    try {
      response = await fetch(this.base + '/' + op + '?' + query, options);
    } catch (e) {
      throw vscode.FileSystemError.Unavailable(String(e));
    }
    if (!response.ok) { throw toError(response.status, uri, await response.text().catch(function () { return ''; })); }
    return response;
  }

  async stat(uri) {
    const j = await (await this._call('stat', { p: uri.path }, null, uri)).json();
    return { type: j.type, ctime: j.ctime, mtime: j.mtime, size: j.size };
  }

  async readDirectory(uri) {
    return (await (await this._call('list', { p: uri.path }, null, uri)).json());
  }

  async readFile(uri) {
    return new Uint8Array(await (await this._call('read', { p: uri.path }, null, uri)).arrayBuffer());
  }

  async writeFile(uri, content, opts) {
    let exists = true;
    try { await this.stat(uri); } catch (e) { exists = false; }
    if (!exists && !opts.create) { throw vscode.FileSystemError.FileNotFound(uri); }
    if (exists && opts.create && !opts.overwrite) { throw vscode.FileSystemError.FileExists(uri); }
    await this._call('write', { p: uri.path }, { method: 'PUT', body: content }, uri);
    this._emitter.fire([{ type: exists ? vscode.FileChangeType.Changed : vscode.FileChangeType.Created, uri: uri }]);
  }

  async createDirectory(uri) {
    await this._call('mkdir', { p: uri.path }, { method: 'POST' }, uri);
    this._emitter.fire([{ type: vscode.FileChangeType.Created, uri: uri }]);
  }

  async delete(uri, opts) {
    await this._call('delete', { p: uri.path, recursive: opts && opts.recursive ? '1' : '0' }, { method: 'DELETE' }, uri);
    this._emitter.fire([{ type: vscode.FileChangeType.Deleted, uri: uri }]);
  }

  async rename(from, to, opts) {
    await this._call('rename', { from: from.path, to: to.path, overwrite: opts && opts.overwrite ? '1' : '0' }, { method: 'POST' }, from);
    this._emitter.fire([{ type: vscode.FileChangeType.Deleted, uri: from }, { type: vscode.FileChangeType.Created, uri: to }]);
  }

  async copy(from, to, opts) {
    await this._call('copy', { from: from.path, to: to.path, overwrite: opts && opts.overwrite ? '1' : '0' }, { method: 'POST' }, from);
    this._emitter.fire([{ type: vscode.FileChangeType.Created, uri: to }]);
  }
}

// ---- Ubuntu terminal: a Pseudoterminal that talks to the app's /__fs/term-* endpoints ----------

class LinuxPty {
  constructor(base) {
    this.base = base;
    this._write = new vscode.EventEmitter();
    this.onDidWrite = this._write.event;
    this._close = new vscode.EventEmitter();
    this.onDidClose = this._close.event;
    this.id = 0;
    this.closed = false;
    this.sendQueue = Promise.resolve();
    this.decoder = new TextDecoder();
    this.dims = { columns: 80, rows: 24 };
  }

  _fetch(op, params, init) {
    const query = Object.keys(params).map(function (k) { return k + '=' + encodeURIComponent(params[k]); }).join('&');
    const options = Object.assign({}, init || {});
    options.headers = Object.assign({ 'X-Vscode-Fs': '1' }, options.headers || {});
    return fetch(this.base + '/' + op + '?' + query, options);
  }

  async open(initial) {
    if (initial) { this.dims = initial; }
    try {
      let lastMessage = '';
      for (;;) {
        const s = await (await this._fetch('term-status', {})).json();
        if (s.state === 'ready') { break; }
        if (s.state === 'no-rootfs' || s.state === 'error') {
          this._write.fire('\r\n' + s.message + '\r\n');
          this._close.fire(1);
          return;
        }
        if (s.message && s.message !== lastMessage) { this._write.fire(s.message + '\r\n'); lastMessage = s.message; }
        await new Promise(function (r) { setTimeout(r, 1000); });
        if (this.closed) { return; }
      }
      const r = await this._fetch('term-open', { cols: this.dims.columns, rows: this.dims.rows }, { method: 'POST' });
      if (!r.ok) { this._write.fire('\r\n' + (await r.text()) + '\r\n'); this._close.fire(1); return; }
      this.id = (await r.json()).id;
      this._readLoop();
    } catch (e) {
      this._write.fire('\r\nCould not reach the terminal service: ' + e + '\r\n');
      this._close.fire(1);
    }
  }

  async _readLoop() {
    while (!this.closed) {
      let r;
      try { r = await this._fetch('term-read', { id: this.id }); } catch (e) { await new Promise(function (x) { setTimeout(x, 500); }); continue; }
      if (r.status === 204) { continue; }
      if (r.status === 410) { this.closed = true; this._write.fire('\r\n[session ended]\r\n'); this._close.fire(0); return; }
      if (!r.ok) { this.closed = true; this._close.fire(1); return; }
      this._write.fire(this.decoder.decode(new Uint8Array(await r.arrayBuffer()), { stream: true }));
    }
  }

  handleInput(data) {
    if (!this.id) { return; }
    const bytes = new TextEncoder().encode(data);
    // Keep keystrokes in order.
    this.sendQueue = this.sendQueue.then(() => this._fetch('term-write', { id: this.id }, { method: 'POST', body: bytes })).catch(function () { });
  }

  setDimensions(dims) {
    this.dims = dims;
    if (this.id) { this._fetch('term-resize', { id: this.id, cols: dims.columns, rows: dims.rows }, { method: 'POST' }).catch(function () { }); }
  }

  close() {
    this.closed = true;
    if (this.id) { this._fetch('term-close', { id: this.id }, { method: 'POST' }).catch(function () { }); }
  }
}

async function linuxStatus(base) {
  const r = await fetch(base + '/term-status', { headers: { 'X-Vscode-Fs': '1' } });
  return r.json(); // { state, message }
}

exports.activate = function (context) {
  const e = context.extensionUri; // http://127.0.0.1:<port>/ios-fs-ext
  const base = e.scheme + '://' + e.authority + '/__fs';

  // Status bar buttons: the only way to reach these on an iPad without a keyboard.
  const terminalItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 1000);
  terminalItem.text = '$(terminal) Ubuntu';
  terminalItem.tooltip = 'Open an Ubuntu terminal';
  terminalItem.command = 'ios-linux.newTerminal';
  terminalItem.show();

  const signInItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 999);
  signInItem.command = 'ios-linux.signIn';
  const refreshSignIn = async function () {
    try {
      const s = await vscode.authentication.getSession('github', [], { silent: true });
      signInItem.text = s ? '$(github) ' + s.account.label : '$(github) Sign in';
      signInItem.tooltip = s ? 'Signed in to GitHub' : 'Sign in to GitHub';
    } catch (err) {
      signInItem.text = '$(github) Sign in';
    }
    signInItem.show();
  };
  refreshSignIn();
  context.subscriptions.push(terminalItem, signInItem,
    vscode.authentication.onDidChangeSessions(function (ev) { if (ev.provider.id === 'github') { refreshSignIn(); } }),
    vscode.commands.registerCommand('ios-linux.signIn', async function () {
      try {
        const s = await vscode.authentication.getSession('github', ['read:user', 'user:email'], { createIfNone: true });
        vscode.window.showInformationMessage('Signed in to GitHub as ' + s.account.label);
      } catch (err) {
        vscode.window.showErrorMessage('GitHub sign-in failed: ' + (err && err.message ? err.message : err));
      }
      refreshSignIn();
    }),
    vscode.commands.registerCommand('ios-linux.status', async function () {
      try {
        const s = await linuxStatus(base);
        vscode.window.showInformationMessage('Linux: ' + s.state + (s.message ? ' - ' + s.message : ''));
      } catch (err) {
        vscode.window.showErrorMessage('Linux status unavailable: ' + err);
      }
    }));
  context.subscriptions.push(
    vscode.workspace.registerFileSystemProvider('ios', new IosFileSystem(base), { isCaseSensitive: true }),
    vscode.window.registerTerminalProfileProvider('ios-linux.ubuntu', {
      provideTerminalProfile: function () { return new vscode.TerminalProfile({ name: 'Ubuntu', pty: new LinuxPty(base) }); }
    }),
    vscode.commands.registerCommand('ios-linux.newTerminal', function () {
      vscode.window.createTerminal({ name: 'Ubuntu', pty: new LinuxPty(base) }).show();
    })
  );
};
exports.deactivate = function () { };
