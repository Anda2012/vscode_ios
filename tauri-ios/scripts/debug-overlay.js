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
})();
