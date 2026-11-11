(function () {
  var lines = [];
  function add(s) { lines.push(s); }
  window.addEventListener('error', function (e) {
    var t = e.target;
    if (t && t !== window && (t.src || t.href)) { add('LOAD FAIL ' + (t.src || t.href)); }
    else { add('ERR ' + e.message + ' @' + (e.filename || '') + ':' + (e.lineno || '')); }
  }, true);
  window.addEventListener('unhandledrejection', function (e) {
    var r = e.reason;
    add('REJECT ' + ((r && (r.stack || r.message)) || r));
  });
  var ce = console.error;
  console.error = function () {
    try { add('console.error ' + [].slice.call(arguments).map(String).join(' ')); } catch (_) {}
    ce.apply(console, arguments);
  };
  // Show the log on screen only if the workbench still has not started after 12s.
  setTimeout(function () {
    if (document.querySelector('.monaco-workbench')) { return; }
    function show(extra) {
      var d = document.createElement('pre');
      d.style.cssText = 'position:fixed;left:0;right:0;bottom:0;max-height:60%;overflow:auto;margin:0;padding:8px;' +
        'font:11px monospace;background:rgba(0,0,0,.9);color:#ffb4b4;z-index:2147483647;white-space:pre-wrap';
      d.textContent = 'secure=' + window.isSecureContext + ' origin=' + location.origin +
        ' subtle=' + !!(window.crypto && window.crypto.subtle) + '\n' + lines.join('\n') + extra;
      document.body.appendChild(d);
    }
    fetch('/__log').then(function (r) { return r.text(); })
      .then(function (t) { show('\n--- server non-200 responses ---\n' + t); })
      .catch(function (err) { show('\n/__log fetch failed: ' + err); });
  }, 12000);

  // ---- Input diagnostics: a small bug button (bottom right) that shows what the keyboard/editor receive.
  var inputLog = [];
  function note(line) { inputLog.push(line); if (inputLog.length > 14) { inputLog.shift(); } }
  function elName(el) {
    if (!el) { return 'none'; }
    return el.tagName + (el.className && typeof el.className === 'string' ? '.' + el.className.split(' ')[0] : '') +
      (el.readOnly ? ' [readonly]' : '') + (el.isContentEditable ? ' [contenteditable]' : '');
  }
  ['keydown', 'beforeinput', 'input', 'compositionstart', 'compositionupdate', 'compositionend'].forEach(function (type) {
    window.addEventListener(type, function (e) {
      // Bubble phase: defaultPrevented tells whether the editor handled the event itself.
      var d = type.indexOf('key') === 0 ? ('key=' + e.key + ' code=' + e.code + ' kc=' + e.keyCode)
        : (e.inputType ? e.inputType + ' ' : '') + (e.data !== undefined && e.data !== null ? JSON.stringify(e.data) : '');
      note(type + ' ' + d + (e.defaultPrevented ? ' (prevented)' : '') + ' @' + elName(e.target));
    }, false);
  });
  window.addEventListener('focusin', function (e) { note('focusin ' + elName(e.target)); }, true);

  function installHud() {
    var btn = document.createElement('div');
    btn.textContent = '\u{1F41E}';
    btn.style.cssText = 'position:fixed;right:6px;bottom:60px;width:32px;height:32px;line-height:32px;text-align:center;' +
      'border-radius:16px;background:rgba(0,0,0,.55);z-index:2147483645;font-size:16px';
    var panel = document.createElement('pre');
    panel.style.cssText = 'display:none;position:fixed;left:8px;right:8px;bottom:100px;max-height:50%;overflow:auto;margin:0;' +
      'padding:8px;font:11px monospace;background:rgba(0,0,0,.88);color:#9ef;z-index:2147483645;white-space:pre-wrap;border-radius:8px';
    function refresh() {
      panel.textContent = 'EditContext in window: ' + (typeof window.EditContext === 'function') +
        '\nsecure=' + window.isSecureContext + ' touch=' + navigator.maxTouchPoints +
        '\nUA: ' + navigator.userAgent +
        '\nactive: ' + elName(document.activeElement) +
        '\n--- last events (tap the bug again to refresh) ---\n' + inputLog.join('\n');
    }
    btn.addEventListener('click', function () {
      var open = panel.style.display === 'none';
      panel.style.display = open ? 'block' : 'none';
      if (open) { refresh(); }
    });
    document.body.append(panel, btn);
  }
  if (document.body) { installHud(); } else { document.addEventListener('DOMContentLoaded', installHud); }
})();
