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

exports.activate = function (context) {
  const e = context.extensionUri; // http://127.0.0.1:<port>/ios-fs-ext
  const base = e.scheme + '://' + e.authority + '/__fs';
  context.subscriptions.push(
    vscode.workspace.registerFileSystemProvider('ios', new IosFileSystem(base), { isCaseSensitive: true })
  );
};
exports.deactivate = function () { };
