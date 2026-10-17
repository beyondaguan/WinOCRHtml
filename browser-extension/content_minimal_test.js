// 极简测试：content script 注入是否正常
(function() {
  'use strict';
  console.log('[WINOCR TEST] content script loaded at', location.href);
  window.__winocrTestLoaded = true;
  var d = document.createElement('div');
  d.id = "winocr-test-marker";
  d.style.cssText = 'position:fixed;top:8px;left:8px;padding:8px 14px;background:#10b981;color:#fff;font:14px sans-serif;z-index:2147483647;border-radius:6px;';
  d.textContent = 'WinOCR extension loaded v0.1.0';
  document.documentElement.appendChild(d);
})();
