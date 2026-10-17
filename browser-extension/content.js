// WinOCR 划词助手 - 自诊断版
// 功能：页面加载时显示绿色标记；划词时气泡弹出；所有步骤都有 console.log
(function () {
  'use strict';

  // ---------- 第 1 步：注入可见标记（页面加载后立即可见） ----------
  function showMarker() {
    if (document.getElementById('winocr-load-marker')) return;
    var m = document.createElement('div');
    m.id = 'winocr-load-marker';
    m.style.cssText = [
      'position:fixed;top:4px;right:4px;padding:3px 10px;',
      'background:#16a34a;color:#fff;font-size:11px;font-family:sans-serif;',
      'border-radius:4px;z-index:2147483647;pointer-events:none;'
    ].join('');
    m.textContent = 'WinOCR 扩展已加载';
    (document.body || document.documentElement).appendChild(m);
  }

  // DOMReady 时立即显示标记
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', showMarker);
  } else {
    showMarker();
  }

  // ---------- 扩展上下文检查 ----------
  // 注意：content script 注入后，扩展可能被重新加载/更新，导致本脚本上下文失效。
  // 此时 chrome.runtime.* 调用会抛 "Extension context invalidated"。
  // 统一用 ctxAliveNow() 判断，所有 chrome API 调用前先检查。
  function ctxAlive() {
    try { return !!(typeof chrome !== 'undefined' && chrome.runtime && chrome.runtime.id); }
    catch (e) { return false; }
  }
  var ctxAliveNow = ctxAlive;

  if (!ctxAlive()) {
    console.error('[WINOCR] 扩展上下文无效，content script 停止');
    return;
  }

  if (window.__winocrContentLoaded) {
    console.warn('[WINOCR] 重复注入，跳过');
    return;
  }
  window.__winocrContentLoaded = true;
  console.log('[WINOCR] content script 初始化开始, 扩展ID=' + chrome.runtime.id);

  // ---------- 状态 ----------
  var connected = false;
  var bubbleEl = null;
  var hideTimer = null;
  var settingsLoaded = false; // chrome.storage.sync 设置是否已加载完成
  var bubbleCreatedAt = 0;    // 气泡创建时间戳，防止刚创建就被 mousedown 误关
  var bubblePos = null;       // 会话内记忆的气泡位置（用户拖动后，新气泡出现在同一处）
  var draggingBubble = false; // 拖动中：抑制 mouseup 触发的划词检测
  var settings = { autoBubble: true, bubbleTimeout: 0, srcLang: 'auto', tgtLang: 'zh' };
  var currentSelection = { text: '', srcLang: 'auto', tgtLang: 'zh' };
  var currentUtterance = null; // 当前朗读对象，用于停止

  // ---------- 样式 ----------
  function injectStyles() {
    if (document.getElementById('winocr-styles')) return;
    var style = document.createElement('style');
    style.id = 'winocr-styles';
    style.textContent = '.winocr-bubble{position:fixed;z-index:2147483647;max-width:560px;background:#fff;border:1px solid #e2e8f0;border-radius:12px;box-shadow:0 8px 24px rgba(30,41,59,.18);font-family:"Segoe UI","Microsoft YaHei",system-ui,sans-serif;font-size:16px;line-height:1.6;color:#1e293b;overflow:hidden}.winocr-bubble .wb-head{display:flex;align-items:center;justify-content:space-between;padding:10px 16px;background:#f8fafc;border-bottom:1px solid #e2e8f0;font-size:14px;color:#64748b;cursor:move;user-select:none;-webkit-user-select:none}.winocr-bubble .wb-dot{display:inline-block;width:10px;height:10px;border-radius:50%;background:#10b981}.winocr-bubble .wb-dot.off{background:#ef4444}.winocr-bubble .wb-body{padding:14px 18px;max-height:56vh;overflow-y:auto;overscroll-behavior:contain;scrollbar-width:thin;scrollbar-color:#cbd5e1 transparent}.winocr-bubble .wb-body::-webkit-scrollbar{width:6px}.winocr-bubble .wb-body::-webkit-scrollbar-thumb{background:#cbd5e1;border-radius:3px}.winocr-bubble .wb-body::-webkit-scrollbar-thumb:hover{background:#94a3b8}.winocr-bubble .wb-body::-webkit-scrollbar-track{background:transparent}.winocr-bubble .wb-src{font-weight:600;color:#1e293b;font-size:17px;line-height:1.6;margin-bottom:8px;word-break:break-word}.winocr-bubble .wb-tgt{color:#475569;font-size:16px;line-height:1.7;word-break:break-word;min-height:24px}.winocr-bubble .wb-actions{display:flex;gap:8px;padding:10px 16px;border-top:1px solid #f1f5f9;flex-wrap:wrap}.winocr-bubble .wb-actions button{border:1px solid #e2e8f0;background:#fff;color:#475569;border-radius:8px;padding:8px 14px;font-size:15px;cursor:pointer}.winocr-bubble .wb-actions button:hover{background:#f1f5f9}.winocr-bubble .wb-actions button.primary{background:#6366f0;border-color:#6366f0;color:#fff}.winocr-bubble .wb-actions button.primary:hover{background:#4f46e5}.winocr-bubble .wb-actions button.copied{background:#10b981;border-color:#10b981;color:#fff}.winocr-bubble .wb-actions button:disabled{opacity:.6;cursor:wait}.winocr-bubble .wb-actions button.tts{width:38px;padding:7px 0;font-weight:700;text-align:center}.winocr-bubble .wb-actions button.tts.speaking{background:#6366f0;border-color:#6366f0;color:#fff}.winocr-bubble .wb-actions button.tts-stop{width:32px;padding:6px 0;text-align:center;font-size:13px}.winocr-bubble .wb-engine{padding:6px 16px;background:#eef2ff;color:#4338ca;border-bottom:1px solid #e0e7ff;font-size:12px;line-height:1.5;word-break:break-word}.winocr-bubble .wb-collect-status{margin-top:8px;padding:8px 12px;background:#d1fae5;color:#065f46;border-radius:6px;font-size:14px;font-weight:600;transition:opacity .4s}.winocr-bubble .wb-actions button.capture{width:38px;padding:7px 0;font-size:16px;text-align:center;cursor:pointer}';
    (document.head || document.documentElement).appendChild(style);
  }

  // ---------- 设置 ----------
  function loadSettings() {
    try {
      if (!ctxAliveNow()) {
        handleContextInvalidated();
        return;
      }
      chrome.storage.sync.get({ autoBubble: true, bubbleTimeout: 0, srcLang: 'auto', tgtLang: 'zh' }, function (items) {
        if (!chrome.runtime.lastError && items) {
          Object.assign(settings, items);
          settingsLoaded = true;
          console.log('[WINOCR] 设置已加载:', JSON.stringify(settings));
        }
      });
    } catch (e) { console.warn('[WINOCR] loadSettings 异常:', e); }
  }
  loadSettings();

  // ---------- 接收来自 background 的框选截图指令（Alt+1 快捷键触发） ----------
  chrome.runtime.onMessage.addListener(function (msg, sender, sendResponse) {
    if (!msg) return;
    if (msg.type === 'start-region-select') {
      startRegionSelect(msg.img);
      sendResponse({ ok: true });
    } else if (msg.type === 'show-bubble') {
      showBubble(msg.src || '', msg.tgt || '', msg.x || 0, msg.y || 0);
      sendResponse({ ok: true });
    }
  });

  chrome.storage.onChanged.addListener(function (changes, area) {
    if (area !== 'sync') return;
    Object.keys(changes).forEach(function (k) { if (k in settings) settings[k] = changes[k].newValue; });
  });

  function calcPosition(x, y, bw, bh) {
    var W = window.innerWidth, H = window.innerHeight;
    var left = Math.max(8, Math.min(x, W - bw - 8));
    var top = y + 14;
    if (top + bh > H - 8) top = y - bh - 12;
    if (top < 8) top = 8;
    return { left: left, top: top };
  }

  function removeBubble() {
    if (bubbleEl && bubbleEl.parentNode) bubbleEl.remove();
    bubbleEl = null;
  }

  // 扩展上下文失效（扩展被重新加载/更新）：清理页面上所有 WinOCR 残留 UI，
  // 避免旧脚本继续展示不可用的气泡/标记。
  function handleContextInvalidated() {
    try {
      removeBubble();
      var marker = document.getElementById('winocr-load-marker');
      if (marker && marker.parentNode) marker.remove();
      var overlay = document.getElementById('winocr-region-overlay');
      if (overlay && overlay.parentNode) overlay.remove();
      var pdfBubble = document.getElementById('winocr-pdf-bubble');
      if (pdfBubble && pdfBubble.parentNode) pdfBubble.remove();
    } catch (_) {}
  }

  // 在气泡头部显示翻译引擎状态（引擎名 + 连接状态）
  function setEngineInfo(r) {
    if (!bubbleEl) return;
    var el = bubbleEl.querySelector('.wb-engine');
    if (!el) return;
    if (!r || !r.ok) {
      el.style.display = 'none';
      return;
    }
    var label = r.label || r.engine || '未知引擎';
    var isHost = (r.engine === 'host');
    var isLocalOcr = (r.engine === 'local-ocr');
    // host = 桌面端已连接；local-ocr = 本地 OCR；其它在线引擎 = 独立可用
    var statusText = isHost ? '桌面端已连接' : (isLocalOcr ? '本地模型' : '在线引擎');
    el.textContent = '⚡ 引擎：' + label + ' · ' + statusText;
    el.style.display = 'block';
  }

  // 安全发送消息：扩展被重新加载/更新后 content script 上下文会失效，
  // 此时直接调用 chrome.runtime.sendMessage 会抛 "Extension context invalidated"。
  // 这里统一做防御：上下文失效时静默失败，不抛未捕获异常。
  function ctxAliveNow() {
    try { return !!(typeof chrome !== 'undefined' && chrome.runtime && chrome.runtime.id); }
    catch (e) { return false; }
  }

  function safeSendMessage(msg, cb) {
    if (!ctxAliveNow()) {
      console.warn('[WINOCR] 扩展上下文已失效，跳过消息:', msg && msg.type);
      handleContextInvalidated();
      if (typeof cb === 'function') { try { cb(null); } catch (_) {} }
      return;
    }
    try {
      chrome.runtime.sendMessage(msg, function (r) {
        // 回调触发时若上下文已失效，lastError 会置为 "Extension context invalidated"
        if (chrome.runtime.lastError) {
          console.warn('[WINOCR] 消息响应错误:', chrome.runtime.lastError.message, msg && msg.type);
          if (String(chrome.runtime.lastError.message || '').indexOf('context invalidated') !== -1) {
            handleContextInvalidated();
          }
          if (typeof cb === 'function') { try { cb(null); } catch (_) {} }
          return;
        }
        if (typeof cb === 'function') { try { cb(r); } catch (_) {} }
      });
    } catch (e) {
      console.warn('[WINOCR] 发送消息异常:', e && e.message, msg && msg.type);
      if (e && String(e.message || '').indexOf('context invalidated') !== -1) {
        handleContextInvalidated();
      }
      if (typeof cb === 'function') { try { cb(null); } catch (_) {} }
    }
  }

  function clearHideTimer() { if (hideTimer) { clearTimeout(hideTimer); hideTimer = null; } }

  // 常显示模式（bubbleTimeout=0）：气泡不自动关闭，只有用户点「关闭」或点击页面其它处才消失。
  // 非零时：显示指定时长后自动关闭。
  function scheduleHide(ms) {
    clearHideTimer();
    if (ms === 0) return; // 常显示
    var secs = (typeof ms === 'number') ? ms / 1000 : (settings.bubbleTimeout || 0);
    if (secs <= 0) return;  // 常显示
    var t = Math.max(1000, Math.min(30000, secs * 1000));
    hideTimer = setTimeout(removeBubble, t);
  }

  // ---------- 气泡 ----------
  function showBubble(src, tgt, x, y) {
    injectStyles();
    bubbleCreatedAt = Date.now(); // 记录创建时间，防止双击选词被 mousedown 误关
    var bw = Math.min(560, window.innerWidth - 16);
    if (bubbleEl && bubbleEl.parentNode && bubbleEl.__text === src) {
      if (tgt) bubbleEl.querySelector('.wb-tgt').textContent = tgt;
      return bubbleEl;
    }
    removeBubble();

    bubbleEl = document.createElement('div');
    bubbleEl.className = 'winocr-bubble';
    bubbleEl.__text = src;

    // 用户拖动过气泡 → 新气泡出现在上次拖到的位置（clamp 在视口内）；否则按选区定位
    var pos;
    if (bubblePos) {
      pos = {
        left: Math.max(8, Math.min(bubblePos.left, window.innerWidth - bw - 8)),
        top: Math.max(8, Math.min(bubblePos.top, window.innerHeight - 120))
      };
    } else {
      pos = calcPosition(x, y, bw, 180);
    }
    bubbleEl.style.left = pos.left + 'px';
    bubbleEl.style.top = pos.top + 'px';
    bubbleEl.style.maxWidth = bw + 'px';

    bubbleEl.innerHTML = '<div class="wb-head"><span>WinOCR 划词助手</span><span><span class="wb-dot' + (connected ? '' : ' off') + '"></span>' + (connected ? '已连接' : '未连接') + '</span></div><div class="wb-engine" style="display:none"></div><div class="wb-body"><div class="wb-src"></div><div class="wb-tgt"></div></div><div class="wb-actions"><button data-act="translate" class="primary">翻译</button><button data-act="collect">采集生词</button><button data-act="copy">复制</button><button data-act="tts-en" class="tts" title="朗读原文（英文）">英</button><button data-act="tts-zh" class="tts" title="朗读译文（中文）">中</button><button data-act="tts-stop" class="tts-stop" title="停止朗读">■</button><button data-act="close">关闭</button></div>';

    bubbleEl.querySelector('.wb-src').textContent = src;
    bubbleEl.querySelector('.wb-tgt').textContent = tgt || '（点击「翻译」获取译文）';

    bubbleEl.addEventListener('mousedown', function (e) {
      if (e.target && (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA')) return;
      e.preventDefault();
    });

    // 标题栏拖动：按住「WinOCR 划词助手」标题栏可移动气泡；
    // 位置记忆到 bubblePos，后续新气泡出现在同一处（跨页面，会话内有效）。
    var head = bubbleEl.querySelector('.wb-head');
    head.addEventListener('mousedown', function (e) {
      e.preventDefault();
      e.stopPropagation();
      draggingBubble = true;
      var sx = e.clientX, sy = e.clientY;
      var rect = bubbleEl.getBoundingClientRect();
      var ox = sx - rect.left, oy = sy - rect.top;
      function onMove(ev) {
        if (!bubbleEl) return;
        var bw2 = bubbleEl.offsetWidth, bh2 = bubbleEl.offsetHeight;
        var left = Math.max(0, Math.min(ev.clientX - ox, window.innerWidth - bw2));
        var top = Math.max(0, Math.min(ev.clientY - oy, window.innerHeight - 32));
        bubbleEl.style.left = left + 'px';
        bubbleEl.style.top = top + 'px';
        bubblePos = { left: left, top: top };
      }
      function onUp() {
        draggingBubble = false;
        document.removeEventListener('mousemove', onMove, true);
        document.removeEventListener('mouseup', onUp, true);
      }
      document.addEventListener('mousemove', onMove, true);
      document.addEventListener('mouseup', onUp, true);
    });

    bubbleEl.addEventListener('click', function (e) {
      var btn = e.target.closest('button');
      if (!btn) return;
      var act = btn.dataset.act;
      if (act === 'translate') {
        btn.disabled = true; btn.textContent = '翻译中…';
        safeSendMessage({ type: 'translate', src: src, srcLang: currentSelection.srcLang, tgtLang: currentSelection.tgtLang }, function (r) {
          btn.disabled = false; btn.textContent = '翻译';
          if (!bubbleEl) return;
          bubbleEl.querySelector('.wb-tgt').textContent = (r && r.ok && r.tgt) ? r.tgt : '（翻译失败：' + ((r && r.error) || '未知错误') + '）';
          setEngineInfo(r);
        });
      } else if (act === 'collect') {
        btn.disabled = true;
        var curTgtText = bubbleEl.querySelector('.wb-tgt').textContent || '';
        if (curTgtText && curTgtText.indexOf('（') === 0) curTgtText = ''; // 忽略占位/错误文案
        safeSendMessage({ type: 'collect', src: src, tgt: curTgtText || currentSelection.tgt || '', tags: [], srcLang: currentSelection.srcLang, tgtLang: currentSelection.tgtLang }, function (r) {
          btn.disabled = false;
          if (r && r.ok) {
            btn.textContent = r.cached ? '已缓存 ✓' : '已采集 ✓';
            // 回传 data.count / data.items：显示「过滤出的生词」原文+译文窗口
            showCollectResult(src, curTgtText || currentSelection.tgt || '', '', true, '', r, r.cached);
          } else {
            btn.textContent = '采集失败';
            showCollectResult(src, curTgtText || currentSelection.tgt || '', '', false, (r && r.error) || '未知错误', r);
          }
        });
      } else if (act === 'copy') {
        navigator.clipboard.writeText(tgt || src).then(function () { btn.textContent = '已复制 ✓'; }).catch(function () {});
      } else if (act === 'tts-en' || act === 'tts-zh') {
        var textToSpeak = act === 'tts-en' ? src : (tgt || src);
        var langToSpeak = act === 'tts-en' ? 'en-US' : 'zh-CN';
        speakText(textToSpeak, langToSpeak, btn);
      } else if (act === 'tts-stop') {
        stopSpeaking();
      } else if (act === 'close') {
        removeBubble();
      }
    });

    document.documentElement.appendChild(bubbleEl);
    console.log('[WINOCR] 气泡已创建');
    return bubbleEl;
  }

  // ---------- 朗读（Web Speech API）----------
  function pickVoiceForLang(lang) {
    if (!('speechSynthesis' in window)) return null;
    var voices = speechSynthesis.getVoices();
    var prefix = (lang || 'en').split('-')[0].toLowerCase();
    var match = voices.find(function (v) { return v.lang && v.lang.toLowerCase().startsWith(prefix); });
    return match || null;
  }

  function speakText(text, lang, btn) {
    if (!text || !text.trim()) return;
    if (!('speechSynthesis' in window)) {
      alert('当前浏览器不支持朗读（Web Speech API）');
      return;
    }
    // Edge 某些版本需先 cancel 再 speak
    window.speechSynthesis.cancel();
    var u = new SpeechSynthesisUtterance(text);
    u.lang = lang;
    u.rate = 0.9;
    var voice = pickVoiceForLang(lang);
    if (voice) u.voice = voice;
    if (btn) btn.classList.add('speaking');
    u.onend = function () {
      currentUtterance = null;
      clearSpeakingState();
    };
    u.onerror = function () {
      currentUtterance = null;
      clearSpeakingState();
    };
    currentUtterance = u;
    window.speechSynthesis.speak(u);
  }

  function clearSpeakingState() {
    if (!bubbleEl) return;
    var tts = bubbleEl.querySelectorAll('.wb-actions button.tts');
    if (tts) tts.forEach(function (b) { b.classList.remove('speaking'); });
  }

  // 截图翻译（Alt+1 快捷键）：接收 background 截下的全视口图 → 十字线框选 → 裁剪 → OCR 翻译。
  // 结果直接复用划词气泡 showBubble，保证界面与功能完全一致。
  function startRegionSelect(dataUrl) {
    if (document.getElementById('winocr-region-overlay')) return; // 防重复

    const dpr = window.devicePixelRatio || 1;

    const ov = document.createElement('div');
    ov.id = 'winocr-region-overlay';
    ov.style.cssText = 'position:fixed;left:0;top:0;width:100vw;height:100vh;z-index:2147483647;cursor:crosshair;user-select:none';

    const box = document.createElement('div');
    box.style.cssText = 'position:absolute;display:none;border:1px solid #1677ff;box-shadow:0 0 0 100000px rgba(0,0,0,.25);background:transparent;pointer-events:none';

    const hline = document.createElement('div');
    hline.style.cssText = 'position:absolute;left:0;width:100%;height:1px;background:rgba(22,119,255,.65);pointer-events:none';

    const vline = document.createElement('div');
    vline.style.cssText = 'position:absolute;top:0;width:1px;height:100%;background:rgba(22,119,255,.65);pointer-events:none';

    const tag = document.createElement('div');
    tag.style.cssText = 'position:absolute;display:none;background:#1677ff;color:#fff;font:11px/1.7 system-ui,sans-serif;padding:0 6px;border-radius:3px;pointer-events:none;white-space:nowrap';

    ov.appendChild(box); ov.appendChild(hline); ov.appendChild(vline); ov.appendChild(tag);
    document.documentElement.appendChild(ov);

    let sx = 0, sy = 0, dragging = false, done = false;
    function cleanup() { done = true; window.removeEventListener('keydown', onKey, true); ov.remove(); }
    function onKey(e) { if (e.key === 'Escape') cleanup(); }
    window.addEventListener('keydown', onKey, true);

    ov.addEventListener('mousedown', (e) => {
      if (e.button !== 0) return;
      dragging = true; sx = e.clientX; sy = e.clientY;
      box.style.display = 'block'; tag.style.display = 'block';
      e.preventDefault(); e.stopPropagation();
    });
    ov.addEventListener('mousemove', (e) => {
      if (done) return;
      hline.style.top = e.clientY + 'px';
      vline.style.left = e.clientX + 'px';
      if (!dragging) return;
      const x = Math.min(sx, e.clientX), y = Math.min(sy, e.clientY);
      const w = Math.abs(e.clientX - sx), h = Math.abs(e.clientY - sy);
      box.style.left = x + 'px'; box.style.top = y + 'px';
      box.style.width = w + 'px'; box.style.height = h + 'px';
      tag.textContent = w + ' × ' + h;
      tag.style.left = Math.max(0, x + w - 64) + 'px';
      tag.style.top = Math.max(0, y - 22) + 'px';
    });
    ov.addEventListener('mouseup', (e) => {
      if (!dragging || done) return;
      const x = Math.min(sx, e.clientX), y = Math.min(sy, e.clientY);
      const w = Math.abs(e.clientX - sx), h = Math.abs(e.clientY - sy);
      cleanup();
      if (w < 5 || h < 5) return; // 点击/拖动过小视为取消

      const img = new Image();
      img.onload = () => {
        const c = document.createElement('canvas');
        c.width = Math.max(1, Math.round(w * dpr));
        c.height = Math.max(1, Math.round(h * dpr));
        const ctx = c.getContext('2d');
        ctx.drawImage(img, Math.round(x * dpr), Math.round(y * dpr), c.width, c.height, 0, 0, c.width, c.height);
        const b64 = c.toDataURL('image/jpeg', 0.85).split(',')[1];

        // 先用划词气泡显示「识别中」，与划词体验一致
        currentSelection = { text: '', srcLang: 'auto', tgtLang: settings.tgtLang };
        showBubble('', '（OCR 识别中…）', x, y);

        safeSendMessage({ type: 'ocr_translate', img: b64 }, (r) => {
          if (!bubbleEl) return;
          const srcEl = bubbleEl.querySelector('.wb-src');
          const tgtEl = bubbleEl.querySelector('.wb-tgt');
          if (r && r.ok) {
            bubbleEl.__text = r.src_text || '';
            currentSelection = { text: r.src_text || '', srcLang: 'auto', tgtLang: settings.tgtLang };
            if (srcEl) srcEl.textContent = r.src_text || '';
            if (tgtEl) tgtEl.textContent = r.tgt || '（无译文）';
          } else {
            if (tgtEl) tgtEl.textContent = '（OCR 失败：' + ((r && r.error) || (chrome.runtime.lastError && chrome.runtime.lastError.message) || '未知错误') + '）';
          }
          // 显示 OCR 翻译引擎状态（本地 OCR / 桌面端回退）
          setEngineInfo(r);
        });
      };
      img.src = dataUrl;
    });
  }

  // 采集结果卡片样式（独立 style 元素注入，避免改动上方长字符串）
  var RESULT_CSS = '.winocr-bubble .wb-collect-result{margin-top:10px;border:1px solid #a7f3d0;background:#ecfdf5;border-radius:8px;overflow:hidden}.winocr-bubble .wb-collect-result.err{border-color:#fca5a5;background:#fef2f2}.winocr-bubble .wb-collect-result .wb-cr-head{padding:8px 12px;background:#10b981;color:#fff;font-size:14px;font-weight:600}.winocr-bubble .wb-collect-result.err .wb-cr-head{background:#ef4444}.winocr-bubble .wb-collect-result .wb-cr-row{display:flex;gap:8px;padding:6px 12px;font-size:14px;line-height:1.5}.winocr-bubble .wb-collect-result .wb-cr-k{flex:0 0 44px;color:#059669;font-weight:600}.winocr-bubble .wb-collect-result .wb-cr-v{flex:1;color:#064e3b;word-break:break-word}.winocr-bubble .wb-collect-result.err .wb-cr-k{color:#b91c1c}.winocr-bubble .wb-collect-result.err .wb-cr-v{color:#7f1d1d}.winocr-bubble .wb-collect-result .wb-cr-tip{padding:6px 12px 10px;font-size:11px;color:#047857}.winocr-bubble .wb-collect-result .wb-cr-words{display:flex;flex-wrap:wrap;gap:4px 6px}.winocr-bubble .wb-collect-result .wb-cr-word{display:inline-block;background:#a7f3d0;color:#065f46;border-radius:4px;padding:1px 6px;font-size:12px;font-weight:600}.winocr-bubble .wb-engine{padding:6px 16px;background:#eef2ff;color:#4338ca;border-bottom:1px solid #e0e7ff;font-size:12px;line-height:1.5;word-break:break-word}';

  function injectResultStyles() {
    if (document.getElementById('winocr-result-styles')) return;
    var style = document.createElement('style');
    style.id = 'winocr-result-styles';
    style.textContent = RESULT_CSS;
    (document.head || document.documentElement).appendChild(style);
  }

  function escapeHtml(s) {
    return String(s == null ? '' : s)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
  }

  // 采集结果界面：成功显示 原文/译文/分组 + 过滤出的生词列表 + 已保存；
// 失败显示红色错误卡片 + 原因。常驻气泡内，不自动淡出。
function showCollectResult(word, tgt, tag, ok, error, resp, cached) {
    if (!bubbleEl) return;
    injectResultStyles();
    var body = bubbleEl.querySelector('.wb-body');
    if (!body) return;
    var existing = bubbleEl.querySelector('.wb-collect-result');
    if (existing) existing.remove();

    var div = document.createElement('div');
    div.className = 'wb-collect-result' + (ok ? '' : ' err');
    var head = ok
      ? (cached ? '✓ 已缓存到本地（待同步）' : '✓ 已采集到生词本')
      : '✗ 采集失败';
    var rows;
    if (ok) {
      var tgtHtml = tgt ? escapeHtml(tgt) : '<span style="color:#94a3b8">（无译文）</span>';
      var tagHtml = (tag && tag.trim()) ? escapeHtml(tag) : '未分组';
      rows = '<div class="wb-cr-row"><span class="wb-cr-k">原文</span><span class="wb-cr-v">' + escapeHtml(word) + '</span></div>' +
             '<div class="wb-cr-row"><span class="wb-cr-k">译文</span><span class="wb-cr-v">' + tgtHtml + '</span></div>' +
             '<div class="wb-cr-row"><span class="wb-cr-k">分组</span><span class="wb-cr-v">' + tagHtml + '</span></div>';
      // 过滤出的生词明细（host 端 ingest_text 拆分/过滤后实际入库的词；缓存时仅原始词）
      var items = (resp && resp.data && resp.data.items) || [];
      var count = (resp && resp.data && typeof resp.data.count === 'number') ? resp.data.count : items.length;
      if (items && items.length > 0) {
        var lis = items.map(function (it) {
          return '<span class="wb-cr-word">' + escapeHtml(it.word) + '</span>';
        }).join('');
        rows += '<div class="wb-cr-row"><span class="wb-cr-k">过滤生词</span><span class="wb-cr-v wb-cr-words">' + lis + '</span></div>';
      } else {
        rows += '<div class="wb-cr-row"><span class="wb-cr-k">过滤生词</span><span class="wb-cr-v">' + (count > 0 ? count + ' 个词' : '（无新词）') + '</span></div>';
      }
      if (cached) {
        rows += '<div class="wb-cr-tip" style="background:#fffbeb;border-top:1px solid #fde68a;color:#92400e">桌面端 WinOCR 未运行，生词已暂存在浏览器本地；下次桌面端启动并连接后自动补传。</div>';
      } else {
        rows += '<div class="wb-cr-tip">共采集 ' + count + ' 个词 · 在桌面端 WinOCR「采录」页查看与复习</div>';
      }
    } else {
      rows = '<div class="wb-cr-row"><span class="wb-cr-k">原因</span><span class="wb-cr-v">' + escapeHtml(error || '未知错误') + '</span></div>';
    }
    div.innerHTML = '<div class="wb-cr-head">' + head + '</div>' + rows;
    body.appendChild(div);
  }

  function stopSpeaking() {
    if ('speechSynthesis' in window) window.speechSynthesis.cancel();
    currentUtterance = null;
    clearSpeakingState();
  }

  // ---------- 划词检测 ----------
  function onSelect(e) {
    console.log('[WINOCR] mouseup 事件触发');
    if (draggingBubble) return; // 拖动气泡结束的 mouseup 不做划词检测
    if (e && e.target && e.target.closest && e.target.closest('.winocr-bubble')) return;

    setTimeout(function () {
      // 设置未加载完成时，等待片刻再读取（避免默认 autoBubble=false 误判）
      if (!settingsLoaded) {
        try {
          chrome.storage.sync.get({ autoBubble: true }, function (items) {
            if (!chrome.runtime.lastError && items) {
              Object.assign(settings, items);
              settingsLoaded = true;
            }
            processSelection();
          });
          return;
        } catch (_) {}
      }
      processSelection();
    }, 80);
  }

  function processSelection() {
    var sel = window.getSelection();
    var text = sel ? sel.toString().trim() : '';
    console.log('[WINOCR] 选区文本="' + text + '" 长度=' + text.length);
    if (!text || text.length > 500) { return; }

    var hasCJK = /[一-鿿]/.test(text);
    currentSelection.srcLang = (settings.srcLang === 'auto') ? (hasCJK ? 'zh' : 'en') : settings.srcLang;
    currentSelection.tgtLang = settings.tgtLang;

    if (!settings.autoBubble) { console.log('[WINOCR] autoBubble=false，跳过'); return; }

    var rect = { left: 80, bottom: 80 };
    try { rect = sel.getRangeAt(0).getBoundingClientRect(); } catch (_) {}

    console.log('[WINOCR] 显示气泡 at', JSON.stringify(rect));
    showBubble(text, '', rect.left, rect.bottom);
    var trBtn = bubbleEl && bubbleEl.querySelector('[data-act="translate"]');
    if (trBtn) trBtn.click();
  }

  document.addEventListener('mouseup', onSelect);
  document.addEventListener('mousedown', function (e) {
    if (e.target && e.target.closest && e.target.closest('.winocr-bubble')) return;
    // 气泡刚创建（例如双击选词的第二击）时忽略，避免气泡一闪而过
    if (Date.now() - bubbleCreatedAt < 300) return;
    removeBubble();
  });
  document.addEventListener('selectionchange', function () {
    if (!bubbleEl) return;
    if (!window.getSelection().toString().trim()) clearHideTimer();
  });

  // ---------- 消息 ----------
  chrome.runtime.onMessage.addListener(function (msg) {
    if (!msg || typeof msg !== 'object') return;
    if (msg.type === 'conn-status') {
      connected = !!msg.connected;
      if (bubbleEl) {
        var dot = bubbleEl.querySelector('.wb-dot');
        if (dot) dot.className = 'wb-dot' + (connected ? '' : ' off');
      }
    } else if (msg.type === 'show-bubble') {
      showBubble(msg.src, msg.tgt || '', window.innerWidth / 2, window.innerHeight / 2);
    }
  });

  safeSendMessage({ type: 'get-status' }, function (r) {
    if (r && typeof r.connected === 'boolean') connected = r.connected;
  });

  // ---------- PDF / 特殊页面 fallback ----------
  // 内置 PDF 查看器可能无法触发 mouseup（PDF plugin 区域拦截事件），
  // 此时靠 selectionchange 兜底
  var pdfCheckTimer = null;
  function checkPdfSelection() {
    try {
      var sel = window.getSelection();
      if (!sel) return;
      var text = sel.toString().trim();
      if (!text || text.length > 500) return;
      if (text === lastPdfSelection) return;
      lastPdfSelection = text;
      var hasCJK = /[一-鿿]/.test(text);
      currentSelection.srcLang = (settings.srcLang === 'auto') ? (hasCJK ? 'zh' : 'en') : settings.srcLang;
      currentSelection.tgtLang = settings.tgtLang;
      if (!settings.autoBubble) return;
      try {
        var rect = sel.getRangeAt(0).getBoundingClientRect();
        showBubble(text, '', rect.left, rect.bottom);
      } catch (_) {
        showBubble(text, '', 80, 80);
      }
    } catch (_) {}
  }
  var lastPdfSelection = '';

  // 页面 URL 含 .pdf 或 content-type 为 PDF 时启动兜底
  if (location.href.match(/\.pdf($|\?|#)/i) || document.contentType === 'application/pdf') {
    console.log('[WINOCR] PDF 页面 detected, 启动 selectionchange fallback');
    document.addEventListener('selectionchange', function () {
      clearTimeout(pdfCheckTimer);
      pdfCheckTimer = setTimeout(checkPdfSelection, 300);
    });
  }

  console.log('[WINOCR] content script 初始化完成');
})();
